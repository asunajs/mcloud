// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use super::*;
use crate::store::SqliteStore;
use mcloud_core::legacy::{
    LegacyAccountView, LegacyPipeline, LegacySchedule, LegacyStep, ReferenceAdvice,
    normalize_pipeline_accounts, parse_rfc3339,
};
use mcloud_core::{
    AccountId, AccountMode, ConfigStore, Pipeline, PipelineStep, Schedule, ScheduleTarget, Scope,
};
use serde_json::Value;

// ═══════════════════════════════════════════════════════════════════════════
// 任务组合
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) async fn write_pipelines(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    let items = value
        .get("pipelines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut written = 0i64;
    for (_, pipeline) in parse_each_indexed::<LegacyPipeline>("pipelines.json", &items, report) {
        let at = format!("pipelines.json「{}」", pipeline.name);

        if pipeline.steps.is_empty() {
            report
                .issues
                .push(MigrationIssue::EmptySteps { at: at.clone() });
            continue;
        }

        // 废弃的步骤级账号：旧版读取时就丢掉，这里也丢，但要说一声
        for (index, step) in pipeline.steps.iter().enumerate() {
            if step.legacy_account.is_some() {
                report.issues.push(MigrationIssue::DroppedStepAccount {
                    at: format!("{at} 第 {} 步", index + 1),
                });
            }
        }

        let account_ids = match resolve_pipeline_accounts(views, &pipeline, &at, report) {
            Ok(ids) => ids,
            Err(()) => {
                // 组合没有 `enabled` 字段，没法"导入但停用"，只能整条跳过
                report
                    .issues
                    .push(MigrationIssue::PipelineSkipped { at: at.clone() });
                continue;
            }
        };

        store
            .upsert_pipeline(
                scope,
                &Pipeline {
                    id: non_empty(&pipeline.id)
                        .unwrap_or_else(|| fallback_id("pipe", &pipeline.name)),
                    owner: scope.owner().cloned(),
                    name: if pipeline.name.is_empty() {
                        "未命名组合".to_string()
                    } else {
                        pipeline.name.clone()
                    },
                    description: pipeline.description.clone(),
                    steps: pipeline
                        .steps
                        .iter()
                        .map(|step: &LegacyStep| PipelineStep {
                            task: step.task.clone(),
                            body: step.body.clone(),
                            continue_on_error: step.continue_on_error,
                        })
                        .collect(),
                    account_ids,
                    account_mode: match pipeline.account_mode.as_deref() {
                        Some("parallel") => AccountMode::Parallel,
                        _ => AccountMode::Sequential,
                    },
                    // 旧版注释写明"默认 true"
                    stop_on_error: pipeline.stop_on_error.unwrap_or(true),
                    created_at: pipeline.created_at.as_deref().and_then(parse_rfc3339),
                    updated_at: pipeline.updated_at.as_deref().and_then(parse_rfc3339),
                    last_run_at: pipeline.last_run_at.as_deref().and_then(parse_rfc3339),
                    last_status: pipeline
                        .last_status
                        .as_deref()
                        .and_then(parse_pipeline_status),
                    last_error: pipeline.last_error.clone(),
                    last_duration_ms: pipeline.last_duration_ms,
                },
            )
            .await?;
        written += 1;
    }

    report.pipelines = written as usize;
    Ok(written)
}

/// 解析组合的账号选择。**任一条无法解析就返回 `Err`** —— 组合没有"停用"这个状态，
/// 少导入一个账号等于悄悄扩大了它的作用范围。
pub(crate) fn resolve_pipeline_accounts(
    views: &[LegacyAccountView],
    pipeline: &LegacyPipeline,
    at: &str,
    report: &mut MigrationReport,
) -> std::result::Result<Vec<AccountId>, ()> {
    let Some(values) = normalize_pipeline_accounts(
        pipeline.accounts.as_deref(),
        pipeline.legacy_account.as_ref(),
    ) else {
        // `None` = 全部账号
        return Ok(Vec::new());
    };

    let mut ids = Vec::with_capacity(values.len());
    for reference in &values {
        match mcloud_core::legacy::advise_reference(views, Some(reference)) {
            ReferenceAdvice::AllAccounts => {
                // `accounts` 里出现空值已被 normalize 剔除，走到这里说明是别的东西
                report.issues.push(MigrationIssue::DeadAccountRef {
                    at: at.to_string(),
                    raw: reference.to_string(),
                    action: "跳过整条组合",
                });
                return Err(());
            }
            ReferenceAdvice::Same { index } => ids.push(views[index - 1].account_id.clone()),
            ReferenceAdvice::RecoveredFromPhone { digits, index } => {
                report.issues.push(MigrationIssue::RecoveredAccountRef {
                    at: at.to_string(),
                    digits,
                    account: views[index - 1].account_id.as_str().to_string(),
                });
                ids.push(views[index - 1].account_id.clone());
            }
            ReferenceAdvice::WasNeverEffective { raw } => {
                report.issues.push(MigrationIssue::DeadAccountRef {
                    at: at.to_string(),
                    raw,
                    action: "跳过整条组合",
                });
                return Err(());
            }
        }
    }
    Ok(ids)
}

// ═══════════════════════════════════════════════════════════════════════════
// 定时任务
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) async fn write_schedules(
    store: &SqliteStore,
    scope: Scope<'_>,
    value: &Value,
    views: &[LegacyAccountView],
    report: &mut MigrationReport,
) -> Result<i64> {
    let items = value
        .get("schedules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut written = 0i64;
    for (_, schedule) in parse_each_indexed::<LegacySchedule>("schedules.json", &items, report) {
        let at = format!("schedules.json「{}」", schedule.name);

        // 旧版 `if (!schedule.enabled) return;` —— 缺字段等于 falsy，等于不启用
        let mut enabled = schedule.enabled.unwrap_or(false);

        if schedule.cron.trim().is_empty() {
            report
                .issues
                .push(MigrationIssue::EmptyCron { at: at.clone() });
            enabled = false;
        }

        // 目标是二选一；旧版优先 pipelineId
        let target = match (
            non_empty(schedule.pipeline_id.as_deref().unwrap_or("")),
            non_empty(schedule.task.as_deref().unwrap_or("")),
        ) {
            (Some(id), _) => Some(ScheduleTarget::Pipeline { id }),
            (None, Some(code)) => Some(ScheduleTarget::Capability { code }),
            (None, None) => {
                report
                    .issues
                    .push(MigrationIssue::MissingTarget { at: at.clone() });
                enabled = false;
                None
            }
        };

        let account_ids = match schedule.account.as_ref() {
            None => Vec::new(), // 未指定 = 全部账号
            Some(reference) => {
                match mcloud_core::legacy::advise_reference(views, Some(reference)) {
                    ReferenceAdvice::AllAccounts => Vec::new(),
                    ReferenceAdvice::Same { index } => vec![views[index - 1].account_id.clone()],
                    ReferenceAdvice::RecoveredFromPhone { digits, index } => {
                        report.issues.push(MigrationIssue::RecoveredAccountRef {
                            at: at.clone(),
                            digits,
                            account: views[index - 1].account_id.as_str().to_string(),
                        });
                        vec![views[index - 1].account_id.clone()]
                    }
                    ReferenceAdvice::WasNeverEffective { raw } => {
                        // 关键：**不能**回落到"全部账号"
                        report.issues.push(MigrationIssue::DeadAccountRef {
                            at: at.clone(),
                            raw,
                            action: "停用该定时任务",
                        });
                        enabled = false;
                        Vec::new()
                    }
                }
            }
        };

        store
            .upsert_schedule(
                scope,
                &Schedule {
                    id: non_empty(&schedule.id)
                        .unwrap_or_else(|| fallback_id("sched", &schedule.name)),
                    owner: scope.owner().cloned(),
                    name: if schedule.name.is_empty() {
                        "未命名任务".to_string()
                    } else {
                        schedule.name.clone()
                    },
                    target,
                    cron: schedule.cron.clone(),
                    timezone: non_empty(schedule.timezone.as_deref().unwrap_or(""))
                        .unwrap_or_else(|| DEFAULT_TZ.to_string()),
                    account_ids,
                    body: schedule.body.clone(),
                    enabled,
                    created_at: schedule.created_at.as_deref().and_then(parse_rfc3339),
                    last_run_at: schedule.last_run_at.as_deref().and_then(parse_rfc3339),
                    last_status: schedule
                        .last_status
                        .as_deref()
                        .and_then(parse_schedule_status),
                    last_error: schedule.last_error.clone(),
                },
            )
            .await?;
        written += 1;
    }

    report.schedules = written as usize;
    Ok(written)
}
