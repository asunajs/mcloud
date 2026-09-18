//! 系统用户与会话的 SQLite 实现（schema v3）。
//!
//! # 两条**不**放进这一层的判断
//!
//! - **口令校验不在存储层**：这里只按用户名取回 `password_hash`，
//!   校验由 [`mcloud_core::verify_password`] 做（纯计算、可单测）；
//! - **"现在几点"不在存储层**：会话是否有效由调用方传时刻判断
//!   （[`Session::is_valid_at`]），清理过期会话也要显式传 `now`。
//!   存储实现偷偷读时钟，会让"到期那一刻算不算有效"变成不可测的。

use chrono::{DateTime, Utc};
use mcloud_core::{
    Error, Random, Role, Session, SessionKind, User, UserId, UserStore, hash_session_token,
    new_session_token,
};
use rusqlite::{OptionalExtension, params};

use super::{SqliteStore, dt_required, dt_to_string, map_sqlite};

/// 用户名冲突时的友好错误。
///
/// 把 SQLite 的 `UNIQUE constraint failed: users.name` 直接抛给用户，
/// 既看不懂也不该看到表结构；这里只保留"名字被占用"这个结论。
fn map_user_write(err: rusqlite::Error) -> Error {
    if let rusqlite::Error::SqliteFailure(inner, _) = &err
        && inner.code == rusqlite::ErrorCode::ConstraintViolation
    {
        return Error::BadRequest("用户名已被占用".to_string());
    }
    map_sqlite(err)
}

const USER_COLUMNS: &str = "id, name, password_hash, role, created_at";

fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User::new(
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, String>(2)?,
        Role::parse(&row.get::<_, String>(3)?),
        dt_required(row.get::<_, String>(4)?, 4)?,
    ))
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session::new(
        row.get::<_, String>(0)?,
        UserId::new(row.get::<_, String>(1)?),
        dt_required(row.get::<_, String>(2)?, 2)?,
        SessionKind::parse(&row.get::<_, String>(3)?),
    ))
}

impl UserStore for SqliteStore {
    async fn upsert_user(&self, user: &User) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.execute(
            r#"
            INSERT INTO users (id, name, password_hash, role, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                name          = excluded.name,
                password_hash = excluded.password_hash,
                role          = excluded.role
            "#,
            params![
                user.id.as_str(),
                user.name,
                user.password_hash,
                user.role.as_str(),
                dt_to_string(&user.created_at),
            ],
        )
        .map_err(map_user_write)?;
        Ok(())
    }

    async fn get_user_by_name(&self, name: &str) -> Result<Option<User>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 列上的 `COLLATE NOCASE` 让这个 `=` 天然就是大小写不敏感的
        conn.query_row(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE name = ?1"),
            params![name],
            row_to_user,
        )
        .optional()
        .map_err(map_sqlite)
    }

    async fn get_user(&self, id: &UserId) -> Result<Option<User>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.query_row(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
            params![id.as_str()],
            row_to_user,
        )
        .optional()
        .map_err(map_sqlite)
    }

    async fn list_users(&self) -> Result<Vec<User>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {USER_COLUMNS} FROM users ORDER BY created_at ASC, id ASC"
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], row_to_user)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    async fn count_users(&self) -> Result<u64, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
            .map_err(map_sqlite)?;
        Ok(count as u64)
    }

    async fn insert_session(&self, session: &Session) -> Result<(), Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 不做 UPSERT：token_hash 是 256 位随机值的哈希，撞上同一个值只可能是
        // 调用方拿旧 token 重复签发 —— 那是错误用法，报出来比悄悄覆盖好。
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, expires_at, kind) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                session.token_hash,
                session.user_id.as_str(),
                dt_to_string(&session.expires_at),
                session.kind.as_str(),
            ],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    async fn get_session(&self, token_hash: &str) -> Result<Option<Session>, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        conn.query_row(
            "SELECT token_hash, user_id, expires_at, kind FROM sessions WHERE token_hash = ?1",
            params![token_hash],
            row_to_session,
        )
        .optional()
        .map_err(map_sqlite)
    }

    async fn delete_session(&self, token_hash: &str) -> Result<bool, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let affected = conn
            .execute(
                "DELETE FROM sessions WHERE token_hash = ?1",
                params![token_hash],
            )
            .map_err(map_sqlite)?;
        Ok(affected > 0)
    }

    async fn delete_expired_sessions(&self, now: DateTime<Utc>) -> Result<u64, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        // 字符串比较成立的前提：写入与比较都走 `dt_to_string`（统一 RFC 3339、统一 UTC），
        // 此时字典序等价于时间序。
        let affected = conn
            .execute(
                "DELETE FROM sessions WHERE expires_at <= ?1",
                params![dt_to_string(&now)],
            )
            .map_err(map_sqlite)?;
        Ok(affected as u64)
    }

    async fn delete_user_sessions(&self, user_id: &UserId) -> Result<u64, Error> {
        let conn = self.conn.lock().expect("数据库锁中毒");
        let affected = conn
            .execute(
                "DELETE FROM sessions WHERE user_id = ?1",
                params![user_id.as_str()],
            )
            .map_err(map_sqlite)?;
        Ok(affected as u64)
    }
}

