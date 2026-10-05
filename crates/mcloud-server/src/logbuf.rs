//! 服务端日志的**环形缓冲** —— 给 WebUI 那个「日志」窗口用。
//!
//! # 为什么要有它（用户 2026-10-04 口径）
//!
//! 「我其实想要一个能看**所有**日志的窗口，**不仅仅是每次任务**」✓。
//!
//! 今天服务端的输出只去两个地方：launchd 重定向的那个**日志文件**
//! （要看就得开终端 `tail` ✗）与 `tracing` 的 stdout。而运行记录里那份
//! `RunRecord.logs` 只管**任务自己发的**日志 ✗ —— 调度器、凭据续期、
//! **上游 http 那一层**，一行都进不去 ✓。
//!
//! 所以在这里挂一个 `tracing` 的 [`Layer`] ✓，把**所有**事件抄进一个定长
//! 环形缓冲 ✓，再由 `/api/logs` 交给界面 ✓。
//!
//! # 为什么是"环形 + 定长"
//!
//! 它活在**内存**里 ✓ ⇒ 必须封顶，否则一个刷屏的循环能把进程撑爆 ✗。
//! 满了丢**最旧**的 ✓（排障要看的是"刚刚发生了什么" ✓）。
//!
//! # ⚠ 脱敏的规矩没变
//!
//! 这里抄的是**事件本身**（与 stdout 上那份同源 ✓）⇒ 脱敏仍由各个 emit 点
//! 负责 ✓（`RunEvent::scrubbed()` 那条规矩 ✓）。反过来说：**别直接往
//! `tracing` 打原始凭证** ✗（`tracing::info!(token = %raw)` 就是漏点 ✓，
//! 因为它绕过了事件那一层的闸 ✓）。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// 最多留多少行。
///
/// 2000 行 ≈ 几百 KB ✓（一条几十到几百字节 ✓）。别再往上加：这是内存 ✓，
/// 而**完整历史**在日志文件里 ✓ —— 这个窗口解决的是"不开终端就能看" ✓，
/// 不是"当归档用" ✓。
pub const RING_CAPACITY: usize = 2000;

/// 一行服务端日志。
#[derive(Debug, Clone, Serialize)]
pub struct ServerLog {
    /// `error` / `warn` / `info` / `debug` / `trace` —— 与前端那套标记同名 ✓。
    pub level: String,
    /// 来源模块（`mcloud_engine::scheduler` 这种 ✓）—— 排障时"谁在说话"很关键 ✓。
    pub target: String,
    pub message: String,
    pub at_ms: i64,
}

fn ring() -> &'static Mutex<VecDeque<ServerLog>> {
    static RING: OnceLock<Mutex<VecDeque<ServerLog>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_CAPACITY)))
}

fn push(entry: ServerLog) {
    let mut r = ring().lock().unwrap_or_else(|e| e.into_inner());
    if r.len() == RING_CAPACITY {
        // 满了丢**最旧**的 ✓（见模块头）
        r.pop_front();
    }
    r.push_back(entry);
}

/// 取最近若干行。
///
/// * `limit` 会夹到 `1..=RING_CAPACITY` ✓（外部传个 0 或天文数字都不该出问题 ✓）
/// * `min_level`：`None`/空 ⇒ 不过滤 ✓；否则只留**不低于**它的 ✓
/// * `query`：大小写不敏感的子串 ✓（匹配正文与来源 ✓，对应界面上那个搜索框 ✓）
///
/// ⚠ 返回的是**时间正序**（最早的在前 ✓）：界面直接从上往下渲染 ✓，
/// 与"日志该有的样子"一致 ✓。
pub fn recent(limit: usize, min_level: Option<&str>, query: Option<&str>) -> Vec<ServerLog> {
    let limit = limit.clamp(1, RING_CAPACITY);
    let floor = min_level.map(rank_of).unwrap_or(0);
    let q = query.unwrap_or("").trim().to_lowercase();
    let r = ring().lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<ServerLog> = r
        .iter()
        .filter(|l| rank_of(&l.level) >= floor)
        .filter(|l| {
            q.is_empty()
                || l.message.to_lowercase().contains(&q)
                || l.target.to_lowercase().contains(&q)
        })
        .rev()
        .take(limit)
        .cloned()
        .collect();
    out.reverse();
    out
}

/// 级别的高低（越大越严重 ✓）—— 用于 `min_level` 过滤 ✓。
fn rank_of(level: &str) -> u8 {
    match level {
        "error" => 4,
        "warn" => 3,
        "info" => 2,
        "debug" => 1,
        _ => 0, // trace 与任何没见过的写法
    }
}

