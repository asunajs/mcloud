//! reqwest 客户端，按域名注入凭证。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcloud_core::{Error, HttpRequest, HttpResponse, Result};

use crate::endpoints::{accepts_basic, accepts_jwt};
use crate::login::PcClient;

/// 客户端凭证配置。
#[derive(Debug, Clone, Default)]
pub struct ClientProfile {
    /// basic token（`authorization` 头的值）。
    pub basic_token: Option<String>,
    /// JWT token。
    pub jwt_token: Option<String>,
    /// 设备标识，注入所有请求。
    pub device_id: Option<String>,
    /// 覆盖 User-Agent。
    pub user_agent: Option<String>,
    /// 请求超时（秒）。
    pub timeout_secs: u64,
    /// **不跟随重定向**（`true` ⇒ `redirect::Policy::none()`）。
    ///
    /// ⚠ 字段名是**反的**、默认 `false`，这是故意的：`ClientProfile` 派生了 `Default`
    /// ⇒ `bool` 的默认值必然是 `false`，而"跟随重定向"正是**本仓一直以来的行为**
    ///（reqwest 默认 ≤10 跳）。若写成 `follow_redirects: bool`，默认值就成了
    /// **不跟随**，等于给全仓每个客户端都换了一次行为 ✗。
    ///
    /// 谁需要它：**推送**那个客户端 —— 地址是用户配的、还常把 token 写在 query 里，
    /// 跟随重定向等于把那枚 token 交给跳转目标（复审 `sv-8`）。
    pub no_redirects: bool,
    /// **冒充的 PC 客户端形态**（Windows / macOS）。
    ///
    /// # 为什么凭证配置里要有"形态"
    ///
    /// 上游把 Windows 与 macOS 当成两个不同的客户端：UA、设备串里的版本段、
    /// 渠道头都不一样。而这件事**只由账号落盘的那一列决定**
    /// （`Account::pc_platform`）—— 凭据本身分不出来，两种平台的
    /// `Authorization` 前缀都是 `pc`。
    ///
    /// 放在这里而不是让每个调用点各传一次平台：`HttpClientFactory` 只拿得到
    /// `&Account`，它构造客户端时读一次 `pc_platform` 就够了，
    /// 调用点再也碰不到"选平台"这件事 —— 选错平台的可能性被收敛到一处。
    ///
    /// `None` = 不是 PC 端账号（App / 历史数据），此时不发 PC 的设备头。
    pub pc: Option<PcClient>,
}

impl ClientProfile {
    /// 实际使用的 User-Agent。
    ///
    /// 优先级：显式覆盖 > 账号的平台 UA > 默认（移动端）。
    ///
    /// 把平台 UA 放在默认值之前，是因为"账号是 PC 端登录的"这件事
    /// 比"我们没特别指定"更强 —— 用 Android UA 去装 PC 账号，
    /// 会被上游按另一端的配额与风控处理。
    pub fn effective_user_agent(&self) -> &str {
        if let Some(ua) = self.user_agent.as_deref() {
            return ua;
        }
        if let Some(pc) = &self.pc {
            return pc.user_agent();
        }
        DEFAULT_UA
    }
}

/// 默认移动端 UA。
pub const DEFAULT_UA: &str = "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 \
(KHTML, like Gecko) Chrome/128.0.0.0 Mobile Safari/537.36";

/// 带体请求在调用方**未**声明 `content-type` 时补的默认值。
pub const CONTENT_TYPE_JSON: &str = "application/json;charset=UTF-8";

/// 请求是否带体 —— 转发到 [`HttpRequest::has_body`]，判据只有那一处。
///
/// ⚠ 别在这里重写一遍 `req.body.is_some()`：两处各写一份，
/// 早晚会有一份被"顺手补上 `!is_empty()`"而另一份没有。
fn has_body(req: &HttpRequest) -> bool {
    req.has_body()
}

/// 该给这个请求补什么 `content-type`（`None` = 不补）。
///
/// 抽成独立纯函数是为了**可测**：原来的写法埋在 `builder` 链里，
/// "会不会补出第二个头"这件事只能靠真发一个请求才看得见。
fn default_content_type(req: &HttpRequest) -> Option<&'static str> {
    if !has_body(req) {
        return None;
    }
    // ⚠ 大小写不敏感：`auth_refresh` 写 `Content-Type`、
    // `live_flower` 写 `content-type`，而 HTTP 头名不分大小写。
    let declared = req
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    if declared {
        None
    } else {
        Some(CONTENT_TYPE_JSON)
    }
}

