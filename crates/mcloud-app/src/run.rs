//! 跨账号编排：把执行器（`mcloud-exec`）的准入表与事件总线接到真实存储上。
//!
//! # 这个模块补的是哪个洞
//!
//! `mcloud-exec` 的四件套（并发闸 / 互斥键 / 准入表 / 事件总线）此前**没有任何调用方**，
//! 而"遍历账号跑一件事"的循环手写在 CLI 里：不产生 `runId`、不过锁、
//! 不发事件、不落 `runs` / `run_events`。于是同一件事在两个地方各有一套语义，
//! 而 WebUI 需要的那套（事件流 + 历史 + 幂等）CLI 那套给不出来。
//!
//! 这里把那两者合成一处：**谁要跨账号跑东西，都走 [`Runner::run`]**。
//!
//! # 一次运行 = N 个账号，每个账号一个 `runId`
//!
//! 粒度选在"账号"上，有三个理由：
//!
//! 1. 互斥键 [`MutexKey`] 本身就是按「能力 + 账号 + 参数指纹」构造的，
//!    准入判定天然是逐账号的；
//! 2. `runs` 表的 `account_id` 是可空列，而旧项目的历史同样是「每任务每账号一行」——
//!    粒度对不上，历史就没法并排看；
//! 3. 一个账号失败不该让另一个账号的结果无处安放。
//!
//! # 顺序执行（v1）
//!
//! **不做并行**，且这是有意的：任务闭包借用 `store` / `clock`，
//! 想并发就得 `spawn`，而 `spawn` 要求 `'static` —— 要么把依赖全换成 `Arc`，
//! 要么自己写 poller。那是一件独立的事，等它能被测试盯住再做。
//! 另外"默认串行"本身是原项目防风控的要求，并行是**显式选择**而非默认行为。
//!
//! # 事件同时走两条路
//!
//! 总线负责**在线的**订阅者（带 `seq`，断了能从 `?after=` 续上），
//! `run_events` 表负责**后来的**人与重启后的续传。两者缺一不可：
//! 只发总线则刷新页面就丢历史，只落库则前端得轮询。
//!
//! 落库按**账号**批量（而不是逐条，也不是整批结束才写）：
//! 崩在最坏的情况下丢一个账号的事件，而不是整次运行的。
//!
//! # 已知缺口：任务侧还写不了日志
//!
//! 任务闭包只拿到账号，拿不到 `runId`，所以它无法自己发 [`RunEvent::Log`]。
//! 现在的留痕只有编排层发的 `started` / `finished` / `progress`。
//! 要补的话应当是给任务传一个**事件出口**（`&mut dyn EventSink` 之类），
//! 而不是把 `runId` 塞进闭包捕获里 —— 后者会让每个调用点都要自己拼事件。

use mcloud_core::{
    AccountId, Clock, Error, Random, RunEvent, RunId, RunOutcome, RunRecord, RunReport, RunStatus,
    Scope, Sequenced, StateStore,
};
use mcloud_exec::{Admission, EventBus, MutexKey, RunRegistry};

/// 一次跨账号运行要跑的东西。
#[derive(Debug, Clone)]
pub struct RunPlan<'a> {
    /// 能力 code（写进 `runs` 行与事件里，前端按它分组）。
    pub capability: &'a str,
    /// 目标账号（调用方已按 [`Scope`] 过滤过 —— 这里不再查一遍库）。
    pub accounts: Vec<AccountId>,
    /// 参数指纹（[`mcloud_exec::params_digest`] 的产物）。
    ///
    /// 参与互斥键：**同一个账号、同一个能力、不同参数**不该互相挡 ——
    /// 手动签到与定时签到用不同参数时是两件事。
    pub params_digest: &'a str,
}

/// 单个账号的结果。
#[derive(Debug, Clone)]
pub enum AccountRun {
    /// 真的跑了。
    Ran {
        account: AccountId,
        run_id: RunId,
        report: RunReport,
    },
    /// 同一把锁被别的 run 占着：**不排队、也不假装成功**。
    ///
    /// 记下占用者，是因为"被谁挡了"比"被挡了"有用得多 —— 排查时能直接找到那一趟。
    Skipped {
        account: AccountId,
        blocked_by: RunId,
    },
}

