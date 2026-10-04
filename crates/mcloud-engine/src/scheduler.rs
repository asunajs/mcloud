//! 定时调度器。
//!
//! # 三条硬规则（都是踩过坑才立下的）
//!
//! ## 1. 先推水位，再打上游
//!
//! 水位（`last_run_ms`）必须在**触发上游之前**落盘。反过来的话，
//! 进程在跑任务时被杀 ⇒ 重启后同一个时间点会**再跑一次**，
//! 而签到、用口令领小红花这类动作在上游那侧**不是幂等的**。
//!
//! 结论：**宁可漏跑一次，不可重跑一次。**
//!
//! ## 2. 错过的整串一次弃掉，绝不逐点补跑
//!
//! 停机 8 小时后再启动，若把错过的每个点都补一遍，等于"连打 8 趟上游"。
//! 所以只判断"最近这个点是不是已经过了很久"，是就整体放弃、把水位推到当前，
//! 而不是逐点追。
//!
//! ## 3. 迟到一点不算错过
//!
//! 一轮 tick 被写盘顶住是常态（几毫秒到几十毫秒），那一次迟到
//! 不该把一条准点任务判成"错过"并弃掉。用 [`GRACE_MS`] 这个宽限窗口
//! 把"被顶了一下"和"进程真的停了"分开。
//!
//! # 与"任务还在跑"的关系
//!
//! 到点时同一 `任务+账号` 还在跑 ⇒ 本次**跳过，不排队**。
//! 排队会让定时任务在慢上游面前无限堆积，掩盖真实问题。

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use tokio::sync::watch;

use mcloud_core::{AccountId, Clock, Error, Result, Schedule, ScheduleId, Store};

use crate::runner::Runner;

/// tick 间隔。
///
/// 30 秒：cron 最小精度是分钟，30 秒能保证"每分钟的点最多迟到 30 秒"，
/// 同时不会让空转开销变得可观。
pub const TICK: Duration = Duration::from_secs(30);

/// 宽限窗口。
///
/// 取 **2 个 tick**（60 秒）而不是 1 个：一轮被写盘顶住而迟到是常态，
/// 60 秒足够把"被顶了一下"和"进程真的停了"分开 ——
/// 真停机迟到的是分钟到小时级，一定越过这条线。
pub const GRACE_MS: i64 = 60_000;

/// 调度器的运行状态，供 `/api/status` 报告**运行时实况**。
///
/// 刻意不是配置项：配置写着 enabled 而循环其实没跑，正是最难发现的失真。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerState {
    /// 循环正在跑。
    Running,
    /// 循环已停止。
    Stopped,
}

/// 一次触发尝试的结果。
/// 排期里表示"这一条跑的是**编排**"的约定任务名。
///
/// ⚠ 编排不是注册表里的任务（`Task::name`）✗，所以不能像任务那样按名字派发。
/// 用约定的任务名 + `body.pipelineId` 表达，**不动数据模型** ✓。
pub const PIPELINE_TASK: &str = "pipeline";

#[derive(Debug, Clone, PartialEq, Eq)]

pub enum FireOutcome {
    /// 触发成功，已交给运行器。
    Fired { run_id: u64 },
    /// 触发成功，已交给**编排运行器**。
    ///
    /// 与 [`FireOutcome::Fired`] 分开是**刻意的** ✗：编排没有单次运行的 `run_id`
    /// （它内部要跑 N 步、可能跨 N 个账号），拿 `Fired { run_id: 0 }` 冒充会撞上
    /// 别处 `run_id == 0` 表示"被并发闸挡住"的语义 ✗ —— 那是两种完全不同的结果。
    FiredPipeline,
    /// 到点了，但**已经交给后台任务**，结论还没出来。
    ///
    /// 与 [`FireOutcome::Fired`] 分开是刻意的：那个是"运行器已经收下并给出
    /// `run_id`"，这个是"我们还没等到它的结论"（守护循环专用，2026-10-04 复审 `sv-2`）。
    Spawned,
    /// 到点了，但同一 `任务+账号` 正在跑 —— 跳过，不排队。
    SkippedBusy,
    /// 错过的太久，整体弃掉（水位已推到当前）。
    AbandonedMissed,
    /// 任务名不在注册表里。
    UnknownTask(String),
}

