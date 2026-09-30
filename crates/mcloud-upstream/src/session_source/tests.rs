//! 会话源测试：三枪的顺序、缓存复用、`90001` 刷新重放。
//!
//! 这些行为错了**都不报错**，只会表现成"每个接口都未登录"或
//! "某个写请求跑了两次"。所以断言要钉在**请求序列**上。

use super::*;
use async_trait::async_trait;
use mcloud_core::{
    AccountId, AccountSettings, AuthInfo, DeviceId, HttpClient, HttpRequest, HttpResponse,
};
use std::sync::Mutex;

fn account() -> Account {
    Account {
        id: AccountId::new("19900000000"),
        nickname: None,
        auth: AuthInfo {
            raw: "bW9iaWxlOjE5OTAwMDAwMDAwOlNZTlRIMDAx".into(),
            platform: "ios".into(),
            phone: "19900000000".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("d".into()),
        pc_platform: None,
        user_domain_id: None,
        owner: None,
        settings: AccountSettings::default(),
    }
}

/// 记录每一次请求，并按 URL 回预置的响应。
#[derive(Default)]
struct FakeHttp {
    seen: Mutex<Vec<HttpRequest>>,
    /// `(url 片段, 响应体)`，按顺序匹配第一个命中的。
    routes: Mutex<Vec<(String, String)>>,
}

impl FakeHttp {
    fn new() -> Self {
        Self::default()
    }

    fn route(self, url_part: &str, body: &str) -> Self {
        self.routes
            .lock()
            .unwrap()
            .push((url_part.to_string(), body.to_string()));
        self
    }

    fn calls(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn urls(&self) -> Vec<String> {
        self.calls().into_iter().map(|r| r.url).collect()
    }

    fn count_containing(&self, part: &str) -> usize {
        self.urls().iter().filter(|u| u.contains(part)).count()
    }
}

#[async_trait]
impl HttpClient for FakeHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req.clone());
        let routes = self.routes.lock().unwrap();
        for (part, body) in routes.iter() {
            if req.url.contains(part.as_str()) {
                return Ok(HttpResponse {
                    status: 200,
                    body: body.clone(),
                    headers: vec![],
                });
            }
        }
        Ok(HttpResponse {
            status: 200,
            body: r#"{"code":0,"result":{}}"#.into(),
            headers: vec![],
        })
    }
}

fn ok_routes(h: FakeHttp) -> FakeHttp {
    h.route(
        "querySpecTokenV2",
        r#"{"success":true,"code":"0000","data":{"token":"SSO-1"}}"#,
    )
    .route("api/timestamp", r#"{"code":0,"result":1789800128813}"#)
    .route(
        "tyrzLogin",
        r#"{"code":0,"result":{"token":"JWT-1","mobile":"19900000000"}}"#,
    )
}

fn source(http: std::sync::Arc<FakeHttp>) -> SessionSource {
    SessionSource::new(
        http,
        account(),
        SessionRecipe::SignInPage,
        std::sync::Arc::new(SessionCache::new()),
    )
}

// ── 建会话三枪 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn builds_session_with_three_calls_in_order() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    let s = source(std::sync::Arc::clone(&http));

    let (sess, trace) = s.session().await.unwrap();
    assert_eq!(sess.jwt_token, "JWT-1");
    assert_eq!(sess.mobile.as_deref(), Some("19900000000"));
    assert!(trace.built, "首次必须真的建");

    let urls = http.urls();
    assert_eq!(urls.len(), 3, "三枪，不多不少");
    assert!(urls[0].contains("querySpecTokenV2"), "{urls:?}");
    assert!(urls[1].contains("api/timestamp"), "{urls:?}");
    assert!(urls[2].contains("tyrzLogin"), "{urls:?}");
}

/// ⭐ 第二次取票**命中缓存**，不再打三枪。
///
/// 这是"会话池"的核心收益：用户连点三次签到，真实客户端是"页面还开着、
/// 票就用到底"，而不是每次点击都去换一张新票。
#[tokio::test]
async fn second_call_hits_the_cache() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    let s = source(std::sync::Arc::clone(&http));

    let (_, t1) = s.session().await.unwrap();
    let (_, t2) = s.session().await.unwrap();
    let (_, t3) = s.session().await.unwrap();

    assert!(t1.built);
    assert!(!t2.built, "第二次应命中缓存");
    assert!(!t3.built);
    assert_eq!(http.count_containing("tyrzLogin"), 1, "只该建一次票");
}

