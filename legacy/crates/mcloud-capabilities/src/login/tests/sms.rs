// 由本模块的 `mod tests` 按主题拆出；helper 见同目录 mod.rs。
use super::*;

use serde_json::json;

#[tokio::test]
async fn sms_login_reports_slider_downgrade_and_tracks_quota() {
    let client = ScriptedClient::new(vec![
        json!({"success": false, "code": "200059554", "data": {"captchaType": 2}}),
        json!({"success": false, "code": "200059553", "data": {"captchaType": 1}}),
        json!({"success": true, "code": "0000", "data": {"remainGetTimes": 8}}),
    ]);
    let upstream = Upstream::new(&client);
    let mut sms = SmsLogin::start(
        PcPlatform::Windows,
        "DEV-1",
        "19900000000",
        &mut SeqRandom(0),
    );

    // ① 首次发码被风控要求滑块
    assert_eq!(
        sms.send_code(&upstream, "tid").await.unwrap(),
        SendSmsOutcome::NeedSlider {
            captcha_type: Some(2)
        }
    );
    // ② 带滑块重试，但挑战已被判废 —— 调用方**必须重新取图**
    assert_eq!(
        sms.send_code_with_slider(&upstream, "tid", "461")
            .await
            .unwrap(),
        SendSmsOutcome::SliderExhausted {
            captcha_type: Some(1)
        }
    );
    // ③ 换新挑战后成功，配额被记下来
    sms.reroll(&mut SeqRandom(9));
    assert_eq!(
        sms.send_code_with_slider(&upstream, "tid", "391")
            .await
            .unwrap(),
        SendSmsOutcome::Sent {
            remain_get_times: Some(8)
        }
    );
    assert_eq!(sms.remain_get_times(), Some(8));

    // 第三次请求体里必须同时有 24 位 random 与加密后的滑块答案
    let body: Value =
        serde_json::from_slice(client.seen.borrow()[2].body.as_ref().unwrap()).unwrap();
    assert_eq!(body["random"].as_str().unwrap().len(), 24);
    assert_eq!(
        body["puzzleVerfycode"],
        "RTUyNkUxRjNGOTc2NTQ0NEA6F1MQJO7Ejcwfa1IsHCs="
    );
}

#[tokio::test]
async fn sms_login_decrypts_user_info_on_success() {
    let client = ScriptedClient::new(vec![
        json!({"success": true, "code": "0000", "data": user_info().to_string()}),
    ]);
    let upstream = Upstream::new(&client);
    let sms = SmsLogin::start(
        PcPlatform::Windows,
        "DEV-1",
        "19900000000",
        &mut SeqRandom(0),
    );

    let info = sms
        .login(&upstream, "tid", "119301", &PlainDecryptor)
        .await
        .unwrap();
    assert_eq!(info["account"], "19900000000");

    let body: Value =
        serde_json::from_slice(client.seen.borrow()[0].body.as_ref().unwrap()).unwrap();
    assert_eq!(body["pintype"], "5");
    assert_eq!(body["dycpwd"], "119301", "短信方式下 dycpwd 是明文验证码");
    assert_eq!(body["extInfo"]["ifOpenAccount"], 1);
}

#[tokio::test]
async fn sms_login_surfaces_business_failure_without_inventing_a_reason() {
    let client = ScriptedClient::new(vec![
        json!({"success": false, "code": "200059526", "message": "登录保护"}),
    ]);
    let upstream = Upstream::new(&client);
    let sms = SmsLogin::start(
        PcPlatform::Windows,
        "DEV-1",
        "19900000000",
        &mut SeqRandom(0),
    );

    // 上游的码与文案**原样带回**：不臆造"验证码错误"这种我们并不知道的结论
    let err = sms
        .login(&upstream, "tid", "000000", &PlainDecryptor)
        .await
        .unwrap_err();
    let detail = format!("{err}");
    assert!(detail.contains("200059526"), "{detail}");
    assert!(detail.contains("登录保护"), "{detail}");
    assert!(!detail.contains("验证码错误"), "不得臆造原因: {detail}");
}
