//! WebUI 登录闸：首次初始化、登录、登出、当前会话。
//!
//! # 这是**另一层**认证，别与账号登录混
//!
//! | | 谁的凭证 | 存哪 | 谁用 |
//! |---|---|---|---|
//! | 系统用户（本模块） | 你设的用户名/口令 | `users.json` | 浏览器 |
//! | 云盘账号（`/api/accounts`） | 移动云盘的 auth | `accounts.json` | 任务运行时 |
//!
//! 本模块管"谁能进这个 WebUI"，不管"能操作哪个云盘账号"。
//!
//! # 三态门
//!
//! - **未初始化**（0 个用户）⇒ 除 `/api/setup` 与 `/api/health` 外**一律 503**
//! - **已初始化、未登录** ⇒ 除 `/api/session` 外一律 401
//! - **已登录** ⇒ 放行
//!
//! ⚠ 未初始化时返回 503 而不是 401，是因为这两件事的**处置不同**：
//! 401 让前端弹登录框（而这时根本没有账号可登），503 让前端跳到初始化向导。
//! 混成一个码，用户会看到"登录失败"而不是"还没建管理员"。

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use mcloud_core::{validate_username, PasswordHash, Session, SessionKind, User, UserId};
// 令牌原语来自 crypto 而不是 core：core 只放**类型与 trait**，
// 密码学原语归 crypto。这条边界让 core 不必依赖任何密码学实现。
use mcloud_crypto::{hash_session_token, new_session_token};

use crate::error::{ApiError, ApiResult};
use crate::ServerState;

/// 会话 cookie 名。
const COOKIE: &str = "mcloud_session";

// ── 请求/响应类型 ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    /// 是否已经建过管理员。
    pub initialized: bool,
    /// 口令长度下限。**必须发给前端** —— 前端硬编一份必然与它漂移，
    /// 而漂移的表现是"前端说可以、后端拒绝"。
    pub min_password_len: usize,
    /// 登录时前端该发什么凭据：`"sha256"` = 发 `SHA-256(明文)` 的 hex；
    /// `"plain"` = 发**明文**（库里还是老档，服务端验过之后会当场升级）。
    ///
    /// 让前端**先问再发**，而不是"发错了再重试一次" —— 后者要多一条错误码、
    /// 一次额外的往返，而两条路的差异只是"这个实例的库是什么时候建的"。
    pub password_scheme: &'static str,
}

/// 凭据档位：库里存的是 `argon2(SHA-256(明文))`。
const SCHEME_DIGEST: &str = "sha256";

/// 凭据档位：库里存的是 `argon2(明文)`（2026-10-01 之前的实例）。
const SCHEME_PLAIN: &str = "plain";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub user_id: String,
    pub name: String,
    pub kind: &'static str,
}

// ── 初始化 ──────────────────────────────────────────────────────────────

/// 这个实例是否已经初始化过。
///
/// 判据是"有 0 个用户"而不是"有个 initialized 标志位" ——
/// 标志位会与实际状态漂移（标志位 true 但用户表空了），而用户表是**事实**。
pub async fn is_initialized(state: &ServerState) -> Result<bool, ApiError> {
    let n = state.handle.store.user_count().await?;
    Ok(n > 0)
}

/// 这个实例的**登录凭据档位**（见 [`SetupStatus::password_scheme`]）。
///
/// ⚠ 只要**有一个**用户还是老档就报 `plain`：前端一次只能发一种凭据，
/// 而"存在老记录"意味着只有明文那条路能同时喂饱两边。单个管理员是本项目的主线，
/// 多管理员只是理论情形 —— 这时候宁可让新记录也再走一次明文（登录成功后会各自升级）。
pub(crate) async fn login_scheme(state: &ServerState) -> Result<&'static str, ApiError> {
    let users = state.handle.store.users().await?;
    Ok(
        if users
            .iter()
            .any(|u| !mcloud_crypto::is_digest_scheme(u.password_hash.as_str()))
        {
            SCHEME_PLAIN
        } else {
            SCHEME_DIGEST
        },
    )
}

pub async fn setup_status(State(state): State<ServerState>) -> ApiResult<SetupStatus> {
    Ok(Json(SetupStatus {
        initialized: is_initialized(&state).await?,
        // 下限是**可改的设置**（默认 8 = `MIN_PASSWORD_LEN`）
        min_password_len: state.handle.store.settings().await?.min_password_len,
        password_scheme: login_scheme(&state).await?,
    }))
}

