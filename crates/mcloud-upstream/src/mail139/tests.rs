//! `mail139` 协议层的测试。
//!
//! 最要紧的三条：**四台主机的凭证是 Cookie 而不是云盘那套头**、
//! **PE 上报的鉴权算法逐字对上旧实现**、**单引号 JSON 那一层解析照抄**。

use super::*;

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn session() -> MailSession {
    MailSession {
        sid: "00c5SID".into(),
        rmkey: "rm-key".into(),
    }
}

/// 登录那一枪：URL、方法、XML 体逐字。
#[test]
fn mail_login_is_the_legacy_xml_shape() {
    let req = mail_login_request("SSO<&>");
    assert_eq!(req.method, "POST");
    assert_eq!(req.url, MAIL_LOGIN_URL);
    assert_eq!(
        header(&req, "content-type"),
        Some("application/xml;charset=UTF-8")
    );
    let body = req.body_str().expect("登录体是文本");
    assert!(
        body.contains("<string name=\"clientId\">10804</string>"),
        "{body}"
    );
    assert!(
        body.contains("<string name=\"loginType\">7</string>"),
        "{body}"
    );
    // token 必须被 XML 转义（旧实现 escapeXml 逐字）
    assert!(
        body.contains("<string name=\"token\">SSO&lt;&amp;&gt;</string>"),
        "{body}"
    );
}

/// 登录响应解析：`var.sid` / `var.rmkey`（单引号 JSON 也要认）。
#[test]
fn parse_mail_session_accepts_single_quoted_json() {
    let s = parse_mail_session("{'var':{'sid':'abc','rmkey':'def'}}").unwrap();
    assert_eq!(s.sid, "abc");
    assert_eq!(s.rmkey, "def");
    assert_eq!(s.cookie(), "RMKEY=def; Os_SSo_Sid=abc");
}

/// 缺 sid / rmkey ⇒ 报错，不返回半个 session。
/// ⭐⭐ 真机回归（2026-10-07，用户截图）：**合法 JSON 里带单引号**不能被兜底改坏 ✗。
///
/// 原文形状取自 `测试` 号那次真实登录响应（只保留出错那一段 + 前后的括号 ✓）：
/// `var` 里那格 `val2` 的值**本身**就是一段带单引号的串。
/// 旧实现先做全局 `'` → `"` 替换 ⇒ 把串里那些也换掉 ⇒ 一份**完全合法**的响应
/// 反而解析失败 ✗（报 `expected ',' or '}' at line 52 column 16`）。
///
/// 红测：把 `loose_json` 里"先严格解"那一步摘掉 ⇒ 这条红 ✓。
#[test]
fn parse_mail_session_keeps_a_valid_json_body_with_inner_single_quotes() {
    let body = r#"{
        "code": "S_OK",
        "var": {
            "uin": "4024016622",
            "list": [
                {"id":"47","val1":"0","val2":"{'billTime':'','cloudPostTime':'2024-04-09'}","type":"0"}
            ],
            "sid": "00c5MTM0ODU0M0AWM",
            "rmkey": "2f0a1b"
        }
    }"#;
    let s = parse_mail_session(body).expect("合法 JSON 就该解得出来");
    assert_eq!(s.sid, "00c5MTM0ODU0M0AWM");
    assert_eq!(s.rmkey, "2f0a1b");
}

/// 兜底那一档**还在**：上游真发单引号 JSON 时照样能解 ✓（顺序改了，能力没丢）。
#[test]
fn loose_json_still_accepts_single_quoted_bodies() {
    let v = loose_json("{'code':'S_OK','var':{'sid':'a','rmkey':'b'}}").unwrap();
    assert_eq!(
        v.get("code").and_then(serde_json::Value::as_str),
        Some("S_OK")
    );
}

#[test]
fn parse_mail_session_rejects_a_half_session() {
    assert!(parse_mail_session("{\"var\":{\"sid\":\"abc\"}}").is_err());
    assert!(parse_mail_session("{\"code\":\"S_OK\"}").is_err());
}

/// 市场打卡：URL + JSON 体三格逐字（⚠ 这一口今天 404，形状照旧要对）。
#[test]
fn market_task_click_v2_body_is_the_legacy_shape() {
    let req = market_task_click_v2_request(472, "sign_in_3");
    assert_eq!(req.method, "POST");
    assert_eq!(req.url, MARKET_TASK_CLICK_V2_URL);
    let body = req.body_str().unwrap();
    assert!(body.contains("\"marketname\":\"sign_in_3\""), "{body}");
    assert!(body.contains("\"taskId\":472"), "{body}");
    assert!(body.contains("\"clientVersion\":\"11.2.1\""), "{body}");
}

/// composeId 的两种形状都要认（字符串 / 对象取 m 或 id）。
#[test]
fn parse_compose_id_accepts_both_shapes() {
    assert_eq!(
        parse_compose_id("{'code':'S_OK','var':'CID'}").unwrap(),
        Some("CID".to_string())
    );
    assert_eq!(
        parse_compose_id("{'code':'S_OK','var':{'m':'CID2'}}").unwrap(),
        Some("CID2".to_string())
    );
    assert_eq!(
        parse_compose_id("{'code':'S_OK','var':{'id':'CID3'}}").unwrap(),
        Some("CID3".to_string())
    );
    assert_eq!(parse_compose_id("{'code':'S_OK','var':''}").unwrap(), None);
    assert_eq!(parse_compose_id("{'code':'S_OK'}").unwrap(), None);
}

