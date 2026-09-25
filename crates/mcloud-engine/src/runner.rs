//! 运行器：并发闸 + 事件扇出 + 历史落盘。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mcloud_core::{
    Account, AccountId, Clock, EventSink, HttpClient, Result, RunEvent, RunId, RunRecord, Store,
    TaskOutcome,
};

use crate::{Registry, TaskContext};

/// 按账号构造 HTTP 客户端的工厂。
///
/// 凭证绑定在客户端上，因此每个账号需要独立实例；用具名别名而非
/// 内联写 `Arc<dyn Fn...>`，既让签名可读，也避免 clippy 的复杂度告警。
pub type HttpClientFactory = Arc<dyn Fn(&Account) -> Arc<dyn HttpClient> + Send + Sync>;

/// 任务运行器。
///
/// 并发闸以 `task:account` 为键：同一账号的同一任务不会并发执行，
/// 但不同任务可以并行（例如签到与备份互不阻塞）。
pub struct Runner {
    registry: Arc<Registry>,
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    sink: Arc<dyn EventSink>,
    http_for: HttpClientFactory,
    next_id: AtomicU64,
    inflight: Mutex<HashMap<String, RunId>>,
}

impl Runner {
    pub fn new(
        registry: Arc<Registry>,
        store: Arc<dyn Store>,
        clock: Arc<dyn Clock>,
        sink: Arc<dyn EventSink>,
        http_for: HttpClientFactory,
    ) -> Self {
        Self {
            registry,
            store,
            clock,
            sink,
            http_for,
            next_id: AtomicU64::new(1),
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// 从历史记录续上 run id，避免重启后重复。
    pub fn seed_run_id(&self, id: u64) {
        self.next_id.fetch_max(id + 1, Ordering::SeqCst);
    }

    /// 该 `task:account` 是否正在运行。
    pub fn is_running(&self, task: &str, account: &AccountId) -> bool {
        let key = Self::key(task, account);
        self.inflight
            .lock()
            .expect("inflight 锁中毒")
            .contains_key(&key)
    }

    fn key(task: &str, account: &AccountId) -> String {
        format!("{task}:{account}")
    }

    /// 执行一个任务并落盘记录。
    ///
    /// 若同 `task:account` 已在运行，返回 `Skipped`，不排队——排队会造成
    /// 定时任务堆积，掩盖真实问题。
    pub async fn run(
        &self,
        task_name: &str,
        account: &Account,
        body: &serde_json::Value,
    ) -> Result<RunRecord> {
        let task = self
            .registry
            .get(task_name)
            .ok_or_else(|| mcloud_core::Error::Config(format!("未知任务：{task_name}")))?;

        let key = Self::key(task_name, &account.id);
        {
            let mut inflight = self.inflight.lock().expect("inflight 锁中毒");
            if inflight.contains_key(&key) {
                return Ok(self.record(
                    RunId::new(0),
                    task_name,
                    account,
                    0,
                    TaskOutcome::skipped("同一任务正在运行中"),
                ));
            }
            inflight.insert(key.clone(), RunId::new(0));
        }

        let run_id = RunId::new(self.next_id.fetch_add(1, Ordering::SeqCst));
        {
            let mut inflight = self.inflight.lock().expect("inflight 锁中毒");
            inflight.insert(key.clone(), run_id);
        }

        let started = self.clock.now_ms();
        self.sink.emit(RunEvent::Started {
            run_id,
            task: task_name.to_string(),
            account: account.id.to_string(),
            at_ms: started,
        });

        let ctx = TaskContext {
            account: account.clone(),
            http: (self.http_for)(account),
            store: Arc::clone(&self.store),
            clock: Arc::clone(&self.clock),
            sink: Arc::clone(&self.sink),
            run_id,
        };

        let outcome = match task.run(&ctx, body).await {
            Ok(o) => o,
            Err(e) => TaskOutcome::failed(e.to_string()),
        };

        let duration_ms = (self.clock.now_ms() - started).max(0) as u64;
        self.sink.emit(RunEvent::Finished {
            run_id,
            outcome: outcome.clone(),
            duration_ms,
        });

        self.inflight.lock().expect("inflight 锁中毒").remove(&key);

        let record = self.record(run_id, task_name, account, duration_ms, outcome);
        // 历史落盘失败不应让整次运行算失败——结果已经产生。
        if let Err(e) = self.store.append_run(&record).await {
            tracing::warn!(error = %e, "运行历史落盘失败");
        }
        Ok(record)
    }

    fn record(
        &self,
        run_id: RunId,
        task: &str,
        account: &Account,
        duration_ms: u64,
        outcome: TaskOutcome,
    ) -> RunRecord {
        RunRecord {
            run_id,
            task: task.to_string(),
            account: account.id.to_string(),
            started_at_ms: self.clock.now_ms() - duration_ms as i64,
            duration_ms,
            outcome,
            details: serde_json::Value::Null,
        }
    }
}
