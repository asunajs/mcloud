//! 趣玩AI 任务层的测试。
//!
//! 钉两类东西：
//!
//! * **判定**（`plan_funai` / `funai_window_of`）—— 纯函数，每档边界都摆出来，
//!   尤其是"字段缺失"与"给了 0"**不是一回事**、以及活动期三态。
//! * **写请求发不发**（`http.count()`）—— 这一条链最贵的错误是
//!   "判据没过就把 `page/lottery` 发出去"：每天只有一次额度，发了就回不来。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

// ══════════════════════════ 夹具 ══════════════════════════

/// 活动期（东八区）：`2025-05-28T00:00:00` ~ `2026-12-31T23:59:59`，实拍形态。
const START_MS: i64 = 1_748_361_600_000;
const END_MS: i64 = 1_798_732_799_000;

/// 一个落在活动期内的时刻（给 `plan_funai` 用）。
const IN_WINDOW_MS: i64 = 1_760_000_000_000;

fn module(draw_times: Option<i64>, draws_used: Option<i64>) -> FunaiModule {
    FunaiModule {
        module_id: "1-250730".into(),
        draw_times,
        draws_used,
        today_get_lottery_count: None,
        start_time: Some(START_MS),
        end_time: Some(END_MS),
    }
}

/// 造一份 `page/getmodule` 响应。`None` = **该键不出现**（与"给了 0"不是一回事）。
fn module_body(
    draw_times: Option<i64>,
    draws_used: Option<i64>,
    window: Option<(i64, i64)>,
) -> String {
    let mut result = serde_json::Map::new();
    result.insert("moduleId".into(), serde_json::json!("1-250730"));
    if let Some(v) = draw_times {
        result.insert("drawTimes".into(), serde_json::json!(v));
    }
    if let Some(v) = draws_used {
        result.insert("drawsUsed".into(), serde_json::json!(v));
    }
    if let Some((start, end)) = window {
        result.insert("startTime".into(), serde_json::json!(fmt_beijing(start)));
        result.insert("endTime".into(), serde_json::json!(fmt_beijing(end)));
    }
    serde_json::json!({"code":0,"msg":"success","result":result}).to_string()
}

/// 把 epoch 毫秒按东八区写成实拍那个形态（`2025-05-28T00:00:00`）。
///
/// 测试里要能造出时间串，而 `mcloud-tasks` 引不到 `chrono` ⇒ 手写一小段。
fn fmt_beijing(ms: i64) -> String {
    let local_secs = ms / 1000 + 8 * 3600;
    let days = local_secs.div_euclid(86_400);
    let sod = local_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// `days_from_civil` 的逆（Howard Hinnant 的 `civil_from_days`）。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn lottery_body(result: serde_json::Value) -> String {
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
        owner: None,
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

/// 跑一趟；时钟默认落在活动期内（[`IN_WINDOW_MS`]）。
async fn run_with(replies: Vec<Reply>) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    run_with_at(replies, IN_WINDOW_MS).await
}

/// 跑一趟，时钟指定。
async fn run_with_at(
    replies: Vec<Reply>,
    now_ms: i64,
) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    let (ctx, http, sink) = ctx_with_at(replies, now_ms);
    let out = FunaiTask.run(&ctx, &serde_json::json!({})).await;
    (out.expect("不该是 Err"), http, sink)
}

// ══════════════════════════ 活动期（纯函数） ══════════════════════════

/// 两个边界都在、`now` 夹在中间 ⇒ 在期。
#[test]
fn window_is_in_when_now_is_between_the_bounds() {
    let m = module(Some(1), Some(0));
    assert_eq!(funai_window_of(&m, IN_WINDOW_MS), FunaiWindow::In);
    // 含端点
    assert_eq!(funai_window_of(&m, START_MS), FunaiWindow::In);
    assert_eq!(funai_window_of(&m, END_MS), FunaiWindow::In);
}

/// 两个边界都没给 ⇒ **判不了**（不是"没有边界"，也不是"在期"）。
#[test]
fn window_is_unknown_when_no_bound_is_given() {
    let m = FunaiModule {
        module_id: "1-250730".into(),
        ..Default::default()
    };
    assert_eq!(funai_window_of(&m, IN_WINDOW_MS), FunaiWindow::Unknown);
}

/// 只给一个边界也能判：给了 `start` 就只夹那一边。
#[test]
fn a_single_bound_is_still_usable() {
    let only_start = FunaiModule {
        module_id: "1-250730".into(),
        start_time: Some(START_MS),
        ..Default::default()
    };
    assert_eq!(funai_window_of(&only_start, IN_WINDOW_MS), FunaiWindow::In);
    assert_eq!(funai_window_of(&only_start, START_MS - 1), FunaiWindow::Out);
}

