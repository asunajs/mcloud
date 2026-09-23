# 刷新 auth（`auth-refresh`）：判据、证据分档与四个触发面口径

> 状态：**规范 + 证据**。2026-09-22 这一条链路（存储层 → 上游 → 能力层 → 驱动 → CLI → HTTP → WebUI）落地；
> 2026-09-23 又接上第四个触发面：**服务进程内的调度循环**（§5.4）。
> 本篇只收"跨层才看得见"的事实；单层的"为什么这么裁"写在代码注释里，指路见 §7。
> 与 [`cli-usage.md`](./cli-usage.md) 的参数表冲突时以那篇为准（参数面是冻结基准）。

## 0. 一句话

库里那串 `Authorization` 里的 token 自己带过期时刻，**快到期时向上游换一个新的**，
然后**覆盖**库里那一套（窄写，其余列不动）。两端（PC 客户端 / 手机 App）走**同一个接口**。

## 1. 证据分档 ⭐ 读之前先看这张表

| 面 | 证据 | 缺证据时的后果 |
| --- | --- | --- |
| **App 支**的请求/响应形状 | **实拍闭环**：一次窗口里 24 次调用、两条完整往返（[`capture/app-rights-token-samples.txt`](./capture/app-rights-token-samples.txt) §4） | —— |
| "新 token 就是 `Authorization` 第三段" | **实拍**：刷新响应里的 `token` 编成 `Basic base64("mobile:<手机号>:<token>")` 后，在同一次会话的后续请求头里逐字命中（次日窗口同样命中） | 若这条不成立，落库就是写进一个用不了的串 —— 所以它必须有直接证据 |
| **PC 支**走不走这个接口 | **零抓包**。PC 桌面客户端的 API 源码里根本没有 refresh 这条（`har/pc-api.js` / `pc-utils.js` 全文无 `refreshToken`）；调这个接口的是 **H5**（[`capture/apiServer.js`](./capture/apiServer.js) 的 `auth.refreshToken(data)`） | PC 那一支是"按 H5 的形状 + PC 的档案头"**拼**出来的，是最可能的形状，不是实测过的形状 |
| 请求体是否要 `hcy-cool-flag` 信封 | 实拍那次**是**加密的（该接口在 192 那把钥匙的命中列表里），但本仓库对"已登录的业务接口"的既有策略是走明文（`querySpecTokenV2` 实测明文过）；参考实现同主机的 `refreshToken` 注明"不带 `hcy-cool-flag`、明文 JSON"（`apis/auth.ts`）⇒ **两档互不一致，仍留 ❓** | 上游若回 `9103` 一类"解不开请求体"，正确反应是**补 AES-192 信封**（`core.RefreshTokenKey`），不是改判据 |
| 旧实现那条 `tellin/authTokenRefresh.do` | **不实现**。旧 TS 刷的是它（XML 体、content-type 却写 json），本仓库对它零抓包，App 流量里一次都没出现 | 两条都未实测时选有实测的那条；一条已实测时**只**走它 |

### 参考实现（原项目 `mcloud-sign`）里刷新是常规动作，不是"要先批准"的动作

- **每一轮任务收尾都自动刷**：`packages/core/src/tasks/after.ts` 的 `refreshAuthToken` 挂在 `afterTask` 里，
  月初 113 号任务前还调第二次（`tasks/app/runners.ts`）；`switchAccount` / `restoreAccount`
  （`tasks/auth-service.ts`）每次切号都向上游 `refreshToken` 派生新 token。
  ⇒ 在这份实现里"刷新"就是**从长期持有的 auth 串反复派生新 token**，一天发生很多次，
  没有任何闸门、也不需要谁点头。
- **判据同形**：`isNeedRefresh(expire, days)` = `expire - Date.now() < days * 86400000`（`auth/utils.ts:45`），
  代码兜底 7 天（`after.ts:9`），配置项 `剩余多少天刷新token` 默认 10（`shared/generated/defaults.ts:16`）
  ⇒ 与本篇 §2 的"提前量 + 未到点零请求"是一件事。
- **成功之后同样写回持久化**：更新 `ctx.config.auth/token/expire`、`http.setAuth`，
  再 `updateAccountAuth(configPath, accountIndex, newAuth)` 落配置文件（`shared/src/config/loader.ts:206`）
  ⇒ 与我们的 `RotateAccountAuth` 覆盖库里四列（§4）同形，不是本仓库多出来的危险动作。
