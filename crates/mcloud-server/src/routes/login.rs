//! 账号登录（扫码 / 短信 / 账密）的 HTTP 接口。
//!
//! # 与 `/api/session` 的区别
//!
//! | 端点 | 登的是谁 | 凭据落在哪 |
//! | --- | --- | --- |
//! | `/api/session` | **本实例的管理员**（WebUI 登录闸） | `users.json` / `sessions.json` |
//! | `/api/login/*`（本文件） | **云盘账号**（要跑任务的那些） | `accounts.json` |
//!
//! 两者混起来会很难查：管理员的会话过期与应用户的凭据过期看起来都是"401"。
//!
//! # 为什么扫码要有"会话"这个概念
//!
//! 扫码是**异步**的：前端拿到二维码后要反复问"扫了没"。所以服务端必须
//! 记住"这次扫码会话"（`sID` + 平台 + 过期时间）。这里用一张内存表存它 ——
//! 刻意**不落盘**：登录会话的生命周期只有两分钟，进程重启后重出一张码
//! 比"从磁盘恢复一个已经过期的会话"更符合直觉。
//!
//! # 平台参数是必填的
//!
//! 请求体里 `platform` **不给默认值**（`windows` / `macos`）。默认值会让
//! 「WebUI 以为在出 mac 的码、实际发的是 win 的请求」这类错误静默通过，
//! 而下游的凭据落盘也跟着错。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Extension, State};
use axum::Json;
use mcloud_core::{PcPlatform, UserId};
use mcloud_tasks::login::{QrLogin, QrProgress, SendSmsOutcome};
use serde::{Deserialize, Serialize};

use crate::auth_gate::CurrentUser;
use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 一次扫码会话在服务端的内存形态。
struct QrSession {
    qr: QrLogin,
    /// 轮询用的 `x-yun-tid`：一次会话内保持一致，更像真实客户端。
    tid: String,
    /// **发起这次扫码的 WebUI 用户**（2026-10-06 安全审计 `SEC-06`）。
    ///
    /// # 为什么必须记它
    ///
    /// 不记的话，`session_id` 就是唯一凭据 —— 而它是**出码方自己**拿到的一串
    /// 随机值，从没打算当密码用（会出现在日志、URL、前端状态里）✗。
    /// 谁拿到它，谁就能对**别人的**扫码会话做两件事：
    ///
    /// 1. `poll` 推进它 —— 于是"扫码成功后把账号挂到**轮询者**名下"这个动作
    ///    可以被第三方完成（QR 码是攻击者出的、受害者扫的，账号落进攻击者手里）；
    /// 2. `DELETE` 取消它（干扰）。
    ///
    /// 记上 owner 之后，这两条都要求"你得出示同一个登录会话" ✓。
    owner: UserId,
}

/// 扫码会话表。
///
/// 用 `Mutex<HashMap>` 而不是更细的锁：这张表的操作只有"插入 / 取出 /
/// 推进一次"，且每次都极短（没有 `.await` 持锁）。用更复杂的结构
/// 只会让"锁有没有跨 await"这件事更难看出来。
#[derive(Default)]
pub struct LoginSessions {
    qr: Mutex<HashMap<String, QrSession>>,
}

