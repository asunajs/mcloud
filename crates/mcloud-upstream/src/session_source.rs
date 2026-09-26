//! 会话源：把三枪串起来、缓存票、撞 `90001` 时刷新并重放。
//!
//! # 为什么单独一层
//!
//! [`crate::session`] 里都是纯函数（构造请求、解析响应），这一层才碰 `HttpClient`。
//! 分开的好处：请求形状可以离线逐字断言，而网络编排只有一条路径要测。
//!
//! # `90001` 的处理
//!
//! 命中 `90001` ⇒ 刷新会话一次 ⇒ 用新票把**那一条请求**重放一次 ⇒
//! 仍失败就交回调用方按任务失败处理。
//!
//! ⚠ **重放的是"那一条"，不是"整轮"。** 这个区别很要紧：
//! 重放整轮的表现是"某个写请求跑了两次"，而它在余额上看不出、日志里不报错 ——
//! 只有把**请求序列**钉住才抓得到。mcloud 为此专门写了测试。
//!
//! 判据用**错误变体**而不是消息文本：`90001` 的展示串将来可能改字，
//! 而"要不要重放"是行为差异 —— 挂在一句给人看的文案上，改一个字就静默失效。

use std::sync::Arc;

use mcloud_core::{Account, Error, HttpClient, HttpRequest, HttpResponse, Result};

use crate::session::{self, auth_fingerprint, basic_authorization, SessionCache, SessionRecipe};

/// 会话失效的业务码。
///
/// mcloud 的实拍：把别的页面的票带给这一族，回 `{"code":90001,"msg":"未登录"}`。
pub const CODE_SESSION_EXPIRED: i64 = 90001;

/// 一次会话建立的过程记录（便于排错与测试断言请求序列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTrace {
    /// 本次是否**新建**了会话（false = 命中缓存）。
    pub built: bool,
}

/// 会话源：一个 `(账号, 配方)` 对应一条，内部持有缓存与 HTTP 客户端。
///
/// 「一次建会话、后续复用」的语义就在这一层：同一个 `SessionSource` 上
/// 连续调 [`Self::with_session`] 只会建一次票。
pub struct SessionSource {
    http: Arc<dyn HttpClient>,
    account: Account,
    recipe: SessionRecipe,
    /// 进程级共享的缓存。多条 source 共用一份，因此"同账号同配方"
    /// 即使由不同任务发起也只建一次票。
    cache: Arc<SessionCache>,
}

impl SessionSource {
    pub fn new(
        http: Arc<dyn HttpClient>,
        account: Account,
        recipe: SessionRecipe,
        cache: Arc<SessionCache>,
    ) -> Self {
        Self {
            http,
            account,
            recipe,
            cache,
        }
    }

    /// 凭据指纹。凭据轮换后键会变，于是旧票不会被误用。
    fn fingerprint(&self) -> String {
        auth_fingerprint(&self.account.auth.raw)
    }

    /// 取票：先看缓存，没有就建。
    ///
    /// **不发请求**当且仅当缓存命中 —— 建会话是懒的，
    /// 所以"取了一次没用"不花任何代价（除非真的没有票）。
    pub async fn session(&self) -> Result<(session::H5Session, SessionTrace)> {
        if let Some(s) = self
            .cache
            .get(self.account.id.as_str(), self.recipe, &self.fingerprint())
        {
            return Ok((s, SessionTrace { built: false }));
        }
        let s = self.build().await?;
        self.cache.put(
            self.account.id.as_str(),
            self.recipe,
            &self.fingerprint(),
            s.clone(),
        );
        Ok((s, SessionTrace { built: true }))
    }

    /// 强制重建（刷新用）。先把旧票丢掉 —— 留着它下次还会被取到。
    pub async fn refresh(&self) -> Result<session::H5Session> {
        self.cache
            .remove(self.account.id.as_str(), self.recipe, &self.fingerprint());
        let s = self.build().await?;
        self.cache.put(
            self.account.id.as_str(),
            self.recipe,
            &self.fingerprint(),
            s.clone(),
        );
        Ok(s)
    }