- ⛔ **旧 token 换下来之后会怎样 —— 两边都不知道**：参考实现里没有"会顶掉端上会话"这类话，
  本仓库也零实测 ⇒ 只留 §8 那个 ❓ 问句，不许写成断言（两个方向都不行，AGENTS.md §1.7），
  更不许拿它当"先别做"的理由。
  > 耻辱柱（2026-09-23）：本篇曾把"真打之前必须先获用户批准"写成开场结论、并标成用户立的规矩，
  > 又拿那个未实测的后果当闸门拖住开发。实况是：用户关于闸门只说过一句「不需要这个功能，
  > 有二次确认就不需要这些」（§2），而参考实现把刷新放进每轮收尾自动做。
  > 见坑表 #125⑤⑥ 与 `AGENTS.md` §1.11。
- **闸门只有一处，就是弹窗本身**：手动那一路的二次确认（§5.2/§5.3）就是全部；
  调度那一路到点直接跑（§5.4）。可证的档位事实只有一条 —— 这条能力是 `Medium`，
  而 `RiskLevel.NeedsConfirmation()` 只对 `High` 为真（`internal/core/model.go`）
  ⇒ 到点触发那一刻没有框可弹。⚠ "没有框"与"谁同意过"是两件事，不许把前者读成后者。

## 2. 判据：三档，且默认提前 7 天

纯函数在 `capabilities`（零 IO），单位陷阱全在这一段：

| 档 | 触发条件 | 打过上游吗 | 对外码（`decision`） |
| --- | --- | --- | --- |
| `RefreshNeeded` | 剩余时间 ≤ 提前量（默认 `RefreshDefaultLeadDays = 7.0` 天）；**`expire == 0` 也算这一档** | 是 | `needed` |
| `RefreshNotYet` | 剩余时间还多 | **否**（一次都不发） | `not-yet` |
| `RefreshManualOnly` | **只有 `expire < 0`** 才判这一档：库里那串根本没有有效过期时刻 | **否** | `manual-only` |

- `manual-only` 是**人工出口**，不是失败：只能重新登录、或把新的 auth 整串贴进 WebUI。
  这条判据来自用户 2026-09-19 的订正（见 [`business-feature-inventory.md`](./business-feature-inventory.md) §2.2 那段补记）：
  旧实现有半边把它写成"恒需刷新"，那会**白白重打**上游。
- **与参考实现逐条对位**（`mcloud-sign`）：判据同一条式子 —— `isNeedRefresh` = `expire - Date.now() < days * 86400000`
  （`auth/utils.ts:45`）⇒ 我们的"剩余 ≤ 提前量"。天数它有两处：代码兜底 7（`tasks/after.ts:9`）、
  给用户配置的默认 10（`剩余多少天刷新token`，`shared/generated/defaults.ts:16`）⇒ 本仓取 7 且把它做成
  CLI 的 `--lead-days`（WebUI 那一支不收这个参数，§5.2），两侧都够调。
  ⚠ "没有有效过期时刻"它两处不一致：`after.ts` 是 `!expire ⇒ 跳过`（对位我们的 `manual-only`），
  `checkAndRefreshAuth` 直接把 0 喂给 `isNeedRefresh` ⇒ 判"该刷"（对位我们的 `expire == 0 ⇒ needed`）。
  我们两处各取一处，不是笔误，登记在这里备查。
