//! 运行器：并发闸 + 事件扇出 + 历史落盘。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mcloud_core::{
    Account, AccountId, Clock, EventSink, HttpClient, Result, RunEvent, RunId, RunRecord,
    RunStatus, Store, TaskOutcome,
};

use crate::{Registry, TaskContext};

/// 按账号构造 HTTP 客户端的工厂。
///
/// 凭证绑定在客户端上，因此每个账号需要独立实例；用具名别名而非
/// 内联写 `Arc<dyn Fn...>`，既让签名可读，也避免 clippy 的复杂度告警。
pub type HttpClientFactory = Arc<dyn Fn(&Account) -> Arc<dyn HttpClient> + Send + Sync>;

/// 任务运行器。
///
/// 并发闸以 `task:account` 为键：同一账号的同一任务不会并发执行，
/// 但不同任务可以并行（例如签到与备份互不阻塞）。
pub struct Runner {
    registry: Arc<Registry>,
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    sink: Arc<dyn EventSink>,
    http_for: HttpClientFactory,
    next_id: AtomicU64,
    inflight: Mutex<HashMap<String, RunId>>,
    /// 运行中任务的取消登记表。
    cancel: Arc<crate::cancel_registry::CancelRegistry>,
    /// run 内产物账本：往用户网盘写了什么、还欠着清理哪些。
    ///
    /// 一个进程一份：清理路径（界面上"待清理"那一栏、将来的收尾任务）
    /// 要能看到跑任务的那条路径记下的东西。
    artifacts: crate::SharedLedger,
    /// run 内**结构化明细**账本：任务自报的、给程序读的形状。
    ///
    /// 与 `artifacts` 同一个套路（运行期间攒、收尾时取走），但语义不同：
    /// 产物是**欠账**，明细只是**信息**。所以这里没有"防漏记"的防线。
    details: crate::SharedDetails,
}

impl Runner {
    pub fn new(
        registry: Arc<Registry>,
        store: Arc<dyn Store>,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn EventSink>,
        http_for: HttpClientFactory,
    ) -> Self {
        Self {
            registry,
            store,
            clock,
            sink,
            http_for,
            next_id: AtomicU64::new(1),
            inflight: Mutex::new(HashMap::new()),
            cancel: Arc::new(crate::cancel_registry::CancelRegistry::new()),
            artifacts: Arc::new(crate::ArtifactLedger::new()),
            details: crate::DetailsLedger::new(),
        }
    }

    /// 取消登记表。服务器据此响应"中止这次运行"。
    pub fn cancel_registry(&self) -> Arc<crate::cancel_registry::CancelRegistry> {
        Arc::clone(&self.cancel)
    }

    /// 产物账本（清理路径与界面读它）。
    pub fn artifacts(&self) -> crate::SharedLedger {
        Arc::clone(&self.artifacts)
    }

    /// 后台启动一次运行，**立刻**返回 run id。
    ///
    /// 界面要能提供"中止"，就必须先拿到 id —— 同步执行的话请求还没返回，
    /// 用户根本没有可中止的时机。
    ///
    /// id 由**本方法**分配并同步返回，再交给 `run_impl` 使用。
    /// 不把这个 id 让调用方自己生成：id 分配必须只有一个来源，
    /// 两处都能分配的话序号就会重复。
    ///
    /// 若同一 `task:account` 已在运行，仍然返回一个 id 与一条
    /// `Skipped` 记录（不排队），调用方据此提示"已在运行中"。
    pub fn start(
        self: &Arc<Self>,
        task_name: &str,
        account: Account,
        body: serde_json::Value,
    ) -> RunId {
        let run_id = RunId::new(self.next_id.fetch_add(1, Ordering::SeqCst));
        // ⚠ **同步**登记取消令牌，再 spawn。
        //
        // 闭环一个真实的竞态窗口：`start` 返回 id 后客户端立刻来订阅事件流，
        // 而此时 spawn 出去的任务可能还没发出 `started` 事件 ——
        // 那一瞬间内存中枢里没有这个 run、存储层也没有（记录在结束时才落盘），
        // 于是订阅会被判 404，而 `EventSource` 遇到 404 **不会重连**，
        // 界面就永远停在"执行中"。
        //
        // 登记在这里之后，"这个 run 存在"在 id 返回时就已经成立。
        let token = self.cancel.register(run_id);
        let this = Arc::clone(self);
        let task_for_spawn = task_name.to_string();
        tokio::spawn(async move {
            if let Err(e) = this
                .run_impl(&task_for_spawn, &account, &body, run_id, None, Some(token))
                .await
            {
                tracing::error!(task = %task_for_spawn, run = %run_id, error = %e, "后台运行失败");
            }
        });
        run_id
    }

