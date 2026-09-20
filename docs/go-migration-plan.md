# Rust → Go 迁移执行方案

> 状态：**执行方案** —— 谁来做、按什么顺序做、每一步怎么算做完。
>
> - 评估（写于**决定之前**，论证"要不要迁"）：[`rust-to-golang-migration.md`](./rust-to-golang-migration.md)
> - 冻结后的约定：[`../legacy/README.md`](../legacy/README.md)
> - 门禁规范：[`dev-gates.md`](./dev-gates.md)
> - 拍板日期：2026-09-20

---

## 0. 已拍板的四个决策

| 决策 | 结论 |
| --- | --- |
| **分工** | 本方案与 Go 骨架（`go.mod` / `internal/` / `cmd/` / Go 门禁）由**抓包会话**产出；**业务迁移由实现会话执行** |
| **Rust 侧** | **冻结到 `legacy/`**，只读保留，作 Go 侧的对拍基准 |
| **范围** | **全部 8 个 crate**（含 `mcloud-server` 与 `web/` 的静态服务） |
| **验证落点** | **协议验证改走 Go**（2026-09-20 用户确认）—— 落点 `cmd/mcloud` + `internal/io`，顺带把编排层跑一遍。⚠ 用户 2026-09-19 的原约定是"验证走 Rust"，现在**别再往 `legacy/` 里写探针**：冻结代码只作对拍基准，用它做验证会**越验越脱离主线** |

---

## 1. 迁移动机（实测）

用户原话：

> 「停，现在需要将项目从rust迁移到golang，rust编译严重影响开发体验，后序代码更多可能就来不及切换了」

实测（macOS arm64 / rustc 1.91.1 / 改 `mcloud-core/src/lib.rs` **一行**后）：

| 命令 | 耗时 |
| --- | --- |
| `cargo check --workspace` | **1 分 58 秒**（`/usr/bin/time -p` → `real 118.71`） |

### ⚠ 这条与 `docs/dev-gates.md` §耗时 直接冲突

那份文档写着：

> **先说结论：写代码后推送 30 秒左右，不需要换语言，也不需要 `sccache`。**

它**在 macOS 上不成立**。两个原因：

1. 那组数据是在 **Windows + Git Bash** 上测的（文档自己也承认"只在 Windows 上实际跑过"）；
2. 当时的 workspace 比现在小 —— 此后又长出了 `mcloud-core/src/cron.rs`、
   `mcloud-server/src/routes/` 等。

**根因不是"Rust 慢"**，是**改最底层 crate 触发全量重编**：
`mcloud-core` 之上 7 个 crate 全都要重新 check。这是单 workspace 的固有代价，
**不是调参能解决的**（`incremental` 已按 `0c65b35` 关掉，那解决的是另一个问题）。

对照：Go 侧同规模改动的 `go build ./...` 是**秒级**（见 §7 的实测）。

> 📌 **待办**：`docs/dev-gates.md` §耗时 要按本节修订 —— 现在那条结论会误导人
> 以为"不需要换语言"。修法不是删掉它，而是**标注测量平台**：
> 同一份代码在 Windows 上是 30s、在 macOS 上是 118s，两个数都要留。

---

## 2. 现状（实测，2026-09-20）

| 项 | 值 |
| --- | --- |
| Rust 总行数 | **42,795** |
| `.rs` 文件 | 177 |
| `#[test]` 函数 | 704 |
| `async fn` | 419 |
| `pub trait` | 8 |
| Go 代码（骨架前） | 0 |
| Go 工具链 | go1.26.4（`/opt/homebrew/bin/go`） |

逐 crate：

| crate | 行数 | 文件 | 角色 |
| --- | --- | --- | --- |
| `mcloud-core` | 10,521 | 34 | 纯计算：密码学、错误类型、`run` 四态、契约 |
| `mcloud-io` | 8,697 | 41 | **唯一**允许 IO 的层 |
| `mcloud-upstream` | 8,064 | 45 | 协议层：请求构造 + 响应解析 + 签名 |
| `mcloud-app` | 7,108 | 28 | 组装层 |
| `mcloud-cli` | 3,817 | 12 | 产物：命令行 |
| `mcloud-server` | 2,152 | 10 | 产物：HTTP + 静态资源 |
| `mcloud-capabilities` | 1,362 | 6 | 能力注册表 |
| `mcloud-exec` | 1,074 | 1 | 任务编排 |

复测命令（引用本表前先跑一遍）：

```bash
for c in mcloud-core mcloud-io mcloud-upstream mcloud-app mcloud-cli mcloud-capabilities mcloud-exec mcloud-server; do
  printf '%-22s %8s\n' "$c" "$(find crates/$c -name '*.rs' -exec cat {} + | wc -l | tr -d ' ')"
done
```

> ⚠ 评估报告 §1 的数字（40,001 行 / 170 文件 / 626 测试）取自更早的工作区，
> **已被本表取代**。

---

## 3. 冻结策略

### 3.1 为什么冻结而不是删除

`legacy/` 里那 704 个测试与 6 份黄金向量是这套代码**真正的资产**，而且与语言无关。
迁 Go 的过程中，"Go 侧算出来的和 Rust 侧不一样"是**必然会遇到**的情况 ——
那时要能确定**是哪一边错了**。删掉 Rust 侧，这个判断就没有基准了。

代价是仓库里有两套代码。但这个代价**有时限**：Go 侧逐块对齐之后，
`legacy/` 可以整目录删掉。

### 3.2 ⚠ 两个 `legacy` 不是一回事

| 路径 | 是什么 |
| --- | --- |
| `legacy/`（新增） | **冻结的 Rust 实现** —— 本项目自己的代码 |
| `test/fixtures/legacy/`（已有） | **旧项目 `mcloud-sign` 的导出件** —— 另一份代码的素材 |

同名不同物，**先看清路径再动手**。

### 3.3 ⚠ 冻结的隐藏成本：17 处 `include_str!` 会失效

`crates/` 移进 `legacy/` 之后，源码里所有 `include_str!` 的相对路径**多了一层**：

```rust
// crates/mcloud-core/src/auth.rs  （现在）
include_str!("../../../test/golden/auth-vectors.json")      // → 仓库根/test/...
// legacy/crates/mcloud-core/src/auth.rs  （冻结后，同样一行）
include_str!("../../../test/golden/auth-vectors.json")      // → legacy/test/...  ✗
```

**17 处**（复测用 `grep -rnE "include_str!" legacy/crates --include='*.rs'`），
`cargo test` 会直接编译不过。

**解法：`legacy/test -> ../test` 符号链接。** 于是源码**一字未改** ——
这对"参照物"这个身份很重要（改过的参照物，其可信度要重新论证）。

> Windows 上 git 会把符号链接检出成普通文本文件，`cargo test` 会失败。
> 替代方案是给那 17 处路径各加一个 `../`。详见 `legacy/README.md`。

### 3.4 执行步骤

```bash
bash scripts/freeze-rust.sh --dry-run   # 先看要做什么
bash scripts/freeze-rust.sh             # 真做
```

脚本会**拒绝**在"`crates/` / `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml` /
`.cargo/` 下有未提交改动"时执行 —— 因为 `git mv` 只搬**已跟踪**的文件，
未跟踪的新文件会被留在原地，目录被劈成两半，现象是「莫名其妙的重复定义」。

**前置条件：另一个会话已停手。** 冻结的那一刻，任何正在编辑 `crates/` 的
编辑器/会话，其文件路径会立刻失效。

### 3.5 已经为冻结做好的准备（冻结前就已生效，向后兼容）

| 位置 | 改了什么 | 为什么 |
| --- | --- | --- |
| `scripts/gates.sh` | 加 `CARGO_MANIFEST` 自动探测 | 写死路径会在冻结那一刻报「找不到 Cargo.toml」—— 那种错会让人先去怀疑代码 |
| `scripts/check-deps.sh` | 同上（用**字符串**不是数组） | bash 3.2 在 `set -u` 下展开空数组会报 unbound variable |
| `.gitignore` | 加 `/legacy/target/` | Cargo 的 target 会跟着 workspace 走 |
| `legacy/README.md` | 新增 | 冻结后的只读约定 |

