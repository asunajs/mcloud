# 各端差异与设备指纹（**PC 已实现；iOS App 已实测落地；其余端待实现**）

> 状态：PC 端已落地为 `mcloud-upstream::client::ClientProfile`；
> **iOS App 端已落地为 `mcloud-upstream::app_client`（2026-09-18 实测，见 §3.3 / §5.3）**；
> 安卓 / 邮箱端 / 微信小程序端**仍未实现**（§7）。
>
> 记这条的直接原因：P2.5 做登录时发现 Windows 客户端版本是 `8.8.6.20260829`、
> 本机 macOS 客户端是 `4.2.1`，于是"版本号到底该填什么"变成一个必须回答的问题 ——
> 而答案是"看你冒充哪个端"。后期还要接安卓、iOS、邮箱端、微信小程序端，
> 每端的版本号、UA、认证方式都不一样。
>
> 本文钉住三件事：**差异在哪、证据在哪、哪些还不知道**。
> 依据 `docs/rust-rewrite-plan.md` §7.0 的硬约束：**不写任何猜测性的头值或常量** ——
> 所以下面凡是"已实测"的行，都能追到一份真实流量；凡是推断的，都显式标了"未确认"。

---

## 1. 结论：头差异最大，体也不是"只差三个参数"

- **请求头差异很大**：认证头、设备指纹头、渠道头、埋点头、签名头，**每端一套**（§3 有实测对照）。
- **请求体差异比原先估计的大**：原先以为登录链路只随端变 `clientType` / `cpid` / `version` 三个公共参数
  （见 `docs/upstream-login-protocol.md` §6），但 iOS App 实测推翻了这一点：
  它还多了 `pintype=8`、`extInfo.ifOpenAccount` 由数字变**字符串**、
  而且**整个请求体是加密的**（PC 是明文 JSON）。
  也就是说"请求体差异很小"这个判断**只对 PC 内部的 win/mac 之间成立，跨端不成立**。

对实现的影响（这一条才是本文存在的意义）：

1. 新写接口时**不要把某一端的头硬编码进 `mcloud-upstream`**，头应由"端档案（client profile）"提供；
2. 端档案落地前，新接口**只发该接口已被实测确认的头**；
   落地后（PC：`client::ClientProfile`；iOS App：`app_client::AppProfile`）**一律从 profile 取头**，
   构造函数里不出现字面量头值；
3. 设备指纹（`deviceId` / `deviceInfo`）是**每安装**一次生成、永久持久化的，不能每次请求现编；
4. **不要把"PC 的形态"当默认值**。两端在设备串语法（§5.2 vs §5.3）与请求体编码（明文 vs 加密）
   上都不兼容，写接口时先问"这是哪端"。

---

## 2. 证据来源

| 编号 | 来源 | 能证明什么 |
|---|---|---|
| A | `test/fixtures/http/`（30 份夹具 / 709 条真实请求） | 各端真实发过哪些头（§3 按 UA 分组统计） |
| B | PC 客户端 `common/utils.js: sendRequest`（L226-289） | PC 端每个头的构造逻辑与条件分支（§4） |
| C | PC 客户端 `apiServer.js: getClientDeviceInfo`（L613-635） | 设备指纹怎么生成、怎么持久化（§5） |
| D | `docs/capture/*` + `docs/upstream-*.md` | 登录链路实测（该链路只出现 `x-yun-tid`） |
| **E** | **`docs/capture/app-sms-login-samples.txt`**（iOS App 自身 FXTrace 网络日志） | **iOS App 端的头集合、设备串、业务常量（§3.3 / §5.3），且是逐字节可复算的** |

A 是**多端**证据（UA 分组天然按端分开），B/C 是 **PC 单端**的代码级证据，
E 是 **iOS App 单端**的证据 —— 它比 A 强，因为 A 只统计"发过哪些头"，
E 还给了"头里每个字段长什么样"，并且能用二进制反汇编交叉验证（见 `docs/upstream-app-sms-login.md` §2）。

> E 的取值来源要特别说明：**不是**代理抓的，而是 App 自己写在沙盒里的网络日志
> （`~/Library/Containers/…/Documents/log/FXTrace*.log`，`Action_outputNetworkingLog:`）。
> 走代理时 App 会把请求体加密后再发，而日志打的是**加密前**的明文 ——
> 这也是本次能一次对齐所有字段的原因。方法见 `docs/capture/app-sms-login-samples.txt` §0。

---

## 3. 实测：按 UA 分组的请求头（证据 A）

| 分组（UA 特征） | 请求数 | 该组特有的头（区分点） |
|---|---|---|
| `Mozilla/5.0 (Linux; Android 12; …; wv)` | 216 | `app_auth` `app_cp` `cp_version` `deviceid` `deviceinfo` `msgid` `signature` `systemmd5` `tokenmd5` `x-yun-neauth` `x-yun-rand` `x-nettype` `channel-id` `activityid` `mcloud-*` |
| `Mozilla/5.0 (iPad; CPU OS 18_5 …)` | 203 | `appid` `reqtmp` `requestid` `sign` `token` `x-kpcc-clientid` `x-kpcc-clientkey` `x-origin` **`x-yun-uni`** `x-tingyun` |
| `okhttp/4.12.0` | 180 | `anonymous` `businessid` `c-client-version` `c-network` `c-system` `mcloud-device` `mcloud-sign` `signtoken` `x-m4c-account` `x-m4c-caller` `x-m4c-token` `x-nationcode` `x-mm-source` `yun-encrypt-index` `yun-encrypt-key` `x-yun-uni` |
| `Dalvik/2.1.0 (…Android 12…)` | 41 | `clienttype` `cmcc-encrypttype` `interfaceversion` `is64code` `os` `osversion` `protocolversion` `sdkversion` `traceid` `charset` |
| `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)` | 21 | `cms-device` `mcloud-route` `mcloud-userid-flag` `x-m4c-caller` `x-m4c-src` `x-yun-channel-source` `x-yun-userdomainid` `caller` |
| `Mozilla/5.0 (iPhone; CPU OS 18_5 …)` | 19 | `deviceid` `jwttoken` `x-nonce` `x-signature` `x-timestamp` `x-request-id` `showloading` `activityid`（**头最少**的一组） |
| `okhttp/3.12.2` | 15 | `x-sdk-channelsrc` `x-yun-uni` `x-mm-source` |
| `mobile` | 6 | `app_auth` `app_cp` `app_number` `note_token` `cp_version` `hcy-cool-field` `charset` |
| `中国移动云盘/<版本> CFNetwork/…`（UA 是 percent-encoded） | 4 | `mop-app-version` `mop-encryption-type` `mop-sdk-key` `mop-sdk-version` |
| `okhttp/3.11.0` | 2 | `mop-app-version` `mop-basic-pack-version` `mop-encryption-type` `mop-sdk-key` `organid` `lang` |
| `Apache-HttpClient/UNAVAILABLE (java 1.4)` | 1 | 仅有基础头 |
| `139PushMail_inhouse/<版本> CFNetwork/…` | 1 | 仅有基础头（+ `cookie`） |

