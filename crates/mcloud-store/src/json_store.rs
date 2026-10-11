//! JSON 文件后端。
//!
//! # 并发模型（本文件最重要的部分）
//!
//! **所有数据常驻内存，用 `RwLock` 保护；写操作先改内存再写透磁盘。**
//!
//! 为什么不是"每次操作都读文件、改、写回"：
//! 那种写法在两个请求同时落盘时会**互相覆盖** —— A 读到 v1、B 也读到 v1，
//! A 写 v2、B 写 v2（丢掉 A 的改动）。这正是本项目要避免的进程内并发问题。
//! 读-改-写三步之间只要没有锁，就一定会在并发下丢更新。
//!
//! 三个刻意的取舍，都是有意为之：
//!
//! 1. **用 `std::sync::RwLock` 而不是 `tokio::sync::RwLock`。**
//!    后者要求 `.await` 持锁，于是"持锁期间做文件 IO"就变成合法的写法，
//!    而文件 IO 在异步上下文里阻塞执行器。用 std 锁 + 全程不 await，
//!    编译期就杜绝了"跨 await 持锁"。代价是锁内做的是阻塞 IO（见第 2 点）。
//!
//! 2. **锁内做阻塞文件 IO。** 这些文件都在 KB 量级（账号、定时、编排），
//!    一次写盘是微秒级。为它引入 `spawn_blocking` 会把简单问题复杂化，
//!    收益还不如它带来的调度开销。**日志是例外**，见 [`JsonStore::append_logs`] ——
//!    单 run 上限 2000 条（约 311KB），实测一次写盘 0.19ms；
//!    即使取编译期断言允许的极限 10000 条（约 1.5MB）也只有 0.66ms。
//!    调用点是 2 秒一次的攒批（`main.rs`），不是每条日志一次。
//!    这个例外成立的前提已写进 `docs/conventions.md` §6。
//!
//! 3. **写盘是"写临时文件 + `fsync` + rename + `fsync` 目录"**（[`JsonStore::write_atomic`]）。
//!    rename 在同一文件系统内是原子的，所以进程被杀也不会留下半截 JSON；
//!    两道 `fsync` 保证"**原子**"同时是"**耐久**"（否则掉电会读回一个空文件）。
//!    覆盖前留一份 `.bak`（只从**非空**的旧文件留），但**不**试图自动恢复 ——
//!    静默回滚到旧数据比报错更难排查。读到一个**空文件**时**报错**而不是当空库：
//!    那多半是上一次写坏的残骸，静默按空库启动等于把账号/会话全丢了还一声不响。
//!
//! # 只保证**进程内**并发安全
//!
//! 两个进程同时打开同一个数据目录会互相覆盖。这不是缺陷而是范围声明：
//! 本工具是单实例的。要防多进程得靠文件锁，而那是另一个量级的复杂度。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use mcloud_core::{
    legacy_owner, slice_page, visible_to, Account, AccountId, AppSettings, AppendOutcome, Error,
    LogLine, LogPage, LogPageQuery, LogStats, Pipeline, PipelineId, Result, RunId, RunRecord,
    RunStatus, Schedule, ScheduleId, Seq, Session, User, UserId,
};

/// 运行历史保留条数的**默认值**（= `mcloud_core::DEFAULT_HISTORY_LIMIT` ✓）。
///
/// ⚠ 2026-10-09 起真正生效的是**设置里那一格**（`AppSettings::history_limit` ✓，
/// 用户口径「让用户设置最大大小」）—— 这个常量只是它的默认值与编译期依据 ✓。
pub const HISTORY_LIMIT: usize = mcloud_core::DEFAULT_HISTORY_LIMIT;

/// 内存里最多缓存**几个 run** 的日志。
///
/// # 为什么要有它（用户口径 2026-10-09）
///
/// 「日志和历史不要全加入内存，使用本地记录」✓ —— 以前 `load_all` 把
/// `data/logs/` 下**每个** run 的文件都读进内存（按 200 个文件 × 2000 行算，
/// 最坏 ≈48 MB ✗，实测现网 139 文件 / 556 KB ✓）。
///
/// 现在磁盘是**唯一真源**，内存只放"最近碰过"的那几个 ✓：
/// 正在跑的那次运行（每 2 秒追加一次 ✓）＋ 用户刚点开看的那几次 ✓。
/// 4 个够覆盖这两条路 ✓ —— 每个 run 至多 `log_limit_per_run` 行
/// （默认 2000 ⇒ 4 个 ≈ 1 MB ✓）。
pub const LOG_CACHE_RUNS: usize = 4;

/// 每个 owner 记住几台"见过的登录设备"。
///
/// # 为什么要有上限
///
/// 判据串里有 peer IP ✓ ⇒ **每换一个网络就是一条新记录**（家里 / 公司 / 手机热点 ✓），
/// 没有上限就是一份只涨不跌的文件 ✗。
///
/// # 挤掉老记录是**刻意**的
///
/// 超出之后丢最旧的 ⇒ 半年前那台设备下次再来会被判成"新设备" ✓ —— 这**不是**
/// 丢数据，而是这条口径本来就要的语义：「**最近没见过的设备**进来了，值得看一眼」✓
/// （与"有史以来第一次"相比，前者才是用户真正想被叫醒的那件事 ✓）。
pub const LOGIN_DEVICES_PER_OWNER: usize = 20;

/// 运行日志的**有界内存缓存**。
///
/// # 定位（2026-10-09 用户口径）
///
/// 「日志和历史不要全加入内存，使用本地记录」✓ ⇒ **磁盘是唯一真源**
/// （`data/logs/<id>.json` ✓），这里只是"最近碰过的那几个 run"的加速层 ✓：
/// 丢缓存不丢数据 —— 下一次访问会从盘上重新读回来 ✓。
///
/// 容量见 [`LOG_CACHE_RUNS`] ✓。淘汰按**最近使用**（LRU ✓）：正在跑的那次运行
/// 每 2 秒追加一次，它永远在最热那一头 ✓；用户点开看过、之后不再碰的，
/// 会被后来者挤出去 ✓。
#[derive(Debug, Default)]
struct LogCache {
    /// `run_id → 行`（按 `seq` 升序 ✓）。
    lines: HashMap<RunId, Vec<LogLine>>,
    /// 最近使用顺序：**尾 = 刚用过**，头 = 最久没用 ✓。
    lru: VecDeque<RunId>,
}

impl LogCache {
    /// 记一次使用（已存在才动顺序 ✓ —— 不凭空建条目 ✗）。
    fn touch(&mut self, run_id: RunId) {
        if !self.lines.contains_key(&run_id) {
            return;
        }
        self.lru.retain(|id| *id != run_id);
        self.lru.push_back(run_id);
    }

    /// 放进来（并标记为刚用过 ✓），随后按上限淘汰 ✓。
    fn insert(&mut self, run_id: RunId, lines: Vec<LogLine>) {
        self.lines.insert(run_id, lines);
        self.lru.retain(|id| *id != run_id);
        self.lru.push_back(run_id);
        while self.lru.len() > LOG_CACHE_RUNS {
            if let Some(old) = self.lru.pop_front() {
                self.lines.remove(&old);
            }
        }
    }

    fn get(&mut self, run_id: RunId) -> Option<&Vec<LogLine>> {
        self.touch(run_id);
        self.lines.get(&run_id)
    }

    fn remove(&mut self, run_id: RunId) {
        self.lines.remove(&run_id);
        self.lru.retain(|id| *id != run_id);
    }

    /// 缓存里有几个 run（诊断用 ✓）。
    fn len(&self) -> usize {
        self.lines.len()
    }
}

/// 内存中的数据。
///
/// ⚠ 2026-10-09 起**不再**是"全量"：运行日志只在 [`LogCache`] 里留最近几个 run ✓
/// （其余按需读盘 ✓）。别处（账号、定时、编排、设置）仍是全量 —— 那些是 KB 量级 ✓。
///
/// 不直接暴露：所有访问都经 [`JsonStore`] 的加锁方法，
/// 避免有人拿到 `&mut` 绕过持久化。
#[derive(Debug, Default)]
struct Db {
    accounts: Vec<Account>,
    /// 运行记录，**最新的在前**。
    runs: VecDeque<RunRecord>,
    /// 运行日志的**内存缓存**（不是全量 ✗ —— 见 [`LogCache`]）。
    logs: LogCache,
    schedules: Vec<Schedule>,
    pipelines: Vec<Pipeline>,
    /// 账号运行期状态（deviceId、jwtToken 缓存等），按账号 id 存。
    state: HashMap<String, serde_json::Value>,
    /// 账号绑定的设备指纹（smid），按账号 id 存。
    ///
    /// **刻意与 `state` 分开存、分开落盘**：`state` 那条路是整块覆盖写入
    /// （`save_account_state` 直接替换整个 `Value`），登录路径一旦顺手走它，
    /// 就会把设备指纹一起抹掉 —— 而"换凭据不该换设备长相"正是本表要保证的。
    /// 分开之后两边的写互不可见，`SmidStore` 的三条窄写才有意义。
    smid: HashMap<String, StoredSmid>,
    /// 系统用户（WebUI 登录闸）。
    users: Vec<User>,
    /// 应用级设置（口令下限、会话时长）。
    ///
    /// `Db: Default` 靠 [`AppSettings`] 自己的 `Default` —— 于是"老实例没有
    /// settings.json"与"新实例刚建目录"两种情况走的是同一条路（默认值）。
    settings: AppSettings,
    /// 每个任务的**默认参数**，按任务名存。
    ///
    /// 与 `settings` 分开落盘：设置是"这个实例怎么跑"，而这里每一格属于某一个任务 ——
    /// 混在一份文件里，写参数就会顺带把设置整体重写一遍。
    task_config: HashMap<String, serde_json::Value>,
    /// 直播口令池。**实例级**（不是按账号/按用户）—— 口令是全场共享的资源。
    kouling_pool: mcloud_core::KoulingPool,
    /// 兑换页的订阅：用户钉在顶部的那几件奖品。
    ///
    /// 与 `settings` / `task_config` 同一类（服务端的一小块用户偏好），
    /// 但**单独落盘**：改订阅不该顺带重写设置或任务参数 ✓。
    ///
    /// ⚠ 键是 **owner id（`UserId` 的字符串形态）** —— 订阅是"我想看哪几件"，
    /// 属于某个人 ✓，与全场共享的 `kouling_pool` 正好相反 ✓。
    exchange_subscriptions: HashMap<String, Vec<mcloud_core::ExchangeSubscription>>,
    /// 推送规则（用户口径 2026-10-07 的第二层，见 `mcloud_core::push`）。
    ///
    /// ⚠ 键是 **owner id** —— 规则是"我想什么情况下被叫醒"，
    /// 属于某个人 ✓（与全场共享的 `kouling_pool` 相反 ✓）。
    ///
    /// ⚠ 2026-10-08 推翻重做：**形状变了**（规则自带 `config` ✓，见
    /// `mcloud_core::PushRule`）⇒ 旧文件启动时先删 ✓（`purge_legacy_push_files`）。
    push_rules: HashMap<String, Vec<mcloud_core::PushRule>>,
    /// **面板行为**的绑定（键 = owner id ✓，2026-10-08 新增）。
    ///
    /// 登录 / 性能这类事件没有定时可绑 ⇒ 用户在设置里单独给它们选规则 ✓
    /// （`data/push-panel.json` ✓）。与 `push_rules` 同一层：都是"某个人的选择" ✓。
    push_panel: HashMap<String, mcloud_core::PanelBindings>,
    /// **面板行为的节流状态**（键 = owner id ✓，2026-10-08 新增）。
    ///
    /// ⚠ 它是**运行时状态**而不是配置（上次发送时刻 / 今日计数 / 上次状态 ✓），
    /// 但**照样要落盘**：只放内存 ⇒ 重启即忘记冷却与计数 ⇒ 重启后同一批事件
    /// 再来一轮风暴 ✗（用户口径：「登录，性能，这些可能**频繁触发**，注意」✓）。
    push_throttle: HashMap<String, mcloud_core::ThrottleState>,
    /// **每个用户自己那份**「凭据自动刷新的预设钟点」。
    ///
    /// ⚠ 键是 **owner id**，值是 `HH:MM`（东八区墙钟 ✓）—— 用户口径
    /// （2026-10-11）：「**要每用户一个钟点**」✓。与 `push_rules` / `push_panel`
    /// 同一层：都是"某个人的选择" ✓，落盘 `data/auth-refresh.json` ✓。
    ///
    /// # ⚠ 没有这一格 ≠ 用默认值 —— 是回落到**实例级那一格**
    ///
    /// 升级之前钟点是**实例级**的（`AppSettings::auth_refresh_at` ✓，那个键**留着** ✓）。
    /// 所以"这个用户从没自己设过"时读到的是**实例级那一格**，不是硬编码的 `03:00`
    /// —— 判据只有一处：[`mcloud_core::auth_refresh::effective_at`] ✓。
    /// 在这里就地兜一个默认值的话，"用户改过的钟点凭空回到 03:00"就会从这个口子漏出来 ✗。
    ///
    /// # 为什么值是裸字符串、不是一个小结构体
    ///
    /// 它**就一格**（`HH:MM`），而那个串的合法形状已经由 `parse_clock` 钉死 ✓
    /// ⇒ 包一层 `{"at": …}` 只会给每个调用点添一次解包 ✓
    ///（`login_devices: HashMap<String, Vec<String>>` 同款：值不是结构体也照存 ✓）。
    auth_refresh: HashMap<String, String>,
    /// **见过的登录设备**（键 = owner id ✓，2026-10-08 新增）。
    ///
    /// # 它是什么、不是什么
    ///
    /// 值是"这台设备/这个地点"的判据串（`UA|peer IP` ✓，见
    /// `mcloud-server` 的 `routes/auth.rs::login_device_key` ✓）—— 本仓**没有**
    /// 真设备这个概念 ✗，所以只能拿请求里有的东西当标识 ✓。
    /// ⚠ 它**不是**"有史以来见过的集合"，而是"最近见过的"（有上限 ✓）：
    /// 被挤掉的老设备下次再来会被当成新的 ✓ —— 那是**刻意**的（见
    /// [`LOGIN_DEVICES_PER_OWNER`] ✓），不是丢数据 ✓。
    login_devices: HashMap<String, Vec<String>>,
    /// 会话，按 `token_hash` 存。
    ///
    /// 用 `HashMap` 而不是 `Vec`：查会话是**每个请求**都走的路径。
    sessions: HashMap<String, Session>,
}

