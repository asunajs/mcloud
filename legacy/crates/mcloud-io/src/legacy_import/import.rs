// 由 legacy_import.rs 拆出；对外路径仍是 mcloud_io::legacy_import::*。
use super::*;
use crate::store::{ImportRecord, SqliteStore};
use chrono::{DateTime, Utc};
use mcloud_core::legacy::{LegacyConfig, describe_legacy_accounts};
use mcloud_core::{Scope, sha256_hex};
use serde_json::Value;

pub type Result<T> = std::result::Result<T, LegacyImportError>;

// ═══════════════════════════════════════════════════════════════════════════
// 入口
// ═══════════════════════════════════════════════════════════════════════════

/// 把旧项目数据导入 `store`。可重复执行（幂等说明见模块头）。
///
/// `now` 显式传入而不是内部取 `Utc::now()`：台账的 `imported_at` 因此可测，
/// 也让"所有时间都从外部进来"这条纪律在 io 层同样成立。
///
/// # `scope` 决定导入的账号归谁
///
/// 账号从 v3 起有归属（`accounts.owner`），而**无主账号对谁都不可见**（见
/// `StateStore` 的隔离契约）。于是：
///
/// - [`Scope::User`]：导入的账号归那个用户 —— `user init` 之后由管理员执行的迁移；
/// - [`Scope::Local`]：导入的账号**保持无主**（`owner IS NULL`），
///   等之后初始化管理员时被 `claim_unowned_accounts` 认领。
///
/// 后者是 CLI 的路径，也是"先 migrate 再 init"这个顺序能成立的**原因**：
/// 本地运维本来就能操作所有数据，没必要为了搬旧数据先造一个用户。
pub async fn import_legacy(
    store: &SqliteStore,
    scope: Scope<'_>,
    source: &LegacySource,
    now: DateTime<Utc>,
) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();

    // ── 1. 账号清单：每次都重新读（后面的引用解析都要用），但只在源变了才写 ──
    let config_text = read_text(&source.config_path)?;
    let config: LegacyConfig = parse_whole(&source.config_path, &config_text)?;
    let views = describe_legacy_accounts(&config.caiyun);

    // ── 2. 掩码冲突检查：**必须在任何写入之前** ──
    // 两个手机号前 2 后 2 相同时会塌缩成一个 AccountId，历史数据会混在一起且不报错。
    // 这里直接失败，代价是"迁移没做"，而不是"迁移做错了"。
    let identity = mcloud_core::legacy::build_identity_map(&views)?;

    if source_changed(store, &source.config_path, &config_text)? {
        let written = write_accounts(store, scope, &views).await?;
        report.accounts = written as usize;
        commit_ledger(
            store,
            &source.config_path,
            &config_text,
            written,
            &report,
            now,
        )?;
    }

    // ── 3. 逐个数据文件 ──
    for name in LEGACY_DATA_FILES {
        let path = source.data_dir.join(name);
        if !path.is_file() {
            report.issues.push(MigrationIssue::MissingFile {
                path: path.display().to_string(),
            });
            continue;
        }
        let text = read_text(&path)?;
        if !source_changed(store, &path, &text)? {
            report.skipped_unchanged.push(name.to_string());
            continue;
        }
        let value: Value = parse_whole(&path, &text)?;

        let issues_before = report.issues.len();
        let written = match name {
            "state.json" => write_state(store, scope, &value, &identity, &mut report).await?,
            "history.json" => write_history(store, scope, &value, &views, &mut report).await?,
            "pipelines.json" => write_pipelines(store, scope, &value, &views, &mut report).await?,
            "schedules.json" => write_schedules(store, scope, &value, &views, &mut report).await?,
            "exchange.json" => write_exchange(store, &value, &views, &mut report).await?,
            "webui.json" => write_webui(store, &value, &mut report).await?,
            other => unreachable!("LEGACY_DATA_FILES 里没有 {other}"),
        };
        let new_issues = (report.issues.len() - issues_before) as i64;

        // 台账最后落：中途失败时台账没写，下次重跑会补完
        store.record_import(&ImportRecord {
            source: path.display().to_string(),
            sha256: sha256_hex(text.as_bytes()),
            imported_at: now.to_rfc3339(),
            record_count: written,
            issues: new_issues,
            raw: Some(text),
        })?;
    }

    // ── 4. 凭据回填：**必须排在所有文件之后**，且不受哈希门控 ──
    //
    // 两个"必须"都有理由：
    //   - 排在 `state.json` 之后：`write_state` 走的是 `upsert_account_state`，
    //     那是**整体覆盖**（没给的列会被写成 NULL）。先回填再覆盖等于白填。
    //   - 不受门控：老库的 `state.json` 早就导过了，走门控就永远不会补上。
    report.credentials_filled =
        fill_credentials(store, scope, &config.caiyun, &views).await? as usize;

    Ok(report)
}

/// 只读预演：把导入跑在一个**内存库**上，跑完连库一起丢掉。
///
/// 为什么不另写一套"只检查、不写入"的代码：那样诊断逻辑和真实导入会各自演化，
/// 迟早出现"预演说没问题、真跑却报错"。这里让两者**走完全同一条路径**，
/// 唯一的区别是最后那个库被丢掉 —— 于是漂移在结构上不可能发生。
pub async fn diagnose_legacy(source: &LegacySource, now: DateTime<Utc>) -> Result<MigrationReport> {
    let scratch = SqliteStore::open_in_memory().map_err(LegacyImportError::Store)?;

    // 预演走 `Scope::Local`：它与真实导入是**同一条路径**，只是最后把库丢掉。
    // 这里不再需要造占位用户了 —— 本地范围不要求账号有主人
    //（导入的账号保持无主，真实迁移之后由认领流程接手）。
    import_legacy(&scratch, Scope::Local, source, now).await
}
