//! `JsonStore` 的测试。
//!
//! 重点在**并发**：这个后端的核心风险是"并发写互相覆盖"，
//! 而那种 bug 在单线程测试里永远不会出现。

use super::*;
use mcloud_core::{
    AccountSettings, AccountStore, AuthInfo, DeviceId, LogLevel, LogStore, PipelineStore, RunStore,
    ScheduleStore,
};

fn tmpdir(tag: &str) -> PathBuf {
    // 用 pid + 纳秒，避免并行测试之间抢同一个目录
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("mcloud-store-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn account(phone: &str) -> Account {
    Account {
        id: AccountId::new(phone),
        nickname: None,
        auth: AuthInfo {
            raw: "raw".into(),
            platform: "android".into(),
            phone: phone.into(),
            token: "t".into(),
            expire: 0,
        },
        device_id: DeviceId("dev".into()),
        settings: AccountSettings::default(),
    }
}

fn run_record(id: u64, status: RunStatus) -> RunRecord {
    RunRecord {
        run_id: RunId::new(id),
        task: "sign".into(),
        account: "13800000000".into(),
        started_at_ms: 0,
        duration_ms: 1,
        outcome: mcloud_core::TaskOutcome::success("ok"),
        status,
        details: serde_json::Value::Null,
    }
}

// ── 基础 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn accounts_upsert_roundtrip() {
    let d = tmpdir("acct");
    let s = JsonStore::open(&d).unwrap();
    assert!(s.accounts().await.unwrap().is_empty());

    s.save_account(&account("13800000001")).await.unwrap();
    s.save_account(&account("13800000002")).await.unwrap();
    assert_eq!(s.accounts().await.unwrap().len(), 2);

    let mut a = account("13800000001");
    a.nickname = Some("小明".into());
    s.save_account(&a).await.unwrap();

    let all = s.accounts().await.unwrap();
    assert_eq!(all.len(), 2, "同 id 再存是更新而非追加");
    assert_eq!(
        all.iter()
            .find(|x| x.id.as_str() == "13800000001")
            .unwrap()
            .nickname
            .as_deref(),
        Some("小明")
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn removing_account_also_drops_its_state() {
    let d = tmpdir("rmstate");
    let s = JsonStore::open(&d).unwrap();
    let id = AccountId::new("13800000001");
    s.save_account(&account("13800000001")).await.unwrap();
    s.save_account_state(&id, &serde_json::json!({"deviceId":"abc"}))
        .await
        .unwrap();

    s.remove_account(&id).await.unwrap();
    // 残留状态会让同号再次添加时继承旧 deviceId
    assert!(
        s.account_state(&id).await.unwrap().is_null(),
        "删账号必须同时清掉它的运行期状态"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn data_survives_reopen() {
    let d = tmpdir("reopen");
    {
        let s = JsonStore::open(&d).unwrap();
        s.save_account(&account("13800000001")).await.unwrap();
        s.upsert_run(&run_record(1, RunStatus::Done)).await.unwrap();
        s.append_logs(
            RunId::new(1),
            &[LogLine::new(1, LogLevel::Info, "第一行", 0, Seq(1))],
        )
        .await
        .unwrap();
    }
    // 新实例：必须从磁盘恢复出全部内容
    let s2 = JsonStore::open(&d).unwrap();
    assert_eq!(s2.accounts().await.unwrap().len(), 1);
    assert_eq!(s2.runs(10).await.unwrap().len(), 1);
    assert_eq!(s2.logs(RunId::new(1), None).await.unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

// ── 并发：这是本后端最容易出错的地方 ────────────────────────────────────

/// 并发保存**不同**账号，一条都不能丢。
///
/// 这是"读-改-写"式实现的经典失败场景：若没有锁，
/// 两个任务同时保存会各自基于同一份旧快照写回，丢掉对方的新增。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_saves_of_different_accounts_lose_nothing() {
    let d = tmpdir("conc-acct");
    let s = std::sync::Arc::new(JsonStore::open(&d).unwrap());

    const N: usize = 64;
    let mut handles = Vec::new();
    for i in 0..N {
        let s = std::sync::Arc::clone(&s);
        handles.push(tokio::spawn(async move {
            s.save_account(&account(&format!("1380000{i:04}")))
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(
        s.accounts().await.unwrap().len(),
        N,
        "并发保存 {N} 个账号后必须是 {N} 条 —— 少了就是丢更新"
    );

    // 磁盘上也必须一致（内存对了、盘上少了同样算 bug）
    let reopened = JsonStore::open(&d).unwrap();
    assert_eq!(reopened.accounts().await.unwrap().len(), N);
    let _ = std::fs::remove_dir_all(&d);
}

/// 并发追加日志，一行都不能丢、seq 也不能乱。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_log_appends_keep_every_line() {
    let d = tmpdir("conc-log");
    let s = std::sync::Arc::new(JsonStore::open(&d).unwrap());
    s.upsert_run(&run_record(1, RunStatus::Running))
        .await
        .unwrap();

    const WRITERS: usize = 8;
    const PER_WRITER: usize = 20;
    let mut handles = Vec::new();
    for w in 0..WRITERS {
        let s = std::sync::Arc::clone(&s);
        handles.push(tokio::spawn(async move {
            for i in 0..PER_WRITER {
                let seq = (w * PER_WRITER + i) as u64 + 1;
                s.append_logs(
                    RunId::new(1),
                    &[LogLine::new(
                        1,
                        LogLevel::Info,
                        format!("w{w}-{i}"),
                        0,
                        Seq(seq),
                    )],
                )
                .await
                .unwrap();
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let lines = s.logs(RunId::new(1), None).await.unwrap();
    assert_eq!(
        lines.len(),
        WRITERS * PER_WRITER,
        "并发追加的日志必须一条不少"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 同一个 run 被并发 upsert（开始写 running、结束写 done），不得产生重复记录。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_upsert_same_run_does_not_duplicate() {
    let d = tmpdir("conc-upsert");
    let s = std::sync::Arc::new(JsonStore::open(&d).unwrap());

    let mut handles = Vec::new();
    for i in 0..20 {
        let s = std::sync::Arc::clone(&s);
        handles.push(tokio::spawn(async move {
            let st = if i % 2 == 0 {
                RunStatus::Running
            } else {
                RunStatus::Done
            };
            s.upsert_run(&run_record(1, st)).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let runs = s.runs(10).await.unwrap();
    assert_eq!(runs.len(), 1, "同 run_id 的并发 upsert 只能有一条记录");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 运行记录 ────────────────────────────────────────────────────────────

#[tokio::test]
async fn runs_are_newest_first_and_upsert_updates_in_place() {
    let d = tmpdir("runs");
    let s = JsonStore::open(&d).unwrap();
    s.upsert_run(&run_record(1, RunStatus::Done)).await.unwrap();
    s.upsert_run(&run_record(2, RunStatus::Done)).await.unwrap();

    let runs = s.runs(10).await.unwrap();
    assert_eq!(runs[0].run_id, RunId::new(2), "最近的应排最前");

    // 回填同一条：位置不变、内容更新
    let mut r1 = run_record(1, RunStatus::Running);
    r1.outcome = mcloud_core::TaskOutcome::failed("炸了");
    s.upsert_run(&r1).await.unwrap();
    let runs = s.runs(10).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert!(s
        .run(RunId::new(1))
        .await
        .unwrap()
        .unwrap()
        .outcome
        .is_failure());
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn history_is_capped_and_logs_are_evicted_with_it() {
    let d = tmpdir("cap");
    let s = JsonStore::open(&d).unwrap();
    for i in 1..=(HISTORY_LIMIT as u64 + 10) {
        s.upsert_run(&run_record(i, RunStatus::Done)).await.unwrap();
        s.append_logs(
            RunId::new(i),
            &[LogLine::new(i, LogLevel::Info, "x", 0, Seq(1))],
        )
        .await
        .unwrap();
    }
    assert_eq!(s.runs(usize::MAX).await.unwrap().len(), HISTORY_LIMIT);
    // 被淘汰的那条的日志也要一起没，否则日志文件无限堆积
    assert!(
        s.logs(RunId::new(1), None).await.unwrap().is_empty(),
        "运行记录被淘汰后，它的日志必须一并清理"
    );
    assert!(!s.log_path(RunId::new(1)).exists(), "日志文件本体也该删掉");
    let _ = std::fs::remove_dir_all(&d);
}

/// 进程被杀后留下的 `running` 记录，重开时必须被标记为已中止。
#[tokio::test]
async fn orphaned_running_records_become_cancelled_on_reopen() {
    let d = tmpdir("orphan");
    {
        let s = JsonStore::open(&d).unwrap();
        s.upsert_run(&run_record(1, RunStatus::Running))
            .await
            .unwrap();
        s.upsert_run(&run_record(2, RunStatus::Done)).await.unwrap();
    }
    let s2 = JsonStore::open(&d).unwrap();
    let r1 = s2.run(RunId::new(1)).await.unwrap().unwrap();
    assert_eq!(
        r1.status,
        RunStatus::Cancelled,
        "进程没了，那条永远 running 的记录必须收尾，否则前端一直显示进行中"
    );
    assert_eq!(
        s2.run(RunId::new(2)).await.unwrap().unwrap().status,
        RunStatus::Done
    );
    assert!(s2.running_runs().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

// ── 日志 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn logs_support_incremental_fetch_by_seq() {
    let d = tmpdir("seq");
    let s = JsonStore::open(&d).unwrap();
    let lines: Vec<LogLine> = (1..=5)
        .map(|i| LogLine::new(1, LogLevel::Info, format!("l{i}"), 0, Seq(i)))
        .collect();
    s.append_logs(RunId::new(1), &lines).await.unwrap();

    assert_eq!(s.logs(RunId::new(1), None).await.unwrap().len(), 5);
    // 断线重连：已收到 seq<=3，只该拿到 4、5
    let rest = s.logs(RunId::new(1), Some(Seq::seen(3))).await.unwrap();
    assert_eq!(rest.len(), 2, "续传只给新的");
    assert_eq!(rest[0].seq, Seq(4));
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn logs_are_trimmed_at_limit_keeping_newest() {
    let d = tmpdir("trim");
    let s = JsonStore::open(&d).unwrap();
    let many: Vec<LogLine> = (1..=(LOG_LIMIT_PER_RUN as u64 + 50))
        .map(|i| LogLine::new(1, LogLevel::Info, format!("l{i}"), 0, Seq(i)))
        .collect();
    let outcome = s.append_logs(RunId::new(1), &many).await.unwrap();
    assert_eq!(outcome, AppendOutcome::Trimmed, "超出上限要报告被截断");

    let kept = s.logs(RunId::new(1), None).await.unwrap();
    assert_eq!(kept.len(), LOG_LIMIT_PER_RUN);
    // 保留的是**最新**的：最后一条应在，第一条应被丢弃
    assert_eq!(kept.last().unwrap().seq, Seq(LOG_LIMIT_PER_RUN as u64 + 50));
    assert!(kept[0].seq > Seq(1), "丢的应是最旧的");
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn empty_log_append_is_a_noop() {
    let d = tmpdir("emptylog");
    let s = JsonStore::open(&d).unwrap();
    assert_eq!(
        s.append_logs(RunId::new(1), &[]).await.unwrap(),
        AppendOutcome::Appended
    );
    assert!(s.logs(RunId::new(1), None).await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

// ── 定时与编排 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn schedules_crud() {
    let d = tmpdir("sched");
    let s = JsonStore::open(&d).unwrap();
    let sch = Schedule {
        id: ScheduleId::new("s1"),
        name: "每天签到".into(),
        task: "sign".into(),
        accounts: vec!["13800000000".into()],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    };
    s.save_schedule(&sch).await.unwrap();
    assert_eq!(s.schedules().await.unwrap().len(), 1);
    assert!(s.schedule(&ScheduleId::new("s1")).await.unwrap().is_some());

    s.remove_schedule(&ScheduleId::new("s1")).await.unwrap();
    assert!(s.schedules().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn pipelines_crud() {
    use mcloud_core::{AccountMode, PipelineStep};
    let d = tmpdir("pipe");
    let s = JsonStore::open(&d).unwrap();
    let p = Pipeline {
        id: PipelineId::new("p1"),
        name: "早班".into(),
        accounts: vec![],
        account_mode: AccountMode::Sequential,
        stop_on_error: true,
        steps: vec![PipelineStep {
            task: "sign".into(),
            body: serde_json::Value::Null,
            continue_on_error: false,
        }],
        enabled: true,
        last_run_ms: None,
    };
    s.save_pipeline(&p).await.unwrap();
    assert_eq!(s.pipelines().await.unwrap().len(), 1);

    s.remove_pipeline(&PipelineId::new("p1")).await.unwrap();
    assert!(s.pipelines().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

// ── 损坏数据的处理 ──────────────────────────────────────────────────────

#[tokio::test]
async fn corrupt_file_reports_filename_not_just_parse_error() {
    let d = tmpdir("corrupt");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(files::ACCOUNTS), "{ 这不是 JSON").unwrap();

    let err = JsonStore::open(&d).unwrap_err().to_string();
    assert!(
        err.contains(files::ACCOUNTS),
        "报错必须带上是哪个文件坏了，否则用户只能猜：{err}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn empty_file_is_treated_as_empty_not_corrupt() {
    let d = tmpdir("empty");
    std::fs::create_dir_all(&d).unwrap();
    // 半途崩溃可能留下空文件；把它当"没有数据"而不是"数据损坏"
    std::fs::write(d.join(files::ACCOUNTS), "").unwrap();
    let s = JsonStore::open(&d).unwrap();
    assert!(s.accounts().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}