/// 建目录，并在 unix 上把权限收成 `0700`。
///
/// 目录用 `0700` 而不是 `0600`：目录必须可**进入**（execute 位）才能读到
/// 里面的文件，抹掉 x 位会让它自己也读不了。
fn create_dir_private(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(Error::Io)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).map_err(Error::Io)?;
        Ok(())
    }
}

/// 磁盘上的文件清单。集中在一处，改名时不会漏。
mod files {
    pub const ACCOUNTS: &str = "accounts.json";
    pub const RUNS: &str = "history.json";
    pub const SCHEDULES: &str = "schedules.json";
    pub const PIPELINES: &str = "pipelines.json";
    pub const STATE: &str = "state.json";
    pub const USERS: &str = "users.json";
    pub const SESSIONS: &str = "sessions.json";
    /// 应用级设置（口令下限、会话时长）。**后来才有的文件** —— 缺了按默认值走。
    pub const SETTINGS: &str = "settings.json";
    /// 每个任务的默认参数（任务页「配置」存的那份）。
    pub const TASK_CONFIG: &str = "task-config.json";
    /// 全实例共享的直播口令池。
    pub const KOULING_POOL: &str = "kouling-pool.json";
    /// 兑换页的订阅（用户钉在顶部的那几件奖品）。
    ///
    /// **后来才有的文件** —— 缺了就是"还没订阅过"，按空列表走 ✓（与 `settings.json` 同一条路）。
    pub const EXCHANGE_SUBSCRIPTIONS: &str = "exchange-subscriptions.json";
    /// 推送规则（按 owner 分表）。
    ///
    /// **后来才有的文件** —— 缺了就是"还没建过规则"，按空列表走 ✓
    /// （与 `exchange-subscriptions.json` / `settings.json` 同一条路）。
    ///
    /// ⚠ **文件名没变、形状变了**（2026-10-08 推翻重做 ✓）：`PushRule` 现在
    /// 自带一份 `config`（凭据在里面 ✓）、没有 `events`/`channels` 了 ✓ ⇒
    /// 旧文件读进来会**整份报错**、把启动带崩 ✗ ⇒ 启动时**先删掉** ✓
    /// （`purge_legacy_push_files` ✓，用户口径：「目前一切都可以重写，
    /// 并没有分发 rc4 版本」✓）。
    pub const PUSH_RULES: &str = "push-rules.json";
    /// **面板行为**的绑定（按 owner 分表 ✓，2026-10-08 新增）。
    ///
    /// ⚠ 里面是"哪个行为用哪几条规则 + 节流参数" ⇒ 进 `harden_existing_permissions`
    /// 的清单 ✓（规则 id 本身不是凭据，但它指向的规则带着 token ✓，
    /// 而且与其余用户数据同一条口径：老目录升级后不该对同机其他用户可读 ✓）。
    pub const PUSH_PANEL: &str = "push-panel.json";
    /// **面板行为的节流状态**（按 owner 分表 ✓，2026-10-08 新增）。
    ///
    /// ⚠ 它装的是"谁在什么时候被叫醒过"（含**被节流拦下**的计数 ✓）——
    /// 同机其他用户读得到它就能看出这个账号的活动节奏 ⇒ 一样进权限清单 ✓。
    pub const PUSH_THROTTLE: &str = "push-throttle.json";
    /// **见过的登录设备**（按 owner 分表 ✓，2026-10-08 新增）。
    ///
    /// ⚠ 里面是"谁从哪台设备/哪个地点登进来过"（`UA|peer IP` ✓）——
    /// 同机其他用户读得到它，就能看出这个账号从哪些网络活动过 ⇒
    /// 与 `push-throttle.json` 同一条口径，**进 `harden_existing_permissions`** ✓。
    ///
    /// **后来才有的文件** —— 缺了就是"还没见过任何设备"，按空表走 ✓
    /// （与 `exchange-subscriptions.json` / `push-panel.json` 同一条路 ✓）。
    pub const LOGIN_DEVICES: &str = "login-devices.json";
    /// **每个用户自己那份**「凭据自动刷新的预设钟点」（按 owner 分表 ✓，2026-10-11 新增）。
    ///
    /// ⚠ 值就是 `HH:MM`（东八区墙钟 ✓）—— 里面**没有凭据**，但**照样进
    /// `harden_existing_permissions` 的清单** ✓：它记着"这个账号每天几点会去轮换凭据"，
    /// 与 `push-throttle.json` / `login-devices.json` 同一条口径 ——
    /// 老目录升级后不该对同机其他用户可读 ✓
    ///（漏掉 = 那个文件仍对同机其他用户可读 ✗）。
    ///
    /// **后来才有的文件** —— 缺了就是"还没有人自己设过"，按空表走 ✓
    ///（与 `push-panel.json` / `login-devices.json` 同一条路 ✓）；
    /// 那时读到的钟点回落到实例级那一格 ✓（见 `Db::auth_refresh` 的说明 ✓）。
    pub const AUTH_REFRESH: &str = "auth-refresh.json";
    /// **已废弃**：用户级推送渠道配置（`push-config.json`）。
    ///
    /// 只留在这里给 [`JsonStore::purge_legacy_push_files`] 当删除清单用 ✓ ——
    /// 它**不再是任何表的落点** ✗：用户口径是「我设置一个推送规则，**这个规则配置
    /// 推送的 token，包括推送哪几个平台**」✓ ⇒ 凭据在规则自己身上 ✓，
    /// "用户级渠道配置"那一层整个不存在了 ✓。
    pub const LEGACY_PUSH_CONFIG: &str = "push-config.json";
    /// 设备指纹：**单独一个文件**，不与 `state.json` 混。
    ///
    /// 混在一起的话，`save_account_state` 那种整块覆盖写会顺手把指纹清掉；
    /// 而这条路的写入方（登录/刷新）根本不知道指纹的存在。
    pub const SMID: &str = "smid.json";
    /// 机器级 PC 设备指纹（`windowMachine`）。
    ///
    /// # 为什么是**纯文本**而不是 JSON
    ///
    /// 它只有一个值。JSON 包一层不会让它更可读，反而让"文件被截断"
    /// 这件事从"值不对"变成"解析失败"——前者至少还能看出是个半截 id。
    ///
    /// # 为什么单独一个文件
    ///
    /// 它是**机器级**登记值，与按账号存的 `state.json` / `smid.json`
    /// 不是一回事。⚠ 它今天也**不再**参与出码或登录：二维码的 `dID` 由出码时
    /// 现生成的那一枚担任，四条登录路径按账号取/生成；这个文件只有
    /// `GET /api/login/device` 与 rotate 读写（见 `mcloud_core::pc_device` 模块头）。
    pub const PC_DEVICE: &str = "pc_device.txt";
    /// 日志目录：每次运行一个文件，避免为了追加一行重写全部日志。
    pub const LOG_DIR: &str = "logs";
}

/// `smid.json` 里一条记录的落盘形态。
///
/// 与 `mcloud_core::SmidState` 分开定义：那个类型刻意不派生 `Serialize`
/// （少一条"顺手塞进 HTTP 响应"的路），而落盘需要序列化。
/// 两边的字段含义一一对应，转换是显式的。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredSmid {
    /// 绑定的设备特征（JSON 文本）。`None` = 还没绑过。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fingerprint_json: Option<String>,
    /// 最近一次上报换回的 `deviceId`（含 `B` 前缀）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
}

impl From<StoredSmid> for mcloud_core::SmidState {
    fn from(s: StoredSmid) -> Self {
        Self {
            fingerprint_json: s.fingerprint_json,
            device_id: s.device_id,
        }
    }
}

/// 基于目录的 JSON 存储。
///
/// 手写 `Debug` 而不是 `derive`：`RwLock<Db>` 与 `Mutex<()>` 都不实现 `Debug`，
/// 而把所有数据都打印出来既没用又会泄漏凭证。只报目录与记录数。
pub struct JsonStore {
    dir: PathBuf,
    db: RwLock<Db>,
    /// 串行化写盘。
    ///
    /// 为什么在 `RwLock` 之外还要一把写锁：写盘是"改内存（持写锁）+ 落盘"，
    /// 若只靠 `RwLock`，两个写操作会串行地各自完成，没问题；
    /// 但这把锁让**落盘顺序**与**内存修改顺序**一致，
    /// 崩溃时磁盘上的内容才是某个真实的中间态，而不是交错的结果。
    write_gate: Mutex<()>,
}

impl JsonStore {
    /// 打开（必要时创建）数据目录，并把已有数据读进内存。
    ///
    /// 启动时做两件事，都在 [`Self::recover_orphans`] 里说明。
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        create_dir_private(&dir)?;
        create_dir_private(&dir.join(files::LOG_DIR))?;

