//! 账户安全：改口令、密码策略、会话时长。
//!
//! # 为什么是单独一块
//!
//! 这三样是**同一件事的三个面**：谁能进这个 WebUI、门槛多高、进来之后待多久。
//! 与 `accounts.rs`（云盘账号）无关 —— 那边管"任务用谁的凭据"，这边管"你凭什么叫得动它"。
//!
//! # 改口令沿用登录那条口径（用户口径 2026-10-01：前端 hash 一次、后端再 hash 一次）
//!
//! 请求里带的**通常已经是摘要**（前端做的）；是明文（缓存里的旧 bundle、
//! 或 LAN 的 http 下 `crypto.subtle` 不可用）也照收，由
//! `to_digest_credential` 补上那一次。**当前口令**那条与 `login` 用同一套档位判断。
//!
//! # 改完为什么要把会话全踢掉、又马上发一张新的
//!
//! 改口令最常见的动机就是"它可能泄露了" —— 那时最该失效的正是别人手里的 cookie，
//! 所以 `remove_user_sessions` 一刀切。但**操作者自己不能被踢出去**：
//! 那样界面只会弹回登录页，用户看到的是"改失败了"。所以紧接着给当前这个浏览器
//! 发一张新令牌，并把踢掉的条数回给前端（"已让 N 处登录失效"）。

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde::{Deserialize, Serialize};

use mcloud_core::{PasswordHash, User};

use crate::auth_gate::{require_admin, CurrentUser};
use crate::error::{ApiError, ApiResult};
use crate::routes::auth::{login_scheme, make_session, session_expires_at, set_cookie_response};
use crate::ServerState;

/// 当前策略 + 口令存储档位。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityView {
    /// 口令长度下限（可改）。
    ///
    /// ⚠ 只对**明文**生效：前端摘要恒 64 位，量不出人到底输了几位。
    /// 所以 WebUI 会拿这个值在前端先卡一道，服务端那道留给 curl 这类直接调用。
    pub min_password_len: usize,
    /// 新建会话的有效期（小时）。改它只影响**之后**新建的会话。
    pub session_ttl_hours: u32,
    /// 库里口令的档位：`sha256` = 前端摘要再 argon2；`plain` = 老档（登录时自动升级）。
    pub password_scheme: &'static str,
}

/// 改策略。**两个字段都要给**（PUT = 整体替换），少给会 422 ——
/// 这样"以为改了、其实没改"不可能发生：前端总是把当前值一起发回来。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityPatch {
    pub min_password_len: usize,
    pub session_ttl_hours: u32,
}

async fn view(state: &ServerState) -> Result<SecurityView, ApiError> {
    let s = state.handle.store.settings().await?;
    Ok(SecurityView {
        min_password_len: s.min_password_len,
        session_ttl_hours: s.session_ttl_hours,
        password_scheme: login_scheme(state).await?,
    })
}

pub async fn get_security(State(state): State<ServerState>) -> ApiResult<SecurityView> {
    Ok(Json(view(&state).await?))
}

/// `PUT /api/admin/security` —— **只有管理员**（安全审计 SEC-07）。
///
/// 这里改的是**实例级**策略（口令长度下限、会话 TTL），影响所有人；
/// 而 `GET` 保持开放：那些值本来就在未登录可读的 `GET /api/setup` 里
/// （前端要靠它渲染"至少 N 位"的提示）⇒ 藏它没有意义 ✗。
pub async fn put_security(
    State(state): State<ServerState>,
    Extension(cur): Extension<CurrentUser>,
    Json(body): Json<SecurityPatch>,
) -> ApiResult<SecurityView> {
    require_admin(&state, &cur).await?;
    // ⚠ **读-改-写**（2026-10-09 起 ✓）：`AppSettings` 是一份整体，
    // 容量那三格（历史条数 / 单次日志行数 / 日志总量 ✓）也在里面 ——
    // 直接拿请求体构造一个新的会把它们重置成默认值 ✗，而那种 bug 只有
    // "先改容量、再改策略"时才现形，极难查 ✓。所以这里只改**自己这两格** ✓
    // （`put_storage` 那侧同一条口径 ✓）。
    let mut next = state.handle.store.settings().await?;
    next.min_password_len = body.min_password_len;
    next.session_ttl_hours = body.session_ttl_hours;
    next.validate()
        .map_err(|e| ApiError(mcloud_core::Error::Config(e)))?;
    state.handle.store.save_settings(&next).await?;
    tracing::info!(
        min_password_len = next.min_password_len,
        session_ttl_hours = next.session_ttl_hours,
        "安全策略已更新（只影响之后新建的会话 / 之后设的口令）"
    );
    Ok(Json(view(&state).await?))
}