- ⭐ **这张表管的是"当场没有人拍板"的那几路**（它最初只为 CLI 批量扫描而写，2026-09-23 起
  服务进程内的调度走的是同一张）。判据是**省请求**用的：一天扫一次，绝大多数轮次落在 `not-yet`，
  一个请求都不发。它**不是**"人不许刷" —— 手动那一路有 `force` 这一位越过 `not-yet`
  （但**越不过** `manual-only`，那一档打了上游也注定失败）。
  "手动那一支不许被这张表挡"来 2026-09-22 用户的两句**原话**（逐字回查过会话记录）：
  先问"还没到提前量，这一次没打上游。这是啥意思"，随后拍"不需要这个功能，有二次确认就不需要这些"
  与"你需要的是当点自动刷新，而不是到点才能手动"。
  ⚠ 用户说过的**只有这两句**：别把它扩写成任何"打上游前要先批准"式的要求（§1 那一格），
  也别引"前端完全不用带这个参数"这类**只存在于我的摘要里、记录中查不到原话**的句子。逐路落点：
  - WebUI 那枚按钮 → 路由**固定直接换**，`force` 与提前量都不作为入参（§5.2）；
  - CLI 单账号 `--account` → 默认也走这张表，要越过得显式给 `--force`；
  - CLI 批量 `--all` → 靠这张表省请求；
  - 后端自己的调度循环（`internal/server/scheduler.go` 到点触发，§5.4）→ **也是这张表本身**
    （`Force=false`：没到提前量就一个请求都不发，而那正是它该做的）；
  - 外部定时（自己配 cron 跑 `mcloud auth refresh --all`）→ 同一张表。它不再是**唯一**那条定时路，
    但仍然是合法用法（比如不想让 WebUI 常驻的人）。
  接口上 `decision` 仍如实报档位：`not-yet` + `rotated=true` 这一对就是"未到点但按点击换了"，
  所以**没有为 force 新增第四个码**（那三个码进了 JSON 契约）。
- ⚠ **三个单位不是一回事**，同屏时最容易看错的就是这里：
  - token 第 4 段 = **13 位 epoch 毫秒**；
  - 库里 `accounts.auth_expire` = 同一个**毫秒**值（由 `core.ParseAuth` 从整串里取，与登录路径同源）；
  - 上游响应的 `expireTime` = **秒**、而且是"**还能用多久**"（实测 `2592000` = 30 天）的相对时长，只带着供排障，**不落库**。
- ⚠ 判据读的是**整串里那个过期时刻**，不是 `auth_expire` 那一列。两者正常时同源，
  手工改库（排障常见动作）会让它们分叉 —— 表现是"界面上显示已过期，刷的时候却判 `not-yet`"。

## 3. 端别由库里那串决定，不是参数

`Authorization` 解码后第一段就是端别（`pc` / `mobile`），它决定配哪一套头：

- `pc` → PC 客户端那套头 + `clientType`，**再要一个 `platform`（windows / macOS）**；
- `mobile` → App 那套头（iPad 设备档案，`clientType=412`）。

为什么 `platform` 非要调用方给：库里 `accounts.platform` 存的是**端别**不是操作系统，
没记这个 PC 账号当时是 Windows 还是 macOS。为什么端别**不**做成参数：
库里是 App 的串、参数选 PC 这种矛盾组合，本仓库拦过五次同类错。

设备档案的落点也不同：App 那份是**文件**（`--app-device-file` / 服务端 `AppDeviceFile`，
指纹必须跨重启稳定，换掉它等于换一台机器），PC 那份用库里已存的 `device_id`。
CLI 一次运行只解析一次；HTTP 每次调用解析一次（一次请求只刷一个账号，同档）。

## 4. 落库：窄写四列，一个事务

`RotateAccountAuth` 只碰 `account_state.jwt_token` / `account_state.authorization` /
`account_state.updated_at` / `accounts.auth_expire`。**上游失败时库里原样不动**
（窄写只在拿到新串之后跑，且在一个事务里）。

⚠ 不自动重试。实测最慢一次 **28.4 秒** —— 重试的代价不是慢，是**再轮换一次真凭据**。

## 5. 四个触发面口径

### 5.1 CLI（参数面是冻结基准，明细见 [`cli-usage.md`](./cli-usage.md)）

```
mcloud auth refresh --account <id> | --all [--lead-days 7] [--force] [--platform windows|macos] [--db …] [--app-device-file …]
```

- `--lead-days` 给 0 或负数 ⇒ **用法错（退 2）**，不静默当默认值：
  想"到期当天才刷"的人写 `0`，静默变 7 天恰好是他想避开的那个含义。
- `--force` = **到不到点都刷**（越过 `--lead-days` 那道闸，越不过 `manual-only`）。
  它与 WebUI 那枚按钮同语义；和 `--all` 同时给等于把每个启用中的账号都换一遍 token。
  换成的那一行会印"按 `--force` 换的：还剩 N 天，本未到提前量" —— 同一句"已换新凭据"
  不区分这两件事，批量跑完人会以为提前量生效过。
