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

    /// 触发限速。映射为 HTTP **429** + `Retry-After`。
    ///
    /// # 为什么做成错误变体，而不是让每个 handler 自己拼一个 429 响应
    ///
    /// handler 的返回类型有两种：`Result<Response, ApiError>`（认证面那几条）
    /// 与 `ApiResult<T>`（= `Result<Json<T>, ApiError>`）。后者**塞不进**一个裸
    /// `Response` —— 于是"限速"这件事要么在每个 handler 里各写一遍、
    /// 要么表达成一个错误。错误变体只写一处，两种返回类型都装得下 ✓。
    ///
    /// `what` 是"什么过于频繁"（"尝试" / "操作"），`retry_after_secs` 会同时进
    /// 文案与 `Retry-After` 头（在 `mcloud-server` 的 `ApiError::into_response` 里）。
    #[error("{what}过于频繁，请 {retry_after_secs} 秒后再试")]
    RateLimited { what: String, retry_after_secs: i64 },

    /// **已登录，但这件事不该由他做**（实例级动作只有管理员能做）。映射为 HTTP 403。
    ///
    /// # 为什么不复用 [`Error::Auth`]（安全审计 SEC-07）
    ///
    /// `Auth` 映射成 401，而前端见到 401 的约定动作是**弹登录框** ——
    /// 于是一个"登录完全正常、只是不是管理员"的用户会看到自己被登出，
    /// 以为会话坏了，反复重登 ✗。403 才是这件事的准确说法：
    /// **身份没问题，权限不够** ✓。
    ///
    /// 也不复用 `NotFound`：那是"别泄露存在性"的场合用的（别人的账号、
    /// 别人的运行记录），而这里路由本身是公开已知的，藏它没有意义、
    /// 只会让排障的人多猜一轮 ✗。
    #[error("没有权限：{0}")]
    Forbidden(String),

    /// 任务被主动中止。
    ///
    /// 与 `Task { .. }` 分开，是因为"中止"**不是失败** ——
    /// 它不该计入错误率、不该触发告警、推送里也不该报红。
    #[error("任务已中止")]
    Cancelled,

    /// 存储层错误（读写失败、格式损坏）。
    #[error("存储错误：{0}")]
    Storage(String),

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
    /// 是否为"用户主动中止"。用于决定要不要报红/告警。
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Error::Cancelled)
    }

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
