//! mcloud-tasks —— 各自动化任务的业务编排。
//!
//! 任务只依赖 `mcloud-core` 的 trait 与 `mcloud-engine` 的上下文，
//! 不直接碰网络或磁盘。

pub mod ai_avatar;
pub mod app_task;
pub mod app_task_matrix;
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
/// # 兑换 `exchange`（**真扣AI豆**，当前**未**注册）
///
/// ⚠ 它**不在**下面 `tasks` 里，也不出现在 `/api/capabilities`：
/// **未注册：缺 `device_id` 来源 + 缺 `visitlog` 预热 + 等待档与 TS 不一致，
/// 补齐后才应放出来。** 三条缺口逐条记在 `mcloud_tasks::exchange` 的
/// `ExchangeTask` 文档里。
///
/// 为什么宁可不给入口：这一族与上面十条**性质不同** —— 签到族与小红花族
/// 都是"领"，失败了只是少领一次；这一条是"花"，下单那一枪**不可逆**
/// （没有幂等键，`oid` 由服务端生成，发出去就不收回来）。而
/// `ExchangeTask::run` 里 `device_id` 是**刻意的 `None`**：那条链在没有
/// `deviceId` 时**一枪都不发**，如实回一句 `device_id_failed`。
/// 注册进注册表就等于对用户宣称"这条可用"，而用户点了**永远什么都不会发生** ——
/// **静默**才是真正的坏结果。
///
/// 任务类型本身仍可被**直接调用**（判定层与编排层能跑、能测）；
/// 用户看得见的两条路（`/api/runs` 与 `/api/capabilities`）都按注册表走，
/// 摘掉注册就摘掉了"对外宣称"这件事。哨兵见文末
/// `tests::register_all_does_not_expose_exchange`。
///
/// # 红包派对 `cloud-phone-redpack`（**今天只读**）
///
/// 它与上面几条**性质不同**：签到族 / 小红花族共用 `H5Client` 的网盘侧凭证，
/// 而这一条要**自己换一套票** —— `GET .../cloud/userdomain/v2/querySpecToken`
/// （`targetSourceId=001216`）拿 SSO 票，再用它换活动站的 `token`。
/// 网盘 `authorization` / H5 `jwttoken` / 活动站 `token` 是三套东西，
/// **不能互相顶替**（协议层 `cloud_phone_redpack` 模块开头有对照表）。
///
/// ⚠ 这一趟**四枪全是只读**：换票两枪 + 任务表 + 余额。写入侧那 7 条
/// （`userSign` / `userCompleteTask` / …）的判据在协议层已落定，但接进
/// 编排层是**另一个**可验证单元（每条写都要先按 `userStatus` 判定该不该发、
/// 同一 `taskCode` 一次运行最多发一次）—— 所以 title 与 `summary_line`
/// 都明写「盘点 / 只读」：不做的事不假装做。
///
/// # AI 头像 `ai-avatar`（**默认关**，而且生成链今天断在下载那一枪）
///
/// 它是一条**会写**的链（合成、抽奖、往网盘写图），所以默认 `false`
/// （`account.rs:184`）；开关关掉时一枪都不发。
///
/// ⚠ 登记它时要说清两件事，免得被读成「已经能无人值守地跑」：
///
/// 1. **生成链断在「下载合成图」那一枪**：本仓的 HTTP 抽象只承载文本
///    （`HttpResponse.body` 是 `String`），而那一枪回的是图片字节 ——
///    有损解码后再 base64 会把图弄坏且**不报错**。协议层主动判失败，
///    所以 `upload` 今天是**不可达**的（请求构造与形状都有测试钉住）。
///    接上它需要给 HTTP 抽象加一条保留原始字节的路径。
/// 2. **收尾刻意不照抄旧实现**：`mcloud_tasks::ai_avatar` 先取一份目录基线，
///    只清「本趟造的」`fileId`；旧 TS 按文件名匹配会把**同名旧文件**一起删。
///
/// 另：参考实现 2026-09-25 实测这一族回 404/405（可能活动已下线，见协议层
/// `mcloud_upstream::ai_avatar` 模块头）—— 运行期非 200 一律如实上抛。
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
        // ⚠ `exchange` **刻意不在这里**：缺 deviceId 来源 / visitlog 预热 /
        // 等待档对齐（见上面那段说明），注册了就是"对外宣称可用"。
        // 哨兵：`tests::register_all_does_not_expose_exchange`。
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

    /// ⭐ `exchange` **不得**出现在 `register_all` 里。
    ///
    /// `/api/capabilities` 与 `/api/runs` 都按注册表说话 ⇒
    /// "注册了"就等于"对用户宣称这条能用"。而 exchange 缺 `deviceId` 来源 /
    /// `visitlog` 预热 / 等待档对齐（见上面 `register_all` 的说明），
    /// 用户点了**永远什么都不会发生**。
    ///
    /// 哪天三条缺口补齐、要放它出来，这条会红 —— 提醒同时改掉
    /// `exchange.rs` 里 `ExchangeTask` 那段"当前不注册"的说明。
    #[test]
    fn register_all_does_not_expose_exchange() {
        let mut registry = Registry::new();
        register_all(&mut registry).expect("注册不该重名");
        let metas = registry.list();
        let names: Vec<&str> = metas.iter().map(|m| m.name.as_str()).collect();

        assert!(
            !names.contains(&"exchange"),
            "exchange 缺 deviceId 来源/visitlog 预热/等待档对齐，不得对外宣称可用：{names:?}"
        );
        // 反面：摘它不该误伤邻居（少一条要有人发现）。
        for name in [
            "sign",
            "live-room",
            "msg-push",
            "notice",
            "cloud-phone-redpack",
        ] {
            assert!(names.contains(&name), "{name} 不该被摘掉：{names:?}");
        }
    }
}
