//! 哈希原语。

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};

/// MD5，小写 hex。用于请求签名。
pub fn md5_hex(input: &str) -> String {
    let mut h = Md5::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

/// SHA-1，**大写** hex。
///
/// 只用于复刻上游协议里的口令字段（PC 端 `dycpwd`）——
/// 那是协议约定，不是本系统的口令存储：本系统的口令走
/// [`crate::hash_password`]（argon2id + 随机盐）。两者**不要混用**。
pub fn sha1_hex_upper(input: &str) -> String {
    let mut h = Sha1::new();
    h.update(input.as_bytes());
    hex::encode_upper(h.finalize())
}

/// SHA-256，小写 hex。
pub fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

/// SHA-256，小写 hex，**吃原始字节**。
///
/// ⚠ 与 [`sha256_hex`] 分开是因为上传那一族要哈希的是**文件的原始字节** ——
/// 那不是 UTF-8 文本，先 `from_utf8` 再哈希会失败或改变内容，
/// 而"哈希与 PUT 上去的字节对不上"在上游那里表现是一次校验失败（方向容易跑偏）。
pub fn sha256_hex_bytes(input: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(input);
    hex::encode(h.finalize())
}

/// SHA-256，大写 hex。密码哈希路径使用。
pub fn sha256_hex_upper(input: &str) -> String {
    sha256_hex(input).to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vectors() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex_upper("abc"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }

    #[test]
    fn sha1_known_vectors_are_uppercase() {
        // 标准 SHA-1 向量；大写是协议要求（客户端 encryptByHash 取 toUpperCase）
        assert_eq!(
            sha1_hex_upper("abc"),
            "A9993E364706816ABA3E25717850C26C9CD0D89D"
        );
        assert_eq!(
            sha1_hex_upper(""),
            "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709"
        );
        // 大小写必须真的不同，否则"大写"这件事没有被钉住
        assert_ne!(sha1_hex_upper("abc"), sha1_hex_upper("abc").to_lowercase());
    }

    #[test]
    fn sha1_is_not_sha256() {
        // 两个原语都叫 sha*_hex_upper，混用的表现是"静默换了一个算法"
        assert_ne!(sha1_hex_upper("abc"), sha256_hex_upper("abc"));
    }
}
