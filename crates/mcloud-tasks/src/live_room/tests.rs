//! `live-room` 编排层测试（含它的「只用手工口令」模式 —— 原独立的 `red-flower` 已删）。
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

    /// 拿去领取的口令（从 `draw` 的 query 里取，已解码）。
    fn drawn_codes(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter(|r| r.url.contains("/redFlower/draw"))
            .map(|r| percent_decode(r.url.split("code=").nth(1).unwrap_or_default()))
            .collect()
    }
}

/// 极简的 `%XX` 解码（只为断言"领的是哪条口令"）。
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
        owner: None,
        enabled: true,
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
            details: mcloud_engine::DetailsRecorder::new(
                mcloud_engine::DetailsLedger::new(),
                mcloud_core::RunId(1),
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

/// 首轮**成功**、重听**失败**的收集器。
///
/// # 为什么不能直接用 [`FakeCollector::failing`]
///
/// `failing` 是"每次 collect 都失败"。用它测"重听失败不致命"时，
/// **首轮收集就已经失败了** ⇒ 一条口令都收不到 ⇒ 六步链走不到
/// `draw` ⇒ 也就**没有无效口令** ⇒ 根本不会触发重听。
/// 那条用例会变成"什么都没发生"，却照样通过（因为它只断言最终结论不是 Err）。
///
/// ⇒ 这个替身把两轮**分开**：第 1 次成功给出口令，第 2 次才失败。
#[derive(Default)]
struct RetryFailCollector {
    /// 第一次 `collect` 返回的口令。
    first: Vec<String>,
    /// 已经调用过几次。
    calls: Mutex<usize>,
}

impl RetryFailCollector {
    fn new(first: &[&str]) -> Self {
        Self {
            first: first.iter().map(|s| s.to_string()).collect(),
            calls: Mutex::new(0),
        }
    }
}

#[async_trait]
impl KoulingCollector for RetryFailCollector {
    async fn collect(
        &self,
        _ctx: &TaskContext,
        _jwt: &str,
        _room: &live::LiveRoom,
        _listen_for: Duration,
        _exclude: &[String],
    ) -> Result<SourceOutcome> {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        if *n == 1 {
            return Ok(SourceOutcome {
                codes: self.first.clone(),
                failure: None,
            });
        }
        Err(Error::Other("假收集器：重听时两个来源都失败了".into()))
    }
}

/// 记录"收集器收到的那串 JWT"的替身。
///
/// # 为什么需要它
///
/// `run_account` 早先用 `unwrap_or_default()` 取票 —— 忘了填就是**静默空串**。
/// 空串传给收集器之后，表现是"WS 连不上"，而真因是"票根本没传下来"，
/// 排查方向完全不同。这个替身把"实际传了什么"变成**可断言的事实**。
#[derive(Default)]
struct JwtRecordingCollector {
    jwts: Mutex<Vec<String>>,
}

impl JwtRecordingCollector {
    fn jwts(&self) -> Vec<String> {
        self.jwts.lock().unwrap().clone()
    }
}

#[async_trait]
impl KoulingCollector for JwtRecordingCollector {
    async fn collect(
        &self,
        _ctx: &TaskContext,
        jwt: &str,
        _room: &live::LiveRoom,
        _listen_for: Duration,
        _exclude: &[String],
    ) -> Result<SourceOutcome> {
        self.jwts.lock().unwrap().push(jwt.to_string());
        Ok(SourceOutcome::default())
    }
}

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

/// 造一个 `LiveRoomRun`。
///
/// ⚠ 这里**没有** `live_jwt` 参数了（该字段已从 `LiveRoomRun` 删掉）：
/// 直播侧 JWT 现在由 `LiveRoomRun::open_gate` 在需要时**自己换**。
/// 于是"从外面塞一张票"变成了一件**编译不过**的事 —— 而它此前正是
/// "两套票混用"最容易发生的入口。
fn run_with<'a>(collector: &'a dyn KoulingCollector) -> LiveRoomRun<'a> {
    LiveRoomRun { collector }
}

/// `run_with` 的**同义词**：门相关用例沿用这个名字读起来更顺。
///
/// ⚠ 已**没有**"外面塞一张 `live_jwt`"那种假开门了：现在门开着这件事
/// **只能**通过"假 HTTP 真的把①②两枪回了"来表达 ——
/// 于是每条门相关测试都**同时**在验证开门的请求序列，覆盖面比过去大。
fn run_gated<'a>(collector: &'a dyn KoulingCollector) -> LiveRoomRun<'a> {
    run_with(collector)
}

/// 一步到位的 `run_account`：省掉每条用例都写 `.run_account(&c.ctx(), &params)`。
///
/// ⚠ 出错时直接 `unwrap` —— 但**只用于"本该成功"的用例**。
/// 要断言错误的用例请显式写 `run_gated(..).run_account(..).await`，
/// 那样 `.expect_err(..)` 上的那句话才是断言的一部分。
async fn run_account_with(
    c: &Ctx,
    collector: &dyn KoulingCollector,
    params: &LiveRoomParams,
) -> LiveRoomAccountResult {
    run_gated(collector)
        .run_account(&c.ctx(), params)
        .await
        .expect("这一趟本该成功")
}

// ── 门的响应构造（开门那两步）────────────────────────────────────────────

/// ① `querySpecTokenV2` 的成功响应。
///
/// ⚠ 形状是**双层信封**且 `code` 是**字符串 `"0000"`**（不是数字）——
/// 协议层 `parse_sso_token` 刻意按字符串解（见那里的说明）。
/// 实测：`toSourceId=017027` ⇒ SSO token **42 字符**。
fn sso_body() -> String {
    serde_json::json!({
        "success": true,
        "code": "0000",
        "message": "成功",
        "data": {"token": "SSO-TOKEN-42CHARS-0123456789012345678901"}
    })
    .to_string()
}

/// ① 的**失败**响应（`success: false`）。
fn sso_fail_body() -> String {
    serde_json::json!({
        "success": false,
        "code": "1001",
        "message": "凭据无效"
    })
    .to_string()
}

/// ② `cmpassLoginByCmToken` 的成功响应。
///
/// ⚠ 第三套响应约定：`rtnCode` 是**字符串 `"0"`**，且 `object`
/// **就是那串 JWT 本身**（不是包着它的对象）—— 实测 180 字符。
fn cmpass_body(jwt: &str) -> String {
    serde_json::json!({
        "rtnMsg": null, "rtnCode": "0", "object": jwt
    })
    .to_string()
}

/// ② 的**失败**响应（`rtnCode != "0"`）。
fn cmpass_fail_body() -> String {
    serde_json::json!({
        "rtnMsg": "登录失败", "rtnCode": "1002", "object": null
    })
    .to_string()
}

/// 测试里用的直播侧 JWT。**刻意取一个不可能与云盘侧票混淆的值**。
const TEST_LIVE_JWT: &str = "LIVE-SIDE-JWT-not-a-jwttoken";

/// 🔑 **开门**的三枪：① SSO → ② 换 JWT → ③④ 两个页签。
///
/// 前两枪固定，后两枪由 `rooms` 决定（第一个页签给房间、第二个给空）。
fn open_gate_chain(rooms: serde_json::Value) -> Vec<String> {
    vec![
        sso_body(),
        cmpass_body(TEST_LIVE_JWT),
        tab_body(rooms),
        tab_body(serde_json::json!([])),
    ]
}

