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
    assert_eq!(s.per_round_ms, 0, "106 不该每轮等");
    assert_eq!(s.stop_at_process, 0);
    assert_eq!(s.file_ext, ".bin", "不带 ext ⇒ 走缺省后缀");
}

// ── 玩法 113：使用PC客户端 ──────────────────────────────────────────

/// ⭐ 113 的形状与 106 **完全同档**，差别**只在渠道头** ✓。
///
/// 旧 TS `runners.ts:27-31` 里**只有**这一个玩法传了 `channelSrc: "10200153"`
///（别处 `uploadRandomFile()` 走缺省=安卓档 ✓）—— 因为这条任务奖励的就是
/// "**用 PC 客户端**"，按安卓档传等于没做 ✓。真机读数（两个账号一致）：
/// `使用PC客户端` / `group month` / `market sign_in_3` / `limitType MONTH` /
/// `currStep 0` / `state WAIT` ✓。
#[test]
fn task_113_uploads_one_file_on_the_pc_channel() {
    let pc = upload_shape_for(&item(
        Some(PC_UPLOAD_TASK_ID),
        Some("month"),
        Some("使用PC客户端"),
    ))
    .expect("113 必须有一档");
    assert_eq!(pc, UPLOAD_ONE_PC);
    assert_eq!(pc.channel, CloudChannel::Pc, "PC 档是这条任务的**本体**");
    assert_eq!(pc.rounds, 1, "与 106 同档：一趟一个");
    assert_eq!(pc.stop_at_process, 0);
    assert_eq!(pc.per_round_ms, 0, "106 那一档没有每轮等待 ✓");

    // 对照：106 是安卓档（缺省档 ✓）⇒ 两档**只差渠道**这一格 ✓
    let android = upload_shape_for(&item(Some(106), None, None)).expect("106 有一档");
    assert_eq!(android.channel, CloudChannel::Android);
    assert_eq!(
        UploadShape {
            channel: CloudChannel::Pc,
            ..android
        },
        pc,
        "除渠道外与 106 一模一样 —— 差一点点都说明形状被改坏了 ✗"
    );
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
    assert_eq!(
        s.per_round_ms, 500,
        "522 每轮等 500ms（旧实现那句 sleep(500) ✓）"
    );
}

