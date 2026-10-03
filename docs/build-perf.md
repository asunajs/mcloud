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
6. **上线只编需要的 bin**：`cargo build --release -p mcloud-server --bin mcloud-server`，
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

## 4. 什么时候该重测

会话（写入线）数变化、crate 数明显增加、`target` 又涨一个量级，
或者把 §2 的 1~4 落地之后 —— 照 §1 / §3 的命令复测即可，命令都在本文里。
