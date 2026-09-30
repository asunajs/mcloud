//! 推送渠道：把**运行结果**投递到一个可配置的 webhook。
//!
//! # 挂点：装饰 app 侧的 `EventSink`（候选 a），不是订阅 hub（候选 b）
//!
//! - `Runner` 把**每一次生命周期事件**都从**同一个** `Arc<dyn EventSink>` 发出去
//!   （`runner.rs` 的 `self.sink.emit(...)`），`Finished` 是终点事件，一次运行
//!   恰好过一次。在它外面套一层 = 在唯一的出口上加旁路，`mcloud-engine`
//!   一行都不用改。
//! - 订阅 hub 是**按 run 拉**的模型：要先拿到 `run_id` 才能 `subscribe`，再反复
//!   `drain` 等终点，还得自己处理订阅者队列容量满时"丢最新"的情况。而这里
//!   事件是**推**过来的，不丢、也不需要额外的订阅生命周期。
//! - `emit` 是**同步**方法，所以投递必须 `tokio::spawn` 出去：HTTP 慢一秒，
//!   运行路径不能跟着慢一秒。这也正是"投递失败影响不到运行结果"的结构性保证
//!   —— 投递跑在**另一条任务**上，它的成败与运行收尾无关。
//!
//! # 可撤性
//!
//! 没配 `MCLOUD_NOTIFY_WEBHOOK` ⇒ [`decorate`] 原样返回内层 sink，连 `Notifier`
//! 都不会构造。删掉配置就回到"完全没有这个功能"的样子。
//!
//! # 主动中止怎么判
//!
//! `RunEvent::Finished` 只带 `TaskOutcome` 与 `duration_ms`，**不带** `RunStatus`
//! —— 中止是在 `Finished` **之后**才写进 `RunRecord.status` 的（`runner.rs`：
//! 先 `sink.emit(Finished)`，再 `cancel.release(run_id)`，最后才盖 `Cancelled`）。
//! 所以事件的字段里看不出"这次是不是用户点的中止"。
//!
//! 本仓现成的判据是 `CancelRegistry::is_cancelled(run_id)`，它正是 `runner.rs`
//! 里 `was_cancelled`（⇒ `RunStatus::Cancelled`）的来源。关键在**时机**：runner
//! 是"先 emit、后 release"，所以在装饰器**同步**处理 `Finished` 的那一刻，
//! 登记表里这一条还在、标志位还准。而 `CancelRegistry` 由 `Runner::new` 内部创建
//! （装饰器必须先于 runner 存在，因为它就是 runner 的 sink）——因此用一个
//! [`CancelSlot`] 槽位：runner 造好后立刻回填，`AppBuilder::build` 返回前一定填好，
//! 那时还没有任何一次运行开始，不存在竞态。
//!
//! ⚠ 槽位没接或查不到时，退回**只看 `TaskOutcome` 三态**。宁可把一次中止报成
//! `failed`（少一次"不报红"），也不去猜一个不存在的字段。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use mcloud_core::{Clock, EventSink, HttpClient, HttpRequest, RunEvent, RunId, TaskOutcome};
use mcloud_engine::CancelRegistry;
use serde::Serialize;

/// 载荷里的来源标识。
///
/// 同一个 webhook 可能接了多家，收方据此分流。
pub const SOURCE: &str = "mcloud-gost";

/// 单次投递的超时。
///
/// 有它，spawn 出去的任务才不会在"对端接了连接却不回话"时永远挂着。
pub const DELIVER_TIMEOUT: Duration = Duration::from_secs(10);

/// 严重级。
///
/// 与 `TaskOutcome` / 主动中止的映射见 [`Severity::for_outcome`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Skipped,
    Failed,
    Cancelled,
}

impl Severity {
    /// 由业务结论 + 「是否用户主动中止」共同决定。
    ///
    /// `cancelled` **优先于** `TaskOutcome`：被中止的运行，业务结论多半是
    /// `Failed`（任务在安全点 `check_cancelled()?` 退出），但那不是要报红的
    /// 失败 —— `docs/architecture.md:452` 写明"用户自己点的中止……推送里
    /// 也不该报红"。先判中止，才不会把它误报成 failed。
    pub fn for_outcome(outcome: &TaskOutcome, cancelled: bool) -> Self {
        if cancelled {
            return Severity::Cancelled;
        }
        match outcome {
            TaskOutcome::Success { .. } => Severity::Ok,
            TaskOutcome::Skipped { .. } => Severity::Skipped,
            TaskOutcome::Failed { .. } => Severity::Failed,
        }
    }

