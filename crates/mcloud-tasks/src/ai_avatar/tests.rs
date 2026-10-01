//! AI 头像任务层的测试。
//!
//! 夹具直接抄 `play_ai/tests.rs` 那一套（`Scripted` / `FixedClock` /
//! `CollectSink` / `NoStore`），多三样能力：**能回非 200**、**能在第 N 枪之后
//! 取消**、**能把非 UTF-8 的原始响应字节交回**（`Reply::Raw` + 覆写
//! `request_raw`）。这三样正是本文件要钉的东西。
//!
//! 钉六类：
//!
//! * 开关关掉 ⇒ **0 个请求**（用一个「谁来都打挂」的客户端证明它一枪都没发）。
//! * 只读路径（余量 / 抽奖）与写路径（合成 / 上传）分开验证。
//! * 下载回来的**原始字节**一路走到 upload，且 base64 **解出来等于**原字节。
//! * 下载回来但形状不是图 ⇒ 如实失败，**不 upload**。
//! * 收尾**只删本趟造的 fileId**：预置一张同名的旧文件，断言它还在。
//! * 上游非 200（404/405）如实失败，**不静默**；取消在安全点生效且不再发请求。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

/// 测试用节奏：三处等待全 0、每轮只试一次、原图 2×2（省时间，形状不重要）。
fn plan_fast(side: u32) -> AiAvatarPlan {
    AiAvatarPlan {
        poll_max: AI_AVATAR_POLL_MAX,
        poll_interval_ms: 0,
        round_interval_ms: 0,
        draw_interval_ms: 0,
        max_attempts: 1,
        image_side: side,
    }
}

/// 固定随机源的任务（`SeqRandom` ⇒ 原图逐字节可复现）。
fn task_with(plan: AiAvatarPlan) -> AiAvatarTask {
    AiAvatarTask {
        plan,
        rng: SharedRandom::new(Box::new(mcloud_crypto::SeqRandom::new(7))),
    }
}

fn account_with(enabled: bool, limit: u32) -> Account {
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
        settings: AccountSettings {
            ai_avatar_enabled: enabled,
            ai_avatar_daily_limit: limit,
            ..Default::default()
        },
    }
}

const CATALOG_BODY: &str =
    r#"{"code":0,"msg":"success","result":{"fullPath_sub":"/a/rabbityearpic"}}"#;
const EMPTY_DIR_BODY: &str =
    r#"{"code":0,"success":true,"message":"请求成功","data":{"items":[]}}"#;

// ══════════════════════════ 夹具 ══════════════════════════

/// 一枪的答复：给个 200 的体、给一个**任意状态码**的响应、交回一串
/// **原始字节**（`request_raw` 专用），或者**打挂**（网络层错误）。
#[derive(Clone)]
enum Reply {
    Body(String),
    Response {
        status: u16,
        body: String,
        headers: Vec<(String, String)>,
    },
    /// `request_raw` 交回的字节 —— 可以是**非 UTF-8**（`Body(String)` 装不下）。
    Raw(Vec<u8>),
    Fail,
}

/// 按顺序答复的假 HTTP 客户端（用尽后重复最后一个）。
///
/// 比 `play_ai` 那一份多三样：`Response`（要测非 200）、`cancel_after`
/// （要测「取消之后不再发请求」）与 `request_raw`（要测**原始图片字节**
/// 能一路走到 upload —— `HttpResponse.body` 是 `String`，装不下它）。
struct Scripted {
    seen: Mutex<Vec<HttpRequest>>,
    replies: Mutex<Vec<Reply>>,
    /// (第几枪之后, 令牌)。第 N 枪**答复之后**调 `cancel()`。
    cancel_after: Mutex<Option<(usize, CancelToken)>>,
}

