# mcloud

中国移动云盘（139 云盘）自动化平台：把签到、领 AI豆、直播口令、App 任务、兑换这些动作
做成一套**可定时、可编排、可回看日志**的自托管服务。

- 后端 **Rust**：8 个 crate。签名与加解密是**纯 Rust 实现**（无 FFI）；TLS 走 `native-tls`
  ⇒ Linux 上链接系统 OpenSSL（交叉编 arm64 时改用 `vendored` 从源码编，产物静态链）。
- 前端**自研 lite 框架**（无虚拟 DOM + 编译期优化）+ TSX + Tailwind 4（`@theme` 令牌层）。
- 交付形态是**一个二进制** `mcloud`：
  - **不带参数** ⇒ 起 WebUI 服务（默认 `http://127.0.0.1:3000`）
  - **带子命令** ⇒ CLI（`tasks` / `run` / `login` / `cron` / `pipeline` / `auth` / `pending`）

> 当前版本 `3.0.0-rc.2`。现算（别背数）：
> ```bash
> sed -n '/\[workspace.package\]/,/^\[/p' Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1
> ```
> 仓库是 `asunajs/mcloud`：`main` = 这份 Rust 实现；那份 **Go 参考实现**是同仓的
> `go-legacy` 分支（抓包素材与协议规范在第三个仓 `mcloud-capture`）。

| 你想干什么 | 看哪 |
|---|---|
| 拿到 Release 包，想跑起来 | [`docs/usage.md`](docs/usage.md) |
| 改代码：分层、约定、门禁 | [`docs/conventions.md`](docs/conventions.md) · [`docs/architecture.md`](docs/architecture.md) |
| 每条业务做到哪一步、证据等级 | [`docs/business-matrix.md`](docs/business-matrix.md) |
| 单条任务的接口规格与 `path:line` | [`docs/task-inventory.md`](docs/task-inventory.md) |
| App 任务（25 条）逐条实现/验收状态 | [`docs/app-task-status.md`](docs/app-task-status.md) |
| 前端框架怎么用 | [`docs/web/lite-framework.md`](docs/web/lite-framework.md) |
| 全库审查结论（77 条） | [`docs/review/review-261004.md`](docs/review/review-261004.md) |
| 用户下达的每件事 → 落点 → 状态 | [`docs/task-log.md`](docs/task-log.md) |

---

## 能力

### 任务（20 条，现算：`mcloud tasks`）

| 分组 | 任务 |
|---|---|
| 签到族（8） | `sign` 每日签到 · `receive` 领取AI豆 · `multiple` 签到翻倍 · `backup-gift` 备份好礼 · `revival` 复活奖励 · `prizes` 待领取奖品 · `cloud-snapshot` AI豆快照 · `task-list` 任务清单 |
| 直播（2） | `live-room` 直播口令小红花（听弹幕 / 抓小红书口令 → 换小红花）· `red-flower` 领取小红花 |
| 兑换（1） | `exchange` 兑换奖品 —— **真扣 AI豆**，下单**不可逆**（界面上二次确认） |
| App 任务（3） | `app-task` 做 App 任务（上传 / 分享 / 固定入口）· `app-task-matrix` 只看矩阵（**纯只读**）· `task-expansion` 任务拓展（自己写一个文件、链尾再删掉） |
| 其他（6） | `cloud-phone-redpack` 红包派对 · `ai-avatar` AI 头像 · `msg-push` 消息推送 · `fun-ai` 趣玩AI抽奖 · `notice` 开启通知领AI豆 · `play-ai-special` 拍拍盘点（**纯只读**） |

> ⚠ **纯只读**（只拉读数、不改上游账本）：`task-list`、`prizes`、`cloud-snapshot`、
> `app-task-matrix`、`play-ai-special`。其余任务都会在上游留下动作 —— 注意
> **方法是不是 GET 不代表有没有副作用**（`receive` 就是 GET，但会记账）。
> `exchange` 花的是真 AI豆，是全仓唯一"发出去就收不回来"的一枪（上游没有幂等键）。
> 每条任务的实拍/推断依据见 [`docs/business-matrix.md`](docs/business-matrix.md)。

### WebUI（10 页）

总览 · 账号 · 任务 · 兑换 · 直播 · 定时 · 编排 · 历史 · 日志 · 设置
（页面清单的唯一来源是 [`web/src/ui/nav.ts`](web/src/ui/nav.ts)；路由是**干净路径** `/{page}`，
不带 `#` 号 —— 刷新与深链接都由服务端的 SPA 回落兜住。）

