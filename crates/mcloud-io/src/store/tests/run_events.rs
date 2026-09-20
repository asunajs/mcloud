// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
//
// `run_events` 这一组的分量集中在**两件事**上，其它都是围绕它们的边角：
//
// 1. **续传**（`run_events_after` 的 `after` 语义）—— SSE 断线重连靠它。
//    序号是唯一的锚点，所以下面每条断言都在盯"边界那一号"到底含不含。
// 2. **归属**（`validate_run_in_scope`）—— 事件行上没有 owner，
//    只顺着 run 查。这条如果漏了，任何登录用户都能往别人的 run 里塞日志。
//
// 幂等那几条（重复写同一批）不是"以防万一"：进程崩在"写完库、还没告诉总线"
// 之间是**正常路径**，重启后重放同一批必须无害。
use super::*;

/// 造一个已归属的 run（带账号）。
async fn put_run(s: &SqliteStore, run_id: &str, account_id: &str) {
    s.insert_run(
        Scope::user(&uid()),
        &RunRecord {
            id: run_id.into(),
            account_id: Some(AccountId::new(account_id)),
            capability: "sign".into(),
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            cloud_gained: None,
            cloud_after: None,
            message: None,
        },
    )
    .await
    .unwrap();
}

/// 造一个 run + 一个账号，返回 run_id。
async fn run_with_account(s: &SqliteStore, run_id: &str) -> RunId {
    put_account(s, "acc-1").await;
    put_run(s, run_id, "acc-1").await;
    RunId::parse(run_id).expect("测试用的 id 必须是合法 uuid")
}

/// 序号从 1 起、按批连续。造一批 `count` 条 `log`。
fn logs(run_id: &RunId, count: usize) -> Vec<Sequenced> {
    (1..=count)
        .map(|i| {
            Sequenced::new(
                Seq::seen(i as u64),
                RunEvent::log(
                    run_id.clone(),
                    mcloud_core::LogLevel::Info,
                    format!("第 {i} 条"),
                ),
            )
        })
        .collect()
}

#[tokio::test]
async fn events_round_trip_in_sequence_order() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440000").await;

    s.append_run_events(Scope::user(&uid()), &run_id, &logs(&run_id, 3))
        .await
        .unwrap();

    let got = s
        .run_events_after(Scope::user(&uid()), &run_id, None, 100)
        .await
        .unwrap();
    assert_eq!(got.len(), 3);
    // 顺序必须是 seq 升序 —— 前端靠它拼出"发生了什么"的时间线
    assert_eq!(
        got.iter().map(|x| x.seq.value()).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(matches!(got[0].event, RunEvent::Log { .. }));
}

/// 这条是整个文件的核心：**续传的边界**。
///
/// `after = Some(n)` 的语义是"我已经看到 n 了"，所以返回的必须**不含 n**。
/// 含了会让前端重复渲染一条；不含 n+1 会让它永远缺一条 —— 两者都静默。
#[tokio::test]
async fn resuming_after_a_seq_excludes_that_seq() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440001").await;
    s.append_run_events(Scope::user(&uid()), &run_id, &logs(&run_id, 5))
        .await
        .unwrap();

    let rest = s
        .run_events_after(Scope::user(&uid()), &run_id, Some(Seq::seen(2)), 100)
        .await
        .unwrap();
    assert_eq!(
        rest.iter().map(|x| x.seq.value()).collect::<Vec<_>>(),
        vec![3, 4, 5],
        "必须从 3 开始：2 已经收到过了"
    );

    // `after = None` 与 `after = Some(0)` 必须等价（0 号从不发出，见 `Seq` 的说明）
    let from_none = s
        .run_events_after(Scope::user(&uid()), &run_id, None, 100)
        .await
        .unwrap();
    let from_zero = s
        .run_events_after(Scope::user(&uid()), &run_id, Some(Seq::start()), 100)
        .await
        .unwrap();
    assert_eq!(from_none.len(), 5);
    assert_eq!(
        from_none.iter().map(|x| x.seq.value()).collect::<Vec<_>>(),
        from_zero.iter().map(|x| x.seq.value()).collect::<Vec<_>>()
    );

    // 已经追平（`after` = 最后一条）→ 空，而不是报错
    let nothing = s
        .run_events_after(Scope::user(&uid()), &run_id, Some(Seq::seen(5)), 100)
        .await
        .unwrap();
    assert!(
        nothing.is_empty(),
        "追平之后应当返回空，方便 SSE 保持连接等新事件"
    );
}

