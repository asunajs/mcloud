# CLI 用法

> 状态：**规范**（命令行操作手册；实现进度见 [`status.md`](./status.md)）
>
> - 入口：`go run ./cmd/mcloud -- help`（或 `go build ./cmd/mcloud` 后的 `./mcloud help`）
> - （2026-09-21 前本文写的是 Rust CLI 的 `cargo run -p mcloud-cli`；Rust 版命令在
>   同级仓库 `../mcloud-rust/` 下照原文可用。）
> - 相关：[`credential-boundary.md`](./credential-boundary.md)（哪些值不能进版本库）

---

## 最常用的一条：什么都不带

```bash
go run ./cmd/mcloud                          # = mcloud signin，对**全部启用**的账号各签一次
go run ./cmd/mcloud -- --list                # 先看下标：1、2、3…
go run ./cmd/mcloud -- --account 2           # 只跑下标 2 那个
```

**顶层与 `mcloud signin` 收同一套参数**，所以上面两条也可以写成
`mcloud signin --list` / `mcloud signin --account 2` —— 两种写法等价，随便用哪种。
（`mcloud --list` / `mcloud --status-only` / `mcloud --json` / `mcloud --db <路径>`
全都成立；实现上顶层那个 `SigninArgs` 与子命令里的**是同一个结构体**，
默认值与互斥关系只有一份定义。）

不带子命令就是**跑签到**，一个参数都不用给 —— 对齐旧项目 `mcloud-sign` 的用法。
账号用**下标**指代（不是掩码 id）：`账号 1` / `账号 2`，与旧项目的日志语义一致。

一个账号失败**不会**让其余账号停下，最后用非零退出码统一报"有几个失败"
（定时任务靠它判断这次到底有没有全部成功）。`--json` 时单账号给**对象**、
多账号给**数组**，进度行一律走 `stderr` —— 所以 `mcloud --json | jq` 是安全的。

---

## 子命令一览

| 子命令 | 作用 |
| --- | --- |
| `user init` / `user list` | 建首个管理员（网页端要用，也是历史账号的认领触发点）/ 看看有哪些人 |
| `login qr` | 扫码登录移动云盘账号：终端出码，手机扫（`--qr-out` 可另存一张图片） |
| `login sms` | 短信验证码登录：发码 → 输验证码。被风控要求滑块时**自动过**（缺口位置由服务端加密下发，见 `logout-sms-login.md` §2.2.1）。⚠ `--client app` 那个"自动过"是**借 PC 端那套**取题与编码、换 App 的头发出去（围魏救赵）；App 自身的滑块**未实现自动化**，真机走的仍是开 WebView 由 H5 发码那条（见 `capture/app-sms-login-samples.txt` §2.2 的澄清） |
| `migrate` | 把旧项目 mcloud-sign 的数据迁移到 SQLite（`--dry-run` 预演） |
| `signin` | 签到（云朵中心）。**不带任何参数就是对全部启用账号各签一次**；`--account <下标>` 只跑一个（`--list` 查下标），`--phone <完整手机号>` 给老账号补一次。另有 `--status-only` / `--force` / `--json`。**纯协议**，不开浏览器、不开 WebView；成败只看回查 `infoV3`（见 [`status.md`](./status.md)） |
| `smid show` / `reset` / `refresh` | 看每个账号绑定的**设备特征**（cMPassport 风控那套，旧项目拿它的结果当请求头 `deviceid` 发）、`reset` 换一个（旧的 `deviceId` 当场作废，`--report` 顺手再上报）、`refresh` 不换特征只再上报一次。输出**只有摘要**：UA 与 `canvas` 识别位不落终端。⚠ 本项目目前**还没有任何请求发这个头**，取值的那一半已就位（见 [`client-profiles.md`](./client-profiles.md) §5.4 ③） |
| `fixture extract` | 从 HAR 流量包抽取脱敏 fixture（用于离线回放）。脱敏有残留则 fail closed，拒绝写出 |
| `capabilities` | 打印能力注册表（`--json` 与 `GET /api/capabilities` 同形状） |
| `version` | 打印版本与运行时信息（`--json` 与 `GET /api/version` 一致） |

---

## 签到（`signin`）—— 七个要记住的

链路：**换 SSO token → 取服务端时间戳 → 签名建会话 → 查 `infoV3` → `startSignIn` → 回查 `infoV3`**。
全程纯协议、无浏览器、无 WebView（协议与坑见 `capture/app-signin-samples.txt`）。

