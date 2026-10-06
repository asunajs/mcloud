//! HTTP 错误映射。

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

/// API 错误包装。
///
/// 派生 `Debug` 是为了让调用方能 `unwrap()`/`expect()` ——
/// 没有它，`Result<T, ApiError>` 在测试里连 `unwrap` 都用不了，
/// 而那是测试里最常见的写法。派生出来的 Debug 打印的是**内层错误**，
/// 与 `Display` 走同一份 `redact` 逻辑无关：内层 `mcloud_core::Error`
/// 自己的 Debug 不脱敏，所以**不要**拿它去打日志。
#[derive(Debug)]
pub struct ApiError(pub mcloud_core::Error);

impl From<mcloud_core::Error> for ApiError {
    fn from(e: mcloud_core::Error) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            mcloud_core::Error::Config(_) | mcloud_core::Error::Parse(_) => StatusCode::BAD_REQUEST,
            mcloud_core::Error::Auth(_) | mcloud_core::Error::AuthExpired => {
                StatusCode::UNAUTHORIZED
            }
            mcloud_core::Error::Network(_) => StatusCode::BAD_GATEWAY,
            mcloud_core::Error::NotFound(_) => StatusCode::NOT_FOUND,
            // ⚠ 403 而不是 401（安全审计 SEC-07）：401 会让前端弹登录框，
            // 而"登录是好的、只是权限不够"不该被显示成掉线 ✗
            mcloud_core::Error::Forbidden(_) => StatusCode::FORBIDDEN,
            // ⚠ 429（安全审计 SEC-02 / SEC-08）：这个码前端认，会退避重试
            mcloud_core::Error::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // 错误文本经过脱敏，避免凭证随错误信息外泄
        let msg = mcloud_core::event::redact(&self.0.to_string());
        let mut resp = (status, Json(serde_json::json!({ "error": msg }))).into_response();
        // ⚠ 限速要带 `Retry-After`：没有它，客户端只能瞎猜该等多久 ——
        // 而"猜太短"的表现就是它立刻再撞一次，等于没退避 ✗。
        if let mcloud_core::Error::RateLimited {
            retry_after_secs, ..
        } = &self.0
        {
            if let Ok(v) = retry_after_secs.to_string().parse() {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;
