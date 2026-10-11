//! 服务端日志的**环形缓冲** —— 给 WebUI 那个「日志」窗口用。
//!
//! # 为什么要有它（用户 2026-10-04 口径）
//!
//! 「我其实想要一个能看**所有**日志的窗口，**不仅仅是每次任务**」✓。
//!
//! 今天服务端的输出只去两个地方：launchd 重定向的那个**日志文件**
//! （要看就得开终端 `tail` ✗）与 `tracing` 的 stdout。而运行日志
//! （`data/logs/<id>.json`，由 `EventHub` 攒批落 ✓）只管**任务自己发的**日志 ✗
//! —— 调度器、凭据续期、**上游 http 那一层**，一行都进不去 ✓。
//!
//! 所以在这里挂一个 `tracing` 的 [`Layer`] ✓，把**所有**事件抄进一个定长
//! 环形缓冲 ✓，再由 `/api/logs` 交给界面 ✓。
//!
//! # ⚠⚠ 2026-10-09 改口径：内存是**窗口**，全量在**磁盘**（用户原话）
//!
//! 「**日志和历史不要全加入内存，使用本地记录，让用户设置最大大小**」
//! ＋「**大日志做虚拟加载或者分页，避免一次加载太多**」。
//!
//! 改之前这里是**唯一**的存储 ⇒ 它同时是"窗口"和"全部"，后果两条：
//! ① 超出 2000 行就**永久丢了** ✗（排障常要看的是"几小时前那次"）；
//! ② 重启即清空 ✗。用户要的正是"**使用本地记录**" ✓。
//!
//! 现在分三层：
//!
//! | 层 | 里面是什么 | 谁管上限 |
//! |---|---|---|
//! | 内存环 | **最近** [`MEM_LINES`] 行（**窗口**，含还没落盘的 ✓） | 设置 `serverLogMemLines` ✓ |
//! | `data/server-logs/<日期>.jsonl` | **全量**（追加写，一天一个文件 ✓） | 设置 `serverLogTotalMb` ✓（超了删**最旧**的整天文件 ✓） |
//! | `/api/logs` 分页 | 缺省给最新一页 ✓，`before=<seq>` 往上翻 ✓ | 一页最多 [`MAX_PAGE`] 行 ✓ |
//!
//! ⚠ 目录是 `server-logs`（不是 `logs`）：`data/logs/` 已经被**运行日志**占了
//! （一次运行一个文件、每 2 秒整份重写 ✓）—— 两者的写入方式与淘汰单位都不同 ✗。
//!
//! ⚠ 落盘是**攒批**的（1 秒或 200 行一次 ✓，与 `EventHub` 同一套节奏 ✓）：
//! 每条日志一次 `write` 会把高频日志拖慢 ✗。代价是"最后不到 1 秒的那几行还在内存里"
//! —— 而它们照样能通过 `/api/logs` 看到 ✓（内存优先 ✓）。
//!
//! # ⚠ 脱敏的规矩没变
//!
//! 这里抄的是**事件本身**（与 stdout 上那份同源 ✓）⇒ 脱敏仍由各个 emit 点
//! 负责 ✓（`RunEvent::scrubbed()` 那条规矩 ✓）。反过来说：**别直接往
//! `tracing` 打原始凭证** ✗（`tracing::info!(token = %raw)` 就是漏点 ✓，
//! 因为它绕过了事件那一层的闸 ✓）。

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// 内存窗口的**默认**行数（= `mcloud_core::DEFAULT_SERVER_LOG_MEM_LINES` ✓）。
///
/// 2000 行 ≈ 几百 KB ✓（一条几十到几百字节 ✓）。⚠ 它现在只是**默认值**：
/// 用户能在设置里改（`serverLogMemLines` ✓），而**完整历史**在
/// `data/server-logs/` 里 ✓ —— 这个窗口解决的是"不开终端就能看" ✓，不是"当归档用" ✓。
pub const RING_CAPACITY: usize = mcloud_core::DEFAULT_SERVER_LOG_MEM_LINES;

/// 内存窗口最多留多少行（**可设** ✓，见 [`apply_settings`]）。
///
/// ⚠ 它是 `AtomicUsize` 而不是 `const`：用户在设置里改完要**立刻生效** ✓
/// （改小 ⇒ 当场把最旧的 trim 掉 ✓）。
static MEM_LINES: AtomicUsize = AtomicUsize::new(RING_CAPACITY);

/// 服务端日志目录的**总量**上限（字节，**可设** ✓）。
static MAX_BYTES: AtomicU64 =
    AtomicU64::new(mcloud_core::DEFAULT_SERVER_LOG_TOTAL_MB * 1024 * 1024);

/// 一页最多多少行（防"要 100 万行"把内存拉爆 ✗）。
pub const MAX_PAGE: usize = 5000;

/// 落盘目录名（在数据目录下 ✓）。
pub const LOG_DIR_NAME: &str = "server-logs";

/// 环里"**任务日志**"那一档（某个 run 的日志）。
///
/// 判据见 [`category_of`]：**只有** [`mcloud_app::run_labels::trace_event`] 打的行
/// 算任务日志 ✓（它盖了账号那一格 ⇒ 能归属到人 ✓）。
pub const CATEGORY_TASK: &str = "task";

/// 环里"**运维日志**"那一档（实例自己的日志：认证失败、路径被拒、调度器、上游 http）。
///
/// ⚠ 它是**兜底**那一档：认不出归属的一律进这里 ⇒ 只给管理员看 ✓
/// （失败关闭 —— 见 `server_logs.rs` 的可见性说明 ✓）。
pub const CATEGORY_OPS: &str = "ops";

/// 一行的类别：**谁打的**决定它是哪一档（2026-10-06 用户口径：
/// 「这种应该分为运维日志」）。
///
/// ⚠ 用 target 而**不是**"有没有 account 字段"来判：上游 http 那层
/// （`mcloud_upstream::http_log`）也会打 `account=…`（`上游应答 account=… status=200` ✓），
/// 但它是**实例**的日志（凭证续期、限流），不是某次运行的日志 ✗
/// ⇒ 按"有没有账号"分会把运维日志错判成任务日志（= 漏给普通用户）✗。
fn category_of(target: &str) -> &'static str {
    if target == mcloud_app::run_labels::LOG_TARGET {
        CATEGORY_TASK
    } else {
        CATEGORY_OPS
    }
}

