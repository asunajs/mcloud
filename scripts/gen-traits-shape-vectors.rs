//! 生成 test/golden/traits-shape-vectors.json —— IO 边界类型的 JSON 形状。
//!
//! # 为什么需要它
//!
//! `HttpRequest` 的注释里写着"结构刻意保持可序列化且可对拍：golden request
//! 对拍依赖它逐字段可比"，而 **step 2（mcloud-upstream）正是靠它做请求对拍的**。
//! 但 Rust 侧对它的形状**只有一个断言**（`http_method_serializes_uppercase`，
//! 且只验了 `HttpMethod` 一个枚举的一个变体）—— 结构体的形状一个都没钉过。
//!
//! 按"迁一个带 JSON 的类型时先问'它的形状有没有被断言过'"这条判据，
//! 没被断言过的就得让**参考实现的序列化库**给出答案，而不是把命名规则再推一遍。
//!
//! # ⚠ 为什么 path 依赖真实的 mcloud-core，而不是在这里镜像一份类型定义
//!
//! 镜像抄错过一次：`ScheduleTarget` 漏抄 `#[serde(tag = "kind")]`，产出 serde
//! 默认的外部标签 —— 类型系统抓不到，只有整串比对抓到了。这里要验的正是
//! `Vec<u8>` / `Vec<(String, String)>` / `Option` + `skip_serializing_if`
//! 这些**属性驱动的**形状，镜像的抄错概率只会更高。
//!
//! ⇒ 直接用冻结那一刻的类型定义**本身**，没有第二份可与之分叉的副本。
//!
//! # 怎么重新生成（离线）
//!
//! ```sh
//! rm -rf /tmp/mcloud-traits-shape && mkdir -p /tmp/mcloud-traits-shape/src
//! cp scripts/gen-traits-shape-vectors.rs /tmp/mcloud-traits-shape/src/main.rs
//! cat > /tmp/mcloud-traits-shape/Cargo.toml <<'TOML'
//! [package]
//! name = "mcloud-traits-shape"
//! version = "0.0.0"
//! edition = "2021"
//!
//! [dependencies]
//! mcloud-core = { path = "<仓库根的绝对路径>/legacy/crates/mcloud-core" }
//! serde = { version = "1", features = ["derive"] }
//! serde_json = "1"
//! TOML
//! cd /tmp/mcloud-traits-shape
//! CARGO_NET_OFFLINE=true cargo run --quiet > test/golden/traits-shape-vectors.json
//! ```
//!
//! ⚠ path 必须是**绝对路径**：生成器跑在 /tmp 下，相对路径会指到别处。

use mcloud_core::{HttpMethod, HttpRequest, HttpResponse};
use serde_json::{Value, json};

