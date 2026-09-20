// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;
use mcloud_core::Error;

// ────────────────────── 复活卡 / 回馈卡（`receiveRevivalReward`） ──────────────────────

#[test]
fn receive_revival_reward_posts_an_empty_object_body() {
    let request = receive_revival_reward_request("JWT", "r", None).unwrap();

    assert_eq!(request.method, mcloud_core::HttpMethod::Post);
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/page/receiveRevivalReward"
    );
    // H5 是 `receiveRevivalReward({})` ⇒ `JSON.stringify({})` == `"{}"`（2 字节）。
    // 老抓包里的 `content-length: 2` 正是这个。
    assert_eq!(request.body.as_deref(), Some(&b"{}"[..]));
    assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    assert_eq!(
        header(&request, "Content-Type"),
        Some("application/json;charset=UTF-8")
    );
}

/// 同页其余接口几乎都是 `showLoading: true`，只有这一条是 `false`
/// （H5 要自己画卡片弹窗，不想被全局 loading 遮住）。照抄，别"统一成 true"。
#[test]
fn receive_revival_reward_asks_for_no_global_loading() {
    let request = receive_revival_reward_request("JWT", "r", None).unwrap();
    assert_eq!(header(&request, "showloading"), Some("false"));

    // 对照组：同一个辅助函数，别的接口拿到的就是 `true`。
    assert_eq!(
        header(&info_v3_request("JWT", "r"), "showloading"),
        Some("true")
    );
}

/// H5 定义里**没有** `isDeviceId`（⇒ 不发）；老抓包里有个 `deviceid` 头（⇒ 发）。
/// 两个分支都得能用，因为它们各自有证据，而"哪个才是服务端要的"未验证。
#[test]
fn receive_revival_reward_only_sends_the_device_header_when_asked() {
    let without = receive_revival_reward_request("JWT", "r", None).unwrap();
    assert_eq!(header(&without, "deviceid"), None);

    let with = receive_revival_reward_request("JWT", "r", Some("BLOB")).unwrap();
    assert_eq!(header(&with, "deviceid"), Some("BLOB"));

    // ⚠ 这里**不加** `isdeviceid` 头 —— 老抓包里也没有。
    // 它与 `receiveV3` 不同：那一条是 `isdeviceid: true` + 值进 body，别照抄。
    assert_eq!(header(&with, "isdeviceid"), None);
}

/// `receiveTaskExpansion` 是 **GET + 单个查询参数**，不是 POST + body。
///
/// 这条形状来自 H5 源码逐字：
/// `receiveTaskExpansion: function(e){ return c["a"].get(".../receiveTaskExpansion",
/// {params: e, headers:{showLoading:!0}})}`，调用点是 `{acceptDate: t.acceptDate}`。
#[test]
fn receive_task_expansion_is_a_get_with_only_the_accept_date() {
    let request = receive_task_expansion_request("JWT", "r", Some("2026-09-01"));

    assert_eq!(request.method, mcloud_core::HttpMethod::Get);
    assert!(request.body.is_none(), "GET 没有 body");
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/page/receiveTaskExpansion?acceptDate=2026-09-01"
    );
    // H5 没标 `isDeviceId` ⇒ 拦截器不注入；`deviceId` 也不在查询串里。
    assert_eq!(header(&request, "isdeviceid"), None);
    assert!(!request.url.contains("deviceId"));
    assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    // H5 这条是 `showLoading: true`（与 `receiveRevivalReward` 的 `false` 相反）。
    assert_eq!(header(&request, "showloading"), Some("true"));
}

/// `cloudMultiple({})` —— `params` 是**空对象**，axios 序列化出来**一个参数都没有**。
#[test]
fn multiple_is_a_bare_get_with_no_query_params() {
    let request = multiple_request("JWT", "r");

    assert_eq!(request.method, mcloud_core::HttpMethod::Get);
    assert!(request.body.is_none(), "GET 没有 body");
    assert_eq!(
        request.url, "https://m.mcloud.139.com/ycloud/signin/page/multiple",
        "空 params ⇒ 连 `?` 都不该有"
    );
    assert!(!request.url.contains('?'));
    // H5 没标 `isDeviceId` ⇒ 拦截器不注入。
    assert_eq!(header(&request, "isdeviceid"), None);
    assert!(!request.url.contains("deviceId"));
    assert_eq!(header(&request, "jwttoken"), Some("JWT"));
    // H5 逐字 `showLoading: true`。
    assert_eq!(header(&request, "showloading"), Some("true"));
}