---

## 4. 目录与包映射

```
go.mod                       module github.com/catlair/mcloud
cmd/
  mcloud/main.go             ← 产物：命令行（对应 mcloud-cli）
  mcloud-server/main.go      ← 产物：HTTP（对应 mcloud-server）
internal/
  core/                      ← mcloud-core          纯层
  upstream/                  ← mcloud-upstream      纯层
  capabilities/              ← mcloud-capabilities  纯层
  exec/                      ← mcloud-exec          纯层（包名 mcloudexec）
  io/                        ← mcloud-io            唯一 IO 层（包名 mcloudio）
  app/                       ← mcloud-app           组装根
  server/                    ← mcloud-server        组装根
  cli/                       ← mcloud-cli           产物
  golden/                    新增：定位 test/golden（**只供测试**）
legacy/                      ← 冻结的 Rust（见 §3）
test/golden/                 ← 不动：两套实现共享的唯一真相
```

### ⚠ 两个包名与目录名不同，是刻意的

| 目录 | 包名 | 为什么 |
| --- | --- | --- |
| `internal/io` | `mcloudio` | 标准库的 `io` 几乎每个文件都要用；重名会让每个 import 都得写别名 |
| `internal/exec` | `mcloudexec` | 同上，`os/exec` |

目录名与包名不同在 Go 里完全合法。**目录名保留成 `io` / `exec` 是为了与 Rust 的
crate 名一一对应** —— 迁移期间要能一眼找到"这一层原来是什么"。

### ⚠ `internal/golden` 只供测试

Go 的 `//go:embed` **不能跨目录**，而黄金向量是全仓共享资产。所以不复制向量，
只把"路径怎么算"封装掉。生产代码不得 import 它 —— `scripts/check-go-deps.sh`
会断言这条。

---

## 5. 门禁

### 5.1 八条，与 Rust 侧七条的对应关系

| # | Go 侧 | Rust 侧对应 |
| --- | --- | --- |
| 1 | `go build ./...` | `cargo build`（"能编译"的最直接判据） |
| 2 | `go vet ./...` | `cargo clippy --all-targets -- -D warnings`（**覆盖窄得多**，见 5.3） |
| 3 | `gofmt -l .` | `cargo fmt --check` |
| 4 | `go test -race ./...` | `cargo test --workspace`（**多一个 `-race`**，见 5.2） |
| 5 | `bash scripts/check-go-deps.sh` | `bash scripts/check-deps.sh` |
| 6 | `bash scripts/check-filenames.sh` | **同一个脚本，复用** |
| 7 | `node scripts/check-identifiers.cjs` | **同一个脚本，复用** |
| 8 | `node scripts/check-golden-vectors.cjs` | **同一个脚本，复用** |

入口：`bash scripts/gates-go.sh`（支持 `bash scripts/gates-go.sh test deps` 只跑指定几条）。

### 5.2 为什么 `test` 必须带 `-race`

Rust 里 `&mut R rng` 保证同一随机源不被并发共享 —— 那是**编译期保证**。
Go 的 `io.Reader` 是引用语义，并发用同一个源是 data race，而它**不一定**让测试
变红：可能只是偶发地产生错误数据（签名对不上、`deviceId` 错一位），
于是被当成"上游抽风"。

**`-race` 是那条编译期保证在 Go 侧的替代品，不是可选项。**

### 5.3 ⚠ `vet` 不等于 `clippy`

`go vet` 只覆盖"几乎肯定是错"的一小撮检查（printf 格式串、锁被复制、无用赋值…），
比 clippy 窄得多。补齐要引 `golangci-lint`，而它是**外部依赖** ——
骨架阶段刻意一个第三方依赖都不引。

**在那之前，不要把 `vet` 当成 `clippy` 用**：它过了，不代表静态检查这一关过了。

### 5.4 为什么后三条复用而不是重写

它们与语言无关（扫文件名 / 扫文本里的真实标识 / 独立复算向量）。
重写一遍等于给自己造**第二份真相** —— 而两份真相迟早会分叉，
分叉的那天没有人知道该信哪一份。

不过为了"同时管两边"，它们**已做了最小改动**（见 5.5）。

### 5.5 已改动的共享守卫

| 脚本 | 改动 | 不改会怎样 |
| --- | --- | --- |
| `check-identifiers.cjs` | `TEXT_EXT` 加 `.go` | **Go 文件完全绕过脱敏检查** —— 而"绕过"的表现是静默通过，不是报错 |
| `check-golden-vectors.cjs` | 消费者探测同时认 `golden.Load("…")`；扫 `crates/` + `legacy/crates/` + `internal/` + `cmd/` | 把 Go 侧真读了的向量标成**死向量** |
| `check-filenames.sh` | `*.go` 与 `*.rs` 同样豁免 kebab-case | Go 的 `_test.go` / `foo_linux.go` 会被误判（它们改名即失效） |

### 5.6 Go 侧依赖门禁（`check-go-deps.sh`）

与 `check-deps.sh` 一一对应：

| 断言 | 内容 |
| --- | --- |
| 纯层 | `core` / `upstream` / `capabilities` / `exec` 不得 import `net/http`、`database/sql`、`internal/io` |
| 组装根 | `app` / `server` **必须** import `internal/io` |
| 产物之间 | `cli` 不得依赖 `server` |
| 测试专用 | 生产代码不得依赖 `internal/golden` |

⚠ **这条在 Go 侧比 Rust 侧更必需**：Rust 里"纯层不许有 IO"是**编译期事实**
（`mcloud-core` 的 `Cargo.toml` 里根本没有 reqwest）；Go 的接口是**结构性实现**，
任何包都能满足 `core` 定义的契约 —— 所以这条约束**从编译期事实降级成 lint 约定**。
没有这个脚本，它就只是一句写在文档里的话。

用 `go list -deps`（**不加** `-test`）读生产依赖图，与 Rust 侧的
`cargo tree --edges normal` 语义一致。

---

## 6. 逐 crate 迁移要点

### 6.1 `mcloud-core`（10,521 行）—— 风险最低，先做

密码学几乎全部落在 Go 标准库：

| Rust | Go |
| --- | --- |
| `md-5` `sha1` `sha2` `hmac` `aes` `cbc` `des` `rsa` | `crypto/*` **全覆盖** |
| `argon2` | `golang.org/x/crypto/argon2` |
| `base64` `hex` `flate2` | `encoding/{base64,hex}`、`compress/gzip` |

⚠ 四个坑：

1. **`Random` 的注入要照搬**。smid 构造混用了 `randomUUID` 与 `Math.random`，
   不注入则夹具回放与黄金向量对拍**无法复现**。`below()` 的**无偏采样**
   （拒绝采样）要逐位对拍，不能用 `% n`。
2. **argon2 的盐由 `Random` 注入**（为了钉住"同盐同哈希"），**别用库自带的随机源**。
3. `crypto/des` 是弱算法，安全类 linter 会报 —— 要**写明豁免理由**（它是上游协议的
   既有事实，不是我们的选择），而不是关掉 linter。
4. `run` 的四态结果 / 三类失败 / 需人工，Rust 用 enum 钉住；Go 只能
   `const iota` + 约定，`switch` 漏一个态**不再有编译错误**。
   ⇒ 这是 §9 里最该补装 linter 的一条。

**退出判据**：6 份黄金向量在 Go 侧逐字段复算通过。

### 步 1 已实测出来的二十七条（写进这里，免得后面的模块重踩）