/// 一行服务端日志。
///
/// ⚠ 它要能**读回来**（`Deserialize` ✓）：盘上那份是翻页的真源 ✓。
/// `#[serde(default)]` 是给"以后加字段"留的口子 —— 老行缺新字段时按默认值走 ✓，
/// 而不是整页 parse 失败 ✗。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerLog {
    /// `error` / `warn` / `info` / `debug` / `trace` —— 与前端那套标记同名 ✓。
    pub level: String,
    /// 来源模块（`mcloud_engine::scheduler` 这种 ✓）—— 排障时"谁在说话"很关键 ✓。
    pub target: String,
    pub message: String,
    pub at_ms: i64,
    /// [`CATEGORY_TASK`] / [`CATEGORY_OPS`] —— 见 [`category_of`] ✓。
    ///
    /// ⚠ 这两档的**可见性不同**（任务日志按账号归属、运维日志只给管理员 ✓），
    /// 所以它是**权限判据的一部分**，不只是给人看的标签 ✓。
    pub category: String,
    /// 这一行属于哪个账号（`message` 里 `account="…"` 那一格的**结构化**副本 ✓）；
    /// 认不出就是 `None` ✓。
    ///
    /// ⚠ 与 `message` 里那格是**同一份事实的两种形状**：`message` 那份是给人看的
    /// （保持原样、一个字都不动 ✓），这一份是给**机器**用的（按账号过滤 ✓）。
    /// 前端原先得拿正则从文本里认（`web/src/pages/logs-page.tsx` 的 `ACCOUNT_RE`），
    /// 现在可以直接用这一格 ✓。
    pub account: Option<String>,
    /// 全局单调序号 —— **翻页游标** ✓（`/api/logs?before=<seq>` ✓）。
    ///
    /// ⚠⚠ 它必须**跨重启单调**：游标一旦会重复，翻页就会取错页（甚至翻不动）✗。
    /// 所以起点取 `(启动时刻毫秒) << 8`，之后自增 —— 毫秒严格递增 ⇒
    /// 两次启动的区间**不会重叠** ✓（同一毫秒内重启两次的概率与后果都可忽略 ✓）。
    ///
    /// ⚠ 为什么是 `<< 8` 而不是更大的位移：这个数会原样发给**前端**，而 JS 的
    /// `Number.MAX_SAFE_INTEGER` 是 9.007e15 ✓。`<< 12` 在 2026 年就已经到 7.3e15
    /// （离上限只剩 20%，约 2039 年就越界 ⇒ 前端比大小**悄悄失精度** ✗）；
    /// `<< 8` 是 4.6e14（余量 20 倍 ✓），而每毫秒 256 条日志远超实际（实测峰值个位数 ✓）。
    ///
    /// ⚠ 它**不是**"第几行"：中间会跳号（进程重启、以及被内存窗口淘汰的行 ✓）——
    /// 游标只需要"可比大小"，不需要连续 ✓。
    pub seq: u64,
}

impl Default for ServerLog {
    fn default() -> Self {
        Self {
            level: "info".into(),
            target: String::new(),
            message: String::new(),
            at_ms: 0,
            category: CATEGORY_OPS.into(),
            account: None,
            seq: 0,
        }
    }
}

fn ring() -> &'static Mutex<VecDeque<ServerLog>> {
    static RING: OnceLock<Mutex<VecDeque<ServerLog>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_CAPACITY)))
}

/// 攒批的节奏：**1 秒或 200 行**先到先刷 ✓（与 `EventHub` 同一套 ✓）。
const FLUSH_MS: i64 = 1000;
const FLUSH_LINES: usize = 200;

/// 查"磁盘总量上限"的节奏：每 30 秒一次 ✓（每刷一次就扫目录没必要，但也不能不查 ✓）。
const BUDGET_MS: i64 = 30_000;

/// 序号的**基准**（0 = 还没定 ⇒ 按时钟兜底，见 [`next_seq`] ✓）。
///
/// ⚠ 它是 `AtomicU64` 而不是 `OnceLock`：`init` 时才知道盘上已有的最大序号，
/// 那时要把基准**抬上去** ✓（`OnceLock` 定死了就抬不动 ✗）。
static SEQ_BASE: AtomicU64 = AtomicU64::new(0);

/// 基准之上的自增计数 ✓。
static SEQ_NEXT: AtomicU64 = AtomicU64::new(0);

/// 按时钟算的基准：`(启动毫秒) << 8` ✓。
fn clock_base() -> u64 {
    (now_ms().max(0) as u64) << 8
}

/// 下一个序号（见 [`ServerLog::seq`] ✓）。
fn next_seq() -> u64 {
    let base = SEQ_BASE.load(Ordering::Relaxed);
    // `init` 之前（启动那几十行 ✓）只按时钟 —— 那几行的序号反正会被 `init` 抬到下面去 ✓
    let base = if base == 0 { clock_base() } else { base };
    base + SEQ_NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 盘上已有的最大序号（没有文件 / 读不到 ⇒ 0 ✓）。
///
/// ⚠ 只在 `init` 里调一次（启动时 ✓）：翻页要靠"新行的 seq 一定大于旧行"，
/// 而这个前提**不能**只靠时钟 —— 换基准、时钟回拨、手工搬过文件都会破坏它 ✗
///（2026-10-09 实测踩到：把基准从 `<< 12` 改成 `<< 8` 之后，旧文件里那几行反而更大 ✓）。
fn max_seq_on_disk(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut max = 0u64;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        for line in text.lines() {
            let Ok(l) = serde_json::from_str::<ServerLog>(line) else {
                continue;
            };
            max = max.max(l.seq);
        }
    }
    max
}

/// 落盘那一侧的状态。
///
/// ⚠ `dir == None` ⇒ **只进内存、不落盘** ✓。两个场合会这样：
/// ① 测试进程（`main()` 不跑 ⇒ [`init`] 不被调用 ✓）；
/// ② CLI 模式（`mcloud <子命令>` 不落盘 ✓ —— 那是"跑一条命令"，日志走 stdout 就够 ✓）。
#[derive(Default)]
struct Sink {
    dir: Option<PathBuf>,
    /// 还没写下去的行 ✓（攒批，见 [`FLUSH_MS`]）。
    pending: Vec<ServerLog>,
    last_flush_ms: i64,
    last_budget_ms: i64,
}

fn sink() -> &'static Mutex<Sink> {
    static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(Sink::default()))
}

/// ⚠ 落盘相关的报错**不能**用 `tracing` 打 ✗ —— `RingLayer` 会把那条错误本身
/// 再抄进环 ⇒ 写盘一直失败就自我刷屏（还可能触发下一次刷盘 ✗）。
/// 所以走 stderr（它同时被 launchd 收进 `webui.err.log` ✓ 不会丢）。
fn warn_sink(msg: &str) {
    eprintln!("⚠ 服务端日志落盘：{msg}");
}

/// 本地日期串（`2026-10-09`）—— 日志文件按**本地**天分 ✓（用户看的是本地时间 ✓）。
fn day_of(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|| "0000-00-00".to_string())
}

fn push(entry: ServerLog) {
    // ⚠ 顺序：**先盘后环** —— 环可能把刚进来的这行立刻淘汰（窗口很小的时候 ✓），
    // 而盘上那份必须留下 ✓（那正是"全量在磁盘"的意思 ✓）。
    let mut sk = sink().lock().unwrap_or_else(|e| e.into_inner());
    if sk.dir.is_some() {
        sk.pending.push(entry.clone());
        flush_if_due(&mut sk);
    }
    drop(sk);

    let cap = MEM_LINES.load(Ordering::Relaxed).max(1);
    let mut r = ring().lock().unwrap_or_else(|e| e.into_inner());
    while r.len() >= cap {
        // 满了丢**最旧**的 ✓（窗口语义：排障看的是"刚刚发生了什么" ✓）
        r.pop_front();
    }
    r.push_back(entry);
}

