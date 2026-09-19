// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use chrono::{TimeZone, Utc};

#[tokio::test]
async fn exchange_config_defaults_when_absent_then_roundtrips() {
    let s = store();
    let defaults = s.exchange_config().await.unwrap();
    assert!(!defaults.enabled);
    assert_eq!(
        defaults.windows,
        vec!["09:59:40", "11:59:40"],
        "旧版默认窗口"
    );
    assert!(defaults.bindings.is_empty());

    let to_save = ExchangeConfig {
        enabled: true,
        windows: vec!["09:59:40".into(), "11:59:40".into()],
        bindings: vec![ExchangeBinding {
            account_id: AccountId::new("19****01"),
            prize_id: 260624006,
            prize_name: Some("100万tokens叠加包".into()),
        }],
        schedule_ids: vec!["sched-x".into()],
        updated_at: Some(Utc.with_ymd_and_hms(2026, 9, 16, 12, 3, 29).unwrap()),
    };
    s.put_exchange_config(&to_save).await.unwrap();
    assert_eq!(s.exchange_config().await.unwrap(), to_save);
}

/// 单行表被覆盖写时不能变成两行。
#[tokio::test]
async fn exchange_and_webui_configs_stay_single_row() {
    let s = store();
    for _ in 0..3 {
        s.put_exchange_config(&ExchangeConfig::default())
            .await
            .unwrap();
        s.put_webui_config(&WebuiConfig::default()).await.unwrap();
    }
    let conn = s.conn.lock().unwrap();
    for table in ["exchange_config", "webui_config"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "{table} 应始终只有一行");
    }
}

#[tokio::test]
async fn webui_config_defaults_and_page_whitelist_on_write() {
    let s = store();
    let defaults = s.webui_config().await.unwrap();
    assert_eq!(defaults, WebuiConfig::default());

    // 写入时就把非法页面收拢，别把脏值留在库里
    s.put_webui_config(&WebuiConfig {
        last_page: "../../etc/passwd".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(s.webui_config().await.unwrap().last_page, "tasks");
}