/// ⚠ 522 与 106 的**唯一**形状差别就是那个每轮等待（500ms vs 0）。
///
/// 混成一个的后果两种都不报错：要么 522 变成连传 5 个零间隔，
/// 要么 106 白等一拍 —— 只在"哪天被判成脚本"那一类最难查的差异上显形。
#[test]
fn the_only_difference_between_106_and_522_is_the_pacing() {
    let one = upload_shape_for(&item(Some(106), None, None)).unwrap();
    let many = upload_shape_for(&item(Some(522), None, None)).unwrap();
    assert_ne!(one.rounds, many.rounds);
    assert_ne!(one.per_round_ms, many.per_round_ms);
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

// ── beiyong1 的按钮门：一次传 10 张（玩法 615 那一族） ─────────────────

/// ⭐⭐ 615「记录国庆节欢乐瞬间」这一类**只有按钮门认得出**：它的名字里既没有"上传"、
/// 也没有"图/照"（`beiyong1_name_gate` 直接漏掉它 ✗）⇒ 全靠 `button.app.link` ✓。
///
/// 实拍（抓包仓 `har/2026-09-25-通知有礼.har` 的 `taskListV3`）：同族条目
/// `id=614 name=上传中秋团圆照 description=上传10张可领 process=6 state=WAIT group=beiyong1`
/// 且 `button.app.link = mcloud://main/uploadView` ✓；真机 615 的 `description`
/// 也是「上传10张可领」、`groupid` 也是 `beiyong1` ✓。
#[test]
fn the_beiyong1_button_gate_routes_to_ten_uploads() {
    let mut it = item(Some(615), Some("beiyong1"), Some("记录国庆节欢乐瞬间"));
    it.button_app_link = Some("mcloud://main/uploadView".into());
    let s = upload_shape_for(&it).expect("按钮门该认出它");
    assert_eq!(s, UPLOAD_TEN);
    assert_eq!(s.rounds, 10, "一次最多 10 张");
    assert_eq!(
        s.stop_at_process, 10,
        "`process` 到 10 就停（= 旧实现那句 `remaining = 10 - process` ✓）"
    );
    assert_eq!(s.per_round_ms, 300, "每张 300ms（旧实现那句 sleep(300) ✓）");
    assert_eq!(s.file_ext, ".jpg", "这一族刻意传 .jpg ✓");
    assert!(
        s.top_up_from_process,
        "⚠ 这一族是「先算剩余再循环」（`remaining = 10 - process`）⇒ 上限随 process 收 ✓"
    );
}

/// ⚠ 名字门**优先**：两条都命中时走"一张"那一档 —— 旧 TS 就是先 `if (名字门) {...return}` ✓。
#[test]
fn the_name_gate_wins_over_the_button_gate() {
    let mut it = item(Some(615), Some("beiyong1"), Some("上传国庆照片"));
    it.button_app_link = Some("mcloud://main/uploadView".into());
    assert_eq!(upload_shape_for(&it), Some(UPLOAD_ONE), "名字门在前 ✓");
}

/// ⚠ beiyong1 里**两条门都没中** ⇒ 不传（只打卡）。
///
/// 别把整个分组都当成"要传文件" ✗ —— 那是往用户网盘里写没人要求过的东西。
#[test]
fn a_beiyong1_item_with_neither_gate_uploads_nothing() {
    let mut other_link = item(Some(616), Some("beiyong1"), Some("记录欢乐瞬间"));
    other_link.button_app_link = Some("mcloud://main/otherLink".into());
    assert_eq!(upload_shape_for(&other_link), None, "别的按钮 ⇒ 不动");
    assert_eq!(
        upload_shape_for(&item(Some(616), Some("beiyong1"), Some("记录欢乐瞬间"))),
        None,
        "连 link 都读不出 ⇒ 也不动"
    );
    // ⚠ 按钮门**只在 beiyong1 里**生效：别的组哪怕带 uploadView 也不因此传 10 张 ✓
    let mut outside = item(Some(700), Some("day"), Some("别的组"));
    outside.button_app_link = Some("mcloud://main/uploadView".into());
    assert_eq!(upload_shape_for(&outside), None);
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

/// ⚠⚠ 复审 `tk-1`（口径 B，用户 2026-10-04 拍板）：
/// **"动了"与"上游确认了"是两件事**，摘要必须把"没核对到"的那部分说出来 ——
/// 但**不许**因此改成败。
///
/// 背景是同一份代码里的真机读数：106 `clicked:true` 而上游 `state` 恒 `WAIT`
/// ⇒ 那种"成功"其实是"**没核对**"。旧摘要只说"做了 N 条"，读起来像"N 条都验过了" ✗。
///
/// 红测：① 撤掉 `settle_unconfirmed()`（或让 `run_app_task` 不再调它）⇒ 前几组断言红；
/// ② 把它改成"未确认就算失败"⇒ 最后那条 `Success` 断言红（那是口径 A，用户否过一次 ✗）。
#[test]
fn acted_but_unconfirmed_is_labelled_without_flipping_the_outcome() {
    let mut r = AppTaskResult {
        planned: 3,
        acted: 3,
        ..Default::default()
    };
    // 434：分享次数 0/7 → 1/7 ⇒ **涨了** = 确认 ✓，不该被标
    r.steps.push(TaskStep {
        task_id: Some(SHARE_TASK_ID),
        name: "分享".into(),
        uploaded: 1,
        shared: true,
        share_before: Some("0/7".into()),
        share_after: Some("1/7".into()),
        share_counted: true,
        ..Default::default()
    });
    // 106：只"打卡 + 传了文件"，**没有**可比读数 ⇒ 未确认（如实说"没核对"）
    r.steps.push(TaskStep {
        task_id: Some(106),
        name: "上传图片".into(),
        uploaded: 1,
        clicked: true,
        ..Default::default()
    });
    // 522：进度 42/100 → 42/100 **没动** ⇒ 未确认（这是"比过了、没动"）
    r.steps.push(TaskStep {
        task_id: Some(522),
        name: "上传".into(),
        uploaded: 5,
        progress_total: Some(100),
        progress_before: Some("42/100".into()),
        progress_after: Some("42/100".into()),
        ..Default::default()
    });

    r.settle_unconfirmed();

    assert_eq!(
        r.unconfirmed,
        vec![106, 522],
        "只有**真涨了**的那条才算确认"
    );
    let line = r.summary_line();
    assert!(line.contains("另有 2 条未获上游确认"), "{line}");

    // 明细里逐条写清**为什么**（"没核对"与"比过了没动"是两句不同的话）
    let p106 = r.steps[1].problems.join(" ");
    assert!(p106.contains("没有可核对的读数"), "{p106}");
    let p522 = r.steps[2].problems.join(" ");
    assert!(p522.contains("42/100 → 42/100"), "{p522}");
    assert!(
        r.steps[0].problems.is_empty(),
        "确认过的条目不该被念叨：{:?}",
        r.steps[0].problems
    );

    // ⚠ 口径 B 的**契约**：加了标注**不改成败** ——
    // 这条要是变成 Failed/Partial，就等于把被否掉的口径 A 偷偷装回来了 ✗。
    assert!(
        matches!(r.task_outcome(), TaskOutcome::Success { .. }),
        "实得：{:?}",
        r.task_outcome()
    );
}

// ══════════════ 打卡的门（currstep） ══════════════
//
// 旧 TS `tasks.ts:19-28` 的 `clickTaskWithStep`：`currstep == 0` 才打，
// 已经走起来的不打（但门**通过**）；434 在第 22 步**额外**打一发（叠加，不是替代）。

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

/// 434：`specialMap` 是**叠加**的一条 —— 第 22 步**额外**打一发，
/// 而"第 0 步 / `currstep` 缺席 ⇒ 打"那一句对它**照样生效** ✓。
///
/// ⚠ 这一条曾经把 434 钉成"只在第 22 步打"的**替代**规则 ✗ ⇒ 上游不给
/// `currstep` 时（2026-10-04 小号 run 96 正是如此）**一发都没打** ✗。
#[test]
fn the_share_task_clicks_at_its_special_step_and_still_at_step_zero() {
    assert!(
        should_click(&with_step(434, Some(22))),
        "第 22 步是额外那一发"
    );
    assert!(
        should_click(&with_step(434, Some(0))),
        "第 0 步照旧打：叠加不是替代"
    );
    assert!(
        should_click(&with_step(434, None)),
        "缺席按 JS 的默认值 0 算 ⇒ 照旧打"
    );
    assert!(
        !should_click(&with_step(434, Some(21))),
        "走到一半（非 0 非 22）不打"
    );
    assert!(!should_click(&with_step(434, Some(23))));
    assert!(!should_click(&with_step(434, Some(5))));
}

/// 例外**只对 434 生效**：别的任务在第 22 步一样不打。
#[test]
fn the_special_step_does_not_leak_to_other_tasks() {
    assert!(!should_click(&with_step(522, Some(22))));
    assert!(!should_click(&with_step(106, Some(22))));
}

// ══════════════ 状态门（`state` / `enable`） ══════════════
//
// 旧 TS `index.ts:34` 逐字：`if (task.state === "FINISH" || task.enable !== 1) continue;`

fn stateful(state: Option<&str>, enable: Option<i64>) -> TaskV2Item {
    TaskV2Item {
        state: state.map(str::to_string),
        enable,
        ..Default::default()
    }
}

/// 已完成 ⇒ 不动。**这条对会写的链最要紧** —— 否则会对着做完的条目
/// 再发一发不可回滚的 `task/click`、再往网盘传一遍文件。
#[test]
fn a_finished_task_is_not_touched() {
    assert!(!should_act(&stateful(Some("FINISH"), Some(1))));
}

/// `enable != 1` ⇒ 不动；**缺席也算关**（`None != Some(1)`）。
#[test]
fn a_disabled_task_is_not_touched_and_missing_enable_counts_as_disabled() {
    assert!(!should_act(&stateful(Some("WAIT"), Some(0))));
    assert!(!should_act(&stateful(Some("WAIT"), Some(2))));
    assert!(
        !should_act(&stateful(Some("WAIT"), None)),
        "缺席过不了那道门"
    );
}

/// 两道门都过 ⇒ 动。
#[test]
fn a_pending_enabled_task_is_acted_on() {
    assert!(should_act(&stateful(Some("WAIT"), Some(1))));
    // ⚠ `state` 缺席**不算完成** —— 只有 `"FINISH"` 才是那一档
    assert!(should_act(&stateful(None, Some(1))));
    assert!(should_act(&stateful(Some("DOING"), Some(1))));
}

// ══════════════ 玩法 434：分享（`Step::Share`） ══════════════
//
// 旧 TS `runners.ts:68-88`。⚠ 它与上传**互斥** —— 它消费别人生产的文件。
//
// ⚠ 判"这一条成没成"**不看**打卡、也不看外链，只看**次数涨没涨**（见 `share_counted` ✓）。

/// 活体形状（run 93 / run 99 实测）⇒ 解出 `(0, 7)`。
#[test]
fn share_progress_comes_from_the_live_name_shape() {
    assert_eq!(
        share_progress_from_name("分享文件领好礼<span id='share_title'>0/7</span>"),
        Some((0, 7))
    );
    assert_eq!(
        share_progress_from_name("分享文件得云朵<span id='share_title'>3/7</span>"),
        Some((3, 7))
    );
}

/// ⚠ **文案变了也要能解** —— 旧 TS 的 `getShareCount` 是 `replace` 死前缀
///（`"分享文件得云朵<span id='share_title'>"` + `"/7</span>"`）：文案一换就解出
/// `NaN`，而 `NaN > x` 恒 `false` ⇒ **静默地永远不报成功** ✗。
#[test]
fn share_progress_survives_copy_changes() {
    assert_eq!(
        share_progress_from_name("分享有礼<span id='share_title'>2/7</span>"),
        Some((2, 7))
    );
    // 连 span 都没有：退到"整串里第一个 a/b"
    assert_eq!(share_progress_from_name("分享文件领好礼 2/7"), Some((2, 7)));
}

/// 读不出 ⇒ `None` —— **不是 0** ✓（当成 0 会把"没涨"判成"涨了/没涨"反过来 ✗）。
#[test]
fn share_progress_is_none_when_there_is_no_ratio() {
    assert_eq!(share_progress_from_name("分享文件领好礼"), None);
    assert_eq!(share_progress_from_name(""), None);
    assert_eq!(
        share_progress_from_name("分享文件领好礼<span id='share_title'>进行中</span>"),
        None
    );
    // 只有一半的也不算
    assert_eq!(share_progress_from_name("分享 0/"), None);
    assert_eq!(share_progress_from_name("分享 /7"), None);
    // 全角斜杠之类没实拍过的东西**不猜**（宁可读不出 ✓）
    assert_eq!(share_progress_from_name("分享 0／7"), None);
}

/// 434 判成 `Share`，**不是**上传。
#[test]
fn task_434_is_the_share_step() {
    let it = item(Some(434), Some("day"), Some("分享文件"));
    assert_eq!(step_for(&it), Some(Step::Share));
    // ⚠ 它**没有**上传形状 —— 两个 `Option` 的写法会让这里表达成"两个都填"
    assert_eq!(upload_shape_for(&it), None);
}

/// 上传那几档仍然判成 `Upload`（接线没把它们挤掉）。
#[test]
fn the_upload_tasks_still_map_to_upload_steps() {
    assert_eq!(
        step_for(&item(Some(106), None, None)),
        Some(Step::Upload(UPLOAD_ONE))
    );
    assert!(matches!(
        step_for(&item(Some(522), None, None)),
        Some(Step::Upload(_))
    ));
    assert!(matches!(
        step_for(&item(None, Some("beiyong1"), Some("上传图片"))),
        Some(Step::Upload(_))
    ));
}

/// 判不出来仍然 ⇒ 什么都不做。
#[test]
fn an_unknown_task_gets_no_step() {
    assert_eq!(
        step_for(&item(Some(999), Some("day"), Some("看视频"))),
        None
    );
    assert_eq!(step_for(&item(None, None, None)), None);
}

/// ⚠ 状态门对 434 一样管用（两道 `continue` 是并列的，不是只管上传）。
#[test]
fn the_state_gate_also_covers_the_share_task() {
    let mut done = item(Some(434), Some("day"), Some("分享文件"));
    done.state = Some("FINISH".into());
    done.enable = Some(1);
    assert!(!should_act(&done), "已完成的 434 不该再去建外链");

    let mut off = item(Some(434), Some("day"), Some("分享文件"));
    off.state = Some("WAIT".into());
    off.enable = Some(0);
    assert!(!should_act(&off), "关掉的 434 不该再去建外链");
}

// ══════════════════════════ 执行层：三道门的接线 ══════════════════════════
//
// 判据层的纯函数在上面已经钉住；这一节钉的是**接线** —— 三道门真的挡在
// `task/click` 前面吗？用假 HTTP 数枪、用假 store 喂账号状态。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use mcloud_core::{
    Account, AccountId, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink,
    HttpClient, HttpResponse, RunEvent,
};
use mcloud_engine::TaskContext;

/// 固定账号 id（与 `AccountId::new` 一致即可，测试里只用来对账）。
const TEST_ACCOUNT: &str = "19****01";

/// 假 HTTP：按顺序答复，用尽后重复最后一个。请求全部留底，供数枪。
struct Scripted {
    seen: Mutex<Vec<mcloud_core::HttpRequest>>,
    replies: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(replies: Vec<String>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            // 反转，便于从队尾取
            replies: Mutex::new(replies.into_iter().rev().collect()),
        }
    }

    fn requests(&self) -> Vec<mcloud_core::HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    /// 打到 `task/click` 的请求 URL（**不可回滚的那一发**）。
    fn clicks(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|r| {
                r.url
                    .contains(mcloud_upstream::app_task_click::TASK_CLICK_PATH)
            })
            .map(|r| r.url)
            .collect()
    }
}

/// 取一枚请求头（**大小写不敏感**）—— 钉子要断言"113 真按 PC 档发" ✓
///（渠道在这条任务里就是**全部**，只看 `uploaded:1` 证明不了它按 PC 档发的 ✗）。
fn header(req: &mcloud_core::HttpRequest, name: &str) -> Option<String> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: mcloud_core::HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req);
        let body = {
            let mut r = self.replies.lock().unwrap();
            if r.len() > 1 {
                r.pop().unwrap()
            } else {
                r.last().cloned().unwrap_or_else(|| "{}".to_string())
            }
        };
        Ok(HttpResponse {
            status: 200,
            body,
            headers: Vec::new(),
        })
    }
}

/// 一份**按账号**存 `account_state` 的替身；别的 store 方法一律拒绝
/// （这条链只该碰 `account_state` / `save_account_state`）。
struct FakeStore {
    states: Mutex<HashMap<String, serde_json::Value>>,
}

impl FakeStore {
    fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    fn seed(&self, id: &str, state: serde_json::Value) {
        self.states.lock().unwrap().insert(id.to_string(), state);
    }

    fn state_of(&self, id: &str) -> serde_json::Value {
        self.states
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    }
}

/// 这条链不该碰的 store 方法统一回这个错。
fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("这条链不该碰 store 的别的部分".into())
}

#[async_trait]
impl mcloud_core::AccountStore for FakeStore {
    async fn accounts(&self) -> Result<Vec<Account>> {
        Err(refuse())
    }
    async fn account(&self, _id: &AccountId) -> Result<Option<Account>> {
        Err(refuse())
    }
    async fn save_account(&self, _a: &Account) -> Result<()> {
        Err(refuse())
    }
    async fn remove_account(&self, _id: &AccountId) -> Result<()> {
        Err(refuse())
    }
    async fn account_state(&self, id: &AccountId) -> Result<serde_json::Value> {
        Ok(self.state_of(id.as_str()))
    }
    async fn save_account_state(&self, id: &AccountId, state: &serde_json::Value) -> Result<()> {
        self.seed(id.as_str(), state.clone());
        Ok(())
    }
}

