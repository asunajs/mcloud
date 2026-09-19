# 移动云盘 **App 端（手机客户端）验证码登录**协议

> 依据：`docs/capture/app-sms-login-samples.txt`（2026-09-18 22:54-22:56 实测抓包）
> + 反汇编 `MCLoginCryptoUtil` / `MCLoginService`
> + 活体探针（合成号码打真实上游）。
> 实现：`crates/mcloud-upstream/src/app_login.rs`、`app_client.rs`；
> 密码学原语：`crates/mcloud-core/src/crypto.rs`。

---

## 1. 一句话概括

**`/user/thirdlogin` 的 App 形态 = 明文 JSON → AES-128-ECB/PKCS7 → hex → 末 10 位移到最前。**
没有国密、没有会话密钥协商、没有 `yun-encrypt-*` 头。

---

## 2. 三层封装

```
第 1 层   明文 JSON（186 字节）
            ↓  AES-128-ECB/PKCS7，key = "lYzvvBMUjucw+h^R"（16 字节 ASCII）
第 2 层   384 个 hex 字符（192 字节）
            ↓  末 10 个字符移到最前
第 3 层   线上请求体（384 个 hex 字符）
```

响应是同一套的逆运算：

```
线上 data（13632 hex）  →  前 10 位挪到末尾  →  AES-128-ECB 解密  →  去 PKCS7  →  UTF-8 JSON
```

第 3 层那一手**不是加密**，只是一次字符串旋转。反汇编依据
（`-[MCLoginCryptoUtil privateEncrypt:isCurrentEnvironment:]` @ `0x1035F2388`）：

```objc
out = [self AESEncrypt:text isCurrentEnvironment:env];
p   = MAX(out.length, 10) - 10;
a   = [out substringFromIndex:p];                    // 末 10 位
b   = [out substringToIndex:p];                      // 其余
return [NSString stringWithFormat:@"%@%@", a, b];     // 末 10 位 + 其余
```

### 2.1 为什么必须做第 3 层

服务端先反旋转再解密。少做这一步 → `9103 输入参数不合法`（服务端拿到的不是合法密文）。
反做方向 → 同样 `9103`。

### 2.2 边界行为（照抄，不要"顺手校验"）

| 输入长度 | `privateEncrypt` | `privateDecrypt` |
| --- | --- | --- |
| ≤ 10 | 恒等变换（`p = 0`，两段拼回原串） | 返回空串 |
| > 10 | 末 10 位 + 其余 | 去掉前 10 位 + 前 10 位 |

实现侧对应 [`mcloud_core::shift_tail10_to_front`] / [`shift_head10_to_back`]，
两者对 ≤ 10 的输入都返回原串。

---

## 3. 完整流程

```
① POST /user/sms/getSmsCode          （明文 JSON）
     ├─ 成功 → 短信已发出
     └─ 200059554 → 需要滑块拼图，进 ②

② 开 WebView 加载 https://yun.139.com/fe/captcha/?data=<base64>
     data 里带着 {headers:{2 个头}, captchaType:2, params:{手机号…}}
     ↓  用户在 H5 里拖滑块；**短信是 H5 自己发的**
   回调 hcy_captcha_callback({code:200, msg:"获取验证码成功"})

③ 用户手输验证码 → POST /user/thirdlogin  （加密体）
     dycpwd = 6 位验证码明文
     ↓  200 + {"success":true,"code":"0000","data":"<6816 字节密文>"}

④ 解 data → 会话 JSON（authToken / userDomainId / routerInfo …）
```

**关键点：②之后不会重发 `getSmsCode`。** 原生侧只负责开 WebView 与收回调，
"过滑块 + 发短信"整件事在 H5 会话内完成。所以 `getSmsCode` 的
`puzzleVerfycode` 字段在整份抓包里**始终是空串**。

> ⚠ **但这是客户端的"选择"，不是协议的"限制"** —— 见 §7.2 的补测结论：
> 该字段**服务端是认的**，而且取图接口就在同一台主机上，
> 所以 App 端**有**纯协议通路（已跑通）。本文 §4.2 描述的 H5 路径
> 在实现侧已退化为**兜底**。

