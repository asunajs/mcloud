//! mcloud-app —— 组装层。
//!
//! 把具体 IO 实现（reqwest / JSON 存储 / 系统时钟）注入 engine，
//! 供 `mcloud-cli` 与 `mcloud-server` 共用。

pub mod auth_refresh;
pub mod builder;
pub mod hub;
pub mod login;
pub mod notify;
// 多渠道运行推送（照 mcloud-sign 的 push 模块移植）。
// ⚠ 本版**只做模块、不接线**：没有生产代码调用它，见 push.rs 模块头。
pub mod push;
pub mod sink;
pub mod smid;

pub use auth_refresh::{decide, AuthRefreshService, RefreshDecision, RefreshOutcome};
pub use builder::{App, AppBuilder};
pub use hub::{Delivery, EventHub};
pub use login::{bare_transport, parse_platform, platform_label, AppSendSmsOutcome, LoginService};
pub use push::{
    format_logs_for_push, send_notify, send_request, BarkConfig, Channel, ChannelOutcome,
    CustomPost, DingTalkConfig, EmailConfig, PushApiData, PushConfig, PushContext,
    PushLogCollector, PushLogEntry, PushLogLevel, PushRequest, PushResponse, PushTransport,
    TgBotConfig, TokenConfig, TwoImConfig, WorkWeixinBotConfig, WorkWeixinConfig,
};
pub use sink::BroadcastSink;
pub use smid::{
    device_id, ensure_fingerprint, report_device_id, rotate_fingerprint, SmidClient, SmidService,
    UploadContext,
};

use std::sync::Arc;

/// 应用句柄。
pub struct AppHandle {
    /// 数据目录（`.json` 表 + 日志都在这儿）。
    ///
    /// 放在句柄上而不是 `App` 上：WebUI 的「存储信息」要报绝对路径，
    /// 而服务器只拿得到句柄（`ServerState::new(Arc::new(app.handle))`）。
    pub data_dir: std::path::PathBuf,
    /// 主动关停信号。
    ///
    /// WebUI 的「重启服务」用它：`notify_one` ⇒ `main.rs` 的
    /// `with_graceful_shutdown` 分支醒过来 ⇒ 进程优雅退出 ⇒ 由 launchd/systemd 重拉。
    /// **不自己 `kill`**：托管场景下信号会走一遍完整收尾（停调度器、日志落盘），
    /// 而 `process::exit` 会把这些全跳过。
    pub shutdown: Arc<tokio::sync::Notify>,
    pub runner: Arc<mcloud_engine::Runner>,
    pub registry: Arc<mcloud_engine::Registry>,
    pub store: Arc<dyn mcloud_core::Store>,
    pub clock: Arc<dyn mcloud_core::Clock>,
    /// 事件中枢：日志收集、序号分配、SSE 重放都经它。
    pub hub: Arc<EventHub>,
    /// 定时调度器。
    pub scheduler: Arc<mcloud_engine::Scheduler>,
    /// 编排执行器。
    pub pipelines: Arc<mcloud_engine::PipelineRunner>,
    /// 运行中任务的取消登记表。
    pub cancel: Arc<mcloud_engine::CancelRegistry>,
    /// smid 设备指纹编排（查看当前设备、主动更换）。
    ///
    /// 请求路径上的 `deviceid` 由 `SmidClient` 自动补，不需要任务关心；
    /// 这一份是给 WebUI / CLI 用的（"换一台设备"这类显式动作）。
    pub smid: Arc<SmidService>,
    /// 账号登录编排（扫码 / 短信 / 账密）。
    ///
    /// 与 `smid` 同类：请求路径不需要它，是给 CLI / WebUI 的**交互式**入口
    /// （登录要等人扫码、输验证码，做不成无人值守的 `Task`）。
    pub login: Arc<crate::login::LoginService>,
    /// 凭据续期编排。
    ///
    /// 与 `login` 并列的理由：它也需要按账号构造请求，而 "刷哪个账号"
    /// 是调用方给的。四个触发面（CLI 单账号 / CLI 批量 / WebUI 按钮 /
    /// 服务内调度）共用它，所以判据与窄写不会各自漂移。
    pub auth_refresh: Arc<crate::auth_refresh::AuthRefreshService>,
}

impl AppHandle {
    /// 用既有历史**续上 run id**。起服务之前必须调一次。
    ///
    /// # 为什么这件事非做不可（2026-10-01 实际踩到）
    ///
    /// `Runner` 的编号从 **1** 开始（`Runner::new`），而运行记录是**按编号 upsert** 的
    /// （`RunStore::upsert_run`：同编号就覆盖）。两件事撞在一起 = **静默丢数据**：
    /// 每次重启之后新跑的 1、2、3… 会把**上次启动**留下的同编号记录一条条盖掉，
    /// 于是一重启历史就"只剩最近这两条"。
    ///
    /// 用户看到的是"为啥历史只有两条" —— 而原因是编号被复用了，
    /// 与历史页、与存储限额都无关。
    ///
    /// # 为什么不在 `AppBuilder::build()` 里顺手做掉
    ///
    /// `build()` 是**同步**的，而读历史是 `async`（`RunStore::runs`）。
    /// 在 sync 里 `block_on` 一个库调用，在 tokio 运行时里会直接 panic ——
    /// 那是比这个 bug 更难查的一类故障。所以它做成必须显式调用的方法，
    /// 调用点唯一且显眼：`mcloud-server/src/main.rs`。
    pub async fn seed_run_ids(&self) -> mcloud_core::Result<()> {
        let runs = self.store.runs(usize::MAX).await?;
        if let Some(max) = runs.iter().map(|r| r.run_id.0).max() {
            self.runner.seed_run_id(max);
            tracing::info!(max_run_id = max, kept = runs.len(), "已从历史续上 run id");
        }
        Ok(())
    }

    /// 事件中枢也是 `EventSink`，任务通过它发日志。
    pub fn sink(&self) -> Arc<dyn mcloud_core::EventSink> {
        Arc::clone(&self.hub) as Arc<dyn mcloud_core::EventSink>
    }
}