### 1. 成败只看回查 `infoV3`

`startSignIn` 的 `todaySignIn` 是调用**前**的快照 ——
实测 `todaySignIn:false` 的那次恰恰把今天签上了；只看 `code == 0` 也不行，
上游对"今天已经签过"同样回 `success`。

### 2. 账号用 `--account <下标>` 给

`--list` 查编号。

**v7 起签到只用库里存的整串 `Authorization`**（见下面第 7 条），所以正常情况下什么参数都不用给。

`--phone <完整手机号>` 只留给"库里既没有整串、也没有完整手机号"的老行 ——
用它补一次就会**记进库里**（**只记号码**，见第 7 条最后一段），下次不必再输。

### 3. 端不匹配那道闸只管"现拼"那条路

签到用整串时前缀是登录时拿到的原文，不存在配错的可能 ——
**实测（2026-09-19）`pc:` 前缀的整串在签到接口上完全有效**
（旧项目也从不区分：它的 `asign.json` 里两个账号一个 `mobile:` 一个 `pc:`，
走的是同一条签到路径）。

只有**退路**（用 `phone` + `jwt_token` 现拼）才把前缀硬编码成 `mobile`，
那时若账号是用 `--client pc` 登录的，会被明确挡下来并提示重登 ——
这个组合没实测过，宁可拒绝也不要发出去试探（失败会长得像"未登录"）。

### 4. 每次查询都会顺手记一次余额

`app.RecordCloud`：余额 → `accounts.cloud_current` + `cloud_stats` 日快照
（日增 / 周增在 `core.NextCloudStats` 里算，是纯函数）。

> ⚠ **2026-09-22 按代码订正两处**：① 上面这两处函数名当时是 Rust 路径
>   （`mcloud-app::cloud::record_cloud` / `mcloud-core::pure::next_cloud_stats`），
>   Rust 已剥离为同级仓库，落点改成本仓库的 `app.RecordCloud` / `core.NextCloudStats`。
>   ② 原文写的是 `infoV3.beforeTotal`，而 `runStatusOnly` 实落的是
>   **`status.BeanTotal`（上游 `total`）** —— `run_signin.go:631` 那条注释就是为了
>   挡住这个：`beforeTotal` 不随领取变动，记它等于让趋势图永远是一条直线
>   （`run_signin.go:645`；`beforeTotal` 这个名字现在挂在 `--raw` 的
>   `CloudTotal` 上，`signin_render.go:453-458`）。下面那段「`before` 指领取前」
>   的语义仍然成立，成立的是**上游那个字段**，不是我们落库选的那个。

注意那个 `before` 指的是**领取前**、不是"签到前"—— 签到只是打卡、云朵要另外领，
所以它签到前后**不变**，不是"签到完余额就该 +3"。

落库失败只打一句 `stderr` 警告，**不影响签到结论**
（余额是顺带采集的副产品，不该把一次成功的签到变成失败）。

### 5. 领云朵是另一个接口（`receiveV3`），而且必须带 H5 那个 `deviceId`

`--receive` 打开。实测三档（2026-09-19，只换 `deviceId` 一项）：

| `deviceId` | 结果 |
| --- | --- |
| 不带 | `615` |
| 内容错一个字符 | `614` |
| 真值 | `code=0` |

⇒ **上游校验的是内容，不只是"非空"**，"随便编一个"走不通。

⚠ 那两个码的 msg 都是"活动太火爆啦…"，会把人的思路带到"限流 / 退避重试"上，
而真实原因是**参数问题** —— 重试一万次也没用。

⚠ 这个 `deviceId` **就是 `smid` 那条线换回来的 `"B"+detail.deviceId`** ——
**本工具自己就能算（`mcloud smid refresh`），不需要浏览器、也不需要装官方 App**。

曾经记成"它是另一个东西、构造不出来（密钥未知）"，那是把两件事看混了。真相：

- 签到页的 `fp.min.js` 是**数美设备指纹**，`window.SMSdk` 是**它自己定义**的
  （所以 App 二进制里搜不到这个名字）；
- `getDeviceId()` 返回 `'B' + localStorage['SMID']`，而 `SMID` 就是它往
  `slw.h5cmpassport.com:9090/deviceprofile/v4` 上报特征后拿回的 `detail.deviceId`；
- 那套配置 —— organization / appId / RSA 公钥 / host —— 写在签到页 HTML 的
  `window._smConf` 里，与 `internal/upstream/smid.go` 的常量**逐字相同**。