/// ⚠ 区间**倒挂**（`start > end`）⇒ 任何时刻都落"不在期"，且**不需要**单开一格。
///
/// 反证：设 `start > end`，则任一时刻要么 `< start`、要么 `> end`，
/// 想同时躲开两条得满足 `start <= now <= end`，而它无解。
#[test]
fn an_inverted_window_leaves_no_moment_in_period() {
    let m = FunaiModule {
        module_id: "1-250730".into(),
        start_time: Some(END_MS),
        end_time: Some(START_MS),
        ..Default::default()
    };
    for now in [START_MS - 1, START_MS, IN_WINDOW_MS, END_MS, END_MS + 1] {
        assert_eq!(
            funai_window_of(&m, now),
            FunaiWindow::Out,
            "倒挂窗口里不该有任何时刻在期（now={now}）"
        );
    }
}

// ══════════════════════════ 判定（纯函数） ══════════════════════════

/// 在期 + 还有额度 ⇒ 可以抽。
#[test]
fn draw_when_in_window_and_chances_left() {
    assert_eq!(
        plan_funai(&module(Some(1), Some(0)), IN_WINDOW_MS),
        FunaiPlan::Draw
    );
    assert_eq!(
        plan_funai(&module(Some(3), Some(0)), IN_WINDOW_MS),
        FunaiPlan::Draw
    );
}

/// 今天已经抽过 ⇒ `AlreadyDrawn`（**即便** `draw_times` 还大于 0）。
///
/// 它排在"没次数"之前：两个条件同时成立时，"今天已经抽过"是更有用的那句 ——
/// 它同时回答了"明天再来有没有"。
#[test]
fn already_drawn_wins_over_no_chances() {
    assert_eq!(
        plan_funai(&module(Some(1), Some(1)), IN_WINDOW_MS),
        FunaiPlan::AlreadyDrawn
    );
    // 两档同时成立（draw_times 也见底）时仍报"已抽过"
    assert_eq!(
        plan_funai(&module(Some(0), Some(1)), IN_WINDOW_MS),
        FunaiPlan::AlreadyDrawn
    );
}

/// 额度见底但今天还没抽 ⇒ `NoChances`。
#[test]
fn no_chances_when_the_budget_is_gone() {
    assert_eq!(
        plan_funai(&module(Some(0), Some(0)), IN_WINDOW_MS),
        FunaiPlan::NoChances
    );
}

/// 判据字段缺失 ⇒ `NoCriteria`（**不发请求**），而**不是**当成"0 次"。
///
/// 实拍里 `timed/getmodule` 那一份三个计数**全是 `null`** —— 这一档不是假想分支。
#[test]
fn missing_criteria_is_unknown_not_zero() {
    assert_eq!(
        plan_funai(&module(None, Some(0)), IN_WINDOW_MS),
        FunaiPlan::NoCriteria
    );
    assert_eq!(
        plan_funai(&module(Some(1), None), IN_WINDOW_MS),
        FunaiPlan::NoCriteria
    );
    assert_eq!(
        plan_funai(&module(None, None), IN_WINDOW_MS),
        FunaiPlan::NoCriteria
    );
}

/// 明确不在期 ⇒ `OutOfWindow`，**且它比"判不了次数"更靠前**（与次数无关）。
#[test]
fn out_of_window_beats_missing_criteria() {
    let m = module(None, None); // 判据也缺
    assert_eq!(plan_funai(&m, END_MS + 1), FunaiPlan::OutOfWindow);
    assert_eq!(plan_funai(&m, START_MS - 1), FunaiPlan::OutOfWindow);
}

/// 次数都够、但活动期**判不了** ⇒ `WindowUnknown`（不发请求）。
///
/// 拿"多半还在期"去发写请求，代价是可能花掉今天唯一那一次。
#[test]
fn window_unknown_blocks_the_draw() {
    let m = FunaiModule {
        module_id: "1-250730".into(),
        draw_times: Some(1),
        draws_used: Some(0),
        ..Default::default() // 两界都没给
    };
    assert_eq!(plan_funai(&m, IN_WINDOW_MS), FunaiPlan::WindowUnknown);
}