---

## 4. 接口

### 4.1 `POST https://user-njs.yun.139.com/user/sms/getSmsCode`

**请求体（明文，全部字段是字符串）**

```json
{"mode":"0","phoneNumber":"<11 位手机号>","reqType":"3","clientType":"412","random":"<5-6 字符>","puzzleVerfycode":""}
```

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `mode` | string | `"0"`。**PC 端这里是数字 0** |
| `reqType` | string | `"3"`。**PC 端这里是数字 3** |
| `clientType` | string | `"412"`（大写 T） |
| `random` | string | 实测见过 `qwert`(5) / `hOjseB`(6)，服务端不校验长度 |
| `puzzleVerfycode` | string | 首次为空串。**非空时必须填"已编码的拖动坐标"** —— 见 §7.2 |

**额外请求头**：抓包里 App 带 `hcy-cool-flag: 1`，但**实现侧不发它** —— 见 §7.1。

**响应**

| code | 含义 | 下一步 |
| --- | --- | --- |
| `0000` / `200` | 已发出，`data.remainGetTimes` 是配额余量 | 等用户输码 |
| `200059554` | 请先获取滑块拼图验证码，`data.captchaType: 2` | **取图 → 解坐标 → 带答案重发**（§7.2）；协议路径不通时才开 H5 |
| `200059553` | 同一滑动拼图验证码校验失败次数超限 —— **该挑战作废，必须重取图** | 重取图重试（有上限）|
| `9103` | 输入参数不合法 —— `puzzleVerfycode` 非空但**没有对应的活挑战**（伪造/过期答案）| 取新题，别拿旧答案硬试 |

### 4.2 滑块拼图（H5，**兜底路径**，不是唯一通路）

> 实现侧的正常路径是 §7.2 的纯协议通路；这一节描述的是 App 客户端的行为，
> 以及协议路径被限流时的退路。

```
https://yun.139.com/fe/captcha/?data=<base64(JSON)>
```

`data` 解出来：

```json
{
  "headers": { "x-yun-app-channel": "10000020",
               "x-yun-client-info": "1|192.168.3.179|2|12.6.0|…|aVBhZA==|" },
  "captchaType": 2,
  "params": { "clientType": "412", "reqType": "3",
              "nationCode": "+86", "phoneNumber": "19900000000" }
}
```

三个容易写错的地方：

1. `headers` 里**只有两个头**，不是请求头全集；
2. 这个 `x-yun-client-info` **末尾是 `|`，没有 `0|;`** —— 与请求头里的同名头不同；
3. `captchaType` / `params` 里的值是**数字 2** 与**字符串**，混着来。

回调：`hcy_captcha_callback({code: 200, msg: "获取验证码成功"})`。

### 4.3 `POST https://user-njs.yun.139.com/user/thirdlogin`

**请求体（加密前的明文）**

```json
{"cpid":"55","extInfo":{"ifOpenAccount":"1"},"dycpwd":"<6 位验证码>","pintype":"8","loginMode":"0","verType":"1","version":"12.6.0","msisdn":"<手机号>","random":"<6 字符>","clienttype":"412"}
```

| 字段 | 值 | 与 PC 端的差异 |
| --- | --- | --- |
| `cpid` | `"55"` | PC 是 `277`/`332` |
| `clienttype` | `"412"` | **全小写 t**；`getSmsCode` 里叫 `clientType` |
| `pintype` | `"8"` | **短信登录也是 8**，不是 PC 的 `5` |
| `dycpwd` | 验证码**明文** | PC 账密才做 SHA-1 |
| `loginMode` / `verType` | `"0"` / `"1"` | App 独有（PC 的 `verType` 是 `"2"`） |
| `extInfo.ifOpenAccount` | `"1"` **字符串** | PC 端是**数字** `1` |
| `random` | 6 字符 | 与 `getSmsCode` 的 `random` 不是同一个值 |