守卫：`smid::tests::the_signin_page_fingerprint_sdk_is_this_same_client` —— 那是 **Rust 基准**里的测试名，
基准现已移到同级仓库 [`../mcloud-rust`](../../mcloud-rust)。
⚠ **Go 侧目前没有把这条守卫钉过来**：`test/golden/app-signin-vectors.json` 与 `crypto-vectors.json`
里存着从签到页抄下来的 `SM_ORGANIZATION` / `SM_API_HOST`，Go 侧也有同值的
`upstream.SmOrganization` / `upstream.SmAPIHost`（`internal/upstream/smid.go`），
但**没有任何一条断言把两边对上**（全仓搜不到读这两个黄金键的代码 —— `smid_test.go` 是拿常量拼期望值，
不是拿黄金值校验常量）。补一条"常量 ≡ 黄金向量"的用例即可等价还原。

⇒ 所以 `--receive` **一个额外参数都不用给**：不传 `--device-id` 时自动从库里
这个账号的 `smid_device_id` 取，库里没有就**现上报一次换一个**：

```bash
mcloud signin --account 2 --receive --cloud-type 2
```

`--device-id` 只在要做对照实验时才显式给（旧名 `--h5-device-id` 仍可用）。

⚠ 那个值末尾有**一个多余的 `=`**（89 字符 = `B` + 86 数据字符 + 2 个 padding，
规范只要 1 个）—— 那是它本来的样子，**原样透传，别"顺手规范化"**。

⚠ 顺带更正一条早先的误判：把整串 89 字符当 base64 解码会得到 65 字节、
"首字节"是 `0x05`/`0x06` —— 那个字节其实是 **`'B'` 前缀与 SMID 首字符共用了一个字节**，
既不是魔数、也没有"65 字节"这回事，真实结构是 **`'B'` + 64 字节**。

当前 `--cloud-type` 也从 `infoV3` 的 `receiveList` 里读（跑一次 `--status-only` 就会打出来）。

### 6. 复活卡 / 回馈卡是第三个接口（`receiveRevivalReward`），而且它不可逆

`--revive` 打开。H5 原话：「关闭弹窗视为放弃机会，**无法补发**」——
所以它单独一个开关、默认不开，绝不跟着签到顺手跑。

请求形状极简：`POST` + **空对象体 `{}`**，且是整页**唯一**一个
`showLoading: false` 的接口（H5 要自己画卡片弹窗）。

⚠ 它**不需要** `deviceId`（H5 定义里没有 `isDeviceId`），
但老抓包里带了个 `deviceid` 头 —— **两个证据不一致，未验证**，
所以参数留成可选：不给就照 H5 定义，给了就补那个头。

⚠ 响应里的 `totalClouds` **没有**接进余额采集：H5 文案说复活卡产出的是**AI豆**，
与 `infoV3.beforeTotal`（云朵余额）可能不是一个池子，现有实拍分不出来。

### 自检参数

- `--status-only`：只查不签；
- `--force`：已签也发一次 `startSignIn`，用来确认请求被上游接受；
- `--json`：给脚本 / 定时任务消费。

⚠ `--receive` / `--revive` 都是**写请求**，它们的结果也会进 `--json`
（`receive` / `revive` 两个键）—— 别把它们写成"只看不写"的开关。

### 7. `Authorization` 是整串存、整串用（schema v7）—— 不要再拆开再拼

