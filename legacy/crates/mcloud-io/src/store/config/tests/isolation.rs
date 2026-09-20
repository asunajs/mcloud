// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use chrono::{TimeZone, Utc};

use mcloud_core::{ClaimedRows, Role, Scope, StateStore, User, UserId, UserStore};

// ───────────── 归属：定时任务与组合（schema v4 / §3.7） ─────────────

/// 两个用户，供归属用例使用。
async fn two_users(s: &SqliteStore) -> (UserId, UserId) {
    ensure_user(s).await;
    let other = UserId::new("other-user");
    s.upsert_user(&User::new(
        other.as_str(),
        other.as_str(),
        "!",
        Role::User,
        Utc.with_ymd_and_hms(2026, 9, 16, 1, 0, 0).unwrap(),
    ))
    .await
    .unwrap();
    (uid(), other)
}

/// 交叉读取：A 的定时任务与组合，B 既看不到也删不掉。
///
/// "看不到"与"不存在"必须是同一个结果（连存在性都不泄露）。
#[tokio::test]
async fn configs_are_isolated_between_users() {
    let s = store();
    let (a, b) = two_users(&s).await;

    let mut sc = schedule("sched-a");
    sc.owner = Some(a.clone());
    s.upsert_schedule(Scope::user(&a), &sc).await.unwrap();
    let mut pl = pipeline("pipe-a");
    pl.owner = Some(a.clone());
    s.upsert_pipeline(Scope::user(&a), &pl).await.unwrap();

    // 本人看得见
    assert_eq!(s.list_schedules(Scope::user(&a)).await.unwrap().len(), 1);
    assert_eq!(s.list_pipelines(Scope::user(&a)).await.unwrap().len(), 1);
    assert!(
        s.get_schedule(Scope::user(&a), "sched-a")
            .await
            .unwrap()
            .is_some()
    );

    // 别人看不见
    assert!(
        s.list_schedules(Scope::user(&b)).await.unwrap().is_empty(),
        "B 不该看见 A 的定时任务"
    );
    assert!(s.list_pipelines(Scope::user(&b)).await.unwrap().is_empty());
    assert!(
        s.get_schedule(Scope::user(&b), "sched-a")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.get_pipeline(Scope::user(&b), "pipe-a")
            .await
            .unwrap()
            .is_none()
    );

    // 删也删不掉，且**返回 false 而不是报错**（不可见等价于不存在）
    assert!(!s.delete_schedule(Scope::user(&b), "sched-a").await.unwrap());
    assert_eq!(
        s.list_schedules(Scope::user(&a)).await.unwrap().len(),
        1,
        "B 的删除不该动到 A 的行"
    );

    // Scope::Local（CLI 口径）不过滤：同一份过滤表达式，两种范围
    assert_eq!(s.list_schedules(Scope::Local).await.unwrap().len(), 1);
    assert_eq!(s.list_pipelines(Scope::Local).await.unwrap().len(), 1);
}

/// 写越权：B 不能拿同一个 id 覆盖 A 的任务。
///
/// 这条比"读不到"更要紧：id 是主键、写入是 `ON CONFLICT DO UPDATE`，
/// 只过滤读的话越权会从"看得到"升级成"改得掉"。
#[tokio::test]
async fn a_cannot_overwrite_another_users_schedule() {
    let s = store();
    let (a, b) = two_users(&s).await;

    let mut mine = schedule("sched-1");
    mine.owner = Some(a.clone());
    s.upsert_schedule(Scope::user(&a), &mine).await.unwrap();

    let mut theirs = schedule("sched-1");
    theirs.owner = Some(b.clone());
    theirs.name = "B 的任务".into();
    let err = s
        .upsert_schedule(Scope::user(&b), &theirs)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), mcloud_core::ErrorKind::BadRequest, "{err}");
    assert!(
        format!("{err}").contains("其他用户"),
        "要给出可行动的提示，而不是让调用方猜: {err}"
    );

    let loaded = s
        .get_schedule(Scope::user(&a), "sched-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.name, "每日签到", "A 的行不能被改动");

    // 也不能把自己写的东西写成"无主"（那会立刻从所有人视野里消失）
    let mut orphan = schedule("sched-2");
    orphan.owner = None;
    assert!(
        s.upsert_schedule(Scope::user(&a), &orphan).await.is_err(),
        "Scope::User 不该写出无主的行"
    );
}

/// 未归属的历史数据：**谁都看不见**；认领后归首个管理员，且幂等、不可被抢。
#[tokio::test]
async fn claiming_covers_schedules_and_pipelines() {
    let s = store();
    let (a, _b) = two_users(&s).await;

    // 迁移留下的无主数据（`Scope::Local` 是迁移/CLI 的口径）
    s.upsert_schedule(Scope::Local, &schedule("sched-legacy"))
        .await
        .unwrap();
    s.upsert_pipeline(Scope::Local, &pipeline("pipe-legacy"))
        .await
        .unwrap();

    assert!(
        s.list_schedules(Scope::user(&a)).await.unwrap().is_empty(),
        "无主的定时任务谁都看不见 —— 宁可暂时不出现，也不要谁都拿得到"
    );

    let claimed = s.claim_unowned(&a).await.unwrap();
    assert_eq!(
        (claimed.accounts, claimed.schedules, claimed.pipelines),
        (0, 1, 1)
    );
    assert_eq!(s.list_schedules(Scope::user(&a)).await.unwrap().len(), 1);
    assert_eq!(s.list_pipelines(Scope::user(&a)).await.unwrap().len(), 1);

    assert_eq!(
        s.claim_unowned(&a).await.unwrap(),
        ClaimedRows::default(),
        "认领必须幂等"
    );
    assert_eq!(
        s.claim_unowned(&UserId::new("late-user")).await.unwrap(),
        ClaimedRows::default(),
        "已归属的数据不能被后来者抢走"
    );
}
