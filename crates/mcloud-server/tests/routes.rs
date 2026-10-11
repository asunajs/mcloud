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
use mcloud_core::{Account, AccountSettings, AuthInfo, DeviceId, PasswordHash, User, UserId};

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

/// 「谁来都报网络错」的假上游（见 [`test_app_with_dead_upstream`] ✓）。
struct DeadUpstream;

#[async_trait::async_trait]
impl mcloud_core::HttpClient for DeadUpstream {
    async fn request(
        &self,
        req: mcloud_core::HttpRequest,
    ) -> mcloud_core::Result<mcloud_core::HttpResponse> {
        // ⚠ 回 `Err` 而不是 panic：重算把"上游取不到"当**一条正常分支**处理
        //（⇒ 走池里那份兜底 ✓），而这里要验的正是那条路 ✓。
        Err(mcloud_core::Error::Network(format!(
            "路由测试的假上游：不打网络（{}）",
            req.url
        )))
    }
}

/// 起一个**按账号的 HTTP 客户端是假的**应用（只有「直播自动定时」那几条用例用 ✓）。
///
/// # 为什么必须注入（不能让它走生产那份）
///
/// 那条定时的重算会**按账号去上游现取**「下一场」（用户口径 2026-10-08 ✓），
/// 而生产那份工厂会把请求打到**真的上游**（`user-njs.yun.139.com` ✗）——
/// 路由测试里那既不可能也不该发生（与 [`test_app_with_notify_http`] 同一条理由 ✓）。
/// 给一个"谁来都报网络错"的客户端之后，重算走**池里那份兜底** ✓，
/// 于是那几条用例的期望（时刻来自池 ✓）原样成立 ✓。
fn test_app_with_dead_upstream(tag: &str) -> (Arc<AppHandle>, std::path::PathBuf) {
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
        .http_for(Arc::new(|_a: &Account| {
            Arc::new(DeadUpstream) as Arc<dyn mcloud_core::HttpClient>
        }))
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
    seed_test_user_and_session(&handle);
    authed_app_with(ServerState::new(handle))
}

/// 建一个测试用户 + 一条 Web 会话（`test-user` / [`TEST_SESSION_TOKEN`]）。
///
/// # 为什么单独一个函数
///
/// [`authed_app`] 顺带做了这件事，但有些用例要**自己造 `ServerState`**
///（往里登记"这条 run 归属谁"），那些用例绕过了 `authed_app` ⇒ 必须能单独
/// 把会话建出来（2026-10-06 审计 `SEC-20` 的用例第一次就漏了这一步：
/// 请求直接 401，而报错看起来像"鉴权坏了"✗）。
fn seed_test_user_and_session(handle: &Arc<AppHandle>) -> mcloud_core::UserId {
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
    let token = TEST_SESSION_TOKEN;
    let user_id = user.id.clone();
    let session = Session {
        token_hash: mcloud_crypto::hash_session_token(token),
        user_id: user.id.clone(),
        kind: SessionKind::Web,
        created_at_ms: now,
        expires_at_ms: now + WEB_SESSION_TTL_MS,
    };

    let store = Arc::clone(&handle.store);
    let (u, se, uid) = (user, session, user_id);
    // 阻塞式建好再返回：`oneshot` 之前必须已经在库里
    futures::executor::block_on(async move {
        store.save_user(&u).await.expect("建测试用户");
        store.save_session(&se).await.expect("建测试会话");
    });
    uid
}

