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
        pc_platform: None,
        user_domain_id: None,
        owner: None,
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
        owner: None,
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
        owner: None,
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

// ── 窄写：只改该改的列 ──────────────────────────────────────────────────

/// 水位线窄写**不能**碰用户改过的字段。
///
/// 调度器手里那行是几秒前读的；整行覆盖会把用户在这期间通过 WebUI
/// 改过的名字/参数一起抹掉。
#[tokio::test]
async fn mark_schedule_fired_only_touches_watermark_and_count() {
    let d = tmpdir("narrow-fire");
    let s = JsonStore::open(&d).unwrap();
    let mut sch = Schedule {
        id: ScheduleId::new("s1"),
        name: "原名".into(),
        task: "sign".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        owner: None,
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    };
    s.save_schedule(&sch).await.unwrap();

    // 模拟：调度器读到这行之后，用户改了个名字
    sch.name = "用户改的新名字".into();
    s.save_schedule(&sch).await.unwrap();

    // 调度器拿着**旧快照**（名字是"原名"）推水位
    let mut stale = sch.clone();
    stale.name = "原名".into();
    s.mark_schedule_fired(&stale.id, 1_700_000_000_000)
        .await
        .unwrap();

    let after = s.schedule(&ScheduleId::new("s1")).await.unwrap().unwrap();
    assert_eq!(after.name, "用户改的新名字", "窄写不能把名字改回去");
    assert_eq!(after.last_fire_ms, Some(1_700_000_000_000));
    assert_eq!(after.run_count, 1);
    let _ = std::fs::remove_dir_all(&d);
}

/// `record_schedule_run` 的两个 `None` 语义**不对称**，这是刻意的。
#[tokio::test]
async fn record_schedule_run_has_asymmetric_none_semantics() {
    let d = tmpdir("narrow-record");
    let s = JsonStore::open(&d).unwrap();
    s.save_schedule(&Schedule {
        id: ScheduleId::new("s1"),
        name: "x".into(),
        task: "sign".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        owner: None,
        last_fire_ms: Some(1),
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 1,
    })
    .await
    .unwrap();

    // 一次真跑：记时间 + ok，并清掉可能的旧错误
    s.record_schedule_run(&ScheduleId::new("s1"), Some(999), "ok", None)
        .await
        .unwrap();
    let a = s.schedule(&ScheduleId::new("s1")).await.unwrap().unwrap();
    assert_eq!(a.last_run_ms, Some(999));
    assert_eq!(a.last_status.as_deref(), Some("ok"));
    assert_eq!(a.last_error, None);

    // 一次被弃掉的错过：`run_ms = None` ⇒ **不动**上次运行时间
    s.record_schedule_run(&ScheduleId::new("s1"), None, "skipped", Some("错过了"))
        .await
        .unwrap();
    let b = s.schedule(&ScheduleId::new("s1")).await.unwrap().unwrap();
    assert_eq!(
        b.last_run_ms,
        Some(999),
        "这一趟没跑，不该把上次真实运行时间抹掉"
    );
    assert_eq!(b.last_status.as_deref(), Some("skipped"));

    // 成功时 `error = None` ⇒ **显式清空**（不是"不动"）
    s.record_schedule_run(&ScheduleId::new("s1"), Some(1000), "ok", None)
        .await
        .unwrap();
    let c = s.schedule(&ScheduleId::new("s1")).await.unwrap().unwrap();
    assert_eq!(
        c.last_error, None,
        "成功必须清掉旧错误，否则界面上 ok 旁边挂着一条旧错误"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn narrow_writes_on_missing_row_are_noops_not_errors() {
    let d = tmpdir("narrow-missing");
    let s = JsonStore::open(&d).unwrap();
    // 定时在这期间被删了：不是错误，只是没得写
    s.mark_schedule_fired(&ScheduleId::new("gone"), 1)
        .await
        .unwrap();
    s.record_schedule_run(&ScheduleId::new("gone"), Some(1), "ok", None)
        .await
        .unwrap();
    s.mark_pipeline_ran(&PipelineId::new("gone"), 1)
        .await
        .unwrap();
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn mark_pipeline_ran_only_touches_run_time() {
    let d = tmpdir("narrow-pipe");
    let s = JsonStore::open(&d).unwrap();
    use mcloud_core::{AccountMode, PipelineStep};
    let p = Pipeline {
        id: PipelineId::new("p1"),
        name: "原编排".into(),
        accounts: vec![],
        account_mode: AccountMode::Sequential,
        stop_on_error: true,
        steps: vec![PipelineStep {
            task: "sign".into(),
            body: serde_json::Value::Null,
            continue_on_error: false,
        }],
        enabled: true,
        owner: None,
        last_run_ms: None,
    };
    s.save_pipeline(&p).await.unwrap();

    // 编排跑了（可能几分钟），期间用户改了名字
    let mut edited = p.clone();
    edited.name = "用户改过的".into();
    s.save_pipeline(&edited).await.unwrap();

    s.mark_pipeline_ran(&PipelineId::new("p1"), 555)
        .await
        .unwrap();
    let after = s.pipeline(&PipelineId::new("p1")).await.unwrap().unwrap();
    assert_eq!(after.name, "用户改过的", "窄写不能抹掉用户的编辑");
    assert_eq!(after.last_run_ms, Some(555));
    let _ = std::fs::remove_dir_all(&d);
}

// ── 权限 ────────────────────────────────────────────────────────────────
//
// `accounts.json` 里是凭证原文，落盘必须是 0600。这类测试在 CI（Linux）
// 上才有意义，所以整组用 cfg(unix) 门控。

/// 新写入的凭证文件权限必须是 0600。
#[cfg(unix)]
#[tokio::test]
async fn accounts_file_is_created_with_0600() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-new");
    let s = JsonStore::open(&d).unwrap();
    s.save_account(&account("13800000000")).await.unwrap();

    let p = d.join("accounts.json");
    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "凭证文件权限应为 0600，实际 {:o} —— 组或其他用户能读到凭证",
        mode
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 数据目录本身不应对外开放。
#[cfg(unix)]
#[tokio::test]
async fn data_dir_is_created_with_0700() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-dir");
    let _s = JsonStore::open(&d).unwrap();

    let mode = std::fs::metadata(&d).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "数据目录权限应为 0700，实际 {:o}", mode);

    let logs = d.join("logs");
    let lmode = std::fs::metadata(&logs).unwrap().permissions().mode() & 0o777;
    assert_eq!(lmode, 0o700, "日志目录权限应为 0700，实际 {:o}", lmode);
    let _ = std::fs::remove_dir_all(&d);
}

/// 老数据目录里的宽松权限要在启动时被收紧。
///
/// 这是升级路径：老版本用默认权限（0644）写出的 `accounts.json`，
/// 新版本打开时必须收紧，否则「修好了」只是对新用户成立。
#[cfg(unix)]
#[tokio::test]
async fn existing_loose_permissions_are_tightened_on_open() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-legacy");
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("accounts.json");
    std::fs::write(&p, "[]").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();

    // 前置条件：确实是宽松的，否则这个测试什么也没验证
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o644
    );

    let _s = JsonStore::open(&d).unwrap();

    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "启动时应把历史遗留的 0644 收紧为 0600");
    let _ = std::fs::remove_dir_all(&d);
}