/// 没有实拍响应，所以 `cloudCount` 缺失**不能**判失败（`None` ≠ `0`）。
#[test]
fn parse_multiple_tolerates_a_missing_cloud_count() {
    let with = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"cloudCount": 21}
    }));
    assert_eq!(parse_multiple(&with).unwrap().cloud_count, Some(21));

    let without = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {}
    }));
    assert_eq!(
        parse_multiple(&without).unwrap().cloud_count,
        None,
        "上游没报数 ≠ 领到 0 豆"
    );

    // 整个 `result` 都没有也不行判失败 —— 两个客户端都只拿 `code` 判成败。
    let no_result = Envelope::new(serde_json::json!({"code": 0, "msg": "success"}));
    assert_eq!(parse_multiple(&no_result).unwrap().cloud_count, None);
}

/// 非 `0` 的 `code` 要**当失败抛出去**，不能悄悄给个 `None`。
#[test]
fn parse_multiple_rejects_a_non_zero_code() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 1302, "msg": "开启自动备份按钮才可领取哦～"
    }));
    let err = parse_multiple(&envelope).unwrap_err();
    assert!(matches!(err, Error::BadRequest(_)), "应是上游报错：{err}");
}

/// `multiple` 与 `cloudCount` 是**两个量**：前者是倍数、后者是合计。
///
/// 旧项目逐字拼的日志是 `"成功获得", multiple, "倍云朵，共计", cloudCount`
/// —— 只报 `cloudCount` 会把"翻了几倍"丢掉。
#[test]
fn parse_multiple_reads_the_multiplier_as_well() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"multiple": 2, "cloudCount": 6}
    }));
    let result = parse_multiple(&envelope).unwrap();
    assert_eq!(result.multiple, Some(2));
    assert_eq!(result.cloud_count, Some(6));

    // 两个字段各自独立缺失，互不牵连。
    let only_count = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"cloudCount": 6}
    }));
    assert_eq!(parse_multiple(&only_count).unwrap().multiple, None);
}

/// `acceptDate` 缺席时**一个查询参数都不带** —— 这是实拍定的，不是省事。
///
/// H5 的 `receiveTaskExpansion({acceptDate: t.acceptDate})` 里，`t.acceptDate`
/// 在实拍的 `receiveList[]` 里根本不存在 ⇒ `undefined` ⇒ axios 的默认序列化器
/// 把整个键丢掉。所以真实客户端发的是**裸 GET**。
///
/// ⚠ 别改成 `?acceptDate=` —— 那是**另一个请求**（多一个空参数），没验过。
#[test]
fn receive_task_expansion_drops_the_param_when_accept_date_is_absent() {
    let request = receive_task_expansion_request("JWT", "r", None);
    assert_eq!(
        request.url,
        "https://m.mcloud.139.com/ycloud/signin/page/receiveTaskExpansion"
    );
    assert!(!request.url.contains('?'), "{}", request.url);
}

/// `acceptDate` 里出现保留字符时要编码 —— 与 `startSignIn` 的 `deviceId` 同源问题。
#[test]
fn receive_task_expansion_percent_encodes_the_accept_date() {
    let request = receive_task_expansion_request("JWT", "r", Some("2026-09-01 12:00+08"));
    assert!(
        request
            .url
            .ends_with("acceptDate=2026-09-01%2012%3A00%2B08"),
        "{}",
        request.url
    );
}

