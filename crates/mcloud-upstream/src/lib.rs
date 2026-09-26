//! mcloud-upstream —— 上游接口封装层。
//!
//! 本 crate 是**唯一**直接使用 `reqwest` 的地方。它负责：
//!
//! - 端点常量；
//! - 按域名注入凭证头（authorization / jwttoken / deviceid）；
//! - 把网络失败映射为 `mcloud_core::Error`。
//!
//! 业务层只看到 `mcloud_core::HttpClient`。

pub mod app_login;
pub mod app_signin;
pub mod auth;
pub mod auth_refresh;
pub mod client;
pub mod endpoints;
pub mod h5_client;
pub mod live_consts;
pub mod live_flower;
pub mod live_kouling;
pub mod live_msync;
pub mod live_room;
pub mod live_ws;
pub mod live_ws_client;
pub mod login;
pub mod session;
pub mod session_source;
pub mod smid;

pub use auth::{
    account_label, format_auth, mask_phone, parse_auth, APP_AUTH_PREFIX, PC_AUTH_PREFIX,
};
pub use auth_refresh::{
    apply_refresh, client_type_for, parse_refresh, rebuild_authorization, refresh_request,
    RefreshResult, APP_CLIENT_TYPE, PC_CLIENT_TYPE,
};
pub use client::{ClientProfile, ReqwestClient};
pub use endpoints::*;
pub use h5_client::H5Client;
pub use login::{
    LoginDataDecryptor, LoginEnvelope, LoginKind, LoginPayloadDecryptor, PcClient, PollOutcome,
    SendSmsOutcome, SliderPuzzle,
};
pub use session::{
    auth_fingerprint, basic_authorization, ensure_http_ok, h5_headers, parse_jwt_token,
    parse_server_timestamp, parse_sso_token, spec_token_request, timestamp_request,
    tyrz_login_request, H5Session, H5Signature, RecipeParams, SessionCache, SessionRecipe,
};
pub use session_source::{describe, is_session_expired, SessionSource};
pub use smid::{SmFingerprint, SmUpload};
