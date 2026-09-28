//! 拍拍任务层的测试。
//!
//! 钉两类东西：
//!
//! * **读数怎么念**（`summary_line` / `counts`）—— 尤其"状态读不出"那一档
//!   不许被折进"未完成"，以及次数那一格的 `None` 与 `0` **不是一回事**。
//! * **只读这件事**（`http.count()` 与两条 URL）—— 这一条链今天**一个写请求都不发**，
//!   而它上游那三口（上报一步 / 抽奖 / 写网盘）都不是能撤的。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

const STATUS_BODY: &str = r#"{"code":0,"msg":"success","result":{"totalLotteryCount":1}}"#;
const TASKS_BODY: &str = r#"{"code":0,"msg":"success","result":[
 {"id":1,"name":"体验拍照问AI","flag":"拍照问AI","description":"抽奖次数+1",
  "groupId":"normal","state":"FINISH","limitType":"DAY","recordid":"20674897","currstep":3,"process":0},
 {"id":2,"name":"拍夏日关键词","flag":"拍夏日关键词","description":"抽奖次数+1",
  "groupId":"normal","state":"WAIT","limitType":"DAY","recordid":null,"currstep":0,"process":0},
 {"id":31,"name":"去小红书点赞","flag":"小红书","description":"抽奖次数+1",
  "groupId":"popup","state":"WAIT","limitType":"EVER","recordid":null,"currstep":0,"process":0}]}"#;

fn board_of(states: &[Option<&str>]) -> PlayAiBoard {
    PlayAiBoard {
        status: PlayAiUserStatus::default(),
        tasks: states
            .iter()
            .map(|s| PlayAiUserTask {
                state: s.map(str::to_string),
                ..Default::default()
            })
            .collect(),
    }
}

fn read_result(board: PlayAiBoard) -> PlayAiResult {
    PlayAiResult {
        verdict: PlayAiVerdict::Read,
        board,
        error: String::new(),
    }
}

// ══════════════════════════ 计数（纯函数） ══════════════════════════

/// 三档各归各的。
#[test]
fn counts_split_the_three_buckets() {
    let board = board_of(&[Some("FINISH"), Some("WAIT"), Some("WAIT")]);
    let c = board.counts();
    assert_eq!((c.total, c.done, c.waiting, c.unknown), (3, 1, 2, 0));
}

/// ⚠ "状态读不出"**单独成档**，**不许**折进"未完成"。
///
/// 折进去的表现是：将来接写请求时，一条读不出状态的任务会被当成"还没做"，
/// 于是**重复上报一次**。
#[test]
fn an_unreadable_state_is_its_own_bucket() {
    let board = board_of(&[Some("FINISH"), Some("WAIT"), None, Some("SOMETHING_NEW")]);
    let c = board.counts();
    assert_eq!(
        (c.total, c.done, c.waiting, c.unknown),
        (4, 1, 1, 2),
        "缺键与没见过的值都进 unknown，且都没进 waiting"
    );
    assert_eq!(
        c.waiting, 1,
        "unknown 一旦折进 waiting，那些条目会被当成「还没做」而重复上报"
    );
}

/// 空表 ⇒ 四个 0，且**不**下"这个号没事做"的结论。
#[test]
fn an_empty_board_is_all_zeros() {
    let board = board_of(&[]);
    assert_eq!(board.counts(), PlayAiTaskCounts::default());
    let summary = read_result(board).summary_line();
    assert!(summary.contains("任务 0 件"), "实得：{summary}");
    assert!(
        !summary.contains("没事做") && !summary.contains("没有可做"),
        "空数组不等于「没任务」——上游两种情形同一种回法：{summary}"
    );
}

// ══════════════════════════ 小结 ══════════════════════════

/// 次数那一格：`0` 要念成"读到 0"，**不能**念成"没给"。
///
/// 实拍两条都有值（`0` 与 `1`），所以"没给"是一种**没样本**的形态。
#[test]
fn a_zero_count_is_read_not_missing() {
    let mut board = board_of(&[]);
    board.status.total_lottery_count = Some(0);
    let summary = read_result(board).summary_line();
    assert!(summary.contains("读到 0"), "实得：{summary}");
    assert!(
        !summary.contains("没给"),
        "0 是实拍值，不是「没给」：{summary}"
    );
}

