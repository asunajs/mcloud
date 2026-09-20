# 门禁与分层铁律

> 状态：**规范**（改代码前必读；跑门禁是完成定义的一部分）
>
> - 入口：**Rust 侧** `bash scripts/gates.sh`（七条）/ **Go 侧** `bash scripts/gates-go.sh`（八条）——
>   都是"一条命令跑完全部，任一条失败即非零退出"。**迁移期两套并存、互不替代**，见下面「迁移期」。
> - 权威说明：本文；分层表与 `scripts/check-deps.sh` 的可执行断言**互为镜像**，改一处要改另一处

---

## 一条命令

```bash
bash scripts/gates.sh                    # 全跑
bash scripts/gates.sh test clippy        # 只跑指定的几条
```

工具链定位只有一份实现：`scripts/toolchain.sh`（被 `gates.sh` 与 `.githooks/`
source）。**不要复制那段代码** —— 它已经因为有三份拷贝而分叉过一次，
详见下面「跨环境」。

---

## 七条门禁

| # | 命令 | 管什么 |
| --- | --- | --- |
| 1 | `cargo test` | 单元测试 |
| 2 | `cargo clippy --all-targets -- -D warnings` | 静态检查（warnings 视为错误） |
| 3 | `cargo fmt --check` | 格式 |
| 4 | `bash scripts/check-deps.sh` | 分层铁律（见下） |
| 5 | `bash scripts/check-filenames.sh` | 非 Rust 文件一律 kebab-case |
| 6 | `node scripts/check-identifiers.cjs` | 仓库里不得有真实标识 |
| 7 | `node scripts/check-golden-vectors.cjs` | 黄金向量能算出自己 |

---

## 迁移期：Go 侧八条（2026-09-20 起）

Rust → Go 迁移期间**两套门禁并存，互不替代**：Rust 侧仍是上面七条
（`gates.sh` 在 `legacy/Cargo.toml` 存在时会**自动**加 `--manifest-path`，不用手改），
Go 侧是：

```bash
bash scripts/gates-go.sh                  # 全跑
bash scripts/gates-go.sh test deps        # 只跑指定的几条
```

| # | 门禁 | 管什么 | 从哪来 |
| --- | --- | --- | --- |
| 1 | `go build ./...` | 能编译 | Go 版 |
| 2 | `go vet ./...` | 静态检查 —— ⚠ **≠ clippy，覆盖窄得多** | Go 版 |
| 3 | `gofmt -l .` | 格式 | Go 版 |
| 4 | `go test -race ./...` | 测试 **+ 竞态** | Go 版 |
| 5 | `bash scripts/check-go-deps.sh` | Go 侧分层铁律 | Go 版 |
| 6 | `bash scripts/check-filenames.sh` | 文件名规则 | **复用** |
| 7 | `node scripts/check-identifiers.cjs` | 不得有真实标识 | **复用** |
| 8 | `node scripts/check-golden-vectors.cjs` | 黄金向量能算出自己 | **复用** |

**6–8 刻意复用同一份脚本，不重写** —— 它们与语言无关（扫文件名 / 扫文本里的真实标识 /
独立复算向量），重写一遍等于给自己造第二份真相，而两份真相迟早分叉，
分叉那天没人知道该信哪一份。

两条**不可省**的替代关系，别把它们当"Rust 侧那条的等价物"：

- **`-race` 不是可选项。** Rust 里 `&mut R rng` 保证同一随机源不被并发共享 —— 那是
  **编译期**保证。Go 的 `io.Reader` 是引用语义，并发共用一个源是 data race，
  而它**不一定**让测试变红：可能只是偶发产生错误数据（签名对不上、`deviceId` 错一位），
  于是被当成"上游抽风"。`-race` 是那条编译期保证在 Go 侧的替代品。
- **`vet` 过了不代表静态检查这关过了。** 它只覆盖"几乎肯定是错"的一小撮检查。
  补齐要引 `golangci-lint`，而那是外部依赖 —— 骨架阶段刻意零第三方依赖。
  引进来之前，**不要拿 `vet` 当 `clippy` 用**。