| 现象 | 根因 | 做法 |
| --- | --- | --- |
| PKCS7 去填充要**自己校验** | Go 标准库只给裸分组与模式，没有 `decrypt_padded_mut::<Pkcs7>` 的等价物 | 手写 unpad 并校验（末字节 ∈ 1..blockSize 且尾部全等于它）。**不判的后果不是"少一个检查"，而是用错密钥时静默产出半个凭据** |
| gzip 的 OS 字节 | Rust 侧要改 `buf[9]` 是因为 `flate2` 没暴露该字段；Go 的 `compress/gzip` 有公开的 `Header.OS`（默认 255） | 用 `Header.OS = 19`，**判据不变**：输出第 10 个字节 == 19 |
| 取 error 类别用 `errors.As` | Go 的惯例是 `fmt.Errorf("…: %w", err)` 包一层（Rust 的 `?` 不会），裸类型断言在那种情况下**静默返回 ok=false** | `errors.As`；并专门写一条"包一层之后还取得出来"的测试 |
| **`json.Marshal` 会转义 HTML** | Go 默认把 `<` `>` `&` 转成 `\u003c` 等，`serde_json` 不转。而占位符就是 `<REDACTED>` | 一律走自写的 `marshalJSON`，内部 `Encoder.SetEscapeHTML(false)`。**这条的隐蔽性在于夹具解析回来完全正确**，只有人眼与按文本扫描的守卫看得见 |
| **默认解 JSON 数字会丢精度** | 默认给 `float64`（53 位有效位），19 位标识末几位**静默归零**，且重映射出的合成值同样"看起来是随机的" | 解析一律 `Decoder.UseNumber()`，数字保持字面量；写入时也写回 `json.Number` 而不是字符串 |
| **`Encoder.Encode` 补尾换行** | `serde_json::to_vec` 不补 | `marshalJSON` 里 `bytes.TrimSuffix(…, "\n")` |
| **`time.Date` 不报错、会归一化** | 夏令时"跳过"的那一小时里没有那个本地时刻，Rust 的 `.earliest()` 给 `None`，Go 的 `time.Date` 把它挪到邻点（02:30 → 01:30 或 03:30） | 构造后**把墙钟读回来逐字段比对**，不一致就 `continue`。不校验 = 凭空多跑一次，且跑在用户没写的那个时刻上 |
| **无符号自增会回绕** | `value += step`（`uint32`）：`59 + 4294967295` 回绕成 58。Rust debug 构建直接 panic，**Go 无溢出 panic，只会静默回绕**；位图去重让**结果**恰好相同，所以它在结果上不可见 | 先判"再加一步就超上界"再自增。判据只能靠**循环次数**，不能靠结果 |
| **`x/crypto/argon2` 不暴露 version，也没有 argon2d** | Go 侧 `IDKey` 把版本**写死在 `initHash` 里**（`Version = 0x13`），`argon2d` 是私有 iota、无导出实现。于是 PHC 串里的 `v=16` 与 `$argon2d$` 在 Go 侧**物理上算不出来**，而 Rust 侧两者都返回 mismatch | 保守拒绝（归 `Internal`），并把差异登记进向量的 `$crossLanguageGaps`。⚠ **绝不能"用 v=19 顶替"**：那样 `$argon2id$v=16$…$<v19 生成的哈希>` 会被算成**匹配** —— 等于"把版本字段改掉反而能过" |
| **`argon2.IDKey` 对 `t<1`/`p<1` 直接 panic，`m` 过小则静默抬升** | `deriveKey` 头两行是 `panic("…too small"/"…too low")`；而 `memory` 过小会被抬到 `2*4*threads` **再算**。哈希串是可能被写坏的，panic 在服务里就是一次请求打崩进程 | 解析器**自己**挡住三条下限（实测 `m≥8`、`t≥1`、`p≥1`，不是推的）。⚠ `m` 那条更阴：它让"格式非法"表现成"口令不对"，故障从此查不出来 |
| **PHC 的参数是可选、有默认值的** | 缺 `p=1` 时 RustCrypto 用默认值 1 补上，校验结果仍是**匹配** | **别加"参数必须齐全"这道校验** —— 那会把一条本来能验过的存量哈希判成"格式非法"，而现象只是"老用户登录不上" |
| **PHC 的段是按位置取的，不是按内容** | `$argon2id$v=19$$<盐>$<哈希>` 里那个**空段会被当成盐**（于是报"盐太短"），不是"空参数表"；`$…$abc$def` 同理 | 判据：参数段必须非空且**每项含 `=`**；base64 无填充，所以盐段不可能含 `=`，不会误判。**别按"看起来像参数"去猜** |
| **打印脱敏在 Go 里要多挡两条路** | Rust 只有 `{:?}` 一条；Go 有 `%v`（走 Stringer）、`%+v`（**展开字段，不走 struct 的 Stringer**）、`%#v`（走 GoStringer） | 把敏感字段做成**有 Stringer 的具名类型**（`PasswordHash`）—— struct 级 Stringer 挡不住 `%+v`。⚠ 副作用：断言失败信息里也只印 `<已隐藏>`，比较处要显式 `string(v)` |
| **⚠⚠ Rust 的 serde 枚举序列化成**文本**，Go 的 `type X int` 序列化成**数字** | 前者带 `#[serde(rename_all = …)]`，输出 `{"outcome":"manual_required","level":"warn","role":"admin"}`；后者默认输出 `{"outcome":2,"level":1,"role":0}`。两边读**同一批** `run_events.payload`、同一张 `users.role` 列 ⇒ 这是**外部契约换了个形状**，而它既不编译错也不运行时错，只在真跑起来之后表现为"另一侧读出来的东西不对" | 凡是会进 JSON 的枚举都必须**显式**实现 `MarshalJSON`/`UnmarshalJSON`，收在 `internal/core/enum.go` 一张名字表里（下标即枚举值，所以顺序必须与常量声明一致）。⚠ 漏写的三条后果都不在编译期：`SelectXxx` 这类测试照旧全绿 |
| **serde 对未知变体 / `null` / 数字一律拒收** | serde 没有"宽松枚举"这回事，三个都失败。而 Go 侧若没有 `UnmarshalJSON`，`2` 会被当成合法值读进来 —— 那正是它自己写出来的形状 | 解码侧走同一张表，三者都报错。⚠ `null` 要**单独挡**：`encoding/json` 把 null 解进 `string` 是**静默成功**（留空串、不报错），不挡的话它会走到"未知的文本 \"\"" 那条报错，行为对但信息指错方向 |
| **15** ⭐ | **Rust 的 serde **结构体字段名**默认 snake_case，Go 不带 tag 输出**字段名原样** | `password_hash` vs `PasswordHash`、`cloud_count` vs `CloudCount`。与 #13 同一个病（"进 JSON 的东西两边默认形状不同"），只是落在字段名上。**已犯三次**（`CloudStats` / `User`+`Session` / `model` 那批），三次都是靠"改坏了却不响"发现的 | 每个字段显式写 `json:"…"`，并由 `scripts/check-go-jsontags.cjs`（第九条门禁）兜住。⚠ **往返测试救不了** —— 它用什么字段名都是绿的，只证明"解得回来"，不证明"同名" |
| **16** ⭐⭐ | **时间列的写法两端不同形，而这一列在 SQL 里是字符串**：chrono 的 `to_rfc3339()` 给 `+00:00` + AutoSi（0/3/6/9 位小数），Go 的 `time.RFC3339Nano` 给 `Z` + **去掉末尾的 0** | 后果不止"看着不一样"：`sessions.expires_at` / `users.created_at` / `runs.started_at` 都在 SQL 里做**字符串比较与排序**（`DELETE FROM sessions WHERE expires_at <= ?1`、`ORDER BY created_at ASC, id ASC`、`WHERE started_at < ?`）⇒ 同一列混两种写法时，`+`(0x2B) < `Z`(0x5A) 让**同一时刻的 Rust 行永远排在 Go 行之前**：次级排序键（`id`）被抢先、`expires_at <= ?` 在边界那一格对两种来源的行给出不同答案，而且**一个错都不报** | Go 侧写 `formatTime` **逐字节复刻 chrono**（`internal/io/datetime.go`），**所有**写出点都走它（含 `time.Now()` 那种顺手 `Format` 的）。规则由第十份向量 `test/golden/datetime-format-vectors.json` 钉（**chrono 0.4.45 真跑**产出：17 条写出 + 19 条读取）。⚠ 读取侧 Go **严格更严** —— 实测四种 chrono 收而 Go 拒（空格代替 `T`、小写 `t`/`z`、闰秒 `12:00:60`），方向安全且这四种 chrono 自己从不写出来，逐条登记在 `chronoAcceptsGoRejects` 里**待复查** |
| **17** | **两个变体的枚举容易顺手写 `if/else`，从而破坏漏态守卫** | `if k == B { return "b" }; return "a"` 会让**任何**越界值都有名字 ⇒ "已知态有几个"再也数不出来 ⇒ 真值表守卫**静默失效** | `String()` 一律 `switch` + 末尾 `return ""`。这个错是 `countKnownStates` 抓出来的（`CapabilityKind` / `AccountMode` 都踩了），`Role` / `SessionKind` 一并改齐 |
| **18** ⭐ | **tag 枚举（`#[serde(tag = "…")]`）的不同变体，字段顺序可能不同** | `RunStage::Done` 是 `stage→outcome→manual_steps→summary`，`RunStage::Failed` 是 `stage→summary→manual_steps`。Go 没有 sum type，用一个结构体 + Kind 近似时**只能按其中一种顺序输出** ⇒ 另一种必然与 serde 不同 | 按 Kind **分派** `MarshalJSON`（每个变体一个匿名结构体）。⚠ **逐字段比对会把这个差异整个放过** —— 它只证明"字段都在"，不证明"顺序也一样"；只有整串比对抓得到 |
| **19** ⭐ | **测试夹具的 RNG 必须逐模块对齐，不能就近复用** | Rust 侧**每个模块各自定义** `SeededRng`，算法不同：`run.rs` 那份是 `s*31+7` 的 **u8 LCG，周期恰好 16 —— 正是 UUID 的字节数** ⇒ 连取两次拿到**同一个 id**；`stage.rs` 那份是 splitmix64，还专门用进程内计数器把种子岔开，并在注释里写明了原因 | 夹具 RNG 逐个模块照抄 Rust 侧那一份（含 `rng()` 的计数器语义）。⚠ 这条我踩了：**就近**复用了 `run_test.go` 那份，而 `runRng` 的注释里本来就写着"别混用" |
| **20** ⭐ | **浮点转整型：Rust 的 `as` 是 saturating 且定义明确，Go 是**实现相关**的** | `n as usize` 对 NaN→0、Inf→usize::MAX，靠它把极端输入挡在 `idx <= len` 之外；而 Go 规范说浮点转整型**超出范围时行为由实现决定** ⇒ 可能拿到一个碰巧合法的数 | 显式 `math.IsNaN` / `math.IsInf` 挡掉再转。⚠ 判小数部分要写 `n - math.Trunc(n) != 0`，别写 `n != math.Trunc(n)` —— 后者对 Inf 是 `false`（不排除），而 Rust 的 `fract()` 对 Inf 给 NaN（排除） |
| **21** ⭐ | **Go 的 map 遍历顺序是随机的**，Rust 的 `BTreeMap` 不是 | 凡是"取第一个满足条件者"的逻辑（第一个冲突、第一个匹配）在 Go 侧**每次运行都可能不同** —— 而报错内容是要进报告给人看的 | 显式排序后再取第一个。⚠ 这条断言**一开始没承重**：只造一组冲突时排不排序报的都是它（反向测试照样绿），必须**两组以上**才问得出"顺序"这件事 |
| **22** | **nil slice 与空 slice 的差别，在反序列化侧同样存在** | Rust 的 `#[serde(default)] Vec<T>` 缺失时是 `vec![]`（序列化成 `[]`），Go 是 nil（序列化成 `null`）。这个病在 marshal 侧已经处理过（`RunReport.ManualSteps`），但**unmarshal 侧要单独再归一一次** | 给含 slice 的类型写 `UnmarshalJSON` 归一化（`type plain X` 绕开递归）。⚠ 只有整串比对抓得到 —— 逐字段断言里 `len(nil)` 与 `len([]T{})` 都是 0 |
| **23** ⭐ | **`Vec<u8>` 在 serde 里是**数字数组**，Go 的 `[]byte` 是 **base64 串** | `{"body":[123,34,...]}` vs `{"body":"eyJhIjox"}`。`encoding/json` 对 `[]byte` 有**专门的** base64 分支 ⇒ 两侧都跑得通、都解析得回来，只是**写出来的文件不是同一份**。而 `HttpRequest` 的注释写着"golden request 对拍依赖它逐字段可比" —— 对拍时两边各写一份文件、逐字节 diff 才发现不一样，单侧跑**永远是绿的** | `Body` 类型显式实现编解码，**只认数字数组**。⚠ 解码要解进 `[]int` 而不是 `[]byte`：后者会**静默接受** base64，而那正是分歧点本身 |
| **24** ⭐ | **同一个 `null` 在两种字段上结果**相反** | `#[serde(default)] Vec<T>`：缺席补 `[]`，但**显式 null 是失败**（`default` 只管**缺席**，管不到 null）；`Option<T>`：null 就是 None，**收**。实测：`HttpRequest.body:null` 收、`headers:null` 拒、`HttpResponse.body:null` 拒 —— 三种 | 必填字段（`method` / `url` / `status`）用**指针**：缺席与 null 都是 nil，都拒；Vec 字段用 `json.RawMessage` + `rejectNullVec`。⚠ 这是"null 在 Go 侧**静默成功**"的**第三个**实例（前两个：解进 `string` 留空串 —— 见 `enumJSON.unmarshal`；解进指针留 nil） |
| **25** | **`UPPERCASE` 是第三族命名规则，且**同样约束解码** | 此前两族是 `snake_case`（`partial_success`）与 `lowercase`（`admin`）；`HttpMethod` 是第三族。实测 `"get"` / `"Get"` **都拒**，只收 `"GET"` —— 按"HTTP 方法大小写不敏感"的常识顺手写就分叉 | 同一张表（`enum.go`），文本照向量逐字抄。⚠ 别因为"方法名大写只是显示习惯"就放松解码侧 |
| **26** ⭐ | **Rust 的 `&T` 非空是**类型保证**，Go 的指针可空 | `Scope::User(&UserId)` 编译器不允许"User 但没有用户"这种态；Go 的 `ScopeUser(nil)` 会**静默退化成 `ScopeLocal`（不过滤）** ⇒ 一次失败的查用户直接变成"把所有人的数据列出来"。这是本项目里唯一一处"指针可空"会放大成**越权**的地方 | `ScopeUser` 对 nil 直接 panic（不返回 error —— 它是写入侧的编程错误，不是运行态可恢复的失败）。`Scope.user` 不导出，逼调用点写 `ScopeLocal()` |
| **27** | **Go 的数组解码"元素不足补零值"，而 serde 的元组长度是**类型的一部分** | `Vec<(String, String)>` 给 `[["x-a"]]`（1 元组）：serde 拒；Go 的 `[2]string` 静默解成 `["x-a", ""]` —— **凭空多出一个空头值**，而它在上游眼里是"带了一个空 header" | 先解进 `[]string` 再判 `len == 2`。⚠ 对**切片** Go 会报长度错，对**数组**不会 —— 差别就在 `[2]T` 与 `[]T` |

