//! mcloud-tasks —— 各自动化任务的业务编排。
//!
//! 任务只依赖 `mcloud-core` 的 trait 与 `mcloud-engine` 的上下文，
//! 不直接碰网络或磁盘。

pub mod live_room;
pub mod login;
pub mod signin;

use mcloud_engine::{Registry, Task};
use std::sync::Arc;

pub use login::{
    account_from_app_login, account_from_login, account_from_user_info_with_prefix,
    auth_from_user_info, logout, persist_login, production_app_decryptor, production_decryptor,
    AppSmsLogin, PasswordLogin, QrFailure, QrLogin, QrProgress, SmsLogin, QR_POLL_INTERVAL_MS,
    QR_TTL_SECONDS,
};

/// 注册全部任务。
///
/// 新增任务时在此处登记；重名会返回错误而不是静默覆盖。
///
/// # P1 签到族八条
///
/// 它们共用**同一条 H5 会话**（`SessionRecipe::SignInPage`），
/// 所以一起登记 —— 参考实现里这就是一层装配。
/// 顺序按"日常会跑"的程度排，`/api/capabilities` 会按名字再排一次序。
///
/// # 直播小红花两条
///
/// `live-room`（开门 → 收口令 → 投）与 `red-flower`（**直接投**给定口令）
/// 是**同一条六步链**的两个入口，所以一起登记。
///
/// ⚠ 六步链要的票是**云盘侧** `jwttoken`（与签到同族），**不是**直播侧 JWT ——
/// 拿后者去读实测回 `90001 未登录`。票由 `H5Client` 按主机透明注入，
/// 任务代码一行都不用碰。
pub fn register_all(registry: &mut Registry) -> Result<(), String> {
    let tasks: Vec<Arc<dyn Task>> = vec![
        Arc::new(signin::SignTask),
        Arc::new(signin::ReceiveTask),
        Arc::new(signin::MultipleTask),
        Arc::new(signin::BackupGiftTask),
        Arc::new(signin::RevivalTask),
        Arc::new(signin::PrizesTask),
        Arc::new(signin::CloudSnapshotTask),
        Arc::new(signin::TaskListTask),
        Arc::new(live_room::LiveRoomTask::production()),
        Arc::new(live_room::RedFlowerTask::production()),
    ];
    for t in tasks {
        registry.register(t)?;
    }
    Ok(())
}
