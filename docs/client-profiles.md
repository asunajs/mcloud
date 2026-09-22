# 各端差异与请求头（§1 结论 · §2 证据来源 · §3 实测头对照 · §4 PC 代码级证据）

> 状态：PC 端已落地为 `upstream.ClientProfile`（`internal/upstream/client.go`）；
> **iOS App 端已落地为 `internal/upstream/app_client.go`（2026-09-18 实测，见 §3.3 / §5.3）**；
> 安卓 / 邮箱端 / 微信小程序端**仍未实现**（§7）。
>
> 记这条的直接原因：P2.5 做登录时发现 Windows 客户端版本是 `8.8.6.20260829`、
> 本机 macOS 客户端是 `4.2.1`，于是"版本号到底该填什么"变成一个必须回答的问题 ——
> 而答案是"看你冒充哪个端"。后期还要接安卓、iOS、邮箱端、微信小程序端，
> 每端的版本号、UA、认证方式都不一样。
>
> 本文钉住三件事：**差异在哪、证据在哪、哪些还不知道**。
> 依据 `../mcloud-rust/docs/rust-rewrite-plan.md` §7.0 的硬约束：**不写任何猜测性的头值或常量** ——
> 所以下面凡是"已实测"的行，都能追到一份真实流量；凡是推断的，都显式标了"未确认"。

> **本篇 2026-09-22 拆成两篇**（内容逐字未改，只按主题分段）：
> [① 端差异与请求头（§1 ~ §4）](./client-profiles.md) ｜ [② 设备指纹 · 落地形状 · 未知项 · 与代码的对应（§5 ~ §8）](./client-profiles-fingerprint-and-landing.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§1 ~ §8 连续），所以「§5.2」「§7」这类引用
> 仍然有效，只是落在不同的文件里。

---

## 1. 结论：头差异最大，体也不是"只差三个参数"

- **请求头差异很大**：认证头、设备指纹头、渠道头、埋点头、签名头，**每端一套**（§3 有实测对照）。
- **请求体差异比原先估计的大**：原先以为登录链路只随端变 `clientType` / `cpid` / `version` 三个公共参数
  （见 `docs/upstream/login-protocol.md` §6），但 iOS App 实测推翻了这一点：
  它还多了 `pintype=8`、`extInfo.ifOpenAccount` 由数字变**字符串**、
  而且**整个请求体是加密的**（PC 是明文 JSON）。
  也就是说"请求体差异很小"这个判断**只对 PC 内部的 win/mac 之间成立，跨端不成立**。

对实现的影响（这一条才是本文存在的意义）：

1. 新写接口时**不要把某一端的头硬编码进 `internal/upstream`**，头应由"端档案（client profile）"提供；
2. 端档案落地前，新接口**只发该接口已被实测确认的头**；
   落地后（PC：`upstream.ClientProfile`；iOS App：`upstream.AppProfile`）**一律从 profile 取头**，
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
| D | `docs/capture/*` + [`docs/upstream/`](./upstream/) 那一族 | 登录链路实测（该链路只出现 `x-yun-tid`） |
| **E** | **`docs/capture/app-sms-login-samples.txt`**（iOS App 自身 FXTrace 网络日志） | **iOS App 端的头集合、设备串、业务常量（§3.3 / §5.3），且是逐字节可复算的** |

A 是**多端**证据（UA 分组天然按端分开），B/C 是 **PC 单端**的代码级证据，
E 是 **iOS App 单端**的证据 —— 它比 A 强，因为 A 只统计"发过哪些头"，
E 还给了"头里每个字段长什么样"，并且能用二进制反汇编交叉验证（见 `docs/upstream/app-sms-login.md` §2）。

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

2026-09-18 实现扫码登录时撞上的（已写进 `docs/upstream/login-protocol.md` §3 的补正）：

| 请求头 | `/user/thirdlogin` 响应 |
|---|---|
| 只有 `x-yun-tid` | `{"code":"9999","message":"登录失败！"}` |
| + `x-yun-client-info` / `x-DeviceInfo` / `x-MM-Source` / `x-huawei-channelSrc` / `x-UserAgent` | `{"code":"200059541","message":"未找到设备的二维码信息"}` ✓ |

也就是说**"请求头不对"的失败长得像"登录被拒"**（`9999` 那个码在协议文档的任何错误码表里都没有），
而不是像"参数不合法"。这一条把"头必须逐端对齐"从设计原则变成了**排查经验**：
以后任何接口返回莫名其妙的业务码，第一个该试的是"把该端的头补齐"。

PC 端这套头已落地为 `upstream.ClientProfile`（含设备信息串的构造 `upstream.DeviceInfo`）。

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
它排除了"App 端走国密会话密钥"的猜测，实际用的是另一套（见 `docs/upstream/app-sms-login.md` §2）。

#### 3.3.1 已登录请求：`Authorization` 与头超集（同源证据）

`Authorization` 在**登录链路**上不出现，但同一份日志里**已登录**的请求有 1076 处，
形态是 `Basic base64("mobile:<手机号>:<authToken>")`：

| | PC | App |
|---|---|---|
| `Authorization` 载荷第一段 | **`pc`** | **`mobile`** |
| `authToken` 形状 | `前缀\|1\|RCS\|13 位毫秒\|载荷` | **完全一样** |
| 未登录时的占位 | 不带这个头 | `Basic base64("mobile:(null):(null)")` |

也就是说**只有第一段的字面量不同** —— 所以实现侧把它做成按端给的参数
（`upstream.IOSAuthPrefix` + `upstream.AppAuthorization`），
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
| `signature` | 64 位小写 hex | **B** | = `SHA256("clientId=iosapp&clientKey=…&msgId=…&timeStamp=…&version=…")` —— 见 [`app-business-api.md`](./upstream/app-business-api.md) |
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

`../mcloud-rust/docs/rust-rewrite-plan.md` §7.2 记着一个缺口：*"`user_domain_id` 落空 ——
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
  [`app-sms-login.md`](./upstream/app-sms-login.md) §7.4；
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

> **代码位置**：`internal/upstream/app_client.go`（`upstream.AppDevice` / `upstream.AppProfile`）。
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