impl Scripted {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            // 反转，便于从队尾取
            replies: Mutex::new(replies.into_iter().rev().collect()),
            cancel_after: Mutex::new(None),
        }
    }

    fn cancel_after(&self, nth: usize, token: CancelToken) {
        *self.cancel_after.lock().unwrap() = Some((nth, token));
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }

    /// 记下这一枪，返回「这是第几枪」（取消判据在答复之后生效）。
    fn record(&self, req: HttpRequest) -> usize {
        let mut seen = self.seen.lock().unwrap();
        seen.push(req);
        seen.len()
    }

    /// 第 `nth` 枪答复之后按需取消。
    fn maybe_cancel(&self, nth: usize) {
        if let Some((n, token)) = self.cancel_after.lock().unwrap().as_ref() {
            if nth >= *n {
                token.cancel();
            }
        }
    }

    /// 取下一枪的答复（反转存过，从队尾取；用尽后重复最后一个）。
    fn next_reply(&self) -> Reply {
        let mut r = self.replies.lock().unwrap();
        if r.len() > 1 {
            r.pop().unwrap()
        } else {
            r.last().cloned().unwrap_or(Reply::Body("{}".into()))
        }
    }
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        let nth = self.record(req);
        self.maybe_cancel(nth);
        match self.next_reply() {
            Reply::Body(body) => Ok(HttpResponse {
                status: 200,
                body,
                headers: Vec::new(),
            }),
            Reply::Response {
                status,
                body,
                headers,
            } => Ok(HttpResponse {
                status,
                body,
                headers,
            }),
            // `request_raw` 专用的答复被 `request()` 取到：只可能是测试脚本把
            // 枪序写错了。按有损文本交回（不 panic），让断言去暴露它。
            Reply::Raw(bytes) => Ok(HttpResponse {
                status: 200,
                body: String::from_utf8_lossy(&bytes).into_owned(),
                headers: Vec::new(),
            }),
            Reply::Fail => Err(mcloud_core::Error::Other("网络断了".into())),
        }
    }

    /// ⚠ 必须覆写：`HttpResponse.body` 是 `String`，装不下图片那串非 UTF-8 字节。
    /// 这一条交回的是**线上原样字节**，正是 `ReqwestClient::request_raw` 的替身。
    async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
        let nth = self.record(req);
        self.maybe_cancel(nth);
        match self.next_reply() {
            Reply::Raw(bytes) => Ok(bytes),
            // 文本答复被这一条取到（其余用例的下载可能走它）：交回它的字节。
            Reply::Body(body) | Reply::Response { body, .. } => Ok(body.into_bytes()),
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

/// 造一个 ctx。时钟给 0 —— 本链**不使用**墙钟（等待是注入的字段）。
fn ctx_of(
    http: Arc<Scripted>,
    acct: Account,
    cancel: CancelToken,
) -> (TaskContext, Arc<Scripted>, Arc<CollectSink>) {
    let sink = Arc::new(CollectSink::default());
    let ctx = TaskContext {
        account: acct,
        http: http.clone(),
        store: Arc::new(NoStore),
        clock: Arc::new(FixedClock(0)),
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

fn ctx_with(replies: Vec<Reply>, acct: Account) -> (TaskContext, Arc<Scripted>, Arc<CollectSink>) {
    ctx_of(Arc::new(Scripted::new(replies)), acct, CancelToken::new())
}

fn summary_of(out: &TaskOutcome) -> String {
    match out {
        TaskOutcome::Success { summary } => summary.clone(),
        TaskOutcome::Failed { reason } => reason.clone(),
        TaskOutcome::Skipped { reason } => reason.clone(),
    }
}

// ═══════════════════ (a) 开关关掉 ⇒ 0 个请求 ═══════════════════

#[tokio::test]
async fn the_switch_off_sends_zero_requests() {
    // 客户端「谁来都打挂」：只要发过一枪，这条就会红。
    let (ctx, http, sink) = ctx_with(vec![Reply::Fail], account_with(false, 10));
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("开关关掉不该是 Err");
    assert!(matches!(out, TaskOutcome::Skipped { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 0, "关掉就要一枪都不发");
    let details = sink.details();
    assert_eq!(details.len(), 1);
    assert_eq!(details[0]["verdict"], "skipped");
}

/// 默认设置就是关的（`account.rs:184`）—— 这条钉住「默认不动用户网盘」。
#[test]
fn the_default_settings_keep_this_task_off() {
    let s = AccountSettings::default();
    assert!(!s.ai_avatar_enabled);
    assert_eq!(
        s.ai_avatar_daily_limit, 10,
        "默认 10 轮，与旧 TS schema 一致"
    );
}

// ═══════════════════ (b) 只读路径 ═══════════════════

#[tokio::test]
async fn the_read_path_sends_only_the_two_get_endpoints() {
    let (ctx, http, sink) = ctx_with(
        vec![
            Reply::Body(r#"{"code":0,"msg":"success","result":2}"#.into()),
            Reply::Body(r#"{"code":0,"result":{"contentName":"云朵"}}"#.into()),
            Reply::Body(r#"{"code":0,"msg":"success"}"#.into()),
        ],
        // 轮数 0 ⇒ 不取基线、不生成：这一趟只有余量与抽奖两枪。
        account_with(true, 0),
    );
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("只读链不该 Err");
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 3, "1 枪余量 + 2 枪抽奖");

    let reqs = http.requests();
    assert_eq!(
        reqs[0].url,
        "https://m.mcloud.139.com/market/rabactive/image/getHashLeft"
    );
    assert_eq!(
        reqs[1].url,
        "https://m.mcloud.139.com/market/rabactive/image/draw"
    );
    assert_eq!(reqs[2].url, reqs[1].url, "第二枪还是 draw");
    for r in &reqs {
        assert_eq!(r.method, "GET");
        for forbidden in ["merge", "upload", "ai/image", "trash", "file/list"] {
            assert!(
                !r.url.contains(forbidden),
                "这一趟不该出现写/网盘路径 {forbidden}：{}",
                r.url
            );
        }
    }

    let d = &sink.details()[0];
    assert_eq!(d["drawLeft"], 2);
    assert_eq!(d["drawAttempted"], 2);
    assert_eq!(d["drawWon"], 2);
    assert_eq!(d["drawFallbackUsed"], false);
    assert_eq!(d["prizes"][0], "云朵");
}

// ═══════════════════ (b) 写路径 ═══════════════════

/// 一张含**非 UTF-8 字节**的真图片数据：JPEG 魔数 `FF D8 FF` 后跟 `0x00 0x80`。
///
/// ⚠ 这几枚字节**故意**不是合法 UTF-8：只要有哪一步偷偷走了 `String`
/// （有损 UTF-8），它们就会被换成 `U+FFFD`，下面「逐字节相等」的断言立刻红。
fn jpeg_bytes() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0x00, 0x80, 0x11, 0xC0, 0xFE]
}

/// ⭐ 这一条钉两件事：链路**走到了 upload**，且 upload 体里的 `mergeImage`
/// **base64 解出来逐字节等于**下载到的那张图。
///
/// 有损那条路的失效点正在这里：[`mcloud_core::HttpResponse::body`] 是 `String`，
/// 走 `reqwest::Response::text()` 时 `FF D8 FF 00 80 …` 会被替换成 `U+FFFD`，
/// 上传的是一张坏图；只有真的 `request_raw`（`.bytes()`）才对得上。
#[tokio::test]
async fn the_downloaded_image_bytes_reach_upload_byte_for_byte() {
    let image = jpeg_bytes();
    let (ctx, http, sink) = ctx_with(
        vec![
            Reply::Body(CATALOG_BODY.into()),
            Reply::Body(EMPTY_DIR_BODY.into()),
            Reply::Body(r#"{"code":0,"result":{"taskId":"T1"}}"#.into()),
            Reply::Body(
                r#"{"code":0,"result":{"status":3,"imageUrl":"https://cdn.example/a.jpg"}}"#.into(),
            ),
            // 下载那一枪：真字节，不是文本。
            Reply::Raw(image.clone()),
            // upload 回执：给了 id ⇒ 链路确实走完了上传。
            Reply::Body(r#"{"code":0,"result":"U-1"}"#.into()),
            // 余量 0 ⇒ 不抽奖。
            Reply::Body(r#"{"code":0,"result":0}"#.into()),
            // 收尾：作品表 → 列目录 → 回收站。
            Reply::Body(
                r#"{"code":0,"result":[{"id":"U-1","imgUrl":"https://x/p/AAA.jpg"}]}"#.into(),
            ),
            Reply::Body(
                r#"{"code":0,"success":true,"data":{"items":[{"fileId":"NEW","name":"AAA.jpg"}]}}"#
                    .into(),
            ),
            Reply::Body(r#"{"code":"0000","success":true}"#.into()),
        ],
        account_with(true, 1),
    );
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("这一趟不该是 Err");
    assert!(
        matches!(out, TaskOutcome::Success { .. }),
        "生成 1 轮并上传成功 ⇒ success：{out:?}"
    );

    let reqs = http.requests();
    assert_eq!(
        http.count(),
        10,
        "目录 + 基线 + merge + getStatus + 下载 + upload + 余量 + 作品表 + 列目录 + 回收站，实得：{:?}",
        reqs.iter().map(|r| r.url.clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        reqs[4].url, "https://cdn.example/a.jpg",
        "第五枪是裸 GET 下载"
    );

    let upload = reqs
        .iter()
        .find(|r| r.url.ends_with("/market/rabactive/image/upload"))
        .expect("链路必须走到 upload");
    let body: serde_json::Value =
        serde_json::from_str(upload.body_str().expect("upload 该带文本体")).expect("该是 JSON");
    let sent = body["mergeImage"].as_str().expect("mergeImage 是字符串");
    assert_eq!(
        mcloud_crypto::b64_decode(sent).expect("上传的 base64 该能解"),
        image,
        "⚠ 上传体里的 base64 必须逐字节等于下载到的图（有损解码会在这里被钉出来）"
    );
    // originImage 仍是本趟那张噪点原图，形状不变。
    let origin = body["originImage"].as_str().expect("originImage 是字符串");
    assert!(mcloud_crypto::b64_decode(origin).is_ok());

    let d = &sink.details()[0];
    assert_eq!(d["generated"], 1);
    assert_eq!(d["generateFailures"], 0);
}

/// 下载回来但**形状不是图**（或空体）⇒ 本轮如实失败、**一枪 upload 都不发**。
///
/// 这是形状校验的下游：`request_raw` 本身不挑内容，判据在这里。
/// ⚠ 之所以不「尽力而为」：把来路不明的字节 base64 上去，上游仍可能回 `code:0`，
/// 结果是网盘里多一张打不开的图 —— 本仓最忌讳的那类失效。
#[tokio::test]
async fn a_downloaded_blob_that_is_not_an_image_never_reaches_upload() {
    let (ctx, http, sink) = ctx_with(
        vec![
            Reply::Body(CATALOG_BODY.into()),
            Reply::Body(EMPTY_DIR_BODY.into()),
            Reply::Body(r#"{"code":0,"result":{"taskId":"T1"}}"#.into()),
            Reply::Body(
                r#"{"code":0,"result":{"status":3,"imageUrl":"https://cdn.example/a"}}"#.into(),
            ),
            Reply::Raw(b"<html>not an image</html>".to_vec()),
            Reply::Body(r#"{"code":0,"result":0}"#.into()),
        ],
        account_with(true, 1),
    );
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("业务层面的失败，不该是 Err");
    assert!(
        matches!(out, TaskOutcome::Failed { .. }),
        "生成 0/1 ⇒ 必须如实报失败：{out:?}"
    );
    assert!(
        http.requests()
            .iter()
            .all(|r| !r.url.ends_with("/image/upload")),
        "形状不对 ⇒ upload 一枪都不该发"
    );
    let d = &sink.details()[0];
    assert_eq!(d["generated"], 0);
    assert_eq!(d["generateFailures"], 1);
    assert!(
        d["failures"][0].as_str().unwrap_or("").contains("形状"),
        "失败原因要说清是形状校验：{}",
        d["failures"][0]
    );
}

/// 形状校验本身：JPEG / PNG 收，空体与别的内容拒。
#[test]
fn the_image_shape_check_accepts_jpeg_and_png_and_rejects_everything_else() {
    let jpeg = jpeg_bytes();
    assert_eq!(
        image_download_base64(&jpeg).unwrap(),
        mcloud_crypto::b64_encode(&jpeg)
    );
    let png = vec![0x89, b'P', 0x4E, 0x47, 0x0D, 0x0A];
    assert_eq!(
        image_download_base64(&png).unwrap(),
        mcloud_crypto::b64_encode(&png)
    );
    assert!(image_download_base64(&[]).is_err(), "空体没有可上传的东西");
    assert!(
        image_download_base64(b"hello").is_err(),
        "不是图的字节不许 base64 上去"
    );
}

/// 业务码非 0（`code:500`）⇒ **本轮失败**，不是致命：会试第二次、然后继续抽奖。
#[tokio::test]
async fn a_business_non_zero_merge_is_a_round_failure_not_fatal() {
    let (ctx, http, sink) = ctx_with(
        vec![
            Reply::Body(CATALOG_BODY.into()),
            Reply::Body(EMPTY_DIR_BODY.into()),
            Reply::Body(r#"{"code":500,"msg":"系统繁忙","result":{}}"#.into()),
            Reply::Body(r#"{"code":500,"msg":"系统繁忙","result":{}}"#.into()),
            Reply::Body(r#"{"code":0,"result":0}"#.into()),
        ],
        account_with(true, 1),
    );
    let plan = AiAvatarPlan {
        max_attempts: 2,
        ..plan_fast(2)
    };
    let out = task_with(plan)
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("业务码不是致命");
    assert!(matches!(out, TaskOutcome::Failed { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 5, "同一轮试两次，然后照旧去读余量");

    let d = &sink.details()[0];
    assert_eq!(d["generateFailures"], 1);
    let failure = d["failures"][0].as_str().unwrap_or("");
    assert!(failure.contains("500"), "实得：{failure}");
    assert!(failure.contains("系统繁忙"), "实得：{failure}");
}

// ═══════════════════ (d) 非 200：如实上抛 ═══════════════════

#[tokio::test]
async fn a_non_200_on_merge_is_fatal_and_not_swallowed() {
    let (ctx, http, _) = ctx_with(
        vec![
            Reply::Body(CATALOG_BODY.into()),
            Reply::Body(EMPTY_DIR_BODY.into()),
            Reply::Response {
                status: 405,
                body: "Method Not Allowed".into(),
                headers: Vec::new(),
            },
        ],
        account_with(true, 1),
    );
    let err = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("405 必须上抛");
    let s = err.to_string();
    assert!(s.contains("405"), "实得：{s}");
    assert!(s.contains("Method Not Allowed"), "实得：{s}");
    assert_eq!(http.count(), 3, "挂了就停：不抽奖、不列目录");
}

#[tokio::test]
async fn a_non_200_on_draw_is_fatal() {
    let (ctx, http, _) = ctx_with(
        vec![
            Reply::Body(r#"{"code":0,"result":3}"#.into()),
            Reply::Response {
                status: 404,
                body: "not found".into(),
                headers: Vec::new(),
            },
        ],
        account_with(true, 0),
    );
    let err = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("404 必须上抛");
    assert!(err.to_string().contains("404"), "实得：{err}");
    assert_eq!(http.count(), 2, "挂了就不再抽第二次");
}

/// ⚠ 与旧 TS 的**刻意分歧**：余量那一枪非 200 ⇒ 上抛，
/// **不**拿兜底 5 次去连打五枪（那会把「这口没了」洗成「抽了 5 次都失败」）。
#[tokio::test]
async fn a_non_200_on_hash_left_is_fatal_not_a_fallback() {
    let (ctx, http, _) = ctx_with(
        vec![Reply::Response {
            status: 404,
            body: "not found".into(),
            headers: Vec::new(),
        }],
        account_with(true, 0),
    );
    let err = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("404 必须上抛");
    assert!(err.to_string().contains("404"), "实得：{err}");
    assert_eq!(http.count(), 1, "上抛之后一枪都不许再发");
}

/// 业务码失败（不是非 200）⇒ 照旧 TS 用兜底，但**要在小结里说出来**。
#[tokio::test]
async fn a_business_failure_on_hash_left_uses_the_fallback_and_says_so() {
    let mut replies = vec![Reply::Body(r#"{"code":500,"msg":"系统繁忙"}"#.into())];
    for _ in 0..AI_AVATAR_DRAW_FALLBACK {
        replies.push(Reply::Body(r#"{"code":0,"msg":"success"}"#.into()));
    }
    let (ctx, http, sink) = ctx_with(replies, account_with(true, 0));
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("业务码不是致命");
    assert_eq!(http.count(), 1 + AI_AVATAR_DRAW_FALLBACK as usize);

    let d = &sink.details()[0];
    assert_eq!(d["drawFallbackUsed"], true);
    assert_eq!(d["drawWon"], AI_AVATAR_DRAW_FALLBACK);
    let summary = summary_of(&out);
    assert!(summary.contains("兜底"), "兜底必须写进小结：{summary}");
    assert!(
        summary.contains("猜测"),
        "并且说清它是猜的不是读数：{summary}"
    );
}

/// `code == 408`：停止那一档 —— 不再抽，也**不算失败**。
#[tokio::test]
async fn a_408_stops_the_draw_loop() {
    let (ctx, http, sink) = ctx_with(
        vec![
            Reply::Body(r#"{"code":0,"result":3}"#.into()),
            Reply::Body(r#"{"code":408,"msg":"次数已用完"}"#.into()),
        ],
        account_with(true, 0),
    );
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("408 不是 Err");
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 2, "408 之后不再抽");
    assert_eq!(sink.details()[0]["drawStopped"], true);
    assert!(
        summary_of(&out).contains("叫停"),
        "实得：{}",
        summary_of(&out)
    );
}

/// 其余非 0 码 ⇒ 记一句**继续抽**（旧 TS 的 `continue`）。
#[tokio::test]
async fn any_other_draw_code_is_logged_and_the_loop_continues() {
    let (ctx, http, sink) = ctx_with(
        vec![
            Reply::Body(r#"{"code":0,"result":2}"#.into()),
            Reply::Body(r#"{"code":400,"msg":"这次没中"}"#.into()),
            Reply::Body(r#"{"code":0,"result":{"prizeName":"AI豆"}}"#.into()),
        ],
        account_with(true, 0),
    );
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("非 0 不是 Err");
    assert_eq!(http.count(), 3, "失败那次不 break");
    let d = &sink.details()[0];
    assert_eq!(d["drawAttempted"], 2, "两次都发了枪");
    assert_eq!(d["drawWon"], 1);
    assert_eq!(d["prizes"][0], "AI豆");
    assert!(
        d["failures"][0].as_str().unwrap_or("").contains("400"),
        "逐条失败要带码"
    );
    assert!(matches!(out, TaskOutcome::Success { .. }));
}

// ═══════════════════ (e) 取消 ═══════════════════

#[tokio::test]
async fn a_cancelled_token_stops_before_any_request() {
    let token = CancelToken::new();
    token.cancel();
    let (ctx, http, _) = ctx_of(
        Arc::new(Scripted::new(vec![Reply::Fail])),
        account_with(true, 1),
        token,
    );
    let err = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("已取消 ⇒ Cancelled");
    assert!(err.is_cancelled(), "实得：{err}");
    assert_eq!(http.count(), 0, "取消之后一枪都不发");
}

/// 取消发生在**两枪之间** ⇒ 循环在下一个安全点退出，且不再发请求。
#[tokio::test]
async fn a_cancel_between_shots_stops_further_requests() {
    let token = CancelToken::new();
    let http = Arc::new(Scripted::new(vec![Reply::Body(
        r#"{"code":0,"result":3}"#.into(),
    )]));
    http.cancel_after(1, token.clone());
    let (ctx, http, _) = ctx_of(http, account_with(true, 0), token);
    let err = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("取消 ⇒ Cancelled");
    assert!(err.is_cancelled(), "实得：{err}");
    assert_eq!(http.count(), 1, "余量那一枪之后就被叫停，抽奖一枪都不发");
}

// ═══════════════════ (c) 收尾护栏：只删本趟造的 fileId ═══════════════════

/// ⭐ 这条是 P0-4 的护栏：**预置一张同名的旧文件**，断言它**没有**被送进回收站。
///
/// # 结论（红过再绿，实测过）
///
/// 把 `let pre_existing = baseline.contains(&entry.file_id);` 临时改成
/// `let pre_existing = false;`（= 照旧 TS 只按名字匹配），这条用例**实测会红**，
/// 逐字读数是：
///
/// ```text
/// test ai_avatar::tests::cleanup_only_trashes_file_ids_created_by_this_run ... FAILED
/// assertion `left == right` failed
///   left: ["NEW", "OLD"]
///  right: ["NEW"]
/// ```
///
/// 也就是说：同名旧文件会被一起登记进账本 ⇒ 下一枪 `batchTrash` 会把它也删掉。
/// 这正是旧 TS `task.ts:12-36` 的行为（活动目录里本来就有同名文件时，
/// 它会被一起删掉），而 TS 没有任何 pre-existing 判据。
/// 本模块的先基线后匹配把它挡在账本之外（`pre_existing=true` 时
/// `register_cloud_file` 返回 `false`，既不进账本也不进清理）。
#[tokio::test]
async fn cleanup_only_trashes_file_ids_created_by_this_run() {
    let (ctx, http, _) = ctx_with(
        vec![
            // ① 作品表：本趟的 uploadId W1 对应文件 AAA.png
            Reply::Body(
                r#"{"code":0,"result":[{"id":"W1","imgUrl":"https://x/p/AAA.png"}]}"#.into(),
            ),
            // ② 动笔之后列目录：本趟那张 + 一张**同名**的旧文件
            Reply::Body(
                r#"{"code":0,"success":true,"data":{"items":[
                    {"fileId":"NEW","name":"AAA.png"},
                    {"fileId":"OLD","name":"AAA.png"}]}}"#
                    .into(),
            ),
            // ③ batchTrash 回执
            Reply::Body(r#"{"code":"0000","success":true}"#.into()),
        ],
        account_with(true, 1),
    );

    // 基线：动笔之前，那个目录里只有 OLD。
    let mut baseline = HashSet::new();
    baseline.insert("OLD".to_string());

    let mut result = AiAvatarResult::default();
    collect_and_clean(
        &ctx,
        &["W1".to_string()],
        Some(&baseline),
        Some("/rabbityearpic"),
        &mut result,
    )
    .await;

    assert_eq!(result.registered_file_ids, vec!["NEW".to_string()]);
    assert_eq!(result.pre_existing_skipped, 1, "同名旧文件要单独计数");
    assert_eq!(result.cleaned_up, 1);
    assert!(result.cleanup_error.is_empty(), "{}", result.cleanup_error);
    assert!(result.summary_line().contains("同名旧文件跳过 1 个，不删"));

    let reqs = http.requests();
    assert_eq!(reqs.len(), 3, "作品表 + 列目录 + 回收站");
    assert_eq!(
        reqs[2].url,
        "https://personal-kd-njs.yun.139.com/hcy/recyclebin/batchTrash"
    );
    let body = reqs[2].body_str().expect("batchTrash 有体");
    assert!(body.contains("NEW"), "本趟造的必须进回收站：{body}");
    assert!(
        !body.contains("OLD"),
        "⚠ 同名的**旧文件**绝不能进回收站（那会删用户的东西）：{body}"
    );
}

/// 基线缺（活动目录没读到）⇒ **整个收尾跳过**，一枪都不发。
#[tokio::test]
async fn without_a_baseline_the_cleanup_is_skipped_entirely() {
    let (ctx, http, _) = ctx_with(vec![Reply::Fail], account_with(true, 1));
    let mut result = AiAvatarResult::default();
    collect_and_clean(&ctx, &["W1".to_string()], None, Some("/rab"), &mut result).await;
    assert_eq!(http.count(), 0, "拿不到基线就不该去列目录/删东西");
    assert!(result.registered_file_ids.is_empty());
    assert_eq!(result.cleaned_up, 0);
    assert!(
        result.notes.iter().any(|n| n.contains("宁可留下")),
        "跳过要说明白：{:?}",
        result.notes
    );
}

/// 收尾那几枪失败 ⇒ **落进 cleanup_error**（进小结），不是静默、也不上抛。
#[tokio::test]
async fn a_failed_cleanup_read_is_reported_in_the_cleanup_error() {
    let (ctx, http, _) = ctx_with(
        vec![Reply::Response {
            status: 500,
            body: "boom".into(),
            headers: Vec::new(),
        }],
        account_with(true, 1),
    );
    let mut baseline = HashSet::new();
    baseline.insert("OLD".to_string());
    let mut result = AiAvatarResult::default();
    collect_and_clean(
        &ctx,
        &["W1".to_string()],
        Some(&baseline),
        Some("/rab"),
        &mut result,
    )
    .await;
    assert_eq!(http.count(), 1, "作品表挂了就停");
    assert!(
        result.cleanup_error.contains("500"),
        "实得：{}",
        result.cleanup_error
    );
    assert!(result.summary_line().contains("收尾失败"));
    assert_eq!(result.cleaned_up, 0);
}

/// 「uploadId 在作品表里一个都对不上」⇒ 没有可匹配的名字，跳过并说明。
#[tokio::test]
async fn a_work_id_that_matches_nothing_skips_the_cleanup() {
    let (ctx, http, _) = ctx_with(
        vec![Reply::Body(
            r#"{"code":0,"result":[{"id":"OTHER","imgUrl":"https://x/p/ZZZ.png"}]}"#.into(),
        )],
        account_with(true, 1),
    );
    let mut baseline = HashSet::new();
    baseline.insert("OLD".to_string());
    let mut result = AiAvatarResult::default();
    collect_and_clean(
        &ctx,
        &["W1".to_string()],
        Some(&baseline),
        Some("/rab"),
        &mut result,
    )
    .await;
    assert_eq!(http.count(), 1, "没有可匹配的名字就不去列目录");
    assert!(result.registered_file_ids.is_empty());
    assert!(
        result.notes.iter().any(|n| n.contains("一个都没对上")),
        "{:?}",
        result.notes
    );
}

// ═══════════════════ 明细 / 小结 ═══════════════════

#[tokio::test]
async fn the_details_and_summary_carry_the_non_idempotency_hedge() {
    let (ctx, _http, sink) = ctx_with(
        vec![Reply::Body(r#"{"code":0,"result":0}"#.into())],
        account_with(true, 0),
    );
    let out = task_with(plan_fast(2))
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect("不该 Err");
    assert_eq!(
        sink.details()[0]["nonIdempotentHedge"],
        AI_AVATAR_NON_IDEMPOTENT_HEDGE
    );
    assert!(
        summary_of(&out).contains(AI_AVATAR_NON_IDEMPOTENT_HEDGE),
        "小结也要带：{}",
        summary_of(&out)
    );
    // 余量读到 0 ⇒ 一枪都没抽；什么都没做 ⇒ skipped（不是失败）。
    assert!(matches!(out, TaskOutcome::Skipped { .. }), "实得：{out:?}");
}

// ═══════════════════ 原图 ═══════════════════

#[test]
fn the_noise_image_is_a_200x200_rgb_png() {
    let mut rng = mcloud_crypto::SeqRandom::new(1);
    let png = noise_png(AI_AVATAR_IMAGE_SIDE, AI_AVATAR_IMAGE_SIDE, &mut rng);
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    // IHDR 的载荷从第 16 字节起（8 签名 + 4 长度 + 4 类型）。
    assert_eq!(
        u32::from_be_bytes([png[16], png[17], png[18], png[19]]),
        200
    );
    assert_eq!(
        u32::from_be_bytes([png[20], png[21], png[22], png[23]]),
        200
    );
    assert_eq!(png[24], 8, "位深 8");
    assert_eq!(png[25], 2, "色型 2 = RGB（旧 TS 的 colortype）");
}

/// 同一起点的随机源 ⇒ 逐字节相同的图；base64 版本就是那串字节的 base64。
#[test]
fn the_noise_image_is_reproducible_from_its_random_source() {
    let a = noise_png(2, 2, &mut mcloud_crypto::SeqRandom::new(1));
    let b = noise_png(2, 2, &mut mcloud_crypto::SeqRandom::new(1));
    assert_eq!(a, b);
    let b64 = noise_png_b64(2, 2, &mut mcloud_crypto::SeqRandom::new(1));
    assert_eq!(mcloud_crypto::b64_decode(&b64).unwrap(), a);
    assert!(!b64.contains("data:image"), "裸 base64，不带 data URI 前缀");
}

// ═══════════════════ 注册表接入 ═══════════════════

#[test]
fn ai_avatar_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");
    let meta = registry
        .list()
        .into_iter()
        .find(|m| m.name == "ai-avatar")
        .expect("ai-avatar 必须注册（CLI/capabilities 都按 registry 走）");
    assert_eq!(meta.title, "AI 头像");
    assert!(meta.params.is_empty(), "这一条没有参数");
    assert!(!meta.hidden);
}

// ═══════════════════ 替身 ═══════════════════

/// 这一条链**不该碰 store** —— 一碰就报错的替身。
struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("AI 头像这条链不该碰 store".into())
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