    /// 真打三枪建会话。
    ///
    /// ⚠ SSO token **不缓存**：实测"换到的 token 几分钟后重放已被拒"。
    /// 所以它只在这一次调用里活着。
    async fn build(&self) -> Result<session::H5Session> {
        let params = self.recipe.params();

        // ① 换 SSO token（原生 A 族头）
        let resp = self
            .http
            .request(session::spec_token_request(
                params,
                &basic_authorization(&self.account),
            ))
            .await?;
        let sso = session::parse_sso_token(session::ensure_http_ok(&resp, "querySpecTokenV2")?)?;

        // referer 里带的是**本次换到的** SSO token，所以它每个会话都不同。
        // 它同时是"这条会话属于哪个页面"的凭据，后续每个请求都要带同一个。
        let referer = params.page_url(&sso);

        // ② 取服务端时间戳（无鉴权）。签名里的 x-timestamp 必须与它逐字相同。
        let resp = self
            .http
            .request(session::timestamp_request(params, &referer))
            .await?;
        let ts = session::parse_server_timestamp(session::ensure_http_ok(&resp, "api/timestamp")?)?;

        // ③ 换 jwttoken（带签名）
        let sign = session::H5Signature::generate(&ts);
        let resp = self
            .http
            .request(session::tyrz_login_request(params, &sso, &sign, &referer))
            .await?;
        session::parse_jwt_token(session::ensure_http_ok(&resp, "tyrzLogin")?, &referer)
    }

    /// 用会话跑一次请求；只在回 `90001` 时刷新并重放**那一条**。
    ///
    /// 闭包拿到**整个会话**而不只是票：业务请求还要用 `referer`，
    /// 而 referer 与票是同一条会话的两半 —— 只传票的话调用方很容易
    /// 忘了带 referer，表现是"凭据都在、却回 `90001`"。
    pub async fn with_session<F>(&self, build: F) -> Result<HttpResponse>
    where
        F: Fn(&session::H5Session) -> HttpRequest,
    {
        let (s, _) = self.session().await?;
        let first = self.http.request(build(&s)).await?;

        if !is_session_expired(&first) {
            return Ok(first);
        }

        // 票失效：刷一次，用新票重放**这一条**（不是整轮）
        tracing::info!(
            account = %self.account.id,
            recipe = ?self.recipe,
            "会话已失效（90001），刷新后重放该条请求"
        );
        let fresh = self.refresh().await?;
        self.http.request(build(&fresh)).await
    }
}

/// 这个响应是不是"会话失效"。
///
/// ⚠ 按**变体**判而不是消息文本：`90001` 的展示串将来可能改字，
/// 而"要不要刷新重放"是行为差异。挂在一句给人看的文案上，
/// 上游改一个字就静默失效，且没有任何测试会因此变红。
///
/// 解析失败一律当成"不是会话失效" —— 一个非 JSON 的响应不该触发刷票重放，
/// 那只会把一次故障放大成两倍请求量。
pub fn is_session_expired(resp: &HttpResponse) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp.body) else {
        return false;
    };
    match v.get("code") {
        Some(serde_json::Value::Number(n)) => n.as_i64() == Some(CODE_SESSION_EXPIRED),
        // 有的接口 code 是字符串
        Some(serde_json::Value::String(s)) => s == "90001",
        _ => false,
    }
}

/// 会话错误的人话解释。
///
/// `90001` 在读代码的人眼里容易被当成"这一号没登录"，而它实际说的是
/// **"被挡下了"** —— 可能是票不对、可能是票过期、也可能是别的。
/// 把它当成"没登录"会让人往错误方向查（去查账号，而问题在票）。
pub fn describe(code: i64) -> &'static str {
    match code {
        CODE_SESSION_EXPIRED => "会话被拒（票可能过期或不属于该页面）",
        _ => "未知错误",
    }
}

/// 构造一个带会话的业务请求。
///
/// 收在这里是为了让调用方不直接碰 `h5_headers` 的参数顺序 ——
/// mcloud 的教训：把两套凭据降级成同一个 `string` 参数后，
/// "摆错顺序"与"摆错变量"在编译器和测试眼里都合法。
pub fn with_jwt(
    params: session::RecipeParams,
    req: HttpRequest,
    referer: &str,
    jwt: &str,
) -> HttpRequest {
    session::h5_headers(req, params, referer, Some(jwt), "true")
}

/// 会话层错误：把"没有票"和"票被拒"分开。
///
/// 两者的人工处置不同：前者是配置/网络问题，后者通常意味着
/// "这个页面的配方不对"或"票过期了"。
impl From<SessionBuildError> for Error {
    fn from(e: SessionBuildError) -> Self {
        match e {
            SessionBuildError::NoSession => Error::Other("会话未建立".into()),
            SessionBuildError::Rejected(code) => {
                Error::Other(format!("会话被拒：code={code}（{}）", describe(code)))
            }
        }
    }
}

/// 建会话阶段的失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionBuildError {
    /// 三枪没跑完（网络失败已由上层转成 `Error`，这里是兜底）。
    NoSession,
    /// 上游明确回了一个拒绝码。
    Rejected(i64),
}

#[cfg(test)]
mod tests;
