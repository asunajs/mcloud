# 移动云盘 iOS App：业务接口的 `signature`、`encodeData` 与 `hcy-cool-flag` 信封（**三把都开了**）

> 状态：**已实测全量验证**。证据链 = iOS App 二进制字符串常量
> + macOS 客户端 `app.asar` 源码（`out/renderer/api/api.js`）
> + App 自有网络日志（`har/applog-2026-09-19-0842/`，gitignore）
> + 活体探针（§7、§8）。
> 依据 `docs/rust-rewrite-plan.md` §7.0：本文只记实测；推断显式标注。
>
> **登录协议一族**（按端分文件，本篇是 **iOS App 已登录业务接口**那一支）：
> [`upstream-app-sms-login.md`](./upstream-app-sms-login.md)（iOS App 短信登录，本篇的前置）、
> [`upstream-login-protocol.md`](./upstream-login-protocol.md)（PC 客户端扫码）、
> [`upstream-logout-sms-login.md`](./upstream-logout-sms-login.md)（PC 客户端退出+短信）、
> [`upstream-web-qr-login.md`](./upstream-web-qr-login.md)（PC Web 扫码）。
> 全库索引见 [`docs/README.md`](./README.md)。

这两把锁属于 `docs/capture/app-api-families.txt` 里的**形态 B**
（`signature` 族：带 `signature`、不带 `x-yun-uni`）。
在本文之前它们都标着"未解"，现在都解了 —— 而且是**同一把密钥**。

§8 的第三把（`hcy-cool-flag`）是**另一条独立线**：它不在形态 B 里，
密钥也**不是**同一把，别混用。

---

## 1. `signature`：`SHA256` 一个固定顺序的键值串

```
signature = SHA256(
    "clientId=iosapp&clientKey=BOFlEUINUdSugD6u&msgId=<msgId>&timeStamp=<13位毫秒>&version=<版本>"
)   // 小写 hex，64 字符
```

| 项 | 值 | 备注 |
|---|---|---|
| `clientId` | `iosapp` | 与请求头 `clientId` 同值 |
| `clientKey` | `BOFlEUINUdSugD6u` | **App 独有**；PC 是 `6lXUhuJGkkAr5HXr` |
| `msgId` | 32 位小写 hex | 与请求头 `msgId` 同值 |
| `timeStamp` | 13 位毫秒 | 与请求头 `timeStamp` 同值（**大写 S**，不是 `timestamp`）|
| `version` | 如 `V1.0` | 与请求头 `version` **逐字相同**（**带 `V`**）|
| 字段顺序 | `clientId → clientKey → msgId → timeStamp → version` | 顺序固定，改了就不匹配 |

**验证：6165 / 6165 条实拍请求逐字命中**，且**跨三个 `version` 都命中**
（`V1.0` 6162 + `V6.1` 2 + `V1.1` 1）—— 所以 `version` 是**按请求头原样代入**，
不是把 `V1.0` 写死。且只有这一种形态命中：

| 试过的形态 | 命中 |
|---|---|
| `version=V1.0`（原样带 V）+ `timeStamp` | **6165 / 6165** |
| `version=1.0`（去掉 V）| 0 / 6165 |
| 省略 `version` | 0 / 6165 |
| 用小写 `timestamp` | 0 / 6165 |
| 按 `version` 分组：`V1.0` / `V6.1` / `V1.1` | **6162 / 6162、2 / 2、1 / 1** |

> PC 端是同一个模板，只是常量不同 —— 见 `docs/capture/apiServer.js` 的
> `Utils.createSignature()`（`clientId="pcapp"`、`clientKey="6lXUhuJGkkAr5HXr"`、
> `version="1.1"`），以及它把 `hash` 塞进 `signature` 头、把 `uuid`/`timeStamp`
> 塞进 `msgId`/`timestamp` 头。
>
> ⚠ 注意 PC 的 `createSignature` 里 `version` 写死 `"1.1"`，而 **App 用的是请求头里那个
> `V1.0`** —— 所以**别照抄 PC 的 `1.1`**，要用"本次请求实际用的 version"。

---

## 2. `encodeData`：`AES-128-CBC` 加密**手机号**