/// 基于 reqwest 的客户端。
pub struct ReqwestClient {
    /// 默认后端（现已统一为 native-tls，见 Cargo.toml 的 reqwest 注释）。
    inner: reqwest::Client,
    /// 老式后端（`native-tls`）：只给 [`LEGACY_TLS_HOSTS`] 里的主机用。
    ///
    /// ⚠ 与 `inner` **共用同一套超时/cookie/gzip 设置**，差别只有 TLS 后端 ——
    /// 两个 builder 的配置必须同步改，否则"同一发请求换个主机就换了行为"。
    legacy: reqwest::Client,
    profile: Arc<ClientProfile>,
}

/// 把一次 `send()` 失败**说清楚**：分类 + 整条原因链。
///
/// # 为什么单独做这一个
///
/// 原先只报 `e.to_string()`，而那对 reqwest 只给出一句
/// `error sending request for url (…)` —— **恰好把唯一有用的信息丢了**。
/// 真机第一次跑 `live-room` 时卡在这里：
///
/// ```text
/// [门·换直播侧 JWT] 网络错误：error sending request for url
///   (https://v.cmcc-cs.cn/cmlive/api/user/auth/cmpassLoginByCmToken)
/// ```
///
/// 同一发用 `curl` 打是通的（HTTP 400 + `{"rtnMsg":"token入参不可为空"}`，
/// **连接没问题**）⇒ 说明是我们这边的事，而上面那句话答不出是
/// **连不上、TLS 没过、还是超时**。三者的修法完全不同。
///
/// reqwest 把真正的原因放在 `source()` 链里（`hyper` 的 connect 错误、
/// `rustls` 的握手错误等），`Display` **只印最外层**。这里把整条链串起来。
pub(crate) fn explain_send(e: &reqwest::Error) -> String {
    // 分类用 reqwest 自己的判据（比翻字符串可靠）
    let kind = if e.is_timeout() {
        "超时"
    } else if e.is_connect() {
        "连不上"
    } else if e.is_request() {
        "请求本身非法"
    } else if e.is_body() || e.is_decode() {
        "响应体读取失败"
    } else if e.is_redirect() {
        "重定向太多"
    } else {
        "未分类"
    };
    let mut out = format!("{kind}：{e}");
    // ⚠ 原因链要自己走 —— `Display` 只给最外层那一句
    let mut src = std::error::Error::source(e);
    let mut depth = 0;
    while let Some(inner) = src {
        depth += 1;
        if depth > 6 {
            out.push_str(" → …（更深的原因已截断）");
            break;
        }
        out.push_str(&format!(" → {inner}"));
        src = inner.source();
    }
    if depth == 0 {
        out.push_str(" →（没有更深的原因；URL 已在上文，检查 DNS 与代理）");
    }
    out
}

/// 只给**老式 TLS 套件**的上游主机（走 `native-tls` 而不是 rustls）。
///
/// # 为什么需要这一张表
///
/// 2026-09-29 实测：直播侧 `v.cmcc-cs.cn` **只给 CBC 套件**
/// `TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA`（`0xC013`，CBC + SHA-1），
/// 而 **rustls 出于安全考虑不实现任何 CBC 套件** ⇒ 双方零交集，
/// 表现在 TLS 层是一句 `HandshakeFailure`（alert 40）。
///
/// 三件已排除的（都实测过，别再重查）：
///
/// 1. **不是 TLS 版本** —— 强制 `max_protocol_version = TLS1.2` 后一模一样。
/// 2. **不是证书** —— 那会是 `UnknownCA` 那一类。
/// 3. **不是代理/假 IP** —— 绕过代理直连两个真实 IP（`117.159.206.181` /
///    `221.181.128.76`）复验：现代套件全 `HTTP 000`、CBC 那条 `HTTP 200`
///    ⇒ 是**源站**的事。
///
/// ⚠ 这张表要**尽量短**：多一台就多一处拿安全换连通的地方。
/// 加之前先确认"全站真的没有现代套件"，而不是"这一发没打通"。
pub const LEGACY_TLS_HOSTS: [&str; 1] = ["v.cmcc-cs.cn"];

