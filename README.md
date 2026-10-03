# mcloud

移动云盘自动化工具。**Go 实现** —— 前身是 Rust 重写版（已冻结为同级仓库
`../mcloud-rust/`），再往前是原 Node/TypeScript 项目
[`mcloud-sign`](https://github.com/catlair/mcloud-sign)。

> ## ⚠ 当前状态：Go 已接棒（2026-09-20 起迁，2026-09-21 对位完成）
>
> 动因是**编译体验**：改一行 `mcloud-core`，`cargo check --workspace` 实测 **118 秒**
> （macOS arm64）。代码只会更多，**再往后就来不及切了**，所以现在切。
>
> | 已拍板 | |
> | --- | --- |
> | Rust 侧 | **已移出本仓库**（2026-09-21）—— 冻结基准在**仓库外**的 `../mcloud-rust/`，只读、留作对拍基准 |
> | 范围 | **全部 8 个 crate** |
> | 节奏 | 先落骨架与门禁，业务分批迁 |
>
> - **执行方案：[`docs/go-migration-plan.md`](./docs/go-migration-plan.md)** ——
>   **与其它文档冲突时以它为准**。
> - 骨架已就位：`internal/` 八层 + `cmd/` 两个入口 + `node scripts/gates-go.js`（**十条门禁**）。
>   **业务已迁完**（2026-09-21）：`core` / `upstream` / `io` / `capabilities` / `exec` /
>   `app` / `server` / `cli` 八层全部落地，**922 条测试**（2026-09-21 的实测值）；`cli` 的
>   **全部 14 个叶子命令**（⚠ 这个数会随新增命令变 —— 权威是
>   `internal/cli/args_test.go` 的 `TestCommandTreeMatchesTheDocumentedSurface`
>   与 `docs/cli-usage.md` 的子命令表，别照抄本行的数字）
>   **全部接上真后端**（缺口表已退役），`cmd/mcloud-server` **也接上了** `internal/server`
>   （不再是占位退 2）。
>   ⭕ **迁移已对位，没有缺口**：`capabilities` 注册表 Go 侧 3 项
>   （`sign` / `exchange` / `cloud-snapshot`）与冻结基准**逐项一致**；
>   〔2026-09-22 补记：注册表全表现在 **4** 项，新增 `live-room`（直播口令 → 小红花）。
>   **对位的那一组仍是这 3 项**，`live-room` 在原项目里是 **TS 实现**、`legacy/` 里无对应
>   crate ⇒ **属新增能力、不是迁移缺口**。见 `docs/go-migration-plan.md` 步 4 的补记〕
>   服务端 15 条路由也逐条对位（有断言盯着，见 `internal/server/routes_test.go`）。
>   ⚠ 常被误记成"还差 16 项能力"与"`POST /api/runs` 未落" —— **那两项冻结基准里也没有**：
>   19 项来自**旧 TS 项目**的 `packages/api/src/tasks/*`，而 `POST /api/runs` 在
>   `../mcloud-rust/crates/mcloud-server/src/routes/mod.rs`（同级仓库） 里自己写着"要等 P4"。
>   它们属**功能开发**（P4/P5），**不是迁移缺口**。
>   进度与判据：[`docs/status.md`](./docs/status.md)。
> - ⚠ **下面关于 Rust 的说明都成了历史背景**：Rust 基准 2026-09-21 已整体剥离到
>   仓库外的 `../mcloud-rust/`（那边自带七条门禁与自己的 README）。本仓库现在
>   **只留 Go 实现** —— 在这里跑 `cargo` 已经没有 workspace 可指。

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

> 依赖方向不是文档约定，是**可执行断言**：Go 侧由 `scripts/check-go-deps.js` 真读 import
> 断言（它现在是十条门禁里的第 5 条）；Rust 侧那条 `scripts/check-deps.sh` 读的是
> `Cargo.toml`，脚本随基准一起搬到了 `../mcloud-rust/`。
> 完整分层图、约束表与"为什么必须这样"见 [`docs/dev-gates.md`](./docs/dev-gates.md)。
> ⚠ 迁到 Go 之后这条约束**降级成 lint 约定**（Go 的接口是结构性实现，
> "纯层不许有 IO"不再是编译期事实）—— 所以 Go 侧的 `check-go-deps.js` 比 Rust 侧那条更必需。

## 快速开始

```bash
go run ./cmd/mcloud                            # 对全部启用账号各签一次（等价于 mcloud signin）
go run ./cmd/mcloud --list                     # 先看下标 1、2、3…

cd web && npm install && npm run build && cd ..   # 前端产物到 web/dist
go run ./cmd/mcloud-server                     # API + 前端产物（含 SPA 回退），默认 127.0.0.1:8787
go run ./cmd/mcloud-server --help              # 五个选项：--db / --host / --port / --web-dir / --secure-cookie

npm install                                    # 根目录装一次：门禁脚本是 zx（node），依赖钉在根 package.json
node scripts/install-hooks.js                  # 装一次：提交/推送时自动跑门禁
node scripts/gates-go.js                       # 十条门禁，一条命令跑完，全绿才算完成
node scripts/gates-go.js filenames idents      # 也可以只跑其中几条（调试时用）
```

> ⚠ **Rust 侧的命令已不在本仓库**：`cargo run -p mcloud-cli` / `cargo run -p mcloud-server`
> 与七条门禁 `bash scripts/gates.sh` 都要在 `../mcloud-rust/` 下跑。本仓库里没有
> Cargo workspace —— 在这里跑只会得到"找不到 Cargo.toml"这种与代码无关的错。

> ⚠ Go 工具链在 `/opt/homebrew/bin`，非交互 shell 的 `PATH` 里没有 ⇒ 先
> `export PATH="/opt/homebrew/bin:$PATH"`。

> 原"迁移期两套门禁并存"一节已随剥离收敛：Rust 七条（`gates.sh`）现在
> `../mcloud-rust/` 里自跑自的；本仓库只剩 Go 侧十条。
> 哪些门禁是**复用**自 Rust 时代的语言无关脚本，见
> [`docs/dev-gates.md`](./docs/dev-gates.md)。

首次用先 `mcloud user init`（建首个管理员）→ `mcloud login qr`（终端出码，手机扫）。

> ⚠ 服务端已经挂上四组业务路由（accounts / capabilities / runs（含 SSE 事件流）/ schedules）
> 加会话与初始化，另有冻结基准之外自扩的 `GET /api/status`。
> 但 **WebUI 主界面仍是占位数据** —— 原因**不是缺接口**，是"跑任务"那条路本身还没建
> （P4/P5 的功能开发）。细节与判据见 [`docs/status.md`](./docs/status.md)。

## 要做什么 → 读哪一部分

| 你现在要做什么 | 读这个 |
| --- | --- |
| 参与 / 了解 **Rust → Go 迁移** | [`docs/go-migration-plan.md`](./docs/go-migration-plan.md) —— 执行方案；冻结约定见 [`../mcloud-rust/docs/freeze-record.md`](../mcloud-rust/docs/freeze-record.md) |
| 敲命令 / 查参数 / 看签到怎么配定时任务 | [`docs/cli-usage.md`](./docs/cli-usage.md) |
| 跑门禁 / 搞清哪层能依赖哪层 | [`docs/dev-gates.md`](./docs/dev-gates.md) |
| 加文件、起新目录、查命名规则 | [`docs/repo-layout.md`](./docs/repo-layout.md) —— **文件名与标识符的规则不同，别改错** |
| 看**现在能跑什么、还差什么** | [`docs/status.md`](./docs/status.md) |
| 写抓包产物 / 夹具 / 测试字面量 | 凭据边界与脱敏 SOP 已迁至独立仓 `mcloud-capture`（本机 `../mcloud-capture/docs/credential-boundary.md`、`../mcloud-capture/docs/sanitize-guide.md`）；本分支的脱敏守卫 `scripts/check-identifiers.cjs` 照旧生效 |
| 实现某个业务能力（签到、领取、抢兑…） | [`docs/business-feature-inventory.md`](./docs/business-feature-inventory.md) |
| 接**新的端**（安卓 / 邮箱 / 小程序） | [`docs/client-profiles.md`](./docs/client-profiles.md) |
| 写任何带"**码**"的请求 | [`docs/device-and-source-codes.md`](./docs/device-and-source-codes.md) |
| 做**登录**（PC 客户端 / PC Web / App） | 协议五篇已迁至 `mcloud-capture`（本机 `../mcloud-capture/docs/upstream/`，按端选）；本分支不再有 `docs/upstream/` |
| 写**前端页面 / 组件** | [`docs/webui-design-guide.md`](./docs/webui-design-guide.md)（视觉与交互）→ [`docs/webui-components.md`](./docs/webui-components.md)（组件与移动端）→ [`docs/webui-code-guide.md`](./docs/webui-code-guide.md)（代码组织与验收） |
| 改 `mcloud signin` 的**编排逻辑**（不只是查参数） | [`docs/signin-cli-spec.md`](./docs/signin-cli-spec.md) |
| 动手前想知道**这里曾经踩过什么坑** | [`docs/migration-pitfalls.md`](./docs/migration-pitfalls.md)（编号登记表，新条目接在最后） |
| 想知道某件事**当初为什么这么判** / 看历史流水 | [`docs/archive/`](./docs/archive/) —— 归档不删，但**别当现状读** |
| 不知道读哪篇 | **[`docs/README.md`](./docs/README.md)** —— 全库索引 + 「权威关系」表 |

> 文档之间冲突时以谁为准，在 [`docs/README.md`](./docs/README.md) 的「权威关系」表里
> 一次说清，不必每篇自己写一份。

## 第三方内容

`.workbuddy/skills/daisyui/` 是 vendor 进来的 daisyUI 官方 agent skill（MIT）。
来源、固定版本、更新方式与完整许可声明见 [`THIRD-PARTY-NOTICES.md`](./THIRD-PARTY-NOTICES.md)。

## 许可

[Apache-2.0](./LICENSE)
