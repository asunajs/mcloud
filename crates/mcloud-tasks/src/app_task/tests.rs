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
    let http = Arc::new(Scripted::new(replies));
    let mut acc = account(id);
    acc.settings = settings;
    let ctx = TaskContext {
        account: acc,
        http: http.clone(),
        store,
        clock: Arc::new(FixedClock(now_ms)),
        sink: Arc::new(NullSink),
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
    (ctx, http)
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
    let result = run_app_task(&ctx, 0).await.expect("不该是 Err");

    let clicks = http.clicks();
    assert_eq!(clicks.len(), 1, "只有 118 该被打卡，实得 {clicks:?}");
    assert!(clicks[0].contains("id=118"), "打卡的是 {}", clicks[0]);
    assert!(!clicks[0].contains("id=117"));
    assert_eq!(result.planned, 1, "黑名单条目不该进 planned");
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
    let r = run_app_task(&ctx, 0).await.expect("不该是 Err");
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
    let r = run_app_task(&ctx, 0).await.expect("不该是 Err");
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
    let r = run_app_task(&ctx, 0).await.expect("不该是 Err");
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
    let r = run_app_task(&ctx, 0).await.expect("不该是 Err");
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
    let r = run_app_task(&ctx, 0).await.expect("不该是 Err");
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
    let r = run_app_task(&ctx, 0).await.expect("不该是 Err");
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
