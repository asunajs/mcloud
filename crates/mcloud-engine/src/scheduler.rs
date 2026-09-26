//! 定时调度器。
//!
//! # 三条硬规则（都是踩过坑才立下的）
//!
//! ## 1. 先推水位，再打上游
//!
//! 水位（`last_run_ms`）必须在**触发上游之前**落盘。反过来的话，
//! 进程在跑任务时被杀 ⇒ 重启后同一个时间点会**再跑一次**，
//! 而签到、投口令这类动作在上游那侧**不是幂等的**。
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireOutcome {
    /// 触发成功，已交给运行器。
    Fired { run_id: u64 },
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
    clock: Arc<dyn Clock>,
    /// 广播自己的运行状态，供 `/api/status` 读运行时实况。
    state_tx: watch::Sender<SchedulerState>,
    /// 自己持有一份接收端，保证 `state_tx.send` 不会因为"无人订阅"而失败。
    _state_rx: watch::Receiver<SchedulerState>,
    /// 让 `run_loop` 能被告知停止。
    stop_rx: watch::Receiver<bool>,
}

impl Scheduler {
    pub fn new(store: Arc<dyn Store>, runner: Arc<Runner>, clock: Arc<dyn Clock>) -> Self {
        // ⚠ 必须留住接收端：`watch::Sender::send` 在**没有接收者**时返回 Err，
        // 而被忽略的 Err 会让状态永远停在 Stopped —— 界面上就是
        // "调度器明明在跑，却显示已停止"。
        let (state_tx, state_rx) = watch::channel(SchedulerState::Stopped);
        let (_stop_tx, stop_rx) = watch::channel(false);
        Self {
            store,
            runner,
            clock,
            state_tx,
            _state_rx: state_rx,
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
    pub async fn tick(&self) -> Result<Vec<(ScheduleId, FireOutcome)>> {
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
                    self.advance_watermark(&sch, now).await?;
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
                    self.advance_watermark(&sch, point_ms).await?;
                    let (outcome, _blocked) = self.fire(&sch, point_ms).await;
                    results.push((sch.id.clone(), outcome));
                }
            }
        }
        Ok(results)
    }

    /// 推水位。
    ///
    /// 用**只改水位**的窄写，而不是整行覆盖：整行覆盖会把用户在这期间
    /// 通过 WebUI 改过的名字/参数一起抹掉（调度器手里那行是几秒前读的）。
    async fn advance_watermark(&self, sch: &Schedule, point_ms: i64) -> Result<()> {
        self.store.mark_schedule_fired(&sch.id, point_ms).await
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

    async fn target_accounts(&self, sch: &Schedule) -> Result<Vec<mcloud_core::Account>> {
        let all = self.store.accounts().await?;
        if sch.accounts.is_empty() {
            return Ok(all);
        }
        let wanted: Vec<AccountId> = sch.accounts.iter().map(AccountId::new).collect();
        Ok(all.into_iter().filter(|a| wanted.contains(&a.id)).collect())
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
                _ = stop.changed() => {
                    if *stop.borrow() {
                        break;
                    }
                }
            }
        }

        self.state_tx.send_replace(SchedulerState::Stopped);
        tracing::info!("调度器已停止");
    }

    async fn tick_and_log(&self) {
        match self.tick().await {
            Ok(results) if results.is_empty() => {}
            Ok(results) => {
                for (id, outcome) in results {
                    tracing::info!(schedule = %id, ?outcome, "定时触发");
                }
            }
            Err(e) => tracing::error!(error = %e, "调度 tick 失败"),
        }
    }

    /// 请求停止（供优雅关闭用）。
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