    /// 是否属于"要报红/要告警"的那一档。
    ///
    /// 只有真失败算：`Skipped` 不计入错误率、不触发告警
    /// （`docs/conventions.md:301`），`Cancelled` 是用户自己的动作。
    pub fn is_alert(self) -> bool {
        matches!(self, Severity::Failed)
    }
}

/// 投递载荷。
///
/// ⚠ **只装运行结果的元信息**：来源、严重级、run/task/account、一句摘要、
/// 时长、时间戳。刻意不装任何凭据（token / cookie / authorization），也不装
/// 任务产出（文件内容、兑换码……）—— 通知渠道是用户自己配的**外部**地址，
/// 往里塞凭据等于把它们交出去。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyPayload {
    /// 来源标识。
    pub source: &'static str,
    pub severity: Severity,
    pub run_id: u64,
    pub task: String,
    pub account: String,
    /// 给人读的一句话，取自 `TaskOutcome::summary()`（成功取 summary，
    /// 跳过/失败取各自的 reason）。
    pub summary: String,
    pub duration_ms: u64,
    /// 投递时刻（Unix 毫秒）。
    pub timestamp_ms: i64,
}

/// 投递器：把一条载荷 POST 到 webhook。
pub struct Notifier {
    webhook: String,
    http: Arc<dyn HttpClient>,
    clock: Arc<dyn Clock>,
    timeout: Duration,
}

impl Notifier {
    pub fn new(
        webhook: impl Into<String>,
        http: Arc<dyn HttpClient>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            webhook: webhook.into(),
            http,
            clock,
            timeout: DELIVER_TIMEOUT,
        }
    }

    /// 当前时刻（Unix 毫秒）。
    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// 投递一条结果。
    ///
    /// **刻意不返回 `Result`**：非 2xx、网络错、超时都只 `tracing::warn!`。
    /// 这是"投递只是旁路"的落点 —— 通知失败不能反过来影响运行结果。
    pub async fn deliver(&self, payload: &NotifyPayload) {
        let body = match serde_json::to_value(payload) {
            Ok(v) => v,
            Err(e) => {
                // 结构体全是可序列化字段，走到这里只可能是上游把类型改坏了；
                // 依旧不能冒泡。
                tracing::warn!(error = %e, "推送载荷序列化失败");
                return;
            }
        };
        let req = HttpRequest::post(&self.webhook)
            .header("content-type", "application/json")
            .json_body(&body);

        match tokio::time::timeout(self.timeout, self.http.request(req)).await {
            Ok(Ok(resp)) if resp.is_success() => {
                tracing::debug!(run = payload.run_id, "运行结果已推送");
            }
            Ok(Ok(resp)) => {
                tracing::warn!(run = payload.run_id, status = resp.status, "推送被拒");
            }
            Ok(Err(e)) => {
                tracing::warn!(run = payload.run_id, error = %e, "推送失败");
            }
            Err(_) => {
                tracing::warn!(
                    run = payload.run_id,
                    timeout_ms = self.timeout.as_millis(),
                    "推送超时"
                );
            }
        }
    }
}

/// 取消登记表的**迟到句柄**。
///
/// 装饰器必须先于 `Runner` 存在（Runner 拿它当 sink），而登记表是
/// `Runner::new` 内部造的 —— 所以先给一个空槽位，runner 造好后立刻回填。
/// `AppBuilder::build` 在返回前一定填好，而那时还没有任何一次运行开始，
/// 因此不需要额外的同步。
pub type CancelSlot = Arc<OnceLock<Arc<CancelRegistry>>>;

/// 空槽位。
pub fn cancel_slot() -> CancelSlot {
    Arc::new(OnceLock::new())
}

/// 从配置里读到的 webhook 是否**真的启用**。
///
/// `None` / 空串 / 纯空白一律算"没配置"。
pub fn configured(webhook: Option<&str>) -> bool {
    webhook.is_some_and(|s| !s.trim().is_empty())
}

