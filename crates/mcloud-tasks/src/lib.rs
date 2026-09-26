//! mcloud-tasks —— 各自动化任务的业务编排。
//!
//! 任务只依赖 `mcloud-core` 的 trait 与 `mcloud-engine` 的上下文，
//! 不直接碰网络或磁盘。

pub mod login;
pub mod sign;

use mcloud_engine::{Registry, Task};
use std::sync::Arc;

pub use login::{
    account_from_login, account_from_user_info_with_prefix, auth_from_user_info, logout,
    persist_login, production_decryptor, PasswordLogin, QrFailure, QrLogin, QrProgress, SmsLogin,
    QR_POLL_INTERVAL_MS, QR_TTL_SECONDS,
};

/// 注册全部任务。
///
/// 新增任务时在此处登记；重名会返回错误而不是静默覆盖。
pub fn register_all(registry: &mut Registry) -> Result<(), String> {
    let tasks: Vec<Arc<dyn Task>> = vec![Arc::new(sign::SignTask)];
    for t in tasks {
        registry.register(t)?;
    }
    Ok(())
}
