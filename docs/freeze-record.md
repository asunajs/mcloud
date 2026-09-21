> 📦 **原文归档**：本篇是 2026-09-20 冻结时写在 mcloud 仓库 `legacy/README.md` 的冻结记录原文，
> 2026-09-21 Rust 实现剥离为本仓库时整体移入（原文一字未改，本注除外）。
> 当时的"仓库根"指 `mcloud/`，`legacy/` 指后来被提升为本仓库根的目录。

# legacy/ —— 冻结的 Rust 实现

> **只读。** 这里的代码不再开发，留下来只为当 Go 侧的**对拍基准**。
>
> ✅ **已冻结**（2026-09-20）。`crates/`、`Cargo.toml`、`Cargo.lock`、
> `rust-toolchain.toml`、`.cargo/` 都在这里了，另加一个 `test` 符号链接（见下）。

---

## 冻结时的状态（2026-09-20）

| | |
| --- | --- |
| 冻结前的最后一次提交 | `df6796d`（把未完成的调度特性补齐并修好 —— 见下）|
| Rust 侧门禁 | **`bash scripts/gates.sh` 七项全绿** |
| 测试 | **714 个通过**（`cargo test --workspace`）|
| 门禁感知 | `gates.sh` / `check-deps.sh` 在 `legacy/Cargo.toml` 存在时**自动**加 `--manifest-path`，不用手改 |

⚠ **冻结前最后一批代码是"补完"而不是"原本就绿"**：工作区里留着 2171 行未提交的
调度特性（cron 解析 + server 四个读路由 + `Runner`），但 `cargo test` 根本编译不过，
clippy 也就从未跑到 lint 阶段。冻结前把它修到全绿（12 处测试编译错误 + 14 处 clippy +
1 处过期探针），理由见 `../docs/go-migration-plan.md` §3.4 ——
**一个红的基线不能当对拍基准**：Go 侧跑通那些测试时，无法判断是谁错了。

⚠ `legacy/target/`（约 7 GB）是从仓库根搬过来的 cargo 构建缓存，**已 gitignore**。
保留它是为了别在下次跑 Rust 测试时从头编译一遍依赖。删掉它只影响速度，不影响正确性。

## 为什么冻结

2026-09-20 决定把实现从 Rust 迁到 Go，动机是**编译体验**：

改 `mcloud-core/src/lib.rs` 一行后，`cargo check --workspace` 实测
**1 分 58 秒**（`real 118.71s`，macOS arm64 / rustc 1.91.1）。
根因不是"Rust 慢"，而是 `mcloud-core` 之上 7 个 crate 全部要重新 check ——
这是单 workspace 的固有代价，不是调参能解决的。

完整论证、逐 crate 迁移要点与每步退出判据见
[`../docs/go-migration-plan.md`](../docs/go-migration-plan.md)。

## 什么不许改

**一行都不许改。**

它现在是**参照物**：Go 侧算出什么、Rust 侧算出什么，两边不一致时，
要能确定**是哪一边错了**。一旦这里被顺手改动，这个判断就没有基准了 ——
而改动**不会让任何测试变红**，所以也没有任何东西会提醒你。

发现 bug 也**不要**在这里修，去 Go 侧修。

## 怎么跑它的测试

```bash
cargo test --manifest-path legacy/Cargo.toml --workspace
```

⚠ 用 `--manifest-path`，**不要** `cd legacy` 再跑。仓库里其它工具
（`scripts/check-filenames.sh`、`scripts/check-identifiers.cjs` 等）
都以**仓库根**为工作目录，`cd` 会让它们找不到文件。

## ⚠ `legacy/test` 是符号链接

```
legacy/test -> ../test
```

它存在的唯一理由是：`crates/` 移进 `legacy/` 之后，**17 处**
`include_str!("../../../../../test/golden/*.json")` 的相对路径会**多一层**，
全部失效（`cargo test` 直接编译不过）。符号链接让它们重新指回仓库根的
`test/`，于是**源码一字未改** —— 这对"参照物"这个身份很重要。

