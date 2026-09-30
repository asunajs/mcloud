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
use mcloud_core::{Account, Error, HttpClient, HttpRequest, HttpResponse, Result};

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

/// 这一发要**哪一页**的票。
///
/// # 为什么按 URL 判、而不是让"链"自报
///
/// 票是按页面签的（同一趟抓包里三张 `jwttoken` 互不相等，各打各的接口 ——
/// 见 `docs/capture/app-msg-push-on-samples.txt` §2 的核对表），而页面在这条链上
/// 唯一的显式痕迹就是**请求路径**。`TaskContext` 里只有一个 `HttpClient`、
/// 不给任务自报配方的地方，所以判据落在这一层。
///
/// ⚠ 拿不到判据时**落签到页**（`SignInPage`）—— 那是今天绝大多数链的真实情况
/// （Go 侧 `RecipeSignInPage` 同样是全仓默认）。这不是"猜"，是**默认档**：
/// 新加一页只在下面加一条前缀，别处一行不用动。
///
/// ⚠ 反过来**不许**仅凭"页面名不同"就在这里加分支：两页若共用一套换票参数，
/// 拆开会变成每趟**多打三枪**、而两张票一模一样 —— 那是空转不是"特有优先"。
/// 加分支的唯一依据是**实拍里那一页自己换过票**。
fn recipe_for(url: &str) -> Option<SessionRecipe> {
    if !needs_session(url) {
        return None;
    }
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| path)
        .unwrap_or("");
    // 「开启通知有礼」那一页：`msgPushOn/*` 在实拍里带的是它自己换的那张票
    // （三张票核对表里 `faa8d052` 用在这 24 个接口上）。
    if path.starts_with("ycloud/msgPushOn/") {
        return Some(SessionRecipe::MsgPushOn);
    }
    // ⚠ **未验证**：`notice` 那条链打的 `openemailsms-service/*` 与签到页那几条
    // （`signin/page`、`signin/task/click`）**是不是同一页**，参考实现里没有明说
    // （Go 侧 Q42 把「开启通知有礼」整族留在公共票，而那一片真跑**没回过鉴权类码**
    // ⇒ "用签到页那张"至少是**已知能用**的）。
    // 所以这一格走默认档、并把它标出来，而不是照着页面名顺手也切成 `MsgPushOn`：
    // 切错的代价是那条链从此带一张**没人验证过**的票，而它的失败长相是业务码。
    Some(SessionRecipe::SignInPage)
}

