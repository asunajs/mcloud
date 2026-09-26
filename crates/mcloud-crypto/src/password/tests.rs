//! 口令哈希测试。
//!
//! 金标对拍在 `tests/golden.rs`（读 `test/golden/password-hash-vectors.json`），
//! 这里测的是**本模块自己的契约**：长度下限、三种结果的分辨、盐的随机性。

use super::*;

// ── 基本往返 ────────────────────────────────────────────────────────────

#[test]
fn hash_then_verify_roundtrip() {
    let h = hash_password("correct-horse-battery").unwrap();
    assert!(h.starts_with("$argon2id$"), "应是 PHC 串：{h}");
    assert!(verify_password("correct-horse-battery", &h).is_ok());
}

#[test]
fn wrong_password_is_mismatch_not_error() {
    let h = hash_password("correct-horse-battery").unwrap();
    assert_eq!(
        verify_password("wrong-horse-battery", &h),
        Err(PasswordError::Mismatch),
        "口令错是 Mismatch，不是 InvalidHash —— 两者处置不同"
    );
}

/// ⭐ 数据损坏与口令错必须分得开。
///
/// 混成一个"认证失败"，排查时就看不出"是口令错还是库坏了"。
///
/// ⚠ 注意边界：`$argon2id$broken` 这种**结构上合法**的 PHC 串
/// （有算法名、无盐无哈希）会走到 `Mismatch` 而不是 `InvalidHash` ——
/// 因为"解析得动"和"内容对"是两件事，而 `password-hash` 认为前者成立。
/// 这不是缺陷：它确实不是**损坏**，只是一个不可能匹配的串。
#[test]
fn corrupt_hash_is_invalid_not_mismatch() {
    for bad in [
        "not-a-phc-string",
        "$argon2id$v=19$m=19456,t=2,p=1$abc$def", // 盐太短
        "",
    ] {
        assert_eq!(
            verify_password("whatever", bad),
            Err(PasswordError::InvalidHash),
            "坏串应是 InvalidHash：{bad}"
        );
    }
}

// ── 长度下限 ────────────────────────────────────────────────────────────

#[test]
fn enforces_minimum_length() {
    assert_eq!(hash_password("short"), Err(PasswordError::TooShort));
    assert_eq!(
        hash_password("1234567"),
        Err(PasswordError::TooShort),
        "7 个字符不够"
    );
    assert!(hash_password("12345678").is_ok(), "8 个字符刚好");
}

/// 长度按**字符**数算，不是字节数 —— 否则"口令带中文"会被错误地拦下。
#[test]
fn length_counts_characters_not_bytes() {
    // 4 个中文字 = 12 字节，但只有 4 个字符 ⇒ 应当被拒
    assert_eq!(hash_password("中文口令"), Err(PasswordError::TooShort));
    // 7 个中文字 = 21 字节，仍只有 7 个字符 ⇒ 按字符数判，应当被拒。
    // 这一条是"按字符不按字节"的关键断言：按字节实现的会放行它。
    assert_eq!(
        hash_password("中文口令八个字"),
        Err(PasswordError::TooShort)
    );
    // 8 个中文字 = 24 字节且 8 个字符 ⇒ 通过
    assert!(hash_password("中文口令八个字符").is_ok());
}

#[test]
fn min_length_is_the_documented_value() {
    // 这条要与 mcloud 的 MinPasswordLen 一致，且要发给前端
    assert_eq!(MIN_PASSWORD_LEN, 8);
}

// ── 盐 ──────────────────────────────────────────────────────────────────

/// ⭐ 同一口令两次哈希必须**不同**（盐是随机的）。
///
/// 相同的话，说明盐被写死或可预测 —— 那样"撞库"从"逐账号算"
/// 变成"算一次查全表"。这是一条**安全**性质，不是格式偏好。
#[test]
fn salt_is_random_so_hashes_differ() {
    let a = hash_password("correct-horse-battery").unwrap();
    let b = hash_password("correct-horse-battery").unwrap();
    assert_ne!(a, b, "两次哈希不该相同 —— 盐必须是随机的");
    // 但两个都能验通过
    assert!(verify_password("correct-horse-battery", &a).is_ok());
    assert!(verify_password("correct-horse-battery", &b).is_ok());
}

#[test]
fn explicit_salt_is_deterministic() {
    let salt = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let a = hash_password_with_salt("correct-horse", &salt).unwrap();
    let b = hash_password_with_salt("correct-horse", &salt).unwrap();
    assert_eq!(a, b, "给定盐就必须可复现 —— 金标对拍依赖这条");
    assert!(verify_password("correct-horse", &a).is_ok());
}

/// 空口令直接拒（长度下限已经覆盖，这里锁住"不是 panic"）。
#[test]
fn empty_password_is_rejected_without_panic() {
    assert_eq!(hash_password(""), Err(PasswordError::TooShort));
}

/// 结构合法但内容不可能的串 ⇒ `Mismatch`（不是 `InvalidHash`）。
///
/// 锁住上面那条边界，免得将来有人"顺手"把它归到 InvalidHash，
/// 而那会让"数据库里有一行半截数据"看起来像"口令输错了"。
#[test]
fn structurally_valid_but_contentless_hash_is_mismatch() {
    assert_eq!(
        verify_password("whatever", "$argon2id$broken"),
        Err(PasswordError::Mismatch)
    );
}

// ── 会话令牌 ────────────────────────────────────────────────────────────

#[test]
fn session_tokens_are_unique_and_long() {
    let a = new_session_token();
    let b = new_session_token();
    assert_ne!(a, b, "令牌必须唯一");
    assert_eq!(a.len(), 64, "32 字节 = 64 hex");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

/// 令牌存的是哈希，不是原文。
#[test]
fn session_token_is_stored_as_a_hash() {
    let t = new_session_token();
    let h = hash_session_token(&t);
    assert_ne!(h, t, "不能存原文 —— 拿到库就等于能冒充任何人");
    assert_eq!(h.len(), 64, "SHA-256 hex");
    assert_eq!(hash_session_token(&t), h, "同一令牌必须得到同一哈希");
}

#[test]
fn different_tokens_hash_differently() {
    assert_ne!(
        hash_session_token(&new_session_token()),
        hash_session_token(&new_session_token())
    );
}