### ⭐⭐ 第九份向量的做法变了：**path 依赖真实的 mcloud-core，不镜像类型定义**

前八份向量需要"参考实现的形状"时，做法是在生成器里**镜像**一份类型定义再让 serde
序列化（`gen-enum-json-vectors.rs` 就是这么写的）。那份镜像抄错过一次：
`ScheduleTarget` 漏抄了 `#[serde(tag = "kind")]`，产出的是 serde 默认的外部标签
—— 与库里已存的形状完全不同，而**类型系统抓不到**，只有整串比对抓到了。

`legacy` 这一批有 14 个类型、七八十个字段，镜像的抄错概率只会更高。
所以 `scripts/gen-legacy-shape-vectors.rs` 改成 **path 依赖真实的
`legacy/crates/mcloud-core`**（`legacy/` 是只读的对拍基准，依赖它只是读）：

- 用的是冻结那一刻的类型定义**本身**，没有第二份可以与之分叉的副本；
- 而"全默认实例"靠 `serde_json::from_value::<T>(json!({}))` 得到 ——
  这批类型大多没 derive `Default`，但所有字段都带 `#[serde(default)]`。

⇒ **判据：需要"参考实现说形状是什么"时，先问能不能直接依赖它；
只有在依赖不上（crate 已删 / 依赖拉不全）时才镜像，且镜像必须配整串比对。**

