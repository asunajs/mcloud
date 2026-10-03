# 编译/检查为什么慢 —— 实测归因与操作口径

> 测于 **2026-10-01 22:55~23:10**（Asia/Shanghai），工作区
> `/Users/catlair/mobile/workspace/mcloud-gost`（外置宗卷 `/dev/disk5s1`，APFS），机器 **10 核 / 16 GB**。
>
> ⚠ 全部是**点测**，而且当时工作区里还有**另一条写入线**正在跑
> `cargo check --workspace --all-targets`、并同时在改 `crates/mcloud-core/src/kouling.rs`
> ⇒ 下面各命令的耗时**含争用**，只用于同类比较，**不要当基准**。

## 0. 结论（按影响排序）

| # | 原因 | 实测证据 | 性质 |
|---|---|---|---|
| 1 | **多条写入线共享同一个 `target/`** ⇒ 构建锁串行 + 指纹互相作废 | 采样时 `pgrep -fl cargo` 抓到另一会话的 `cargo check --workspace --all-targets`；`mcloud-core` / `mcloud-tasks` / `mcloud-engine` 是**全仓所有 crate 的下游**，谁改一行，下游全部失效 | 机制性，**会话越多越重** |
| 2 | **`--all-targets` 让工作量翻倍** | 含 `#[cfg(test)]` 的文件 **94** 个（每个 lib 再编一份 test 版）+ `crates/*/tests/*.rs` **8** 个集成目标（各要一次**链接**） | 自检口径选错 |
| 3 | **`target/` 已累积到 31 GB / 551,339 个文件** | `du -sh target` → `31G`；`target/debug` `29G`，其中 `target/debug/incremental` **15G**；`find target -type f` 计数 551339 | 累积性，**时间越久越慢** |
| 4 | release 的 `lto = "fat"` + `codegen-units = 1` | 根 `Cargo.toml` 的 `[profile.release]`（还有 `opt-level="s"` / `panic="abort"` / `strip`） | **有意为之**的体积权衡，不是故障 |
| 5 | 并发额度：两条线同时编 ⇒ 最多 20 个 rustc 抢 10 核 | `sysctl -n hw.ncpu` = 10；负载 4.2~4.6 | 争用 |

**机制串起来是这样**：会话/写入线变多 ⇒ 别人的 `cargo` 占着 `target/` 的独占锁（我的命令在**等**）✗；
而且对方改的是**全仓下游**的底层 crate ⇒ 我这边刚编好的缓存**立刻作废**，下次整片重来 ✗。
叠加 `target/` 一路涨到 55 万个文件 ⇒ "越用越慢"。

## 1. 被实测**排除**的路（别再往这些方向查）

| 假设 | 实测 | 结论 |
|---|---|---|
| 外置盘顺序吞吐慢 | `dd` 256 MB：写 **978 MB/s**、读 10.3 GB/s（走缓存） | ✗ 不是 |
| 小文件随机 I/O 差（cargo 真正吃的那个） | 写 1200 个 × 4 KB = **0.13 ms/个**；`stat` **3 µs**；列 1200 项 45 ms | ✗ 不是 |
| 内存不足在换页 | `vm.swapusage` total **0.00M**；`Swapins` / `Swapouts` = **0** | ✗ 不是 |
| 磁盘满了 | 外置盘 302 GB 可用 | ✗ 不是 |
| 把 `target` 挪到内置盘 | 内置盘 `/System/Volumes/Data` 只剩 **53 GB** | ✗ **不可行**（装不下 31 GB）|

> 最后一条是"本来想推荐、实测后否掉"的典型：外置盘吞吐实测很好，唯一的动机只剩
> "少触发可移除宗卷授权"，但内置盘空间不够 ⇒ 不值得。

## 2. 操作口径（按性价比排序）

0. ⚠⚠ **一个回路里只跑一种 cargo 命令**（2026-10-03 实测，见 §3.1）：
   `check` / `clippy` / `test` 各用一套编译产物、**互不复用** ✗ ⇒ 三种都跑 = 同一张依赖图
   编三遍（改一处顶层 crate 就要 ≈7.7 分钟）。平时自检只挑一种 ✓。

1. **日常"能不能编过"用 `cargo check -p <crate>`** —— 省掉 test 版编译与 8 次链接；
   **提交前**才用 `--workspace --all-targets` 与 `cargo test --workspace`（那是运行期的门）。
2. **起编译前先看一眼有没有别人的在跑**：`pgrep -fl 'cargo check'`。有就等它 ——
   cargo 对 `target/` 上独占锁，硬抢只会两边都慢。
3. **底层 crate 的改动攒成一批再编**：`mcloud-core` / `mcloud-tasks` / `mcloud-engine`
   是**全仓下游**，每改一行就编一次 = 全仓重编一次，这是最亏的用法。
4. **gate 类跑法可加 `CARGO_INCREMENTAL=0`**：`incremental` 吃掉了 `target/debug` 的一半
   （15 GB），而跨会话它大多白存（指纹早被作废）。
   ⚠ 但**单 crate 紧改循环**反过来该**保留** incremental —— 两种用法别一刀切。