/// `SmidStore` 替身：默认"这个 store 不管 smid"。
///
/// 2026-10-03 起 `Store` 把 `SmidStore` 并成了 supertrait（任务层要能读
/// 这个账号**已换回来**的那个 `deviceId` —— 它和登录时那份不是同一个东西）。
/// 这些替身要证明的事与 smid 无关，所以读一律 `None`、写一律拒绝。
#[async_trait]
impl mcloud_core::SmidStore for FakeStore {
    async fn get_smid_state(
        &self,
        _id: &mcloud_core::AccountId,
    ) -> mcloud_core::Result<Option<mcloud_core::SmidState>> {
        Ok(None)
    }
    async fn set_smid_fingerprint(
        &self,
        _id: &mcloud_core::AccountId,
        _fingerprint_json: &str,
    ) -> mcloud_core::Result<()> {
        Err(refuse())
    }
    async fn set_smid_device_id(
        &self,
        _id: &mcloud_core::AccountId,
        _device_id: &str,
    ) -> mcloud_core::Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::RunStore for FakeStore {
    async fn upsert_run(&self, _r: &mcloud_core::RunRecord) -> Result<()> {
        Err(refuse())
    }
    async fn runs(&self, _limit: usize) -> Result<Vec<mcloud_core::RunRecord>> {
        Err(refuse())
    }
    async fn run(&self, _id: mcloud_core::RunId) -> Result<Option<mcloud_core::RunRecord>> {
        Err(refuse())
    }
    async fn running_runs(&self) -> Result<Vec<mcloud_core::RunRecord>> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::LogStore for FakeStore {
    async fn append_logs(
        &self,
        _id: mcloud_core::RunId,
        _lines: &[mcloud_core::LogLine],
    ) -> Result<mcloud_core::AppendOutcome> {
        Err(refuse())
    }
    async fn logs(
        &self,
        _id: mcloud_core::RunId,
        _after: Option<mcloud_core::Seq>,
    ) -> Result<Vec<mcloud_core::LogLine>> {
        Err(refuse())
    }
    async fn clear_logs(&self, _id: mcloud_core::RunId) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::ScheduleStore for FakeStore {
    async fn schedules(&self) -> Result<Vec<mcloud_core::Schedule>> {
        Err(refuse())
    }
    async fn schedule(
        &self,
        _id: &mcloud_core::ScheduleId,
    ) -> Result<Option<mcloud_core::Schedule>> {
        Err(refuse())
    }
    async fn save_schedule(&self, _s: &mcloud_core::Schedule) -> Result<()> {
        Err(refuse())
    }
    async fn remove_schedule(&self, _id: &mcloud_core::ScheduleId) -> Result<()> {
        Err(refuse())
    }
    async fn mark_schedule_fired(&self, _id: &mcloud_core::ScheduleId, _ms: i64) -> Result<()> {
        Err(refuse())
    }
    async fn record_schedule_run(
        &self,
        _id: &mcloud_core::ScheduleId,
        _ms: Option<i64>,
        _status: &str,
        _err: Option<&str>,
    ) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::PipelineStore for FakeStore {
    async fn pipelines(&self) -> Result<Vec<mcloud_core::Pipeline>> {
        Err(refuse())
    }
    async fn pipeline(
        &self,
        _id: &mcloud_core::PipelineId,
    ) -> Result<Option<mcloud_core::Pipeline>> {
        Err(refuse())
    }
    async fn save_pipeline(&self, _p: &mcloud_core::Pipeline) -> Result<()> {
        Err(refuse())
    }
    async fn remove_pipeline(&self, _id: &mcloud_core::PipelineId) -> Result<()> {
        Err(refuse())
    }
    async fn mark_pipeline_ran(&self, _id: &mcloud_core::PipelineId, _ms: i64) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::UserStore for FakeStore {
    async fn user_count(&self) -> Result<usize> {
        Err(refuse())
    }
    async fn users(&self) -> Result<Vec<mcloud_core::User>> {
        Err(refuse())
    }
    async fn user_by_name(&self, _name: &str) -> Result<Option<mcloud_core::User>> {
        Err(refuse())
    }
    async fn user(&self, _id: &mcloud_core::UserId) -> Result<Option<mcloud_core::User>> {
        Err(refuse())
    }
    async fn save_user(&self, _u: &mcloud_core::User) -> Result<()> {
        Err(refuse())
    }
    async fn session(&self, _h: &str) -> Result<Option<mcloud_core::Session>> {
        Err(refuse())
    }
    async fn save_session(&self, _s: &mcloud_core::Session) -> Result<()> {
        Err(refuse())
    }
    async fn remove_session(&self, _h: &str) -> Result<()> {
        Err(refuse())
    }
    async fn remove_user_sessions(&self, _id: &mcloud_core::UserId) -> Result<usize> {
        Err(refuse())
    }
    async fn purge_expired_sessions(&self, _now_ms: i64) -> Result<usize> {
        Err(refuse())
    }
}

/// 固定时钟（月份门要能把它挪到任意一天）。
struct FixedClock(i64);
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

/// 什么都不留的 sink —— 这一节不读事件，只要 `TaskContext` 构造得出来。
struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _event: RunEvent) {}
}

/// **把日志行留下来**的 sink —— 用来钉"用户能不能在日志里看到进度" ✓。
///
/// 起因（2026-10-04 用户口径）：「522 应该是成功的，但是当前日志没有输出他的进度，
/// 毕竟需要 100 个，也不急着一次做完，**进度还是很重要的**」✓。
#[derive(Default)]
struct CapturedSink {
    lines: Mutex<Vec<String>>,
}
impl CapturedSink {
    /// 打出来的所有信息（含 `info` / `success` / `fail`，级别都留着 ✓）。
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}
impl EventSink for CapturedSink {
    fn emit(&self, event: RunEvent) {
        if let RunEvent::Log { message, .. } = event {
            self.lines.lock().unwrap().push(message);
        }
    }
}

fn account(id: &str) -> Account {
    Account {
        id: AccountId::new(id),
        nickname: None,
        auth: AuthInfo {
            raw: "Basic eA==".into(),
            platform: "pc".into(),
            phone: "19900000001".into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("DEV+1/base=".into()),
        pc_platform: None,
        user_domain_id: None,
        owner: None,
        enabled: true,
        settings: AccountSettings::default(),
    }
}

fn ctx_with(
    id: &str,
    replies: Vec<String>,
    now_ms: i64,
    settings: AccountSettings,
    store: Arc<FakeStore>,
) -> (TaskContext, Arc<Scripted>) {
    let (ctx, http, _sink) =
        ctx_with_sink(id, replies, now_ms, settings, store, Arc::new(NullSink));
    (ctx, http)
}

/// 同上，但**把 sink 交出去**（要给"日志里到底打了什么"写断言时用 ✓）。
fn ctx_with_sink(
    id: &str,
    replies: Vec<String>,
    now_ms: i64,
    settings: AccountSettings,
    store: Arc<FakeStore>,
    sink: Arc<dyn EventSink>,
) -> (TaskContext, Arc<Scripted>, Arc<dyn EventSink>) {
    let http = Arc::new(Scripted::new(replies));
    let mut acc = account(id);
    acc.settings = settings;
    let ctx = TaskContext {
        account: acc,
        http: http.clone(),
        store,
        clock: Arc::new(FixedClock(now_ms)),
        sink: sink.clone(),
        run_id: mcloud_core::RunId(1),
        cancel: CancelToken::new(),
        chain_cancel: None,
        artifacts: mcloud_engine::ArtifactRecorder::new(
            Arc::new(mcloud_engine::ArtifactLedger::new()),
            mcloud_core::RunId(1),
            AccountId::new(id),
        ),
        details: mcloud_engine::DetailsRecorder::new(
            mcloud_engine::DetailsLedger::new(),
            mcloud_core::RunId(1),
        ),
    };
    (ctx, http, sink)
}

/// 东八区 2026-10-<day> 12:00 的 epoch 毫秒。
///
/// ⚠ 不引 `chrono`（`mcloud-tasks` 的依赖里没有它）—— 用 core 的
/// `parse_beijing_time` 把墙钟串解成毫秒，顺带与它同源。
fn beijing_noon(day: i64) -> i64 {
    let s = format!("2026-10-{day:02}T12:00:00");
    mcloud_core::parse_beijing_time(&s, "%Y-%m-%dT%H:%M:%S").expect("固定日期必须解得出来")
}

/// 一条任务的 JSON（进 `result.<group>` 数组）。
fn task_json(id: i64, group: &str, name: &str, market: &str) -> String {
    format!(
        r#"{{"id":{id},"name":"{name}","groupid":"{group}","state":"WAIT","limitType":"DAY","enable":1,"currstep":0,"process":0,"marketname":"{market}"}}"#
    )
}

/// 与 [`task_json`] 同形，但**带 `button.app.link` 与 `process`** ——
/// 615 那一族的门就在 `button` 这一格（实拍形状见 `app_task_list_v2.rs` 的注释 ✓）。
fn task_json_with_button(
    id: i64,
    group: &str,
    name: &str,
    market: &str,
    link: &str,
    process: i64,
) -> String {
    format!(
        r#"{{"id":{id},"name":"{name}","groupid":"{group}","state":"WAIT","limitType":"EVER","enable":1,"currstep":0,"process":{process},"marketname":"{market}","button":{{"app":{{"text":"去完成","action":"jump","link":"{link}"}}}}}}"#
    )
}

/// 与 [`task_json`] 同形，但**带 `process`** —— 522 那一档的"进度"就在这一格 ✓。
fn task_json_with_process(id: i64, group: &str, name: &str, market: &str, process: i64) -> String {
    format!(
        r#"{{"id":{id},"name":"{name}","groupid":"{group}","state":"WAIT","limitType":"DAY","enable":1,"currstep":0,"process":{process},"marketname":"{market}"}}"#
    )
}

/// 与 [`task_json`] 同形，但**不带 `currstep`** —— 上游确实会这么给
///（2026-10-04 小号 run 96 就是：面板那句自述打成"第 0 步"，而格子里是缺席 ✓）。
fn task_json_without_step(id: i64, group: &str, name: &str, market: &str) -> String {
    format!(
        r#"{{"id":{id},"name":"{name}","groupid":"{group}","state":"WAIT","limitType":"DAY","enable":1,"process":0,"marketname":"{market}"}}"#
    )
}

/// 十六次拉取都回同一份"这一组就这些条目"的响应（去重后留下的就是它们）。
fn page_body(items: &[String]) -> String {
    format!(
        r#"{{"code":0,"msg":"success","groupState":{{}},"result":{{"g":[{}]}}}}"#,
        items.join(",")
    )
}

/// 十六份相同页面 + 后续的打卡 / 上传答复（页数用生产同一个函数算）。
fn replies_for(items: &[String], tail: Vec<String>) -> Vec<String> {
    let n = crate::app_task_matrix::app_task_matrix_fetches();
    let mut v = vec![page_body(items); n];
    v.extend(tail);
    v
}

/// `task/click` 成功。
fn click_ok() -> String {
    r#"{"code":0,"msg":"success"}"#.to_string()
}

/// `file/create` 回"撞同名" ⇒ 编排层不再发 PUT / complete，也不清理。
fn create_exists() -> String {
    r#"{"success":true,"code":"0000","message":"success","data":{"fileId":"F-EXIST","uploadId":null,"exist":true,"rapidUpload":false,"partInfos":null}}"#.to_string()
}

/// `file/create` 回一个**新文件**（形状取自 `app_cloud_upload/tests.rs` 那条合成样本 ✓）
/// ⇒ 后面还会跟 PUT 与 complete 两发。
fn create_new() -> String {
    r#"{"code":"0000","message":"请求成功","success":true,"data":{"exist":false,"fileId":"SYNTH-FID","fileName":"synth.png","formInfo":null,"parentFileId":"/","partInfos":[{"uploadUrl":"https://synth.example.invalid/put"}],"rapidUpload":false,"transferToken":null,"type":"file","uploadId":"SYNTH-UID"}}"#.to_string()
}

/// `getOutLink` 成功（形状取自 `app_cloud_outlink/tests.rs` 那条 ✓）。
fn outlink_ok() -> String {
    r#"{"code":"0","data":{"getOutLinkRes":{"getOutLinkResSet":[{"linkID":"L1","linkUrl":"https://caiyun.139.com/w/#/synth"}]}}}"#.to_string()
}

/// `delOutLink` 成功。
fn del_outlink_ok() -> String {
    r#"{"code":"0","data":{}}"#.to_string()
}

// ── a) 黑名单：命中就一枪不发 ────────────────────────────────────────

/// 黑名单**逐字**照抄 TS `tasks.ts:111`（不增删；TS 原文是 11 个）。
#[test]
fn the_skip_list_is_copied_verbatim_from_ts() {
    assert_eq!(
        SKIP_TASK_LIST,
        [117, 437, 991, 406, 385, 480, 1022, 1018, 603, 1021, 120]
    );
    for id in SKIP_TASK_LIST {
        assert!(is_skipped_task(Some(id)), "{id} 该命中");
        assert!(!should_click(&with_step(id, Some(0))), "{id} 不该打卡");
        assert!(!should_act(&with_step(id, Some(0))), "{id} 不该进 planned");
    }
    assert!(!is_skipped_task(None));
    assert!(!is_skipped_task(Some(118)));
}

/// ⭐ a) 黑名单 id 的条目**一发 `task/click` 都不发**；未列入的照发。
///
/// 117（黑名单）与 118（不在名单）都是 beiyong1 里的"上传图片" ⇒ 两道都过
/// 名字门、都有上传形状。没有黑名单这道门时排序后先打 117、再打 118（**两发**）；
/// 补上之后只剩 118 那一发。
#[tokio::test]
async fn a_blacklisted_id_never_gets_clicked() {
    let items = vec![
        task_json(117, "beiyong1", "上传图片甲", "sign_in_3"),
        task_json(118, "beiyong1", "上传图片乙", "sign_in_3"),
    ];
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![click_ok(), create_exists()]),
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");

    let clicks = http.clicks();
    assert_eq!(clicks.len(), 1, "只有 118 该被打卡，实得 {clicks:?}");
    assert!(clicks[0].contains("id=118"), "打卡的是 {}", clicks[0]);
    assert!(!clicks[0].contains("id=117"));
    assert_eq!(result.planned, 1, "黑名单条目不该进 planned");
}

/// ⭐ 434 **没有 `currstep`**（上游就是这么给的）时也要真打那一发 `task/click`。
///
/// 参考实现里 `specialMap` 是**叠加**的 ⇒ 那句 `currstep === 0` 对 434 一样命中 ✓；
/// 写成"替代"的那一版这一趟是 `clicked:false`（2026-10-04 小号 run 96 实测 ✗）。
/// ⚠ 这条必须落在**运行级**：上一版的错值正是被一条单测钉住的 ✗ ⇒ 只改单测
/// 证明不了"请求真的发出去了"。
#[tokio::test]
async fn the_share_task_clicks_when_upstream_omits_the_step() {
    let items = vec![task_json_without_step(
        434,
        "month",
        "分享文件领好礼",
        "sign_in_3",
    )];
    // 这一趟只有 434 ⇒ 它手里没有文件 ⇒ **自己补传一个**（用户口径 2026-10-04）✓：
    // 打卡 → create → PUT → complete → 建外链 → 删外链 → 回收站 → 事后整表。
    let mut replies = replies_for(&items, vec![]);
    replies.push(click_ok());
    replies.push(create_new());
    replies.push("{}".to_string());
    replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    replies.push(outlink_ok());
    replies.push(del_outlink_ok());
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string());
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");

    let clicks = http.clicks();
    assert_eq!(clicks.len(), 1, "434 该真打一发，实得 {clicks:?}");
    assert!(clicks[0].contains("id=434"), "打卡的是 {}", clicks[0]);

    assert_eq!(result.planned, 1, "434 该进 planned");
    assert!(result.steps[0].clicked, "明细里 clicked 必须是 true");
    assert_eq!(result.steps[0].uploaded, 1, "⭐ 空手就自己传一个");
    assert!(result.steps[0].shared, "有了自己传的文件 ⇒ 外链建得起来");
    assert_eq!(result.cleaned_up, 1, "自己补传那个也要在收尾被移进回收站");
    assert!(
        !result.steps[0]
            .problems
            .iter()
            .any(|p| p.contains("没有可分享的文件")),
        "⚠ 这条现在**不该**再出现（自己补传了）：{:?}",
        result.steps[0].problems
    );
    // ⭐ 这一档**仍不算推进**：判据是"**上游次数涨没涨**" ✓ —— 它这一趟名字里没有那个
    // `a/b`（"动手前读不出"）、事后也读不出 ⇒ `acted` 停在 0、结论 failed ✓。
    // **"发过写请求"本身不构成推进**（2026-10-04 用户口径：官方没有判断完成的接口，
    // 只能比次数 ✓）；补传那个文件只是**把这一条变得可做**，不是"做成了" ✓。
    assert_eq!(result.acted, 0, "没比出次数上涨就不算推进");
    assert!(
        result.task_outcome().is_failure(),
        "没推进就是 failed：{:?}",
        result.task_outcome()
    );
    assert!(
        result.steps[0]
            .problems
            .iter()
            .any(|p| p.contains("读不出次数")),
        "要如实说清为什么不算：{:?}",
        result.steps[0].problems
    );
}

/// ⭐⭐ **只跑 434** 时也要能成 —— 用户口径 2026-10-04：「**没有共享文件 id 就主动上传**」✓。
///
/// 这一趟只有 434 ⇒ 它自己补传一个文件 → 建外链/删外链 → 事后整表 `0/7` 变 `1/7`
/// ⇒ `share_counted` ✓、`acted = 1` ✓、结论 success ✓、补传那个也进了回收站 ✓。
/// ⚠ 撤掉"补传"那几行 ⇒ 这条会红在 `shared`（退回"没有可分享的文件"✗）。
#[tokio::test]
async fn a_lone_share_task_uploads_its_own_file_and_counts() {
    let name = |done: &str| format!("分享文件领好礼<span id='share_title'>{done}/7</span>");
    let before = vec![task_json(434, "month", &name("0"), "sign_in_3")];
    let after = vec![task_json(434, "month", &name("1"), "sign_in_3")];
    let mut replies = replies_for(&before, vec![]);
    replies.push(click_ok());
    replies.push(create_new()); // ⭐ 自己补的那一个：create → PUT → complete
    replies.push("{}".to_string());
    replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    replies.push(outlink_ok());
    replies.push(del_outlink_ok());
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 回收站
    replies.extend(replies_for(&after, vec![]));

    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[434]).await.expect("不该是 Err");

    let share = &result.steps[0];
    assert_eq!(share.uploaded, 1, "空手 ⇒ 主动传一个");
    assert!(share.shared, "有了自己传的 ⇒ 外链建得起来");
    assert_eq!(share.share_before.as_deref(), Some("0/7"));
    assert_eq!(share.share_after.as_deref(), Some("1/7"));
    assert!(share.share_counted, "次数涨了 ⇒ 算记上");
    assert_eq!(result.acted, 1, "这一条算推进了");
    assert!(
        result.task_outcome().is_success(),
        "{:?}",
        result.task_outcome()
    );
    assert_eq!(result.cleaned_up, 1, "补传那个要收回");
    assert!(http.clicks().iter().any(|c| c.contains("id=434")));

    // ⚠ 这一条里 434 自己是"动的那个"，次数真涨了 ⇒ **不该**被标未确认
    //（`settle_unconfirmed` 的端到端断言在下面那条 106+434 的场景里 —— 那里才有
    //  "动了但没有可比读数"的那一档）。
    assert!(
        result.unconfirmed.is_empty(),
        "次数涨了的那条不算未确认：{:?}",
        result.unconfirmed
    );
    assert!(
        !result.summary_line().contains("未获上游确认"),
        "{}",
        result.summary_line()
    );
}

/// ⭐ 434 的**真判据**：跑完再拉一次表，**次数涨了**才算这一条成了 ✓。
///
/// 这一条是正向端到端：106 真传一个文件（分享才有得分享）→ 434 打卡 + 建外链删外链
/// → 事后再拉整表，名字从 `0/7` 变成 `1/7` ⇒ `share_counted = true`、`acted` +1 ✓。
#[tokio::test]
async fn the_share_task_counts_only_when_the_upstream_number_moves() {
    let before = vec![
        task_json(106, "day", "手动上传一个文件", "sign_in_3"),
        task_json(
            434,
            "month",
            "分享文件领好礼<span id='share_title'>0/7</span>",
            "sign_in_3",
        ),
    ];
    let after = vec![
        task_json(106, "day", "手动上传一个文件", "sign_in_3"),
        task_json(
            434,
            "month",
            "分享文件领好礼<span id='share_title'>1/7</span>",
            "sign_in_3",
        ),
    ];
    let mut replies = replies_for(&before, vec![]);
    // ⚠ 顺序是**每条先打卡、再做那一件事**（`app_task.rs` 的 ③-a/③-b ✓）：
    // 106 打卡 → create → PUT → complete → 434 打卡 → 建外链 → 删外链 → 回收站 → 事后整表。
    replies.push(click_ok()); // 106 打卡
    replies.push(create_new());
    replies.push("{}".to_string()); // PUT（状态 200 就算成功）
    replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    replies.push(click_ok()); // 434 打卡
    replies.push(outlink_ok());
    replies.push(del_outlink_ok());
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 回收站
    replies.extend(replies_for(&after, vec![]));

    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");

    assert_eq!(result.planned, 2, "106 与 434 都该进 planned");
    let clicks = http.clicks();
    assert_eq!(clicks.len(), 2, "106 与 434 各打一发：{clicks:?}");
    assert!(clicks.iter().any(|c| c.contains("id=434")));
    let share = &result.steps[1];
    assert!(share.shared, "外链建了又删了 ⇒ 技术动作成了");
    assert_eq!(share.share_before.as_deref(), Some("0/7"));
    assert_eq!(share.share_after.as_deref(), Some("1/7"));
    assert!(share.share_counted, "次数涨了 ⇒ 这一条算记上");
    assert_eq!(result.acted, 2, "上传 1 条 + 分享记上 1 条");
    assert!(
        result.task_outcome().is_success(),
        "{:?}",
        result.task_outcome()
    );
    assert!(result.summary_line().contains("做了 2 条"));

    // ⚠⚠ 复审 tk-1（口径 B）——**端到端**这一侧：**动过 ≠ 上游确认过**。
    //
    // 这一趟里 106 打了卡、也真传了文件，但那一档**没有任何可比读数**
    // ⇒ 必须被标成"未获上游确认"（旧实现只说"做了 2 条"，读起来像两条都验过了 ✗）；
    // 434 的次数真涨了 ⇒ **不**算未确认 ✓。
    //
    // 挂在这个既有场景（而不是只测 `settle_unconfirmed` 本身）是**故意**的：
    // 只测方法的话，`run_app_task` 哪天不再调它就没人发现
    //（sv-2 那条钉子第一版就是这么写成绿的 ✗）。
    assert_eq!(
        result.unconfirmed,
        vec![106],
        "实得：{:?}",
        result.unconfirmed
    );
    assert!(
        result.summary_line().contains("另有 1 条未获上游确认"),
        "{}",
        result.summary_line()
    );
    assert!(
        result.steps[0]
            .problems
            .iter()
            .any(|p| p.contains("没有可核对的读数")),
        "{:?}",
        result.steps[0].problems
    );
    assert!(
        result.steps[1].problems.is_empty(),
        "确认过的那条不该被念叨：{:?}",
        result.steps[1].problems
    );
    // 口径 B 的契约：加了标注**不改成败**
    assert!(
        result.task_outcome().is_success(),
        "{:?}",
        result.task_outcome()
    );
}

/// ⭐ 反向：外链建成了、但**上游次数没动** ⇒ 这一条**不算**记上 ✓。
///
/// 这正是参考实现那句「分享文件成功（**分享成功不等于任务完成**）」的意思 ——
/// 没有接口能判完成，只能比次数 ✓。`acted` 只算上传那 1 条，`problems` 里必须
/// 把"动了多少"写清楚（`0/7 → 0/7`），别让人以为它成了 ✓。
#[tokio::test]
async fn a_share_that_does_not_move_the_number_is_not_counted() {
    let same = vec![
        task_json(106, "day", "手动上传一个文件", "sign_in_3"),
        task_json(
            434,
            "month",
            "分享文件领好礼<span id='share_title'>0/7</span>",
            "sign_in_3",
        ),
    ];
    let mut replies = replies_for(&same, vec![]);
    replies.push(click_ok()); // 106 打卡
    replies.push(create_new());
    replies.push("{}".to_string());
    replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    replies.push(click_ok()); // 434 打卡
    replies.push(outlink_ok());
    replies.push(del_outlink_ok());
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string());
    replies.extend(replies_for(&same, vec![])); // 事后那一轮也还是 0/7

    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");

