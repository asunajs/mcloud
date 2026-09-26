//! 事件中枢测试。
//!
//! 重点：**断线续传不丢不重**、**先订阅再读库不漏窗口**、**丢了要报出来**。

use super::*;
use async_trait::async_trait;
use mcloud_core::testkit::FixedClock;
use mcloud_core::{
    Account, AccountId, AppendOutcome, Pipeline, PipelineId, RunRecord, RunStatus, Schedule,
    ScheduleId, TaskOutcome,
};
use std::sync::Mutex;

const NOW_MS: i64 = 1_700_000_000_000;

/// 内存存储：记录所有落盘的日志。
#[derive(Default)]
struct MemStore {
    logs: Mutex<Vec<LogLine>>,
    fail: bool,
}

#[async_trait]
impl mcloud_core::AccountStore for MemStore {
    async fn accounts(&self) -> mcloud_core::Result<Vec<Account>> {
        Ok(Vec::new())
    }
    async fn account(&self, _i: &AccountId) -> mcloud_core::Result<Option<Account>> {
        Ok(None)
    }
    async fn save_account(&self, _a: &Account) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn remove_account(&self, _i: &AccountId) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn account_state(&self, _i: &AccountId) -> mcloud_core::Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    async fn save_account_state(
        &self,
        _i: &AccountId,
        _s: &serde_json::Value,
    ) -> mcloud_core::Result<()> {
        Ok(())
    }
}
#[async_trait]
impl mcloud_core::RunStore for MemStore {
    async fn upsert_run(&self, _r: &RunRecord) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn runs(&self, _l: usize) -> mcloud_core::Result<Vec<RunRecord>> {
        Ok(Vec::new())
    }
    async fn run(&self, _i: RunId) -> mcloud_core::Result<Option<RunRecord>> {
        Ok(None)
    }
    async fn running_runs(&self) -> mcloud_core::Result<Vec<RunRecord>> {
        Ok(Vec::new())
    }
}
#[async_trait]
impl mcloud_core::LogStore for MemStore {
    async fn append_logs(
        &self,
        _r: RunId,
        lines: &[LogLine],
    ) -> mcloud_core::Result<AppendOutcome> {
        if self.fail {
            return Err(mcloud_core::Error::Storage("磁盘写满".into()));
        }
        self.logs.lock().unwrap().extend_from_slice(lines);
        Ok(AppendOutcome::Appended)
    }
    async fn logs(&self, _r: RunId, _a: Option<Seq>) -> mcloud_core::Result<Vec<LogLine>> {
        Ok(self.logs.lock().unwrap().clone())
    }
    async fn clear_logs(&self, _r: RunId) -> mcloud_core::Result<()> {
        Ok(())
    }
}
#[async_trait]
impl mcloud_core::ScheduleStore for MemStore {
    async fn schedules(&self) -> mcloud_core::Result<Vec<Schedule>> {
        Ok(Vec::new())
    }
    async fn schedule(&self, _i: &ScheduleId) -> mcloud_core::Result<Option<Schedule>> {
        Ok(None)
    }
    async fn save_schedule(&self, _s: &Schedule) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn remove_schedule(&self, _i: &ScheduleId) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn mark_schedule_fired(&self, _i: &ScheduleId, _ms: i64) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn record_schedule_run(
        &self,
        _i: &ScheduleId,
        _run_ms: Option<i64>,
        _status: &str,
        _error: Option<&str>,
    ) -> mcloud_core::Result<()> {
        Ok(())
    }
}
#[async_trait]
impl mcloud_core::PipelineStore for MemStore {
    async fn pipelines(&self) -> mcloud_core::Result<Vec<Pipeline>> {
        Ok(Vec::new())
    }
    async fn pipeline(&self, _i: &PipelineId) -> mcloud_core::Result<Option<Pipeline>> {
        Ok(None)
    }
    async fn save_pipeline(&self, _p: &Pipeline) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn remove_pipeline(&self, _i: &PipelineId) -> mcloud_core::Result<()> {
        Ok(())
    }
    async fn mark_pipeline_ran(&self, _i: &PipelineId, _ms: i64) -> mcloud_core::Result<()> {
        Ok(())
    }
}

fn hub() -> EventHub {
    EventHub::new(
        Arc::new(MemStore::default()) as Arc<dyn Store>,
        Arc::new(FixedClock(NOW_MS)),
    )
}

