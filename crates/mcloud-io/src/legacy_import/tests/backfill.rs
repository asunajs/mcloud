// 由 legacy_import.rs 的单元测试按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::Scope;
use mcloud_core::{ConfigStore, StateStore};
use mcloud_core::{ScheduleStatus, ScheduleTarget};

/// 凭据回填要能补**老库**，反复跑要安全，且**绝不把新凭据换成旧的**。
///
/// 三条合起来才说明这条回填可以无条件地"每次 `migrate` 都跑"。
/// 特别是第三条：`write_state` 走的是整体覆盖，若它把 `authorization` 一起写成
/// `NULL`，后面那条回填就会把用户重登后的新串换成 `asign.json` 里的旧串 ——
/// 而那时没有任何线索指向这里。
#[tokio::test]
async fn credentials_backfill_reaches_old_databases_without_downgrading() {
    let dir = copy_fixtures();
    let s = store();
    let source = source_of(&dir);

    // ① 第一次导入：两个账号都补上
    let first = run_import(&s, &source).await.unwrap();
    assert_eq!(first.credentials_filled, 2);
    let before = s
        .get_account_state(Scope::user(&uid()), &id(ACC_1))
        .await
        .unwrap()
        .unwrap();
    let stored = before.authorization.clone().expect("整串应已落库");

    // ② 源文件全都没变时再跑一遍：回填**照样执行**（不受哈希门控），但没缺口可补
    let second = run_import(&s, &source).await.unwrap();
    assert_eq!(second.accounts, 0);
    assert_eq!(
        second.credentials_filled, 0,
        "没缺口就不该计数 —— 否则这个数字第二次运行起就没有意义了"
    );

    // ③ 模拟"用户重登了一次"：登录路径是整体覆盖写，这里照做
    const FRESH: &str = "Basic MjoxOTkwMDAwMDAwMTpGUkVTSC1UT0tFTg==";
    let mut fresh = before.clone();
    fresh.authorization = Some(FRESH.into());
    s.upsert_account_state(Scope::user(&uid()), &fresh)
        .await
        .unwrap();

    // ④ 让 `state.json` 变一下，逼 `write_state` 真的跑一次
    let text = std::fs::read_to_string(PathBuf::from(FIXTURES).join("state.json")).unwrap();
    let touched = text.replacen('{', "{\n  \"_\": 1,", 1);
    write_file(&dir, "state.json", &touched);

    let third = run_import(&s, &source).await.unwrap();
    assert!(
        !third.skipped_unchanged.contains(&"state.json".to_string()),
        "state.json 变了，`write_state` 必须真的跑过，否则这条测试测不到东西"
    );
    assert_eq!(third.credentials_filled, 0, "库里已有整串，回填不该再动它");
    let after = s
        .get_account_state(Scope::user(&uid()), &id(ACC_1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.authorization.as_deref(),
        Some(FRESH),
        "重登后的新凭据不能被 asign.json 里的旧串换掉（现在是 {stored:?} 那种）"
    );
}

#[tokio::test]
async fn state_only_backfills_account_state_not_accounts() {
    let dir = copy_fixtures();
    let s = store();
    // 先只导入账号（把别的数据文件都删掉），模拟"账号清单早就导完了"
    for name in LEGACY_DATA_FILES {
        remove_file(&dir, name);
    }
    run_import(&s, &source_of(&dir)).await.unwrap();
    assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);

    // 再把 state.json 放回来：这次只该补登录态
    let text = std::fs::read_to_string(PathBuf::from(FIXTURES).join("state.json")).unwrap();
    write_file(&dir, "state.json", &text);

    let report = run_import(&s, &source_of(&dir)).await.unwrap();
    assert_eq!(report.accounts, 0, "asign.json 没变就不重写账号");
    assert_eq!(report.account_states, 2);
    assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
    assert!(
        s.get_account_state(Scope::user(&uid()), &id(ACC_1))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn orphan_and_empty_state_entries_are_reported_not_invented() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "state.json",
        r#"{
  "accounts": {
"19900000001": { "jwtToken": "t", "deviceId": "d" },
"17700000000": { "jwtToken": "不该被导入" },
"19900000002": { "jwtToken": "", "deviceId": "", "userDomainId": "", "cloudPhoneRedpackToken": "" },
"19900000003": { "jwtToken": "t", "新字段": 1 }
  }
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    assert_eq!(report.account_states, 1, "只有 19900000001 是可用的");
    assert_eq!(
        report.credentials_filled, 2,
        "两个账号在 asign.json 里都有 auth，都该被回填"
    );
    assert!(
        s.get_account_state(Scope::user(&uid()), &id(ACC_1))
            .await
            .unwrap()
            .is_some()
    );
    // `19900000002` 在 `state.json` 里是**空条目**（被报出来，没贡献任何登录态），
    // 但它在 `asign.json` 里有 `auth` —— 于是凭据回填会为它造出一行。
    //
    // 这**不是**"凭空造登录态"：`jwt_token` / `device_id` 仍然全是 `NULL`，
    // 回填进去的只有 `authorization` 与 `phone`，两样都来自 `asign.json`。
    // 这条断言把界线钉住：空条目贡献 0，凭据回填贡献那两列。
    let acc2 = s
        .get_account_state(Scope::user(&uid()), &id(ACC_2))
        .await
        .unwrap()
        .expect("asign.json 里有 auth，回填就该把这一行补上");
    assert_eq!(acc2.jwt_token, None, "空 state 条目不该贡献任何登录态");
    assert_eq!(acc2.device_id, None);
    assert_eq!(acc2.user_domain_id, None);
    assert_eq!(acc2.phone.as_deref(), Some("19900000002"));
    assert!(
        acc2.authorization
            .as_deref()
            .is_some_and(|a| a.starts_with("Basic ")),
        "存进去的必须是规范化后的整串：{:?}",
        acc2.authorization
    );
    assert!(
        s.list_accounts(Scope::user(&uid()))
            .await
            .unwrap()
            .iter()
            .all(|a| a.id.as_str() != "17****00"),
        "找不到归属的登录态不能凭空造账号"
    );

    let lines = report.issue_lines();
    assert!(lines.iter().any(|l| l.contains("找不到")));
    assert!(lines.iter().any(|l| l.contains("状态是空的")));
    assert!(
        lines.iter().any(|l| l.contains("新字段")),
        "未识别字段要如实报出而不是静默丢弃：{lines:#?}"
    );
}

#[tokio::test]
async fn schedule_last_status_is_mapped_or_dropped() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "schedules.json",
        r#"{
  "schedules": [
{ "id": "s1", "name": "a", "task": "sign", "cron": "* * * * * *", "lastStatus": "error", "lastError": "上游 500" },
{ "id": "s2", "name": "b", "task": "sign", "cron": "* * * * * *", "lastStatus": "skipped" },
{ "id": "s3", "name": "c", "task": "sign", "cron": "* * * * * *", "lastStatus": "从没见过的状态" }
  ]
}"#,
    );

    let s = store();
    run_import(&s, &source_of(&dir)).await.unwrap();

    assert_eq!(
        s.get_schedule(Scope::user(&uid()), "s1")
            .await
            .unwrap()
            .unwrap()
            .last_status,
        Some(ScheduleStatus::Error)
    );
    assert_eq!(
        s.get_schedule(Scope::user(&uid()), "s1")
            .await
            .unwrap()
            .unwrap()
            .last_error
            .as_deref(),
        Some("上游 500")
    );
    assert_eq!(
        s.get_schedule(Scope::user(&uid()), "s2")
            .await
            .unwrap()
            .unwrap()
            .last_status,
        Some(ScheduleStatus::Skipped)
    );
    // 认不出来的状态宁可为空，也不该猜一个
    assert_eq!(
        s.get_schedule(Scope::user(&uid()), "s3")
            .await
            .unwrap()
            .unwrap()
            .last_status,
        None
    );
}