    let share = &result.steps[1];
    assert!(share.shared, "外链这一步确实成了");
    assert_eq!(share.share_before.as_deref(), Some("0/7"));
    assert_eq!(share.share_after.as_deref(), Some("0/7"));
    assert!(!share.share_counted, "次数没动 ⇒ 不算记上");
    assert_eq!(result.acted, 1, "只算上传那一条");
    assert!(
        share
            .problems
            .iter()
            .any(|p| p.contains("次数没动") && p.contains("0/7 → 0/7")),
        "要写明动了多少：{:?}",
        share.problems
    );
    assert!(
        http.clicks()
            .iter()
            .filter(|c| c.contains("id=434"))
            .count()
            == 1
    );
}

/// ⭐⭐ 434 **必须排在生产者后面** —— 它消费的是**这一趟**传上去的文件 ✓。
///
/// 计划按 id 升序（`merge_pages` 的 `sort_by_key`）⇒ 434(434) 排在 522(522) 前面 ✗，
/// 而**两个账号的 106 都是 `FINISH`** ⇒ 真机上 434 永远拿不到文件。
/// 实证（2026-10-04 run 103，测试号，`onlyTaskIds=522,434`）：522 传了 5 个文件，
/// 434 那一格却是「没有可分享的文件，跳过分享任务」✗。这条就是那个洞的钉子 ——
/// ⚠ 它**只在运行级**能验（单测夹具里 106 常年是 `WAIT`，正好把这个顺序洞盖住 ✗）。
#[tokio::test]
async fn the_share_task_runs_after_the_uploaders() {
    let name_434 = |done: &str| format!("分享文件领好礼<span id='share_title'>{done}/7</span>");
    let before = vec![
        task_json(434, "month", &name_434("0"), "sign_in_3"),
        task_json(522, "month", "当月上传文件满100个", "sign_in_3"),
    ];
    let after = vec![
        task_json(434, "month", &name_434("1"), "sign_in_3"),
        task_json(522, "month", "当月上传文件满100个", "sign_in_3"),
    ];
    let mut replies = replies_for(&before, vec![]);
    replies.push(click_ok()); // 522 打卡
    for _ in 0..5 {
        // 522 那一档最多传 5 个（`ROUNDS_PER_RUN` ✓）
        replies.push(create_new());
        replies.push("{}".to_string());
        replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    }
    replies.push(click_ok()); // 434 打卡
    replies.push(outlink_ok());
    replies.push(del_outlink_ok());
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 回收站
    replies.extend(replies_for(&after, vec![]));

    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    // ⚠ 传进去的顺序故意把 434 放前面（模拟 `onlyTaskIds="434,522"`）—— 仍该被挪到末尾 ✓
    let result = run_app_task(&ctx, 0, &[434, 522])
        .await
        .expect("不该是 Err");

    assert_eq!(result.steps[0].task_id, Some(522), "生产者先跑");
    assert_eq!(result.steps[1].task_id, Some(434), "434 挪到末尾");
    assert_eq!(result.steps[0].uploaded, 5, "522 传满 5 个");
    let share = &result.steps[1];
    assert!(
        share.shared,
        "434 该拿得到 522 刚传上去的文件：{:?}",
        share.problems
    );
    assert!(share.share_counted, "次数 0/7 → 1/7 ⇒ 记上");
    assert_eq!(result.acted, 2, "522 一条 + 434 一条");
    assert!(http.clicks().iter().any(|c| c.contains("id=434")));
}

