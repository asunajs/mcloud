# 移动云盘 PC 客户端：退出登录 + 短信登录协议

> 状态：**已确认**。证据链 = Reqable 抓包归档（实测）+ `app.asar` 源码交叉验证。
>
> - 采集时间：2026-09-17 22:05-22:07
> - 采集环境：Windows 11，客户端 `8.8.6.20260829`
> - **登录协议一族**（按端分文件，本篇是 **PC 客户端退出+短信**那一支）：
>   [`upstream-login-protocol.md`](./upstream-login-protocol.md)（PC 客户端扫码）、
>   [`upstream-web-qr-login.md`](./upstream-web-qr-login.md)（PC Web 扫码）、
>   [`upstream-app-sms-login.md`](./upstream-app-sms-login.md)（iOS App 短信）、
>   [`upstream-app-business-api.md`](./upstream-app-business-api.md)（iOS App 业务接口）。
>   全库索引见 [`docs/README.md`](./README.md)。
> - 依据 `docs/rust-rewrite-plan.md` §7.0：本文件全部来自实测，无猜测性内容

---

## 1. 退出登录

### 接口
```
POST https://user-njs.yun.139.com/user/logout
Content-Type: application/json
```
```json
{ "phoneNumber": "19900000000" }
```

源码依据：`apiServer.js` L155-165 `quitOutLogin()`：
```js
let data = { phoneNumber: window.userAccount || window.UserInfo.account };
Utils.sendRequest("/user/logout", data, { type: "json" })
```

### 实测响应：**失败**
```json
{"success":false,"code":"01000001","message":"用户token不能为空","data":null}
```

### 关键洞察：调用顺序
实测时序显示，客户端**先清空本地 token，再发 logout**，因此服务端收到时已无凭据，返回 `01000001`。

| 结论 | 说明 |
|---|---|
| logout 是尽力而为 | 失败不阻断退出流程 |
| 真正的退出在本地 | 清 token / 清账号 / 回登录页 |
| 实现应照抄此顺序 | 先清本地，再 fire-and-forget 调 logout |

### 退出后的行为
退出后客户端**立即重启扫码轮询**（回归登录页）：
```json
{"clientType":"656","cpid":"277","dycpwd":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
 "extInfo":{"ifOpenAccount":0},"loginMode":"0","msisdn":"","pintype":"21",...}
```
响应 `200059541`（未找到设备的二维码信息）。

### 错误码
| code | message | 语义 |
|---|---|---|
| `01000001` | 用户token不能为空 | token 已清或已失效（实测常态） |

---

## 2. 短信登录全链路

### 2.1 发送验证码

```
POST https://user-njs.yun.139.com/user/sms/getSmsCode
```
```json
{
  "phoneNumber": "19900000000",
  "random": "0aebc7fd57048e7b92fbd22efe",
  "mode": 0,
  "reqType": 3,
  "clientType": "656"
}
```

- `random`：实测 **24 位十六进制**，由两段 `Math.random().toString(16).substring(2)` 直接拼接（源码 L179-181），**不是 UUID**
- `mode: 0`、`reqType: 3` 为固定值

**成功响应**
```json
{"success":true,"code":"0000","message":"请求成功",
 "data":{"regFlag":null,"mode":"0","remainGetTimes":8,
         "verifyCode":null,"encrptVerifyCode":null,"objNumber":null}}
```
> `remainGetTimes: 8` —— **发码次数配额**，需在状态中跟踪。

**需要滑块时**
```json
{"success":false,"code":"200059554","message":"请先获取滑块拼图验证码","data":{"captchaType":2}}
```

**滑块校验超限时**
```json
{"success":false,"code":"200059553","message":"同一滑动拼图验证码在单位时间内校验失败次数超限","data":{"captchaType":1}}
```

### 2.2 获取滑块拼图

```
POST https://user-njs.yun.139.com/user/verfycode
```
```json
{ "account": "19900000000", "type": 2 }
```