- **登录闸**：首次打开走初始化向导（建管理员 + 口令），之后用会话 cookie。
  三态：**未初始化 ⇒ 503**、**未登录 ⇒ 401**、已登录放行
  （[`crates/mcloud-server/src/routes/auth.rs`](crates/mcloud-server/src/routes/auth.rs)）。
- **账号登录**（与上面的管理员登录是两件事）：扫码 / 短信 / App 短信 / 账号密码 / 设备号轮换。
- **实时日志**：跑任务时走 SSE（`GET /api/runs/{id}/events`）；**跑完还能回看**完整日志
  （`GET /api/runs/{id}/logs?after=<seq>`）。
- **日志页**：整个进程的环形缓冲（含调度器与上游 http 那一层），`GET /api/logs` ——
  与"某次运行的日志"是两条不同的路。
- **定时**：cron + 时区 + 账号范围，也能直接触发一条编排。
- **编排**：多个任务串成一条链；**编排本身不绑账号**，账号由触发方（手动 / 定时）给。
- **通知**：渠道配在**每个云盘账号**上（pushplus / Server酱 / 企业微信 / 企业微信 Bot /
  Telegram Bot / Bark / 回逍 / 钉钉 / 自定义 POST / email）。email 本版**未实现**（会如实记）；
  **没配 = 一个包都不发**，不做进程级兜底（多用户下那是串号）。

## 快速开始（从源码）

### 0. 依赖

- Rust ≥ **1.88**（`Cargo.toml` 的 `rust-version`；CI 与本地日常跑 **1.91.1**）
- Node（只用来构建前端；CI 用 `lts/*`）+ npm（`web/package-lock.json` 已入库）

### 1. 构建 —— ⚠ 顺序有依赖

```bash
cd web && npm ci && npm run build && cd ..
cargo build --release --bin mcloud
```

`crates/mcloud-server/src/static_files.rs` 用 `rust-embed` 在**编译期**把 `web/dist`
读进二进制，而 `cargo build` **不跟踪** `web/dist` 的变化。顺序反了会内嵌旧产物
（表现：页面白屏 / chunk 404），而 `cargo build` 照样打 "Finished" 不报错。

### 2. 起服务

```bash
MCLOUD_ADDR=127.0.0.1:3000 MCLOUD_DATA_DIR=./data ./target/release/mcloud
```

打开 `http://127.0.0.1:3000` → 初始化管理员 → 「账号」页登录云盘账号 → 「任务」页跑任务。

| 环境变量 | 默认 | 说明 |
|---|---|---|
| `MCLOUD_ADDR` | `127.0.0.1:3000` | 必须是**完整的 `ip:port`** —— 只写 `3000` 会在解析层直接失败（`invalid socket address`），连监听都不会试 |
| `MCLOUD_DATA_DIR` | `data`（**相对当前目录**） | 所有落盘数据都在这儿；换目录 = 换一份库 |
| `RUST_LOG` | 服务 `info,mcloud=debug`；CLI `info` | 标准 `tracing` 过滤；`RUST_LOG=mcloud=trace` 能看上游请求/应答正文（**已脱敏**） |

### 3. CLI（同一个二进制）

```bash
./target/release/mcloud tasks                                # 列出可用任务
./target/release/mcloud run sign <账号-id>                    # 跑一次；账号 id 见 WebUI「账号」页
./target/release/mcloud run live-room <账号-id> --body '{"codes":"口令甲 口令乙","listenSeconds":45}'
./target/release/mcloud pending                              # 本进程里还没清理的网盘产物
./target/release/mcloud login qr|sms|app|password|device     # 登录云盘账号
./target/release/mcloud cron <定时-id>                        # 跑一条已配置的定时（给系统 crontab 调用）
./target/release/mcloud pipeline run <编排-id>
./target/release/mcloud auth refresh --all
```

⚠ `mcloud` **不带参数是"起服务"，不是"打印用法"**（托管那条命令行就是不带参数的）；
随手指一个未知子命令会打出用法文本并 `exit 2`。
定时与编排的**创建**不在命令行：用 WebUI（或改 `data/` 下的配置）。

## 开发

```bash
# 开发期：后端起在 38123，vite dev 把 /api 代理过去（生产形态本来就是同源托管）
MCLOUD_ADDR=127.0.0.1:38123 cargo run -p mcloud-server
cd web && npm ci && npm run dev

cd web && npm run typecheck   # tsgo --noEmit（类型检查只有这一套，不装 tsc）
cd web && npm run build       # 产物 web/dist（含 .gz 边车）
```

