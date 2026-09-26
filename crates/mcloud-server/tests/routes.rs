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
        settings: AccountSettings::default(),
    }
}

// ── 健康与自述 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_ok() {
    let (handle, dir) = test_app("health");
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/health"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\""), "健康检查应返回 ok，实际：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn version_reports_crate_version() {
    let (handle, dir) = test_app("version");
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/version"),
    )
    .await;
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
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/status"),
    )
    .await;
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
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/capabilities"),
    )
    .await;
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
    let (status, body) = send(mcloud_server::app(ServerState::new(handle)), req).await;
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
    let (status, body) = send(mcloud_server::app(ServerState::new(handle)), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");
    assert!(body.contains("账号不存在"), "实际：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 不存在的运行 → 404（`Error::NotFound`）。
#[tokio::test]
async fn missing_run_is_404() {
    let (handle, dir) = test_app("missing-run");
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/runs/9999"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "实际体：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 不存在的运行的日志 → 404，而不是 200 + `[]`。
///
/// 与下一条对照：**"记录不存在"和"存在但还没日志"必须可区分**。
#[tokio::test]
async fn logs_of_missing_run_is_404_not_empty_list() {
    let (handle, dir) = test_app("logs-404");
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/runs/9999/logs"),
    )
    .await;
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

    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/runs/1/logs"),
    )
    .await;
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
        mcloud_server::app(ServerState::new(handle)),
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
    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/runs"),
    )
    .await;
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

    let (status, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get("/api/runs"),
    )
    .await;
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
