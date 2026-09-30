//! 红包派对任务层的测试。
//!
//! 钉四类东西：
//!
//! * **会话顺序**：先 GET `querySpecToken`（targetSourceId=001216）→ 再 POST
//!   `user/tokenValidate` → 之后每条业务请求的 `token` 头都是换回来的那个；
//!   换票那一枪的 `token` 头是**空串**（签名也按空串算）。
//! * **只读这件事**：这一趟正好 4 枪，且路径逐条钉死 —— 写入侧那 7 条今天没接，
//!   测试就把「没接」钉住（接上时这一条必须一起改）。
//! * **读数怎么念**：未知状态码单独成档、`userStatus` 缺失按未完成、
//!   空表不念成「没事做」、余额的「没读成」与「上游没给数」分清。
//! * **凭据不进台账**：`details` 里不许出现 token / SSO 票。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

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

/// 一个只带 `http` 的 ctx，外加能回读的 sink。**时钟是入参** ——
/// 活动期是判据的一部分，测试必须能把它挪到任意时刻。
fn ctx_with_at(replies: Vec<Reply>, now_ms: i64) -> (TaskContext, Arc<Scripted>, Arc<CollectSink>) {
    let http = Arc::new(Scripted::new(replies));
    let sink = Arc::new(CollectSink::default());
    let ctx = TaskContext {
        account: account(),
        http: http.clone(),
        store: Arc::new(NoStore),
        clock: Arc::new(FixedClock(now_ms)),
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
/// 这一条链**不该碰 store** —— 一碰就报错的替身。
struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("拍拍这条链不该碰 store".into())
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

// ══════════════════════════ 夹具 ══════════════════════════

const SSO_BODY: &str = r#"{"code":0,"msg":"成功","result":"SSO-TICKET"}"#;
const TOKEN_BODY: &str = r#"{"header":{"status":"200"},"data":{"token":"CPT-TOKEN"}}"#;
/// 三组各一条：新手（未完成）/ 每日（已完成）/ 每月（**没见过的码 2**）。
const TASKS_BODY: &str = r#"{"header":{"status":"200"},"data":{
 "configTaskSignList":[{"isToday":1,"status":0,"signAmount":10}],
 "configTaskNoviceList":[{"id":1,"taskCode":"NOVICE_1","taskName":"新手签到","userStatus":0,"prizeAmount":100}],
 "configTaskDailyList":[{"id":2,"taskCode":"DAILY_1","taskName":"每日一次","userStatus":1,"taskAmount":20}],
 "configTaskMonthlyList":[{"id":3,"taskCode":"MONTHLY_1","taskName":"每月一次","userStatus":2}]}}"#;
/// 有一项**没有** `userStatus`（旧 TS 读 `Number(userStatus ?? 0)` ⇒ 未完成）。
const MISSING_STATUS_BODY: &str = r#"{"header":{"status":"200"},"data":{
 "configTaskNoviceList":[{"id":1,"taskCode":"NOVICE_1","taskName":"新手签到"}]}}"#;
const BALANCE_BODY: &str =
    r#"{"header":{"status":"200"},"data":{"info":{"canAmount":1234,"totalAmount":56789}}}"#;
const EMPTY_TASKS_BODY: &str = r#"{"header":{"status":"200"},"data":{}}"#;

fn task() -> CloudPhoneRedpackTask {
    // 顺序随机源 ⇒ requestId / x-DeviceInfo 里那截随机串可复现
    CloudPhoneRedpackTask::with_rng(Box::new(mcloud_crypto::SeqRandom::new(1)))
}

fn all_ok() -> Vec<Reply> {
    vec![
        Reply::Body(SSO_BODY.into()),
        Reply::Body(TOKEN_BODY.into()),
        Reply::Body(TASKS_BODY.into()),
        Reply::Body(BALANCE_BODY.into()),
    ]
}

async fn run_with(replies: Vec<Reply>) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    let (ctx, http, sink) = ctx_with_at(replies, 1_700_000_000_000);
    let out = task().run(&ctx, &json!({})).await.expect("不该是 Err");
    (out, http, sink)
}

fn summary_of(out: &TaskOutcome) -> String {
    match out {
        TaskOutcome::Success { summary } => summary.clone(),
        TaskOutcome::Failed { reason } | TaskOutcome::Skipped { reason } => reason.clone(),
    }
}

fn h(req: &HttpRequest, name: &str) -> Option<String> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn body_of(req: &HttpRequest) -> Value {
    serde_json::from_slice(req.body.as_deref().expect("该带体")).unwrap()
}