/// 这台主机该不该走老式 TLS 后端。
pub fn needs_legacy_tls(host: &str) -> bool {
    LEGACY_TLS_HOSTS
        .iter()
        .any(|h| host.eq_ignore_ascii_case(h))
}

/// 从 URL 取主机（不含 scheme 与端口）。
fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    Some(host.split(':').next().unwrap_or(host))
}

/// 预签名 `PUT` 的总 deadline（10 分钟）。
///
/// ⚠ 为什么它与 API 那条 30s **不同**（2026-10-04 复审 `up-3`）：客户端建的时候
/// 那条 `.timeout(...)` 是 **reqwest 的总 deadline**（连接 + 写体 + 读响应**加起来**），
/// 对 API 调用正合适，但预签名上传那一枪传的是**整个文件的原始字节** ⇒
/// 文件大一点、或对方链路慢一点，就会在传到一半被掐断（`is_timeout` 那一档），
/// 表现是"上传失败"，而真正的原因是**我们给自己设的钟**。
///
/// 10 分钟是**宽到不像限制**的值：这一枪不该因为我们的钟而失败；
/// 真挂了有连接层的错误兜底。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// 这一发要不要用 [`UPLOAD_TIMEOUT`] 而不是客户端那条总 deadline。
///
/// 判据是 `PUT` —— 本仓上游层**只有一处**发 `PUT`：`app_cloud_upload::upload_put_request`
/// （预签名写入入口）。这条对应关系有钉子：`app_cloud_upload/tests.rs` 断言那一枪的
/// `method == "PUT"`。⇒ 看到 `PUT` 就是"在传文件"，不是"在调 API"，不会误伤。
fn presigned_upload_timeout(method: &reqwest::Method) -> Option<Duration> {
    (*method == reqwest::Method::PUT).then_some(UPLOAD_TIMEOUT)
}

impl ReqwestClient {
    pub fn new(profile: ClientProfile) -> Result<Self> {
        let timeout = if profile.timeout_secs == 0 {
            30
        } else {
            profile.timeout_secs
        };
        let no_redirects = profile.no_redirects;
        let base = || {
            let b = reqwest::Client::builder()
                .timeout(Duration::from_secs(timeout))
                .cookie_store(true)
                .gzip(true);
            // ⚠ 只有**明说要**的客户端才拒跟随（见 `ClientProfile::no_redirects` 上
            // 那段"字段名为什么是反的"）；缺省与一直以来的行为逐字相同 ✓。
            if no_redirects {
                b.redirect(reqwest::redirect::Policy::none())
            } else {
                b
            }
        };
        let inner = base().build().map_err(|e| Error::Network(e.to_string()))?;
        // 同一套配置，只把 TLS 后端换成 native-tls（见 LEGACY_TLS_HOSTS）
        let legacy = base()
            .use_native_tls()
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            inner,
            legacy,
            profile: Arc::new(profile),
        })
    }

    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }
}

/// 发送层要不要**补**这一枚凭证头。
///
/// # 为什么已经有一枚时**必须**不补
///
/// 补第二根不会"覆盖"，发出去就是**两根同名线**，而上游的反应是**直接拒**：
///
/// ```text
/// $ curl -H "Authorization: …" -H "authorization: …" … /user/querySpecTokenV2
/// HTTP 400  <html>…openresty…400 Bad Request</html>     ← APISIX 在业务层之前就拒了
/// ```
///
/// ⚠ 这个 400 **看起来像"网关/网络问题"**，与只带一根时的真实结论
/// （`HTTP 200 {"code":"05050009","message":"非法的token！"}`，即凭据过期）
/// 完全不是一回事 —— 排查方向会被整个带偏。这一条是实测钉出来的。
///
/// `host_allows` 是那条域名安全边界（[`accepts_basic`] / [`accepts_jwt`]）；
/// 它与"已经有一枚"是**两个独立**理由，任一不成立就不补。
pub fn should_inject_credential(
    existing: &[(String, String)],
    name: &str,
    host_allows: bool,
) -> bool {
    host_allows && !existing.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
}

