// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;

// ─────────────────── 豆的流水（`cloudRecordV2`） ───────────────────

/// 形状来自 H5 逐字：`get("/ycloud/signin/public/cloudRecordV2", {params: e})`，
/// 调用点是 `{aiDou, type, pageNumber, pageSize}` —— **GET + 四个查询参数**。
#[test]
fn cloud_record_v2_is_a_get_with_four_query_params() {
    let request = cloud_record_v2_request("JWT", "r", RECORD_BOOK_BEAN, RECORD_FILTER_ALL, 1, 50);

    assert_eq!(request.method, mcloud_core::HttpMethod::Get);
    assert!(request.body.is_none(), "GET 没有 body");
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/public/cloudRecordV2\
         ?aiDou=1&type=0&pageNumber=1&pageSize=50"
    );
    // H5 没标 `isDeviceId` ⇒ 拦截器不注入；查询串里也不该有 `deviceId`。
    assert_eq!(header(&request, "isdeviceid"), None);
    assert!(!request.url.contains("deviceId"));
    assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    // 与 `infoV3` 同族：`showLoading: true`。
    assert_eq!(header(&request, "showloading"), Some("true"));
}

/// 两个账本只差 `aiDou` 一个参数 —— 别把它写成两条不同的接口。
#[test]
fn cloud_record_v2_books_differ_only_by_the_ai_dou_param() {
    let cloud = cloud_record_v2_request("J", "r", RECORD_BOOK_CLOUD, RECORD_FILTER_ALL, 1, 10);
    let bean = cloud_record_v2_request("J", "r", RECORD_BOOK_BEAN, RECORD_FILTER_ALL, 1, 10);
    assert!(cloud.url.contains("aiDou=0"), "{}", cloud.url);
    assert!(bean.url.contains("aiDou=1"), "{}", bean.url);
    assert_eq!(
        cloud.url.replace("aiDou=0", "aiDou=1"),
        bean.url,
        "除 `aiDou` 外必须逐字相同"
    );
    // `type` 是**负数**（`-1` = 使用记录）也要原样进查询串。
    let spent = cloud_record_v2_request("J", "r", RECORD_BOOK_BEAN, RECORD_FILTER_SPENT, 1, 10);
    assert!(spent.url.contains("type=-1"), "{}", spent.url);
}

/// 流水条目的**字段名没有实拍**，所以解析层只保证两个有依据的字段，
/// 条目本身**原样**交出去 —— 这里断言"原样"这件事真的成立。
#[test]
fn parse_cloud_records_keeps_items_verbatim_and_tolerates_an_empty_book() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": 137,
            "records": [{"whateverTheServerCallsIt": 6, "another": "2026-09-19"}]
        }
    }));
    let page = parse_cloud_records(&envelope).unwrap();
    assert_eq!(page.total, Some(137), "`total` 是**条数**，不是金额");
    assert_eq!(page.records.len(), 1);
    // 逐字保真：一个键都没改、没加、没删。
    assert_eq!(
        page.records[0],
        serde_json::json!({"whateverTheServerCallsIt": 6, "another": "2026-09-19"})
    );

    // 改名前的账本本来就可能没有记录 —— 空是**答案**，不是错误。
    let empty = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"total": 0, "records": []}
    }));
    let page = parse_cloud_records(&empty).unwrap();
    assert!(page.records.is_empty());
    assert_eq!(page.total, Some(0));

    // 连 `result` 都没有也不判失败（`None` ≠ 0，见 `total` 的说明）。
    let none = Envelope::new(serde_json::json!({"code": 0, "msg": "success"}));
    let page = parse_cloud_records(&none).unwrap();
    assert!(page.records.is_empty());
    assert_eq!(page.total, None);
}

/// 上游失败要**原样透出**，不能变成"这一档没有记录" —— 那会把故障伪装成空账本。
#[test]
fn parse_cloud_records_does_not_turn_failures_into_an_empty_book() {
    let envelope = Envelope::new(serde_json::json!({"code": CODE_NOT_LOGGED_IN, "msg": "未登录"}));
    assert!(parse_cloud_records(&envelope).is_err());
}
