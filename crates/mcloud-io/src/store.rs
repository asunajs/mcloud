//! SQLite 存储实现。
//!
//! # 为什么用 SQLite（这是对原项目惯例的有意变更）
//!
//! 原项目用本地 JSON（`packages/api/data/*.json`）。但重写后有三类**查询型**需求：
//! 30 天云朵趋势、运行历史按账号/能力/状态筛选、事件流回放。
//! JSON 方案每次都要整文件读写 + 内存全表扫，既慢又易损。
//! 因此改为 SQLite（`rusqlite` bundled，无系统依赖），
//! 而 `asign.json` 仍作为配置源保留 —— 配置要人手改，数据不要。
//!
//! # 并发模型
//!
//! 单进程、本地文件、操作都是微秒级，因此用 `Mutex<Connection>` 顺序化，
//! 不做连接池。所有方法体内**不含** `.await`，所以持锁不会跨越 await 点。

use chrono::{DateTime, Utc};
use mcloud_core::{
    Account, AccountId, AccountState, ClaimedRows, CloudStats, Error, RunEvent, RunId, RunRecord,
    RunStatus, Scope, Seq, Sequenced, SmidState, StateStore, UserId,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

mod config;
mod schema;
mod user;

pub use config::ImportRecord;
pub use user::issue_session;

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// 打开（或创建）数据库并执行迁移。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Internal(format!("创建数据目录失败 {}: {e}", parent.display()))
            })?;
        }
        let conn = Connection::open(path)
            .map_err(|e| Error::Internal(format!("打开数据库失败 {}: {e}", path.display())))?;
        Self::from_connection(conn)
    }

    /// 内存数据库，用于单测。
    pub fn open_in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()
            .map_err(|e| Error::Internal(format!("创建内存数据库失败: {e}")))?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, Error> {
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(map_sqlite)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(map_sqlite)?;
        schema::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 当前 schema 版本（供"数据版本与迁移函数清单一致"的门禁使用）。
    pub fn schema_version(&self) -> Result<i64, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(map_sqlite)
    }

    /// 直接拿到连接（**仅测试**）。
    ///
    /// 有些断言必须绕过 API 才能表达，例如"无归属的旧记录确实写进了库、
    /// 但不属于任何用户" —— 用 API 恰好表达不了这件事（那正是隔离本身），
    /// 而少了这条断言，"旧历史被悄悄丢掉"就没人盯着了。
    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Mutex<Connection> {
        &self.conn
    }
}

fn map_sqlite(err: rusqlite::Error) -> Error {
    Error::Internal(format!("SQLite 错误: {err}"))
}

/// 校验账号属于该用户；否则返回 [`Error::NotFound`]。
///
/// # 为什么写入路径用 `NotFound` 而不是 `Forbidden`
///
/// 写入路径没法像读取路径那样返回 `None`，但**对外结论必须与读取路径一致**：
/// 读取说"没有这个账号"，写入却说"你不能写这个账号"，两者的差异本身就告诉调用方
/// "这个 id 是存在的"，账号 id 空间于是变成可枚举的。
fn validate_owned(conn: &Connection, scope: Scope<'_>, id: &AccountId) -> Result<(), Error> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM accounts WHERE id = ?1 AND (?2 IS NULL OR owner = ?2)",
            params![id.as_str(), scope.owner().map(UserId::as_str)],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    found
        .map(|_| ())
        .ok_or_else(|| Error::NotFound(format!("账号 {}", id.as_str())))
}

