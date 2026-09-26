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
    // `as_chunks::<16>()` 而不是 `chunks_exact(16)`：后者给的是 `&[u8]`，
    // 要再 `copy_from_slice` 进 `[u8; 16]` 才能调 `decrypt_ecb_block`；
    // 前者直接给 `&[[u8; 16]]`，**少一次拷贝**，且"块长必须是 16"
    // 由类型写死而不是由调用点的字面量约定。
    //
    // 余数在上面已由 `ct.len() % 16 != 0` 拦掉，这里必然是空的；
    // 用 `let (blocks, _) =` 而不是断言，是因为前提就在上方 7 行之内，
    // 再加一句 panic 只会让人以为它可能真的非空。
    let (blocks, _) = ct.as_chunks::<16>();
    for block in blocks {
        let mut buf = *block;
        decrypt_ecb_block(key.as_bytes(), &mut buf)?;
        out.extend_from_slice(&buf);
    }

    Ok(String::from_utf8(unpad_pkcs7(&out)?)?)
}

/// AES-ECB/PKCS7 **加密**，输出是 **小写 hex 字符串**。
///
/// 与 [`aes_ecb_pkcs7_decrypt_hex`] 配对，用于 App 端登录体
/// （见 `mcloud_upstream::app_login`）。
///
/// # 为什么 PKCS7 是手写的
///
/// `encrypt_padded_mut::<Pkcs7>` 当然也能用，但手写这几行把
/// **"块间无链接"** 这件事摆在代码里：ECB 的全部语义就是"逐块独立加密"。
/// 用 `_mut` 变体时读者要去看它的文档才知道有没有做链式处理。
///
/// 填充规则与 PKCS7 一致：`pad = 16 - len % 16`，因此
/// **整块时补满一整块**（16 字节），不是补 0。
pub fn aes_ecb_pkcs7_encrypt_hex(key: &str, plaintext: &str) -> Result<String> {
    let data = plaintext.as_bytes();
    let pad = 16 - (data.len() % 16);

    let mut buf = Vec::with_capacity(data.len() + pad);
    buf.extend_from_slice(data);
    buf.extend(std::iter::repeat_n(pad as u8, pad));

    // 同解密路径：`as_chunks_mut` 直接给 `&mut [[u8; 16]]`，
    // 就地加密后写回，不必先复制到临时数组再拷回来。
    // 上面刚补过 PKCS7 填充，块数必然是整数，余数为空。
    let (blocks, _) = buf.as_chunks_mut::<16>();
    for block in blocks {
        encrypt_ecb_block(key.as_bytes(), block)?;
    }

    Ok(hex::encode(&buf))
}

/// 就地加密单个 ECB 块（无链接，故逐块独立）。
fn encrypt_ecb_block(key: &[u8], buf: &mut [u8; 16]) -> Result<()> {
    use aes::cipher::BlockEncrypt;

    match key.len() {
        16 => aes::Aes128::new_from_slice(key)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .encrypt_block(buf.into()),
        24 => aes::Aes192::new_from_slice(key)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .encrypt_block(buf.into()),
        32 => aes::Aes256::new_from_slice(key)
            .map_err(|_| CryptoError::InvalidKeyOrIv)?
            .encrypt_block(buf.into()),
        _ => return Err(CryptoError::InvalidKeyOrIv),
    }
    Ok(())
}

/// 把**末尾 10 个字符**移到最前面（App 端登录体的第三层封装）。
///
/// # 这不是加密，是一次字符串旋转
///
/// 反汇编 `-[MCLoginCryptoUtil privateEncrypt:isCurrentEnvironment:]` 得到的
/// 就是两行 `substringFromIndex:` / `substringToIndex:` 加一次拼接：
///
/// ```objc
/// p    = MAX(len, 10) - 10;
/// tail = [e substringFromIndex:p];   // 末 10 个
/// head = [e substringToIndex:p];     // 其余
/// out  = [NSString stringWithFormat:@"%@%@", tail, head];
/// ```
///
/// 少做这一步，服务端会回 `9103 输入参数不合法` —— 因为它要先反旋转再解密。
///
/// # 按**字符**而不是字节
///
/// ObjC 的 `substringFromIndex:` 按 UTF-16 码元计数，这里按 Unicode 标量
/// 计数；两者只在**辅助平面字符**（emoji 等）上不同，而本函数的输入
/// 永远是 ASCII hex，所以三种口径在实际流量上完全等价。
///
/// 长度 ≤ 10 时**原样返回**（不是报错）：此时客户端算出的 `p` 是 0，
/// 拼接结果就是原串本身。
pub fn shift_tail10_to_front(s: &str) -> String {
    let n = s.chars().count();
    if n <= 10 {
        return s.to_string();
    }
    let cut = n - 10;
    let byte_at = s.char_indices().nth(cut).map_or(s.len(), |(i, _)| i);
    let (head, tail) = s.split_at(byte_at);
    format!("{tail}{head}")
}