impl ReqwestClient {
    /// 把一枪**准备好并发送**，返回 reqwest 的响应；**读响应体由调用方决定**
    /// （`text()` 还是 `bytes()`）。
    ///
    /// # 为什么拆出这一段
    ///
    /// 发送层现在有两条读体的路：[`mcloud_core::HttpClient::request`]（文本）与
    /// [`mcloud_core::HttpClient::request_raw`]（原样字节）。两条路的**前半段
    /// （按主机选 TLS 后端 / 注入凭据 / 补 content-type / 挂体）必须逐字相同**
    /// —— 各写一份，早晚有一份被改到，而表现是「同一个请求换个入口就少一枚头」，
    /// 排查方向会被整个带偏。所以准备与发送只做一次，只有读体分叉。
    async fn send(&self, req: HttpRequest) -> Result<reqwest::Response> {
        let url = reqwest::Url::parse(&req.url)
            .map_err(|e| Error::Network(format!("URL 非法 {}：{e}", req.url)))?;
        let host = url.host_str().unwrap_or("").to_string();

        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| Error::Network(format!("方法非法 {}：{e}", req.method)))?;
        // ⚠ 先算好：下一行会把 `method` move 走。
        // 见 `UPLOAD_TIMEOUT` —— 预签名 PUT 不该吃 API 那条 30s 总 deadline（复审 up-3）。
        let upload_timeout = presigned_upload_timeout(&method);

        // ⚠ 按**主机**选 TLS 后端：只有老式主机走 `legacy`（见 `LEGACY_TLS_HOSTS`）。
        // 判据是 URL 里那一段，不是"调用方是谁" —— 同一条链上可能混着两类主机
        // （直播链就是：小红花族走 m.mcloud、换 JWT 走 v.cmcc-cs.cn）。
        let client = match host_of(&req.url) {
            Some(h) if needs_legacy_tls(h) => &self.legacy,
            _ => &self.inner,
        };
        let mut builder = client.request(method, url);
        // 预签名 PUT：换成"宽到不像限制"的那条（reqwest 的**按请求** deadline
        // 会覆盖客户端那条总 deadline ✓ —— 见 `UPLOAD_TIMEOUT`）。
        if let Some(t) = upload_timeout {
            builder = builder.timeout(t);
        }

        // ⚠⚠ **调用方已经明确带了那一枚，就不再注入第二枚。**
        //
        // 这根线是**实测**钉出来的，不是洁癖：`querySpecTokenV2` 自己设了一枚
        // `authorization`（它要用账号凭据换 SSO，而那一刻客户端 profile 里
        // 还没有令牌），发送层又按域名注入一枚 ⇒ 发出去**两根同名头**。
        //
        // 后果**不是"取第一根/最后一根"，而是上游直接拒**：
        //
        //   $ curl -H "Authorization: …" -H "authorization: …" … /user/querySpecTokenV2
        //   HTTP 400  <html>…openresty…400 Bad Request</html>     ← APISIX 在业务层之前就拒了
        //
        // 而只带一根时同一发是 `HTTP 200 {"code":"05050009","message":"非法的token！"}`
        // —— 也就是说这个 400 **看起来像"网络/网关问题"**，与真正的结论
        // （凭据过期）完全不是一回事，排查方向会被整个带偏。
        //
        // 同理适用于 `jwttoken` 与 `deviceid`：三者的规则都是
        // "发送层补**缺**，不覆盖调用方显式给的那一枚"。
        // 凭证按域名注入：这是安全边界，不能放宽。
        // ⚠ 已经有一枚同名的就**不补** —— 理由见 [`should_inject_credential`]。
        if let Some(tok) = &self.profile.basic_token {
            if should_inject_credential(&req.headers, "authorization", accepts_basic(&host)) {
                builder = builder.header("authorization", tok);
            }
        }
        if let Some(jwt) = &self.profile.jwt_token {
            if should_inject_credential(&req.headers, "jwttoken", accepts_jwt(&host)) {
                builder = builder.header("jwttoken", jwt);
            }
        }
        if let Some(dev) = &self.profile.device_id {
            if should_inject_credential(&req.headers, "deviceid", true) {
                builder = builder.header("deviceid", dev);
            }
        }