/// 发信那一枪：URL 带 `peFlag=1`、Cookie 只带 RMKEY、XML 体逐字。
#[test]
fn compose_mail_matches_the_legacy_xml() {
    let mail = ComposeMail {
        compose_id: "CID".into(),
        account: "13900000000@139.com".into(),
        to: "13900000000@139.com".into(),
        subject: "s".into(),
        content: "c".into(),
        is_html: 0,
        action: "asyn".into(),
    };
    let req = compose_mail_request(&session(), &mail);
    assert_eq!(
        req.url,
        "https://appmail.mail.10086.cn/RmWeb/mail?func=mbox:compose&peFlag=1&sid=00c5SID"
    );
    assert_eq!(header(&req, "Cookie"), Some("RMKEY=rm-key"));
    assert_eq!(
        header(&req, "Content-Type"),
        Some("text/xml; charset=UTF-8")
    );
    let body = req.body_str().unwrap();
    assert!(body.contains("<string name='id'>CID</string>"), "{body}");
    assert!(body.contains("<int name='isHtml'>0</int>"), "{body}");
    assert!(
        body.contains("<string name='action'>asyn</string>"),
        "{body}"
    );
    // 旧实现把 subject/content 走 xmlEscape（单引号也转义）
    assert!(body.contains("<string name='subject'>s</string>"), "{body}");
}

/// `code === "S_OK"` 是唯一成功判据（旧 TS 逐字）。
#[test]
fn mail_code_success_is_exactly_s_ok() {
    assert!(parse_mail_code("{'code':'S_OK'}").unwrap().is_ok());
    assert!(!parse_mail_code("{'code':'S_FAIL','summary':'nope'}")
        .unwrap()
        .is_ok());
    assert!(parse_mail_code("{}").is_err());
}

/// ⭐ PE 上报的鉴权头：`Basic ` + MD5(ts + PE_SECRET)，ts 是 14 位。
///
/// 这一条是**纯算法**，所以可以直接钉死；`gzip` 那一半只断言
/// `Content-Encoding` 与体非空（体是 gzip，不当文本读）。
#[test]
fn pe_auth_header_is_md5_of_timestamp_plus_secret() {
    let clock = PeClock::from_local(2026, 10, 7, 10, 45, 30);
    assert_eq!(clock.stamp(), "20261007104530");
    assert_eq!(clock.event_time, "2026-10-07 10:45:30");
    let ev = PeActiveEvent {
        event_time: clock.event_time.clone(),
        event_type: "3".into(),
    };
    let req = report_active_action_request(&session(), &[ev], "deadbeef", &clock).unwrap();
    let expect = format!("Basic {}", md5_hex(&format!("20261007104530{PE_SECRET}")));
    assert_eq!(header(&req, "Authorization"), Some(expect.as_str()));
    assert_eq!(header(&req, "Content-Encoding"), Some("gzip"));
    assert_eq!(header(&req, "Cookie"), Some("RMKEY=rm-key"));
    assert!(req.url.contains("func=user:activeAction"));
    assert!(req.url.contains("cguid=deadbeef"));
    assert!(req.has_body(), "PE 上报必须带体");
}

/// PDF 转换：查询串六格逐字、体是 XML。
#[test]
fn convert_pdf_query_and_body_are_the_legacy_shape() {
    let p = ConvertPdf {
        file_name: "asign-pdf-1.pdf".into(),
        file_real_size: 500,
        file_id: "FID".into(),
        convert_type: 1,
    };
    let req = convert_pdf_request(&session(), &p, "0.5", "0abc");
    assert_eq!(
        req.url,
        "https://html5.mail.10086.cn/mw2/file/disk?func=disk:convertPdfFromYunFile&sid=00c5SID\
         &behaviorData=&rnd=0.5&cguid=0abc&k=&comefrom=166"
    );
    assert_eq!(
        header(&req, "Cookie"),
        Some("RMKEY=rm-key; Os_SSo_Sid=00c5SID")
    );
    assert_eq!(header(&req, "X-Requested-With"), Some("cn.cj.pe"));
    let body = req.body_str().unwrap();
    assert!(
        body.contains("<string name=\"fileId\">FID</string>"),
        "{body}"
    );
    assert!(
        body.contains("<int name=\"fileRealSize\">500</int>"),
        "{body}"
    );
    assert!(
        body.contains("<string name=\"channel\">10242000</string>"),
        "{body}"
    );
}

/// 邮箱备份那一枪（⚠ 今天 404）：URL 带 sid、体是 `{}`。
#[test]
fn mail_backup_is_a_post_with_an_empty_object() {
    let req = mail_backup_request(&session());
    assert_eq!(req.method, "POST");
    assert_eq!(
        req.url,
        "https://mail.10086.cn/mailsms/sms/backup?sid=00c5SID"
    );
    assert_eq!(req.body_str(), Some("{}"));
}

/// `queryUserToken`：URL 与 XML 空对象体逐字，内层三格解析。
#[test]
fn query_user_token_shape_and_parse() {
    let req = query_user_token_request(&session(), "0.25", "0fff");
    assert!(req.url.starts_with(
        "https://html5.mail.10086.cn/mw2/file/disk?func=disk:queryUserToken&sid=00c5SID"
    ));
    assert_eq!(req.body_str(), Some("<object></object>"));

    let body = "{'code':'S_OK','var':{'data':{'data':{'account':'ENC','token':'TOK',\
                'userDomainId':'1234567890123456789'}}}}";
    let t = parse_query_user_token(body).unwrap();
    assert_eq!(t.account, "ENC");
    assert_eq!(t.token, "TOK");
    assert_eq!(t.user_domain_id.as_deref(), Some("1234567890123456789"));

    assert!(parse_query_user_token("{'code':'S_OK'}").is_err());
}