/// 到点了就把 `pending` 追加进当天的文件 ✓。
///
/// ⚠ 写盘失败**只警告不失败**（与"池写失败"同一口径 ✓）：日志是排障用的，
/// 不能拿它去否定正在跑的业务 ✗。失败的代价是"这一段历史只在内存里" ✓。
fn flush_if_due(sk: &mut Sink) {
    let now = now_ms();
    let due = sk.pending.len() >= FLUSH_LINES || now - sk.last_flush_ms >= FLUSH_MS;
    if !due {
        return;
    }
    sk.last_flush_ms = now;
    flush_pending(sk);
    // 总量上限低频查一次 ✓（目录里就几个文件，但没必要每秒扫 ✓）
    if now - sk.last_budget_ms >= BUDGET_MS {
        sk.last_budget_ms = now;
        if let Some(dir) = sk.dir.clone() {
            enforce_budget(&dir);
        }
    }
}

/// 把 `pending` 写下去（**追加** ✓ —— 不动已有内容 ✓）。
fn flush_pending(sk: &mut Sink) {
    if sk.pending.is_empty() {
        return;
    }
    let Some(dir) = sk.dir.clone() else {
        sk.pending.clear();
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn_sink(&format!("建目录 {dir:?} 失败：{e}"));
        return;
    }
    let path = dir.join(format!("{}.jsonl", day_of(now_ms())));
    let mut buf = String::new();
    for e in sk.pending.drain(..) {
        match serde_json::to_string(&e) {
            Ok(line) => {
                buf.push_str(&line);
                buf.push('\n');
            }
            // 单行序列化失败不该拖垮整批 ✓（跳过它，其余照写 ✓）
            Err(e) => warn_sink(&format!("某一行序列化失败（已跳过）：{e}")),
        }
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(buf.as_bytes()) {
                warn_sink(&format!("写 {path:?} 失败：{e}"));
            }
        }
        Err(e) => warn_sink(&format!("打开 {path:?} 失败：{e}")),
    }
}

/// 目录总量超上限 ⇒ 删**最旧**的整天文件 ✓。
///
/// ⚠ **不删当天那个正在写的文件**：哪怕它自己就超了上限 —— 删它等于把"现在"删掉 ✗
/// （单日超上限只可能发生在"一天几百万行"那种刷屏场景，而那时上限本身就该调大 ✓）。
fn enforce_budget(dir: &Path) {
    let max = MAX_BYTES.load(Ordering::Relaxed);
    let today = format!("{}.jsonl", day_of(now_ms()));
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        files.push((name, e.path(), md.len()));
    }
    // 文件名就是日期 ⇒ 字典序 = 时间序 ✓（定长格式 ✓）
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    for (name, path, len) in &files {
        if total <= max {
            break;
        }
        if *name == today {
            continue; // 见上面的"不删今天"✓
        }
        match std::fs::remove_file(path) {
            Ok(()) => {
                total = total.saturating_sub(*len);
            }
            Err(e) => warn_sink(&format!("删 {path:?} 失败：{e}")),
        }
    }
}

/// [`recent`] 的筛选条件。
///
/// ⚠ 做成结构体而**不是**继续往 `recent` 上加参数（2026-10-06 加 `category`/`accounts`
/// 时正好三变五）：五个位置参数里有两个是 `Option<&str>`，调用点写错顺序**编译器
/// 不会拦** ✗（两个都是 `Option<&str>` ⇒ 类型一模一样）。
#[derive(Debug, Clone)]
pub struct Filter<'a> {
    /// 最多回多少行（内部会夹到 `1..=RING_CAPACITY` ✓）。
    pub limit: usize,
    /// 只看**不低于**这个级别的；`None` ⇒ 不过滤 ✓。
    pub min_level: Option<&'a str>,
    /// 关键词（大小写不敏感，匹配正文与来源 ✓）。
    pub query: Option<&'a str>,
    /// 只看某一档（[`CATEGORY_TASK`] / [`CATEGORY_OPS`]）；`None` ⇒ 两档都要 ✓。
    pub category: Option<&'a str>,
    /// 只留这些账号的行；`None` ⇒ 不按账号过滤 ✓。
    ///
    /// ⚠ 只对**任务日志**有意义（运维日志没有归属 ✓），而**没有账号**的任务日志行
    /// 在给了 `Some(..)` 时会被滤掉 —— 这正是"归属不明 ⇒ 不给"的失败关闭 ✓。
    pub accounts: Option<&'a std::collections::HashSet<String>>,
}

impl<'a> Filter<'a> {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            min_level: None,
            query: None,
            category: None,
            accounts: None,
        }
    }
}

/// 取最近若干行（= [`page`] 不带游标 ✓）。
///
/// 筛选条件见 [`Filter`] ✓。
///
/// ⚠ 返回的是**时间正序**（最早的在前 ✓）：界面直接从上往下渲染 ✓，
/// 与"日志该有的样子"一致 ✓。
pub fn recent(f: &Filter<'_>) -> Vec<ServerLog> {
    page(f, None)
}

/// 一行是否过得了这套筛选条件 ✓。
///
/// ⚠ 抽出来是**必须**的：内存那一层与磁盘那一层都要用它 ⇒ 两处各写一遍
/// 必然分叉（"翻到第二页发现筛选失效了"就是那种分叉的典型症状 ✗）。
fn keeps(l: &ServerLog, f: &Filter<'_>) -> bool {
    let floor = f.min_level.map(rank_of).unwrap_or(0);
    if rank_of(&l.level) < floor {
        return false;
    }
    if let Some(c) = f.category {
        if l.category != c {
            return false;
        }
    }
    if let Some(acc) = f.accounts {
        // ⚠ `None` 与"空集合"是**两件事**：前者不按账号过滤 ✓，
        // 后者是"这个人一个账号都没有" ⇒ 一条任务日志都不给 ✓。
        if !l.account.as_deref().is_some_and(|a| acc.contains(a)) {
            return false;
        }
    }
    let q = f.query.unwrap_or("").trim().to_lowercase();
    if !q.is_empty()
        && !l.message.to_lowercase().contains(&q)
        && !l.target.to_lowercase().contains(&q)
    {
        return false;
    }
    true
}

/// 取一页服务端日志（**分页** ✓）—— 界面上那个「日志」窗口用 ✓。
///
/// 语义与运行日志那套**同一口径**（`mcloud_core::slice_page` ✓）：
///
/// | `before` | 给什么 |
/// |---|---|
/// | `None` | **最新** `limit` 行 ✓（界面打开时的第一屏 ✓） |
/// | `Some(seq)` | `seq < before` 的最后 `limit` 行 ✓（**往上翻** ✓） |
///
/// 返回**时间正序** ✓；返回行数 `< limit` ⇒ 没有更多了 ✓（前端据此收起按钮 ✓）。
///
/// # 两条路，内存优先
///
/// 先扫**内存环**（它装着最新的那些 ✓，而且是**唯一**装着"还没落盘的行"的地方 ✓
/// —— 攒批最多 1 秒，那 1 秒里的行只在这里 ✓），不够再从**磁盘**按 `seq` 倒着补 ✓。
/// 两边的筛选判据是同一个 [`keeps`] ✓。
pub fn page(f: &Filter<'_>, before: Option<u64>) -> Vec<ServerLog> {
    let limit = f.limit.clamp(1, MAX_PAGE);
    // 收集时**从新到旧**（最后统一 reverse ✓），这样"取满 limit 就停"是最省的做法 ✓
    let mut out: Vec<ServerLog> = Vec::new();
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    {
        let r = ring().lock().unwrap_or_else(|e| e.into_inner());
        for l in r.iter().rev() {
            if let Some(b) = before {
                if l.seq >= b {
                    continue;
                }
            }
            if !keeps(l, f) {
                continue;
            }
            seen.insert(l.seq);
            out.push(l.clone());
            if out.len() == limit {
                break;
            }
        }
    }
    if out.len() < limit {
        read_from_disk(f, before, limit - out.len(), &mut seen, &mut out);
    }
    out.reverse();
    out
}