**请求体（线上形态）**：384 个**小写** hex 字符（186 字节明文 → 192 字节密文）。
`Content-Type` 仍然是 `application/json;charset=UTF-8` —— 内容是 hex 文本也照旧，
不要"顺手改成 `text/plain`"。

**请求头**（`docs/capture/app-sms-login-samples.txt` §3.2 有全集）：
没有 `yun-encrypt-index` / `yun-encrypt-key` / `x-MM-Source` / `x-yun-tid`。

**响应**：`data` 是密文（6816 字节），解密后是会话 JSON（见 §5）。

---

## 5. 会话载荷字段

| 字段 | 类型 | 形态 |
| --- | --- | --- |
| `account` | string | 11 位手机号（服务端回显） |
| `areaCode` | string | 3 位区号（由归属地决定，不是客户端传的） |
| `atExpiretime` | number | `2592000` |
| `expiretime` | **string** | `"2592000"` —— 与上一个同值不同型 |
| `authToken` | string | 201 字符：`前缀\|1\|RCS\|13 位毫秒\|载荷` |
| `belongsPlatform` | number | `2` |
| `cutoverStatus` | number | `1` |
| `deviceid` | string | 32 位**大写** hex（服务端侧的设备 id） |
| `hasChangeAccount` | number | `0` |
| `loginRequestVersion` | string | `"v2"` |
| `nationCode` | string | `"+86"` |
| `serverinfo.pingurl` | string | 长连接探测地址 |
| `userAssetFlag` | number | `0` |
| `userDomainId` | string | 19 位数字 —— **就是请求头 `x-yun-uni` 的值** |
| `userExtInfo[].passID` | string | 10 位数字 |
| `userid` | string | 12 字符 |
| `routerInfo[]` | array | `modName` = `personal` / `file-agent` / `shared`，每项含 `hashName`/`httpUrl`/`httpsUrl` |

---

## 6. 错误码

| code | message | 含义 | 排查方向 |
| --- | --- | --- | --- |
| `0000` / `200` | 请求成功 | 成功 | — |
| `9103` | 输入参数不合法 | **服务端解不开请求体** | ①体没加密 ②密钥用了备用那把 ③错位方向写反 |
| `9441` | 验证码不正确 | **体已解开**，码不对/过期 | 加密这一层没问题 |
| `9999` | 登录失败！ | 设备头缺失/不完整 | 补齐 `AppProfile` 的头 |
| `200059554` | 请先获取滑块拼图验证码 | 需要过滑块 | 开 H5 |

`9103` / `9441` 构成一个**二值判别器**：调加密参数时不需要真的登录成功，
只看回哪个码就能判断"服务端有没有解开这个体"。

---

## 7. 已知缺口（如实记录，不做猜测性补全）

### 7.1 ~~`hcy-cool-flag` 在 `getSmsCode` 上会切换响应形态，而密钥来源未知~~ → **已解开**

> **2026-09-19 深夜收口。** 结论不是"密钥来源未知"，而是**我们把它理解反了**：
> 这个头声明的不是"响应请加密给我"，而是 **"我这个请求体是加密的"**；
> 响应加密只是同一选项的另一半。密钥就是二进制里的字符串常量
> **`uDjGSA8auFlbwDkY`**（用已知明文暴力命中，201124 个候选里唯一一个）。
>
> ```
> 请求：hcy-cool-flag: <任意值>    ← 声明 body 已加密；不发则 body 按明文解析
>       body = base64( IV(16 字节，二进制) ‖ AES-128-CBC/PKCS7(key, iv=IV, 请求 JSON) )
> 响应：同格式；包装有两种（裸 base64 / JSON 引号包着的 base64），解析时都要认
> ```
>
> 决定性实验：把请求体按该格式加密后发出去，服务端**不再**回
> `01000019 动态解密错误…Illegal base64 character 7b`，而是正常处理并回
> `200059554`（且响应也是同格式加密）⇒ 算法在真服务器上成立。
>
> 证据、矩阵、向量：[`capture/app-sms-login-samples.txt`](./capture/app-sms-login-samples.txt) §1.2；
> 向量 `test/golden/app-business-crypto-vectors.json` 的 `hcyCoolFlagEnvelope`（3 条，含 2 条实拍）；
> 工具 `scripts/probe-hcy-cool-flag.py` / `scripts/crack-cool-flag-response.py`。

