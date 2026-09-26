//! 口令哈希：argon2id 的 PHC 编码串。
//!
//! # 参数与 mcloud 一致，所以金标向量能对拍
//!
//! `m=19456 KiB, t=2, p=1, out=32, salt=16 字节` —— 这正好是
//! [`argon2::Argon2::default()`] 的取值，也是 mcloud 用的那一套。
//!
//! 两边参数一致才能用同一份 `password-hash-vectors.json` 对拍。
//! 对拍的意义：口令哈希是**跨语言互认**的东西 —— 今天 Go 版建的库，
//! Rust 版要能验通过，反之亦然。参数差一个数，两条路就互不认账，
//! 而失败现象是"口令明明对、就是登不上"。
//!
//! # 为什么用 PHC 串而不是自己拼格式
//!
//! PHC 串自带算法、版本与参数：`$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`。
//! 参数变了不必改 schema —— 验证时从串里读参数。
//! 自己拼一个"哈希:盐"的格式，将来调参数就得加列、写迁移、兼容两种形状。
//!
//! # 盐的生成
//!
//! 生产代码用 `OsRng`（操作系统熵源）。**不用时间戳或计数器** ——
//! 那些是可预测的，而盐可预测意味着同一口令在不同账号上产生相同哈希，
//! 于是"撞库"从"逐账号算"变成"算一次查全表"。
//!
//! 金标向量里的盐由 `seed` 派生（`salt[i] = (seed + i) % 256`），
//! 那是为了让对拍**可复现** —— 测试专用，生产路径不走它。

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::CryptoError;

/// 口令长度下限。
///
/// 取 8：与 mcloud 的 `MinPasswordLen` 一致。**这条要发给前端** ——
/// 前端硬编一份必然与它漂移，而漂移的表现是"前端说可以、后端拒绝"。
pub const MIN_PASSWORD_LEN: usize = 8;

/// 口令哈希错误。
///
/// 实现 `PartialEq` 是为了让调用方能**精确断言**三种结果
/// （匹配 / 不匹配 / 串损坏）—— 用 `matches!` 也行，但那样
/// "断言了哪一种"读起来不如相等直观。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    #[error("口令太短：至少 {MIN_PASSWORD_LEN} 个字符")]
    TooShort,

    #[error("口令哈希格式非法")]
    InvalidHash,

    #[error("口令不匹配")]
    Mismatch,
}

/// 生成一个 PHC 编码的 argon2id 哈希。
///
/// 每次调用都用新的随机盐 ⇒ 同一口令两次调用得到**不同**的串。
/// 这是正确的：验证时从串里读盐。
pub fn hash_password(plain: &str) -> Result<String, PasswordError> {
    if plain.chars().count() < MIN_PASSWORD_LEN {
        return Err(PasswordError::TooShort);
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| PasswordError::InvalidHash)
}

/// 用**指定的盐**生成哈希。仅供金标对拍 —— 生产路径用 [`hash_password`]。
///
/// `salt_bytes` 必须是 16 字节。金标向量里那些 `seed` 派生的盐走这条。
pub fn hash_password_with_salt(plain: &str, salt_bytes: &[u8]) -> Result<String, PasswordError> {
    let salt = SaltString::encode_b64(salt_bytes).map_err(|_| PasswordError::InvalidHash)?;
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| PasswordError::InvalidHash)
}

/// 校验口令。
///
/// **三种结果要分清**（对应金标向量里的 `ok` / `mismatch` / `error`）：
///
/// - `Ok(())` —— 匹配
/// - `Err(Mismatch)` —— 串合法但不匹配（口令错）
/// - `Err(InvalidHash)` —— 串本身不合法（数据损坏 / 不是我们的格式）
///
/// 把后两者混成一个"认证失败"，排查时就看不出"是口令错还是库坏了" ——
/// 而这两种情况的处置完全不同（前者正常，后者要修数据）。
pub fn verify_password(plain: &str, encoded: &str) -> Result<(), PasswordError> {
    let parsed = PasswordHash::new(encoded).map_err(|_| PasswordError::InvalidHash)?;
    match Argon2::default().verify_password(plain.as_bytes(), &parsed) {
        Ok(()) => Ok(()),
        Err(argon2::password_hash::Error::Password) => Err(PasswordError::Mismatch),
        Err(_) => Err(PasswordError::InvalidHash),
    }
}

/// 会话令牌：32 字节随机数的 hex。
///
/// 用随机数而不是"用户名 + 时间戳"的派生：派生值可预测，
/// 而会话令牌是**唯一**的认证凭据 —— 猜中一个就等于拿到那个账号。
pub fn new_session_token() -> String {
    use argon2::password_hash::rand_core::RngCore;
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 令牌的存储形态：SHA-256 的 hex。
///
/// # 为什么不存原文
///
/// 会话表会被备份、会被日志打印、会被误读。存原文等于"拿到库就能冒充任何人"。
/// 存哈希后，拿到库也只能验"这个令牌对不对"，推不出原文。
///
/// 这里用**裸 SHA-256** 而不是 argon2：令牌是 32 字节的高熵随机数，
/// 不像口令那样可猜 —— 不需要抗暴力破解的慢哈希，而会话校验是**每个请求**都走，
/// 用 argon2 会让每次 API 调用多几百毫秒。
pub fn hash_session_token(token: &str) -> String {
    crate::sha256_hex(token)
}

/// 把 [`CryptoError`] 转成 [`PasswordError`]（给调用方省一次 `map_err`）。
impl From<CryptoError> for PasswordError {
    fn from(_: CryptoError) -> Self {
        PasswordError::InvalidHash
    }
}

#[cfg(test)]
mod tests;
