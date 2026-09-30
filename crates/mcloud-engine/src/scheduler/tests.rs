//! 调度器测试。
//!
//! 重点在**水位线与错过策略** —— 那是算错代价最具体的地方：
//! 漏跑让人以为任务已经跑过，重跑会重复领奖。

use super::*;
use mcloud_core::ScheduleId;

/// 固定基准时刻：2026-01-01 00:00:00 UTC = 1767225600000
const T0: i64 = 1_767_225_600_000;

fn schedule(expr: &str, last_fire_ms: Option<i64>) -> Schedule {
    Schedule {
        id: ScheduleId::new("s1"),
        name: "测试".into(),
        task: "sign".into(),
        accounts: vec![],
        expr: expr.into(),
        timezone: "UTC".into(),
        body: serde_json::Value::Null,
        enabled: true,
        last_fire_ms,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    }
}

// ── 时区解析 ────────────────────────────────────────────────────────────

#[test]
fn parses_supported_timezones() {
    assert!(parse_timezone("UTC").is_ok());
    assert!(parse_timezone("Asia/Shanghai").is_ok());
    assert!(parse_timezone("Asia/Tokyo").is_ok());
    assert!(parse_timezone("+08:00").is_ok());
    assert!(parse_timezone("UTC+8").is_ok());
}

/// 不支持的时区必须**报错**，不能静默按 UTC 算。
///
/// 静默按 UTC 会让任务在错误的钟点跑，而且没有任何提示。
#[test]
fn unsupported_timezone_errors_instead_of_silently_using_utc() {
    let e = parse_timezone("America/New_York").unwrap_err();
    assert!(e.contains("不支持"), "应明确说不支持：{e}");
    assert!(e.contains("America/New_York"), "应带上原名便于排查：{e}");
}

#[test]
fn shanghai_is_utc_plus_8() {
    let tz = parse_timezone("Asia/Shanghai").unwrap();
    assert_eq!(tz.local_minus_utc(), 8 * 3600);
}

// ── 基本判定 ────────────────────────────────────────────────────────────

#[test]
fn disabled_schedule_never_fires() {
    let mut s = schedule("0 0 * * * *", Some(T0));
    s.enabled = false;
    assert_eq!(decide(&s, T0 + 86_400_000).unwrap(), Decision::Disabled);
}

#[test]
fn not_yet_when_next_point_is_in_the_future() {
    // 每小时的第 0 分 0 秒。水位在整点，now 是整点后 1 分钟
    let s = schedule("0 0 * * * *", Some(T0));
    let d = decide(&s, T0 + 60_000).unwrap();
    match d {
        Decision::NotYet { next_ms } => assert_eq!(next_ms, T0 + 3_600_000),
        other => panic!("应为 NotYet，实际 {other:?}"),
    }
}

#[test]
fn due_when_the_point_just_passed_within_grace() {
    let s = schedule("0 0 * * * *", Some(T0));
    // 整点刚过 5 秒（在 60 秒宽限内）
    let d = decide(&s, T0 + 3_600_000 + 5_000).unwrap();
    assert_eq!(
        d,
        Decision::Due {
            point_ms: T0 + 3_600_000
        }
    );
}

/// 迟到超过宽限 ⇒ 判定为错过，应整体弃掉。
#[test]
fn missed_when_late_beyond_grace() {
    let s = schedule("0 0 * * * *", Some(T0));
    // 整点过了 10 分钟（远超 60 秒宽限）
    let d = decide(&s, T0 + 3_600_000 + 600_000).unwrap();
    assert_eq!(
        d,
        Decision::Missed {
            point_ms: T0 + 3_600_000
        }
    );
}

/// 宽限边界：恰好等于 GRACE_MS 时**不算**错过。
#[test]
fn grace_boundary_is_inclusive() {
    let s = schedule("0 0 * * * *", Some(T0));
    let point = T0 + 3_600_000;
    let d = decide(&s, point + GRACE_MS).unwrap();
    assert_eq!(
        d,
        Decision::Due { point_ms: point },
        "恰好到宽限边界仍应触发"
    );

    let d2 = decide(&s, point + GRACE_MS + 1).unwrap();
    assert_eq!(
        d2,
        Decision::Missed { point_ms: point },
        "超过 1ms 即为错过"
    );
}