- **macOS / Linux**：开箱即用。
- **Windows**：git 默认会把符号链接检出成一个普通文本文件（内容就是
  `../test`），`cargo test` 会失败。两个选择：① 用 `mklink /D` 手工重建；
  ② 放弃符号链接，把那 17 处 `include_str!` 的路径各加一个 `../`。
  ——本仓库当前的开发机是 macOS，这条只是提醒。

## ⚠ 两个 `legacy` 不是一回事

| 路径 | 是什么 |
| --- | --- |
| `legacy/`（这里） | **冻结的 Rust 实现** —— 本项目自己的代码 |
| `test/fixtures/legacy/` | **旧项目 `mcloud-sign` 的导出件** —— 另一份代码的素材 |

同名不同物。看到 `legacy` 先看清是哪个路径，别把两件事混成一件。

---

# 附：剥离补记（2026-09-21）

上面那份记录写于 Rust 寄居在 mcloud 仓库 `legacy/` 下的时期。本节记录把它
**移出为同级仓库**的那次改动 —— 判据与上面同一条：冻结基准不许被悄悄改掉，
所以动了什么，逐条列清。

## 动了什么：一张表

比对方法：与 mcloud 仓库剥离前的提交逐字节对比
（`git -C ../mcloud show HEAD:legacy/<路径>` ↔ 本仓库同名文件），
不是凭印象列的。

| 范围 | 结论 |
| --- | --- |
| `crates/`（177 个 `.rs`）、`Cargo.toml`、`Cargo.lock`、`rust-toolchain.toml`、`.cargo/` | **逐字节一致，0 处改动** |
| `test/` 里**入库的** 19 个文件（8 份 `fixtures/legacy/` + 11 份 `golden/`） | 16 份逐字一致；3 份只改了 `$note` 一行里的落点路径（见下表）。其余本机数据（`test/fixtures/http/`，两侧 `.gitignore` 都不收）也整体复制了一份 |
| `scripts/` 里随剥离移动的 15 个 | 12 个改了注释（见下面清单），3 个逐字一致：`toolchain.sh`、`gen-legacy-fixtures.cjs`、`gen-login-crypto-vectors.cjs` |
| `README.md` | 新写（冻结记录本体逐字保留在本文件上方） |

⇒ 冻结记录里"路径提升一级正好抵消 17 处 `include_str!` 的 `../`"那条账**成立**：
提升为仓库根之后，**业务代码一行没改**。这是本次剥离最重要的一条，
所以它值得一个可复现的核法，而不是一句"我核过了"。

## 改动的清单（全部是注释 / 门禁脚本 / 说明文字）

| 文件 | 改动 | 为什么非改不可 |
| --- | --- | --- |
| `scripts/gates.sh` | 删"冻结探测"整段与 `--manifest-path legacy/Cargo.toml` | 仓库根就是 workspace 根，探测条件永假 |
| `scripts/check-deps.sh` | 删同一套 `MANIFEST` 探测，`cargo tree` 直接跑 | 同上 |
| `scripts/check-golden-vectors.cjs` | 消费者落点收敛为 `crates/`（原来还扫 `internal`/`cmd`/`legacy/crates`）；`checkReasonConsumers()` 加守卫：**本仓库没有 Go 侧目录就打印"跳过"**；新增 `GO_SIDE_ONLY` 表（六份向量的消费者只在 Go 侧，见下） | 落点消失后 `walk()` 的 `try/catch` 会把"什么都没扫"当成"没有死向量"——空转不报错 |
| `scripts/check-identifiers.cjs` | ① 一处措辞：`Cargo.lock（legacy 侧的…）`→`（本仓库的…）`；② 删掉三条**只服务于 Go 侧文件**的豁免（两个多冲突组掩码测试值 + 一个 H5 `logProjectId` 白盒常量），并把 `VENDOR_SOURCE_PATHS` 清空（`docs/capture/` 没有随剥离移动） | 这两张表都有"条目对不上文件就失败"的自查 —— 不删就是**红的门禁**，删了才是本仓库的事实 |
| `scripts/gen-*.rs`（5）、`scripts/gen-*.cjs`（2） | 注释与产出 `$note` 里的 `legacy/crates/` → `crates/` | 那个前缀在本仓库不存在 |
| `test/golden/{app-signin,auth,crypto}-vectors.json` | **只改 `$note` 一行**，`input` / `expect*` 一个字节没动 | 说明文字里的路径失效了 |
| `THIRD-PARTY-NOTICES.md` | 按本仓库实际内容重写 | 原来是 mcloud 那份的逐字副本，里面的 daisyUI 一节描述的是**没有随剥离移动**的 `web/` |
| `.githooks/run-gates.sh` | 从 mcloud 移入归档，只加"历史脚本"抬头；执行体逐字未动 | 它是 Rust 时代的 hook 执行器 |
| `docs/` | 新增本篇、`dev-gates.md`（Rust 侧那一半）、`rust-rewrite-plan.md`、`rust-to-golang-migration.md`、`go-migration-plan.md`（只读副本）、`freeze-rust.sh`（历史脚本归档） | 这些内容讲的都是 Rust 侧，留在 Go 仓库就成了幽灵引用 |