/// 签发会话：生成明文 token → 落库**哈希** → 返回明文。
///
/// 做成一个函数而不是让调用方自己拼 `new_session_token` + `hash_session_token`：
/// **漏掉哈希就是明文入库**，而这种事既不会编译失败也不会运行报错。
pub async fn issue_session<S: UserStore + ?Sized>(
    store: &S,
    user_id: &UserId,
    expires_at: DateTime<Utc>,
    kind: SessionKind,
    rng: &mut impl Random,
) -> Result<(String, Session), Error> {
    let token = new_session_token(rng);
    let session = Session::new(
        hash_session_token(&token),
        user_id.clone(),
        expires_at,
        kind,
    );
    store.insert_session(&session).await?;
    Ok((token, session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcloud_core::Scope;
    use mcloud_core::{Account, StateStore, hash_password};

    /// 确定性随机源：让口令哈希与会话 token 可复现。
    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().unwrap()
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    async fn make_user(store: &SqliteStore, name: &str, role: Role) -> User {
        let user = User::new(
            format!("u-{name}"),
            name,
            hash_password("correct-horse", &mut SeqRandom(7)).unwrap(),
            role,
            at("2026-09-18T00:00:00Z"),
        );
        store.upsert_user(&user).await.unwrap();
        user
    }

    #[tokio::test]
    async fn user_round_trips_through_sqlite() {
        let s = store();
        let user = make_user(&s, "alice", Role::Admin).await;

        assert_eq!(s.get_user(&user.id).await.unwrap().unwrap(), user);
        assert_eq!(s.get_user_by_name("alice").await.unwrap().unwrap(), user);
        assert_eq!(s.count_users().await.unwrap(), 1);
        assert_eq!(s.list_users().await.unwrap(), vec![user.clone()]);
        assert!(user.is_admin());
        // 口令哈希必须原样回得来（否则登录会莫名其妙失败）
        assert!(mcloud_core::verify_password("correct-horse", &user.password_hash).unwrap());
    }

    #[tokio::test]
    async fn user_name_is_case_insensitively_unique() {
        let s = store();
        make_user(&s, "Alice", Role::Admin).await;

        // 大小写不同的查询必须命中同一个人 —— 登录路径靠这条
        assert!(s.get_user_by_name("alice").await.unwrap().is_some());
        assert!(s.get_user_by_name("ALICE").await.unwrap().is_some());

        // 换个 id 但用大小写不同的同名 → 必须拒绝
        let dup = User::new(
            "u-other",
            "aLiCe",
            "x",
            Role::User,
            at("2026-09-18T00:00:00Z"),
        );
        let err = s.upsert_user(&dup).await.unwrap_err();
        assert_eq!(err.kind(), mcloud_core::ErrorKind::BadRequest, "{err}");
        assert!(format!("{err}").contains("已被占用"), "{err}");

        // 报错不能把原来的人覆盖掉
        let back = s.get_user_by_name("alice").await.unwrap().unwrap();
        assert_eq!(back.id.as_str(), "u-Alice");
        assert_eq!(s.count_users().await.unwrap(), 1, "不该多出一个人");
    }

    #[tokio::test]
    async fn upsert_user_updates_in_place_without_touching_created_at() {
        let s = store();
        let mut user = make_user(&s, "alice", Role::User).await;

        user.role = Role::Admin;
        user.password_hash = "新的哈希".to_string();
        s.upsert_user(&user).await.unwrap();

        let back = s.get_user(&user.id).await.unwrap().unwrap();
        assert_eq!(back.role, Role::Admin);
        assert_eq!(back.password_hash, "新的哈希");
        assert_eq!(
            back.created_at, user.created_at,
            "created_at 记录的是「何时创建」，不该被 upsert 改掉"
        );
        assert_eq!(s.count_users().await.unwrap(), 1, "不该插成两个人");
    }

    #[tokio::test]
    async fn session_stores_only_the_hash() {
        let s = store();
        let user = make_user(&s, "alice", Role::Admin).await;

        let (token, session) = issue_session(
            &s,
            &user.id,
            at("2026-09-19T00:00:00Z"),
            SessionKind::Web,
            &mut SeqRandom(1),
        )
        .await
        .unwrap();

        assert_ne!(session.token_hash, token, "库里不能是明文");
        assert_eq!(session.token_hash, hash_session_token(&token));

        let back = s.get_session(&session.token_hash).await.unwrap().unwrap();
        assert_eq!(back, session);
        assert!(back.is_valid_at(at("2026-09-18T12:00:00Z")));
        // 拿明文当哈希去查必须查不到 —— 反证"没存明文"
        assert!(s.get_session(&token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn expired_sessions_are_swept_and_fresh_ones_survive() {
        let s = store();
        let user = make_user(&s, "alice", Role::Admin).await;

        let (_, stale) = issue_session(
            &s,
            &user.id,
            at("2026-09-17T00:00:00Z"),
            SessionKind::Cli,
            &mut SeqRandom(1),
        )
        .await
        .unwrap();
        let (_, fresh) = issue_session(
            &s,
            &user.id,
            at("2026-09-20T00:00:00Z"),
            SessionKind::Cli,
            &mut SeqRandom(2),
        )
        .await
        .unwrap();

        let removed = s
            .delete_expired_sessions(at("2026-09-18T00:00:00Z"))
            .await
            .unwrap();
        assert_eq!(removed, 1, "只该清掉过期那一条");
        assert!(s.get_session(&stale.token_hash).await.unwrap().is_none());
        assert!(s.get_session(&fresh.token_hash).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn session_expiring_exactly_now_is_treated_as_expired() {
        // 与 `Session::is_expired_at`（`<=`）保持一致：宁可让用户重登，
        // 也不要放行一个刚好到点的会话
        let s = store();
        let user = make_user(&s, "alice", Role::Admin).await;
        let (_, edge) = issue_session(
            &s,
            &user.id,
            at("2026-09-18T00:00:00Z"),
            SessionKind::Cli,
            &mut SeqRandom(3),
        )
        .await
        .unwrap();

        assert_eq!(
            s.delete_expired_sessions(at("2026-09-18T00:00:00Z"))
                .await
                .unwrap(),
            1
        );
        assert!(s.get_session(&edge.token_hash).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn logout_removes_exactly_one_session() {
        let s = store();
        let user = make_user(&s, "alice", Role::Admin).await;
        let (_, web) = issue_session(
            &s,
            &user.id,
            at("2026-09-19T00:00:00Z"),
            SessionKind::Web,
            &mut SeqRandom(1),
        )
        .await
        .unwrap();
        let (_, cli) = issue_session(
            &s,
            &user.id,
            at("2026-09-19T00:00:00Z"),
            SessionKind::Cli,
            &mut SeqRandom(2),
        )
        .await
        .unwrap();

        assert!(s.delete_session(&web.token_hash).await.unwrap());
        assert!(
            !s.delete_session(&web.token_hash).await.unwrap(),
            "重复删除应返回 false"
        );
        // 登出 WebUI 不该顺手把 CLI 的登录态一起干掉
        assert!(s.get_session(&cli.token_hash).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn all_sessions_of_one_user_can_be_revoked_without_touching_others() {
        let s = store();
        let alice = make_user(&s, "alice", Role::Admin).await;
        let bob = make_user(&s, "bob", Role::User).await;

        for seed in 1..=3 {
            issue_session(
                &s,
                &alice.id,
                at("2026-09-19T00:00:00Z"),
                SessionKind::Web,
                &mut SeqRandom(seed),
            )
            .await
            .unwrap();
        }
        let (_, bob_session) = issue_session(
            &s,
            &bob.id,
            at("2026-09-19T00:00:00Z"),
            SessionKind::Web,
            &mut SeqRandom(9),
        )
        .await
        .unwrap();

        assert_eq!(s.delete_user_sessions(&alice.id).await.unwrap(), 3);
        assert_eq!(s.delete_user_sessions(&alice.id).await.unwrap(), 0);
        assert!(
            s.get_session(&bob_session.token_hash)
                .await
                .unwrap()
                .is_some(),
            "只清自己的，别人的会话不能动"
        );
    }

    #[tokio::test]
    async fn unowned_accounts_are_claimed_exactly_once() {
        let s = store();
        // 模拟"从 v2 升上来、还没初始化管理员"的库：owner 为 NULL 的行。
        //
        // 这里**直接用 SQL 写**而不是走 `upsert_account`：后者现在会拒绝无主写入
        // （无主状态只该由 v3 迁移产生）。测试要模拟的正是迁移留下的那个状态，
        // 所以就得按迁移的方式写 —— 用 API 反而是在测一件不存在的事。
        {
            let conn = s.conn.lock().expect("数据库锁中毒");
            for id in ["19****01", "19****02"] {
                conn.execute(
                    "INSERT INTO accounts (id, enabled, sort_order, fail_streak) \
                     VALUES (?1, 1, 0, 0)",
                    params![id],
                )
                .unwrap();
            }
        }
        let admin = make_user(&s, "admin", Role::Admin).await;
        let other = make_user(&s, "bob", Role::User).await;

        // 未归属时**谁都看不见**：宁可账号暂时不出现，也不要出现"谁都拿得到"
        for account in s.list_accounts(Scope::user(&admin.id)).await.unwrap() {
            assert!(account.owner.is_none());
            assert!(!account.is_visible_to(&admin.id));
        }
        assert!(
            s.list_accounts(Scope::user(&admin.id))
                .await
                .unwrap()
                .is_empty(),
            "未归属的账号不该出现在任何人的列表里"
        );

        assert_eq!(s.claim_unowned_accounts(&admin.id).await.unwrap(), 2);
        assert_eq!(
            s.claim_unowned_accounts(&admin.id).await.unwrap(),
            0,
            "认领必须幂等"
        );
        assert_eq!(
            s.claim_unowned_accounts(&other.id).await.unwrap(),
            0,
            "已归属的账号不能被别人抢走"
        );

        for account in s.list_accounts(Scope::user(&admin.id)).await.unwrap() {
            assert_eq!(account.owner.as_ref(), Some(&admin.id));
            assert!(account.is_visible_to(&admin.id));
            assert!(!account.is_visible_to(&other.id), "交叉可见就是越权");
        }
    }

    #[tokio::test]
    async fn owner_survives_an_account_upsert() {
        // 覆盖写账号时把 owner 一起写丢，是"改个昵称就丢了账号"这类 bug 的常见来源
        let s = store();
        let user = make_user(&s, "alice", Role::Admin).await;
        let mut account = Account::new("19****01");
        account.owner = Some(user.id.clone());
        account.nickname = Some("旧昵称".into());
        s.upsert_account(Scope::user(&user.id), &account)
            .await
            .unwrap();

        account.nickname = Some("新昵称".into());
        s.upsert_account(Scope::user(&user.id), &account)
            .await
            .unwrap();

        let back = s
            .get_account(Scope::user(&user.id), &account.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(back.nickname.as_deref(), Some("新昵称"));
        assert_eq!(back.owner.as_ref(), Some(&user.id), "owner 不能被覆盖丢掉");
    }
}