        // PC 形态的设备头：账号是 PC 端登录的就必须带上。
        //
        // 这不是装饰：实测只带 `x-yun-tid` 调登录接口会被回 `9999 登录失败！`，
        // 补上这几个头才是正常业务码（见 `login::PcClient::headers` 的说明）。
        //
        // 业务请求多带一个 `x-yun-app-channel`：它属于客户端
        // `cutoverStatus == 1` 那一套**登录后**的头，而这里是已登录路径。
        // 登录请求本身只能发那 5 个（`login::PcClient::headers`），
        // 两套混起来的话"登录请求多带一个头"这种错误不会报错、只会静默。
        // ⚠⚠ 这两处**必须先查重**（2026-10-03 review §1.10）：
        // `reqwest` 的 `RequestBuilder::header` 是 **append**，不是 set ——
        // 调用方自己在 `req.headers` 里带了同名头时，这里再补一枚就是
        // **两根同名线**，与凭据那一族（`authorization`/`jwttoken`/`deviceid`，
        // 见 `should_inject_credential`）**完全同一个坑**。
        // 实测踩中的两支：`smid::device_profile_request` 自己带 `user-agent`
        // （`smid.rs:687`），以及 PC 那套 `x-yun-*` 业务头。
        // 口径同凭据：**调用方声明的赢**。
        if let Some(pc) = &self.profile.pc {
            for (name, value) in pc.business_headers() {
                if should_inject_credential(&req.headers, name, true) {
                    builder = builder.header(name, value);
                }
            }
        }

        // UA 优先级：显式覆盖 > 平台 UA > 默认移动端（见 `effective_user_agent`）
        if should_inject_credential(&req.headers, "user-agent", true) {
            builder = builder.header("user-agent", self.profile.effective_user_agent());
        }

        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }

        // ⚠⚠ 只在调用方**没有**声明 `content-type` 时才补默认值。
        //
        // 这里原先是无条件补 `application/json;charset=UTF-8`，于是**任何**
        // 自带 content-type 的非 JSON 请求上线都会带**两个** `content-type`
        // （调用方那个在前、这里补的在后）。实测（本地端口抓原始报文）：
        //
        //     content-type: application/x-www-form-urlencoded;charset=UTF-8
        //     content-type: application/json;charset=UTF-8      ← 多出来的
        //     content-length: 107
        //
        // 这属于本仓最忌讳的那类错：**不发错、不报错，只是行为不对**。
        // 受害的是 `live_flower` 的 `visitlog/journaling`（form-urlencoded）
        // 与 `auth_refresh`（自带 `Content-Type`）—— 它们声明得都对，
        // 却消不掉第二个头。
        //
        // 判据用 `req.headers`（调用方显式给的那一份），**不是** `builder`：
        // reqwest 的 `RequestBuilder` 不暴露已设置的头，读不到就只能靠来源判断。
        // ⚠ 先算判据、再移动 `body`：`default_content_type` 借的是
        // `req`（要看 headers 是否已声明），而 `req.body` 是按值取走的
        // —— 顺序反了就是 `borrow of partially moved value`。
        if let Some(ct) = default_content_type(&req) {
            builder = builder.header("content-type", ct);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }

        builder
            .send()
            .await
            .map_err(|e| Error::Network(explain_send(&e)))
    }
}

#[async_trait]
impl mcloud_core::HttpClient for ReqwestClient {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        let resp = self.send(req).await?;

        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = resp
            .text()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;

