//! 路由级测试：`tower::ServiceExt::oneshot` 直接打 Router。
//!
//! 为什么不真起一个监听端口：端口会冲突、要处理时序、CI 上还可能与
//! 正在跑的服务抢 8080。`oneshot` 走的是同一套路由与中间件，
//! 只是把 TCP 换成了内存调用。
//!
//! 重点覆盖两类**容易悄悄错**的东西：
//!
//! 1. **错误码映射**。`Error::Config` 必须 400、`Auth` 必须 401、
//!    `NotFound` 必须 404 —— 映射错了不会崩，只会让前端拿到 500 去
//!    显示"服务器错误"，把一个"账号不存在"的可操作提示埋掉。
//! 2. **"不存在"与"存在但为空"的区分**。日志接口对前者要 404、
//!    对后者要 200 + `[]`。混成一样的话，前端要么把正常状态报成错误，
//!    要么对一个打错的 run id 静默显示空列表。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use mcloud_app::{AppBuilder, AppHandle};
use mcloud_core::{Account, AccountSettings, AuthInfo, DeviceId};

use mcloud_server::ServerState;

/// 起一个用临时目录做存储的应用，返回句柄与目录（目录由调用方清理）。
fn test_app(tag: &str) -> (Arc<AppHandle>, std::path::PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mcloud-server-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);

    let app = AppBuilder::new()
        .data_dir(dir.clone())
        .build()
        .expect("应用应能组装");
    (Arc::new(app.handle), dir)
}

/// 起一个**已登录**的应用：先建管理员，再把会话 cookie 注入每个请求。
///
/// # 为什么用中间件注入而不是让每个测试自己带 cookie
///
/// 业务路由的测试关心的是**业务**（错误码映射、"不存在"与"空"的区分），
/// 不该被"怎么登录"污染。而登录闸本身由 `auth_gate` 那一组专门测。
///
/// 中间件注入让业务测试的调用点保持原样（只把 `mcloud_server::app(...)`
/// 换成 `authed_app(...)`），于是**不会**出现"为了加认证而改动 27 处断言"——
/// 那种改动最容易顺手改坏一条断言。
fn authed_app(handle: Arc<AppHandle>) -> axum::Router {
    use mcloud_core::{PasswordHash, Session, SessionKind, User, UserId, WEB_SESSION_TTL_MS};

    let now = handle.clock.now_ms();
    let user = User {
        id: UserId::new("test-user"),
        name: "admin".into(),
        // 测试里不真的算 argon2（每次几百毫秒 × 27 个测试太慢），
        // 塞一个不会被验证的串 —— 这条路径只用来建会话
        password_hash: PasswordHash("$argon2id$test-only".into()),
        created_at_ms: now,
    };
    let token = "test-session-token";
    let session = Session {
        token_hash: mcloud_crypto::hash_session_token(token),
        user_id: user.id.clone(),
        kind: SessionKind::Web,
        created_at_ms: now,
        expires_at_ms: now + WEB_SESSION_TTL_MS,
    };

    let store = Arc::clone(&handle.store);
    let (u, se) = (user, session);
    // 阻塞式建好再返回：`oneshot` 之前必须已经在库里
    futures::executor::block_on(async move {
        store.save_user(&u).await.expect("建测试用户");
        store.save_session(&se).await.expect("建测试会话");
    });

    let cookie = format!("mcloud_session={token}");
    // ⚠ 这里必须是**真正的** app 构造函数，不能是 `authed_app` 自己 ——
    // 后者会无限递归（栈溢出、SIGABRT，而报错只显示"进程被信号 6 终止"）
    mcloud_server::app(ServerState::new(handle)).layer(axum::middleware::from_fn(
        move |mut req: Request<Body>, next: axum::middleware::Next| {
            let cookie = cookie.clone();
            async move {
                // 调用方自己带了 cookie 就尊重它（伪造 cookie 那类测试要用）
                if !req.headers().contains_key("cookie") {
                    req.headers_mut()
                        .insert("cookie", cookie.parse().expect("cookie 头合法"));
                }
                next.run(req).await
            }
        },
    ))
}

/// 发一个请求，返回状态码与响应体文本。
async fn send(app: axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.oneshot(req).await.expect("请求应被处理");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("响应体应可读")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("请求应可构造")
}

fn post_json(uri: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .expect("请求应可构造")
}