fn hub_with(store: Arc<MemStore>) -> EventHub {
    EventHub::new(store as Arc<dyn Store>, Arc::new(FixedClock(NOW_MS)))
}

// ── 序号 ────────────────────────────────────────────────────────────────

#[test]
fn seq_starts_at_one_and_increases() {
    let h = hub();
    let id = RunId::new(1);
    let sub = h.subscribe(id, Seq::ZERO);

    h.emit(RunEvent::info(id, "第一条", NOW_MS));
    h.emit(RunEvent::info(id, "第二条", NOW_MS));

    let d = h.drain(id, sub);
    assert_eq!(d.events.len(), 2);
    assert_eq!(d.events[0].seq, Seq(1), "从 1 开始：0 是'还没有事件'的哨兵");
    assert_eq!(d.events[1].seq, Seq(2));
}

// ── 迟到的订阅者 ────────────────────────────────────────────────────────

/// ⭐ 运行已经开始、用户才打开页面：必须能拿到之前的事件。
///
/// 这是 `broadcast` 做不到的第一件事，也是选内存历史的理由。
#[test]
fn late_subscriber_receives_history() {
    let h = hub();
    let id = RunId::new(1);

    h.emit(RunEvent::info(id, "早于订阅", NOW_MS));
    h.emit(RunEvent::info(id, "也早于订阅", NOW_MS));

    let sub = h.subscribe(id, Seq::ZERO);
    let d = h.drain(id, sub);
    assert_eq!(d.events.len(), 2, "迟到的订阅者必须拿到已发布的历史");
}

/// 断线续传：带上已看到的 seq，只拿新的。
#[test]
fn resubscribe_with_seen_seq_only_gets_new_events() {
    let h = hub();
    let id = RunId::new(1);
    h.emit(RunEvent::info(id, "1", NOW_MS));
    h.emit(RunEvent::info(id, "2", NOW_MS));
    h.emit(RunEvent::info(id, "3", NOW_MS));

    // 客户端说"我已经看到 2 号"
    let sub = h.subscribe(id, Seq::seen(2));
    let d = h.drain(id, sub);
    assert_eq!(d.events.len(), 1, "只该补发 3 号");
    assert_eq!(d.events[0].seq, Seq(3));
}

/// ⭐ "已看到 n" 与 "下一个要取" 混淆会**静默跳过一条**。
///
/// 这个测试锁住语义：`Seq::seen(3)` 之后第一条必须是 4，不是 5。
#[test]
fn seen_seq_does_not_skip_the_next_one() {
    let h = hub();
    let id = RunId::new(1);
    for i in 1..=5 {
        h.emit(RunEvent::info(id, format!("第{i}条"), NOW_MS));
    }
    let sub = h.subscribe(id, Seq::seen(3));
    let d = h.drain(id, sub);
    assert_eq!(d.events[0].seq, Seq(4), "必须是 4：跳过 4 就是那个经典 bug");
    assert_eq!(d.events.len(), 2);
}

/// `resume_after(n)` 与 `seen(n)` 数值相同，行为也必须相同。
#[test]
fn resume_after_matches_seen() {
    let h = hub();
    let id = RunId::new(1);
    for i in 1..=4 {
        h.emit(RunEvent::info(id, format!("{i}"), NOW_MS));
    }
    let a = h.drain(id, h.subscribe(id, Seq::seen(2))).events;
    let b = h.drain(id, h.subscribe(id, Seq::resume_after(2))).events;
    assert_eq!(a, b);
    assert_eq!(a[0].seq, Seq(3));
}

// ── 先订阅再读库 ────────────────────────────────────────────────────────

/// ⭐ 订阅后、读库前发布的事件不能丢。
///
/// 正确顺序是"先订阅再读库"：订阅会把 `since` 之后的内存历史补进队列，
/// 于是两步之间那个窗口里的事件一定在队列里。
#[test]
fn subscribing_before_reading_covers_the_gap() {
    let h = hub();
    let id = RunId::new(1);
    h.emit(RunEvent::info(id, "库里的老事件", NOW_MS));

    // 模拟：先订阅（拿到 backlog）…
    let sub = h.subscribe(id, Seq::ZERO);
    // …在这个窗口里又有事件发布（此时"读库"还没发生）
    h.emit(RunEvent::info(id, "窗口里的事件", NOW_MS));

    let d = h.drain(id, sub);
    assert_eq!(d.events.len(), 2, "窗口里的事件不能两头都漏掉");
    assert_eq!(d.dropped, 0);
}

