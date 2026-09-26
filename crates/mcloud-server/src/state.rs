//! 服务状态。

use std::sync::Arc;

use mcloud_app::AppHandle;

use crate::routes::login::LoginSessions;

#[derive(Clone)]
pub struct ServerState {
    pub handle: Arc<AppHandle>,
    /// 进行中的扫码会话。
    ///
    /// **刻意不落盘**：一次扫码会话只活两分钟，进程重启后重出一张码
    /// 比"从磁盘恢复一个已经过期的会话"更符合直觉，也少一份要清理的状态。
    pub login_sessions: Arc<LoginSessions>,
}

impl ServerState {
    pub fn new(handle: Arc<AppHandle>) -> Self {
        Self {
            handle,
            login_sessions: Arc::new(LoginSessions::new()),
        }
    }
}
