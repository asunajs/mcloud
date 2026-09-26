# mcloud-gost 编写约定

> 本文件是**编码与协作的强制约定**，与 [`architecture.md`](./architecture.md) 分工如下：
>
> - `architecture.md`：**做什么** —— 分层、领域模型、接口契约、里程碑。
> - `conventions.md`（本文件）：**怎么写** —— 目录、命名、错误、测试、提交、工具链。
>
> 状态：**已生效**。可执行部分由 `rustfmt.toml`、`.editorconfig`、根 `Cargo.toml` 的
> `[workspace.lints]` 落地，不靠自觉。
>
> 约定分两类，务必区分：
>
> - **【硬约束】**：已由工具强制，违反会导致 `fmt --check` / `clippy` 失败。
> - **【软约定】**：工具管不到，靠 review 与本文档约束。

---

## 0. 现状基线（写约定时的事实）

写下这些数字是为了让后续判断有基准，不是为了自我表扬。

| 项 | 值 |
|---|---|
| crate 数 | 8（`core` / `crypto` / `upstream` / `engine` / `tasks` / `store` / `app` / `server`） |
| 工具链 | stable `1.91.1`，`rust-version = 1.88`，`edition 2021` |
| 测试 | 全绿（58 条通过、0 失败，含 `mcloud-crypto` 黄金向量对拍） |
| `cargo fmt --check` | ✅ 干净（0 漂移） |
| `cargo clippy --all-targets` | ✅ 干净（0 告警，含测试代码） |
| 注释语言 | **中文**，术语（trait / crate / 标识符）保留英文 |
| 版本控制 | 已 `git init`，分支 `main`（见 §10.1） |

**这些数字是写约定时的快照。** 格式与 lint 都已干净，因此
`fmt --check` 与 `clippy` 可以直接当硬门禁（§9.2）。

---

## 1. 分层铁律【硬约束 · 靠 review 与 CI 脚本】

`architecture.md` §2 的依赖图是**唯一权威**。此处只重申最容易违反的几条：

1. **依赖只能自上而下。** 反向依赖一律视为设计错误，不接受「临时这样写」。
2. **零 IO 层**：`mcloud-core`、`mcloud-crypto`、`mcloud-engine`、`mcloud-tasks`
   **不得**直接依赖 `reqwest`、`sqlx`、`std::fs`、`std::net`。
   所有外部能力经 `mcloud-core` 的 trait 注入。
3. **`mcloud-upstream` 是唯一直接用 `reqwest` 的 crate**，也是唯一构造凭证头的地方。
4. **`mcloud-store` 是唯一碰磁盘的 crate**。

### 1.1 依赖白名单检查

新增依赖前先自问「能不能用 trait 解决」。确认要加时，`Cargo.toml` 里必须走
`workspace = true` 引用根 `[workspace.dependencies]`，**不允许在 crate 内写裸版本号**：

```toml
# ✅ 正确
[dependencies]
reqwest.workspace = true

# ❌ 禁止：版本号散落各处，升级时漏改
[dependencies]
reqwest = "0.12"
```

依赖属于哪个 crate，由 §1.2 决定；**不要图省事把它加到 `mcloud-core`**。

### 1.2 每个 crate 的职责边界

| crate | 允许做的事 | 明确禁止 |
|---|---|---|
| `mcloud-core` | 领域类型、错误、trait 定义 | 任何 IO、任何具体实现 |
| `mcloud-crypto` | 纯函数密码学 | IO、读配置、发请求 |
| `mcloud-upstream` | 端点常量、请求/响应类型、`reqwest` 客户端 | 业务编排、读写磁盘 |
| `mcloud-engine` | 注册表、运行器、并发闸、事件定义 | IO、依赖具体任务 |
| `mcloud-tasks` | 任务业务编排 | 直接 `reqwest` / `std::fs` |
| `mcloud-store` | 持久化实现 | 业务逻辑、网络 |
| `mcloud-app` | 组装、注入 IO 实现 | 业务逻辑 |
| `mcloud-server` | axum 路由、SSE、内嵌前端 | 业务逻辑、直接碰磁盘 |

> 当前遗留：`mcloud-store` 的 `JsonStore` 在 `async fn` 里用了阻塞的 `std::fs`。
> 数据量小、调用不频繁时可以接受；**一旦出现并发写或大文件，必须改为
> `tokio::fs` 或 `spawn_blocking`**。见 §7.4。

---

