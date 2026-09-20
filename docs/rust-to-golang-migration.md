# Rust → Go 迁移评估报告

> 评估对象：本仓库当前的 Rust 实现（`crates/`，8 个 crate）。
> 结论口径：**这是"用 Go 重写一遍"，不是"把 Rust 翻译成 Go"**。下面用实测数字说明为什么。
> 数据取自 `b9d7170` 之后的工作区，行数/测试数当天有效，引用前先 `wc -l` 复核。

---

## 1. 迁移对象有多大（实测）

| crate | 行数 | `.rs` 文件 | 其中测试 | 职责 |
| --- | --- | --- | --- | --- |
| `mcloud-core` | 9,850 | 34 | 1,936 | 纯计算：密码学、错误类型、`run` 四态、trait 契约 |
| `mcloud-io` | 8,697 | 41 | 3,629 | **唯一**允许 IO 的层：reqwest / rusqlite / 随机源 |
| `mcloud-upstream` | 8,064 | 45 | 3,125 | 协议层：请求构造 + 响应解析 + 错误码语义 |
| `mcloud-app` | 6,402 | 27 | 1,064 | 编排：签到全链路、登录、迁移、账号档案 |
| `mcloud-cli` | 3,817 | 12 | 0 | 命令行入口 |
| `mcloud-capabilities` | 1,362 | 6 | 491 | 能力层（登录状态机等） |
| `mcloud-exec` | 1,001 | 1 | 0 | 执行/事件流 |
| `mcloud-server` | 808 | 4 | 0 | HTTP 产物（axum + 静态资源） |
| **合计** | **40,001** | **170** | **10,245** | |

配套资产（**这些是迁移时真正的价值所在，且与语言无关**）：

- 626 个测试（`cargo test --workspace`，2026-09-20 实测全绿）
- 6 份黄金向量、脚本侧 122 条断言（`node scripts/check-golden-vectors.cjs`）
- 七条门禁（`docs/dev-gates.md`）+ `pre-commit` / `pre-push` 钩子
- 一批抓包实证文档（`docs/upstream-*.md`、`docs/capture/`）

结构性事实：`415` 处 `async fn`、`8` 个 `pub trait`（`Clock` / `Random` / `HttpClient` /
`StateStore`〔19 个方法〕 / `UserStore` / `ConfigStore` / `LoginObserver` / `LoginPayloadDecryptor`）、
`11` 处 `<C, K, R>` 三泛型依赖注入、`50` 行 `pub use` 转发。

---

## 2. 依赖对照表

| Rust | Go 对应 | 风险 |
| --- | --- | --- |
| `tokio` | goroutine + `context` | 中：无 runtime 概念，但取消/超时要点全在调用方，见 §3.4 |
| `reqwest`（rustls + gzip/brotli/deflate + cookies） | `net/http` | 中：Go 自动解 gzip；**brotli 需第三方 transport**；cookie jar 换 `net/http/cookiejar`；TLS 指纹与 HTTP/2 行为与 rustls 不同，需双跑对拍 |
| `rusqlite`（`bundled`） | `modernc.org/sqlite`（纯 Go，免 cgo） | 中：换驱动后要**专门验一次 schema v1→v7 迁移**；`chrono` 列类型改 `time.Time` |
| `axum` 0.8 + `tower-http`(fs) | `net/http` + `chi`，静态用 `http.FileServer` | 低：`mcloud-server` 仅 808 行 |
| `clap` derive | `cobra` + `pflag` | 低：`b9d7170` 要钉的"参数互斥"在 cobra 里是 `MarkFlagsMutuallyExclusive`，**比手写更省事** |
| `serde` / `serde_json` | `encoding/json` + tag | **高**：上游解析大量用 `Value` 手取字段（`require_str` / `require_i64`），Go 侧是 `map[string]any`，漏判即 panic —— 见 §3.2 |
| `thiserror` / `anyhow` | 具体 error 类型 + `errors.Is/As` / `%w` | 中：错误分类语义可保，**穷尽性检查会丢** |
| `chrono` + `chrono-tz` | `time` + `time/tzdata` | 低：签到"今天"的 `Asia/Shanghai` 判定有测试钉住 |
| `md-5` `sha1` `sha2` `hmac` `aes` `cbc` `des` `rsa` `cipher` | 标准库 `crypto/*` **全覆盖** | 低：这是最省力的一块；但 `crypto/des` 属弱算法，安全类 linter 会报，需要写明豁免理由 |
| `argon2` | `golang.org/x/crypto/argon2` | 低：现有"盐由 `Random` 注入以钉住同盐同哈希"的做法要照搬，别用库自带随机源 |
| `flate2` `base64` `hex` `uuid` `rand`(OsRng) | `compress/gzip` `encoding/{base64,hex}` `google/uuid` `crypto/rand` | 低 |
| `tracing` / `tracing-subscriber` | `log/slog` | 低 |
| `qrcode`（终端字符画 + `--qr-out` SVG） | `skip2/go-qrcode` | 低 |
| `rpassword` | `golang.org/x/term` | 低：口令仍只从 stdin 读、不回显 |
| `Random` trait（`fill_bytes` / `uuid_v4` / `below` 拒绝采样） | 自定义 `interface { io.Reader; … }` | 中：测试里的确定性假随机源可平移，但 `below()` 的**无偏采样**要逐位对拍 |