/// 从磁盘（`data/server-logs/*.jsonl`）**倒着**补 `need` 行进 `out` ✓。
///
/// ⚠ 读的顺序：文件按**文件名倒序**（= 日期从新到旧 ✓），文件内按**行倒序** ✓
/// —— 与"翻页总是从最新往回走"这件事对齐 ✓。
///
/// ⚠ 代价（如实写）：为了凑满一页，最坏要**把一个文件整个读一遍并逐行 parse** ✗。
/// 单日文件按实测几千行（几百 KB）⇒ 毫秒级 ✓；真到了几十万行那天，
/// 该做的是"按 seq 二分 + 偏移索引"，而不是现在这版顺序扫 ✓
/// （先不做索引：索引要和"删旧文件""追加写"两处同步，漏一处就取错页 ✗）。
fn read_from_disk(
    f: &Filter<'_>,
    before: Option<u64>,
    need: usize,
    seen: &mut std::collections::HashSet<u64>,
    out: &mut Vec<ServerLog>,
) {
    let Some(dir) = sink().lock().unwrap_or_else(|e| e.into_inner()).dir.clone() else {
        return; // 没落盘（测试进程 / CLI 模式 ✓）
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".jsonl").then(|| (name, e.path()))
        })
        .collect();
    // 文件名 = 日期 ⇒ 字典序 = 时间序 ✓（这里要**从新到旧**，所以反向排 ✓）
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_name, path) in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines().rev() {
            if out.len() >= need {
                return;
            }
            if line.trim().is_empty() {
                continue;
            }
            // 坏行（半行 / 手工改过）⇒ 跳过它，别让整页失败 ✗
            let Ok(l) = serde_json::from_str::<ServerLog>(line) else {
                continue;
            };
            if let Some(b) = before {
                if l.seq >= b {
                    continue;
                }
            }
            if seen.contains(&l.seq) || !keeps(&l, f) {
                continue;
            }
            seen.insert(l.seq);
            out.push(l);
        }
    }
}

/// 挂上落盘目录并套用设置（**起服务时调一次** ✓，见 `main.rs::serve`）。
///
/// ⚠ 在它之前发生的日志只在内存里（`tracing` subscriber 比它早装 ✓）——
/// 代价可接受：那几行是启动过程本身，而它们同时也在 launchd 的 `webui.err.log` 里 ✓。
pub fn init(data_dir: &Path, settings: &mcloud_core::AppSettings) {
    {
        let mut sk = sink().lock().unwrap_or_else(|e| e.into_inner());
        sk.dir = Some(data_dir.join(LOG_DIR_NAME));

        // ⚠⚠ **先把序号基准抬到"大于盘上与环里已有的任何序号"**，再谈别的。
        //
        // 翻页的全部前提就是"新行的 seq 一定大于旧行"（`before` 游标才成立 ✓）。
        // 只按时钟算基准的话，三件事会破坏它：换基准（今天刚发生 ✓）、时钟回拨、
        // 从别处搬来一份日志文件 ✗ —— 而症状是"翻页取回同一段/取错页"，
        // 极难归因 ✓。
        let mut max_seen = max_seq_on_disk(&sk.dir.clone().expect("刚设过"));
        if let Ok(r) = ring().lock() {
            for l in r.iter() {
                max_seen = max_seen.max(l.seq);
            }
        }
        SEQ_BASE.store(
            clock_base().max(max_seen.saturating_add(1)),
            Ordering::Relaxed,
        );
        SEQ_NEXT.store(0, Ordering::Relaxed);
        // ⚠⚠ 把**环里已经有的**那些补进待写队列 —— 它们是 `init` **之前**发生的
        // （启动过程那几十行 ✓），而那时还没有目录 ⇒ 它们**从没进过** `pending` ✗。
        // 不补的话"启动日志"永远只活在内存里（用户口径是「**所有**日志都要本地」✗）——
        // 而"重启之后想查上次启动怎么了"正是最常翻的一段 ✓。
        //
        // ⚠ 不用担心重复：`init` 之前 `dir` 是 `None` ⇒ 一条都没落过盘 ✓。
        {
            let r = ring().lock().unwrap_or_else(|e| e.into_inner());
            let early: Vec<ServerLog> = r.iter().cloned().collect();
            sk.pending.extend(early);
        }
        // 启动时立刻落下去 ✓（不等 1 秒 ✓）
        sk.last_flush_ms = 0;
        flush_pending(&mut sk);
        if let Some(dir) = sk.dir.clone() {
            enforce_budget(&dir);
        }
    }
    apply_settings(settings);
}

/// 套用设置（内存窗口行数 + 磁盘总量上限 ✓）—— **改完立刻生效** ✓。
///
/// ⚠ 改小内存窗口 ⇒ 当场把最旧的 trim 掉 ✓（否则要等下一行日志才生效，
/// 而"改完看不出变化"正是用户最烦的那种反馈 ✗）。
pub fn apply_settings(settings: &mcloud_core::AppSettings) {
    MEM_LINES.store(settings.server_log_mem_lines.max(1), Ordering::Relaxed);
    MAX_BYTES.store(settings.server_log_total_bytes(), Ordering::Relaxed);

    // ⚠⚠ **先落盘、再 trim** —— 顺序反了就是**静默丢历史** ✗。
    //
    // 攒批最多 1 秒 ⇒ 窗口缩小的那一刻，`pending` 里可能还有几百行还没写下去 ✓。
    // 直接 trim 环的话，被淘汰的那些**既不在环里也不在盘上** ⇒ 永久消失 ✗
    // （而"全量在磁盘"这条口径的含义正是：**淘汰只能淘汰已经落盘的** ✓）。
    let dir = {
        let mut sk = sink().lock().unwrap_or_else(|e| e.into_inner());
        flush_pending(&mut sk);
        sk.dir.clone()
    };

    let cap = MEM_LINES.load(Ordering::Relaxed);
    {
        let mut r = ring().lock().unwrap_or_else(|e| e.into_inner());
        while r.len() > cap {
            r.pop_front();
        }
    }
    if let Some(dir) = dir {
        enforce_budget(&dir);
    }
}

/// 立刻把攒着的行写下去（测试用 ✓；生产靠 [`FLUSH_MS`] 到点自动刷 ✓）。
#[cfg(test)]
fn flush_now() {
    let mut sk = sink().lock().unwrap_or_else(|e| e.into_inner());
    flush_pending(&mut sk);
}

/// 当前内存窗口的行数上限（测试用 ✓）。
#[cfg(test)]
fn mem_lines() -> usize {
    MEM_LINES.load(Ordering::Relaxed)
}