/// 一次运行的身份，来自 `Started`，供 `Finished` 组装载荷。
#[derive(Debug, Clone, Default)]
struct RunMeta {
    task: String,
    account: String,
}

/// 事件旁路：转发给内层 sink，同时把 `Finished` 送去投递。
pub struct NotifySink {
    inner: Arc<dyn EventSink>,
    notifier: Arc<Notifier>,
    cancel: CancelSlot,
    /// 在跑的运行：`run_id → (task, account)`。
    ///
    /// 只在内存里攒、`Finished` 时删。`Started` 与 `Finished` 都从 `Runner`
    /// 的同一个 sink 过，所以这一对一定成对出现（同一 `task:account` 已在运行时
    /// runner 直接返回、两个事件都不发）。因此表的大小 = 并发在跑的 run 数。
    started: Mutex<HashMap<RunId, RunMeta>>,
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

    /// 锁中毒时取回内部值继续用（与 `EventHub` 同一理由：每次操作在锁内一次
    /// 做完，没有"写了一半"的中间态，跟着 panic 只会让整个服务停摆）。
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RunId, RunMeta>> {
        self.started.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventSink for NotifySink {
    fn emit(&self, event: RunEvent) {
        // 先算要不要投递，再把事件**原样**转给内层。
        //
        // 判据（是否主动中止）依赖"runner 还没 release 取消登记表"——那与转发
        // 顺序无关：整段都在同一个同步 `emit` 调用里完成，而 runner 的
        // `release` 在 `emit` 返回之后。
        match &event {
            RunEvent::Started {
                run_id,
                task,
                account,
                ..
            } => {
                self.lock().insert(
                    *run_id,
                    RunMeta {
                        task: task.clone(),
                        account: account.clone(),
                    },
                );
            }
            RunEvent::Finished {
                run_id,
                outcome,
                duration_ms,
            } => {
                let id = *run_id;
                let meta = self.lock().remove(&id).unwrap_or_default();
                let cancelled = self
                    .cancel
                    .get()
                    .is_some_and(|registry| registry.is_cancelled(id));
                let payload = NotifyPayload {
                    source: SOURCE,
                    severity: Severity::for_outcome(outcome, cancelled),
                    run_id: id.get(),
                    task: meta.task,
                    account: meta.account,
                    summary: outcome.summary().to_string(),
                    duration_ms: *duration_ms,
                    timestamp_ms: self.notifier.now_ms(),
                };

                let notifier = Arc::clone(&self.notifier);
                match tokio::runtime::Handle::try_current() {
                    // 必须 spawn：`emit` 是同步的，而 HTTP 是异步的；在这里
                    // 等待就把投递的延迟算进了运行收尾。
                    Ok(handle) => {
                        handle.spawn(async move { notifier.deliver(&payload).await });
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

/// 按配置给 `inner` 套上推送旁路。
///
/// `webhook` 没配置 ⇒ **原样返回 `inner`**：不建 `Notifier`、不建 `NotifySink`，
/// 运行路径上一个字都不会发出去。这就是"删掉配置即回到今天"的可撤性。
pub(crate) fn decorate(
    inner: Arc<dyn EventSink>,
    webhook: Option<String>,
    http: Arc<dyn HttpClient>,
    clock: Arc<dyn Clock>,
    cancel: CancelSlot,
) -> Arc<dyn EventSink> {
    let Some(url) = webhook.filter(|s| !s.trim().is_empty()) else {
        return inner;
    };
    let notifier = Arc::new(Notifier::new(url.trim().to_string(), http, clock));
    Arc::new(NotifySink::new(inner, notifier, cancel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::testkit::FixedClock;
    use mcloud_core::{HttpResponse, Result as CoreResult};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW_MS: i64 = 1_700_000_000_000;

    /// 假传输：记下每一枪；`fail` 为真时返回网络错误。
    struct FakeHttp {
        seen: Mutex<Vec<HttpRequest>>,
        calls: AtomicUsize,
        fail: bool,
    }

    impl FakeHttp {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                seen: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                fail,
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// 最后一枪的体，解析成 JSON。
        fn last_body(&self) -> serde_json::Value {
            let raw = self.last_raw();
            serde_json::from_str(&raw).expect("载荷必须是合法 JSON")
        }

        fn last_raw(&self) -> String {
            let seen = self.seen.lock().unwrap();
            seen.last()
                .expect("应当有一枪")
                .body_str()
                .expect("载荷体应是 UTF-8 文本")
                .to_string()
        }
    }

    #[async_trait::async_trait]
    impl HttpClient for FakeHttp {
        async fn request(&self, req: HttpRequest) -> CoreResult<HttpResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(req);
            if self.fail {
                return Err(mcloud_core::Error::Other("网络断了".into()));
            }
            Ok(HttpResponse {
                status: 200,
                body: "{}".into(),
                headers: Vec::new(),
            })
        }
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

    /// 造一个"已配置"的旁路 sink，外加能回读的内层 sink。
    fn sink_with(
        http: Arc<FakeHttp>,
        cancel: Option<Arc<CancelRegistry>>,
    ) -> (Arc<dyn EventSink>, Arc<CollectSink>) {
        let inner = Arc::new(CollectSink::default());
        let slot = cancel_slot();
        if let Some(registry) = cancel {
            slot.set(registry).expect("空槽位应当设置成功");
        }
        let notifier = Arc::new(Notifier::new(
            "https://example.test/hook",
            http as Arc<dyn HttpClient>,
            clock(),
        ));
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

    /// 未配置 ⇒ 一个投递都不发生。
    ///
    /// 这就是 [`decorate`] 的接线本身 —— `AppBuilder::build` 走的是同一个函数，
    /// 所以这里断言的正是"组装出来的应用不会投递"。
    #[tokio::test]
    async fn unconfigured_wiring_never_delivers() {
        let http = FakeHttp::new(false);
        let inner = Arc::new(CollectSink::default());
        for cfg in [None, Some(String::new()), Some("   ".to_string())] {
            let sink = decorate(
                Arc::clone(&inner) as Arc<dyn EventSink>,
                cfg,
                Arc::clone(&http) as Arc<dyn HttpClient>,
                clock(),
                cancel_slot(),
            );
            sink.emit(started(1, "sign", "19900000001"));
            sink.emit(finished(1, TaskOutcome::failed("boom"), 10));
        }
        // 事件照常进了内层 sink（3 组 × 2 条），但一个包都没发出去
        assert_eq!(inner.len(), 6);
        assert_eq!(http.calls(), 0, "没配置 webhook 就不该有任何投递");
    }

    /// `configured` 的判定：空、纯空白、`None` 都不算配置。
    #[test]
    fn blank_webhook_is_unconfigured() {
        assert!(!configured(None));
        assert!(!configured(Some("")));
        assert!(!configured(Some("  \t\n ")));
        assert!(configured(Some(" https://example.test/hook ")));
    }

    /// 三态的 severity 与载荷字段逐个断言。
    #[tokio::test]
    async fn payload_fields_and_severity_for_each_outcome() {
        let cases = [
            (TaskOutcome::success("签到成功，获得 3 AI豆"), "ok"),
            (TaskOutcome::skipped("已签到"), "skipped"),
            (TaskOutcome::failed("网络断了"), "failed"),
        ];
        for (outcome, want_severity) in cases {
            let http = FakeHttp::new(false);
            let (sink, inner) = sink_with(Arc::clone(&http), None);
            sink.emit(started(7, "sign", "19900000001"));
            sink.emit(finished(7, outcome.clone(), 1500));
            wait_calls(&http, 1).await;

            let body = http.last_body();
            assert_eq!(body["source"], SOURCE);
            assert_eq!(body["severity"], want_severity);
            assert_eq!(body["runId"], 7);
            assert_eq!(body["task"], "sign");
            assert_eq!(body["account"], "19900000001");
            assert_eq!(body["summary"], outcome.summary());
            assert_eq!(body["durationMs"], 1500);
            assert_eq!(body["timestampMs"], NOW_MS);

            // `severity` 只是字符串；"报不报红"看 is_alert
            let severity = match want_severity {
                "ok" => Severity::Ok,
                "skipped" => Severity::Skipped,
                _ => Severity::Failed,
            };
            assert_eq!(severity.is_alert(), want_severity == "failed");
            // 事件必须**原样**继续往下走
            assert_eq!(inner.len(), 2);
            assert!(matches!(inner.last(), Some(RunEvent::Finished { .. })));
        }
    }

    /// 主动中止 ⇒ `cancelled`，且**不报红**；同一个 `Failed` 结论在没有中止时
    /// 仍是 `failed`。
    #[tokio::test]
    async fn user_cancelled_maps_to_cancelled_and_not_alert() {
        let registry = Arc::new(CancelRegistry::new());
        let token = registry.register(RunId::new(42));
        token.cancel();

        let http = FakeHttp::new(false);
        let (sink, _inner) = sink_with(Arc::clone(&http), Some(Arc::clone(&registry)));
        sink.emit(started(42, "sign", "19900000001"));
        // 中止的运行在业务上多半也是 Failed —— 但绝不能被报红
        sink.emit(finished(42, TaskOutcome::failed("已中止"), 300));
        wait_calls(&http, 1).await;

        let body = http.last_body();
        assert_eq!(body["severity"], "cancelled");
        assert!(!Severity::Cancelled.is_alert(), "用户自己点的中止不该报红");

        // 对照：槽位里没有中止时，同样的 Failed 仍是 failed
        let http2 = FakeHttp::new(false);
        let (sink2, _) = sink_with(Arc::clone(&http2), None);
        sink2.emit(started(42, "sign", "19900000001"));
        sink2.emit(finished(42, TaskOutcome::failed("超时"), 300));
        wait_calls(&http2, 1).await;
        assert_eq!(http2.last_body()["severity"], "failed");
    }

    /// `Skipped` 不报红。
    #[test]
    fn skipped_is_not_alert() {
        assert!(!Severity::Skipped.is_alert());
        assert_eq!(
            Severity::for_outcome(&TaskOutcome::skipped("已签到"), false),
            Severity::Skipped
        );
    }

    /// 投递失败 ⇒ 不影响运行结果、不 panic。
    #[tokio::test]
    async fn delivery_failure_does_not_affect_the_run() {
        let http = FakeHttp::new(true);
        let (sink, inner) = sink_with(Arc::clone(&http), None);
        sink.emit(started(1, "sign", "19900000001"));
        sink.emit(finished(1, TaskOutcome::failed("boom"), 5));
        wait_calls(&http, 1).await;

        // 投递确实发生了、而且失败了……
        assert_eq!(http.calls(), 1);
        // ……但事件原样进了内层 sink：运行结果一点没受影响，也没有 panic
        assert_eq!(inner.len(), 2);
        assert!(matches!(
            inner.last(),
            Some(RunEvent::Finished {
                outcome: TaskOutcome::Failed { .. },
                ..
            })
        ));
    }

    /// 载荷里不得出现 token / cookie / authorization 之类字样。
    #[tokio::test]
    async fn payload_never_leaks_credentials() {
        let http = FakeHttp::new(false);
        let (sink, _inner) = sink_with(Arc::clone(&http), None);
        sink.emit(started(9, "sign", "19900000001"));
        sink.emit(finished(
            9,
            TaskOutcome::success("签到成功，获得 3 AI豆"),
            1200,
        ));
        wait_calls(&http, 1).await;

        // 脱敏扫描大小写不敏感（Authorization / Cookie 也要抓得到）
        let raw = http.last_raw();
        let lower = raw.to_lowercase();
        for needle in [
            "token",
            "cookie",
            "authorization",
            "jwt",
            "password",
            "secret",
        ] {
            assert!(!lower.contains(needle), "载荷不该出现 {needle} 字样：{raw}");
        }
        // 正向确认载荷就是我们声明的那几个字段（camelCase，且只有这几格）
        assert_eq!(
            http.last_body(),
            serde_json::json!({
                "source": SOURCE,
                "severity": "ok",
                "runId": 9,
                "task": "sign",
                "account": "19900000001",
                "summary": "签到成功，获得 3 AI豆",
                "durationMs": 1200,
                "timestampMs": NOW_MS,
            }),
            "字段名必须是 camelCase，且不夹带别的格子"
        );
    }
}