### 3.0 一条**实测确认**的结论：登录接口强制要求设备头

2026-09-18 实现扫码登录时撞上的（已写进 `docs/upstream-login-protocol.md` §3 的补正）：

| 请求头 | `/user/thirdlogin` 响应 |
|---|---|
| 只有 `x-yun-tid` | `{"code":"9999","message":"登录失败！"}` |
| + `x-yun-client-info` / `x-DeviceInfo` / `x-MM-Source` / `x-huawei-channelSrc` / `x-UserAgent` | `{"code":"200059541","message":"未找到设备的二维码信息"}` ✓ |

也就是说**"请求头不对"的失败长得像"登录被拒"**（`9999` 那个码在协议文档的任何错误码表里都没有），
而不是像"参数不合法"。这一条把"头必须逐端对齐"从设计原则变成了**排查经验**：
以后任何接口返回莫名其妙的业务码，第一个该试的是"把该端的头补齐"。

PC 端这套头已落地为 `mcloud-upstream::client::ClientProfile`（含设备信息串的构造）。

### 3.1 跨端**共有**的头（做 profile 时的公共部分）

- `x-deviceinfo`（264 次，出现组最多）—— **设备指纹的通用载体**，但各端内容格式未必相同；
- `x-huawei-channelsrc`（166 次）—— 华为渠道号，跨多端都有；
- `x-tingyun` / `x-tingyun-id` —— 听云 APM 埋点；
- `version` / `appversion`、`clientid` / `deviceid`、`platform`、`x-nettype`、`msgid`、`timestamp`；
- `signature` / `signtoken` / `x-signature` / `mcloud-sign` —— **签名头各端名字与算法都不同**，
  这是 profile 里最需要逐端确认的部分。

### 3.2 端名归属**尚未确认**

上表的"端名"只是**按 UA 与头名推断**的，除 PC 外**都没有与真实端核对过**。已知的推断与疑点：

- `x-yun-uni` 说明该端基于 **uni-app**（跨端框架），所以 iPad / okhttp 两组都带它，
  不能据此断定是"小程序"还是"App" ——
  **现已由 iOS App 实测确认：它的值是 19 位 `userDomainId`，且登录后才出现（§3.3）**；
- `mop-sdk-*` 疑似移动 MOP 框架（具体载体：快应用 / 小程序 / 内嵌 H5，**未确认**）；
- `139PushMail_inhouse` 是**邮箱端**（用户已提到要接邮箱端，这条是现成证据）；
- `中国移动云盘/<版本> CFNetwork` 是 **iOS 原生**（`CFNetwork` + 版本号形如 `04241958`）；
- `Mozilla/5.0 (Macintosh…)` 那组带 `sec-ch-ua-*`，更像**浏览器/Electron 外壳**而非原生客户端。

**结论**：接某一端之前，先拿该端**真实流量**核对上表，不要把推断当结论。

### 3.3 iOS App 端：**已实测**（证据 E，已落地为代码）

上面 §3 那张表是按 **UA 特征**分组的，而 iOS App 的 UA 是
`mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)` —— **它根本没出现在那张表里**。
这是一个必须说清的坑：表里那行 `Mozilla/5.0 (iPad; CPU OS 18_5 …)`（203 条请求）
**不是**原生 App，而是走 WebView/uni-app 的页面（它带 `x-kpcc-*` / `sign` / `reqtmp` 那一套）。
两者名字里都有 `iPad`，混起来会写出一套**既不是 PC 也不是 App** 的头。

原生 App 实测发出的头（16 个，`/user/sms/getSmsCode` 与 `/user/thirdlogin` 完全一致）：

| 头 | 实测值（脱敏） | 备注 |
|---|---|---|
| `Pragma` | `no-cache` | |
| `x-yun-user-timestamp` | `1789…`（毫秒） | **每请求变**，由调用方注入 |
| `x-yun-app-channel` | `10000020` | 与 `x-huawei-channelSrc` 同值（PC 端这两个头是不同值） |
| `x-yun-net-type` | `1` | |
| `x-yun-svc-type` | `1` | |
| `x-huawei-channelSrc` | `10000020` | |
| `x-yun-api-version` | `v1` | |
| `x-UserAgent` | `412\|iPad\|ios 26.6\|12.6.0-200` | 自造格式，**不是** HTTP UA |
| `x-yun-client-info` | 16 段设备串（§5.3） | |
| `Accept-Language` | `zh-CN` | |
| `service_identifier_key` | `service_identifier_userDomain` | **PC 端没有这个头**；少了它会路由到别的服务 |
| `User-Agent` | `mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)` | 真 HTTP UA |
| `x-DeviceName` | `aVBhZA==`（base64 的 `iPad`） | |
| `x-DeviceInfo` | 14 段设备串（§5.3） | **与 `x-yun-client-info` 不是同一个值** |
| `x-yun-user-agent` | `iPad\|iPad\|ios 26.6\|mCloud12.6.0-200` | |
| `x-SvcType` | `1` | |
| `x-yun-uni` | 19 位 `userDomainId` | 登录**后**才有的值（§3.2 那条推断在这里被证实） |