### 步 4 已实测出来的九条（接在步 1 那二十七条之后）

⚠ 编号是**全局连续**的：步 1 占了 1–27，这里从 28 起。
两个人在同一份文档上加条目时最容易犯的就是各编各的 —— 一旦出现
两组 `#20`，引用"见 §6.1 #20"就变成歧义，而**歧义恰恰出现在这张表最该被
引用的场合**（有人正要踩坑、来查条目）。改步 1 的条目数时记得同步这里。

| 现象 | 根因 | 做法 |
| --- | --- | --- |
| **28** | **Rust 的 `&'static` / `LazyLock` 只读引用，Go 的切片与指针**都可写** | `REGISTRY` 取出来只能读；Go 若把底层切片或指针交出去，任何调用方都能改写"签到这个能力的声明"，而下一个读到它的能力**不报错**，只是拿到被改过的那一份 —— 注册表是两个产物**共用的唯一契约来源**，被改了就是两个产物同时静默错 | `capabilities.All()` 返回**副本**（`Get` 返回 `Capability` 值而非指针）。⚠ 反向测试已确认：改成 `return registry` 之后 `TestAllCannotBeUsedToMutateTheRegistry` 立刻变红 |
| **29** ⭐ | **Go 的 map 遍历顺序是**故意随机化**的，Rust 的 `HashMap` 只是"不保证" | `params_digest` 必须先按 key 排序：不排序则"同一组参数"算出两个摘要 ⇒ 互斥失效（同一账号能并发跑两次）。Rust 侧这条**碰巧**稳定了一段时间，Go 侧连这个缓冲都没有 —— **同一进程里两次调用就不一样** | 显式 `sort.Strings`。⚠ 断言不能照搬 Rust 那条"换个顺序再算一次"（Go 里根本构造不出"顺序"），要改成**连算 50 次**；反向测试（把 `sort.Strings` 换成不排序但仍引用 `sort` 的写法）确认它会变红 |
| **30** | **外部契约的键**不一定**是 snake_case** | `mcloud-app::payload` 的响应体是 Rust 侧用 `json!({...})` **手工**拼的（`runInBatch` / `needsConfirmation` / `defaultAccountMode`），不走 derive ⇒ 它是这套代码里少数几个"契约不是 snake_case"的地方。按"Go 的 struct 默认 snake_case"顺手推字段名，会得到 `run_in_batch`，而后果不是报错，是**前端拿到 `undefined`** | payload 结构体显式写 camelCase tag，并由一条**按文本扫描**的断言守住（`TestPayloadKeysAreCamelCase`）。⚠ **往返测试查不出它**：用什么键名都是绿的 |
| **31** | **Rust 的 `impl Default` → Go 没有对应物，零值是 nil map** | `RunRegistry::default()` 天然可用；Go 的 `var reg RunRegistry` 两个 map 都是 nil，**读**可以、**写直接 panic**。而"能用零值"的错觉会让构造函数被绕过 | 一律走 `NewRunRegistry()` / `NewEventBus()`，并在文档里写明"零值不可用"。测试只断言"读不 panic"，不假装"零值也能用" |
| **32** | **"同一条规则两份实现"会发生在 JSON 编解码上** | 产物层要拼对外 payload，于是每个包都想写一遍 `json.NewEncoder` + `SetEscapeHTML(false)`；而 `core` 里那份（`marshalJSON`）是**不导出**的 | 导出 `core.MarshalJSON` / `core.ParseJSON`。⚠ 这不是"方便"，是§5.4 那条原则的正反面：**复用而不是重写**，重写就等于给自己造第二份真相 |
| **33** | **枚举名容易在产物层再抄一遍字面量** | Rust 的 `payload.rs` 里四个 `*_name` 常量函数（为了 `const fn`）把枚举名又写了一遍；Go 侧若照抄，`RiskLevel` 多一档时这里会**默默输出空串**，而空串在前端就是"没有风险等级" | 一律取 `core` 的 `String()`，并由 `TestEnumNamesComeFromCoreNotFromADuplicateTable` 逐个能力比对（**空串也算失败**） |
| **34** ⭐ | **`core.ParseJSON` 为了"19 位标识不丢精度"返回 `json.Number`，不是 `float64`** | 于是 `body["ifOpenAccount"] != float64(1)` 在**值就是 1** 的时候也是"不相等"，报错长成 `应为 1: 1` —— 看上去像断言写错，其实是类型不对。这条只在"断言请求体里的数字"时才碰到，而那正是**对拍协议形状**时最常做的事 | 断言数字一律**比文本**（`numText`）。⚠ 已实测：第一次跑红的就是这条，报错是"应为 1: **1**" |
| **35** | **`core.StateStore` 有 27 个方法，测试里手写一遍会淹没重点** | 登录落库只用到其中 2 个。Rust 侧 trait 可以给默认实现，Go 的接口**必须全实现** —— 于是要么写 25 行 `panic("未使用")`，要么用 `nil` 接口内嵌 | 用**内嵌接口**做部分假实现（`type recordingStore struct{ core.StateStore; … }`）：没重写的方法一旦被调用就 panic ⇒ "登录这条路径碰了别的存储入口"会**当场响**，而不是静默返回零值。⚠ 这不是偷懒，是把"不该发生的事"变成会响的事 |
| **36** | **夹具里 `data` 是**字符串**，不是嵌套对象** | Rust 的 `json!({… "data": v.to_string()})` 里 `to_string()` 会把对象变成**带引号的字符串**（真实链路里它是 AES 密文，本来就是字符串）。Go 没有 `json!` 宏，顺手写成一个嵌套对象，解密器那一步就被**整个跳过**（它收不到 string，判定降级成别的分支） | 夹具显式**序列化两次**（`quotedJSON` = `mustJSON(mustJSON(v))`）。⚠ 反向测试已确认：改成一次序列化后 `TestQrLoginWalksNotScannedScannedSuccess` 与 `TestSmsLoginDecryptsUserInfoOnSuccess` 同时变红，且报错明确（"登录成功但 data 不是密文字符串"） |
| **37** | **Rust 用嵌套模块表达"这一组常量属于哪一端"，Go 没有嵌套模块** | `pub mod ios { … }` 那十来个常量（`CLIENT_TYPE` / `CPID` / `AUTH_PREFIX` …）在 Rust 里带着 `ios::` 前缀，和 PC 端同名含义的常量**天然不撞**。Go 只能平铺；若顺手写成 `ClientType` / `AuthPrefix`，就会与 `PcPlatform.ClientType()` 那一族混用 —— 而混用的后果是"被上游认成另一个客户端"，**不报错** | 一律带端前缀（`IOSChannel` / `IOSClientType` / `IOSAuthPrefix`），并在注释里写明"这不是装饰"。⚠ 反向测试已确认：把 `IOSAuthPrefix` 改成 `pc` 之后四条断言同时变红（含"与 capabilities 那侧拼出来的必须一致"那条） |
| **38** ⭐⭐ | **同一个作用域里 `err` 的**类型**由第一次 `:=` 定下来** | `core.MarshalJSON` 返回 `error`（接口），`ParseEnvelope` 返回 `*core.Error`（具体类型）。写成 `buf, err := core.MarshalJSON(…)` 再 `env, err := ParseEnvelope(…)` —— 第二个 `:=` 只是**赋值**不重新声明，于是 nil 的 `*core.Error` 被塞进 `error` 接口 ⇒ `err != nil` **恒为真**，而 `%v` 打印出来是 `<nil>`。报错信息里一个线索都没有，编译期也不报 | 返回 `*core.Error` 的函数**一律换变量名**（`perr`）。⚠ 判据：一个函数里同时出现 `error`-返回与 `*core.Error`-返回的调用时，第二个起就不要再叫 `err`。已实测：这个坑是我自己在测试里踩出来的（六条向量用例集体报"解析信封失败: \<nil\>"） |
| **39** | **HTTP 头在 Go 侧只能追加，Rust 侧可以 `retain` 后覆盖** | Rust 的 `headers.retain(…)` 直接删掉同名头；Go 的 `WithHeader` **只 append**。于是"覆盖 content-type"在 Go 里默认做不到，而多发一个 `Content-Type` 的表现是**上游按第一个解析** —— 请求看起来完全正常，只有 charset 那部分被丢掉，于是带中文的 body 在某一天开始乱码 | 覆盖必须 **`dropHeader` + `WithHeader`** 两步（`internal/upstream/app_signin.go`）。⚠ 反向测试已确认：去掉 drop 那一步，`content-type 应恰好一个` 立刻变红（实得 2） |
| **40** | **"可能是数字也可能是字符串"的字段，Rust 靠 `Value::as_i64()` 一把梭，Go 没有对应的通用取值器** | `oid` / `total` / `curMonth` 这类字段在 Rust 里一行 `as_i64()` / `as_bool()` 就拿到（内部已经判了形态）。Go 侧必须**显式写 switch**，而数字在这里是 `json.Number`（**字符串**形态的字面量，见 #34）—— 漏掉这一分支的表现是"值恒为 `nil`"，**不报错** | 一律走 `asInt64` / `optIDField` / `truthyField` 这类显式 switch 的 helper，且**每种形态一条分支**。⚠ 反向测试已确认：删掉 `optIDField` 的 `json.Number` 分支后 `TestPrizeRecordOidAcceptsBothNumberAndString` 变红 —— 而这条路径挂了不会有任何报错，只是 id 为空 |
| **41** ⭐⭐ | **注释里写的"典型值"不等于夹具里的值** | `SignInResult.TodaySignIn` 的注释逐字写着"`false` 恰恰表示这次把今天签上了"（这句话本身是对的），于是照着它写断言 `if got.TodaySignIn { t.Error(...) }`。而 `app-signin-vectors.json` 里那条实拍的 `todaySignIn` 是 **`true`**（账号当天早些时候已经签过）⇒ 断言当场反。**反了之后最容易做的错事是去改注释或改实现** —— 那会把一条对的语义删掉 | 判据：**期望值一律从黄金向量里读，不从注释里推**（与"期望值必须跑出来而非推出来"同源）。⚠ 这次的修法是改**断言**（改为"应原样交回 true"），并顺势把"发送层不代劳回查 infoV3"补成一条真断言（多发的那一个请求会让 `len(requests)` 变 2 并变红） |

