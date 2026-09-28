//! 消息推送任务层的测试。
//!
//! 这一组钉两类东西：
//!
//! * **判定**（`plan_msg_push` / `first_reward_pending`）—— 纯函数，把每档的
//!   边界都摆出来，尤其是"字段缺失"与"给了 0"**不是一回事**。
//! * **写请求发不发**（`http.count()`）—— 这一条链最贵的错误是
//!   "在判据没过的时候把 `obtain` 发出去"：那一枪零样本、幂等与否未知。
//!   所以每个"不该发"的档都要断言请求数**恰好 1**。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

// ══════════════════════════ 夹具 ══════════════════════════

/// 2026-09-25 实拍那份 `task/status`，逐字（`secondTaskStatus=1` ⇒ 不该领）。
const CAPTURED_STATUS: &str = r#"{"code":0,"msg":"success","result":{
    "pushOn":1,"firstTaskStatus":3,"secondTaskStatus":1,
    "pushTaskStatus":0,"onDuaration":16,"total":31}}"#;

/// 造一份 `task/status`。`None` = **该键不出现**（与"给了 0"不是一回事）。
fn status_body(push_on: Option<i64>, second: Option<i64>) -> String {
    let mut result = serde_json::Map::new();
    if let Some(v) = push_on {
        result.insert("pushOn".into(), serde_json::json!(v));
    }
    result.insert("firstTaskStatus".into(), serde_json::json!(3));
    if let Some(v) = second {
        result.insert("secondTaskStatus".into(), serde_json::json!(v));
    }
    serde_json::json!({"code":0,"msg":"success","result":result}).to_string()
}

fn obtain_body(result: serde_json::Value) -> String {
    serde_json::json!({"code":0,"msg":"success","result":result}).to_string()
}

/// 一枪的答复：给个 body，或者**打挂**（网络层错误）。
#[derive(Clone)]
enum Reply {
    Body(String),
    Fail,
}

/// 按顺序答复的假 HTTP 客户端（用尽后重复最后一个）。
struct Scripted {
    seen: Mutex<Vec<HttpRequest>>,
    replies: Mutex<Vec<Reply>>,
}

impl Scripted {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            // 反转，便于从队尾取
            replies: Mutex::new(replies.into_iter().rev().collect()),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }

    fn url_at(&self, n: usize) -> String {
        self.requests()[n].url.clone()
    }
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req);
        let reply = {
            let mut r = self.replies.lock().unwrap();
            if r.len() > 1 {
                r.pop().unwrap()
            } else {
                r.last().cloned().unwrap_or(Reply::Body("{}".into()))
            }
        };
        match reply {
            Reply::Body(body) => Ok(HttpResponse {
                status: 200,
                body,
                headers: Vec::new(),
            }),
            Reply::Fail => Err(mcloud_core::Error::Other("网络断了".into())),
        }
    }
}

struct FixedClock(i64);
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

#[derive(Default)]
struct CollectSink {
    events: Mutex<Vec<RunEvent>>,
}

impl CollectSink {
    /// 取回所有 `Details` 事件的 payload。
    fn details(&self) -> Vec<serde_json::Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                RunEvent::Details { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }
}

impl EventSink for CollectSink {
    fn emit(&self, event: RunEvent) {
        self.events.lock().unwrap().push(event);
    }
}

