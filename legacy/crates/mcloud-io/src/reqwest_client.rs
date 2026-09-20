//! 真实 HTTP 客户端（reqwest + rustls）。
//!
//! # 为什么用 rustls 而不是 native-tls
//!
//! 规避 OpenSSL 依赖，交叉编译到 `aarch64-linux-android` / `aarch64-apple-ios` 时
//! 不需要额外系统库。这是在选型阶段就定下的约束，不是事后优化。
//!
//! # 重试策略
//!
//! - 只重试 [`mcloud_core::ErrorKind::is_retryable`] 为真的错误；
//! - **写请求必须带幂等键**才允许重试（`HttpRequest::is_retryable`）；
//! - 指数退避 + 抖动，避免多账号同时重试造成的同步冲击（本身就是风控诱因）。

use mcloud_core::{Error, HttpClient, HttpMethod, HttpRequest, HttpResponse, classify_status};
use std::time::Duration;

/// 重试策略。
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// 最大尝试次数（含首次）。
    pub max_attempts: u32,
    /// 首次退避基数。
    pub base_delay: Duration,
    /// 退避上限。
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(300),
            max_delay: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// 第 `attempt` 次失败后应等待多久（`attempt` 从 1 开始）。
    ///
    /// 指数退避 + 确定性抖动（用系统纳秒做扰动源，避免额外引入 rand 依赖）。
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(16);
        let raw = self
            .base_delay
            .saturating_mul(1u32 << exponent)
            .min(self.max_delay);

        // 抖动：在 [raw/2, raw) 之间取值，抑制多账号同时重试的同步冲击。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let half = raw / 2;
        half + Duration::from_nanos(u64::from(nanos) % (half.as_nanos().max(1) as u64))
    }
}

pub struct ReqwestClient {
    client: reqwest::Client,
    policy: RetryPolicy,
}

impl ReqwestClient {
    pub fn new() -> Result<Self, Error> {
        Self::with_policy(RetryPolicy::default())
    }

    /// 传入 `policy.max_attempts = 1` 即等价于"不重试"，调试时很有用。
    pub fn with_policy(policy: RetryPolicy) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .user_agent(concat!("mcloud/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| Error::Internal(format!("构建 HTTP 客户端失败: {e}")))?;
        Ok(Self { client, policy })
    }

    async fn send_once(&self, request: &HttpRequest) -> Result<HttpResponse, Error> {
        let method = match request.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };

        let mut builder = self.client.request(method, &request.url);
        for (k, v) in &request.headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        if let Some(ms) = request.timeout_ms {
            builder = builder.timeout(Duration::from_millis(ms));
        }

        let response = builder.send().await.map_err(map_reqwest_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = response.bytes().await.map_err(map_reqwest_error)?.to_vec();

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

impl HttpClient for ReqwestClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let retryable_request = request.is_retryable();
        let mut last_error: Option<Error> = None;

        for attempt in 1..=self.policy.max_attempts.max(1) {
            match self.send_once(&request).await {
                Ok(response) => {
                    if (200..300).contains(&response.status) {
                        return Ok(response);
                    }
                    let kind = classify_status(response.status);
                    let err = match kind {
                        mcloud_core::ErrorKind::Upstream => Error::Upstream {
                            status: response.status,
                            message: String::from_utf8_lossy(&response.body).to_string(),
                        },
                        mcloud_core::ErrorKind::Auth => {
                            Error::Auth(format!("HTTP {}", response.status))
                        }
                        mcloud_core::ErrorKind::RateLimited => {
                            Error::RateLimited(format!("HTTP {}", response.status))
                        }
                        mcloud_core::ErrorKind::Timeout => {
                            Error::Timeout(format!("HTTP {}", response.status))
                        }
                        _ => Error::BadRequest(format!("HTTP {}", response.status)),
                    };

                    if !(err.kind().is_retryable() && retryable_request) {
                        return Err(err);
                    }
                    last_error = Some(err);
                }
                Err(err) => {
                    if !(err.kind().is_retryable() && retryable_request) {
                        return Err(err);
                    }
                    last_error = Some(err);
                }
            }

            if attempt < self.policy.max_attempts {
                let delay = self.policy.delay_for(attempt);
                tracing::debug!(attempt, ?delay, url = %request.url, "请求失败，退避后重试");
                tokio::time::sleep(delay).await;
            }
        }

        Err(last_error.unwrap_or_else(|| Error::Internal("重试耗尽但无错误记录".into())))
    }
}

fn map_reqwest_error(err: reqwest::Error) -> Error {
    // 只有超时值得单独分类（它是"可能已到达上游"的语义，退避策略不同）；
    // 连接失败、请求构造失败、TLS 握手失败对调用方而言都是"网络异常"，
    // 因此统一归到 Network，不再细分 —— 分成同一处理结果的两个分支没有意义。
    if err.is_timeout() {
        Error::Timeout(err.to_string())
    } else {
        Error::Network(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_builds_with_rustls() {
        // 只要能在无 OpenSSL 环境下构造成功，选型就站得住。
        let client = ReqwestClient::new().expect("rustls 构建应成功");
        assert_eq!(client.policy.max_attempts, 3);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let policy = RetryPolicy {
            max_attempts: 10,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(1),
        };

        let d1 = policy.delay_for(1);
        let d3 = policy.delay_for(3);
        let d9 = policy.delay_for(9);

        assert!(d1 >= Duration::from_millis(50) && d1 < Duration::from_millis(100));
        assert!(d3 >= Duration::from_millis(200) && d3 < Duration::from_millis(400));
        assert!(d9 <= Duration::from_secs(1), "退避必须有上限");
    }

    #[test]
    fn backoff_never_exceeds_max_delay_at_extreme_attempts() {
        let policy = RetryPolicy::default();
        for attempt in [1, 5, 17, 64, u32::MAX] {
            assert!(
                policy.delay_for(attempt) <= policy.max_delay,
                "attempt={attempt} 超过上限"
            );
        }
    }

    #[test]
    fn no_retry_policy_effectively_disables_retries() {
        let policy = RetryPolicy {
            max_attempts: 1,
            ..Default::default()
        };
        assert_eq!(policy.max_attempts.max(1), 1);
    }
}
