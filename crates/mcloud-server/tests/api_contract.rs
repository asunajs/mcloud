//! L3 路由契约：**只走公开接口**（`router` / `AppState`），像外部调用方那样用。
//!
//! 这就是计划 §6 里点名的 `cargo test -p mcloud-server --test api_contract`。
//! 放在 `tests/` 而不是 `#[cfg(test)] mod tests` 里，是因为它要证明的正是
//! "这层 API 从外面能不能用" —— 单元测试能看见私有函数，反而证不了这件事。
//!
//! 时间用 `FixedClock` 注入：会话到期这类断言必须能"拨到未来"，
//! 靠真实时间只能写 `sleep`，那既慢又不稳。

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use mcloud_core::FixedClock;
use mcloud_io::SqliteStore;
use mcloud_server::{AppState, ServerConfig, router};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt as _;

const COOKIE_NAME: &str = "mcloud_session";
const PASSWORD: &str = "s3cret-pass";

fn state() -> AppState {
    AppState::new(
        SqliteStore::open_in_memory().unwrap(),
        Arc::new(FixedClock::parse("2026-09-18T00:00:00Z")),
        ServerConfig::default(),
    )
}

fn request(method: Method, path: &str, body: Option<Value>, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(token) = cookie {
        builder = builder.header(header::COOKIE, format!("{COOKIE_NAME}={token}"));
    }
    builder
        .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
        .unwrap()
}

/// 发一个请求，返回状态码、响应头（取 `Set-Cookie` 用）与响应体 JSON。
async fn send(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, body)
}

fn set_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// 从 `Set-Cookie` 里抠出 token 值（浏览器会这么干）。
fn token_of(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix(&format!("{COOKIE_NAME}="))
        .unwrap()
        .to_string()
}

fn credentials() -> Value {
    json!({ "name": "admin", "password": PASSWORD })
}

