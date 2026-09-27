//! run 内**结构化明细**账本。
//!
//! # 它解决什么
//!
//! 有些任务要交回的不只是一句话，而是一份**形状**：兑换要给出
//! `status / plan / code / offset / order`，前端要按字段渲染。
//! 而 `TaskOutcome` 只有 success/skipped/failed 三档加一句 `summary`。
//!
//! `RunRecord.details` 就是为这件事准备的（它的注释写着"前端可直接渲染"），
//! 但它是**运行结束后**由 `Runner` 组装的 —— 任务手里没有 `RunRecord`。
//!
//! 于是任务走**既有**的事件通道把自己那份明细发出来
//! （[`mcloud_core::RunEvent::Details`]），由本账本按 `run_id` 收着，
//! 运行结束时 `Runner` 取出来并进 `details`。
//!
//! # 为什么不给 `TaskContext` 一个"直接写 details"的口子
//!
//! 那需要在任务与记录之间开一条旁路，而"运行历史长什么样"这件事
//! 就会从执行器分散到每个任务里。走事件则不然：任务只管**自报**，
//! 怎么落、落进哪一格仍然由执行器决定。
//!
//! # 与产物账本（`ArtifactLedger`）的区别
//!
//! 两者都是"运行期间攒、收尾时取走"，但语义不同：产物账本是**欠账**
//! （不清掉就留在用户网盘上），明细只是**信息**。所以这里没有
//! `register_created` 那种防线 —— 它不需要防漏记。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mcloud_core::RunId;

/// 一个 run 攒下的明细条目（按到达顺序）。
type Entries = Vec<serde_json::Value>;

/// 进程内共享的明细账本。
pub type SharedDetails = Arc<DetailsLedger>;

/// 按 `run_id` 收着任务自报的结构化明细。
#[derive(Default)]
pub struct DetailsLedger {
    inner: Mutex<HashMap<RunId, Entries>>,
}

impl DetailsLedger {
    pub fn new() -> SharedDetails {
        Arc::new(Self::default())
    }

    /// 记下一条。多次调用按顺序累积。
    ///
    /// 锁中毒时取回内部值继续用（与 `EventHub::lock` 同一取向）：
    /// 每次操作都在锁内一次做完，没有"写了一半"的中间态可被读到，
    /// 所以中毒不构成不可用的理由，而跟着 panic 会让整个服务停摆。
    pub fn push(&self, run_id: RunId, payload: serde_json::Value) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.entry(run_id).or_default().push(payload);
    }

    /// 取走并遗忘某个 run 的明细。
    ///
    /// 语义与 `ArtifactLedger::forget` 一致：**取走即遗忘**，
    /// 免得一次运行结束之后账本还替它占着内存。
    ///
    /// 返回空 `Vec` 有两种可能（"这条任务没自报明细"与"已经被取走过"），
    /// 而调用方对这两种的处置**相同** —— 都是"没有明细可写"。
    pub fn take(&self, run_id: RunId) -> Entries {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.remove(&run_id).unwrap_or_default()
    }
}

/// 交给任务的那只手：`run_id` 已经填好，任务不需要（也不该）自己知道。
///
/// 与 [`crate::ArtifactRecorder`] 同一形状 —— 执行器建、任务用、
/// 任务不碰账本本身。
#[derive(Clone)]
pub struct DetailsRecorder {
    ledger: SharedDetails,
    run_id: RunId,
}

impl DetailsRecorder {
    pub fn new(ledger: SharedDetails, run_id: RunId) -> Self {
        Self { ledger, run_id }
    }

    /// 自报一条结构化明细。
    ///
    /// # 大小自觉
    ///
    /// 这一格会落进运行历史、并在界面上被渲染。往里塞整份上游响应
    /// （几百 KB）会让历史文件与页面一起变重 —— 传**要给人看的那几个字段**。
    pub fn record(&self, payload: serde_json::Value) {
        self.ledger.push(self.run_id, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(n: u64) -> RunId {
        RunId::new(n)
    }

    #[test]
    fn accumulates_in_arrival_order() {
        let ledger = DetailsLedger::new();
        ledger.push(rid(1), serde_json::json!({"a": 1}));
        ledger.push(rid(1), serde_json::json!({"b": 2}));
        let got = ledger.take(rid(1));
        assert_eq!(
            got,
            vec![serde_json::json!({"a": 1}), serde_json::json!({"b": 2})]
        );
    }

    #[test]
    fn take_is_idempotent_and_forgets() {
        // 取走即遗忘：第二次取是空的。这与 `ArtifactLedger::forget` 同一口径，
        // 也让"一次运行结束之后账本不再替它占内存"这件事成立。
        let ledger = DetailsLedger::new();
        ledger.push(rid(7), serde_json::json!({"x": true}));
        assert_eq!(ledger.take(rid(7)).len(), 1);
        assert!(ledger.take(rid(7)).is_empty());
    }

    #[test]
    fn runs_do_not_bleed_into_each_other() {
        let ledger = DetailsLedger::new();
        ledger.push(rid(1), serde_json::json!({"who": "one"}));
        ledger.push(rid(2), serde_json::json!({"who": "two"}));
        assert_eq!(ledger.take(rid(2)), vec![serde_json::json!({"who": "two"})]);
        assert_eq!(ledger.take(rid(1)), vec![serde_json::json!({"who": "one"})]);
    }

    #[test]
    fn unknown_run_is_empty_not_an_error() {
        assert!(DetailsLedger::new().take(rid(999)).is_empty());
    }
}
