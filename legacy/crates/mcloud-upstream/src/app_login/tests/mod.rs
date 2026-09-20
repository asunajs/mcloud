// 本模块单元测试的公共 helper（确定性随机源、夹具取值器）；主题用例见同目录其它文件。
use super::*;

use mcloud_core::Random;

/// 确定性随机源：把给定字节循环填出去。
///
/// 用它而不是真实 `OsRng`，是为了让 [`new_login_random`] 的断言可复现 ——
/// "随机的形状"（长度、字符集）才是被测对象，具体取值不是。
struct SeqRandom {
    bytes: Vec<u8>,
    at: usize,
}

impl SeqRandom {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            at: 0,
        }
    }
}

impl Random for SeqRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for slot in dst.iter_mut() {
            *slot = self.bytes[self.at % self.bytes.len()];
            self.at += 1;
        }
    }
}

/// 抓包里那台设备的脱敏形态（与 `docs/capture/app-sms-login-samples.txt` 一致）。
fn profile() -> AppProfile {
    AppProfile::new(AppDevice::ipad(
        "AAA00000000000000000000000000000",
        "AAA000000000",
    ))
}

const PHONE: &str = "19900000000";
const TS: i64 = 1_789_743_362_439_040;

/// 抓包里那条明文的脱敏形态（186 字节）。
fn captured_plaintext() -> String {
    sms_thirdlogin_body(PHONE, "429344", "hOjseB").to_string()
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> &'a str {
    request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .unwrap_or_else(|| panic!("缺少头 {name}"))
}

mod bodies;
mod crypto;
mod h5_classify;
mod requests;
