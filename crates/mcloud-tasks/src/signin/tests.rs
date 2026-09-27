//! 签到族（P1）测试。
//!
//! 全程离线：假 HTTP 按脚本回放**实拍形状**的响应，
//! 断言的是"发了哪几枪、按什么分支走、结论说成什么"。
//!
//! 判据来源：`test/golden/app-signin-vectors.json`（2026-09-19 真账号抓包）。
//! 本文件里的响应体都照着那份形状写，数字用实拍值
//! （`total=10496`、`beforeTotal=8027`、复活 `rewardClouds=114`…）。

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

    /// 第 n 个请求的 URL（0 起）。
    fn url_at(&self, n: usize) -> String {
        self.requests()[n].url.clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }
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
    clock: std::sync::Arc<FixedClock>,
}

#[derive(Default)]
struct CollectSink {
    events: Mutex<Vec<mcloud_core::RunEvent>>,
}

impl CollectSink {
    fn logs(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                mcloud_core::RunEvent::Log { message, .. } => Some(message.clone()),
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

struct FixedClock(i64);
impl mcloud_core::Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

impl Ctx {
    fn new(replies: &[&str]) -> (Self, std::sync::Arc<Scripted>) {
        let http = std::sync::Arc::new(Scripted::new(replies));
        (
            Self {
                http: http.clone(),
                account: account(),
                sink: std::sync::Arc::new(CollectSink::default()),
                clock: std::sync::Arc::new(FixedClock(1_700_000_000_000)),
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

// ── 响应构造 ────────────────────────────────────────────────────────────

/// `infoV3`：用实拍的那份数字，`signed` 控制"今天签没签"。
fn info_body(signed_today: bool, total: i64) -> String {
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": total,
            "beforeTotal": 8027,          // 实拍：与 total **不相等**
            "signInPoints": 3,
            "signCount": 1,
            "monthDays": 7,
            "curMonthBackup": true,
            "curMonthBackupSignAccept": true,
            "toReceive": 6,
            "cal": [
                {"d":31,"s":false,"n":0,"p":0,"t":false,"currentMonth":0},
                {"d":18,"s":true,"n":0,"p":0,"t":false,"currentMonth":1},
                {"d":19,"s":signed_today,"n":3,"p":0,"t":true,"currentMonth":1}
            ]
        }
    })
    .to_string()
}

/// 带 `receiveList` 的 `infoV3`。
fn info_with_receive(items: serde_json::Value) -> String {
    let mut v: serde_json::Value = serde_json::from_str(&info_body(false, 10496)).unwrap();
    v["result"]["receiveList"] = items;
    v.to_string()
}

fn ok(result: serde_json::Value) -> String {
    serde_json::json!({"code":0,"msg":"success","result":result}).to_string()
}

/// 上游回**非零码**（业务失败的正常形状）。
///
/// 与 [`ok`] 配对：用来构造"某一笔失败、其余仍要照跑"这类场景。
fn fail(msg: &str, code: i64) -> String {
    serde_json::json!({"code":code,"msg":msg}).to_string()
}

// ══════════════════════════ sign ══════════════════════════

/// 今日已签 ⇒ **一枪都不发**（除了那次查询），并且不报失败。
#[tokio::test]
async fn sign_skips_when_already_signed_today() {
    let (c, http) = Ctx::new(&[&info_body(true, 10496)]);
    let out = SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "已签过是成功，不是失败：{out:?}");
    assert_eq!(http.count(), 1, "已签过不该发 startSignIn");
    assert!(out.summary().contains("今日已签到"));
}

/// 未签 ⇒ 提交 + **回查**（判据是第二次 `infoV3`）。
#[tokio::test]
async fn sign_submits_then_rechecks() {
    let (c, http) = Ctx::new(&[
        &info_body(false, 10496),
        &ok(serde_json::json!({})),
        &info_body(true, 10499),
    ]);
    let out = SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert_eq!(http.count(), 3, "查询 → 提交 → 回查");
    assert!(http.url_at(1).contains("/ycloud/signin/page/startSignIn"));
    assert!(http.url_at(2).contains("/ycloud/signin/page/infoV3"));
    assert!(
        out.summary().contains("获得 3 云朵"),
        "实得：{}",
        out.summary()
    );
}

/// ⭐ `startSignIn` 的响应**不作为判据**。
///
/// 实拍：它说没签，但紧接着 `infoV3` 已经是 `s:true`。
/// 只信它的实现会把一次**成功**的签到报成失败 —— 用户看到失败又点一次，
/// 就是重复请求。
#[tokio::test]
async fn sign_trusts_the_recheck_not_the_submit_response() {
    // startSignIn 回一份"看起来没签"的响应（cal 全是 false）
    let misleading = serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"todaySignIn": false, "cal": [{"d":19,"s":false,"t":true,"currentMonth":1}]}
    })
    .to_string();
    let (c, _http) = Ctx::new(&[
        &info_body(false, 10496),
        &misleading,
        &info_body(true, 10499),
    ]);

    let out = SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        out.is_success(),
        "回查说签上了就该报成功，不能信 startSignIn：{out:?}"
    );
}

