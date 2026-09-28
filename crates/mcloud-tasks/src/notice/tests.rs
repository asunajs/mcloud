//! 公告任务层的测试。
//!
//! 这一组钉三类东西：
//!
//! * **判定**（`plan_notice` / `notice_slot_claimable`）—— 纯函数，把每档的
//!   边界都摆出来，尤其是"字段缺失"与"给了 0"**不是一回事**；
//! * **写请求发不发**（`http.count()`）—— 这一条链最贵的错误是
//!   "在判据没过的时候把 `reward` 发出去"：那一枪会真的记账，幂等与否**零样本**。
//!   所以每个"不该发"的档都要断言请求数**恰好 1**；
//! * **「已领 + 复查挂了」不许写成失败** —— 它是这一条链与 `msg-push` 最不同的一档：
//!   报了失败，用户就会再去点一次。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use mcloud_core::testkit::FixedClock;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, DeviceId, EventSink, HttpClient, HttpResponse,
    RunEvent,
};

// ══════════════════════════ 夹具 ══════════════════════════

/// 实拍那份 `getTaskInfo`（`开启通知领云朵.json` 第 3 / 6 条，两遍逐字相同）。
///
/// ⚠ 它判成 [`NoticePlan::Reward`]（槽二 `cloudStatus=1` 且 `taskStatus=3`），
/// 而实拍里客户端**紧接着就发了 `reward`** —— 不过那个对应关系是**猜的**，
/// 见 `plan_notice` 那一大段。
const CAPTURED_INFO: &str = r#"{"code":0,"msg":"success","result":{
    "taskOneInfo":{"cloudStatus":2,"sum":200,"taskStatus":3},
    "taskTwoInfo":{"cloudStatus":1,"progress":31,"sum":100,"taskStatus":3}}}"#;

/// 实拍那条 `reward`：`{"code":0,"msg":"success"}` —— **连 `result` 都没有**。
const CAPTURED_REWARD: &str = r#"{"code":0,"msg":"success"}"#;

/// 造一个槽的 JSON。两个参数各是 `None` = **该键不出现**（与"给了 0"不是一回事）。
fn slot_json_with(cloud: Option<i64>, task: Option<i64>) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(v) = cloud {
        m.insert("cloudStatus".into(), serde_json::json!(v));
    }
    if let Some(v) = task {
        m.insert("taskStatus".into(), serde_json::json!(v));
    }
    serde_json::Value::Object(m)
}

/// 造一份 `getTaskInfo`。`None` = **该槽整槽不出现**。
fn info_body(one: Option<serde_json::Value>, two: Option<serde_json::Value>) -> String {
    let mut result = serde_json::Map::new();
    if let Some(v) = one {
        result.insert("taskOneInfo".into(), v);
    }
    if let Some(v) = two {
        result.insert("taskTwoInfo".into(), v);
    }
    serde_json::json!({"code":0,"msg":"success","result":result}).to_string()
}

/// 一枪的答复：给个 body，或者**打挂**（传输层错误）。
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

    /// 取回所有 `Warn` 级日志的正文（复查挂那一档要在这里留痕）。
    fn warns(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                RunEvent::Log { level, message, .. } if *level == mcloud_core::LogLevel::Warn => {
                    Some(message.clone())
                }
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
        // ⚠ 固定时钟用 `mcloud_core::testkit` 那一份，不自己再写一个
        // （`docs/conventions.md` §8.7：各写一份迟早行为不一致）。
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
    let out = NoticeTask.run(&ctx, &serde_json::json!({})).await;
    (out.expect("这一档不该是 Err"), http, sink)
}

/// 直接拼一个槽（纯函数那几条用）。
fn slot_of(cloud: Option<i64>, task: Option<i64>) -> NoticeTaskSlot {
    NoticeTaskSlot {
        cloud_status: cloud,
        task_status: task,
        ..Default::default()
    }
}

