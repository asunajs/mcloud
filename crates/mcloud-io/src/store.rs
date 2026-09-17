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
    Account, AccountId, AccountState, CloudStats, Error, RunRecord, RunStatus, StateStore,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

mod config;
mod schema;

pub use config::ImportRecord;

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
}

fn map_sqlite(err: rusqlite::Error) -> Error {
    Error::Internal(format!("SQLite 错误: {err}"))
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

impl StateStore for SqliteStore {
    async fn upsert_account(&self, account: &Account) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.execute(
            r#"
            INSERT INTO accounts (
                id, nickname, phone_masked, platform, enabled, sort_order,
                cloud_current, cloud_updated_at, auth_expire, last_ok_at, fail_streak, note
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
                note             = excluded.note
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
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn get_account(&self, id: &AccountId) -> Result<Option<Account>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.query_row(
            "SELECT id, nickname, phone_masked, platform, enabled, sort_order, \
             cloud_current, cloud_updated_at, auth_expire, last_ok_at, fail_streak, note \
             FROM accounts WHERE id = ?1",
            params![id.as_str()],
            row_to_account,
        )
        .optional()
        .map_err(map_sqlite)
    }

    async fn list_accounts(&self) -> Result<Vec<Account>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(
                "SELECT id, nickname, phone_masked, platform, enabled, sort_order, \
                 cloud_current, cloud_updated_at, auth_expire, last_ok_at, fail_streak, note \
                 FROM accounts ORDER BY sort_order ASC, id ASC",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], row_to_account)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    async fn get_account_state(&self, id: &AccountId) -> Result<Option<AccountState>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.query_row(
            "SELECT account_id, jwt_token, device_id, user_domain_id, \
             cloud_phone_redpack_token, updated_at \
             FROM account_state WHERE account_id = ?1",
            params![id.as_str()],
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
    async fn upsert_account_state(&self, state: &AccountState) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
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

    async fn upsert_cloud_stats(&self, stats: &CloudStats) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
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
        id: &AccountId,
        since: &str,
    ) -> Result<Vec<CloudStats>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(
                "SELECT account_id, date, cloud_count, cloud_diff, cloud_diff_week \
                 FROM cloud_stats WHERE account_id = ?1 AND date >= ?2 ORDER BY date ASC",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![id.as_str(), since], |row| {
                Ok(CloudStats {
                    account_id: AccountId::new(row.get::<_, String>(0)?),
                    date: row.get(1)?,
                    cloud_count: row.get(2)?,
                    cloud_diff: row.get(3)?,
                    cloud_diff_week: row.get(4)?,
                })
            })
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    async fn insert_run(&self, run: &RunRecord) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
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

    async fn update_run(&self, run: &RunRecord) -> Result<(), Error> {
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
                "#,
                params![
                    run.id,
                    run_status_to_str(run.status),
                    run.finished_at.as_ref().map(dt_to_string),
                    run.cloud_gained,
                    run.cloud_after,
                    run.message,
                ],
            )
            .map_err(map_sqlite)?;
        if affected == 0 {
            return Err(Error::Internal(format!("运行记录不存在: {}", run.id)));
        }
        Ok(())
    }

    async fn list_runs(&self, limit: usize) -> Result<Vec<RunRecord>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(
                "SELECT id, account_id, capability, status, started_at, finished_at, \
                 cloud_gained, cloud_after, message \
                 FROM runs ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let status: String = row.get(3)?;
                let started: String = row.get(4)?;
                let finished: Option<String> = row.get(5)?;
                Ok(RunRecord {
                    id: row.get(0)?,
                    account_id: row.get::<_, Option<String>>(1)?.map(AccountId::new),
                    capability: row.get(2)?,
                    status: run_status_from_str(&status),
                    started_at: dt_from_string(started).unwrap_or_else(Utc::now),
                    finished_at: finished.and_then(dt_from_string),
                    cloud_gained: row.get(6)?,
                    cloud_after: row.get(7)?,
                    message: row.get(8)?,
                })
            })
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
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
    use mcloud_core::pure;

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("内存库应可打开")
    }

    fn account(id: &str) -> Account {
        Account {
            nickname: Some("账号2".into()),
            phone_masked: Some("199****0001".into()),
            ..Account::new(id)
        }
    }

    #[test]
    fn schema_version_is_recorded() {
        let s = store();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn account_roundtrip() {
        let s = store();
        let mut acc = account("acc-1");
        acc.cloud_current = Some(1286);
        acc.auth_expire = Some(1_893_456_000_000);
        acc.note = Some("云图书馆阅读月卡".into());

        s.upsert_account(&acc).await.unwrap();

        let loaded = s
            .get_account(&AccountId::new("acc-1"))
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
        let s = store();
        let acc = account("acc-1");
        s.upsert_account(&acc).await.unwrap();

        let mut updated = acc.clone();
        updated.cloud_current = Some(1300);
        updated.fail_streak = 2;
        s.upsert_account(&updated).await.unwrap();

        let all = s.list_accounts().await.unwrap();
        assert_eq!(all.len(), 1, "重复写入不应产生第二条账号");
        assert_eq!(all[0].cloud_current, Some(1300));
        assert_eq!(all[0].fail_streak, 2);
    }

    #[tokio::test]
    async fn accounts_are_ordered_by_sort_order() {
        let s = store();
        let mut a2 = account("acc-2");
        a2.sort_order = 2;
        let mut a1 = account("acc-1");
        a1.sort_order = 1;
        s.upsert_account(&a2).await.unwrap();
        s.upsert_account(&a1).await.unwrap();

        let ids: Vec<String> = s
            .list_accounts()
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.id.to_string())
            .collect();
        assert_eq!(ids, vec!["acc-1", "acc-2"]);
    }

    #[tokio::test]
    async fn cloud_stats_upsert_is_keyed_by_account_and_date() {
        let s = store();
        let id = AccountId::new("acc-1");

        let day1 = pure::next_cloud_stats(&id, "2026-09-15", 1278, &[]);
        s.upsert_cloud_stats(&day1).await.unwrap();

        // 同日重复采集应覆盖而不是新增
        let day1_again = pure::next_cloud_stats(&id, "2026-09-15", 1280, &[]);
        s.upsert_cloud_stats(&day1_again).await.unwrap();

        let stats = s.cloud_stats_since(&id, "2026-09-01").await.unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].cloud_count, 1280);
    }

    #[tokio::test]
    async fn cloud_stats_since_filters_and_orders() {
        let s = store();
        let id = AccountId::new("acc-1");
        let mut history: Vec<CloudStats> = Vec::new();

        for (date, count) in [
            ("2026-08-01", 900),
            ("2026-09-10", 1200),
            ("2026-09-15", 1278),
        ] {
            let stats = pure::next_cloud_stats(&id, date, count, &history);
            s.upsert_cloud_stats(&stats).await.unwrap();
            history.push(stats);
        }

        let stats = s.cloud_stats_since(&id, "2026-09-01").await.unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].date, "2026-09-10");
        assert_eq!(stats[1].date, "2026-09-15");
    }

    #[tokio::test]
    async fn cloud_stats_are_isolated_per_account() {
        let s = store();
        let a = AccountId::new("acc-1");
        let b = AccountId::new("acc-2");
        s.upsert_cloud_stats(&pure::next_cloud_stats(&a, "2026-09-15", 100, &[]))
            .await
            .unwrap();
        s.upsert_cloud_stats(&pure::next_cloud_stats(&b, "2026-09-15", 200, &[]))
            .await
            .unwrap();

        assert_eq!(
            s.cloud_stats_since(&a, "2026-01-01").await.unwrap()[0].cloud_count,
            100
        );
        assert_eq!(
            s.cloud_stats_since(&b, "2026-01-01").await.unwrap()[0].cloud_count,
            200
        );
    }

    #[tokio::test]
    async fn run_lifecycle_insert_then_update() {
        let s = store();
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
        s.insert_run(&run).await.unwrap();

        run.status = RunStatus::Done;
        run.finished_at = Some(Utc::now());
        run.cloud_gained = Some(8);
        run.cloud_after = Some(1286);
        run.message = Some("签到成功".into());
        s.update_run(&run).await.unwrap();

        let runs = s.list_runs(10).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Done);
        assert_eq!(runs[0].cloud_gained, Some(8));
        assert_eq!(runs[0].cloud_after, Some(1286));
        assert!(runs[0].finished_at.is_some());
    }

    #[tokio::test]
    async fn duplicate_run_id_does_not_create_a_second_row() {
        let s = store();
        let run = RunRecord {
            id: "run-1".into(),
            account_id: None,
            capability: "sign".into(),
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        };
        s.insert_run(&run).await.unwrap();
        s.insert_run(&run).await.unwrap();
        assert_eq!(s.list_runs(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn updating_a_missing_run_is_an_error() {
        let s = store();
        let run = RunRecord {
            id: "does-not-exist".into(),
            account_id: None,
            capability: "sign".into(),
            status: RunStatus::Failed,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        };
        assert!(s.update_run(&run).await.is_err());
    }

    #[tokio::test]
    async fn history_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcloud.db");

        {
            let s = SqliteStore::open(&path).unwrap();
            s.upsert_account(&account("acc-1")).await.unwrap();
        }

        let s = SqliteStore::open(&path).unwrap();
        let all = s.list_accounts().await.unwrap();
        assert_eq!(
            all.len(),
            1,
            "重启后数据必须仍在（原 TS 版运行历史仅存内存）"
        );
    }
}