/// 提交后回查仍未签 ⇒ **失败**（不能吞错）。
#[tokio::test]
async fn sign_fails_when_recheck_still_unsigned() {
    let (c, _http) = Ctx::new(&[
        &info_body(false, 10496),
        &ok(serde_json::json!({})),
        &info_body(false, 10496),
    ]);
    let out = SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_failure(), "回查没签上必须报失败：{out:?}");
    assert!(out.summary().contains("回查仍未签到"));
}

/// 签上了但**余额没动** ⇒ 成功，且如实说出来（补签 / 延迟入账）。
#[tokio::test]
async fn sign_reports_zero_gain_honestly() {
    let (c, _http) = Ctx::new(&[
        &info_body(false, 10496),
        &ok(serde_json::json!({})),
        &info_body(true, 10496),
    ]);
    let out = SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success());
    assert!(
        out.summary().contains("余额未变"),
        "实得：{}",
        out.summary()
    );
}

/// 演练模式：查一次、不提交。
#[tokio::test]
async fn sign_dry_run_does_not_submit() {
    let (c, http) = Ctx::new(&[&info_body(false, 10496)]);
    let out = SignTask
        .run(&c.ctx(), &serde_json::json!({"dryRun": true}))
        .await
        .unwrap();

    assert!(out.is_success());
    assert_eq!(http.count(), 1, "演练不该提交");
    assert!(out.summary().contains("演练"));
}

/// `deviceId` 是 base64 ⇒ 查询串里必须编码（`+` 不编码表示空格）。
#[tokio::test]
async fn sign_percent_encodes_the_device_id() {
    let (c, http) = Ctx::new(&[&info_body(true, 1)]);
    SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    let url = http.url_at(0);
    assert!(url.contains("deviceId=DEV%2B1%2Fbase%3D"), "实得：{url}");
    assert!(!url.contains("DEV+1"), "`+` 必须编码：{url}");
}

