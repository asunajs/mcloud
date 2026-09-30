//! mcloud-app —— 组装层。
//!
//! 把具体 IO 实现（reqwest / JSON 存储 / 系统时钟）注入 engine，
//! 供 `mcloud-cli` 与 `mcloud-server` 共用。

pub mod auth_refresh;
pub mod builder;
pub mod hub;
pub mod login;
pub mod notify;
pub mod sink;
pub mod smid;

pub use auth_refresh::{decide, AuthRefreshService, RefreshDecision, RefreshOutcome};
pub use builder::{App, AppBuilder};
pub use hub::{Delivery, EventHub};
pub use login::{bare_transport, parse_platform, platform_label, AppSendSmsOutcome, LoginService};
pub use sink::BroadcastSink;
pub use smid::{
    device_id, ensure_fingerprint, report_device_id, rotate_fingerprint, SmidClient, SmidService,
    UploadContext,
};

use std::sync::Arc;

/// 应用句柄。
pub struct AppHandle {
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
    /// 事件中枢也是 `EventSink`，任务通过它发日志。
    pub fn sink(&self) -> Arc<dyn mcloud_core::EventSink> {
        Arc::clone(&self.hub) as Arc<dyn mcloud_core::EventSink>
    }
}