源码依据 `apiServer.js` L485-500 `getVerfyCode()`：`type: 2` 表示滑块（注释：目前仅支持 2）。
响应字段映射（源码 L497-499）：
```js
obj.slidePuzzle = {
  backGroundImg: obj.data?.picture,
  puzzleImg:     obj.data?.puzzle,
  ...
}
```

### 2.2.1 响应里还有 `puzzleLeft`：**服务端把答案给了出来**（2026-09-18 实测）

一次真实调用（账号用 199 合成号段，本接口**不发短信**，所以随时可复现）拿到的 `data`：

| 字段 | 值的形状 |
|---|---|
| `puzzle` | PNG（`iVBORw0KGgo…`，6152 字符） |
| `picture` | **JPEG**（`/9j/4AAQ…`，11828 字符）—— 两张图格式不同 |
| `puzzleWidth` / `picWidth` / `picHeight` / `picType` | **字符串** `"96"` / `"680"` / `"400"` / `"png"` |
| `puzzleLeft` | base64，解码后**恰好 32 字节**（16 IV + 16 密文） |

`puzzleLeft` 用的是**同一个公开密钥** `CREATPUZZLE03F9A`（即 `encryptByAesCBC` 那一个），
但 IV 每次随机 —— 所以它只能用"解密"方向做向量：

```text
AES-128-CBC 解密 → 明文 "370"，而同响应里 picWidth = 680 ⇒ 落在图宽之内
```

**这是缺口横坐标，也就是服务端要的拖动答案。**

#### 已由一次真实挑战证实（2026-09-18 晚）

原先只有三条**间接**依据（`370` 落在同响应 `picWidth=680` 内、与归档的 `391`/`461` 同量级同结构、
两边同密钥同封装），属于强推断。当晚一次真实短信登录把它验成了事实：

| 步骤 | 观察到的结果 |
|---|---|
| 首发发码 | **被风控拦下**：`200059554 请先获取滑块拼图验证码` |
| 取图 + `decrypt(puzzleLeft)` | 自动解出坐标（无需图像识别、无需人工量像素） |
| 带 `puzzleVerfycode` 重发 | **`验证码已发送`，`remainGetTimes: 8`** |
| 输码登录 | 成功，`thirdlogin` 返回的 `data` 解密得 `userInfo` |

两条附带结论：

- **首批发码就可能被要求滑块**（本次即如此），所以它不是罕见分支，必须自动处理 ——
  手工量像素那套会让每次登录都变成人工介入；
- 被拦下的那次**不扣配额**（过滑块后 `remainGetTimes` 仍是 8），所以"换图重试"是廉价的。

> **两端独立复现（2026-09-19）**：App 端把同一条路又走了一遍 —— 它发的
> `/user/sms/getSmsCode` 与本文的 `/user/verfycode` **是同一台主机**
> （`user-njs.yun.139.com`），取图接口同样裸请求即可，`puzzleLeft` 同样用
> `CREATPUZZLE03F9A` 直接解出坐标，提交同样用固定 IV `E526E1F3F9765444`。
> 也就是说**这不是某一端的实现技巧，而是协议面的能力**：谁算得出答案，服务端就认。
> 详见 [`upstream-app-sms-login.md`](./upstream-app-sms-login.md) §7.2 与
> [`capture/app-sms-login-samples.txt`](./capture/app-sms-login-samples.txt) §2.1。

实现落点：`SliderPuzzle::answer()`（自检 + 解密）、`mcloud_app::login::sms_send_code`
（降级循环：取图 → 解坐标 → 重发 → 挑战作废则重新取图，上限 3 次）。

向量落库于 `test/golden/pcclient-crypto-vectors.json` 的 `puzzleLeftDecrypt`，
单测 `puzzle_left_decrypts_to_the_gap_coordinate` 盯着它；
`SliderPuzzle::answer()` 还会做两道自检（必须是十进制数、必须落在图宽内），
宁可报错也不把一个荒谬坐标发上去 —— 滑块失败次数是有限的。

