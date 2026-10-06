//! 「这个 run 是哪个账号」—— 好让**每一行**日志都认得出主人。
//!
//! # 为什么要有这一层
//!
//! 用户口径（2026-10-05）：「页面上看不到是那个账号的日志啊」。
//!
//! 根因不在页面，在**事件流本身**：账号只挂在 [`RunEvent::Started`] 上，
//! 而 `Log` / `Progress` / `Finished` 三个变体**都不带**它（`mcloud-core::event` ✓）。
//! 于是一趟运行里除第一行之外，服务端日志页上全是"没主的行" ✗ ——
//! 而同一件事往往是**几个账号轮流跑**（截图里 17:13:11 / 17:13:25 / 17:13:40
//! 三次 `任务开始` 就是三个号 ✓），那几条 WARN 到底是谁的，从页面上根本认不出来 ✗。
//!
//! # 为什么不去改 `RunEvent` 的形状
//!
//! 在 `Log`/`Progress`/`Finished` 上各加一格 `account` 是"更结构化"的做法 ✓，
//! 但它要动 mcloud-core 的公开枚举、每个构造点（`mcloud-engine` 三处）以及脱敏那条链
//! （`RunEvent::scrubbed`）✗ —— 而这里需要的信息，
//! sink 在 `Started` 那一刻**就已经拿到了** ✓：记一份映射就够 ✓。
//!
//! ⚠ 上面那行原先写成"……公开枚举 + 每个构造点…… + 脱敏那条链"，其中 `+` 落在**行首** ⇒
//! markdown 把它当成**列表项**，于是下一行成了"没缩进的续行" ⇒
//! `clippy::doc_lazy_continuation` 警告 ✗ —— 而 CI 的 gates 是
//! `clippy --workspace --all-targets -- -D warnings` ⇒ **流水线会红、一个产物都发不出去** ✗
//! （2026-10-06 发 rc.3 之前按 CI 原样验出来的 ✓）。改掉行首那个 `+` 才是对的：
//! 它本来就只是句子里的连接词，不是列表 ✓。
//!
//! ⚠ 哪天要让**前端 SSE** 也按结构化字段认账号（而不是从文本里认 ✓），
//! 那时再改事件形状才划算 —— 届时这一层可以原地退化成"只给进程日志用" ✓。
//!
//! # 为什么放在 sink 这一层
//!
//! 两个 sink（`EventHub` 与 `BroadcastSink`）原本各抄了一份**逐字相同**的
//! `match` ✗。收成 [`trace_event`] 一份是顺手，真正的好处是：
//! "只有一部分 sink 盖了账号"这种漏，从此不可能发生 ✓。

use std::collections::HashMap;
use std::sync::Mutex;

use mcloud_core::{LogLevel, RunEvent, RunId};

/// `run_id → 账号`。
///
/// ⚠ 与 `account_names`（前端那份）分工不同：这里是**进程内**的运行归属，
/// 不查库、不翻译成人名 —— 人名的映射在页面上做（`web/src/account-names.ts` ✓）。
#[derive(Default)]
pub struct RunLabels {
    inner: Mutex<HashMap<RunId, String>>,
}

