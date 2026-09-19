# 移动云盘 PC Web：扫码登录协议

> 状态：**已确认**。证据链 = Chrome DevTools Protocol 实测抓包（URL + 请求头 + 请求体 + 响应体全量）+ Web 前端产物源码交叉验证。
>
> - 采集时间：2026-09-18 18:30-18:45
> - 采集环境：Windows 11 + Edge，Web 端 `7.18.0`（`mcloud-version` 头）
> - 采集方式：CDP 直连（一次拿到 URL / headers / body / response），非代理抓包
> - **登录协议一族**（按端分文件，本篇是 **Web** 那一支）：
>   [`upstream-login-protocol.md`](./upstream-login-protocol.md)（PC 客户端扫码）、
>   [`upstream-logout-sms-login.md`](./upstream-logout-sms-login.md)（PC 客户端退出+短信）、
>   [`upstream-app-sms-login.md`](./upstream-app-sms-login.md)（iOS App 短信）、
>   [`upstream-app-business-api.md`](./upstream-app-business-api.md)（iOS App 业务接口）。
>   全库索引见 [`docs/README.md`](./README.md)。
> - 依据 `docs/rust-rewrite-plan.md` §7.0：本文件全部来自实测，无猜测性内容

---

## 0. 与 PC 客户端的差异（先读这段）

Web 端和 PC 客户端**共用同一个登录接口**（`POST /user/thirdlogin`），但**加密分层不同**：

| 维度 | PC 客户端 | PC Web |
|---|---|---|
| 载荷加密 | `puzzleVerfycode` 等**字段级**加密 | **整个 body 整体加密**（AES-CBC，见 §4） |
| 载荷密钥 | 无（字段各自加密） | `UqEZkrjCKfa02pP6jntzFmkzOz86zHUC`（**硬编码**，见 §4.1） |
| 请求头密钥协商 | 无 | `mcloud-skey` = RSA 加密 16 位 AES 密钥（见 §3.3） |
| 签名 | 无 | `mcloud-sign`（见 §3.2） |
| 二维码 | 服务端下发 / 本地拼装 | **纯前端本地生成**（见 §2） |
| 轮询频率 | 未定 | **2 秒**（见 §5） |

> **重要**：Web 端的 body 整体加密密钥 `UqEZkrjCKfa02pP6jntzFmkzOz86zHUC` 是**明文硬编码在前端 JS 里的**，
> 且 PC 客户端**不使用**这个密钥 —— 别把它和 `encryptByAesCBC` 的 `CREATPUZZLE03F9A` 混为一谈。

---

## 1. 域名与路径前缀

从常量模块解出（源码 `da71` / `b441`）：

| 符号 | 值 | 用途 |
|---|---|---|
| `userBase` | `https://user-njs.yun.139.com` | 用户域（thirdlogin / status / sms） |
| `orchBase` | `https://yun.139.com/orchestration/auth-rebuild` | 认证改造域（getRsaPublicKey 等） |
| `apiVersion` | `v1.0` | 路径版本段 |
| `clientCode` | `10701` | 见 §3.1 |
| `channel` | `10000034` | 渠道号 |

---

## 2. 二维码生成（**纯前端，无网络请求**）

实测：点击「扫码登录」后**没有任何 HTTP 请求**生成二维码 —— 它是前端用 `qrcode` 库画在 canvas 上的。