impl LoginSessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入前**顺手扫掉已过期的**（2026-10-04 复审 `sv-7`）。
    ///
    /// 这张表原先**只进不出**：出一张码就插一条，只有"轮询到终态"或"显式 DELETE"
    /// 才摘得掉 ⇒ 用户点开登录框看一眼就走、页面刷新、扫码超时后前端不再轮询
    ///（前端只在终态 `close`）都会留下**常驻**条目，长跑实例里单调增长 ——
    /// 这是"跑久了内存慢慢涨"最容易复现的一条 ✗。
    ///
    /// ⚠ 判据用 `QrLogin` 自己带着的 `started_at_ms + ttl_seconds`（**上游给的**有效期），
    /// 不另记一个 `inserted_at` —— 两套时间早晚会漂 ✓。
    fn insert(&self, id: String, session: QrSession, now_ms: i64) {
        let mut qr = self.qr.lock().unwrap();
        qr.retain(|_, s| !s.qr.is_expired(now_ms));
        qr.insert(id, session);
    }

    /// 推进一次。**锁只在这段同步代码里持有**，网络请求在外面。
    ///
    /// 连同 `owner` 一起取出来：调用方必须能核对"这是不是**你**发起的那次"（SEC-06）。
    /// 不在表内比对，是因为"取出 → 发网络请求 → 放回"之间要**松手**，
    /// 在松手期间持锁会把整张表卡在一次上游请求上 ✗。
    fn take(&self, id: &str) -> Option<(QrLogin, String, UserId)> {
        self.qr
            .lock()
            .unwrap()
            .remove(id)
            .map(|s| (s.qr, s.tid, s.owner))
    }

    fn put_back(&self, id: String, qr: QrLogin, tid: String, owner: UserId, now_ms: i64) {
        self.insert(id, QrSession { qr, tid, owner }, now_ms);
    }

    /// 只删**自己的**那条（SEC-06）：`true` = 真的删掉了。
    ///
    /// ⚠ 判据与 `poll` 用同一个（`owner`）—— 两处判据不一样的话，
    /// 就会出现"能轮询不能取消"（或反过来）这种自相矛盾的口子 ✗。
    fn remove_for(&self, id: &str, owner: &UserId) -> bool {
        let mut qr = self.qr.lock().unwrap();
        match qr.get(id) {
            Some(s) if &s.owner == owner => {
                qr.remove(id);
                true
            }
            _ => false,
        }
    }
}

// ── 出码 ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StartQrRequest {
    /// `windows` | `macos`。**必填**（见模块文档）。
    ///
    /// 加了 `default` 是为了让"缺字段"走我们自己那条 400 —— 否则 axum 的
    /// `Json` 提取器会先回 422，而它的报错文本是 Rust 字段名（`missing field
    /// `platform``），对调用方没有指导意义。
    #[serde(default)]
    pub platform: String,
}

#[derive(Serialize)]
pub struct StartQrResponse {
    /// 会话 id（前端轮询时带回来）。
    pub session_id: String,
    /// 二维码载荷（原始链接）。
    ///
    /// 仍然给出：它是**排障与自渲染**的入口（用户可能想在自己手机上打开、
    /// 或贴进别的工具里）。
    pub payload: String,
    /// 二维码图片（SVG 的 data URL），前端直接塞进 `<img src=...>`。
    ///
    /// # 为什么由服务端渲染
    ///
    /// 前端项目里**没有二维码库**，而在浏览器里现写一个编码器等于把
    /// "码对不对"交给前端。服务端已经有编码器（CLI 出码用的是同一份矩阵），
    /// 两边共用一份的好处是：画错的话**两边一起错**、也就一起被发现，
    /// 而不是"命令行能扫、网页不能"这种最难查的差异。
    ///
    /// 用 data URL 而不是让前端 `innerHTML` 塞 SVG 串：前者是**数据**，
    /// 浏览器按图片处理；后者是往 DOM 注入标记，多开一条本不需要的路。
    pub image_svg: String,
    /// 平台回显（让用户确认自己选的是哪个）。
    pub platform: String,
    /// 平台的人话名字。
    pub platform_label: String,
    /// 轮询间隔（毫秒）。
    pub poll_interval_ms: u64,
    /// 有效期（秒）。
    pub ttl_seconds: i64,
    /// **本次扫码会话自己**那枚设备号（排障用）。
    ///
    /// 每张码现生成一枚，与机器级登记值（`GET /api/login/device` 读到的那个）
    /// 是两个值；换后者**不影响**这一枚，也不作废任何扫码会话。
    pub device_id: String,
}

/// `POST /api/login/qr` —— 出码（**不发任何上游请求**）。
///
/// ⚠ 要 `Extension<CurrentUser>` 不是为了"挡人"（闸已经挡了），而是要把
/// **发起者记进会话**（安全审计 `SEC-06`）—— 见 [`QrSession::owner`]。
pub async fn start_qr(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<StartQrRequest>,
) -> ApiResult<StartQrResponse> {
    let platform = parse_platform(&req.platform)?;
    let qr = state
        .handle
        .login
        .start_qr(platform)
        .await
        .map_err(ApiError::from)?;

    let session_id = qr.session_id().to_string();
    let payload = qr.qr_payload();
    // 渲染失败不该让整个出码失败：前端还有 `payload` 可以自己想办法。
    let image_svg = crate::qr::Matrix::new(&payload)
        .map(|m| m.to_svg_data_url())
        .unwrap_or_default();
    let device_id = qr.client().device_id().to_string();
    let tid = state.handle.login.new_tid();

    // ⚠ 顺手扫一遍过期的（复审 sv-7）：这张表原先只进不出。
    // ⚠ `owner` 一起落进去（审计 SEC-06）：之后 poll / cancel 都要核对它。
    state.login_sessions.put_back(
        session_id.clone(),
        qr,
        tid,
        cur.0,
        state.handle.clock.now_ms(),
    );

    Ok(Json(StartQrResponse {
        session_id,
        payload,
        image_svg,
        platform: platform.as_str().to_string(),
        platform_label: mcloud_app::platform_label(platform).to_string(),
        poll_interval_ms: mcloud_tasks::login::QR_POLL_INTERVAL_MS,
        ttl_seconds: mcloud_tasks::login::QR_TTL_SECONDS,
        device_id,
    }))
}