> **这修正了本文原先的落地假设**（"把两张图交给用户看、让他量缺口位置"）。
> 那条路不是错的，只是当时没有实测响应时的保守设计：既要求人干预、又容易量不准。
> 实测一次就发现服务端直接把答案给了出来 —— 这正是"先抓包再写代码"的价值。

### 2.3 带滑块重试发码

在基础 body 上追加**加密后**的滑块答案：
```json
{
  "phoneNumber": "19900000000",
  "random": "94328e9cffb9c6c47b678cea52",
  "mode": 0, "reqType": 3, "clientType": "656",
  "puzzleVerfycode": "RTUyNkUxRjNGOTc2NTQ0NEA6F1MQJO7Ejcwfa1IsHCs="
}
```

> **源码警示**（L187-188）：
> ```js
> // 这里的字段要特别注意，区别正常验证码的，大坑！！！
> data.puzzleVerfycode = Utils.encryptByAesCBC(params.imageCode);
> ```
> 字段名是 `puzzleVerfycode`（注意拼写：`verfy` 不是 `verify`），且**必须 AES-CBC 加密**。

#### `puzzleVerfycode` 结构（本次实测解码）

base64 解码后**恰好 32 字节**，结构确定性拆分：

| 段 | 长度 | 内容 | 实测值（两次） |
|---|---|---|---|
| 前段 | 16 B | ASCII **IV（硬编码）** | `E526E1F3F9765444`（两次相同） |
| 后段 | 16 B | **答案密文**（AES-CBC 单块） | `f5d28017dd9de6b048681f09afe0fd18` / `403a17531024eec48dcc1f6b522c1c2b` |

**核心结论**：前 16 字节在两次请求间**保持不变**，后 16 字节随拖动轨迹变化。这与源码的 `encryptByAesCBC()` 完全吻合（16 字节 = AES 单块）。

> **补正（2026-09-17 晚，补到 `common/utils.js` 实现后）**：
> 前 16 字节**不是服务端挑战标识**，就是 `encryptByAesCBC` 里那一行硬编码常量：
>
> ```js
> encryptByAesCBC (plainText) {
>   const aesKey = 'CREATPUZZLE03F9A';
>   const aesIV  = 'E526E1F3F9765444';
>   ...  // AES-CBC/PKCS7，返回 base64(IV ‖ CT)
> }
> ```
>
> 用该 key/IV 反解上表两条**真实密文**，得到的明文是 `461` / `391`
> （拖动像素偏移；末尾 13 字节 `0x0d` 正是 PKCS7 填充）——
> 也就是说答案是**十进制坐标字符串**，而不是轨迹点序列。
> 原文"挑战标识"的推测来自当时只有密文、没有实现，属可理解的误判；
> 「前段两次相同」这个观察本身是对的，只是原因被解释错了。
> 向量落库于 `test/golden/pcclient-crypto-vectors.json`。

### 2.4 短信登录

```
POST https://user-njs.yun.139.com/user/thirdlogin
```
```json
{
  "clientType": "656",
  "cpid": "277",
  "dycpwd": "119301",
  "extInfo": { "ifOpenAccount": 1 },
  "loginMode": "0",
  "msisdn": "19900000000",
  "pintype": "5",
  "verType": "2",
  "version": "8.8.6.20260829"
}
```

| 字段 | 扫码登录 | 短信登录 |
|---|---|---|
| `pintype` | `21` | `5` |
| `dycpwd` | 32 位 HEX sessionId | **6 位短信验证码明文** |
| `msisdn` | `""` | 手机号 |
| `extInfo.ifOpenAccount` | `0` | **`1`** |

成功分支同扫码：`data` 为 AES 密文，解密得 `userInfo`。

---

## 3. 实测完整时序（含两次滑块）

