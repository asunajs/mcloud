// 由 legacy_import.rs 的单元测试按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::ConfigStore;
use mcloud_core::WEBUI_PAGES;

// ── 抢兑与界面偏好 ─────────────────────────────────────────────────────

#[tokio::test]
async fn exchange_windows_are_normalized_and_duplicates_collapse() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "exchange.json",
        r#"{
  "enabled": true,
  "windows": ["11:59:40", " 09:59:40 ", "095940", "09:59:40"],
  "bindings": [
{ "account": "账号1", "prizeId": "42", "prizeName": "话费" },
{ "account": "账号1", "prizeId": 42, "prizeName": "重复绑定" },
{ "account": 1, "prizeId": 42 },
{ "account": "19900000002", "prizeId": 7 }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    let exchange = s.exchange_config().await.unwrap();
    assert_eq!(
        exchange.windows,
        vec!["09:59:40", "11:59:40"],
        "空白被裁掉、5 位补齐、重复折叠、整体排序"
    );
    assert!(exchange.enabled, "有绑定才谈得上启用");
    // 注意夹具里配置顺序是 [账号2, 账号1]，所以「账号1」= 序号 2 = 19****02
    assert_eq!(
        exchange
            .bindings
            .iter()
            .map(|b| (b.account_id.as_str().to_string(), b.prize_id))
            .collect::<Vec<_>>(),
        vec![
            (ACC_2.to_string(), 42),
            (ACC_1.to_string(), 42),
            (ACC_2.to_string(), 7),
        ],
        "第 2 条与第 1 条同账号同奖品 → 折叠；第 4 条用手机号引用 → 按掩码归一后也是 19****02/7"
    );
    assert_eq!(
        exchange.bindings[0].prize_name.as_deref(),
        Some("话费"),
        "折叠时保留首次出现的奖品名"
    );
    // 「19900000002」是手机号原文 → 旧版当成序号越界（手机号认不出是账号），
    // 这里应当恢复并留下记录，而不是静默吃掉
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::RecoveredAccountRef { .. })),
        "手机号引用被恢复必须报出来：{:#?}",
        report.issue_lines()
    );
}

#[tokio::test]
async fn exchange_with_no_window_falls_back_to_the_default() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "exchange.json",
        r#"{ "enabled": true, "windows": ["", " "], "bindings": [] }"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    let exchange = s.exchange_config().await.unwrap();
    assert_eq!(exchange.windows, vec!["09:59:40", "11:59:40"]);
    assert!(!exchange.enabled, "没有绑定就谈不上抢兑");
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::BadExchangeWindows { .. })),
        "{:#?}",
        report.issue_lines()
    );
}

#[tokio::test]
async fn broken_bindings_are_dropped_one_by_one() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "exchange.json",
        r#"{
  "enabled": true,
  "windows": ["09:59:40"],
  "bindings": [
{ "account": "账号1", "prizeId": "不是数字" },
{ "prizeId": 42 },
{ "account": "查无此人", "prizeId": 42 },
{ "account": "账号1", "prizeId": 42 }
  ]
}"#,
    );

    let s = store();
    let report = run_import(&s, &source_of(&dir)).await.unwrap();

    let exchange = s.exchange_config().await.unwrap();
    assert_eq!(exchange.bindings.len(), 1, "只有最后一条是完整的");
    assert_eq!(exchange.bindings[0].account_id, id(ACC_2));
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::BadPrizeId { .. }))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::BindingWithoutAccount { index: 1 }))
    );
    assert!(
        report
            .issues
            .iter()
            .any(|i| matches!(i, MigrationIssue::DeadAccountRef { .. }))
    );
}

#[tokio::test]
async fn unknown_page_falls_back_to_the_default_page() {
    let dir = copy_fixtures();
    write_file(
        &dir,
        "webui.json",
        r#"{ "rememberLastPage": true, "lastPage": "被删掉的页面", "logExpanded": true }"#,
    );

    let s = store();
    run_import(&s, &source_of(&dir)).await.unwrap();

    let webui = s.webui_config().await.unwrap();
    assert!(WEBUI_PAGES.contains(&webui.last_page.as_str()));
    assert!(webui.log_expanded);
    assert!(
        !webui.background_run,
        "旧版 `backgroundRun === true` 才为真，缺省是 false"
    );
}

// ── 台账与报告 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn ledger_archives_the_original_text_and_issue_counts() {
    let dir = copy_fixtures();
    let s = store();
    run_import(&s, &source_of(&dir)).await.unwrap();

    let imports = s.list_imports().unwrap();
    assert_eq!(
        imports.len(),
        LEGACY_DATA_FILES.len() + 1,
        "6 个数据文件 + asign.json"
    );

    let history = imports
        .iter()
        .find(|r| r.source.ends_with("history.json"))
        .expect("history.json 的台账");
    assert_eq!(history.record_count, 12);
    assert_eq!(history.issues, 0);
    assert_eq!(history.imported_at, now().to_rfc3339());

    // 原文归档：旧 history.json 的 logs / result 不在新表里，
    // 但整份原文在台账里，信息没丢，将来要回填随时能回填
    let raw = history.raw.as_deref().expect("原文应被归档");
    assert!(raw.contains("\"logs\""));
    assert_eq!(
        raw,
        std::fs::read_to_string(dir.path().join("history.json")).unwrap()
    );

    // 台账的摘要形式不能泄露原文（它可能含凭据）
    let summary = history.summary();
    assert!(!summary.contains("logs"));
    assert!(summary.contains("history.json"));
}

#[tokio::test]
async fn report_summary_is_a_single_safe_line() {
    let s = store();
    let report = run_import(&s, &fixture_source()).await.unwrap();
    let line = report.summary();
    assert!(line.contains("账号 2"));
    assert!(line.contains("历史 12 条"));
    assert!(!line.contains('\n'), "摘要必须是一行");
    assert_eq!(report.issue_lines().len(), report.issues.len());
}