原先的记录（保留，因为"怎么错的"比结论更值得记）：

**在 `/user/sms/getSmsCode` 上**，这个头**只要出现**（值 0 或 1 都一样），
服务端就把响应从明文 JSON 换成**加密体**：

```
不带         → {"success":false,"code":"200059554","message":"请先获取滑块拼图验证码","data":{"captchaType":2}}
hcy-cool-flag → "<192 个 base64 字符>"        // 裸 JSON 字符串，144 字节密文
```

> ⚠ 这句话有两处不对，**都已在上面的收口里改掉**：
> 1. **因果反了** —— 加密的是**双向**，且这个头声明的是**请求体**；
> 2. **§1 那条抓包本身就是反例** —— App 带着这个头、体是明文，响应却是明文。
>    两个候选解释（服务端灰度 / 该头集合是整理稿而非同一请求）**都未确认**。

> ⚠ **别把这个结论推广到业务接口**（2026-09-19 更正）：
> 真实客户端在 `…/IOutLink/queryRecentFriendLinkList` 上
> **5991 / 5991 条**都带 `hcy-cool-flag: 1`，响应**全是明文 JSON**；
> `hcy/group/manage/*`、`isbo2/*` 同样带它。整个窗口里"裸 base64 响应"是 **0 条**。
> 所以这是**逐接口**的行为，不是全局能力声明。
> 详见 [`capture/app-api-families.txt`](./capture/app-api-families.txt) §3。

已排除的解（详见 `docs/capture/app-sms-login-samples.txt` §1.1）：
`MCLoginCryptoUtil` 那套、`MCStateCryptoHelper` 的 SM4-ECB（含带自己的
`yun-encrypt-*` 会话密钥）、`SecretKeyConfig` 全部静态密钥 × 多种模式。
响应头里也没有密钥。

> 另外记一条**错线**（已排除）：iOS 二进制里 `kRelease_A_ES_Key` /
> `kDebug_A_ES_Key` 就紧挨着 `hcy-cool-flag` 字符串，看着像答案；
> 但它们和 `kUserDefaultsKeyServerEnvironment` 一起落在 `NDDatabaseManager.m`
> 的编译单元里 —— 那是**数据库**加密的钥匙，与响应无关。

**实现侧的处理：在 `getSmsCode` 上仍然不发这个头。** 理由是"照抄抓包"与"能跑起来"在这里冲突，
而**不发**时响应语义与抓包完全一致（同一个 `200059554`）。
（业务接口上是否要带它，等接那些接口时按各自的实测再定 ——
现在即使要带也知道该怎么带了。）

### 7.2 非空 `puzzleVerfycode`：**曾经以为没有样本，其实有路**（已补测，已实现）

**原先的结论（错，保留在此作为教训）**：抓包里三条 `getSmsCode` 全是空串，
因为发短信是 H5 干的。所以实现侧只把它做成"可传入的字符串参数、**不做任何编码**"。

**错在哪**：把**客户端的行为选择**当成了**协议的能力边界**。
App 客户端开 WebView 是它的实现选择（面前坐着真人，它没必要自己算），
不代表服务端不认这个字段。

**补测（2026-09-19）三步就把它钉死了**：

1. **取图接口就在同一台主机上** —— `base::USER_YUN`（`user-njs.yun.139.com`）
   同时服务 `getSmsCode` 与 `/user/verfycode`，**裸请求即可**（无鉴权、无设备头、无 tid）；
2. **活体探针判别字段是否被读**：只改 `puzzleVerfycode` 一个字段，
   空串回 `200059554`，非空则**不再回它** ⇒ 服务端确实在处理；