/// 重放同一批必须无害 —— 进程崩在"写完库、还没告诉总线"之间是正常路径。
#[tokio::test]
async fn replaying_the_same_batch_is_idempotent() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440002").await;
    let batch = logs(&run_id, 3);

    s.append_run_events(Scope::user(&uid()), &run_id, &batch)
        .await
        .unwrap();
    s.append_run_events(Scope::user(&uid()), &run_id, &batch)
        .await
        .unwrap();

    let got = s
        .run_events_after(Scope::user(&uid()), &run_id, None, 100)
        .await
        .unwrap();
    assert_eq!(got.len(), 3, "重放不该产生重复行");
}

/// 第二批可以接在第一批后面（seq 连续），这是"边跑边落库"的正常形态。
#[tokio::test]
async fn a_second_batch_appends_after_the_first() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440003").await;

    s.append_run_events(Scope::user(&uid()), &run_id, &logs(&run_id, 2))
        .await
        .unwrap();
    // 从 3 开始的下一批
    let next: Vec<Sequenced> = (3..=4)
        .map(|i| {
            Sequenced::new(
                Seq::seen(i),
                RunEvent::log(
                    run_id.clone(),
                    mcloud_core::LogLevel::Warn,
                    format!("第二批 {i}"),
                ),
            )
        })
        .collect();
    s.append_run_events(Scope::user(&uid()), &run_id, &next)
        .await
        .unwrap();

    let got = s
        .run_events_after(Scope::user(&uid()), &run_id, None, 100)
        .await
        .unwrap();
    assert_eq!(
        got.iter().map(|x| x.seq.value()).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

/// 归属：别人的事件读写不到，也写不进去。
///
/// 三条断言必须给**同一个**结论（`NotFound`）—— "报什么错"本身不能泄漏
/// "这条 run 存在"。这与 `runs.rs` 的 `updating_a_missing_or_foreign_run_is_an_error`
/// 是同一条理由。
#[tokio::test]
async fn another_users_run_events_are_invisible_and_unwritable() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440004").await;
    s.append_run_events(Scope::user(&uid()), &run_id, &logs(&run_id, 2))
        .await
        .unwrap();

    let other = UserId::new("other-user");
    s.upsert_user(&mcloud_core::User::new(
        "other-user",
        "other-user",
        "!",
        mcloud_core::Role::User,
        Utc::now(),
    ))
    .await
    .unwrap();

    assert_eq!(
        s.run_events_after(Scope::user(&other), &run_id, None, 100)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "别人的 run 读不到"
    );
    assert_eq!(
        s.append_run_events(Scope::user(&other), &run_id, &logs(&run_id, 1))
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "别人的 run 也写不进"
    );
    assert_eq!(
        s.delete_run_events(Scope::user(&other), &run_id)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "更不该能删别人的事件"
    );

    // 而主人自己一切正常
    s.run_events_after(Scope::user(&uid()), &run_id, None, 100)
        .await
        .unwrap();
}

/// 不存在的 run 不接受事件写入 —— 否则会留下"事件表有行、runs 表没行"的孤儿数据。
///
/// **读也一样报 `NotFound`**，而不是返回空 Vec。这一点值得说明：
/// "这个 run 不存在"与"它存在但没有事件"是两件不同的事，而返回空把两者混成一件事，
/// 调用方（SSE 端点）就无法区分"该 404"和"该挂着一个空流等新事件"。
/// 一致地报错还顺带保住了"报什么错不泄漏它存不存在"。
#[tokio::test]
async fn events_for_a_missing_run_are_rejected() {
    let s = store_with_user().await;
    let ghost = RunId::parse("550e8400-e29b-41d4-a716-4466554400ff").unwrap();

    assert_eq!(
        s.append_run_events(Scope::user(&uid()), &ghost, &logs(&ghost, 1))
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "run 不存在就不该有事件"
    );
    assert_eq!(
        s.run_events_after(Scope::user(&uid()), &ghost, None, 100)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "读一个不存在的 run 也要 404，不能返回空（否则 SSE 端点分不清该 404 还是挂着等）"
    );
}