/// 收紧是幂等的：已经是 0600 的文件再打开一次不能变坏。
#[cfg(unix)]
#[tokio::test]
async fn tightening_is_idempotent() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-idem");
    let s = JsonStore::open(&d).unwrap();
    s.save_account(&account("13800000001")).await.unwrap();
    drop(s);

    let p = d.join("accounts.json");
    let _s2 = JsonStore::open(&d).unwrap();
    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "重复打开不应放松权限");
    let _ = std::fs::remove_dir_all(&d);
}

/// 阻塞写盘的**上界**必须留在"可接受"的量级内。
///
/// §6 给 `mcloud-store` 开了唯一的阻塞 IO 例外，理由是「一次写盘亚毫秒级」。
/// 那是个会被悄悄推翻的前提：有人把 `LOG_LIMIT_PER_RUN` 调大一个数量级、
/// 或者往每行日志里塞进大段 JSON，例外就不成立了 —— 而**不会有任何测试变红**。
/// 这里把上界钉住：单 run 满额日志的体积与一次写盘耗时。
///
/// 阈值取得很宽（体积 8MB、耗时 200ms），因为这不是性能基准，
/// 而是防"量级失控"。真到需要收紧时，说明该换 `spawn_blocking` 了。
#[test]
fn worst_case_log_write_stays_bounded() {
    use mcloud_core::LOG_LIMIT_PER_RUN;

    // 单条日志取一个偏大的真实形态：80 字符消息 + 序号/时间戳
    let one = LogLine::new(
        1,
        LogLevel::Info,
        "x".repeat(80),
        1_700_000_000_000,
        Seq(LOG_LIMIT_PER_RUN as u64),
    );
    let bytes = serde_json::to_vec(&vec![one; LOG_LIMIT_PER_RUN]).unwrap();
    let mb = bytes.len() as f64 / (1024.0 * 1024.0);
    assert!(
        mb < 8.0,
        "单 run 满额日志已达 {mb:.1}MB，§6 的阻塞 IO 例外需要重新评估"
    );

    // 量一下真实写盘耗时（同目录、同临时文件路径）
    let d = tmpdir("worstcase");
    let s = JsonStore::open(&d).unwrap();
    let p = d.join("bench.json");
    let t = std::time::Instant::now();
    for _ in 0..20 {
        s.write_atomic(&p, &bytes).unwrap();
    }
    let per_ms = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
    assert!(
        per_ms < 200.0,
        "单次写盘 {per_ms:.1}ms，已超出 §6 例外所依赖的量级"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ── 设备指纹（smid）窄写 ────────────────────────────────────────────────

const FP_V1: &str = r#"{"ua":"v1","canvas":"11111111"}"#;
const FP_V2: &str = r#"{"ua":"v2","canvas":"22222222"}"#;
const DEVICE_ID_1: &str = "B1990000000000000000000000000000000000000000000001";

#[tokio::test]
async fn smid_state_is_absent_before_the_first_bind() {
    let d = tmpdir("smid-absent");
    let s = JsonStore::open(&d).unwrap();
    let id = AccountId::new("13800000000");
    // 「从没绑过」必须是 None，不能伪造成一个空状态 ——
    // 两者的后续动作不同（一个要生成、一个不该重生成）
    assert!(
        mcloud_core::SmidStore::get_smid_state(&s, &id)
            .await
            .unwrap()
            .is_none(),
        "未绑定不该伪造一个空 SmidState"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn smid_state_round_trips_across_reopen() {
    let d = tmpdir("smid-roundtrip");
    let id = AccountId::new("13800000000");
    {
        let s = JsonStore::open(&d).unwrap();
        mcloud_core::SmidStore::set_smid_fingerprint(&s, &id, FP_V1)
            .await
            .unwrap();
        mcloud_core::SmidStore::set_smid_device_id(&s, &id, DEVICE_ID_1)
            .await
            .unwrap();
    }
    // 重开一次：绑定必须活过进程重启，否则每次启动都换一台"设备"
    let s = JsonStore::open(&d).unwrap();
    let got = mcloud_core::SmidStore::get_smid_state(&s, &id)
        .await
        .unwrap()
        .expect("重开后应还在");
    assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V1));
    assert_eq!(got.device_id.as_deref(), Some(DEVICE_ID_1));
    let _ = std::fs::remove_dir_all(&d);
}

/// 换指纹必须**连带清掉**旧 deviceId —— 旧 id 是**旧长相**在风控那边换来的。
#[tokio::test]
async fn rotating_the_fingerprint_clears_the_stale_device_id() {
    let d = tmpdir("smid-rotate");
    let s = JsonStore::open(&d).unwrap();
    let id = AccountId::new("13800000000");
    mcloud_core::SmidStore::set_smid_fingerprint(&s, &id, FP_V1)
        .await
        .unwrap();
    mcloud_core::SmidStore::set_smid_device_id(&s, &id, DEVICE_ID_1)
        .await
        .unwrap();

    mcloud_core::SmidStore::set_smid_fingerprint(&s, &id, FP_V2)
        .await
        .unwrap();
    let got = mcloud_core::SmidStore::get_smid_state(&s, &id)
        .await
        .unwrap()
        .expect("有状态");
    assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V2));
    assert_eq!(
        got.device_id, None,
        "新长相配旧 id 会被风控认成同一台设备，比没有 id 更糟"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 窄写的另一面：写 deviceId **不能**碰掉指纹。
#[tokio::test]
async fn writing_the_device_id_leaves_the_fingerprint_alone() {
    let d = tmpdir("smid-narrow");
    let s = JsonStore::open(&d).unwrap();
    let id = AccountId::new("13800000000");
    mcloud_core::SmidStore::set_smid_fingerprint(&s, &id, FP_V1)
        .await
        .unwrap();
    mcloud_core::SmidStore::set_smid_device_id(&s, &id, DEVICE_ID_1)
        .await
        .unwrap();
    let got = mcloud_core::SmidStore::get_smid_state(&s, &id)
        .await
        .unwrap()
        .expect("有状态");
    assert_eq!(
        got.fingerprint_json.as_deref(),
        Some(FP_V1),
        "指纹不该被 deviceId 写入碰掉"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 登录态那条整块覆盖的写**不得**影响指纹。
///
/// 这是把两张表分开存的原因：`save_account_state` 直接替换整个 `Value`，
/// 若指纹也放在里面，登录路径会顺手把它抹掉。
#[tokio::test]
async fn saving_account_state_does_not_touch_the_fingerprint() {
    let d = tmpdir("smid-isolation");
    let s = JsonStore::open(&d).unwrap();
    let id = AccountId::new("13800000000");
    mcloud_core::SmidStore::set_smid_fingerprint(&s, &id, FP_V1)
        .await
        .unwrap();
    mcloud_core::SmidStore::set_smid_device_id(&s, &id, DEVICE_ID_1)
        .await
        .unwrap();

    // 登录路径：整块覆盖登录态
    s.save_account_state(&id, &serde_json::json!({"jwtToken":"JWT-1"}))
        .await
        .unwrap();

    let got = mcloud_core::SmidStore::get_smid_state(&s, &id)
        .await
        .unwrap()
        .expect("有状态");
    assert_eq!(
        got.fingerprint_json.as_deref(),
        Some(FP_V1),
        "换凭据不该换设备长相"
    );
    assert_eq!(got.device_id.as_deref(), Some(DEVICE_ID_1));
    // 而登录态自己确实写进去了
    assert_eq!(s.account_state(&id).await.unwrap()["jwtToken"], "JWT-1");
    let _ = std::fs::remove_dir_all(&d);
}

/// 删账号要连带删指纹，否则同号再次添加会继承旧设备长相。
#[tokio::test]
async fn removing_an_account_drops_its_fingerprint() {
    let d = tmpdir("smid-remove");
    let s = JsonStore::open(&d).unwrap();
    let id = AccountId::new("13800000000");
    s.save_account(&account("13800000000")).await.unwrap();
    mcloud_core::SmidStore::set_smid_fingerprint(&s, &id, FP_V1)
        .await
        .unwrap();

    s.remove_account(&id).await.unwrap();
    assert!(
        mcloud_core::SmidStore::get_smid_state(&s, &id)
            .await
            .unwrap()
            .is_none(),
        "账号没了，它的设备指纹也该跟着走"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ── 系统用户与会话 ──────────────────────────────────────────────────────

use mcloud_core::{Session, SessionKind, UserId, UserStore, WEB_SESSION_TTL_MS};

fn user(id: &str, name: &str) -> mcloud_core::User {
    mcloud_core::User {
        id: UserId::new(id),
        name: name.into(),
        password_hash: mcloud_core::PasswordHash("$argon2id$v=19$m=19456,t=2,p=1$s$h".into()),
        created_at_ms: 1_700_000_000_000,
    }
}

fn session(token_hash: &str, user_id: &str, expires_at_ms: i64) -> Session {
    Session {
        token_hash: token_hash.into(),
        user_id: UserId::new(user_id),
        kind: SessionKind::Web,
        created_at_ms: 1_700_000_000_000,
        expires_at_ms,
    }
}

/// 首次部署判定：0 个用户 ⇒ 需要走 `/api/setup`。
#[tokio::test]
async fn user_count_starts_at_zero() {
    let d = tmpdir("users-empty");
    let s = JsonStore::open(&d).unwrap();
    assert_eq!(s.user_count().await.unwrap(), 0, "新库没有用户");
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn user_crud_and_lookup_by_name() {
    let d = tmpdir("users-crud");
    let s = JsonStore::open(&d).unwrap();

    s.save_user(&user("u1", "admin")).await.unwrap();
    assert_eq!(s.user_count().await.unwrap(), 1);
    assert_eq!(
        s.user_by_name("admin").await.unwrap().unwrap().id,
        UserId::new("u1")
    );
    assert_eq!(
        s.user(&UserId::new("u1")).await.unwrap().unwrap().name,
        "admin"
    );
    assert!(s.user_by_name("nobody").await.unwrap().is_none());
    assert!(s.user(&UserId::new("nope")).await.unwrap().is_none());

    // 同 id 再存是更新
    let mut u = user("u1", "admin2");
    u.password_hash = mcloud_core::PasswordHash("$argon2id$NEW".into());
    s.save_user(&u).await.unwrap();
    assert_eq!(s.user_count().await.unwrap(), 1, "同 id 应是更新");
    assert_eq!(
        s.user(&UserId::new("u1")).await.unwrap().unwrap().name,
        "admin2"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 用户与会话必须**落盘**（重启后还在）。
#[tokio::test]
async fn users_and_sessions_survive_reopen() {
    let d = tmpdir("users-reopen");
    {
        let s = JsonStore::open(&d).unwrap();
        s.save_user(&user("u1", "admin")).await.unwrap();
        s.save_session(&session("hash-1", "u1", 9_999_999_999_999))
            .await
            .unwrap();
    }
    let s2 = JsonStore::open(&d).unwrap();
    assert_eq!(s2.user_count().await.unwrap(), 1);
    assert!(s2.session("hash-1").await.unwrap().is_some(), "会话要落盘");
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn session_lookup_by_token_hash() {
    let d = tmpdir("sess-lookup");
    let s = JsonStore::open(&d).unwrap();
    s.save_session(&session("h1", "u1", 1000)).await.unwrap();
    s.save_session(&session("h2", "u1", 2000)).await.unwrap();

    assert!(s.session("h1").await.unwrap().is_some());
    assert!(s.session("h2").await.unwrap().is_some());
    assert!(
        s.session("nope").await.unwrap().is_none(),
        "不存在的令牌给 None"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn remove_session_logs_out() {
    let d = tmpdir("sess-logout");
    let s = JsonStore::open(&d).unwrap();
    s.save_session(&session("h1", "u1", 1000)).await.unwrap();
    s.remove_session("h1").await.unwrap();
    assert!(
        s.session("h1").await.unwrap().is_none(),
        "登出后令牌立即失效"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 改口令必须清掉该用户的**所有**会话。
///
/// 不清的话"改了口令、旧会话还能用" —— 而用户改口令的动机通常正是
/// "怀疑别人知道我的口令"。这时旧会话还在，等于改了也没用。
#[tokio::test]
async fn changing_password_revokes_all_sessions_of_that_user() {
    let d = tmpdir("sess-revoke");
    let s = JsonStore::open(&d).unwrap();
    s.save_session(&session("a", "u1", 1000)).await.unwrap();
    s.save_session(&session("b", "u1", 1000)).await.unwrap();
    s.save_session(&session("c", "u2", 1000)).await.unwrap();

    let removed = s.remove_user_sessions(&UserId::new("u1")).await.unwrap();
    assert_eq!(removed, 2, "u1 有两条会话，都要清掉");
    assert!(s.session("a").await.unwrap().is_none());
    assert!(s.session("b").await.unwrap().is_none());
    assert!(
        s.session("c").await.unwrap().is_some(),
        "别把别的用户的会话也清了"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ── 过期回收 ────────────────────────────────────────────────────────────

/// ⭐ 过期会话要被清掉；**CLI 会话（不过期）不能被误清**。
#[tokio::test]
async fn purge_removes_expired_but_keeps_permanent() {
    let d = tmpdir("sess-purge");
    let s = JsonStore::open(&d).unwrap();

    s.save_session(&session("expired", "u1", 500))
        .await
        .unwrap();
    s.save_session(&session("future", "u1", 5000))
        .await
        .unwrap();
    // CLI 会话：expires_at_ms = 0 ⇒ 永不过期
    let mut cli = session("cli", "u1", 0);
    cli.kind = SessionKind::Cli;
    s.save_session(&cli).await.unwrap();

    let removed = s.purge_expired_sessions(1000).await.unwrap();
    assert_eq!(removed, 1, "只该清掉那一条过期的");
    assert!(s.session("expired").await.unwrap().is_none());
    assert!(s.session("future").await.unwrap().is_some());
    assert!(
        s.session("cli").await.unwrap().is_some(),
        "不过期的会话不能被清掉 —— expires_at_ms=0 是'永不过期'，不是'早已过期'"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn purge_with_nothing_expired_is_a_noop() {
    let d = tmpdir("sess-purge-none");
    let s = JsonStore::open(&d).unwrap();
    s.save_session(&session("a", "u1", 10_000)).await.unwrap();
    assert_eq!(s.purge_expired_sessions(1000).await.unwrap(), 0);
    let _ = std::fs::remove_dir_all(&d);
}

/// 会话落盘的**是哈希**，不是令牌原文。
#[tokio::test]
async fn session_file_stores_the_hash_not_the_token() {
    let d = tmpdir("sess-hash");
    let s = JsonStore::open(&d).unwrap();
    s.save_session(&session("sha256-of-token", "u1", 1000))
        .await
        .unwrap();

    let raw = std::fs::read_to_string(d.join(files::SESSIONS)).unwrap();
    assert!(raw.contains("token_hash"), "{raw}");
    assert!(raw.contains("sha256-of-token"));
    // 这个字段名本身就是提醒：库里存的是哈希。真正的令牌从不出现在磁盘上。
    let _ = std::fs::remove_dir_all(&d);
}

/// 并发建会话不能丢 —— 多标签页同时登录是常态。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_session_creation_loses_nothing() {
    let d = tmpdir("sess-conc");
    let s = std::sync::Arc::new(JsonStore::open(&d).unwrap());
    let mut handles = Vec::new();
    for i in 0..32 {
        let s = std::sync::Arc::clone(&s);
        handles.push(tokio::spawn(async move {
            s.save_session(&session(&format!("h{i}"), "u1", 9_999_999_999_999))
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let mut found = 0;
    for i in 0..32 {
        if s.session(&format!("h{i}")).await.unwrap().is_some() {
            found += 1;
        }
    }
    assert_eq!(found, 32, "并发建会话一条都不能丢");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn web_session_ttl_is_a_week() {
    assert_eq!(WEB_SESSION_TTL_MS, 604_800_000);
}

// ── 机器级 PC 设备指纹 ──────────────────────────────────────────────────

/// 落盘 / 读回，且**跨实例稳定**（重启不换设备 —— 这是它存在的全部意义）。
#[tokio::test]
async fn pc_device_id_round_trips_and_survives_reopen() {
    use mcloud_core::PcDeviceStore as _;
    let d = tmpdir("pcdev-round");
    let id = "62AA2BDA29F99744536E142A83CA5B03-ENDIN";

    {
        let s = JsonStore::open(&d).unwrap();
        assert_eq!(s.pc_device_id().await.unwrap(), None, "一开始应当没有");
        s.set_pc_device_id(id).await.unwrap();
        assert_eq!(s.pc_device_id().await.unwrap().as_deref(), Some(id));
    }

    // 重新打开：必须是**同一个** id
    let s = JsonStore::open(&d).unwrap();
    assert_eq!(
        s.pc_device_id().await.unwrap().as_deref(),
        Some(id),
        "重开之后设备号漂移了 —— 服务端会永远定位不到扫码会话"
    );

    let _ = std::fs::remove_dir_all(&d);
}

/// 形状非法的值必须被**拒绝写入**，而不是写进去再读不出来。
#[tokio::test]
async fn pc_device_id_rejects_malformed_values() {
    use mcloud_core::PcDeviceStore as _;
    let d = tmpdir("pcdev-bad");
    let s = JsonStore::open(&d).unwrap();

    for bad in [
        "",                                       // 空
        "ABC-ENDIN",                              // 太短
        "62aa2bda29f99744536e142a83ca5b03-ENDIN", // 小写
        "62AA2BDA29F99744536E142A83CA5B03",       // 缺后缀
        "62AA2BDA29F99744536E142A83CA5B0Z-ENDIN", // 非 hex
    ] {
        assert!(
            s.set_pc_device_id(bad).await.is_err(),
            "非法值 {bad:?} 不该被接受"
        );
    }
    assert_eq!(s.pc_device_id().await.unwrap(), None, "非法写不该留下东西");

    let _ = std::fs::remove_dir_all(&d);
}

/// 文件被截断/清空 ⇒ 视作"没有"，而不是当成一个半截设备号。
#[tokio::test]
async fn truncated_pc_device_file_reads_as_absent() {
    use mcloud_core::PcDeviceStore as _;
    let d = tmpdir("pcdev-trunc");
    let s = JsonStore::open(&d).unwrap();
    s.set_pc_device_id("62AA2BDA29F99744536E142A83CA5B03-ENDIN")
        .await
        .unwrap();

    // 模拟写到一半被中断留下的半截文件
    std::fs::write(d.join(files::PC_DEVICE), "62AA2BDA29F997").unwrap();
    assert_eq!(
        s.pc_device_id().await.unwrap(),
        None,
        "半截 id 必须被当成没有（否则每轮都换一个新设备）"
    );

    std::fs::write(d.join(files::PC_DEVICE), "   \n").unwrap();
    assert_eq!(s.pc_device_id().await.unwrap(), None);

    let _ = std::fs::remove_dir_all(&d);
}

/// 末尾换行要能容忍（我们写盘时带一个）。
#[tokio::test]
async fn trailing_newline_is_tolerated() {
    use mcloud_core::PcDeviceStore as _;
    let d = tmpdir("pcdev-nl");
    let s = JsonStore::open(&d).unwrap();
    let id = "62AA2BDA29F99744536E142A83CA5B03-ENDIN";
    std::fs::write(d.join(files::PC_DEVICE), format!("{id}\n")).unwrap();
    assert_eq!(s.pc_device_id().await.unwrap().as_deref(), Some(id));
    let _ = std::fs::remove_dir_all(&d);
}
// ══════════════════════════ 多用户归属 ══════════════════════════
//
// 审计 P1-6：B 能看到 / 删除 A 的账号、读到 A 的全部运行日志、用 A 的账号
// 真花 AI豆。归属判定只有 `mcloud_core::owner::visible_to` 一份实现，
// 路由层直接调这些 `*_for` 方法 —— 所以**在 store 层测**就等于测了路由的
// 判据（`mcloud-server` 的测试在本仓不能跑：链接会 SIGKILL launchd 托管的
// 服务，见 AGENTS.md）。

/// 带归属的账号。
fn owned_account(phone: &str, owner: Option<&str>) -> Account {
    let mut a = account(phone);
    a.owner = owner.map(UserId::new);
    a
}

fn owned_schedule(id: &str, owner: Option<&str>) -> Schedule {
    Schedule {
        id: ScheduleId::new(id),
        name: format!("sch-{id}"),
        task: "sign".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        owner: owner.map(UserId::new),
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    }
}

fn owned_pipeline(id: &str, owner: Option<&str>) -> Pipeline {
    use mcloud_core::{AccountMode, PipelineStep};
    Pipeline {
        id: PipelineId::new(id),
        name: format!("pipe-{id}"),
        accounts: vec![],
        account_mode: AccountMode::Sequential,
        stop_on_error: true,
        steps: vec![PipelineStep {
            task: "sign".into(),
            body: serde_json::Value::Null,
            continue_on_error: false,
        }],
        enabled: true,
        owner: owner.map(UserId::new),
        last_run_ms: None,
    }
}

fn owned_run(id: u64, account_id: &str) -> RunRecord {
    let mut r = run_record(id, RunStatus::Done);
    r.account = account_id.into();
    r
}

/// (a) 账号：列表只含自己的；直取别人的拿不到（路由据此 404）。
#[tokio::test]
async fn accounts_are_listed_and_fetched_only_by_their_owner() {
    let d = tmpdir("owner-acct");
    let s = JsonStore::open(&d).unwrap();
    s.save_user(&user("alice", "alice")).await.unwrap();
    s.save_user(&user("bob", "bob")).await.unwrap();
    s.save_account(&owned_account("13800000001", Some("alice")))
        .await
        .unwrap();
    s.save_account(&owned_account("13800000002", Some("bob")))
        .await
        .unwrap();

    let alice = UserId::new("alice");
    let bob = UserId::new("bob");
    let list_a = s.accounts_for(&alice).await.unwrap();
    assert_eq!(list_a.len(), 1, "A 的列表只该有 A 的账号");
    assert_eq!(list_a[0].id.as_str(), "13800000001");
    assert!(
        s.accounts_for(&bob)
            .await
            .unwrap()
            .iter()
            .all(|x| x.id.as_str() != "13800000001"),
        "B 的列表里不该出现 A 的账号"
    );

    // 直取：别人的一律拿不到（调用方回 404，不泄露「存在但不属于你」）
    assert!(s
        .account_for(&alice, &AccountId::new("13800000002"))
        .await
        .unwrap()
        .is_none());
    assert!(s
        .account_for(&bob, &AccountId::new("13800000001"))
        .await
        .unwrap()
        .is_none());
    // 自己的照常拿得到（不是把功能一并关死）
    assert!(s
        .account_for(&alice, &AccountId::new("13800000001"))
        .await
        .unwrap()
        .is_some());
    let _ = std::fs::remove_dir_all(&d);
}

/// (b) 定时：列表隔离 + 直取 404。
#[tokio::test]
async fn schedules_are_isolated_by_owner() {
    let d = tmpdir("owner-sched");
    let s = JsonStore::open(&d).unwrap();
    s.save_user(&user("alice", "alice")).await.unwrap();
    s.save_user(&user("bob", "bob")).await.unwrap();
    s.save_schedule(&owned_schedule("s-a", Some("alice")))
        .await
        .unwrap();
    s.save_schedule(&owned_schedule("s-b", Some("bob")))
        .await
        .unwrap();

    let alice = UserId::new("alice");
    let bob = UserId::new("bob");
    let list = s.schedules_for(&alice).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id.as_str(), "s-a", "A 只该看到自己的定时");
    assert!(
        s.schedule_for(&alice, &ScheduleId::new("s-b"))
            .await
            .unwrap()
            .is_none(),
        "直取 B 的定时必须是 None ⇒ 路由 404"
    );
    assert!(s
        .schedule_for(&bob, &ScheduleId::new("s-a"))
        .await
        .unwrap()
        .is_none());
    assert!(s
        .schedule_for(&alice, &ScheduleId::new("s-a"))
        .await
        .unwrap()
        .is_some());
    let _ = std::fs::remove_dir_all(&d);
}

/// (b) 编排：列表隔离 + 直取 404。
#[tokio::test]
async fn pipelines_are_isolated_by_owner() {
    let d = tmpdir("owner-pipe");
    let s = JsonStore::open(&d).unwrap();
    s.save_user(&user("alice", "alice")).await.unwrap();
    s.save_user(&user("bob", "bob")).await.unwrap();
    s.save_pipeline(&owned_pipeline("p-a", Some("alice")))
        .await
        .unwrap();
    s.save_pipeline(&owned_pipeline("p-b", Some("bob")))
        .await
        .unwrap();

    let alice = UserId::new("alice");
    let bob = UserId::new("bob");
    let list = s.pipelines_for(&alice).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id.as_str(), "p-a");
    assert!(
        s.pipeline_for(&alice, &PipelineId::new("p-b"))
            .await
            .unwrap()
            .is_none(),
        "直取 B 的编排必须是 None ⇒ 路由 404"
    );
    assert!(s
        .pipeline_for(&bob, &PipelineId::new("p-a"))
        .await
        .unwrap()
        .is_none());
    assert!(s
        .pipeline_for(&alice, &PipelineId::new("p-a"))
        .await
        .unwrap()
        .is_some());
    let _ = std::fs::remove_dir_all(&d);
}

/// (c) 运行记录 / 日志：A 看不到 B 的。
///
/// 日志入口（`GET /api/runs/{id}/logs`）与 SSE、中止三条都先过 `run_for`；
/// 所以「拿不到运行记录」就是「拿不到它的日志」。
#[tokio::test]
async fn runs_and_logs_are_isolated_by_owner() {
    let d = tmpdir("owner-run");
    let s = JsonStore::open(&d).unwrap();
    s.save_user(&user("alice", "alice")).await.unwrap();
    s.save_user(&user("bob", "bob")).await.unwrap();
    s.save_account(&owned_account("13800000001", Some("alice")))
        .await
        .unwrap();
    s.save_account(&owned_account("13800000002", Some("bob")))
        .await
        .unwrap();
    s.upsert_run(&owned_run(1, "13800000001")).await.unwrap();
    s.upsert_run(&owned_run(2, "13800000002")).await.unwrap();
    s.append_logs(
        RunId::new(1),
        &[LogLine::new(1, LogLevel::Info, "A 的日志", 0, Seq(1))],
    )
    .await
    .unwrap();

    let alice = UserId::new("alice");
    let bob = UserId::new("bob");
    let list = s.runs_for(&alice, 100).await.unwrap();
    assert_eq!(list.len(), 1, "A 的运行历史只该有 A 的");
    assert_eq!(list[0].run_id, RunId::new(1));
    assert!(
        s.run_for(&bob, RunId::new(1)).await.unwrap().is_none(),
        "B 不该能看到 A 的运行记录（日志入口据此 404）"
    );
    assert!(s.run_for(&alice, RunId::new(2)).await.unwrap().is_none());
    assert!(s.run_for(&alice, RunId::new(1)).await.unwrap().is_some());
    let _ = std::fs::remove_dir_all(&d);
}

/// (d) 迁移：只有一个管理员、老数据没有 owner ⇒ 加载后回填、照常可用。
#[tokio::test]
async fn legacy_records_without_owner_are_backfilled_to_the_admin() {
    let d = tmpdir("owner-migrate");
    std::fs::create_dir_all(&d).unwrap();
    let admin = user("admin-id", "admin");
    std::fs::write(
        d.join(files::USERS),
        serde_json::to_vec(&vec![admin]).unwrap(),
    )
    .unwrap();
    // 三条**老格式**数据：序列化时 owner=None 会被 skip_serializing_if 跳过
    std::fs::write(
        d.join(files::ACCOUNTS),
        serde_json::to_vec(&vec![account("13800000009")]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        d.join(files::SCHEDULES),
        serde_json::to_vec(&vec![owned_schedule("s-legacy", None)]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        d.join(files::PIPELINES),
        serde_json::to_vec(&vec![owned_pipeline("p-legacy", None)]).unwrap(),
    )
    .unwrap();

    // 前置：盘上确实**没有** owner 这一列，否则这条测试什么也没验
    let raw = std::fs::read_to_string(d.join(files::ACCOUNTS)).unwrap();
    assert!(!raw.contains("owner"), "前置：老数据不该有 owner：{raw}");

    let s = JsonStore::open(&d).unwrap();
    let admin_id = UserId::new("admin-id");
    let list = s.accounts_for(&admin_id).await.unwrap();
    assert_eq!(list.len(), 1, "回填后管理员必须能看到老账号");
    assert_eq!(
        list[0].owner.as_ref(),
        Some(&admin_id),
        "老数据的归属要补给那个唯一的管理员"
    );
    assert_eq!(s.schedules_for(&admin_id).await.unwrap().len(), 1);
    assert_eq!(s.pipelines_for(&admin_id).await.unwrap().len(), 1);
    // 老库照常可用（不是「回填成功但读不出来」）
    assert!(s
        .account_for(&admin_id, &AccountId::new("13800000009"))
        .await
        .unwrap()
        .is_some());

    // 回填结果**落盘**：重启后不再依赖「再回填一次」
    let raw = std::fs::read_to_string(d.join(files::ACCOUNTS)).unwrap();
    assert!(raw.contains("owner"), "回填要写回磁盘：{raw}");
    let reopened = JsonStore::open(&d).unwrap();
    assert_eq!(
        reopened.accounts_for(&admin_id).await.unwrap()[0]
            .owner
            .as_ref(),
        Some(&admin_id)
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// (e) 单用户回归：无主数据对唯一用户照常可见（不许「升级后账号全不见了」）。
#[tokio::test]
async fn single_user_instance_still_sees_ownerless_records() {
    let d = tmpdir("owner-solo");
    let s = JsonStore::open(&d).unwrap();
    s.save_user(&user("solo", "admin")).await.unwrap();
    // 进程内直接造、没写 owner 的数据（测试里常见；也让「某条路忘了写 owner」
    // 不至于把数据变成谁都看不见）
    s.save_account(&account("13800000000")).await.unwrap();
    let solo = UserId::new("solo");
    assert_eq!(s.accounts_for(&solo).await.unwrap().len(), 1);
    assert!(s
        .account_for(&solo, &AccountId::new("13800000000"))
        .await
        .unwrap()
        .is_some());
    let _ = std::fs::remove_dir_all(&d);
}

/// 多用户实例里的无主数据**谁都看不见**（失败关闭），不摊给所有人。
#[tokio::test]
async fn ownerless_records_are_invisible_in_a_multi_user_instance() {
    let d = tmpdir("owner-multi");
    let s = JsonStore::open(&d).unwrap();
    s.save_user(&user("u1", "a")).await.unwrap();
    s.save_user(&user("u2", "b")).await.unwrap();
    // ⚠ 注意：这是**运行期**存进去的无主数据，不走加载回填
    s.save_account(&account("13800000000")).await.unwrap();
    assert!(s.accounts_for(&UserId::new("u1")).await.unwrap().is_empty());
    assert!(s.accounts_for(&UserId::new("u2")).await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d);
}
