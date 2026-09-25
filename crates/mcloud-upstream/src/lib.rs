//! mcloud-upstream —— 上游接口封装层。
//!
//! 本 crate 是**唯一**直接使用 `reqwest` 的地方。它负责：
//!
//! - 端点常量；
//! - 按域名注入凭证头（authorization / jwttoken / deviceid）；
//! - 把网络失败映射为 `mcloud_core::Error`。
//!
//! 业务层只看到 `mcloud_core::HttpClient`。

pub mod client;
pub mod endpoints;

pub use client::{ClientProfile, ReqwestClient};
pub use endpoints::*;