/// ⭐ 522 的**进度**要打出来（明细两格 + 日志一行）—— 用户口径 2026-10-04：
/// 「522 应该是成功的，但是当前日志没有输出他的进度，毕竟需要 100 个，
/// 也不急着一次做完，**进度还是很重要的**」✓。
///
/// 这一档一趟只传 `ROUNDS_PER_RUN` 个、要攒 `STOP_AT_PROCESS` 个 ⇒ 只看
/// `uploaded:5` **完全看不出走到哪儿** ✗；所以跑完那一趟"事后整表"要顺带把
/// `process` 读回来、与动手前那个数**并列写出来** ✓（`42/100 → 47/100` ✓）。
#[tokio::test]
async fn the_monthly_upload_reports_its_progress_before_and_after() {
    let name = "当月上传文件满100个";
    let before = vec![task_json_with_process(522, "month", name, "sign_in_3", 42)];
    let after = vec![task_json_with_process(522, "month", name, "sign_in_3", 47)];
    let mut replies = replies_for(&before, vec![]);
    replies.push(click_ok());
    for _ in 0..5 {
        replies.push(create_new());
        replies.push("{}".to_string());
        replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    }
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 回收站
    replies.extend(replies_for(&after, vec![]));

    let sink = Arc::new(CapturedSink::default());
    let (ctx, _http, _sink) = ctx_with_sink(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
        sink.clone(),
    );
    let result = run_app_task(&ctx, 0, &[522]).await.expect("不该是 Err");

    let step = &result.steps[0];
    assert_eq!(step.uploaded, 5, "这一趟传满 5 个");
    assert_eq!(step.progress_before.as_deref(), Some("42/100"));
    assert_eq!(step.progress_after.as_deref(), Some("47/100"));
    let lines = sink.lines();
    assert!(
        lines.iter().any(|l| l.contains("42/100 → 47/100")),
        "日志里必须有进度：{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("这一趟 +5")),
        "还要写清这一趟贡献了多少：{lines:?}"
    );
    assert_eq!(result.acted, 1, "这一档的成就是「这一趟传了文件」");
    assert!(
        result.task_outcome().is_success(),
        "{:?}",
        result.task_outcome()
    );
}

