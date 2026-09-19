//! 会话与初始化：网络路径上的"你是谁"。
//!
//! # 两条规则
//!
//! 1. **没初始化就只暴露初始化**：库里一个用户都没有时，除健康检查与 `/api/setup`
//!    外一律 `503 setup_required`。否则一个刚部署、还没建管理员的实例，
//!    会把"添加账号""跑任务"这类接口暴露给任何一个扫到端口的人。
//! 2. **其余接口一律要会话**：解析出 [`AuthUser`] 才继续，解析不出来 401。
//!    端点里再取 `Scope::User(&user.id)` —— 数据隔离本身在存储层那一份过滤表达式里，
//!    这一层只负责"把请求映射成某个人"。
//!
//! # 为什么 `AuthUser` 由中间件**放进 extensions**
//!
//! 处理器再解析一次就是第二次查库（每个请求两次会话查询），而且会出现
//! "中间件判过了、处理器忘了判"的空窗。放进来之后处理器只能从
//! extensions 取 —— 拿不到就编不过。
//!
//! # cookie 的三个属性都是必要的
//!
//! - `HttpOnly`：JS 读不到 token（XSS 拿不走会话）；
//! - `SameSite=Lax`：跨站 POST 不带 cookie（挡掉最常见的 CSRF 形态），
//!   同时保留"从外部链接点进来仍是已登录"的正常体验；
//! - `Path=/`：整站共用。
//!
//! `Secure` 由 [`ServerConfig::secure_cookie`] 决定，默认**关** —— 本地是
//! `http://127.0.0.1`，开了它浏览器根本不会回传，表现为"登录成功但一直是未登录"。

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use mcloud_app::session;
use mcloud_app::user::init_first_admin;
use mcloud_core::{Clock, Error as CoreError, Session, User, UserStore};
use mcloud_io::{SqliteStore, SystemRandom};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiJson};

/// 会话 cookie 名。
const COOKIE_NAME: &str = "mcloud_session";

/// 服务端配置。
#[derive(Debug, Clone, Copy, Default)]
pub struct ServerConfig {
    /// 给 cookie 打 `Secure`（HTTPS 部署时开）。
    ///
    /// 默认关，理由见模块文档：本地 http 下打开它等于"登录永远不生效"。
    pub secure_cookie: bool,
}

/// 路由状态（可 `Clone`：axum 每个请求都会拿一份）。
#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    store: SqliteStore,
    clock: Arc<dyn Clock + Send + Sync>,
    config: ServerConfig,
}

/// 已认证的请求身份。
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
}

impl AppState {
    pub fn new(
        store: SqliteStore,
        clock: Arc<dyn Clock + Send + Sync>,
        config: ServerConfig,
    ) -> Self {
        Self(Arc::new(Inner {
            store,
            clock,
            config,
        }))
    }

    /// 组装层与处理器都从这里取存储。
    pub fn store(&self) -> &SqliteStore {
        &self.0.store
    }

    pub fn config(&self) -> ServerConfig {
        self.0.config
    }

    /// 当前时间（注入的时钟）—— 会话到期判定必须能被测试拨动。
    pub fn now(&self) -> DateTime<Utc> {
        self.0.clock.now()
    }

    /// 解析请求里的会话。中间件与处理器共用这一处。
    async fn current(&self, headers: &HeaderMap) -> Result<Option<AuthUser>, CoreError> {
        let Some(token) = cookie_token(headers) else {
            return Ok(None);
        };
        let Some((user, session)) =
            session::current_by_token(&self.0.store, &token, self.now()).await?
        else {
            return Ok(None);
        };
        Ok(Some(AuthUser { user, session }))
    }
}

/// 组装路由。
///
/// 静态前端产物**不在这里**：那是二进制启动时才知道的目录（[`crate::serve`]），
/// 路由本身不该依赖文件系统。
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/setup", get(setup_status).post(setup_create))
        .route(
            "/api/session",
            // 登录（POST）必须免登录，否则是个死结 —— 见 `is_public`
            post(login).get(current_session).delete(logout),
        )
        // 兜底：`/api` 下不存在的路径必须**明确报 404**，不能落到前端 index.html ——
        // 那会让"路径写错"表现成"返回了一段 HTML"，前端解析失败且毫无线索。
        // 静态段优先于通配，所以它不会抢走上面几条。
        .route("/api/{*rest}", axum::routing::any(api_not_found))
        // 加入顺序有讲究：后加入的是**外层**，所以 `require_setup` 先跑
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), require_setup))
        .with_state(state)
}

/// 免登录的端点。
///
/// 用**白名单**而不是黑名单：以后新加一条路由时，默认是"需要登录"。
/// 反过来的话，漏登记一条就等于把那个接口公开了。
fn is_public(method: &Method, path: &str) -> bool {
    if matches!(path, "/api/health" | "/api/setup") {
        return true;
    }
    method == Method::POST && path == "/api/session"
}