⚠ 两个易踩的实现细节（都已在 `scripts/gates-go.sh` 里处理，改脚本时别退化）：
`gofmt -l` **退出码恒为 0**，必须自己判输出是否为空；每条门禁的输出写**单独**日志文件，
否则前面通过的门禁留下的字样会污染诊断（Rust 侧踩过这个坑）。

完整理由、逐条设计与"四条会静默降级的编译期保证"见
[`go-migration-plan.md`](./go-migration-plan.md) §5。

---

## 在哪里跑

两个入口，**跑的是同一组命令**：

| 入口 | 什么时候 | 覆盖面 |
| --- | --- | --- |
| `pre-commit` | 每次提交 | `fmt` `deps` `filenames` `idents`（不需要编译的那四条） |
| `pre-push` | 每次推送 | 全七条 |

装一次即可：`bash scripts/install-hooks.sh`（细节见 `.githooks/README.md`）。

---

## 耗时

> ### ⚠⚠ 本节数据是 **Windows + Git Bash** 上测的，在 macOS 上**不成立**
>
> macOS 实测（arm64 / rustc 1.91.1，2026-09-20）：改 `mcloud-core/src/lib.rs`
> **一行**后，`cargo check --workspace` = **1 分 58 秒**（`/usr/bin/time -p` → `real 118.71`）。
>
> 差 4 倍的原因**不是平台 API**，是**这个 workspace 长大了**：`mcloud-core` 之上
> 7 个 crate 全都要重新 check。这是单 workspace 的固有代价，不是调参能解决的
> （`incremental` 已按 `0c65b35` 关掉，那解决的是另一个问题）。
>
> ⇒ **引用本节任何一个数字之前，先看它是在哪个平台、什么时候测的。**
> 下面保留原始数据（它在 Windows 上仍然有效），但**不要再把"30 秒"当成
> "这台机器上的日常"** —— 它已经导致过一次误判（"不需要换语言"）。
> 背景与决策见 [`go-migration-plan.md`](./go-migration-plan.md) §1。

**先说结论（Windows + Git Bash，6 核）：写代码后推送 30 秒左右，不需要 `sccache`。**

实测（6 核，一次只跑一个构建 —— 否则数据全废，见下「怎么量才准」）：

| 场景 | 耗时 |
| --- | --- |
| **写代码后推送（真正的日常）** | **~30s** |
| 全量七条门禁单跑 | 23s |
| `git push` 带全七条 hook | 26s |

拆开看：

| 门禁 | 热 target | 说明 |
| --- | --- | --- |
| `fmt` | ~1s | 纯静态 |
| `deps` | ~1s | 纯静态 |
| `filenames` | 跳过 / ~1s | 无增删改名时跳过 |
| `idents` | ~3s | 纯静态 |
| `clippy --all-targets` | **首次 33s ／ 紧接再跑 2s** | 纯静态分析，不编 crate |
| `test --workspace` | **~20s** | 需重编本仓 8 个 crate |

> ### ⚠ 这里曾有两个 bug，把耗时抬到 8 分钟以上（都已修）
>
> **一、hook 没有写盘重试。** `.githooks/run-gates.sh` 原来失败即判"未通过"
> （而 `scripts/gates.sh` 有重试）：`cargo test` 被环境拦掉、进程中途死亡、
> **不留任何缓存**，故障成本被转嫁到下一次推送。实测 **8 分 11 秒**。
>
> **二、`incremental` 被环境拒绝写入，进而让编译器 ICE。** 这是更贵的那个：
>
> ```
> thread 'rustc' panicked at rustc_metadata/src/rmeta/encoder.rs:2448:51:
> no entry found for key
> error: compiler flags: ... -C incremental=[REDACTED]
> ```
>
> `target/debug/incremental/` 的读写一直被拒（日志里十几到二十几条
> `拒绝访问 (os error 5)`），cargo 反复重试+超时 —— 冷态 `cargo test`
> **跑 10 分钟以上仍编不完**；缓存停在半截状态后 rustc 读它就会 ICE。
> **这个错误看起来像代码问题，其实不是。**
>
> **修法**：`.cargo/config.toml` 的 `[build] incremental = false`。
> 修完：冷态 `cargo test --no-run` **10 分钟+ → 1 秒**，
> 全量门禁 **23 秒**，`push` **26 秒**。