5. **清一次 `target`**：31 GB / 55 万文件，代价是一次全量重编（≈20 分钟），
   建议在**所有写入线都不忙**的时候做。
6. **上线只编需要的 bin**：`cargo build --release -p mcloud-server --bin mcloud`，
   不要 `--workspace`（release 那边还有 `lto="fat"`，成本天生高）。
7. 可选 **sccache**（跨 target 目录 / 跨分支复用 rustc 产物，正好对治原因 1）；
   建议先按 1~4 压一轮，再评估值不值一个常驻 daemon。

## 3. 本次实测耗时（含争用，仅供同类比较）

| 命令 | 耗时 |
|---|---|
| `cargo check --workspace --all-targets` | 5m53s |
| `cargo fmt --all -- --check` | < 1 s |
| `cargo clippy --workspace --all-targets -- -D warnings` | 数分钟（当次先红在 `mcloud-engine` 的 `derivable_impls`，修掉后 exit 0） |
| `cargo test --workspace` | 26 个测试目标；通过条数**别背数**（会随每轮改动涨），当次读到 1740 条。现算：`grep -rh "#\[test\]\|#\[tokio::test\]" crates \| wc -l`（22:30→22:43 那一段） |
| `node scripts/gates-web.mjs --full` | ≈ 110 s（**当次含争用**；与 Rust 侧无关，纯前端 + 无头 Chrome。耗时随机器变，别当基准） |

## 3.1 2026-10-03 复测（用户口径「门禁越来越慢了」）

同一台机器（10 核 / 16 GB），`[profile.dev] debug = "line-tables-only"` 落地**之前**
冷跑的两次（改 bin 名之后，全量重编）：

| 命令 | 耗时 | 备注 |
|---|---|---|
| `cargo fmt --all -- --check` | **1.34 s** | |
| `cargo clippy --workspace --all-targets -- -D warnings` | **19m13s** | 冷（profile 指纹变了） |
| `cargo test --workspace` | **27m14s** | 冷 |
| └ **其中"真跑测试"** | **1.17 s** | **717 个测试全过** ✓ |

⚠⚠ **这条最值得记住：门禁 99.9% 的时间是编译与链接，跑测试本身只要 1 秒** ✓。
所以"把测试写快一点"没有意义，**能省的只有"少编一遍"** ✓。

### 新发现：`check` / `clippy` / `test` 各一套产物，**互不复用** ✗

改**一处**源码（顶层 crate）之后逐条实测：

| 命令 | 耗时 | 其中"真跑测试" |
|---|---|---|
| `cargo check -p mcloud-server` | 122.9 s | — |
| `cargo clippy -p mcloud-server` | 152.1 s | — |
| `cargo test -p mcloud-server` | 186.1 s | 34.0 s（52 个测试） |

⇒ 三条**加起来 ≈ 7.7 分钟**，而它们编的是**同一张依赖图** ✗ —— clippy 是另一套
fingerprint（跑的是 `clippy-driver`），check 与 test 各是另一套（`.rmeta` vs 真实目标文件）✓。
**同一个回路里把三种都跑一遍 = 把图编三遍** ✗✗。这是"门禁越来越慢"最直接的原因 ✓。

**口径**（照这个来）：

* **平时改完一处自检**：只挑**一种** —— `cargo check -p <改动的 crate>` ✓
  （**第二次起是秒级**：第一次只是为了把该 crate 的 `.rmeta` 变热 ✓）。
* **要跑测试**：`cargo test -p <crate>` ✓，别顺手 `--workspace` ✗（那是提交前的事）。
* **改前端**：`node scripts/gates-web.mjs`（快回路 ≈ 110 s）✓。
* **提交前**：四条全量（`cargo test --workspace` / `clippy --workspace --all-targets` /
  `fmt --check` / `gates-web --full`）✓ —— 冷的时候将近 50 分钟，**别在赶时间时顺带跑** ✓。

### 这一轮落地的两条改动

1. **`[profile.dev] debug = "line-tables-only"`**（`Cargo.toml`，test profile 继承它）——
   `target/debug/mcloud` 实测 **39.5 MB → 37.2 MB** ✓。
   ⚠ 诚实说明：**链接时间的收益没有单独量到**（两种配置各要一次全量重编，成本比收益高）；
   保留它的理由是"少生成/少搬运 debuginfo"这条机制本身 ✓，以及需要完整调试信息时
   可以用 `CARGO_PROFILE_DEV_DEBUG=2` 临时抬回去 ✓。
2. **三条要无头 Chrome 的 web 门禁改成并行**（`scripts/gates-web.mjs`）——
   实测 **246 s → 142 s**（`--full` 全程 **4m06s → 2m22.8s** ✓，省 ≈1.7 分钟/次 ✓）。
   它们互不依赖（各自起夹具后端、端口由内核分配、各用自己的 Chrome profile ✓），
   而且**都不是靠计时断言的**（数堆/数节点/量计算样式 + 固定 dwell）⇒ 抢 CPU 不会让它们变脆 ✓。
   ⚠ 真机只 2 核时脚本会自动退回串行 ✓（`cores >= 4` 才并行）。

