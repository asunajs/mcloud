# mcloud-gost

移动云盘（中国移动云盘 / 139）自动化平台。后端 **Rust**，前端 **Vue Vapor + TSX**。

> 设计详见 [`docs/architecture.md`](./docs/architecture.md)。

## 状态

**已落地 10 个任务**：P1 签到族八条（`sign` / `receive` / `multiple` /
`backup-gift` / `revival` / `prizes` / `cloud-snapshot` / `task-list`）
+ 直播小红花两条（`live-room` / `red-flower`）。

**直播小红花（2026-09-26 落地）**的诚实边界 —— 读代码前先读这段：

| 层 | 状态 |
|---|---|
| 口令抽取 / MSync 编解码 / `/cmlive` / 六步链 | ✅ 黄金向量对拍通过（字节级 / 逐字） |
| 开播门（换 SSO → 换直播侧 JWT → 房间列表） | ✅ **实证**（抓包文档 §5.3 走通过）+ 真机确认会打上游 |
| **自动收集**：不填 `codes` ⇒ 开门 → 听弹幕 → 投口令 → 领花 | ✅ 编排端到端单测（假 HTTP + 假收集器） |
| WebSocket 连接层 | ✅ 离线假服务端测试通过（`duplex` + 真 TCP 各一条） |
| 注册、CLI、HTTP API、WebUI | ✅ 端到端真机跑通 |

⚠ **没有真实开播窗口，以下都没跑过**（不是"应该能"，是**没验证**）：

- 真实 `wss://videoim.cmcc-cs.cn:20102` **一次都没连过**（未开播时 WS 结构性不存在）
  ⇒ **「自动收集口令」这条链的最后一环（真弹幕）从未真机跑过**；
  上游那一段用假收集器替换，验的是**编排**（开门 → 收 → 投 → 汇总），不是"对面认不认我们"
- 帧顺序 / `command` 取值 / UNREAD 是否必需 / `h` 是否要回 `h`
  —— 这四条证据等级是**「原项目实现，零实拍」**，
  测试只能证明"我们按那个假设实现了"，**证明不了假设成立**
- `200112`（已兑换）语义仍**零实拍**；`draw` 的重放边界**待验证**
- TLS 只验到"依赖同源"，未验真实证书链

> 开播门本身**有实证**（`docs/capture/app-live-redflower-samples.txt` §5.3：
> `querySpecTokenV2(017027)` → `cmpassLoginByCmToken` ⇒ JWT，三步都走通过，
> 因为门第③步是 GET、不需要开播）。**别把"WS 没验过"误读成"门也没验过"。**

上游接口大部分尚未接入真实抓包（见「路线图」）。

## 目录

```
crates/
  mcloud-core        领域类型 + IO trait（零 IO）
  mcloud-crypto      签名与加解密原语（纯 Rust，零 FFI）
  mcloud-upstream    上游接口封装（唯一用 reqwest 的层）
  mcloud-engine      任务注册表 / 运行器 / 并发闸 / 事件
  mcloud-tasks       各任务业务编排
  mcloud-store       持久化（JSON + 原子写）
  mcloud-app         组装层
  mcloud-server      axum 路由 + SSE + CLI
web/                 Vue Vapor + TSX + daisyUI
scripts/             门禁脚本（gates-web.mjs）
test/golden/         权威黄金向量（对拍用，从 mcloud 同步）
docs/                设计文档
```

> 本仓库只放**干净的实现代码**。抓包记录、逆向素材、客户端分析
> 都在上游仓库 [`mcloud`](../mcloud/)（`har/`、`test/golden/`、
> `test/fixtures/`、`scripts/`），本仓库不复制这些内容。

依赖只能自上而下：`core ← crypto/upstream/store ← engine ← tasks ← app ← server`。
`core` / `crypto` / `engine` / `tasks` **禁止**直接做 IO。

## 构建与运行

```bash
# ⚠ 构建顺序有依赖：前端产物在**编译期**被 rust-embed 内嵌，
#   所以改过前端必须先 build 前端、再编译后端，否则二进制里还是旧产物。
cd web && npm install && npm run build && cd ..
cargo build --release
cargo test --workspace

# 跑服务（默认 127.0.0.1:3000）
MCLOUD_ADDR=127.0.0.1:38123 MCLOUD_DATA_DIR=./data cargo run -p mcloud-server

# CLI（收敛后：tasks / run / cron / pipeline / auth / login）
cargo run -p mcloud-server --bin mcloud-cli -- tasks
cargo run -p mcloud-server --bin mcloud-cli -- run sign <account-id>   # 账号 id 见 WebUI「账号」页
cargo run -p mcloud-server --bin mcloud-cli -- cron <schedule-id>     # 供外部触发，CLI 自身不常驻
cargo run -p mcloud-server --bin mcloud-cli -- auth refresh --all
cargo run -p mcloud-server --bin mcloud-cli -- login app <手机号>      # App 端验证码登录（另见 login sms）

# 前端
cd web && npm install && npm run dev     # 开发（代理到 38123）
cd web && npm run build                  # 产物在 web/dist
```

