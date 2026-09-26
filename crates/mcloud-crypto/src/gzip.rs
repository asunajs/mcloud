//! gzip 压缩 —— smid 上报体里 `data` 字段的必需步骤。
//!
//! # OS 字节必须是 19
//!
//! gzip 头的第 10 个字节（下标 9）是 "OS" 字段，标记压缩端的操作系统。
//! 上游特征体走的是 `CompressionStream("gzip")`，它产出的就是 **19**（未知/现代浏览器）。
//! `flate2` 默认写的不是 19，所以这里**显式改**。
//!
//! 这不是"魔改协议"，而是**归一化**：客户端与服务端都不解析这个字节，
//! 但它在字节级对拍里会露出来（见 `test/golden/crypto-vectors.json` 的 `gzip` 段，
//! 那里就带着 `osByteBefore` / `osByteAfter`）。
//!
//! # 为什么不做字节级对拍
//!
//! deflate 的具体输出**因实现而异**（压缩级别、匹配策略、窗口实现都会影响）。
//! 所以黄金向量里 `byteExact: false`，只要求两条：
//! 能 gunzip 回原 JSON，且 OS 字节 == 19。

use std::io::Write as _;

use crate::{CryptoError, Result};

/// gzip 头里 OS 字段的下标。
const GZIP_OS_INDEX: usize = 9;
/// gzip 头里 OS 字段必须被改成的值。
const GZIP_OS_VALUE: u8 = 19;

/// gzip 压缩，并**强制把 OS 字节写成 19**。
pub fn gzip_with_os19(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = flate2::GzBuilder::new().write(Vec::new(), flate2::Compression::default());
    enc.write_all(data)
        .map_err(|e| CryptoError::Internal(format!("gzip 写入失败：{e}")))?;
    let mut out = enc
        .finish()
        .map_err(|e| CryptoError::Internal(format!("gzip 收尾失败：{e}")))?;
    if out.len() > GZIP_OS_INDEX {
        out[GZIP_OS_INDEX] = GZIP_OS_VALUE;
    }
    Ok(out)
}

/// gzip 压缩后 base64（smid 载荷用的就是这个）。
pub fn gzip_with_os19_base64(data: &[u8]) -> Result<String> {
    let gz = gzip_with_os19(data)?;
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(gz))
}

/// gzip 解压。
///
/// **只服务于测试与调试**：上报出去的 `data` 是单向加密的，
/// 光有加密函数就永远证明不了"发出去的那一份里装的确实是绑定给这个账号的特征"。
/// 有了它，测试才能把 `data` 一路解回特征体并逐字段断言。
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read as _;

    let mut dec = flate2::read::GzDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| CryptoError::Parse(format!("gzip 解压失败：{e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_byte_is_normalised_to_19() {
        let out = gzip_with_os19(br#"{"a":1}"#).unwrap();
        assert_eq!(out[GZIP_OS_INDEX], 19, "OS 字节必须被改成 19");
    }

    #[test]
    fn roundtrip_returns_the_original_bytes() {
        let raw = br#"{"protocol":4,"appId":"default","os":"web"}"#;
        let out = gzip_with_os19(raw).unwrap();
        assert_eq!(gunzip(&out).unwrap(), raw);
    }

    #[test]
    fn base64_variant_decodes_to_the_same_gzip() {
        let raw = b"hello smid";
        let b64 = gzip_with_os19_base64(raw).unwrap();
        use base64::Engine as _;
        let gz = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        assert_eq!(gz[GZIP_OS_INDEX], 19);
        assert_eq!(gunzip(&gz).unwrap(), raw);
    }
}
