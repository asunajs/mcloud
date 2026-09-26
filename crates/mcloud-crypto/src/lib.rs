//! mcloud-crypto —— 移动云盘（139 / 中国移动云盘）签名与加解密原语。
//!
//! 全部为标准算法，**纯 Rust 实现，无 FFI、无 native 依赖**。
//!
//! 算法细节与来源记录在上游 `mcloud` 仓库；本 crate 只承载实现，
//! 权威性由 `tests/golden.rs` 的向量对拍保证。

pub mod aes;
pub mod des;
pub mod hash;
pub mod sign;

pub use aes::{aes_cbc_decrypt, aes_cbc_encrypt, decrypt_with_dynamic_iv, encrypt_with_dynamic_iv};
pub use des::{des_decrypt_b64, des_encrypt_b64};
pub use hash::{md5_hex, sha256_hex, sha256_hex_upper};
pub use sign::{generate_signature, sign_headers, sign_headers_with, SignHeaders, SALT_MAP};

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("AES 密钥或 IV 长度非法：需要 16 字节")]
    InvalidKeyOrIv,

    #[error("明文不能为空")]
    EmptyInput,

    #[error("解密失败（密钥/IV 错误或填充损坏）")]
    Decrypt,

    #[error("不是合法的 base64：{0}")]
    Base64(#[from] base64::DecodeError),

    #[error("不是合法的 UTF-8：{0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub type Result<T> = std::result::Result<T, CryptoError>;