/// 首次建管理员。
///
/// ⚠ **已经初始化过就拒绝**（409）。不这样的话，任何人都能在你的实例上
/// 再建一个管理员账号 —— 那等于没有登录闸。
///
/// # 口令怎么落库（用户口径 2026-10-01：**前端 hash 一次、后端再 hash 一次**）
///
/// 新建的管理员**一律落新档**：`argon2(SHA-256(明文))`，存储串带 `sha256$` 前缀。
/// 前端没做那一次（缓存里的旧 bundle 还在发明文）时，服务端**现场补上**
/// —— 兼容口子开在这里，而不是"拒绝明文"，否则老 bundle 的用户一点登录就 400。
pub async fn setup_create(
    State(state): State<ServerState>,
    Json(body): Json<Credentials>,
) -> Result<Response, ApiError> {
    if is_initialized(&state).await? {
        return Err(ApiError(mcloud_core::Error::Config(
            "已经初始化过了，不能重复创建管理员".into(),
        )));
    }

    validate_username(&body.username).map_err(mcloud_core::Error::Config)?;

    // ⚠ 长度下限只在**明文**这条路上判得出来：摘要恒 64 位，拿它比长度没有意义。
    // 所以"收到明文"时在这里卡一道，报错文案与从前一致（前端已经拿 minPasswordLen 卡过一次，
    // 这是服务端那道，缺了它 curl 直接 POST 就能设一个 1 位的口令）。
    let settings = state.handle.store.settings().await?;
    if !mcloud_crypto::is_client_digest(&body.password)
        && body.password.chars().count() < settings.min_password_len
    {
        return Err(ApiError(mcloud_core::Error::Config(format!(
            "口令太短：至少 {} 个字符（下限 {} 位）",
            settings.min_password_len, settings.min_password_len
        ))));
    }

    let digest = mcloud_crypto::to_digest_credential(&body.password);
    let hash = mcloud_crypto::hash_password(&digest).map_err(|e| {
        mcloud_core::Error::Config(format!("{e}（下限 {} 位）", settings.min_password_len))
    })?;

    let now = state.handle.clock.now_ms();
    let user = User {
        id: UserId::new(uuid::Uuid::new_v4().to_string()),
        name: body.username.clone(),
        password_hash: PasswordHash(mcloud_crypto::mark_digest_scheme(&hash)),
        created_at_ms: now,
    };
    state.handle.store.save_user(&user).await?;

    // 建完管理员**顺手登录** —— 让用户接着填一遍刚设的口令是多余的
    let (token, session) = make_session(&user, now, settings.session_ttl_ms());
    state.handle.store.save_session(&session).await?;

    tracing::info!(user = %user.name, "已创建管理员");
    Ok(set_cookie_response(
        StatusCode::CREATED,
        &token,
        session_expires_at(session.expires_at_ms, now),
        Json(SessionView {
            user_id: user.id.to_string(),
            name: user.name,
            kind: "web",
        }),
    ))
}

// ── 登录 / 登出 ─────────────────────────────────────────────────────────

