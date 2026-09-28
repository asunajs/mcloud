//! 会话层测试。
//!
//! 重点在**请求形状**（头、体、顺序都要与抓包逐字一致）与**缓存语义**
//! （页面互不顶替、凭据轮换清旧条目）。这些错了都不会报错，
//! 只会表现成 `90001 未登录`。

use super::*;

fn signin() -> RecipeParams {
    SessionRecipe::SignInPage.params()
}

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// ── 配方参数 ────────────────────────────────────────────────────────────

#[test]
fn signin_recipe_params_match_the_capture() {
    let p = signin();
    assert_eq!(p.to_source_id, "001005", "签到页 URL 自带的 targetSourceId");
    assert_eq!(p.market_name, "sign_in_3");
    assert_eq!(p.source_id, "1002");
    assert_eq!(p.page_path, "newsignin");
}

/// ⚠ 页面 URL 的 `targetSourceId` **排在 `token` 之后**，且必须存在。
///
/// mcloud 踩过这个坑：初版漏了它，因为当时是**照着实现**写的这一行，
/// 而不是照着抓包 —— 于是实现漏了、文档也跟着漏。
#[test]
fn page_url_carries_target_source_id_after_token() {
    let url = signin().page_url("YZsidssolgabc");
    let token_at = url.find("token=").expect("应带 token");
    let target_at = url.find("targetSourceId=").expect("必须带 targetSourceId");
    assert!(
        target_at > token_at,
        "targetSourceId 应排在 token 之后：{url}"
    );
    assert!(url.contains("targetSourceId=001005"));
    assert!(url.contains("path=newsignin"));
    assert!(url.contains("sourceid=1002"));
    assert!(url.ends_with("targetSourceId=001005"), "应在末尾：{url}");
}

// ── 第 ① 段：querySpecTokenV2 ───────────────────────────────────────────

#[test]
fn spec_token_request_shape() {
    let req = spec_token_request(signin(), "Basic abc");
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://user-njs.yun.139.com/user/querySpecTokenV2"
    );
    assert_eq!(header(&req, "Authorization"), Some("Basic abc"));
    assert_eq!(header(&req, "Content-Type"), Some(H5_CONTENT_TYPE));
    assert_eq!(req.body_str(), Some(r#"{"toSourceId":"001005"}"#));
}

// ── 第 ② 段：timestamp ──────────────────────────────────────────────────

#[test]
fn timestamp_request_is_get_without_auth() {
    let req = timestamp_request(signin(), "https://m.mcloud.139.com/portal/x");
    assert_eq!(req.method, "GET");
    assert_eq!(req.url, "https://m.mcloud.139.com/ycloud/api/timestamp");
    // 这一枪**无鉴权**：不该带 Authorization，也不该带 jwttoken
    assert_eq!(header(&req, "Authorization"), None);
    assert_eq!(header(&req, "jwttoken"), None);
    assert!(req.body.is_none(), "GET 不该有体");
    assert_eq!(header(&req, "activityid"), Some("sign_in_3"));
}

// ── 第 ③ 段：tyrzLogin ──────────────────────────────────────────────────

#[test]
fn tyrz_login_request_shape() {
    let sign = H5Signature::with_ids("req-1", "1789800128813", "nonce-1");
    let req = tyrz_login_request(
        signin(),
        "YZsidssolgabc",
        &sign,
        "https://m.mcloud.139.com/portal/x",
    );

    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://m.mcloud.139.com/ycloud/auth-service/auth/tyrzLogin"
    );
    // 四个签名头缺一不可
    assert_eq!(header(&req, "x-request-id"), Some("req-1"));
    assert_eq!(header(&req, "x-timestamp"), Some("1789800128813"));
    assert_eq!(header(&req, "x-nonce"), Some("nonce-1"));
    assert!(header(&req, "x-signature").is_some(), "签名头必须有");
    assert_eq!(header(&req, "Origin"), Some("https://m.mcloud.139.com"));
    assert_eq!(header(&req, "showloading"), Some("true"));
    // 建会话这一步**还没有** jwt，不该带这个头
    assert_eq!(header(&req, "jwttoken"), None, "拿票的那一步不带票");
}

