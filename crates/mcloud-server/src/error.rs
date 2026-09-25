//! HTTP 错误映射。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// API 错误包装。
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
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // 错误文本经过脱敏，避免凭证随错误信息外泄
        let msg = mcloud_core::event::redact(&self.0.to_string());
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;
