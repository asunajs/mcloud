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
