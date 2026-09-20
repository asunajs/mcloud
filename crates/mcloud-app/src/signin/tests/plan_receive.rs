// 由 signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_upstream::app_signin::ReceiveItem;
// ── 领取分流（`plan_receive`）────────────────────────────────────────────
//
// 这一组测的是"**该领哪个**"——它错了的表现是"领到 0 朵"，
// 看起来像上游没发，所以必须有断言钉住。

fn item(
    cloud_type: i64,
    cloud_num: i64,
    record_id: Option<&str>,
    accept: Option<&str>,
) -> ReceiveItem {
    ReceiveItem {
        cloud_type,
        cloud_num,
        record_id: record_id.map(str::to_string),
        accept_date: accept.map(str::to_string),
    }
}

/// **本次的核心结论**：`cloudType == 2` 是「下月可领取」—— 什么都不发。
///
/// 判据是 H5 的点击分流（`2 === cloudType → $toast("下月可领取")`，
/// 同一份源码里 `"is-next-month": 2 === cloudType` 与
/// `2 === cloudType ? "" : "可领取"` 两处独立旁证）。
///
/// 实测抓包里那条账号的 `receiveList` 只有一条 `{cloudType:2, cloudNum:234}`，
/// 而整个 HAR 里 `receiveV3` 的请求数是 **0** —— 官方客户端正因为它是 `2`
/// 而**从未领过**。所以"`receiveList` 里有个数"**推不出**"现在能领到"。
#[test]
fn cloud_type_2_is_next_month_and_never_claimed() {
    let plan = plan_receive(&[item(2, 154, None, None)]);
    assert_eq!(plan, vec![ReceiveAction::NextMonth { cloud_num: 154 }]);
}

/// `cloudType == 1` 走**月度**那条线：`acceptDate` 有就用，没有就发**裸 GET**。
///
/// ⚠ 这条断言改过一次，原因值得记下来：原先写的是"缺 `acceptDate` 就跳过不发"。
/// 那**与真实客户端分叉** —— H5 是 `receiveTaskExpansion({acceptDate: t.acceptDate})`，
/// `t.acceptDate` 缺席时是 `undefined`，axios 把 `undefined` 整个键丢掉，
/// 于是它照样发、只是不带参数。所以这里也发，把"没带日期"如实记进结果。
#[test]
fn cloud_type_1_is_monthly_and_omits_a_missing_accept_date() {
    let plan = plan_receive(&[item(1, 88, None, Some("2026-09-01"))]);
    assert_eq!(
        plan,
        vec![ReceiveAction::MonthlyExpansion {
            accept_date: Some("2026-09-01".to_string()),
            cloud_num: 88,
        }]
    );

    // 缺 `acceptDate` → 仍然是**要发**的那一条，只是不带参数。
    let plan = plan_receive(&[item(1, 88, None, None)]);
    assert_eq!(
        plan,
        vec![ReceiveAction::MonthlyExpansion {
            accept_date: None,
            cloud_num: 88,
        }],
        "缺日期不等于不发 —— 真实客户端会发裸 GET"
    );

    // 空串按"没有"处理：`?acceptDate=` 只是多一个空参数，不如不发。
    let plan = plan_receive(&[item(1, 88, None, Some(""))]);
    assert_eq!(
        plan,
        vec![ReceiveAction::MonthlyExpansion {
            accept_date: None,
            cloud_num: 88,
        }]
    );
}

/// 其它 `cloudType` 才是**立即领取**；`cloudId` 照 H5 的 `recordId ?? ""`。
#[test]
fn other_cloud_types_are_immediate_and_default_cloud_id_to_empty() {
    let plan = plan_receive(&[item(3, 20, None, None)]);
    assert_eq!(
        plan,
        vec![ReceiveAction::Immediate {
            cloud_id: String::new(),
            cloud_type: 3,
            cloud_num: 20,
        }],
        "实拍里 `receiveList[]` 没有 recordId，H5 给的就是空串"
    );

    let plan = plan_receive(&[item(3, 20, Some("REC-1"), None)]);
    assert_eq!(
        plan,
        vec![ReceiveAction::Immediate {
            cloud_id: "REC-1".to_string(),
            cloud_type: 3,
            cloud_num: 20,
        }]
    );
}

/// 一条 `receiveList` 里混着三种时，**顺序与逐条语义都要原样保住**。
#[test]
fn mixed_list_keeps_each_items_own_verdict() {
    let plan = plan_receive(&[
        item(2, 154, None, None),
        item(1, 88, None, Some("2026-09-01")),
        item(4, 7, Some("R-9"), None),
    ]);
    assert_eq!(
        plan,
        vec![
            ReceiveAction::NextMonth { cloud_num: 154 },
            ReceiveAction::MonthlyExpansion {
                accept_date: Some("2026-09-01".to_string()),
                cloud_num: 88,
            },
            ReceiveAction::Immediate {
                cloud_id: "R-9".to_string(),
                cloud_type: 4,
                cloud_num: 7,
            },
        ]
    );
}
