//! 系统用户与会话。
//!
//! 这是**WebUI 自己的登录闸**，与"移动云盘账号"是两件事：
//!
//! | | 谁的凭证 | 存哪 | 谁用 |
//! |---|---|---|---|
//! | 系统用户 | 你设的用户名/口令 | `users.json` | 浏览器 |
//! | 云盘账号 | 移动云盘的 auth | `accounts.json` | 任务运行时 |
//!
//! 两者不要混：云盘账号是"要操作的对象"，系统用户是"谁能操作"。

use serde::{Deserialize, Serialize};

/// 系统用户标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

impl UserId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
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

/// 口令哈希（argon2id 的 PHC 编码串）。
///
/// # 为什么是独立类型而不是裸 `String`
///
/// 它**自带盐与参数**（所以库里不需要额外的 salt 列），本身不是"可复用凭据"，
/// 但仍不该出现在日志或 API 响应里 —— 与云盘 `auth` 同一条边界：只落本地存储。
///
/// 做成独立类型后，`Debug` 由我们手写 ⇒ 连 `{:?}` 也脱敏。
/// 裸 `String` 字段则在任何一处 `{:?}` 里全泄漏，而那是**静默**的：
/// 打印一次不报错、测试也不红，直到某天日志被人看到。
///
/// ⚠ serde **不受影响**：落库要真值，所以 `Serialize` 输出原文。
/// 这与"打印脱敏"是两条不同的路径，不能混为一谈。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PasswordHash(pub String);

impl PasswordHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<已隐藏>")
    }
}

/// 系统用户。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub name: String,
    /// argon2id 编码串（**自带盐与参数**）。
    pub password_hash: PasswordHash,
    /// 创建时间（毫秒）。
    pub created_at_ms: i64,
}

/// 会话来源。
///
/// 零值是 [`SessionKind::Web`] —— 方向是**保守的**：忘赋值时拿到"有到期的那个"，
/// 比拿到"长期有效的那个"安全。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    /// HTTP 会话（有到期与登出）。
    #[default]
    Web,
    /// 本机会话（CLI）。登录态可长期有效。
    Cli,
}

/// 一个会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// `sha256(token)` 的十六进制。
    ///
    /// **不存原文**：会话表会被备份、会被日志打印、会被误读。
    /// 存原文等于"拿到库就能冒充任何人"。
    pub token_hash: String,
    pub user_id: UserId,
    pub kind: SessionKind,
    /// 创建时间（毫秒）。
    pub created_at_ms: i64,
    /// 过期时间（毫秒）。`0` 表示**不过期**（本机会话）。
    #[serde(default)]
    pub expires_at_ms: i64,
}

impl Session {
    /// 这个会话在 `now_ms` 时是否还有效。
    ///
    /// `expires_at_ms == 0` ⇒ 不过期（CLI 会话）。
    pub fn is_valid_at(&self, now_ms: i64) -> bool {
        self.expires_at_ms == 0 || now_ms < self.expires_at_ms
    }
}

/// WebUI 会话的有效期：7 天。
///
/// 取 7 天而不是"永不过期"：浏览器里的会话是**唯一**的认证凭据，
/// 而本地工具常常一开就是几周。7 天是"不用天天登录"与"不会无限期挂着"的折中。
///
/// CLI 会话**不设过期** —— 它在本机，且 `login` 是显式动作。
pub const WEB_SESSION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// 口令最小长度。
///
/// 按 **Unicode 字符数**算，不是字节数 —— 写成字节数会让"口令至少 8 位"
/// 对中文变成"至少 3 个字"。
///
/// ⚠ 这条要**发给前端**：前端硬编一份必然与它漂移，
/// 而漂移的表现是"前端说可以、后端拒绝"。
pub const MIN_PASSWORD_LEN: usize = 8;