/// 摘掉落盘目录（测试用 ✓ —— 静态量在用例之间会串，每条开头先摘干净 ✓）。
#[cfg(test)]
fn detach() {
    let mut sk = sink().lock().unwrap_or_else(|e| e.into_inner());
    sk.dir = None;
    sk.pending.clear();
    MEM_LINES.store(RING_CAPACITY, Ordering::Relaxed);
    MAX_BYTES.store(
        mcloud_core::DEFAULT_SERVER_LOG_TOTAL_MB * 1024 * 1024,
        Ordering::Relaxed,
    );
}

/// 级别的高低（越大越严重 ✓）—— 用于 `min_level` 过滤 ✓。
fn rank_of(level: &str) -> u8 {
    match level {
        "error" => 4,
        "warn" => 3,
        "info" => 2,
        "debug" => 1,
        _ => 0, // trace 与任何没见过的写法
    }
}

/// 把 `tracing` 的事件抄进环形缓冲的 Layer ✓。
///
/// 装法与理由见 `main.rs` 里 `serve` 之前那一段 ✓。
pub struct RingLayer;

impl<S> Layer<S> for RingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let target = event.metadata().target();
        push(ServerLog {
            level: event.metadata().level().as_str().to_lowercase(),
            target: target.to_string(),
            category: category_of(target).to_string(),
            account: fields.account.clone(),
            message: fields.finish(),
            at_ms: now_ms(),
            // ⚠ 序号在这里取：**每一条**进环的日志都要有，且要单调 ✓
            seq: next_seq(),
        });
    }
}

/// 事件字段的收集器。
///
/// `message` 放最前 ✓，其余字段按 `k=v` 追加 ✓ —— 这两样正是
/// `mcloud_upstream::http_log` 那种日志的可读形状（`上游应答 account=… status=200` ✓）。
///
/// ⚠ `account` **额外**留一份结构化的（见 [`Fields::account`]）：正文那份照旧，
/// 这一份只给机器用 ✓。
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
    /// `account="…"` 那一格的裸值（没有引号 ✓）—— 见 [`unquote`]。
    account: Option<String>,
}

impl Fields {
    fn finish(self) -> String {
        if self.rest.is_empty() {
            self.message
        } else {
            format!("{} {}", self.message, self.rest)
        }
    }
}

/// 把 `{:?}` 打出来的值还原成裸值（只脱**一层**引号 ✓）。
///
/// `tracing` 里 `account = ?a` 与 `account = a`（`a: &str`）最终都走到
/// `record_debug`，而 `&str` 的 `Debug` 是**带引号**的 ⇒ 结构化那份要脱掉 ✓。
/// 不是"两端都是引号"就原样返回（数字、`Option` 之类不该被削 ✓）。
fn unquote(s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // ⚠ 这里**不能**用 `{:?}` 的思路套所有字段：对 `message`（它是
            // `format_args!`）`{:?}` 打出来就是原文 ✓，但别的字段值若是 `&str`，
            // `{:?}` 会**带引号** ✓ —— 所以两条分支分开写 ✓。
            self.message = format!("{value:?}");
        } else {
            if field.name() == "account" {
                self.account = Some(unquote(format!("{value:?}")));
            }
            if !self.rest.is_empty() {
                self.rest.push(' ');
            }
            self.rest.push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// ⚠⚠ 环形缓冲是**进程级**静态量：**所有**碰它的用例（本文件的 `tests` 与
/// `category_tests` 两个模块）必须共用**同一把**串行锁，并且每条先 `reset()` ✓。
///
/// # 为什么提到文件级（2026-10-06 实测踩到）
///
/// 原先 `tests` 与 `category_tests` **各写了一份** `serial()` —— 两份是**两个不同的
/// `static LOCK`** ✗ ⇒ 两个模块的用例可以同时跑、同时读写同一个环 ⇒
/// `filters_by_level_and_query` 当场红（它期望 2 条，看到的是别人塞进去的 0 条 ✓）。
/// 一把锁放两处 = 没锁 ✓。
///
/// 真代码里没有这个问题 ✓：只有一个进程、一个环，而写它的只有一个 `tracing`
/// subscriber ✓。这是**测试**的事，不是设计缺陷 ✓。
#[cfg(test)]
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 清空缓冲（每条用例自己先清一遍 ✓，断言一律用"包含"而不是"等于" ✓）。
#[cfg(test)]
fn reset() {
    ring().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: &str, target: &str, message: &str) -> ServerLog {
        ServerLog {
            level: level.into(),
            target: target.into(),
            message: message.into(),
            at_ms: 1_700_000_000_000,
            // ⚠ 走**真**的序号生成器：这样"翻页游标单调"这件事在用例里也成立 ✓
            seq: next_seq(),
            // ⚠ 走**真**的判据（而不是写死 "ops"）：这样 `category_of` 变了
            // 这些用例会跟着变，不会变成"自己跟自己一致"的摆设 ✓
            category: category_of(target).to_string(),
            account: None,
        }
    }

    /// 便捷构造筛选条件：`Filter::new(limit)` 那三格默认全是 `None` ✓。
    fn filt(limit: usize) -> Filter<'static> {
        Filter::new(limit)
    }

    #[test]
    fn keeps_at_most_capacity_and_drops_the_oldest() {
        let _guard = serial();
        reset();
        for i in 0..(RING_CAPACITY + 10) {
            push(entry("info", "t", &format!("第 {i} 行")));
        }
        let all = recent(&filt(RING_CAPACITY));
        assert_eq!(all.len(), RING_CAPACITY, "必须封顶");
        // 丢的是**最旧**的 ✓
        assert!(
            !all.iter().any(|l| l.message == "第 0 行"),
            "最早的该被丢掉"
        );
        assert_eq!(
            all.last().expect("非空").message,
            format!("第 {} 行", RING_CAPACITY + 9),
            "最新的必须在 ✓"
        );
    }

    #[test]
    fn filters_by_level_and_query() {
        let _guard = serial();
        reset();
        push(entry("info", "mcloud_engine::scheduler", "调度器已启动"));
        push(entry(
            "warn",
            "mcloud_upstream::http_log",
            "上游应答 status=403",
        ));
        push(entry("error", "mcloud_app", "落盘失败"));

        // 级别过滤：只看 warn 及以上 ✓
        let warns = recent(&Filter {
            min_level: Some("warn"),
            ..filt(50)
        });
        assert!(warns.iter().all(|l| l.level != "info"), "{warns:?}");
        assert_eq!(warns.len(), 2);

        // 关键词：大小写不敏感 ✓，正文与来源都算 ✓
        assert_eq!(
            recent(&Filter {
                query: Some("STATUS=403"),
                ..filt(50)
            })
            .len(),
            1
        );
        assert_eq!(
            recent(&Filter {
                query: Some("scheduler"),
                ..filt(50)
            })
            .len(),
            1
        );
        assert_eq!(
            recent(&Filter {
                query: Some("没有这回事"),
                ..filt(50)
            })
            .len(),
            0
        );

        // 顺序：时间正序 ✓
        let all = recent(&filt(50));
        assert_eq!(all.first().expect("非空").message, "调度器已启动");
        assert_eq!(all.last().expect("非空").message, "落盘失败");
    }

    #[test]
    fn limit_is_clamped() {
        let _guard = serial();
        reset();
        push(entry("info", "t", "只有一行"));
        assert_eq!(recent(&filt(0)).len(), 1, "0 也要给一行，别返回空");
        assert!(recent(&filt(usize::MAX)).len() <= RING_CAPACITY);
    }

    /// ⚠ 真正装一个 subscriber 跑一遍：证明 `RingLayer` **确实**在收事件 ✓
    /// —— 少了这条，整个环可能一直空着而不报错 ✗。
    #[test]
    fn layer_actually_records_events() {
        use tracing_subscriber::layer::SubscriberExt;
        let _guard = serial();
        reset();
        let sub = tracing_subscriber::registry().with(RingLayer);
        tracing::subscriber::with_default(sub, || {
            tracing::warn!(account = "13800000000", "上游应答 status=403");
        });
        let got = recent(&filt(10));
        let line = got.last().expect("该收到一条");
        assert_eq!(line.level, "warn");
        assert!(line.message.contains("上游应答 status=403"), "{line:?}");
        // 结构化字段也要跟着 ✓（`k=v` ✓）—— 排障靠的就是它们 ✓
        assert!(line.message.contains("account"), "{line:?}");
    }

    /// ⚠⚠ 用户口径（2026-10-05）：「页面上看不到是那个账号的日志啊」。
    ///
    /// 这一条钉的就是它，而且是**端到端**那一段：sink 往 tracing 打的字段
    /// （`mcloud_app::trace_event` ✓）→ 环里的文本（`RingLayer` ✓）→
    /// 页面读到的 `message`（`/api/logs` ✓）。
    ///
    /// 为什么放在这个 crate：这里已经有一套"真装 subscriber + 读环"的现成装置 ✓，
    /// 而这条要证的正是**两层的接缝** —— 只测 `RunLabels` 的增删（`mcloud-app` 里那几条 ✓）
    /// 抓不到"字段有没有真的落到环的文本里"✗。
    #[test]
    fn every_line_of_a_run_carries_its_account() {
        use mcloud_app::{trace_event, RunLabels};
        use mcloud_core::{LogLevel, RunEvent, RunId, TaskOutcome};
        use tracing_subscriber::layer::SubscriberExt;
        let _guard = serial();
        reset();
        let labels = RunLabels::default();
        let sub = tracing_subscriber::registry().with(RingLayer);
        let log = |run: u64, msg: &str| RunEvent::Log {
            run_id: RunId(run),
            level: LogLevel::Warn,
            kind: Default::default(),
            message: msg.into(),
            at_ms: 0,
        };
        tracing::subscriber::with_default(sub, || {
            trace_event(
                &labels,
                &RunEvent::Started {
                    run_id: RunId(1),
                    task: "app-task".into(),
                    account: "0f6050105ab111f4".into(),
                    at_ms: 0,
                    origin: None,
                },
            );
            trace_event(&labels, &log(1, "分享文件领好礼1/7：1/7 → 1/7"));
            // 没 Started 过的 run：宁可**不带**账号，也不能安一个错的主人 ✗
            trace_event(&labels, &log(2, "来路不明的一行"));
            trace_event(
                &labels,
                &RunEvent::Finished {
                    run_id: RunId(1),
                    outcome: TaskOutcome::success("跑完了"),
                    duration_ms: 5,
                },
            );
        });

        let lines = recent(&filt(10));
        let find = |needle: &str| {
            lines
                .iter()
                .find(|l| l.message.contains(needle))
                .unwrap_or_else(|| panic!("环里没有含 {needle:?} 的那一行：{lines:?}"))
        };
        // ① 关键那条：运行**中间**的日志行（截图里那类 WARN）也要带账号 ✓
        let mid = find("分享文件领好礼1/7");
        assert!(
            mid.message.contains("account=\"0f6050105ab111f4\""),
            "运行中间的日志行没带账号：{:?}",
            mid.message
        );
        // ② 结束那条也要（实现里是"先打这一行、再忘掉"，反了就丢 ✗）
        let end = find("任务结束");
        assert!(
            end.message.contains("account=\"0f6050105ab111f4\""),
            "「任务结束」没带账号：{:?}",
            end.message
        );
        // ③ 没见过的 run 不许被安上别人的账号 ✓
        let orphan = find("来路不明的一行");
        assert!(
            !orphan.message.contains("account"),
            "来路不明的 run 被安了账号：{:?}",
            orphan.message
        );
        // ④ 运行结束后映射要收干净（长跑进程里只增不减就是慢泄漏 ✗）
        assert_eq!(labels.len(), 0, "结束后 RunLabels 没清干净");
    }
}