/// 单条定时的触发判定。
///
/// 抽成纯函数（不碰 IO），因为它是最容易算错的地方，
/// 而算错的后果很具体：**漏跑或重跑**。
pub fn decide(schedule: &Schedule, now_ms: i64) -> std::result::Result<Decision, String> {
    if !schedule.enabled {
        return Ok(Decision::Disabled);
    }

    let tz: chrono::FixedOffset = parse_timezone(&schedule.timezone)?;
    let expr = normalize_expr(&schedule.expr)?;
    let cron = cron::Schedule::from_str(&expr).map_err(|e| format!("cron 解析失败：{e}"))?;

    // 水位：上次已处理过的触发时刻。
    // 没有任何记录时，从"宽限窗口之前"开始 —— 既不回填历史，
    // 也不会因为起点恰好等于 now 而永远等不到下一个点。
    let watermark = schedule.last_fire_ms.unwrap_or(now_ms - GRACE_MS);

    let after = Utc
        .timestamp_millis_opt(watermark)
        .single()
        .ok_or_else(|| format!("水位时间戳非法：{watermark}"))?
        .with_timezone(&tz);

    // 下一个应触发的点（严格晚于水位）
    let Some(next) = cron.after(&after).next() else {
        // 表达式永不触发（例如 2 月 30 日）。这不是错误，是"这条定时是死的"。
        return Ok(Decision::NeverFires);
    };
    let next_ms = next.with_timezone(&Utc).timestamp_millis();

    if next_ms > now_ms {
        return Ok(Decision::NotYet { next_ms });
    }

    // 到点了。判断是不是"错过太久"
    let late_by = now_ms - next_ms;
    if late_by > GRACE_MS {
        // 整串弃掉：把水位推到 now，下次从当前时刻之后重新算
        return Ok(Decision::Missed { point_ms: next_ms });
    }

    Ok(Decision::Due { point_ms: next_ms })
}

/// [`decide`] 的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 未启用。
    Disabled,
    /// 还没到点。带上下次时间供前端展示。
    NotYet { next_ms: i64 },
    /// 到点，应触发。
    Due { point_ms: i64 },
    /// 错过太久，应整体弃掉。
    Missed { point_ms: i64 },
    /// 表达式永不触发。
    NeverFires,
}

/// 把 5 段 cron 规范成 6 段（补秒）。
///
/// ⚠ 补的是 **`0` 秒，不是 `*`**。补 `*` 会把"每天 7:30"变成
/// "7:30 那一分钟里每秒跑一次"—— 60 倍的请求量，而且完全不报错。
///
/// `cron` crate 只吃 6 段，而用户习惯写 POSIX 的 5 段（`0 8 * * *`），
/// 所以规范化放在解析入口，而不是要求用户改习惯。
fn normalize_expr(expr: &str) -> std::result::Result<String, String> {
    let trimmed = expr.trim();
    let fields = trimmed.split_whitespace().count();
    match fields {
        5 => Ok(format!("0 {trimmed}")),
        6 => Ok(trimmed.to_string()),
        n => Err(format!(
            "cron 表达式需要 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周），当前 {n} 段"
        )),
    }
}

/// 解析时区名。只支持固定偏移与 UTC —— 不引 `chrono-tz`。
///
/// 为什么不上完整时区库：本项目的默认值是 `Asia/Shanghai`（固定 +08:00，
/// 无夏令时），而完整时区库会显著增加编译时间与依赖面。
/// 需要真实 DST 语义时再引，且**在那之前**要明确报错而不是静默按 UTC 算 ——
/// 静默按 UTC 会让任务在错误的钟点跑，且没有任何提示。
pub fn parse_timezone(name: &str) -> std::result::Result<chrono::FixedOffset, String> {
    match name {
        "UTC" | "Etc/UTC" => Ok(chrono::FixedOffset::east_opt(0).expect("0 偏移合法")),
        "Asia/Shanghai" | "Asia/Chongqing" | "PRC" => {
            Ok(chrono::FixedOffset::east_opt(8 * 3600).expect("+8 合法"))
        }
        "Asia/Tokyo" => Ok(chrono::FixedOffset::east_opt(9 * 3600).expect("+9 合法")),
        // 显式接受 "UTC+8" / "+08:00" 这类写法
        other => parse_offset(other).ok_or_else(|| {
            format!("不支持的时区：{other}（当前支持 UTC / Asia/Shanghai / Asia/Tokyo / +HH:MM）")
        }),
    }
}

