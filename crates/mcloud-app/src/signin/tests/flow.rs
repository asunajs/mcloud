// 由 signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_upstream::Upstream;
use mcloud_upstream::app_client::AppProfile;
use serde_json::json;
// ───────────────────────── 主流程 ─────────────────────────

/// 四步都成功、今天还没签 → 应当发满 6 个请求，并以 infoV3 的**回查**为准。
#[tokio::test]
async fn signs_when_today_is_not_signed_yet() {
    let client = ScriptedClient::new(vec![
        // ① querySpecTokenV2
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
        // ② api/timestamp
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        // ③ tyrzLogin
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        // ④ infoV3（签到前）：今天未签
        info_v3_with_today_signed(false),
        // ⑤ startSignIn
        json!({ "code": 0, "msg": "success", "result": {
            "todaySignIn": false, "signInPoints": 3, "signCount": 2,
            "time": 1789799929696i64 } }),
        // ⑥ infoV3（回查）：今天已签
        info_v3_with_today_signed(true),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();

    let report = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions::default(),
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    assert!(report.signed_now(), "今天从「未签」变成了「已签」");
    assert!(report.signed_today());
    assert_eq!(report.sign_in_points(), Some(3));
    assert_eq!(client.sent().len(), 6, "四段链路一共 6 个请求");

    // 顺序：三段鉴权在前，两次 infoV3 夹着一次 startSignIn
    assert!(client.url_of(0).contains("/user/querySpecTokenV2"));
    assert!(client.url_of(1).ends_with("/ycloud/api/timestamp"));
    assert!(
        client
            .url_of(2)
            .contains("/ycloud/auth-service/auth/tyrzLogin")
    );
    assert!(client.url_of(3).contains("/ycloud/signin/page/infoV3"));
    assert!(client.url_of(4).contains("/ycloud/signin/page/startSignIn"));
    assert!(client.url_of(5).contains("/ycloud/signin/page/infoV3"));

    // `todaySignIn:false` 出现在**成功的**那一次 —— 它正是"调用前快照"的证据。
    // 如果哪天有人把它改成成败判据，这条断言会连同上面 `signed_now()` 一起炸。
    assert!(!report.result.as_ref().unwrap().today_sign_in);
}

/// ⚠ 这条锁住 `todaySignIn` 的反直觉语义：它**不能**当成败判据。
///
/// 构造"`startSignIn` 回 `todaySignIn:true`（= 来的时候就已经签过了），
/// 但回查 `infoV3` 说今天没签" —— 这种矛盾下必须**报错**而不是报成功。
/// 真实世界里它对应"上游把我们的签到吞了"。
#[tokio::test]
async fn refuses_success_when_info_v3_still_says_unsigned() {
    let client = ScriptedClient::new(vec![
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        info_v3_with_today_signed(false),
        // 上游回了成功，而且 `todaySignIn:true`
        json!({ "code": 0, "msg": "success", "result": {
            "todaySignIn": true, "signInPoints": 3, "signCount": 2,
            "time": 1789799929696i64 } }),
        // 回查说**还是没签** —— 以这个为准
        info_v3_with_today_signed(false),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();

    let err = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions::default(),
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .expect_err("回查说没签上就不能报成功");

    assert_eq!(err.kind(), mcloud_core::ErrorKind::Internal, "{err}");
    assert!(format!("{err}").contains("infoV3"), "{err}");
}

/// 今天已经签过 → **不发** `startSignIn`（少一次写请求、少一次风控记数）。
#[tokio::test]
async fn skips_start_sign_in_when_already_signed() {
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

    let report = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions::default(),
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    assert_eq!(client.sent().len(), 4, "已签过就只走鉴权 + 一次 infoV3");
    assert!(report.signed_today(), "今天确实已签");
    assert!(!report.signed_now(), "但**不是这次**签上的");
    assert!(report.result.is_none());
    assert_eq!(report.before, report.after, "没再查第二次");
}

/// `--force`（协议自检）：已签也发一次 `startSignIn`，
/// 但**不能**把这次报成"刚签上的" —— 否则日志里会凭空多出一次签到成功。
#[tokio::test]
async fn force_resends_start_sign_in_without_claiming_a_fresh_sign_in() {
    let client = ScriptedClient::new(vec![
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        // 今天已经签过
        info_v3_with_today_signed(true),
        // 上游对"同一天重复签到"回成功 —— 实拍里真实客户端就是这么被回的
        // （黄金向量 `startSignIn` 那条：`todaySignIn:true` + `success`）
        json!({ "code": 0, "msg": "success", "result": {
            "todaySignIn": true, "signInPoints": 3, "signCount": 2,
            "time": 1789799929696i64 } }),
        info_v3_with_today_signed(true),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();

    let report = sign_in(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &SignInOptions {
            force: true,
            ..Default::default()
        },
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    assert_eq!(client.sent().len(), 6, "force 下已签也要发 startSignIn");
    assert!(report.was_already_signed(), "来的时候就已经签过");
    assert!(report.signed_today());
    assert!(!report.signed_now(), "本来就是已签，不能说成「这次签上的」");
}

#[tokio::test]
async fn status_only_never_signs() {
    let client = ScriptedClient::new(vec![
        json!({ "success": true, "code": "0000", "message": "请求成功",
                "data": { "token": "SSOTOKEN", "resultCode": "104000" } }),
        json!({ "code": 0, "msg": "success", "result": 1789800128813i64 }),
        json!({ "code": 0, "msg": "success",
                "result": { "token": "JWT", "mobile": "19900000001" } }),
        info_v3_with_today_signed(false),
    ]);
    let upstream = Upstream::new(&client);
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();

    let s = status(
        &upstream,
        &AppProfile::new(app_device()),
        &credential,
        &fixed_clock(),
        &mut SeqRandom(7),
    )
    .await
    .unwrap();

    assert_eq!(client.sent().len(), 4, "只查不签");
    assert_eq!(s.signed_today(), Some(false));
    assert!(!client.sent().iter().any(|r| r.url.contains("startSignIn")));
}
