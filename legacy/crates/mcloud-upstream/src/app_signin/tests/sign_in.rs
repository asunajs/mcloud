// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;
use serde_json::Value;

// ───────────────────────── 签到主体 ─────────────────────────

#[test]
fn info_v3_request_needs_jwt_and_uses_client_app() {
    let request = info_v3_request("JWT", "https://referer/");
    assert_eq!(request.method, mcloud_core::HttpMethod::Get);
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/page/infoV3?client=app"
    );
    assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    assert_eq!(header(&request, "Referer"), Some("https://referer/"));
    assert_eq!(header(&request, "activityid"), Some(MARKET_NAME));
    assert_eq!(header(&request, "appversion"), Some(H5_APP_VERSION));
}

#[test]
fn start_sign_in_without_device_id_is_just_client_app() {
    // 实测形态：不带 deviceId 也能签上（账号 B 验证过），
    // 而且此时**不发** `isdeviceid` 头 —— 发一个空的等于自相矛盾。
    let request = start_sign_in_request("JWT", "r", None);
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app"
    );
    assert_eq!(header(&request, "isdeviceid"), None);
    assert_eq!(header(&request, "showloading"), Some("false"));
}

#[test]
fn start_sign_in_with_device_id_percent_encodes_base64() {
    // `deviceId` 是 base64，含 `+` `/` `=`。`+` 在查询串里表示空格 ——
    // 不编码就会被服务端解成另一个值，而且**不报错**（只是行为不对）。
    let raw =
        "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0A=";
    let request = start_sign_in_request("JWT", "r", Some(raw));
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/page/startSignIn?client=app\
         &deviceId=AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0%2BP0A%3D"
    );
    assert_eq!(header(&request, "isdeviceid"), Some("true"));
    assert!(!request.url.contains('+'), "`+` 必须被编码成 %2B");
}

#[test]
fn percent_encode_leaves_unreserved_alone() {
    assert_eq!(percent_encode("aZ0-._~"), "aZ0-._~");
    assert_eq!(percent_encode("+/="), "%2B%2F%3D");
    assert_eq!(percent_encode("A B"), "A%20B");
}

#[test]
fn task_list_v3_body_matches_capture() {
    let request = task_list_v3_request("JWT", "r", 1).unwrap();
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/task/taskListV3"
    );
    assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    assert_eq!(
        header(&request, "Content-Type"),
        Some("application/json;charset=UTF-8")
    );
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "marketname": "sign_in_3",
            "client": 1,
            "clientVersion": "12.6.0",
        })
    );
    // `client` 是**数字**（与 startSignIn 的 `client=app` 字符串不是一回事）
    assert!(body["client"].is_i64());
}

#[test]
fn do_task_post_requires_a_device_id() {
    let request = do_task_post_request("JWT", "r", "DEV").unwrap();
    let body: Value = serde_json::from_slice(request.body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"client": "app", "deviceId": "DEV"})
    );
    assert_eq!(header(&request, "isdeviceid"), Some("true"));
    // 这里 `device_id` 是 `&str`（必填）—— 漏发是编译错误，不是运行时报错
}

/// `infoV3` 的两个翻倍门槛字段 —— 只认**真布尔**，缺了给 `None` 而不是 `false`。
#[test]
fn parse_sign_in_status_reads_the_two_backup_gates() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": 1, "signInPoints": 3, "signCount": 1, "monthDays": 0, "cal": [],
            "curMonthBackup": true, "curMonthBackupSignAccept": false
        }
    }));
    let status = parse_sign_in_status(&envelope).unwrap();
    assert_eq!(status.cur_month_backup, Some(true));
    assert_eq!(status.cur_month_backup_sign_accept, Some(false));

    // 缺字段 ⇒ `None`（"没说"），**不是** `Some(false)`（"没开"）。
    // 这个区分是 `plan_multiple` 的安全前提：把"没说"当"没开"会漏领，
    // 当"开了"会白发一个写请求。
    let missing = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": 1, "signInPoints": 3, "signCount": 1, "monthDays": 0, "cal": []
        }
    }));
    let status = parse_sign_in_status(&missing).unwrap();
    assert_eq!(status.cur_month_backup, None);
    assert_eq!(status.cur_month_backup_sign_accept, None);
}