- `--all` 只碰**启用中**的账号：停用是用户主动的决定，把它悄悄刷成"看起来还能用"是抹掉那个决定。
- 批量**逐个跑完**再统一报"几个失败"：中途 return 会让人不知道剩下那几个动没动。
- 失败文案的分界线与 HTTP 侧同一条：4xx / 上游业务码给原文，5xx / 网络 / 内部只给类别文案
  （那几类原文里实测带过 SQL 片段与上游响应体）。

### 5.2 HTTP（WebUI 用的那条）

```
POST /api/accounts/{id}/refresh-auth      body: {"platform"?: "windows"|"macos"}
→ 200 {"refresh": {accountId, end, decision, rotated, beforeExpire, afterExpire}, "account": {…整行展示形状}}
```

- ⭐ **这条路由没有提前量入参，点一下就换**（固定 `force`）。理由见 §2 那条订正：
  走到这里的每一次请求背后都是**刚在确认弹窗上点过一下的人**。
  旧形状里的 `leadDays` 仍**收而不理**（带了不 400、也不参与判据）—— 浏览器缓存着老版 JS，
  把它判成 400 的表现是"点了刷新说参数不合法"。
- **`manual-only` 也是 200**：那不是失败，正确出口是"重新登录 / 贴整串"，不是重试。
  这一档是这条路上唯一还能返回 `rotated: false` 的分支。
- 空请求体合法（`curl -XPOST` 不送体也能刷），等价于"按 Windows 档案直接换"。
- 先认账号再动手：不存在或不属于当前用户 ⇒ **404**（不是 400"没有存 Authorization"，
  那会把"不存在"说成"坏了"）。
- 响应里的 `account` 是**回读库**得到的整行，不是自己拼的"新的那一行"。
- `decision` 走 `RefreshDecision.Code()` 那三个**稳定英文码**；中文那句 `String()` 明说"不给机器解析"。
  要加一档就加新值，别改这三个旧值。

### 5.3 WebUI（账号管理页第四枚行内动作）

`web/src/pages/accounts-page.tsx` 的"刷新 auth"→ 一个 `Dialog`（原生 `<dialog>` + daisyUI `modal`，见 [`webui-components.md`](./webui-components.md) §5.8）：

- 弹窗正文说三件事：**会打一次上游**、新 token **只落库**（不显示、不进地址栏、不进日志）、
  上游最慢约 30 秒期间按钮是灰的（不自动重试，所以也不给连点的机会）。
- 只有**非 `mobile`** 那几行给 Windows / macOS 二选一；手机那行免选（端档案取库里那串自带的）。
  `platform` 为 null 的旧导入数据也给选择 —— 给错了后端在 App 支会忽略它，不给选择则 PC 支没法刷。
- **界面上关于"时机"只有一个判断：这一行已过期 / 还在期内。** 不提"还剩几天"、更不提提前量 ——
  那一位在这条路上没有读者（§2、§5.2）。那一行显示**已过期**时，弹窗里只给一句人工出口的提示
  （"过期之后换不换得动没有实测记录，真失败就走重新登录或贴整串 auth"），
  **不**替上游断言"多半不认"。
- 结果落**页顶那条提示**：`rotated` → 绿（`alert-soft alert-success`）并带新到期时刻；
  没换 → 中性（裸 `alert-soft`）。**两态必须不同色**（同 [`webui-code-guide.md`](./webui-code-guide.md)
  §十二 未决项表第 9 行那条"启用/停用同色"的报障口径 —— 那一行量的实况是
  "两态本来不是同一个色，是绿的那一态在浅色主题下读不出来"，所以这里的"不同色"
  靠**一个带语义色、一个裸 `alert-soft`**，不是靠把中性条也上色）。
- 失败文案**只能显示在弹窗里面**：页顶那条在遮罩背后，藏起来等于没有。
- ⚠ 这个弹窗写在**页面作用域**里而不是抽成组件：Vapor 下子组件元素的 props 只求值一次
  （[`webui-code-guide.md`](./webui-code-guide.md) §9.2 ④），抽出去会变成"点第二行、弹窗显示第一行的名字"。

### 5.4 调度循环（2026-09-23 接上的第四个触发面）

