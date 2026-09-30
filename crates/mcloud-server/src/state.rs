//! 服务状态。

use std::sync::Arc;

use mcloud_app::AppHandle;

use crate::routes::login::LoginSessions;

#[derive(Clone)]
pub struct ServerState {
    pub handle: Arc<AppHandle>,
    /// 本进程**开始服务**的时刻（epoch 毫秒）。
    ///
    /// 只给界面算「运行时间」用。为什么值得单独留一格：状态点只会说"现在看着正常"，
    /// 而一个**持续增长**的时长是"这个进程真的活着"最难伪造的证据 ——
    /// 顺带也回答了"上一次重启是什么时候"。
    ///
    /// ⚠ 取的是构造这一刻：进程起来到建好 HTTP 层之间只有毫秒级，
    /// 与"进程启动时刻"的差别看不出来。
    pub started_at_ms: i64,
    /// 进行中的扫码会话。
    ///
    /// **刻意不落盘**：一次扫码会话只活两分钟，进程重启后重出一张码
    /// 比"从磁盘恢复一个已经过期的会话"更符合直觉，也少一份要清理的状态。
    pub login_sessions: Arc<LoginSessions>,
}

impl ServerState {
    pub fn new(handle: Arc<AppHandle>) -> Self {
        let started_at_ms = handle.clock.now_ms();
        Self {
            handle,
            started_at_ms,
            login_sessions: Arc::new(LoginSessions::new()),
        }
    }
}
