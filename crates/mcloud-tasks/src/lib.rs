//! mcloud-tasks —— 各自动化任务的业务编排。
//!
//! 任务只依赖 `mcloud-core` 的 trait 与 `mcloud-engine` 的上下文，
//! 不直接碰网络或磁盘。

pub mod exchange;
pub mod fun_ai;
pub mod live_room;
pub mod login;
pub mod msg_push;
pub mod notice;
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
/// `live-room`（开门 → 获取口令 → 领取）与 `red-flower`（**直接领**给定口令）
/// 是**同一条六步链**的两个入口，所以一起登记。
///
/// ⚠ 六步链要的票是**云盘侧** `jwttoken`（与签到同族），**不是**直播侧 JWT ——
/// 拿后者去读实测回 `90001 未登录`。票由 `H5Client` 按主机透明注入，
/// 任务代码一行都不用碰。
///
/// # 兑换 `exchange`（**真扣AI豆**）
///
/// 它与上面十条**性质不同**：签到族与小红花族都是"领"，
/// 失败了只是少领一次；这一条是"花"，下单那一枪**不可逆**
/// （`mcloud_tasks::exchange` 开头那段解释过：这一族没有幂等键，
/// `oid` 由服务端生成，发出去就不收回来）。
///
/// ⚠ 所以 `ExchangeTask::run` 里 `device_id` 是**刻意的 `None`** ——
/// 那条链在没有 `deviceId` 时**一枪都不发**，如实回一句 `device_id_failed`。
/// 这不是缺陷，是"缺依据就停下来"：随手编一个 `deviceId` 会被上游不认、
/// 白扣一次机会，而**静默**才是真正的坏结果。
///
/// 真正的注入点在组装层（`mcloud_app` 已经依赖本 crate，方向是对的）：
/// 由它把"上报设备指纹换 id"那一套做成 `DeviceIdFn` 注进来。
/// 在那之前，本任务的定位是"判定层与编排层已经能跑、能测"，
/// 不是"能无人值守地花豆"。
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
        Arc::new(exchange::ExchangeTask),
        Arc::new(msg_push::MsgPushTask),
        Arc::new(fun_ai::FunaiTask),
        Arc::new(notice::NoticeTask),
    ];
    for t in tasks {
        registry.register(t)?;
    }
    Ok(())
}
