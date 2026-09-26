//! `live-room` / `red-flower` 编排层测试。
//!
//! 全程离线：假 HTTP 按脚本回放**实拍形状**的响应，断言的是
//! "发了哪几枪、按什么分支走、结论说成什么"。
//!
//! 判据来源：协议层 `live_flower` / `live_room` 的实拍值
//! （`flowerCnt=235`、`200108` / `200112` / `200118` / `200110`）。
//!
//! # ⚠ 替身是本文件**本地**写的，不是 `mcloud_core::testkit` 的
//!
//! `testkit` 只提供 [`FixedClock`]（固定时钟）。`Scripted` / `CollectSink` /
//! `NoStore` 这三个是 `signin/tests.rs` **本地**定义的，本文件照它的做法各写一份
//! —— 理由与那边一样：这三个替身的形状跟着被测任务走，
//! 提到公共处会让"某天签到族改了形状"波及这边。

use std::sync::Mutex;

use async_trait::async_trait;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, DeviceId, HttpClient, HttpRequest, HttpResponse,
};

use super::*;

// ── 测试替身 ────────────────────────────────────────────────────────────

/// 按顺序回放响应，并记录收到过哪些请求。
#[derive(Default)]
struct Scripted {
    seen: Mutex<Vec<HttpRequest>>,
    replies: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(replies: &[&str]) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            // 反转，便于从队尾取（用尽后重复最后一个）
            replies: Mutex::new(replies.iter().rev().map(|s| s.to_string()).collect()),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }

    /// 命中某个路径片段的请求数。
    fn count_of(&self, needle: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.url.contains(needle))
            .count()
    }

    /// 投出去的口令（从 `draw` 的 query 里取，已解码）。
    fn drawn_codes(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter(|r| r.url.contains("/redFlower/draw"))
            .map(|r| percent_decode(r.url.split("code=").nth(1).unwrap_or_default()))
            .collect()
    }
}

/// 极简的 `%XX` 解码（只为断言"投的是哪条口令"）。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req);
        let body = {
            let mut r = self.replies.lock().unwrap();
            if r.len() > 1 {
                r.pop().unwrap()
            } else {
                r.last().cloned().unwrap_or_else(|| "{}".into())
            }
        };
        Ok(HttpResponse {
            status: 200,
            body,
            headers: Vec::new(),
        })
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

/// 造一个只带 `http` 的 ctx（其余字段用最小替身）。
struct Ctx {
    http: std::sync::Arc<dyn HttpClient>,
    account: Account,
    sink: std::sync::Arc<CollectSink>,
    clock: std::sync::Arc<TestClock>,
}

#[derive(Default)]
struct CollectSink {
    events: Mutex<Vec<mcloud_core::RunEvent>>,
}

impl CollectSink {
    /// 所有 warn 级日志。
    fn warns(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                mcloud_core::RunEvent::Log {
                    message,
                    level: mcloud_core::LogLevel::Warn,
                    ..
                } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }
}

impl mcloud_core::EventSink for CollectSink {
    fn emit(&self, event: mcloud_core::RunEvent) {
        self.events.lock().unwrap().push(event);
    }
}

/// 固定时钟 —— 用 `testkit` 那一份，不自己再写一个。
///
/// ⚠ 不 `use ... as FixedClock` 起别名：那样拿到的是**类型别名**，
/// 而类型别名**不能当构造函数用**（`FixedClock(0)` 会报 E0423）。
use mcloud_core::testkit::FixedClock as TestClock;

impl Ctx {
    fn new(replies: &[&str]) -> (Self, std::sync::Arc<Scripted>) {
        let http = std::sync::Arc::new(Scripted::new(replies));
        (
            Self {
                http: http.clone(),
                account: account(),
                sink: std::sync::Arc::new(CollectSink::default()),
                clock: std::sync::Arc::new(TestClock(1_700_000_000_000)),
            },
            http,
        )
    }

    fn ctx(&self) -> TaskContext {
        TaskContext {
            account: self.account.clone(),
            http: self.http.clone(),
            store: std::sync::Arc::new(NoStore),
            clock: self.clock.clone(),
            sink: self.sink.clone(),
            run_id: mcloud_core::RunId(1),
            cancel: mcloud_core::CancelToken::new(),
            chain_cancel: None,
            artifacts: mcloud_engine::ArtifactRecorder::new(
                std::sync::Arc::new(mcloud_engine::ArtifactLedger::new()),
                mcloud_core::RunId(1),
                mcloud_core::AccountId::new("19****01"),
            ),
        }
    }
}

/// 造一个带脚本指令的 ctx（与 [`Ctx::new`] 同一形状，供动态拼装用）。
fn ctx_with(replies: &[String]) -> (Ctx, std::sync::Arc<Scripted>) {
    let refs: Vec<&str> = replies.iter().map(String::as_str).collect();
    Ctx::new(&refs)
}

// ── 假收集器（口令来源） ────────────────────────────────────────────────

/// 按脚本回放口令来源，并记录"要过哪些排除项"。
#[derive(Default)]
struct FakeCollector {
    /// 每次 `collect` 依次返回一批口令；用尽后返回空。
    batches: Mutex<Vec<Vec<String>>>,
    /// 每次 `collect` 收到的 `exclude`。
    excludes: Mutex<Vec<Vec<String>>>,
    /// 是否让 `collect` 报错。
    fails: Mutex<bool>,
}

impl FakeCollector {
    fn new(batches: &[&[&str]]) -> Self {
        Self {
            batches: Mutex::new(
                batches
                    .iter()
                    .map(|b| b.iter().map(|s| s.to_string()).collect())
                    .collect(),
            ),
            ..Default::default()
        }
    }

    fn empty() -> Self {
        Self::default()
    }

    fn failing() -> Self {
        Self {
            fails: Mutex::new(true),
            ..Default::default()
        }
    }

    fn call_count(&self) -> usize {
        self.excludes.lock().unwrap().len()
    }

    /// 第 n 次（0 起）收集时的排除表。
    fn exclude_at(&self, n: usize) -> Vec<String> {
        self.excludes
            .lock()
            .unwrap()
            .get(n)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl KoulingCollector for FakeCollector {
    async fn collect(
        &self,
        _ctx: &TaskContext,
        _jwt: &str,
        _room: &live::LiveRoom,
        _listen_for: Duration,
        exclude: &[String],
    ) -> Result<SourceOutcome> {
        self.excludes.lock().unwrap().push(exclude.to_vec());
        if *self.fails.lock().unwrap() {
            return Err(Error::Other("假收集器：两个来源都失败了".into()));
        }
        let mut b = self.batches.lock().unwrap();
        let codes = if b.is_empty() {
            Vec::new()
        } else {
            b.remove(0)
        };
        let excluded: Vec<&str> = exclude.iter().map(|s| s.trim()).collect();
        Ok(SourceOutcome {
            codes: codes
                .into_iter()
                .filter(|c| !excluded.contains(&c.as_str()))
                .collect(),
            failure: None,
        })
    }
}

// ── 响应构造 ────────────────────────────────────────────────────────────

/// `user/info`（小红花余额）。实拍形状：`result.flowerCnt` 是余额。
fn balance_body(flower_cnt: i64) -> String {
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "isQuarterDrawed": 0,
            "flowerCnt": flower_cnt,
            "prizeId": 0,
            "secondPrizeId": 0,
            "redFlowerValidDate": "2026年12月31日",
            "hasNotReceivedRedFlower": 0
        }
    })
    .to_string()
}

/// `draw` 成功：回 `flowerNum` 朵。
fn draw_ok(flower_num: i64) -> String {
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"prizeId": 1, "flowerNum": flower_num, "prizeName": "小红花", "prizeType": 1}
    })
    .to_string()
}

/// `draw` / `give` 的业务码响应。
fn code_body(code: i64, msg: &str) -> String {
    serde_json::json!({"code": code, "msg": msg}).to_string()
}

