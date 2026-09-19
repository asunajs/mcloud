//! 错误分类。
//!
//! 分类的唯一目的是回答两个问题：**能不能重试**、**能不能把原文给用户看**。
//!
//! 对齐参考项目 caiyun 的两条约定：
//! - `exchange_request_control.go`：错误分类决定重试策略；
//! - `exchange_errors.go`：对外**绝不回传** `err.Error()`，走文案白名单。

use thiserror::Error;

/// 错误类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// 凭据失效 —— 不重试，标记该账号需要重新登录/刷新。
    Auth,
    /// 触发风控或限流 —— 退避后重试（并发过高时最常出现）。
    RateLimited,
    /// 奖品已兑完 —— 不重试。
    SoldOut,
    /// 云朵不足 —— 不重试。
    InsufficientCloud,
    /// 超时 —— 可重试。
    Timeout,
    /// 网络层错误 —— 可重试。
    Network,
    /// 上游 5xx —— 可重试。
    Upstream,
    /// 请求参数错误 —— 不重试。
    BadRequest,
    /// 资源不存在**或不属于当前用户** —— 不重试。
    ///
    /// 两者刻意共用一个类别（见 [`crate::user::is_visible_to`]）：
    /// "不属于你"若给出与"不存在"不同的错误，响应差异本身就泄漏了资源是否存在，
    /// 把 id 空间变成可枚举的。
    NotFound,
    /// 响应解析失败 —— 不重试（通常是上游改动，重试无意义）。
    Parse,
    /// 内部错误 —— 不重试。
    Internal,
}

impl ErrorKind {
    /// 是否可安全重试。
    ///
    /// 注意：**写请求**即使 `is_retryable() == true`，也必须带幂等键才能重试。
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Timeout | Self::Network | Self::Upstream
        )
    }

    /// 是否可以立即判定"该账号有硬问题"（用于健康度与告警）。
    pub const fn marks_account_unhealthy(self) -> bool {
        matches!(self, Self::Auth | Self::RateLimited)
    }

    /// 对外可见文案。
    ///
    /// 原文只进日志，**不进 API 响应**。
    pub const fn public_message(self) -> &'static str {
        match self {
            Self::Auth => "账号凭据已失效，请重新登录",
            Self::RateLimited => "触发限流，请降低并发后重试",
            Self::SoldOut => "奖品已兑完",
            Self::InsufficientCloud => "云朵不足",
            Self::Timeout => "请求超时",
            Self::Network => "网络异常",
            Self::Upstream => "上游服务异常",
            Self::BadRequest => "请求参数不合法",
            Self::NotFound => "资源不存在",
            Self::Parse => "响应解析失败",
            Self::Internal => "内部错误",
        }
    }
}

/// 按 HTTP 状态码分类。
///
/// 这是纯函数，重试策略完全由它决定，因此单测直接覆盖边界值。
pub const fn classify_status(status: u16) -> ErrorKind {
    match status {
        200..=299 => ErrorKind::Internal, // 成功不该走错误路径；落到 Internal 说明调用方判断有误
        401 | 403 => ErrorKind::Auth,
        408 => ErrorKind::Timeout,
        429 => ErrorKind::RateLimited,
        400 | 404 | 422 => ErrorKind::BadRequest,
        500..=599 => ErrorKind::Upstream,
        _ => ErrorKind::Internal,
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("凭据失效: {0}")]
    Auth(String),

    #[error("限流/风控: {0}")]
    RateLimited(String),

    #[error("已兑完: {0}")]
    SoldOut(String),

    #[error("云朵不足: {0}")]
    InsufficientCloud(String),

    #[error("超时: {0}")]
    Timeout(String),

    #[error("网络错误: {0}")]
    Network(String),

    #[error("上游错误 {status}: {message}")]
    Upstream { status: u16, message: String },

    #[error("参数错误: {0}")]
    BadRequest(String),

    /// 资源不存在或对当前用户不可见。**两种情形刻意不区分**，详见 [`ErrorKind::NotFound`]。
    #[error("不存在: {0}")]
    NotFound(String),

    #[error("解析失败: {0}")]
    Parse(String),

    #[error("内部错误: {0}")]
    Internal(String),

    /// fixture 未命中 —— 只可能出现在测试/调试台。
    ///
    /// 单独成类是为了让"偷偷打真网"立刻暴露而不是静默通过。
    #[error("fixture 未命中: {0}")]
    FixtureMiss(String),
}

