//! smid 模块的测试。
//!
//! 按主题拆成几个文件（对齐 docs/conventions.md §8.3 的分层策略）：
//! 黄金向量对拍、特征体、载荷编码、DES 规则表、请求与响应。
//!
//! 公共 helper（确定性随机源、夹具取值器）在本文件里，子模块用 `super::*` 取。

use super::*;
use serde::Deserialize;

/// TS 原版产出的黄金向量。
///
/// 本模块里**纯计算**的那几段（smid 拼接、priId、tn 摘要、DES 改名表）
/// 全部以这份文件为准 —— 生成侧用旧实现，消费侧用本项目自带的 DES/AES 实现，
/// 两边同值才叫交叉验证。
const VECTORS: &str = include_str!("../../../../test/golden/crypto-vectors.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SmidStringCase {
    pub name: String,
    pub time_digits: String,
    pub uid: String,
    pub expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SmidPriIdCase {
    pub name: String,
    pub uid: String,
    pub expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SmidTnCase {
    pub name: String,
    /// **`tn` 仍为空串**的特征体（摘要就是在这一刻算的）
    pub target: Value,
    pub expect: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SmidDesCase {
    pub name: String,
    /// 已经把 `tn` 写回去的特征体
    pub target: Value,
    /// 改名 + 加密之后的上报体，整份比对（键和值都不许差）
    pub expect: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SmidVectors {
    pub smid_string: Vec<SmidStringCase>,
    pub smid_pri_id: Vec<SmidPriIdCase>,
    pub smid_tn: Vec<SmidTnCase>,
    pub smid_des_rules: Vec<SmidDesCase>,
}

pub(super) fn smid_vectors() -> SmidVectors {
    serde_json::from_str(VECTORS).expect("黄金向量文件应能解析出 smid* 四段")
}

/// 顺序吐字节的随机源（`mcloud_crypto::SeqRandom` 的薄包装，便于本模块直接取用）。
pub(super) fn seq_rng(start: u8) -> mcloud_crypto::SeqRandom {
    mcloud_crypto::SeqRandom::new(start)
}

pub(super) fn upload() -> SmUpload<'static> {
    SmUpload {
        time_digits: "20260919153000",
        now_millis: 1_789_743_328_974,
        old_device_id: "",
    }
}

pub(super) fn fp() -> SmFingerprint {
    SmFingerprint::caiyun_android_web()
}

/// 规则表里加密过的字段，密文要一模一样地算出来。
pub(super) fn des(key: &str, message: &str) -> Value {
    Value::String(des_encrypt_b64(message, key).unwrap())
}

mod des_rules;
mod fingerprint;
mod payload;
mod request_response;
mod smid_string;
mod ts_vectors;