fn account() -> Account {
    Account {
        id: mcloud_core::AccountId::new("19****01"),
        nickname: None,
        auth: AuthInfo {
            raw: "Basic eA==".into(),
            platform: "pc".into(),
            phone: "19900000001".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("DEV+1/base=".into()),
        pc_platform: None,
        user_domain_id: None,
        settings: AccountSettings::default(),
    }
}

/// 一个只带 `http` 的 ctx，外加能回读的 sink。
fn ctx_with(replies: Vec<Reply>) -> (TaskContext, Arc<Scripted>, Arc<CollectSink>) {
    let http = Arc::new(Scripted::new(replies));
    let sink = Arc::new(CollectSink::default());
    let acct = account();
    let ctx = TaskContext {
        account: acct.clone(),
        http: http.clone(),
        store: Arc::new(NoStore),
        clock: Arc::new(FixedClock(1_700_000_000_000)),
        sink: sink.clone(),
        run_id: mcloud_core::RunId(1),
        cancel: CancelToken::new(),
        chain_cancel: None,
        artifacts: mcloud_engine::ArtifactRecorder::new(
            Arc::new(mcloud_engine::ArtifactLedger::new()),
            mcloud_core::RunId(1),
            mcloud_core::AccountId::new("19****01"),
        ),
        details: mcloud_engine::DetailsRecorder::new(
            mcloud_engine::DetailsLedger::new(),
            mcloud_core::RunId(1),
        ),
    };
    (ctx, http, sink)
}

async fn run_with(replies: Vec<Reply>) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    let (ctx, http, sink) = ctx_with(replies);
    let out = MsgPushTask.run(&ctx, &serde_json::json!({})).await;
    (out.expect("不该是 Err"), http, sink)
}

// ══════════════════════════ 判定（纯函数） ══════════════════════════

fn status(push_on: Option<i64>, second: Option<i64>) -> MsgPushStatus {
    MsgPushStatus {
        push_on,
        second_task_status: second,
        ..Default::default()
    }
}

/// 判据那个值 ⇒ 该领。
#[test]
fn second_status_two_is_claimable() {
    assert_eq!(
        plan_msg_push(&status(Some(1), Some(SECOND_TASK_CLAIMABLE))),
        MsgPushPlan::Obtain
    );
}

/// **实拍那一条读数**（`secondTaskStatus = 1`）⇒ 不该领。
#[test]
fn the_captured_reading_is_not_claimable() {
    assert_eq!(
        plan_msg_push(&status(Some(1), Some(1))),
        MsgPushPlan::Nothing
    );
}

/// `pushOn == 0` 是**前置**开关：即便 `secondTaskStatus` 是该领的值，也不发。
#[test]
fn push_on_zero_wins_over_a_claimable_status() {
    assert_eq!(
        plan_msg_push(&status(Some(PUSH_ON_OFF), Some(SECOND_TASK_CLAIMABLE))),
        MsgPushPlan::Manual
    );
}

/// `pushOn` **缺失**不等于"给了 0" ⇒ 往下走判据，**不**报"要人去开通知"。
///
/// 旧项目用的是严格相等（`undefined === 0` 为 `false`），我们照这个口径。
/// 反过来会让用户为了一个可能根本没坏的东西去翻 App 设置。
#[test]
fn a_missing_push_on_does_not_mean_manual() {
    assert_eq!(
        plan_msg_push(&status(None, Some(SECOND_TASK_CLAIMABLE))),
        MsgPushPlan::Obtain
    );
    assert_eq!(plan_msg_push(&status(None, Some(1))), MsgPushPlan::Nothing);
}

/// `secondTaskStatus` **缺失** ⇒ `Unknown`，**不是** `Nothing`。
///
/// 落 `Nothing` 会让小结说成"现在没得领"，而那是一句**没根据的肯定话**。
#[test]
fn a_missing_second_status_is_unknown_not_nothing() {
    assert_eq!(plan_msg_push(&status(Some(1), None)), MsgPushPlan::Unknown);
}

/// 首次奖励"还没领"的判据：缺字段时**不提示**（与旧项目相反，见函数注释）。
#[test]
fn first_reward_notice_needs_the_field() {
    assert!(
        !first_reward_pending(&status(Some(1), Some(1))),
        "缺字段不提示"
    );

    let mut pending = status(Some(1), Some(1));
    pending.first_task_status = Some(1);
    assert!(first_reward_pending(&pending), "给了且不是 3 ⇒ 提示");

    pending.first_task_status = Some(FIRST_TASK_DONE);
    assert!(!first_reward_pending(&pending), "给了 3 ⇒ 不提示");
}

// ══════════════════════════ 不该发的三档：请求数必须恰好 1 ══════════════════════════