impl Error {
    /// 错误正文 —— **不含**类别前缀。
    ///
    /// [`Display`](std::fmt::Display) 会给每条错误加上"凭据失效:" / "不存在:" 这类前缀：
    /// 那在**日志**里很有用（一眼看出类别），但直接端给使用者就很怪 ——
    /// 登录页上写"凭据失效: 用户名或口令不正确"，前半句是给运维看的。
    ///
    /// 所以要不要前缀是调用方的事，而这个能力必须显式提供：
    /// 不给的话，各处的解法会是 `split(": ")` 或者干脆把前缀一起发出去。
    pub fn message(&self) -> &str {
        match self {
            Self::Auth(m)
            | Self::RateLimited(m)
            | Self::SoldOut(m)
            | Self::InsufficientCloud(m)
            | Self::Timeout(m)
            | Self::Network(m)
            | Self::BadRequest(m)
            | Self::NotFound(m)
            | Self::Parse(m)
            | Self::Internal(m)
            | Self::FixtureMiss(m) => m,
            Self::Upstream { message, .. } => message,
        }
    }

    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Auth(_) => ErrorKind::Auth,
            Self::RateLimited(_) => ErrorKind::RateLimited,
            Self::SoldOut(_) => ErrorKind::SoldOut,
            Self::InsufficientCloud(_) => ErrorKind::InsufficientCloud,
            Self::Timeout(_) => ErrorKind::Timeout,
            Self::Network(_) => ErrorKind::Network,
            Self::Upstream { .. } => ErrorKind::Upstream,
            Self::BadRequest(_) => ErrorKind::BadRequest,
            Self::NotFound(_) => ErrorKind::NotFound,
            Self::Parse(_) => ErrorKind::Parse,
            Self::Internal(_) | Self::FixtureMiss(_) => ErrorKind::Internal,
        }
    }

    /// 可对外暴露的文案（不含任何原文细节）。
    pub fn public_message(&self) -> &'static str {
        self.kind().public_message()
    }
}

/// 从上游响应的业务码推断错误类别。
///
/// 上游用 200 + 业务码表达失败很常见，所以不能只看 HTTP 状态。
pub fn classify_business_code(code: i64) -> Option<ErrorKind> {
    match code {
        0 => None,
        // 各期活动/接口的"已抢完"码不统一，这里只收敛出语义明确的几类，
        // 其余返回 BadRequest，交由调用方按业务文案细化。
        _ => Some(ErrorKind::BadRequest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_errors_are_not_retryable() {
        assert!(!ErrorKind::Auth.is_retryable());
        assert!(ErrorKind::Auth.marks_account_unhealthy());
    }

    #[test]
    fn only_transient_kinds_are_retryable() {
        let retryable = [
            ErrorKind::RateLimited,
            ErrorKind::Timeout,
            ErrorKind::Network,
            ErrorKind::Upstream,
        ];
        for kind in retryable {
            assert!(kind.is_retryable(), "{kind:?} 应可重试");
        }

        let not_retryable = [
            ErrorKind::Auth,
            ErrorKind::SoldOut,
            ErrorKind::InsufficientCloud,
            ErrorKind::BadRequest,
            ErrorKind::Parse,
            ErrorKind::Internal,
        ];
        for kind in not_retryable {
            assert!(!kind.is_retryable(), "{kind:?} 不应重试");
        }
    }

    #[test]
    fn classify_status_covers_boundaries() {
        assert_eq!(classify_status(401), ErrorKind::Auth);
        assert_eq!(classify_status(403), ErrorKind::Auth);
        assert_eq!(classify_status(408), ErrorKind::Timeout);
        assert_eq!(classify_status(429), ErrorKind::RateLimited);
        assert_eq!(classify_status(400), ErrorKind::BadRequest);
        assert_eq!(classify_status(422), ErrorKind::BadRequest);
        assert_eq!(classify_status(500), ErrorKind::Upstream);
        assert_eq!(classify_status(503), ErrorKind::Upstream);
        assert_eq!(classify_status(599), ErrorKind::Upstream);
        assert_eq!(classify_status(600), ErrorKind::Internal);
    }

    #[test]
    fn public_message_never_leaks_detail() {
        let err = Error::Upstream {
            status: 502,
            message: "upstream said: auth=SECRET_TOKEN_abc123".into(),
        };
        let msg = err.public_message();
        assert!(!msg.contains("SECRET_TOKEN"), "对外文案泄漏了原文: {msg}");
        assert_eq!(msg, "上游服务异常");
    }

    #[test]
    fn business_code_zero_means_ok() {
        assert!(classify_business_code(0).is_none());
        assert_eq!(classify_business_code(-1), Some(ErrorKind::BadRequest));
    }

    /// `Display` 带类别前缀（给日志看），`message()` 不带（给使用者看）。
    #[test]
    fn message_is_free_of_the_kind_prefix() {
        let err = Error::Auth("用户名或口令不正确".into());
        assert_eq!(err.to_string(), "凭据失效: 用户名或口令不正确");
        assert_eq!(err.message(), "用户名或口令不正确");
        assert!(!err.message().contains("凭据失效"));

        // 带字段的变体也要能取到正文
        let upstream = Error::Upstream {
            status: 502,
            message: "坏网关".into(),
        };
        assert_eq!(upstream.message(), "坏网关");
    }
}