// ── 轮询 ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PollQrRequest {
    pub session_id: String,
}

#[derive(Serialize)]
pub struct PollQrResponse {
    /// `not_scanned` | `scanned` | `logged_in` | `failed`。
    pub phase: String,
    /// 是否该停止轮询。
    pub terminal: bool,
    /// 失败原因的人话说明（`phase == "failed"` 时才有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 限流时建议的退避毫秒数（其他情况为 `None`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// 登录成功后的**掩码**账号名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// 登录成功后落盘的平台（前端据此提示"已按 macOS 保存"）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// `POST /api/login/qr/poll` —— 问一次"扫了没"。
///
/// # 为什么是 POST 而不是 GET
///
/// 它会**改变服务端状态**（推进会话、成功后落库），也可能触发上游请求。
/// GET 在语义上必须可重复且无副作用，而"轮询一次"不满足。
pub async fn poll_qr(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<PollQrRequest>,
) -> ApiResult<PollQrResponse> {
    // 取出会话（不持锁发请求）—— 锁跨 await 是这类表最容易犯的错。
    let Some((mut qr, tid, owner)) = state.login_sessions.take(&req.session_id) else {
        // 会话不存在：可能是过期被清了，也可能是前端拿错了 id。
        // 404 让前端能明确区分"再出一张码"与"网络问题重试"。
        return Err(session_not_found(&req.session_id));
    };

    // ⚠⚠ 必须是**发起这次扫码的那个人**（安全审计 `SEC-06`）。
    //
    // 少了这一条，`session_id` 就是一张"谁拿到谁能用"的通行证：第三方可以
    // 推进别人的会话，把"扫码成功后落进谁名下"这件事从受害者手里夺走
    //（QR 码是攻击者出的、受害者扫的 ⇒ 账号落进攻击者名下）✗。
    //
    // 放回去再回 404（而不是直接丢掉）：否则"猜一个 id"就成了一次
    // **对别人扫码会话的拒绝服务** —— 真正的主人再轮询就只剩 404 了 ✗。
    if owner != cur.0 {
        state.login_sessions.put_back(
            req.session_id.clone(),
            qr,
            tid,
            owner,
            state.handle.clock.now_ms(),
        );
        // 与"不存在"同一个响应：不告诉猜 id 的人"这个 id 是有的，只是不是你的" ✓
        return Err(session_not_found(&req.session_id));
    }

    let progress = state.handle.login.poll_qr(&mut qr, &tid).await;

    match progress {
        Ok(QrProgress::LoggedIn { user_info }) => {
            // 成功即落库；会话到此结束，不再放回表里。
            let account = state
                .handle
                .login
                .finish_qr(&qr, &user_info)
                .await
                .map_err(ApiError::from)?;
            let account = claim_account(&state, &cur.0, account).await?;
            Ok(Json(PollQrResponse {
                phase: "logged_in".into(),
                terminal: true,
                reason: None,
                retry_after_ms: None,
                account: Some(account.id.as_str().to_string()),
                platform: account.pc_platform.map(|p| p.as_str().to_string()),
            }))
        }
        Ok(QrProgress::Failed { reason }) => {
            let terminal = QrProgress::Failed {
                reason: reason.clone(),
            }
            .is_terminal();
            // 限流不是终态：会话要放回去，前端退避后接着问。
            if !terminal {
                state.login_sessions.put_back(
                    req.session_id,
                    qr,
                    tid,
                    owner,
                    state.handle.clock.now_ms(),
                );
            }
            Ok(Json(PollQrResponse {
                phase: "failed".into(),
                terminal,
                reason: Some(describe_failure(&reason)),
                retry_after_ms: (!terminal).then_some(5_000),
                account: None,
                platform: None,
            }))
        }
        Ok(QrProgress::NotScanned) | Ok(QrProgress::Scanned) => {
            let phase = match progress {
                Ok(QrProgress::Scanned) => "scanned",
                _ => "not_scanned",
            };
            state.login_sessions.put_back(
                req.session_id,
                qr,
                tid,
                owner,
                state.handle.clock.now_ms(),
            );
            Ok(Json(PollQrResponse {
                phase: phase.into(),
                terminal: false,
                reason: None,
                retry_after_ms: None,
                account: None,
                platform: None,
            }))
        }
        Err(e) => {
            // 网络抖动不该让用户重出一张码：会话放回去，前端可以再问。
            state.login_sessions.put_back(
                req.session_id,
                qr,
                tid,
                owner,
                state.handle.clock.now_ms(),
            );
            Err(ApiError(e))
        }
    }
}

/// "扫码会话不存在"的 404 —— `poll` 与 `cancel` 共用一份措辞，
/// 免得两条路给出的说法不一样、被人拿来区分"id 存在但不是我"与"id 不存在"。
fn session_not_found(id: &str) -> ApiError {
    ApiError(mcloud_core::Error::NotFound(format!(
        "扫码会话不存在或已过期：{id}"
    )))
}

/// `DELETE /api/login/qr/{session_id}` —— 主动放弃一次扫码。
///
/// ⚠ 只能取消**自己发起**的那次（安全审计 `SEC-06`）：判据与 `poll` 完全一致。
/// 不是自己的（或本来就不存在）⇒ 同样回 404，且**不动**那条会话。
pub async fn cancel_qr(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> ApiResult<serde_json::Value> {
    if !state.login_sessions.remove_for(&session_id, &cur.0) {
        return Err(session_not_found(&session_id));
    }
    Ok(Json(serde_json::json!({ "cancelled": true })))
}

// ── 短信 ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SmsStartRequest {
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub platform: String,
    /// 滑块答案（`start` 第一次返回 `need_slider` 时，把 `slider.answer` 原样回填）。
    ///
    /// ⚠ **必须有这一格，否则发码那一枪永远过不去**：上游在需要滑块时**不发**短信，
    /// 而"带答案重发"这件事在此之前只在**提交**那一枪上（那时验证码还没发出来，
    /// 是鸡生蛋）⇒ 前端拿到题面后没有任何一枪可以把它推进到 `sent`。
    ///
    /// 这个能力任务层早就有（`SmsSession::send_code_with_slider`），缺的只是这个入口。
    #[serde(default)]
    pub slider_answer: Option<String>,
}

#[derive(Serialize)]
pub struct SmsStartResponse {
    /// `sent` | `need_slider` | `slider_exhausted`。
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remain_get_times: Option<u32>,
    /// 需要滑块时把图与答案一起给前端。
    ///
    /// ⚠ 答案**直接给**是刻意的：上游把答案加密在 `puzzleLeft` 里
    /// （固定密钥，见 `SliderPuzzle::answer`），根本没有"识别图像"这一步。
    /// 让前端自己做图像识别纯属自找麻烦。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slider: Option<SliderPayload>,
}

#[derive(Serialize)]
pub struct SliderPayload {
    pub background: String,
    pub puzzle: String,
    pub width: Option<i64>,
    /// 已解出的答案（前端可直接用它重发验证码）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// `POST /api/login/sms/start` —— 发验证码。
///
/// ⚠ **限速**（安全审计 `SEC-08`）：这条路是 `/api/login/*` 里唯一会对**外部**
/// 产生副作用的一枪 —— 每调一次就真发一条短信（可能被运营商计费、也可能触发
/// 上游风控）。已登录用户不受登录闸限速，所以必须单独设闸 ✓。
pub async fn start_sms(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<SmsStartRequest>,
) -> ApiResult<SmsStartResponse> {
    // ⚠ 先过闸再干活：放后面就等于"短信已经发出去了才说你太快了" ✗
    if let Some(err) = super::auth::sensitive_rate_limit(&state, &cur.0) {
        return Err(err);
    }
    let platform = parse_platform(&req.platform)?;
    if req.phone.trim().is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "手机号不能为空".into(),
        )));
    }

    let sms = state
        .handle
        .login
        .start_sms(platform, req.phone.trim())
        .await
        .map_err(ApiError::from)?;
    let tid = state.handle.login.new_tid();
    // ⚠ 带了答案就走「带滑块发码」那一枪，而不是再发一次裸的：
    // 裸的那一枪在需要滑块时**上游根本不发短信**，再发一次只会拿回同一道题
    // （表现是"点自动完成没反应"，短信永远发不出来）。
    let outcome = match req.slider_answer.as_deref().filter(|a| !a.is_empty()) {
        Some(answer) => {
            sms.send_code_with_slider(state.handle.login.http(), &tid, answer)
                .await
        }
        None => sms.send_code(state.handle.login.http(), &tid).await,
    }
    .map_err(ApiError::from)?;

    match outcome {
        SendSmsOutcome::Sent { remain_get_times } => Ok(Json(SmsStartResponse {
            phase: "sent".into(),
            remain_get_times,
            slider: None,
        })),
        SendSmsOutcome::NeedSlider { .. } | SendSmsOutcome::SliderExhausted { .. } => {
            let puzzle = sms
                .slider_puzzle(state.handle.login.http(), &tid)
                .await
                .map_err(ApiError::from)?;
            let exhausted = matches!(outcome, SendSmsOutcome::SliderExhausted { .. });
            Ok(Json(SmsStartResponse {
                phase: if exhausted {
                    "slider_exhausted".into()
                } else {
                    "need_slider".into()
                },
                remain_get_times: None,
                slider: Some(SliderPayload {
                    background: puzzle.background_b64.clone(),
                    puzzle: puzzle.puzzle_b64.clone(),
                    width: puzzle.pic_width,
                    answer: puzzle.answer().ok(),
                }),
            }))
        }
        other => Err(ApiError(mcloud_core::Error::Upstream {
            code: String::new(),
            message: format!("发送验证码失败：{other:?}"),
        })),
    }
}

