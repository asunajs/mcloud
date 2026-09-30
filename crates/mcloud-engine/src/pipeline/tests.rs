//! 编排执行测试。
//!
//! 重点：**失败语义的两层**、**预检在任何请求之前**、**中止不半途掐断**。

use super::*;
use crate::registry::{Registry, Task};
use crate::runner::{HttpClientFactory, Runner};
use crate::{TaskContext, TaskResult};
use async_trait::async_trait;
use mcloud_core::testkit::FixedClock;
use mcloud_core::{
    Account, AccountId, AccountSettings, AppendOutcome, AuthInfo, DeviceId, EventSink, HttpClient,
    HttpRequest, HttpResponse, LogLine, PipelineId, RunEvent, RunId, RunRecord, Store, TaskOutcome,
};
use std::sync::Mutex;

const NOW_MS: i64 = 1_700_000_000_000;

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
        settings: AccountSettings::default(),
    }
}

fn step(task: &str, cont: bool) -> PipelineStep {
    PipelineStep {
        task: task.into(),
        body: serde_json::Value::Null,
        continue_on_error: cont,
    }
}

fn pipeline(steps: Vec<PipelineStep>, stop_on_error: bool) -> Pipeline {
    Pipeline {
        id: PipelineId::new("p1"),
        name: "测试链".into(),
        accounts: Vec::new(),
        account_mode: AccountMode::Sequential,
        stop_on_error,
        steps,
        enabled: true,
        owner: None,
        last_run_ms: None,
    }
}

// ── 测试替身 ────────────────────────────────────────────────────────────

pub(crate) struct NoHttp;
#[async_trait]
impl HttpClient for NoHttp {
    async fn request(&self, _req: HttpRequest) -> mcloud_core::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: "{}".into(),
            headers: Vec::new(),
        })
    }
}

#[derive(Default)]
pub(crate) struct MemStore {
    runs: Mutex<Vec<RunRecord>>,
}
#[async_trait]
impl mcloud_core::AccountStore for MemStore {
    async fn accounts(&self) -> mcloud_core::Result<Vec<Account>> {
        Ok(Vec::new())
    }
    async fn account(&self, _id: &AccountId) -> mcloud_core::Result<Option<Account>> {
        Ok(None)
    }
    async fn save_account(&self, _a: &Account) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn remove_account(&self, _id: &AccountId) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn account_state(&self, _id: &AccountId) -> mcloud_core::Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    async fn save_account_state(
        &self,
        _i: &AccountId,
        _s: &serde_json::Value,
    ) -> mcloud_core::Result<()> {
        Ok(())
    }
}
#[async_trait]
impl mcloud_core::RunStore for MemStore {
    async fn upsert_run(&self, r: &RunRecord) -> mcloud_core::Result<()> {
        self.runs.lock().unwrap().push(r.clone());
        Ok(())
    }
    async fn runs(&self, limit: usize) -> mcloud_core::Result<Vec<RunRecord>> {
        let mut v = self.runs.lock().unwrap().clone();
        v.truncate(limit);
        Ok(v)
    }
    async fn run(&self, id: RunId) -> mcloud_core::Result<Option<RunRecord>> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.run_id == id)
            .cloned())
    }
    async fn running_runs(&self) -> mcloud_core::Result<Vec<RunRecord>> {
        Ok(Vec::new())
    }
}
#[async_trait]
impl mcloud_core::LogStore for MemStore {
    async fn append_logs(&self, _r: RunId, _l: &[LogLine]) -> mcloud_core::Result<AppendOutcome> {
        Ok(AppendOutcome::Appended)
    }
    async fn logs(
        &self,
        _r: RunId,
        _a: Option<mcloud_core::Seq>,
    ) -> mcloud_core::Result<Vec<LogLine>> {
        Ok(Vec::new())
    }
    async fn clear_logs(&self, _r: RunId) -> mcloud_core::Result<()> {
        Ok(())
    }
}
#[async_trait]
impl mcloud_core::ScheduleStore for MemStore {
    async fn schedules(&self) -> mcloud_core::Result<Vec<mcloud_core::Schedule>> {
        Ok(Vec::new())
    }
    async fn schedule(
        &self,
        _i: &mcloud_core::ScheduleId,
    ) -> mcloud_core::Result<Option<mcloud_core::Schedule>> {
        Ok(None)
    }
    async fn save_schedule(&self, _s: &mcloud_core::Schedule) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn remove_schedule(&self, _i: &mcloud_core::ScheduleId) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn mark_schedule_fired(
        &self,
        _i: &mcloud_core::ScheduleId,
        _ms: i64,
    ) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn record_schedule_run(
        &self,
        _i: &mcloud_core::ScheduleId,
        _run_ms: Option<i64>,
        _status: &str,
        _error: Option<&str>,
    ) -> mcloud_core::Result<()> {
        Ok(())
    }
}
#[async_trait]
impl mcloud_core::PipelineStore for MemStore {
    async fn pipelines(&self) -> mcloud_core::Result<Vec<Pipeline>> {
        Ok(Vec::new())
    }
    async fn pipeline(&self, _i: &PipelineId) -> mcloud_core::Result<Option<Pipeline>> {
        Ok(None)
    }
    async fn save_pipeline(&self, _p: &Pipeline) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn remove_pipeline(&self, _i: &PipelineId) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn mark_pipeline_ran(&self, _i: &PipelineId, _ms: i64) -> mcloud_core::Result<()> {
        Ok(())
    }
}