### 两个反直觉的实测结论

**一、`clippy` 去掉 `--all-targets` 会更慢（39s vs 2s）。**
不带它时 `clippy` 只 lint 库目标，走的是另一套 fingerprint，
等于凭空多编一遍。**"少检查"不等于"更快"——关键是产物能否复用。**

**二、`clippy` 的 2s 来自"刚跑过"，不是来自 `test`。**
首次跑 `clippy --all-targets` 要 33s（重新 lint 所有目标），
**紧接再跑同一命令只要 2s**。它的缓存只对自己有效。

### 所以不要这样"优化"

- ❌ **砍门禁条数** —— 静态四条加起来才 6s，砍它们省不下任何东西，
  却丢掉"不写下来就会悄悄失效的约束"（见下）。
- ❌ **给 `clippy` 去掉 `--all-targets`** —— 见上面第一条，反而慢 19 倍。
- ❌ **把 `clippy` 降级成 warning** —— lint 会无声堆积，直到某天一次跑出几百条。
  实测过：`eval "$cmd"` 在 `set -uo pipefail` 下**不会**因 warning 返回非零，
  所以"看得见"是假的。
- ❌ **在 Linux/macOS 上盲目保留 `incremental = false`** —— 那段配置是为本机的
  环境问题写的。**那里 incremental 通常是正常的**，删之前先确认没有上述两个症状。

### 没有采用的提速手段

- **`sccache`** —— 不需要（日常 30s）。且本机沙箱会拦
  `E:\devtools\.cargo` 的写入，`cargo install` 解包 `aws-lc-sys` 时被拒。
- **按 crate 反向依赖缩小测试范围** —— 改 `mcloud-core` 时
  `mcloud-server` 不必重编。收益大但要写"谁依赖谁"的推导，复杂度高。

### 怎么量才准（我用 5 次才量准，前 4 次全是垃圾数据）

1. **一次只跑一个 cargo 构建。** 并行会撞
   `Blocking waiting for file lock on build directory`，
   我因此测出过 352s 这种假数。
2. **记下"测之前的状态"。** 前面刚跑过 `cargo build` 就换了 profile 指纹，
   我因此测出过 63s —— 那不是"日常一次"的状态。
3. **测典型情况，不是最省事的情况。** 我曾拿"改 `.md` 只要 30s"当日常，
   而**写代码才是日常**。这个错误是用户当场指出来的。
4. **做对照时，务必确认两侧都真的执行了被测路径。** 我曾据"冷热都是
   11 条 `拒绝访问`"断言 incremental 不是瓶颈 —— 但那次"热态"里 cargo
   **整个跳过**了 incremental（fingerprint 命中），根本没付出撞墙成本。
   **我把"跳过"当成了"不贵"。**
5. **别拿一次观测当规律。** 前 4 次我每次都以为"这次对了"。
6. **排除一个假设时，要么给出干净的对照实验，要么标注"未验证"** ——
   不许用断言句式。上面第 4 条就是因为写成了断言，把后来的自己带偏了。
7. **记下测量的平台与日期。** 本节第一版把"30 秒"写成了普适结论，而它只在
   Windows + Git Bash 上成立；在 macOS 上是 **118 秒**（差 4 倍）。
   **一个没写平台的耗时数字等于没有数字** —— 它会在别人的机器上变成一条
   误导性结论，而读者不会去怀疑一个写得那么确定的句子。


