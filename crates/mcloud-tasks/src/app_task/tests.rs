//! `app-task` 写侧**判据层**的测试。
//!
//! 这一层刻意不碰网络：它的全部价值就是"哪一条要传、按哪一档传"能在离线状态下
//! 被逐条钉住（与参考实现把判据做成数据表同一条理由）。真发的枪在后面的执行层。

use super::*;

fn item(id: Option<i64>, group: Option<&str>, name: Option<&str>) -> TaskV2Item {
    TaskV2Item {
        task_id: id,
        group_id: group.map(str::to_string),
        name: name.map(str::to_string),
        ..Default::default()
    }
}

// ── 玩法 106：就传一个 ────────────────────────────────────────────────

/// 旧 `runners.ts:41-44`：runner 体里只有裸的 `uploadRandomFile(ctx)`。
#[test]
fn task_106_uploads_exactly_one() {
    let s = upload_shape_for(&item(Some(106), Some("new"), Some("上传一个随机文件"))).unwrap();
    assert_eq!(s, UPLOAD_ONE);
    assert_eq!(s.rounds, 1);
    // ⚠ 106 的 runner 体里**没有**那句 sleep ⇒ 不踱步
    assert!(!s.pace_each_round, "106 不该每轮等一拍");
    assert_eq!(s.stop_at_process, 0);
}

// ── 玩法 522：循环凑满 ────────────────────────────────────────────────

/// 旧 `runners.ts:117-125`：`for i < (?? 5)`，每轮先看 `process >= 100`，
/// 传一个、等 500ms。
#[test]
fn task_522_is_the_looping_shape() {
    let s = upload_shape_for(&item(Some(522), Some("month"), Some("每月上传任务"))).unwrap();
    assert_eq!(s.rounds, ROUNDS_PER_RUN);
    assert_eq!(s.rounds, 5, "旧实现的默认值是 5，不是 100");
    assert_eq!(s.stop_at_process, STOP_AT_PROCESS);
    assert!(s.pace_each_round, "522 每轮都要等一拍");
}

/// ⚠ 522 与 106 的**唯一**形状差别就是那个 `pace_each_round`。
///
/// 混成一个的后果两种都不报错：要么 522 变成连传 5 个零间隔，
/// 要么 106 白等一拍 —— 只在"哪天被判成脚本"那一类最难查的差异上显形。
#[test]
fn the_only_difference_between_106_and_522_is_the_pacing() {
    let one = upload_shape_for(&item(Some(106), None, None)).unwrap();
    let many = upload_shape_for(&item(Some(522), None, None)).unwrap();
    assert_ne!(one.rounds, many.rounds);
    assert_ne!(one.pace_each_round, many.pace_each_round);
    assert_ne!(one.stop_at_process, many.stop_at_process);
    assert!(one.rounds < many.rounds);
}

// ── beiyong1 的名字门 ─────────────────────────────────────────────────

/// 旧 `tasks.ts:68-77` 逐字：`name.includes("上传") && (name.includes("图") || name.includes("照"))`。
#[test]
fn the_beiyong1_name_gate_is_the_legacy_conjunction() {
    assert!(beiyong1_name_gate("上传图片"));
    assert!(beiyong1_name_gate("上传照片"));
    assert!(beiyong1_name_gate("上传一张图"));
    assert!(!beiyong1_name_gate("上传文件"), "有'上传'但没有'图/照'");
    assert!(!beiyong1_name_gate("看图有礼"), "有'图'但没有'上传'");
    assert!(!beiyong1_name_gate("签到"));
    assert!(!beiyong1_name_gate(""));
}

/// 过名字门 ⇒ 与 106 **同一档**（旧实现两处都是裸的 `uploadRandomFile(ctx)`）。
#[test]
fn the_beiyong1_name_gate_uses_the_same_shape_as_106() {
    let by_name = upload_shape_for(&item(None, Some("beiyong1"), Some("上传照片领云朵"))).unwrap();
    let by_id = upload_shape_for(&item(Some(106), None, None)).unwrap();
    assert_eq!(by_name, by_id, "两条路必须给同一档形状");
}