/// 没有实拍响应，所以 `cloudCount` 缺失**不能**判失败（`None` ≠ `0`）。
#[test]
fn parse_receive_task_expansion_tolerates_a_missing_cloud_count() {
    let with = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {"cloudCount": 88}
    }));
    assert_eq!(
        parse_receive_task_expansion(&with).unwrap().cloud_count,
        Some(88)
    );

    let without = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success", "result": {}
    }));
    assert_eq!(
        parse_receive_task_expansion(&without).unwrap().cloud_count,
        None,
        "上游没报数 ≠ 领到 0 朵"
    );

    // **连 `result` 都没有**也不能判失败 —— 两个客户端都只认 `code`：
    // H5 只比 `e.data.code`，旧项目写的是 `receiveData?.result?.cloudCount`。
    // 这里曾经把这种响应报成 `Error::Parse`，那会让 `code=0` 的成功
    // 以一个看不懂的解析错误收场。
    let no_result = Envelope::new(serde_json::json!({ "code": 0, "msg": "success" }));
    assert_eq!(
        parse_receive_task_expansion(&no_result)
            .unwrap()
            .cloud_count,
        None,
        "没有 result 是「没说领到多少」，不是失败"
    );
}

/// H5 的两个业务错误码要**原样透出**，不能变成一句"网络异常"。
///
/// `1302` = 未开启自动备份（这是"月度那条线"的门槛，不是 bug）；
/// `500` = 已过领取有效期。
#[test]
fn parse_receive_task_expansion_surfaces_the_h5_error_codes() {
    for code in [1302, 500] {
        let envelope = Envelope::new(serde_json::json!({
            "code": code, "msg": "nope"
        }));
        let err = parse_receive_task_expansion(&envelope).unwrap_err();
        assert!(
            err.to_string().contains(&code.to_string()),
            "错误码要看得见：{err}"
        );
    }
}

#[test]
fn parse_revival_reward_reads_the_card_and_the_amount() {
    // 逐字取自实拍响应（`test/fixtures/http/复活.json` 的 `receiveRevivalReward`）。
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"cardType": 1, "littleFirstTime": 0, "rewardClouds": 114, "totalClouds": 571}
    }));
    let reward = parse_revival_reward(&envelope).unwrap();
    assert_eq!(reward.card_type, 1, "1 = 复活卡，2 = 回馈卡");
    assert_eq!(reward.reward_clouds, 114, "本次进账");
    assert_eq!(
        reward.total_clouds, 571,
        "累计（复活卡口径下是「累计复活AI豆」）"
    );
    assert_eq!(reward.little_first_time, 0);
}

#[test]
fn missing_reward_clouds_is_a_parse_error_not_a_silent_zero() {
    // 与 `receive` / `beforeTotal` 同一条理由：`rewardClouds` 就是"这次领到多少"，
    // 静默成 0 会让"没领到"看起来像"领到了 0 朵"。
    for missing in ["cardType", "rewardClouds", "totalClouds"] {
        let mut result = serde_json::json!({
            "cardType": 1, "littleFirstTime": 0, "rewardClouds": 114, "totalClouds": 571
        });
        result.as_object_mut().unwrap().remove(missing);
        let envelope =
            Envelope::new(serde_json::json!({"code": 0, "msg": "success", "result": result}));
        let err = parse_revival_reward(&envelope).unwrap_err();
        assert!(
            matches!(err, Error::Parse(_)),
            "缺 {missing} 应是解析错误：{err}"
        );
        assert!(
            format!("{err}").contains(missing),
            "报错要点名缺的是哪个字段：{err}"
        );
    }
}

/// `littleFirstTime` 是**附属字段**：它只影响一句文案，答不了本函数的任何问题。
///
/// 判据与 `receiveList` 那条一致 —— "缺了它还能不能回答本函数的问题"。
/// 与上面那条"主判据缺了必须报错"成对：**别把两种字段用同一把尺子量**。
#[test]
fn little_first_time_is_optional_and_does_not_break_the_verdict() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {"cardType": 2, "rewardClouds": 8, "totalClouds": 20}
    }));
    let reward = parse_revival_reward(&envelope).unwrap();
    assert_eq!(reward.card_type, 2);
    assert_eq!(reward.reward_clouds, 8);
    assert_eq!(reward.little_first_time, 0, "缺了给 0，不因此判整条失败");
}

/// 上游的业务失败必须转成 [`Error::BadRequest`]，不能被当成"复活到 0 朵"。
#[test]
fn parse_revival_reward_turns_upstream_failures_into_bad_request() {
    for code in [CODE_NOT_LOGGED_IN, CODE_SIGNATURE_REJECTED] {
        let envelope = Envelope::new(serde_json::json!({"code": code, "msg": "x"}));
        let err = parse_revival_reward(&envelope).unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "码 {code}：{err}");
    }
}