`auth-refresh` 同时是能力注册表里的一条（`CodeAuthRefresh`），为的是让 `schedules.target`
能指名它；到点跑的是**同一个 builder**（`internal/server/run_dispatch.go` 的 `buildAuthRefreshRun`），
不是又写一遍刷新实现。

- **与账号页那枚按钮的唯一区别是 `force`**：这一路上没有人，所以恒 `Force=false`（§2 那张表全生效）。
  提前量在调用点给 **0**，由能力层落到 `RefreshDefaultLeadDays` —— 服务端不另记一份数字，
  否则"界面上说的提前 7 天"与"后端实际用的天数"会各自漂移。
- **参数面只有 `accounts` / `accountMode`**：刻意**没有** `force`（定时里没人拍板，开了它
  等于"每个触发点硬换一次 token"），也刻意**没有** `leadDays`（那是这台机器替所有人定的一个数，
  不是逐行旋钮；要改成逐行可调，先改 `core.Schedule` 的契约，不是顺手加参数）。
- **每次触发的四态**：`not-yet` ⇒ **成功**（`ok`）且一个请求都不发 —— "没到点所以什么都没做"
  正是它该做的事，与签到"今天已经签过了"同一档；`manual-only` ⇒ `ManualRequired`，
  而调度那侧的映射是"非成功一律记 `error`"，所以库里那串没有有效过期时刻的账号
  会让这一行**每个触发点都报 error**（不是偶发）。两种情形下每个账号仍各留一条 `runs` 记录。
  ⚠ "非成功即 error"那一半有单元测试钉住（`scheduledOutcome` 收 `ManualRequired` ⇒ `ScheduleError`，
  `scheduler_test.go` 那张表里"要人操作"那一行）；**"库里那串判到 `manual-only` ⇒ 这一行每次触发都报 error"
  这一整条链没有测试跑过**（判据来自两段代码相推）⇒ 算"待验证"，别当实测结论。
- **错过补跑（`MissCatchUp`）**：这条能力声明的默认档是"补"，也就是**在没人看着的时刻照样换 token**。
  这正与旧项目的做法一致 —— 它把刷新挂在每轮任务收尾，本来就没有人在场（§1）；
  再加一条"到期不换更糟"。⚠ 那半句"真过期 ⇒ 那个账号所有能力都跑不动、只能人工重登"
  是**选型时的推断，不是判据**：上游在到期后回什么，本仓库零实测样本（§8）。
  注册表那条声明（`registry.go` 的 `CodeAuthRefresh`）用的是同一句措辞，读的时候按推断对待。
  一行任务可用 `schedules.miss_policy` 覆盖成 `skip`，但**目前 `PUT` 只能保留这一列、不能设它**
  （写入面还没开，见 §8）。
- **PC 那一支在定时的路上固定按 Windows 档案打**（`upstream.PlatformWindows`）：
  库里 `accounts.platform` 存的是**端别**不是操作系统（§3），手动那一路由人在弹窗里选，
  而定时这一路没有调用方可给。一个 macOS 上登录的 PC 账号会被打成 Windows 头，
  ⚠ **这样换得动还是换不动，未实测**（要核它得：真打一次把 PC 支打通，再换平台对拍）。
- **两条运行前提**（判据在 `internal/server/scheduler.go` 文件头，这里只列与"真凭据"相关的那两条）：
  **一份库只跑一个服务端进程**（水位不是跨进程原子认领，两个进程 = 同一个点各跑一次 ⇒ 同一套凭据换两次）；
  **无主行（`owner IS NULL`）一律不触发**（没有 owner 就没有 scope，答不出"这一趟看得见哪些账号"）。
- **界面还没有这一页**：`GET/PUT/DELETE /api/schedules` 三条路由已就绪，
  但 web 侧没有对应页面与路由，所以**目前建行只能直接调 `PUT /api/schedules/{id}`**
  （body：`target.kind=capability` + `target.code=auth-refresh`、`cron`、`name`，
  账号那一栏的 JSON 名是 **`account_ids`**，留空 = 全部启用账号；三条路由都在 `authed` 后面，
  手工调要带已登录会话的 Cookie）。
  `/api/status` 的 `schedulerRunning` 现在是**运行时实况**：`mcloud-server` 起了循环就是 true，
  与"界面上能不能建任务"是两件事，别按后者读它。

