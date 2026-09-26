//! AES-CBC/PKCS7 原语。
//!
//! 原实现用 crypto-js，它按**密钥字节长度**自动选择 AES-128/192/256。
//! 上游几个密钥长度并不一致（例如刷新令牌密钥是 24 字节 = AES-192），
//! 因此这里必须按长度分派，不能写死 128。

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyInit, KeyIvInit};
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

/// 复刻 `key.substring(0, 16).padEnd(16, "\0")`。
///
/// 注意：JS 的 `substring` 按 UTF-16 码元计数。本项目**所有密钥都是 ASCII**
/// （`priId` 是 md5 的前 16 个 hex 字符），因此按字节截断/补零等价；
/// 非 ASCII 密钥会与 JS 不一致（原实现从未使用，仅记录）。
fn fit_key16(key: &str) -> [u8; 16] {
    let mut k = [0u8; 16];
    let src = key.as_bytes();
    let n = src.len().min(16);
    k[..n].copy_from_slice(&src[..n]);
    k
}

/// 复现 `rt.crypto.aesEncrypt` 的宽容行为。
///
/// 原实现是 `key.substring(0, 16).padEnd(16, "\0")`：**先截断到 16 字节，
/// 不足则补零**，再用固定 IV 做 AES-128-CBC/PKCS7，输出 hex。
///
/// 注意这与 [`aes_cbc_encrypt`] 不同——后者按密钥实际长度分派
/// （24 字节即 AES-192）。两套语义在真实代码里并存，不能混用。
pub fn aes_encrypt_compat(key: &str, message: &str) -> Result<String> {
    let k = fit_key16(key);
    let ct = aes_cbc_encrypt(&k, AES_FIXED_IV, message.as_bytes())?;
    Ok(hex::encode(ct))
}