        Ok(HttpResponse {
            status,
            body,
            headers,
        })
    }

    /// ⚠⚠ 这一条**必须**由 leaf 覆写：默认实现走 `self.request()`，而那条路已经
    /// 过了 `reqwest::Response::text()`（**有损** UTF-8）⇒ 拿不到线上字节。
    ///
    /// 用的是 `resp.bytes()`（**原样字节**），**不是** `text()`。AI 头像那条链
    /// 把下载到的合成图 base64 之后 upload，走错这一步的后果是：内容被替换字符
    /// 破坏、上游却仍可能回 `code:0` ⇒ 网盘里是一张打不开的图，**且不报错**。
    ///
    /// ⚠ 这一条只负责「线上是什么字节就回什么字节」；它**不判**这些字节是不是
    /// 一张合法的图 —— 形状校验在下游（见 `mcloud_tasks::ai_avatar`）。
    async fn request_raw(&self, req: HttpRequest) -> Result<Vec<u8>> {
        let resp = self.send(req).await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::PcPlatform;

    fn pc(p: PcPlatform) -> PcClient {
        PcClient::new(p, "DEV-ENDIN")
    }

    /// UA 三级优先：显式覆盖 > 平台 UA > 默认。
    ///
    /// 这条决定"账号是 PC 端登录的"会不会真的反映到每一次请求上 ——
    /// 若只覆盖登录那一次，后续业务请求又会退回 Android UA。
    #[test]
    fn user_agent_precedence_prefers_override_then_platform_then_default() {
        // 什么都没有 → 默认移动端
        let bare = ClientProfile::default();
        assert_eq!(bare.effective_user_agent(), DEFAULT_UA);

        // 只有平台 → 平台 UA
        let win = ClientProfile {
            pc: Some(pc(PcPlatform::Windows)),
            ..Default::default()
        };
        assert!(win.effective_user_agent().contains("Windows NT"));
        assert_ne!(win.effective_user_agent(), DEFAULT_UA);

        // 显式覆盖压过平台
        let forced = ClientProfile {
            pc: Some(pc(PcPlatform::MacOs)),
            user_agent: Some("custom-agent".into()),
            ..Default::default()
        };
        assert_eq!(forced.effective_user_agent(), "custom-agent");
    }

    /// mac 与 win 的 profile 解析出的 UA 必须不同（底层区分的落点）。
    #[test]
    fn platform_profile_yields_different_user_agents() {
        let w = ClientProfile {
            pc: Some(pc(PcPlatform::Windows)),
            ..Default::default()
        };
        let m = ClientProfile {
            pc: Some(pc(PcPlatform::MacOs)),
            ..Default::default()
        };
        assert_ne!(w.effective_user_agent(), m.effective_user_agent());
        assert!(m.effective_user_agent().contains("Macintosh"));
    }

    /// 非 PC 账号（`pc: None`）不该带 PC 的设备头。
    ///
    /// 这条防的是"把 PC 头撒到 App 账号上"—— 那会让上游按另一端的
    /// 配额与风控处理，现象是莫名其妙的业务码。
    #[test]
    fn non_pc_profile_has_no_platform_headers() {
        let profile = ClientProfile::default();
        assert!(profile.pc.is_none());
        assert_eq!(profile.effective_user_agent(), DEFAULT_UA);
    }

    // ── content-type 去重（回归测试）──────────────────────────────────────

    /// 调用方**没**声明 content-type 时，才补默认的 JSON。
    ///
    /// 这是这条规则的**主路径**：绝大多数请求（签到、小红花那五条 JSON）
    /// 都靠它拿头，所以"去重"不能把默认值也一起去掉。
    #[test]
    fn json_content_type_is_added_when_absent() {
        // ⚠ `HttpRequest::post` 建出来的 `body` 是 `None`（见 core::traits），
        // 所以这里必须显式挂一个体 —— 否则测的是"无体请求"，走的是另一条分支。
        let req = HttpRequest::post("https://example.com/x").text_body("{}");
        assert_eq!(default_content_type(&req), Some(CONTENT_TYPE_JSON));
    }

    /// ⚠ 调用方**已**声明 content-type 时，**不再补** —— 否则上线两个头。
    ///
    /// 这是实测过的缺陷（本地端口抓原始报文，`journaling` 收到两个
    /// `content-type`）。受害的正是**非 JSON** 的那些请求：
    /// `visitlog/journaling`（form-urlencoded）与 `auth_refresh`（自带 JSON 头）。
    ///
    /// ⚠ 大小写两种写法都要认：本仓两处调用方分别写的是
    /// `Content-Type`（`auth_refresh`）与 `content-type`（`live_flower`），
    /// 而 HTTP 头名**大小写不敏感** —— 按字节比会漏掉前者。
    #[test]
    fn declared_content_type_is_never_overridden() {
        for name in [
            "content-type",
            "Content-Type",
            "CONTENT-TYPE",
            "cOnTeNt-TyPe",
        ] {
            let req = HttpRequest::post("https://example.com/x")
                .header(name, "application/x-www-form-urlencoded;charset=UTF-8")
                .text_body("module=uservisit");
            assert_eq!(
                default_content_type(&req),
                None,
                "调用方已用 {name} 声明，不该再补一个 —— 那会上线两个 content-type"
            );
        }
    }

    /// 不带 body 的请求（GET）**不加** content-type。
    ///
    /// `queryRevivalFlowers` 是 GET；给它补一个 content-type 声明了一个
    /// 并不存在的体，是另一种形式的失真。
    #[test]
    fn bodyless_requests_get_no_content_type() {
        let get = HttpRequest::get("https://example.com/x");
        assert!(!has_body(&get));
        assert_eq!(default_content_type(&get), None);
    }

    /// 去重判据只看 `content-type`，**不能**被别的头误伤。
    ///
    /// 例如 `Accept: */*`、`activityid: sign_in_3` 都在这一族里出现过：
    /// 把它们误认成 content-type 会让默认头时而加、时而不加。
    #[test]
    fn other_headers_do_not_suppress_the_default() {
        let req = HttpRequest::post("https://example.com/x")
            .header("Accept", "*/*")
            .header("activityid", "sign_in_3")
            .header("x-content-type-options", "nosniff")
            .text_body("{}");
        assert_eq!(
            default_content_type(&req),
            Some(CONTENT_TYPE_JSON),
            "只有真正的 content-type 才算已声明"
        );
    }

    /// 调用方显式给过的凭证头，发送层**一格都不许再补**。
    ///
    /// ⭐ 这条是实测 400 的根因：`querySpecTokenV2` 自己带了一枚 `authorization`
    /// （那一刻 profile 里还没有令牌），发送层又按域名补一枚 ⇒ 两根同名线
    /// ⇒ APISIX 回 400，**看起来像网关故障**，而真相是另一回事。
    #[test]
    fn an_explicit_credential_header_is_never_doubled() {
        let given = vec![("Authorization".to_string(), "Basic abc".to_string())];
        // 大小写不敏感（HTTP 头就是不分大小写）
        assert!(!should_inject_credential(&given, "authorization", true));
        assert!(!should_inject_credential(&given, "AUTHORIZATION", true));

        // 没有同名 ⇒ 该补
        assert!(should_inject_credential(&[], "authorization", true));
        assert!(should_inject_credential(&[], "jwttoken", true));

        // 域名边界不成立 ⇒ 不补（与"已经有一枚"是两个独立理由）
        assert!(!should_inject_credential(&[], "authorization", false));

        // 三者互不干扰：带了 authorization 不该挡住 jwttoken
        assert!(should_inject_credential(&given, "jwttoken", true));
        assert!(should_inject_credential(&given, "deviceid", true));
    }

    /// ⚠⚠ 复审 `up-3`：预签名上传那一枪不能被 API 那条 30s 总 deadline 掐断。
    ///
    /// 这是**结构性**钉子：真正的"线上有没有被掐断"要拿一个慢/大的文件真传才验得到
    /// （本仓传的是小文件 ⇒ 从没触发过），所以这里钉**策略**本身 ——
    /// `PUT`（= 唯一那处预签名上传）⇒ 更宽的 deadline；其余 ⇒ 不覆盖。
    ///
    /// 红测：把 `presigned_upload_timeout` 改成恒 `None`（等于撤掉 `send` 里那次
    /// `.timeout(..)`）⇒ 第一条断言就被 `expect` 打红。
    #[test]
    fn the_presigned_upload_gets_a_wider_deadline_than_api_calls() {
        // 与 `ReqwestClient::new` 的缺省一致（`profile.timeout_secs == 0` ⇒ 30）
        let api = Duration::from_secs(30);

        let put = presigned_upload_timeout(&reqwest::Method::PUT)
            .expect("预签名 PUT 必须换成更宽的 deadline");
        assert_eq!(put, UPLOAD_TIMEOUT);
        assert!(put > api, "要**宽**于 API 那条：实得 {put:?} vs {api:?}");

        for m in [reqwest::Method::GET, reqwest::Method::POST] {
            assert!(
                presigned_upload_timeout(&m).is_none(),
                "{m} 不该改 deadline：API 调用要的就是那条总 deadline"
            );
        }
    }
}

#[cfg(test)]
mod redirect_policy_tests {
    use super::*;

    /// ⚠⚠ 复审 `sv-8`：`no_redirects` 的**默认必须是 `false`**（= 跟随）。
    ///
    /// 这条钉的是一个很容易踩的坑：字段是"反着命名"的 `bool`（见它上面那段说明），
    /// 谁哪天顺手把它改成 `follow_redirects`，`Default` 一算就变成"全仓默认不跟随"
    /// ✗ —— 那是给每个客户端都换了一次行为，而且**不会有任何测试变红**（除非有这一条）。
    #[test]
    fn following_redirects_stays_the_default() {
        assert!(
            !ClientProfile::default().no_redirects,
            "默认必须仍然是跟随重定向（`no_redirects` 是反名字段，默认 false）"
        );
        // 反过来也要能开：推送那条路会显式设 true
        let p = ClientProfile {
            no_redirects: true,
            ..Default::default()
        };
        assert!(p.no_redirects);
    }
}