/// 把 `tracing` 的事件抄进环形缓冲的 Layer ✓。
///
/// 装法与理由见 `main.rs` 里 `serve` 之前那一段 ✓。
pub struct RingLayer;

impl<S> Layer<S> for RingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        push(ServerLog {
            level: event.metadata().level().as_str().to_lowercase(),
            target: event.metadata().target().to_string(),
            message: fields.finish(),
            at_ms: now_ms(),
        });
    }
}

/// 事件字段的收集器。
///
/// `message` 放最前 ✓，其余字段按 `k=v` 追加 ✓ —— 这两样正是
/// `mcloud_upstream::http_log` 那种日志的可读形状（`上游应答 account=… status=200` ✓）。
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Fields {
    fn finish(self) -> String {
        if self.rest.is_empty() {
            self.message
        } else {
            format!("{} {}", self.message, self.rest)
        }
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // ⚠ 这里**不能**用 `{:?}` 的思路套所有字段：对 `message`（它是
            // `format_args!`）`{:?}` 打出来就是原文 ✓，但别的字段值若是 `&str`，
            // `{:?}` 会**带引号** ✓ —— 所以两条分支分开写 ✓。
            self.message = format!("{value:?}");
        } else {
            if !self.rest.is_empty() {
                self.rest.push(' ');
            }
            self.rest.push_str(&format!("{}={:?}", field.name(), value));
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 清空缓冲（测试之间互不影响 ✓）——
    /// ⚠ 环形缓冲是**进程级**静态量 ✓，测试并行跑时它们会互相看见 ✗ ⇒
    /// 每条用例自己先清一遍 ✓，并且断言都用"包含"而不是"等于" ✓。
    fn reset() {
        ring().lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// ⚠ 这些用例共用**进程级**的环形缓冲 ⇒ 必须**串行**跑 ✓。
    ///
    /// 并行时一条用例的 `reset()` 会把另一条正在断言的数据清掉 ✗
    /// —— 实测踩到：`filters_by_level_and_query` 期望 2 条却看到 0 条 ✓，
    /// `layer_actually_records_events` 期望 `warn` 却看到 `info` ✓。
    ///
    /// 真代码里没有这个问题 ✓：只有一个进程、一个环 ✓，而写它的只有一个
    /// `tracing` subscriber ✓。这是**测试**的事，不是设计缺陷 ✓。
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn entry(level: &str, target: &str, message: &str) -> ServerLog {
        ServerLog {
            level: level.into(),
            target: target.into(),
            message: message.into(),
            at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn keeps_at_most_capacity_and_drops_the_oldest() {
        let _guard = serial();
        reset();
        for i in 0..(RING_CAPACITY + 10) {
            push(entry("info", "t", &format!("第 {i} 行")));
        }
        let all = recent(RING_CAPACITY, None, None);
        assert_eq!(all.len(), RING_CAPACITY, "必须封顶");
        // 丢的是**最旧**的 ✓
        assert!(
            !all.iter().any(|l| l.message == "第 0 行"),
            "最早的该被丢掉"
        );
        assert_eq!(
            all.last().expect("非空").message,
            format!("第 {} 行", RING_CAPACITY + 9),
            "最新的必须在 ✓"
        );
    }

    #[test]
    fn filters_by_level_and_query() {
        let _guard = serial();
        reset();
        push(entry("info", "mcloud_engine::scheduler", "调度器已启动"));
        push(entry(
            "warn",
            "mcloud_upstream::http_log",
            "上游应答 status=403",
        ));
        push(entry("error", "mcloud_app", "落盘失败"));

        // 级别过滤：只看 warn 及以上 ✓
        let warns = recent(50, Some("warn"), None);
        assert!(warns.iter().all(|l| l.level != "info"), "{warns:?}");
        assert_eq!(warns.len(), 2);

        // 关键词：大小写不敏感 ✓，正文与来源都算 ✓
        assert_eq!(recent(50, None, Some("STATUS=403")).len(), 1);
        assert_eq!(recent(50, None, Some("scheduler")).len(), 1);
        assert_eq!(recent(50, None, Some("没有这回事")).len(), 0);

        // 顺序：时间正序 ✓
        let all = recent(50, None, None);
        assert_eq!(all.first().expect("非空").message, "调度器已启动");
        assert_eq!(all.last().expect("非空").message, "落盘失败");
    }

    #[test]
    fn limit_is_clamped() {
        let _guard = serial();
        reset();
        push(entry("info", "t", "只有一行"));
        assert_eq!(recent(0, None, None).len(), 1, "0 也要给一行，别返回空");
        assert!(recent(usize::MAX, None, None).len() <= RING_CAPACITY);
    }

    /// ⚠ 真正装一个 subscriber 跑一遍：证明 `RingLayer` **确实**在收事件 ✓
    /// —— 少了这条，整个环可能一直空着而不报错 ✗。
    #[test]
    fn layer_actually_records_events() {
        use tracing_subscriber::layer::SubscriberExt;
        let _guard = serial();
        reset();
        let sub = tracing_subscriber::registry().with(RingLayer);
        tracing::subscriber::with_default(sub, || {
            tracing::warn!(account = "13800000000", "上游应答 status=403");
        });
        let got = recent(10, None, None);
        let line = got.last().expect("该收到一条");
        assert_eq!(line.level, "warn");
        assert!(line.message.contains("上游应答 status=403"), "{line:?}");
        // 结构化字段也要跟着 ✓（`k=v` ✓）—— 排障靠的就是它们 ✓
        assert!(line.message.contains("account"), "{line:?}");
    }

    /// ⚠⚠ 用户口径（2026-10-05）：「页面上看不到是那个账号的日志啊」。
    ///
    /// 这一条钉的就是它，而且是**端到端**那一段：sink 往 tracing 打的字段
    /// （`mcloud_app::trace_event` ✓）→ 环里的文本（`RingLayer` ✓）→
    /// 页面读到的 `message`（`/api/logs` ✓）。
    ///
    /// 为什么放在这个 crate：这里已经有一套"真装 subscriber + 读环"的现成装置 ✓，
    /// 而这条要证的正是**两层的接缝** —— 只测 `RunLabels` 的增删（`mcloud-app` 里那几条 ✓）
    /// 抓不到"字段有没有真的落到环的文本里"✗。
    #[test]
    fn every_line_of_a_run_carries_its_account() {
        use mcloud_app::{trace_event, RunLabels};
        use mcloud_core::{LogLevel, RunEvent, RunId, TaskOutcome};
        use tracing_subscriber::layer::SubscriberExt;
        let _guard = serial();
        reset();
        let labels = RunLabels::default();
        let sub = tracing_subscriber::registry().with(RingLayer);
        let log = |run: u64, msg: &str| RunEvent::Log {
            run_id: RunId(run),
            level: LogLevel::Warn,
            kind: Default::default(),
            message: msg.into(),
            at_ms: 0,
        };
        tracing::subscriber::with_default(sub, || {
            trace_event(
                &labels,
                &RunEvent::Started {
                    run_id: RunId(1),
                    task: "app-task".into(),
                    account: "0f6050105ab111f4".into(),
                    at_ms: 0,
                },
            );
            trace_event(&labels, &log(1, "分享文件领好礼1/7：1/7 → 1/7"));
            // 没 Started 过的 run：宁可**不带**账号，也不能安一个错的主人 ✗
            trace_event(&labels, &log(2, "来路不明的一行"));
            trace_event(
                &labels,
                &RunEvent::Finished {
                    run_id: RunId(1),
                    outcome: TaskOutcome::success("跑完了"),
                    duration_ms: 5,
                },
            );
        });

        let lines = recent(10, None, None);
        let find = |needle: &str| {
            lines
                .iter()
                .find(|l| l.message.contains(needle))
                .unwrap_or_else(|| panic!("环里没有含 {needle:?} 的那一行：{lines:?}"))
        };
        // ① 关键那条：运行**中间**的日志行（截图里那类 WARN）也要带账号 ✓
        let mid = find("分享文件领好礼1/7");
        assert!(
            mid.message.contains("account=\"0f6050105ab111f4\""),
            "运行中间的日志行没带账号：{:?}",
            mid.message
        );
        // ② 结束那条也要（实现里是"先打这一行、再忘掉"，反了就丢 ✗）
        let end = find("任务结束");
        assert!(
            end.message.contains("account=\"0f6050105ab111f4\""),
            "「任务结束」没带账号：{:?}",
            end.message
        );
        // ③ 没见过的 run 不许被安上别人的账号 ✓
        let orphan = find("来路不明的一行");
        assert!(
            !orphan.message.contains("account"),
            "来路不明的 run 被安了账号：{:?}",
            orphan.message
        );
        // ④ 运行结束后映射要收干净（长跑进程里只增不减就是慢泄漏 ✗）
        assert_eq!(labels.len(), 0, "结束后 RunLabels 没清干净");
    }
}