/// ⭐ 会话失效（90001）要说清是**票**的问题，不能混成"任务失败"。
#[tokio::test]
async fn session_expiry_is_explained_not_generic() {
    let (c, _http) = Ctx::new(&[r#"{"code":90001,"msg":"未登录"}"#]);
    let err = SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap_err();
    let text = err.to_string();

    assert!(text.contains("会话"), "应说清是会话问题：{text}");
    assert!(text.contains("票"), "应引向票而不是账号：{text}");
}

/// 上游业务码非 0 ⇒ 失败，且带上原文。
#[tokio::test]
async fn upstream_error_code_becomes_failure() {
    let (c, _http) = Ctx::new(&[r#"{"code":500,"msg":"系统繁忙"}"#]);
    let err = SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("500") && text.contains("系统繁忙"),
        "实得：{text}"
    );
}

// ══════════════════════════ receive ══════════════════════════

/// `cloudType == 2`（下月可领取）⇒ **不发任何请求**。
///
/// 抓包证据：那条账号的 `receiveList` 只有 `{cloudType:2}`，
/// 而整个 HAR 里 `receiveV3` 的请求数是 **0**。
#[tokio::test]
async fn receive_does_not_send_for_next_month_items() {
    let info = info_with_receive(serde_json::json!([
        {"cloudType":2,"cloudNum":240}
    ]));
    let (c, http) = Ctx::new(&[&info]);
    let out = ReceiveTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success());
    assert_eq!(http.count(), 1, "只有那一次状态查询，没有领取请求");
    assert!(
        out.summary().contains("下月可领取"),
        "实得：{}",
        out.summary()
    );
}

/// `cloudType == 0` ⇒ 走 `receiveV3`，并读回 `result.receive`。
#[tokio::test]
async fn receive_claims_now_items_and_reads_the_amount() {
    let info = info_with_receive(serde_json::json!([
        {"cloudType":0,"cloudNum":6}
    ]));
    let (c, http) = Ctx::new(&[&info, &ok(serde_json::json!({"receive":6}))]);
    let out = ReceiveTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert_eq!(http.count(), 2);
    assert!(http.url_at(1).contains("/ycloud/signin/page/receiveV3"));
    assert!(out.summary().contains("6 云朵"), "实得：{}", out.summary());

    // ⭐ 领云朵那一枪**必须**带 deviceId 到 body 里（不带会回 615）
    let body: serde_json::Value =
        serde_json::from_str(http.requests()[1].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["deviceId"], "DEV+1/base=");
    assert!(http.requests()[1]
        .headers
        .iter()
        .any(|(k, v)| k == "isdeviceid" && v == "true"));
}

/// `cloudType == 1`（已到期）⇒ 走 `receiveTaskExpansion`，**且 result 是裸数字**。
#[tokio::test]
async fn receive_handles_expansion_items_with_bare_numeric_result() {
    let info = info_with_receive(serde_json::json!([
        {"cloudType":1,"cloudNum":20,"acceptDate":"2026-09-30"}
    ]));
    // 实拍形状：`result` 是**裸数字** 100，不是对象
    let (c, http) = Ctx::new(&[&info, r#"{"code":0,"msg":"success","result":100}"#]);
    let out = ReceiveTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "裸数字 result 不该被判成失败：{out:?}");
    assert!(http.url_at(1).contains("receiveTaskExpansion"));
    assert!(http.url_at(1).contains("acceptDate=2026-09-30"));
}

/// 没有待领取 ⇒ 成功且不谎报。
#[tokio::test]
async fn receive_reports_nothing_to_claim() {
    let info = info_with_receive(serde_json::json!([]));
    let (c, http) = Ctx::new(&[&info]);
    let out = ReceiveTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success());
    assert_eq!(http.count(), 1);
    assert!(out.summary().contains("没有待领取"));
}

// ══════════════════════════ multiple ══════════════════════════

/// 门槛不成立 ⇒ **跳过**，且**一发都不发**（每月只有一次机会，白耗不起）。
#[tokio::test]
async fn multiple_skips_when_gate_is_closed() {
    // curMonthBackupSignAccept=true ⇒ 本月已用掉
    let (c, http) = Ctx::new(&[&info_body(false, 10496)]);
    let out = MultipleTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(
        matches!(out, TaskOutcome::Skipped { .. }),
        "条件不成立是跳过，不是失败：{out:?}"
    );
    assert_eq!(http.count(), 1, "不该发 multiple");
    assert!(
        out.summary().contains("已经用掉"),
        "实得：{}",
        out.summary()
    );
}

/// 门槛成立 ⇒ 发 `multiple`，且**倍数与合计都报**。
#[tokio::test]
async fn multiple_claims_and_reports_factor_and_total() {
    let mut v: serde_json::Value = serde_json::from_str(&info_body(false, 10496)).unwrap();
    v["result"]["curMonthBackupSignAccept"] = serde_json::json!(false);
    let info = v.to_string();

    let (c, http) = Ctx::new(&[
        &info,
        &ok(serde_json::json!({"cloudCount":42,"multiple":14})),
    ]);
    let out = MultipleTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert_eq!(http.count(), 2);
    // 裸 GET：不带查询参数
    assert!(
        http.url_at(1).ends_with("/ycloud/signin/page/multiple"),
        "实得：{}",
        http.url_at(1)
    );
    assert!(
        out.summary().contains("14 倍"),
        "倍数是 14 倍：{}",
        out.summary()
    );
    assert!(out.summary().contains("42"), "合计是 42：{}", out.summary());
}

/// 没开备份 ⇒ 跳过（不是失败）。
#[tokio::test]
async fn multiple_skips_when_backup_is_off() {
    let mut v: serde_json::Value = serde_json::from_str(&info_body(false, 10496)).unwrap();
    v["result"]["curMonthBackup"] = serde_json::json!(false);
    let info = v.to_string();

    let (c, http) = Ctx::new(&[&info]);
    let out = MultipleTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(matches!(out, TaskOutcome::Skipped { .. }));
    assert_eq!(http.count(), 1);
    assert!(out.summary().contains("没有开启自动备份"));
}

/// 上游没给门槛字段 ⇒ **也不领**（宁可少领一次，不白耗机会）。
#[tokio::test]
async fn multiple_skips_when_gate_fields_are_missing() {
    let v = serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"total": 100, "cal": []}
    });
    let (c, http) = Ctx::new(&[&v.to_string()]);
    let out = MultipleTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(matches!(out, TaskOutcome::Skipped { .. }));
    assert_eq!(http.count(), 1, "字段不明时不该盲领");
}