/// 确认这个 run 在范围内 —— **`run_events` 的归属校验全在这里**。
///
/// # 为什么要顺着 `runs` 查，而不是给事件表加一个 `owner` 列
///
/// 事件行上没有 `owner`，只有 `run_id`。给事件表加一列"冗余的 owner"看起来省事，
/// 但那会立刻带来一个经典问题：**同一份事实两个存储位置，迟早不一致** ——
/// 而修复方式只能靠"记得两个地方一起改"。顺着 `runs` 查一次只有一次 JOIN 的成本，
/// 换来的是"归属只有一个来源"。
///
/// # 为什么 `account_id` 为空的 run 只有 `Scope::Local` 能用
///
/// 与 [`StateStore::list_runs`] 对它们的可见性完全一致：那种 run 无法归属
/// （旧数据导入时"匹配不到账号"的历史），所以它不出现任何人的列表里。
/// 若这里放行 `Scope::User`，服务层就能往一个自己看不见的 run 里塞日志 ——
/// 用户看不到，但它占着库。
///
/// # 为什么必须先确认 run 存在
///
/// 不确认的话 `run_events` 会接受任何 `run_id`，于是"事件表里有 500 行、
/// `runs` 里一行都没有"这种状态可以出现（写错 id、或 run 行被删之后又有人补写）。
/// 这类孤儿行会让"按 run 查事件"永远返回空，而排查时又看不出问题在哪。
///
/// 返回 `NotFound`（不是 `Internal`）：对调用者而言，"这个 run 不是你的"与
/// "这个 run 不存在"本来就是同一个结论，而且**报什么错本身不能泄漏它存在** ——
/// 这条与 `update_run` / `runs.rs` 的 `updating_a_missing_or_foreign_run_is_an_error`
/// 是同一条理由。
fn validate_run_in_scope(conn: &Connection, scope: Scope<'_>, run_id: &RunId) -> Result<(), Error> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM runs \
             WHERE id = ?1 \
               AND (?2 IS NULL OR account_id IN (SELECT id FROM accounts WHERE owner = ?2))",
            params![run_id.as_str(), scope.owner().map(UserId::as_str)],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    found
        .map(|_| ())
        .ok_or_else(|| Error::NotFound(format!("运行记录 {}", run_id.as_str())))
}

/// 事件在库里的 `kind` 列取值（= JSON 里的 `type` 标签）。
///
/// # 为什么单独存一列，而不是每次从 JSON 里解
///
/// 它让"只看某一类事件"（比如排障时只想看 `errored`）能走 SQL 的 `WHERE kind = ?`，
/// 不必把每个 payload 都解一遍。JSON 里**也**有 `type`，两者冗余但同源 ——
/// 都由这个函数产出，不会漂移。
fn event_kind(event: &RunEvent) -> &'static str {
    match event {
        RunEvent::Started { .. } => "started",
        RunEvent::Log { .. } => "log",
        RunEvent::Progress { .. } => "progress",
        RunEvent::Finished { .. } => "finished",
        RunEvent::Errored { .. } => "errored",
    }
}

/// 把事件序列化成落库的 payload。
///
/// 序列化失败在这里是 `Internal` 而不是丢弃：`RunEvent` 全是 `String`/数字/枚举，
/// 正常不可能失败。真失败了说明类型定义里混进了不可序列化的东西 ——
/// 那是**程序缺陷**，静默丢一条事件只会让排查更晚。
fn serialize_event(event: &RunEvent) -> Result<String, Error> {
    serde_json::to_string(event)
        .map_err(|e| Error::Internal(format!("运行事件序列化失败（类型定义有问题）: {e}")))
}

/// 从 payload 还原事件。
///
/// # 解不出来时给的是 `Internal`，不是"跳过这条"
///
/// 这个选择影响排障体验：跳过会让前端看到一个**少了中间几条**的进度
/// （比如 `progress 3/20` 之后直接跳到 `9/20`），而人只会怀疑"是不是漏发了"，
/// 不会想到是库里有一条坏了。直接报错至少能把"哪一行坏了"暴露出来。
fn deserialize_event(payload: &str) -> Result<RunEvent, Error> {
    serde_json::from_str(payload)
        .map_err(|e| Error::Internal(format!("运行事件反序列化失败（payload: {payload}）: {e}")))
}

