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
    Account, AccountId, AccountState, CloudStats, Error, RunRecord, RunStatus, Scope, StateStore,
    UserId,
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
             s.cloud_phone_redpack_token, s.updated_at \
             FROM account_state s JOIN accounts a ON a.id = s.account_id \
             WHERE s.account_id = ?1 AND (?2 IS NULL OR a.owner = ?2)",
            params![id.as_str(), scope.owner().map(UserId::as_str)],
            |row| {
                let updated_at: Option<String> = row.get(5)?;
                Ok(AccountState {
                    account_id: AccountId::new(row.get::<_, String>(0)?),
                    jwt_token: row.get(1)?,
                    device_id: row.get(2)?,
                    user_domain_id: row.get(3)?,
                    cloud_phone_redpack_token: row.get(4)?,
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
                cloud_phone_redpack_token, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(account_id) DO UPDATE SET
                jwt_token                 = excluded.jwt_token,
                device_id                 = excluded.device_id,
                user_domain_id            = excluded.user_domain_id,
                cloud_phone_redpack_token = excluded.cloud_phone_redpack_token,
                updated_at                = excluded.updated_at
            "#,
            params![
                state.account_id.as_str(),
                state.jwt_token,
                state.device_id,
                state.user_domain_id,
                state.cloud_phone_redpack_token,
                state.updated_at.as_ref().map(dt_to_string),
            ],
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

    /// 认领未归属账号（`owner IS NULL`）。实现见 `store/user.rs` 的理由说明，
    /// 放在这里是因为该契约属于 `StateStore`，而**同一个 trait 不能拆成两个 impl 块**。
    async fn claim_unowned_accounts(&self, user_id: &mcloud_core::UserId) -> Result<u64, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let affected = conn
            .execute(
                "UPDATE accounts SET owner = ?1 WHERE owner IS NULL",
                params![user_id.as_str()],
            )
            .map_err(map_sqlite)?;
        Ok(affected as u64)
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
        assert_eq!(s.claim_unowned_accounts(&alice).await.unwrap(), 1);
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
}
