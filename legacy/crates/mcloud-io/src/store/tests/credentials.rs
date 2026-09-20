// 由 store.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;

/// 回填是**只填空缺**：已有值的列原样保留，哪怕调用方递了一个新值上来。
///
/// 这条是回填能"每次 `migrate` 都跑"的前提。语义写成"有值就覆盖"也不会报错，
/// 只会在某次迁移时把用户重登后的新凭据换成 `asign.json` 里的旧串 ——
/// 而那时没有任何线索指向这里。
#[tokio::test]
async fn fill_account_credentials_only_fills_the_blanks() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let id = AccountId::new("acc-1");

    // ① 行都不存在时：两列都填上（历史账号可能压根没登录态行）
    s.fill_account_credentials(Scope::user(&uid()), &id, Some(AUTH_V1), Some("19900000001"))
        .await
        .unwrap();
    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("回填应当把行造出来");
    assert_eq!(state.authorization.as_deref(), Some(AUTH_V1));
    assert_eq!(state.phone.as_deref(), Some("19900000001"));
    assert_eq!(state.jwt_token, None, "回填不碰别的列");
    assert_eq!(state.updated_at, None, "补缺口不是新快照");

    // ② 已有值时：递新值上去也必须**不动**它
    s.fill_account_credentials(Scope::user(&uid()), &id, Some(AUTH_V2), Some("19900000002"))
        .await
        .unwrap();
    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有登录态");
    assert_eq!(
        state.authorization.as_deref(),
        Some(AUTH_V1),
        "已有凭据必须原样保留 —— 覆盖它就是把新登录换成旧的"
    );
    assert_eq!(state.phone.as_deref(), Some("19900000001"));

    // ③ `None` = "这一列我没有可填的"，不该把已有值清成 NULL
    s.fill_account_credentials(Scope::user(&uid()), &id, None, None)
        .await
        .unwrap();
    let state = s
        .get_account_state(Scope::user(&uid()), &id)
        .await
        .unwrap()
        .expect("有登录态");
    assert_eq!(state.authorization.as_deref(), Some(AUTH_V1));
    assert_eq!(state.phone.as_deref(), Some("19900000001"));
}

/// 回填的归属检查：不能拿本地范围以外的手去补别人的凭据。
#[tokio::test]
async fn fill_account_credentials_requires_an_owned_account() {
    let s = store_with_user().await;
    put_account(&s, "acc-1").await;
    let alice = uid();
    let bob = UserId::new("someone-else");

    // 别人的账号：拒
    assert_eq!(
        s.fill_account_credentials(
            Scope::user(&bob),
            &AccountId::new("acc-1"),
            Some(AUTH_V1),
            None
        )
        .await
        .unwrap_err()
        .kind(),
        mcloud_core::ErrorKind::NotFound
    );
    // 不存在的账号：也拒（否则就是"给未来那个同名账号预定凭据"）
    assert_eq!(
        s.fill_account_credentials(
            Scope::user(&alice),
            &AccountId::new("no-such"),
            Some(AUTH_V1),
            None
        )
        .await
        .unwrap_err()
        .kind(),
        mcloud_core::ErrorKind::NotFound
    );
}