## 6. 凭据边界（这一条链路特有的一条）

新增的这条边界：**新 token 只落库，不进响应体、不进 URL、不进日志**。
具体落法：

- `refreshAuthPayload` 是**手写**的 map，不给 `app.RefreshReport` 补 JSON tag ——
  那个结构体将来会加字段，而"加了就自动出网"正是凭据泄漏最常见的路径；
- `RefreshedCredential` / `RefreshTokenResult` 里那两个 `core.SecretValue` 与那个 `string`
  **整个结构体没有 tag**（已登记进 `scripts/check-go-jsontags.cjs` 的白名单并写明理由）：
  `SecretValue` 的 `String()` 只挡打印，**不挡 `encoding/json`** —— 补了 tag 就等于
  任何一次"顺手 marshal 整个 report"都会把新 token 原样发出去；
- 界面上成功也只给掩码形状的 `accountId`；测试里那条守卫断言响应文本既不含新 token 明文、
  也不含它的 base64。

## 7. 指路（每层的"为什么"只写在那一层）

| 层 | 文件 | 那层独有的判据 |
| --- | --- | --- |
| 存储（窄写） | `internal/io/store.go` 的 `RotateAccountAuth` + `internal/io/store_auth_rotate_test.go` | 只碰四列、单事务 |
| 上游（请求形状） | `internal/upstream/auth_refresh.go` | §1 那张证据表（文件头就是它） |
| 能力（状态机 + 判据） | `internal/capabilities/auth_refresh.go` | §2 三档、`Code()` 是对外契约、`CredentialFromRefresh` 拼完回验 |
| 驱动（端差异 + 落库） | `internal/app/auth_refresh.go`（含 `AuthRefresher`） | 端别不作为参数、不重试、设备档案从文件现读 |
| 编排那一步的四态 | `internal/app/auth_refresh_run.go` | 为什么 `not-yet` 记**成功**、`manual-only` 记 `ManualRequired` |
| 能力声明（给调度一个可寻址的名字） | `internal/capabilities/registry.go` 的 `CodeAuthRefresh` | Hidden、不进组合编排、`Medium` 而非 `High`、默认档 `catch_up` 各自的理由 |
| 调度触发 | `internal/server/scheduler.go` | 水位先于起跑、错过整串一次弃掉、无主行不触发（§5.4 末尾那两条前提） |
| 执行分派 | `internal/server/run_dispatch.go` 的 `buildAuthRefreshRun` | 与按钮的**唯一**区别是 `force`、提前量给 0 落到默认、PC 支固定 Windows 档案 |
| CLI | `internal/cli/{args,run}_auth.go` | §5.1 那四条 |
| HTTP | `internal/server/auth_refresh.go` | §5.2 那六条 |
| WebUI | `web/src/pages/accounts-page.tsx`、`web/src/api.ts` 的 `refreshAccountAuth` | §5.3 那七条 |

## 8. 未验证 / 待办

- ⛔ **PC 支未实测**：形状按 H5 拼，打通之前 `PCRefreshTokenRequest` 按未验证对待。
  要核它得：拿一个 PC 账号真跑一次 `mcloud auth refresh --account <id> --platform pc --force`，
  看回码落在"换了"还是 `9103` 那一类（§1 信封那一行）。⚠ 这是**形状待对拍**，不是"要不要打"的闸门
  —— 旧项目每轮任务收尾都在自动刷（§1），本仓调度到点走的也是同一个动作（§5.4）。
- ⛔ **`rotated=true` 那一支界面分支只在假上游下验过**（Go 测试里断言"响应不含新 token"）；
  真上游换 token 成功后绿条与到期时刻的渲染，还没在浏览器里看过 —— 网页侧没有测试运行器（AGENTS §1.6），
  要验只能等一次真实刷新发生（手动点一次、或调度到点跑一次，都会发生）。