```
encodeData = base64( 16 字符 ASCII 的 IV ‖ AES-128-CBC/PKCS7(
                        key = "BOFlEUINUdSugD6u",
                        iv  = 上面那 16 个 ASCII 字节,
                        plain = 手机号（11 位明文，PKCS7 补到 16 字节）) )
```

| 项 | 值 |
|---|---|
| 密钥 | **`BOFlEUINUdSugD6u`**（与 `clientKey` **是同一把**）|
| IV | 每次随机，**16 个可打印 ASCII 字符**，**就拼在密文前面** |
| 明文 | 手机号（11 字符）|
| 密文长度 | 恒 **16 字节**（一块）；整体 base64 后 **32 字节** |
| 方向 | 只用于**请求**；响应是明文（形态 B 不等于加密）|

**验证（两层，第二层是后加的，强得多）**：

1. **全量解得开**：本窗口 **6158 / 6158** 条带 `encodeData` 的请求全部解出，
   明文去重后**只有 1 种、长度 11** —— 即手机号。
2. **逐条跨字段对上**（2026-09-19 补）：把每条 `encodeData` 解出的明文，与**同一个请求**
   `Authorization` 头里的手机号比对 —— **6158 / 6158 逐条相等，零例外**。
   这比"去重后只有一种"强得多：它把"明文是手机号"从一个**统计推断**变成了**逐条证据**
   （同一请求里两个字段互相印证，不需要信任任何外部假设）。

> 顺带钉住两个细节：
> - **IV 恒为可打印 ASCII**：6158 / 6158，**0 条二进制** —— 与 §8 的 `hcy-cool-flag`
>   信封（**二进制** IV）正好相反。别互相套用。
> - `encodeType` 本窗口取值**只有 `"2"`**（6155 条带它）；另 **3 条没带**这个键
>   （例：`aiAssistant/iconConfig/query`），服务端照样接受 —— 所以它是"可选"的，
>   不是必填。

> **PC 端的对应实现在 `app.asar` 里，且是原生函数**（不是 JS）：
>
> ```js
> var signatureAndAESencrypt = (num) => {
>   let str = "", uuid = getuuid(), timeStamp = new Date().getTime();
>   if (num == 2) str = `clientId=…&clientKey=…&msgId=${uuid}&timeStamp=${timeStamp}&version=1.1`;
>   else if (num == 3 || num == 6) str = window.userAccount;   // ← 就是手机号
>   return new Promise(... jsonRequestMission("safeBoxEncrypt", { context: str, type: num }, ...));
> };
> // 调用处：signatureAndAESencrypt(2) → signature 头；signatureAndAESencrypt(3) → encodeData
> ```
>
> 所以 `encodeData` 的**语义**是"把手机号加密后随请求带上"，供服务端做身份校验；
> 加密本身由原生 `safeBoxEncrypt` 完成 —— **这也解释了为什么 JS 里找不到密钥**。

---

## 3. 形态 B 的完整头集（照抄即可）

```http
POST https://middle.yun.139.com/openapi/general/getStatus
Content-Type: application/json;charset=UTF-8
Authorization: Basic base64("mobile:<手机号>:<authToken>")
x-yun-user-timestamp: <16 位微秒>
User-Agent: mCloud_iPhone/12.6.0 (iPad; iOS 26.6; Scale/2.00)
x-DeviceInfo: 1|192.168.3.179|2|12.6.0|APPLE|iPad8,6|<uuid32>|<short12>|ipados 26.6|375 * 667|zh-Hans-CN|0.0000|0.0000|
version: V1.0                     ← 参与签名，必须与签名输入里的版本一致
clientId: iosapp
authType: 1
anonymous: 0                      （未登录时 1）
service_identifier_key: service_identifier_caixun
businessId: 600                   （只有部分接口有）
msgId: <32 位小写 hex>
timeStamp: <13 位毫秒>
signature: <SHA256 的小写 hex，64 字符>
```

**注意：没有 `x-yun-uni`。** 身份改由 `Authorization` + `encodeData`（手机号）承担。

请求体（`getStatus` 的例子，**明文**）：

```json
{"userId":"<19 位>","businessType":"aiMessageGroup"}
```

带 `encodeData` 的接口（如 `pDynamicInfo/queryBatchList`）长这样：

```json
{"unloadSource":7,"meituSource":1,"encodeData":"<base64>","encodeType":"2","pageSize":20}
```

