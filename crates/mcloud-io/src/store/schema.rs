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
pub const SCHEMA_VERSION: i64 = 8;

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
    // v3：系统用户 + 会话 + 账号归属（方案 §3.7）
    //
    // # 为什么这一段外面包了显式事务
    //
    // `ALTER TABLE ... ADD COLUMN` 是**唯一**一条无法写成 `IF NOT EXISTS` 的语句
    // （SQLite 不支持）。本框架的"幂等"其实有两层保障：
    //   1. `user_version` 门控 —— 已应用的迁移不会再跑，所以 ALTER 事实上只执行一次；
    //   2. 包事务 —— 万一这条 ALTER 之后的语句失败，不包事务就会留下
    //      "列已加、版本没升"的半成品库，而重跑时那条 ALTER 会报"duplicate column name"，
    //      用户从此进不了程序。包上事务后失败即整体回滚，重跑是干净的。
    // 注意：这一段里 `CREATE` 仍然全部带 `IF NOT EXISTS`（由单测逐条把关），
    // 事务只是把"半成品"这个状态消掉。
    r#"
    BEGIN;

    -- 系统用户：登录**本系统**的账号，与云盘账号是两回事（见 §3.7 的三概念表）
    CREATE TABLE IF NOT EXISTS users (
        id            TEXT PRIMARY KEY,
        -- 大小写不敏感唯一：否则 Alice 与 alice 能各建一个，
        -- 而登录时"匹配到谁"取决于查询顺序 —— 这种不确定性在鉴权路径上不可接受
        name          TEXT NOT NULL COLLATE NOCASE UNIQUE,
        -- argon2id 的 PHC 编码串（`$argon2id$v=19$m=...$salt$hash`）：
        -- 它自带盐与参数，所以**不需要单独的 salt 列**，换参数也不用改 schema
        password_hash TEXT NOT NULL,
        role          TEXT NOT NULL DEFAULT 'user',
        created_at    TEXT NOT NULL
    );

    -- 会话：只存 token 的哈希，明文只在签发那一刻返回一次。
    -- 拿到库文件也无法直接冒充登录，与口令存哈希是同一个理由。
    CREATE TABLE IF NOT EXISTS sessions (
        token_hash TEXT PRIMARY KEY,
        user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        expires_at TEXT NOT NULL,
        kind       TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_sessions_user    ON sessions (user_id);
    CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions (expires_at);

    -- 账号归属。历史数据（v2 及以前）为 NULL = 未归属，
    -- 由 `mcloud user init` 初始化首个管理员时认领（见 StateStore::claim_unowned_accounts）。
    -- 迁移期无法给 owner 赋值：那时管理员还不存在，硬编一个只能是猜。
    ALTER TABLE accounts ADD COLUMN owner TEXT REFERENCES users(id);

    COMMIT;
    "#,
    // v4：定时任务与组合也归属系统用户（§3.7「所有数据都要有主人」的收口）
    //
    // v3 只覆盖了账号；`schedules` / `pipelines` 当时仍是全局表。不补的后果不是"少个字段"，
    // 而是这两类数据**没有越权判据可加**：等 P5 的调度引擎与 WebUI 上来，
    // 一个用户就能看见并触发别人的定时任务与组合。
    //
    // 与 v3 一样必须包事务，理由同 v3 的注释（ALTER 无法 IF NOT EXISTS）。
    // 注意这里比 v3 **更危险**：v3 那一段至少还有"重跑时 CREATE 带 IF NOT EXISTS"
    // 这层缓冲，而这一段只有两条 ALTER —— 事务是唯一的保障。
    r#"
    BEGIN;

    -- 历史数据（v3 及以前）为 NULL = 未归属，与账号同一套语义：
    -- **谁都看不见**（漏判的代价不对称，见 model.rs 的 is_visible_to），
    -- 由 `mcloud user init` 初始化首个管理员时认领。
    ALTER TABLE schedules ADD COLUMN owner TEXT REFERENCES users(id);
    ALTER TABLE pipelines ADD COLUMN owner TEXT REFERENCES users(id);

    COMMIT;
    "#,
    // v5：smid 设备指纹的两份状态（第三方风控服务发下来的那个 `deviceid` 头）
    //
    // # 为什么放这张表，却又**不属于** [`AccountState`]
    //
    // 它们确实是"这个账号在这台设备上的状态"，与 `account_state` 是同一主体；
    // 但那张表的写入契约是**整体覆盖**（`upsert_account_state`，见 v2 注释与 model.rs），
    // 而这两列的写入契约正相反：**换登录凭据不该动它，换指纹只动它**。
    // 所以列放在一起（同一个主体、同一套归属校验），而**读写走各自独立的方法**，
    // 并由 `upsert_account_state_leaves_the_smid_columns_alone` 那条单测把关。
    //
    // 语义（三条生命周期不同，别混）：
    //   - `smid_fingerprint`：绑定账号的那份设备特征，JSON 文本。**只在主动更换时变**；
    //   - `smid_device_id`：最近一次上报换回的 `deviceId`（含 `B` 前缀）。服务端发的，会变，
    //     只用作下一次上报的 `box` 字段；换指纹时必须清空（旧 id 属于旧长相）。
    //
    // 与 v3/v4 同理：ALTER 不能 `IF NOT EXISTS`，必须包事务。
    r#"
    BEGIN;

    ALTER TABLE account_state ADD COLUMN smid_fingerprint TEXT;
    ALTER TABLE account_state ADD COLUMN smid_device_id TEXT;

    COMMIT;
    "#,
    // v6：`account_state.phone` —— **完整手机号**，补上"拼不出 `Authorization`"那个缺口
    //
    // # 为什么必须存
    //
    // `Authorization` 的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`，
    // 中间那段是完整手机号，而它**推不出来**：
    // `accounts.id` 是掩码、`jwt_token` 里没有它、`user_domain_id` 是另一个东西。
    // 不存的结果就是每个要用这个头的能力都只能让调用方在命令行上重输一遍手机号
    // （`mcloud signin --phone`）—— 那是缺口不是设计。
    //
    // # 为什么放 `account_state` 而不是 `accounts`
    //
    // `accounts` 是**直接吐给前端**的展示表，那里面只能有掩码（`phone_masked`）。
    // `account_state` 里已经有同类的"标识而非凭据"（`user_domain_id`），
    // 且"列出账号"这条最常走的查询天然不碰这张表。
    //
    // # 与 `smid_*` 两列的差别（别把 v6 也做成"独立读写"）
    //
    // `smid_*` 之所以要独立读写，是因为它们的生命周期与登录态**相反**；
    // 而 `phone` 恰恰**跟着登录态走**：它来自这次登录的 `auth` 载荷，
    // 换登录就该跟着换。所以它**接进** `upsert_account_state` 的整体覆盖列表，
    // 不另开方法 —— 这条由 `store.rs` 的 `upsert_account_state_writes_the_phone` 把关。
    //
    // # 历史行是 NULL
    //
    // v6 之前登录的账号这一列是 `NULL`：那是**如实记录"当时没存"**，
    // 不是"没有手机号"。不要拿掩码去回填 —— 掩码拼出来的头服务端认不认是未知数，
    // 而这类"能过但不该过"的值一旦过了一次，就会变成没人敢动的隐性依赖。
    // 补法只有一种：**重登一次**。
    //
    // 与前几版同理：ALTER 不能 `IF NOT EXISTS`，必须包事务。
    r#"
    BEGIN;

    ALTER TABLE account_state ADD COLUMN phone TEXT;

    COMMIT;
    "#,
    // ── v7：`account_state.authorization` ────────────────────────────────────
    //
    // 原样存下整个 `Authorization` 头值（`Basic …`）。
    //
    // # 为什么是"加一列"而不是"继续用 v6 的 phone 拼"
    //
    // v6 的思路是"把缺的那一段（手机号）补上，拼的时候就不缺料了"。
    // 那是**绕远路**，而且有三处具体的代价：
    //
    // 1. **旧数据搬不过来**。旧项目 `mcloud-sign` 的 `asign.json` 里，
    //    `auth` 存的**就是这个整串**（`Basic base64("mobile:<手机号>:<authToken>")`），
    //    它一直用得好好的。我们重写时把它解码、拆成三段、只留 token，
    //    再把手机号单存一列 —— 于是"旧库里有的东西，新库里凑不齐"。
    // 2. **前缀要靠猜**。`Basic` 后面第一段是 `mobile`（App）还是 `pc`（PC），
    //    拆开之后就只剩"调用点硬编码"。猜错不会报错，只会发一个服务端不认的头，
    //    表现成"未登录"（`90001`）—— 指向凭据坏了，而不是指向拼错了。
    //    整串存则前缀本来就在里面。
    // 3. **多一处能失败的环节**。三段缺一就用不了；整串只有一个"有没有"。
    //
    // # 与 `phone` 的关系
    //
    // 两者**冗余但不同源**，各有各的用途，都留着：
    //   - `authorization`：给上游用的凭据，读取侧**优先**用它；
    //   - `phone`：给人看的标识（日志里认出"签的是谁"），掩码不够用。
    //
    // 写入契约与 `phone` 完全一致：**跟着登录态走**，接进
    // `upsert_account_state` 的整体覆盖列表，不另开窄写方法。
    //
    // # 历史行是 NULL
    //
    // v7 之前登录的账号这一列是 `NULL`，读取侧会**退回**"用 `phone` + `jwt_token`
    // 现拼"那条老路（`SignInCredential::load` 里写明了退路与它的代价）。
    // 两条路并存是有意的：新行走新路，老行不必为了这一列重登一次。
    //
    // 与前几版同理：ALTER 不能 `IF NOT EXISTS`，必须包事务。
    r#"
    BEGIN;

    ALTER TABLE account_state ADD COLUMN authorization TEXT;

    COMMIT;
    "#,
    // ── v8：`run_events` —— 运行事件流落库 ───────────────────────────────────
    //
    // # 为什么必须落库（而不是只在内存里广播）
    //
    // 旧项目的事件只活在内存（`api/src/runner.ts` 的 `AsyncRunState`），实测有三个洞：
    //
    // 1. **进程重启即丢**。用户第二天打开页面看"昨晚那次签到到底怎么了" —— 没有了。
    //    只剩 `runs` 表里一行 `Done`，谁也不知道那 20 个账号里哪一个失败了。
    // 2. **晚连的人看不到前面**。前端在 run 开始 3 秒后才打开页面，
    //    于是这 3 秒的日志永远拿不到 —— 而那几条恰恰是最关键的"在干什么"。
    // 3. **续传没有锚点**。断了要重连时，只能"整个重放"或"接下来什么都不给"。
    //    前者重复、后者丢事件，两者都会让前端显示一个错的进度。
    //
    // `seq` 就是给第 3 条用的：客户端把最后收到的 `seq` 带回来
    // （`GET /api/runs/:id/events?after=<seq>`），服务端从 `seq + 1` 接着发。
    // 这也是为什么 `(run_id, seq)` 必须**唯一** —— 序号一旦重复，
    // "继续发"就会变成"永远跳过某条"，而那是静默的、没人会发现。
    //
    // # 为什么事件体存 JSON 而不是拆列
    //
    // 五类事件的字段差别很大（`Progress` 有 done/total，`Finished` 有 outcome/
    // manual_steps/summary，`Log` 有 level/message）。拆成宽表的话大部分列对大部分行是
    // NULL，而且每加一个事件类型都要动 schema —— 而这恰恰是最容易变的东西。
    // 拆列的唯一好处是"能按 message 内容做 SQL 查询"，但我们不需要：
    // 查事件永远是"某个 run 的全部事件，按 seq 排"，用不到 JSON 内部的字段。
    //
    // # 为什么不用 `REFERENCES runs(id)` 的外键
    //
    // 会。删 run 时事件没清 → 库越用越大；而 SQLite 的外键默认**不生效**
    // （要 `PRAGMA foreign_keys=ON`），所以写了也是假的。这里改用索引 + 显式清理：
    // 删 run 时必须同时删它的事件（`delete_run_events`），由 store 层的测试把关。
    //
    // # 保留策略
    //
    // 事件量按"一个 run 几百条"估，不做自动清理 —— 但 `delete_run_events` 要有，
    // 否则将来想清也没有称手的工具。真要限制总量时加一条
    // `DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE started_at < ?)`
    // 即可，不必改 schema。
    r#"
    -- run_id 不设外键（理由见上）。seq 由发布侧（EventBus）统一分配，从 1 起。
    CREATE TABLE IF NOT EXISTS run_events (
        run_id     TEXT NOT NULL,
        seq        INTEGER NOT NULL,
        kind       TEXT NOT NULL,
        payload    TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (run_id, seq)
    );

    -- 主键已经把 (run_id, seq) 做成唯一，且天然支持
    -- "某个 run 按 seq 序取出"（最常走的查询）；这条索引给"按时间查最近的事件"用。
    CREATE INDEX IF NOT EXISTS idx_run_events_created ON run_events (created_at DESC);
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
            // v3
            "users",
            "sessions",
            // v8
            "run_events",
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

    /// v4 比 v3 更依赖事务：这一段**只有两条 ALTER、没有任何 CREATE 兜底**。
    #[test]
    fn v4_migration_is_wrapped_in_a_transaction() {
        let v4 = ALL_MIGRATIONS[3].to_lowercase();
        assert!(v4.contains("begin;"), "v4 必须在显式事务里执行");
        assert!(v4.contains("commit;"), "v4 必须有配套的 COMMIT");
        assert!(
            v4.contains("alter table schedules add column owner"),
            "v4 要给 schedules 加 owner"
        );
        assert!(
            v4.contains("alter table pipelines add column owner"),
            "v4 要给 pipelines 加 owner"
        );
        assert!(
            !v4.contains("create "),
            "v4 只该是两条 ALTER + 事务包装；建新表属于另一版"
        );
    }

    /// v5 给 `account_state` 加两份 smid 状态，同样是"两条 ALTER + 事务"。
    ///
    /// 断言"只加这两列"是有理由的：这两列的写入契约与这张表原有的**整体覆盖**相反
    /// （见上面的 v5 注释），所以它们绝不能被顺手接进 `upsert_account_state` 的列表里 ——
    /// 那条由 `store.rs` 的 `upsert_account_state_leaves_the_smid_columns_alone` 把关。
    #[test]
    fn v5_migration_adds_the_smid_columns_in_a_transaction() {
        let v5 = ALL_MIGRATIONS[4].to_lowercase();
        assert!(v5.contains("begin;"), "v5 必须在显式事务里执行");
        assert!(v5.contains("commit;"), "v5 必须有配套的 COMMIT");
        assert!(
            v5.contains("alter table account_state add column smid_fingerprint"),
            "v5 要加绑定的设备特征列"
        );
        assert!(
            v5.contains("alter table account_state add column smid_device_id"),
            "v5 要加最近拿到的 deviceId 列"
        );
        assert!(
            !v5.contains("create "),
            "v5 只该是两条 ALTER + 事务包装；建新表属于另一版"
        );
    }

    /// v4 老库升到 v5：已有的登录态一行都不能被动过。
    #[test]
    fn v4_database_gains_smid_columns_and_keeps_credentials() {
        let conn = Connection::open_in_memory().unwrap();
        // 模拟 v4 老库：v1..v4 手工应用，v5 交给 migrate
        for sql in &ALL_MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, nickname, enabled, sort_order, fail_streak) \
             VALUES ('19****01', '账号2', 1, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO account_state (account_id, jwt_token, device_id, updated_at) \
             VALUES ('19****01', 'OLD-TOKEN', 'OLD-ENDIN', '2026-09-18T00:00:00Z')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(account_state)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            columns.iter().any(|c| c == "smid_fingerprint"),
            "{columns:?}"
        );
        assert!(columns.iter().any(|c| c == "smid_device_id"), "{columns:?}");

        // 新列对老行是 NULL：绑定的特征**只能由用户主动生成**，迁移期凭空造一份
        // 等于替每台老库发明一台设备（与 v3 不给 owner 硬编值是同一条理由）
        let (jwt, device, fp, smid): (String, String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT jwt_token, device_id, smid_fingerprint, smid_device_id \
                 FROM account_state WHERE account_id = '19****01'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(jwt, "OLD-TOKEN", "升级不该动凭据");
        assert_eq!(device, "OLD-ENDIN");
        assert_eq!(fp, None);
        assert_eq!(smid, None);
    }

    /// v6 只加一列 `phone`，且必须包事务（ALTER 无法 `IF NOT EXISTS`）。
    #[test]
    fn v6_migration_adds_the_phone_column_in_a_transaction() {
        let v6 = ALL_MIGRATIONS[5].to_lowercase();
        assert!(v6.contains("begin;"), "v6 必须在显式事务里执行");
        assert!(v6.contains("commit;"), "v6 必须有配套的 COMMIT");
        assert!(
            v6.contains("alter table account_state add column phone"),
            "v6 要加完整手机号列"
        );
        assert!(
            !v6.contains("create "),
            "v6 只该是一条 ALTER + 事务包装；建新表属于另一版"
        );
    }

    /// v5 老库升到 v6：`phone` 对老行是 `NULL`，且**不能**从掩码回填。
    ///
    /// 断言"是 NULL"而不是"等于某个值"是有意的：`NULL` = 如实记录"当时没存"，
    /// 拿 `phone_masked`（`19****01`）去填会让 `Authorization` 拼出一个
    /// **服务端认不认未知**的头 —— 那正是本项目反复踩的"能过但不该过"。
    #[test]
    fn v5_database_gains_the_phone_column_without_backfilling_the_mask() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &ALL_MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, nickname, phone_masked, enabled, sort_order, fail_streak) \
             VALUES ('19****01', '账号2', '19****01', 1, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO account_state (account_id, jwt_token, device_id, updated_at) \
             VALUES ('19****01', 'OLD-TOKEN', 'OLD-ENDIN', '2026-09-18T00:00:00Z')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let (jwt, phone): (String, Option<String>) = conn
            .query_row(
                "SELECT jwt_token, phone FROM account_state WHERE account_id = '19****01'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(jwt, "OLD-TOKEN", "升级不该动凭据");
        assert_eq!(phone, None, "老行的 phone 必须是 NULL，不许拿掩码回填");
    }

    /// `ALTER TABLE ... ADD COLUMN` 是唯一一条无法幂等的语句，验证它被包在事务里。
    ///
    /// 不包事务的后果很具体：ALTER 之后的语句失败 → 列已加、版本未升 →
    /// 重跑时这条 ALTER 报 "duplicate column name" → 用户从此进不了程序。
    #[test]
    fn v3_migration_is_wrapped_in_a_transaction() {
        let v3 = ALL_MIGRATIONS[2].to_lowercase();
        assert!(v3.contains("begin;"), "v3 必须在显式事务里执行");
        assert!(v3.contains("commit;"), "v3 必须有配套的 COMMIT");
        assert!(
            v3.contains("alter table accounts add column owner"),
            "v3 的核心动作是给 accounts 加 owner"
        );
    }

    /// v2 老库升到 v3：拿到 `owner` 列，且老数据一条不少。
    #[test]
    fn v2_database_gains_owner_column_and_keeps_rows() {
        let conn = Connection::open_in_memory().unwrap();
        // 模拟 v2 老库：只应用 v1 + v2（v3 交给 migrate）
        conn.execute_batch(ALL_MIGRATIONS[0]).unwrap();
        conn.execute_batch(ALL_MIGRATIONS[1]).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, nickname, enabled, sort_order, fail_streak) \
             VALUES ('19****01', '账号2', 1, 0, 0)",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let owner: Option<String> = conn
            .query_row(
                "SELECT owner FROM accounts WHERE id = '19****01'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            owner, None,
            "迁移期不能给历史账号硬编 owner —— 那时首个管理员还不存在，只能落到未归属"
        );

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(accounts)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(columns.iter().any(|c| c == "owner"), "缺列: {columns:?}");
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

    /// v8 是**建表**而非加列，所以断言的重点与 v3…v7 不同。
    ///
    /// v3…v7 的形态都是 `ALTER TABLE ... ADD COLUMN`，它们的风险是
    /// "库改一半、版本没升"，所以要盯事务。v8 用的是 `CREATE TABLE IF NOT EXISTS`，
    /// 它本身幂等，真正需要盯的是**语义约束有没有落到 DDL 上** ——
    /// 因为约束一旦漏了，补起来要动数据，而那时库里已经有重复行了。
    #[test]
    fn v8_migration_creates_run_events_with_a_unique_sequence() {
        let v8 = ALL_MIGRATIONS[7].to_lowercase();
        assert!(
            v8.contains("create table if not exists run_events"),
            "v8 要建 run_events 表"
        );
        // (run_id, seq) 必须唯一 —— 依赖它做续传锚点，重复序号会让客户端静默跳过事件
        assert!(
            v8.contains("primary key (run_id, seq)"),
            "run_events 的 (run_id, seq) 必须唯一，否则续传会漏事件"
        );
        // 只建表，不该顺手改别的表 —— 改别的表属于另一版
        assert!(
            !v8.contains("alter table"),
            "v8 只该建 run_events；改已有表属于另一版"
        );
    }

    /// `(run_id, seq)` 的唯一性要在真库上验一遍，不能只看 SQL 文本。
    #[test]
    fn run_events_reject_a_duplicate_sequence() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let insert = "INSERT INTO run_events (run_id, seq, kind, payload, created_at) \
                      VALUES (?1, ?2, 'log', '{}', '2026-09-20T00:00:00Z')";
        conn.execute(insert, rusqlite::params!["run-a", 1]).unwrap();
        conn.execute(insert, rusqlite::params!["run-a", 2]).unwrap();

        // 同一 run 重复序号 → 拒绝（这正是续传锚点被破坏的样子）
        assert!(
            conn.execute(insert, rusqlite::params!["run-a", 2]).is_err(),
            "同一 run 的 seq 不能重复"
        );
        // 不同 run 可以用同一个 seq —— 序号是**每个 run 各自**的计数
        conn.execute(insert, rusqlite::params!["run-b", 1])
            .unwrap_or_else(|e| panic!("不同 run 的 seq 应当互不干扰: {e}"));

        // 最常走的查询是"某个 run 按 seq 序取出"，验证索引/主键真的能用且有序
        let seqs: Vec<i64> = conn
            .prepare("SELECT seq FROM run_events WHERE run_id = ?1 ORDER BY seq")
            .unwrap()
            .query_map(["run-a"], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    /// v7 老库升到 v8：`runs` 里的历史记录一条都不能动，且新表是空的。
    ///
    /// 空表是对的：v8 之前的事件**根本没落过库**（旧项目只放内存），
    /// 所以没有东西可以回填 —— 造一批假事件反而会污染"那次运行到底发生了什么"。
    #[test]
    fn v7_database_gains_run_events_and_keeps_runs() {
        let conn = Connection::open_in_memory().unwrap();
        for sql in &ALL_MIGRATIONS[..7] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        conn.execute(
            "INSERT INTO runs (id, account_id, capability, status, started_at, message) \
             VALUES ('run-old', '19****01', 'sign', 'Done', '2026-09-19T00:00:00Z', '旧记录')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let message: String = conn
            .query_row("SELECT message FROM runs WHERE id = 'run-old'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(message, "旧记录", "升级不该动 runs 里的历史");

        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM run_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0, "v8 之前没有事件记录，不许凭空造");
    }
}