/// 登录闸在编排测试里用不到 —— 全部给"空/无"的实现。
///
/// 这组方法存在的意义只是让 `MemStore` 满足 `Store`。
/// 给空实现而不是 `unimplemented!()`：将来某个测试顺路调到它时，
/// 得到的是"没有用户"这个**语义正确**的结果，而不是一次 panic。
#[async_trait]
impl mcloud_core::UserStore for MemStore {
    async fn user_count(&self) -> mcloud_core::Result<usize> {
        Ok(0)
    }
    async fn users(&self) -> mcloud_core::Result<Vec<mcloud_core::User>> {
        Ok(Vec::new())
    }
    async fn user_by_name(&self, _n: &str) -> mcloud_core::Result<Option<mcloud_core::User>> {
        Ok(None)
    }
    async fn user(
        &self,
        _id: &mcloud_core::UserId,
    ) -> mcloud_core::Result<Option<mcloud_core::User>> {
        Ok(None)
    }
    async fn save_user(&self, _u: &mcloud_core::User) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn save_session(&self, _s: &mcloud_core::Session) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn session(&self, _h: &str) -> mcloud_core::Result<Option<mcloud_core::Session>> {
        Ok(None)
    }
    async fn remove_session(&self, _h: &str) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn remove_user_sessions(&self, _id: &mcloud_core::UserId) -> mcloud_core::Result<usize> {
        Ok(0)
    }
    async fn purge_expired_sessions(&self, _now: i64) -> mcloud_core::Result<usize> {
        Ok(0)
    }
}

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _e: RunEvent) {}
}

/// 记录调用顺序，用于验证"预检先于任何请求"与"步骤在外、账号在内"。
#[derive(Default)]
struct CallLog(Mutex<Vec<String>>);
impl CallLog {
    fn push(&self, s: String) {
        self.0.lock().unwrap().push(s);
    }
    fn snapshot(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// 总是成功的任务。
struct OkTask(&'static str, Arc<CallLog>);
#[async_trait]
impl Task for OkTask {
    fn name(&self) -> &'static str {
        self.0
    }
    fn title(&self) -> &'static str {
        "ok"
    }
    fn description(&self) -> &'static str {
        "总是成功"
    }
    async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
        self.1.push(format!("{}/{}", self.0, ctx.account.id));
        Ok(TaskOutcome::success("ok"))
    }
}

