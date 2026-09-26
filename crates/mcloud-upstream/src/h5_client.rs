//! H5 业务客户端：自动建会话、自动带票、自动在 `90001` 时刷新重放。
//!
//! # 为什么做成 `HttpClient` 装饰器
//!
//! 任务代码只依赖 `mcloud_core::HttpClient`。把会话逻辑放在这一层意味着：
//!
//! - **任务不需要知道有"会话"这回事** —— 它只管发业务请求；
//! - **`90001` 刷新重放只有一处实现** —— 20 个任务各写一遍必然分叉，
//!   而分叉的表现是"某个任务在票过期时静默失败"；
//! - 请求形状（`referer`、`activityid`、`jwttoken` 头名）也只有一处。
//!
//! # 只对 H5 主机生效
//!
//! `user-njs.yun.139.com`（原生 A 族）走 `Authorization`，不需要票；
//! 对它建会话是白打三枪。所以本装饰器**只接管 `m.mcloud.139.com`
//! 这一族**，其余请求原样透传。
//!
//! 这个边界很重要：mcloud 踩过的坑是"把直播侧 JWT 传进了云盘侧的参数位"，
//! 而根因是**类型把两套凭据降级成了同一个 `string`** —— 摆错顺序在
//! 编译器和测试眼里都合法。这里用"按主机分派"把它变成没有这个位置。

use std::sync::Arc;

use async_trait::async_trait;
use mcloud_core::{Account, HttpClient, HttpRequest, HttpResponse, Result};

use crate::session::SessionRecipe;
use crate::session_source::SessionSource;

/// 需要 `jwttoken` 的主机。
///
/// 与 [`crate::endpoints::JWT_HOSTS`] 同一个集合 —— 那边的判据是"注入
/// `jwttoken` 头的范围"，这里复用它，避免两处各维护一份。
fn needs_session(url: &str) -> bool {
    host_of(url).is_some_and(crate::endpoints::accepts_jwt)
}

/// 取出 URL 的主机（不含 scheme 与端口）。
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    Some(host.split(':').next().unwrap_or(host))
}

/// 带会话的 H5 客户端。
pub struct H5Client {
    /// 底层传输（不注入任何凭证）。
    transport: Arc<dyn HttpClient>,
    /// 会话源：负责建票、缓存、刷新重放。
    session: SessionSource,
}

impl H5Client {
    /// 为某个账号与配方建一个带会话的客户端。
    pub fn new(
        transport: Arc<dyn HttpClient>,
        account: Account,
        recipe: SessionRecipe,
        cache: Arc<crate::session::SessionCache>,
    ) -> Self {
        Self {
            session: SessionSource::new(Arc::clone(&transport), account, recipe, cache),
            transport,
        }
    }

    pub fn session_source(&self) -> &SessionSource {
        &self.session
    }

    /// 原样发送（不建会话）。给原生 A 族请求用。
    async fn passthrough(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.transport.request(req).await
    }
}

#[async_trait]
impl HttpClient for H5Client {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        if !needs_session(&req.url) {
            // 原生族（`user-njs.yun.139.com` 等）不需要票。
            // 对它建会话是白打三枪 —— 而且那三枪里**第一条**就在这个主机上，
            // 会绕回来，所以必须显式短路。
            return self.passthrough(req).await;
        }

        let params = SessionRecipe::SignInPage.params();
        // 调用方若自己指定了 referer，优先用它；否则用会话的。
        // 会话的 referer 里带着本次的 SSO token，是"这条会话属于哪个页面"的凭据。
        let own_referer = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("referer"))
            .map(|(_, v)| v.clone());

        self.session
            .with_session(|sess| {
                // `referer` 与票是同一条会话的两半，必须一起带 ——
                // 只带票的话，表现是"凭据都在、却回 90001"。
                let referer = own_referer.as_deref().unwrap_or(sess.referer.as_str());
                let out = crate::session::h5_headers(
                    strip_referer(req.clone()),
                    params,
                    referer,
                    Some(&sess.jwt_token),
                    "true",
                );
                dedup_headers(out)
            })
            .await
    }
}

/// 去掉调用方给的 `referer`（改用会话里那条）。
fn strip_referer(req: HttpRequest) -> HttpRequest {
    HttpRequest {
        headers: req
            .headers
            .into_iter()
            .filter(|(k, _)| !k.eq_ignore_ascii_case("referer"))
            .collect(),
        ..req
    }
}