fn parse_offset(s: &str) -> Option<chrono::FixedOffset> {
    let s = s.strip_prefix("UTC").unwrap_or(s);
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, s.strip_prefix('+')?),
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h, m),
        None => (rest, "0"),
    };
    let hours: i32 = h.parse().ok()?;
    let mins: i32 = m.parse().ok()?;
    if hours > 23 || mins > 59 {
        return None;
    }
    chrono::FixedOffset::east_opt(sign * (hours * 3600 + mins * 60))
}

/// 调度器。
pub struct Scheduler {
    store: Arc<dyn Store>,
    runner: Arc<Runner>,
    /// 编排运行器。**定时要能拉起一条编排**（用户口径 2026-10-02：编排只能手动点
    /// 运行没有意义 ⇒「定时跑编排」才是它的命）。
    ///
    /// ⚠ 与 `/api/pipelines/{id}/run` **共用同一个实例**（`AppHandle::pipelines`）✓ ——
    /// 编排语义、并发闸、取消登记表都在它里面，各建一份迟早分叉 ✗。
    pipelines: Arc<crate::pipeline::PipelineRunner>,
    clock: Arc<dyn Clock>,
    /// 广播自己的运行状态，供 `/api/status` 读运行时实况。
    state_tx: watch::Sender<SchedulerState>,
    /// 自己持有一份接收端，保证 `state_tx.send` 不会因为"无人订阅"而失败。
    _state_rx: watch::Receiver<SchedulerState>,
    /// ⚠ **发送端必须活着**，否则 `run_loop` 会空转烧满一核 —— 见 `new()` 与 `stop()`。
    stop_tx: watch::Sender<bool>,
    /// 让 `run_loop` 能被告知停止。
    stop_rx: watch::Receiver<bool>,
}

impl Scheduler {
    pub fn new(
        store: Arc<dyn Store>,
        runner: Arc<Runner>,
        clock: Arc<dyn Clock>,
        pipelines: Arc<crate::pipeline::PipelineRunner>,
    ) -> Self {
        // ⚠ 必须留住接收端：`watch::Sender::send` 在**没有接收者**时返回 Err，
        // 而被忽略的 Err 会让状态永远停在 Stopped —— 界面上就是
        // "调度器明明在跑，却显示已停止"。
        let (state_tx, state_rx) = watch::channel(SchedulerState::Stopped);
        // ⚠ 停止通道的**发送端必须跟着结构体一起活**。
        //
        // `watch::Receiver::changed()` 在发送端被 drop 之后会**立刻**返回
        // `Err(RecvError)`，而 `Err` 也是个"就绪"值 ⇒ `run_loop` 的 `select!`
        // 每次都选中停止分支、`*stop.borrow()` 又是 `false`，于是空转烧满一核。
        // 2026-09-30 实测：:3000 的 debug 服务空载一核跑满、单线程累计 187 分钟
        // 用户态 CPU，`sample` 的热点栈正是
        // `Scheduler::run_loop` → `poll_fn` → `watch::Receiver::changed`。
        // 原来这里写成 `let (_stop_tx, stop_rx) = ...`，发送端当场就没了。
        let (stop_tx, stop_rx) = watch::channel(false);
        Self {
            store,
            runner,
            pipelines,
            clock,
            state_tx,
            _state_rx: state_rx,
            stop_tx,
            stop_rx,
        }
    }

    /// 运行状态的订阅端，供 `/api/status` 用。
    pub fn state_rx(&self) -> watch::Receiver<SchedulerState> {
        self.state_tx.subscribe()
    }

    pub fn state(&self) -> SchedulerState {
        *self.state_tx.borrow()
    }

    /// 跑一个 tick：检查所有定时，该触发的触发。
    ///
    /// 返回本次的触发结果，便于测试与日志。
    ///
    /// ⚠ 这是**同步等结论**的那一版（CLI 的 `cron` 与测试要的就是它）。
    /// 守护循环走 [`Self::tick_inner`] 的 `Some(..)` 分支 —— 见那里的说明。
    pub async fn tick(&self) -> Result<Vec<(ScheduleId, FireOutcome)>> {
        self.tick_inner(None).await
    }

