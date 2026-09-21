// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use crate::store::schema::SCHEMA_VERSION;

use chrono::TimeZone;

#[test]
fn schema_version_is_recorded() {
    let s = store();
    assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
}

#[tokio::test]
async fn account_roundtrip() {
    let s = store_with_user().await;
    let mut acc = account("acc-1");
    acc.cloud_current = Some(1286);
    acc.auth_expire = Some(1_893_456_000_000);
    acc.note = Some("云图书馆阅读月卡".into());

    s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

    let loaded = s
        .get_account(Scope::user(&uid()), &AccountId::new("acc-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.nickname.as_deref(), Some("账号2"));
    assert_eq!(loaded.cloud_current, Some(1286));
    assert_eq!(loaded.auth_expire, Some(1_893_456_000_000));
    assert_eq!(loaded.note.as_deref(), Some("云图书馆阅读月卡"));
    assert!(loaded.enabled);
}

#[tokio::test]
async fn upsert_account_updates_in_place() {
    let s = store_with_user().await;
    let acc = account("acc-1");
    s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

    let mut updated = acc.clone();
    updated.cloud_current = Some(1300);
    updated.fail_streak = 2;
    s.upsert_account(Scope::user(&uid()), &updated)
        .await
        .unwrap();

    let all = s.list_accounts(Scope::user(&uid())).await.unwrap();
    assert_eq!(all.len(), 1, "重复写入不应产生第二条账号");
    assert_eq!(all[0].cloud_current, Some(1300));
    assert_eq!(all[0].fail_streak, 2);
}

/// 窄写的意义：只刷余额两列，不把"读出来时"的其余列一起写回去。
///
/// 反例很具体 —— 若这里改用 [`StateStore::upsert_account`]（整行覆盖），
/// 调用方就得"读整行 → 改余额 → 写回"，而读与写之间别人改的昵称
/// 会被**静默写回旧值**：一个只在列表页显示错名字的 bug，查起来毫无头绪。
#[tokio::test]
async fn set_account_cloud_touches_only_the_two_cloud_columns() {
    let s = store_with_user().await;
    let mut acc = account("acc-1");
    acc.cloud_current = Some(1286);
    acc.fail_streak = 3;
    s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

    let at = Utc.with_ymd_and_hms(2026, 9, 19, 6, 0, 0).unwrap();
    s.set_account_cloud(Scope::user(&uid()), &AccountId::new("acc-1"), 5392, at)
        .await
        .unwrap();

    let loaded = s
        .get_account(Scope::user(&uid()), &AccountId::new("acc-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.cloud_current, Some(5392));
    assert_eq!(loaded.cloud_updated_at, Some(at));
    assert_eq!(
        loaded.nickname.as_deref(),
        Some("账号2"),
        "昵称不该被顺手覆盖"
    );
    assert_eq!(loaded.fail_streak, 3, "健康度不该被顺手清零");
    assert!(loaded.enabled);
    assert_eq!(loaded.owner.as_ref(), Some(&uid()));
}

#[tokio::test]
async fn set_account_cloud_refuses_a_foreign_account() {
    let s = store_with_user().await;
    let mut acc = account("acc-1");
    acc.cloud_current = Some(1286);
    s.upsert_account(Scope::user(&uid()), &acc).await.unwrap();

    let at = Utc.with_ymd_and_hms(2026, 9, 19, 6, 0, 0).unwrap();
    let err = s
        .set_account_cloud(
            Scope::user(&UserId::new("别人")),
            &AccountId::new("acc-1"),
            5392,
            at,
        )
        .await
        .unwrap_err();

    assert_eq!(err.kind(), mcloud_core::ErrorKind::NotFound);
    let loaded = s
        .get_account(Scope::user(&uid()), &AccountId::new("acc-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.cloud_current,
        Some(1286),
        "被拒的写入不该改到任何东西"
    );
}

#[tokio::test]
async fn accounts_are_ordered_by_sort_order() {
    let s = store_with_user().await;
    let mut a2 = account("acc-2");
    a2.sort_order = 2;
    let mut a1 = account("acc-1");
    a1.sort_order = 1;
    s.upsert_account(Scope::user(&uid()), &a2).await.unwrap();
    s.upsert_account(Scope::user(&uid()), &a1).await.unwrap();

    let ids: Vec<String> = s
        .list_accounts(Scope::user(&uid()))
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.id.to_string())
        .collect();
    assert_eq!(ids, vec!["acc-1", "acc-2"]);
}
