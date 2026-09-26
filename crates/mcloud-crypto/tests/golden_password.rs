//! 口令哈希的金标对拍（**跨语言互认**）。
//!
//! 向量由 **RustCrypto argon2 0.5.3** 产出，mcloud（Go）侧也在消费同一份。
//! 两边参数完全一致：`argon2id, v=19, m=19456, t=2, p=1, out=32, salt=16B`。
//!
//! # 为什么这份对拍特别值得做
//!
//! 口令哈希是**跨语言互认**的东西：今天 Go 版建的库，Rust 版要能验通过，
//! 反之亦然。参数差一个数，两条路就互不认账 ——
//! 而失败现象是"口令明明对、就是登不上"，排查起来毫无线索。
//!
//! # 盐是可复现的（仅测试）
//!
//! 向量里的盐由 `seed` 派生：`salt[i] = (seed + i) % 256`，16 字节。
//! 那是为了让对拍**可复现**。生产路径用 `OsRng`，不走这条。
//!
//! # 三种期望值
//!
//! `$verify` 里的 `expect` 有三个取值，本测试逐条分辨：
//!
//! - `ok` —— 匹配
//! - `mismatch` —— 串合法但不匹配
//! - `error` —— 串本身不合法
//!
//! 把后两者混成一个"认证失败"，就看不出"是口令错还是库坏了"。

use std::path::PathBuf;

use mcloud_crypto::password::{hash_password_with_salt, verify_password, PasswordError};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors {
    #[serde(rename = "$params")]
    params: Params,
    #[serde(rename = "$hashPassword")]
    hash_password: Vec<HashCase>,
    #[serde(rename = "$verify")]
    verify: Vec<VerifyCase>,
}

#[derive(Deserialize)]
struct Params {
    algorithm: String,
    #[serde(rename = "mCost")]
    m_cost: u32,
    #[serde(rename = "tCost")]
    t_cost: u32,
    #[serde(rename = "pCost")]
    p_cost: u32,
    #[serde(rename = "outputLen")]
    output_len: usize,
    #[serde(rename = "saltBytes")]
    salt_bytes: usize,
}

#[derive(Deserialize)]
struct HashCase {
    plain: String,
    seed: u64,
    encoded: String,
}

#[derive(Deserialize)]
struct VerifyCase {
    plain: String,
    encoded: String,
    /// `ok` / `mismatch` / `error`
    expect: String,
}

fn load() -> Vectors {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/golden/password-hash-vectors.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到金标向量 {}：{e}", path.display()));
    serde_json::from_str(&text).expect("金标向量解析失败")
}

/// 由 `seed` 派生 16 字节盐：`salt[i] = (seed + i) % 256`。
///
/// 这个式子是从向量的 `encoded` 里**反解出来**的（把 PHC 串的盐段做
/// base64 解码后逐字节比对），不是从文档抄的。
fn salt_from_seed(seed: u64) -> Vec<u8> {
    (0..16u64).map(|i| ((seed + i) % 256) as u8).collect()
}

// ── 参数一致性 ──────────────────────────────────────────────────────────

/// 我们用的参数必须与向量声明的**逐项相同**。
///
/// 这条单独测，是因为参数不一致时其它用例会以"哈希不匹配"的形式失败，
/// 而那个报错不会告诉你"是参数不对"。
#[test]
fn parameters_match_the_vector_declaration() {
    let v = load();
    assert_eq!(v.params.algorithm, "argon2id");
    // argon2::Argon2::default() 就是这套取值
    assert_eq!(v.params.m_cost, 19 * 1024, "m=19456 KiB");
    assert_eq!(v.params.t_cost, 2);
    assert_eq!(v.params.p_cost, 1);
    assert_eq!(v.params.output_len, 32);
    assert_eq!(v.params.salt_bytes, 16);
}

// ── 哈希复现 ────────────────────────────────────────────────────────────

/// ⭐ 给定盐，我们算出的 PHC 串必须与向量**逐字节相同**。
///
/// 这是最强的一条：它同时锁住算法、参数、版本、盐编码与输出长度。
#[test]
fn hashing_reproduces_the_vectors_byte_for_byte() {
    let v = load();
    assert!(!v.hash_password.is_empty(), "向量文件不该是空的");

    for case in &v.hash_password {
        let salt = salt_from_seed(case.seed);
        let got = hash_password_with_salt(&case.plain, &salt)
            .unwrap_or_else(|e| panic!("[seed={}] 哈希失败：{e}", case.seed));
        assert_eq!(
            got, case.encoded,
            "[seed={} plain={:?}] 哈希串不一致\n  得到: {}\n  期望: {}",
            case.seed, case.plain, got, case.encoded
        );
    }
}

/// 反向：向量里的每个串，我们都要能验通过。
///
/// 与上一条不同 —— 上一条证明"我们能算出同样的串"，
/// 这一条证明"我们能验别人算出来的串"（跨语言互认的正方向）。
#[test]
fn verifies_every_hashed_vector() {
    let v = load();
    for case in &v.hash_password {
        assert!(
            verify_password(&case.plain, &case.encoded).is_ok(),
            "[seed={}] 应能验通过：{}",
            case.seed,
            case.encoded
        );
    }
}

// ── 校验的三种结果 ──────────────────────────────────────────────────────

/// ⭐ `ok` / `mismatch` / `error` 三档逐条分辨。
#[test]
fn verification_distinguishes_ok_mismatch_and_error() {
    let v = load();
    assert!(!v.verify.is_empty(), "向量文件不该是空的");

    for case in &v.verify {
        let got = verify_password(&case.plain, &case.encoded);
        match case.expect.as_str() {
            "ok" => assert!(got.is_ok(), "[{}] 期望 ok，实际 {got:?}", case.encoded),
            "mismatch" => assert_eq!(
                got,
                Err(PasswordError::Mismatch),
                "[{}] 期望 mismatch —— 串合法但口令不对",
                case.encoded
            ),
            "error" => assert_eq!(
                got,
                Err(PasswordError::InvalidHash),
                "[{}] 期望 error —— 串本身不合法",
                case.encoded
            ),
            other => panic!("向量里出现了未知的 expect：{other}"),
        }
    }
}

/// 向量覆盖了三档 —— 防止"文件被换成只有 ok 的一版"而不自知。
#[test]
fn vectors_cover_all_three_outcomes() {
    let v = load();
    let mut kinds: Vec<&str> = v.verify.iter().map(|c| c.expect.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    for want in ["ok", "mismatch", "error"] {
        assert!(
            kinds.contains(&want),
            "向量缺少 {want} 这一档；只有 {kinds:?} 时对拍覆盖不全"
        );
    }
}

/// 参数不同（如 m=8）或算法不同（argon2i/d）的串，**都不能**验通过。
///
/// 危险在于它们**长得几乎一样** —— 只有几个字符不同。
/// 若实现忽略了串里的参数而用了自己的默认值，这些用例就会错误地通过。
#[test]
fn does_not_ignore_parameters_embedded_in_the_hash() {
    let v = load();
    let strict = v.verify.iter().filter(|c| c.expect != "ok").count();
    assert!(
        strict >= 4,
        "应有若干条'参数/算法不同'的负例，当前只有 {strict} 条"
    );
}