/// 去掉 scheme/host 与查询串的路径。
fn path_of(req: &HttpRequest) -> String {
    let after_host = req.url.split_once("://").map_or(&*req.url, |(_, r)| r);
    match after_host.split_once('/') {
        Some((_, rest)) => {
            let path = rest.split('?').next().unwrap_or(rest);
            format!("/{path}")
        }
        None => after_host.to_string(),
    }
}

/// 四枪的路径**逐条钉死**。多一枪（比如接上写入侧）这条就会红 —— 那是故意的。
const READ_PATHS: [&str; 4] = [
    "/ycloud/api/cloud/userdomain/v2/querySpecToken",
    "/cloudphone-market/user/tokenValidate",
    "/cloudphone-market/redpacket/configTaskLoginList",
    "/cloudphone-market/redpacket/userAccountInfo",
];

// ══════════════════════════ 会话 ══════════════════════════

#[tokio::test]
async fn session_switches_the_sso_ticket_for_the_activity_token() {
    let (out, http, _) = run_with(all_ok()).await;
    let reqs = http.requests();
    assert_eq!(reqs.len(), 4);

    // ① 换 SSO 票：GET，带 targetSourceId，且**没有体**
    assert_eq!(reqs[0].method, "GET");
    assert!(
        reqs[0].url.contains("targetSourceId=001216"),
        "{}",
        reqs[0].url
    );
    assert!(reqs[0].body.is_none());

    // ② 换活动 token：这一枪的 token 头是**空串**，SSO 票在请求体里
    assert_eq!(reqs[1].method, "POST");
    assert_eq!(h(&reqs[1], "token").as_deref(), Some(""));
    assert_eq!(body_of(&reqs[1])["token"], json!("SSO-TICKET"));
    assert_eq!(body_of(&reqs[1])["deviceId"], json!("DEV+1/base="));

    // ③④ 业务请求带的是换回来的那个 token
    for r in &reqs[2..] {
        assert_eq!(h(r, "token").as_deref(), Some("CPT-TOKEN"), "{}", r.url);
    }

    assert!(matches!(out, TaskOutcome::Success { .. }), "{out:?}");
    let s = summary_of(&out);
    assert!(s.contains("红包派对任务 3 件"), "{s}");
    assert!(s.contains("余额 12.34 元（累计 567.89 元）"), "{s}");
    assert!(s.contains("本工具只读"), "{s}");
}

#[tokio::test]
async fn the_inventory_only_reads_the_four_known_read_paths() {
    let (_, http, _) = run_with(all_ok()).await;
    let got: Vec<String> = http.requests().iter().map(path_of).collect();
    assert_eq!(got, READ_PATHS.to_vec(), "多一枪都得在这里看见");
}

// ══════════════════════════ 判读 ══════════════════════════

#[tokio::test]
async fn an_unreadable_user_status_is_its_own_bucket() {
    let (out, _, sink) = run_with(all_ok()).await;
    let d = sink.details();
    assert_eq!(d.len(), 1, "只该有一次 details");
    let c = &d[0]["counts"];
    assert_eq!(c["total"], json!(3));
    assert_eq!(c["waiting"], json!(1));
    assert_eq!(c["done"], json!(1));
    assert_eq!(c["unknown"], json!(1), "码 2 没见过 ⇒ 单独成档");
    assert_eq!(
        d[0]["tasks"][2]["statusText"],
        Value::Null,
        "未知码不许编文案"
    );
    assert!(
        summary_of(&out).contains("状态读不出 1"),
        "{:?}",
        summary_of(&out)
    );
}

#[tokio::test]
async fn a_missing_user_status_counts_as_waiting() {
    let replies = vec![
        Reply::Body(SSO_BODY.into()),
        Reply::Body(TOKEN_BODY.into()),
        Reply::Body(MISSING_STATUS_BODY.into()),
        Reply::Body(BALANCE_BODY.into()),
    ];
    let (out, _, sink) = run_with(replies).await;
    let d = sink.details();
    assert_eq!(d[0]["counts"]["waiting"], json!(1), "旧 TS 的 ?? 0 照做");
    assert_eq!(d[0]["counts"]["unknown"], json!(0));
    assert!(
        summary_of(&out).contains("未完成 1"),
        "{:?}",
        summary_of(&out)
    );
}

#[tokio::test]
async fn an_empty_task_table_is_all_zeros_and_not_a_conclusion() {
    let replies = vec![
        Reply::Body(SSO_BODY.into()),
        Reply::Body(TOKEN_BODY.into()),
        Reply::Body(EMPTY_TASKS_BODY.into()),
        Reply::Body(BALANCE_BODY.into()),
    ];
    let (out, _, sink) = run_with(replies).await;
    let c = &sink.details()[0]["counts"];
    for k in ["total", "waiting", "done", "soldOut", "tomorrow", "unknown"] {
        assert_eq!(c[k], json!(0), "{k}");
    }
    let s = summary_of(&out);
    assert!(s.contains("任务 0 件"), "{s}");
    assert!(
        !s.contains("没事做") && !s.contains("没有可做"),
        "空数组不等于「没任务」：{s}"
    );
}

