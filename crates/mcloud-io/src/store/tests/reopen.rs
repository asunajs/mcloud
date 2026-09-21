// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::UserStore;

#[tokio::test]
async fn history_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcloud.db");

    {
        let s = SqliteStore::open(&path).unwrap();
        // 用户与账号都要落盘：归属是账号的一部分，重启后必须仍然成立
        s.upsert_user(&mcloud_core::User::new(
            TEST_USER,
            TEST_USER,
            "!",
            mcloud_core::Role::Admin,
            Utc::now(),
        ))
        .await
        .unwrap();
        s.upsert_account(Scope::user(&uid()), &account("acc-1"))
            .await
            .unwrap();
    }

    let s = SqliteStore::open(&path).unwrap();
    let all = s.list_accounts(Scope::user(&uid())).await.unwrap();
    assert_eq!(
        all.len(),
        1,
        "重启后数据必须仍在（原 TS 版运行历史仅存内存）"
    );
}

/// 手机号要活过重启 —— 它只在登录时写一次，丢了就得重登才能拿回来。
#[tokio::test]
async fn account_state_phone_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcloud.db");
    let id = AccountId::new("acc-1");

    {
        let s = SqliteStore::open(&path).unwrap();
        s.upsert_user(&mcloud_core::User::new(
            TEST_USER,
            TEST_USER,
            "!",
            mcloud_core::Role::Admin,
            Utc::now(),
        ))
        .await
        .unwrap();
        s.upsert_account(Scope::user(&uid()), &account("acc-1"))
            .await
            .unwrap();
        s.upsert_account_state(
            Scope::user(&uid()),
            &AccountState {
                account_id: id.clone(),
                jwt_token: Some("JWT-1".into()),
                phone: Some("19900000001".into()),
                ..AccountState::new("acc-1")
            },
        )
        .await
        .unwrap();
    }

    let s = SqliteStore::open(&path).unwrap();
    let got = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("重启后仍有登录态");
    assert_eq!(got.phone.as_deref(), Some("19900000001"));
}

/// 两列都要活过重启：绑定是一次性的事，丢了就等于悄悄换了台设备。
#[tokio::test]
async fn smid_state_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcloud.db");
    let id = AccountId::new("acc-1");

    {
        let s = SqliteStore::open(&path).unwrap();
        s.upsert_user(&mcloud_core::User::new(
            TEST_USER,
            TEST_USER,
            "!",
            mcloud_core::Role::Admin,
            Utc::now(),
        ))
        .await
        .unwrap();
        s.upsert_account(Scope::user(&uid()), &account("acc-1"))
            .await
            .unwrap();
        s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
            .await
            .unwrap();
        s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
            .await
            .unwrap();
    }

    let s = SqliteStore::open(&path).unwrap();
    let got = s
        .get_smid_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("重启后两列都还在");
    assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V1));
    assert_eq!(got.device_id.as_deref(), Some(DEVICE_ID_1));
}