/// 一次跨账号运行的汇总。
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub capability: String,
    /// 由**真的跑了**的账号合成；被挡下的那些不参与（它们没有结果可合）。
    pub outcome: RunOutcome,
    pub accounts: Vec<AccountRun>,
    /// 落库的事件条数（观测用：SSE 断线续传读的就是它们）。
    pub events: usize,
}

impl RunSummary {
    /// 真的跑了的账号及其结果。
    pub fn ran(&self) -> impl Iterator<Item = (&AccountId, &RunReport)> {
        self.accounts.iter().filter_map(|entry| match entry {
            AccountRun::Ran {
                account, report, ..
            } => Some((account, report)),
            AccountRun::Skipped { .. } => None,
        })
    }

    /// 被锁挡下的账号数。
    pub fn skipped(&self) -> usize {
        self.accounts
            .iter()
            .filter(|entry| matches!(entry, AccountRun::Skipped { .. }))
            .count()
    }

    /// 给人看的一句话。
    ///
    /// 被挡下的账号**必须**出现在这句话里：否则"这次只跑了一个账号"看起来
    /// 跟"另一个账号跑失败了"一模一样，而两者的处置完全不同。
    pub fn summary_line(&self) -> String {
        let ran = self.ran().count();
        let skipped = self.skipped();
        let mut line = format!(
            "{}：跑了 {ran} 个账号，{}",
            self.capability,
            outcome_word(self.outcome)
        );
        if skipped > 0 {
            line.push_str(&format!("，{skipped} 个因正在运行被跳过"));
        }
        line
    }
}

const fn outcome_word(outcome: RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Success => "全部成功",
        RunOutcome::PartialSuccess => "部分成功",
        RunOutcome::ManualRequired => "需要你去客户端操作",
        RunOutcome::Failed => "失败",
    }
}

/// 保证"受理过就一定释放"。
///
/// 用 guard 而不是"记得在每个 `return` 前 release"：后者在加了一行 `?`
/// 之后就会静默漏掉，而漏掉的后果是**那把锁永远被占着** ——
/// 同一个账号从此再也跑不起来，且没有任何报错指向这里。
struct Admitted<'a> {
    registry: &'a RunRegistry,
    run_id: RunId,
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        self.registry.release(&self.run_id);
    }
}

/// 跨账号运行器。
///
/// 持有运行级的共享状态（准入表与事件总线），因为它们的生命周期**比一次 run 长**：
/// 服务端进程活着的时候，"谁正在跑"与"SSE 订阅者"都要跨 run 延续。
///
/// 两者都是**借用**（而不是各自持有一份）：调用方必须让同一个实例被跑任务与
/// 看事件两条路共用 —— 各持一份的话，SSE 那边永远看不到这里发布的事件。
/// 它们按 `&self` 暴露（内部锁，见 `mcloud-exec` 的 crate 头），
/// 所以这里的借用不是独占的：一次运行期间订阅者照样能读。
pub struct Runner<'a, S, K> {
    store: &'a S,
    scope: Scope<'a>,
    clock: &'a K,
    registry: &'a RunRegistry,
    bus: &'a EventBus,
}

