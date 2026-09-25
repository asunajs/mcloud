//! AES-CBC/PKCS7 原语。
//!
//! 原实现用 crypto-js，它按**密钥字节长度**自动选择 AES-128/192/256。
//! 上游几个密钥长度并不一致（例如刷新令牌密钥是 24 字节 = AES-192），
//! 因此这里必须按长度分派，不能写死 128。

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::Engine as _;

use crate::{CryptoError, Result};

type Enc128 = cbc::Encryptor<aes::Aes128>;
type Dec128 = cbc::Decryptor<aes::Aes128>;
type Enc192 = cbc::Encryptor<aes::Aes192>;
type Dec192 = cbc::Decryptor<aes::Aes192>;
type Enc256 = cbc::Encryptor<aes::Aes256>;
type Dec256 = cbc::Decryptor<aes::Aes256>;

/// 通用 AES 固定 IV。
pub const AES_FIXED_IV: &[u8; 16] = b"0102030405060708";

/// 刷新令牌加解密所用的密钥（24 字节 → AES-192）。
pub const REFRESH_TOKEN_KEY: &str = "2olBaQGYnEKoStYomsd1n7ax";

/// 复现 `rt.crypto.aesEncrypt` 的宽容行为。
///
/// 原实现是 `key.substring(0, 16).padEnd(16, "\0")`：**先截断到 16 字节，
/// 不足则补零**，再用固定 IV 做 AES-128-CBC/PKCS7，输出 hex。
///
/// 注意这与 [`aes_cbc_encrypt`] 不同——后者按密钥实际长度分派
/// （24 字节即 AES-192）。两套语义在真实代码里并存，不能混用。
pub fn aes_encrypt_compat(key: &str, message: &str) -> Result<String> {
    let mut k = [0u8; 16];
    let src = key.as_bytes();
    let n = src.len().min(16);
    k[..n].copy_from_slice(&src[..n]);
    let ct = aes_cbc_encrypt(&k, AES_FIXED_IV, message.as_bytes())?;
    Ok(hex::encode(ct))
}

/// AES-CBC/PKCS7 加密，返回原始密文字节。
///
/// 密钥长度必须为 16 / 24 / 32 字节；IV 固定 16 字节。
pub fn aes_cbc_encrypt(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    // 空明文是合法的：PKCS7 会产出完整一块填充。
    // 原实现的向量里就有 key-16-exact__plain-empty 这一条。
    if iv.len() != 16 {
        return Err(CryptoError::InvalidKeyOrIv);
    }
    let mut buf = vec![0u8; plaintext.len() + 16];
    buf[..plaintext.len()].copy_from_slice(plaintext);

    let ct = match key.len() {
        16 => Enc128::new_from_slices(key, iv)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .to_vec(),
        24 => Enc192::new_from_slices(key, iv)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .to_vec(),
        32 => Enc256::new_from_slices(key, iv)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .to_vec(),
        _ => return Err(CryptoError::InvalidKeyOrIv),
    };
    Ok(ct)
}

/// AES-CBC/PKCS7 解密，输入为原始密文字节。
pub fn aes_cbc_decrypt(key: &[u8], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    if iv.len() != 16 {
        return Err(CryptoError::InvalidKeyOrIv);
    }
    let mut buf = ciphertext.to_vec();
    let pt = match key.len() {
        16 => Dec128::new_from_slices(key, iv)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|_| CryptoError::Decrypt)?
            .to_vec(),
        24 => Dec192::new_from_slices(key, iv)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|_| CryptoError::Decrypt)?
            .to_vec(),
        32 => Dec256::new_from_slices(key, iv)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .map_err(|_| CryptoError::Decrypt)?
            .to_vec(),
        _ => return Err(CryptoError::InvalidKeyOrIv),
    };
    Ok(pt)
}

/// 动态 IV 加密：`base64(IV(16B) ‖ AES-CBC(明文))`。
///
/// 这是刷新令牌接口的报文形态。IV 由调用方提供，使测试可控。
pub fn encrypt_with_dynamic_iv(key: &[u8], iv: &[u8; 16], plaintext: &str) -> Result<String> {
    let ct = aes_cbc_encrypt(key, iv, plaintext.as_bytes())?;
    let mut joined = Vec::with_capacity(16 + ct.len());
    joined.extend_from_slice(iv);
    joined.extend_from_slice(&ct);
    Ok(base64::engine::general_purpose::STANDARD.encode(joined))
}