### 步 3（io）已实测出来的两条（编号接在最后）

⚠ 编号按**发现的先后**排，与步号无关 —— 这张表是**追加式**的，
重编别人的条目会让正在引用它的人指向错处。
当前：步 1 = 1–27，步 4 = 28–39，步 3 = 40–41。

| 现象 | 根因 | 做法 |
| --- | --- | --- |
| **40** | **Go 的原始字符串用反引号作定界符，装不下 SQL 里的反引号** | 逐字照抄 `schema.rs` 的 SQL 时，注释里的 SQLite 标识符引用（`sha256` / `raw` / `state`）会把字面量**当场截断**。编译器报的是一堆指不到源头的错 —— 没有一条线索指向"注释里有个反引号"。Rust 的 `r#"…"#` 允许内含反引号，所以原文件里那些注释是合法的 | 照抄 SQL 密集的文件时，注释里的反引号**全部去掉**（或改用解释字符串）。⚠ 本仓库只有 SQL 这么写，风险面因此有限，但也因此**只在迁 SQL 时才遇到** |
| **41** ⭐ | **"两侧都跑得通"不等于"读的是同一批数据"** | io 是唯一真正落盘的一层：`+00:00` 与 `Z` 各自都能被对方解析，于是"读老库没问题"这句话是对的，却掩盖了"同一列里会有两种写法"—— 而 SQL 把它们当**字符串**比大小。这是 #13/#15（枚举、字段名的形状）在**时间列**上的第三个面：形状差异不报错，只在**比较与排序**时才显形 | 见 #16：写出侧逐字节复刻参考实现，读取侧的宽严方向**显式钉住**（只允许"更严"，且更严的每一条都要登记理由）。⚠ 判据：**凡是要被 SQL 比较或排序的列，格式差异就是功能差异，不是风格差异。** |

⚠ #13/#14 是**同一个病**的两个面（枚举 JSON），所以判据也共用一条：
**"有没有 MarshalJSON"这件事必须能被一条断言问出来** —— `enum_test.go` 里的
第一条断言是"序列化结果的首字符必须是 `\"`"，它专门用来接住"漏写了 MarshalJSON"
这种改动（反向测试已确认：拆掉 `RunOutcome.MarshalJSON` 会让整串比对立刻变红）。

⚠⚠ **反向测试不只是"验证断言能变红"，它还能发现"修法绕开了陷阱"**：
`TestEventsFromAnotherRunAreRejected` 第一次跑红时（两个 id 撞了），
我的第一版修法是**改成同一个 rng 连取两次** —— 那样也能拿到不同的 id，
但它**绕开**了 Rust 侧那条陷阱而不是守住它，于是"有人把计数器改回固定种子"
这种改动从此不会再被发现。是反向测试（把步进改成 0）仍然**不变红**
暴露了这件事：那条断言其实**没有承重**。
⇒ **修一处失败的测试时，先问"我的修法是守住了陷阱，还是绕开了它"**，
然后用反向测试确认。绕开的修法特征很明确：断言对"陷阱被重新引入"不再敏感。

⚠ 另有一条与语言无关、但 Go 侧更容易犯的：**黄金向量门禁靠正则
`golden.Load("…")` 的字面量判断"这份向量有没有人读"**。把文件名藏进
`loadGolden(t, name)` 这类包装函数会让探测**静默失配**，把读得好好的向量
标成"死向量" —— 那种失真比漏检更坏，它教人忽略报告。所以各包的取向量函数
必须把字面量写进 `golden.Load` 调用里。

⚠ **跨语言对拍必须"生成侧 ≠ 消费侧"**（迁移时新增的第七份向量就是照这条做的）：
用 Go 生成、再用 Go 验证是**自证** —— 两边一起错的时候照样是绿的。
所以 `test/golden/password-hash-vectors.json` 由 **Rust 侧**产出
（`scripts/gen-password-hash-vectors.rs`，RustCrypto argon2 0.5.3，
即迁移前一直在跑的那一份实现），Go 侧只是它的消费者。

第八份向量 `enum-json-vectors.json` 同理：由 **serde 1.0 / serde_json 1.0** 产出
（`scripts/gen-enum-json-vectors.rs`），Go 侧只是它的消费者。它比第七份更能说明
这条原则 —— JSON 文本形状这件事**在 Rust 侧原本一个断言都没有**（`run.rs` 里没有
serde 测试、也没有 `tests/` 目录），形状只隐含在 `rename_all` 属性里。
⇒ 若照"从实现反推"的老路写 Go 断言，就是把 serde 的命名规则再推一遍，
而推错时两边会**一起错、一起绿**。

同因：**期望值必须"跑出来"，不能"推出来"**。那份生成器对每条用例都真的跑一遍
`verify_password`，实测结果与写下的 `expect` 不一致就 panic。这个自检在本次开发中
**推翻了四次预判**（算法名被改是 mismatch 还是 error、盐的下限是 4 还是 8、
缺一个参数的后果、成本参数的下限）—— 若直接把那四条写成 Go 断言，
它们的表现是"Go 实现错了"，而实际是期望值错了，排查成本极高。

