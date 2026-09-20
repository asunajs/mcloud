// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;
use mcloud_core::Error;

/// 备份好礼两条都打在 `caiyun.feixin.10086.cn` —— 主机换过，别照抄 `MCLOUD`。
#[test]
fn backup_gift_requests_use_the_caiyun_host() {
    let info = backup_gift_info_request("JWT", "r");
    assert_eq!(info.method, mcloud_core::HttpMethod::Get);
    assert!(info.body.is_none());
    assert_eq!(
        info.url,
        "https://caiyun.feixin.10086.cn/market/backupgift/info"
    );
    assert_eq!(header(&info, "jwttoken"), Some("JWT"));

    let receive = backup_gift_receive_request("JWT", "r");
    assert_eq!(receive.method, mcloud_core::HttpMethod::Get);
    assert_eq!(
        receive.url,
        "https://caiyun.feixin.10086.cn/market/backupgift/receive"
    );
}

/// `state` 是**主判据**（缺了就必须报错），`curMonth` 是前置开关（缺了当 `false`）。
#[test]
fn parse_backup_gift_reads_state_and_cur_month() {
    let ready = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"state": 0, "curMonth": true}
    }));
    let gift = parse_backup_gift(&ready).unwrap();
    assert_eq!(gift.state, 0);
    assert!(gift.cur_month);

    // 旧项目 `if (!curMonth)` —— 数字 0 / 字符串 "0" 都要当假。
    for falsy in [
        serde_json::json!(0),
        serde_json::json!("0"),
        serde_json::json!(false),
    ] {
        let envelope = Envelope::new(serde_json::json!({
            "code": 0, "msg": "success", "result": {"state": -1, "curMonth": falsy}
        }));
        assert!(!parse_backup_gift(&envelope).unwrap().cur_month, "{falsy}");
    }

    // 缺 `state` ⇒ 解析错误：没有它就回答不了"该不该领"。
    let missing = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"curMonth": true}
    }));
    let err = parse_backup_gift(&missing).unwrap_err();
    assert!(matches!(err, Error::Parse(_)), "{err}");
}

/// `backupgift/receive` 的 `result` **是裸数字**，不是对象。
#[test]
fn parse_backup_gift_receive_reads_a_bare_number() {
    let with = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": 30
    }));
    assert_eq!(parse_backup_gift_receive(&with).unwrap().received, Some(30));

    let without = Envelope::new(serde_json::json!({"code": 0, "msg": "success"}));
    assert_eq!(
        parse_backup_gift_receive(&without).unwrap().received,
        None,
        "上游没报数 ≠ 领到 0 豆"
    );
}