/// 直接拼一份 `getTaskInfo`（纯函数那几条用）。
fn info(one: Option<NoticeTaskSlot>, two: Option<NoticeTaskSlot>) -> NoticeTaskInfo {
    NoticeTaskInfo {
        task_one_info: one,
        task_two_info: two,
    }
}

// ══════════════════════════ 判定（纯函数） ══════════════════════════

/// ⚠ 这是**猜的**映射：`cloudStatus == 1` 读成"未领"、`taskStatus == 3` 读成"已完成"。
///
/// 用例把它钉住，是为了"哪天补上正面样本要改它"时能一眼看见改动面 ——
/// **不是**说它被证实了。
#[test]
fn cloud_one_with_task_three_is_read_as_claimable() {
    assert!(notice_slot_claimable(Some(&slot_of(
        Some(NOTICE_CLOUD_STATUS_UNCLAIMED),
        Some(NOTICE_TASK_STATUS_DONE)
    ))));
    assert_eq!(
        plan_notice(&info(Some(slot_of(Some(1), Some(3))), None)),
        NoticePlan::Reward
    );
}

/// 实拍那两条读数（槽二 `cloudStatus=1` / `taskStatus=3`）判成**该领** ——
/// 与实拍里"紧接着就发了 `reward`"一致。⚠ 方向仍是猜的。
#[test]
fn the_captured_pair_is_read_as_claimable() {
    let env = SignInEnvelope::parse(CAPTURED_INFO).unwrap();
    let parsed = proto::parse_notice_task_info(&env).unwrap();
    assert_eq!(plan_notice(&parsed), NoticePlan::Reward);
}

/// `cloudStatus == 2`（读作"已入账"）不是可领的那一格 ⇒ 没得领。
#[test]
fn cloud_two_is_not_claimable() {
    let both_two = info(
        Some(slot_of(Some(NOTICE_CLOUD_STATUS_CLAIMED), Some(3))),
        Some(slot_of(Some(NOTICE_CLOUD_STATUS_CLAIMED), Some(3))),
    );
    assert_eq!(plan_notice(&both_two), NoticePlan::Nothing);
}

/// ⚠ 真账号读到过 `cloudStatus = 0`：它**既不是 1 也不是 2**。
///
/// 判定照旧落 [`NoticePlan::Nothing`]（一枪不发），**但这一档不是"已入账"** ——
/// 小结里那句措辞因此不能写成"都读成已入账"（见 [`NoticeResult::readings`]）。
#[test]
fn a_zero_cloud_status_lands_on_nothing_not_on_claimed() {
    let zero = info(
        Some(slot_of(Some(0), Some(0))),
        Some(slot_of(Some(0), Some(1))),
    );
    assert_eq!(plan_notice(&zero), NoticePlan::Nothing);
}

/// 判据字段**缺任何一个** ⇒ 这一槽不参与判断；两槽都判不了 ⇒ `Unknown`。
///
/// ⚠ 这就是"`None` 与 `Some(0)` 不是一回事"那一条：把"上游没说"当成
/// "说了没完成"会让这条链**静默漏领**。
#[test]
fn a_slot_missing_a_criterion_is_not_judged_so_unknown() {
    let only_status = info(Some(slot_of(None, Some(3))), None);
    assert!(!notice_slot_judged(Some(&slot_of(None, Some(3)))));
    assert!(!notice_slot_claimable(Some(&slot_of(None, Some(3)))));
    assert_eq!(plan_notice(&only_status), NoticePlan::Unknown);

    let only_cloud = info(Some(slot_of(Some(1), None)), None);
    assert_eq!(plan_notice(&only_cloud), NoticePlan::Unknown);

    // 空槽（给了但字段全缺）同样不可判 —— 它**不是** `None`，但这一格上等价。
    let empty = info(Some(NoticeTaskSlot::default()), None);
    assert_eq!(plan_notice(&empty), NoticePlan::Unknown);
}

