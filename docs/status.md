# 实现进度与现状须知

> 状态：**实况**（这里写的是"现在能跑什么、还差什么"，不是计划）
>
> ⚠ **2026-09-20：Rust 侧已冻结到 `legacy/`，本仓库正在迁往 Go。**
> 下面这张表描述的是**冻结那一刻**的 Rust 实现 —— 它仍然有效、也仍然是对拍基准，
> 但"进度"从此按 Go 侧算。迁移方案与顺序见 [`go-migration-plan.md`](./go-migration-plan.md)。
> Go 侧进度见下方「Go 侧迁移实况」一节。
>
> - 最后核对：2026-09-19 收工（Rust 侧）；Go 侧 2026-09-20 起
> - 规划（**写于动工前**，进度不要以它为准）：[`rust-rewrite-plan.md`](./rust-rewrite-plan.md)
> - 实现顺序（先做哪项能力）：同文档 §7.1
> - 门禁怎么跑：[`dev-gates.md`](./dev-gates.md)

---

## Go 侧迁移实况（2026-09-20 起）

**当前：步 1（`mcloud-core`）收尾中（只剩 `error` 后半的 `classify_body`）；步 4 已落地 `capabilities`（注册表 + `login`）、`exec`、`app::payload`，并为此**前置**补了步 2 的 `upstream::login`。**

> 步 4 的落点见下方「步 4 实况」一节 —— 它**没有**按 `capabilities → exec → app → server/cli`
> 一口气做完：`capabilities::login` 与 `app` 的绝大多数模块**依赖步 2（`mcloud-upstream`）与
> 步 3（`mcloud-io`）**，那两步还没迁，所以先落了不依赖它们的三块；
> 之后为了解开 `capabilities::login`，又**回头补了步 2 的 `upstream::login` 那一块**
> （只补到够用为止，步 2 的 `signin` 协议族仍未迁）。

| `mcloud-core` 的模块 | Go 侧 | 说明 |
| --- | --- | --- |
| `crypto` | ✅ | 哈希 / AES(CBC,ECB) / DES / RSA / gzip / 签名头 / getTn |
| `error` | ✅ 前半 | ErrorKind / Error / classify_status / classify_business_code；**后半的 `classify_body` 那台"响应体分类机"未迁** |
| `auth` | ✅ | 含全部原版怪癖（只去一处 `Basic ` 前缀、token 在 `:` 处截断、`Number() \|\| 0`） |
| `pure` | ✅ | 日键 / 增量 / 开抢时刻 / 脱敏 |
| `cron` | ✅ | 5/6 段分流、日与周 OR、周日 `0`/`7` 归一化、时区为参数、4 年上界返回 false |
| `sanitize` | ✅ | header 白名单 / JSON 递归脱敏 / token 形状打码 / 199 号段重映射 / `scan_for_leaks` 自检 / `build_fixture_entry` |
| `user` | ✅ | 口令哈希（argon2id PHC 串）/ 会话 token / 数据隔离判据；**带来了第七份黄金向量** |
| `run` | ✅ | RunID / 四态结论 / 需人工 / Seq 语义 / 五类事件流；**顺带补上了枚举的 JSON 编解码（见下）** |
| `model` | ✅ | Account / RunRecord / 能力注册表 7 个枚举 / AccountState / SmidState / Schedule / Pipeline / Exchange / Webui |
| `stage` | ✅ | 三阶段存储生命周期 / 四类迁移错误 / RunExecution；**`RunStage` 是 tag 枚举，形状由 serde 产出后对拍** |
| `legacy` | ✅ | 旧文档 14 个类型 / 规整规则 / 账号视图 / 引用解析；**带来了第九份黄金向量（形状）** |
| `traits` | ✅ | HTTP 请求/响应形状 / Clock / Scope / ClaimedRows / 三个 store 接口；**带来了第十份黄金向量（形状）** |

- 落点 `internal/core`，**267 条测试**；读 `test/golden/*.json` 经 `internal/golden`，
  读 `test/fixtures/legacy/*.json` 经 `internal/golden.LoadFixture`，都不复制。
- ⚠ **步 1 只剩 `error` 后半的 `classify_body` ~400 行**（那台"响应体分类机"）。
  它之所以一直排在最后：写出来之后**没有消费者**（用到它的层还没迁），
  而"写下来就没人再看"的代码正是这个项目反复吃亏的地方。
