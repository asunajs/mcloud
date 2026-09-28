//! 兑换路由的**纯函数**测试。
//!
//! # 为什么不在这里起 Router 做端到端
//!
//! 那需要一份带账号的 `ServerState`，而真正值得钉的东西**不在 HTTP 层**：
//! 是"任务自报的那份形状"与"路由读回来的字段"之间的**一一对应**。
//! 两者对不上时不报错 —— 只表现为页面上少显示一栏或显示成 `0`，
//! 所以那正是要在编译期之外钉住的。
//!
//! 上游交互（清单、下单）由 `mcloud-upstream` 与 `mcloud-tasks` 各自的
//! 测试盯着，这里不重测。

use super::*;
// ⚠ 这两类都从 `mcloud_tasks::exchange` 转出来引，**不要**写 `mcloud_upstream`：
// `mcloud-server` 不依赖那个 crate（只依赖 `mcloud-app` 与 `mcloud-tasks`），
// 为了两个类型去加一条依赖会把"server 不直接碰上游协议层"这条边界拆掉。
use mcloud_tasks::exchange::{
    ExchangeOrder, ExchangeOutcome, ExchangePlan, ExchangePrize, ExchangeStatus,
};

/// 造一份读数齐全的 outcome。
fn outcome(status: ExchangeStatus, plan: ExchangePlan) -> ExchangeOutcome {
    ExchangeOutcome {
        status,
        plan,
        prize: ExchangePrize {
            prize_id: 251_230_051,
            name: Some("移动云盘100G云空间月卡".into()),
            price: Some(1800),
            ..ExchangePrize::default()
        },
        balance: 5000,
        after_balance: Some(3200),
        ..ExchangeOutcome::default()
    }
}

/// ⭐ **任务自报的键名与路由读的键名必须对得上**。
///
/// 这条是整条 `details` 管线的接缝：`ExchangeTask::run` 用
/// `mcloud_tasks::exchange::detail_payload` 写出形状，本路由用字面量键名读回来。
/// 两边任一侧改名字，**编译都过、测试之外没有任何东西会红** ——
/// 界面上只是"余额那一栏恒为 0"。
#[test]
fn every_field_the_route_reads_is_actually_reported() {
    let payload =
        mcloud_tasks::exchange::detail_payload(&outcome(ExchangeStatus::Ordered, ExchangePlan::Go));

    for key in [
        "status",
        "plan",
        "plan_reason",
        "code",
        "message",
        "rounds",
        "attempts",
        "offset",
        "balance",
        "after_balance",
        "waited_until_ms",
        "device_id_failed",
        "error",
        "order",
        "reached",
    ] {
        assert!(
            payload.get(key).is_some(),
            "detail_payload 没给 {key}（而路由会去读它）—— 实得：{payload}"
        );
    }
    // 奖品那几个字段在嵌套那一层，单独点名。
    for key in ["prize_id", "name", "price"] {
        assert!(
            payload["prize"].get(key).is_some(),
            "detail_payload 的 prize 里没有 {key}：{payload}"
        );
    }
}

/// 档位名逐档点名，且**互不相同**（重名会让界面把两档染成同一个颜色）。
#[test]
fn every_status_has_a_distinct_slug() {
    use mcloud_tasks::exchange::status_slug;
    let all = [
        ExchangeStatus::NotAttempted,
        ExchangeStatus::Ordered,
        ExchangeStatus::Skipped,
        ExchangeStatus::AlreadyExchanged,
        ExchangeStatus::SliderUnresolved,
        ExchangeStatus::NotTargetUser,
        ExchangeStatus::Unknown,
        ExchangeStatus::Failed,
    ];
    let mut seen = std::collections::HashSet::new();
    for status in all {
        let slug = status_slug(status);
        assert!(!slug.is_empty(), "{status:?} 没有 slug");
        assert!(seen.insert(slug), "slug 重复：{slug}");
    }
}

