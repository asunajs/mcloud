//! 任务拓展任务层的测试。
//!
//! 钉两类东西：
//!
//! * **判据**（`should_upload` / `should_claim` / `judged`）—— 纯函数。
//!   最要紧的是 `None`（上游没给那一格）**不等于** `false`：
//!   那是"要不要往用户网盘写一个文件"的分界。
//! * **清理只删我们造的**（`cleanup_uploaded`）—— 撞同名那一档 `fileId` 是
//!   **用户本来就有的文件**，把它放进清理名单就是删用户的东西。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use mcloud_core::{
    Account, AccountSettings, AuthInfo, CancelToken, Clock, DeviceId, EventSink, HttpClient,
    HttpResponse, RunEvent,
};

const DEVICE: &str = "B-abc";

/// 一份"备份没开、记录数 0"的读数（⇒ 要凑、不领）。
const NEEDS_BACKUP: &str =
    r#"{"code":0,"msg":"success","result":{"curMonthBackup":false,"curMonthTaskRecordCount":0}}"#;
/// 一份"备份已开、没记录"的读数（⇒ 什么都不做）。
const BACKUP_ON: &str =
    r#"{"code":0,"msg":"success","result":{"curMonthBackup":true,"curMonthTaskRecordCount":0}}"#;
/// 一份"备份已开、有记录"的读数（⇒ 去领）。
const HAS_RECORDS: &str =
    r#"{"code":0,"msg":"success","result":{"curMonthBackup":true,"curMonthTaskRecordCount":3}}"#;
/// 实拍那一份（**没给 `acceptDate`**）。
const CAPTURED: &str = r#"{"code":0,"msg":"success","result":{
    "curMonthBackup":true,"curMonthBackupTaskAccept":false,
    "curMonthTaskRecordCount":0,"nextMonthTaskRecordCount":358,"preMonthBackup":false}}"#;

fn read_of(json: &str) -> TaskExpansion {
    proto::parse_task_expansion(json).unwrap()
}

/// `file/create` 的成功响应（字符串 `"0000"` + `data` 载荷）。
fn create_body(file_id: &str, exist: bool, rapid: bool) -> String {
    serde_json::json!({
        "success": true, "code": "0000", "message": "ok",
        "data": {
            "fileId": file_id, "uploadId": "U-1",
            "exist": exist, "rapidUpload": rapid,
            "partInfos": [{"uploadUrl": "https://up.example/put"}],
        }
    })
    .to_string()
}

const COMPLETE_BODY: &str = r#"{"success":true,"code":"0000","message":"ok","data":{}}"#;
const TRASH_BODY: &str = r#"{"success":true,"code":"0000","message":"ok"}"#;

fn plan() -> ExpansionPlan {
    ExpansionPlan {
        name: "probe.bin".into(),
        content: b"hello".to_vec(),
        backup_wait_ms: 0,
    }
}

// ══════════════════════════ 判据（纯函数） ══════════════════════════