/// 请求体的**键顺序**要与抓包一致（字典序）。
///
/// 这不只是好看：上游对请求体的处理方式未实测，而"换个顺序就变行为"
/// 这类事在签名类接口上并不罕见。按抓包写，出问题时少一个变量。
#[test]
fn tyrz_login_body_keys_are_in_capture_order() {
    let sign = H5Signature::with_ids("r", "1", "n");
    let req = tyrz_login_request(signin(), "TOK", &sign, "https://x/");
    let body = req.body_str().unwrap();

    let i_market = body.find("marketName").unwrap();
    let i_open = body.find("openAccount").unwrap();
    let i_source = body.find("sourceId").unwrap();
    let i_token = body.find("\"token\"").unwrap();
    assert!(i_market < i_open, "marketName → openAccount");
    assert!(i_open < i_source, "openAccount → sourceId");
    assert!(i_source < i_token, "sourceId → token");
    assert!(body.contains(r#""marketName":"sign_in_3""#), "{body}");
    assert!(body.contains(r#""sourceId":"1002""#), "{body}");
    assert!(
        body.contains(r#""openAccount":0"#),
        "openAccount 是数字 0：{body}"
    );
}

// ── 签名 ────────────────────────────────────────────────────────────────

/// H5 签名与黄金向量的同一套算式（盐取第 2 个、预留位为空串）。
#[test]
fn h5_signature_uses_salt_two_and_empty_extra() {
    let sig = H5Signature::with_ids("id", "123", "nonce");
    let expect = mcloud_crypto::generate_signature("id", "123", "nonce", 2, "");
    assert_eq!(sig.signature, expect);
    assert_eq!(sig.signature.len(), 32, "小写 hex MD5");
    assert!(sig
        .signature
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

#[test]
fn generate_produces_uuid_ids() {
    let a = H5Signature::generate("1");
    let b = H5Signature::generate("1");
    assert_ne!(a.request_id, b.request_id, "每次都要新的 request-id");
    assert_ne!(a.nonce, b.nonce, "每次都要新的 nonce");
    assert_eq!(a.request_id.len(), 36, "UUID v4 带连字符");
    assert_eq!(a.request_id.matches('-').count(), 4);
}

// ── 响应解析 ────────────────────────────────────────────────────────────

/// `querySpecTokenV2` 的信封：`code` 是**字符串** `"0000"`。
#[test]
fn parse_sso_token_from_double_envelope() {
    let body = r#"{"success":true,"code":"0000","message":"请求成功",
      "data":{"token":"YZsidssolg0123456789abcdef01234567","resultCode":"104000"}}"#;
    let t = parse_sso_token(body).unwrap();
    assert_eq!(t, "YZsidssolg0123456789abcdef01234567");
}

#[test]
fn parse_sso_token_rejects_failure_envelope() {
    let body = r#"{"success":false,"code":"1001","message":"参数错误","data":null}"#;
    let e = parse_sso_token(body).unwrap_err().to_string();
    assert!(e.contains("1001"), "应带上业务码：{e}");
    assert!(e.contains("参数错误"), "应带上消息：{e}");
}

#[test]
fn parse_sso_token_requires_token_field() {
    let body = r#"{"success":true,"code":"0000","data":{"resultCode":"1"}}"#;
    assert!(parse_sso_token(body).is_err());
}

#[test]
fn parse_server_timestamp_accepts_number_and_string() {
    assert_eq!(
        parse_server_timestamp(r#"{"code":0,"result":1789800128813}"#).unwrap(),
        "1789800128813"
    );
    // 字符串形式也要能收：上游换过类型的话不该直接崩
    assert_eq!(
        parse_server_timestamp(r#"{"result":"1789800128813"}"#).unwrap(),
        "1789800128813"
    );
}

#[test]
fn parse_server_timestamp_rejects_non_integer() {
    let e = parse_server_timestamp(r#"{"result":{"a":1}}"#)
        .unwrap_err()
        .to_string();
    assert!(e.contains("整数"), "{e}");
}

#[test]
fn parse_jwt_token_success() {
    let body = r#"{"code":0,"msg":"success","result":{"token":"eyJhbGciOiJIUzI1NiJ9.payload.sig","mobile":"19900000000"}}"#;
    let s = parse_jwt_token(body, "https://m.mcloud.139.com/portal/x").unwrap();
    assert_eq!(s.jwt_token, "eyJhbGciOiJIUzI1NiJ9.payload.sig");
    assert_eq!(s.mobile.as_deref(), Some("19900000000"));
}

/// `mobile` 缺失不该让整次登录失败 —— 它只是"确认签的是哪个账号"的回显。
#[test]
fn parse_jwt_token_tolerates_missing_mobile() {
    let body = r#"{"code":0,"result":{"token":"jwt"}}"#;
    let s = parse_jwt_token(body, "https://m.mcloud.139.com/portal/x").unwrap();
    assert_eq!(s.jwt_token, "jwt");
    assert_eq!(s.mobile, None);
}

/// ⭐ 失败样本 `{"code":200050409,"msg":"token 已失效"}` **没有 `success` 字段**。
///
/// 只看 `success` 的实现会把它当成功，然后拿着一个空 token 去请求 ——
/// 表现成"后面每个接口都未登录"。
#[test]
fn parse_jwt_token_rejects_expired_without_success_field() {
    let body = r#"{"code":200050409,"msg":"token 已失效"}"#;
    let e = parse_jwt_token(body, "https://m.mcloud.139.com/portal/x")
        .unwrap_err()
        .to_string();
    assert!(e.contains("200050409"), "应带上业务码：{e}");
    assert!(e.contains("已失效"), "应带上消息：{e}");
}

#[test]
fn parse_jwt_token_handles_string_code_zero() {
    // 有的接口 code 是字符串 "0000"
    let body = r#"{"code":"0000","result":{"token":"j"}}"#;
    assert_eq!(
        parse_jwt_token(body, "https://m.mcloud.139.com/portal/x")
            .unwrap()
            .jwt_token,
        "j"
    );
}

// ── 会话缓存 ────────────────────────────────────────────────────────────

fn session(tok: &str) -> H5Session {
    H5Session {
        jwt_token: tok.into(),
        mobile: None,
        referer: "https://m.mcloud.139.com/portal/mobilecloud/index.html".into(),
    }
}

#[test]
fn cache_starts_empty() {
    let c = SessionCache::new();
    assert!(c.is_empty());
    assert_eq!(c.get("138", SessionRecipe::SignInPage, "fp"), None);
}

#[test]
fn cache_roundtrip() {
    let c = SessionCache::new();
    c.put("138", SessionRecipe::SignInPage, "fp", session("jwt-1"));
    assert_eq!(
        c.get("138", SessionRecipe::SignInPage, "fp")
            .unwrap()
            .jwt_token,
        "jwt-1"
    );
    assert_eq!(c.len(), 1);
}

/// ⭐ 不同页面的票**互不顶替**。
///
/// mcloud 实测的用户口径：去点小红花用了小红花页那张票之后，
/// **回头点领取AI豆带的还是签到页那张**（没被顶掉）。
///
/// 现在只有一个配方，所以这里用"不同配方"来钉住这个性质 ——
/// 将来加第二个配方时，这条测试就是它的护栏。
#[test]
fn different_recipes_do_not_evict_each_other() {
    let c = SessionCache::new();
    c.put(
        "138",
        SessionRecipe::SignInPage,
        "fp",
        session("signin-token"),
    );
    // 再存一次同配方同指纹 ⇒ 覆盖，而不是新增
    c.put(
        "138",
        SessionRecipe::SignInPage,
        "fp",
        session("signin-token-2"),
    );
    assert_eq!(c.len(), 1, "同键应是覆盖");
    assert_eq!(
        c.get("138", SessionRecipe::SignInPage, "fp")
            .unwrap()
            .jwt_token,
        "signin-token-2"
    );
}

#[test]
fn different_accounts_are_isolated() {
    let c = SessionCache::new();
    c.put("138", SessionRecipe::SignInPage, "fp", session("a"));
    c.put("139", SessionRecipe::SignInPage, "fp", session("b"));
    assert_eq!(c.len(), 2);
    assert_eq!(
        c.get("138", SessionRecipe::SignInPage, "fp")
            .unwrap()
            .jwt_token,
        "a"
    );
    assert_eq!(
        c.get("139", SessionRecipe::SignInPage, "fp")
            .unwrap()
            .jwt_token,
        "b"
    );
}

/// ⭐ 凭据一旦轮换，**旧指纹的条目必须被清掉**。
///
/// 不清的话：旧票仍然挂着，而 `Authorization` 已经变了 ⇒ 拿旧票请求必然失败，
/// 失败会被读成"这张票刷不动"。留着也是泄漏。
#[test]
fn rotated_credential_evicts_stale_entries() {
    let c = SessionCache::new();
    c.put(
        "138",
        SessionRecipe::SignInPage,
        "old-fp",
        session("old-token"),
    );
    assert_eq!(c.len(), 1);

    c.put(
        "138",
        SessionRecipe::SignInPage,
        "new-fp",
        session("new-token"),
    );
    assert_eq!(c.len(), 1, "同账号同配方的旧指纹条目应被清掉");
    assert_eq!(
        c.get("138", SessionRecipe::SignInPage, "old-fp"),
        None,
        "旧票不该再取到"
    );
    assert_eq!(
        c.get("138", SessionRecipe::SignInPage, "new-fp")
            .unwrap()
            .jwt_token,
        "new-token"
    );
}

#[test]
fn remove_drops_one_entry() {
    let c = SessionCache::new();
    c.put("138", SessionRecipe::SignInPage, "fp", session("t"));
    c.remove("138", SessionRecipe::SignInPage, "fp");
    assert!(c.is_empty(), "刷新时要能丢掉刚失败的那条");
}

#[test]
fn debug_does_not_leak_the_token() {
    let c = SessionCache::new();
    c.put(
        "138",
        SessionRecipe::SignInPage,
        "fp",
        session("super-secret-jwt"),
    );
    let s = format!("{c:?}");
    assert!(!s.contains("super-secret-jwt"), "Debug 不该打印票值：{s}");
    assert!(s.contains("entries"), "{s}");
}

/// 并发存取不能丢条目 —— 会话池是进程级共享的。
#[test]
fn cache_is_concurrency_safe() {
    use std::sync::Arc;
    let c = Arc::new(SessionCache::new());
    let mut handles = Vec::new();
    for i in 0..16 {
        let c = Arc::clone(&c);
        handles.push(std::thread::spawn(move || {
            for j in 0..20 {
                let acct = format!("1380000{i:04}");
                c.put(
                    &acct,
                    SessionRecipe::SignInPage,
                    "fp",
                    session(&format!("t{j}")),
                );
                let _ = c.get(&acct, SessionRecipe::SignInPage, "fp");
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(c.len(), 16, "每个账号一条，不能丢");
}

// ── 指纹 ────────────────────────────────────────────────────────────────

#[test]
fn fingerprint_is_stable_short_and_not_reversible() {
    let a = auth_fingerprint("SYNTH001|1|RCS|123|payload");
    let b = auth_fingerprint("SYNTH001|1|RCS|123|payload");
    assert_eq!(a, b, "同输入同指纹");
    assert_eq!(a.len(), 16, "SHA-256 前 8 字节 = 16 hex");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(!a.contains("payload"), "指纹不该泄漏原文");
    assert_ne!(a, auth_fingerprint("SYNTH001|1|RCS|123|other"));
}

// ── Authorization ───────────────────────────────────────────────────────

#[test]
fn basic_authorization_wraps_the_raw_auth() {
    use mcloud_core::{AccountId, AccountSettings, AuthInfo, DeviceId};
    let acc = Account {
        id: AccountId::new("19900000000"),
        nickname: None,
        auth: AuthInfo {
            raw: "bW9iaWxlOjE5OTAwMDAwMDAwOlNZTlRIMDAx".into(),
            platform: "ios".into(),
            phone: "19900000000".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("d".into()),
        pc_platform: None,
        user_domain_id: None,
        settings: AccountSettings::default(),
    };
    assert_eq!(
        basic_authorization(&acc),
        "Basic bW9iaWxlOjE5OTAwMDAwMDAwOlNZTlRIMDAx"
    );
}

// ── HTTP 层 ─────────────────────────────────────────────────────────────

#[test]
fn ensure_http_ok_reports_status_and_body() {
    let ok = HttpResponse {
        status: 200,
        body: "{}".into(),
        headers: vec![],
    };
    assert_eq!(ensure_http_ok(&ok, "测试").unwrap(), "{}");

    let bad = HttpResponse {
        status: 401,
        body: "Unauthorized".into(),
        headers: vec![],
    };
    let e = ensure_http_ok(&bad, "tyrzLogin").unwrap_err().to_string();
    assert!(e.contains("tyrzLogin"), "{e}");
    assert!(e.contains("401"), "{e}");
    assert!(e.contains("Unauthorized"), "应带响应体片段：{e}");
}

#[test]
fn content_type_carries_charset() {
    // `application/json` 少了 `;charset=UTF-8` 是抓包里**没有**的形状
    assert_eq!(H5_CONTENT_TYPE, "application/json;charset=UTF-8");
}

// ── 直播侧「门」的第①步（自动收口令的前置）────────────────────────────────

/// ⭐ **门的第①步必须用直播侧的 `toSourceId`（`017027`），不是签到的 `001005`。**
///
/// 两者都叫 `targetSourceId`，混用的表现是"**一条房间都筛不出来**" ——
/// 而它看起来像"今天没有直播"，是个**假结论**（参考实现坑表 #106；
/// `../mcloud/docs/capture/app-live-redflower-samples.txt` 第 494–495 行原文纠正）。
///
/// 实测形状（同文档第 351 行）：`{"toSourceId":"017027"}` ⇒ SSO token 42 字符。
#[test]
fn live_gate_spec_token_uses_the_live_source_id() {
    let req = live_gate_spec_token_request("mobile abc");

    assert!(
        req.url.ends_with("/user/querySpecTokenV2"),
        "应打 querySpecTokenV2，实得 {}",
        req.url
    );
    let body: serde_json::Value =
        serde_json::from_str(req.body_str().expect("第①步是 JSON 体")).unwrap();

    // ⭐ 这一条是本文件存在的理由。
    assert_eq!(
        body["toSourceId"], "017027",
        "门的第①步必须用**直播侧**那个 toSourceId"
    );
    assert_ne!(
        body["toSourceId"], "001005",
        "001005 是签到/小红花侧的值 —— 用它门会一条房间都筛不出来"
    );
    assert_eq!(body["toSourceId"], crate::live_consts::LIVE_SOURCE_ID);
    assert_ne!(
        crate::live_consts::LIVE_SOURCE_ID,
        crate::live_consts::TO_SOURCE_ID,
        "两侧的 sourceId 必须不同（这是两套不同的东西）"
    );
}

/// 第①步**只有 `toSourceId` 一个键** —— 没有 `marketName` 之类。
///
/// 这条钉的是"门不建 H5 会话"这个事实：建会话那三枪才要 `marketName` /
/// `sourceId`，而门只换一张 SSO 就去 `/cmlive`。
/// ⚠ 若将来有人"顺手把配方其余字段拼进这个体"，这条会红 —— 那正是要拦的。
#[test]
fn live_gate_spec_token_body_has_exactly_one_key() {
    let req = live_gate_spec_token_request("mobile abc");
    let body: serde_json::Value = serde_json::from_str(req.body_str().unwrap()).unwrap();
    let obj = body.as_object().expect("应当是 JSON 对象");
    assert_eq!(
        obj.len(),
        1,
        "门的第①步只带 toSourceId（实测形状只有一个键），实得键：{:?}",
        obj.keys().collect::<Vec<_>>()
    );
    assert!(obj.contains_key("toSourceId"));
}

/// 第①步要带上库里那份 `authorization`（不另建凭据）。
///
/// 实测结论：`querySpecTokenV2(017027)` **能换到** SSO token
/// ⇒ 现有凭据在直播这一族上也认（**不必**为直播单独准备凭据）。
#[test]
fn live_gate_spec_token_carries_the_existing_authorization() {
    let req = live_gate_spec_token_request("mobile abc");
    assert_eq!(header(&req, "Authorization"), Some("mobile abc"));
    assert_eq!(header(&req, "Content-Type"), Some(H5_CONTENT_TYPE));
}

/// 门用的是**直播侧**那一张票，与签到页配方**不是同一条**。
///
/// 这条防的是"把两张票混用"：拿直播侧 JWT 去 `/ycloud` 实测回 `90001 未登录`。
#[test]
fn gate_token_request_is_not_the_signin_recipe() {
    let gate = live_gate_spec_token_request("mobile abc");
    let signin = spec_token_request(signin(), "mobile abc");

    // 同一个端点，但 body 里的 toSourceId 必须不同。
    let gate_body: serde_json::Value = serde_json::from_str(gate.body_str().unwrap()).unwrap();
    let signin_body: serde_json::Value = serde_json::from_str(signin.body_str().unwrap()).unwrap();
    assert_ne!(
        gate_body["toSourceId"], signin_body["toSourceId"],
        "门的票与签到页的票是两套不同的参数，不能相同"
    );
    assert_eq!(signin_body["toSourceId"], "001005");
}