⚠ **`cron` 与 `sanitize` 没有黄金向量** —— 它们的规格来自 Rust 侧的单元测试
（`sanitize` 拆成 5 个文件、348 行）。迁移这两块时是**逐条平移测试**，
不是照文档重写；新增的断言只针对"换语言新长出来的坑"，不重新定义语义。

⚠ **注意其中一份是"死向量"**：`test/golden/login-crypto-vectors.json` 有生成器
（`scripts/gen-login-crypto-vectors.cjs`）、但**没有任何测试读它**（`gates-go.sh` 的 golden
门禁自己会把它标成 `⚠ 无任何测试读取（死向量）`，**不失败**）。它覆盖
`aesEcbEncrypt/Decrypt`、`sha1Password`、`md5SafeVerify`、`puzzleAnswer`、`guid32`。
⇒ 这一份在 Go 侧**不是"照搬已有断言"，而是"第一次真正接上断言"**：Rust 侧这些函数
目前**处于无断言状态**（是"本来就没测"，不是"测过又丢了"）。另外它的 `§guid32`
（uuid → md5 的具体拼接）**向量里没给出规则**，门禁会明确跳过它 ——
所以"接上消费者"只解决一半，剩下那半得回原始出处补。

### 6.2 `mcloud-upstream`（8,064 行）—— 向量兜底最厚

⚠ **本层的铁律是"跨端常量不许互搬"**（这条在 Rust 侧已经踩过两次）：

PC 与 App 是**两套客户端**，同名同形的常量值可能不同，而"两边实测都能过"
只说明服务端在这条路径上不校验，**不等于可以互换** —— 错了是**静默失效**，不报错。

已确认按「谁发的」×「哪个环境」分的常量（**Go 侧的常量表必须保留这两个维度，
不许扁平化**）：

| 位置 | 取值 |
| --- | --- |
| `Authorization` 第 1 段 | PC `pc` / App `mobile` |
| 业务签名密钥 | PC `6lXUhuJGkkAr5HXr` / App `BOFlEUINUdSugD6u` |
| 签名 `version` | PC 写死 `1.1` / App **必须用请求头里那个** |
| `hcy-cool-flag` 整包信封钥匙 | 原生 AES-192 / H5 默认 AES-256 / H5 非 `dev2Prod` AES-128 |

**判据（一句话）：这个码能在同一端的两个接口上取不同值吗？能，就不许钉进端档案。**
（`toSourceId` 就是这种 —— 同一个 PC 文件里有三个值。）

⚠ 另一个坑：`serde_json::Value` 的手取字段（`require_str` / `require_i64` /
`ensure_ok`）在 Go 里是 `map[string]any` 的链式取值，**漏判直接 panic**，
而 Rust 侧漏判是编译错误。
⇒ 必须先把这几个 helper 写成一等 API 并配对拍测试，**禁止在业务里裸取**。

**退出判据**：同一条抓包请求 Rust/Go **双跑**，header 顺序 / percent-encode /
签名串逐字节一致。

### 6.3 `mcloud-io`（8,697 行）—— 唯一 IO 层

| Rust | Go | 风险 |
| --- | --- | --- |
| `reqwest`（rustls + gzip/brotli/deflate + cookies） | `net/http` | 中 |
| `rusqlite`（`bundled`） | `modernc.org/sqlite`（纯 Go，免 cgo） | 中 |
| `rand`（`OsRng`） | `crypto/rand` | 低 |

⚠ 三个坑：

1. **Go 的 `net/http` 只自动解 gzip**，brotli 要自己接 transport。
2. **换 sqlite 驱动后要专门验一次 schema v1→v7 迁移**，以及"老库只读导入结果一致"。
   `chrono` 列类型要改成 `time.Time`。
3. **TLS 指纹**：rustls 与 Go `crypto/tls` 的握手行为不同（扩展顺序、JA3 形状）。
   若上游有指纹风控，这里会**静默**失败。⇒ 迁移后要拿真实流量验一次。

**退出判据**：纯 Go 驱动下 schema v1→v7 迁移 + 老库只读导入结果一致。

### 6.4 `mcloud-capabilities` / `mcloud-exec`

- `capabilities`（1,362 行）：能力注册表，CLI 与 WebUI 的**唯一契约来源**。
  两个产物各自描述一遍同一批能力，就会出现"CLI 有、WebUI 没有"这种只有人肉比对
  才发现的漂移 —— 所以它是**唯一**来源这条要保持。
- `exec`（1,074 行）：事件模型 / 生命周期 / 事件总线 / `run_events` 表。
  ⚠ 只有 1 个文件、1,074 行，但它是**编排的中枢**，别按行数低估。

### 6.5 `mcloud-app`（7,108 行）—— 组装层

签到全链路编排、登录驱动、账号档案、迁移策略。

⚠ 两条已经踩过的坑，迁 Go 时**要原样带过去**：

1. **`Authorization` 整串存、整串用**。形状是
   `Basic base64("<前缀>:<手机号>:<authToken>")`，登录时一次拿到整串。
   **别再"拆开存、用的时候拼回去"** —— 本地合成的串一律不许落库，
   理由不是洁癖：读侧优先采信整串，而整串路径**没有前缀可校验**，
   一串拼错的会被永久当原文用，现象只是"忽然回 `90001`"。
2. **成败只看回查 `infoV3`**。`startSignIn` 的 `todaySignIn` 是**调用前**快照；
   `code == 0` 也不够（"今天已签"同样回 `success`）。

### 6.6 `mcloud-cli`（3,817 行）

`clap` derive → `cobra` + `pflag`。

- ⚠ **零参数 + 下标**（用户明确要求）：不带子命令直接跑 = 对**全部启用**账号各签一次；
  账号用**下标**（`--account 2`），不是掩码 id。
- ⚠ 顶层必须收全套签到参数（Rust 侧用 `#[command(flatten)]` 解决）。
- ⚠ `--json` 时进度行**一律走 stderr**，否则 `| jq` 直接语法错。
  单账号给**对象**、多账号给**数组**。
- ✅ 好消息：参数互斥在 cobra 里是 `MarkFlagsMutuallyExclusive`，比 Rust 侧手写
  `conflicts_with_all` **更省事**。但"只读开关与写开关互斥"这条语义要照搬。

### 6.7 `mcloud-server`（2,152 行）

`axum` + `tower-http`(fs) → `net/http` + `chi`，静态用 `http.FileServer`。

⚠ `mcloud-server/tests/api_contract.rs` 的**契约测试要平移** —— 它是前端
（1,265 行 TS，**不动**）与后端之间的接口约定。

---

## 7. 顺序与每步退出判据

原则：**保命网先行，双跑对拍收口，Rust 侧不下线。**

| 步 | 内容 | 退出判据 | 状态 |
| --- | --- | --- | --- |
| **0** | 骨架 + 守卫先行：`go.mod` / `internal/` 八层 / `cmd/` 两个入口 / `scripts/gates-go.sh` / `check-go-deps.sh` / 三处共享守卫改造 | **八项门禁全绿**；依赖断言经**反向测试**证明能抓错 | ✅ **已完成** |
| **冻结** | Rust 侧冻结到 `legacy/`（`bash scripts/freeze-rust.sh`）| 冻结后 **Rust 七条 + Go 八条同时全绿**；`legacy/test` 以符号链接入库（模式 120000）| ✅ **已完成**（`333d05c`，2026-09-20）|
| **1** | `mcloud-core`（纯计算，标准库覆盖最好） | 黄金向量在 Go 侧逐字段复算通过 | 🔄 **进行中**：`crypto` / `error`（前半）/ `auth` / `pure` / `cron` / `sanitize` / `user` 已落（**143 条测试**）。按 Rust 侧 src 行数口径约 **50%**；余 `run`（1,123 行）→ `model` → `stage` → `legacy` → `traits`，另 `error` 后半的 `classify_body`。⚠ 判据要按"**Go 侧**读了几份向量"算，别照门禁那行 `✓ 有测试读取` —— 它不区分是哪一侧在读（见 `status.md`） |
| **2** | `mcloud-upstream`（协议层） | 同一条抓包请求 Rust/Go **双跑**，header 顺序 / percent-encode / 签名串逐字节一致 | ⬜ |
| **3** | `mcloud-io`（唯一 IO 层） | 纯 Go sqlite 驱动下 schema v1→v7 迁移 + 老库只读导入结果一致 | ⬜ |
| **4** | `capabilities` → `exec` → `app` → `server` / `cli` | 19 项能力逐项打勾；CLI 参数互斥与四态输出一致；`api_contract` 契约测试平移，前端不动 | 🔄 **进行中**：`capabilities` 注册表 / `exec` / `app::payload` 已落（**52 条测试**）。⚠ **它依赖步 2 与步 3**：`capabilities::login` 是 `upstream::login` 的消费者，`app` 其余模块与 `server` 还要 `io` —— 所以先落了不依赖它们的三块，见 `status.md` 的「步 4 实况」 |