/// 未初始化时只放行健康检查与初始化接口。
fn is_setup_allowed(path: &str) -> bool {
    matches!(path, "/api/health" | "/api/setup")
}

async fn require_setup(State(state): State<AppState>, request: Request, next: Next) -> Response {
    // 查库失败时不给结论：让后面的层去报错，别把"读不到用户数"当成"已初始化"
    if let Ok(users) = state.store().count_users().await
        && users == 0
        && !is_setup_allowed(request.uri().path())
    {
        return ApiError::SetupRequired.into_response();
    }
    next.run(request).await
}

async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    if is_public(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    match state.current(request.headers()).await {
        Ok(Some(auth)) => {
            request.extensions_mut().insert(auth);
            next.run(request).await
        }
        Ok(None) => ApiError::Unauthorized.into_response(),
        Err(err) => ApiError::from(err).into_response(),
    }
}

// ────────────────────────────── 处理器 ──────────────────────────────

#[derive(Deserialize)]
struct Credentials {
    name: String,
    password: String,
}

/// `GET /api/health`
async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "name": mcloud_core::PRODUCT_NAME }))
}

/// `/api` 下的未知路径：用**同一个错误形状**报 404。
async fn api_not_found(request: Request) -> ApiError {
    ApiError::Domain(CoreError::NotFound(format!(
        "没有这个接口: {}",
        request.uri().path()
    )))
}

/// `GET /api/setup` —— 前端据此决定显示初始化页还是登录页。
///
/// 顺带把口令长度下限**发给前端**：那是 `mcloud_core::MIN_PASSWORD_LEN` 定的策略，
/// 前端硬编一份必然与它漂移，而漂移的表现是"前端说可以、后端拒绝"。
async fn setup_status(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let users = state.store().count_users().await?;
    Ok(Json(json!({
        "needsSetup": users == 0,
        "minPasswordLen": mcloud_core::MIN_PASSWORD_LEN,
    })))
}

/// `POST /api/setup` —— 建首个管理员。
///
/// 走的是 CLI `mcloud user init` **同一个用例**（方案 §3.7 的"避免两套 bootstrap"）：
/// 同一张表、同一套校验（用户名规则、口令长度、argon2id 参数）、
/// 同一个"无主数据认领"。这里只多了一件事：顺手把会话发出去，
/// 免得用户刚设完口令又被要求输一遍。
async fn setup_create(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<Credentials>,
) -> Result<Response, ApiError> {
    let mut rng = SystemRandom;
    let now = state.now();
    let outcome =
        init_first_admin(state.store(), &body.name, &body.password, &mut rng, now).await?;
    let (user, token, session) =
        session::sign_in(state.store(), &body.name, &body.password, &mut rng, now).await?;

    Ok(with_session_cookie(
        &state,
        &token,
        &session,
        StatusCode::CREATED,
        Json(json!({
            "user": user_payload(&user),
            "claimed": {
                "accounts": outcome.claimed.accounts,
                "schedules": outcome.claimed.schedules,
                "pipelines": outcome.claimed.pipelines,
            },
        })),
    ))
}

/// `POST /api/session` —— 登录。
async fn login(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<Credentials>,
) -> Result<Response, ApiError> {
    let mut rng = SystemRandom;
    let now = state.now();
    let (user, token, session) =
        session::sign_in(state.store(), &body.name, &body.password, &mut rng, now).await?;

    Ok(with_session_cookie(
        &state,
        &token,
        &session,
        StatusCode::OK,
        Json(json!({ "user": user_payload(&user) })),
    ))
}

/// `GET /api/session` —— 当前身份。未登录时由中间件直接 401。
async fn current_session(Extension(auth): Extension<AuthUser>) -> Json<Value> {
    Json(json!({
        "user": user_payload(&auth.user),
        "expiresAt": auth.session.expires_at.to_rfc3339(),
    }))
}

/// `DELETE /api/session` —— 登出。
///
/// 按**会话行的哈希**删，不用把 cookie 里的明文再取一次：
/// 中间件已经把会话解析出来了，撤销就是删掉那一行 —— **立刻生效**。
async fn logout(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> Result<Response, ApiError> {
    state
        .store()
        .delete_session(&auth.session.token_hash)
        .await?;

    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Ok(value) = HeaderValue::from_str(&clear_cookie_header(state.config().secure_cookie)) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    Ok(response)
}

// ────────────────────────────── 形状与 cookie ──────────────────────────────

/// 用户的**对外形状**。
///
/// 手写而不是给 `User` 加 `Serialize`：`User` 里有 `password_hash`，
/// 一旦它有 derive，某天一个 `Json(user)` 就会把口令哈希发到浏览器。
/// 这里只列出允许出网的字段。
fn user_payload(user: &User) -> Value {
    json!({
        "id": user.id.as_str(),
        "name": user.name,
        "role": user.role.as_str(),
    })
}

/// 从 `Cookie` 头里取会话 token。
fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        // 先剥名字再要求紧接 `=`：否则 `mcloud_session_x=...` 会被误认成它
        if let Some(value) = part
            .strip_prefix(COOKIE_NAME)
            .and_then(|rest| rest.strip_prefix('='))
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }
    None
}

