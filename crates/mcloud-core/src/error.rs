//! 统一错误类型。

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("配置错误：{0}")]
    Config(String),

    /// 凭证相关。`auth` / `token` 一律不外泄，只给原因。
    #[error("凭证无效：{0}")]
    Auth(String),

    #[error("凭证已过期")]
    AuthExpired,

    /// 网络层失败（连接、超时、TLS）。
    #[error("网络错误：{0}")]
    Network(String),

    /// 上游返回的业务错误。
    #[error("上游错误 [{code}]：{message}")]
    Upstream { code: String, message: String },

    /// 响应无法按预期解析。
    #[error("解析错误：{0}")]
    Parse(String),

    #[error("任务 {task} 执行失败：{reason}")]
    Task { task: String, reason: String },

    /// 请求的资源不存在。映射为 HTTP 404，而不是 500。
    #[error("资源不存在：{0}")]
    NotFound(String),

    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误：{0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// 上游返回的错误是否为「需要重新登录」。
    ///
    /// 调用方据此决定是否触发凭证刷新，而不是盲目重试。
    pub fn is_auth_failure(&self) -> bool {
        match self {
            Error::AuthExpired | Error::Auth(_) => true,
            Error::Upstream { code, .. } => {
                matches!(
                    code.as_str(),
                    // 上游在凭证失效时使用的几个码值
                    "1001" | "1002" | "401" | "AUTH_FAILED" | "TOKEN_EXPIRED"
                )
            }
            _ => false,
        }
    }
}
