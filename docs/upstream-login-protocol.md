# 移动云盘 PC 客户端登录协议（真实流量 + 客户端源码双向确认）

> 状态：**已确认**。来源为两条独立证据链的交叉验证，不包含任何推测。
>
> - **证据链 A（真实流量）**：Reqable v3.2.23 抓包归档（1622 个 body 文件）+ 客户端 `log/web.log`（923 次请求全文）
> - **证据链 B（客户端源码）**：`D:\Program Files\mcloud\resources\app.asar` 提取 `/out/renderer/api/apiServer.js`
> - 采集时间：2026-09-17
> - 采集环境：Windows 11 (10.0.26100.1591)，客户端版本 `8.8.6.20260829`
>
> 依据 `docs/rust-rewrite-plan.md` §7.0 硬约束：**本文件内容全部来自实测，未落笔任何猜测性接口**。

---

## 1. 结论摘要

| 项 | 结论 |
|---|---|
| 取码接口 | **不存在**。二维码 URL 由客户端本地拼装，不发起任何网络请求 |
| 扫码登录接口 | 唯一，`POST /user/thirdlogin`，扫码与短信/账密**共用**同一接口 |
| 区分方式 | `pintype` 字段（扫码 `21` / 短信 `5` / 账密 `9` / SIM `?`） |
| 轮询间隔 | 1000ms |
| 是否 WebSocket | 否。纯 HTTP 轮询（`web.log` 中 `thirdlogin` 923 次，无 WS 痕迹） |

**这一条推翻了「取码要调接口」的直觉假设**——这是本次抓包最重要的发现。

---

## 2. 二维码生成（本地，零请求）

来源：`apiServer.js` L669-704 `createLoginQRcode()`。

```js
let deviceId = localStorage.getItem("windowMachine");   // 设备指纹，缺失则调原生接口生成
let sessionId = Utils.getGUID32();                       // 32 位十六进制，即后续的 dycpwd
let QRcodeUrl = "https://yun.139.com/w/#/qrcLogin";
let QRCodePath = QRcodeUrl
  + "?dID=" + deviceId
  + "&sID=" + sessionId
  + "&cType=11&ver=" + window.appVersion;
```

**二维码载荷**
```
https://yun.139.com/w/#/qrcLogin?dID=<32位设备指纹>&sID=<32位sessionId>&cType=11&ver=8.8.6.20260829
```

| 参数 | 含义 | 来源 |
|---|---|---|
| `dID` | 设备指纹，`localStorage["windowMachine"]`，形如 `<32位大写HEX>-ENDIN` | 原生 `nativeAbility.common.getDeviceInfo()` |
| `sID` | 会话 ID，`getGUID32()` 随机生成 | 客户端本地 |
| `cType` | 固定 `11` | 硬编码 |
| `ver` | 客户端版本 | `window.appVersion` |

**关键点**：`sID` 与 `dycpwd` 是**同一个值**。客户端源码注释明确写出：
> `// QRCodeID也是sessionId`

**落地含义**：mcloud 实现扫码登录时，**只需自行生成一个 32 位 HEX 字符串**，拼进 URL 渲染成二维码即可，无需任何「取码接口」。

---

## 3. 扫码登录轮询

来源：`apiServer.js` L596-640 `QRLogin()` + 实测 192 条响应。

### 请求
```
POST https://user-njs.yun.139.com/user/thirdlogin
Content-Type: application/json
```
```json
{
  "clientType": "656",
  "cpid": "277",
  "dycpwd": "<32位大写十六进制，即二维码的 sID>",
  "extInfo": { "ifOpenAccount": 0 },
  "loginMode": "0",
  "msisdn": "",
  "pintype": "21",
  "verType": "2",
  "version": "8.8.6.20260829"
}
```

### 常量取值（`apiServer.js` L11-12）
```js
let clientType = isMacPc ? '710' : '656';   // Windows=656, macOS=710
let cpid       = isMacPc ? '332' : '277';   // Windows=277, macOS=332
```

### 请求头
- `x-yun-tid`：每次请求一个新 UUID（幂等/链路追踪用）

