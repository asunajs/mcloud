//! [`mcloud_core::ConfigStore`] 的 SQLite 实现：定时任务、任务组合、抢兑配置、界面偏好。
//!
//! 与 `store.rs`（`StateStore`）分开的理由见 trait 定义处的注释：
//! 两者服务不同的调用方，合在一起会逼着"只想给能力层写个内存假实现"的人
//! 去实现一堆用不到的方法。
//!
//! # JSON 字段的取舍
//!
//! `target` / `account_ids` / `body` / `steps` / `windows` / `bindings` 一律存 TEXT（JSON 原文）。
//! 它们都是"整块读写"的结构，拆成列只会让 schema 跟着能力参数的变化不停改版；
//! 而真正需要被 SQL 查询/排序的维度（`enabled`、`cron`、`last_status`）已经单独成列。
//!
//! # 为什么先读成 `Raw*` 再转换
//!
//! `rusqlite` 的 `query_map` 闭包只能返回 `rusqlite::Result`，没法直接往里塞
//! "JSON 解析失败"这种领域错误。硬塞的话要么 `expect`（把损坏的数据变成 panic），
//! 要么静默 `unwrap_or_default`（把损坏的数据变成"" 却看不出来）。
//! 所以分两步：闭包只做取值，解析在闭包外用 `Result` 做，错误如实往上抛。
//!
//! # 列表顺序
//!
//! `ORDER BY created_at ASC, rowid ASC`。第二排序键用 `rowid` 是刻意的：
//! 旧文件里的顺序就是数组顺序，导入后 rowid 即那个顺序，
//! 而 `ON CONFLICT DO UPDATE` 不改 rowid —— 于是**迁移前后顺序一致**，
//! 不会出现"界面上的定时任务莫名重排了"这种没法解释的现象。

use super::{SqliteStore, dt_from_string, dt_to_string, map_sqlite, user_id_from_opt};
use mcloud_core::{
    AccountId, AccountMode, ConfigStore, Error, ExchangeBinding, ExchangeConfig, Pipeline,
    PipelineStatus, PipelineStep, Schedule, ScheduleStatus, ScheduleTarget, Scope, UserId,
    WebuiConfig,
};
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn json_err(err: serde_json::Error) -> Error {
    Error::Internal(format!("JSON 编解码失败: {err}"))
}

/// 序列化为 JSON 文本（用于 TEXT 列）。
fn to_json<T: Serialize>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(json_err)
}

fn from_json<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    serde_json::from_str(text).map_err(json_err)
}

/// `Option<Value>` → TEXT 列：`None` 写 NULL，而不是 `"null"`。
///
/// 这个区分很重要：NULL 表示"没给参数"，而 `"null"` 是个合法的 JSON 值，
/// 读回来会被当成"显式传了一个 null 参数"。
fn opt_json_to_text(value: &Option<Value>) -> Result<Option<String>, Error> {
    value.as_ref().map(to_json).transpose()
}

fn text_to_opt_json(text: Option<String>) -> Result<Option<Value>, Error> {
    text.map(|t| from_json::<Value>(&t)).transpose()
}

fn account_ids_to_text(ids: &[AccountId]) -> Result<String, Error> {
    to_json(&ids.iter().map(|id| id.as_str()).collect::<Vec<_>>())
}

fn account_ids_from_text(text: &str) -> Result<Vec<AccountId>, Error> {
    Ok(from_json::<Vec<String>>(text)?
        .into_iter()
        .map(AccountId::new)
        .collect())
}

const fn schedule_status_str(status: ScheduleStatus) -> &'static str {
    match status {
        ScheduleStatus::Ok => "ok",
        ScheduleStatus::Error => "error",
        ScheduleStatus::Skipped => "skipped",
    }
}

fn schedule_status_from_str(value: &str) -> Option<ScheduleStatus> {
    match value {
        "ok" => Some(ScheduleStatus::Ok),
        "error" => Some(ScheduleStatus::Error),
        "skipped" => Some(ScheduleStatus::Skipped),
        // 认不出来就当作"没有状态"，而不是硬塞一个默认值 ——
        // 一个来路不明的状态会让界面显示错误的结论
        _ => None,
    }
}

const fn pipeline_status_str(status: PipelineStatus) -> &'static str {
    match status {
        PipelineStatus::Ok => "ok",
        PipelineStatus::Error => "error",
        PipelineStatus::Partial => "partial",
    }
}

fn pipeline_status_from_str(value: &str) -> Option<PipelineStatus> {
    match value {
        "ok" => Some(PipelineStatus::Ok),
        "error" => Some(PipelineStatus::Error),
        "partial" => Some(PipelineStatus::Partial),
        _ => None,
    }
}

const fn account_mode_str(mode: AccountMode) -> &'static str {
    match mode {
        AccountMode::Sequential => "sequential",
        AccountMode::Parallel => "parallel",
    }
}

fn account_mode_from_str(value: &str) -> AccountMode {
    match value {
        "parallel" => AccountMode::Parallel,
        // 缺省串行是刻意偏保守：并行更容易触发风控
        _ => AccountMode::Sequential,
    }
}

