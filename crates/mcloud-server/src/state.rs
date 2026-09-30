//! 服务状态。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mcloud_app::AppHandle;
use mcloud_core::{RunId, UserId};

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
    /// 本进程由**本服务**发起的运行 → 归属。
    ///
    /// # 为什么需要它
    ///
    /// 运行记录**结束时**才落盘（`Runner::run_impl` 收尾那一步），而客户端
    /// 拿到 run id 后会立刻订阅 SSE、或请求中止。那一刻存储层里没有这条记录，
    /// 归属也就无从推导 —— 不登记的话，B 只要猜到 id 就能订阅 / 中止 A 的运行。
    ///
    /// # 为什么只留「还在跑」的
    ///
    /// 表项在 [`ServerState::note_run_owner`] 里顺手按取消登记表清理：
    /// 运行结束后记录已经落盘，归属可由 `RunRecord.account` 推出来，
    /// 留着只会让这张表无限增长。
    pub run_owners: Arc<Mutex<HashMap<RunId, UserId>>>,
}

impl ServerState {
    pub fn new(handle: Arc<AppHandle>) -> Self {
        let started_at_ms = handle.clock.now_ms();
        Self {
            handle,
            started_at_ms,
            login_sessions: Arc::new(LoginSessions::new()),
            run_owners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 登记一次「本服务发起的运行」属于谁。
    ///
    /// 顺手把已经跑完的条目清掉（见 `run_owners` 的说明）。锁不跨 `await`。
    pub fn note_run_owner(&self, run_id: RunId, owner: &UserId) {
        let Ok(mut map) = self.run_owners.lock() else {
            // 锁中毒说明别的线程在这段同步代码里 panic 过。归属登记丢了
            // 只会让**这次**运行在落盘前不可订阅，不该让请求失败。
            return;
        };
        map.retain(|id, _| self.handle.cancel.is_running(*id));
        map.insert(run_id, owner.clone());
    }
}
