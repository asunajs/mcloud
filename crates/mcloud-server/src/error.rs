//! 领域错误 → HTTP 响应。
//!
//! # 为什么要有这一层
//!
//! 存储层返回的是 `mcloud_core::Error`（带 [`mcloud_core::ErrorKind`]），
//! 而 HTTP 要的是**状态码 + 稳定的响应体形状**。把映射写在一个地方，
//! 前端才可能只认一套形状；散在每个处理器里，就会出现
//! "同一个错误这类接口返 400、那类接口返 500"。
//!
//! # 文案的两条规矩（沿用 `ErrorKind::public_message` 的约定）
//!
//! - **4xx 用原文**：这些文案是我们自己写给使用者的（"口令至少 8 位"、
//!   "系统里已经有 1 个用户了"），删掉限定词只剩"请求参数不合法"，使用者没法据此改。
//! - **5xx 用类别文案**：原文常常带着路径、SQL 片段或上游返回体 ——
//!   它只进 `tracing`，响应里给 [`ErrorKind::public_message`] 的类别文案。
//!
//! 这条分界不是洁癖：4xx 的文案是我们**故意**写给对方看的，
//! 5xx 的文案是出了意外时顺手 `format!` 出来的。

use axum::Json;
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use mcloud_core::{Error, ErrorKind};
use serde::de::DeserializeOwned;
use serde_json::json;

/// 请求体提取器：**所有**请求体错误都变成我们自己的形状。
///
/// # 为什么不能直接用 `axum::Json`
///
/// 直接用的话，"缺字段"与"不是合法 JSON"这两类会被 axum 自己拦下，
/// 返回它自带的 422 **纯文本**（`Failed to deserialize the JSON body into the target type: ...`）。
/// 前端是按 `{code, message}` 写的错误处理，拿到纯文本就只能显示"HTTP 422" ——
/// 用户看不出到底缺了什么。
///
/// 这一层把拒绝理由**原样带上**（它说的就是"缺 password 字段"，正好可行动），
/// 归到 [`ApiError::BadBody`]（400）。用 400 而不是 422：这个前端不需要区分两者，
/// 而"参数不合法"与"语义没错但状态不对（409）"才是它真正要分的。
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    Json<T>: FromRequest<S, Rejection = axum::extract::rejection::JsonRejection>,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::BadBody(rejection.body_text())),
        }
    }
}

/// 查询串提取器：与 [`ApiJson`] 同一个理由，只不过对象是 `?a=1` 而不是请求体。
///
/// 直接用 `axum::extract::Query` 时，`?days=abc` 会得到 axum 自带的 **400 纯文本**
/// （`Failed to deserialize query string: ...`）。前端是按 `{code, message}` 写的，
/// 拿到纯文本只能显示"HTTP 400" —— 而这里丢掉的恰恰是"days 得是个数字"这条可行动的信息。
pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|rejection| ApiError::BadBody(rejection.body_text()))
    }
}

/// 接口错误。
#[derive(Debug)]
pub enum ApiError {
    /// 领域层错误，按 [`ErrorKind`] 映射状态码。
    Domain(Error),
    /// 未登录 / 会话无效。
    ///
    /// **不区分**"没带 cookie"与"cookie 无效/已撤销"：两者该导出同一个动作 —— 去登录。
    Unauthorized,
    /// 还没初始化首个管理员：此时除初始化接口外一律不响应（方案 §3.7）。
    SetupRequired,
    /// 请求体不是合法 JSON，或字段不符合契约。
    BadBody(String),
}

impl ApiError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::SetupRequired => (StatusCode::SERVICE_UNAVAILABLE, "setup_required"),
            Self::BadBody(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Domain(err) => match err.kind() {
                ErrorKind::Auth => (StatusCode::UNAUTHORIZED, "unauthorized"),
                ErrorKind::NotFound => (StatusCode::NOT_FOUND, "not_found"),
                ErrorKind::BadRequest => (StatusCode::BAD_REQUEST, "bad_request"),
                ErrorKind::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
                // 业务冲突：不是"参数写错了"，而是"当前状态下做不了" ——
                // 用 409 让前端能把它与 400 区分开（前者可以重试/换时间，后者不能）
                ErrorKind::SoldOut | ErrorKind::InsufficientCloud => {
                    (StatusCode::CONFLICT, "conflict")
                }
                // 错不在这个请求，也不在我们内部：上游/网络问题一律 502
                ErrorKind::Timeout | ErrorKind::Network | ErrorKind::Upstream => {
                    (StatusCode::BAD_GATEWAY, "upstream")
                }
                // Parse / Internal：真正的 5xx。
                // 用通配而不是逐个列：新增一种 `ErrorKind` 时**默认落 500**，
                // 而不是让编译器逼着人给它挑一个"看起来合适"的 4xx。
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            },
        }
    }
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        Self::Domain(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();

        let message = match &self {
            // 用 `message()` 而不是 `to_string()`：后者带"凭据失效:"这类类别前缀，
            // 那是给日志看的，端给使用者就成了"凭据失效: 用户名或口令不正确"
            Self::Domain(err) if status.is_client_error() => err.message().to_string(),
            Self::Domain(err) => {
                tracing::error!(kind = ?err.kind(), error = %err, "接口错误");
                err.kind().public_message().to_string()
            }
            Self::Unauthorized => "请先登录".to_string(),
            Self::SetupRequired => "还没初始化首个管理员，请先完成初始化".to_string(),
            Self::BadBody(detail) => detail.clone(),
        };

        (status, Json(json!({ "code": code, "message": message }))).into_response()
    }
}
