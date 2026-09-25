//! 请求签名。
//!
//! ```text
//! data = salt + id + timestamp + nonce + other + salt
//! sign = MD5(data)            // 小写 hex
//! ```
//!
//! 盐值只有两个，未知 `salt_key` 回落到 1（对齐原实现的 `|| SALT_MAP[1]`）。

use serde::{Deserialize, Serialize};

use crate::hash::md5_hex;

/// 签名盐值表。
pub const SALT_MAP: [(u8, &str); 2] = [(1, "seedMdYYLIZfbCxg"), (2, "sekaMdYYLIZfbCfm")];

/// 取签名盐值；未知 key 回落第 1 个。
pub const fn salt_for(salt_key: u8) -> &'static str {
    match salt_key {
        2 => SALT_MAP[1].1,
        _ => SALT_MAP[0].1,
    }
}

/// 计算签名。`timestamp` 以字符串参与拼接，避免 i64 / f64 的格式化差异。
pub fn generate_signature(
    id: &str,
    timestamp: &str,
    nonce: &str,
    salt_key: u8,
    other: &str,
) -> String {
    let salt = salt_for(salt_key);
    let mut data = String::with_capacity(
        salt.len() * 2 + id.len() + timestamp.len() + nonce.len() + other.len(),
    );
    data.push_str(salt);
    data.push_str(id);
    data.push_str(timestamp);
    data.push_str(nonce);
    data.push_str(other);
    data.push_str(salt);
    md5_hex(&data)
}

/// 签名相关的请求头。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignHeaders {
    pub x_request_id: String,
    pub x_timestamp: String,
    pub x_nonce: String,
    pub x_signature: String,
}

impl SignHeaders {
    /// 转为 `(name, value)` 列表，名称即线上形式。
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        vec![
            ("x-request-id".into(), self.x_request_id.clone()),
            ("x-timestamp".into(), self.x_timestamp.clone()),
            ("x-nonce".into(), self.x_nonce.clone()),
            ("x-signature".into(), self.x_signature.clone()),
        ]
    }
}

/// 用调用方提供的 id / nonce 计算签名头。
///
/// 显式传入随机量，使测试可复现；生产代码用 [`sign_headers`]。
pub fn sign_headers_with(
    id: impl Into<String>,
    nonce: impl Into<String>,
    timestamp_ms: i64,
    salt_key: u8,
    other: &str,
) -> SignHeaders {
    let id = id.into();
    let nonce = nonce.into();
    let ts = timestamp_ms.to_string();
    let sig = generate_signature(&id, &ts, &nonce, salt_key, other);
    SignHeaders {
        x_request_id: id,
        x_timestamp: ts,
        x_nonce: nonce,
        x_signature: sig,
    }
}

/// 生成签名头，随机量自动产生。
pub fn sign_headers(timestamp_ms: i64, salt_key: u8, other: &str) -> SignHeaders {
    sign_headers_with(
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        timestamp_ms,
        salt_key,
        other,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salt_lookup_and_fallback() {
        assert_eq!(salt_for(1), "seedMdYYLIZfbCxg");
        assert_eq!(salt_for(2), "sekaMdYYLIZfbCfm");
        // 未知 key 回落第 1 个
        assert_eq!(salt_for(0), SALT_MAP[0].1);
        assert_eq!(salt_for(99), SALT_MAP[0].1);
    }

    #[test]
    fn signature_is_salt_wrapped_twice() {
        let salt = salt_for(1);
        let expected = md5_hex(&format!("{salt}id1ts1n1{salt}"));
        assert_eq!(generate_signature("id1", "ts1", "n1", 1, ""), expected);
    }

    #[test]
    fn other_is_concatenated_before_trailing_salt() {
        let salt = salt_for(2);
        let expected = md5_hex(&format!("{salt}id1ts1n1XYZ{salt}"));
        assert_eq!(generate_signature("id1", "ts1", "n1", 2, "XYZ"), expected);
    }

    #[test]
    fn headers_pairs_use_wire_names() {
        let h = sign_headers_with("i", "n", 1700000000000, 1, "");
        let pairs = h.to_pairs();
        let names: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            vec!["x-request-id", "x-timestamp", "x-nonce", "x-signature"]
        );
    }

    #[test]
    fn random_headers_are_deterministic_given_inputs() {
        let a = sign_headers_with("i", "n", 123, 1, "");
        let b = sign_headers_with("i", "n", 123, 1, "");
        assert_eq!(a, b);
    }

    #[test]
    fn random_headers_differ_between_calls() {
        let a = sign_headers(123, 1, "");
        let b = sign_headers(123, 1, "");
        assert_ne!(a.x_nonce, b.x_nonce, "nonce 必须每次不同");
    }
}