/// 两槽一个都没给 ⇒ `Unknown`，**不是** `Nothing`。
///
/// 落 `Nothing` 会让小结说成"现在没得领"，而那是一句**没根据的肯定话**。
#[test]
fn no_slots_at_all_is_unknown_not_nothing() {
    assert_eq!(plan_notice(&info(None, None)), NoticePlan::Unknown);
}

/// 两个槽各给一半判据 ⇒ 一个可判、一个不可判 ⇒ `Nothing`（能判的那个说"不可领"）。
#[test]
fn one_judged_slot_is_enough_to_conclude_nothing() {
    let mixed = info(
        Some(slot_of(Some(2), Some(3))),
        Some(slot_of(None, Some(3))),
    );
    assert_eq!(plan_notice(&mixed), NoticePlan::Nothing);
}

/// ⚠⚠ **`Manual` 那一档今天不可达** —— 判据恒 `false`。
///
/// 这是**就地声明**（造不出反例的分支不许当作"没红过所以不重要"）：
/// "通知未开启"态的响应**零样本**，见 `notice_needs_manual` 头注。
/// 哪天它变成可达，这条用例会红 —— 那是**信号**（该补这一档的映射测试了）。
#[test]
fn the_manual_tier_is_unreachable_today() {
    let samples = [
        info(None, None),
        info(Some(slot_of(Some(0), Some(0))), None),
        info(
            Some(slot_of(Some(1), Some(3))),
            Some(slot_of(Some(2), Some(3))),
        ),
        info(
            Some(slot_of(Some(2), Some(3))),
            Some(slot_of(Some(2), Some(3))),
        ),
    ];
    for sample in &samples {
        assert!(!notice_needs_manual(sample), "这一档今天不该可达");
        assert_ne!(plan_notice(sample), NoticePlan::Manual);
    }
}

// ══════════════════════════ 不该发的档：请求数必须恰好 1 ══════════════════════════

