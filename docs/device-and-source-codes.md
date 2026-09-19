# 设备码 / 来源码 / 渠道码 —— **是"调用方按场景选的参数"，不是端常量**

> 状态：**结论已定；取值只收有出处的**（2026-09-19）。
>
> 直接动因：`mcloud-upstream::app_signin` 的 `TO_SOURCE_ID` 初版注释写成了"App 端的常量"，
> 而依据是 PC 客户端包里的默认值 —— 被指出这是**跨端外推**。
> 查下去发现比"跨端"更糟：**同一个 PC 客户端文件里，同一条接口就有三个不同的合法值**。
>
> 本文钉三件事：**这些码分几类、每个值的出处在哪一行、实现时该放在哪一层**。
> 规矩沿用 `docs/rust-rewrite-plan.md` §7.0 —— **不写没有出处的常量**。

---

## 1. 一句话结论

这类码（`toSourceId` / `channelSrc` / `clientType` / `sourceChannel` …）**绝大多数不是"这端的属性"，
而是"这次调用要冒充谁 / 要落到哪个业务入口"**。所以：

- **同一条接口、同一个进程、同一个用户，在不同调用点传不同的值是正常的**（§2 有代码级证据）；
- 因此它们**必须从调用点传进来**，不能钉成端档案里的字段；
- **任何硬编码都得能追到一份实拍流量或一行上游源码**，否则就是一个会静默生效的猜测。

⚠ 反面教材（本文存在的理由）：`toSourceId` 一度被当成"端常量"，
PC 的默认值被拿来注解 App 的抓包 —— 而两者只是**碰巧撞了同一个数**。

---

## 2. 最强的一条证据：**同一个 PC 客户端文件里，`toSourceId` 有三个值**

仓库内已归档该文件：`docs/capture/apiServer.js`（上游客户端源码，按来源豁免脱敏，
见 `scripts/check-identifiers.cjs` 的 `VENDOR_SOURCE_PATHS`）。

同一条接口 `POST /user/querySpecTokenV2`，同一个文件里三处调用：

| 行 | 调用点 | `toSourceId` | 拿到票据之后干什么 |
|---|---|---|---|
| [`878-884`](./capture/apiServer.js) | `auth.getCloudTicket(toSourceId)` | **形参，默认 `001003`** | 只换票，用途由调用方决定 |
| [`901-948`](./capture/apiServer.js) | `auth.openWebBrowser(redirectUrl)` | **`001005`** | 换票 → AES-ECB + RSA → 打开 `yun.139.com/caiyun/openapi/authentication/singleLogin?clientCode=10301&channel=0010112&…`（[L935](./capture/apiServer.js)） |
| [`950-981`](./capture/apiServer.js) | `auth.openAIWebChat()` | **`001248`** | 换票 → 打开 `www.cmccaikf.com/cs-wap/sso/ssoLogin?…&targetsourceid=001005&channel=pc`（[L969](./capture/apiServer.js)） |

而唯一的默认值调用点是不传参的：`Utils.openAdvertiesmentBycommon` 里
`apiServer.auth.getCloudTicket()`（`common/utils.js`）→ 走 `001003`，用于**广告位跳转**
（把票据填进 `#ssoToken#` 再开 WebView）。

从这四处能直接读出三条**规则级**结论：

1. **`toSourceId` 是调用方参数。** 同一个客户端、同一条接口、三个值，
   差别只在"这次要落到哪个业务入口"。
2. **请求体里的 `toSourceId` 与随后打开的 URL 里的 `targetsourceid` 不要求相等。**
   `openAIWebChat` 是 **`001248`（请求体）vs `001005`（URL）**。
   ⚠ 这条推翻了"两处必须同值"的直觉 —— 签到链路里两处恰好都是 `001005`，
   那是**这一条链路的实况**，不是上游的约束（见 §6 第 4 条）。
3. **同一个文件里还有第二个按场景变的码**：`clientCode`。
   [`L890`](./capture/apiServer.js) 的注释直接写明
   `PC Windows填10301 PC Mac填10302 UOS填10303`，而 `openWebBrowser` 里写死 `10301`
   —— 因为它自己就是 Windows 版。**同一个码按平台变，注释还得留在代码里。**

---

## 3. 第二强：App 原生的三个值（同一条接口）

`docs/capture/app-signin-samples.txt` §2.1 已收口（App 自身 FXTrace 日志，不是代理抓包）：