    /// `tick()` 的共享实现：`spawner` 为 `Some` 时，到点的那条**丢进后台任务**
    /// 立刻返回（守护循环用）；为 `None` 时**当场跑完**再返回（CLI / 测试要的是
    /// 最终结论）。
    ///
    /// ⚠⚠ 为什么守护循环不能等（2026-10-04 复审 `sv-2`，[高]）：`fire()` 要把一条
    /// 定时的**所有账号串行跑完**（上游限流要的），而一条慢任务（上游慢、或等到整点）
    /// 能让这一拍占住几十秒到几分钟；`run_loop` 却是
    /// `interval.tick() => tick_and_log().await` ⇒ **下一拍被整体推后**，
    /// 这期间到点的**其它**定时在下一拍里已经超出宽限窗口 ⇒ 被判
    /// `Decision::Missed` **弃掉**（水位推到 now），用户看到的是"某天那个定时没跑"。
    ///
    /// 拆开后判定所用的 `now` **没变**（仍是拍头那一刻，与旧实现一致 ⇒ 谁该跑
    /// 还是谁该跑），变的只是"跑"不再占住循环；同一条定时内部的账号**依旧串行** ✓
    /// （那是上游限流要的，不能改成并发）。
    async fn tick_inner(
        &self,
        spawner: Option<&Arc<Self>>,
    ) -> Result<Vec<(ScheduleId, FireOutcome)>> {
        let now = self.clock.now_ms();
        let schedules = self.store.schedules().await?;
        let mut results = Vec::new();

        for sch in schedules {
            let decision = match decide(&sch, now) {
                Ok(d) => d,
                Err(e) => {
                    // 表达式/时区坏了：**推水位**并记录，避免每个 tick 重放同一条错误。
                    // 但如果是"解析不出来"，则不推 —— 那是一条能修好的数据，
                    // 推了等于把它判死。
                    tracing::warn!(schedule = %sch.id, error = %e, "定时配置有误，本轮跳过");
                    results.push((sch.id.clone(), FireOutcome::UnknownTask(e)));
                    continue;
                }
            };

            match decision {
                Decision::Disabled | Decision::NotYet { .. } | Decision::NeverFires => {}
                Decision::Missed { point_ms } => {
                    // 先推水位（推到 now），再记账。顺序反了会在中途崩溃时重复弃
                    // ⚠ `false`：弃置**不算**跑过一次（否则「累计触发 N 次」虚高）
                    self.advance_watermark(&sch, now, false).await?;
                    // 这一趟没有运行 ⇒ `run_ms` 传 None（不动上次运行时间）
                    self.record(&sch, None, "skipped", Some("错过的触发点已整体弃掉"))
                        .await;
                    tracing::warn!(
                        schedule = %sch.id,
                        point_ms,
                        late_ms = now - point_ms,
                        "错过的触发点已整体弃掉（不逐点补跑）"
                    );
                    results.push((sch.id.clone(), FireOutcome::AbandonedMissed));
                }
                Decision::Due { point_ms } => {
                    // ★ 先推水位，再打上游 —— 这是本文件最重要的一行顺序
                    self.advance_watermark(&sch, point_ms, true).await?;
                    match spawner {
                        // 守护循环：丢进后台，**别占住这一拍**（见上面那段说明）
                        Some(this) => {
                            let this = Arc::clone(this);
                            let sch_for_task = sch.clone();
                            tokio::spawn(async move {
                                let (outcome, _blocked) = this.fire(&sch_for_task, point_ms).await;
                                tracing::info!(
                                    schedule = %sch_for_task.id,
                                    ?outcome,
                                    "定时触发（后台）"
                                );
                            });
                            results.push((sch.id.clone(), FireOutcome::Spawned));
                        }
                        // CLI / 测试：当场跑完，把真实结论交回去
                        None => {
                            let (outcome, _blocked) = self.fire(&sch, point_ms).await;
                            results.push((sch.id.clone(), outcome));
                        }
                    }
                }
            }
        }
        Ok(results)
    }

