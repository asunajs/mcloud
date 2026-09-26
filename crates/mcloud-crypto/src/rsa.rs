//! RSA PKCS#1 v1.5 公钥加密 —— smid 上报里 `ep` 字段的算法。
//!
//! # 它在这条链路上做什么
//!
//! 客户端每次上报现生成一个 uid，那份 uid 被**原样**用 skland 公钥加密成 `ep`；
//! 同一个 uid 的另一半（`md5(uid)[0..16]`）是 `data` 字段的 AES 密钥。
//! 服务端拿私钥解出 uid，才能解开 `data` —— 两者必须是同一个 uid，
//! 否则"包裹用的 uid"与"解密用的密钥"对不上，上报直接失败。
//!
//! # ⚠ 输出不可对拍
//!
//! PKCS#1 v1.5 的 Type 02 填充含**随机非零字节**，同一明文每次密文都不同。
//! 所以黄金向量里 `rsaPkcs1v15B64` 的 `byteExact: false`，只断言：
//! 输出是 128 字节（1024 位公钥的定长密文）且小于模数。
//! 真正的正确性靠**用私钥解回明文**验证 —— 向量里那份 `rsaSelfKeyPair`
//! 就是为此准备的自持密钥对（测试密钥，可公开）。

use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};

use crate::rand::Random;
use crate::{CryptoError, Result};

/// 把 [`Random`] 适配成 `rsa` 需要的 `CryptoRngCore`。
///
/// 能安全地打上 `CryptoRng` 标记，是因为 [`Random`] 的语义约定就是 CSPRNG
/// （见该 trait 的文档）。
struct RsaRngAdapter<'a, R: Random + ?Sized>(&'a mut R);

impl<R: Random + ?Sized> rsa::rand_core::RngCore for RsaRngAdapter<'_, R> {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.0.fill_bytes(&mut b);
        u32::from_be_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.0.fill_bytes(&mut b);
        u64::from_be_bytes(b)
    }

    fn fill_bytes(&mut self, dst: &mut [u8]) {
        self.0.fill_bytes(dst);
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> std::result::Result<(), rsa::rand_core::Error> {
        self.0.fill_bytes(dst);
        Ok(())
    }
}

impl<R: Random + ?Sized> rsa::rand_core::CryptoRng for RsaRngAdapter<'_, R> {}

/// RSA PKCS#1 v1.5 公钥加密，输出 base64。
///
/// `public_key_spki_der` 是 **SPKI DER** 字节（即 base64 解开后的那一份）。
pub fn rsa_pkcs1v15_encrypt_base64(
    public_key_spki_der: &[u8],
    message: &[u8],
    rng: &mut impl Random,
) -> Result<String> {
    use base64::Engine as _;

    let key = rsa::RsaPublicKey::from_public_key_der(public_key_spki_der)
        .map_err(|e| CryptoError::Internal(format!("RSA 公钥解析失败：{e}")))?;
    let mut adapter = RsaRngAdapter(rng);
    let ct = key
        .encrypt(&mut adapter, rsa::Pkcs1v15Encrypt, message)
        .map_err(|e| CryptoError::Internal(format!("RSA 加密失败：{e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&ct))
}

/// 传入 base64 的 SPKI 公钥（如 [`crate::SKLAND_PUBLIC_KEY_B64`]）的便捷版本。
pub fn rsa_pkcs1v15_encrypt_base64_with_b64_key(
    public_key_b64: &str,
    message: &[u8],
    rng: &mut impl Random,
) -> Result<String> {
    let der = crate::b64_decode(public_key_b64)?;
    rsa_pkcs1v15_encrypt_base64(&der, message, rng)
}

/// RSA PKCS#1 v1.5 私钥解密。
///
/// **仅测试与调试台使用**：生产链路拿不到私钥，而且上报是单向的。
/// 存在的理由与 [`crate::gunzip`] 同理 —— 没有它，"发出去的 ep 里装的
/// 确实是这个账号的 uid"就只能靠"算出了一个 base64 串"来自证，那等于没测。
pub fn rsa_pkcs1v15_decrypt(private_key_pkcs8_der: &[u8], cipher_b64: &str) -> Result<Vec<u8>> {
    let key = rsa::RsaPrivateKey::from_pkcs8_der(private_key_pkcs8_der)
        .map_err(|e| CryptoError::Internal(format!("RSA 私钥解析失败：{e}")))?;
    let ct = crate::b64_decode(cipher_b64)?;
    key.decrypt(rsa::Pkcs1v15Encrypt, &ct)
        .map_err(|e| CryptoError::Parse(format!("RSA 解密失败：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归护栏：**同一明文两次加密必须不同**。
    ///
    /// 这条不是"随手写的性质测试"：如果哪天换成一个确定性填充的实现，
    /// 密文就变成"可预测的 uid 封装"，而这正是 PKCS#1 v1.5 随机填充要避免的。
    #[test]
    fn same_plaintext_encrypts_differently_each_time() {
        let mut rng = crate::SeqRandom::new(1);
        let a =
            rsa_pkcs1v15_encrypt_base64_with_b64_key(crate::SKLAND_PUBLIC_KEY_B64, b"v", &mut rng)
                .unwrap();
        let b =
            rsa_pkcs1v15_encrypt_base64_with_b64_key(crate::SKLAND_PUBLIC_KEY_B64, b"v", &mut rng)
                .unwrap();
        assert_ne!(a, b, "PKCS#1 v1.5 填充含随机字节，两次结果不该相同");
    }

    #[test]
    fn skland_ciphertext_is_128_bytes() {
        let ct = rsa_pkcs1v15_encrypt_base64_with_b64_key(
            crate::SKLAND_PUBLIC_KEY_B64,
            b"some-uid",
            &mut crate::SeqRandom::new(7),
        )
        .unwrap();
        let raw = crate::b64_decode(&ct).unwrap();
        assert_eq!(raw.len(), 128, "1024 位公钥 → 定长 128 字节密文");
    }

    #[test]
    fn rejects_a_key_that_is_not_a_public_key() {
        assert!(rsa_pkcs1v15_encrypt_base64_with_b64_key(
            "bm90LWEta2V5",
            b"x",
            &mut crate::SeqRandom::new(1)
        )
        .is_err());
    }
}