登录链路里 App 端**没有**出现的头（反过来说明它们不是必需的）：
`x-yun-tid`、`x-kpcc-*`、`sign`、`reqtmp`、`yun-encrypt-index`、`yun-encrypt-key`、
`Authorization`。**特别地：没有 `yun-encrypt-*`** —— 这条很关键，
它排除了"App 端走国密会话密钥"的猜测，实际用的是另一套（见 `docs/upstream-app-sms-login.md` §2）。

#### 3.3.1 已登录请求：`Authorization` 与头超集（同源证据）

`Authorization` 在**登录链路**上不出现，但同一份日志里**已登录**的请求有 1076 处，
形态是 `Basic base64("mobile:<手机号>:<authToken>")`：

| | PC | App |
|---|---|---|
| `Authorization` 载荷第一段 | **`pc`** | **`mobile`** |
| `authToken` 形状 | `前缀\|1\|RCS\|13 位毫秒\|载荷` | **完全一样** |
| 未登录时的占位 | 不带这个头 | `Basic base64("mobile:(null):(null)")` |

也就是说**只有第一段的字面量不同** —— 所以实现侧把它做成按端给的参数
（`app_client::ios::AUTH_PREFIX` + `AppProfile::authorization`），
**不设默认值**：默认值会让"给 App 的登录结果套了 PC 的前缀"静默通过。

已登录请求比登录链路**多**这些头（记下来是为了下一批抓包不用重来）：

> ⚠ **这些头分属两套互斥的形态，不要混着用**（2026-09-19 实测，见
> [`capture/app-api-families.txt`](./capture/app-api-families.txt) §1）。
> 「族」列的 **A** = 带 `x-yun-uni`、**没有** `signature`；
> **B** = 带 `signature`、**没有** `x-yun-uni`。
> 实测 12243 条**零反例**：`signature` 与 `x-yun-uni` 从不共存。

| 头 | 形态 | 族 | 备注 |
|---|---|---|---|
| `version` | `V1.0` / `V1.1` | **B** | 与请求体里的 `version`（`12.6.0`）不是一回事 |
| `clientId` | `iosapp` | **B** | |
| `authType` | `1` | **B** | |
| `anonymous` | `0`（未登录 `1`） | **B** | |
| `timeStamp` | 13 位（**毫秒**） | **B** | 与 `x-yun-user-timestamp`（16 位微秒）**单位不同**，且**差一个整 8 小时** —— 见下 |
| `msgId` | 32 位小写 hex | **B** | 每请求一个，**响应会原样回同一个值**（可做关联） |
| `signature` | 64 位小写 hex | **B** | = `SHA256("clientId=iosapp&clientKey=…&msgId=…&timeStamp=…&version=…")` —— 见 [`upstream-app-business-api.md`](./upstream-app-business-api.md) |
| `businessId` | `600` 等 | **B** | 只有部分接口有 |
| `service_identifier_key` | **按服务变** | **B** | **与主机一一对应**（已查清，见下）—— 不能钉成端常量 |
| `x-yun-uni` | 19 位十进制 | **A** | **就是 `userDomainId`** —— 见 §3.3.2 |
| `x-yun-app-channel` / `x-huawei-channelSrc` | `10000020` | **A** | 登录链路没有 |
| `hcy-cool-flag` / `hcy-ios-option-enc` | `1` / `1` | **A** | **不是** `hcy` 系列独有 —— `getSmsCode`、`isbo2/*`、`IOutLink/*` 上都有；见 §7 的缺口说明 |
| `API-VERSION` | `v2` | **A** | |

**`service_identifier_key` 与主机一一对应**（2026-09-19 全窗口统计，12243 条）：

| 主机 | `service_identifier_key` | 条数 |
| --- | --- | --- |
| `middle.yun.139.com` | `service_identifier_caixun` | 6164 |
| `portal-njs.yun.139.com` | `service_identifier_recommend` | 2 |
| `user-njs.yun.139.com` | `service_identifier_userDomain` | 1 |
| 其余 **12 个**主机 | **完全不带这个头** | 6063 |

⇒ 落地规则是**按主机查表**，表里没有就**不发**。
两个容易踩的点：`_cloudDisk` 这个取值**本窗口一次都没出现**（有出处、无样本，别凭名字猜它配哪个主机）；
`grdt.yun.139.com`（`pDynamicInfo/queryBatchList` 所在）**恰恰是不带的那个** ——
别看到"业务接口"就顺手加上。

**`timeStamp` 与 `x-yun-user-timestamp` 的关系是确定的**（两组样本，差值都恰好 `28800` 秒）：

```
timeStamp(ms) = x-yun-user-timestamp(μs) / 1000 + 28_800_000
```

即 `x-yun-user-timestamp` 是**真 UTC 纪元微秒**，而 `timeStamp` 是**把本地墙上时间
（UTC+8）当纪元毫秒**写出来的。所以 `timeStamp` 比真纪元**快 8 小时**。
这不是笔误、也不是随机数：两个值在同一个请求里同时出现且恒定相差 8 小时，
实现侧要发 `timeStamp` 就得照这个偏移算（若 `signature` 覆盖了它，偏移算错会直接失败）。

#### 3.3.2 `x-yun-uni` 就是 `userDomainId`（**补上了计划里那个"落空"的缺口**）

`docs/rust-rewrite-plan.md` §7.2 记着一个缺口：*"`user_domain_id` 落空 ——
真实登录响应的 `userInfo` 里没有 `userId`，需要用它的接口得另找来源"*。
App 日志给出了答案，**三个名字是同一个值**（下面用合成值 `1990000000000000001`
示意形状，真值不落仓库）：