    /// 推水位。
    ///
    /// 用**只改水位**的窄写，而不是整行覆盖：整行覆盖会把用户在这期间
    /// 通过 WebUI 改过的名字/参数一起抹掉（调度器手里那行是几秒前读的）。
    ///
    /// `counted` = 这一次触发点**算不算跑过一次**（`run_count` 那一列）。
    /// ⚠ 弃置错过的触发点（`Decision::Missed`）要传 `false`：那一支一次上游都没打，
    /// 记成"跑过一次"会让界面上的「累计触发 N 次」虚高（2026-10-03 review §1.7）。
    async fn advance_watermark(&self, sch: &Schedule, point_ms: i64, counted: bool) -> Result<()> {
        if counted {
            self.store.mark_schedule_fired(&sch.id, point_ms).await
        } else {
            self.store
                .advance_schedule_watermark(&sch.id, point_ms)
                .await
        }
    }

    /// 记账：本次执行的结果。
    ///
    /// 独立于水位线：水位线在触发**前**就推进了，而结果要等任务跑完才有。
    /// 两者用不同的窄写，因为它们改的是不同的列、发生在不同的时刻。
    async fn record(
        &self,
        sch: &Schedule,
        ran_at_ms: Option<i64>,
        status: &str,
        error: Option<&str>,
    ) {
        if let Err(e) = self
            .store
            .record_schedule_run(&sch.id, ran_at_ms, status, error)
            .await
        {
            // 记账失败不该影响已经跑完的任务
            tracing::warn!(schedule = %sch.id, error = %e, "定时执行结果记账失败");
        }
    }

    /// 真正触发：对定时指向的每个账号起一次运行。
    ///
    /// 触发一条**编排**（`sch.task == PIPELINE_TASK`）。
    ///
    /// # 三点与任务分支**刻意不同**
    ///
    /// 1. **整份账号**交给编排器 ✓ —— 不逐个账号跑一遍 ✗：编排自己按 `accountMode`
    ///    依次/并发走账号（`pipeline.rs` 里那套），外层再套一层会把它跑 N 遍。
    /// 2. 取编排**按归属**（`pipeline_for`）✓ —— 取不到就**如实报错** ✓，不静默跳过 ✗
    ///    （老数据没有 owner 才退回不带归属的 `pipeline`）。
    /// 3. 记账沿用 `record` ✓：编排整体 `Partial` / `Error` **都记 error** ✗ ——
    ///    定时没人当场看着，"一半没成"记成 ok 就等于把它藏起来 ✓（同 `fire` 那条取向）。
    async fn fire_pipeline(
        &self,
        sch: &Schedule,
        accounts: &[mcloud_core::Account],
        point_ms: i64,
    ) -> (FireOutcome, bool) {
        let Some(id) = sch.body.get("pipelineId").and_then(|v| v.as_str()) else {
            return (
                FireOutcome::UnknownTask("这条排期说要跑编排，但 body 里没有 pipelineId".into()),
                false,
            );
        };
        let id = mcloud_core::PipelineId::new(id);
        let found = match sch.owner.as_ref() {
            Some(owner) => self.store.pipeline_for(owner, &id).await,
            None => self.store.pipeline(&id).await,
        };
        let pipeline = match found {
            Ok(Some(p)) => p,
            Ok(None) => {
                return (
                    FireOutcome::UnknownTask(format!("编排不存在或不属于这个用户：{id}")),
                    false,
                )
            }
            Err(e) => return (FireOutcome::UnknownTask(e.to_string()), false),
        };

        match self.pipelines.run(&pipeline, accounts, None).await {
            Ok(outcome) => {
                let status = if outcome.status() == crate::pipeline::StepStatus::Ok {
                    "ok"
                } else {
                    "error"
                };
                let err = if status == "ok" {
                    None
                } else {
                    Some(outcome.summary_line())
                };
                self.record(sch, Some(point_ms), status, err.as_deref())
                    .await;
                (FireOutcome::FiredPipeline, false)
            }
            Err(e) => {
                let msg = e.to_string();
                self.record(sch, Some(point_ms), "error", Some(&msg)).await;
                (FireOutcome::UnknownTask(msg), false)
            }
        }
    }