> **补正（2026-09-18，实现后实测）**：本节原先只列了 `x-yun-tid`，**这是漏项**。
> 只带 `x-yun-tid` 调该接口会被上游直接拒绝：
>
> | 请求头 | 响应 |
> |---|---|
> | 只有 `x-yun-tid` | `{"code":"9999","message":"登录失败！"}` ← 不在本文任何错误码表里 |
> | 加上下面五个头 | `{"code":"200059541","message":"未找到设备的二维码信息"}` ✓ 正常态 |
>
> 需要补上的头（客户端 `common/utils.js: sendRequest` 的 **pre-login 分支**，
> 登录时还没有 `UserInfo`，走的正是这一支）：
> `x-yun-client-info`、`x-DeviceInfo`（同一个值、两个头）、`x-MM-Source: "000"`、
> `x-huawei-channelSrc: "10200153"`、`x-UserAgent: "PC"`。
> 其中 `x-yun-client-info` / `x-DeviceInfo` 的值就是设备信息串（见 §7 与
> [`docs/client-profiles.md`](./client-profiles.md) §5.2）。
>
> 实现落点：`mcloud-upstream::client::ClientProfile`，并由单测逐个断言这五个头存在 ——
> 少了任何一个都会退化成 `9999`，而那种失败看起来像"登录被拒"，极难定位。

### 轮询节奏
- 间隔 **1000ms**，由 `setInterval` 驱动
- 登录成功后**必须主动 `clearInterval`**，源码注释：`// 停止定时器，避免继续轮询`

---

## 4. 状态机（实测 192 条响应）

| code | 次数 | message | 语义 |
|---|---|---|---|
| `200059541` | 121 | 未找到设备的二维码信息 | 二维码尚未被扫描 |
| `200059548` | 39 | 二维码信息未授权 | **已扫码，等待用户在手机上确认** |
| `200059544` | 31 | 二维码信息与登录二维码信息不一致 | sID 不匹配（换码后用了旧值） |
| `200059542` | 1 | 二维码已失效 | 二维码超时 |
| `200` / `0000` | 18 / 13 | 请求成功 | **登录成功**，`data` 为 AES 密文 |
| `{"error_msg":"404 Route Not Found"}` | 105 | — | 非登录请求，归档噪声 |

> `200059541` 的原文含「**设备**的二维码信息」，说明服务端按 `dID + sID` 联合定位会话。

---

## 5. 登录成功后的处理

来源：`apiServer.js` L615-631。

```js
if (result.data) {
  let str = Utils.decryptByAes(result.data);     // data 是 AES 密文，需解密
  if (typeof str == "string" && result.code == '0000') {
    params.timer && params.timer();              // 停轮询
    let userInfo = JSON.parse(str);              // 明文用户信息
    localStorage.setItem("user_unique_id", userInfo.account);
    localStorage.setItem('fromUserInfo', JSON.stringify(userInfo));
    await apiNative.account.addAccountOrModify(userInfo);
  }
}
```

**重要**：成功响应的 `data` 不是明文，**必须 AES 解密**才能拿到 `userInfo`。这是 `README.md` 里「旧 TS 与真实流量不一致案例表」的一个同类陷阱。

> **补全（2026-09-17 晚，补到 `common/utils.js` 实现后）**：解密参数为
> **AES-128-ECB / PKCS7**，密钥 `qPqDw263XgFgL3u8`（非生产环境为 `2ErfJus1Ofr@2o24`），
> 且 `decryptByAes` 的**输入是 hex 而不是 base64**（同文件里的 `encryptByAes` 才输出 base64，
> 两者不是一对逆运算）。已落地于 `mcloud-upstream::login::LoginDataDecryptor`，
> 向量见 `test/golden/pcclient-crypto-vectors.json`。
>
> **端到端确认（2026-09-18）**：扫码登录已真机跑通（CLI 出码 → 手机扫码确认 →
> 本地落库）。落库后 `account_state.jwt_token` 长度 201、`accounts.auth_expire`
> 落在 30 天后 —— 说明**解密、字段提取、`auth` 重建三段都正确**：
> `auth_expire` 是从 `authToken` 的第 4 段（epoch 毫秒）解出来的，
> 而它只有在"重建的 auth 形状与真实 token 分段一致"时才会是个合理值。
>
> **一处与本文示例不一致（待下次登录确认字段名）**：真实 `userInfo` 里
> **没有 `userId`**，所以 `user_domain_id` 落了空。本文 §5 那个 `userId` 是
> 从设备绑定流程抄来的合成样本，不能当成登录响应的字段表。
> 下次登录时打印 `userInfo` 的**字段名**（不打印值）即可确定它到底叫什么。

