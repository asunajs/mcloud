//! App 任务矩阵任务层的测试。
//!
//! 钉两类东西：
//!
//! * **聚合口径**（`merge_pages` / `state_counts` / `disabled_count`）—— 纯函数。
//!   "按 `name` 去重保留先来的"与"`name` 缺席的各自保留"这两条最容易被顺手写错。
//! * **只读与容错**（请求数、失败登记、全挂才失败）—— 这一条链今天
//!   **一个写请求都不发**，而它上游那两口写（`task/click` 与上传三枪）都不是能撤的。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

fn item(id: Option<i64>, name: Option<&str>) -> TaskV2Item {
    TaskV2Item {
        task_id: id,
        name: name.map(str::to_string),
        ..Default::default()
    }
}

fn page(items: Vec<TaskV2Item>) -> TaskListV2 {
    TaskListV2 {
        group: "g".into(),
        items,
        group_finish_states: Vec::new(),
    }
}

/// 一个 group 的正常响应（一条任务）。
fn ok_body(id: i64) -> String {
    format!(
        r#"{{"code":0,"msg":"success","groupState":{{"new":{{"taskFinishState":2}}}},
           "result":{{"new":[{{"id":{id},"name":"任务{id}","groupid":"new","state":"WAIT",
           "limitType":"DAY","enable":1,"currstep":0,"process":0,"marketname":"sign_in_3"}}]}}}}"#
    )
}

/// 构造十六份"哪个第几次失败"的答复。
fn replies_with_failures(fail_at: &[usize]) -> Vec<Reply> {
    (0..app_task_matrix_fetches())
        .map(|i| {
            if fail_at.contains(&i) {
                Reply::Fail
            } else {
                Reply::Body(ok_body(i as i64 + 1))
            }
        })
        .collect()
}

// ══════════════════════════ 聚合（纯函数） ══════════════════════════

/// 按 `id` **升序**排。
#[test]
fn items_are_sorted_by_id() {
    let (items, _) = merge_pages(&[page(vec![
        item(Some(7), Some("丙")),
        item(Some(2), Some("甲")),
        item(Some(5), Some("乙")),
    ])]);
    assert_eq!(
        items.iter().map(|t| t.task_id).collect::<Vec<_>>(),
        vec![Some(2), Some(5), Some(7)]
    );
}

/// `id` 缺席的排**最后**（且彼此保持原顺序 —— 稳定排序）。
#[test]
fn items_without_an_id_sort_last_and_keep_their_order() {
    let (items, _) = merge_pages(&[page(vec![
        item(None, Some("无甲")),
        item(Some(3), Some("有")),
        item(None, Some("无乙")),
    ])]);
    let names: Vec<Option<&str>> = items.iter().map(|t| t.name.as_deref()).collect();
    assert_eq!(names, vec![Some("有"), Some("无甲"), Some("无乙")]);
}

/// 按 `name` 去重，**保留先来的那条**（先"编号大的"排前面时，去重后留下的仍按 id 序）。
#[test]
fn duplicate_names_keep_the_first_after_sorting() {
    // 两页里都出现「同名」，但 id 不同 ⇒ 排序后 id 小的在前，去重留下它
    let (items, raw) = merge_pages(&[
        page(vec![item(Some(9), Some("同名"))]),
        page(vec![item(Some(4), Some("同名"))]),
    ]);
    assert_eq!(raw, 2, "去重前的条数要留着");
    assert_eq!(items.len(), 1, "同名只留一条");
    assert_eq!(
        items[0].task_id,
        Some(4),
        "留下 id 小的那条（排序后先来的）"
    );
}

/// ⚠ `name` **缺席**的条目**各自保留**，不合并成一条。
///
/// 旧 TS 会把它们全去重成一条 —— 那会把条目数报少，
/// 而"上游没给名字"与"两条同名"是两回事。
#[test]
fn entries_without_a_name_are_all_kept() {
    let (items, raw) = merge_pages(&[page(vec![
        item(Some(1), None),
        item(Some(2), None),
        item(Some(3), None),
    ])]);
    assert_eq!(raw, 3);
    assert_eq!(items.len(), 3, "没名字的条目不许被折成一条");
}