pub async fn login(
    State(state): State<ServerState>,
    Json(body): Json<Credentials>,
) -> Result<Response, ApiError> {
    // 未初始化时明确告知，而不是"用户名或口令错误" ——
    // 后者会让人反复试一个根本不存在的账号
    if !is_initialized(&state).await? {
        return Err(ApiError(mcloud_core::Error::Config(
            "实例尚未初始化，请先创建管理员".into(),
        )));
    }

    let user = state
        .handle
        .store
        .user_by_name(&body.username)
        .await?
        // ⚠ 用户不存在与口令错**回同一句话**：区分开来就是一个
        // "这个用户名存不存在"的探测器。
        .ok_or_else(|| ApiError(mcloud_core::Error::Auth("用户名或口令错误".into())))?;

    // ── 口令校验：两种档位，按**库里那条记录**决定怎么验 ──────────────────
    //
    // 老档（2026-10-01 之前建的实例）：库里是对**明文**做的 argon2 ⇒ 只能用明文验。
    // 新档：库里是对**前端摘要**做的 argon2 ⇒ 请求里是摘要就直接用；
    //       是明文（浏览器缓存里的旧 bundle）就现场补一次摘要。
    // 两条兼容口子都指向同一个目的：**任何一侧还没更新，都不许把人锁在门外**。
    let stored = user.password_hash.as_str();
    let legacy = !mcloud_crypto::is_digest_scheme(stored);
    let digest = mcloud_crypto::to_digest_credential(&body.password);
    let candidate = if legacy {
        body.password.as_str()
    } else {
        digest.as_str()
    };

    mcloud_crypto::verify_password(candidate, mcloud_crypto::strip_digest_scheme(stored))
        .map_err(|_| ApiError(mcloud_core::Error::Auth("用户名或口令错误".into())))?;

    // 老档验过 ⇒ **当场升级**。不升级的话这条记录永远只能靠明文登录，
    // "明文不出浏览器"就永远只是新装实例的事。
    // ⚠ 升级用的一定是**摘要**（`digest`），不是刚验过的明文 —— 后者等于白做。
    if legacy {
        let hash = mcloud_crypto::hash_password(&digest)
            .map_err(|e| mcloud_core::Error::Config(format!("口令升级失败：{e}")))?;
        let upgraded = User {
            password_hash: PasswordHash(mcloud_crypto::mark_digest_scheme(&hash)),
            ..user.clone()
        };
        state.handle.store.save_user(&upgraded).await?;
        tracing::info!(user = %upgraded.name, "管理员口令已升级为「前端摘要」档");
    }

    let now = state.handle.clock.now_ms();
    let ttl_ms = state.handle.store.settings().await?.session_ttl_ms();
    let (token, session) = make_session(&user, now, ttl_ms);
    state.handle.store.save_session(&session).await?;

    tracing::info!(user = %user.name, "登录成功");
    Ok(set_cookie_response(
        StatusCode::OK,
        &token,
        session_expires_at(session.expires_at_ms, now),
        Json(SessionView {
            user_id: user.id.to_string(),
            name: user.name,
            kind: "web",
        }),
    ))
}

/// 登出。
///
/// 返回类型是 `Result<StatusCode, ApiError>` 而**不是** `ApiResult<StatusCode>`：
/// 后者展开成 `Result<Json<StatusCode>, _>`，会把 `204` 包成 `Json(204)` ——
/// 于是响应变成 `200` 加一个 `204` 的 JSON 体。
pub async fn logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    if let Some(token) = cookie_token(&headers) {
        state
            .handle
            .store
            .remove_session(&hash_session_token(&token))
            .await?;
    }
    // 即使没有 cookie 也返回 200：登出是幂等的，
    // "本来就没登录"不该报错
    Ok(StatusCode::NO_CONTENT)
}

/// 当前会话。未登录时 401（由中间件拦，这里只处理已登录的情况）。
pub async fn current_session(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> ApiResult<SessionView> {
    let session = require_session(&state, &headers).await?;
    let user = state
        .handle
        .store
        .user(&session.user_id)
        .await?
        .ok_or_else(|| ApiError(mcloud_core::Error::Auth("会话对应的用户已不存在".into())))?;

    Ok(Json(SessionView {
        user_id: user.id.to_string(),
        name: user.name,
        kind: match session.kind {
            SessionKind::Web => "web",
            SessionKind::Cli => "cli",
        },
    }))
}

// ── 会话读写 ────────────────────────────────────────────────────────────

/// 建会话。
///
/// `ttl_ms` 由调用方从 `AppSettings` 取（默认值 = 原来的 `WEB_SESSION_TTL_MS`，
/// 见 `mcloud-core::settings`）—— **改设置只影响之后新建的会话**，
/// 已经发出去的 cookie 不会被动缩短有效期。
pub(crate) fn make_session(user: &User, now_ms: i64, ttl_ms: i64) -> (String, Session) {
    let token = new_session_token();
    let session = Session {
        token_hash: hash_session_token(&token),
        user_id: user.id.clone(),
        kind: SessionKind::Web,
        created_at_ms: now_ms,
        expires_at_ms: now_ms + ttl_ms,
    };
    // 令牌原文只在这一次响应里出现（写进 cookie），之后库里只有哈希
    (token, session)
}

pub(crate) fn session_expires_at(expires_at_ms: i64, now_ms: i64) -> i64 {
    // cookie 的 Max-Age 用秒；会话表用毫秒
    ((expires_at_ms - now_ms) / 1000).max(0)
}

/// 从 `Cookie` 头里取我们的令牌。
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&format!("{COOKIE}=")) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 取当前有效会话；没有/过期/用户已删都返回 401。
pub async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Session, ApiError> {
    let token =
        cookie_token(headers).ok_or_else(|| ApiError(mcloud_core::Error::Auth("未登录".into())))?;
    let hash = hash_session_token(&token);

    let session = state
        .handle
        .store
        .session(&hash)
        .await?
        .ok_or_else(|| ApiError(mcloud_core::Error::Auth("会话已失效".into())))?;

    let now = state.handle.clock.now_ms();
    if !session.is_valid_at(now) {
        // 顺手清掉：过期会话留着既占地方，又会在下次被误读
        state.handle.store.remove_session(&hash).await?;
        return Err(ApiError(mcloud_core::Error::Auth("会话已过期".into())));
    }

    Ok(session)
}

