//! 第九份黄金向量 `test/golden/legacy-shape-vectors.json` 的生成器。
//!
//! # 为什么它**必须**依赖真实的 `mcloud-core`
//!
//! 前八份向量里，需要"参考实现的形状"时，做法是在本文件里**镜像**一份类型定义
//! 再让 serde 去序列化（见 `gen-enum-json-vectors.rs`）。那份镜像抄错过一次：
//! `ScheduleTarget` 漏抄了 `#[serde(tag = "kind")]`，于是产出的是 serde 默认的
//! 外部标签 —— 与库里已存的形状完全不同，而**类型系统抓不到**。
//!
//! legacy 这一批有 14 个类型、七八十个字段，镜像的抄错概率只会更高。
//! 所以这里改成 **path 依赖真实的 `mcloud-core`**（`legacy/crates/mcloud-core`）:
//! 用的是冻结那一刻的类型定义本身，没有第二份可以与之分叉的副本。
//!
//! ⚠ `legacy/` 是**只读**的对拍基准；依赖它只是读，不修改它。
//!
//! # 为什么 legacy 的形状特别值得钉
//!
//! 旧文档的所有字段都带 `#[serde(default)]` ⇒ **字段名写错不会报错，只会静默读到零值**。
//! 表现不是"解析失败"，而是"历史数据读出来是空的、且没有任何线索指向字段名"。
//! 这与"枚举序列化成数字"是同一类病，但后果更直接：凭据挂不到账号上。
//!
//! 而 Rust 侧对这批形状**一个断言都没有**（`legacy.rs` 的测试只做字段值断言，
//! 从不序列化）。照"逐条平移已有测试"的常规做法，这个洞会被整块平移过去。
//!
//! # 怎么重新生成
//!
//! ```sh
//! mkdir -p /tmp/mcloud-legacy-shape/src
//! cp scripts/gen-legacy-shape-vectors.rs /tmp/mcloud-legacy-shape/src/main.rs
//! cat > /tmp/mcloud-legacy-shape/Cargo.toml <<'TOML'
//! [package]
//! name = "mcloud-legacy-shape"
//! version = "0.0.0"
//! edition = "2021"
//!
//! [dependencies]
//! mcloud-core = { path = "<仓库绝对路径>/legacy/crates/mcloud-core" }
//! serde_json = "1"
//! TOML
//! cd /tmp/mcloud-legacy-shape
//! MCLOUD_ROOT=<仓库绝对路径> CARGO_NET_OFFLINE=true cargo run --quiet \
//!   > <仓库绝对路径>/test/golden/legacy-shape-vectors.json
//! ```
//!
//! ⚠ 两处路径都必须是**绝对路径**，且 `MCLOUD_ROOT` 不能省：
//! 生成器跑在 /tmp 下，相对路径会指到别处；而 `$fixtureKeys` 要读
//! `test/fixtures/legacy/*.json`，只能从仓库根算起。

use mcloud_core::PipelineStep;
use mcloud_core::legacy::{
    LegacyAccountConfig, LegacyBinding, LegacyConfig, LegacyLogEntry, LegacyPipeline, LegacyRun,
    LegacySchedule, LegacyStep,
};
use serde_json::{Value, json};