| 值 | 出处 | 场景 |
|---|---|---|
| `001005` | **签到页 URL 自带 `targetSourceId=001005`**；`CredentialTokenAction` / `MCQueryGotoneAction` / `MCCreditStateAction` / `MCUserRightsAction` | **签到链路** |
| `001003` | 同页那条 **WebKit 发起**的请求；`MCUserRightsAction` / `MCQueryGotoneAction` 各 1 次 | 权益 / 云票 |
| `010001` | `MCUserRightsAction` 2 次 | 权益 |

⇒ **跨端（PC ↔ App）与跨发起方（原生 ↔ H5/WebKit）的常量一律不互搬。**
同一条接口在同一个 App 进程里有两个发起方，头与参数都不一样 —— 判据是 HAR 里的
`_application`：`com.apple.WebKit.Networking` 是 WebView，**不是**原生。

---

## 4. 旧项目里"**故意换设备码**"的请求（这一节是用户点名要找的）

旧项目 `/Users/catlair/Documents/workspace/mcloud-sign`（只读参考，未改）。
下面每一条都是"**同一个账号，为了让上游认成另一个端 / 另一个入口，主动换码**"。

### 4.1 用户举的那条：每月任务「113 使用 PC 客户端」

`packages/core/src/tasks/app/runners.ts:90-96`：

```ts
// ==================== 每月任务 ====================
/** 113: 使用PC客户端 */
113: async (ctx) => {
  await refreshAuthToken(ctx);
  await sleep(1000);
  await uploadRandomFile(ctx, { channelSrc: "10200153" });
},
```

**形态是"换渠道码 = 换端"**：任务本身只要求"用 PC 客户端上传一次"，
所以实现先刷令牌（`refreshAuthToken`），再用 **PC 的渠道号**走一次上传。
上传接口是同一个 `/hcy/file/create` —— 变的只有码。

### 4.2 `channelSrc` 不是单个头，是**整套头三选一**

`packages/core/src/apis/file.ts:13-48` 的 `getFileOpHeader({channelSrc, opType})`：

| `channelSrc` | 附带换掉的整套头 |
|---|---|
| `CHANNEL_SRC.pc`（`10200153`） | `x-yun-market-source: 001`、`x-yun-module-type: 100`、`x-yun-svc-type: 1`、`x-yun-client-info: CAIYUN_PC_CLIENT`、**`User-Agent: Mozilla/5.0`** |
| `CHANNEL_SRC.android`（`10000023`） | `x-huawei-channelSrc`、`x-yun-client-info: CAIYUN_APP_CLIENT`、`x-yun-op-type`（backup→2 / upload→1）、`x-yun-sub-op-type`（op×100） |
| 其余（web） | `x-huawei-channelSrc`、`x-yun-client-info: CAIYUN_WEB_CLIENT`、`x-yun-device-id` |

渠道号表在 `packages/shared/src/constants.ts:66-70`：

```ts
export const CHANNEL_SRC = { web: "10000034", pc: "10200153", android: "10000023" } as const;
```

⇒ **"换端"在这条链路上 = 换一个字符串**，而它同时改掉 5~6 个头（连 UA 都换）。
实现侧如果只把 `x-yun-app-channel` 做成参数、别的头仍取端档案，就会造出
**一个上游一眼可辨的矛盾组合**（PC 的渠道号 + App 的设备串）。

> ⚠ 旧项目 web 分支把 `x-yun-device-id` 填成了 `CAIYUN_WEB_CLIENT`（即 client-info 串）
> —— 看起来是复制粘贴失误。**不要照抄**，这一处我们没有独立证据。

### 4.3 逐条清单

| 场景 | 位置 | 换的码 |
|---|---|---|
| 409 / 1005 云朵中心入口 | `tasks/app/runners.ts:99-130` | 走 **`mrp.mcloud.139.com`** 的 `…/market/signin/page/doTask?client=app&deviceId=…` |
| 刷新令牌 | `apis/auth.ts:151-166` | `refreshToken(clientType = "414")`（形参）+ 固定 `x-yun-app-channel` / `x-huawei-channelsrc` = `10000023` |
| 云笔记 | `apis/note.ts:20-26` | `x-huawei-channelsrc: 10001400`、`APP_CP: android`、`CP_VERSION: 3.2.0`、`User-Agent: mobile` |
| 分享（建/删外链） | `apis/share.ts:48-56` | `mcloud-channel: 1000101`、`x-huawei-channelsrc` / `x-yun-app-channel` / `x-yun-channel-source` = `10000034`、`x-yun-client-info: ‖9|7.14.4|edge|…`（**冒充 Edge/Linux**） |
| AI 工作台 | `apis/ai-workbench.ts:163-175` | `x-yun-app-channel: sourceChannel`（形参，默认 `10175`）；`getCloudTaskStatus` 的**请求体**也带同一个 `sourceChannel` |
| AI 相机 / 上传 10 图 | `tasks/ai-tasks.ts:28,143` | `x-yun-app-channel: 10214200` + `x-yun-client-info: ‖8‖…`（**一个几乎空的设备串**） |
| AI 相机对话体 | `tasks/ai-tasks.ts:110` | body `sourceChannel: "101"` |
| 玩 AI 抽红包 | `tasks/play-ai-special/task.ts:202-203` | `x-yun-client-info: 6‖2|12.6.0‖iPhone|DE3B…‖ios 18.7…`（**冒充 iPhone**）+ `x-yun-app-channel: 101` |
| 邮箱 | `apis/mail139.ts:100,132` | XML 里 `<string name="channel">10242000</string>` |
| 红包派对 | `tasks/cloud-phone-redpack/api.ts:8,126` | `CHANNEL_SRC = "red-mcloudapp-001"`（**非数字**）；`getRcsToken(targetSourceId = "0")` |
| 通知任务列表 | `apis/task.ts:37-46` | `clientVersion: "11.2.1"` —— **与真机不同**，见 `README.md` 的"旧 TS 与真实流量不一致"对拍表 |

