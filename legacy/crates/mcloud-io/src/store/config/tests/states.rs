// 由 .rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use mcloud_core::{Scope, StateStore};

#[tokio::test]
async fn account_state_is_stored_apart_from_accounts() {
    let s = store();
    let account = owned_account(&s, "19****01").await;

    let state = mcloud_core::AccountState {
        jwt_token: Some("SECRETJWT".into()),
        device_id: Some("DEV".into()),
        ..mcloud_core::AccountState::new("19****01")
    };
    s.upsert_account_state(Scope::user(&uid()), &state)
        .await
        .unwrap();

    // 读账号列表**不会**带出凭据：两张表没有 JOIN，凭据物理上不在结果里
    let accounts = s.list_accounts(Scope::user(&uid())).await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0], account, "账号行不应被登录态污染");

    let loaded = s
        .get_account_state(Scope::user(&uid()), &AccountId::new("19****01"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.jwt_token.as_deref(), Some("SECRETJWT"));
    assert_eq!(loaded, state);
}

/// 覆盖写而不是合并：`None` 必须能真的清掉旧值。
#[tokio::test]
async fn account_state_upsert_overwrites_instead_of_merging() {
    let s = store();
    let id = AccountId::new("19****01");
    // 登录态依附于账号：账号必须先存在且归属于调用者，否则写入会被隔离校验挡住
    owned_account(&s, "19****01").await;

    s.upsert_account_state(
        Scope::user(&uid()),
        &mcloud_core::AccountState {
            jwt_token: Some("OLD".into()),
            device_id: Some("OLDDEV".into()),
            ..mcloud_core::AccountState::new("19****01")
        },
    )
    .await
    .unwrap();

    s.upsert_account_state(
        Scope::user(&uid()),
        &mcloud_core::AccountState {
            jwt_token: Some("NEW".into()),
            ..mcloud_core::AccountState::new("19****01")
        },
    )
    .await
    .unwrap();

    let loaded = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.jwt_token.as_deref(), Some("NEW"));
    assert_eq!(
        loaded.device_id, None,
        "旧的 deviceId 必须被清掉，否则重新登录后还会拿旧设备号去用"
    );
}
