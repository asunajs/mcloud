# 设备指纹与落地形状（§5 指纹 · §6 目标形状 · §7 未知项 · §8 与代码的关系）

> 状态：与 [`client-profiles.md`](./client-profiles.md) 同一套判据、同一轮取证，2026-09-22 从该文件逐字拆出。要接新端或动设备串，先读那篇的 §1 ~ §4，再读本篇。

> **本篇 2026-09-22 拆成两篇**（内容逐字未改，只按主题分段）：
> [① 端差异与请求头（§1 ~ §4）](./client-profiles.md) ｜ [② 设备指纹 · 落地形状 · 未知项 · 与代码的对应（§5 ~ §8）](./client-profiles-fingerprint-and-landing.md)
> ⚠ **`§` 编号沿用拆分前那一版**（§1 ~ §8 连续），所以「§5.2」「§7」这类引用
> 仍然有效，只是落在不同的文件里。

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
| 0, 1 | 空串（前导 `\|\|`） | 固定 |
| 2 | `11` | 固定（疑似系统/环境枚举） |
| 3 | 客户端版本 | `appVersion` |
| 4 | `PC` | 端标识 |
| 5 | **base64(设备型号码)** | 兜底模板里是固定常量 `BF-202003101647` |
| 6 | **deviceId** | `windowMachine`（与 `x-yun-device-id` 同值） |
| 7 | 空串 | — |
| 8 | OS 描述 | 原生接口返回，如 ` Windows 11 (10.0.26100.1591)`（**前导空格**） |
| 9 | 分辨率 | 如 `1920X1020` / `1920X1040` |
| 10 | **base64(语言)** | 兜底模板里是 `Chinese (Simplified)` |
| 11-13 | 空串（尾部 `\|\|\|`） | 固定 |

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
| 13 | 空串（原串以 `\|` 结尾） | — | 同 PC 端 |
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
   代码里只有 `osVersion` 一个字段 + 三段拼接（`AppDevice.DeviceInfo` / `XUserAgent` / `YunUserAgent`）。

> **代码位置**：`upstream.AppDevice`。
> `DeviceID` / `DeviceShort` 由调用方注入（生命周期不同，见 §5.4），
> 其余字段有实测默认值（`upstream.NewIPadAppDevice`）。

### 5.4 持久化要求（**两端不一样**）

**PC**：`windowDeviceInfo` / `windowMachine` 存在 `localStorage`，
**必须在首次运行后持久化且永不漂移**：换了 deviceId 等于换了台设备，
服务端会按 `dID + sID` 联合定位会话（见 `docs/upstream/login-protocol.md` §4 的
`200059541 未找到设备的二维码信息`）。

**App**：只有第 6 段（`deviceId`）持久化，第 7 段（`deviceShort`）**不持久化**。

| 段 | 生命周期 | 实测依据 |
|---|---|---|
| 6 `deviceId` | **持久化，跨月不变** | 同一台设备在 2026-08-12 与 2026-09-18 都是 `67F4C56A…315C` |
| 7 `deviceShort` | **每次运行现生成** | 同一个 `deviceId`，08-12 是 `8A708867DBC3`、09-18 变成 `EAB9BF94B8DE` |

> 把 `deviceShort` 也持久化，等于**声称一个已被证伪的稳定性** ——
> 所以实现上刻意只存 `deviceId`（`app.LoadOrCreateAppDeviceID`），
> short 每次运行用 `app.NewAppDeviceShort` 现生成。
>
> **待确认**：它到底按什么变（每次启动 / 每天 / 某个事件）？
> 目前只有两个观测点，定不下来 —— 记在 §7。

**③ 上面两段讲的是设备串（`deviceInfo`）里的字段。还有第三种指纹**，
与它**完全不同源、不同形状、不同生命周期**：cMPassport 风控侧的 `smid`。

（2026-09-19 追加，Go 侧实现见 `internal/upstream/smid.go` 与 `internal/app/smid.go`；
Rust 基准在 `../mcloud-rust/crates/mcloud-upstream/src/smid.rs` 与
`../mcloud-rust/crates/mcloud-app/src/smid.rs`。）

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
> 所以 `AppProfile.Headers()` 保持**不发** `deviceid`，
> 等某一条**原生**抓包明确带上了，再由那个接口的构造函数按端点显式带上 ——
> 而不是往公共头里塞一个只来自旧项目、且我们从未测到的值。

---

## 6. 落地时的目标形状（三层；PC / iOS App 已落地，其余端未实现）

| 层 | 内容 | 生命周期 | 已落地的例子 |
|---|---|---|---|
| **每端常量** | 端标识、版本号、渠道号、`clientType`/`cpid`、UA 模板、`deviceInfo` 模板 | 编译期 | `PcPlatform` 的 `ClientType()` / `Cpid()` / `AppVersion()`；`upstream.IOSClientType` / `IOSCpid` / `IOSAppVersion` / `IOSChannel` |
| **每安装指纹** | `deviceId`、`deviceInfo`、听云 id | 首次生成 → 持久化，不可漂移 | `windowMachine`；`AppDevice.DeviceID()` |
| **每运行** | App 设备串第 7 段（`deviceShort`） | 每次运行生成，**不落盘** | `app.NewAppDeviceShort` |
| **每请求** | `x-yun-tid`、`Authorization`/`jwttoken`、`timestamp`/`nonce`/`signature`、`msgid` | 每次计算 | 登录链路的 `x-yun-tid`（PC）；`x-yun-user-timestamp`（App，由调用方传 `timestamp` 注入） |
| **每账号**（2026-09-19 追加） | smid 的**设备特征**（~31 字段，见 §5.4 ③） | 首次生成 → 落 `account_state.smid_fingerprint`，**可主动换** | `app.EnsureFingerprint` / `app.RotateFingerprint`（`internal/app/smid.go`） |
| **每上报**（2026-09-19 追加） | smid 的 `deviceId`（`B…`，服务端下发） | 每次上报都可能换；旧值经 `box` 回喂 | `app.DeviceID`（库里有就不再打网络） |

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