> `encodeType: "2"` 与 PC 源码里的 `data.encodeType = 2` 对得上。

---

## 4. 密钥是怎么找出来的（方法论，下次可复用）

> 相关源码已按 `docs/capture/apiServer.js` 的同款惯例归档进仓库：
> **`docs/capture/pcclient-api.js`** = macOS 客户端 `app.asar` 里的
> `out/renderer/api/api.js`（原样，未改动）。下面的 `signatureAndAESencrypt`
> 就在它的第 644 行附近，可直接复核。
> 提取工具：`scripts/asar-extract.cjs`（`list` / `cat` / `save` / `grep`）。

iOS 二进制里**没有** `clientKey` 字面量（`strings` 搜不到），
所以走了"**先证算法、再暴力验钥匙**"两条腿：

1. **先拿算法**：从 macOS 客户端 `app.asar` 里读到 PC 的 `Utils.createSignature()`，
   得到模板 `clientId=…&clientKey=…&msgId=…&timeStamp=…&version=…` + SHA256；
   又从 `api.js` 读到 `signatureAndAESencrypt(num)`，知道 `encodeData` 是**加密手机号**。
2. **再验钥匙**：从 iOS 二进制的 `strings` 里取 **5632 个 16 字符候选**，
   对每条候选算 SHA256 并与**实拍的 6165 条**比对 —— 一次命中：
   `BOFlEUINUdSugD6u`。
3. **交叉验证**：用**同一把** key 去解 `encodeData`（AES-CBC，IV = 前 16 字节），
   要求"合法 PKCS7 + 全可打印" —— 5632 个候选里**只有一个**通过，
   而且明文正好是 11 位手机号。

> 两条腿互相独立：SHA256 那条用的是**已知输入**（msgId/timeStamp 明文可见），
> AES 那条用的是**已知输出形态**（明文必须是手机号）。同时命中 ⇒ 结论可靠。

---

## 5. 怎么复现（不需要重抓包）

数据源是 `har/applog-2026-09-19-0842/`（gitignore）。核心三行：

```python
# 1) 签名
import hashlib
hashlib.sha256(f"clientId=iosapp&clientKey=BOFlEUINUdSugD6u&msgId={msgId}"
               f"&timeStamp={ts}&version={version}".encode()).hexdigest()

# 2) 解 encodeData
import base64; from Crypto.Cipher import AES
raw = base64.b64decode(encodeData)          # 32 字节 = 16 字节 IV + 16 字节密文
pt  = AES.new(b"BOFlEUINUdSugD6u", AES.MODE_CBC, raw[:16]).decrypt(raw[16:])
pt  = pt[:-pt[-1]]                          # PKCS7 去补
pt.decode()                                 # → 11 位手机号
```

> ⚠ **两个必踩的坑，都踩过了：**
> 1. **IV 在 base64 里面**（前 16 字节），**不是**单独一个字段。
>    拿 `ivAscii` 直接当 `AES.MODE_CBC` 的 IV、再把整个 base64 结果当密文喂进去，
>    会得到"第一块乱码 + 后面正确"的假象 —— 因为 CBC 只有第一块依赖 IV。
> 2. **去补要用明文末字节**（`pt[-1]`），**不是密文末字节**。
>    写 `[:-raw[-1]]` 不报错，只是解出垃圾 —— 这种错最容易被当成"算法不对"。

黄金向量落库于 `test/golden/app-business-crypto-vectors.json`：

| 段 | 内容 | 是实拍还是合成 |
|---|---|---|
| `signatureSha256` | 3 条 | **实拍**（`msgId`/`timeStamp` 不含个人信息）|
| `encodeDataDecrypt` | 2 条 | **合成** —— 见下 |

⚠ **`encodeData` 的向量必须是合成的**：实拍密文的明文是**真实手机号**，
而密钥是要公开写进仓库的 ⇒ **实拍密文 + 公开密钥 = 等于公布手机号**。
所以那两条向量是用已确认的算法**算出来的**（明文用 `199` 号段）。
算法本身的证据是上面那两个"全量命中"的计数，不是这两条合成向量。

### 一条命令复核（不需要重抓包，也不需要 Rust）

```bash
node scripts/check-golden-vectors.cjs
```

