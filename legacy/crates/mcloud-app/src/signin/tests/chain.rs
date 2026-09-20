// 由 signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::Clock;
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use serde_json::{Value, json};
// ───────────────────────── 链路上的细节 ─────────────────────────

/// `referer` 必须带上本次的 SSO token，且**逐字**是抓包里那个形状。
#[tokio::test]
async fn referer_carries_the_sso_token_and_the_signin_page_shape() {
    let client = ScriptedClient::new(vec![
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        info_v3_with_today_signed(true),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();
    let _ = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions::default(),
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    let expected = mcloud_upstream::app_signin::signin_page_url("SSOTOKEN");
    // ③ 之后的每个请求都带同一个 referer
    for index in 2..4 {
        assert_eq!(
            client.header_of(index, "Referer").as_deref(),
            Some(expected.as_str()),
            "第 {index} 个请求的 referer 不对"
        );
    }
    assert!(expected.contains("targetSourceId=001005"), "{expected}");
}

/// 签名头必须与 `api/timestamp` 取回的那个值**逐字相同**。
///
/// 这是"不能用本地时钟"那条规则的可执行形式：本地时间是 `2026-09-19T06:00:00Z`，
/// 而服务端给的是 `1789800128813` —— 两者差着好几年，一旦有人改成读本地时钟，
/// 这条断言立刻失败。
#[tokio::test]
async fn signature_uses_the_server_timestamp_not_the_local_clock() {
    let client = ScriptedClient::new(vec![
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        info_v3_with_today_signed(true),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();
    let _ = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions::default(),
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    assert_eq!(
        client.header_of(2, "x-timestamp").as_deref(),
        Some("1789800128813")
    );
    let local = fixed_clock().now().timestamp_millis().to_string();
    assert_ne!(
        client.header_of(2, "x-timestamp").as_deref(),
        Some(local.as_str()),
        "用本地时钟就是 401"
    );
}

/// `querySpecTokenV2` 必须带完整手机号拼出来的 `Authorization`，
/// 而**不是**掩码。掩码能过与否是未知数，所以不许出现。
#[tokio::test]
async fn query_spec_token_uses_the_full_phone_in_authorization() {
    let client = ScriptedClient::new(vec![
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        info_v3_with_today_signed(true),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();
    let _ = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions::default(),
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    let auth = client.header_of(0, "Authorization").unwrap();
    assert_eq!(auth, AppProfile::authorization("19900000001", "AUTHTOKEN"));
    let decoded = mcloud_core::parse_auth(&auth).unwrap();
    assert_eq!(decoded.phone.as_deref(), Some("19900000001"));
    assert_ne!(decoded.phone.as_deref(), Some("19****01"));
}

/// 换来的 SSO token 必须原样进 `tyrzLogin` 的请求体，不许截断/转义。
#[tokio::test]
async fn sso_token_is_passed_through_verbatim() {
    let client = ScriptedClient::new(vec![
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "YZsidssolgSYNTH000000000000000000000000000",
                          "resultCode": "104000" } }),
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        info_v3_with_today_signed(true),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();
    let _ = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions::default(),
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    let body: Value = serde_json::from_slice(client.sent()[2].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body["token"],
        json!("YZsidssolgSYNTH000000000000000000000000000")
    );
    assert_eq!(body["marketName"], json!("sign_in_3"));
    assert_eq!(body["sourceId"], json!("1002"));
}

/// `device_id` 是 `Option`，但**传与不传必须在 URL 上看得出来**。
#[tokio::test]
async fn device_id_is_optional_but_visible_when_given() {
    async fn run(device_id: Option<&str>) -> String {
        let client = ScriptedClient::new(vec![
            json!({ "success": true, "code": "0000", "message": "请求成功",
                    "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
            json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
            json!({ "code": 0, "msg": "success",
                    "result": { "token": "JWT", "mobile": "19900000001" } }),
            info_v3_with_today_signed(false),
            json!({ "code": 0, "msg": "success", "result": {
                "todaySignIn": false, "signInPoints": 3, "signCount": 2,
                "time": 1789799929696i64 } }),
            info_v3_with_today_signed(true),
        ]);
        let upstream = Upstream::new(&client);
        let (store, scope, _) = store_with_login().await;
        let credential = SignInCredential::from_phone(&store, scope, "19900000001")
            .await
            .unwrap();
        let _ = sign_in(
            &upstream,
            &AppProfile::new(app_device()),
            &credential,
            &SignInOptions {
                device_id,
                ..Default::default()
            },
            &fixed_clock(),
            &mut SeqRandom(7),
        )
        .await
        .unwrap();
        client.url_of(4)
    }

    let without = run(None).await;
    assert_eq!(
        without,
        "https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app"
    );

    // 传了就要百分号编码：base64 里的 `+` 在查询串里表示空格，
    // 不编码服务端会解成另一个值**而且不报错**。
    let with = run(Some("a+b/c=")).await;
    assert!(with.ends_with("&deviceId=a%2Bb%2Fc%3D"), "{with}");
}