// ══════════════════════════ revival ══════════════════════════

/// ⭐ "本月已领取" ⇒ **跳过，不是失败**。
///
/// 真账号实拍：`{"code":500,"msg":"本月已领取复活卡奖励"}`（HTTP 200）。
/// 判成失败会让历史页每天一条红、并触发推送，而它下个月之前都不会好。
#[tokio::test]
async fn revival_treats_already_claimed_as_skipped() {
    let (c, _http) = Ctx::new(&[r#"{"code":500,"msg":"本月已领取复活卡奖励"}"#]);
    let out = RevivalTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(
        matches!(out, TaskOutcome::Skipped { .. }),
        "已领取是跳过，不是失败：{out:?}"
    );
    assert!(
        out.summary().contains("本月已领取过"),
        "实得：{}",
        out.summary()
    );
}

/// 认不出的失败文案 ⇒ **仍然报失败**（宁可漏认，也不把真故障洗白）。
#[tokio::test]
async fn revival_keeps_unrecognised_errors_as_failures() {
    let (c, _http) = Ctx::new(&[r#"{"code":500,"msg":"本月已领取"}"#]);
    let err = RevivalTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("本月已领取"),
        "裸一句'本月已领取'刻意不在匹配面里：{err}"
    );
}

/// 成功：报卡型 + 本次进账，**且不把 totalClouds 当余额**。
#[tokio::test]
async fn revival_reports_card_type_and_reward() {
    // 实拍响应
    let real = r#"{"code":0,"msg":"success","result":{"cardType":1,"littleFirstTime":0,"rewardClouds":114,"totalClouds":571}}"#;
    let (c, http) = Ctx::new(&[real]);
    let out = RevivalTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert_eq!(http.count(), 1);
    assert!(http.url_at(0).contains("receiveRevivalReward"));
    assert!(
        out.summary().contains("复活卡"),
        "卡型要说出来：{}",
        out.summary()
    );
    assert!(
        out.summary().contains("114"),
        "本次进账要报：{}",
        out.summary()
    );
    // ⚠ 571 是 totalClouds（累计），不是余额 —— 不该出现在小结里当余额
    assert!(
        !out.summary().contains("余额 571"),
        "别把 totalClouds 当余额：{}",
        out.summary()
    );
}

/// `cardType == 2` ⇒ 回馈卡。
#[tokio::test]
async fn revival_reports_feedback_card() {
    let body =
        r#"{"code":0,"msg":"success","result":{"cardType":2,"rewardClouds":5,"totalClouds":9}}"#;
    let (c, _http) = Ctx::new(&[body]);
    let out = RevivalTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.summary().contains("回馈卡"), "实得：{}", out.summary());
}

// ══════════════════════════ backup-gift ══════════════════════════

/// `backupgift/info` 的响应。
fn gift_body(state: i64, cur_month: bool) -> String {
    serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"state": state, "curMonth": cur_month}
    })
    .to_string()
}

/// ⭐ 端点必须是**另一个主机**上的 `/market/backupgift/*`。
///
/// 这一族里只有它不是 `m.mcloud.139.com/ycloud/`。照抄 `infoV3` 那套
/// 会打到一个**不存在的路径**上 —— 而返回的非零码会被读成"上游改了接口"。
#[tokio::test]
async fn backup_gift_uses_its_own_host_and_path() {
    let (c, http) = Ctx::new(&[&gift_body(0, true), &ok(serde_json::json!(1))]);
    BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    let info = http.url_at(0);
    assert!(
        info.starts_with("https://caiyun.feixin.10086.cn/market/backupgift/info"),
        "状态查询的主机/路径不对：{info}"
    );
    let recv = http.url_at(1);
    assert!(
        recv.starts_with("https://caiyun.feixin.10086.cn/market/backupgift/receive"),
        "领取的主机/路径不对：{recv}"
    );
}

