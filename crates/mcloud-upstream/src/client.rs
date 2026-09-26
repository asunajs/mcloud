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

        if let Some(body) = req.body {
            builder = builder
                .header("content-type", "application/json;charset=UTF-8")
                .body(body);
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
}
