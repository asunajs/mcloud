// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use super::*;
use crate::store::SqliteStore;
use mcloud_core::legacy::{
    LegacyAccountView, LegacyBinding, ReferenceAdvice, normalize_exchange_windows, normalize_webui,
    parse_rfc3339,
};
use mcloud_core::{ConfigStore, ExchangeBinding, ExchangeConfig, WebuiConfig};
use serde_json::Value;

// ═══════════════════════════════════════════════════════════════════════════
// 定时抢兑
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) async fn write_exchange(
    store: &SqliteStore,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    // 单对象文件：整体解析。手改坏了就应该报出来，而不是按"每条独立"硬凑一半
    let file: mcloud_core::legacy::LegacyExchangeFile = match serde_json::from_value(value.clone())
    {
        Ok(file) => file,
        Err(err) => {
            report.issues.push(MigrationIssue::UnparsableRecord {
                file: "exchange.json".into(),
                index: 0,
                error: err.to_string(),
            });
            return Ok(0);
        }
    };

    let windows = match normalize_exchange_windows(&file.windows) {
        Ok(windows) => windows,
        Err(err) => {
            report.issues.push(MigrationIssue::BadExchangeWindows {
                error: err.to_string(),
            });
            ExchangeConfig::default().windows
        }
    };

    let mut bindings: Vec<ExchangeBinding> = Vec::new();
    for (index, binding) in file.bindings.iter().enumerate() {
        let Some(prize_id) = read_prize_id(binding) else {
            report.issues.push(MigrationIssue::BadPrizeId {
                index,
                raw: binding
                    .prize_id
                    .as_ref()
                    .map(Value::to_string)
                    .unwrap_or_default(),
            });
            continue;
        };

        let Some(reference) = binding.account.as_ref() else {
            report
                .issues
                .push(MigrationIssue::BindingWithoutAccount { index });
            continue;
        };

        let account_id = match mcloud_core::legacy::advise_reference(views, Some(reference)) {
            ReferenceAdvice::AllAccounts => {
                report
                    .issues
                    .push(MigrationIssue::BindingWithoutAccount { index });
                continue;
            }
            ReferenceAdvice::Same { index: at } => views[at - 1].account_id.clone(),
            ReferenceAdvice::RecoveredFromPhone { digits, index: at } => {
                report.issues.push(MigrationIssue::RecoveredAccountRef {
                    at: format!("exchange.json 第 {index} 条绑定"),
                    digits,
                    account: views[at - 1].account_id.as_str().to_string(),
                });
                views[at - 1].account_id.clone()
            }
            ReferenceAdvice::WasNeverEffective { raw } => {
                report.issues.push(MigrationIssue::DeadAccountRef {
                    at: format!("exchange.json 第 {index} 条绑定"),
                    raw,
                    action: "跳过该绑定",
                });
                continue;
            }
        };

        // 旧版按 `account:prizeId` 去重，保留首次出现
        if bindings
            .iter()
            .any(|b| b.account_id == account_id && b.prize_id == prize_id)
        {
            continue;
        }
        bindings.push(ExchangeBinding {
            account_id,
            prize_id,
            prize_name: binding.prize_name.clone(),
        });
    }

    store
        .put_exchange_config(&ExchangeConfig {
            enabled: file.enabled && !bindings.is_empty(),
            windows,
            bindings,
            schedule_ids: file.schedule_ids.clone(),
            updated_at: file.updated_at.as_deref().and_then(parse_rfc3339),
        })
        .await?;

    Ok(1)
}

/// 奖项 ID：旧版走 `Number(...)`，所以数字字符串也算数。
pub(crate) fn read_prize_id(binding: &LegacyBinding) -> Option<i64> {
    let value = binding.prize_id.as_ref()?;
    let parsed = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))?;
    (parsed > 0).then_some(parsed)
}

// ═══════════════════════════════════════════════════════════════════════════
// 界面偏好
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) async fn write_webui(
    store: &SqliteStore,
    value: &Value,
    report: &mut MigrationReport,
) -> Result<i64> {
    let file: mcloud_core::legacy::LegacyWebuiFile = match serde_json::from_value(value.clone()) {
        Ok(file) => file,
        Err(err) => {
            report.issues.push(MigrationIssue::UnparsableRecord {
                file: "webui.json".into(),
                index: 0,
                error: err.to_string(),
            });
            // 读不出来就用默认值：界面偏好不值得让一次迁移失败
            return write_webui_default(store).await;
        }
    };
    store.put_webui_config(&normalize_webui(&file)).await?;
    Ok(1)
}

pub(crate) async fn write_webui_default(store: &SqliteStore) -> Result<i64> {
    store.put_webui_config(&WebuiConfig::default()).await?;
    Ok(0)
}
