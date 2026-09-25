//! 哈希原语。

use md5::Md5;
use sha2::{Digest, Sha256};

/// MD5，小写 hex。用于请求签名。
pub fn md5_hex(input: &str) -> String {
    let mut h = Md5::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

/// SHA-256，小写 hex。
pub fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
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
}
