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

/// 请求是否带体。
///
/// 注意 `body == Some("")` 也算带体（空串是**显式**的空体，与 `None`
/// 语义不同 —— 直播那两条 `give` / `draw` 靠 `None` 表达"无体"）。
fn has_body(req: &HttpRequest) -> bool {
    req.body.is_some()
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
    inner: reqwest::Client,
    profile: Arc<ClientProfile>,
}

impl ReqwestClient {
    pub fn new(profile: ClientProfile) -> Result<Self> {
        let timeout = if profile.timeout_secs == 0 {
            30
        } else {
            profile.timeout_secs
        };
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .cookie_store(true)
            .gzip(true)
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            inner,
            profile: Arc::new(profile),
        })
    }

    pub fn profile(&self) -> &ClientProfile {
        &self.profile
    }
}

#[async_trait]
impl mcloud_core::HttpClient for ReqwestClient {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        let url = reqwest::Url::parse(&req.url)
            .map_err(|e| Error::Network(format!("URL 非法 {}：{e}", req.url)))?;
        let host = url.host_str().unwrap_or("").to_string();

        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| Error::Network(format!("方法非法 {}：{e}", req.method)))?;

        let mut builder = self.inner.request(method, url);

        // 凭证按域名注入：这是安全边界，不能放宽
        if let Some(tok) = &self.profile.basic_token {
            if accepts_basic(&host) {
                builder = builder.header("authorization", tok);
            }
        }
        if let Some(jwt) = &self.profile.jwt_token {
            if accepts_jwt(&host) {
                builder = builder.header("jwttoken", jwt);
            }
        }
        if let Some(dev) = &self.profile.device_id {
            builder = builder.header("deviceid", dev);
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
        if let Some(pc) = &self.profile.pc {
            for (name, value) in pc.business_headers() {
                builder = builder.header(name, value);
            }
        }

        // UA 优先级：显式覆盖 > 平台 UA > 默认移动端（见 `effective_user_agent`）
        builder = builder.header("user-agent", self.profile.effective_user_agent());

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

        let resp = builder
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;

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
        for name in ["content-type", "Content-Type", "CONTENT-TYPE", "cOnTeNt-TyPe"] {
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
}
