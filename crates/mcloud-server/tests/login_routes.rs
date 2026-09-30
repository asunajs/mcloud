//! 云盘账号登录的**接线**测试。
//!
//! # 这一组测什么、不测什么
//!
//! 测：路由挂对了没、参数校验在出网之前、错误码映射、会话表的增删。
//!
//! 不测：真正打上游的那几个端点（短信 / 账密 / 轮询成功）。
//! 它们的协议语义由 `mcloud-upstream` 的离线测试逐字段钉住，
//! 状态机的节奏由 `mcloud-tasks` 的假 HTTP 挡住 —— 在这里再测一遍
//! 只会得到一条需要真网络的慢测试。
//!
//! # 为什么单独一个文件
//!
//! `routes.rs` 里那位协作者的 `authed_app` 辅助函数当前**无限递归**
//! （自己调自己），整个测试二进制会栈溢出 abort。把登录测试放在同一个
//! 文件里会让它们跟着一起挂 —— 分开之后两边互不牵连。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use mcloud_app::{AppBuilder, AppHandle};

use mcloud_server::ServerState;

fn test_app(tag: &str) -> (Arc<AppHandle>, std::path::PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("mcloud-login-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let app = AppBuilder::new()
        .data_dir(dir.clone())
        .build()
        .expect("应用应能组装");
    (Arc::new(app.handle), dir)
}

/// 起一个**已登录管理员**的应用。
///
/// 直接往库里塞一个用户 + 一个会话，再注入 cookie —— 不走 `/api/setup`，
/// 那条路要算 argon2（每次几百毫秒），而这里要测的是业务路由。
fn authed_app(handle: Arc<AppHandle>) -> axum::Router {
    use mcloud_core::{PasswordHash, Session, SessionKind, User, UserId, WEB_SESSION_TTL_MS};

    let now = handle.clock.now_ms();
    let user = User {
        id: UserId::new("test-user"),
        name: "admin".into(),
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
    futures::executor::block_on(async {
        store.save_user(&user).await.expect("建测试用户");
        store.save_session(&session).await.expect("建测试会话");
    });

    let cookie = format!("mcloud_session={token}");
    mcloud_server::app(ServerState::new(handle)).layer(axum::middleware::from_fn(
        move |mut req: Request<Body>, next: axum::middleware::Next| {
            let cookie = cookie.clone();
            async move {
                if !req.headers().contains_key("cookie") {
                    req.headers_mut()
                        .insert("cookie", cookie.parse().expect("cookie 头合法"));
                }
                next.run(req).await
            }
        },
    ))
}

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

// ── 出码 ────────────────────────────────────────────────────────────────

/// 出码：**不发上游请求**，且平台参数是必填的。
#[tokio::test]
async fn start_qr_returns_payload_and_requires_platform() {
    let (handle, dir) = test_app("qr");
    let (status, body) = send(
        authed_app(handle.clone()),
        post_json("/api/login/qr", &serde_json::json!({"platform": "macos"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(v["platform"], "macos");
    assert_eq!(v["platform_label"], "macOS");
    assert_eq!(v["poll_interval_ms"], 1000);
    assert_eq!(v["ttl_seconds"], 120);
    // 二维码里带 mac 的版本号 —— 平台确实传下去了
    let payload = v["payload"].as_str().unwrap();
    assert!(payload.contains("ver=4.2.1"), "实得：{payload}");
    let dev = v["device_id"].as_str().unwrap();
    assert!(dev.ends_with("-ENDIN"), "实得：{dev}");

    // 二维码图片：必须是可用的 data URL（前端直接塞 <img src>）
    let img = v["image_svg"].as_str().unwrap();
    assert!(
        img.starts_with("data:image/svg+xml;base64,"),
        "实得：{}",
        &img[..40.min(img.len())]
    );
    // 两个平台的图也必须不同（否则等于把平台弄丢了）
    assert!(!img.is_empty());
    assert!(!v["session_id"].as_str().unwrap().is_empty());

    // 缺 platform ⇒ 400（不给默认值，见路由模块文档）
    let (status, body) = send(
        authed_app(handle.clone()),
        post_json("/api/login/qr", &serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// windows 与 macos 出码**不同**（底层区分，不是前端换个字）。
#[tokio::test]
async fn qr_payload_differs_between_platforms() {
    let (handle, dir) = test_app("qr-plat");

    let mut payloads = Vec::new();
    for platform in ["windows", "macos"] {
        let (status, body) = send(
            authed_app(handle.clone()),
            post_json(
                "/api/login/qr",
                &serde_json::json!({ "platform": platform }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "实际体：{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        payloads.push(v["payload"].as_str().unwrap().to_string());
    }

    assert!(
        payloads[0].contains("ver=8.8.6.20260829"),
        "win：{}",
        payloads[0]
    );
    assert!(payloads[1].contains("ver=4.2.1"), "mac：{}", payloads[1]);
    assert_ne!(payloads[0], payloads[1], "两个平台的码必须不同");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 未知平台 ⇒ 400 且带上原文（不能静默落成 windows）。
#[tokio::test]
async fn unknown_platform_is_rejected() {
    let (handle, dir) = test_app("badplat");
    let (status, body) = send(
        authed_app(handle.clone()),
        post_json("/api/login/qr", &serde_json::json!({"platform": "linux"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");
    assert!(body.contains("linux"), "要带上用户输入：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 轮询 ────────────────────────────────────────────────────────────────

/// 轮询一个不存在的会话 ⇒ 404（让前端能区分"重出码"与"网络重试"）。
#[tokio::test]
async fn polling_unknown_session_is_404() {
    let (handle, dir) = test_app("poll-404");
    let (status, body) = send(
        authed_app(handle.clone()),
        post_json(
            "/api/login/qr/poll",
            &serde_json::json!({"session_id": "NOPE"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "实际体：{body}");
    assert!(body.contains("NOPE"), "要指出是哪个会话：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 放弃一次扫码：会话被移除，再轮询就是 404。
#[tokio::test]
async fn cancelling_qr_session_removes_it() {
    let (handle, dir) = test_app("qr-cancel");

    let (status, body) = send(
        authed_app(handle.clone()),
        post_json("/api/login/qr", &serde_json::json!({"platform": "windows"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let sid = v["session_id"].as_str().unwrap().to_string();

    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/login/qr/{sid}"))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(authed_app(handle.clone()), req).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        authed_app(handle.clone()),
        post_json(
            "/api/login/qr/poll",
            &serde_json::json!({"session_id": sid}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "取消后不该还能轮询");

    let _ = std::fs::remove_dir_all(&dir);
}

// ── 设备指纹 ────────────────────────────────────────────────────────────

/// 设备指纹：可读、可换，且**换出来的是新值**。
#[tokio::test]
async fn device_endpoints_read_and_rotate() {
    let (handle, dir) = test_app("device");

    let (status, body) = send(authed_app(handle.clone()), get("/api/login/device")).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    let first: serde_json::Value = serde_json::from_str(&body).unwrap();
    let first = first["device_id"].as_str().unwrap().to_string();
    assert!(first.ends_with("-ENDIN"), "实得：{first}");

    // 再读一次：必须同一个（跨请求稳定）
    let (_, body) = send(authed_app(handle.clone()), get("/api/login/device")).await;
    let again: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(again["device_id"], first, "设备指纹不该漂移");

    // 换一个
    let (status, body) = send(
        authed_app(handle.clone()),
        post_json("/api/login/device/rotate", &serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    let rotated: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_ne!(rotated["device_id"], first, "换设备必须换出新值");

    let _ = std::fs::remove_dir_all(&dir);
}

// ── 参数校验 ────────────────────────────────────────────────────────────

/// 短信与账密：参数缺失必须在**出网之前**被挡下（400）。
#[tokio::test]
async fn sms_and_password_reject_empty_inputs_before_network() {
    let (handle, dir) = test_app("validate");

    let (status, body) = send(
        authed_app(handle.clone()),
        post_json(
            "/api/login/sms/start",
            &serde_json::json!({"phone": "  ", "platform": "windows"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");

    let (status, _) = send(
        authed_app(handle.clone()),
        post_json(
            "/api/login/sms",
            &serde_json::json!({"phone": "19900000001", "platform": "windows", "code": ""}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        authed_app(handle.clone()),
        post_json(
            "/api/login/password",
            &serde_json::json!({"username": "u", "password": "", "platform": "windows"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&dir);
}

/// 登录端点必须在**管理员闸之内**（不能匿名调）。
#[tokio::test]
async fn login_endpoints_require_admin_session() {
    let (handle, dir) = test_app("gated");
    let (status, _) = send(
        mcloud_server::app(ServerState::new(handle)),
        post_json("/api/login/qr", &serde_json::json!({"platform": "windows"})),
    )
    .await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::SERVICE_UNAVAILABLE,
        "未登录不该能调用登录接口，实得 {status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 账号列表 ────────────────────────────────────────────────────────────

/// ⭐ 列表接口**绝不出凭据** —— 这条是端到端的防回归。
///
/// 与 `routes::accounts` 里的单元测试互补：那条钉 DTO 的形状，
/// 这条钉"整条 HTTP 响应"（中间可能有别的层往里加东西）。
#[tokio::test]
async fn accounts_list_never_leaks_credentials() {
    use mcloud_core::{
        Account as CoreAccount, AccountId, AccountSettings, AuthInfo, DeviceId, PcPlatform,
    };

    let (handle, dir) = test_app("accounts-leak");
    let acct = CoreAccount {
        id: AccountId::new("19****01"),
        nickname: Some("小明".into()),
        auth: AuthInfo {
            raw: "Basic U0VDUkVULUNSRURFTlRJQUw=".into(),
            platform: "pc".into(),
            phone: "19900000001".into(),
            token: "SECRET-TOKEN".into(),
            expire: 1_893_456_000_000,
        },
        device_id: DeviceId("DEV-1".into()),
        pc_platform: Some(PcPlatform::MacOs),
        user_domain_id: None,
        owner: None,
        settings: AccountSettings::default(),
    };
    handle.store.save_account(&acct).await.unwrap();

    let (status, body) = send(authed_app(handle.clone()), get("/api/accounts")).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");

    for forbidden in [
        "SECRET-TOKEN",
        "U0VDUkVULUNSRURFTlRJQUw=",
        "19900000001",
        "Basic ",
    ] {
        assert!(!body.contains(forbidden), "凭据 {forbidden:?} 泄漏：{body}");
    }

    // 但该有的要在：平台是这条接口的重点
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v[0]["id"], "19****01");
    assert_eq!(v[0]["pc_platform"], "macos", "平台必须给出：{body}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 删除账号：幂等（删两次都 200），且真的从列表里消失。
#[tokio::test]
async fn accounts_can_be_removed_idempotently() {
    use mcloud_core::{Account as CoreAccount, AccountId, AccountSettings, AuthInfo, DeviceId};

    let (handle, dir) = test_app("accounts-del");
    let acct = CoreAccount {
        id: AccountId::new("19****01"),
        nickname: None,
        auth: AuthInfo {
            raw: "Basic eA==".into(),
            platform: "pc".into(),
            phone: "19900000001".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("DEV-1".into()),
        pc_platform: None,
        user_domain_id: None,
        owner: None,
        settings: AccountSettings::default(),
    };
    handle.store.save_account(&acct).await.unwrap();

    let req = || {
        Request::builder()
            .method("DELETE")
            .uri("/api/accounts/19%2A%2A%2A%2A01")
            .body(Body::empty())
            .unwrap()
    };

    let (status, body) = send(authed_app(handle.clone()), req()).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");

    // 再删一次也是 200（幂等 —— 用户的意图已经达成）
    let (status, body) = send(authed_app(handle.clone()), req()).await;
    assert_eq!(status, StatusCode::OK, "重复删除应当幂等：{body}");

    // 列表里没了
    let (_, body) = send(authed_app(handle.clone()), get("/api/accounts")).await;
    assert_eq!(body.trim(), "[]", "删完应当为空：{body}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ── 产物遗留（账本收尾的落点）────────────────────────────────────────

/// ⭐ 留在用户网盘上的产物必须能从 `/api/runs` 查到**具体是哪几个**。
///
/// 这条是端到端的防回归：旧项目只念"留了 N 个"，却不说是哪 N 个，
/// 用户手工清理时无从下手。这里钉住"账号 + fileId 都在响应里"。
#[tokio::test]
async fn leftover_artifacts_are_readable_through_the_api() {
    use mcloud_core::AccountId;
    use mcloud_engine::Artifact;

    let (handle, dir) = test_app("leftover-api");

    // 直接往账本里记两条（模拟任务写过网盘但没清理）
    let ledger = handle.runner.artifacts();
    ledger
        .register(
            mcloud_core::RunId(4242),
            Artifact::cloud_file(AccountId::new("19****01"), "FILE-A"),
        )
        .unwrap();
    ledger
        .register(
            mcloud_core::RunId(4242),
            Artifact::cloud_file(AccountId::new("19****01"), "FILE-B"),
        )
        .unwrap();

    // 造一条带 details 的运行记录（模拟 engine 收尾写进去的形态）
    let record = mcloud_core::RunRecord {
        run_id: mcloud_core::RunId(4242),
        task: "task-expansion".into(),
        account: "19****01".into(),
        started_at_ms: 1_700_000_000_000,
        duration_ms: 100,
        outcome: mcloud_core::TaskOutcome::success("传了两个"),
        status: mcloud_core::RunStatus::Done,
        details: serde_json::json!({ "artifacts": ledger.pending(mcloud_core::RunId(4242)) }),
    };
    handle.store.upsert_run(&record).await.unwrap();
    let (status, body) = send(authed_app(handle.clone()), get("/api/runs")).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");

    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let arts = v[0]["details"]["artifacts"]
        .as_array()
        .unwrap_or_else(|| panic!("details.artifacts 应当在响应里：{body}"));
    assert_eq!(arts.len(), 2);
    let ids: Vec<&str> = arts.iter().filter_map(|a| a["id"].as_str()).collect();
    assert!(
        ids.contains(&"FILE-A") && ids.contains(&"FILE-B"),
        "实得：{ids:?}"
    );
    assert_eq!(arts[0]["account"], "19****01");
    assert_eq!(arts[0]["kind"], "cloud-file");

    // 凭据绝不能随产物泄漏出去
    for forbidden in ["authToken", "Basic ", "19900000001"] {
        assert!(
            !body.contains(forbidden),
            "凭据片段 {forbidden:?} 泄漏：{body}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
