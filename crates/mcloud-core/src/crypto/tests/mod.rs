// 密码学单元测试的公共夹具：黄金向量、用例类型与取值器；各原语用例见同目录其它文件。
use super::*;

use serde::Deserialize;

use serde_json::Value;

const VECTORS: &str = include_str!("../../../../../test/golden/crypto-vectors.json");

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

// ─────────────────────────────────────────────────────────────────────────
// PC 客户端（common/utils.js）的原语 —— 向量与出处见
// test/golden/pcclient-crypto-vectors.json
// ─────────────────────────────────────────────────────────────────────────

const PC_VECTORS: &str = include_str!("../../../../../test/golden/pcclient-crypto-vectors.json");

fn pc_vectors() -> Value {
    serde_json::from_str(PC_VECTORS).expect("向量文件必须是合法 JSON")
}

// ─────────────────────────────────────────────────────────────────────────
// App 业务接口（形态 B）的三把锁：`signature` / `encodeData` / `hcy-cool-flag` 整包信封。
// 向量与出处见 test/golden/app-business-crypto-vectors.json ——
// **本文件是它唯一的 Rust 消费者**（少了它，黄金向量门禁会把它标成"死向量"）。
// ─────────────────────────────────────────────────────────────────────────

const APP_BUSINESS_VECTORS: &str =
    include_str!("../../../../../test/golden/app-business-crypto-vectors.json");

fn app_business_vectors() -> Value {
    serde_json::from_str(APP_BUSINESS_VECTORS).expect("向量文件必须是合法 JSON")
}

mod app;
mod gzip;
mod hash;
mod pc_client;
mod random;
mod rsa;
mod sign;
mod symmetric;