#[derive(Deserialize)]
pub struct SmsLoginRequest {
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub code: String,
    /// 需要滑块时把它带上（`start` 若返回 `need_slider`，前端应先带答案再发一次）。
    #[serde(default)]
    pub slider_answer: Option<String>,
}

/// `POST /api/login/sms` —— 用验证码完成登录。
///
/// # 为什么把"带滑块重发"合进这一个端点
///
/// 不带滑块时它只做最后一步；带了 `slider_answer` 就先重发一次验证码。
/// 分成两个端点会让前端必须自己记住"刚才那次发码是不是失败了"，
/// 而这个状态服务端本来就有。
pub async fn login_sms(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<SmsLoginRequest>,
) -> ApiResult<PollQrResponse> {
    let platform = parse_platform(&req.platform)?;
    let phone = req.phone.trim();
    if phone.is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "手机号不能为空".into(),
        )));
    }
    if req.code.trim().is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "验证码不能为空".into(),
        )));
    }

    let sms = state
        .handle
        .login
        .start_sms(platform, phone)
        .await
        .map_err(ApiError::from)?;
    let tid = state.handle.login.new_tid();
    let http = state.handle.login.http();

    // 需要滑块时先带答案重发一次
    if let Some(answer) = req.slider_answer.as_deref().filter(|a| !a.is_empty()) {
        match sms.send_code_with_slider(http, &tid, answer).await {
            Ok(SendSmsOutcome::Sent { .. }) => {}
            Ok(other) => {
                return Err(ApiError(mcloud_core::Error::Upstream {
                    code: String::new(),
                    message: format!("带滑块发码失败：{other:?}"),
                }))
            }
            Err(e) => return Err(ApiError(e)),
        }
    }

    let user_info = sms
        .login(
            http,
            &tid,
            req.code.trim(),
            &mcloud_tasks::production_decryptor(),
        )
        .await
        .map_err(ApiError::from)?;
    let account = state
        .handle
        .login
        .finish_sms(&sms, &user_info)
        .await
        .map_err(ApiError::from)?;
    let account = claim_account(&state, &cur.0, account).await?;

    Ok(Json(PollQrResponse {
        phase: "logged_in".into(),
        terminal: true,
        reason: None,
        retry_after_ms: None,
        account: Some(account.id.as_str().to_string()),
        platform: account.pc_platform.map(|p| p.as_str().to_string()),
    }))
}

