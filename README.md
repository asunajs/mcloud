# mcloud

移动云盘自动化工具。**Rust 重写版** —— 由原 Node/TypeScript 项目
[`mcloud-sign`](https://github.com/catlair/mcloud-sign) 重写而来。

> ## ⚠ 当前状态：正在从 Rust 迁往 Go（2026-09-20 起）
>
> 动因是**编译体验**：改一行 `mcloud-core`，`cargo check --workspace` 实测 **118 秒**
> （macOS arm64）。代码只会更多，**再往后就来不及切了**，所以现在切。
>
> | 已拍板 | |
> | --- | --- |
> | Rust 侧 | **冻结到 [`legacy/`](./legacy/README.md)** —— 只读，留作对拍基准 |
> | 范围 | **全部 8 个 crate** |
> | 节奏 | 先落骨架与门禁，业务分批迁 |
>
> - **执行方案：[`docs/go-migration-plan.md`](./docs/go-migration-plan.md)** ——
>   **与其它文档冲突时以它为准**。
> - 骨架已就位：`internal/` 八层 + `cmd/` 两个入口 + `bash scripts/gates-go.sh`（**九条门禁**）。
>   **业务已迁完**（2026-09-21）：`core` / `upstream` / `io` / `capabilities` / `exec` /
>   `app` / `server` / `cli` 八层全部落地，**914 条测试**；`cli` 的 12 个叶子命令
>   **全部接上真后端**（缺口表已退役），`cmd/mcloud-server` **也接上了** `internal/server`
>   （不再是占位退 2）。
>   ⭕ **迁移已对位，没有缺口**：`capabilities` 注册表 Go 侧 3 项
>   （`sign` / `exchange` / `cloud-snapshot`）与冻结基准**逐项一致**；
>   服务端 15 条路由也逐条对位（有断言盯着，见 `internal/server/routes_test.go`）。
>   ⚠ 常被误记成"还差 16 项能力"与"`POST /api/runs` 未落" —— **那两项冻结基准里也没有**：
>   19 项来自**旧 TS 项目**的 `packages/api/src/tasks/*`，而 `POST /api/runs` 在
>   `legacy/crates/mcloud-server/src/routes/mod.rs` 里自己写着"要等 P4"。
>   它们属**功能开发**（P4/P5），**不是迁移缺口**。
>   进度与判据：[`docs/status.md`](./docs/status.md)。
> - ⚠ **下面所有 Rust 说明在当前阶段仍然有效** —— 冻结执行前 `cargo` 那套照常能用；
>   冻结之后改用 `cargo <cmd> --manifest-path legacy/Cargo.toml`（**不要 `cd legacy`**，
>   理由见 `legacy/README.md`）。

**这个 README 只做导航。** 详细内容按"你要做什么"分派到 `docs/`，
不要在这里继续堆 —— 它以前有 459 行，每一节都在说"另有一篇更权威的文档"。

## 为什么重写

原实现是 TypeScript，长期受三件事困扰：任务逻辑与 IO 纠缠导致难以单测、依赖体积大、
发布要靠 esbuild 打包。Rust 版的目标是把**纯业务逻辑**与**IO 实现**彻底分开，
让"任务行为是否正确"这件事可以在没有网络的情况下穷尽测试。

一句话概括架构（**Rust 侧现状**；Go 侧是 `internal/` 八层，映射见
[`docs/go-migration-plan.md`](./docs/go-migration-plan.md) §4）：
**8 个 crate 分三层** —— 纯逻辑（core / upstream / capabilities / exec，禁止任何 IO）
+ IO 实现（io，全项目唯一）+ 组装与产物（app / server / cli）。
外部世界一律走 4 个注入 trait：`HttpClient`、`Clock`、`StateStore`、`Random`。

> 依赖方向不是文档约定，是**可执行断言**（`scripts/check-deps.sh` 真读 `Cargo.toml` 断言）。
> 完整分层图、约束表与"为什么必须这样"见 [`docs/dev-gates.md`](./docs/dev-gates.md)。
> ⚠ 迁到 Go 之后这条约束**降级成 lint 约定**（Go 的接口是结构性实现，
> "纯层不许有 IO"不再是编译期事实）—— 所以 Go 侧的 `check-go-deps.sh` 比 Rust 侧那条更必需。

## 快速开始

```bash
cargo run -p mcloud-cli                        # 对全部启用账号各签一次（等价于 mcloud signin）
cargo run -p mcloud-cli -- --list              # 先看下标 1、2、3…

cd web && npm install && npm run build && cd ..   # 前端产物到 web/dist
cargo run -p mcloud-server                        # API + 前端，默认 http://127.0.0.1:8787

# ── Go 侧（迁移后主线）──────────────────────────────────────────────
go run ./cmd/mcloud                            # 对全部启用账号各签一次（等价于 mcloud signin）
go run ./cmd/mcloud --list                     # 先看下标 1、2、3…
go run ./cmd/mcloud-server                     # API + 前端产物（含 SPA 回退），默认 127.0.0.1:8787
go run ./cmd/mcloud-server --help              # 五个选项：--db / --host / --port / --web-dir / --secure-cookie

bash scripts/install-hooks.sh                  # 装一次：提交/推送时自动跑门禁
bash scripts/gates.sh                          # Rust 侧七条门禁，一条命令跑完，全绿才算完成
bash scripts/gates.sh clippy fmt               # 也可以只跑其中几条（调试时用）
bash scripts/gates-go.sh                       # Go 侧九条门禁（迁移期两套并存）
```

> ⚠ Go 工具链在 `/opt/homebrew/bin`，非交互 shell 的 `PATH` 里没有 ⇒ 先
> `export PATH="/opt/homebrew/bin:$PATH"`。

> 迁移期**两套门禁并存**，互不替代：Rust 侧 `gates.sh`（`legacy/Cargo.toml` 存在时会
> **自动**加 `--manifest-path`，不用手改）、Go 侧 `gates-go.sh`。
> 哪些是 Go 自己的、哪些是**复用** Rust 侧脚本，见
> [`docs/dev-gates.md`](./docs/dev-gates.md) §迁移期。

首次用先 `mcloud user init`（建首个管理员）→ `mcloud login qr`（终端出码，手机扫）。

> ⚠ 服务端目前只有"会话"这一层（`/api/health` `/api/setup` `/api/session`）——
> WebUI 登录与初始化是真接口，**主界面仍是占位数据**。详见 [`docs/status.md`](./docs/status.md)。

## 要做什么 → 读哪一部分

| 你现在要做什么 | 读这个 |
| --- | --- |
| 参与 / 了解 **Rust → Go 迁移** | [`docs/go-migration-plan.md`](./docs/go-migration-plan.md) —— 执行方案；冻结约定见 [`legacy/README.md`](./legacy/README.md) |
| 敲命令 / 查参数 / 看签到怎么配定时任务 | [`docs/cli-usage.md`](./docs/cli-usage.md) |
| 跑门禁 / 搞清哪层能依赖哪层 | [`docs/dev-gates.md`](./docs/dev-gates.md) |
| 加文件、起新目录、查命名规则 | [`docs/repo-layout.md`](./docs/repo-layout.md) —— **文件名与标识符的规则不同，别改错** |
| 看**现在能跑什么、还差什么** | [`docs/status.md`](./docs/status.md) |
| 写抓包产物 / 夹具 / 测试字面量 | [`docs/credential-boundary.md`](./docs/credential-boundary.md) + [`docs/sanitize-guide.md`](./docs/sanitize-guide.md) |
| 实现某个业务能力（签到、领取、抢兑…） | [`docs/business-feature-inventory.md`](./docs/business-feature-inventory.md) |
| 接**新的端**（安卓 / 邮箱 / 小程序） | [`docs/client-profiles.md`](./docs/client-profiles.md) |
| 写任何带"**码**"的请求 | [`docs/device-and-source-codes.md`](./docs/device-and-source-codes.md) |
| 做**登录**（PC 客户端 / PC Web / App） | [`docs/upstream-login-protocol.md`](./docs/upstream-login-protocol.md) 一族，按端选 |
| 写**前端页面 / 组件** | [`docs/webui-design-guide.md`](./docs/webui-design-guide.md)（视觉与交互唯一依据） |
| 不知道读哪篇 | **[`docs/README.md`](./docs/README.md)** —— 全库索引 + 「权威关系」表 |

> 文档之间冲突时以谁为准，在 [`docs/README.md`](./docs/README.md) 的「权威关系」表里
> 一次说清，不必每篇自己写一份。

## 第三方内容

`.workbuddy/skills/daisyui/` 是 vendor 进来的 daisyUI 官方 agent skill（MIT）。
来源、固定版本、更新方式与完整许可声明见 [`THIRD-PARTY-NOTICES.md`](./THIRD-PARTY-NOTICES.md)。

## 许可

[Apache-2.0](./LICENSE)
