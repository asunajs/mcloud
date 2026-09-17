# 移动云盘 PC 客户端：退出登录 + 短信登录协议

> 状态：**已确认**。证据链 = Reqable 抓包归档（实测）+ `app.asar` 源码交叉验证。
>
> - 采集时间：2026-09-17 22:05-22:07
> - 采集环境：Windows 11，客户端 `8.8.6.20260829`
> - 配套文档：`docs/upstream-login-protocol.md`（扫码登录）
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
| 前段 | 16 B | ASCII **挑战 ID** | `E526E1F3F9765444`（两次相同） |
| 后段 | 16 B | **答案密文**（AES-CBC 单块） | `f5d28017dd9de6b048681f09afe0fd18` / `403a17531024eec48dcc1f6b522c1c2b` |

**核心结论**：前 16 字节在两次请求间**保持不变**，是服务端挑战标识；后 16 字节随拖动轨迹变化，为答案密文。这与源码的 `encryptByAesCBC()` 完全吻合（16 字节 = AES 单块）。

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
| `200059553` | 同一滑动拼图验证码在单位时间内校验失败次数超限 | 滑块挑战作废 | 发码 |
| `200059554` | 请先获取滑块拼图验证码 | 需滑块 | 发码 |
| `200059526` | （源码注释）CLOUD_LOGIN_CODE_PROTECT_USER | 账密登录保护 | 账密登录 |
| `200059537` | （源码注释）CLOUD_LOGIN_CODE_REGION_PROTECT | 异地登录保护 | 账密登录 |
| `200059504` | — | 客户端硬转为 `200059554`（源码承认的历史兼容） | — |

---

## 5. 落地建议（给 mcloud）

1. **退出登录**：本地清态优先，`logout` 调用 fire-and-forget，失败不阻断。
2. **发码**：`random` 用两个随机 hex 片段拼接（24 位），不要用标准 UUID。
3. **滑块**：`puzzleVerfycode` 需实现 AES-CBC 加密，注意字段名拼写 `verfy`。
4. **滑块重试**：捕获 `200059553` 后必须**重新调 `/user/verfycode` 换新挑战**，不能复用旧挑战。
5. **配额**：跟踪 `remainGetTimes`，避免无谓消耗。
6. **trait 映射**：滑块图片处理属 IO 层；挑战重试逻辑属 capabilities 层，可用 `Random` + `Clock` 单测。