> ### 曾经有 `ci.yml`，已删（2026-09-20）
>
> `.github/workflows/ci.yml` 从 `0fd68ec` 存在到 `a5607e6`，**期间推了 9 个提交**。
> 它在这段时间里**从未产生过任何影响**：没有一个提交提到过它的结果，
> 没有任何一次改动是因为它而做的。
>
> 删掉它的理由不是"CI 不好"，而是**没人看的检查不是检查**：
>
> 1. **它测的不是本机在跑的工具链。** 本机固定 `1.96.1`，CI 用
>    `dtolnay/rust-toolchain@stable` 拉**当时最新**的 stable，
>    再叠一个 `RUSTFLAGS: -D warnings` —— 每一条新 lint 都变成错误。
>    两边本就不在同一个工具链上，**它红也不能说明代码有问题**。
> 2. **没人能收到它的警报。** 私有仓库 + 不看 Actions 页面 = 一条发出去没人接的线路。
> 3. **这个项目只有一个开发者在本地跑。** 没有别人需要"对外核对"。
>    用"公开仓库后任何人可见"来留它，是拿一个**没验证过的未来**当现在的理由。
>
> 留着它的真实代价不是 CI 分钟数，而是**文档里可以写"CI 已就位"**，
> 让人以为有一层保障。那比没有这层保障更糟。
>
> 如果将来真的需要它（有协作者、或在 Linux 上部署），**先钉住工具链版本**
> （把 `rust-toolchain.toml` 的 `channel = "stable"` 换成一个具体版本号，
> 让 CI 与本机跑同一个），再谈别的。

**用 `--no-verify` 跳过 hook 之后，不会有任何东西替你补上那次检查。**

---

## 跨环境

七条门禁本身都是**平台无关**的，这是刻意核过的（不是"碰巧能跑"）：

- **Rust 侧**：零个 `cfg(unix)` / `cfg(windows)`；测试用 `tempfile::tempdir()`
  而不是硬编码 `/tmp`；`local_tz()` 是硬编码 `TZ_NAME.parse()`，不读系统时区。
- **TLS**：用 `rustls`（不是 `native-tls`），依赖树里没有 openssl ——
  所以不需要目标平台的头文件。
- **Node 侧**（`idents` / `golden`）：零个 `execSync` / `process.platform`，纯 fs 遍历。
- **bash 侧**（`filenames` / `deps`）：零个 `uname` / `cygpath` / `realpath` 分支。
- **bash 侧（第 5 条，2026-09-20 补）**：只用 **POSIX + bash 3.2** 就能跑的语法 ——
  不用 `declare -A` / `mapfile` / `${v,,}` / `&>>`。macOS 自带的 `/bin/bash` 永远是 3.2.57。
  这条是**实跑才发现的**，详见下面「macOS 首跑记录」。

**唯一有平台差异的是工具链定位**（`scripts/toolchain.sh`）：

- **Windows + Git Bash** —— 需要手工补：`RUSTUP_HOME` 指向非默认位置、
  工具链真实 `bin` 置顶 PATH（绕开会联网同步的 rustup shim）、
  MSVC 的 `Hostx64/x64` 进 PATH（顶掉 Git Bash 自带的 GNU `link`）、
  导出 `INCLUDE` / `LIB`（必须是 `C:/...` 原生路径，MSVC 不认 `/c/...`）。
- **macOS / Linux** —— 装好的 rustup 就在 PATH 上，**不需要补任何东西**。
- **其他平台** —— 打印提示但不直接退出（WSL 里 rustup 装好了就能用），
  成败交给后面的 `rustc` 探测。

⚠ 这段逻辑历史上**有三份拷贝**（`gates.sh` / `.githooks/run-gates.sh` /
`toolchain.sh`），并且已经分叉：hook 那份少了 `INCLUDE` / `LIB`，
于是 Windows 上 `clippy` / `test` **在 hook 里链接失败、在 `gates.sh` 里却能过** ——
同一仓库、同一组命令、两个结论。现在统一成 `scripts/toolchain.sh` 一份。
**再加入口时 source 它，不要抄。**

> **首次在 Linux / macOS 上跑门禁时**，请把实际结果（过了、或卡在哪条）
> 补进本节。目前**只在 Windows + Git Bash 上实际跑过** ——
> 上面的"平台无关"是读代码得出的结论，不是跑出来的。
> 在有人真的在类 Unix 上跑过一次之前，那些结论都还是**推断**。

### ✅ macOS 首跑记录（2026-09-20）：七条全绿，但**第一次是一行没跑就退出**

上面的推断**只对了一半**。实跑环境：macOS（arm64 / Darwin 25）+ rustc 1.91.1。

**七条门禁的结论全对** —— 没有任何一条因为平台而失败：