3. **编码对齐 PC 端**（同一个第一方滑块）：实测取新题解出 `466`、
   按 `base64(固定IV ‖ AES-128-CBC/PKCS7('CREATPUZZLE03F9A', 固定IV, 坐标))` 填进去，
   上游回 `{"success":true,"code":"0000","data":{"remainGetTimes":8,...}}` —— **短信真的发出去了**。
   PC 端早有一条同构的记录（`docs/upstream-logout-sms-login.md` §2.2），
   这次是**独立复现**：两端走的是同一台主机、同一个密钥、同一个提交格式。

完整四步与两个 IV 的区别见
[`capture/app-sms-login-samples.txt`](./capture/app-sms-login-samples.txt) §2.1。

**实现侧已改为"协议优先、H5 兜底"**：`AppSendSmsOutcome::NeedCaptcha` 之后
自动取图 → 解坐标 → 带答案重发（有上限，因为该接口**计失败次数**），
只有重试用尽才把 H5 地址交给用户。

> ⚠ **两条判据别搞反**（都是这次踩出来的）：
> - 字段在样本里**恒为空**，只说明**那个客户端**没填，**不能**说明服务端不认；
> - `captchaType` 从 2 变 1 **不是**"换了验证码类型"的信号 ——
>   PC 端夹具里 `200059553` 也带 `1`，它只是"非初始错误"的通用取值。

### 7.3 Android 端没抓

`clientType`/`cpid`/设备串语法在 Android 上是另一组值，所以 `app_client.rs` 里
**没有** `Android` 变体 —— 有实测了再加。

### 7.4 `x-yun-uni` 的取值来源：**已答**（就是登录响应自己给的）

抓包里它 = 响应里的 `userDomainId`，但**首次**登录时客户端从哪拿到它没有观察到
（当时的猜测是"本地缓存的上一会话"）。实现侧因此把它做成可选项：不知道就不带，
而不是编一个值。

> **补测确认（2026-09-19）**：`x-yun-uni` 头、请求体里的 `userDomainId`、
> 请求体里的 `userId` **是同一个 19 位值**（同一份日志里 5839 / 11982 / 38 次）。
> 也就是说它不是"另一个 id"，而是同一个 id 的三个名字 ——
> 这条同时补上了 `docs/rust-rewrite-plan.md` §7.2 那个"`user_domain_id` 落空"的缺口：
> **任何一条已登录请求的头里就带着它**，不必为此再登录一次。
> 计数与出处见 [`client-profiles.md`](./client-profiles.md) §3.3.2。

> **真机登录收口（2026-09-19）**：跑通一次完整的 App 短信登录后，
> **`userDomainId` 就在 `thirdlogin` 的响应里** —— 猜测里那个"本地缓存的上一会话"
> 是多虑了。CLI 打印的响应字段清单同时证实了另外两件事：
>
> - 字段名是 `userDomainId`，而 `userId` **根本不在响应里**（详见
>   [`upstream-login-protocol.md`](./upstream-login-protocol.md) §5 的收口说明）；
> - `userid`（全小写）是**另一个**字段，与 `userDomainId` 同时出现、值不同。
>
> 落库结果：`account_state.user_domain_id` 有值（19 位），`jwt_token` 201 字符，
> `auth_expire` 正好在 30 天后 —— 与 PC 扫码登录那次（§3.4 / `upstream-login-protocol.md` §5）
> 的各项数值一致。
>
> **所以"首次登录拿不到"只对登录握手期间的请求成立**（那时确实还没有这个值，
> 编一个会让上游定位到别人的用户域）；一旦 `thirdlogin` 返回，它就有了、也存下来了。

> ⚠ **一个尚未接上的环节（留给代码线）**：`AppProfile::with_user_domain_id` 目前
> **只在测试里被调用过** —— 生产路径没有把库里存的 `user_domain_id` 回填进 App 档案。
> 现在还不痛，因为 App 端**还没有任何已登录接口**（CLI 只有 `login` / `fixture` /
> `migrate` / `capabilities` / `user`）。
> 但下一批抓包（签到、任务列表等）一落地，第一个已登录的 App 请求就会缺 `x-yun-uni`。
> 届时先接这条线，再写接口。