它用 Node `crypto` **独立实现**本页两条公式，把 `test/golden/*.json` 里能算的**全部**算一遍
（当前 101 条），并与 Rust 侧互为交叉验证。与本页相关的两段会出现在覆盖汇总里：

```
✓ app-business-crypto-vectors.json  §signatureSha256     3/3
✓ app-business-crypto-vectors.json  §encodeDataDecrypt   2/2
```

> 这个脚本是**抓包侧**的产物：向量文件此前没有任何消费者（Rust 侧只 `include_str!` 了
> `auth- / crypto- / pcclient-` 三个），属于"写下来就没人再看"的死文件。
> 它同时补上第 5 道门禁扫不到的洞 —— 见 `docs/rust-rewrite-plan.md` §6.4。

---

## 6. 边界与仍未解的

1. ~~**`signature` 只验证过 `V1.0`**。日志里还有 2 条 `V6.1`、1 条 `V1.1`，
   量太少（各 1~2 条）**没单独验**，但按同一模板应当一致。~~
   → **已验证：跨三个 version 全命中**（`V1.0` 6162/6162、`V6.1` 2/2、`V1.1` 1/1）。
   `version` 是**按请求头原样代入**，不是写死 `V1.0`。
   向量里也各留了一条 `V6.1` / `V1.1`（`test/golden/app-business-crypto-vectors.json`
   的 `signatureSha256` 第 4、5 条）—— 这两条专门钉"version 不写死"。
2. **`encodeData` 只见过"加密手机号"这一种用法**（type 3 / type 6）。
   PC 源码里还有 `type == 2`（加密签名串），App 上**未观察到**。
   接新接口时若 `encodeData` 解出来不是 11 位手机号，**不要硬套**。
3. ~~**`signature` 是否覆盖请求体，未验证**~~
   → **已验证：不覆盖 body。**
   这一条不是靠探针，是靠**反证**：6165 条带签名的请求里，
   body 有 **6161 种不同取值**（`encodeData` 每请求都变），
   而 6165 条签名**全部**等于"仅那 5 个字段"的 SHA256。
   如果签名覆盖 body，那 6161 个互不相同的 body 不可能都算出同一个五字段哈希。
   ⇒ **签名与 body 无关，可以放心只签那 5 个字段。**
4. ~~**`service_identifier_key` 有 4 个取值**（`_userDomain` / `_caixun` / `_cloudDisk` /
   `_recommend`），接接口时先确认属于哪个服务，**不要钉成全局常量**。~~
   → **已查清：它不是逐接口的，而是与主机一一对应**（2026-09-19 全窗口统计）。

   | 主机 | `service_identifier_key` | 条数 |
   |---|---|---|
   | `middle.yun.139.com` | `service_identifier_caixun` | 6164 |
   | `portal-njs.yun.139.com` | `service_identifier_recommend` | 2 |
   | `user-njs.yun.139.com` | `service_identifier_userDomain` | 1 |
   | **其余 12 个主机** | **完全不带这个头** | 6063 |

   ⇒ 落地规则：**按主机查表**；表里没有的主机就**不发**这个头。
   （`_cloudDisk` 这个取值在本窗口**一次都没出现** —— 它有出处但无样本，别凭名字猜它配哪个主机。）
   注意"其余 12 个主机"里包括 `share-kd-njs.yun.139.com`（5991 条）、`ad.mcloud.139.com`（43 条）、
   `grdt.yun.139.com` 等 —— 也就是说 `pDynamicInfo/queryBatchList` 所在的 `grdt`
   **恰恰是不带的那个**，别看到"业务接口"就顺手加上。
5. **响应信封不是一种** —— 见 `docs/capture/app-api-families.txt` §4。

---

## 7. 活体探针：算法在**真服务器**上也成立

上面所有验证都只是"复现历史值"。为排除"服务端其实没在校验签名、
我们只是在自我印证"这种可能，用**自己算的签名**打了一次只读接口
（`general/getStatus`，与抓包里那条同接口同体）：

