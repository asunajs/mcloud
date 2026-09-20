// 由 app_signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use crate::Envelope;

// ───────────────────────── 响应解析（黄金向量） ─────────────────────────

/// 复活卡的响应从向量里读，**不在测试里重抄一遍数字**。
///
/// 这一段向量的来源与其余几段不同（老项目夹具，不是本次 iOS 抓包），
/// 出处与保留理由写在向量自己的 `note` 里。
#[test]
fn parse_revival_reward_from_the_pinned_capture() {
    let v = vectors();
    let segment = &v["receiveRevivalReward"];
    let envelope = Envelope::new(segment["raw"].clone());

    let reward = parse_revival_reward(&envelope).unwrap();
    let expect = &segment["raw"]["result"];
    assert_eq!(reward.card_type, expect["cardType"].as_i64().unwrap());
    assert_eq!(
        reward.reward_clouds,
        expect["rewardClouds"].as_i64().unwrap()
    );
    assert_eq!(reward.total_clouds, expect["totalClouds"].as_i64().unwrap());
    assert_eq!(
        reward.little_first_time,
        expect["littleFirstTime"].as_i64().unwrap()
    );
    assert_eq!(reward.card_type, 1, "这一段实拍用的是复活卡（回馈卡是 2）");
}

#[test]
fn parse_sso_token_from_real_capture() {
    let v = vectors();
    let entry = v["envelopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "querySpecTokenV2-success")
        .unwrap();
    let envelope = Envelope::new(entry["raw"].clone());
    let token = parse_sso_token(&envelope).unwrap();
    assert_eq!(token, entry["expectToken"].as_str().unwrap());
    assert_eq!(token.len(), 42, "形状：`YZsidssolg` + 32 位小写 hex");
    assert!(token.starts_with("YZsidssolg"));
    assert_eq!(envelope.code(), Some(0), "`code` 是字符串 \"0000\"，也要认");
}

#[test]
fn parse_server_timestamp_keeps_all_13_digits() {
    let v = vectors();
    let entry = v["envelopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "h5-timestamp-success")
        .unwrap();
    let envelope = Envelope::new(entry["raw"].clone());
    let ts = parse_server_timestamp(&envelope).unwrap();
    assert_eq!(ts, "1789800128813");
    assert_eq!(ts.len(), 13, "毫秒级 —— 少一位就是秒，签名会不对");
}

#[test]
fn parse_sign_in_status_from_real_capture() {
    let v = vectors();
    let envelope = Envelope::new(v["infoV3"]["raw"].clone());
    let status = parse_sign_in_status(&envelope).unwrap();

    // ── 余额是 `total`，不是 `beforeTotal`（这对数字是实拍的一前一后）──
    //
    // 黄金向量这份是**领取前**的实拍：`total = 10496`、`beforeTotal = 8027`。
    // 领掉那条 `{cloudType:0, cloudNum:6}` 之后重新抓，`total` 变成 **10502**（+6 ✓），
    // 而 `beforeTotal` **一动不动还是 8027**。
    //
    // ⇒ 余额是 `total`。`beforeTotal` 是云朵明细（历史）页口径的另一个数。
    // 这一条曾经记反过（拿 `beforeTotal` 当余额写进 `cloud_stats`）。
    assert_eq!(status.bean_total, 10496);
    assert_eq!(status.cloud_total, Some(8027));
    // 兑换门槛 12000 减去余额，正好是上游自己报的"还差多少"。
    // 这条等式是 `total` = 余额最硬的旁证：两个字段由服务端分别算出，能对上说明口径一致。
    assert_eq!(12000 - status.bean_total, 1504);
    assert_eq!(
        v["infoV3"]["raw"]["result"]["lackCloudNum"].as_i64(),
        Some(1504),
        "`lackCloudNum` 必须等于 12000 - total"
    );
    assert_eq!(status.sign_in_points, 3);
    assert_eq!(status.sign_count, 1);
    assert_eq!(status.month_days, 7);
    assert_eq!(status.days.len(), 35, "抓包里的 cal 有 35 格");

    // 两条独立路径必须一致：一条数日历格，一条读服务端字段
    assert_eq!(
        status.counted_month_days() as i64,
        status.month_days,
        "`monthDays` 应等于 cal 里 `s && currentMonth` 的条数"
    );

    // 权威判据
    assert_eq!(status.signed_today(), Some(true), "抓包那一刻 19 号已签");
    assert_eq!(status.signed_on(19), Some(true));
    assert_eq!(status.signed_on(3), Some(false));
    assert_eq!(status.signed_on(0), None);
    assert_eq!(status.signed_on(32), None);
}

#[test]
fn signed_on_ignores_other_months() {
    // `cal` 里混着上月最后一天与下月开头几天。按 `d` 裸查会命中错的月份 ——
    // 这里构造"上月 31 号签了、本月 31 号没签"，必须只认本月那个。
    let status = SignInStatus {
        bean_total: 0,
        cloud_total: Some(0),
        sign_in_points: 0,
        sign_count: 0,
        month_days: 0,
        days: vec![
            SignInDay {
                day: 31,
                signed: true,
                current_month: false,
                points: 0,
                today: false,
            },
            SignInDay {
                day: 31,
                signed: false,
                current_month: true,
                points: 0,
                today: false,
            },
        ],
        receive_list: Vec::new(),
        to_receive: None,
        cur_month_backup: None,
        cur_month_backup_sign_accept: None,
    };
    assert_eq!(status.signed_on(31), Some(false));
    assert_eq!(status.signed_today(), None, "没有 t:true 的格子时返回 None");
}

/// `receiveList` 是**可选的**：不给就是空数组，不能拖垮"今天签没签"这个主判据。
///
/// 这条与 `missing_required_field_is_a_parse_error_not_a_silent_zero` 是**一对**：
/// 主判据缺字段必须报错，附属字段缺了必须放过。判据是"缺了它还能不能回答本函数的问题"。
#[test]
fn receive_list_is_optional_and_never_breaks_the_main_verdict() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": 1, "beforeTotal": 1, "signInPoints": 1, "signCount": 1, "monthDays": 0,
            "cal": [{"d": 1, "s": true, "n": 0, "p": 0, "t": true, "currentMonth": 1}]
        }
    }));
    let status = parse_sign_in_status(&envelope).unwrap();
    assert!(status.receive_list.is_empty());
    assert_eq!(status.to_receive, None);
    assert_eq!(status.signed_today(), Some(true), "主判据照常可用");
}

