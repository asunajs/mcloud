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

/// 标准 base64 编码（带 `=` 填充）。
///
/// 对外公开是因为**上游层需要它拼 URL**：App 端的滑块验证码 H5 页面把整组请求头
/// 塞进 `?data=<base64(JSON)>`（见 `mcloud_upstream::app_login::captcha_h5_url`），
/// 那是纯计算，不该为此让 `mcloud-upstream` 多背一个 `base64` 依赖。
pub fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, Error> {
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

/// 固定 IV 那条路的**反向**：hex 密文 → UTF-8 明文。
///
/// **仅测试与调试台使用**（与 [`rsa_pkcs1v15_decrypt`] 同理：生产链路不需要解）。
/// 存在的理由很具体：上报出去的 smid 载荷是单向加密的，光有加密函数就永远
/// 证明不了"发出去的那一份里装的确实是绑定给这个账号的特征"。
pub fn aes_cbc_fixed_iv_decrypt_hex(key: &str, cipher_hex: &str) -> Result<String, Error> {
    let k = js_pad_key16(key);
    let ct =
        hex::decode(cipher_hex).map_err(|e| Error::Parse(format!("AES 密文不是合法 hex: {e}")))?;
    let pt = cbc_pkcs7_decrypt(&k, AES_FIXED_IV, &ct)?;
    String::from_utf8(pt).map_err(|e| Error::Parse(format!("AES 明文不是 UTF-8: {e}")))
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

// ─────────────────────────────────────────────────────────────────────────────
// 3.6 AES-ECB/PKCS7 加密 + 10 字符错位 —— App 端 `MCLoginCryptoUtil`
// ─────────────────────────────────────────────────────────────────────────────

fn ecb_pkcs7_encrypt(key: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    macro_rules! enc {
        ($alg:ty) => {{
            use cipher::BlockEncrypt as _;
            let cipher = <$alg as KeyInit>::new_from_slice(key)
                .map_err(|e| Error::Internal(format!("AES 密钥不合法: {e}")))?;

            // 自己补 PKCS7 再逐块加密 —— 显式写出"块间无链接"，
            // 而不是靠某个 `_mut` 变体的默认行为。ECB 就是"每块独立"，
            // 这里多写两行是为了让这件事在代码里看得见。
            let pad = 16 - data.len() % 16;
            let mut buf = Vec::with_capacity(data.len() + pad);
            buf.extend_from_slice(data);
            buf.extend(std::iter::repeat_n(pad as u8, pad));
            for block in buf.chunks_exact_mut(16) {
                cipher.encrypt_block(GenericArray::from_mut_slice(block));
            }
            buf
        }};
    }

    match key.len() {
        16 => Ok(enc!(aes::Aes128)),
        24 => Ok(enc!(aes::Aes192)),
        32 => Ok(enc!(aes::Aes256)),
        n => Err(Error::Internal(format!("AES 密钥长度非法: {n}"))),
    }
}

/// AES-ECB/PKCS7 加密，输出 **hex** —— [`aes_ecb_pkcs7_decrypt_hex`] 的逆运算。
///
/// 与解密不同，这个方向在 PC 客户端里不存在（PC 只解密响应）；
/// 它是 **App 端**才有的：`MCLoginCryptoUtil +AESEncrypt:isCurrentEnvironment:`
/// 走 `CCCrypt(op=kCCEncrypt, alg=kCCAlgorithmAES, options=kCCOptionPKCS7Padding|kCCOptionECBMode,
/// keyLength=16)`，再 `hexStringFromData:`。
///
/// 密钥**原样使用**（ASCII 16 字节 → AES-128），不做截断/补零。
pub fn aes_ecb_pkcs7_encrypt_hex(key: &str, plaintext: &str) -> Result<String, Error> {
    Ok(hex::encode(ecb_pkcs7_encrypt(
        key.as_bytes(),
        plaintext.as_bytes(),
    )?))
}

/// App 端登录体的**末 10 位移到最前**错位（`MCLoginCryptoUtil +privateEncrypt:`）。
///
/// 反汇编实参（`-[MCLoginCryptoUtil privateEncrypt:isCurrentEnvironment:]`）：
///
/// ```objc
/// p    = MAX(len, 10) - 10;                        // MAX 是编译出的 csel
/// tail = [e substringFromIndex:p];                 // 末 10 个字符
/// head = [e substringToIndex:p];                   // 其余
/// out  = [NSString stringWithFormat:@"%@%@", tail, head];
/// ```
///
/// 长度不足 10 时 `p == 0`，两段拼回来等于原串 —— **恒等变换**，不是错误路径。
/// 所以本函数对短串原样返回，不报错（照抄客户端行为，别"顺手校验长度"）。
pub fn shift_tail10_to_front(s: &str) -> String {
    let n = s.chars().count();
    if n <= 10 {
        return s.to_string();
    }
    let cut = n - 10;
    let (head, tail) = s.split_at(s.char_indices().nth(cut).map_or(s.len(), |(i, _)| i));
    format!("{tail}{head}")
}

/// [`shift_tail10_to_front`] 的逆运算（`MCLoginCryptoUtil +privateDecrypt:`）。
///
/// ```objc
/// if (len < 11) return @"";                        // ← 客户端在此丢弃，返回空串
/// out = [NSString stringWithFormat:@"%@%@",
///            [s substringFromIndex:10],            // 去掉前 10 个
///            [s substringToIndex:10]];            // 前 10 个挪到末尾
/// ```
///
/// 与加密侧一样，长度 ≤ 10 时是恒等变换（此处的 `len < 11` 分支只是返回空串，
/// 对长度恰为 10 的输入两条路径同解）。
pub fn shift_head10_to_back(s: &str) -> String {
    let n = s.chars().count();
    if n <= 10 {
        return s.to_string();
    }
    let cut = s.char_indices().nth(10).map_or(s.len(), |(i, _)| i);
    let (head, tail) = s.split_at(cut);
    format!("{tail}{head}")
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

/// gzip 解压。与 [`gzip_with_os19`] 配成往返，**仅测试与调试台使用**。
///
/// 为什么要有它：`data` 字段解出来是一串 gzip 的 base64，而"上报体里带的
/// 字段值对不对"只能解开来看。没有它，smid 那一整段的断言就只能停在
/// "算出了一个 hex 串"—— 那等于没测。
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, Error> {
    use std::io::Read as _;

    let mut dec = flate2::read::GzDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| Error::Parse(format!("gzip 解压失败: {e}")))?;
    Ok(out)
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
mod tests;