## 2. 格式与风格

### 2.1 统一入口【硬约束】

```bash
cargo fmt --all              # 写回
cargo fmt --all -- --check   # CI 校验，必须无输出
```

配置在 `rustfmt.toml`。**只用 stable 选项**——本仓库工具链是 stable，
nightly-only 选项会被静默忽略（只在首次调用时打一行 warning），
写进去等于没写，还会误导后来者。

> 已踩过的坑：`format_code_in_doc_comments = true` 属于 nightly-only。
> 需要它时先做「升级到 nightly 工具链」的决策，不要在 stable 下加。

### 2.2 风格要点【软约定】

- **缩进**：Rust 4 空格；`toml`/`json`/`ts`/`tsx` 2 空格（`.editorconfig` 已设）。
- **换行符**：一律 `LF`，文件末尾留一个换行。
- **行宽**：`max_width = 100`。超过说明该拆表达式或提局部变量了。
- **中文排版**：中文注释里，中英文之间**加一个空格**（`用 reqwest 客户端`、
  `任务只依赖 mcloud-core 的 trait`）。这不是审美偏好——实测现有代码
  **166 处加空格、0 处不加**，且不加空格会让中英混排难以断词。
  标点用全角。
- **注释语言**：**中文**。但标识符、类型名、crate 名、标准术语保持英文，
  不要硬译（写 `trait`，不写「特征」；写 `crate`，不写「箱子」）。
- **文档注释**：公开项（`pub`）应当有 `///`，说明**为什么**而非复述签名。
  crate 根用 `//!` 写职责与铁律，参照 `mcloud-core/src/lib.rs`。

  > **这是目标，不是现状**：现有代码约 48 个 `pub` 项没有文档注释
  > （多为 `new()` / `as_str()` 这类自解释的取值方法，以及 `builder.rs` 的
  > 链式 setter）。不必为它们专门补一轮文档提交——**新增的公开 API 必须写**，
  > 改动到旧代码时顺手补。真正需要解释的是「为什么」：语义不直观的、
  > 有隐含前提的、或看起来可以简化但不能简化的。
- **空行节奏**：`use` 分组之间、`impl` 块之间、逻辑段落之间留一个空行；
  不要连续两个以上空行。
- **`use` 排序**：由 `reorder_imports = true` 处理。分组习惯沿用现有风格：
  标准库 → 外部 crate → 本 crate/workspace。

### 2.3 格式门禁【硬约束】

`cargo fmt --all -- --check` 当前**干净**，因此它是可以直接生效的硬门禁：
提交前必须无输出，见 §9.2。

```bash
cargo fmt --all                 # 写回全部
cargo fmt -p mcloud-core        # 只格式化本次改动的 crate
```

**不要**为了省事提交「全仓格式化」的独立改动——那会把真正的逻辑改动埋在
大量 diff 里，review 无从下手。日常只格式化自己碰过的 crate。

新增文件必须**从第一次提交起就干净**。

---

## 3. Lint 基线【硬约束】

### 3.1 已启用

根 `Cargo.toml` 的 `[workspace.lints]` 定义，各 crate 以 `[lints] workspace = true` 继承：

```toml
[workspace.lints.rust]
unsafe_code = "forbid"                                  # 纯 Rust，无 unsafe
unused_must_use = "deny"                                # 丢掉 Result 是真实缺陷
rust_2018_idioms = { level = "warn", priority = -1 }

[workspace.lints.clippy]
all = "warn"
style = "warn"
suspicious = "warn"
complexity = "warn"
perf = "warn"
```

**取值原则：只钉住「当前代码已全部满足」的规则。** 启用一条规则前，先实测它在
现有代码上的告警数；会产生历史欠账的规则不进基线（否则 `-D warnings` 立刻红）。

`rust_2018_idioms` 必须带 `priority = -1`：该组内含 `unused_must_use`，
同优先级会让 clippy 报 `lint_groups_priority`。

### 3.2 刻意未启用【软约定】

| 规则 | 不启用的原因 |
|---|---|
| `missing_debug_implementations` | 多为内部类型，噪音大于收益 |
| `clippy::pedantic` | 大量风格偏好，会淹没真问题 |
| `clippy::nursery` | 不稳定，跨版本易变 |

> 表中**刻意不写告警条数**：这些数字随代码量增长而变，写死必然过期
> （曾写过一组，几个提交后即失效）。要复核就现场跑：
> `cargo clippy --workspace --all-targets -- -W clippy::pedantic`