        let store = Self {
            db: RwLock::new(Db::default()),
            write_gate: Mutex::new(()),
            dir,
        };
        // ⚠ **先删旧形状的推送文件**（`load_all` 之前 ✓）：`PushRule` 的形状这一版
        // 变了（多了必填的 `config` ✓）⇒ 旧 `push-rules.json` 读进来会整份报错、
        // 把启动带崩 ✗。用户口径：「目前一切都可以重写，并没有分发 rc4 版本」✓。
        store.purge_legacy_push_files();
        store.harden_existing_permissions();
        store.load_all()?;
        // 归属回填：老数据（没有 owner 列）必须在这里补上，否则升级后
        // 会表现成「账号全不见了」。放在 `recover_orphans` 之前只是让
        // 「数据先完整、再收尾」这条阅读顺序成立 —— 两者改的是不同的表。
        store.backfill_owners()?;
        store.recover_orphans()?;
        Ok(store)
    }

    /// 删掉**旧形状**的推送落盘文件（2026-10-08 推翻重做 ✓）。
    ///
    /// # 为什么是"直接删"而不是"迁移"
    ///
    /// 用户口径：「目前一切都可以重写，**并没有分发 rc4 版本**」✓ ⇒ 没有历史用户
    /// 需要照顾，迁移代码只会留下一份"两种形状都能读"的复杂度 ✗。
    ///
    /// 而**非删不可**的理由更硬：
    /// * `push-rules.json`：`PushRule` 的形状变了（多了必填的 `config` ✓、
    ///   少了 `events`/`channels` ✓）⇒ 旧文件读进来**整份报错** ⇒ 启动失败 ✗；
    /// * `push-config.json`：用户级渠道配置那一层**整个不存在了** ✗ ⇒ 留着它
    ///   只会让人以为"渠道配置还在某处"，而且里面是 token ✓ ⇒ **无条件删** ✓。
    ///
    /// # ⚠ `push-rules.json` 是"**旧形状才删**"，不是"每次启动都删"
    ///
    /// 用户口径里的「启动时先删掉这个文件 —— 它现在是旧形状」说的是**这次升级**
    /// 要清掉那份旧文件 ✓。写成"每次启动无条件删"就把规则表变成**不持久**的 ✗
    /// （写进去、重启就没了 ⇒ 规则页配完等于白配 ✓）—— 那是与"按 owner 分表
    /// 落盘"直接矛盾的一件事 ✗。所以判据落在**形状**上：
    ///
    /// * 合法 JSON 但**读不出新形状** ⇒ 是旧形状 ⇒ 删（连 `.bak` 一起 ✓）；
    /// * **压根不是合法 JSON** ⇒ **不删**：那是"写坏了的文件"，
    ///   本仓的既有口径是**大声报错**而不是静默当空库 ✓（见模块头第 3 条 ✓）
    ///   —— 删掉它等于把用户的规则悄悄扔了 ✓。
    ///
    /// ⚠ 只删这两个（外加各自的 `.bak` ✓）—— 别顺手删别的：
    /// 这一层跑在启动路径上，删错一个就是"用户数据不见了" ✗。
    ///
    /// 失败**不**让启动失败（与 [`Self::harden_existing_permissions`] 同一口径 ✓）：
    /// 删不掉（权限/占用）最多是"旧文件还躺在目录里"，而它已经不会被读了 ✓。
    fn purge_legacy_push_files(&self) {
        let rules = self.path(files::PUSH_RULES);
        if let Ok(text) = std::fs::read_to_string(&rules) {
            // 是合法 JSON，但**读不出新形状** ⇒ 旧形状 ⇒ 删 ✓。
            let is_json = serde_json::from_str::<serde_json::Value>(&text).is_ok();
            let is_new_shape =
                serde_json::from_str::<HashMap<String, Vec<mcloud_core::PushRule>>>(&text).is_ok();
            if is_json && !is_new_shape {
                self.remove_with_backup(files::PUSH_RULES);
            }
        }
        // 用户级渠道配置那一层已经不存在了 ⇒ 无条件删 ✓（它连"读"都没有了 ✓）。
        self.remove_with_backup(files::LEGACY_PUSH_CONFIG);
    }

    /// 删一个数据文件及其 `.bak`（`write_atomic` 覆盖前留的那份 ✓）。删不掉只警告 ✓。
    fn remove_with_backup(&self, name: &str) {
        let p = self.path(name);
        for path in [p.clone(), p.with_extension("bak")] {
            if !path.exists() {
                continue;
            }
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("警告：无法删除旧推送文件 {}：{e}", path.display());
            }
        }
    }

    /// 收紧**已存在**文件的权限（老版本装出来的数据目录是默认权限）。
    ///
    /// 新写的文件在 [`Self::write_atomic`] 里就已经是 `0600`，这里只处理
    /// 历史遗留。只收紧、不放松，所以重复执行无副作用。
    ///
    /// 清单覆盖**所有会落盘的表**，尤其是装凭据与会话的那几个：
    /// `accounts.json`（`AuthInfo.raw` 凭据原文）/ `users.json`（口令哈希）/
    /// `sessions.json`（WebUI 会话，拿到就能冒充登录）/ `smid.json`（账号设备指纹）/
    /// `pc_device.txt`（机器指纹），再加上运行数据（历史 / 定时 / 编排 / 状态）。
    /// 漏掉其中一个 = 老目录升级后那个文件仍对同机其他用户可读。
    ///
    /// 失败**不**让启动失败：可能是文件系统不支持 mode（如某些网络盘、
    /// Windows 挂载），为此拒绝启动会让工具彻底不可用。但也不静默 ——
    /// 权限没收紧这件事必须留下痕迹。
    #[cfg(unix)]
    fn harden_existing_permissions(&self) {
        use std::os::unix::fs::PermissionsExt as _;
        let names = [
            files::ACCOUNTS,
            files::RUNS,
            files::SCHEDULES,
            files::PIPELINES,
            files::STATE,
            // ⚠ 下面四个装的正是凭据与会话，曾经漏在清单外；不要再删。
            files::USERS,
            files::SESSIONS,
            files::SMID,
            files::PC_DEVICE,
            // ⚠ 2026-10-04 复审 co-3 补的四个：上面那句"覆盖**所有会落盘的表**"
            // 在此之前是不成立的 ✗ —— `settings.json` 里有登录口令哈希与安全设置、
            // `task-config.json` / `kouling-pool.json` 里是账号相关的口令与参数、
            // `exchange-subscriptions.json` 是账号的兑换订阅。它们与其余表一样
            // 是**用户数据**，老目录升级后不该仍对同机其他用户可读。
            files::SETTINGS,
            files::TASK_CONFIG,
            files::KOULING_POOL,
            files::EXCHANGE_SUBSCRIPTIONS,
            // 推送规则里是"什么情况下被叫醒"，同样是用户数据 ✓。
            files::PUSH_RULES,
            // ⚠ 2026-10-08 推翻重做时补的两个：面板绑定（指向带 token 的规则 ✓）
            // 与节流状态（谁在什么时候被叫醒过 ✓）。**两个新文件必须都在清单里** ——
            // 漏掉 = 老目录升级后那两个文件仍对同机其他用户可读 ✗
            // （`push-config.json` 那一格随文件一起删掉了 ✓，它已经不再是落点 ✓）。
            files::PUSH_PANEL,
            files::PUSH_THROTTLE,
            // ⚠ 2026-10-08 接登录触发线时补的第三个：它记着"这个账号从哪台设备/
            // 哪个地点登进来过"（`UA|peer IP` ✓）。同机其他用户读得到它，就能看出
            // 这个账号的活动网络 ⇒ 与节流状态同一条口径，**必须也在清单里** ✓
            // （漏掉 = 老目录升级后这个文件仍对同机其他用户可读 ✗）。
            files::LOGIN_DEVICES,
            // ⚠ 2026-10-11 把钟点改成**每用户一份**时补的第五个：它记着
            // "这个账号每天几点会去轮换凭据" ⇒ 与上面那个同一条口径，
            // **必须也在清单里** ✓（漏掉 = 老目录升级后这个文件仍对同机其他用户可读 ✗）。
            files::AUTH_REFRESH,
        ];
        for name in names {
            let p = self.path(name);
            if !p.exists() {
                continue;
            }
            // 只处理"组或其他用户可读"的情况，避免无谓的 chmod
            match std::fs::metadata(&p) {
                Ok(m) if m.permissions().mode() & 0o077 != 0 => {
                    if let Err(e) =
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))
                    {
                        eprintln!("警告：无法收紧 {} 的权限：{e}", p.display());
                    }
                }
                _ => {}
            }
        }
    }

    #[cfg(not(unix))]
    fn harden_existing_permissions(&self) {}

    // ── 加锁 ────────────────────────────────────────────────────────────

    /// 读锁。中毒说明有别的线程在持写锁时 panic 了 —— 数据可能不一致，
    /// 这时候继续跑比直接报错更危险。
    fn read(&self) -> Result<RwLockReadGuard<'_, Db>> {
        self.db
            .read()
            .map_err(|_| Error::Storage("读锁中毒：先前的写操作发生 panic".into()))
    }

    fn write(&self) -> Result<RwLockWriteGuard<'_, Db>> {
        self.db
            .write()
            .map_err(|_| Error::Storage("写锁中毒：先前的写操作发生 panic".into()))
    }

    // ── 路径与原子写 ────────────────────────────────────────────────────

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn log_path(&self, run_id: RunId) -> PathBuf {
        self.dir
            .join(files::LOG_DIR)
            .join(format!("{}.json", run_id.get()))
    }

    /// 原子**且耐久**写：临时文件（0600）→ `fsync` → rename → `fsync` 目录，
    /// 覆盖前留 `.bak`。
    ///
    /// rename 在同一文件系统内原子 ⇒ 进程被杀不会留下半截 JSON。
    /// 临时文件名带 pid，避免同目录下两个实例互踩（虽然本工具单实例，
    /// 但代价只是一点点，能免掉一整类诡异现象）。
    ///
    /// ⚠ **"原子"不等于"落盘"**：rename 改的是内存里的目录项，掉电时新内容
    /// 可能还在页缓存里 ⇒ 重新读回来是**空文件**（或旧文件）。所以这里有两道
    /// `fsync`：① 临时文件在 rename **之前**刷盘（否则 rename 成功而数据没落盘，
    /// 得到一个"名字对、内容空"的文件）；② 目录项在 rename **之后**刷盘
    /// （否则"改名"这一步本身可能丢）。见 2026-10-04 复审 `co-1`。
    ///
    /// # 权限
    ///
    /// `accounts.json` 里有凭证原文（`AuthInfo.raw`），所以文件落盘时就是
    /// `0600`。**必须在创建时就限制**，不能先按默认权限（通常 0644）写、
    /// 再 `chmod` 收紧 —— 那中间有一小段窗口，同机器上的其他用户
    /// 正好能读到凭证。所以这里用 `OpenOptions::mode` 而不是事后
    /// `set_permissions`。
    ///
    /// 只在 unix 上做这件事：Windows 没有等价的 mode 位，其 ACL 模型
    /// 由用户目录继承，硬套 `0600` 反而会写出奇怪的东西。
    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        self.write_atomic_opts(path, bytes, true)
    }

    /// 同上，但**不留 `.bak`** —— 给运行日志用（`data/logs/<id>.json` ✓）。
    ///
    /// # 为什么日志不留备份
    ///
    /// ① 它**可再生**：日志是观测数据，丢了不影响任何业务结果 ✓（而账号/会话丢了
    /// 就是真丢 ✓）；
    /// ② 它**每 2 秒被重写一次**（`EventHub::flush` 攒批 ✓）⇒ 留 `.bak` 等于
    /// 让日志目录**凭空翻倍**、还要占用户设的容量上限 ✗
    /// （2026-10-09 实测现网：139 个文件里有 30 个是 `.bak` ✓）。
    fn write_json_log<T: Serialize>(&self, path: &Path, v: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(v)?;
        self.write_atomic_opts(path, &bytes, false)
    }

    /// `backup` = 覆盖前是否留一份 `.bak`（见上面两条的说明 ✓）。
    fn write_atomic_opts(&self, path: &Path, bytes: &[u8], backup: bool) -> Result<()> {
        // ⚠ 备份只从**非空**的旧文件来。0 字节只可能是上一次半截写入的残骸
        // （见 `read_json`），拿它 `copy` 过去等于把唯一一份好 `.bak` 也毁掉
        // —— 那正是"掉电后连备份一起失效"的原因（2026-10-04 复审 co-1）。
        if backup {
            if let Ok(old) = std::fs::read(path) {
                if !old.is_empty() {
                    let _ = std::fs::copy(path, path.with_extension("bak"));
                }
            }
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));

        #[cfg(unix)]
        let mut f = {
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(Error::Io)?
        };
        #[cfg(not(unix))]
        let mut f = std::fs::File::create(&tmp).map_err(Error::Io)?;

        {
            use std::io::Write as _;
            f.write_all(bytes).map_err(Error::Io)?;
            // 必须在 rename 之前：否则 rename 会把一个"还没落盘的空壳"顶上去
            f.sync_all().map_err(Error::Io)?;
        }
        drop(f);

        std::fs::rename(&tmp, path).map_err(Error::Io)?;

        // 目录项也要落盘，否则 rename 自身可能丢（掉电后回到旧文件/空文件）。
        // Windows 打不开目录句柄，那里不做（也没有等价的语义）。
        //
        // ⚠ 这一道的失败**只警告、不致命**：内容已经 `fsync` 过，最坏情况是崩溃后
        // 退回**上一个完整版本**（旧文件原样还在）—— 那不是数据丢失；
        // 而把它做成致命错误，会让"文件系统不支持目录 fsync"的环境（某些网络盘/FUSE）
        // 连账号都存不下去。上面那道**文件**的 `sync_all` 则相反：它失败意味着
        // 内容没落盘，正是"空文件"的来源 ⇒ 必须致命。
        #[cfg(unix)]
        {
            let dir = path.parent().unwrap_or_else(|| Path::new("."));
            if let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all()) {
                eprintln!("警告：无法 fsync 目录 {}：{e}", dir.display());
            }
        }
        Ok(())
    }

    fn write_json<T: Serialize>(&self, path: &Path, v: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(v)?;
        self.write_atomic(path, &bytes)
    }

    /// 读一个 JSON 文件。
    ///
    /// * 文件**不存在** ⇒ 默认值（首次启动是正常路径）；
    /// * 文件存在但**是空的**（0 字节 / 只有空白）⇒ **报错**，不按默认值处理。
    ///
    /// ⚠ 后者是 2026-10-04 复审 `co-1` 改的：0 字节只可能来自"上一次写入被截断
    /// 或掉电"，把它读成"空库"会让账号、会话、口令池**静默消失**
    /// （表现是"账号全不见了"，而日志里一个字都没有）。报错则会在启动时停下，
    /// 并给出恢复路径：删掉它（按空库启动）或从同名 `.bak` 取回。
    fn read_json<T: for<'de> Deserialize<'de> + Default>(&self, path: &Path) -> Result<T> {
        if !path.exists() {
            return Ok(T::default());
        }
        let s = std::fs::read_to_string(path).map_err(Error::Io)?;
        if s.trim().is_empty() {
            return Err(Error::Storage(format!(
                "{} 是空文件：上一次写入没有完整落盘（掉电/被杀）。\
                 这里不按空库处理，免得静默丢数据 —— 确认后删掉它，\
                 或从 {} 恢复",
                path.display(),
                path.with_extension("bak").display()
            )));
        }
        serde_json::from_str(&s).map_err(|e| {
            // 解析失败时带上文件名与片段：只报 "expected value at line 1"
            // 等于让人去猜是哪个文件坏了
            Error::Storage(format!(
                "{} 解析失败：{e}；文件前 200 字符：{}",
                path.display(),
                s.chars().take(200).collect::<String>()
            ))
        })
    }

    // ── 归属 ────────────────────────────────────────────────────────────

    /// 实例**唯一**的用户（只有一个用户时才有值）。
    ///
    /// 用于「未归属数据」的可见性判定，规则见 `mcloud_core::owner::visible_to`：
    /// 单用户时无主数据对该用户可见（不存在「别人的东西」），多用户时谁都看不见
    /// （失败关闭）。
    fn sole_user(db: &Db) -> Option<&UserId> {
        if db.users.len() == 1 {
            db.users.first().map(|u| &u.id)
        } else {
            None
        }
    }

    /// 某个账号 id 的归属。
    ///
    /// 返回 `None` = **库里没有这个账号**；`Some(None)` = 「账号在、但未归属」。
    /// 这两者必须分得开：前者用于「账号被删了」的运行记录归属兜底，
    /// 后者用于单用户可见规则。
    fn account_owner(db: &Db, account_id: &str) -> Option<Option<UserId>> {
        db.accounts
            .iter()
            .find(|a| a.id.as_str() == account_id)
            .map(|a| a.owner.clone())
    }

    /// 一条运行记录对 viewer 是否可见。
    ///
    /// 运行记录**自己没有 owner 列**：它的归属由 `RunRecord.account` 指向的
    /// 那个账号决定。账号已不在库里（被删）时按「未归属」处理 —— 多用户实例里
    /// 它就谁都看不见了，这是刻意的失败方向（宁可看不到，也不误给）。
    fn run_visible(db: &Db, viewer: &UserId, run: &RunRecord) -> bool {
        let sole = Self::sole_user(db);
        match Self::account_owner(db, &run.account) {
            Some(owner) => visible_to(viewer, owner.as_ref(), sole),
            None => visible_to(viewer, None, sole),
        }
    }

    /// 把**老数据的归属**回填给最初的管理员。
    ///
    /// # 为什么必须在加载时做
    ///
    /// 老数据（本功能之前建的）的 JSON 里没有 owner 列。不回填的话，按
    /// `visible_to` 的规则它们只对「实例唯一的用户」可见 —— 而**老库升级**
    /// 正是最常见的场景，绝不能表现成「升级后账号全不见了」。
    /// 选谁当 owner 见 `mcloud_core::owner::legacy_owner`（创建最早的那个，
    /// 也就是最初的管理员）。
    ///
    /// # 为什么要把回填结果写回磁盘
    ///
    /// 只改内存的话，归属会随 `users.json` 的变化而漂移（例如又加了一个用户，
    /// 或用户被删）。「归属漂移」是这类功能最难查的形态，所以一次性钉死。
    /// 这里在启动阶段调用，还没有并发写者，因此直接用 `write_json`（与
    /// `recover_orphans` 同样的取舍）。
    fn backfill_owners(&self) -> Result<()> {
        let (accounts, schedules, pipelines, touched_a, touched_s, touched_p) = {
            let mut db = self.write()?;
            let Some(legacy) = legacy_owner(&db.users) else {
                // 没有用户 ⇒ 没有可归属的对象；数据保持未归属，
                // 由 `visible_to` 的单用户规则处置。
                return Ok(());
            };
            let mut touched_a = false;
            let mut touched_s = false;
            let mut touched_p = false;
            for a in db.accounts.iter_mut() {
                if a.owner.is_none() {
                    a.owner = Some(legacy.clone());
                    touched_a = true;
                }
            }
            for s in db.schedules.iter_mut() {
                if s.owner.is_none() {
                    s.owner = Some(legacy.clone());
                    touched_s = true;
                }
            }
            for p in db.pipelines.iter_mut() {
                if p.owner.is_none() {
                    p.owner = Some(legacy.clone());
                    touched_p = true;
                }
            }
            if !touched_a && !touched_s && !touched_p {
                return Ok(());
            }
            (
                db.accounts.clone(),
                db.schedules.clone(),
                db.pipelines.clone(),
                touched_a,
                touched_s,
                touched_p,
            )
        };
        // 只在**真的动过**的表上写盘：无谓的写会给每个文件都留一份 .bak
        if touched_a {
            self.write_json(&self.path(files::ACCOUNTS), &accounts)?;
        }
        if touched_s {
            self.write_json(&self.path(files::SCHEDULES), &schedules)?;
        }
        if touched_p {
            self.write_json(&self.path(files::PIPELINES), &pipelines)?;
        }
        Ok(())
    }

    // ── 启动装载与恢复 ──────────────────────────────────────────────────

    fn load_all(&self) -> Result<()> {
        let accounts: Vec<Account> = self.read_json(&self.path(files::ACCOUNTS))?;
        let runs: Vec<RunRecord> = self.read_json(&self.path(files::RUNS))?;
        let schedules: Vec<Schedule> = self.read_json(&self.path(files::SCHEDULES))?;
        let pipelines: Vec<Pipeline> = self.read_json(&self.path(files::PIPELINES))?;
        let state: HashMap<String, serde_json::Value> = self.read_json(&self.path(files::STATE))?;
        let smid: HashMap<String, StoredSmid> = self.read_json(&self.path(files::SMID))?;
        let users: Vec<User> = self.read_json(&self.path(files::USERS))?;
        let settings: AppSettings = self.read_json(&self.path(files::SETTINGS))?;
        let task_config: HashMap<String, serde_json::Value> =
            self.read_json(&self.path(files::TASK_CONFIG))?;
        let kouling_pool: mcloud_core::KoulingPool =
            self.read_json(&self.path(files::KOULING_POOL))?;
        let exchange_subscriptions: HashMap<String, Vec<mcloud_core::ExchangeSubscription>> =
            self.read_json(&self.path(files::EXCHANGE_SUBSCRIPTIONS))?;
        let push_rules: HashMap<String, Vec<mcloud_core::PushRule>> =
            self.read_json(&self.path(files::PUSH_RULES))?;
        let push_panel: HashMap<String, mcloud_core::PanelBindings> =
            self.read_json(&self.path(files::PUSH_PANEL))?;
        let push_throttle: HashMap<String, mcloud_core::ThrottleState> =
            self.read_json(&self.path(files::PUSH_THROTTLE))?;
        let login_devices: HashMap<String, Vec<String>> =
            self.read_json(&self.path(files::LOGIN_DEVICES))?;
        // ⚠ 老目录上这个文件**还不存在**（2026-10-11 才有 ✓）⇒ `read_json` 对缺文件
        // 回 `Default`（空表 ✓），于是"还没人自己设过钟点"这条升级路径天然成立 ✓。
        let auth_refresh: HashMap<String, String> =
            self.read_json(&self.path(files::AUTH_REFRESH))?;
        let sessions: HashMap<String, Session> = self.read_json(&self.path(files::SESSIONS))?;

        // ⚠⚠ 日志**一个文件都不读**（2026-10-09 用户口径：「日志和历史不要全加入内存，
        // 使用本地记录」✓）。磁盘是唯一真源，内存只留最近碰过的那几个 run
        // （`LogCache` ✓），其余按需读盘（`ensure_logs_loaded` ✓）。
        //
        // 以前这里会 `for r in &runs { read_json(log_path) }` —— 200 个文件
        // 全量进内存，最坏 ≈48 MB ✗（现网实测 139 文件 / 556 KB ✓，量级差异见
        // `LOG_CACHE_RUNS` 的说明 ✓）。

        let mut db = self.write()?;
        db.accounts = accounts;
        db.runs = runs.into();
        db.schedules = schedules;
        db.pipelines = pipelines;
        db.state = state;
        db.smid = smid;
        db.users = users;
        db.settings = settings;
        db.task_config = task_config;
        db.kouling_pool = kouling_pool;
        db.exchange_subscriptions = exchange_subscriptions;
        db.push_rules = push_rules;
        db.push_panel = push_panel;
        db.push_throttle = push_throttle;
        db.login_devices = login_devices;
        db.auth_refresh = auth_refresh;
        db.sessions = sessions;
        db.logs = LogCache::default();
        // 老 `history.json` 里那份**内联日志**搬到日志文件里（一次性 ✓）：
        // 那份形状从这一版起不再落盘 ⇒ 不搬就等于把升级前的回看日志丢了 ✗。
        let cleared = self.migrate_inline_logs(&mut db.runs)?;
        // 盘上那份可能是在**没有**容量上限之前写下的（历史遗留）⇒ 读回来先收一次口，
        // 免得"启动就把几万行读进内存"这件事重演 ✗（下一次写盘自然是限过的形状 ✓）。
        self.enforce_log_budget(&mut db)?;
        if cleared > 0 {
            // ⚠ 搬完**立刻把盘上那份也写小**（2026-10-09 ✓）：不写的话那份内联日志会
            // **一直躺在盘上**（实测 0.5 MB ✓），而且每次重启都再"迁移"一遍
            //（幂等，但白做 ✓）—— 用户口径里的"不要全加入内存"看起来就没发生 ✓。
            let runs: Vec<RunRecord> = db.runs.iter().cloned().collect();
            drop(db);
            let _gate = self
                .write_gate
                .lock()
                .map_err(|_| Error::Storage("写锁中毒".into()))?;
            self.write_json(&self.path(files::RUNS), &runs)?;
        }
        Ok(())
    }

    /// 把老 `history.json` 里**内联**的日志搬进 `data/logs/<id>.json`（一次性迁移 ✓）。
    ///
    /// # 为什么必须搬
    ///
    /// 2026-10-09 起 `RunRecord.logs` 不再落盘（见它的文档 ✓），写盘前会被清空 ✓
    /// ⇒ 升级前写下的那份内联日志如果只是"读进来、然后丢掉"，用户的回看日志
    /// 就**静默消失**了 ✗（正是台账 184 行专门补上的那个功能 ✓）。
    ///
    /// # 两条判据
    ///
    /// * 日志文件**已经有** ⇒ 什么都不做，只把内联那份清掉 ✓（文件是更全的那一份：
    ///   2000 行 vs 200 行 ✓，而且带 `seq` ✓）；
    /// * 日志文件**没有** ⇒ 把内联那些写成一个日志文件 ✓（`seq` 按顺序现编
    ///   —— 内联那份本来就不带序号 ✓，顺序即真相 ✓）。
    ///
    /// 迁移失败（写盘错）**只警告、不拦启动**：这是"尽力保住旧日志"，
    /// 拦下启动等于"为了 200 行旧日志让整个服务起不来" ✗（那 200 行本来就快被清了 ✓）。
    ///
    /// 返回值 = **有多少条记录的内联日志被清掉**（不管有没有真写出文件 ✓）——
    /// 调用方据此决定"要不要顺手把盘上那份重写小" ✓。
    fn migrate_inline_logs(&self, runs: &mut VecDeque<RunRecord>) -> Result<usize> {
        let mut moved = 0usize;
        let mut cleared = 0usize;
        for r in runs.iter_mut() {
            if r.logs.is_empty() {
                continue;
            }
            let path = self.log_path(r.run_id);
            if !path.exists() {
                let lines: Vec<LogLine> = r
                    .logs
                    .iter()
                    .enumerate()
                    .map(|(i, l)| LogLine {
                        run_id: r.run_id.get(),
                        level: l.level,
                        kind: l.kind,
                        message: l.message.clone(),
                        at_ms: l.at_ms,
                        seq: Seq::seen(i as u64 + 1),
                    })
                    .collect();
                if let Err(e) = self.write_json_log(&path, &lines) {
                    eprintln!(
                        "警告：运行 {} 的内联日志迁移失败（这 {n} 行会丢）：{e}",
                        r.run_id,
                        n = r.logs.len()
                    );
                    continue;
                }
                moved += 1;
            }
            // 内存里也不再留着它 ✓（磁盘那份才是真源 ✓）
            r.logs = Vec::new();
            cleared += 1;
        }
        if moved > 0 {
            eprintln!("已把 {moved} 条老记录的运行日志迁移到 data/logs/（它们以前内联在 history.json 里）");
        }
        Ok(cleared)
    }

    /// 日志目录超预算 ⇒ 按**最旧**删文件，直到回到预算内 ✓。
    ///
    /// # 判据
    ///
    /// 预算 = `AppSettings::log_total_mb`（字节形态 ✓，用户可设 ✓）。
    /// 排序按 **mtime 升序**（`run_id` 当并列时的次序 —— 编号单调递增 ✓）：
    /// 删的是"最久没被写过"的那些 ✓，而**正在跑**的那次运行总是最新的
    /// ⇒ 它排在最后、只有在"预算连一次运行都装不下"时才会被删 ✓
    /// （那时删掉的也只是文件，内存缓存与后续追加会把它重新写出来 ✓）。
    ///
    /// ⚠ `.bak` 一起删 ✓（它照样占磁盘 ✓）；运行记录**不动** ✓
    /// —— 用户仍然看得到"跑过什么、结果如何"，只是回看日志没了 ✓
    /// （与老 `strip_logs_beyond_budget` 的语义一致，只是判据从"行数"换成了"字节" ✓）。
    ///
    /// ⚠ 目录扫一遍是 100 多个 `stat`（微秒级 ✓），每次追加日志后跑一次可以接受 ✓
    /// —— 它换来的是"日志真的封得住"这件事 ✓。
    fn enforce_log_budget(&self, db: &mut Db) -> Result<()> {
        let max = db.settings.log_total_bytes();
        let dir = self.dir.join(files::LOG_DIR);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(()); // 目录还没建（第一次启动）⇒ 没有超预算这回事
        };
        let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = Vec::new();
        let mut total: u64 = 0;
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let bytes = meta.len();
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            total = total.saturating_add(bytes);
            files.push((mtime, e.path(), bytes));
        }
        if total <= max {
            return Ok(());
        }
        // 最旧的先走 ✓（时间相同则按文件名，保证顺序稳定、不会每拍删不同的 ✓）
        files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut removed = 0usize;
        for (_, path, bytes) in files {
            if total <= max {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(bytes);
                removed += 1;
                // 缓存里也清掉 ✓ —— 否则"盘上删了、内存还留着"，下次读会给出
                // 一份盘上已经不存在的日志 ✗（用户会以为它没被清 ✓）
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(id) = stem.parse::<u64>() {
                        db.logs.remove(RunId::new(id));
                    }
                }
            }
        }
        if removed > 0 {
            tracing::warn!(
                removed,
                limit_bytes = max,
                "日志目录超出上限，已按最旧的删掉 {removed} 个文件（上限可在设置里调大）"
            );
        }
        Ok(())
    }

    /// 把上次进程留下的"运行中"记录标记为**已中止**。
    ///
    /// 进程被杀时来不及回填状态，于是磁盘上会留下永远 `running` 的记录。
    /// 不处理的话前端会一直显示"进行中"，而那个任务其实早就不存在了 ——
    /// 用户等一个永远不会来的结果，或者以为服务卡住了。
    ///
    /// 标记为 `Cancelled` 而不是 `Failed`：它不是任务失败，是进程没了。
    fn recover_orphans(&self) -> Result<()> {
        let mut db = self.write()?;
        let mut changed = false;
        for r in db.runs.iter_mut() {
            if r.status == RunStatus::Running {
                r.status = RunStatus::Cancelled;
                r.outcome = mcloud_core::TaskOutcome::skipped("进程退出，运行未完成");
                changed = true;
            }
        }
        if changed {
            let runs: Vec<RunRecord> = db.runs.iter().cloned().collect();
            drop(db); // 落盘前先放锁，避免持锁做 IO 的时间被拉长
            self.write_json(&self.path(files::RUNS), &runs)?;
        }
        Ok(())
    }

    // ── 内部持久化（调用方必须已持有写锁）────────────────────────────────

    fn persist_accounts(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::ACCOUNTS), &db.accounts)
    }

    fn persist_runs(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        // ⚠⚠ 写盘前把 `logs` 清空（2026-10-09 用户口径：「日志和历史不要全加入内存，
        // 使用本地记录」✓）。这一格只是**接口形状**：`GET /api/runs/{id}` 会从日志文件
        // 现取最近 200 行填进来 ✓（见 `RunRecord::logs` 的文档 ✓）。
        // 日志的家是 `data/logs/<id>.json` ✓ —— 一份数据只存一处 ✓。
        let runs: Vec<RunRecord> = db
            .runs
            .iter()
            .map(|r| {
                let mut c = r.clone();
                c.logs = Vec::new();
                c
            })
            .collect();
        store.write_json(&store.path(files::RUNS), &runs)
    }

    fn persist_schedules(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::SCHEDULES), &db.schedules)
    }

    fn persist_pipelines(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::PIPELINES), &db.pipelines)
    }

    fn persist_users(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::USERS), &db.users)
    }

    fn persist_settings(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::SETTINGS), &db.settings)
    }

    fn persist_kouling_pool(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::KOULING_POOL), &db.kouling_pool)
    }

    fn persist_exchange_subscriptions(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(
            &store.path(files::EXCHANGE_SUBSCRIPTIONS),
            &db.exchange_subscriptions,
        )
    }

    fn persist_push_rules(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::PUSH_RULES), &db.push_rules)
    }

    fn persist_push_panel(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::PUSH_PANEL), &db.push_panel)
    }

    fn persist_push_throttle(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::PUSH_THROTTLE), &db.push_throttle)
    }

    fn persist_login_devices(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::LOGIN_DEVICES), &db.login_devices)
    }

    fn persist_auth_refresh(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::AUTH_REFRESH), &db.auth_refresh)
    }

    fn persist_task_config(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::TASK_CONFIG), &db.task_config)
    }

    fn persist_sessions(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::SESSIONS), &db.sessions)
    }

    fn persist_state(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::STATE), &db.state)
    }

    fn persist_smid(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::SMID), &db.smid)
    }
    /// 淘汰超出上限的运行记录，并**连同其日志文件一起删除**。
    ///
    /// ⚠ 上限来自**设置**（`AppSettings::history_limit` ✓，用户可设 ✓），
    /// 默认 = [`HISTORY_LIMIT`] ✓。
    fn evict_runs(db: &mut Db, store: &Self) {
        let limit = db.settings.history_limit;
        while db.runs.len() > limit {
            if let Some(old) = db.runs.pop_back() {
                db.logs.remove(old.run_id);
                let p = store.log_path(old.run_id);
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::remove_file(p.with_extension("bak"));
            }
        }
    }

    /// 内存里缓存了几个 run 的日志（**诊断与用例用** ✓）。
    ///
    /// 它存在的理由很具体：用户口径「日志和历史不要全加入内存」需要一个
    /// **可断言**的读数 —— 否则"启动时不再全量读日志""缓存有界"这两件事
    /// 一条用例都钉不住 ✗。
    #[cfg(test)]
    pub(crate) fn cached_log_runs(&self) -> usize {
        self.read().map(|db| db.logs.len()).unwrap_or(0)
    }

    /// 取某个 run 的日志行（**缓存优先，未命中读盘** ✓）。
    ///
    /// # 为什么读盘要在这里做（而不是启动时全读）
    ///
    /// 用户口径 2026-10-09：「日志和历史不要全加入内存，使用本地记录」✓。
    /// 读进来就进 [`LogCache`]（有界 ✓）—— 下一次同一条 run 的访问不必再读盘 ✓，
    /// 而缓存满了会挤掉最久没用的那个 ✓（磁盘那份还在，随时能再读 ✓）。
    ///
    /// # 锁内做文件 IO
    ///
    /// 与本文件头第 2 点同一条口径：日志文件在 KB 量级（实测均 119 B/行 ✓，
    /// 默认上限 2000 行 ≈ 240 KB ✓），一次读盘是微秒级 ✓；
    /// 而"读盘 + 放进缓存"必须是**一次原子动作**，否则两个请求会各读一遍、
    /// 后写的覆盖先写的 ✓（代价只是白读一次，不值得为它拆锁 ✗）。
    ///
    /// ⚠ 读**坏**的文件（0 字节 / 解析失败）**报错**而不是当空 —— 与
    /// [`JsonStore::read_json`] 同一条口径：静默当空会让人以为"这次运行没日志" ✗，
    /// 而真相是"文件坏了，能去 `.bak` 或磁盘上找" ✓。
    fn ensure_logs_loaded(&self, db: &mut Db, run_id: RunId) -> Result<()> {
        if db.logs.get(run_id).is_some() {
            return Ok(());
        }
        let path = self.log_path(run_id);
        let lines: Vec<LogLine> = if path.exists() {
            self.read_json(&path)?
        } else {
            Vec::new()
        };
        db.logs.insert(run_id, lines);
        Ok(())
    }
}