/// 动态 IV 解密，输入为 `base64(IV ‖ CT)`。
pub fn decrypt_with_dynamic_iv(key: &[u8], payload_b64: &str) -> Result<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(payload_b64)?;
    if raw.len() < 32 {
        // 至少 IV + 一个块，否则不可能是合法密文
        return Err(CryptoError::Decrypt);
    }
    let (iv, ct) = raw.split_at(16);
    let pt = aes_cbc_decrypt(key, iv, ct)?;
    Ok(String::from_utf8(pt)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY192: &[u8] = b"2olBaQGYnEKoStYomsd1n7ax";
    const KEY128: &[u8] = b"0102030405060708";

    #[test]
    fn refresh_key_is_192_bit() {
        assert_eq!(REFRESH_TOKEN_KEY.len(), 24, "该密钥是 AES-192");
    }

    #[test]
    fn roundtrip_aes128() {
        let iv = b"0123456789abcdef";
        let ct = aes_cbc_encrypt(KEY128, iv, b"hello").unwrap();
        assert_eq!(aes_cbc_decrypt(KEY128, iv, &ct).unwrap(), b"hello");
    }

    #[test]
    fn roundtrip_aes192() {
        let iv = b"0123456789abcdef";
        let ct = aes_cbc_encrypt(KEY192, iv, b"hello").unwrap();
        assert_eq!(aes_cbc_decrypt(KEY192, iv, &ct).unwrap(), b"hello");
    }

    #[test]
    fn roundtrip_aes256() {
        let key = b"0123456789abcdef0123456789abcdef";
        let iv = b"0123456789abcdef";
        let ct = aes_cbc_encrypt(key, iv, b"hello").unwrap();
        assert_eq!(aes_cbc_decrypt(key, iv, &ct).unwrap(), b"hello");
    }

    #[test]
    fn pkcs7_pads_full_block_when_aligned() {
        let iv = b"0123456789abcdef";
        let ct = aes_cbc_encrypt(KEY128, iv, &[0u8; 16]).unwrap();
        assert_eq!(ct.len(), 32, "整块明文必须补满一整块");
    }

    #[test]
    fn rejects_bad_key_and_iv_lengths() {
        let iv = b"0123456789abcdef";
        assert!(matches!(
            aes_cbc_encrypt(b"short", iv, b"x"),
            Err(CryptoError::InvalidKeyOrIv)
        ));
        assert!(matches!(
            aes_cbc_encrypt(KEY128, b"tooshort", b"x"),
            Err(CryptoError::InvalidKeyOrIv)
        ));
    }

    #[test]
    fn empty_plaintext_yields_one_padding_block() {
        // 与原实现一致：空明文加密后是一个整块
        let iv = b"0123456789abcdef";
        let ct = aes_cbc_encrypt(KEY128, iv, b"").unwrap();
        assert_eq!(ct.len(), 16);
        assert_eq!(aes_cbc_decrypt(KEY128, iv, &ct).unwrap(), b"");
    }

    #[test]
    fn dynamic_iv_roundtrip_uses_192_bit_key() {
        let iv = [7u8; 16];
        let enc = encrypt_with_dynamic_iv(KEY192, &iv, "refresh-me").unwrap();
        assert_eq!(decrypt_with_dynamic_iv(KEY192, &enc).unwrap(), "refresh-me");
    }

    #[test]
    fn dynamic_iv_prefix_is_the_iv() {
        let iv = [9u8; 16];
        let enc = encrypt_with_dynamic_iv(KEY192, &iv, "x").unwrap();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(enc)
            .unwrap();
        assert_eq!(&raw[..16], &iv[..], "前 16 字节必须是 IV");
    }

    #[test]
    fn compat_truncates_long_key_to_16() {
        // 24 字节密钥被截断到前 16 字节
        let long = "2olBaQGYnEKoStYomsd1n7ax";
        let short = "2olBaQGYnEKoStYo";
        assert_eq!(
            aes_encrypt_compat(long, "x").unwrap(),
            aes_encrypt_compat(short, "x").unwrap(),
            "compat 必须截断到 16 字节"
        );
    }

    #[test]
    fn compat_zero_pads_short_key() {
        // 5 字节密钥补零到 16
        let mut padded = b"short".to_vec();
        padded.resize(16, 0);
        let iv = AES_FIXED_IV;
        let expected = hex::encode(aes_cbc_encrypt(&padded, iv, b"x").unwrap());
        assert_eq!(aes_encrypt_compat("short", "x").unwrap(), expected);
    }

    #[test]
    fn compat_differs_from_raw_192_path() {
        // 同一 24 字节密钥，两条路径结果必须不同（AES-128 vs AES-192）
        let key = "2olBaQGYnEKoStYomsd1n7ax";
        let compat = aes_encrypt_compat(key, "x").unwrap();
        let raw = hex::encode(aes_cbc_encrypt(key.as_bytes(), AES_FIXED_IV, b"x").unwrap());
        assert_ne!(compat, raw, "两条路径语义不同，不应相同");
    }

    #[test]
    fn wrong_iv_does_not_yield_plaintext() {
        let ct = aes_cbc_encrypt(KEY192, b"0123456789abcdef", b"secret").unwrap();
        let r = aes_cbc_decrypt(KEY192, b"fedcba9876543210", &ct);
        assert!(r.is_err() || r.unwrap() != b"secret");
    }
}
