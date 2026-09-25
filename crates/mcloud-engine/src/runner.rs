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
        }
    }

    /// 取消登记表。服务器据此响应"中止这次运行"。
    pub fn cancel_registry(&self) -> Arc<crate::cancel_registry::CancelRegistry> {
        Arc::clone(&self.cancel)
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
        let this = Arc::clone(self);
        let task_for_spawn = task_name.to_string();
        tokio::spawn(async move {
            if let Err(e) = this
                .run_impl(&task_for_spawn, &account, &body, run_id, None)
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

    /// 任务是否已注册。供编排预检用 —— 它必须在发请求**之前**调用。
    pub fn has_task(&self, name: &str) -> bool {
        self.registry.contains(name)
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
        self.run_impl(task_name, account, body, run_id, chain_cancel).await
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
    ) -> Result<RunRecord> {
        let task = self
            .registry
            .get(task_name)
            .ok_or_else(|| mcloud_core::Error::Config(format!("未知任务：{task_name}")))?;

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
        let cancel_token = self.cancel.register(run_id);

        let ctx = TaskContext {
            account: account.clone(),
            http: (self.http_for)(account),
            store: Arc::clone(&self.store),
            clock: Arc::clone(&self.clock),
            sink: Arc::clone(&self.sink),
            run_id,
            cancel: cancel_token.clone(),
            chain_cancel,
        };

        let outcome = match task.run(&ctx, body).await {
            Ok(o) => o,
            Err(e) => TaskOutcome::failed(e.to_string()),
        };

        let duration_ms = (self.clock.now_ms() - started).max(0) as u64;
        // 中止与业务结论分开记：中止不是失败，不该计入错误率
        let was_cancelled = cancel_token.is_cancelled();
        self.sink.emit(RunEvent::Finished {
            run_id,
            outcome: outcome.clone(),
            duration_ms,
        });

        self.inflight.lock().expect("inflight 锁中毒").remove(&key);
        self.cancel.release(run_id);

        let mut record = self.record(run_id, task_name, account, duration_ms, outcome);
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
}