**这张表本身就是结论**：一个账号、一个进程，为了拿齐不同任务，
会轮流冒充 PC / 安卓 / iPhone / Edge-on-Linux / 邮箱端。
所以"我们这端该发什么头"这个问题**在跨任务的语境下没有唯一答案**。

### 4.4 旧 HAR 里 `targetSourceId` 的全部取值（15 个）

统计范围：旧项目 `har/*.har`。**这是"取值的全谱"，不是"语义表"** ——
除 `001005` / `001003` 外，其余只做到"与 HAR 场景相关"，**没有代码级语义**。

| 值 | 次数 | 与哪些 HAR 场景同现 |
|---|---|---|
| `001005` | 3514 | 几乎每个 HAR 都有（默认入口） |
| `017027` | 238 | 直播系列（`直播716/723/730/wss`、`直播登录`、`领小红花18/2`）—— `直播登录.har` **只有这一个值** |
| `001216` | 69 | 知识库、每月阅读任务、切换账号、复活 |
| `001304` | 46 | 知识库、每月阅读任务、复活 |
| `017018` | 31 | 直播系列、红包派对、旧照片 |
| `056002` | 22 | 直播系列、红包派对、合成云朵、夏日拍拍大作战 |
| `001003` | 21 | `3d.har`、`旧照片.har`、`玩 AI 抽支付宝红包.har` |
| `012012` | 13 | 红包派对、合成云朵、夏日拍拍大作战、`使用知识库3.har` |
| `001248` | 9 | 知识库、`开启通知领云朵.har`、`sign_in_3任务列表.har` |
| `001191` | 9 | 合成云朵、夏日拍拍大作战、红包派对 |
| `007001` | 6 | 同上 |
| `051003` | 4 | 同上 |
| `001205` | 4 | 同上 |
| `001327` | 3 | 同上 |
| `001326` | 1 | `红包派对.har` |

两个只能"存疑记录"的观察（**不要当结论用**）：

- `红包派对.har` / `合成云朵.har` / `夏日拍拍大作战.har` 三个文件各自都同时出现 10~12 个值
  —— 说明**一条 HAR 窗口里多值同现是常态**，不能靠"一个 HAR 一个值"去反推语义；
- 旧项目源码里唯一带语义注释的是 `apis/auth.ts:52`：
  `@param sourceId 源 ID（001005=默认，001003=邮件）`。
  **其余 13 个值没有任何注释**，别从 HAR 文件名给它们编名字。

---

## 5. 这些码分几类（放代码时的判据）

| 类 | 判据 | 例子 | 该放在哪 |
|---|---|---|---|
| **每端常量** | 换端才变，同端所有接口一致 | `clientType` / `cpid` / `version` / App 的 `x-yun-app-channel: 10000020` | 端档案（`ClientProfile` / `AppProfile`） |
| **每调用参数** | **同端、同接口、不同调用点可以不同** | `toSourceId` / `channelSrc` / `sourceChannel` / `marketName` / `clientCode` | **函数形参**，调用点显式传 |
| **每请求随机** | 每次请求都要新值 | `msgId` / `nonce` / `x-request-id` / `x-yun-tid` | 构造函数内部现生成 |
| **服务路由** | 与主机一一对应 | `service_identifier_key`（见 `client-profiles.md` §3.3.1） | 按主机查表 |