```
test       674 个测试通过
clippy     Finished `dev` profile in 11.03s
fmt        通过（无输出）
deps       依赖方向门禁通过
filenames  143 个文件均为 kebab-case
idents     扫了 326 个文本文件，无未登记的疑似真实标识
golden     122 条断言全绿
```

**但入口脚本自己先挂了**，而且挂得很不显眼：

```
$ bash scripts/gates.sh
scripts/gates.sh: line 42: test: unbound variable
$ echo $?
0
```

**根因是 bash 版本，不是平台 API。** macOS 自带的 `/bin/bash` 是 **3.2.57** ——
Apple 因为 GPLv3 停在了 2007 年那个版本，且不会升；而 `bash` 在 PATH 上默认就是它。
`declare -A`（关联数组）要 **bash 4.0+**。

失败链条值得逐字记住，因为**每一环都不报"真正的原因"**：

1. `declare -A GATES=(` → bash 3.2 报 `invalid option`（但脚本继续往下跑）；
2. `GATES` 于是被当成**普通索引数组**；
3. 紧跟的 `[test]="cargo test --workspace"` 里，`[test]` 是**算术下标**，
   要展开变量 `test`；
4. 它未定义，而脚本头上有 `set -u` → **`test: unbound variable`**；
5. 脚本一行门禁都没跑就退出，**退出码还是 0**。

⇒ 现象与原因隔了四层，而且第 3 步里"`test`"是**数组下标的语法糖**、
看上去像 `test` 命令 —— 排查时很容易往 clap / cargo 那边想。

**修法**：把命令表从 `declare -A` 换成 `case` 分派的 `gate_cmd()`
（`scripts/gates.sh` 与 `.githooks/run-gates.sh` 各一份，**两张表逐字一致**），
bash 3.2 直接就能跑，代价只是查表多一次子进程 —— 门禁的耗时全在 cargo 上，这点可以忽略。

**为什么在 Windows 上测不出来**：Git Bash 的 bash 是 4.4/5.x，`declare -A` 完全正常。
**这类问题没有"写测试"之外的暴露途径** —— 而它恰好是本节第一句那个
"刻意核过的平台无关清单"覆盖不到的一类：

> ⚠ 那份清单核的是**平台 API**（`cfg(unix)` / `/tmp` / openssl / `uname`），
> 核不到**解释器的版本**。本次要补的第 5 条是：
> **bash 脚本只用 POSIX + bash 3.2 就能跑的语法**（不用 `declare -A` / `mapfile` /
> `${v,,}` / `&>>`）。理由是 macOS 的默认 `bash` 永远是 3.2，
> 而它偏偏是"类 Unix 上最可能先被撞到"的那个。

`.githooks/` 那两个 hook 的 shebang 是 `#!/usr/bin/env bash` —— 在 macOS 上同样是 3.2。
所以这次一并修了：**hook 挂在提交路径上，坏掉的表现是拦住每一次提交**，
而人只会得出"hook 坏了"并去用 `--no-verify`；`--no-verify` 之后没有任何东西替你补上（见上一节）。

> 注：`core.hooksPath` 当时**尚未设置**（hook 没启用），所以这个坑还没有真的拦到人 ——
> 是"装一次就会中"。修在装之前。

### 第 5 条为什么值得单独存在

这条规则以前**只写在文档里**，于是 `web/src/pages/` 下三个 `AuthPage.tsx`
一路活到被用户点名。规则写在文档里没人执行，就会反复被违反 ——
而违反的表现极其安静：编译能过、测试全绿、界面正常，
只有下一个人加文件时"照着旁边那个写"，于是错误自我复制。

判据：按 `.` 把文件名拆成段，每段必须匹配 `^[a-z0-9]+(-[a-z0-9]+)*$`。
Rust 源文件不在管辖范围 —— 文件名就是模块名，`mod my-mod;` 不合法，编译器已经强制了。

豁免分三类（**不允许因为"改名麻烦"而豁免**）：

1. **生态固定名**：`Cargo.toml` / `README.md` / `SKILL.md` / `index.html` —— 工具按字面找它们；
2. **上游原样取出的证据**：`docs/capture/apiServer.js`、`test/fixtures/legacy/$meta.json`
   —— 改名会破坏"文件里能搜到的符号 ↔ 文件名"的对应，而这份素材的价值恰恰在于可逐字对照；