/// 关掉重听那一支的参数（专测"领了之后"的结论）。
fn no_retry_params(codes: &[&str]) -> LiveRoomParams {
    LiveRoomParams {
        codes: codes.iter().map(|s| s.to_string()).collect(),
        listen_seconds: 60,
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

/// ⭐ **验出两条有效之后** ⇒ 不再继续听（排除表都不该被构造出来）。
///
/// ⚠ 判据 2026-10-01 换过：从前看"有没有口令被判无效"，现在看
/// **池里（含这一趟刚验出来的）有效够不够两条** —— 用户口径"要的是两条有效的"。
/// 这条用例钉的是新判据的"够了就收工"那一半。
#[test]
fn retry_plan_stops_once_two_valid_codes_exist() {
    let (retry, exclude) = retry_plan(&["甲".into(), "乙".into()], &[], 2);
    assert!(!retry, "两条有效 ⇒ 收工，别再占着弹幕窗口");
    assert!(exclude.is_empty());
}

/// ⭐ 反过来：**不够两条有效就要继续听** —— 哪怕一个无效口令都没有。
///
/// 这正是老判据漏掉的情形：收到一条没分类的码、或只验出一条有效，
/// 老实现从"没有无效口令"直接就结束了，而那恰恰是最该继续听下去的时候。
#[test]
fn retry_plan_keeps_listening_while_fewer_than_two_are_valid() {
    let (retry, exclude) = retry_plan(&["说不清的".into()], &[], 0);
    assert!(retry, "一条有效都没有 ⇒ 继续听");
    assert_eq!(exclude, vec!["说不清的".to_string()], "试过的要排除");

    let (retry, _) = retry_plan(&["甲".into()], &[], 1);
    assert!(retry, "只有一条有效 ⇒ 继续听，去凑第二条");
}

/// ⭐ 重听时的排除表 = 第一批 ∪ 无效表。
///
/// ⚠ 关键：**第一次就成功的口令也必须进排除表** —— 不排除的话第二轮会把
/// 同一批再领一遍，而同一账号对同一条**正确**口令的第二次请求回 `200112`，
/// 会污染归因、看起来像"口令坏了"。
///
/// ⚠⚠ 这个理由**不是**"口令会被消耗"（口令全场通用）。本测试的注释里也刻意
/// 不出现"消耗"二字 —— 那个概念会把人引向错误的方向。
#[test]
fn retry_plan_excludes_both_the_successful_and_the_invalid() {
    let first: Vec<String> = vec!["成功甲".into(), "无效乙".into()];
    let invalid: Vec<String> = vec!["无效乙".into()];
    // `0` = 池里还没有有效口令 ⇒ 该继续听（这一条要验的是**排除表**）。
    let (retry, exclude) = retry_plan(&first, &invalid, 0);

    assert!(retry, "一条有效都还没有 ⇒ 继续听");
    assert!(exclude.contains(&"无效乙".to_string()), "无效的要排除");
    assert!(
        exclude.contains(&"成功甲".to_string()),
        "⭐ **第一次就成功的**也必须排除 —— 否则第二轮会重领它对，回 200112"
    );
}

/// ⭐ `pick_new_codes`：只挑既不在第一批、也不在无效表里的。
#[test]
fn pick_new_codes_filters_out_everything_already_tried() {
    let retried: Vec<String> = vec!["新丙".into(), "成功甲".into(), "无效乙".into()];
    let first: Vec<String> = vec!["成功甲".into(), "无效乙".into()];
    let invalid: Vec<String> = vec!["无效乙".into()];

    let to_draw = pick_new_codes(&retried, &first, &invalid);
    assert_eq!(to_draw, vec!["新丙".to_string()], "只该领真正的新口令");
}

/// ⭐⭐ 重试路径端到端：无效口令进排除表、**成功的不进**、不重复领取。
///
/// 第一轮领「甲（成功）+ 乙（200108）」；重听又听到「乙 + 丙」，
/// 而排除表必须把乙挡住 ⇒ 第二轮**只领丙**。
///
/// # ⚠ 这一趟必须**不用手工 codes**（改了）
///
/// 「有 codes ⇒ 跳过开门」那个取舍一旦生效，**手工 codes 那一支就没有房间**
/// ⇒ 永远走不到重听。所以这条用例的口令要**从收集器来**（第 1 次收「甲、乙」），
/// 这样门是**真的开过**的、`gate.rooms` 非空 ⇒ 重听才有地方听。
/// 排除表的判据完全一样，覆盖面没少。
#[tokio::test]
async fn invalid_code_triggers_one_retry_and_never_redraws_tried_codes() {
    // 开门那几枪 —— **重听需要一间可听的房间**，没开成门就没有。
    let mut chain = open_gate_chain(serde_json::json!([room_entry(147606, 1, "中国移动云盘")]));
    // 第一轮：甲成功、乙无效
    chain.push(balance_body(10));
    push_journaling(&mut chain);
    chain.push(give_already());
    chain.push(draw_ok(5)); // 甲成功
    chain.push(code_body(200108, "口令无效")); // 乙无效
    chain.push(revival_body(0));
    chain.push(balance_body(15)); // 第一轮终值
                                  // 第二轮（只领丙）
    chain.extend(happy_chain(15, 18, Some(give_already()), &[draw_ok(3)], 0));
    let (c, http) = ctx_with(&chain);

    // 第 1 次收集给「甲、乙」（首轮领这两个）；第 2 次（重听）给「乙、丙」。
    let collector = FakeCollector::new(&[&["甲", "乙"], &["乙", "丙"]]);
    // ⚠ 不给手工 codes：那一支会跳过开门，于是没有房间 ⇒ 重听根本不会发生。
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert!(res.retried, "有无效口令 ⇒ 触发过重听");
    assert_eq!(collector.call_count(), 2, "首轮收集 1 次 + 重听 1 次");

    // ⭐ 排除表必须含"甲（成功）"与"乙（无效）"两者
    let exclude = collector.exclude_at(1);
    assert!(
        exclude.contains(&"甲".to_string()),
        "成功过的也要排除：{exclude:?}"
    );
    assert!(
        exclude.contains(&"乙".to_string()),
        "无效的要排除：{exclude:?}"
    );

    // ⭐ 领取序列：甲 → 乙 → 丙，**乙只领一次**（第二轮被排除表挡住）
    let drawn = http.drawn_codes();
    assert_eq!(
        drawn,
        vec!["甲".to_string(), "乙".to_string(), "丙".to_string()],
        "不重复领取已试过的"
    );
    assert_eq!(
        drawn.iter().filter(|c| *c == "乙").count(),
        1,
        "无效口令只领一次"
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

/// ⭐ 领了、**一个都没成** ⇒ 结论必须是**失败**（不是"跳过"，也不是"成功"）。
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
    assert!(out.is_failure(), "领了全败必须报失败：{out:?}");
    // 失败小结里必须带**原因**，不能只剩光秃秃的"失败"
    assert!(
        out.summary().contains("无效口令"),
        "失败要说清为什么：{}",
        out.summary()
    );
}

/// ⭐ 领了、**成了部分** ⇒ 部分成功（不是失败）。
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
    // 开门那两枪（①SSO ②JWT）+ 两个页签都回空房间 ⇒ live_count == 0
    let (c, http) = ctx_with(&open_gate_chain(serde_json::json!([])));
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
    let (c_a, _) = ctx_with(&open_gate_chain(serde_json::json!([])));
    let collector_a = FakeCollector::empty();
    let res_a = run_gated(&collector_a)
        .run_account(&c_a.ctx(), &params)
        .await
        .unwrap();

    // 情形 B：有房间在播，但主播不是「中国移动云盘」（实拍 147490「福建移动」）
    let live = serde_json::json!([room_entry(147490, 1, "福建移动")]);
    let (c_b, _) = ctx_with(&open_gate_chain(live));
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
        let (c, _) = ctx_with(&open_gate_chain(rooms));
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
        let (c, _) = ctx_with(&open_gate_chain(rooms));
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

/// ⭐ 门开了但没收到口令 ⇒ **第三种**说法（既不是"没直播"，也不是"有别的间在播"）。
#[tokio::test]
async fn opened_gate_with_no_codes_says_so() {
    let (c, http) = ctx_with(&open_gate_chain(serde_json::json!([room_entry(
        147606,
        1,
        "中国移动云盘"
    )])));
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
        "这是第三种「没领」，措辞不能与另两种混"
    );
}

/// ⭐ 房间数 > 1 ⇒ **必须 warn**（代价告知：只听第一间，其余的整场漏掉）。
#[tokio::test]
async fn multiple_live_rooms_produce_a_warning() {
    let rooms = serde_json::json!([
        room_entry(147606, 1, "中国移动云盘"),
        room_entry(147608, 1, "中国移动云盘"),
    ]);
    // 开门三枪 + 一轮完整的六步链（收集到的口令会被真的拿去领）
    let mut chain = open_gate_chain(rooms);
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
    // ⚠ 两次都打在**同一间**（`gate.rooms.first()`）：这条用例钉的是"只听第一间"，
    // 而重听那一轮用的也是这一间 ⇒ 断言次数是 2 不改变它的意思。
    // （2 而不是 1 的理由同下一条用例：第一批只收到 1 条 ⇒ 有效不足 2 ⇒ 再听一次。）
    assert_eq!(
        collector.call_count(),
        2,
        "只听**第一间**（刻意的取舍）；两次都打在那一间"
    );
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
    let mut chain = open_gate_chain(rooms);
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

    // ③ 关掉唯一来源又没给手工口令
    let no_codes = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        skip_kouling: true,
    };
    assert_eq!(no_codes.check(), Err(LiveRoomFault::SkipWithoutCodes));

    // ④ 时长非正
    let zero = LiveRoomParams {
        codes: vec!["甲".into()],
        listen_seconds: 0,
        skip_kouling: true,
    };
    assert_eq!(zero.check(), Err(LiveRoomFault::ListenNotPositive));

    // ⚠ 0 秒 + 关掉弹幕来源 = **仍然非法**（时长那一条不看 skip_kouling）
    let zero_only = LiveRoomParams {
        codes: vec![],
        listen_seconds: 0,
        skip_kouling: false,
    };
    assert_eq!(
        zero_only.check(),
        Err(LiveRoomFault::ListenNotPositive),
        "时长非正在任何情况下都非法"
    );

    // 三条规则的中性描述都非空（措辞由调用方写，但领域层得说得出自己是什么）
    for f in [
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
    // ⚠ 这一条同时是"**没有账号级开关**"的回归证明：`run()` 直接进参数校验
    //（2026-10-03 之前这里得先 `kouling_enabled = true` 才走得到）。
    let task = LiveRoomTask::production();

    // 关掉了唯一的弹幕来源，却没给手工口令
    let body = serde_json::json!({
        "codes": "", "listenSeconds": 60,
        "skipKouling": true
    });
    let e = task.run(&c.ctx(), &body).await.unwrap_err();
    assert_eq!(http.count(), 0, "参数不成立就不该发任何请求");
    let text = e.to_string();
    assert!(
        text.contains("skipKouling") && text.contains("codes"),
        "要给出可操作的措辞（哪个 flag 该改）：{text}"
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

    // 不给 listenSeconds ⇒ 用缺省值（用户口径 2026-10-08「默认180」）
    let p = LiveRoomRun::params_from_body(&serde_json::json!({"codes": "甲"}));
    assert_eq!(p.listen_seconds, DEFAULT_LISTEN_SECONDS);
    assert_eq!(
        DEFAULT_LISTEN_SECONDS, 180,
        "缺省值本身也要钉住，别只对着常量自证"
    );
}

// ⚠ 这里**删掉过**一条 ⭐ 用例：`disabled_kouling_is_skipped_with_zero_requests`
// （`kouling_enabled = false` ⇒ 一个请求都不发、结论 `Skipped`）。
//
// 2026-10-03 用户口径「**账号不应该有这个配置**」⇒ `AccountSettings::kouling_enabled`
// 整个字段已删，那道门也不在了 ⇒ 这条用例的**被测行为不存在**了，留着就是钉一个幻觉。
//
// 它从前还配着一条"反面用例"（`enabled = true` ⇒ 照旧跑），那条早先就因为 `dryRun`
// 演练档被删而失去了立足点 —— 而它想证明的"打开就真跑"，今天由
// `auto_chain_opens_gate_collects_kouling_then_earns_flowers`（开门 + 完整六步链）
// 与 `task_surfaces_invalid_params_as_an_error`（`run()` 直接进参数校验）覆盖着。

// ══════════════════════════ 判据 7：六步链状态机 ══════════════════════════

/// ⭐ `next` **不推进**状态 —— 同一份账重放不会漏领或重领。
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
        bodies.push(req.body_str().unwrap_or_default().to_string());
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
        req.body_str()
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

/// ⭐ 口令为**空**时 ③ 之后直接跳 ⑤（不领任何口令，也不卡在 Draw）。
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

// ══════════════════════════ 判据 9：任务元信息 ══════════════════════════

/// ⭐ 名字是 **wire 契约**（编排与定时都按它指），且元信息完整。
///
/// ⚠ 2026-10-05 用户口径：「小红花弄两个干嘛，输入口令并领取有单独的页面，做定时任务你口令
/// 都没有也不行」⇒ 独立的 `red-flower` 任务**删掉**了：它就是本任务「只用手工口令」
/// （`skipKouling` + `codes`）那一支，而那一支在直播页上本来就有开关 ✓。所以这里只剩
/// 一个名字 —— 它仍是 wire 契约，**别改名** ✗。
#[test]
fn task_metadata_is_complete_and_stable() {
    let metas = [LiveRoomTask::production().meta()];
    let names: Vec<String> = metas.iter().map(|m| m.name.clone()).collect();
    assert_eq!(names, vec!["live-room"], "名字是 wire 契约");
    for m in &metas {
        assert!(!m.title.is_empty(), "{} 缺标题", m.name);
        assert!(!m.description.is_empty(), "{} 缺说明", m.name);
    }
    // ⚠ `codes` 不是可选项：小红书那条口令只能从这里进来
    assert!(
        LiveRoomTask::production()
            .params()
            .iter()
            .any(|p| p.name == "codes"),
        "codes 必须是一个被声明的参数"
    );
}

/// ⭐ 「只用手工口令」但一条口令都没给 ⇒ **报配置错误**（关了唯一的口令来源，没有别的来源）。
///
/// 这一条原来跑在独立的 `red-flower` 任务上；那个任务删掉之后，覆盖面**原样搬到**
/// `live-room` 的这个模式上（它俩本来就是同一支）✓。
#[tokio::test]
async fn skip_kouling_without_codes_is_a_config_error() {
    let (c, http) = Ctx::new(&[]);
    let e = LiveRoomTask::production()
        .run(&c.ctx(), &serde_json::json!({"skipKouling": true}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("codes"), "要指出缺的是 codes：{e}");
    assert_eq!(http.count(), 0);
}

/// ⭐ 「只用手工口令」直接领取：不发门那几枪，直接跑六步链。
#[tokio::test]
async fn skip_kouling_draws_directly_without_the_gate() {
    let (c, http) = ctx_with(&happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let out = LiveRoomTask::production()
        .run(
            &c.ctx(),
            &serde_json::json!({"codes": "中秋口令", "skipKouling": true}),
        )
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert!(out.summary().contains("进账 10"), "实得：{}", out.summary());
    // 门那一族（/cmlive）一枪都不该有
    assert_eq!(http.count_of("/cmlive"), 0, "只用手工口令时不开门");
    assert_eq!(http.count_of("/redFlower/draw"), 1);
}

// ══════════════════════════ 判据 10：收集失败的影响面 ══════════════════════════

/// ⭐ 收集**两个来源都失败** ⇒ 整趟失败（不能静默变成"今天没收到口令"）。
#[tokio::test]
async fn collector_failure_propagates() {
    let (c, _) = ctx_with(&open_gate_chain(serde_json::json!([room_entry(
        147606,
        1,
        "中国移动云盘"
    )])));
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
///
/// # ⚠ 这一趟得**先开成门**，否则根本走不到重听
///
/// 「有 codes ⇒ 跳过开门」那个取舍意味着：**手工 codes 那一支没有房间**，
/// 于是它连重听的机会都没有（`gate.rooms` 是空的）。
/// 所以这条用例必须是"**门开着收来的**口令被判无效 ⇒ 重听 ⇒ 重听失败"，
/// 而不是"手工 codes + 重听失败"—— 后者在新语义下**永远不会发生重听**。
#[tokio::test]
async fn retry_collection_failure_keeps_the_first_round() {
    // 门开着（才有房间可听）+ 一轮全败的链
    let mut chain = open_gate_chain(serde_json::json!([room_entry(147606, 1, "中国移动云盘")]));
    chain.extend(happy_chain(
        235,
        235,
        Some(give_already()),
        &[code_body(200108, "口令无效")],
        0,
    ));
    let (c, _) = ctx_with(&chain);
    // 首轮收集**成功**（拿到一条口令），重听**失败** —— 两步必须能区分开，
    // 否则这条用例测的其实是"首轮收集就失败了"，与它的名字不符。
    let collector = RetryFailCollector::new(&["无效甲"]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    assert!(res.retried, "触发过重听");
    let report = res.report.as_ref().expect("第一轮的结果必须保留");
    assert_eq!(report.attempted(), 1, "第一轮领过 1 条");
    assert_eq!(report.invalid_codes, vec!["无效甲".to_string()]);
    // 重听失败 ⇒ 但结论仍是"领了全败"（不是 Err）
    assert_eq!(res.verdict(), RunVerdict::NoneSucceeded);
    let warns = c.sink.warns();
    assert!(
        // ⚠ 措辞随「循环」改过：原来是「重听失败」，现在是「再听失败」。
        // 断言的是**判据本身**，别只改提示文案 ✗（第一次就是这么漏的）。
        warns.iter().any(|w| w.contains("再听失败")),
        "再听失败要留下痕迹（措辞随「循环」改过：原来叫「重听失败」）：{warns:?}"
    );
}

// ══════════════════════════ 判据 12：⭐ 有 codes 时不开门 ══════════════════════════

/// ⭐⭐ 「有 codes ⇒ 跳过开门」那个取舍的**判据**。
///
/// # 这条测试存在的理由
///
/// 顺序错了就看不出来：若实现写成"**先开门再看 codes**"，那么
/// 门那两枪的偶发失败会把一趟**本来能跑完**的领取整体拖黄 ——
/// 而用户给的口令是对的、六步链是好的，他却一朵花没拿到，
/// 归因方向会指向"口令错了"（**完全错**）。
///
/// ⇒ 断言的是**请求序列里根本没有门的痕迹**。
#[tokio::test]
async fn manual_codes_skip_the_gate_entirely() {
    // ⚠ 只给六步链的响应：**故意不给**门那两枪的响应。
    // 如果实现真的去开门，它会拿到错位的响应而**必然**解析失败 ——
    // 于是"跳过开门"这件事被钉在**行为**上，而不只是钉在计数器上。
    let (c, http) = ctx_with(&happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let collector = FakeCollector::empty();
    let params = LiveRoomParams {
        codes: vec!["手工口令".into()],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .expect("有 codes 就不该开门，于是也不该因为开门失败而整趟黄掉");

    // ⭐ 门的**两个**请求一个都不能有。
    assert_eq!(
        http.count_of("/querySpecTokenV2"),
        0,
        "有 codes 时不该换 SSO（省两个请求 + 不让门失败拖累领取）"
    );
    assert_eq!(
        http.count_of("/cmpassLoginByCmToken"),
        0,
        "有 codes 时不该换直播侧 JWT"
    );
    assert_eq!(
        http.count_of("/cmlive"),
        0,
        "有 codes 时一个 /cmlive 都不该有"
    );
    assert!(!res.has_jwt, "没开门 ⇒ 没有直播侧票");
    assert!(res.gate.rooms.is_empty(), "没开门 ⇒ 没有房间");

    // 但六步链照跑，且真的进账了。
    assert_eq!(res.report.as_ref().unwrap().gained(), 10);
    assert_eq!(res.verdict(), RunVerdict::AllSucceeded);
    assert_eq!(http.count_of("/redFlower/draw"), 1);
    assert_eq!(collector.call_count(), 0, "手工 codes ⇒ 首轮不收集");
}

/// ⭐ 手工 codes 且**门那一族拿到的响应是坏的**时，领取照常成功。
///
/// 上一条的**强化版**：让门若真被打出去，拿到的会是明确的失败响应
/// （六步链的尾巴 `balance_body` ⇒ 解析必然失败），
/// 用来钉住"门不会因为 codes 存在而被打，也就不会被它的坏响应传染"。
#[tokio::test]
async fn manual_codes_deliver_even_when_the_gate_would_fail() {
    let (c, http) = ctx_with(&happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let collector = FakeCollector::empty();
    let params = LiveRoomParams {
        codes: vec!["手工口令".into()],
        listen_seconds: 60,
        ..Default::default()
    };

    let out = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .expect("门不会被走到 ⇒ 它的坏响应也不该被消费");

    assert!(out.outcome().is_success(), "{:?}", out.outcome());
    assert_eq!(http.count_of("/cmlive"), 0);
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

// ══════════════════════════ 判据 13：🌸 开门那条链（G2 的核心）══════════════════════════

/// ⭐⭐⭐ **完整链：开门 → 自动收到口令 → 领取 → 进账。**
///
/// 这是任务的原话要求「**小红花自动获取口令，并使用口令领取小红花**」，
/// 也是 G2 存在的理由。用户**不填任何 codes**，整条链必须自己跑通：
///
/// ```text
/// ① querySpecTokenV2(017027)  → SSO
/// ② cmpassLoginByCmToken      → 直播侧 JWT
/// ③④ tabContentList ×2        → 房间（147606 在播，主播「中国移动云盘」）
/// ⑤ WS 收集（用注入的假收集器）→ 口令
/// ⑥ 六步链：余额 235 → … → 245
/// ```
///
/// ⚠ WS 那一环用 `FakeCollector`（`KoulingCollector` trait 就是为此留的接缝）——
/// 真 WS **结构性抓不到**（见模块头证据等级），所以这里只验**编排**：
/// "门开了之后，收集到的口令会被真的拿去领"。
#[tokio::test]
async fn auto_chain_opens_gate_collects_kouling_then_earns_flowers() {
    // 门的前四枪 + 六步链
    let rooms = serde_json::json!([room_entry(147606, 1, "中国移动云盘")]);
    let mut chain = open_gate_chain(rooms);
    chain.extend(happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let (c, http) = ctx_with(&chain);

    // 用户**没有**填 codes ⇒ 收集器必须被真的用到
    let collector = FakeCollector::new(&[&["自动收集到的口令"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let res = run_account_with(&c, &collector, &params).await;

    // ── ① 门真的被开了 ────────────────────────────────────────────────
    assert!(res.has_jwt, "⭐ 门开成了 ⇒ 拿到了直播侧凭证");
    assert!(res.gate.active, "门开着");
    assert_eq!(res.gate.rooms.len(), 1);
    assert_eq!(res.gate.live_count, 1);

    // ── ② 口令是**自动收来的**，不是用户给的 ────────────────────────────
    // ⚠ **两次**是对的，不是多收了一次（2026-10-01 换判据之后）：
    // 第一批只收到 **1 条**口令 ⇒ 验出 1 条有效 < 2 ⇒ 按新判据"要的是两条有效的"
    // 再听一次去凑第二条。第二次那个假收集器又给同一条口令，而它已在排除表里
    // ⇒ `pick_new_codes` 过滤掉 ⇒ **不会**重复领取（所以下面的 `res.codes` 仍是一条）。
    assert_eq!(
        collector.call_count(),
        2,
        "第一批只验出 1 条有效 ⇒ 按新判据再听一次（要凑够两条有效）"
    );
    assert_eq!(res.codes, vec!["自动收集到的口令".to_string()]);

    // ── ③ 收来的口令**真的拿去领了**，且进了账 ──────────────────────────
    let report = res.report.as_ref().expect("应当有报告");
    assert_eq!(report.balance_before.flower_cnt, 235);
    assert_eq!(report.balance_after.flower_cnt, 245);
    assert_eq!(report.gained(), 10, "⭐ 进账 = ⑥ − ①");
    assert_eq!(report.succeeded(), 1);
    assert_eq!(res.verdict(), RunVerdict::AllSucceeded);
    assert!(res.outcome().is_success(), "{:?}", res.outcome());
    assert_eq!(http.count_of("/redFlower/draw"), 1);

    // ── ④ 请求序列：门在前、六步链在后 ─────────────────────────────────
    let urls: Vec<String> = http.requests().iter().map(|r| r.url.clone()).collect();
    let idx = |needle: &str| urls.iter().position(|u| u.contains(needle)).unwrap();
    assert!(
        idx("/querySpecTokenV2") < idx("/cmpassLoginByCmToken"),
        "① 在 ② 之前"
    );
    assert!(
        idx("/cmpassLoginByCmToken") < idx("/tabContentList"),
        "② 在 ③ 之前（第③步要拿 ② 换到的 JWT）"
    );
    assert!(
        idx("/tabContentList") < idx("/liveRoomFeedback/user/info"),
        "门在六步链之前"
    );
}

/// ⭐⭐ 开门那一步的 `toSourceId` **必须是 `017027`**（直播侧）。
///
/// # 用错的后果是**一条房间都筛不出来**，而那看起来像"今天没有直播"
///
/// 两者都叫 `targetSourceId`：签到/小红花侧是 `001005`，直播侧是 `017027`。
/// 混用**不会报错** —— 只是筛不出东西，于是结论说"今天没有直播"：
/// 一个**假结论**（参考实现坑表 #106；抓包文档第 494–495 行的原文纠正）。
///
/// ⚠ 所以这条断言的是**发出去的请求体**，而不只是常量相等 ——
/// 常量对但拼装时用错（比如顺手复用了签到页配方）照样会被抓住。
#[tokio::test]
async fn gate_opens_with_the_live_source_id_not_the_signin_one() {
    let rooms = serde_json::json!([room_entry(147606, 1, "中国移动云盘")]);
    // 开门四枪 + 一条完整的六步链：这条用例让链**跑完**，
    // 所以响应脚本也得给全（否则失败的是六步链，掩盖了要断言的门）。
    let mut chain = open_gate_chain(rooms);
    chain.extend(happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let (c, http) = ctx_with(&chain);
    let collector = FakeCollector::new(&[&["口令"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    // 找到第①枪，断言它的 body。
    let first = http
        .requests()
        .into_iter()
        .find(|r| r.url.contains("/querySpecTokenV2"))
        .expect("门必须打 querySpecTokenV2");
    let body: serde_json::Value =
        serde_json::from_str(first.body_str().expect("第①步是 JSON 体")).unwrap();

    assert_eq!(
        body["toSourceId"], "017027",
        "⭐ 门必须用**直播侧**的 toSourceId（不是签到的 001005）"
    );
    assert_ne!(
        body["toSourceId"], "001005",
        "001005 是小火花/签到侧的值 —— 用它门会一条房间都筛不出来"
    );
    // 第①步**只带这一个键**（门不建 H5 会话）
    assert_eq!(
        body.as_object().unwrap().len(),
        1,
        "门的第①步只有 toSourceId，实得：{body}"
    );
}

/// ⭐⭐ **开门失败 ⇒ 如实报错**，绝不能报"今天没有直播"。
///
/// # 这条测试拦的是一个**谎言**
///
/// 「今天没开播」与「我们的凭据坏了」是**两件完全不同的事**，
/// 而它们的下一步也不同（一个是"等明天"，一个是"去修票"）。
/// 参考实现早先在 SSO/登录失败时 `return true` 放行，
/// 它自己后来明确否定了那条 —— 它让"凭据坏了"表现成
/// "门开着、却一条口令也听不到"，真因被盖住，排查方向从
/// "今天是不是没开播"开始，**完全错**。
///
/// ⇒ 这里逐个覆盖开门两枪各自的失败，断言的都是 **Err**（不是成功的 idle）。
#[tokio::test]
async fn gate_failure_is_reported_honestly_never_as_no_live_today() {
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    // ── 情形 A：第①步（换 SSO）失败 ─────────────────────────────────────
    let (c, _) = ctx_with(&[sso_fail_body()]);
    let e = run_gated(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .expect_err("⭐ 换 SSO 失败必须上抛，绝不能变成一句'今天没有直播'");
    let text = e.to_string();
    assert!(
        text.contains("[门·换 SSO token]"),
        "错误要带步名前缀，好让人知道崩在哪一步：{text}"
    );
    assert!(
        !text.contains("今天没有直播"),
        "⭐⭐ '凭据坏了'绝不能报成'今天没开播' —— 那是撒谎：{text}"
    );

    // ── 情形 B：第②步（换直播侧 JWT）失败 ───────────────────────────────
    let (c, _) = ctx_with(&[sso_body(), cmpass_fail_body()]);
    let e = run_gated(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .expect_err("⭐ 换 JWT 失败必须上抛");
    let text = e.to_string();
    assert!(
        text.contains("[门·换直播侧 JWT]"),
        "错误要带步名前缀：{text}"
    );
    assert!(
        !text.contains("今天没有直播"),
        "⭐⭐ 同上，不能撒谎：{text}"
    );

    // ── 情形 C：第①步的响应不是信封（解析失败）也要如实报 ────────────────
    let (c, _) = ctx_with(&["<html>网关错误</html>".to_string()]);
    let e = run_gated(&FakeCollector::empty())
        .run_account(&c.ctx(), &params)
        .await
        .expect_err("⭐ 解析失败必须上抛");
    assert!(
        e.to_string().contains("[门·换 SSO token]"),
        "解析失败也要带步名：{e}"
    );
}

/// ⭐ **开门失败时绝不退回"放行"**：收集器一次都不该被调到。
///
/// 与上一条配对：上一条钉的是"报了 Err"，这条钉的是"没有偷偷继续跑"。
/// 参考实现那条被否定的做法（`return true` 放行）的表现正是
/// "崩了但继续"—— 于是收集器被调用、门却是空的。
#[tokio::test]
async fn gate_failure_does_not_fall_back_to_running_the_chain() {
    let (c, http) = ctx_with(&[sso_fail_body()]);
    let collector = FakeCollector::new(&[&["不该被收集"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    let _ = run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .expect_err("必须 Err");

    assert_eq!(collector.call_count(), 0, "⭐ 门没开成 ⇒ 一次都不该去收集");
    assert_eq!(
        http.count_of("/liveRoomFeedback"),
        0,
        "更不该打小红花六步链"
    );
    assert_eq!(http.count_of("/redFlower/draw"), 0, "一个口令都不该拿去领");
}

/// ⭐⭐ **两套凭据绝不能混用**：直播侧 JWT 只在 `/cmlive`，云盘侧票只在 `/ycloud`。
///
/// # 这条测试拦的是实测过的 `90001 未登录`
///
/// 参考实现记录过"把**直播侧** JWT 传进**云盘侧**参数位" ⇒ 实测回
/// **`90001 未登录`**（2026-09-22，两个账号都是那一句）。
///
/// 两套票**不是靠长相区分的**（都是长字符串），而是靠**域**：
///
/// | 票 | 挂在哪个头 | 域 |
/// |---|---|---|
/// | 直播侧 JWT | **`Authorization`** | `/cmlive` 与弹幕 WS |
/// | 云盘侧 `jwttoken` | **`jwttoken`** | `/ycloud` 六步链 |
///
/// ⇒ 断言两件事：**(a)** 直播侧 JWT 只出现在 `/cmlive` 的请求上且只在
/// `Authorization` 头里；**(b)** `/ycloud` 的请求上**从不**出现它。
#[tokio::test]
async fn the_two_credentials_never_cross_domains() {
    let rooms = serde_json::json!([room_entry(147606, 1, "中国移动云盘")]);
    let mut chain = open_gate_chain(rooms);
    chain.extend(happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let (c, http) = ctx_with(&chain);
    let collector = FakeCollector::new(&[&["口令"]]);
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    let reqs = http.requests();
    assert!(!reqs.is_empty());

    let header = |r: &HttpRequest, name: &str| -> Option<String> {
        r.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };

    let mut cmlive_with_jwt = 0usize;
    for r in &reqs {
        let on_cmlive = r.url.contains("/cmlive");
        let on_ycloud = r.url.contains("/ycloud") || r.url.contains("m.mcloud.139.com");
        let on_gate_step1 = r.url.contains("/querySpecTokenV2");

        // ── (a) 直播侧 JWT：只在 /cmlive 上，且只走 Authorization 头 ──────
        if header(r, "Authorization").as_deref() == Some(TEST_LIVE_JWT) {
            assert!(
                on_cmlive,
                "⭐⭐ 直播侧 JWT 出现在了**非 /cmlive** 的请求上：{}",
                r.url
            );
            cmlive_with_jwt += 1;
        }

        // ── (b) 云盘侧六步链的请求上**绝不能**出现直播侧 JWT ───────────────
        if on_ycloud {
            assert!(
                header(r, "Authorization").as_deref() != Some(TEST_LIVE_JWT),
                "⭐⭐⭐ 直播侧 JWT 传进了云盘侧请求（实测会回 90001 未登录）：{}",
                r.url
            );
            assert!(
                header(r, "jwttoken").as_deref() != Some(TEST_LIVE_JWT),
                "⭐⭐⭐ 直播侧 JWT 被当成了云盘侧 jwttoken：{}",
                r.url
            );
            assert!(
                !r.url.contains("/cmlive") || on_gate_step1,
                "云盘侧请求不该借道 /cmlive：{}",
                r.url
            );
        }

        // ── (c) 反过来：/cmlive 的请求不能带云盘侧那套 H5 头 ────────────────
        if on_cmlive {
            assert!(
                header(r, "activityid").is_none(),
                "⭐⭐ /cmlive 的请求不该带 H5 的 activityid 头（那是云盘侧那一套）：{}",
                r.url
            );
            assert!(
                header(r, "jwttoken").is_none(),
                "⭐⭐ /cmlive 的请求不该带 jwttoken 头（那是云盘侧那一套）：{}",
                r.url
            );
        }

        // ── (d) 第①步（换 SSO）不该带任何一张业务票 ───────────────────────
        if on_gate_step1 {
            assert!(
                header(r, "jwttoken").is_none(),
                "换 SSO 这一步还没有票，不该带 jwttoken：{}",
                r.url
            );
            assert!(
                header(r, "Authorization").as_deref() != Some(TEST_LIVE_JWT),
                "换 SSO 用的是库里的 Basic 凭据，不是直播侧 JWT：{}",
                r.url
            );
        }
    }

    // 直播侧 JWT 必须**真的**被用上（否则上面那些断言全是空转）。
    //
    // ⚠ 计数是 **2** 而不是更多：收集器在这条用例里是**替身**（不碰 HTTP），
    // 所以真正带这张票的只有门第③步的**两个 `tabContentList`**。
    assert_eq!(
        cmlive_with_jwt, 2,
        "直播侧 JWT 应当正好用在门第③步的两个页签上（实得 {cmlive_with_jwt} 处）"
    );
    assert_eq!(
        http.count_of("/tabContentList"),
        2,
        "两个页签各一枪（127 福利 + 129 回放）"
    );

    // 而六步链**确实**打了（否则 (b) 也是空转）。
    assert!(
        reqs.iter().any(|r| r.url.contains("/liveRoomFeedback")),
        "六步链必须真的跑过，否则'两套票不混用'这条断言没有意义"
    );
}

/// ⭐ 直播侧 JWT **确实**被传给了收集器（而不是传了个空串）。
///
/// # 为什么这条必须单独有
///
/// `run_account` 里那个 `jwt` 早先是 `unwrap_or_default()` ——
/// 忘了填就是**静默传空串**：收集器拿去连 WS 会失败，
/// 而报错说的是"连不上"，排查方向完全错。
/// 这里用替身把"收到的那串"记下来，钉住"传的是门换来的那一张"。
#[tokio::test]
async fn the_gate_jwt_is_handed_to_the_collector_for_real() {
    let rooms = serde_json::json!([room_entry(147606, 1, "中国移动云盘")]);
    let (c, _) = ctx_with(&open_gate_chain(rooms));
    let collector = JwtRecordingCollector::default();
    let params = LiveRoomParams {
        codes: vec![],
        listen_seconds: 60,
        ..Default::default()
    };

    run_gated(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();

    let seen = collector.jwts();
    assert_eq!(seen.len(), 1, "应当收集过一次");
    assert_eq!(
        seen[0], TEST_LIVE_JWT,
        "⭐⭐ 收集器拿到的必须是**门换来的那张直播侧 JWT**，不是空串、不是别的票"
    );
    assert!(!seen[0].is_empty(), "绝不能是空串（那会变成一句'连不上'）");
}

/// ⭐ `LiveRoomTask::run` 端到端：**不填 codes** 也能自动收口令并进账。
///
/// 前面的用例测的是 `run_account`（编排本体），这条测的是**任务入口** ——
/// 它才是"用户点一下"真正走到的地方。两者之间的差别正是历史上那处
/// `live_jwt: None` 写死所在的位置。
#[tokio::test]
async fn live_room_task_auto_collects_without_manual_codes() {
    let rooms = serde_json::json!([room_entry(147606, 1, "中国移动云盘")]);
    let mut chain = open_gate_chain(rooms);
    chain.extend(happy_chain(235, 245, Some(give_ok(0)), &[draw_ok(10)], 0));
    let (c, http) = ctx_with(&chain);

    // 注入假收集器：`LiveRoomTask` 生产上用的是真 WS 收集器。
    let task = LiveRoomTask {
        collector: std::sync::Arc::new(FakeCollector::new(&[&["自动口令"]])),
    };

    let out = task
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .expect("不填 codes 也要能跑（这正是 G2 要修的那件事）");

    assert!(out.is_success(), "{out:?}");
    assert!(
        out.summary().contains("进账 10"),
        "⭐ 自动收来的口令真的换到了花：{}",
        out.summary()
    );
    assert_eq!(http.count_of("/querySpecTokenV2"), 1);
    assert_eq!(http.count_of("/redFlower/draw"), 1);
}

// ══════════════ 源 B（小红书）**单独**可测 —— 不需要开播 ══════════════

/// ⭐ 小红书那一源**自己就能测**：它既不需要开播，也不需要直播间。
///
/// # 这条是用户口径直接要的（2026-10-03）
///
/// 「小红书验证与开播是否无关…… 你可以自己写不验证开播的测试，只测试小红书抓取」。
///
/// 做法是**直接调那一源**（`fetch_xiaohongshu`），绕开 `LiveRoomRun::run_account`
/// 那道门 —— 门要求"有目标主播的房间在播"，而主页是**公开页面**，与房间无关。
///
/// # ⚠ 它顺带把一处行为上的不对称钉在案上
///
/// `run_account` 在 `!gate.active` 时是**直接 `return Ok(out)`**（`live_room.rs:1386`）
/// ⇒ 真跑一次（当天没开播）时，源 B **永远不会被走到** ✓。所以模块头那句
/// "两个来源（WebSocket 弹幕 / 小红书主页）**地位相同**"只在
/// [`LiveKoulingCollector::collect_inner`] 里面成立，**在任务层不成立**。
/// 这一条测的就是"源 B 本身是好的，挡住它的是任务层的门"。
#[tokio::test]
async fn xiaohongshu_source_extracts_codes_without_any_live_room() {
    // 样本**从黄金向量里取**（真机抓的那份），不另造：造一份"我认为对"的 HTML
    // 只能证明我自己同意自己。
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/golden/live-room-vectors.json"
    ))
    .expect("黄金向量文件必须可读");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("向量必须是合法 JSON");
    let case = &v["xiaohongshuExtract"][0];
    let html = case["html"].as_str().expect("用例应有 html");
    let expect: Vec<String> = case["expect"]
        .as_array()
        .expect("expect 应为数组")
        .iter()
        .map(|x| x.as_str().expect("expect 里应是字符串").to_string())
        .collect();
    assert!(
        !expect.is_empty(),
        "挑中的第一条向量必须有口令，否则这条测试证明不了什么（实得：{expect:?}）"
    );

    let (c, http) = Ctx::new(&[html]);
    // ⚠ 没有房间、没有门、没有 poll —— 直接调那一源。
    let out = LiveKoulingCollector::production()
        .fetch_xiaohongshu(&c.ctx())
        .await
        .expect("取一个公开主页不该失败");

    assert_eq!(out.codes, expect, "提取结果必须与黄金向量逐条一致");
    assert!(
        out.failure.is_none(),
        "取到了就不该带 failure：{:?}",
        out.failure
    );
    assert_eq!(http.count(), 1, "只有那一枪 GET");
    let urls: Vec<String> = http.requests().iter().map(|r| r.url.clone()).collect();
    assert!(
        urls[0].contains("xiaohongshu.com/user/profile"),
        "打的是那个公开主页：{}",
        urls[0]
    );
}

// ══════════════════════════ 判据 11：register_all 接入 ══════════════════════════

/// ⭐ 两个任务真的**注册进了 registry** —— CLI 与 `/api/capabilities` 都由它驱动。
///
/// 这条是"接入"这件事的判据：光有任务类型不注册，用户在 CLI 里
/// `run live-room` 会拿到"未知任务"，而 `/api/capabilities` 里根本不出现。
#[test]
fn live_room_task_is_registered() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let names: Vec<String> = registry.list().iter().map(|m| m.name.clone()).collect();
    assert!(
        names.contains(&"live-room".to_string()),
        "live-room 必须注册（CLI/capabilities 都按 registry 走）：{names:?}"
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

/// `SmidStore` 替身：默认"这个 store 不管 smid"。
///
/// 2026-10-03 起 `Store` 把 `SmidStore` 并成了 supertrait（任务层要能读
/// 这个账号**已换回来**的那个 `deviceId` —— 它和登录时那份不是同一个东西）。
/// 这些替身要证明的事与 smid 无关，所以读一律 `None`、写一律拒绝。
#[async_trait]
impl mcloud_core::SmidStore for NoStore {
    async fn get_smid_state(
        &self,
        _id: &mcloud_core::AccountId,
    ) -> mcloud_core::Result<Option<mcloud_core::SmidState>> {
        Ok(None)
    }
    async fn set_smid_fingerprint(
        &self,
        _id: &mcloud_core::AccountId,
        _fingerprint_json: &str,
    ) -> mcloud_core::Result<()> {
        Err(refuse())
    }
    async fn set_smid_device_id(
        &self,
        _id: &mcloud_core::AccountId,
        _device_id: &str,
    ) -> mcloud_core::Result<()> {
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
/// 服务端必然回 `200108 口令无效`；用户看到的是"我填了两条，怎么只领一条还说口令错"。
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
/// 拆错了会拿去领一条不存在的口令（`200108`）。
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

/// ⭐ 两份 `KOULING_ANCHOR` 必须**字面相同**（用户口径：有效的主播只有一个）。
///
/// `mcloud-core`（挑「下一场」的地方）与 `mcloud-upstream`（判定门的地方）层次上
/// **互不依赖** ✗ ⇒ 常量各立一处 ⇒ 靠这条用例钉住相等 ✓。
/// 谁改了一处而忘了另一处 ⇒ 这里当场红 ✓（而不是界面上悄悄换了一个主播 ✗）。
#[test]
fn anchors_agree_between_core_and_upstream() {
    assert_eq!(
        mcloud_core::kouling::KOULING_ANCHOR,
        mcloud_upstream::live_consts::KOULING_ANCHOR,
        "core 与 upstream 的 KOULING_ANCHOR 必须一致 —— 这是「有效的主播」的唯一判据"
    );
}

/// 不变量：两次自动抓取之间的最小间隔 **必须 ≥ 一次抓取要听的时长**。
///
/// 依据：`SCRAPE_MIN_INTERVAL_MS` 存在的理由就是"一个 mcloud 用户只开一个抓取"
/// —— 一次抓取要听满 `listenSeconds`（缺省 [`DEFAULT_LISTEN_SECONDS`]），
/// 间隔比它短 ⇒ 第一个账号还没听完、第二个账号就另起一次 ✗。
///
/// ⚠ 这条断言**只能**钉在这里：两个常量分属两个 crate，依赖方向是
/// `mcloud-tasks → mcloud-core`，`mcloud-core` 里引不到 `DEFAULT_LISTEN_SECONDS`
/// （那边注释与这边互相点名，见两处文档）。今后任一方改了数字，这条会红。
#[test]
fn scrape_interval_covers_a_whole_listen() {
    assert!(
        mcloud_core::kouling::SCRAPE_MIN_INTERVAL_MS >= DEFAULT_LISTEN_SECONDS as i64 * 1000,
        "抓取去重窗口（{} ms）短于一次抓取要听的时长（{} 秒）⇒ 多账号会重叠抓取",
        mcloud_core::kouling::SCRAPE_MIN_INTERVAL_MS,
        DEFAULT_LISTEN_SECONDS,
    );
}

// ══════════════════════ 判据 14：逐口令那一行（"每个口令领了多少"）══════════════════════
//
// 用户口径：「我总得看到每个口令领取了多少吧」。
//
// ⚠ 断言打在**产生文案的纯函数**（`RedFlowerReport::draw_lines` / `draw_line`）上，
// **不**去抓 tracing 事件：`with_default` 是线程局部、interest 缓存是进程全局
// ⇒ 并行用例会互相踩（本仓刚踩过）。纯函数是逐字稳定的。

/// 造一条 `draw` 记录（直接给判定，不走 HTTP —— 断的就是文案）。
fn draw_rec(code: &str, outcome: RedFlowerDrawOutcome) -> DrawRecord {
    DrawRecord {
        code: code.to_string(),
        outcome,
    }
}

/// `draw` 成功那一档的判定（含奖品名）。
fn draw_success(flower_num: i64, prize_name: &str) -> RedFlowerDrawOutcome {
    RedFlowerDrawOutcome {
        kind: RedFlowerDrawKind::Succeeded,
        result: proto::RedFlowerDrawResult {
            flower_num,
            prize_name: prize_name.to_string(),
            ..Default::default()
        },
        code: Some(0),
        message: "success".into(),
    }
}

/// 造一条"上游回了业务码"的判定（**走真实分类函数**，不手搓 kind）。
fn draw_by_code(code: &str, biz: i64, msg: &str) -> DrawRecord {
    draw_rec(
        code,
        proto::classify_red_flower_draw(&RedFlowerEnvelope::parse(&code_body(biz, msg))),
    )
}

/// ⭐ 全部成功 ⇒ 每个口令一行，**带上它领到几朵**（上游 `result.flowerNum`）。
#[test]
fn draw_lines_all_succeeded_carry_the_count() {
    let report = RedFlowerReport {
        draws: vec![
            draw_rec("AI笔记一键提炼", draw_success(10, "会员日奖励")),
            // 奖品名为空时不多打一对空括号。
            draw_rec("云盘AI高效办公", draw_success(15, "")),
        ],
        ..Default::default()
    };

    assert_eq!(
        report.draw_lines(),
        vec![
            "口令「AI笔记一键提炼」领到 10 小红花（会员日奖励）",
            "口令「云盘AI高效办公」领到 15 小红花",
        ]
    );
}

/// ⭐ 部分成功部分失败 ⇒ **成功的写数量、失败的写原因**，一行一个、保序。
///
/// ⚠ 断言里刻意要求三种"没领到"**措辞各不相同** —— 它们的排查方向不同
/// （已兑换过 = 口令有效只是领过 / 口令无效 = 抄错了 / 已失效 = 本场结束了）。
#[test]
fn draw_lines_mixed_success_and_failure() {
    let report = RedFlowerReport {
        draws: vec![
            draw_rec("甲", draw_success(10, "")),
            draw_by_code("乙", 200112, "已兑换"),
            draw_by_code("丙", 200108, "口令无效"),
        ],
        ..Default::default()
    };

    assert_eq!(
        report.draw_lines(),
        vec![
            "口令「甲」领到 10 小红花",
            "口令「乙」没领到：已兑换过",
            "口令「丙」没领到：口令无效",
        ]
    );
}

/// ⭐ 全失败 ⇒ 每行都**写清原因**，不许只有"失败"两个字。
#[test]
fn draw_lines_all_failed_state_each_reason() {
    let report = RedFlowerReport {
        draws: vec![
            draw_by_code("甲", 200112, "已兑换"),
            draw_by_code("乙", 200108, "口令无效"),
            draw_by_code("丙", 200118, "口令已失效"),
            // 未分类码：把上游**原话**带上，别只留一个数字。
            draw_by_code("丁", 400001, "上游说了个别的码"),
            // 响应解不开：没有 code 字段 / 有 code=0 却没有 result，两种分开说。
            draw_rec(
                "戊",
                proto::classify_red_flower_draw(&RedFlowerEnvelope::parse(r#"{"msg":"无码"}"#)),
            ),
            draw_rec(
                "己",
                proto::classify_red_flower_draw(&RedFlowerEnvelope::parse(
                    r#"{"code":0,"msg":"success"}"#,
                )),
            ),
        ],
        ..Default::default()
    };

    assert_eq!(
        report.draw_lines(),
        vec![
            "口令「甲」没领到：已兑换过",
            "口令「乙」没领到：口令无效",
            "口令「丙」没领到：口令已失效（本场已结束）",
            "口令「丁」没领到：上游回 code=400001（上游说了个别的码）",
            "口令「戊」没领到：上游响应没有 code 字段",
            "口令「己」没领到：上游回了 code=0 但没有 result",
        ]
    );
    for line in report.draw_lines() {
        assert!(line.contains("没领到："), "每行都要写清原因：{line}");
    }
}

/// ⭐ 口令数 0 ⇒ 一行都不打（**别刷屏**：没投过口令就没什么可说的）。
#[test]
fn draw_lines_empty_when_no_codes() {
    assert!(RedFlowerReport::default().draw_lines().is_empty());
}

/// ⭐⭐ 首次参与惊喜奖（③）**不归到任何口令头上**：它是独立的一笔。
///
/// 口径：口令那一行说"这条口令领到 10 朵"必须**是真话** ⇒ 那 +10 不能塞进去；
/// 它照旧由 `summary_line` 的尾巴说（`首次参与 +10`）。
#[test]
fn first_participate_bonus_is_not_attributed_to_a_code() {
    let report = RedFlowerReport {
        balance_before: balance(100),
        balance_after: balance(120),
        first_participate: Some(proto::FirstParticipateOutcome {
            kind: FirstParticipateKind::Gained,
            flower_num: 10,
            ..Default::default()
        }),
        draws: vec![draw_rec("甲", draw_success(10, ""))],
        ..Default::default()
    };

    assert_eq!(report.draw_lines(), vec!["口令「甲」领到 10 小红花"]);
    assert!(
        !report.draw_lines()[0].contains("首次参与"),
        "独立奖励不许塞进口令那一行"
    );
    assert!(
        report.summary_line().contains("首次参与 +10"),
        "它该在汇总里：{}",
        report.summary_line()
    );
    assert_eq!(report.gained(), 20, "进账 = ⑥−① = 口令 10 + 首参 10");
}

/// ⭐ 端到端：真跑六步链（口令 10 + 已兑换）⇒ `draw_lines` 与判定逐字对得上。
///
/// 这条钉的是"报告里的 `draws` 真的带着上游 `flowerNum`"（纯函数用例是手搓判定，
/// 单独跑通不能证明解析那条路）。
#[tokio::test]
async fn draw_lines_reflect_the_real_chain() {
    let (c, _http) = ctx_with(&happy_chain(
        235,
        245,
        Some(give_ok(0)),
        &[draw_ok(10), code_body(200112, "已兑换")],
        0,
    ));
    let collector = FakeCollector::empty();
    let params = no_retry_params(&["甲", "乙"]);

    let res = run_with(&collector)
        .run_account(&c.ctx(), &params)
        .await
        .unwrap();
    let report = res.report.as_ref().unwrap();

    assert_eq!(
        report.draw_lines(),
        vec![
            "口令「甲」领到 10 小红花（小红花）",
            "口令「乙」没领到：已兑换过"
        ]
    );
}