fn cookie_header(token: &str, max_age_secs: i64, secure: bool) -> String {
    let mut value = format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        max_age_secs.max(0)
    );
    if secure {
        value.push_str("; Secure");
    }
    value
}

fn clear_cookie_header(secure: bool) -> String {
    cookie_header("", 0, secure)
}

/// 把会话 cookie 写进响应。
fn with_session_cookie(
    state: &AppState,
    token: &str,
    session: &Session,
    status: StatusCode,
    body: impl IntoResponse,
) -> Response {
    let max_age = (session.expires_at - state.now()).num_seconds();
    let mut response = body.into_response();
    if let Ok(value) =
        HeaderValue::from_str(&cookie_header(token, max_age, state.config().secure_cookie))
    {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    //! 只留**纯单元**的断言。
    //!
    //! 路由级的行为（状态码、cookie、门禁）在 `tests/api_contract.rs` 里 ——
    //! 那边只走公开接口，能证明"这层 API 从外面能用"，而这里的单元测试
    //! 能看见私有函数，反而证不了那件事。

    use super::*;
    use mcloud_core::{Clock, FixedClock, Role};

    fn some_user() -> User {
        User::new(
            "alice",
            "alice",
            "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA",
            Role::User,
            FixedClock::parse("2026-09-18T00:00:00Z").now(),
        )
    }

    #[test]
    fn cookie_parsing_does_not_confuse_a_prefixed_name() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=1; mcloud_session_extra=nope; mcloud_session=abc; z=2"),
        );
        assert_eq!(cookie_token(&headers).as_deref(), Some("abc"));

        headers.insert(header::COOKIE, HeaderValue::from_static("mcloud_session="));
        assert_eq!(cookie_token(&headers), None, "空值不算带了 cookie");

        headers.insert(header::COOKIE, HeaderValue::from_static("nothing=here"));
        assert_eq!(cookie_token(&headers), None);

        let empty = HeaderMap::new();
        assert_eq!(cookie_token(&empty), None, "没有 Cookie 头也不该慌");
    }

    /// 对外只发允许出网的字段。
    ///
    /// 这条盯的是"某天给 `User` 加了 `Serialize` 就直接 `Json(user)`"这个具体错法：
    /// `User` 里有 `password_hash`。
    #[test]
    fn user_payload_lists_only_allowed_fields() {
        let payload = user_payload(&some_user());
        let object = payload.as_object().unwrap();
        assert_eq!(object.len(), 3, "对外只有 id/name/role: {object:?}");
        assert!(object.contains_key("id") && object.contains_key("name"));
        assert_eq!(object["role"], "user");
        assert!(
            !payload.to_string().contains("argon2"),
            "口令哈希绝不能出现在对外形状里"
        );
    }

    #[test]
    fn errors_map_to_stable_status_codes() {
        // 走 `into_response()` 而不是内部映射函数：验的是**对外那一层**的行为
        let cases = [
            (ApiError::Unauthorized, StatusCode::UNAUTHORIZED),
            (ApiError::SetupRequired, StatusCode::SERVICE_UNAVAILABLE),
            (ApiError::BadBody("x".into()), StatusCode::BAD_REQUEST),
            (
                ApiError::Domain(CoreError::Auth("x".into())),
                StatusCode::UNAUTHORIZED,
            ),
            (
                ApiError::Domain(CoreError::NotFound("x".into())),
                StatusCode::NOT_FOUND,
            ),
            (
                ApiError::Domain(CoreError::RateLimited("x".into())),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                ApiError::Domain(CoreError::SoldOut("x".into())),
                StatusCode::CONFLICT,
            ),
            (
                ApiError::Domain(CoreError::Timeout("x".into())),
                StatusCode::BAD_GATEWAY,
            ),
            (
                ApiError::Domain(CoreError::Internal("x".into())),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];
        for (err, expected) in cases {
            let response = err.into_response();
            assert_eq!(response.status(), expected);
        }
    }

    /// 5xx 不回显原文：那些文案常常带着路径、SQL 片段或上游返回体。
    #[tokio::test]
    async fn internal_errors_do_not_leak_details() {
        let response = ApiError::Domain(CoreError::Internal(
            "SELECT * FROM accounts 失败: no such column".into(),
        ))
        .into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "internal");
        let message = body["message"].as_str().unwrap();
        assert!(
            !message.contains("SELECT"),
            "内部细节泄漏到响应里: {message}"
        );
        assert!(!message.contains("no such column"), "{message}");
        // 给的是类别文案（`ErrorKind::public_message`），不是原文
        assert_eq!(
            message,
            CoreError::Internal(String::new()).kind().public_message()
        );
    }
}