```http
POST https://middle.yun.139.com/openapi/general/getStatus
version: V1.0   clientId: iosapp   authType: 1   anonymous: 0
service_identifier_key: service_identifier_caixun
msgId: 2aeab39fa9f00ff9e3712b467df83f5e          ← 本次现生成
timeStamp: 1789781610762                        ← 本次现生成
signature: 7319f77601e642ebfdbacccc62592a50f6439df8de775e7dc0fd59a55498fc73
           = SHA256("clientId=iosapp&clientKey=BOFlEUINUdSugD6u&msgId=2aeab3…&timeStamp=1789781610762&version=V1.0")
Authorization: Basic base64("mobile:<手机号>:<authToken>")
```

响应：

```json
{"resultCode":200,"resultMsg":"success","resultData":{"status":0},
 "msgId":"2aeab39fa9f00ff9e3712b467df83f5e","success":true,"code":"200"}
```

**`resultCode: 200` + `msgId` 原样回传** ⇒ 服务端确实在校验签名，而我们算对了。
这条探针同时确认了两件小事：`msgId` 是**请求/响应关联 id**，
以及 `x-DeviceInfo` 用的设备串不影响该接口的判定。

> 探针是**只读**接口、用用户自己的登录态，且只打了一次；
> 按 §7.0 的精神，这类"验证已抓到的协议"的实验不算猜测性接口开发。

---

## 8. 第三把：`hcy-cool-flag` 信封（**方向是反的**）

这一把**不在形态 B 里**，密钥也**不是** `BOFlEUINUdSugD6u`。它此前被理解成
"请把响应加密给我"，**理解反了** —— 它声明的是 **"我这个请求体是加密的"**。

```
请求：hcy-cool-flag: <任意值>          ← 不发则 body 按明文解析
      body = base64( IV(16 字节，**二进制**) ‖ AES-128-CBC/PKCS7(
                       key = "uDjGSA8auFlbwDkY",
                       iv  = IV,
                       plain = 请求 JSON 的 UTF-8 字节) )

响应：同一个格式。**包装有两种**，解析时都要认：
      · 裸 base64 字符串（172 字符，无 JSON 引号）   ← 正常业务分支
      · JSON 引号包着的 base64（194 字符）           ← 错误分支
```

| 项 | 值 |
|---|---|
| 密钥 | **不止一把，按「谁发的」分** —— 见 §8.0 与 §8.1 |
| IV | 16 字节，拼在载荷前面。**原生**用随机**字节**（二进制）；**H5/WebView** 用随机**字符**（全可打印）|
| 方向 | **双向**（请求体 + 响应体都用它）|

### 8.0 ⚠⚠ 更正（2026-09-20）：钥匙**不止一把**，这一节写的那把只覆盖发码链路

上面那套格式（`base64(IV(16 字节二进制) ‖ AES-CBC/PKCS7(...))`、双向）是对的，
但**钥匙写窄了**。用旧项目 `mcloud-sign/har/`（`Authorization` 前缀 `mobile` ⇒ App 流量）
的实拍重跑：

| 钥匙 | 长度 / 算法 | 在 **396 条**带 flag 实拍上的命中 | 覆盖 |
|---|---|---|---|
| `uDjGSA8auFlbwDkY` | 16 字节 / AES-128 | **0** | 只在这节描述的**发码链路**上验证过（活体探针 D）|
| **`2olBaQGYnEKoStYomsd1n7ax`** | **24 字节 / AES-192** | **396 / 396** | `querySpecTokenV2`、`queryAvailableBenefit`、`queryUserBenefits`、`getUser`、`avatar/get`、`refreshToken`、`queryUserMemberCards` … 跨 10 个 HAR、30+ 个 URL×方向 |

⇒ **至少两把钥匙，按链路分**。判据只能是**逐条实拍**，不是"哪把更晚发现"。

第二把的出处值得单独记一笔：它就是旧项目 `packages/core/src/utils/encrypt.ts:3` 的
`REFRESH_TOKEN_KEY`，而同一文件导出的解码函数 `aesDecryptWithDynamicIV`
**全仓无人调用** —— 旧项目只用它**加密** note 的 refreshToken 体，从没用它解过码。
**钥匙一直在仓库里，缺的只是"把它和这个头连起来"这一步。**

黄金向量：`hcyCoolFlagBodyEnvelope192`（5 条实拍，双向）；Rust 消费者见
`legacy/crates/mcloud-core/src/crypto/tests/app.rs`。

