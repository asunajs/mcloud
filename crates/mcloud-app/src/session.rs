//! **登录本系统**的会话（WebUI 用；**CLI 刻意不用**）。
//!
//! # 为什么 CLI 不用它
//!
//! 数据库就在本机：能跑 CLI 的人本来就能直接 `sqlite3` 读库，
//! 给他加一道"你是谁"的关卡**不增加任何安全性**，却把本地运维变别扭
//! （列个账号都要先登录）。所以 CLI 走 [`mcloud_core::Scope::Local`]：
//! 本地持有库文件的人本来就能操作所有数据，这在逻辑上是一句实话，
//! 而不是一个权限漏洞。
//!
//! 真正需要会话的是**网络路径**：WebUI 的用户拿不到库文件，
//! "A 不能看到 B 的账号"才有意义 —— 那里每个请求都带 [`mcloud_core::Session`]，
//! 与会话表对照后才决定 `Scope::User(谁)`。
//!
//! # 网络与会话要做的事（P5）
//!
//! 身份来自 `sessions` 表、到期由 `expires_at` 决定、撤销就是删一行。
//! 这一层现在就已具备可用的实现与测试，等 HTTP 层接上即可。
//!
//! # 会话文件里存的是**哈希**
//!
//! 明文 token 只在签发那一刻返回一次（见 `mcloud_core::new_session_token`）。
//! 本地文件放 `sha256(token)`，权威信息（用户、到期）全在库里：
//! 文件只回答"这台机器的当前会话是哪一个"。
//!
//! 这样设计的直接好处是**撤销立刻生效** —— 因为查不到会话记录，
//! 而不是因为哪个文件被删了。若把明文 token 写进文件，就又多了一份
//! "能被拿来冒充登录"的东西，而它并不带来任何额外能力。
//!
//! # CLI 与 WebUI 的差异只在"怎么携带"
//!
//! 两者共用 [`mcloud_core::Session`] 与同一套校验（[`Session::is_valid_at`]）：
//! CLI 通过本地文件携带，WebUI 通过 cookie 携带（方案 §3.7）。

use chrono::{DateTime, Duration, Utc};
use mcloud_core::{
    Error, Random, Session, SessionKind, User, UserId, UserStore, hash_session_token,
    new_session_token, verify_password,
};
use std::path::{Path, PathBuf};

/// CLI 会话的有效期（天）。
///
/// CLI 是本机单机工具，登录态"可长期有效"（方案 §3.7），
/// 但仍然给一个**明确的到期时间**：无到期的会话既清不掉，
/// 也答不了"还能用多久"。
pub const CLI_SESSION_DAYS: i64 = 365;

/// 本机会话文件（只存 token 的哈希，**不存明文**）。
#[derive(Debug, Clone)]
pub struct CliSession {
    path: PathBuf,
}

impl CliSession {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读会话哈希。文件不存在或为空 → `None`（＝未登录）。
    pub fn token_hash(&self) -> Result<Option<String>, Error> {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => {
                let trimmed = raw.trim();
                Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(Error::Internal(format!(
                "读会话文件失败 {}: {err}",
                self.path.display()
            ))),
        }
    }

    /// 记下会话哈希（原子写：先写临时文件再改名）。
    fn remember(&self, token_hash: &str) -> Result<(), Error> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Internal(format!("创建会话目录失败 {}: {e}", parent.display()))
            })?;
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, format!("{token_hash}\n"))
            .map_err(|e| Error::Internal(format!("写会话文件失败 {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| Error::Internal(format!("落盘会话失败 {}: {e}", self.path.display())))
    }

    /// 清掉本机会话记录。返回是否真的删掉了文件。
    pub fn clear(&self) -> Result<bool, Error> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(Error::Internal(format!(
                "删会话文件失败 {}: {err}",
                self.path.display()
            ))),
        }
    }
}

/// 登录本系统：校验口令 → 签发会话。
pub async fn sign_in<S: UserStore>(
    store: &S,
    name: &str,
    password: &str,
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
    let session = Session::new(
        hash_session_token(&token),
        user.id.clone(),
        now + Duration::days(CLI_SESSION_DAYS),
        SessionKind::Cli,
    );
    store.insert_session(&session).await?;

    Ok((user, token, session))
}