- 门禁**九条**（新增 `jsontags`）—— 见 `docs/dev-gates.md`。
- `auth-vectors.json` 从"只有 Rust 读"变成**两边都读**。
- 步 1 的退出判据是"黄金向量在 Go 侧逐字段复算通过"，**尚未达成**：
  `app-signin-vectors` 的消费者在步 2 接线，而 `login-crypto-vectors.json`
  至今仍是**死向量**（有生成器 `scripts/gen-login-crypto-vectors.cjs`、无消费者），
  且它的 `§guid32` 连拼接规则都没在向量里给出 —— 详见 `go-migration-plan.md` §6.1 末尾。
- ⚠⚠ **别照门禁那行"✓ 有测试读取"判定进度**：它的消费者探测扫**四个落点**
  （`crates/` / `legacy/crates/` / `internal/` / `cmd/`），命中任一即算有消费者，
  **不区分是哪一侧在读**。所以 `app-signin-vectors.json` 至今显示 ✓，
  而读它的**只有已冻结的 Rust 侧**。
  Go 侧实际读了 **8 份**：`crypto` / `pcclient-crypto` / `app-business-crypto` /
  `auth` / `password-hash` / `enum-json` / `legacy-shape` / `traits-shape`
  （见 `internal/core/*_test.go` 里的 `golden.Load`）。
- ⚠ `user` 是第一个引入**第三方依赖**（`golang.org/x/crypto`，只为 argon2）的模块。
  它只给**裸 KDF**，PHC 串的编解码要自己写；且 `v=16` 与 `$argon2d$` 两条
  Go 侧**物理上算不出来**，已在向量的 `$crossLanguageGaps` 里登记为
  "Rust 是 mismatch、Go 是 error"（详见 `go-migration-plan.md` §6.1）。
- ⚠ `cron` 与 `sanitize` **没有黄金向量** —— 它们的规格来自 Rust 侧的单元测试
  （`sanitize` 的 5 个测试文件共 348 行），迁移时是**逐条平移**的，不是照文档重写。
- ⚠⚠ **枚举的 JSON 形状是 `run` 这一步顺手补上的一个既有洞**，不是新需求：
  Rust 侧每个进 JSON 的枚举都带 `#[serde(rename_all = …)]`，序列化成**文本**；
  而 Go 的 `type X int` 默认序列化成**数字**。两边读同一批 `run_events.payload`
  与同一张 `role` 列 ⇒ 同一个字段有两个形状，且**不报错**。
  `user` 那一步的 `Role` / `SessionKind` 就已经是这个状态（它的测试只比 `String()`，
  所以没露出来）。现在收进 `internal/core/enum.go` 一张名字表，
  并由第八份黄金向量 `enum-json-vectors.json`（**serde 产出**）逐条对拍。
- ⚠ **`stage` 这一步新钉住一条**：tag 枚举（`#[serde(tag = "stage")]`）的
  **不同变体字段顺序不同** —— `Done` 是 `outcome→manual_steps→summary`，
  `Failed` 是 `summary→manual_steps`。单个结构体做不到，
  只能按 Kind 分派 `MarshalJSON`；而**逐字段比对会把这个差异整个放过**，
  只有整串比对抓得到。
- ⚠⚠ **`legacy` 是最容易"静默读空"的一层**：旧文档的所有字段都带
  `#[serde(default)]` ⇒ **字段名写错不报错，只静默读到零值** —— 表现是
  "历史数据读出来是空的"，没有任何线索指向字段名。
  而 Rust 侧对这批形状**一个断言都没有**。⇒ 新增第九份向量
  `legacy-shape-vectors.json`，且它的生成器 `scripts/gen-legacy-shape-vectors.rs`
  是 **path 依赖真实的 mcloud-core**（不是镜像类型定义）—— 镜像会抄错，
  `ScheduleTarget` 那次已经证明过一次。
  向量两节：`$serializedShapes`（8 个可序列化类型的空/填充整串）
  与 `$fixtureKeys`（夹具里实际出现的键名，用反射读 tag 逐条认领）。
- ⚠⚠ **`traits` 是"两侧都跑得通、但写出来的不是同一份东西"最典型的一层**：
  `HttpRequest` 的注释写着"golden request 对拍依赖它逐字段可比"，
  而 Rust 侧对它**只断言过一个枚举的一个变体**。第十份向量
  `traits-shape-vectors.json`（同样 path 依赖真实 mcloud-core）实测出四条
  Go 侧会**静默接受**的坏形状：`body` 的 base64 串、`headers: null`、
  缺席的 `method` / `url` / `status`、1 元组的 header。
  其中 `body` 那条最要命 —— `encoding/json` 对 `[]byte` 有**专门的** base64 分支，
  于是两侧各自序列化、各自解析都成功，只有两边文件逐字节 diff 才看得见。
  详见 `go-migration-plan.md` §6.1 #23–#27。