## ⚠ `test/` 从此是**两份实体副本**

原来 `legacy/test` 是指向 mcloud `test/` 的**符号链接** —— 一份真相。
现在两个仓库各有一份，**没有任何门禁盯着它们是否一致**：

- 新增一份向量、或改一份向量的 `expect*`，**必须两边都落**，
  否则另一侧的守卫只会说"我自己的向量能复算我自己的算法"，对拍性质就没了。
- 判据：**跨仓一致性目前靠人**。要么接受，要么在任一侧加一条比对
  `../mcloud/test/golden/` 的检查（那会让两侧产生路径耦合，
  剥离的初衷恰恰是解除它 —— 所以这里**没加**，是有意的取舍，不是漏了）。

## ⚠ 搬过来的 `target/` 会先制造**假失败**

`target/` 是随仓库一起搬的（不想从空缓存重编 8 个 crate）。但里面那批产物是在
**旧绝对路径**下编译的，而 `env!("CARGO_MANIFEST_DIR")` 是**编译期烘进去**的常量 ——
两处运行期读夹具的测试因此指向已经不存在的 `…/mcloud/legacy/test/fixtures/legacy`：

- `crates/mcloud-io/src/legacy_import/tests/mod.rs:11`
- `crates/mcloud-app/src/migration.rs:203`

剥离后第一次 `bash scripts/gates.sh` 于是报了 **30 条**
`读夹具 asign.json 失败: No such file or directory`（`legacy_import` 整组），
`test` 门禁红。让 cargo 重编那两个 crate 之后，同样的用例**全绿**。

⇒ 判据：**搬过 target/ 之后，第一次门禁的结论不算结论** —— 要么先 `cargo clean -p`
那两个包，要么把第一次跑当成"预热 + 重编"，看第二次的结果。
`cargo clean -p mcloud-io` 之后 `cargo test --workspace` 全绿 —— 注意**只清那一个包**就够，但 `migration.rs:203` 那一处同样烘了路径，所以稳妥做法是
`cargo clean -p mcloud-io -p mcloud-app` 再跑。这里没写进 `gates.sh` 自动处理，因为"缓存来自别的路径"这件事只有搬的人知道。

## 六份向量在本仓库**没有** Rust 消费者

`datetime-format` / `enum-json` / `legacy-shape` / `login-crypto` / `password-hash` /
`traits-shape` 这六份是为"钉住 Go 侧要复刻的形状"补的，Rust 侧从未读过它们
（判据：`include_str!` 全仓 grep 为 0）。剥离前它们在同一仓库里由 Go 测试读，
脚本扫 `internal`/`cmd` 就看得见；现在看不见，会被印成**死向量**。

⇒ 脚本里加了 `GO_SIDE_ONLY` 表，把这六份印成"本仓库无消费者，**Go 侧有**"，
并且**反过来自查**：本仓库哪天真有了消费者，这条表项就算失败。
不这么做的话，"死向量"警告一多，真死的那份就混在里面没人看了。

## ⚠ 两侧 `check-golden-vectors.cjs` 会持续分叉

mcloud 那份在剥离之后又长了两条守卫（"落点清单只扫真存在的目录" +
"一个落点都没扫到就算失败"）。本仓库这份**没有**，因为这里只有一个落点、
且它真的存在。共同的判据（脱敏扫描、逐段复算、豁免表自报）仍同源。