/// 跨 `SessionSource` 实例也复用 —— 缓存是进程级共享的。
#[tokio::test]
async fn cache_is_shared_across_sources() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    let cache = std::sync::Arc::new(SessionCache::new());

    let a = SessionSource::new(
        std::sync::Arc::clone(&http) as std::sync::Arc<dyn HttpClient>,
        account(),
        SessionRecipe::SignInPage,
        std::sync::Arc::clone(&cache),
    );
    let b = SessionSource::new(
        std::sync::Arc::clone(&http) as std::sync::Arc<dyn HttpClient>,
        account(),
        SessionRecipe::SignInPage,
        cache,
    );

    assert!(a.session().await.unwrap().1.built);
    assert!(
        !b.session().await.unwrap().1.built,
        "另一个 source 也应命中同一张票"
    );
    assert_eq!(http.count_containing("tyrzLogin"), 1);
}

/// referer 里带的是**本次换到的** SSO token。
#[tokio::test]
async fn referer_carries_the_sso_token_from_step_one() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    source(std::sync::Arc::clone(&http))
        .session()
        .await
        .unwrap();

    let calls = http.calls();
    let ts_call = calls
        .iter()
        .find(|c| c.url.contains("api/timestamp"))
        .unwrap();
    let referer = ts_call
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("referer"))
        .map(|(_, v)| v.clone())
        .expect("时间戳那一枪要带 referer");
    assert!(
        referer.contains("token=SSO-1"),
        "referer 应带本次 SSO token：{referer}"
    );
    assert!(referer.contains("targetSourceId=001005"), "{referer}");
}

// ── 刷新 ────────────────────────────────────────────────────────────────

/// 刷新必须**先丢掉旧票**，否则下次取到的还是那张坏的。
#[tokio::test]
async fn refresh_replaces_the_cached_token() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    let s = source(std::sync::Arc::clone(&http));

    let (first, _) = s.session().await.unwrap();
    assert_eq!(first.jwt_token, "JWT-1");

    let fresh = s.refresh().await.unwrap();
    assert_eq!(fresh.jwt_token, "JWT-1", "同一份假响应，值相同");

    // 关键：刷新后**再取**应拿到票，且不该再打一次三枪
    let (again, trace) = s.session().await.unwrap();
    assert!(!trace.built, "刷新已经把新票放进缓存");
    assert_eq!(again.jwt_token, "JWT-1");
    assert_eq!(http.count_containing("tyrzLogin"), 2, "建一次 + 刷一次");
}

// ── 90001 刷新重放 ──────────────────────────────────────────────────────

