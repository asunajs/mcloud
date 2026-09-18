//! 系统用户、会话，以及"云盘账号归属"——**纯逻辑**部分。
//!
//! # 为什么这一层是新的
//!
//! 旧项目**完全没有**它：`/api/*` 没有任何鉴权中间件，CORS 全开，
//! 账号只是配置数组里的下标。而本项目的目标是"用户用自己的账号登录**本系统**，
//! 每个用户各自管理属于自己的移动云盘账号，数据按用户隔离"，
//! 所以它是一个领域模型，不是"加个中间件"。
//!
//! # 三个概念必须分清（混起来就会设计错）
//!
//! | 概念 | 是什么 | 由谁提供 |
//! | --- | --- | --- |
//! | **系统用户**（[`User`]） | 登录**本系统**的账号 | 我们自己 |
//! | **会话**（[`Session`]） | 用户登录后的凭据 | 我们自己 |
//! | **云盘账号**（[`Account`](crate::Account)） | 移动云盘账号（含 `auth`） | 移动侧，**归属**某个系统用户 |
//!
//! # 本模块只做纯计算
//!
//! 口令哈希（argon2id）与 token 派生（SHA-256）都是纯计算，**不碰 IO** ——
//! 随机源从 [`Random`] trait 注入，所以"同一口令加同一盐得到同一哈希"能在单测里钉死。
//! 持久化在 `mcloud-io`（schema v3），HTTP 层的鉴权在 `mcloud-server`。

use crate::crypto::{Random, sha256_hex};
use crate::error::Error;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 系统用户标识。
///
/// 单独做成 newtype 而不是复用 [`AccountId`](crate::AccountId)：
/// 两者都是字符串，但"把云盘账号 id 当系统用户 id 用"是一个会静默通过类型检查的越权 bug。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

impl UserId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 角色。**只有两级**，不做细粒度授权矩阵（见 `docs/rust-rewrite-plan.md` §9 的非目标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    /// 默认取**最小权限**：新增用户忘了指定角色时，不该默认拿到管理员。
    #[default]
    User,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    /// 从存储里读回。无法识别的值一律降级为 [`Role::User`] ——
    /// 反过来（未知 → Admin）会让一次脏数据变成提权。
    pub fn parse(raw: &str) -> Self {
        match raw {
            "admin" => Self::Admin,
            _ => Self::User,
        }
    }
}

/// 系统用户。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub name: String,
    /// argon2id 编码串（**自带盐与参数**，所以不需要额外的 salt 列）。
    ///
    /// 它本身不是"可复用凭据"，但仍不该出现在日志或 API 响应里 ——
    /// 与 `auth` 同一条边界：只落本地存储。
    pub password_hash: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

/// 手写 `Debug` 而不是 `derive`：`password_hash` 虽然是不可逆的编码串，
/// 但 `{:?}` 是调试时最容易顺手打出来的东西（`println!("{user:?}")`、断言失败信息都会用它），
/// 而它与 `auth` 属同一条边界 —— 只落本地存储。这里只暴露能印的字段。
impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("role", &self.role)
            .field("created_at", &self.created_at)
            .field("password_hash", &"<已隐藏>")
            .finish()
    }
}

impl User {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        password_hash: impl Into<String>,
        role: Role,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: UserId::new(id),
            name: name.into(),
            password_hash: password_hash.into(),
            role,
            created_at,
        }
    }

    pub const fn is_admin(&self) -> bool {
        matches!(self.role, Role::Admin)
    }
}

/// 会话来源。CLI 是本机单机（登录态可长期有效），WebUI 是 HTTP（有到期与登出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Web,
    Cli,
}

impl SessionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Cli => "cli",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "web" => Self::Web,
            _ => Self::Cli,
        }
    }
}

/// 会话。
///
/// **只存 token 的哈希**：明文只在签发那一刻返回一次。这样即便库被读走，
/// 也无法直接拿来冒充登录（与口令存哈希是同一个理由）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// `sha256(token)` 的十六进制。
    pub token_hash: String,
    pub user_id: UserId,
    pub expires_at: DateTime<Utc>,
    pub kind: SessionKind,
}

impl Session {
    pub const fn new(
        token_hash: String,
        user_id: UserId,
        expires_at: DateTime<Utc>,
        kind: SessionKind,
    ) -> Self {
        Self {
            token_hash,
            user_id,
            expires_at,
            kind,
        }
    }