这些靠 review 把关：**公开类型加 `#[derive(Debug)]`**，其余按需。

### 3.3 告警欠账：当前为零

```bash
cargo clippy --workspace --all-targets
```

**当前 0 告警**（含测试代码），因此可以直接当硬门禁。

> 已清理的历史欠账，留档说明「搭车清理」的用法：
>
> - `very complex type`（3 处）→ 引入 `mcloud_engine::HttpClientFactory` 类型别名收敛；
> - `min_broadcast` 死字段 → 移除；
> - `field iv is never read`（`mcloud-crypto/tests/golden.rs`）→
>   核对向量后确认该字段在数据里并不存在，删除结构体字段而非补假断言。
>
> 也就是说：**清账不是让告警消失，而是先判断代码或数据哪一边错了。**
> 最后一条本可以随手 `#[allow]` 掉，但那是把问题藏起来。

### 3.4 禁止项【硬约束 · review】

- **禁止 `unwrap()` / `expect()` 出现在非测试代码**，除非能证明不可达。
  当前仅有两处豁免，新增需在 review 中说明理由：
  1. `runner.rs` 的 `expect("inflight 锁中毒")`——毒锁意味着已有 panic，继续运行无意义；
  2. `builder.rs` 的 `expect("最小客户端应可构建")`——退化客户端，构造参数是常量；
  3. `event.rs` 的 `expect("cursor 在边界内")`——游标由 `char_indices` 保证落在字符边界。

  > 注意 `event.rs` 那处在 `#[cfg(test)]` **之前**，属生产路径；
  > 不要因为「看起来像测试」就忽略它。
- **禁止 `panic!` / `todo!` / `unimplemented!` 进入主路径**。
  测试里可以用（`golden.rs` 用它给出可读的失败信息）。
- **禁止 `dbg!`**。临时调试用 `tracing::debug!`。
- **禁止在库代码里 `println!`**。用户可见输出只允许出现在 `mcloud-server/src/bin/cli.rs`；
  日志一律走 `tracing`。
- **禁止 `unsafe`**（已由 lint 强制）。

---

## 4. 命名

| 对象 | 规则 | 示例 |
|---|---|---|
| crate | `mcloud-` 前缀，短横线 | `mcloud-crypto` |
| 模块/文件 | `snake_case`，一个文件一个主题 | `json_store.rs`、`sign.rs` |
| 类型/trait | `UpperCamelCase` | `TaskContext`、`EventSink` |
| 函数/方法 | `snake_case`，动词开头 | `generate_signature`、`accepts_basic` |
| 常量 | `SCREAMING_SNAKE_CASE` | `AES_FIXED_IV`、`SALT_MAP` |
| 新类型 | 包裹单字段，提供构造与取值方法 | `AccountId(pub String)` |

**新类型（newtype）优先于裸 `String`**：`AccountId`、`DeviceId`、`RunId` 是范例。
理由：签名/ID 混用是安全事件，类型系统是最便宜的防线。

取值方法按语义取名，不要强求统一：`AccountId`/`DeviceId` 用 `as_str()`，
`RunId` 用 `get()`（它本来就是数值）。构造用 `new(...)` 或元组字面量
（`DeviceId` 目前只有元组构造，属可接受的现状）。

任务名用 **kebab-case**（`ai-avatar`、`cloud-phone-redpack`），因为它是 wire 标识；
Rust 里的注册表键沿用同一字符串，不要转成下划线。

---

## 5. 错误处理【硬约束】

### 5.1 统一错误类型

业务错误一律用 `mcloud_core::Error`（`thiserror`），**不要**在业务层引入 `anyhow`。
`anyhow` 只允许出现在**二进制入口**（`main.rs`、`cli.rs`）用于拼接上下文。

crate 内部可以有私有错误类型（如 `CryptoError`），但**跨 crate 边界必须转成
`mcloud_core::Error`**。

### 5.2 错误语义：网络 vs 业务

这是本项目的核心区分，**不许含糊**：

```rust
// 网络层失败（连接/超时/TLS）→ Err
Err(mcloud_core::Error::Network(..))

// 上游返回了业务错误 → 也是 Err，但带 code
Err(mcloud_core::Error::Upstream { code, message })
```

判断「凭证失效」必须用 `Error::is_auth_failure()`，**不要**在业务代码里
散落 `code == "1001"` 这类魔法判断——码值集中在那一个函数里。