/// 逐字对照 HAR 里的真实 `infoV3` 响应：`[{"cloudType":2,"cloudNum":234}]`。
#[test]
fn parse_sign_in_status_reads_the_receive_list() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": 7431, "beforeTotal": 5392, "signInPoints": 3, "signCount": 1, "monthDays": 6,
            "toReceive": 0,
            "cal": [],
            "receiveList": [{"cloudType": 2, "cloudNum": 234}]
        }
    }));
    let status = parse_sign_in_status(&envelope).unwrap();

    assert_eq!(status.to_receive, Some(0));
    assert_eq!(status.receive_list.len(), 1);
    assert_eq!(status.receive_list[0].cloud_type, 2);
    assert_eq!(status.receive_list[0].cloud_num, 234);
    assert_eq!(
        status.receive_list[0].record_id, None,
        "实拍里没有 recordId —— 别替上游编一个，H5 的 `?? \"\"` 给的就是空串"
    );
}

/// `receiveList` 里缺 `cloudType` / `cloudNum` 的条目被**跳过**，而不是整条报错。
#[test]
fn receive_items_without_the_required_numbers_are_skipped() {
    let envelope = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": {
            "total": 1, "beforeTotal": 1, "signInPoints": 1, "signCount": 1, "monthDays": 0,
            "cal": [],
            "receiveList": [{"cloudNum": 234}, {"cloudType": 2}, {"cloudType": 1, "cloudNum": 7}]
        }
    }));
    let status = parse_sign_in_status(&envelope).unwrap();
    assert_eq!(status.receive_list.len(), 1, "只有最后一条是完整的");
    assert_eq!(status.receive_list[0].cloud_num, 7);
}

#[test]
fn parse_start_sign_in_from_real_capture() {
    let v = vectors();
    let envelope = Envelope::new(v["startSignIn"]["raw"].clone());
    let result = parse_start_sign_in(&envelope).unwrap();
    assert!(result.today_sign_in, "抓包那一刻已签，所以是 true");
    assert_eq!(result.sign_in_points, 3);
    assert_eq!(result.sign_count, 1);
    assert_eq!(result.server_time_ms, Some(1789799929696));
}

#[test]
fn parse_task_list_requires_an_array() {
    let ok = Envelope::new(serde_json::json!({
        "code": 0, "msg": "success",
        "result": [{"id": 549, "name": "开启备份领好礼"}]
    }));
    let items = parse_task_list(&ok).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], 549);

    let bad = Envelope::new(serde_json::json!({"code": 0, "result": {"id": 1}}));
    assert!(
        parse_task_list(&bad).is_err(),
        "result 是对象时要报错，不能当空列表"
    );
}
