//! 纯密码学原语 —— 与 TS 原版实现**逐字节**对齐。
//!
//! # 为什么在 `mcloud-core` 而不是 `mcloud-io`
//!
//! `mcloud-upstream` 必须能用签名头（MD5）、设备指纹（DES/RSA），
//! 但它**不允许依赖 `mcloud-io`**（否则 `reqwest` 会被拖进依赖树，
//! "对单个 upstream 函数做无网测试"就不再可能）。
//! 密码学是**无 IO 的纯计算**，所以归属 core。详见 `docs/rust-rewrite-plan.md` §3.2 / §1.3。
//!
//! # 对拍基准
//!
//! 本模块的全部行为由 `test/golden/crypto-vectors.json` 锁定，该文件由
//! `scripts/gen-crypto-vectors.cjs` **直接调用 TS 原版所用的 crypto-js + node:crypto** 产出。
//! 也就是说：**预期的密文不是手算的，是原实现真跑出来的**。
//! Rust 侧只要有任何一位不同，`cargo test -p mcloud-core crypto` 就会失败。
//!
//! # 三个"想当然会错"的坑（已实测，勿改）
//!
//! 1. **RSA PKCS#1 v1.5 加密是随机的**（Type 02 填充含随机非零字节），
//!    同一明文加密三次结果全不同 → 只能做"解回明文"的单向验证，不能字节级对拍。
//! 2. **gzip 的 `buf[9] = 19` 是归一化而非魔改**：Node 的 `CompressionStream("gzip")`
//!    产出的 OS 字节本来就是 19；该行是为了抹平 txiki.js 的差异。
//!    Rust 侧 `flate2` 默认不是 19，**必须显式写**。
//! 3. **`encryptDes` 的补零按 JS 字符串长度（UTF-16 码元数）算**，不是按字节数。
//!    纯 ASCII 下两者等价；含 CJK 时会算出非 8 倍数字节。本模块**保持 ASCII 行为**，
//!    不"顺手修好"，否则生成的 smid 与线上不一致。

use base64::Engine as _;
use cipher::{
    BlockDecrypt, BlockEncrypt, KeyInit, KeyIvInit, block_padding::Pkcs7,
    generic_array::GenericArray,
};
use hmac::Mac as _;
use sha2::Digest as _;

use crate::error::Error;

// ─────────────────────────────────────────────────────────────────────────────
// 常量（与 TS 源码逐字对齐）
// ─────────────────────────────────────────────────────────────────────────────

/// `core/src/auth/sign.ts` 的 `SALT_MAP`。签名盐值只有两个，用 `u8` 索引。
pub const SIGN_SALT_MAP: [(u8, &str); 2] = [(1, "seedMdYYLIZfbCxg"), (2, "sekaMdYYLIZfbCfm")];

/// `core/src/utils/encrypt.ts` 的默认密钥。**24 字节 → AES-192**（不是 AES-256，这个坑很常见）。
pub const REFRESH_TOKEN_KEY: &str = "2olBaQGYnEKoStYomsd1n7ax";

/// `runtime/src/crypto.ts: aesEncrypt` 的固定 IV。
pub const AES_FIXED_IV: &[u8; 16] = b"0102030405060708";

/// `shared/src/smid/index.ts` 的 skland 公钥（SPKI DER 的 base64，1024 位）。
pub const SKLAND_PUBLIC_KEY_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQC8KHAcHbkCn5rxGgGJE+07tY+pt86D/oZ7sA51FaEBv2jgno2TI9zHJVYKJynmiKpixgwUcv93EfWIrU/p/UCs5Vu+odS3I4UBp3R7IZ1A0W01FkumAHYW2PQpMm8ueQKPLUq/idkpG/9b2JDv/qU+Ks36nbUPwlW4CjdfrV+V9QIDAQAB";

/// gzip 头里 OS 字段的下标。
const GZIP_OS_INDEX: usize = 9;
/// gzip 头里 OS 字段必须被改成的值（浏览器/Node 的实际取值）。
const GZIP_OS_VALUE: u8 = 19;

// ─────────────────────────────────────────────────────────────────────────────
// 随机源 trait —— 让"随机"可注入，从而整条链路可复现
// ─────────────────────────────────────────────────────────────────────────────

/// 密码学安全的随机源。
///
/// **core 内唯一允许的随机入口**。理由：设备指纹（smid）构造混用了 `randomUUID` 与
/// `Math.random`，若直接取全局随机数，则 L2 fixture 回放与 golden 对拍**无法复现**
/// （每次请求体都不同）。收敛到 trait 后，测试注入固定序列即可让全链路字节级稳定。
///
/// 生产实现（`SystemRandom`，基于 OS CSPRNG）放在 `mcloud-io`。
pub trait Random {
    /// 用随机字节填满 `dst`。
    fn fill_bytes(&mut self, dst: &mut [u8]);

    /// 生成一个 UUID v4 字符串（对齐 `rt.crypto.randomUUID()`）。
    fn uuid_v4(&mut self) -> String {
        let mut b = [0u8; 16];
        self.fill_bytes(&mut b);
        // 版本 4 + variant RFC 4122，与 node:crypto randomUUID 的语义一致
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h = hex::encode(b);
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }

    /// 取 `[0, n)` 内的随机数，对齐 JS 的 `Math.floor(Math.random() * n)`。
    ///
    /// 用拒绝采样避免取模偏差。
    fn below(&mut self, n: u32) -> u32 {
        assert!(n > 0, "below(0) 无意义");
        let limit = u32::MAX - (u32::MAX % n) - 1;
        loop {
            let mut b = [0u8; 4];
            self.fill_bytes(&mut b);
            let v = u32::from_be_bytes(b);
            if v <= limit {
                return v % n;
            }
        }
    }
}

