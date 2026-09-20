// 生成 test/golden/enum-json-vectors.json —— 枚举与运行事件的 **JSON 文本形状**。
//
// # 为什么要有这份向量
//
// Rust 侧每一个进 JSON 的枚举都带 `#[serde(rename_all = …)]`，序列化成**文本**；
// 而 Go 的 `type X int` 默认序列化成**数字**。两边读同一批 `run_events.payload`、
// 同一张 `users.role` 列 —— 默认行为不一致就是同一个字段有两个形状，
// 而它**不报错**，只在真跑起来之后表现为"另一侧读出来的东西不对"。
//
// 所以 Go 侧必须给每个进 JSON 的枚举显式实现 MarshalJSON / UnmarshalJSON
// （见 internal/core/enum.go）。而"实现得对不对"不能靠 Go 自己说 ——
// **生成侧必须是另一份实现**，用 Go 生成再用 Go 验证是自证。
//
// # ⚠ 本文件里的类型定义是**镜像**，不是引用
//
// Rust 侧已冻结（legacy/crates），这里不可能 `use` 它 ——
// 于是照抄它的 `rename_all` 与变体名。抄错的风险由两处兜住：
//   ① 变体名逐字抄自 `legacy/crates/mcloud-core/src/{run,model,user}.rs`；
//   ② **`$eventJson` 是整串比较**，字段顺序、tag 位置、Option 的 null 全在里面，
//      抄漏一处就对不上（serde 对 tag 枚举把 tag 放第一个，Go 侧也得放第一个）。
//
// 如果 Rust 侧哪天改了 `rename_all`，本文件与 Go 侧会**一起变红** —— 那正是想要的。
//
// # 怎么重新生成
//
//   cargo new /tmp/mcloud-enum-vector && cd $_
//   # 把本文件拷成 src/main.rs，Cargo.toml 里加：
//   #   serde = { version = "1", features = ["derive"] }
//   #   serde_json = "1"
//   CARGO_NET_OFFLINE=true cargo run --quiet > test/golden/enum-json-vectors.json
//
// ⚠ 这是**一次性数据变换**，不参与任何构建（Go / Rust 两侧的门禁都不编它）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ── 镜像：变体名与 rename_all 逐字抄自 Rust 侧 ──────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
    Running,
    Done,
    Failed,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RunOutcome {
    Success,
    PartialSuccess,
    ManualRequired,
    Failed,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ManualTarget {
    MobileApp,
    Web,
    Webui,
    Cli,
    Unspecified,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    User,
    Admin,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SessionKind {
    Web,
    Cli,
}

#[derive(Serialize)]
struct ManualStep {
    capability: String,
    account: Option<String>,
    instruction: String,
    target: ManualTarget,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RunEvent {
    Started {
        #[serde(rename = "runId")]
        run_id: String,
        capability: String,
    },
    Log {
        #[serde(rename = "runId")]
        run_id: String,
        level: LogLevel,
        message: String,
    },
    Progress {
        #[serde(rename = "runId")]
        run_id: String,
        done: u32,
        total: u32,
    },
    Finished {
        #[serde(rename = "runId")]
        run_id: String,
        outcome: RunOutcome,
        manual_steps: Vec<ManualStep>,
        summary: Option<String>,
    },
    Errored {
        #[serde(rename = "runId")]
        run_id: String,
        message: String,
    },
}

// ── 输出 ─────────────────────────────────────────────────────────────────────

/// 一条"枚举值 → serde 文本"。
fn text(name: &str, variant: &str, value: impl Serialize) -> Value {
    json!({
        "enum": name,
        "variant": variant,
        "text": serde_json::to_value(&value).expect("枚举必然能序列化"),
    })
}

/// 试解一段 JSON，记录 serde 是收还是拒。
fn decode(name: &str, raw: &str, ok: bool) -> Value {
    json!({ "type": name, "json": raw, "ok": ok })
}

fn rejects<T: for<'de> Deserialize<'de>>(raw: &str) -> bool {
    serde_json::from_str::<T>(raw).is_err()
}

fn main() {
    // ⚠ 固定的 runId 与固定文案：让两侧比较的是**形状**，不是随机值。
    // message 里刻意放 `<` `>` `&` 与中文 —— 那三个字符正是 Go 默认
    // `json.Marshal` 会转义成 `\u003c` 的地方，而 serde_json 不转。
    let run_id = "64234443-2463-4483-a4a3-c4c3a4e38403";
    let tricky = "风控退避 <b>&amp;</b> 明天再试";

    let enum_text: Vec<Value> = vec![
        text("RunStatus", "Running", RunStatus::Running),
        text("RunStatus", "Done", RunStatus::Done),
        text("RunStatus", "Failed", RunStatus::Failed),
        text("RunOutcome", "Success", RunOutcome::Success),
        text("RunOutcome", "PartialSuccess", RunOutcome::PartialSuccess),
        text("RunOutcome", "ManualRequired", RunOutcome::ManualRequired),
        text("RunOutcome", "Failed", RunOutcome::Failed),
        text("ManualTarget", "MobileApp", ManualTarget::MobileApp),
        text("ManualTarget", "Web", ManualTarget::Web),
        text("ManualTarget", "Webui", ManualTarget::Webui),
        text("ManualTarget", "Cli", ManualTarget::Cli),
        text("ManualTarget", "Unspecified", ManualTarget::Unspecified),
        text("LogLevel", "Info", LogLevel::Info),
        text("LogLevel", "Warn", LogLevel::Warn),
        text("LogLevel", "Error", LogLevel::Error),
        text("Role", "User", Role::User),
        text("Role", "Admin", Role::Admin),
        text("SessionKind", "Web", SessionKind::Web),
        text("SessionKind", "Cli", SessionKind::Cli),
    ];

    let step = ManualStep {
        capability: "backup-gift".to_string(),
        account: Some("19900001234".to_string()),
        instruction: "未开启备份，请前往 App 手动开启".to_string(),
        target: ManualTarget::MobileApp,
    };

    let events: Vec<(&str, RunEvent)> = vec![
        (
            "started",
            RunEvent::Started {
                run_id: run_id.to_string(),
                capability: "sign".to_string(),
            },
        ),
        (
            "log",
            RunEvent::Log {
                run_id: run_id.to_string(),
                level: LogLevel::Warn,
                message: tricky.to_string(),
            },
        ),
        (
            "progress",
            RunEvent::Progress {
                run_id: run_id.to_string(),
                done: 3,
                total: 20,
            },
        ),
        (
            "finished-with-steps",
            RunEvent::Finished {
                run_id: run_id.to_string(),
                outcome: RunOutcome::ManualRequired,
                manual_steps: vec![step],
                summary: Some("有一件待办".to_string()),
            },
        ),
        (
            "finished-empty",
            RunEvent::Finished {
                run_id: run_id.to_string(),
                outcome: RunOutcome::Success,
                manual_steps: Vec::new(),
                summary: None,
            },
        ),
        (
            "errored",
            RunEvent::Errored {
                run_id: run_id.to_string(),
                message: "上游 500".to_string(),
            },
        ),
    ];

    let mut event_json: Vec<Value> = Vec::new();
    for (name, ev) in &events {
        event_json.push(json!({
            "kind": name,
            "json": serde_json::to_string(ev).expect("事件必然能序列化"),
        }));
    }

    // 解码侧：哪些输入 serde 收、哪些拒。
    // ⚠ `2` 是 Go 侧在没有 MarshalJSON 时**会写出来**的东西 ——
    // 让它在 Rust 侧被拒，Go 侧也就必须拒，否则两边对同一行的处理不同。
    let mut decode_cases: Vec<Value> = Vec::new();
    decode_cases.push(decode("RunOutcome", "\"manual_required\"", !rejects::<RunOutcome>("\"manual_required\"")));
    decode_cases.push(decode("RunOutcome", "\"nope\"", !rejects::<RunOutcome>("\"nope\"")));
    decode_cases.push(decode("RunOutcome", "2", !rejects::<RunOutcome>("2")));
    decode_cases.push(decode("RunOutcome", "null", !rejects::<RunOutcome>("null")));
    decode_cases.push(decode("LogLevel", "\"warn\"", !rejects::<LogLevel>("\"warn\"")));
    decode_cases.push(decode("LogLevel", "\"trace\"", !rejects::<LogLevel>("\"trace\"")));
    decode_cases.push(decode("Role", "\"admin\"", !rejects::<Role>("\"admin\"")));
    decode_cases.push(decode("Role", "\"root\"", !rejects::<Role>("\"root\"")));

    let mut out = serde_json::Map::new();
    out.insert(
        "$comment".to_string(),
        json!(concat!(
            "枚举与运行事件的 JSON 文本形状，由 serde 1.0 / serde_json 1.0 产出。",
            "生成器 = scripts/gen-enum-json-vectors.rs。",
            "⚠ 这是跨语言对拍用的：Rust 侧的 serde 枚举序列化成文本，",
            "而 Go 的 `type X int` 默认是数字 —— 生成侧与消费侧必须是两份实现，否则是自证。"
        )),
    );
    out.insert("$enumText".to_string(), Value::Array(enum_text));
    out.insert("$eventJson".to_string(), Value::Array(event_json));
    out.insert("$decode".to_string(), Value::Array(decode_cases));

    // ⚠ 用 serde_json 自己序列化，不手拼 —— 手拼会漏转义、多尾逗号。
    // 生成器产出的东西必须自己先能被解析，这是上一份向量踩过的坑。
    println!("{}", serde_json::to_string_pretty(&Value::Object(out)).expect("输出必然可序列化"));
}
