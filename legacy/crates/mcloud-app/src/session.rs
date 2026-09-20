//! **网络路径**的会话：登录本系统（WebUI 用；**CLI 刻意不用**）。
//!
//! # 为什么只有网络路径需要"你是谁"
//!
//! 数据库就在本机：能跑 CLI 的人本来就能直接 `sqlite3` 读库，
//! 给他加一道"你是谁"的关卡**不增加任何安全性**，却把本地运维变别扭
//! （列个账号都要先登录）。所以 CLI 走 [`mcloud_core::Scope::Local`]：
//! 本地持有库文件的人本来就能操作所有数据，这在逻辑上是一句实话，
//! 而不是一个权限漏洞。
//!
//! 真正需要会话的是**网络路径**：WebUI 的用户拿不到库文件，
//! "A 不能看到 B 的账号"才有意义 —— 那里每个请求都要先解析出
//! [`mcloud_core::UserId`]，再据此取 `Scope::User(谁)`。
//!
//! # 携带方式：cookie 里是**明文 token**，库里只有哈希
//!
//! - 明文 token 只在签发那一刻返回一次（[`sign_in`] 的返回值），
//!   调用方立刻写进 `Set-Cookie`，**不落任何持久介质**；
//! - 库里存的是 `sha256(token)`（[`mcloud_core::hash_session_token`]）：
//!   拿到库文件也无法直接冒充登录；
//! - 撤销 = 删掉那一行，**立刻生效** —— 而不是"等 cookie 自己过期"。
//!
//! # 三种情况都返回 `None`（刻意不区分）
//!
//! 没带 cookie、会话已过期、会话已被撤销。对调用方来说它们该导出同一个动作 ——
//! 去登录；区分只会诱使人写出"过期了但还能再试试"这种分支。
//!
//! # 这一层不碰 HTTP
//!
//! cookie 的读写、状态码、`Set-Cookie` 属性都在 `mcloud-server`。
//! 放这里就会把 axum 拖进组装层，而 CLI 也要依赖组装层（方案 §3.2）。

use chrono::{DateTime, Duration, Utc};
use mcloud_core::{
    Error, Random, Session, SessionKind, User, UserStore, hash_session_token, new_session_token,
    verify_password,
};

/// WebUI 会话的有效期（天）。
///
/// 与 CLI 那边的 365 天刻意不同：cookie 会被浏览器带着走**每一个**请求，
/// 泄露面比"本机上一个只存哈希的文件"大得多。
///
/// 到期不是**唯一**的保护（撤销立刻生效），所以不必短到几分钟 ——
/// 那会让用户每隔一会儿就被踢回登录页，而安全性并没有实质变化。
/// 若要"记住我"，那是 P6 的产品决定，这里给一个安全且不烦人的默认值。
pub const WEB_SESSION_DAYS: i64 = 7;

/// 登录本系统：校验口令 → 签发会话，返回**明文 token（只此一次）**。
///
/// `kind` / `ttl` 由调用方给：WebUI 用 [`sign_in`]，其它携带方式各自决定。
/// 不在这里写死，是因为"多久算过期"与"这是哪种会话"是**产品决定**，
/// 而库里那一行要如实记下是什么会话。
pub async fn sign_in_with<S: UserStore>(
    store: &S,
    name: &str,
    password: &str,
    kind: SessionKind,
    ttl: Duration,
    rng: &mut impl Random,
    now: DateTime<Utc>,
) -> Result<(User, String, Session), Error> {
    // 用户不存在与口令错误给**同一个**结论：否则登录页会变成用户名枚举器
    let invalid = || Error::Auth("用户名或口令不正确".to_string());

    let user = store.get_user_by_name(name).await?.ok_or_else(invalid)?;
    if !verify_password(password, &user.password_hash)? {
        return Err(invalid());
    }

    let token = new_session_token(rng);
    let session = Session::new(hash_session_token(&token), user.id.clone(), now + ttl, kind);
    store.insert_session(&session).await?;

    Ok((user, token, session))
}

/// WebUI 登录：`SessionKind::Web`，有效期 [`WEB_SESSION_DAYS`] 天。
pub async fn sign_in<S: UserStore>(
    store: &S,
    name: &str,
    password: &str,
    rng: &mut impl Random,
    now: DateTime<Utc>,
) -> Result<(User, String, Session), Error> {
    sign_in_with(
        store,
        name,
        password,
        SessionKind::Web,
        Duration::days(WEB_SESSION_DAYS),
        rng,
        now,
    )
    .await
}

/// 按**明文 token** 解析当前会话（cookie 携带的就是明文）。
///
/// 空 token 直接返回 `None`：`hash_session_token("")` 是一个合法哈希，
/// 万一库里真有个空串派生的会话，就会变成"不带 cookie 也能登录"。
pub async fn current_by_token<S: UserStore>(
    store: &S,
    token: &str,
    now: DateTime<Utc>,
) -> Result<Option<(User, Session)>, Error> {
    if token.is_empty() {
        return Ok(None);
    }
    let Some(session) = store.get_session(&hash_session_token(token)).await? else {
        return Ok(None);
    };
    if !session.is_valid_at(now) {
        return Ok(None);
    }
    // 会话存在但用户被删了（外键 CASCADE 会清会话，这里是双保险）
    let Some(user) = store.get_user(&session.user_id).await? else {
        return Ok(None);
    };
    Ok(Some((user, session)))
}

