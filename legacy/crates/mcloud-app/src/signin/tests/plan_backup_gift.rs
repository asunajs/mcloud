// 由 signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_upstream::app_signin::BackupGift;
// ── 备份好礼的门槛（`plan_backup_gift`）────────────────────────────────
//
// 这一组错了的表现是**白白用掉每月一次的名额**，与 `plan_multiple` 同源风险。

/// 只带两个判据字段的备份好礼状态。
fn gift(state: i64, cur_month: bool) -> BackupGift {
    BackupGift { state, cur_month }
}

/// 旧项目 `switch (state)` 的逐字翻译：`0` 领、`1` 已领、其余（实测 `-1`）未开备份。
#[test]
fn backup_gift_is_claimed_only_when_state_is_zero() {
    assert_eq!(plan_backup_gift(&gift(0, true)), BackupGiftPlan::Claim);
    assert_eq!(
        plan_backup_gift(&gift(1, true)),
        BackupGiftPlan::AlreadyReceived
    );
    assert_eq!(plan_backup_gift(&gift(-1, true)), BackupGiftPlan::NoBackup);
    // 认不出来的 `state` **落到不领**（`switch` 的 `default` 旧项目是"未知状态"告警）。
    // 这里不乐观：宁可漏领也不能把每月一次的名额乱花。
    assert_eq!(plan_backup_gift(&gift(7, true)), BackupGiftPlan::NoBackup);
}

/// `curMonth` 是**前置开关**，比 `state` 更靠前 —— 为假时连 `state` 都不看。
///
/// 判据是旧项目那行 `if (!curMonth) { "无备份礼物可领"; return; }`。
/// 所以这一档**不是**"未开备份"（[`BackupGiftPlan::NoBackup`]）——
/// 把两者混起来会让用户跑去开一个已经开着的备份。
#[test]
fn backup_gift_cur_month_short_circuits_the_state() {
    assert_eq!(
        plan_backup_gift(&gift(0, false)),
        BackupGiftPlan::NothingThisMonth,
        "可领的 state 也救不了 curMonth 为假"
    );
    assert_eq!(
        plan_backup_gift(&gift(-1, false)),
        BackupGiftPlan::NothingThisMonth,
        "别把它说成 NoBackup"
    );
}