// ══════════════════════════ 致命读与半份结论 ══════════════════════════

#[tokio::test]
async fn a_broken_session_stops_at_the_first_shot() {
    let (out, http, sink) = run_with(vec![Reply::Body("not json".into())]).await;
    assert_eq!(http.count(), 1, "第一枪就废了 ⇒ 后面一枪都不许发");
    let s = summary_of(&out);
    assert!(s.contains("没读成"), "{s}");
    assert!(matches!(out, TaskOutcome::Failed { .. }), "{out:?}");
    assert_eq!(sink.details()[0]["verdict"], json!("failed"));
}

#[tokio::test]
async fn a_broken_balance_does_not_take_the_inventory_down() {
    let replies = vec![
        Reply::Body(SSO_BODY.into()),
        Reply::Body(TOKEN_BODY.into()),
        Reply::Body(TASKS_BODY.into()),
        Reply::Body(r#"{"header":{"status":500,"errMsg":"系统繁忙"}}"#.into()),
    ];
    let (out, _, sink) = run_with(replies).await;
    assert!(matches!(out, TaskOutcome::Success { .. }), "{out:?}");
    assert_eq!(sink.details()[0]["counts"]["total"], json!(3), "任务表照报");
    assert_eq!(sink.details()[0]["account"], Value::Null);
    assert!(
        summary_of(&out).contains("余额没读成"),
        "{:?}",
        summary_of(&out)
    );
}

#[tokio::test]
async fn cancellation_stops_before_the_first_shot() {
    let (ctx, http, _) = ctx_with_at(all_ok(), 0);
    ctx.cancel.cancel();
    let err = task()
        .run(&ctx, &json!({}))
        .await
        .expect_err("取消该是 Err");
    assert!(err.is_cancelled(), "{err:?}");
    assert_eq!(http.count(), 0, "取消后一枪都不发");
}

// ══════════════════════════ 凭据与明细 ══════════════════════════

#[tokio::test]
async fn details_never_carry_the_tokens() {
    let (_, _, sink) = run_with(all_ok()).await;
    let text = serde_json::to_string(&sink.details()).unwrap();
    assert!(!text.contains("CPT-TOKEN"), "token 是凭据，不许进台账");
    assert!(!text.contains("SSO-TICKET"), "SSO 票是凭据，不许进台账");
}

#[tokio::test]
async fn details_report_the_sign_grid_separately_from_tasks() {
    let (_, _, sink) = run_with(all_ok()).await;
    let d = sink.details();
    assert_eq!(d[0]["signDays"][0]["isToday"], json!(1));
    assert_eq!(d[0]["signDays"][0]["signAmount"], json!(10));
    assert_eq!(d[0]["counts"]["total"], json!(3), "签到格不进任务计数");
}

// ══════════════════════════ 接入与纯函数 ══════════════════════════

#[test]
fn the_registry_knows_this_task() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");
    let t = registry.get("cloud-phone-redpack").expect("应当注册了");
    assert_eq!(t.title(), "红包派对盘点");
}

#[test]
fn yuan_is_integer_arithmetic() {
    assert_eq!(yuan(1234), "12.34 元");
    assert_eq!(yuan(0), "0.00 元");
    assert_eq!(yuan(5), "0.05 元");
    assert_eq!(yuan(-5), "-0.05 元");
}

#[test]
fn request_id_and_device_nonce_have_the_old_ts_shape() {
    let mut rng = mcloud_crypto::SeqRandom::new(7);
    let clock = FixedClock(1_700_000_000_000);
    let id = request_id(&mut rng, &clock, 1_700_000_000_000);
    // yyyyMMddHHmmss(14) + epoch 毫秒(13) + base36(8)
    assert_eq!(id.len(), 35, "{id}");
    assert!(
        id.starts_with(&mcloud_core::beijing_time_digits(&clock)),
        "{id}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
        "{id}"
    );
    let nonce = base36(&mut rng, 16);
    assert_eq!(nonce.len(), 16);
    assert!(
        nonce
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
        "{nonce}"
    );
}

#[tokio::test]
async fn a_network_error_at_the_first_shot_is_also_fatal() {
    // 换票那一枪连不上 ⇒ 一样是「没读成」，不许折成半份结论。
    let (out, http, _) = run_with(vec![Reply::Fail]).await;
    assert_eq!(http.count(), 1);
    assert!(matches!(out, TaskOutcome::Failed { .. }), "{out:?}");
    assert!(
        summary_of(&out).contains("网络断了"),
        "{:?}",
        summary_of(&out)
    );
}
