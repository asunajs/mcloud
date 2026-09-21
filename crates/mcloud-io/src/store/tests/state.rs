// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

/// **schema.rs 的 v5 注释点名的那条**：整体覆盖的登录态写入必须绕开这两列。
///
/// 现实路径很平常：重新登录 → `upsert_account_state`（新快照，`jwt_token` 之外全是
/// `None`）。如果这两列被顺手接进那条 UPDATE 的列表里，绑定就白做了 ——
/// 而登录路径根本不知道它们存在，出事也不会有人怀疑到这里。
#[tokio::test]
async fn upsert_account_state_leaves_the_smid_columns_alone() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let id = AccountId::new("acc-1");
    s.set_smid_fingerprint(Scope::user(&uid()), &id, FP_V1)
        .await
        .unwrap();
    s.set_smid_device_id(Scope::user(&uid()), &id, DEVICE_ID_1)
        .await
        .unwrap();

    // 重新登录：一份只带新 token 的快照，其余字段全是 None
    s.upsert_account_state(
        Scope::user(&uid()),
        &AccountState {
            account_id: id.clone(),
            jwt_token: Some("JWT-2".into()),
            ..AccountState::new("acc-1")
        },
    )
    .await
    .unwrap();

    let smid = s
        .get_smid_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("覆盖登录态之后，smid 两列必须原样还在");
    assert_eq!(smid.fingerprint_json.as_deref(), Some(FP_V1));
    assert_eq!(smid.device_id.as_deref(), Some(DEVICE_ID_1));
    // 顺带确认覆盖本身生效了（否则上一条断言可能只是"没写进去"的假象）
    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有登录态");
    assert_eq!(state.jwt_token.as_deref(), Some("JWT-2"));
    assert_eq!(state.user_domain_id, None, "整体覆盖的语义没被改坏");
}

/// **schema.rs 的 v6 注释点名的那条**：`phone` 与 `smid_*` 相反，**必须**跟着登录态走。
///
/// 这条与上一条是同一个坑的两面：把 `phone` 也做成"独立读写"，
/// 重新登录（换了个手机号）之后库里就会留着**上一个号**，
/// 而 `Authorization` 会被拼成 `新 token + 旧号码` —— 一个服务端只会回
/// "未登录"的、看不出根因的组合。
#[tokio::test]
async fn upsert_account_state_writes_the_phone() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let id = AccountId::new("acc-1");

    // 首次登录：写入完整手机号
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
    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有登录态");
    assert_eq!(state.phone.as_deref(), Some("19900000001"));

    // 重新登录换了号：旧号码必须被覆盖掉，不能留下
    s.upsert_account_state(
        Scope::user(&uid()),
        &AccountState {
            account_id: id.clone(),
            jwt_token: Some("JWT-2".into()),
            phone: Some("19900000002".into()),
            ..AccountState::new("acc-1")
        },
    )
    .await
    .unwrap();
    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有登录态");
    assert_eq!(
        state.phone.as_deref(),
        Some("19900000002"),
        "换号后旧号码不许留在库里"
    );

    // 快照里没给手机号 = "上游这次没给"，写成 NULL 而不是保留旧值
    s.upsert_account_state(
        Scope::user(&uid()),
        &AccountState {
            account_id: id.clone(),
            jwt_token: Some("JWT-3".into()),
            ..AccountState::new("acc-1")
        },
    )
    .await
    .unwrap();
    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有登录态");
    assert_eq!(state.phone, None, "整体覆盖语义：没给就是 NULL");
}
