//! 特征体的 `tn` 摘要输入 —— `getTn` 的逐条复刻。
//!
//! # 这个函数存在的唯一理由
//!
//! smid 特征体里有一个 `tn` 字段，值是 `md5(getTn(整份特征体))`。
//! 它不是签名（服务端不做校验意义上的比对），但**必须算得一模一样**：
//! 它是服务端侧用来识别"这份特征有没有被改过"的一个摘要。
//!
//! # 规则（逐条对齐旧实现，别按直觉改）
//!
//! 1. 只处理 JSON 对象；**按键名排序**后遍历（排序是内容的一部分）；
//! 2. `number` 一律 `* 10000`：
//!    - 整数 → `v * 10000`
//!    - 非整数 → `Math.floor(v * 10000)`
//! 3. `object`（含数组，不含 `null`）→ **递归**；
//! 4. 其余按 `String(v)` 拼接，注意 `null` → `"null"`（**不是空串**）、
//!    `bool` → `"true"` / `"false"`。
//!
//! # 已知边界（旧实现如此，向量未覆盖，不要"顺手修好"）
//!
//! - **键排序**：JS 按 UTF-16 码元，这里按 UTF-8 字节；非 BMP 字符下顺序可能不同。
//!   特征体的键全是 ASCII，所以当前等价。
//! - **极大/极小数字**：JS 会输出 `1e+21` 这类指数形式，Rust 的 `f64` `Display`
//!   不会。特征体里的数字都是屏幕尺寸、时间戳这种量级，碰不到这条边界。
//!
//! 把这两条写下来而不是"修一版更正确的"：**对拍基准是旧实现的实际行为**，
//! 不是我们期望的正确行为。要改就得先有新的抓包证据。

use serde_json::Value;

use crate::hash::md5_hex;

/// 按键名排序，把 JSON 的值拼成一串（`getTn` 的返回）。
///
/// 非对象输入返回空串 —— 对齐旧实现对非对象的宽容处理。
pub fn get_tn(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return String::new();
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();

    let mut out = String::new();
    for k in keys {
        match &obj[k] {
            Value::Number(n) => {
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
            v @ Value::Object(_) => out.push_str(&get_tn(v)),
            Value::String(s) => out.push_str(s),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Null => out.push_str("null"),
            v @ Value::Array(_) => out.push_str(&get_tn(v)),
        }
    }
    out
}

/// smid 的 `tn` 字段：`md5(getTn(obj))`。
///
/// # 调用顺序不能换
///
/// 旧实现是 `desTarget.tn = md5Hash(getTn(desTarget))`：**求值时 `tn` 还是空串**。
/// 先把 `tn` 写回再算摘要，摘要里会多出一段 32 位十六进制，整份指纹当场作废 ——
/// 而且线上不会报错，只是认不出一台设备。smid 模块的
/// `tn_must_be_digest_of_the_target_while_tn_is_still_empty` 钉的就是这一步。
pub fn get_tn_md5(value: &Value) -> String {
    md5_hex(&get_tn(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_scaled_by_10000_and_ints_print_without_a_decimal_point() {
        // 整数：7 → 70000；负数：-480 → -4800000
        let got = get_tn(&json!({"time": 7, "timezone": -480}));
        // 键名排序：time < timezone
        assert_eq!(got, "70000-4800000");
    }

    #[test]
    fn non_integers_are_floored_after_scaling() {
        // 1.234567 * 10000 = 12345.67 → floor → 12345
        assert_eq!(get_tn(&json!({"a": 1.234567})), "12345");
    }

    #[test]
    fn objects_recurse_and_keys_are_sorted() {
        assert_eq!(
            get_tn(&json!({"outer": "o", "inner": {"z": 1, "a": "q"}})),
            "q10000o"
        );
    }

    #[test]
    fn null_is_literal_null_and_bool_is_lowercase() {
        assert_eq!(get_tn(&json!({"d": null})), "null");
        assert_eq!(get_tn(&json!({"a": true, "b": false})), "truefalse");
    }

    #[test]
    fn non_object_yields_empty_string() {
        assert_eq!(get_tn(&json!("hello")), "");
        assert_eq!(get_tn(&json!(7)), "");
        assert_eq!(get_tn(&json!(null)), "");
    }

    #[test]
    fn get_tn_matches_the_ts_vectors() {
        // 与 test/golden/crypto-vectors.json 的 getTn 段逐条一致。
        // 注意向量的 `expect` 是**拼接串本身**（不是 md5）——
        // 向量生成脚本在那一层还没有取摘要。
        let cases: Value =
            serde_json::from_str(include_str!("../../../test/golden/crypto-vectors.json"))
                .expect("黄金向量应能解析");
        let list = cases["getTn"].as_array().expect("getTn 段应是数组");
        assert!(!list.is_empty(), "getTn 段不能为空");
        for c in list {
            assert_eq!(
                get_tn(&c["input"]),
                c["expect"].as_str().unwrap(),
                "getTn 用例 {}",
                c["name"].as_str().unwrap_or("?")
            );
        }
    }

    #[test]
    fn tn_md5_is_the_digest_of_the_concatenation() {
        // `get_tn_md5` 是 getTn 外面再套一层 md5：两条必须能对上，
        // 否则 smid 的 tn 字段会与旧实现不同（而线上不报错，只是认不出设备）。
        let v = json!({"protocol": 4, "organization": "FXly"});
        assert_eq!(get_tn_md5(&v), md5_hex(&get_tn(&v)));
    }
}