/// `state == -1`（未开备份）⇒ 跳过 + 给出**人工出口**。
#[tokio::test]
async fn backup_gift_skips_when_backup_is_off() {
    let (c, http) = Ctx::new(&[&gift_body(-1, true)]);
    let out = BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(matches!(out, TaskOutcome::Skipped { .. }), "{out:?}");
    assert_eq!(http.count(), 1, "不可领就不该发 receive");
    assert!(
        out.summary().contains("App"),
        "要给出人工出口：{}",
        out.summary()
    );
}

/// `state == 1`（本月已领过）⇒ 跳过，且措辞与"未开备份"**不同**。
#[tokio::test]
async fn backup_gift_skips_when_already_claimed() {
    let (c, http) = Ctx::new(&[&gift_body(1, true)]);
    let out = BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(matches!(out, TaskOutcome::Skipped { .. }));
    assert_eq!(http.count(), 1);
    assert!(
        out.summary().contains("已经领过"),
        "实得：{}",
        out.summary()
    );
    assert!(
        !out.summary().contains("App"),
        "已领过不需要用户去做任何事，别让他去开备份：{}",
        out.summary()
    );
}

/// `curMonth == false` ⇒ 跳过，且**连 state 都不看**（旧项目的判据顺序）。
#[tokio::test]
async fn backup_gift_skips_when_there_is_no_gift_this_month() {
    // state 是"可领"，但本月没有这份礼 ⇒ 仍然跳过
    let (c, http) = Ctx::new(&[&gift_body(0, false)]);
    let out = BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(matches!(out, TaskOutcome::Skipped { .. }), "{out:?}");
    assert_eq!(http.count(), 1);
    assert!(
        out.summary().contains("本月没有"),
        "实得：{}",
        out.summary()
    );
}

/// `state == 0` 且本月有 ⇒ 领，且**只发一枪**（GET 但是写，不能重试）。
#[tokio::test]
async fn backup_gift_claims_exactly_once() {
    let (c, http) = Ctx::new(&[&gift_body(0, true), &ok(serde_json::json!(1))]);
    let out = BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert_eq!(http.count(), 2, "查一次 + 领一次，不重试");
    assert!(
        out.summary().contains("获得 1 云朵"),
        "实得：{}",
        out.summary()
    );
}

/// ⭐ 认不出的 `state` ⇒ **报错，不猜**。
///
/// 猜"可领"会白打一枪写请求；猜"不可领"会永久漏领。
/// 这条防的是"上游加了个新状态"被静默处理成其中一种。
#[tokio::test]
async fn backup_gift_refuses_an_unknown_state() {
    let (c, http) = Ctx::new(&[&gift_body(7, true)]);
    let err = BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("state=7"), "要点出是哪个值：{err}");
    assert_eq!(http.count(), 1, "状态不明时绝不发写请求");
}

/// `receive` 的 `result` 是**裸数字**，缺了也不判失败。
#[tokio::test]
async fn backup_gift_tolerates_missing_amount() {
    let (c, _http) = Ctx::new(&[&gift_body(0, true), r#"{"code":0,"msg":"success"}"#]);
    let out = BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "份数缺失不该把一次成功报成失败：{out:?}");
    assert!(
        out.summary().contains("份数上游未给"),
        "实得：{}",
        out.summary()
    );
}

/// `info` 缺 `state` ⇒ 解析失败（它是主判据，缺了没法判该不该领）。
#[tokio::test]
async fn backup_gift_requires_the_state_field() {
    let body = r#"{"code":0,"msg":"success","result":{"curMonth":true}}"#;
    let (c, _http) = Ctx::new(&[body]);
    let err = BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("state"), "实得：{err}");
}

// ══════════════════════════ prizes ══════════════════════════