impl std::fmt::Debug for JsonStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let counts = self.read().ok().map(|db| {
            (
                db.accounts.len(),
                db.runs.len(),
                db.schedules.len(),
                db.pipelines.len(),
                // 日志是**缓存**不是全量（见 `LogCache` ✓）—— 排障时"内存里留了几个 run
                // 的日志"正是要看的那一格 ✓
                db.logs.len(),
            )
        });
        f.debug_struct("JsonStore")
            .field("dir", &self.dir)
            .field("accounts/runs/schedules/pipelines/logs_cached", &counts)
            .finish()
    }
}

// ── 账号 ────────────────────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::AccountStore for JsonStore {
    async fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self.read()?.accounts.clone())
    }

    async fn account(&self, id: &AccountId) -> Result<Option<Account>> {
        Ok(self.read()?.accounts.iter().find(|a| &a.id == id).cloned())
    }

    async fn accounts_for(&self, owner: &UserId) -> Result<Vec<Account>> {
        let db = self.read()?;
        let sole = Self::sole_user(&db);
        Ok(db
            .accounts
            .iter()
            .filter(|a| visible_to(owner, a.owner.as_ref(), sole))
            .cloned()
            .collect())
    }

    async fn account_for(&self, owner: &UserId, id: &AccountId) -> Result<Option<Account>> {
        let db = self.read()?;
        let sole = Self::sole_user(&db);
        Ok(db
            .accounts
            .iter()
            .find(|a| &a.id == id && visible_to(owner, a.owner.as_ref(), sole))
            .cloned())
    }

    async fn save_account(&self, account: &Account) -> Result<()> {
        let mut db = self.write()?;
        match db.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(slot) => *slot = account.clone(),
            None => db.accounts.push(account.clone()),
        }
        Self::persist_accounts(&db, self)
    }

    async fn remove_account(&self, id: &AccountId) -> Result<()> {
        let mut db = self.write()?;
        db.accounts.retain(|a| &a.id != id);
        // 账号没了，它的运行期状态与该设备的指纹都该跟着走，
        // 否则同号再次添加会继承旧 deviceId（等于新账号套用旧设备长相）
        db.state.remove(id.as_str());
        db.smid.remove(id.as_str());
        let r = Self::persist_accounts(&db, self);
        Self::persist_state(&db, self)?;
        Self::persist_smid(&db, self)?;
        r
    }

    async fn account_state(&self, id: &AccountId) -> Result<serde_json::Value> {
        Ok(self
            .read()?
            .state
            .get(id.as_str())
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    async fn save_account_state(&self, id: &AccountId, state: &serde_json::Value) -> Result<()> {
        let mut db = self.write()?;
        db.state.insert(id.as_str().to_string(), state.clone());
        Self::persist_state(&db, self)
    }
}

// ── 机器级 PC 设备指纹 ──────────────────────────────────────────────────

/// 设备指纹文件的内容校验。
///
/// 只做**形状**校验（32 位大写 hex + `-ENDIN`），不校验"是不是这台机器"——
/// 后者无从校验。形状校验的价值在于：文件被截断/清空时能立刻发现，
/// 而不是拿一个半截 id 去出码（那张码服务端永远定位不到）。
fn parse_pc_device_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let body = trimmed.strip_suffix(mcloud_core::PC_DEVICE_SUFFIX)?;
    if body.len() != 32 || !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if body != body.to_uppercase() {
        return None;
    }
    Some(trimmed.to_string())
}