/// ⚠ 名字门**只在 beiyong1 分组里**生效。
///
/// 别的分组里出现"上传图片"是**不传**的 —— 旧实现那一句在 `case "beiyong1"` 里。
/// 顺手放宽会变成"在别的分组里也往网盘写文件"，而那是一条**没人要求过的写**。
#[test]
fn the_name_gate_only_applies_inside_beiyong1() {
    assert!(upload_shape_for(&item(None, Some("beiyong1"), Some("上传图片"))).is_some());
    for group in ["new", "day", "month", "time", "cloudEmail", "hidden"] {
        assert!(
            upload_shape_for(&item(None, Some(group), Some("上传图片"))).is_none(),
            "分组 {group} 里不该凭名字传：旧实现那句在 beiyong1 分支里"
        );
    }
    assert!(upload_shape_for(&item(None, None, Some("上传图片"))).is_none());
}

/// 判不出来 ⇒ `None`（**不传**）。默认档必须是"什么都不做"。
#[test]
fn an_unknown_task_uploads_nothing() {
    assert!(upload_shape_for(&item(Some(0), Some("new"), Some("签到"))).is_none());
    assert!(upload_shape_for(&item(Some(999), Some("new"), Some("看视频"))).is_none());
    assert!(upload_shape_for(&item(None, None, None)).is_none());
    assert!(upload_shape_for(&item(Some(105), Some("new"), None)).is_none());
}

/// ⚠ beiyong1 里 `task_id == 106` 时，两条路给的是**同一档**。
#[test]
fn id_and_name_gates_agree_when_both_hit() {
    let both = upload_shape_for(&item(Some(106), Some("beiyong1"), Some("上传图片"))).unwrap();
    assert_eq!(both, UPLOAD_ONE);
}

// ── 每轮还要不要再传 ─────────────────────────────────────────────────

/// 传满上限就停。
#[test]
fn the_round_cap_is_the_real_guard() {
    let s = upload_shape_for(&item(Some(522), None, None)).unwrap();
    for done in 0..s.rounds {
        assert!(should_upload_another(s, done, None), "已传 {done} 个应继续");
    }
    assert!(!should_upload_another(s, s.rounds, None), "传满就该停");
    assert!(!should_upload_another(s, s.rounds + 9, None));
}

/// ⭐ 就算 `process` 永远读到 0（R15 说这一格语义未验证），**上限照样兜底**。
#[test]
fn the_cap_holds_even_if_process_never_moves() {
    let s = upload_shape_for(&item(Some(522), None, None)).unwrap();
    let mut done = 0;
    while should_upload_another(s, done, Some(0)) {
        done += 1;
        assert!(done <= ROUNDS_PER_RUN, "绝不能超过上限");
    }
    assert_eq!(done, ROUNDS_PER_RUN);
}

/// `process` 到门槛就提前停（含"已经在门槛之上 ⇒ 一个都不传"）。
#[test]
fn reaching_the_process_threshold_stops_early() {
    let s = upload_shape_for(&item(Some(522), None, None)).unwrap();
    assert!(should_upload_another(s, 0, Some(0)));
    assert!(should_upload_another(s, 0, Some(STOP_AT_PROCESS - 1)));
    assert!(!should_upload_another(s, 0, Some(STOP_AT_PROCESS)));
    assert!(!should_upload_another(s, 0, Some(STOP_AT_PROCESS + 1)));
    assert!(!should_upload_another(s, 0, Some(999)));
}

/// ⚠ `process` 缺失当 0 ⇒ **继续传**，与旧实现 `undefined >= 100 === false` 同向。
#[test]
fn a_missing_process_reads_as_zero() {
    let s = upload_shape_for(&item(Some(522), None, None)).unwrap();
    assert!(should_upload_another(s, 0, None));
}

/// 106 那一档没有 `process` 门槛 ⇒ 那一格读什么都不影响它。
#[test]
fn the_one_shot_shape_ignores_process() {
    let s = UPLOAD_ONE;
    assert_eq!(s.stop_at_process, 0);
    assert!(
        should_upload_another(s, 0, Some(999)),
        "没有门槛就不该被它停住"
    );
    assert!(!should_upload_another(s, 1, Some(999)), "但上限仍然管用");
}

// ══════════════ 结论（判据层之外，执行层产出怎么念） ══════════════

/// 表里没有要传的 ⇒ `Skipped`，且话说清是"没有"，不是"做完了"。
#[test]
fn nothing_planned_is_skipped_not_success() {
    let r = AppTaskResult::default();
    assert_eq!(r.planned, 0);
    assert!(matches!(r.task_outcome(), TaskOutcome::Skipped { .. }));
    assert!(
        r.summary_line().contains("没有需要上传的条目"),
        "{}",
        r.summary_line()
    );
}