/// `firstParticipate/give` 成功：回 `flowerNum` 朵。
fn give_ok(flower_num: i64) -> String {
    serde_json::json!({
        "code": 0, "msg": "success", "result": {"flowerNum": flower_num}
    })
    .to_string()
}

/// `firstParticipate/give`：已参与过（**正常**的一支）。
fn give_already() -> String {
    code_body(200110, "已参与过")
}

/// `queryRevivalFlowers`：`result` 是**裸数字**。
fn revival_body(n: i64) -> String {
    serde_json::json!({"code": 0, "msg": "success", "result": n}).to_string()
}

/// 埋点响应（**内容无所谓**，链上不解析它）。
fn journaling_ok() -> String {
    serde_json::json!({"code": 0, "msg": "success"}).to_string()
}

/// 往 `v` 里追加 5 条埋点响应 —— 六步里②要发满 [`proto::JOURNALING_EVENTS`] 条。
fn push_journaling(v: &mut Vec<String>) {
    for _ in 0..proto::JOURNALING_EVENTS.len() {
        v.push(journaling_ok());
    }
}

/// 造一份"正常链"的响应序列：①余额 → ②埋点×5 → ③give → ④draw×N → ⑤复活 → ⑥余额。
fn happy_chain(
    before: i64,
    after: i64,
    give: Option<String>,
    draws: &[String],
    revival: i64,
) -> Vec<String> {
    let mut v = vec![balance_body(before)];
    push_journaling(&mut v);
    v.push(give.unwrap_or_else(|| give_ok(0)));
    v.extend(draws.iter().cloned());
    v.push(revival_body(revival));
    v.push(balance_body(after));
    v
}

/// 两个页签的 `tabContentList` 响应（门那一步）。
fn tab_body(rooms: serde_json::Value) -> String {
    serde_json::json!({
        "rtnCode": "0", "rtnMsg": null,
        "object": {"dataList": rooms}
    })
    .to_string()
}

/// 一间房的 `dataList[]` 条目。
fn room_entry(id: i64, status: i64, nickname: &str) -> serde_json::Value {
    serde_json::json!({
        "room": {
            "id": id, "title": "直播", "status": status, "channelId": 134,
            "expectStartTime": "", "anchorId": 1, "authorId": 1,
            "anchor": {"nickname": nickname}
        }
    })
}

/// 造一个 `LiveRoomRun`（门那一支默认关着）。
fn run_with<'a>(collector: &'a dyn KoulingCollector) -> LiveRoomRun<'a> {
    LiveRoomRun {
        collector,
        live_jwt: None,
    }
}

/// 造一个**门开着**的 run（`live_jwt` 非空 ⇒ 会去打两个页签）。
fn run_gated<'a>(collector: &'a dyn KoulingCollector) -> LiveRoomRun<'a> {
    LiveRoomRun {
        collector,
        live_jwt: Some("LIVE-JWT".into()),
    }
}

/// 关掉重听那一支的参数（专测"投了之后"的结论）。
fn no_retry_params(codes: &[&str]) -> LiveRoomParams {
    LiveRoomParams {
        codes: codes.iter().map(|s| s.to_string()).collect(),
        listen_seconds: 60,
        kouling_only: false,
        skip_kouling: true,
    }
}

// ══════════════════════════ 判据 1：正常链 ══════════════════════════