| 出现位置 | 形态 | 本窗口次数 |
|---|---|---|
| 请求头 `x-yun-uni` | `1990000000000000001` | 5839 |
| 请求体 `"userDomainId"`（`IOutLink/queryRecentFriendLinkList`）| 同上 | 11982 |
| 请求体 `"userId"`（`general/getStatus`、`getRecommendList` 等）| 同上 | 38 |

结论：

- **`userId` 与 `userDomainId` 是同一个 19 位数字的两个名字**，不是两套 id。
  注意区别在**位置**而不在端：`userId` 是**请求体**里常见的键名，`userDomainId` 是
  **响应载荷**里的字段名（App 实测如此，PC 源码也指向同一个名字 —— 见下）；
- **任何一条已登录请求的头里就带着它**（`x-yun-uni`），
  所以"要用 `userDomainId` 的接口"**不缺来源** —— 不需要为此再登录一次；
- **首次登录时"拿不到"只对握手期间的请求成立** —— 那时确实还没有这个值，
  编一个会让上游定位到别人的用户域。但 **`thirdlogin` 一返回它就有了**
  （就在响应里，字段名 `userDomainId`），并且已经落进库。
  原始观察与真机收口记在
  [`upstream-app-sms-login.md`](./upstream-app-sms-login.md) §7.4；
- 这也是**唯一**一处 `check-identifiers.cjs` 的 19 位规则会命中的真实值形态，
  所以从日志导出的 HAR **只落 `har/`**（已 gitignore），脱敏交给 `mcloud fixture extract`。

**PC 端呢？—— 已对齐（2026-09-19 实测）**：PC 客户端源码里 `UserInfo.userDomainId`
被读了 11 次、`UserInfo.userId` **一次都没有**，而 `userId` 只出现在**请求体**里、
由 `userDomainId` 赋过去。随后一次真机 App 登录打印出的响应字段清单
（`account, …, userDomainId, userExtInfo, userid`）**证实了这个推断**：
响应里是 `userDomainId`，`userId` 根本不在。因为两端**都调 `POST /user/thirdlogin`**，
所以这条字段名结论对 PC 与 App 同时成立。
唯一仍未对齐的是**取值方式**：PC 客户端**不发** `x-yun-uni` 头（App 独有），
所以"App 靠这个头拿值"那句话不能搬给 PC —— 但 PC 从登录响应里也能拿到同样的值。

⚠ **别把 `userid`（全小写）混进来**：它在同一份登录响应里与 `userDomainId`
**同时出现、值不同**（`userid` 12 字符、`userDomainId` 19 位），是另一个东西。
同理，PC 源码里 `fromUserInfo.userid` 读的是本地镜像，来源是原生账号记录
`userData.userid`，也不是登录响应字段。


业务常量（请求体里的公共参数）与 PC 端的对照：

| 参数 | PC（`4.2.1` / win `8.8.6.20260829`） | **iOS App** |
|---|---|---|
| `pintype` | `21` / `5` / `9` | **`8`** |
| `clienttype` | `656` / `710` | **`412`**（注意是**小写 `t`**） |
| `cpid` | `277` / `332` | **`55`** |
| `version` | `8.8.6.20260829` / `4.2.1` | **`12.6.0`** |
| 请求体形态 | 明文 JSON | **AES-ECB + 末 10 位前移**（另一套，见 §6） |

另外三个 App 端独有的**字段类型**差异（同样是实测，不是猜的）：
`extInfo.ifOpenAccount` 是**字符串** `"1"`（PC 端是数字 `1`）；
`getSmsCode` 的 `mode` / `reqType` 也是**字符串**。
App 端对"字符串数字"的容忍度看起来比 PC 端高，但**不要**据此把 PC 端的字段也改成字符串 ——
两端各按各的实测来。

> **代码位置**：`mcloud-upstream::app_client`（`AppDevice` / `AppProfile`）。
> 该模块**故意不提供 `Android` 变体**：安卓端的 `clientType` / `cpid` / 设备串语法都没有实测，
> 先造一个只会让"传错端"变成一个可以静默通过的选项。

---

## 4. PC 端的头是代码级证据（证据 B）

`common/utils.js: sendRequest` 里按 `window.UserInfo.cutoverStatus` 分成两套：

**公共（L226）**：`Content-Type`

**登录后且有 authToken 时（L229-237）**：

```
Authorization: Basic base64("pc:" + account + ":" + authToken)
APP_AUTH:      同 Authorization（笔记模块用）
APP_CP:        pc
CP_VERSION:    <appVersion>
```

**`cutoverStatus == 1`（L239-267）**：

```
x-yun-app-channel:   macOS ? 10301000 : 10200153
x-yun-client-info:   windowDeviceInfo
x-DeviceInfo:        windowDeviceInfo      ← 注意大小写，两个头共存
x-yun-device-id:     windowMachine
x-yun-market-source: appSource 或 "000"
x-yun-module-type:   100（默认）/ 101（仅 /hcy/task/get、/hcy/file/batchMove 且有 sessionid）
x-yun-op-type:       1
x-yun-svc-type:      1
x-ExpRoute-Code:     routeCode=<account>,type=2   （有账号时）
x-yun-api-version:   v1（默认）/ v2（仅 firstApplySecMail）
```

**否则（L268-282）**：

```
x-yun-client-info:   windowDeviceInfo
x-DeviceInfo:        windowDeviceInfo
x-MM-Source:         appSource 或 "000"
x-huawei-channelSrc: 10200153
x-UserAgent:         PC
sec-ch-ua-platform:  （空串）
```

**另有 `options.appendHeads`（L283-288）**：调用方可追加私有头 ——
说明"同一端不同接口还可以有不同的头"，profile 需要支持**按接口叠加**，不能只做一层端常量。

三个易错点：`x-DeviceInfo` 与 `x-yun-client-info` 是**两个头、同一个值**；
`cutoverStatus` 决定整套头；`x-yun-app-channel` **mac 与 win 不同值**。

---

## 5. 设备指纹（证据 C）

`apiServer.js: getClientDeviceInfo()` 的两条路径：

