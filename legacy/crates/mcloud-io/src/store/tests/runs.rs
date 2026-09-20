// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::UserStore;

#[tokio::test]
async fn run_lifecycle_insert_then_update() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let mut run = RunRecord {
        id: "run-1".into(),
        account_id: Some(AccountId::new("acc-1")),
        capability: "sign".into(),
        status: RunStatus::Running,
        started_at: Utc::now(),
        finished_at: None,
        cloud_gained: None,
        cloud_after: None,
        message: None,
    };
    s.insert_run(Scope::user(&uid()), &run).await.unwrap();

    run.status = RunStatus::Done;
    run.finished_at = Some(Utc::now());
    run.cloud_gained = Some(8);
    run.cloud_after = Some(1286);
    run.message = Some("签到成功".into());
    s.update_run(Scope::user(&uid()), &run).await.unwrap();

    let runs = s.list_runs(Scope::user(&uid()), 10).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Done);
    assert_eq!(runs[0].cloud_gained, Some(8));
    assert_eq!(runs[0].cloud_after, Some(1286));
    assert!(runs[0].finished_at.is_some());
}

#[tokio::test]
async fn duplicate_run_id_does_not_create_a_second_row() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let run = RunRecord {
        id: "run-1".into(),
        account_id: Some(AccountId::new("acc-1")),
        capability: "sign".into(),
        status: RunStatus::Running,
        started_at: Utc::now(),
        finished_at: None,
        cloud_gained: None,
        cloud_after: None,
        message: None,
    };
    s.insert_run(Scope::user(&uid()), &run).await.unwrap();
    s.insert_run(Scope::user(&uid()), &run).await.unwrap();
    assert_eq!(s.list_runs(Scope::user(&uid()), 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn updating_a_missing_or_foreign_run_is_an_error() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let run = RunRecord {
        id: "run-1".into(),
        account_id: Some(AccountId::new("acc-1")),
        capability: "sign".into(),
        status: RunStatus::Failed,
        started_at: Utc::now(),
        finished_at: None,
        cloud_gained: None,
        cloud_after: None,
        message: None,
    };

    // ① 记录不存在
    assert_eq!(
        s.update_run(Scope::user(&uid()), &run)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );

    // ② 记录存在、但调用者不是它账号的主人 → 必须给出**同一个**结论，
    //    否则"报什么错"本身就泄漏了这条记录存在
    s.insert_run(Scope::user(&uid()), &run).await.unwrap();
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
        s.update_run(Scope::user(&other), &run)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );

    // ③ 而主人自己仍然能更新 —— 确认这不是"谁都改不了"
    s.update_run(Scope::user(&uid()), &run).await.unwrap();
}