/// 撤销一个会话（登出）。返回是否真的删掉了一行。
pub async fn sign_out_token<S: UserStore>(store: &S, token: &str) -> Result<bool, Error> {
    if token.is_empty() {
        return Ok(false);
    }
    store.delete_session(&hash_session_token(token)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{Role, UserId, hash_password};
    use mcloud_io::SqliteStore;

    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-18T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    async fn store_with_user(name: &str, password: &str, role: Role) -> SqliteStore {
        let store = SqliteStore::open_in_memory().unwrap();
        let user = User::new(
            name,
            name,
            hash_password(password, &mut SeqRandom(9)).unwrap(),
            role,
            now(),
        );
        store.upsert_user(&user).await.unwrap();
        store
    }

    #[tokio::test]
    async fn sign_in_then_token_resolves_the_user() {
        let store = store_with_user("alice", "correct-horse", Role::Admin).await;

        let (user, token, session) =
            sign_in(&store, "alice", "correct-horse", &mut SeqRandom(1), now())
                .await
                .unwrap();
        assert_eq!(user.name, "alice");
        assert_eq!(session.kind, SessionKind::Web);
        assert_eq!(
            session.expires_at,
            now() + Duration::days(WEB_SESSION_DAYS),
            "到期时间必须能被算出来 —— 否则 Cookie 的 Max-Age 只能靠猜"
        );

        let (current_user, current_session) = current_by_token(&store, &token, now())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current_user.id, user.id);
        assert_eq!(current_session.token_hash, session.token_hash);
    }

    /// 库里必须是**哈希**：明文 token 只存在于签发那一刻的返回值里。
    #[tokio::test]
    async fn the_plaintext_token_never_lands_in_the_database() {
        let store = store_with_user("alice", "correct-horse", Role::Admin).await;
        let (_, token, session) =
            sign_in(&store, "alice", "correct-horse", &mut SeqRandom(1), now())
                .await
                .unwrap();

        assert_ne!(session.token_hash, token, "存哈希，不存明文");
        assert_eq!(session.token_hash, hash_session_token(&token));
        assert_eq!(session.token_hash.len(), 64, "SHA-256 十六进制");
        // 拿明文 token 查不到：库里没有这东西
        assert!(store.get_session(&token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn wrong_password_and_unknown_user_are_indistinguishable() {
        let store = store_with_user("alice", "correct-horse", Role::Admin).await;

        let wrong = sign_in(&store, "alice", "wrong-horse", &mut SeqRandom(1), now())
            .await
            .unwrap_err();
        let unknown = sign_in(&store, "nobody", "correct-horse", &mut SeqRandom(1), now())
            .await
            .unwrap_err();

        // 两种情况文案必须一致：否则登录页就成了用户名枚举器
        assert_eq!(format!("{wrong}"), format!("{unknown}"));
        assert_eq!(wrong.kind(), mcloud_core::ErrorKind::Auth);
        // 失败不能留下任何会话
        assert!(store.list_users().await.unwrap().len() == 1);
    }

    #[tokio::test]
    async fn expired_or_revoked_session_is_treated_as_logged_out() {
        let store = store_with_user("alice", "correct-horse", Role::Admin).await;
        let (_, token, session) =
            sign_in(&store, "alice", "correct-horse", &mut SeqRandom(1), now())
                .await
                .unwrap();

        // ① 还没过期
        assert!(
            current_by_token(&store, &token, now())
                .await
                .unwrap()
                .is_some()
        );

        // ② 过了有效期
        let later = session.expires_at + Duration::seconds(1);
        assert!(
            current_by_token(&store, &token, later)
                .await
                .unwrap()
                .is_none(),
            "过期即未登录"
        );

        // ③ 撤销：立刻失效（不是"等 cookie 过期"）
        assert!(store.delete_session(&session.token_hash).await.unwrap());
        assert!(
            current_by_token(&store, &token, now())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn sign_out_revokes_immediately_and_is_idempotent() {
        let store = store_with_user("alice", "correct-horse", Role::Admin).await;
        let (_, token, session) =
            sign_in(&store, "alice", "correct-horse", &mut SeqRandom(1), now())
                .await
                .unwrap();

        assert!(sign_out_token(&store, &token).await.unwrap());
        assert!(
            store
                .get_session(&session.token_hash)
                .await
                .unwrap()
                .is_none(),
            "撤销就是删掉那一行"
        );
        // 重复登出不报错，只是"无事可做"
        assert!(!sign_out_token(&store, &token).await.unwrap());
        assert!(
            current_by_token(&store, &token, now())
                .await
                .unwrap()
                .is_none()
        );
    }

    /// 空 token 不能被当成"一种有效的 token"。
    #[tokio::test]
    async fn empty_token_is_not_a_session() {
        let store = store_with_user("alice", "correct-horse", Role::Admin).await;
        assert!(current_by_token(&store, "", now()).await.unwrap().is_none());
        assert!(!sign_out_token(&store, "").await.unwrap());
    }

    #[tokio::test]
    async fn two_users_do_not_share_identity() {
        // 补这个模块的原始动机：多用户时"我是谁"必须明确
        let store = store_with_user("alice", "correct-horse", Role::Admin).await;
        let bob = User::new(
            "bob",
            "bob",
            hash_password("correct-horse", &mut SeqRandom(3)).unwrap(),
            Role::User,
            now(),
        );
        store.upsert_user(&bob).await.unwrap();

        let (_, bob_token, _) = sign_in(&store, "bob", "correct-horse", &mut SeqRandom(1), now())
            .await
            .unwrap();
        let (_, alice_token, _) =
            sign_in(&store, "alice", "correct-horse", &mut SeqRandom(2), now())
                .await
                .unwrap();

        // 即便 alice 是"库里第一个用户"，bob 的 token 也必须解析成 bob
        let (who, _) = current_by_token(&store, &bob_token, now())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(who.id, bob.id);
        let (who, _) = current_by_token(&store, &alice_token, now())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(who.id, UserId::new("alice"));

        // 一个 token 只能解析出一个人；两个会话互不影响
        assert!(who.role == Role::Admin);
    }
}
