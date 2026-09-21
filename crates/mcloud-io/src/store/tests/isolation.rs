// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::UserStore;

use mcloud_core::pure;

/// **数据隔离**的集中验收：两个用户各一个账号，把所有入口交叉摸一遍。
///
/// 隔离规则是逐方法写在 SQL 里的，这条测试是唯一把它们串起来的地方 ——
/// 漏掉任何一处（"忘了加 `owner` 条件"）都会在这里被抓住，
/// 而漏掉一处不会有任何编译期提示（这正是 `docs/rust-rewrite-plan.md` §3.7 的要点）。
#[tokio::test]
async fn cross_user_isolation_is_enforced_on_every_entry_point() {
    let s = store_with_user().await;
    let alice = uid();
    let bob = UserId::new("bob");
    s.upsert_user(&mcloud_core::User::new(
        "bob",
        "bob",
        "!",
        mcloud_core::Role::User,
        Utc::now(),
    ))
    .await
    .unwrap();

    let mut alice_account = account("acc-a");
    alice_account.owner = Some(alice.clone());
    s.upsert_account(Scope::user(&alice), &alice_account)
        .await
        .unwrap();
    let mut bob_account = account("acc-b");
    bob_account.owner = Some(bob.clone());
    s.upsert_account(Scope::user(&bob), &bob_account)
        .await
        .unwrap();

    // ① 列表与单项读取：只看得见自己的
    let visible_to_alice = s.list_accounts(Scope::user(&alice)).await.unwrap();
    assert_eq!(visible_to_alice.len(), 1);
    assert_eq!(visible_to_alice[0].id.as_str(), "acc-a");
    assert_eq!(s.list_accounts(Scope::user(&bob)).await.unwrap().len(), 1);
    assert!(
        s.get_account(Scope::user(&alice), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.get_account(Scope::user(&bob), &AccountId::new("acc-a"))
            .await
            .unwrap()
            .is_none()
    );

    // ② 登录态：读不到，也写不进
    let bob_state = mcloud_core::AccountState::new("acc-b");
    assert_eq!(
        s.upsert_account_state(Scope::user(&alice), &bob_state)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );
    s.upsert_account_state(Scope::user(&bob), &bob_state)
        .await
        .unwrap();
    assert!(
        s.get_account_state(Scope::user(&alice), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.get_account_state(Scope::user(&bob), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_some()
    );

    // ②b smid 两列：同理。这里的后果比登录态更隐蔽 ——
    //      alice 若写得进 bob 的账号，就是替 bob 的账号凭空认下一台设备
    assert_eq!(
        s.set_smid_fingerprint(Scope::user(&alice), &AccountId::new("acc-b"), FP_V1)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );
    assert_eq!(
        s.set_smid_device_id(Scope::user(&alice), &AccountId::new("acc-b"), DEVICE_ID_1)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );
    s.set_smid_fingerprint(Scope::user(&bob), &AccountId::new("acc-b"), FP_V1)
        .await
        .unwrap();
    assert!(
        s.get_smid_state(Scope::user(&alice), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.get_smid_state(Scope::user(&bob), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_some()
    );

    // ③ 云朵快照：同理
    let stats = pure::next_cloud_stats(&AccountId::new("acc-b"), "2026-09-15", 100, &[]);
    assert_eq!(
        s.upsert_cloud_stats(Scope::user(&alice), &stats)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );
    s.upsert_cloud_stats(Scope::user(&bob), &stats)
        .await
        .unwrap();
    assert!(
        s.cloud_stats_since(Scope::user(&alice), &AccountId::new("acc-b"), "2026-01-01")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.cloud_stats_since(Scope::user(&bob), &AccountId::new("acc-b"), "2026-01-01")
            .await
            .unwrap()
            .len(),
        1
    );

    // ④ 运行记录：写、读、改三条路都要挡住
    let run = RunRecord {
        id: "run-b".into(),
        account_id: Some(AccountId::new("acc-b")),
        capability: "sign".into(),
        status: RunStatus::Done,
        started_at: Utc::now(),
        finished_at: None,
        cloud_gained: None,
        cloud_after: None,
        message: None,
    };
    assert_eq!(
        s.insert_run(Scope::user(&alice), &run)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "不能往别人的账号上写运行记录"
    );
    s.insert_run(Scope::user(&bob), &run).await.unwrap();
    assert!(
        s.list_runs(Scope::user(&alice), 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.list_runs(Scope::user(&bob), 100).await.unwrap().len(), 1);
    assert_eq!(
        s.update_run(Scope::user(&alice), &run)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "不能改别人的运行记录"
    );
    s.update_run(Scope::user(&bob), &run).await.unwrap();

    // ⑤ 不能替别人建账号（owner 与调用者不一致就直接拒）
    let mut forged = account("acc-a2");
    forged.owner = Some(bob.clone());
    assert_eq!(
        s.upsert_account(Scope::user(&alice), &forged)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );

    // ⑥ 同一个 AccountId 被他人占用时不能覆盖：给出可行动的提示，
    //    而不是静默把对方的账号顶掉（AccountId 是脱敏手机号派生的，会撞）
    let mut duplicated = account("acc-b");
    duplicated.owner = Some(alice.clone());
    let err = s
        .upsert_account(Scope::user(&alice), &duplicated)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), mcloud_core::ErrorKind::BadRequest, "{err}");
    assert!(format!("{err}").contains("已被其他用户添加"), "{err}");
    assert_eq!(
        s.get_account(Scope::user(&bob), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .unwrap()
            .nickname
            .as_deref(),
        Some("账号2"),
        "被拒之后原账号必须原样还在"
    );
}

/// **本地范围**（CLI）：看得见所有人的账号，也能写任意账号的数据。
///
/// 这条与上面那条隔离测试是**一对**：隔离管的是"网络上的用户"，
/// 而拿着库文件的人本来就能操作所有 —— 两条都得成立，边界才说得清。
#[tokio::test]
async fn local_scope_sees_and_writes_everything() {
    let s = store_with_user().await;
    let alice = uid();
    let bob = UserId::new("bob");
    s.upsert_user(&mcloud_core::User::new(
        "bob",
        "bob",
        "!",
        mcloud_core::Role::User,
        Utc::now(),
    ))
    .await
    .unwrap();

    let mut a = account("acc-a");
    a.owner = Some(alice.clone());
    s.upsert_account(Scope::user(&alice), &a).await.unwrap();
    let mut b = account("acc-b");
    b.owner = Some(bob.clone());
    s.upsert_account(Scope::user(&bob), &b).await.unwrap();

    // ① 列表：两个都看得见（无论归谁）
    let all = s.list_accounts(Scope::Local).await.unwrap();
    assert_eq!(all.len(), 2, "本地视角必须看得见所有人的账号");

    // ② 单项：别人的账号也读得到（用户视角下这里是 None）
    assert!(
        s.get_account(Scope::Local, &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        s.get_account(Scope::user(&alice), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_none()
    );

    // ③ 写：本地视角可以写别人账号的登录态与快照
    let state = mcloud_core::AccountState::new("acc-b");
    s.upsert_account_state(Scope::Local, &state).await.unwrap();
    assert!(
        s.get_account_state(Scope::Local, &AccountId::new("acc-b"))
            .await
            .unwrap()
            .is_some()
    );
    let stats = pure::next_cloud_stats(&AccountId::new("acc-b"), "2026-09-15", 7, &[]);
    s.upsert_cloud_stats(Scope::Local, &stats).await.unwrap();
    assert_eq!(
        s.cloud_stats_since(Scope::Local, &AccountId::new("acc-b"), "2026-01-01")
            .await
            .unwrap()
            .len(),
        1
    );

    // ③b smid 两列：同理。CLI 侧的 `mcloud smid` 就是本地范围 ——
    //      拿着库文件的人本来就能改所有账号的设备长相，这与隔离不矛盾
    s.set_smid_fingerprint(Scope::Local, &AccountId::new("acc-b"), FP_V1)
        .await
        .unwrap();
    s.set_smid_device_id(Scope::Local, &AccountId::new("acc-b"), DEVICE_ID_1)
        .await
        .unwrap();
    let smid = s
        .get_smid_state(Scope::Local, &AccountId::new("acc-b"))
        .await
        .unwrap()
        .expect("本地范围读得到别人的两列");
    assert_eq!(smid.fingerprint_json.as_deref(), Some(FP_V1));
    assert_eq!(smid.device_id.as_deref(), Some(DEVICE_ID_1));
}

/// 本地范围能写**无主**账号 —— 这是"先 migrate / 先 login 再 init"能成立的关键。
///
/// 用户范围下这条是被拒的（见 `upsert_account` 的说明）：无主账号对谁都不可见，
/// 不该由"某个用户"随手造出来。
#[tokio::test]
async fn local_scope_can_create_unowned_accounts_but_user_scope_cannot() {
    let s = store_with_user().await;
    let alice = uid();

    let orphan = Account::new("acc-orphan"); // owner: None
    s.upsert_account(Scope::Local, &orphan).await.unwrap();

    // 无主账号对任何人都不可见，但本地视角看得见
    assert!(
        s.list_accounts(Scope::user(&alice))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.list_accounts(Scope::Local).await.unwrap().len(), 1);

    // 用户范围下造无主账号必须被拒
    let mut another = Account::new("acc-2");
    another.owner = None;
    assert_eq!(
        s.upsert_account(Scope::user(&alice), &another)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );

    // 认领之后它才进入 alice 的视野
    assert_eq!(s.claim_unowned(&alice).await.unwrap().accounts, 1);
    assert_eq!(s.list_accounts(Scope::user(&alice)).await.unwrap().len(), 1);
}

/// 本地范围下可以覆盖别人的账号（运维语义），用户范围下不行。
#[tokio::test]
async fn local_scope_may_rewrite_someone_elses_account() {
    let s = store_with_user().await;
    let alice = uid();
    let bob = UserId::new("bob");
    s.upsert_user(&mcloud_core::User::new(
        "bob",
        "bob",
        "!",
        mcloud_core::Role::User,
        Utc::now(),
    ))
    .await
    .unwrap();

    let mut b = account("acc-b");
    b.owner = Some(bob.clone());
    s.upsert_account(Scope::user(&bob), &b).await.unwrap();

    // 用户范围：被占用 → 拒绝（这条早已有用例，这里把它和本地范围并排钉住）
    let mut stolen = b.clone();
    stolen.owner = Some(alice.clone());
    stolen.nickname = Some("顶掉".into());
    assert_eq!(
        s.upsert_account(Scope::user(&alice), &stolen)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::BadRequest
    );

    // 本地范围：允许改写（调用方本来就拿着库文件）
    let mut refreshed = b.clone();
    refreshed.nickname = Some("运维刷新".into());
    s.upsert_account(Scope::Local, &refreshed).await.unwrap();
    assert_eq!(
        s.get_account(Scope::user(&bob), &AccountId::new("acc-b"))
            .await
            .unwrap()
            .unwrap()
            .nickname
            .as_deref(),
        Some("运维刷新")
    );
}

/// 本地范围看得到"匹配不到账号"的历史运行记录（它不属于任何人）。
#[tokio::test]
async fn local_scope_lists_orphan_runs() {
    let s = store_with_user().await;
    let run = RunRecord {
        id: "run-orphan".into(),
        account_id: None,
        capability: "sign".into(),
        status: RunStatus::Done,
        started_at: Utc::now(),
        finished_at: None,
        cloud_gained: None,
        cloud_after: None,
        message: None,
    };
    s.insert_run(Scope::Local, &run).await.unwrap();

    assert_eq!(s.list_runs(Scope::Local, 10).await.unwrap().len(), 1);
    assert!(
        s.list_runs(Scope::user(&uid()), 10)
            .await
            .unwrap()
            .is_empty(),
        "无归属的运行记录不该冒充某个人的历史"
    );
}