fn main() {
    let mut out = serde_json::Map::new();
    out.insert(
        "$generator".to_string(),
        json!("scripts/gen-legacy-shape-vectors.rs"),
    );
    out.insert(
        "$comment".to_string(),
        json!(concat!(
            "由**真实的** mcloud-core（path 依赖，非镜像）的 serde 产出。",
            "⚠ 这批形状在 Rust 侧原本一个断言都没有 —— 而所有字段都带 ",
            "#[serde(default)]，字段名写错只会静默读到零值。",
            "另含 $fixtureKeys：直接由 serde_json 解析 test/fixtures/legacy/*.json ",
            "得出的**实际出现的键名**（不经过任何类型），用作字段名的客观真值。"
        )),
    );

    // ── $serializedShapes ────────────────────────────────────────────────
    //
    // 每个类型给两条：
    //   · empty  —— 由 `{}` 解出来再序列化（**所有字段都带 serde(default)**，
    //               所以 `{}` 就是"全默认"实例）⇒ 钉住 [] 与 null 的区别；
    //   · filled —— 由一段填充 JSON 解出来再序列化 ⇒ 钉住字段**顺序**与卷名。
    //
    // ⚠ 填充 JSON 是手写的，所以每条下面都 assert 了一次"该字段真的被赋值了"：
    // 键名写错时 `serde(default)` 会静默给默认値，只有这条 assert 抓得到。
    let mut shapes: Vec<Value> = Vec::new();

    push_shape::<LegacyConfig>(
        &mut shapes,
        "LegacyConfig",
        json!({}),
        json!({"version": 2, "caiyun": [{"auth": "Basic x", "nickname": "账号1"}]}),
        |v| assert_eq!(v.version, Some(2), "填充的 version 必须被读到"),
    );

    push_shape::<LegacyAccountConfig>(
        &mut shapes,
        "LegacyAccountConfig",
        json!({}),
        json!({"auth": "Basic x", "nickname": "账号1", "phone": "19900000001"}),
        |v| assert_eq!(v.phone.as_deref(), Some("19900000001")),
    );

    push_shape::<LegacyRun>(
        &mut shapes,
        "LegacyRun",
        json!({}),
        json!({
            "task": "sign", "account": "账号1", "ok": true,
            "start": "2026-09-16T12:00:00.000Z", "end": "2026-09-16T12:00:01.000Z",
            "durationMs": 1000,
            "logs": [{"level": 3, "type": "info", "msg": "m", "date": "d"}],
            "result": {"a": 1}, "error": "e"
        }),
        |v| {
            assert_eq!(v.task, "sign");
            assert_eq!(v.duration_ms, Some(1000));
            assert_eq!(v.logs.len(), 1, "logs 必须读到一条");
        },
    );

    push_shape::<LegacyLogEntry>(
        &mut shapes,
        "LegacyLogEntry",
        json!({}),
        json!({"level": 3, "type": "info", "msg": "m", "date": "d"}),
        |v| assert_eq!(v.kind.as_deref(), Some("info"), "type 字段必须被读到"),
    );

    push_shape::<LegacyPipeline>(
        &mut shapes,
        "LegacyPipeline",
        json!({}),
        json!({
            "id": "p-1", "name": "n", "description": "d",
            "steps": [{"task": "prizes"}],
            "account": "旧单值", "accounts": ["a", "b"],
            "accountMode": "parallel", "stopOnError": true,
            "createdAt": "c", "updatedAt": "u", "lastRunAt": "l",
            "lastStatus": "ok", "lastError": "e", "lastDurationMs": 2481
        }),
        |v| {
            assert_eq!(v.id, "p-1");
            assert!(v.legacy_account.is_some(), "废弃的 account 字段必须被读到");
            assert_eq!(v.last_duration_ms, Some(2481));
        },
    );

    push_shape::<LegacyStep>(
        &mut shapes,
        "LegacyStep",
        json!({}),
        json!({"task": "prizes", "body": {"b": 1}, "continueOnError": true, "account": "旧单值"}),
        |v| {
            assert_eq!(v.task, "prizes");
            assert!(v.legacy_account.is_some());
        },
    );

    push_shape::<LegacySchedule>(
        &mut shapes,
        "LegacySchedule",
        json!({}),
        json!({
            "id": "s-1", "name": "n", "task": "sign", "pipelineId": "p-1",
            "cron": "0 9 * * *", "timezone": "Asia/Shanghai", "account": 1,
            "body": {"b": 1}, "enabled": true,
            "createdAt": "c", "lastRunAt": "l",
            "lastStatus": "ok", "lastError": "e"
        }),
        |v| {
            assert_eq!(v.id, "s-1");
            assert_eq!(v.pipeline_id.as_deref(), Some("p-1"));
            assert_eq!(v.timezone.as_deref(), Some("Asia/Shanghai"));
        },
    );

    push_shape::<LegacyBinding>(
        &mut shapes,
        "LegacyBinding",
        json!({}),
        json!({"account": 1, "prizeId": 7, "prizeName": "云朵"}),
        |v| assert_eq!(v.prize_name.as_deref(), Some("云朵")),
    );

    out.insert("$serializedShapes".to_string(), Value::Array(shapes));

    // ── $pipelineStepDecode / $pipelineStepEncode ────────────────────────
    //
    // `PipelineStep` 是**唯一一个会被嵌进 JSON 再读回来**的 core 类型
    // （`pipelines.steps` 列），而它的 `body` / `continue_on_error` 是
    // `Option<Value>` / `Option<bool>`。
    //
    // ⚠ Go 侧对应的是 `json.RawMessage` / `*bool`，而 `json.RawMessage` 的
    // `UnmarshalJSON` 会把 `null` 存成**字面的四个字节**（不是 nil）——
    // 于是"往返之后 `== nil` 判定反转"，而**再序列化出来还是 `null`**，
    // 光看 JSON 文本一个字都看不出来。
    //
    // ⇒ 所以这里逐条实测 serde 的宽严与**编码形状**，而不是照 `Option` 的
    // 直觉推。要钉三件事：
    //   1. 缺席与 null 是不是同一个结果（都是 None？）；
    //   2. 未知字段、类型不符、顶层不是对象，各收还是拒；
    //   3. `None` 序列化出来是 `null` 还是**字段消失**。
    let mut step_encode: Vec<Value> = Vec::new();
    for (label, step) in [
        (
            "both-none",
            PipelineStep {
                task: "prizes".to_string(),
                body: None,
                continue_on_error: None,
            },
        ),
        (
            "both-some",
            PipelineStep {
                task: "receive".to_string(),
                body: Some(json!({"onlyNew": true})),
                continue_on_error: Some(true),
            },
        ),
        (
            "body-null-value",
            PipelineStep {
                task: "receive".to_string(),
                body: Some(Value::Null),
                continue_on_error: Some(false),
            },
        ),
        // ⚠ 这条是给 Go 侧的 `MarshalJSON` 用的：`json.Marshal` 会对
        // `json.RawMessage` 的内容**再做一遍 HTML 转义**（`<` → `\u003c`），
        // 而 serde_json **不转义**。没有含 `<` / `&` 的 body 时，
        // "必须走 marshalJSON"这件事在 Go 侧**没有任何断言看得见**。
        (
            "body-html-chars",
            PipelineStep {
                task: "receive".to_string(),
                body: Some(json!({"note": "<a&b>"})),
                continue_on_error: None,
            },
        ),
    ] {
        step_encode.push(json!({
            "label": label,
            "json": serde_json::to_string(&step).expect("PipelineStep 必然能序列化"),
        }));
    }
    out.insert("$pipelineStepEncode".to_string(), Value::Array(step_encode));

    let mut step_decode: Vec<Value> = Vec::new();
    for (label, raw) in [
        (
            "full-nulls",
            r#"{"task":"prizes","body":null,"continue_on_error":null}"#,
        ),
        (
            "body-present",
            r#"{"task":"prizes","body":{"a":1},"continue_on_error":true}"#,
        ),
        ("optionals-absent", r#"{"task":"prizes"}"#),
        ("task-absent", r#"{"body":null,"continue_on_error":null}"#),
        (
            "unknown-field",
            r#"{"task":"prizes","body":null,"continue_on_error":null,"extra":1}"#,
        ),
        (
            "body-string",
            r#"{"task":"prizes","body":"not-an-object","continue_on_error":null}"#,
        ),
        (
            "coe-string",
            r#"{"task":"prizes","body":null,"continue_on_error":"yes"}"#,
        ),
        ("top-level-array", r#"[]"#),
    ] {
        match serde_json::from_str::<PipelineStep>(raw) {
            Ok(v) => step_decode.push(json!({
                "label": label,
                "input": raw,
                "ok": true,
                "task": v.task,
                // 显式区分"缺席/null ⇒ None"与"真的给了一个值"
                "bodyIsNone": v.body.is_none(),
                "bodyText": v.body.as_ref().map(|b| b.to_string()),
                "coeIsNone": v.continue_on_error.is_none(),
            })),
            Err(e) => step_decode.push(json!({
                "label": label,
                "input": raw,
                "ok": false,
                "error": e.to_string(),
            })),
        }
    }
    out.insert("$pipelineStepDecode".to_string(), Value::Array(step_decode));

    // ── $fixtureKeys ─────────────────────────────────────────────────────
    //
    // 旧文档里**实际出现过**的键名（按层级路径），直接用 serde_json 解析夹具得出，
    // 不经过任何类型定义 —— 所以它是字段名的客观真值，不依赖 Rust 侧的类型是否写对。
    //
    // Go 侧拿它做两件事：
    //   1. "我认得这里出现的每一个键" —— 认不出说明 tag 写错或漏了；
    //   2. 解出来的字段不是零值 —— 认得出但读不到同样要红。
    let mut fixture_keys: Vec<Value> = Vec::new();
    for name in [
        "asign.json",
        "state.json",
        "history.json",
        "pipelines.json",
        "schedules.json",
        "exchange.json",
        "webui.json",
    ] {
        let root =
            std::env::var("MCLOUD_ROOT").expect("必须设 MCLOUD_ROOT（夹具要从仓库根算起）");
        let path = format!("{root}/test/fixtures/legacy/{name}");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读 {path} 失败: {e}"));
        let value: Value = serde_json::from_str(&text).expect("夹具应是合法 JSON");
        let mut entries: Vec<String> = Vec::new();
        collect_keys(&value, "$", &mut entries);
        entries.sort();
        // 同一路径会被每个数组元素各记一次 ⇒ 去重，否则 12 条历史会产出 12 份同样的路径。
        entries.dedup();
        fixture_keys.push(json!({ "file": name, "paths": entries }));
    }
    out.insert("$fixtureKeys".to_string(), Value::Array(fixture_keys));

    println!("{}", serde_json::to_string_pretty(&out).expect("必然能序列化"));
}

/// 序列化一个类型的"空"与"填充"两个实例。
///
/// ⚠ `empty` 走 `from_value::<T>(json!({}))` 而不是 `T::default()`：
/// 这批类型大多**没有** derive `Default`，但它们的所有字段都带
/// `#[serde(default)]` —— 所以 `{}` 解出来的就是全默认实例，而且这条路
/// 不需要类型实现 Default。
fn push_shape<T>(
    out: &mut Vec<Value>,
    name: &str,
    empty: Value,
    filled: Value,
    assert_filled: impl Fn(&T),
) where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let empty_v: T = serde_json::from_value(empty).expect("{} 必然能解出全默认实例");
    let filled_v: T = serde_json::from_value(filled.clone()).expect("填充 JSON 必须能解出来");
    assert_filled(&filled_v);

    out.push(json!({
        "type": name,
        "empty": serde_json::to_string(&empty_v).expect("必然能序列化"),
        "filled": serde_json::to_string(&filled_v).expect("必然能序列化"),
    }));
}

/// 递归收集一棵 JSON 树里每个对象路径下出现过的键名。
///
/// 只收**对象**的键与**数组**下标标记：数组的键名没有意义（元素形状才重要），
/// 所以下标统一记成 `[]`，让 `$.runs[].xxx` 这种路径在不同元素间归并。
///
/// ⚠ `$.accounts.<手机号>` 是个例外：那层的键是**数据**（旧 `accountKey`），
/// 不是字段名。写成字面值会让向量里出现账号标识，而且它每次重生成都可能变
/// —— 所以统一记成 `{}`。这类"键是数据、值是记录"的层目前只有这一处。
fn collect_keys(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            let keys_are_data = path == "$.accounts";
            for (k, v) in map {
                let child = format!("{path}.{}", if keys_are_data { "{}" } else { k });
                out.push(child.clone());
                collect_keys(v, &child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_keys(item, &format!("{path}[]"), out);
            }
        }
        _ => {}
    }
}