/// ⭐⭐ **回归钉子：`exist:null` 那一版回执照样要能传上去** ✓。
///
/// 2026-10-05 真机：上游把"没撞同名"从 `exist:false` 改成 **`exist:null`** ✗ ⇒
/// 106 / 113 / 522 / 615 与 434 的补传**一起**废掉（86 发 create 全被拒 ✗）。
/// 判据改成"有没有开出新上传会话"之后，这一版形状必须走通 ✓ —— 传上去、且**记进清理名单** ✓
/// （真机 run 131：`uploaded=1`、`收回 1/1` ✓）。
#[tokio::test]
async fn the_null_exist_receipt_still_uploads_and_gets_cleaned_up() {
    let before = vec![task_json_with_process(
        522,
        "month",
        "当月上传文件满100个",
        "sign_in_3",
        42,
    )];
    let after = vec![task_json_with_process(
        522,
        "month",
        "当月上传文件满100个",
        "sign_in_3",
        47,
    )];
    // ⚠ 这一版就是 2026-10-05 实拍的形状：`exist:null` + 有 uploadId/uploadUrl ✓
    //（522 是**循环 5 轮** ⇒ 五轮都要给齐，少一轮后面全错位 ✗ —— 2026-10-05 自己踩过 ✓）
    let null_exist_create = |n: usize| {
        format!(
            r#"{{"success":true,"code":"0000","message":"请求成功","data":{{"parentFileId":"/","fileId":"F-NULL-{n}","type":"file","fileName":"x{n}.bin","rapidUpload":false,"uploadId":"U-{n}","partInfos":[{{"uploadUrl":"https://oss.invalid/put{n}"}}],"exist":null,"formInfo":null,"transferToken":null}}}}"#
        )
    };
    let mut replies = replies_for(&before, vec![]);
    replies.push(click_ok());
    for n in 0..5 {
        replies.push(null_exist_create(n));
        replies.push("{}".to_string());
        replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    }
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 回收站
    replies.extend(replies_for(&after, vec![]));

    let (ctx, _http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[522]).await.expect("不该是 Err");

    assert_eq!(
        result.steps[0].uploaded, 5,
        "exist:null 那一版必须能传上去（5 轮全成）"
    );
    assert!(
        result.file_ids.contains(&"F-NULL-0".to_string()),
        "既然是新开的会话 ⇒ 那个 id 是我们造的 ⇒ 必须进清理名单: {:?}",
        result.file_ids
    );
    assert_eq!(result.cleaned_up, 5, "链尾要把这 5 个都收回");
    assert!(result.failures.is_empty(), "{:?}", result.failures);
}

/// ⭐⭐ **上传全废那一趟必须报出来**（2026-10-05 真机：86 发 create 被 `exist:null` 打死，
/// 而摘要一个字没提、结论还因为别的条目"打卡成功"报了 success ✗✗）。
///
/// 钉三件事：① 硬失败进 `failures`（摘要会说"1 条有失败" ✓，按**条数**不是按发数 ✓）；
/// ② settle 不再说"可能还没记账"这种假安慰 ✗，而是明说"都没传上去" ✓；
/// ③ 结论是 **`Partial`**（不是 success ✗ —— 与 2026-10-03 用户报的
/// 「下面的 614 失败还直接说成功」同一口径 ✓）。
#[tokio::test]
async fn a_failed_upload_is_reported_not_dressed_up_as_a_delay() {
    let before = vec![task_json_with_process(
        522,
        "month",
        "当月上传文件满100个",
        "sign_in_3",
        42,
    )];
    // 事后读数**没动** —— 正是"看着像还没记账"的那种读数 ✗
    let after = vec![task_json_with_process(
        522,
        "month",
        "当月上传文件满100个",
        "sign_in_3",
        42,
    )];
    let mut replies = replies_for(&before, vec![]);
    replies.push(click_ok());
    for _ in 0..5 {
        // create 回一个**解不动**的信封 ⇒ 每一发都失败（真机那次是 `exist:null` ✓）
        replies.push(r#"{"code":"9999","success":false,"message":"上游说不行"}"#.to_string());
    }
    replies.extend(replies_for(&after, vec![]));

    let sink = Arc::new(CapturedSink::default());
    let (ctx, _http, _sink) = ctx_with_sink(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
        sink.clone(),
    );
    let result = run_app_task(&ctx, 0, &[522]).await.expect("不该是 Err");

    assert_eq!(result.steps[0].uploaded, 0, "一发都没成");
    assert_eq!(
        result.failures.len(),
        1,
        "1 条任务失败要记成 1 条（不是 5 发 = 5 条）: {:?}",
        result.failures
    );
    let summary = result.summary_line();
    assert!(summary.contains("1 条有失败"), "摘要必须说出来：{summary}");
    let lines = sink.lines();
    assert!(
        lines.iter().any(|l| l.contains("都没传上去")),
        "要说清是传不上去：{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("可能还没记账")),
        "不许再说那句假安慰：{lines:?}"
    );
    assert!(
        result.task_outcome().is_partial(),
        "有成功也有硬失败 ⇒ Partial，不是 success：{:?}",
        result.task_outcome()
    );
}

/// ⭐ 差额也要**说出来**：实测"我们传了 5 个、上游只 +4"（记账慢一步 ✓ —— run 105
/// `48 → 52`，而 run 106 的动手前读数正好是 **53** ✓）⇒ 日志不能只写 `+4` 就完事，
/// 否则两个数字看着像打架 ✗。
#[tokio::test]
async fn the_monthly_upload_explains_a_short_count() {
    let name = "当月上传文件满100个";
    let before = vec![task_json_with_process(522, "month", name, "sign_in_3", 42)];
    let after = vec![task_json_with_process(522, "month", name, "sign_in_3", 46)];
    let mut replies = replies_for(&before, vec![]);
    replies.push(click_ok());
    for _ in 0..5 {
        replies.push(create_new());
        replies.push("{}".to_string());
        replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    }
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string());
    replies.extend(replies_for(&after, vec![]));

    let sink = Arc::new(CapturedSink::default());
    let (ctx, _http, _sink) = ctx_with_sink(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
        sink.clone(),
    );
    let result = run_app_task(&ctx, 0, &[522]).await.expect("不该是 Err");
    assert_eq!(result.steps[0].uploaded, 5);
    assert_eq!(result.steps[0].progress_after.as_deref(), Some("46/100"));
    let lines = sink.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("这一趟 +4；我们实际传了 5 个，差的 1 个上游还没记账")),
        "差额要说清：{lines:?}"
    );
}

/// ⭐⭐ 运行级：113 那一发 `file/create` 必须带 **PC 档的头** ✓。
///
/// ⚠ 只断言 `uploaded:1` **证明不了**这条任务做成了：它奖励的是"用 PC 客户端"，
/// 按安卓档传一样会 `uploaded:1` ✗ ⇒ 必须逐枚头断到底：
/// PC 档有 `x-yun-app-channel: 10200153` + `market-source`/`module-type`/`svc-type` 三枚，
/// 而**没有** `x-huawei-channelSrc`、**没有** `x-yun-sub-op-type`（那两枚是安卓档的 ✗）。
#[tokio::test]
async fn task_113_sends_the_pc_channel_headers() {
    let items = vec![task_json(113, "month", "使用PC客户端", "sign_in_3")];
    let mut replies = replies_for(&items, vec![]);
    replies.push(click_ok());
    replies.push(create_new());
    replies.push("{}".to_string());
    replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 回收站
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[113]).await.expect("不该是 Err");

    assert_eq!(result.planned, 1, "113 该进 planned");
    assert_eq!(result.steps[0].uploaded, 1, "一趟一个");
    assert_eq!(result.acted, 1);
    assert!(
        http.clicks().iter().any(|c| c.contains("id=113")),
        "113 也要打卡（它是 TASK_LIST 里的条目 ✓）"
    );

    let create = http
        .requests()
        .into_iter()
        .find(|r| r.url.contains("/hcy/file/create"))
        .expect("该发过 file/create");
    assert_eq!(
        header(&create, "x-yun-app-channel").as_deref(),
        Some(mcloud_upstream::app_cloud_upload::CLOUD_CHANNEL_PC),
        "PC 档的渠道号"
    );
    assert_eq!(
        header(&create, "x-yun-market-source").as_deref(),
        Some("001")
    );
    assert_eq!(header(&create, "x-yun-module-type").as_deref(), Some("100"));
    assert_eq!(header(&create, "x-yun-svc-type").as_deref(), Some("1"));
    // ⚠ 这两枚是**安卓档**专有 ⇒ PC 档一个都不该出现（出现了就是档位没生效 ✗）
    assert!(
        header(&create, "x-huawei-channelSrc").is_none(),
        "PC 档没有这一枚：{:?}",
        header(&create, "x-huawei-channelSrc")
    );
    assert!(
        header(&create, "x-yun-sub-op-type").is_none(),
        "PC 档没有这一枚：{:?}",
        header(&create, "x-yun-sub-op-type")
    );
}