/// 总是失败的任务。
struct FailTask(&'static str, Arc<CallLog>);
#[async_trait]
impl Task for FailTask {
    fn name(&self) -> &'static str {
        self.0
    }
    fn title(&self) -> &'static str {
        "fail"
    }
    fn description(&self) -> &'static str {
        "总是失败"
    }
    async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
        self.1.push(format!("{}/{}", self.0, ctx.account.id));
        Ok(TaskOutcome::failed("炸了"))
    }
}

fn build(tasks: Vec<Arc<dyn Task>>, log: Arc<CallLog>) -> (Arc<Runner>, Arc<CallLog>) {
    let mut reg = Registry::new();
    for t in tasks {
        reg.register(t).unwrap();
    }
    let http: HttpClientFactory = Arc::new(|_a: &Account| Arc::new(NoHttp) as Arc<dyn HttpClient>);
    let runner = Runner::new(
        Arc::new(reg),
        Arc::new(MemStore::default()) as Arc<dyn Store>,
        Arc::new(FixedClock(NOW_MS)),
        Arc::new(NullSink) as Arc<dyn EventSink>,
        http,
    );
    (Arc::new(runner), log)
}

/// 只注册前两个任务，第三个是"不存在的任务"，用于预检测试。
fn tasks3(log: &Arc<CallLog>) -> Vec<Arc<dyn Task>> {
    vec![
        Arc::new(OkTask("a", Arc::clone(log))),
        Arc::new(FailTask("b", Arc::clone(log))),
        Arc::new(OkTask("c", Arc::clone(log))),
    ]
}

// ── 预检 ────────────────────────────────────────────────────────────────

/// ⭐ 未知任务必须在**发出任何请求之前**就被挡住。
///
/// 否则"第 3 步的任务名拼错了"的表现是：前两步已经把真上游打了一遍
/// （签到是写操作），然后整条链报错。
#[tokio::test]
async fn preflight_rejects_unknown_task_before_any_request() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);

    let p = pipeline(
        vec![step("a", false), step("b", false), step("不存在", false)],
        true,
    );
    let err = pr.preflight(&p).unwrap_err();
    assert!(err.to_string().contains("第 3 步"), "应指出是第几步：{err}");
    assert!(err.to_string().contains("不存在"), "应带上任务名：{err}");
    assert!(log.snapshot().is_empty(), "预检失败时不该发出任何调用");
}

#[tokio::test]
async fn preflight_rejects_empty_steps() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);
    assert!(pr.preflight(&pipeline(vec![], true)).is_err());
}

// ── 执行顺序 ────────────────────────────────────────────────────────────

/// 步骤在外、账号在内。
#[tokio::test]
async fn steps_run_outer_and_accounts_inner() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);

    let p = pipeline(vec![step("a", false), step("c", false)], true);
    let accounts = vec![account("111"), account("222")];
    pr.run(&p, &accounts, None).await.unwrap();

    assert_eq!(
        log.snapshot(),
        vec!["a/111", "a/222", "c/111", "c/222"],
        "应先跑完一步的所有账号，再进入下一步"
    );
}

// ── 失败语义 ────────────────────────────────────────────────────────────

/// 编排级 `stop_on_error=true` + 步骤没写 continue ⇒ 中止，后续步骤记为 Skipped。
#[tokio::test]
async fn stop_on_error_aborts_and_marks_rest_skipped() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);

    let p = pipeline(
        vec![step("a", false), step("b", false), step("c", false)],
        true,
    );
    let out = pr.run(&p, &[account("111")], None).await.unwrap();

    assert_eq!(out.stopped_after, Some(1), "应在第 2 步（下标 1）中止");
    assert_eq!(out.steps[0].status, StepStatus::Ok);
    assert_eq!(out.steps[1].status, StepStatus::Error);
    assert_eq!(
        out.steps[2].status,
        StepStatus::Skipped,
        "后续步骤要记为跳过"
    );
    assert_eq!(out.status(), StepStatus::Error);
    // 结果数组长度不变 —— 前端不该处理"数组会变短"
    assert_eq!(out.steps.len(), 3);
    assert!(
        !log.snapshot().contains(&"c/111".to_string()),
        "中止后不该再跑"
    );
}