    /// 从历史记录续上 run id，避免重启后重复。
    pub fn seed_run_id(&self, id: u64) {
        self.next_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    /// 后台**串行**启动一组运行（一个账号一个 run id），立刻返回全部 id。
    ///
    /// # 为什么需要它（用户口径 2026-10-01）
    ///
    /// 「任何（包括直播小红花）应该允许配置多账号/全部，点击运行后后端保证串行执行」。
    ///
    /// 与 [`Runner::start`] 的唯一区别是**执行顺序**：`start` 每个 run 各自 spawn、
    /// 彼此并发；这里只 spawn **一个**驱动任务，逐个 `await` [`Runner::run_impl`]，
    /// 于是"点一次运行、N 个账号"在后端是**严格串行**的。
    ///
    /// ⚠ **不要**改成"在调用方循环 `start`"：那样每个都立刻 spawn ⇒ 并发 ✗，
    /// 而上游限流（直播小红花这类）最怕的就是并发打过去。
    ///
    /// ⚠ 也**不要**指望并发闸来串行：那道闸的语义是**撞上就 Skipped、不排队**
    /// （见 `run_impl` 里那条注释与 [`Runner::run`] 的文档），它保证不了
    /// "第 2 个账号等第 1 个跑完"，只会把第 2 个直接跳过。
    ///
    /// 取消令牌在**返回之前**逐个登记（与 `start` 同一个理由：客户端拿到 id 会立刻订阅
    /// 事件流，那一刻"这个 run 存在"必须已经成立）。因此还没轮到的账号也能被订阅与中止 ——
    /// 中止一个还没轮到它的账号，它会在轮到自己时带着已取消的令牌进任务体，由任务在安全点
    /// 自己收尾；**其余账号照常继续**（中止只作用于那一条）。
    pub fn start_serial(
        self: &Arc<Self>,
        task_name: &str,
        accounts: Vec<Account>,
        body: serde_json::Value,
    ) -> Vec<RunId> {
        let ids: Vec<RunId> = accounts
            .iter()
            .map(|_| RunId::new(self.next_id.fetch_add(1, Ordering::SeqCst)))
            .collect();
        // ⚠ 同步登记取消令牌，再 spawn —— 理由同 `start`（见那里的长注释）。
        let tokens: Vec<mcloud_core::CancelToken> =
            ids.iter().map(|id| self.cancel.register(*id)).collect();
        let this = Arc::clone(self);
        let task_for_spawn = task_name.to_string();
        // ⚠ 给协程一份**拷贝**：`ids` 还要在最后返回给调用方
        // （调用方靠它订阅事件、请求中止）。直接 `move` 进去就是
        // "use of moved value" —— 修于 2026-10-01，**这一行不是本线的改动**，
        // 是别人未提交的半成品编译不过，顺手补上以免挡住整条 `mcloud-tasks` 编译。
        let ids_for_spawn = ids.clone();
        tokio::spawn(async move {
            for ((account, id), token) in accounts
                .into_iter()
                .zip(ids_for_spawn.iter().copied())
                .zip(tokens.into_iter())
            {
                if let Err(e) = this
                    .run_impl(&task_for_spawn, &account, &body, id, None, Some(token))
                    .await
                {
                    tracing::error!(
                        task = %task_for_spawn,
                        run = %id,
                        account = %account.id,
                        error = %e,
                        "串行批次中的一次运行失败"
                    );
                }
            }
        });
        ids
    }

    /// 任务是否已注册。供编排预检用 —— 它必须在发请求**之前**调用。
    pub fn has_task(&self, name: &str) -> bool {
        self.registry.contains(name)
    }

    /// 给某个账号构造 HTTP 客户端（与任务体拿到的**同一条**装饰链）。
    ///
    /// # 为什么要有这个出口，而不是让调用方自己建一条
    ///
    /// 有几个交互式入口（WebUI 的按钮、CLI 的只读查询）要发**一次**
    /// 业务请求，而它们走的必须是同一条链 —— 会话、`deviceid` 头、
    /// 按域名注入的 `Authorization` 全在这条链上。
    ///
    /// 各调用方自己 `ReqwestClient::new(...)` 一份的表现是
    /// "任务跑得通、这个按钮永远 401"：新那条链上没有会话，
    /// 而它会**静默**少一个头，不报错。
    ///
    /// ⚠ 它**不**登记 run、**不**过并发闸、**不**发事件。
    /// 所以只给**只读**查询用；写操作必须走 [`Runner::run`]。
    pub fn http_for(&self, account: &Account) -> Arc<dyn HttpClient> {
        (self.http_for)(account)
    }

    /// 该 `task:account` 是否正在运行。
    pub fn is_running(&self, task: &str, account: &AccountId) -> bool {
        let key = Self::key(task, account);
        self.inflight
            .lock()
            .expect("inflight 锁中毒")
            .contains_key(&key)
    }

    fn key(task: &str, account: &AccountId) -> String {
        format!("{task}:{account}")
    }

    /// 执行一个任务并落盘记录。
    ///
    /// 若同 `task:account` 已在运行，返回 `Skipped`，不排队——排队会造成
    /// 定时任务堆积，掩盖真实问题。
    pub async fn run(
        &self,
        task_name: &str,
        account: &Account,
        body: &serde_json::Value,
    ) -> Result<RunRecord> {
        self.run_with_chain_cancel(task_name, account, body, None)
            .await
    }

    /// 同上，但额外接受**整条编排链**的取消令牌。
    ///
    /// 单次运行的令牌由本方法内部登记；链令牌由编排器传入。
    /// 任务两个都看，因此"中止整条链"能传到每一步内部。
    pub async fn run_with_chain_cancel(
        &self,
        task_name: &str,
        account: &Account,
        body: &serde_json::Value,
        chain_cancel: Option<mcloud_core::CancelToken>,
    ) -> Result<RunRecord> {
        let run_id = RunId::new(self.next_id.fetch_add(1, Ordering::SeqCst));
        self.run_impl(task_name, account, body, run_id, chain_cancel, None)
            .await
    }

    /// 实际执行。`run_id` 由调用方预分配 —— 这样"立刻返回 id 再后台跑"
    /// 与"同步跑完再返回"可以共用同一条实现。
    async fn run_impl(
        &self,
        task_name: &str,
        account: &Account,
        body: &serde_json::Value,
        run_id: RunId,
        chain_cancel: Option<mcloud_core::CancelToken>,
        // 已由 `start` 预登记的令牌。`None` 表示现登记。
        pre_registered: Option<mcloud_core::CancelToken>,
    ) -> Result<RunRecord> {
        let task = self
            .registry
            .get(task_name)
            .ok_or_else(|| mcloud_core::Error::Config(format!("未知任务：{task_name}")))?;

        // ⚠ 「排队中被取消」要**跳过**，不是照样开跑。
        //
        // `start_serial`（多账号）会给**每个**账号预登记取消令牌，于是轮到某个账号时，
        // 它可能在那之前就被取消了（用户在队列里点了中止 / 点了"全部中止"）。
        // 不查这一下的话，"取消"就退化成"跑起来之后再停"：界面上会冒出一串
        // "开始了又立刻中止"的假运行，而且每一趟都可能已经向上游发过请求
        // —— 副作用已经产生，而用户以为自己取消了。
        //
        // ⚠ 必须查在 `Started` **之前**：那个事件的语义是"真的开始干活了"。
        // ⚠ 令牌是调用方预登记的，走这条提前返回要**自己还回去**（`release`），
        // 否则取消表里会留一条没人回收的条目（进程越久积得越多）。
        if pre_registered.as_ref().is_some_and(|t| t.is_cancelled()) {
            self.cancel.release(run_id);
            return Ok(self.record(
                run_id,
                task_name,
                account,
                0,
                TaskOutcome::skipped("已被中止，这一趟跳过"),
            ));
        }

        let key = Self::key(task_name, &account.id);
        {
            let mut inflight = self.inflight.lock().expect("inflight 锁中毒");
            if inflight.contains_key(&key) {
                // 不排队：排队会让定时任务在慢上游面前无限堆积
                return Ok(self.record(
                    RunId::new(0),
                    task_name,
                    account,
                    0,
                    TaskOutcome::skipped("同一任务正在运行中"),
                ));
            }
            inflight.insert(key.clone(), run_id);
        }

        let started = self.clock.now_ms();
        self.sink.emit(RunEvent::Started {
            run_id,
            task: task_name.to_string(),
            account: account.id.to_string(),
            at_ms: started,
        });

        // 登记取消令牌：任务在安全点检查它。
        // 登记放在 Started 事件**之前**，这样"刚发出 Started 就点中止"
        // 也能被接住，不会出现一个永远无法中止的窗口。
        let cancel_token = pre_registered.unwrap_or_else(|| self.cancel.register(run_id));

        let ctx = TaskContext {
            account: account.clone(),
            http: (self.http_for)(account),
            store: Arc::clone(&self.store),
            clock: Arc::clone(&self.clock),
            sink: Arc::clone(&self.sink),
            run_id,
            cancel: cancel_token.clone(),
            chain_cancel,
            artifacts: crate::ArtifactRecorder::new(
                Arc::clone(&self.artifacts),
                run_id,
                account.id.clone(),
            ),
            details: crate::DetailsRecorder::new(Arc::clone(&self.details), run_id),
        };

        let outcome = match task.run(&ctx, body).await {
            Ok(o) => o,
            Err(e) => TaskOutcome::failed(e.to_string()),
        };

        let duration_ms = (self.clock.now_ms() - started).max(0) as u64;
        // 中止与业务结论分开记：中止不是失败，不该计入错误率
        let was_cancelled = cancel_token.is_cancelled();

        // ⭐ 收尾：把**还没清掉的产物**取出来，落进运行记录。
        //
        // # 为什么必须取出来而不是留在账本里
        //
        // 账本留在内存里有两个后果，都属于"漏记"那一类：
        //
        // ① **看不见**。旧项目只在小结里念"网盘里留了 N 个待清理文件"，
        //    却不说是**哪 N 个** —— 将来手工清理时无从下手。
        //    这里把清单连同 ID 一起写进 `details.artifacts`，
        //    于是"留了什么"在历史记录里查得到。
        // ② **不释放**。一次运行结束了账本还替它留着条目，
        //    进程跑得越久占得越多 —— 而这些东西要么已被清理（该销账），
        //    要么已经交给记录（该遗忘）。
        //
        // `forget` 的返回值**刻意**是"还没清的那些"：
        // 账本消失不代表用户网盘上的文件消失。非空本身就是一条要说出去的事实。
        let leftovers = self.artifacts.forget(run_id);

        // ⚠ 顺序要紧：这条警告必须在 `Finished` **之前**发。
        //
        // 前端收到 `finished` 就 `close()` 那条 SSE（`web/src/api.ts`
        // 的 `subscribeRun` 有一条"终局即关闭"的说明）。发在后面的话，
        // 警告会落在**已经关掉的流**上 —— 直连看日志的人还能看到，
        // 但正在看页面的那个人一个字都收不到。
        //
        // 静默是这一层最该防的失效：用户不会知道自己网盘里多了文件。
        if !leftovers.is_empty() {
            self.sink.emit(RunEvent::Log {
                run_id,
                level: mcloud_core::LogLevel::Warn,
                message: format!(
                    "本次运行在网盘里留下 {} 个未清理的文件（清单见运行记录）：{}",
                    leftovers.len(),
                    leftovers
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                at_ms: self.clock.now_ms(),
            });
        }

        self.sink.emit(RunEvent::Finished {
            run_id,
            outcome: outcome.clone(),
            duration_ms,
        });

        self.inflight.lock().expect("inflight 锁中毒").remove(&key);
        self.cancel.release(run_id);

        let mut record = self.record(run_id, task_name, account, duration_ms, outcome);

        // ── 并把两样收尾的东西落进 `details` ──────────────────────────────
        //
        // `details` 是"前端可直接渲染"的那一格，这里有两个来源：
        //   - `artifacts`：**欠账**（还没清掉的网盘文件），非空才写；
        //   - `details`：任务**自报**的结构化明细，按到达顺序累积。
        //
        // ⚠ 合并（而不是互相覆盖）：两者可以同时存在 —— 一个任务既留了
        // 待清理文件、又交回一份形状，这是正常情形。
        //
        // ⚠ `artifacts` 那一格的**名字与形状不要动**：既有测试
        // （`runner.rs` 里读 `details["artifacts"]` 的那两条）与前端都按它读。
        //
        // 自报明细统一落在 `details["reported"]`：
        //   - 只有一条时**不裹数组**（`{"reported": {...}}`），多了一条才是数组；
        //   - 任务**不选**这个键名 —— 让任务自己定键名，等于让"运行历史
        //     长什么样"分散到 N 个任务里，读的人还得先知道是哪条任务。
        //     固定一个键名，消费方按"我这次跑的什么任务"去解里面的形状。
        let reported = self.details.take(run_id);
        if !leftovers.is_empty() || !reported.is_empty() {
            let mut details = serde_json::Map::new();
            if !leftovers.is_empty() {
                details.insert("artifacts".into(), serde_json::json!(leftovers));
            }
            if !reported.is_empty() {
                // 单条时**不裹数组**：`{"reported": {...}}` 比
                // `{"reported": [{...}]}` 好用得多，而"只自报一条"是绝大多数
                // 任务的样子。多条时才是数组 —— 消费方两种都要能读。
                let value = if reported.len() == 1 {
                    reported.into_iter().next().expect("长度已判为 1")
                } else {
                    serde_json::Value::Array(reported)
                };
                details.insert("reported".into(), value);
            }
            record.details = serde_json::Value::Object(details);
        }
        if was_cancelled {
            record.status = RunStatus::Cancelled;
        }
        // 历史落盘失败不应让整次运行算失败——结果已经产生。
        if let Err(e) = self.store.upsert_run(&record).await {
            tracing::warn!(error = %e, "运行历史落盘失败");
        }
        Ok(record)
    }

    fn record(
        &self,
        run_id: RunId,
        task: &str,
        account: &Account,
        duration_ms: u64,
        outcome: TaskOutcome,
    ) -> RunRecord {
        RunRecord {
            run_id,
            task: task.to_string(),
            account: account.id.to_string(),
            started_at_ms: self.clock.now_ms() - duration_ms as i64,
            duration_ms,
            outcome,
            // 生命周期与业务结论分开：这里刚跑完，是 Done；
            // 被中止时由调用方覆盖成 Cancelled。
            status: RunStatus::Done,
            details: serde_json::Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests::NoHttp;
    use crate::{Task, TaskResult};
    use async_trait::async_trait;
    use mcloud_core::testkit::FixedClock;
    use mcloud_core::{
        AccountSettings, AccountStore, AuthInfo, DeviceId, HttpRequest, HttpResponse, LogStore,
        PipelineStore, RunStore, ScheduleStore,
    };

    /// 固定时刻，便于断言时长与开始时间。
    const NOW_MS: i64 = 1_700_000_000_000;

    // ── 测试替身 ────────────────────────────────────────────────────────

    struct FakeHttp;

    #[async_trait]
    impl HttpClient for FakeHttp {
        async fn request(&self, _req: HttpRequest) -> Result<HttpResponse> {
            Ok(HttpResponse {
                status: 200,
                body: "{}".into(),
                headers: Vec::new(),
            })
        }
    }

    /// 只实现 Runner 真正用到的能力。
    ///
    /// `Store` 是 `AccountStore + RunStore + LogStore + ScheduleStore +
    /// PipelineStore` 的组合，因此这里五个都要实现（哪怕多数是空壳）——
    /// 这正是拆分 trait 的代价，换来的是真实后端可以逐个替换。
    #[derive(Default)]
    struct FakeStore {
        runs: Mutex<Vec<RunRecord>>,
        /// 为 true 时落盘失败，用于验证「落盘失败不影响运行结果」。
        fail_upsert: bool,
    }

    impl FakeStore {
        fn failing() -> Self {
            Self {
                fail_upsert: true,
                ..Default::default()
            }
        }
    }

    #[async_trait]
    impl AccountStore for FakeStore {
        async fn accounts(&self) -> Result<Vec<Account>> {
            Ok(Vec::new())
        }
        async fn account(&self, _id: &AccountId) -> Result<Option<Account>> {
            Ok(None)
        }
        async fn save_account(&self, _account: &Account) -> Result<()> {
            Ok(())
        }
        async fn remove_account(&self, _id: &AccountId) -> Result<()> {
            Ok(())
        }
        async fn account_state(&self, _id: &AccountId) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        async fn save_account_state(
            &self,
            _id: &AccountId,
            _state: &serde_json::Value,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl RunStore for FakeStore {
        async fn upsert_run(&self, record: &RunRecord) -> Result<()> {
            if self.fail_upsert {
                return Err(mcloud_core::Error::Other("磁盘写满".into()));
            }
            let mut runs = self.runs.lock().expect("锁中毒");
            match runs.iter_mut().find(|r| r.run_id == record.run_id) {
                Some(slot) => *slot = record.clone(),
                None => runs.push(record.clone()),
            }
            Ok(())
        }
        async fn runs(&self, limit: usize) -> Result<Vec<RunRecord>> {
            let mut all = self.runs.lock().expect("锁中毒").clone();
            all.truncate(limit);
            Ok(all)
        }
        async fn run(&self, id: RunId) -> Result<Option<RunRecord>> {
            Ok(self
                .runs
                .lock()
                .expect("锁中毒")
                .iter()
                .find(|r| r.run_id == id)
                .cloned())
        }
        async fn running_runs(&self) -> Result<Vec<RunRecord>> {
            Ok(self
                .runs
                .lock()
                .expect("锁中毒")
                .iter()
                .filter(|r| r.status == RunStatus::Running)
                .cloned()
                .collect())
        }
    }

    #[async_trait]
    impl LogStore for FakeStore {
        async fn append_logs(
            &self,
            _run_id: RunId,
            _lines: &[mcloud_core::LogLine],
        ) -> Result<mcloud_core::AppendOutcome> {
            Ok(mcloud_core::AppendOutcome::Appended)
        }
        async fn logs(
            &self,
            _run_id: RunId,
            _after_seq: Option<mcloud_core::Seq>,
        ) -> Result<Vec<mcloud_core::LogLine>> {
            Ok(Vec::new())
        }
        async fn clear_logs(&self, _run_id: RunId) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl ScheduleStore for FakeStore {
        async fn schedules(&self) -> Result<Vec<mcloud_core::Schedule>> {
            Ok(Vec::new())
        }
        async fn schedule(
            &self,
            _id: &mcloud_core::ScheduleId,
        ) -> Result<Option<mcloud_core::Schedule>> {
            Ok(None)
        }
        async fn save_schedule(&self, _schedule: &mcloud_core::Schedule) -> Result<()> {
            Ok(())
        }
        async fn remove_schedule(&self, _id: &mcloud_core::ScheduleId) -> Result<()> {
            Ok(())
        }
        async fn mark_schedule_fired(&self, _id: &mcloud_core::ScheduleId, _ms: i64) -> Result<()> {
            Ok(())
        }
        async fn record_schedule_run(
            &self,
            _id: &mcloud_core::ScheduleId,
            _run_ms: Option<i64>,
            _status: &str,
            _error: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl PipelineStore for FakeStore {
        async fn pipelines(&self) -> Result<Vec<mcloud_core::Pipeline>> {
            Ok(Vec::new())
        }
        async fn pipeline(
            &self,
            _id: &mcloud_core::PipelineId,
        ) -> Result<Option<mcloud_core::Pipeline>> {
            Ok(None)
        }
        async fn save_pipeline(&self, _pipeline: &mcloud_core::Pipeline) -> Result<()> {
            Ok(())
        }
        async fn remove_pipeline(&self, _id: &mcloud_core::PipelineId) -> Result<()> {
            Ok(())
        }
        async fn mark_pipeline_ran(&self, _id: &mcloud_core::PipelineId, _ms: i64) -> Result<()> {
            Ok(())
        }
    }

    /// 登录闸在 runner 测试里用不到 —— 全部给"空/无"的实现。
    ///
    /// 给空实现而不是 `unimplemented!()`：将来某个测试顺路调到它时，
    /// 得到的是"没有用户"这个**语义正确**的结果，而不是一次 panic。
    #[async_trait]
    impl mcloud_core::UserStore for FakeStore {
        async fn user_count(&self) -> Result<usize> {
            Ok(0)
        }
        async fn users(&self) -> Result<Vec<mcloud_core::User>> {
            Ok(Vec::new())
        }
        async fn user_by_name(&self, _n: &str) -> Result<Option<mcloud_core::User>> {
            Ok(None)
        }
        async fn user(&self, _id: &mcloud_core::UserId) -> Result<Option<mcloud_core::User>> {
            Ok(None)
        }
        async fn save_user(&self, _u: &mcloud_core::User) -> Result<()> {
            Ok(())
        }
        async fn save_session(&self, _s: &mcloud_core::Session) -> Result<()> {
            Ok(())
        }
        async fn session(&self, _h: &str) -> Result<Option<mcloud_core::Session>> {
            Ok(None)
        }
        async fn remove_session(&self, _h: &str) -> Result<()> {
            Ok(())
        }
        async fn remove_user_sessions(&self, _id: &mcloud_core::UserId) -> Result<usize> {
            Ok(0)
        }
        async fn purge_expired_sessions(&self, _now: i64) -> Result<usize> {
            Ok(0)
        }
    }

    #[derive(Default)]
    struct FakeSink {
        events: Mutex<Vec<RunEvent>>,
    }

    impl FakeSink {
        /// 事件名序列，用于断言顺序。
        fn names(&self) -> Vec<&'static str> {
            self.events
                .lock()
                .expect("锁中毒")
                .iter()
                .map(|e| e.event_name())
                .collect()
        }

        fn run_ids(&self) -> Vec<RunId> {
            self.events
                .lock()
                .expect("锁中毒")
                .iter()
                .map(|e| e.run_id())
                .collect()
        }
    }

    impl EventSink for FakeSink {
        fn emit(&self, event: RunEvent) {
            self.events.lock().expect("锁中毒").push(event);
        }
    }

    // ── 假任务 ──────────────────────────────────────────────────────────

    struct OkTask;

    #[async_trait]
    impl Task for OkTask {
        fn name(&self) -> &'static str {
            "ok"
        }
        fn title(&self) -> &'static str {
            "成功任务"
        }
        fn description(&self) -> &'static str {
            "永远成功"
        }
        async fn run(&self, _ctx: &TaskContext, _body: &serde_json::Value) -> TaskResult {
            Ok(TaskOutcome::success("完成"))
        }
    }

    struct FailTask;

    #[async_trait]
    impl Task for FailTask {
        fn name(&self) -> &'static str {
            "fail"
        }
        fn title(&self) -> &'static str {
            "失败任务"
        }
        fn description(&self) -> &'static str {
            "永远报错"
        }
        async fn run(&self, _ctx: &TaskContext, _body: &serde_json::Value) -> TaskResult {
            Err(mcloud_core::Error::Network("断网了".into()))
        }
    }

    struct SkipTask;

    #[async_trait]
    impl Task for SkipTask {
        fn name(&self) -> &'static str {
            "skip"
        }
        fn title(&self) -> &'static str {
            "跳过任务"
        }
        fn description(&self) -> &'static str {
            "条件不满足"
        }
        async fn run(&self, _ctx: &TaskContext, _body: &serde_json::Value) -> TaskResult {
            Ok(TaskOutcome::skipped("今天已签到"))
        }
    }

    /// 会上报进度与日志的任务，用于验证事件序列。
    struct ChattyTask;

    #[async_trait]
    impl Task for ChattyTask {
        fn name(&self) -> &'static str {
            "chatty"
        }
        fn title(&self) -> &'static str {
            "话多任务"
        }
        fn description(&self) -> &'static str {
            "发进度与日志"
        }
        async fn run(&self, ctx: &TaskContext, _body: &serde_json::Value) -> TaskResult {
            ctx.progress(1, 2, "第一步");
            ctx.info("进行中");
            ctx.warn("注意");
            Ok(TaskOutcome::success("完成"))
        }
    }

    /// 进入任务体后阻塞，直到 `release` 被通知；用于测试并发闸。
    struct GatedTask {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Task for GatedTask {
        fn name(&self) -> &'static str {
            "gated"
        }
        fn title(&self) -> &'static str {
            "闸门任务"
        }
        fn description(&self) -> &'static str {
            "阻塞直到放行"
        }
        async fn run(&self, _ctx: &TaskContext, _body: &serde_json::Value) -> TaskResult {
            self.started.notify_one();
            self.release.notified().await;
            Ok(TaskOutcome::success("放行"))
        }
    }

    // ── 组装辅助 ────────────────────────────────────────────────────────

    fn build_with(tasks: Vec<Arc<dyn Task>>, store: Arc<FakeStore>) -> (Runner, Arc<FakeSink>) {
        let mut registry = Registry::new();
        for t in tasks {
            registry.register(t).expect("测试任务注册失败");
        }
        let sink = Arc::new(FakeSink::default());
        let clock = Arc::new(FixedClock(NOW_MS));
        let http: HttpClientFactory =
            Arc::new(|_account: &Account| Arc::new(FakeHttp) as Arc<dyn HttpClient>);

        let runner = Runner::new(
            Arc::new(registry),
            store,
            clock,
            Arc::clone(&sink) as Arc<dyn EventSink>,
            http,
        );
        (runner, sink)
    }

    fn build(tasks: Vec<Arc<dyn Task>>) -> (Runner, Arc<FakeStore>, Arc<FakeSink>) {
        let store = Arc::new(FakeStore::default());
        let (runner, sink) = build_with(tasks, Arc::clone(&store));
        (runner, store, sink)
    }

    fn account(phone: &str) -> Account {
        Account {
            id: AccountId::new(phone),
            nickname: None,
            auth: AuthInfo {
                raw: "raw".into(),
                platform: "android".into(),
                phone: phone.into(),
                token: "t".into(),
                expire: 0,
            },
            device_id: DeviceId("dev".into()),
            pc_platform: None,
            user_domain_id: None,
            owner: None,
            enabled: true,
            settings: AccountSettings::default(),
        }
    }

    /// 等任务真正进入任务体。
    ///
    /// 带超时是刻意的：如果闸逻辑坏掉，这个等待会永远不返回，测试就退化成
    /// 「挂到 CI 超时」——比失败更难排查。超时后给出明确信息，直接指向
    /// 真正的问题，而不是让人猜为什么测试卡住了。
    async fn wait_started(started: &tokio::sync::Notify) {
        tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
            .await
            .expect("任务未在 5 秒内进入任务体：并发闸的建立逻辑可能已损坏");
    }

    // ── 多账号串行批次（用户口径 2026-10-01）────────────────────────────

    /// 记账任务：把自己的"进入/离开"按顺序写进共享轨迹。
    ///
    /// 用它而不是 `GatedTask`：这里要证的正是**没有重叠**，
    /// 所以轨迹里每条都带账号，事后一眼看出是不是 `开始A→结束A→开始B→结束B`。
    struct TraceTask {
        trace: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Task for TraceTask {
        fn name(&self) -> &'static str {
            "trace"
        }
        fn title(&self) -> &'static str {
            "记账任务"
        }
        fn description(&self) -> &'static str {
            "记录进出顺序"
        }
        async fn run(&self, ctx: &TaskContext, _body: &serde_json::Value) -> TaskResult {
            self.trace
                .lock()
                .expect("轨迹锁中毒")
                .push(format!("开始:{}", ctx.account.id));
            // 让出执行权并睡一下：真并发的话，第二个账号会插到这两行中间。
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.trace
                .lock()
                .expect("轨迹锁中毒")
                .push(format!("结束:{}", ctx.account.id));
            Ok(TaskOutcome::success("跑完"))
        }
    }

    #[tokio::test]
    async fn start_serial_runs_accounts_one_at_a_time_in_order() {
        let trace: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let (runner, _store, _sink) = build(vec![Arc::new(TraceTask {
            trace: Arc::clone(&trace),
        })]);
        let runner = Arc::new(runner);
        let a = account("13800000001");
        let b = account("13800000002");
        let c = account("13800000003");

        let ids = runner.start_serial(
            "trace",
            vec![a.clone(), b.clone(), c.clone()],
            serde_json::json!({}),
        );
        // id 由 runner 预分配并**立刻**返回（界面拿到就要订阅事件流/能中止）
        assert_eq!(ids.len(), 3, "三个账号应当预分配三个 run id");
        let uniq: std::collections::HashSet<String> = ids.iter().map(|i| i.to_string()).collect();
        assert_eq!(uniq.len(), 3, "预分配的 id 必须互不相同：{ids:?}");

        // 等三个都跑完。带超时是刻意的：卡住要直接指向问题，而不是挂成 CI 超时。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if trace.lock().expect("轨迹锁中毒").len() >= 6 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "串行批次没在 5 秒内跑完（可能死等或被并发闸跳过）：{trace:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let got = trace.lock().expect("轨迹锁中毒").clone();
        assert_eq!(
            got,
            vec![
                format!("开始:{}", a.id),
                format!("结束:{}", a.id),
                format!("开始:{}", b.id),
                format!("结束:{}", b.id),
                format!("开始:{}", c.id),
                format!("结束:{}", c.id),
            ],
            "三个账号必须**一个跑完再跑下一个**，且顺序与传入顺序一致；实际 {got:?}"
        );
    }

    /// 多账号串行时，**排队中被取消的账号要被跳过** —— 不是"跑起来再停"。
    ///
    /// 界面上点「中止」/「全部中止」之后，那些**还没轮到**的账号必须直接跳过：
    /// 让它们开跑再停，既会冒出一串"开始了又立刻中止"的假运行，
    /// 也可能已经向上游发过请求（副作用产生了，而用户以为取消了）。
    /// 这条用例同时钉住两件事：**跳过的那趟连 `Started` 都不发**、**后面的账号照跑**。
    #[tokio::test]
    async fn start_serial_skips_an_account_cancelled_while_queued() {
        let trace: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let (runner, _store, sink) = build(vec![Arc::new(TraceTask {
            trace: Arc::clone(&trace),
        })]);
        let runner = Arc::new(runner);
        let a = account("13800000001");
        let b = account("13800000002");
        let c = account("13800000003");

        let ids = runner.start_serial(
            "trace",
            vec![a.clone(), b.clone(), c.clone()],
            serde_json::json!({}),
        );

        // 等 1 号真的开跑，再去取消 **2 号**（它此刻还排在队列里）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while trace.lock().expect("轨迹锁中毒").is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "1 号没在 5 秒内开跑，后面就没法构造「排队中被取消」这个局面"
            );
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert!(
            runner.cancel_registry().cancel(ids[1]),
            "排队中的账号也必须能被取消（界面拿到的 id 就是为这个给的）"
        );

        // 等整批跑完：2 号被跳过 ⇒ 轨迹只有 4 条（1 号两条 + 3 号两条）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while trace.lock().expect("轨迹锁中毒").len() < 4 {
            assert!(
                std::time::Instant::now() < deadline,
                "批次没在 5 秒内跑完（跳过一个不该把后面的也带走）：{trace:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let got = trace.lock().expect("轨迹锁中毒").clone();
        assert_eq!(
            got,
            vec![
                format!("开始:{}", a.id),
                format!("结束:{}", a.id),
                format!("开始:{}", c.id),
                format!("结束:{}", c.id),
            ],
            "被取消的排队账号必须**整个跳过**，后面的账号照常跑；实际 {got:?}"
        );

        // 更硬的一条：跳过的那趟**连 `Started` 都没发** ——
        // 否则界面上仍会闪一次"开始"，用户会以为取消没生效。
        let started: Vec<RunId> = sink
            .events
            .lock()
            .expect("事件锁中毒")
            .iter()
            .filter_map(|e| match e {
                RunEvent::Started {
                    run_id, account, ..
                } if account == &b.id.to_string() => Some(*run_id),
                _ => None,
            })
            .collect();
        assert!(
            started.is_empty(),
            "2 号被取消后不该有任何 Started 事件，实际：{started:?}"
        );
    }

    // ── 基本执行路径 ────────────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_task_is_config_error() {
        let (runner, _store, _sink) = build(Vec::new());
        let err = runner
            .run("nope", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(err, mcloud_core::Error::Config(_)),
            "未知任务应是配置错误，而不是被当成别的失败：{err:?}"
        );
    }

    #[tokio::test]
    async fn successful_run_emits_started_then_finished() {
        let (runner, _store, sink) = build(vec![Arc::new(OkTask)]);
        let record = runner
            .run("ok", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        assert!(record.outcome.is_success());
        assert_eq!(
            sink.names(),
            vec!["started", "finished"],
            "订阅方依赖这两个事件界定一次运行的始终"
        );
    }

    #[tokio::test]
    async fn all_events_carry_the_run_id_of_the_record() {
        let (runner, _store, sink) = build(vec![Arc::new(ChattyTask)]);
        let record = runner
            .run("chatty", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        let ids = sink.run_ids();
        assert!(!ids.is_empty());
        assert!(
            ids.iter().all(|id| *id == record.run_id),
            "事件里的 run_id 必须与落盘记录一致，否则 SSE 会推错流：{ids:?} vs {:?}",
            record.run_id
        );
    }

    #[tokio::test]
    async fn progress_and_log_events_keep_their_order() {
        let (runner, _store, sink) = build(vec![Arc::new(ChattyTask)]);
        runner
            .run("chatty", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(
            sink.names(),
            vec!["started", "progress", "log", "log", "finished"],
            "进度与日志必须按发生顺序发出"
        );
    }

    // ── 结果语义：失败与跳过 ────────────────────────────────────────────

    #[tokio::test]
    async fn task_error_becomes_failed_outcome_and_is_still_recorded() {
        let (runner, store, sink) = build(vec![Arc::new(FailTask)]);
        let record = runner
            .run("fail", &account("13800000000"), &serde_json::json!({}))
            .await
            .expect("任务报错不应让 run 本身返回 Err");

        assert!(record.outcome.is_failure());
        assert!(
            record.outcome.summary().contains("断网了"),
            "失败原因要保留下来：{}",
            record.outcome.summary()
        );
        assert_eq!(store.runs(10).await.unwrap().len(), 1, "失败也要落盘");
        assert_eq!(
            sink.names(),
            vec!["started", "finished"],
            "失败同样必须发 finished，否则前端会一直转圈"
        );
    }

    #[tokio::test]
    async fn skipped_outcome_is_preserved_as_success_path() {
        let (runner, _store, _sink) = build(vec![Arc::new(SkipTask)]);
        let record = runner
            .run("skip", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        // 跳过不是失败：不应触发告警，也不该被记成错误
        assert!(!record.outcome.is_failure(), "跳过不能被当成失败");
        assert!(!record.outcome.is_success(), "跳过也不是成功");
        assert_eq!(record.outcome.summary(), "今天已签到");
        assert!(!record.needs_attention());
    }

    // ── 并发闸 ──────────────────────────────────────────────────────────

    /// 闸的键是 `task:account`，因此既不能串到别的任务，也不能串到别的账号。
    #[tokio::test]
    async fn gate_key_is_scoped_by_task_and_account() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let task = Arc::new(GatedTask {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });
        let (runner, _store, _sink) = build(vec![task]);
        let runner = Arc::new(runner);

        let me = account("13800000000");
        let other = account("13900000000");

        let first = {
            let runner = Arc::clone(&runner);
            let me = me.clone();
            tokio::spawn(async move { runner.run("gated", &me, &serde_json::json!({})).await })
        };

        // 等任务真正进入任务体，闸此时必然已建立
        wait_started(&started).await;
        assert!(runner.is_running("gated", &me.id), "运行期间应标记为在运行");
        assert!(
            !runner.is_running("gated", &other.id),
            "别的账号不该被同一把闸挡住"
        );
        assert!(
            !runner.is_running("other", &me.id),
            "别的任务不该被同一把闸挡住"
        );

        release.notify_one();
        let record = first.await.unwrap().unwrap();
        assert!(record.outcome.is_success());
        assert!(!runner.is_running("gated", &me.id), "结束后必须释放闸");
    }

    /// ⭐ `start` 返回时该 run **必须已经可被认定为"存在"**。
    ///
    /// 这是修过的一个真 bug：`start` 返回 id 后客户端立刻订阅事件流，
    /// 而 spawn 出去的任务此刻可能还没发出 `started` 事件 ——
    /// 那一瞬间内存中枢里没有它、存储层也没有（运行记录在**结束时**才落盘），
    /// 于是订阅被判 404。而 `EventSource` 遇到 404 **不会重连**，
    /// 界面就永远停在"执行中"（实测卡了 60 秒以上）。
    ///
    /// 所以 `start` 必须**同步**登记取消令牌 —— "这个 run 存在"在
    /// id 返回的那一刻就成立。
    #[tokio::test]
    async fn start_registers_the_run_before_returning_the_id() {
        // 一个跑很久的任务，确保 start 返回时它还没结束
        struct HangingTask;
        #[async_trait]
        impl Task for HangingTask {
            fn name(&self) -> &'static str {
                "hang"
            }
            fn title(&self) -> &'static str {
                "挂着"
            }
            fn description(&self) -> &'static str {
                "一直跑"
            }
            async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
                for _ in 0..200 {
                    ctx.check_cancelled()?;
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Ok(TaskOutcome::success("done"))
            }
        }

        let mut reg = Registry::new();
        reg.register(Arc::new(HangingTask)).unwrap();
        let http: HttpClientFactory =
            Arc::new(|_a: &Account| Arc::new(NoHttp) as Arc<dyn HttpClient>);
        let runner = Arc::new(Runner::new(
            Arc::new(reg),
            Arc::new(FakeStore::default()) as Arc<dyn Store>,
            Arc::new(FixedClock(NOW_MS)),
            Arc::new(FakeSink::default()) as Arc<dyn EventSink>,
            http,
        ));

        let run_id = runner.start("hang", account("13800000000"), serde_json::Value::Null);

        // **不给任何等待时间**：id 一返回，"存在"就该成立。
        // 服务端的 SSE 存在性判定正是靠这个。
        assert!(
            runner.cancel_registry().is_running(run_id),
            "start 返回时必须已登记，否则订阅事件流会被判 404，而 EventSource 不重连"
        );

        runner.cancel_registry().cancel(run_id);
        // 让它退干净，避免影响其他测试
        for _ in 0..100 {
            if runner.cancel_registry().running_count() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        }
    }

    /// ⭐ 后台启动**立刻**返回真实的 run id，且该 id 可用来中止。
    ///
    /// 这是"界面上能点中止"的前提：同步执行的话请求还没返回，
    /// 用户根本没有可中止的时机。
    #[tokio::test]
    async fn start_returns_a_live_cancellable_run_id() {
        // 一个会跑很久、并在安全点检查取消的任务
        struct SlowTask;
        #[async_trait]
        impl Task for SlowTask {
            fn name(&self) -> &'static str {
                "slow"
            }
            fn title(&self) -> &'static str {
                "慢任务"
            }
            fn description(&self) -> &'static str {
                "跑很久，可被中止"
            }
            async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
                for _ in 0..100 {
                    // 安全点：每轮检查一次取消
                    ctx.check_cancelled()?;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Ok(TaskOutcome::success("跑完了"))
            }
        }

        let mut reg = Registry::new();
        reg.register(Arc::new(SlowTask)).unwrap();
        let http: HttpClientFactory =
            Arc::new(|_a: &Account| Arc::new(NoHttp) as Arc<dyn HttpClient>);
        let store = Arc::new(FakeStore::default());
        let runner = Arc::new(Runner::new(
            Arc::new(reg),
            Arc::clone(&store) as Arc<dyn Store>,
            Arc::new(FixedClock(NOW_MS)),
            Arc::new(FakeSink::default()) as Arc<dyn EventSink>,
            http,
        ));

        let acc = account("13800000000");
        let run_id = runner.start("slow", acc.clone(), serde_json::Value::Null);
        assert!(run_id.get() > 0, "后台启动必须立刻返回真实 id，不能是 0");

        // 等它真的进入运行中
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert!(
            runner.cancel_registry().running_count() > 0,
            "后台任务应已登记，否则无法中止"
        );

        // 中止它
        assert!(
            runner.cancel_registry().cancel(run_id),
            "刚启动的运行必须能被中止"
        );

        // 等它退出
        for _ in 0..60 {
            if runner.cancel_registry().running_count() == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        let rec = store
            .runs(10)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.run_id == run_id)
            .expect("中止的运行也必须落盘");
        assert_eq!(
            rec.status,
            RunStatus::Cancelled,
            "生命周期状态应是已中止，而不是已结束"
        );
        assert!(
            !rec.needs_attention(),
            "用户自己点的中止不该报红 —— 那只是噪音"
        );
    }

    #[tokio::test]
    async fn same_task_account_while_running_is_skipped_not_queued() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let task = Arc::new(GatedTask {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });
        let (runner, _store, _sink) = build(vec![task]);
        let runner = Arc::new(runner);
        let me = account("13800000000");

        let first = {
            let runner = Arc::clone(&runner);
            let me = me.clone();
            tokio::spawn(async move { runner.run("gated", &me, &serde_json::json!({})).await })
        };
        wait_started(&started).await;

        // 同样加超时：闸若失效，这里不会返回 Skipped，而是直接进入任务体
        // 并阻塞在 release 上。没有超时的话，整条测试会挂到 CI 超时。
        let second = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            runner.run("gated", &me, &serde_json::json!({})),
        )
        .await
        .expect("第二次 run 未在 5 秒内返回：应当立即返回 Skipped，而不是排队等待")
        .unwrap();

        assert!(
            matches!(second.outcome, TaskOutcome::Skipped { .. }),
            "同一 task:account 并发时必须跳过而不是排队：{:?}",
            second.outcome
        );
        assert_eq!(second.run_id, RunId::new(0), "被跳过的运行不占用 run_id");

        release.notify_one();
        assert!(first.await.unwrap().unwrap().outcome.is_success());
    }

    /// 回归测试：任务失败时若忘了释放闸，该任务将永远无法再次运行。
    #[tokio::test]
    async fn gate_is_released_even_when_task_fails() {
        let (runner, _store, _sink) = build(vec![Arc::new(FailTask)]);
        let me = account("13800000000");

        runner
            .run("fail", &me, &serde_json::json!({}))
            .await
            .unwrap();

        assert!(
            !runner.is_running("fail", &me.id),
            "失败路径也必须释放闸，否则该任务从此再也跑不起来"
        );
    }

    // ── run_id 与历史 ───────────────────────────────────────────────────

    #[tokio::test]
    async fn run_ids_start_at_one_and_increase() {
        let (runner, _store, _sink) = build(vec![Arc::new(OkTask)]);
        let me = account("13800000000");

        let a = runner.run("ok", &me, &serde_json::json!({})).await.unwrap();
        let b = runner.run("ok", &me, &serde_json::json!({})).await.unwrap();

        assert_eq!(a.run_id, RunId::new(1));
        assert_eq!(b.run_id, RunId::new(2), "run_id 必须递增且不复用");
    }

    #[tokio::test]
    async fn seed_run_id_continues_after_restart() {
        let (runner, _store, _sink) = build(vec![Arc::new(OkTask)]);
        runner.seed_run_id(41);

        let record = runner
            .run("ok", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(
            record.run_id,
            RunId::new(42),
            "重启后应从历史最大值 +1 继续，避免与已有记录撞号"
        );
    }

    #[tokio::test]
    async fn run_is_persisted_to_history() {
        let (runner, store, _sink) = build(vec![Arc::new(OkTask)]);
        let record = runner
            .run("ok", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        let saved = store.runs(10).await.unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].run_id, record.run_id);
        assert_eq!(saved[0].task, "ok");
        assert_eq!(saved[0].account, "13800000000");
    }

    #[tokio::test]
    async fn history_write_failure_does_not_fail_the_run() {
        let store = Arc::new(FakeStore::failing());
        let (runner, _sink) = build_with(vec![Arc::new(OkTask)], Arc::clone(&store));

        let record = runner
            .run("ok", &account("13800000000"), &serde_json::json!({}))
            .await
            .expect("历史落盘失败不应让整次运行算失败");

        assert!(
            record.outcome.is_success(),
            "结果已经产生，不该被落盘失败改写"
        );
        assert!(store.runs(10).await.unwrap().is_empty());
    }

    // ── 时间来自注入的时钟 ──────────────────────────────────────────────

    #[tokio::test]
    async fn record_time_comes_from_injected_clock() {
        let (runner, _store, _sink) = build(vec![Arc::new(OkTask)]);
        let record = runner
            .run("ok", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(
            record.duration_ms, 0,
            "固定时钟不前进，时长应为 0——若这里不是 0，说明混入了真实时钟"
        );
        assert_eq!(record.started_at_ms, NOW_MS);
    }
    // ── 产物账本与收尾 ──────────────────────────────────────────────────

    /// 写文件但**不清理**的任务：收尾必须把清单交出来。
    struct LeaveArtifactsTask;

    #[async_trait]
    impl Task for LeaveArtifactsTask {
        fn name(&self) -> &'static str {
            "leaves"
        }
        fn title(&self) -> &'static str {
            "留产物"
        }
        fn description(&self) -> &'static str {
            "往网盘写两个文件且不清理"
        }
        async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            ctx.register_cloud_file("F1", false)?;
            ctx.register_cloud_file("F2", false)?;
            Ok(TaskOutcome::success("传了两个"))
        }
    }

    /// 自报一条结构化明细的任务。
    struct ReportDetailsTask;

    #[async_trait]
    impl Task for ReportDetailsTask {
        fn name(&self) -> &'static str {
            "reports"
        }
        fn title(&self) -> &'static str {
            "自报明细"
        }
        fn description(&self) -> &'static str {
            "把一份形状交回 details"
        }
        async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            ctx.details(serde_json::json!({
                "status": "ordered",
                "plan": "go",
                "code": 0,
            }));
            Ok(TaskOutcome::success("兑到了"))
        }
    }

    /// 自报**两条**明细的任务（多条要落成数组）。
    struct ReportTwoDetailsTask;

    #[async_trait]
    impl Task for ReportTwoDetailsTask {
        fn name(&self) -> &'static str {
            "reports-two"
        }
        fn title(&self) -> &'static str {
            "自报两条明细"
        }
        fn description(&self) -> &'static str {
            "两条明细要落成数组"
        }
        async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            ctx.details(serde_json::json!({ "n": 1 }));
            ctx.details(serde_json::json!({ "n": 2 }));
            Ok(TaskOutcome::success("两条"))
        }
    }

    /// 既不报明细也不留产物的任务（收尾必须**不写** details）。
    struct ReportNothingTask;

    #[async_trait]
    impl Task for ReportNothingTask {
        fn name(&self) -> &'static str {
            "reports-nothing"
        }
        fn title(&self) -> &'static str {
            "什么都不报"
        }
        fn description(&self) -> &'static str {
            "details 应保持 null"
        }
        async fn run(&self, _ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            Ok(TaskOutcome::success("没留东西"))
        }
    }

    /// 写文件**并在链尾清掉**的任务（参考实现的形状）。
    struct CleanUpTask;

    #[async_trait]
    impl Task for CleanUpTask {
        fn name(&self) -> &'static str {
            "cleans"
        }
        fn title(&self) -> &'static str {
            "传了又删"
        }
        fn description(&self) -> &'static str {
            "往网盘写一个文件然后在链尾销账"
        }
        async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            ctx.register_cloud_file("F1", false)?;
            // 链尾"发完 batchTrash"之后销账
            assert!(ctx.artifacts.mark_cloud_file_removed("F1"));
            Ok(TaskOutcome::success("传了又删了"))
        }
    }

    /// ⭐ 留下的产物必须写进运行记录 —— 否则"留了什么"永远查不出来。
    ///
    /// 旧项目只念"网盘里留了 N 个"，却不说是哪 N 个，将来手工清理无从下手。
    #[tokio::test]
    async fn leftover_artifacts_land_in_the_run_record() {
        let (runner, _store, _sink) = build(vec![Arc::new(LeaveArtifactsTask)]);
        let record = runner
            .run("leaves", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        let list = record.details["artifacts"]
            .as_array()
            .expect("details.artifacts 应当是数组");
        assert_eq!(list.len(), 2, "两个文件都要记下来：{list:?}");
        let ids: Vec<&str> = list.iter().filter_map(|a| a["id"].as_str()).collect();
        assert!(ids.contains(&"F1") && ids.contains(&"F2"), "实得：{ids:?}");
        // 账号也要在（清理要用它的凭据）
        assert_eq!(list[0]["account"], "13800000000");
        assert_eq!(list[0]["kind"], "cloud-file");
    }

    /// ⭐ 留下产物时必须**发一条警告** —— 静默是这一层最该防的失效。
    #[tokio::test]
    async fn leftover_artifacts_emit_a_warning() {
        let (runner, _store, sink) = build(vec![Arc::new(LeaveArtifactsTask)]);
        runner
            .run("leaves", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        let events = sink.events.lock().unwrap();
        let warned = events.iter().any(|e| match e {
            RunEvent::Log { level, message, .. } => {
                *level == mcloud_core::LogLevel::Warn
                    && message.contains("未清理")
                    && message.contains("F1")
            }
            _ => false,
        });
        assert!(warned, "留下产物必须告警并点名是哪几个：{events:?}");
    }

    /// ⭐ 遗留警告必须发在 `finished` **之前**。
    ///
    /// # 为什么这条值得单测
    ///
    /// 前端收到 `finished` 就 `close()` 那条 SSE（见 `web/src/api.ts` 的
    /// `subscribeRun`）。警告发在后面的话，它会落在**已经关掉的流**上 ——
    /// 直连看日志的人还能看到，但正在看页面的那个人**一个字都收不到**。
    ///
    /// 而"收不到"这件事本身不报错，所以只能靠顺序断言钉住。
    #[tokio::test]
    async fn leftover_warning_is_emitted_before_finished() {
        let (runner, _store, sink) = build(vec![Arc::new(LeaveArtifactsTask)]);
        runner
            .run("leaves", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        let names = sink.names();
        let warn_at = names
            .iter()
            .position(|n| *n == "log")
            .expect("应当有 log 事件");
        let finished_at = names
            .iter()
            .position(|n| *n == "finished")
            .expect("应当有 finished 事件");

        assert!(
            warn_at < finished_at,
            "遗留警告必须在 finished 之前（否则前端已关闭流，收不到）：{names:?}"
        );
    }

    /// 链尾清理干净的任务**不该**误报。
    ///
    /// 这条防的是反向失效：每次正常运行都报"留了东西"，
    /// 用户很快就会学会忽略那条警告 —— 那比不报更糟。
    #[tokio::test]
    async fn fully_cleaned_run_reports_nothing() {
        let (runner, _store, sink) = build(vec![Arc::new(CleanUpTask)]);
        let record = runner
            .run("cleans", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        assert!(
            record.details.is_null(),
            "清干净了就不该写 details：{:?}",
            record.details
        );
        let events = sink.events.lock().unwrap();
        assert!(
            !events.iter().any(|e| matches!(
                e,
                RunEvent::Log { message, .. } if message.contains("未清理")
            )),
            "清干净了不该告警：{events:?}"
        );
    }

    /// 收尾必须**释放**账本（否则进程跑得越久占得越多）。
    #[tokio::test]
    async fn ledger_is_released_after_each_run() {
        let (runner, _store, _sink) = build(vec![Arc::new(LeaveArtifactsTask)]);
        runner
            .run("leaves", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        let ledger = runner.artifacts();
        assert_eq!(ledger.total_pending(), 0, "运行结束后不该还占着条目");
        assert!(ledger.runs().is_empty(), "也不该还跟踪着这个 run");
    }

    /// 失败的任务留下的产物**同样**要收尾。
    ///
    /// 这条是"失败也清理"的一半：任务在中途报错时，
    /// 它之前写进网盘的东西**不能**因为异常路径就被忘掉。
    #[tokio::test]
    async fn artifacts_survive_a_failing_task() {
        struct WriteThenFail;

        #[async_trait]
        impl Task for WriteThenFail {
            fn name(&self) -> &'static str {
                "write-fail"
            }
            fn title(&self) -> &'static str {
                "写一半失败"
            }
            fn description(&self) -> &'static str {
                "写一个文件然后报错"
            }
            async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
                ctx.register_cloud_file("HALF", false)?;
                Err(mcloud_core::Error::Task {
                    task: "write-fail".into(),
                    reason: "中途断了".into(),
                })
            }
        }

        let (runner, _store, _sink) = build(vec![Arc::new(WriteThenFail)]);
        let record = runner
            .run(
                "write-fail",
                &account("13800000000"),
                &serde_json::json!({}),
            )
            .await
            .unwrap();

        assert!(record.outcome.is_failure(), "这次运行应当是失败的");
        let list = record.details["artifacts"]
            .as_array()
            .expect("失败路径也必须把留下的产物记下来");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["id"], "HALF", "那个半途写进去的文件必须可查");
    }

    /// 任务**不能**销别的账号的账（跨账号销账 = 静默漏记）。
    #[tokio::test]
    async fn task_cannot_settle_another_accounts_artifact() {
        let ledger = std::sync::Arc::new(crate::ArtifactLedger::new());
        // 另一个账号在同一次运行里留下的东西
        ledger
            .register(
                RunId(1),
                crate::Artifact::cloud_file(AccountId::new("other"), "OTHER-F"),
            )
            .unwrap();

        let rec = crate::ArtifactRecorder::new(
            std::sync::Arc::clone(&ledger),
            RunId(1),
            AccountId::new("13800000000"),
        );
        assert!(!rec.mark_cloud_file_removed("OTHER-F"));
        assert_eq!(ledger.count(RunId(1)), 1, "别人的账必须还在");
    }

    // ── 结构化明细（`RunEvent::Details` → `details.reported`）─────────────

    /// 任务自报的明细必须落到 `RunRecord.details["reported"]`。
    ///
    /// 这条盯的是 route A 的**整条链**：`ctx.details()` → 账本 →
    /// runner 收尾并进记录。断链的任一环都会让 `details` 里没有那一格，
    /// 而路由正是从那里读兑换结论的。
    #[tokio::test]
    async fn reported_details_land_in_the_run_record() {
        let (runner, _store, _sink) = build(vec![Arc::new(ReportDetailsTask)]);
        let record = runner
            .run("reports", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(
            record.details["reported"]["status"], "ordered",
            "明细应落进 details.reported：{:?}",
            record.details
        );
        assert_eq!(record.details["reported"]["code"], 0);
        // 没留产物 ⇒ 不该凭空多出 `artifacts` 那一格
        assert!(
            record.details.get("artifacts").is_none(),
            "没留产物就不该有 artifacts：{:?}",
            record.details
        );
    }

    /// 两条明细落成**数组**（一条时不裹数组，两条时是数组）。
    #[tokio::test]
    async fn two_details_become_an_array() {
        let (runner, _store, _sink) = build(vec![Arc::new(ReportTwoDetailsTask)]);
        let record = runner
            .run(
                "reports-two",
                &account("13800000000"),
                &serde_json::json!({}),
            )
            .await
            .unwrap();

        let arr = record.details["reported"]
            .as_array()
            .unwrap_or_else(|| panic!("两条应是数组：{:?}", record.details));
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["n"], 1, "按到达顺序");
        assert_eq!(arr[1]["n"], 2);
    }

    /// 既不自报也不留产物 ⇒ `details` 必须仍是 `null`。
    ///
    /// 防的是反向失效：每次正常运行都写一个空 `{}`，
    /// "没留东西"与"老数据没有这个字段"在历史里就长得一样了。
    #[tokio::test]
    async fn nothing_reported_keeps_details_null() {
        let (runner, _store, _sink) = build(vec![Arc::new(ReportNothingTask)]);
        let record = runner
            .run(
                "reports-nothing",
                &account("13800000000"),
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        assert!(
            record.details.is_null(),
            "什么都没报就不该写 details：{:?}",
            record.details
        );
    }

    /// 明细账本收尾必须**释放**（与产物账本同一口径）。
    #[tokio::test]
    async fn details_ledger_is_released_after_each_run() {
        let (runner, _store, _sink) = build(vec![Arc::new(ReportDetailsTask)]);
        runner
            .run("reports", &account("13800000000"), &serde_json::json!({}))
            .await
            .unwrap();
        assert!(
            runner.details.take(RunId(1)).is_empty(),
            "收尾已取走过，账本里不该还留着"
        );
    }
}
