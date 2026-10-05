//! 事件扇出：一份事件同时进日志与 SSE 广播。

use mcloud_core::{EventSink, RunEvent};
use tokio::sync::broadcast;

use crate::run_labels::{trace_event, RunLabels};

/// 广播事件到所有订阅者（SSE 连接）。
pub struct BroadcastSink {
    tx: broadcast::Sender<RunEvent>,
    /// `run_id → 账号`：让每一行日志都认得出主人（见 [`crate::run_labels`]）。
    labels: RunLabels,
}

impl BroadcastSink {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self {
            tx,
            labels: RunLabels::default(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.tx.subscribe()
    }

    /// 当前订阅者数量，便于诊断。
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl EventSink for BroadcastSink {
    fn emit(&self, event: RunEvent) {
        // 出去之前先过一道脱敏（复核意见 §1.14）：
        // 这一路会把任务的日志/结论原样带出去（进程日志 / SSE / 推送 / 落盘），
        // 而上游回显里可能就有一枚凭证 ✗。形状由 mcloud-core 给死 ⇒ sink 只要一行 ✓。
        let event = event.scrubbed();
        // 先落日志：即使没有订阅者，运行过程也要可追溯。
        // ⚠ 走共用的 [`trace_event`]（`EventHub` 也用它 ✓）：两个 sink 各抄一份逐字相同的
        // `match` 时，"只有一个 sink 盖了账号"这种漏是迟早的事 ✗。
        trace_event(&self.labels, &event);

        // 无订阅者时 send 返回 Err，属正常情况，忽略
        let _ = self.tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::RunId;

    #[tokio::test]
    async fn subscribers_receive_events() {
        let sink = BroadcastSink::new(16);
        let mut rx = sink.subscribe();
        assert_eq!(sink.receiver_count(), 1);

        sink.emit(RunEvent::info(RunId::new(1), "hello", 0));
        let got = rx.recv().await.unwrap();
        assert_eq!(got.run_id(), RunId::new(1));
    }

    /// ⚠ **边界级**钉子：上面那批复核钉子都在 `mcloud-core` 里测纯函数 ✓，
    /// 于是"某个 sink 忘了调 `event.scrubbed()`"这件事它们**一条都抓不到** ✗。
    /// 这一条测的正是边界本身：订阅者（页面的 SSE 流）拿到的事件必须已经脱敏 ✓。
    #[tokio::test]
    async fn emit_scrubs_before_fanning_out() {
        let sink = BroadcastSink::new(16);
        let mut rx = sink.subscribe();
        sink.emit(RunEvent::info(
            RunId::new(1),
            "登录失败 password=hunter2；请求头 Authorization: Bearer \
             eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.abcdefghijklmnop",
            0,
        ));
        let text = format!("{:?}", rx.recv().await.expect("订阅者必须收到事件"));
        assert!(!text.contains("hunter2"), "sink 没脱敏：{text}");
        assert!(!text.contains("eyJhbGciOi"), "sink 没脱敏：{text}");
        // 正常内容要留着（脱敏不是"什么都藏起来"）
        assert!(text.contains("登录失败"), "正常文案被吃掉了：{text}");
    }

    #[tokio::test]
    async fn emit_without_subscribers_does_not_panic() {
        let sink = BroadcastSink::new(4);
        sink.emit(RunEvent::info(RunId::new(1), "no one listening", 0));
        assert_eq!(sink.receiver_count(), 0);
    }
}