#[async_trait]
impl mcloud_core::PcDeviceStore for JsonStore {
    async fn pc_device_id(&self) -> Result<Option<String>> {
        let path = self.path(files::PC_DEVICE);
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path).map_err(Error::Io)?;
        // 空白/截断都算"没有"：把它当成已有设备号，等于每次运行都换一台设备。
        //
        // 这里**不删文件**：读不出来就返回 None，让上层生成并覆盖。
        // 顺手删掉会让"文件权限不对导致读失败"这类问题更难查。
        Ok(parse_pc_device_id(&raw))
    }

    async fn set_pc_device_id(&self, device_id: &str) -> Result<()> {
        // 只接受合法形状：写进去一个半截 id，下一次读会被判为"没有"，
        // 于是每轮都生成一个新的 —— 那正是这个文件要防的事。
        let checked = parse_pc_device_id(device_id).ok_or_else(|| {
            Error::Config(format!(
                "设备指纹形状非法（应形如 <32 位大写 HEX>-ENDIN）：{device_id}"
            ))
        })?;
        // 走既有的原子写：临时文件 → rename，且创建时就是 0600。
        self.write_atomic(
            &self.path(files::PC_DEVICE),
            format!("{checked}\n").as_bytes(),
        )
    }
}

// ── 设备指纹（smid，窄写）────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::SmidStore for JsonStore {
    async fn get_smid_state(&self, id: &AccountId) -> Result<Option<mcloud_core::SmidState>> {
        // `None` 与"空状态"必须可区分：前者要走"生成一份并落库"，
        // 后者（绑过指纹但 deviceId 丢了）不该重新生成长相。
        Ok(self.read()?.smid.get(id.as_str()).cloned().map(Into::into))
    }

    async fn set_smid_fingerprint(&self, id: &AccountId, fingerprint_json: &str) -> Result<()> {
        let mut db = self.write()?;
        // 换指纹与清旧 deviceId 是**同一个动作**：中间崩溃会留下
        // "新长相 + 旧 id"，而那正是风控最想看到的设备漂移信号。
        db.smid.insert(
            id.as_str().to_string(),
            StoredSmid {
                fingerprint_json: Some(fingerprint_json.to_string()),
                device_id: None,
            },
        );
        Self::persist_smid(&db, self)
    }

    async fn set_smid_device_id(&self, id: &AccountId, device_id: &str) -> Result<()> {
        let mut db = self.write()?;
        // 窄写：只动 device_id，**不碰**指纹。
        let entry = db.smid.entry(id.as_str().to_string()).or_default();
        entry.device_id = Some(device_id.to_string());
        Self::persist_smid(&db, self)
    }
}

