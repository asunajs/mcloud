// 由 legacy_import.rs 的单元测试按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::ConfigStore;
use mcloud_core::Scope;
use mcloud_core::{AccountMode, ScheduleTarget};

// ── 账号引用：绝不静默扩大范围 ─────────────────────────────────────────

#[tokio::test]
async fn phone_number_reference_is_recovered_and_flagged() {
    let dir = copy_fixtures();
    // 旧版把纯数字串当序号 → 19900000001 越界 → 从未生效。
    // 但它显然是手机号，且能唯一匹配，所以要恢复 + 报出来让人确认。
    write_file(
        &dir,
        "schedules.json",
        r#"{
  "schedules": [
{ "id": "s1", "name": "早班签到", "task": "sign", "cron": "0 0 8 * * *",
  "account": "19900000001", "enabled": true }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();
    assert_eq!(report.schedules, 1);

    let schedule = &s.list_schedules(Scope::user(&uid())).await.unwrap()[0];
    assert_eq!(schedule.account_ids, vec![id(ACC_1)], "恢复到了正确的账号");
    assert!(schedule.enabled, "恢复成功就不该停用它");
    assert_eq!(schedule.timezone, DEFAULT_TZ, "旧版缺省是上海");
    assert_eq!(
        schedule.target,
        Some(ScheduleTarget::Capability {
            code: "sign".into()
        })
    );
    assert_eq!(schedule.cron, "0 0 8 * * *");
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::RecoveredAccountRef { .. })),
        "恢复是行为变更，必须报出来：{:#?}",
        report.issue_lines()
    );
}

#[tokio::test]
async fn unresolvable_reference_disables_the_schedule_instead_of_broadening_it() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "schedules.json",
        r#"{
  "schedules": [
{ "id": "s1", "name": "幽灵账号的任务", "task": "sign", "cron": "0 0 8 * * *",
  "account": "早就删掉的账号", "enabled": true }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    let schedule = &s.list_schedules(Scope::user(&uid())).await.unwrap()[0];
    assert!(
        !schedule.enabled,
        "解析不到账号必须停用 —— 回落到'全部账号'等于把'过去什么都没跑'变成'对所有人跑'"
    );
    assert!(
        schedule.account_ids.is_empty(),
        "原有账号引用不可用时不能凭空补一组账号"
    );
    assert!(
        report.issues.iter().any(|i| matches!(
            i,
            MigrationIssue::DeadAccountRef { action, .. } if *action == "停用该定时任务"
        )),
        "{:#?}",
        report.issue_lines()
    );
}

#[tokio::test]
async fn schedule_without_target_or_cron_is_imported_disabled() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "schedules.json",
        r#"{
  "schedules": [
{ "id": "s1", "name": "没有目标", "cron": "0 0 8 * * *", "enabled": true },
{ "id": "s2", "name": "没有 cron", "task": "sign", "cron": "  ", "enabled": true },
{ "id": "s3", "name": "默认关闭", "task": "sign", "cron": "0 0 8 * * *" }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();
    assert_eq!(report.schedules, 3, "三条都要留下，靠 enabled 表达处置");

    let schedules = s.list_schedules(Scope::user(&uid())).await.unwrap();
    assert!(schedules.iter().all(|s| !s.enabled));
    assert_eq!(
        schedules
            .iter()
            .find(|s| s.name == "没有目标")
            .unwrap()
            .target,
        None
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::MissingTarget { .. }))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::EmptyCron { .. }))
    );
}

#[tokio::test]
async fn pipeline_binding_a_dead_account_is_skipped_whole() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "pipelines.json",
        r#"{
  "pipelines": [
{ "id": "p1", "name": "好组合", "steps": [{ "task": "prizes" }] },
{ "id": "p2", "name": "坏组合", "steps": [{ "task": "prizes" }], "accounts": ["查无此人"] },
{ "id": "p3", "name": "空组合", "steps": [] },
{ "id": "p4", "name": "混合引用", "steps": [{ "task": "sign", "account": "账号1" }],
  "accounts": [2, "19900000001"], "accountMode": "parallel" }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    assert_eq!(report.pipelines, 2, "好组合与混合引用各一条");
    let names: Vec<String> = s
        .list_pipelines(Scope::user(&uid()))
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, vec!["好组合", "混合引用"], "坏组合与空组合都不入库");

    let mixed = s
        .get_pipeline(Scope::user(&uid()), "p4")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mixed.account_ids,
        vec![id(ACC_2), id(ACC_1)],
        "序号 2 → 第 2 个账号；手机号 → 恢复成第 1 个账号"
    );
    assert_eq!(mixed.account_mode, AccountMode::Parallel);
    // 步骤级 account 已废弃，必须丢掉
    assert!(mixed.steps[0].body.is_none());
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::PipelineSkipped { .. }))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::EmptySteps { .. }))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::DroppedStepAccount { .. })),
        "被丢掉的废弃字段要说一声：{:#?}",
        report.issue_lines()
    );
}

#[tokio::test]
async fn empty_account_list_means_all_accounts() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "pipelines.json",
        r#"{
  "pipelines": [
{ "id": "p1", "name": "空数组", "steps": [{ "task": "sign" }], "accounts": [] },
{ "id": "p2", "name": "全是空值", "steps": [{ "task": "sign" }], "accounts": [null, ""] }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();
    assert_eq!(report.pipelines, 2);
    assert!(
        s.list_pipelines(Scope::user(&uid()))
            .await
            .unwrap()
            .iter()
            .all(|p| p.account_ids.is_empty()),
        "旧版 `if (accounts.length)` 为假 → 全部账号"
    );
    assert!(
        !report.has_issues(),
        "空列表是已知且正确的语义，不该报成问题：{:#?}",
        report.issue_lines()
    );
}