```
1) 原生接口成功： nativeAbility.common.getDeviceInfo() → { machine, deviceinfo }
2) 兜底（无原生能力，如纯浏览器）：
     windowMachine    = getGUID32() + '-ENDIN'
     windowDeviceInfo = "||11|<appVersion>|PC|<base64 型号>|<windowMachine>||<OS>|<分辨率>|<base64 语言>|||"
```

`getGUID32()` = `MD5(随机 UUID).toString(Hex).toUpperCase()` → 32 位大写十六进制。

### 5.1 `deviceId`（`windowMachine`）形状

**32 位大写 HEX + `-ENDIN` 后缀**。兜底路径说明它是"按安装随机生成"而非硬件派生，
所以**我们完全可以自己生成**（`Random` → 16 字节 → 大写 hex）。

### 5.2 `deviceInfo` 字段表（**PC 端**，14 段，`|` 分隔，下标 0 起）

| 下标 | 内容 | 来源 |
|---|---|---|
| 0, 1 | 空串（前导 `||`） | 固定 |
| 2 | `11` | 固定（疑似系统/环境枚举） |
| 3 | 客户端版本 | `appVersion` |
| 4 | `PC` | 端标识 |
| 5 | **base64(设备型号码)** | 兜底模板里是固定常量 `BF-202003101647` |
| 6 | **deviceId** | `windowMachine`（与 `x-yun-device-id` 同值） |
| 7 | 空串 | — |
| 8 | OS 描述 | 原生接口返回，如 ` Windows 11 (10.0.26100.1591)`（**前导空格**） |
| 9 | 分辨率 | 如 `1920X1020` / `1920X1040` |
| 10 | **base64(语言)** | 兜底模板里是 `Chinese (Simplified)` |
| 11-13 | 空串（尾部 `|||`） | 固定 |

> 第 5 / 10 段是 base64 包一层，第 6 段是明文 —— 同一串里两种编码混用，抄的时候极易看错。

### 5.3 App 端设备串字段表（**已实测**，证据 E）

App 端**也是 14 段**，但**段的含义与 PC 端完全不同** —— 别拿 PC 的表去套。
实测原串（脱敏）：

```text
x-DeviceInfo      (14 段)
1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|<uuid32>|<short12>|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|

x-yun-client-info (16 段)  = 上面那串 + "aVBhZA==|0|;"
1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|<uuid32>|<short12>|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|aVBhZA==|0|;
```

| 下标 | 内容 | 实测值 / 来源 | 与 PC 端的差异 |
|---|---|---|---|
| 0 | 常量 | `1` | PC 端是**空串** |
| 1 | 客户端自报的局域网 IP | `192.168.3.179` | PC 端是空串；服务端不据此鉴权 |
| 2 | 常量 | `2` | PC 端是 `11` |
| 3 | 客户端版本 | `12.6.0` | PC 端同位置也是版本，一致 |
| 4 | 端标识 | `APPLE` | PC 端是 `PC` |
| 5 | **机型（明文）** | `iPad8,6` | PC 端这一段是 **base64** |
| 6 | **deviceId** | 32 位**大写** hex | 同 PC 端的 `windowMachine`（形状一致） |
| 7 | 设备短标识 | 12 位大写 hex | **PC 端这一段是空串**；**它按运行变，不是每安装稳定值**（§5.4） |
| 8 | OS 描述 | `ipados 26.6` | PC 端形如 ` Windows 11 (10.0.26100.1591)` |
| 9 | 分辨率 | `375 * 667`（**带空格，别 trim**） | PC 端形如 `1920X1020` |
| 10 | 语言 | `zh-Hans-CN`（**明文**） | PC 端这一段是 **base64** |
| 11, 12 | 常量 | `0.0000` / `0.0000` | PC 端是空串 |
| 13 | 空串（原串以 `|` 结尾） | — | 同 PC 端 |
| 14 | `x-DeviceName` 的 base64 | `aVBhZA==` = `iPad` | **仅 `x-yun-client-info` 有** |
| 15 | 常量 `0`，随后 `;` 收尾 | — | **仅 `x-yun-client-info` 有** |

三个最容易抄错的点：

1. **`x-DeviceInfo` 与 `x-yun-client-info` 不是同一个值**（14 段 vs 16 段）。
   PC 端那两个头是"同值双名"（§4），App 端**不是** —— 这是两端差异里最隐蔽的一处。
2. 第 5 / 10 段的编码方向**与 PC 端正好相反**：App 是机型明文 + 语言明文，
   PC 是机型 base64 + 语言 base64。
3. 同一个系统版本在**三个位置三种写法**（实测原样）：

   | 位置 | 写法 |
   |---|---|
   | 设备串第 8 段 | `ipados 26.6`（iPad 上是小写 `ipados`） |
   | `x-UserAgent` / `x-yun-user-agent` | `ios 26.6` |
   | `User-Agent` 括号内 | `iOS 26.6` |

   不要"顺手归一化"成一个写法 —— 上游是按正则解析的。
   代码里保留 `os_version` 一个字段 + 三段拼接（`AppDevice::{device_info, x_user_agent, yun_user_agent}`）。

> **代码位置**：`mcloud-upstream::app_client::AppDevice`。
> `device_id` / `device_short` 由调用方注入（生命周期不同，见 §5.4），
> 其余字段有实测默认值（`AppDevice::ipad`）。

### 5.4 持久化要求（**两端不一样**）

**PC**：`windowDeviceInfo` / `windowMachine` 存在 `localStorage`，
**必须在首次运行后持久化且永不漂移**：换了 deviceId 等于换了台设备，
服务端会按 `dID + sID` 联合定位会话（见 `docs/upstream-login-protocol.md` §4 的
`200059541 未找到设备的二维码信息`）。

**App**：只有第 6 段（`deviceId`）持久化，第 7 段（`deviceShort`）**不持久化**。

| 段 | 生命周期 | 实测依据 |
|---|---|---|
| 6 `deviceId` | **持久化，跨月不变** | 同一台设备在 2026-08-12 与 2026-09-18 都是 `67F4C56A…315C` |
| 7 `deviceShort` | **每次运行现生成** | 同一个 `deviceId`，08-12 是 `8A708867DBC3`、09-18 变成 `EAB9BF94B8DE` |