// ── 运行记录 ────────────────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::RunStore for JsonStore {
    async fn upsert_run(&self, record: &RunRecord) -> Result<()> {
        let mut db = self.write()?;
        // ⚠ 进来就把内联日志丢掉（2026-10-09 口径 ✓）：日志的家是
        // `data/logs/<id>.json` ✓，`RunRecord.logs` 只是**接口形状**、
        // 由 `GET /api/runs/{id}` 现取现填 ✓。
        // 在这里清掉 ⇒ **内存与磁盘两边都不背它** ✓
        //（写盘那侧还有 `persist_runs` 兜一道 ✓，两条路都不漏 ✓）。
        let mut record = record.clone();
        record.logs = Vec::new();
        let record = &record;
        match db.runs.iter_mut().find(|r| r.run_id == record.run_id) {
            Some(slot) => {
                // 同一次运行的**更新**（起 → 完）走这里，是正常路径。
                // 但"编号相同、却是另一次运行"意味着**编号被复用** —— 那会静默丢掉
                // 一条历史（2026-10-01 的真实故障：`seed_run_id` 没人调用，
                // 重启后编号从 1 重来）。这里留一声警告，让这类事再也无法无声发生。
                if slot.started_at_ms != record.started_at_ms || slot.task != record.task {
                    tracing::warn!(
                        run = %record.run_id,
                        old_task = %slot.task,
                        new_task = %record.task,
                        old_started = slot.started_at_ms,
                        new_started = record.started_at_ms,
                        "运行编号被复用：这次要覆盖的是**另一次**运行，历史会少一条"
                    );
                }
                *slot = record.clone();
            }
            None => db.runs.push_front(record.clone()),
        }
        Self::evict_runs(&mut db, self);
        Self::persist_runs(&db, self)
    }

    async fn runs(&self, limit: usize) -> Result<Vec<RunRecord>> {
        Ok(self.read()?.runs.iter().take(limit).cloned().collect())
    }

    async fn run(&self, id: RunId) -> Result<Option<RunRecord>> {
        Ok(self.read()?.runs.iter().find(|r| r.run_id == id).cloned())
    }

    async fn runs_for(&self, owner: &UserId, limit: usize) -> Result<Vec<RunRecord>> {
        let db = self.read()?;
        Ok(db
            .runs
            .iter()
            .filter(|r| Self::run_visible(&db, owner, r))
            .take(limit)
            .cloned()
            .collect())
    }

    async fn run_for(&self, owner: &UserId, id: RunId) -> Result<Option<RunRecord>> {
        let db = self.read()?;
        Ok(db
            .runs
            .iter()
            .find(|r| r.run_id == id && Self::run_visible(&db, owner, r))
            .cloned())
    }

    async fn running_runs(&self) -> Result<Vec<RunRecord>> {
        Ok(self
            .read()?
            .runs
            .iter()
            .filter(|r| r.status == RunStatus::Running)
            .cloned()
            .collect())
    }
}

// ── 日志 ────────────────────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::LogStore for JsonStore {
    async fn append_logs(&self, run_id: RunId, lines: &[LogLine]) -> Result<AppendOutcome> {
        if lines.is_empty() {
            return Ok(AppendOutcome::Appended);
        }
        let (outcome, snapshot) = {
            let mut db = self.write()?;
            // ⚠ 缓存里没有就从**盘上**读回来（用户口径 2026-10-09：
            // 「日志和历史不要全加入内存，使用本地记录」✓）——
            // 追加是"读改写"，所以必须先拿到已有的那部分 ✓。
            self.ensure_logs_loaded(&mut db, run_id)?;
            // 上限来自**设置**（用户可设 ✓），默认 `LOG_LIMIT_PER_RUN` ✓
            let limit = db.settings.log_limit_per_run;
            let outcome = {
                let bucket = db.logs.lines.entry(run_id).or_default();
                bucket.extend_from_slice(lines);
                let mut outcome = AppendOutcome::Appended;
                if bucket.len() > limit {
                    // 丢**最旧**的：排错时最近的日志信息量最大
                    let excess = bucket.len() - limit;
                    bucket.drain(..excess);
                    outcome = AppendOutcome::Trimmed;
                }
                outcome
            };
            db.logs.touch(run_id);
            let snapshot = db.logs.lines.get(&run_id).cloned().unwrap_or_default();
            (outcome, snapshot)
        };
        // 日志每次追加都整体重写该 run 的文件。单 run 上限默认 2000 条，
        // 文件在百 KB 级，这个代价可以接受；换成"追加到文件尾"会让
        // JSON 不再是一个完整文档，读的时候还得容错解析半截内容。
        //
        // ⚠ 日志**不留 `.bak`**（见 `write_json_log` ✓）：它是可再生的观测数据，
        // 覆盖前的副本只会让日志目录凭空翻倍、还得占预算 ✗。
        {
            let _gate = self
                .write_gate
                .lock()
                .map_err(|_| Error::Storage("写锁中毒".into()))?;
            self.write_json_log(&self.log_path(run_id), &snapshot)?;
        }
        // 写盘之后才可能超预算 ⇒ 在这里收一次口 ✓
        // （锁外做：`enforce_log_budget` 要拿写锁 ✓，而上面那段已经放掉了 ✓）
        {
            let mut db = self.write()?;
            self.enforce_log_budget(&mut db)?;
        }
        Ok(outcome)
    }

    async fn logs(&self, run_id: RunId, after_seq: Option<Seq>) -> Result<Vec<LogLine>> {
        let mut db = self.write()?;
        // ⚠ 用**写锁**：缓存是"读到就塞进去"的（有界 LRU ✓），
        // 只读锁改不了缓存 —— 而每次访问都读盘就白做了这层缓存 ✗。
        // 代价：同一 run 的并发读会串行（微秒级 ✓）。
        self.ensure_logs_loaded(&mut db, run_id)?;
        let all = db.logs.lines.get(&run_id);
        Ok(match (all, after_seq) {
            (None, _) => Vec::new(),
            // 断线重连续传：只要比已收到的 seq 更新的
            (Some(v), Some(seq)) => v.iter().filter(|l| l.seq > seq).cloned().collect(),
            (Some(v), None) => v.clone(),
        })
    }

    /// 分页取（用户口径 2026-10-09：「大日志做虚拟加载或者分页，避免一次加载太多」✓）。
    ///
    /// ⚠ 切片判据只有一份（`mcloud_core::slice_page` ✓）—— 默认实现（从内存那份拿）
    /// 与这里（从盘上拿）必须是**同一条**判据，否则"内存命中"与"盘上命中"会给出
    /// 不同的页 ✗。
    async fn logs_page(&self, run_id: RunId, q: LogPageQuery) -> Result<LogPage> {
        let mut db = self.write()?;
        self.ensure_logs_loaded(&mut db, run_id)?;
        let all = db.logs.lines.get(&run_id).cloned().unwrap_or_default();
        Ok(slice_page(&all, &q))
    }

    /// 日志目录的用量读数（给「存储信息」卡与容量上限用 ✓）。
    ///
    /// 直接扫目录（`stat` 一次一个 ✓）—— 不维护"计数器"是**刻意**的：
    /// 计数器要与"删除/迁移/外部清理"三处同步，漏一处就永远对不上 ✗，
    /// 而扫 100 多个文件是微秒级 ✓。
    async fn log_stats(&self) -> Result<LogStats> {
        let dir = self.dir.join(files::LOG_DIR);
        let max_bytes = self.read()?.settings.log_total_bytes();
        let mut stats = LogStats {
            max_bytes,
            ..LogStats::default()
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(stats);
        };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            stats.bytes = stats.bytes.saturating_add(meta.len());
            // `.bak` 占磁盘但不算"日志文件"（见 `write_json_log`：日志本来就不留它 ✓）
            if e.path().extension().is_some_and(|x| x == "json") {
                stats.files += 1;
            }
        }
        Ok(stats)
    }

    async fn clear_logs(&self, run_id: RunId) -> Result<()> {
        let mut db = self.write()?;
        db.logs.remove(run_id);
        let p = self.log_path(run_id);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("bak"));
        Ok(())
    }
}