/// 带会话的 H5 客户端。
pub struct H5Client {
    /// 底层传输（不注入任何凭证）。
    transport: Arc<dyn HttpClient>,
    /// 会话源：负责建票、缓存、刷新重放。
    ///
    /// ⚠ 它按**默认配方**（签到页）建。请求的路径指向别的页时，
    /// [`H5Client::source_for`] 会现建一个——`SessionSource` 只是几个引用，
    /// 真正花钱的那三枪在[`SessionCache`](crate::session::SessionCache) 里缓存，
    /// 所以"每发都现建"不等于"每发都换票"。
    session: SessionSource,
    /// 账号与池：现建别的配方的 source 要用。
    account: Account,
    cache: Arc<crate::session::SessionCache>,
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
            session: SessionSource::new(
                Arc::clone(&transport),
                account.clone(),
                recipe,
                cache.clone(),
            ),
            transport,
            account,
            cache,
        }
    }

    pub fn session_source(&self) -> &SessionSource {
        &self.session
    }

    /// 取**这一发该用**的会话源。
    ///
    /// 与默认配方一致时直接复用现有的那个（绝大多数请求走这条）；
    /// 不一致才现建 —— 见 [`recipe_for`] 与 `H5Client::session` 的说明。
    fn source_for(&self, url: &str) -> SessionSource {
        match recipe_for(url) {
            Some(r) if r != self.session.recipe() => SessionSource::new(
                Arc::clone(&self.transport),
                self.account.clone(),
                r,
                Arc::clone(&self.cache),
            ),
            // 判不出（原生族）或就是默认页 ⇒ 用现成那个
            _ => self.session.clone(),
        }
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

        // ⚠ 会话与签名头**必须同源**：`params` 决定 `activityid` 等头，
        // 而票是按配方签的。两处各写一个 `SignInPage` 会让"票是这一页的、
        // 头是那一页的"—— 那是拼不出会话的，且表现只是一个业务码。
        let source = self.source_for(&req.url);
        let params = source.recipe().params();
        // 调用方若自己指定了 referer，优先用它；否则用会话的。
        // 会话的 referer 里带着本次的 SSO token，是"这条会话属于哪个页面"的凭据。
        let own_referer = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("referer"))
            .map(|(_, v)| v.clone());

        source
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

    /// 转发 `request_raw` —— **两条分支，没有第三条**。
    ///
    /// # 分支一：不需要会话的主机 ⇒ 直通
    ///
    /// 原生 A 族（`user-njs.yun.139.com` 等）本来就不带票，判据与 `request()`
    /// 的短路同一个（[`needs_session`]）。直接 `transport.request_raw` 转发，
    /// 字节不经这一层任何加工。
    ///
    /// # 分支二：需要会话的主机 ⇒ **明确报错**（本版不做）
    ///
    /// 会话路径整个是**围绕文本 `HttpResponse` 建的**：
    /// `SessionSource::with_session` 收的闭包交回 `HttpResponse`，
    /// 而 `90001` 刷新重放也只在那个文本体上判。要给它加一条"返回原始字节"的入口，
    /// 得让换票/重放那套机制整个泛化到字节 —— 那是**另一个增量**，不是转发一行能做到的。
    ///
    /// ⚠ **绝不允许**在这里静默落到默认实现（即内层 `request()`）：那会把
    /// 二进制响应体在有损 UTF-8 解码里毁掉，而且**不报错** —— 正是本仓最忌讳的
    /// 「不发错、不报错，只是东西不对」。宁可**现在**带一句能读的中文原因回来。
    async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
        if !needs_session(&req.url) {
            return self.transport.request_raw(req).await;
        }
        Err(Error::Other(format!(
            "H5 会话主机 {} 暂无 request_raw 通路：现有会话路径围绕文本 HttpResponse 建，             给它加一条返回原始字节的入口是另一个增量，故本版明确报错，             而不是静默退到有损的 request()",
            req.url
        )))
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

    /// 非 UTF-8 的"线上原始字节"：`String` 装不下，用来证明转发没被有损路径掉包。
    const RAW_BYTES: &[u8] = &[0x89, b'P', b'N', b'G', 0xFF, 0x00, 0x80];

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

        async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
            self.seen.lock().unwrap().push(req);
            Ok(RAW_BYTES.to_vec())
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

    /// ⭐ 票是**按页面**签的，而页面在这条链上唯一的显式痕迹是请求路径。
    ///
    /// 依据见 `docs/capture/app-msg-push-on-samples.txt` §2：那一趟三张票互不相等，
    /// `msgPushOn/*` 带的是这一页自己换的那张。
    #[test]
    fn the_notice_page_is_picked_by_path() {
        assert_eq!(
            recipe_for("https://m.mcloud.139.com/ycloud/msgPushOn/task/status"),
            Some(SessionRecipe::MsgPushOn)
        );
        assert_eq!(
            recipe_for("https://m.mcloud.139.com/ycloud/msgPushOn/task/list"),
            Some(SessionRecipe::MsgPushOn)
        );
    }

    /// 别的路径**落签到页这个默认档**（绝大多数链的真实情况）。
    ///
    /// ⚠ 这一条也是"**不许仅凭页面名加分支**"的守门：`/ycloud/signin/...`
    /// 与 `/ycloud/signin/task/taskListV2` 今天都该是签到页，因为**没有实拍**
    /// 证明它们各换各的票。给它们加分支就是每趟多打三枪、而两张票一模一样。
    #[test]
    fn everything_else_falls_back_to_the_signin_page() {
        for url in [
            "https://m.mcloud.139.com/ycloud/signin/page/infoV3",
            "https://m.mcloud.139.com/ycloud/signin/task/taskListV2",
            "https://m.mcloud.139.com/ycloud/signin/page/taskExpansion",
            // 路径里带 `msgpush` 但不是那一页的目录 —— 前缀判据要够窄
            "https://m.mcloud.139.com/ycloud/signin/msgPushOnElsewhere",
        ] {
            assert_eq!(
                recipe_for(url),
                Some(SessionRecipe::SignInPage),
                "应落默认档：{url}"
            );
        }
    }

    /// 原生族（不需要票的）判成 `None` ⇒ 直通，不为它建会话。
    #[test]
    fn native_hosts_need_no_recipe() {
        assert_eq!(
            recipe_for("https://user-njs.yun.139.com/user/querySpecTokenV2"),
            None
        );
        assert_eq!(
            recipe_for("https://personal-kd-njs.yun.139.com/hcy/file/list"),
            None
        );
    }

    /// 原生族主机：`request_raw` **直通**，字节不被改写，且不建会话。
    #[tokio::test]
    async fn native_host_request_raw_passes_through_untouched() {
        let http = Arc::new(Rec::default());
        let c = client(Arc::clone(&http));

        let got = c
            .request_raw(HttpRequest::get(
                "https://user-njs.yun.139.com/user/querySpecTokenV2",
            ))
            .await
            .unwrap();

        assert_eq!(got, RAW_BYTES, "字节必须原样交回");
        let seen = http.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "只该透传一条");
        assert!(
            !seen.iter().any(|r| r.url.contains("tyrzLogin")),
            "不需要票的主机不该建会话"
        );
    }

    /// 需要会话的主机：**明确报错**，而且**一条请求都不发**。
    ///
    /// 静默落到默认实现会走内层 `request()`（有损文本）—— 那正是要防的。
    #[tokio::test]
    async fn session_host_request_raw_is_refused_without_sending_anything() {
        let http = Arc::new(Rec::default());
        let c = client(Arc::clone(&http));

        let err = c
            .request_raw(HttpRequest::get("https://m.mcloud.139.com/business"))
            .await
            .unwrap_err();

        assert!(err.to_string().contains("request_raw"), "实得：{err}");
        assert!(
            http.seen.lock().unwrap().is_empty(),
            "报错前不能发任何请求（含建会话那三枪）"
        );
    }
}