> 把 `deviceShort` 也持久化，等于**声称一个已被证伪的稳定性** ——
> 所以实现上刻意只存 `deviceId`（`mcloud_app::device::load_or_create_app_device_id`），
> short 每次运行用 `new_app_device_short` 现生成。
>
> **待确认**：它到底按什么变（每次启动 / 每天 / 某个事件）？
> 目前只有两个观测点，定不下来 —— 记在 §7。

**③ 上面两段讲的是设备串（`deviceInfo`）里的字段。还有第三种指纹**，
与它**完全不同源、不同形状、不同生命周期**：cMPassport 风控侧的 `smid`。

（2026-09-19 追加，实现见 `crates/mcloud-upstream/src/smid.rs` 与 `crates/mcloud-app/src/smid.rs`。）

它产出两样东西，**绑账号的是前者、会变的后者**：

| | 设备特征（我们生成） | `deviceId`（服务端下发） |
|---|---|---|
| 内容 | ~31 个字段的浏览器/设备特征体 + 那 25 条 DES 改名表 | 18 字符，`B` + 17 位 |
| 从哪来 | 客户端本地 `randomUUID()` → RSA 上送 `uid` → 组特征体 → `tn = md5(getTn(body))` → DES 映射 → gzip(`buf[9]=19`) → base64 → AES-CBC 定 IV → hex 成 `data` | `POST https://slw.h5cmpassport.com:9090/deviceprofile/v4`，`code == 1100` 时取 `"B" + detail.deviceId` |
| 生命周期 | **绑到账号**：一次生成、长期复用、可**主动更换** | **每次上报都可能换**；下一次的 `box` 字段要回喂上一次的这个值 |
| 落在哪 | `account_state.smid_fingerprint` | `account_state.smid_device_id` |

**为什么特征绑账号、上报值不绑**（与上面 ② 那条 `deviceShort` 是同类的判据）：
`deviceId` 是**服务端**决定的，我们若把它当稳定值持久化，就是在声称一个不由我们控制的稳定性；
而"换特征"必然换 `deviceId`，所以 `rotate` 那一步**同一条语句里清空** `smid_device_id`
（旧值对新特征无意义，留着只会让人以为它还能回喂）。

这与旧项目**刻意不同**：旧 TS 每次启动现算、失败就吞成空串
（`packages/shared/src/smid/index.ts:370-397` 的 `catch { … } return "";`），
于是每台机器每次都是一个新设备。新实现把"生成"和"上报"分开，
生成一次就绑住，换不换由用户决定（CLI：`mcloud smid show / reset / refresh`）。

> **`deviceid` 请求头：值确认是它，注入点未确认**
>
> 旧项目确实把 `B…` 这个值原样发成请求头，代码级证据三条：
> `packages/shared/src/smid/index.ts:370-397`（`getDeviceId()` → `"B" + json.detail.deviceId`）、
> `packages/core/src/utils/init-account.ts:33-40`（拿到后 `createHttp({ deviceid: deviceId })`）、
> `packages/shared/src/http/base.ts:44-45 / 92-95`（`deviceid` 全请求注入）与 `:166-181`
> （`createTaskHttp` 把 `deviceid` 写进静态头）。
>
> **但我们自己不照做**，因为注入范围没有证据：
> ① App 签到那批抓包里原生请求头 `deviceid` **一次都没出现**
> （`capture/app-signin-samples.txt` §4 那张两列对照表）；
> ② 把夹具（`test/fixtures/http/` 本次共 711 条）按"哪些请求真带 `deviceid`"过一遍，
> 带它的 178 条**全是 H5 / WebView**
> （iPad UA、Android `wv`、活动页 iPhone UA），没有一条是原生 App 的流量。
> 所以 `AppProfile::headers()` 保持**不发** `deviceid`，
> 等某一条**原生**抓包明确带上了，再由那个接口的构造函数按端点显式带上 ——
> 而不是往公共头里塞一个只来自旧项目、且我们从未测到的值。

---

## 6. 落地时的目标形状（三层；PC / iOS App 已落地，其余端未实现）

| 层 | 内容 | 生命周期 | 已落地的例子 |
|---|---|---|---|
| **每端常量** | 端标识、版本号、渠道号、`clientType`/`cpid`、UA 模板、`deviceInfo` 模板 | 编译期 | `PcPlatform::{client_type, cpid, app_version}`；`app_client::ios::{CLIENT_TYPE, CPID, APP_VERSION, CHANNEL}` |
| **每安装指纹** | `deviceId`、`deviceInfo`、听云 id | 首次生成 → 持久化，不可漂移 | `windowMachine`；`AppDevice::device_id` |
| **每运行** | App 设备串第 7 段（`deviceShort`） | 每次运行生成，**不落盘** | `mcloud_app::device::new_app_device_short` |
| **每请求** | `x-yun-tid`、`Authorization`/`jwttoken`、`timestamp`/`nonce`/`signature`、`msgid` | 每次计算 | 登录链路的 `x-yun-tid`（PC）；`x-yun-user-timestamp`（App，由调用方传 `timestamp` 注入） |
| **每账号**（2026-09-19 追加） | smid 的**设备特征**（~31 字段，见 §5.4 ③） | 首次生成 → 落 `account_state.smid_fingerprint`，**可主动换** | `mcloud_app::smid::{ensure_fingerprint, rotate_fingerprint}` |
| **每上报**（2026-09-19 追加） | smid 的 `deviceId`（`B…`，服务端下发） | 每次上报都可能换；旧值经 `box` 回喂 | `mcloud_app::smid::device_id`（库里有就不再打网络） |

> 这两行**不是**"每端 / 每安装 / 每运行 / 每请求"里某一行的别名，而是**另一棵指纹树**：
> 上面那张表按"一台设备的设备串"分层，这两行按"一个账号绑一份风控特征"分层。
> 混起来会犯两类错 —— 把 smid 的 `deviceId` 当每安装常量持久化（它每次上报都在变），
> 或者反过来把它每次现生成（那正是旧 TS 的行为，等于每请求换一台设备）。

