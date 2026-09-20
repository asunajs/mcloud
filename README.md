# mcloud

移动云盘自动化工具。**Rust 重写版** —— 由原 Node/TypeScript 项目
[`mcloud-sign`](https://github.com/catlair/mcloud-sign) 重写而来。

**这个 README 只做导航。** 详细内容按"你要做什么"分派到 `docs/`，
不要在这里继续堆 —— 它以前有 459 行，每一节都在说"另有一篇更权威的文档"。

## 为什么重写

原实现是 TypeScript，长期受三件事困扰：任务逻辑与 IO 纠缠导致难以单测、依赖体积大、
发布要靠 esbuild 打包。Rust 版的目标是把**纯业务逻辑**与**IO 实现**彻底分开，
让"任务行为是否正确"这件事可以在没有网络的情况下穷尽测试。

一句话概括架构：**8 个 crate 分三层** —— 纯逻辑（core / upstream / capabilities / exec，
禁止任何 IO）+ IO 实现（io，全项目唯一）+ 组装与产物（app / server / cli）。
外部世界一律走 4 个注入 trait：`HttpClient`、`Clock`、`StateStore`、`Random`。

> 依赖方向不是文档约定，是**可执行断言**（`scripts/check-deps.sh` 真读 `Cargo.toml` 断言）。
> 完整分层图、约束表与"为什么必须这样"见 [`docs/dev-gates.md`](./docs/dev-gates.md)。

## 快速开始

```bash
cargo run -p mcloud-cli                        # 对全部启用账号各签一次（等价于 mcloud signin）
cargo run -p mcloud-cli -- --list              # 先看下标 1、2、3…

cd web && npm install && npm run build && cd ..   # 前端产物到 web/dist
cargo run -p mcloud-server                        # API + 前端，默认 http://127.0.0.1:8787

bash scripts/install-hooks.sh                  # 装一次：提交/推送时自动跑门禁
bash scripts/gates.sh                          # 七条门禁一条命令跑完，全绿才算完成
bash scripts/gates.sh clippy fmt               # 也可以只跑其中几条（调试时用）
```

首次用先 `mcloud user init`（建首个管理员）→ `mcloud login qr`（终端出码，手机扫）。

> ⚠ 服务端目前只有"会话"这一层（`/api/health` `/api/setup` `/api/session`）——
> WebUI 登录与初始化是真接口，**主界面仍是占位数据**。详见 [`docs/status.md`](./docs/status.md)。

## 要做什么 → 读哪一部分

| 你现在要做什么 | 读这个 |
| --- | --- |
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
