//! `app_task::runners` 的测试。
//!
//! 这一层管的是**一枪一枪的编排**，所以最贵的错误是"不该发的枪发出去了"
//! （写请求不可回滚）与"该发的没发"。用例因此**同时**钉两件事：
//! ① 结论（`acted` / `failures`）；② **请求条数与 URL**。
//!
//! ⚠ 107 那两条尤其重要：上游今天不再给 `headers.app_auth`（2026-10-07 实测）⇒
//! 后三枪**一枪都不该发**（旧 TS 那句 `return` 逐字）。这一条只有数请求数才钉得住。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use crate::app_task::tests::FakeStore;
use crate::app_task::{is_skipped_task, CLICK_ONLY_TASK_IDS};
use mcloud_core::testkit::FixedClock;
use mcloud_core::{
    Account, AccountId, AccountSettings, AuthInfo, CancelToken, DeviceId, HttpClient, HttpResponse,
};

// ══════════════════════════ 夹具 ══════════════════════════

/// 一枪的答复：状态码 + 正文。
#[derive(Clone)]
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
        }
    }
    fn status(code: u16) -> Self {
        Self {
            status: code,
            body: String::new(),
        }
    }
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
            replies: Mutex::new(replies.into_iter().rev().collect()),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }

    fn urls(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.url).collect()
    }

    fn hits(&self, needle: &str) -> usize {
        self.urls().iter().filter(|u| u.contains(needle)).count()
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
                r.last().cloned().unwrap_or_else(|| Reply::ok("{}"))
            }
        };
        Ok(HttpResponse {
            status: reply.status,
            body: reply.body,
            headers: Vec::new(),
        })
    }
}

fn account(user_domain_id: Option<&str>) -> Account {
    Account {
        id: AccountId::new("19****01"),
        nickname: None,
        auth: AuthInfo {
            raw: "Basic eA==".into(),
            platform: "pc".into(),
            phone: "19900000001".into(),
            token: "RAWTOKEN".into(),
            expire: 0,
        },
        device_id: DeviceId("DEV+1/base=".into()),
        pc_platform: None,
        user_domain_id: user_domain_id.map(str::to_string),
        owner: None,
        enabled: true,
        settings: AccountSettings::default(),
    }
}

fn ctx_with(replies: Vec<Reply>, user_domain_id: Option<&str>) -> (TaskContext, Arc<Scripted>) {
    let http = Arc::new(Scripted::new(replies));
    let ctx = TaskContext {
        account: account(user_domain_id),
        http: http.clone(),
        store: Arc::new(FakeStore::new()),
        clock: Arc::new(FixedClock(1_700_000_000_000)),
        sink: Arc::new(NullSink),
        run_id: mcloud_core::RunId(1),
        cancel: CancelToken::new(),
        chain_cancel: None,
        artifacts: mcloud_engine::ArtifactRecorder::new(
            Arc::new(mcloud_engine::ArtifactLedger::new()),
            mcloud_core::RunId(1),
            AccountId::new("19****01"),
        ),
        details: mcloud_engine::DetailsRecorder::new(
            mcloud_engine::DetailsLedger::new(),
            mcloud_core::RunId(1),
        ),
    };
    (ctx, http)
}

struct NullSink;
impl mcloud_core::EventSink for NullSink {
    fn emit(&self, _event: mcloud_core::RunEvent) {}
}

// ── 路由表（纯函数）────────────────────────────────────────────────────

/// id → 执行器的映射逐条对上旧 TS `runners.ts` 那张表。
#[test]
fn runner_table_matches_the_legacy_keys() {
    assert_eq!(AppRunner::for_task(107), Some(AppRunner::Note));
    assert_eq!(AppRunner::for_task(472), Some(AppRunner::MailActive));
    assert_eq!(AppRunner::for_task(473), Some(AppRunner::MailActive));
    assert_eq!(AppRunner::for_task(1004), Some(AppRunner::SendMail));
    assert_eq!(AppRunner::for_task(1014), Some(AppRunner::PdfConvert));
    assert_eq!(AppRunner::for_task(1020), Some(AppRunner::MailBackup));
    assert_eq!(AppRunner::for_task(1028), Some(AppRunner::AiWorkbench));
    assert_eq!(AppRunner::for_task(585), Some(AppRunner::AiCamera));
    // ⚠ 两个**真机 id 别名**（2026-10-07：585/1028 在两个真实账号的任务表里都不存在，
    //    同一个能力今天叫 618 / 1057）—— 按能力收，不按数字收 ✓。
    assert_eq!(AppRunner::for_task(1057), Some(AppRunner::AiWorkbench));
    assert_eq!(AppRunner::for_task(618), Some(AppRunner::AiCamera));
    // 609「和AI助手对话」（2026-10-11 加）：**两边都没有 runner** ⇒ 这一条是本仓补的
    //（真机读数：只打卡 `WAIT → WAIT` ✗，接上 AI 对话链才翻 ✓，见 §5-13）。
    assert_eq!(AppRunner::for_task(609), Some(AppRunner::AiAssistant));
    assert_eq!(
        AppRunner::for_task(547),
        None,
        "547 只打卡就翻 ⇒ 不进执行器表 ✓"
    );
    // 不在表里的 id 一律 None（**不许**兜底到某一个执行器）
    for id in [106, 113, 409, 434, 522, 1005, 115, 478, 481, 0, -1] {
        assert_eq!(AppRunner::for_task(id), None, "id={id} 不该有执行器");
    }
}