**密码学与编码层在 Go 标准库里几乎一一对应**，所以"能不能做"从来不是问题；
问题全在 §3 那四条结构性退化和 §5 的对拍成本。

---

## 3. 四条真正的难点（不是翻译，是返工）

### 3.1 泛型依赖注入 → 依赖容器

现在的写法（`mcloud-app/src/signin/query.rs` 为代表）：

```rust
pub async fn cloud_records<C, K, R>(
    upstream: &Upstream<'_, C>, profile: &AppProfile, credential: &SignInCredential,
    query: &RecordsQuery, clock: &K, rng: &mut R,
) -> Result<CloudRecords, Error>
where C: mcloud_core::HttpClient, K: Clock, R: Random { … }
```

Go 没有"函数级类型参数把依赖说清楚"这种用法，惯例是把 `Client`/`Clock`/`Rand` 收进一个结构体：

```go
type Service struct {
    Up   *upstream.Client
    Clock Clock
    Rand  io.Reader
}
func (s *Service) CloudRecords(ctx context.Context, cred *SignInCredential, q RecordsQuery) (*CloudRecords, error)
```

净影响：**签名变短、可读性变好**（`b9d7170` 之前那条 `too_many_arguments` 红点在 Go 里根本不存在）。
但 415 个 `async fn` 与 11 处三泛型壳都要重新落依赖，`mcloud-upstream` 里"这一层不许有 IO"
从**编译期事实**变成**lint 约定**（Go 的 interface 是结构性实现，任何包都能满足它）。

### 3.2 穷尽性与空值：错误语义会降级

- `run.rs`（495 行）把"四态结果 / 三类失败 / 需人工"用 enum 钉住；Go 只能 `const iota` + 约定，
  `switch` 漏一个态不再有编译错误。该 commit 的原话是：**不先收敛这四条语义，19 个能力会各写一套判据，
  重演旧项目** —— 迁 Go 削弱的正是这条防线（除非加装 `exhaustive` linter，且它只管字面 switch）。
- 上游响应里 `Option`/`serde_json::Value` 的手取字段（`require_str` / `require_i64` / `ensure_ok`）
  在 Go 里是 `map[string]any` 的链式取值，**漏判直接 panic**，而 Rust 侧漏判是编译错误。
  这一条决定了：Go 版必须先把这几个 helper 写成一等 API 并配对拍测试，禁止在业务里裸取。

### 3.3 可见性与测试目录：刚拆完的结构要重画

Rust 有 `pub(crate)` / `pub(super)` / 私有，Go 只有"首字母大小写"两档、且**一个目录一个包**。

现在（2026-09-20 刚落地的拆分）：

```
crates/mcloud-upstream/src/app_signin/   ← 11 个生产子模块 + tests/ 下 11 个测试子文件
crates/mcloud-app/src/signin/            ← 9 个生产子模块 + tests/ 下 6 个测试子文件
```

- 生产侧的"一主题一文件"在 Go 里没问题（同包多文件）。
- **测试侧的 `tests/` 子目录在 Go 里是另一个包**，看不见父包私有项。当前这些测试文件靠
  `use super::*` 共享 helper（`SeqRandom`、`vectors()`、`ScriptedClient`），迁移时要改成
  同目录 `xxx_test.go`（同包，可见私有）或把 helper 导出。
- `include_str!("../../../../../test/golden/*.json")` → Go 的 `testdata/` 约定（自动不下沉、
  不打包），或用 `embed`。**注意**：`check-golden-vectors.cjs` 现在靠正则认 `include_str!`，
  而它已经因为 rustfmt 折行误报过一次（`app-signin-vectors.json` 被当成死向量）；
  Go 侧这条探测必须重写，否则"谁读了这个向量"这道防线直接失效。

### 3.4 生命周期与并发：编译期保证 → 运行期检查

- `&mut R rng` 保证同一随机源不被并发共享；Go 里 `io.Reader` 是引用语义，并发用同一个源是
  **data race**，只能靠 `-race` 兜。→ `go test -race` 必须进 Go 侧门禁第一版。
- `&Upstream<'_, C>` 的借用期检查消失；取消语义要显式 `ctx` 穿到每个上游调用（等于再改一遍所有签名）。

---

## 4. 保命网的现状（迁移前就该修，与语言无关）

