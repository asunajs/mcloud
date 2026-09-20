// 由 signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_upstream::app_signin::SignInStatus;
// ── 签到翻倍的门槛（`plan_multiple`）──────────────────────────────────────
//
// 判据逐字来自旧项目：`if (curMonthBackup && !curMonthBackupSignAccept)`。
// 这一组错了的表现是**白白用掉每月一次的名额**，而名额是消耗品 ——
// 错了不会报错，只会"这个月没领到"，所以必须有断言钉住。

/// 只带两个门槛字段的状态 —— 其余字段与判定无关，给最小可编译的值。
fn status_with_gates(backup: Option<bool>, accepted: Option<bool>) -> SignInStatus {
    SignInStatus {
        bean_total: 0,
        cloud_total: None,
        sign_in_points: 0,
        sign_count: 0,
        month_days: 0,
        days: Vec::new(),
        receive_list: Vec::new(),
        to_receive: None,
        cur_month_backup: backup,
        cur_month_backup_sign_accept: accepted,
    }
}

/// 两个都成立才领 —— 这是旧项目那行 `&&` 的逐字翻译。
#[test]
fn multiple_is_claimed_only_when_backup_is_on_and_not_yet_accepted() {
    assert_eq!(
        plan_multiple(&status_with_gates(Some(true), Some(false))),
        MultiplePlan::Claim
    );
    assert_eq!(
        plan_multiple(&status_with_gates(Some(true), Some(true))),
        MultiplePlan::AlreadyAccepted
    );
    assert_eq!(
        plan_multiple(&status_with_gates(Some(false), Some(false))),
        MultiplePlan::NoBackup
    );
    // 没开备份时 `SignAccept` 本该也是 `false`；万一上游给了矛盾的一对，
    // **照样不领**（`NoBackup` 优先于 `AlreadyAccepted` 的前提是它排在后面 ——
    // 这里钉住实际行为：先判 `(true,true)`，所以 `(false,true)` 落到 `NoBackup`）。
    assert_eq!(
        plan_multiple(&status_with_gates(Some(false), Some(true))),
        MultiplePlan::NoBackup
    );
}

/// ⚠ `None`（上游没说）**既不是"开了"也不是"没开"** —— 判不了就一个请求都不发。
///
/// 这一条是**安全方向**的断言：当成"开了"会白发一个写请求、把每月一次的名额用掉；
/// 当成"没开"会漏领。两个都不对，所以只能不动作 + 如实报告。
#[test]
fn multiple_does_nothing_when_a_gate_field_is_missing() {
    assert_eq!(
        plan_multiple(&status_with_gates(None, Some(false))),
        MultiplePlan::Unknown
    );
    assert_eq!(
        plan_multiple(&status_with_gates(Some(true), None)),
        MultiplePlan::Unknown
    );
    assert_eq!(
        plan_multiple(&status_with_gates(None, None)),
        MultiplePlan::Unknown
    );
}