/// 首次运行（没有水位）不得回填历史。
///
/// 水位起点是 `now - GRACE_MS`，所以"很久以前"的点不会被当成待补的。
#[test]
fn first_run_does_not_backfill_history() {
    let s = schedule("0 0 * * * *", None);
    // now 是 T0（整点），水位起点 = T0 - 60s，下一个点 = T0
    let d = decide(&s, T0).unwrap();
    assert_eq!(
        d,
        Decision::Due { point_ms: T0 },
        "起点落在宽限窗口内 ⇒ 恰好落在窗口里的点仍应触发"
    );

    // now = T0+30s 时水位起点是 T0-30s，下一个点是 T0 —— 落在宽限窗口内，
    // 所以应当**触发**（这是"迟到一点不算错过"，不是回填历史）
    assert_eq!(
        decide(&s, T0 + 30_000).unwrap(),
        Decision::Due { point_ms: T0 },
        "窗口内的点该触发；回填历史指的是更早的点，不是这个"
    );

    // 真正的"回填历史"测试：now 远在 T0 之后，T0 那个点不该被补
    let later = T0 + 86_400_000 * 3 + 12_345;
    match decide(&s, later).unwrap() {
        // 只判"最近这一个点"，不逐点追补 —— 要么 Due 要么 Missed，
        // 绝不会得到一个"三天前的点"
        Decision::Due { point_ms } => assert!(
            later - point_ms <= GRACE_MS,
            "触发的必须是最近的点（{point_ms}），而不是三天前那个"
        ),
        Decision::Missed { .. } => {}
        other => panic!("不该是 {other:?}"),
    }
}

/// 永不触发的表达式（2 月 30 日）不是错误，是"这条定时是死的"。
#[test]
fn never_firing_expression_is_reported_as_such() {
    let s = schedule("0 0 0 30 2 *", Some(T0));
    assert_eq!(decide(&s, T0).unwrap(), Decision::NeverFires);
}

#[test]
fn bad_cron_returns_error_not_panic() {
    let s = schedule("这不是 cron", Some(T0));
    assert!(decide(&s, T0).is_err());
}

#[test]
fn bad_timezone_returns_error() {
    let mut s = schedule("0 0 * * * *", Some(T0));
    s.timezone = "Mars/Olympus".into();
    assert!(decide(&s, T0).is_err());
}

/// 5 段表达式（分 时 日 月 周）也要能用。
#[test]
fn five_field_expression_works() {
    let s = schedule("0 8 * * *", Some(T0));
    // T0 是 00:00，下一个 08:00 应在 8 小时后
    let d = decide(&s, T0 + 1000).unwrap();
    match d {
        Decision::NotYet { next_ms } => assert_eq!(next_ms, T0 + 8 * 3_600_000),
        other => panic!("应为 NotYet，实际 {other:?}"),
    }
}

/// 跨时区：上海的 08:00 对应 UTC 的 00:00。
#[test]
fn timezone_shifts_the_fire_instant() {
    let mut s = schedule("0 0 8 * * *", Some(T0 - 86_400_000));
    s.timezone = "Asia/Shanghai".into();
    // T0 = UTC 00:00 = 上海 08:00，所以这个点应恰好是 T0
    let d = decide(&s, T0).unwrap();
    assert_eq!(
        d,
        Decision::Due { point_ms: T0 },
        "上海的 08:00 就是 UTC 的 00:00"
    );
}

// ── 运行状态广播 ────────────────────────────────────────────────────────

/// ⭐ 状态广播必须**在没有外部订阅者时也能更新**。
///
/// `watch::Sender::send` 在无接收者时返回 `Err`；如果实现里忽略了它，
/// 状态会永远停在 `Stopped` —— 界面上就是"调度器明明在跑，却显示已停止"。
/// 所以 `Scheduler` 自己持有一份接收端，并用 `send_replace`。
#[tokio::test]
async fn state_broadcast_works_without_external_subscribers() {
    use crate::registry::Registry;
    use crate::runner::{HttpClientFactory, Runner};
    use mcloud_core::testkit::FixedClock;
    use mcloud_core::{Account, EventSink, HttpClient, RunEvent, Store};
    use std::sync::Arc;

    struct NullSink;
    impl EventSink for NullSink {
        fn emit(&self, _e: RunEvent) {}
    }

    // 用一个最小的假存储：这里只关心调度器的状态广播
    let store: Arc<dyn Store> = Arc::new(crate::pipeline::tests::MemStore::default());
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(T0));
    let http: HttpClientFactory =
        Arc::new(|_a: &Account| Arc::new(crate::pipeline::tests::NoHttp) as Arc<dyn HttpClient>);
    let runner = Arc::new(Runner::new(
        Arc::new(Registry::new()),
        Arc::clone(&store),
        Arc::clone(&clock),
        Arc::new(NullSink) as Arc<dyn EventSink>,
        http,
    ));

    let sched = Arc::new(Scheduler::new(store, runner, clock));
    assert_eq!(sched.state(), SchedulerState::Stopped, "初始应为已停止");

    // 起循环，不给任何外部订阅者
    let stop = sched.stop_handle();
    let s2 = Arc::clone(&sched);
    let handle = tokio::spawn(async move { s2.run_loop(stop).await });

    // 等它把自己标成 Running
    for _ in 0..50 {
        if sched.state() == SchedulerState::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        sched.state(),
        SchedulerState::Running,
        "没有外部订阅者时状态也必须能更新 —— 否则界面永远显示'已停止'"
    );

    handle.abort();
}