    /// 是否已过期（**时间从外部传入**，不在内部读时钟）。
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }

    /// 当前时刻下是否有效。
    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        !self.is_expired_at(now)
    }
}

// ============================ 口令 ============================

/// 口令最小长度。
///
/// 这是**自建系统的下限**，不是安全策略的全部：真正的防线是 argon2id 的默认参数
/// （Argon2id，m=19456 KiB / t=2 / p=1，见 [`hash_password`]）。
pub const MIN_PASSWORD_LEN: usize = 8;

/// 计算口令哈希（argon2id，默认参数）。
///
/// 盐从 [`Random`] 注入而不是内部取全局随机源：这样"同一口令 + 同一盐 → 同一哈希"
/// 可以在单测里断言，而"两次哈希不同"也能被钉住（盐确实是随机的）。
pub fn hash_password(plain: &str, rng: &mut impl Random) -> Result<String, Error> {
    use argon2::PasswordHasher;

    if plain.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::BadRequest(format!("口令至少 {MIN_PASSWORD_LEN} 位")));
    }

    let mut salt_bytes = [0u8; 16];
    rng.fill_bytes(&mut salt_bytes);
    let salt = argon2::password_hash::SaltString::encode_b64(&salt_bytes)
        .map_err(|e| Error::Internal(format!("盐编码失败: {e}")))?;

    argon2::Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(format!("口令哈希失败: {e}")))
}

/// 校验口令。
///
/// 三个分支必须分清：**匹配**、**不匹配**（正常业务结果）、**哈希串本身非法/算法出错**
/// （环境问题）。把后两者合并会让"库里的哈希被写坏"表现成"用户密码错了"，
/// 从而永远查不到真正的原因。
pub fn verify_password(plain: &str, encoded_hash: &str) -> Result<bool, Error> {
    use argon2::password_hash::PasswordVerifier;

    let parsed = argon2::PasswordHash::new(encoded_hash)
        .map_err(|e| Error::Internal(format!("口令哈希格式非法: {e}")))?;

    match argon2::Argon2::default().verify_password(plain.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(Error::Internal(format!("口令校验失败: {e}"))),
    }
}

// ============================ 会话 token ============================

/// 会话 token 的随机字节数（256 位，足以抵御离线爆破，因此只需 SHA-256 存哈希）。
pub const SESSION_TOKEN_BYTES: usize = 32;

