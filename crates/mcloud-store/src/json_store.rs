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
//! 3. **写盘是"写临时文件 + rename"**（[`JsonStore::write_atomic`]）。
//!    rename 在同一文件系统内是原子的，所以进程被杀也不会留下半截 JSON。
//!    覆盖前留一份 `.bak`，但**不**试图自动恢复 —— 静默回滚到旧数据
//!    比报错更难排查。
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
    Account, AccountId, AppendOutcome, Error, LogLine, Pipeline, PipelineId, Result, RunId,
    RunRecord, RunStatus, Schedule, ScheduleId, Seq, LOG_LIMIT_PER_RUN,
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
    /// 日志目录：每次运行一个文件，避免为了追加一行重写全部日志。
    pub const LOG_DIR: &str = "logs";
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
        store.recover_orphans()?;
        Ok(store)
    }

    /// 收紧**已存在**文件的权限（老版本装出来的数据目录是默认权限）。
    ///
    /// 新写的文件在 [`Self::write_atomic`] 里就已经是 `0600`，这里只处理
    /// 历史遗留。只收紧、不放松，所以重复执行无副作用。
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

    /// 原子写：临时文件 + rename，覆盖前留 `.bak`。
    ///
    /// rename 在同一文件系统内原子 ⇒ 进程被杀不会留下半截 JSON。
    /// 临时文件名带 pid，避免同目录下两个实例互踩（虽然本工具单实例，
    /// 但代价只是一点点，能免掉一整类诡异现象）。
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
        if path.exists() {
            let _ = std::fs::copy(path, path.with_extension("bak"));
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));

        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(Error::Io)?;
            f.write_all(bytes).map_err(Error::Io)?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&tmp, bytes).map_err(Error::Io)?;
        }

        std::fs::rename(&tmp, path).map_err(Error::Io)?;
        Ok(())
    }

    fn write_json<T: Serialize>(&self, path: &Path, v: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(v)?;
        self.write_atomic(path, &bytes)
    }

    /// 读一个 JSON 文件。不存在或为空 ⇒ 默认值（首次启动是正常路径）。
    fn read_json<T: for<'de> Deserialize<'de> + Default>(&self, path: &Path) -> Result<T> {
        if !path.exists() {
            return Ok(T::default());
        }
        let s = std::fs::read_to_string(path).map_err(Error::Io)?;
        if s.trim().is_empty() {
            return Ok(T::default());
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

    // ── 启动装载与恢复 ──────────────────────────────────────────────────

    fn load_all(&self) -> Result<()> {
        let accounts: Vec<Account> = self.read_json(&self.path(files::ACCOUNTS))?;
        let runs: Vec<RunRecord> = self.read_json(&self.path(files::RUNS))?;
        let schedules: Vec<Schedule> = self.read_json(&self.path(files::SCHEDULES))?;
        let pipelines: Vec<Pipeline> = self.read_json(&self.path(files::PIPELINES))?;
        let state: HashMap<String, serde_json::Value> = self.read_json(&self.path(files::STATE))?;

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

    fn persist_state(db: &Db, store: &Self) -> Result<()> {
        let _gate = store
            .write_gate
            .lock()
            .map_err(|_| Error::Storage("写锁中毒".into()))?;
        store.write_json(&store.path(files::STATE), &db.state)
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
        // 账号没了，它的运行期状态也该跟着走，否则同号再次添加会继承旧 deviceId
        db.state.remove(id.as_str());
        let r = Self::persist_accounts(&db, self);
        Self::persist_state(&db, self)?;
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

// ── 运行记录 ────────────────────────────────────────────────────────────

#[async_trait]
impl mcloud_core::RunStore for JsonStore {
    async fn upsert_run(&self, record: &RunRecord) -> Result<()> {
        let mut db = self.write()?;
        match db.runs.iter_mut().find(|r| r.run_id == record.run_id) {
            Some(slot) => *slot = record.clone(),
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

#[cfg(test)]
mod tests;