再加一层**按接口叠加**（对应 `options.appendHeads`）：`x-yun-module-type`、
`x-yun-api-version`、`x-ExpRoute-Code` 都随接口变化。

> **还有一层比"按接口"更细：按调用点。**
> `toSourceId` / `channelSrc` / `sourceChannel` 这类"码"不是端属性，是**调用方按场景选的参数** ——
> 同一个 PC 客户端文件里同一条接口就有三个值（`001003` / `001005` / `001248`），
> 旧项目为拿不同任务会轮流冒充 PC / 安卓 / iPhone / Edge-on-Linux。
> **完整取值谱、分类判据与"哪些码会整套换头"见
> [`device-and-source-codes.md`](./device-and-source-codes.md)** ——
> 接新接口前先看它，别把某端的默认值当常量抄过来。

> **App 端的实现方式与 PC 端有意不同**：`AppProfile::headers(timestamp)` 是**纯函数**，
> 时间戳由参数传入而不是内部读时钟 —— 这样请求头可以**逐字段断言**（L2 夹具回放的要求），
> 也让"同一时刻的两个请求必须同头"变成可测的性质。PC 端后续改造时可对齐这个形状。
>
> 两端**还没有**抽出共同的 trait。现在抽是过早的：PC 与 App 除了"都叫 profile"之外，
> 头的名字、值的语法、加密方式全不一样，共同点只有 `Vec<(String, String)>` 这个返回类型。
> 等第三端（安卓或邮箱端）进来、能看出真正的公共部分时再抽。

trait 形状（**草案；App 端落地时有意没照它做**，理由见上一段）：

```rust
// 仅示意分层，不是最终签名
pub trait ClientProfile {
    fn kind(&self) -> ClientKind;                       // 每端
    fn base_headers(&self, fp: &DeviceFingerprint) -> Vec<(String, String)>;   // 每端 + 每安装
    fn request_headers(&self, ctx: &RequestCtx) -> Vec<(String, String)>;      // 每请求 + 按接口叠加
}
```

---

## 7. 未知项（**不猜，等证据**）

**已由 iOS App 实测回答、从本表移除的**（留一行索引，便于回查）：
iOS 的 `clientType`=`412` / `cpid`=`55` / 渠道号=`10000020` / `version`=`12.6.0`、
`x-yun-uni` 的语义、iOS 的 `deviceInfo` 字段表、App 的 `Authorization` 前缀（`mobile`）
→ 见 §3.3 / §5.3。

仍未回答的：

- **安卓端**的 `clientType`、`cpid`、渠道号、设备串语法（`Dalvik` 那组夹具只够统计头名，不够定字段）；
- **微信小程序端**同上；`mop-sdk-*` 那组的具体载体（快应用 / 小程序 / 内嵌 H5）也未确认；
- `x-deviceinfo`（小写 d）与 `x-DeviceInfo` 的关系：前者跨端出现，后者在 PC 源码与 iOS App 实测里都有，
  但**两者是否同值仍未验证**（PC 端源码里那两个是 `x-DeviceInfo` + `x-yun-client-info`，没有小写那个）；
- 各端签名算法（`signature` / `signtoken` / `mcloud-sign` / `x-signature` 是四套不同的东西）。
  ~~App 的 `signature`（64 位小写 hex）与 `msgId` 的算法**都没解**~~
  → **已解开（2026-09-19）**：`signature = SHA256("clientId=iosapp&clientKey=…&msgId=…
  &timeStamp=…&version=…")`，6165/6165 条实拍逐字命中；见
  [`upstream-app-business-api.md`](./upstream-app-business-api.md)；
- **安卓 / 小程序端 UA 完整串**（PC 与 iOS App 的已实测拿到，见 §3.3）；
- **App 设备串第 7 段（`deviceShort`）按什么变**：已知它**不是**每安装稳定值（§5.4），
  但只有两个观测点，定不出规则；
- ~~业务接口请求体里的 `encodeData` 用什么算法/密钥~~
  → **已解开（2026-09-19）**：`base64(16 字符 ASCII 的 IV ‖ AES-128-CBC/PKCS7(key, iv, 手机号))`，
  6159/6159 个样本全部解出、明文去重后只有 1 种长度 11（即手机号）；见
  [`upstream-app-business-api.md`](./upstream-app-business-api.md)；
- ~~**`hcy-cool-flag` 触发的那条加密响应用什么密钥**（App 端 `getSmsCode` 的坑，
  详见 `docs/upstream-app-sms-login.md` §7.1）—— 这是登录链路上**唯一**没解开的加密点。
  ⚠ 注意它与上面那个 `encodeData` **不是同一回事**（方向相反：一个是请求、一个是响应），
  **别拿 `encodeData` 的密钥去试它**。~~
  → **已解开（2026-09-19 深夜）**：密钥 **`uDjGSA8auFlbwDkY`**，格式
  `base64(IV(16 字节，**二进制**) ‖ AES-128-CBC/PKCS7(key, iv=IV, JSON))`。
  ⚠ 但**"响应加密"这个理解本身是错的** —— 这个头声明的其实是
  **"请求体已加密"**（响应加密只是同一选项的另一半）：
  服务端收到它就把 body 当 base64 解再 AES 解，我们发明文 body 时它回
  `01000019 Illegal base64 character 7b`（`7b` = `{`）。
  把请求体按同格式加密发出去，服务端就正常处理并回 `200059554` ⇒ 已用活体探针走通。
  证据见 [`capture/app-sms-login-samples.txt`](./capture/app-sms-login-samples.txt) §1.2。
  **至此登录链路上的加密点全部解开。**
- **`deviceid` 头在原生 App 上到底哪些端点带**（2026-09-19 追加）：**值**已确认就是 smid 那套
  （`B` + 17 位，§5.4 ③），**注入范围**没有一条实测 —— 现有夹具里带它的 178 条全是 H5 / WebView，
  原生流量一条都没有。所以 `AppProfile` 现在**不发**它；
  接第一批已登录的原生接口时按抓包**逐端点**回填，不要先塞进公共头。
