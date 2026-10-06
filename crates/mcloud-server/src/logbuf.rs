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

/// 环里"**任务日志**"那一档（某个 run 的日志）。
///
/// 判据见 [`category_of`]：**只有** [`mcloud_app::run_labels::trace_event`] 打的行
/// 算任务日志 ✓（它盖了账号那一格 ⇒ 能归属到人 ✓）。
pub const CATEGORY_TASK: &str = "task";

/// 环里"**运维日志**"那一档（实例自己的日志：认证失败、路径被拒、调度器、上游 http）。
///
/// ⚠ 它是**兜底**那一档：认不出归属的一律进这里 ⇒ 只给管理员看 ✓
/// （失败关闭 —— 见 `server_logs.rs` 的可见性说明 ✓）。
pub const CATEGORY_OPS: &str = "ops";

/// 一行的类别：**谁打的**决定它是哪一档（2026-10-06 用户口径：
/// 「这种应该分为运维日志」）。
///
/// ⚠ 用 target 而**不是**"有没有 account 字段"来判：上游 http 那层
/// （`mcloud_upstream::http_log`）也会打 `account=…`（`上游应答 account=… status=200` ✓），
/// 但它是**实例**的日志（凭证续期、限流），不是某次运行的日志 ✗
/// ⇒ 按"有没有账号"分会把运维日志错判成任务日志（= 漏给普通用户）✗。
fn category_of(target: &str) -> &'static str {
    if target == mcloud_app::run_labels::LOG_TARGET {
        CATEGORY_TASK
    } else {
        CATEGORY_OPS
    }
}

/// 一行服务端日志。
#[derive(Debug, Clone, Serialize)]
pub struct ServerLog {
    /// `error` / `warn` / `info` / `debug` / `trace` —— 与前端那套标记同名 ✓。
    pub level: String,
    /// 来源模块（`mcloud_engine::scheduler` 这种 ✓）—— 排障时"谁在说话"很关键 ✓。
    pub target: String,
    pub message: String,
    pub at_ms: i64,
    /// [`CATEGORY_TASK`] / [`CATEGORY_OPS`] —— 见 [`category_of`] ✓。
    ///
    /// ⚠ 这两档的**可见性不同**（任务日志按账号归属、运维日志只给管理员 ✓），
    /// 所以它是**权限判据的一部分**，不只是给人看的标签 ✓。
    pub category: String,
    /// 这一行属于哪个账号（`message` 里 `account="…"` 那一格的**结构化**副本 ✓）；
    /// 认不出就是 `None` ✓。
    ///
    /// ⚠ 与 `message` 里那格是**同一份事实的两种形状**：`message` 那份是给人看的
    /// （保持原样、一个字都不动 ✓），这一份是给**机器**用的（按账号过滤 ✓）。
    /// 前端原先得拿正则从文本里认（`web/src/pages/logs-page.tsx` 的 `ACCOUNT_RE`），
    /// 现在可以直接用这一格 ✓。
    pub account: Option<String>,
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

/// [`recent`] 的筛选条件。
///
/// ⚠ 做成结构体而**不是**继续往 `recent` 上加参数（2026-10-06 加 `category`/`accounts`
/// 时正好三变五）：五个位置参数里有两个是 `Option<&str>`，调用点写错顺序**编译器
/// 不会拦** ✗（两个都是 `Option<&str>` ⇒ 类型一模一样）。
#[derive(Debug, Clone)]
pub struct Filter<'a> {
    /// 最多回多少行（内部会夹到 `1..=RING_CAPACITY` ✓）。
    pub limit: usize,
    /// 只看**不低于**这个级别的；`None` ⇒ 不过滤 ✓。
    pub min_level: Option<&'a str>,
    /// 关键词（大小写不敏感，匹配正文与来源 ✓）。
    pub query: Option<&'a str>,
    /// 只看某一档（[`CATEGORY_TASK`] / [`CATEGORY_OPS`]）；`None` ⇒ 两档都要 ✓。
    pub category: Option<&'a str>,
    /// 只留这些账号的行；`None` ⇒ 不按账号过滤 ✓。
    ///
    /// ⚠ 只对**任务日志**有意义（运维日志没有归属 ✓），而**没有账号**的任务日志行
    /// 在给了 `Some(..)` 时会被滤掉 —— 这正是"归属不明 ⇒ 不给"的失败关闭 ✓。
    pub accounts: Option<&'a std::collections::HashSet<String>>,
}

