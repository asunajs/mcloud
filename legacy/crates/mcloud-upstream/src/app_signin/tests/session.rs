// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::app_client::AppProfile;

// ───────────────────────── 第 1 段：SSO token ─────────────────────────

#[test]
fn query_spec_token_body_is_exactly_to_source_id() {
    assert_eq!(
        query_spec_token_body(TO_SOURCE_ID),
        serde_json::json!({"toSourceId": "001005"})
    );
}

#[test]
fn query_spec_token_request_keeps_app_headers_and_authorization() {
    let profile = AppProfile::new(crate::app_client::AppDevice::ipad(
        "AAA00000000000000000000000000000",
        "AAA000000000",
    ))
    .with_user_domain_id("1990000000000000001");
    let body = query_spec_token_body(TO_SOURCE_ID);
    let request =
        query_spec_token_request(&profile, 1789800128813000, "Basic SYNTH", &body).unwrap();

    assert_eq!(request.method, mcloud_core::HttpMethod::Post);
    assert_eq!(
        request.url,
        "https://user-njs.yun.139.com/user/querySpecTokenV2"
    );
    // 与 App 登录链路同一套头（复用 `AppProfile::headers`），加一个 Authorization
    assert_eq!(header(&request, "Authorization"), Some("Basic SYNTH"));
    assert_eq!(header(&request, "x-yun-uni"), Some("1990000000000000001"));
    assert_eq!(
        header(&request, "x-yun-user-timestamp"),
        Some("1789800128813000")
    );
    assert_eq!(
        header(&request, "Content-Type"),
        Some("application/json;charset=UTF-8"),
        "抓包收的是带 charset 的那个，且只能有一个同名头"
    );
    assert_eq!(
        request
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .count(),
        1,
        "json_body 会自己塞一个 content-type，必须被覆盖掉而不是叠成两个"
    );
}

// ───────────────────────── 第 2 段：时间戳 ─────────────────────────

#[test]
fn h5_timestamp_request_is_bare_get() {
    let request = h5_timestamp_request();
    assert_eq!(request.method, mcloud_core::HttpMethod::Get);
    assert_eq!(request.url, "https://m.mcloud.139.com/ycloud/api/timestamp");
    assert!(request.body.is_none(), "这条是无鉴权裸 GET");
    assert!(header(&request, "jwttoken").is_none());
}

// ───────────────────────── 第 3 段：建会话 ─────────────────────────

#[test]
fn tyrz_login_body_matches_capture() {
    assert_eq!(
        tyrz_login_body("YZsidssolgSYNTH000000000000000000000000000"),
        serde_json::json!({
            "token": "YZsidssolgSYNTH000000000000000000000000000",
            "openAccount": 0,
            "marketName": "sign_in_3",
            "sourceId": "1002",
        })
    );
    // `openAccount` 是**数字** 0，不是字符串 —— 抓包原样如此
    assert!(tyrz_login_body("x")["openAccount"].is_i64());
}

#[test]
fn tyrz_login_request_carries_all_four_sign_headers() {
    let sign = sign_h5_request_with("rid-1", "1789800128813", "nonce-1");
    let body = tyrz_login_body("sso");
    let referer = signin_page_url("sso");
    let request = tyrz_login_request(&sign, &referer, &body).unwrap();

    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin"
    );
    assert_eq!(header(&request, "x-request-id"), Some("rid-1"));
    assert_eq!(header(&request, "x-timestamp"), Some("1789800128813"));
    assert_eq!(header(&request, "x-nonce"), Some("nonce-1"));
    assert_eq!(
        header(&request, "x-signature"),
        Some(sign.signature.as_str())
    );
    assert_eq!(header(&request, "Origin"), Some("https://m.mcloud.139.com"));
    assert_eq!(header(&request, "activityid"), Some(MARKET_NAME));
    assert_eq!(header(&request, "appversion"), Some(H5_APP_VERSION));
    // 还没建会话 —— 这条**不带** jwttoken（带了反而是错的）
    assert!(
        header(&request, "jwttoken").is_none(),
        "tyrzLogin 是建会话那一步，此时还没有 jwt"
    );
}

#[test]
fn signin_page_url_is_the_referer_shape_from_capture() {
    // 期望值**逐字抄自抓包**（HAR 里 128 处 `referer` 头都是这个形状），
    // 不是从实现反推的。踩过的坑：初版漏了末尾的 `&targetSourceId=001005`，
    // 而当时的断言正是照着实现写的，于是"实现漏了、测试也跟着漏"。
    let url = signin_page_url("SSO");
    assert_eq!(
        url,
        "https://m.mcloud.139.com/portal/mobilecloud/index.html?path=newsignin\
         &sourceid=1002&enableShare=1\
         &appID=cy62d5a5e8373a272aeca467accad61722c51cfd48&token=SSO\
         &targetSourceId=001005"
    );
    // `targetSourceId` 必须排在 `token` **之后**：抓包里就是这个顺序。
    //
    // 本链路里 URL 的 `targetSourceId` 与 `querySpecTokenV2` 的 `toSourceId`
    // 恰好同值（都是 `001005`），所以两处都由 `TO_SOURCE_ID` 供 —— 改一处不会漏另一处。
    // ⚠ 但**别把"同值"当成上游的约束**：PC 客户端 `openAIWebChat` 就是
    // `toSourceId=001248` 而 URL 里写 `targetsourceid=001005`（`docs/capture/apiServer.js` L954/L969）。
    // 这条断言钉的是**抓包这一份实况**，不是一条协议规则。
    assert!(url.ends_with(&format!("&targetSourceId={TO_SOURCE_ID}")));
}
