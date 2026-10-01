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
use mcloud_core::PcPlatform;
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

    fn insert(&self, id: String, session: QrSession) {
        self.qr.lock().unwrap().insert(id, session);
    }

    /// 推进一次。**锁只在这段同步代码里持有**，网络请求在外面。
    fn take(&self, id: &str) -> Option<(QrLogin, String)> {
        self.qr.lock().unwrap().remove(id).map(|s| (s.qr, s.tid))
    }

    fn put_back(&self, id: String, qr: QrLogin, tid: String) {
        self.insert(id, QrSession { qr, tid });
    }

    fn remove(&self, id: &str) {
        self.qr.lock().unwrap().remove(id);
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
pub async fn start_qr(
    State(state): State<ServerState>,
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

    state.login_sessions.put_back(session_id.clone(), qr, tid);

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
    let Some((mut qr, tid)) = state.login_sessions.take(&req.session_id) else {
        // 会话不存在：可能是过期被清了，也可能是前端拿错了 id。
        // 404 让前端能明确区分"再出一张码"与"网络问题重试"。
        return Err(ApiError(mcloud_core::Error::NotFound(format!(
            "扫码会话不存在或已过期：{}",
            req.session_id
        ))));
    };

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
                state.login_sessions.put_back(req.session_id, qr, tid);
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
            state.login_sessions.put_back(req.session_id, qr, tid);
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
            state.login_sessions.put_back(req.session_id, qr, tid);
            Err(ApiError(e))
        }
    }
}

/// `DELETE /api/login/qr/{session_id}` —— 主动放弃一次扫码。
pub async fn cancel_qr(
    State(state): State<ServerState>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> ApiResult<serde_json::Value> {
    state.login_sessions.remove(&session_id);
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
pub async fn start_sms(
    State(state): State<ServerState>,
    Json(req): Json<SmsStartRequest>,
) -> ApiResult<SmsStartResponse> {
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
pub async fn app_start_sms(
    State(state): State<ServerState>,
    Json(req): Json<AppSmsStartRequest>,
) -> ApiResult<AppSmsStartResponse> {
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
/// 补写是**覆盖式**的：同一条账号再 save 一次只是把 owner 钉成当前用户。
async fn claim_account(
    state: &ServerState,
    owner: &mcloud_core::UserId,
    mut account: mcloud_core::Account,
) -> Result<mcloud_core::Account, ApiError> {
    account.owner = Some(owner.clone());
    state.handle.store.save_account(&account).await?;
    Ok(account)
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