#[cfg(test)]
mod category_tests {
    use super::*;

    fn filt(limit: usize) -> Filter<'static> {
        Filter::new(limit)
    }

    /// ⭐⭐ 2026-10-06 用户口径「这种应该分为运维日志」：**判据是"谁打的"**。
    ///
    /// 任务日志只有一条出口（`mcloud_app::run_labels::trace_event` ✓），
    /// 其余一律算运维日志（**失败关闭**：认不出就只给管理员看 ✓）。
    ///
    /// 红测：把 `category_of` 改成恒返回 `CATEGORY_TASK` ⇒ 本用例红
    /// （`mcloud_server::auth_gate` 那种实例日志会被当成任务日志 ⇒ 漏给普通用户 ✗）。
    #[test]
    fn only_the_run_log_target_is_a_task_log() {
        assert_eq!(
            category_of(mcloud_app::run_labels::LOG_TARGET),
            CATEGORY_TASK,
            "run_labels 打的就是任务日志"
        );
        for t in [
            "mcloud_server::auth_gate",
            "mcloud_server::routes::auth",
            "mcloud_engine::scheduler",
            // ⚠ 上游 http 那层**也**带 account 字段，但它是实例日志 ⇒ 必须是 ops
            "mcloud_upstream::http_log",
            "mcloud_app::notify",
            // 没见过的一律兜底成 ops ✓
            "某个将来的模块",
        ] {
            assert_eq!(category_of(t), CATEGORY_OPS, "{t} 该算运维日志");
        }
    }

    /// ⭐ 任务日志的行真的会被判成 `task` —— 用**真**的 `trace_event` 走一遍
    /// （不是手搓一个 `ServerLog` ⇒ 这样"判据"和"出口"是绑在一起验的 ✓）。
    ///
    /// ⚠ 这条同时是**耦合的钉子**：`LOG_TARGET` 跟着模块走 ⇒ 模块改名时
    /// 本用例仍然绿（它跟着常量 ✓），但若有人把任务日志改到**别处**去发，
    /// 本用例会红 ✓。
    #[test]
    fn trace_event_lines_land_in_the_task_category_with_a_structured_account() {
        let _guard = serial();
        reset();
        use tracing_subscriber::layer::SubscriberExt;

        let labels = mcloud_app::RunLabels::default();
        let run_id = mcloud_core::RunId::new(7);
        // ⚠ 必须**真装一个 subscriber**：`RingLayer` 只在 `main()` 里装过，
        // 测试进程里 `tracing::info!` 默认哪儿都不去 ⇒ 不装的话环一直是空的 ✗
        //（第一版就是这么红的：`got.last().expect("该记下一行")` panic ✓）。
        // 用 `with_default` 而不是 `init()`：它是**线程局部**的 ⇒ 不与并行用例抢
        // "全局唯一 subscriber" 那个位置 ✓（环本身仍靠 `serial()` 串起来 ✓）。
        let sub = tracing_subscriber::registry().with(RingLayer);
        tracing::subscriber::with_default(sub, || {
            mcloud_app::trace_event(
                &labels,
                &mcloud_core::RunEvent::Started {
                    run_id,
                    task: "app-task".into(),
                    account: "0f6050105ab111f4".into(),
                    at_ms: 0,
                    origin: None,
                },
            );
        });

        let got = recent(&filt(10));
        let line = got.last().expect("该记下一行");
        assert_eq!(line.category, CATEGORY_TASK, "trace_event 打的是任务日志");
        assert_eq!(
            line.account.as_deref(),
            Some("0f6050105ab111f4"),
            "账号那一格要**结构化**地留一份（没有引号）"
        );
        // ⚠ 正文里那一格照旧（一个字都不动 ✓）—— 前端与既有用例都还在读它
        assert!(
            line.message.contains("account=\"0f6050105ab111f4\""),
            "正文那份不能被改动：{}",
            line.message
        );
    }

    /// ⭐ 按账号过滤：**只对任务日志有意义**，而"没有账号的行"在给了账号集合时
    /// 会被滤掉（失败关闭 —— 归属不明就不给 ✓）。
    #[test]
    fn filtering_by_account_drops_lines_without_an_account() {
        let _guard = serial();
        reset();
        let mine: std::collections::HashSet<String> = ["acc-a".to_string()].into_iter().collect();

        // ① 带账号的任务日志（三条：acc-a 两条、acc-b 一条）
        for (acc, msg) in [
            ("acc-a", "我的第一条"),
            ("acc-a", "我的第二条"),
            ("acc-b", "别人的"),
        ] {
            push(ServerLog {
                level: "info".into(),
                target: mcloud_app::run_labels::LOG_TARGET.into(),
                message: format!("{msg} account=\"{acc}\""),
                at_ms: 1,
                category: CATEGORY_TASK.into(),
                account: Some(acc.into()),
                seq: next_seq(),
            });
        }
        // ② 没有账号的任务日志（进程重启后 `labels` 空掉时就是这个形状）
        push(ServerLog {
            level: "info".into(),
            target: mcloud_app::run_labels::LOG_TARGET.into(),
            message: "任务结束：成功".into(),
            at_ms: 1,
            category: CATEGORY_TASK.into(),
            account: None,
            seq: next_seq(),
        });
        // ③ 运维日志（实例自己的）
        push(ServerLog {
            level: "warn".into(),
            target: "mcloud_server::auth_gate".into(),
            message: "拒绝形状可疑的请求路径 path=//api/accounts".into(),
            at_ms: 1,
            category: CATEGORY_OPS.into(),
            account: None,
            seq: next_seq(),
        });

        // 不按账号过滤 ⇒ 五条都在
        assert_eq!(recent(&filt(50)).len(), 5);

        // 只看我的任务日志 ⇒ 两条（acc-b 与"没有账号"那两条都不给 ✓）
        let mine_only = recent(&Filter {
            category: Some(CATEGORY_TASK),
            accounts: Some(&mine),
            ..filt(50)
        });
        assert_eq!(mine_only.len(), 2, "实际：{mine_only:#?}");
        assert!(mine_only
            .iter()
            .all(|l| l.account.as_deref() == Some("acc-a")));

        // 只看运维日志 ⇒ 那条 WARN
        let ops = recent(&Filter {
            category: Some(CATEGORY_OPS),
            ..filt(50)
        });
        assert_eq!(ops.len(), 1);
        assert!(ops[0].message.contains("拒绝形状可疑"));
    }

    /// ⭐ 空账号集合 ≠ 不过滤：`Some(空集)` 是"这个人一个账号都没有" ⇒ 一条都不给 ✓。
    #[test]
    fn an_empty_account_set_yields_nothing_instead_of_everything() {
        let _guard = serial();
        reset();
        push(ServerLog {
            level: "info".into(),
            target: mcloud_app::run_labels::LOG_TARGET.into(),
            message: "谁的任务 account=\"acc-a\"".into(),
            at_ms: 1,
            category: CATEGORY_TASK.into(),
            account: Some("acc-a".into()),
            seq: next_seq(),
        });
        let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
        let got = recent(&Filter {
            accounts: Some(&empty),
            ..filt(50)
        });
        assert!(got.is_empty(), "空集合不该等于'不过滤'：{got:#?}");
    }
}