/// 改口令。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChange {
    /// 当前口令，**必填**。
    ///
    /// 挡的是"别人趁你没锁屏、坐下就改掉口令"。会话被盗时它拦不住（能走到这个接口的
    /// 会话本来就能删账号），所以严格说是"防手滑"；但用户口径明确要它
    /// （2026-10-01："你口令更改怎么取消了旧口令"）—— 一次 argon2 换这份确定性，值。
    pub current_password: String,
    pub new_password: String,
}

/// 改口令的结果：新会话（顺手换 cookie）+ 被踢掉的会话数。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChanged {
    pub user_id: String,
    pub name: String,
    pub kind: &'static str,
    /// 这次改口令顺带结束了多少个**旧**会话（不含马上发的这张新令牌）。
    pub sessions_ended: usize,
}

pub async fn change_password(
    State(state): State<ServerState>,
    Extension(CurrentUser(user_id)): Extension<CurrentUser>,
    Json(body): Json<PasswordChange>,
) -> Result<Response, ApiError> {
    // ⚠ 先过闸（安全审计 SEC-08）：下面那次 `verify_password` 就是一次 argon2
    // （19 MiB × 2 轮），而这个路由**不受登录闸限速**（那时已经登录了）⇒
    // 不设闸的话，一个已登录用户就能用"反复改口令"把自己的机器打满 ✗。
    if let Some(err) = crate::routes::auth::sensitive_rate_limit(&state, &user_id) {
        return Err(err);
    }
    let store = Arc::clone(&state.handle.store);
    let user = store
        .user(&user_id)
        .await?
        .ok_or_else(|| ApiError(mcloud_core::Error::Auth("会话对应的用户已不存在".into())))?;

    // ── 当前口令：与 login 同一套档位判断 ────────────────────────────────
    let stored = user.password_hash.as_str();
    let legacy = !mcloud_crypto::is_digest_scheme(stored);
    let current_digest = mcloud_crypto::to_digest_credential(&body.current_password);
    let candidate = if legacy {
        body.current_password.as_str()
    } else {
        current_digest.as_str()
    };
    mcloud_crypto::verify_password(candidate, mcloud_crypto::strip_digest_scheme(stored))
        .map_err(|_| ApiError(mcloud_core::Error::Auth("当前口令不正确".into())))?;

    // ── 新口令 ───────────────────────────────────────────────────────────
    let settings = store.settings().await?;
    if !mcloud_crypto::is_client_digest(&body.new_password)
        && body.new_password.chars().count() < settings.min_password_len
    {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "新口令太短：至少 {} 个字符",
            settings.min_password_len
        ))));
    }
    let new_digest = mcloud_crypto::to_digest_credential(&body.new_password);
    if new_digest == current_digest {
        // 挡在落库前：否则会白踢一堆会话，而口令其实没变
        return Err(ApiError(mcloud_core::Error::Config(
            "新口令与当前口令相同".into(),
        )));
    }

    let hash = mcloud_crypto::hash_password(&new_digest)
        .map_err(|e| ApiError(mcloud_core::Error::Config(format!("口令哈希失败：{e}"))))?;
    let updated = User {
        password_hash: PasswordHash(mcloud_crypto::mark_digest_scheme(&hash)),
        ..user.clone()
    };
    store.save_user(&updated).await?;

    // 旧会话一刀切（口令泄露时该失效的就是它们），再给当前浏览器发一张新的
    let sessions_ended = store.remove_user_sessions(&user.id).await?;
    let now = state.handle.clock.now_ms();
    let (token, session) = make_session(&user, now, settings.session_ttl_ms());
    store.save_session(&session).await?;

    tracing::info!(user = %user.name, sessions_ended, "管理员口令已更换，旧会话已失效");
    Ok(set_cookie_response(
        StatusCode::OK,
        &token,
        session_expires_at(session.expires_at_ms, now),
        state.cookie_secure,
        Json(PasswordChanged {
            user_id: user.id.to_string(),
            name: user.name,
            kind: "web",
            sessions_ended,
        }),
    ))
}
