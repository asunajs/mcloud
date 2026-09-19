// 本模块单元测试的公共 helper（确定性随机源、夹具取值器）；主题用例见同目录其它文件。
use super::*;

use serde::Deserialize;

/// TS 原版（`shared/src/smid/index.ts` + crypto-js 4.2.0）产出的黄金向量。
///
/// 本模块里**纯计算**的那几段（smid 拼接、priId、tn 摘要、DES 改名表）
/// 全部以这份文件为准 —— 生成侧用 crypto-js，消费侧用本项目自带的 DES/AES 实现，
/// 两边同值才叫交叉验证（见 `scripts/gen-crypto-vectors.cjs` 的 smid* 四段）。
const VECTORS: &str = include_str!("../../../../../test/golden/crypto-vectors.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SmidStringCase {
    name: String,
    time_digits: String,
    uid: String,
    expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SmidPriIdCase {
    name: String,
    uid: String,
    expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SmidTnCase {
    name: String,
    /// **`tn` 仍为空串**的特征体（摘要就是在这一刻算的）
    target: Value,
    expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SmidDesCase {
    name: String,
    /// 已经把 `tn` 写回去的特征体
    target: Value,
    /// 改名 + 加密之后的上报体，整份比对（键和值都不许差）
    expect: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SmidVectors {
    smid_string: Vec<SmidStringCase>,
    smid_pri_id: Vec<SmidPriIdCase>,
    smid_tn: Vec<SmidTnCase>,
    smid_des_rules: Vec<SmidDesCase>,
}

fn smid_vectors() -> SmidVectors {
    serde_json::from_str(VECTORS).expect("黄金向量文件应能解析出 smid* 四段")
}

/// 顺序吐字节的随机源。
///
/// 用它而不是真随机：本模块的**全部**随机都收敛到了 `Random`，所以只要
/// 给定起点，整份特征、uid、RSA 填充都可复现 —— 可复现才有下面那些断言。
struct SeqRng(u8);

impl Random for SeqRng {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for byte in dst.iter_mut() {
            *byte = self.0;
            self.0 = self.0.wrapping_add(1);
        }
    }
}

fn upload() -> SmUpload<'static> {
    SmUpload {
        time_digits: "20260919153000",
        now_millis: 1_789_743_328_974,
        old_device_id: "",
    }
}

fn fp() -> SmFingerprint {
    SmFingerprint::caiyun_android_web()
}

/// 规则表里加密过的字段，密文要一模一样地算出来。
fn des(key: &str, message: &str) -> Value {
    Value::String(des_ecb_no_padding_encrypt_base64(key, message).unwrap())
}

mod des_rules;
mod fingerprint;
mod payload;
mod request_response;
mod smid_string;
mod ts_vectors;
