// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use serde_json::json;

// ───────────────────────── 规整规则 ─────────────────────────

/// `windowToCron`：**前导零会被 `Number()` 去掉**（`09` → `9`）。
#[test]
fn exchange_window_conversion_follows_old_rules() {
    assert_eq!(
        exchange_window_to_cron("09:59:40").unwrap(),
        "40 59 9 * * *"
    );
    assert_eq!(exchange_window_to_cron("00:00:00").unwrap(), "0 0 0 * * *");
    assert_eq!(
        exchange_window_to_cron("23:59:59").unwrap(),
        "59 59 23 * * *"
    );
    // 旧版在正则前先 trim
    assert_eq!(
        exchange_window_to_cron(" 09:59:40 ").unwrap(),
        "40 59 9 * * *"
    );

    for bad in [
        "9:59:40",
        "09:59",
        "09:59:40:00",
        "24:00:00",
        "09:60:00",
        "09:59:60",
        "",
        "abc",
    ] {
        assert!(
            exchange_window_to_cron(bad).is_err(),
            "{bad:?} 应被判为非法时刻"
        );
    }
}

#[test]
fn normalize_windows_pads_dedupes_and_sorts() {
    let windows = |list: &[&str]| {
        normalize_exchange_windows(&list.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
    };

    assert_eq!(
        windows(&["11:59:40", "09:59:40", "11:59:40"]).unwrap(),
        vec!["09:59:40", "11:59:40"],
        "去重并排序"
    );
    assert_eq!(windows(&["09:59"]).unwrap(), vec!["09:59:00"], "5 位补 :00");
    assert_eq!(
        windows(&[" 09:59:40 ", ""]).unwrap(),
        vec!["09:59:40"],
        "空串被剔除"
    );
    assert_eq!(windows(&[]), Err(LegacyError::NoExchangeWindow));
    assert_eq!(windows(&[""]), Err(LegacyError::NoExchangeWindow));
}

/// 组合的账号选择：只有真数组才算数，空结果表示"全部账号"。
#[test]
fn pipeline_accounts_follow_legacy_normalization() {
    let norm = normalize_pipeline_accounts;

    // 废弃的单值字段 → 数组
    assert_eq!(
        norm(None, Some(&json!("账号1"))),
        Some(vec![json!("账号1")])
    );
    // 去空、去重，保持首次出现顺序
    assert_eq!(
        norm(
            Some(&[json!("a"), json!(""), json!("a"), Value::Null]),
            None
        ),
        Some(vec![json!("a")])
    );
    // **空数组会盖掉废弃字段**（旧版先判 Array.isArray）
    assert_eq!(norm(Some(&[]), Some(&json!("账号1"))), None);
    // 都没有 → 全部账号
    assert_eq!(norm(None, None), None);
    // 非数组的 accounts 被当作"没给"
    assert_eq!(
        norm(Some(&[json!(1), json!(2)]), None),
        Some(vec![json!(1), json!(2)])
    );
}

/// 界面偏好：两个布尔的默认方向相反 —— 这是旧版的真实行为。
#[test]
fn webui_boolean_defaults_point_in_opposite_directions() {
    let prefs = normalize_webui(&LegacyWebuiFile::default());
    assert!(prefs.remember_last_page, "缺省即 true（!== false）");
    assert!(!prefs.log_expanded, "缺省即 false（=== true）");
    assert!(!prefs.background_run);
    assert_eq!(prefs.last_page, "tasks");

    let explicit = normalize_webui(&LegacyWebuiFile {
        remember_last_page: Some(false),
        log_expanded: Some(true),
        last_page: Some("history".into()),
        ..Default::default()
    });
    assert!(!explicit.remember_last_page);
    assert!(explicit.log_expanded);
    assert_eq!(explicit.last_page, "history");

    // 白名单外的页面回落到默认
    let bogus = normalize_webui(&LegacyWebuiFile {
        last_page: Some("../../etc/passwd".into()),
        ..Default::default()
    });
    assert_eq!(bogus.last_page, "tasks", "白名单外的值必须回落");
}

#[test]
fn cap_history_keeps_the_tail() {
    let mut runs: Vec<usize> = (0..250).collect();
    cap_history(&mut runs, LEGACY_HISTORY_CAP);
    assert_eq!(runs.len(), 200);
    assert_eq!(runs.first(), Some(&50));
    assert_eq!(runs.last(), Some(&249));

    let mut small: Vec<usize> = vec![1, 2, 3];
    cap_history(&mut small, LEGACY_HISTORY_CAP);
    assert_eq!(small, vec![1, 2, 3], "不足上限时不动");
}

/// 一条坏记录不该拖垮整份文件。
#[test]
fn parse_each_isolates_broken_items() {
    let items = vec![
        json!({"task": "sign", "account": "账号1", "start": "t", "end": "t"}),
        json!(42),
        json!({"task": "prize", "account": "账号2"}),
    ];
    let (ok, bad) = parse_each::<LegacyRun>(&items);
    assert_eq!(ok.len(), 2);
    assert_eq!(ok[0].task, "sign");
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].0, 1, "坏条目的下标要能定位");
}

#[test]
fn legacy_state_reports_unknown_fields_instead_of_dropping_them() {
    let state: LegacyAccountState =
        serde_json::from_value(json!({"jwtToken": "j", "totalCloud": 123})).unwrap();
    assert_eq!(state.jwt_token.as_deref(), Some("j"));
    assert_eq!(
        state.unknown_fields(),
        vec!["totalCloud"],
        "旧版本写过的字段应被报告，而不是静默丢弃"
    );
    assert!(!state.is_empty());
    assert!(LegacyAccountState::default().is_empty());
}