fn main() {
    let mut out = serde_json::Map::new();

    // ── HttpMethod 的文本（rename_all = "UPPERCASE"，一族新的命名规则）──────
    let mut enum_text: Vec<Value> = Vec::new();
    for (variant, m) in [
        ("Get", HttpMethod::Get),
        ("Post", HttpMethod::Post),
        ("Put", HttpMethod::Put),
        ("Delete", HttpMethod::Delete),
    ] {
        enum_text.push(json!({
            "enum": "HttpMethod",
            "variant": variant,
            "text": serde_json::to_value(m).expect("HttpMethod 必然能序列化"),
            "asStr": m.as_str(),
            "isRead": m.is_read(),
        }));
    }
    out.insert("$enumText".to_string(), Value::Array(enum_text));

    // ── 结构体形状 ──────────────────────────────────────────────────────────
    //
    // 要回答的三个问题：
    //   1. `body: Option<Vec<u8>>` 是数字数组还是 base64 串？
    //      （Go 的 `[]byte` 默认是 **base64 串** —— 若这里给的是数字数组，
    //       两侧写出的 golden request 文件就不是同一份东西。）
    //   2. `headers: Vec<(String, String)>` 是二维数组还是对象？
    //   3. `skip_serializing_if = "Option::is_none"` 的字段缺席时**是否出现**。
    let mut struct_json: Vec<Value> = Vec::new();

    push(&mut struct_json, "request-get-minimal", &HttpRequest::get("https://example.com/a"));

    push(
        &mut struct_json,
        "request-post-headers",
        &HttpRequest::post("https://example.com/b")
            .header("x-a", "1")
            .header("x-b", "2"),
    );

    // json_body 会自己补一个 content-type（仅当没有同名 header 时）。
    push(
        &mut struct_json,
        "request-json-body",
        &HttpRequest::post("https://example.com/c")
            .json_body(&json!({ "phone": "19900001234", "n": 1 }))
            .expect("json 体必然能序列化"),
    );

    // 已经带 content-type 时**不**再补（大小写不敏感比较）。
    push(
        &mut struct_json,
        "request-content-type-kept",
        &HttpRequest::post("https://example.com/d")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .json_body(&json!({ "a": 1 }))
            .expect("json 体必然能序列化"),
    );

    push(
        &mut struct_json,
        "request-full",
        &HttpRequest::post("https://example.com/e")
            .header("x-trace", "t-1")
            .json_body(&json!({ "a": 1 }))
            .expect("json 体必然能序列化")
            .timeout_ms(3000)
            .idempotency_key("run-1"),
    );

    push(&mut struct_json, "response-200", &HttpResponse::new(200, br#"{"a":1}"#.to_vec()));

    push(
        &mut struct_json,
        "response-empty-body",
        &HttpResponse { status: 404, headers: vec![("content-type".into(), "text/plain".into())], body: Vec::new() },
    );

    out.insert("$structJson".to_string(), Value::Array(struct_json));

    // ── 解码侧的宽严 ────────────────────────────────────────────────────────
    //
    // ⚠ 这一节回答的是"serde 收不收"，不是"我们想不想收"。
    // Go 侧必须逐条对齐，否则一侧拒的行另一侧读成合法值。
    let mut decode_cases: Vec<Value> = Vec::new();

    // HttpMethod：UPPERCASE 是否也约束**解码**（"get" 到底收不收）。
    for raw in ["\"GET\"", "\"get\"", "\"Get\"", "\"POST\"", "\"PATCH\"", "0", "null"] {
        let r: Result<HttpMethod, _> = serde_json::from_str(raw);
        decode_cases.push(json!({ "type": "HttpMethod", "json": raw, "ok": r.is_ok() }));
    }

    // HttpRequest：可选字段缺席时收不收；`headers` / `body` 缺席时收不收。
    for raw in [
        r#"{"method":"GET","url":"https://example.com/a"}"#,
        r#"{"method":"GET","url":"https://example.com/a","headers":[]}"#,
        r#"{"method":"GET","url":"https://example.com/a","headers":null}"#,
        r#"{"method":"POST","url":"https://example.com/a","body":null}"#,
        r#"{"method":"POST","url":"https://example.com/a","body":[123,34]}"#,
        r#"{"method":"POST","url":"https://example.com/a","body":"eyJhIjox"}"#,
        r#"{"method":"get","url":"https://example.com/a"}"#,
        r#"{"url":"https://example.com/a"}"#,
        r#"{"method":"GET"}"#,
        r#"{"method":"GET","url":null}"#,
        r#"{"method":null,"url":"https://example.com/a"}"#,
        // 元组必须是**两个**元素：只给一个是"缺席"不了的另一种坏形状。
        r#"{"method":"GET","url":"https://example.com/a","headers":[["x-a"]]}"#,
    ] {
        let r: Result<HttpRequest, _> = serde_json::from_str(raw);
        decode_cases.push(json!({ "type": "HttpRequest", "json": raw, "ok": r.is_ok() }));
    }

    // HttpResponse：`body` 缺席时收不收。
    //
    // ⚠ 刻意也试 `body: null`：它是 `#[serde(default)] Vec<u8>`（**不是** Option），
    // 按"default 只管缺席"的读法应当拒 —— 但这条与 HttpRequest 的
    // `body: Option<Vec<u8>>`（实测**收**）正好相反，不实测就会想当然地对齐。
    for raw in [
        r#"{"status":200}"#,
        r#"{"status":200,"body":[]}"#,
        r#"{"status":200,"body":[123]}"#,
        r#"{"status":200,"body":null}"#,
        // ⚠ `status` **没有** `#[serde(default)]` ⇒ 缺席也是失败。
        // 与 `headers` / `body`（有 default）正好相反，别想当然地对齐。
        r#"{"headers":[],"body":[]}"#,
    ] {
        let r: Result<HttpResponse, _> = serde_json::from_str(raw);
        decode_cases.push(json!({ "type": "HttpResponse", "json": raw, "ok": r.is_ok() }));
    }

    out.insert("$decode".to_string(), Value::Array(decode_cases));

    // ── 往返：缺席字段解回来是什么 ──────────────────────────────────────────
    //
    // 这一节专门回答"解回来之后的**再序列化**是否与输入同形" ——
    // `#[serde(default)] Vec` 缺席时是 `vec![]`（再序列化会**多出** `[]`），
    // 而 Go 的 nil slice 会输出 `null`。两侧不同形，必须显式归一化。
    let mut round_trip: Vec<Value> = Vec::new();
    for raw in [
        r#"{"method":"GET","url":"https://example.com/a"}"#,
        r#"{"status":200}"#,
    ] {
        //
        // ⚠ 刻意**不**经 `to_value` 中转：`serde_json::Value::Object` 默认是
        // `BTreeMap`，一转就把字段顺序变成字母序，而这里要验的正是
        // "**结构体**声明顺序下的输出"。直接 `to_string(&req)` 才是那个形状。
        let (json, back): (String, Value) = if raw.contains("method") {
            let req: HttpRequest = serde_json::from_str(raw).expect("必然能解");
            (
                serde_json::to_string(&req).expect("必然能序列化"),
                serde_json::to_value(&req).expect("必然能序列化"),
            )
        } else {
            let resp: HttpResponse = serde_json::from_str(raw).expect("必然能解");
            (
                serde_json::to_string(&resp).expect("必然能序列化"),
                serde_json::to_value(&resp).expect("必然能序列化"),
            )
        };
        let input: Value = serde_json::from_str(raw).expect("输入必然合法");
        round_trip.push(json!({ "input": raw, "json": json, "same": input == back }));
    }
    out.insert("$roundTrip".to_string(), Value::Array(round_trip));

    let rendered = serde_json::to_string_pretty(&Value::Object(out)).expect("必然能序列化");
    println!("{rendered}");
}

fn push(out: &mut Vec<Value>, kind: &str, value: &impl serde::Serialize) {
    out.push(json!({
        "kind": kind,
        "json": serde_json::to_string(value).expect("结构体必然能序列化"),
    }));
}