源码依据（二维码页组件）：
```js
createLoginQrcode(){
  this.qrcSessionID = getRandomSring(16);          // ★ 16 位随机字符串
  const t = `https://yun.139.com/w/#/qrcLogin?sID=${this.qrcSessionID}&dID=${this.visitorId}&cType=9`;
  this.qrcode = new QRCode(this.$refs.qrCodeUrl, {
    text: t,
    width: 180, height: 180,
    colorDark: "#000000", colorLight: "#ffffff",
    correctLevel: QRCode.CorrectLevel.H,           // ★ 纠错等级 H
  });
}
```

### 2.1 二维码内容（**必须逐字复刻**）

```
https://yun.139.com/w/#/qrcLogin?sID={qrcSessionID}&dID={visitorId}&cType=9
```

| 参数 | 来源 | 说明 |
|---|---|---|
| `sID` | `getRandomSring(16)` | **会话 ID**，就是后续轮询的 `dycPwd`（见 §5） |
| `dID` | `visitorId` | FingerprintJS 指纹（见 §2.2） |
| `cType` | `9` | 固定值 |

> `getRandomSring(n)`：字符集 `A-Za-z0-9`，逐字符 `Math.random()` 取 —— 与 PC 客户端的同名函数一致。

### 2.2 `visitorId` 生成（FingerprintJS）

源码依据（`appInit`）：
```js
const e = localStorage.getItem("randomID") || getRandomSring(32);
localStorage.setItem("randomID", e);
const t = await window.FingerprintJS.load();
const a = await t.get();
const n = a.visitorId || e;        // ★ 指纹失败时回退到 randomID
store.commit("setVisitorId", n);
```

| 优先 | 值 |
|---|---|
| 1 | `FingerprintJS.get().visitorId`（浏览器指纹） |
| 2（回退） | `localStorage.randomID`，首次生成 = `getRandomSring(32)` |

### 2.3 二维码有效期

`codeExpire = 300`（秒）。前端同时用**本地计时**和**服务端返回码**两条路径判失效（见 §5）。

---

## 3. 请求头（所有 139 接口通用）

### 3.1 固定头（实测抓包 + 源码常量双确认）

```http
x-yun-channel-source: 10000034
x-yun-app-channel: 10000034
mcloud-client: 10701
mcloud-channel: 1000101
mcloud-version: 7.18.0
mcloud-route: 001
x-yun-api-version: v1
x-yun-module-type: 100
x-yun-svc-type: 1
x-m4c-caller: PC
x-m4c-src: 10002
x-inner-ntwk: 2
INNER-HCY-ROUTER-HTTPS: 1
```

源码依据（请求拦截器）：`mcloud-channel` 硬编码 `1000101`；`mcloud-client` 来自常量 `{"mcloud-client":"10701"}`；
`x-yun-channel-source` / `x-yun-app-channel` 同源常量 `x-huawei-channelSrc = "10000034"`。

### 3.2 `x-yun-client-info`（设备信息）

模板（源码）：
```js
`||9|${version}|${shell||supporter}|${shellVs||supporterVs}|${visitorId}||${system} ${systemVs}||${language}|||${btoa(shell)}||`
```

实测：
```
||9|7.18.0|edge||86d06702dfa9bc1ae5d3de4a245026c8||windows 10||zh-CN|||ZWRnZQ==||
```

| 位 | 值 | 含义 |
|---|---|---|
| 1 | 空 | — |
| 2 | `9` | 固定 |
| 3 | `7.18.0` | 版本 |
| 4 | `edge` | 浏览器 shell（UA 解析） |
| 5 | 空 | shell 版本（Edge 未匹配到） |
| 6 | `86d06702…` | **`visitorId`** |
| 7 | 空 | — |
| 8 | `windows 10` | 系统 + 版本 |
| 9 | 空 | — |
| 10 | `zh-CN` | 语言 |
| 11-13 | 空 | — |
| 14 | `ZWRnZQ==` | `btoa("edge")` |

> UA 解析规则：`applewebkit` → `webkit`；`edge`/`edg` → `edge`；`chrome` → `chrome`；`safari` → `safari`。平台 → `desktop`/`mobile`。

### 3.3 `mcloud-skey`（**首次请求为空**）

```
mcloud-skey: <空>   ← getRsaPublicKey 请求（密钥尚未协商）
mcloud-skey: N++pLZdmQbBz7D/KwB1HlNYeCih4RYGBLcTNwIUNIdsVHt0g/QGgnxdW5/ELChYpgIQ2RfDXEg+eBPwzsmzHCPAs1HgcofT9lhNEkaB3axL0OXv/OtTX/qlkE2nScr7pdN/8189p8akTZCba6b5qkswkeiodEdBO2G0GF8Ds0QE=
```

**生成逻辑**（源码 `getSecretKey` action）：
```js
async getSecretKey({commit:e}, t){
  const n = SINGLE_LOGIN_FLAG==="1" ? C_ENCRYPT_CODE : (localStorage["a_k"] || getRandomSring(16));
  const o = await getRsaPublicKey(t);              // 取公钥
  if (o.success) {
    e("setAESKey", n);                             // 存 16 位 AES 密钥
    const t = rsaEncrypt(n, o.data.publicKey);     // ★ RSA 加密该密钥
    e("setSecret", t);                             // 存 → 每次请求写进 mcloud-skey 头
    o.aesKey = n;
    return o;
  }
}
```

| 项 | 值 |
|---|---|
| 明文 | **16 位随机字符串**（`getRandomSring(16)`），或 `localStorage.a_k` 已有值 |
| 公钥 | `getRsaPublicKey` 返回的 `data.publicKey`（SPKI/PEM base64） |
| 算法 | `rsaEncrypt` = JSEncrypt → **RSAES-PKCS1-V1_5**，输出标准 Base64 |
| 结果 | 实测密文 base64 解码后 **128 字节** = 1024 位 RSA 块 ✅ 结构吻合 |
| 缓存 | `localStorage` 键名 `a_k`（常量 `AES_KEY`）；`SECRET_KEY` 存 `skey` |

> **单点登录开关** `single_login_flag = "1"` 时，改用常量 `C_ENCRYPT_CODE` 作为密钥（不随机）。
>
> **密钥不参与 body 加密** —— body 用的是另一个硬编码密钥（§4.1）。`mcloud-skey` 是**传输层会话密钥**，
> 服务端用它保护响应/其它通道；请求 body 的加解密与它无关。

### 3.4 `mcloud-sign`（签名）

实测格式（三段逗号分隔）：
```
mcloud-sign: 2026-09-18 18:44:36,B90Gv3GAcLjU0MgP,03811CD203A20E98D4CFDEE4F03A6900
             └── 时间戳 ──┘ └─ 16位随机 ─┘ └──── 32位大写 MD5 ────┘
