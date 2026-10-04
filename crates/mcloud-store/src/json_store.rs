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
    legacy_owner, visible_to, Account, AccountId, AppSettings, AppendOutcome, Error, LogLine,
    Pipeline, PipelineId, Result, RunId, RunRecord, RunStatus, Schedule, ScheduleId, Seq, Session,
    User, UserId, LOG_LIMIT_PER_RUN,
};

/// 运行历史保留条数。
///
/// 超出后丢弃最旧的**运行记录及其日志** —— 两者必须一起丢，
/// 否则日志文件会无限堆积而没人再引用它们。
pub const HISTORY_LIMIT: usize = 200;

/// 内存中的全量数据。
///
/// 不直接暴露：所有访问都经 [`JsonStore`] 的加锁方法，
/// 避免有人拿到 `&mut` 绕过持久化。
#[derive(Debug, Default)]
struct Db {
    accounts: Vec<Account>,
    /// 运行记录，**最新的在前**。
    runs: VecDeque<RunRecord>,
    /// 按运行归档的日志。
    logs: HashMap<RunId, Vec<LogLine>>,
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
        store.harden_existing_permissions();
        store.load_all()?;
        // 归属回填：老数据（没有 owner 列）必须在这里补上，否则升级后
        // 会表现成「账号全不见了」。放在 `recover_orphans` 之前只是让
        // 「数据先完整、再收尾」这条阅读顺序成立 —— 两者改的是不同的表。
        store.backfill_owners()?;
        store.recover_orphans()?;
        Ok(store)
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
        // ⚠ 备份只从**非空**的旧文件来。0 字节只可能是上一次半截写入的残骸
        // （见 `read_json`），拿它 `copy` 过去等于把唯一一份好 `.bak` 也毁掉
        // —— 那正是"掉电后连备份一起失效"的原因（2026-10-04 复审 co-1）。
        if let Ok(old) = std::fs::read(path) {
            if !old.is_empty() {
                let _ = std::fs::copy(path, path.with_extension("bak"));
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
        let sessions: HashMap<String, Session> = self.read_json(&self.path(files::SESSIONS))?;

        // 日志按 run 分文件，按需读取（只读还保留着的那些 run）
        let mut logs = HashMap::new();
        for r in &runs {
            let p = self.log_path(r.run_id);
            if p.exists() {
                if let Ok(lines) = self.read_json::<Vec<LogLine>>(&p) {
                    logs.insert(r.run_id, lines);
                }
            }
        }

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
        db.sessions = sessions;
        db.logs = logs;
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
        let runs: Vec<&RunRecord> = db.runs.iter().collect();
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
    fn evict_runs(db: &mut Db, store: &Self) {
        while db.runs.len() > HISTORY_LIMIT {
            if let Some(old) = db.runs.pop_back() {
                db.logs.remove(&old.run_id);
                let p = store.log_path(old.run_id);
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::remove_file(p.with_extension("bak"));
            }
        }
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
            )
        });
        f.debug_struct("JsonStore")
            .field("dir", &self.dir)
            .field("accounts/runs/schedules/pipelines", &counts)
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
            let bucket = db.logs.entry(run_id).or_default();
            bucket.extend_from_slice(lines);
            let mut outcome = AppendOutcome::Appended;
            if bucket.len() > LOG_LIMIT_PER_RUN {
                // 丢**最旧**的：排错时最近的日志信息量最大
                let excess = bucket.len() - LOG_LIMIT_PER_RUN;
                bucket.drain(..excess);
                outcome = AppendOutcome::Trimmed;
            }
            (outcome, bucket.clone())
        };
        // 日志每次追加都整体重写该 run 的文件。单 run 上限 2000 条，
        // 文件在百 KB 级，这个代价可以接受；换成"追加到文件尾"会让
        // JSON 不再是一个完整文档，读的时候还得容错解析半截内容。
        let _gate = self
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        self.write_json(&self.log_path(run_id), &snapshot)?;
        Ok(outcome)
    }

    async fn logs(&self, run_id: RunId, after_seq: Option<Seq>) -> Result<Vec<LogLine>> {
        let db = self.read()?;
        let all = db.logs.get(&run_id);
        Ok(match (all, after_seq) {
            (None, _) => Vec::new(),
            // 断线重连续传：只要比已收到的 seq 更新的
            (Some(v), Some(seq)) => v.iter().filter(|l| l.seq > seq).cloned().collect(),
            (Some(v), None) => v.clone(),
        })
    }

    async fn clear_logs(&self, run_id: RunId) -> Result<()> {
        let mut db = self.write()?;
        db.logs.remove(&run_id);
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

    async fn settings(&self) -> Result<AppSettings> {
        Ok(self.read()?.settings)
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

    async fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        // 领域约束在这里也守一道：绕过 HTTP 的调用方（CLI / 脚本）同样不该写进非法值。
        settings.validate().map_err(Error::Config)?;
        let mut db = self.write()?;
        db.settings = *settings;
        Self::persist_settings(&db, self)
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