/// ⭐⭐ 运行级：615 真传 **10 张 `.jpg`** ✓（用户那条 `onlyTaskIds=615` 的离线复刻）。
///
/// 门是 `button.app.link`、形状是「最多 10 张 / process 到 10 停 / 每张 300ms / `.jpg`」✓
/// —— 四格任一写错，这条就红 ✓。
#[tokio::test]
async fn task_615_uploads_ten_jpg_files() {
    let items = vec![task_json_with_button(
        615,
        "beiyong1",
        "记录国庆节欢乐瞬间",
        "sign_in_3",
        "mcloud://main/uploadView",
        0,
    )];
    let mut replies = replies_for(&items, vec![]);
    replies.push(click_ok());
    for _ in 0..10 {
        replies.push(create_new());
        replies.push("{}".to_string());
        replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    }
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 回收站
                                                                                       // 事后那一趟整表：这一条 `process` 已经到 10 ✓
    let after = vec![task_json_with_button(
        615,
        "beiyong1",
        "记录国庆节欢乐瞬间",
        "sign_in_3",
        "mcloud://main/uploadView",
        10,
    )];
    replies.extend(replies_for(&after, vec![]));

    let sink = Arc::new(CapturedSink::default());
    let (ctx, http, _sink) = ctx_with_sink(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
        sink.clone(),
    );
    let result = run_app_task(&ctx, 0, &[615]).await.expect("不该是 Err");

    assert_eq!(result.planned, 1);
    assert_eq!(result.steps[0].uploaded, 10, "一次 10 张");
    assert_eq!(result.steps[0].task_id, Some(615));
    assert_eq!(result.acted, 1);
    assert_eq!(result.cleaned_up, 10, "这 10 张也要全收回");
    assert!(
        result.steps[0].problems.is_empty(),
        "{:?}",
        result.steps[0].problems
    );
    // ⚠⚠ 分母必须是**这一档自己的 10**，不是 522 那个 100 ✗
    //（2026-10-04 真机 run 110 就把 615 显示成了 `0/100` ✗ ⇒ 这一条就是那个 bug 的钉子 ✓）
    assert_eq!(result.steps[0].progress_total, Some(10));
    assert_eq!(result.steps[0].progress_before.as_deref(), Some("0/10"));
    assert_eq!(result.steps[0].progress_after.as_deref(), Some("10/10"));
    let lines = sink.lines();
    assert!(
        lines.iter().any(|l| l.contains("0/10 → 10/10（这一趟 +10")),
        "进度行要按这一档的分母：{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("/100")),
        "615 那一族**不该**出现 522 的分母：{lines:?}"
    );
    assert!(
        http.clicks().iter().any(|c| c.contains("id=615")),
        "615 也要打卡 ✓"
    );

    let creates: Vec<_> = http
        .requests()
        .into_iter()
        .filter(|r| r.url.contains("/hcy/file/create"))
        .collect();
    assert_eq!(creates.len(), 10, "一共 10 发 create");
    for c in &creates {
        let body = String::from_utf8_lossy(c.body.as_deref().unwrap_or_default());
        assert!(
            body.contains(".jpg"),
            "这一族刻意传 .jpg（旧实现 `ext: \".jpg\"` ✓）：{body}"
        );
    }
}

/// ⭐ `process` 已经走了一部分 ⇒ 只补 **10 − process** 张（旧 TS 那句 `remaining` ✓）。
///
/// 实拍 614（同族）就是 `process=6` ✓ ⇒ 补 4 张、不是 10 张 —— 写错会白传 6 个文件 ✗。
#[tokio::test]
async fn task_615_tops_up_only_the_remaining_pictures() {
    let items = vec![task_json_with_button(
        615,
        "beiyong1",
        "记录国庆节欢乐瞬间",
        "sign_in_3",
        "mcloud://main/uploadView",
        6,
    )];
    let mut replies = replies_for(&items, vec![]);
    replies.push(click_ok());
    for _ in 0..4 {
        replies.push(create_new());
        replies.push("{}".to_string());
        replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string());
    }
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string());
    let after = vec![task_json_with_button(
        615,
        "beiyong1",
        "记录国庆节欢乐瞬间",
        "sign_in_3",
        "mcloud://main/uploadView",
        10,
    )];
    replies.extend(replies_for(&after, vec![]));
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[615]).await.expect("不该是 Err");

    assert_eq!(result.steps[0].uploaded, 4, "6/10 ⇒ 只补 4 张");
    assert_eq!(result.steps[0].progress_before.as_deref(), Some("6/10"));
    assert_eq!(result.steps[0].progress_after.as_deref(), Some("10/10"));
    assert_eq!(result.acted, 1);
    assert_eq!(
        http.requests()
            .into_iter()
            .filter(|r| r.url.contains("/hcy/file/create"))
            .count(),
        4
    );
}

/// ⭐ 反面：**打卡门都没过**（`task/click` 回非 0）⇒ 更谈不上推进、整个 run 判 failed ✓。
///
/// 与上一条一起把这一档的三段读清：**外链成没成**（`shared`）、**打卡过没过**（`clicked`）
/// 都只是过程，`acted` 只认**次数涨没涨**（`share_counted`）✓。
#[tokio::test]
async fn the_share_task_that_fails_its_click_is_not_counted() {
    let items = vec![task_json_without_step(
        434,
        "month",
        "分享文件领好礼",
        "sign_in_3",
    )];
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![r#"{"code":1,"msg":"boom"}"#.to_string()]),
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let result = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");

    assert_eq!(http.clicks().len(), 1, "那一发照样发出去了（只是上游没认）");
    assert_eq!(result.planned, 1);
    assert!(!result.steps[0].clicked, "打卡门没过");
    assert_eq!(result.acted, 0, "没动过就不计");
    assert!(
        result.task_outcome().is_failure(),
        "一条没动 ⇒ failed：{:?}",
        result.task_outcome()
    );
}

// ── b) skip_tasks：本账号名单 ───────────────────────────────────────

/// ⭐ b) `skip_tasks` 含某 id ⇒ 该条目跳过；不含 ⇒ 处理。
#[tokio::test]
async fn b_configured_skip_tasks_are_skipped() {
    let items = vec![task_json(106, "new", "上传一个随机文件", "sign_in_3")];

    // 含 106 ⇒ 计划为空、一枪不发。
    let skip = AccountSettings {
        skip_tasks: vec![106],
        ..Default::default()
    };
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![]),
        beijing_noon(5),
        skip,
        Arc::new(FakeStore::new()),
    );
    let r = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");
    assert_eq!(r.planned, 0, "名单命中的条目不该进 planned");
    assert!(http.clicks().is_empty(), "名单命中的条目不该打卡");

    // 不含 106 ⇒ 照常处理（一发打卡 + 一次 create）。
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![click_ok(), create_exists()]),
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let r = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");
    assert_eq!(r.planned, 1);
    assert_eq!(http.clicks().len(), 1);
}

// ── c) 月份门：本账号当月有没有备份 ──────────────────────────────────

/// 月份门是逐字的合取（旧 TS `tasks.ts:86-95`），三态 + 20 号边界。
#[test]
fn the_month_gate_is_the_legacy_conjunction() {
    let signin_month = |market: &str| {
        let mut i = item(Some(106), Some("month"), Some("每月任务"));
        i.market_name = Some(market.to_string());
        i
    };
    let it = signin_month("sign_in_3");
    assert!(
        month_gate_skips(&it, Some(false), Some(5)),
        "未备份 + 5 号 ⇒ 跳"
    );
    assert!(
        !month_gate_skips(&it, Some(false), Some(20)),
        "20 号是边界，不跳"
    );
    assert!(!month_gate_skips(&it, Some(false), Some(21)));
    assert!(!month_gate_skips(&it, Some(true), Some(5)), "已备份不跳");
    assert!(!month_gate_skips(&it, None, Some(5)), "不知道就不跳");
    assert!(!month_gate_skips(&it, Some(false), None), "时钟读不出不跳");
    // 别的市场 / 别的组不适用。
    assert!(!month_gate_skips(
        &signin_month("newsign_139mail"),
        Some(false),
        Some(5)
    ));
    let mut other = item(Some(106), Some("new"), Some("每月任务"));
    other.market_name = Some("sign_in_3".to_string());
    assert!(!month_gate_skips(&other, Some(false), Some(5)));
}

/// 月份与日只从注入的时钟来（东八区）。
#[test]
fn the_month_and_day_come_from_the_injected_clock() {
    let (month, day) = beijing_month_day(&FixedClock(beijing_noon(5))).expect("该解得出来");
    assert_eq!(month, "202610");
    assert_eq!(day, 5);
}

/// ⭐ c) 接线：`cur_month_backup=false` 且日期 < 20 ⇒ 跳过；`=true` 或日期 ≥ 20 ⇒ 处理。
#[tokio::test]
async fn c_month_gate_reads_this_accounts_backup_state() {
    let items = vec![task_json(106, "month", "每月任务", "sign_in_3")];
    let seed = |backup: bool| {
        let store = Arc::new(FakeStore::new());
        store.seed(
            TEST_ACCOUNT,
            serde_json::json!({"cur_month_backup": {"month": "202610", "backup": backup}}),
        );
        store
    };

    // false + 5 号 ⇒ 跳过。
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![]),
        beijing_noon(5),
        AccountSettings::default(),
        seed(false),
    );
    let r = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");
    assert_eq!(r.month_backup, Some(false));
    assert_eq!(r.planned, 0, "未备份且 5 号 ⇒ 该跳");
    assert!(http.clicks().is_empty(), "跳过就不该打卡");

    // true + 5 号 ⇒ 处理。
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![click_ok(), create_exists()]),
        beijing_noon(5),
        AccountSettings::default(),
        seed(true),
    );
    let r = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");
    assert_eq!(r.month_backup, Some(true));
    assert_eq!(r.planned, 1, "已备份就不跳");
    assert_eq!(http.clicks().len(), 1);

    // false + 25 号 ⇒ 处理（过了 20 号那道限）。
    let (ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![click_ok(), create_exists()]),
        beijing_noon(25),
        AccountSettings::default(),
        seed(false),
    );
    let r = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");
    assert_eq!(r.planned, 1, "20 号之后不再跳");
    assert_eq!(http.clicks().len(), 1);

    // 没有记录 ⇒ 不跳（不知道 ≠ 没开）。
    let (ctx, _) = ctx_with(
        TEST_ACCOUNT,
        replies_for(&items, vec![click_ok(), create_exists()]),
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let r = run_app_task(&ctx, 0, &[]).await.expect("不该是 Err");
    assert_eq!(r.month_backup, None);
    assert_eq!(r.planned, 1, "没记录时不该跳");
}

