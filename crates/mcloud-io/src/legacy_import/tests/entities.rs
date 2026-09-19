// 由 legacy_import.rs 的单元测试按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::Scope;
use mcloud_core::{AccountMode, RunStatus, WEBUI_PAGES};
use mcloud_core::{ConfigStore, StateStore};

// ── 端到端 ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn fixture_import_lands_every_entity() {
    let s = store();
    let report = run_import(&s, &fixture_source())
        .await
        .expect("真实夹具应可导入");

    assert_eq!(report.accounts, 2);
    assert_eq!(report.account_states, 2);
    assert_eq!(report.pipelines, 1, "夹具里只有 1 条组合");
    assert_eq!(report.schedules, 0, "夹具的 schedules.json 是空的");
    assert_eq!(report.runs, 12, "夹具里有 12 条运行历史");
    assert!(
        report.skipped_unchanged.is_empty(),
        "首次导入不该跳过任何文件，实际跳过 {:?}",
        report.skipped_unchanged
    );
    assert!(
        !report.has_issues(),
        "干净的旧数据不该产生问题，实际：{:#?}",
        report.issue_lines()
    );

    // 落库结果与报告一致 —— 报告说导了却查不到是最坏的失败方式
    assert_eq!(s.list_accounts(Scope::user(&uid())).await.unwrap().len(), 2);
    assert_eq!(
        s.list_runs(Scope::user(&uid()), 100).await.unwrap().len(),
        12
    );
    assert_eq!(
        s.list_pipelines(Scope::user(&uid())).await.unwrap().len(),
        1
    );
    assert!(
        s.list_schedules(Scope::user(&uid()))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn accounts_are_keyed_by_mask_not_by_phone() {
    let s = store();
    run_import(&s, &fixture_source()).await.unwrap();

    let accounts = s.list_accounts(Scope::user(&uid())).await.unwrap();
    let ids: Vec<&str> = accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, vec![ACC_1, ACC_2], "应按配置顺序落库");

    // 手机号原文**不得**成为账号主键：那会把可识别信息带进所有下游表
    assert!(
        accounts.iter().all(|a| !a.id.as_str().contains("1990000")),
        "账号 id 不应是手机号原文"
    );

    let first = s
        .get_account(Scope::user(&uid()), &id(ACC_1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.nickname.as_deref(), Some("账号2"));
    assert_eq!(first.phone_masked.as_deref(), Some(ACC_1));
    assert_eq!(first.sort_order, 0);
    assert!(first.enabled);
    // 夹具的 expire 是 13 位毫秒（2030-01-01），不是 0，所以应当带过来
    assert_eq!(first.auth_expire, Some(1_893_456_000_000));
}

#[tokio::test]
async fn state_keys_are_remapped_through_the_identity_map() {
    let s = store();
    run_import(&s, &fixture_source()).await.unwrap();

    // state.json 的键是手机号原文；迁移后必须挂到掩码 id 上
    let state = s
        .get_account_state(Scope::user(&uid()), &id(ACC_1))
        .await
        .unwrap()
        .unwrap();
    assert!(state.jwt_token.is_some(), "凭据应随迁移一起搬过去");
    assert!(state.device_id.is_some());
    // userDomainId 是纯标识：运行期（库 / 日志 / Debug）保留 —— 它有排障价值且不可复用；
    // 但**进版本库的东西里必须是合成值**。界线是"能不能进 GitHub"，不是"能不能进日志"：
    // 日志是本地资产、随用随丢；仓库是全量留痕、推上去就收不回来。
    assert!(state.user_domain_id.is_some());
    assert!(state.cloud_phone_redpack_token.is_some());
    // 旧 state.json 没有时间戳，"导入时间"看台账
    assert!(state.updated_at.is_none());

    // 反向：拿手机号原文当 id 必须查不到东西
    assert!(
        s.get_account_state(Scope::user(&uid()), &id("19900000001"))
            .await
            .unwrap()
            .is_none(),
        "手机号原文不该成为 id"
    );
    assert!(
        !s.list_accounts(Scope::user(&uid()))
            .await
            .unwrap()
            .iter()
            .any(|a| a.id.as_str() == "19900000001")
    );
}

#[tokio::test]
async fn runs_attach_to_accounts_by_display_name() {
    let s = store();
    run_import(&s, &fixture_source()).await.unwrap();

    let runs = s.list_runs(Scope::user(&uid()), 100).await.unwrap();
    let mut per_account = std::collections::BTreeMap::new();
    for run in &runs {
        if let Some(account) = &run.account_id {
            *per_account.entry(account.as_str().to_string()).or_insert(0) += 1;
        }
    }
    assert_eq!(per_account.get(ACC_1), Some(&6), "「账号2」的 6 条历史");
    assert_eq!(per_account.get(ACC_2), Some(&6), "「账号1」的 6 条历史");
    assert!(
        runs.iter().all(|r| r.account_id.is_some()),
        "夹具里每条历史的展示名都应能唯一解析"
    );

    // 状态映射：ok=true → Done，ok=false → Failed
    assert_eq!(
        runs.iter().filter(|r| r.status == RunStatus::Done).count(),
        10
    );
    assert_eq!(
        runs.iter()
            .filter(|r| r.status == RunStatus::Failed)
            .count(),
        2
    );
    // 失败记录的 error 原文要搬到 message，否则失败原因就丢了
    assert!(runs.iter().any(|r| {
        r.message
            .as_deref()
            .is_some_and(|m| m.contains("prizeIdOrName"))
    }));
    // 旧数据从未采过云朵净增（原项目写入被注释），编不出来就留空
    assert!(
        runs.iter()
            .all(|r| r.cloud_gained.is_none() && r.cloud_after.is_none())
    );
}

#[tokio::test]
async fn pipeline_keeps_steps_order_and_defaults_to_sequential() {
    let s = store();
    run_import(&s, &fixture_source()).await.unwrap();

    let pipelines = s.list_pipelines(Scope::user(&uid())).await.unwrap();
    let pipeline = &pipelines[0];
    assert_eq!(pipeline.name, "轻量巡检");
    assert_eq!(
        pipeline
            .steps
            .iter()
            .map(|s| s.task.as_str())
            .collect::<Vec<_>>(),
        vec!["prizes", "receive"],
        "步骤顺序即执行顺序，不能重排"
    );
    assert_eq!(pipeline.account_mode, AccountMode::Sequential);
    assert!(pipeline.stop_on_error);
    assert!(
        pipeline.account_ids.is_empty(),
        "旧数据没指定 accounts = 全部账号"
    );
    assert_eq!(pipeline.last_duration_ms, Some(2481));
    // 旧 id 是 UUID，应当原样保留而不是换成兜底 id
    assert_eq!(pipeline.id, "84848ee7-3b94-483b-80fe-2ee33c93c7fe");
}

#[tokio::test]
async fn exchange_windows_and_webui_prefs_round_trip() {
    let s = store();
    run_import(&s, &fixture_source()).await.unwrap();

    let exchange = s.exchange_config().await.unwrap();
    assert_eq!(exchange.windows, vec!["09:59:40", "11:59:40"]);
    assert!(!exchange.enabled, "旧数据没有绑定，启用没有意义");
    assert!(exchange.bindings.is_empty());
    assert!(exchange.updated_at.is_some());

    let webui = s.webui_config().await.unwrap();
    // 旧版两个布尔字段的缺省方向相反，这里校验它被归一化成了确定值
    assert!(webui.remember_last_page, "true 原样搬过来");
    assert!(!webui.log_expanded);
    assert!(!webui.background_run);
    assert_eq!(webui.last_page, "tasks");
    assert!(WEBUI_PAGES.contains(&webui.last_page.as_str()));
}