/// ⭐ 成交那一趟读回来的字段要**带得动事实**，不能被压成默认值。
#[test]
fn an_ordered_outcome_survives_the_round_trip() {
    let mut o = outcome(ExchangeStatus::Ordered, ExchangePlan::Go);
    o.attempts = 1;
    o.rounds = 1;
    o.offset = Some(237);
    o.code = 0;
    o.order = Some(ExchangeOrder {
        order_id: 88_001,
        prize_id: 251_230_051,
        prize_name: Some("移动云盘100G云空间月卡".into()),
        cost: Some(1800),
        insert_time: Some("2026-09-19 12-16-03".into()),
        ..ExchangeOrder::default()
    });

    let payload = mcloud_tasks::exchange::detail_payload(&o);
    let view = run_response(
        7,
        "兑到「移动云盘100G云空间月卡」，价 1800 豆",
        Some(&payload),
    );

    assert_eq!(view.run_id, 7);
    assert_eq!(view.status, "ordered");
    assert_eq!(view.plan, "go");
    assert_eq!(view.attempts, 1, "枪数是这一族最要紧的读数之一");
    assert_eq!(view.offset, Some(237));
    assert_eq!(view.balance, 5000);
    assert_eq!(view.after_balance, Some(3200));
    assert!(view.reached, "成交这一趟必然是走到了结论的");
    let order = view.order.expect("成交必须带回那一单");
    assert_eq!(order["order_id"], 88_001);
    assert_eq!(order["cost"], 1800);
}

/// ⚠ **一枪未发的档位不能被读成"成交"**。
///
/// `attempts == 0` 是本模块最重要的一条不变式（判据没过 ⇒ 一条写请求都不发）。
/// 它在这条路上要能穿过 `details` 到达界面，否则用户看到"跳过"却看不到
/// "一分豆都没花"。
#[test]
fn a_skipped_outcome_reports_zero_attempts_and_a_reason() {
    let mut o = outcome(ExchangeStatus::Skipped, ExchangePlan::ExchangedThisPrize);
    o.after_balance = None;

    let payload = mcloud_tasks::exchange::detail_payload(&o);
    let view = run_response(9, "没兑换：这一件本月已经兑过了（limit=2）", Some(&payload));

    assert_eq!(view.status, "skipped");
    assert_eq!(view.plan, "exchanged_this_prize");
    assert_eq!(view.attempts, 0, "判据没过时一枪都不能发");
    assert_eq!(view.rounds, 0);
    assert!(view.offset.is_none(), "没发过单就不该有 offset");
    assert!(view.order.is_none());
    assert!(view.reached);
    // 原因必须带着走：界面那一行 disabled 的按钮靠它说"为什么不能兑"。
    assert!(
        view.plan_reason.contains("本月已经兑过"),
        "实得：{}",
        view.plan_reason
    );
}

/// ⚠ 断在只读查询上时，**不许编一个档位**。
///
/// `ExchangePlan` 的零值是 `Go`（"可以下单"）—— 一份漏填/缺字段的读数
/// 如果被读成 `go`，界面就会显示"可兑"。这条钉住"缺了就如实说缺了"。
#[test]
fn a_missing_report_says_so_instead_of_inventing_a_plan() {
    let view = run_response(3, "兑换没跑起来：读取兑换清单失败", None);

    assert!(!view.reached);
    assert_eq!(view.status, "not_attempted");
    assert_eq!(view.plan, "", "没判过就不该有档位（尤其不能是 go）");
    assert_eq!(view.plan_reason, "");
    assert_eq!(view.attempts, 0);
    assert!(!view.error.is_empty(), "要说清为什么读不到");
    // 那句话仍要传达给用户 —— 它是这一趟唯一的事实。
    assert_eq!(view.summary, "兑换没跑起来：读取兑换清单失败");
}

/// `Err` 那条路自报的形状：`reached: false` + 一句原因。
#[test]
fn an_unreached_run_is_reported_as_not_reached() {
    let payload = serde_json::json!({
        "kind": "exchange",
        "reached": false,
        "error": "读取兑换清单失败：HTTP 502",
    });
    let view = run_response(11, "兑换没跑起来", Some(&payload));

    assert!(!view.reached);
    assert_eq!(view.error, "读取兑换清单失败：HTTP 502");
    // 没跑起来就不该有任何"下单过"的痕迹。
    assert_eq!(view.attempts, 0);
    assert!(view.order.is_none());
}

// ── `details.reported` 的两种形状 ────────────────────────────────────

/// 自报**一条** ⇒ runner 不裹数组，路由要能读对象。
#[test]
fn reported_reads_a_bare_object() {
    let details = serde_json::json!({
        "reported": { "status": "skipped", "attempts": 0 }
    });
    let got = reported_of(&details).expect("对象形态必须读得到");
    assert_eq!(got["status"], "skipped");
}

/// 自报**多条** ⇒ 数组，取**最后**一条（那是收尾时的读数）。
///
/// 只按对象读的话这一格会**静默变成 `None`** —— 表现是
/// "兑成功了但页面什么都不显示"，而那不报错。
#[test]
fn reported_reads_the_last_item_of_an_array() {
    let details = serde_json::json!({
        "reported": [
            { "status": "skipped", "attempts": 0 },
            { "status": "ordered", "attempts": 1 },
        ]
    });
    let got = reported_of(&details).expect("数组形态必须读得到");
    assert_eq!(got["attempts"], 1, "要取最后一条，不是第一条");
}

