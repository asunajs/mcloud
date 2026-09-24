package mcloudio

import (
	"context"
	"database/sql"
	"fmt"

	"github.com/catlair/mcloud/internal/core"
)

// SchemaVersion 是当前 schema 版本。
//
// **新增迁移时必须同步递增**，并且 [allMigrations] 的长度要与它一致
// （这条由 TestMigrationsAreSequential 把关）。
//
// # 什么时候可以改老迁移，什么时候只能追加
//
// 本项目**尚未发布**，任何一个已应用的迁移理论上都可以直接改（改完把版本号退回去）。
// 但一旦有了外部用户的库文件，"已应用的迁移"就是历史，只能往后追加 ——
// 否则老库的 user_version 会说谎：它声称已到 v2，而 v2 的语句已经不是你写的那句了。
// 判断标准很简单：**有没有可能存在一个不是本机跑出来的库**。没有，就可以改；有，就只能加。
//
// ⚠ 方案文档 §6.3 的退出判据写作"schema v1→v7"，那是 v8（run_events）之前的口径。
// 以本常量为准：当前是 **10**。
const SchemaVersion int64 = 10

// allMigrations 是迁移清单：索引 i（0-based）对应版本 i+1。
//
// 每个元素必须是幂等 SQL（用 IF NOT EXISTS），
// 这样重复执行不会炸，也让"重跑迁移"成为安全操作。
//
// 这份清单是 schema 的**唯一真相源**。v1–v8 是在与 Rust 实现并行开发的时期
// 逐字对应着写的，但 2026-09-21 Rust 冻结并剥离为同级仓库后，那条同步义务
// **已终结**：改或加迁移都**不**需要、也不应该去对照 `../mcloud-rust` ——
// 那边的 schema.rs 是冻结快照，不再跟改。
var allMigrations = []string{
	// v1：账号 + 云朵日快照 + 运行记录
	`
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
    `,
	// v2：P2 的持久化实体 + 旧数据导入台账
	`
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
    --   1. sha256 判断"源文件变过没有" → 幂等重跑的依据；
    --   2. raw 保存**原始 JSON 全文** → 映射规则将来改了还能重放，
    --      而不用去求用户把旧目录找回来（这条也替代了方案里原本的 .bak 文件：
    --      归档在库里，搬走/备份数据库时不会漏掉）；
    --   3. issues 记录这次导入报了几个问题 → 便于"我上次到底看没看过那份报告"。
    CREATE TABLE IF NOT EXISTS imports (
        source       TEXT PRIMARY KEY,
        sha256       TEXT NOT NULL,
        imported_at  TEXT NOT NULL,
        record_count INTEGER NOT NULL DEFAULT 0,
        issues       INTEGER NOT NULL DEFAULT 0,
        raw          TEXT
    );
    `,
	// v3：系统用户 + 会话 + 账号归属（方案 §3.7）
	//
	// # 为什么这一段外面包了显式事务
	//
	// ALTER TABLE ... ADD COLUMN 是**唯一**一条无法写成 IF NOT EXISTS 的语句
	// （SQLite 不支持）。本框架的"幂等"其实有两层保障：
	//   1. user_version 门控 —— 已应用的迁移不会再跑，所以 ALTER 事实上只执行一次；
	//   2. 包事务 —— 万一这条 ALTER 之后的语句失败，不包事务就会留下
	//      "列已加、版本没升"的半成品库，而重跑时那条 ALTER 会报"duplicate column name"，
	//      用户从此进不了程序。包上事务后失败即整体回滚，重跑是干净的。
	// 注意：这一段里 CREATE 仍然全部带 IF NOT EXISTS（由单测逐条把关），
	// 事务只是把"半成品"这个状态消掉。
	`
    BEGIN;

    -- 系统用户：登录**本系统**的账号，与云盘账号是两回事（见 §3.7 的三概念表）
    CREATE TABLE IF NOT EXISTS users (
        id            TEXT PRIMARY KEY,
        -- 大小写不敏感唯一：否则 Alice 与 alice 能各建一个，
        -- 而登录时"匹配到谁"取决于查询顺序 —— 这种不确定性在鉴权路径上不可接受
        name          TEXT NOT NULL COLLATE NOCASE UNIQUE,
        -- argon2id 的 PHC 编码串（$argon2id$v=19$m=...$salt$hash）：
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
    -- 由 mcloud user init 初始化首个管理员时认领（见 StateStore.ClaimUnowned）。
    -- 迁移期无法给 owner 赋值：那时管理员还不存在，硬编一个只能是猜。
    ALTER TABLE accounts ADD COLUMN owner TEXT REFERENCES users(id);

    COMMIT;
    `,
	// v4：定时任务与组合也归属系统用户（§3.7「所有数据都要有主人」的收口）
	//
	// v3 只覆盖了账号；schedules / pipelines 当时仍是全局表。不补的后果不是"少个字段"，
	// 而是这两类数据**没有越权判据可加**：等 P5 的调度引擎与 WebUI 上来，
	// 一个用户就能看见并触发别人的定时任务与组合。
	//
	// 与 v3 一样必须包事务，理由同 v3 的注释（ALTER 无法 IF NOT EXISTS）。
	// 注意这里比 v3 **更危险**：v3 那一段至少还有"重跑时 CREATE 带 IF NOT EXISTS"
	// 这层缓冲，而这一段只有两条 ALTER —— 事务是唯一的保障。
	`
    BEGIN;

    -- 历史数据（v3 及以前）为 NULL = 未归属，与账号同一套语义：
    -- **谁都看不见**（漏判的代价不对称，见 model 的 IsVisibleTo），
    -- 由 mcloud user init 初始化首个管理员时认领。
    ALTER TABLE schedules ADD COLUMN owner TEXT REFERENCES users(id);
    ALTER TABLE pipelines ADD COLUMN owner TEXT REFERENCES users(id);

    COMMIT;
    `,
	// v5：smid 设备指纹的两份状态（第三方风控服务发下来的那个 deviceid 头）
	//
	// # 为什么放这张表，却又**不属于** AccountState
	//
	// 它们确实是"这个账号在这台设备上的状态"，与 account_state 是同一主体；
	// 但那张表的写入契约是**整体覆盖**（upsert_account_state，见 v2 注释与 model），
	// 而这两列的写入契约正相反：**换登录凭据不该动它，换指纹只动它**。
	// 所以列放在一起（同一个主体、同一套归属校验），而**读写走各自独立的方法**，
	// 并由 TestUpsertAccountStateLeavesTheSmidColumnsAlone 那条单测把关。
	//
	// 语义（三条生命周期不同，别混）：
	//   - smid_fingerprint：绑定账号的那份设备特征，JSON 文本。**只在主动更换时变**；
	//   - smid_device_id：最近一次上报换回的 deviceId（含 B 前缀）。服务端发的，会变，
	//     只用作下一次上报的 box 字段；换指纹时必须清空（旧 id 属于旧长相）。
	//
	// 与 v3/v4 同理：ALTER 不能 IF NOT EXISTS，必须包事务。
	`
    BEGIN;

    ALTER TABLE account_state ADD COLUMN smid_fingerprint TEXT;
    ALTER TABLE account_state ADD COLUMN smid_device_id TEXT;

    COMMIT;
    `,
	// v6：account_state.phone —— **完整手机号**，补上"拼不出 Authorization"那个缺口
	//
	// # 为什么必须存
	//
	// Authorization 的形状是 Basic base64("<前缀>:<手机号>:<authToken>")，
	// 中间那段是完整手机号，而它**推不出来**：
	// accounts.id 是掩码、jwt_token 里没有它、user_domain_id 是另一个东西。
	// 不存的结果就是每个要用这个头的能力都只能让调用方在命令行上重输一遍手机号
	// （mcloud signin --phone）—— 那是缺口不是设计。
	//
	// # 为什么放 account_state 而不是 accounts
	//
	// accounts 是**直接吐给前端**的展示表，那里面只能有掩码（phone_masked）。
	// account_state 里已经有同类的"标识而非凭据"（user_domain_id），
	// 且"列出账号"这条最常走的查询天然不碰这张表。
	//
	// # 与 smid_* 两列的差别（别把 v6 也做成"独立读写"）
	//
	// smid_* 之所以要独立读写，是因为它们的生命周期与登录态**相反**；
	// 而 phone 恰恰**跟着登录态走**：它来自这次登录的 auth 载荷，
	// 换登录就该跟着换。所以它**接进** UpsertAccountState 的整体覆盖列表，
	// 不另开方法 —— 这条由 TestUpsertAccountStateWritesThePhone 把关。
	//
	// # 历史行是 NULL
	//
	// v6 之前登录的账号这一列是 NULL：那是**如实记录"当时没存"**，
	// 不是"没有手机号"。不要拿掩码去回填 —— 掩码拼出来的头服务端认不认是未知数，
	// 而这类"能过但不该过"的值一旦过了一次，就会变成没人敢动的隐性依赖。
	// 补法只有一种：**重登一次**。
	//
	// 与前几版同理：ALTER 不能 IF NOT EXISTS，必须包事务。
	`
    BEGIN;

    ALTER TABLE account_state ADD COLUMN phone TEXT;

    COMMIT;
    `,
	// ── v7：account_state.authorization ─────────────────────────────────────
	//
	// 原样存下整个 Authorization 头值（Basic …）。
	//
	// # 为什么是"加一列"而不是"继续用 v6 的 phone 拼"
	//
	// v6 的思路是"把缺的那一段（手机号）补上，拼的时候就不缺料了"。
	// 那是**绕远路**，而且有三处具体的代价：
	//
	// 1. **旧数据搬不过来**。旧项目 mcloud-sign 的 asign.json 里，
	//    auth 存的**就是这个整串**（Basic base64("mobile:<手机号>:<authToken>")），
	//    它一直用得好好的。我们重写时把它解码、拆成三段、只留 token，
	//    再把手机号单存一列 —— 于是"旧库里有的东西，新库里凑不齐"。
	// 2. **前缀要靠猜**。Basic 后面第一段是 mobile（App）还是 pc（PC），
	//    拆开之后就只剩"调用点硬编码"。猜错不会报错，只会发一个服务端不认的头，
	//    表现成"未登录"（90001）—— 指向凭据坏了，而不是指向拼错了。
	//    整串存则前缀本来就在里面。
	// 3. **多一处能失败的环节**。三段缺一就用不了；整串只有一个"有没有"。
	//
	// # 与 phone 的关系
	//
	// 两者**冗余但不同源**，各有各的用途，都留着：
	//   - authorization：给上游用的凭据，读取侧**优先**用它；
	//   - phone：给人看的标识（日志里认出"签的是谁"），掩码不够用。
	//
	// 写入契约与 phone 完全一致：**跟着登录态走**，接进
	// UpsertAccountState 的整体覆盖列表，不另开窄写方法。
	//
	// # 历史行是 NULL
	//
	// v7 之前登录的账号这一列是 NULL，读取侧会**退回**"用 phone + jwt_token
	// 现拼"那条老路（SignInCredential.Load 里写明了退路与它的代价）。
	// 两条路并存是有意的：新行走新路，老行不必为了这一列重登一次。
	//
	// 与前几版同理：ALTER 不能 IF NOT EXISTS，必须包事务。
	`
    BEGIN;

    ALTER TABLE account_state ADD COLUMN authorization TEXT;

    COMMIT;
    `,
	// ── v8：run_events —— 运行事件流落库 ────────────────────────────────────
	//
	// # 为什么必须落库（而不是只在内存里广播）
	//
	// 旧项目的事件只活在内存（api/src/runner.ts 的 AsyncRunState），实测有三个洞：
	//
	// 1. **进程重启即丢**。用户第二天打开页面看"昨晚那次签到到底怎么了" —— 没有了。
	//    只剩 runs 表里一行 Done，谁也不知道那 20 个账号里哪一个失败了。
	// 2. **晚连的人看不到前面**。前端在 run 开始 3 秒后才打开页面，
	//    于是这 3 秒的日志永远拿不到 —— 而那几条恰恰是最关键的"在干什么"。
	// 3. **续传没有锚点**。断了要重连时，只能"整个重放"或"接下来什么都不给"。
	//    前者重复、后者丢事件，两者都会让前端显示一个错的进度。
	//
	// seq 就是给第 3 条用的：客户端把最后收到的 seq 带回来
	// （GET /api/runs/:id/events?after=<seq>），服务端从 seq + 1 接着发。
	// 这也是为什么 (run_id, seq) 必须**唯一** —— 序号一旦重复，
	// "继续发"就会变成"永远跳过某条"，而那是静默的、没人会发现。
	//
	// # 为什么事件体存 JSON 而不是拆列
	//
	// 五类事件的字段差别很大（Progress 有 done/total，Finished 有 outcome/
	// manual_steps/summary，Log 有 level/message）。拆成宽表的话大部分列对大部分行是
	// NULL，而且每加一个事件类型都要动 schema —— 而这恰恰是最容易变的东西。
	// 拆列的唯一好处是"能按 message 内容做 SQL 查询"，但我们不需要：
	// 查事件永远是"某个 run 的全部事件，按 seq 排"，用不到 JSON 内部的字段。
	//
	// # 为什么 run_events 不用 REFERENCES runs(id) 的外键
	//
	// 本仓库的 DSN 把 `foreign_keys(1)` 钉在**每条连接**上（见 store.go 的 Open），
	// 所以 schema 里声明的级联（如 v3 的 `sessions … ON DELETE CASCADE`）**是真正生效的**
	// —— 新增 REFERENCES 会当场起作用，别按"默认不生效、写了是假的"的旧口径想。
	// 即便如此，事件的删除仍以**显式清理**（DeleteRunEvents）为第一道、库级联只是兜底：
	// "哪些行属于这个 run"是业务语义，不该下放给 schema；而删 run 不清事件
	// 会让库越用越大。删用户 / 删 run 都不能只喊一声"CASCADE 会管"，由 store 层测试把关。
	//
	// # 保留策略
	//
	// 事件量按"一个 run 几百条"估，不做自动清理 —— 但 DeleteRunEvents 要有，
	// 否则将来想清也没有称手的工具。真要限制总量时加一条
	// DELETE FROM run_events WHERE run_id IN (SELECT id FROM runs WHERE started_at < ?)
	// 即可，不必改 schema。
	`
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
    `,
	// ── v9：schedules 的两列 —— 调度循环要能"择时"且能"只写记账" ─────────────
	//
	// v1–v8 期间这张表是"可存可读、但没有人照着它择时"的：`last_run_at` /
	// `last_status` / `last_error` 三列自落下那天起**没有任何写入方**
	// （`PUT /api/schedules` 只是把它们原样搬回去）。加上服务进程内的调度循环，
	// 这两列补的是循环缺的两样东西。
	//
	// # `miss_policy` —— 错过触发点补不补（逐行一个，不是全局开关）
	//
	// 用户 2026-09-22 的口径是"看任务情况：必须准时的不需要补，其他的需要"。
	// 为什么不能只在能力表里写死：那等于"改一个能力的脾气，会静默改掉用户
	// 已经建好的那些定时任务"。为什么这一列**可空**：NULL 表示"没有人显式选过"，
	// 此时跟随能力声明的默认；建表时替用户拍一个值填进去，就是把"没人选过"
	// 谎报成"他选了"（[core.MissPolicy] 的三态纪律）。
	//
	// # `last_fire_at` —— 已处理过的那个触发时刻
	//
	// 循环靠它算下一个点，且它**不能**复用 `last_run_at`：按 [core.MissSkip]
	// 弃掉的那一次不会留下任何运行，用 `last_run_at` 当水位的话，下一个 tick
	// 会把同一次错过重新捡起来 —— 表现成"每刷一次页面就多一条 skipped"。
	//
	// 与前几版同理：ALTER 不能 IF NOT EXISTS，必须包事务。
	`
    BEGIN;

    ALTER TABLE schedules ADD COLUMN miss_policy  TEXT;
    ALTER TABLE schedules ADD COLUMN last_fire_at TEXT;

    COMMIT;
    `,
	// ── v10：notify_config —— 推送（通知）配置落库 ─────────────────────────────
	//
	// # 为什么又是单行表
	//
	// 与前两张（`exchange_config` / `webui_config`）同一条理由：推送配置全局唯一一份，
	// 用 `CHECK (id = 1)` 钉住，免得"读出来的是哪一行"变成运气问题。
	// 推送的"这份配置属于谁"由 server 层的鉴权决定，不由这一行的列决定 ——
	// 真要按用户分那是**改语义**（单行 → 每用户一行），见 core.ConfigStore 里那条注释。
	//
	// # 为什么整块渠道配置存成一列 JSON
	//
	// 十个渠道各有自己的字段（还有 `Extra` 这种用户自由键），拆成列会让 schema 跟着
	// 渠道参数不停改版，而且拆完之后"读一份配置"要 JOIN 十次。
	// 需要被 SQL 查询的维度这里**一个都没有**：推送配置永远是整块读、整块写。
	// 三个标量（`enabled` / `only_error` / `min_level`）单独成列是有理由的 ——
	// 它们是界面上那一排开关，也是"这次到底发不发"的判据，读它们不该带出全部凭据。
	//
	// # 默认值方向：关闭 + info
	//
	// `enabled DEFAULT 0` 是这一版唯一的安全相关默认：这一族会把运行正文发到**第三方**
	// （以及用户自己的邮箱），"配置一迁过来就往外发"不是保真，是意外。
	// `min_level DEFAULT 'info'` 复刻旧实现的默认门槛。
	// `title` 可空且**没有默认值**：NULL 表示"没有人填过"，读取侧走
	// [core.DefaultNotifyTitle]；填一个默认标题进库会把"没填"谎报成"填了这个"，
	// 而将来要改默认文案时（就像本轮把旧值 `mcloud-v2 运行推送` 改掉那次）
	// 库里那一行会跟不上 —— 判据同 `miss_policy` 那一列。
	`
    CREATE TABLE IF NOT EXISTS notify_config (
        id         INTEGER PRIMARY KEY CHECK (id = 1),
        enabled    INTEGER NOT NULL DEFAULT 0,
        title      TEXT,
        only_error INTEGER NOT NULL DEFAULT 0,
        min_level  TEXT NOT NULL DEFAULT 'info',
        channels   TEXT NOT NULL DEFAULT '{}',
        updated_at TEXT
    );
    `,
}

// Migrate 执行未应用的迁移。
func Migrate(ctx context.Context, db *sql.DB) error {
	var current int64
	if err := db.QueryRowContext(ctx, "PRAGMA user_version").Scan(&current); err != nil {
		return core.NewInternal(fmt.Sprintf("读取 schema 版本失败: %v", err))
	}

	if current > SchemaVersion {
		return core.NewInternal(fmt.Sprintf(
			"数据库 schema 版本 %d 高于本程序支持的 %d，请升级程序", current, SchemaVersion))
	}

	for index, sqlText := range allMigrations {
		version := int64(index) + 1
		if version <= current {
			continue
		}
		if _, err := db.ExecContext(ctx, sqlText); err != nil {
			return core.NewInternal(fmt.Sprintf("应用迁移 v%d 失败: %v", version, err))
		}
		// ⚠ PRAGMA user_version 不能用占位符（SQLite 的 pragma 值不接受绑定参数），
		// 只能拼字符串。version 是本地循环下标，不来自外部输入。
		if _, err := db.ExecContext(ctx, fmt.Sprintf("PRAGMA user_version = %d", version)); err != nil {
			return core.NewInternal(fmt.Sprintf("写入 schema 版本 v%d 失败: %v", version, err))
		}
	}

	return nil
}
