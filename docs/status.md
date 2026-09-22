# 实现进度与现状须知

> 状态：**实况**（这里写的是"现在能跑什么、还差什么"，不是计划）
>
> ⚠ **2026-09-20：Rust 侧已冻结到 `legacy/`；2026-09-21 该目录已整体剥离为同级仓库
> [`../mcloud-rust/`](../../mcloud-rust)。**  
> 下面那张表描述的是**冻结那一刻**的 Rust 实现 —— 它仍然有效、也仍然是对拍基准，  
> 但代码与产物都不再位于本仓库；"进度"从此按 Go 侧算。迁移方案与顺序见
> [`go-migration-plan.md`](./go-migration-plan.md)。  
> Go 侧进度见下方「Go 侧迁移实况」一节。  
> ⚠ 剥离带来一条**新约束**：`test/golden/` 与 `test/fixtures/legacy/` 从此是
> 两个仓库各一份**实体副本**（原来 `legacy/test` 是指向本仓库的符号链接），
> 而**没有任何门禁盯着两边是否一致** —— 改一份向量的 `expect*` 必须两边都落。
> 动了哪些文件、为什么：[`../mcloud-rust/docs/freeze-record.md`](../../mcloud-rust/docs/freeze-record.md) 的「剥离补记」。
>
> ✅ **剥离后两侧各跑过一次全量门禁（2026-09-21，剥离当日）**：Go 侧 `bash scripts/gates-go.sh` →
> **九项门禁全绿**（927 个测试函数、`-race` 全量）；Rust 侧在同级仓库 `bash scripts/gates.sh` →
> **七项门禁全绿**、**714 个测试通过**（与冻结记录里那个数字一致，说明搬动没碰行为）。
> ⚠ Rust 侧**第一次**跑报了 30 条 `读夹具 … No such file` —— 那是搬过来的 `target/` 里
> 烘着旧绝对路径的产物（`env!` 是编译期常量），`cargo clean -p` 重编后消失。
> 判据见 [`../mcloud-rust/docs/freeze-record.md`](../../mcloud-rust/docs/freeze-record.md) 的「剥离补记」。
>
> - 最后核对：2026-09-19 收工（Rust 侧）；Go 侧 2026-09-20 起
> - **2026-09-22**：这一轮只动文档（重排 + 去重 + 把 `mcloud-<crate>::…` 那套 Rust 旧写法逐条核到 Go 落点），
>   **本文的进度结论没有变化**。写作约定（Go 落点怎么标、归档怎么读）在
>   [`README.md`](./README.md) 末尾两节；逐项 Go 现状的核对记录在
>   [`business-feature-inventory.md`](./business-feature-inventory.md) 抬头那条 09-22 块里。
> - 规划（**写于动工前**，进度不要以它为准）：[`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md)
> - 实现顺序（先做哪项能力）：同文档 §7.1
> - 门禁怎么跑：[`dev-gates.md`](./dev-gates.md)

---



> 📦 **过程性实况已移出**：每一轮迁移当时记下的 692 行流水在
> [`archive/migration-log.md`](./archive/migration-log.md)（2026-09-22 逐字搬移，未改写）。
> 本文从此只放**结论**：现在能跑什么、还差什么、按任务先读哪篇。


## 一句话总览

| 层            | 状态                                                                                                                                           |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| CLI：初始化 / 登录 | ✅ 可用（扫码 + 短信，PC 端与 App 端）                                                                                                                    |
| CLI：签到（云朵中心） | ✅ 端到端跑通（含领取、复活卡）                                                                                                                             |
| 上游接口层        | ⚠ **逐块重写中** —— 只有 `login` 与 `app_signin` 是完整的，其余仍是骨架                                                                                         |
| 服务入口         | ✅ **Go 侧已接线**：API 四组业务路由（含 SSE）+ 前端产物与 SPA 回退；**15 条路由逐条对位冻结基准**（有断言盯着）。⚠ `POST /api/runs` **基准里也没有**（P4 功能，非迁移缺口） |
| WebUI        | ⚠ 登录与初始化是真接口，**主界面仍是 P0 spike 的占位数据**                                                                                                        |
| 运行时层         | ⚠ 事件模型 / 生命周期 / 事件总线 / `run_events` 表已落地，编排与调度接入中                                                                                            |
| 夹具回放（L2）     | 🟡 **回放器已就绪**（`internal/io/fixture.go`，17 条测试）；已有消费者 `TestEveryArchivedFixtureIsLeakFree`（31 份抓包夹具全量自检），⭕ 但上游协议层**还没真的加载这批夹具跑一遍**（见下方「夹具回放」） |

---

## 按任务指路

| 你要做的事       | 读哪篇                                                                |
| ----------- | ------------------------------------------------------------------ |
| 跑门禁 / 看分层约束 | [`dev-gates.md`](./dev-gates.md)                                   |
| 敲命令行 / 查参数  | [`cli-usage.md`](./cli-usage.md)（编排层的规格在 [`signin-cli-spec.md`](./signin-cli-spec.md)） |
| 加文件 / 起新目录  | [`repo-layout.md`](./repo-layout.md)                               |
| 实现某个业务能力    | [`business-feature-inventory.md`](./business-feature-inventory.md) |
| 写某个上游接口（字段、状态机、错误码） | [`upstream/`](./upstream/) 里对应那个端的那一篇（五篇按端分） |
| 写前端页面 / 组件  | [`webui-design-guide.md`](./webui-design-guide.md) → [`webui-components.md`](./webui-components.md) → [`webui-code-guide.md`](./webui-code-guide.md) |
| 动迁移剩下的步骤   | [`go-migration-plan.md`](./go-migration-plan.md)；**先翻一遍** [`migration-pitfalls.md`](./migration-pitfalls.md)（那张编号表是活的，别重新踩） |
| 看当初为什么这么判 / 历史流水 | [`archive/`](./archive/) —— 归档不删，但**别当现状读** |
| 全部文档的索引与"谁说了算" | [`README.md`](./README.md)                                         |


## 逐项须知

### 服务入口：Go 侧已接线，前端产物与 API 都在

> ⚠ 这一节早先写的是 Rust 侧（冻结那一刻）的样子：`cargo run -p mcloud-server`
> 起 API + 前端产物，但路由只有 `/api/health` / `/api/setup` / `/api/session`。
> 那是**旧**状态 —— 下面按 **Go 侧**重写。

`go run ./cmd/mcloud-server`（默认 `127.0.0.1:8787`）现在能起来，并同时提供：

- **API**：健康 / 初始化 / 会话，加上 accounts、capabilities、runs（含 SSE 事件流）、
  schedules 四组业务路由 —— 白名单免登录（只有 `/api/health`、`/api/setup`
  与 `POST /api/session`），其余一律要会话；
- **前端产物**：`web/dist` 的静态文件 + SPA 回退（非 `/api` 的未知路径交给
  `index.html`，前端路由自己处理）。目录不存在时**只挂 API**，并在启动横幅里说明 ——
  开发期先 `cd web && npm run dev` 是常态。

命令行五个选项与 Rust 侧逐项对齐（`--db` / `--host` / `--port` / `--web-dir` /
`--secure-cookie`），退出码同一套口径：0 正常、1 运行错误、2 用法错误。

⭕ **迁移侧没有缺口**：15 条路由逐条对位冻结基准（`internal/server/routes_test.go` 盯着），
`POST /api/runs` **基准里也没有**（`routes/mod.rs` 写明"要等 P4"）。
⚠ 所以"WebUI 主界面仍是占位数据"的**原因不是缺接口**，而是**跑任务那条路本身还没建**
（P4/P5 的功能开发）—— 别把它记成迁移没做完。`openapi.json` 导出同理，是新功能。

> 2026-09-21 追加：Go 侧在冻结基准之外**自扩 1 条** —— `GET /api/status`
> （`internal/server/routes.go` 的 `getStatus`：产品名+版本、`schedulerRunning`、最近一次 run）。
> 它**不进** `routes_test.go` 那张 15 条对位表 —— 那张表只回答"基准要求有的都在吗"，
> 把自己扩的混进去会让"移植漏了"与"我们改坏了"两种失效分不开；
> 它自己的断言在 `internal/server/status_test.go`（含"未登录必须 401"、"版本必须与
> `/api/version` 同源"、"没 run 时是 null 而不是 `{}`"）。
> 动机是 WebUI 侧栏底部那一行：原先那里把 `name` 与 `role` 并排显示，
> 而初始用户这两个字段字面都是 `admin`（详见 [`webui-design-guide.md`](./webui-design-guide.md) §4.5）。
> ⚠ `schedulerRunning` 当前**恒 false**，与上面运行时层那条"调度接入中"是同一件事 ——
> 接口宁可显式报 false 也不省略这一项，省略的表现是界面一片绿而绿的意思是"在跑"。

> 更早（2026-09-17）时 `mcloud-server` 只有 lib、没有 `serve` 命令，CLI 能跑而 WebUI  
> 没有后端可连，前端只能靠 `vite dev` 单独起。这一条已经过去了。

> 2026-09-21 追加（**静态那一层加了压缩与缓存头**，同样是基准之外的自扩）：
> 迁移过来的静态是裸 `http.FileServer`，产物**原样**发出去 —— 实测
> `web/dist/assets/index-*.css` 走线 **80,213 字节**，而它 gzip 后 **12,771 字节**
> （-84%）。差的**不是 tree shaking**：产物里没有未用到的组件（逐类名核过，
> 181 个类名都在被引用），是**没人压**。
> ⚠ **别把这一条记成"迁移漏了"**：同一份 CSS 打在冻结基准的 `mcloud-server`（8787）上，
> 实测 `content-length: 80213` 且**没有** `Content-Encoding`/`Cache-Control` ——
> 基准自己也没压。这一层是按"用户实际下多少字节"改的，不是对位。落点在 `internal/server/static.go`，
> 只用标准库 `compress/gzip`（骨架期"零第三方依赖"那条没破）。
> 三件一起做的：**gzip**（按扩展名判，png/woff2 这类内部已压的不碰）、
> **`Cache-Control`**（`/assets/*` 是 Vite 内容哈希名 ⇒ `immutable` 一年；
> 固定名的首页/回退 ⇒ `no-cache`，可存但必须带 `Last-Modified` 回源）、
> **`Vary: Accept-Encoding`**（无条件给：不压也是一次"按头而定"的应答）。
> ⚠ 两处**顺序/分支**上的坑各由一条测试钉住：
> ① 压缩只包**静态分支**，不能包整个 handler —— `GET /api/runs/{id}/events` 是 SSE，
> gzip 的块缓冲会把事件攒着，现象是"进度要等很久才一次性全出来"
> （`TestAPIResponsesCarryNoCompressionHeaders`）；
> ② 缓存与压缩的判据要按**真正送出去的文件**，不是请求路径 ——
> `/assets/` 是个目录、走 SPA 回退送出的是首页，按路径判会给首页打上一年的 `immutable`
> （`TestTheAssetsDirectoryFallbackIsNotTreatedAsAnAsset`）。
> 另外 304 那条：压缩流只在 `WriteHeader(200)` 时建，否则 `Close` 会吐出一段
> 20 字节的**空 gzip 流**，现象是"304 带了一个 body"（`TestAConditionalHitStaysABodyless304`）。
> ⚠ `http.ServeContent` **不造 ETag**（只尊重调用方预设的），所以回源校验走 `Last-Modified`；
> 将来若自己加 ETag，压缩变体必须弱化成 `W/`。判据：`internal/server/static_test.go` 新增 10 条。

### CLI：已能完成"从零到登录"

`mcloud user init` → `mcloud login qr`（终端二维码）/ `mcloud login sms`。

短信登录有两个端：`--client pc`（默认）与 `--client app`（iOS 客户端形态）。  
两端**不是同一件事的两种参数** —— 请求体加解密方式、`Authorization` 前缀、  
设备指纹形状都不同（差异见 [`client-profiles.md`](./client-profiles.md)）。

登录态落在本地库的 `account_state`（凭据与展示信息**分表**）；  
设备指纹写在 `data/device.txt`（**换掉它等于换一台设备**，服务端会定位不到会话）。

### 签到（云朵中心）已端到端跑通

`mcloud signin --account <掩码id>`。这是第一个按新抓包写完整的**业务**接口块 ——  
`internal/upstream/app_signin*.go`（请求构造 + 响应解析）+ `internal/app/signin.go`（编排）。

协议、六个要点与坑见 [`cli-usage.md`](./cli-usage.md) §签到。

### 上游接口层按新抓包逐块重写

目前 `login`（扫码 / 短信 / 账密 + 退出登录）与 `app_signin`（签到）是完整的，  
它们是"按新流量写接口"的头两块；其余云盘业务接口仍是骨架（`Upstream` + `Envelope` + `base`）。

旧 TS 的 12 个接口模块**一律不从旧代码搬迁** —— 旧 HAR 与旧 TS 都会作废，  
搬了只是把旧形态再抄一遍。新 HAR 抓好后：`mcloud fixture extract` → 按新流量写接口  
→ 用夹具离线回放并断言请求逐字段正确。

### ⚠ 夹具回放（L2）仍只有一个消费者

此前本文件写着"`mcloud-upstream` 的 L2 夹具回放已跑通  
（`../mcloud-rust/crates/mcloud-upstream/tests/l2_replay.rs`）"—— **那个文件不存在**，  
`../mcloud-rust/crates/mcloud-upstream/tests/` 是个**空目录**。

现状是：`mcloud fixture extract` 往 `test/fixtures/http/` 写，消费者目前**只有一条**  
—— `io` 包的 `TestEveryArchivedFixtureIsLeakFree`（把 31 份夹具全量加载跑脱敏自检）。  
上游协议层**还没有**真的加载这批夹具把响应解析一遍，所以"回放器已跑通"这句话  
仍然**不能**按"L2 回放已接上"来理解。这是规划 §6.3 里要补的一环。

已确认可用的样本（写回放时直接照着接）：

| 接口                                      | 在哪个夹具                  |
| --------------------------------------- | ---------------------- |
| `startSignIn` / `infoV3` / `doTaskPost` | `3d.json`              |
| `taskListV3`                            | `每月阅读任务.json`          |
| `receiveRevivalReward`                  | `复活.json`              |
| `receiveV3`                             | **没有任何夹具**（只能靠单测 + 真机） |

### 旧 TS 与线上实际请求本就不一致，重写时要逐条核对

上一轮对拍换来的经验，**别默认"照 TS 抄"就是对的**：

| 接口                            | TS 写法                                | 真实流量                                      |
| ----------------------------- | ------------------------------------ | ----------------------------------------- |
| `visitlog/journaling`         | 参数放在 **query**                       | 参数放在**表单体**                               |
| `signin/task/taskListV2`      | `clientVersion=11.2.1`、`group=month` | `clientVersion=11.4.0`、`group=new`        |
| `auth-service/auth/tyrzLogin` | body 带 `type`/`channel`              | body 无这两个字段，`marketName=newsign_139mail`  |
| `market/signin/page/infoV3`   | 不带端口的 `/market/...`                  | `caiyun.feixin.10086.cn:7071/ycloud/...`  |
| `noteRefreshToken`            | `note.mcloud.139.com/noteServer/...` | `note-njs.yun.139.com/yun-note/...`（另一个域） |

另有：`refreshToken` 的真实流量是**加密载荷**（原实现注释却说是明文 JSON 接口）；  
`auth/refreshToken`、`querySpecTokenV2` 这类接口的 `code` 是**字符串**不是数字。

> ✅ 上面"加密载荷"这条 **2026-09-20 已定案**（此前只有结论、没有钥匙）：  
> 带 `hcy-cool-flag` 的请求与响应都是  
> `base64(IV(16 字节二进制) ‖ AES-192-CBC/PKCS7("2olBaQGYnEKoStYomsd1n7ax", IV, 明文))`，  
> 双向；**396 条实拍零例外**。钥匙就藏在旧项目  
> `packages/core/src/utils/encrypt.ts:3` 的 `REFRESH_TOKEN_KEY`，*而它导出的解码函数    
> **全仓无人调用** —— 见 [`upstream/app-business-api.md`](./upstream/app-business-api.md) §8.0。
> ⚠ 它与发码链路的 `uDjGSA8auFlbwDkY`（AES-128）**不是同一把**，别互搬。
>
> ✅ **同日再更正：这类信封其实有不止一把钥匙，按「谁发的」分** ——  
> 上面那把 AES-192 是 **App 原生**（`FXHttpBaseAccessor`）用的；  
> **H5 / WebView**（`coolFlagPivotal`）用另一把 **AES-256**  
> `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1`，格式完全同形。  
> 判据：`x-yun-uni` + `x-yun-api-version` ⇒ 原生；`origin: mcloudlocal://` + `sec-fetch-*` ⇒ WebView。  
> ⚠ **钥匙是「谁发的」的函数，不是版本号** —— 别按"最新版就该用最新那把"去猜。  
> 见 `docs/upstream/app-business-api.md` §8.1。

---

## 历史流水账（已折叠，只留结论）

测试数一路涨上来的过程本身不构成"当前状态"，故压成下表。当前数见下方「门禁实况」。

| 时间         | 测试数     | 当轮新增了什么                                                             |
| ---------- | ------- | ------------------------------------------------------------------- |
| 2026-09-17 | 241     | 当时的一帧                                                               |
| ——         | 493     | 接完 smid（设备指纹绑账号）那条线，自贡献 46 条（上游算法 25 + 生命周期 13 + 存储 8）              |
| ——         | 516     | App 签到线：上游接口层 + 编排层 + 存储层（`account_state.phone`，schema v6）          |
| ——         | 523     | 云朵余额采集（`mcloud-app::cloud` 5 条 + `set_account_cloud` 窄写 2 条）        |
| ——         | 533     | `receiveV3` 领取（请求构造 / 解析 / 错误码 10 条）                                |
| ——         | 541     | `revival` 复活卡（8 条，含一条读黄金向量的）                                        |
| ——         | 542     | 查明 H5 `deviceId` 根因并接线（新增一条把签到页 `_smConf` 与 Rust 常量钉在一起的守卫）         |
| ——         | 546     | `Authorization` 改为整串存整串用（schema v7），新增 4 条                          |
| ——         | 554     | CLI 账号选择（下标语义）补 9 条用例                                               |
| 2026-09-19 | **586** | 把云端那 9 个提交拉进本地（`mcloud-app` 组装层、App 签到全链路、schema v7 等），并修掉两处 clippy |

> 这里曾经把 554 写成"当前数"—— 那是**拉取云端之前**本机的数。  
> 云端那 9 个提交带进来的 `mcloud-app` 等新模块另有自己的 lib 测试，落地后实跑是 **586**。

**门禁实况（2026-09-21，Go 侧）**：**922 个测试通过**，**九条门禁全绿**  
（黄金向量那段是 **167 条断言**，见 [`dev-gates.md`](./dev-gates.md)）。  
⚠ 上面「当前数见下方门禁实况」指的就是这一段 —— 它早先写的是 **Rust 侧**
（2026-09-19：586 个 / 七条），那是**冻结前**的数，别拿它当现状。
Go 侧逐包：core 270 / upstream 157 / io 150 / app 118 / cli 116 / server 44 /
exec 35 / capabilities 32。

---
