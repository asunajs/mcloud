// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use super::*;
use crate::store::{ImportRecord, SqliteStore};
use chrono::{DateTime, Utc};
use mcloud_core::{md5_hex, sha256_hex};
use serde_json::Value;
use std::path::Path;

// ═══════════════════════════════════════════════════════════════════════════
// 小工具
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| LegacyImportError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

pub(crate) fn parse_whole<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    serde_json::from_str(text).map_err(|e| LegacyImportError::Parse {
        path: path.display().to_string(),
        source: e,
    })
}

/// 逐条解析数组元素，坏的那条**只丢自己**。
///
/// 与 `mcloud_core::legacy::parse_each` 的区别：保留下标。
/// 报告里写「第 3 条」却指不出是哪一条，等于没说 —— 所以这里宁可自己留下标。
/// 下标对外是 **1 起**（给人看的），对内仍是 0 起。
pub(crate) fn parse_each_indexed<T: serde::de::DeserializeOwned>(
    file: &str,
    items: &[Value],
    report: &mut MigrationReport,
) -> Vec<(usize, T)> {
    let mut parsed = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match serde_json::from_value::<T>(item.clone()) {
            Ok(value) => parsed.push((index, value)),
            Err(err) => report.issues.push(MigrationIssue::UnparsableRecord {
                file: file.to_string(),
                index: index + 1,
                error: err.to_string(),
            }),
        }
    }
    parsed
}

/// 源内容与台账里的 sha256 不一致（或从没导入过）时为 `true`。
pub(crate) fn source_changed(store: &SqliteStore, path: &Path, text: &str) -> Result<bool> {
    let digest = sha256_hex(text.as_bytes());
    Ok(match store.import_record(&path.display().to_string())? {
        Some(record) => record.sha256 != digest,
        None => true,
    })
}

pub(crate) fn commit_ledger(
    store: &SqliteStore,
    path: &Path,
    text: &str,
    records: i64,
    report: &MigrationReport,
    now: DateTime<Utc>,
) -> Result<()> {
    store.record_import(&ImportRecord {
        source: path.display().to_string(),
        sha256: sha256_hex(text.as_bytes()),
        imported_at: now.to_rfc3339(),
        record_count: records,
        issues: report.issues.len() as i64,
        raw: Some(text.to_string()),
    })?;
    Ok(())
}

pub(crate) fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

/// 旧记录缺 id 时用的兜底 id。
///
/// 用**名称的内容哈希**而不是下标：下标会在文件重排后变，那就等于每次重排都换一批新记录，
/// 幂等立刻失效。
pub(crate) fn fallback_id(kind: &str, name: &str) -> String {
    format!("legacy-{kind}-{}", &md5_hex(name.as_bytes())[..12])
}

pub(crate) fn parse_schedule_status(value: &str) -> Option<mcloud_core::ScheduleStatus> {
    match value {
        "ok" => Some(mcloud_core::ScheduleStatus::Ok),
        "error" => Some(mcloud_core::ScheduleStatus::Error),
        "skipped" => Some(mcloud_core::ScheduleStatus::Skipped),
        _ => None,
    }
}

pub(crate) fn parse_pipeline_status(value: &str) -> Option<mcloud_core::PipelineStatus> {
    match value {
        "ok" => Some(mcloud_core::PipelineStatus::Ok),
        "error" => Some(mcloud_core::PipelineStatus::Error),
        "partial" => Some(mcloud_core::PipelineStatus::Partial),
        _ => None,
    }
}
