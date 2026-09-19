// 本模块单元测试的公共 helper（确定性随机源、夹具取值器）；主题用例见同目录其它文件。
use super::*;

/// 确定性 `Random`：按顺序吐字节，让 32 位 / 24 位十六进制的断言可复现。
struct SeqRandom(u8);

impl Random for SeqRandom {
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        for byte in dst.iter_mut() {
            *byte = self.0;
            self.0 = self.0.wrapping_add(1);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 密码学：向量与出处见 test/golden/pcclient-crypto-vectors.json
// ─────────────────────────────────────────────────────────────────────────

const PC_VECTORS: &str = include_str!("../../../../../test/golden/pcclient-crypto-vectors.json");

fn pc_vectors() -> Value {
    serde_json::from_str(PC_VECTORS).expect("向量文件必须是合法 JSON")
}

mod classify;
mod decrypt;
mod puzzle;
mod requests;