// ── 定时 ────────────────────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::ScheduleStore for JsonStore {
    async fn schedules(&self) -> Result<Vec<Schedule>> {
        Ok(self.read()?.schedules.clone())
    }

    async fn schedule(&self, id: &ScheduleId) -> Result<Option<Schedule>> {
        Ok(self.read()?.schedules.iter().find(|s| &s.id == id).cloned())
    }

    async fn schedules_for(&self, owner: &UserId) -> Result<Vec<Schedule>> {
        let db = self.read()?;
        let sole = Self::sole_user(&db);
        Ok(db
            .schedules
            .iter()
            .filter(|s| visible_to(owner, s.owner.as_ref(), sole))
            .cloned()
            .collect())
    }

    async fn schedule_for(&self, owner: &UserId, id: &ScheduleId) -> Result<Option<Schedule>> {
        let db = self.read()?;
        let sole = Self::sole_user(&db);
        Ok(db
            .schedules
            .iter()
            .find(|s| &s.id == id && visible_to(owner, s.owner.as_ref(), sole))
            .cloned())
    }

    async fn save_schedule(&self, schedule: &Schedule) -> Result<()> {
        let mut db = self.write()?;
        match db.schedules.iter_mut().find(|s| s.id == schedule.id) {
            Some(slot) => *slot = schedule.clone(),
            None => db.schedules.push(schedule.clone()),
        }
        Self::persist_schedules(&db, self)
    }

    async fn remove_schedule(&self, id: &ScheduleId) -> Result<()> {
        let mut db = self.write()?;
        db.schedules.retain(|s| &s.id != id);
        Self::persist_schedules(&db, self)
    }

    async fn mark_schedule_fired(&self, id: &ScheduleId, fire_ms: i64) -> Result<()> {
        let mut db = self.write()?;
        let found = db.schedules.iter_mut().find(|s| &s.id == id);
        match found {
            Some(sl) => {
                sl.last_fire_ms = Some(fire_ms);
                sl.run_count = sl.run_count.saturating_add(1);
            }
            // 定时在这期间被删了：不是错误，只是没得写
            None => return Ok(()),
        }
        Self::persist_schedules(&db, self)
    }

    /// 只推水位、**不**计数（`Decision::Missed` 那条弃置支走这里）。
    ///
    /// 与 `mark_schedule_fired` 的差别只有 `run_count` 那一行 ——
    /// 弃置的触发点一次上游都没打，不该算"跑过一次"（2026-10-03 review §1.7）。
    async fn advance_schedule_watermark(&self, id: &ScheduleId, fire_ms: i64) -> Result<()> {
        let mut db = self.write()?;
        match db.schedules.iter_mut().find(|s| &s.id == id) {
            Some(sl) => sl.last_fire_ms = Some(fire_ms),
            None => return Ok(()),
        }
        Self::persist_schedules(&db, self)
    }

    async fn record_schedule_run(
        &self,
        id: &ScheduleId,
        run_ms: Option<i64>,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let mut db = self.write()?;
        if let Some(sl) = db.schedules.iter_mut().find(|s| &s.id == id) {
            // None ⇒ 不动这一列；Some ⇒ 更新
            if let Some(ms) = run_ms {
                sl.last_run_ms = Some(ms);
            }
            sl.last_status = Some(status.to_string());
            // None ⇒ 显式清空（不是"不动"）
            sl.last_error = error.map(str::to_string);
        }
        Self::persist_schedules(&db, self)
    }

    /// **窄写**：只记「上次结果」那句人话说明（`Schedule::last_note` ✓）。
    ///
    /// 为什么与 `record_schedule_run` 分成两写：见 trait 那边那条说明
    ///（并进去要改二十来处假 store 的签名 ✗）。
    /// 语义与它**逐字相同**：`None` ⇒ **显式清空**（不是"不动" ✓）——
    /// 结论换了却留着上一趟的说明，就是"成功旁边挂着旧错误"那一类形态 ✓。
    async fn record_schedule_note(&self, id: &ScheduleId, note: Option<&str>) -> Result<()> {
        let mut db = self.write()?;
        if let Some(sl) = db.schedules.iter_mut().find(|s| &s.id == id) {
            sl.last_note = note.map(str::to_string);
        }
        Self::persist_schedules(&db, self)
    }
}

// ── 编排 ────────────────────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::PipelineStore for JsonStore {
    async fn pipelines(&self) -> Result<Vec<Pipeline>> {
        Ok(self.read()?.pipelines.clone())
    }

    async fn pipeline(&self, id: &PipelineId) -> Result<Option<Pipeline>> {
        Ok(self.read()?.pipelines.iter().find(|p| &p.id == id).cloned())
    }

    async fn pipelines_for(&self, owner: &UserId) -> Result<Vec<Pipeline>> {
        let db = self.read()?;
        let sole = Self::sole_user(&db);
        Ok(db
            .pipelines
            .iter()
            .filter(|p| visible_to(owner, p.owner.as_ref(), sole))
            .cloned()
            .collect())
    }

    async fn pipeline_for(&self, owner: &UserId, id: &PipelineId) -> Result<Option<Pipeline>> {
        let db = self.read()?;
        let sole = Self::sole_user(&db);
        Ok(db
            .pipelines
            .iter()
            .find(|p| &p.id == id && visible_to(owner, p.owner.as_ref(), sole))
            .cloned())
    }

    async fn save_pipeline(&self, pipeline: &Pipeline) -> Result<()> {
        let mut db = self.write()?;
        match db.pipelines.iter_mut().find(|p| p.id == pipeline.id) {
            Some(slot) => *slot = pipeline.clone(),
            None => db.pipelines.push(pipeline.clone()),
        }
        Self::persist_pipelines(&db, self)
    }

    async fn remove_pipeline(&self, id: &PipelineId) -> Result<()> {
        let mut db = self.write()?;
        db.pipelines.retain(|p| &p.id != id);
        Self::persist_pipelines(&db, self)
    }

    async fn mark_pipeline_ran(&self, id: &PipelineId, run_ms: i64) -> Result<()> {
        let mut db = self.write()?;
        match db.pipelines.iter_mut().find(|p| &p.id == id) {
            Some(sl) => sl.last_run_ms = Some(run_ms),
            // 编排在这期间被删了：不是错误，只是没得写
            None => return Ok(()),
        }
        Self::persist_pipelines(&db, self)
    }
}