    /// 返回 `(结果, 是否全被并发闸挡住)`：后者用于区分
    /// "跑了但失败" 与 "一个账号都没跑成" —— 两者在界面上的含义不同。
    async fn fire(&self, sch: &Schedule, point_ms: i64) -> (FireOutcome, bool) {
        // 账号清单为空 = 所有已启用账号
        let accounts = match self.target_accounts(sch).await {
            Ok(a) => a,
            Err(e) => return (FireOutcome::UnknownTask(e.to_string()), false),
        };
        if accounts.is_empty() {
            return (FireOutcome::SkippedBusy, true);
        }

        // ── 编排：**整份账号**交给编排运行器，不逐个账号跑 ✗ ──────────────
        if sch.task == PIPELINE_TASK {
            return self.fire_pipeline(sch, &accounts, point_ms).await;
        }

        let mut fired = None;
        let mut attempted = 0usize;
        let mut failures: Vec<String> = Vec::new();

        for acc in accounts {
            // 同一 task+account 正在跑 ⇒ 跳过不排队（在 runner 里判定）
            match self.runner.run(&sch.task, &acc, &sch.body).await {
                Ok(rec) => {
                    if rec.run_id.get() == 0 {
                        // 被并发闸挡住，不算"跑过"
                        continue;
                    }
                    attempted += 1;
                    if fired.is_none() {
                        fired = Some(rec.run_id.get());
                    }
                    if rec.outcome.is_failure() {
                        failures.push(format!("{}：{}", acc.id, rec.outcome.summary()));
                    }
                }
                Err(Error::Config(msg)) => return (FireOutcome::UnknownTask(msg), false),
                Err(e) => {
                    attempted += 1;
                    failures.push(format!("{}：{e}", acc.id));
                    tracing::warn!(schedule = %sch.id, error = %e, "定时触发失败");
                }
            }
        }

        // 记账：**部分失败记 error，不记 ok**。
        // 定时任务没人当场看着，把"一半没成"记成 ok 等于把它藏起来 ——
        // 而"ok 配一句旧错误"正是查不出原因的形态。
        let blocked_all = attempted == 0;
        let status = if blocked_all {
            "skipped"
        } else if failures.is_empty() {
            "ok"
        } else {
            "error"
        };
        let err_text = if failures.is_empty() {
            None
        } else {
            Some(failures.join("；"))
        };
        // 全被挡住时**不动**上次运行时间：这一趟什么都没跑，
        // 抹掉真实的上次运行时间会显示成"从未执行"
        let ran_at = if blocked_all { None } else { Some(point_ms) };
        self.record(sch, ran_at, status, err_text.as_deref()).await;

        (
            match fired {
                Some(id) => FireOutcome::Fired { run_id: id },
                None => FireOutcome::SkippedBusy,
            },
            blocked_all,
        )
    }

    /// 这条定时该跑**哪些账号**。
    ///
    /// ⚠ 归属是硬边界：`accounts: []` 的语义是「我名下的全部账号」，
    /// **不是**「库里全部账号」—— 后者会让 B 建一条不带账号的定时去跑
    /// A 的账号（真花 A 的 AI豆、动 A 的网盘文件）。
    async fn target_accounts(&self, sch: &Schedule) -> Result<Vec<mcloud_core::Account>> {
        let owned = match sch.owner.as_ref() {
            // 已归属：只取主人的账号（`accounts_for` 里含「单用户无主可见」规则）。
            Some(owner) => self.store.accounts_for(owner).await?,
            // 未归属：只有「老库且用户表为空」才该出现（有用户时加载会回填）。
            // 单用户 ⇒ 就是它的账号；没有用户 ⇒ 全库（此时不存在多用户）；
            // 多用户 + 无主 ⇒ **空表**（失败关闭：宁可这次不跑，也不越权）。
            None => match self.store.users().await?.as_slice() {
                [sole] => self.store.accounts_for(&sole.id).await?,
                [] => self.store.accounts().await?,
                _ => Vec::new(),
            },
        };
        if sch.accounts.is_empty() {
            return Ok(owned);
        }
        let wanted: Vec<AccountId> = sch.accounts.iter().map(AccountId::new).collect();
        Ok(owned
            .into_iter()
            .filter(|a| wanted.contains(&a.id))
            .collect())
    }