### 5.3 结果 vs 错误：`TaskOutcome` 的用法

任务返回 `Result<TaskOutcome>`。**「没做成」不等于「失败」**：

| 场景 | 返回 |
|---|---|
| 正常完成 | `Ok(TaskOutcome::success("签到成功，获得 3 云朵"))` |
| 活动未开启 / 今日已签到 / 条件不满足 | `Ok(TaskOutcome::skipped("已签到"))` |
| 真的出错了（网络、解析、上游拒绝） | `Err(..)`，运行器会转成 `Failed` |

`Skipped` **不计入错误率、不触发告警**。把「已签到」当失败上报是最常见的误用，
会让告警失去意义。

`summary()` 是给人看的一句话，**前端直接渲染**，不要让它去解析日志文本。

### 5.4 上游响应解析

上游 `code` 成功值判定集中在任务层，标准写法（见 `mcloud-tasks/src/sign.rs`）：

```rust
let code = v.get("code").and_then(|c| c.as_str()).unwrap_or("");
if !matches!(code, "0" | "0000" | "200") {
    // 注意：这里返回 Err，由调用方决定是否算 skipped
    return Err(mcloud_core::Error::Upstream { code: code.into(), message: msg.into() });
}
```

解析失败时，`HttpResponse::json()` 会带上原文前 200 字符——**保留这个行为**，
它是排障的主要线索。

---

## 6. 异步与并发【硬约束】

- 异步 trait 一律用 `#[async_trait]`（`mcloud-core` 的 `HttpClient` / `Store` 是范例）。
- **默认禁止在异步上下文里做阻塞 IO**。`std::fs`、`std::thread::sleep` 会占住
  一个 runtime worker。需要时用 `tokio::fs` 或 `tokio::task::spawn_blocking`。
- **唯一的例外是 `mcloud-store` 的短写盘**，它是有意为之，不是欠账：
  数据都在 KB 量级，实测一次写盘 **0.19ms**（311KB，单 run 日志上限 2000 条）；
  即使取编译期断言允许的极限 10000 条（1.5MB）也只有 **0.66ms**。
  落盘节拍是 2 秒一次（`main.rs`），runtime 是多线程的。
  为 0.19ms 引入 `spawn_blocking` 的调度开销比它省下的还多。
  **例外成立的前提**（任一条不成立就必须改回 `spawn_blocking`）：
  1. 写的是本地文件系统，不是网络盘；
  2. 单次写入量有上界（`LOG_LIMIT_PER_RUN` 已有编译期断言兜底，见 §3）；
  3. 不在紧循环里调用 —— 调用点是 2 秒一次的攒批，不是每条日志一次。
  代码侧的对应约束：`mcloud-store` 全程**不跨 `.await` 持锁**，
  这一点由「用 `std::sync::RwLock` 而非 `tokio::sync::RwLock`」编译期保证。
- **锁**：用 `std::sync::Mutex` 保护**不跨 `.await`** 的临界区（如 `runner.rs` 的
  `inflight`）。一旦需要跨 `.await` 持锁，必须换 `tokio::sync::Mutex`。
- **锁中毒**：沿用 `expect("...锁中毒")`，因为毒锁意味着已有 panic，继续运行无意义。
- **并发闸**：以 `task:account` 为键。**同一键已在运行时不排队**，直接返回
  `Skipped`——排队会让定时任务堆积，掩盖真实问题。
- **重试**：只对**幂等**请求重试，且必须指数退避（参考架构文档 §5.3 的 `1s·2^i`）。
  非幂等的写操作（签到、兑换）**不许自动重试**。

---

## 7. 领域与 wire 契约【硬约束】

前端与上游都依赖这些形态，改动等于破坏兼容。

### 7.1 序列化形态

- **命名有两种形态，取决于类型属于哪一批**（这不是笔误，是既成事实）：

  | 批次 | 形态 | 类型 |
  |---|---|---|
  | 早期领域类型 | **`snake_case`**（Rust 默认，不加注解） | `RunRecord`、`RunEvent`、`TaskOutcome`、`AuthInfo` |
  | 后期新增类型 | **`camelCase`**（`#[serde(rename_all = "camelCase")]`） | `LogLine`、`Schedule`、`Pipeline`、`PipelineStep`、`PipelineSchedule`、`ServiceStatus`、`CancelResponse`、`ScheduleInput`、`PipelineInput` |

  **新增类型一律用 `camelCase`**（与前端习惯一致，也是后期批次的方向）。
  但**不要**为了"统一"去改早期类型的命名：它们已经落盘（`history.json`、
  `accounts.json`）且被前端读取，改名是破坏性变更。
  改任何 wire 字段前，先在前端 `web/src/api.ts` 里搜一遍它有没有被依赖。

  > 本节此前写的是「JSON 字段**一律** snake_case，**不要**加 camelCase」——
  > 与代码不符：生产代码里有 11 个类型在用 camelCase。
  > 照那句话去改会把前端打挂。