### 8.1 ⚠⚠ 再更正（2026-09-20 同日）：第三把钥匙 —— **H5 / WebView 那一层**

§8.0 收尾时写下的"`user/getUser` 的体是 `base64(IV(16 个**可打印 ASCII**) ‖ CT)`，
形态像 `encodeData` 但 `clientKey` 也开不了" —— **现在解开了**，而且原因不在钥匙，
在**调用方**：那是 **H5 / WebView** 发的，和原生那两把**不是同一个调用方**。

**第三把钥匙**：

| 项 | 值 |
|---|---|
| 钥匙 | `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1`（**32 字节 → AES-256**）|
| 格式 | 与原生**完全同形**：`base64(IV(16) ‖ AES-CBC/PKCS7(key, IV, 明文))`，双向 |
| 出处 | iOS App 沙盒的 **H5 离线包**：`Data/Documents/AIOfflinePKG/stable/ai-helper/assets/index-af02d869.js` 里 `Kr.coolFlagPivotal` |
| 调用点 | `AES_CBCDecrypt({ data, key: coolFlagPivotal, getIVFromEncrypt: true })` |

H5 侧实现原文（同一文件，`AES_CBCDecrypt`）：

```js
AES_CBCDecrypt(e){
  let {data:t, iv:n}=e, {pivotal:o="", key:i="", padding:a="Pkcs7", getIVFromEncrypt:s=false}=e;
  o = o || i;                                  // 实际密钥 = pivotal || key
  const r = M.enc.Base64.parse(t);
  if (s) {                                     // getIVFromEncrypt
    n = M.lib.WordArray.create(r.words.slice(0,4));                        // IV = 前 16 字节
    t = M.enc.Base64.stringify(M.lib.WordArray.create(r.words.slice(4)));   // CT = 其余
  } else n = M.enc.Utf8.parse(n);
  return M.AES.decrypt(t, M.enc.Utf8.parse(o), {iv:n, mode:M.mode.CBC, padding:M.pad[a]})
          .toString(M.enc.Utf8).toString()
}
```

**⇒ 判据：三把钥匙是「谁发的」的函数，不是版本号。**

| 谁发的 | 钥匙 | 算法 | IV 来源 | 前 16 字节 | 头特征 |
|---|---|---|---|---|---|
| **App 原生**（`FXHttpBaseAccessor`）| `2olBaQGYnEKoStYomsd1n7ax` | AES-192 | 随机字节 | 二进制 | `x-yun-uni` + `x-yun-api-version` |
| **H5 / WebView**（`coolFlagPivotal`）| `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1` | **AES-256** | 请求：`Bn.generateRandomString(16)`（随机**字符**）| 请求**全可打印** | `origin: mcloudlocal://` + `sec-fetch-*` |
| **发码链路** `user/sms/getSmsCode` | `uDjGSA8auFlbwDkY` | AES-128 | — | — | — |

⚠ **指纹只在请求侧成立**：响应侧的 IV 由**服务端**生成，两族都是二进制，
所以**响应不能靠 IV 区分** —— 要么看头判断是哪个客户端发的，要么两把都试。

**四组交叉对照（全部干净，可在 Rust 里复现）**：

| | AES-256（H5 那把）| AES-192（原生那把）|
|---|---|---|
| WebView 族实拍 | **3 / 3 ✓** | 0 / 3 |
| 原生族实拍 | 0 / 2 | **3 / 3 ✓** |

覆盖面：在 14 条"可打印 IV"候选里 **11 条命中**（3 个接口）；
另外 3 条分别是微软遥测 ×2（与本协议无关）与 `ILog` ×1。
⚠ 其中 `ILog` 那一条的归类**已在 §8.2 更正**（它是原生族、IV 是二进制，
"可打印 IV"这个判断当时落在了 base64 字符串上而非解码后的字节上）。

⚠ **"前 16 字节可打印"不是本族的充分条件** —— `encodeData` 也是可打印 IV（§4）。
这条指纹只用于在**已知是 `hcy-cool-flag` 整包信封**的前提下挑钥匙。

**踩坑记录（值得单独记一笔）**：这一把之所以最后才找到，是因为我按
`[A-Za-z0-9]` 过滤候选钥匙 —— 而它**含一个 `+`**，被整批滤掉了
（21173 个候选 × 4 种模式全部落空，其中就包括真凶）。
**候选集的"字符类"和"长度档位"一样，是会漏掉答案的筛子。**

