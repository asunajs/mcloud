//! 红包派对任务层的测试。
//!
//! 钉六类东西（对应派活时点名的六条）：
//!
//! 1. 会话顺序：querySpecToken 必须在 tokenValidate 之前，换票那一枪的 token
//!    头是空串，之后每条业务请求都带换回来的新 token；
//! 2. 只读盘点路径：全员已完成 / 暂不可做时，一个写请求都不发；
//! 3. 写入判定：status=1 不发那一条写；读到没见过的码计入 Unknown 且不发；
//!    （缺 userStatus 按旧 TS 的 ?? 0 当未完成 —— 单独一条钉住这个选择）
//! 4. 空 token / 上游 status 非 200 ⇒ 报错，不静默；
//! 5. summary_line 的成功与失败两态；
//! 6. 取消：中途取消 ⇒ Error::Cancelled，且之后一枪都不发。
//!
//! 另外两条：同一个 taskCode 一次运行最多写一次；账号开关关掉 ⇒ 一枪都不发。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

// ══════════════════════ 假上游的固定答复 ══════════════════════

const SPEC_BODY: &str = r#"{"code":0,"msg":"成功","result":"SSO-1"}"#;
const VALIDATE_BODY: &str = r#"{"header":{"status":200},"data":{"token":"CPT"}}"#;
const BALANCE_BODY: &str =
    r#"{"header":{"status":200},"data":{"info":{"canAmount":1234,"totalAmount":5600}}}"#;
const TOAST_BODY: &str = r#"{"header":{"status":200},"data":{"lastUserPrize":{"prizeName":"1元话费","prizeAmount":100}}}"#;
const WRITE_OK: &str = r#"{"header":{"status":200},"data":{"status":1}}"#;
const EMPTY_OK: &str = r#"{"header":{"status":200},"data":{}}"#;

/// 全员非「未完成」：签到已完成、三条任务分别落在 1 / 3 / 4 ⇒ 一个写都不该发。
const TASKS_READ_ONLY: &str = r#"{
  "header": {"status": 200},
  "data": {
    "configTaskSignList": [{"isToday": 1, "status": 1, "signAmount": 30}],
    "configTaskNoviceList": [{"id": 11, "taskCode": "NOVICE_2", "taskName": "看视频", "userStatus": 1}],
    "configTaskDailyList": [{"id": 21, "taskCode": "DAILY_1", "taskName": "云机使用", "userStatus": 3}],
    "configTaskMonthlyList": [{"id": 31, "taskCode": "MONTHLY_4", "taskName": "体验一下", "userStatus": 4}]
  }
}"#;

/// 三条正好覆盖三种判定：status=1 不许写、status=99 归未知不许写、status=0 该写。
const TASKS_GATING: &str = r#"{
  "header": {"status": 200},
  "data": {
    "configTaskSignList": [{"isToday": 1, "status": 1}],
    "configTaskNoviceList": [{"id": 11, "taskCode": "NOVICE_2", "taskName": "看视频", "userStatus": 1}],
    "configTaskDailyList": [{"id": 21, "taskCode": "DAILY_9", "taskName": "没见过的码", "userStatus": 99}],
    "configTaskMonthlyList": [{"id": 31, "taskCode": "MONTHLY_4", "taskName": "直接完成", "userStatus": 0}]
  }
}"#;

/// 缺 userStatus 那一格（上游回了 null）—— 旧 TS 的 ?? 0 会当成未完成。
const TASKS_MISSING_STATUS: &str = r#"{
  "header": {"status": 200},
  "data": {
    "configTaskSignList": [{"isToday": 1, "status": 1}],
    "configTaskNoviceList": [],
    "configTaskDailyList": [],
    "configTaskMonthlyList": [{"id": 31, "taskCode": "MONTHLY_4", "taskName": "缺状态", "userStatus": null}]
  }
}"#;