- **前端技术栈**：`web/lite/` 是自研框架（TSX 在构建期被折成"模板串 + 每个动态位一条
  effect"，运行时只留用到的原语）；CSS 是 Tailwind 4 的 CSS-first 配置，令牌层在
  [`web/src/app.css`](web/src/app.css) 的 `@theme`（**没注册的 token ⇒ 对应 utility 静默不生成**）；
  daisyUI 已移除。设计/踩坑全在 [`docs/web/lite-framework.md`](docs/web/lite-framework.md)。
- **多写入线**：`target/` 上的锁是串行的 —— 两条线同时编译只会排队。
  约定每线一个 `CARGO_TARGET_DIR="$PWD/target/lines/<线名>"`（详见 [`AGENTS.md`](AGENTS.md)）。
- **沙箱里 `~/.cargo` 不可写**时：`export CARGO_HOME="$PWD/.cargo-home"`（仓库已忽略该目录）。
- **macOS 自托管**（本机就是这条路，踩过的坑都在 [`AGENTS.md`](AGENTS.md)）：
  - 工作区在外置盘 ⇒ TCC 会弹「想访问可移除宗卷」；`scripts/run-local.sh` 把二进制与数据都放到内置盘跑；
  - 未签名的二进制按**内容哈希**认身份，每次重编都要重新授权 ⇒
    `scripts/dev-reload.sh`（先停托管 → 编译 → **固定身份签名** → 起）与 `scripts/sign-local.sh`。
    ⚠ 别 `kill` 托管进程、也别另起一个占 3000 的实例（会触发 KeepAlive 重启循环）。

## 门禁

```bash
scripts/gates-rust.sh --full        # fmt --check + clippy -D warnings + test --workspace
node scripts/gates-web.mjs          # = --commit：类型 + 构建 + 编译期负例 + demo/bench + 真页面交互
node scripts/gates-web.mjs --push   # 再加 320/375 窄屏、login/setup 两个认证态、前端泄漏量测
scripts/install-git-hooks.sh        # 装钩子：pre-commit 跑提交档、pre-push 跑推送档
```

- 两档的分界**不是"重要不重要"，而是"贵不贵"**：提交是高频动作，等两分钟就会有人
  `--no-verify`；泄漏量测那种"不看就发现不了"的挪到推送档。
- 跳过一律 `--no-verify`，**跳了就是没验证过**；没有浏览器时门禁会明说
  `skipped(无浏览器)` —— **跳过 ≠ 通过**。
- CI（[`.github/workflows/ci.yml`](.github/workflows/ci.yml)）只在 **PR 与手动触发**时跑
  （Actions 分钟数要花钱）；直接 push 到 `main` 没有兜底，所以本地交付前自己跑齐。

## 发布

打 `v*` tag ⇒ [`.github/workflows/release.yml`](.github/workflows/release.yml) 出四个平台各**一种**压缩包
（unix `tar.gz` / Windows `zip`，各附 `.sha256`），包内**只有 `mcloud` 一个文件**：

| 目标 | 备注 |
|---|---|
| `x86_64-unknown-linux-gnu` / `aarch64-unknown-linux-gnu` | arm64 走 `cross` + `vendored` OpenSSL（静态链） |
| `x86_64-pc-windows-msvc` | |
| `aarch64-apple-darwin` | macOS 不跑 UPX（会破坏签名） |

冒烟是**真起服务** + 轮询 `/api/health`，不是跑 `--help`。
`workflow_dispatch` 只出 artifact、不发 Release（方便先验一遍）。

## 数据与安全

```
data/                          ← MCLOUD_DATA_DIR（默认 ./data，不入库）
  accounts.json                云盘账号与凭据（**只存本地**）
  smid.json / state.json       设备指纹与会话附带状态
  users.json / sessions.json   管理员与浏览器会话（口令是 argon2id PHC 串）
  history.json                 运行历史（含每次运行的日志尾部）
  schedules.json pipelines.json task-config.json exchange-subscriptions.json
  kouling-pool.json            直播口令池
  logs/                        每次运行一份日志 JSON（`<run_id>.json`）
  *.bak                        覆盖前留的上一份（只从**非空**的旧文件留，不自动恢复）
```

- 凭证与口令**只落本地磁盘**；日志与通知正文过一遍脱敏
  （`mcloud_core::event::redact`：键名路标 + JWT/`Bearer` 裸形状兜底）。
