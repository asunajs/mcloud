# mcloud-sign Rust 重写方案

> 状态：**提案（写于动工之前）**。本文是 P0 开工前的规划快照，自 `mcloud-sign`
> 仓库同步而来；**实施进度以本仓库 [`status.md`](./status.md) 为准**（门禁见 [`dev-gates.md`](./dev-gates.md)），本文不再更新
> （同步进来的原因：原文档悬空引用了它，而且原仓库是公开的，里面不该留带真实标识的内部文档）。
> 目标项目位置：`/Users/catlair/mobile/workspace/mcloud`（与 `auto-flow` / `macro-recorder` / `mold-management` 同级）。
> 取代：`docs/REFACTOR_PLAN_20260916.md`（该文基于 TS→TS 渐进优化 + 复用 Vue 前端，已被本方案取代）。
> 参考项目：[3238614968/caiyun](https://github.com/3238614968/caiyun)（Go + Vue3），clone 于 `/tmp/caiyun-ref`。
> 日期：2026-09-16

---

## 0. 摘要

### 0.1 已确定的前提

| 项 | 决定 |
| --- | --- |
| **语言** | 后端全部 Rust（Cargo workspace） |
| **位置 / 名称** | `/Users/catlair/mobile/workspace/mcloud`（新项目名 `mcloud`） |
| **存储** | **SQLite**（`rusqlite` bundled） |
| **txiki.js** | **删除**，不再保留双运行时 |
| **前端** | **不沿用现有 Vue 页面**，改为 **Vue Vapor（无虚拟 DOM）+ TSX + daisyUI**，移动端兼容 |
| **多账号** | **一等公民**（每个账号独立状态、独立余额、独立调度、独立并发控制），不降级、不裁剪 |
| **用户体系** | **多用户，系统自有账号**。用户用自己的账号登录**本系统**（与"移动云盘账号登录"是两件事）；每个系统用户各自管理属于自己的移动云盘账号，**数据按用户隔离**。旧项目**完全没有这一层**（旧 API 无任何鉴权中间件、CORS 全开） |
| **交付形态** | **两个独立产物**：`mcloud-cli`（纯 CLI，不含 WebUI、**不依赖** HTTP 服务框架）与 `mcloud-server`（API + 内嵌 WebUI）。两者各自成型、互不拖动 |
| **首个功能** | **移动云盘账号登录**：通过 CLI 或 WebUI 完成**短信登录**与**扫码登录**，拿到 `auth`（一切上游接口都依赖它，因此排在所有接口之前）。旧项目**零实现**，无参考代码 |

### 0.2 一句话目标

> 用 **trait 注入 IO** 让业务逻辑能被函数级单测（无网络、无数据库即可穷尽验证）；把 19 个能力变成**声明式注册表**，让前端表单一律由 schema 生成；补上**系统自有用户体系**（多用户、数据隔离）与**账号云朵余额 + 日快照**这两条缺失的维度；上游接口**不照抄旧 TS**，而是按**重新抓取的真实流量**逐个重写；前端用 **Vapor + TSX + daisyUI** 全量重写，纯 CSS 类、无 VDOM 依赖。

### 0.3 为什么这次重写风险可控

重写最容易死的地方是"功能看起来搬完了，但请求暗中发错了"。本方案用两把锁解决：

1. **前端不存在 VDOM 依赖**：选 **daisyUI**（纯 CSS 类名，**无 JS 组件**）而不是 JS 组件库，Vapor 树里没有任何需要 interop 的"虚拟 DOM 孤岛"，`<TTable>` 那类插槽跨界问题根本不会出现。`vue-jsx-vapor` 本身同一份 TSX 既能编 Vapor（`defineVaporComponent`）也能编 VDOM（`defineComponent`），留作兜底。
2. **请求逐字节对拍**：**以重新抓取的真实流量为基准** —— 夹具里连"请求那一侧"（method / URL / body）一起录下来了，Rust 端口必须产出除时间戳/随机数外**完全一致**的请求。这是保证重写不静默跑偏的唯一可靠手段（详见 §6.3）。

> 对拍基准从"TS 原版录制"改成"新抓包"是有原因的一条实测结论：旧 TS 与线上实际请求**本就不一致**（详见 §6.3 的差异表），照抄 TS 会得到一份"能编译、跑起来打错地方"的实现。

---

## 1. 现状资产盘点（要搬的东西）

### 1.1 规模

| 包 | 文件 | 行数 | 去留 |
| --- | --- | --- | --- |
| `packages/shared` | 34 | 3,348 | **部分搬**：类型、http 基座的"按域名注入凭据 + CookieJar"语义、smid 算法；**logger / push / 配置**按 §1.6 重做 |
| `packages/core` | 84 | 9,367 | 19 个能力的**业务逻辑**要搬；**12 个上游 API 封装不搬** —— 按新抓包重写（§7.1） |
| `packages/runtime` | 10 | 979 | **删**（见 §2.3） |
| `packages/cli` | 4 | 356 | **重写**：旧 CLI 其实没有命令面（§1.6），clap 子命令从零设计 |
| `packages/api` | 30 | 2,283 | **重写**：旧 Hono 路由平铺且**无鉴权**；axum 重写并加用户体系（§3.7） |
| `packages/webui` | 7 页 | — | **重写**（Vapor + TSX + daisyUI）。**不参考旧前端实现**，控制粒度不同（§1.6、§4.1） |

### 1.2 能力清单（19 个已注册任务）

`sign`(签到) / `receive`(领取) / `revival`(复活) / `task-expansion`(任务扩展) / `msg-push`(消息推送) / `notice`(公告) / `backup-gift`(备份礼物) / `app`(App任务) / `ai-avatar`(AI头像) / `play-ai-special`(拍拍系列) / `play-ai-lottery`(抽奖) / `cloud-phone-redpack`(云手机红包) / `hc1t`(合成云朵) / `kouling`(直播口令) / `red-flower`(小红花) / `exchange`(兑换，`hidden`) / `prizes`(查询) / `auth-refresh`(认证刷新) / `after`(收尾)

> 注：`playAiSpecialTask` 是拍拍系列通用实现，春日/夏日/秋日拍拍只是不同期次名称，**不是配置不一致**。微信签到 `signInWx`、微信抽奖 `wxDraw`、摇一摇 `shakeTask` 已过期，**不搬**。
>
> **范围说明（写文档时的常见混淆）**：上面 19 项一律是**云盘业务内容** —— 包含 `kouling`（直播口令，含小红书 HTML 解析）、`ai-avatar` / `play-ai-special` / `play-ai-lottery`（AI 玩法）、`cloud-phone-redpack`（云手机红包）等。它们**不是"周边"**。与云盘无关的工程面（前端布局、后端设计、日志、推送通道、定时逻辑、CLI、配置）另见 §1.6。
>
> 另注：`packages/core/src/apis/mail139.ts`（339 行）在 19 个任务里**找不到消费方**，属"有封装、无任务"，其去留见 §1.6 的待拍板项。

### 1.3 加密原语（重写必须逐一对接）

| 原语 | 现状位置 | 用途 | 精确参数（重写必须逐字对齐） | 可对拍性 |
| --- | --- | --- | --- | --- |
| MD5 | `runtime/src/crypto.ts`、`hc1t` 签名 | 接口签名 | UTF-8 入参 → hex | ✅ 字节级 |
| SHA256 | `runtime/src/crypto.ts`、`core/src/utils/file.ts` | 文件内容哈希 | UTF-8 入参 → hex | ✅ 字节级 |
| HmacSHA256 | `shared/src/push/index.ts:240` | 钉钉加签 | key/msg 均 UTF-8，**输出 base64**（不是 hex） | ✅ 字节级 |
| AES-CBC/PKCS7 固定 IV | `runtime/src/crypto.ts` | 设备指纹 smid 载荷 | key=`substring(0,16).padEnd(16,"\0")`，IV=`0102030405060708`，输出 **hex** | ✅ 字节级 |
| AES-CBC/PKCS7 动态 IV | `core/src/utils/encrypt.ts` | refreshToken 包裹 | key **原样不截断**（`REFRESH_TOKEN_KEY` 24 字节 → 实为 **AES-192**），IV 随机 16 字节，输出 `Base64(IV‖CT)` | ⚠ 加密随机；固定 IV 时字节级，另加"TS 密文 → Rust 解密"单向验证 |
| AES-128/256-CBC | `live-room/proto.ts` | 直播间消息体 | 待 P3 定 | 待定 |
| DES-ECB/**NoPadding** | `shared/src/smid/index.ts:244` | 设备指纹 smid | key=`substring(0,8).padEnd(8,"\0")`；**明文按 JS 字符数补 `\0` 到 8 的倍数**，输出 base64；空串 → 空 base64 | ✅ 字节级（仅 ASCII 输入；含 CJK 时"字符数≠字节数"是原实现缺陷，见下） |
| DES-ECB/PKCS7 | `runtime/src/crypto.ts` | smid 响应解密 | base64 进、UTF-8 出 | ✅ 字节级 |
| RSA PKCS#1 v1.5 公钥加密 | `runtime/src/tjs.ts:171`(纯 JS)、`core/src/apis/ai-workbench.ts:106` | 加密 token 查询 | key 为 SPKI DER(base64)，1024 位，输出 128 字节 base64 | ❌ **不可字节级对拍**（PKCS#1 v1.5 加密填充含随机非零字节，已实测同一明文三次输出不同） |
| gzip（OS 字节改 19） | `shared/src/smid/index.ts:256` | smid 载荷压缩 | 压缩后强制 `buf[9]=19`，输出 base64 | ⚠ 不做字节级（deflate 实现差异）；断言"可解回原 JSON + OS 字节==19" |
| randomUUID / randomBytes | `runtime/src/node.ts:85` | 设备 ID / 随机数 | UUID v4、CSPRNG | — 见 §3.2 的 `Random` trait |

> **两个实测结论（本次新增，都是"想当然会对拍失败"的坑）**：
> 1. **RSA 不能字节级对拍**。PKCS#1 v1.5 **加密**（Type 02）填充要求随机非零字节，所以它是随机的 —— 已用同一公钥对同一明文加密三次，输出全不同。正确做法：另生成一对自有测试密钥，用"Node 加密 → Rust 私钥解回明文"和"Rust 加密 → Rust 自解"双向验证实现正确性；对 skland 真实公钥只断言结构（输出 128 字节、密文整数值 < n）。
> 2. **gzip 的 `buf[9]=19` 是"归一化"而非魔改**。Node 的 `CompressionStream("gzip")` 产出的 OS 字节本来就是 19。该行的真实作用是抹平不同运行时（txiki.js）的差异。Rust 侧（`flate2` 默认 OS 字节不是 19）必须**显式**写 19。
>
> 另注：`encryptDes` 的补零按 **JS 字符串长度**（UTF-16 码元数）计算，再交给 `enc.Utf8.parse`；纯 ASCII 下等价于按字节补零，但含 CJK 时会导致字节数非 8 的倍数（CryptoJS `NoPadding` 下产生截断/异常）。**重写时保持 ASCII 行为，把 CJK 作为已知边界单独测试**，不要"顺手修好"——否则生成的 smid 与线上不一致。
>
> **命名澄清（避免把设施误当业务功能）**：`shared/src/smid/index.ts` 里的 `SKLAND_SM_CONFIG`（`organization` / `appId` / `publicKey` / `apiHost: slw.h5cmpassport.com:9090`）**不是**某个独立业务功能 —— `skland` 只是这套**设备指纹（smid）**配置沿用的名字。它的实际用途是：生成随机 `uid` → 用该 RSA 公钥包裹 → 连同浏览器特征一起上报给**中国移动统一认证（cmpassport）的风控服务**。属**登录链路**，`apiHost` 指向 cmpassport，与云盘业务无关但与登录强相关，重写登录时必须一并实现。

### 1.4 协议与传输

- **protobuf**：`core/src/live-room/proto.ts` 内嵌 `PROTO_DEF`（proto3 字符串，**无独立 .proto 文件**），仅 `cmos-websdk` 使用。
- **WebSocket**：`live-room`（长连接）+ `api` 口令聚合。
- **推送渠道**：实为 **10 类**（`shared/src/push/index.ts` 的 `CHANNEL_MAP`）：`pushplus` / `serverChan`(Server酱) / `workWeixin`(企业微信应用) / `workWeixinBot`(企业微信机器人) / `tgBot` / `bark` / `twoIm`(回逍) / `dingTalk`(加签) / `email`(SMTP) / `customPost`(通用，支持 `{title}{text}` 占位符)。配置来源是**全局**配置（`AppConfig.message`），不是每任务。旧实现**无重试**、仅 10s 超时（fire-and-forget）。

### 1.5 已知缺陷（重写必须顺带解决）

| ID | 问题 | 证据 |
| --- | --- | --- |
| D1 | **云朵余额未采集**：`totalCloud` 写入被注释 | `core/src/tasks/sign/task.ts:45` → `// ctx.state.totalCloud = total;` |
| D2 | 无余额历史、无日/周增量 → 前端无法回答"这账号还有多少云朵" | `UserState` 有 `totalCloud?` 但无快照表 |
| D3 | 能力无声明式描述 → 前端表单按任务硬编码，新增任务要改前端 | `webui/src/pages/TasksPage.vue` |
| D4 | 运行历史仅内存（重启丢），长任务无法实时看进度 | `api/src/runner.ts` 的 `AsyncRunState` |
| D5 | 无 API 契约，前后端类型手工同步易漂移 | `webui/src/api.ts` 手写 |
| D6 | 36 个真实 HAR 完全未用于测试；api/webui 零测试 | `har/*.har`、`vitest --passWithNoTests` |
| D7 | **无鉴权、无用户概念**：`/api/*` 全部裸露，CORS 对所有来源放开，账号只是配置数组下标 | `api/src/index.ts` 无任何 auth 中间件；`cors()` 无参数 |
| D8 | 配置 schema 写了但**运行期从不校验**；`auth` **明文落盘** | `shared/src/config/loader.ts` 只 `JSON.parse`；`api/src/index.ts` 的 `PUT /api/config` 只判"非空对象" |
| D9 | CLI **没有命令面**：`bin` 只接一个可选配置文件路径，无参数解析、无交互输入 | `cli/src/index.ts` 不读业务参数；无 readline |
| D10 | 推送 **fire-and-forget**：无重试、失败只打日志 | `shared/src/push/index.ts` 的 `sendRequest`（10s 超时、retry 0） |
| D11 | 日志**非结构化**且脱敏只覆盖 URL query（body / header 明文照进日志） | `shared/src/logger/index.ts` 的 `redact()` 只匹配 query |

---

### 1.6 工程面周边盘点（与云盘无关；去留见文末待拍板项）

口径：这一节**只收工程面** —— 它们不实现任何云盘业务，但决定项目能不能用、好不好维护。19 个云盘能力在 §1.2，不要混进来。

| 工程面 | 旧项目位置与实际做法 | 新方案 | 说明 |
| --- | --- | --- | --- |
| 前端布局 | `webui/src/`：Vue + TDesign，**无 vue-router**（`App.vue` 用变量 + `v-if` 链切页）、**无 store**（每页各自 `onMounted` 取数）、**无公共组件层**，界面偏好存服务端 `data/webui.json` | **← 不参考旧实现**。两者的**控制粒度不一样**：旧页面是"几个大块拼一起"，新前端要做到字段级/操作级的粒度（表单由 `FieldSpec` 生成、状态与错误逐项可控）。旧前端**只用来确认功能范围**（要有哪些页面、每页要能做什么），实现方式一律自行设计 | 技术栈与粒度见 §2.2、§4 |
| 后端设计 | `api/src/`：Hono，路由在 `index.ts` **平铺注册**（无集中路由表）；参数校验很薄（`readBody()` + 少量手写判断），**无统一 schema 中间件**；`registry.ts` 只登记任务，`runner.ts` 管互斥 + 执行 + 落库 + 写历史 | **重做**：axum 路由 + 统一校验与错误映射；**保留** `runner` 的"任务+账号互斥"语义（防重复请求、定时任务据此跳过）与"提交返 runId + 轮询"形态 | 见 §3.5 |
| 日志设计 | `shared/src/logger/`：5 级（debug/info/warn/error/fatal）+ 语义化 type；**仅控制台**（无文件 transport）；**纯文本**拼接；脱敏只覆盖 URL query | **重做**：结构化日志（字段化，可落库检索）+ 文件/滚动输出；脱敏覆盖 body/header，**与夹具脱敏共用同一套规则**（§6.2） | 旧实现 Transport 可插拔 + scope 前缀的设计可借鉴 |
| 推送通道 | `shared/src/push/`：10 类渠道（§1.4），统一 `sendRequest`（10s 超时、**无重试**），配置来自全局 `AppConfig.message`；邮箱走 nodemailer，tjs 运行时直接跳过 | **渠道集保留**、**发送层重做**：补重试、超时、失败可见；渠道分发沿用字典式 `(ctx, config, title, text)` 签名 | 属 P2 剩余项 |
| 定时逻辑 | `api/src/scheduler.ts` + `exchange-schedule.ts`：croner，支持 **5/6 段（含秒）**，默认 `Asia/Shanghai`；定义落 `data/schedules.json`，**启动自动重注册**；同一"任务+账号"在跑则**静默跳过**；`windowToCron` 把 `HH:mm:ss` 翻成 6 段 cron | **模型保留**（定义落盘 + 启动重注册 + 运行中跳过），**换库**：Rust 侧需选定支持秒级的 cron 实现；"跳过 vs 排队"策略显式化 | 属 P2 剩余项；抢兑需要毫秒级 |
| CLI | `cli/src/`：**没有真正的子命令** —— `bin` 只接一个可选配置文件路径，不解析业务参数、无 readline；`kouling.ts` / `tjs-main.ts` 是各自独立入口；与 api **完全独立、分别启动**，任务编排是**两套**（CLI 硬编码顺序 vs api registry） | **重做**：clap 正经子命令（`login` / `run` / `serve` …）+ 一致退出码 + 交互式输入（登录必须）；CLI 与 server **共用同一任务注册源**，不再两套编排 | 与 §0.1「两个独立产物」配套 |
| 配置 | `shared/src/config/`：`asign.json`（`version/caiyun/common/message`，中文任务开关键名）；valibot schema **只用于生成类型/默认值/JSON-Schema，运行期不校验**；敏感值 `auth` **明文落盘**；改配置后**下次任务运行**才生效 | **重做**：运行期真校验（serde + schemars 复刻"单一来源生成"的思路）；分层"默认 → common → 账号"**保留**；配置改动即时生效 | 手改 JSON 的形态由 SQLite 取代（§3.x） |
| 共享基建 | `shared/src/http/`：请求基座，**按域名注入**凭据（`authorization` 仅发 `AUTH_DOMAINS`、`jwttoken` 仅发目标域、`deviceid` 全发）+ 内置 CookieJar；**无签名、默认不重试**。<br>`shared/src/slide/`：滑块验证码**只算缺口偏移**（自实现 Sobel 边缘检测 + 轮廓提取 + 模板匹配，fast-png 解码），**不含轨迹模拟与提交** —— 服务于旧项目**自己的**任务（**旧项目没有登录流程**，别把它和本次登录链路扯在一起） | http 基座的"按域名注入凭据 + CookieJar"语义**保留**（登录后绝大多数接口依赖它）；**滑块那一整套图像识别不搬迁** —— 登录链路上的滑块，缺口坐标由服务端用客户端公开密钥直接下发（`data.puzzleLeft`，解密得坐标；**已由一次真实风控挑战证实**：首发发码被 `200059554` 拦下 → 自动解坐标重发 → `验证码已发送`，见 `docs/upstream-logout-sms-login.md` §2.2.1）。所以既不需要边缘检测，也不需要人工量像素 | 重试策略见 §3.5 |

**待拍板项**（旧做法有争议，或存在多种合理选择）：

1. **对外暴露的防护** —— 旧 API 完全裸露、CORS 全开（D7）。新方案已定多用户（§0.1），需确认：默认只监听本地？公网部署时最低防护要求是什么？
2. **敏感值落盘** —— 旧项目 `auth` 明文存 JSON。新方案是否加密（或只靠文件权限 + 明确警告）。
3. **配置校验严格度** —— 旧项目"宽松解析 + 运行时报错"（schema 形同摆设）。是否改为严格校验（拒绝未知键、类型不符）。
4. **定时冲突策略** —— 现在静默跳过（不排队、不告警）。是否改为排队 / 告警 / 可配置。
5. **推送失败处理** —— 现在 fire-and-forget。是否需要重试 + 持久化队列（重启不丢）。
6. **异步运行状态** —— 仅内存、重启即丢（D4）。是否随运行历史一起落库。
7. ~~**CLI 是否也要能起服务**~~ —— **已定（2026-09-18）**：服务由 `mcloud-server` **自己的二进制**启动。
   依赖门禁早就给出了答案（`mcloud-cli -> mcloud-server` 被 `FORBIDDEN_EDGES` 挡住），
   而理由也成立：CLI 一旦拿到 axum，就不再是"能在任何没有 HTTP 环境的地方跑"的命令行工具。

---

## 2. 技术选型

### 2.1 后端 crate 映射

| 能力 | TS 现状 | Rust 选型 | 说明 |
| --- | --- | --- | --- |
| 异步运行时 | Node 事件循环 | `tokio`（full） | 标准解 |
| HTTP 客户端 | ofetch / fetch | `reqwest` + `rustls-tls` | **不用 native-tls**，规避 OpenSSL，Android/Termux 交叉编译友好 |
| HTTP 服务 | Hono + @hono/node-server | `axum` + `tower-http` | fs / cors / trace / compression。**只在 `mcloud-server`**（`mcloud-cli` 不得出现，见 §3.2） |
| 实时推送 | 轮询 | `axum::response::sse` | 替代轮询 |
| 序列化 | JSON | `serde` / `serde_json` | 配置由 **SQLite** 承载；旧 `asign.json` 仅作迁移输入（§3.7 / §8.1） |
| 存储 | 本地 JSON | **`rusqlite`（bundled）** | 见 §2.4 决策点 |
| 调度 | croner | `cron`（解析）+ `tokio::time::sleep_until`（精确到点） | 抢兑需要毫秒级 |
| 日志 | 自研 logger | `tracing` + `tracing-subscriber` + `tracing-appender` | 保留 `error/fatal` / `fail(info)` 语义；同时补**结构化字段**与**文件/滚动输出**，脱敏要覆盖 body/header（旧实现只覆盖 URL query，D11） |
| CLI | 自研"入口" | `clap`（derive） | **不是"保留既有参数"** —— 旧 CLI 没有参数面（§1.6），子命令（`login` / `run` / `serve` / `user` …）从零设计 |
| WebSocket | ws | `tokio-tungstenite` | 直播间 |
| protobuf | protobufjs（运行时解析 PROTO_DEF） | `prost` + `prost-build`（build.rs） | 把 `PROTO_DEF` 抽成 `proto/cmos.proto` 参与编译期生成 |
| HTML 解析 | 手写/正则 | `scraper`（按需） | 小红书主页口令 |
| 邮件 | nodemailer 等价 | `lettre` + rustls | 推送 Email 渠道 |
| 静态资源 | vite 产物目录 | `rust-embed` | 前端产物编进单二进制 |
| 密码学 | node:crypto + crypto-js | `md-5` / `sha2` / `hmac` / `aes` / `cbc` / `des` / `rsa` / `rand` / `base64` / `hex` | RustCrypto 全家桶，纯 Rust，无 OpenSSL |
| 日期时区 | Date | `chrono` + `chrono-tz` | 日快照边界按 `Asia/Shanghai` |
| 错误 | 自研 | `thiserror`（库内）+ `anyhow`（应用层） | |
| ID | randomUUID | `uuid` v4 | runId |
| 测试 | vitest | 内置 `#[test]`/`#[tokio::test]` + `insta`（快照）+ `tower::ServiceExt::oneshot`（路由) | 见 §6 |

**async trait 说明**：Rust 1.91 原生支持 trait 内 `async fn`（1.75 起），无需 `async-trait`；但需要 `dyn HttpClient` 时用 `trait_variant` 或保留 `async-trait`。建议 **trait 用泛型参数（静态分发）**，测试注入零成本抽象。

### 2.2 前端选型（本次新增决定）

| 项 | 选择 | 依据 |
| --- | --- | --- |
| 框架 | **Vue 3.6+，Vapor 模式**（无虚拟 DOM） | 3.6 起 Vapor 稳定，`vapor` 为 opt-in |
| 语法 | **TSX**（不是 SFC） | 用户指定 |
| TSX 编译器 | **`vue-jsx-vapor`**（`vuejs/vue-jsx-vapor`，v3.2.x，Oxc/Rust 驱动） | vuejs 官方组织，官方 JSX Vapor 方案 |
| 样式与组件 | **daisyUI 5 + Tailwind CSS 4** | 纯 CSS 类、零 JS 组件 → Vapor 原生兼容，不需要 interop |
| 构建 | Vite + `vue-jsx-vapor/vite` + `@tailwindcss/vite` | |
| 类型检查 | **`@ts-macro/tsc`（`tsmc`），不是 `tsc`** | 用了 macros，`tsc` 会报错 |
| 图标 | `unplugin-icons` + Iconify（inline SVG） | 零 VDOM 依赖 |
| 图表 | **手写 SVG**（云朵趋势） | 纯函数 + `insta` 快照，比引 ECharts 更轻更可测 |
| 编辑器 | VSCode + `TS Macro` 扩展（自动加载 `vue-jsx-vapor/volar`） | DX |
| 移动端 | **Tailwind 响应式 + daisyUI `drawer` / `dock`** | 见 §2.2.5 |

#### 2.2.1 为什么 daisyUI 比 JS 组件库更适合 Vapor

daisyUI 是 **Tailwind CSS 的插件**，提供的是**类名**（`btn` / `card` / `table` / `modal`…），不是 JS 组件。三个直接后果：

| | JS 组件库（如 TDesign） | daisyUI |
| --- | --- | --- |
| 与 Vapor 的关系 | 依赖虚拟 DOM → 必须走 interop 混树 | **纯 CSS 类** → Vapor 原生，零适配 |
| 插槽 / 属性跨界 | 有边界限制与命名约束 | 不存在这问题（没有组件边界） |
| 复杂交互（Modal / Tab / Drawer） | 库内部实现，状态不透明 | 我们用自己的 `ref` 驱动 + daisyUI 类，**状态与 a11y 更可控** |
| 代价 | 开箱即用的复杂组件 | 复杂组件的**逻辑**要自己写（样式 daisyUI 已给，见 §2.2.3） |

**结论**：daisyUI 把"组件库消费者"换成"自有轻量组件层的主人"。少一层 VDOM，多一层可控。

#### 2.2.2 集成方式（Tailwind 4 是 CSS-first，没有 `tailwind.config.js`）

```bash
npm install tailwindcss@latest @tailwindcss/vite@latest daisyui@latest
```

```ts
// vite.config.ts —— daisyUI 是 Tailwind 插件，绝不能放进 plugins 数组
import { defineConfig } from 'vite'
import tailwindcss from '@tailwindcss/vite'
import vueJsxVapor from 'vue-jsx-vapor/vite'

export default defineConfig({
  plugins: [
    tailwindcss(),
    vueJsxVapor({ macros: true }),   // 不再需要 interop
  ],
})
```

```css
/* src/app.css —— Tailwind 4 用 @plugin 注册 daisyUI */
@import "tailwindcss";
@plugin "daisyui" {
  themes: light --default, dark --prefersdark;
}
```

```ts
// main.ts —— Vapor 根，不需要 vaporInteropPlugin
import { createVaporApp } from 'vue'
import App from './app'
import './app.css'
createVaporApp(App).mount('#app')
```

要点与坑：

- **`tailwind.config.js` 在 v4 已废弃**，不要再创建（网上大量旧教程仍在教它）。
- **daisyUI 不是 Vite 插件**，写进 `plugins: [daisyui]` 会报 `handler is not a function`。
- 主题切换：`<html data-theme="...">`，daisyUI 5 内置 **35 个主题**；我们只用 `light`（默认）+ `dark`，并用 `@plugin "daisyui/theme"` 微调 `--color-primary` 等变量对齐现有配色。**注意：默认浅色主题，且「涨红跌绿」的约定不受影响。**
- **Tailwind 类名扫描陷阱（最容易踩）**：Tailwind 只识别**字面量**类名，`btn-${variant}` 这类拼接会在构建后静默失效。必须查表：
  ```ts
  const btnTheme = { primary: 'btn-primary', danger: 'btn-error' } as const
  // ✅ class={`btn ${btnTheme[variant]}`}    ❌ class={`btn-${variant}`}
  ```
  这条同时是给 AI 写代码的硬约束 —— 动态类名静默失效是这类项目最常见的样式 bug。

#### 2.2.3 需要自建的组件层（daisyUI 的代价，已按 v5 实际清单清点）

daisyUI 5 的组件比我预想的多（含 `pagination` / `calendar` / `countdown` / `validator` / `fieldset` / `filter` / `status` / `tab` / `collapse` / `modal` / `drawer` / `dock` / `toast` / `dropdown`），所以真正要自己写的只剩**逻辑**：

| 需要 | daisyUI 给什么 | 我们要写什么 |
| --- | --- | --- |
| 运行历史 / 奖品表格 | `table` + `pagination` + `filter` 样式 | 排序 / 分页 / 筛选**逻辑**（纯函数，可单测 —— 正好修掉旧实现的分页 `total` bug） |
| 账号多选 | `checkbox` + `badge` | 无（账号 ≤5，比下拉多选更好用） |
| 抢兑时刻 | `calendar` 只解决**日期** | 时刻用 **3 个原生 `<select>`**（时/分/秒）+ →cron 纯函数映射 |
| 云朵趋势 | 无图表组件 | **手写 SVG**（`buildTrendPath()` 纯函数 + `insta` 快照） |
| JSON 入参 | `textarea` + `validator` | 校验 / 格式化纯函数（不引 Monaco） |
| 距开抢倒计时 | **`countdown` 内置** | 无 |
| Modal / Drawer / Confirm | `modal` / `drawer` 样式 | `<dialog>` + 极简 store 驱动 |
| Toast | `toast` / `alert` 样式 | 20 行 store |

> 图标不要从别处借：用 `unplugin-icons` + Iconify（inline SVG），零 VDOM 依赖。

#### 2.2.4 Vapor 自身的边界（与组件库无关，仍需遵守）

- 仅支持 **Composition API**；Options API、class 组件留在 VDOM；
- Vapor 组件内 `getCurrentInstance()`、`app.config.globalProperties` **不可用**；
- **Suspense 不支持** → 异步一律用显式 `loading` 状态；
- `v-model` 不支持数组表达式语法 → 用修饰符写法 `v-model:$value$_trim`；
- `vue-jsx-vapor` 不把连字符名转小驼峰 → 组件名与属性一律 camelCase；
- **兜底（当前不用）**：`interop: true` + `vaporInteropPlugin` 仍写在文档里。将来若真要引入 VDOM 库（如实时代码编辑器），零成本开启即可。当前**默认不开**。

#### 2.2.5 移动端兼容

用户要求"兼容手机"。daisyUI 把移动端做成了内置组件（`drawer` / `dock` / `navbar` / `join`），比组件库的响应式省事：

- 断点 768px 以下：侧栏改 `drawer`，底部切 **`dock`**（注意：daisyUI 5 里旧名 `btm-nav` 已改名 `dock`）；
- 触控目标 ≥44px（`btn` 默认尺寸已满足，移动端禁用 `btn-xs`）；
- 禁用 hover-only 交互（表头操作、行内按钮改常驻或长按菜单）；
- `env(safe-area-inset-*)` 适配刘海 / 小白条；
- 长表格窄屏切**卡片列表**（`card` 替代 `table`，不做横向滚动）；
- 兑换抢兑页移动端优先，"距开抢 T-xx:xx"直接用 `countdown`。

#### 2.2.6 AI 友好性（选它的原始理由，落地为两条）

1. **类名是语义化字符串**：LLM 不需要记组件 API / 属性 / 插槽签名，写错通常只是样式不对，而不是编译不过 —— 比 JS 组件库的 props 更抗幻觉。
2. daisyUI 官方提供 **MCP Server（Blueprint，代码生成）** 与**面向 AI 的纯文本文档**。建议开发期接入该 MCP，并在仓库内 vendor 一份文档文本，让类名用法有据可查、不靠记忆。

### 2.3 明确放弃：txiki.js 双运行时

现状用 `packages/runtime` 抹平 Node 与 txiki.js 差异，产物为 `.tjs.mjs`。Rust 版本直接产出**各平台原生静态二进制**，txiki 的存在意义（受限环境跑 JS）被原生二进制完全覆盖且更强（无运行时、无解释器、可交叉编译 aarch64-linux-android / aarch64-apple-ios）。

**结论**：`packages/runtime` 与其 tjs 分支、`tsup` 的 `TJS=1` 路径、`tjs-tls-compat` 相关 patch **全部不再需要**。这是重写最大的净减法。
（**已决策**：确认不再需要 txiki，不受宿主 App 嵌入场景约束。）

### 2.4 明确不照搬参考项目

MySQL / Redis / 多用户与权限体系 / K8s / 监控栈 / 双二进制 API+Worker 拆分。

原因：本项目是**本地优先、单进程、多账号、零外部服务依赖**。注意区分——
- **多账号 = 保留且强化**（一等公民）；
- **多用户/角色权限 = 不引入**。

---

## 3. 目标架构

### 3.1 Cargo workspace 布局

```
mcloud/                               # /Users/catlair/mobile/workspace/mcloud
├── Cargo.toml                        # [workspace] members
├── rust-toolchain.toml               # 固定 1.91
├── crates/
│   ├── mcloud-core/                     # 领域类型 + 纯函数 + trait 定义。零 IO、零网络、零文件
│   │   ├── src/time_window.rs        #   时间窗口/开抢时刻计算（纯）
│   │   ├── src/offset.rs             #   服务器时间纠偏（纯）
│   │   ├── src/merge.rs              #   状态归并、口令聚合（纯）
│   │   ├── src/error.rs              #   错误分类：Auth/RateLimited/SoldOut/InsufficientCloud/Timeout
│   │   ├── src/traits.rs             #   HttpClient / StateStore / Clock trait
│   │   └── src/model/                #   Account / CloudStats / Run / CapabilityDef / FieldSpec
│   ├── mcloud-upstream/                 # 上游接口封装。★ 按新抓包逐个重写，不从旧 TS 搬
│   │   ├── src/login.rs              #   ★ 移动云盘登录（短信 / 扫码）—— 旧项目无此功能
│   │   ├── src/auth.rs  signin.rs  task.rs  exchange.rs  ai_workbench.rs
│   │   ├── src/live_room.rs  file.rs  share.rs  note.rs  mail139.rs
│   │   ├── src/live_room_proto.rs    #   prost 生成物 + 编解码
│   │   └── （模块划分随新抓包调整；目前仅 lib.rs 骨架 = Upstream + Envelope + base）
│   │       （2026-09-19 补：上面那行是当时的实况，原文留着。此后按抓包落地的有
│   │        login / client / app_client / app_login / app_signin /
│   │        smid（cmpassport 设备指纹上报，见 §7.2 与 docs/client-profiles.md §5.4 ③））
│   ├── mcloud-capabilities/             # 19 个云盘能力 + 注册表
│   │   ├── src/registry.rs           #   静态 LazyLock<Vec<Capability>>
│   │   ├── src/login/                #   ★ 登录流程（短信 / 扫码）：多步交互，CLI 与 WebUI 共用一份状态机
│   │   ├── src/sign.rs  receive.rs  …  after.rs
│   │   └── src/params/               #   每个能力的 FieldSpec 声明
│   ├── mcloud-io/                       # 具体实现（唯一允许碰 IO 的 crate）
│   │   ├── src/http/reqwest_client.rs
│   │   ├── src/http/retry.rs         #   读重试+指数退避+抖动；写必须幂等键
│   │   ├── src/http/fixture_client.rs#   ★ 离线回放（测试用；夹具不入库，见 §6.2）
│   │   ├── src/store/sqlite.rs       #   repository 层（含用户/会话，见 §3.7）
│   │   ├── src/store/migrate.rs      #   旧 data/*.json 导入（v3：用户维度）
│   │   ├── src/crypto/               #   与 §1.3 一一对应，含对拍测试
│   │   ├── src/push/                 #   10 个渠道（§1.4）
│   │   └── src/sched/                #   cron + 精确到点
│   ├── mcloud-exec/                     # 执行器：runId / 并发闸 / 事件总线
│   │   ├── src/run.rs                #   RunStore（SQLite 持久化，替代内存 asyncRuns）
│   │   ├── src/concurrency.rs        #   默认串行、全局上限、同(能力+账号)互斥
│   │   └── src/event.rs              #   broadcast 事件流 → SSE
│   ├── mcloud-app/                      # ★ 组装层：两个产物共用它，见 §3.2
│   │   ├── src/migration.rs          #   旧数据迁移：先归档、再建库、后导入（+ 预演走同一条路径）
│   │   ├── src/payload.rs            #   能力注册表 / 版本 → 前端要吃的 JSON
│   │   ├── src/device.rs             #   设备指纹（dID）：最早存在、永不漂移的那份本地状态
│   │   ├── src/user.rs               #   首个管理员初始化（CLI 与 WebUI 引导共用同一用例）
│   │   └── src/login.rs              #   登录驱动：状态机 + 真实等待（1000ms）+ 观察者回调
│   │       （2026-09-19 补：上面那份是 PC 那条。此后还有 app_login.rs（App 端登录驱动）、
│   │        session.rs（本系统会话）、smid.rs（设备特征绑账号 + 上报换 deviceId + rotate））
│   ├── mcloud-server/                   # ★ 产物二：axum 路由 + SSE + 内嵌前端 + OpenAPI
│   │   ├── src/routes/               #   login / accounts / capabilities / runs / schedules / exchange / debug
│   │   └── src/openapi.rs            #   utoipa → openapi.json（供前端生成类型）
│   └── mcloud-cli/                      # ★ 产物一：纯 CLI（clap），禁止依赖 axum/tower-http/rust-embed
├── web/                              # ★ 全新前端
│   ├── vite.config.ts                #   tailwindcss() + vueJsxVapor({ macros: true })
│   ├── tsconfig.json                 #   jsx: preserve, jsxImportSource: vue-jsx-vapor
│   └── src/
│       ├── app.css                   #   @import "tailwindcss"; @plugin "daisyui"
│       ├── main.ts                   #   createVaporApp(App).mount('#app')
│       ├── app.tsx
│       ├── api/                      #   ★ 由 openapi.json 生成，不手写
│       ├── composables/              #   纯逻辑，可单测
│       ├── ui/                       #   ★ 自有轻量组件层（表格/Modal/Toast…）
│       ├── components/               #   多为 defineVaporComponent
│       └── pages/                    #   8 个页面 + ★ 登录页（短信 / 扫码）
├── proto/cmos.proto                  # 从 PROTO_DEF 抽出
├── test/
│   ├── fixtures/http/                # 新 HAR → 脱敏夹具（★ **本机数据，不入库**，见 §6.2）
│   └── golden/                       # 与 TS 原实现对拍的黄金向量（crypto / auth）
└── data/                             # 运行时数据（SQLite；旧 asign.json 仅作迁移输入）
```

**文件命名约定（统一，别再混风格）**：

| 范围 | 规则 | 例 |
| --- | --- | --- |
| **非 Rust 文件**（文档 / 前端 / 脚本 / 夹具 / JSON） | **kebab-case**：全小写、`-` 分隔、不用空格与下划线 | `rust-rewrite-plan.md`、`app.tsx`、`gen-auth-vectors.cjs`、`check-deps.sh` |
| **Rust 源文件** | **snake_case**（**语言强制**，不是风格偏好） | `test_support.rs`、`reqwest_client.rs`、`fixture_extract.rs` |
| 生态固定名 | 不改 | `README.md`、`LICENSE`、`Cargo.toml`、`Cargo.lock`、`THIRD-PARTY-NOTICES.md` |

> Rust 那条为什么不能也 kebab：**文件名就是模块名**，而 `mod` 只接受标识符 —— `mod my-mod;` 不合法。
> 想用连字符只能 `#[path = "my-mod.rs"] mod my_mod;` 硬绕，代价是模块树与文件树对不上，得不偿失。
>
> 夹具的文件名由 `fixture extract` 的 `slugify` 统一（HAR 名实测带中文、空格、下划线、大写），
> 中文保留（它本身就是信息量），其余非字母数字一律折成 `-`。

### 3.2 分层与依赖方向

**依赖方向：↓ 表示"依赖下方"，▲ 表示"实现 core 定义的 trait"。两个产物各自成型、互不拖动。**

```
   两个产物（互不拖动；★ 由「cli 不依赖 server/axum」这一要求引出）
   ┌────────────────────────────────┐      ┌──────────────────────────────┐
   │ web（Vapor + TSX + daisyUI）    │      │ 终端用户                      │
   │ 登录/仪表盘/账号/任务/兑换       │      │                              │
   │ 定时/历史/调试台/设置            │      │                              │
   └───────────────┬────────────────┘      │                              │
       生成式类型 + SSE │                    │                              │
   ┌───────────────▼────────────────┐      ┌──────────────────────────────┐
   │ mcloud-server（axum）           │      │ mcloud-cli（clap）            │
   │ 路由 + 校验 + SSE + OpenAPI     │      │ 子命令 login/run/…           │
   │ + rust-embed 内嵌前端产物        │      │ ★ 不含 axum/tower-http/embed │
   └───────────────┬────────────────┘      └───────────────┬──────────────┘
                   └──────────────┬───────────────────────┘
                        ┌─────────▼──────────────────────┐
   mcloud-app           │ 组装根：装配运行时、注入 IO 实现 │  不含 axum、不含 clap
                        │ 两个产物共用同一份能力与编排      │
                        └─────────┬──────────────────────┘
             ┌────────────────────┼───────────────────────┐
   ┌─────────▼────────┐  ┌────────▼────────────┐  ┌───────▼──────────────┐
   │ mcloud-exec      │  │ mcloud-capabilities │  │ mcloud-upstream      │
   │ runId/并发/事件   │  │ 声明式注册表 + 登录  │  │ 上游接口（含登录）    │
   └─────────┬────────┘  └────────┬────────────┘  └───────┬──────────────┘
             └────────────────────┼───────────────────────┘
                        ┌─────────▼─────────────────────────────┐
                        │ mcloud-core（纯计算，全项目最底层）     │
                        │ 类型 / 纯函数 / crypto（纯密码学）/     │
                        │ 错误分类 / trait 定义 / 用户与凭据类型  │
                        │ ★ 严禁 reqwest、rusqlite、std::fs、     │
                        │   std::net、now()                      │
                        └─────────▲─────────────────────────────┘
                                  │ 实现 core 定义的 trait
                        ┌─────────┴─────────────────────────────┐
                        │ mcloud-io                             │
                        │ ReqwestClient / SqliteStore /         │
                        │ FixtureHttpClient / SystemClock /     │
                        │ SystemRandom / push 渠道 / 调度        │
                        └───────────────────────────────────────┘
```

**★ 这次分层变更需要同步改两个地方（本次只登记，不改）**

旧方案里 `mcloud-server` 是**唯一**组装根，且 `mcloud-cli` **不**直依赖 `mcloud-io`（"只做参数解析"）。
而新要求是「`mcloud-cli` 是纯 CLI 产物、不依赖 server/axum，同时要能独立完成登录与跑任务」——
两者冲突。落地方案是抽出不含 axum/clap 的组装层 `mcloud-app`：

| 要改的地方 | 现状 | 改成 |
| --- | --- | --- |
| `scripts/check-deps.sh` | `COMPOSITION_ROOTS=(mcloud-server)` | 增列 `mcloud-app`；并**新增对称断言**：`mcloud-cli` 的依赖树不含 `axum` / `tower-http` / `rust-embed` |
| `README.md`「分层铁律」表与依赖方向图 | 组装根只有 `mcloud-server`；`mcloud-cli` 不依赖 io | 增加 `mcloud-app` 一行；两个产物都依赖 `mcloud-app` |

> 之所以**现在不改**这两个文件：分层还没落地，先改会让 README 描述一个不存在的结构。等 `mcloud-app` 真正建起来时一并改。

> **为什么 `crypto` 必须在 `mcloud-core` 而不是 `mcloud-io`**（原图示把它画在 io 下，是个错误）：
> `mcloud-upstream` 要生成签名头（MD5）、构造设备指纹（DES/RSA），而 **upstream 不允许依赖 `mcloud-io`**
> —— 否则会把 `reqwest` 拖进 upstream 的依赖树，"对单个 upstream 函数做纯测试"就不再可能。
> 纯密码学（哈希/对称/非对称/编码/压缩）是**无 IO 的纯计算**，天然属于 core。

**硬约束（"能对某个函数单独测试"的前提）**：`mcloud-core` 与 `mcloud-capabilities` 内**禁止**出现 `reqwest`、`rusqlite`、`std::fs`、`std::net`、`SystemTime::now()`。三类外部依赖全部走 trait：

| 依赖 | trait（定义在 core） | 生产实现（io） | 测试实现 |
| --- | --- | --- | --- |
| 网络 | `HttpClient` | `ReqwestClient` | `FixtureHttpClient`（未命中即报错，绝不偷偷打真网） |
| 时间 | `Clock` | `SystemClock` | `FixedClock` |
| 随机 | `Random` | `SystemRandom`（CSPRNG） | `SeededRandom`（固定种子，可复现） |
| 存储 | `StateStore` | `SqliteStore` | 内存实现 |

> **`Random` trait 是本次新增的**。原因：设备指纹（`smid`）构造里混用了 `randomUUID` 与 `Math.random`，
> 若直接调用全局随机数，**L2 fixture 回放与 golden 对拍都无法复现**（每次跑出来的请求体都不同）。
> 把随机数收敛到 trait 后，测试注入固定序列即可让整条链路字节级可复现。


### 3.3 能力注册表

```rust
pub struct Capability {
    pub code: &'static str,            // "sign" / "pending-prizes" / "cloud-snapshot"
    pub title: &'static str,
    pub group: Group,                  // Daily | Activity | Query | Exchange | Maintenance
    pub kind: CapabilityKind,          // Action | Query（Query 无副作用、可反复执行）
    pub description: &'static str,
    pub params: fn() -> Vec<FieldSpec>,// 声明式入参 → 前端自动表单 + 服务端校验
    pub output: fn() -> Vec<FieldSpec>,// 声明式出参 → 前端自动结果卡片
    pub hidden: bool,                  // 独立页面运行（如 exchange）
    pub run_in_batch: bool,
    pub risk: RiskLevel,               // Low | Medium | High（高 → 前端二次确认）
    pub account_mode: AccountMode,     // Sequential | Parallel（默认 Sequential，防风控）
}

pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,               // Text | Number | Bool | Select | MultiSelect | Json
    pub required: bool,
    pub default: Option<serde_json::Value>,
    pub options: Option<Vec<SelectOption>>,
    pub help: Option<&'static str>,
}
```

注册用 `std::sync::LazyLock<Vec<Capability>>`（1.80 稳定），**不用宏魔法** —— 列表显式、可 grep、可 diff。

收益：新增能力只改注册表 → WebUI 表单/结果卡片、API 校验、组合步骤参数、CLI `--help` **全部自动**。直接消灭 D3。

### 3.4 账号状态中心（解决 D1/D2）

```sql
-- 账号主表：多账号一等公民，★ 每个账号归属一个系统用户（§3.7）
CREATE TABLE accounts (
  id            TEXT PRIMARY KEY,      -- 账号标识（脱敏手机号或昵称派生）
  owner_id      TEXT NOT NULL,         -- ★ 归属的系统用户；所有查询都要带上它（§3.7 的隔离断言）
  nickname      TEXT,
  phone_masked  TEXT,
  platform      TEXT,
  enabled       INTEGER NOT NULL DEFAULT 1,
  sort_order    INTEGER NOT NULL DEFAULT 0,
  cloud_current INTEGER,               -- ★ 当前云朵余额（修复 D1）
  cloud_updated_at TEXT,
  auth_expire   INTEGER,               -- 凭据到期时间戳（不存 token 本体到查询视图）
  last_ok_at    TEXT,
  fail_streak   INTEGER NOT NULL DEFAULT 0,
  note          TEXT,
  FOREIGN KEY (owner_id) REFERENCES users(id)
);

-- 云朵日快照：PK(account_id, date)，UPSERT
CREATE TABLE cloud_stats (
  account_id      TEXT NOT NULL,
  date            TEXT NOT NULL,       -- YYYY-MM-DD（Asia/Shanghai）
  cloud_count     INTEGER NOT NULL,
  cloud_diff      INTEGER NOT NULL,    -- 与上一有记录日之差
  cloud_diff_week INTEGER NOT NULL,    -- 与 7 天前之差
  PRIMARY KEY (account_id, date)
);

-- 运行记录 + 事件（替代内存 asyncRuns，解决 D4）
CREATE TABLE runs (
  id TEXT PRIMARY KEY, account_id TEXT, capability TEXT,
  status TEXT, started_at TEXT, finished_at TEXT,
  cloud_gained INTEGER, cloud_after INTEGER, message TEXT
);
CREATE TABLE run_events (
  run_id TEXT, seq INTEGER, ts TEXT, level TEXT, kind TEXT, payload TEXT,
  PRIMARY KEY (run_id, seq)
);
```

余额采集来源（多源取可信值，失败降级，不阻塞主流程）：

1. `getSignInInfoV3.signInPoints`（`core/src/types/exchange.ts`）
2. `getUserTaskAndCloudStatusExternal.cloudStatus`（`core/src/apis/ai-workbench.rs` 对应文件）
3. 复活/领取类任务返回的 `totalClouds`（`core/src/types/sign.ts:65`，`sign/task.rs:170` 已打日志）
4. `hc1t` 的云朵记录接口

采集后统一走一个 `record_cloud(account, count)` 用例：写 `accounts.cloud_current` + UPSERT `cloud_stats` 并算日/周增量。**纯函数部分（增量计算、日期归属）在 `mcloud-core` 内，可单测。**

> **落地情况（2026-09-19）**：`record_cloud` 已实现于 `mcloud-app::cloud`，
> 上面 4 个来源里目前只接上了 **① `infoV3.beforeTotal`**（签到链路）——
> 另外三个各自的接口本身也还没写。增量计算用的就是上面那句说的纯函数
> `mcloud_core::pure::next_cloud_stats`（它和 `daily_key` 在实现之前就已经在了）。
> 存储侧为此补了一个**窄写**方法 `StateStore::set_account_cloud`：
> 不复用整行覆盖的 `upsert_account`，因为"读整行 → 改一列 → 写回"会把
> 读与写之间别人改的昵称**静默写回旧值**。

### 3.5 执行器

- `runId`（uuid v4）= 幂等单元；同 runId 重放不重复执行。
- 事件流：`log` / `progress` / `result` / `error` → `broadcast` → `GET /api/runs/:id/events`（SSE），同时落 `run_events` 表。
- **并发策略（防风控）**：默认 `Sequential`；全局并发上限默认 2 可配；同（能力+账号）互斥；长等待型（抢兑预热）在等待期**释放并发槽**。
- 写请求必须带幂等键；读请求默认重试 + 指数退避 + 抖动。错误分类决定可否重试。
- 对外错误**绝不回传** `err.Error()` 原文，走文案白名单。

### 3.6 调度

- 通用定时：`cron` 解析 + `tokio` 任务。
- 抢兑精确到点：`sleep_until(target - offset)`，offset 可配（默认提前 300ms 校正），触发前预热并连接复用。
- 任务执行结果 → 回写 `accounts.cloud_current` 并 UPSERT `cloud_stats`。
- 下次执行时间必须**可查询**（前端"下次执行"列的数据来源）。

---

### 3.7 用户、会话与云盘账号归属（本次新增）

**为什么是一等公民而不是"加个中间件"**：旧项目**完全没有**这一层 —— 没有任何鉴权中间件、没有用户概念，账号只是配置数组里的下标（D7）。新要求是"用户用自己的账号登录本系统，每个用户各自管理属于自己的移动云盘账号"，所以它是一个领域模型。

**三个概念必须分清（混起来就会设计错）**：

| 概念 | 是什么 | 由谁提供 |
| --- | --- | --- |
| **系统用户** | 登录**本系统**的账号 | 我们自己：用户名 + 密码哈希 + 角色 |
| **会话** | 用户登录后的凭据 | 我们自己：WebUI cookie 会话 / CLI 本地登录态 |
| **云盘账号** | 移动云盘账号（含 `auth`） | 移动侧；**归属**于某个系统用户 |

**领域类型（放 `mcloud-core`，纯计算）**：

- `User { id, name, password_hash, role, created_at }`，`Role::{Admin, User}`
- `Session { token_hash, user_id, expires_at, kind }`，`SessionKind::{Web, Cli}`
- `Account` 增加 `owner: UserId`
- `Capability` 增加"是否需要登录态"—— 未登录时 WebUI 只应暴露登录页

**密码与会话凭据**：

- 密码哈希是**纯计算**，放 core 不破坏零 IO 约束；用**可对拍的现代 KDF**（`argon2`，参数写进文档并留测试向量），不要裸 SHA256。
- 会话 token **只存哈希**，明文仅在签发那一刻返回一次。

**数据隔离 —— 写成可测断言**：

- 所有 `StateStore` 查询**必须带 `UserId`**。现有方法都是账号维度，需要加用户维度；漏掉一处就是越权。
- 断言方式（列进 §6 的 L1）：内存 store 造两个用户各一个账号，交叉读取必须为空或报错。

**首个管理员（避免两套 bootstrap）**：

- CLI `mcloud user init` 与 WebUI 首次访问引导**写同一张表、走同一个用例**；未初始化时 WebUI 只暴露初始化页，不暴露其余任何接口。

**schema v3 迁移**：

- 新增 `users` / `sessions` 表，`accounts` 加 `owner`。
- **归属分两步**（迁移是纯 SQL，执行时管理员还不存在，硬编一个 owner 只能靠猜）：
  1. 迁移把历史账号的 `owner` 置为 `NULL` = **未归属**，且未归属的账号**谁都看不见**（宁可暂时不出现，也不要谁都拿得到）；
  2. `mcloud user init` 初始化首个管理员后调用 `StateStore::claim_unowned_accounts` **认领**（只认领 `owner IS NULL`，幂等）。
- 迁移必须幂等，且 `--dry-run` 走**完全相同**的代码路径（沿用 §8 的既有约定）。
  v3 里的 `ALTER TABLE ... ADD COLUMN` 是唯一无法写成 `IF NOT EXISTS` 的语句，
  因此那一段外面包了**显式事务**：不包的话失败会留下"列已加、版本未升"的半成品库，重跑直接报重复列。
- `users.name` 用 `COLLATE NOCASE` 唯一：否则 `Alice` 与 `alice` 能各建一个，登录时匹配到谁取决于查询顺序。

**schema v4 迁移（定时任务与组合的归属）**：

- `schedules` / `pipelines` 加 `owner`，与账号**同一套语义**（历史数据 `NULL` = 未归属、谁都看不见；认领时机同为 `user init`）。
  不补的后果不是"少个字段"，而是这两类数据**没有越权判据可加** —— 等 P5 的调度引擎与 WebUI 上来，一个用户就能看见并触发别人的任务。
- 与 v3 一样包显式事务，且**这一段比 v3 更依赖它**：只有两条 ALTER，没有任何 `CREATE ... IF NOT EXISTS` 兜底。
- 认领从 `claim_unowned_accounts` 扩成 `claim_unowned`，返回**三笔分别计数**（账号 / 定时任务 / 组合）：
  `mcloud user init` 会把三笔分开打印 —— 升级上来的用户最关心"我的东西还在不在"，只回一个总数等于把这件事糊掉。
- **仍然全局**的是 `exchange_config`（schema 里被 `CHECK (id = 1)` 钉成单行表）与 `webui_config`（界面偏好）。
  要按用户分是**改语义**（单行表 → 每用户一行），该单独做一次决定，而不是顺手加个 owner 列。
- 写操作也要过归属检查（不只是读）：`id` 是主键、写入是 `ON CONFLICT DO UPDATE`，
  只过滤读会让越权从"看得到"升级成**"改得掉"**。已存在且属于别人的行返回
  `BadRequest("...已被其他用户创建")`，而不是静默顶掉。

**本地与网络的分界（2026-09-18 修正）**：原先设想 CLI 也要"登录本系统"，这是**错的** ——
数据库就在本机，能跑 CLI 的人本来就能直接 `sqlite3` 读库，给他加一道"你是谁"
既不增加任何安全性，又把本地运维变得别扭（列个账号都要先登录）。所以：

- **CLI 一律用 `Scope::Local`**（不过滤，能操作所有人的账号）—— 这是句实话，不是后门；
- **WebUI 的每个请求用 `Scope::User(会话用户)`** —— 那里的用户拿不到库文件，隔离才有意义；
- 过滤逻辑**只有一份**：查询写 `(?n IS NULL OR owner = ?n)`，一个表达式同时表达两种范围，
  于是"某条查询漏了归属条件"在结构上不可能发生（它必须调用这个表达式）。
  `owner IS NULL` 的历史账号对两者都不可见，直到被认领。

CLI 与 WebUI 仍共用同一份 `Session` 领域类型与校验逻辑（`mcloud_app::session`），
但**只有 WebUI 用它**：网络才需要"你是谁"。

**网络层的落地（2026-09-18）**：

- 路由在 `mcloud-server`：`POST/GET/DELETE /api/session`（登录 / 当前身份 / 登出）、
  `GET/POST /api/setup`（是否要初始化 / 建首个管理员）、`GET /api/health`。
  免登录端点用**白名单**判定（`is_public`）：新加的路由默认是"需要登录"，
  漏登记一条不会把它变成公开接口。
- cookie：名 `mcloud_session`，`HttpOnly; SameSite=Lax; Path=/`；`Secure` 由
  `--secure-cookie` 决定且**默认关** —— 本地是 `http://127.0.0.1`，
  开了它浏览器根本不会回传，表现是"登录接口返回成功但每个请求都未登录"，很难往 cookie 上想。
- 有效期 **7 天**（`mcloud_app::session::WEB_SESSION_DAYS`，与 CLI 那边的 365 天刻意不同：
  cookie 会被浏览器带到每一个请求上）。到期不是唯一保护 —— **撤销立刻生效**（删掉会话行）。
- **错误形状统一为 `{code, message}`**：4xx 给正文（`Error::message()`，
  不带 `Display` 的类别前缀 —— "凭据失效: 用户名或口令不正确"前半句是给运维看的）；
  5xx 给 `ErrorKind::public_message()` 的类别文案，原文只进 `tracing`。
  请求体错误也走同一形状（`ApiJson` 提取器）：直接用 `axum::Json` 会返回
  axum 自带的 422 **纯文本**，前端只能显示"HTTP 422"。
- 契约测试：`cargo test -p mcloud-server --test api_contract`，**只走公开接口** ——
  放在 `tests/` 而不是单元测试里，要证的正是"这层 API 从外部能不能用"。

> 相关的待拍板项（对外暴露的防护、敏感值落盘、配置校验严格度）见 §1.6 文末。

---

## 4. 前端重写设计

### 4.1 页面（全新，不沿用旧页）

> **口径**：下表只界定**功能范围**（要有哪些页面、每页要能做什么）。**布局、交互与粒度一律自行设计**，
> 不参考旧前端 —— 两者的控制粒度不一样（旧页面是"几个大块拼一起"，新前端要做到字段级/操作级）。

| 页面 | 定位 | 移动端形态 |
| --- | --- | --- |
| **登录** | 系统账号登录/初始化管理员；云盘账号的**短信登录**与**扫码登录**（多步交互） | 单列表单 |
| **仪表盘** | 账号卡墙：云朵余额 / 今日增量 / 7 日迷你趋势 / 上次任务 / 健康度 | 单列卡片流 |
| **账号详情** | 30 天云朵趋势图、任务执行明细、凭据到期、连续失败、单账号一键巡检 | 折叠分区 |
| **任务中心** | 注册表驱动的能力列表：分组、开关、FieldSpec 渲染的参数表单、单/多账号、串行/并行；Query 型能力输出结构化结果 | Tab + 卡片 |
| **兑换中心** | 奖品分组 + 库存 + 状态、规则（账号×奖品×时刻）、任务列表（下次执行/重试/结果）、记录、T-xx:xx 倒计时 | 抢兑优先布局 |
| **定时任务** | 统一调度视图（含兑换规则生成的任务），暂停/立即执行/下次执行时间 | 列表 + 底部操作 |
| **历史与日志** | 运行历史 + SSE 实时日志流，按账号/能力/状态筛选 | 卡片流 |
| **调试台** | ★ **函数级测试界面**：选能力/上游接口 → 填参数 → 执行 → 看请求/响应/耗时/日志；**离线 fixture 模式**（不发真实请求） | 简化版 |
| **设置** | 并发与风控参数、推送渠道、数据路径、schema 版本与迁移、版本号 | 分区表单 |

### 4.2 精细化控制落地要点

1. **任何能力都能在 WebUI 触发**：由注册表生成，新增任务**零前端改动**。
2. **账号维度可见**：余额、今日增量、上次成功、连续失败、凭据到期（**只显示到期时间，不显示 token**）。
3. **参数表单化**：`FieldSpec` → 输入控件；保留"高级 JSON"折叠区兜底。
4. **查询与动作分离**：`kind = Query` 不写历史、不产生日志噪音。
5. **危险动作二次确认**：`RiskLevel::High`（如兑换消耗云朵）显示影响面（账号数 × 云朵数）。
6. **实时**：SSE 日志 + 进度；长任务显示倒计时。
7. **移动端**：见 §2.2.5。

### 4.3 类型同步（解决 D5）

`mcloud-server` 用 `utoipa` 导出 `openapi.json` → `web` 侧 `openapi-typescript` 生成 `src/api/types.gen.ts` → CI 跑 `--check`，生成物与提交物不一致即失败。**前端类型不再手写。**

---

## 5. Rust 版"单独测某个函数"怎么用

这是用户明确要的能力，Rust 天然支持且比 vitest 更干净——**测试名过滤即函数级选择**：

```bash
cargo test                                          # 全量（含五项门禁里的第一项）
cargo test -p mcloud-core                           # 只测纯逻辑层
cargo test -p mcloud-core time_window               # 只跑名字含 time_window 的测试
cargo test -p mcloud-capabilities exchange::resolve_prize   # ★ 单个函数
cargo test -p mcloud-upstream                       # 上游 L2 夹具回放（零联网；无夹具时自动跳过）
cargo test -p mcloud-server --test api_contract     # 路由契约
cargo test -- --ignored                             # 真实链路 smoke（需 MCS_SMOKE=1）
```

> 上游回放**没有** `--features fixture`：`FixtureHttpClient` 在 `mcloud-io`，通过 `[dev-dependencies]`
> 引入（生产构建里 upstream 仍不沾 IO，这由 `scripts/check-deps.sh` 只看 normal 边来保证）。
> 夹具是**本机数据、不入库**，所以"没有夹具"是正常状态 —— 用例会跳过，全新克隆上 `cargo test` 必须是绿的。

约定：**被测函数所在模块内写 `#[cfg(test)] mod tests`**（同文件白盒）。每个能力至少 1 个纯函数用例 + 1 个夹具用例。

调试台（WebUI 页面之一）把同一能力界面化：选函数 → 填参数 → 走 `FixtureHttpClient` 或真实 `ReqwestClient` → 展示请求/响应/耗时。

---

## 6. 测试体系

### 6.1 五层

| 层 | 范围 | 手段 | 联网 | 目标 |
| --- | --- | --- | --- | --- |
| **L1 纯函数（后端）** | 算法/校验/归并/时间窗口/增量 | 内置 `#[test]` | ❌ | 单函数定向验证，毫秒级 |
| **L1' 纯函数（前端）** | 排序/分页/筛选/时刻→cron/趋势路径/JSON 校验 | vitest | ❌ | 自有组件层的逻辑与 UI 解耦后可单测 |
| **L2 fixture 回放** | 上游接口封装 | `FixtureHttpClient` | ❌ | 用真实 HAR 验证请求构造与响应解析 |
| **L3 API 契约** | axum 路由 + 校验 | `tower::ServiceExt::oneshot` + `insta` 快照 | ❌ | 路由/校验/响应结构 + OpenAPI `--check` |
| **L4 E2E** | 前端主链路 | Playwright + `page.route('**/api/**')` 全 mock | ❌ | 页面交互回归，替代人工点 |
| **L5 真实 smoke** | CLI / 真账号 | `#[ignore]` + `MCS_SMOKE=1` | ✅ | 上线前手工触发，默认不跑 |

> 前端 L1' 是 daisyUI 路线的额外收益：因为复杂组件的**逻辑**由我们自己写（§2.2.3），它天然是纯函数、可被 vitest 直接测 —— 而不是埋在组件库内部无法验证。

### 6.2 fixture 生产（重点投入）

命令是 **`mcloud fixture extract --har <HAR 目录> --out <输出目录>`**（外加 `--dry-run`）。**已实现**，实测 36 份 HAR → 30 个文件 / 709 条 / 8.3 MB / **0 残留**。

**产物形态**：按 HAR 文件名**平铺**输出 `test/fixtures/http/<HAR 名>.json`（一个 HAR 一个文件，内含该次抓包里所有保留条目）。
文件名经 **`slugify`** 规范化成 kebab-case（HAR 名实测带中文 / 空格 / 下划线 / 大写；中文保留，其余非字母数字折成 `-`），与仓库命名约定一致（§3.1）。**夹具不入库**（`.gitignore` 已含 `test/fixtures/http/`）——它是本机测试数据，每人抓到的流量都不同；干净性由 L2 用例里的 `audit_leaks()` 在每次 `cargo test` 时兜底。

**匹配键**：`method + host + path`（**query 全部丢掉**）——所以夹具对 query 差异不敏感，请求构造的正确性靠测试里**逐字段断言 URL** 补上，不靠匹配。

**脱敏（这几条都是被真实流量打出来的，改动前先读注释）**：

| 规则 | 要点 |
| --- | --- |
| header 走**白名单** | 不是黑名单。黑名单在真实流量上被证伪：`cookieToken` / `x-yun-tid` 这类"名字不在名单里"的 cookie / 追踪头带着可复用凭据原样通过。判据只能是"登记过的才放行" |
| 键名黑名单 + **形状兜底** | 键名由上游决定，无法预知哪个字段装凭据（`{"xxx":"<base64 的 mobile:手机号:token>"}` 很常见），所以键名之外还要按"长串形状"兜住值 |
| 脱敏与自检**共用同一套字符集**，只差阈值 | 脱敏 32 / 自检 40。两侧定义不一致时门禁会恒红且无从修复 |
| **标识形状的数字重映射** | 11 / 19 位、`1` 开头 → 改成 `199` 号段（**不是打码**：撞上这个形状的不只是号码，实测 `taskId` 就是 19 位 `1…`，打码会让它失去形状无法参与解析）。映射由原值 sha256 派生，**确定性**，否则每次重新生成 diff 全是噪声 |
| 表单体**逐对**脱敏 | `k=v&k=v` 与 query 同构，用同一套规则（只打码值、保留键名）。整串按形状打码会把参数名一起吃掉，夹具就没法用来验证请求构造了 |
| `auth` 是 base64 | 里面的号码字面扫描看不见，守卫会**解码后复查** |

**抽取策略**（都是为了让夹具"够用且不失控"，各有注释说明理由）：

- 主机**后缀白名单**：只留上游域（`139.com` / `10086.cn` / `cmcc-cs.cn` / `cmpassport.com`）；APM 埋点、系统遥测、第三方 SDK 一律不进（实测它们把体积从几 MB 撑到几十 MB）
- 单个响应体上限 **128 KiB**，超限**整条跳过**（截断会得到非法 JSON）
- 同一个接口最多留 **2 条**采样（一次抓包会把同一接口打几十上百次）
- JSON 响应**只留 `body_json`**，不再重复存 `body`（两份是同一内容，白翻一倍体积）
- 每次抽取会**清理上一轮留下的陈旧夹具**（它是更宽松规则下的产物，正是最不该留在盘上的东西）

**自检 fail closed**：写盘前先扫一遍，有残留就**拒绝写出**并报出 `$路径 + 形状 + 长度`，退出码非 0。

**首批覆盖**（用户已确认四域全要，见 §7.1 的抓包顺序）：核心域（含**登录**）、活动类、文件与笔记、口令与直播 WS。

> ⚠️ `har/` 内是**真实登录凭据**。原始 HAR 必须 `.gitignore`（`.gitignore` 已含 `har/` 与 `*.har`）。

### 6.3 请求对拍（重写不跑偏的核心保障）

**对拍基准 = 重新抓取的真实流量**，不是 TS 原版。

夹具里连"**请求那一侧**"一起录下来了（`method` / `url` / `body` / `headers`），所以 L2 用例可以直接断言：

1. **必须命中夹具** —— `FixtureHttpClient` 未命中返回 `FixtureMiss` 而**不回退真实网络**，所以"测试通过"等价于"请求确实打到了那个 `method + host + path` 上"；
2. **再断言请求逐字段正确**（URL、body、content-type，必要时 headers）—— 只断言"没报错"等于什么都没测；
3. 签名 / 时间戳 / 随机数类字段无法逐字段对拍，改用**固定输入做算法对拍**（同一 key + IV → 同一密文），密码学层单独对拍一次（见下）。

> **旧 TS 仍可参考，但它是待核对的线索，不是基准**。两者不一致的地方真实存在，且不止一处：

| 接口 | 旧 TS 写法 | 真实流量 |
| --- | --- | --- |
| `visitlog/journaling` | 参数放在 **query** | 参数放在**表单体** |
| `signin/task/taskListV2` | `clientVersion=11.2.1`、`group=month` | `clientVersion=11.4.0`、`group=new` |
| `auth-service/auth/tyrzLogin` | body 带 `type` / `channel` | body 无这两个字段，`marketName=newsign_139mail` |
| `market/signin/page/infoV3` | 不带端口的 `/market/...` | `caiyun.feixin.10086.cn:7071/ycloud/...` |
| `noteRefreshToken` | `note.mcloud.139.com/noteServer/...` | `note-njs.yun.139.com/yun-note/...`（另一个域） |
| `auth/refreshToken` | 注释称"明文 JSON 接口" | 真实请求与响应都是**加密载荷** |

另有两个信封上的坑要记住：这类接口的 `code` 可能是**字符串**（`"0"`）而不是数字；`success: false` 必须**优先于** `code` 判失败 —— 把 `success` 折进 `code` 会得到 `0`，于是失败被判成成功。

**密码学对拍的具体落法**（`test/golden/crypto-vectors.json`，由 `scripts/gen-crypto-vectors.cjs` 从 TS 原版库直接产出）：

- 可字节级对拍（共 **70** 例）：MD5 9 / SHA256 9 / HmacSHA256(base64) 3 / AES-CBC 固定 IV 20 / DES-ECB NoPadding 18 / DES-ECB PKCS7 解密 3 / 签名头 3 / `getTn` 4 / AES-192 动态 IV（固定 IV 版）1；
- 不可字节级对拍（随机性所致，改成单向验证）：
  - AES 动态 IV：固定 IV 版本做字节级；另给一份 TS 随机 IV 密文，要求 Rust 能解回原文；
  - RSA PKCS#1 v1.5：给一对**自有** 1024 位测试密钥，要求「Node 加密 → Rust 私钥解回」与「Rust 加密 → Rust 自解」双向通过；真实公钥（cmpassport 设备指纹用的那把，名字叫 `skland`，见 §1.3）只断言 128 字节结构；
  - gzip：断言可解回原 JSON + OS 字节 == 19（`flate2` 默认不是 19，必须显式写）。

> 该脚本**不做任何网络与账号访问**，只固化算法与公开常量（盐值、DES key、那把公钥都已在原 TS 源码里明文提交）。

这是**无法逐字段对拍的那部分**（签名 / 加密 / 压缩）唯一可验证正确性的手段 —— 请求构造本身由 §6.3 的夹具回放兜住。

### 6.4 门禁

**当前已落地的六项**（改完代码跑这六条，全绿才算完成 —— 与 `README.md` 一致）：

| # | 命令 | 守住什么 |
| --- | --- | --- |
| 1 | `cargo test` | 单元测试 + L2 夹具回放（无夹具时跳过） |
| 2 | `cargo clippy --all-targets -- -D warnings` | warnings 视为错误 |
| 3 | `cargo fmt --check` | 格式 |
| 4 | `bash scripts/check-deps.sh` | 分层铁律：纯层不含 `reqwest`/`rusqlite`、不依赖 `mcloud-io`；组装根必须接上 IO |
| 5 | `node scripts/check-identifiers.cjs` | 仓库里不得有真实标识（字面 + **base64 解码后**复查） |
| 6 | `node scripts/check-golden-vectors.cjs` | `test/golden/*.json` 能算出自己（Node `crypto` 独立实现，101 条） |

> 第 5 条不是可有可无的：夹具与源码字面量都进版本库，而 `auth` 是 base64 —— 字面扫描看不见里面的号码，所以必须解码后复查。

> 第 6 条补的是**第 5 条扫不到的那部分**：第 5 条只扫"有测试去 `include_str!` 的文件"，
> 于是新加的向量文件在接线之前**没有任何守卫**。第 6 条对每个 golden 文件都扫，
> 并把"无 Rust 测试读取"的向量文件直接列出来。
> 它还要求**没有校验器的段必须显式打印**（`NOT_VERIFIED_REASONS`），避免"没验 = 通过"。

**计划追加的**：

- 覆盖率：`mcloud-core` + `mcloud-capabilities` 行覆盖 ≥ 70%（先锁 baseline）；`mcloud-server` 路由 ≥ 60%
- 契约：`openapi.json` 生成物 == 提交物
- 依赖方向（随 §3.2 的两个产物一起加）：**`mcloud-cli` 的依赖树不含 `axum` / `tower-http` / `rust-embed`**
- 迁移：`data/*.json` → SQLite 的导入函数有幂等测试（重复导入不产生脏数据）
- 前端：`tsmc --noEmit`（不是 `tsc`）+ oxlint + Playwright L4；**禁止动态拼接 Tailwind 类名**（自定义 lint 规则断言 `class={...}` 中不出现模板字符串拼接，见 §2.2.2）

> 夹具**不进版本库**，所以它不在第 5 条的扫描范围内 —— 改为由 L2 用例里的 `audit_leaks()` 在每次 `cargo test` 时检查本机夹具（见 §6.2）。

---

## 7. 分阶段实施

| 阶段 | 目标 | 主要交付 | 验收 | 状态 |
| --- | --- | --- | --- | --- |
| **P0 骨架 + spike** | 地基与风险出清 | Cargo workspace 空壳；`fixture extract`；`FixtureHttpClient`；黄金向量生成脚本；**Vapor + TSX + daisyUI 骨架 spike** | `cargo test` 全绿；spike 结论明确（Vapor+daisyUI 基础链路可用 / 需局部 VDOM 回退） | ✅ **已完成** |
| **P1 纯逻辑层** | `mcloud-core` | 类型、trait、错误分类、时间窗口、偏移、归并、增量计算、**crypto（纯密码学）** | L1 覆盖 ≥70%；`cargo test -p mcloud-core <fn>` 可单跑；零 IO 依赖（CI 断言）；**crypto 对拍 TS 黄金向量全绿**（`test/golden/crypto-vectors.json`，脚本 `scripts/gen-crypto-vectors.cjs`） | ✅ **已完成** |
| **P2 IO 层** | `mcloud-io` | reqwest client（重试/幂等/熔断）、SQLite store + 迁移、push 渠道、调度、`Random`/`Clock` 生产实现。**身份解析与旧数据映射的纯逻辑先落在 `mcloud-core`**（`auth.rs` / `legacy.rs`） | 旧 data/*.json 可导入且幂等；`FixtureHttpClient` 未命中必须报错；**身份解析对拍 TS 黄金向量全绿**（`test/golden/auth-vectors.json`）；**旧数据映射对拍脱敏夹具全绿**（`test/fixtures/legacy/`） | 🟡 **基本完成**（缺 push 10 渠道、调度引擎） |
| **P2.5 端到端首条链路：账号登录** ★ | 打通"两个产物 + 用户体系 + 首个真实上游流程" | 系统用户 / 会话 / 云盘账号归属（§3.7）；`mcloud-app` 组装层与**两个产物**骨架（§3.2）；移动云盘**短信登录 + 扫码登录** → `auth`；CLI 与 WebUI 两条入口**共用同一状态机** | 见 §7.0 | ⬜ **当前阶段** |
| **P3 上游接口** | `mcloud-upstream` | **不照抄旧 TS**：按**重新抓取的真实流量**逐域重写（四批见 §7.1）+ prost 直播间编解码 | 每个接口 = 夹具回放 + 请求逐字段断言（URL/body/headers），**未命中即失败** | ⬜ 待抓包 |
| **P4 能力层** | `mcloud-capabilities` | 19 个能力 + 注册表 + FieldSpec | 每个能力 ≥1 纯函数 +1 fixture 用例；`exchange` 仍 `hidden` | ⬜ |
| **P5 服务与 CLI** | `mcloud-exec` + `mcloud-server` + `mcloud-cli` | runId/并发/SSE、axum 路由、OpenAPI、clap 子命令 | L3 契约通过；同 runId 不重复执行；SSE 断线重连 | ⬜ |
| **P6 前端重写** | `web/` | Vapor+TSX+daisyUI 页面 + `src/ui/` 自有组件层，类型全部生成，移动端适配 | 每个能力都能在 WebUI 触发；账号云朵与 30 天趋势可见；表单由 FieldSpec 生成；窄屏可用 | ⬜ |
| **P7 门禁与迁移** | 收口 | Playwright L4 主链路、覆盖率/契约/依赖门禁、数据迁移、原项目归档 | 一条命令跑通全部检查；L5 smoke 通过 | ⬜ |

依赖：P0 → P1 → P2 → **P2.5** → P3 → P4 → P5 → P6 → P7。

> **P2.5 为什么单独成一段，而不是并进 P3**：登录是**第一条真正的端到端链路** ——
> 它同时需要用户体系（§3.7）、组装层与两个产物（§3.2）、上游接口（登录本身是上游的一部分）、
> 以及 CLI / WebUI 两条入口。把它单独拉出来，是为了让"两个产物能构建、用户体系能用、
> 真实上游能通"这三件事**在最小范围内先各验证一遍**；否则会等 P3~P6 全做完才发现分层或用户模型要返工。
> 它**不依赖任何云盘业务能力**，因此可以立刻开工。

P6 的**视觉与组件基建可在 P2 后并行启动**（接口先冻结 `openapi.json` 骨架），但**页面必须等 P5 接口冻结**，否则又会回到"前端硬编码"。

### 7.0 P2.5 的验收（账号登录）★

**系统账号侧**

- CLI `mcloud user init` 与 WebUI 首次访问引导走**同一个用例**、写同一张表；未初始化时 WebUI 只暴露初始化页
- 登录 / 登出 / 会话过期可用；**数据隔离有断言**：两个用户各一个云盘账号，交叉读取必须为空或报错
- 密码用 `argon2` 哈希落库；会话 token **只存哈希**（明文只在签发那一刻返回一次）

**云盘登录侧**（短信、扫码两条）

- 短信登录：手机号 → 发送验证码 → 输入 → 拿到 `auth`；且落库的 `auth` 能被 `core::auth::parse_auth` 解析出 `userDomainId` / `expire`
- 扫码登录：请求二维码（**二维码由服务端下发**，我们只渲染）→ 按服务端给的间隔轮询 → 确认后换到 `auth`
- 两条流程都**先用夹具回放跑通（零联网）**，再接真实链路做一次手工 smoke
- CLI 与 WebUI **共用同一份状态机**：不得出现"CLI 一套、WebUI 再写一套"
- **抓包前不写任何猜测性接口代码**：二维码载荷字段名、轮询接口与间隔、短信前置校验（是否有图形验证码）一律**先确认再落笔**

**落地形态**

- `cargo build -p mcloud-cli` 与 `cargo build -p mcloud-server` 各自能出产物
- `mcloud-cli` 的依赖树**不含** `axum` / `tower-http` / `rust-embed`（§3.2 的待实施断言）
- 两个产物各自都能完成一次登录：CLI 走交互式提示，server 走 HTTP 接口

### 7.1 抓包与重写顺序

**抓包顺序**（按风险递增；四个域用户已确认全要）：

| 批 | 域 | 覆盖范围 | 备注 |
| --- | --- | --- | --- |
| 1 | 核心域 | 签到、领云朵、任务列表、兑换、认证刷新，**以及登录（短信 / 扫码）** | 旧 HAR 有类似流量，但**需重新抓**；登录 **旧项目零实现**，只能逆推 |
| 2 | 活动类 | 直播红花、合成云朵、玩 AI（拍拍 / 抽奖） | 同上，需重抓 |
| 3 | 文件与笔记 | 文件上传、分享外链、云笔记、139 邮箱 | 同上，需重抓 |
| 4 | 口令与直播 WS | `kouling`（WebSocket + 小红书 HTML） | 同上，需重抓（最高风险，最后做） |

**每一批的固定流程（不要跳步）**：

```
抓包 → mcloud fixture extract → 按真实流量写上游接口 → L2（夹具回放 + 请求逐字段断言）→ 再进能力层
```

> 反过来先写接口再找夹具，就会变成"照着旧 TS 猜" —— 而旧 TS 与线上请求本就不一致（§6.3）。

**能力实现顺序**（沿用原计划，按风险递增）：

1. 查询型低风险：`prizes`、`cloud-snapshot`
2. 日常核心：`sign` → `receive` → `revival` → `auth-refresh` → `after`
3. 活动类：`task-expansion` → `app` → `ai-avatar` → `msg-push` → `notice` → `backup-gift`
4. 复杂活动：`hc1t` → `red-flower` → `play-ai-special` → `play-ai-lottery` → `cloud-phone-redpack`
5. 最高风险：`kouling`（WS + 小红书 HTML）、`exchange`（并发抢兑 + 幂等）

### 7.2 当前进度（2026-09-18）

**已完成并验证过**：

| 项 | 状态 |
| --- | --- |
| reqwest client + `FixtureHttpClient` | ✅ 完成 |
| SQLite store（schema v2、迁移、`ConfigStore`/`StateStore` 分离） | ✅ 完成 |
| `SystemClock` / `SystemRandom` 生产实现 | ✅ 完成 |
| `mcloud-core::auth` 身份解析（对拍 TS 黄金向量） | ✅ 完成 |
| `mcloud-core::legacy` 旧数据映射 + 三个陷阱 | ✅ 完成 |
| 旧数据导入器 + `mcloud migrate`（含 `--dry-run`、归档、幂等台账） | ✅ 完成 |
| **夹具脱敏与抽取流水线**（§6.2）：36 份 HAR → 30 个文件 / 709 条 / 8.3 MB / **0 残留** | ✅ 完成 |
| `docs/rust-rewrite-plan.md` 收进本仓库（自 `mcloud-sign` 同步并脱敏） | ✅ 完成 |
| **`mcloud-upstream::login`**：二维码本地拼装、`thirdlogin`（扫码/短信/账密）、发码、滑块、退出 + 轮询状态机 | ✅ 完成（P2.5 第一块） |
| **PC 客户端三个密码学原语**：`decryptByAes` / `encryptByAesCBC` / `encryptByHash`（密钥取自本机 `app.asar`） | ✅ 完成，且用真实抓包**逐字节重放**验证 |
| **`mcloud-core::user`**：`User`/`Session`/`Role`、argon2id 口令哈希、数据隔离判据 | ✅ 完成 |
| **schema v3 迁移**：`users` / `sessions` 表 + `accounts.owner`（事务包裹的 ALTER）、`UserStore` 契约与 SQLite 实现、认领未归属账号 | ✅ 完成 |
| **数据隔离落地**：`UserId` 贯穿 `StateStore` 全部方法（读不到=看不到、写要校验、无主账号对谁都不可见）+ 交叉读取验收测试 | ✅ 完成 |
| **`mcloud-capabilities::login`**：扫码/短信状态机（不 sleep、终态不可逆）、登录结果 → 账号/登录态映射、`auth` 与原客户端逐字一致 | ✅ 完成 |
| **`mcloud-app` 组装层**：迁移策略下移、能力 JSON、设备指纹持久化、首个管理员初始化、扫码登录驱动 | ✅ 完成 |
| **CLI**：`user init` / `user list` / `login qr`（终端二维码）/ `login sms`（含滑块降级），与 HTTP 产物共用组装层 | ✅ 完成 |
| **短信验证码登录全链路**：发码（自动过滑块）→ 输码 → 解密落库，流程在共享层（CLI 与 WebUI 共用） | ✅ 完成，**真机跑通**（首批发码被风控拦下 → 自动解坐标重发 → 输码登录成功 → 落库 `15****42`） |
| **schema v4**：`schedules` / `pipelines` 加 `owner`（事务包裹的 ALTER）、配置类方法全部带 `Scope`、写操作校验归属、认领扩成三笔分别计数 | ✅ 完成（真实 v3 库迁移验证：列已加、既有账号与归属完好、重跑幂等） |
| **WebUI 登录与会话鉴权**：`POST/GET/DELETE /api/session` + `GET/POST /api/setup`、cookie 会话（HttpOnly / SameSite=Lax / 可配 Secure）、未初始化只暴露初始化页、统一错误形状、10 条路由契约测试 | ✅ 完成（`cargo test -p mcloud-server --test api_contract`） |
| **`mcloud-server` 可运行**：`main.rs`（clap）+ 静态发前端产物 + `/api` 未知路径报 404 JSON + 前端登录/初始化页（Vapor + daisyUI） | ✅ 完成（真机跑通：初始化 → 落 cookie → 会话查询 → 登出立刻失效） |
| **App 档案回填 `user_domain_id`**（`x-yun-uni`）：按手机号从 `account_state` 读，CLI 与将来的已登录接口共用同一个函数 | ✅ 完成（含"查不到/空串/属于别人"三种情况一律不带，见下） |
| **`mcloud-upstream::smid`**：cMPassport 风控侧的设备指纹算法 —— `uid=randomUUID()` → `ep=RSA(uid)` + `priId=md5(uid)[0..16]` → 约 31 字段的特征体 → `tn=md5(getTn(特征体))`（**在 `tn` 还是空串时算**）→ 25 条 DES-ECB/NoPadding「改名 + 加密」规则 → gzip（OS 字节写 `19`）→ base64 → AES-CBC/PKCS7 固定 IV → hex 成 `data` → `POST https://slw.h5cmpassport.com:9090/deviceprofile/v4`，`code==1100` 时取 `"B"+detail.deviceId` | ✅ 完成（25 条用例；`test/golden/crypto-vectors.json` 新增 `§smidString` / `§smidPriId` 各 3 条对拍 TS，`§smidTn` / `§smidDesRules` 由 Rust 侧与 Node 生成侧两份实现交叉） |
| **schema v5：`account_state.smid_fingerprint` + `smid_device_id`** —— 两列的**写契约与 `AccountState` 相反**（窄写、不碰整行），所以专门补了一条守卫测试：**重登（`upsert_account_state`）不得抹掉已绑的设备特征** | ✅ 完成（真实 v4 库迁移到 v5 后账号与凭据完好；8 条用例 = 存储 6（往返 / 重开 / 换特征清空旧 `deviceId` / 窄写不动凭据与 `updated_at` / 越权写 `NotFound` / 上面那条重登守卫）+ schema 2） |
| **`mcloud-app::smid` 生命周期层 + CLI `mcloud smid show / reset / refresh`**：**设备特征绑账号**、只在主动更换时重掷；换特征时**同一条 SQL 语句**作废旧 `deviceId`；库里那串解析不出来时**报错并提示 `smid reset`，不偷偷重掷**（静默重掷等于换设备） | ✅ 完成（13 条用例，含"上报请求一个凭据头都不带"的断言 —— 发给第三方风控服务的请求必须裸） |
| **schema v6：`account_state.phone`** —— 存**完整手机号**。因为 `accounts.id` 只有掩码，而 `Authorization` 中间那段要的是完整号码（**不许拿掩码去凑**）。放在 `account_state` 而不是 `accounts`：后者是**直接吐给前端的展示表**，只能有掩码 | ✅ 完成（真实 v5 库迁到 v6、老行留 `NULL` 不回填；登录落库走 `phone_or_none()`；读取侧 `SignInCredential::from_account()` 是首选路径、`from_phone()` 只留给 v6 之前登的号；**端不匹配会被显式拒绝**（拿 PC 登的号去跑 App 签到 → 报错并提示重登）。真实上游实跑：`--account` 与 `--phone` 对同一账号输出逐字相同） |
| **云朵余额采集**（§3.4 的 `record_cloud` 落地）：`infoV3.beforeTotal` → `accounts.cloud_current` + `cloud_stats` 日快照，日/周增量走纯函数 `next_cloud_stats` | ✅ 完成（新增 `StateStore::set_account_cloud` **窄写**方法 —— 不复用整行覆盖的 `upsert_account`，否则"读整行 → 改一列 → 写回"会把别人改的昵称静默写回旧值；历史窗口取**全量**而非近 7 天，窄窗口会把"两周没采集"的增量算成 0；同日重复采集幂等。真机实跑：余额 5392 落库，第二次跑仍是同一行） |
| **`receiveV3` 领云朵**（§4 的 `receive` 能力第一块）：请求构造 / 响应解析 / 错误码语义 + CLI `signin --receive` | ✅ 完成（**上游层的三个错误码全部实证**：不带 `deviceId` → `615`、内容错一个字符 → `614`、真值 → `code=0`。⇒ 上游**校验 `deviceId` 的内容**，所以"随便编一个"走不通。**取值通路已补齐（2026-09-19）**：签到页 JS 自己把它缓存在 WebView 的 `localStorage`（桥只调一次、之后走缓存），`scripts/h5-device-id.py` 只读取出来，与官方 App 实发值**逐字节相同**（sha256 一致）。仍未解的是那 48 字节密文的**密钥**（三把已知密钥在 CBC/ECB × 两种切分下全不对），所以值只能"取"不能"造"，前提是这台机器上用官方 App 打开过一次签到页。⚠ 实测形状有个坑：89 字符 = 87 数据字符 + **2 个** padding（规范只要 1 个），**原样透传、别规范化**；首字节 0x05/0x06 都见过，**不是固定魔数**。顺带把 `infoV3.receiveList` / `toReceive` 解析出来 —— `--cloud-type` 的取值来源就是它。详见 `docs/capture/app-signin-samples.txt` §4.2 / §4.3） |
| **`revival` 复活卡 / 回馈卡**（§4 能力清单第 3 项）：请求构造 / 响应解析 + CLI `signin --revive` | ✅ 完成（**接口是 `POST /ycloud/signin/page/receiveRevivalReward`**，H5 源码逐字：`receiveRevivalReward({})` ⇒ body 是**空对象 `{}`**，整页唯一一个 `showLoading: false`。实拍响应 `{cardType:1, littleFirstTime:0, rewardClouds:114, totalClouds:571}` —— `cardType` 1 = 复活卡 / 2 = 回馈卡，两张卡连累计文案的单位都不同。⚠ **它不可逆**（H5：「关闭弹窗视为放弃机会，无法补发」），所以做成独立开关、默认不开。⚠ **`totalClouds` 没有接进 `record_cloud`** —— 规划 §3.4 把它列为余额来源③，但 H5 文案说复活卡产出的是 **AI豆**，与 `infoV3.beforeTotal` 可能不是一个池子，**没有实拍能分辨**，接进去只会把两个口径混进同一条曲线。⚠ `deviceid` 头：H5 定义没有 `isDeviceId`、老抓包却带了 ⇒ 做成可选参数而不是猜一个。**顺带修掉一个静默空操作**：`--json` 原先在 `--receive` 之前就 `return`，导致 `--json --receive` 什么都不做且不报错；现在两个写开关都跑在 json 分支之前，结果也进 JSON。另：§7 的接口清单按正则重新枚举过，**原表少了 14 条**，已补全为 43 条） |
| **H5 `deviceId` 根因查明 + 接线**（拆掉 `receiveV3` 唯一的拦路石）：签到页的 `deviceId` **就是 `smid` 换回来的那个**，`--receive` 不再需要任何手工参数 | ✅ 完成（**根因**：签到页的 `fp.min.js` 是数美设备指纹，`window.SMSdk` 是**它自己定义**的（所以 App 二进制里搜不到这个名字），`getDeviceId()` 返回 `'B' + localStorage['SMID']`，而 `SMID` 是它往 `slw.h5cmpassport.com:9090/deviceprofile/v4` 上报后拿回的 `detail.deviceId`；那套配置写在签到页 HTML 的 `window._smConf` 里，与 `mcloud-upstream::smid` 的常量**逐字相同**（含 RSA 公钥）。⇒ 曾经的"65 字节 = `1+16+48`、48 字节密文密钥是独立的第四把、未解"是**从错误的解码方式推出来的**：把 `'B'` 前缀当成了 base64 数据，那个"会变的 0x05/0x06 首字节"其实是前缀与 SMID 首字符**共用**出来的字节，真实结构是 `'B'` + 64 字节，**根本不是密文**。**实测**：`mcloud smid refresh` 纯 Rust 换回 `B…Dw==`（89 字符、`B` 前缀、64 字节，与 HAR 真值同形）。**接线**：CLI 新增 `resolve_device_id`（显式 `--device-id` → 否则库里 `smid_device_id`，没有就现上报一次），`--receive` / 签到都走它；`--revive` **故意不回落**（H5 没有 `isDeviceId`，默认就该一个头都不带）；`--h5-device-id` 改名为 `--device-id` 并保留为别名。守卫测试 `the_signin_page_fingerprint_sdk_is_this_same_client` 把两边常量钉在一起（常量漂移时先炸，而不是"领取忽然回 615"）。⚠ **仍缺一次 A/B**：Rust 产出的 id 打 `receiveV3` 上游认不认 —— 卡在账号 `account_state.phone` 为 `NULL`（v6 之前登的号），要么重登一次、要么 `--phone`。在那之前结论只到"不需要浏览器"，不到"领取已跑通"） |
| **schema v7：`Authorization` 整串存、整串用** —— 拆开存再拼回去改成原文存取；`phone` 降级为"标识"（不参与拼头） | ✅ 完成（**形状**是 `Basic base64("<前缀>:<手机号>:<authToken>")`，登录时**一次就拿到整串** —— 旧项目 `mcloud-sign` 的 `asign.json` 里 `auth` 存的就是它，一直直接用。我们重写时解码 → 拆三段 → 只留 token → 要求用户每次补手机号，是**绕远路且会错**：① 必须凑齐手机号；② 前缀被硬编码成 `mobile`，猜错**静默**发错头、表现为 `90001` 未登录。⇒ 新增 `account_state.authorization`；读侧**整串优先**，`phone` + `jwt_token` 现拼只作 v7 之前老行的退路。**闸门顺序也一并修正**：`platform` 前缀闸与 `jwt_token` 非空闸原先排在整串分支**之前**，会把"库里只有整串"的行误判成"没有凭据"（而那正是回填造出来的形态）；这两道闸的前提在整串这条路上都不成立，故移到退路里。**实测**：整串路径下 `mobile:` 与 `pc:` 两个账号的 `--status-only` 都正常（余额 5392 / 8027）—— `pc:` 那条同时证明了"前缀闸不该管整串"（旧项目也从不区分，两个前缀走同一条签到路径）。**迁移回填**：`asign.json` 的 `auth` → `account_state.authorization`，走新窄写方法 `fill_account_credentials`（**只填空缺、不覆盖**，故幂等、可每次 `migrate` 都跑）；它**不受源文件哈希门控**（老库的 `state.json` 早导过了，走门控就永远补不上 —— 那正是 `write_state` 里那条"两种库形态"注释描述的问题）；`write_state` 改为把库里现有的两列**原样带过去**（`state.json` 对它们没有主张），否则整体覆盖会先抹掉、再被旧配置里的串填回来，把重登后的新凭据**换成旧的**。报告新增 `凭据回填 N`（**实际**补上的个数，无缺口不计入）。**CLI 零参数 + 下标**：不带子命令直接 `mcloud` = 对全部启用账号各签一次（对齐旧项目"直接运行、都没参数"）；`--account` 收**下标**（`--list` 查编号）而非掩码 id；`--phone` 补一次即落库、下次不必再输；一个账号失败不中断其余、最后用非零退出码统一报数。**顶层收全套签到参数**：顶层那个 `SigninArgs` 是 `#[command(flatten)]`，与子命令里的**同一个结构体** —— 早先顶层只认子命令，`mcloud --list` / `--status-only` / `--json` / `--db X` 会被 clap 判成"未知参数"并报 `Usage: mcloud [COMMAND]`，一个主打"直接运行"的命令行却要用户先知道子命令叫 `signin` 才能查编号。平铺后默认值、互斥关系、帮助文本**只有一份定义**（原来那种"再解析一遍 `["mcloud","signin"]`"的写法一旦有人改回手写字面量，漂移只在"直接运行"时表现为"某个默认值没生效"，最难被发现）。**顺带修掉一个既有 bug**：`--json` 时进度行也打到 stdout，`mcloud signin --json \| jq` 直接语法错 —— 现在进度一律走 stderr，单账号给对象、多账号给数组。**`--phone` 只记号码、不记拼出来的串**：那一列的口径是"服务端给的原文"，写进一个本地合成物等于给它伪造来源 —— 它会走整串优先路径、**绕过前缀闸**，于是此后每个请求都带着一个我们自己安上去的前缀且没人再核对；`pc:` 端的老行被 `--phone` 补一次就会存下**永久错的**凭据。只记 `phone` 即可（下次照旧现拼、闸照旧生效），用户同样不必输第二遍。**新增 8 条 CLI 用例**（用 clap 真解析 `mcloud signin …`，连 `conflicts_with_all` 一起验）：零参数只跑启用账号且下标用**全列表位置**（停用的位次不被顶替）、`--account` 数停用账号、下标从 1 开始、越界要报总数与 `--list`、`--phone` 空库也能选中且下标记 0、`--list` 与二者互斥、不带子命令补出来的确实是签到且默认值逐字一致） |
| 六项门禁（`cargo test` / `clippy -D warnings` / `fmt --check` / `check-deps.sh` / `check-identifiers.cjs` / `check-golden-vectors.cjs`） | ✅ 全绿，**586 个测试**（前端另有 `npm run typecheck` + `vite build`）。这一行的数是一路涨上来的、**每次都会当天过期**，按时间顺序：本行最初记于 **365 个** → 接完上面那条 App 档案回填线 **421 个** → 接完上面三条 smid 线 **493 个**（同日）→ 签到线补齐上游接口层 / 编排层 / 存储层（`account_state.phone`，schema v6）后 **516 个** → 接上云朵余额采集后 **523 个** → 接上 `receiveV3` 领取（上游层请求构造 / 解析 / 错误码 10 条）后 **533 个** → 接上 `revival` 复活卡（上游层请求构造 / 解析 / 错误码 8 条）后 **541 个** → 查明 H5 `deviceId` 根因并接线（新增常量等价守卫 1 条）后 **542 个** → 接上 schema v7 整串凭据（窄写"只填空缺"2 条 + 整串优先 1 条 + 迁移回填的幂等与不降级 1 条）后 **546 个** → 补 CLI 账号选择用例（下标语义 / 越界 / `--phone` / `--list` 互斥 / 零参数落到签到 / 顶层与子命令等价，9 条）后 **554 个**（2026-09-19 收工，实测 554 passed / 0 failed）。⚠ **365→493 那 72 个不是一条线贡献的**：smid 那条 46 个（上游算法 25 + 生命周期 13 + 存储 8），同时在飞的 App 签到线 26 个 —— 别把整段算成自己的账，也别据此认为签到线已经完成。黄金向量脚本 **122 条断言全绿**（`§smidString` / `§smidPriId` 是 smid 那次新增的 6 条）。⇒ 并行开工时以 `cargo test` 现测为准，本行的数只是当时的一帧。**另一条当时踩出来的教训留着**：这条线在飞时 `fmt --check` 红过，红点**全不在 smid 这条线**上，而在并行那条线的 `crates/mcloud-app/src/signin.rs` 与它写进 `mcloud-cli/src/main.rs` 的那两处；本行涉及的每个文件单独 `rustfmt --check` 都干净 —— **别为了凑绿去格式化别人的在飞文件** |

> **2026-09-19 订正上表两处**：① 门禁清单原先写成"分层 / 产物解耦"两项来凑够六条，
> 于是**漏掉了黄金向量守卫**。实际六项是 `check-deps.sh`（一条，内含分层与产物解耦两个断言）
> 加 `check-identifiers.cjs`、`check-golden-vectors.cjs` —— 以 [`dev-gates.md`](./dev-gates.md) 为准。
> ② 末数 **554 → 586**：拉取云端那 9 个提交（`mcloud-app` 组装层等）落地后，新模块另有自己的 lib 测试。

**旧数据实测**（`Documents/workspace/mcloud-sign`）：账号 2 / 登录态 2 / **凭据回填 2** / 组合 1 / 历史 96 条 / **问题 0 条**；
二次执行 6 个数据文件全部跳过、0 次写入、**凭据回填 0**（没缺口就不计数），行数不变。

**已回退（重要）**：P3 曾按旧 TS 搬过 `signin` / `exchange` / `task` / `auth` 四个模块与 15 个 L2 用例，
**现已全部删除** —— 上游改为按新抓包重写，`mcloud-upstream` 除 `login` 外只保留骨架
（`Upstream` + `Envelope` + `base`）。

**已记录待实现**：**各端设备指纹与请求头差异** → [`docs/client-profiles.md`](./client-profiles.md)。
差异主要在请求头（认证/设备/渠道/埋点/签名各端一套），请求体只有 `clientType`/`cpid`/`version` 随端变化。
该文档记录了实测证据（709 条夹具按 UA 分组）、PC 端头的代码级来源、设备指纹 14 段字段表，以及未知项清单；
落地时机是"接第二个端的时候"。

**P2.5（账号登录）到此闭环**：用户体系（schema v3 + `UserStore` + 数据隔离）→
「首管理员初始化」→ 扫码/短信登录状态机 → 「登录结果落成账号与登录态」→ 两个产物共用组装层，
CLI 侧已端到端跑通（`user init` → `login qr` / `login sms`）。

**数据隔离的覆盖面（v4 后）**：账号、登录态、运行记录、定时任务、组合**都已带归属**，
且过滤逻辑只有一份表达式（`(?n IS NULL OR owner = ?n)`）。
只有两处**故意保持全局**：`exchange_config`（单行表，`CHECK (id = 1)`）与 `webui_config`（界面偏好），
见 §3.7 的说明。

**尚未开始**：各端设备指纹/请求头档案的完整形态（见上，[`docs/client-profiles.md`](./client-profiles.md)；
登录链路的必需部分已落地为 `mcloud-upstream::client::ClientProfile`）、
**P6 的真实页面**（当前主界面仍是 P0 spike 的占位数据；登录/初始化页已是真接口）、
push 10 渠道（§1.4）、调度引擎（cron 解析与触发）、
OpenAPI 导出（`utoipa` → `openapi.json` → 前端生成类型，见 §5）。

**已记录的小缺口**（逐条标注状态；都不影响当前可用性）：

> 第 3 条（App 业务接口的两把锁 + `hcy-cool-flag` 信封）**已全部解开** ——
> 连同"三条加密点"在内，App 侧已**没有未解密的点**。往下三条按"怎么错的"留着。

- **`user_domain_id` 落空** —— **已闭环（2026-09-19 真机登录实测）**。
  这条缺口原话是"登录响应的 `userInfo` 里没有 `userId`，需要用它的接口得另找来源"。
  现在把响应字段名打印出来了（只打印名字、不打印值）：**字段名就叫 `userDomainId`，
  `userId` 根本不在响应里** —— 原来那个 `userId` 是**请求体**的键名，由 `userDomainId`
  赋过去。App 与 PC **都调 `POST /user/thirdlogin`**，所以这条对两端同时成立。
  落库后 `account_state.user_domain_id` 有值（19 位）、`jwt_token` 201 字符、
  `auth_expire` 正好 30 天后。
  事前是靠"统计 PC 客户端源码里 `UserInfo.<字段>` 被读的次数"反推出来的
  （`userDomainId` 11 次 / `userId` 0 次），事后与实测字段清单完全吻合。
  详见 `docs/upstream-login-protocol.md` §5、`docs/client-profiles.md` §3.3.2、
  `docs/upstream-app-sms-login.md` §7.4。
  ⚠ **还差一段线没接**：`AppProfile::with_user_domain_id` **只在测试里被调用过**，
  生产路径没把库里存的值回填进 App 档案。现在不痛（App 端还没有已登录接口），
  但**下一批抓包的第一个已登录请求就会缺 `x-yun-uni`** —— 先接这条线，再写接口。
  **已接（2026-09-19，上面那段是当时的实况，留着）**：
  `mcloud_app::app_login::profile_with_stored_domain_id` 按手机号从 `account_state`
  回填，CLI 的 `login sms --client app` 在建档案后立刻调用它。
  回填**登录握手请求**是有实测依据的，不是想当然：抓包里真机的
  `getSmsCode` 与 `thirdlogin` **都带着** `x-yun-uni`（`docs/capture/app-sms-login-samples.txt`
  §1 与 §3.2）—— 它缓存了上一次会话的 `userDomainId`，不会因为"这次在登录"就不发。
  查不到时原样不带：编一个会把这次请求定位到**别人**的用户域，比缺头更糟。
  读回填用的 `Scope` 与写落库同一个，且"属于别人的账号读不到"由隔离契约兜住（有测试）。
  下一批已登录接口直接复用这个函数即可，不要再各自拼档案。
- **WebUI 的会话鉴权未接**：`mcloud_app::session`（登录本系统、签发/校验/撤销）已实现且有测试，
  但还没有 HTTP 层去用它 —— 那是 P5 的 `routes/` 骨架。CLI **不需要**它（见 §3.7 的分界说明）。
  **已接（2026-09-19，schema v4 那一批，上面是当时的实况）**：HTTP 层落在
  `mcloud-server/src/auth.rs`（`/api/session` 三个方法 + `/api/setup` + cookie 会话与
  白名单判定），`mcloud_app::session` 有了调用方。
- **App 业务接口的两把"锁"：`signature` 与 `encodeData`** —— **已解开（2026-09-19）**。
  App 里有**两套互斥的请求形态**（12243 条零反例）：
  **A 族**带 `x-yun-uni`、没有 `signature`；**B 族**带 `signature`、没有 `x-yun-uni`。
  登录链路学到的那套头**只覆盖 A 族**。
  - **`signature`** = `SHA256("clientId=iosapp&clientKey=BOFlEUINUdSugD6u&msgId=<msgId>
    &timeStamp=<13位毫秒>&version=<与请求头逐字相同，含 V>")`
    —— **6165 / 6165 条实拍逐字命中**；
  - **`encodeData`** = `base64(16 字符 ASCII 的 IV ‖ AES-128-CBC/PKCS7(key=同一把, iv=IV, 明文=手机号))`
    —— **6159 / 6159 个样本全部解出**，明文去重后只有 1 种、长度 11（即手机号）。
  - 密钥怎么来的：iOS 二进制里**没有** `clientKey` 字面量，
    用 5632 个 16 字符候选做 SHA256 比对命中，再用同一把 key 做 AES 解密交叉验证。
  - 黄金向量：`test/golden/app-business-crypto-vectors.json`
    （`signature` 是实拍向量；`encodeData` 是**合成**向量 —— 实拍密文的明文是真实手机号，
    而密钥要写进仓库 ⇒ 实拍密文等于公布手机号）。
  - 完整头集、复现命令与仍未解的边界见 `docs/upstream-app-business-api.md`；
    形态分类与逐端点可行性见 `docs/capture/app-api-families.txt`。
- **第三条：`hcy-cool-flag` 信封** —— **已解开（2026-09-19 深夜）**。
  这个头声明的**不是**"响应请加密给我"，而是 **"我这个请求体是加密的"**
  （响应加密只是同一选项的另一半 —— 这条因果此前写反了，见
  `docs/upstream-app-sms-login.md` §7.1 的更正）：
  ```
  hcy-cool-flag: <任意值>      ← 不发则 body 按明文解析
  body = base64( IV(16 字节，二进制) ‖ AES-128-CBC/PKCS7(key, iv=IV, 请求 JSON) )
  key  = "uDjGSA8auFlbwDkY"    ← iOS 二进制字符串常量
  响应 = 同格式；包装有两种（裸 base64 / JSON 引号包着的），解析时都要认
  ```
  - 密钥怎么来的：拿**已知明文**（长度线索 + 服务端自己回的错误）暴力命中，
    201124 个候选里唯一一个 —— 判据是"解出的明文里必须出现 `200059554`"。
  - **活体探针已走通**：把请求体按该格式加密发出去，服务端不再回
    `01000019 Illegal base64 character 7b`，而是正常处理并回 `200059554`。
  - 黄金向量：`test/golden/app-business-crypto-vectors.json` 的 `hcyCoolFlagEnvelope`
    （3 条，**含 2 条实拍** —— 实拍明文里没有标识，所以这次不必合成）。
  - ⚠ **实现侧目前不发这个头**（发码接口上不发时语义与抓包一致）。
    解开它的价值是"再也不用猜"，不是"必须带上"。
- **smid 算出来了，但还没有请求发它** —— **有意留着，不是漏了**（2026-09-19）。
  旧项目把 `getDeviceId()` 的结果原样发成请求头 `deviceid`
  （`packages/shared/src/http/base.ts:44-45` 静态头、`:92-95` 「所有请求」注入、
  `:166-181` `createTaskHttp`，账号初始化在 `core/src/utils/init-account.ts:33-40`）。
  我们**没有照搬进 `AppProfile`**，两条实测依据：
  ① App 签到那批抓包里，原生请求头 `deviceid` **一次都没出现**
  （`docs/capture/app-signin-samples.txt` §4 那张「两个同名不同物」对照表 ——
  那一批带 `deviceId` 的是 **H5 的 65 字节 blob**，与 smid 的 `B…` 不是一个东西）；
  ② 把 `test/fixtures/http/` 的 711 条夹具扫一遍，带 `deviceid` 头的 178 条
  **全是 H5 / WebView**（iPad UA、Android `wv`、活动页 iPhone UA），原生 App 流量一条没有。
  所以取值的那一半已经就位（`mcloud_app::smid::device_id`，库里有就不打网络），
  **注入的那一半等一条原生抓包**：届时按那个接口的要求显式带上，逐端点判断，
  而不是塞进公共头替所有接口做决定。详见 `docs/client-profiles.md` §5.4 ③ 与 §7。

**CLI 新建账号的归属策略**：库里已有用户时归**首个用户**，没有则保持无主
（等 `mcloud user init` 认领）。这不是权限检查，而是"账号必须有主人"的落地 ——
否则网页端按隔离规则看不到它，会出现"扫码成功了、网页里却没有这个账号"。

**业务功能明细已单独成文**（2026-09-19 追加）：[`docs/business-feature-inventory.md`](./business-feature-inventory.md)
—— 19 项能力逐条的**上游请求序列 / 判据分支 / 副作用分级 / 幂等缺口 / 依赖与共享状态**，
外加运行时层缺口（run 与并发闸、调度、组合、推送 10 渠道、CLI、WebUI）与硬编码常量集中清单。
它**不改 §7.1 的实现顺序**，只补"每一项能力内部到底怎么走"这一层。
其中该文 §5 的「动代码前必须实测确认的冲突」是 §6.3「不照搬旧 TS」的**具体检查表**
（例如旧代码写 `/market/signin/…` 而我们抓到的是 `/ycloud/signin/…`，前缀与 `taskListV2/V3`、
`doTask/doTaskPost` 的分歧都记在那里，未实测前不写接口）。

**参考项目的对应盘点也已单独成文**（2026-09-19 追加）：[`docs/caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md)
—— §2 点名的 Go 参考项目（`/tmp/caiyun-ref` @ `1b2cf07`，backend 61,215 行）逐条对照：24 条任务目录、
抢兑子系统（一次下单的 11 步 + 响应分支表 + 12 类错误→重试决策表 + 三层锁与 double-spend 面）、
运行时层（日幂等的确切语义、熔断状态机、6 类令牌、事件与推送、跨任务态建模）、参数面全量与
**搬 / 不搬 / 先修再搬**总表。它是 §2.4「明确不照搬参考项目」的**逐项落实**：
MySQL / Redis / 多用户+RBAC / K8s / 监控栈 / API+Worker 双进程各自的替代物写在该文 §8.2，
"哪些是他们踩过的坑、我们不能继承"汇总在其 §8.4（编号 `T-*/X-*/R-*/C-*`，均带 `file:line`）。
该文 §7 与旧 TS 盘点篇 §5 是同一张检查表的两个来源，**三源冲突时以我们的抓包为准**。

---

## 8. 兼容、迁移与安全

### 8.1 旧数据实况（已实测，迁移以此为准）

旧项目的运行数据**不在仓库根目录**，而在 `packages/api/data/`（`storage.ts` 里 `DATA_DIR` 默认 `resolve("data")`，
相对于进程 cwd；从 `packages/api` 启动时就是这里；`.gitignore` 忽略的是 `packages/api/data/`）。
实测该目录内容：

| 文件 | 实况 | 迁移要点 |
| --- | --- | --- |
| `asign.json`（仓库根） | `{version:2, caiyun:[{auth, nickname}]}`；**真实配置里没有 `phone` 字段** | 号码只能从 `auth` 现场解析；`auth` 是唯一凭据来源 |
| `state.json` | `{accounts:{<手机号原文>:{jwtToken,deviceId,userDomainId,cloudPhoneRedpackToken}}}` | 键是旧 `accountKey`（**手机号原文**）→ 新版换成掩码 `AccountId` |
| `history.json` | `{runs:[RunRecord]}`，96 条，上限 200 | `runs[].account` 存的是**展示名**（昵称/掩码号），不是标识 |
| `pipelines.json` | `{pipelines:[PipelineDef]}` | 有废弃字段：单值 `account` 迁移为 `accounts`；**步骤级 `account` 直接丢弃** |
| `schedules.json` | `{schedules:[]}`（实测为空） | 字段定义只能从 `scheduler.ts` 的 `ScheduleDef` 取，不能靠样本推断 |
| `exchange.json` | `{enabled,windows:["09:59:40","11:59:40"],bindings,scheduleIds,updatedAt}` | 窗口 `HH:mm:ss` → 6 段 cron，且 `Number()` **去掉前导零**（`09`→`9`） |
| `webui.json` | `{rememberLastPage,lastPage,logExpanded,backgroundRun,updatedAt}` | 两个布尔默认方向**相反**，见下。**注意**：这是**旧前端**的界面偏好；新前端不参考旧实现（§1.6），界面偏好自行设计 —— 这里保留它只为"迁移不丢数据" |

`.migrated` 文件（值是一个 epoch 毫秒时间戳）是**更早一版迁移留下的标记**，不是本方案的产物。

`history.json` 的日志条目形如 `{level, type, msg, date}`：`level` 是数值严重级（实测只有 `0`=error / `3`=info / `4`=debug），
`type` 才是语义（`info`/`start`/`success`/`debug`/`error`）——**同一个 `level` 对应多种 `type`**，只看 `level` 会丢掉 start/success。

### 8.2 四个必须处理的旧版陷阱（已在 `mcloud-core::legacy` / `mcloud-core::auth` 里钉住）

1. **`/^\d+$/` 分支在昵称匹配之前**（`account.ts::findAccount`）：纯数字字符串一律按**序号**解释，
   于是"用手机号当账号引用"**永远匹配不到**，定时任务每次记一条 `账号 19900000001 不存在` 然后什么都不做。（号码按本仓库约定替换为合成号段）
   迁移时这类引用会被标为 `RecoveredFromPhone`（可按手机号恢复，但**属行为变更，需人工确认**）或 `WasNeverEffective`。
2. **掩码撞车**：`AccountId` 用掩码（前 2 + `****` + 后 2），两个号码可能撞到同一个 id。
   一旦发生，两个账号会塌缩成一个且**不报错** —— 所以 `build_identity_map` 直接报错中止迁移。
3. **`webui` 布尔默认方向相反**：`rememberLastPage` 是 `!== false`（缺省 true），
   `logExpanded`/`backgroundRun` 是 `=== true`（缺省 false）。照搬时不要"统一"成一种。
4. **`auth` 里的 `expire` 单位是毫秒，不是秒**（实测，不是推断）。
   旧版把它与 `Date.now()` 直接相减（`auth/utils.ts::isNeedRefresh`：`expireTime - Date.now() < days * 86400000`），
   真实 `asign.json` 里也是 13 位（`1790818813374` / `1790818876407`）。
   **这个错是静默的**：拿 13 位毫秒去比秒级时间戳，`expire - now` 恒为约 `1.25e9`，
   于是"该刷新了吗"永远回答"否" —— auth 刷新永不触发，直到上游开始回 401 才暴露。
   因此 `mcloud-core::auth` 的访问器把单位写进名字（`expire_millis`，而不是 `expire_unix`），
   并把"过期"与"该刷新"拆成 `is_expired_at` / `is_need_refresh` 两个方法（前者是后者的下游、不是同义词）。
   **对拍的取值也必须是毫秒量级**：黄金向量与 `test/fixtures/legacy/` 都改用 13 位，
   否则夹具与真实数据差三个数量级，这个问题在测试里根本看不出来（早先就踩过这个坑）。

### 8.3 其他迁移约定

- **配置不变**：`asign.json`（version 2）继续沿用，字段与语义不动。
- **CLI 对等**：原有参数与语义保持一致；`--version` 取自 `CARGO_PKG_VERSION`（替代 `__APP_VERSION__`），同时暴露 `GET /api/version`。
- **数据迁移（已实现）**：命令是 `mcloud migrate`，实现分三层 ——
  `mcloud_io::import_legacy`（读旧数据落 SQLite）、`mcloud_server::migration`（执行策略：归档顺序、报告形状）、CLI（参数）。
  几处刻意决定：
  - **`--dry-run` 走的是真实导入路径**（在内存库上跑完再丢掉），而不是另写一套"只检查不写入"的代码 ——
    否则诊断与导入会各自演化，"预演说没事、真跑报错"迟早出现。
  - **归档不是回滚用的 `.bak`**：旧目录自始至终只读、一个字节都不改。
    归档是为了让新项目**自带一份可离线重放的原始数据**，旧仓库将来归档掉之后这次迁移仍能被检查和重跑。
    归档发生在建库之前；内容相同的文件跳过，故重复执行不写盘。
  - **掩码冲突在任何写入之前中止**（连台账都不落），代价是"迁移没做"而不是"迁移做错了"。
  - **绝不静默扩大目标范围**：解析不到的账号引用**不回落**到"全部账号" ——
    那是把"过去什么都没跑"变成"现在对所有人跑"。定时任务导入为**停用**，组合（无 `enabled` 字段）**整条跳过**。
  - 幂等靠**稳定的 UPSERT 主键**（账号用 `AccountId`、运行记录用内容哈希 `legacy-<hash>`），
    台账只在写入完成之后落；源没变则整份跳过（省时间，不承担正确性）。
  - 运行记录的 `logs` / `result` 不进新表，但**整份原文归档在 `imports.raw`**，将来要回填随时能回填。
- **日志语义沿用**：`error`/`fatal` 判失败、`fail` 为 info 级业务结果不影响退出码；无 `error`/`fatal` 时 CLI 退出码 0。
- **凭据边界**：`auth`/`token`/`jwt` 只落本地存储，**不出现在 API 响应、日志、fixture**。但同时——配置绝对路径、完整收件地址、临时文件 ID 有实际排障价值，**照旧保留，不做机械脱敏**。
- **旧数据夹具**：`test/fixtures/legacy/` 由 `scripts/gen-legacy-fixtures.cjs` 从真实数据**脱敏**生成（凭据→同长度规律串、手机号→确定性重映射，`auth` 用映射后号码重建，`expire` 保持 13 位毫秒），
  脚本写盘前会拿原件里的敏感值反向搜一遍输出，命中即中止。改脱敏规则要连夹具一起重生成。
- **"要进版本库的生成物"的凭据边界**（这一条是踩过才补上的）：
  - 夹具与黄金向量（`test/golden/`）都是脚本从**本机真实数据**产出的，因此必须脱敏 —— 手机号统一重映射到 **199 号段**（`19900000001` 起）。
  - **反向自检的 haystack 必须包含 `$meta.json`**：元信息最容易被"顺手写下原值以便对照"，而它曾经被漏扫，导致真实手机号被写进版本库。
  - Rust 侧用 `mcloud_core::test_support::assert_only_synthetic_phones` 断言"出现的每个 11 位号码都是 199 开头"，作为第二道防线（夹具被手改也能发现）。
    判定要**卡边界**（号码两侧不能是字母数字），否则 sha256 十六进制串里的 `…18531489942b…` 会误报。
  - `.gitignore` 必须覆盖 `mcloud migrate` 的默认归档目录 `/data/legacy-source/`：它含旧 `state.json` 的真实 `jwtToken`/`deviceId`，
    漏掉就会随一次 `git add .` 一起提交。
- **`userDomainId` 是刻意保留的真实标识**（不是凭据、不可直接复用、有排障价值），与"auth/token 一律不得入库/出日志"的边界并列，不要机械地一并脱敏。
- **原项目处置**：`Documents/workspace/mcloud-sign` 保持原状只读归档，重写期间不动它；新项目独立仓库（`mobile/workspace/mcloud`，分支 `main`）。

---

## 9. 非目标与风险

**非目标**：多用户之外的**角色权限矩阵**（只有 `Admin` / `User` 两级，不做细粒度授权）、云端托管、MySQL/Redis、K8s、txiki 双运行时（见 §2.3）、移动端独立组件库（响应式足够，见 §2.2.5）。

**不引入**：JSON 配置的运行时 schema 校验 —— **待拍板**（旧项目 schema 形同摆设，见 §1.6 待拍板项 3；本方案的 `FieldSpec` 只约束**能力入参**，不是配置校验）。

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| **动态拼接 Tailwind 类名 → 样式静默失效** | 中高（且难排查） | 类名一律字面量 + 查表（§2.2.2）；自定义 lint 规则进 CI；这是 daisyUI 路线上最现实的坑 |
| 复杂组件逻辑要自己写（表格排序分页、JSON 编辑器等） | 中 | §2.2.3 已清点到 8 项，且关键逻辑全做成**纯函数**（可单测）——顺带修掉旧实现的分页 `total` bug |
| Vapor 生态仍在成熟（DevTools、个别第三方库依赖 VDOM） | 中 | 需要时对单个组件改用 `defineComponent`（VDOM），写法与构建不变，**不放弃 Vapor** |
| `vue-jsx-vapor` 版本演进快（3.2.x） | 中 | 锁版本；macros/plugin 用法在 README 固定记录；升级前跑 L4 |
| Tailwind 4 与 daisyUI 5 的版本绑定（daisyUI 5 需 Tailwind 4） | 低 | 两者一并锁版本；`tailwind.config.js` 已废弃，勿按旧教程创建 |
| **上游接口按新抓包重写，抓包覆盖不全** | **高** | 四域分批抓、每批"抓包 → extract → 写接口 → L2"闭环（§7.1）；抓包前不写猜测性代码 |
| 业务逻辑重写出错（9,367 行 TS 的语义） | **高** | 以新抓包为基准的请求对拍 + 夹具回放 + 密码学对拍（§6.3） |
| **登录是全新功能、无参考实现** | **中高** | 严格按官方实际流程逆推；两条链路都先夹具回放跑通再上真机；状态机只有一份（§7.0） |
| **多用户引入越权风险** | **中高** | 所有查询带 `UserId`，并有"两用户交叉读取必须失败"的断言（§3.7） |
| HAR 含真实凭据 | 高 | 强制脱敏（header 白名单 + 键名黑名单 + 形状兜底 + 标识重映射）+ 写盘前 fail closed + 原始 HAR 不入库（§6.2） |
| 风控（并发过高） | 中 | 默认串行、并发上限、随机抖动、`RiskLevel::High` 二次确认 |
| 上游接口变更 | 中 | 夹具回放快速定位 + 错误分类 + 契约测试 |
| SQLite 引入打破"无 SQL"惯例 | 低 | 见下方决策点 |
| 重写期间两套并存、需求漂移 | 中 | 冻结功能范围（**除"登录/用户体系"这一项新增外，不新增云盘业务功能**），其余新需求进 Rust 版之后再开 |

---

## 10. 已决策事项（原 3 个待拍板项）

| # | 决策 | 结论 | 影响 |
| --- | --- | --- | --- |
| 1 | **存储** | **SQLite**（`rusqlite` bundled，`data/mcloud.db`） | 30 天趋势、运行历史筛选、事件流均可下推 SQL；**配置也由 SQLite 承载**（旧 `asign.json` 只作迁移输入，§3.7）；旧 `data/*.json` 一次性导入后写 `.bak`。**注意：这是对原项目"本地 JSON、无 SQL"惯例的有意变更，不是遗留问题。** |
| 2 | **项目名 / 目录** | **`mcloud`** → `/Users/catlair/mobile/workspace/mcloud` | crate 前缀随之为 `mcloud-*`（原名 `mcs-*` 已废） |
| 3 | **txiki.js** | **删除**，不做双运行时 | `packages/runtime`、`TJS=1` 构建路径、`tjs-tls-compat` patch 全部不搬；产物为各平台原生静态二进制 |
| 4 | **用户体系** | **多用户**：系统自有账号登录，每用户各自管理自己的云盘账号，数据按用户隔离（§3.7） | 旧项目**完全没有**这一层（D7）。新增 `users` / `sessions` 与 `accounts.owner`，schema 走 v3 |
| 5 | **交付形态** | **两个独立产物**：`mcloud-cli`（纯 CLI，不依赖 axum）与 `mcloud-server`（API + 内嵌 WebUI） | 为满足"cli 不依赖 server/axum"抽出组装层 `mcloud-app`（§3.2），`check-deps.sh` 与 README 分层表待同步 |
| 6 | **上游基准** | **以重新抓取的真实流量为准**，不从旧 TS 搬迁（§6.3） | 旧 TS 与线上请求已知有多处不一致；已按旧 TS 搬的 4 个模块**已全部回退**（§7.2） |
| 7 | **首个落地功能** | **移动云盘账号登录**（短信 + 扫码）→ `auth`（§7.0） | 旧项目零实现，需抓包逆推；插为 P2.5，先于所有上游业务接口 |

### 10.1 crate 清单（8 个：7 + 待实施的 `mcloud-app`）

| crate | 职责 | 允许 IO |
| --- | --- | --- |
| `mcloud-core` | 领域类型、纯函数、trait 定义（`HttpClient` / `StateStore` / `Clock`）、错误分类、密码哈希与用户/会话类型 | ❌ 禁止 |
| `mcloud-upstream` | 上游接口封装（按新抓包重写）+ 直播间 prost 编解码 | ❌ 禁止 |
| `mcloud-capabilities` | 19 个能力 + 注册表 + `FieldSpec` + 登录流程 | ❌ 禁止 |
| `mcloud-io` | reqwest 客户端（重试/幂等/熔断）、`FixtureHttpClient`、SQLite store（含用户/会话）、crypto、push、scheduler | ✅ 唯一允许 |
| `mcloud-exec` | runId、并发闸、事件总线 | ❌ |
| `mcloud-app` | ★ **组装根**：装配运行时、注入 IO 实现；不含 axum / clap | ❌（用 `mcloud-io`） |
| `mcloud-server` | axum 路由 + SSE + OpenAPI + 内嵌前端产物 | ❌（用 `mcloud-app`） |
| `mcloud-cli` | clap CLI（`login` / `run` / `serve` / `user` …）+ `fixture extract` | ❌（用 `mcloud-app`） |

> "允许 IO"= 该 crate 的依赖树里可以出现 `reqwest` / `rusqlite`；其余 crate 一律由 CI 断言禁止。
> `mcloud-app` 是本次新增（§3.2）：`mcloud-cli` 与 `mcloud-server` 都依赖它，前者不碰 axum。

---

## 11. 验收清单（DoD）

**门禁与形态**

- [ ] 五项门禁（§6.4）一条命令全绿：`cargo test` / `clippy -D warnings` / `fmt --check` / `check-deps.sh` / `check-identifiers.cjs`
- [ ] `cargo test -p mcloud-capabilities <能力>::<函数>` 可定向单跑
- [ ] **两个独立产物各自可跑**：`mcloud-cli`（纯 CLI，依赖树**不含** `axum` / `tower-http` / `rust-embed`）与 `mcloud-server`（单个原生二进制，含内嵌前端产物）；无 Node / 无 txiki 依赖
- [ ] `web/` 侧 `tsmc --noEmit` + Playwright 与后端门禁合成一条命令

**用户体系（新增，旧项目完全没有）**

- [ ] 用户能用自己的账号登录**本系统**；未初始化时 WebUI 只暴露初始化页，`mcloud user init` 与 WebUI 引导**走同一用例**
- [ ] 云盘账号**归属**系统用户；**数据隔离有断言**：两个用户各一个账号，交叉读取必须为空或报错
- [ ] 密码以 `argon2` 哈希落库；会话 token 只存哈希

**上游与对拍**

- [ ] 每个上游接口都有夹具回放用例，并**断言请求逐字段正确**（URL / body / headers）；未命中夹具即**失败**
- [ ] 夹具为**本机数据、不入库**；其干净性由 L2 用例的 `audit_leaks()` 兜底
- [ ] 密码学（MD5 / SHA256 / HMAC / AES-CBC / DES-ECB / RSA）与 TS 原版黄金向量一致（不可字节级对拍的按 §6.3 的单向验证）
- [ ] 登录两条链路（短信 / 扫码）**零联网夹具回放通过**，并各做过一次真实 smoke
- [ ] 抓包前未落任何猜测性接口代码（二维码字段、轮询间隔、短信前置校验均以实测为准）

**能力与前端**

- [ ] 每个账号的云朵余额 + 30 天趋势 + 日/周增量在 WebUI 可见（D1/D2 关闭）
- [ ] 新增能力只改注册表，前端零改动自动出现表单与结果卡片（D3 关闭）
- [ ] 运行历史重启不丢、SSE 实时可见（D4 关闭）
- [ ] 前端类型全部来自 `openapi.json` 生成物，CI 有 `--check`（D5 关闭）
- [ ] Vapor 全量（无 VDOM 依赖、无 interop）；daisyUI 类名全部字面量，窄屏（≤768px）全页面可用
- [ ] `web/src/ui/` 自有组件层的纯逻辑（排序/分页/筛选/时刻→cron/趋势路径/JSON 校验）有单测
- [ ] `mcloud-core` / `mcloud-capabilities` 依赖树不含 `reqwest` / `rusqlite`（CI 断言）
- [ ] `exchange` 仍在独立页面，不进入通用任务列表

**凭据边界**

- [ ] 凭据不出现在 API 响应、日志正文里；进版本库的一切（夹具、黄金向量、源码字面量）都经过脱敏，且标识守卫在 base64 解码后复查仍通过
