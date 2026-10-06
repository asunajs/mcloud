//! 登录路由的**纯函数**测试。
//!
//! 端到端的接线（真实 Router + cookie + 落盘）在 `tests/login_routes.rs`。
//! 这里只测不依赖 `ServerState` 的两块逻辑：平台解析与失败措辞。

use super::*;

#[test]
fn parse_platform_accepts_both_spellings() {
    // 用 `matches!` 而不是 `assert_eq!`：`ApiError` 刻意没有 `Debug`
    // （它的文本没走脱敏，打印出来可能带凭据），所以不能 `unwrap()`。
    assert!(matches!(parse_platform("windows"), Ok(PcPlatform::Windows)));
    assert!(matches!(parse_platform("macos"), Ok(PcPlatform::MacOs)));
    assert!(matches!(parse_platform("Mac"), Ok(PcPlatform::MacOs)));
}

/// 未知平台必须报 `Config`（→ 400）并带上用户输入。
///
/// 不能静默落成 `windows`：那会让"WebUI 以为在出 mac 的码、
/// 实际发的是 win 请求"这类错误一路静默到落盘。
#[test]
fn unknown_platform_is_a_config_error_naming_the_input() {
    let e = parse_platform("linux").unwrap_err();
    assert!(
        matches!(e.0, mcloud_core::Error::Config(_)),
        "必须是 400 类"
    );
    assert!(e.0.to_string().contains("linux"), "要带上用户输入");
}

/// 每一种失败都有**能给人看**的措辞，且不吐凭据。
#[test]
fn every_failure_has_a_human_message() {
    use mcloud_tasks::login::QrFailure as F;

    let cases = [
        F::TtlElapsed { ttl_seconds: 120 },
        F::ExpiredUpstream,
        F::SessionMismatch,
        F::TooFrequent,
        F::Rejected {
            code: Some(200059541),
            message: "未扫".into(),
        },
        F::Rejected {
            code: None,
            message: "无码".into(),
        },
    ];

    for case in cases {
        let text = describe_failure(&case);
        assert!(!text.is_empty(), "{case:?} 没有说明");
        // 上游的原文必须保留（新码靠它才查得出来）
        if let F::Rejected { message, .. } = &case {
            assert!(text.contains(message.as_str()), "{case:?} 丢了上游原文");
        }
    }
}

/// 限流那条要说清"会自动重试"，而不是让用户以为要重开。
#[test]
fn too_frequent_says_retry_is_automatic() {
    let text = describe_failure(&mcloud_tasks::login::QrFailure::TooFrequent);
    assert!(text.contains("自动"), "实得：{text}");
}

/// 会话表的增删查：`take` 之后就没了吗？
#[test]
fn session_table_take_and_remove() {
    use mcloud_core::UserId;
    use mcloud_crypto::SeqRandom;

    let table = LoginSessions::new();
    let owner = UserId::new("u1");
    let mut rng = SeqRandom::new(1);
    let qr = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng, 0);
    let sid = qr.session_id().to_string();
    table.put_back(sid.clone(), qr, "tid-1".into(), owner.clone(), 0);

    // 取出后表里就没了（避免两个请求同时推进同一个会话）
    let (qr, tid, who) = table.take(&sid).expect("应当取得到");
    assert_eq!(tid, "tid-1");
    assert_eq!(who, owner, "发起者要跟着会话一起取出来（审计 SEC-06）");
    assert!(table.take(&sid).is_none(), "取出后不该还在");

    // 放回去又能取到
    table.put_back(sid.clone(), qr, "tid-1".into(), owner.clone(), 0);
    assert!(table.take(&sid).is_some());

    // 显式移除
    let mut rng = SeqRandom::new(2);
    let qr2 = QrLogin::start(PcPlatform::MacOs, "DEV", &mut rng, 0);
    let sid2 = qr2.session_id().to_string();
    table.put_back(sid2.clone(), qr2, "t".into(), owner.clone(), 0);
    assert!(table.remove_for(&sid2, &owner), "自己的会话该删得掉");
    assert!(table.take(&sid2).is_none());
}

/// ⭐⭐ 审计 `SEC-06`：扫码会话**只归发起它的那个人**。
///
/// 不绑发起者的话，`session_id` 就是一张"谁拿到谁能用"的通行证 ——
/// 第三方可以推进别人的会话，把"扫码成功后账号落进谁名下"从受害者手里夺走 ✗。
///
/// 红测：把 `remove_for` 里的 `s.owner == owner` 判断去掉（退回旧的 `remove`）
/// ⇒ 本测试后两条断言红。
#[test]
fn a_qr_session_belongs_to_the_user_who_started_it() {
    use mcloud_core::UserId;
    use mcloud_crypto::SeqRandom;

    let table = LoginSessions::new();
    let alice = UserId::new("alice");
    let bob = UserId::new("bob");
    let mut rng = SeqRandom::new(11);
    let qr = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng, 0);
    let sid = qr.session_id().to_string();
    table.put_back(sid.clone(), qr, "tid".into(), alice.clone(), 0);

    // 别人删不掉，而且**那条会话还在**（不能因为"不是你的"就顺手丢掉 ——
    // 否则猜一个 id 就成了一次对别人扫码流程的拒绝服务 ✗）
    assert!(!table.remove_for(&sid, &bob), "别人的会话不该删得掉");
    assert!(table.take(&sid).is_some(), "没删掉就得还在");

    // 放回去，发起者自己删得掉
    let mut rng = SeqRandom::new(12);
    let qr = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng, 0);
    let sid2 = qr.session_id().to_string();
    table.put_back(sid2.clone(), qr, "tid".into(), alice.clone(), 0);
    assert!(table.remove_for(&sid2, &alice), "发起者该删得掉");
}

/// ⚠⚠ 复审 `sv-7`：**插入时要把过期的扫掉**（这张表原先只进不出）。
///
/// 没有这一扫的话，"出一张码但没轮询到终态"（点开看一眼就走 / 刷新页面 /
/// 超时后前端不再轮询）每发生一次就留一条**常驻**条目，长跑实例里单调增长 ✗。
///
/// 红测：把 `insert` 里那句 `retain(|_, s| !s.qr.is_expired(now_ms))` 删掉 ⇒ 红。
#[test]
fn inserting_sweeps_the_expired_sessions() {
    use mcloud_core::UserId;
    use mcloud_crypto::SeqRandom;

    let table = LoginSessions::new();
    let owner = UserId::new("u1");
    let mut rng = SeqRandom::new(7);

    // 一张**只有 1 秒**有效期的码（`with_ttl` 是"把码发给别人扫"用的那条口子）
    let stale = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng, 0).with_ttl(1);
    let stale_id = stale.session_id().to_string();
    table.put_back(stale_id.clone(), stale, "tid-old".into(), owner.clone(), 0);
    assert_eq!(table.qr.lock().unwrap().len(), 1, "先是插进去了");

    // 5 秒后再插一条新的 ⇒ 旧的早过期了，必须被顺手扫掉
    let fresh = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng, 5_000);
    let fresh_id = fresh.session_id().to_string();
    table.put_back(
        fresh_id.clone(),
        fresh,
        "tid-new".into(),
        owner.clone(),
        5_000,
    );

    let qr = table.qr.lock().unwrap();
    assert_eq!(qr.len(), 1, "过期那条应当被扫掉，实得 {} 条", qr.len());
    assert!(!qr.contains_key(&stale_id), "过期那条还在表里");
    assert!(qr.contains_key(&fresh_id), "新那条不该被误扫");
}
