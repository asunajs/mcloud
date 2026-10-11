//! `JsonStore` 的测试。
//!
//! 重点在**并发**：这个后端的核心风险是"并发写互相覆盖"，
//! 而那种 bug 在单线程测试里永远不会出现。

use super::*;
use mcloud_core::{
    AccountSettings, AccountStore, AuthInfo, DeviceId, LogLevel, LogStore, PipelineStore, RunStore,
    ScheduleStore, LOG_LIMIT_PER_RUN,
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
        enabled: true,
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
        logs: Vec::new(),
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

// ── 运行日志：**只在磁盘上**（2026-10-09 用户口径）─────────────────────────
//
// 用户口径：「日志和历史不要全加入内存，使用本地记录，让用户设置最大大小」✓。
//
// 这一组钉住存储层那一半：
// * 日志的家是 `data/logs/<id>.json`，**按需读盘**（启动不再全量读 ✓）；
// * `history.json` **不再内联**日志（`RunRecord.logs` 只是接口形状 ✓）；
// * 老 `history.json` 里那份内联日志**迁移**进日志文件（不丢 ✓）；
// * 三个上限（历史条数 / 单次行数 / 目录总量）真的生效 ✓。

/// 造一条**带内联日志**的运行记录（`n` 行）—— 模拟 2026-10-09 之前落盘的形状。
fn run_record_with_logs(id: u64, n: usize) -> RunRecord {
    let mut r = run_record(id, RunStatus::Done);
    r.logs = (0..n)
        .map(|i| mcloud_core::RunLogLine {
            level: LogLevel::Info,
            kind: mcloud_core::LogKind::Info,
            message: format!("第 {i} 行"),
            at_ms: 1_700_000_000_000 + i as i64,
        })
        .collect();
    r
}

/// ⚠ 跨 crate 的钉子：`mcloud_store::HISTORY_LIMIT` 必须**等于**设置里那一格的默认值。
///
/// 两个常量在两个 crate 里（core 不能依赖 store ⇒ 对照只能放在这边 ✓）。
/// 漂了的话，"没改过设置的实例"行为会与改之前不一样 ✗ —— 而那种回归最难查。
#[test]
fn the_history_default_matches_the_store_constant() {
    assert_eq!(
        HISTORY_LIMIT,
        mcloud_core::DEFAULT_HISTORY_LIMIT,
        "两个 crate 的默认值必须一致"
    );
    assert_eq!(
        mcloud_core::AppSettings::default().history_limit,
        HISTORY_LIMIT
    );
}

/// 造 `n` 行日志（`seq = 1..=n` ✓）。
fn log_lines(run: u64, n: usize) -> Vec<LogLine> {
    (1..=n as u64)
        .map(|i| LogLine::new(run, LogLevel::Info, format!("l{i}"), 0, Seq(i)))
        .collect()
}

/// ⭐ 日志落在**自己的文件**里，换个进程（新 `JsonStore`）逐行读得回来；
/// 而 `history.json` 里**一个字节的日志都没有** ✓。
#[tokio::test]
async fn run_logs_live_in_their_own_file_and_survive_a_reopen() {
    let d = tmpdir("runlog-file");
    {
        let s = JsonStore::open(&d).unwrap();
        s.upsert_run(&run_record(7, RunStatus::Done)).await.unwrap();
        let lines: Vec<LogLine> = (1..=5)
            .map(|i| {
                LogLine::new(
                    7,
                    LogLevel::Info,
                    format!("第 {i} 行"),
                    100 + i as i64,
                    Seq(i),
                )
                .with_kind(mcloud_core::LogKind::Success)
            })
            .collect();
        s.append_logs(RunId::new(7), &lines).await.unwrap();
        assert!(s.log_path(RunId::new(7)).exists(), "日志该落成自己的文件");
    }

    // 重启那一半：新实例（= 新进程）按 id 取
    let s2 = JsonStore::open(&d).unwrap();
    let got = s2.logs(RunId::new(7), None).await.unwrap();
    assert_eq!(got.len(), 5, "重启后日志一行都不能少");
    assert_eq!(got[0].message, "第 1 行");
    assert_eq!(got[0].at_ms, 101, "时刻要逐行保住（回看按它显示）");
    assert_eq!(
        got[0].kind,
        mcloud_core::LogKind::Success,
        "分类也要保住（界面按它上徽章与颜色）"
    );

    // ⭐ 历史文件里**没有**日志 —— 这一格正是"不要全加入内存"的落点 ✓
    let raw = std::fs::read_to_string(d.join(files::RUNS)).unwrap();
    assert!(
        !raw.contains("第 1 行"),
        "history.json 里不该再有日志正文，实际：{raw}"
    );
    let on_disk: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(
        on_disk.as_array().unwrap()[0].get("logs").is_none(),
        "记录里连 `logs` 键都不该出现，实际：{raw}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐⭐ 启动**不再把日志读进内存**（这条就是用户那句口径的直接钉子）。
///
/// 盘上有 5 个 run 的日志文件，新开的 store 里缓存应当**是空的** ✓；
/// 而按 id 一取就回来了（按需读盘 ✓）。
#[tokio::test]
async fn startup_does_not_load_logs_into_memory() {
    let d = tmpdir("runlog-lazy");
    {
        let s = JsonStore::open(&d).unwrap();
        for id in 1..=5u64 {
            s.upsert_run(&run_record(id, RunStatus::Done))
                .await
                .unwrap();
            s.append_logs(RunId::new(id), &log_lines(id, 3))
                .await
                .unwrap();
        }
    }
    // 新实例：装载时一个日志文件都不读
    let s2 = JsonStore::open(&d).unwrap();
    assert_eq!(s2.cached_log_runs(), 0, "启动不该把日志读进内存");
    // 但按需读得回来（磁盘才是真源 ✓）
    assert_eq!(s2.logs(RunId::new(3), None).await.unwrap().len(), 3);
    assert_eq!(s2.cached_log_runs(), 1, "读过之后才进缓存");
    let _ = std::fs::remove_dir_all(&d);
}

/// 缓存**有界**：超过 `LOG_CACHE_RUNS` 个 run 就按最近使用淘汰；
/// 被挤出去的那个**数据不丢**（下次访问从盘上读回来 ✓）。
#[tokio::test]
async fn the_log_cache_is_bounded_and_eviction_loses_nothing() {
    let d = tmpdir("runlog-cache");
    let s = JsonStore::open(&d).unwrap();
    let n = (LOG_CACHE_RUNS + 2) as u64;
    for id in 1..=n {
        s.append_logs(RunId::new(id), &log_lines(id, 2))
            .await
            .unwrap();
    }
    assert_eq!(
        s.cached_log_runs(),
        LOG_CACHE_RUNS,
        "缓存必须封顶（否则又变成'全在内存里'✗）"
    );
    // 最早那个被挤出去了 —— 但盘上还在、读得回来 ✓
    assert_eq!(
        s.logs(RunId::new(1), None).await.unwrap().len(),
        2,
        "被淘汰只是离开内存，不是丢数据"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 老 `history.json` 里**内联**的那份日志要**迁移**进日志文件（一次性 ✓）——
/// 不迁就等于把升级前的回看日志丢了 ✗。
#[tokio::test]
async fn inline_logs_from_old_files_are_migrated_into_the_log_file() {
    let d = tmpdir("runlog-migrate");
    // 手写一份"旧形状"的 history.json（内联 logs ✓，没有 data/logs/ 目录）
    let runs = vec![run_record_with_logs(1, 3), run_record(2, RunStatus::Done)];
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(files::RUNS), serde_json::to_vec(&runs).unwrap()).unwrap();

    let s = JsonStore::open(&d).unwrap();
    let got = s.logs(RunId::new(1), None).await.unwrap();
    assert_eq!(got.len(), 3, "老日志必须被搬进日志文件，而不是丢掉");
    assert_eq!(got[0].message, "第 0 行");
    assert_eq!(
        got[0].seq,
        Seq(1),
        "老那份没有序号 ⇒ 按顺序现编（顺序即真相）"
    );
    assert_eq!(got[2].seq, Seq(3));
    assert!(s.log_path(RunId::new(1)).exists(), "日志文件该被建出来");
    // ⭐ 盘上那份也**立刻**变小（不是等下一次运行才写 ✓）：内联日志搬完就从
    // `history.json` 里消失 —— 否则用户"改了却看不出变化" ✓
    let raw = std::fs::read_to_string(d.join(files::RUNS)).unwrap();
    assert!(
        !raw.contains("第 0 行"),
        "迁移之后 history.json 里不该还留着内联日志：{raw}"
    );
    // 内存里那份也清掉了（磁盘那份才是真源 ✓）
    assert!(
        s.run(RunId::new(1)).await.unwrap().unwrap().logs.is_empty(),
        "迁移完记录里不该还留着内联日志"
    );

    // 再写一次盘：`logs` 键彻底消失（以后不会再内联 ✓）
    s.upsert_run(&run_record(3, RunStatus::Done)).await.unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join(files::RUNS)).unwrap()).unwrap();
    assert!(
        after
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("logs").is_none()),
        "迁移之后不该再有内联日志：{after}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 老数据兼容：**没有 `logs` 键**的记录照旧读得进来、取详情不报错（空日志 ✓）。
///
/// 盘上确实有这种记录 —— 加那一格之前落盘的（2026-10-09 实测 `data/history.json`
/// 200 条里还有 29 条是这种 ✓）。
#[tokio::test]
async fn records_without_a_logs_key_still_load_and_keep_their_summary() {
    let d = tmpdir("runlog-legacy");
    // 先正常写一条，再**手工把 `logs` 键删掉** —— 精确模拟"加那一格之前"的落盘形状
    {
        let s = JsonStore::open(&d).unwrap();
        s.upsert_run(&run_record(1, RunStatus::Done)).await.unwrap();
    }
    let p = d.join(files::RUNS);
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    v.as_array_mut().unwrap()[0]
        .as_object_mut()
        .unwrap()
        .remove("logs");
    let legacy_shape = v.as_array().unwrap()[0].clone();
    std::fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();

    let s = JsonStore::open(&d).unwrap();
    let got = s
        .run(RunId::new(1))
        .await
        .unwrap()
        .expect("老记录照旧读得进");
    assert!(got.logs.is_empty(), "没这一格 ⇒ 空日志，不是报错");
    // 汇总字段一个都不能丢（逐字段比：读回来的形状应当与盘上那份**一字不差**）
    assert_eq!(
        serde_json::to_value(&got).unwrap(),
        legacy_shape,
        "读回来的记录必须与盘上那份完全一致（含 outcome/details/status ✓）"
    );

    // 再写一次盘：不许**凭空长出** `"logs": []`
    s.upsert_run(&run_record(2, RunStatus::Done)).await.unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let old = after
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == 1)
        .expect("老记录还在");
    assert!(
        old.get("logs").is_none(),
        "空日志不该写回 `logs` 键，实际：{old}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// `history.json` 的体积**不再随日志长度涨**（日志已经搬去自己的文件 ✓）。
///
/// 这是"最坏体积"那条旧用例的替代：以前 200 条 × 200 行内联日志能到 MB 级 ✓，
/// 现在无论引擎往记录里塞多少行，**写盘那份都只有汇总** ✓（`persist_runs` 与
/// `upsert_run` 两道都把 `logs` 清掉 ✓）。
#[tokio::test]
async fn history_json_does_not_grow_with_logs() {
    let d = tmpdir("history-size");
    let s = JsonStore::open(&d).unwrap();
    // 200 条记录，每条都塞满 200 行日志（模拟"引擎还在攒"的中间态）
    for i in 1..=HISTORY_LIMIT as u64 {
        s.upsert_run(&run_record_with_logs(i, mcloud_core::RUN_LOG_LIMIT))
            .await
            .unwrap();
    }
    let raw = std::fs::read_to_string(d.join(files::RUNS)).unwrap();
    assert!(
        !raw.contains("第 0 行"),
        "写盘那份不该带日志正文（{HISTORY_LIMIT} 条 × {} 行会到 MB 级 ✗）",
        mcloud_core::RUN_LOG_LIMIT
    );
    assert!(
        raw.len() < 512 * 1024,
        "history.json（{HISTORY_LIMIT} 条汇总）不该到 512 KB，实际 {} B",
        raw.len()
    );
    // 记录里那份也没进内存（否则"不要全加入内存"只做了一半 ✓）
    assert_eq!(s.cached_log_runs(), 0, "记录里带的日志不该被收进缓存");
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 日志目录的**总量上限**：超了按**最旧**删文件，运行记录本身不动 ✓。
#[tokio::test]
async fn the_log_directory_is_capped_by_bytes_and_drops_the_oldest() {
    let d = tmpdir("runlog-budget");
    std::fs::create_dir_all(d.join("logs")).unwrap();
    // 直接造文件（不必真跑一遍日志流程）：5 个 × 400 KB = 2 MB > 1 MB 的上限
    for id in 1..=5u64 {
        let blob: Vec<LogLine> = vec![LogLine::new(
            id,
            LogLevel::Info,
            "y".repeat(400 * 1024),
            0,
            Seq(1),
        )];
        std::fs::write(
            d.join("logs").join(format!("{id}.json")),
            serde_json::to_vec(&blob).unwrap(),
        )
        .unwrap();
    }
    // 上限设成最小（1 MB ✓）；同时把记录写进 history.json（记录该留着 ✓）
    let runs: Vec<RunRecord> = (1..=5u64).map(|i| run_record(i, RunStatus::Done)).collect();
    std::fs::write(d.join(files::RUNS), serde_json::to_vec(&runs).unwrap()).unwrap();
    let settings = mcloud_core::AppSettings {
        log_total_mb: 1,
        ..mcloud_core::AppSettings::default()
    };
    std::fs::write(
        d.join(files::SETTINGS),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();

    let s = JsonStore::open(&d).unwrap();
    let stats = s.log_stats().await.unwrap();
    assert!(
        stats.bytes <= 1024 * 1024,
        "启动装载就该把日志目录收到上限内，实际 {} B",
        stats.bytes
    );
    // 留的是**最新**的那几个 ✓（`5.json` 必在，`1.json` 必走）
    assert!(s.log_path(RunId::new(5)).exists(), "最新的必须留着");
    assert!(!s.log_path(RunId::new(1)).exists(), "最旧的该被删");
    // ⚠ 运行记录**一条都不许少**：用户照样看得到"跑过什么、结果如何" ✓
    assert_eq!(s.runs(usize::MAX).await.unwrap().len(), 5);
    let _ = std::fs::remove_dir_all(&d);
}

/// 上限**调小之后立刻生效**（不等下一次运行 ✓），而且盘上那份也跟着变小 ✓。
#[tokio::test]
async fn shrinking_a_limit_applies_immediately_and_persists() {
    let d = tmpdir("limit-shrink");
    let s = JsonStore::open(&d).unwrap();
    for id in 1..=30u64 {
        s.upsert_run(&run_record(id, RunStatus::Done))
            .await
            .unwrap();
    }
    assert_eq!(s.runs(usize::MAX).await.unwrap().len(), 30);

    let mut next = s.settings().await.unwrap();
    next.history_limit = 20;
    s.save_settings(&next).await.unwrap();
    assert_eq!(
        s.runs(usize::MAX).await.unwrap().len(),
        20,
        "调小上限要立刻生效"
    );
    // 重启后仍是 20（盘上那份也写了小 ✓）
    drop(s);
    let s2 = JsonStore::open(&d).unwrap();
    assert_eq!(s2.runs(usize::MAX).await.unwrap().len(), 20);
    let _ = std::fs::remove_dir_all(&d);
}

/// 分页：不带游标取**最后** limit 行；`before` 往上翻；`after` 往下接 ✓。
#[tokio::test]
async fn logs_page_walks_the_file_by_cursor() {
    let d = tmpdir("runlog-page");
    let s = JsonStore::open(&d).unwrap();
    s.append_logs(RunId::new(1), &log_lines(1, 10))
        .await
        .unwrap();

    let tail = s
        .logs_page(RunId::new(1), mcloud_core::LogPageQuery::new(3))
        .await
        .unwrap();
    assert_eq!(
        tail.lines.iter().map(|l| l.seq.get()).collect::<Vec<_>>(),
        vec![8, 9, 10]
    );
    assert_eq!(
        tail.total, 10,
        "total 是全部行数（界面显示'已显示 N / 共 M'）"
    );

    let up = s
        .logs_page(
            RunId::new(1),
            mcloud_core::LogPageQuery {
                before: Some(Seq(8)),
                ..mcloud_core::LogPageQuery::new(3)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        up.lines.iter().map(|l| l.seq.get()).collect::<Vec<_>>(),
        vec![5, 6, 7]
    );

    let down = s
        .logs_page(
            RunId::new(1),
            mcloud_core::LogPageQuery {
                after: Some(Seq(7)),
                ..mcloud_core::LogPageQuery::new(2)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        down.lines.iter().map(|l| l.seq.get()).collect::<Vec<_>>(),
        vec![8, 9]
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 单次运行的行数上限是**设置**说了算（不是写死的常量 ✓）。
#[tokio::test]
async fn the_per_run_limit_comes_from_the_settings() {
    let d = tmpdir("runlog-perrun");
    let s = JsonStore::open(&d).unwrap();
    let mut next = s.settings().await.unwrap();
    next.log_limit_per_run = mcloud_core::LOG_LIMIT_PER_RUN_RANGE.0; // 200
    s.save_settings(&next).await.unwrap();

    s.append_logs(RunId::new(1), &log_lines(1, 300))
        .await
        .unwrap();
    let kept = s.logs(RunId::new(1), None).await.unwrap();
    assert_eq!(kept.len(), 200, "上限该按设置走");
    assert_eq!(kept.last().unwrap().seq, Seq(300), "留最新那 200 行");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 定时与编排 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn schedules_crud() {
    let d = tmpdir("sched");
    let s = JsonStore::open(&d).unwrap();
    let sch = Schedule {
        push_rules: Vec::new(),
        push_events: Vec::new(),
        id: ScheduleId::new("s1"),
        name: "每天签到".into(),
        task: "sign".into(),
        accounts: vec!["13800000000".into()],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        // 这些用例造的都是**手建**的定时 ⇒ 系统维护标记留空 ✓
        //（带标记的那条见 `mcloud-app::live_auto` 的用例 ✓）。
        managed_by: None,
        managed_wanted: None,
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
        push_rules: Vec::new(),
        push_events: Vec::new(),
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

/// **空文件 = 上一次写坏了**，要响亮地失败（2026-10-04 复审 `co-1` 翻的口径）。
///
/// 这条原来钉的是"空文件当没有数据"，理由是"半途崩溃可能留下空文件"。
/// 复审时翻掉：`write_atomic` 现在在 rename **之前** `fsync`，"改了名、内容没落盘"
/// 那条路已经堵上 ⇒ 盘上再出现 0 字节文件，只可能是上一次写入**真的坏了**。
/// 这时静默按空库启动，等于把账号/会话/口令全丢了还一声不响
/// （`accounts.json` 空 ⇒ 用户看到"账号全不见了"）。⇒ 改成报错 + 给恢复路径。
/// 真正的"还没有数据"是**文件不存在**（对照组装在下面）。
#[tokio::test]
async fn empty_file_is_a_loud_error_not_an_empty_database() {
    let d = tmpdir("empty");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(files::ACCOUNTS), "").unwrap();

    let err = JsonStore::open(&d).unwrap_err().to_string();
    assert!(err.contains("空文件"), "实得：{err}");
    assert!(err.contains(".bak"), "要给出恢复路径，实得：{err}");
    let _ = std::fs::remove_dir_all(&d);

    // 对照组：文件**不存在**（首次启动）仍然是"没有数据"，不是错误。
    let d2 = tmpdir("missing");
    let s = JsonStore::open(&d2).unwrap();
    assert!(s.accounts().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&d2);
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
        push_rules: Vec::new(),
        push_events: Vec::new(),
        id: ScheduleId::new("s1"),
        name: "原名".into(),
        task: "sign".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        // 这些用例造的都是**手建**的定时 ⇒ 系统维护标记留空 ✓
        //（带标记的那条见 `mcloud-app::live_auto` 的用例 ✓）。
        managed_by: None,
        managed_wanted: None,
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
        push_rules: Vec::new(),
        push_events: Vec::new(),
        id: ScheduleId::new("s1"),
        name: "x".into(),
        task: "sign".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        // 这些用例造的都是**手建**的定时 ⇒ 系统维护标记留空 ✓
        //（带标记的那条见 `mcloud-app::live_auto` 的用例 ✓）。
        managed_by: None,
        managed_wanted: None,
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
        push_rules: Vec::new(),
        push_events: Vec::new(),
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

/// 装**凭据、会话与账号数据**的文件必须在启动时被收紧。
///
/// 回归护栏：`harden_existing_permissions` 的清单曾经只有五个"运行数据"文件，
/// 于是老数据目录里的 `users.json`（口令哈希）/ `sessions.json`（会话）/
/// `smid.json`（账号设备指纹）/ `pc_device.txt`（机器指纹）仍对同机其他
/// 用户可读 —— 拿到会话哈希对应的令牌等于直接冒充登录。
///
/// ⚠ 2026-10-04 复审 `co-3` 又补了四个（`settings` / `task-config` /
/// `kouling-pool` / `exchange-subscriptions`）：上面那句"覆盖**所有会落盘的表**"
/// 在那之前是不成立的，而 `settings.json` 里正是登录口令哈希。
///
/// ⚠ 2026-10-08 推翻重做推送时又补了两个（`push-panel` / `push-throttle`）——
/// 同时 `push-config.json` 那一格随文件一起删了 ✓（它不再是任何表的落点 ✓）。
#[cfg(unix)]
#[tokio::test]
async fn legacy_loose_credentials_and_sessions_are_tightened_on_open() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-legacy-secrets");
    std::fs::create_dir_all(&d).unwrap();
    // 内容必须**能解析**，否则 `JsonStore::open` 会在 load_all 那一步先报错。
    let secrets: [(&str, &str); 11] = [
        (files::USERS, "[]"),
        (files::SESSIONS, "{}"),
        (files::SMID, "{}"),
        (files::PC_DEVICE, "62AA2BDA29F99744536E142A83CA5B03-ENDIN\n"),
        // 这四个都 `#[serde(default)]`（或在 load_all 里当 map 读）⇒ `{}` 合法
        (files::SETTINGS, "{}"),
        (files::TASK_CONFIG, "{}"),
        (files::KOULING_POOL, "{}"),
        (files::EXCHANGE_SUBSCRIPTIONS, "{}"),
        // ⚠ 2026-10-08 推翻重做补的两个：面板绑定（指向带 token 的规则 ✓）
        // 与节流状态（谁在什么时候被叫醒过 ✓）。**新文件必须一起进清单** ——
        // 漏掉就是老目录升级后仍对同机其他用户可读 ✗。
        (files::PUSH_PANEL, "{}"),
        (files::PUSH_THROTTLE, "{}"),
        // ⚠ 2026-10-08 接登录触发线时补的第三个：它记着"这个账号从哪台设备/
        // 哪个地点登进来过"（`UA|peer IP` ✓）—— 同样是"这个账号怎么活动的"，
        // 与节流状态同一条口径 ⇒ 必须也在清单里 ✓。
        (files::LOGIN_DEVICES, "{}"),
    ];
    for (name, body) in secrets {
        let p = d.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        // 前置：确实是宽松的，否则这条测试什么也没验证
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o644,
            "{name} 的前置权限没设成 0644"
        );
    }

    let _s = JsonStore::open(&d).unwrap();

    for (name, _) in secrets {
        let mode = std::fs::metadata(d.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "{name} 装的是凭据/会话，启动时必须收紧为 0600，实际 {mode:o}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// 这四个文件**新写**时也必须是 0600（`write_atomic` 创建即限权）。
///
/// 与上一条互补：那条管历史遗留，这条管"从现在起写的"。
#[cfg(unix)]
#[tokio::test]
async fn credentials_and_sessions_are_created_with_0600() {
    use mcloud_core::{PcDeviceStore, SmidStore};
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-new-secrets");
    let s = JsonStore::open(&d).unwrap();
    s.save_user(&user("u1", "u1")).await.unwrap();
    s.save_session(&session("h1", "u1", 9_999_999_999_999))
        .await
        .unwrap();
    s.set_smid_fingerprint(&AccountId::new("13800000000"), "{}")
        .await
        .unwrap();
    s.set_pc_device_id("62AA2BDA29F99744536E142A83CA5B03-ENDIN")
        .await
        .unwrap();

    for name in [files::USERS, files::SESSIONS, files::SMID, files::PC_DEVICE] {
        let mode = std::fs::metadata(d.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{name} 创建时就必须是 0600，实际 {mode:o}");
    }
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

/// ⭐⭐ 2026-10-06 安全审计 `SEC-09`：**首次初始化是"判空 + 插入"一个原子动作**。
///
/// 旧写法（server 层先 `user_count`、再 `save_user`）两步之间隔着一次 argon2
/// 且不持锁 ⇒ 两个并发的 `POST /api/setup` 都能看到 0 个用户，于是各建一个
/// 管理员 ✗（后建的那个覆盖前者，谁才是管理员从此不确定）。
///
/// 红测：把 `create_first_user` 改回"先 `user_count()` 再 `save_user()`" ⇒
/// 本测试第二条断言在并发下会红（这里用"先塞一个再调用"的确定写法钉住语义，
/// 并发本身由"判空与插入同一把写锁"保证）。
#[tokio::test]
async fn create_first_user_only_works_on_an_empty_instance() {
    let d = tmpdir("create-first-user");
    let s = JsonStore::open(&d).unwrap();

    // 空库：建得成
    assert!(
        s.create_first_user(&user("u1", "admin")).await.unwrap(),
        "空实例上应当建得成"
    );
    assert_eq!(s.user_count().await.unwrap(), 1);

    // 已经有用户：**不覆盖、也不追加**，如实回 false
    assert!(
        !s.create_first_user(&user("u2", "second")).await.unwrap(),
        "已经有用户时必须回 false"
    );
    assert_eq!(s.user_count().await.unwrap(), 1, "不该多出第二个用户");
    assert_eq!(
        s.user(&UserId::new("u1")).await.unwrap().unwrap().name,
        "admin",
        "原来那个用户必须原样还在"
    );
    assert!(
        s.user(&UserId::new("u2")).await.unwrap().is_none(),
        "竞争失败的那个不该落库"
    );

    // 落盘了才算数（重启后仍是 1 个用户）
    drop(s);
    let s = JsonStore::open(&d).unwrap();
    assert_eq!(s.user_count().await.unwrap(), 1);
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
        push_rules: Vec::new(),
        push_events: Vec::new(),
        id: ScheduleId::new(id),
        name: format!("sch-{id}"),
        task: "sign".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        managed_by: None,
        managed_wanted: None,
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
        push_rules: Vec::new(),
        push_events: Vec::new(),
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

// ── 兑换订阅（用户口径 2026-10-02：按单个奖品订阅 + 存服务端）─────────────

fn sub(prize_id: i64, name: &str, at: i64) -> mcloud_core::ExchangeSubscription {
    mcloud_core::ExchangeSubscription {
        prize_id,
        name: name.into(),
        group_id: Some(1),
        group_title: Some("AI豆兑换".into()),
        subscribed_at_ms: at,
    }
}

/// 写进去 → **重开** → 读回来，而且**按 owner 分表**。
///
/// ⚠ 为什么这条非有不可：订阅的**文件名与键名**是在这一层定的
/// （`exchange-subscriptions.json` + owner id）—— 写错一个字母，功能在界面上就是
/// "点了没反应"，而**编译期一点提示都没有** ✗。这条测试就是那份提示 ✓。
#[tokio::test]
async fn exchange_subscriptions_survive_reopen_and_are_per_owner() {
    let d = tmpdir("exch-sub");
    let a = UserId::new("user-a");
    let b = UserId::new("user-b");
    {
        let s = JsonStore::open(&d).unwrap();
        // 没订阅过 ⇒ **空列表**（不是错误），与 `task_params()` 同一口径 ✓
        assert!(s.exchange_subscriptions(&a).await.unwrap().is_empty());
        s.save_exchange_subscriptions(
            &a,
            &[sub(
                251230053,
                "移动云盘100万tokens叠加包",
                1_790_000_000_000,
            )],
        )
        .await
        .unwrap();
    }
    let s2 = JsonStore::open(&d).unwrap();
    let got = s2.exchange_subscriptions(&a).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].prize_id, 251230053);
    assert_eq!(got[0].name, "移动云盘100万tokens叠加包");
    // **分表**：换个人登录看不见别人的订阅 ✓
    assert!(
        s2.exchange_subscriptions(&b).await.unwrap().is_empty(),
        "订阅是某个人的偏好，不是实例级资源（与口令池相反）"
    );

    // 全取消 ⇒ 那一格**删掉**（文件里不留空数组），读回来仍是空列表 ✓
    s2.save_exchange_subscriptions(&a, &[]).await.unwrap();
    let s3 = JsonStore::open(&d).unwrap();
    assert!(s3.exchange_subscriptions(&a).await.unwrap().is_empty());
    let raw = std::fs::read_to_string(d.join("exchange-subscriptions.json")).unwrap();
    assert!(!raw.contains("user-a"), "全取消后不该留空数组：{raw}");
    let _ = std::fs::remove_dir_all(&d);
}

/// 顺序按**订阅时刻**定死（前端与服务端各算一次也得同序 ✓），
/// 而**重复的 `prize_id` 直接报错** —— 不是静默去重。
///
/// 理由：顶部"已订阅"的判定与"切换"语义都依赖 `prize_id` 唯一 ✓；
/// 静默去重会让"谁写错了"这件事永远查不出来 ✗（写进去的人以为成功了）。
#[tokio::test]
async fn exchange_subscriptions_are_ordered_and_reject_duplicates() {
    let d = tmpdir("exch-sub-order");
    let a = UserId::new("user-a");
    let s = JsonStore::open(&d).unwrap();

    // 故意**乱序**写：读回来必须按订阅时刻排 ✓
    s.save_exchange_subscriptions(&a, &[sub(2, "乙", 200), sub(1, "甲", 100)])
        .await
        .unwrap();
    let got = s.exchange_subscriptions(&a).await.unwrap();
    assert_eq!(
        got.iter().map(|x| x.prize_id).collect::<Vec<_>>(),
        vec![1, 2],
        "按 subscribed_at_ms 排，不按写入顺序"
    );

    let dup = s
        .save_exchange_subscriptions(&a, &[sub(7, "甲", 1), sub(7, "甲又一件", 2)])
        .await;
    assert!(dup.is_err(), "重复的 prize_id 必须报错，不能静默去重");
    let _ = std::fs::remove_dir_all(&d);
}

// ── 推送规则（用户口径 2026-10-07：按 owner 分表 + 落盘）──────────────────

fn rule(id: &str, owner: Option<&str>) -> mcloud_core::PushRule {
    let mut r = mcloud_core::PushRule::new_default(id, owner.map(UserId::new));
    r.name = format!("规则 {id}");
    r
}

/// 写进去 → **重开** → 读回来，而且**按 owner 分表**。
///
/// ⚠ 为什么这条非有不可：文件名（`push-rules.json`）与键名（owner id）都是在
/// 这一层定的 —— 写错一个字母，功能在界面上就是"点了没反应"，而**编译期一点
/// 提示都没有** ✗（与兑换订阅那条同一个理由 ✓）。这条测试就是那份提示 ✓。
#[tokio::test]
async fn push_rules_are_per_owner_and_round_trip() {
    let d = tmpdir("push-rules");
    let a = UserId::new("user-a");
    let b = UserId::new("user-b");
    let r1 = mcloud_core::push::PushRuleId::new("r1");
    let r2 = mcloud_core::push::PushRuleId::new("r2");
    {
        let s = JsonStore::open(&d).unwrap();
        // 没建过 ⇒ **空列表**（不是错误），与 `exchange_subscriptions()` 同一口径 ✓
        assert!(s.push_rules(&a).await.unwrap().is_empty());
        assert!(s.push_rule(&a, &r1).await.unwrap().is_none());

        s.save_push_rule(&a, &rule("r1", Some("user-a")))
            .await
            .unwrap();
        s.save_push_rule(&a, &rule("r2", Some("user-a")))
            .await
            .unwrap();
        // 同一个 id 再存 = **原地替换**，不是新增一条
        let mut renamed = rule("r1", Some("user-a"));
        renamed.name = "改过名的规则".into();
        s.save_push_rule(&a, &renamed).await.unwrap();
        assert_eq!(s.push_rules(&a).await.unwrap().len(), 2, "覆盖不该多出一条");
        assert_eq!(
            s.push_rule(&a, &r1).await.unwrap().unwrap().name,
            "改过名的规则"
        );
    }

    let s2 = JsonStore::open(&d).unwrap();
    let got = s2.push_rules(&a).await.unwrap();
    assert_eq!(got.len(), 2, "重启后两条都还在");
    assert_eq!(got[0].id.as_str(), "r1", "覆盖要保位置（界面顺序不该跳）");
    assert_eq!(got[0].name, "改过名的规则");
    // **分表**：换个人登录读不到别人的规则 ✓
    assert!(
        s2.push_rules(&b).await.unwrap().is_empty(),
        "规则是某个人自己配的，不是实例级资源（与口令池相反）"
    );
    // `all_push_rules` 把 map 的所有值摊平 —— **事件匹配那一层没有"当前用户"** ✓
    let all = s2.all_push_rules().await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|r| r.owner == Some(a.clone())));

    // 删一条：另一条还在
    s2.remove_push_rule(&a, &r1).await.unwrap();
    assert_eq!(s2.push_rules(&a).await.unwrap().len(), 1);
    // 删光 ⇒ 那一格**删掉**（文件里不留空数组），读回来仍是空列表 ✓
    s2.remove_push_rule(&a, &r2).await.unwrap();
    let s3 = JsonStore::open(&d).unwrap();
    assert!(s3.push_rules(&a).await.unwrap().is_empty());
    let raw = std::fs::read_to_string(d.join("push-rules.json")).unwrap();
    assert!(!raw.contains("user-a"), "全删后不该留空数组：{raw}");
    let _ = std::fs::remove_dir_all(&d);
}

/// 造一条**自带 token** 的规则（用户口径：「这个规则配置推送的 token」✓）。
fn rule_with_token(id: &str, token: &str) -> mcloud_core::PushRule {
    let mut r = mcloud_core::PushRule::new_default(id, Some(UserId::new("user-a")));
    r.config = serde_json::from_str(&format!(r#"{{"serverChan":{{"token":"{token}"}}}}"#)).unwrap();
    r
}

/// ⭐ 每条规则**自带凭据** ⇒ 换一条规则的 token **只影响它自己** ✓（§6 验收第 1 条）。
///
/// 这一层能证明的是"落盘之后仍然各是各的" ✓ —— 而"发的时候只用自己那份"在
/// `mcloud-app::notify` 的测试里钉 ✓（那边才是投递 ✓）。两条缺一不可：
/// 只钉投递的话，"存的时候把 token 串了"照样漏网 ✗。
#[tokio::test]
async fn each_rule_keeps_its_own_credentials() {
    let d = tmpdir("push-rules-config");
    let a = UserId::new("user-a");
    {
        let s = JsonStore::open(&d).unwrap();
        s.save_push_rule(&a, &rule_with_token("r1", "TOKEN-1"))
            .await
            .unwrap();
        s.save_push_rule(&a, &rule_with_token("r2", "TOKEN-2"))
            .await
            .unwrap();
    }

    let s = JsonStore::open(&d).unwrap();
    let r1 = s
        .push_rule(&a, &mcloud_core::push::PushRuleId::new("r1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r1.config.server_chan.as_ref().unwrap().token, "TOKEN-1");

    // 改 r1 的 token ⇒ r2 一格都不动 ✓（"改一条影响一片"正是老模型的问题 ✗）
    let mut edited = r1.clone();
    edited.config.server_chan.as_mut().unwrap().token = "TOKEN-1-NEW".into();
    s.save_push_rule(&a, &edited).await.unwrap();
    let r2 = s
        .push_rule(&a, &mcloud_core::push::PushRuleId::new("r2"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r2.config.server_chan.as_ref().unwrap().token, "TOKEN-2");
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 面板绑定与节流状态：**按 owner 分表**、能重开读回，节流状态**重启不能忘** ✓。
#[tokio::test]
async fn panel_bindings_and_throttle_state_round_trip_per_owner() {
    let d = tmpdir("push-panel");
    let a = UserId::new("user-a");
    let b = UserId::new("user-b");
    {
        let s = JsonStore::open(&d).unwrap();
        // 没配过 ⇒ **默认那份**（不绑规则 + 保守节流 ✓），不是错误 ✓
        let fresh = s.push_panel(&a).await.unwrap();
        assert!(fresh.perf_slow.rules.is_empty());
        assert_eq!(
            fresh.perf_slow.throttle.mode,
            mcloud_core::ThrottleMode::OnChange
        );
        assert_eq!(
            fresh.login_new.throttle.mode,
            mcloud_core::ThrottleMode::Every
        );

        let mut panel = mcloud_core::PanelBindings::default();
        panel.perf_slow.rules = vec![
            mcloud_core::push::PushRuleId::new("r1"),
            mcloud_core::push::PushRuleId::new("r2"),
        ];
        panel.perf_slow.throttle.cooldown_secs = 60;
        panel.perf_slow.throttle.daily_cap = 3;
        s.save_push_panel(&a, &panel).await.unwrap();

        let mut throttle = mcloud_core::ThrottleState::default();
        throttle.set(
            mcloud_core::BehaviorKey::PerfSlow,
            mcloud_core::BehaviorState {
                last_sent_ms: Some(1_700_000_000_000),
                day: Some(19_675),
                sent_today: 2,
                throttled_today: 5,
                last_status: Some(mcloud_core::BehaviorStatus::Abnormal),
            },
        );
        s.save_push_throttle(&a, &throttle).await.unwrap();
    }

    let s = JsonStore::open(&d).unwrap();
    let panel = s.push_panel(&a).await.unwrap();
    assert_eq!(panel.perf_slow.rules.len(), 2, "一个行为可以选**多条**规则");
    assert_eq!(panel.perf_slow.throttle.cooldown_secs, 60);
    assert_eq!(panel.perf_slow.throttle.daily_cap, 3);

    // ⭐ 重启不能忘（否则重启即再来一轮风暴 ✗）
    let perf = s
        .push_throttle(&a)
        .await
        .unwrap()
        .get(mcloud_core::BehaviorKey::PerfSlow);
    assert_eq!(perf.last_sent_ms, Some(1_700_000_000_000));
    assert_eq!(perf.sent_today, 2);
    assert_eq!(perf.throttled_today, 5, "「今天已节流 N 次」的留痕要活着");
    assert_eq!(
        perf.last_status,
        Some(mcloud_core::BehaviorStatus::Abnormal)
    );

    // **分表**：换个人登录拿到的是默认那份 ✓
    assert!(s.push_panel(&b).await.unwrap().perf_slow.rules.is_empty());
    assert_eq!(
        s.push_throttle(&b)
            .await
            .unwrap()
            .get(mcloud_core::BehaviorKey::PerfSlow)
            .sent_today,
        0
    );

    // 全空的状态 ⇒ 那一格删掉（文件里不留空对象 ✓，读回来仍是默认 ✓）
    s.save_push_throttle(&a, &mcloud_core::ThrottleState::default())
        .await
        .unwrap();
    let raw = std::fs::read_to_string(d.join("push-throttle.json")).unwrap();
    assert!(!raw.contains("user-a"), "全空后不该留一格：{raw}");
    assert_eq!(
        s.push_throttle(&a)
            .await
            .unwrap()
            .get(mcloud_core::BehaviorKey::PerfSlow)
            .sent_today,
        0
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 两个新落盘文件**新写时也是 0600**（里面有 token / 状态 ✓）。
#[cfg(unix)]
#[tokio::test]
async fn push_panel_and_throttle_are_created_with_0600() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-push-panel");
    let a = UserId::new("user-a");
    let s = JsonStore::open(&d).unwrap();
    let mut panel = mcloud_core::PanelBindings::default();
    panel.login_failed.rules = vec![mcloud_core::push::PushRuleId::new("r1")];
    s.save_push_panel(&a, &panel).await.unwrap();
    let mut throttle = mcloud_core::ThrottleState::default();
    throttle.set(
        mcloud_core::BehaviorKey::LoginFailed,
        mcloud_core::BehaviorState {
            last_sent_ms: Some(1),
            ..Default::default()
        },
    );
    s.save_push_throttle(&a, &throttle).await.unwrap();

    for name in [files::PUSH_PANEL, files::PUSH_THROTTLE] {
        let mode = std::fs::metadata(d.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{name} 创建时就必须是 0600，实际 {mode:o}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 「每用户一个钟点」（`data/auth-refresh.json` ✓，2026-10-11 第二轮口径）。
///
/// 钉四件事：
/// * **按 owner 分表**：一个用户设了，另一个读到的仍是"没设过"（`None` ✓）；
/// * **落盘且读得回来**（重启后还认那个钟点 ✓ —— 只放内存的话重启就回到初始值 ✗）；
/// * **缺文件 / 缺键 ⇒ `None`**（老目录升级路径 ✓，**不报错** ✓）；
/// * **非法钟点当场拒**（`半夜` 不许进这个文件 ✓ —— 写进去之后 `daily_expr` 当场
///   报"写不成 cron"，表现是"钟点存上了、定时却装不上" ✗）。
#[tokio::test]
async fn auth_refresh_clock_is_per_owner_and_round_trips() {
    let d = tmpdir("auth-refresh");
    let a = UserId::new("user-a");
    let b = UserId::new("user-b");
    {
        let s = JsonStore::open(&d).unwrap();
        // 前置条件：**文件都还没有** —— 老目录就是这样 ✓（读得进、回 `None` ✓）
        assert!(!d.join(files::AUTH_REFRESH).exists());
        assert_eq!(s.auth_refresh_at(&a).await.unwrap(), None);

        s.save_auth_refresh_at(&a, "22:30").await.unwrap();
        // 另一个用户**不受影响**（分表 ✓）
        assert_eq!(s.auth_refresh_at(&b).await.unwrap(), None);
        s.save_auth_refresh_at(&b, "07:15").await.unwrap();
        // 前后空白先 trim（与 `parse_clock` 同一条口径 ✓）
        s.save_auth_refresh_at(&b, " 07:15 ").await.unwrap();
        assert_eq!(
            s.auth_refresh_at(&b).await.unwrap().as_deref(),
            Some("07:15")
        );

        // ⭐ 非法钟点**当场拒**，而且**不落盘** ✓
        for bad in ["", "25:00", "12:60", "半夜", "0300"] {
            assert!(
                s.save_auth_refresh_at(&a, bad).await.is_err(),
                "「{bad}」不是钟点，应被拒"
            );
        }
        assert_eq!(
            s.auth_refresh_at(&a).await.unwrap().as_deref(),
            Some("22:30"),
            "被拒的写入不该把原来那个值改掉"
        );
    }

    // 重启：两个用户各是各的 ✓
    let s = JsonStore::open(&d).unwrap();
    assert_eq!(
        s.auth_refresh_at(&a).await.unwrap().as_deref(),
        Some("22:30")
    );
    assert_eq!(
        s.auth_refresh_at(&b).await.unwrap().as_deref(),
        Some("07:15")
    );
    // 文件里也真的是按 owner 存的（键就是 owner id ✓）
    let raw = std::fs::read_to_string(d.join(files::AUTH_REFRESH)).unwrap();
    assert!(raw.contains("user-a") && raw.contains("22:30"), "{raw}");
    assert!(raw.contains("user-b") && raw.contains("07:15"), "{raw}");
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 新落盘的那个文件**新写时也是 0600**（它记着"这个账号每天几点轮换凭据" ✓）。
///
/// ⚠ 与 `push_panel_and_throttle_are_created_with_0600` 同一个理由：
/// 进了 `harden_existing_permissions` 清单的文件，**新写时**就已经是 0600 ✓
///（那道 hardening 只管历史遗留 ✓）。
#[cfg(unix)]
#[tokio::test]
async fn auth_refresh_file_is_created_with_0600() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-auth-refresh");
    let s = JsonStore::open(&d).unwrap();
    s.save_auth_refresh_at(&UserId::new("user-a"), "22:30")
        .await
        .unwrap();
    let mode = std::fs::metadata(d.join(files::AUTH_REFRESH))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode,
        0o600,
        "{} 创建时就必须是 0600，实际 {mode:o}",
        files::AUTH_REFRESH
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 老目录上那个文件**权限松**（手工拷过来的 / 建得早 ✓）⇒ 启动时收紧 ✓。
#[cfg(unix)]
#[tokio::test]
async fn a_loose_auth_refresh_file_is_tightened_on_open() {
    use std::os::unix::fs::PermissionsExt as _;

    let d = tmpdir("perm-auth-refresh-legacy");
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join(files::AUTH_REFRESH);
    std::fs::write(&p, r#"{"user-a":"22:30"}"#).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();

    let s = JsonStore::open(&d).unwrap();
    let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "老目录升级后不该对同机其他用户可读");
    // 顺手钉住：那份老内容读得进来 ✓（不是"收权限把内容搞丢了" ✓）
    assert_eq!(
        s.auth_refresh_at(&UserId::new("user-a"))
            .await
            .unwrap()
            .as_deref(),
        Some("22:30")
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 「见过的登录设备」（`remember_login_device` ✓，2026-10-08 接 `loginNew` 时新增）。
///
/// 钉四件事：
/// * **第一次见**回 `true`（= 该报"新设备" ✓）、**第二次见**回 `false` ✓；
/// * **分表**：同一个判据串在别人名下仍是"第一次见" ✓（判据是"账号 + 设备" ✓）；
/// * **落盘且读得回来**（重启之后同一台设备不再算新 ✓ —— 只放内存的话
///   每次重启都报一轮"新设备" ✗）；
/// * **有上限**：超出 `LOGIN_DEVICES_PER_OWNER` 丢最旧的 ✓（判据串里带 peer IP
///   ⇒ 每换一个网络就是一条，没上限就是只涨不跌 ✗）。
#[tokio::test]
async fn login_devices_are_per_owner_persisted_and_capped() {
    let d = tmpdir("login-devices");
    let a = UserId::new("user-a");
    let b = UserId::new("user-b");

    {
        let s = JsonStore::open(&d).unwrap();
        assert!(
            s.remember_login_device(&a, "UA-1|10.0.0.1").await.unwrap(),
            "第一次见必须回 true（否则 loginNew 永远不触发 ✗）"
        );
        assert!(
            !s.remember_login_device(&a, "UA-1|10.0.0.1").await.unwrap(),
            "同一台设备第二次登录**不是**新设备"
        );
        assert!(
            s.remember_login_device(&a, "UA-2|10.0.0.1").await.unwrap(),
            "换一台设备（UA 变了）算新设备"
        );
        // 分表：同样的串在别人名下是第一次见 ✓
        assert!(s.remember_login_device(&b, "UA-1|10.0.0.1").await.unwrap());

        // 撑过上限：灌 30 条，只留最近 20 条 ✓
        for i in 0..30 {
            s.remember_login_device(&a, &format!("flood-{i}|10.0.0.9"))
                .await
                .unwrap();
        }
    }

    // 重新打开 = 重启：落盘那份还在 ✓
    let s = JsonStore::open(&d).unwrap();
    assert!(
        !s.remember_login_device(&a, "flood-29|10.0.0.9")
            .await
            .unwrap(),
        "重启之后最近见过的设备不该又变成新的（只放内存就会这样 ✗）"
    );
    assert!(
        s.remember_login_device(&a, "UA-1|10.0.0.1").await.unwrap(),
        "被上限挤掉的最旧那条，回来时**就该**算新设备 ✓（这是刻意的语义）"
    );

    let raw = std::fs::read_to_string(d.join(files::LOGIN_DEVICES)).unwrap();
    let map: std::collections::HashMap<String, Vec<String>> = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        map.get("user-a").map(Vec::len),
        Some(LOGIN_DEVICES_PER_OWNER),
        "每个 owner 最多留 {LOGIN_DEVICES_PER_OWNER} 条：{raw}"
    );
    assert!(
        !raw.contains("flood-0|"),
        "最旧的那条该被挤掉（无界增长迟早是负担 ✗）：{raw}"
    );
    assert!(raw.contains("flood-29|"), "最新的那条要在：{raw}");
    assert_eq!(map.get("user-b").map(Vec::len), Some(1), "分表：{raw}");

    let _ = std::fs::remove_dir_all(&d);
}

/// ⭐ 旧形状的推送文件**删掉即可** ⇒ 启动不报错 ✓（§6 验收第 6 条）。
///
/// 两个场景都要能起来 ✓：
/// ① 目录里躺着**旧形状**的 `push-rules.json` / `push-config.json`；
/// ② 目录里**压根没有**这两个文件（新装 / 清空过 ✓）。
#[tokio::test]
async fn legacy_push_files_are_purged_and_startup_survives() {
    let d = tmpdir("push-legacy-purge");
    std::fs::create_dir_all(&d).unwrap();
    // ① 旧形状的规则：只有 events/channels、**没有 config** ✓ ——
    //    新形状读它一定报错，所以必须在 `load_all` **之前**删掉 ✓。
    std::fs::write(
        d.join("push-rules.json"),
        r#"{"user-a":[{"id":"r1","name":"旧规则","events":["runFailed"],"channels":["serverChan"],"digest":"perRun","slowFactor":3.0,"failStreak":3,"enabled":true,"owner":"user-a"}]}"#,
    )
    .unwrap();
    // `write_atomic` 覆盖前留的那份也一起清 ✓
    std::fs::write(d.join("push-rules.bak"), "{}").unwrap();
    // ② 用户级渠道配置那一层（**整个不存在了** ✓，而且里面是 token ✓）
    std::fs::write(
        d.join("push-config.json"),
        r#"{"user-a":{"serverChan":{"token":"SCT"}}}"#,
    )
    .unwrap();

    let s = JsonStore::open(&d).unwrap();
    assert!(
        !d.join("push-rules.json").exists(),
        "旧形状的规则文件必须删掉（否则 load_all 整份报错、启动失败）"
    );
    assert!(!d.join("push-rules.bak").exists(), ".bak 也要一起清");
    assert!(
        !d.join("push-config.json").exists(),
        "用户级渠道配置那一层已经不存在了，文件不该留着"
    );
    assert!(s
        .push_rules(&UserId::new("user-a"))
        .await
        .unwrap()
        .is_empty());

    // ② 缺文件的场景：新目录照样起得来 ✓
    let empty = tmpdir("push-legacy-absent");
    let s2 = JsonStore::open(&empty).unwrap();
    assert!(s2.all_push_rules().await.unwrap().is_empty());
    assert!(s2
        .push_panel(&UserId::new("user-a"))
        .await
        .unwrap()
        .login_new
        .rules
        .is_empty());

    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&empty);
}

/// ⚠ 清理**只认旧形状**：新形状的规则文件**不能**在启动时被删掉 ✓
/// （否则规则表就成了"写进去、重启就没了" ✗ —— 那与"按 owner 分表落盘"矛盾 ✓）。
#[tokio::test]
async fn new_shape_push_rules_survive_a_restart() {
    let d = tmpdir("push-purge-keeps-new");
    let a = UserId::new("user-a");
    {
        let s = JsonStore::open(&d).unwrap();
        s.save_push_rule(&a, &rule_with_token("r1", "TOKEN-1"))
            .await
            .unwrap();
    }
    let s = JsonStore::open(&d).unwrap();
    assert_eq!(
        s.push_rules(&a).await.unwrap().len(),
        1,
        "新形状的文件必须活着（清理只针对旧形状）"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ── 定时水位线的两种口径（2026-10-03 review §1.7）──────────────────────

fn schedule(id: &str) -> Schedule {
    Schedule {
        push_rules: Vec::new(),
        push_events: Vec::new(),
        id: ScheduleId::new(id),
        name: "签到".into(),
        task: "sign".into(),
        accounts: vec![],
        expr: "0 8 * * *".into(),
        timezone: "Asia/Shanghai".into(),
        body: serde_json::Value::Null,
        enabled: true,
        // 这些用例造的都是**手建**的定时 ⇒ 系统维护标记留空 ✓
        //（带标记的那条见 `mcloud-app::live_auto` 的用例 ✓）。
        managed_by: None,
        managed_wanted: None,
        owner: None,
        last_fire_ms: None,
        last_run_ms: None,
        last_status: None,
        last_error: None,
        run_count: 0,
    }
}

/// ⭐ **弃置的触发点不算"跑过一次"**（review §1.7）。
///
/// `mark_schedule_fired` 里那句 `run_count += 1` 记的是"真的跑过一次"，
/// 而 `Decision::Missed`（错过的触发点整体弃掉）也要推水位 —— 它共用那个方法时，
/// 一次上游都没打的弃置会被记成一次运行，界面上「累计触发 N 次」虚高
/// （`web/src/pages/schedules-page.tsx:384` 展示它）。
///
/// 两条口径都钉住：**弃置**只动水位、**真跑**才 +1。
#[tokio::test]
async fn watermark_advance_does_not_count_a_run() {
    let d = tmpdir("watermark");
    let s = JsonStore::open(&d).unwrap();
    s.save_schedule(&schedule("s1")).await.unwrap();

    let id = ScheduleId::new("s1");

    // 弃置：推水位，但不计数
    s.advance_schedule_watermark(&id, 1_700_000_000_000)
        .await
        .unwrap();
    let row = s.schedule(&id).await.unwrap().expect("应有这一行");
    assert_eq!(row.last_fire_ms, Some(1_700_000_000_000), "水位要推");
    assert_eq!(row.run_count, 0, "弃置不该记成跑过一次");

    // 真跑：推水位**并且** +1
    s.mark_schedule_fired(&id, 1_700_000_060_000).await.unwrap();
    let row = s.schedule(&id).await.unwrap().unwrap();
    assert_eq!(row.last_fire_ms, Some(1_700_000_060_000));
    assert_eq!(row.run_count, 1, "真跑要计数");

    // 对不存在的 id：两种都不报错（定时在这期间被删了）
    let gone = ScheduleId::new("nope");
    s.advance_schedule_watermark(&gone, 1).await.unwrap();
    s.mark_schedule_fired(&gone, 1).await.unwrap();

    let _ = std::fs::remove_dir_all(&d);
}

// ── co-1（2026-10-04 复审）：0 字节文件不能毁掉备份、也不能被当成空库 ──────

/// 上一次写入被截断（0 字节残骸）时，**不能**拿它去覆盖 `.bak`。
///
/// 红测：把 `write_atomic` 里的 `if !old.is_empty()` 去掉 ⇒ 最后一条断言
/// 读到的 `.bak` 是空字符串，红。
#[test]
fn a_torn_empty_file_must_not_destroy_the_backup() {
    let d = tmpdir("torn-bak");
    let s = JsonStore::open(&d).unwrap();
    let path = d.join(files::ACCOUNTS);
    let bak = path.with_extension("bak");

    s.write_atomic(&path, b"v1").unwrap(); // 首次写：还没有 .bak
    s.write_atomic(&path, b"v2").unwrap(); // 这次才留下 .bak = v1
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), "v1");

    // 模拟"写了一半被杀"：主文件在，但 0 字节
    std::fs::write(&path, b"").unwrap();
    s.write_atomic(&path, b"v3").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "v3");
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "v1",
        "0 字节的旧文件不该把上一份好备份覆盖掉"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// 端到端：盘上躺着一个 0 字节的 `accounts.json` 时，`open` 必须**响亮地失败**
/// （这正是"掉电后静默丢全部账号"那条路的入口）。
#[test]
fn opening_a_store_with_a_torn_accounts_file_fails_loudly() {
    let d = tmpdir("torn-open");
    {
        let s = JsonStore::open(&d).unwrap();
        s.write_json(&d.join(files::ACCOUNTS), &vec![account("19900000001")])
            .unwrap();
    }
    std::fs::write(d.join(files::ACCOUNTS), b"").unwrap();

    let msg = JsonStore::open(&d).unwrap_err().to_string();
    assert!(msg.contains("空文件"), "实得：{msg}");
    let _ = std::fs::remove_dir_all(&d);
}
