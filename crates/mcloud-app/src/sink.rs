//! 事件扇出：一份事件同时进日志与 SSE 广播。

use mcloud_core::{EventSink, LogLevel, RunEvent};
use tokio::sync::broadcast;

/// 广播事件到所有订阅者（SSE 连接）。
pub struct BroadcastSink {
    tx: broadcast::Sender<RunEvent>,
}

impl BroadcastSink {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
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
        // 先落日志：即使没有订阅者，运行过程也要可追溯
        match &event {
            RunEvent::Log { level, message, .. } => match level {
                LogLevel::Debug => tracing::debug!("{message}"),
                LogLevel::Info => tracing::info!("{message}"),
                LogLevel::Warn => tracing::warn!("{message}"),
                LogLevel::Error => tracing::error!("{message}"),
            },
            RunEvent::Started { task, account, .. } => {
                tracing::info!(task, account, "任务开始");
            }
            RunEvent::Finished {
                outcome,
                duration_ms,
                ..
            } => {
                tracing::info!(duration_ms, "任务结束：{}", outcome.summary());
            }
            RunEvent::Progress {
                step, total, label, ..
            } => {
                tracing::debug!("进度 {step}/{total} {label}");
            }
            // 结构化明细**不逐条打日志**：它可能很大（整份兑换结论），
            // 而日志那一格是给人扫的。它真正的去处是 `RunRecord.details`
            // —— 由 runner 累积后落历史，前端从那里读。
            RunEvent::Details { .. } => {}
        }

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

    #[tokio::test]
    async fn emit_without_subscribers_does_not_panic() {
        let sink = BroadcastSink::new(4);
        sink.emit(RunEvent::info(RunId::new(1), "no one listening", 0));
        assert_eq!(sink.receiver_count(), 0);
    }
}