// ── App 端（手机客户端）短信登录 ────────────────────────────────────────

/// `POST /api/login/app/sms/start` —— App 端发验证码。
///
/// # 与 PC 版 `/api/login/sms/start` 的区别
///
/// 请求体里**没有 `platform`**：App 端只有一种形态，而 PC 端那个参数
/// 存在的意义正是"区分 windows / macos"。给 App 也塞一个 platform 参数，
/// 只会让人以为它能变。
#[derive(Deserialize)]
pub struct AppSmsStartRequest {
    #[serde(default)]
    pub phone: String,
    /// 滑块答案 —— 与 PC 那支同一个理由：上游需要滑块时**不发**短信，
    /// 只有带上答案的那一次才会真的发出去。
    #[serde(default)]
    pub slider_answer: Option<String>,
}

/// `POST /api/login/app/sms/start` 的响应。
///
/// 与 PC 版**刻意不共用**：PC 那支的 `need_slider` 之后是"去取图、本地识别"，
/// App 这支之后是同一件事，但含义上属于 App 链路 —— 共用一个响应类型会让
/// 前端以为两端可以互换。
#[derive(Serialize)]
pub struct AppSmsStartResponse {
    /// `sent` / `need_captcha` / `slider_exhausted`。
    pub phase: String,
    /// 今日还可发送的次数（仅 `sent` 时可能有）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remain_get_times: Option<u32>,
    /// 上游给的验证码类型。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captcha_type: Option<i64>,
    /// 已解出的滑块答案（前端可直接用它重发，**不需要用户去拖**）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slider_answer: Option<String>,
    /// 滑块背景图（base64）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slider_background: Option<String>,
    /// 滑块拼图块（base64）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slider_puzzle: Option<String>,
    /// 滑块题面宽度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slider_width: Option<i64>,
}

