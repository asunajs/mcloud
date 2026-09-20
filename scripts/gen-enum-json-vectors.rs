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

// ── 以下镜像 model.rs ────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Group {
    Daily,
    Activity,
    Query,
    Exchange,
    Maintenance,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CapabilityKind {
    Action,
    Query,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AccountMode {
    Sequential,
    Parallel,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FieldKind {
    Text,
    Number,
    Bool,
    Select,
    MultiSelect,
    Json,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScheduleStatus {
    Ok,
    Error,
    Skipped,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PipelineStatus {
    Ok,
    Error,
    Partial,
}

/// `RunStage` —— `#[serde(tag = "stage")]`，与 `ScheduleTarget` 同族。
///
/// ⚠ 两个变体的**字段顺序不同**：`Done` 是 outcome → manual_steps → summary，
/// 而 `Failed` 是 summary → manual_steps。整串比对才看得出来，
/// 逐字段比对会把这个顺序差异整个放过。
#[derive(Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
enum RunStage {
    Running,
    Done {
        outcome: RunOutcome,
        manual_steps: Vec<ManualStep>,
        summary: Option<String>,
    },
    Failed {
        summary: String,
        manual_steps: Vec<ManualStep>,
    },
}

#[derive(Serialize)]
#[serde(transparent)]
struct AccountId(String);

// ⚠ `tag = "kind"` **不能漏**：漏了会退回 serde 默认的**外部标签**
// （`{"capability":{"code":"sign"}}`），与库里已存的形状完全不同。
// 我第一次抄镜像时就漏了它 —— 这个错误类型系统抓不到，只有整串比对抓得到。
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ScheduleTarget {
    Capability { code: String },
    Pipeline { id: String },
}

#[derive(Serialize)]
struct UserId(String);

#[derive(Serialize)]
struct SelectOption {
    label: String,
    value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<String>,
}

#[derive(Serialize)]
struct FieldSpec {
    key: String,
    label: String,
    kind: FieldKind,
    #[serde(default)]
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<SelectOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<String>,
}

/// 只取 `Account` 里能钉住"字段名与 Option 形状"的那几个字段。
///
/// ⚠ 不照抄整个结构体：`chrono::DateTime<Utc>` 的秒以下精度在两端
/// 不一定同形（chrono 用 AutoSi，Go 的 time.Time 用 RFC3339Nano），
/// 把时间列进整串比对会引入一个与本次无关的失败点。
/// 这里关心的是 **`owner` 的 `skip_serializing_if`** 与 **其余 Option 是 null 不是省略**。
/// `User` —— **一度整个漏写了 json tag**（`user` 那一步带过来的），
/// 而往返测试用什么字段名都是绿的，所以改坏它完全不响。
///
/// ⚠ 时间列只取**整秒**：chrono 用 AutoSi（0/3/6/9 位小数），
/// Go 的 `time.Time` 用 RFC3339Nano（去掉末尾的 0）。整秒时两者一致，
/// 亚秒是**已知未对齐**的一条 —— 别在这里假装它对齐了。
#[derive(Serialize)]
struct UserShape {
    id: UserId,
    name: String,
    password_hash: String,
    role: Role,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct SessionShape {
    token_hash: String,
    user_id: UserId,
    expires_at: chrono::DateTime<chrono::Utc>,
    kind: SessionKind,
}

/// `CloudStats` —— 单独钉它是因为它**一度漏写了全部 json tag**。
///
/// Rust 侧字段名默认 snake_case（`cloud_count`），而 Go 不带 tag 会输出
/// **字段名原样**（`CloudCount`）。反向测试证明：改坏它**没有任何测试变红**，
/// 所以这里补一条整串期望值把那条路堵上。
#[derive(Serialize)]
struct CloudStatsShape {
    account_id: AccountId,
    date: String,
    cloud_count: i64,
    cloud_diff: i64,
    cloud_diff_week: i64,
}

#[derive(Serialize)]
struct AccountShape {
    id: AccountId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<UserId>,
    nickname: Option<String>,
    enabled: bool,
    sort_order: i64,
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

/// 把 `v` 序列化成**整串**存进向量。
///
/// ⚠ 必须存 `to_string` 的结果，不能存 `to_value`：后者是 `Value::Object`，
/// 默认实现是 `BTreeMap`（键会**按字典序重排**），字段顺序这一信息就丢了。
fn push_json(out: &mut Vec<Value>, name: &str, v: impl Serialize) {
    out.push(json!({
        "kind": name,
        "json": serde_json::to_string(&v).expect("必然能序列化"),
    }));
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
        // ── model.rs ──
        text("Group", "Daily", Group::Daily),
        text("Group", "Activity", Group::Activity),
        text("Group", "Query", Group::Query),
        text("Group", "Exchange", Group::Exchange),
        text("Group", "Maintenance", Group::Maintenance),
        text("CapabilityKind", "Action", CapabilityKind::Action),
        text("CapabilityKind", "Query", CapabilityKind::Query),
        text("RiskLevel", "Low", RiskLevel::Low),
        text("RiskLevel", "Medium", RiskLevel::Medium),
        text("RiskLevel", "High", RiskLevel::High),
        text("AccountMode", "Sequential", AccountMode::Sequential),
        text("AccountMode", "Parallel", AccountMode::Parallel),
        text("FieldKind", "Text", FieldKind::Text),
        text("FieldKind", "Number", FieldKind::Number),
        text("FieldKind", "Bool", FieldKind::Bool),
        text("FieldKind", "Select", FieldKind::Select),
        // ⚠ `MultiSelect` 是重点：snake_case 会插下划线推成 `multi_select`，
        // 而 `lowercase` 会给 `multiselect`。这两个串只差一个字符，
        // 写错之后前端 `switch (kind)` 永远走 default —— 且不报错。
        text("FieldKind", "MultiSelect", FieldKind::MultiSelect),
        text("FieldKind", "Json", FieldKind::Json),
        text("ScheduleStatus", "Ok", ScheduleStatus::Ok),
        text("ScheduleStatus", "Error", ScheduleStatus::Error),
        text("ScheduleStatus", "Skipped", ScheduleStatus::Skipped),
        text("PipelineStatus", "Ok", PipelineStatus::Ok),
        text("PipelineStatus", "Error", PipelineStatus::Error),
        text("PipelineStatus", "Partial", PipelineStatus::Partial),
    ];

    let mut tagged_json: Vec<Value> = Vec::new();
    push_json(&mut tagged_json, "schedule-target-capability", ScheduleTarget::Capability { code: "sign".to_string() });
    push_json(&mut tagged_json, "schedule-target-pipeline", ScheduleTarget::Pipeline { id: "p-1".to_string() });

    let step_for_stage = ManualStep {
        capability: "msg-push".to_string(),
        account: None,
        instruction: "请前往 App 手动打开消息通知".to_string(),
        target: ManualTarget::MobileApp,
    };
    push_json(&mut tagged_json, "run-stage-running", RunStage::Running);
    push_json(
        &mut tagged_json,
        "run-stage-done",
        RunStage::Done {
            outcome: RunOutcome::ManualRequired,
            manual_steps: vec![step_for_stage],
            summary: Some("有一件待办".to_string()),
        },
    );
    push_json(
        &mut tagged_json,
        "run-stage-done-empty",
        RunStage::Done {
            outcome: RunOutcome::Success,
            manual_steps: Vec::new(),
            summary: None,
        },
    );
    push_json(
        &mut tagged_json,
        "run-stage-failed",
        RunStage::Failed {
            summary: "上游 502".to_string(),
            manual_steps: Vec::new(),
        },
    );

    let mut struct_json: Vec<Value> = Vec::new();
    push_json(
        &mut struct_json,
        "field-spec-bool",
        FieldSpec {
                key: "skipWait".to_string(),
                label: "跳过等待".to_string(),
                kind: FieldKind::Bool,
                required: false,
                default: Some(json!(true)),
                options: None,
                help: Some("关闭后等到整点开抢".to_string()),
            },
    );
    push_json(
        &mut struct_json,
        "field-spec-text",
        FieldSpec {
            key: "phone".to_string(),
            label: "手机号".to_string(),
            kind: FieldKind::Text,
            required: true,
            default: None,
            options: None,
            help: None,
        },
    );
    push_json(
        &mut struct_json,
        "field-spec-select",
        FieldSpec {
            key: "prize".to_string(),
            label: "奖品".to_string(),
            kind: FieldKind::Select,
            required: false,
            default: None,
            options: Some(vec![SelectOption {
                label: "云朵".to_string(),
                value: json!(7),
                help: None,
            }]),
            help: None,
        },
    );
    // 整秒的 UTC 时刻，两端都应当是 `2026-09-20T10:00:00Z`。
    let whole_second =
        chrono::DateTime::from_timestamp(1_784_000_400, 0).expect("整秒时刻必然可构造");

    push_json(
        &mut struct_json,
        "user",
        UserShape {
            id: UserId("u-1".to_string()),
            name: "admin".to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$abc$def".to_string(),
            role: Role::Admin,
            created_at: whole_second,
        },
    );
    push_json(
        &mut struct_json,
        "session",
        SessionShape {
            token_hash: "e3b0c442".to_string(),
            user_id: UserId("u-1".to_string()),
            expires_at: whole_second,
            kind: SessionKind::Web,
        },
    );
    push_json(
        &mut struct_json,
        "cloud-stats",
        CloudStatsShape {
            account_id: AccountId("19900001234".to_string()),
            date: "2026-09-20".to_string(),
            cloud_count: 120,
            cloud_diff: 7,
            cloud_diff_week: 30,
        },
    );
    push_json(
        &mut struct_json,
        "account-owner-none",
        AccountShape {
            id: AccountId("19900001234".to_string()),
            owner: None,
            nickname: Some("小号".to_string()),
            enabled: true,
            sort_order: 0,
        },
    );
    push_json(
        &mut struct_json,
        "account-owner-some",
        AccountShape {
            id: AccountId("19900001234".to_string()),
            owner: Some(UserId("u-1".to_string())),
            nickname: None,
            enabled: true,
            sort_order: 3,
        },
    );

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
    out.insert("$taggedJson".to_string(), Value::Array(tagged_json));
    out.insert("$structJson".to_string(), Value::Array(struct_json));
    out.insert("$decode".to_string(), Value::Array(decode_cases));

    // ⚠ 用 serde_json 自己序列化，不手拼 —— 手拼会漏转义、多尾逗号。
    // 生成器产出的东西必须自己先能被解析，这是上一份向量踩过的坑。
    println!("{}", serde_json::to_string_pretty(&Value::Object(out)).expect("输出必然可序列化"));
}