### 步 0 的实测记录

| 项 | 结果 |
| --- | --- |
| `go build $GO_PKGS` | 通过（**零第三方依赖** —— 骨架刻意不引，好让"能编译"这条判据不被下载失败干扰） |
| `go vet $GO_PKGS` | 通过 |
| `gofmt -l ./cmd ./internal` | 空 |
| `go test -race $GO_PKGS` | 通过 |
| `check-go-deps.sh` | 通过；**并做了反向测试**：往纯层塞 `net/http`、往 CLI 塞 `server`，两条都被抓出且退出码 1 |
| `check-filenames.sh` | 144 个文件通过 |
| `check-identifiers.cjs` | 351 个文件通过；**并做了反向测试**：往 `.go` 里塞未登记号码，被抓出 |
| `check-golden-vectors.cjs` | 131 条断言全绿；**并做了反向测试**：临时让 Go 读一条死向量，消费者探测确实变绿 |

> 📌 反向测试不是形式 —— 一条**只会在正确情况下通过**的断言不是断言。
>
> ⚠ 这里**不是** `./...`，理由见下面第 3 条。`GO_PKGS = ./cmd/... ./internal/...`，
> 定义在 `scripts/gates-go.sh`。

### 冻结暴露的四个问题（2026-09-20）

冻结本身只做了两件事（`git mv` + 一个符号链接），却当场弄红了三个门禁。
四个问题都是"只有真跑一遍才会出现"的，记在这里免得下次迁移重踩：

| # | 现象 | 根因 | 修法 |
| --- | --- | --- | --- |
| 1 | `cargo fmt` 报 `Failed to find targets` | workspace 根是**虚拟 manifest**（只有 `[workspace]`）；一旦用 `--manifest-path` 指过去，`cargo fmt` 就推不出要格式化哪些 target。冻结前它在仓库根跑，靠 CWD 能推出来，所以一直没暴露 | `cargo fmt` 必须同时带 `--all` |
| 2 | `check-filenames.sh` 报 `legacy/Cargo.lock` 不是 kebab-case | 豁免表是**按路径点名**的：`Cargo.toml` 有根级 + `*/` 两条，而 `Cargo.lock` / `rust-toolchain.toml` 只有根级一条 —— 一进 `legacy/` 就掉出豁免 | 豁免表补 `*/Cargo.lock`、`*/rust-toolchain.toml` |
| 3 | `go test ./...` 报 `open legacy/target/debug/deps/xxxx: no such file or directory` | `./...` 会遍历 `legacy/target/`（7 GB cargo 构建产物）。**cargo 正在改写那个目录时**，Go 撞上刚被删的临时目录 ⇒ 门禁红了而代码一行问题都没有。第一次跑就撞上（当时两套门禁并行跑） | 改用显式包路径 `GO_PKGS`；`gofmt` 范围与之对齐 |
| 4 | （第 3 条的代价）新加一个顶层 Go 目录会被**静默漏掉** | 显式路径不会自己发现新目录 | `check-go-deps.sh` 补一条断言：所有 `.go` 文件必须在 `cmd/` 或 `internal/` 下 |

> 📌 第 3 条最值得记：**门禁偶发变红比门禁缺失更危险** ——
> 缺失是已知的；偶发会被当成"环境问题"，于是人学会"重跑一次就好"，防线从此失效。

---

## 8. 第一批要引入的依赖

骨架阶段**零依赖**。开始实现时按这个顺序引：

| 用途 | 包 | 备注 |
| --- | --- | --- |
| SQLite | `modernc.org/sqlite` | 纯 Go，免 cgo |
| CLI | `github.com/spf13/cobra` | 含 `pflag` |
| argon2 | `golang.org/x/crypto/argon2` | |
| 口令输入 | `golang.org/x/term` | 只从 stdin 读、不回显 |
| UUID | `github.com/google/uuid` | |
| 二维码 | `github.com/skip2/go-qrcode` | 终端字符画 + `--qr-out` SVG |
| brotli | `github.com/andybalholm/brotli` | `net/http` 不自动解它 |
| 静态检查（**外部工具**，不是依赖） | `golangci-lint` | 补 `vet` 覆盖不到的部分，见 5.3 |

⚠ **GOPROXY**：本机当前是 `https://proxy.golang.org,direct`，国内拉包会慢。
建议先设一次：

```bash
go env -w GOPROXY=https://goproxy.cn,direct
```

---

## 9. 已知的语义降级（Go 侧必须补的）

迁 Go 会让四条**编译期保证**降级。列在这里不是"反对迁移"，
而是**明确知道丢了什么、用什么补**：

| 丢了什么 | Rust 侧 | Go 侧 | 补法 |
| --- | --- | --- | --- |
| 分层铁律 | 编译期事实（`Cargo.toml` 里没有那个依赖） | lint 约定 | `check-go-deps.sh`（**已就位**） |
| `run` 四态穷尽性 | enum + `match` 穷尽 | `const iota` + `switch`，漏一个态不报错 | `exhaustive` linter |
| `Option` 强制判空 | 编译错误 | `map[string]any` 漏判**直接 panic** | 把 `require_*` 写成一等 API，禁止裸取 |
| 随机源不被并发共享 | `&mut R` | data race | `go test -race`（**已进 §5 门禁**） |

---

## 10. 不能自动翻译

**任何"自动翻译"路线都不成立。** 泛型壳（11 处三泛型依赖注入）、生命周期、
enum 穷尽性在 Go 里没有对应物，翻译器只会产出一个**能编译但丢掉全部防线**的
Go 程序 —— 而它**看起来**像完成了迁移。

这是"用 Go 重写一遍"，不是"把 Rust 翻译成 Go"。

---

## 11. 与评估报告的关系

| 文档 | 回答什么 | 状态 |
| --- | --- | --- |
| [`rust-to-golang-migration.md`](./rust-to-golang-migration.md) | **要不要迁**（含 90–150 人日的工作量估算与反对理由） | 保留 —— 它是决策的依据，别删 |
| **本文** | **怎么迁**（顺序、判据、目录、门禁、坑） | 执行时以本文为准 |

⚠ 评估报告 §8 列的"三个需要拍板的问题"已在 §0 拍板。
但它 §4 指出的"保命网现状"（CI 已删、`gates.sh` 曾在 macOS 上假绿）**仍然有效**，
且**与是否迁移无关**：

- `gates.sh` 的假绿已在 `c267d84` 修掉；
- **CI 仍然没有**。若将来有协作者或在 Linux 上部署，先钉住工具链版本再谈 CI。

> 📌 评估报告 §5 的"全程约束：凭据边界不变"继续生效 ——
> 仓库只允许 `199` 开头的合成标识，`authToken` / `jwt_token` / `auth`
> 不得打印、落库、进 fixture。**Go 的 `testdata/` 同样受守卫管辖**
> （`check-identifiers.cjs` 的 `TEXT_EXT` 已加 `.go`，见 5.5）。