/// `cur_month_backup == Some(false)` 才凑 —— **严格判**。
#[test]
fn should_upload_is_strict_about_false() {
    assert!(should_upload(&read_of(NEEDS_BACKUP)));
    assert!(!should_upload(&read_of(BACKUP_ON)));
    // ⚠ 没读到**不凑**：把"上游没说"读成"上游说了没开"就是拿一次写去补空洞
    let missing = read_of(r#"{"code":0,"msg":"ok","result":{"curMonthTaskRecordCount":0}}"#);
    assert!(!should_upload(&missing), "None ≠ false");
}

/// `cur_month_task_record_count > 0` 才领；`None` **不领**。
#[test]
fn should_claim_requires_a_positive_count() {
    assert!(should_claim(&read_of(HAS_RECORDS)));
    assert!(!should_claim(&read_of(BACKUP_ON)), "0 不领");
    let missing = read_of(r#"{"code":0,"msg":"ok","result":{"curMonthBackup":true}}"#);
    assert!(!should_claim(&missing), "None 也不领");
}

/// 两格都没读到 ⇒ **判不了**（一个请求都不发）。
#[test]
fn judged_needs_at_least_one_criterion() {
    assert!(judged(&read_of(NEEDS_BACKUP)));
    assert!(judged(&read_of(
        r#"{"code":0,"msg":"ok","result":{"curMonthBackup":true}}"#
    )));
    assert!(judged(&read_of(
        r#"{"code":0,"msg":"ok","result":{"curMonthTaskRecordCount":0}}"#
    )));
    assert!(!judged(&read_of(r#"{"code":0,"msg":"ok","result":{}}"#)));
}

/// 实拍那一份逐字解出来（五格、`acceptDate` 缺）。
#[test]
fn the_captured_reading_is_parsed() {
    let r = read_of(CAPTURED);
    assert_eq!(r.cur_month_backup, Some(true));
    assert_eq!(r.cur_month_backup_task_accept, Some(false));
    assert_eq!(r.cur_month_task_record_count, Some(0));
    assert_eq!(r.next_month_task_record_count, Some(358));
    assert_eq!(r.pre_month_backup, Some(false));
    assert_eq!(r.accept_date, None);
    // 这一份 → 不凑、不领
    assert!(!should_upload(&r));
    assert!(!should_claim(&r));
}

/// "一枪没发"那一档**不等**；其余发过的档都等。
#[test]
fn only_the_never_sent_buckets_skip_the_wait() {
    assert!(!BackupUpload::NotNeeded.needs_wait());
    assert!(!BackupUpload::Blocked.needs_wait());
    for u in [
        BackupUpload::Done,
        BackupUpload::Rapid,
        BackupUpload::Exists,
        BackupUpload::Failed,
    ] {
        assert!(u.needs_wait(), "发过的档要等服务端记账：{u:?}");
    }
}

// ══════════════════════════ 判不了：一枪不发 ══════════════════════════

/// 两格判据都没读到 ⇒ 只发了读那一枪，**一个写请求都没有**。
#[tokio::test]
async fn unreadable_sends_no_write_request() {
    let (out, http, _) = run_with(vec![Reply::Body(
        r#"{"code":0,"msg":"ok","result":{"nextMonthTaskRecordCount":358}}"#.into(),
    )])
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("判不了"), "实得：{reason}");
    assert_eq!(http.count(), 1, "判不了就不该发任何写请求");
}

// ══════════════════════════ 不凑的那些 ══════════════════════════

/// 备份已开、没记录 ⇒ **只读一枪**，不凑也不领。
#[tokio::test]
async fn backup_already_on_does_nothing() {
    let (out, http, _) = run_with(vec![Reply::Body(BACKUP_ON.into())]).await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 1);
    assert!(reason.contains("没领"), "实得：{reason}");
    assert!(
        reason.contains("curMonthBackup=true"),
        "要照抄读数：{reason}"
    );
    assert!(
        reason.contains("curMonthTaskRecordCount=0"),
        "实得：{reason}"
    );
}

/// 备份已开、有记录 ⇒ 读一枪 + 领一枪，**不凑**。
#[tokio::test]
async fn claims_without_uploading_when_backup_is_on() {
    let (out, http, _) = run_with(vec![
        Reply::Body(HAS_RECORDS.into()),
        Reply::Body(r#"{"code":0,"msg":"success","result":100}"#.into()),
    ])
    .await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 2, "读一枪 + 领一枪");
    assert!(
        http.url_at(1).contains("receiveTaskExpansion"),
        "实得：{}",
        http.url_at(1)
    );
    assert!(!http
        .requests()
        .iter()
        .any(|r| r.url.contains("file/create")));
    assert!(summary.contains("领到 100"), "实得：{summary}");
}

/// ⚠ 领取成功但上游**没报数** ⇒ 仍然报"已领"，不是一句都不念。
#[tokio::test]
async fn a_claim_without_a_count_still_reports_claimed() {
    let (out, _, _) = run_with(vec![
        Reply::Body(HAS_RECORDS.into()),
        Reply::Body(r#"{"code":0,"msg":"success"}"#.into()),
    ])
    .await;
    let TaskOutcome::Success { summary } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(summary.contains("已领"), "实得：{summary}");
    assert!(summary.contains("没报数"), "实得：{summary}");
}

/// 领取那一发失败 ⇒ `Failed`（请求发过、没成，是实况）。
#[tokio::test]
async fn a_failed_claim_is_reported_as_failed() {
    let (out, _, _) = run_with(vec![Reply::Body(HAS_RECORDS.into()), Reply::Fail]).await;
    assert!(matches!(out, TaskOutcome::Failed { .. }), "实得：{out:?}");
}

// ══════════════════════════ 凑备份那一支 ══════════════════════════

/// 换不到 deviceId ⇒ **一枪都不发**（如实挡下），但**仍然重读**。
#[tokio::test]
async fn a_missing_device_id_blocks_the_upload_but_still_rereads() {
    let (out, http, _) = run_with_device(
        vec![
            Reply::Body(NEEDS_BACKUP.into()),
            Reply::Body(NEEDS_BACKUP.into()),
        ],
        None,
    )
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 2, "读一枪 + 重读一枪；**没有**上传三枪");
    assert!(!http
        .requests()
        .iter()
        .any(|r| r.url.contains("file/create")));
    assert!(reason.contains("deviceId"), "实得：{reason}");
    assert!(reason.contains("一枪没发"), "实得：{reason}");
}

/// 三枪走完 ⇒ `Done`，且链尾**把我们造的那个文件删掉**。
#[tokio::test]
async fn a_successful_backup_is_trashed_afterwards() {
    let (out, http, _) = run_with(vec![
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Body(create_body("F-1", false, false)),
        Reply::Body(String::new()), // PUT 的响应体不参与判定（只看状态码）
        Reply::Body(COMPLETE_BODY.into()),
        Reply::Body(NEEDS_BACKUP.into()), // 重读：备份仍 false
        Reply::Body(TRASH_BODY.into()),
    ])
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(http.count(), 6, "读 + 三枪上传 + 重读 + 删");
    // 第二枪是 PUT，且带的是**原始字节**
    let put = &http.requests()[2];
    assert_eq!(put.method, "PUT");
    assert_eq!(put.body.as_deref(), Some(b"hello".as_slice()));
    // 删除那一枪只带我们造的那个 id
    let reqs = http.requests();
    let trash = reqs.last().expect("有删除那一枪");
    assert!(trash.url.contains("batchTrash"), "实得：{}", trash.url);
    assert!(trash.body_str().unwrap().contains("F-1"));
    assert!(reason.contains("传了一个文件"), "实得：{reason}");
    assert!(
        reason.contains("已移进回收站") || reason.contains("回收站"),
        "实得：{reason}"
    );
    assert!(
        reason.contains("本月备份仍读成 false") || reason.contains("备份"),
        "凑完仍未开要说出来：{reason}"
    );
}

/// ⚠⚠ **撞同名那一档绝不删**：那个 `fileId` 是用户本来就有的文件。
#[tokio::test]
async fn a_name_collision_is_never_trashed() {
    let (out, http, _) = run_with(vec![
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Body(create_body("USER-OWN-FILE", true, false)),
        Reply::Body(NEEDS_BACKUP.into()), // 重读
    ])
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(
        !http.requests().iter().any(|r| r.url.contains("batchTrash")),
        "撞同名时**不许**发删除：那是用户的文件"
    );
    assert_eq!(http.count(), 3, "读 + create + 重读");
    assert!(
        !reason.contains("USER-OWN-FILE"),
        "用户那个文件的 id 连念都不该念：{reason}"
    );
    assert!(reason.contains("撞上同名"), "实得：{reason}");
}

/// 秒传 ⇒ 后两枪不发，但**删那一枪照发**（文件确实是我们造的）。
#[tokio::test]
async fn a_rapid_upload_skips_put_and_complete_but_is_still_trashed() {
    let (out, http, _) = run_with(vec![
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Body(create_body("F-RAPID", false, true)),
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Body(TRASH_BODY.into()),
    ])
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert_eq!(
        http.count(),
        4,
        "读 + create + 重读 + 删；PUT/complete 不发"
    );
    assert!(!http.requests().iter().any(|r| r.method == "PUT"));
    assert!(http.requests().iter().any(|r| r.url.contains("batchTrash")));
    assert!(reason.contains("秒传"), "实得：{reason}");
}

/// `create` 之后挂了 ⇒ `Failed`，但**只要占了坑就仍要删**。
#[tokio::test]
async fn a_failed_put_still_cleans_up_the_claimed_file() {
    let (out, http, _) = run_with(vec![
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Body(create_body("F-FAIL", false, false)),
        Reply::Fail, // PUT 挂
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Body(TRASH_BODY.into()),
    ])
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(
        http.requests().iter().any(|r| r.url.contains("batchTrash")),
        "create 已占坑 ⇒ 失败路径也要删"
    );
    assert!(reason.contains("没传成"), "实得：{reason}");
}

/// 清理那一枪失败 ⇒ 把原文带出来（否则网盘里留了个不知名的文件）。
#[tokio::test]
async fn a_failed_cleanup_surfaces_its_reason() {
    let (out, _, _) = run_with(vec![
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Body(create_body("F-1", false, false)),
        Reply::Body(String::new()),
        Reply::Body(COMPLETE_BODY.into()),
        Reply::Body(NEEDS_BACKUP.into()),
        Reply::Fail,
    ])
    .await;
    let TaskOutcome::Skipped { reason } = &out else {
        panic!("实得：{out:?}");
    };
    assert!(reason.contains("清理失败"), "实得：{reason}");
    assert!(reason.contains("网络断了"), "要带原文：{reason}");
}

/// 读数原样进明细，且**上传的 id 也进去**（将来对账用）。
#[tokio::test]
async fn the_readings_are_reported_as_details() {
    let (_, _, sink) = run_with(vec![
        Reply::Body(HAS_RECORDS.into()),
        Reply::Body(r#"{"code":0,"msg":"success","result":100}"#.into()),
    ])
    .await;
    let details = sink.details();
    assert_eq!(details.len(), 1);
    let d = &details[0];
    assert_eq!(d["kind"], "task-expansion");
    assert_eq!(d["verdict"], "claimed");
    assert_eq!(d["cloudCount"], 100);
    assert_eq!(d["first"]["curMonthBackup"], true);
    assert_eq!(d["first"]["curMonthTaskRecordCount"], 3);
    assert_eq!(d["cleanup"], "not-needed");
}

/// 零值档位是 `NotAttempted`：一句"领到 N 颗"不会在**一个请求都没发**的那趟上念出来。
#[test]
fn the_zero_verdict_never_claims_a_prize() {
    assert_eq!(ExpansionVerdict::default(), ExpansionVerdict::NotAttempted);
    let zero = ExpansionResult::default().summary_line();
    assert!(!zero.contains("领到"), "实得：{zero}");
    assert!(zero.contains("没办成"), "实得：{zero}");
}

// ══════════════════════════ 注册表接入 ══════════════════════════

/// ⭐ `task-expansion` 真的注册进了 registry。
#[test]
fn task_expansion_is_registered_with_the_right_meta() {
    let mut registry = mcloud_engine::Registry::new();
    crate::register_all(&mut registry).expect("注册不该重名");
    let metas = registry.list();
    let meta = metas
        .iter()
        .find(|m| m.name == "task-expansion")
        .expect("task-expansion 必须注册");
    assert_eq!(meta.title, "任务拓展");
    assert!(meta.params.is_empty());
    assert!(!meta.hidden);
}

/// 最多凑**一次**（这一格不是装饰：谁改成循环，这条会红）。
#[test]
fn at_most_one_backup_upload_is_allowed() {
    assert_eq!(TASK_EXPANSION_MAX_UPLOADS, 1);
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

impl Scripted {
    /// 第 n 次请求的 URL（本模块要用它断言打到了哪一口）。
    fn url_at(&self, n: usize) -> String {
        self.requests()[n].url.clone()
    }
}

/// 跑一趟（带 deviceId —— 这一族每一枪都要它）。
async fn run_with(replies: Vec<Reply>) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    run_with_device(replies, Some(DEVICE)).await
}

/// 同上，但 deviceId 可指定（用来钉"换不到就不发"那一档）。
async fn run_with_device(
    replies: Vec<Reply>,
    device: Option<&str>,
) -> (TaskOutcome, Arc<Scripted>, Arc<CollectSink>) {
    let (ctx, http, sink) = ctx_with_at(replies, 0);
    // ⚠ 不等待（`backup_wait_ms: 0`）：真睡 20 秒的测试既慢又看不出语义。
    let out = run_expansion(&ctx, device, &plan()).await;
    (out.expect("不该是 Err").task_outcome(), http, sink)
}

/// 这一条链**不该碰 store** —— 一碰就报错的替身。
struct NoStore;

fn refuse() -> mcloud_core::Error {
    mcloud_core::Error::Other("task-expansion 这条链不该碰 store".into())
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