/// `POST /api/login/app/sms/start` —— App 端发码；需要滑块时顺手把题取回来。
///
/// ⚠ 与 PC 那条**共用一个闸**（`sensitive_limiter`，键 = 用户 id）：两条路都是
/// "真发一条短信"，各设一个闸等于把配额翻倍 ✓（安全审计 `SEC-08`）。
pub async fn app_start_sms(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<AppSmsStartRequest>,
) -> ApiResult<AppSmsStartResponse> {
    if let Some(err) = super::auth::sensitive_rate_limit(&state, &cur.0) {
        return Err(err);
    }
    let phone = req.phone.trim();
    if phone.is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "手机号不能为空".into(),
        )));
    }

    let sms = state
        .handle
        .login
        .start_app_sms(phone)
        .await
        .map_err(ApiError::from)?;
    let now = state.handle.login.now_micros();
    // ⚠ 带了答案就走「带滑块发码」那一枪（同 PC 那支的理由）。
    let outcome = match req.slider_answer.as_deref().filter(|a| !a.is_empty()) {
        Some(answer) => {
            sms.send_code_with_slider(state.handle.login.http(), now, answer)
                .await?
        }
        None => sms.send_code(state.handle.login.http(), now).await?,
    };

    match outcome {
        mcloud_app::AppSendSmsOutcome::Sent { remain_get_times } => Ok(Json(AppSmsStartResponse {
            phase: "sent".into(),
            remain_get_times,
            captcha_type: None,
            slider_answer: None,
            slider_background: None,
            slider_puzzle: None,
            slider_width: None,
        })),
        // 需要滑块：**顺手把题取回来并解出答案**。
        //
        // 取图接口与 PC 端是同一个第一方接口（`/user/verfycode`），
        // 而 `puzzleLeft` 里带着动态 IV，所以本地就能算出坐标，
        // 不需要用户去拖、也不需要图像识别。
        mcloud_app::AppSendSmsOutcome::NeedCaptcha { captcha_type }
        | mcloud_app::AppSendSmsOutcome::SliderExhausted { captcha_type } => {
            let exhausted = matches!(
                outcome,
                mcloud_app::AppSendSmsOutcome::SliderExhausted { .. }
            );
            // 挑战作废（200059553）时**不重试旧答案**，只把状态如实报给前端。
            let puzzle = sms.slider_puzzle(state.handle.login.http(), now).await.ok();
            let (answer, background, pz, width) = match &puzzle {
                Some(p) => (
                    p.answer().ok(),
                    Some(p.background_b64.clone()),
                    Some(p.puzzle_b64.clone()),
                    p.pic_width,
                ),
                None => (None, None, None, None),
            };
            Ok(Json(AppSmsStartResponse {
                phase: if exhausted {
                    "slider_exhausted".into()
                } else {
                    "need_captcha".into()
                },
                remain_get_times: None,
                captcha_type,
                slider_answer: answer,
                slider_background: background,
                slider_puzzle: pz,
                slider_width: width,
            }))
        }
        other => Err(ApiError(mcloud_core::Error::Upstream {
            code: String::new(),
            message: format!("App 端发送验证码失败：{other:?}"),
        })),
    }
}

