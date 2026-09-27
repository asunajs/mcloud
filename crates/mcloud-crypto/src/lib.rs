//! mcloud-crypto —— 移动云盘（139 / 中国移动云盘）签名与加解密原语。
//!
//! 全部为标准算法，**纯 Rust 实现，无 FFI、无 native 依赖**。
//!
//! 算法细节与来源记录在上游 `mcloud` 仓库；本 crate 只承载实现，
//! 权威性由 `tests/golden.rs` 的向量对拍保证。

pub mod aes;
pub mod des;
pub mod gzip;
pub mod hash;
pub mod password;
pub mod rand;
pub mod rsa;
pub mod sign;
pub mod slide;
pub mod tn;

pub use aes::{
    aes_cbc_decrypt, aes_cbc_encrypt, aes_cbc_fixed_iv_decrypt_hex, aes_ecb_pkcs7_decrypt_hex,
    aes_ecb_pkcs7_encrypt_hex, aes_encrypt_compat, decrypt_with_dynamic_iv,
    encrypt_with_dynamic_iv, shift_head10_to_back, shift_tail10_to_front, AES_FIXED_IV,
    REFRESH_TOKEN_KEY,
};
pub use des::{des_decrypt_b64, des_encrypt_b64};
pub use gzip::{gunzip, gzip_with_os19, gzip_with_os19_base64};
pub use hash::{md5_hex, sha1_hex_upper, sha256_hex, sha256_hex_upper};
pub use password::{
    hash_password, hash_password_with_salt, hash_session_token, new_session_token, verify_password,
    PasswordError, MIN_PASSWORD_LEN,
};
pub use rand::{OsRandom, Random, SeqRandom, SharedRandom};
pub use rsa::{
    rsa_pkcs1v15_decrypt, rsa_pkcs1v15_encrypt_base64, rsa_pkcs1v15_encrypt_base64_with_b64_key,
};
pub use sign::{generate_signature, sign_headers, sign_headers_with, SignHeaders, SALT_MAP};
pub use slide::{
    block_box, decode_slide_image, sobel_magnitude, solve_contour, solve_from_images, solve_slide,
    SlideBox, SlideCandidate, SlideImage, SlideSolution, SLIDE_MAX_DEVIATION, SLIDE_TOP_N,
};
pub use tn::{get_tn, get_tn_md5};

/// 设备指纹（smid）上报里用来包住 uid 的公钥：SPKI DER 的 base64，1024 位。
///
/// 来源是签到页加载的那份风控 SDK 配置（`window._smConf`），
/// 由 `test/golden/app-signin-vectors.json` 的 `SM_PUBLIC_KEY` 锁住 ——
/// 常量哪天被改，`smid_constants_match_the_signin_vectors` 会先炸。
pub const SKLAND_PUBLIC_KEY_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQC8KHAcHbkCn5rxGgGJE+07tY+pt86D/oZ7sA51FaEBv2jgno2TI9zHJVYKJynmiKpixgwUcv93EfWIrU/p/UCs5Vu+odS3I4UBp3R7IZ1A0W01FkumAHYW2PQpMm8ueQKPLUq/idkpG/9b2JDv/qU+Ks36nbUPwlW4CjdfrV+V9QIDAQAB";

/// 标准 base64（带 `=` 填充）编码。
///
/// 公开是因为上游层拼 URL / 组报文都要它，不该为此让 `mcloud-upstream`
/// 自己再背一个 `base64` 依赖。
pub fn b64_encode(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 标准 base64 解码。
pub fn b64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| CryptoError::Parse(format!("base64 解码失败：{e}")))
}

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

    /// 输入不合预期（密文不是合法 hex、gzip 解压失败、RSA 解密失败等）。
    #[error("解析失败：{0}")]
    Parse(String),

    /// 内部不变量被破坏（密钥长度非法、原语库报错等），属于代码缺陷而非输入问题。
    #[error("内部错误（原语）: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, CryptoError>;