黄金向量：`hcyCoolFlagWebViewEnvelope256`（3 条，双向，已按脱敏 SOP 换值不换形）；
Rust 消费者见 `legacy/crates/mcloud-core/src/crypto/tests/app.rs` 的三条 `app_cool_flag_*` 测试。

⚠ **仍未解开**：`ILog`（`queryRights` / `IControl` / `thirdlogin` / `getSmsCode` 也各有解不开的条目）。
`ILog` 的体同样是"可打印 IV"，但它**不属于**这把钥匙 —— 说明 H5 侧还有别的信封。
**别把不同信封互相套用。**

### 8.2 ⚠⚠ 三更正（2026-09-20 同日）：钥匙还按**运行环境**切换；`ILog` 是原生族

#### 更正一：H5 那把钥匙不是常量，是「运行环境」的函数

H5 离线包 `index-af02d869.js` 原文（就在 §8.1 那把钥匙的定义旁边）：

```js
const Kr = {cpid:"255", puzzleCodePivotal:"CREATPUZZLE03F9A",
            userZonePivotal:"qPqDw263XgFgL3u8", clienttype:"937",
            coolFlagPivotal:"h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1", coolFlagTerminalType:"3"};
const _r = T("h5_env");
let $r = "10000000003";
!_r || "dev2Prod" === _r || (
    Kr.userZonePivotal = "2ErfJus1Ofr@2o24",
    Kr.coolFlagPivotal = "PVGDwmcvfs1uV3d1",   // ← 16 字节 ⇒ AES-128
    $r = "1111");
```

默认值出自同一文件的 `localStorage.getItem("h5_env") || "dev2Prod"`：

| `h5_env` | `coolFlagPivotal` | 算法 | `userZonePivotal` | `logProjectId` |
|---|---|---|---|---|
| 未设置 / `dev2Prod` | `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1` | **AES-256** | `qPqDw263XgFgL3u8` | `10000000003` |
| 其它任意值 | `PVGDwmcvfs1uV3d1` | **AES-128** | `2ErfJus1Ofr@2o24` | `1111` |

真机没设 `h5_env` ⇒ 走 `dev2Prod` ⇒ 用 AES-256 那把，与 §8.1 的实拍 3/3 命中一致。

> ⇒ **「按谁发的分」之外还有「按哪个环境分」这一维。**
> 只按"谁发的"挑钥匙，在把 `h5_env` 设过的设备上会**静默失效**。

#### 更正二：`ILog` 是**原生**请求，不是"H5 侧的可打印 IV 信封"

§8.1 收尾把 `ILog` 归成"体同样是可打印 IV，但不属于这把钥匙 ⇒ H5 侧还有别的信封"。
**这个归类是错的** —— 错在判据落在了 base64 字符串上，而不是解码后的字节上。

| 项 | 事实 |
|---|---|
| URL | `https://ose.caiyun.feixin.10086.cn/richlifeApp/devapp/ILog` |
| 头 | `x-yun-uni` + `x-yun-api-version` + `hcy-cool-flag: 1` + **`hcy-ios-option-enc: 2`** ⇒ **原生** |
| 日志里的体 | **明文 XML**（`<recordOprLogEx>` / `<key>` …）—— 再次印证"日志永远显示明文" |
| 线上体 | `base64(IV(16) ‖ CT)`，但 **IV 是二进制**（42/43 条） |
| **响应** | AES-192 `2olBaQGYnEKoStYomsd1n7ax` 解出 **40 / 65** 条 → `<?xml …?>\n<result resultCode="0" desc="SUCCESS"/>\n`（其余 25 条长度不对齐，非钥匙问题）|
| **请求** | **钥匙未知**（见下）|

**请求方向的穷举结果（全部落空）**：

- 11623 个「全可打印」候选（16/24/32 字符，取自 H5 全部 JS + iOS 二进制 `strings`）
  × {前 16 字节作 IV, 零 IV, 末 16 字节作 IV, ECB} —— **0 命中**；
- 再补**无填充 CBC** 与 IV 偏移 1 / 2 / 4 / 8 / 16 / 32 —— **仍 0**（排除了"填充非 PKCS7"这个解释）；
- 判据始终是**强判据**：明文里必须出现 `<recordOprLogEx`（App 日志给了明文）。