⚠ **"每端常量"这一类的边界比看起来窄**：`x-yun-app-channel` 在旧项目里
**同时**是端常量（登录链路 `10000020`）和每调用参数（AI `10214200` / `10175` / 分享 `10000034`）。
判据是：**它能不能在同一端的两个接口上取不同值？能，就是每调用参数。**
`x-yun-app-channel` 属于后者 —— 它只是**有个默认值**的每调用参数。

---

## 6. 落地规则（照这几条写代码）

1. **默认值只在有实拍时才给，且默认值本身要有测试钉住。**
   `getCloudTicket` 的 `001003` 是上游自己写的默认值；我们这边的默认值必须能追到一行抓包。
2. **一个码换掉的不止一个头时，把整组一起换。**
   `channelSrc` 换 5~6 个头（§4.2）；只换一个 = 造出矛盾组合。
3. **同一个概念在请求体与 URL 里各出现一次时，不要假定它们相等。**
   `openAIWebChat` 是 `001248` vs `001005`（§2 结论 2）。
4. **不设"看起来合理"的默认值。** 默认值等于"一个会静默生效的猜测"：
   上游哪天开始校验，错的那个**不报错、只是不生效**。
   `app_client::ios::AUTH_PREFIX` 故意不给默认值，是同一条理由。
5. **`sourceid` ≠ `targetSourceId`。** 签到页 URL 里两个都有，值不同：
   `sourceid=1002`（页面自身）vs `targetSourceId=001005`（票据入口）。
6. **名字撞车要先分辨，别合并**：`deviceid`（头，cMPassport 那套 `B…`）vs
   `deviceId`（H5 参数，89 字符 base64）；`userid`（12 位）vs `userDomainId`（19 位）；
   `x-yun-app-channel` vs `x-huawei-channelSrc`（App 端同值、PC 端不同值）。

---

## 7. 与本仓库代码的对应

| 位置 | 现状 | 备注 |
|---|---|---|
| `mcloud-upstream::app_signin::TO_SOURCE_ID` | `"001005"`，作为**本功能默认值**；`query_spec_token_body(to_source_id)` 已经是形参 | 注释已改为"调用方参数"，并写明 PC 那个 `001003` 不能拿来换 |
| `mcloud-upstream::app_signin::SOURCE_ID` | `"1002"`（URL 的 `sourceid` / `tyrzLogin` 体的 `sourceId`） | **与 `TO_SOURCE_ID` 不是同一个码** |
| `mcloud-upstream::app_client::ios::CHANNEL` | `"10000020"`，在 `AppProfile::headers()` 里 | 属"每端常量"；接 AI / 分享类接口时**要能按调用覆盖** |
| `mcloud-upstream::client::HUAWEI_CHANNEL_SRC` | `"10200153"`（PC） | 与旧项目 `CHANNEL_SRC.pc` 同值 —— 这是**唯一**一处旧项目与本仓库能对上的渠道码 |
| `mcloud-upstream::app_client::ios::AUTH_PREFIX` | `"mobile"`，**故意无默认值** | 见 §6 第 4 条 |

**下一批接口（任务列表 / 领云朵 / 分享）落地时的动作**：
先把该接口在抓包里的码抄进构造函数当形参，**不要**先加到 `AppProfile::headers()` 里。

---

## 8. 未收口（**不猜，等证据**）

- 15 个 `targetSourceId` 里只有 2 个有代码级语义（§4.4）；
- `mrp.mcloud.139.com/market/signin/page/doTask`（旧项目 409/1005 用）与签到主体的
  `m.mcloud.139.com` **是两个主机**，本仓库**没有**这条主机的实测流量；
- 旧项目 `getFileOpHeader` web 分支的 `x-yun-device-id = CAIYUN_WEB_CLIENT` 疑似笔误，未采信；
- App 侧 `x-huawei-channelSrc` 在签到抓包里出现过第二个值（`10252400`），
  与登录链路的 `10000020` 不同 —— **谁在换它、什么时候换，没有结论**；
- `x-yun-market-source: 001` / `x-yun-module-type: 100` / `x-yun-svc-type: 1`
  这套 PC 侧的头，App 端要不要跟着 `channelSrc` 换，**没有实测**。

以上每条，**接对应接口时用真实流量回填**，不要从旧项目反推。

---

## 9. 交叉引用

- `docs/client-profiles.md` —— 各端差异与设备指纹（**每端常量**那一层的完整清单）；
- `docs/capture/app-signin-samples.txt` §2.1 —— App 原生三个 `toSourceId` 的原始样本与纠错记录；
- `docs/capture/apiServer.js` —— §2 引用的 PC 客户端源码（vendor 素材，按来源豁免脱敏）；
- `docs/upstream-app-business-api.md` —— B 族的 `signature` / `encodeData`（**与本文的码无关**，别混）。