impl<'a, S, K> Runner<'a, S, K>
where
    S: StateStore,
    K: Clock,
{
    /// 组装一次编排（`registry` / `bus` 由调用方持有并复用）。
    pub fn new(
        store: &'a S,
        scope: Scope<'a>,
        clock: &'a K,
        registry: &'a RunRegistry,
        bus: &'a EventBus,
    ) -> Self {
        Self {
            store,
            scope,
            clock,
            registry,
            bus,
        }
    }

    /// 按计划跑一遍：逐账号准入 → 执行 → 发事件 → 落库。
    ///
    /// `task` 是"针对单个账号做一件事"，成功给 [`RunReport`]、失败给 [`Error`]。
    /// 单账号失败**不会**中断整批（继续跑下一个），因为"一个账号风控了"不该让
    /// 其余账号今天什么都不做。
    pub async fn run<F, Fut>(
        &mut self,
        plan: RunPlan<'_>,
        rng: &mut impl Random,
        mut task: F,
    ) -> Result<RunSummary, Error>
    where
        F: FnMut(AccountId) -> Fut,
        Fut: std::future::Future<Output = Result<RunReport, Error>>,
    {
        let total = plan.accounts.len() as u32;
        let mut entries = Vec::with_capacity(plan.accounts.len());
        let mut events_written = 0usize;
        let mut done = 0u32;

        for account in plan.accounts {
            let key = MutexKey::new(plan.capability, account.as_str(), plan.params_digest);
            let run_id = RunId::new(rng);

            match self.registry.admit(run_id.clone(), key) {
                Admission::Admitted => {}
                Admission::Blocked { holder, .. } => {
                    entries.push(AccountRun::Skipped {
                        account,
                        blocked_by: holder,
                    });
                    continue;
                }
                // 刚生成的 runId 不该重复；真重复了说明随机源被钉住了（测试里会）。
                // 按"被挡下"处理而不是 panic：编排层不该因为随机源退化就炸掉整批。
                Admission::DuplicateRunId(existing) => {
                    entries.push(AccountRun::Skipped {
                        account,
                        blocked_by: existing,
                    });
                    continue;
                }
            }

            // 从这里到 guard 掉落之间**不要**出现裸 `?`：guard 已经保证释放，
            // 但中途返回会让这一趟的事件与 runs 行缺失（下一趟就查不到了）。
            let admitted = Admitted {
                registry: self.registry,
                run_id: run_id.clone(),
            };
            let started_at = self.clock.now();
            self.bus.open(&run_id);
            let mut batch = Vec::new();
            batch.push(self.publish(RunEvent::started(run_id.clone(), plan.capability)));
            self.store
                .insert_run(
                    self.scope,
                    &RunRecord {
                        id: run_id.as_str().to_string(),
                        account_id: Some(account.clone()),
                        capability: plan.capability.to_string(),
                        // 先记 Running：跑起来就该在历史里看得见，
                        // 而不是等它结束才突然出现（长任务尤其如此）
                        status: RunStatus::Running,
                        started_at,
                        finished_at: None,
                        cloud_gained: None,
                        cloud_after: None,
                        message: None,
                    },
                )
                .await?;

            let result = task(account.clone()).await;
            let (outcome, report) = match result {
                Ok(report) => (report.outcome, report),
                Err(err) => {
                    batch.push(self.publish(RunEvent::errored(run_id.clone(), err.to_string())));
                    (RunOutcome::Failed, RunReport::failed(err.to_string()))
                }
            };

            // `finished` 收的是切片（它自己会克隆），别在这里多克隆一次
            batch.push(self.publish(RunEvent::finished(
                run_id.clone(),
                outcome,
                &report.manual_steps,
                report.summary.clone(),
            )));
            done += 1;
            batch.push(self.publish(RunEvent::progress(run_id.clone(), done, total)));

            let finished_at = self.clock.now();
            self.store
                .update_run(
                    self.scope,
                    &RunRecord {
                        id: run_id.as_str().to_string(),
                        account_id: Some(account.clone()),
                        capability: plan.capability.to_string(),
                        status: outcome.to_legacy_status(),
                        started_at,
                        finished_at: Some(finished_at),
                        cloud_gained: None,
                        cloud_after: None,
                        message: report.summary.clone(),
                    },
                )
                .await?;

            // 落库在逐个账号的边界上：崩了最多丢一个账号的事件，而不是整批
            events_written += self.persist(&run_id, &batch).await?;
            drop(admitted);

            entries.push(AccountRun::Ran {
                account,
                run_id,
                report,
            });
        }

        let results: Vec<RunReport> = entries
            .iter()
            .filter_map(|entry| match entry {
                AccountRun::Ran { report, .. } => Some(report.clone()),
                AccountRun::Skipped { .. } => None,
            })
            .collect();

        Ok(RunSummary {
            capability: plan.capability.to_string(),
            outcome: RunOutcome::combine(&results),
            accounts: entries,
            events: events_written,
        })
    }

    /// 发一条事件到总线，返回**带序号的副本**（落库要用它）。
    ///
    /// 序号由总线发，落库的 `(run_id, seq)` 主键因此与在线的 SSE 流一致 ——
    /// 断线重连时 `?after=<seq>` 才能接得上。
    fn publish(&mut self, event: RunEvent) -> Sequenced {
        let (seq, _) = self.bus.publish(event.clone());
        Sequenced::new(seq, event)
    }

    /// 把一批事件写进 `run_events`。
    async fn persist(&self, run_id: &RunId, batch: &[Sequenced]) -> Result<usize, Error> {
        if batch.is_empty() {
            return Ok(0);
        }
        self.store
            .append_run_events(self.scope, run_id, batch)
            .await?;
        Ok(batch.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use mcloud_core::{Account, FixedClock, Role, User, UserStore};
    use mcloud_io::SqliteStore;

    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    fn uid() -> mcloud_core::UserId {
        mcloud_core::UserId::new("alice")
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap()
    }

    fn account(suffix: &str) -> AccountId {
        AccountId::new(format!("19****{suffix}"))
    }

    /// 一个用户 + 两个归他的账号。
    ///
    /// 账号必须真的存在：`runs` 的读写是按归属过滤的（`account_id IN (SELECT … owner = ?)`），
    /// 拿一个不存在的账号 id 去写，得到的不是"写进去了"，而是"看不见"。
    async fn store_with_accounts() -> SqliteStore {
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .upsert_user(&User::new("alice", "alice", "!", Role::Admin, now()))
            .await
            .unwrap();
        for suffix in ["01", "02"] {
            let mut owned = Account::new(account(suffix).as_str());
            owned.owner = Some(uid());
            store
                .upsert_account(Scope::user(&uid()), &owned)
                .await
                .unwrap();
        }
        store
    }

    fn plan<'a>(capability: &'a str, accounts: Vec<AccountId>, digest: &'a str) -> RunPlan<'a> {
        RunPlan {
            capability,
            accounts,
            params_digest: digest,
        }
    }

    fn ok_report() -> RunReport {
        RunReport::success()
    }

    fn manual_report() -> RunReport {
        RunReport {
            outcome: RunOutcome::ManualRequired,
            manual_steps: Vec::new(),
            summary: Some("要去 App 打开消息通知".into()),
        }
    }

    #[tokio::test]
    async fn runs_every_account_and_records_runs_and_events() {
        let store = store_with_accounts().await;
        let clock = FixedClock::parse("2026-09-20T00:00:00Z");
        let registry = RunRegistry::new();
        let bus = EventBus::new();
        let mut rng = SeqRandom(1);

        let user = uid();
        let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
        let summary = runner
            .run(
                plan("signin", vec![account("01"), account("02")], "d0"),
                &mut rng,
                |_account| async { Ok(ok_report()) },
            )
            .await
            .unwrap();

        assert_eq!(summary.outcome, RunOutcome::Success);
        assert_eq!(summary.ran().count(), 2);
        assert_eq!(summary.skipped(), 0);
        assert!(
            summary.summary_line().contains("全部成功"),
            "{}",
            summary.summary_line()
        );

        // 历史：每账号一行，且都已终结
        let runs = store.list_runs(Scope::user(&uid()), 10).await.unwrap();
        assert_eq!(runs.len(), 2, "每账号一行");
        for run in &runs {
            assert_eq!(run.capability, "signin");
            assert!(run.status.is_terminal(), "跑完不能还是 Running: {run:?}");
            assert!(run.finished_at.is_some());
        }

        // 事件：每个账号一条流，且能按 seq 续传
        for entry in &summary.accounts {
            let AccountRun::Ran { run_id, .. } = entry else {
                panic!("两个都该真跑");
            };
            let events = store
                .run_events_after(Scope::user(&uid()), run_id, None, 100)
                .await
                .unwrap();
            assert!(
                events.len() >= 3,
                "started + finished + progress: {events:?}"
            );
            let seqs: Vec<u64> = events.iter().map(|e| e.seq.value()).collect();
            let mut sorted = seqs.clone();
            sorted.sort_unstable();
            assert_eq!(seqs, sorted, "序号必须递增");
            assert!(matches!(events[0].event, RunEvent::Started { .. }));
            assert!(matches!(
                events.last().unwrap().event,
                RunEvent::Progress { .. }
            ));
        }
    }

    #[tokio::test]
    async fn one_failing_account_does_not_stop_the_others() {
        let store = store_with_accounts().await;
        let clock = FixedClock::parse("2026-09-20T00:00:00Z");
        let registry = RunRegistry::new();
        let bus = EventBus::new();
        let mut rng = SeqRandom(1);
        let bad = account("02");
        // 闭包是 `move` 的，所以给它一份副本；`bad` 本身后面还要用来查历史
        let failed_account = bad.clone();

        let user = uid();
        let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
        let summary = runner
            .run(
                plan("signin", vec![account("01"), bad.clone()], "d0"),
                &mut rng,
                move |target: AccountId| {
                    let fails = target == failed_account.clone();
                    async move {
                        if fails {
                            Err(Error::BadRequest("模拟上游失败".into()))
                        } else {
                            Ok(ok_report())
                        }
                    }
                },
            )
            .await
            .unwrap();

        assert_eq!(summary.outcome, RunOutcome::PartialSuccess);
        assert_eq!(
            summary.ran().count(),
            2,
            "失败的账号也要有结果，只是结果是失败"
        );

        let runs = store.list_runs(Scope::user(&uid()), 10).await.unwrap();
        let failed = runs
            .iter()
            .find(|r| r.account_id.as_ref() == Some(&bad))
            .unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        // 失败原因要留下（否则历史里只有"失败"两个字）
        assert!(
            failed
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("模拟上游失败"),
            "{:?}",
            failed.message
        );

        // 失败的那个账号也有 errored 事件
        let run_id = runs
            .iter()
            .find(|r| r.account_id.as_ref() == Some(&bad))
            .map(|r| RunId::parse(&r.id).unwrap())
            .unwrap();
        let events = store
            .run_events_after(Scope::user(&uid()), &run_id, None, 100)
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e.event, RunEvent::Errored { .. })),
            "出错要有 errored 事件"
        );
    }

    #[tokio::test]
    async fn needing_a_human_outranks_a_plain_failure() {
        let store = store_with_accounts().await;
        let clock = FixedClock::parse("2026-09-20T00:00:00Z");
        let registry = RunRegistry::new();
        let bus = EventBus::new();
        let mut rng = SeqRandom(1);
        let manual = account("01");

        let user = uid();
        let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
        let summary = runner
            .run(
                plan("signin", vec![manual.clone(), account("02")], "d0"),
                &mut rng,
                move |target: AccountId| {
                    let is_manual = target == manual.clone();
                    async move {
                        if is_manual {
                            Ok(manual_report())
                        } else {
                            Err(Error::BadRequest("另一个失败了".into()))
                        }
                    }
                },
            )
            .await
            .unwrap();

        assert_eq!(summary.outcome, RunOutcome::ManualRequired);
        assert!(summary.outcome.needs_human_action());
        assert!(
            summary.summary_line().contains("客户端操作"),
            "{}",
            summary.summary_line()
        );
    }

    #[tokio::test]
    async fn a_held_lock_skips_only_that_account() {
        let store = store_with_accounts().await;
        let clock = FixedClock::parse("2026-09-20T00:00:00Z");
        let registry = RunRegistry::new();
        let bus = EventBus::new();
        let mut rng = SeqRandom(1);

        // 先让别人占住 01 那把锁（同一个能力 + 账号 + 参数指纹）
        let holder = RunId::new(&mut rng);
        let key = MutexKey::new("signin", account("01").as_str(), "d0");
        assert_eq!(registry.admit(holder.clone(), key), Admission::Admitted);

        let user = uid();
        let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
        let summary = runner
            .run(
                plan("signin", vec![account("01"), account("02")], "d0"),
                &mut rng,
                |_account| async { Ok(ok_report()) },
            )
            .await
            .unwrap();

        assert_eq!(summary.skipped(), 1, "被占的那个跳过，另一个照跑");
        assert_eq!(summary.ran().count(), 1);
        // 跳过不是失败：总判定只看真的跑了的那个
        assert_eq!(summary.outcome, RunOutcome::Success);
        // 而且必须能看出"被谁挡了"
        let skipped = summary
            .accounts
            .iter()
            .find_map(|entry| match entry {
                AccountRun::Skipped { blocked_by, .. } => Some(blocked_by.clone()),
                AccountRun::Ran { .. } => None,
            })
            .unwrap();
        assert_eq!(skipped, holder);
        // 那句给人看的话里也要有它，否则"只跑了一个"看起来像"另一个失败了"
        assert!(
            summary.summary_line().contains("跳过"),
            "{}",
            summary.summary_line()
        );

        // 被挡下的账号不该产生 runs 行（它没跑）
        let runs = store.list_runs(Scope::user(&uid()), 10).await.unwrap();
        assert_eq!(runs.len(), 1);
    }

    #[tokio::test]
    async fn locks_are_released_even_when_the_task_fails() {
        let store = store_with_accounts().await;
        let clock = FixedClock::parse("2026-09-20T00:00:00Z");
        let registry = RunRegistry::new();
        let bus = EventBus::new();
        let mut rng = SeqRandom(1);
        let user = uid();

        {
            let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
            runner
                .run(
                    plan("signin", vec![account("01")], "d0"),
                    &mut rng,
                    |_account| async { Err(Error::Internal("炸了".into())) },
                )
                .await
                .unwrap();
        }

        assert_eq!(
            registry.running(),
            0,
            "跑完必须把锁放掉，否则这个账号再也跑不起来"
        );

        // 再跑同一个账号：不该被判成"有人在跑"
        let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
        let summary = runner
            .run(
                plan("signin", vec![account("01")], "d0"),
                &mut rng,
                |_account| async { Ok(ok_report()) },
            )
            .await
            .unwrap();
        assert_eq!(summary.skipped(), 0);
        assert_eq!(summary.outcome, RunOutcome::Success);
    }

    #[tokio::test]
    async fn different_params_do_not_block_each_other() {
        let store = store_with_accounts().await;
        let clock = FixedClock::parse("2026-09-20T00:00:00Z");
        let registry = RunRegistry::new();
        let bus = EventBus::new();
        let mut rng = SeqRandom(1);

        let holder = RunId::new(&mut rng);
        // 同一个账号、同一个能力，但**参数不同**
        let key = MutexKey::new("signin", account("01").as_str(), "别的参数");
        assert_eq!(registry.admit(holder, key), Admission::Admitted);

        let user = uid();
        let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
        let summary = runner
            .run(
                plan("signin", vec![account("01")], "d0"),
                &mut rng,
                |_account| async { Ok(ok_report()) },
            )
            .await
            .unwrap();

        assert_eq!(summary.skipped(), 0, "参数不同是两件事，不该互相挡");
    }

    #[tokio::test]
    async fn empty_targets_are_reported_as_failed() {
        let store = store_with_accounts().await;
        let clock = FixedClock::parse("2026-09-20T00:00:00Z");
        let registry = RunRegistry::new();
        let bus = EventBus::new();
        let mut rng = SeqRandom(1);

        let user = uid();
        let mut runner = Runner::new(&store, Scope::user(&user), &clock, &registry, &bus);
        let summary = runner
            .run(
                plan("signin", Vec::new(), "d0"),
                &mut rng,
                |_account| async { Ok(ok_report()) },
            )
            .await
            .unwrap();

        assert_eq!(
            summary.outcome,
            RunOutcome::Failed,
            "一个账号都没跑不算成功"
        );
        assert!(
            store
                .list_runs(Scope::user(&uid()), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