⇒ 钥匙很可能是**运行期派生**或**服务端下发**，不在可打印字面量里。
注意 `ILog` 多带一个 `hcy-ios-option-enc`，而二进制里存在按 option 取钥匙的选择器
`getAESKeyWithEncryptBodyOption:` —— **"option 号 → 哪把钥匙"这张表还没拿到**。

#### 判据教训（值得单独记一笔）

**「IV 可不可打印」必须落在 base64 解码后的字节上。**
base64 字母表本身就是可打印 ASCII —— 看 base64 字符串会**永远得到"可打印"**，
据此把 `ILog` 误判成 H5 族。**这正是上一轮犯的错。**
正确做法：先 `b64decode`，再看 `raw[..16]` 的字节值域。

#### 顺带一个正面结论

AES-192 这把钥匙的适用范围**按信封算，不按域名算** ——
`ose.caiyun.feixin.10086.cn`（OSE 平台）与 `yun.139.com` 是两套子系统，同一把钥匙照样能解它的响应。

黄金向量：`hcyCoolFlagBodyEnvelope192` 新增 1 条 **ILog 响应实拍**（跨主机证据，明文是通用成功 XML、无标识）；
`constants` 新增 `BODY_AES_KEY_128_H5_NONPROD`；Rust 侧新增两条测试钉住本节两个结论。

**怎么确认的**（三步，都可复现）：

1. **长度线索**：密文固定 144 字节 = 16(IV) + 128。128 字节明文对应 113~128 字节原文 ——
   而"同一份 JSON 的 `\uXXXX` 转义形态"是 140 字节 → PKCS7 补 4 → 128 ✔
   （原样 UTF-8 是 107 → 112，对不上）。
2. **已知明文暴力**：`scripts/crack-cool-flag-response.py` 取 iOS 二进制里
   201124 个 16/24/32 字符候选，判据是"解出的明文里必须出现 `200059554`"。
   唯一命中 `uDjGSA8auFlbwDkY`，解出：
   ```json
   {"success":false,"code":"01000019",
    "message":"动态解密错误，错误信息:Illegal base64 character 7b","data":null}
   ```
   `7b` = `{` ⇒ **服务端在 base64 解码我们的请求体，而它是明文** —— 语义到此明确。
3. **活体探针**：把请求体按该格式加密再发，服务端不再报 base64 错，
   而是正常处理并回 `200059554`（响应同样是加密的）。

矩阵（`scripts/probe-hcy-cool-flag.py`，合成号码，不发短信）：

| 请求体 | 头 | 响应 |
|---|---|---|
| 明文 | 无 | 明文 `200059554` |
| 明文 | `hcy-cool-flag: 1` | 密文 → `01000019 Illegal base64 character 7b` |
| 明文 | `hcy-ios-option-enc: 1` | 明文 `200059554`（**这个头不触发**）|
| **加密** | `hcy-cool-flag: 1` | 密文 → `200059554` ✔ |
| 加密 | 无 | 明文 `01000001 参数错误`（不带该头 ⇒ body 按明文解析）|

黄金向量：`test/golden/app-business-crypto-vectors.json` 的 `hcyCoolFlagEnvelope`
（3 条，**含 2 条实拍** —— 实拍明文里没有标识，这次不必合成）。

> **实现侧目前不发这个头。** 解开它的价值是"再也不用猜"，
> 不是"必须带上"（发码接口上不发时响应语义与抓包完全一致）。
> 完整证据见 `docs/capture/app-sms-login-samples.txt` §1.2。
>
> ✅ **2026-09-20：那条"仍未确认的矛盾"结案了 —— 它不是矛盾。**
> `app-sms-login-samples.txt` §1.2 (f) 记的是"App 自己带这个头、体是明文，响应却是明文"。
> 真相是：**App 日志记的是加密前的请求与解密后的响应**，所以它永远是明文。
> 线上两头都是密文（同一条 `user/getUser`：日志明文 vs 抓包密文，逐条对上）。
> ⇒ 那一节列的两个候选解释（服务端灰度 / 头集合是整理稿）**都不需要**；
> 需要改的是"拿日志当线"这个取证习惯。