    /// 起循环。
    ///
    /// **首个 tick 立刻跑，不等一个间隔。** 定时器的第一次触发是在
    /// 一个间隔之后，那样每次重启都要白等 30 秒 —— 而重启恰恰是
    /// "错过今天那一趟"最常见的原因。
    pub async fn run_loop(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
        // 用 send_replace 而不是 send：前者不依赖有没有接收者
        self.state_tx.send_replace(SchedulerState::Running);
        tracing::info!(tick_secs = TICK.as_secs(), "调度器已启动");

        // 立刻跑一次
        self.tick_and_log().await;

        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await; // interval 的第一次是立即返回的，先吃掉

        loop {
            tokio::select! {
                _ = interval.tick() => self.tick_and_log().await,
                // ⚠ 这里必须接住 `Err`，不能图省事写 `_ = stop.changed()`：
                // 发送端一旦没了，`changed()` **立刻**返回 `Err(RecvError)` —— 那是个
                // 就绪值，被 `_` 吃掉之后这个分支每次都就绪，而 `*stop.borrow()` 还是
                // `false`（不会 break）⇒ select 退化成忙等，30 秒的 TICK 形同虚设，
                // 空转烧满一核。发送端留在结构体里是正解，这里是第二道保险。
                res = stop.changed() => {
                    if res.is_err() || *stop.borrow() {
                        break;
                    }
                }
            }
        }

        self.state_tx.send_replace(SchedulerState::Stopped);
        tracing::info!("调度器已停止");
    }

    async fn tick_and_log(self: &Arc<Self>) {
        // ⚠ 守护循环走 `Some(self)`：到点的那条丢进后台，这一拍**不占住循环**
        //（否则慢任务会把下一拍推后 ⇒ 同期其它定时被判"错过太久"弃掉，
        //  见 `tick_inner` 的说明 / 复审 sv-2）。
        match self.tick_inner(Some(self)).await {
            Ok(results) if results.is_empty() => {}
            Ok(results) => {
                for (id, outcome) in results {
                    // ⚠ `Spawned` 只表示"已经丢出去"，真正的结论由那个后台任务
                    // 自己打一行（见 `tick_inner`）⇒ 这里不重复。
                    if matches!(outcome, FireOutcome::Spawned) {
                        continue;
                    }
                    tracing::info!(schedule = %id, ?outcome, "定时触发");
                }
            }
            Err(e) => tracing::error!(error = %e, "调度 tick 失败"),
        }
    }

    /// 请求 `run_loop` 退出（优雅停止）。
    ///
    /// ⚠ 这个方法的**存在本身**是忙等那个 bug 的另一半：发送端原先在
    /// `new()` 里被当场丢掉，于是 receiver 侧的 `changed()` 永远立刻返回
    /// `Err`，循环空转（见 `new()` 上的实测记录）。留住发送端只是"别再空转"，
    /// 真正**能让它停下来**的是这里 —— 两者缺一，停止通道都是个摆设。
    ///
    /// 用 `send_replace` 而不是 `send`：不依赖"此刻有没有接收者"。
    pub fn stop(&self) {
        self.stop_tx.send_replace(true);
    }

    /// 停止信号的订阅端（`run_loop` 拿它等停止）。
    pub fn stop_handle(&self) -> watch::Receiver<bool> {
        self.stop_rx.clone()
    }
}

/// 计算下次触发时间，供前端展示。
///
/// 放在服务端算而不是各客户端各算一遍：5/6 段、周日的 0/7、
/// 日与周的 OR 语义这些细节，让每个客户端各实现一遍必然分叉。
pub fn next_fire_at(schedule: &Schedule, now_ms: i64) -> Option<i64> {
    let tz = parse_timezone(&schedule.timezone).ok()?;
    let expr = normalize_expr(&schedule.expr).ok()?;
    let cron = cron::Schedule::from_str(&expr).ok()?;
    let after: DateTime<chrono::FixedOffset> = Utc
        .timestamp_millis_opt(now_ms)
        .single()?
        .with_timezone(&tz);
    cron.after(&after)
        .next()
        .map(|t| t.with_timezone(&Utc).timestamp_millis())
}

#[cfg(test)]
mod tests;