/// ⭐ **判出该做却一条都没做成** ⇒ 报**失败**，不许报成"没事发生"。
///
/// 这条是零值纪律的那一侧：`acted == 0` 而 `planned > 0` 说明**该做的没做**，
/// 把它念成成功会让一条天天失败的能力在界面上看起来一切正常。
#[test]
fn planned_but_nothing_acted_is_a_failure() {
    let r = AppTaskResult {
        planned: 3,
        acted: 0,
        ..Default::default()
    };
    assert!(
        matches!(r.task_outcome(), TaskOutcome::Failed { .. }),
        "实得：{:?}",
        r.task_outcome()
    );
}

/// 清理那一半的账要念全：清干净与没清干净是两句不同的话。
#[test]
fn the_cleanup_tail_reports_what_was_actually_reclaimed() {
    let clean = AppTaskResult {
        planned: 1,
        acted: 1,
        file_ids: vec!["F1".into(), "F2".into()],
        cleaned_up: 2,
        ..Default::default()
    };
    assert!(
        clean.summary_line().contains("新增 2 个文件，已移进回收站"),
        "{}",
        clean.summary_line()
    );

    let partial = AppTaskResult {
        planned: 1,
        acted: 1,
        file_ids: vec!["F1".into(), "F2".into()],
        cleaned_up: 1,
        cleanup_error: "网络断了".into(),
        ..Default::default()
    };
    let line = partial.summary_line();
    assert!(line.contains("只收回 1 个"), "{line}");
    assert!(line.contains("网络断了"), "失败原文要带出来：{line}");
}

/// 没传任何文件时**不念**清理那一段（避免"新增 0 个文件"这种噪音）。
#[test]
fn an_empty_cleanup_tail_is_omitted() {
    let r = AppTaskResult {
        planned: 1,
        acted: 1,
        file_ids: vec![],
        ..Default::default()
    };
    assert!(!r.summary_line().contains("回收站"), "{}", r.summary_line());
}

// ══════════════ 打卡的门（currstep） ══════════════
//
// 旧 TS `tasks.ts:19-28` 的 `clickTaskWithStep`：`currstep == 0` 才打，
// 已经走起来的不打（但门**通过**）；434 是唯一例外，只在第 22 步打。

fn with_step(id: i64, step: Option<i64>) -> TaskV2Item {
    TaskV2Item {
        task_id: Some(id),
        curr_step: step,
        ..Default::default()
    }
}

/// 第 0 步 ⇒ 打。**`currstep` 缺席也算第 0 步**（JS 的默认参数就是 0）。
#[test]
fn step_zero_clicks() {
    assert!(should_click(&with_step(106, Some(0))));
    assert!(
        should_click(&with_step(522, None)),
        "缺席按 JS 的默认值 0 算"
    );
}

/// ⭐ 已经走起来的 ⇒ **不打**。这一条是那个真 bug 的守门。
///
/// 无条件打卡会让"上游本没打算再记一笔"的条目多一发写请求，
/// 而 `task/click` **不可回滚**。真机第一趟的
/// `522 uploaded 0 clicked true` 就是这么来的。
#[test]
fn a_started_task_is_not_clicked() {
    for step in [1, 2, 5, 21, 23, 99] {
        assert!(
            !should_click(&with_step(522, Some(step))),
            "第 {step} 步不该打"
        );
    }
}

/// 434 是唯一例外：只在**恰好第 22 步**打。
#[test]
fn the_share_task_clicks_only_at_its_special_step() {
    assert!(should_click(&with_step(434, Some(22))));
    assert!(
        !should_click(&with_step(434, Some(0))),
        "434 在第 0 步**不**打"
    );
    assert!(!should_click(&with_step(434, Some(21))));
    assert!(!should_click(&with_step(434, Some(23))));
    assert!(!should_click(&with_step(434, None)));
}

/// 例外**只对 434 生效**：别的任务在第 22 步一样不打。
#[test]
fn the_special_step_does_not_leak_to_other_tasks() {
    assert!(!should_click(&with_step(522, Some(22))));
    assert!(!should_click(&with_step(106, Some(22))));
}
