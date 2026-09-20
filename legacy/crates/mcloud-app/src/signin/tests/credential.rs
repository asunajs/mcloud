// 由 signin.rs 的 `mod tests` 按主题拆出；公共 helper 见同目录 mod.rs。
use super::*;
use mcloud_core::{AccountId, Scope, StateStore, User, UserId, UserStore, account_label};
use mcloud_io::SqliteStore;
use mcloud_upstream::app_client::AppProfile;
// ───────────────────────── 凭据 ─────────────────────────

#[tokio::test]
async fn credential_uses_the_stored_auth_token_and_the_full_phone() {
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();

    // `Authorization` 的形状必须与真机一致：`mobile:<完整手机号>:<authToken>`
    // —— 第一段是 `mobile`（不是 PC 的 `pc`），中间是**完整**手机号。
    assert_eq!(
        credential.authorization(),
        AppProfile::authorization("19900000001", "AUTHTOKEN")
    );
    let decoded = mcloud_core::parse_auth(credential.authorization()).unwrap();
    assert_eq!(decoded.platform.as_deref(), Some("mobile"));
    assert_eq!(decoded.phone.as_deref(), Some("19900000001"));
    assert_eq!(decoded.token.as_deref(), Some("AUTHTOKEN"));
    assert_eq!(credential.account_id().as_str(), "19****01");
}

#[tokio::test]
async fn credential_debug_never_prints_the_token_or_the_full_phone() {
    let (store, scope, _) = store_with_login().await;
    let credential = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();

    let shown = format!("{credential:?}");
    assert!(!shown.contains("AUTHTOKEN"), "{shown}");
    assert!(!shown.contains("19900000001"), "{shown}");
    assert!(shown.contains("19****01"), "{shown}");
}

#[tokio::test]
async fn credential_refuses_an_account_that_is_not_in_the_store() {
    let (store, scope, _) = store_with_login().await;
    let err = SignInCredential::from_phone(&store, scope, "19900000002")
        .await
        .expect_err("库里没有这个账号就该早失败");
    assert!(format!("{err}").contains("没有账号"), "{err}");
}