/// 给**调用方自己造好的** `ServerState` 套上"注入测试 cookie"的中间件。
///
/// # 为什么要有这一版
///
/// 有些用例要往 `ServerState` 里登记东西（例如"这条 run 归属谁"的
/// `note_run_owner`），而 [`authed_app`] 内部自己 `new` 了一个 state ⇒
/// 登记的那个和真正处理请求的那个**不是同一个** ✗
///（2026-10-06 审计 `SEC-20` 的用例就是这么踩到的）。
fn authed_app_with(state: ServerState) -> axum::Router {
    let cookie = format!("mcloud_session={TEST_SESSION_TOKEN}");
    // ⚠ 这里必须是**真正的** app 构造函数，不能是 `authed_app` 自己 ——
    // 后者会无限递归（栈溢出、SIGABRT，而报错只显示"进程被信号 6 终止"）
    mcloud_server::app(state).layer(axum::middleware::from_fn(
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

/// 测试用的会话令牌（[`authed_app`] 与 [`authed_app_with`] 必须一致）。
const TEST_SESSION_TOKEN: &str = "test-session-token";

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
        owner: None,
        enabled: true,
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

// ── 多账号 / 全部账号（用户口径 2026-10-01）──────────────────────────────
//
// 「任何（包括直播小红花）应该允许配置多账号/全部，点击运行后后端保证串行执行」。
// 串行本身由 `mcloud-engine` 的 `start_serial_runs_accounts_one_at_a_time_in_order`
// 锁住（那条测的是"没有重叠 + 顺序一致"）；这里锁**请求口径**。

/// **不猜**：既没给 `account` 也没给 `accounts` ⇒ 400。
///
/// 这条是刻意的严格：`accounts: []` 的语义是"我名下**全部**账号"，
/// 而"没写"如果也当成"全部"，一次手滑就是把名下每个号都真跑一遍。
#[tokio::test]
async fn run_without_any_account_is_400() {
    let (handle, dir) = test_app("run-no-account");
    let req = post_json("/api/runs", &serde_json::json!({ "task": "sign" }));
    let (status, body) = send(authed_app(handle), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");
    assert!(body.contains("缺少账号"), "实际：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `accounts: []`（全部）而名下没有账号 ⇒ 400，且**不是**「缺少账号」。
///
/// 两条错误必须可区分：一个是"你没给"，一个是"给了但没有可跑的账号"。
#[tokio::test]
async fn run_with_empty_account_list_is_400_when_user_has_none() {
    let (handle, dir) = test_app("run-all-empty");
    let req = post_json(
        "/api/runs",
        &serde_json::json!({ "task": "sign", "accounts": [] }),
    );
    let (status, body) = send(authed_app(handle), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");
    assert!(body.contains("没有可运行的账号"), "实际：{body}");
    assert!(
        !body.contains("缺少账号"),
        "给了空数组是「全部」的意思，不该报成「没给账号」：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `accounts` 数组里的账号不存在 ⇒ 400（与单账号同一套归属规则）。
#[tokio::test]
async fn run_with_unknown_account_in_array_is_400() {
    let (handle, dir) = test_app("run-array-unknown");
    let req = post_json(
        "/api/runs",
        &serde_json::json!({ "task": "sign", "accounts": ["13900000000"] }),
    );
    let (status, body) = send(authed_app(handle), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际体：{body}");
    assert!(body.contains("账号不存在"), "实际：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 多账号：**一个账号一条 run**，id 一次给全 —— 界面据此逐条订阅、逐条中止。
#[tokio::test]
async fn run_with_multiple_accounts_returns_one_run_id_per_account() {
    let (handle, dir) = test_app("run-multi");
    seed_account(&handle, account("13800000000"));
    seed_account(&handle, account("13800000001"));
    // `accounts: []` = 我名下全部；后台执行 ⇒ 立刻返回一组 id（不等任务跑完）
    let req = post_json(
        "/api/runs",
        &serde_json::json!({ "task": "sign", "accounts": [], "background": true }),
    );
    let (status, body) = send(authed_app(handle), req).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("应是合法 JSON");
    let ids = v["run_ids"].as_array().expect("run_ids 应为数组");
    assert_eq!(ids.len(), 2, "两个账号应给两个 run id：{body}");
    assert_eq!(v["run_id"], ids[0], "run_id 是兼容字段，等于第一个：{body}");
    assert_ne!(ids[0], ids[1], "两个 run id 必须不同：{body}");
    let accounts = v["accounts"].as_array().expect("accounts 应为数组");
    assert_eq!(
        accounts.len(),
        2,
        "accounts 应列出本次涉及的全部账号：{body}"
    );
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
            logs: Vec::new(),
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
                logs: Vec::new(),
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

/// ⭐ 「重启后回看日志」那条路的**接口形状**：列表不带日志、按 id 取详情带日志。
///
/// 前端正是这两步（`/api/runs` 拿列表 ⇒ 点一条 ⇒ `getRun(id)` 拿详情，
/// 见 `web/src/pages/history-page.tsx::openRunLog` ✓），详情走 `routes/runs.rs::get_one`。
/// 列表**故意**剥掉日志（100 条 × 200 行不是列表该有的体量 ✓）⇒
/// 这里把两侧都钉住：列表剥得干净（连 `logs` 键都不出现 ✓）、详情一条不少 ✓。
///
/// ⚠ 后半段**真的重开一次**（丢掉第一个实例、用 `AppBuilder` 在**同一份数据目录**
/// 上再组装一个）—— 这才是用户说的"重启"在进程内的等价物 ✓。
///
/// ⚠⚠ 2026-10-09：日志的**家换了**（用户口径「日志和历史不要全加入内存，
/// 使用本地记录」✓）—— 现在写进 `data/logs/<id>.json`（`append_logs` ✓），
/// 而 `history.json` 里一个字节都不带 ✓；详情那格由 `get_one` **现取** ✓。
#[tokio::test]
async fn run_list_strips_logs_and_detail_keeps_them() {
    let (handle, dir) = test_app("runs-logs-shape");
    handle
        .store
        .upsert_run(&mcloud_core::RunRecord {
            run_id: mcloud_core::RunId::new(1),
            task: "sign".into(),
            account: "13800000000".into(),
            outcome: mcloud_core::TaskOutcome::success("ok"),
            duration_ms: 1,
            started_at_ms: 1,
            status: mcloud_core::RunStatus::Done,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        })
        .await
        .unwrap();
    // 日志走**唯一**那条路：`data/logs/<id>.json` ✓（带 `kind`，界面按它上颜色 ✓）
    handle
        .store
        .append_logs(
            mcloud_core::RunId::new(1),
            &[
                mcloud_core::LogLine::new(
                    1,
                    mcloud_core::LogLevel::Info,
                    "第一行",
                    1,
                    mcloud_core::Seq(1),
                )
                .with_kind(mcloud_core::LogKind::Info),
                mcloud_core::LogLine::new(
                    1,
                    mcloud_core::LogLevel::Error,
                    "第二行",
                    2,
                    mcloud_core::Seq(2),
                )
                .with_kind(mcloud_core::LogKind::Fail),
            ],
        )
        .await
        .unwrap();

    // ⭐ 历史文件里**不带**日志正文（"不要全加入内存"的落点 ✓）
    let history = std::fs::read_to_string(dir.join("history.json")).unwrap();
    assert!(
        !history.contains("第一行"),
        "history.json 里不该有日志正文：{history}"
    );
    assert!(
        dir.join("logs").join("1.json").exists(),
        "日志该落在自己的文件里"
    );

    let (status, body) = send(authed_app(Arc::clone(&handle)), get("/api/runs")).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        v[0].get("logs").is_none(),
        "列表必须**不带**日志（连键都不该出现），实际：{body}"
    );
    // 汇总字段照旧在（剥的只是日志那一格 ✓）
    assert_eq!(v[0]["run_id"], 1, "实际：{body}");
    assert_eq!(v[0]["task"], "sign", "实际：{body}");

    let (status, body) = send(authed_app(Arc::clone(&handle)), get("/api/runs/1")).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let logs = v["logs"].as_array().expect("详情必须带日志");
    assert_eq!(logs.len(), 2, "实际：{body}");
    assert_eq!(logs[0]["message"], "第一行", "实际：{body}");
    assert_eq!(logs[0]["level"], "info", "实际：{body}");
    assert_eq!(logs[0]["kind"], "info", "实际：{body}");
    assert_eq!(logs[1]["message"], "第二行", "实际：{body}");
    assert_eq!(logs[1]["level"], "error", "实际：{body}");
    assert_eq!(logs[1]["kind"], "fail", "实际：{body}");

    // ── 重启（同一份数据目录上重新组装一个实例）⇒ 详情仍要拿到那两行 ──
    drop(handle);
    let restarted = AppBuilder::new()
        .data_dir(dir.clone())
        .build()
        .expect("重启后应用应能组装");
    let (status, body) = send(authed_app(Arc::new(restarted.handle)), get("/api/runs/1")).await;
    assert_eq!(status, StatusCode::OK, "重启后按 id 取详情：{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let logs = v["logs"].as_array().expect("重启后详情仍必须带日志");
    assert_eq!(logs.len(), 2, "重启后行数不变：{body}");
    assert_eq!(logs[0]["message"], "第一行", "重启后逐行相同：{body}");
    assert_eq!(logs[1]["message"], "第二行", "重启后逐行相同：{body}");
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
    // ⚠ 这一段包在**超时**里，不是保险起见，是防挂死：
    // 这个 helper 的语义是"响应会结束"。若有人拿它去打一条不结束的流
    // （SSE 就是），整轮 `cargo test` 会静默挂住 —— 实测发生过一次，
    // 表现是 `cargo test` 十几分钟不返回、也不报错，只能靠 `sample`
    // 看调用栈才知道卡在哪。超时把"静默挂死"变成"一条失败的断言"。
    let fut = async {
        let resp = app.oneshot(req).await.expect("请求应被处理");
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, bytes)
    };
    let (status, headers, bytes) = tokio::time::timeout(std::time::Duration::from_secs(10), fut)
        .await
        .expect(
            "响应 10s 内没有结束 —— 这个 helper 只能用于**会结束**的响应；无限流（SSE）请用 send_headers_only",
        );
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

/// 只取**响应头**，不碰 body。
///
/// # 为什么必须有这个（而不是复用 `send_full`）
///
/// SSE 是**无限流**：`send_full` 会 `collect()` 整个 body，而那永远不会结束
/// —— 实测直接挂死，`cargo test` 十分钟不返回。
///
/// 而"SSE 有没有被压缩"这件事**只看响应头就够了**，body 一个字节都不用读。
/// 所以这里刻意把 body 丢掉。
///
/// ⚠ 别用它测"内容对不对" —— 它拿不到 body。那种断言用 `send_full`。
async fn send_headers_only(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap) {
    let resp = app.oneshot(req).await.expect("请求应被处理");
    let status = resp.status();
    let headers = resp.headers().clone();
    // ⚠ **不能** `into_body().collect()`：这条流要等下一个事件或 15s 的
    // keep-alive 才产出，而订阅存在期间永远不会有 `None` —— 实测挂死，
    // `cargo test` 十分钟不返回。响应头已经在手上，body 直接丢掉即可。
    // （落一条**终局**事件就能让流正常收尾，见 `sse_streams_are_never_compressed`。）
    drop(resp);
    (status, headers)
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

// ── 静态资源的传输策略（压缩 + 缓存）──────────────────────────────────
//
// 这一组测的不是业务，而是**"省流量"的两条承诺会不会被悄悄改回去**。
// 它们都"坏了也不报错"：少一个 `Content-Encoding`，站点照常运行，
// 只是每次多传 220KB；少一个 `immutable`，只是每次刷新白跑一趟 304。
// 正因如此才需要断言锁住。
//
// ⚠ 这些断言**依赖 `web/dist` 已构建**（`rust-embed` 编译期内嵌）。
// 未构建时资产集为空，测试会跳过而不是失败 —— 不让 `cargo test`
// 反过来依赖 npm（与 `static_files.rs` 里那条注释同一个取舍）。

/// 带哈希的产物：`assets/*` 必须能压、且必须长缓存。
#[tokio::test]
async fn hashed_assets_are_compressed_and_immutable() {
    // 前端未构建时跳过（见上面那段注释）。
    let Some(asset) = mcloud_server::static_files::find_hashed_asset(".css") else {
        return;
    };
    let (handle, dir) = test_app("static-hashed");
    let app = authed_app(handle);

    // ① 客户端愿意收 gzip ⇒ 必须回 `content-encoding: gzip`
    let (status, headers, _) = send_full(
        app.clone(),
        Request::builder()
            .uri(format!("/{asset}"))
            .header("accept-encoding", "gzip")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-encoding").map(|v| v.to_str().unwrap()),
        Some("gzip"),
        "带哈希的 {asset} 没有 gzip —— 首屏会多传几百 KB"
    );
    // 有 `content-encoding` 就必须有 `vary`，否则中间缓存会把压缩版
    // 发给不支持 gzip 的客户端。
    assert_eq!(
        headers.get("vary").map(|v| v.to_str().unwrap()),
        Some("accept-encoding"),
    );

    // ② 缓存：文件名带内容哈希 ⇒ 一年 immutable
    let cc = headers
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        cc.contains("immutable") && cc.contains("max-age=31536000"),
        "{asset} 的 cache-control 不是长缓存：{cc}"
    );

    // ③ 不压缩的客户端拿到的是可读原文，不能被压坏
    let (status, headers, body) = send_full(
        app,
        Request::builder()
            .uri(format!("/{asset}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-encoding"), None);
    assert!(!body.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

/// 入口 HTML：gzip 可以，但**绝不能**跟着 `assets/*` 一起 immutable。
///
/// 这是发版安全的关键：`index.html` 是"指向当前哈希"的那张表。
/// 它一旦被缓存，浏览器会拿旧 HTML 去要已经删掉的旧哈希 JS ⇒ 白屏。
#[tokio::test]
async fn entry_html_is_never_immutable() {
    if !mcloud_server::static_files::has_built_assets() {
        return;
    }
    let (handle, dir) = test_app("static-html");
    let app = authed_app(handle);

    for uri in ["/", "/some/deep/route"] {
        let (status, headers, body) = send_full(
            app.clone(),
            Request::builder()
                .uri(uri)
                .header("accept-encoding", "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{uri} 应回落成前端");
        // ⚠ 别断言 `body.contains("<html>")`：这一发带了 `accept-encoding: gzip`，
        // 拿到的是**压缩后**的字节（乱码），不是 HTML 原文。
        // 用 `content-type` 判断"回落成了前端"才稳。
        let ct = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            ct.starts_with("text/html"),
            "{uri} 应回落成 HTML，实际 content-type={ct}"
        );
        assert_eq!(
            headers.get("content-encoding").map(|v| v.to_str().unwrap()),
            Some("gzip"),
            "{uri} 这么大一张 HTML 也该压"
        );
        assert!(!body.is_empty());

        let cc = headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            cc, "no-cache",
            "{uri} 的 cache-control 必须是 no-cache（否则发版后白屏），实际：{cc}"
        );
        assert!(
            !cc.contains("immutable"),
            "{uri} 不该带 immutable —— 它会引向已删除的旧哈希资源"
        );
    }

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

/// SSE 事件流**不能被压缩**，也不能被攒批。
///
/// # 为什么这条值得单独锁
///
/// `CompressionLayer` 是全局挂的，而 `tower-http` 的默认判据会把
/// SSE 排除掉（`content-type: text/event-stream` 在排除表里）。
/// 也就是说这条**今天是对的，但靠的是上游库的默认行为** ——
/// 哪天有人为了"压得更狠"换掉 predicate，就会把它悄悄压掉。
///
/// 而它坏掉的后果不是报错，是**"实时"没了**：压缩层为了压好一批攒着写，
/// 用户在任务页看着日志一动不动，过一会儿一次性全冒出来 ——
/// 那种"卡住了还是在跑"的不确定，正是这个页面最不该有的。
///
/// # ⚠ 两个坑（都是实测踩出来的）
///
/// 1. **必须用一个真的存在的 run**。拿不存在的 id 打这条路由回的是
///    **404 JSON**，而那个 JSON **会被正常压缩**（带 `content-encoding: gzip`）
///    —— 拿它当判据会得到"SSE 被压了"的**假警报**。所以下面先造一条 run。
/// 2. **必须断言响应真的是 `text/event-stream`**。少了这一步，
///    测试会在"根本没订上"的情况下绿 —— 那是摆设，不是护栏。
///
/// 这里**不真跑任务**（那会打真实上游，慢且看运气）：SSE 只要求
/// `store.run(id)` 存在，所以直接落一条记录即可。
#[tokio::test]
async fn sse_streams_are_never_compressed() {
    let (handle, dir) = test_app("sse-no-gzip");

    // 造一条**存在**的 run（不真跑任务，见上面第 2 点）。
    handle
        .store
        .upsert_run(&mcloud_core::RunRecord {
            run_id: mcloud_core::RunId::new(1),
            task: "sign".into(),
            account: "13800000000".into(),
            outcome: mcloud_core::TaskOutcome::skipped("测试"),
            duration_ms: 1,
            started_at_ms: 1,
            status: mcloud_core::RunStatus::Done,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        })
        .await
        .unwrap();

    // ⚠ 用 `send_headers_only`：SSE 是无限流，`send_full` 会永久阻塞。
    // 而"有没有被压"只看响应头。
    let (status, headers) = send_headers_only(
        authed_app(Arc::clone(&handle)),
        Request::builder()
            .uri("/api/runs/1/events")
            // 客户端**主动要** gzip —— 正因如此才需要断言服务端不给。
            .header("accept-encoding", "gzip")
            .body(Body::empty())
            .unwrap(),
    )
    .await;

    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(status, StatusCode::OK, "这条 run 应当存在并可订阅");
    assert!(
        ct.starts_with("text/event-stream"),
        "没订上 SSE（content-type={ct}）—— 那样下面的断言就成了摆设"
    );
    assert!(
        headers.get("content-encoding").is_none(),
        "SSE 被压缩了 —— 事件流会攒批到达，实时日志不再实时"
    );

    // ⚠ 第 3 个坑：**不能只测"订上了"的那一发**。
    // 上面那条 run 是 `RunStatus::Done`，若它已经有终局事件，流会立刻收尾 ——
    // 那断言虽然成立，覆盖的却是"流正好结束"这条偶然路径。
    // 所以再打一发**确定会被跳过订阅**的（run 不存在 ⇒ 404 JSON），
    // 那个 JSON 必须**照常被压缩**：证明这道排除是**只针对 SSE**的，
    // 而不是"碰巧这条路径整体不压缩"。把它压掉才是回归。
    let (status2, headers2) = send_headers_only(
        authed_app(handle),
        Request::builder()
            .uri("/api/runs/424242/events")
            .header("accept-encoding", "gzip")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status2, StatusCode::NOT_FOUND, "不存在的 run 应 404");
    assert_eq!(
        headers2
            .get("content-encoding")
            .map(|v| v.to_str().unwrap()),
        Some("gzip"),
        "非 SSE 的错误响应也必须照常压缩 —— 否则说明压缩层被整体关掉了",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 账号导入与编辑 ──────────────────────────────────────────────────────
//
// 这两条路是 WebUI「粘贴凭据添加账号」与「编辑账号」的全部后端：
// `POST /api/accounts`（建账号 / 换凭据）与 `PATCH /api/accounts/{id}`
// （改昵称 / 设备号 / 凭据）。重点钉三件容易悄悄错的事：
//
// 1. **已存在的账号只换凭据** —— 昵称、每账号设置、设备号不能被顺手抹掉；
// 2. **先校验完再落笔** —— 半套更新（昵称改了、凭据却是坏的）不该存在；
// 3. **凭据从不回显** —— 响应里出现 auth/token 就是泄了一次凭据。

/// 拼一条**真能解析**的凭据：用上游自己的拼装函数，不手搓 base64
/// （手搓的"看起来对"的串，恰恰测不出 `parse_auth` 的真实行为）。
fn auth_string(prefix: &str, phone: &str, expire_ms: i64) -> String {
    mcloud_upstream::auth::format_auth(prefix, phone, &format!("a|b|c|{expire_ms}|e"))
        .expect("拼 auth 串")
}

fn patch_json(uri: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .expect("请求应可构造")
}

/// `PUT` + JSON + cookie。账户安全那两条都走它。
fn put_json_with_cookie(uri: &str, cookie: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .expect("请求应可构造")
}

fn json_body(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("响应不是 JSON：{e}；原文：{text}"))
}

/// 造一条**主键与路由算法一致**的账号（手机号哈希）。
///
/// ⚠ 不能用上面那个 `account()`：它的 id 是老式「id = 手机号」，
/// 而 `POST/PATCH` 算出来的是手机号哈希 —— 两者在库里是**两个不同的账号**，
/// 预置成前者会让用例去改一个不存在的 id（这一版就这么错过一次）。
fn seed_for(phone: &str) -> Account {
    let mut a = account(phone);
    a.id = mcloud_core::AccountId::new(mcloud_upstream::account_id_for(phone));
    a
}

/// 把一条账号直接塞进库（带归属），给"已存在"那几条用例当前置条件。
fn seed_account(handle: &Arc<AppHandle>, mut account: Account) {
    account.owner = Some(mcloud_core::UserId::new("test-user"));
    let store = Arc::clone(&handle.store);
    futures::executor::block_on(store.save_account(&account)).expect("落库");
}

#[tokio::test]
async fn import_credential_creates_an_account() {
    let (handle, dir) = test_app("acct-import-create");
    let expire = 1_893_456_000_000i64;
    let phone = "19900000001";
    let body = serde_json::json!({
        "auth": auth_string("pc", phone, expire),
        "platform": "macos",
        // 前后空白应当被归一化掉，而不是原样存进侧栏
        "nickname": "  主力号  ",
    });

    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/accounts", &body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际：{text}");
    let v = json_body(&text);
    assert_eq!(v["created"], true, "第一次导入应当是新建");
    assert_eq!(v["account"]["nickname"], "主力号");
    assert_eq!(v["account"]["pc_platform"], "macos");
    assert_eq!(v["account"]["expire"], expire);
    assert_eq!(
        v["account"]["id"],
        mcloud_upstream::account_id_for(phone),
        "主键是手机号哈希"
    );
    assert_eq!(v["account"]["label"], "19****01", "界面用 label，不是哈希");
    assert!(
        !text.contains("Basic ") && !text.contains("a|b|c|"),
        "响应里不能出现凭据：{text}"
    );

    let store = Arc::clone(&handle.store);
    let id = mcloud_core::AccountId::new(mcloud_upstream::account_id_for(phone));
    let saved = futures::executor::block_on(store.account(&id))
        .expect("读库")
        .expect("账号应已落库");
    assert_eq!(saved.auth.expire, expire);
    assert_eq!(
        saved.owner.as_ref().map(|o| o.to_string()),
        Some("test-user".to_string()),
        "归属要补写成当前会话用户（否则多用户实例里谁都看不到）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn import_credential_on_existing_account_keeps_everything_but_the_credential() {
    let (handle, dir) = test_app("acct-import-existing");
    let phone = "19900000002";
    let id_text = mcloud_upstream::account_id_for(phone);

    // 前置：这个号已在库里，且用户配过昵称 / 设置 / 设备号
    let mut existing = seed_for(phone);
    existing.nickname = Some("老号".into());
    existing.device_id = DeviceId("keep-me-ENDIN".into());
    existing.settings.backup_wait_secs = 42;
    seed_account(&handle, existing);

    let new_expire = 1_900_000_000_000i64;
    let body = serde_json::json!({
        "auth": auth_string("pc", phone, new_expire),
        "platform": "windows",
        // ⚠ 昵称**不该**被这次导入改掉：这是"换凭据"，不是"改账号"
        "nickname": "不该生效",
    });
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/accounts", &body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际：{text}");
    let v = json_body(&text);
    assert_eq!(v["created"], false, "同一手机号第二次导入是「换凭据」");

    let store = Arc::clone(&handle.store);
    let saved = futures::executor::block_on(store.account(&mcloud_core::AccountId::new(id_text)))
        .expect("读库")
        .expect("账号还在");
    assert_eq!(saved.auth.expire, new_expire, "凭据要换掉");
    assert_eq!(
        saved.nickname.as_deref(),
        Some("老号"),
        "昵称不能被导入抹掉"
    );
    assert_eq!(
        saved.settings.backup_wait_secs, 42,
        "每账号设置不能被导入抹掉"
    );
    assert_eq!(
        saved.device_id.as_str(),
        "keep-me-ENDIN",
        "设备号是登录时定的，换凭据不该动它"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn import_credential_rejects_unusable_input() {
    let (handle, dir) = test_app("acct-import-bad");

    // 解不出来的串 ⇒ 400，且**什么都别落库**
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        post_json(
            "/api/accounts",
            &serde_json::json!({ "auth": "这不是 base64" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际：{text}");

    // PC 凭据不给客户端形态 ⇒ 400（形态决定后续每一次请求的 UA，猜不得）
    let phone = "19900000003";
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        post_json(
            "/api/accounts",
            &serde_json::json!({ "auth": auth_string("pc", phone, 1_900_000_000_000i64) }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际：{text}");

    // App 凭据不需要形态 ⇒ 通过，且 `pc_platform` 必须是 null
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        post_json(
            "/api/accounts",
            &serde_json::json!({ "auth": auth_string("mobile", phone, 1_900_000_000_000i64) }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际：{text}");
    assert!(json_body(&text)["account"]["pc_platform"].is_null());

    let store = Arc::clone(&handle.store);
    let all = futures::executor::block_on(store.accounts()).expect("读库");
    assert_eq!(all.len(), 1, "两次失败的导入不该留下账号：{all:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn update_account_changes_nickname_and_device() {
    let (handle, dir) = test_app("acct-update");
    let phone = "19900000004";
    let id_text = mcloud_upstream::account_id_for(phone);
    seed_account(&handle, seed_for(phone));

    let uri = format!("/api/accounts/{id_text}");
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        patch_json(
            &uri,
            &serde_json::json!({ "nickname": "副号", "deviceId": "  new-dev-ENDIN  " }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际：{text}");
    assert_eq!(json_body(&text)["nickname"], "副号");

    let store = Arc::clone(&handle.store);
    let saved = futures::executor::block_on(store.account(&mcloud_core::AccountId::new(&id_text)))
        .expect("读库")
        .expect("账号还在");
    assert_eq!(saved.nickname.as_deref(), Some("副号"));
    assert_eq!(saved.device_id.as_str(), "new-dev-ENDIN", "设备号要去空白");

    // 空串 = 清空昵称（回到掩码手机号），而不是"存一个空名字"
    let (status, _) = send(
        authed_app(Arc::clone(&handle)),
        patch_json(&uri, &serde_json::json!({ "nickname": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let saved = futures::executor::block_on(store.account(&mcloud_core::AccountId::new(&id_text)))
        .expect("读库")
        .expect("账号还在");
    assert_eq!(saved.nickname, None, "空串应当清成 None");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn update_account_validates_before_writing_anything() {
    let (handle, dir) = test_app("acct-update-atomic");
    let phone = "19900000005";
    let id_text = mcloud_upstream::account_id_for(phone);
    let mut seeded = seed_for(phone);
    seeded.nickname = Some("原名".into());
    let old_expire = seeded.auth.expire;
    seed_account(&handle, seeded);

    // 昵称合法、凭据非法 ⇒ 400，且**昵称也不能变**（半套更新不该存在）
    let uri = format!("/api/accounts/{id_text}");
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        patch_json(
            &uri,
            &serde_json::json!({ "nickname": "改过了", "auth": "坏的凭据", "platform": "windows" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际：{text}");

    let store = Arc::clone(&handle.store);
    let saved = futures::executor::block_on(store.account(&mcloud_core::AccountId::new(&id_text)))
        .expect("读库")
        .expect("账号还在");
    assert_eq!(
        saved.nickname.as_deref(),
        Some("原名"),
        "校验没过就不该落笔"
    );
    assert_eq!(saved.auth.expire, old_expire);

    // 设备号里带空白 ⇒ 400（那种值会让每一次请求都带一个畸形头）
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        patch_json(&uri, &serde_json::json!({ "deviceId": "has space" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际：{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn update_account_swaps_credential_and_follows_its_platform() {
    let (handle, dir) = test_app("acct-update-auth");
    let phone = "19900000006";
    let id_text = mcloud_upstream::account_id_for(phone);
    // 前置：一个 PC 账号（macos）
    let mut seeded = seed_for(phone);
    seeded.pc_platform = Some(mcloud_core::PcPlatform::MacOs);
    seed_account(&handle, seeded);

    // 换成一条 **App** 凭据 ⇒ 形态必须跟着变成 null：
    // 留着一个 windows/macos 会让后续请求按 PC 的身份发头，而凭据是 App 的。
    let new_expire = 1_950_000_000_000i64;
    let uri = format!("/api/accounts/{id_text}");
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        patch_json(
            &uri,
            &serde_json::json!({ "auth": auth_string("mobile", phone, new_expire) }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "实际：{text}");
    assert!(
        !text.contains("Basic ") && !text.contains("a|b|c|"),
        "响应里不能出现凭据：{text}"
    );

    let store = Arc::clone(&handle.store);
    let saved = futures::executor::block_on(store.account(&mcloud_core::AccountId::new(&id_text)))
        .expect("读库")
        .expect("账号还在");
    assert_eq!(saved.auth.expire, new_expire, "凭据要换掉");
    assert_eq!(saved.auth.platform, "mobile");
    assert_eq!(saved.pc_platform, None, "换成 App 凭据后形态必须归 None");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn account_mutations_are_scoped_to_the_owner() {
    let (handle, dir) = test_app("acct-owner");
    // 账号存在，但**不属于**测试用户 ⇒ 改它必须 404（"别人的"与"不存在"同一个回包）
    let mut foreign = seed_for("19900000007");
    foreign.owner = Some(mcloud_core::UserId::new("someone-else"));
    let store = Arc::clone(&handle.store);
    futures::executor::block_on(store.save_account(&foreign)).expect("落库");

    let uri = format!("/api/accounts/{}", foreign.id.as_str());
    let (status, text) = send(
        authed_app(Arc::clone(&handle)),
        patch_json(&uri, &serde_json::json!({ "nickname": "抢过来" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "实际：{text}");
    let saved = futures::executor::block_on(store.account(&foreign.id))
        .expect("读库")
        .expect("账号还在");
    assert_eq!(saved.nickname, None, "别人的账号一个字段都不该被改");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 口令：前端 hash 一次 + 后端再 hash 一次（用户口径 2026-10-01）────────

/// ⭐ 建管理员时收到的就是**前端那次 SHA-256** ⇒ 库里是 `sha256$argon2(摘要)`。
///
/// 同时钉住兼容口子：**缓存里的旧 bundle（还在发明文）必须仍然登得上** ——
/// 否则更新服务端的那一刻，所有人都被自己的登录闸关在门外。
#[tokio::test]
async fn setup_stores_the_client_digest_scheme() {
    let (handle, dir) = test_app("gate-digest-setup");
    let state = ServerState::new(handle.clone());
    let digest = mcloud_crypto::sha256_hex("correct-horse");

    let (s, _, body) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", &digest)),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{body}");

    let users = handle.store.users().await.unwrap();
    let stored = users
        .first()
        .expect("建完应该有一个管理员")
        .password_hash
        .as_str();
    assert!(stored.starts_with("sha256$"), "新档必须带前缀：{stored}");
    assert!(!stored.contains("correct-horse"), "明文不许落库：{stored}");
    assert!(
        mcloud_crypto::verify_password(&digest, mcloud_crypto::strip_digest_scheme(stored)).is_ok(),
        "存下来的必须是对**摘要**做的 argon2"
    );

    // 前端发摘要 ⇒ 登得上
    let (s, _, body) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/session", &setup_json("admin", &digest)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");

    // 缓存里的旧前端发明文 ⇒ 也得登得上（服务端现场补一次摘要）
    let (s, _, body) = send_full(
        mcloud_server::app(state),
        post_json("/api/session", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "旧 bundle 发明文不能因此登不上：{body}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 老库（argon2 直接吃明文）在**登录成功那一刻**升级到新档；
/// 升级后用摘要登录也通 —— 升完不能把人锁在外面。
#[tokio::test]
async fn legacy_password_record_is_upgraded_on_login() {
    let (handle, dir) = test_app("gate-digest-upgrade");
    let plain = "correct-horse";
    // 老档的样子：**没有** `sha256$` 前缀的 argon2 串
    Arc::clone(&handle.store)
        .save_user(&User {
            id: UserId::new("u-legacy"),
            name: "admin".into(),
            password_hash: PasswordHash(mcloud_crypto::hash_password(plain).unwrap()),
            created_at_ms: 0,
        })
        .await
        .unwrap();

    let state = ServerState::new(handle.clone());

    // 前端**先问档位**：老库必须自述 `plain`，否则前端发摘要 ⇒ 谁也验不过
    let (_, body) = send(mcloud_server::app(state.clone()), get("/api/setup")).await;
    assert!(
        body.contains("\"passwordScheme\":\"plain\""),
        "老库要自述明文档，否则前端会发摘要：{body}"
    );

    // 明文登录 ⇒ 成功 + 当场升级
    let (s, _, body) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/session", &setup_json("admin", plain)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");

    let stored = handle.store.users().await.unwrap()[0]
        .password_hash
        .as_str()
        .to_string();
    assert!(
        stored.starts_with("sha256$"),
        "登录成功那一刻就该升级：{stored}"
    );
    assert!(
        !stored.contains(plain),
        "升级后库里仍是 argon2 串，不该出现明文：{stored}"
    );

    // 档位跟着翻成 sha256；摘要登录也通
    let (_, body) = send(mcloud_server::app(state.clone()), get("/api/setup")).await;
    assert!(body.contains("\"passwordScheme\":\"sha256\""), "{body}");

    let (s, _, body) = send_full(
        mcloud_server::app(state),
        post_json(
            "/api/session",
            &setup_json("admin", &mcloud_crypto::sha256_hex(plain)),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "升级后摘要必须能登：{body}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `/api/capabilities` 的**字段名**必须和前端读的一致。
///
/// 这条测试存在的唯一理由：装置（fixture）里的 JSON 是手写的，字段名写对与否
/// 只取决于写的人 —— 2026-10-01 真实发生：Rust 侧漏了 `rename_all = "camelCase"`，
/// 发出的是 `group_label`，而装置写的是 `groupLabel` ⇒ **门禁全绿、线上分组名全丢**。
/// 断言 key 才能挡住这一类。
#[tokio::test]
async fn capabilities_expose_group_and_param_specs_with_camel_case_keys() {
    let (handle, dir) = test_app("gate-caps-fields");
    let (status, body) = send(authed_app(Arc::clone(&handle)), get("/api/capabilities")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for key in ["groupLabel", "group_label"] {
        let present = body.contains(&format!("\"{key}\""));
        if key == "groupLabel" {
            assert!(
                present,
                "少了下划线转驼峰：前端读 groupLabel，发了 group_label 就会回落成兜底值"
            );
        } else {
            assert!(!present, "不该把 snake_case 的 group_label 发出去：{body}");
        }
    }
    // 参数说明也要在（前端照它画任务配置页的表单）
    assert!(
        body.contains("\"specs\"") || body.contains("\"params\""),
        "{body}"
    );
    assert!(
        body.contains("\"kind\"") || !body.contains("\"name\":\"sign\""),
        "有参数的任务必须连类型一起给：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 任务参数配置（2026-10-01：参数从任务页挪到单独的配置页）──────────

fn put_json(uri: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .expect("请求应可构造")
}

/// ⭐ 任务参数：默认值补齐、类型/范围/未知键都拒、存了能读回、任务不存在 404、
/// 没登录读不到。
#[tokio::test]
async fn task_config_round_trip_and_validation() {
    let (handle, dir) = test_app("gate-task-config");
    let _ = authed_app(Arc::clone(&handle));

    // 没配过 ⇒ 200 + 空对象（**不是** 404：那会让界面显示一个空表单），
    // 且带上参数说明与默认值 —— 前端不必自己算默认
    let (s, body) = send(
        authed_app(Arc::clone(&handle)),
        get("/api/tasks/live-room/config"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v = json_body(&body);
    assert_eq!(v["name"], "live-room");
    assert_eq!(
        v["params"]["skipKouling"], false,
        "缺省键要补成默认值：{body}"
    );
    assert!(
        v["specs"]
            .as_array()
            .expect("specs 是数组")
            .iter()
            .any(|x| x["name"] == "skipKouling" && x["kind"] == "bool"),
        "参数说明要带上类型（前端照它画控件）：{body}"
    );

    // 存
    let (s, body) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/tasks/live-room/config",
            &serde_json::json!({ "params": { "skipKouling": true } }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(json_body(&body)["params"]["skipKouling"], true);

    // 读回（落盘了，不是只回了显）
    let (_, body) = send(
        authed_app(Arc::clone(&handle)),
        get("/api/tasks/live-room/config"),
    )
    .await;
    assert_eq!(
        json_body(&body)["params"]["skipKouling"],
        true,
        "存了必须读得回"
    );

    // 类型错 / 范围错 / 不认识的键 —— 一律 400，且话要能照着改
    let cases = [
        (
            "/api/tasks/live-room/config",
            serde_json::json!({ "params": { "skipKouling": "yes" } }),
            "true / false",
        ),
        (
            "/api/tasks/live-room/config",
            serde_json::json!({ "params": { "listenSeconds": 9999 } }),
            "超出范围",
        ),
        (
            "/api/tasks/live-room/config",
            serde_json::json!({ "params": { "nope": 1 } }),
            "没有参数",
        ),
    ];
    for (uri, body, want) in cases {
        let (s, got) = send(authed_app(Arc::clone(&handle)), put_json(uri, &body)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body} 应被拒：{got}");
        assert!(got.contains(want), "错误里要出现「{want}」：{got}");
    }

    // 不存在的任务 ⇒ 404
    let (s, _) = send(
        authed_app(Arc::clone(&handle)),
        get("/api/tasks/nope/config"),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // 没登录读不到（构建信息都不给，参数更不给）
    let (s, _) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        get("/api/tasks/live-room/config"),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 账户安全：策略 / 改口令（2026-10-01 补的后端）──────────────────────

/// ⭐ 策略可读可写、非法值 400 且**不覆盖**已存的值；没登录读不到。
#[tokio::test]
async fn security_policy_round_trip() {
    let (handle, dir) = test_app("gate-security-policy");
    let state = ServerState::new(Arc::clone(&handle));

    let (_, headers, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json(
            "/api/setup",
            &setup_json("admin", &mcloud_crypto::sha256_hex("correct-horse")),
        ),
    )
    .await;
    let cookie = cookie_of(&headers).expect("初始化后应有 cookie");

    // 已初始化但**没带 cookie** ⇒ 401。这一条是"策略本身也在闸后面"的证明：
    // 能匿名改口令下限，等于把闸的门槛交给任何路过的人。
    // （未初始化时是 503 而不是 401 —— 那是仓库既有语义，另有一组测试锁着。）
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        get("/api/admin/security"),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (s, body) = send(
        mcloud_server::app(state.clone()),
        get_with_cookie("/api/admin/security", &cookie),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body.contains("\"minPasswordLen\":8"), "默认下限 8：{body}");
    assert!(
        body.contains("\"sessionTtlHours\":168"),
        "默认会话必须是 7 天 = 168 小时：{body}"
    );
    assert!(body.contains("\"passwordScheme\":\"sha256\""), "{body}");

    let (s, body) = send(
        mcloud_server::app(state.clone()),
        put_json_with_cookie(
            "/api/admin/security",
            &cookie,
            &serde_json::json!({ "minPasswordLen": 12, "sessionTtlHours": 24 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body.contains("\"minPasswordLen\":12"), "{body}");

    // 真的落盘了（不是只回显了一下）
    let saved = futures::executor::block_on(Arc::clone(&handle.store).settings()).expect("读设置");
    assert_eq!(saved.min_password_len, 12);
    assert_eq!(saved.session_ttl_hours, 24);

    // 非法值 400，且**已存的值不动** —— 半套写入是最难查的那类状态
    for bad in [
        serde_json::json!({ "minPasswordLen": 3, "sessionTtlHours": 24 }),
        serde_json::json!({ "minPasswordLen": 12, "sessionTtlHours": 24 * 91 }),
    ] {
        let (s, body) = send(
            mcloud_server::app(state.clone()),
            put_json_with_cookie("/api/admin/security", &cookie, &bad),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad} 应被拒：{body}");
    }
    let after = futures::executor::block_on(Arc::clone(&handle.store).settings()).expect("读设置");
    assert_eq!(after.min_password_len, 12, "非法值不该写进去");
    assert_eq!(after.session_ttl_hours, 24);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 改口令：换哈希、**旧会话全失效**、当前浏览器拿到新令牌、新旧口令各自能/不能登。
///
/// 当前口令**必填**且要照验（用户口径 2026-10-01：「你口令更改怎么取消了旧口令」
/// —— 曾一度做成可选，被明确指出后改回；缺字段直接 422，不留"省略即跳过"这条路）。
#[tokio::test]
async fn change_password_kills_other_sessions_and_keeps_this_one() {
    let (handle, dir) = test_app("gate-change-pw");
    let state = ServerState::new(Arc::clone(&handle));
    let old = "correct-horse";
    let new = "another-horse";
    let third = "third-horse";

    // 两个浏览器 = 两次登录
    let (_, h1, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", old)),
    )
    .await;
    let cookie1 = cookie_of(&h1).expect("cookie1");
    let (_, h2, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/session", &setup_json("admin", old)),
    )
    .await;
    let cookie2 = cookie_of(&h2).expect("cookie2");

    fn change(cookie: &str, current: &str, next: &str) -> Request<Body> {
        put_json_with_cookie(
            "/api/admin/password",
            cookie,
            &serde_json::json!({
                "currentPassword": mcloud_crypto::sha256_hex(current),
                "newPassword": mcloud_crypto::sha256_hex(next),
            }),
        )
    }

    // 当前口令不对 ⇒ 401
    let (s, body) = send(
        mcloud_server::app(state.clone()),
        change(&cookie1, "nope", new),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{body}");

    // 干脆不传当前口令 ⇒ 422（必填字段缺失）
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        put_json_with_cookie(
            "/api/admin/password",
            &cookie1,
            &serde_json::json!({ "newPassword": mcloud_crypto::sha256_hex(new) }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "缺少当前口令必须拒");

    // 新口令太短（明文这条路量得出长度）⇒ 400
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        put_json_with_cookie(
            "/api/admin/password",
            &cookie1,
            &serde_json::json!({
                "currentPassword": mcloud_crypto::sha256_hex(old),
                "newPassword": "short",
            }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "低于下限必须拒");

    // 新 = 旧 ⇒ 400（否则会白踢一堆会话，而口令其实没变）
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        change(&cookie1, old, old),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // 真改
    let (s, headers, body) = send_full(
        mcloud_server::app(state.clone()),
        change(&cookie1, old, new),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        body.contains("\"sessionsEnded\":2"),
        "两个旧会话都要被踢（含发起这次请求的那张）：{body}"
    );
    let fresh = cookie_of(&headers).expect("改完必须给当前浏览器发新令牌");

    // 当前浏览器还在（用新令牌）—— 把操作者一起踢出去，用户会以为"改失败了"
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        get_with_cookie("/api/accounts", &fresh),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "操作者不该被自己踢出去");

    // 另一个浏览器的旧令牌必须失效
    let (s, _) = send(
        mcloud_server::app(state.clone()),
        get_with_cookie("/api/accounts", &cookie2),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "改口令后旧会话必须失效");

    // 库里那片是对**新摘要**做的 argon2、且带新档前缀
    let stored = handle.store.users().await.unwrap()[0]
        .password_hash
        .as_str()
        .to_string();
    assert!(stored.starts_with("sha256$"), "{stored}");

    // 新口令能登、旧口令不能
    let (s, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json(
            "/api/session",
            &setup_json("admin", &mcloud_crypto::sha256_hex(new)),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "新口令必须能登");
    let (s, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json(
            "/api/session",
            &setup_json("admin", &mcloud_crypto::sha256_hex(old)),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "旧口令必须不能登");

    // 再改一次：**带对**当前口令 ⇒ 能过（证明这道检查不是"一律拒绝"）
    let (_, h3, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json(
            "/api/session",
            &setup_json("admin", &mcloud_crypto::sha256_hex(new)),
        ),
    )
    .await;
    let cookie3 = cookie_of(&h3).expect("cookie3");
    let (s, body) = send(
        mcloud_server::app(state.clone()),
        change(&cookie3, new, third),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "带对当前口令必须能改：{body}");
    let (s, _, _) = send_full(
        mcloud_server::app(state),
        post_json(
            "/api/session",
            &setup_json("admin", &mcloud_crypto::sha256_hex(third)),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "第三次设的口令必须能登");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 「关于」：版本来自编译期常量、构建信息来自构建机（拿不到就是空串，但字段必须在），
/// 没登录读不到。
#[tokio::test]
async fn about_reports_build_facts() {
    let (handle, dir) = test_app("gate-about");
    let _ = authed_app(Arc::clone(&handle));

    let (s, body) = send(authed_app(Arc::clone(&handle)), get("/api/system/about")).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v = json_body(&body);
    assert_eq!(v["name"], "mcloud");
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        v["buildTime"].is_string(),
        "构建时间必须是字符串（拿不到就是空串，但不能没有）：{body}"
    );
    assert!(v["changes"].is_array(), "更新说明必须是数组：{body}");
    assert!(
        v["profile"] == "release" || v["profile"] == "debug",
        "构建类型只能是这两种：{body}"
    );
    // 提交号：测试机上有 git 就是 7 位上下，没有就是空串 —— 两种都接受，但不能是 null
    assert!(v["commit"].is_string(), "{body}");
    assert!(v["dirty"].is_boolean(), "{body}");

    let (s, _) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        get("/api/system/about"),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "构建信息不该匿名可见");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 系统：存储信息 / 重启 ──────────────────────────────────────────────

/// ⭐ 存储信息报的是**真实路径**（不是猜的、也不是相对路径），并列出数据文件。
#[tokio::test]
async fn storage_reports_real_paths() {
    let (handle, dir) = test_app("gate-storage");
    let (status, body) = send(authed_app(Arc::clone(&handle)), get("/api/system/storage")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v = json_body(&body);

    // ⚠ 不能直接比字符串：接口报的是 **canonicalize** 之后的路由，
    // 而 macOS 上临时目录 `/var/folders/…` 会规范化成 `/private/var/folders/…`。
    // 比"两边规范化之后相等"才是这条断言真正想说的东西。
    let reported = std::path::PathBuf::from(v["dataDir"].as_str().unwrap());
    assert!(
        reported.is_absolute(),
        "要报绝对路径（相对路径等于没给）：{reported:?}"
    );
    assert_eq!(
        std::fs::canonicalize(&reported).unwrap(),
        std::fs::canonicalize(&dir).unwrap(),
        "必须是这个实例真实的数据目录"
    );
    assert!(v["logDir"].as_str().unwrap().ends_with("/logs"), "{body}");
    assert!(
        !v["binary"].as_str().unwrap().is_empty(),
        "二进制路径要报出来"
    );
    let files = v["files"].as_array().expect("files 是数组");
    assert!(
        files.iter().any(|f| f["name"] == "users.json"),
        "建过管理员就该有 users.json：{body}"
    );
    assert!(v["dataBytes"].as_u64().unwrap() > 0, "占用不该恒为 0");

    // ⭐ 2026-10-09：日志用量与三个**用户可设**的上限（用户口径：「让用户设置最大大小」✓）
    assert_eq!(v["logUsage"]["files"].as_u64().unwrap(), 0, "还没写过日志");
    assert_eq!(
        v["logUsage"]["maxBytes"].as_u64().unwrap(),
        mcloud_core::DEFAULT_LOG_TOTAL_MB * 1024 * 1024,
        "默认上限 = 64 MB：{body}"
    );
    assert_eq!(
        v["limits"]["historyLimit"].as_u64().unwrap(),
        mcloud_core::DEFAULT_HISTORY_LIMIT as u64,
        "默认 = 老的 HISTORY_LIMIT（行为不变 ✓）：{body}"
    );
    assert_eq!(
        v["limits"]["logLimitPerRun"].as_u64().unwrap(),
        mcloud_core::LOG_LIMIT_PER_RUN as u64,
        "默认 = 老的 LOG_LIMIT_PER_RUN：{body}"
    );
    assert_eq!(
        v["limits"]["logTotalMb"].as_u64().unwrap(),
        mcloud_core::DEFAULT_LOG_TOTAL_MB,
        "默认 64 MB：{body}"
    );
    // ⭐ 服务端日志是**第二个目录**（`server-logs/`）：运行日志按 run 归档、
    // 服务端日志按天追加 ⇒ 淘汰单位不同，不能混在一个目录里 ✓
    assert!(
        v["serverLogDir"]
            .as_str()
            .unwrap()
            .ends_with("/server-logs"),
        "{body}"
    );
    assert_eq!(
        v["serverLogUsage"]["files"].as_u64().unwrap(),
        0,
        "还没写过服务端日志：{body}"
    );
    assert_eq!(
        v["serverLogUsage"]["maxBytes"].as_u64().unwrap(),
        mcloud_core::DEFAULT_SERVER_LOG_TOTAL_MB * 1024 * 1024,
        "{body}"
    );
    assert_eq!(
        v["limits"]["serverLogMemLines"].as_u64().unwrap(),
        2000,
        "内存窗口默认 = 老的 RING_CAPACITY（行为不变 ✓）：{body}"
    );
    assert_eq!(
        v["limits"]["serverLogTotalMb"].as_u64().unwrap(),
        mcloud_core::DEFAULT_SERVER_LOG_TOTAL_MB,
        "{body}"
    );

    // 没登录 ⇒ 401（存储路径属于"这台机器怎么部署的"，不该匿名可见）
    let (s, _) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        get("/api/system/storage"),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ 容量上限：能改、立刻生效、**只有管理员**；而改安全策略**不许**把它冲回默认 ✓。
///
/// 最后那一条是这次最值钱的一条：`AppSettings` 是一份**整体**（口令下限、会话时长、
/// 容量三格都在里面 ✓）⇒ 两个 PUT 各写一半的话，`PUT /api/admin/security`
/// 会把容量上限重置成默认值 ✗ —— 那种 bug 只有"先改容量、再改策略"才现形 ✓。
#[tokio::test]
async fn storage_limits_round_trip_and_are_admin_only() {
    let (handle, dir) = test_app("storage-limits");
    let (app, admin, other) = two_user_app(Arc::clone(&handle));

    let body_ok = serde_json::json!({
        "historyLimit": 50,
        "logLimitPerRun": 500,
        "logTotalMb": 2,
        "serverLogMemLines": 400,
        "serverLogTotalMb": 3,
    });

    // 非管理员 ⇒ 403（实例级设置，与 `restart` 同一条口径 ✓）
    let (st, body) = send(
        app.clone(),
        put_json_with_cookie("/api/admin/storage", &other, &body_ok),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "普通用户不该能改实例容量：{body}"
    );

    // 管理员 ⇒ 200，且回的是**改完的即时状态** ✓
    let (st, body) = send(
        app.clone(),
        put_json_with_cookie("/api/admin/storage", &admin, &body_ok),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v = json_body(&body);
    assert_eq!(v["limits"]["historyLimit"], 50, "{body}");
    assert_eq!(v["limits"]["logLimitPerRun"], 500, "{body}");
    assert_eq!(v["limits"]["logTotalMb"], 2, "{body}");
    assert_eq!(v["limits"]["serverLogMemLines"], 400, "{body}");
    assert_eq!(v["limits"]["serverLogTotalMb"], 3, "{body}");
    assert_eq!(
        v["logUsage"]["maxBytes"].as_u64().unwrap(),
        2 * 1024 * 1024,
        "上限换了，用量里那份也要跟着换：{body}"
    );
    assert_eq!(
        v["serverLogUsage"]["maxBytes"].as_u64().unwrap(),
        3 * 1024 * 1024,
        "服务端日志那格同理：{body}"
    );

    // ⭐⭐ **只给一格**也能改，其余原样（这条钉的是"老页面只发三格"那种兼容：
    // 加了一个设置项不能让另一个设置项突然存不了 ✗）
    let (st, body) = send(
        app.clone(),
        put_json_with_cookie(
            "/api/admin/storage",
            &admin,
            &serde_json::json!({ "serverLogMemLines": 600 }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let s = handle.store.settings().await.unwrap();
    assert_eq!(s.server_log_mem_lines, 600, "给的那格要改到");
    assert_eq!(s.history_limit, 50, "没给的那几格必须原样 ✗");
    assert_eq!(s.server_log_total_mb, 3);
    assert_eq!(s.log_total_mb, 2);

    // ⚠ 一个都不给 ⇒ 400（静默成功会让人以为改了 ✓）
    let (st, body) = send(
        app.clone(),
        put_json_with_cookie("/api/admin/storage", &admin, &serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("至少要给一格"), "{body}");

    // ⚠⚠ 再改**安全策略**：容量那三格必须原样保留 ✓
    let (st, body) = send(
        app.clone(),
        put_json_with_cookie(
            "/api/admin/security",
            &admin,
            &serde_json::json!({ "minPasswordLen": 10, "sessionTtlHours": 24 }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let s = handle.store.settings().await.unwrap();
    assert_eq!(s.min_password_len, 10, "口令下限该改到了");
    assert_eq!(s.history_limit, 50, "改安全策略**不该**把容量上限冲回默认");
    assert_eq!(s.log_limit_per_run, 500);
    assert_eq!(s.log_total_mb, 2);
    assert_eq!(s.server_log_mem_lines, 600, "服务端日志那两格同理");
    assert_eq!(s.server_log_total_mb, 3);

    // 越界 ⇒ 400 + 人话（不静默夹取：用户以为设成了 0 就是"一条都不留"✗）
    for (bad, want) in [
        (
            serde_json::json!({"historyLimit": 1, "logLimitPerRun": 500, "logTotalMb": 2}),
            "历史保留条数",
        ),
        (
            serde_json::json!({"historyLimit": 50, "logLimitPerRun": 1, "logTotalMb": 2}),
            "单次运行日志行数",
        ),
        (
            serde_json::json!({"historyLimit": 50, "logLimitPerRun": 500, "logTotalMb": 0}),
            "日志总量上限",
        ),
    ] {
        let (st, body) = send(
            app.clone(),
            put_json_with_cookie("/api/admin/storage", &admin, &bad),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains(want), "报错该说清是哪一格：{body}");
    }
    // 越界那次不许写进去 ✓
    assert_eq!(handle.store.settings().await.unwrap().log_total_mb, 2);

    // 没登录 ⇒ 401
    let (st, _) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        put_json("/api/admin/storage", &body_ok),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 运行日志**分页**（用户口径 2026-10-09：「大日志做虚拟加载或者分页，
/// 避免一次加载太多」✓）。
///
/// 三条语义各钉一次：不带游标取**最后**一页、`before` **往上**翻、`after` **往下**接。
#[tokio::test]
async fn run_logs_are_paged_by_cursor() {
    let (handle, dir) = test_app("run-logs-page");
    handle
        .store
        .upsert_run(&mcloud_core::RunRecord {
            run_id: mcloud_core::RunId::new(1),
            task: "sign".into(),
            account: "13800000000".into(),
            outcome: mcloud_core::TaskOutcome::success("ok"),
            duration_ms: 1,
            started_at_ms: 1,
            status: mcloud_core::RunStatus::Done,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        })
        .await
        .unwrap();
    let lines: Vec<mcloud_core::LogLine> = (1..=12u64)
        .map(|i| {
            mcloud_core::LogLine::new(
                1,
                mcloud_core::LogLevel::Info,
                format!("第 {i} 行"),
                i as i64,
                mcloud_core::Seq(i),
            )
        })
        .collect();
    handle
        .store
        .append_logs(mcloud_core::RunId::new(1), &lines)
        .await
        .unwrap();
    let app = authed_app(Arc::clone(&handle));

    // 不带游标 ⇒ **最后** 5 行（打开就是最近这些 ✓）
    let (st, body) = send(app.clone(), get("/api/runs/1/logs?limit=5")).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v = json_body(&body);
    let got: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        got,
        vec!["第 8 行", "第 9 行", "第 10 行", "第 11 行", "第 12 行"]
    );

    // `before` ⇒ 往上翻（要更早的 ✓）
    let (_, body) = send(app.clone(), get("/api/runs/1/logs?limit=5&before=8")).await;
    let v = json_body(&body);
    let got: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["message"].as_str().unwrap())
        .collect();
    assert_eq!(
        got,
        vec!["第 3 行", "第 4 行", "第 5 行", "第 6 行", "第 7 行"]
    );

    // 翻到头 ⇒ 给能给的（少于 limit 就是"没有更多了" ✓，前端据此收起按钮 ✓）
    let (_, body) = send(app.clone(), get("/api/runs/1/logs?limit=5&before=3")).await;
    let v = json_body(&body);
    assert_eq!(v.as_array().unwrap().len(), 2, "{body}");

    // `after` ⇒ 往下接（断线续传，既有语义 ✓）
    let (_, body) = send(app.clone(), get("/api/runs/1/logs?limit=3&after=7")).await;
    let v = json_body(&body);
    let got: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["message"].as_str().unwrap())
        .collect();
    assert_eq!(got, vec!["第 8 行", "第 9 行", "第 10 行"]);

    // `limit` 被夹到**设置里那一格**（一页不该比"一次运行能留的全部"还大 ✓）
    handle
        .store
        .save_settings(&{
            let mut s = handle.store.settings().await.unwrap();
            s.log_limit_per_run = mcloud_core::LOG_LIMIT_PER_RUN_RANGE.0; // 200
            s
        })
        .await
        .unwrap();
    let (_, body) = send(app.clone(), get("/api/runs/1/logs?limit=100000")).await;
    let v = json_body(&body);
    assert_eq!(
        v.as_array().unwrap().len(),
        12,
        "总共只有 12 行 ⇒ 夹到上限也只是这 12 行：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 重启：202 受理，并且**真的**唤醒了关停路径（`main.rs` 等的是同一个信号）。
#[tokio::test]
async fn restart_is_accepted_and_signals_shutdown() {
    let (handle, dir) = test_app("gate-restart");

    // ⚠ 先让 authed_app 把用户建出来：**未初始化**时网关回 503（另一组测试锁着），
    // 而这里要证的是"已初始化但没登录" ⇒ 401。
    let _ = authed_app(Arc::clone(&handle));
    let (s, _) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        post_json("/api/system/restart", &serde_json::json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "重启是特权动作，必须登录");

    let (s, body) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/system/restart", &serde_json::json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{body}");

    // 信号是 300ms 后发的（先让响应出去）：等得到才算真发了。
    let woke = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        handle.shutdown.notified(),
    )
    .await;
    assert!(
        woke.is_ok(),
        "重启请求必须真的唤醒关停路径，否则按钮是个摆设"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── SSE 断开必须退订（2026-10-03 review §1.5）────────────────────────────

/// ⭐⭐ 流被丢掉时**必须退订**，否则中枢里一次运行漏一份。
///
/// 改之前的实情：`hub.subscribe` 在生产路径上只有一处调用（这条路由），
/// 而 `hub.unsubscribe` **一个都没有** —— `grep -rn "hub\.unsubscribe" crates`
/// 只剩测试。浏览器关掉 `EventSource`（前端在终局事件后就是这么做的）时，
/// axum 丢掉响应体 ⇒ `unfold` 生成器被 drop ⇒ **循环后面那些语句一行都不执行**
/// ⇒ 订阅者条目永远留着。
///
/// ⚠ 这条用例必须**先断言"订上了"**（`subscriber_count == 1`）：只断言"断开后是 0"
/// 的话，订阅根本没发生也能绿 —— 那是摆设。
#[tokio::test]
async fn sse_disconnect_unsubscribes_from_the_hub() {
    let (handle, dir) = test_app("sse-unsubscribes");
    let run_id = mcloud_core::RunId::new(7);
    handle
        .store
        .upsert_run(&mcloud_core::RunRecord {
            run_id,
            task: "sign".into(),
            account: "13800000000".into(),
            outcome: mcloud_core::TaskOutcome::skipped("测试"),
            duration_ms: 1,
            started_at_ms: 1,
            status: mcloud_core::RunStatus::Done,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        })
        .await
        .unwrap();
    // ⚠⚠ 2026-10-06 审计 `SEC-17` 之后，"造一条**只在存储层**的 run"已经不够了：
    // 新口径下"存储层有记录、内存里没有"= **已结束且历史被回收** ⇒ handler 直接
    // 回放终局就收尾，压根不会建订阅者 ⇒ 本用例会测成"从来没订上"（下面那条
    // "先断言订上了"的断言就是为防这个而写的，它这次真的逮到了 ✓）。
    // 要让"断开时退订"这件事可测，这条 run 必须**在内存中枢里**（= 真在跑）。
    {
        // `emit` 来自 `EventSink` trait ⇒ 得把它带进来（生产代码里是 sink 那侧用的）
        use mcloud_core::EventSink as _;
        handle
            .hub
            .emit(mcloud_core::RunEvent::info(run_id, "跑着呢", 1));
    }
    assert_eq!(handle.hub.subscriber_count(run_id), 0, "前提：还没人订阅");

    // 拿住响应体不 drop —— 此时订阅**必须**是活的
    let resp = authed_app(Arc::clone(&handle))
        .oneshot(
            Request::builder()
                .uri("/api/runs/7/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("请求应被处理");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        handle.hub.subscriber_count(run_id),
        1,
        "响应拿到手时订阅就该是活的（否则下面的断言测的是「从来没订上」）"
    );

    // 客户端断开 = 丢掉响应体（`EventSource.close()` / 关标签页都走这条路）
    drop(resp);
    assert_eq!(
        handle.hub.subscriber_count(run_id),
        0,
        "断开后订阅者条目必须清掉 —— 留着就是一次运行漏一份"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── 定时任务的记账字段（2026-10-03 review §1.4 / §1.7）──────────────────

/// ⭐ 更新定时**不能**把记账字段抹掉。
///
/// ⭐⭐ 2026-10-07 用户实测报的 bug：**停用一条「跑编排」的定时被拒**。
///
/// 截图原话：「无法停用定时任务 / 配置错误：这条定时要跑编排，但 body 里没有 pipelineId」。
///
/// # 根因
///
/// 界面上「停用/启用」（`schedules-page.tsx` 的 `toggleEnabled` ✓）与编辑表单**都不发 `body`** ✗，
/// 而 `ScheduleInput.body` 那时是 `serde_json::Value` + `#[serde(default)]`
/// ⇒ 缺席与 `null` **分不出来** ✗ ⇒ 那句校验把"没给"当成"没有 pipelineId"拒掉 ✗。
///
/// 同一个毛病在**带参数的任务**上更坏：编辑一条 `live-room` 的定时会把口令**清空** ✗
/// （静默的数据丢失 —— 比报错更坏）。
///
/// # 现在的口径
///
/// **没给 = 沿用旧值** ✓（新建时才是 `Null` ✓）。这条用例把两半都钉住：
/// ① `task: "pipeline"` 的定时不带 `body` 也能停用 ✓；② 带参数的任务不带 `body` 也不丢参数 ✓。
#[tokio::test]
async fn schedule_update_without_body_keeps_the_old_one() {
    let (handle, dir) = test_app("sched-put-keeps-body");

    // 校验会查"这条编排属于你" ⇒ 先造一条属于 `test-user` 的编排 ✓
    let pid = mcloud_core::PipelineId::new("p-default");
    handle
        .store
        .save_pipeline(&mcloud_core::Pipeline {
            id: pid.clone(),
            name: "每日一键（默认）".into(),
            accounts: vec![],
            account_mode: mcloud_core::AccountMode::Sequential,
            stop_on_error: false,
            steps: vec![mcloud_core::PipelineStep {
                task: "sign".into(),
                body: serde_json::json!({}),
                continue_on_error: true,
            }],
            enabled: true,
            owner: Some(mcloud_core::UserId::new("test-user")),
            last_run_ms: None,
            push_rules: vec![],
            push_events: vec![],
        })
        .await
        .unwrap();

    let id = "sched-put-keeps-body";
    let created = serde_json::json!({
        "name": "每日一键",
        "task": "pipeline",
        "accounts": [],
        "expr": "0 8 * * *",
        "timezone": "Asia/Shanghai",
        "body": { "pipelineId": "p-default" },
        "enabled": true,
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id}"), &created),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");

    // ① **不带 body** 地停用 —— 用户截图里那一步 ✓
    let toggle = serde_json::json!({
        "name": "每日一键",
        "task": "pipeline",
        "accounts": [],
        "expr": "0 8 * * *",
        "timezone": "Asia/Shanghai",
        "enabled": false,
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id}"), &toggle),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "不带 body 的停用必须成功：{out}");
    let v = json_body(&out);
    assert_eq!(v["enabled"], false, "{out}");
    assert_eq!(
        v["body"]["pipelineId"], "p-default",
        "body 要**沿用旧值**，不能被这次没带 body 的请求清空：{out}"
    );

    // ② 带参数的任务同理：编辑（不带 body）不能把参数清空 ✓
    let id2 = "sched-put-keeps-body-2";
    let live = serde_json::json!({
        "name": "直播口令",
        "task": "live-room",
        "accounts": [],
        "expr": "0 9 * * *",
        "timezone": "Asia/Shanghai",
        "body": { "codes": "口令甲" },
        "enabled": true,
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id2}"), &live),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let renamed = serde_json::json!({
        "name": "直播口令（改名）",
        "task": "live-room",
        "accounts": [],
        "expr": "0 9 * * *",
        "timezone": "Asia/Shanghai",
        "enabled": true,
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id2}"), &renamed),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(
        json_body(&out)["body"]["codes"],
        "口令甲",
        "改个名字不该把口令清空（静默的数据丢失）：{out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 定时上的**推送规则绑定**（2026-10-08 新模型：**可多选** ✓）：
/// **没给 = 保持原样** ✓、**给 `[]` = 清空** ✓。
///
/// # 为什么这两条必须分开（2026-10-07，A 线自报的缺口；2026-10-08 换到 `Vec` 上 ✓）
///
/// `Option<Vec<PushRuleId>>` 里，`{"pushRules": []}` 与"根本没有这一格"都反序列化成
/// `None` ✗ ⇒ 只剩"沿用老行"一种可能 ⇒ **界面上「不绑」会静默失败** ✗（用户选了不推、
/// 保存后还绑着，而且不报错 ✗ —— 最坏的一类）。所以那一格走 `absent_or_list`
/// （**没给 ⇒ 沿用** ✓、**给了 ⇒ 用入参** ✓）。
///
/// 这条用例把两个方向都钉住；顺带钉住"列表上的停用/启用不带这一格"那条老路径 ✓
/// （它必须**沿用**，否则一点停用就把绑定丢了 ✗）。
#[tokio::test]
async fn schedule_push_rules_binding_distinguishes_empty_from_absent() {
    let (handle, dir) = test_app("sched-push-rules-absent-or-empty");

    let base = serde_json::json!({
        "name": "签到",
        "task": "sign",
        "accounts": [],
        "expr": "0 8 * * *",
        "timezone": "Asia/Shanghai",
        "body": {},
        "enabled": true,
    });
    let id = "sched-push-rules-bind";

    // ① 先绑**两条**（可多选 ✓）
    let mut bound = base.clone();
    bound["pushRules"] = serde_json::json!(["r1", "r2"]);
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id}"), &bound),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(
        json_body(&out)["pushRules"],
        serde_json::json!(["r1", "r2"]),
        "绑两条要原样回两条：{out}"
    );

    // ② **不带这一格**（列表上点停用/启用就是这样 ✓）⇒ 必须沿用
    let mut toggled = base.clone();
    toggled["enabled"] = serde_json::json!(false);
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id}"), &toggled),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(
        json_body(&out)["pushRules"],
        serde_json::json!(["r1", "r2"]),
        "不带这一格时必须**沿用**（否则点一下停用就把绑定丢了）：{out}"
    );

    // ③ **显式给 `[]`**（界面上的「不推」✓）⇒ 必须真的清掉
    let mut cleared = base.clone();
    cleared["pushRules"] = serde_json::json!([]);
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id}"), &cleared),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // ⚠ 空清单是 `skip_serializing_if` 掉的 ⇒ 读回来是**缺席**（`null`）而不是 `[]` ✓
    // —— 两者在"清空"这个语义上等价 ✓。
    let v = json_body(&out);
    assert!(
        v["pushRules"].is_null() || v["pushRules"] == serde_json::json!([]),
        "显式给 [] 就是**清空**（界面上的「不推」全靠它）：{out}"
    );
    // 再读一次列表确认落盘也是空的（不只是这次响应里没带 ✓）
    let (_, out) = send(authed_app(Arc::clone(&handle)), get("/api/schedules")).await;
    let got = json_body(&out);
    let row = got
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .expect("这条定时还在");
    assert!(
        row["pushRules"].is_null() || row["pushRules"] == serde_json::json!([]),
        "落盘也必须是空的：{out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 2026-10-03 review §1.4：`PUT /api/schedules/{id}` 那个 struct 字面量里，
/// `last_error` / `run_count` 被重置成 `None` / `0`，而同八行上面的注释写的是
/// "保留水位线与**记账字段**" ⇒ 改个名字就把界面上「累计触发 N 次」清零
/// （`web/src/pages/schedules-page.tsx:384` 展示它），
/// 而 `last_status` 还留着 `error`、`last_error` 却是空 —— 写着失败却答不出原因。
#[tokio::test]
async fn schedule_update_keeps_the_accounting_columns() {
    let (handle, dir) = test_app("sched-put-keeps-accounting");

    let body = serde_json::json!({
        "name": "签到",
        "task": "sign",
        "accounts": [],
        "expr": "0 8 * * *",
        "timezone": "Asia/Shanghai",
        "body": {},
        "enabled": true,
    });
    // ⚠ 建/改都是 **PUT `/api/schedules/{id}`**（`/api/schedules` 只有 GET）——
    // 这条路径本身是 upsert，第一次调用就是"新建"。
    let id = "sched-put-keeps-accounting";
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id}"), &body),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(json_body(&out)["id"], id, "{out}");

    // 直接把"跑过一次、且上次失败了"写进库（不跑调度器：那要等 tick）
    let sid = mcloud_core::ScheduleId::new(id);
    handle
        .store
        .mark_schedule_fired(&sid, 1_700_000_000_000)
        .await
        .unwrap();
    handle
        .store
        .record_schedule_run(&sid, Some(1_700_000_000_500), "error", Some("上游 500"))
        .await
        .unwrap();

    // 只改名字
    let mut renamed = body.clone();
    renamed["name"] = serde_json::json!("签到（改名）");
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/schedules/{id}"), &renamed),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let v = json_body(&out);

    assert_eq!(v["name"], "签到（改名）", "{out}");
    assert_eq!(v["runCount"], 1, "改个名字不该把「累计触发」清零：{out}");
    assert_eq!(
        v["lastFireMs"], 1_700_000_000_000i64,
        "水位线要留（清了会把已处理的触发点重放一遍）：{out}"
    );
    assert_eq!(
        v["lastError"], "上游 500",
        "上次错误也要留：清了就是「写着失败却答不出原因」：{out}"
    );
    assert_eq!(v["lastStatus"], "error", "{out}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ── 推送规则（`/api/push/rules`，用户口径 2026-10-07）──────────────────

fn delete_req(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .expect("请求应可构造")
}

/// PUT 建 → GET 列表里有 → PUT 改 → DELETE → 列表空。
///
/// ⚠ 顺带钉住两张清单的**键名**：`key` 必须等于 serde 的 camelCase 名
/// （`runFailed` / `pushPlus`）—— 手写字符串一旦与枚举脱钩，界面选的键与
/// 规则里存的键就对不上，表现是「静默地永不触发」✗（不报错、也查不出）。
#[tokio::test]
async fn push_rules_crud_round_trip() {
    let (handle, dir) = test_app("push-rules-crud");

    // 清单：事件
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/events")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let events = json_body(&out);
    let arr = events.as_array().expect("事件清单是数组");
    let keys: Vec<&str> = arr.iter().map(|e| e["key"].as_str().unwrap()).collect();
    assert!(keys.contains(&"runFailed"), "{out}");
    assert!(keys.contains(&"loginFailed"), "{out}");
    let failed = arr.iter().find(|e| e["key"] == "runFailed").unwrap();
    assert_eq!(failed["label"], "运行失败", "{out}");
    assert_eq!(failed["defaultSelected"], true, "失败默认选上：{out}");
    // ⭐ 2026-10-08：每一项带 `kind`，前端靠它分组 ✓（`match` 穷尽算出来的，
    // 不是前端按 key 硬编的一张表 ✓）。
    assert_eq!(
        failed["kind"], "run",
        "运行类事件要标 run（前端靠它分组）：{out}"
    );
    let login = arr.iter().find(|e| e["key"] == "loginFailed").unwrap();
    assert_eq!(
        login["kind"], "panel",
        "登录/性能是面板类，要标 panel：{out}"
    );

    // 清单：渠道
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/channels")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert!(
        json_body(&out)
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["key"] == "pushPlus"),
        "{out}"
    );

    // 建（⚠ 2026-10-08 新形状：**凭据在规则自己的 `config` 里** ✓；
    // `events` / `channels` / `slowFactor` / `failStreak` 都**不在规则上**了 ✗；
    // ⚠ 2026-10-08 当天稍后又删掉 `quiet`（静默时段）—— 用户口径「不要静默时间」✓）
    let id = "rule-1";
    let body = serde_json::json!({
        "name": "失败时叫我",
        "config": {"bark": {"key": "bark-key-1"}},
        "digest": "perRun",
        "enabled": true,
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/push/rules/{id}"), &body),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(json_body(&out)["id"], id, "{out}");
    assert_eq!(json_body(&out)["name"], "失败时叫我", "{out}");

    // 列表里有（而且只有这一条）
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/rules")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let list = json_body(&out);
    assert_eq!(list.as_array().unwrap().len(), 1, "{out}");
    assert_eq!(list[0]["id"], id, "{out}");
    assert_eq!(list[0]["config"]["bark"]["key"], "bark-key-1", "{out}");
    // ⚠ 静默时段已经整块删掉 ⇒ 读回来的规则里**不该**再有 `quiet` 那一格 ✗
    // （2026-10-08 用户口径「不要静默时间」✓）。
    assert!(list[0].get("quiet").is_none(), "{out}");

    // 改：同一个 id 再 PUT 是**覆盖**，不是又加一条
    let mut renamed = body.clone();
    renamed["name"] = serde_json::json!("改名了");
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/push/rules/{id}"), &renamed),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(json_body(&out)["name"], "改名了", "{out}");
    let (_, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/rules")).await;
    assert_eq!(
        json_body(&out).as_array().unwrap().len(),
        1,
        "覆盖不该多出一条：{out}"
    );

    // 删
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        delete_req(&format!("/api/push/rules/{id}")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let (_, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/rules")).await;
    assert_eq!(
        json_body(&out).as_array().unwrap().len(),
        0,
        "删完列表要空：{out}"
    );

    // 删不存在的 ⇒ 404（别人的 id 与不存在一样，不幂等回 200）
    let (s, _) = send(
        authed_app(Arc::clone(&handle)),
        delete_req("/api/push/rules/never-existed"),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `POST /api/push/test`：**规则不存在 / 不属于当前用户 ⇒ 404** ✓
/// （别人的 id 与不存在一样，不泄露"存在这么一条" ✓ —— 与 `DELETE` 同一条口径 ✓）。
#[tokio::test]
async fn push_test_rejects_unknown_rule_with_404() {
    let (handle, dir) = test_app("push-test-404");

    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json(
            "/api/push/test",
            &serde_json::json!({"ruleId": "never-existed"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{out}");
    assert!(out.contains("never-existed"), "要说清是哪条规则：{out}");

    // 连 `ruleId` 都不给 ⇒ 400（**不是** serde 那句英文 `missing field` ✗）
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/push/test", &serde_json::json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{out}");
    assert!(out.contains("ruleId"), "要告诉用户该给哪个字段：{out}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `POST /api/push/test`：规则**一个渠道都没配** ⇒ **400**（错误原话来自
/// `PushConfig::validate` ✓）—— 一个包都发不出去，不能回一个"测试成功" ✗。
///
/// ⚠ 顺带钉住"**没发**"：用假的推送 HTTP 客户端数枪 ⇒ 被拒的那次**0 枪** ✓
/// （否则"400 了但还是发出去了"这种形状照样绿 ✗）。
#[tokio::test]
async fn push_test_rejects_rule_without_any_channel() {
    let fake = Arc::new(CountingHttp::default());
    let (handle, dir) = test_app_with_notify_http(
        "push-test-no-channel",
        Arc::clone(&fake) as Arc<dyn mcloud_core::HttpClient>,
    );
    // 先让"当前用户"存在（`authed_app` 会种一个 ✓），再塞一条**空配置**的规则
    let owner = seed_test_user_and_session(&handle);
    let rule = mcloud_core::PushRule::new_default("empty-rule", Some(owner.clone()));
    handle
        .store
        .save_push_rule(&owner, &rule)
        .await
        .expect("存规则");

    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json(
            "/api/push/test",
            &serde_json::json!({"ruleId": "empty-rule"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{out}");
    assert!(
        out.contains("没有任何渠道"),
        "错误原话要能自己解释为什么发不出去：{out}"
    );
    settle_push().await;
    assert_eq!(fake.calls(), 0, "被拒的试发一个请求都不该发出去：{out}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `POST /api/push/test` 的**正路**：用那条规则**自己的**平台与凭据发一条 ✓
/// —— 标题以「[测试] 」开头 ✓、正文说清"来自规则 X" ✓、结果按渠道回给调用方 ✓。
#[tokio::test]
async fn push_test_sends_with_the_rules_own_config() {
    let fake = Arc::new(CountingHttp::default());
    let (handle, dir) = test_app_with_notify_http(
        "push-test-send",
        Arc::clone(&fake) as Arc<dyn mcloud_core::HttpClient>,
    );
    let owner = seed_test_user_and_session(&handle);
    let mut rule = mcloud_core::PushRule::new_default("r1", Some(owner.clone()));
    rule.name = "我的规则".into();
    // 只有 Server 酱这一个渠道 ⇒ 这一枪必须正好打 1 次 ✓
    rule.config = serde_json::from_str(r#"{"serverChan":{"token":"SCT-test"}}"#).expect("测试配置");
    handle
        .store
        .save_push_rule(&owner, &rule)
        .await
        .expect("存规则");

    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/push/test", &serde_json::json!({"ruleId": "r1"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["ok"], true, "{out}");
    assert_eq!(v["ruleId"], "r1", "{out}");
    assert_eq!(
        v["title"], "[测试] 我的规则",
        "标题必须以「[测试] 」开头（用户一眼认出它不是运行结果）：{out}"
    );
    assert!(
        v["text"].as_str().is_some_and(|t| t.contains("来自规则")),
        "正文要说清来自哪条规则：{out}"
    );
    // 每个渠道这一次的结果原样回给调用方（成败判据与运行类推送**同一套** ✓）
    let logs = v["logs"].as_array().expect("logs 是数组");
    assert!(
        logs.iter()
            .any(|l| l["msg"].as_str().is_some_and(|m| m.contains("Server酱"))),
        "回里要带上渠道自己写的那句结果：{out}"
    );
    // 真的发出去了（1 个渠道 ⇒ 1 枪 ✓）
    wait_push_calls(&fake, 1).await;

    let _ = std::fs::remove_dir_all(&dir);
}

// ── AI豆明细（`GET /api/beans/records`）──────────────────────────────────
//
// ⚠ 这里**只覆盖"还没碰上游"的那几条**：认证、参数校验、归属判定。
// 成功那一趟要真发 `cloudRecordV2`（`http_for` 建的是真 H5 会话），
// 离线跑不了 —— 流水的解析 / 翻页 / 排序 / 截断都在
// `crates/mcloud-tasks/src/beans/tests.rs` 用 fixture 回放覆盖。
// 这样分工的理由：那几条判据（东八区窗口、倒序、`hasMore`）**与 HTTP 层无关**，
// 摆在路由测试里只会更难写、还更慢。

/// ⭐ 明细**必须**要认证 —— 未登录的正确答案是 401，不是"这个号没流水"的空列表。
#[tokio::test]
async fn bean_records_requires_login() {
    let (handle, dir) = test_app("beans-records-gate");
    let state = ServerState::new(handle);
    let (_, _, _) = send_full(
        mcloud_server::app(state.clone()),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;

    let (s, body) = send(
        mcloud_server::app(state),
        get("/api/beans/records?account=deadbeef&days=7"),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "实际：{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 缺 `account` ⇒ 400。不能是 500，更不能是"空流水"的 200 ——
/// 后者会把"你漏传了参数"显示成"这个号没有流水"。
#[tokio::test]
async fn bean_records_without_account_is_400() {
    let (handle, dir) = test_app("beans-records-noacct");
    let (status, text) = send(authed_app(handle), get("/api/beans/records?days=7")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实际：{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 别人的账号 ⇒ **404**（归属错误一律 404，不泄露"这个号存在但不是你的"）。
///
/// ⚠ 这条同时钉住"归属判定在上游之前"：拿别人的 id 打这个端点，
/// 绝不能变成"替别人去上游读一次账"。
#[tokio::test]
async fn bean_records_of_a_foreign_account_is_404() {
    let (handle, dir) = test_app("beans-records-foreign");
    let mut other = seed_for("19900000009");
    other.owner = Some(mcloud_core::UserId::new("someone-else"));
    let store = Arc::clone(&handle.store);
    futures::executor::block_on(store.save_account(&other)).expect("落库");

    let uri = format!("/api/beans/records?account={}&days=7", other.id.as_str());
    let (status, text) = send(authed_app(handle), get(&uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "实际：{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 不存在的账号 ⇒ 404（与"别人的账号"同一个码，两者不可区分）。
#[tokio::test]
async fn bean_records_of_an_unknown_account_is_404() {
    let (handle, dir) = test_app("beans-records-unknown");
    let (status, text) = send(
        authed_app(handle),
        get("/api/beans/records?account=0000000000000000&days=7"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "实际：{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `days` 越界 ⇒ 400（挡在算窗口之前：`days=999999999` 那种乘法会溢出 panic）。
#[tokio::test]
async fn bean_records_with_out_of_range_days_is_400() {
    let (handle, dir) = test_app("beans-records-days");
    let mine = seed_for("19900000010");
    seed_account(&handle, mine.clone());

    for bad in ["-1", "3651", "999999999"] {
        let uri = format!("/api/beans/records?account={}&days={bad}", mine.id.as_str());
        let (status, text) = send(authed_app(Arc::clone(&handle)), get(&uri)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "days={bad} 实际：{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ══════════════════════════ 第二批安全修复（2026-10-06）══════════════════════
//
// 覆盖审计报告里的 SEC-05 / SEC-07 / SEC-11 / SEC-18。
//
// ⚠ 这几条都需要**两个用户**的实例，而上面的 `authed_app` 只建一个 ——
// 而"最早的那个用户 = 实例管理员"这条判据在单用户实例上**永远成立**，
// 用它测等于什么都没测 ✗。所以这里另起一个双用户装配，让"谁是管理员"可分辨。

/// 两个用户的实例：`admin-user`（创建最早 ⇒ 实例管理员）与 `other-user`。
/// 返回 `(app, admin 的 cookie, other 的 cookie)`。
fn two_user_app(handle: Arc<AppHandle>) -> (axum::Router, String, String) {
    use mcloud_core::{PasswordHash, Session, SessionKind, User, UserId, WEB_SESSION_TTL_MS};

    let now = handle.clock.now_ms();
    let admin_token = "admin-session-token";
    let other_token = "other-session-token";
    // ⚠ `created_at_ms` 必须**差开**：管理员判据是"最早的那个"（`legacy_owner`），
    // 同时刻建的两个用户谁早谁晚没有定义 ⇒ 那样这条测试会随机红 ✗
    let specs = [
        ("admin-user", "admin", admin_token, now - 1_000),
        ("other-user", "other", other_token, now),
    ];

    let mut users = Vec::new();
    let mut sessions = Vec::new();
    for (id, name, token, created_at_ms) in specs {
        users.push(User {
            id: UserId::new(id),
            name: name.into(),
            // 与 `authed_app` 同一条理由：这里不真算 argon2，只用它建会话
            password_hash: PasswordHash("$argon2id$test-only".into()),
            created_at_ms,
        });
        sessions.push(Session {
            token_hash: mcloud_crypto::hash_session_token(token),
            user_id: UserId::new(id),
            kind: SessionKind::Web,
            created_at_ms: now,
            expires_at_ms: now + WEB_SESSION_TTL_MS,
        });
    }

    let store = Arc::clone(&handle.store);
    futures::executor::block_on(async move {
        for u in &users {
            store.save_user(u).await.expect("建测试用户");
        }
        for s in &sessions {
            store.save_session(s).await.expect("建测试会话");
        }
    });

    (
        mcloud_server::app(ServerState::new(handle)),
        format!("mcloud_session={admin_token}"),
        format!("mcloud_session={other_token}"),
    )
}

/// 带指定 cookie 发 POST（`get_with_cookie` / `put_json_with_cookie` 只管 GET / PUT）。
fn post_json_with_cookie(uri: &str, cookie: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .expect("请求应可构造")
}

/// ⭐⭐ 审计 `SEC-07` / `SEC-11` / `SEC-18`：**实例级动作只有管理员能做**。
///
/// 这些路由原先对"任何登录用户"开放：重启服务（现成的 DoS 开关）、
/// 读绝对路径与文件清单、读**所有人**的服务端日志、改全局任务参数、
/// 改全局安全策略 ✗。单管理员部署下无感，多用户时它们全都是越权面。
///
/// 红测：把任一 handler 里的 `require_admin(..)` 删掉 ⇒ 该行断言红。
/// 把 `Error::Forbidden` 的映射改回 401 ⇒ 每条都红（这也是刻意的：
/// 401 会让前端弹登录框，把"权限不够"显示成"你掉线了" ✗）。
#[tokio::test]
async fn instance_level_actions_are_admin_only() {
    let (handle, dir) = test_app("admin-only");
    let (app, admin, other) = two_user_app(handle);

    // `Request` 不是 `Clone`，而同一个动作要对两个身份各发一次 ⇒ 存"按 cookie 造请求"的闭包
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Box<dyn Fn(&str) -> Request<Body>>)> = vec![
        (
            "POST /api/system/restart",
            Box::new(|c: &str| {
                post_json_with_cookie("/api/system/restart", c, &serde_json::json!({}))
            }),
        ),
        (
            "GET /api/system/storage",
            Box::new(|c: &str| get_with_cookie("/api/system/storage", c)),
        ),
        // ⚠ 2026-10-06 起这一条**不再是**"整个 `/api/logs` 只给管理员"：
        // 日志按**类别**分了两档（用户口径「这种应该分为运维日志，且只有管理员可见」）——
        // **运维**那一档仍然是管理员专属（就是这一条 ✓），而**任务**那一档改成
        // "按账号归属给"（自己的账号的才给，见 `a_plain_user_gets_only_their_own_task_logs_not_an_error` ✓）。
        // 所以这里要显式带上 `category=ops`；不带参数时普通用户拿到的是 200 + 自己的任务日志 ✓。
        (
            "GET /api/logs?category=ops",
            Box::new(|c: &str| get_with_cookie("/api/logs?category=ops", c)),
        ),
        (
            "GET /api/tasks/live-room/config",
            Box::new(|c: &str| get_with_cookie("/api/tasks/live-room/config", c)),
        ),
        (
            "PUT /api/tasks/live-room/config",
            Box::new(|c: &str| {
                put_json_with_cookie(
                    "/api/tasks/live-room/config",
                    c,
                    &serde_json::json!({ "params": { "skipKouling": true } }),
                )
            }),
        ),
        (
            "PUT /api/admin/security",
            Box::new(|c: &str| {
                put_json_with_cookie(
                    "/api/admin/security",
                    c,
                    // ⚠ 字段名是 camelCase（`SecurityPatch` 上有 `rename_all`）：写成 snake_case
                    // 会先被 axum 的 Json 提取器拦成 422，压根走不到权限判断 ✗
                    &serde_json::json!({ "minPasswordLen": 8, "sessionTtlHours": 24 }),
                )
            }),
        ),
    ];

    for (what, build) in cases {
        let (st, body) = send(app.clone(), build(&other)).await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "{what} 对非管理员该回 403，实得 {st}：{body}"
        );
        assert!(
            body.contains("没有权限"),
            "{what} 的说明要能看懂（别只说 forbidden）：{body}"
        );

        // 管理员不该被这条闸挡住（各路由自身可能另有 400/404，只要不是 403）
        let (st, body) = send(app.clone(), build(&admin)).await;
        assert_ne!(
            st,
            StatusCode::FORBIDDEN,
            "{what} 把管理员也挡了，实得 {st}：{body}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ 审计 `SEC-05`：**别人的账号不能靠"导入"抢过来**。
///
/// 旧代码用 `account_for(自己)` 判"这条账号在不在"，而它对别人的账号回 `None`
/// ⇒ 代码走"新建"分支 ⇒ **整行覆盖**（昵称、设置、停用状态一起），
/// 并把 owner 改成自己。而 `parse_credential` 只做形状检查、不验真伪 ⇒
/// 不需要知道对方任何口令就能做到 ✗。
///
/// 红测：把 `accounts::import` 里那段"用不带过滤的 `account()` 查一次"删掉 ⇒ 红。
#[tokio::test]
async fn importing_an_account_owned_by_someone_else_is_404() {
    let (handle, dir) = test_app("acct-import-takeover");
    let phone = "19900000002";
    let id = mcloud_core::AccountId::new(mcloud_upstream::account_id_for(phone));

    // 库里先有一条**属于 admin-user** 的账号，带用户改过的昵称
    let mut theirs = seed_for(phone);
    theirs.owner = Some(mcloud_core::UserId::new("admin-user"));
    theirs.nickname = Some("别人的号".into());
    {
        let store = Arc::clone(&handle.store);
        futures::executor::block_on(store.save_account(&theirs)).expect("落库");
    }

    let (app, _admin, other) = two_user_app(Arc::clone(&handle));

    // other-user 拿一条**形状合法**（内容随便）的凭据去"导入"同一条账号
    let body = serde_json::json!({
        "auth": auth_string("pc", phone, 1_893_456_000_000i64),
        "platform": "macos",
        "nickname": "抢过来的",
    });
    let (st, text) = send(
        app.clone(),
        post_json_with_cookie("/api/accounts", &other, &body),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "该按'不存在'处理，实得 {st}：{text}"
    );

    // 那一行必须**一个字段都没动**
    let after = futures::executor::block_on(handle.store.account(&id))
        .expect("读库")
        .expect("账号还在");
    assert_eq!(
        after.owner.as_ref().map(|u| u.as_str()),
        Some("admin-user"),
        "归属被抢走了"
    );
    assert_eq!(after.nickname.as_deref(), Some("别人的号"), "昵称被覆盖了");

    // 反向：**自己的**账号照样能更新（这条修复不能把正常路径也堵上）
    let mine = serde_json::json!({
        "auth": auth_string("pc", phone, 1_893_456_000_000i64),
        "platform": "macos",
        "nickname": "我的",
    });
    let (st, text) = send(
        app.clone(),
        post_json_with_cookie("/api/accounts", &_admin, &mine),
    )
    .await;
    // admin-user 是那条账号的主人 ⇒ 200（不是 404）
    assert_eq!(st, StatusCode::OK, "主人自己更新该成功，实得 {st}：{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 审计 `SEC-08`：**已登录用户的昂贵动作也要限速**。
///
/// `PUT /api/admin/password` 每次都算一次 argon2（19 MiB × 2 轮），而它**不受**
/// 登录闸限速（那时已经登录了）⇒ 不设闸的话，一个已登录用户就能拿它把自己的
/// 机器打满 ✗。键是**用户 id**（不是 IP）：这些动作身份确定，而同一个 IP 后面
/// 可能坐着多个人（局域网），按 IP 限会误伤。
///
/// 这里用"当前口令错"的请求来打：限速在**校验之前**，所以第 11 发必然 429，
/// 而前 10 发是 401（口令不对）—— 两个码不同，正好把"闸在验证前"这件事也钉住。
///
/// 红测：把 `change_password` 里那句 `sensitive_rate_limit` 删掉 ⇒ 最后一发拿到 401 而不是 429。
#[tokio::test]
async fn changing_password_is_rate_limited_per_user() {
    let (handle, dir) = test_app("pw-ratelimit");
    let (app, admin, _other) = two_user_app(handle);

    // ⚠ camelCase（同上：字段名错了会先 422）
    let body = serde_json::json!({ "currentPassword": "wrong", "newPassword": "whatever-long" });
    let mut last = StatusCode::OK;
    for i in 1..=11 {
        let (st, text) = send(
            app.clone(),
            put_json_with_cookie("/api/admin/password", &admin, &body),
        )
        .await;
        last = st;
        if i <= 10 {
            assert_eq!(
                st,
                StatusCode::UNAUTHORIZED,
                "第 {i} 发在配额内，该是'当前口令不对'：{st} {text}"
            );
        }
    }
    assert_eq!(
        last,
        StatusCode::TOO_MANY_REQUESTS,
        "第 11 发必须被限速（配额 10/分）"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 带指定 cookie 发 DELETE。
fn delete_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("请求应可构造")
}

/// ⭐⭐ 审计 `SEC-06`：扫码会话**只归发起它的那个人**。
///
/// `session_id` 是出码方自己拿到的一串随机值，从没打算当密码用（会出现在日志、
/// 前端状态里）。不绑发起者的话，谁拿到它谁就能推进/取消**别人的**扫码会话 ——
/// 而"推进"正好是"把账号挂到谁名下"那一步（QR 码是攻击者出的、受害者扫的）✗。
///
/// 这里用 `DELETE` 验证接线（`POST /api/login/qr` 不发任何上游请求，所以测得动）；
/// `poll` 用的是同一份 `owner` 判据，逻辑由 `login/tests.rs` 的表级测试钉住。
///
/// 红测：把 `cancel_qr` 里的 `remove_for(&session_id, &cur.0)` 改回 `remove(&session_id)`
/// ⇒ 第一条断言红。
#[tokio::test]
async fn a_qr_session_can_only_be_cancelled_by_its_starter() {
    let (handle, dir) = test_app("qr-owner");
    let (app, admin, other) = two_user_app(handle);

    // admin 出一张码
    let (st, text) = send(
        app.clone(),
        post_json_with_cookie(
            "/api/login/qr",
            &admin,
            &serde_json::json!({ "platform": "windows" }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "出码该成功：{st} {text}");
    let sid = json_body(&text)["session_id"]
        .as_str()
        .expect("响应里该有 session_id")
        .to_string();

    // 别人删不掉，且拿到的是"不存在"（不告诉他"这个 id 有，只是不是你的"）
    let (st, text) = send(
        app.clone(),
        delete_with_cookie(&format!("/api/login/qr/{sid}"), &other),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "别人不该取消得掉：{st} {text}");

    // 而且那张码**还在** —— 否则"猜一个 id"就成了一次对别人扫码流程的拒绝服务 ✗
    //（下面这次成功即证明它没被上一次顺手丢掉）
    let (st, text) = send(
        app.clone(),
        delete_with_cookie(&format!("/api/login/qr/{sid}"), &admin),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "发起者自己该删得掉：{st} {text}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ══════════════════════════ 第三批安全修复（2026-10-06）══════════════════════
//
// 覆盖审计报告里的 SEC-12 / SEC-13 / SEC-14 / SEC-16 / SEC-19。

/// ⭐⭐ 审计 `SEC-12`：**连续猜错会被挡，但"口令对的那一次"永远进得来**。
///
/// 这两件事必须同时成立：
/// * 只挡错的不挡对的 ⇒ 限速器不会变成"把管理员锁在门外"的武器
///   （`SEC-02` 那个坑的教训）；
/// * 挡得住 ⇒ 一个每 10 秒试一次口令的脚本（撞不到 30/分/IP 那个闸）
///   会在第 6 次撞墙。
///
/// 红测：① 把 `note_login_failure` 里那次 `check` 删掉 ⇒ 第 6 发拿到 401 而不是 429；
/// ② 把成功分支的 `reset` 换成"先 check 再放行" ⇒ 最后那条"口令对了也进不来"红。
#[tokio::test]
async fn repeated_wrong_passwords_are_limited_but_the_right_one_still_works() {
    let (handle, dir) = test_app("login-fail-limit");
    let app = mcloud_server::app(ServerState::new(handle));

    // 真建一个管理员（走一次 argon2，口令是 correct-horse）
    let (st, _, _) = send_full(
        app.clone(),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // 前 5 次错口令：401，且文案与"用户不存在"那句一致（不构成探测器）
    for i in 1..=5 {
        let (st, body) = send(
            app.clone(),
            post_json("/api/session", &setup_json("admin", "wrong-password")),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "第 {i} 次错口令：{st} {body}");
        assert!(body.contains("用户名或口令错误"), "{body}");
    }

    // 第 6 次：撞上失败计数（5 次 / 5 分钟）
    let (st, body) = send(
        app.clone(),
        post_json("/api/session", &setup_json("admin", "wrong-password")),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::TOO_MANY_REQUESTS,
        "第 6 次该被失败计数挡住：{st} {body}"
    );

    // ⭐ 底线：口令正确的那一次**不受失败计数影响**
    let (st, body) = send(
        app.clone(),
        post_json("/api/session", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "口令对了就必须放行（配额只限制猜错的速度，不限制能不能进来）：{st} {body}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 审计 `SEC-13`：`MCLOUD_COOKIE_SECURE` 打开时，登录 cookie 要带 `Secure`。
///
/// 默认**必须**关着：本工具跑在 `http://127.0.0.1`，加了 `Secure` 浏览器不存，
/// 表现是"登录成功但立刻又未登录" ✗（另一条测试 `session_cookie_is_hardened`
/// 钉的就是默认关着这件事）。
#[tokio::test]
async fn cookie_can_carry_secure_when_the_deployment_asks_for_it() {
    let (handle, dir) = test_app("cookie-secure");
    let mut state = ServerState::new(handle);
    // 等价于部署时设了 `MCLOUD_COOKIE_SECURE=1`
    state.cookie_secure = true;

    let (st, headers, _) = send_full(
        mcloud_server::app(state),
        post_json("/api/setup", &setup_json("admin", "correct-horse")),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let raw = headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(raw.contains("Secure"), "开了开关就该带 Secure：{raw}");
    // 其余属性一个都不能少（与默认那条逐项一致，只是多了 Secure）
    for attr in ["HttpOnly", "SameSite=Strict", "Path=/", "Max-Age="] {
        assert!(raw.contains(attr), "缺 {attr}：{raw}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ 审计 `SEC-19`：**CLI 会话不能当 Web cookie 用**。
///
/// `SessionKind::Cli` 是给命令行用的**长期**会话（`expires_at_ms == 0` 表示
/// 永不过期），而 WebUI 这一侧（闸、登出、改口令踢会话）全都按
/// "Web 会话有到期时间"设计 ⇒ 放它进来等于开了一条永不失效的登录态 ✗。
///
/// 红测：把 `require_session` 里那句 `session.kind != SessionKind::Web` 判断删掉
/// ⇒ 本测试拿到 200（而不是 401）。
#[tokio::test]
async fn cli_sessions_are_not_accepted_as_web_cookies() {
    use mcloud_core::{PasswordHash, Session, SessionKind, User, UserId};

    let (handle, dir) = test_app("cli-session-cookie");
    let token = "cli-session-token";
    let now = handle.clock.now_ms();
    let uid = UserId::new("test-user");
    let user = User {
        id: uid.clone(),
        name: "admin".into(),
        password_hash: PasswordHash("$argon2id$test-only".into()),
        created_at_ms: now,
    };
    let cli = Session {
        token_hash: mcloud_crypto::hash_session_token(token),
        user_id: uid,
        kind: SessionKind::Cli,
        created_at_ms: now,
        // ⚠ 0 = **永不过期**（`is_valid_at` 的约定）⇒ 唯一挡住它的就是 kind 这道判断
        expires_at_ms: 0,
    };
    {
        let store = Arc::clone(&handle.store);
        futures::executor::block_on(async move {
            store.save_user(&user).await.expect("建用户");
            store.save_session(&cli).await.expect("建会话");
        });
    }

    let (st, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get_with_cookie("/api/accounts", &format!("mcloud_session={token}")),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "CLI 会话不该能过 Web 闸：{st} {body}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 审计 `SEC-19`：会话对应的**用户被删掉**之后，那张还没到期的会话立刻失效。
///
/// 会话表与用户表之间没有外键，谁也不会替你对齐它们 ⇒ 只能每次读会话时对一次。
/// 红测：把 `require_session` 里那次 `store.user(..)` 删掉 ⇒ 本测试拿到 200。
#[tokio::test]
async fn a_session_of_a_user_that_no_longer_exists_is_rejected() {
    use mcloud_core::{Session, SessionKind, UserId, WEB_SESSION_TTL_MS};

    use mcloud_core::{PasswordHash, User};

    let (handle, dir) = test_app("orphan-session");
    let token = "orphan-session-token";
    let now = handle.clock.now_ms();
    // ⚠ 库里要有**一个**用户：否则闸会先判"未初始化"回 503，根本走不到会话检查
    //（这条我第一版就踩了 —— 断言拿到 503 而不是 401）
    let real = User {
        id: UserId::new("real-user"),
        name: "admin".into(),
        password_hash: PasswordHash("$argon2id$test-only".into()),
        created_at_ms: now,
    };
    // 会话属于一个**不存在**的用户 —— 这正是"用户被删了、会话还留着"那个状态
    let session = Session {
        token_hash: mcloud_crypto::hash_session_token(token),
        user_id: UserId::new("ghost"),
        kind: SessionKind::Web,
        created_at_ms: now,
        expires_at_ms: now + WEB_SESSION_TTL_MS,
    };
    {
        let store = Arc::clone(&handle.store);
        futures::executor::block_on(async move {
            store.save_user(&real).await.expect("建用户");
            store.save_session(&session).await.expect("建会话");
        });
    }

    let (st, body) = send(
        mcloud_server::app(ServerState::new(handle)),
        get_with_cookie("/api/accounts", &format!("mcloud_session={token}")),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "用户没了的会话不该放行：{st} {body}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ 审计 `SEC-17`：订阅一条**已结束、内存历史已被回收**的 run —— 流必须
/// **立刻收尾**，而不是永远挂着。
///
/// 旧行为：`hub.subscribe` 用 `entry().or_default()` 建一个**非终态**空条目 ⇒
/// `drain` 永远空、通知永远等不到 ⇒ 客户端只能一直收 15 秒一次的 keep-alive ✗。
/// 而这条路的入口是"`run_access` 说 Allowed（存储层有记录）、内存中枢里却没有它"
///（内存只留 8 份终态历史，更老的被回收）—— 完全可达。
///
/// ⚠ 这里用 `send_full`（自带 10 秒超时）**而不是** `send_headers_only`：
/// 本用例要断言的正是"流会结束"，超时挂死会变成一条失败的断言 ✓
///（反过来说：如果哪天又变回"永不结束"，本用例会红在超时那条 expect 上 ✓）。
#[tokio::test]
async fn subscribing_to_a_finished_run_replays_the_terminal_event_then_ends() {
    let (handle, dir) = test_app("sse-evicted-terminal");
    // 造一条**已结束**的记录：内存中枢里没有它（本进程从没发布过它的事件），
    // 归属只能从存储层判 ⇒ 这正是"第 9 条之后被回收"的那个形态。
    handle
        .store
        .upsert_run(&mcloud_core::RunRecord {
            run_id: mcloud_core::RunId::new(42),
            task: "sign".into(),
            account: "13800000000".into(),
            outcome: mcloud_core::TaskOutcome::success("测试完成"),
            duration_ms: 1234,
            started_at_ms: 0,
            status: mcloud_core::RunStatus::Done,
            details: serde_json::Value::Null,
            logs: Vec::new(),
        })
        .await
        .expect("落盘应成功");

    let (status, headers, body) = send_full(authed_app(handle), get("/api/runs/42/events")).await;
    assert_eq!(status, StatusCode::OK, "实际体：{body}");
    assert!(
        headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .starts_with("text/event-stream"),
        "该是事件流：{headers:?}"
    );
    assert!(
        body.contains("event: finished"),
        "该补发一条终局事件：{body}"
    );
    assert!(
        body.contains("\"duration_ms\":1234"),
        "终局事件要带**真实**时长（不是现编的 0）：{body}"
    );
    assert!(
        body.contains("id: 1"),
        "序号必须大于客户端已看到的最大序号（否则前端会当重复丢掉）：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 审计 `SEC-20`：`/api/runs/{id}/logs` 与 `/api/runs/{id}/events` 的可见性
/// 口径必须**一致** —— 一条**正在跑**的 run（记录是结束时才落盘的）两个端点都该 200。
///
/// 旧行为：`/logs` 只查存储层 ⇒ 运行中的 run 一律 404，而同一时刻 `/events` 是 200
/// —— 同一个 run、两个端点给出相反的答案 ✗。
///
/// 红测：把 `list` 里那段 `run_access` 判断换回 `store.run_for(..).is_none()`
/// ⇒ 本用例拿到 404（而不是 200）。
#[tokio::test]
async fn logs_of_a_running_run_are_200_just_like_its_event_stream() {
    let (handle, dir) = test_app("logs-running");
    let run_id = mcloud_core::RunId::new(9);
    {
        // 这条 run 只**在跑**：内存中枢里有它的事件，存储层里没有记录（记录结束时才写）
        use mcloud_core::EventSink as _;
        handle
            .hub
            .emit(mcloud_core::RunEvent::info(run_id, "跑起来了", 1));
    }
    // 归属：本服务发起的运行在**发起那一刻**就登记（生产路径就是 `note_run_owner`）。
    // ⚠ 必须登记到**真正处理请求的那个 state** 上 ⇒ 用 `authed_app_with` 自己造
    //（`authed_app` 内部会另 new 一个 state，登记到另一个上等于没登记 ✗）
    let uid = seed_test_user_and_session(&handle);
    let state = ServerState::new(Arc::clone(&handle));
    state.note_run_owner(run_id, &uid);

    let (status, body) = send(authed_app_with(state), get("/api/runs/9/logs")).await;
    assert_eq!(status, StatusCode::OK, "运行中的 run 该能读到日志：{body}");
    assert!(
        body.contains("跑起来了"),
        "该把内存里那条日志带出来：{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ══════════════════ 日志分档的可见性（2026-10-06 用户口径）══════════════════
//
// 「这种应该分为运维日志，且只有管理员可见吧」
//
// ⚠ 这一组测的是**权限那一层**（HTTP 状态码）。"哪些行进哪一档"以及
// "按账号过滤"的**内容**逻辑在 `crates/mcloud-server/src/logbuf.rs` 的单元测试里
// —— 那边能直接往环里塞行 ✓；集成测试这边**塞不进去**（环形缓冲是进程级静态量，
// 而 `RingLayer` 只在 `main()` 里装、测试进程从来没装过 ⇒ 这一组里 `/api/logs`
// 恒回空数组 ✓）。两半合起来才是完整覆盖 ✓。

/// ⭐⭐ 运维那一档**只有管理员**能拿：普通用户 ⇒ **403**。
///
/// 红测：把 `server_logs.rs` 里那句 `require_admin` 删掉 ⇒ 普通用户会拿到
/// **200**（而环里此刻没有他的任务日志 ⇒ 恰好是空数组，看起来"没泄露"）——
/// 所以本用例断言的是**状态码**，不是内容 ✓（内容那半靠 logbuf 的单测 ✓）。
#[tokio::test]
async fn ops_logs_are_admin_only() {
    let (handle, dir) = test_app("logs-ops-admin");
    let (app, admin, other) = two_user_app(handle);

    let (st, body) = send(
        app.clone(),
        get_with_cookie("/api/logs?category=ops", &admin),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "管理员该能读运维日志：{st} {body}");

    let (st, body) = send(
        app.clone(),
        get_with_cookie("/api/logs?category=ops", &other),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "普通用户不该能读运维日志：{st} {body}"
    );
    assert!(
        body.contains("只有实例管理员"),
        "文案该与别的实例级动作一致：{body}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `before` 游标**被接受**（分页参数不能把接口打挂 ✓）。
///
/// ⚠ 分页**语义**（往上翻、到头）钉在 `logbuf::persistence_tests` 里
///（那边能直接往环/盘里塞行 ✓）；这里只钉"路由这一层认得它" ✓ ——
/// 集成测试进程里 `logbuf` 没挂落盘目录（`init` 在 `serve()` 里 ✓），
/// 所以这里必然拿到**空数组**而不是报错 ✓。
#[tokio::test]
async fn the_logs_page_cursor_is_accepted() {
    let (handle, dir) = test_app("logs-before-cursor");
    let (app, admin, other) = two_user_app(handle);

    for uri in [
        "/api/logs?limit=5",
        "/api/logs?limit=5&before=12345",
        "/api/logs?limit=5&before=0&category=task",
    ] {
        let (st, body) = send(app.clone(), get_with_cookie(uri, &admin)).await;
        assert_eq!(st, StatusCode::OK, "{uri} 该放行：{st} {body}");
        assert_eq!(body.trim(), "[]", "{uri}：测试进程里环是空的 ✓");
    }
    // 普通用户也一样（不带 category=ops 时 ✓）
    let (st, body) = send(app.clone(), get_with_cookie("/api/logs?before=1", &other)).await;
    assert_eq!(st, StatusCode::OK, "{st} {body}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 普通用户请求 `all`（或**漏传** category）**不报错**，只是拿不到运维那一档 ✓。
///
/// 后端把 `all` 收敛成"任务日志 + 自己的账号"（`server_logs.rs` ✓）——
/// 所以这里是 **200 + 空数组**，而不是 403、更不是"全部日志" ✓。
#[tokio::test]
async fn a_plain_user_gets_only_their_own_task_logs_not_an_error() {
    let (handle, dir) = test_app("logs-plain-user");
    let (app, _admin, other) = two_user_app(handle);

    for uri in [
        "/api/logs",
        "/api/logs?category=all",
        "/api/logs?category=task",
    ] {
        let (st, body) = send(app.clone(), get_with_cookie(uri, &other)).await;
        assert_eq!(
            st,
            StatusCode::OK,
            "{uri} 该放行（只是内容受限）：{st} {body}"
        );
        assert_eq!(
            body.trim(),
            "[]",
            "{uri}：他没有账号 ⇒ 一条任务日志都没有（而运维那一档压根不该给）"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 写错的 `category` **报错**，不静默当成 `all` ✓。
///
/// 静默兜底的失效方向是**失败开放**的：把 `ops` 打成 `opss` 的人会拿到
/// **全部**日志（而不是 400）✗ —— 正是"越权"最爱的形状 ✓。
#[tokio::test]
async fn an_unknown_category_is_rejected_instead_of_defaulting_to_all() {
    let (handle, dir) = test_app("logs-bad-category");
    let (app, admin, _other) = two_user_app(handle);

    let (st, body) = send(app, get_with_cookie("/api/logs?category=opss", &admin)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "实际：{st} {body}");
    assert!(body.contains("只认 all / task / ops"), "{body}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ **老 `data/accounts.json` 兼容**：文件里还留着 `"notify": {...}` 时，
/// 整份账号库必须照常读得出来。
///
/// # 为什么必须钉住（2026-10-08 用户口径）
///
/// 「**很显然我说的和云盘账户没有关系**，为啥你这里云盘账户有配置」＋
/// 「**丢弃现有情况，重写**」⇒ 推送渠道配置从"每个云盘账号一份"收成
/// "**每个登录用户一份**"（见 `docs/task-log.md` 154）✓ ——
/// `AccountSettings::notify` 那一格已**整个删掉** ✗。
///
/// 而用户机器上那份 `accounts.json` 里**可能还存着**它 ⇒ 删字段的那一刻必须确认
/// **旧文件照样读得出来**：`AccountSettings` 上没有 `deny_unknown_fields`
///（只有线格式 DTO 有）⇒ serde 静默忽略未知键 ✓。
///
/// ⚠ 这条走的是**真 store**：`JsonStore` 启动时全量读盘，解析失败 = 账号库打不开、
/// 整个实例起不来 ✗ —— 比只测 core 那个结构更接近事故现场 ✓。
#[test]
fn legacy_accounts_json_with_notify_still_loads() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mcloud-server-legacy-notify-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 老形状：`settings` 里带着 `notify`（渠道配置）与更早的 `notify_webhook`
    std::fs::write(
        dir.join("accounts.json"),
        r#"[{
            "id": "19900000009",
            "nickname": "老号",
            "auth": {"raw":"x","platform":"android","phone":"19900000009","token":"t","expire":0},
            "device_id": "D-LEGACY",
            "settings": {
                "backup_wait_secs": 30,
                "refresh_token_days": 10,
                "skip_tasks": [],
                "ai_avatar_daily_limit": 10,
                "notify": {"title":"我的推送","onlyError":true,"serverChan":{"token":"SCT-SECRET"}},
                "notify_webhook": "https://old.test/hook"
            }
        }]"#,
    )
    .unwrap();

    let app = AppBuilder::new()
        .data_dir(dir.clone())
        .build()
        .expect("带旧 notify 的 accounts.json 必须能读（读不出来 = 整个实例起不来）");
    let store = Arc::clone(&app.handle.store);
    let all = futures::executor::block_on(store.accounts()).expect("读库");
    assert_eq!(all.len(), 1, "旧账号不许因为一个未知键被丢掉");
    assert_eq!(
        all[0].settings.backup_wait_secs, 30,
        "同一条记录里别的设置照常读出来"
    );
    assert_eq!(all[0].settings.refresh_token_days, 10, "其余字段照常取默认");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 规则**自带凭据**（2026-10-08 用户口径：「这个规则配置推送的 token，
/// 包括推送哪几个平台」✓）。
///
/// 钉两件事：
/// * 存一份带 `serverChan.token` 的规则 ⇒ 读回来 **token 还在** ✓
///   （凭据就在规则的 `config` 里 ✓ —— 原来那层"用户级渠道配置"已经删掉 ✗）；
/// * `config` 里**一个渠道都没有** ⇒ **400** ✓ —— 那样配一条都发不出去 ✗，
///   而用户会以为"我配好了" ✗（判据在 `PushConfig::validate` ✓，
///   路由只是**存之前**调它 ✓）。
#[tokio::test]
async fn push_rule_config_round_trip() {
    let (handle, dir) = test_app("push-rule-config");

    let id = "rule-cfg";
    let body = serde_json::json!({
        "name": "带凭据的规则",
        "config": {"title": "我的推送", "serverChan": {"token": "SCT-abc"}},
        "digest": "perRun",
        "enabled": true,
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(&format!("/api/push/rules/{id}"), &body),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(
        json_body(&out)["config"]["serverChan"]["token"],
        "SCT-abc",
        "{out}"
    );

    // 读回来：token 在 ✓（读侧不做任何脱敏 —— 是本机管理员自己看 ✓）
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/rules")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let got = json_body(&out);
    assert_eq!(got[0]["config"]["title"], "我的推送", "{out}");
    assert_eq!(got[0]["config"]["serverChan"]["token"], "SCT-abc", "{out}");

    // 一个渠道都没有 ⇒ 400（不是 200 + 一条发不出去的规则 ✗）
    let empty = serde_json::json!({
        "name": "空配置",
        "config": {"title": "只有标题"},
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json("/api/push/rules/rule-empty", &empty),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "空配置必须被拒：{out}");
    assert!(out.contains("没有任何渠道"), "{out}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 面板行为绑定（`/api/push/panel`，2026-10-08 用户口径：
/// 「在**设置**里，单独为这种**面板行为**增加一个可配置推送规则的入口」✓）。
///
/// 钉三件事：
/// * **没配过 ⇒ 回内置保守默认** ✓（不是 `null` ✗ —— 用户口径
///   「登录，性能，这些可能**频繁触发**，注意」✓）；
/// * `PUT` 一份 ⇒ **回同一份** ✓、`GET` 读回来一致 ✓；
/// * 规则 id 必须**存在且属于当前用户** ✓（`PUT` 校验，见下一条用例 ✓）。
#[tokio::test]
async fn push_panel_round_trip_and_conservative_defaults() {
    let (handle, dir) = test_app("push-panel");

    // ① 没配过 ⇒ 保守默认（登录 every/1800/10、性能 onChange/1800/10 ✓）
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/panel")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let d = json_body(&out);
    assert_eq!(d["loginNew"]["throttle"]["mode"], "every", "{out}");
    assert_eq!(d["loginNew"]["throttle"]["cooldownSecs"], 1800, "{out}");
    assert_eq!(d["loginNew"]["throttle"]["dailyCap"], 10, "{out}");
    assert_eq!(d["loginFailed"]["throttle"]["mode"], "every", "{out}");
    assert_eq!(d["perfSlow"]["throttle"]["mode"], "onChange", "{out}");
    assert_eq!(d["perfSlow"]["throttle"]["cooldownSecs"], 1800, "{out}");
    assert_eq!(d["perfSlow"]["throttle"]["dailyCap"], 10, "{out}");
    assert_eq!(d["perfFailStreak"]["throttle"]["mode"], "onChange", "{out}");

    // ② 先建一条自己的规则（PUT panel 会校验 id 存在 ✓）
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/push/rules/r1",
            &serde_json::json!({
                "name": "性能告警",
                "config": {"bark": {"key": "bark-key-1"}},
            }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");

    // ③ PUT 一份 ⇒ 回同一份 ✓
    let body = serde_json::json!({
        "loginNew": {
            "rules": ["r1"],
            "throttle": {"mode": "every", "cooldownSecs": 60, "dailyCap": 3}
        },
        "perfSlow": {
            "rules": ["r1"],
            "throttle": {"mode": "onChange", "cooldownSecs": 120, "dailyCap": 5},
            "slowFactor": 2.5
        },
        "perfFailStreak": {
            "rules": [],
            "throttle": {"mode": "onChange", "cooldownSecs": 1800, "dailyCap": 10},
            "failStreak": 4
        }
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json("/api/push/panel", &body),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let echoed = json_body(&out);
    assert_eq!(echoed["loginNew"]["rules"][0], "r1", "{out}");
    assert_eq!(echoed["loginNew"]["throttle"]["cooldownSecs"], 60, "{out}");
    assert_eq!(echoed["loginNew"]["throttle"]["dailyCap"], 3, "{out}");
    assert_eq!(echoed["perfSlow"]["slowFactor"], 2.5, "{out}");
    assert_eq!(echoed["perfFailStreak"]["failStreak"], 4, "{out}");

    // ④ GET 读回来一致 ✓
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/panel")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let got = json_body(&out);
    assert_eq!(got["loginNew"]["rules"][0], "r1", "{out}");
    assert_eq!(got["loginNew"]["throttle"]["cooldownSecs"], 60, "{out}");
    assert_eq!(got["perfSlow"]["slowFactor"], 2.5, "{out}");
    assert_eq!(got["perfFailStreak"]["failStreak"], 4, "{out}");
    // 没动过的那一格照旧是默认 ✓
    assert_eq!(
        got["loginFailed"]["throttle"]["cooldownSecs"], 1800,
        "{out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `PUT /api/push/panel` 点名的规则 id **必须存在且属于当前用户** ⇒ 否则 **400** ✓。
///
/// 不校验的后果：任何一个登录用户能把**别人的规则 id** 绑到自己的行为上 ✗ ——
/// 投递侧按 owner 查不到那条规则，表现是"配了但永远不发" ✗（不报错 ✓，最难查的一类）。
#[tokio::test]
async fn push_panel_rejects_unknown_rule() {
    let (handle, dir) = test_app("push-panel-unknown-rule");

    let body = serde_json::json!({
        "loginNew": {"rules": ["never-existed"]},
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json("/api/push/panel", &body),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "绑不存在的规则必须 400：{out}");
    assert!(out.contains("never-existed"), "要说清是哪个 id：{out}");

    // 而且**没落盘**（读回来还是默认：不绑任何规则 ✓）
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/panel")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert!(
        json_body(&out)["loginNew"]["rules"].is_null(),
        "被拒的请求不该留下半份绑定：{out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `slowFactor` 必须 **> 0**（且是有限数）⇒ 否则 **400** ✓ ——
/// `Some(0.0)` 会让「耗时异常」的判据**永远成立**（`耗时 > 中位数 × 0` 恒真 ✓）
/// ⇒ 用户设一次 0，从此每一次运行都推一条"耗时异常" ✗（而界面上那一格看着正常 ✓）。
///
/// ⚠ `failStreak: 0` **不在这里拦**：0 是**合法**的语义（= 关掉连判 ✓）——
/// 把它当错误报掉，等于把用户明确表达的一个意愿判成非法 ✗。
#[tokio::test]
async fn push_panel_rejects_non_positive_slow_factor() {
    let (handle, dir) = test_app("push-panel-slow-factor");

    // 先建一条自己的规则（PUT panel 会校验 id 存在 ✓；阈值那两条不需要它）
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/push/rules/r1",
            &serde_json::json!({
                "name": "性能告警",
                "config": {"bark": {"key": "bark-key-1"}},
            }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");

    for bad in [serde_json::json!(0), serde_json::json!(-1)] {
        let body = serde_json::json!({
            "perfSlow": {
                "rules": ["r1"],
                "throttle": {"mode": "onChange", "cooldownSecs": 1800, "dailyCap": 10},
                "slowFactor": bad
            }
        });
        let (s, out) = send(
            authed_app(Arc::clone(&handle)),
            put_json("/api/push/panel", &body),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "slowFactor = {bad} 必须被拒：{out}"
        );
        assert!(out.contains("倍数必须大于 0"), "错误原话要说清判据：{out}");
    }

    // 正数照旧放行 ✓（2.5 是合法阈值：判据是"多少倍"，不是"至少多少" ✓）
    let ok = serde_json::json!({
        "perfSlow": {
            "rules": ["r1"],
            "throttle": {"mode": "onChange", "cooldownSecs": 1800, "dailyCap": 10},
            "slowFactor": 2.5
        },
        "perfFailStreak": {
            "rules": [],
            "throttle": {"mode": "onChange", "cooldownSecs": 1800, "dailyCap": 10},
            // ⚠ 0 = **关掉连判**，合法 ✓（别顺手把它也判成错 ✗）
            "failStreak": 0
        }
    });
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json("/api/push/panel", &ok),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "slowFactor = 2.5 应当放行：{out}");
    let echoed = json_body(&out);
    assert_eq!(echoed["perfSlow"]["slowFactor"], 2.5, "{out}");
    assert_eq!(
        echoed["perfFailStreak"]["failStreak"], 0,
        "0 是合法的连败阈值（关掉连判）：{out}"
    );

    // 被拒的那两次**没落盘**（读回来是刚存下的 2.5，不是 0 / -1 ✓）
    let (s, out) = send(authed_app(Arc::clone(&handle)), get("/api/push/panel")).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(json_body(&out)["perfSlow"]["slowFactor"], 2.5, "{out}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ── 面板行为的**触发线**：登录那两条（2026-10-08 接上 ✓）────────────────────
//
// 验收要求（§6 第 3、6 条）：**认证路由**上的一次失败登录 / 一次新设备登录
// ⇒ 面板行为收到一次触发，且**用假 HTTP 客户端断言发了几枪** ✓
//（生产那份客户端会把包真的发到用户配的地址上去 ✗）。
//
// ⚠ 触发线是 `spawn` 出去的（HTTP 不能拖住登录 ✓，见 `Notifier::spawn_panel` ✓）
// ⇒ 断言前要先**等它落地**（[`wait_push_calls`] ✓）。

/// 只数"发了几枪"的假 HTTP（**推送投递**那条路 ✓）。
#[derive(Default)]
struct CountingHttp {
    calls: std::sync::atomic::AtomicUsize,
}

impl CountingHttp {
    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl mcloud_core::HttpClient for CountingHttp {
    async fn request(
        &self,
        _req: mcloud_core::HttpRequest,
    ) -> mcloud_core::Result<mcloud_core::HttpResponse> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(mcloud_core::HttpResponse {
            status: 200,
            body: "{}".into(),
            headers: Vec::new(),
        })
    }
}

/// 起一个应用，并把**推送投递**用的 HTTP 客户端换成假的 ✓（见 `AppBuilder::notify_http` ✓）。
fn test_app_with_notify_http(
    tag: &str,
    http: Arc<dyn mcloud_core::HttpClient>,
) -> (Arc<AppHandle>, std::path::PathBuf) {
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
        .notify_http(http)
        .build()
        .expect("应用应能组装");
    (Arc::new(app.handle), dir)
}

/// 建一条**自带凭据**的规则，并把它绑给某个面板行为（走**真落盘** ✓ ——
/// 与设置页那条路逐格一致 ✓）。
async fn bind_panel_rule(
    handle: &Arc<AppHandle>,
    owner: &UserId,
    behavior: mcloud_core::BehaviorKey,
    rule_id: &str,
) {
    let mut rule = mcloud_core::PushRule::new_default(rule_id, Some(owner.clone()));
    rule.config = serde_json::from_str(r#"{"serverChan":{"token":"SC"}}"#).expect("测试配置");
    handle
        .store
        .save_push_rule(owner, &rule)
        .await
        .expect("存规则");

    let mut panel = handle.store.push_panel(owner).await.expect("读面板绑定");
    let slot = panel.get_mut(behavior);
    slot.rules = vec![mcloud_core::PushRuleId::new(rule_id)];
    slot.throttle = mcloud_core::Throttle {
        mode: mcloud_core::ThrottleMode::Every,
        cooldown_secs: 1800,
        daily_cap: 10,
    };
    handle
        .store
        .save_push_panel(owner, &panel)
        .await
        .expect("存面板绑定");
}

/// 登录请求（可以指定 `User-Agent` —— `loginNew` 的判据用它 ✓）。
fn login_req(body: &serde_json::Value, ua: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/session")
        .header("content-type", "application/json")
        .header("user-agent", ua)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .expect("请求应可构造")
}

/// 等 `spawn` 出去的投递落地（**只让出、不睡** ✓）。
async fn wait_push_calls(http: &CountingHttp, n: usize) {
    for _ in 0..3000 {
        if http.calls() >= n {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("等待推送投递超时（期望 {n} 枪，实际 {} 枪）", http.calls());
}

/// 给 `spawn` 出去的任务一点时间，然后断言**没有新增** ✓。
async fn settle_push() {
    for _ in 0..500 {
        tokio::task::yield_now().await;
    }
}

/// ⭐ §6 验收第 3 条：认证路由上**一次失败登录** ⇒ 面板行为收到**一次触发** ✓
/// （假 HTTP 客户端断言**发了 1 个请求** ✓）；**冷却期内第二次不发** ✓。
///
/// ⚠ 这里用的是**库里存在**的那个用户名 ⇒ owner 拿得到 ✓
///（用户名根本不存在那条路**不发** —— 拿不到归属就不猜 ✓，见 `note_login_failure` ✓）。
#[tokio::test]
async fn a_failed_login_triggers_the_panel_once_per_cooldown() {
    let fake = Arc::new(CountingHttp::default());
    let (handle, dir) = test_app_with_notify_http(
        "login-failed-panel",
        Arc::clone(&fake) as Arc<dyn mcloud_core::HttpClient>,
    );
    let owner = seed_test_user_and_session(&handle);
    bind_panel_rule(
        &handle,
        &owner,
        mcloud_core::BehaviorKey::LoginFailed,
        "login-failed-rule",
    )
    .await;

    // ① 第一次失败（口令错）⇒ **发一枪** ✓
    let (s, out) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        login_req(
            &serde_json::json!({"username":"admin","password":"wrong"}),
            "UA-test",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{out}");
    wait_push_calls(&fake, 1).await;

    // ② 冷却期内第二次失败 ⇒ **不发** ✓（节流那一档的事 ✓，不许静默 ⇒ 下面查留痕 ✓）
    let (s, out) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        login_req(
            &serde_json::json!({"username":"admin","password":"wrong-again"}),
            "UA-test",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{out}");
    settle_push().await;
    assert_eq!(fake.calls(), 1, "冷却期内第二次不该再发");

    // ③ 留痕落盘了（`data/push-throttle.json` ✓）：发过 1 条、被拦 1 次 ✓
    let st = handle
        .store
        .push_throttle(&owner)
        .await
        .expect("读节流状态")
        .get(mcloud_core::BehaviorKey::LoginFailed);
    assert_eq!(st.sent_today, 1, "第一条真的发出去了");
    assert_eq!(
        st.throttled_today, 1,
        "被拦下必须留痕（用户口径：不许静默 ✗）"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⚠⚠ `loginNew`（「登录新设备」）**接的是什么** —— 这条把近似关系说清楚 ✓。
///
/// 本仓**没有真设备**这个概念 ✗（登录请求里只有用户名与口令 ✓）⇒ 判据退化成
/// 「这个账号 + 这个 `User-Agent` + 这个 peer IP **没见过**」✓
///（见 `routes/auth.rs::login_device_key` ✓）。
///
/// ⇒ 钉三件事（全部走**成功登录**那条路 ✓）：
/// * 第一次（这台设备没见过）⇒ **报** ✓；
/// * 第二次（同一台设备）⇒ **不报** ✓ —— 这一条正是"**不许**拿'任何一次成功登录'
///   冒充'新设备'"✗（那是骗人 ✓）；
/// * 换一台设备（UA 变了）⇒ **又报** ✓。
#[tokio::test]
async fn login_new_fires_only_for_an_unseen_device() {
    let fake = Arc::new(CountingHttp::default());
    let (handle, dir) = test_app_with_notify_http(
        "login-new-panel",
        Arc::clone(&fake) as Arc<dyn mcloud_core::HttpClient>,
    );

    // 建一个**真能登进来**的用户（口令哈希用真 argon2 ✓ —— 登录要真验一遍 ✓）。
    let digest = mcloud_crypto::to_digest_credential("secret-pw");
    let hash = mcloud_crypto::hash_password(&digest).expect("算口令哈希");
    let user = User {
        id: UserId::new("login-new-user"),
        name: "someone".into(),
        password_hash: PasswordHash(mcloud_crypto::mark_digest_scheme(&hash)),
        created_at_ms: handle.clock.now_ms(),
    };
    handle.store.save_user(&user).await.expect("存用户");
    bind_panel_rule(
        &handle,
        &user.id,
        mcloud_core::BehaviorKey::LoginNew,
        "new-login-rule",
    )
    .await;
    // 冷却设 0 ⇒ 后面"换设备"那一次不该被节流挡住 ✓
    //（这一节验的是**设备判据**，不是节流 —— 节流另有专门用例 ✓）。
    let mut panel = handle.store.push_panel(&user.id).await.expect("读面板绑定");
    panel.login_new.throttle.cooldown_secs = 0;
    handle
        .store
        .save_push_panel(&user.id, &panel)
        .await
        .expect("存面板绑定");

    let body = serde_json::json!({"username":"someone","password":"secret-pw"});

    // ① 第一次：这台设备没见过 ⇒ **报** ✓
    let (s, out) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        login_req(&body, "UA-first"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    wait_push_calls(&fake, 1).await;

    // ② 同一台设备再登一次 ⇒ **不报** ✓
    let (s, out) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        login_req(&body, "UA-first"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    settle_push().await;
    assert_eq!(
        fake.calls(),
        1,
        "同一台设备再登录**不算**新设备 —— 否则就是拿「任何一次成功登录」冒充新设备 ✗"
    );

    // ③ 换一台设备（UA 变了）⇒ **又报** ✓
    let (s, out) = send(
        mcloud_server::app(ServerState::new(Arc::clone(&handle))),
        login_req(&body, "UA-second"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    wait_push_calls(&fake, 2).await;

    // 判据串落盘了（`data/login-devices.json` ✓）：两台设备各一条 ✓
    let raw = std::fs::read_to_string(dir.join("login-devices.json")).expect("读设备表");
    assert!(
        raw.contains("UA-first") && raw.contains("UA-second"),
        "见过的设备要落盘（重启之后同一台不该又变成新的 ✓）：{raw}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── 直播自动定时（`/api/live-room/auto-schedule`）────────────────────────
//
// 用户口径（2026-10-08）：「为直播增加一个自动设置定时的功能，设置为下次直播开始
// 时间的一个小时后」⇒ 每个用户一条定时（`auto-live-<owner>` ✓），
// 时刻 = 这批账号里最早的「下一场」+ 1 小时 ✓。这几条用例钉的是**接口那一层**：
// 形状、鉴权口径、以及"改开关 / 改账号多选"落到哪几格上 ✓
//（算法与幂等在 `mcloud-app::live_auto` 的用例里 ✓）。
//
// ⚠⚠ **这些用例必须用 [`test_app_with_dead_upstream`]**（不是 `test_app` ✓）：
// 重算会**按账号去上游现取**「下一场」（用户口径 2026-10-08：「下一场应该从上游
// 获取啊」✓），而 `test_app` 用的是生产那份工厂 ⇒ 会把请求打到**真的上游**去 ✗
//（单测里既不可能也不该发生 ✓）。假上游一律报网络错 ⇒ 重算走**池里那份兜底** ✓，
// 于是下面那几条"时刻来自池"的期望原样成立 ✓
//（"上游现取"那条路本身由 `mcloud-app::live_auto` 的用例钉住 ✓）。

/// 往共享口令池里放一场预告场。
///
/// ⚠ 时刻必须真的在**未来**：路由用例用的是**真时钟**（`AppBuilder` 注入的
/// 系统时钟 ✓），而池挑「下一场」时会跳过已经过去的场次
///（`next_start_at` ✓）⇒ 写一个过去的时刻会得到"没有下一场"，用例就会
/// 测出一个假结论 ✗。所以这里固定写 2099 年。
/// 相对"现在"算一个**未来**的墙钟串（东八区，与上游同形 ✓）。
///
/// ⚠⚠ **不要写死 2099 那样的远未来**：本仓的定时是 cron，而 cron **没有年字段**
///（见 `mcloud_core::live_auto::one_shot_expr` ✓）⇒ 一条"2099-01-01 11:00"的表达式
/// 下一次触发其实是 **2027-01-01 11:00** ✗ —— 用例会因此测出一个假结论
///（实测踩过一次 ✓）。真实的"下一场"永远是几天内的事，用例也照这个量级写 ✓。
fn future_stamp(handle: &Arc<AppHandle>, offset_ms: i64) -> String {
    mcloud_core::live_auto::beijing_stamp(handle.clock.now_ms() + offset_ms)
}

/// 两天后的那一刻（用例统一用它当"下一场"）。
const TWO_DAYS_MS: i64 = 2 * 24 * 60 * 60 * 1000;
/// 一小时。
const ONE_HOUR_MS: i64 = 60 * 60 * 1000;

fn seed_live_pool(handle: &Arc<AppHandle>, expect: &str) {
    let now = handle.clock.now_ms();
    let mut pool = mcloud_core::kouling::KoulingPool::default();
    pool.normalize(now);
    pool.note_probe(
        &[mcloud_core::kouling::ProbeRoom {
            room_id: 147608,
            title: "预告".into(),
            status: 0,
            expect_start_time: expect.into(),
            anchor_nickname: mcloud_upstream::live_consts::KOULING_ANCHOR.into(),
        }],
        false,
        now,
    );
    let store = Arc::clone(&handle.store);
    futures::executor::block_on(store.save_kouling_pool(&pool)).expect("存口令池");
}

/// `GET` 在"还没跑过直播任务"时也要能回答（而不是 404 / 空对象）✓ ——
/// 前端那张卡第一次打开就是这个状态 ✓。
#[tokio::test]
async fn live_auto_schedule_get_reports_nothing_computed_yet() {
    let (handle, dir) = test_app("live-auto-get-fresh");
    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        get("/api/live-room/auto-schedule"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["enabled"], false, "{out}");
    assert!(v["at"].is_null(), "没有时刻就给 null，不要占位值：{out}");
    assert_eq!(v["accounts"], serde_json::json!([]), "{out}");
    assert!(
        v["note"].as_str().is_some_and(|s| !s.is_empty()),
        "要说清「还没有这条自动定时」：{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ `PUT` = 改开关 + 账号多选，并**立刻重算一次** ✓ ——
/// 算出来的时刻要真的落到那条定时上（`/api/schedules` 里看得见 ✓）。
#[tokio::test]
async fn live_auto_schedule_put_computes_and_persists() {
    let (handle, dir) = test_app_with_dead_upstream("live-auto-put");
    let acc = mcloud_upstream::account_id_for("19900000031");
    seed_account(&handle, seed_for("19900000031"));
    let next_live = future_stamp(&handle, TWO_DAYS_MS);
    let want_at = future_stamp(&handle, TWO_DAYS_MS + ONE_HOUR_MS);
    seed_live_pool(&handle, &next_live);

    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/live-room/auto-schedule",
            &serde_json::json!({ "enabled": true, "accounts": [acc.clone()] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["enabled"], true, "{out}");
    assert_eq!(v["at"], want_at, "下一场（{next_live}）+ 1 小时：{out}");
    assert_eq!(v["accounts"], serde_json::json!([acc.clone()]), "{out}");

    // 落盘是**那条普通定时**（用户口径：进既有文件，别新开表 ✓）
    //
    // ⚠ 断言用**解析后的字段**而不是 `raw.contains("…")`：落盘那份是美化过的
    // JSON（键值之间有空格 ✓），按子串比会把"格式变了"报成"字段丢了" ✗ ——
    // 而这条要钉的恰恰是字段本身 ✓。
    let raw = std::fs::read_to_string(dir.join("schedules.json")).expect("读定时表");
    let saved: serde_json::Value =
        serde_json::from_str(&raw).expect("schedules.json 应是合法 JSON");
    let saved_auto = saved
        .as_array()
        .expect("定时表应是数组")
        .iter()
        .find(|s| s["id"] == "auto-live-test-user")
        .unwrap_or_else(|| panic!("id 必须是 auto-live-<owner>：{raw}"));
    assert_eq!(
        saved_auto["managedBy"], "live-auto",
        "必须带系统维护标记（前端靠它标注「系统维护」）：{raw}"
    );

    // 它同时出现在定时列表里（用户口径：它是**用户的**定时 ✓），且带着标记 ✓
    let (status, out) = send(authed_app(Arc::clone(&handle)), get("/api/schedules")).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let list = json_body(&out);
    let auto = list
        .as_array()
        .expect("列表应是数组")
        .iter()
        .find(|s| s["id"] == "auto-live-test-user")
        .expect("那条自动定时必须出现在定时列表里");
    assert_eq!(auto["managedBy"], "live-auto", "{out}");
    assert_eq!(auto["task"], "live-room", "{out}");
    assert_eq!(
        auto["expr"],
        mcloud_core::live_auto::one_shot_expr(&want_at).expect("算出来的时刻应能写成 cron"),
        "那条定时的 cron 必须正好是「下一场 + 1 小时」那一刻：{out}"
    );

    // `GET` 读回来是同一份状态（它**只读**，不重算 ✓）
    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        get("/api/live-room/auto-schedule"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["enabled"], true, "{out}");
    assert_eq!(
        v["at"], want_at,
        "GET 反推出来的时刻要与 PUT 算出的一致：{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `PUT` 只改开关时**不能**把账号多选清空（缺席 = 保持原样 ✓）——
/// 定时页上踩过同一个坑（见 `ScheduleInput::body` 的注释 ✓）。
#[tokio::test]
async fn live_auto_schedule_put_without_accounts_keeps_them() {
    let (handle, dir) = test_app_with_dead_upstream("live-auto-put-keep-accounts");
    let a1 = mcloud_upstream::account_id_for("19900000041");
    let a2 = mcloud_upstream::account_id_for("19900000042");
    seed_account(&handle, seed_for("19900000041"));
    seed_account(&handle, seed_for("19900000042"));
    let want_at = future_stamp(&handle, TWO_DAYS_MS + ONE_HOUR_MS);
    seed_live_pool(&handle, &future_stamp(&handle, TWO_DAYS_MS));

    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/live-room/auto-schedule",
            &serde_json::json!({ "enabled": true, "accounts": [a1.clone(), a2.clone()] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");

    // 只翻开关（**不带** accounts 这一格）
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/live-room/auto-schedule",
            &serde_json::json!({ "enabled": false }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["enabled"], false, "{out}");
    assert_eq!(
        v["accounts"],
        serde_json::json!([a1.clone(), a2.clone()]),
        "只改开关不能把账号多选清空：{out}"
    );
    assert_eq!(v["at"], want_at, "关掉之后时刻照样算出来给用户看：{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 关掉之后**不能**被下一次重算自己打开 ✓（那是 `managed_wanted` 与 `enabled`
/// 分成两格的理由 ✓）—— 从接口这一层再钉一次：连 `PUT` 两次只带 `enabled: false`。
#[tokio::test]
async fn live_auto_schedule_stays_off_once_turned_off() {
    let (handle, dir) = test_app_with_dead_upstream("live-auto-off-sticks");
    seed_live_pool(&handle, &future_stamp(&handle, TWO_DAYS_MS));

    for _ in 0..2 {
        let (s, out) = send(
            authed_app(Arc::clone(&handle)),
            put_json(
                "/api/live-room/auto-schedule",
                &serde_json::json!({ "enabled": false }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{out}");
        assert_eq!(json_body(&out)["enabled"], false, "{out}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 账号必须是**自己的**：写别人的 id 会被 400 拒掉（不是静默存下一个跑不起来的定时 ✓）。
#[tokio::test]
async fn live_auto_schedule_put_rejects_accounts_that_are_not_yours() {
    let (handle, dir) = test_app_with_dead_upstream("live-auto-put-foreign-account");
    seed_live_pool(&handle, &future_stamp(&handle, TWO_DAYS_MS));
    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/live-room/auto-schedule",
            &serde_json::json!({ "enabled": true, "accounts": ["19900000099"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert!(out.contains("不属于你"), "要说清是哪个账号：{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 账号**存在、但属于别人** ⇒ 同样 400 ✓。
///
/// # 为什么这条要与上一条分开
///
/// 上一条给的是一个**压根不存在**的 id（`visible_to` 与"查无此号"在
/// `account_for` 里走的是同一条分支 ✓）。只钉那一条的话，"校验写成了
/// `store.account(id).is_some()`"（**不看归属** ✗）照样全绿 ——
/// 而在多用户实例里那意味着 A 能把自己的定时指向 B 的云盘账号 ✗：
/// 调度器只按定时的主人过滤账号（`Scheduler::target_accounts` ✓）⇒
/// 那条定时到点**一个账号都跑不起来**，而接口与界面看起来是配好的 ✗。
#[tokio::test]
async fn live_auto_schedule_put_rejects_another_users_account() {
    let (handle, dir) = test_app_with_dead_upstream("live-auto-put-foreign-owner");
    seed_live_pool(&handle, &future_stamp(&handle, TWO_DAYS_MS));

    // 一个**别人的**账号：id 与手机号哈希一致（与 `seed_for` 同一条口径 ✓），
    // 归属显式写成另一个用户 ⇒ `visible_to` 对 `Some(owner)` 只认相等 ✓。
    let foreign = mcloud_upstream::account_id_for("19900000088");
    let mut account = seed_for("19900000088");
    account.owner = Some(mcloud_core::UserId::new("someone-else"));
    let store = Arc::clone(&handle.store);
    futures::executor::block_on(store.save_account(&account)).expect("落库");

    // 前置条件：它在库里**真的存在** —— 否则这条用例会退化成上面那条"不存在" ✓
    assert!(
        futures::executor::block_on(store.account(&mcloud_core::AccountId::new(foreign.clone())))
            .expect("读账号")
            .is_some(),
        "前置条件：账号必须在库里（只是不归当前用户）"
    );

    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/live-room/auto-schedule",
            &serde_json::json!({ "enabled": true, "accounts": [foreign.clone()] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    assert!(out.contains("不属于你"), "要说清是哪个账号：{out}");

    // 而且**没落盘**：那条自动定时压根不该被建出来 ✓
    assert!(
        futures::executor::block_on(
            store.schedule(&mcloud_core::ScheduleId::new("auto-live-test-user"))
        )
        .expect("读定时")
        .is_none(),
        "被拒的请求不该留下一条跑不起来的定时"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 在**定时页**上翻那条自动定时的开关，与设置页那张卡上的开关必须是**同一格**
///（`managed_wanted` ✓）—— 否则两处会各说各话：在定时页关掉它，
/// 系统下一次重算又按"用户想要"把它打开 ✗（用户会报"我关了它自己又开了"✗）。
///
/// 顺带钉住：**编辑保存不能把"这条是系统维护的"抹掉** —— 抹掉的话，
/// 系统从此不再维护它（变成一条永远停在那儿的普通定时 ✓ 而用户并不知道 ✗）。
#[tokio::test]
async fn toggling_the_auto_schedule_from_the_schedules_page_sets_the_user_intent() {
    let (handle, dir) = test_app_with_dead_upstream("live-auto-toggle-from-schedules");
    seed_live_pool(&handle, &future_stamp(&handle, TWO_DAYS_MS));

    // 先让那条自动定时存在（第一次 PUT ⇒ 系统建出来 ✓）
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/live-room/auto-schedule",
            &serde_json::json!({ "enabled": true }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(json_body(&out)["enabled"], true, "{out}");

    // 再从**定时页**把它停用（整条 PUT，enabled=false ✓）
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/schedules/auto-live-test-user",
            &serde_json::json!({
                "name": "直播自动定时",
                "task": "live-room",
                "accounts": [],
                "expr": "0 0 11 1 1 *",
                "timezone": "Asia/Shanghai",
                "enabled": false,
            }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["enabled"], false, "{out}");
    assert_eq!(
        v["managedBy"], "live-auto",
        "编辑保存不能把「系统维护」这个事实抹掉：{out}"
    );

    // 系统再重算一次（不带任何 patch）⇒ 开关必须**保持关着** ✓
    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json("/api/live-room/auto-schedule", &serde_json::json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(
        v["enabled"], false,
        "在定时页关掉的开关不能被下一次重算打开：{out}"
    );
    assert!(v["at"].as_str().is_some(), "时刻照样算出来给他看：{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ══════════════════════════════════════════════════════════════════════════
// 「凭据刷新」：手动一键刷新 + 自动定时的开关与预设时间（用户口径 2026-10-11 ✓）
// ══════════════════════════════════════════════════════════════════════════

/// 回一份**形状照实拍**的 `refreshToken` 应答（`code="0000"` + `data.token` ✓）。
///
/// ⚠ 新 token **自带过期段**（`a|b|c|<expire_ms>|e` 是实拍形状 ✓）——
/// `AuthRefreshService` 就是从这里取新过期时刻的 ✓（喂明文的话它取不到，
/// 于是每次判"该刷"、**反复真轮换凭据** ✗，见那个函数头上的说明 ✓）。
struct FakeRefreshUpstream {
    /// 新 token 里那个过期时刻（毫秒）。
    new_expire_ms: i64,
}

#[async_trait::async_trait]
impl mcloud_core::HttpClient for FakeRefreshUpstream {
    async fn request(
        &self,
        _req: mcloud_core::HttpRequest,
    ) -> mcloud_core::Result<mcloud_core::HttpResponse> {
        let token = format!("SYNTH|1|RCS|{}|payload", self.new_expire_ms);
        Ok(mcloud_core::HttpResponse {
            status: 200,
            body: format!(
                r#"{{"success":true,"code":"0000","message":"ok","data":{{"token":"{token}"}}}}"#
            ),
            headers: Vec::new(),
        })
    }
}

/// 起一个**按账号的 HTTP 客户端是假的**应用：刷新那一枪会回一份成功的应答 ✓。
///
/// ⚠ 必须注入（不能让它走生产那份 ✓）：生产那份会把请求打到**真的上游**
///（`user-njs.yun.139.com` ✗），路由测试里那既不可能也不该发生
///（与 [`test_app_with_dead_upstream`] 同一条理由 ✓）。
fn test_app_with_refresh_upstream(
    tag: &str,
    new_expire_ms: i64,
) -> (Arc<AppHandle>, std::path::PathBuf) {
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
        .http_for(Arc::new(move |_a: &Account| {
            Arc::new(FakeRefreshUpstream { new_expire_ms }) as Arc<dyn mcloud_core::HttpClient>
        }))
        .build()
        .expect("应用应能组装");
    (Arc::new(app.handle), dir)
}

/// 一条**该刷**的账号（`expire` 在过去 ⇒ 判据是 `Needed` ✓）。
///
/// ⚠ `user_domain_id` **必须有**：`refresh_request` 缺它直接报"请重新登录" ✓
///（那是真实形态，不是测试的凑数 ✓）。
fn seed_refreshable_account(handle: &Arc<AppHandle>, phone: &str, expire_ms: i64) -> String {
    let id = mcloud_upstream::account_id_for(phone);
    let mut a = account(phone);
    a.id = mcloud_core::AccountId::new(id.clone());
    a.auth = AuthInfo {
        raw: auth_string("pc", phone, expire_ms),
        platform: "pc".into(),
        phone: phone.into(),
        token: "t".into(),
        expire: expire_ms,
    };
    a.pc_platform = Some(mcloud_core::PcPlatform::Windows);
    a.user_domain_id = Some("1990000000000000001".into());
    seed_account(handle, a);
    id
}

/// ⭐ 手动刷新**单个账号**：过期了 ⇒ 真刷、换新凭据、回 `refreshed` + 新过期时刻 ✓，
/// 而且**库里真的换了**（窄写落盘 ✓）。
#[tokio::test]
async fn auth_refresh_single_account_rotates_it() {
    let new_expire = 1_893_456_000_000i64;
    let (handle, dir) = test_app_with_refresh_upstream("auth-refresh-single", new_expire);
    let id = seed_refreshable_account(&handle, "19900000031", 1_600_000_000_000);

    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/auth/refresh", &serde_json::json!({ "account": id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["refreshed"], 1, "{out}");
    assert_eq!(v["failed"], 0, "{out}");
    assert_eq!(v["results"][0]["kind"], "refreshed", "{out}");
    assert_eq!(v["results"][0]["account"], id, "{out}");
    assert_eq!(
        v["results"][0]["newExpireMs"], new_expire,
        "新过期时刻来自**新凭据自己**：{out}"
    );

    // 库里真的换了（窄写落盘 ✓）
    let stored = futures::executor::block_on(
        Arc::clone(&handle.store).account(&mcloud_core::AccountId::new(id)),
    )
    .expect("读账号")
    .expect("账号还在");
    assert_eq!(
        stored.auth.expire, new_expire,
        "库里那串凭据的过期时刻必须跟着换（否则每轮都判「该刷」⇒ 反复真轮换 ✗）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 手动刷新**全部**：只刷**我名下**的账号 ✓，且逐条给结局 ✓。
#[tokio::test]
async fn auth_refresh_all_only_touches_my_accounts() {
    let new_expire = 1_893_456_000_000i64;
    let (handle, dir) = test_app_with_refresh_upstream("auth-refresh-all", new_expire);
    let mine = seed_refreshable_account(&handle, "19900000032", 1_600_000_000_000);
    // 一个**别人的**账号：过期、且在库里，但归属是别人 ⇒ 刷全部**不该碰它** ✓
    let foreign_id = mcloud_upstream::account_id_for("19900000033");
    let mut foreign = account("19900000033");
    foreign.id = mcloud_core::AccountId::new(foreign_id.clone());
    foreign.auth = AuthInfo {
        raw: auth_string("pc", "19900000033", 1_600_000_000_000),
        platform: "pc".into(),
        phone: "19900000033".into(),
        token: "t".into(),
        expire: 1_600_000_000_000,
    };
    foreign.pc_platform = Some(mcloud_core::PcPlatform::Windows);
    foreign.user_domain_id = Some("1990000000000000002".into());
    foreign.owner = Some(mcloud_core::UserId::new("someone-else"));
    futures::executor::block_on(Arc::clone(&handle.store).save_account(&foreign)).expect("落库");

    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/auth/refresh", &serde_json::json!({ "all": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["refreshed"], 1, "只刷自己那一个：{out}");
    let ids: Vec<String> = v["results"]
        .as_array()
        .expect("结果应是数组")
        .iter()
        .map(|r| r["account"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ids, vec![mine.clone()], "别人的账号不该出现：{out}");

    // 别人的那条**一个字节都没动** ✓
    let stored = futures::executor::block_on(
        Arc::clone(&handle.store).account(&mcloud_core::AccountId::new(foreign_id)),
    )
    .expect("读账号")
    .expect("账号还在");
    assert_eq!(
        stored.auth.expire, 1_600_000_000_000,
        "刷全部**绝不能**动别人的凭据"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ **不属于你的账号 ⇒ 拒绝**（用户口径点名必须有的一条 ✓）。
///
/// 两种形态都要拒，而且回**同一个 404**（不泄露存在性 ✓）：
/// ① 账号在库里但属于别人 ✓；② 账号压根不存在 ✓。
#[tokio::test]
async fn auth_refresh_rejects_an_account_that_is_not_yours() {
    let (handle, dir) = test_app_with_refresh_upstream("auth-refresh-foreign", 1_893_456_000_000);
    // ① 存在、但属于别人
    let foreign_id = mcloud_upstream::account_id_for("19900000034");
    let mut foreign = account("19900000034");
    foreign.id = mcloud_core::AccountId::new(foreign_id.clone());
    foreign.owner = Some(mcloud_core::UserId::new("someone-else"));
    futures::executor::block_on(Arc::clone(&handle.store).save_account(&foreign)).expect("落库");
    assert!(
        futures::executor::block_on(
            Arc::clone(&handle.store).account(&mcloud_core::AccountId::new(foreign_id.clone()))
        )
        .expect("读账号")
        .is_some(),
        "前置条件：账号必须在库里（只是不归当前用户）"
    );

    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json(
            "/api/auth/refresh",
            &serde_json::json!({ "account": foreign_id }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "别人的账号必须被拒：{out}");
    assert!(
        !out.contains("不属于"),
        "不能说出「不属于你」（那本身就是「这个 id 存在」的信息）：{out}"
    );

    // ② 压根不存在
    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json(
            "/api/auth/refresh",
            &serde_json::json!({ "account": "19900000099" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⚠ 参数矛盾 / 缺参数 ⇒ **400**（不默认「全部」—— 那是个会改凭据的动作 ✓）。
#[tokio::test]
async fn auth_refresh_needs_exactly_one_target() {
    let (handle, dir) = test_app_with_refresh_upstream("auth-refresh-args", 1_893_456_000_000);

    for bad in [
        serde_json::json!({}),
        serde_json::json!({ "all": false }),
        serde_json::json!({ "account": "x", "all": true }),
    ] {
        let (status, out) = send(
            authed_app(Arc::clone(&handle)),
            post_json("/api/auth/refresh", &bad),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} ⇒ {out}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⚠ 还没到提前量 ⇒ **跳过、不打上游**，并**说清原因**（还剩几天 ✓）。
#[tokio::test]
async fn auth_refresh_skips_when_not_due() {
    let (handle, dir) = test_app_with_refresh_upstream("auth-refresh-skip", 1_893_456_000_000);
    // 过期时刻在很远的将来（还剩几万天）⇒ NotYet ✓
    let id = seed_refreshable_account(&handle, "19900000035", 9_999_999_999_999);

    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        post_json("/api/auth/refresh", &serde_json::json!({ "account": id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["skipped"], 1, "{out}");
    assert_eq!(v["refreshed"], 0, "{out}");
    assert_eq!(v["results"][0]["kind"], "skipped", "{out}");
    assert!(
        v["results"][0]["remainingDays"].as_f64().unwrap_or(0.0) > 1.0,
        "跳过必须说清「还剩多少天」（用户口径：把跳过的原因写出来 ✓）：{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 自动定时**开箱就有、默认开启、算得出下一次** ✓（用户口径「自动设置定时」✓）。
#[tokio::test]
async fn auth_refresh_schedule_is_seeded_on_and_scheduled() {
    let (handle, dir) = test_app("auth-refresh-schedule-seed");
    let _ = authed_app(Arc::clone(&handle)); // 建测试用户 + 会话（id 恒为 test-user ✓）
    let uid = mcloud_core::UserId::new("test-user");

    // ⚠ "开箱就有"发生在**启动 / 登录**那一步（`AppHandle::seed_defaults_for` ✓）——
    // 路由测试的 `authed_app` 只建用户与会话（不走登录那条路 ✓），
    // 所以这里显式调一次，钉的正是"登录/启动之后它就已经在那儿" ✓。
    futures::executor::block_on(handle.seed_defaults_for(&uid)).expect("种默认数据");

    // 那条定时在**登录/启动那一步**就该被种出来（`seed_defaults_for` ✓）
    let sch = futures::executor::block_on(Arc::clone(&handle.store).schedule(
        &mcloud_core::auth_refresh::auth_refresh_schedule_id(&mcloud_core::UserId::new(
            "test-user",
        )),
    ))
    .expect("读定时")
    .expect("「凭据自动刷新」必须开箱就有");
    assert!(sch.enabled, "默认开启：{sch:?}");
    assert_eq!(sch.managed_by.as_deref(), Some("auth-refresh"), "{sch:?}");
    assert_eq!(sch.task, "auth-refresh", "{sch:?}");
    assert_eq!(
        sch.expr,
        mcloud_core::auth_refresh::daily_expr(mcloud_core::auth_refresh::DEFAULT_AUTH_REFRESH_AT)
            .unwrap(),
        "cron 必须是那个预设钟点：{sch:?}"
    );

    // `GET` 读回来：开启 + 下一次时刻有值 ✓
    let (status, out) = send(
        authed_app(Arc::clone(&handle)),
        get("/api/auth/refresh-schedule"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["enabled"], true, "{out}");
    assert_eq!(v["at"], "03:00", "{out}");
    assert!(
        v["nextAt"].as_str().unwrap_or("").contains("03:00"),
        "必须算得出下一次触发：{out}"
    );
    assert!(v["note"].is_null(), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 用户关掉之后**重算不许打开** ✓（`managed_wanted: Some(false)` 原样保留 ✓）。
#[tokio::test]
async fn auth_refresh_schedule_stays_off_once_turned_off() {
    let (handle, dir) = test_app("auth-refresh-schedule-off");
    let _ = authed_app(Arc::clone(&handle));

    for round in 0..2 {
        // 关掉它（只带 enabled ✓）
        let (s, out) = send(
            authed_app(Arc::clone(&handle)),
            put_json(
                "/api/auth/refresh-schedule",
                &serde_json::json!({ "enabled": false }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{out}");
        assert_eq!(json_body(&out)["enabled"], false, "第 {round} 轮：{out}");

        // 只改预设时间（**不带** enabled ✓）⇒ 不许把开关打开 ✗
        let (s, out) = send(
            authed_app(Arc::clone(&handle)),
            put_json(
                "/api/auth/refresh-schedule",
                &serde_json::json!({ "at": "22:30" }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{out}");
        assert_eq!(
            json_body(&out)["enabled"],
            false,
            "第 {round} 轮：只改时间不能把用户关掉的开关打开：{out}"
        );
    }

    // 那条定时自己的 `enabled` 也要是 false（调度器读的就是它 ✓）
    let sch = futures::executor::block_on(Arc::clone(&handle.store).schedule(
        &mcloud_core::auth_refresh::auth_refresh_schedule_id(&mcloud_core::UserId::new(
            "test-user",
        )),
    ))
    .expect("读定时")
    .expect("那条定时还在");
    assert!(!sch.enabled, "调度器读的那一格必须是关的：{sch:?}");
    assert_eq!(sch.managed_wanted, Some(false), "意愿如实留着：{sch:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐ 改了预设时间 ⇒ **下一次触发跟着变** ✓（用户口径点名的那条 ✓）。
#[tokio::test]
async fn auth_refresh_schedule_time_change_moves_the_next_fire() {
    let (handle, dir) = test_app("auth-refresh-schedule-time");
    let _ = authed_app(Arc::clone(&handle));

    let (s, out) = send(
        authed_app(Arc::clone(&handle)),
        put_json(
            "/api/auth/refresh-schedule",
            &serde_json::json!({ "at": "22:30" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let v = json_body(&out);
    assert_eq!(v["at"], "22:30", "{out}");
    assert!(
        v["nextAt"].as_str().unwrap_or("").contains("22:30"),
        "下一次触发必须跟着变：{out}"
    );
    assert_eq!(v["enabled"], true, "只改时间不该动开关：{out}");

    // 那条定时的 cron 就是它（调度器读的就是这一格 ✓）
    let sch = futures::executor::block_on(Arc::clone(&handle.store).schedule(
        &mcloud_core::auth_refresh::auth_refresh_schedule_id(&mcloud_core::UserId::new(
            "test-user",
        )),
    ))
    .expect("读定时")
    .expect("那条定时还在");
    assert_eq!(sch.expr, "0 30 22 * * *", "{sch:?}");
    // 而且**落进了设置**（重启后还认这个钟点 ✓）
    let saved = futures::executor::block_on(Arc::clone(&handle.store).settings()).expect("读设置");
    assert_eq!(saved.auth_refresh_at, "22:30");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 非法预设时间 ⇒ **400**（不静默兜默认值 ✗），且**不落盘** ✓。
#[tokio::test]
async fn auth_refresh_schedule_rejects_a_bad_clock() {
    let (handle, dir) = test_app("auth-refresh-schedule-bad-time");
    let _ = authed_app(Arc::clone(&handle));

    for bad in ["25:00", "12:60", "半夜"] {
        let (s, out) = send(
            authed_app(Arc::clone(&handle)),
            put_json(
                "/api/auth/refresh-schedule",
                &serde_json::json!({ "at": bad }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad} ⇒ {out}");
        assert!(out.contains("HH:MM"), "要说清正确形状：{out}");
    }
    let saved = futures::executor::block_on(Arc::clone(&handle.store).settings()).expect("读设置");
    assert_eq!(
        saved.auth_refresh_at,
        mcloud_core::auth_refresh::DEFAULT_AUTH_REFRESH_AT,
        "被拒的请求不该把设置写坏"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ⭐⭐ 到点**该刷就刷、不该刷就跳过** —— 两条都从**调度器那条路**走一遍 ✓。
///
/// # 为什么必须走调度器
///
/// 只测 `AuthRefreshService::refresh` 的话，"定时到点会不会真的调它"
/// 这一层是空的 ✗ —— 而那正是本次新增的东西 ✓。
/// 所以这里用 `Scheduler::tick()`（同步等结论那一版 ✓）真触发一次那条定时，
/// 再看**库里那条定时的记账**（`last_status` ✓）与**凭据有没有换** ✓。
#[tokio::test]
async fn the_scheduled_tick_refreshes_only_what_is_due() {
    let new_expire = 1_893_456_000_000i64;
    let (handle, dir) = test_app_with_refresh_upstream("auth-refresh-tick", new_expire);
    let _ = authed_app(Arc::clone(&handle));
    // ⚠ 那条定时是"开箱就有"的（`seed_defaults_for` ✓）—— `authed_app` 不走登录那条路 ✓
    futures::executor::block_on(handle.seed_defaults_for(&mcloud_core::UserId::new("test-user")))
        .expect("种默认数据");

    // ① 该刷的：过期时刻在过去 ⇒ 到点真刷 ✓
    let due = seed_refreshable_account(&handle, "19900000036", 1_600_000_000_000);
    // ② 不该刷的：还剩几万天 ⇒ 到点跳过 ✓
    let not_due = seed_refreshable_account(&handle, "19900000037", 9_999_999_999_999);

    // 把那条定时推成"现在就该触发"：水位设到 2 秒前、表达式设成每秒一次。
    //
    // ⚠ 水位**不能**设得太早：`decide()` 对"迟到超过 `GRACE_MS`（60 秒）"的
    // 触发点是**整体弃掉**（`Decision::Missed` ✓，那是防"停机 8 小时补跑 8 趟"的 ✓）。
    // 设 2 秒前 ⇒ 下一个点落在宽限窗口内 ⇒ `Due` ✓。
    //（⚠ 这里改的是**测试的装置**，不是业务含义：`expr` 与水位都是那条定时自己的
    //  数据，本用例要的是"让它立刻响一次" ✓。）
    let sid =
        mcloud_core::auth_refresh::auth_refresh_schedule_id(&mcloud_core::UserId::new("test-user"));
    let store = Arc::clone(&handle.store);
    let mut sch = futures::executor::block_on(store.schedule(&sid))
        .expect("读定时")
        .expect("那条定时在");
    sch.expr = "* * * * * *".into(); // 每秒一次（6 段 ✓）
    sch.last_fire_ms = Some(handle.clock.now_ms() - 2_000);
    futures::executor::block_on(store.save_schedule(&sch)).expect("写回");

    let results = handle.scheduler.tick().await.expect("tick 不该失败");
    let fired = results
        .iter()
        .find(|(id, _)| id == &sid)
        .unwrap_or_else(|| panic!("那条定时必须被触发一次：{results:?}"));
    assert!(
        !matches!(fired.1, mcloud_engine::FireOutcome::AbandonedMissed),
        "这条用例要的是**真的触发**，不是「错过太久弃掉」：{results:?}"
    );

    // ① 该刷的：库里真的换了 ✓
    let after_due =
        futures::executor::block_on(store.account(&mcloud_core::AccountId::new(due.clone())))
            .expect("读账号")
            .expect("账号在");
    assert_eq!(
        after_due.auth.expire, new_expire,
        "到点**该刷的**必须真换了凭据"
    );
    // ② 不该刷的：一个字节都没动 ✓
    let after_not_due =
        futures::executor::block_on(store.account(&mcloud_core::AccountId::new(not_due.clone())))
            .expect("读账号")
            .expect("账号在");
    assert_eq!(
        after_not_due.auth.expire, 9_999_999_999_999,
        "到点**不该刷的**必须原样不动（跳过 ✓）"
    );

    // 那条定时的记账：这一趟是 ok ✓（"跳过"不是失败 ✓）
    let after = futures::executor::block_on(store.schedule(&sid))
        .expect("读定时")
        .expect("那条定时在");
    assert_eq!(
        after.last_status.as_deref(),
        Some("ok"),
        "记 ok（跳过是正常结局）：{after:?}"
    );

    // ⭐ 而且**逐条的结局与原因**都留了痕：把这条定时再触发一次、看日志里那句
    //（`record_schedule_run` 只记一句汇总 ✓ ⇒ 逐条原因在服务端日志里 ✓，
    //  那条路由测试覆盖不到，所以这里只钉"记了 ok"这一格 ✓）。
    let _ = std::fs::remove_dir_all(&dir);
}