- ⛔ **§5.4 那一条"调度到点自己触发"同样只在假上游下验过**：测试里那条链是真判点、真推水位、
  真落 `runs` 与 `schedules` 那几列，只有上游是假的（默认夹具"不许出门"）。
  ⇒ 与上面两条同一个前提：**"到点 ⇒ 真换一个 token"至今没发生过。**
  > 耻辱柱（2026-09-23）：本篇在 2026-09-22 那一版里只有**三个**触发面，并把"到点才换"这件事
  > 整个交给**外部** cron（成文写的是"这个后端不装调度循环"）。实况是：`schedules` 那三列记账
  > 自 schema v2 落下起一直没人写（`internal/server/scheduler.go` 文件头记的正是这件事）。
  > 用户 2026-09-22 夜拍板"调度是需要的，能力也是在待办里的"，2026-09-23 落地，把那句承诺推翻。
  > 教训：**"我们不做 X"这类否定式架构承诺，写之前要问的是需求方，不是参照实现。**
- ❓ **macOS 上登录的 PC 账号在定时的路上会拿到 Windows 档案**（§5.4 那条）。上游认不认这套组合，
  两个方向都没有实测记录 —— 要核它得：真打一次把 PC 支打通，再换平台对拍。
- ❓ **换下去那串旧 token 之后还能不能用、会不会影响别的端正在用的登录态 —— 两个方向都没有实测记录。**
  既不能写"会顶掉"，也不能写"不影响"：本仓库至今**一次真上游刷新都没打过**。
  要核它得：真打一次，再拿换下的旧串在另一端打一枪只读接口对拍。
  ⚠ 这一格只登记"没测"，它**不是一道闸门**：旧项目把刷新当常规动作天天做（§1），
  本仓的手动支与调度支也已经按"到点就换"实现完了 —— 卡在这里只会拖住开发，不会多换来任何安全。
  > 耻辱柱（2026-09-22）：本日两层错。① 我把"刷新会顶掉这个端当前正登着的会话"当既成事实
  > 写进 `internal/app/auth_refresh.go` 的 `Force` 注释和本文件，实况是**一次都没实测过**，
  > 被用户当场质问"你真的测试过吗，还是瞎说的"。② 被质问后我把它**归因给用户**（"这句最早出自你上一条消息"），
  > 用户回"我特么从来没说过会踢下线"。逐轮回查会话记录：「会踢下线」与「不影响其他端」**两句都不在**
  > 用户任何一轮消息里，全部出自我自己的输出 —— 连那句"反向的"也是我给的。
  > 代码注释已改成只陈述核实过的那件事（窄写：只碰 §4 那几列，门禁断言其余列不动）。
  > 教训两条：**风险的来源没实测过就写成疑问，不写成断言**；**要引用用户说过的话先逐字回查记录**，
  > 记不住就说"我记不准"，不把锅递到对方头上。展开见 `migration-pitfalls.md` #125。
  > ⚠ **同族复发 2026-09-23 两次**，记在 §1 那一格的耻辱柱与 #125 的 ⑤⑥ 两变体，此处不重述。
- ❓ 请求体要不要补 AES-192 信封（§1 最后一行那两种表现之一）。
- ❓ 停用账号在 WebUI 上仍给"刷新 auth"这一枚（CLI 的 `--all` 不碰它们）：
  目前靠弹窗里那句说明承担差异，没做界面屏蔽。
- 📋 **待办（都在 §5.4 那条链上，按依赖顺序）**：
  1. ~~定时任务页~~ —— ✅ **2026-09-23 落地**：`web/src/pages/schedules-page.tsx` 接上
     `GET/PUT/DELETE /api/schedules`（列表 + 建/改/删/启停，能力表来自 `/api/capabilities`，
     调度是否在跑来自 `/api/status`）。实机核验与**仍未验**的那几点记在
     [`status.md`](./status.md)"定时任务页"那一段；
  2. `PUT /api/schedules` 补 `miss_policy` 的**写入与校验**（现在只保留、设不了；
     未知文本必须 400，不得静默变 NULL）。⇒ 界面对应地**只显示不给编辑入口**
     （`schedules-page.tsx` 文件头第 3 条）；这条打通后要一并加那个下拉；
  3. 指向 `pipeline`（任务组合）的排期还没接上执行 —— 到点只记一句 error，
     所以"把刷新排进日常组合"这条路暂不成立，而它也正是这条能力 `RunInBatch: false` 的同一判据。
     ⇒ 界面上这一类目标**只显示不改**（没有 `/api/pipelines` 对外接口可列候选），
     保存时把库里那一份 `target` 原样送回去。