- 枚举用 `#[serde(tag = "...")]` 做内部标签，**标签名是 wire 契约**：

```rust
// mcloud-core/src/run.rs
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskOutcome { Success{..}, Skipped{..}, Failed{..} }
// → {"status":"success","summary":"ok"}

// mcloud-core/src/event.rs
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent { Started{..}, Log{..}, Progress{..}, Finished{..} }
// → {"type":"finished","run_id":1,"outcome":"success","duration_ms":1832}
```

改动这些 tag 或字段名**必须同步改前端**，并视为破坏性变更。

### 7.2 时间

- **秒级**用于凭证过期（`AuthInfo::expire`、`days_until_expiry`）。
- **毫秒级**用于事件与运行记录（字段名带 `_ms` 后缀）。
- 时间一律取 `Clock` trait，**不许直接 `SystemTime::now()`**——
  测试要靠 `FixedClock` 才能稳定。

### 7.3 凭证与脱敏【安全红线】

- 日志、事件、错误信息里**绝不能出现完整凭证**。
- 所有对外文本过 `mcloud_core::event::redact()`，它覆盖
  `authorization` / `jwttoken` / `token` / `ssoToken` / `auth` / `cookie`
  （大小写不敏感，支持 `k=v`、`k: v`、`"k":"v"` 三种形态）。
- **新增凭证字段名必须同步加进 `redact()` 的 `KEYS`**，否则会静默泄漏。
- 凭证按域名注入（`mcloud-upstream/src/endpoints.rs`），**这是安全边界，不许放宽**：
  `basic` 只给 `AUTH_DOMAINS`，`jwt` 只给 `JWT_HOSTS`（精确匹配，子域不算）。
- 写盘保留 `.bak`（`JsonStore::write_atomic` 已实现，注意实际文件名是
  `accounts.bak` 而非 `accounts.json.bak`——`with_extension` 会替换扩展名）。

> **权限已落地**：数据目录 `0700`、凭证文件 `0600`。
> 关键在于**创建时**就限制，不是写完再 `chmod` —— 先按默认权限写、
> 再收紧，中间有一小段窗口同机其他用户能读到凭证。所以用的是
> `OpenOptions::mode` / `DirBuilder::mode`。
> 老数据目录（默认 umask 装出来的）在 `open` 时会被收紧，覆盖升级路径。
> 目录是 `0700` 而非 `0600`：目录必须可进入（x 位）才能读到里面的文件。

### 7.4 持久化

- 写文件必须**原子**：临时文件 → `rename`（见 `JsonStore::write_atomic`）。
- 历史记录保留上限 `HISTORY_LIMIT = 200`。
- 演进到 SQLite 时只换实现，**`Store` trait 与业务代码不动**。

---

## 8. 测试【硬约束】

### 8.1 分层策略

| 层 | 方式 | 现状 |
|---|---|---|
| `mcloud-crypto` | **黄金向量对拍**（`test/golden/*.json`） | ✅ 已有 `tests/golden.rs` |
| `mcloud-upstream` | fixture 回放（录制真实响应，离线断言） | ⬜ 待补 |
| `mcloud-engine` | 假 Task + 假 Store，断言事件序列与并发闸 | ✅ `runner.rs` 已覆盖（见 §8.5） |
| `mcloud-server` | `tower::ServiceExt::oneshot` 路由级测试 | ✅ `tests/routes.rs` 已覆盖（见 §8.6） |
| web | `tsc --noEmit` + 关键组件测试 | ⬜ 待建 |

### 8.2 黄金向量铁律

`test/golden/*.json` 是**权威**，来自 TS 原实现 + 真实抓包。

> **任何不一致都是 bug，禁止「调整期望值」让测试变绿。**

向量不可字节级对拍的部分（动态 IV、随机数），改为**往返测试**
（`dynamic_iv_roundtrip_with_golden_key` 是范例）。