本项目实测捕获到的 `authToken`（明文形态，来自设备绑定流程）。
**下例是等长同结构合成值**：分段数、各段长度、载荷里 `_` 与 `.` 的位置都与实测一致，
只有载荷字符被替换 —— 真实 token 是可复用凭据，不得入库：
```json
{"authToken":"SYNTH001|1|RCS|1792245335331|AAAAAAAAAAAAAAAAAAAAAAA_AAAAAAAAAAAAAAAAAAA..A.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAA.__AAAAAA_AAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAA-","userId":"1990000000000000001"}
```

---

## 6. 同接口的其他登录方式（同文件确认）

`/user/thirdlogin` 是**登录总入口**，靠 `pintype` + `extInfo` 区分：

| 方式 | `pintype` | `msisdn` | `dycpwd` | `extInfo` 额外字段 | 源码行 |
|---|---|---|---|---|---|
| 扫码 | `21` | `""` | sessionId | — | L596 |
| 短信验证码 | `5` | 手机号 | 验证码 | `ifOpenAccount: 1` | L216 |
| 账号密码 | `9` | 用户名 | `Utils.encryptByHash(password)` | `ifOpenAccount: 0`；安全登录时改用 `secinfo` 且 `dycpwd=""` | L395 |
| SIM | — | — | 验证码 | `appId: "300012063908"`、`appKey: "336CF2D0146B51CEDAB502750E7E8C04"`、`templateId: "DK20210727112418626b47"` | L258 |

**账号密码登录注意**：密码经 `Utils.encryptByHash()` 哈希后放入 `dycpwd`。
**安全登录分支**：若 `params.smsCheckRandom` 有值，则 `dycpwd` 置空，改传 `secinfo = Utils.safeVerifyCodeEncrypt(smsCheckRandom + username + verifyCode)`。

---

## 7. 设备指纹

来源：`apiServer.js` L642-667 `getClientDeviceInfo()`。

```js
localStorage.setItem("windowMachine", res.machine + '-ENDIN');
localStorage.setItem("windowDeviceInfo", res.deviceinfo);
```

实测值：
```
windowMachine    = 62AA2BDA29F99744536E142A83CA5B03-ENDIN
windowDeviceInfo = ||11|8.8.6.20260829|PC|TGktMjAyNjA4MjgxNjI5|...| Windows 11 (10.0.26100.1591)|1920X1020|Q2hpbmVzZSAoU2ltcGxpZmllZCk=
```

`windowDeviceInfo` 为 `|` 分隔（14 段，字段表见 [`docs/client-profiles.md`](./client-profiles.md) §5.2），
`TGktMjAyNjA4MjgxNjI5` 解码得 `Li-202608281629`（设备型号码）。
代码内的兜底模板（原生接口失败时使用）：
```js
`||11|${appVersion}|PC|QkYtMjAyMDAzMTAxNjQ3|${windowMachine}|| Windows 10 (10.0)|1920X1040|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||`
```

---

## 8. 授权二维码（另一条独立流程，勿混淆）

来源：`apiServer.js` L707-734 `createAuthLoginCode()`。

用于**已登录设备被新设备请求授权**，与扫码登录是两回事：

```js
let authQRcodeUrl = "https://caiyun.feixin.10086.cn:7071/portal/app/launch.html";
let QRcodeInfo = {
  account: data.account,
  deviceID: deviceId,
  authRecordID: data?.extInfo?.logInAuthInfoRecordId
};
let QRCodePath = authQRcodeUrl
  + "?userInfo=" + btoa(JSON.stringify(QRcodeInfo))
  + "&url=mcloud://untrustedDeviceAuthorize";
```

轮询接口：
```
POST /user/device/getUserAuthDevInfo
{ "queryType": 1, "authRecordID": "...", "deviceID": "...", "phoneNumber": "..." }
```

---

## 9. 落地建议（给 mcloud 实现）

1. **不要实现「取码接口」** —— 本地生成 32 位 HEX 即可，这一步零网络依赖，纯 `Random` trait 输出。
2. `dycpwd` 直传二维码里的 `sID`，无需变换。
3. 轮询用 1000ms 定时器，状态 `200059548` 时可对外提示「已扫码待确认」。
4. 成功分支**必须先 AES 解密 `data`**（AES-128-ECB/PKCS7，hex 输入，key 见 §5 补全），才能取到 `userInfo.account` 与 `authToken`。
5. `dID` 需持久化（设备指纹），否则刷新二维码会导致设备 ID 漂移、服务端定位失败。
6. 建议抽象为 `Clock`（1000ms 定时）+ `Random`（GUID32）+ `HttpClient`（thirdlogin）三个 trait，无 IO 依赖即可单测。