### 7.5 `random` 的作用不明（**一半已答**）

原先的表述是"两个接口的 `random` 长度都不一样（5 / 6），看不出参与校验"。补测后可以收窄：

- **`getSmsCode` 的 `random` 是硬编码常量 `qwert`** —— 跨 2026-08-12 ~ 09-18 的
  **4 次**抓包逐字节相同。一个真正随机的值不可能跨一个月不变。
  所以它**不做成参数**（`app_login::SEND_SMS_RANDOM`）：参数会暗示"这里该变化"，
  而客户端从不变化它。
- **`thirdlogin` 的 `random` 只有 1 个样本**（`hOjseB`，6 字符）。
  "每次不同"是从"客户端为什么要随机"推的，**不是实测到的** ——
  它是否为定值需要第二次登录抓包才能确认。

服务端不校验这个字段（否则 `qwert` 这个常量早就该被拒），所以实现侧只需要形状对；
但**上面第二条的诚实标注要保留**。

### 7.6 `routerInfo` 的消费方式未实现

它属于登录**之后**的接口寻址，不在登录链路的输入里。

### 7.7 登录之后用什么头（**已答，记在这里便于回查**）

`thirdlogin` 链路本身不带 `Authorization`，但**已登录**的请求带，形态是
`Basic base64("mobile:<手机号>:<authToken>")` —— 第一段是 `mobile`（PC 端是 `pc`），
`authToken` 与响应里那个逐字相同。

证据与"已登录请求的头超集"（`version` / `clientId` / `authType` / `anonymous` /
`timeStamp` / `msgId` / `signature` / `service_identifier_key`）见
`docs/capture/app-sms-login-samples.txt` §3.5；端对照表见 `docs/client-profiles.md` §3.3.1。

仍未解的是 **`signature`（64 位小写 hex）与 `msgId` 的算法** ——
它不影响登录链路，但会影响后续所有业务接口。

---

## 8. 怎么复核（不需要成功登录）

```bash
# 1. 离线：黄金向量 + 逐字段断言
cargo test -p mcloud-core crypto::tests::app_login
cargo test -p mcloud-upstream app_login
cargo test -p mcloud-app app_login

# 2. 联网：合成号码打真实上游，只看错误码
#    （见 crates/mcloud-upstream/src/app_login.rs 的测试与
#      docs/capture/app-sms-login-samples.txt §6.2）

# 3. 端到端：走真实 CLI（合成号码；设备与库都写到 /tmp，不碰本机数据）
cargo run -q -p mcloud-cli -- login sms --client app \
  --phone 19900000000 --db /tmp/probe-app.db \
  --app-device-file /tmp/probe-device-app.txt
```

第 3 步的期望输出（**实测过的**）：

```text
正在以 iOS App 身份登录（deviceId FEAE36BB5A026AE7D5EB50BBFF59E295）
上游要求先过滑块拼图（captchaType=2）。请在浏览器打开下面这个地址，拖动拼图完成验证：

https://yun.139.com/fe/captcha/?data=…

（**短信由该页面发出**，完成拖动后回到这里继续。不要再重新执行本命令）
请输入收到的 6 位验证码: 错误: 参数错误: 验证码不正确或已过期（9441）。加密这一层是通的，重新取一次验证码即可
```

最后那行 `9441` 就是判据：**它证明服务端成功解开了本实现产出的请求体**。
（标准输入给空即可，不需要真的收到短信。）

第 2 步的判据：

| 送出的体 | 期望响应 |
| --- | --- |
| 明文 JSON | `9103` |
| 加密体 + 备用密钥 | `9103` |
| 加密体 + `lYzvvBMUjucw+h^R` | **`9441`** |

只要第三行是 `9441`，密钥、模式、错位方向、hex 大小写就全部正确。
