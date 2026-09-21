# mcloud-rust —— 移动云盘自动化工具的 Rust 实现（已冻结）

> 本仓库是 [mcloud](../mcloud/) 的 **Rust 实现**，2026-09-21 从该仓库的
> `legacy/` 目录整体剥离而来。当前项目活动实现是 **Go**（在 `../mcloud/`）。
>
> **这里不再开发。** 代码保持 2026-09-20 冻结时的原样 —— 七项门禁全绿、
> 714 个测试通过（`cargo test --workspace`）。发现 bug 去 Go 侧修，
> 不要改这里（理由见 [`docs/freeze-record.md`](./docs/freeze-record.md)）。
>
> ⚠ "原样"精确到什么程度：`crates/` 与 `Cargo.*` 与冻结快照**逐字节一致**，
> 改动只落在注释、门禁脚本与向量文件的 `$note` 一行上 ——
> 全部列在 [`docs/freeze-record.md`](./docs/freeze-record.md) 的「剥离补记」里。

---

## 目录

```
crates/         8 个 crate（分层铁律见 docs/dev-gates.md）
scripts/        七项门禁与黄金向量生成器
test/
  fixtures/     夹具（legacy/ = 旧项目 mcloud-sign 的脱敏导出件）
  golden/       与 TS 原实现对拍的黄金向量
docs/           Rust 侧文档（门禁与分层 / 冻结记录 / 历史规划）
.githooks/      **只有归档**：Rust 时代的门禁执行器，本仓库不启用任何 hook
.cargo/         incremental = false（绕开本机写盘故障，见其内注释）
target/         cargo 构建缓存（不入库；带着冻结时的编译产物，别从头编）
```

⚠ 两个 "legacy" 不是一回事：`test/fixtures/legacy/` 是**旧 TS 项目**
`mcloud-sign` 的导出素材；历史上另有一个 `legacy/` 目录装的是**本仓库**
（当时寄居在 mcloud 仓库下）—— 那个目录现在就是本仓库的根，见
[`docs/freeze-record.md`](./docs/freeze-record.md)。

## 怎么跑

```bash
bash scripts/gates.sh                        # 七项门禁一键跑
cargo test --workspace                       # 只跑测试
node scripts/check-golden-vectors.cjs        # 只复算黄金向量
```

仓库根就是 Cargo workspace 根，**不需要** `--manifest-path`
（那是寄居 `mcloud/legacy/` 时期的用法）。

## 文档

| 文档 | 是什么 |
| --- | --- |
| [`docs/dev-gates.md`](./docs/dev-gates.md) | Rust 侧七条门禁 + 分层铁律 + 编译耗时实测 |
| [`docs/freeze-record.md`](./docs/freeze-record.md) | 2026-09-20 冻结记录原文（为什么冻结、不许改的理由）＋ **2026-09-21 剥离补记**：移出时逐字节核过什么、改了哪些注释 |
| [`docs/rust-rewrite-plan.md`](./docs/rust-rewrite-plan.md) | 动工前的 Rust 重写规划快照（提案，进度以冻结记录为准） |
| [`docs/rust-to-golang-migration.md`](./docs/rust-to-golang-migration.md) | Rust → Go 迁移评估报告（规模 / 难点 / 工作量，数字为 2026-09-20 实测） |
| [`docs/go-migration-plan.md`](./docs/go-migration-plan.md) | Rust → Go 执行方案（**权威在 mcloud 仓库**，此处为只读副本） |

## 与 Go 侧（mcloud）的关系

- Go 侧的黄金向量守卫与本仓库**互为交叉验证**：`test/golden/*.json`
  两边各存一份副本，独立复算（Node 侧脚本两边共用同一份逻辑）。
- Go 侧代码注释里大量"对应 Rust 侧 `crates/...`"的对位说明，
  指向的就是本仓库的路径（剥离前写作 `legacy/crates/...`）。

## 历史

冻结前的完整开发历史在 mcloud 仓库的 git 记录里（`git log -- legacy/ crates/`
及其之前的路径）；本仓库从剥离时快照起重新计史。