#[tokio::test]
async fn schedule_prefers_the_pipeline_target_when_both_are_present() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "schedules.json",
        r#"{
  "schedules": [
{ "id": "s1", "name": "两个目标都有", "task": "sign", "pipelineId": "p1", "cron": "0 0 8 * * *" },
{ "id": "s2", "name": "只有 pipelineId", "pipelineId": " p1 ", "cron": "0 0 8 * * *" }
  ]
}"#,
    );

    let s = store();
    run_import(&s, &source_of(&dir)).await.unwrap();

    for id in ["s1", "s2"] {
        assert_eq!(
            s.get_schedule(Scope::user(&uid()), id)
                .await
                .unwrap()
                .unwrap()
                .target,
            Some(ScheduleTarget::Pipeline { id: "p1".into() }),
            "旧版优先 pipelineId；{id} 的目标不对"
        );
    }
}

#[tokio::test]
async fn records_without_ids_get_stable_fallback_ids() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "pipelines.json",
        r#"{ "pipelines": [ { "name": "没 id 的组合", "steps": [{ "task": "sign" }] } ] }"#,
    );
    write_file(
        &dir,
        "schedules.json",
        r#"{ "schedules": [ { "name": "没 id 的任务", "task": "sign", "cron": "0 0 8 * * *" } ] }"#,
    );

    let s = store();
    run_import(&s, &source_of(&dir)).await.unwrap();
    let first_pipeline = s.list_pipelines(Scope::user(&uid())).await.unwrap()[0]
        .id
        .clone();
    let first_schedule = s.list_schedules(Scope::user(&uid())).await.unwrap()[0]
        .id
        .clone();

    assert!(first_pipeline.starts_with("legacy-pipe-"));
    assert!(first_schedule.starts_with("legacy-sched-"));

    // 改内容触发重跑，兜底 id 必须不变 —— 否则幂等立刻失效
    write_file(
        &dir,
        "pipelines.json",
        r#"{ "pipelines": [ { "name": "没 id 的组合", "steps": [{ "task": "prizes" }] } ] }"#,
    );
    run_import(&s, &source_of(&dir)).await.unwrap();

    let pipelines = s.list_pipelines(Scope::user(&uid())).await.unwrap();
    assert_eq!(pipelines.len(), 1, "兜底 id 变了就会多出一行");
    assert_eq!(pipelines[0].id, first_pipeline);
    assert_eq!(
        s.list_schedules(Scope::user(&uid())).await.unwrap()[0].id,
        first_schedule
    );
}
