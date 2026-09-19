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
| 密钥 | **`uDjGSA8auFlbwDkY`**（16 字符 ASCII；与 `clientKey` **不同**）|
| IV | 16 字节**二进制**，拼在载荷前面 —— ⚠ **不是** `encodeData` 那种"16 个可打印 ASCII" |
| 方向 | **双向**（请求体 + 响应体都用它）|

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
> 完整证据与一处**仍未确认**的矛盾（App 自己带这个头却拿到明文响应）
> 见 `docs/capture/app-sms-login-samples.txt` §1.2 (f)。