签名相关的测试必须用 `sign_headers_with`（显式传 id/nonce）保证可复现；
生产代码才用 `sign_headers`（自动随机）。

### 8.3 测试写法

- 单测放同文件 `#[cfg(test)] mod tests`；跨 crate 的集成测试放 `<crate>/tests/`。
- 测试名用**陈述句**描述被保护的行为，不要叫 `test1`：
  `skipped_is_not_failure`、`duplicate_registration_is_rejected`、`runs_are_newest_first`。
- **每个测试都要有明确的失败信息**：断言带上下文，
  `assert_eq!(got, want, "AES 用例 {}", c.name)`。
- 断言要覆盖**边界**：`refresh_threshold_is_exclusive` 就同时测了「恰好 10 天不刷新」
  和「9 天刷新」——这种「阈值是开区间还是闭区间」的测试最值钱。
- 涉及磁盘的测试用临时目录并在结束时清理（见 `json_store.rs` 的 `tmpdir`）。
- **共享测试替身放 `mcloud_core::testkit`**，用 `feature = "testkit"` 门控：

  ```toml
  # 其他 crate 的 dev-dependencies
  mcloud-core = { workspace = true, features = ["testkit"] }
  ```

  `FixedClock` 已迁到这里（原先埋在 `mcloud-store` 的 `#[cfg(test)]` 里，
  别的 crate 根本用不到）。**不要在各自 crate 里再复制一份**——那样迟早会出现
  几个行为不一致的假实现，让测试结果取决于你碰巧用了哪一个。

- **测试必须能快速失败**：涉及并发的用例一律给等待加超时
  （`tokio::time::timeout`）。闸逻辑一坏，无超时的等待会让整条测试挂到 CI 超时，
  那是比失败更难排查的形态。`runner.rs` 的 `wait_started` 是范例。

### 8.4 补测优先级

当前空白按风险排序，建议依次补：

1. **`mcloud-upstream` 的凭证注入边界**——域名匹配已有测试，但
   `ReqwestClient` 的注入行为没有；这是安全边界，漏了会把凭证发错域名。
2. **`mcloud-upstream` 的 fixture 回放**——录制真实响应离线断言，
   让上游改字段时能在 CI 上发现，而不是等用户报错。
3. **web 层**——`tsc --noEmit` 已有 CI 门禁；组件测试待建。

### 8.5 已固化的行为契约（`runner.rs`）

`Runner` 的并发闸是「同 `task:account` 不并发」的核心，改动它前先读这些测试
（每条都是先写坏实现、确认测试真的变红之后才留下的）：

| 契约 | 测试 |
|---|---|
| 闸键是 `task:account`，不串任务也不串账号 | `gate_key_is_scoped_by_task_and_account` |
| 已在运行时**跳过而非排队** | `same_task_account_while_running_is_skipped_not_queued` |
| **失败路径也必须释放闸**（否则任务从此再也跑不起来） | `gate_is_released_even_when_task_fails` |
| 任务报错 → `Failed` 且仍落盘、仍发 `finished` | `task_error_becomes_failed_outcome_and_is_still_recorded` |
| 跳过不是失败（不触发告警） | `skipped_outcome_is_preserved_as_success_path` |
| 历史落盘失败**不**让整次运行算失败 | `history_write_failure_does_not_fail_the_run` |
| run_id 递增、不占用被跳过的运行 | `run_ids_start_at_one_and_increase` |
| 重启后从历史最大值 +1 续号 | `seed_run_id_continues_after_restart` |
| 时间一律来自注入的 `Clock` | `record_time_comes_from_injected_clock` |


### 8.6 已固化的行为契约（`mcloud-server/tests/routes.rs`）

路由测试用 `tower::ServiceExt::oneshot` 直接打 `Router`，**不起监听端口** ——
端口会冲突、要处理时序，CI 上还可能与正在跑的服务抢 8080。

| 契约 | 测试 |
|---|---|
| 未知**任务名** → 400（`Config`），不是 404/500 | `unknown_task_is_400_not_500` |
| 账号不存在 → 400，且信息里带账号名 | `missing_account_is_400` |
| 运行记录不存在 → 404 | `missing_run_is_404` |
| 不存在运行的日志 → **404**（不是 200+`[]`） | `logs_of_missing_run_is_404_not_empty_list` |
| 存在但无日志 → **200 + `[]`**（不是 404） | `logs_of_existing_run_without_lines_is_empty_200` |
| 取消没在跑的运行 → 200 且 `cancelled=false`（不是错误） | `cancelling_idle_run_is_ok_not_error` |
| `taskCount` 是数字不是 `null`（`null` 会让前端白屏） | `status_task_count_is_a_number` |
| 运行列表最新在前 | `runs_list_only_returns_own_records` |