/// 次数那一格真没给 ⇒ 明说"没给"，且不猜它等于几。
#[test]
fn a_missing_count_says_so() {
    let summary = read_result(board_of(&[])).summary_line();
    assert!(summary.contains("没给"), "实得：{summary}");
    assert!(!summary.contains("读到 0"), "没给 ≠ 0：{summary}");
}

/// 无论哪一种，都要带上"这一格没解释"那句 —— 不然它会连着数字被引用成结论。
#[test]
fn the_hedge_always_rides_along_with_the_number() {
    for value in [Some(0), Some(1), Some(7)] {
        let mut board = board_of(&[]);
        board.status.total_lottery_count = value;
        let summary = read_result(board).summary_line();
        assert!(
            summary.contains(LOTTERY_COUNT_HEDGE),
            "读到数字时必须同时说清它没解释：{summary}"
        );
    }
}

/// 小结里**不出现**任务名（上游文案不进我们对外承诺的那一列）。
#[test]
fn the_summary_never_prints_upstream_task_names() {
    let board = PlayAiBoard {
        status: PlayAiUserStatus {
            total_lottery_count: Some(1),
        },
        tasks: vec![PlayAiUserTask {
            name: Some("拍夏日关键词".into()),
            flag: Some("小红书".into()),
            state: Some("WAIT".into()),
            ..Default::default()
        }],
    };
    let summary = read_result(board).summary_line();
    assert!(!summary.contains("拍夏日关键词"), "实得：{summary}");
    assert!(!summary.contains("小红书"), "实得：{summary}");
    assert!(summary.contains("未完成 1"), "实得：{summary}");
}

/// 只读这件事要写在结论里 —— 跑它的人不该以为顺手替他把任务做了。
#[test]
fn the_summary_states_that_it_is_read_only() {
    let summary = read_result(board_of(&[Some("WAIT")])).summary_line();
    assert!(summary.contains("只读"), "实得：{summary}");
}

/// 零值档位是 **Failed**，不是 `Read`。
#[test]
fn the_zero_verdict_is_failed() {
    assert_eq!(PlayAiVerdict::default(), PlayAiVerdict::Failed);
    // 于是零值结果念的是"没读成"，而不是一句看起来正常的"任务 0 件"
    let zero = PlayAiResult::default().summary_line();
    assert!(zero.contains("没读成"), "实得：{zero}");
    assert!(matches!(
        PlayAiResult::default().task_outcome(),
        TaskOutcome::Failed { .. }
    ));
}

// ══════════════════════════ 只读那两枪 ══════════════════════════