// ───────────────────────────── 导入台账 ─────────────────────────────

/// 一次旧数据导入的台账记录。
///
/// 放在 io 层而不是 core 的领域模型里：它记录的是**磁盘上的源文件**（路径、哈希、原文），
/// 而 core 的纯净性正建立在"不知道文件和路径存在"之上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRecord {
    /// 源文件绝对路径。
    pub source: String,
    /// 源文件内容的 sha256 —— 幂等判断的依据。
    pub sha256: String,
    pub imported_at: String,
    pub record_count: i64,
    pub issues: i64,
    /// 原始内容归档（便于将来映射规则变了之后重放）。
    ///
    /// `Debug` 会把它整段打出来，所以**日志里不要直接 `?record`** ——
    /// 它可能含凭据。需要一个安全的展示形式时用 [`ImportRecord::summary`]。
    pub raw: Option<String>,
}

impl ImportRecord {
    /// 可安全进日志的摘要（不含 `raw`）。
    pub fn summary(&self) -> String {
        format!(
            "{} (sha256={}… 记录 {} 条 / 问题 {} 个)",
            self.source,
            &self.sha256[..8.min(self.sha256.len())],
            self.record_count,
            self.issues
        )
    }
}

impl SqliteStore {
    /// 写入/覆盖一条导入台账。
    pub fn record_import(&self, record: &ImportRecord) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.execute(
            r#"
            INSERT INTO imports (source, sha256, imported_at, record_count, issues, raw)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(source) DO UPDATE SET
                sha256       = excluded.sha256,
                imported_at  = excluded.imported_at,
                record_count = excluded.record_count,
                issues       = excluded.issues,
                raw          = excluded.raw
            "#,
            params![
                record.source,
                record.sha256,
                record.imported_at,
                record.record_count,
                record.issues,
                record.raw,
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    /// 读取某个源文件的台账；没导入过返回 `None`。
    pub fn import_record(&self, source: &str) -> Result<Option<ImportRecord>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.query_row(
            "SELECT source, sha256, imported_at, record_count, issues, raw \
             FROM imports WHERE source = ?1",
            params![source],
            read_import,
        )
        .optional()
        .map_err(map_sqlite)
    }

    /// 全部台账，按源路径排序。
    pub fn list_imports(&self) -> Result<Vec<ImportRecord>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(
                "SELECT source, sha256, imported_at, record_count, issues, raw \
                 FROM imports ORDER BY source ASC",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], read_import)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }
}

fn read_import(row: &rusqlite::Row<'_>) -> rusqlite::Result<ImportRecord> {
    Ok(ImportRecord {
        source: row.get(0)?,
        sha256: row.get(1)?,
        imported_at: row.get(2)?,
        record_count: row.get(3)?,
        issues: row.get(4)?,
        raw: row.get(5)?,
    })
}

// ───────────────────────────── 原始行 ─────────────────────────────

/// `schedules` 表的原始列。
#[derive(Debug)]
struct RawSchedule {
    id: String,
    name: String,
    target: Option<String>,
    cron: String,
    timezone: String,
    account_ids: String,
    body: Option<String>,
    enabled: i64,
    created_at: Option<String>,
    last_run_at: Option<String>,
    last_status: Option<String>,
    last_error: Option<String>,
    owner: Option<String>,
}

impl RawSchedule {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            target: row.get(2)?,
            cron: row.get(3)?,
            timezone: row.get(4)?,
            account_ids: row.get(5)?,
            body: row.get(6)?,
            enabled: row.get(7)?,
            created_at: row.get(8)?,
            last_run_at: row.get(9)?,
            last_status: row.get(10)?,
            last_error: row.get(11)?,
            owner: row.get(12)?,
        })
    }

    fn into_schedule(self) -> Result<Schedule, Error> {
        Ok(Schedule {
            id: self.id,
            owner: user_id_from_opt(self.owner),
            name: self.name,
            target: self
                .target
                .map(|t| from_json::<ScheduleTarget>(&t))
                .transpose()?,
            cron: self.cron,
            timezone: self.timezone,
            account_ids: account_ids_from_text(&self.account_ids)?,
            body: text_to_opt_json(self.body)?,
            enabled: self.enabled != 0,
            created_at: self.created_at.and_then(dt_from_string),
            last_run_at: self.last_run_at.and_then(dt_from_string),
            last_status: self
                .last_status
                .as_deref()
                .and_then(schedule_status_from_str),
            last_error: self.last_error,
        })
    }
}

/// `pipelines` 表的原始列。
#[derive(Debug)]
struct RawPipeline {
    id: String,
    name: String,
    description: Option<String>,
    steps: String,
    account_ids: String,
    account_mode: String,
    stop_on_error: i64,
    created_at: Option<String>,
    updated_at: Option<String>,
    last_run_at: Option<String>,
    last_status: Option<String>,
    last_error: Option<String>,
    last_duration_ms: Option<i64>,
    owner: Option<String>,
}