/// 有奖品 ⇒ 逐条念出来（用户要拿着名字去 App 找）。
#[tokio::test]
async fn prizes_lists_pending_items() {
    let body = ok(serde_json::json!({"records":[
        {"prizeName":"移动云盘1000万tokens叠加包","flag":1,"market":"sign_in_3_ex",
         "oid":1270932714,"prizeId":260630014,"expireTime":"2026-09-30T15:59:59.000+00:00"},
        {"prizeName":"已领过的","flag":0,"market":"sign_in_3_ex","oid":1,"prizeId":2}
    ]}));
    let (c, http) = Ctx::new(&[&body]);
    let out = PrizesTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert!(out.is_success(), "{out:?}");
    assert_eq!(http.count(), 1, "只读任务，一枪");
    assert!(http.url_at(0).contains("/ycloud/signin/public/record"));
    assert!(http.url_at(0).contains("marketname=sign_in_3"));
    assert!(
        out.summary().contains("1 个待领取"),
        "实得：{}",
        out.summary()
    );

    let logs = c.sink.logs();
    assert!(
        logs.iter()
            .any(|l| l.contains("移动云盘1000万tokens叠加包")),
        "要把奖品名字念出来：{logs:?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("2026-09-30")),
        "要念过期时间"
    );
}

/// ⭐ 只查第一页要**说出来**（旧实现静默截断 = "看起来没奖品，其实有"）。
#[tokio::test]
async fn prizes_admits_it_only_read_the_first_page() {
    let (c, _http) = Ctx::new(&[&ok(serde_json::json!({"records":[]}))]);
    let out = PrizesTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        out.summary().contains("只查了第一页"),
        "要如实说明覆盖范围：{}",
        out.summary()
    );
}

// ══════════════════════════ cloud-snapshot ══════════════════════════

/// ⭐ 快照报的是 `total`（现行余额），**不是** `beforeTotal`。
///
/// 实拍里两个数不相等。把 `beforeTotal` 当余额记进趋势图 = 一条直线。
#[tokio::test]
async fn snapshot_reports_total_not_before_total() {
    let (c, http) = Ctx::new(&[&info_body(false, 10496)]);
    let out = CloudSnapshotTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert_eq!(http.count(), 1, "只读");
    assert!(
        out.summary().contains("10496"),
        "要报 total：{}",
        out.summary()
    );
    assert!(
        !out.summary().contains("8027"),
        "beforeTotal 是冻结快照，不能当余额：{}",
        out.summary()
    );
}

// ══════════════════════════ task-list ══════════════════════════

/// 清单把三类账目都说出来。
#[tokio::test]
async fn task_list_reports_every_kind_of_item() {
    let info = info_with_receive(serde_json::json!([
        {"cloudType":0,"cloudNum":6},
        {"cloudType":1,"cloudNum":20,"acceptDate":"2026-09-30"},
        {"cloudType":2,"cloudNum":240}
    ]));
    let (c, http) = Ctx::new(&[&info]);
    let out = TaskListTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    assert_eq!(http.count(), 1, "只读");
    assert!(
        out.summary().contains("3 件可做"),
        "实得：{}",
        out.summary()
    );
    assert!(out.summary().contains("现在可领"));
    assert!(out.summary().contains("延期领取"));
    assert!(out.summary().contains("下月可领"));
}

/// 什么都没得做 ⇒ 成功 + 说清楚。
#[tokio::test]
async fn task_list_handles_the_empty_case() {
    let v = serde_json::json!({
        "code":0,"msg":"success",
        "result":{"total":100,"cal":[],"curMonthBackup":true,"curMonthBackupSignAccept":true}
    });
    let (c, _http) = Ctx::new(&[&v.to_string()]);
    let out = TaskListTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.is_success());
    assert!(
        out.summary().contains("没有可做的事"),
        "实得：{}",
        out.summary()
    );
}

/// 翻倍机会可用时要列进清单。
#[tokio::test]
async fn task_list_includes_the_doubling_opportunity() {
    let mut v: serde_json::Value = serde_json::from_str(&info_body(false, 10496)).unwrap();
    v["result"]["curMonthBackupSignAccept"] = serde_json::json!(false);
    v["result"]["receiveList"] = serde_json::json!([]);
    let (c, _http) = Ctx::new(&[&v.to_string()]);

    let out = TaskListTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        out.summary().contains("签到翻倍"),
        "实得：{}",
        out.summary()
    );
}

// ══════════════════════════ 全链吞错的防线 ══════════════════════════

