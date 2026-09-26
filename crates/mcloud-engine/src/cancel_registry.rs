//! 运行中任务的登记表：支持按 run 中止。
//!
//! # 中止是协作式的
//!
//! [`CancelRegistry::cancel`] 只**置一个标志**，不打断正在进行的请求。
//! 任务在安全点（每轮循环、每个请求之间）自己检查并退出。
//!
//! 为什么不直接砍掉 future：那样会留下"请求真的打到上游了、
//! 本地却记成已中止"的不一致 —— 用户再点一次就是重复签到。
//! 代价是中止不是瞬时的，换来的是"中止"不会造成半个副作用。
//!
//! # 为什么按 run 而不是按 `task:account`
//!
//! 用户看到的是"这次运行"，点中止时给的也是 run id。
//! 用 `task:account` 当键的话，"中止了签到" 会连带把同一账号
//! 另一个正在跑的、用户没想停的运行一起停掉。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mcloud_core::{CancelToken, RunId};

/// 运行中任务的登记表。
#[derive(Default)]
pub struct CancelRegistry {
    inner: Mutex<HashMap<RunId, CancelToken>>,
}

impl CancelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RunId, CancelToken>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 登记一次运行，返回它的取消令牌。
    ///
    /// 任务拿到令牌后在安全点检查。
    pub fn register(&self, run_id: RunId) -> CancelToken {
        let token = CancelToken::new();
        self.lock().insert(run_id, token.clone());
        token
    }

    /// 运行结束时注销。
    pub fn release(&self, run_id: RunId) {
        self.lock().remove(&run_id);
    }

    /// 请求中止某次运行。
    ///
    /// 返回 `false` 表示这次运行**不在运行中**（可能已经结束）。
    /// 这不是错误 —— 用户点了中止而任务刚好跑完，是完全正常的竞态。
    pub fn cancel(&self, run_id: RunId) -> bool {
        match self.lock().get(&run_id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// 该运行是否已请求中止。
    pub fn is_cancelled(&self, run_id: RunId) -> bool {
        self.lock().get(&run_id).is_some_and(|t| t.is_cancelled())
    }

    /// 该运行**当前是否仍在运行**（登记着且未结束）。
    ///
    /// 与 [`Self::is_cancelled`] 是两件事：一个已请求中止的运行
    /// 在任务退出前**仍在运行**。界面据此区分
    /// "已请求中止、还在收尾" 与 "已经停了"。
    pub fn is_running(&self, run_id: RunId) -> bool {
        self.lock().contains_key(&run_id)
    }

    pub fn running_count(&self) -> usize {
        self.lock().len()
    }

    /// 当前运行中的所有 run id。
    pub fn running(&self) -> Vec<RunId> {
        self.lock().keys().copied().collect()
    }
}

/// 共享句柄。
pub type SharedCancelRegistry = Arc<CancelRegistry>;

impl std::fmt::Debug for CancelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelRegistry")
            .field("running", &self.running_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_cancel_flips_the_token() {
        let reg = CancelRegistry::new();
        let token = reg.register(RunId::new(1));
        assert!(!token.is_cancelled());

        assert!(reg.cancel(RunId::new(1)), "登记过的运行应能中止");
        assert!(token.is_cancelled(), "任务手里的令牌必须看到取消");
        assert!(reg.is_cancelled(RunId::new(1)));
    }

    /// 中止一个不存在的运行返回 false，而不是 panic 或报错。
    ///
    /// "用户点中止时任务刚好跑完"是完全正常的竞态。
    #[test]
    fn cancelling_an_unknown_run_is_false_not_an_error() {
        let reg = CancelRegistry::new();
        assert!(!reg.cancel(RunId::new(99)));
    }

    /// "仍在运行"与"已请求中止"是两件事。
    ///
    /// 一个已请求中止的运行，在任务退到安全点之前**仍在运行** ——
    /// 混为一谈会让界面在任务还在跑时就显示"已停止"。
    #[test]
    fn is_running_is_distinct_from_is_cancelled() {
        let reg = CancelRegistry::new();
        let _ = reg.register(RunId::new(1));

        assert!(reg.is_running(RunId::new(1)));
        assert!(!reg.is_cancelled(RunId::new(1)));

        reg.cancel(RunId::new(1));
        assert!(reg.is_cancelled(RunId::new(1)));
        assert!(
            reg.is_running(RunId::new(1)),
            "已请求中止但还没退出 ⇒ 仍在运行"
        );

        reg.release(RunId::new(1));
        assert!(!reg.is_running(RunId::new(1)));
    }

    #[test]
    fn release_removes_the_entry() {
        let reg = CancelRegistry::new();
        let _ = reg.register(RunId::new(1));
        assert_eq!(reg.running_count(), 1);
        reg.release(RunId::new(1));
        assert_eq!(reg.running_count(), 0);
        assert!(reg.running().is_empty());
    }

    /// 每个运行拿到**独立**的令牌：中止一个不影响另一个。
    #[test]
    fn tokens_are_per_run_not_global() {
        let reg = CancelRegistry::new();
        let a = reg.register(RunId::new(1));
        let b = reg.register(RunId::new(2));

        reg.cancel(RunId::new(1));
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled(), "中止一次运行不该影响另一次");
    }

    #[test]
    fn release_then_cancel_is_a_noop() {
        let reg = CancelRegistry::new();
        let token = reg.register(RunId::new(1));
        reg.release(RunId::new(1));
        assert!(!reg.cancel(RunId::new(1)), "已注销的不能再中止");
        assert!(!token.is_cancelled(), "注销后不该再被动改状态");
    }

    #[test]
    fn re_registering_same_run_gets_a_fresh_token() {
        let reg = CancelRegistry::new();
        let first = reg.register(RunId::new(1));
        reg.cancel(RunId::new(1));
        reg.release(RunId::new(1));

        let second = reg.register(RunId::new(1));
        assert!(!second.is_cancelled(), "重新登记必须是干净的状态");
        assert!(first.is_cancelled(), "旧令牌仍保持它当时的状态");
    }
}