/// `POST /api/login/app/sms` 的请求体。
#[derive(Deserialize)]
pub struct AppSmsLoginRequest {
    #[serde(default)]
    pub phone: String,
    #[serde(default)]
    pub code: String,
    /// 需要滑块时带上答案（`start` 返回的 `slider_answer` 直接回填即可）。
    #[serde(default)]
    pub slider_answer: Option<String>,
}

/// `POST /api/login/app/sms` —— 用验证码完成 App 端登录。
///
/// 带 `slider_answer` 时先重发一次验证码（与 PC 版同样的理由：
/// 分成两个端点会让前端必须自己记住"刚才那次发码是不是失败了"，
/// 而这个状态服务端本来就有）。
pub async fn app_login_sms(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<AppSmsLoginRequest>,
) -> ApiResult<PollQrResponse> {
    let phone = req.phone.trim();
    if phone.is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "手机号不能为空".into(),
        )));
    }
    if req.code.trim().is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "验证码不能为空".into(),
        )));
    }

    let sms = state
        .handle
        .login
        .start_app_sms(phone)
        .await
        .map_err(ApiError::from)?;
    let now = state.handle.login.now_micros();
    let http = state.handle.login.http();

    if let Some(answer) = req.slider_answer.as_deref().filter(|a| !a.is_empty()) {
        match sms.send_code_with_slider(http, now, answer).await? {
            mcloud_app::AppSendSmsOutcome::Sent { .. } => {}
            other => {
                return Err(ApiError(mcloud_core::Error::Upstream {
                    code: String::new(),
                    message: format!("带滑块发码失败：{other:?}"),
                }))
            }
        }
    }

    let user_info = sms.login(http, now, req.code.trim()).await?;
    let account = state
        .handle
        .login
        .finish_app_sms(&sms, &user_info)
        .await
        .map_err(ApiError::from)?;
    let account = claim_account(&state, &cur.0, account).await?;

    Ok(Json(PollQrResponse {
        phase: "logged_in".into(),
        terminal: true,
        reason: None,
        retry_after_ms: None,
        account: Some(account.id.as_str().to_string()),
        // App 账号没有 PC 平台 —— 前端据此显示"手机端"而不是某个 PC 平台。
        platform: None,
    }))
}

// ── 账密 ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PasswordLoginRequest {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub platform: String,
}

/// `POST /api/login/password` —— 账密登录。
///
/// # 口令不进日志
///
/// 这个 handler **不记录** `req.password`，错误信息里也不带它。
/// 请求体由 axum 的 `Json` 提取器消费，不经我们自己的日志。
/// （`tracing` 层的 `TraceLayer` 只记方法与路径，不记 body —— 见 `lib.rs`。）
pub async fn login_password(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(req): Json<PasswordLoginRequest>,
) -> ApiResult<PollQrResponse> {
    let platform = parse_platform(&req.platform)?;
    if req.username.trim().is_empty() || req.password.is_empty() {
        return Err(ApiError(mcloud_core::Error::Config(
            "用户名与口令都不能为空".into(),
        )));
    }

    let tid = state.handle.login.new_tid();
    let account = state
        .handle
        .login
        .login_with_password(platform, req.username.trim(), &req.password, &tid)
        .await
        .map_err(ApiError::from)?;
    let account = claim_account(&state, &cur.0, account).await?;

    Ok(Json(PollQrResponse {
        phase: "logged_in".into(),
        terminal: true,
        reason: None,
        retry_after_ms: None,
        account: Some(account.id.as_str().to_string()),
        platform: account.pc_platform.map(|p| p.as_str().to_string()),
    }))
}

// ── 设备指纹 ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DeviceResponse {
    pub device_id: String,
}

/// `GET /api/login/device` —— 查看本机 PC 设备指纹（机器级登记值，排障用）。
pub async fn get_device(State(state): State<ServerState>) -> ApiResult<DeviceResponse> {
    let id = state
        .handle
        .login
        .pc_device_id()
        .await
        .map_err(ApiError::from)?;
    Ok(Json(DeviceResponse { device_id: id }))
}