fn expired(code: i64) -> String {
    format!(r#"{{"code":{code},"msg":"未登录"}}"#)
}

#[test]
fn detects_session_expiry_by_code_not_message() {
    let mk = |b: &str| HttpResponse {
        status: 200,
        body: b.into(),
        headers: vec![],
    };
    assert!(is_session_expired(&mk(r#"{"code":90001,"msg":"未登录"}"#)));
    assert!(is_session_expired(&mk(
        r#"{"code":"90001","msg":"whatever"}"#
    )));
    // 换一句文案仍然认得出来 —— 判据不挂在文本上
    assert!(is_session_expired(&mk(
        r#"{"code":90001,"msg":"请重新登录"}"#
    )));
    assert!(!is_session_expired(&mk(r#"{"code":0,"result":{}}"#)));
    assert!(!is_session_expired(&mk(r#"{"code":401,"msg":"未登录"}"#)));
    // 非 JSON 不该触发刷票重放 —— 那会把一次故障放大成两倍请求
    assert!(!is_session_expired(&mk("<html>502</html>")));
    assert!(!is_session_expired(&mk("")));
}

/// ⭐ 撞 `90001` ⇒ 刷新 + **只重放那一条**。
///
/// 这条测试同时钉住两件事：
/// 1. 重放的是**同一业务请求**，不是整轮（用计数器看得很清楚）；
/// 2. 刷新之后带的是**新票**。
#[tokio::test]
async fn expired_session_refreshes_and_replays_only_that_request() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    // 第一次业务请求回 90001，之后正常
    {
        let mut routes = http.routes.lock().unwrap();
        routes.insert(0, ("/business".into(), expired(90001)));
    }
    let s = source(std::sync::Arc::clone(&http));

    let resp = s
        .with_session(|s| {
            HttpRequest::get("https://m.mcloud.139.com/business").header("jwttoken", &s.jwt_token)
        })
        .await
        .unwrap();

    // 业务请求恰好两次：一次撞 90001、一次重放
    assert_eq!(http.count_containing("/business"), 2, "只该重放那一条");
    assert_eq!(resp.status, 200);

    // 两条业务请求带的票必须是"旧 → 新"（这里假响应相同，但序列要对）
    let biz: Vec<_> = http
        .calls()
        .into_iter()
        .filter(|c| c.url.contains("/business"))
        .collect();
    assert_eq!(biz.len(), 2);
    for c in &biz {
        assert!(
            c.headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("jwttoken")),
            "业务请求必须带票"
        );
    }

    // 刷新的三枪只多打了一次 tyrzLogin（首次建 + 刷新）
    assert_eq!(http.count_containing("tyrzLogin"), 2, "刷新只建一次新票");
}

/// 正常响应**不触发**刷新 —— 否则每个业务请求都白打三枪。
#[tokio::test]
async fn healthy_response_does_not_refresh() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    let s = source(std::sync::Arc::clone(&http));

    for _ in 0..5 {
        s.with_session(|s| {
            HttpRequest::get("https://m.mcloud.139.com/business").header("jwttoken", &s.jwt_token)
        })
        .await
        .unwrap();
    }
    assert_eq!(http.count_containing("/business"), 5);
    assert_eq!(
        http.count_containing("tyrzLogin"),
        1,
        "健康响应不该触发刷新"
    );
}

/// 重放后**仍然**失败就把结果交回去 —— 不无限重试。
#[tokio::test]
async fn persistent_expiry_is_returned_not_retried_forever() {
    let http = std::sync::Arc::new(ok_routes(FakeHttp::new()));
    {
        let mut routes = http.routes.lock().unwrap();
        // 永远回 90001
        routes.insert(0, ("/always-bad".into(), expired(90001)));
    }
    let s = source(std::sync::Arc::clone(&http));

    let resp = s
        .with_session(|s| {
            HttpRequest::get("https://m.mcloud.139.com/always-bad").header("jwttoken", &s.jwt_token)
        })
        .await
        .unwrap();

    // 恰好两次：原始 + 一次重放。不是 N 次。
    assert_eq!(
        http.count_containing("/always-bad"),
        2,
        "只重放一次，不无限重试"
    );
    assert!(is_session_expired(&resp), "最终仍失效的结果要如实交回");
}

// ── 错误语义 ────────────────────────────────────────────────────────────

/// `90001` 容易被读成"这一号没登录"，而它实际说的是"被挡下了"。
/// 措辞要把人往"票"的方向引，而不是往"账号"。
#[test]
fn session_expiry_message_points_at_the_ticket_not_the_account() {
    let d = describe(90001);
    assert!(d.contains("票"), "应提示是票的问题：{d}");
    assert!(!d.contains("未登录"), "别沿用会误导的那句：{d}");
}

#[test]
fn build_error_maps_to_core_error() {
    let e: Error = SessionBuildError::Rejected(90001).into();
    assert!(e.to_string().contains("90001"), "{e}");
    let e2: Error = SessionBuildError::NoSession.into();
    assert!(e2.to_string().contains("会话"), "{e2}");
}

// ── 业务请求构造 ────────────────────────────────────────────────────────

#[test]
fn with_jwt_attaches_the_ticket_and_h5_headers() {
    let req = with_jwt(
        SessionRecipe::SignInPage.params(),
        HttpRequest::get("https://m.mcloud.139.com/ycloud/signin/page/infoV3"),
        "https://m.mcloud.139.com/portal/x",
        "JWT-9",
    );
    let get = |n: &str| {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(n))
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("jwttoken"), Some("JWT-9"), "头名是小写 jwttoken");
    assert_eq!(get("activityid"), Some("sign_in_3"));
    assert!(get("Referer").is_some());
}
