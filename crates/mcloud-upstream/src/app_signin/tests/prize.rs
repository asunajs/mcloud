// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;

/// 奖品记录是 **GET + 四个查询参数**、没有 body、没有 `isDeviceId`。
#[test]
fn prize_record_is_a_get_with_four_query_params() {
    let request = prize_record_request("JWT", "r", PRIZE_MARKET_NAME, PRIZE_STATUS_PENDING, 1, 10);

    assert_eq!(request.method, mcloud_core::HttpMethod::Get);
    assert!(request.body.is_none(), "GET 没有 body");
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/public/record\
         ?marketname=sign_in_3_ex&type=1&pageNumber=1&pageSize=10"
    );
    // H5 没标 `isDeviceId` ⇒ 拦截器不注入。
    assert_eq!(header(&request, "isdeviceid"), None);
    assert!(!request.url.contains("deviceId"));
    assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    assert_eq!(header(&request, "showloading"), Some("true"));
}

/// `marketname` 里出现保留字符要编码 —— 与 `acceptDate` 同源问题。
#[test]
fn prize_record_percent_encodes_the_market_name() {
    let request = prize_record_request("JWT", "r", "a b&c", 1, 1, 10);
    assert!(
        request.url.contains("marketname=a%20b%26c"),
        "{}",
        request.url
    );
}

/// 条目字段名有**实拍**（2026-09-19 账号 159 的「待领取」页签），
/// 但**没有的字段也不判失败** —— 空页才是最常见的答案。
#[test]
fn parse_prize_records_reads_the_h5_field_names() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"records": [{
            "prizeName": "移动云盘1000万tokens叠加包",
            "insertTime": "2026-09-16T11:12:54.000+00:00",
            "expireTime": "2026-09-30T15:59:59.000+00:00",
            "flag": 1,
            "prizeId": 260630014,
            "oid": 1270932714,
            "marketName": "sign_in_3_ex",
            "type": 1057,
            "extraWeDoNotKnow": {"whatever": true}
        }]}
    }));
    let page = parse_prize_records(&envelope).unwrap();
    assert_eq!(page.records.len(), 1);
    let item = &page.records[0];
    assert_eq!(
        item.prize_name.as_deref(),
        Some("移动云盘1000万tokens叠加包")
    );
    assert_eq!(
        item.insert_time.as_deref(),
        Some("2026-09-16T11:12:54.000+00:00")
    );
    // ⚠ 过期时间**在响应里**（虽然 H5 的列表项模板不渲染它）。
    assert_eq!(
        item.expire_time.as_deref(),
        Some("2026-09-30T15:59:59.000+00:00")
    );
    assert_eq!(item.flag, Some(1), "旧项目就是按 flag===1 过滤的");
    assert_eq!(item.prize_id, Some(260630014));
    assert_eq!(item.oid.as_deref(), Some("1270932714"), "实拍里 oid 是数字");
    assert_eq!(item.market_name.as_deref(), Some("sign_in_3_ex"));
    assert_eq!(item.prize_type, Some(1057));
    // 认不出来的字段原样留着，不丢。
    assert_eq!(
        item.raw["extraWeDoNotKnow"]["whatever"],
        serde_json::json!(true)
    );
}

/// 黄金向量里那条**实拍**奖品记录，逐字段对一遍。
///
/// 上面那条用的是手写夹具；这一条用的是**线上真回来的**那一份 ——
/// 手写夹具只能证明"按我们以为的字段名能解析出来"，证明不了
/// "线上就是这个字段名"。两者都要有。
#[test]
fn golden_prize_record_parses_into_the_same_shape() {
    let envelope = Envelope::new(vectors()["prizeRecords"]["raw"].clone());
    let page = parse_prize_records(&envelope).expect("实拍那条必须能解析");
    assert_eq!(page.records.len(), 1);
    let item = &page.records[0];
    assert_eq!(
        item.prize_name.as_deref(),
        Some("移动云盘1000万tokens叠加包")
    );
    assert_eq!(
        item.insert_time.as_deref(),
        Some("2026-09-16T11:12:54.000+00:00")
    );
    assert_eq!(
        item.expire_time.as_deref(),
        Some("2026-09-30T15:59:59.000+00:00")
    );
    assert_eq!(item.flag, Some(1));
    assert_eq!(item.prize_id, Some(260630014));
    assert_eq!(item.oid.as_deref(), Some("1270932714"));
    assert_eq!(item.market_name.as_deref(), Some("sign_in_3_ex"));
    assert_eq!(item.prize_type, Some(1057));

    // ⚠ 脱敏口径：向量里**必须没有** `userDomainId`。
    // 这条不是形式主义 —— 线上那条响应里真的有它（19 位账号标识），
    // 摘录时删掉是**动作**，会有人漏。守卫放在这里，漏了立刻红。
    assert!(
        item.raw.get("userDomainId").is_none(),
        "黄金向量里不许留账号标识字段"
    );
}

/// 三个页签里总有两个是空的 ⇒ **没有 `records` 是正常答案**，不是错误。
#[test]
fn parse_prize_records_treats_an_empty_page_as_normal() {
    for payload in [
        serde_json::json!({"code": 0, "msg": "success", "result": {"records": []}}),
        serde_json::json!({"code": 0, "msg": "success", "result": {}}),
        serde_json::json!({"code": 0, "msg": "success"}),
    ] {
        let page = parse_prize_records(&Envelope::new(payload)).unwrap();
        assert!(page.records.is_empty());
    }
}

/// `oid` 可能是数字也可能是字符串（H5 直接把它塞进 URL 参数），两种都要收。
#[test]
fn parse_prize_records_accepts_a_numeric_oid() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"records": [{"oid": 20260919}]}
    }));
    let page = parse_prize_records(&envelope).unwrap();
    assert_eq!(page.records[0].oid.as_deref(), Some("20260919"));
}