#[tokio::test]
async fn credential_refuses_an_empty_auth_token() {
    let (store, scope, _) = store_with_login().await;
    let account_id = AccountId::new(account_label(None, "19900000003"));
    let mut account = mcloud_core::Account::new(account_id.as_str());
    account.owner = Some(UserId::new("u1"));
    store.upsert_account(scope, &account).await.unwrap();
    store
        .upsert_account_state(
            scope,
            &mcloud_core::AccountState {
                account_id,
                // 空串与 None 必须同等对待：拼出来的头只会换来 90001，
                // 而那个错误指向"未登录"，与真实原因（库里存了个空值）不符。
                jwt_token: Some(String::new()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let err = SignInCredential::from_phone(&store, scope, "19900000003")
        .await
        .expect_err("空 authToken 必须被拒");
    assert!(format!("{err}").contains("凭据"), "{err}");
}

/// **v7 的核心**：库里有整串 `Authorization` 时**原样用它** —— 不解析、不重拼。
///
/// 这条同时钉住三件事：
///   ① 整串优先于"`phone` + `jwt_token` 现拼"（两者故意给成不同的值）；
///   ② 不需要 `phone` 列 —— `asign.json` 导入的历史行没有它；
///   ③ 不需要 `jwt_token` 列 —— token 在整串里，所以"只有凭据、没有登录态"的行也能用。
/// ③ 正是迁移回填造出来的形态；闸门顺序一旦排错，它会被误判成"没有凭据"。
///
/// `platform` 故意写成 `2`（不是 `mobile`）：整串自带前缀，那道"端不匹配"的闸
/// 拦的是"我们硬编码 `mobile`"，对这条路径不成立。旧项目同样不做这个区分
/// （`initAccount` 里是 `setAuth({type:"basic", token})`，整串原样用）。
#[tokio::test]
async fn credential_prefers_the_whole_stored_authorization() {
    /// `Basic MjoxOTkwMDAwMDAwMTpTVE9SRUQtVE9LRU4=` = `2:19900000001:STORED-TOKEN`
    const STORED: &str = "Basic MjoxOTkwMDAwMDAwMTpTVE9SRUQtVE9LRU4=";

    let s = SqliteStore::open_in_memory().unwrap();
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-19T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let user = User::new("u1", "alice", "!", mcloud_core::Role::Admin, now);
    s.upsert_user(&user).await.unwrap();

    let account_id = AccountId::new(account_label(None, "19900000001"));
    let mut account = mcloud_core::Account::new(account_id.as_str());
    account.owner = Some(user.id.clone());
    account.platform = Some("2".into());
    s.upsert_account(Scope::Local, &account).await.unwrap();

    s.upsert_account_state(
        Scope::Local,
        &mcloud_core::AccountState {
            account_id: account_id.clone(),
            // 故意给一个**不同**的 token：整串优先时它不该被用上
            jwt_token: Some("REBUILT".into()),
            // `phone` 故意留空：整串里带着号码，这条路径不该需要它
            phone: None,
            authorization: Some(STORED.into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let cred = SignInCredential::from_account(&s, Scope::Local, &account_id)
        .await
        .expect("整串在，就该能用 —— 不需要手机号也不需要 jwt_token");

    assert_eq!(cred.authorization(), STORED, "必须原样用整串，逐字节相同");
    assert_ne!(
        cred.authorization(),
        AppProfile::authorization("19900000001", "REBUILT"),
        "绝不能退回现拼 —— 那正是 v7 要消灭的路径"
    );
    assert_eq!(
        cred.phone(),
        "19900000001",
        "手机号只用于展示，从整串里顺手解出来"
    );
}

/// **缺口收口那条**：只要一个掩码 id，手机号从库里读。
///
/// 与 `from_phone` 走同一条实现，所以拼出来的头必须**逐字节相同** ——
/// 否则"补了个便捷入口"就变成了"多了一条会走偏的路"。
#[tokio::test]
async fn credential_reads_the_phone_from_the_store() {
    let (store, scope, _) = store_with_login().await;
    let id = AccountId::new(account_label(None, "19900000001"));

    let from_account = SignInCredential::from_account(&store, scope, &id)
        .await
        .unwrap();
    let from_phone = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .unwrap();

    assert_eq!(
        from_account.authorization(),
        from_phone.authorization(),
        "两条路径拼出来的 Authorization 必须一模一样"
    );
    assert_eq!(
        from_account.authorization(),
        AppProfile::authorization("19900000001", "AUTHTOKEN")
    );
    assert_eq!(from_account.account_id().as_str(), "19****01");
}

/// `phone` 为 `NULL` 时（v6 之前登录的老账号）报错，且错误里给出**两条出路**。
///
/// 这条刻意断言错误文案里同时出现"重新登录"与 `--phone`：
/// 只报"没有手机号"会把用户晾在那儿 —— 他并不知道库里为什么会缺这个值。
#[tokio::test]
async fn credential_refuses_a_row_without_a_stored_phone() {
    let (store, scope, _) = store_with_legacy_login().await;
    let id = AccountId::new(account_label(None, "19900000001"));

    let err = SignInCredential::from_account(&store, scope, &id)
        .await
        .expect_err("库里没有完整手机号就该早失败，而不是拿掩码去凑");
    let text = format!("{err}");
    assert!(text.contains("重新登录"), "{text}");
    assert!(text.contains("--phone"), "{text}");
    // 关键：不能悄悄退化成用掩码拼头
    assert!(text.contains("掩码"), "要点明「别拿掩码凑」: {text}");

    // 同一个账号走 `--phone` 那条老路径仍然可用（这就是它保留的原因）
    let ok = SignInCredential::from_phone(&store, scope, "19900000001")
        .await
        .expect("显式给号码时应能拼出来");
    assert_eq!(
        ok.authorization(),
        AppProfile::authorization("19900000001", "AUTHTOKEN")
    );
}

/// **PC 端登录的账号要拒掉。**
///
/// 签到走的是 App 端凭据（`Authorization` 前缀 `mobile`），而 PC 登录存下来的
/// 是 PC 端的 token 与 `pc` 前缀。这个组合**我们没有实测过**，
/// 所以宁可让调用方显式去重登一次，也不要发出去试探 ——
/// 那种失败长得像"未登录"，会把人带到"凭据坏了"的错误方向上。
#[tokio::test]
async fn credential_refuses_a_pc_logged_in_account() {
    let (store, scope, _) = store_with_login().await;
    set_platform(&store, scope, Some("pc")).await;
    let id = AccountId::new(account_label(None, "19900000001"));

    let err = SignInCredential::from_account(&store, scope, &id)
        .await
        .expect_err("PC 登录的凭据不该被拿来跑 App 链路");
    let text = format!("{err}");
    assert!(text.contains("--client pc"), "{text}");
    assert!(text.contains("--client app"), "要给出改法: {text}");
}

/// 反向确认那道闸**只新增拒绝、不改变既有行为**：
/// `platform` 缺失（老数据、导入数据）时照旧放行。
///
/// 不收紧的理由：`--phone` 这条老路径一直就是这个行为，
/// 在这里顺手收紧会让一批能用的账号突然用不了，而收益只是"更一致"。
#[tokio::test]
async fn credential_allows_an_account_whose_platform_is_unknown() {
    let (store, scope, _) = store_with_login().await;
    set_platform(&store, scope, None).await;
    let id = AccountId::new(account_label(None, "19900000001"));

    let credential = SignInCredential::from_account(&store, scope, &id)
        .await
        .expect("登录端未知时不该拦");
    assert_eq!(
        credential.authorization(),
        AppProfile::authorization("19900000001", "AUTHTOKEN")
    );
}
