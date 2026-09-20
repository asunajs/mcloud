// ─────────────────────────────────────────────────────────────────────────────
// 测试：黄金向量 + 逐字段断言
// ─────────────────────────────────────────────────────────────────────────────
//
// 这里**只测纯函数**（请求构造 + 响应解析）—— 它们承载了本模块的全部逻辑。
// `impl Upstream` 那几个 `app_*` 方法是三行包装（建请求 → `call` → 解析），
// 而 `mcloud-upstream` 的依赖树里**没有 async runtime**（这是刻意的：
// 本层禁止 IO，能跑异步的只有 `mcloud-io`），所以包装层由 `mcloud-app`
// 的编排测试覆盖，不在这里硬造一个 runtime。

use super::*;
use mcloud_core::HttpRequest;
use mcloud_core::Random;
use serde_json::Value;
// `sign_salt` 只在测试里用（把常量与向量、与 core 三者钉在一起），
// 生产代码走的是 `generate_signature`，所以它不进模块顶部的 import。

/// 黄金向量（真实抓包 + 真实响应）。
fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../test/golden/app-signin-vectors.json"
    ))
    .expect("黄金向量文件必须是合法 JSON")
}

/// 确定性随机源：把给定字节循环填出去。
///
/// 用它而不是真实 CSPRNG，是为了让 [`sign_h5_request`] 可复现 ——
/// 被测的是"两个 uuid + 一次 md5"这个形状，不是具体取值。
struct SeqRandom {
    bytes: Vec<u8>,
    at: usize,
}

impl SeqRandom {
    fn new(seed: u8) -> Self {
        Self {
            bytes: (0..64).map(|i| seed.wrapping_add(i as u8)).collect(),
            at: 0,
        }
    }
}

impl Random for SeqRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for b in dst.iter_mut() {
            *b = self.bytes[self.at % self.bytes.len()];
            self.at += 1;
        }
    }
}

/// 向量里的那个 `x-signature` 实例。
fn captured_signature() -> Value {
    vectors()["h5Signature"][0].clone()
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

mod backup_gift;
mod errors;
mod prize;
mod receive;
mod records;
mod revival;
mod session;
mod sign_in;
mod signature;
mod vectors;
