//! reqwest 客户端，按域名注入凭证。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcloud_core::{Error, HttpRequest, HttpResponse, Result};

use crate::endpoints::{accepts_basic, accepts_jwt};

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

        builder = builder.header(
            "user-agent",
            self.profile.user_agent.as_deref().unwrap_or(DEFAULT_UA),
        );

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