> `CARGO_HOME` 说明：若 `~/.cargo` 不可写（沙箱环境），设置
> `export CARGO_HOME="$PWD/.cargo-home"` 即可，仓库已忽略该目录。

## HTTP 接口

```
GET  /api/version
GET  /api/health
GET  /api/capabilities          任务清单
GET  /api/runs                  最近运行
POST /api/runs                  启动运行 {task, account, body}
GET  /api/runs/{id}             单次运行详情
GET  /api/runs/{id}/events      ★ SSE 事件流
```

SSE 事件示例：

```
event: started
data: {"type":"started","run_id":1,"task":"sign","account":"138...","at_ms":...}

event: progress
data: {"type":"progress","run_id":1,"step":1,"total":2,"label":"查询签到状态"}

event: finished
data: {"type":"finished","run_id":1,"outcome":{"status":"success","summary":"..."},"duration_ms":159}
```

## 密码学

业务签名是 **MD5 + 固定盐**：

```
data = salt + id + timestamp + nonce + other + salt
sign = MD5(data)          // 小写 hex
salt = { 1: "seedMdYYLIZfbCxg", 2: "sekaMdYYLIZfbCfm" }
```

请求头：`x-request-id` / `x-timestamp` / `x-nonce` / `x-signature`。

全部是标准算法，**纯 Rust 实现，无 FFI、无 native 依赖**。
算法的来源与推导记录在上游 `mcloud` 仓库，本仓库只承载实现。

### AES 的两套语义（易踩坑）

原本并存两种行为，**不可混用**：

| 场景 | 密钥处理 | 实际算法 |
|---|---|---|
| `rt.crypto.aesEncrypt`（固定 IV） | 截断/补零到 16 字节 | AES-128-CBC/PKCS7，输出 hex |
| 刷新令牌（动态 IV） | 用原始密钥 | 24 字节密钥 → **AES-192**，`base64(IV‖CT)` |

对应 API：`aes_encrypt_compat()` 与 `encrypt_with_dynamic_iv()`。

## 测试

- `cargo test --workspace` —— **715 个测试**
- `cargo clippy --workspace --all-targets -- -D warnings` —— 0 告警
- 黄金向量对拍：`test/golden/crypto-vectors.json`（签名、MD5、SHA256、两种 AES 语义）
  + `test/golden/app-signin-vectors.json`（签到族实拍信封与常量）
- 对拍是**字节级**的；不一致就是 bug，不允许改期望值来变绿。

## 路线图

| 阶段 | 内容 | 状态 |
|---|---|---|
| P0 | 设计定稿 + 分层脚手架 | ✅ |
| P1 | 密码学 + 黄金向量对拍 | ✅ |
| P2 | axum 路由 + SSE 事件流 | ✅ |
| P3 | Vue Vapor WebUI（主题 + 门禁） | ✅ |
| P4 | `rust-embed` 单二进制 / 同源托管 | ✅ |
| P5 | **通用存储 + 定时调度 + 任务编排 + 日志收集 + 任务中止** | ✅ |
| P6 | **H5 会话层（JWT）+ 签到端点校正** | ✅ |
| P7 | **WebUI 登录闸 + 认证闸/登出 + CLI 收敛（6 命令）** | ✅ |
| P8 | **账号登录（扫码/短信）+ 账号页 + 定时/编排 CRUD 页** | ✅ |
| P9a | **签到族 8 条**（`sign` / `receive` / `multiple` / `backup-gift` / `revival` / `prizes` / `cloud-snapshot` / `task-list`） | ✅ |
| P9b | 其余任务增量接入（`notice` / `app-task` / 活动类 等）+ 推送渠道 | 待做 |

> **端点必须来自抓包，不能凭印象写。** 本项目的签到端点最初是
> `/ycloud/api/sign/query`（凭印象），实测回 **405**；换成实拍的
> `/ycloud/signin/page/infoV3` 后回 **200**。这类错误的产物是
> "能编译、能过测试、但打到一个不存在的地址"，比不写更糟。

## 注意

个人账号自用。不要用于批量注册、刷量或商业分发。
凭证只存本地，日志已做脱敏（`mcloud_core::event::redact`）。