- ⚠ **测试夹具的 RNG 必须逐模块对齐 Rust 侧各自那一份，不能就近复用**：
  `run.rs` 与 `stage.rs` 各自定义了 `SeededRng`，算法不同
  （前者是 `s*31+7` 的 u8 LCG，**周期恰好 16 —— 正是 UUID 的字节数**）。
  `stage.rs` 还专门用一个进程内计数器把种子岔开，并在注释里写明了原因。
  迁移时我照样踩了（就近复用了 `run_test.go` 那份），详见 §6.1 #19。
- ⚠⚠ **同一个病还有第二个面：结构体字段名**。Rust 的 serde 默认 snake_case，
  Go 不带 tag 输出字段名原样 ⇒ `password_hash` vs `PasswordHash`。
  这个版本已修三处：`CloudStats`（pure 带过来的）、`User` + `Session`（user 带过来的）、
  `model` 那批。⚠ **往返测试救不了它**（用什么字段名都是绿的），
  所以新增了第九条门禁 `scripts/check-go-jsontags.cjs`：
  混合 tag 直接失败、全无 tag 必须进白名单并写明理由。

### 步 4 实况（2026-09-20 起）

| 模块 | Go 侧 | 说明 |
| --- | --- | --- |
| `capabilities` 注册表 | ✅ | `Capability` / `REGISTRY` / `get` / `list_visible` / `list_batchable`（**10 条测试**）。对应 `capabilities/src/lib.rs` |
| `capabilities::login` | ✅ | 扫码状态机（推进 / 终态不可逆 / 自身 TTL / 码的映射 / 限流非终态）+ 短信链路（滑块降级 / 配额 / 登录）+ 登录结果 → `auth` 与领域对象（**22 条测试**）。前置的 `upstream::login` 已先落地 |
| `exec` | ✅ | 并发闸 / 互斥键 / 参数摘要 / 准入（幂等 + 互斥一起判）/ 事件总线（**35 条测试**）。对应 `mcloud-exec/src/lib.rs` |
| `app::payload` | ✅ | `/api/capabilities` 与 `/api/version` 的响应体（**7 条测试**）。纯：只依赖 `capabilities` + `core` |
| `app` 其余 | ⬜ | `login` / `run` / `smid` / `session` / `migration` / `signin/*` 等**依赖 `upstream` 其余部分与 `io`**，步 2 收尾 / 步 3 落地前迁不了 |
| `upstream::login` | ✅ | 为使 `capabilities::login` 能落地而**提前做的前置**：信封 / 端档案 / 请求体 / 滑块 / 口令哈希 / 判定（**33 条测试**）。步 2 的其余部分（`signin` 协议族）仍未迁 |
| `cli` / `server` | ⬜ | 在 `app` 之后 |

- 落点 `internal/capabilities`、`internal/exec`（包名 `mcloudexec`）、`internal/app/payload.go`。
- ⚠ **`app` 现在仍只是"组装根接上了 IO"那个骨架**（`internal/app/app.go` 只有一个 `Deps`）。
  payload 是第一个真正有内容的模块。
- ⚠ `core.MarshalJSON` / `core.ParseJSON` 这次**导出**了：产物层要拼对外 payload，
  而"不转义 HTML + 不补尾换行"这两条规则**必须只有一份实现**（详见方案 §6.1 #22）。
- ⚠ `core.Version` 是新增常量 —— Go 没有 `env!("CARGO_PKG_VERSION")`，
  它得与 `legacy/Cargo.toml` 的 `version` **手工同步**（不同步的表现只是两个产物报的版本号不一样）。

---

## 一句话总览

| 层            | 状态                                                   |
| ------------ | ---------------------------------------------------- |
| CLI：初始化 / 登录 | ✅ 可用（扫码 + 短信，PC 端与 App 端）                            |
| CLI：签到（云朵中心） | ✅ 端到端跑通（含领取、复活卡）                                     |
| 上游接口层        | ⚠ **逐块重写中** —— 只有 `login` 与 `app_signin` 是完整的，其余仍是骨架 |
| 服务入口         | ⚠ 能起，但**只有"会话"这一层**，跑任务的那批路由仍在 P5                    |
| WebUI        | ⚠ 登录与初始化是真接口，**主界面仍是 P0 spike 的占位数据**                |
| 运行时层         | ⚠ 事件模型 / 生命周期 / 事件总线 / `run_events` 表已落地，编排与调度接入中    |
| 夹具回放（L2）     | ❌ **尚无消费者** —— `test/fixtures/http/` 里的夹具一个断言都没跑     |

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

**门禁实况（2026-09-19）**：**586 个测试通过**，七条门禁全绿  
（黄金向量那段是 **122 条断言**，见 [`dev-gates.md`](./dev-gates.md)）。

