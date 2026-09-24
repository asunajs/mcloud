# mcloud-rust —— 移动云盘自动化工具的 Rust 实现

> 本仓库是 [mcloud](../mcloud/) 的 **Rust 实现**，2026-09-21 从该仓库的
> `legacy/` 目录整体剥离而来。
>
> **2026-09-24 解冻，恢复为独立活跃项目。** 此前 2026-09-20 至 2026-09-24
> 曾整体冻结、只读保留作 Go 侧的对拍基准；冻结与剥离的完整记录（含"当时
> 为什么不许改"）见 [`docs/freeze-record.md`](./docs/freeze-record.md)，
> 那是**历史文档**，其禁令已随解冻失效。
>
> 解冻后与 Go 侧的关系：**各自独立开发**（见下文「与 Go 侧的关系」）。

---

## 目录

```
crates/         8 个 crate（分层铁律见 docs/dev-gates.md）
scripts/        七项门禁与黄金向量生成器
test/
  fixtures/     夹具（legacy/ = 旧项目 mcloud-sign 的脱敏导出件）
  golden/       黄金向量（与另一侧实现交叉复算）
docs/           Rust 侧文档（门禁与分层 / 冻结与剥离记录 / 历史规划）
.githooks/      入库的 pre-commit / pre-push 门禁（每个克隆跑一次安装，见其 README）
.cargo/         incremental = false（绕开本机写盘故障，见其内注释）
web             → ../mcloud/web 的符号链接（WebUI 只在 mcloud 维护，见下）
target/         cargo 构建缓存（不入库）
```

⚠ 两个 "legacy" 不是一回事：`test/fixtures/legacy/` 是**旧 TS 项目**
`mcloud-sign` 的导出素材；历史上另有一个 `legacy/` 目录装的是**本仓库**
（当时寄居在 mcloud 仓库下）—— 那个目录现在就是本仓库的根，见
[`docs/freeze-record.md`](./docs/freeze-record.md)。

## 怎么跑

```bash
git config core.hooksPath .githooks   # 每个克隆跑一次：启用提交/推送门禁
bash scripts/gates.sh                 # 七项门禁一键跑（全量，手动）
cargo test --workspace                # 只跑测试
node scripts/check-golden-vectors.cjs # 只复算黄金向量
```

仓库根就是 Cargo workspace 根，**不需要** `--manifest-path`
（那是寄居 `mcloud/legacy/` 时期的用法）。

提交与推送路径上会自动跑门禁（pre-commit 快查、pre-push 全查，按改动范围
收窄），执行器是 `.githooks/run-gates.sh`；设计与判据见
[`.githooks/README.md`](./.githooks/README.md)。

## 文档

| 文档 | 是什么 |
| --- | --- |
| [`docs/dev-gates.md`](./docs/dev-gates.md) | Rust 侧七条门禁 + 分层铁律 + 编译耗时实测 |
| [`docs/freeze-record.md`](./docs/freeze-record.md) | **历史文档**：2026-09-20 冻结记录原文（为什么冻结、当时不许改的理由）＋ 2026-09-21 剥离补记 ＋ 2026-09-24 解冻标注 |
| [`docs/rust-rewrite-plan.md`](./docs/rust-rewrite-plan.md) | 动工前的 Rust 重写规划快照（历史提案） |
| [`docs/rust-to-golang-migration.md`](./docs/rust-to-golang-migration.md) | Rust → Go 迁移评估报告（历史文档，数字为 2026-09-20 实测） |
| [`docs/go-migration-plan.md`](./docs/go-migration-plan.md) | Rust → Go 执行方案（历史文档，权威在 mcloud 仓库，此处为只读副本） |

## 与 Go 侧（mcloud）的关系

- **2026-09-24 起各自独立开发**：本仓库恢复活跃，不再充当对拍基准；
  mcloud 侧继续演进它自己的 Go 实现。
- **WebUI 只维护 mcloud 那一版**：本仓库的 `web/` 是指向 `../mcloud/web` 的
  **符号链接**，不落副本、不在这里改前端。构建产物 `web/dist` 也随之可用
  （`mcloud-server` 默认 `--web-dir web/dist` 直接伺服）。
  ⚠ Windows 上 git 会把符号链接检出成文本文件（见 freeze-record 的同类提醒），
  需要时用 `mklink /D web ..\mcloud\web` 重建。
- **API 接口完全按 Go 版输出**：路径、方法、payload 形状以
  `../mcloud/internal/server/` 为唯一契约 —— 新增接口先看 Go 侧怎么定义，
  已有接口的出网字段以 Go 版为准逐字对齐（差异清单见
  [`docs/dev-gates.md`](./docs/dev-gates.md) 之外的 server 模块注释与
  `crates/mcloud-server/src/routes/mod.rs`）。
- 黄金向量（`test/golden/*.json`）两边各存一份副本，曾经互为交叉验证。
  现在各自维护 —— 新增或修改向量时**跨仓一致性靠人**（没有门禁盯这一点，
  这是剥离时的有意取舍，见 freeze-record）。
- **抓包素材（HAR / capture）只保留在 Go 侧** mcloud 仓库，本仓库不收
  （`.gitignore` 里的 `har/`、`test/fixtures/http/` 即此）。
- mcloud 侧代码注释里"对应 Rust 侧 `crates/...`"的对位说明，
  指向的仍是本仓库的路径（剥离前写作 `legacy/crates/...`）。

## 历史

冻结前的完整开发历史在 mcloud 仓库的 git 记录里（`git log -- legacy/ crates/`
及其之前的路径）；本仓库从剥离时快照起重新计史，2026-09-24 解冻后在本仓库继续。