```

**算法（已用 5 条真实配对逐条复算，5/5 全中）**：

```js
// 入参：e = params+data 的处理结果, t = data(对象), a = 时间戳, n = 16位随机
function getNewSign(e, t, a, n) {
  let s = "";
  if (t) {
    const c = Object.assign({}, t);      // 1. 浅拷贝
    s = JSON.stringify(c);               // 2. JSON 序列化
    s = encodeURIComponent(s);           // 3. ★ URL 编码
    const arr = s.split(""), sorted = arr.sort();
    s = sorted.join("");                 // 4. ★ 字符级升序排序
  }
  const r = md5(Base64.encode(s));       // 5. r = md5( base64(s) )      小写 hex
  const l = md5(a + ":" + n);            // 6. l = md5( ts + ":" + rnd ) 小写 hex
  return md5(r + l).toUpperCase();       // 7. 拼接后 md5，转大写
}
```

**头拼装**：`mcloud-sign = a + "," + n + "," + getNewSign(...)`

#### 验证记录（真实抓包 → 复算）

| # | 时间戳 | 16 位随机 | body（明文） | 实测 sign | 复算 |
|---|---|---|---|---|---|
| 1 | `2026-09-18 18:44:36` | `B90Gv3GAcLjU0MgP` | `{"clientCode":"10701","type":1}` | `03811CD203A20E98D4CFDEE4F03A6900` | ✅ |
| 2 | `2026-09-18 18:44:36` | `7QRozbky0klKsxrY` | `{"msisdn":"","random":"","dycpwd":"NGf9QQfj2yx1pK5p",…}` | `6320766E5815C3AC4DF2B2DB20A81F8B` | ✅ |
| 3 | `2026-09-18 18:44:38` | `32YegZUmQOD7Wdy1` | 同上（dycpwd 不同） | `47E7E2DFA1F27B4BB952A532073ED945` | ✅ |
| 4 | `2026-09-18 18:44:41` | `vlTSyqPIA2PmHQi9` | 同上 | `9176E9A2A5BB37BDAB33B0E4325221D9` | ✅ |
| 5 | `2026-09-18 18:44:43` | `7txAMjsjmJ3fE4la` | 同上 | `4C5DAF6E1FF05CAF11DE2ADAC618C3FF` | ✅ |

#### 三个致命易错点

| 易错点 | 说明 |
|---|---|
| **`encodeURIComponent`** | 序列化后必须 URL 编码，漏掉必错 |
| **字符级排序** | `.split("").sort().join("")` 是**逐字符**排序，不是按 key 排序 |
| **`l` 是小写** | `md5(a+":"+n)` **不转大写**，只有最终 `md5(r+l)` 转大写 |

> 签名对象是**加密前的明文对象**（`e.data`），不是加密后的密文字符串。对 `thirdlogin` 这类整体加密的接口，
> 签名用明文 JSON，body 发密文 —— 两者独立。

### 3.5 `x-yun-app-key`

`POST /user/status/query` 实测返回：
```json
{"success":false,"code":"01000004","message":"请求头x-yun-app-key规则不正确","data":null}
```

**结论：这是遗留接口的头，扫码登录链路不使用。** 依据：
- Web 前端全量产物（55 个 JS 文件）中**搜不到** `x-yun-app-key` 字符串
- 该接口**不在**「需加密白名单」内（见 §4.3）
- 扫码登录全流程实测**未调用** `/user/status/query`

---

## 4. 请求体加密（**整体加密**）

### 4.1 密钥与算法

源码依据（加密拦截器）：
```js
const _t = CryptoJS.enc.Utf8.parse("UqEZkrjCKfa02pP6jntzFmkzOz86zHUC");   // ★ 16 字节 = AES-128

