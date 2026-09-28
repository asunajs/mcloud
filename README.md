# mcloud-gost

移动云盘（中国移动云盘 / 139）自动化平台。后端 **Rust**，前端 **Vue Vapor + TSX**。

> 设计详见 [`docs/architecture.md`](./docs/architecture.md)。

## 状态

**已落地 16 个任务**：P1 签到族八条（`sign` / `receive` / `multiple` /
`backup-gift` / `revival` / `prizes` / `cloud-snapshot` / `task-list`）
+ 直播小红花两条（`live-room` / `red-flower`）
+ 兑换（`exchange`）+ 消息推送（`msg-push`）
+ 趣玩AI抽奖（`fun-ai`）+ 公告任务（`notice`）
+ 拍拍盘点（`play-ai-special`，**只读**）
+ App 任务矩阵（`app-task-matrix`，**只读**）
+ 任务拓展（`task-expansion`）。

**`task-expansion`（2026-09-28 落地）—— 第一条"写网盘并自己收尾"的任务**：

它是旧项目里**唯一**"为了改变上游状态而写用户网盘"的任务：本月没开备份时，
不是提示你去开，而是**自己传一个文件**把 `curMonthBackup` 刷成 true。

> ⚠⚠ **与参考实现的一处刻意不同：本仓跑完会删掉自己传的那个文件。**
> 参考实现这一条链**不删**（留在它的 P1 缺口里），跑一次你的网盘里就多一个没人清理的文件。
> 本仓有 `batchTrash`（2026-09-26 起有真跑读数）⇒ 链尾自己收尾。
>
> ⚠⚠ **但"撞同名"那一档绝不删**：`exist:true` 时上游按 `refuse` 语义把**你本来就有的
> 那个文件**的 `fileId` 交回来 —— 把它放进清理名单就是**删你的东西**。
> 所以那一档**连 id 都不交**（有测试钉住"撞同名时一条 `batchTrash` 都不发"）。
>
> ⚠ 删除是**移进回收站**、不是彻底删；真删错了还能捞回来。
>
> 另外两处"刻意"：**一枪没发出去时不空等**那 20 秒（等的是"服务端把这次备份记进当月"，
> 没有上传就没有东西要记），以及**领取成功但上游没报数时仍然报"已领"**
> （旧实现那个 truthy 判据会让那一趟一句都不念，看起来像什么都没发生）。

**`app-task-matrix`（2026-09-28 落地）—— 只拉表，不打卡、不上传**：

16 次只读请求 = 2 个市场名（`sign_in_3` / `newsign_139mail`）× 8 个 group
（`new` / `beiyong1` / `beiyong2` / `cloudEmail` / `time` / `day` / `month` / `hidden`），
然后按「按 `id` 升序 → 按 `name` 去重保留先来的 → 没名字的各自保留」聚合。

> ⚠⚠ **它的名字不叫 `app-task`，是故意的**。参考实现里 `app-task` 的含义是
> **「做任务」**（逐条 `task/click`，会写用户的账且不可撤销），而且它自己写着
> **「今天一次真打都没有」**、`RunInBatch: false`，并指明"手工点一次是拿到第一个
> 实拍的最快路径"。拿同一个 code 实现成只读，会让那个名字在两边指不同的东西
> ⇒ 这里另起一个名，**把写的那一半原样留给 `app-task`**。
>
> ⚠ 上游那一族另有两口是**写**、且**都零实拍**：`task/click`（写账）与上传三枪
> （`file/create` → `PUT` → `file/complete`，真往用户网盘写 `asign-*.png`），
> 收尾那一枪 `batchTrash` 同样零实拍 ⇒ 本仓一个都不发。
>
> **失败策略**：某组挂了只**登记**、继续拉（照旧实现）；⚠ 但**十六次全挂 ⇒ 报错** ——
> 旧 TS 那种"全失败仍回一张空表"会让"全没读到"看起来像"这个号没任务"。

**`play-ai-special`（2026-09-28 落地）—— 只做只读那一半，另三口各自卡在不同地方**：

| 口 | 状态 |
|---|---|
| ① `GET newyear/user/info`（次数读数） | ✅ 实拍逐字，`0 → 1` 两档都有 |
| ② `GET newyear/task/list`（任务表） | ✅ 实拍逐字，两次同一 URL（差别只在 `id:1` 那条的响应） |
| ③ `POST task/step/click`（**写**） | ⛔ 旧实现把 `visitlog/journaling` 埋点当**门槛**，而那两条埋点**零实拍** |
| ④ `POST blindbox/lottery`（**写**） | ⛔ 判据 `totalLotteryCount` **语义未验证**（那句 `1` 出现在**抽奖之前**），且不可回滚 |
| ⑤ `POST aiRecognize`（**写网盘**） | ⛔ `uploadToCloud` 实拍恒 `true` ⇒ 真往用户网盘写图，清理链今天冻结 |