/// 步骤级 `continue_on_error=true` 覆盖编排级的 stop_on_error。
#[tokio::test]
async fn step_continue_on_error_overrides_pipeline_stop() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);

    let p = pipeline(
        vec![step("a", false), step("b", true), step("c", false)],
        true,
    );
    let out = pr.run(&p, &[account("111")], None).await.unwrap();

    assert_eq!(out.stopped_after, None, "步骤说继续就不该中止");
    assert_eq!(out.steps[1].status, StepStatus::Error);
    assert_eq!(out.steps[2].status, StepStatus::Ok, "后续步骤应照跑");
    assert!(log.snapshot().contains(&"c/111".to_string()));
    // 整体仍是 error（单调：失败不会被后面的成功翻绿）
    assert_eq!(out.status(), StepStatus::Error);
}

/// 编排级 `stop_on_error=false` 时任何失败都继续。
#[tokio::test]
async fn lenient_pipeline_never_aborts() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);

    let p = pipeline(
        vec![step("a", false), step("b", false), step("c", false)],
        false,
    );
    let out = pr.run(&p, &[account("111")], None).await.unwrap();

    assert_eq!(out.stopped_after, None);
    assert_eq!(out.steps[2].status, StepStatus::Ok);
    assert_eq!(out.status(), StepStatus::Error, "有失败，整体就是 error");
}

// ── 多账号汇总 ──────────────────────────────────────────────────────────

/// 部分账号失败 ⇒ partial，不是 error 也不是 ok。
#[tokio::test]
async fn partial_failure_yields_partial_not_error() {
    let log = Arc::new(CallLog::default());
    // 一个任务：对 111 成功、对 222 失败
    struct HalfTask(Arc<CallLog>);
    #[async_trait]
    impl Task for HalfTask {
        fn name(&self) -> &'static str {
            "half"
        }
        fn title(&self) -> &'static str {
            "half"
        }
        fn description(&self) -> &'static str {
            "一半成功"
        }
        async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            self.0.push(format!("half/{}", ctx.account.id));
            if ctx.account.id.as_str() == "222" {
                Ok(TaskOutcome::failed("这个账号炸了"))
            } else {
                Ok(TaskOutcome::success("ok"))
            }
        }
    }
    let mut reg = Registry::new();
    reg.register(Arc::new(HalfTask(Arc::clone(&log)))).unwrap();
    let http: HttpClientFactory = Arc::new(|_a: &Account| Arc::new(NoHttp) as Arc<dyn HttpClient>);
    let runner = Arc::new(Runner::new(
        Arc::new(reg),
        Arc::new(MemStore::default()) as Arc<dyn Store>,
        Arc::new(FixedClock(NOW_MS)),
        Arc::new(NullSink) as Arc<dyn EventSink>,
        http,
    ));
    let pr = PipelineRunner::new(runner);

    let p = pipeline(vec![step("half", false)], true);
    let out = pr
        .run(&p, &[account("111"), account("222")], None)
        .await
        .unwrap();

    assert_eq!(
        out.steps[0].status,
        StepStatus::Partial,
        "半成半败是 partial"
    );
    assert_eq!(out.status(), StepStatus::Partial);
    // 只有一步且它 partial，stop_on_error 会判为失败而中止（但已无后续步骤）
    assert_eq!(out.steps[0].runs.len(), 2);
}

// ── 中止 ────────────────────────────────────────────────────────────────

/// 已取消的令牌 ⇒ 第一步都不开始，全部记为跳过。
#[tokio::test]
async fn cancelled_token_skips_everything() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);

    let token = CancelToken::new();
    token.cancel();

    let p = pipeline(vec![step("a", false), step("c", false)], true);
    let out = pr.run(&p, &[account("111")], Some(token)).await.unwrap();

    assert!(log.snapshot().is_empty(), "已取消时不该发出任何调用");
    assert!(out.steps.iter().all(|s| s.status == StepStatus::Skipped));
    assert_eq!(out.status(), StepStatus::Cancelled);
}