- **原生侧 smid 特征是什么形状**（同期追加）：`SmFingerprint::caiyun_android_web` 那份是
  **web/H5 特征**（`os: "web"`、`platform: "Linux armv8l"`、Chrome 插件表），照旧项目逐字沿用的。
  iOS 原生 App 若走同一套风控，它的特征体（含 `ua`、`plugins`）我们没有观测过，
  **不拿 H5 那份冒充原生** —— 那等于凭空发明一份特征。

以上每一条，**接该端时用真实流量回填**，不要从推断反推。

### 7.1 一条**运维级**的坑：App 日志是滚动覆盖的，而且窗口**不到 2 小时**

App 的 `FXTrace*.log` **每个文件封顶 ~10 MB，只保留最近 5 个**。
09-18 挖到的那份 `FXTrace2026081208260217.log`（含 08-12 起一个多月的流量）
在几小时后就被新文件挤掉了。

**实测的保留窗口比"几小时"紧得多**：09-19 一份完整快照里 5 个文件共 43 MB，
覆盖 **06:26:06 → 08:14:52，即 1 小时 48 分**。原因不是 App 忙，而是它在**空转轮询**：

| 接口 | 106 分钟内次数 |
|---|---|
| `middle.yun.139.com/openapi/toolBox/load/recommend` | 5928 |
| `share-kd-njs.yun.139.com/yun-share/.../queryRecentFriendLinkList` | 5769 |

两个接口合计 **12097 条配对记录里的 99%**，真正有用的业务流量只剩 103 条。

**所以：抓到之后立刻 `cp` 出来**（`/tmp` 或 `har/`，注意后者已 gitignore），
不要指望回头再读。本次能挖到 §3.3.1 那批证据，纯粹是因为当时顺手扫了一遍。

工具已就位（`scripts/applog-capture.py`），把上面两件事都兜住了：

```bash
scripts/applog-capture.py list                  # 文件、体积、时间跨度（含窗口告警）
scripts/applog-capture.py snapshot /tmp/fxlog   # 抢一份出来，防滚动
scripts/applog-capture.py summary               # 接口分布（默认剔掉那两个轮询）
scripts/applog-capture.py dump 领云朵            # 完整请求 + 响应（凭据头默认打码）
scripts/applog-capture.py har har/x.har --path /user/
cargo run -p mcloud-cli -- fixture extract --har har/x.har --out test/fixtures/http
```

**日志比代理抓包更强**：它有 `*************Request Start***************` 块，
里面是 `API Name` / `Method` / `Service` / **`HTTP Header`（整块 dump）** / `HTTP Body`，
配对上 `=====================API Response==================` 块（`Status` / `Duration` / `Tid` /
`Raw Response String`）就能拼出**完整** HAR —— 方法、头、请求体、状态、响应体全有，
不需要 Reqable、不需要装 CA。两条通路互补：代理能看时序与连接，日志能看头与明文信封。

> **另一条实测的坑**：日志里大量记录 `Status: 0` + 耗时恰好 `30000 ms`，
> 那是**没拿到响应**（超时），不是 HTTP 0。`applog-capture.py summary` 的
> 状态列会显示成 `0/200` 这种混合形态，别当成接口返回了 0。


---

## 8. 与现有代码的关系

- `mcloud-upstream::login`（PC 链路）**只发 `x-yun-tid`**（证据 D 是该链路唯一实测到的头）。
  不要往里摊 PC 的那一堆头 —— 否则别的端接进来时会被迫改登录逻辑。
- **App 链路的头由 `mcloud-upstream::app_client::AppProfile` 提供**，
  请求构造函数（`app_login::send_sms_code_request` / `thirdlogin_request`）通过
  `with_app_headers(request, profile, timestamp)` 叠加，
  这样"换端"= 换 profile，不动业务逻辑。**登录链路的两个函数没有硬编码任何头值。**
- `PcPlatform::{client_type, cpid, app_version}` 已经是"每端常量"的雏形；
  App 端照同样思路落成了 `app_client::ios`。第三端进来时再考虑合并成一张表。
- **smid 那条线（§5.4 ③）目前不挂在任何请求链路上**，它是独立的一层，
  等哪个原生接口要用 `deviceid` 时再接上去：

  | 层 | 位置 | 职责 |
  |---|---|---|
  | 算法 | `mcloud-upstream::smid` | 特征体 → `data` → 上报 → `B…`；**只管一次调用，不碰库** |
  | 生命周期 | `mcloud-app::smid` | 读库里的特征、没有就生成并绑账号、上报、存 `deviceId`；`rotate` 时同一条语句清掉旧 `deviceId` |
  | 落库 | `account_state.smid_fingerprint` / `smid_device_id`（schema v5） | 见 §6 那两行新层 |
  | 入口 | `mcloud smid show / reset [--report] / refresh` | 只打印摘要（特征 fp 摘要 + 平台/尺寸/dpr/触屏/CPU + 打码的 `deviceId`），**不打印 UA** |

端档案的调用链（**从 CLI 一路到上游**，可以直接照着读）：

| 层 | PC | App |
|---|---|---|
| 端常量 | `mcloud-upstream::login::PcPlatform` | `mcloud-upstream::app_client::ios` |
| 端档案 | `mcloud-upstream::client::ClientProfile` | `mcloud-upstream::app_client::AppProfile` |
| 请求构造 | `mcloud-upstream::login::*` | `mcloud-upstream::app_login::*` |
| 驱动 | `mcloud-app::login::{sms_send_code, sms_login}` | `mcloud-app::app_login::{app_send_sms_code, app_sms_login}` |
| 落库 | `persist_login`（前缀 `pc`） | `persist_login_with_prefix`（前缀 `mobile`） |
| 入口 | `mcloud login sms --client pc` | `mcloud login sms --client app` |
