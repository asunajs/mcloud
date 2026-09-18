# 各端差异与设备指纹（**已记录，待实现**）

> 状态：**只记录，未实现**。
>
> 记这条的直接原因：P2.5 做登录时发现 Windows 客户端版本是 `8.8.6.20260829`、
> 本机 macOS 客户端是 `4.2.1`，于是"版本号到底该填什么"变成一个必须回答的问题 ——
> 而答案是"看你冒充哪个端"。后期还要接安卓、iOS、邮箱端、微信小程序端，
> 每端的版本号、UA、认证方式都不一样。
>
> 本文只钉住三件事：**差异在哪、证据在哪、哪些还不知道**。
> 依据 `docs/rust-rewrite-plan.md` §7.0 的硬约束：**不写任何猜测性的头值或常量**。

---

## 1. 结论：差异主要在**请求头**，不在请求体

- **请求体差异很小**：登录链路里随端变化的只有 `clientType` / `cpid` / `version` 三个公共参数
  （见 `docs/upstream-login-protocol.md` §6）。
- **请求头差异很大**：认证头、设备指纹头、渠道头、埋点头、签名头，**每端一套**（§3 有实测对照）。

对实现的影响（这一条才是本文存在的意义）：

1. 新写接口时**不要把某一端的头硬编码进 `mcloud-upstream`**，头应由"端档案（client profile）"提供；
2. 端档案落地前，新接口**只发该接口已被实测确认的头**（登录链路目前就只有一个 `x-yun-tid`）；
3. 设备指纹（`deviceId` / `deviceInfo`）是**每安装**一次生成、永久持久化的，不能每次请求现编。

---

## 2. 证据来源

| 编号 | 来源 | 能证明什么 |
|---|---|---|
| A | `test/fixtures/http/`（30 份夹具 / 709 条真实请求） | 各端真实发过哪些头（§3 按 UA 分组统计） |
| B | PC 客户端 `common/utils.js: sendRequest`（L226-289） | PC 端每个头的构造逻辑与条件分支（§4） |
| C | PC 客户端 `apiServer.js: getClientDeviceInfo`（L613-635） | 设备指纹怎么生成、怎么持久化（§5） |
| D | `docs/capture/*` + `docs/upstream-*.md` | 登录链路实测（该链路只出现 `x-yun-tid`） |

A 是**多端**证据（UA 分组天然按端分开），B/C 是 **PC 单端**的代码级证据。

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
  不能据此断定是"小程序"还是"App"；
- `mop-sdk-*` 疑似移动 MOP 框架（具体载体：快应用 / 小程序 / 内嵌 H5，**未确认**）；
- `139PushMail_inhouse` 是**邮箱端**（用户已提到要接邮箱端，这条是现成证据）；
- `中国移动云盘/<版本> CFNetwork` 是 **iOS 原生**（`CFNetwork` + 版本号形如 `04241958`）；
- `Mozilla/5.0 (Macintosh…)` 那组带 `sec-ch-ua-*`，更像**浏览器/Electron 外壳**而非原生客户端。

**结论**：接某一端之前，先拿该端**真实流量**核对上表，不要把推断当结论。

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

### 5.2 `deviceInfo` 字段表（14 段，`|` 分隔，下标 0 起）

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

### 5.3 持久化要求

`windowDeviceInfo` / `windowMachine` 存在 `localStorage`，
**必须在首次运行后持久化且永不漂移**：换了 deviceId 等于换了台设备，
服务端会按 `dID + sID` 联合定位会话（见 `docs/upstream-login-protocol.md` §4 的
`200059541 未找到设备的二维码信息`）。

---

## 6. 落地时的目标形状（三层，尚未实现）

| 层 | 内容 | 生命周期 | 例子 |
|---|---|---|---|
| **每端常量** | 端标识、版本号、渠道号、`clientType`/`cpid`、UA 模板、`deviceInfo` 模板 | 编译期 | `PcPlatform::{client_type, cpid, app_version}` |
| **每安装指纹** | `deviceId`、`deviceInfo`、听云 id | 首次生成 → 持久化，不可漂移 | `windowMachine` |
| **每请求** | `x-yun-tid`、`Authorization`/`jwttoken`、`timestamp`/`nonce`/`signature`、`msgid` | 每次计算 | 登录链路的 `x-yun-tid` |

再加一层**按接口叠加**（对应 `options.appendHeads`）：`x-yun-module-type`、
`x-yun-api-version`、`x-ExpRoute-Code` 都随接口变化。

trait 形状（草案，落地时再定）：

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

- 安卓 / iOS / 小程序的 `clientType`、`cpid`、渠道号；
- 各端 `deviceInfo` 的字段表（PC 的 14 段已由源码确认，其余端未知，可能是别套格式）；
- `x-deviceinfo`（小写 d）与 `x-DeviceInfo` 的关系：前者跨端出现，后者只在 PC 端源码里见到；
- 各端签名算法（`signature` / `signtoken` / `mcloud-sign` / `x-signature` 是四套不同的东西）；
- 各端 UA 完整串（夹具里已被截断统计，原始串需从抓包取）。

以上每一条，**接该端时用真实流量回填**，不要从推断反推。

---

## 8. 与现有代码的关系

- `mcloud-upstream::login` 目前**只发 `x-yun-tid`**（证据 D 是该链路唯一实测到的头）。
  profile 落地前不要往 login 里摊 PC 的那一堆头 —— 否则别的端接进来时会被迫改登录逻辑。
- `PcPlatform::{client_type, cpid, app_version}` 已经是"每端常量"的雏形，
  届时**并入 profile**，不要再长一份平行的表。