/// ⭐ 中止是**协作式**的：在当前账号跑完之后才停，不半途掐断。
///
/// 直接 drop 一个正在发请求的 future 会留下"请求真的发出去了、
/// 本地却记成已中止"的不一致 —— 用户再点一次就是重复签到。
#[tokio::test]
async fn cancellation_takes_effect_at_the_next_safe_point() {
    let log = Arc::new(CallLog::default());
    let token = CancelToken::new();
    let token_in_task = token.clone();
    let log_in_task = Arc::clone(&log);

    // 任务：每跑一次就取消令牌（模拟"用户点了中止"）
    struct CancelAfterFirst(Arc<CallLog>, CancelToken);
    #[async_trait]
    impl Task for CancelAfterFirst {
        fn name(&self) -> &'static str {
            "cancelme"
        }
        fn title(&self) -> &'static str {
            "cancelme"
        }
        fn description(&self) -> &'static str {
            "跑一个就取消"
        }
        async fn run(&self, ctx: &TaskContext, _b: &serde_json::Value) -> TaskResult {
            self.0.push(format!("cancelme/{}", ctx.account.id));
            self.1.cancel();
            Ok(TaskOutcome::success("ok"))
        }
    }

    let mut reg = Registry::new();
    reg.register(Arc::new(CancelAfterFirst(log_in_task, token_in_task)))
        .unwrap();
    let http: HttpClientFactory = Arc::new(|_a: &Account| Arc::new(NoHttp) as Arc<dyn HttpClient>);
    let runner = Arc::new(Runner::new(
        Arc::new(reg),
        Arc::new(MemStore::default()) as Arc<dyn Store>,
        Arc::new(FixedClock(NOW_MS)),
        Arc::new(NullSink) as Arc<dyn EventSink>,
        http,
    ));
    let pr = PipelineRunner::new(runner);

    let p = pipeline(vec![step("cancelme", false), step("cancelme", false)], true);
    let out = pr
        .run(&p, &[account("111"), account("222")], Some(token))
        .await
        .unwrap();

    assert_eq!(
        log.snapshot(),
        vec!["cancelme/111"],
        "第一个账号跑完后应停下，第二个不该开始"
    );
    assert_eq!(out.status(), StepStatus::Cancelled);
}

// ── 并行模式 ────────────────────────────────────────────────────────────

/// `Parallel` 必须明确报错，不能静默按串行跑。
#[test]
fn parallel_mode_is_rejected_explicitly() {
    assert!(ensure_supported_account_mode(AccountMode::Sequential).is_ok());
    let e = ensure_supported_account_mode(AccountMode::Parallel).unwrap_err();
    assert!(e.to_string().contains("并行"), "应明确说并行未实现：{e}");
}

// ── 摘要 ────────────────────────────────────────────────────────────────

#[test]
fn summary_line_mentions_stop_point() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let p = pipeline(
        vec![step("a", false), step("b", false), step("c", false)],
        true,
    );
    let out = rt.block_on(pr.run(&p, &[account("111")], None)).unwrap();

    let s = out.summary_line();
    assert!(s.contains("第 2 步"), "摘要必须说明在哪一步停的：{s}");
    // 3 步里跑了 2 步（第 3 步被中止跳过）
    assert!(s.contains("2/3"), "摘要应给出已执行/总数：{s}");
}

/// 未中止时摘要不该说"在第 N 步中止"。
#[tokio::test]
async fn summary_line_has_no_stop_point_when_complete() {
    let log = Arc::new(CallLog::default());
    let (runner, _) = build(tasks3(&log), Arc::clone(&log));
    let pr = PipelineRunner::new(runner);
    let p = pipeline(vec![step("a", false), step("c", false)], true);
    let out = pr.run(&p, &[account("111")], None).await.unwrap();
    assert!(
        !out.summary_line().contains("中止"),
        "{}",
        out.summary_line()
    );
    assert_eq!(out.status(), StepStatus::Ok);
}
