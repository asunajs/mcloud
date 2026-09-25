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
| 工具链 | stable `1.91.1`，`rust-version = 1.82`，`edition 2021` |
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
- **禁止在异步上下文里做阻塞 IO**。`std::fs`、`std::thread::sleep` 会卡住整个 runtime。
  需要时用 `tokio::fs` 或 `tokio::task::spawn_blocking`。
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

- **JSON 字段一律 `snake_case`**（Rust 默认），**不要**加 `rename_all = "camelCase"`。
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

> ⚠️ **已知文档分歧**：`architecture.md` §9.2 的 SSE 示例写的是 camelCase
> （`"durationMs"`），而**代码实际输出 snake_case**（`duration_ms`，已实测）。
> **以代码为准**，`architecture.md` 该处待修正。

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

> **尚未实现、但必须遵守**：配置目录权限 `0600`。当前代码没有任何
> `set_permissions` 调用，`data/` 目录按默认 umask 创建。这是安全欠账，
> 见 §11。在它落地前，不要把含真实凭证的 `data/` 放在共享目录。

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
| `mcloud-engine` | 假 Task + 假 Store，断言事件序列与并发闸 | ⚠️ 仅注册表有测试 |
| `mcloud-server` | `tower::ServiceExt::oneshot` 路由级测试 | ⬜ 待补 |
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
- **`FixedClock` 目前是 `mcloud-store` 里的 `#[cfg(test)]` 私有项**，其他 crate 的测试
  用不到。engine 层要测时间相关逻辑时，把它提为 `#[cfg(any(test, feature = "testkit"))]`
  的公开项，不要各自复制一份。

### 8.4 补测优先级

当前空白按风险排序，建议依次补：

1. **`mcloud-engine` 的 `Runner`**（`runner.rs` 目前 0 测试）——
   并发闸、事件序列、`task:account` 去重是核心逻辑，最该有测试。
2. **`mcloud-upstream` 的凭证注入边界**——域名匹配已有测试，但
   `ReqwestClient` 的注入行为没有；这是安全边界。
3. **`mcloud-server` 路由**——用 `oneshot` 断言状态码与 JSON 形态。

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

CI 尚未配置（见 §11），目前靠人工执行；配置后这三条即为合并门禁。

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
/target
/.cargo-home
/web/node_modules
/web/dist
/data
*.log
```

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
| 1 | `des` 依赖已声明但 `des.rs` 未实现 | `mcloud-crypto` | 死依赖 |
| 2 | `architecture.md` §9.2 SSE 示例用 camelCase，与代码不符 | `docs/architecture.md` | 文档 |
| 3 | `Runner` 无测试（核心并发逻辑裸奔） | `mcloud-engine/src/runner.rs` | 测试 |
| 4 | `json_store` 在 async 中用阻塞 `std::fs` | `mcloud-store` | 并发 |
| 5 | `FixedClock` 是 `#[cfg(test)]` 私有项，其他 crate 用不到 | `mcloud-store/src/clock.rs` | 可测性 |
| 6 | 无 CI 配置，§9.2 的三条命令靠人工执行 | 根目录 | 工程 |
| 7 | `mcloud-server` 路由 0 测试；`upstream` 无 fixture 回放 | 见 §8.4 | 测试 |
| 8 | SSE 只按 `run_id` 过滤，且同步 `create` 不产生可订阅事件 | `mcloud-server/src/routes/runs.rs` | 设计 |
| 9 | 凭证目录权限 `0600` 未实现（无 `set_permissions`） | `mcloud-store` | 安全 |

> 已完成：`git init` + 首次提交、`clippy` 归零、`fmt` 归零（见 §3.3、§10.1）。
> 本文档**只记录，不擅自修改代码**——清账请在专门的提交里做，并同步删掉对应行。

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