/// 「今天已经抽过」那句理由里的数字必须跟着 [`FUNAI_DAILY_DRAW_LIMIT`] 走。
///
/// 它是个 `&'static str`，没法插值 ⇒ 靠这条测试把两者绑在一起。
/// 改常量却忘了改文案，表现是界面上写着"最多抽 1 次"而代码其实允许 2 次 ——
/// 一句**看起来对**的假话。
#[test]
fn the_already_drawn_reason_tracks_the_limit() {
    let reason = FunaiPlan::AlreadyDrawn.reason();
    assert!(
        reason.contains(&FUNAI_DAILY_DRAW_LIMIT.to_string()),
        "理由里要出现当前上限 {FUNAI_DAILY_DRAW_LIMIT}，实得：{reason}"
    );
}

/// 每档都有一句人话理由，且彼此不重复。
#[test]
fn every_plan_has_a_distinct_reason_and_slug() {
    let plans = [
        FunaiPlan::Draw,
        FunaiPlan::AlreadyDrawn,
        FunaiPlan::NoChances,
        FunaiPlan::NoCriteria,
        FunaiPlan::OutOfWindow,
        FunaiPlan::WindowUnknown,
    ];
    let mut reasons: Vec<&str> = plans.iter().map(|p| p.reason()).collect();
    reasons.sort();
    reasons.dedup();
    assert_eq!(reasons.len(), plans.len(), "理由不许重复");
    let mut slugs: Vec<&str> = plans.iter().map(|p| p.slug()).collect();
    slugs.sort();
    slugs.dedup();
    assert_eq!(slugs.len(), plans.len(), "标识不许重复");
}

/// 零值档位是 **Failed** 而不是 Done。
#[test]
fn the_zero_verdict_is_failed_not_done() {
    assert_eq!(FunaiVerdict::default(), FunaiVerdict::Failed);
}

// ══════════════════════════ 不该抽的那些：请求数必须恰好 1 ══════════════════════════

/// 判据说"没得抽" ⇒ **只发了读那一枪**，一个写请求都没有。
#[tokio::test]
async fn nothing_sends_no_write_request() {
    for (times, used) in [(Some(1), Some(1)), (Some(0), Some(0)), (None, None)] {
        let body = module_body(times, used, Some((START_MS, END_MS)));
        let (out, http, _) = run_with(vec![Reply::Body(body)]).await;
        assert!(
            matches!(out, TaskOutcome::Skipped { .. }),
            "({times:?},{used:?}) 应落跳过，实得：{out:?}"
        );
        assert_eq!(
            http.count(),
            1,
            "({times:?},{used:?}) 判据没过就不该发第二枪"
        );
        assert!(http.url_at(0).ends_with("/page/getmodule"));
    }
}

/// 不在期时也**不发**写请求。
#[tokio::test]
async fn out_of_window_sends_no_write_request() {
    // 造一个已经结束的窗口，并让时钟落在它之后
    let body = module_body(
        Some(1),
        Some(0),
        Some((START_MS - 200 * 86_400_000, START_MS - 100 * 86_400_000)),
    );
    let (out, http, _) = run_with_at(vec![Reply::Body(body)], IN_WINDOW_MS).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("不在活动起止时间"), "实得：{reason}");
    assert_eq!(http.count(), 1);
}

// ══════════════════════════ 该抽的那些 ══════════════════════════

/// 判据放行 ⇒ 两枪，第二枪打在 `/page/lottery` 上、body 里的 id 来自**第一枪**。
#[tokio::test]
async fn draw_sends_the_lottery_with_the_module_id_from_the_first_shot() {
    let (out, http, _) = run_with(vec![
        Reply::Body(module_body(Some(1), Some(0), Some((START_MS, END_MS)))),
        Reply::Body(lottery_body(
            serde_json::json!({"isWin":1,"prizeId":260107009,"prizeName":"30云朵"}),
        )),
    ])
    .await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 2);
    let second = &http.requests()[1];
    assert!(
        second.url.ends_with("/page/lottery"),
        "实得：{}",
        second.url
    );
    assert_eq!(second.method, "POST");
    assert_eq!(second.body_str(), Some(r#"{"moduleId":"1-250730"}"#));
    assert!(summary.contains("30云朵"), "实得：{summary}");
}

/// ⚠ **没中奖也是成功**：`code:0` + `isWin:0` + `prizeName` 全 `null`。
///
/// 拿它走失败支会把一次正常抽完写红。
#[tokio::test]
async fn a_losing_draw_is_still_a_success() {
    let (out, http, _) = run_with(vec![
        Reply::Body(module_body(Some(1), Some(0), Some((START_MS, END_MS)))),
        Reply::Body(lottery_body(
            serde_json::json!({"isWin":0,"prizeId":null,"prizeName":null,"prizeType":1}),
        )),
    ])
    .await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("没中奖是成功，不是失败，实得：{out:?}");
    };
    assert_eq!(http.count(), 2, "这一枪确实发出去了");
    assert!(summary.contains("没报奖品名"), "实得：{summary}");
    assert!(
        !summary.contains("读数"),
        "抽成了那一档**不念读数**（那是抽之前的数）：{summary}"
    );
}