/// 删除返回真实行数 —— 调用方要靠它确认"确实清干净了"。
#[tokio::test]
async fn deleting_events_reports_the_real_count() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440005").await;
    s.append_run_events(Scope::user(&uid()), &run_id, &logs(&run_id, 4))
        .await
        .unwrap();

    assert_eq!(
        s.delete_run_events(Scope::user(&uid()), &run_id)
            .await
            .unwrap(),
        4
    );
    assert!(
        s.run_events_after(Scope::user(&uid()), &run_id, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    // 重复删 → 0 行，而不是报错（"已经没了"不是失败）
    assert_eq!(
        s.delete_run_events(Scope::user(&uid()), &run_id)
            .await
            .unwrap(),
        0
    );
}

/// 删除一个 run 的事件不影响别的 run —— 序号是**每个 run 各自**的计数。
#[tokio::test]
async fn deleting_one_runs_events_leaves_the_other_alone() {
    let s = store_with_user().await;
    let a = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440006").await;
    put_run(&s, "550e8400-e29b-41d4-a716-446655440007", "acc-1").await;
    let b = RunId::parse("550e8400-e29b-41d4-a716-446655440007").unwrap();

    s.append_run_events(Scope::user(&uid()), &a, &logs(&a, 3))
        .await
        .unwrap();
    s.append_run_events(Scope::user(&uid()), &b, &logs(&b, 2))
        .await
        .unwrap();

    s.delete_run_events(Scope::user(&uid()), &a).await.unwrap();

    assert!(
        s.run_events_after(Scope::user(&uid()), &a, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.run_events_after(Scope::user(&uid()), &b, None, 100)
            .await
            .unwrap()
            .len(),
        2,
        "删 A 不该动 B"
    );
}

/// `limit` 真的生效 —— **且截断的是尾部**（保留最老的几条）。
///
/// 方向很重要：如果实现写成 `ORDER BY seq DESC LIMIT n`，拿到的是"最新的 n 条"，
/// 前端第一次连上就会看到 run 的结尾、然后发现前面全没了。
#[tokio::test]
async fn the_limit_keeps_the_oldest_events() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440008").await;
    s.append_run_events(Scope::user(&uid()), &run_id, &logs(&run_id, 6))
        .await
        .unwrap();

    let page = s
        .run_events_after(Scope::user(&uid()), &run_id, None, 2)
        .await
        .unwrap();
    assert_eq!(
        page.iter().map(|x| x.seq.value()).collect::<Vec<_>>(),
        vec![1, 2],
        "截断必须从尾部截，保留最老的几条"
    );

    // 拿最后一条的 seq 再问一次 —— 这就是调用方翻页的方式
    let next = s
        .run_events_after(
            Scope::user(&uid()),
            &run_id,
            Some(page.last().unwrap().seq),
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        next.iter().map(|x| x.seq.value()).collect::<Vec<_>>(),
        vec![3, 4]
    );
}

/// 终局事件（含 `ManualRequired` 与它的待办）必须原样往返 —— 这是回看历史的价值所在。
#[tokio::test]
async fn a_manual_required_finish_survives_the_round_trip() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-446655440009").await;

    let step = mcloud_core::ManualStep::new(
        "msg-push",
        None,
        "请前往 App 手动打开消息通知",
        mcloud_core::ManualTarget::MobileApp,
    );
    let batch = vec![
        Sequenced::new(Seq::seen(1), RunEvent::started(run_id.clone(), "msg-push")),
        Sequenced::new(Seq::seen(2), RunEvent::progress(run_id.clone(), 3, 10)),
        Sequenced::new(
            Seq::seen(3),
            RunEvent::finished(
                run_id.clone(),
                mcloud_core::RunOutcome::ManualRequired,
                std::slice::from_ref(&step),
                Some("3 个账号已签到，1 个需手动".into()),
            ),
        ),
    ];
    s.append_run_events(Scope::user(&uid()), &run_id, &batch)
        .await
        .unwrap();

    let got = s
        .run_events_after(Scope::user(&uid()), &run_id, None, 100)
        .await
        .unwrap();
    assert_eq!(got.len(), 3);

    // 终局那条的待办必须一字不差地回来 —— 丢掉它，回看历史就只剩一句"需人工"，
    // 而"要人去做什么"恰恰是这条结论的全部价值
    match &got[2].event {
        RunEvent::Finished {
            outcome,
            manual_steps,
            ..
        } => {
            assert_eq!(*outcome, mcloud_core::RunOutcome::ManualRequired);
            assert_eq!(manual_steps, &vec![step]);
        }
        other => panic!("最后一条应当是 Finished，实得 {other:?}"),
    }
    assert!(got[2].event.is_terminal());

    // 进度那条的百分比也要能还原（前端直接拿它画进度条）
    assert_eq!(got[1].event.percent(), Some(30));
}

/// 空批不报错 —— 调用方不该为了"这次没有日志"写一个 if。
#[tokio::test]
async fn an_empty_batch_is_accepted() {
    let s = store_with_user().await;
    let run_id = run_with_account(&s, "550e8400-e29b-41d4-a716-44665544000a").await;
    s.append_run_events(Scope::user(&uid()), &run_id, &[])
        .await
        .unwrap();
    assert!(
        s.run_events_after(Scope::user(&uid()), &run_id, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
}