impl RunLabels {
    /// 记下"这个 run 是哪个账号的"。由 [`trace_event`] 在 `Started` 时调用 ✓。
    pub fn started(&self, run_id: RunId, account: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(run_id, account.to_string());
        }
    }

    /// 查这个 run 的账号。查不到就 `None`（**不编造**：宁可那一行没有账号，
    /// 也不要写个错的主人 ✗）。
    pub fn get(&self, run_id: RunId) -> Option<String> {
        self.inner.lock().ok()?.get(&run_id).cloned()
    }

    /// 忘掉这个 run。运行结束、或 hub 回收终态 run 时调 ✓。
    pub fn forget(&self, run_id: RunId) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(&run_id);
        }
    }

    /// 当前记着几条 —— 只给测试与诊断用 ✓（看它有没有随运行结束而回落）。
    pub fn len(&self) -> usize {
        self.inner.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// 空不空（`len` 的伴生方法，clippy 要求成对 ✓）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 本模块打出的 tracing 事件的 **target**（= `module_path!()`）。
///
/// # 为什么要有这个常量（2026-10-06 用户口径：「这种应该分为运维日志」）
///
/// 服务端那个「日志」窗口（`mcloud-server` 的 `logbuf`）现在把环里的行分成两档：
/// **任务日志**（某个 run 的）与**运维日志**（实例自己的 —— 认证失败、路径被拒、
/// 调度器、上游 http）。判据就是"**谁打的**"：只有本模块的 [`trace_event`]
/// 打的是任务日志 ✓（它盖了账号那一格，能归属到人 ✓）。
///
/// ⚠ 写成常量而**不是**在读取侧写死一个字符串：`module_path!()` 跟着模块走，
/// 模块改名/挪位置时它自己就变了 ⇒ 读取侧引这个常量就**不会漂** ✓。
/// ⚠ 万一真漂了，失效方向是**失败关闭**的：任务日志被判成运维日志
/// （= 只给管理员看）✓，不会反过来把实例日志漏给普通用户 ✓。
pub const LOG_TARGET: &str = module_path!();

/// 把一条事件打进 tracing，**每一行都盖上这个 run 的账号**。
///
/// 字段落在 `message` 之后（`RingLayer` 的 [`Fields`] 就是这么拼的 ✓），
/// 于是进程日志 / 服务端日志页上看到的形状是：
///
/// ```text
/// 分享文件领好礼1/7: 1/7 → 1/7（上游次数没动 ⇒ 不算记上） account="0f6050105ab111f4"
/// ```
///
/// —— 与「任务开始」那行（`task=… account=…` ✓）同一形状 ✓。
///
/// ⚠⚠ 本函数是**唯一**的"任务日志"出口：`logbuf` 按 [`LOG_TARGET`] 认它
/// ⇒ 在这里之外新开一条 `tracing::info!` 打任务日志，那行会被判成**运维日志**
/// （只给管理员看）✓ —— 是失败关闭，但也会让普通用户看不到它，别忘了这件事 ✓。
///
/// [`Fields`]: https://docs.rs/mcloud-server
pub fn trace_event(labels: &RunLabels, event: &RunEvent) {
    match event {
        RunEvent::Started {
            run_id,
            task,
            account,
            ..
        } => {
            labels.started(*run_id, account);
            tracing::info!(task, account, "任务开始");
        }
        // ⚠ 下面三支都先查账号再打：查不到就**不带**那一格（不是空串、不是 "?"）✓
        RunEvent::Log {
            run_id,
            level,
            message,
            ..
        } => {
            let account = labels.get(*run_id);
            match (level, account) {
                (LogLevel::Debug, Some(a)) => tracing::debug!(account = ?a, "{message}"),
                (LogLevel::Info, Some(a)) => tracing::info!(account = ?a, "{message}"),
                (LogLevel::Warn, Some(a)) => tracing::warn!(account = ?a, "{message}"),
                (LogLevel::Error, Some(a)) => tracing::error!(account = ?a, "{message}"),
                (LogLevel::Debug, None) => tracing::debug!("{message}"),
                (LogLevel::Info, None) => tracing::info!("{message}"),
                (LogLevel::Warn, None) => tracing::warn!("{message}"),
                (LogLevel::Error, None) => tracing::error!("{message}"),
            }
        }
        RunEvent::Progress {
            run_id,
            step,
            total,
            label,
        } => {
            let account = labels.get(*run_id);
            match account {
                Some(a) => tracing::debug!(account = ?a, "进度 {step}/{total} {label}"),
                None => tracing::debug!("进度 {step}/{total} {label}"),
            }
        }
        RunEvent::Finished {
            run_id,
            outcome,
            duration_ms,
        } => {
            // ⚠ 顺序要紧：**先打这一行、再忘掉**。反了的话"任务结束"那行就没有账号了 ✗
            // （而它正是"这趟跑完是谁"的最后一句 ✓）。
            let account = labels.get(*run_id);
            match account {
                Some(a) => {
                    tracing::info!(account = ?a, duration_ms, "任务结束：{}", outcome.summary())
                }
                None => tracing::info!(duration_ms, "任务结束：{}", outcome.summary()),
            }
            labels.forget(*run_id);
        }
        // 结构化明细**不逐条打日志**：它可能很大（整份兑换结论），
        // 而日志那一格是给人扫的。它真正的去处是 `RunRecord.details`
        // —— 由 runner 累积后落历史，前端从那里读。
        RunEvent::Details { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{LogLevel, RunEvent, RunId, TaskOutcome};

    fn log(run_id: u64, level: LogLevel, message: &str) -> RunEvent {
        RunEvent::Log {
            run_id: RunId(run_id),
            level,
            kind: Default::default(),
            message: message.into(),
            at_ms: 0,
        }
    }

    fn started(run_id: u64, account: &str) -> RunEvent {
        RunEvent::Started {
            run_id: RunId(run_id),
            task: "app-task".into(),
            account: account.into(),
            at_ms: 0,
        }
    }

    #[test]
    fn remembers_account_per_run_and_forgets_on_finish() {
        let labels = RunLabels::default();
        trace_event(&labels, &started(1, "aaa"));
        trace_event(&labels, &started(2, "bbb"));
        assert_eq!(labels.get(RunId(1)).as_deref(), Some("aaa"));
        assert_eq!(labels.get(RunId(2)).as_deref(), Some("bbb"));
        assert_eq!(labels.len(), 2);

        // 结束之后**忘掉**：否则一趟跑完的账号会一直攒着（长跑进程里的慢泄漏 ✗）
        trace_event(
            &labels,
            &RunEvent::Finished {
                run_id: RunId(1),
                outcome: TaskOutcome::success("跑完了"),
                duration_ms: 5,
            },
        );
        assert_eq!(labels.get(RunId(1)), None);
        assert_eq!(labels.len(), 1, "另一个 run 的账号不该被连带清掉");
    }

    #[test]
    fn two_runs_interleaved_keep_their_own_account() {
        // 这正是用户截图里的场景：几个账号轮流跑，日志交替出现 ✓
        let labels = RunLabels::default();
        trace_event(&labels, &started(1, "aaa"));
        trace_event(&labels, &started(2, "bbb"));
        assert_eq!(labels.get(RunId(1)).as_deref(), Some("aaa"));
        assert_eq!(labels.get(RunId(2)).as_deref(), Some("bbb"));
        trace_event(&labels, &log(2, LogLevel::Warn, "分享文件领好礼1/7"));
        trace_event(&labels, &log(1, LogLevel::Warn, "分享文件领好礼0/7"));
        // 两条各自的账号都没串（查得到就是对的 ✓；串了会在这里现形）
        assert_eq!(labels.get(RunId(1)).as_deref(), Some("aaa"));
        assert_eq!(labels.get(RunId(2)).as_deref(), Some("bbb"));
    }

    #[test]
    fn unknown_run_has_no_label_instead_of_a_wrong_one() {
        let labels = RunLabels::default();
        assert_eq!(
            labels.get(RunId(9)),
            None,
            "没见过的 run 不能编一个主人出来"
        );
        // 不带账号也要照常打得出来（退化成旧形状 ✓），不能因此丢日志 ✗
        trace_event(&labels, &log(9, LogLevel::Info, "孤立的一行"));
        assert!(labels.is_empty());
    }
}