> **踩过的坑**：`unknown_task` 原先返回 404，而 engine 对同一情况返回
> `Config`(400) 并有测试锁着 —— 同一个"未知任务"在两层是两个错误类型。
> 而且 404 语义也不对：错的是**请求体里的字段**，不是 URL 指向的资源，
> 客户端会以为 `/api/runs` 这个端点不存在。已统一为 400。

### 8.7 测试替身的共享

见 §8.3 的 `mcloud_core::testkit`。**不要**在各自 crate 里复制假实现。

---

## 9. 提交与变更

### 9.1 提交信息

Conventional Commits，**中文正文**：

```
<type>(<scope>): <简短描述>

<为什么这么改，而不是改了什么——diff 已经说明了改了什么>
```

- `type`：`feat` / `fix` / `refactor` / `perf` / `test` / `docs` / `chore` / `build`
- `scope`：crate 短名（`crypto`、`engine`、`server`）或领域（`sign`、`auth`）
- 例：`fix(core): redact 未处理带引号的 JSON 键值`

### 9.2 提交前自检【硬约束】

```bash
cargo fmt --all -- --check          # 必须无输出
cargo clippy --workspace --all-targets
cargo test --workspace              # 必须 0 failed
```

三条全绿才能提交。**本次提交新引入的告警/失败由本次提交负责**——
当前 §3.3 记录的欠账为零，所以「本来就有」这个借口不存在。

CI 已配置（`.github/workflows/ci.yml`），上面三条即为合并门禁，
另加 MSRV 编译与前端 `typecheck`。

### 9.3 任务新增清单

新增一个任务时，逐项确认：

1. 在 `mcloud-tasks/src/<name>.rs` 实现 `Task`，`name()` 用 kebab-case。
2. 在 `mcloud-tasks/src/lib.rs` 的 `register_all` 登记（重名会返回错误，不静默覆盖）。
3. 若涉及活动开关，在 `AccountSettings` 加字段，**默认值遵循「保守开启」**：
   能稳定跑通的默认开，涉及额外消耗或易触发风控的默认关。
4. `run` 里先 `ctx.progress(...)` 上报步骤，关键节点 `ctx.info(...)`。
5. 条件不满足返回 `Ok(TaskOutcome::skipped(...))`，不要返回 `Err`。
6. 在 `docs/architecture.md` §7 的任务清单表里补一行。

---

## 10. 仓库卫生

### 10.1 版本控制

仓库已初始化（分支 `main`），首次提交收录 64 个文件，`.gitignore` 排除
`target` / `.cargo-home` / `node_modules` / `dist` / `data`。
此后所有改动走提交，**不再直接改工作区不留痕**。

`.gitignore` 必须持续覆盖：

```gitignore
# 不锚定根目录（见下方说明）
target/
.cargo-home/

/web/node_modules
/web/dist
/data
*.log
```

> **`target/` 与 `.cargo-home/` 不要写成 `/target`、`/.cargo-home`。**
> 带前导 `/` 只匹配仓库根目录下的那一层。本项目实际出现过
> `crates/mcloud-server/src/routes/.cargo-home/` —— 某次在子目录里跑了
> cargo，生成了 **134MB** 的 registry 副本；根锚定的规则挡不住它，
> 会被直接提交进去。凡是被 cargo/构建工具在任意深度生成的目录，
> 一律用不锚定的写法。

> **不要提交 `data/`**：那是运行期账号与凭证数据。
> 提交前用 `git status` 确认没有 `data/` 下的文件混入。

### 10.2 目录约定

```
crates/       Rust 各层
docs/         设计文档（architecture.md / conventions.md）
test/golden/  黄金向量（权威数据，勿手改；从 mcloud 同步）
web/          前端（Vue Vapor + TSX）
data/         运行时数据（不入库）
```

**本仓库只放实现代码。** 抓包记录、逆向素材、客户端产物、一次性分析脚本
一律留在上游 [`mcloud`](../../mcloud/) 仓库，不复制进来：