/// 组装带 `Set-Cookie` 的响应。
///
/// cookie 属性逐条都有理由：
///
/// - `HttpOnly` —— JS 读不到，XSS 偷不走令牌
/// - `SameSite=Strict` —— 跨站请求不带它，挡住 CSRF
/// - `Path=/` —— 全站可用
/// - `Max-Age` —— 与库里的到期时间**一致**；只靠 cookie 过期的话，
///   客户端时间被改就绕过了
///
/// **不加 `Secure`**：本工具默认跑在 `http://127.0.0.1`，加了 `Secure`
/// 浏览器会直接不存这个 cookie，表现是"登录成功但立刻又未登录"。
/// 部署在 HTTPS 后面时应当由反代加。
pub(crate) fn set_cookie_response<T: Serialize>(
    status: StatusCode,
    token: &str,
    max_age_secs: i64,
    body: Json<T>,
) -> Response {
    let cookie =
        format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}");
    (status, [(header::SET_COOKIE, cookie)], body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    // 这两个只在测试里当"默认值"用：生产路径改从 `AppSettings` 读（默认值正是它们）。
    use axum::http::HeaderMap;
    use mcloud_core::WEB_SESSION_TTL_MS;

    fn headers_with_cookie(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, v.parse().unwrap());
        h
    }

    #[test]
    fn reads_our_cookie_from_a_crowded_header() {
        let h = headers_with_cookie("other=1; mcloud_session=abc123; another=2");
        assert_eq!(cookie_token(&h).as_deref(), Some("abc123"));
    }

    #[test]
    fn ignores_foreign_cookies() {
        let h = headers_with_cookie("other=1; another=2");
        assert_eq!(cookie_token(&h), None);
    }

    #[test]
    fn empty_cookie_value_is_treated_as_absent() {
        let h = headers_with_cookie("mcloud_session=");
        assert_eq!(cookie_token(&h), None, "空值等于没有，不该去查库");
    }

    #[test]
    fn missing_cookie_header_is_none() {
        assert_eq!(cookie_token(&HeaderMap::new()), None);
    }

    /// cookie 里存的是**原文**，库里存的是哈希 —— 两者不能混。
    #[test]
    fn session_creation_stores_only_the_hash() {
        let user = User {
            id: UserId::new("u1"),
            name: "admin".into(),
            password_hash: PasswordHash("$argon2id$...".into()),
            created_at_ms: 0,
        };
        let (token, session) = make_session(&user, 1_000, WEB_SESSION_TTL_MS);
        assert_ne!(session.token_hash, token, "库里不能是原文");
        assert_eq!(
            session.token_hash,
            hash_session_token(&token),
            "存的必须是这个令牌的哈希"
        );
        assert_eq!(session.user_id, user.id);
        assert_eq!(session.kind, SessionKind::Web);
        assert_eq!(session.expires_at_ms, 1_000 + WEB_SESSION_TTL_MS);
    }

    /// `Max-Age` 是**秒**，会话表是**毫秒** —— 换算错就是"cookie 活得久"
    /// 或"立刻过期"。
    #[test]
    fn max_age_converts_ms_to_secs() {
        let now = 1_000_000;
        assert_eq!(session_expires_at(now + 60_000, now), 60);
        assert_eq!(session_expires_at(now, now), 0, "不能是负数");
        assert_eq!(
            session_expires_at(now + WEB_SESSION_TTL_MS, now),
            WEB_SESSION_TTL_MS / 1000
        );
    }
}
