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
    use mcloud_crypto::SeqRandom;

    let table = LoginSessions::new();
    let mut rng = SeqRandom::new(1);
    let qr = QrLogin::start(PcPlatform::Windows, "DEV", &mut rng, 0);
    let sid = qr.session_id().to_string();
    table.put_back(sid.clone(), qr, "tid-1".into());

    // 取出后表里就没了（避免两个请求同时推进同一个会话）
    let (qr, tid) = table.take(&sid).expect("应当取得到");
    assert_eq!(tid, "tid-1");
    assert!(table.take(&sid).is_none(), "取出后不该还在");

    // 放回去又能取到
    table.put_back(sid.clone(), qr, "tid-1".into());
    assert!(table.take(&sid).is_some());

    // 显式移除
    let mut rng = SeqRandom::new(2);
    let qr2 = QrLogin::start(PcPlatform::MacOs, "DEV", &mut rng, 0);
    let sid2 = qr2.session_id().to_string();
    table.put_back(sid2.clone(), qr2, "t".into());
    table.remove(&sid2);
    assert!(table.take(&sid2).is_none());
}