### 试过、**不划算**的：换 `ld64.lld`（2026-10-03 实测）

链接是瓶颈 ⇒ 自然会想换链接器。本机装了 `/opt/homebrew/bin/ld64.lld` ✓，实测**不划算** ✗：

做法：照抄 cargo 实际用的那条 `rustc` 命令（`cargo build --bin mcloud -v` 抓下来 ✓），
只把 `--out-dir` / `-C extra-filename` / `-C incremental` 指到 `/tmp`（别碰正在跑的二进制 ✓），
然后同一条命令各跑两遍 —— 两组**编译部分逐字节相同**，差值就是链接器的贡献 ✓：

| 链接器 | 较优一次的耗时 | 产物 |
|---|---|---|
| 默认（`ld64`） | **9.62 s** | 37.2 MB |
| `-C link-arg=-fuse-ld=lld` | **11.08 s**（慢 15% ✗） | 34.8 MB |

⇒ **没有采用** ✗，也就免掉了"写进仓内配置后、发布机上没装 lld 直接编不过"那个风险 ✓。
⚠ 两条边界：① 量的是 macOS/Apple Silicon 上的 **debug** 链接（本仓日常最贵的那条 ✓），
Linux/发布链路没量 ✗；② 第二遍已经是"增量复用后只剩链接"的形状，但严格说仍是
`rustc` 的编译+链接总时长，纯链接的占比没有单独切出来 ✗。

## 3.2 「写代码之外那一圈」怎么压（2026-10-03 实测，用户口径「太慢了」）

先给结论，再说数据：**时间不是花在"编译慢"，而是花在"命令种类一换、整个依赖图重编一遍"** ✗。

### 实测

| 动作 | 耗时 |
|---|---|
| `cargo check -p mcloud-core`（改一处叶子 crate 之后，产物是热的） | **8.0 s** ✓ |
| `cargo check -p mcloud-tasks` —— **那一档产物还是冷的时候** | **663.5 s** ✗ |
| `cargo check -p mcloud-server` —— 同上 | **227.2 s** ✗ |
| 同一档命令**再跑一遍**（没有新改动） | **7.6 s** ✓ |
| `cargo check -p mcloud-tasks --all-targets`（多编一份 test 版） | **31.5 s**（≈ 4 倍 ✗） |
| `cargo clippy --workspace --all-targets`（冷） | 19m13s ✗ |
| `cargo test --workspace`（冷） | 27m14s ✗ |
| └ 其中**真跑测试** | **1.17 s** ✓（717 个用例） |

⇒ 两条读数：

1. **只有第一次是贵的** ✓ —— 同一档命令补齐之后是秒级 ✓；而三档命令
   （`check` / `clippy` / `test`）**各一套产物、互不复用** ✗ ⇒ 在一个回路里换着跑
   = 把同一张图按三种口味各编一遍 ✗✗。
2. **改中游 crate 的代价是它的下游** ✗ —— `mcloud-core` / `mcloud-tasks` /
   `mcloud-engine` 是**全仓下游**，碰它们之后**任何**一档命令的第一遍都要重来 ✗。
   ⇒ 一次编辑尽量只落在**一个** crate 里；碰了中游就先想清楚"这一轮要哪一档" ✓。

### 口径：开发循环（照这个来，别改）

```bash
cargo check -p <刚改的那个 crate>      # 改完立刻自检：热的时候秒级 ✓（别用 --workspace）
cargo test  -p <那个 crate>            # 要跑测试时：**只挑这一档**，别 check/test 来回换 ✗
node scripts/gates-web.mjs             # 前端：≈110 s（三条要 Chrome 的已并行 ✓）
```

* ⚠ **提交前那四条全量**（`test --workspace` / `clippy --workspace --all-targets` /
  `fmt --check` / `gates-web --full`）照旧要跑 ✓ —— 但**放后台跑** ✓，别在前台等 ✗
  （前台等的那些分钟，正好拿来写下一处改动 ✓）。
* ⚠ `cargo check --workspace --all-targets` 与 `cargo clippy --workspace --all-targets`
  **是同一张图的两次编** ✗ ⇒ 两条里**只留一条**（clippy 覆盖 check 的全部检查 ✓）。
* 前端那一档与 Rust 那三档**互不相干** ✓ ⇒ 可以同时跑（两边各用各的 CPU ✓）。

### 还试过、**不划算**的

* `ld64.lld`：**慢 15%** ✗（见上一节）⇒ 不采用。
* `[profile.dev] debug = "line-tables-only"`：留下 ✓（`target/debug/mcloud` 37.2 MB ✓），
  但**它只影响"编+链"里的链接那一段**，对上面的秒级循环**没有帮助** ✗ ——
  别指望 profile 设置能救开发循环，**能救它的是"少换命令种类 + 缩小作用域"** ✓。

## 4. 什么时候该重测

会话（写入线）数变化、crate 数明显增加、`target` 又涨一个量级，
或者把 §2 的 1~4 落地之后 —— 照 §1 / §3 的命令复测即可，命令都在本文里。