/// 同一个 taskCode 列两遍（都是 status=0）—— 同一趟最多只该写一次。
const TASKS_DUPLICATE_CODE: &str = r#"{
  "header": {"status": 200},
  "data": {
    "configTaskSignList": [{"isToday": 1, "status": 1}],
    "configTaskNoviceList": [
      {"id": 11, "taskCode": "NOVICE_2", "taskName": "看视频", "userStatus": 0},
      {"id": 12, "taskCode": "NOVICE_2", "taskName": "看视频（重复项）", "userStatus": 0}
    ],
    "configTaskDailyList": [],
    "configTaskMonthlyList": []
  }
}"#;

// ══════════════════════ 假 HTTP 客户端（按路径分派） ══════════════════════

/// 一枪的答复。
#[derive(Clone)]
enum Reply {
    Body(String),
    /// 网络层打挂（不是业务码失败）。
    Fail,
}

type Handler = Box<dyn Fn(&HttpRequest) -> Reply + Send + Sync>;

struct Routed {
    seen: Mutex<Vec<HttpRequest>>,
    handler: Handler,
}

impl Routed {
    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }
}

#[async_trait]
impl HttpClient for Routed {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req.clone());
        match (self.handler)(&req) {
            Reply::Body(body) => Ok(HttpResponse {
                status: 200,
                body,
                headers: Vec::new(),
            }),
            Reply::Fail => Err(mcloud_core::Error::Other("网络断了".into())),
        }
    }
}

/// 按路径分派：除任务表外都用固定答复。
///
/// ⚠ 没列到的路径回一个合法空信封 —— 测试若指望某个端点有读数，
/// 会在断言处红，而不是在这里被静默吞掉。
fn route(req: &HttpRequest, tasks_body: &str) -> Reply {
    let url = &req.url;
    if url.contains("querySpecToken") {
        Reply::Body(SPEC_BODY.to_string())
    } else if url.ends_with("/user/tokenValidate") {
        Reply::Body(VALIDATE_BODY.to_string())
    } else if url.ends_with("/redpacket/configTaskLoginList") {
        Reply::Body(tasks_body.to_string())
    } else if url.ends_with("/redpacket/userAccountInfo") {
        Reply::Body(BALANCE_BODY.to_string())
    } else if url.ends_with("/redpacket/userToastInfo") {
        Reply::Body(TOAST_BODY.to_string())
    } else if url.ends_with("/redpacket/userCompleteTask") || url.ends_with("/redpacket/userSign") {
        Reply::Body(WRITE_OK.to_string())
    } else {
        Reply::Body(EMPTY_OK.to_string())
    }
}

fn handler_for(tasks_body: &'static str) -> Handler {
    Box::new(move |req| route(req, tasks_body))
}

// ══════════════════════ 假 ctx ══════════════════════

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
    fn details(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
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

fn account_with(settings: AccountSettings) -> Account {
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
        enabled: true,
        settings,
    }
}