/// 建好管理员并返回（app, 已登录的 cookie token）。
async fn initialized() -> (Router, String) {
    let app = router(state());
    let (status, headers, _) = send(
        &app,
        request(Method::POST, "/api/setup", Some(credentials()), None),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = token_of(&set_cookie(&headers).expect("初始化应当顺带登录"));
    (app, token)
}

#[tokio::test]
async fn before_setup_only_the_setup_endpoints_answer() {
    let app = router(state());

    let (status, _, body) = send(&app, request(Method::GET, "/api/setup", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["needsSetup"], true);
    assert!(
        body["minPasswordLen"].as_u64().unwrap() > 0,
        "口令长度下限由后端给，前端不硬编"
    );

    let (status, _, _) = send(&app, request(Method::GET, "/api/health", None, None)).await;
    assert_eq!(status, StatusCode::OK);

    // 没管理员就没有"已登录"这回事：其余接口一律 503，而不是 401
    let (status, _, body) = send(&app, request(Method::GET, "/api/accounts", None, None)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "setup_required");
}

#[tokio::test]
async fn setup_creates_the_admin_and_logs_in() {
    let (app, token) = initialized().await;

    let (status, _, body) = send(&app, request(Method::GET, "/api/setup", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["needsSetup"], false, "初始化完就不该再要初始化");

    let (status, _, body) = send(
        &app,
        request(Method::GET, "/api/session", None, Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["name"], "admin");
    assert_eq!(body["user"]["role"], "admin");
    assert!(
        body["user"].get("password_hash").is_none(),
        "口令哈希绝不能出网"
    );
}

#[tokio::test]
async fn setup_cannot_run_twice() {
    let (app, _) = initialized().await;

    let mut second = credentials();
    second["name"] = json!("another");
    let (status, _, body) = send(
        &app,
        request(Method::POST, "/api/setup", Some(second), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap().contains("已经有"),
        "{}",
        body["message"]
    );
}

#[tokio::test]
async fn a_bad_body_keeps_our_error_shape() {
    let app = router(state());

    let (status, _, body) = send(
        &app,
        request(
            Method::POST,
            "/api/setup",
            Some(json!({"name": "admin"})),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad_request");
    assert!(
        body["message"].as_str().unwrap().contains("password"),
        "要指出缺哪个字段: {}",
        body["message"]
    );
}

#[tokio::test]
async fn login_sets_a_hardened_cookie() {
    let (app, _) = initialized().await;

    let (status, headers, body) = send(
        &app,
        request(Method::POST, "/api/session", Some(credentials()), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["name"], "admin");

    let cookie = set_cookie(&headers).expect("登录必须发 cookie");
    for attr in ["HttpOnly", "SameSite=Lax", "Path=/", "Max-Age="] {
        assert!(cookie.contains(attr), "cookie 缺少 {attr}: {cookie}");
    }
    assert!(
        !cookie.contains("Secure"),
        "默认不该打 Secure —— 本地 http 下浏览器会拒收"
    );
}

#[tokio::test]
async fn wrong_password_and_unknown_user_look_the_same() {
    let (app, _) = initialized().await;

    let mut wrong = credentials();
    wrong["password"] = json!("nope");
    let mut unknown = credentials();
    unknown["name"] = json!("nobody");

    let (wrong_status, _, wrong_body) = send(
        &app,
        request(Method::POST, "/api/session", Some(wrong), None),
    )
    .await;
    let (unknown_status, _, unknown_body) = send(
        &app,
        request(Method::POST, "/api/session", Some(unknown), None),
    )
    .await;

    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong_body, unknown_body,
        "两种失败必须无法区分（防用户名枚举）"
    );
    // 前端会把这句话直接显示出来，所以它必须是给人看的：
    // 不能带 `Display` 的类别前缀（"凭据失效: …"），那是给日志的
    assert_eq!(wrong_body["message"], "用户名或口令不正确");
}

#[tokio::test]
async fn session_requires_a_valid_cookie() {
    let (app, token) = initialized().await;

    let (status, _, _) = send(&app, request(Method::GET, "/api/session", None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "没带 cookie 就是未登录");

    let (status, _, _) = send(
        &app,
        request(Method::GET, "/api/session", None, Some("deadbeef")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "伪造 token 不算登录");

    let (status, _, _) = send(
        &app,
        request(Method::GET, "/api/session", None, Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn logout_revokes_immediately() {
    let (app, token) = initialized().await;

    let (status, headers, _) = send(
        &app,
        request(Method::DELETE, "/api/session", None, Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        set_cookie(&headers).unwrap().contains("Max-Age=0"),
        "登出必须清 cookie"
    );

    // cookie 还在浏览器里，但库里那行已经没了 → 立刻失效
    let (status, _, _) = send(
        &app,
        request(Method::GET, "/api/session", None, Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "撤销必须立刻生效");
}

#[tokio::test]
async fn unknown_api_paths_are_json_not_html() {
    let (app, token) = initialized().await;

    let (status, _, body) = send(
        &app,
        request(Method::GET, "/api/accounts", None, Some(&token)),
    )
    .await;
    // 前端路由的回退会把未知路径交给 index.html，而那对 `/api` 是灾难性的：
    // 接口路径写错会表现成"返回一段 HTML"，前端解析失败且毫无线索
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    assert!(body["message"].as_str().unwrap().contains("/api/accounts"));

    // 未登录时优先报未登录（先问"你是谁"，再谈"有没有这个接口"）
    let (status, _, _) = send(&app, request(Method::GET, "/api/accounts", None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn secure_cookie_is_configurable() {
    let state = AppState::new(
        SqliteStore::open_in_memory().unwrap(),
        Arc::new(FixedClock::parse("2026-09-18T00:00:00Z")),
        ServerConfig {
            secure_cookie: true,
        },
    );
    let app = router(state);
    let (_, headers, _) = send(
        &app,
        request(Method::POST, "/api/setup", Some(credentials()), None),
    )
    .await;
    assert!(set_cookie(&headers).unwrap().contains("Secure"));
}