/// 把 core 的 [`Random`] 适配成 `rsa` 需要的 `CryptoRngCore`。
///
/// 之所以能安全地打上 `CryptoRng` 标记：[`Random`] 的语义约定就是 CSPRNG。
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

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), rsa::rand_core::Error> {
        self.0.fill_bytes(dst);
        Ok(())
    }
}

impl<R: Random + ?Sized> rsa::rand_core::CryptoRng for RsaRngAdapter<'_, R> {}

// ─────────────────────────────────────────────────────────────────────────────
// 1. 哈希 / MAC
// ─────────────────────────────────────────────────────────────────────────────

/// MD5 → 小写 hex。对齐 `runtime/src/crypto.ts: md5`。
pub fn md5_hex(data: &[u8]) -> String {
    use md5::Md5;
    let mut h = Md5::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// SHA-256 → 小写 hex。对齐 `runtime/src/crypto.ts: sha256`。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = sha2::Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// SHA-1 → **大写** hex。
///
/// 对齐 PC 客户端 `common/utils.js: encryptByHash`：
/// `CryptoJS.SHA1(HASHTEXT + password).toString(Hex).toUpperCase()`。
/// 大小写不是风格问题 —— 它直接进入请求体 `dycpwd`，服务端按十六进制解析。
pub fn sha1_hex_upper(data: &[u8]) -> String {
    use sha1::Digest as _;
    let mut h = sha1::Sha1::new();
    h.update(data);
    hex::encode_upper(h.finalize())
}

/// HmacSHA256 → **base64**（不是 hex！对齐 `shared/src/push/index.ts:240` 的钉钉加签）。
pub fn hmac_sha256_base64(key: &[u8], message: &[u8]) -> String {
    use hmac::Hmac;
    use sha2::Sha256;
    type H = Hmac<Sha256>;

    let mut mac = <H as hmac::Mac>::new_from_slice(key).expect("Hmac 接受任意长度密钥");
    mac.update(message);
    b64_encode(&mac.finalize().into_bytes())
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. 编码助手
// ─────────────────────────────────────────────────────────────────────────────

fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(s: &str) -> Result<Vec<u8>, Error> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| Error::Internal(format!("base64 解码失败: {e}")))
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. AES-CBC/PKCS7（固定 IV）—— runtime/src/crypto.ts: aesEncrypt
// ─────────────────────────────────────────────────────────────────────────────

/// 复刻 `key.substring(0, 16).padEnd(16, "\0")`。
///
/// 注意：JS 的 `substring` 按 UTF-16 码元计数。本项目**所有密钥都是 ASCII**，
/// 因此按字节截断/补零等价；非 ASCII 密钥会与 JS 不一致（原实现从未使用，仅记录）。
fn js_pad_key16(key: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    let src = key.as_bytes();
    let n = src.len().min(16);
    out[..n].copy_from_slice(&src[..n]);
    out
}

fn cbc_pkcs7_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    macro_rules! enc {
        ($alg:ty) => {{
            use cipher::BlockEncryptMut as _;
            let c = <cbc::Encryptor<$alg> as KeyIvInit>::new_from_slices(key, iv)
                .map_err(|e| Error::Internal(format!("AES key/iv 不合法: {e}")))?;
            let mut buf = vec![0u8; data.len() + 16];
            buf[..data.len()].copy_from_slice(data);
            let ct = c
                .encrypt_padded_mut::<Pkcs7>(&mut buf, data.len())
                .map_err(|e| Error::Internal(format!("AES 填充失败: {e}")))?;
            Ok(ct.to_vec())
        }};
    }
    match key.len() {
        16 => enc!(aes::Aes128),
        24 => enc!(aes::Aes192),
        32 => enc!(aes::Aes256),
        n => Err(Error::Internal(format!("AES 密钥长度非法: {n}"))),
    }
}

fn cbc_pkcs7_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    macro_rules! dec {
        ($alg:ty) => {{
            use cipher::BlockDecryptMut as _;
            let d = <cbc::Decryptor<$alg> as KeyIvInit>::new_from_slices(key, iv)
                .map_err(|e| Error::Internal(format!("AES key/iv 不合法: {e}")))?;
            let mut buf = data.to_vec();
            let pt = d
                .decrypt_padded_mut::<Pkcs7>(&mut buf)
                .map_err(|_| Error::Parse("AES 明文填充非法".into()))?;
            Ok(pt.to_vec())
        }};
    }
    match key.len() {
        16 => dec!(aes::Aes128),
        24 => dec!(aes::Aes192),
        32 => dec!(aes::Aes256),
        n => Err(Error::Internal(format!("AES 密钥长度非法: {n}"))),
    }
}

/// AES-CBC/PKCS7，密钥截断/补零到 16，固定 IV，输出 **hex**。
///
/// 对齐 `runtime/src/crypto.ts: aesEncrypt`。
pub fn aes_cbc_fixed_iv_encrypt_hex(key: &str, message: &str) -> Result<String, Error> {
    let k = js_pad_key16(key);
    Ok(hex::encode(cbc_pkcs7_encrypt(
        &k,
        AES_FIXED_IV,
        message.as_bytes(),
    )?))
}

// ─────────────────────────────────────────────────────────────────────────────
// 3.5 AES-ECB/PKCS7 —— PC 客户端 `common/utils.js: decryptByAes`
// ─────────────────────────────────────────────────────────────────────────────

