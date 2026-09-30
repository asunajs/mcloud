//! 推送渠道：把一次运行的结果推到**那个账号自己配的**渠道上。
//!
//! # 配置在**账号**上，没有进程级兜底
//!
//! 收件渠道存在账号自己的 AccountSettings::notify: Option<PushConfig> 里（见
//! mcloud-core 的 account 模块与 push 模块头）。这是本仓作为**多用户服务**的
//! 硬约束：同一进程里 A、B 两个账号各配各的渠道，绝不能把 A 的渠道套到 B 的
//! 运行上。进程级推送 env 与它代表的兜底语义**已整个删除**。
//!
//! ⚠ 与 TS（mcloud-sign，单用户）的差异：它那份全局配置是为了 DRY，不是兜底。
//! 本仓多用户 ⇒ 收件地址落在账号维度；"同一份配置套多个账号"的正确形态是
//! **用户级模板**，本版没做（见 mcloud-core::push 的未做项），**没有**用
//! fallback 冒充它。
//!
//! # 挂点：装饰 app 侧的 EventSink（候选 a），不是订阅 hub（候选 b）
//!
//! - Runner 把**每一次生命周期事件**都从**同一个** Arc<dyn EventSink> 发出去
//!   （runner.rs 的 self.sink.emit(...)），Finished 是终点事件，一次运行恰好过
//!   一次。在它外面套一层 = 在唯一的出口上加旁路，mcloud-engine 一行都不用改。
//! - 订阅 hub 是**按 run 拉**的模型：要先拿到 run_id 才能 subscribe，再反复
//!   drain 等终点，还得自己处理订阅者队列容量满时"丢最新"的情况。而这里事件
//!   是**推**过来的，不丢、也不需要额外的订阅生命周期。
//! - emit 是**同步**方法，所以投递必须 tokio::spawn 出去：HTTP 慢一秒，运行
//!   路径不能跟着慢一秒。**按账号解析配置也是 async 的**（要读 store），同样
//!   不能发生在同步的 emit 里 —— 所以顺序是"先进 spawn，再 await 解析，最后
//!   投递"。这也正是"投递失败影响不到运行结果"的结构性保证。
//!
//! # 一次运行是怎么变成一条推送的
//!
//! 1. Started：给这个 run 建一份 PushLogCollector；
//! 2. Log：把每一条运行日志喂给它（error/info/warn/debug 逐字对应）；
//! 3. Finished：按**运行的那个账号**读 settings.notify —— 没有就**一个包都不发**；
//!    有就把结尾摘要补成最后一行日志（类型见 outcome_line），用 PushConfig::push_text
//!    过 onlyError + minLevel，最后 send_notify 扇出所有已配置渠道。
//!
//! # 可撤性
//!
//! 账号没配 ⇒ config_for 回 None ⇒ **连一个包都不会发出去**（http.request 一次
//! 都不调用）。删掉配置就回到"这个账号没有推送"。
//!
//! # 主动中止怎么判
//!
//! RunEvent::Finished 只带 TaskOutcome 与 duration_ms，**不带** RunStatus ——
//! 中止是在 Finished **之后**才写进 RunRecord.status 的（runner.rs：先
//! sink.emit(Finished)，再 cancel.release(run_id)，最后才盖 Cancelled）。所以
//! 事件的字段里看不出"这次是不是用户点的中止"。
//!
//! 本仓现成的判据是 CancelRegistry::is_cancelled(run_id)，它正是 runner.rs 里
//! was_cancelled（⇒ RunStatus::Cancelled）的来源。关键在**时机**：runner 是
//! "先 emit、后 release"，所以在装饰器**同步**处理 Finished 的那一刻，登记表里
//! 这一条还在、标志位还准。而 CancelRegistry 由 Runner::new 内部创建（装饰器
//! 必须先于 runner 存在，因为它就是 runner 的 sink）——因此用一个 CancelSlot
//! 槽位：runner 造好后立刻回填，AppBuilder::build 返回前一定填好，那时还没有
//! 任何一次运行开始，不存在竞态。
//!
//! 判出来是中止之后，结尾摘要记 warn（不是 error）—— see outcome_line 与
//! docs/architecture.md:452。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use mcloud_core::push::{PushConfig, PushLogCollector};
use mcloud_core::{
    AccountId, AccountStore, Clock, EventSink, HttpClient, HttpRequest, LogLevel, RunEvent, RunId,
    TaskOutcome,
};
use mcloud_engine::CancelRegistry;

use crate::push::{PushContext, PushRequest, PushResponse, PushTransport};

/// PushTransport 适配现有的 HttpClient。
///
/// 推送的 10s 超时必须在这一层真正生效：HttpClient::request 的签名里没有超时
/// 参数，所以用 tokio::time::timeout 包住 —— 这正是 PushRequest::timeout_ms 存在
/// 的理由（见 mcloud-app::push 模块头）。
struct HttpPushTransport {
    http: Arc<dyn HttpClient>,
}