---

## 逐项须知

### 服务入口：能起，但只有"会话"这一层

`cargo run -p mcloud-server` 起 API + 前端产物，路由目前只有  
`/api/health`、`/api/setup`、`/api/session`（白名单免登录，其余一律要会话）。

跑任务的那一批（accounts / capabilities / runs / schedules）、SSE 与 `openapi.json`  
导出仍在 P5 —— 所以 **WebUI 登录与初始化是真接口，主界面仍是 P0 spike 的占位数据**。

> 更早（2026-09-17）时 `mcloud-server` 只有 lib、没有 `serve` 命令，CLI 能跑而 WebUI  
> 没有后端可连，前端只能靠 `vite dev` 单独起。这一条已经过去了。

### CLI：已能完成"从零到登录"

`mcloud user init` → `mcloud login qr`（终端二维码）/ `mcloud login sms`。

短信登录有两个端：`--client pc`（默认）与 `--client app`（iOS 客户端形态）。  
两端**不是同一件事的两种参数** —— 请求体加解密方式、`Authorization` 前缀、  
设备指纹形状都不同（差异见 [`client-profiles.md`](./client-profiles.md)）。

登录态落在本地库的 `account_state`（凭据与展示信息**分表**）；  
设备指纹写在 `data/device.txt`（**换掉它等于换一台设备**，服务端会定位不到会话）。

### 签到（云朵中心）已端到端跑通

`mcloud signin --account <掩码id>`。这是第一个按新抓包写完整的**业务**接口块 ——  
`mcloud-upstream::app_signin`（请求构造 + 响应解析）+ `mcloud-app::signin`（编排）。

协议、六个要点与坑见 [`cli-usage.md`](./cli-usage.md) §签到。

### 上游接口层按新抓包逐块重写

目前 `login`（扫码 / 短信 / 账密 + 退出登录）与 `app_signin`（签到）是完整的，  
它们是"按新流量写接口"的头两块；其余云盘业务接口仍是骨架（`Upstream` + `Envelope` + `base`）。

旧 TS 的 12 个接口模块**一律不从旧代码搬迁** —— 旧 HAR 与旧 TS 都会作废，  
搬了只是把旧形态再抄一遍。新 HAR 抓好后：`mcloud fixture extract` → 按新流量写接口  
→ 用夹具离线回放并断言请求逐字段正确。

### ⚠ 夹具回放（L2）尚无消费者

此前本文件写着"`mcloud-upstream` 的 L2 夹具回放已跑通  
（`legacy/crates/mcloud-upstream/tests/l2_replay.rs`）"—— **那个文件不存在**，  
`legacy/crates/mcloud-upstream/tests/` 是个**空目录**。

现状是：`mcloud fixture extract` 往 `test/fixtures/http/` 写，但**没有任何消费者**，  
所以那 31 个夹具目前一个断言都没跑。这是规划 §6.3 里要补的一环，  
**别按"已经有了"来理解**。

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
> `packages/core/src/utils/encrypt.ts:3` 的 `REFRESH_TOKEN_KEY`，*而它导出的解码函数  &#xA;****全仓无人调用****&#x20;—— 见 `docs/upst`*`ream-app-business-api.md` §8.0。  
> ⚠ 它与发码链路的 `uDjGSA8auFlbwDkY`（AES-128）**不是同一把**，别互搬。
>
> ✅ **同日再更正：这类信封其实有不止一把钥匙，按「谁发的」分** ——  
> 上面那把 AES-192 是 **App 原生**（`FXHttpBaseAccessor`）用的；  
> **H5 / WebView**（`coolFlagPivotal`）用另一把 **AES-256**  
> `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1`，格式完全同形。  
> 判据：`x-yun-uni` + `x-yun-api-version` ⇒ 原生；`origin: mcloudlocal://` + `sec-fetch-*` ⇒ WebView。  
> ⚠ **钥匙是「谁发的」的函数，不是版本号** —— 别按"最新版就该用最新那把"去猜。  
> 见 `docs/upstream-app-business-api.md` §8.1。



---

## 按任务指路

| 你要做的事       | 读哪篇                                                                |
| ----------- | ------------------------------------------------------------------ |
| 跑门禁 / 看分层约束 | [`dev-gates.md`](./dev-gates.md)                                   |
| 敲命令行 / 查参数  | [`cli-usage.md`](./cli-usage.md)                                   |
| 加文件 / 起新目录  | [`repo-layout.md`](./repo-layout.md)                               |
| 实现某个业务能力    | [`business-feature-inventory.md`](./business-feature-inventory.md) |
| 全部文档的索引     | [`README.md`](./README.md)                                         |