/// 同一名字的头只保留**最后一个**。
///
/// 后写覆盖先写是 HTTP 的常规语义，而这个项目里"头被设置两次"是个
/// 真实会发生的路径（`h5_headers` 先给默认 referer，调用方又给一个）。
/// 留着两个同名头会让服务端行为不确定。
fn dedup_headers(req: HttpRequest) -> HttpRequest {
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in req.headers.into_iter().rev() {
        let key = k.to_ascii_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push((k, v));
    }
    out.reverse();
    HttpRequest {
        method: req.method,
        url: req.url,
        headers: out,
        body: req.body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_extraction_handles_ports_and_paths() {
        assert_eq!(
            host_of("https://m.mcloud.139.com/ycloud/x"),
            Some("m.mcloud.139.com")
        );
        assert_eq!(
            host_of("https://m.mcloud.139.com:443/x"),
            Some("m.mcloud.139.com")
        );
        assert_eq!(
            host_of("https://user-njs.yun.139.com/user/x"),
            Some("user-njs.yun.139.com")
        );
        assert_eq!(host_of("not a url"), None);
    }

    #[test]
    fn only_jwt_hosts_need_a_session() {
        assert!(needs_session(
            "https://m.mcloud.139.com/ycloud/signin/page/infoV3"
        ));
        assert!(needs_session(
            "https://caiyun.feixin.10086.cn/market/signin/page/infoV3"
        ));
        // 原生族不需要票 —— 对它建会话是白打三枪
        assert!(!needs_session(
            "https://user-njs.yun.139.com/user/querySpecTokenV2"
        ));
        assert!(!needs_session("https://personal-kd-njs.yun.139.com/hcy/x"));
    }

    #[test]
    fn dedup_keeps_the_last_value() {
        let req = HttpRequest::get("https://x/")
            .header("Referer", "first")
            .header("X-A", "1")
            .header("referer", "second");
        let out = dedup_headers(req);
        let referers: Vec<_> = out
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("referer"))
            .collect();
        assert_eq!(referers.len(), 1, "同名头只留一个");
        assert_eq!(referers[0].1, "second", "保留最后写的那个");
        assert_eq!(out.headers.len(), 2);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use mcloud_core::{
        AccountId, AccountSettings, AuthInfo, DeviceId, HttpClient, HttpRequest, HttpResponse,
    };
    use std::sync::Mutex;

    fn account() -> Account {
        Account {
            id: AccountId::new("19900000000"),
            nickname: None,
            auth: AuthInfo {
                raw: "cmF3".into(),
                platform: "ios".into(),
                phone: "19900000000".into(),
                token: "t".into(),
                expire: 0,
            },
            device_id: DeviceId("d".into()),
            pc_platform: None,
            user_domain_id: None,
            settings: AccountSettings::default(),
        }
    }

    #[derive(Default)]
    struct Rec {
        seen: Mutex<Vec<HttpRequest>>,
    }

    #[async_trait]
    impl HttpClient for Rec {
        async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push(req.clone());
            let body = if req.url.contains("querySpecTokenV2") {
                r#"{"success":true,"code":"0000","data":{"token":"SSO"}}"#
            } else if req.url.contains("api/timestamp") {
                r#"{"code":0,"result":1789800128813}"#
            } else if req.url.contains("tyrzLogin") {
                r#"{"code":0,"result":{"token":"JWT-A"}}"#
            } else if req.url.contains("/business") {
                // 业务接口：只在带票时成功
                if req
                    .headers
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("jwttoken") && v == "JWT-A")
                {
                    r#"{"code":0,"result":{"ok":true}}"#
                } else {
                    r#"{"code":90001,"msg":"未登录"}"#
                }
            } else {
                r#"{"code":0}"#
            };
            Ok(HttpResponse {
                status: 200,
                body: body.into(),
                headers: vec![],
            })
        }
    }

    fn client(http: Arc<Rec>) -> H5Client {
        H5Client::new(
            http,
            account(),
            SessionRecipe::SignInPage,
            Arc::new(crate::session::SessionCache::new()),
        )
    }

    /// ⭐ 任务视角：只管发业务请求，票的事**完全不用管**。
    #[tokio::test]
    async fn task_gets_a_working_request_without_touching_sessions() {
        let http = Arc::new(Rec::default());
        let c = client(Arc::clone(&http));

        let resp = c
            .request(HttpRequest::get("https://m.mcloud.139.com/business"))
            .await
            .unwrap();
        assert!(
            !crate::is_session_expired(&resp),
            "任务不该看到 90001 —— 建会话是客户端透明做的"
        );

        // 三枪 + 一条业务
        let urls: Vec<String> = http
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.url.clone())
            .collect();
        assert!(
            urls.iter().any(|u| u.contains("querySpecTokenV2")),
            "{urls:?}"
        );
        assert!(urls.iter().any(|u| u.contains("tyrzLogin")), "{urls:?}");
        assert!(urls.iter().any(|u| u.contains("/business")), "{urls:?}");
    }

    /// 原生族请求**不建会话**（对它建会话是白打三枪，而且会绕回自己）。
    #[tokio::test]
    async fn native_hosts_pass_through_without_building_a_session() {
        let http = Arc::new(Rec::default());
        let c = client(Arc::clone(&http));

        c.request(HttpRequest::post(
            "https://user-njs.yun.139.com/user/querySpecTokenV2",
        ))
        .await
        .unwrap();

        let urls: Vec<String> = http
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.url.clone())
            .collect();
        assert_eq!(urls.len(), 1, "只该有那一条透传：{urls:?}");
        assert!(!urls.iter().any(|u| u.contains("tyrzLogin")), "不该建会话");
    }

    /// 业务请求带上了 `jwttoken` 与 `referer`，且 `referer` 只有一个。
    #[tokio::test]
    async fn business_request_carries_ticket_and_single_referer() {
        let http = Arc::new(Rec::default());
        let c = client(Arc::clone(&http));
        c.request(HttpRequest::get("https://m.mcloud.139.com/business"))
            .await
            .unwrap();

        let seen = http.seen.lock().unwrap();
        let biz = seen.iter().find(|r| r.url.contains("/business")).unwrap();
        let get = |n: &str| {
            biz.headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(n))
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(get("jwttoken"), vec!["JWT-A"]);
        assert_eq!(get("referer").len(), 1, "referer 只能有一个");
        assert!(get("referer")[0].contains("targetSourceId=001005"));
        assert_eq!(get("activityid"), vec!["sign_in_3"]);
    }

    /// 第二条业务请求复用同一张票（不再打三枪）。
    #[tokio::test]
    async fn second_business_request_reuses_the_ticket() {
        let http = Arc::new(Rec::default());
        let c = client(Arc::clone(&http));
        for _ in 0..3 {
            c.request(HttpRequest::get("https://m.mcloud.139.com/business"))
                .await
                .unwrap();
        }
        let seen = http.seen.lock().unwrap();
        let logins = seen.iter().filter(|r| r.url.contains("tyrzLogin")).count();
        assert_eq!(logins, 1, "三条业务请求只该建一次票");
    }
}