/// ⭐ 每个任务在上游报错时都必须**变成失败**，不能吞掉。
///
/// 旧实现每个失败分支只打日志、从不抛错 ⇒ 对 runner 永远"成功"，
/// 于是定时任务能在"天天报错"的情况下绿灯一个月。
#[tokio::test]
async fn every_task_surfaces_upstream_errors() {
    let err = r#"{"code":500,"msg":"系统繁忙"}"#;

    // sign：第一枪查状态就失败
    let (c, _) = Ctx::new(&[err]);
    assert!(SignTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());

    let (c, _) = Ctx::new(&[err]);
    assert!(ReceiveTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());

    let (c, _) = Ctx::new(&[err]);
    assert!(MultipleTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());

    let (c, _) = Ctx::new(&[err]);
    assert!(BackupGiftTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());

    let (c, _) = Ctx::new(&[err]);
    assert!(PrizesTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());

    let (c, _) = Ctx::new(&[err]);
    assert!(CloudSnapshotTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());

    let (c, _) = Ctx::new(&[err]);
    assert!(TaskListTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());

    // revival 是唯一例外：它的失败文案要经过"是不是已领取"那一关，
    // 而"系统繁忙"认不出 ⇒ 仍按失败。
    let (c, _) = Ctx::new(&[err]);
    assert!(RevivalTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .is_err());
}

/// 八个任务的名字/标题互不重复，且与注册表一致。
#[test]
fn task_metadata_is_complete_and_distinct() {
    let metas = [
        SignTask.meta(),
        ReceiveTask.meta(),
        MultipleTask.meta(),
        BackupGiftTask.meta(),
        RevivalTask.meta(),
        PrizesTask.meta(),
        CloudSnapshotTask.meta(),
        TaskListTask.meta(),
    ];

    let names: Vec<String> = metas.iter().map(|m| m.name.clone()).collect();
    assert_eq!(
        names,
        vec![
            "sign",
            "receive",
            "multiple",
            "backup-gift",
            "revival",
            "prizes",
            "cloud-snapshot",
            "task-list"
        ],
        "名字是 wire 契约（编排与定时都按它指）"
    );
    for m in &metas {
        assert!(!m.title.is_empty(), "{} 缺标题", m.name);
        assert!(!m.description.is_empty(), "{} 缺说明", m.name);
    }
}

// ── 最小替身 ────────────────────────────────────────────────────────────

/// 这批任务都不碰 store；给一个**必然报错**的替身。
///
/// 于是"哪天有人给签到族加了一句读 store"会立刻炸出来，
/// 而不是静默拿到一个空值、然后断言失败在一个很别扭的地方。
///
/// `Store` 是一组子 trait 的**超 trait**（`AccountStore + RunStore + …`），
/// 所以要实现的是那 6 个子 trait，而不是 `Store` 本身。
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

/// ⭐ **自动领取的批处理语义**：一条失败**不该**拖垮其余条目。
///
/// 参考实现（`mcloud` 的 `ReceiveAll`）对每一条是 `perr != nil ⇒ 记一笔
/// ReceiveOutcomeFailed、continue`，即**逐条独立**。理由：`receiveList`
/// 里可能同时有「现在可领」与「已到期延期」两档，而它们是**互不相干**的
/// 两笔账 —— 第一笔因为某种原因失败（例如它自己那条已被领过、或上游偶发
/// 5xx），没有理由让第二笔也跟着不发。
///
/// ⚠ 本仓此前是 `?` 直接上抛 ⇒ **第一条失败整批中止**。用户看到的是
/// "领取失败"，而实际上后面那笔**本来能领到** —— 这是"少领了却说失败"
/// 的那一侧错误，代价是该领的没领、且下轮要等一整天。
#[tokio::test]
async fn receive_keeps_going_when_one_item_fails() {
    let info = info_with_receive(serde_json::json!([
        {"cloudType":0,"cloudNum":6,"recordId":"R1"},
        {"cloudType":0,"cloudNum":9,"recordId":"R2"}
    ]));
    let (c, http) = Ctx::new(&[
        &info,
        // 第一笔失败（非零码），第二笔成功。
        &fail("领取失败", 500),
        &ok(serde_json::json!({"receive":9})),
    ]);
    let out = ReceiveTask
        .run(&c.ctx(), &serde_json::json!({}))
        .await
        .unwrap();

    // ⭐ 关键断言：**第二笔仍然发出去了**（共 3 枪：1 查 + 2 领）。
    assert_eq!(
        http.count(),
        3,
        "一条失败不该中止整批 —— 后续条目仍应投递；实得 URL：{:?}",
        http.requests()
            .iter()
            .map(|r| r.url.clone())
            .collect::<Vec<_>>()
    );
    // 领到的 9 朵要被如实计入。
    assert!(out.summary().contains('9'), "实得：{}", out.summary());
}
