//! 数据库 schema 与迁移。
//!
//! 迁移用 `PRAGMA user_version` 做版本号，**纯 SQL、可幂等重跑**。
//!
//! 对应方案里的门禁："`data/*.json` → SQLite 的导入函数有幂等测试"
//! 与"迁移文件清单断言" —— 参考项目 caiyun 用 `expectedMigrationFiles` 做同样的事。

use mcloud_core::Error;
use rusqlite::Connection;

/// 当前 schema 版本。
///
/// **新增迁移时必须同步递增**，并且 `ALL_MIGRATIONS` 的长度要与它一致
/// （这条由 `migrations_are_sequential` 单测把关）。
///
/// # 什么时候可以改老迁移，什么时候只能追加
///
/// 本项目**尚未发布**，任何一个已应用的迁移理论上都可以直接改（改完把版本号退回去）。
/// 但一旦有了外部用户的库文件，"已应用的迁移"就是历史，只能往后追加 ——
/// 否则老库的 `user_version` 会说谎：它声称已到 v2，而 v2 的语句已经不是你写的那句了。
/// 判断标准很简单：**有没有可能存在一个不是本机跑出来的库**。没有，就可以改；有，就只能加。
pub const SCHEMA_VERSION: i64 = 2;

/// 迁移清单：索引 i（0-based）对应版本 i+1。
///
/// 每个元素必须是幂等 SQL（用 `IF NOT EXISTS`），
/// 这样重复执行不会炸，也让"重跑迁移"成为安全操作。
pub const ALL_MIGRATIONS: &[&str] = &[
    // v1：账号 + 云朵日快照 + 运行记录
    r#"
    CREATE TABLE IF NOT EXISTS accounts (
        id               TEXT PRIMARY KEY,
        nickname         TEXT,
        phone_masked     TEXT,
        platform         TEXT,
        enabled          INTEGER NOT NULL DEFAULT 1,
        sort_order       INTEGER NOT NULL DEFAULT 0,
        cloud_current    INTEGER,
        cloud_updated_at TEXT,
        auth_expire      INTEGER,
        last_ok_at       TEXT,
        fail_streak      INTEGER NOT NULL DEFAULT 0,
        note             TEXT
    );

    -- 云朵日快照：唯一键 account + date，UPSERT 语义
    CREATE TABLE IF NOT EXISTS cloud_stats (
        account_id      TEXT NOT NULL,
        date            TEXT NOT NULL,
        cloud_count     INTEGER NOT NULL,
        cloud_diff      INTEGER NOT NULL,
        cloud_diff_week INTEGER NOT NULL,
        PRIMARY KEY (account_id, date)
    );
    CREATE INDEX IF NOT EXISTS idx_cloud_stats_date ON cloud_stats (date);

    -- 运行记录：原 TS 版仅存内存（api/src/runner.ts 的 AsyncRunState），重启即丢
    CREATE TABLE IF NOT EXISTS runs (
        id           TEXT PRIMARY KEY,
        account_id   TEXT,
        capability   TEXT NOT NULL,
        status       TEXT NOT NULL,
        started_at   TEXT NOT NULL,
        finished_at  TEXT,
        cloud_gained INTEGER,
        cloud_after  INTEGER,
        message      TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_runs_started   ON runs (started_at DESC);
    CREATE INDEX IF NOT EXISTS idx_runs_account   ON runs (account_id, started_at DESC);
    CREATE INDEX IF NOT EXISTS idx_runs_capability ON runs (capability, started_at DESC);
    "#,
    // v2：P2 的持久化实体 + 旧数据导入台账
    r#"
    -- 账号登录态（旧 data/state.json）。
    --
    -- **刻意与 accounts 分表**：这张表装的是可直接复用的凭据（jwtToken 能直接冒充登录用户），
    -- 而 accounts 装的是能直接吐给前端的展示信息。分表之后，
    -- "列出账号"这条最常走的查询天然拿不到凭据，不依赖任何人的自觉。
    CREATE TABLE IF NOT EXISTS account_state (
        account_id                TEXT PRIMARY KEY,
        jwt_token                 TEXT,
        device_id                 TEXT,
        user_domain_id            TEXT,
        cloud_phone_redpack_token TEXT,
        updated_at                TEXT
    );

    -- 定时任务（旧 data/schedules.json）。JSON 字段统一存 TEXT：
    -- 这些结构是"整块读写"的，拆成列只会让 schema 跟着能力参数变化而改。
    CREATE TABLE IF NOT EXISTS schedules (
        id           TEXT PRIMARY KEY,
        name         TEXT NOT NULL,
        -- {"kind":"capability","code":"sign"} 或 {"kind":"pipeline","id":"..."}；
        -- NULL = 目标缺失（旧文件里可能存在，导入时会作为问题报出）
        target       TEXT,
        cron         TEXT NOT NULL,
        timezone     TEXT NOT NULL,
        -- AccountId 的 JSON 数组。**空数组 = 全部账号**，不是"没有账号"
        account_ids  TEXT NOT NULL DEFAULT '[]',
        body         TEXT,
        enabled      INTEGER NOT NULL DEFAULT 1,
        created_at   TEXT,
        last_run_at  TEXT,
        last_status  TEXT,
        last_error   TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_schedules_enabled ON schedules (enabled);

    -- 任务组合（旧 data/pipelines.json）
    CREATE TABLE IF NOT EXISTS pipelines (
        id               TEXT PRIMARY KEY,
        name             TEXT NOT NULL,
        description      TEXT,
        steps            TEXT NOT NULL DEFAULT '[]',
        account_ids      TEXT NOT NULL DEFAULT '[]',
        account_mode     TEXT NOT NULL DEFAULT 'sequential',
        stop_on_error    INTEGER NOT NULL DEFAULT 1,
        created_at       TEXT,
        updated_at       TEXT,
        last_run_at      TEXT,
        last_status      TEXT,
        last_error       TEXT,
        last_duration_ms INTEGER
    );

    -- 定时抢兑（旧 data/exchange.json）：全局唯一一份，用 CHECK 钉成单行表，
    -- 免得哪天多插一行之后"读出来的是哪一行"变成运气问题
    CREATE TABLE IF NOT EXISTS exchange_config (
        id           INTEGER PRIMARY KEY CHECK (id = 1),
        enabled      INTEGER NOT NULL DEFAULT 0,
        windows      TEXT NOT NULL DEFAULT '[]',
        bindings     TEXT NOT NULL DEFAULT '[]',
        schedule_ids TEXT NOT NULL DEFAULT '[]',
        updated_at   TEXT
    );

    -- 界面偏好（旧 data/webui.json）：同样单行
    CREATE TABLE IF NOT EXISTS webui_config (
        id                 INTEGER PRIMARY KEY CHECK (id = 1),
        remember_last_page INTEGER NOT NULL DEFAULT 1,
        last_page          TEXT NOT NULL DEFAULT 'tasks',
        log_expanded       INTEGER NOT NULL DEFAULT 0,
        background_run     INTEGER NOT NULL DEFAULT 0,
        updated_at         TEXT
    );

    -- 旧数据导入台账。一行 = 一个源文件。
    --
    -- 三重作用：
    --   1. `sha256` 判断"源文件变过没有" → 幂等重跑的依据；
    --   2. `raw` 保存**原始 JSON 全文** → 映射规则将来改了还能重放，
    --      而不用去求用户把旧目录找回来（这条也替代了方案里原本的 `.bak` 文件：
    --      归档在库里，搬走/备份数据库时不会漏掉）；
    --   3. `issues` 记录这次导入报了几个问题 → 便于"我上次到底看没看过那份报告"。
    CREATE TABLE IF NOT EXISTS imports (
        source       TEXT PRIMARY KEY,
        sha256       TEXT NOT NULL,
        imported_at  TEXT NOT NULL,
        record_count INTEGER NOT NULL DEFAULT 0,
        issues       INTEGER NOT NULL DEFAULT 0,
        raw          TEXT
    );
    "#,
];

/// 执行未应用的迁移。
pub fn migrate(conn: &Connection) -> Result<(), Error> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| Error::Internal(format!("读取 schema 版本失败: {e}")))?;

    if current > SCHEMA_VERSION {
        return Err(Error::Internal(format!(
            "数据库 schema 版本 {current} 高于本程序支持的 {SCHEMA_VERSION}，请升级程序"
        )));
    }

    for (index, sql) in ALL_MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        conn.execute_batch(sql)
            .map_err(|e| Error::Internal(format!("应用迁移 v{version} 失败: {e}")))?;
        conn.pragma_update(None, "user_version", version)
            .map_err(|e| Error::Internal(format!("写入 schema 版本 v{version} 失败: {e}")))?;
        tracing::info!(version, "已应用数据库迁移");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_sequential() {
        assert_eq!(
            ALL_MIGRATIONS.len() as i64,
            SCHEMA_VERSION,
            "迁移数量必须与 SCHEMA_VERSION 一致，否则版本号会撒谎"
        );
    }

    /// 每个迁移里的**每条** `CREATE` 都必须带 `IF NOT EXISTS`。
    ///
    /// 只检查"整段字符串里出现过一次"是不够的：那样只要有一条写了，同段里其它几条
    /// 用裸 `CREATE TABLE` 也能蒙过去，而重跑时正是那几条会炸。
    #[test]
    fn migrations_are_idempotent_sql() {
        for (index, sql) in ALL_MIGRATIONS.iter().enumerate() {
            let statements: Vec<String> = sql
                .lines()
                .filter(|line| !line.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n")
                .split(';')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();

            assert!(
                !statements.is_empty(),
                "迁移 v{} 里没解析出任何语句，说明拆分逻辑坏了",
                index + 1
            );

            for statement in &statements {
                if statement.starts_with("create ") {
                    assert!(
                        statement.contains("if not exists"),
                        "迁移 v{} 里的这条语句缺少 IF NOT EXISTS，重跑会炸：\n{}",
                        index + 1,
                        statement
                    );
                }
            }
        }
    }

    #[test]
    fn fresh_database_reaches_current_version() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn migration_is_safe_to_rerun() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn future_version_is_refused_instead_of_corrupting() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 5)
            .unwrap();
        let err = migrate(&conn).expect_err("未来版本必须拒绝，不能硬写下去");
        assert!(err.to_string().contains("请升级程序"));
    }

    #[test]
    fn expected_tables_exist_after_migration() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        for table in [
            // v1
            "accounts",
            "cloud_stats",
            "runs",
            // v2
            "account_state",
            "schedules",
            "pipelines",
            "exchange_config",
            "webui_config",
            "imports",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "缺少表 {table}");
        }
    }

    /// 单行配置表必须真的只能有一行 —— `CHECK (id = 1)` 是这条断言的依据。
    #[test]
    fn single_row_config_tables_reject_a_second_row() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        for table in ["exchange_config", "webui_config"] {
            conn.execute(&format!("INSERT INTO {table} (id) VALUES (1)"), [])
                .unwrap_or_else(|e| panic!("{table} 第一行应能插入: {e}"));
            assert!(
                conn.execute(&format!("INSERT INTO {table} (id) VALUES (2)"), [])
                    .is_err(),
                "{table} 不应允许第二行"
            );
        }
    }

    /// 老版本升到新版本时，已存在的表不能被重建或清空。
    #[test]
    fn upgrading_keeps_existing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        // 只应用 v1，模拟"老库"
        conn.execute_batch(ALL_MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, nickname, enabled, sort_order, fail_streak) \
             VALUES ('19****01', '账号2', 1, 0, 0)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let nickname: String = conn
            .query_row(
                "SELECT nickname FROM accounts WHERE id = '19****01'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nickname, "账号2", "升级不该动老数据");
    }
}