/// 两枪、都是 GET，且**没有任何写路径**。
#[tokio::test]
async fn the_run_sends_exactly_two_get_requests() {
    let (out, http, _) = run_with(vec![
        Reply::Body(STATUS_BODY.to_string()),
        Reply::Body(TASKS_BODY.to_string()),
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 2, "这一条链只发两枪");
    let reqs = http.requests();
    assert_eq!(
        reqs[0].url,
        "https://m.mcloud.139.com/ycloud/newyear/user/info?marketName=National_PlayAISpecial",
        "① 先查状态（实拍顺序）"
    );
    assert_eq!(
        reqs[1].url,
        "https://m.mcloud.139.com/ycloud/newyear/task/list?marketName=National_PlayAISpecial&source=app"
    );
    for r in &reqs {
        assert_eq!(r.method, "GET");
        assert!(r.body.is_none(), "两枪都无请求体");
        for forbidden in ["lottery", "step/click", "aiRecognize", "trash", "upload"] {
            assert!(
                !r.url.contains(forbidden),
                "这一条链不许出现写路径 `{forbidden}`：{}",
                r.url
            );
        }
    }
}

/// 第一枪就挂 ⇒ 交回 `Err`，且**一枪都没往下走**。
#[tokio::test]
async fn a_failed_status_query_is_fatal_and_stops() {
    let (ctx, http, _) = ctx_with(vec![Reply::Fail]);
    let err = PlayAiTask
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("① 是致命读");
    assert!(err.to_string().contains("网络断了"), "实得：{err}");
    assert_eq!(http.count(), 1, "① 挂了就不该再打 ②");
}

/// 第二枪挂 ⇒ 同样致命（**不**交半份结论）。
///
/// 缺任务表会让用户以为这个号没事做 —— 那是会改变行为的误读。
#[tokio::test]
async fn a_failed_task_list_is_also_fatal() {
    let (ctx, http, _) = ctx_with(vec![Reply::Body(STATUS_BODY.to_string()), Reply::Fail]);
    let err = PlayAiTask
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("② 也是致命读");
    assert!(err.to_string().contains("网络断了"), "实得：{err}");
    assert_eq!(http.count(), 2, "② 确实发出去了");
}

/// 非 0 业务码 ⇒ `Err`（不是"半份结论"）。
#[tokio::test]
async fn a_non_zero_code_on_either_shot_is_fatal() {
    let (ctx, _, _) = ctx_with(vec![Reply::Body(
        r#"{"code":500,"msg":"系统繁忙","result":{}}"#.to_string(),
    )]);
    assert!(PlayAiTask.run(&ctx, &serde_json::json!({})).await.is_err());

    let (ctx, _, _) = ctx_with(vec![
        Reply::Body(STATUS_BODY.to_string()),
        Reply::Body(r#"{"code":500,"msg":"系统繁忙","result":[]}"#.to_string()),
    ]);
    assert!(PlayAiTask.run(&ctx, &serde_json::json!({})).await.is_err());
}

// ══════════════════════════ 明细 ══════════════════════════

/// 任务级读数要进明细 —— 小结刻意不解释，那些格子只能走这里。
#[tokio::test]
async fn the_task_level_readings_are_reported_as_details() {
    let (out, _, sink) = run_with(vec![
        Reply::Body(STATUS_BODY.to_string()),
        Reply::Body(TASKS_BODY.to_string()),
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Success { .. }));

    let details = sink.details();
    assert_eq!(details.len(), 1, "只自报一次");
    let d = &details[0];
    assert_eq!(d["kind"], "play-ai-special");
    assert_eq!(d["verdict"], "read");
    assert_eq!(d["totalLotteryCount"], 1);
    assert_eq!(d["lotteryCountHedge"], LOTTERY_COUNT_HEDGE);
    assert_eq!(d["counts"]["total"], 3);
    assert_eq!(d["counts"]["done"], 1);
    assert_eq!(d["counts"]["waiting"], 2);
    assert_eq!(d["counts"]["unknown"], 0);

    let tasks = d["tasks"].as_array().expect("任务数组");
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0]["taskId"], 1);
    assert_eq!(tasks[0]["state"], "FINISH");
    assert_eq!(tasks[0]["currStep"], 3);
    assert_eq!(tasks[0]["recordId"], "20674897");
    assert_eq!(tasks[0]["groupId"], "normal");
    assert_eq!(
        tasks[1]["recordId"],
        serde_json::Value::Null,
        "没做过时是 null"
    );
}

// ══════════════════════════ 注册表接入 ══════════════════════════

/// ⭐ `play-ai-special` 真的注册进了 registry。
#[test]
fn play_ai_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let metas = registry.list();
    let meta = metas
        .iter()
        .find(|m| m.name == "play-ai-special")
        .expect("play-ai-special 必须注册（CLI/capabilities 都按 registry 走）");
    assert_eq!(meta.title, "拍拍盘点");
    assert!(meta.params.is_empty(), "这一条没有参数");
    assert!(!meta.hidden);
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

/// 跑一趟；本链不依赖活动期，时钟给 0 即可。
async fn run_with(replies: Vec<Reply>) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    let (ctx, http, sink) = ctx_with_at(replies, 0);
    let out = PlayAiTask.run(&ctx, &serde_json::json!({})).await;
    (out.expect("不该是 Err"), http, sink)
}

/// 同上，但把 `Err` 也交回（用来钉"致命读"那一档）。
fn ctx_with(replies: Vec<Reply>) -> (TaskContext, Arc<Scripted>, Arc<CollectSink>) {
    ctx_with_at(replies, 0)
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