/// [`shift_tail10_to_front`] 的逆操作：把**开头 10 个字符**移到末尾。
///
/// 客户端对应的是 `if (len < 11) return @"";`，即长度不足时它返回**空串**；
/// 这里刻意偏离成"原样返回"，理由是：长度 ≤ 10 的输入本来就不是合法密文，
/// 后续 AES 解密一定会因为长度或填充非法而报错 ——
/// 让错误在**解密层**报出来，比在这里安静地变成空串更容易查。
pub fn shift_head10_to_back(s: &str) -> String {
    let n = s.chars().count();
    if n <= 10 {
        return s.to_string();
    }
    let byte_at = s.char_indices().nth(10).map_or(s.len(), |(i, _)| i);
    let (head, tail) = s.split_at(byte_at);
    format!("{tail}{head}")
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
        // 刚补过填充，块数整数（同实现里的写法）
        let (blocks, _) = buf.as_chunks::<16>();
        for block in blocks {
            let mut b = *block;
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

#[cfg(test)]
mod app_login_tests {
    use super::*;

    /// 抓包里那条 `thirdlogin` 明文（186 字节），键序取**抓包原样**。
    ///
    /// 这份字面量在 Go（`internal/core/crypto_test.go`）与 Rust
    /// （`crates/mcloud-core/src/crypto/tests/app.rs`）两个仓库里**逐字节相同**，
    /// 所以它是这份协议目前最强的证据 —— 优于实现侧自家键序的那一份。
    const CAPTURED_PLAINTEXT: &str = r#"{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"429344","pintype":"8","loginMode":"0","verType":"1","version":"12.6.0","msisdn":"19900000000","random":"hOjseB","clienttype":"412"}"#;

    /// 上面那串经 `lYzvvBMUjucw+h^R` 加密 + 末 10 位错位后的线上形态。
    ///
    /// 由抓包侧的**独立** AES-ECB 实现算出（不是本仓自产自销），
    /// 两个参考仓库都钉着同一个值。
    const GOLDEN_WIRE: &str = "2584e02d73b549d837f9fa6e4bdd4968c816497fba1be0770aeba9d7249a928db623ee6b11f26284588aa8dd943fdca3a5e7d21a36fdc347dbf16e1a2d0d55f23c77b1409ad5afff5600844ded9a613635003471e71be8e07b9b7fb8bbd14639e90ffa9716c22fb01670ee5b4b208efc0d59a0b16c087688ad5fd68561af8860b1e1d93ad7d00150cdb6bac04a3cc792a05347c7813afd2e06ca996da52959d7bc92881e4584c088cf7b01dc35341927e92c11d7e66e76c1625228a8d84c0eb0";

    /// 同一逻辑请求的**实现侧键序**（serde_json 字母序）密文。
    ///
    /// 与 [`GOLDEN_WIRE`] **不是一个值** —— 键序进了加密输入，密文必然不同。
    /// 两条都留着是有意为之：它们一起证明"键序不参与服务端校验"
    /// （活体探针两种键序都回 `9441`），同时防止有人把两份黄金值串了线。
    ///
    /// 只比长度是**查不出**这种串线的：两份明文都是 186 字节。
    const SORTED_PLAINTEXT: &str = r#"{"clienttype":"412","cpid":"55","dycpwd":"429344","extInfo":{"ifOpenAccount":"1"},"loginMode":"0","msisdn":"19900000000","pintype":"8","random":"hOjseB","verType":"1","version":"12.6.0"}"#;

    const SORTED_WIRE: &str = "17e30bcd98e6e4bdccc6b6e0c034688a3d2db656484a7de56ee67b2580af3dde4075b72cd78a11415da4b19ce641dbf7fad4a096f842da2556008fbbc3521d6ebf0499470b3dcf08532c25cf5ce6ac9052c20db6035c039af766e0206a772f94c3933802caaa302cc5e6d3c7160711a3d966cfc770568650de3a503048bb660f9e57597dc82cf15674c8f622823bc517ef2f7f51483fdff8b8191c6f1e6335d9c48aaba2d076b067ff091c65f3a107a60bd61352d9718a0466512bce9a4397b6";

    const APP_KEY: &str = "lYzvvBMUjucw+h^R";

    #[test]
    fn both_plaintexts_are_the_measured_186_bytes() {
        // 长度只是**哨兵**，不是判据：两份键序不同的明文长度相同，
        // 所以"长度对"根本无法区分它们 —— 真正的判据是下面那条整串比对。
        assert_eq!(CAPTURED_PLAINTEXT.len(), 186);
        assert_eq!(SORTED_PLAINTEXT.len(), 186);
    }

    #[test]
    fn ecb_encrypt_reproduces_the_captured_golden_wire() {
        // 这条钉的是"上游层有没有多加/少加/反做一次错位"。
        // 一旦有人把旋转方向写反、或漏掉旋转，这里立刻炸，
        // 而不是等一次真实登录收到 9103 才发现。
        let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, CAPTURED_PLAINTEXT).unwrap();
        assert_eq!(ecb.len(), 384, "186 字节 → PKCS7 补 6 → 192 字节 → 384 hex");
        assert_eq!(
            shift_tail10_to_front(&ecb),
            GOLDEN_WIRE,
            "加密 + 末 10 位错位必须逐字节命中抓包侧算出的黄金值"
        );
    }

    #[test]
    fn ecb_encrypt_reproduces_the_sorted_golden_wire() {
        let wire =
            shift_tail10_to_front(&aes_ecb_pkcs7_encrypt_hex(APP_KEY, SORTED_PLAINTEXT).unwrap());
        assert_eq!(wire, SORTED_WIRE);
        // 两份黄金值必须不同 —— 如果哪天相等了，说明键序没进加密输入，
        // 那才是真的出错。
        assert_ne!(GOLDEN_WIRE, SORTED_WIRE);
    }

    #[test]
    fn wire_is_lowercase_hex() {
        let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, CAPTURED_PLAINTEXT).unwrap();
        let wire = shift_tail10_to_front(&ecb);
        assert_eq!(wire, wire.to_ascii_lowercase(), "请求体是小写 hex");
        assert!(wire.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn rotation_moves_exactly_the_tail_ten_to_the_front() {
        // 逐位验证方向：不是"转了一圈"，而是"末 10 个搬到最前"。
        let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, CAPTURED_PLAINTEXT).unwrap();
        let wire = shift_tail10_to_front(&ecb);
        assert_eq!(&wire[..10], &ecb[ecb.len() - 10..], "前 10 = 原串末 10");
        assert_eq!(
            &wire[10..],
            &ecb[..ecb.len() - 10],
            "其余 = 去掉末 10 的前缀"
        );
    }

    #[test]
    fn rotation_round_trips() {
        for s in ["0123456789A", "abcdefghijklmnop", &"x".repeat(384)] {
            let back = shift_head10_to_back(&shift_tail10_to_front(s));
            assert_eq!(back, s, "两次旋转必须回到原串：{s}");
        }
    }

    #[test]
    fn short_inputs_are_identity_not_errors() {
        // 客户端算出的下标是 `MAX(len,10)-10`，长度 ≤ 10 时就是 0，
        // 拼接结果即原串 —— 所以这里**不该**报错，也不该补字符。
        for s in ["", "a", "0123456789"] {
            assert_eq!(shift_tail10_to_front(s), s);
            assert_eq!(shift_head10_to_back(s), s);
        }
    }

    #[test]
    fn rotation_is_char_based_not_byte_based() {
        // 10 个汉字 = 10 字符 / 30 字节。按字节实现会切在字符中间而 panic
        // 或切错位置；按字符实现则"末 10 个字符"正好是全部，结果 = 原串。
        let s = "零一二三四五六七八九";
        assert_eq!(shift_tail10_to_front(s), s, "10 字符 → 原样");
        // 11 个字符时，两者方向相反（这正是"认反"会踩的坑）：
        //   末 10 个搬到最前 → 首字符落到末尾
        //   前 10 个搬到末尾 → 末字符落到最前
        let s11 = "零一二三四五六七八九十";
        assert_eq!(shift_tail10_to_front(s11), "一二三四五六七八九十零");
        assert_eq!(shift_head10_to_back(s11), "十零一二三四五六七八九");
        // 它们互为逆操作
        assert_eq!(shift_head10_to_back(&shift_tail10_to_front(s11)), s11);
        assert_eq!(shift_tail10_to_front(&shift_head10_to_back(s11)), s11);
    }

    #[test]
    fn wrong_rotation_direction_does_not_decrypt() {
        // 这是最容易犯的错：把请求方向用到响应上。
        // 旋转不改变字符集，所以"忘了反旋转"只会解出乱码或填充非法，
        // 而不会在旋转这一步报错 —— 必须由解密层兜住。
        let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, CAPTURED_PLAINTEXT).unwrap();
        let wire = shift_tail10_to_front(&ecb);

        // 正确方向：解得回原文
        let ok = aes_ecb_pkcs7_decrypt_hex(APP_KEY, &shift_head10_to_back(&wire)).unwrap();
        assert_eq!(ok, CAPTURED_PLAINTEXT);

        // 未反旋转（方向写错）：必须失败，而不是安静地给出垃圾
        assert!(
            aes_ecb_pkcs7_decrypt_hex(APP_KEY, &wire).is_err(),
            "漏掉反旋转必须报错"
        );
    }

    #[test]
    fn alt_key_does_not_decrypt_production_ciphertext() {
        // 用错密钥（备用分支）会得到 9103，这里离线复现那条判据：
        // 备用密钥必须解不开生产密钥的密文。
        let ecb = aes_ecb_pkcs7_encrypt_hex(APP_KEY, CAPTURED_PLAINTEXT).unwrap();
        assert!(
            aes_ecb_pkcs7_decrypt_hex("+09T5lHhO+smS7x@", &ecb).is_err(),
            "备用密钥不该解开生产密文"
        );
    }

    #[test]
    fn pkcs7_appends_a_full_block_when_already_aligned() {
        // 16 字节整块时要补满一整块（16），不是补 0 —— 补 0 会让
        // 服务端解出的明文尾部缺字节，表现为 JSON 解析失败。
        let aligned = "0123456789abcdef";
        assert_eq!(aligned.len(), 16);
        let hexed = aes_ecb_pkcs7_encrypt_hex(APP_KEY, aligned).unwrap();
        assert_eq!(hexed.len(), 64, "16 字节明文 → 32 字节密文");
        assert_eq!(aes_ecb_pkcs7_decrypt_hex(APP_KEY, &hexed).unwrap(), aligned);
    }

    #[test]
    fn ecb_round_trip_for_unspecified_lengths() {
        for n in [0usize, 1, 15, 16, 17, 31, 32, 33, 186] {
            let plain: String = (0..n).map(|i| char::from(b'a' + (i as u8 % 26))).collect();
            let hexed = aes_ecb_pkcs7_encrypt_hex(APP_KEY, &plain).unwrap();
            assert_eq!(
                aes_ecb_pkcs7_decrypt_hex(APP_KEY, &hexed).unwrap(),
                plain,
                "长度 {n} 往返失败"
            );
        }
    }

    #[test]
    fn encrypt_rejects_non_16_byte_keys() {
        // 密钥按字节长度分派；非法长度必须报错而不是静默截断。
        // App 登录密钥恰好 16 字节 → AES-128，这一点也要钉住。
        assert_eq!(APP_KEY.len(), 16, "App 登录密钥是 16 字节 → AES-128");
        assert!(aes_ecb_pkcs7_encrypt_hex("short", "x").is_err());
        assert!(aes_ecb_pkcs7_encrypt_hex(&"k".repeat(20), "x").is_err());
    }
}