/// 没有 `reported` 这一格（例如产物-only 的运行）⇒ `None`，不 panic。
#[test]
fn reported_is_none_when_absent_or_null() {
    assert!(reported_of(&serde_json::json!({})).is_none());
    assert!(reported_of(&serde_json::json!({ "reported": null })).is_none());
    assert!(reported_of(&serde_json::json!({ "reported": [] })).is_none());
    // ⚠ `artifacts` 那一格与 `reported` 共存，不能被误当成明细。
    let artifacts_only = serde_json::json!({ "artifacts": [{ "id": "F1" }] });
    assert!(reported_of(&artifacts_only).is_none());
}

/// `skipWait` 缺省必须是 `true`。
///
/// ⚠ 这一条防的是"改成一个裸 `bool` 字段"：那样零值 `false` 会让
/// 每个不带这个字段的请求**挂在那里等整点**（最长一小时），
/// 而请求看起来只是"很慢" —— 没有任何一处会报错。
#[test]
fn skip_wait_defaults_to_true() {
    let req: ExchangeRequest = serde_json::from_str(r#"{"prizeIdOrName":"251230051"}"#).unwrap();
    assert!(req.skip_wait, "缺省必须立即试，不是等到整点");
    assert_eq!(req.prize_id_or_name, "251230051");

    let req: ExchangeRequest =
        serde_json::from_str(r#"{"prizeIdOrName":"x","skipWait":false}"#).unwrap();
    assert!(!req.skip_wait, "显式给了 false 就要认");
}

/// 空请求体也要能被接住（走我们自己那句中文 400，而不是 axum 的 422 英文）。
#[test]
fn an_empty_body_deserializes_to_an_empty_request() {
    let req: ExchangeRequest = serde_json::from_str("{}").unwrap();
    assert!(req.prize_id_or_name.is_empty());
    assert!(req.skip_wait);
}

// ── 只读那一侧的映射 ────────────────────────────────────────────────

/// 档位串与"能不能兑"必须一致：`blocks == (plan != "go")`。
#[test]
fn prize_view_keeps_plan_and_blocks_consistent() {
    let prize = ExchangePrize {
        prize_id: 1,
        name: Some("A".into()),
        price: Some(10),
        ..ExchangePrize::default()
    };

    let go = prize_view(&prize, ExchangePlan::Go);
    assert_eq!(go.plan, "go");
    assert!(!go.blocks, "go 不该被判成挡住");

    let blocked = prize_view(&prize, ExchangePlan::InsufficientCloud);
    assert_eq!(blocked.plan, "insufficient_cloud");
    assert!(blocked.blocks);
    assert_eq!(
        blocked.plan_reason,
        ExchangePlan::InsufficientCloud.reason()
    );
}

/// ⭐ **十七档逐档都要能从清单路由里出去**，且"可兑"只有一档。
///
/// 这条同时钉住"界面上那些不可兑的档位各有各的说法"：如果有两档
/// `plan` 串相同，前端那张颜色表会把它们染成同一个颜色 ——
/// 而需求明确要求"本月已兑"与"余额不足"不能显示成同一句。
#[test]
fn every_plan_is_distinguishable_from_the_list_route() {
    let prize = ExchangePrize {
        prize_id: 1,
        name: Some("A".into()),
        price: Some(10),
        ..ExchangePrize::default()
    };

    let mut slugs = std::collections::HashSet::new();
    let mut reasons = std::collections::HashSet::new();
    for plan in ExchangePlan::ALL {
        let view = prize_view(&prize, plan);
        assert_eq!(view.plan, plan.slug());
        assert_eq!(view.blocks, plan.blocks());
        assert!(
            slugs.insert(view.plan),
            "档位串重复：{}（界面会把两档染成同一个颜色）",
            plan.slug()
        );
        assert!(
            reasons.insert(view.plan_reason),
            "两档的人话一模一样：{}（用户分不清该等明天还是换一件）",
            plan.reason()
        );
    }
    assert_eq!(slugs.len(), 17);
    // 可兑**只有一档** —— 多一档就等于多一处"什么情况下可以花真豆"。
    assert_eq!(
        ExchangePlan::ALL.iter().filter(|p| !p.blocks()).count(),
        1,
        "放行的档位必须唯一"
    );
}