- `mcloud/har/` —— 抓包归档
- `mcloud/test/golden/` —— 黄金向量原始出处（本仓库按需同步副本）
- `mcloud/test/fixtures/` —— 脱敏夹具
- `mcloud/scripts/` —— asar 解包等分析脚本

这样本仓库能保持「可读、可审计、无第三方产物」，也避免两份素材各自漂移。

### 10.3 文件头注释

每个 crate 的 `lib.rs` 必须用 `//!` 写明：**职责**、**铁律**（不允许做什么）、
以及必要时**为什么**（如 `mcloud-crypto` 解释为何不需要国密）。
参照 `mcloud-core/src/lib.rs` 与 `mcloud-crypto/src/lib.rs` 的写法。

---

## 11. 待清理清单

按优先级排列，供后续处理（**本文档只记录，未擅自修改**）：

| # | 项 | 位置 | 类型 |
|---|---|---|---|
| 1 | `EventHub::forget` **零生产调用** ⇒ 内存里的 run 状态无上限增长 | `mcloud-app/src/hub.rs` | 内存 |
| 2 | `mcloud-upstream` 无 fixture 回放；`ReqwestClient` 注入行为无测试 | 见 §8.4 | 测试 |

> **第 1 项的证据**（`forget` 存在且文档写明用途，但没人调用）：
> `publish` 用 `runs.entry(run_id).or_default()` 建状态，唯一的移除路径是
> `forget`，而它在生产代码里**一次都没被调用**（只有 `hub/tests.rs` 调）。
> 于是每跑一次就多留一份 `RunState`（含最多 `HISTORY_PER_RUN` 条事件），
> 而磁盘侧的 `HISTORY_LIMIT = 200` 只管落盘。
> **不能**简单地在结束时清掉：SSE 与 `/logs` 都以内存历史优先，
> 清早了会让"运行刚结束、用户来看日志"变成空列表。必须等日志确实落盘。
> 这是设计问题，不是顺手能改的一行。

> 已完成：`git init` + 首次提交、`clippy` 归零、`fmt` 归零、CI 配置、
> MSRV 修正为 1.88、`Runner` 测试补齐（14 条，见 §8.5）、
> `FixedClock` 迁到 `core::testkit`（见 §8.3）、
> DES-ECB 实现并消费 21 条黄金向量（原记为"死依赖"，实为未实现的功能）、
> 凭证目录/文件权限（`0700`/`0600`，见 §7.3）、
> 阻塞写盘改判为**有意的例外**并给出实测依据（见 §6）、
> `mcloud-server` 路由测试 12 条（见 §8.6）。
>
> **记录本身也会失实**，几条值得留意的教训：
> - 「`des` 是死依赖」是错的：依赖声明没错，缺的是实现，
>   而黄金向量里早有 21 条用例在等消费者。差点把功能连同向量一起删掉。
> - 「async 里用阻塞 `std::fs`」被记为缺陷，但实测单次写盘 0.19ms、
>   调用节拍 2 秒一次，`spawn_blocking` 的调度开销比它省下的还多。
>   它是有意为之的取舍，真正的问题在于 §6 把规则写成了绝对禁止。
> - 「SSE 因同步 `create` 收不到事件」已过时：前端早已用
>   `background: true` 拿真实 run id 再订阅，后端也返回真实 id。
> - §7.1 原先写「字段**一律** snake_case，**不要**用 camelCase」，
>   而生产代码里有 11 个类型在用 camelCase。照那句话改会把前端打挂。
> - `architecture.md` §9.2 的 camelCase 分歧也已被修掉，注记随之删除。
>
> **清单项在下手前要重新核实，不要照着描述直接改。**
>
> 处理原则：清单项**默认只记录**。但若某项会让已落地的门禁必然失败
> （例如声明 `rust-version = "1.82"` 而 CI 要用它编译），那属于门禁本身的
> bug，必须当场修掉并在此留痕——留着一条永远红着的 CI 比没有 CI 更糟。
> 其余项请在专门提交里清账，并同步删掉对应行。

---

## 附：一分钟速查

```bash
cargo fmt --all -- --check                          # 格式
cargo clippy --workspace --all-targets              # Lint
cargo test --workspace                              # 测试
cargo run -p mcloud-server --bin mcloud-cli -- tasks  # 列出任务
```

改代码前问自己三句：

1. 这个 crate **允许**碰这个依赖吗？（§1）
2. 这是「失败」还是「跳过」？（§5.3）
3. 我加的东西**会不会把凭证写进日志**？（§7.3）
