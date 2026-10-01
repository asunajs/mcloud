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

// ── 「前端先哈希一次」那一档（用户口径 2026-10-01）────────────────────────

/// ⭐ **对拍向量**：Web 侧（`web/src/ui/password.ts`）算出来的必须与这里逐字节一致。
///
/// 两边不一致的表现是"口令明明对、就是登不上"，而且换台机器、换个浏览器
/// 才复现 —— 所以这里钉死一条公开的向量（`SHA-256("abc")` 是标准测试向量，
/// 随便哪本手册都能查到）。
#[test]
fn client_digest_matches_the_web_side() {
    assert_eq!(
        client_password_digest("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(client_password_digest("").len(), 64);
    // 确定性：同明文两次必须一致（否则建管理员与登录算出来的值不同）
    assert_eq!(
        client_password_digest("correct-horse"),
        client_password_digest("correct-horse")
    );
}

#[test]
fn digest_recognition_is_strict() {
    assert!(is_client_digest(&client_password_digest("x")));
    assert!(!is_client_digest(""), "空串不是摘要");
    assert!(!is_client_digest("correct-horse"), "明文不是摘要");
    assert!(
        !is_client_digest(&"A".repeat(64)),
        "大写 hex 不认 —— Web 侧产出的恒为小写"
    );
    assert!(
        !is_client_digest(&"g".repeat(64)),
        "非 hex 字符不认（否则明文恰好 64 位会被误判）"
    );
    assert!(!is_client_digest(&"a".repeat(63)), "位数不足不是摘要");
}

/// 老记录（没有前缀）必须原样通过 `strip_digest_scheme` ——
/// 它是"升级前那些实例还能登录"的唯一依据。
#[test]
fn scheme_marker_roundtrip_and_legacy_passthrough() {
    let phc = hash_password("correct-horse").unwrap();
    assert!(!is_digest_scheme(&phc), "argon2 裸串是**老档**");

    let marked = mark_digest_scheme(&phc);
    assert!(is_digest_scheme(&marked));
    assert_eq!(
        strip_digest_scheme(&marked),
        phc,
        "去前缀必须还原成能校验的 PHC 串"
    );
    assert_eq!(
        mark_digest_scheme(&marked),
        marked,
        "幂等：别叠成 sha256$sha256$…"
    );

    // 老档：原样返回，不会被砍掉开头
    assert_eq!(strip_digest_scheme(&phc), phc);
    assert!(verify_password("correct-horse", strip_digest_scheme(&marked)).is_ok());
}

/// 请求里带的凭据换算：摘要原样、明文补一次 —— 两条路算出来的必须**相同**。
#[test]
fn to_digest_credential_is_idempotent() {
    let plain = "correct-horse";
    let once = to_digest_credential(plain);
    assert_eq!(once, client_password_digest(plain));
    assert_eq!(
        to_digest_credential(&once),
        once,
        "对摘要再来一次不该变成 sha256(摘要)"
    );
}

/// 新档落库后，**用摘要验得通、用明文（未换算）验不通** ——
/// 后者正是"库里存的不是明文派生值"的证据。
#[test]
fn new_scheme_verifies_the_digest_not_the_plaintext() {
    let plain = "correct-horse";
    let phc = hash_password(&client_password_digest(plain)).unwrap();
    let stored = mark_digest_scheme(&phc);

    assert!(verify_password(&to_digest_credential(plain), strip_digest_scheme(&stored)).is_ok());
    assert_eq!(
        verify_password(plain, strip_digest_scheme(&stored)),
        Err(PasswordError::Mismatch),
        "明文直接压上去必须不匹配 —— 否则说明库里存的还是明文那档"
    );
}
