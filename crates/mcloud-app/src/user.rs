//! 系统用户的启动流程：**首个管理员的初始化**。
//!
//! # 为什么 CLI 与 WebUI 必须共用这一个用例
//!
//! 方案 §3.7 点名了一个坑：**两套 bootstrap**。
//! 如果 CLI 有自己的一套"建第一个用户"、WebUI 首次访问又有一 套，
//! 就会出现"CLI 建了 A、浏览器又建了 B"，而账号归属只有一个 —— 数据从此分家。
//! 所以这里只有一条路径，两个入口都调它。
//!
//! # 为什么"只有从零到一"
//!
//! 初始化只在 `count_users() == 0` 时成立。加第二个用户是**另一件事**：
//! 它涉及权限（谁能建、建成什么角色），混进这里会让"手滑多跑一次 init"
//! 变成"系统里多出一个人"，而且这个人拿的还是管理员。

use chrono::{DateTime, Utc};
use mcloud_core::{
    Error, MIN_PASSWORD_LEN, Random, Role, StateStore, User, UserId, UserStore, hash_password,
};

/// 用户名长度上限。
///
/// 它是**产品约束**（终端里要能对齐打印、URL 里要能放），不是协议限制。
pub const MAX_USERNAME_LEN: usize = 64;

/// 初始化结果。
pub struct InitOutcome {
    pub user: User,
    /// 被认领的历史账号条数。
    ///
    /// 单独返回它是为了让人看见"升级上来的账号去哪了" ——
    /// 从旧项目升上来的用户最关心的就是这个数（0 和 2 是两种心情）。
    pub claimed_accounts: u64,
}

/// 手写 `Debug`：结果里带 `User`（含口令哈希），而调试输出最容易顺手打印。
impl std::fmt::Debug for InitOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitOutcome")
            .field("user", &self.user)
            .field("claimed_accounts", &self.claimed_accounts)
            .finish()
    }
}

/// 用户名 → 稳定 id。
///
/// 直接用名字当 id（不做任何变形）：`mcloud login --user alice` 里的 `alice`
/// 就是标识，用户不需要另记一串 uuid。`users.name` 上有大小写不敏感的唯一约束，
/// 所以撞名会在写入时被拒 —— 而"变形"（比如加哈希后缀）会把"改名"变成"换人"。
pub fn user_id_from_name(name: &str) -> UserId {
    UserId::new(name)
}

/// 校验用户名。空、过长、含空白字符都会被拒。
fn validate_name(name: &str) -> Result<&str, Error> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::BadRequest("用户名不能为空".into()));
    }
    if trimmed.chars().count() > MAX_USERNAME_LEN {
        return Err(Error::BadRequest(format!(
            "用户名最长 {MAX_USERNAME_LEN} 个字符"
        )));
    }
    // 空白字符单独拒掉：它会让"看起来一样"的两个名字变成两个用户，
    // 而在终端里根本分辨不出来
    if trimmed.chars().any(char::is_whitespace) {
        return Err(Error::BadRequest("用户名不能包含空白字符".into()));
    }
    Ok(trimmed)
}

/// 初始化首个管理员，并把无主账号认领给它。
///
/// 口令强度由 `mcloud_core::hash_password` 把关（argon2id + 最小长度），
/// 这里不重复实现一遍 —— 策略只能有一处。
pub async fn init_first_admin<S: UserStore + StateStore>(
    store: &S,
    name: &str,
    password: &str,
    rng: &mut impl Random,
    now: DateTime<Utc>,
) -> Result<InitOutcome, Error> {
    let name = validate_name(name)?;

    let existing = store.count_users().await?;
    if existing > 0 {
        return Err(Error::BadRequest(format!(
            "系统里已经有 {existing} 个用户了。初始化只负责「从零到一」；\
             新增用户要走用户管理（涉及角色权限），不能靠再跑一次初始化"
        )));
    }
    if password.chars().count() < MIN_PASSWORD_LEN {
        // 早失败：先别写用户，免得留下一个"建了但口令太短"的半成品
        return Err(Error::BadRequest(format!("口令至少 {MIN_PASSWORD_LEN} 位")));
    }

    let user = User::new(
        // id 与 name 取同一个值：见 `user_id_from_name` 的说明
        name,
        name,
        hash_password(password, rng)?,
        Role::Admin,
        now,
    );
    store.upsert_user(&user).await?;

    // 认领从旧库迁移过来、还没归属的账号（`owner IS NULL`）。
    // 这是"先 migrate 再 init"顺序下账号的去处；反过来（先 init）则是导入时直接归属。
    let claimed_accounts = store.claim_unowned_accounts(&user.id).await?;

    Ok(InitOutcome {
        user,
        claimed_accounts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().unwrap()
    }

    #[tokio::test]
    async fn creates_an_admin_and_reports_the_claimed_count() {
        let s = store();
        let outcome = init_first_admin(&s, "admin", "correct-horse", &mut SeqRandom(1), now())
            .await
            .unwrap();

        assert_eq!(outcome.user.name, "admin");
        assert_eq!(outcome.user.role, Role::Admin);
        assert!(outcome.user.is_admin());
        // 新库没有无主账号，所以是 0
        assert_eq!(outcome.claimed_accounts, 0);

        // 落库了，而且口令能验回来（口令只以 argon2id 编码串形态存储）
        let stored = s.get_user_by_name("admin").await.unwrap().unwrap();
        assert!(mcloud_core::verify_password("correct-horse", &stored.password_hash).unwrap());
        assert_ne!(stored.password_hash, "correct-horse");

        // "先 migrate 再 init 时历史账号被认领"这条路径在 `mcloud-io::store::user`
        // 的单测里覆盖（那里能造出真正的无主账号，本层造不出来也不该造）。
    }

    #[tokio::test]
    async fn refuses_to_run_twice() {
        let s = store();
        let first = init_first_admin(&s, "admin", "correct-horse", &mut SeqRandom(1), now())
            .await
            .unwrap();
        assert_eq!(first.claimed_accounts, 0);

        let err = init_first_admin(&s, "admin2", "correct-horse", &mut SeqRandom(2), now())
            .await
            .expect_err("第二次初始化必须被拒");
        assert!(format!("{err}").contains("已经有 1 个用户"), "{err}");

        // 而且不能真的建出第二个人
        assert_eq!(s.count_users().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn rejects_weak_password_before_writing_anything() {
        let s = store();
        let err = init_first_admin(&s, "admin", "short", &mut SeqRandom(1), now())
            .await
            .expect_err("短口令必须被拒");
        assert!(format!("{err}").contains("至少"), "{err}");
        assert_eq!(s.count_users().await.unwrap(), 0, "失败不该留下半成品用户");
    }

    #[tokio::test]
    async fn rejects_bad_usernames() {
        let s = store();
        for bad in ["", "   ", "a b", "名字里有 空格"] {
            let err = init_first_admin(&s, bad, "correct-horse", &mut SeqRandom(1), now())
                .await
                .expect_err("非法用户名必须被拒");
            assert_eq!(err.kind(), mcloud_core::ErrorKind::BadRequest, "{err}");
        }
        assert_eq!(s.count_users().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn login_by_name_is_case_insensitive_after_init() {
        let s = store();
        init_first_admin(&s, "Admin", "correct-horse", &mut SeqRandom(1), now())
            .await
            .unwrap();
        // 登录路径靠这个：大小写不同也找得到人
        assert!(s.get_user_by_name("admin").await.unwrap().is_some());
        assert_eq!(
            s.get_user_by_name("admin").await.unwrap().unwrap().name,
            "Admin"
        );
    }
}