> ⚠ 这一族的请求头**比仓库其余 H5 接口少两个**（**没有** `appversion`、`activityid`、
> `deviceid`）—— 同归档里活动门户那四条**都带**这三个头 ⇒ 是这一族真的没发。
> 所以它**不复用** `h5()`。顺手复用会发出一条**夹具里不存在的请求**，而上游对多余头的反应**没有样本**。
>
> ⚠ 这一页的票走**公共来源**（特有换票链有出处但**零实拍**）；**Referer 不借**，
> 用活动落地页那一串（B 档）。

**`fun-ai` / `notice`（2026-09-28 落地）的证据边界**：

| 任务 | 实拍的 | 零样本 / 未验证的 |
|---|---|---|
| `fun-ai` | 两条请求（`page/getmodule` 裸 GET + `page/lottery` body 只有 `moduleId`）、三个计数与活动期形态、**中奖与没中奖两档**响应 | ⚠ 三个计数**跨天清不清零**；⚠ 转盘与刮刮乐（`timed/lottery`）**互不改写账本**但只验过转盘这一口 |
| `notice` | 四条请求（`getTaskInfo` ×2 → `reward`，另一场是 `receiveTask?taskId=1021`） | ⚠⚠ **判据映射是猜的**：`cloudStatus` 哪个值是"未领"**没有正面样本**；`noticeNeedsManual` **恒 false**（那一档今天**不可达**）；实拍里**没有** `taskListV2` |

> ⚠ `fun-ai` 的"每天抽一次"是**本工具**的取向（`FUNAI_DAILY_DRAW_LIMIT = 1`），
> **不是**上游规则 —— 上游那条"每日最多 5 次"说的是**做任务能挣到几次**，不是能抽几次。
> 这是刻意比上游可能允许的更少：不拿写请求去试探额度。

**`msg-push`（2026-09-28 落地）的证据分两半 —— 别混着读**：

| 那一半 | 状态 |
|---|---|
| `task/status`（URL / 方法 / 六个字段名） | ✅ 实拍逐字（2026-09-25 进页抓包） |
| `task/list`（形状） | ✅ 实拍逐字，但**没接进任务链**（`obtain` 之后列表变不变零样本） |
| 这一页**自己的建会话参数** | ✅ 实拍到了（`msgpushon` / `1062`），但**没有实现** ⇒ 仍走公共来源 |
| **`task/obtain`（那一枪写请求）** | ⚠ **整条零样本**：实拍那天 `secondTaskStatus=1`，页面上没有领取入口 |
| **"什么值该做什么"那层判据** | ⚠ **仍是旧项目口径**，实拍一条都没命中过 ⇒ 判据未被上游证实 |

> 所以：`msg-push` 的**读**是实证的，**写**是"原项目实现、零实拍"。
> 别把"测试全绿"读成"这一枪真机跑通过" —— 它的方法（GET/POST）都还没定，
> 常量在 `mcloud_upstream::app_msg_push::MSG_PUSH_OBTAIN_IS_POST`，A/B 只改那一处。

**直播小红花（2026-09-26 落地）**的诚实边界 —— 读代码前先读这段：

| 层 | 状态 |
|---|---|
| 口令抽取 / MSync 编解码 / `/cmlive` / 六步链 | ✅ 黄金向量对拍通过（字节级 / 逐字） |
| 开播门（换 SSO → 换直播侧 JWT → 房间列表） | ✅ **实证**（抓包文档 §5.3 走通过）+ 真机确认会打上游 |
| **自动获取**：不填 `codes` ⇒ 开门 → 听弹幕获取口令 → 领取小红花 | ✅ 编排端到端单测（假 HTTP + 假收集器） |
| WebSocket 连接层 | ✅ 离线假服务端测试通过（`duplex` + 真 TCP 各一条） |
| 注册、CLI、HTTP API、WebUI | ✅ 端到端真机跑通 |

⚠ **没有真实开播窗口，以下都没跑过**（不是"应该能"，是**没验证**）：

- 真实 `wss://videoim.cmcc-cs.cn:20102` **一次都没连过**（未开播时 WS 结构性不存在）
  ⇒ **「自动收集口令」这条链的最后一环（真弹幕）从未真机跑过**；
  上游那一段用假收集器替换，验的是**编排**（开门 → 获取 → 领取 → 汇总），不是"对面认不认我们"
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
| P9b | **P2 只读查询那一档全部落完**：`exchange` + `msg-push` + `fun-ai` + `notice` | 已落 4 条 |
| P9c | 其余任务增量接入（`app-task` **写的那一半** / `cloud-phone-redpack` / `ai-avatar`）+ 推送渠道 | 待做（已落：`play-ai-special`、`app-task-matrix` 的只读那一半，以及 `task-expansion` 全链）|

> **端点必须来自抓包，不能凭印象写。** 本项目的签到端点最初是
> `/ycloud/api/sign/query`（凭印象），实测回 **405**；换成实拍的
> `/ycloud/signin/page/infoV3` 后回 **200**。这类错误的产物是
> "能编译、能过测试、但打到一个不存在的地址"，比不写更糟。

## 注意

个人账号自用。不要用于批量注册、刷量或商业分发。
凭证只存本地，日志已做脱敏（`mcloud_core::event::redact`）。