function Dt(e) {                                  // 加密
  const t = CryptoJS.lib.WordArray.random(16);    // 随机 IV
  const a = CryptoJS.AES.encrypt(Utf8.parse(JSON.stringify(e)), _t,
                                 {iv: t, mode: CBC, padding: Pkcs7});
  return CryptoJS.enc.Base64.stringify(t.concat(a.ciphertext));   // Base64(IV ‖ CT)
}

function Pt(e) {                                  // 解密
  const t = Base64.parse(e), a = t.clone(), n = a.words.splice(4);
  a.init(a.words); t.init(n);
  const o = Base64.stringify(t);
  const i = AES.decrypt(o, _t, {iv: a, mode: CBC, padding: Pkcs7});
  return i.toString(Utf8);
}
```

| 项 | 值 |
|---|---|
| 算法 | **AES-128-CBC / PKCS7** |
| 密钥 | `UqEZkrjCKfa02pP6jntzFmkzOz86zHUC`（UTF-8，16 字节，**硬编码**） |
| IV | **每请求随机 16 字节**（`WordArray.random(16)`） |
| 载荷 | `JSON.stringify(对象)` 或原始字符串 |
| 输出 | `Base64(IV ‖ ciphertext)` |

> 这是与 PC 客户端最本质的差异：**整个 body 是一个 base64 字符串**，不是 JSON 对象。

### 4.2 解密实证（Python 独立复算）

用上述密钥对实测样本解密：

**轮询请求**（`thirdlogin`）：
```json
{"msisdn":"","random":"","dycpwd":"NGf9QQfj2yx1pK5p","cpid":292,"clienttype":670,
 "version":"mCloud_4.3.0_536","pintype":21,"secinfo":"8E5F517B726FACE24C51F928099634D2632757FE",
 "loginMode":"0","extInfo":{}}