/// click-only 与"刻意跳过"那两串**不得有交集**。
///
/// 有交集时 `should_act` 的黑名单门会让这一档永远到不了 `Step::ClickOnly`
/// —— 那是安全的方向，但会让"两串互相矛盾"这件事**看不出来** ⇒ 在这里钉住。
#[test]
fn click_only_and_skip_lists_do_not_overlap() {
    for id in CLICK_ONLY_TASK_IDS {
        assert!(
            !is_skipped_task(Some(id)),
            "{id} 同时在 click-only 与 SKIP_TASK_LIST 里"
        );
    }
}

/// click-only 那一串逐字对上 TS 文档（`TASKS.md`）。
///
/// ⚠ 1017 **不在**这里（它走 `Step::PortalEntry`）—— 见 `docs/app-task-status.md` §5-11。
#[test]
fn click_only_ids_are_the_legacy_eight() {
    assert_eq!(
        CLICK_ONLY_TASK_IDS,
        [115, 431, 435, 1009, 1010, 1013, 1016, 447, 547]
    );
    // ⚠ 609 **不在**这一串里（它走 `AppRunner::AiAssistant` ✓ —— 打卡不翻账，见 §5-13）
    assert!(!crate::app_task::is_click_only(609));
    // ⚠ 1017 **不在**这一串里（它走 `Step::PortalEntry`，见 §5-11）✓
    assert!(!crate::app_task::is_click_only(1017));
    assert!(
        !crate::app_task::is_click_only(604),
        "604 也走入口页那一档 ✓"
    );
    assert!(
        !crate::app_task::is_click_only(1017),
        "1017 走入口页那一档，不进 click-only"
    );
}

// ── 107 创建云笔记 ─────────────────────────────────────────────────────