/// ⭐⭐ 正常链：余额 235 → 口令成功 +10 → 终值 245 ⇒ 报告**进账 = 10**（⑥−①）。
///
/// 这是任务描述里点名的那条用例。它同时钉住三件事：
/// 六步的**顺序**、埋点发了 **5** 条、进账走**两次读数相减**。
#[tokio::test]
async fn happy_chain_reports_gain_from_two_readings() {
    let (c, http) = ctx_with(&happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let collector = FakeCollector::empty();
    let params = LiveRoomParams {
        codes: vec!["中秋口令".into()],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_with(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    let report = res.report.as_ref().expect("应当有报告");
    assert_eq!(report.balance_before.flower_cnt, 235);
    assert_eq!(report.balance_after.flower_cnt, 245);
    assert_eq!(report.gained(), 10, "进账 = ⑥ − ①");
    assert_eq!(report.succeeded(), 1);
    assert_eq!(report.attempted(), 1);
    assert_eq!(res.verdict(), RunVerdict::AllSucceeded);
    assert!(res.outcome().is_success(), "{:?}", res.outcome());

    // ⚠ 埋点必须发满 5 条（少发一条服务端不会拒，只会回 200108 —— 方向全错）。
    assert_eq!(http.count_of("/visitlog/journaling"), 5, "埋点固定 5 条");
    assert_eq!(http.count_of("/liveRoomFeedback/user/info"), 2, "①⑥ 两枪");
    assert_eq!(http.count_of("/firstParticipate/give"), 1);
    assert_eq!(http.count_of("/redFlower/draw"), 1);
    assert_eq!(http.count_of("/queryRevivalFlowers"), 1);

    // 六步的**顺序**：① → ② → ③ → ④ → ⑤ → ⑥
    let urls: Vec<String> = http.requests().iter().map(|r| r.url.clone()).collect();
    let idx = |needle: &str| urls.iter().position(|u| u.contains(needle)).unwrap();
    assert!(
        idx("/user/info") < idx("/visitlog/journaling"),
        "① 在 ② 之前"
    );
    assert!(
        idx("/visitlog/journaling") < idx("/firstParticipate/give"),
        "② 在 ③ 之前"
    );
    assert!(
        idx("/firstParticipate/give") < idx("/redFlower/draw"),
        "③ 在 ④ 之前"
    );
    assert!(
        idx("/redFlower/draw") < idx("/queryRevivalFlowers"),
        "④ 在 ⑤ 之前"
    );
    assert!(
        urls.iter().rposition(|u| u.contains("/user/info")).unwrap() > idx("/queryRevivalFlowers"),
        "⑥ 在最后"
    );

    assert!(
        res.outcome().summary().contains("进账 10"),
        "实得：{}",
        res.outcome().summary()
    );
}

/// ⭐⭐ 进账**不能漏掉首次参与奖**：口令只 +0，但 give +N ⇒ 进账含那 N 朵。
///
/// 这是钉「进账 = ⑥−①，不是把 flowerNum 相加」的那条判据。
/// **把口令的 `flowerNum` 相加的实现在这里会得到 0。**
#[tokio::test]
async fn gain_includes_the_first_participate_bonus() {
    // give +7，口令 +0，但终值比起始多 7
    let (c, _http) = ctx_with(&happy_chain(100, 107, Some(give_ok(7)), &[draw_ok(0)], 0));
    let collector = FakeCollector::empty();
    let params = no_retry_params(&["口令甲"]);

    let res = run_with(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();
    let report = res.report.as_ref().unwrap();

    assert_eq!(
        report.gained(),
        7,
        "进账必须含③首次参与奖那 7 朵 —— 把口令的 flowerNum 相加会得到 0"
    );
    assert_eq!(report.succeeded(), 1, "口令本身是成功的（只是 +0）");
    // 尾巴上要如实说出这次首参拿了多少
    assert!(
        report.summary_line().contains("首次参与 +7"),
        "实得：{}",
        report.summary_line()
    );
}

// ══════════════════════════ 判据 2：重试路径 ══════════════════════════

/// ⭐ 没有无效口令 ⇒ **不重听**（排除表都不该被构造出来）。
#[test]
fn retry_plan_has_no_retry_without_invalid_codes() {
    let (retry, exclude) = retry_plan(&["甲".into(), "乙".into()], &[]);
    assert!(!retry, "没有无效口令就不该重听");
    assert!(exclude.is_empty());
}

/// ⭐ 重听时的排除表 = 第一批 ∪ 无效表。
///
/// ⚠ 关键：**第一次就成功的口令也必须进排除表** —— 不排除的话第二轮会把
/// 同一批再投一遍，而同一账号对同一条**正确**口令的第二次请求回 `200112`，
/// 会污染归因、看起来像"口令坏了"。
///
/// ⚠⚠ 这个理由**不是**"口令会被消耗"（口令全场通用）。本测试的注释里也刻意
/// 不出现"消耗"二字 —— 那个概念会把人引向错误的方向。
#[test]
fn retry_plan_excludes_both_the_successful_and_the_invalid() {
    let first: Vec<String> = vec!["成功甲".into(), "无效乙".into()];
    let invalid: Vec<String> = vec!["无效乙".into()];
    let (retry, exclude) = retry_plan(&first, &invalid);

    assert!(retry, "有无效口令就该重听");
    assert!(exclude.contains(&"无效乙".to_string()), "无效的要排除");
    assert!(
        exclude.contains(&"成功甲".to_string()),
        "⭐ **第一次就成功的**也必须排除 —— 否则第二轮会重投它对，回 200112"
    );
}

/// ⭐ `pick_new_codes`：只挑既不在第一批、也不在无效表里的。
#[test]
fn pick_new_codes_filters_out_everything_already_tried() {
    let retried: Vec<String> = vec!["新丙".into(), "成功甲".into(), "无效乙".into()];
    let first: Vec<String> = vec!["成功甲".into(), "无效乙".into()];
    let invalid: Vec<String> = vec!["无效乙".into()];

    let to_draw = pick_new_codes(&retried, &first, &invalid);
    assert_eq!(to_draw, vec!["新丙".to_string()], "只该投真正的新口令");
}

/// ⭐⭐ 重试路径端到端：无效口令进排除表、**成功的不进**、不重复投递。
///
/// 第一轮投「甲（成功）+ 乙（200108）」；重听又听到「乙 + 丙」，
/// 而排除表必须把乙挡住 ⇒ 第二轮**只投丙**。
#[tokio::test]
async fn invalid_code_triggers_one_retry_and_never_redraws_tried_codes() {
    // 门那两步（两个页签）—— **重听需要一间可听的房间**，门没跑就没有。
    let mut chain = vec![
        tab_body(serde_json::json!([room_entry(147606, 1, "中国移动云盘")])),
        tab_body(serde_json::json!([])),
    ];
    // 第一轮：甲成功、乙无效
    chain.push(balance_body(10));
    push_journaling(&mut chain);
    chain.push(give_already());
    chain.push(draw_ok(5)); // 甲成功
    chain.push(code_body(200108, "口令无效")); // 乙无效
    chain.push(revival_body(0));
    chain.push(balance_body(15)); // 第一轮终值
                                  // 第二轮（只投丙）
    chain.extend(happy_chain(15, 18, Some(give_already()), &[draw_ok(3)], 0));
    let (c, http) = ctx_with(&chain);

    // 这一趟给了手工 codes（所以首轮**不**收集），但门是开的 ⇒ 重听时能听到新口令。
    let collector = FakeCollector::new(&[&["乙", "丙"]]);
    let params = LiveRoomParams {
        codes: vec!["甲".into(), "乙".into()],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert!(res.retried, "有无效口令 ⇒ 触发过重听");
    assert_eq!(collector.call_count(), 1, "只重听一次");

    // ⭐ 排除表必须含"甲（成功）"与"乙（无效）"两者
    let exclude = collector.exclude_at(0);
    assert!(
        exclude.contains(&"甲".to_string()),
        "成功过的也要排除：{exclude:?}"
    );
    assert!(
        exclude.contains(&"乙".to_string()),
        "无效的要排除：{exclude:?}"
    );

    // ⭐ 投递序列：甲 → 乙 → 丙，**乙只投一次**（第二轮被排除表挡住）
    let drawn = http.drawn_codes();
    assert_eq!(
        drawn,
        vec!["甲".to_string(), "乙".to_string(), "丙".to_string()],
        "不重复投递已试过的"
    );
    assert_eq!(
        drawn.iter().filter(|c| *c == "乙").count(),
        1,
        "无效口令只投一次"
    );

    // 两轮合并：进账 = 第一轮起始 10 → 第二轮终值 18
    let report = res.report.as_ref().unwrap();
    assert_eq!(report.balance_before.flower_cnt, 10, "起始取第一轮");
    assert_eq!(report.balance_after.flower_cnt, 18, "终值取第二轮");
    assert_eq!(report.gained(), 8);
    assert_eq!(report.attempted(), 3, "三轮记录都在（甲、乙、丙）");
    assert_eq!(
        report.invalid_codes,
        vec!["乙".to_string()],
        "⭐ 无效口令取**并集**：第二轮里没有乙只是因为它被排除了，"
    );
}

/// ⭐ 没有无效口令 ⇒ **不重听**（一次正常运行不该多听 60 秒）。
#[tokio::test]
async fn no_invalid_code_means_no_extra_listen() {
    let (c, _http) = ctx_with(&happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let collector = FakeCollector::new(&[&["不该被用到"]]);
    let params = LiveRoomParams {
        codes: vec!["口令甲".into()],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_with(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert!(!res.retried, "全成功就不该重听");
    assert_eq!(collector.call_count(), 0, "压根不该去收集");
}

// ══════════════════════════ 判据 3：200118 与四分类 ══════════════════════════

/// ⭐⭐ `200118`（口令已失效/本场已结束）**不触发重听**，且与未分类**分成两句**。
#[tokio::test]
async fn expired_code_does_not_retry_and_gets_its_own_sentence() {
    let (c, _http) = ctx_with(&happy_chain(
        235,
        235,
        Some(give_already()),
        &[
            code_body(200118, "口令已失效"),       // 已失效（本场结束）
            code_body(400001, "上游说了个别的码"), // 未分类
        ],
        0,
    ));
    let collector = FakeCollector::new(&[&["不该被用到"]]);
    let params = LiveRoomParams {
        codes: vec!["失效甲".into(), "未分类乙".into()],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_with(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();
    let report = res.report.as_ref().unwrap();

    // ⭐ 200118 不进无效表 ⇒ 不触发重听（重听也只会再拿一次 200118）
    assert!(
        !report.invalid_codes.contains(&"失效甲".to_string()),
        "200118 不是无效口令：{:?}",
        report.invalid_codes
    );
    assert!(!res.retried, "200118 不触发重听");
    assert_eq!(collector.call_count(), 0, "不该去重听");

    assert_eq!(report.expired_codes, vec!["失效甲".to_string()]);
    assert_eq!(report.other_codes, vec!["未分类乙".to_string()]);
    assert!(report.invalid_codes.is_empty());

    // ⭐ 两句必须**分开**，且 200118 那句不能是"未分类口令"
    let line = report.summary_line();
    assert!(
        line.contains("口令已失效（本场已结束）失效甲"),
        "200118 要有自己那句：{line}"
    );
    assert!(
        line.contains("未分类口令 未分类乙"),
        "未分类要单独一句：{line}"
    );
    assert!(
        !line.contains("未分类口令 失效甲"),
        "⚠ 别把 200118 说成「未分类口令」——那会把人引向「口令抄错了」：{line}"
    );
    assert_ne!(
        report.expired_codes, report.other_codes,
        "两栏装的东西不是一回事"
    );
}

/// ⭐ 三种码各自落在自己的栏里（状态机级，一次把四分类钉住）。
#[test]
fn draw_kinds_land_in_their_own_buckets() {
    let mut s = RedFlowerSession::new(vec![
        "无效".into(),
        "已兑换".into(),
        "已失效".into(),
        "未分类".into(),
        "成功".into(),
    ]);
    s.feed(&balance_body(0)).unwrap();
    for _ in 0..proto::JOURNALING_EVENTS.len() {
        s.feed(&journaling_ok()).unwrap();
    }
    s.feed(&give_already()).unwrap();
    s.feed(&code_body(200108, "口令无效")).unwrap();
    s.feed(&code_body(200112, "已兑换")).unwrap();
    s.feed(&code_body(200118, "已失效")).unwrap();
    s.feed(&code_body(999, "别的码")).unwrap();
    s.feed(&draw_ok(5)).unwrap();
    s.feed(&revival_body(0)).unwrap();
    s.feed(&balance_body(5)).unwrap();

    assert!(s.done(), "六步应当走完");
    let r = s.report();
    assert_eq!(
        r.invalid_codes,
        vec!["无效".to_string()],
        "只有 200108 进无效表"
    );
    assert_eq!(r.expired_codes, vec!["已失效".to_string()]);
    assert_eq!(r.other_codes, vec!["未分类".to_string()]);
    assert_eq!(r.draws.len(), 5);
    assert_eq!(r.succeeded(), 1, "只有最后那条成功");
    assert_eq!((r.succeeded(), r.attempted()), (1, 5));
    // ⚠ 200112（已兑换）既不进无效表、也不进未分类 —— 口令是**对的**
    assert!(
        !r.invalid_codes.contains(&"已兑换".to_string())
            && !r.other_codes.contains(&"已兑换".to_string()),
        "200112 是独立的一档（口令对、已领过）"
    );
}

/// ⭐ 投了、**一个都没成** ⇒ 结论必须是**失败**（不是"跳过"，也不是"成功"）。
#[tokio::test]
async fn all_draws_failed_is_a_failed_outcome() {
    let (c, _http) = ctx_with(&happy_chain(
        235,
        235,
        Some(give_already()),
        &[code_body(200108, "口令无效")],
        0,
    ));
    let collector = FakeCollector::empty();
    let params = no_retry_params(&["无效甲"]);

    let res = run_with(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();
    assert_eq!(res.verdict(), RunVerdict::NoneSucceeded);
    let out = res.outcome();
    assert!(out.is_failure(), "投了全败必须报失败：{out:?}");
    // 失败小结里必须带**原因**，不能只剩光秃秃的"失败"
    assert!(
        out.summary().contains("无效口令"),
        "失败要说清为什么：{}",
        out.summary()
    );
}

/// ⭐ 投了、**成了部分** ⇒ 部分成功（不是失败）。
#[tokio::test]
async fn partial_success_is_reported_as_partial() {
    let (c, _http) = ctx_with(&happy_chain(
        235,
        240,
        Some(give_already()),
        &[draw_ok(5), code_body(200108, "口令无效")],
        0,
    ));
    let collector = FakeCollector::empty();
    let params = no_retry_params(&["成功甲", "无效乙"]);

    let res = run_with(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();
    assert_eq!(res.verdict(), RunVerdict::PartialSuccess);
    let out = res.outcome();
    assert!(
        !out.is_failure(),
        "部分成功不该报失败（用户拿到了一部分）：{out:?}"
    );
    assert!(
        out.summary().contains("部分成功"),
        "实得：{}",
        out.summary()
    );
    assert!(
        out.summary().contains("成功 1/2"),
        "实得：{}",
        out.summary()
    );
}

// ══════════════════════════ 判据 4：开播门未开 ══════════════════════════

/// ⭐ 门没开 ⇒ **不跑链**，且结论可区分（"今天没开播"是成功，不是失败）。
#[tokio::test]
async fn closed_gate_does_not_run_the_chain() {
    // 门那两步：两个页签都回空房间 ⇒ live_count == 0
    let (c, http) = Ctx::new(&[
        &tab_body(serde_json::json!([])),
        &tab_body(serde_json::json!([])),
    ]);
    let collector = FakeCollector::new(&[&["不该被收集"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert!(res.report.is_none(), "门没开 ⇒ 一份报告都不该有");
    assert!(!res.gate.active);
    assert_eq!(res.gate.live_count, 0, "谁都没在播");
    assert_eq!(collector.call_count(), 0, "门没开就不该去收集");
    assert_eq!(
        http.count_of("/liveRoomFeedback"),
        0,
        "更不该打小红花那六步"
    );
    // ⚠ "今天没开播"是**正常结果**，不是失败
    assert!(
        res.outcome().is_success(),
        "门没开是正常结果，不是失败：{:?}",
        res.outcome()
    );
    assert!(
        res.outcome().summary().contains("今天没有直播"),
        "实得：{}",
        res.outcome().summary()
    );
}

/// ⭐⭐ `LiveCount` 的**唯一理由**：两种"门没开"的话必须**不同**。
///
/// - `live_count == 0` ⇒ 谁都没在播 ⇒ 下一步是**等**；
/// - `live_count > 0 && !active` ⇒ 有人**在播**但不是那个主播 ⇒ 下一步是**别等了**。
///
/// 混成一句"没有直播"会让人去查**收集**那一步，而那一步根本没被走到。
#[tokio::test]
async fn the_two_ways_a_gate_can_be_closed_say_different_things() {
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    // 情形 A：一个在播房间都没有
    let (c_a, _) = Ctx::new(&[
        &tab_body(serde_json::json!([])),
        &tab_body(serde_json::json!([])),
    ]);
    let collector_a = FakeCollector::empty();
    let res_a = run_gated(&collector_a)
        .run_account(&c_a.ctx(), &params)
        .await
        .unwrap();

    // 情形 B：有房间在播，但主播不是「中国移动云盘」（实拍 147490「福建移动」）
    let live = serde_json::json!([room_entry(147490, 1, "福建移动")]);
    let (c_b, _) = Ctx::new(&[&tab_body(live), &tab_body(serde_json::json!([]))]);
    let collector_b = FakeCollector::empty();
    let res_b = run_gated(&collector_b)
        .run_account(&c_b.ctx(), &params)
        .await
        .unwrap();

    assert_eq!(res_a.gate.live_count, 0);
    assert_eq!(res_b.gate.live_count, 1, "有人在播，只是不是那个主播");
    assert!(!res_b.gate.active, "昵称不对 ⇒ 门不算开");

    let a = res_a.idle_summary();
    let b = res_b.idle_summary();
    assert!(a.contains("今天没有直播"), "实得：{a}");
    assert!(
        b.contains("有 1 间在播") && b.contains("中国移动云盘"),
        "实得：{b}"
    );
    assert_ne!(
        a, b,
        "⭐ 两种「没有」必须说得不一样（这就是 LiveCount 存在的理由）"
    );
}

/// ⭐ 门开着（昵称精确相等）⇒ 才算门开了。
///
/// 判据是**相等**不是 `contains`：实拍那张表里还有「福建移动」「移动心选」
/// 「移动臻选」，用包含关系一个都拦不住（都含「移动」）。
#[tokio::test]
async fn only_the_exact_anchor_nickname_opens_the_gate() {
    for nickname in ["福建移动", "移动心选", "移动臻选", "中国移动云盘x"] {
        let rooms = serde_json::json!([room_entry(1, 1, nickname)]);
        let (c, _) = Ctx::new(&[&tab_body(rooms), &tab_body(serde_json::json!([]))]);
        let collector = FakeCollector::empty();
        let params = LiveRoomParams {
            codes: vec![],
            listen_seconds: 60,
            ..Default::default()
        };
        let res = run_gated(&collector)
            .run_account(&c.ctx(), &params)
            .await
            .unwrap();
        assert!(!res.gate.active, "「{nickname}」不该开门（要精确相等）");
    }

    // 预告（status=0）与回放（status=6）也**不算** —— 那时没有聊天室
    for status in [0, 6] {
        let rooms = serde_json::json!([room_entry(1, status, "中国移动云盘")]);
        let (c, _) = Ctx::new(&[&tab_body(rooms), &tab_body(serde_json::json!([]))]);
        let collector = FakeCollector::empty();
        let params = LiveRoomParams {
            codes: vec![],
            listen_seconds: 60,
            ..Default::default()
        };
        let res = run_gated(&collector)
            .run_account(&c.ctx(), &params)
            .await
            .unwrap();
        assert!(
            !res.gate.active,
            "status={status} 不该开门（只有 1 是在播）"
        );
    }
}

/// ⭐ 只听不投：收到了口令但**一个都不投**，且说明与另两种"没投"不同。
#[tokio::test]
async fn kouling_only_collects_but_never_draws() {
    let (c, http) = Ctx::new(&[
        &tab_body(serde_json::json!([room_entry(147606, 1, "中国移动云盘")])),
        &tab_body(serde_json::json!([])),
    ]);
    let collector = FakeCollector::new(&[&["口令甲", "口令乙"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        kouling_only: true,
        skip_kouling: false,
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert_eq!(res.codes, vec!["口令甲".to_string(), "口令乙".to_string()]);
    assert!(res.report.is_none(), "只听不投 ⇒ 没有报告");
    assert_eq!(http.count_of("/liveRoomFeedback"), 0, "一个口令都不能投");
    assert!(res.outcome().is_success());
    assert!(
        res.outcome().summary().contains("本次只收不投"),
        "实得：{}",
        res.outcome().summary()
    );
    // ⚠ 措辞里**不能**出现"消耗"这个概念（口令全场通用）
    assert!(
        !res.outcome().summary().contains("消耗"),
        "口令全场通用，不存在消耗：{}",
        res.outcome().summary()
    );
}

/// ⭐ 门开了但没收到口令 ⇒ **第三种**说法（不是"没直播"，也不是"只听不投"）。
#[tokio::test]
async fn opened_gate_with_no_codes_says_so() {
    let (c, http) = Ctx::new(&[
        &tab_body(serde_json::json!([room_entry(147606, 1, "中国移动云盘")])),
        &tab_body(serde_json::json!([])),
    ]);
    let collector = FakeCollector::new(&[&[]]); // 听到了 0 条
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert!(res.gate.active, "门是开的");
    assert!(res.report.is_none());
    assert_eq!(http.count_of("/liveRoomFeedback"), 0);
    assert_eq!(
        res.idle_summary(),
        "开了门但没收到口令",
        "这是第三种「没投」，措辞不能与另两种混"
    );
}

/// ⭐ 房间数 > 1 ⇒ **必须 warn**（代价告知：只听第一间，其余的整场漏掉）。
#[tokio::test]
async fn multiple_live_rooms_produce_a_warning() {
    let rooms = serde_json::json!([
        room_entry(147606, 1, "中国移动云盘"),
        room_entry(147608, 1, "中国移动云盘"),
    ]);
    // 门两个页签 + 一轮完整的六步链（收集到的口令会被真的投出去）
    let mut chain = vec![tab_body(rooms), tab_body(serde_json::json!([]))];
    chain.extend(happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let (c, _http) = ctx_with(&chain);
    let collector = FakeCollector::new(&[&["口令甲"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert_eq!(res.gate.rooms.len(), 2);
    assert_eq!(collector.call_count(), 1, "只听**第一间**（刻意的取舍）");
    let warns = c.sink.warns();
    assert!(
        warns.iter().any(|w| w.contains("只听第一间")),
        "⭐ 房间数 > 1 必须 warn，否则用户以为已经听全了：{warns:?}"
    );
}

/// ⭐ 房间数 == 1 时**不该**有那句 warn（否则它就成了噪声，用户会忽略它）。
#[tokio::test]
async fn single_live_room_does_not_warn_about_coverage() {
    let rooms = serde_json::json!([room_entry(147606, 1, "中国移动云盘")]);
    let mut chain = vec![tab_body(rooms), tab_body(serde_json::json!([]))];
    chain.extend(happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let (c, _http) = ctx_with(&chain);
    let collector = FakeCollector::new(&[&["口令甲"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    let warns = c.sink.warns();
    assert!(
        !warns.iter().any(|w| w.contains("只听第一间")),
        "只有一间时不该提覆盖缺口：{warns:?}"
    );
}

// ══════════════════════════ 判据 5：失败传播 ══════════════════════════

/// ⭐⭐ 上游非零码必须让 run **失败**（不能吞错）—— 每一步都要传播，且带步名。
#[tokio::test]
async fn upstream_error_code_at_each_step_becomes_an_error() {
    let err = r#"{"code":500,"msg":"系统繁忙"}"#;
    let params = no_retry_params(&["甲"]);

    // ① 起始余额就失败
    let (c, _) = Ctx::new(&[err]);
    let e = run_with(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("user_info_before"),
        "错误要带步名：{e}"
    );

    // ⚠ ③ give 的**非零码不中止** —— 这是照抄参考实现的一处刻意行为：
    // `classify_first_participate` 只把码原样带出来，`feed` 不返回 Err
    // （唯一该拦的是"响应不是信封"，见下一个用例）。
    // 于是这一枪回 500 之后，链会**继续**走到 ⑥ 才失败 —— 而错误前缀
    // 正确地说出是 `user_info_after`。
    let mut chain = vec![balance_body(235)];
    push_journaling(&mut chain);
    chain.push(err.to_string());
    let (c, _) = ctx_with(&chain);
    let e = run_with(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("user_info_after"),
        "give 的非零码不中止（照抄参考实现），链走到⑥才失败：{e}"
    );

    // ⑥ 终值失败 —— ⚠ 必须能分清是**起始余额**还是**终值**（同一个接口）
    let mut chain = vec![balance_body(235)];
    push_journaling(&mut chain);
    chain.push(give_already());
    chain.push(draw_ok(5));
    chain.push(revival_body(0));
    chain.push(err.to_string());
    let (c, _) = ctx_with(&chain);
    let e = run_with(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("user_info_after"),
        "错误要带步名（要能分清起始余额与终值）：{e}"
    );
}

/// ⭐ 解析失败要**中止**（不是跳过）—— 上游换了信封形状时继续跑只会重复六遍。
#[tokio::test]
async fn unparsable_response_aborts_the_chain() {
    // ① 回一个不像信封的东西
    let (c, http) = Ctx::new(&["<html>网关错误</html>"]);
    let params = no_retry_params(&["甲"]);
    let e = run_with(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .unwrap_err();

    assert!(e.to_string().contains("user_info_before"), "实得：{e}");
    assert_eq!(
        http.count(),
        1,
        "⭐ 解析失败要中止，不能继续把同一个问题重复六遍"
    );
}

/// ⭐ **复活花是唯一例外**：拿不到它，留 0 并继续 —— 不该让整条链失败。
#[tokio::test]
async fn revival_flowers_failure_does_not_break_the_chain() {
    let mut chain = vec![balance_body(235)];
    push_journaling(&mut chain);
    chain.push(give_already());
    chain.push(draw_ok(10));
    // ⑤ 复活花回了非零码（解析失败）
    chain.push(r#"{"code":500,"msg":"系统繁忙"}"#.to_string());
    chain.push(balance_body(245));
    let (c, _) = ctx_with(&chain);
    let params = no_retry_params(&["甲"]);

    let res = run_with(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    let report = res.report.as_ref().expect("链应当跑完");
    assert_eq!(report.revival_flowers, 0, "拿不到就留 0");
    assert_eq!(report.gained(), 10, "后面的步骤照常跑完");
    assert!(res.outcome().is_success(), "{:?}", res.outcome());
}

/// ⭐ ⑥−① 为**负**时也如实报（余额被别的动作扣了），不粉饰成 0。
#[tokio::test]
async fn negative_gain_is_reported_honestly() {
    let (c, _) = ctx_with(&happy_chain(
        245,
        235,
        Some(give_already()),
        &[draw_ok(0)],
        0,
    ));
    let params = no_retry_params(&["甲"]);
    let res = run_with(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();
    assert_eq!(res.report.as_ref().unwrap().gained(), -10);
    assert!(
        res.outcome().summary().contains("进账 -10"),
        "实得：{}",
        res.outcome().summary()
    );
}

/// ⭐ 会话失效（90001）要让整趟失败，且说清是**票**的问题。
#[tokio::test]
async fn session_expiry_surfaces_as_an_error() {
    let (c, _) = Ctx::new(&[r#"{"code":90001,"msg":"未登录"}"#]);
    let params = no_retry_params(&["甲"]);
    let e = run_with(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .unwrap_err();
    let text = e.to_string();
    assert!(text.contains("会话"), "应说清是会话问题：{text}");
    assert!(text.contains("user_info_before"), "也要带步名：{text}");
}

// ══════════════════════════ 判据 6：参数校验四态 ══════════════════════════

/// ⭐ 参数校验的**三条规则**（判据在本层，只有一份）。
#[test]
fn params_check_covers_all_four_states() {
    // ① 成立
    let ok = no_retry_params(&["甲"]);
    assert_eq!(ok.check(), Ok(()));

    // ② 互斥：只听不投 + 只用给定口令
    let excl = LiveRoomParams {
        codes: vec!["甲".into()],
        listen_seconds: 60,
        kouling_only: true,
        skip_kouling: true,
    };
    assert_eq!(excl.check(), Err(LiveRoomFault::ExclusiveSwitches));

    // ③ 关掉唯一来源又没给手工口令
    let no_codes = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        kouling_only: false,
        skip_kouling: true,
    };
    assert_eq!(no_codes.check(), Err(LiveRoomFault::SkipWithoutCodes));

    // ④ 时长非正
    let zero = LiveRoomParams {
        codes: vec!["甲".into()],
        listen_seconds: 0,
        kouling_only: false,
        skip_kouling: true,
    };
    assert_eq!(zero.check(), Err(LiveRoomFault::ListenNotPositive));

    // ⚠ 0 秒 + 只听不投 = **仍然非法**（第三条不看 skip_kouling / kouling_only）
    let zero_only = LiveRoomParams {
        codes: vec![],
        listen_seconds: 0,
        kouling_only: true,
        skip_kouling: false,
    };
    assert_eq!(
        zero_only.check(),
        Err(LiveRoomFault::ListenNotPositive),
        "时长非正在任何情况下都非法"
    );

    // 三条规则的中性描述都非空（措辞由调用方写，但领域层得说得出自己是什么）
    for f in [
        LiveRoomFault::ExclusiveSwitches,
        LiveRoomFault::SkipWithoutCodes,
        LiveRoomFault::ListenNotPositive,
    ] {
        assert!(!f.describe().is_empty());
    }
}

/// ⭐ 任务层把参数错误翻成 `Err`，且措辞**点出 flag 名**（措辞在调用方）。
#[tokio::test]
async fn task_surfaces_invalid_params_as_an_error() {
    let (c, http) = Ctx::new(&[]);
    let task = LiveRoomTask::production();

    // codes 非空但跳过了弹幕来源 + 只听不投 ⇒ 互斥
    let body = serde_json::json!({
        "codes": "甲", "listenSeconds": 60,
        "koulingOnly": true, "skipKouling": true
    });
    let e = task.run(&c.ctx(), &body).await.unwrap_err();
    assert_eq!(http.count(), 0, "参数不成立就不该发任何请求");
    let text = e.to_string();
    assert!(
        text.contains("koulingOnly") && text.contains("skipKouling"),
        "要给出可操作的措辞（哪个 flag 冲突）：{text}"
    );

    // listenSeconds == 0
    let body = serde_json::json!({"codes": "甲", "listenSeconds": 0});
    let e = task.run(&c.ctx(), &body).await.unwrap_err();
    assert!(e.to_string().contains("listenSeconds"), "实得：{e}");
}

/// ⭐ `codes` 支持"一栏自由文本"（换行 / 逗号 / 顿号都是分隔符，保序去重）。
#[test]
fn codes_accept_free_text_with_multiple_separators() {
    let codes = split_kouling("口令甲,口令乙，口令丙、口令丁\n口令戊\n\n口令甲");
    assert_eq!(
        codes,
        vec!["口令甲", "口令乙", "口令丙", "口令丁", "口令戊"],
        "五条 + 末条重复（保序去重）+ 空行丢掉"
    );
}

/// ⭐ `codes` 也收字符串**数组**（两种形态都行）。
#[test]
fn params_accept_codes_as_an_array() {
    let p = LiveRoomRun::params_from_body(&serde_json::json!({
        "codes": ["甲", "乙"],
        "listenSeconds": 30
    }));
    assert_eq!(p.codes, vec!["甲".to_string(), "乙".to_string()]);
    assert_eq!(p.listen_seconds, 30);

    // 不给 listenSeconds ⇒ 默认 60（与参考实现一致）
    let p = LiveRoomRun::params_from_body(&serde_json::json!({"codes": "甲"}));
    assert_eq!(p.listen_seconds, 60);
}

/// ⭐ 演练模式：**一枪都不发**。
#[tokio::test]
async fn dry_run_sends_nothing() {
    let (c, http) = Ctx::new(&[]);
    let task = LiveRoomTask::production();
    let body = serde_json::json!({"codes": "甲\n乙", "dryRun": true});
    let out = task.run(&c.ctx(), &body).await.unwrap();

    assert_eq!(http.count(), 0, "演练不该发任何请求");
    assert!(out.is_success());
    assert!(out.summary().contains("演练"), "实得：{}", out.summary());
}

// ══════════════════════════ 判据 7：六步链状态机 ══════════════════════════

/// ⭐ `next` **不推进**状态 —— 同一份账重放不会漏投或重投。
#[test]
fn next_does_not_advance_the_state() {
    let s = RedFlowerSession::new(vec!["甲".into()]);
    let (a, r1) = s.next().unwrap();
    let (b, r2) = s.next().unwrap();
    assert_eq!(a, RedFlowerStep::BalanceBefore);
    assert_eq!(b, RedFlowerStep::BalanceBefore, "next 不推进");
    assert_eq!(r1.url, r2.url);
    assert_eq!(s.step(), RedFlowerStep::BalanceBefore);
}

/// ⭐ 埋点**恰好 5 条**，且第 1 条是 `_pv`、最后一条是实拍证实过的 `_flowerBtn`。
#[test]
fn journaling_sends_exactly_five_events_ending_with_flower_btn() {
    let mut s = RedFlowerSession::new(vec!["甲".into()]);
    s.feed(&balance_body(0)).unwrap();

    let mut bodies: Vec<String> = Vec::new();
    for _ in 0..proto::JOURNALING_EVENTS.len() {
        let (step, req) = s.next().unwrap();
        assert_eq!(step, RedFlowerStep::Journaling);
        bodies.push(req.body.clone().unwrap_or_default());
        s.feed(&journaling_ok()).unwrap();
    }
    assert_eq!(bodies.len(), 5, "埋点固定 5 条");
    assert_eq!(
        s.next().unwrap().0,
        RedFlowerStep::FirstParticipate,
        "5 条之后进③"
    );
    assert!(
        bodies
            .last()
            .unwrap()
            .contains("National_LiveRoomFeedback_flowerBtn"),
        "最后一条是实拍证实的那条：{:?}",
        bodies.last()
    );
    assert!(
        bodies[0].contains("National_LiveRoomFeedback_pv"),
        "第一条是 pv：{:?}",
        bodies[0]
    );
    // 埋点走的是 **form-urlencoded**（不是 JSON）
    let (_, req) = {
        let mut s2 = RedFlowerSession::new(vec!["甲".into()]);
        s2.feed(&balance_body(0)).unwrap();
        s2.next().unwrap()
    };
    assert!(
        req.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.contains("form-urlencoded")),
        "埋点是 form 体：{:?}",
        req.headers
    );
    assert!(
        req.body
            .as_deref()
            .unwrap_or_default()
            .starts_with("module=uservisit&"),
        "form 体的形状照抄原项目：{:?}",
        req.body
    );
}

/// ⭐ 埋点的**响应不解析**：回什么都不改变后面的步骤（回 5xx 也不失败）。
#[test]
fn journaling_response_is_never_parsed() {
    let mut s = RedFlowerSession::new(vec!["甲".into()]);
    s.feed(&balance_body(235)).unwrap();
    for _ in 0..proto::JOURNALING_EVENTS.len() {
        // 埋点回一个 5xx 信封 —— 它只是噪声，不该当门槛
        s.feed(r#"{"code":500,"msg":"系统繁忙"}"#).unwrap();
    }
    assert_eq!(
        s.step(),
        RedFlowerStep::FirstParticipate,
        "埋点坏不影响推进"
    );
}

/// ⭐ 口令为**空**时 ③ 之后直接跳 ⑤（不投任何口令，也不卡在 Draw）。
#[test]
fn empty_codes_skip_the_draw_step() {
    let mut s = RedFlowerSession::new(vec![]);
    s.feed(&balance_body(0)).unwrap();
    for _ in 0..proto::JOURNALING_EVENTS.len() {
        s.feed(&journaling_ok()).unwrap();
    }
    s.feed(&give_ok(0)).unwrap();
    assert_eq!(s.step(), RedFlowerStep::RevivalFlowers, "没有口令就直接进⑤");
}

/// ⭐ `draw` 走 **query string**（不是 body），且中文口令被百分号编码。
#[test]
fn draw_puts_the_code_in_the_query_string_encoded() {
    let mut s = RedFlowerSession::new(vec!["中秋口令".into()]);
    s.feed(&balance_body(0)).unwrap();
    for _ in 0..proto::JOURNALING_EVENTS.len() {
        s.feed(&journaling_ok()).unwrap();
    }
    s.feed(&give_ok(0)).unwrap();

    let (step, req) = s.next().unwrap();
    assert_eq!(step, RedFlowerStep::Draw);
    assert!(
        req.url.contains("/redFlower/draw?code="),
        "实得：{}",
        req.url
    );
    assert!(
        req.url.contains("%E4%B8%AD%E7%A7%8B"),
        "中文必须编码：{}",
        req.url
    );
    assert!(
        req.body.is_none(),
        "draw 的 body 是空的（实拍 content-length: 0）"
    );
}

/// ⭐ 步名的字符串是**稳定契约**（错误前缀与日志都按它指）。
#[test]
fn step_names_are_stable() {
    let names: Vec<&str> = [
        RedFlowerStep::BalanceBefore,
        RedFlowerStep::Journaling,
        RedFlowerStep::FirstParticipate,
        RedFlowerStep::Draw,
        RedFlowerStep::RevivalFlowers,
        RedFlowerStep::BalanceAfter,
        RedFlowerStep::Done,
    ]
    .iter()
    .map(|s| s.as_str())
    .collect();
    assert_eq!(
        names,
        vec![
            "user_info_before",
            "journaling",
            "first_participate",
            "draw",
            "revival_flowers",
            "user_info_after",
            "done"
        ]
    );
}

/// ⭐ 走完之后再问 `next` 是错误（不是静默回一个空请求）。
#[test]
fn next_after_done_is_an_error() {
    let mut s = RedFlowerSession::new(vec![]);
    s.feed(&balance_body(0)).unwrap();
    for _ in 0..proto::JOURNALING_EVENTS.len() {
        s.feed(&journaling_ok()).unwrap();
    }
    s.feed(&give_already()).unwrap();
    s.feed(&revival_body(0)).unwrap();
    s.feed(&balance_body(0)).unwrap();
    assert!(s.done());
    assert!(s.next().is_err(), "走完了就不该再有下一步");
}

// ══════════════════════════ 判据 8：两轮合并 ══════════════════════════

/// ⭐ 两轮合并：起始取第一轮、终值取第二轮、无效口令取**并集**。
#[test]
fn merge_keeps_first_start_second_end_and_the_union_of_invalids() {
    let first = RedFlowerReport {
        balance_before: balance(100),
        balance_after: balance(110),
        draws: vec![DrawRecord {
            code: "甲".into(),
            outcome: succeeded(5),
        }],
        invalid_codes: vec!["乙".into()],
        ..Default::default()
    };

    let second = RedFlowerReport {
        balance_before: balance(110),
        balance_after: balance(118),
        draws: vec![DrawRecord {
            code: "丙".into(),
            outcome: succeeded(8),
        }],
        ..Default::default()
    };

    let m = merge_reports(&first, &second);

    assert_eq!(m.balance_before.flower_cnt, 100, "起始取**第一轮**");
    assert_eq!(m.balance_after.flower_cnt, 118, "终值取**第二轮**");
    assert_eq!(m.gained(), 18, "两轮加起来进账 18");
    assert_eq!(m.draws.len(), 2, "第一次成功的不能因为第二轮而消失");
    assert_eq!(m.succeeded(), 2);
    assert_eq!(
        m.invalid_codes,
        vec!["乙".to_string()],
        "⭐ 无效口令取**并集**：取第二轮那份会让「为什么少了一朵」这件事实消失"
    );
}

// ══════════════════════════ 判据 9：任务元信息与 red-flower ══════════════════════════

/// ⭐ 两个任务的名字是 **wire 契约**（编排与定时都按它指），且元信息完整。
#[test]
fn task_metadata_is_complete_and_stable() {
    let metas = [
        LiveRoomTask::production().meta(),
        RedFlowerTask::production().meta(),
    ];
    let names: Vec<String> = metas.iter().map(|m| m.name.clone()).collect();
    assert_eq!(names, vec!["live-room", "red-flower"], "名字是 wire 契约");
    for m in &metas {
        assert!(!m.title.is_empty(), "{} 缺标题", m.name);
        assert!(!m.description.is_empty(), "{} 缺说明", m.name);
    }
    // ⚠ `codes` 不是可选项：小红书那条口令只能从这里进来
    assert!(
        LiveRoomTask::production()
            .params()
            .contains(&"codes".to_string()),
        "codes 必须是一个被声明的参数"
    );
}

/// ⭐ `red-flower` 没有 codes 时**报配置错误**（它不收集，没有别的来源）。
#[tokio::test]
async fn red_flower_without_codes_is_a_config_error() {
    let (c, http) = Ctx::new(&[]);
    let e = RedFlowerTask::production()
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("codes"), "要指出缺的是 codes：{e}");
    assert_eq!(http.count(), 0);
}

/// ⭐ `red-flower` 直接投：不发门那几枪，直接跑六步链。
#[tokio::test]
async fn red_flower_draws_directly_without_the_gate() {
    let (c, http) = ctx_with(&happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let out = RedFlowerTask::production()
        .run(&c.ctx(), &serde_json::json!({"codes": "中秋口令"}))
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert!(out.summary().contains("进账 10"), "实得：{}", out.summary());
    // 门那一族（/cmlive）一枪都不该有
    assert_eq!(http.count_of("/cmlive"), 0, "red-flower 不开门");
    assert_eq!(http.count_of("/redFlower/draw"), 1);
}

// ══════════════════════════ 判据 10：收集失败的影响面 ══════════════════════════

/// ⭐ 收集**两个来源都失败** ⇒ 整趟失败（不能静默变成"今天没收到口令"）。
#[tokio::test]
async fn collector_failure_propagates() {
    let (c, _) = Ctx::new(&[
        &tab_body(serde_json::json!([room_entry(147606, 1, "中国移动云盘")])),
        &tab_body(serde_json::json!([])),
    ]);
    let collector = FakeCollector::failing();
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let e = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap_err();
    // ⚠ 这一趟用的是**假收集器**，所以看到的是它自己的措辞；
    // 断言的是"收集失败**变成了 Err**"这件事（真实现的那句在
    // `LiveKoulingCollector::collect_inner` 里，见那里的单测）。
    assert!(
        e.to_string().contains("两个来源都失败了"),
        "两个来源都坏才算失败，而它必须是 Err：{e}"
    );
}

/// ⭐ 重听失败**不致命**：保留第一轮的结果（重听是补救措施）。
#[tokio::test]
async fn retry_collection_failure_keeps_the_first_round() {
    // 门开着（才有房间可听）+ 一轮全败的链
    let mut chain = vec![
        tab_body(serde_json::json!([room_entry(147606, 1, "中国移动云盘")])),
        tab_body(serde_json::json!([])),
    ];
    chain.extend(happy_chain(
        235,
        235,
        Some(give_already()),
        &[code_body(200108, "口令无效")],
        0,
    ));
    let (c, _) = ctx_with(&chain);
    // 手工 codes ⇒ 首轮不收集；重听**会**走收集 —— 而它失败了。
    let collector = FakeCollector::failing();
    let params = LiveRoomParams {
        codes: vec!["无效甲".into()],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert!(res.retried, "触发过重听");
    let report = res.report.as_ref().expect("第一轮的结果必须保留");
    assert_eq!(report.attempted(), 1, "第一轮投过 1 条");
    assert_eq!(report.invalid_codes, vec!["无效甲".to_string()]);
    // 重听失败 ⇒ 但结论仍是"投了全败"（不是 Err）
    assert_eq!(res.verdict(), RunVerdict::NoneSucceeded);
    let warns = c.sink.warns();
    assert!(
        warns.iter().any(|w| w.contains("重听失败")),
        "重听失败要留下痕迹：{warns:?}"
    );
}

// ── 小助手：直接造余额表与判定 ──────────────────────────────────────────

fn balance(n: i64) -> RedFlowerBalance {
    RedFlowerBalance {
        flower_cnt: n,
        ..Default::default()
    }
}

fn succeeded(flower_num: i64) -> RedFlowerDrawOutcome {
    proto::classify_red_flower_draw(&RedFlowerEnvelope::parse(&draw_ok(flower_num)))
}

// ══════════════════════════ 判据 11：register_all 接入 ══════════════════════════

/// ⭐ 两个任务真的**注册进了 registry** —— CLI 与 `/api/capabilities` 都由它驱动。
///
/// 这条是"接入"这件事的判据：光有任务类型不注册，用户在 CLI 里
/// `run live-room` 会拿到"未知任务"，而 `/api/capabilities` 里根本不出现。
#[test]
fn both_tasks_are_registered() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let names: Vec<String> = registry.list().iter().map(|m| m.name.clone()).collect();
    assert!(
        names.contains(&"live-room".to_string()),
        "live-room 必须注册（CLI/capabilities 都按 registry 走）：{names:?}"
    );
    assert!(
        names.contains(&"red-flower".to_string()),
        "red-flower 必须注册：{names:?}"
    );
    // 旧的签到族没被这次改动碰掉
    assert!(
        names.contains(&"sign".to_string()),
        "签到族仍应在：{names:?}"
    );
}

/// ⭐ 注册的名字**互不重复**（重名会让 `register` 报错，而不是静默覆盖）。
#[test]
fn registration_has_no_duplicate_names() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("重名会返回 Err");
    let mut names: Vec<String> = registry.list().iter().map(|m| m.name.clone()).collect();
    let before = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), before, "名字必须唯一");
}

// ── 最小替身 ────────────────────────────────────────────────────────────

/// 这批任务都不碰 store；给一个**必然报错**的替身。
///
/// 于是"哪天有人给直播族加了一句读 store"会立刻炸出来，
/// 而不是静默拿到一个空值、然后断言失败在一个很别扭的地方。
struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("直播族不该碰 store".into())
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

// ══════════════════════════ 口令拆分的分隔符 ══════════════════════════

/// 空白（含**全角空格**）必须算分隔符 —— 这是**修过的一个真缺陷**。
///
/// 早先只拆 `\n \r , ， 、`，于是 `"口令甲 口令乙"` 被当成**一条**口令发上去，
/// 服务端必然回 `200108 口令无效`；用户看到的是"我填了两条，怎么只投一条还说口令错"。
///
/// 漏掉它是因为 WebUI 在 `api.ts` 里自己先按空格拆了一遍（界面上看着是对的），
/// 而 CLI 与 HTTP API 两条路直接走本函数、没有那层补偿 ——
/// 所以这条测试的价值在于：**它钉的是所有入口共用的那一处**。
#[test]
fn whitespace_is_a_separator_including_ideographic_space() {
    assert_eq!(split_kouling("口令甲 口令乙"), vec!["口令甲", "口令乙"]);
    // 手机输入法默认全角空格，而口令是从手机里复制粘贴出来的 ⇒ 常见输入
    assert_eq!(
        split_kouling("口令甲\u{3000}口令乙"),
        vec!["口令甲", "口令乙"]
    );
    assert_eq!(split_kouling("甲\t乙"), vec!["甲", "乙"]);
    assert_eq!(split_kouling("甲\u{a0}乙"), vec!["甲", "乙"]);
    // 混用多种分隔符 + 连续空白（连续空白不该产出空口令）
    assert_eq!(
        split_kouling("甲,乙、丙 丁\n\n戊"),
        vec!["甲", "乙", "丙", "丁", "戊"]
    );
    assert_eq!(split_kouling("  甲   乙  "), vec!["甲", "乙"]);
}

/// 拆分**不拆口令内部**的字符：只有分隔符才拆。
///
/// 防的是"顺手按长度或按标点乱拆" —— 口令本身可能含数字、字母与标点，
/// 拆错了会投出一条不存在的口令（`200108`）。
#[test]
fn splitting_does_not_touch_characters_inside_a_code() {
    assert_eq!(split_kouling("一键9图出圈"), vec!["一键9图出圈"]);
    assert_eq!(split_kouling("中秋美好存云盘"), vec!["中秋美好存云盘"]);
    // 口令里的连字符/半角括号不是分隔符
    assert_eq!(split_kouling("A-B(C)1"), vec!["A-B(C)1"]);
}

/// 空输入与纯分隔符 ⇒ 空列表（不是"一个空口令"）。
#[test]
fn empty_or_separator_only_input_yields_no_codes() {
    assert!(split_kouling("").is_empty());
    assert!(split_kouling("   ").is_empty());
    assert!(split_kouling(" , 、\n").is_empty());
}

/// 拆分要**去重且保序**（与弹幕来源合并时的口径一致）。
#[test]
fn splitting_dedupes_and_keeps_order() {
    assert_eq!(split_kouling("甲 乙 甲 丙 乙"), vec!["甲", "乙", "丙"]);
}

/// `codes` 用**字符串数组**给时也走同一条拆分路径（两个入口不能漂移）。
///
/// `params_from_body` 同时收字符串与数组两种形态；这条钉的是
/// **数组里的每一项也按同一套分隔符处理** —— 否则"数组给一条含空格的口令"
/// 与"字符串给同样的内容"会得到不同结果。
#[test]
fn params_accept_codes_as_string_or_array_with_the_same_semantics() {
    let from_string = LiveRoomRun::params_from_body(&serde_json::json!({
        "codes": "甲 乙", "listenSeconds": 45
    }));
    let from_array = LiveRoomRun::params_from_body(&serde_json::json!({
        "codes": ["甲 乙"], "listenSeconds": 45
    }));
    assert_eq!(from_string.codes, vec!["甲", "乙"]);
    assert_eq!(from_array.codes, from_string.codes);
}