impl<'a> Filter<'a> {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            min_level: None,
            query: None,
            category: None,
            accounts: None,
        }
    }
}

/// 取最近若干行。
///
/// 筛选条件见 [`Filter`] ✓。
///
/// ⚠ 返回的是**时间正序**（最早的在前 ✓）：界面直接从上往下渲染 ✓，
/// 与"日志该有的样子"一致 ✓。
pub fn recent(f: &Filter<'_>) -> Vec<ServerLog> {
    let limit = f.limit.clamp(1, RING_CAPACITY);
    let floor = f.min_level.map(rank_of).unwrap_or(0);
    let q = f.query.unwrap_or("").trim().to_lowercase();
    let r = ring().lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<ServerLog> = r
        .iter()
        .filter(|l| rank_of(&l.level) >= floor)
        .filter(|l| match f.category {
            Some(c) => l.category == c,
            None => true,
        })
        .filter(|l| match f.accounts {
            // ⚠ `None` 与"空集合"是**两件事**：前者不按账号过滤 ✓，
            // 后者是"这个人一个账号都没有" ⇒ 一条任务日志都不给 ✓。
            Some(acc) => l.account.as_deref().is_some_and(|a| acc.contains(a)),
            None => true,
        })
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
        let target = event.metadata().target();
        push(ServerLog {
            level: event.metadata().level().as_str().to_lowercase(),
            target: target.to_string(),
            category: category_of(target).to_string(),
            account: fields.account.clone(),
            message: fields.finish(),
            at_ms: now_ms(),
        });
    }
}

/// 事件字段的收集器。
///
/// `message` 放最前 ✓，其余字段按 `k=v` 追加 ✓ —— 这两样正是
/// `mcloud_upstream::http_log` 那种日志的可读形状（`上游应答 account=… status=200` ✓）。
///
/// ⚠ `account` **额外**留一份结构化的（见 [`Fields::account`]）：正文那份照旧，
/// 这一份只给机器用 ✓。
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
    /// `account="…"` 那一格的裸值（没有引号 ✓）—— 见 [`unquote`]。
    account: Option<String>,
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

/// 把 `{:?}` 打出来的值还原成裸值（只脱**一层**引号 ✓）。
///
/// `tracing` 里 `account = ?a` 与 `account = a`（`a: &str`）最终都走到
/// `record_debug`，而 `&str` 的 `Debug` 是**带引号**的 ⇒ 结构化那份要脱掉 ✓。
/// 不是"两端都是引号"就原样返回（数字、`Option` 之类不该被削 ✓）。
fn unquote(s: String) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s
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
            if field.name() == "account" {
                self.account = Some(unquote(format!("{value:?}")));
            }
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