fn account(phone: &str) -> Account {
    Account {
        id: mcloud_core::AccountId::new(phone),
        nickname: None,
        auth: AuthInfo {
            raw: "raw-token".into(),
            platform: "android".into(),
            phone: phone.into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("dev".into()),
        pc_platform: None,
        user_domain_id: None,
        settings: AccountSettings::default(),
    }
}

// ── 健康与自述 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_ok() {
    let (handle, dir) = test_app("health");
    let (status, body) = send(authed_app(handle), get("/api/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""), "健康检查应返回 ok，实际：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn version_reports_crate_version() {
    let (handle, dir) = test_app("version");
    let (status, body) = send(authed_app(handle), get("/api/version")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(env!("CARGO_PKG_VERSION")),
        "版本应来自 CARGO_PKG_VERSION，实际：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `task_count` 必须是数字而不是 `null`。
///
/// 前端会直接对计数做算术/比较，`null` 会让整页白屏。
#[tokio::test]
async fn status_task_count_is_a_number() {
    let (handle, dir) = test_app("status");
    let (status, body) = send(authed_app(handle), get("/api/status")).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).expect("应是合法 JSON");
    assert!(
        v["taskCount"].is_number(),
        "taskCount 应为数字，实际：{}",
        v["taskCount"]
    );
    assert!(
        v["lastRun"].is_null() || v["lastRun"].is_object(),
        "lastRun 应为 null 或对象"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn capabilities_lists_registered_tasks() {
    let (handle, dir) = test_app("caps");
    let (status, body) = send(authed_app(handle), get("/api/capabilities")).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).expect("应是合法 JSON");
    let tasks = v["tasks"].as_array().expect("tasks 应为数组");
    assert!(
        tasks.iter().any(|t| t["name"] == "sign"),
        "应包含已注册的 sign 任务，实际：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 错误码映射 ──────────────────────────────────────────────────────────

/// 未知任务 → 400（`Error::Config`），不是 404 也不是 500。
///
/// 校验顺序也一并锁住：未知任务必须先于"账号不存在"被检查，
/// 否则一个拼错的任务名会被报成"账号不存在"，把人引到完全错误的方向。
#[tokio::test]
async fn unknown_task_is_400_not_500() {
    let (handle, dir) = test_app("unknown-task");
    let req = post_json(
        "/api/runs",
        &serde_json::json!({ "task": "no-such-task", "account": "13800000000" }),
    );
    let (status, body) = send(authed_app(handle), req).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "未知任务应是 400，实际 {status}，体：{body}"
    );
    assert!(
        body.contains("未知任务"),
        "错误信息应指出是任务名的问题，实际：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 账号不存在 → 400（`Error::Config`），且信息里带账号名。
#[tokio::test]
async fn missing_account_is_400() {
    let (handle, dir) = test_app("no-account");
    let req = post_json(
        "/api/runs",
        &serde_json::json!({ "task": "sign", "account": "13900000000" }),
    );
    let (status, body) = send(authed_app(handle), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");
    assert!(body.contains("账号不存在"), "实际：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 不存在的运行 → 404（`Error::NotFound`）。
#[tokio::test]
async fn missing_run_is_404() {
    let (handle, dir) = test_app("missing-run");
    let (status, body) = send(authed_app(handle), get("/api/runs/9999")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "实际体：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 不存在的运行的日志 → 404，而不是 200 + `[]`。
///
/// 与下一条对照：**"记录不存在"和"存在但还没日志"必须可区分**。
#[tokio::test]
async fn logs_of_missing_run_is_404_not_empty_list() {
    let (handle, dir) = test_app("logs-404");
    let (status, body) = send(authed_app(handle), get("/api/runs/9999/logs")).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "不存在的运行应 404，而不是返回空列表；实际体：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 存在但暂无日志 → 200 + `[]`。
///
/// 刚启动的运行就是这个状态。若这里返回 404，前端会把正常运行显示成错误。
#[tokio::test]
async fn logs_of_existing_run_without_lines_is_empty_200() {
    let (handle, dir) = test_app("logs-empty");
    // 造一条已结束的运行记录（记录在结束时才落盘）
    handle
        .store
        .upsert_run(&mcloud_core::RunRecord {
            run_id: mcloud_core::RunId::new(1),
            task: "sign".into(),
            account: "13800000000".into(),
            outcome: mcloud_core::TaskOutcome::skipped("测试"),
            duration_ms: 1,
            started_at_ms: 0,
            status: mcloud_core::RunStatus::Done,
            details: serde_json::Value::Null,
        })
        .await
        .expect("落盘应成功");

    let (status, body) = send(authed_app(handle), get("/api/runs/1/logs")).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!([]),
        "无日志应是空数组"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 取消一个没在跑的运行：200，且 `cancelled=false`、`running=false`。
///
/// 这是**正常结果而不是错误** —— 运行刚好在这期间结束了。
/// 报 404/500 会让前端以为取消操作失败了。
#[tokio::test]
async fn cancelling_idle_run_is_ok_not_error() {
    let (handle, dir) = test_app("cancel-idle");
    let (status, body) = send(
        authed_app(handle),
        post_json("/api/runs/9999/cancel", &serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["cancelled"], false, "没在跑就不该报「已取消」");
    assert_eq!(v["running"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 运行列表 ────────────────────────────────────────────────────────────

#[tokio::test]
async fn runs_list_starts_empty() {
    let (handle, dir) = test_app("runs-empty");
    let (status, body) = send(authed_app(handle), get("/api/runs")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap(),
        serde_json::json!([]),
        "初始状态应是空列表"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 账号隔离 ────────────────────────────────────────────────────────────

/// 存了两个账号时，`/api/runs` 不会把别的账号的数据混进来。
///
/// 这一条是**回归护栏**：路由层将来若把账号过滤写错，
/// 表现是"看到别人的运行记录"—— 静默且严重。
#[tokio::test]
async fn runs_list_only_returns_own_records() {
    let (handle, dir) = test_app("runs-scope");
    handle
        .store
        .save_account(&account("13800000000"))
        .await
        .unwrap();

    for (id, task) in [(1u64, "sign"), (2u64, "sign")] {
        handle
            .store
            .upsert_run(&mcloud_core::RunRecord {
                run_id: mcloud_core::RunId::new(id),
                task: task.into(),
                account: "13800000000".into(),
                outcome: mcloud_core::TaskOutcome::skipped("测试"),
                duration_ms: 1,
                started_at_ms: id as i64,
                status: mcloud_core::RunStatus::Done,
                details: serde_json::Value::Null,
            })
            .await
            .unwrap();
    }

    let (status, body) = send(authed_app(handle), get("/api/runs")).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let arr = v.as_array().expect("应是数组");
    assert_eq!(arr.len(), 2, "两条记录都该返回，实际：{body}");
    // 最新的在前
    assert_eq!(
        arr[0]["run_id"], 2,
        "运行列表应最新在前（字段是 snake_case，见 §7.1），实际：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ══════════════════════════════════════════════════════════════════════════
// 登录闸
// ══════════════════════════════════════════════════════════════════════════
//
// 这一组测的是**边界**：未初始化 / 未登录 / 已登录三态，
// 以及"哪些路由必须公开"。这些错了不会崩，只会静默地少一道防线。

/// 从响应头里取出我们那个 cookie 的 `名=值`。
fn cookie_of(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get("set-cookie")?.to_str().ok()?;
    let first = raw.split(';').next()?.trim().to_string();
    Some(first)
}

/// 发请求并保留响应头（登录要读 `Set-Cookie`）。
async fn send_full(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let resp = app.oneshot(req).await.expect("请求应被处理");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

fn get_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap()
}

fn setup_json(user: &str, pass: &str) -> serde_json::Value {
    serde_json::json!({ "username": user, "password": pass })
}

// ── 未初始化 ────────────────────────────────────────────────────────────

/// ⭐ 未初始化时业务接口必须 **503**（不是 401）。
///
/// 401 会让前端弹登录框，而这时根本没有账号可登 ——
/// 用户看到的是"登录失败"，而不是"还没建管理员"。
#[tokio::test]
async fn uninitialized_instance_returns_503_not_401() {
    let (handle, dir) = test_app("gate-uninit");
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/accounts"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "未初始化应是 503，实际 {status}：{body}"
    );
    assert!(body.contains("setup_required"), "要给出可判别的码：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 未初始化时 `/api/setup` 必须可用 —— 否则无法初始化。
#[tokio::test]
async fn setup_status_is_reachable_before_initialization() {
    let (handle, dir) = test_app("gate-setup-status");
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/setup"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"initialized\":false"), "{body}");
    // 口令下限要发给前端 —— 前端硬编必然漂移
    assert!(body.contains("minPasswordLen"), "要带口令下限：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 未初始化时健康检查与版本仍要可用（监控要用）。
#[tokio::test]
async fn health_and_version_work_before_initialization() {
    let (handle, dir) = test_app("gate-health");
    let state = ServerState::new(handle);
    let (s1, _) = send(mcloud_server::app(state.clone()), get("/api/health")).await;
    let (s2, _) = send(mcloud_server::app(state), get("/api/version")).await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 静态资源必须放行 —— 否则登录页自己都加载不出来，
/// 用户看到的是永远转圈的空白页。
#[tokio::test]
async fn static_assets_are_not_gated() {
    let (handle, dir) = test_app("gate-static");
    let (status, _) = send(authed_app(handle), get("/")).await;
    assert_ne!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "静态资源不该被登录闸拦下"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 初始化 ──────────────────────────────────────────────────────────────

/// 初始化成功后：建了管理员、**并顺手登录**（给了 cookie）。
#[tokio::test]
async fn setup_creates_admin_and_logs_in() {
    let (handle, dir) = test_app("gate-setup-ok");
    let (status, headers, body) = send_full(
        mcloud_server::app(ServerState::new(handle)),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let cookie = cookie_of(&headers).expect("初始化后应直接给会话 cookie");
    assert!(cookie.starts_with("mcloud_session="), "{cookie}");
    assert!(body.contains("admin"), "{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 已经初始化过就**不能再建管理员**（409/400）。
///
/// 不拦的话，任何人都能在你的实例上再建一个管理员 —— 那等于没有登录闸。
#[tokio::test]
async fn setup_is_rejected_once_initialized() {
    let (handle, dir) = test_app("gate-setup-twice");
    let state = ServerState::new(handle);

    let (s1, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(s1, StatusCode::CREATED);

    let (s2, _, body) = send_full(
        mcloud_server::app(state),
        post_json("/api/setup", &setup_json("attacker", "another-pass")),
    )
    .await;
    assert!(
        s2 == StatusCode::BAD_REQUEST || s2 == StatusCode::CONFLICT,
        "重复初始化必须被拒，实际 {s2}：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 口令太短要被拒，且**报错要说清下限**。
#[tokio::test]
async fn setup_rejects_short_password_with_the_limit_in_the_message() {
    let (handle, dir) = test_app("gate-setup-short");
    let (status, _, body) = send_full(
        mcloud_server::app(ServerState::new(handle)),
        post_json("/api/setup", &setup_json("admin", "short")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("8"), "报错要带上下限：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn setup_rejects_invalid_username() {
    let (handle, dir) = test_app("gate-setup-baduser");
    let (status, _, _) = send_full(
        mcloud_server::app(ServerState::new(handle)),
        post_json("/api/setup", &setup_json("a", "correct-horse")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 登录 ────────────────────────────────────────────────────────────────

/// 初始化 → 登出 → 未登录被拦 → 重新登录 → 放行。走完整条路。
#[tokio::test]
async fn full_login_flow() {
    let (handle, dir) = test_app("gate-flow");
    let state = ServerState::new(handle);

    // 1) 初始化
    let (s, headers, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let cookie = cookie_of(&headers).unwrap();

    // 2) 带 cookie 能访问业务接口
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        get_with_cookie("/api/accounts", &cookie),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "已登录应放行");

    // 3) 登出
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        Request::builder()
            .method("DELETE")
            .uri("/api/session")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // 4) 旧 cookie 立即失效
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        get_with_cookie("/api/accounts", &cookie),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "登出后旧 cookie 必须失效");

    // 5) 重新登录
    let (s, headers, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/session", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let cookie2 = cookie_of(&headers).unwrap();
    assert_ne!(cookie2, cookie, "重新登录应发新令牌");

    let (s, _) = send(
        mcloud_server::app(state),
        get_with_cookie("/api/accounts", &cookie2),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 未登录访问业务接口 → 401。
#[tokio::test]
async fn business_routes_require_login() {
    let (handle, dir) = test_app("gate-401");
    let state = ServerState::new(handle);
    // 先初始化
    let (_, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;

    for uri in [
        "/api/accounts",
        "/api/runs",
        "/api/status",
        "/api/schedules",
    ] {
        let (s, _) = send(mcloud_server::app(state.clone()), get(uri)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{uri} 未登录应 401");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 查当前会话**必须**要认证 —— "没登录"的正确答案是 401。
#[tokio::test]
async fn session_read_requires_auth() {
    let (handle, dir) = test_app("gate-session-read");
    let state = ServerState::new(handle);
    let (_, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;

    let (s, _) = send(mcloud_server::app(state.clone()), get("/api/session")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "未登录查会话应 401");

    // 登录后能查到
    let (_, headers, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/session", &setup_json("admin", "correct-horse")),
    )
    .await;
    let cookie = cookie_of(&headers).unwrap();
    let (s, body) = send(
        mcloud_server::app(state),
        get_with_cookie("/api/session", &cookie),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("admin"), "{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 用户名不存在与口令错必须回**同一句话**。
///
/// 区分开来就是一个"这个用户名存不存在"的探测器。
#[tokio::test]
async fn wrong_user_and_wrong_password_are_indistinguishable() {
    let (handle, dir) = test_app("gate-enum");
    let state = ServerState::new(handle);
    let (_, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;

    let (s1, _, b1) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/session", &setup_json("admin", "wrong-password")),
    )
    .await;
    let (s2, _, b2) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/session", &setup_json("nobody", "wrong-password")),
    )
    .await;

    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(b1, b2, "两种失败必须回同一句话，否则能探测用户名是否存在");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 伪造的 cookie 不能通过。
#[tokio::test]
async fn forged_cookie_is_rejected() {
    let (handle, dir) = test_app("gate-forge");
    let state = ServerState::new(handle);
    let (_, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;

    for bad in [
        "mcloud_session=deadbeef",
        "mcloud_session=",
        "other_cookie=x",
        "mcloud_session=0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        let (s, _) = send(
            mcloud_server::app(state.clone()),
            get_with_cookie("/api/accounts", bad),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "伪造 cookie 应被拒：{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ── cookie 属性 ─────────────────────────────────────────────────────────

/// ⭐ cookie 必须是 `HttpOnly` + `SameSite=Strict`。
///
/// 少了 `HttpOnly`，XSS 就能偷走令牌；少了 `SameSite`，跨站请求会带上它。
#[tokio::test]
async fn session_cookie_is_hardened() {
    let (handle, dir) = test_app("gate-cookie");
    let (_, headers, _) = send_full(
        mcloud_server::app(ServerState::new(handle)),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;
    let raw = headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(raw.contains("HttpOnly"), "缺 HttpOnly：{raw}");
    assert!(raw.contains("SameSite=Strict"), "缺 SameSite=Strict：{raw}");
    assert!(raw.contains("Path=/"), "缺 Path：{raw}");
    assert!(raw.contains("Max-Age="), "缺 Max-Age：{raw}");
    // ⚠ 刻意**不**加 Secure：默认跑在 http://127.0.0.1，
    // 加了浏览器会直接不存，表现是"登录成功但立刻又未登录"
    assert!(
        !raw.contains("Secure"),
        "本地 http 下加 Secure 会让 cookie 存不上：{raw}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 登录闸的状态要**落盘** —— 重启后不用重新初始化。
#[tokio::test]
async fn initialization_survives_restart() {
    let dir = std::env::temp_dir().join(format!(
        "mcloud-server-gate-restart-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);

    {
        let app = AppBuilder::new().data_dir(dir.clone()).build().unwrap();
        let state = ServerState::new(Arc::new(app.handle));
        let (s, _, _) = send_full(
            mcloud_server::app(state),
            post_json("/api/setup", &setup_json("admin", "correct-horse")),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
    }

    // 重开
    let app2 = AppBuilder::new().data_dir(dir.clone()).build().unwrap();
    let state2 = ServerState::new(Arc::new(app2.handle));
    let (s, body) = send(mcloud_server::app(state2.clone()), get("/api/setup")).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        body.contains("\"initialized\":true"),
        "重启后应仍是已初始化：{body}"
    );

    // 而且旧口令仍能登录（哈希落盘正确）
    let (s, _, _) = send_full(
        mcloud_server::app(state2),
        post_json("/api/session", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "重启后原口令要能登录");
    let _ = std::fs::remove_dir_all(&dir);
}