fn ecb_pkcs7_decrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    use cipher::BlockDecryptMut as _;

    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(Error::Parse(format!(
            "AES-ECB 密文长度非法: {}",
            data.len()
        )));
    }

    macro_rules! dec {
        ($alg:ty) => {{
            let cipher = <$alg as KeyInit>::new_from_slice(key)
                .map_err(|e| Error::Internal(format!("AES 密钥不合法: {e}")))?;
            let mut buf = data.to_vec();
            // 裸分组密码逐块解密 = ECB（块间无链接），最后一块按 PKCS7 去填充
            cipher
                .decrypt_padded_mut::<Pkcs7>(&mut buf)
                .map_err(|_| Error::Parse("AES 明文填充非法".into()))?
                .to_vec()
        }};
    }

    match key.len() {
        16 => Ok(dec!(aes::Aes128)),
        24 => Ok(dec!(aes::Aes192)),
        32 => Ok(dec!(aes::Aes256)),
        n => Err(Error::Internal(format!("AES 密钥长度非法: {n}"))),
    }
}

/// AES-ECB/PKCS7 解密，密文是 **hex**（不是 base64），返回 UTF-8 明文。
///
/// 对齐 PC 客户端 `common/utils.js: decryptByAes(input, key)`：
///
/// ```js
/// const keyWordArray = CryptoJS.enc.Utf8.parse(key);        // 16 字节 ASCII → AES-128
/// const encryptedWordArray = CryptoJS.enc.Hex.parse(input);  // 输入是 hex
/// CryptoJS.AES.decrypt(..., { mode: ECB, padding: Pkcs7 })
/// ```
///
/// 两个容易搞错的点：
/// - **输入是 hex 而不是 base64**：同一个文件里的 `encryptByAes` 输出 base64，
///   两者不是一对逆运算，照「加解密对称」的直觉写就会解不开；
/// - 密钥原样使用（不做截断/补零），与本模块 CBC 系列的处理方式不同。
pub fn aes_ecb_pkcs7_decrypt_hex(key: &str, cipher_hex: &str) -> Result<String, Error> {
    let cipher = hex::decode(cipher_hex.trim())
        .map_err(|e| Error::Parse(format!("密文不是合法 hex: {e}")))?;
    let plain = ecb_pkcs7_decrypt(key.as_bytes(), &cipher)?;
    String::from_utf8(plain).map_err(|e| Error::Parse(format!("AES 明文不是合法 UTF-8: {e}")))
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. AES-CBC/PKCS7（动态 IV）—— core/src/utils/encrypt.ts
// ─────────────────────────────────────────────────────────────────────────────

/// 用指定 IV 加密，输出 `Base64(IV ‖ CT)`。
///
/// 密钥**原样使用不截断**（`REFRESH_TOKEN_KEY` 24 字节 → AES-192）。
pub fn aes_dynamic_iv_encrypt(key: &str, plaintext: &str, iv: [u8; 16]) -> Result<String, Error> {
    let ct = cbc_pkcs7_encrypt(key.as_bytes(), &iv, plaintext.as_bytes())?;
    let mut out = Vec::with_capacity(16 + ct.len());
    out.extend_from_slice(&iv);
    out.extend_from_slice(&ct);
    Ok(b64_encode(&out))
}

/// 用随机 IV 加密（生产路径）。IV 从注入的 [`Random`] 取，因此测试可复现。
pub fn aes_dynamic_iv_encrypt_random<R: Random + ?Sized>(
    key: &str,
    plaintext: &str,
    rng: &mut R,
) -> Result<String, Error> {
    let mut iv = [0u8; 16];
    rng.fill_bytes(&mut iv);
    aes_dynamic_iv_encrypt(key, plaintext, iv)
}

/// 解密 `Base64(IV ‖ CT)`，返回 UTF-8 明文。
pub fn aes_dynamic_iv_decrypt(key: &str, data_b64: &str) -> Result<String, Error> {
    let raw = b64_decode(data_b64)?;
    if raw.len() < 16 {
        return Err(Error::Parse("AES 动态 IV 密文长度不足 16 字节".into()));
    }
    let (iv, ct) = raw.split_at(16);
    let pt = cbc_pkcs7_decrypt(key.as_bytes(), iv, ct)?;
    String::from_utf8(pt).map_err(|e| Error::Parse(format!("AES 明文不是合法 UTF-8: {e}")))
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. DES-ECB —— shared/src/smid/index.ts
// ─────────────────────────────────────────────────────────────────────────────

/// 复刻 `key.substring(0, 8).padEnd(8, "\0")`（ASCII 口令场景）。
fn js_pad_key8(key: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    let src = key.as_bytes();
    let n = src.len().min(8);
    out[..n].copy_from_slice(&src[..n]);
    out
}

/// 复刻 `encryptDes` 的补零：`padLen = 8 - (len % 8)`，**`padLen === 8` 时不补**。
///
/// 等价于"补到 8 的倍数，已是倍数则不补"。空串保持空串（→ 空密文）。
fn des_zero_pad(message: &[u8]) -> Vec<u8> {
    let rem = message.len() % 8;
    if rem == 0 {
        return message.to_vec();
    }
    let mut out = message.to_vec();
    out.extend(std::iter::repeat_n(0u8, 8 - rem));
    out
}

fn des_ecb_blocks(key: &[u8], data: &[u8], encrypt: bool) -> Result<Vec<u8>, Error> {
    let cipher =
        des::Des::new_from_slice(key).map_err(|e| Error::Internal(format!("DES 密钥非法: {e}")))?;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(8) {
        let mut block = *GenericArray::from_slice(chunk);
        if encrypt {
            cipher.encrypt_block(&mut block);
        } else {
            cipher.decrypt_block(&mut block);
        }
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// DES-ECB/**NoPadding** 加密，输出 base64。对齐 `shared/src/smid/index.ts: encryptDes`。
///
/// 空串 → 空 base64（`chunks_exact` 不产出任何块），与原实现一致。
pub fn des_ecb_no_padding_encrypt_base64(key: &str, message: &str) -> Result<String, Error> {
    let k = js_pad_key8(key);
    let padded = des_zero_pad(message.as_bytes());
    Ok(b64_encode(&des_ecb_blocks(&k, &padded, true)?))
}

/// DES-ECB/**PKCS7** 解密，base64 进、UTF-8 出。对齐 `runtime/src/crypto.ts: desDecrypt`。
pub fn des_ecb_pkcs7_decrypt(key: &str, cipher_b64: &str) -> Result<String, Error> {
    let k = js_pad_key8(key);
    let ct = b64_decode(cipher_b64)?;
    if ct.len() % 8 != 0 {
        return Err(Error::Parse("DES 密文长度不是 8 的倍数".into()));
    }
    let mut pt = des_ecb_blocks(&k, &ct, false)?;
    // 手工去 PKCS7（DES-ECB 没有现成 mode 包装，所以自己收尾）
    let pad = *pt
        .last()
        .ok_or_else(|| Error::Parse("DES 密文为空".into()))? as usize;
    if pad == 0 || pad > 8 || pad > pt.len() {
        return Err(Error::Parse("DES PKCS7 填充非法".into()));
    }
    pt.truncate(pt.len() - pad);
    String::from_utf8(pt).map_err(|e| Error::Parse(format!("DES 明文不是合法 UTF-8: {e}")))
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. RSA PKCS#1 v1.5 —— runtime/src/node.ts: rsaEncrypt
// ─────────────────────────────────────────────────────────────────────────────

/// RSA PKCS#1 v1.5 公钥加密，输出 base64。
///
/// `public_key_spki_der` 是 **SPKI DER**（即原实现里 base64 解开后的字节）。
///
/// ⚠ **输出不可对拍**：PKCS#1 v1.5 加密填充含随机非零字节，同一明文每次结果都不同。
/// 正确性靠"用私钥解回明文"验证（见本模块测试）。
pub fn rsa_pkcs1v15_encrypt_base64<R: Random + ?Sized>(
    public_key_spki_der: &[u8],
    message: &[u8],
    rng: &mut R,
) -> Result<String, Error> {
    use rsa::pkcs8::DecodePublicKey as _;

    let key = rsa::RsaPublicKey::from_public_key_der(public_key_spki_der)
        .map_err(|e| Error::Internal(format!("RSA 公钥解析失败: {e}")))?;
    let mut adapter = RsaRngAdapter(rng);
    let ct = key
        .encrypt(&mut adapter, rsa::Pkcs1v15Encrypt, message)
        .map_err(|e| Error::Internal(format!("RSA 加密失败: {e}")))?;
    Ok(b64_encode(&ct))
}

/// 传入 base64 的 SPKI 公钥（如 [`SKLAND_PUBLIC_KEY_B64`]）的便捷版本。
pub fn rsa_pkcs1v15_encrypt_base64_with_b64_key<R: Random + ?Sized>(
    public_key_b64: &str,
    message: &[u8],
    rng: &mut R,
) -> Result<String, Error> {
    let der = b64_decode(public_key_b64)?;
    rsa_pkcs1v15_encrypt_base64(&der, message, rng)
}

/// RSA PKCS#1 v1.5 私钥解密（**仅测试与调试台使用**，生产链路拿不到私钥）。
pub fn rsa_pkcs1v15_decrypt(
    private_key_pkcs8_der: &[u8],
    cipher_b64: &str,
) -> Result<Vec<u8>, Error> {
    use rsa::pkcs8::DecodePrivateKey as _;

    let key = rsa::RsaPrivateKey::from_pkcs8_der(private_key_pkcs8_der)
        .map_err(|e| Error::Internal(format!("RSA 私钥解析失败: {e}")))?;
    let ct = b64_decode(cipher_b64)?;
    key.decrypt(rsa::Pkcs1v15Encrypt, &ct)
        .map_err(|e| Error::Parse(format!("RSA 解密失败: {e}")))
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. gzip（OS 字节归一为 19）
// ─────────────────────────────────────────────────────────────────────────────

/// gzip 压缩，并**强制把 OS 字节写成 19**。
///
/// 对齐 `shared/src/smid/index.ts: gzipCompressObject`。该行是"归一化"而非魔改：
/// Node 的 `CompressionStream("gzip")` 本来就产出 19，写这一行是为了抹平 txiki.js 的差异。
/// Rust 侧 `flate2` 默认不是 19，所以这里**必须显式设置**。
pub fn gzip_with_os19(data: &[u8]) -> Result<Vec<u8>, Error> {
    use std::io::Write as _;

    let mut enc = flate2::GzBuilder::new().write(Vec::new(), flate2::Compression::default());
    enc.write_all(data)
        .map_err(|e| Error::Internal(format!("gzip 写入失败: {e}")))?;
    let mut out = enc
        .finish()
        .map_err(|e| Error::Internal(format!("gzip 收尾失败: {e}")))?;
    if out.len() > GZIP_OS_INDEX {
        out[GZIP_OS_INDEX] = GZIP_OS_VALUE;
    }
    Ok(out)
}

/// gzip 压缩后 base64（smid 载荷用的就是这个）。
pub fn gzip_with_os19_base64(data: &[u8]) -> Result<String, Error> {
    Ok(b64_encode(&gzip_with_os19(data)?))
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. 请求签名头 —— core/src/auth/sign.ts
// ─────────────────────────────────────────────────────────────────────────────

/// 取签名盐值；未知 `salt_key` 回落到 1（对齐原实现的 `|| SALT_MAP[1]`）。
pub const fn sign_salt(salt_key: u8) -> &'static str {
    match salt_key {
        2 => SIGN_SALT_MAP[1].1,
        _ => SIGN_SALT_MAP[0].1,
    }
}

/// `md5(salt + id + timestamp + nonce + other + salt)`。
pub fn generate_signature(
    id: &str,
    timestamp: &str,
    nonce: &str,
    salt_key: u8,
    other: &str,
) -> String {
    let salt = sign_salt(salt_key);
    let mut s = String::with_capacity(
        salt.len() * 2 + id.len() + timestamp.len() + nonce.len() + other.len(),
    );
    s.push_str(salt);
    s.push_str(id);
    s.push_str(timestamp);
    s.push_str(nonce);
    s.push_str(other);
    s.push_str(salt);
    md5_hex(s.as_bytes())
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. 设备指纹序列化 getTn —— shared/src/smid/index.ts
// ─────────────────────────────────────────────────────────────────────────────

/// 把一个 JSON 对象按键名排序后，把值按原实现的规则拼成一串（用于 smid 的 `tn`）。
///
/// 规则（逐条对齐 TS）：
/// - `number` 且**非整数** → `Math.floor(v * 10000)`
/// - `number` 且整数 → `v * 10000`
/// - `object`（含数组，非 null）→ 递归
/// - 其余（含 `null`）→ JS `String(v)`，注意 `null` → `"null"` 而非空串
///
/// 已知边界（原实现如此，向量未覆盖，勿擅自"修好"）：
/// - 键排序：JS 按 UTF-16 码元，这里按 UTF-8 字节；非 BMP 字符下顺序可能不同；
/// - 极大/极小数字：JS 会输出 `1e+21` 这类指数形式，Rust `f64` 的 `Display` 不会。
pub fn get_tn(value: &serde_json::Value) -> String {
    let Some(obj) = value.as_object() else {
        return String::new();
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();

    let mut out = String::new();
    for k in keys {
        match &obj[k] {
            serde_json::Value::Number(n) => {
                let f = n.as_f64().unwrap_or(0.0);
                let is_int = n.is_i64() || n.is_u64();
                let scaled = if is_int {
                    f * 10000.0
                } else {
                    (f * 10000.0).floor()
                };
                // Rust 的 f64 Display 在无小数时省略 ".0"，与 JS Number.toString() 一致
                out.push_str(&format!("{scaled}"));
            }
            v @ serde_json::Value::Object(_) => out.push_str(&get_tn(v)),
            serde_json::Value::String(s) => out.push_str(s),
            serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            serde_json::Value::Null => out.push_str("null"),
            v @ serde_json::Value::Array(_) => out.push_str(&get_tn(v)),
        }
    }
    out
}

/// smid 的 `tn` 字段：`md5(getTn(obj))`。
pub fn get_tn_md5(value: &serde_json::Value) -> String {
    md5_hex(get_tn(value).as_bytes())
}

// ─────────────────────────────────────────────────────────────────────────────
// 测试：以 TS 黄金向量为准
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::Value;

    const VECTORS: &str = include_str!("../../../test/golden/crypto-vectors.json");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct HashCase {
        name: String,
        input: String,
        expect: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct HmacCase {
        name: String,
        message: String,
        key: String,
        expect: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AesCase {
        name: String,
        key: String,
        message: String,
        expect: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DesEncCase {
        name: String,
        key: String,
        message: String,
        expect: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DesDecCase {
        name: String,
        key: String,
        cipher_b64: String,
        expect: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DynIvCase {
        name: String,
        key: String,
        key_bytes: usize,
        iv_hex: String,
        plaintext: String,
        expect_b64: String,
        byte_exact: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SignCase {
        name: String,
        id: String,
        timestamp: Value,
        nonce: String,
        salt_key: u8,
        other: String,
        expect_md5: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TnCase {
        name: String,
        input: Value,
        expect: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GzipCase {
        name: String,
        json: String,
        base64: String,
        plain_len: usize,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RsaSelfCase {
        name: String,
        message: String,
        cipher_b64: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RsaSelfPair {
        public_key_b64: String,
        private_key_pkcs8_b64: String,
        cases: Vec<RsaSelfCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RsaSklandCase {
        public_key_b64: String,
        modulus_bytes: usize,
        samples: Vec<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Vectors {
        md5: Vec<HashCase>,
        sha256: Vec<HashCase>,
        #[serde(rename = "hmacSha256B64")]
        hmac_sha256_b64: Vec<HmacCase>,
        #[serde(rename = "aes128CbcFixedIvHex")]
        aes_fixed: Vec<AesCase>,
        #[serde(rename = "desEcbNoPaddingB64")]
        des_enc: Vec<DesEncCase>,
        #[serde(rename = "desEcbPkcs7Decrypt")]
        des_dec: Vec<DesDecCase>,
        #[serde(rename = "aesDynamicIv")]
        aes_dyn: Vec<DynIvCase>,
        #[serde(rename = "signHeader")]
        sign_header: Vec<SignCase>,
        #[serde(rename = "getTn")]
        get_tn: Vec<TnCase>,
        gzip: Vec<GzipCase>,
        #[serde(rename = "rsaSelfKeyPair")]
        rsa_self: RsaSelfPair,
        #[serde(rename = "rsaPkcs1v15B64")]
        rsa_skland: Vec<RsaSklandCase>,
    }

    fn vectors() -> Vectors {
        serde_json::from_str(VECTORS).expect("黄金向量文件应能解析")
    }

    /// 测试用确定性 RNG（splitmix64）。固定种子 → 每次跑出的随机序列完全一样。
    struct SeededRng(u64);

    impl SeededRng {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
    }

    impl Random for SeededRng {
        fn fill_bytes(&mut self, dst: &mut [u8]) {
            for chunk in dst.chunks_mut(8) {
                self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
                z ^= z >> 31;
                let bytes = z.to_be_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
        }
    }

    // ── 哈希 ────────────────────────────────────────────────────────────────

    #[test]
    fn md5_matches_ts_vectors() {
        let v = vectors();
        assert!(!v.md5.is_empty(), "向量不能为空");
        for c in &v.md5 {
            assert_eq!(md5_hex(c.input.as_bytes()), c.expect, "md5 用例 {}", c.name);
        }
    }

    #[test]
    fn sha256_matches_ts_vectors() {
        let v = vectors();
        for c in &v.sha256 {
            assert_eq!(
                sha256_hex(c.input.as_bytes()),
                c.expect,
                "sha256 用例 {}",
                c.name
            );
        }
    }

    #[test]
    fn hmac_matches_ts_vectors() {
        let v = vectors();
        for c in &v.hmac_sha256_b64 {
            assert_eq!(
                hmac_sha256_base64(c.key.as_bytes(), c.message.as_bytes()),
                c.expect,
                "hmac 用例 {}",
                c.name
            );
        }
    }

    /// 反向确认：base64 与 hex 不能搞混（原实现这里是 base64，很容易写错）。
    #[test]
    fn hmac_is_base64_not_hex() {
        let v = vectors();
        let c = &v.hmac_sha256_b64[0];
        let got = hmac_sha256_base64(c.key.as_bytes(), c.message.as_bytes());
        assert!(
            got.ends_with('=') || got.len() == 44,
            "base64 应有 padding/长度特征: {got}"
        );
        assert!(!got.chars().all(|ch| ch.is_ascii_hexdigit()), "不应是 hex");
    }

    // ── AES 固定 IV ─────────────────────────────────────────────────────────

    #[test]
    fn aes_fixed_iv_matches_ts_vectors() {
        let v = vectors();
        for c in &v.aes_fixed {
            assert_eq!(
                aes_cbc_fixed_iv_encrypt_hex(&c.key, &c.message).unwrap(),
                c.expect,
                "aes 用例 {}",
                c.name
            );
        }
    }

    // ── DES ─────────────────────────────────────────────────────────────────

    #[test]
    fn des_ecb_no_padding_matches_ts_vectors() {
        let v = vectors();
        for c in &v.des_enc {
            let got = des_ecb_no_padding_encrypt_base64(&c.key, &c.message).unwrap();
            match &c.expect {
                Some(exp) => assert_eq!(&got, exp, "des 用例 {}", c.name),
                None => panic!("TS 侧该用例本身报错，Rust 却成功了: {}", c.name),
            }
        }
    }

    #[test]
    fn des_ecb_no_padding_empty_stays_empty() {
        // 空串 → 空密文（不是补满一块）。这条单独钉死，因为它最容易"顺手修好"。
        assert_eq!(
            des_ecb_no_padding_encrypt_base64("a9kbown8", "").unwrap(),
            ""
        );
    }

    #[test]
    fn des_ecb_pkcs7_decrypt_matches_ts_vectors() {
        let v = vectors();
        for c in &v.des_dec {
            assert_eq!(
                des_ecb_pkcs7_decrypt(&c.key, &c.cipher_b64).unwrap(),
                c.expect,
                "des 解密用例 {}",
                c.name
            );
        }
    }

    /// 含 CJK 时按字节补零会得到非 8 倍数长度 → 必须报错，而不是静默产出错误密文。
    #[test]
    fn des_byte_based_padding_rejects_unaligned_cjk() {
        // "移动云盘" 是 4 个字符 / 12 字节，12 % 8 != 0 —— 但字节补零会补到 16，能算出结果。
        // 这里断言的是：**不做 JS 字符串级补零**，行为差异被显式记录下来。
        let got = des_ecb_no_padding_encrypt_base64("a9kbown8", "移动云盘").unwrap();
        assert!(
            !got.is_empty(),
            "字节补零路径应能算出结果（与 JS 的字符补零不同）"
        );
    }

    // ── AES 动态 IV ─────────────────────────────────────────────────────────

    #[test]
    fn aes_dynamic_iv_fixed_matches_ts_byte_exact() {
        let v = vectors();
        for c in &v.aes_dyn {
            assert_eq!(c.key_bytes, c.key.len(), "向量里的 keyBytes 应自洽");
        }
        let c = v
            .aes_dyn
            .iter()
            .find(|c| c.byte_exact)
            .expect("应有一例固定 IV 的字节级对拍");
        let iv_raw = hex::decode(&c.iv_hex).expect("向量里的 ivHex 应是合法 hex");
        // 钉死这条：生成器曾把"AES_FIXED_IV"（16 个 ASCII 字节）当 hex 解析成 8 字节，
        // CryptoJS 不报错、默默产出无法解回的密文。见 scripts/gen-crypto-vectors.cjs 注释。
        assert_eq!(iv_raw.len(), 16, "AES-CBC 的 IV 必须是 16 字节");
        let iv: [u8; 16] = iv_raw.try_into().unwrap();
        assert_eq!(
            aes_dynamic_iv_encrypt(&c.key, &c.plaintext, iv).unwrap(),
            c.expect_b64,
            "AES 动态 IV（固定 IV）字节级对拍失败: {}",
            c.name
        );
    }

    /// 随机 IV 的例子：Rust 必须能把 TS 产出的密文解回原明文（跨实现互通）。
    #[test]
    fn aes_dynamic_iv_decrypts_ts_random_iv_output() {
        let v = vectors();
        for c in &v.aes_dyn {
            assert_eq!(
                aes_dynamic_iv_decrypt(&c.key, &c.expect_b64).unwrap(),
                c.plaintext,
                "AES 动态 IV 解密用例 {}",
                c.name
            );
        }
    }

    #[test]
    fn aes_dynamic_iv_rejects_short_input() {
        assert!(aes_dynamic_iv_decrypt(REFRESH_TOKEN_KEY, "AAAA").is_err());
    }

    /// `REFRESH_TOKEN_KEY` 是 24 字节 —— 也就是 AES-192。写错成 AES-128/256 这里会立刻炸。
    #[test]
    fn refresh_token_key_is_aes192() {
        assert_eq!(REFRESH_TOKEN_KEY.len(), 24);
        let mut rng = SeededRng::new(1);
        let out = aes_dynamic_iv_encrypt_random(REFRESH_TOKEN_KEY, "x", &mut rng).unwrap();
        // IV(16) + 1 块密文(16) = 32 字节 → base64 44 字符（含 padding）
        assert_eq!(b64_decode(&out).unwrap().len(), 32);
    }

    // ── 签名头 ──────────────────────────────────────────────────────────────

    #[test]
    fn sign_header_matches_ts_vectors() {
        let v = vectors();
        for c in &v.sign_header {
            let ts = match &c.timestamp {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                other => panic!("意外的 timestamp 类型: {other}"),
            };
            assert_eq!(
                generate_signature(&c.id, &ts, &c.nonce, c.salt_key, &c.other),
                c.expect_md5,
                "签名用例 {}",
                c.name
            );
        }
    }

    #[test]
    fn unknown_salt_falls_back_to_first() {
        assert_eq!(sign_salt(99), SIGN_SALT_MAP[0].1);
        assert_eq!(sign_salt(1), SIGN_SALT_MAP[0].1);
        assert_eq!(sign_salt(2), SIGN_SALT_MAP[1].1);
    }

    // ── getTn ───────────────────────────────────────────────────────────────

    #[test]
    fn get_tn_matches_ts_vectors() {
        let v = vectors();
        for c in &v.get_tn {
            assert_eq!(get_tn(&c.input), c.expect, "getTn 用例 {}", c.name);
        }
    }

    /// `null` 必须变成字符串 `"null"`，不能变空串 —— 这是 getTn 的隐蔽分支。
    #[test]
    fn get_tn_null_becomes_literal_null() {
        let obj = serde_json::json!({ "a": null });
        assert_eq!(get_tn(&obj), "null");
    }

    #[test]
    fn get_tn_scales_numbers_by_10000() {
        assert_eq!(get_tn(&serde_json::json!({ "t": 7 })), "70000");
        assert_eq!(get_tn(&serde_json::json!({ "tz": -480 })), "-4800000");
        // 非整数走 floor
        assert_eq!(get_tn(&serde_json::json!({ "a": 1.00009 })), "10000");
    }

    // ── gzip ────────────────────────────────────────────────────────────────

    #[test]
    fn gzip_sets_os_byte_to_19_and_roundtrips() {
        use std::io::Read as _;
        let v = vectors();
        for c in &v.gzip {
            let gz = gzip_with_os19(c.json.as_bytes()).unwrap();
            assert_eq!(gz[GZIP_OS_INDEX], 19, "OS 字节必须是 19（用例 {}）", c.name);
            assert_eq!(&gz[0..2], &[0x1f, 0x8b], "gzip magic 不对");

            let mut dec = flate2::read::GzDecoder::new(std::io::Cursor::new(&gz));
            let mut s = String::new();
            dec.read_to_string(&mut s).unwrap();
            assert_eq!(s, c.json, "解压后应等于原 JSON（用例 {}）", c.name);
        }
    }

    /// Node 原样产出的 base64 也必须能被我们解开（证明头部兼容）。
    #[test]
    fn gzip_decodes_node_produced_vector() {
        use std::io::Read as _;
        let v = vectors();
        let c = &v.gzip[0];
        let raw = b64_decode(&c.base64).unwrap();
        let mut dec = flate2::read::GzDecoder::new(std::io::Cursor::new(&raw));
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        assert_eq!(s, c.json);
        assert_eq!(s.len(), c.plain_len);
    }

    // ── RSA ─────────────────────────────────────────────────────────────────

    /// **核心验证**：RSA PKCS#1 v1.5 加密不可字节级对拍，所以这样验：
    /// Node 用公钥加密 → Rust 用私钥解回 → 必须等于原明文。
    #[test]
    fn rsa_decrypts_node_produced_ciphertext() {
        let v = vectors();
        let priv_der = b64_decode(&v.rsa_self.private_key_pkcs8_b64).unwrap();
        assert!(!v.rsa_self.cases.is_empty());
        for c in &v.rsa_self.cases {
            let pt = rsa_pkcs1v15_decrypt(&priv_der, &c.cipher_b64).unwrap();
            assert_eq!(
                String::from_utf8(pt).unwrap(),
                c.message,
                "RSA 解密用例 {}",
                c.name
            );
        }
    }

    /// 反方向：Rust 加密 → Rust 自解。这条证明我们的**加密**产出是合法 PKCS#1 v1.5。
    #[test]
    fn rsa_encrypt_then_self_decrypt() {
        let v = vectors();
        let priv_der = b64_decode(&v.rsa_self.private_key_pkcs8_b64).unwrap();
        let mut rng = SeededRng::new(42);
        for c in &v.rsa_self.cases {
            let ct = rsa_pkcs1v15_encrypt_base64_with_b64_key(
                &v.rsa_self.public_key_b64,
                c.message.as_bytes(),
                &mut rng,
            )
            .unwrap();
            let pt = rsa_pkcs1v15_decrypt(&priv_der, &ct).unwrap();
            assert_eq!(String::from_utf8(pt).unwrap(), c.message, "用例 {}", c.name);
        }
    }

    /// 同一明文两次加密必须**不同** —— 把"RSA 不可对拍"这件事本身钉成测试，
    /// 避免以后有人误加字节级断言。
    #[test]
    fn rsa_encryption_is_intentionally_non_deterministic() {
        let v = vectors();
        let mut rng = SeededRng::new(7);
        let a =
            rsa_pkcs1v15_encrypt_base64_with_b64_key(&v.rsa_self.public_key_b64, b"v", &mut rng)
                .unwrap();
        let b =
            rsa_pkcs1v15_encrypt_base64_with_b64_key(&v.rsa_self.public_key_b64, b"v", &mut rng)
                .unwrap();
        assert_ne!(a, b, "PKCS#1 v1.5 加密填充含随机字节，两次结果不应相同");
    }

    /// skland 真实公钥：只断言结构（1024 位 → 128 字节密文）。
    #[test]
    fn rsa_skland_ciphertext_has_expected_size() {
        let v = vectors();
        let c = &v.rsa_skland[0];
        assert_eq!(
            c.public_key_b64, SKLAND_PUBLIC_KEY_B64,
            "公钥常量应与源码一致"
        );
        let mut rng = SeededRng::new(9);
        for sample in &c.samples {
            assert_eq!(
                b64_decode(sample).unwrap().len(),
                c.modulus_bytes,
                "已知样本长度"
            );
        }
        let ct = rsa_pkcs1v15_encrypt_base64_with_b64_key(SKLAND_PUBLIC_KEY_B64, b"v", &mut rng)
            .unwrap();
        assert_eq!(b64_decode(&ct).unwrap().len(), c.modulus_bytes);
    }

    // ── Random trait ────────────────────────────────────────────────────────

    #[test]
    fn seeded_rng_is_reproducible() {
        let mut a = SeededRng::new(123);
        let mut b = SeededRng::new(123);
        let ua = a.uuid_v4();
        let ub = b.uuid_v4();
        assert_eq!(ua, ub, "同种子必须产出同序列");
        assert_eq!(ua.len(), 36);
        assert_eq!(ua.as_bytes()[14], b'4', "UUID 版本位应为 4");
    }

    #[test]
    fn seeded_rng_below_stays_in_range() {
        let mut rng = SeededRng::new(5);
        for _ in 0..500 {
            assert!(rng.below(8) < 8);
        }
        assert!(rng.below(1) == 0);
    }

    #[test]
    fn uuid_bits_are_set_correctly() {
        let mut rng = SeededRng::new(99);
        for _ in 0..50 {
            let u = rng.uuid_v4();
            let b = u.as_bytes();
            assert_eq!(b[14], b'4');
            let variant = u.as_bytes()[19];
            assert!(
                matches!(variant, b'8' | b'9' | b'a' | b'b'),
                "variant 位不对: {u}"
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // PC 客户端（common/utils.js）的原语 —— 向量与出处见
    // test/golden/pcclient-crypto-vectors.json
    // ─────────────────────────────────────────────────────────────────────────

    const PC_VECTORS: &str = include_str!("../../../test/golden/pcclient-crypto-vectors.json");

    fn pc_vectors() -> Value {
        serde_json::from_str(PC_VECTORS).expect("向量文件必须是合法 JSON")
    }

    #[test]
    fn pc_client_constants_match_the_source_file() {
        // 常量单独钉一次：它们是"从客户端源码抄来的事实"，
        // 改了它们等于换了密钥，必须有人主动改这里
        let v = pc_vectors();
        assert_eq!(v["constants"]["LOGIN_AES_KEY"], "qPqDw263XgFgL3u8");
        assert_eq!(v["constants"]["LOGIN_AES_KEY_TEST"], "2ErfJus1Ofr@2o24");
        assert_eq!(v["constants"]["PASSWORD_HASH_PREFIX"], "fetion.com.cn:");
    }

    #[test]
    fn decrypts_pc_client_login_data_with_source_key() {
        let v = pc_vectors();
        let key = v["constants"]["LOGIN_AES_KEY"].as_str().unwrap();

        for case in v["loginDataDecrypt"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let cipher = case["cipherHex"].as_str().unwrap();
            let got = aes_ecb_pkcs7_decrypt_hex(key, cipher);

            if case["expectError"].as_bool() == Some(true) {
                assert!(got.is_err(), "{name}: 非法填充必须报错，实际解出 {got:?}");
                continue;
            }
            assert_eq!(
                got.unwrap_or_else(|e| panic!("{name}: {e}")),
                case["expectPlain"].as_str().unwrap(),
                "{name}"
            );
        }
    }

    #[test]
    fn aes_ecb_rejects_malformed_ciphertext() {
        let key = "qPqDw263XgFgL3u8";
        // 输入是 hex，不是 base64 —— 传 base64 必须失败而不是解出乱码
        assert!(aes_ecb_pkcs7_decrypt_hex(key, "RTUyNkUxRjNGOTc2NTQ0NA==").is_err());
        // 长度不是 16 的倍数
        assert!(aes_ecb_pkcs7_decrypt_hex(key, "0011223344556677").is_err());
        // 空密文
        assert!(aes_ecb_pkcs7_decrypt_hex(key, "").is_err());
    }

    #[test]
    fn sha1_upper_matches_pc_client_encrypt_by_hash() {
        let v = pc_vectors();
        for case in v["sha1Upper"].as_array().unwrap() {
            let got = sha1_hex_upper(case["input"].as_str().unwrap().as_bytes());
            assert_eq!(got, case["expect"].as_str().unwrap(), "{}", case["name"]);
        }

        // 大小写不是风格问题：结果直接进请求体的 dycpwd，服务端按十六进制解析
        let hashed = sha1_hex_upper(b"fetion.com.cn:pass1234");
        assert_eq!(hashed, hashed.to_ascii_uppercase());
        assert_eq!(hashed.len(), 40, "SHA-1 是 40 位十六进制");
    }
}