/// `Nothing` ⇒ 跳过，且**只发了一枪**（查状态）。
#[tokio::test]
async fn nothing_sends_no_write_request() {
    let body = info_body(Some(slot_json_with(Some(0), Some(0))), None);
    let (out, http, _) = run_with(vec![Reply::Body(body)]).await;
    assert!(matches!(out, TaskOutcome::Skipped { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 1, "判据没过就不该发第二枪");
    assert!(http.url_at(0).ends_with("/getTaskInfo"));
}

/// `Unknown` ⇒ 跳过（**不是失败**），不发写请求。
#[tokio::test]
async fn unknown_sends_no_write_request() {
    let (out, http, _) = run_with(vec![Reply::Body(info_body(None, None))]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 1);
    assert!(reason.contains("cloudStatus"), "要说清缺哪两个键：{reason}");
    assert!(reason.contains("没发请求"), "实得：{reason}");
}

// ══════════════════════════ 该发的档：写请求的形状与成败 ══════════════════════════

/// 实拍那条读数 ⇒ 三枪：查 → `reward` → **复查**。
///
/// ⚠ 复查那一枪是这一条与 `msg-push` 最不同的地方：`reward` 报不出到账数，
/// 复查是唯一的到账读数口。
#[tokio::test]
async fn the_captured_body_triggers_the_reward_and_a_recheck() {
    let (out, http, _) = run_with(vec![
        Reply::Body(CAPTURED_INFO.to_string()),
        Reply::Body(CAPTURED_REWARD.to_string()),
        Reply::Body(CAPTURED_INFO.to_string()),
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 3, "查 → 领 → 复查");
    assert!(http.url_at(0).ends_with("/getTaskInfo"));
    assert!(
        http.url_at(1).ends_with("/openEmailsms/reward"),
        "实得：{}",
        http.url_at(1)
    );
    assert!(http.url_at(2).ends_with("/getTaskInfo"), "第三枪是复查");
}

/// **该发**的那一枪是 GET 且不带 body（实拍逐字如此，尽管它会记账）。
#[tokio::test]
async fn the_reward_shot_is_a_bare_get() {
    let (_, http, _) = run_with(vec![
        Reply::Body(CAPTURED_INFO.to_string()),
        Reply::Body(CAPTURED_REWARD.to_string()),
        Reply::Body(CAPTURED_INFO.to_string()),
    ])
    .await;
    let reward = &http.requests()[1];
    assert_eq!(reward.method, "GET");
    assert!(reward.body.is_none(), "这一枪不带 body");
}

/// `reward` 打挂了 ⇒ **失败**（不是跳过）：那一枪已经出去了。
#[tokio::test]
async fn a_failed_reward_is_reported_as_failed_not_skipped() {
    let (out, http, _) = run_with(vec![Reply::Body(CAPTURED_INFO.to_string()), Reply::Fail]).await;
    assert!(matches!(out, TaskOutcome::Failed { .. }), "实得：{out:?}");
    assert_eq!(
        http.count(),
        2,
        "写请求发过了，这一趟的实况是「发过、没成」"
    );
    assert!(
        !http.url_at(1).contains("getTaskInfo"),
        "失败之后不该有复查那一枪"
    );
}

/// 上游对那一枪回非 0 ⇒ 失败（写请求发出去了）。
#[tokio::test]
async fn a_non_zero_reward_code_is_a_failure() {
    let (out, http, _) = run_with(vec![
        Reply::Body(CAPTURED_INFO.to_string()),
        Reply::Body(r#"{"code":500,"msg":"系统繁忙"}"#.to_string()),
    ])
    .await;
    let TaskOutcome::Failed { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("500"), "要把上游那个码念出来：{reason}");
    assert_eq!(http.count(), 2);
}

/// 查判据就挂了 ⇒ **交回 `Err`**（整趟没有结论可报）。
#[tokio::test]
async fn a_failed_before_query_is_an_error() {
    let (ctx, http, _) = ctx_with(vec![Reply::Fail]);
    let err = NoticeTask
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("查判据失败要交回错误");
    assert!(err.to_string().contains("网络断了"), "实得：{err}");
    assert_eq!(http.count(), 1, "只发了那一枪");
}

// ══════════════════════════ ⭐ 「已领 + 复查挂了」 ══════════════════════════

/// ⚠⚠ **领成功、复查打挂 ⇒ 不是失败**。
///
/// 写成失败会让用户以为没领上**再去点一次**，而 `reward` 幂等与否**零样本**。
/// 本仓 `TaskResult` 只有一个槽装不下"结论 + 错误"那一对 ⇒ 错误进
/// 小结 + 明细 + 一条 warn 日志，档位照"已领"报（见文件头"结构差之二"）。
#[tokio::test]
async fn a_claimed_reward_with_a_failed_recheck_is_not_a_failure() {
    let (out, http, sink) = run_with(vec![
        Reply::Body(CAPTURED_INFO.to_string()),
        Reply::Body(CAPTURED_REWARD.to_string()),
        Reply::Fail,
    ])
    .await;

    let TaskOutcome::Success { summary } = &out else {
        panic!("领成了就**不许**报失败/跳过（那会让用户再点一次），实得：{out:?}");
    };
    assert!(summary.contains("已领"), "实得：{summary}");
    assert!(summary.contains("复查没跑成"), "实得：{summary}");
    assert!(
        summary.contains("网络断了"),
        "那条错误要跟结论一起交回，实得：{summary}"
    );
    assert!(
        !summary.contains("无槽"),
        "这一档刻意不念两个槽（会把「没读到」与「本来就该没有」混起来）：{summary}"
    );
    assert_eq!(http.count(), 3, "复查那一枪确实发出去了");

    // 错误同时进明细，界面/脚本按字段就能看见"这一趟没走完"。
    let details = sink.details();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["recheckFailed"], true);
    assert_eq!(details[0]["verdict"], "done");
    assert!(
        details[0]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("网络断了"),
        "实得：{}",
        details[0]
    );
    assert_eq!(details[0]["recheck"], serde_json::Value::Null);

    // 而且它留一条 warn 日志（失败会报红，这一档不该报红）。
    assert_eq!(sink.warns().len(), 1, "实得：{:?}", sink.warns());
    assert!(sink.warns()[0].contains("复查"));
}

// ══════════════════════════ 小结与明细 ══════════════════════════

/// ⚠⚠ **只认复查那份读数，不许回落到 `before`**。
///
/// 拿"领之前"那份去填"复查之后"那两个字段，表现是小结念出
/// "复查：cloudStatus=1"，而它其实是领之前的数 —— 那正是复查要看的东西，
/// 念错了比念不出来更糟（会把"到底变没变"这个唯一的问题答成假的）。
#[tokio::test]
async fn the_summary_uses_the_recheck_readings_never_the_before_ones() {
    let before = CAPTURED_INFO.to_string();
    // 复查那份：槽二从 1 变成 0（一个"既不是 1 也不是 2"的读数）。
    let after = info_body(
        Some(slot_json_with(Some(2), Some(3))),
        Some(slot_json_with(Some(0), Some(3))),
    );
    let (out, _, _) = run_with(vec![
        Reply::Body(before),
        Reply::Body(CAPTURED_REWARD.to_string()),
        Reply::Body(after),
    ])
    .await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(summary.contains("复查："), "实得：{summary}");
    assert!(
        summary.contains("cloudStatus=0"),
        "要念复查那份，实得：{summary}"
    );
    assert!(
        !summary.contains("cloudStatus=1"),
        "领之前那份（1）**不许**出现在复查那句话里，实得：{summary}"
    );
}

/// 小结里**照抄**四个读数，缺的键念「缺」而**不念 0**。
#[test]
fn the_summary_prints_absent_fields_as_absent() {
    let result = NoticeResult {
        verdict: NoticeVerdict::Nothing,
        task_one: Some(slot_of(Some(0), Some(0))),
        reason: "能判的槽里没有一个读成可领".into(),
        ..Default::default()
    };
    let text = result.summary_line();
    assert!(text.contains("cloudStatus=0"), "实得：{text}");
    assert!(text.contains("sum=缺"), "实得：{text}");
    assert!(text.contains("progress=缺"), "实得：{text}");
    assert!(
        !text.contains("progress=0"),
        "缺不能念成 0（那是两件事）：{text}"
    );
}

/// 读数**不翻成中文** —— 翻就等于把那份未验证的映射写成结论。
#[test]
fn the_summary_copies_readings_verbatim_without_translating() {
    let result = NoticeResult {
        verdict: NoticeVerdict::Nothing,
        task_one: Some(slot_of(Some(0), Some(0))),
        reason: "能判的槽里没有一个读成可领".into(),
        ..Default::default()
    };
    let text = result.summary_line();
    for forbidden in ["已入账", "未领", "已领", "已完成"] {
        assert!(
            !text.contains(forbidden),
            "别把读数翻成中文（{forbidden}）：{text}"
        );
    }
}

/// `Nothing` 那句理由**不许**写成"能判的槽都读成已入账"（参考实现踩过这一格）。
#[tokio::test]
async fn nothing_says_no_judged_slot_is_claimable() {
    let body = info_body(Some(slot_json_with(Some(0), Some(0))), None);
    let (out, _, _) = run_with(vec![Reply::Body(body)]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("没有一个读成可领"), "实得：{reason}");
    assert!(
        !reason.contains("已入账"),
        "`cloudStatus=0` 既不是可领也不是已入账：{reason}"
    );
}

/// 两槽都没读到 ⇒ 小结里**不出现**那句小括号（"无槽，无槽"占掉半句没有信息量的话）。
#[tokio::test]
async fn no_readings_means_no_parenthesis() {
    let (out, _, _) = run_with(vec![Reply::Body(info_body(None, None))]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(!reason.contains("读数"), "实得：{reason}");
    assert!(!reason.contains("无槽"), "实得：{reason}");
}

/// 结论走明细通道：档位 + 两份读数（`before` 与 `recheck` **分开**）。
#[tokio::test]
async fn the_two_readings_are_reported_as_details() {
    let after = CAPTURED_INFO.to_string();
    let (out, _, sink) = run_with(vec![
        Reply::Body(CAPTURED_INFO.to_string()),
        Reply::Body(CAPTURED_REWARD.to_string()),
        Reply::Body(after),
    ])
    .await;
    assert!(matches!(out, TaskOutcome::Success { .. }));

    let details = sink.details();
    assert_eq!(details.len(), 1, "这一条只自报一次");
    let d = &details[0];
    assert_eq!(d["kind"], "notice");
    assert_eq!(d["verdict"], "done");
    assert_eq!(d["recheckFailed"], false);
    // `reward` 实拍恒无 `result` ⇒ `gained` 是 null，**不是 0**。
    assert_eq!(d["gained"], serde_json::Value::Null);
    // 两份读数都在，键名照抄上游。
    assert_eq!(d["before"]["taskTwoInfo"]["cloudStatus"], 1);
    assert_eq!(d["recheck"]["taskTwoInfo"]["cloudStatus"], 1);
    assert_eq!(d["before"]["taskOneInfo"]["sum"], 200);
    assert_eq!(d["before"]["taskTwoInfo"]["progress"], 31);
}

/// 槽"没给"念成 `null`，"给了但字段全缺"念成四个 `null` 的**对象**。
#[tokio::test]
async fn a_missing_slot_is_null_while_an_empty_slot_is_an_object() {
    let body = info_body(Some(serde_json::json!({})), None);
    let (_, _, sink) = run_with(vec![Reply::Body(body)]).await;
    let d = &sink.details()[0];
    assert!(d["before"]["taskOneInfo"].is_object(), "空槽仍是对象");
    assert_eq!(
        d["before"]["taskOneInfo"]["cloudStatus"],
        serde_json::Value::Null
    );
    assert_eq!(d["before"]["taskTwoInfo"], serde_json::Value::Null);
}

// ══════════════════════════ 档位映射 ══════════════════════════

/// 五档到 [`TaskOutcome`] 的映射。
#[test]
fn verdicts_map_to_outcomes() {
    let of = |verdict| NoticeResult {
        verdict,
        ..Default::default()
    };
    assert!(matches!(
        of(NoticeVerdict::Done).task_outcome(),
        TaskOutcome::Success { .. }
    ));
    for verdict in [
        NoticeVerdict::Nothing,
        NoticeVerdict::Manual,
        NoticeVerdict::Unknown,
    ] {
        assert!(
            matches!(of(verdict).task_outcome(), TaskOutcome::Skipped { .. }),
            "{verdict:?} 该落 Skipped"
        );
    }
    assert!(matches!(
        of(NoticeVerdict::Failed).task_outcome(),
        TaskOutcome::Failed { .. }
    ));
}

/// ⚠ [`NoticeVerdict::Manual`] 落 [`TaskOutcome::Skipped`]，而那句"要人去 App 里
/// 开通知"的说明**逐字在小结里**。
///
/// # 这是**已知的结构差**，不是等价物
///
/// 参考实现把这一档记成 `OutcomeManualRequired`（界面/推送能看见"要做什么"），
/// 而本仓 [`TaskOutcome`] 没有那一档，照 `exchange` 的既有处置：说明进小结、
/// 档位落 `Skipped`。等那一档补上，这一支改挂它、这句就是现成的 `instruction`。
///
/// ⚠ 用例钉的是**映射**，不是"这一档会被走到"—— 它今天不可达
/// （见 [`the_manual_tier_is_unreachable_today`]）。
#[test]
fn the_manual_tier_lands_on_skipped_and_carries_the_instruction() {
    let result = NoticeResult {
        verdict: NoticeVerdict::Manual,
        task_one: Some(slot_of(Some(2), Some(3))),
        reason: "有槽读成通知未开启".into(),
        ..Default::default()
    };
    let TaskOutcome::Skipped { reason } = result.task_outcome() else {
        panic!("这一档该落 Skipped");
    };
    for needle in ["App", "邮箱通知", "短信通知", "代劳不了"] {
        assert!(
            reason.contains(needle),
            "说明要逐字在小结里（{needle}）：{reason}"
        );
    }
    assert_eq!(reason, result.summary_line(), "两处念的是同一句话");
}

/// 判据哪一档失败要说清"缺的是哪两个键"，而不是只说"没得领"。
#[test]
fn the_unknown_reason_names_the_missing_criteria() {
    let result = NoticeResult {
        verdict: NoticeVerdict::Unknown,
        reason: "上游没给判据字段（cloudStatus / taskStatus），判不了，本次没发请求".into(),
        ..Default::default()
    };
    let text = result.summary_line();
    assert!(text.contains("cloudStatus / taskStatus"), "实得：{text}");
}

/// `NoticeResult` 的零值档位必须是 **Failed**，不能是 Done。
///
/// 这个结构体有多条提前 `return` 的路径 —— 零值若是"已领"，
/// 一句"领到了"就会在**一个请求都没发**的那趟上被念出来。
/// （⚠ 参考实现那一侧正好有这个坑：它的 `NoticeStatus` 零值就是"已领"。）
#[test]
fn the_zero_verdict_is_failed_not_done() {
    assert_eq!(NoticeVerdict::default(), NoticeVerdict::Failed);
    assert!(
        NoticeResult::default().task_outcome().is_failure(),
        "零值必须落失败（它没有 error 可念，那就念一句平铺直叙的失败）"
    );
}

/// 链路层那个结构体的零值档位必须是 `None`（"没走到能报结论的那一步"），
/// **不能**是 `Claimed`。
#[test]
fn the_zero_outcome_kind_is_none_not_claimed() {
    assert_eq!(NoticeOutcomeKind::default(), NoticeOutcomeKind::None);
    // 零值投影出来是失败，不是"已领"。
    assert_eq!(
        project(&NoticeOutcome::default()).verdict,
        NoticeVerdict::Failed
    );
}

/// 判定档位的标识是**稳定字符串**（不是 `Debug` 输出）。
#[test]
fn verdict_slugs_are_stable() {
    assert_eq!(verdict_slug(NoticeVerdict::Done), "done");
    assert_eq!(verdict_slug(NoticeVerdict::Nothing), "nothing");
    assert_eq!(verdict_slug(NoticeVerdict::Manual), "manual");
    assert_eq!(verdict_slug(NoticeVerdict::Unknown), "unknown");
    assert_eq!(verdict_slug(NoticeVerdict::Failed), "failed");
}

// ══════════════════════════ 注册表接入 ══════════════════════════

/// ⭐ `notice` 真的注册进了 registry —— CLI 与 `/api/capabilities` 都由它驱动。
///
/// 这条是"接入"这件事的判据：光有任务类型不注册，用户在 CLI 里 `run notice`
/// 会拿到"未知任务"，而 `/api/capabilities` 里根本不出现。
///
/// ⚠ 注册那一行在 `lib.rs` 的 `register_all` 里（**不在本模块**，由接线方维护）——
/// 这条用例钉的是"名字与元信息对得上"，不是"接线不会被改"。
#[test]
fn notice_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let metas = registry.list();
    let meta = metas
        .iter()
        .find(|m| m.name == "notice")
        .expect("notice 必须注册（CLI/capabilities 都按 registry 走）");

    assert_eq!(meta.title, "开启通知领AI豆");
    assert!(
        meta.params.is_empty(),
        "这一条没有参数（判据全在上游响应里，不需要用户填）"
    );
    assert!(
        !meta.hidden,
        "它该出现在通用任务列表里 —— 隐藏的只有 exchange 与 auth-refresh"
    );
    assert!(
        !meta.description.is_empty(),
        "描述是界面上唯一解释这一条做什么的地方"
    );

    // 旧的几条没被这次改动碰掉。
    for name in ["sign", "exchange", "msg-push"] {
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
    mcloud_core::Error::Other("公告任务不该碰 store".into())
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