impl RawPipeline {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            steps: row.get(3)?,
            account_ids: row.get(4)?,
            account_mode: row.get(5)?,
            stop_on_error: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            last_run_at: row.get(9)?,
            last_status: row.get(10)?,
            last_error: row.get(11)?,
            last_duration_ms: row.get(12)?,
            owner: row.get(13)?,
        })
    }

    fn into_pipeline(self) -> Result<Pipeline, Error> {
        Ok(Pipeline {
            id: self.id,
            owner: user_id_from_opt(self.owner),
            name: self.name,
            description: self.description,
            steps: from_json::<Vec<PipelineStep>>(&self.steps)?,
            account_ids: account_ids_from_text(&self.account_ids)?,
            account_mode: account_mode_from_str(&self.account_mode),
            stop_on_error: self.stop_on_error != 0,
            created_at: self.created_at.and_then(dt_from_string),
            updated_at: self.updated_at.and_then(dt_from_string),
            last_run_at: self.last_run_at.and_then(dt_from_string),
            last_status: self
                .last_status
                .as_deref()
                .and_then(pipeline_status_from_str),
            last_error: self.last_error,
            last_duration_ms: self.last_duration_ms,
        })
    }
}

/// 列顺序与 [`RawSchedule::read`] 的索引**一一对应**；`owner` 固定放最后。
///
/// 与 `store.rs` 的 `ACCOUNT_COLUMNS` 同一个理由：列清单只能有一处，
/// 否则某条查询会静默少一列而不报错。
const SCHEDULE_SELECT: &str = "SELECT id, name, target, cron, timezone, account_ids, body, enabled, \
     created_at, last_run_at, last_status, last_error, owner FROM schedules";

const PIPELINE_SELECT: &str = "SELECT id, name, description, steps, account_ids, account_mode, \
     stop_on_error, created_at, updated_at, last_run_at, last_status, last_error, last_duration_ms, \
     owner FROM pipelines";

/// 按 id 的单条查询/删除用的归属过滤：`?1` 是 id，归属是 `?2`。
///
/// 表达式与 `StateStore` 那边**是同一个**：`?2 IS NULL` 即 `Scope::Local`（不过滤），
/// `?2` 有值时退化成等值比较；未归属（`owner IS NULL`）的行对**所有人**都不匹配。
const OWNER_VISIBLE_BY_ID: &str = "(?2 IS NULL OR owner = ?2)";

/// 列表查询用的归属过滤：整条查询只有归属一个参数，所以它是 `?1`。
///
/// 与 [`OWNER_VISIBLE_BY_ID`] 是同一个判据、只是占位符编号不同 ——
/// 写成两个常量而不是一个，是因为编号错了不会编译报错，只会在运行期报
/// "Wrong number of parameters"，而这类错误最容易在"复制一条查询改改"时产生。
const OWNER_VISIBLE_ONLY: &str = "(?1 IS NULL OR owner = ?1)";

/// 配置行写入前的归属校验（`sql` 只传本文件的常量，不接受外部输入）。
///
/// 与 `store.rs` 账号写入的第二道校验同构：id 是主键，upsert 会顺手改掉别人的那一行，
/// 所以"已被别人创建"必须显式挡住，而不是让 `ON CONFLICT` 静默顶掉。
fn ensure_not_owned_by_other(
    conn: &Connection,
    scope: Scope<'_>,
    sql: &str,
    id: &str,
    label: &str,
) -> Result<(), Error> {
    let existing: Option<Option<String>> = conn
        .query_row(sql, params![id], |row| row.get(0))
        .optional()
        .map_err(map_sqlite)?;
    if let Some(Some(other)) = existing
        && scope.owner().is_some_and(|uid| other != uid.as_str())
    {
        // 给可行动的提示：表现成"不存在"会变成查不出原因的怪象（同 store.rs 的口径）
        return Err(Error::BadRequest(format!("该{label}已被其他用户创建")));
    }
    Ok(())
}

// ───────────────────────────── ConfigStore ─────────────────────────────