/// 抽奖那一枪**打挂了** ⇒ 失败（不是跳过）：那一枪已经出去了。
#[tokio::test]
async fn a_failed_draw_is_reported_as_failed() {
    let (out, http, _) = run_with(vec![
        Reply::Body(module_body(Some(1), Some(0), Some((START_MS, END_MS)))),
        Reply::Fail,
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Failed { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 2);
}

/// 抽奖那一枪回非 0 码 ⇒ 失败，且**不猜码值含义**（本口零样本）。
#[tokio::test]
async fn a_non_zero_lottery_code_is_a_failure() {
    let (out, _, _) = run_with(vec![
        Reply::Body(module_body(Some(1), Some(0), Some((START_MS, END_MS)))),
        Reply::Body(r#"{"code":500,"msg":"系统繁忙"}"#.to_string()),
    ])
    .await;
    let TaskOutcome::Failed { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("500"), "要把上游那句带出来：{reason}");
}

/// 读判据就挂了 ⇒ **交回 `Err`**（整趟没有结论可报）。
#[tokio::test]
async fn a_failed_criteria_query_is_an_error() {
    let (ctx, http, _) = ctx_with_at(vec![Reply::Fail], IN_WINDOW_MS);
    let err = FunaiTask
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("读判据失败要交回错误");
    assert!(err.to_string().contains("网络断了"), "实得：{err}");
    assert_eq!(http.count(), 1, "只发了那一枪");
}

// ══════════════════════════ 小结与明细 ══════════════════════════

/// "没抽"那一档**要念读数**，且缺的键念「缺」而**不念 0**。
///
/// 真账号跑一轮判成"没抽"时，界面上若只留一句"没有可用次数"，
/// 这一趟就答不出任何判据问题 —— 而下一批读数正是收口那些问题的唯一途径。
#[tokio::test]
async fn the_summary_of_a_skip_prints_the_readings() {
    let (out, _, _) = run_with(vec![Reply::Body(module_body(
        Some(0),
        Some(0),
        Some((START_MS, END_MS)),
    ))])
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("drawTimes=0"), "实得：{reason}");
    assert!(reason.contains("drawsUsed=0"), "实得：{reason}");
    assert!(
        reason.contains("todayGetLotteryCount=缺"),
        "缺的那格要念「缺」：{reason}"
    );
    assert!(
        !reason.contains("todayGetLotteryCount=0"),
        "缺不能念成 0（那是两件事）：{reason}"
    );
    // 两界真读到了才念
    assert!(
        reason.contains(&format!("startTime={START_MS}")),
        "实得：{reason}"
    );
}

/// 结论走明细通道，五格读数都在里面。
#[tokio::test]
async fn the_readings_are_reported_as_details() {
    let (out, _, sink) = run_with(vec![
        Reply::Body(module_body(Some(1), Some(0), Some((START_MS, END_MS)))),
        Reply::Body(lottery_body(
            serde_json::json!({"isWin":1,"prizeId":260107009,"prizeName":"30云朵"}),
        )),
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Success { .. }));

    let details = sink.details();
    assert_eq!(details.len(), 1, "这一条只自报一次");
    let d = &details[0];
    assert_eq!(d["kind"], "fun-ai");
    assert_eq!(d["verdict"], "done");
    assert_eq!(d["plan"], "draw");
    assert_eq!(d["moduleId"], "1-250730");
    assert_eq!(d["prizeName"], "30云朵");
    assert_eq!(d["readings"]["drawTimes"], 1);
    assert_eq!(d["readings"]["endTime"], END_MS);
}

// ══════════════════════════ 注册表接入 ══════════════════════════

/// ⭐ `fun-ai` 真的注册进了 registry —— CLI 与 `/api/capabilities` 都由它驱动。
#[test]
fn fun_ai_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let metas = registry.list();
    let meta = metas
        .iter()
        .find(|m| m.name == "fun-ai")
        .expect("fun-ai 必须注册（CLI/capabilities 都按 registry 走）");

    assert_eq!(meta.title, "趣玩AI抽奖");
    assert!(meta.params.is_empty(), "这一条没有参数");
    assert!(!meta.hidden, "它该出现在通用任务列表里");
}

struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("趣玩AI 这条链不该碰 store".into())
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