#[async_trait::async_trait]
impl PushTransport for HttpPushTransport {
    async fn send(&self, req: PushRequest) -> anyhow::Result<PushResponse> {
        let http_req = HttpRequest {
            method: req.method.clone(),
            url: req.url.clone(),
            headers: req.headers.clone(),
            body: req.body_bytes(),
        };
        let timeout = Duration::from_millis(req.timeout_ms);
        match tokio::time::timeout(timeout, self.http.request(http_req)).await {
            Ok(Ok(resp)) => Ok(PushResponse {
                status: resp.status,
                body: resp.body,
            }),
            Ok(Err(error)) => Err(anyhow::anyhow!(error.to_string())),
            Err(_) => Err(anyhow::anyhow!("推送请求超时（{}ms）", req.timeout_ms)),
        }
    }
}

/// 投递器：按账号读配置，然后把整次运行的日志推出去。
///
/// ⚠ 它只依赖 AccountStore（Store 的子集），因为投递只需要读账号这一件事。
/// 这与 mcloud-core::traits 说的拆子 trait 的理由一致：调用方不该被迫依赖它
/// 用不到的存储能力，测试里也只造需要的那一个假实现。
pub struct Notifier {
    store: Arc<dyn AccountStore>,
    http: Arc<dyn HttpClient>,
    clock: Arc<dyn Clock>,
}

impl Notifier {
    pub fn new(
        store: Arc<dyn AccountStore>,
        http: Arc<dyn HttpClient>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { store, http, clock }
    }

    /// 读**这个账号**的推送配置。
    ///
    /// 账号查不到 / 查询失败 ⇒ **只 warn** 并回 None（⇒ 一个包都不发）。
    /// 绝不 panic、绝不冒泡：账号库读不出来不该让一次运行变成"没有结果"，
    /// 更不该把整个进程带崩。**没有兜底** —— 这正是"账号没配就零行为"的落点。
    async fn config_for(&self, account: &str) -> Option<PushConfig> {
        let id = AccountId::new(account);
        match self.store.account(&id).await {
            Ok(Some(acc)) => acc.settings.notify,
            Ok(None) => {
                tracing::warn!(account, "账号不在库里，本次不发推送");
                None
            }
            Err(e) => {
                tracing::warn!(account, error = %e, "读账号推送配置失败，本次不发推送");
                None
            }
        }
    }

    /// 投递一次运行的结果。
    ///
    /// **刻意不返回 Result**：非 2xx、网络错、超时都只落进推送日志（各 send_*
    /// 内部消化）。这是"投递只是旁路"的落点 —— 通知失败不能反过来影响运行结果。
    async fn deliver(&self, run: FinishedRun) {
        let FinishedRun {
            run_id,
            meta,
            outcome,
            duration_ms,
            cancelled,
            logs,
        } = run;

        // ① 按**运行的那个账号**读配置；没有 ⇒ 一个包都不发（无兜底）。
        //    这个 await 发生在 spawn 出去的独立任务上，不在同步的 emit 里，
        //    它的延迟不算进运行收尾。
        let Some(config) = self.config_for(&meta.account).await else {
            return;
        };

        // ② 把结尾摘要补成日志的最后一行 —— 它就是旧载荷里 severity + summary
        //    的接替者：类型决定 onlyError 会不会触发（见 outcome_line）。
        let (kind, summary) = outcome_line(&outcome, cancelled);
        logs.write(
            kind,
            format!(
                "{}@{} run#{} 用时{}ms：{}",
                meta.task,
                meta.account,
                run_id.get(),
                duration_ms,
                summary
            ),
            self.clock.now_ms(),
        );

        // ③ onlyError 且本次**没有任何 error 级**日志 ⇒ 不发；minLevel 过滤也
        //    在这里（push_text 把 onlyError 与 minLevel 两步收成一个函数）。
        let Some(text) = config.push_text(&logs) else {
            tracing::debug!(
                run = run_id.get(),
                "不满足推送条件（onlyError/无日志），不发"
            );
            return;
        };

        // ④ 扇出所有已配置渠道。标题用配置里的（默认 mcloud 运行推送）。
        let transport = HttpPushTransport {
            http: Arc::clone(&self.http),
        };
        let ctx = PushContext::new(&transport, self.clock.as_ref(), &logs);
        let title = config.title.clone();
        crate::push::send_notify(&ctx, &config, &title, &text).await;
    }
}

/// 取消登记表的**迟到句柄**。
///
/// 装饰器必须先于 Runner 存在（Runner 拿它当 sink），而登记表是 Runner::new
/// 内部造的 —— 所以先给一个空槽位，runner 造好后立刻回填。AppBuilder::build
/// 在返回前一定填好，而那时还没有任何一次运行开始，因此不需要额外的同步。
pub type CancelSlot = Arc<OnceLock<Arc<CancelRegistry>>>;