它的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`，登录时**一次就拿到整串**，
所以 `account_state.authorization` 存的就是它**原文**，发请求时原样用出去。

旧项目 `mcloud-sign` 一直这么做（它的 `asign.json` 里 `auth` 存的就是整串），
照搬即可，没有重新发明的必要。

早先的写法是"只留第三段 token，用的时候拿手机号现拼" —— 那是**绕远路且会错**：

1. 必须凑齐手机号，缺了就只能让用户每次在命令行上补（`--phone`）；
2. 前缀被硬编码成 `mobile`，猜错**静默**发出错误的头，表现为 `90001` 未登录。

⇒ 读侧**整串优先**，`phone` 列降级成"标识"（打印、排障），**不参与拼头**；
`phone` + `jwt_token` 现拼只作为 v7 之前老行的退路。

⇒ 老行由 `migrate` 回填：`asign.json` 的 `auth` → `account_state.authorization`。

这条回填**按状态判**（"这一列空着就填"）而不是按源文件哈希判 ——
后者对已经导入过的库永远不会再跑，会让同一个版本的程序产出两种库形态。
只填空缺的语义让它幂等，于是"每次都跑"是安全的：
**重登过的新凭据不会被旧配置覆盖回去**（有测试钉住）。

> ⚠ **这一列只装「拿到的」，不装「拼出来的」。**
>
> 合法来源只有两个：登录响应里的 `auth`、旧配置里的 `auth` —— 都是原样给的。
> **本地合成的串一律不许写进去**：读侧会**优先采信**整串，
> 而整串路径**没有前缀可校验**（前缀就在串里），
> 于是一串拼错的凭据落库后会被永久当成原文使用，现象只是"忽然回 `90001` 未登录"。
>
> 典型反例正是 `--phone` 那条退路：它拼出来的前缀固定是 `mobile`，
> 拿它去补一个 `pc:` 端账号，存下去就是**永久错的**。
>
> ⇒ 所以 `--phone` **只记手机号**（那是用户给的事实），不记那次现拼的串；
> 下次照旧现拼、前缀闸照旧生效，而用户同样不必输第二遍。

---

## 退出码约定

沿用原项目：只有**运行错误**影响退出码；业务层面的 fail 属 info 级，不影响退出码。
无 error/fatal 时退出码 0 是正确设计。

---

## CLI 不需要"登录本系统"

数据库就在本机：能跑 CLI 的人本来就能直接 `sqlite3` 读库，
给他加一道"你是谁"**不增加任何安全性**，却让本地运维变别扭。
所以 CLI 一律用 `Scope::Local`（不过滤，能操作所有人的账号）。

隔离是给**网络上的用户**准备的（WebUI：A 看不到 B 的账号）—— 那里用户拿不到库文件，
边界才有意义。过滤逻辑只有一份：查询写 `(?n IS NULL OR owner = ?n)`，
两种范围共用同一句，所以"某条查询漏了归属条件"在结构上不可能发生。

**WebUI 那侧确实要**：`POST /api/session` 登录、每个请求带 cookie 解析出 `UserId`，
再据此取 `Scope::User(谁)`。两条硬规则（见 [`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md) §3.7）：

- **没初始化就只暴露初始化**：库里没有用户时，除 `/api/setup` 与健康检查外一律 `503`；
- **其余接口一律要会话**，且**白名单**判定豁免（新加的路由默认是"需要登录"）。

CLI 新建的云盘账号会归**库里首个用户**（没有用户则保持无主，等 `user init` 时认领）——
这不是权限检查，而是"账号必须有主人"：否则网页端按隔离规则看不到它，
会出现"扫码成功了、网页里却没有这个账号"。

---

## 旧数据迁移

```bash
go run ./cmd/mcloud -- migrate --repo-root <旧项目路径> --dry-run
```

`--dry-run` 是"在内存库上跑**完全相同**的路径再丢掉库"，不是另写一套只检查不写入的代码
（两者会各自演化，预演就会失去意义）。

硬底线：

- **解析不到的账号引用绝不回落到"全部账号"** —— 那会把"过去没跑"变成"对所有人跑"。
  定时任务导入为**停用**，组合（无 `enabled` 字段）**整条跳过**；
- 掩码 `AccountId` 撞车必须在**任何写入之前**中止（连台账都不落）；
- 源目录**只读**；"归档"是为了让新项目自带可离线重放的原始数据，**不是回滚用的 `.bak`**。

迁移还会**回填凭据**（`asign.json` 的 `auth` → `account_state.authorization`，见上面第 7 条）。
报告里的 `凭据回填 N` 是**本次实际补上**的个数 —— 没有缺口可补的不计入，
所以第二次运行会看到 `0`，那个数字才有意义。

---

## WebUI

```bash
cd web && npm install && npm run build      # 前端产物到 web/dist
go run ./cmd/mcloud-server                   # API + 前端，默认 http://127.0.0.1:8787
```

首次打开是**初始化页**（建首个管理员）；之后就一直是登录页。
前端开发用 `cd web && npm run dev`（vite 5173 → 后端 8787 已配好代理）。

**改前端样式前先读 [`webui-design-guide.md`](./webui-design-guide.md)** ——
它是视觉与交互的唯一依据（页面骨架、组件类名、移动端硬要求、验收清单都在那里）。

会话走 cookie：`HttpOnly` + `SameSite=Lax`，库里只存 `sha256(token)`，
登出 = 删掉那一行，**立刻生效**。口令用于登录**本系统**，与移动云盘账号无关。