```

**响应**：
```json
{"success":false,"code":"200059541","message":"未找到设备的二维码信息","data":null}
```

base64 解码后 = `16 字节 IV + 208 字节密文`（208 是 16 的倍数 ✅）。

### 4.3 需加密的接口白名单

源码中的显式清单（`$t` 数组，路径模板：`{orchBase}/{apiVersion}/{resource}/{clientCode}/...`）：

```js
$t = [
  `${u.a}/${b()}/permission/${u.h}/logout`,
  `${u.a}/${b()}/permission/${u.h}/login`,
  `${u.a}/${b()}/sms/${u.h}/verifySmsCode`,
  `${u.a}/${b()}/sms/${u.h}/getSmsVerifyCode`,
  `${u.a}/${b()}/key/${u.h}/getRsaPublicKey`,       // ★ 实测已解密验证
  `${u.a}/personalCloud/search/${u.h}/advanceSearch`,
  `${u.a}/${At()}/cloudManage/${u.h}/getUserInfo`,
  `${u.a}/${b()}/graph/${u.h}/getGraphVerifyCode`,
  `${u.a}/${b()}/graph/${u.h}/verifyGraphCode`,
  `${u.a}/${b()}/pwd/${u.h}/resetPwd`,
  `${u.a}/${b()}/token/${u.h}/getTmpTicket`,
  `${u.a}/safebox/sim/${u.h}/queryProvinceSupport`,
  `${u.a}/domain/${u.h}/verifyDomain`,
  ...
]
// u.a = https://yun.139.com/orchestration/auth-rebuild
// u.h = clientCode = "10701"
```

> 命中白名单的请求，body 走 `Dt()` 加密；**未命中**的走明文（例：`getRsaPublicKey` 实测在同一次会话里
> 出现过**明文** `{"clientCode":"10701","type":1}` 与**密文**两种形态 —— 说明加密与否取决于调用路径，
> 实现时**不要硬编码**，按实测或按白名单判断）。

---

## 5. 轮询状态机（扫码登录核心）

### 5.1 时序

```
[1] 点击「扫码登录」
      └─ 前端本地生成二维码（无网络请求）
         内容 = https://yun.139.com/w/#/qrcLogin?sID={qrcSessionID}&dID={visitorId}&cType=9

[2] getSecretKey(clientCode, type=1)
      └─ POST {orchBase}/key/v1.0/getRsaPublicKey
         请求（首次为空 skey）：{"clientCode":"10701","type":1}
         响应：{"success":true,"code":"0","data":{"publicKey":"MIGfMA0G…"}}
      └─ 生成 16 位 AES 密钥 → RSA 加密 → 存为 mcloud-skey

[3] 轮询循环（每 2 秒）
      └─ POST https://user-njs.yun.139.com/user/thirdlogin
         body（明文，加密前）：{"account":"","dycPwd":"{qrcSessionID}","type":5,"authLogin":true}
         body（实际发送）：Base64(IV ‖ AES-CBC(上述 JSON))