// ── 丢弃策略 ────────────────────────────────────────────────────────────

/// 队列满时**丢最新的**，并让调用方看得见丢了多少。
#[test]
fn full_queue_drops_newest_and_reports_it() {
    let h = hub();
    let id = RunId::new(1);
    let sub = h.subscribe(id, Seq::ZERO);

    // 灌满队列再多发一些
    for i in 0..(SUBSCRIBER_CAPACITY + 5) {
        h.emit(RunEvent::info(id, format!("{i}"), NOW_MS));
    }

    let d = h.drain(id, sub);
    assert_eq!(
        d.events.len(),
        SUBSCRIBER_CAPACITY,
        "队列容量就是能拿到的上限"
    );
    assert_eq!(d.dropped, 5, "丢了多少必须报出来");
    // 保留的是**最老的**：前端至少能从 run 的开头看起
    assert_eq!(d.events[0].seq, Seq(1), "丢最新而不是最老");
}

/// 丢弃计数**累计且不自动清零** —— 静默少了半截日志最难查。
#[test]
fn drop_count_is_cumulative() {
    let h = hub();
    let id = RunId::new(1);
    let sub = h.subscribe(id, Seq::ZERO);
    for i in 0..(SUBSCRIBER_CAPACITY + 3) {
        h.emit(RunEvent::info(id, format!("{i}"), NOW_MS));
    }
    let first = h.drain(id, sub);
    assert_eq!(first.dropped, 3);

    h.emit(RunEvent::info(id, "再来一条", NOW_MS));
    let second = h.drain(id, sub);
    assert_eq!(second.dropped, 3, "计数不该被清零");
}

// ── 订阅者互相独立 ──────────────────────────────────────────────────────

/// 一个卡住的订阅者**不影响**跟得上的订阅者。
///
/// 这正是 `broadcast` 做不到的第二件事：它的环形缓冲按**最慢**的
/// 订阅者推进，于是"手机切后台了"会让其他客户端一起丢事件。
///
/// 这里 healthy 边收边取（模拟一个跟得上的消费者），stuck 一直不取。
/// 结果必须是：stuck 丢事件，healthy 一条不丢。
#[test]
fn a_stuck_subscriber_does_not_affect_a_healthy_one() {
    let h = hub();
    let id = RunId::new(1);
    let stuck = h.subscribe(id, Seq::ZERO);
    let healthy = h.subscribe(id, Seq::ZERO);

    // 发两轮容量：healthy 每轮都取走，stuck 从不取
    let total = SUBSCRIBER_CAPACITY * 2;
    let mut healthy_seen = 0;
    for i in 0..total {
        h.emit(RunEvent::info(id, format!("{i}"), NOW_MS));
        if i % 16 == 0 {
            let d = h.drain(id, healthy);
            assert_eq!(d.dropped, 0, "跟得上的订阅者不该丢事件");
            healthy_seen += d.events.len();
        }
    }
    healthy_seen += h.drain(id, healthy).events.len();
    assert_eq!(healthy_seen, total, "healthy 必须一条不漏地拿到全部");

    let s = h.drain(id, stuck);
    assert!(s.dropped > 0, "从不取的那个应该丢事件并被报出来");
    assert_eq!(s.events[0].seq, Seq(1), "它仍保留最老的一段");
}

// ── 落盘 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn flush_persists_logs() {
    let store = Arc::new(MemStore::default());
    let h = hub_with(Arc::clone(&store));
    let id = RunId::new(1);

    h.emit(RunEvent::info(id, "要落盘的日志", NOW_MS));
    h.emit(RunEvent::info(id, "第二条", NOW_MS));

    let n = h.flush().await.unwrap();
    assert_eq!(n, 2);

    let saved = store.logs.lock().unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0].message, "要落盘的日志");
    assert_eq!(saved[0].seq, Seq(1), "落盘的序号要与事件流一致，才能续传");
}