/// ⚠⚠ 环形缓冲是**进程级**静态量：**所有**碰它的用例（本文件的 `tests` 与
/// `category_tests` 两个模块）必须共用**同一把**串行锁，并且每条先 `reset()` ✓。
///
/// # 为什么提到文件级（2026-10-06 实测踩到）
///
/// 原先 `tests` 与 `category_tests` **各写了一份** `serial()` —— 两份是**两个不同的
/// `static LOCK`** ✗ ⇒ 两个模块的用例可以同时跑、同时读写同一个环 ⇒
/// `filters_by_level_and_query` 当场红（它期望 2 条，看到的是别人塞进去的 0 条 ✓）。
/// 一把锁放两处 = 没锁 ✓。
///
/// 真代码里没有这个问题 ✓：只有一个进程、一个环，而写它的只有一个 `tracing`
/// subscriber ✓。这是**测试**的事，不是设计缺陷 ✓。
#[cfg(test)]
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 清空缓冲（每条用例自己先清一遍 ✓，断言一律用"包含"而不是"等于" ✓）。
#[cfg(test)]
fn reset() {
    ring().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: &str, target: &str, message: &str) -> ServerLog {
        ServerLog {
            level: level.into(),
            target: target.into(),
            message: message.into(),
            at_ms: 1_700_000_000_000,
            // ⚠ 走**真**的判据（而不是写死 "ops"）：这样 `category_of` 变了
            // 这些用例会跟着变，不会变成"自己跟自己一致"的摆设 ✓
            category: category_of(target).to_string(),
            account: None,
        }
    }

    /// 便捷构造筛选条件：`Filter::new(limit)` 那三格默认全是 `None` ✓。
    fn filt(limit: usize) -> Filter<'static> {
        Filter::new(limit)
    }

    #[test]
    fn keeps_at_most_capacity_and_drops_the_oldest() {
        let _guard = serial();
        reset();
        for i in 0..(RING_CAPACITY + 10) {
            push(entry("info", "t", &format!("第 {i} 行")));
        }
        let all = recent(&filt(RING_CAPACITY));
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
        let warns = recent(&Filter {
            min_level: Some("warn"),
            ..filt(50)
        });
        assert!(warns.iter().all(|l| l.level != "info"), "{warns:?}");
        assert_eq!(warns.len(), 2);

        // 关键词：大小写不敏感 ✓，正文与来源都算 ✓
        assert_eq!(
            recent(&Filter {
                query: Some("STATUS=403"),
                ..filt(50)
            })
            .len(),
            1
        );
        assert_eq!(
            recent(&Filter {
                query: Some("scheduler"),
                ..filt(50)
            })
            .len(),
            1
        );
        assert_eq!(
            recent(&Filter {
                query: Some("没有这回事"),
                ..filt(50)
            })
            .len(),
            0
        );

        // 顺序：时间正序 ✓
        let all = recent(&filt(50));
        assert_eq!(all.first().expect("非空").message, "调度器已启动");
        assert_eq!(all.last().expect("非空").message, "落盘失败");
    }

    #[test]
    fn limit_is_clamped() {
        let _guard = serial();
        reset();
        push(entry("info", "t", "只有一行"));
        assert_eq!(recent(&filt(0)).len(), 1, "0 也要给一行，别返回空");
        assert!(recent(&filt(usize::MAX)).len() <= RING_CAPACITY);
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
        let got = recent(&filt(10));
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

        let lines = recent(&filt(10));
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

#[cfg(test)]
mod category_tests {
    use super::*;

    fn filt(limit: usize) -> Filter<'static> {
        Filter::new(limit)
    }

    /// ⭐⭐ 2026-10-06 用户口径「这种应该分为运维日志」：**判据是"谁打的"**。
    ///
    /// 任务日志只有一条出口（`mcloud_app::run_labels::trace_event` ✓），
    /// 其余一律算运维日志（**失败关闭**：认不出就只给管理员看 ✓）。
    ///
    /// 红测：把 `category_of` 改成恒返回 `CATEGORY_TASK` ⇒ 本用例红
    /// （`mcloud_server::auth_gate` 那种实例日志会被当成任务日志 ⇒ 漏给普通用户 ✗）。
    #[test]
    fn only_the_run_log_target_is_a_task_log() {
        assert_eq!(
            category_of(mcloud_app::run_labels::LOG_TARGET),
            CATEGORY_TASK,
            "run_labels 打的就是任务日志"
        );
        for t in [
            "mcloud_server::auth_gate",
            "mcloud_server::routes::auth",
            "mcloud_engine::scheduler",
            // ⚠ 上游 http 那层**也**带 account 字段，但它是实例日志 ⇒ 必须是 ops
            "mcloud_upstream::http_log",
            "mcloud_app::notify",
            // 没见过的一律兜底成 ops ✓
            "某个将来的模块",
        ] {
            assert_eq!(category_of(t), CATEGORY_OPS, "{t} 该算运维日志");
        }
    }

    /// ⭐ 任务日志的行真的会被判成 `task` —— 用**真**的 `trace_event` 走一遍
    /// （不是手搓一个 `ServerLog` ⇒ 这样"判据"和"出口"是绑在一起验的 ✓）。
    ///
    /// ⚠ 这条同时是**耦合的钉子**：`LOG_TARGET` 跟着模块走 ⇒ 模块改名时
    /// 本用例仍然绿（它跟着常量 ✓），但若有人把任务日志改到**别处**去发，
    /// 本用例会红 ✓。
    #[test]
    fn trace_event_lines_land_in_the_task_category_with_a_structured_account() {
        let _guard = serial();
        reset();
        use tracing_subscriber::layer::SubscriberExt;

        let labels = mcloud_app::RunLabels::default();
        let run_id = mcloud_core::RunId::new(7);
        // ⚠ 必须**真装一个 subscriber**：`RingLayer` 只在 `main()` 里装过，
        // 测试进程里 `tracing::info!` 默认哪儿都不去 ⇒ 不装的话环一直是空的 ✗
        //（第一版就是这么红的：`got.last().expect("该记下一行")` panic ✓）。
        // 用 `with_default` 而不是 `init()`：它是**线程局部**的 ⇒ 不与并行用例抢
        // "全局唯一 subscriber" 那个位置 ✓（环本身仍靠 `serial()` 串起来 ✓）。
        let sub = tracing_subscriber::registry().with(RingLayer);
        tracing::subscriber::with_default(sub, || {
            mcloud_app::trace_event(
                &labels,
                &mcloud_core::RunEvent::Started {
                    run_id,
                    task: "app-task".into(),
                    account: "0f6050105ab111f4".into(),
                    at_ms: 0,
                },
            );
        });

        let got = recent(&filt(10));
        let line = got.last().expect("该记下一行");
        assert_eq!(line.category, CATEGORY_TASK, "trace_event 打的是任务日志");
        assert_eq!(
            line.account.as_deref(),
            Some("0f6050105ab111f4"),
            "账号那一格要**结构化**地留一份（没有引号）"
        );
        // ⚠ 正文里那一格照旧（一个字都不动 ✓）—— 前端与既有用例都还在读它
        assert!(
            line.message.contains("account=\"0f6050105ab111f4\""),
            "正文那份不能被改动：{}",
            line.message
        );
    }

    /// ⭐ 按账号过滤：**只对任务日志有意义**，而"没有账号的行"在给了账号集合时
    /// 会被滤掉（失败关闭 —— 归属不明就不给 ✓）。
    #[test]
    fn filtering_by_account_drops_lines_without_an_account() {
        let _guard = serial();
        reset();
        let mine: std::collections::HashSet<String> = ["acc-a".to_string()].into_iter().collect();

        // ① 带账号的任务日志（三条：acc-a 两条、acc-b 一条）
        for (acc, msg) in [
            ("acc-a", "我的第一条"),
            ("acc-a", "我的第二条"),
            ("acc-b", "别人的"),
        ] {
            push(ServerLog {
                level: "info".into(),
                target: mcloud_app::run_labels::LOG_TARGET.into(),
                message: format!("{msg} account=\"{acc}\""),
                at_ms: 1,
                category: CATEGORY_TASK.into(),
                account: Some(acc.into()),
            });
        }
        // ② 没有账号的任务日志（进程重启后 `labels` 空掉时就是这个形状）
        push(ServerLog {
            level: "info".into(),
            target: mcloud_app::run_labels::LOG_TARGET.into(),
            message: "任务结束：成功".into(),
            at_ms: 1,
            category: CATEGORY_TASK.into(),
            account: None,
        });
        // ③ 运维日志（实例自己的）
        push(ServerLog {
            level: "warn".into(),
            target: "mcloud_server::auth_gate".into(),
            message: "拒绝形状可疑的请求路径 path=//api/accounts".into(),
            at_ms: 1,
            category: CATEGORY_OPS.into(),
            account: None,
        });

        // 不按账号过滤 ⇒ 五条都在
        assert_eq!(recent(&filt(50)).len(), 5);

        // 只看我的任务日志 ⇒ 两条（acc-b 与"没有账号"那两条都不给 ✓）
        let mine_only = recent(&Filter {
            category: Some(CATEGORY_TASK),
            accounts: Some(&mine),
            ..filt(50)
        });
        assert_eq!(mine_only.len(), 2, "实际：{mine_only:#?}");
        assert!(mine_only
            .iter()
            .all(|l| l.account.as_deref() == Some("acc-a")));

        // 只看运维日志 ⇒ 那条 WARN
        let ops = recent(&Filter {
            category: Some(CATEGORY_OPS),
            ..filt(50)
        });
        assert_eq!(ops.len(), 1);
        assert!(ops[0].message.contains("拒绝形状可疑"));
    }

    /// ⭐ 空账号集合 ≠ 不过滤：`Some(空集)` 是"这个人一个账号都没有" ⇒ 一条都不给 ✓。
    #[test]
    fn an_empty_account_set_yields_nothing_instead_of_everything() {
        let _guard = serial();
        reset();
        push(ServerLog {
            level: "info".into(),
            target: mcloud_app::run_labels::LOG_TARGET.into(),
            message: "谁的任务 account=\"acc-a\"".into(),
            at_ms: 1,
            category: CATEGORY_TASK.into(),
            account: Some("acc-a".into()),
        });
        let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
        let got = recent(&Filter {
            accounts: Some(&empty),
            ..filt(50)
        });
        assert!(got.is_empty(), "空集合不该等于'不过滤'：{got:#?}");
    }
}