/// 空槽位。
pub fn cancel_slot() -> CancelSlot {
    Arc::new(OnceLock::new())
}

/// 结尾摘要的日志类型 —— 旧 Severity 的语义接替者。
///
/// | TaskOutcome / 是否中止 | 类型 | 效果 |
/// |---|---|---|
/// | Success | success（level 3） | 不报红 |
/// | Skipped | info（level 3） | **不算错误**（docs/conventions.md:301） |
/// | Failed | error（level 0） | onlyError 会触发；正文里报红 |
/// | 用户主动中止 | warn（level 1） | **不许是 error**（docs/architecture.md:452） |
///
/// 中止**优先于** TaskOutcome：被中止的运行业务上多半是 Failed（任务在安全点
/// check_cancelled()? 退出），但那不是要报红的失败 —— 那正是"用户自己点的中止，
/// 推送里也不该报红"。
fn outcome_line(outcome: &TaskOutcome, cancelled: bool) -> (&'static str, String) {
    if cancelled {
        return ("warn", format!("已中止：{}", outcome.summary()));
    }
    match outcome {
        TaskOutcome::Success { summary } => ("success", summary.clone()),
        TaskOutcome::Skipped { reason } => ("info", format!("跳过：{reason}")),
        TaskOutcome::Failed { reason } => ("error", format!("失败：{reason}")),
    }
}