/// ⭐ c) **隔离**：A 账号的"本月没备份"**不影响** B 账号。
#[tokio::test]
async fn month_state_is_isolated_per_account() {
    let store = Arc::new(FakeStore::new());
    store.seed(
        "A",
        serde_json::json!({"cur_month_backup": {"month": "202610", "backup": false}}),
    );

    let (ctx_a, _) = ctx_with("A", vec![], 0, AccountSettings::default(), store.clone());
    let (ctx_b, _) = ctx_with("B", vec![], 0, AccountSettings::default(), store.clone());

    assert_eq!(read_month_backup(&ctx_a, "202610").await, Some(false));
    assert_eq!(
        read_month_backup(&ctx_b, "202610").await,
        None,
        "B 没记录，绝不该被 A 的状态影响"
    );
    // 月份对不上同样当"没记录"：上个月的 false 不许套到本月。
    assert_eq!(read_month_backup(&ctx_a, "202611").await, None);
}

/// 落盘是**读-改-写**：只动自己那一枚键，别把 `device_id` 这类别人的键抹掉。
#[tokio::test]
async fn saving_the_month_reading_keeps_other_account_state() {
    let store = Arc::new(FakeStore::new());
    store.seed("A", serde_json::json!({"device_id": "DEV-1"}));
    let (ctx, _) = ctx_with("A", vec![], 0, AccountSettings::default(), store.clone());

    save_month_backup(&ctx, "202610", false)
        .await
        .expect("写不该失败");

    let state = store.state_of("A");
    assert_eq!(state["device_id"], "DEV-1", "别人的键必须原样留着");
    assert_eq!(state["cur_month_backup"]["month"], "202610");
    assert_eq!(state["cur_month_backup"]["backup"], false);
}

/// ⚠ 用户 2026-10-04 报「app-task 存在 bug，无法正常使用」——
/// 现场是从**落盘的运行日志**里看到的（这正是新做的"回看日志"第一次派上用场 ✓）：
///
/// ```text
/// [2/3] 做任务 分享文件领好礼<span id='share_title'>0/7</span>
/// ```
///
/// 上游把 HTML 塞进了任务名 ✗ ⇒ 这一条钉住"进记录之前清干净" ✓。
#[test]
fn task_name_from_upstream_has_its_html_stripped() {
    // 真实样本（原样抄自运行 #81 的日志 ✓）
    assert_eq!(
        super::clean_task_name("分享文件领好礼<span id=\'share_title\'>0/7</span>"),
        "分享文件领好礼0/7"
    );
    // 标签里有属性、有嵌套、且标签后面还跟文字
    assert_eq!(
        super::clean_task_name("手动上传一个文件 <b>每天</b> 都能做"),
        "手动上传一个文件 每天 都能做"
    );
    // 没有标签时**原样保留**（别把正常名字改坏 ✗）
    assert_eq!(super::clean_task_name("分享文件领好礼"), "分享文件领好礼");
    // 未闭合的 `<` 别把后面整段吃掉（宁可留着，也别静默丢内容 ✗）
    assert_eq!(super::clean_task_name("a < b 上传"), "a < b 上传");
}

/// ⭐ `task/click` 的 `key` 是 **`task`**，不是任务分组名。
///
/// # 为什么这条要紧
///
/// 2026-10-04 用户实测 106：那趟记录是 `{uploaded:1, clicked:true, problems:[]}` ✓，
/// 可上游 `state` **始终是 `WAIT`** ✗ —— 打卡发的是 `key=day`（分组名 ✗），
/// 而参考实现默认发 `task`（`…/app/helpers.ts:10` 的 `taskStr ?? "task"`）。
/// 从前**没有任何测试**钉这一格 ⇒ 错了很久没人发现 ✓。
#[test]
fn click_key_is_task_not_a_group_name() {
    assert_eq!(click_key(Some(106)), "task");
    assert_eq!(click_key(Some(522)), "task");
    assert_eq!(click_key(Some(434)), "task");
    assert_eq!(click_key(None), "task", "没 id 也走默认值（不发 key=空）");
    assert_eq!(click_key(Some(409)), "task2", "409 按参考实现给专用值");

    // 真请求上也带着它
    let req = mcloud_upstream::app_task_click::task_click_request(click_key(Some(106)), 106)
        .expect("构造打卡请求");
    assert!(
        req.url.contains("key=task&id=106"),
        "打卡 URL 必须是 key=task：{}",
        req.url
    );
}

/// 409 / 1005 走「固定入口」那一档 —— 从前这两条**什么都不做** ✗。
///
/// 依据：`runners.ts:98-108`（409）与 `:126-133`（1005），形状相同：
/// `page/doTask` 埋点 → `sleep(500)` → `clickTask`（409 带 `task2` ✓，1005 用默认 key ✓）。
#[test]
fn entry_tasks_get_the_entry_step() {
    for id in [409_i64, 1005] {
        assert_eq!(
            step_for(&item(
                Some(id),
                Some("time"),
                Some("从固定入口访问云朵中心")
            )),
            Some(Step::Entry),
            "id={id}"
        );
    }
}

// ── 复审 tk-2 / tk-3（2026-10-04）：上传产物的回收不能因为一次失败或取消就漏 ──

/// 让某一发请求在**传输层**失败（`Scripted` 只会回 200，没有失败档）。
struct FailOn {
    inner: Arc<Scripted>,
    needle: &'static str,
}

#[async_trait]
impl HttpClient for FailOn {
    async fn request(&self, req: mcloud_core::HttpRequest) -> Result<HttpResponse> {
        if req.url.contains(self.needle) {
            return Err(mcloud_core::Error::Other("测试夹具：让这一发失败".into()));
        }
        self.inner.request(req).await
    }
}

/// 打到 `needle` 的那一发**之后**按下取消 —— 模拟"跑到一半用户点了中止"。
struct CancelOn {
    inner: Arc<Scripted>,
    needle: &'static str,
    cancel: CancelToken,
}

#[async_trait]
impl HttpClient for CancelOn {
    async fn request(&self, req: mcloud_core::HttpRequest) -> Result<HttpResponse> {
        let hit = req.url.contains(self.needle);
        let out = self.inner.request(req).await;
        if hit {
            self.cancel.cancel();
        }
        out
    }
}

/// `file/create` 成功、随后的 PUT 挂掉 ⇒ **那个文件仍要进回收站**。
///
/// 红测：把 `upload_one_file` 里那句提前落账（`owned.push`）挪回调用方
/// （只有 `Ok(Some(id))` 才记）⇒ 这一条不再发 `batchTrash`，红。
#[tokio::test]
async fn a_file_whose_put_failed_is_still_reclaimed() {
    let items = vec![task_json(106, "month", "上传图片", "sign_in_3")];
    let mut replies = replies_for(&items, vec![]);
    replies.push(click_ok()); // 106 打卡
    replies.push(create_new()); // create 成了 ⇒ 拿到 SYNTH-FID
                                // PUT 之后的 complete 不会发（PUT 就挂了）；这一发是收尾的 batchTrash
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string());

    let (mut ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    ctx.http = Arc::new(FailOn {
        inner: Arc::clone(&http),
        needle: "synth.example.invalid",
    });

    let result = run_app_task(&ctx, 0, &[])
        .await
        .expect("这一趟本身不该是 Err");

    assert_eq!(
        result.cleaned_up, 1,
        "create 已经拿到 fileId ⇒ 即使 PUT 挂了也必须回收（否则文件永久留在用户网盘）"
    );
    let reqs = http.requests();
    let trash = reqs
        .iter()
        .find(|r| r.url.contains("batchTrash"))
        .expect("必须发 batchTrash");
    assert!(
        trash.body_str().unwrap_or_default().contains("SYNTH-FID"),
        "回收的应当是我们造的那个 id"
    );
    assert!(
        result.steps[0]
            .problems
            .iter()
            .any(|p| p.contains("上传失败")),
        "失败要如实记：{:?}",
        result.steps[0].problems
    );
}

/// **上传途中被取消**，已经传上去的文件仍要被回收。
///
/// 这一趟有两条（106 在前、113 在后）：106 传完一个文件后按下取消 ⇒ 轮到 113
/// 时那道 `check_cancelled` 命中。旧写法是 `.await?` ⇒ 取消直接窜出函数、
/// **绕过链尾的清理**，106 传上去的文件就永远留在用户网盘里。
///
/// 红测：把 `run_app_task` 里两处取消检查换回 `?` ⇒ 这一条不再发 `batchTrash`，红。
#[tokio::test]
async fn cancelling_mid_run_still_reclaims_what_was_uploaded() {
    let items = vec![
        task_json(106, "month", "上传图片", "sign_in_3"),
        task_json(113, "month", "使用PC客户端", "sign_in_3"),
    ];
    let mut replies = replies_for(&items, vec![]);
    replies.push(click_ok()); // 106 打卡
    replies.push(create_new()); // 106 create
    replies.push("{}".to_string()); // 106 PUT
    replies.push(r#"{"code":"0000","success":true,"message":"请求成功"}"#.to_string()); // complete ⇒ 在这里取消
    replies.push(r#"{"code":"0000","success":true,"message":"success"}"#.to_string()); // 收尾 batchTrash

    let (mut ctx, http) = ctx_with(
        TEST_ACCOUNT,
        replies,
        beijing_noon(5),
        AccountSettings::default(),
        Arc::new(FakeStore::new()),
    );
    let token = ctx.cancel.clone();
    ctx.http = Arc::new(CancelOn {
        inner: Arc::clone(&http),
        needle: "file/complete",
        cancel: token,
    });

    let err = run_app_task(&ctx, 0, &[])
        .await
        .expect_err("取消要如实报出去，不能假装跑完了");
    assert!(
        matches!(err, mcloud_core::Error::Cancelled),
        "报出来的应当是取消本身，实得：{err:?}"
    );

    let reqs = http.requests();
    assert!(
        reqs.iter().any(|r| r.url.contains("batchTrash")
            && r.body_str().unwrap_or_default().contains("SYNTH-FID")),
        "取消**不是**不回收的理由：已经传上去的文件必须被移进回收站"
    );
    assert!(
        !reqs.iter().any(|r| r.url.contains("id=113")),
        "取消之后 113 一条都不该动手"
    );
}