/// rusqlite 对 `DateTime<Utc>` 的存取统一走 RFC 3339 字符串，
/// 便于人工查看与跨工具导出。
fn dt_to_string(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn dt_from_string(value: String) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// `accounts` 的列顺序 —— **只此一处**。
///
/// 加列时改这里 + [`row_to_account`] + `upsert_account`；
/// 三条 SELECT 各写一份列清单必然漂移，而漂移的表现是
/// "某条查询静默少一列"，不会报错。
const ACCOUNT_COLUMNS: &str = "id, nickname, phone_masked, platform, enabled, sort_order, \
     cloud_current, cloud_updated_at, auth_expire, last_ok_at, fail_streak, note, owner";

fn user_id_from_opt(value: Option<String>) -> Option<mcloud_core::UserId> {
    value.map(mcloud_core::UserId::new)
}

/// 读取**非空**时间列。
///
/// 解析失败时报错，而不是退回一个 "现在" —— 后者会让"库里存着坏时间"这件事
/// 永远不被发现，而它偏偏出现在 `created_at` / `expires_at` 这种关键列上
/// （`created_at` 变成现在看不出来，`expires_at` 变成现在则会让会话静默失效）。
fn dt_required(value: String, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    dt_from_string(value.clone()).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            format!("时间列不是合法 RFC 3339: {value}").into(),
        )
    })
}

