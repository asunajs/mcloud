//! mcloud-tasks —— 各自动化任务的业务编排。
//!
//! 任务只依赖 `mcloud-core` 的 trait 与 `mcloud-engine` 的上下文，
//! 不直接碰网络或磁盘。

pub mod ai_avatar;
pub mod app_task;
pub mod app_task_matrix;
/// 今日AI豆（云朵记录汇总）—— 总览页那张卡的**只读**读数来源。
pub mod beans;
pub mod cloud_phone_redpack;
pub mod exchange;
pub mod fun_ai;
pub mod live_room;
pub mod login;
pub mod msg_push;
pub mod notice;
pub mod play_ai;
pub mod signin;
pub mod task_expansion;

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
/// # 兑换 `exchange`（**真扣AI豆**，2026-10-02 起**已注册**）
///
/// ⚠ 它**在**下面 `tasks` 里，也出现在 `/api/capabilities`。原先摘掉注册的三条理由
/// （缺 `device_id` 来源 / 缺 `visitlog` 预热 / 等待档与 TS 不一致）**都已补齐**：
///
/// 1. `device_id` 走**请求体**（路由那层调 `mcloud_app::smid::device_id` 取一个可用的 id，
///    库里有就不打网络）⇒ 链上包成一个常量 `DeviceIdFn`；
/// 2. `visitlog` **两枪**（`…_to_exchange` → `sleep(500ms)` → `…_exchange_times`），
///    顺序照 `mcloud-sign`；
/// 3. 等待档与 TS 对齐：任务层缺省 `skipWait = false`（等到 **hh:59:40** 预热，
///    再等到**整点 − 10ms** 发单）；而**用户点按钮那条路不等** —— 用户口径
///    「用户点按钮就不用等」，路由的缺省是 `true`。
///
/// 为什么这条与上面那些任务**性质不同**（这条理由**没变**）：签到族与小红花族都是
/// 「领」，失败了只是少领一次；这一条是「花」，下单那一枪**不可逆**（没有幂等键，
/// `oid` 由服务端生成，发出去就不收回来）。
///
/// 因此保留两道安全阀：没有 `deviceId` ⇒ **一枪都不发**（如实回 `device_id_failed`）；
/// `skipWait=false` 但没接上等待实现 ⇒ **如实挡下**，绝不静默往下走。
/// 哨兵是正向的：`tests::register_all_exposes_exchange`（"哪天有人把注册摘掉"时会红）。
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
        // ⭐ `exchange` 2026-10-02 起**在这里**：三条缺口已补齐（deviceId 来源
        // 走请求体 / visitlog 预热两枪 / 等待档与 TS 对齐），用户点「兑换」
        // 能真的走完整条链。哨兵：`tests::register_all_exposes_exchange`。
        Arc::new(exchange::ExchangeTask),
        Arc::new(msg_push::MsgPushTask),
        Arc::new(fun_ai::FunaiTask),
        Arc::new(notice::NoticeTask),
        Arc::new(play_ai::PlayAiTask),
        Arc::new(ai_avatar::AiAvatarTask::production()),
        Arc::new(cloud_phone_redpack::CloudPhoneRedpackTask::production()),
        Arc::new(app_task_matrix::AppTaskMatrixTask),
        Arc::new(app_task::AppTaskTask::new()),
        Arc::new(task_expansion::TaskExpansionTask::new()),
    ];
    for t in tasks {
        registry.register(t)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! 注册表哨兵：钉住"哪些任务对外宣称可用"。

    use super::*;

    /// ⭐ `exchange` **必须**出现在 `register_all` 里（2026-10-02 起）。
    ///
    /// `/api/capabilities` 与 `/api/runs` 都按注册表说话 ⇒ "注册了"就等于
    /// "对用户宣称这条能用"，所以这条哨兵盯的是**注册表与功能的说法一致**。
    ///
    /// ⚠ 这里原先叠着一条**正好相反**的旧哨兵（"`exchange` 不得出现"，理由是
    /// 缺 `deviceId` 来源 / `visitlog` 预热 / 等待档对齐，用户点了什么都不会发生）。
    /// 三条缺口补齐后它按自己的话红了，同一次改动里删掉了实现，**却把这整段注释留在原地**
    /// ⇒ 上下两段互斥的结论并排（2026-10-03 review §5 C2）。按"错记录就地修"的口径，
    /// 历史归并到这一段里，不再并排两份。
    ///
    /// 三条缺口补齐后（`deviceId` 来源 / `visitlog` 预热两枪 / 等待档与 TS 对齐），
    /// 用户点「兑换」能真的走完整条链 —— 所以它与"未注册"那版**正好相反**。
    /// 留一条正向哨兵，是为了"哪天有人把注册摘掉"时门禁会红，
    /// 而不是让一个功能静默消失（旧哨兵 `register_all_does_not_expose_exchange`
    /// 已在同一次改动里删掉 —— 它的使命就是红，现在红了，于是照它自己的话办）。
    #[test]
    fn register_all_exposes_exchange() {
        let mut registry = Registry::new();
        register_all(&mut registry).expect("注册不该重名");
        let names: Vec<String> = registry.list().iter().map(|m| m.name.clone()).collect();
        assert!(
            names.iter().any(|n| n == "exchange"),
            "exchange 应当已注册（三条缺口已补齐）；实际注册的有：{names:?}"
        );
    }
}