/// `POST /api/login/device/rotate` —— 换掉机器级登记值。
///
/// ⚠ 它**不会**作废任何扫码会话、也不影响任何账号的登录：出码用的是每张码
/// 现生成的那一枚，四条登录路径按账号取设备。这里换的只是随后
/// `GET /api/login/device` 读到的值。
pub async fn rotate_device(State(state): State<ServerState>) -> ApiResult<DeviceResponse> {
    let id = state
        .handle
        .login
        .rotate_pc_device_id()
        .await
        .map_err(ApiError::from)?;
    Ok(Json(DeviceResponse { device_id: id }))
}

// ── 辅助 ────────────────────────────────────────────────────────────────

/// 把刚登录建出来的云盘账号归到**当前系统用户**名下。
///
/// # 为什么在这里补写
///
/// 账号是 `mcloud_app::LoginService::finish_*` 落库的，而那一层不知道
/// 「当前 WebUI 会话是谁」（认证在 server 层）。不补的话，多用户实例里
/// 新登录的账号就是「无主数据」—— 谁都看不到（失败关闭）。
///
/// # ⚠⚠ 2026-10-06 安全审计 `SEC-05`：这里**不能**盲目覆盖
///
/// 旧实现是"`account.owner = Some(当前用户)` 然后整行 save"。两个问题：
///
/// 1. **整行覆盖**：`account` 是 `finish_*` 刚**新建**出来的对象（昵称 / 每账号
///    设置 / 停用状态都是默认值）⇒ 把别人那一行的这些字段一起抹平 ✗
///    （现在 `persist_login` 已经改成保留式合并，但这一层不该**依赖**那个前提 ——
///    它要是退回覆盖式，这里会安静地把别人的配置删掉）；
/// 2. **抢归属**：这一行要是已经属于**别人**，覆盖 owner 就是一次归属劫持 ✗。
///    能走到这里说明"这个云盘账号的登录被完成了"，而登录可以由第三方促成
///    （`SEC-06` 的 QR 场景、或知道手机号+验证码的人）⇒ 归属检查必须在这里兜底 ✓。
///
/// 现在的做法：**以库里那一行为准**（`persist_login` 已经做过保留式合并），
/// 只改 `owner`；归属冲突 ⇒ 404（不泄露"这个账号存在"）。
async fn claim_account(
    state: &ServerState,
    owner: &mcloud_core::UserId,
    account: mcloud_core::Account,
) -> Result<mcloud_core::Account, ApiError> {
    // ⚠ 用**不带归属过滤**的 `account()`：`account_for` 对"别人的账号"回 None，
    // 那正是这里要区分出来的情形 —— 用它就永远看不出冲突 ✗
    let mut stored = state
        .handle
        .store
        .account(&account.id)
        .await?
        .unwrap_or(account);

    if let Some(prev) = stored.owner.as_ref() {
        if prev != owner {
            return Err(ApiError(mcloud_core::Error::NotFound(format!(
                "账号不存在：{}",
                stored.id.as_str()
            ))));
        }
    }

    stored.owner = Some(owner.clone());
    state.handle.store.save_account(&stored).await?;
    Ok(stored)
}

fn parse_platform(text: &str) -> Result<PcPlatform, ApiError> {
    mcloud_app::parse_platform(text).map_err(ApiError::from)
}

fn describe_failure(reason: &mcloud_tasks::login::QrFailure) -> String {
    use mcloud_tasks::login::QrFailure as F;
    match reason {
        F::TtlElapsed { ttl_seconds } => format!("二维码已超时（{ttl_seconds} 秒），请重新出码"),
        F::ExpiredUpstream => "上游说二维码已失效，请重新出码".into(),
        F::SessionMismatch => "二维码标识不匹配（本程序的状态错误）".into(),
        F::TooFrequent => "扫码过于频繁，稍后会自动重试".into(),
        F::Rejected { code, message } => match code {
            Some(c) => format!("上游拒绝（{c}）：{message}"),
            None => format!("上游拒绝：{message}"),
        },
    }
}

/// 让 `ServerState` 拿到会话表（字段加在 `state.rs` 里）。
pub type Sessions = Arc<LoginSessions>;

#[cfg(test)]
mod tests;
