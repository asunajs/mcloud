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
    Account, AccountId, AccountState, ClaimedRows, CloudStats, Error, RunRecord, RunStatus, Scope,
    SmidState, StateStore, UserId,
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
mod tests {
    use super::*;
    use crate::store::schema::SCHEMA_VERSION;
    use chrono::TimeZone;
    use mcloud_core::UserStore;
    use mcloud_core::pure;

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("内存库应可打开")
    }

    /// 测试用户。
    ///
    /// v3 起账号必须有主人（`accounts.owner` 有外键指向 `users`，隔离也靠它过滤），
    /// 所以碰账号的用例都先要有一个用户 —— 这正是生产的顺序（先 `user init` 再导入）。
    const TEST_USER: &str = "test-user";

    fn uid() -> UserId {
        UserId::new(TEST_USER)
    }

    /// 建好测试用户的库。
    async fn store_with_user() -> SqliteStore {
        let s = store();
        s.upsert_user(&mcloud_core::User::new(
            TEST_USER,
            TEST_USER,
            "!",
            mcloud_core::Role::Admin,
            Utc::now(),
        ))
        .await
        .expect("建测试用户");
        s
    }

    fn account(id: &str) -> Account {
        Account {
            nickname: Some("账号2".into()),
            phone_masked: Some("199****0001".into()),
            // 统一归给测试用户；不设的话写入会被隔离校验拒掉
            owner: Some(uid()),
            ..Account::new(id)
        }
    }

    /// 建一个已归属的账号行。
    ///
    /// 写云朵快照 / 运行记录之前必须有这一行：隔离校验靠 `accounts.owner`，
    /// 账号不存在时那些写入会被正确地拒掉（这正是它该有的行为）。
    async fn put_account(s: &SqliteStore, id: &str) {
        s.upsert_account(Scope::user(&uid()), &account(id))
            .await
            .unwrap();
    }

    #[test]
    fn schema_version_is_recorded() {
        let s = store();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn account_roundtrip() {
        let s = store_with_user().await;
        let mut acc = account("acc-1");
        acc.cloud_current = Some(1286);
        acc.auth_expire = Some(1_893_456_000_000);
        acc.note = Some("云图书馆阅读月卡".into());

        s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

        let loaded = s
            .get_account(Scope::user(&uid()), &AccountId::new("acc-1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.nickname.as_deref(), Some("账号2"));
        assert_eq!(loaded.cloud_current, Some(1286));
        assert_eq!(loaded.auth_expire, Some(1_893_456_000_000));
        assert_eq!(loaded.note.as_deref(), Some("云图书馆阅读月卡"));
        assert!(loaded.enabled);
    }

    #[tokio::test]
    async fn upsert_account_updates_in_place() {
        let s = store_with_user().await;
        let acc = account("acc-1");
        s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

        let mut updated = acc.clone();
        updated.cloud_current = Some(1300);
        updated.fail_streak = 2;
        s.upsert_account(Scope::user(&uid()), &updated)
            .await
            .unwrap();

        let all = s.list_accounts(Scope::user(&uid())).await.unwrap();
        assert_eq!(all.len(), 1, "重复写入不应产生第二条账号");
        assert_eq!(all[0].cloud_current, Some(1300));
        assert_eq!(all[0].fail_streak, 2);
    }

    /// 窄写的意义：只刷余额两列，不把"读出来时"的其余列一起写回去。
    ///
    /// 反例很具体 —— 若这里改用 [`StateStore::upsert_account`]（整行覆盖），
    /// 调用方就得"读整行 → 改余额 → 写回"，而读与写之间别人改的昵称
    /// 会被**静默写回旧值**：一个只在列表页显示错名字的 bug，查起来毫无头绪。
    #[tokio::test]
    async fn set_account_cloud_touches_only_the_two_cloud_columns() {
        let s = store_with_user().await;
        let mut acc = account("acc-1");
        acc.cloud_current = Some(1286);
        acc.fail_streak = 3;
        s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

        let at = Utc.with_ymd_and_hms(2026, 9, 19, 6, 0, 0).unwrap();
        s.set_account_cloud(Scope::user(&uid()), &AccountId::new("acc-1"), 5392, at)
            .await
            .unwrap();

        let loaded = s
            .get_account(Scope::user(&uid()), &AccountId::new("acc-1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.cloud_current, Some(5392));
        assert_eq!(loaded.cloud_updated_at, Some(at));
        assert_eq!(
            loaded.nickname.as_deref(),
            Some("账号2"),
            "昵称不该被顺手覆盖"
        );
        assert_eq!(loaded.fail_streak, 3, "健康度不该被顺手清零");
        assert!(loaded.enabled);
        assert_eq!(loaded.owner.as_ref(), Some(&uid()));
    }

    #[tokio::test]
    async fn set_account_cloud_refuses_a_foreign_account() {
        let s = store_with_user().await;
        let mut acc = account("acc-1");
        acc.cloud_current = Some(1286);
        s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

        let at = Utc.with_ymd_and_hms(2026, 9, 19, 6, 0, 0).unwrap();
        let err = s
            .set_account_cloud(
                Scope::user(&UserId::new("别人")),
                &AccountId::new("acc-1"),
                5392,
                at,
            )
            .await
            .unwrap_err();

        assert_eq!(err.kind(), mcloud_core::ErrorKind::NotFound);
        let loaded = s
            .get_account(Scope::user(&uid()), &AccountId::new("acc-1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.cloud_current,
            Some(1286),
            "被拒的写入不该改到任何东西"
        );
    }

    #[tokio::test]
    async fn accounts_are_ordered_by_sort_order() {
        let s = store_with_user().await;
        let mut a2 = account("acc-2");
        a2.sort_order = 2;
        let mut a1 = account("acc-1");
        a1.sort_order = 1;
        s.upsert_account(Scope::user(&uid()), &a2).await.unwrap();
        s.upsert_account(Scope::user(&uid()), &a1).await.unwrap();

        let ids: Vec<String> = s
            .list_accounts(Scope::user(&uid()))
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.id.to_string())
            .collect();
        assert_eq!(ids, vec!["acc-1", "acc-2"]);
    }

    #[tokio::test]
    async fn cloud_stats_upsert_is_keyed_by_account_and_date() {
        let s = store_with_user().await;
        let id = AccountId::new("acc-1");
        // 快照依附于账号：账号行必须先存在且归属于调用者
        put_account(&s, "acc-1").await;

        let day1 = pure::next_cloud_stats(&id, "2026-09-15", 1278, &[]);
        s.upsert_cloud_stats(Scope::user(&uid()), &day1)
            .await
            .unwrap();

        // 同日重复采集应覆盖而不是新增
        let day1_again = pure::next_cloud_stats(&id, "2026-09-15", 1280, &[]);
        s.upsert_cloud_stats(Scope::user(&uid()), &day1_again)
            .await
            .unwrap();

        let stats = s
            .cloud_stats_since(Scope::user(&uid()), &id, "2026-09-01")
            .await
            .unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].cloud_count, 1280);
    }

    #[tokio::test]
    async fn cloud_stats_since_filters_and_orders() {
        let s = store_with_user().await;
        let id = AccountId::new("acc-1");
        put_account(&s, "acc-1").await;
        let mut history: Vec<CloudStats> = Vec::new();

        for (date, count) in [
            ("2026-08-01", 900),
            ("2026-09-10", 1200),
            ("2026-09-15", 1278),
        ] {
            let stats = pure::next_cloud_stats(&id, date, count, &history);
            s.upsert_cloud_stats(Scope::user(&uid()), &stats)
                .await
                .unwrap();
            history.push(stats);
        }

        let stats = s
            .cloud_stats_since(Scope::user(&uid()), &id, "2026-09-01")
            .await
            .unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].date, "2026-09-10");
        assert_eq!(stats[1].date, "2026-09-15");
    }

    #[tokio::test]
    async fn cloud_stats_are_isolated_per_account() {
        let s = store_with_user().await;
        let a = AccountId::new("acc-1");
        let b = AccountId::new("acc-2");
        put_account(&s, "acc-1").await;
        put_account(&s, "acc-2").await;
        s.upsert_cloud_stats(
            Scope::user(&uid()),
            &pure::next_cloud_stats(&a, "2026-09-15", 100, &[]),
        )
        .await
        .unwrap();
        s.upsert_cloud_stats(
            Scope::user(&uid()),
            &pure::next_cloud_stats(&b, "2026-09-15", 200, &[]),
        )
        .await
        .unwrap();

        assert_eq!(
            s.cloud_stats_since(Scope::user(&uid()), &a, "2026-01-01")
                .await
                .unwrap()[0]
                .cloud_count,
            100
        );
        assert_eq!(
            s.cloud_stats_since(Scope::user(&uid()), &b, "2026-01-01")
                .await
                .unwrap()[0]
                .cloud_count,
            200
        );
    }

    #[tokio::test]
    async fn run_lifecycle_insert_then_update() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let mut run = RunRecord {
            id: "run-1".into(),
            account_id: Some(AccountId::new("acc-1")),
            capability: "sign".into(),
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        };
        s.insert_run(Scope::user(&uid()), &run).await.unwrap();

        run.status = RunStatus::Done;
        run.finished_at = Some(Utc::now());
        run.cloud_gained = Some(8);
        run.cloud_after = Some(1286);
        run.message = Some("签到成功".into());
        s.update_run(Scope::user(&uid()), &run).await.unwrap();

        let runs = s.list_runs(Scope::user(&uid()), 10).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Done);
        assert_eq!(runs[0].cloud_gained, Some(8));
        assert_eq!(runs[0].cloud_after, Some(1286));
        assert!(runs[0].finished_at.is_some());
    }

    #[tokio::test]
    async fn duplicate_run_id_does_not_create_a_second_row() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let run = RunRecord {
            id: "run-1".into(),
            account_id: Some(AccountId::new("acc-1")),
            capability: "sign".into(),
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        };
        s.insert_run(Scope::user(&uid()), &run).await.unwrap();
        s.insert_run(Scope::user(&uid()), &run).await.unwrap();
        assert_eq!(s.list_runs(Scope::user(&uid()), 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn updating_a_missing_or_foreign_run_is_an_error() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let run = RunRecord {
            id: "run-1".into(),
            account_id: Some(AccountId::new("acc-1")),
            capability: "sign".into(),
            status: RunStatus::Failed,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        };

        // ① 记录不存在
        assert_eq!(
            s.update_run(Scope::user(&uid()), &run)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );

        // ② 记录存在、但调用者不是它账号的主人 → 必须给出**同一个**结论，
        //    否则"报什么错"本身就泄漏了这条记录存在
        s.insert_run(Scope::user(&uid()), &run).await.unwrap();
        let other = UserId::new("other-user");
        s.upsert_user(&mcloud_core::User::new(
            "other-user",
            "other-user",
            "!",
            mcloud_core::Role::User,
            Utc::now(),
        ))
        .await
        .unwrap();
        assert_eq!(
            s.update_run(Scope::user(&other), &run)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );

        // ③ 而主人自己仍然能更新 —— 确认这不是"谁都改不了"
        s.update_run(Scope::user(&uid()), &run).await.unwrap();
    }

    /// **数据隔离**的集中验收：两个用户各一个账号，把所有入口交叉摸一遍。
    ///
    /// 隔离规则是逐方法写在 SQL 里的，这条测试是唯一把它们串起来的地方 ——
    /// 漏掉任何一处（"忘了加 `owner` 条件"）都会在这里被抓住，
    /// 而漏掉一处不会有任何编译期提示（这正是 `docs/rust-rewrite-plan.md` §3.7 的要点）。
    #[tokio::test]
    async fn cross_user_isolation_is_enforced_on_every_entry_point() {
        let s = store_with_user().await;
        let alice = uid();
        let bob = UserId::new("bob");
        s.upsert_user(&mcloud_core::User::new(
            "bob",
            "bob",
            "!",
            mcloud_core::Role::User,
            Utc::now(),
        ))
        .await
        .unwrap();

        let mut alice_account = account("acc-a");
        alice_account.owner = Some(alice.clone());
        s.upsert_account(Scope::user(&alice), &alice_account)
            .await
            .unwrap();
        let mut bob_account = account("acc-b");
        bob_account.owner = Some(bob.clone());
        s.upsert_account(Scope::user(&bob), &bob_account)
            .await
            .unwrap();

        // ① 列表与单项读取：只看得见自己的
        let visible_to_alice = s.list_accounts(Scope::user(&alice)).await.unwrap();
        assert_eq!(visible_to_alice.len(), 1);
        assert_eq!(visible_to_alice[0].id.as_str(), "acc-a");
        assert_eq!(s.list_accounts(Scope::user(&bob)).await.unwrap().len(), 1);
        assert!(
            s.get_account(Scope::user(&alice), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            s.get_account(Scope::user(&bob), &AccountId::new("acc-a"))
                .await
                .unwrap()
                .is_none()
        );

        // ② 登录态：读不到，也写不进
        let bob_state = mcloud_core::AccountState::new("acc-b");
        assert_eq!(
            s.upsert_account_state(Scope::user(&alice), &bob_state)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );
        s.upsert_account_state(Scope::user(&bob), &bob_state)
            .await
            .unwrap();
        assert!(
            s.get_account_state(Scope::user(&alice), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            s.get_account_state(Scope::user(&bob), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_some()
        );

        // ②b smid 两列：同理。这里的后果比登录态更隐蔽 ——
        //      alice 若写得进 bob 的账号，就是替 bob 的账号凭空认下一台设备
        assert_eq!(
            s.set_smid_fingerprint(Scope::user(&alice), &AccountId::new("acc-b"), FP_V1)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );
        assert_eq!(
            s.set_smid_device_id(Scope::user(&alice), &AccountId::new("acc-b"), DEVICE_ID_1)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );
        s.set_smid_fingerprint(Scope::user(&bob), &AccountId::new("acc-b"), FP_V1)
            .await
            .unwrap();
        assert!(
            s.get_smid_state(Scope::user(&alice), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            s.get_smid_state(Scope::user(&bob), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_some()
        );

        // ③ 云朵快照：同理
        let stats = pure::next_cloud_stats(&AccountId::new("acc-b"), "2026-09-15", 100, &[]);
        assert_eq!(
            s.upsert_cloud_stats(Scope::user(&alice), &stats)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );
        s.upsert_cloud_stats(Scope::user(&bob), &stats)
            .await
            .unwrap();
        assert!(
            s.cloud_stats_since(Scope::user(&alice), &AccountId::new("acc-b"), "2026-01-01")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            s.cloud_stats_since(Scope::user(&bob), &AccountId::new("acc-b"), "2026-01-01")
                .await
                .unwrap()
                .len(),
            1
        );

        // ④ 运行记录：写、读、改三条路都要挡住
        let run = RunRecord {
            id: "run-b".into(),
            account_id: Some(AccountId::new("acc-b")),
            capability: "sign".into(),
            status: RunStatus::Done,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        };
        assert_eq!(
            s.insert_run(Scope::user(&alice), &run)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound,
            "不能往别人的账号上写运行记录"
        );
        s.insert_run(Scope::user(&bob), &run).await.unwrap();
        assert!(
            s.list_runs(Scope::user(&alice), 100)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.list_runs(Scope::user(&bob), 100).await.unwrap().len(), 1);
        assert_eq!(
            s.update_run(Scope::user(&alice), &run)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound,
            "不能改别人的运行记录"
        );
        s.update_run(Scope::user(&bob), &run).await.unwrap();

        // ⑤ 不能替别人建账号（owner 与调用者不一致就直接拒）
        let mut forged = account("acc-a2");
        forged.owner = Some(bob.clone());
        assert_eq!(
            s.upsert_account(Scope::user(&alice), &forged)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );

        // ⑥ 同一个 AccountId 被他人占用时不能覆盖：给出可行动的提示，
        //    而不是静默把对方的账号顶掉（AccountId 是脱敏手机号派生的，会撞）
        let mut duplicated = account("acc-b");
        duplicated.owner = Some(alice.clone());
        let err = s
            .upsert_account(Scope::user(&alice), &duplicated)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), mcloud_core::ErrorKind::BadRequest, "{err}");
        assert!(format!("{err}").contains("已被其他用户添加"), "{err}");
        assert_eq!(
            s.get_account(Scope::user(&bob), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .unwrap()
                .nickname
                .as_deref(),
            Some("账号2"),
            "被拒之后原账号必须原样还在"
        );
    }

    /// **本地范围**（CLI）：看得见所有人的账号，也能写任意账号的数据。
    ///
    /// 这条与上面那条隔离测试是**一对**：隔离管的是"网络上的用户"，
    /// 而拿着库文件的人本来就能操作所有 —— 两条都得成立，边界才说得清。
    #[tokio::test]
    async fn local_scope_sees_and_writes_everything() {
        let s = store_with_user().await;
        let alice = uid();
        let bob = UserId::new("bob");
        s.upsert_user(&mcloud_core::User::new(
            "bob",
            "bob",
            "!",
            mcloud_core::Role::User,
            Utc::now(),
        ))
        .await
        .unwrap();

        let mut a = account("acc-a");
        a.owner = Some(alice.clone());
        s.upsert_account(Scope::user(&alice), &a).await.unwrap();
        let mut b = account("acc-b");
        b.owner = Some(bob.clone());
        s.upsert_account(Scope::user(&bob), &b).await.unwrap();

        // ① 列表：两个都看得见（无论归谁）
        let all = s.list_accounts(Scope::Local).await.unwrap();
        assert_eq!(all.len(), 2, "本地视角必须看得见所有人的账号");

        // ② 单项：别人的账号也读得到（用户视角下这里是 None）
        assert!(
            s.get_account(Scope::Local, &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            s.get_account(Scope::user(&alice), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_none()
        );

        // ③ 写：本地视角可以写别人账号的登录态与快照
        let state = mcloud_core::AccountState::new("acc-b");
        s.upsert_account_state(Scope::Local, &state).await.unwrap();
        assert!(
            s.get_account_state(Scope::Local, &AccountId::new("acc-b"))
                .await
                .unwrap()
                .is_some()
        );
        let stats = pure::next_cloud_stats(&AccountId::new("acc-b"), "2026-09-15", 7, &[]);
        s.upsert_cloud_stats(Scope::Local, &stats).await.unwrap();
        assert_eq!(
            s.cloud_stats_since(Scope::Local, &AccountId::new("acc-b"), "2026-01-01")
                .await
                .unwrap()
                .len(),
            1
        );

        // ③b smid 两列：同理。CLI 侧的 `mcloud smid` 就是本地范围 ——
        //      拿着库文件的人本来就能改所有账号的设备长相，这与隔离不矛盾
        s.set_smid_fingerprint(Scope::Local, &AccountId::new("acc-b"), FP_V1)
            .await
            .unwrap();
        s.set_smid_device_id(Scope::Local, &AccountId::new("acc-b"), DEVICE_ID_1)
            .await
            .unwrap();
        let smid = s
            .get_smid_state(Scope::Local, &AccountId::new("acc-b"))
            .await
            .unwrap()
            .expect("本地范围读得到别人的两列");
        assert_eq!(smid.fingerprint_json.as_deref(), Some(FP_V1));
        assert_eq!(smid.device_id.as_deref(), Some(DEVICE_ID_1));
    }

    /// 本地范围能写**无主**账号 —— 这是"先 migrate / 先 login 再 init"能成立的关键。
    ///
    /// 用户范围下这条是被拒的（见 `upsert_account` 的说明）：无主账号对谁都不可见，
    /// 不该由"某个用户"随手造出来。
    #[tokio::test]
    async fn local_scope_can_create_unowned_accounts_but_user_scope_cannot() {
        let s = store_with_user().await;
        let alice = uid();

        let orphan = Account::new("acc-orphan"); // owner: None
        s.upsert_account(Scope::Local, &orphan).await.unwrap();

        // 无主账号对任何人都不可见，但本地视角看得见
        assert!(
            s.list_accounts(Scope::user(&alice))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.list_accounts(Scope::Local).await.unwrap().len(), 1);

        // 用户范围下造无主账号必须被拒
        let mut another = Account::new("acc-2");
        another.owner = None;
        assert_eq!(
            s.upsert_account(Scope::user(&alice), &another)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );

        // 认领之后它才进入 alice 的视野
        assert_eq!(s.claim_unowned(&alice).await.unwrap().accounts, 1);
        assert_eq!(s.list_accounts(Scope::user(&alice)).await.unwrap().len(), 1);
    }

    /// 本地范围下可以覆盖别人的账号（运维语义），用户范围下不行。
    #[tokio::test]
    async fn local_scope_may_rewrite_someone_elses_account() {
        let s = store_with_user().await;
        let alice = uid();
        let bob = UserId::new("bob");
        s.upsert_user(&mcloud_core::User::new(
            "bob",
            "bob",
            "!",
            mcloud_core::Role::User,
            Utc::now(),
        ))
        .await
        .unwrap();

        let mut b = account("acc-b");
        b.owner = Some(bob.clone());
        s.upsert_account(Scope::user(&bob), &b).await.unwrap();

        // 用户范围：被占用 → 拒绝（这条早已有用例，这里把它和本地范围并排钉住）
        let mut stolen = b.clone();
        stolen.owner = Some(alice.clone());
        stolen.nickname = Some("顶掉".into());
        assert_eq!(
            s.upsert_account(Scope::user(&alice), &stolen)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::BadRequest
        );

        // 本地范围：允许改写（调用方本来就拿着库文件）
        let mut refreshed = b.clone();
        refreshed.nickname = Some("运维刷新".into());
        s.upsert_account(Scope::Local, &refreshed).await.unwrap();
        assert_eq!(
            s.get_account(Scope::user(&bob), &AccountId::new("acc-b"))
                .await
                .unwrap()
                .unwrap()
                .nickname
                .as_deref(),
            Some("运维刷新")
        );
    }

    /// 本地范围看得到"匹配不到账号"的历史运行记录（它不属于任何人）。
    #[tokio::test]
    async fn local_scope_lists_orphan_runs() {
        let s = store_with_user().await;
        let run = RunRecord {
            id: "run-orphan".into(),
            account_id: None,
            capability: "sign".into(),
            status: RunStatus::Done,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        };
        s.insert_run(Scope::Local, &run).await.unwrap();

        assert_eq!(s.list_runs(Scope::Local, 10).await.unwrap().len(), 1);
        assert!(
            s.list_runs(Scope::user(&uid()), 10)
                .await
                .unwrap()
                .is_empty(),
            "无归属的运行记录不该冒充某个人的历史"
        );
    }

    #[tokio::test]
    async fn history_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcloud.db");

        {
            let s = SqliteStore::open(&path).unwrap();
            // 用户与账号都要落盘：归属是账号的一部分，重启后必须仍然成立
            s.upsert_user(&mcloud_core::User::new(
                TEST_USER,
                TEST_USER,
                "!",
                mcloud_core::Role::Admin,
                Utc::now(),
            ))
            .await
            .unwrap();
            s.upsert_account(Scope::user(&uid()), &account("acc-1"))
                .await
                .unwrap();
        }

        let s = SqliteStore::open(&path).unwrap();
        let all = s.list_accounts(Scope::user(&uid())).await.unwrap();
        assert_eq!(
            all.len(),
            1,
            "重启后数据必须仍在（原 TS 版运行历史仅存内存）"
        );
    }

    // ── smid 两列 ───────────────────────────────────────────────────────────
    //
    // 这一组的分量比看上去重：`SmidState` 与 `AccountState` 的写入契约**相反**
    // （前者窄写、后者整体覆盖），而这个相反只写在注释和 SQL 里，编译器看不见。
    // 下面几条就是它唯一的执行力来源 —— 尤其 `upsert_account_state_leaves_the_smid_columns_alone`，
    // schema.rs 的 v5 注释直接点名了它。

    /// 探针用的设备特征文本。存储层**从不解析**它（`SmidState` 的"这一层只搬文本"），
    /// 所以随手给一段任意 JSON 就是合法的 —— 这本身也是那条契约的断言之一。
    const FP_V1: &str = r#"{"ua":"UA-v1","canvas":"aaaa1111"}"#;
    const FP_V2: &str = r#"{"ua":"UA-v2","canvas":"bbbb2222"}"#;

    /// 合成 deviceId（`199` 段，见脱敏约定）。
    const DEVICE_ID_1: &str = "B1990000000000001";

    /// 合成整串 `Authorization`（`199` 段手机号，见脱敏约定）。
    ///
    /// 形状与真实值一致：`Basic base64("<前缀>:<手机号>:<token>")`。
    /// 这一列存的是**原文**，所以测试里也照原文比 —— 不做任何解析。
    const AUTH_V1: &str = "Basic MjoxOTkwMDAwMDAwMTphdXRoLXYx";

    /// 另一个合成整串，专用来验证"回填不覆盖已有值"。
    const AUTH_V2: &str = "Basic MjoxOTkwMDAwMDAwMjphdXRoLXYy";

    #[tokio::test]
    async fn smid_state_roundtrips_and_is_absent_before_first_bind() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let id = AccountId::new("acc-1");

        // 没上报过的账号：整行都还不存在，读到的是 None 而不是"空状态"
        let never = s.get_smid_state(Scope::user(&uid()), &id).await.unwrap();
        assert!(never.is_none(), "未绑定不该伪造一个空 SmidState");

        s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
            .await
            .unwrap();
        let got = s
            .get_smid_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("写完应读得到");
        assert_eq!(got.account_id, id);
        assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V1));
        assert_eq!(got.device_id, None, "刚绑定还没有 deviceId");

        s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
            .await
            .unwrap();
        let got = s
            .get_smid_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有状态");
        assert_eq!(
            got.fingerprint_json.as_deref(),
            Some(FP_V1),
            "特征不该被 deviceId 写入碰掉"
        );
        assert_eq!(got.device_id.as_deref(), Some(DEVICE_ID_1));
    }

    /// 换指纹必须连带清掉旧 deviceId —— 旧 id 是**旧长相**在风控那边换来的。
    #[tokio::test]
    async fn rotating_the_fingerprint_clears_the_stale_device_id() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let id = AccountId::new("acc-1");
        s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
            .await
            .unwrap();
        s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
            .await
            .unwrap();

        s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V2)
            .await
            .unwrap();
        let got = s
            .get_smid_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有状态");
        assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V2));
        assert_eq!(
            got.device_id, None,
            "新长相配旧 id 会被风控认成同一台设备，比没有 id 更糟"
        );
    }

    /// 窄写的另一面：`set_smid_*` 不能顺手清掉登录态，也不能刷 `updated_at`。
    ///
    /// `updated_at` 的语义是"登录态快照的时间"。一次指纹上报不是新快照 ——
    /// 把它刷成 now，会让人以为这个账号刚重新登录过。
    #[tokio::test]
    async fn smid_writes_leave_credentials_and_updated_at_alone() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let id = AccountId::new("acc-1");
        let at = Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, 0).unwrap();
        s.upsert_account_state(
            Scope::user(&uid()),
            &AccountState {
                account_id: id.clone(),
                jwt_token: Some("JWT-1".into()),
                device_id: Some("own-install-id-ENDIN".into()),
                user_domain_id: Some("domain-1".into()),
                cloud_phone_redpack_token: None,
                phone: Some("19900000001".into()),
                authorization: Some(AUTH_V1.into()),
                updated_at: Some(at),
            },
        )
        .await
        .unwrap();

        s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
            .await
            .unwrap();
        s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
            .await
            .unwrap();

        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("登录态还在");
        assert_eq!(state.jwt_token.as_deref(), Some("JWT-1"));
        assert_eq!(state.device_id.as_deref(), Some("own-install-id-ENDIN"));
        assert_eq!(state.user_domain_id.as_deref(), Some("domain-1"));
        assert_eq!(
            state.authorization.as_deref(),
            Some(AUTH_V1),
            "整串凭据也得原样留着 —— 它是签到唯一需要的那个头"
        );
        assert_eq!(
            state.updated_at,
            Some(at),
            "smid 写入不该把登录态时间刷成 now"
        );
    }

    /// 回填是**只填空缺**：已有值的列原样保留，哪怕调用方递了一个新值上来。
    ///
    /// 这条是回填能"每次 `migrate` 都跑"的前提。语义写成"有值就覆盖"也不会报错，
    /// 只会在某次迁移时把用户重登后的新凭据换成 `asign.json` 里的旧串 ——
    /// 而那时没有任何线索指向这里。
    #[tokio::test]
    async fn fill_account_credentials_only_fills_the_blanks() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let id = AccountId::new("acc-1");

        // ① 行都不存在时：两列都填上（历史账号可能压根没登录态行）
        s.fill_account_credentials(Scope::user(&uid()), &id, Some(AUTH_V1), Some("19900000001"))
            .await
            .unwrap();
        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("回填应当把行造出来");
        assert_eq!(state.authorization.as_deref(), Some(AUTH_V1));
        assert_eq!(state.phone.as_deref(), Some("19900000001"));
        assert_eq!(state.jwt_token, None, "回填不碰别的列");
        assert_eq!(state.updated_at, None, "补缺口不是新快照");

        // ② 已有值时：递新值上去也必须**不动**它
        s.fill_account_credentials(Scope::user(&uid()), &id, Some(AUTH_V2), Some("19900000002"))
            .await
            .unwrap();
        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有登录态");
        assert_eq!(
            state.authorization.as_deref(),
            Some(AUTH_V1),
            "已有凭据必须原样保留 —— 覆盖它就是把新登录换成旧的"
        );
        assert_eq!(state.phone.as_deref(), Some("19900000001"));

        // ③ `None` = "这一列我没有可填的"，不该把已有值清成 NULL
        s.fill_account_credentials(Scope::user(&uid()), &id, None, None)
            .await
            .unwrap();
        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有登录态");
        assert_eq!(state.authorization.as_deref(), Some(AUTH_V1));
        assert_eq!(state.phone.as_deref(), Some("19900000001"));
    }

    /// 回填的归属检查：不能拿本地范围以外的手去补别人的凭据。
    #[tokio::test]
    async fn fill_account_credentials_requires_an_owned_account() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let alice = uid();
        let bob = UserId::new("someone-else");

        // 别人的账号：拒
        assert_eq!(
            s.fill_account_credentials(
                Scope::user(&bob),
                &AccountId::new("acc-1"),
                Some(AUTH_V1),
                None
            )
            .await
            .unwrap_err()
            .kind(),
            mcloud_core::ErrorKind::NotFound
        );
        // 不存在的账号：也拒（否则就是"给未来那个同名账号预定凭据"）
        assert_eq!(
            s.fill_account_credentials(
                Scope::user(&alice),
                &AccountId::new("no-such"),
                Some(AUTH_V1),
                None
            )
            .await
            .unwrap_err()
            .kind(),
            mcloud_core::ErrorKind::NotFound
        );
    }

    /// **schema.rs 的 v5 注释点名的那条**：整体覆盖的登录态写入必须绕开这两列。
    ///
    /// 现实路径很平常：重新登录 → `upsert_account_state`（新快照，`jwt_token` 之外全是
    /// `None`）。如果这两列被顺手接进那条 UPDATE 的列表里，绑定就白做了 ——
    /// 而登录路径根本不知道它们存在，出事也不会有人怀疑到这里。
    #[tokio::test]
    async fn upsert_account_state_leaves_the_smid_columns_alone() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let id = AccountId::new("acc-1");
        s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
            .await
            .unwrap();
        s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
            .await
            .unwrap();

        // 重新登录：一份只带新 token 的快照，其余字段全是 None
        s.upsert_account_state(
            Scope::user(&uid()),
            &AccountState {
                account_id: id.clone(),
                jwt_token: Some("JWT-2".into()),
                ..AccountState::new("acc-1")
            },
        )
        .await
        .unwrap();

        let smid = s
            .get_smid_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("覆盖登录态之后，smid 两列必须原样还在");
        assert_eq!(smid.fingerprint_json.as_deref(), Some(FP_V1));
        assert_eq!(smid.device_id.as_deref(), Some(DEVICE_ID_1));
        // 顺带确认覆盖本身生效了（否则上一条断言可能只是"没写进去"的假象）
        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有登录态");
        assert_eq!(state.jwt_token.as_deref(), Some("JWT-2"));
        assert_eq!(state.user_domain_id, None, "整体覆盖的语义没被改坏");
    }

    /// **schema.rs 的 v6 注释点名的那条**：`phone` 与 `smid_*` 相反，**必须**跟着登录态走。
    ///
    /// 这条与上一条是同一个坑的两面：把 `phone` 也做成"独立读写"，
    /// 重新登录（换了个手机号）之后库里就会留着**上一个号**，
    /// 而 `Authorization` 会被拼成 `新 token + 旧号码` —— 一个服务端只会回
    /// "未登录"的、看不出根因的组合。
    #[tokio::test]
    async fn upsert_account_state_writes_the_phone() {
        let s = store_with_user().await;
        put_account(&s, "acc-1").await;
        let id = AccountId::new("acc-1");

        // 首次登录：写入完整手机号
        s.upsert_account_state(
            Scope::user(&uid()),
            &AccountState {
                account_id: id.clone(),
                jwt_token: Some("JWT-1".into()),
                phone: Some("19900000001".into()),
                ..AccountState::new("acc-1")
            },
        )
        .await
        .unwrap();
        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有登录态");
        assert_eq!(state.phone.as_deref(), Some("19900000001"));

        // 重新登录换了号：旧号码必须被覆盖掉，不能留下
        s.upsert_account_state(
            Scope::user(&uid()),
            &AccountState {
                account_id: id.clone(),
                jwt_token: Some("JWT-2".into()),
                phone: Some("19900000002".into()),
                ..AccountState::new("acc-1")
            },
        )
        .await
        .unwrap();
        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有登录态");
        assert_eq!(
            state.phone.as_deref(),
            Some("19900000002"),
            "换号后旧号码不许留在库里"
        );

        // 快照里没给手机号 = "上游这次没给"，写成 NULL 而不是保留旧值
        s.upsert_account_state(
            Scope::user(&uid()),
            &AccountState {
                account_id: id.clone(),
                jwt_token: Some("JWT-3".into()),
                ..AccountState::new("acc-1")
            },
        )
        .await
        .unwrap();
        let state = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("有登录态");
        assert_eq!(state.phone, None, "整体覆盖语义：没给就是 NULL");
    }

    /// 手机号要活过重启 —— 它只在登录时写一次，丢了就得重登才能拿回来。
    #[tokio::test]
    async fn account_state_phone_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcloud.db");
        let id = AccountId::new("acc-1");

        {
            let s = SqliteStore::open(&path).unwrap();
            s.upsert_user(&mcloud_core::User::new(
                TEST_USER,
                TEST_USER,
                "!",
                mcloud_core::Role::Admin,
                Utc::now(),
            ))
            .await
            .unwrap();
            s.upsert_account(Scope::user(&uid()), &account("acc-1"))
                .await
                .unwrap();
            s.upsert_account_state(
                Scope::user(&uid()),
                &AccountState {
                    account_id: id.clone(),
                    jwt_token: Some("JWT-1".into()),
                    phone: Some("19900000001".into()),
                    ..AccountState::new("acc-1")
                },
            )
            .await
            .unwrap();
        }

        let s = SqliteStore::open(&path).unwrap();
        let got = s
            .get_account_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("重启后仍有登录态");
        assert_eq!(got.phone.as_deref(), Some("19900000001"));
    }

    /// 两列都要活过重启：绑定是一次性的事，丢了就等于悄悄换了台设备。
    #[tokio::test]
    async fn smid_state_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcloud.db");
        let id = AccountId::new("acc-1");

        {
            let s = SqliteStore::open(&path).unwrap();
            s.upsert_user(&mcloud_core::User::new(
                TEST_USER,
                TEST_USER,
                "!",
                mcloud_core::Role::Admin,
                Utc::now(),
            ))
            .await
            .unwrap();
            s.upsert_account(Scope::user(&uid()), &account("acc-1"))
                .await
                .unwrap();
            s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
                .await
                .unwrap();
            s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
                .await
                .unwrap();
        }

        let s = SqliteStore::open(&path).unwrap();
        let got = s
            .get_smid_state(Scope::user(&uid()), &id)
            .await
            .unwrap()
            .expect("重启后两列都还在");
        assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V1));
        assert_eq!(got.device_id.as_deref(), Some(DEVICE_ID_1));
    }

    /// 没有归属账号就没有 smid 状态：写入被拒，而不是凭空造一行登录态。
    #[tokio::test]
    async fn smid_writes_require_an_owned_account() {
        let s = store_with_user().await;
        let missing = AccountId::new("no-such-account");
        assert_eq!(
            s.set_smid_fingerprint(Scope::user(&uid()), &missing, FP_V1)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound
        );
        assert_eq!(
            s.set_smid_device_id(Scope::user(&uid()), &missing, DEVICE_ID_1)
                .await
                .unwrap_err()
                .kind(),
            mcloud_core::ErrorKind::NotFound,
            "往不存在的账号上挂 deviceId = 给未来那个同名账号预定设备"
        );
        assert!(
            s.get_smid_state(Scope::user(&uid()), &missing)
                .await
                .unwrap()
                .is_none()
        );
    }
}