/// 登录本系统并把会话记到本机文件（CLI 的用法）。
///
/// 返回值里**不含明文 token** —— 它会短暂存在于栈上（这是必要的），
/// 但调用方拿不到，也就无法顺手打印出来。
pub async fn sign_in_and_remember<S: UserStore>(
    store: &S,
    file: &CliSession,
    name: &str,
    password: &str,
    rng: &mut impl Random,
    now: DateTime<Utc>,
) -> Result<(User, Session), Error> {
    let (user, token, session) = sign_in(store, name, password, rng, now).await?;
    file.remember(&session.token_hash)?;
    // 明文 token 用完即弃：本机使用时"验证身份"靠的是会话文件里的哈希
    drop(token);
    Ok((user, session))
}

/// 解析当前会话对应的用户与会话记录。
///
/// 三种情况都返回 `None`（**刻意不区分**）：没登录过、会话已过期、会话已被撤销。
/// 对调用方来说它们该导出同一个动作 —— 去登录；区分只会诱使人写出
/// "过期了但还能再试试"这种分支。
pub async fn current<S: UserStore>(
    store: &S,
    file: &CliSession,
    now: DateTime<Utc>,
) -> Result<Option<(User, Session)>, Error> {
    let Some(token_hash) = file.token_hash()? else {
        return Ok(None);
    };
    let Some(session) = store.get_session(&token_hash).await? else {
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

/// 登出：删会话记录 + 清本机会话文件。
///
/// 顺序是"先库后文件"：反过来的话，删文件之后如果删库失败，
/// 库里会留下一个**谁也用不到的**会话 —— 而它看起来仍然有效。
pub async fn sign_out<S: UserStore>(store: &S, file: &CliSession) -> Result<bool, Error> {
    let Some(token_hash) = file.token_hash()? else {
        return Ok(false);
    };
    let removed = store.delete_session(&token_hash).await?;
    file.clear()?;
    Ok(removed)
}

/// 当前用户的 id（调用方只关心"是谁"时的简写）。
pub async fn current_user_id<S: UserStore>(
    store: &S,
    file: &CliSession,
    now: DateTime<Utc>,
) -> Result<Option<UserId>, Error> {
    Ok(current(store, file, now).await?.map(|(user, _)| user.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::{Role, hash_password};
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

    async fn store_with_user() -> SqliteStore {
        let store = SqliteStore::open_in_memory().unwrap();
        let user = User::new(
            "alice",
            "alice",
            hash_password("correct-horse", &mut SeqRandom(9)).unwrap(),
            Role::Admin,
            now(),
        );
        store.upsert_user(&user).await.unwrap();
        store
    }

    fn session_file(dir: &Path) -> CliSession {
        CliSession::new(dir.join("cli-session"))
    }

    #[tokio::test]
    async fn sign_in_then_current_resolves_the_user() {
        let store = store_with_user().await;
        let dir = tempfile::tempdir().unwrap();
        let file = session_file(dir.path());

        let (user, session) = sign_in_and_remember(
            &store,
            &file,
            "alice",
            "correct-horse",
            &mut SeqRandom(1),
            now(),
        )
        .await
        .unwrap();
        assert_eq!(user.name, "alice");
        assert_eq!(session.kind, SessionKind::Cli);
        assert!(session.expires_at > now());

        let (current_user, current_session) = current(&store, &file, now()).await.unwrap().unwrap();
        assert_eq!(current_user.id, user.id);
        assert_eq!(current_session.token_hash, session.token_hash);
    }

    /// 会话文件里必须是**哈希**，不能是明文 token。
    ///
    /// 这条断言是有意为之：文件一旦存明文，就凭空多出一份"能冒充登录"的东西，
    /// 而它并不带来任何额外能力（本机使用时查表就够）。
    #[tokio::test]
    async fn session_file_never_contains_the_plaintext_token() {
        let store = store_with_user().await;
        let dir = tempfile::tempdir().unwrap();
        let file = session_file(dir.path());

        let (_, token, session) =
            sign_in(&store, "alice", "correct-horse", &mut SeqRandom(1), now())
                .await
                .unwrap();
        file.remember(&session.token_hash).unwrap();

        let raw = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(raw.trim(), session.token_hash);
        assert_ne!(raw.trim(), token, "文件里不能是明文 token");
        assert_eq!(raw.trim().len(), 64, "SHA-256 十六进制");
    }

    #[tokio::test]
    async fn wrong_password_and_unknown_user_are_indistinguishable() {
        let store = store_with_user().await;
        let dir = tempfile::tempdir().unwrap();
        let file = session_file(dir.path());

        let wrong = sign_in_and_remember(
            &store,
            &file,
            "alice",
            "wrong-horse",
            &mut SeqRandom(1),
            now(),
        )
        .await
        .unwrap_err();
        let unknown = sign_in_and_remember(
            &store,
            &file,
            "nobody",
            "correct-horse",
            &mut SeqRandom(1),
            now(),
        )
        .await
        .unwrap_err();

        // 两种情况文案必须一致：否则登录界面就成了用户名枚举器
        assert_eq!(format!("{wrong}"), format!("{unknown}"));
        assert_eq!(wrong.kind(), mcloud_core::ErrorKind::Auth);
        // 失败不能留下任何会话
        assert!(file.token_hash().unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_or_revoked_session_is_treated_as_logged_out() {
        let store = store_with_user().await;
        let dir = tempfile::tempdir().unwrap();
        let file = session_file(dir.path());
        let (_, session) = sign_in_and_remember(
            &store,
            &file,
            "alice",
            "correct-horse",
            &mut SeqRandom(1),
            now(),
        )
        .await
        .unwrap();

        // ① 还没过期
        assert!(current(&store, &file, now()).await.unwrap().is_some());

        // ② 过了有效期
        let later = session.expires_at + Duration::seconds(1);
        assert!(current(&store, &file, later).await.unwrap().is_none());

        // ③ 撤销（会话行被删）—— 文件还在，但立刻失效
        assert!(store.delete_session(&session.token_hash).await.unwrap());
        assert!(current(&store, &file, now()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn sign_out_removes_both_the_row_and_the_file() {
        let store = store_with_user().await;
        let dir = tempfile::tempdir().unwrap();
        let file = session_file(dir.path());
        let (_, session) = sign_in_and_remember(
            &store,
            &file,
            "alice",
            "correct-horse",
            &mut SeqRandom(1),
            now(),
        )
        .await
        .unwrap();

        assert!(sign_out(&store, &file).await.unwrap());
        assert!(
            store
                .get_session(&session.token_hash)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!file.path().exists());
        // 重复登出不报错，只是"无事可做"
        assert!(!sign_out(&store, &file).await.unwrap());
        assert!(current(&store, &file, now()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_file_means_not_logged_in() {
        let store = store_with_user().await;
        let dir = tempfile::tempdir().unwrap();
        let file = session_file(dir.path());

        assert!(file.token_hash().unwrap().is_none());
        assert!(current(&store, &file, now()).await.unwrap().is_none());
        assert!(
            current_user_id(&store, &file, now())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn two_users_on_one_machine_do_not_share_identity() {
        // 这条是补这个缺口的原始动机：多用户时"我是谁"必须明确
        let store = store_with_user().await;
        let bob = User::new(
            "bob",
            "bob",
            hash_password("correct-horse", &mut SeqRandom(3)).unwrap(),
            Role::User,
            now(),
        );
        store.upsert_user(&bob).await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let file = session_file(dir.path());
        sign_in_and_remember(
            &store,
            &file,
            "bob",
            "correct-horse",
            &mut SeqRandom(1),
            now(),
        )
        .await
        .unwrap();
        // 即便 alice 是"库里第一个用户"，当前身份也必须是 bob
        assert_eq!(
            current_user_id(&store, &file, now())
                .await
                .unwrap()
                .unwrap(),
            bob.id
        );

        sign_in_and_remember(
            &store,
            &file,
            "alice",
            "correct-horse",
            &mut SeqRandom(2),
            now(),
        )
        .await
        .unwrap();
        assert_eq!(
            current_user_id(&store, &file, now())
                .await
                .unwrap()
                .unwrap(),
            UserId::new("alice")
        );
    }
}
