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

// ══════════════════════════ 只读与容错 ══════════════════════════
//
// ⚠ 这一节原来打的是那条**只读任务**（`AppTaskMatrixTask`）。2026-10-07 用户口径
// 「app-task-matrix 这个任务本身就没有意义」⇒ 任务摘掉了，但**这段循环本身**还在
//（`fetch_all` ✓，`app-task` 开头那一趟与收尾复查那一趟都走它 ✓）
// ⇒ 测试改成直接打 [`fetch_all`]：行为一条没少 ✓。

/// 十六次请求、**全是 POST 到同一个只读端点**，没有任何写路径。
#[tokio::test]
async fn fetch_all_sends_sixteen_read_requests() {
    let (ctx, http, _) = ctx_with(replies_with_failures(&[]));
    let (pages, failures) = fetch_all(&ctx).await.expect("不该是 Err");
    assert_eq!(pages.len(), 16, "2 个市场 × 8 个 group");
    assert!(failures.is_empty(), "实得：{failures:?}");
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
    let (ctx, http, _) = ctx_with(replies_with_failures(&[]));
    let _ = fetch_all(&ctx).await.expect("不该是 Err");
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

/// ⚠ **一次失败不算失败**：某个 group 挂了只登记，其余照拉。
#[tokio::test]
async fn one_failed_group_is_recorded_not_fatal() {
    let (ctx, http, _) = ctx_with(replies_with_failures(&[3, 9]));
    let (pages, failures) = fetch_all(&ctx).await.expect("部分失败不该是 Err");
    assert_eq!(http.count(), 16, "挂了也要把十六次都走完");
    assert_eq!(pages.len(), 14, "16 - 2");
    assert_eq!(failures.len(), 2);
    assert!(
        failures[0].contains("读取 sign_in_3/"),
        "失败要说清是哪一组：{failures:?}"
    );
}

/// ⚠⚠ **十六次全挂** ⇒ `pages` 空、`failures` 十六条。
///
/// ⚠ "全挂算不算错"**由调用方判**（这一层只负责如实回话 ✓）——
/// 旧 TS 那种"全失败仍回一张空表"会让"全没读到"看起来像"这个号没任务" ✗。
#[tokio::test]
async fn all_fetches_failing_is_reported_not_hidden() {
    let all: Vec<usize> = (0..app_task_matrix_fetches()).collect();
    let (ctx, http, _) = ctx_with(replies_with_failures(&all));
    let (pages, failures) = fetch_all(&ctx).await.expect("这一层不该自己判错");
    assert!(pages.is_empty(), "一次都没拉到");
    assert_eq!(failures.len(), 16, "十六组全要登记");
    assert_eq!(http.count(), 16, "确实试满了十六次");
}

/// 某一次返回非 0 码，同样按"这一组失败"登记（不中断）。
#[tokio::test]
async fn a_non_zero_code_counts_as_a_failed_group() {
    let mut replies = replies_with_failures(&[]);
    replies[0] = Reply::Body(r#"{"code":500,"msg":"系统繁忙"}"#.into());
    let (ctx, _, _) = ctx_with(replies);
    let (pages, failures) = fetch_all(&ctx).await.expect("不该是 Err");
    assert_eq!(pages.len(), 15, "只挂一组");
    assert_eq!(failures.len(), 1);
}

/// 这张表转成明细 JSON 的形状 —— **字段名是对外契约**（界面上那份清单按它画 ✓）。
#[test]
fn items_json_keeps_the_field_names_the_ui_reads() {
    let mut it = item(Some(106), Some("手动上传一个文件"));
    it.group_id = Some("day".into());
    it.state = Some("FINISH".into());
    it.market_name = Some("sign_in_3".into());
    it.curr_step = Some(3);
    it.button_app_link = Some("mcloud://main/uploadView".into());
    let js = items_json(&[it]);
    // `buttonProperty.out.text` —— 界面照抄上游那句话的那一格 ✓（2026-10-11 加 ✓）。
    assert!(
        js[0].get("buttonOutText").is_some(),
        "`items_json` 必须报出 `buttonOutText`（界面靠它显示上游自己的文案 ✓）"
    );
    assert_eq!(js[0]["taskId"], 106);
    assert_eq!(js[0]["groupId"], "day");
    assert_eq!(js[0]["state"], "FINISH");
    assert_eq!(js[0]["currStep"], 3);
    assert_eq!(js[0]["buttonAppLink"], "mcloud://main/uploadView");
    // ⚠ 状态分布同一份口径 ✓：`WAIT` 计未完成、没给状态算"读不出"（**分开** ✓）。
    let mut waiting = item(Some(1), Some("a"));
    waiting.state = Some("WAIT".into());
    let counts = state_counts_json(&[waiting, item(Some(2), Some("b"))]);
    assert_eq!(counts["waiting"], 1);
    assert_eq!(counts["unknown"], 1, "没给状态的那条不该被算成未完成 ✗");
}

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

/// 造一个上下文；本层不依赖时钟，给 0 即可。
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

/// `SmidStore` 替身：默认"这个 store 不管 smid"。
///
/// 2026-10-03 起 `Store` 把 `SmidStore` 并成了 supertrait（任务层要能读
/// 这个账号**已换回来**的那个 `deviceId` —— 它和登录时那份不是同一个东西）。
/// 这些替身要证明的事与 smid 无关，所以读一律 `None`、写一律拒绝。
#[async_trait]
impl mcloud_core::SmidStore for NoStore {
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