/// 固定 IV 那条路的**反向**：hex 密文 → UTF-8 明文。
///
/// **仅测试与调试台使用**（与 [`crate::rsa_pkcs1v15_decrypt`] 同理：生产链路不需要解）。
/// 存在的理由很具体：smid 上报出去的载荷是单向加密的，光有加密函数就永远
/// 证明不了"发出去的那一份里装的确实是绑定给这个账号的特征"。
pub fn aes_cbc_fixed_iv_decrypt_hex(key: &str, cipher_hex: &str) -> Result<String> {
    let k = fit_key16(key);
    let ct = hex::decode(cipher_hex)
        .map_err(|e| CryptoError::Parse(format!("AES 密文不是合法 hex：{e}")))?;
    let pt = aes_cbc_decrypt(&k, AES_FIXED_IV, &ct)?;
    Ok(String::from_utf8(pt)?)
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

/// AES-ECB/PKCS7 解密，输入是 **hex 字符串**（不是 base64）。
///
/// # 为什么单独一个函数
///
/// PC 客户端 `common/utils.js: decryptByAes` 用的是 AES-128-**ECB**，
/// 与上面那条 CBC 路径**模式不同**，不能互相替代 —— 用错了表现为
/// "解出来是乱码"，而调用方往往会把它当成"密钥不对"，白查半天。
///
/// 这里按块调用 `aes` crate 的 `BlockDecrypt`，而不是新增 `ecb` 依赖：
/// 该模式的全部语义就是"逐块、无链接"，手写即是它的全部内容，引依赖
/// 只多一层版本面。ECB 是**上游协议规定的**形态，不是我们的选择
/// （ECB 本身不安全，这里只用于兼容既有协议）。
pub fn aes_ecb_pkcs7_decrypt_hex(key: &str, ciphertext_hex: &str) -> Result<String> {
    // 密钥按**字节长度**分派，与 CBC 路径同口径（上游几个密钥长度不一致）。
    let ct = hex::decode(ciphertext_hex.trim())
        .map_err(|e| CryptoError::Parse(format!("AES 密文不是合法 hex：{e}")))?;
    if ct.is_empty() || ct.len() % 16 != 0 {
        return Err(CryptoError::Decrypt);
    }

    let mut out = Vec::with_capacity(ct.len());
    for block in ct.chunks_exact(16) {
        let mut buf = [0u8; 16];
        buf.copy_from_slice(block);
        decrypt_ecb_block(key.as_bytes(), &mut buf)?;
        out.extend_from_slice(&buf);
    }

    Ok(String::from_utf8(unpad_pkcs7(&out)?)?)
}

/// 就地解密单个 ECB 块（无链接，故逐块独立）。
fn decrypt_ecb_block(key: &[u8], buf: &mut [u8; 16]) -> Result<()> {
    use aes::cipher::BlockDecrypt;

    match key.len() {
        16 => aes::Aes128::new_from_slice(key)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .decrypt_block(buf.into()),
        24 => aes::Aes192::new_from_slice(key)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .decrypt_block(buf.into()),
        32 => aes::Aes256::new_from_slice(key)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .decrypt_block(buf.into()),
        _ => return Err(CryptoError::InvalidKeyOrIv),
    }
    Ok(())
}

/// 去除 PKCS7 填充，并**校验填充字节本身**。
///
/// 只按最后一个字节截断是不够的：密钥错误时那样会静默切出一个
/// 看似正常的短串，把"解密失败"伪装成"解出了一段乱码"。
fn unpad_pkcs7(data: &[u8]) -> Result<Vec<u8>> {
    let &last = data.last().ok_or(CryptoError::Decrypt)?;
    let pad = last as usize;
    if pad == 0 || pad > 16 || pad > data.len() {
        return Err(CryptoError::Decrypt);
    }
    let (body, tail) = data.split_at(data.len() - pad);
    if !tail.iter().all(|&b| b == last) {
        return Err(CryptoError::Decrypt);
    }
    Ok(body.to_vec())
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

    // ── AES-ECB/PKCS7（登录成功响应的 `data`）────────────────────────────

    /// NIST FIPS-197 附录 C.1 的 AES-128 ECB 单块向量（无填充形态）。
    /// 这里用它锁住"逐块、无链接"这一条语义：CBC 解同一密文必然不同。
    #[test]
    fn ecb_decrypts_nist_single_block_vector() {
        // key = 000102...0f, plaintext = 00112233445566778899aabbccddeeff
        // ciphertext = 69c4e0d86a7b0430d8cdb78070b4c55a
        let ct = hex::decode("69c4e0d86a7b0430d8cdb78070b4c55a").unwrap();
        // 该向量不含 PKCS7 填充，直接验块解密的正确性
        let mut buf = [0u8; 16];
        buf.copy_from_slice(&ct);
        let key = hex::decode("000102030405060708090a0b0c0d0e0f").unwrap();
        decrypt_ecb_block(&key, &mut buf).unwrap();
        assert_eq!(
            hex::encode(buf),
            "00112233445566778899aabbccddeeff",
            "ECB 必须解出 NIST 标准向量"
        );
    }

    #[test]
    fn ecb_roundtrip_with_pkcs7() {
        let key = b"qPqDw263XgFgL3u8"; // 登录响应密钥（16 字节）
        let pt = r#"{"account":"19900000001"}"#;

        // 手工做 PKCS7 填充后逐块 ECB 加密，作为解密的输入。
        let mut buf = pt.as_bytes().to_vec();
        let pad = 16 - (buf.len() % 16);
        buf.resize(buf.len() + pad, pad as u8);

        use aes::cipher::BlockEncrypt;
        let cipher = aes::Aes128::new_from_slice(key).unwrap();
        let mut ct = Vec::with_capacity(buf.len());
        for block in buf.chunks_exact(16) {
            let mut b = [0u8; 16];
            b.copy_from_slice(block);
            cipher.encrypt_block((&mut b).into());
            ct.extend_from_slice(&b);
        }

        let dec = aes_ecb_pkcs7_decrypt_hex(std::str::from_utf8(key).unwrap(), &hex::encode(&ct))
            .unwrap();
        assert_eq!(dec, pt);
    }

    #[test]
    fn ecb_rejects_non_hex_and_ragged_ciphertext() {
        let key = "qPqDw263XgFgL3u8";
        // 非 hex
        assert!(aes_ecb_pkcs7_decrypt_hex(key, "zzzz").is_err());
        // 长度不是 16 的倍数
        assert!(aes_ecb_pkcs7_decrypt_hex(key, "00112233").is_err());
        // 空串
        assert!(aes_ecb_pkcs7_decrypt_hex(key, "").is_err());
    }

    #[test]
    fn unpad_pkcs7_rejects_bad_padding() {
        // 填充字节与内容不符（最后 4 字节声称填 4，里面有 1 个不是 4）
        assert!(unpad_pkcs7(&[1, 2, 3, 4, 4, 4, 3]).is_err());
        // 填充值 0 非法
        assert!(unpad_pkcs7(&[1, 2, 3, 0]).is_err());
        // 填充超过块长非法
        assert!(unpad_pkcs7(&[1, 2, 3, 17]).is_err());
        // 填满一整块是合法的
        assert_eq!(unpad_pkcs7(&[16u8; 16]).unwrap(), Vec::<u8>::new());
        // 正常形态：末尾 3 个 `2` ⇒ 去掉 2 字节填充（最后一个 `2` 是数据本身的一部分）
        assert_eq!(unpad_pkcs7(&[1, 2, 2, 2]).unwrap(), vec![1, 2]);
        // 空输入非法
        assert!(unpad_pkcs7(&[]).is_err());
    }

    #[test]
    fn ecb_differs_from_cbc_for_same_key_and_ciphertext() {
        // 同一密钥下两种模式的解密结果必须不同 —— 混用模式的静默性正来自这里
        let key = b"qPqDw263XgFgL3u8";
        let iv = b"0123456789abcdef";
        let cipher = aes_cbc_encrypt(key, iv, b"hello world!!").unwrap();
        let cbc = aes_cbc_decrypt(key, iv, &cipher).unwrap();
        assert_eq!(cbc, b"hello world!!");
        // 把同一段 CBC 密文塞进 ECB 解密：要么报错、要么解出别的东西，绝不会等于原文
        let ecb =
            aes_ecb_pkcs7_decrypt_hex(std::str::from_utf8(key).unwrap(), &hex::encode(&cipher));
        assert!(
            ecb.as_ref().map(|s| s.as_str()).unwrap_or("") != "hello world!!",
            "ECB 与 CBC 不可互换"
        );
    }
}