/// 签发一个新的会话 token（**明文，只在这一次返回**）。
pub fn new_session_token(rng: &mut impl Random) -> String {
    let mut bytes = [0u8; SESSION_TOKEN_BYTES];
    rng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 计算会话 token 的存储形式。
///
/// 用 SHA-256 而不是 argon2：token 是 256 位随机值，不存在"弱口令"问题，
/// 而它每个请求都要校验一次 —— 用慢哈希会把并发能力白白吃掉。
pub fn hash_session_token(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

// ============================ 数据隔离 ============================

/// 资源是否对该用户可见。
///
/// # 为什么做成一个函数
///
/// 隔离规则一旦有多个副本，漏掉任何一处就是越权。全项目只走这一个判据，
/// 新增查询时"忘了带 `UserId`"就只能发生在**没有调用它**的地方 —— 那在 review 时是可见的。
///
/// # 为什么返回 `bool` 而不是 `Error`
///
/// "不属于你"与"不存在"必须给出**同一个对外结果**（404，不是 403）：
/// 回 403 等于确认了"这个资源存在"，把账号 id 空间变成可枚举的。
/// 返回 `bool` 是为了让调用方把两种情形收敛到同一个 `None`，
/// 而不是各自决定要不要写一个 `Forbidden`。
pub fn is_visible_to(owner: &UserId, viewer: &UserId) -> bool {
    owner == viewer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性 `Random`，让"同一盐 → 同一哈希"可复现。
    struct SeqRandom(u8);

    impl Random for SeqRandom {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for byte in dst.iter_mut() {
                *byte = self.0;
                self.0 = self.0.wrapping_add(1);
            }
        }
    }

    #[test]
    fn password_roundtrip() {
        let mut rng = SeqRandom(7);
        let hash = hash_password("correct-horse", &mut rng).unwrap();
        assert!(verify_password("correct-horse", &hash).unwrap());
        assert!(!verify_password("wrong-horse", &hash).unwrap());
    }

    #[test]
    fn same_password_with_same_salt_is_reproducible() {
        // 盐注入的意义：同一盐必须得到同一编码串，否则这条链路无法被单测钉住
        let a = hash_password("correct-horse", &mut SeqRandom(1)).unwrap();
        let b = hash_password("correct-horse", &mut SeqRandom(1)).unwrap();
        assert_eq!(a, b);

        // 不同盐必须得到不同串（否则说明盐没起作用）
        let c = hash_password("correct-horse", &mut SeqRandom(2)).unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn password_hash_is_argon2id_not_a_bare_digest() {
        let hash = hash_password("correct-horse", &mut SeqRandom(3)).unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        // 裸 SHA-256 是 64 位十六进制；这里是带参数的 PHC 串
        assert!(hash.contains("$m="), "{hash}");
    }

    #[test]
    fn short_password_is_rejected() {
        let err = hash_password("short", &mut SeqRandom(0)).unwrap_err();
        assert!(format!("{err}").contains("至少"), "{err}");
        assert_eq!(MIN_PASSWORD_LEN, 8);
    }

    #[test]
    fn broken_hash_is_an_error_not_a_wrong_password() {
        // 库里的哈希被写坏 ≠ 用户输错密码。混为一谈会让故障永远查不出来。
        let err = verify_password("whatever", "not-a-phc-string").unwrap_err();
        assert_eq!(err.kind(), crate::ErrorKind::Internal, "{err}");
    }

    #[test]
    fn session_token_is_high_entropy_and_only_hash_is_stored() {
        let token = new_session_token(&mut SeqRandom(9));
        assert_eq!(
            token.len(),
            SESSION_TOKEN_BYTES * 2,
            "应为主体为 64 位十六进制"
        );

        let stored = hash_session_token(&token);
        assert_eq!(stored.len(), 64, "SHA-256 十六进制长度");
        assert_ne!(stored, token, "库里存的不能是明文");
        // 同样的 token 必须得到同样的哈希（否则登录态校验不成立）
        assert_eq!(hash_session_token(&token), stored);
    }

    #[test]
    fn session_expiry_is_pure_and_boundary_is_closed() {
        let expires = DateTime::parse_from_rfc3339("2026-09-18T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let session = Session::new("h".into(), UserId::new("u1"), expires, SessionKind::Web);

        let before = DateTime::parse_from_rfc3339("2026-09-17T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(session.is_valid_at(before));
        // 到期时刻本身算过期（<= 而不是 <）：宁可让用户重登，也不要放行一个刚好到点的会话
        assert!(session.is_expired_at(expires));
        assert!(!session.is_valid_at(expires));
    }

    #[test]
    fn unknown_role_degrades_to_least_privilege() {
        // 反过来（未知 → Admin）会让一次脏数据变成提权
        assert_eq!(Role::parse("admin"), Role::Admin);
        assert_eq!(Role::parse(""), Role::User);
        assert_eq!(Role::parse("root"), Role::User);
        assert_eq!(Role::default(), Role::User);
        assert_eq!(SessionKind::parse("web"), SessionKind::Web);
        assert_eq!(SessionKind::parse("cli"), SessionKind::Cli);
        assert_eq!(SessionKind::parse("unknown"), SessionKind::Cli);
    }

    #[test]
    fn data_isolation_only_matches_the_same_user() {
        let alice = UserId::new("user-alice");
        let bob = UserId::new("user-bob");

        assert!(is_visible_to(&alice, &alice));
        // 交叉读取必须不可见 —— 这是"数据按用户隔离"的最小断言
        assert!(!is_visible_to(&alice, &bob));
        assert!(!is_visible_to(&bob, &alice));
    }

    #[test]
    fn types_round_trip_through_serde_for_storage() {
        // SQLite 里 Role / SessionKind 以文本存储，这里钉住取值与解析一致
        assert_eq!(serde_json::to_value(Role::Admin).unwrap(), "admin");
        assert_eq!(serde_json::to_value(Role::User).unwrap(), "user");
        assert_eq!(serde_json::to_value(SessionKind::Cli).unwrap(), "cli");

        let user = User::new(
            "u1",
            "alice",
            "$argon2id$v=19$m=19456,t=2,p=1$abc$def",
            Role::Admin,
            DateTime::parse_from_rfc3339("2026-09-17T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert!(user.is_admin());
        let json = serde_json::to_string(&user).unwrap();
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(back, user);
    }
}