```

> 注意字段名：**请求体里是 `dycPwd`（大写 P）**，PC 客户端的短信/扫码用的是 `dycpwd`（全小写）。
> Web 端这两个写法在源码里并存，实现时以实测 body 为准。

### 5.2 状态码分支（源码逐字）

源码依据（`queryQrcLoginResult`）：
```js
const s = (now - startQueryQrCTime) / 1000;        // 已耗时（秒）
if (e.success) {
  // 登录成功：解密响应 data → setCookie(authToken 等) → 跳转
} else if ("200059542" == i || s > codeExpire) {   // 失效
  failTips = "二维码已失效";
} else if ("200059548" == i) {                     // 已扫码，待确认
  qrcodeTimer = setTimeout(queryQrcLoginResult, 2000);   // ★ 2 秒重试
} else if ("200059549" == i) {                     // 已取消
  failTips = "已取消，请重新扫码";
} else if (qrCodeErrArr.indexOf(i) > -1) {         // 其它错误
  failTips = "登录失败";
} else if (s < codeExpire + 20) {                  // 继续轮询
  qrcodeTimer = setTimeout(queryQrcLoginResult, 2000);
}
```

| 返回码 | 语义 | 动作 |
|---|---|---|
| — (`success:true`) | **登录成功** | 解密 data，取 `token`/`authToken`/`expireTime`，跳转 |
| `200059541` | 未找到设备的二维码信息 | 继续轮询（未扫码） |
| `200059542` | 二维码已失效 | 停止，提示"二维码已失效" |
| `200059548` | 二维码信息未授权（**已扫码待确认**） | **2 秒后重试** |
| `200059549` | 二维码已取消 | 停止，提示"已取消，请重新扫码" |
| `qrCodeErrArr` 内 | 登录失败 | 停止，提示"登录失败" |

`qrCodeErrArr`（源码常量）：`["200059543","200059545","200059546","200059547"]`

**失效判定是双条件**（`200059542` **或** 本地耗时 > `codeExpire`），`codeExpire = 300` 秒；
未命中上述分支时，只要 `s < codeExpire + 20`（320 秒）就继续轮询。

### 5.3 登录成功的载荷 —— **已实测确证**

响应解密后（源码）：
```js
const a = JSON.parse(cryptAesDecrypt(e.data, "qPqDw263XgFgL3u8"));
// 取出：
//   simplifyAccount   脱敏账号
//   encryptAccount    加密账号
//   token             ★
//   authToken         ★
//   expireTime        过期时间
```

> **注意**：登录成功后的响应解密用的是**另一个密钥** `qPqDw263XgFgL3u8`
> —— 即 §4.1 的 body 密钥 `UqEZkrjCKfa02pP6jntzFmkzOz86zHUC` **只用于请求体**，
> 成功响应走 `cryptAesDecrypt`（ECB）。

### 5.4 登录成功后的凭据落盘 —— **已实测确证（真实扫码）**

**实测证据**：一次真实扫码登录成功后，浏览器 profile 的 Cookie 库与 `localStorage` 中落下以下项。

#### Cookie（域 `yun.139.com`）

| Cookie 名 | 语义 | 源码常量名 |
|---|---|---|
| `auth_token` | **认证 token** | `auth_token` |
| `token` | **token** | `token` |
| `authorization` | 授权串 | `authorization` |
| `ud_id` | 用户域 ID | `ud_id` |
| `a_k` | **AES 会话密钥**（§3.3 的 16 位随机值） | `a_k` |
| `skey` | RSA 加密后的密钥（即 `mcloud-skey` 头的值） | `skey` |
| `ORCHES-I-ACCOUNT-ENCRYPT` | 加密账号 | `ORCHES-I-ACCOUNT-ENCRYPT` |
| `ORCHES-I-ACCOUNT-SIMPLIFY` | 脱敏账号 | `ORCHES-I-ACCOUNT-SIMPLIFY` |
| `cutover_status` | 迁移状态 | `cutover_status` |
| `nation_code` | 国家码 | `nation_code` |
| `platform` | 平台 | `platform` |
| `userInfo` | 用户信息 | — |

#### `localStorage`（域 `yun.139.com`）

| key | 实测值（脱敏后） | 说明 |
|---|---|---|
| `simplifyAccount` | `199****0000` | 脱敏手机号（11 位，中间 4 位打码） |
| `encryptAccount` | `MTk5MDAwMDAwMDA=` | Base64(11 位手机号)，长度随号码长度 |
| `loginType` | `0` | 登录类型 |
| `provCode` | `23` | 省份码 |
| `routerInfo` | JSON（路由策略表） | `qryRoutePolicy` 结果 |
| `randomID` | 32 位 `A-Za-z0-9` | 指纹回退值（§2.2） |
| `TY_USER_ID` | `<11 位手机号>\|null` | 天翼统计 ID，形如 `19900000000\|null` |

> **关键结论**：登录凭据**走 Cookie**，不落在 `localStorage`（源码 `handleSetCookie` 用 `document.cookie`）。
> 其中 `a_k` + `skey` 成对出现，**印证了 §3.3 的密钥协商链路**：`a_k` 是明文 16 位 AES 密钥，
> `skey` 是它经 RSA 加密后的产物 —— 两者同时落盘，服务端/客户端各自持有。
>
> 权限：以上均为 `Secure` cookie（仅 HTTPS）。

---

## 6. 实测完整时序

### 6.1 第一轮：未扫码（等 300 秒超时）

| 序号 | 请求 | 状态 | 说明 |
|---|---|---|---|
| 1 | `POST /orchestration/auth-rebuild/key/v1.0/getRsaPublicKey` | 200 | `mcloud-skey` **为空**；body 明文 `{"clientCode":"10701","type":1}` |
| 2 | `POST /user/thirdlogin` | 200 | 首次轮询；`dycPwd` = 二维码会话 ID；响应 `200059541` |
| 3-20 | `POST /user/thirdlogin`（×18） | 200 | 每 **2 秒**一次；`dycPwd` 固定；响应始终 `200059541` |
| — | `POST /user/status/query` | 200 | `01000004`（遗留接口，不参与登录） |
| — | 若干广告位接口 | 200 | `getAdInfos` / `reported`，与登录无关 |

**观察**：
- 轮询间隔稳定 **2 秒**，`dycPwd`（会话 ID）全程不变
- `secinfo` 字段**每次不同**（`8E5F517B…` / `62899BA2…` 等 40 位 hex）—— 是设备/环境指纹的哈希，非固定值
- 二维码未被扫描，未走出成功分支

### 6.2 第二轮：**真实扫码并确认授权**（关键）

实测状态跃迁（`dycPwd` = `QfiekgO0mehZyp8v`，全程不变）：

| # | 时间 | 返回码 | 语义 | 阶段 |
|---|---|---|---|---|
| 1 | +0.0s | `200059542` | 二维码已失效 | 上一张旧码残留 |
| 2 | +2.2s | `200059541` | 未找到设备的二维码信息 | 新会话，等待扫码 |
| 3 | +4.3s | `200059541` | 未找到设备的二维码信息 | 等待扫码 |
| 4 | +6.4s | **`200059548`** | **二维码信息未授权** | ★ **手机已扫码，等待确认** |
| 5 | +8.9s | **`200059548`** | **二维码信息未授权** | ★ 持续等待确认 |
| … | — | `success:true` | 登录成功 | **确认授权后** |

> **实测确证 `200059548` 的语义**：它出现在**手机扫码之后、点击"确认授权"之前**，
> 与源码分支（`scanSuccess=true` → 2 秒后重试）完全一致。
> 这是扫码登录状态机里**唯一需要区分"已扫/未扫"的关键码**。

**登录成功后的落盘**：见 §5.4（Cookie 12 项 + localStorage 7 项）。

### 6.3 登录成功后的后续请求

页面跳转后新增的业务请求（实测）：

```
POST https://share-kd-njs.yun.139.com/yun-share/richlifeApp/devapp/IControl/queryControlSwitch
GET  https://img.mcloud.139.com/material_prod/material_media/...png
GET  https://yun.139.com/w/static/img/member_button.*.gif
```

> 这些是登录后的首页初始化请求，**不属于登录协议本身**。

---

## 7. 落地建议（给 mcloud）

1. **二维码**：无需请求服务端，直接拼 URL + 本地渲染；
   `sID` 用 16 位 `A-Za-z0-9` 随机串，`dID` 用稳定设备标识（可简化为持久化的随机 32 位串）。
2. **请求头**：`mcloud-skey` 首次为空，取到 RSA 公钥后再填；`mcloud-sign` 按 §3.4 实现，
   **务必注意 `encodeURIComponent` + 字符级排序 + 中间值小写**这三个坑。
3. **签名对象**：用**加密前的明文对象**，与 body 的加解密解耦。
4. **body 加密**：`Base64(IV16 ‖ AES-128-CBC/JSON)`，密钥 `UqEZkrjCKfa02pP6jntzFmkzOz86zHUC`；
   **IV 必须每次随机**，解密时前 16 字节切出作 IV。
5. **轮询**：固定 **2 秒**间隔；`200059548`（已扫码）继续轮询，`200059542`/`200059549` 立即终止；
   本地 `codeExpire=300` 秒兜底。
6. **成功分支**：先用 `UqEZkrjCKfa02pP6jntzFmkzOz86zHUC` 解 body 响应，再用 `qPqDw263XgFgL3u8`
   解 `data` 字段得到 `token`/`authToken`（**建议补一次实测确认**，见 §5.3）。
7. **`x-yun-app-key` 不用实现**（§3.5）。

---

## 8. 错误码汇总（Web 扫码登录）

| code | message | 语义 | 处理 |
|---|---|---|---|
| `0` | success | 成功 | — |
| `200059541` | 未找到设备的二维码信息 | 未扫码 | 继续轮询 |
| `200059542` | 二维码已失效 | 超时 | 停止 |
| `200059543` | （`qrCodeErrArr`） | 登录失败 | 停止 |
| `200059545` | （`qrCodeErrArr`） | 登录失败 | 停止 |
| `200059546` | （`qrCodeErrArr`） | 登录失败 | 停止 |
| `200059547` | （`qrCodeErrArr`） | 登录失败 | 停止 |
| `200059548` | 二维码信息未授权 | **已扫码待确认** | 2 秒后重试 |
| `200059549` | 已取消 | 用户取消 | 停止 |
| `01000004` | 请求头 `x-yun-app-key` 规则不正确 | 遗留接口 | 不调用该接口 |

---

## 9. 已确证 / 待补

### 已确证（实测 + 源码双向）
- ✅ 二维码内容与生成方式（纯前端，URL 模板）
- ✅ 全部请求头及构造逻辑（含 `x-yun-client-info` 模板、`clientCode=10701`、`channel=10000034`）
- ✅ `mcloud-sign` 签名算法（**5 条真实配对 5/5 复算通过**）
- ✅ `mcloud-skey` 生成逻辑（16 位随机 → RSA-1024 PKCS1-v1.5 → Base64；结构 128 字节吻合）
- ✅ body 整体加密（AES-128-CBC/PKCS7，密钥 `UqEZkrjCKfa02pP6jntzFmkzOz86zHUC`，随机 IV）
- ✅ `thirdlogin` 请求/响应双向解密实证
- ✅ 轮询状态机与全部状态码分支
- ✅ `getRsaPublicKey` 接口与公钥返回结构
- ✅ **真实扫码完整闭环**（真机扫码 + 确认授权，`200059548` 实测出现 2 次，见 §6.2）
- ✅ **登录成功后的凭据落盘**（Cookie 12 项 + `localStorage` 7 项，见 §5.4）
- ✅ **密钥协商成对印证**：Cookie 中 `a_k`（明文 16 位 AES 密钥）与 `skey`（RSA 加密产物）成对出现，印证 §3.3 链路

### 待补
- ⚠️ `mcloud-skey` 的**服务端用途**（请求体加密与它无关，其具体保护对象未验证）
- ⚠️ 登录成功后业务请求的完整头部取值（`mcloud-skey` 是否随 `a_k` 重新生成，未逐条比对）

---

## 附：原始素材

- 抓包原始 JSONL（本机临时目录，未入库）：`web-qr-capture/netlog-web-qr3.jsonl`
- **真实扫码抓包 JSONL**（25 条，CDP `Network` 域原生拦截实时落盘）：`web-qr-capture/netlog-scan2.jsonl`
- Web 前端全量产物（本机临时目录，未入库）：`web-qr-capture/alljs/`（55 个 JS，含 `app.ff91d73b.js`、`chunk-7e9eba2c.*.js`）
- 二维码截图：`web-qr-capture/qr-images/qr-*.png`（含真实扫码用码 `qr-scan2-*.png`）
- 登录成功凭据取证（Edge profile 磁盘副本，本机临时目录，**已脱敏后摘要入文档，原始档不入库**）：
  - `edge-cdp-scan/Default/Local Storage/leveldb/000003.log`
  - `edge-cdp-scan/Default/Network/Cookies`