/// 运行日志级别 → 推送日志类型。
///
/// 两个枚举的取值刻意一一对应，于是只有真正的 error 运行日志（以及 Failed 的
/// 结尾摘要）会让 PushLogCollector::has_error 为真、让 onlyError 触发。
fn log_type(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

/// 一次运行的身份，来自 Started，供 Finished 组装推送。
#[derive(Debug, Clone, Default)]
struct RunMeta {
    task: String,
    account: String,
}

/// 在跑的一次运行：身份 + 它自己的日志收集器。
struct RunState {
    meta: RunMeta,
    logs: Arc<PushLogCollector>,
}

/// Finished 那一刻定下来、交给 spawn 出去的任务去做投递的一组值。
struct FinishedRun {
    run_id: RunId,
    meta: RunMeta,
    outcome: TaskOutcome,
    duration_ms: u64,
    cancelled: bool,
    logs: Arc<PushLogCollector>,
}

/// 事件旁路：转发给内层 sink，同时收集日志、在 Finished 触发投递。
pub struct NotifySink {
    inner: Arc<dyn EventSink>,
    notifier: Arc<Notifier>,
    cancel: CancelSlot,
    /// 在跑的运行：run_id → (身份, 它自己的日志收集器)。
    ///
    /// 只在内存里攒、Finished 时删。Started 与 Finished 都从 Runner 的同一个
    /// sink 过，所以这一对一定成对出现（同一 task:account 已在运行时 runner
    /// 直接返回、两个事件都不发）。因此表的大小 = 并发在跑的 run 数。
    started: Mutex<HashMap<RunId, RunState>>,
}

impl NotifySink {
    pub fn new(inner: Arc<dyn EventSink>, notifier: Arc<Notifier>, cancel: CancelSlot) -> Self {
        Self {
            inner,
            notifier,
            cancel,
            started: Mutex::new(HashMap::new()),
        }
    }

    /// 锁中毒时取回内部值继续用（与 EventHub 同一理由：每次操作在锁内一次做完，
    /// 没有"写了一半"的中间态，跟着 panic 只会让整个服务停摆）。
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RunId, RunState>> {
        self.started.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventSink for NotifySink {
    fn emit(&self, event: RunEvent) {
        // 先做推送侧的动作，再把事件**原样**转给内层。
        //
        // 判据（是否主动中止）依赖"runner 还没 release 取消登记表"——那与转发
        // 顺序无关：整段都在同一个同步 emit 调用里完成，而 runner 的 release 在
        // emit 返回之后。
        match &event {
            RunEvent::Started {
                run_id,
                task,
                account,
                ..
            } => {
                self.lock().insert(
                    *run_id,
                    RunState {
                        meta: RunMeta {
                            task: task.clone(),
                            account: account.clone(),
                        },
                        logs: Arc::new(PushLogCollector::new()),
                    },
                );
            }
            RunEvent::Log {
                run_id,
                level,
                message,
                at_ms,
            } => {
                if let Some(state) = self.lock().get(run_id) {
                    state.logs.write(log_type(*level), message.clone(), *at_ms);
                }
            }
            RunEvent::Finished {
                run_id,
                outcome,
                duration_ms,
            } => {
                let id = *run_id;
                let cancelled = self
                    .cancel
                    .get()
                    .is_some_and(|registry| registry.is_cancelled(id));
                // Started 缺席（例如回放）也给一份空身份 + 空收集器：账号读不到
                // ⇒ 不发。绝不 panic。
                let state = self.lock().remove(&id).unwrap_or_else(|| RunState {
                    meta: RunMeta::default(),
                    logs: Arc::new(PushLogCollector::new()),
                });
                let run = FinishedRun {
                    run_id: id,
                    meta: state.meta,
                    outcome: outcome.clone(),
                    duration_ms: *duration_ms,
                    cancelled,
                    logs: state.logs,
                };

                let notifier = Arc::clone(&self.notifier);
                match tokio::runtime::Handle::try_current() {
                    // 必须 spawn：emit 是同步的，而"读账号配置"与 HTTP 都是异步的；
                    // 在这里等待就把这两段延迟算进了运行收尾。解析与投递都在任务
                    // 内做，任何失败都不会回到运行路径上。
                    Ok(handle) => {
                        handle.spawn(async move { notifier.deliver(run).await });
                    }
                    // 没有运行时（例如脱离 tokio 直接调 sink）：只记一条日志，
                    // 绝不 panic。
                    Err(_) => {
                        tracing::warn!(run = id.get(), "没有 tokio 运行时，推送跳过");
                    }
                }
            }
            _ => {}
        }

        self.inner.emit(event);
    }
}

/// 给 inner 套上推送旁路。
///
/// ⚠ **总是套一层**：渠道是**每账号**的，地址在投递那一刻才从该账号的
/// settings 读（账号级配置还能在运行时改）。所以不能因为"启动时看不出有配置"
/// 就跳过装饰 —— 那样账号级配置永远不会生效。
///
/// "未配置就零行为"由 Notifier::config_for 兜住：账号没配 ⇒ 回 None ⇒ 一个包
/// 都不发。**没有**进程级兜底可回落。
pub(crate) fn decorate(
    inner: Arc<dyn EventSink>,
    store: Arc<dyn AccountStore>,
    http: Arc<dyn HttpClient>,
    clock: Arc<dyn Clock>,
    cancel: CancelSlot,
) -> Arc<dyn EventSink> {
    let notifier = Arc::new(Notifier::new(store, http, clock));
    Arc::new(NotifySink::new(inner, notifier, cancel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::testkit::FixedClock;
    use mcloud_core::{
        Account, AccountSettings, AuthInfo, DeviceId, HttpResponse, Result as CoreResult,
    };
    use serde_json::Value;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW_MS: i64 = 1_700_000_000_000;

    /// 假传输排队的一条应答。
    #[derive(Debug, Clone, Copy)]
    enum Reply {
        Http(u16),
        Fail,
    }

    /// 记录每一枪、并按队列给应答的假 HttpClient。
    struct FakeHttp {
        replies: Mutex<VecDeque<Reply>>,
        seen: Mutex<Vec<HttpRequest>>,
        calls: AtomicUsize,
    }

    impl FakeHttp {
        fn new() -> Arc<Self> {
            Self::with(Vec::new())
        }

        fn with(replies: Vec<Reply>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn requests(&self) -> Vec<HttpRequest> {
            self.seen.lock().unwrap().clone()
        }

        fn urls(&self) -> Vec<String> {
            self.requests().into_iter().map(|r| r.url).collect()
        }

        /// 第 index 枪的体，解析成 JSON。
        fn body_at(&self, index: usize) -> Value {
            let raw = self.requests()[index]
                .body_str()
                .expect("体应是 UTF-8")
                .to_string();
            serde_json::from_str(&raw).expect("体应是合法 JSON")
        }
    }

    #[async_trait::async_trait]
    impl HttpClient for FakeHttp {
        async fn request(&self, req: HttpRequest) -> CoreResult<HttpResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(req);
            match self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Reply::Http(200))
            {
                Reply::Http(status) => Ok(HttpResponse {
                    status,
                    body: "{}".into(),
                    headers: Vec::new(),
                }),
                Reply::Fail => Err(mcloud_core::Error::Other("网络断了".into())),
            }
        }
    }

    /// 账号配置的替身：只实现 AccountStore（notify 只读账号这一件事）。
    ///
    /// fail 为真 ⇒ account() 一律报错 —— 专门钉"查询失败不许 panic、不许影响
    /// 运行、且**没有兜底**可回落"。
    #[derive(Default)]
    struct FakeStore {
        accounts: HashMap<String, Account>,
        fail: bool,
    }

    impl FakeStore {
        fn with(accounts: Vec<Account>) -> Self {
            Self {
                accounts: accounts.into_iter().map(|a| (a.id.0.clone(), a)).collect(),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                accounts: HashMap::new(),
                fail: true,
            }
        }

        fn arc(self) -> Arc<dyn AccountStore> {
            Arc::new(self)
        }
    }

    fn refuse() -> mcloud_core::Error {
        mcloud_core::Error::Other("这条链不该写账号库".into())
    }

    #[async_trait::async_trait]
    impl AccountStore for FakeStore {
        async fn accounts(&self) -> CoreResult<Vec<Account>> {
            if self.fail {
                return Err(refuse());
            }
            Ok(self.accounts.values().cloned().collect())
        }

        async fn account(&self, id: &AccountId) -> CoreResult<Option<Account>> {
            if self.fail {
                return Err(refuse());
            }
            Ok(self.accounts.get(id.as_str()).cloned())
        }

        async fn save_account(&self, _a: &Account) -> CoreResult<()> {
            Err(refuse())
        }

        async fn remove_account(&self, _id: &AccountId) -> CoreResult<()> {
            Err(refuse())
        }

        async fn account_state(&self, _id: &AccountId) -> CoreResult<serde_json::Value> {
            Err(refuse())
        }

        async fn save_account_state(
            &self,
            _id: &AccountId,
            _s: &serde_json::Value,
        ) -> CoreResult<()> {
            Err(refuse())
        }
    }

    /// 造一个账号，只指定它的推送配置。
    fn account_with(id: &str, notify: Option<PushConfig>) -> Account {
        Account {
            id: AccountId::new(id),
            nickname: None,
            auth: AuthInfo {
                raw: "x".into(),
                platform: "android".into(),
                phone: id.into(),
                token: "t".into(),
                expire: 0,
            },
            device_id: DeviceId("d".into()),
            pc_platform: None,
            user_domain_id: None,
            owner: None,
            settings: AccountSettings {
                notify,
                ..Default::default()
            },
        }
    }

    /// 解析一份 PushConfig。
    fn cfg(raw: &str) -> PushConfig {
        serde_json::from_str(raw).expect("测试配置必须能解析")
    }

    /// 只收集事件的内层 sink。
    #[derive(Default)]
    struct CollectSink {
        events: Mutex<Vec<RunEvent>>,
    }

    impl CollectSink {
        fn len(&self) -> usize {
            self.events.lock().unwrap().len()
        }

        fn last(&self) -> Option<RunEvent> {
            self.events.lock().unwrap().last().cloned()
        }
    }

    impl EventSink for CollectSink {
        fn emit(&self, event: RunEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn started(run_id: u64, task: &str, account: &str) -> RunEvent {
        RunEvent::Started {
            run_id: RunId::new(run_id),
            task: task.into(),
            account: account.into(),
            at_ms: NOW_MS,
        }
    }

    fn logged(run_id: u64, level: LogLevel, message: &str) -> RunEvent {
        RunEvent::Log {
            run_id: RunId::new(run_id),
            level,
            message: message.into(),
            at_ms: NOW_MS,
        }
    }

    fn finished(run_id: u64, outcome: TaskOutcome, duration_ms: u64) -> RunEvent {
        RunEvent::Finished {
            run_id: RunId::new(run_id),
            outcome,
            duration_ms,
        }
    }

    fn clock() -> Arc<dyn Clock> {
        Arc::new(FixedClock(NOW_MS))
    }

    /// 完整可控的旁路 sink（**没有**兜底参数 —— 它已经不存在了）。
    fn sink_for(
        http: Arc<FakeHttp>,
        store: Arc<dyn AccountStore>,
        cancel: Option<Arc<CancelRegistry>>,
    ) -> (Arc<dyn EventSink>, Arc<CollectSink>) {
        let inner = Arc::new(CollectSink::default());
        let slot = cancel_slot();
        if let Some(registry) = cancel {
            slot.set(registry).expect("空槽位应当设置成功");
        }
        let notifier = Arc::new(Notifier::new(store, http as Arc<dyn HttpClient>, clock()));
        let sink: Arc<dyn EventSink> = Arc::new(NotifySink::new(
            Arc::clone(&inner) as Arc<dyn EventSink>,
            notifier,
            slot,
        ));
        (sink, inner)
    }

    /// 等 spawn 出去的投递跑完。
    async fn wait_calls(http: &FakeHttp, n: usize) {
        for _ in 0..1000 {
            if http.calls() >= n {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("等待投递完成超时（期望 {n} 枪）");
    }

    /// 给 spawn 出去的任务一点时间，然后断言**一个包都没有**。
    async fn expect_no_calls(http: &FakeHttp) {
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(http.calls(), 0, "没配推送渠道就不该有任何投递");
    }

    // ── (a) 每账号两份配置、两枪 ────────────────────────────────────────

    /// (a) 账号配了两条渠道（serverChan + workWeixinBot）⇒ 两枪都发、
    /// 各自 URL / body 对。
    #[tokio::test]
    async fn account_with_two_channels_fires_both_with_correct_requests() {
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(
                r#"{"serverChan":{"token":"SC"},"workWeixinBot":{"url":"https://qyapi.test/hook?key=K"}}"#,
            )),
        )])
        .arc();
        let (sink, inner) = sink_for(Arc::clone(&http), store, None);

        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(
            1,
            TaskOutcome::success("签到成功，获得 3 AI豆"),
            10,
        ));
        wait_calls(&http, 2).await;

        assert_eq!(http.calls(), 2, "两条渠道各一枪");
        assert_eq!(http.urls()[0], "https://sctapi.ftqq.com/SC.send");
        let sc = http.body_at(0);
        assert_eq!(sc["text"], "mcloud 运行推送", "默认标题");
        assert!(
            sc["desp"]
                .as_str()
                .unwrap()
                .contains("签到成功，获得 3 AI豆"),
            "Server 酱正文要带结尾摘要：{sc}"
        );
        assert_eq!(http.urls()[1], "https://qyapi.test/hook?key=K");
        let wx = http.body_at(1);
        assert_eq!(wx["msgtype"], "text");
        assert!(
            wx["text"]["content"].as_str().unwrap().contains("签到成功"),
            "企业微信正文要带摘要：{wx}"
        );
        assert_eq!(inner.len(), 2, "事件要原样往下走");
    }

    /// 标题用账号配置里的 title（不是硬编码）。
    #[tokio::test]
    async fn configured_title_is_used() {
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"title":"我的标题","serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink, _) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::success("好了"), 1));
        wait_calls(&http, 1).await;
        let body = http.body_at(0);
        assert_eq!(body["title"], "我的标题");
        assert_eq!(body["text"], "我的标题");
    }

    /// 结尾摘要带运行身份（旧载荷的 task/account/run/duration 语义接替者）。
    #[tokio::test]
    async fn ending_summary_carries_run_identity() {
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink, _) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(7, "sign", "19900000001"));
        sink.emit(finished(7, TaskOutcome::success("好了"), 1500));
        wait_calls(&http, 1).await;
        let body = http.body_at(0);
        let desp = body["desp"].as_str().unwrap();
        assert!(desp.contains("sign@19900000001"), "任务/账号要在：{desp}");
        assert!(desp.contains("run#7"), "run 号要在：{desp}");
        assert!(desp.contains("1500ms"), "时长要在：{desp}");
    }

    /// 渠道凭证只该在自己那一格（URL），不该被写进正文。
    #[tokio::test]
    async fn channel_credential_is_not_in_the_body() {
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"serverChan":{"token":"SC-SECRET"}}"#)),
        )])
        .arc();
        let (sink, _) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::success("好了"), 1));
        wait_calls(&http, 1).await;
        assert!(http.urls()[0].contains("SC-SECRET"), "凭证在 URL 里");
        let raw = serde_json::to_string(&http.body_at(0)).unwrap();
        assert!(!raw.contains("SC-SECRET"), "凭证不许进正文：{raw}");
    }

    // ── (b)(c) 没配 / 查询失败 ⇒ 0 枪 ───────────────────────────────────

    /// (b) 账号没配（None，或库里压根没有这个账号）⇒ **0 枪**（无兜底）。
    #[tokio::test]
    async fn account_without_push_config_delivers_nothing() {
        for store in [
            FakeStore::default().arc(),
            FakeStore::with(vec![account_with("19900000001", None)]).arc(),
        ] {
            let http = FakeHttp::new();
            let (sink, inner) = sink_for(Arc::clone(&http), store, None);

            sink.emit(started(1, "sign", "19900000001"));
            sink.emit(finished(1, TaskOutcome::failed("boom"), 10));
            expect_no_calls(&http).await;

            assert_eq!(inner.len(), 2, "事件仍要原样往下走");
            assert!(matches!(inner.last(), Some(RunEvent::Finished { .. })));
        }
    }

    /// (c) 账号查询失败 ⇒ warn + **0 枪**、事件照常转发、不 panic（无兜底）。
    #[tokio::test]
    async fn store_error_warns_and_delivers_nothing() {
        let http = FakeHttp::new();
        let (sink, inner) = sink_for(Arc::clone(&http), FakeStore::failing().arc(), None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::failed("boom"), 10));
        expect_no_calls(&http).await;
        assert_eq!(inner.len(), 2, "查询失败不许影响事件转发");
        assert!(matches!(inner.last(), Some(RunEvent::Finished { .. })));
    }

    // ── (d) onlyError / minLevel ────────────────────────────────────────

    /// (d) onlyError：本次全 info ⇒ 0 枪；含 error ⇒ 发，且 minLevel=error 时
    /// 正文里只剩 error 行。
    #[tokio::test]
    async fn only_error_gates_push_and_min_level_filters_text() {
        // 本次全 info（结尾 success 也是非 error）⇒ 不推
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"onlyError":true,"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink, _) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(logged(1, LogLevel::Info, "普通日志"));
        sink.emit(finished(1, TaskOutcome::success("搞定"), 10));
        expect_no_calls(&http).await;

        // 含 error 且 minLevel=error ⇒ 发，正文只剩 error 级那几行
        let http2 = FakeHttp::new();
        let store2 = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(
                r#"{"onlyError":true,"minLevel":"error","serverChan":{"token":"SC"}}"#,
            )),
        )])
        .arc();
        let (sink2, _) = sink_for(Arc::clone(&http2), store2, None);
        sink2.emit(started(1, "sign", "19900000001"));
        sink2.emit(logged(1, LogLevel::Info, "普通日志"));
        sink2.emit(logged(1, LogLevel::Error, "炸了"));
        sink2.emit(finished(1, TaskOutcome::success("搞定"), 10));
        wait_calls(&http2, 1).await;

        let body = http2.body_at(0);
        let desp = body["desp"].as_str().unwrap();
        assert!(desp.contains("炸了"), "error 行要在：{desp}");
        assert!(
            !desp.contains("普通日志"),
            "minLevel=error 时不该留 info 行：{desp}"
        );
        assert!(
            !desp.contains("搞定"),
            "success 摘要（level 3）也要被 error 过滤掉：{desp}"
        );
    }

    // ── (e) 一个渠道失败不影响其余 ──────────────────────────────────────

    /// (e) 一条渠道非 2xx ⇒ 其余渠道照常发、不影响运行结果。
    #[tokio::test]
    async fn one_channel_failure_does_not_stop_the_others() {
        let http = FakeHttp::with(vec![Reply::Http(500), Reply::Http(200)]);
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(
                r#"{"serverChan":{"token":"SC"},"workWeixinBot":{"url":"https://qyapi.test/hook"}}"#,
            )),
        )])
        .arc();
        let (sink, inner) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::failed("boom"), 10));
        wait_calls(&http, 2).await;

        assert_eq!(http.calls(), 2, "第一条失败不妨碍第二条");
        let urls = http.urls();
        assert!(urls[0].contains("sctapi.ftqq.com"));
        assert!(urls[1].contains("qyapi.test"));
        assert_eq!(inner.len(), 2, "运行结果一点没受影响");
    }

    /// (e 补充) 一条渠道**传输层抛错** ⇒ 同样不影响其余。
    #[tokio::test]
    async fn one_channel_transport_error_does_not_stop_the_others() {
        let http = FakeHttp::with(vec![Reply::Fail, Reply::Http(200)]);
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(
                r#"{"serverChan":{"token":"SC"},"workWeixinBot":{"url":"https://qyapi.test/hook"}}"#,
            )),
        )])
        .arc();
        let (sink, inner) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::failed("boom"), 10));
        wait_calls(&http, 2).await;
        assert_eq!(http.calls(), 2, "抛错也吃掉，不影响第二条");
        assert_eq!(inner.len(), 2);
    }

    // ── (f) 用户主动中止 / Skipped 不算错误 ─────────────────────────────

    /// (f) 用户主动中止 ⇒ 结尾摘要**不是 error 级**；onlyError 下不推。
    ///
    /// 对应 docs/architecture.md:452「用户自己点的中止，推送里也不该报红」。
    #[tokio::test]
    async fn user_cancelled_run_is_not_error_and_only_error_skips_it() {
        let registry = Arc::new(CancelRegistry::new());
        registry.register(RunId::new(42)).cancel();

        // onlyError=true：本次只跑了「开始 + 用户中止」⇒ 0 枪
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"onlyError":true,"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink, _) = sink_for(Arc::clone(&http), store, Some(Arc::clone(&registry)));
        sink.emit(started(42, "sign", "19900000001"));
        sink.emit(finished(42, TaskOutcome::failed("已中止"), 300));
        expect_no_calls(&http).await;

        // 对照（onlyError=false）：正文里出现「已中止」，且**不是** error 级
        let http2 = FakeHttp::new();
        let store2 = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink2, _) = sink_for(Arc::clone(&http2), store2, Some(Arc::clone(&registry)));
        sink2.emit(started(42, "sign", "19900000001"));
        sink2.emit(finished(42, TaskOutcome::failed("已中止"), 300));
        wait_calls(&http2, 1).await;

        let body = http2.body_at(0);
        let desp = body["desp"].as_str().unwrap();
        assert!(desp.contains("已中止"), "摘要要进正文：{desp}");
        assert!(
            !desp.contains("[error"),
            "用户主动中止的结尾摘要不许是 error 级：{desp}"
        );
        assert!(desp.contains("[warn"), "中止记成 warn：{desp}");
    }

    /// (f 补充) Skipped **不算错误**（docs/conventions.md:301）：onlyError 下不推，
    /// 正文里也不是 error 级。
    #[tokio::test]
    async fn skipped_is_not_error_and_only_error_skips_it() {
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"onlyError":true,"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink, _) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::skipped("已签到"), 5));
        expect_no_calls(&http).await;

        let http2 = FakeHttp::new();
        let store2 = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink2, _) = sink_for(Arc::clone(&http2), store2, None);
        sink2.emit(started(1, "sign", "19900000001"));
        sink2.emit(finished(1, TaskOutcome::skipped("已签到"), 5));
        wait_calls(&http2, 1).await;

        let body = http2.body_at(0);
        let desp = body["desp"].as_str().unwrap();
        assert!(desp.contains("[info"), "跳过记成 info：{desp}");
        assert!(!desp.contains("[error"), "跳过不是错误：{desp}");
    }

    // ── 旁路保证 ────────────────────────────────────────────────────────

    /// 投递失败 ⇒ 不影响运行结果、不 panic。
    #[tokio::test]
    async fn delivery_failure_does_not_affect_the_run() {
        let http = FakeHttp::with(vec![Reply::Fail]);
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink, inner) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::failed("boom"), 5));
        wait_calls(&http, 1).await;

        assert_eq!(http.calls(), 1);
        assert_eq!(inner.len(), 2);
        assert!(matches!(
            inner.last(),
            Some(RunEvent::Finished {
                outcome: TaskOutcome::Failed { .. },
                ..
            })
        ));
    }

    /// 日志按 run 分桶：run1 的日志不会跑进 run2 的推送正文。
    #[tokio::test]
    async fn logs_are_scoped_per_run() {
        let http = FakeHttp::new();
        let store = FakeStore::with(vec![account_with(
            "19900000001",
            Some(cfg(r#"{"serverChan":{"token":"SC"}}"#)),
        )])
        .arc();
        let (sink, _) = sink_for(Arc::clone(&http), store, None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(logged(1, LogLevel::Info, "run1 的日志"));
        sink.emit(started(2, "sign", "19900000001"));
        sink.emit(logged(2, LogLevel::Info, "run2 的日志"));
        sink.emit(finished(1, TaskOutcome::success("一"), 1));
        sink.emit(finished(2, TaskOutcome::success("二"), 2));
        wait_calls(&http, 2).await;

        let bodies: Vec<String> = (0..2)
            .map(|i| http.body_at(i)["desp"].as_str().unwrap().to_string())
            .collect();
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("run1 的日志") && !b.contains("run2 的日志")),
            "run1 的正文只能有 run1 的日志：{bodies:?}"
        );
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("run2 的日志") && !b.contains("run1 的日志")),
            "run2 的正文只能有 run2 的日志：{bodies:?}"
        );
    }

    /// 组装走的是同一个 decorate：没配的账号不会投递（未配置就零行为）。
    ///
    /// decorate **总是**装一层 —— 账号配置运行时可变，不能凭启动时是否配置决定。
    #[tokio::test]
    async fn unconfigured_wiring_never_delivers() {
        let http = FakeHttp::new();
        let inner = Arc::new(CollectSink::default());
        let store = FakeStore::with(vec![account_with("19900000001", None)]).arc();
        let sink = decorate(
            Arc::clone(&inner) as Arc<dyn EventSink>,
            store,
            Arc::clone(&http) as Arc<dyn HttpClient>,
            clock(),
            cancel_slot(),
        );
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::failed("boom"), 10));
        assert_eq!(inner.len(), 2);
        expect_no_calls(&http).await;
    }

    /// outcome_line 的四档映射（旧 Severity::for_outcome 的接替者）逐条钉住。
    #[test]
    fn outcome_line_maps_like_the_old_severity() {
        assert_eq!(
            outcome_line(&TaskOutcome::success("好了"), false).0,
            "success"
        );
        assert_eq!(
            outcome_line(&TaskOutcome::skipped("已签到"), false).0,
            "info"
        );
        assert_eq!(outcome_line(&TaskOutcome::failed("炸了"), false).0, "error");
        // 中止优先于 TaskOutcome：即便业务结论是 Failed，也不许是 error
        assert_eq!(outcome_line(&TaskOutcome::failed("已中止"), true).0, "warn");
        assert_eq!(
            outcome_line(&TaskOutcome::success("居然成功"), true).0,
            "warn"
        );
    }

    /// 运行日志级别 → 推送类型的映射：只有 Error 会变成 error。
    #[test]
    fn log_level_maps_to_push_type() {
        assert_eq!(log_type(LogLevel::Debug), "debug");
        assert_eq!(log_type(LogLevel::Info), "info");
        assert_eq!(log_type(LogLevel::Warn), "warn");
        assert_eq!(log_type(LogLevel::Error), "error");
    }
}
