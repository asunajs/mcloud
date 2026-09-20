// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::pure;

#[tokio::test]
async fn cloud_stats_upsert_is_keyed_by_account_and_date() {
    let s = store_with_user().await;
    let id = AccountId::new("acc-1");
    // 快照依附于账号：账号行必须先存在且归属于调用者
    put_account(&s, "acc-1").await;

    let day1 = pure::next_cloud_stats(&id, "2026-09-15", 1278, &[]);
    s.upsert_cloud_stats(Scope::user(&uid()), &day1)
        .await
        .unwrap();

    // 同日重复采集应覆盖而不是新增
    let day1_again = pure::next_cloud_stats(&id, "2026-09-15", 1280, &[]);
    s.upsert_cloud_stats(Scope::user(&uid()), &day1_again)
        .await
        .unwrap();

    let stats = s
        .cloud_stats_since(Scope::user(&uid()), &id, "2026-09-01")
        .await
        .unwrap();
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].cloud_count, 1280);
}

#[tokio::test]
async fn cloud_stats_since_filters_and_orders() {
    let s = store_with_user().await;
    let id = AccountId::new("acc-1");
    put_account(&s, "acc-1").await;
    let mut history: Vec<CloudStats> = Vec::new();

    for (date, count) in [
        ("2026-08-01", 900),
        ("2026-09-10", 1200),
        ("2026-09-15", 1278),
    ] {
        let stats = pure::next_cloud_stats(&id, date, count, &history);
        s.upsert_cloud_stats(Scope::user(&uid()), &stats)
            .await
            .unwrap();
        history.push(stats);
    }

    let stats = s
        .cloud_stats_since(Scope::user(&uid()), &id, "2026-09-01")
        .await
        .unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].date, "2026-09-10");
    assert_eq!(stats[1].date, "2026-09-15");
}

#[tokio::test]
async fn cloud_stats_are_isolated_per_account() {
    let s = store_with_user().await;
    let a = AccountId::new("acc-1");
    let b = AccountId::new("acc-2");
    put_account(&s, "acc-1").await;
    put_account(&s, "acc-2").await;
    s.upsert_cloud_stats(
        Scope::user(&uid()),
        &pure::next_cloud_stats(&a, "2026-09-15", 100, &[]),
    )
    .await
    .unwrap();
    s.upsert_cloud_stats(
        Scope::user(&uid()),
        &pure::next_cloud_stats(&b, "2026-09-15", 200, &[]),
    )
    .await
    .unwrap();

    assert_eq!(
        s.cloud_stats_since(Scope::user(&uid()), &a, "2026-01-01")
            .await
            .unwrap()[0]
            .cloud_count,
        100
    );
    assert_eq!(
        s.cloud_stats_since(Scope::user(&uid()), &b, "2026-01-01")
            .await
            .unwrap()[0]
            .cloud_count,
        200
    );
}