// ── 优雅停止 ────────────────────────────────────────────────────────────

/// 一个只关心"跑没跑、停没停"的调度器：存储、网络、事件出口都是空壳。
///
/// 抽出来是因为下面两条测试要重复"起循环 → 等 Running → 停"这套动作，
/// 而它们关心的是**停止通道通不通**，不是调度逻辑本身。
fn bare_scheduler() -> Arc<Scheduler> {
    use crate::registry::Registry;
    use crate::runner::{HttpClientFactory, Runner};
    use mcloud_core::testkit::FixedClock;
    use mcloud_core::{Account, EventSink, HttpClient, RunEvent, Store};
    use std::sync::Arc;

    struct NullSink;
    impl EventSink for NullSink {
        fn emit(&self, _e: RunEvent) {}
    }

    let store: Arc<dyn Store> = Arc::new(crate::pipeline::tests::MemStore::default());
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(T0));
    let http: HttpClientFactory =
        Arc::new(|_a: &Account| Arc::new(crate::pipeline::tests::NoHttp) as Arc<dyn HttpClient>);
    let runner = Arc::new(Runner::new(
        Arc::new(Registry::new()),
        Arc::clone(&store),
        Arc::clone(&clock),
        Arc::new(NullSink) as Arc<dyn EventSink>,
        http,
    ));
    Arc::new(Scheduler::new(store, runner, clock))
}

/// ⭐ `Scheduler::stop()` 必须真的让 `run_loop` **退出**，而且不能等到下一个 tick。
///
/// `TICK` 是 30 秒，而这里只给 2 秒 —— 能退出就只可能走的是停止分支。
///
/// 这条盯的是 2026-09-30 那个缺陷：发送端在 `new()` 里被当场丢掉，
/// 于是 `stop.changed()` 每次都立刻返回 `Err`（就绪值），`*stop.borrow()`
/// 又始终是 `false` ⇒ 不 break，只是**空转**。表现不是"报错"，而是
/// 空载一核跑满（实测单线程累计 187 分钟用户态 CPU）。
#[tokio::test]
async fn stop_wakes_run_loop_long_before_the_next_tick() {
    let sched = bare_scheduler();
    let stop = sched.stop_handle();
    let s2 = Arc::clone(&sched);
    let handle = tokio::spawn(async move { s2.run_loop(stop).await });

    for _ in 0..100 {
        if sched.state() == SchedulerState::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(sched.state(), SchedulerState::Running, "循环应当已经起来");

    sched.stop();
    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("stop() 之后 run_loop 应在 2 秒内退出（TICK 是 30 秒 ⇒ 只可能是停止分支生效）")
        .expect("run_loop 不该 panic");
    assert_eq!(
        sched.state(),
        SchedulerState::Stopped,
        "退出后状态应回到 Stopped"
    );
}

/// ⭐ 停止信号在**句柄**上就能看见 —— 这等价于"发送端是活的"。
///
/// 旧写法（`let (_stop_tx, stop_rx) = watch::channel(false)`）在这里会拿到
/// `Err(RecvError)`：`changed()` 在发送端消失后立刻返回 `Err`，而那正是上面
/// 那个忙等的根。所以这条断言的是**通道不能断**，不只是"值变了"。
#[tokio::test]
async fn stop_handle_observes_the_signal() {
    let sched = bare_scheduler();
    let mut rx = sched.stop_handle();
    assert!(!*rx.borrow(), "初始应是未停止");

    sched.stop();
    tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
        .await
        .expect("stop() 之后应立即收到信号")
        .expect("停止通道不该断开（发送端必须活着，否则 run_loop 会空转）");
    assert!(*rx.borrow(), "收到的值应是 true");
}

// ── 下次触发时间（供前端展示）──────────────────────────────────────────

#[test]
fn next_fire_at_is_computed_for_display() {
    let s = schedule("0 0 * * * *", Some(T0));
    let n = next_fire_at(&s, T0).expect("应能算出下次时间");
    assert_eq!(n, T0 + 3_600_000);
}

#[test]
fn next_fire_at_is_none_for_broken_config() {
    let mut s = schedule("坏表达式", Some(T0));
    assert!(next_fire_at(&s, T0).is_none());

    s.expr = "0 0 * * * *".into();
    s.timezone = "Mars/Olympus".into();
    assert!(next_fire_at(&s, T0).is_none(), "时区坏了也不该 panic");
}

/// 未启用的定时，前端不该显示"下次执行时间"——那是骗人。
#[test]
fn disabled_schedule_still_has_a_next_time_but_caller_must_hide_it() {
    // 纯函数只负责算；"未启用就别显示"是展示层的判断。
    // 这里锁住"算得出来"，避免调用方误以为算不出而写死 null。
    let mut s = schedule("0 0 * * * *", Some(T0));
    s.enabled = false;
    assert!(next_fire_at(&s, T0).is_some());
}