/// ⭐ 拿不到 `headers.app_auth` ⇒ **后三枪一枪都不发**（旧 TS 那句 `return` 逐字）。
///
/// 这是 2026-10-07 实测的那一支（上游不再给这一格）。
#[tokio::test]
async fn note_runner_sends_nothing_after_a_failed_auth() {
    let (ctx, http) = ctx_with(vec![Reply::ok(r#"{"headers":{}}"#)], None);
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::Note, 107, &mut owned).await;

    assert!(!r.acted, "鉴权没过就不算做成");
    assert_eq!(http.count(), 1, "只该发鉴权那一枪：{:?}", http.urls());
    assert!(
        r.notes.iter().any(|n| n.contains("获取笔记鉴权失败")),
        "明细里要说明为什么跳过：{:?}",
        r.notes
    );
    assert!(owned.is_empty(), "没建笔记就不该有文件落账");
}

/// 鉴权过了 ⇒ 三枪（鉴权 → 建 → 删），并算做成。
#[tokio::test]
async fn note_runner_creates_then_trashes_the_note() {
    let (ctx, http) = ctx_with(
        vec![
            Reply::ok(r#"{"headers":{"app_auth":"A1"}}"#),
            Reply::ok(r#"{"code":0}"#),
            Reply::ok(r#"{"code":0}"#),
        ],
        None,
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::Note, 107, &mut owned).await;

    assert!(r.acted);
    assert_eq!(http.count(), 3, "三枪：{:?}", http.urls());
    assert_eq!(http.hits("authTokenRefresh.do"), 1);
    assert_eq!(http.hits("createNote.do"), 1);
    assert_eq!(http.hits("moveToRecycleBin.do"), 1);
    // 建/删两枪都要带上鉴权头那两枚（旧 TS 的 `...headers` 展开 ✓）
    let create = http
        .requests()
        .into_iter()
        .find(|r| r.url.contains("createNote.do"))
        .expect("有建笔记那一枪");
    assert!(
        create
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("app_auth") && v == "A1"),
        "建笔记那一枪要带上 app_auth"
    );
}

// ── 邮箱 session 缓存 ──────────────────────────────────────────────────

/// 邮箱登录那一发要按 SSO 的 `001003` 走，并且**一次运行只登一次**。
#[tokio::test]
async fn mail_session_is_logged_in_once_and_reused() {
    let (ctx, http) = ctx_with(
        vec![
            // ① querySpecTokenV2
            Reply::ok(r#"{"success":true,"code":"0000","data":{"token":"SSO"}}"#),
            // ② inlogin
            Reply::ok(r#"{"var":{"sid":"S1","rmkey":"R1"}}"#),
            // ③ 472 的 autologin（200 即可）
            Reply::ok(""),
            // ④ 472 的市场打卡（今天 404）
            Reply::status(404),
            // ⑤ 1004 的 getComposeId
            Reply::ok(r#"{"code":"S_OK","var":"CID"}"#),
            // ⑥ 1004 的 compose
            Reply::ok(r#"{"code":"S_OK"}"#),
            // ⑦ PE 上报
            Reply::ok(r#"{"code":"S_OK"}"#),
            // ⑧ 1004 的市场打卡
            Reply::status(404),
        ],
        None,
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();

    let a = run_runner(&ctx, &mut state, AppRunner::MailActive, 472, &mut owned).await;
    let b = run_runner(&ctx, &mut state, AppRunner::SendMail, 1004, &mut owned).await;

    assert_eq!(
        http.hits("querySpecTokenV2"),
        1,
        "邮箱登录只该发生一次：{:?}",
        http.urls()
    );
    assert_eq!(http.hits("inlogin.action"), 1);
    // 472 那一发打卡 404 ⇒ 不算做成；autologin 打过了要如实记
    assert!(!a.acted, "打卡 404 ⇒ 这一条没做成");
    assert!(a.notes.iter().any(|n| n.contains("404")), "{:?}", a.notes);
    // 1004 发信成功 ⇒ 做成
    assert!(b.acted, "发信成功该算做成：{:?}", b.notes);
    assert!(
        b.notes.iter().any(|n| n.contains("邮件发送成功")),
        "{:?}",
        b.notes
    );
    // 两个 marketname 各出现在自己那一枪里
    assert!(http.urls().iter().any(|u| u.contains("taskClickV2")));
}

/// 取不到 composeId ⇒ 不发信、不算做成（旧 TS 那句 `fail(...)` + `return`）。
#[tokio::test]
async fn send_mail_stops_when_compose_id_is_missing() {
    let (ctx, http) = ctx_with(
        vec![
            Reply::ok(r#"{"success":true,"code":"0000","data":{"token":"SSO"}}"#),
            Reply::ok(r#"{"var":{"sid":"S1","rmkey":"R1"}}"#),
            Reply::ok(r#"{"code":"S_OK"}"#),
        ],
        None,
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::SendMail, 1004, &mut owned).await;

    assert!(!r.acted);
    assert_eq!(http.hits("mbox:compose"), 0, "取不到 composeId 就不该发信");
    assert!(
        r.notes.iter().any(|n| n.contains("取不到 composeId")),
        "{:?}",
        r.notes
    );
}

// ── 1020 邮件转存（App 实拍三发；旧路 404）────────────────────────────

/// ⭐ 三发都成 ⇒ 做成；三发的**形状**逐条钉住，且旧那条死路**一枪不发** ✓。
#[tokio::test]
async fn mail_backup_walks_the_app_chain_and_counts_only_the_transfer() {
    let (ctx, http) = ctx_with(
        vec![
            Reply::ok(r#"{"success":true,"code":"0000","data":{"token":"SSO"}}"#),
            Reply::ok(r#"{"var":{"sid":"S1","rmkey":"R1"}}"#),
            // ① 目录（形状是**合成**的：这一口的体零实拍，键名照 App 的字面量 ✓）
            Reply::ok(
                r#"<object><array name="directorys"><object><string name="directoryId">DIR1</string></object></array></object>"#,
            ),
            // ② 邮件列表
            Reply::ok(
                r#"<object><array name="messages"><object><string name="mid">MID1</string><int name="fid">1</int></object></array></object>"#,
            ),
            // ③ 转存
            Reply::ok(r#"{"code":"S_OK"}"#),
        ],
        None,
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::MailBackup, 1020, &mut owned).await;

    assert!(r.acted, "{:?}", r.failures);
    assert!(r.failures.is_empty(), "{:?}", r.failures);

    let urls = http.urls();
    assert!(
        urls.iter().any(|u| u.contains("func=disk:getDirectorys")),
        "{urls:?}"
    );
    assert!(
        urls.iter()
            .any(|u| u.contains("func=mbox:SearchByFidOrCondition")),
        "{urls:?}"
    );
    let backup = urls
        .iter()
        .find(|u| u.contains("func=disk:backupMail"))
        .expect("转存那一发");
    assert!(backup.contains("comefrom=16861"), "{backup}");
    assert_eq!(
        http.hits("mailsms/sms/backup"),
        0,
        "旧那条死路**不再发**（实测 404）"
    );

    // 体里要带上 ② 拿到的 `mid` 与 ① 拿到的目录 ✓
    let req = http
        .requests()
        .into_iter()
        .find(|r| r.url.contains("func=disk:backupMail"))
        .unwrap();
    let body = req.body_str().unwrap().to_string();
    assert!(body.contains(">MID1<"), "{body}");
    assert!(body.contains(">DIR1<"), "{body}");
    assert!(
        body.contains(r#"<string name="channel">16861</string>"#),
        "{body}"
    );
}

/// 取不到 `mid` ⇒ **硬失败**，且明细里要带上**体的键名形状**（这一口零实拍 ⇒ 靠它取证 ✓）。
#[tokio::test]
async fn a_mail_list_without_mid_fails_with_the_body_shape() {
    let (ctx, http) = ctx_with(
        vec![
            Reply::ok(r#"{"success":true,"code":"0000","data":{"token":"SSO"}}"#),
            Reply::ok(r#"{"var":{"sid":"S1","rmkey":"R1"}}"#),
            Reply::ok(r#"<object><string name="directoryId">DIR1</string></object>"#),
            Reply::ok(r#"<object><string name="somethingElse">X</string></object>"#),
        ],
        None,
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::MailBackup, 1020, &mut owned).await;

    assert!(!r.acted);
    assert_eq!(r.failures.len(), 1, "{:?}", r.failures);
    assert!(r.failures[0].contains("mid"), "{:?}", r.failures);
    assert!(
        r.failures[0].contains("somethingElse"),
        "要把体的键名形状带出来：{:?}",
        r.failures
    );
    assert_eq!(
        http.hits("func=disk:backupMail"),
        0,
        "拿不到 mid 就不该转存"
    );
}

/// 转存那一发被上游打回（业务码）⇒ **硬失败**，不是"提交过了" ✓。
#[tokio::test]
async fn a_rejected_transfer_is_a_hard_failure() {
    let (ctx, http) = ctx_with(
        vec![
            Reply::ok(r#"{"success":true,"code":"0000","data":{"token":"SSO"}}"#),
            Reply::ok(r#"{"var":{"sid":"S1","rmkey":"R1"}}"#),
            Reply::ok(r#"<object><string name="directoryId">DIR1</string></object>"#),
            Reply::ok(r#"<object><string name="mid">MID1</string></object>"#),
            Reply::ok(r#"{"code":"PML10410050","summary":"转存失败"}"#),
        ],
        None,
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::MailBackup, 1020, &mut owned).await;

    assert!(!r.acted);
    assert_eq!(r.failures.len(), 1, "{:?}", r.failures);
    assert!(r.failures[0].contains("PML10410050"), "{:?}", r.failures);
    assert_eq!(http.hits("func=disk:backupMail"), 1);
}

// ── 585 AI 相机 ────────────────────────────────────────────────────────

/// 识图成功 + 对话成功 ⇒ 做成；两枪的 URL 与鉴权头逐条对上旧 TS。
#[tokio::test]
async fn ai_camera_happy_path() {
    let (ctx, http) = ctx_with(
        vec![
            Reply::ok(r#"{"success":true,"data":{"fileId":"F1","taskId":123}}"#),
            Reply::ok(r#"data: {"success":true,"code":"0000"}"#),
        ],
        Some("1234567890123456789"),
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::AiCamera, 585, &mut owned).await;

    assert!(r.acted, "{:?}", r.notes);
    assert_eq!(http.count(), 2, "{:?}", http.urls());
    let rec = http.requests()[0].clone();
    assert_eq!(rec.url, ai_cloud::AI_RECOGNIZE_URL);
    // ⚠⚠ 识图那一枪的 Authorization 是**完整 auth**（`"Basic …"`），不是裸 token
    //（2026-10-07 真机对拍：裸 token ⇒ `鉴权失败` ✓）
    assert!(rec
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("Authorization") && v == "Basic eA=="));
    // ⚠ `deviceid` 必须带上（旧 TS 的 http 层按域名注入；少了它上游回「鉴权失败」）
    assert!(rec
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("deviceid") && !v.is_empty()));
    assert!(rec
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("x-yun-uni") && v == "1234567890123456789"));

    let chat = http.requests()[1].clone();
    assert_eq!(chat.url, ai_cloud::AI_CHAT_V2_URL);
    // ⚠ 对话那一枪用的是**完整 auth**（旧 TS 就是这么分的，别统一）
    assert!(chat
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("Authorization") && v == "Basic eA=="));
    // 文件名 = taskId + 1
    assert!(
        chat.body_str().unwrap_or_default().contains("124.jpeg"),
        "{}",
        chat.body_str().unwrap_or_default()
    );
}

/// 识图失败 ⇒ **不发对话那一枪**（省一发写请求）。
#[tokio::test]
async fn ai_camera_stops_when_recognize_fails() {
    let (ctx, http) = ctx_with(
        vec![Reply::ok(r#"{"success":false,"message":"no"}"#)],
        Some("1234567890123456789"),
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::AiCamera, 585, &mut owned).await;

    assert!(!r.acted);
    assert_eq!(http.count(), 1, "识图没过就不该发对话：{:?}", http.urls());
    assert!(r.notes.iter().any(|n| n.contains("no")), "{:?}", r.notes);
}

/// 账号没记 `userDomainId` **且**邮箱侧也派生不出来 ⇒ **一枪都不打 AI 那两个口**。
///
/// ⚠ 2026-10-07 起多了一条兜底：先走 `disk:queryUserToken` 派生（PC 端登录的账号
/// 没有 `user_domain_id` 那一格，真机实测「移动」就是这种 ⇒ 没有兜底它永远做不成 ✗）。
/// 所以这一条钉的不是"零请求"，而是**"没拿到 userId 就不发识图那一枪"** ✓。
#[tokio::test]
async fn ai_camera_needs_user_domain_id() {
    // 邮箱那条链也走不通（`{}` 解不出 SSO token）
    let (ctx, http) = ctx_with(vec![Reply::ok("{}")], None);
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::AiCamera, 585, &mut owned).await;

    assert!(!r.acted);
    assert!(
        r.notes.iter().any(|n| n.contains("缺少用户信息")),
        "{:?}",
        r.notes
    );
    assert_eq!(
        http.hits("ai.yun.139.com"),
        0,
        "没拿到 userId 就不该打 AI 那两个口：{:?}",
        http.urls()
    );
}

/// `account.user_domain_id` 缺格时走**邮箱侧派生**（`disk:queryUserToken` → `userDomainId`）。
///
/// 这是让 PC 端登录的账号也能做 585 的那条兜底（真机：`移动` 账号 ✓）。
#[tokio::test]
async fn ai_camera_derives_user_domain_id_from_mail() {
    let (ctx, http) = ctx_with(
        vec![
            // 邮箱登录两枪
            Reply::ok(r#"{"success":true,"code":"0000","data":{"token":"SSO"}}"#),
            Reply::ok(r#"{"var":{"sid":"S1","rmkey":"R1"}}"#),
            // queryUserToken
            Reply::ok(
                r#"{"code":"S_OK","var":{"data":{"data":{"account":"ENC","token":"TOK","userDomainId":"1122334455667788990"}}}}"#,
            ),
            // aiRecognize
            Reply::ok(r#"{"success":true,"data":{"fileId":"F1","taskId":123}}"#),
            // chat
            Reply::ok(r#"data: {"success":true,"code":"0000"}"#),
        ],
        None,
    );
    let mut state = RunnerState::new();
    let mut owned = Vec::new();
    let r = run_runner(&ctx, &mut state, AppRunner::AiCamera, 585, &mut owned).await;

    assert!(r.acted, "{:?}", r.notes);
    let rec = http
        .requests()
        .into_iter()
        .find(|q| q.url.contains("aiRecognize"))
        .expect("有识图那一枪");
    assert!(
        rec.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-yun-uni") && v == "1122334455667788990"),
        "派生出来的 userDomainId 要进 x-yun-uni"
    );
}