/// 用户名规则：2–32 个字符，不含空白与控制字符。
///
/// 不做更复杂的规则（不许特殊符号之类）—— 用户名不是安全边界，
/// 口令才是。过严的规则只会让人把用户名写成手机号。
pub fn validate_username(name: &str) -> Result<(), String> {
    let n = name.chars().count();
    if n < 2 {
        return Err("用户名至少 2 个字符".into());
    }
    if n > 32 {
        return Err("用户名最多 32 个字符".into());
    }
    if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("用户名不能包含空白或控制字符".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_debug_is_redacted() {
        let h = PasswordHash("$argon2id$v=19$m=19456$SECRET".into());
        let dbg = format!("{h:?}");
        assert!(!dbg.contains("SECRET"), "Debug 必须脱敏：{dbg}");
        assert!(dbg.contains("已隐藏"), "{dbg}");
    }

    /// 脱敏不能影响**落库** —— serde 要输出真值。
    #[test]
    fn password_hash_serializes_the_real_value() {
        let h = PasswordHash("$argon2id$v=19$m=19456,t=2,p=1$salt$hash".into());
        let j = serde_json::to_string(&h).unwrap();
        assert!(j.contains("$argon2id$"), "落库要真值：{j}");
        let back: PasswordHash = serde_json::from_str(&j).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn user_roundtrips() {
        let u = User {
            id: UserId::new("u1"),
            name: "admin".into(),
            password_hash: PasswordHash("$argon2id$...".into()),
            created_at_ms: 1_700_000_000_000,
        };
        let j = serde_json::to_string(&u).unwrap();
        assert!(j.contains("\"password_hash\""), "字段名是 snake_case：{j}");
        assert!(j.contains("\"created_at_ms\""));
        assert_eq!(serde_json::from_str::<User>(&j).unwrap(), u);
    }

    /// ⚠ 用户对象出现在日志里时，口令哈希不能跟着出去。
    #[test]
    fn user_debug_does_not_leak_the_hash() {
        let u = User {
            id: UserId::new("u1"),
            name: "admin".into(),
            password_hash: PasswordHash("$argon2id$SUPERSECRET".into()),
            created_at_ms: 0,
        };
        let dbg = format!("{u:?}");
        assert!(
            !dbg.contains("SUPERSECRET"),
            "用户 Debug 不该泄漏哈希：{dbg}"
        );
    }

    // ── 会话有效性 ──────────────────────────────────────────────────────

    #[test]
    fn web_session_expires() {
        let s = Session {
            token_hash: "h".into(),
            user_id: UserId::new("u1"),
            kind: SessionKind::Web,
            created_at_ms: 0,
            expires_at_ms: 1000,
        };
        assert!(s.is_valid_at(999));
        assert!(!s.is_valid_at(1000), "到点即失效");
        assert!(!s.is_valid_at(1001));
    }

    /// CLI 会话不过期（`expires_at_ms == 0`）。
    #[test]
    fn cli_session_never_expires() {
        let s = Session {
            token_hash: "h".into(),
            user_id: UserId::new("u1"),
            kind: SessionKind::Cli,
            created_at_ms: 0,
            expires_at_ms: 0,
        };
        assert!(s.is_valid_at(i64::MAX));
    }

    /// 零值是 Web（保守方向）：忘赋值时拿到"有到期的那个"。
    #[test]
    fn session_kind_defaults_to_web() {
        assert_eq!(SessionKind::default(), SessionKind::Web);
    }

    #[test]
    fn session_roundtrips_with_lowercase_kind() {
        let s = Session {
            token_hash: "abc".into(),
            user_id: UserId::new("u1"),
            kind: SessionKind::Cli,
            created_at_ms: 1,
            expires_at_ms: 0,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"kind\":\"cli\""), "{j}");
        assert_eq!(serde_json::from_str::<Session>(&j).unwrap(), s);
    }

    // ── 用户名校验 ──────────────────────────────────────────────────────

    #[test]
    fn username_rules() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("管 理员").is_err(), "不能含空白");
        assert!(validate_username("a").is_err(), "太短");
        assert!(validate_username(&"x".repeat(33)).is_err(), "太长");
        assert!(validate_username("a\nb").is_err(), "不能含控制字符");
        assert!(validate_username("管理员").is_ok(), "中文用户名可以");
    }

    #[test]
    fn min_password_len_is_eight() {
        assert_eq!(MIN_PASSWORD_LEN, 8);
    }

    #[test]
    fn web_session_ttl_is_seven_days() {
        assert_eq!(WEB_SESSION_TTL_MS, 7 * 24 * 60 * 60 * 1000);
    }
}