// ── 系统用户与会话 ──────────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::UserStore for JsonStore {
    async fn user_count(&self) -> Result<usize> {
        Ok(self.read()?.users.len())
    }

    async fn users(&self) -> Result<Vec<User>> {
        Ok(self.read()?.users.clone())
    }

    async fn user_by_name(&self, name: &str) -> Result<Option<User>> {
        Ok(self.read()?.users.iter().find(|u| u.name == name).cloned())
    }

    async fn user(&self, id: &UserId) -> Result<Option<User>> {
        Ok(self.read()?.users.iter().find(|u| &u.id == id).cloned())
    }

    async fn save_user(&self, user: &User) -> Result<()> {
        let mut db = self.write()?;
        match db.users.iter_mut().find(|u| u.id == user.id) {
            Some(slot) => *slot = user.clone(),
            None => db.users.push(user.clone()),
        }
        Self::persist_users(&db, self)
    }

    /// 首次初始化（2026-10-06 安全审计 `SEC-09`）：**判空与插入在同一把写锁内**。
    ///
    /// ⚠ 关键就在"同一把锁"上：`user_count()` + `save_user()` 是两次独立的
    /// `read()` / `write()`，中间隔着调用方那次 argon2（几十毫秒）⇒
    /// 第二个并发请求也会看到 0 个用户，于是各建一个管理员 ✗
    /// （详见 `UserStore::create_first_user` 上的说明）。
    async fn create_first_user(&self, user: &User) -> Result<bool> {
        let mut db = self.write()?;
        if !db.users.is_empty() {
            return Ok(false);
        }
        db.users.push(user.clone());
        Self::persist_users(&db, self)?;
        Ok(true)
    }

    async fn settings(&self) -> Result<AppSettings> {
        // ⚠ `.clone()` 不能省：`AppSettings` 自 2026-10-11 起**不是 `Copy`**
        //（它多了一格 `auth_refresh_at: String`，`HH:MM` 的落盘形态 ✓）。
        // 去掉 clone 会直接编不过（`cannot move out of … RwLockReadGuard` ✓）——
        // 这里如实记一笔，免得后人以为它是"多余的一次克隆" ✗。
        Ok(self.read()?.settings.clone())
    }

    async fn kouling_pool(&self) -> Result<Box<mcloud_core::KoulingPool>> {
        Ok(Box::new(self.read()?.kouling_pool.clone()))
    }

    async fn save_kouling_pool(&self, pool: &mcloud_core::KoulingPool) -> Result<()> {
        let mut db = self.write()?;
        db.kouling_pool = pool.clone();
        Self::persist_kouling_pool(&db, self)
    }

    async fn task_params(&self, task: &str) -> Result<serde_json::Value> {
        Ok(self
            .read()?
            .task_config
            .get(task)
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())))
    }

    async fn save_task_params(&self, task: &str, params: &serde_json::Value) -> Result<()> {
        // 形状在这里也守一道：绕过 HTTP 的调用方（CLI / 脚本）同样不该塞进非对象。
        if !params.is_object() {
            return Err(Error::Config("任务参数必须是一个 JSON 对象".into()));
        }
        let mut db = self.write()?;
        db.task_config.insert(task.to_string(), params.clone());
        Self::persist_task_config(&db, self)
    }

    async fn exchange_subscriptions(
        &self,
        owner: &mcloud_core::UserId,
    ) -> Result<Vec<mcloud_core::ExchangeSubscription>> {
        // 没订阅过 ⇒ 空列表（不是错误），与 `task_params()` 同一口径 ✓。
        Ok(self
            .read()?
            .exchange_subscriptions
            .get(owner.as_str())
            .cloned()
            .unwrap_or_default())
    }

    async fn save_exchange_subscriptions(
        &self,
        owner: &mcloud_core::UserId,
        items: &[mcloud_core::ExchangeSubscription],
    ) -> Result<()> {
        // 形状在这里也守一道（与 `save_task_params` 同一理由）：绕过 HTTP 的调用方
        // （CLI / 脚本）同样不该塞进**重复的 `prize_id`** —— 顶部的"已订阅"判定与
        // 切换语义都依赖它唯一 ✓；重复会让同一件在顶部出现两行 ✗。
        let mut seen = std::collections::HashSet::new();
        for s in items {
            if !seen.insert(s.prize_id) {
                return Err(Error::Config(format!(
                    "订阅里有重复的奖品 id：{}",
                    s.prize_id
                )));
            }
        }
        // 顺序在这里也定死（按订阅时刻）：前端与服务端各算一次也得同序，
        // 否则同一次订阅在两个标签页里显示成两种顺序 ✗。见 `mcloud_core::exchange::toggled`。
        let mut ordered = items.to_vec();
        ordered.sort_by_key(|s| (s.subscribed_at_ms, s.prize_id));
        let mut db = self.write()?;
        if ordered.is_empty() {
            // 全取消掉就把这一格删掉，而不是留一个空数组：文件里少一行垃圾 ✓
            // （读回来时缺键 = 空列表，与上面 `unwrap_or_default` 对上 ✓）。
            db.exchange_subscriptions.remove(owner.as_str());
        } else {
            db.exchange_subscriptions
                .insert(owner.as_str().to_string(), ordered);
        }
        Self::persist_exchange_subscriptions(&db, self)
    }

    async fn push_panel(&self, owner: &mcloud_core::UserId) -> Result<mcloud_core::PanelBindings> {
        // 没配过 ⇒ **默认那份**（"不绑规则" + 保守节流 ✓）—— 不是错误，
        // 与 `push_rules()` 回空列表同一口径 ✓（见 `UserStore::push_panel` 的说明 ✓）。
        Ok(self
            .read()?
            .push_panel
            .get(owner.as_str())
            .cloned()
            .unwrap_or_default())
    }

    async fn save_push_panel(
        &self,
        owner: &mcloud_core::UserId,
        bindings: &mcloud_core::PanelBindings,
    ) -> Result<()> {
        let mut db = self.write()?;
        db.push_panel
            .insert(owner.as_str().to_string(), bindings.clone());
        Self::persist_push_panel(&db, self)
    }

    async fn push_throttle(
        &self,
        owner: &mcloud_core::UserId,
    ) -> Result<mcloud_core::ThrottleState> {
        // 没记过 ⇒ 全默认（`last_sent_ms: None` ⇒ 第一次一定放行 ✓）。
        Ok(self
            .read()?
            .push_throttle
            .get(owner.as_str())
            .cloned()
            .unwrap_or_default())
    }

    async fn save_push_throttle(
        &self,
        owner: &mcloud_core::UserId,
        state: &mcloud_core::ThrottleState,
    ) -> Result<()> {
        let mut db = self.write()?;
        if state.behaviors.is_empty() {
            // 全空的就**不留一格**（与 `remove_push_rule` 删光后的口径一致 ✓）：
            // 文件里少一行垃圾 ✓，读回来时缺键 = 全默认 ✓ 对得上。
            if db.push_throttle.remove(owner.as_str()).is_none() {
                return Ok(()); // 本来就没有这一格 ⇒ 不必重写整份文件 ✓
            }
        } else {
            db.push_throttle
                .insert(owner.as_str().to_string(), state.clone());
        }
        Self::persist_push_throttle(&db, self)
    }

    async fn auth_refresh_at(&self, owner: &mcloud_core::UserId) -> Result<Option<String>> {
        // 没设过 ⇒ `None`（**不在这里回落实例级那一格** ✓ ——
        // 回落是口径、只有一处实现，见 `UserStore::auth_refresh_at` 的说明 ✓）。
        Ok(self.read()?.auth_refresh.get(owner.as_str()).cloned())
    }

    async fn save_auth_refresh_at(&self, owner: &mcloud_core::UserId, at: &str) -> Result<()> {
        // 领域约束在这里也守一道（与 `save_settings` 里那道 `validate()` 同一条理由 ✓）：
        // 绕过 HTTP 的调用方（CLI / 脚本）同样不该把 `半夜` 写进这个文件 ——
        // 写进去之后 `daily_expr` 当场报"写不成 cron"，表现是"钟点存上了、定时却装不上" ✗。
        //
        // ⚠ 判据**只有一处**（`auth_refresh::parse_clock` ✓）—— 在这里再写一遍
        // "两段、冒号、范围"就是第二份口径，迟早与它分叉 ✗。
        if mcloud_core::auth_refresh::parse_clock(at).is_none() {
            return Err(Error::Config(format!(
                "预设刷新时间要是 24 小时制的 HH:MM（例如 03:00），收到的是「{at}」"
            )));
        }
        let mut db = self.write()?;
        // ⚠ **只动这个 owner 那一行** ✓（`insert` 按 key 覆盖 ⇒ 别人的那一行碰不到 ✓）——
        // 这正是"别人的钟点一个字节都改不了"的落点 ✓。
        db.auth_refresh
            .insert(owner.as_str().to_string(), at.trim().to_string());
        Self::persist_auth_refresh(&db, self)
    }

    async fn remember_login_device(&self, owner: &mcloud_core::UserId, key: &str) -> Result<bool> {
        let mut db = self.write()?;
        let seen = db
            .login_devices
            .entry(owner.as_str().to_string())
            .or_default();
        if seen.iter().any(|k| k == key) {
            // 见过 ⇒ **不落盘** ✓：这条路上每一次成功登录都会走一遍，
            // 每次都把整份文件重写一次是白费 ✓（判据在内存里已经有了 ✓）。
            return Ok(false);
        }
        seen.push(key.to_string());
        if seen.len() > LOGIN_DEVICES_PER_OWNER {
            // 超出就丢**最旧**的（队首 ✓）。见 `LOGIN_DEVICES_PER_OWNER`：
            // 这是"最近见过的"，被挤掉的老设备下次再来会被当成新的 —— 刻意 ✓。
            let drop = seen.len() - LOGIN_DEVICES_PER_OWNER;
            seen.drain(..drop);
        }
        Self::persist_login_devices(&db, self)?;
        Ok(true)
    }

    async fn push_rules(&self, owner: &mcloud_core::UserId) -> Result<Vec<mcloud_core::PushRule>> {
        // 没建过 ⇒ 空列表（不是错误），与 `exchange_subscriptions()` 同一口径 ✓。
        Ok(self
            .read()?
            .push_rules
            .get(owner.as_str())
            .cloned()
            .unwrap_or_default())
    }

    async fn push_rule(
        &self,
        owner: &mcloud_core::UserId,
        id: &mcloud_core::PushRuleId,
    ) -> Result<Option<mcloud_core::PushRule>> {
        Ok(self
            .read()?
            .push_rules
            .get(owner.as_str())
            .and_then(|rules| rules.iter().find(|r| &r.id == id).cloned()))
    }

    async fn save_push_rule(
        &self,
        owner: &mcloud_core::UserId,
        rule: &mcloud_core::PushRule,
    ) -> Result<()> {
        let mut db = self.write()?;
        let rules = db.push_rules.entry(owner.as_str().to_string()).or_default();
        match rules.iter_mut().find(|r| r.id == rule.id) {
            // 老行**原地替换**：保持它在列表里的位置 ⇒ 改一次名字不会让界面顺序跳 ✓
            Some(slot) => *slot = rule.clone(),
            None => rules.push(rule.clone()),
        }
        Self::persist_push_rules(&db, self)
    }

    async fn remove_push_rule(
        &self,
        owner: &mcloud_core::UserId,
        id: &mcloud_core::PushRuleId,
    ) -> Result<()> {
        let mut db = self.write()?;
        let mut changed = false;
        let mut empty = false;
        if let Some(rules) = db.push_rules.get_mut(owner.as_str()) {
            let before = rules.len();
            rules.retain(|r| &r.id != id);
            changed = rules.len() != before;
            empty = rules.is_empty();
        }
        if empty {
            // 全删光就把这一格删掉，而不是留一个空数组：文件里少一行垃圾 ✓
            //（读回来时缺键 = 空列表，与上面 `unwrap_or_default` 对上 ✓）。
            db.push_rules.remove(owner.as_str());
        }
        if !changed {
            // 删不存在的**不落盘**：幂等，也省一次整文件重写 ✓。
            return Ok(());
        }
        Self::persist_push_rules(&db, self)
    }

    async fn all_push_rules(&self) -> Result<Vec<mcloud_core::PushRule>> {
        // 摊平所有 owner —— 事件匹配那一层手里只有账号/定时/编排，
        // **没有"当前用户"** ✓（归属判定用每条自己的 `owner` 那一格 ✓）。
        Ok(self
            .read()?
            .push_rules
            .values()
            .flatten()
            .cloned()
            .collect())
    }

    async fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        // 领域约束在这里也守一道：绕过 HTTP 的调用方（CLI / 脚本）同样不该写进非法值。
        settings.validate().map_err(Error::Config)?;
        let mut db = self.write()?;
        // ⚠ `.clone()` 不能省：`AppSettings` 自 2026-10-11 起**不是 `Copy`**
        //（多了一格 `auth_refresh_at: String` ✓）—— 解引用搬移会直接编不过 ✓。
        db.settings = settings.clone();
        // ⚠ 新上限**立刻应用一次**（2026-10-09 用户口径：「让用户设置最大大小」✓）：
        // 把上限调小之后，不该等到"下一次运行"才看见效果 ✓。
        // 两件事都是幂等的（没超就是空操作 ✓），所以这里无脑调一次 ✓。
        let before = db.runs.len();
        Self::evict_runs(&mut db, self);
        let runs_shrunk = db.runs.len() != before;
        self.enforce_log_budget(&mut db)?;
        Self::persist_settings(&db, self)?;
        // ⚠ 淘汰了运行记录就**顺手把盘上那份也写小**：只改内存不落盘的话，
        // 重启后那几条又回来了 ✗（"上限看起来没生效"就是这么来的 ✓）。
        if runs_shrunk {
            Self::persist_runs(&db, self)?;
        }
        Ok(())
    }

    async fn save_session(&self, session: &Session) -> Result<()> {
        let mut db = self.write()?;
        db.sessions
            .insert(session.token_hash.clone(), session.clone());
        Self::persist_sessions(&db, self)
    }

    async fn session(&self, token_hash: &str) -> Result<Option<Session>> {
        Ok(self.read()?.sessions.get(token_hash).cloned())
    }

    async fn remove_session(&self, token_hash: &str) -> Result<()> {
        let mut db = self.write()?;
        db.sessions.remove(token_hash);
        Self::persist_sessions(&db, self)
    }

    async fn remove_user_sessions(&self, user_id: &UserId) -> Result<usize> {
        let mut db = self.write()?;
        // 改口令时必须把旧会话全清掉 —— 否则"改了口令但旧会话还能用"
        let before = db.sessions.len();
        db.sessions.retain(|_, s| &s.user_id != user_id);
        let removed = before - db.sessions.len();
        if removed > 0 {
            Self::persist_sessions(&db, self)?;
        }
        Ok(removed)
    }

    async fn purge_expired_sessions(&self, now_ms: i64) -> Result<usize> {
        let mut db = self.write()?;
        let before = db.sessions.len();
        db.sessions.retain(|_, s| s.is_valid_at(now_ms));
        let removed = before - db.sessions.len();
        if removed > 0 {
            Self::persist_sessions(&db, self)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests;

// ⚠ 这里原先有 `impl mcloud_core::PushConfigStore for JsonStore`（给 `Notifier` 用的
// 那个窄 trait ✓）—— 2026-10-08 推翻重做时与 trait 一起**整个删掉** ✗：
// 投递层不再读"用户级渠道配置"（规则自带 `config` ✓，用户口径：
// 「这个规则配置推送的 token，包括推送哪几个平台」✓）。
