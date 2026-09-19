// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use super::*;
use crate::store::SqliteStore;
use mcloud_core::legacy::{LegacyAccountView, LegacyRun, find_by_label, parse_rfc3339};
use mcloud_core::{RunRecord, RunStatus, Scope, StateStore, md5_hex};
use serde_json::Value;

// ═══════════════════════════════════════════════════════════════════════════
// 运行历史
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) async fn write_history(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    let items = value
        .get("runs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut written = 0i64;
    for (index, run) in parse_each_indexed::<LegacyRun>("history.json", &items, report) {
        let Some(started_at) = parse_rfc3339(&run.start) else {
            report.issues.push(MigrationIssue::BadRunTimestamp {
                file: "history.json".into(),
                index: index + 1,
                start: run.start.clone(),
            });
            continue;
        };

        // `runs[].account` 存的是**展示名**，不是标识 —— 这是旧版留下的第二个标识体系
        let account_id = match find_by_label(views, &run.account).as_slice() {
            [only] => Some(only.account_id.clone()),
            [] => {
                report.issues.push(MigrationIssue::UnresolvedRunAccount {
                    index: index + 1,
                    label: run.account.clone(),
                    reason: "匹配不到任何账号",
                });
                None
            }
            _ => {
                report.issues.push(MigrationIssue::UnresolvedRunAccount {
                    index: index + 1,
                    label: run.account.clone(),
                    reason: "对应多个账号（展示名重复）",
                });
                None
            }
        };

        store
            .insert_run(
                scope,
                &RunRecord {
                    id: legacy_run_id(&run),
                    account_id,
                    capability: run.task.clone(),
                    status: if run.ok {
                        RunStatus::Done
                    } else {
                        RunStatus::Failed
                    },
                    started_at,
                    finished_at: parse_rfc3339(&run.end),
                    // 旧数据从未采到"净增云朵"（原项目该字段的写入被注释掉了），
                    // 编不出来就不编：留空比瞎填一个 0 诚实
                    cloud_gained: None,
                    cloud_after: None,
                    message: run.error.clone(),
                },
            )
            .await?;
        written += 1;
    }

    report.runs = written as usize;
    Ok(written)
}

/// 旧运行记录的稳定 id：内容相同即同一条。
///
/// 用内容哈希而不是自增序号，是为了让"重跑导入"天然不产生重复行。
/// 键取 `task|account|start|end` —— 这四段就是旧记录的**身份**：
/// 重排文件、加空行都不会让同一条历史换个 id。
///
/// `logs` / `result` **刻意不参与哈希**：它们体积大（夹具里 history.json 有 97KB，
/// 几乎全是日志），且同一账号同一时刻不可能有两条不同的运行。
/// 把它们算进去只会让"日志被截断上限动过一次"变成一次全量 id 变更。
pub(crate) fn legacy_run_id(run: &LegacyRun) -> String {
    let canonical = format!("{}|{}|{}|{}", run.task, run.account, run.start, run.end);
    format!("legacy-{}", &md5_hex(canonical.as_bytes())[..16])
}