> **App 端的实现方式与 PC 端有意不同**：`AppProfile.Headers(timestamp)` 是**纯函数**，
> 时间戳由参数传入而不是内部读时钟 —— 这样请求头可以**逐字段断言**（L2 夹具回放的要求），
> 也让"同一时刻的两个请求必须同头"变成可测的性质。PC 端后续改造时可对齐这个形状。
>
> 两端**还没有**抽出共同的 trait。现在抽是过早的：PC 与 App 除了"都叫 profile"之外，
> 头的名字、值的语法、加密方式全不一样，共同点只有返回类型那一层
> （Go 侧 `[]core.HeaderPair`；Rust 侧 `Vec<(String, String)>`）。
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
  [`app-business-api.md`](./upstream/app-business-api.md)；
- **安卓 / 小程序端 UA 完整串**（PC 与 iOS App 的已实测拿到，见 §3.3）；
- **App 设备串第 7 段（`deviceShort`）按什么变**：已知它**不是**每安装稳定值（§5.4），
  但只有两个观测点，定不出规则；
- ~~业务接口请求体里的 `encodeData` 用什么算法/密钥~~
  → **已解开（2026-09-19）**：`base64(16 字符 ASCII 的 IV ‖ AES-128-CBC/PKCS7(key, iv, 手机号))`，
  6159/6159 个样本全部解出、明文去重后只有 1 种长度 11（即手机号）；见
  [`app-business-api.md`](./upstream/app-business-api.md)；
- ~~**`hcy-cool-flag` 触发的那条加密响应用什么密钥**（App 端 `getSmsCode` 的坑，
  详见 `docs/upstream/app-sms-login.md` §7.1）—— 这是登录链路上**唯一**没解开的加密点。
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
- **原生侧 smid 特征是什么形状**（同期追加）：`upstream.CaiyunAndroidWebFingerprint` 那份是
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

工具已就位（`scripts/applog-capture.py`），把上面两件事都兜住了 ——
**逐条命令、`--all` / `--show-secrets` 的口径、"窗口按体积滚"这条判据**都归
[`capture-tools.md`](./capture-tools.md) §2，本文不复述（复述就会和那边对不上）。
上面那两组实测数字（轮询占比、43 MB 覆盖多长）**仍以本文为出处**，那边只引不抄。

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

- `internal/upstream/login.go`（PC 链路）**只发 `x-yun-tid`**（证据 D 是该链路唯一实测到的头）。
  不要往里摊 PC 的那一堆头 —— 否则别的端接进来时会被迫改登录逻辑。
- **App 链路的头由 `upstream.AppProfile`（`app_client.go`）提供**，
  请求构造函数（`upstream.AppSendSmsCodeRequest` / `AppThirdLoginRequest`）通过
  `upstream.WithAppHeaders(request, profile, timestamp)` 叠加，
  这样"换端"= 换 profile，不动业务逻辑。**登录链路的两个函数没有硬编码任何头值。**
- `PcPlatform` 的 `ClientType()` / `Cpid()` / `AppVersion()` 已经是"每端常量"的雏形；
  App 端照同样思路落成了 `app_client.go` 里那批 `IOS*` 常量（Go 没有 Rust 那个 `ios` 子模块，
  同名常量的前缀就是 `IOS`）。第三端进来时再考虑合并成一张表。
- **smid 那条线（§5.4 ③）目前不挂在任何请求链路上**，它是独立的一层，
  等哪个原生接口要用 `deviceid` 时再接上去：

  | 层 | 位置 | 职责 |
  |---|---|---|
  | 算法 | `internal/upstream/smid.go` | 特征体 → `data` → 上报 → `B…`；**只管一次调用，不碰库** |
  | 生命周期 | `internal/app/smid.go` | 读库里的特征、没有就生成并绑账号、上报、存 `deviceId`；`rotate` 时同一条语句清掉旧 `deviceId` |
  | 落库 | `account_state.smid_fingerprint` / `smid_device_id`（**v5 引入，schema 现为 v9**） | 见 §6 那两行新层 |
  | 入口 | `mcloud smid show / reset [--report] / refresh` | 只打印摘要（特征 fp 摘要 + 平台/尺寸/dpr/触屏/CPU + 打码的 `deviceId`），**不打印 UA** |

端档案的调用链（**从 CLI 一路到上游**，可以直接照着读）：

| 层 | PC | App |
|---|---|---|
| 端常量 | `upstream.PcPlatform` | `app_client.go` 的 `IOS*` 常量组 |
| 端档案 | `upstream.ClientProfile` | `upstream.AppProfile` |
| 请求构造 | `internal/upstream/login.go` | `internal/upstream/app_login.go` |
| 驱动 | `app.SmsSendCode` / `app.SmsLogin` | `app.AppSendSmsCode` / `app.AppSmsLogin` |
| 落库 | `capabilities.PersistLogin`（前缀 `pc`） | `capabilities.PersistLoginWithPrefix`（前缀 `mobile`） |
| 入口 | `mcloud login sms --client pc` | `mcloud login sms --client app` |
