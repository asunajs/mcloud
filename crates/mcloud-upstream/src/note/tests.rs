//! `note`（云笔记，玩法 107）协议层的测试。

use super::*;

fn header<'a>(req: &'a HttpRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// 鉴权那一枪：URL、四枚头、体两格逐字。
#[test]
fn note_auth_is_the_legacy_shape() {
    let req = note_auth_request("RAWTOKEN", "13900000000");
    assert_eq!(req.method, "POST");
    assert_eq!(req.url, NOTE_AUTH_URL);
    assert_eq!(header(&req, "APP_CP"), Some("android"));
    assert_eq!(header(&req, "APP_NUMBER"), Some("13900000000"));
    assert_eq!(header(&req, "CP_VERSION"), Some("3.2.0"));
    assert_eq!(header(&req, "x-huawei-channelsrc"), Some("10001400"));
    let body = req.body_str().unwrap();
    assert!(body.contains("\"authToken\":\"RAWTOKEN\""), "{body}");
    assert!(body.contains("\"userPhone\":\"13900000000\""), "{body}");
}

/// `headers.app_auth` 才是通行证；缺它就整条链放弃（旧 TS 那句 `return`）。
#[test]
fn app_auth_gate_is_headers_app_auth() {
    let h = parse_note_headers("{\"headers\":{\"app_auth\":\"A1\",\"other\":\"x\"}}")
        .unwrap()
        .expect("有 headers");
    assert_eq!(note_app_auth(&h), Some("A1"));

    let h = parse_note_headers("{\"headers\":{\"other\":\"x\"}}")
        .unwrap()
        .unwrap();
    assert_eq!(note_app_auth(&h), None);

    let none = parse_note_headers("{\"code\":0}").unwrap();
    assert!(none.is_none());
}

/// 建笔记：URL、`APP_CP: pc`、并把 ① 那份头**整份**并进来。
#[test]
fn create_note_merges_the_auth_headers_and_keeps_the_big_body() {
    let headers = serde_json::json!({"app_auth": "A1", "user_id": "U9"});
    let req = create_note_request("NID", "ttl", "13900000000", &headers, 1_700_000_000_000);
    assert_eq!(
        req.url,
        "https://mnote.caiyun.feixin.10086.cn/noteServer/api/createNote.do"
    );
    assert_eq!(header(&req, "APP_CP"), Some("pc"));
    assert_eq!(header(&req, "CP_VERSION"), Some("7.7.1.20240115"));
    assert_eq!(header(&req, "app_auth"), Some("A1"));
    assert_eq!(header(&req, "user_id"), Some("U9"));

    let body = req.body_str().unwrap();
    assert!(body.contains("\"noteid\":\"NID\""), "{body}");
    assert!(body.contains("\"topmost\":\"0\""), "{body}");
    assert!(body.contains("\"createtime\":\"1700000000000\""), "{body}");
    assert!(body.contains("\"data\":\"<span></span>\""), "{body}");
    assert!(body.contains("\"expands\":{\"noteType\":0}"), "{body}");
}

/// 删笔记：体是 `{noteids:[{noteid}]}`。
#[test]
fn delete_note_body_is_the_legacy_shape() {
    let headers = serde_json::json!({"app_auth": "A1"});
    let req = delete_note_request("NID", &headers);
    assert_eq!(
        req.url,
        "https://mnote.caiyun.feixin.10086.cn/noteServer/api/moveToRecycleBin.do"
    );
    assert_eq!(header(&req, "app_auth"), Some("A1"));
    assert_eq!(req.body_str(), Some("{\"noteids\":[{\"noteid\":\"NID\"}]}"));
}

/// 头上那些**非字符串**的成员被跳过（不是 panic、也不是写个假值进去）。
#[test]
fn non_string_header_members_are_skipped() {
    let headers = serde_json::json!({"app_auth": "A1", "n": 1, "b": true, "o": {"x": 1}});
    let req = create_note_request("NID", "t", "p", &headers, 1);
    assert_eq!(header(&req, "app_auth"), Some("A1"));
    assert!(header(&req, "n").is_none());
    assert!(header(&req, "b").is_none());
    assert!(header(&req, "o").is_none());
}