/// ⭐⭐ 2026-10-09：**服务端日志的"本地 + 可配 + 分页"**那一组。
///
/// 用户口径：「日志和历史不要全加入内存，使用本地记录，让用户设置最大大小」
/// ＋「大日志做虚拟加载或者分页，避免一次加载太多」✓。
///
/// ⚠ 单独一个 mod 但**共用**同一把 [`serial`] 锁（`super::serial` ✓）——
/// 两个模块各立一把锁等于没锁（2026-10-06 已经踩过一次 ✓）。
#[cfg(test)]
mod persistence_tests {
    use super::*;
    use std::path::PathBuf;

    /// 本用例独占的临时目录（用完删 ✓）。
    ///
    /// ⚠ 必须**每例一个**：`page` 会读目录下**所有** `.jsonl`（那是它的语义 ✓）
    /// ⇒ 共用目录的话，别的用例写进去的行会串进这一页 ✗。
    fn tmpdir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "mcloud-logbuf-{}-{tag}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("建临时目录");
        p
    }

    /// 与 `tests::entry` 同形状（那边是私有 ✓）。
    ///
    /// ⚠ 没把它提到公共 `testkit`：那要同时改 `tests` 与 `category_tests` 两个
    /// 已有模块（每个模块都自带一份同名函数 ✓）⇒ 改三处换一处复用，不划算 ✓。
    fn entry(level: &str, target: &str, message: &str) -> ServerLog {
        ServerLog {
            level: level.into(),
            target: target.into(),
            message: message.into(),
            at_ms: 1_700_000_000_000,
            category: category_of(target).to_string(),
            account: None,
            seq: next_seq(),
        }
    }

    fn filt(limit: usize) -> Filter<'static> {
        Filter::new(limit)
    }

    fn settings_with(mem: usize, mb: u64) -> mcloud_core::AppSettings {
        mcloud_core::AppSettings {
            server_log_mem_lines: mem,
            server_log_total_mb: mb,
            ..mcloud_core::AppSettings::default()
        }
    }

    fn msg_of(v: &[ServerLog]) -> Vec<String> {
        v.iter().map(|l| l.message.clone()).collect()
    }

    /// ⭐ 落盘：写进去的行**从磁盘读得回来**（内存环清空也读得到 ✓）——
    /// 这正是"重启后还能看"的等价物 ✓。
    #[test]
    fn lines_land_on_disk_and_are_readable_with_an_empty_ring() {
        let _guard = serial();
        detach();
        reset();
        let dir = tmpdir("disk");
        init(&dir, &settings_with(200, 64));

        for i in 0..3 {
            push(entry("info", "t", &format!("第 {i} 行")));
        }
        flush_now();
        // 把内存环清掉：此后只能从盘上读 ✓
        reset();

        let got = page(&filt(50), None);
        assert_eq!(
            msg_of(&got),
            vec!["第 0 行", "第 1 行", "第 2 行"],
            "内存清空后必须还能从磁盘读回来（这就是「本地记录」的意思）"
        );
        // 真的落到 `server-logs/` 里了（不是别处 ✓）
        let files: Vec<_> = std::fs::read_dir(dir.join(LOG_DIR_NAME))
            .expect("目录该存在")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files.len(), 1, "一天一个文件：{files:?}");
        assert!(files[0].ends_with(".jsonl"), "{files:?}");

        detach();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐ 分页：`before` 往上翻，且**盘上那一路也认筛选条件** ✓。
    #[test]
    fn paging_walks_backwards_by_cursor() {
        let _guard = serial();
        detach();
        reset();
        let dir = tmpdir("page");
        init(&dir, &settings_with(200, 64));

        for i in 1..=12 {
            push(entry("info", "t", &format!("第 {i} 行")));
        }
        flush_now();
        reset(); // 只留盘上那份 ⇒ 逼着翻页走磁盘 ✓

        // 第一屏：最新 5 行 ✓
        let first = page(&filt(5), None);
        assert_eq!(
            msg_of(&first),
            vec!["第 8 行", "第 9 行", "第 10 行", "第 11 行", "第 12 行"]
        );
        // 游标 = 这一页**最上面**那一行的 seq ✓
        let cursor = first.first().expect("非空").seq;
        let second = page(&filt(5), Some(cursor));
        assert_eq!(
            msg_of(&second),
            vec!["第 3 行", "第 4 行", "第 5 行", "第 6 行", "第 7 行"]
        );
        // 到头：给不满一页 ⇒ 前端据此收起按钮 ✓
        let cursor = second.first().expect("非空").seq;
        let third = page(&filt(5), Some(cursor));
        assert_eq!(msg_of(&third), vec!["第 1 行", "第 2 行"]);

        // ⚠ 筛选在磁盘那一路也生效（"翻到第二页筛选就失效"是典型分叉 ✗）
        let warn_only = Filter {
            min_level: Some("warn"),
            ..filt(5)
        };
        assert!(
            page(&warn_only, None).is_empty(),
            "全是 info ⇒ 一条都不该给"
        );

        detach();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐ 内存是**窗口**：行数可设，改小当场生效 ✓。
    #[test]
    fn the_memory_window_is_settable_and_trims_immediately() {
        let _guard = serial();
        detach();
        reset();
        let dir = tmpdir("mem");
        init(&dir, &settings_with(200, 64));

        for i in 0..50 {
            push(entry("info", "t", &format!("第 {i} 行")));
        }
        assert_eq!(page(&filt(1000), None).len(), 50);
        // ⚠ 这里**故意不手动 flush**：`apply_settings` 自己会先落盘再 trim ✓
        // —— 它不这么做的话，下面那 50 行会变成 11 行（静默丢历史 ✗）。

        // 改成 10 行 ⇒ **内存**当场 trim 到 10（不等下一行日志 ✓）
        apply_settings(&settings_with(10, 64));
        assert_eq!(mem_lines(), 10);
        assert_eq!(
            ring().lock().unwrap_or_else(|e| e.into_inner()).len(),
            10,
            "改小要立刻生效（用户看不到变化就等于没生效 ✗）"
        );
        // ⚠ 而 `page` 照样给得满 50 行 —— 它从**磁盘**补 ✓
        //（"内存是窗口、全量在磁盘"这条口径最直接的证据 ✓）
        assert_eq!(
            page(&filt(1000), None).len(),
            50,
            "窗口小了不等于历史少了：更早的从盘上取 ✓"
        );

        detach();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐ 磁盘上限：超了删**最旧**的整天文件，**当天那个不删** ✓。
    #[test]
    fn the_disk_budget_drops_the_oldest_day_but_never_today() {
        let _guard = serial();
        detach();
        reset();
        let dir = tmpdir("budget");
        std::fs::create_dir_all(dir.join(LOG_DIR_NAME)).unwrap();
        // 造两天：一个"很久以前"的（400 KB）、一个"今天"的（400 KB）
        let old = dir.join(LOG_DIR_NAME).join("2020-01-01.jsonl");
        let today = dir
            .join(LOG_DIR_NAME)
            .join(format!("{}.jsonl", day_of(now_ms())));
        let blob = "x".repeat(400 * 1024);
        std::fs::write(&old, &blob).unwrap();
        std::fs::write(&today, &blob).unwrap();

        // 上限 1 MB ⇒ 800 KB 没超，两个都留 ✓
        init(&dir, &settings_with(200, 1));
        assert!(old.exists() && today.exists(), "没超上限不该删");

        // 上限压到最小（1 MB 就是下限 ⇒ 用假的更小值直接调 enforce ✓）
        MAX_BYTES.store(500 * 1024, Ordering::Relaxed);
        enforce_budget(&dir.join(LOG_DIR_NAME));
        assert!(!old.exists(), "超了要删**最旧**的那个");
        assert!(
            today.exists(),
            "当天那个正在写的**不能删**（等于把「现在」删掉 ✗）"
        );

        detach();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐⭐ 混基准不倒挂：盘上已有一批**大**序号（换基准前的旧行 / 时钟回拨 / 搬来的文件 ✓）
    /// ⇒ `init` 之后的新行序号必须**比它们都大** ✓。
    ///
    /// 不这么做的话，`before` 游标会"看不见"新行（新行更小 ⇒ 被过滤掉 ✓），
    /// 翻页取回同一段 ✗ —— 2026-10-09 实测踩到过（把基准从 `<< 12` 改成 `<< 8` ✓）。
    #[test]
    fn init_lifts_the_seq_baseline_above_existing_data() {
        let _guard = serial();
        detach();
        reset();
        let dir = tmpdir("baseline");
        std::fs::create_dir_all(dir.join(LOG_DIR_NAME)).unwrap();
        // 造一份"旧基准"的文件：序号远大于今天按时钟算出来的那个
        let huge = 9_000_000_000_000_000u64;
        let old = format!(
            "{{\"level\":\"info\",\"target\":\"t\",\"message\":\"旧的\",\"atMs\":1,\"category\":\"ops\",\"account\":null,\"seq\":{huge}}}\n"
        );
        std::fs::write(dir.join(LOG_DIR_NAME).join("2020-01-01.jsonl"), old).unwrap();

        init(&dir, &settings_with(200, 64));
        let fresh = next_seq();
        assert!(
            fresh > huge,
            "新行的序号必须大于盘上已有的最大值（否则翻页看不见新行 ✗）：{fresh} vs {huge}"
        );

        detach();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⭐ 游标跨重启单调（`seq` 的起点必须随"启动时刻"走 ✓）——
    /// 否则重启后 `before` 会取错页 ✓。
    #[test]
    fn seq_is_monotonic_and_above_the_boot_clock() {
        let a = next_seq();
        let b = next_seq();
        assert!(b > a, "必须严格递增");
        // ⚠ 上限也要卡：这个数会发给前端，越过 JS 的 `MAX_SAFE_INTEGER`（9.007e15）
        // 之后前端比大小会**悄悄失精度** ✗（`<< 12` 约 2039 年就越界 ✓）
        assert!(
            a < 9_007_199_254_740_991,
            "seq 必须落在 JS 的安全整数范围内：a={a}"
        );
        assert!(
            a > (now_ms() as u64) << 7,
            "起点该是 (启动毫秒 << 8) ⇒ 远大于当前的毫秒数：a={a}"
        );
    }
}