| 序号 | 请求 | 响应 | 说明 |
|---|---|---|---|
| 253 | `logout` `{phoneNumber}` | `01000001` token 为空 | 退出（本地先清 token） |
| 255-259 | thirdlogin `pintype=21` | `200059541` | 退出后回登录页，重启扫码轮询 |
| 291 | 发码（无滑块） | `200059554` 要求滑块 | 首次触发风控 |
| 293 | `/user/verfycode` | 下发拼图 | 取滑块图 |
| 299 | 发码 + **滑块①** | **`200059553` 校验次数超限** | **第一次滑块失败** |
| 303 | 再发码 | `200059554` 又要求滑块 | 客户端自动重试 |
| 305 | `/user/verfycode` | 重新下发拼图 | **必须重新取图** |
| 307 | 发码 + **滑块②** | **`0000` 成功** `remainGetTimes:8` | **第二次滑块通过** |
| 313 | thirdlogin `pintype=5` `dycpwd=119301` | 成功 | 短信登录完成 |

**降级路径（必须实现）**：
```
发码 → 200059554 → 取滑块 → 带滑块发码
                    ↓ 200059553（超限）
              重新取滑块（新挑战）→ 再发码
```

> `200059553` 说明单个滑块挑战**有失败次数上限**，超限后该挑战作废，必须重新调 `/user/verfycode` 获取新挑战。客户端实测正是这么做的。

---

## 4. 错误码汇总（累计）

| code | message | 语义 | 出现场景 |
|---|---|---|---|
| `0000` | 请求成功 | 成功 | 全部接口 |
| `01000001` | 用户token不能为空 | 无凭据 | logout |
| `200059541` | 未找到设备的二维码信息 | 码未被扫 | 扫码轮询 |
| `200059542` | 二维码已失效 | 码超时 | 扫码轮询 |
| `200059544` | 二维码信息与登录二维码信息不一致 | sID 不匹配 | 扫码轮询 |
| `200059548` | 二维码信息未授权 | 已扫码待确认 | 扫码轮询 |
| `200059545` | 扫码登录过于频繁 | **限流**：该退避，换码重试只会更深地撞进风控。2026-09-18 实现时补（原文没有） | 扫码轮询 |
| `200059553` | 同一滑动拼图验证码在单位时间内校验失败次数超限 | 滑块挑战作废 | 发码 |
| `200059554` | 请先获取滑块拼图验证码 | 需滑块 | 发码 |
| `5010047` | 国家码和手机号不匹配或手机号格式错误 | 号码非法（**在发短信之前**就被挡下）。2026-09-18 实现短信登录时实测补（原文没有） | 发码 |
| `200059526` | （源码注释）CLOUD_LOGIN_CODE_PROTECT_USER | 账密登录保护 | 账密登录 |
| `200059537` | （源码注释）CLOUD_LOGIN_CODE_REGION_PROTECT | 异地登录保护 | 账密登录 |
| `200059504` | — | 客户端硬转为 `200059554`（源码承认的历史兼容） | — |

---

## 5. 落地建议（给 mcloud）

1. **退出登录**：本地清态优先，`logout` 调用 fire-and-forget，失败不阻断。
2. **发码**：`random` 用两个随机 hex 片段拼接（24 位），不要用标准 UUID。
3. **滑块**：`puzzleVerfycode` = `base64(IV ‖ AES-128-CBC/PKCS7(key='CREATPUZZLE03F9A', iv='E526E1F3F9765444', 像素偏移))`，
   注意字段名拼写 `verfy`。已在 `mcloud-upstream::login::puzzle_verfycode_encrypt` 落地，
   并用两条真实抓包密文做了逐字节重放断言。
4. **滑块重试**：捕获 `200059553` 后必须**重新调 `/user/verfycode` 换新挑战**，不能复用旧挑战。
5. **配额**：跟踪 `remainGetTimes`，避免无谓消耗。
6. **trait 映射**：滑块图片处理属 IO 层；挑战重试逻辑属 capabilities 层，可用 `Random` + `Clock` 单测。