/// `Nothing` ⇒ 跳过，且**只发了一枪**（查状态）。
#[tokio::test]
async fn nothing_sends_no_write_request() {
    let (out, http, _) = run_with(vec![Reply::Body(status_body(Some(1), Some(1)))]).await;
    assert!(matches!(out, TaskOutcome::Skipped { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 1, "判据没过就不该发第二枪");
    assert!(http.url_at(0).ends_with("/task/status"));
}

/// `Manual` ⇒ 跳过，不发写请求，且小结里**点了名**要人去 App 里开通知。
#[tokio::test]
async fn manual_skips_and_tells_the_user_to_open_notifications() {
    let (out, http, _) = run_with(vec![Reply::Body(status_body(Some(0), Some(2)))]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 1, "通知没开就不该发写请求");
    assert!(
        reason.contains("App") && reason.contains("消息通知"),
        "得说清要用户做什么，实得：{reason}"
    );
}

/// `Unknown` ⇒ 跳过（**不是失败**），不发写请求，理由说清"判不了"。
#[tokio::test]
async fn unknown_skips_without_sending_and_says_why() {
    let (out, http, _) = run_with(vec![Reply::Body(status_body(Some(1), None))]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 1);
    assert!(
        reason.contains("secondTaskStatus") && reason.contains("没发请求"),
        "实得：{reason}"
    );
}

/// 实拍那条读数（`secondTaskStatus=1`）走完整条链也**不发写请求**。
#[tokio::test]
async fn the_captured_body_never_triggers_a_write() {
    let (out, http, _) = run_with(vec![Reply::Body(CAPTURED_STATUS.to_string())]).await;
    assert!(matches!(out, TaskOutcome::Skipped { .. }));
    assert_eq!(http.count(), 1);
}

// ══════════════════════════ 该发的那些：写请求的形状与成败 ══════════════════════════

/// `obtainCode == 0` ⇒ 成功，且第二枪打在 `/task/obtain` 上、body 是 `{"type":2}`。
#[tokio::test]
async fn obtain_code_zero_is_a_success() {
    let (out, http, _) = run_with(vec![
        Reply::Body(status_body(Some(1), Some(2))),
        Reply::Body(obtain_body(serde_json::json!({"obtainCode":0}))),
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 2);
    let second = &http.requests()[1];
    assert!(second.url.ends_with("/task/obtain"), "实得：{}", second.url);
    assert_eq!(second.body_str(), Some(r#"{"type":2}"#));
}

/// ⚠⚠ **缺 `obtainCode` 算失败，不算成功**。
///
/// 写请求已经发出去了 —— 把"判不了"伪造成"成功"，用户就再也不会去查这一笔。
#[tokio::test]
async fn a_missing_obtain_code_is_a_failure_not_a_success() {
    let (out, http, _) = run_with(vec![
        Reply::Body(status_body(Some(1), Some(2))),
        Reply::Body(obtain_body(serde_json::json!({}))),
    ])
    .await;
    let TaskOutcome::Failed { reason } = &out else {
        panic!("缺 obtainCode 不许念成成功，实得：{out:?}");
    };
    assert!(reason.contains("obtainCode"), "实得：{reason}");
    assert_eq!(http.count(), 2, "这一枪确实发出去了");
}

/// `obtainCode` 非 0 ⇒ 失败，并把那个数念出来。
#[tokio::test]
async fn a_non_zero_obtain_code_is_a_failure() {
    let (out, _, _) = run_with(vec![
        Reply::Body(status_body(Some(1), Some(2))),
        Reply::Body(obtain_body(serde_json::json!({"obtainCode":5}))),
    ])
    .await;
    let TaskOutcome::Failed { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains('5'), "要念出上游那个数，实得：{reason}");
}

/// 写请求**打挂了** ⇒ 失败（不是跳过）：那一枪已经出去了。
#[tokio::test]
async fn a_failed_write_is_reported_as_failed() {
    let (out, http, _) = run_with(vec![
        Reply::Body(status_body(Some(1), Some(2))),
        Reply::Fail,
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Failed { .. }), "实得：{out:?}");
    assert_eq!(
        http.count(),
        2,
        "写请求发过了，这一趟的实况是「发过、没成」"
    );
}

/// 查状态就挂了 ⇒ **交回 `Err`**（整趟没有结论可报）。
#[tokio::test]
async fn a_failed_status_query_is_an_error() {
    let (ctx, http, _) = ctx_with(vec![Reply::Fail]);
    let err = MsgPushTask
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("查判据失败要交回错误");
    assert!(err.to_string().contains("网络断了"), "实得：{err}");
    assert_eq!(http.count(), 1, "只发了那一枪");
}

// ══════════════════════════ 小结与明细 ══════════════════════════

/// 小结里**照抄**那五格读数，缺的键念「缺」而**不念 0**。
#[tokio::test]
async fn the_summary_prints_absent_fields_as_absent() {
    let (out, _, _) = run_with(vec![Reply::Body(status_body(Some(1), None))]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    // 上游那块 `result` 里我们只放了 pushOn / firstTaskStatus / secondTaskStatus，
    // 于是 onDuaration 与 total 应当是「缺」。
    assert!(reason.contains("secondTaskStatus=缺"), "实得：{reason}");
    assert!(reason.contains("total=缺"), "实得：{reason}");
    assert!(reason.contains("pushOn=1"), "实得：{reason}");
    assert!(
        !reason.contains("total=0"),
        "缺不能念成 0（那是两件事）：{reason}"
    );
}

/// 小结里念的是上游**拼错**的那个键名（读的是哪一个键，就念哪一个）。
#[tokio::test]
async fn the_summary_uses_the_misspelled_duration_key() {
    let (out, _, _) = run_with(vec![Reply::Body(CAPTURED_STATUS.to_string())]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("onDuaration=16"), "实得：{reason}");
    assert!(!reason.contains("onDuration"), "不许出现正确拼法：{reason}");
}

/// `pushTaskStatus` **不进小结**：它既不是判据，也没有界面读数能与它对上号。
#[tokio::test]
async fn push_task_status_stays_out_of_the_summary() {
    let (out, _, _) = run_with(vec![Reply::Body(CAPTURED_STATUS.to_string())]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(!reason.contains("pushTaskStatus"), "实得：{reason}");
}

/// 首次奖励那句提示只出现在**真的去领**那一趟，且**不改档位**
/// （旧项目里它不影响成功，只打一句 info）。
///
/// # 为什么"不该领"那几趟**不**提它
///
/// 参考实现里 `FirstRewardPending` 只在 `Obtain` 那条路上赋值 ——
/// 提前返回的 `Nothing` / `Manual` / `Unknown` 三档**不设**。
/// 我们照这个口径：那句话是对"这一趟领第二次奖励"的补充说明，
/// 没去领的时候它只是噪声（用户还没走到那一步）。
#[tokio::test]
async fn the_first_reward_notice_only_rides_along_with_a_claim() {
    // ① 真去领（secondTaskStatus=2）且首次未领 ⇒ 小结里有那句，**档位仍是成功**
    let claimable = r#"{"code":0,"msg":"success","result":{
        "pushOn":1,"firstTaskStatus":1,"secondTaskStatus":2}}"#;
    let (out, _, _) = run_with(vec![
        Reply::Body(claimable.to_string()),
        Reply::Body(obtain_body(serde_json::json!({"obtainCode":0}))),
    ])
    .await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("首奖未领不该把领取本身判成失败，实得：{out:?}");
    };
    assert!(summary.contains("首次奖励"), "实得：{summary}");

    // ② 没去领（secondTaskStatus=1，实拍那个值）⇒ 不提它
    let not_claimable = r#"{"code":0,"msg":"success","result":{
        "pushOn":1,"firstTaskStatus":1,"secondTaskStatus":1}}"#;
    let (out, _, _) = run_with(vec![Reply::Body(not_claimable.to_string())]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(
        !reason.contains("首次奖励"),
        "没去领的那趟不该提首奖（与参考实现一致）：{reason}"
    );
}

/// 结论走明细通道，且读数在里面（含那两格"旧描述没提"的新键）。
#[tokio::test]
async fn the_readings_are_reported_as_details() {
    let (out, _, sink) = run_with(vec![
        Reply::Body(status_body(Some(1), Some(2))),
        Reply::Body(obtain_body(serde_json::json!({"obtainCode":0}))),
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Success { .. }));

    let details = sink.details();
    assert_eq!(details.len(), 1, "这一条只自报一次");
    let d = &details[0];
    assert_eq!(d["kind"], "msg-push");
    assert_eq!(d["verdict"], "done");
    assert_eq!(d["obtainCode"], 0);
    assert_eq!(d["readings"]["pushOn"], 1);
    assert_eq!(d["readings"]["secondTaskStatus"], 2);
}

/// 判定档位的标识是**稳定字符串**（不是 `Debug` 输出）。
#[test]
fn verdict_slugs_are_stable() {
    assert_eq!(verdict_slug(MsgPushVerdict::Done), "done");
    assert_eq!(verdict_slug(MsgPushVerdict::Nothing), "nothing");
    assert_eq!(verdict_slug(MsgPushVerdict::Manual), "manual");
    assert_eq!(verdict_slug(MsgPushVerdict::Unknown), "unknown");
    assert_eq!(verdict_slug(MsgPushVerdict::Failed), "failed");
}

/// `MsgPushResult` 的零值档位必须是 **Failed**，不能是 Done。
///
/// 这个结构体有多条提前 `return` 的路径 —— 零值若是"已领"，
/// 一句"领到了"就会在**一个请求都没发**的那趟上被念出来。
#[test]
fn the_zero_verdict_is_failed_not_done() {
    assert_eq!(MsgPushVerdict::default(), MsgPushVerdict::Failed);
}

// ══════════════════════════ 注册表接入 ══════════════════════════

/// ⭐ `msg-push` 真的注册进了 registry —— CLI 与 `/api/capabilities` 都由它驱动。
///
/// 这条是"接入"这件事的判据：光有任务类型不注册，用户在 CLI 里
/// `run msg-push` 会拿到"未知任务"，而 `/api/capabilities` 里根本不出现。
#[test]
fn msg_push_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let metas = registry.list();
    let meta = metas
        .iter()
        .find(|m| m.name == "msg-push")
        .expect("msg-push 必须注册（CLI/capabilities 都按 registry 走）");

    assert_eq!(meta.title, "消息推送");
    assert!(
        meta.params.is_empty(),
        "这一条没有参数（判据全在上游响应里，不需要用户填）"
    );
    assert!(
        !meta.hidden,
        "它该出现在通用任务列表里 —— 隐藏的只有 exchange 与 auth-refresh"
    );

    // 旧的几条没被这次改动碰掉
    for name in ["sign", "exchange", "live-room"] {
        assert!(
            metas.iter().any(|m| m.name == name),
            "{name} 仍应在：{:?}",
            metas.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
        );
    }
}

// ══════════════════════════ store 替身 ══════════════════════════

/// 这一条链**不该碰 store**。做成"一碰就报错"的替身，
/// 于是"哪天有人给它加了一句读 store"会立刻炸出来。
struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("签到族不该碰 store".into())
}

#[async_trait]
impl mcloud_core::AccountStore for NoStore {
    async fn accounts(&self) -> Result<Vec<Account>> {
        Err(refuse())
    }
    async fn account(&self, _id: &mcloud_core::AccountId) -> Result<Option<Account>> {
        Err(refuse())
    }
    async fn save_account(&self, _a: &Account) -> Result<()> {
        Err(refuse())
    }
    async fn remove_account(&self, _id: &mcloud_core::AccountId) -> Result<()> {
        Err(refuse())
    }
    async fn account_state(&self, _id: &mcloud_core::AccountId) -> Result<serde_json::Value> {
        Err(refuse())
    }
    async fn save_account_state(
        &self,
        _id: &mcloud_core::AccountId,
        _s: &serde_json::Value,
    ) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::RunStore for NoStore {
    async fn upsert_run(&self, _r: &mcloud_core::RunRecord) -> Result<()> {
        Err(refuse())
    }
    async fn runs(&self, _limit: usize) -> Result<Vec<mcloud_core::RunRecord>> {
        Err(refuse())
    }
    async fn run(&self, _id: mcloud_core::RunId) -> Result<Option<mcloud_core::RunRecord>> {
        Err(refuse())
    }
    async fn running_runs(&self) -> Result<Vec<mcloud_core::RunRecord>> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::LogStore for NoStore {
    async fn append_logs(
        &self,
        _id: mcloud_core::RunId,
        _lines: &[mcloud_core::LogLine],
    ) -> Result<mcloud_core::AppendOutcome> {
        Err(refuse())
    }
    async fn logs(
        &self,
        _id: mcloud_core::RunId,
        _after: Option<mcloud_core::Seq>,
    ) -> Result<Vec<mcloud_core::LogLine>> {
        Err(refuse())
    }
    async fn clear_logs(&self, _id: mcloud_core::RunId) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::ScheduleStore for NoStore {
    async fn schedules(&self) -> Result<Vec<mcloud_core::Schedule>> {
        Err(refuse())
    }
    async fn schedule(
        &self,
        _id: &mcloud_core::ScheduleId,
    ) -> Result<Option<mcloud_core::Schedule>> {
        Err(refuse())
    }
    async fn save_schedule(&self, _s: &mcloud_core::Schedule) -> Result<()> {
        Err(refuse())
    }
    async fn remove_schedule(&self, _id: &mcloud_core::ScheduleId) -> Result<()> {
        Err(refuse())
    }
    async fn mark_schedule_fired(&self, _id: &mcloud_core::ScheduleId, _ms: i64) -> Result<()> {
        Err(refuse())
    }
    async fn record_schedule_run(
        &self,
        _id: &mcloud_core::ScheduleId,
        _ms: Option<i64>,
        _status: &str,
        _err: Option<&str>,
    ) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::PipelineStore for NoStore {
    async fn pipelines(&self) -> Result<Vec<mcloud_core::Pipeline>> {
        Err(refuse())
    }
    async fn pipeline(
        &self,
        _id: &mcloud_core::PipelineId,
    ) -> Result<Option<mcloud_core::Pipeline>> {
        Err(refuse())
    }
    async fn save_pipeline(&self, _p: &mcloud_core::Pipeline) -> Result<()> {
        Err(refuse())
    }
    async fn remove_pipeline(&self, _id: &mcloud_core::PipelineId) -> Result<()> {
        Err(refuse())
    }
    async fn mark_pipeline_ran(&self, _id: &mcloud_core::PipelineId, _ms: i64) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::UserStore for NoStore {
    async fn user_count(&self) -> Result<usize> {
        Err(refuse())
    }
    async fn users(&self) -> Result<Vec<mcloud_core::User>> {
        Err(refuse())
    }
    async fn user_by_name(&self, _name: &str) -> Result<Option<mcloud_core::User>> {
        Err(refuse())
    }
    async fn user(&self, _id: &mcloud_core::UserId) -> Result<Option<mcloud_core::User>> {
        Err(refuse())
    }
    async fn save_user(&self, _u: &mcloud_core::User) -> Result<()> {
        Err(refuse())
    }
    async fn session(&self, _h: &str) -> Result<Option<mcloud_core::Session>> {
        Err(refuse())
    }
    async fn save_session(&self, _s: &mcloud_core::Session) -> Result<()> {
        Err(refuse())
    }
    async fn remove_session(&self, _h: &str) -> Result<()> {
        Err(refuse())
    }
    async fn remove_user_sessions(&self, _id: &mcloud_core::UserId) -> Result<usize> {
        Err(refuse())
    }
    async fn purge_expired_sessions(&self, _now_ms: i64) -> Result<usize> {
        Err(refuse())
    }
}