fn ctx_with_at(
    handler: Handler,
    settings: AccountSettings,
    now_ms: i64,
    cancel: CancelToken,
) -> (TaskContext, Arc<Routed>, Arc<CollectSink>) {
    let http = Arc::new(Routed {
        seen: Mutex::new(Vec::new()),
        handler,
    });
    let sink = Arc::new(CollectSink::default());
    let ctx = TaskContext {
        account: account_with(settings),
        http: http.clone(),
        store: Arc::new(NoStore),
        clock: Arc::new(FixedClock(now_ms)),
        sink: sink.clone(),
        run_id: mcloud_core::RunId(1),
        cancel,
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

/// 零等待的任务：三处 sleep 全关，测试不会真等 15 秒。
fn fast_task() -> CloudPhoneRedpackTask {
    CloudPhoneRedpackTask {
        plan: RedpackPlan {
            browse_wait_ms: 0,
            topic_attempts: 3,
            topic_interval_ms: 0,
            install_poll_attempts: 3,
            install_poll_interval_ms: 0,
        },
    }
}

fn header(req: &HttpRequest, name: &str) -> Option<String> {
    req.headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn body_of(req: &HttpRequest) -> Value {
    serde_json::from_slice(req.body.as_deref().expect("这一族每条都带体")).unwrap()
}

fn count_path(reqs: &[HttpRequest], suffix: &str) -> usize {
    reqs.iter().filter(|req| req.url.ends_with(suffix)).count()
}

// ══════════════════════ 1. 会话顺序 ══════════════════════

#[tokio::test]
async fn session_orders_the_login_and_reuses_the_new_token() {
    let (ctx, http, _) = ctx_with_at(
        handler_for(TASKS_READ_ONLY),
        AccountSettings::default(),
        1_700_000_000_000,
        CancelToken::new(),
    );
    let out = fast_task()
        .run(&ctx, &json!({}))
        .await
        .expect("只读盘点不该 Err");
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");

    let reqs = http.requests();
    assert!(reqs.len() >= 4, "至少换票两枪 + 任务表 + 余额");
    assert!(
        reqs[0]
            .url
            .contains("/ycloud/api/cloud/userdomain/v2/querySpecToken"),
        "① 是 querySpecToken：{}",
        reqs[0].url
    );
    assert!(reqs[0].url.contains("targetSourceId=001216"));
    assert!(
        reqs[1].url.ends_with("/user/tokenValidate"),
        "② 是 tokenValidate"
    );

    // 换票那一枪：token 头是空串，SSO 票在 body 里。
    assert_eq!(header(&reqs[1], "token").as_deref(), Some(""));
    let body = body_of(&reqs[1]);
    assert_eq!(body["token"], json!("SSO-1"));
    assert_eq!(body["deviceId"], json!("DEV+1/base="));

    // ③ 之后每条业务请求都带换回来的新 token。
    for req in &reqs[2..] {
        assert_eq!(
            header(req, "token").as_deref(),
            Some("CPT"),
            "{} 没带新 token",
            req.url
        );
    }
}

// ══════════════════════ 2. 只读盘点路径 ══════════════════════

#[tokio::test]
async fn the_inventory_path_sends_no_write_request() {
    let (ctx, http, sink) = ctx_with_at(
        handler_for(TASKS_READ_ONLY),
        AccountSettings::default(),
        0,
        CancelToken::new(),
    );
    let out = fast_task().run(&ctx, &json!({})).await.expect("不该 Err");
    assert!(matches!(out, TaskOutcome::Success { .. }));

    for req in http.requests() {
        for forbidden in [
            "userSign",
            "userCompleteTask",
            "userBrowse",
            "userTopicAnswer",
            "apkInstall",
            "userInstallApp",
            "getRcsToken",
        ] {
            assert!(
                !req.url.contains(forbidden),
                "只读盘点不该出现写路径 {forbidden}：{}",
                req.url
            );
        }
    }

    let details = sink.details();
    assert_eq!(details.len(), 1, "只自报一次");
    let d = &details[0];
    assert_eq!(d["kind"], "cloud-phone-redpack");
    assert_eq!(d["verdict"], "done");
    assert_eq!(d["counts"]["total"], 3);
    assert_eq!(d["counts"]["done"], 1);
    assert_eq!(d["counts"]["blocked"], 2);
    assert_eq!(d["counts"]["todo"], 0);
    assert_eq!(d["counts"]["unknown"], 0);
    assert_eq!(d["writes"].as_array().unwrap().len(), 0);
    assert_eq!(d["balanceBefore"]["canAmount"], 1234);
}

// ══════════════════════ 3. 写入判定 ══════════════════════

#[tokio::test]
async fn status_one_and_unknown_codes_are_never_written() {
    let (ctx, http, sink) = ctx_with_at(
        handler_for(TASKS_GATING),
        AccountSettings::default(),
        0,
        CancelToken::new(),
    );
    let out = fast_task().run(&ctx, &json!({})).await.expect("不该 Err");
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");

    let reqs = http.requests();
    assert_eq!(
        count_path(&reqs, "/redpacket/userCompleteTask"),
        1,
        "只有 status=0 的那条该写"
    );
    let completes: Vec<&HttpRequest> = reqs
        .iter()
        .filter(|req| req.url.ends_with("/redpacket/userCompleteTask"))
        .collect();
    assert_eq!(body_of(completes[0])["taskId"], json!(31));
    assert_eq!(
        count_path(&reqs, "/redpacket/userBrowse"),
        0,
        "status=1 的浏览任务不该被写"
    );

    let details = sink.details();
    let d = &details[0];
    assert_eq!(d["counts"]["todo"], 1);
    assert_eq!(d["counts"]["done"], 1, "status=1 那一条算已完成");
    assert_eq!(d["counts"]["unknown"], 1, "没见过的码单独一档");
    assert_eq!(d["failures"].as_array().unwrap().len(), 0);

    let tasks = d["tasks"].as_array().unwrap();
    let unknown = tasks
        .iter()
        .find(|task| task["taskCode"] == "DAILY_9")
        .expect("DAILY_9 必须在明细里");
    assert_eq!(unknown["state"], "unknown");
    assert_eq!(unknown["handled"], false);
    let done = tasks
        .iter()
        .find(|task| task["taskCode"] == "NOVICE_2")
        .expect("NOVICE_2 必须在明细里");
    assert_eq!(done["state"], "done");
    assert_eq!(done["handled"], false);
}

/// ⚠ 这条钉的是一个**刻意选择**：缺 userStatus 按旧 TS 的 Number(userStatus ?? 0)
/// 当未完成（会去写），与"读到没见过的码 ⇒ 不写"不是一回事。
#[tokio::test]
async fn a_missing_user_status_is_treated_as_todo_like_the_old_ts() {
    let (ctx, http, sink) = ctx_with_at(
        handler_for(TASKS_MISSING_STATUS),
        AccountSettings::default(),
        0,
        CancelToken::new(),
    );
    let _ = fast_task().run(&ctx, &json!({})).await.expect("不该 Err");

    assert_eq!(
        count_path(&http.requests(), "/redpacket/userCompleteTask"),
        1,
        "缺格 ⇒ 旧 TS 的 ?? 0 ⇒ 未完成 ⇒ 去写"
    );
    let d = &sink.details()[0];
    assert_eq!(d["counts"]["todo"], 1);
    assert_eq!(d["counts"]["unknown"], 0);
    assert_eq!(d["tasks"][0]["state"], "todo");
    assert_eq!(d["tasks"][0]["userStatus"], Value::Null);
}

/// 同一个 taskCode 列两遍 ⇒ 一趟最多写一次（同一趟的第二条只记一句"跳过"）。
#[tokio::test]
async fn the_same_task_code_is_written_at_most_once_per_run() {
    let (ctx, http, sink) = ctx_with_at(
        handler_for(TASKS_DUPLICATE_CODE),
        AccountSettings::default(),
        0,
        CancelToken::new(),
    );
    let _ = fast_task().run(&ctx, &json!({})).await.expect("不该 Err");

    let reqs = http.requests();
    assert_eq!(
        count_path(&reqs, "/redpacket/userCompleteTask"),
        1,
        "两遍同码只写一次"
    );
    assert_eq!(count_path(&reqs, "/redpacket/userBrowse"), 1);
    let d = &sink.details()[0];
    let tasks = d["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(
        tasks[1]["note"].as_str().unwrap().contains("跳过"),
        "{tasks:?}"
    );
}

// ══════════════════════ 4. 报错不静默 ══════════════════════

#[tokio::test]
async fn an_empty_token_fails_loudly_and_stops() {
    let handler: Handler = Box::new(|req| {
        if req.url.ends_with("/user/tokenValidate") {
            Reply::Body(r#"{"header":{"status":200},"data":{"token":""}}"#.to_string())
        } else {
            route(req, TASKS_READ_ONLY)
        }
    });
    let (ctx, http, sink) = ctx_with_at(handler, AccountSettings::default(), 0, CancelToken::new());
    let err = fast_task()
        .run(&ctx, &json!({}))
        .await
        .expect_err("空 token 必须报错，不能静默");
    assert!(err.to_string().contains("token"), "实得：{err}");
    assert_eq!(http.count(), 2, "换票失败 ⇒ 业务请求一条都不发");
    assert_eq!(sink.details()[0]["verdict"], "failed");
}

#[tokio::test]
async fn a_non_200_login_status_fails_loudly() {
    let handler: Handler = Box::new(|req| {
        if req.url.ends_with("/user/tokenValidate") {
            Reply::Body(r#"{"header":{"status":401,"errMsg":"票过期"}}"#.to_string())
        } else {
            route(req, TASKS_READ_ONLY)
        }
    });
    let (ctx, http, _) = ctx_with_at(handler, AccountSettings::default(), 0, CancelToken::new());
    let err = fast_task()
        .run(&ctx, &json!({}))
        .await
        .expect_err("非 200 必须报错");
    let text = err.to_string();
    assert!(text.contains("401"), "实得：{text}");
    assert!(text.contains("票过期"), "实得：{text}");
    assert_eq!(http.count(), 2);
}

#[tokio::test]
async fn a_non_200_task_list_is_fatal() {
    let handler: Handler = Box::new(|req| {
        if req.url.ends_with("/redpacket/configTaskLoginList") {
            Reply::Body(r#"{"header":{"status":500,"errMsg":"系统繁忙"}}"#.to_string())
        } else {
            route(req, TASKS_READ_ONLY)
        }
    });
    let (ctx, http, sink) = ctx_with_at(handler, AccountSettings::default(), 0, CancelToken::new());
    let err = fast_task()
        .run(&ctx, &json!({}))
        .await
        .expect_err("任务表是致命读");
    let text = err.to_string();
    assert!(text.contains("500"), "实得：{text}");
    assert!(text.contains("系统繁忙"), "实得：{text}");
    assert_eq!(http.count(), 3, "任务表挂了就不该再往后走");
    assert_eq!(sink.details()[0]["verdict"], "failed");
}

#[tokio::test]
async fn a_network_error_on_the_task_list_is_fatal() {
    let handler: Handler = Box::new(|req| {
        if req.url.ends_with("/redpacket/configTaskLoginList") {
            Reply::Fail
        } else {
            route(req, TASKS_READ_ONLY)
        }
    });
    let (ctx, _, _) = ctx_with_at(handler, AccountSettings::default(), 0, CancelToken::new());
    let err = fast_task()
        .run(&ctx, &json!({}))
        .await
        .expect_err("网络挂也算失败");
    assert!(err.to_string().contains("网络断了"), "实得：{err}");
}

// ══════════════════════ 5. summary_line ══════════════════════

fn entry(code: &str, state: RedpackTaskState) -> RedpackTaskEntry {
    RedpackTaskEntry {
        task_code: code.to_string(),
        state,
        ..Default::default()
    }
}

#[test]
fn summary_line_spells_out_the_three_counts() {
    let board = RedpackBoard {
        tasks: vec![
            entry("A", RedpackTaskState::Todo),
            entry("B", RedpackTaskState::Done),
            entry("C", RedpackTaskState::Unknown),
        ],
        ..Default::default()
    };
    let result = RedpackResult {
        verdict: RedpackVerdict::Done,
        board,
        error: String::new(),
    };
    let line = result.summary_line();
    assert!(line.contains("未完成 1"), "实得：{line}");
    assert!(line.contains("已完成 1"), "实得：{line}");
    assert!(line.contains("未知 1"), "实得：{line}");
    assert!(matches!(result.task_outcome(), TaskOutcome::Success { .. }));
}

#[test]
fn summary_line_of_a_failure_says_which_step_failed() {
    let result = RedpackResult {
        verdict: RedpackVerdict::Failed,
        board: RedpackBoard::default(),
        error: "MONTHLY_4：userCompleteTask 没成功（上游错误 [500]：系统繁忙）".to_string(),
    };
    let line = result.summary_line();
    assert!(line.contains("失败"), "实得：{line}");
    assert!(line.contains("MONTHLY_4"), "实得：{line}");
    assert!(line.contains("未完成 0"), "失败也要带上计数：{line}");
    assert!(matches!(result.task_outcome(), TaskOutcome::Failed { .. }));
}

#[test]
fn the_zero_verdict_is_failed_not_done() {
    // 零值结果绝不能被念成"一切正常"。
    assert_eq!(RedpackVerdict::default(), RedpackVerdict::Failed);
    let zero = RedpackResult::default().summary_line();
    assert!(zero.contains("失败"), "实得：{zero}");
    assert!(matches!(
        RedpackResult::default().task_outcome(),
        TaskOutcome::Failed { .. }
    ));
}

// ══════════════════════ 6. 取消 ══════════════════════

#[tokio::test]
async fn cancelling_mid_run_stops_before_the_next_request() {
    let cancel = CancelToken::new();
    let trip = cancel.clone();
    // 第 ① 枪就把令牌打掉 ⇒ 换票两枪会走完，任务表那条不该发。
    let handler: Handler = Box::new(move |req| {
        if req.url.contains("querySpecToken") {
            trip.cancel();
        }
        route(req, TASKS_READ_ONLY)
    });
    let (ctx, http, _) = ctx_with_at(handler, AccountSettings::default(), 0, cancel);
    let err = fast_task()
        .run(&ctx, &json!({}))
        .await
        .expect_err("中途取消必须 Err(Cancelled)");
    assert!(err.is_cancelled(), "实得：{err:?}");
    assert_eq!(http.count(), 2, "取消后一枪都不该再发");
    for req in http.requests() {
        assert!(
            !req.url.contains("configTaskLoginList"),
            "取消点之后不许有业务请求：{}",
            req.url
        );
    }
}

// ══════════════════════ 开关 ══════════════════════

#[tokio::test]
async fn the_redpack_switch_off_skips_without_a_single_request() {
    let settings = AccountSettings {
        redpack_enabled: false,
        ..Default::default()
    };
    let (ctx, http, sink) = ctx_with_at(
        handler_for(TASKS_READ_ONLY),
        settings,
        0,
        CancelToken::new(),
    );
    let out = fast_task()
        .run(&ctx, &json!({}))
        .await
        .expect("跳过不是 Err");
    assert!(matches!(out, TaskOutcome::Skipped { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 0, "关掉就一枪都不发");
    assert_eq!(sink.details()[0]["verdict"], "skipped");
}

// ══════════════════════ 注册表接入 ══════════════════════

#[test]
fn redpack_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let metas = registry.list();
    let meta = metas
        .iter()
        .find(|meta| meta.name == "cloud-phone-redpack")
        .expect("cloud-phone-redpack 必须注册（CLI/capabilities 都按 registry 走）");
    assert_eq!(meta.title, "红包派对");
    assert!(meta.params.is_empty(), "这一条没有参数");
    assert!(!meta.hidden);
    // 这一格是**全局哨兵**：加任务时它会红，提醒你"注册表条数变了"。
    // 别把它删掉换成"包含某条就行" —— 那正是它要防的（少一条没人发现）。
    //
    // 20 - 1：`exchange` 已从 `register_all` 摘掉
    //（缺 deviceId 来源 / visitlog 预热 / 等待档对齐，见 `lib.rs` 那段说明）。
    assert_eq!(
        metas.len(),
        19,
        "原 18 条 + 红包派对 + ai-avatar = 20 条，摘掉 exchange 后 19 条"
    );
}

// ══════════════════════ 这条链不碰 store ══════════════════════

/// 一碰 store 就报错的替身。
struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("红包派对这条链不该碰 store".into())
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
    async fn account_state(&self, _id: &mcloud_core::AccountId) -> Result<Value> {
        Err(refuse())
    }
    async fn save_account_state(&self, _id: &mcloud_core::AccountId, _s: &Value) -> Result<()> {
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