| 现状 | 影响 |
| --- | --- |
| CI 已删除（`15e053b`：存在 9 个提交、从未产生任何影响） | 门禁只在本地钩子里跑 |
| `scripts/gates.sh` 在 macOS（`/bin/bash` 3.2）上**假绿**：`declare -A` 需 bash 4+，打一行 `line 91: test: unbound variable` 就 exit 0，七条一条都不跑 | 用它验门禁等于没验；作者环境是 Windows/Git Bash，看不见这个坑 |
| `gates.sh` 的 clippy 那条是 `--all-targets`，缺 `--workspace` | 只 lint 当前包 |
| `pre-commit` 只跑不需要编译的四条 | 编译/测试靠 `pre-push` 兜 |

**这三条比换语言便宜两个数量级**。若迁移动机是"让别人也能信这套代码"，先补 CI 与修脚本。

---

## 5. 若决定迁：顺序与每步退出判据

原则：**保命网先行，双跑对拍收口，Rust 侧不下线**。

| 步 | 内容 | 退出判据 |
| --- | --- | --- |
| 0 | 先搬守卫：`check-identifiers`（含 base64 解码复检）、golden 向量对拍、kebab-case、分层 depguard；`go test -race` | 一条真实标识/向量不放过；Go 侧能算出同一批断言 |
| 1 | `mcloud-core`（纯计算，标准库覆盖最好，风险最低） | 6 份黄金向量在 Go 侧逐字段复算通过 |
| 2 | `mcloud-upstream`（协议层，向量兜底最厚：3,125 行测试） | 同一条抓包请求 Rust/Go **双跑**，header 顺序 / percent-encode / 签名串逐字节一致 |
| 3 | `mcloud-io`（唯一 IO 层） | 纯 Go sqlite 驱动下 schema v1→v7 迁移 + 老库只读导入结果一致 |
| 4 | `capabilities` → `exec` → `app` → `server`/`cli` | 19 项能力逐项打勾；CLI 参数互斥与四态输出一致；`mcloud-server/tests/api_contract.rs` 的契约测试平移，前端（1,265 行 TS）不动 |

全程约束：**凭据边界不变** —— 仓库只允许 `199` 开头的合成标识，`authToken` / `jwt_token` / `auth`
不得打印、落库、进 fixture；Go 的 `testdata/` 同样受守卫管辖（见 `docs/credential-boundary.md`）。

---

## 6. 工作量

生产代码 `29,756` 行 + 测试 `10,245` 行。测试的**断言与向量可复用，代码要重写**。

按 Go 侧含门禁返工与双跑对拍的实际吞吐 ~250 行/人日估：

- 乐观（协议/密码层复用度高、对拍脚本先建好）：**~90 人日**
- 中值：**~120 人日**
- 悲观（sqlite 驱动、brotli、`Value` 手取字段返工）：**~150 人日**

不确定性的三个来源，按大小排：① 上游协议行为必须双跑才能定论（换语言不改变这一点）；
② `bundled` sqlite 换纯 Go 驱动的迁移语义；③ §3.2 的空值/穷尽性返工量。

任何"自动翻译"路线都不成立：泛型壳、生命周期、enum 穷尽性在 Go 没有对应物，
翻译器只会产出一个能编译但丢掉全部防线的 Go 程序。

---

## 7. 建议

**支持迁移的硬理由**（满足任一才值得投入 §6）：

1. 目标交付环境拿不到 Rust 工具链，而 Go 是既定栈；
2. 要进 K8s / serverless / 内部 Go 基础设施，且由 Go 团队长期接手；
3. 要给非开发者分发单文件可执行（注：Rust 侧已能 static 单文件，这条**不构成差异**）。

**不支持的理由**：

1. 本项目当前最大风险是**协议行为正确性**，与语言无关；换语言不减小这个风险，反而在 §5 每一步重新引入；
2. 已经付过成本的编译期保证（分层铁律、四态穷尽性、`Option` 强制判空、可见性）有 4 条会降级成 lint + 约定（§3）；
3. 1.02 万行测试与 122 条向量断言是这套代码真正的资产，迁移期间它们**必须双活**，等于两个语言同时维护；
4. 真正的当务之急是 §4 那三条（CI 缺失 + 一键门禁假绿），成本是"改脚本"，不是"改语言"。

**折中方案**（若动机只是"给别人用/进 Go 生态"）：协议内核留 Rust，宿主（server/cli）用 Go，
经 cgo + static lib 暴露。代价是两套构建与 FFI 边界，实测往往大于收益；
本项目已有 `check-deps.sh` 挡层，这种"混栈"需要新增一层门禁才不失控。

---

## 8. 需要拍板的三件事

1. 迁移的**真实动机**是上面哪一条？（决定 §6 取哪个档，也决定要不要做 §5 步 0）
2. 是否接受"迁移期间 Rust 侧继续作为基准、Go 侧只读对拍"（不接受则双跑对拍无从做起，风险翻倍）？
3. §4 的三条要不要**现在就修**（与是否迁移无关，都是净收益）？
