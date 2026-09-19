// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

use chrono::TimeZone;

#[tokio::test]
async fn smid_state_roundtrips_and_is_absent_before_first_bind() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let id = AccountId::new("acc-1");

    // 没上报过的账号：整行都还不存在，读到的是 None 而不是"空状态"
    let never = s.get_smid_state(Scope::user(&uid()), &id).await.unwrap();
    assert!(never.is_none(), "未绑定不该伪造一个空 SmidState");

    s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
        .await
        .unwrap();
    let got = s
        .get_smid_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("写完应读得到");
    assert_eq!(got.account_id, id);
    assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V1));
    assert_eq!(got.device_id, None, "刚绑定还没有 deviceId");

    s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
        .await
        .unwrap();
    let got = s
        .get_smid_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有状态");
    assert_eq!(
        got.fingerprint_json.as_deref(),
        Some(FP_V1),
        "特征不该被 deviceId 写入碰掉"
    );
    assert_eq!(got.device_id.as_deref(), Some(DEVICE_ID_1));
}

/// 换指纹必须连带清掉旧 deviceId —— 旧 id 是**旧长相**在风控那边换来的。
#[tokio::test]
async fn rotating_the_fingerprint_clears_the_stale_device_id() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let id = AccountId::new("acc-1");
    s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
        .await
        .unwrap();
    s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
        .await
        .unwrap();

    s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V2)
        .await
        .unwrap();
    let got = s
        .get_smid_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有状态");
    assert_eq!(got.fingerprint_json.as_deref(), Some(FP_V2));
    assert_eq!(
        got.device_id, None,
        "新长相配旧 id 会被风控认成同一台设备，比没有 id 更糟"
    );
}

/// 窄写的另一面：`set_smid_*` 不能顺手清掉登录态，也不能刷 `updated_at`。
///
/// `updated_at` 的语义是"登录态快照的时间"。一次指纹上报不是新快照 ——
/// 把它刷成 now，会让人以为这个账号刚重新登录过。
#[tokio::test]
async fn smid_writes_leave_credentials_and_updated_at_alone() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let id = AccountId::new("acc-1");
    let at = Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, 0).unwrap();
    s.upsert_account_state(
        Scope::user(&uid()),
        &AccountState {
            account_id: id.clone(),
            jwt_token: Some("JWT-1".into()),
            device_id: Some("own-install-id-ENDIN".into()),
            user_domain_id: Some("domain-1".into()),
            cloud_phone_redpack_token: None,
            phone: Some("19900000001".into()),
            authorization: Some(AUTH_V1.into()),
            updated_at: Some(at),
        },
    )
    .await
    .unwrap();

    s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
        .await
        .unwrap();
    s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
        .await
        .unwrap();

    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("登录态还在");
    assert_eq!(state.jwt_token.as_deref(), Some("JWT-1"));
    assert_eq!(state.device_id.as_deref(), Some("own-install-id-ENDIN"));
    assert_eq!(state.user_domain_id.as_deref(), Some("domain-1"));
    assert_eq!(
        state.authorization.as_deref(),
        Some(AUTH_V1),
        "整串凭据也得原样留着 —— 它是签到唯一需要的那个头"
    );
    assert_eq!(
        state.updated_at,
        Some(at),
        "smid 写入不该把登录态时间刷成 now"
    );
}

/// 没有归属账号就没有 smid 状态：写入被拒，而不是凭空造一行登录态。
#[tokio::test]
async fn smid_writes_require_an_owned_account() {
    let s = store_with_user().await;
    let missing = AccountId::new("no-such-account");
    assert_eq!(
        s.set_smid_fingerprint(Scope::user(&uid()), &missing, FP_V1)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound
    );
    assert_eq!(
        s.set_smid_device_id(Scope::user(&uid()), &missing, DEVICE_ID_1)
            .await
            .unwrap_err()
            .kind(),
        mcloud_core::ErrorKind::NotFound,
        "往不存在的账号上挂 deviceId = 给未来那个同名账号预定设备"
    );
    assert!(
        s.get_smid_state(Scope::user(&uid()), &missing)
            .await
            .unwrap()
            .is_none()
    );
}