- 会话 cookie 名 `mcloud_session`；管理员口令走 argon2id。
- 上游应答里带 HTML 之类的脏数据会在进记录前清掉（例如 App 任务名里的 `<span>`）。

## 目录结构

```
crates/
  mcloud-core        领域类型 + IO trait（零 IO）
  mcloud-crypto      签名与加解密原语（纯 Rust，零 FFI）
  mcloud-upstream    上游接口封装（唯一用 reqwest 的层）
  mcloud-engine      任务注册表 / 运行器 / 调度器 / 编排 / 事件
  mcloud-tasks       各任务的业务编排（20 条都在这里注册）
  mcloud-store       持久化（JSON + 原子写）
  mcloud-app         组装层（把上面几层拼成一个 App）
  mcloud-server      axum 路由 + SSE + 内嵌前端 + CLI（唯一二进制 `mcloud`）
web/
  lite/              自研前端框架（编译器 + 运行时 + 门禁夹具 + bench）
  src/               页面与组件（pages/ · ui/ · api.ts · app.css）
scripts/             gates-rust.sh · gates-web.mjs · dev-reload.sh · sign-local.sh · run-local.sh · hooks/
test/golden/         黄金向量（签名 / 口令哈希 / 认证 / 签到信封 / 直播），对拍是字节级
docs/                设计、约定、审查、任务状态与台账
```

依赖只能自上而下：`core ← crypto/upstream/store ← engine ← tasks ← app ← server`。
`core` / `crypto` / `engine` / `tasks` **禁止**直接做 IO（见 [`docs/architecture.md`](docs/architecture.md) §2）。

## 边界与已知缺口（别把"测试全绿"读成"真机验过"）

| 事 | 现状 |
|---|---|
| **直播弹幕那条链** | 主播开播那一小时才存在 WS。真机 `wss://…` **一次都没连过** ⇒「自动收集口令」最后一环（真弹幕）未验证（`data/history.json` 里的 `live-room` 至今没有一次"自动收集成功"的读数）；帧序 / `command` 取值 / UNREAD 等四条是**零实拍**。开播门本身**有实证** |
| **`msg-push` 的写** | 读那半实拍逐字；`task/obtain` 整条**零样本**（方法都没定，由 `MSG_PUSH_OBTAIN_IS_POST` 一处 A/B） |
| **`play-ai-special`** | 只做只读两口；写的那三口各自卡在"零实拍 / 语义未验证 / 会真写网盘" |
| **`fun-ai` / `notice` 的判据** | 部分计数与档位语义是推断的，实拍没命中过 |
| **`app-task`** | 上游 25 条里项目口径 **10/25**；判不出形状的条目**一枪都不发**（详见 [`docs/app-task-status.md`](docs/app-task-status.md)） |
| **`exchange`** | 真扣 AI豆且不可逆：没有 `deviceId` ⇒ 一枪不发；`skipWait=false` 但没接上等待实现 ⇒ 如实挡下，绝不静默往下走 |
| **文档自己** | 本 README 与 [`docs/usage.md`](docs/usage.md) 已按当前形态重写过一遍；其余文档的欠账见 [`docs/review/findings-261004-docs.md`](docs/review/findings-261004-docs.md)（19 条） |

> **端点必须来自抓包，不能凭印象写。** 本项目的签到端点最初凭印象写，实测回 **405**；
> 换成实拍的路径才 200。这类错误的产物是"能编译、能过测试、但打到一个不存在的地址"。

## 密码学（实现者向）

业务签名是 **MD5 + 固定盐**：

```
data = salt + id + timestamp + nonce + other + salt
sign = MD5(data)                      // 小写 hex
salt = { 1: "seedMdYYLIZfbCxg", 2: "sekaMdYYLIZfbCfm" }
```

请求头：`x-request-id` / `x-timestamp` / `x-nonce` / `x-signature`。

AES 有两套语义，**不可混用**：

| 场景 | 密钥处理 | 实际算法 |
|---|---|---|
| `rt.crypto.aesEncrypt`（固定 IV） | 截断/补零到 16 字节 | AES-128-CBC/PKCS7，输出 hex |
| 刷新令牌（动态 IV） | 用原始密钥 | 24 字节密钥 → **AES-192**，`base64(IV‖CT)` |

对应 API：`aes_encrypt_compat()` 与 `encrypt_with_dynamic_iv()`。
黄金向量在 [`test/golden/`](test/golden/)，对拍不一致就是 bug，**不许改期望值来变绿**。

## 注意

个人账号自用。不要用于批量注册、刷量或商业分发。
凭证只存本地，日志与推送正文已做脱敏。