/// 落盘失败**不能**影响正在跑的任务，且日志要留着重试。
#[tokio::test]
async fn flush_failure_is_retried_not_lost() {
    let store = Arc::new(MemStore {
        logs: Mutex::new(Vec::new()),
        fail: true,
    });
    let h = hub_with(Arc::clone(&store));
    let id = RunId::new(1);

    h.emit(RunEvent::info(id, "会失败的日志", NOW_MS));
    let n = h.flush().await.unwrap();
    assert_eq!(n, 0, "失败时不该报告已落盘");

    // 失败后日志必须还在缓冲里，供下次重试
    assert_eq!(
        h.lock().pending.get(&id).map(|v| v.len()),
        Some(1),
        "失败的日志要留着重试"
    );
}

#[tokio::test]
async fn flush_with_nothing_pending_is_ok() {
    let h = hub();
    assert_eq!(h.flush().await.unwrap(), 0);
}

// ── 结束与回收 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn finished_is_terminal() {
    let h = hub();
    let id = RunId::new(1);
    let sub = h.subscribe(id, Seq::ZERO);
    h.emit(RunEvent::Finished {
        run_id: id,
        outcome: TaskOutcome::success("ok"),
        duration_ms: 100,
    });
    let d = h.drain(id, sub);
    assert_eq!(d.events.len(), 1);
    assert!(matches!(d.events[0].event, RunEvent::Finished { .. }));
}

#[test]
fn forget_removes_memory_state() {
    let h = hub();
    let id = RunId::new(1);
    h.emit(RunEvent::info(id, "x", NOW_MS));
    assert!(h.is_tracked(id));
    h.forget(id);
    assert!(!h.is_tracked(id), "已落盘后应能回收内存");
    assert!(h.replay(id, Seq::ZERO).is_empty());
}

/// `replay` 给"运行已结束、用户来看历史"用。
#[test]
fn replay_returns_history_after_seq() {
    let h = hub();
    let id = RunId::new(1);
    for i in 1..=4 {
        h.emit(RunEvent::info(id, format!("{i}"), NOW_MS));
    }
    assert_eq!(h.replay(id, Seq::ZERO).len(), 4);
    let after2 = h.replay(id, Seq::seen(2));
    assert_eq!(after2.len(), 2);
    assert_eq!(after2[0].seq, Seq(3));
}

#[test]
fn replay_of_unknown_run_is_empty_not_an_error() {
    let h = hub();
    assert!(h.replay(RunId::new(999), Seq::ZERO).is_empty());
}

// ── 内存上限 ────────────────────────────────────────────────────────────

/// 内存历史有上限：失控的任务不能把内存吃光。
#[test]
fn memory_history_is_capped() {
    let h = hub();
    let id = RunId::new(1);
    for i in 0..(HISTORY_PER_RUN + 100) {
        h.emit(RunEvent::info(id, format!("{i}"), NOW_MS));
    }
    let all = h.replay(id, Seq::ZERO);
    assert_eq!(all.len(), HISTORY_PER_RUN);
    // 保留最新的
    assert_eq!(all.last().unwrap().seq, Seq(HISTORY_PER_RUN as u64 + 100));
}

// ── 诊断 ────────────────────────────────────────────────────────────────

#[test]
fn subscriber_count_tracks_subscriptions() {
    let h = hub();
    let id = RunId::new(1);
    assert_eq!(h.subscriber_count(id), 0);
    let a = h.subscribe(id, Seq::ZERO);
    let b = h.subscribe(id, Seq::ZERO);
    assert_eq!(h.subscriber_count(id), 2);
    h.unsubscribe(id, a);
    assert_eq!(h.subscriber_count(id), 1);
    h.unsubscribe(id, b);
    assert_eq!(h.subscriber_count(id), 0);
}

#[test]
fn drain_of_unknown_subscriber_is_empty() {
    let h = hub();
    let d = h.drain(RunId::new(1), 999);
    assert!(d.events.is_empty());
    assert_eq!(d.dropped, 0);
}

#[test]
fn record_carries_status_and_roundtrips() {
    // RunRecord 的 serde 形状：status 是 camelCase 的 lowercase 值
    let r = RunRecord {
        run_id: RunId::new(1),
        task: "sign".into(),
        account: "138".into(),
        started_at_ms: 0,
        duration_ms: 1,
        outcome: TaskOutcome::success("ok"),
        status: RunStatus::Cancelled,
        details: serde_json::Value::Null,
    };
    let j = serde_json::to_string(&r).unwrap();
    assert!(j.contains("\"status\":\"cancelled\""), "{j}");
}