impl ConfigStore for SqliteStore {
    async fn upsert_schedule(&self, scope: Scope<'_>, schedule: &Schedule) -> Result<(), Error> {
        // ① 用户范围下不能替别人建任务，也不能把任务写成"无主"
        //（无主只由 schema 迁移与本地运维产生）—— 与 store.rs 的账号口径一致。
        if let Some(user_id) = scope.owner()
            && schedule.owner.as_ref() != Some(user_id)
        {
            return Err(Error::NotFound(format!("定时任务 {}", schedule.id)));
        }
        let target = schedule.target.as_ref().map(to_json).transpose()?;
        let account_ids = account_ids_to_text(&schedule.account_ids)?;
        let body = opt_json_to_text(&schedule.body)?;
        let conn = self.conn.lock().expect("数据库锁中毒");
        ensure_not_owned_by_other(
            &conn,
            scope,
            "SELECT owner FROM schedules WHERE id = ?1",
            &schedule.id,
            "定时任务",
        )?;
        conn.execute(
            r#"
            INSERT INTO schedules (
                id, name, target, cron, timezone, account_ids, body, enabled,
                created_at, last_run_at, last_status, last_error, owner
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(id) DO UPDATE SET
                name        = excluded.name,
                target      = excluded.target,
                cron        = excluded.cron,
                timezone    = excluded.timezone,
                account_ids = excluded.account_ids,
                body        = excluded.body,
                enabled     = excluded.enabled,
                last_run_at = excluded.last_run_at,
                last_status = excluded.last_status,
                last_error  = excluded.last_error,
                -- 归属可以在这条语句里被"认领"（无主 → 本人），但不能被换成别人：
                -- 换成别人的情况已由上面两道校验挡住。
                owner       = excluded.owner
            "#,
            params![
                schedule.id,
                schedule.name,
                target,
                schedule.cron,
                schedule.timezone,
                account_ids,
                body,
                schedule.enabled as i64,
                schedule.created_at.as_ref().map(dt_to_string),
                schedule.last_run_at.as_ref().map(dt_to_string),
                schedule.last_status.map(schedule_status_str),
                schedule.last_error,
                schedule.owner.as_ref().map(UserId::as_str),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn get_schedule(&self, scope: Scope<'_>, id: &str) -> Result<Option<Schedule>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let raw = conn
            .query_row(
                &format!("{SCHEDULE_SELECT} WHERE id = ?1 AND {OWNER_VISIBLE_BY_ID}"),
                params![id, scope.owner().map(UserId::as_str)],
                RawSchedule::read,
            )
            .optional()
            .map_err(map_sqlite)?;
        raw.map(RawSchedule::into_schedule).transpose()
    }

    async fn list_schedules(&self, scope: Scope<'_>) -> Result<Vec<Schedule>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(&format!(
                "{SCHEDULE_SELECT} WHERE {OWNER_VISIBLE_ONLY} ORDER BY created_at ASC, rowid ASC"
            ))
            .map_err(map_sqlite)?;
        let raw = stmt
            .query_map(
                params![scope.owner().map(UserId::as_str)],
                RawSchedule::read,
            )
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        raw.into_iter().map(RawSchedule::into_schedule).collect()
    }

    async fn delete_schedule(&self, scope: Scope<'_>, id: &str) -> Result<bool, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let affected = conn
            .execute(
                &format!("DELETE FROM schedules WHERE id = ?1 AND {OWNER_VISIBLE_BY_ID}"),
                params![id, scope.owner().map(UserId::as_str)],
            )
            .map_err(map_sqlite)?;
        Ok(affected > 0)
    }

    async fn upsert_pipeline(&self, scope: Scope<'_>, pipeline: &Pipeline) -> Result<(), Error> {
        // 两道校验与 upsert_schedule 完全同构，理由见那里的注释
        if let Some(user_id) = scope.owner()
            && pipeline.owner.as_ref() != Some(user_id)
        {
            return Err(Error::NotFound(format!("组合 {}", pipeline.id)));
        }
        let steps = to_json(&pipeline.steps)?;
        let account_ids = account_ids_to_text(&pipeline.account_ids)?;
        let conn = self.conn.lock().expect("数据库锁中毒");
        ensure_not_owned_by_other(
            &conn,
            scope,
            "SELECT owner FROM pipelines WHERE id = ?1",
            &pipeline.id,
            "组合",
        )?;
        conn.execute(
            r#"
            INSERT INTO pipelines (
                id, name, description, steps, account_ids, account_mode, stop_on_error,
                created_at, updated_at, last_run_at, last_status, last_error, last_duration_ms,
                owner
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(id) DO UPDATE SET
                name             = excluded.name,
                description      = excluded.description,
                steps            = excluded.steps,
                account_ids      = excluded.account_ids,
                account_mode     = excluded.account_mode,
                stop_on_error    = excluded.stop_on_error,
                updated_at       = excluded.updated_at,
                last_run_at      = excluded.last_run_at,
                last_status      = excluded.last_status,
                last_error       = excluded.last_error,
                last_duration_ms = excluded.last_duration_ms,
                owner            = excluded.owner
            "#,
            params![
                pipeline.id,
                pipeline.name,
                pipeline.description,
                steps,
                account_ids,
                account_mode_str(pipeline.account_mode),
                pipeline.stop_on_error as i64,
                pipeline.created_at.as_ref().map(dt_to_string),
                pipeline.updated_at.as_ref().map(dt_to_string),
                pipeline.last_run_at.as_ref().map(dt_to_string),
                pipeline.last_status.map(pipeline_status_str),
                pipeline.last_error,
                pipeline.last_duration_ms,
                pipeline.owner.as_ref().map(UserId::as_str),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn get_pipeline(&self, scope: Scope<'_>, id: &str) -> Result<Option<Pipeline>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let raw = conn
            .query_row(
                &format!("{PIPELINE_SELECT} WHERE id = ?1 AND {OWNER_VISIBLE_BY_ID}"),
                params![id, scope.owner().map(UserId::as_str)],
                RawPipeline::read,
            )
            .optional()
            .map_err(map_sqlite)?;
        raw.map(RawPipeline::into_pipeline).transpose()
    }

    async fn list_pipelines(&self, scope: Scope<'_>) -> Result<Vec<Pipeline>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(&format!(
                "{PIPELINE_SELECT} WHERE {OWNER_VISIBLE_ONLY} ORDER BY created_at ASC, rowid ASC"
            ))
            .map_err(map_sqlite)?;
        let raw = stmt
            .query_map(
                params![scope.owner().map(UserId::as_str)],
                RawPipeline::read,
            )
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        raw.into_iter().map(RawPipeline::into_pipeline).collect()
    }

    async fn exchange_config(&self) -> Result<ExchangeConfig, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let row = conn
            .query_row(
                "SELECT enabled, windows, bindings, schedule_ids, updated_at \
                 FROM exchange_config WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite)?;

        let Some((enabled, windows, bindings, schedule_ids, updated_at)) = row else {
            // 旧版语义：文件不存在就返回默认值，且**不写盘**
            return Ok(ExchangeConfig::default());
        };

        Ok(ExchangeConfig {
            enabled: enabled != 0,
            windows: from_json::<Vec<String>>(&windows)?,
            bindings: from_json::<Vec<ExchangeBinding>>(&bindings)?,
            schedule_ids: from_json::<Vec<String>>(&schedule_ids)?,
            updated_at: updated_at.and_then(dt_from_string),
        })
    }

    async fn put_exchange_config(&self, config: &ExchangeConfig) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.execute(
            r#"
            INSERT INTO exchange_config (id, enabled, windows, bindings, schedule_ids, updated_at)
            VALUES (1, ?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                enabled      = excluded.enabled,
                windows      = excluded.windows,
                bindings     = excluded.bindings,
                schedule_ids = excluded.schedule_ids,
                updated_at   = excluded.updated_at
            "#,
            params![
                config.enabled as i64,
                to_json(&config.windows)?,
                to_json(&config.bindings)?,
                to_json(&config.schedule_ids)?,
                config.updated_at.as_ref().map(dt_to_string),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn webui_config(&self) -> Result<WebuiConfig, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let row = conn
            .query_row(
                "SELECT remember_last_page, last_page, log_expanded, background_run, updated_at \
                 FROM webui_config WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(map_sqlite)?;

        let Some((remember, last_page, log_expanded, background_run, updated_at)) = row else {
            return Ok(WebuiConfig::default());
        };

        let mut config = WebuiConfig {
            remember_last_page: remember != 0,
            last_page,
            log_expanded: log_expanded != 0,
            background_run: background_run != 0,
            updated_at: updated_at.and_then(dt_from_string),
        };
        // 白名单约束在读写两侧都收一遍：库里可能是手改过的
        config.normalize_page();
        Ok(config)
    }

    async fn put_webui_config(&self, config: &WebuiConfig) -> Result<(), Error> {
        let mut config = config.clone();
        config.normalize_page();
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.execute(
            r#"
            INSERT INTO webui_config (
                id, remember_last_page, last_page, log_expanded, background_run, updated_at
            ) VALUES (1, ?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                remember_last_page = excluded.remember_last_page,
                last_page          = excluded.last_page,
                log_expanded       = excluded.log_expanded,
                background_run     = excluded.background_run,
                updated_at         = excluded.updated_at
            "#,
            params![
                config.remember_last_page as i64,
                config.last_page,
                config.log_expanded as i64,
                config.background_run as i64,
                config.updated_at.as_ref().map(dt_to_string),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use mcloud_core::{Account, ClaimedRows, Role, Scope, StateStore, User, UserId, UserStore};

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("内存库应可打开")
    }

    /// 测试用户。v3 起账号与登录态都要求归属（`accounts.owner` 有外键指向 `users`），
    /// 所以碰账号的用例都得先有个"主人"。
    const TEST_USER: &str = "test-user";

    fn uid() -> UserId {
        UserId::new(TEST_USER)
    }

    async fn ensure_user(s: &SqliteStore) {
        s.upsert_user(&User::new(
            TEST_USER,
            TEST_USER,
            "!",
            Role::Admin,
            Utc.with_ymd_and_hms(2026, 9, 16, 1, 0, 0).unwrap(),
        ))
        .await
        .unwrap();
    }

    /// 建一个归属于测试用户的账号。
    async fn owned_account(s: &SqliteStore, id: &str) -> Account {
        ensure_user(s).await;
        let mut account = Account::new(id);
        account.owner = Some(uid());
        s.upsert_account(Scope::user(&uid()), &account)
            .await
            .unwrap();
        account
    }

    fn schedule(id: &str) -> Schedule {
        Schedule {
            id: id.into(),
            owner: None,
            name: "每日签到".into(),
            target: Some(ScheduleTarget::Capability {
                code: "sign".into(),
            }),
            cron: "0 30 7 * * *".into(),
            timezone: "Asia/Shanghai".into(),
            account_ids: vec![AccountId::new("19****01")],
            body: Some(serde_json::json!({"codes": ["口令"]})),
            enabled: true,
            created_at: Some(Utc.with_ymd_and_hms(2026, 9, 16, 1, 0, 0).unwrap()),
            last_run_at: None,
            last_status: None,
            last_error: None,
        }
    }

    fn pipeline(id: &str) -> Pipeline {
        Pipeline {
            id: id.into(),
            owner: None,
            name: "轻量巡检".into(),
            description: Some("查奖品 + 领奖品".into()),
            steps: vec![
                PipelineStep {
                    task: "prizes".into(),
                    body: None,
                    continue_on_error: None,
                },
                PipelineStep {
                    task: "receive".into(),
                    body: Some(serde_json::json!({"onlyNew": true})),
                    continue_on_error: Some(true),
                },
            ],
            account_ids: Vec::new(),
            account_mode: AccountMode::Sequential,
            stop_on_error: true,
            created_at: Some(Utc.with_ymd_and_hms(2026, 9, 16, 1, 57, 41).unwrap()),
            updated_at: Some(Utc.with_ymd_and_hms(2026, 9, 16, 2, 24, 7).unwrap()),
            last_run_at: None,
            last_status: Some(PipelineStatus::Ok),
            last_error: None,
            last_duration_ms: Some(2481),
        }
    }

    #[tokio::test]
    async fn schedule_roundtrip_keeps_every_field() {
        let s = store();
        let original = schedule("sched-1");
        s.upsert_schedule(Scope::Local, &original).await.unwrap();

        let loaded = s
            .get_schedule(Scope::Local, "sched-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, original);
    }

    #[tokio::test]
    async fn schedule_target_absent_survives_as_none() {
        let s = store();
        let mut sc = schedule("sched-1");
        sc.target = None;
        s.upsert_schedule(Scope::Local, &sc).await.unwrap();
        assert_eq!(
            s.get_schedule(Scope::Local, "sched-1")
                .await
                .unwrap()
                .unwrap()
                .target,
            None
        );
    }

    /// `None` 必须读写为 NULL，不能被写成字符串 `"null"`。
    #[tokio::test]
    async fn absent_body_is_null_not_the_string_null() {
        let s = store();
        let mut sc = schedule("sched-1");
        sc.body = None;
        s.upsert_schedule(Scope::Local, &sc).await.unwrap();

        let raw: Option<String> = {
            let conn = s.conn.lock().unwrap();
            conn.query_row("SELECT body FROM schedules WHERE id = 'sched-1'", [], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(raw, None, "没给参数应存 NULL");

        assert_eq!(
            s.get_schedule(Scope::Local, "sched-1")
                .await
                .unwrap()
                .unwrap()
                .body,
            None
        );
    }

    #[tokio::test]
    async fn schedule_upsert_updates_in_place() {
        let s = store();
        let mut sc = schedule("sched-1");
        s.upsert_schedule(Scope::Local, &sc).await.unwrap();

        sc.name = "改名了".into();
        sc.enabled = false;
        sc.last_status = Some(ScheduleStatus::Skipped);
        s.upsert_schedule(Scope::Local, &sc).await.unwrap();

        let all = s.list_schedules(Scope::Local).await.unwrap();
        assert_eq!(all.len(), 1, "重复写入不应产生第二条");
        assert_eq!(all[0].name, "改名了");
        assert!(!all[0].enabled);
        assert_eq!(all[0].last_status, Some(ScheduleStatus::Skipped));
    }

    #[tokio::test]
    async fn schedules_keep_insertion_order_when_created_at_ties() {
        let s = store();
        // 三个都没有 created_at —— 排序完全靠 rowid，也就是插入顺序
        for id in ["c", "a", "b"] {
            let mut sc = schedule(id);
            sc.created_at = None;
            s.upsert_schedule(Scope::Local, &sc).await.unwrap();
        }
        let ids: Vec<String> = s
            .list_schedules(Scope::Local)
            .await
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert_eq!(ids, vec!["c", "a", "b"], "同 created_at 时应保持插入顺序");
    }

    #[tokio::test]
    async fn delete_schedule_reports_whether_a_row_went_away() {
        let s = store();
        s.upsert_schedule(Scope::Local, &schedule("sched-1"))
            .await
            .unwrap();
        assert!(s.delete_schedule(Scope::Local, "sched-1").await.unwrap());
        assert!(
            !s.delete_schedule(Scope::Local, "sched-1").await.unwrap(),
            "第二次应为 false"
        );
        assert!(s.list_schedules(Scope::Local).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn pipeline_roundtrip_keeps_steps_and_accounts() {
        let s = store();
        let mut original = pipeline("pipe-1");
        original.account_ids = vec![AccountId::new("19****01"), AccountId::new("19****02")];
        original.account_mode = AccountMode::Parallel;
        original.stop_on_error = false;
        s.upsert_pipeline(Scope::Local, &original).await.unwrap();

        let loaded = s
            .get_pipeline(Scope::Local, "pipe-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, original);
        assert_eq!(loaded.steps.len(), 2);
        assert_eq!(loaded.steps[1].continue_on_error, Some(true));
    }

    /// 组合与定时任务的状态枚举**不是同一个**（组合有 partial、没有 skipped），别合并。
    #[test]
    fn pipeline_status_is_not_schedule_status() {
        assert_eq!(
            pipeline_status_from_str("partial"),
            Some(PipelineStatus::Partial)
        );
        assert_eq!(pipeline_status_from_str("skipped"), None);
        assert_eq!(
            schedule_status_from_str("skipped"),
            Some(ScheduleStatus::Skipped)
        );
        assert_eq!(schedule_status_from_str("partial"), None);
    }

    #[tokio::test]
    async fn unknown_status_string_reads_as_none_instead_of_a_wrong_value() {
        let s = store();
        s.upsert_schedule(Scope::Local, &schedule("sched-1"))
            .await
            .unwrap();
        {
            let conn = s.conn.lock().unwrap();
            conn.execute(
                "UPDATE schedules SET last_status = 'weird' WHERE id = 'sched-1'",
                [],
            )
            .unwrap();
        }
        assert_eq!(
            s.get_schedule(Scope::Local, "sched-1")
                .await
                .unwrap()
                .unwrap()
                .last_status,
            None
        );
    }

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

    #[tokio::test]
    async fn import_ledger_records_and_reads_back() {
        let s = store();
        assert!(s.import_record("/old/state.json").unwrap().is_none());

        let record = ImportRecord {
            source: "/old/state.json".into(),
            sha256: "a".repeat(64),
            imported_at: "2026-09-16T22:00:00+00:00".into(),
            record_count: 2,
            issues: 1,
            raw: Some("{\"accounts\":{}}".into()),
        };
        s.record_import(&record).unwrap();
        assert_eq!(s.import_record("/old/state.json").unwrap().unwrap(), record);

        // 同一个源再写一次是覆盖，不是新增
        s.record_import(&ImportRecord {
            sha256: "b".repeat(64),
            ..record.clone()
        })
        .unwrap();
        let all = s.list_imports().unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].sha256.starts_with('b'));

        assert!(record.summary().contains("记录 2 条"));
        assert!(!record.summary().contains("accounts"), "摘要不得带出原文");
    }

    #[tokio::test]
    async fn account_state_is_stored_apart_from_accounts() {
        let s = store();
        let account = owned_account(&s, "19****01").await;

        let state = mcloud_core::AccountState {
            jwt_token: Some("SECRETJWT".into()),
            device_id: Some("DEV".into()),
            ..mcloud_core::AccountState::new("19****01")
        };
        s.upsert_account_state(Scope::user(&uid()), &state)
            .await
            .unwrap();

        // 读账号列表**不会**带出凭据：两张表没有 JOIN，凭据物理上不在结果里
        let accounts = s.list_accounts(Scope::user(&uid())).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0], account, "账号行不应被登录态污染");

        let loaded = s
            .get_account_state(Scope::user(&uid()), &AccountId::new("19****01"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.jwt_token.as_deref(), Some("SECRETJWT"));
        assert_eq!(loaded, state);
    }

    /// 覆盖写而不是合并：`None` 必须能真的清掉旧值。
    #[tokio::test]
    async fn account_state_upsert_overwrites_instead_of_merging() {
        let s = store();
        let id = AccountId::new("19****01");
        // 登录态依附于账号：账号必须先存在且归属于调用者，否则写入会被隔离校验挡住
        owned_account(&s, "19****01").await;

        s.upsert_account_state(
            Scope::user(&uid()),
            &mcloud_core::AccountState {
                jwt_token: Some("OLD".into()),
                device_id: Some("OLDDEV".into()),
                ..mcloud_core::AccountState::new("19****01")
            },
        )
        .await
        .unwrap();

        s.upsert_account_state(
            Scope::user(&uid()),
            &mcloud_core::AccountState {
                jwt_token: Some("NEW".into()),
                ..mcloud_core::AccountState::new("19****01")
            },
        )
        .await
        .unwrap();

        let loaded = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.jwt_token.as_deref(), Some("NEW"));
        assert_eq!(
            loaded.device_id, None,
            "旧的 deviceId 必须被清掉，否则重新登录后还会拿旧设备号去用"
        );
    }

    #[tokio::test]
    async fn config_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcloud.db");
        {
            let s = SqliteStore::open(&path).unwrap();
            s.upsert_schedule(Scope::Local, &schedule("sched-1"))
                .await
                .unwrap();
            s.upsert_pipeline(Scope::Local, &pipeline("pipe-1"))
                .await
                .unwrap();
        }
        let s = SqliteStore::open(&path).unwrap();
        assert_eq!(s.list_schedules(Scope::Local).await.unwrap().len(), 1);
        assert_eq!(s.list_pipelines(Scope::Local).await.unwrap().len(), 1);
    }

    // ───────────── 归属：定时任务与组合（schema v4 / §3.7） ─────────────

    /// 两个用户，供归属用例使用。
    async fn two_users(s: &SqliteStore) -> (UserId, UserId) {
        ensure_user(s).await;
        let other = UserId::new("other-user");
        s.upsert_user(&User::new(
            other.as_str(),
            other.as_str(),
            "!",
            Role::User,
            Utc.with_ymd_and_hms(2026, 9, 16, 1, 0, 0).unwrap(),
        ))
        .await
        .unwrap();
        (uid(), other)
    }

    /// 交叉读取：A 的定时任务与组合，B 既看不到也删不掉。
    ///
    /// "看不到"与"不存在"必须是同一个结果（连存在性都不泄露）。
    #[tokio::test]
    async fn configs_are_isolated_between_users() {
        let s = store();
        let (a, b) = two_users(&s).await;

        let mut sc = schedule("sched-a");
        sc.owner = Some(a.clone());
        s.upsert_schedule(Scope::user(&a), &sc).await.unwrap();
        let mut pl = pipeline("pipe-a");
        pl.owner = Some(a.clone());
        s.upsert_pipeline(Scope::user(&a), &pl).await.unwrap();

        // 本人看得见
        assert_eq!(s.list_schedules(Scope::user(&a)).await.unwrap().len(), 1);
        assert_eq!(s.list_pipelines(Scope::user(&a)).await.unwrap().len(), 1);
        assert!(
            s.get_schedule(Scope::user(&a), "sched-a")
                .await
                .unwrap()
                .is_some()
        );

        // 别人看不见
        assert!(
            s.list_schedules(Scope::user(&b)).await.unwrap().is_empty(),
            "B 不该看见 A 的定时任务"
        );
        assert!(s.list_pipelines(Scope::user(&b)).await.unwrap().is_empty());
        assert!(
            s.get_schedule(Scope::user(&b), "sched-a")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            s.get_pipeline(Scope::user(&b), "pipe-a")
                .await
                .unwrap()
                .is_none()
        );

        // 删也删不掉，且**返回 false 而不是报错**（不可见等价于不存在）
        assert!(!s.delete_schedule(Scope::user(&b), "sched-a").await.unwrap());
        assert_eq!(
            s.list_schedules(Scope::user(&a)).await.unwrap().len(),
            1,
            "B 的删除不该动到 A 的行"
        );

        // Scope::Local（CLI 口径）不过滤：同一份过滤表达式，两种范围
        assert_eq!(s.list_schedules(Scope::Local).await.unwrap().len(), 1);
        assert_eq!(s.list_pipelines(Scope::Local).await.unwrap().len(), 1);
    }

    /// 写越权：B 不能拿同一个 id 覆盖 A 的任务。
    ///
    /// 这条比"读不到"更要紧：id 是主键、写入是 `ON CONFLICT DO UPDATE`，
    /// 只过滤读的话越权会从"看得到"升级成"改得掉"。
    #[tokio::test]
    async fn a_cannot_overwrite_another_users_schedule() {
        let s = store();
        let (a, b) = two_users(&s).await;

        let mut mine = schedule("sched-1");
        mine.owner = Some(a.clone());
        s.upsert_schedule(Scope::user(&a), &mine).await.unwrap();

        let mut theirs = schedule("sched-1");
        theirs.owner = Some(b.clone());
        theirs.name = "B 的任务".into();
        let err = s
            .upsert_schedule(Scope::user(&b), &theirs)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), mcloud_core::ErrorKind::BadRequest, "{err}");
        assert!(
            format!("{err}").contains("其他用户"),
            "要给出可行动的提示，而不是让调用方猜: {err}"
        );

        let loaded = s
            .get_schedule(Scope::user(&a), "sched-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.name, "每日签到", "A 的行不能被改动");

        // 也不能把自己写的东西写成"无主"（那会立刻从所有人视野里消失）
        let mut orphan = schedule("sched-2");
        orphan.owner = None;
        assert!(
            s.upsert_schedule(Scope::user(&a), &orphan).await.is_err(),
            "Scope::User 不该写出无主的行"
        );
    }

    /// 未归属的历史数据：**谁都看不见**；认领后归首个管理员，且幂等、不可被抢。
    #[tokio::test]
    async fn claiming_covers_schedules_and_pipelines() {
        let s = store();
        let (a, _b) = two_users(&s).await;

        // 迁移留下的无主数据（`Scope::Local` 是迁移/CLI 的口径）
        s.upsert_schedule(Scope::Local, &schedule("sched-legacy"))
            .await
            .unwrap();
        s.upsert_pipeline(Scope::Local, &pipeline("pipe-legacy"))
            .await
            .unwrap();

        assert!(
            s.list_schedules(Scope::user(&a)).await.unwrap().is_empty(),
            "无主的定时任务谁都看不见 —— 宁可暂时不出现，也不要谁都拿得到"
        );

        let claimed = s.claim_unowned(&a).await.unwrap();
        assert_eq!(
            (claimed.accounts, claimed.schedules, claimed.pipelines),
            (0, 1, 1)
        );
        assert_eq!(s.list_schedules(Scope::user(&a)).await.unwrap().len(), 1);
        assert_eq!(s.list_pipelines(Scope::user(&a)).await.unwrap().len(), 1);

        assert_eq!(
            s.claim_unowned(&a).await.unwrap(),
            ClaimedRows::default(),
            "认领必须幂等"
        );
        assert_eq!(
            s.claim_unowned(&UserId::new("late-user")).await.unwrap(),
            ClaimedRows::default(),
            "已归属的数据不能被后来者抢走"
        );
    }
}