impl StateStore for SqliteStore {
    async fn upsert_account(&self, scope: Scope<'_>, account: &Account) -> Result<(), Error> {
        // ① 用户范围下不能替别人建账号，也不能把账号写成"无主"
        //（无主只由 schema 迁移与本地运维产生）。
        //   本地范围不做这条检查 —— 见 `Scope::Local` 的说明。
        if let Some(user_id) = scope.owner()
            && account.owner.as_ref() != Some(user_id)
        {
            return Err(Error::NotFound(format!("账号 {}", account.id.as_str())));
        }
        if account.id.as_str().is_empty() {
            return Err(Error::BadRequest("账号 id 不能为空".into()));
        }

        let conn = self.conn.lock().expect("数据库锁中毒");

        // ② 同一个 AccountId 已被别人占用时**不能覆盖**。
        //
        // AccountId 由脱敏手机号派生，是**全局唯一**的，所以"两个系统用户添加同一个
        // 云盘账号"在当前的 `accounts(id)` 主键下做不到 —— 这是已知限制，正确修法是
        // 把主键改成 `(owner, id)`（需一次表重建迁移）。
        // 在那之前这里要被显式挡住，而不是让 upsert 静默把别人的账号顶掉。
        let existing_owner: Option<Option<String>> = conn
            .query_row(
                "SELECT owner FROM accounts WHERE id = ?1",
                params![account.id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        if let Some(Some(other)) = existing_owner
            && scope
                .owner()
                .is_some_and(|user_id| other != user_id.as_str())
        {
            // 给出可行动的提示：让它表现成"账号不存在"会变成查不出原因的怪象
            return Err(Error::BadRequest("该账号已被其他用户添加".into()));
        }

        conn.execute(
            r#"
            INSERT INTO accounts (
                id, nickname, phone_masked, platform, enabled, sort_order,
                cloud_current, cloud_updated_at, auth_expire, last_ok_at, fail_streak, note, owner
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(id) DO UPDATE SET
                nickname         = excluded.nickname,
                phone_masked     = excluded.phone_masked,
                platform         = excluded.platform,
                enabled          = excluded.enabled,
                sort_order       = excluded.sort_order,
                cloud_current    = excluded.cloud_current,
                cloud_updated_at = excluded.cloud_updated_at,
                auth_expire      = excluded.auth_expire,
                last_ok_at       = excluded.last_ok_at,
                fail_streak      = excluded.fail_streak,
                note             = excluded.note,
                owner            = excluded.owner
            "#,
            params![
                account.id.as_str(),
                account.nickname,
                account.phone_masked,
                account.platform,
                account.enabled as i64,
                account.sort_order,
                account.cloud_current,
                account.cloud_updated_at.as_ref().map(dt_to_string),
                account.auth_expire,
                account.last_ok_at.as_ref().map(dt_to_string),
                account.fail_streak,
                account.note,
                account.owner.as_ref().map(|o| o.as_str()),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn get_account(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
    ) -> Result<Option<Account>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 归属条件**写进 SQL**：不属于该用户的行根本不会出现在结果集里，
        // 所以它天然返回 None（与"不存在"同一个结论）
        conn.query_row(
            &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1 AND (?2 IS NULL OR owner = ?2)"),
            params![id.as_str(), scope.owner().map(UserId::as_str)],
            row_to_account,
        )
        .optional()
        .map_err(map_sqlite)
    }

    async fn list_accounts(&self, scope: Scope<'_>) -> Result<Vec<Account>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE (?1 IS NULL OR owner = ?1) \
                 ORDER BY sort_order ASC, id ASC"
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![scope.owner().map(UserId::as_str)], row_to_account)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    async fn set_account_cloud(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
        count: i64,
        at: DateTime<Utc>,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 归属检查放在 UPDATE 之前：`Scope::User` 下写别人的账号要报错，
        // 而不是"UPDATE 影响 0 行"这种看不出对错的静默结果。
        validate_owned(&conn, scope, id)?;
        conn.execute(
            "UPDATE accounts SET cloud_current = ?1, cloud_updated_at = ?2 WHERE id = ?3",
            params![count, dt_to_string(&at), id.as_str()],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn get_account_state(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
    ) -> Result<Option<AccountState>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 归属条件用 JOIN 和取值放在**同一条语句**里：先校验再取会有 TOCTOU 窗口，
        // 而且那条路径上"忘了校验"不会有任何提示
        conn.query_row(
            "SELECT s.account_id, s.jwt_token, s.device_id, s.user_domain_id, \
             s.cloud_phone_redpack_token, s.phone, s.authorization, s.updated_at \
             FROM account_state s JOIN accounts a ON a.id = s.account_id \
             WHERE s.account_id = ?1 AND (?2 IS NULL OR a.owner = ?2)",
            params![id.as_str(), scope.owner().map(UserId::as_str)],
            |row| {
                let updated_at: Option<String> = row.get(7)?;
                Ok(AccountState {
                    account_id: AccountId::new(row.get::<_, String>(0)?),
                    jwt_token: row.get(1)?,
                    device_id: row.get(2)?,
                    user_domain_id: row.get(3)?,
                    cloud_phone_redpack_token: row.get(4)?,
                    phone: row.get(5)?,
                    authorization: row.get(6)?,
                    updated_at: updated_at.and_then(dt_from_string),
                })
            },
        )
        .optional()
        .map_err(map_sqlite)
    }

    /// 覆盖写入登录态。
    ///
    /// 是**覆盖**而不是"合并"：登录态是一个整体快照，`None` 表示"上游这次没给"，
    /// 若做合并就永远删不掉已经失效的 token（例如重新登录换了新 deviceId 之后，
    /// 旧 deviceId 还留在库里，下次就会被拿出来用）。
    async fn upsert_account_state(
        &self,
        scope: Scope<'_>,
        state: &AccountState,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        validate_owned(&conn, scope, &state.account_id)?;
        conn.execute(
            r#"
            INSERT INTO account_state (
                account_id, jwt_token, device_id, user_domain_id,
                cloud_phone_redpack_token, phone, authorization, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(account_id) DO UPDATE SET
                jwt_token                 = excluded.jwt_token,
                device_id                 = excluded.device_id,
                user_domain_id            = excluded.user_domain_id,
                cloud_phone_redpack_token = excluded.cloud_phone_redpack_token,
                phone                     = excluded.phone,
                authorization             = excluded.authorization,
                updated_at                = excluded.updated_at
            "#,
            params![
                state.account_id.as_str(),
                state.jwt_token,
                state.device_id,
                state.user_domain_id,
                state.cloud_phone_redpack_token,
                state.phone,
                state.authorization,
                state.updated_at.as_ref().map(dt_to_string),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    // ── smid：绑定账号的设备特征 + 最近拿到的 deviceId ──
    //
    // 这三条**刻意不走** `upsert_account_state` 那张"整体覆盖"的路，理由见
    // `mcloud_core::SmidState` 与 schema.rs 的 v5 注释：换凭据不该换设备长相。

    async fn get_smid_state(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
    ) -> Result<Option<SmidState>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 与 get_account_state 同一个形状：归属条件写在**同一条语句**里（TOCTOU 的理由见那处）
        conn.query_row(
            "SELECT s.account_id, s.smid_fingerprint, s.smid_device_id \
             FROM account_state s JOIN accounts a ON a.id = s.account_id \
             WHERE s.account_id = ?1 AND (?2 IS NULL OR a.owner = ?2)",
            params![id.as_str(), scope.owner().map(UserId::as_str)],
            |row| {
                Ok(SmidState {
                    account_id: AccountId::new(row.get::<_, String>(0)?),
                    fingerprint_json: row.get(1)?,
                    device_id: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite)
    }

    async fn set_smid_fingerprint(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
        fingerprint_json: &str,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        validate_owned(&conn, scope, id)?;
        // 写特征与清空旧 deviceId 是**同一条语句**：拆成两次调用，中间崩溃就留下
        // "新长相 + 旧 id"，而这两列各自都合法，事后看不出发生过什么。
        conn.execute(
            r#"
            INSERT INTO account_state (account_id, smid_fingerprint, smid_device_id)
            VALUES (?1, ?2, NULL)
            ON CONFLICT(account_id) DO UPDATE SET
                smid_fingerprint = excluded.smid_fingerprint,
                smid_device_id   = NULL
            "#,
            params![id.as_str(), fingerprint_json],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn set_smid_device_id(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
        device_id: &str,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        validate_owned(&conn, scope, id)?;
        // 只碰这一列，**不动 updated_at**：那一列记的是"登录态快照的时间"，
        // 一次指纹上报不是新快照（把它刷成 now，会让人以为这个账号刚重新登录过）。
        conn.execute(
            r#"
            INSERT INTO account_state (account_id, smid_device_id)
            VALUES (?1, ?2)
            ON CONFLICT(account_id) DO UPDATE SET
                smid_device_id = excluded.smid_device_id
            "#,
            params![id.as_str(), device_id],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    /// 把账号**缺**的 `authorization` / `phone` 两列填上 —— 只碰这两列，
    /// **不动 `updated_at`**，也**不覆盖已有值**。
    ///
    /// # 为什么 `COALESCE` 的两边是反的
    ///
    /// 写惯了 `= excluded.x` 的人会本能地把 `excluded` 放前面，那样就成了"有值就覆盖"。
    /// 这里要的是**填空缺**：已有值优先，`excluded` 只在那一列是 `NULL` 时才生效。
    /// 顺序反了不会报错，只会让"每次 `migrate` 都重跑的回填"把重登后的新凭据
    /// 用旧配置里的串盖回去 —— 而且事后完全看不出发生过什么。
    ///
    /// 入参 `None` 的语义是"这一列我没有可填的"，与 `Some` 在列已填时**行为相同**。
    ///
    /// # 为什么 `INSERT` 分支也不怕
    ///
    /// 历史账号可能**根本没有 `account_state` 行**（v6 之前只登过、没上报过指纹）。
    /// 那时插入的是一行"只有这两列"的状态，其余列按 `NULL` 落 —— 与
    /// `upsert_account_state` 的整体覆盖语义不冲突（它本来就是覆盖）。
    async fn fill_account_credentials(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
        authorization: Option<&str>,
        phone: Option<&str>,
    ) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        validate_owned(&conn, scope, id)?;
        // 刻意**不写 updated_at**：那一列是"登录态快照的时间"，补一个缺口不是新快照。
        conn.execute(
            r#"
            INSERT INTO account_state (account_id, authorization, phone)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(account_id) DO UPDATE SET
                authorization = COALESCE(account_state.authorization, excluded.authorization),
                phone         = COALESCE(account_state.phone,         excluded.phone)
            "#,
            params![id.as_str(), authorization, phone],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn upsert_cloud_stats(&self, scope: Scope<'_>, stats: &CloudStats) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        validate_owned(&conn, scope, &stats.account_id)?;
        conn.execute(
            r#"
            INSERT INTO cloud_stats (account_id, date, cloud_count, cloud_diff, cloud_diff_week)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(account_id, date) DO UPDATE SET
                cloud_count     = excluded.cloud_count,
                cloud_diff      = excluded.cloud_diff,
                cloud_diff_week = excluded.cloud_diff_week
            "#,
            params![
                stats.account_id.as_str(),
                stats.date,
                stats.cloud_count,
                stats.cloud_diff,
                stats.cloud_diff_week,
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn cloud_stats_since(
        &self,
        scope: Scope<'_>,
        id: &AccountId,
        since: &str,
    ) -> Result<Vec<CloudStats>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(
                "SELECT s.account_id, s.date, s.cloud_count, s.cloud_diff, s.cloud_diff_week \
                 FROM cloud_stats s JOIN accounts a ON a.id = s.account_id \
                 WHERE s.account_id = ?1 AND (?2 IS NULL OR a.owner = ?2) AND s.date >= ?3 \
                 ORDER BY s.date ASC",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(
                params![id.as_str(), scope.owner().map(UserId::as_str), since],
                |row| {
                    Ok(CloudStats {
                        account_id: AccountId::new(row.get::<_, String>(0)?),
                        date: row.get(1)?,
                        cloud_count: row.get(2)?,
                        cloud_diff: row.get(3)?,
                        cloud_diff_week: row.get(4)?,
                    })
                },
            )
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    async fn insert_run(&self, scope: Scope<'_>, run: &RunRecord) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 带账号的运行记录必须先确认账号是自己的；不带账号的（旧数据里"匹配不到账号"
        // 的历史）允许写入 —— 它随后通过 list_runs 也对谁都不可见，只是留个痕迹。
        if let Some(account_id) = &run.account_id {
            validate_owned(&conn, scope, account_id)?;
        }
        conn.execute(
            r#"
            INSERT INTO runs (
                id, account_id, capability, status, started_at, finished_at,
                cloud_gained, cloud_after, message
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO NOTHING
            "#,
            params![
                run.id,
                run.account_id.as_ref().map(|a| a.as_str().to_string()),
                run.capability,
                run_status_to_str(run.status),
                dt_to_string(&run.started_at),
                run.finished_at.as_ref().map(dt_to_string),
                run.cloud_gained,
                run.cloud_after,
                run.message,
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn update_run(&self, scope: Scope<'_>, run: &RunRecord) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let affected = conn
            .execute(
                r#"
                UPDATE runs SET
                    status      = ?2,
                    finished_at = ?3,
                    cloud_gained = ?4,
                    cloud_after  = ?5,
                    message      = ?6
                WHERE id = ?1
                  AND (?7 IS NULL OR account_id IN (SELECT id FROM accounts WHERE owner = ?7))
                "#,
                params![
                    run.id,
                    run_status_to_str(run.status),
                    run.finished_at.as_ref().map(dt_to_string),
                    run.cloud_gained,
                    run.cloud_after,
                    run.message,
                    scope.owner().map(UserId::as_str),
                ],
            )
            .map_err(map_sqlite)?;
        if affected == 0 {
            // 与读取路径同一个结论：不属于你的记录，对你而言就是不存在。
            // （不用 Internal：那是"程序内部出问题"，而这是正常的越权请求。）
            return Err(Error::NotFound(format!("运行记录 {}", run.id)));
        }
        Ok(())
    }

    async fn list_runs(&self, scope: Scope<'_>, limit: usize) -> Result<Vec<RunRecord>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // `account_id` 为空的记录**不出现**在任何人的列表里：它无法归属，
        // 而"无法归属就谁都看得见"会让别人的历史混进我的列表。
        // 这类记录的去处是迁移报告（`MigrationIssue::UnresolvedRunAccount`），不是这里。
        let mut stmt = conn
            .prepare(
                "SELECT id, account_id, capability, status, started_at, finished_at, \
                 cloud_gained, cloud_after, message \
                 FROM runs \
                 WHERE (?1 IS NULL OR account_id IN (SELECT id FROM accounts WHERE owner = ?1)) \
                 ORDER BY started_at DESC LIMIT ?2",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(
                params![scope.owner().map(UserId::as_str), limit as i64],
                |row| {
                    let status: String = row.get(3)?;
                    let started: String = row.get(4)?;
                    let finished: Option<String> = row.get(5)?;
                    Ok(RunRecord {
                        id: row.get(0)?,
                        account_id: row.get::<_, Option<String>>(1)?.map(AccountId::new),
                        capability: row.get(2)?,
                        status: run_status_from_str(&status),
                        started_at: dt_required(started, 4)?,
                        finished_at: finished.and_then(dt_from_string),
                        cloud_gained: row.get(6)?,
                        cloud_after: row.get(7)?,
                        message: row.get(8)?,
                    })
                },
            )
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    /// 追加运行事件。
    async fn append_run_events(
        &self,
        scope: Scope<'_>,
        run_id: &RunId,
        events: &[Sequenced],
    ) -> Result<(), Error> {
        if events.is_empty() {
            // 空批不校验归属：没有东西可写，报"不属于你"会让一个无害的调用
            // 看起来像越权。但也不开事务 —— 空事务只是白跑一趟。
            return Ok(());
        }
        let mut conn = self.conn.lock().expect("数据库锁中毒");
        validate_run_in_scope(&conn, scope, run_id)?;

        let tx = conn.transaction().map_err(map_sqlite)?;
        let now = dt_to_string(&Utc::now());
        {
            let mut stmt = tx
                .prepare(
                    r#"
                    INSERT INTO run_events (run_id, seq, kind, payload, created_at)
                    VALUES (?1, ?2, ?3, ?4, ?5)
                    ON CONFLICT(run_id, seq) DO NOTHING
                    "#,
                )
                .map_err(map_sqlite)?;
            for sequenced in events {
                stmt.execute(params![
                    run_id.as_str(),
                    // seq 是 u64，SQLite 的 INTEGER 是有符号 64 位。
                    // 超过 i64::MAX 会回绕成负数 —— 现实中到不了（`Seq` 自己会先饱和），
                    // 所以这里用 `as i64` 而不是报错：报错会给一个跑不到的分支加一条错误路径。
                    sequenced.seq.value() as i64,
                    event_kind(&sequenced.event),
                    serialize_event(&sequenced.event)?,
                    now,
                ])
                .map_err(map_sqlite)?;
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(())
    }

    async fn run_events_after(
        &self,
        scope: Scope<'_>,
        run_id: &RunId,
        after: Option<Seq>,
        limit: usize,
    ) -> Result<Vec<Sequenced>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        validate_run_in_scope(&conn, scope, run_id)?;

        // `after = None` → 从头开始。序号从 1 起、0 从不出现，
        // 所以用 0 做下界与"没有下界"等价 —— 不必写两种 SQL。
        let from = after.map_or(0, Seq::value) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT seq, payload FROM run_events \
                 WHERE run_id = ?1 AND seq > ?2 \
                 ORDER BY seq ASC LIMIT ?3",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![run_id.as_str(), from, limit as i64], |row| {
                let seq: i64 = row.get(0)?;
                let payload: String = row.get(1)?;
                Ok((seq, payload))
            })
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;

        rows.into_iter()
            .map(|(seq, payload)| {
                let event = deserialize_event(&payload)?;
                // 存的是 `seq` 本身（不是"seen"那种语义），所以直接按原值还原。
                // 用 `resume_after(seq - 1)` 也能得到同一个值，但那是在绕圈子里
                // 表达"这就是 seq"，读的人得先算一遍才知道对不对。
                Ok(Sequenced::new(Seq::from_stored(seq), event))
            })
            .collect()
    }

    async fn delete_run_events(&self, scope: Scope<'_>, run_id: &RunId) -> Result<usize, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        validate_run_in_scope(&conn, scope, run_id)?;
        let affected = conn
            .execute(
                "DELETE FROM run_events WHERE run_id = ?1",
                params![run_id.as_str()],
            )
            .map_err(map_sqlite)?;
        Ok(affected)
    }

    /// 认领未归属的数据（`owner IS NULL`）：账号、定时任务、组合。
    ///
    /// 实现见 `store/user.rs` 的理由说明，放在这里是因为该契约属于 `StateStore`，
    /// 而**同一个 trait 不能拆成两个 impl 块**。
    async fn claim_unowned(&self, user_id: &mcloud_core::UserId) -> Result<ClaimedRows, Error> {
        let mut conn = self.conn.lock().expect("数据库锁中毒");
        // 三张表在**同一个事务**里认领：半途而废会留下"账号回来了、定时任务没回来"
        // 这种很难向用户解释的状态，而它偏偏发生在升级路径上（用户第一眼就会看到）。
        //
        // 三段 SQL 看着像该被抽成循环，但循环需要一个"把结果写回哪个字段"的映射，
        // 为省三行引入 unsafe 或元组指针都不值 —— 显式写三遍最安全也最好读。
        let tx = conn.transaction().map_err(map_sqlite)?;
        let accounts = tx
            .execute(
                "UPDATE accounts SET owner = ?1 WHERE owner IS NULL",
                params![user_id.as_str()],
            )
            .map_err(map_sqlite)? as u64;
        let schedules = tx
            .execute(
                "UPDATE schedules SET owner = ?1 WHERE owner IS NULL",
                params![user_id.as_str()],
            )
            .map_err(map_sqlite)? as u64;
        let pipelines = tx
            .execute(
                "UPDATE pipelines SET owner = ?1 WHERE owner IS NULL",
                params![user_id.as_str()],
            )
            .map_err(map_sqlite)? as u64;
        tx.commit().map_err(map_sqlite)?;
        Ok(ClaimedRows {
            accounts,
            schedules,
            pipelines,
        })
    }
}

fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    let cloud_updated_at: Option<String> = row.get(7)?;
    let last_ok_at: Option<String> = row.get(9)?;
    Ok(Account {
        id: AccountId::new(row.get::<_, String>(0)?),
        nickname: row.get(1)?,
        phone_masked: row.get(2)?,
        platform: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        sort_order: row.get(5)?,
        cloud_current: row.get(6)?,
        cloud_updated_at: cloud_updated_at.and_then(dt_from_string),
        auth_expire: row.get(8)?,
        last_ok_at: last_ok_at.and_then(dt_from_string),
        fail_streak: row.get(10)?,
        note: row.get(11)?,
        // v3 加在最后：列序只此一处（见 ACCOUNT_COLUMNS），
        // 放末尾可以让这次改动的下标变动最小
        owner: user_id_from_opt(row.get(12)?),
    })
}

const fn run_status_to_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Done => "done",
        RunStatus::Failed => "failed",
    }
}

fn run_status_from_str(value: &str) -> RunStatus {
    match value {
        "done" => RunStatus::Done,
        "failed" => RunStatus::Failed,
        _ => RunStatus::Running,
    }
}

#[cfg(test)]
mod tests;