3. **生成物 / 依赖目录**：`node_modules` / `target` / `dist`。

新增豁免要同时满足"它属于上述三类之一"与"在脚本注释里写清属于哪一类"。

### 第 7 条是抓包侧的自检

`test/golden/*.json` 是要进版本库的产物，但"有人手工改坏一个字符"只有真算一遍才发现。
它用 Node `crypto` 独立实现算**122 条**向量，与 Rust 侧**互为交叉验证** ——
一边过一边不过，说明其中一边错了。

它同时做两件 Rust 侧做不到的事：

- 对**每个** golden 文件跑脱敏扫描（Rust 只扫 `include_str!` 进来的那些）；
- 列出**没有任何测试读取**的向量文件
  （`login-crypto-vectors.json` 目前是这唯一一种状态；
  `app-business-crypto-vectors.json` 已随 Go 侧 `crypto` 落地而有消费者）。

覆盖范围是**显式**的：没有校验器的段会被**打印出来**，不假装通过。

---

## 分层铁律

依赖方向（箭头左侧依赖于右侧，与各 crate 的 `Cargo.toml` 一致）：

```
mcloud-core          （无内部依赖）
mcloud-upstream      → core
mcloud-capabilities  → core, upstream
mcloud-exec          → core, capabilities
mcloud-io            → core        （+ reqwest / rusqlite / rand：全项目唯一实现 IO 的地方）
mcloud-app           → core, upstream, capabilities, io          组装层（两个产物共用）
mcloud-server        → core, capabilities, exec, io, app         产物之一（HTTP/WebUI）
mcloud-cli           → core, upstream, capabilities, io, app     产物之一（命令行）
```

| crate | 角色 | 约束 |
| --- | --- | --- |
| `mcloud-core` | 领域类型、纯函数、注入 trait 定义 | **禁止任何 IO**：不得依赖 reqwest、rusqlite、mcloud-io |
| `mcloud-upstream` | 上游接口（请求构造、响应解析、签名） | 同上。因此密码学放在 core 而非 io |
| `mcloud-capabilities` | 能力注册表（CLI 与 WebUI 的唯一契约来源） | 同上 |
| `mcloud-exec` | 任务编排 | 同上 |
| `mcloud-io` | reqwest / rusqlite / 系统随机 | 唯一实现 IO 的地方 |
| `mcloud-app` | 组装层 | **必须**依赖 mcloud-io；迁移策略、能力注册表 JSON、登录驱动都在这里 |
| `mcloud-server` | 产物：HTTP/WebUI | **必须**依赖 mcloud-io，否则是"没人接上 IO 实现"的静默死配置 |
| `mcloud-cli` | 产物：命令行 | **不得**依赖 `mcloud-server`（否则会把 axum 拖进命令行产物） |

外部世界一律走注入 trait，共 4 个：`HttpClient`、`Clock`、`StateStore`、`Random`。
`Random` 是必需的 —— smid 构造混用了 `randomUUID` 与 `Math.random`，
不注入则 fixture 回放与黄金向量对拍无法复现。

### 两条"不写下来就会悄悄失效"的约束

**`mcloud-server` 必须依赖 `mcloud-io`。** 反过来（没人接上 IO 实现）是一个静默的死配置：
编译能过、跑起来什么都没发生。

**`mcloud-cli` 不得依赖 `mcloud-server`**（`check-deps.sh` 的 `FORBIDDEN_EDGES` 断言这条）。
两个产物只共用 `mcloud-app`：CLI 一旦拿到 `mcloud-server`，就会连带把 axum / 路由 / 静态资源
拖进命令行产物，而这件事**不会让任何测试变红**，只会让"CLI 能在没有 HTTP 环境的地方跑"
这条性质消失。

### 为什么是脚本而不是文档约定

上表不是文档约定，是**可执行断言**：`scripts/check-deps.sh` 会真的读每个 `Cargo.toml`
并断言依赖方向，违反即非零退出 —— 而不是只在文档里写着"请勿依赖"。