/// `raw_count` 是**去重之前**的总数（十六次加起来）。
#[test]
fn raw_count_is_measured_before_dedup() {
    let (items, raw) = merge_pages(&[
        page(vec![item(Some(1), Some("甲")), item(Some(2), Some("乙"))]),
        page(vec![item(Some(1), Some("甲"))]),
    ]);
    assert_eq!(raw, 3);
    assert_eq!(items.len(), 2);
}

/// `state` 分布：已完成 / 未完成 / **读不出**三档。
///
/// ⚠ "读不出"单独一档 —— 折进"未完成"的表现是将来接写链时**重复上报**。
#[test]
fn state_counts_keep_unknown_separate() {
    let m = AppTaskMatrix {
        items: vec![
            TaskV2Item {
                state: Some("FINISH".into()),
                ..Default::default()
            },
            TaskV2Item {
                state: Some("WAIT".into()),
                ..Default::default()
            },
            TaskV2Item {
                state: Some("SOMETHING_NEW".into()),
                ..Default::default()
            },
            TaskV2Item {
                state: None,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(m.state_counts(), (1, 1, 2));
}

/// `enable != 1` 的条目数，`None`（上游没给）也算进来。
#[test]
fn disabled_count_counts_anything_that_is_not_one() {
    let m = AppTaskMatrix {
        items: vec![
            TaskV2Item {
                enable: Some(1),
                ..Default::default()
            },
            TaskV2Item {
                enable: Some(0),
                ..Default::default()
            },
            TaskV2Item {
                enable: None,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        m.disabled_count(),
        2,
        "0 与「没给」都过不了 `enable != 1` 那道门"
    );
}

// ══════════════════════════ 小结 ══════════════════════════

/// 小结念出"拉成几次 / 去重前几条 / 去重后几条"。
#[test]
fn the_summary_reports_the_fetch_and_dedup_counts() {
    let result = AppTaskMatrixResult {
        verdict: MatrixVerdict::Read,
        matrix: AppTaskMatrix {
            items: vec![item(Some(1), Some("任务一"))],
            raw_count: 3,
            succeeded: 16,
            ..Default::default()
        },
        error: String::new(),
    };
    let s = result.summary_line();
    assert!(s.contains("16/16"), "实得：{s}");
    assert!(s.contains("3 条去重后 1 条"), "实得：{s}");
    assert!(s.contains("只读"), "实得：{s}");
}

/// 部分失败要在小结里说出来 —— 不然"少了几组"会被读成"那几组没任务"。
#[test]
fn partial_failures_are_announced() {
    let result = AppTaskMatrixResult {
        verdict: MatrixVerdict::Read,
        matrix: AppTaskMatrix {
            succeeded: 14,
            failures: vec![
                FetchFailure {
                    market_name: "sign_in_3".into(),
                    group: "time".into(),
                    error: "网络断了".into(),
                },
                FetchFailure {
                    market_name: "sign_in_3".into(),
                    group: "day".into(),
                    error: "网络断了".into(),
                },
            ],
            ..Default::default()
        },
        error: String::new(),
    };
    let s = result.summary_line();
    assert!(s.contains("14/16"), "实得：{s}");
    assert!(s.contains("2 组没读到"), "实得：{s}");
}

/// 小结里**不出现**任务名与奖励文案。
#[test]
fn the_summary_never_prints_upstream_copy() {
    let result = AppTaskMatrixResult {
        verdict: MatrixVerdict::Read,
        matrix: AppTaskMatrix {
            items: vec![TaskV2Item {
                name: Some("制作童趣3D玩偶".into()),
                description: Some("永葆童心 领38云朵".into()),
                ..Default::default()
            }],
            succeeded: 16,
            ..Default::default()
        },
        error: String::new(),
    };
    let s = result.summary_line();
    assert!(!s.contains("制作童趣3D玩偶"), "实得：{s}");
    assert!(
        !s.contains("领38云朵"),
        "奖励文案会随改名漂移，不进小结：{s}"
    );
}

/// 零值档位是 **Failed**：一次都没拉成时不许念出一句"矩阵 0 条"。
#[test]
fn the_zero_verdict_is_failed() {
    assert_eq!(MatrixVerdict::default(), MatrixVerdict::Failed);
    let zero = AppTaskMatrixResult::default().summary_line();
    assert!(zero.contains("没读成"), "实得：{zero}");
    assert!(
        !zero.contains("0 条去重后"),
        "零值不该看起来像一份正常读数：{zero}"
    );
}

// ══════════════════════════ 只读与容错 ══════════════════════════

/// 十六次请求、**全是 POST 到同一个只读端点**，没有任何写路径。
#[tokio::test]
async fn the_run_sends_sixteen_read_requests() {
    let (out, http, _) = run_with(replies_with_failures(&[])).await;
    assert!(matches!(out, TaskOutcome::Success { .. }), "实得：{out:?}");
    assert_eq!(http.count(), 16, "2 个市场 × 8 个 group");
    for r in &http.requests() {
        assert_eq!(r.method, "POST");
        assert_eq!(
            r.url,
            "https://m.mcloud.139.com/ycloud/signin/task/taskListV2"
        );
        for forbidden in ["task/click", "file/create", "batchTrash", "upload"] {
            assert!(!r.url.contains(forbidden), "不许出现写路径：{}", r.url);
        }
    }
}

/// 两个市场名都真的用上了，且**先云盘、后邮箱**（顺序是行为的一部分）。
#[tokio::test]
async fn both_markets_are_queried_in_order() {
    let (_, http, _) = run_with(replies_with_failures(&[])).await;
    let markets: Vec<String> = http
        .requests()
        .iter()
        .filter_map(|r| {
            let v: Value = serde_json::from_str(r.body_str()?).ok()?;
            Some(v["marketname"].as_str()?.to_string())
        })
        .collect();
    assert_eq!(markets.len(), 16);
    assert_eq!(markets[0], "sign_in_3");
    assert!(
        markets[..8].iter().all(|m| m == "sign_in_3"),
        "前八次是云盘那一支：{markets:?}"
    );
    assert!(
        markets[8..].iter().all(|m| m == "newsign_139mail"),
        "后八次是邮箱那一支：{markets:?}"
    );
}

/// ⚠ **一次失败不算失败**：某个 group 挂了仍算成功，且失败被登记。
#[tokio::test]
async fn one_failed_group_is_recorded_not_fatal() {
    let (out, http, _) = run_with(replies_with_failures(&[3, 9])).await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("部分失败不该让整趟失败：{out:?}");
    };
    assert_eq!(http.count(), 16, "挂了也要把十六次都走完");
    assert!(summary.contains("14/16"), "实得：{summary}");
    assert!(summary.contains("2 组没读到"), "实得：{summary}");
}

/// ⚠⚠ **十六次全挂** ⇒ 交回 `Err`。
///
/// 旧 TS 那种"全失败仍回一张空表"会让"全没读到"看起来像"这个号没任务"。
#[tokio::test]
async fn all_fetches_failing_is_an_error() {
    let all: Vec<usize> = (0..app_task_matrix_fetches()).collect();
    let (ctx, http, _) = ctx_with(replies_with_failures(&all));
    let err = AppTaskMatrixTask
        .run(&ctx, &serde_json::json!({}))
        .await
        .expect_err("全挂要报错");
    assert!(err.to_string().contains("全部失败"), "实得：{err}");
    assert_eq!(http.count(), 16, "确实试满了十六次");
}

/// 某一次返回非 0 码，同样按"这一组失败"登记（不中断）。
#[tokio::test]
async fn a_non_zero_code_counts_as_a_failed_group() {
    let mut replies = replies_with_failures(&[]);
    replies[0] = Reply::Body(r#"{"code":500,"msg":"系统繁忙"}"#.into());
    let (out, _, _) = run_with(replies).await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(summary.contains("15/16"), "实得：{summary}");
}

// ══════════════════════════ 明细 ══════════════════════════

/// 逐条读数进明细（小结刻意不念任务名，那一份只能走这里）。
#[tokio::test]
async fn the_items_and_group_readings_are_reported_as_details() {
    let (out, _, sink) = run_with(replies_with_failures(&[])).await;
    assert!(matches!(out, TaskOutcome::Success { .. }));

    let details = sink.details();
    assert_eq!(details.len(), 1);
    let d = &details[0];
    assert_eq!(d["kind"], "app-task-matrix");
    assert_eq!(d["verdict"], "read");
    assert_eq!(d["fetches"], 16);
    assert_eq!(d["succeeded"], 16);
    assert_eq!(d["dedupedCount"], 16, "十六组各一条、名字各不相同");
    assert_eq!(d["stateCounts"]["waiting"], 16);
    assert_eq!(d["items"].as_array().map(|a| a.len()), Some(16));
    assert_eq!(d["items"][0]["state"], "WAIT");
    assert_eq!(d["items"][0]["groupId"], "new");
    assert_eq!(d["groups"].as_array().map(|a| a.len()), Some(16));
    assert_eq!(d["groups"][0]["taskFinishState"], 2);
    assert_eq!(d["failures"].as_array().map(|a| a.len()), Some(0));
}

// ══════════════════════════ 注册表接入 ══════════════════════════

/// ⭐ `app-task-matrix` 真的注册进了 registry。
///
/// ⚠ 它与 `app-task`（"做任务"，写）**是两个 code，不是一回事**：
/// 这一条"只看"，而两条**必须同时存在**且各自独立。
#[test]
fn app_task_matrix_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");

    let metas = registry.list();
    let meta = metas
        .iter()
        .find(|m| m.name == "app-task-matrix")
        .expect("app-task-matrix 必须注册");
    assert_eq!(meta.title, "App 任务矩阵（只看）");
    assert!(meta.params.is_empty());
    assert!(!meta.hidden);
    // ⚠ 这条断言**以前是反的**（那时写的那一半没实现，所以断言 `app-task`
    // 不该出现）。写侧落地之后它必须**存在**，而且与只读这条**分开**：
    // 拿一个 code 做两件事会让它在两处指不同的东西。
    let write = metas
        .iter()
        .find(|m| m.name == "app-task")
        .expect("写的那一半（app-task）必须注册");
    assert_ne!(write.title, meta.title, "两条的标题必须分得开");
    assert_eq!(write.title, "App 任务（做任务）");
}

/// 一枪的答复：给个 body，或者**打挂**（网络层错误）。
#[derive(Clone)]
enum Reply {
    Body(String),
    Fail,
}

/// 按顺序答复的假 HTTP 客户端（用尽后重复最后一个）。
struct Scripted {
    seen: Mutex<Vec<HttpRequest>>,
    replies: Mutex<Vec<Reply>>,
}

impl Scripted {
    fn new(replies: Vec<Reply>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            // 反转，便于从队尾取
            replies: Mutex::new(replies.into_iter().rev().collect()),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.requests().len()
    }
}

#[async_trait]
impl HttpClient for Scripted {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse> {
        self.seen.lock().unwrap().push(req);
        let reply = {
            let mut r = self.replies.lock().unwrap();
            if r.len() > 1 {
                r.pop().unwrap()
            } else {
                r.last().cloned().unwrap_or(Reply::Body("{}".into()))
            }
        };
        match reply {
            Reply::Body(body) => Ok(HttpResponse {
                status: 200,
                body,
                headers: Vec::new(),
            }),
            Reply::Fail => Err(mcloud_core::Error::Other("网络断了".into())),
        }
    }
}

struct FixedClock(i64);
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

#[derive(Default)]
struct CollectSink {
    events: Mutex<Vec<RunEvent>>,
}

impl CollectSink {
    /// 取回所有 `Details` 事件的 payload。
    fn details(&self) -> Vec<serde_json::Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                RunEvent::Details { payload, .. } => Some(payload.clone()),
                _ => None,
            })
            .collect()
    }
}

impl EventSink for CollectSink {
    fn emit(&self, event: RunEvent) {
        self.events.lock().unwrap().push(event);
    }
}

fn account() -> Account {
    Account {
        id: mcloud_core::AccountId::new("19****01"),
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

/// 一个只带 `http` 的 ctx，外加能回读的 sink。**时钟是入参** ——
/// 活动期是判据的一部分，测试必须能把它挪到任意时刻。
fn ctx_with_at(replies: Vec<Reply>, now_ms: i64) -> (TaskContext, Arc<Scripted>, Arc<CollectSink>) {
    let http = Arc::new(Scripted::new(replies));
    let sink = Arc::new(CollectSink::default());
    let ctx = TaskContext {
        account: account(),
        http: http.clone(),
        store: Arc::new(NoStore),
        clock: Arc::new(FixedClock(now_ms)),
        sink: sink.clone(),
        run_id: mcloud_core::RunId(1),
        cancel: CancelToken::new(),
        chain_cancel: None,
        artifacts: mcloud_engine::ArtifactRecorder::new(
            Arc::new(mcloud_engine::ArtifactLedger::new()),
            mcloud_core::RunId(1),
            mcloud_core::AccountId::new("19****01"),
        ),
        details: mcloud_engine::DetailsRecorder::new(
            mcloud_engine::DetailsLedger::new(),
            mcloud_core::RunId(1),
        ),
    };
    (ctx, http, sink)
}

/// 跑一趟；本链不依赖时钟，给 0 即可。
async fn run_with(replies: Vec<Reply>) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    let (ctx, http, sink) = ctx_with_at(replies, 0);
    let out = AppTaskMatrixTask.run(&ctx, &serde_json::json!({})).await;
    (out.expect("不该是 Err"), http, sink)
}

/// 同上，但把 `Err` 也交回（用来钉"全挂才失败"那一档）。
fn ctx_with(replies: Vec<Reply>) -> (TaskContext, Arc<Scripted>, Arc<CollectSink>) {
    ctx_with_at(replies, 0)
}

/// 这一条链**不该碰 store** —— 一碰就报错的替身。
struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("这条链不该碰 store".into())
}

#[async_trait]
impl mcloud_core::AccountStore for NoStore {
    async fn accounts(&self) -> Result<Vec<Account>> {
        Err(refuse())
    }
    async fn account(&self, _id: &mcloud_core::AccountId) -> Result<Option<Account>> {
        Err(refuse())
    }
    async fn save_account(&self, _a: &Account) -> Result<()> {
        Err(refuse())
    }
    async fn remove_account(&self, _id: &mcloud_core::AccountId) -> Result<()> {
        Err(refuse())
    }
    async fn account_state(&self, _id: &mcloud_core::AccountId) -> Result<serde_json::Value> {
        Err(refuse())
    }
    async fn save_account_state(
        &self,
        _id: &mcloud_core::AccountId,
        _s: &serde_json::Value,
    ) -> Result<()> {
        Err(refuse())
    }
}

#[async_trait]
impl mcloud_core::RunStore for NoStore {
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
impl mcloud_core::LogStore for NoStore {
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
impl mcloud_core::ScheduleStore for NoStore {
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
impl mcloud_core::PipelineStore for NoStore {
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
impl mcloud_core::UserStore for NoStore {
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
