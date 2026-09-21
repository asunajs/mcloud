// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;
use mcloud_core::Error;

// ───────────────────────── 失败路径 ─────────────────────────

#[test]
fn every_real_error_envelope_is_classified() {
    let v = vectors();
    for entry in v["envelopes"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        if entry["code"].as_i64().unwrap_or(0) == 0 {
            continue;
        }
        let envelope = Envelope::new(entry["raw"].clone());
        assert!(envelope.ensure_ok().is_err(), "{name} 应被判失败");
    }
}

#[test]
fn not_logged_in_error_carries_an_actionable_hint() {
    let envelope = Envelope::new(serde_json::json!({"code": 90001, "msg": "未登录"}));
    let err = parse_sign_in_status(&envelope).unwrap_err();
    let detail = format!("{err}");
    assert!(detail.contains("90001"), "{detail}");
    assert!(
        detail.contains("jwttoken"),
        "提示要指向真正的原因：{detail}"
    );
    // 对外文案仍走白名单，不夹带上游原文
    assert_eq!(err.public_message(), "请求参数不合法");
}

#[test]
fn diagnose_covers_the_three_debugging_signals() {
    assert!(diagnose(CODE_NOT_LOGGED_IN).unwrap().contains("jwttoken"));
    assert!(diagnose(CODE_SSO_EXPIRED).unwrap().contains("立刻用"));
    assert!(diagnose(CODE_SIGNATURE_REJECTED).unwrap().contains("签名"));
    assert_eq!(diagnose(0), None);
}

/// 领取类的三个码**必须都有提示**，而且提示里要能看出"这是领取失败" ——
/// 否则它们会掉进 `None` 那一档，被当成"没有已知提示"的陌生错误。
#[test]
fn diagnose_covers_the_receive_codes() {
    assert!(diagnose(CODE_RECEIVE_TOO_BUSY).unwrap().contains("火爆"));
    assert!(diagnose(CODE_RECEIVE_FAILED).unwrap().contains("领取失败"));
    assert!(
        diagnose(CODE_RECEIVE_FAILED_ALT)
            .unwrap()
            .contains("领取失败")
    );
}

/// `614` 与 `615` 的提示必须**不一样**。
///
/// 上游把它们归成同一句 toast（「领取失败，请稍后再试」），但实测是两件事：
/// 一个是"没带"、一个是"带了但内容无效"。合并成一条提示，
/// 等于把"该去查参数有没有传"与"该去查参数对不对"这两条排查路径又混回去。
#[test]
fn diagnose_distinguishes_a_missing_device_id_from_an_invalid_one() {
    let missing = diagnose(CODE_RECEIVE_FAILED_ALT).unwrap();
    let invalid = diagnose(CODE_RECEIVE_FAILED).unwrap();

    assert_ne!(missing, invalid);
    assert!(missing.contains("没带"), "615 应指向「没带」：{missing}");
    assert!(
        invalid.contains("无效"),
        "614 应指向「内容无效」：{invalid}"
    );
}

#[test]
fn missing_required_field_is_a_parse_error_not_a_silent_zero() {
    // `total`（余额）缺失时**不能**静默成 0 —— 那会让"余额 0"看起来像真实数据，
    // 而它是要被记进 `cloud_stats` 的。
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"signInPoints": 3, "signCount": 1, "monthDays": 1, "cal": []}
    }));
    let err = parse_sign_in_status(&envelope).unwrap_err();
    assert!(matches!(err, Error::Parse(_)), "应是解析错误：{err}");
    assert!(
        format!("{err}").contains("total"),
        "要点名缺的是哪个字段：{err}"
    );

    // ⚠ 对照组：`beforeTotal` 是**我们不用的旁证数**，缺了**不该**打挂整条调用。
    // 这两条合起来就是"两种字段用两把尺子"——判据是"缺了它还能不能回答问题"。
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": 7431, "signInPoints": 3, "signCount": 1, "monthDays": 1,
            "cal": [{"d": 1, "s": true, "n": 3, "p": 0, "t": true, "currentMonth": 1}]
        }
    }));
    let status = parse_sign_in_status(&envelope).unwrap();
    assert_eq!(status.bean_total, 7431);
    assert_eq!(status.cloud_total, None, "缺了给 `None`，不是 0");
}

#[test]
fn calendar_day_out_of_range_is_rejected() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0,
        "result": {
            "beforeTotal": 1, "signInPoints": 1, "signCount": 1, "monthDays": 0,
            "cal": [{"d": 999, "s": false, "n": 0, "p": 0, "t": false, "currentMonth": 1}]
        }
    }));
    assert!(parse_sign_in_status(&envelope).is_err());
}
