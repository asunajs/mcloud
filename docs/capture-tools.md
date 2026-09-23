# 抓包工具与通路（**动抓包之前照着这一页敲命令**）

> 状态：**规范**（2026-09-23 首次成文。内容 = 各脚本抬头注释里的实测结论 + 当天一次只读核验）
>
> 本文管**用什么工具、敲什么命令、前置条件是什么、每条通路能拿到什么**。
>
> 它**不管**（各有归属，别往这里抄）：
>
> | 不管什么 | 去看 |
> | --- | --- |
> | **纪律**：何时抓、抓多少、停止条件、合规红线、抓包面选谁 | [`capture-guidelines.md`](./capture-guidelines.md) |
> | 脱敏具体怎么做（换值不换形） | [`sanitize-guide.md`](./sanitize-guide.md) |
> | 哪些值能进版本库 | [`credential-boundary.md`](./credential-boundary.md) |
> | `fixture extract` 的参数面 | [`cli-usage.md`](./cli-usage.md) |
> | 协议字段 / 错误码 | [`upstream/`](./upstream/) 那一族 |
>
> 坑只**按编号引用** [`migration-pitfalls.md`](./migration-pitfalls.md)，不复述正文。

---

## 0. 先选通路（一张表）

| 代号 | 通路 | 工具 | 取证面 | 能拿到 | 代价 / 拿不到 |
| --- | --- | --- | --- | --- | --- |
| **A** | Reqable 系统代理 | `scripts/reqable-capture.py` | 走代理的流量（App 原生 CFNetwork、App 内 WebView） | URL / 头 / 体 / 时序 / 连接 / WS 帧 | 自己构造 `Transport` 的客户端**不进代理**（#120）；请求体是密文原样 |
| **B** | App 自写网络日志 | `scripts/applog-capture.py` | App 内部**所有**网络调用 | 头整块、`encodeData` 之外的**明文信封**、`Raw Response String` | **按体积滚动**，不当场 `snapshot` 就没了；信噪比极低（轮询占九成） |
| **C** | 客户端产物静态取证 | `scripts/asar-extract.cjs` | PC Electron 的 `app.asar` | 官方文案、码名、调用点 | **不是服务端契约**（#136） |
| **D** | 活体探针 | `probe-hcy-cool-flag.py`、`crack-cool-flag-response.py` | 我们自己发出的请求 | "算法在真服务器上成立" | 打上游 ⇒ 按准则 §2 交计划；只用合成号码 |
| **E** | 浏览器侧 CDP | **本仓没有脚本**（见 §4） | 官方 PC Web 页面 | 全量（URL/头/体/响应） | 只适用 Web 端；工具尚未落地 |
| **F** | WebView localStorage | `scripts/h5-device-id.py` | 官方 App 实发的 `deviceId` | 与本项目自算值**对拍** | 取证用，**不是**日常路径 |

> A 与 B **互补**，而且各管一半：**App 原生层走日志（B），App 内 WebView / H5 走代理（A）** ——
> 直播间是 H5 ⇒ 只能走代理，而代理里它挂在 `com.apple.WebKit.Networking` 名下，
> 默认 `--app mCloud` **一条都抓不到**（坑表 #133 那一轮）。
> 一条链路两样都要 ⇒ 两边各留一份素材（`app-rights-token-samples.txt` 抬头就是这么写的）。

**三条落地的硬要求**（工具层面，不是纪律层面）：

1. **默认不碰网络、不碰系统代理。** `probe-live-room.js --wire` 只**检查**前置并打印步骤，
   开关交回人 —— 改系统代理是机器级副作用（[`scripts/probe-live-room.js`](../scripts/probe-live-room.js) 抬头第 3 条纪律）。
2. **打印前先脱敏。** `reqable-capture.py detail` 是**原值**输出 ⇒ 看详情一律走 `reqable-detail-masked.py`（§1）。
3. **HAR 只写 `har/`**（`.gitignore` 里），脱敏由 `mcloud fixture extract` 在**写盘之前**做完并 fail closed。

---

## 1. 通路 A：Reqable 系统代理

### 1.1 前置（缺一不可）

| 前置 | 怎么确认 |
| --- | --- |
| Reqable 的 **GUI** 在跑 | `pgrep -fl Reqable.app/Contents/MacOS/Reqable` |
| 它**已接管系统代理** | `networksetup -getsecurewebproxy Wi-Fi` 应显示 `127.0.0.1:9000`；Reqable 里的开关叫"系统代理" |
| 目标 App 已启动过 | 记录里的 `application.name` 才对得上 |
| HTTPS 的 CA 已被信任 | macOS 上 Reqable 自己装 —— 这正是选它而不是自建代理的原因 |

⚠ **判据必须落在 GUI 那个二进制上**：`pgrep -f Reqable` 会连带命中
`Contents/Helpers/mcp-server`，而那个 helper 是抓包脚本**自己拉起来的** ⇒ 判据**恒真**（坑表 #93 那一族）。

一条命令把上面这些跑完：

```bash
node scripts/probe-live-room.js --wire     # 只检查、只打印，不动网络也不动代理
```

### 1.2 命令

```bash
python3 scripts/reqable-capture.py on                     # 开始抓包
python3 scripts/reqable-capture.py off                    # 停止
python3 scripts/reqable-capture.py clear                  # 清空当前记录（开抓前先清）
python3 scripts/reqable-capture.py summary [--host <子串>] [--all]
python3 scripts/reqable-capture.py watch [--interval 2]   # 边操作边看新记录
python3 scripts/reqable-capture.py har har/<日期>-<链路>.har [--host <子串>] [--all]

python3 scripts/reqable-detail-masked.py <id> [<id> ...]  # ⭐ 看单条详情：打印前就换形
python3 scripts/reqable-detail-masked.py <id> --show-body 3000   # 放宽响应体截断
```

- **进程名过滤**：默认 `mCloud`。抓 **App 内 WebView / H5** 要换 —— 那一侧的进程名是
  `com.apple.WebKit.Networking`（准则 §1.1）。开关有两种：`--app <子串>`，或环境变量
  `MCLOUD_CAPTURE_APP`（一整轮都抓 WebView 时用后者，省得每条忘）。
- `--all` = 不限进程。**别拿"这一族的标志"当全局过滤器**（#106）。
- ⚠ `detail` 子命令**不脱敏**：那一族的请求头里有 900 多字符的 `jwttoken`、整串 cookie、
  `x-tingyun`，直接 `detail` 就把原值打进终端和对话里了。`reqable-detail-masked.py` 是它的替代品，
  换形名单（哪些头、哪些查询参数）写在该脚本头部，脱敏口径见 `sanitize-guide.md`。

### 1.3 已知现状与缺口（2026-09-23 一次只读核验）

- ✅ 本机系统代理 `Enabled: Yes` / `127.0.0.1:9000`，Reqable GUI 在跑。
- ✅ `summary --all` 正常返回（当次"总记录 0 条"= 桥是通的、只是当前没有留存记录）。
- ✅ App 日志通路（B）当次可用，见 §2.2。
- ❓ `status` 子命令这次超时（`TimeoutException: /capture/live/status`）而 `summary` 正常。
  **单次观察，未核实是否稳定复现，也没查原因** ⇒ 待确认；判"能不能抓"暂时先看 `summary`。
- ❓ **本会话那个 `reqable` MCP 连接器（46 个工具）当次两次只读调用都超时**
  （`/capture/live/filter`、`/collection/list`）⇒ **本仓已验证可用的 Reqable 入口只有脚本这一条**。
  它的过滤器 schema 里成文的事实可以先记下：多条件之间是 **AND**（正是 #106 那条），
  可用的过滤器类型有 `keyword` / `url` / `host` / `ip` / `method` / `code` / `application`（后者支持
  `name` / `id` / `pid`，抓 WebView 时按 `pid` 收窄最干净）。
  ⚠ **要不要把 MCP 连接器列为日常通路，取决于 Reqable 端那个本地服务怎么开 —— 这一步我们没实测**，
  开对了再补进 §1.2，别先写"可以直接用"。

### 1.4 这条路上踩过的坑（都在脚本注释里，这里只指路）

MCP 握手时序（`initialize` 必须先回）、`readline()` 配 deadline 会假死、
记录里的 `body` 是**对象**不是字符串（按字符串量长度恒为 3）、
访问本机端口要 `--noproxy '*'`（本机有 Clash）—— 逐条见
[`scripts/reqable-capture.py`](../scripts/reqable-capture.py) 抬头「本机踩过的坑」。

---

## 2. 通路 B：App 自写网络日志（`FXTrace*.log`）

### 2.1 命令

```bash
python3 scripts/applog-capture.py list                    # 有哪些文件、体积、时间跨度
python3 scripts/applog-capture.py summary [--host --path --service] [--all]
python3 scripts/applog-capture.py snapshot /tmp/fxlog-<日期>   # ⭐ 抢出来，防滚动覆盖
python3 scripts/applog-capture.py dump <URL 或 API Name 子串> [--limit 1]
python3 scripts/applog-capture.py har har/<日期>-<域>.har --path /user/

go run ./cmd/mcloud fixture extract --har har/xxx.har --out test/fixtures/http
```

- `dump` **默认把凭据头打码**，要看原值显式加 `--show-secrets`；`--all` 才包含空转轮询。
- 日志目录默认是本机那台 iOS App 的容器路径，换设备/换安装用 `--dir <路径>` 或 `MCLOUD_APPLOG_DIR`。

### 2.2 保留窗口：按**体积**算，不要按小时估

5 个文件 × 10 MB 滚动封顶。跨多少时间**完全取决于流量**，所以两个现场差出一个量级：

- 2026-09-19 那次 App 在前台空转轮询，5 个文件 43 MB 只覆盖 **1 小时 48 分**
  —— 数字与轮询占比的出处是 `client-profiles-fingerprint-and-landing.md` §7，那边只引不抄。
- 2026-09-23 当晚几乎没动 App，`list` 显示最末一个文件 4.5 MB 跨 15:18 → 20:42（**5 小时 24 分**）。

⇒ 结论只有一条，而且它是纪律不是估算：**看到有用的当场 `snapshot`**，回头再读只会读到轮询。
（脚本 `list` 那句"保留窗口不到 2 小时"是按前一种现场写的**保守提示** —— 别反过来当成"还有 2 小时"。）

⚠ 另一个读数坑：日志里大量 `Status: 0` + 耗时恰好 `30000 ms`，那是**超时没拿到响应**，
不是 HTTP 0（`summary` 的状态列会显示成 `0/200` 的混合形态）。同上指回 §7 那篇。

---

## 3. 通路 C：客户端产物（`app.asar`）静态取证

```bash
node scripts/asar-extract.cjs list  <app.asar> [子串过滤]
node scripts/asar-extract.cjs cat   <app.asar> <内部路径>
node scripts/asar-extract.cjs grep  <app.asar> <正则> [只搜 .js]
node scripts/asar-extract.cjs save  <app.asar> <内部路径> <输出文件>
```

- 自己解析 asar 头部，**不联网**（不用 `npx @electron/asar`，门禁要求离线可跑）。
  头部布局与自检式写在脚本注释里（实测于 `/Applications/中国移动云盘.app`）。
- ⭐ **先搜仓库里已经抽出来的产物，再考虑打上游**（#141）：
  `docs/capture/apiServer.js`、`pcclient-api.js` 就是 `app.asar` 里的 renderer 源码。
- ⚠ 两条边界：产物**不是服务端契约**（问"它叫什么"可以，问"服务端会不会照做"不行 —— #136）；
  这类素材按**来源**在 `check-identifiers.cjs` 的 `VENDOR_SOURCE_PATHS` 里豁免，
  **新增一份就得逐个判断并登记**（`sanitize-guide.md` §二）。

---

## 4. 通路 D / E / F：探针、浏览器侧、对拍

**D 活体探针**（两份一次性脚本，抬头各自写明背景与结论）：

```bash
python3 scripts/probe-hcy-cool-flag.py --baseline --save har/hcy-cool-flag-samples.json
python3 scripts/probe-hcy-cool-flag.py --phase2
python3 scripts/crack-cool-flag-response.py har/hcy-cool-flag-samples.json [--sm4]
```

全部用**合成号码**（`199` 号段），不给任何真实号码发短信。它们打的是真上游 ⇒
计划、上游请求数、幂等性按准则 §2 / §5.2 走，不因"是探针"而豁免。

**E 浏览器侧 CDP**：⚠ **本仓没有对应脚本**。2026-09-18 那次 PC Web 扫码取证是在
Windows 11 + Edge 上直连 CDP（`Network` 域实时落盘 JSONL），原始文件在本机临时目录、**未入库**
（清单见 `upstream/web-qr-login.md` 文末）。⇒ 再要一次 Web 端取证得**先重建工具**，
且新工具按 [`AGENTS.md`](../AGENTS.md) §3.5 用 zx 写。

**F 官方实发 `deviceId` 对拍**：`scripts/h5-device-id.py`（只读，带 `--self-test`）。
日常**不需要** —— 那个值本项目自己就能算（`mcloud smid refresh`，见 `cli-usage.md`）；
留着只为了"拿官方实发值跟我们的产出对拍"。

---

## 5. 一次取证的固定动作（从"能抓"到"落档"）

| 步 | 做什么 | 用什么 |
| --- | --- | --- |
| 0 | 交计划（假设 / 过滤 / 最大请求数 / 停止条件 / 落档位置） | 准则 §2，**先于一切** |
| 1 | 前置自检，**不代开代理** | `node scripts/probe-live-room.js --wire` |
| 2 | 清空 + 开抓，选好进程名（App 还是 WebView） | `reqable-capture.py clear` / `on`，`--app` 或 `MCLOUD_CAPTURE_APP` |
| 3 | 在**厂商客户端**里把那一部操作走一遍（能只读就只读） | 人工 |
| 4 | **当场**抢出素材：导出 HAR；日志那条路先 `snapshot` 再 `har` | `… har har/<日期>-<链路>.har` / `applog-capture.py snapshot` |
| 5 | 看单条详情**只用脱敏那份** | `reqable-detail-masked.py <id>` |
| 6 | 转夹具（写盘前脱敏 + fail closed） | `go run ./cmd/mcloud fixture extract --har … --out test/fixtures/http` |
| 7 | 落档：追加进 `docs/capture/<链路>-samples.txt`，抬头写**通路代号 + 采集时刻 + 原始素材路径** | 见 §6 |
| 8 | 停抓把系统代理交回人（脚本不代关） | `reqable-capture.py off` |

第 3 步与第 4 步之间不要夹别的动作：日志那边窗口是按体积滚的（§2.2），晚一步就没了。

---

## 6. 工具 → 落档抬头（"采集方式"那一栏怎么写）

抬头字段本身按 [`README.md`](./README.md)「抬头模板」，素材**追加式**（准则 §6）。
"采集方式"要写到**能复跑**的程度 —— 通路代号 + 命令 + 原始素材在哪：

| 通路 | 抬头写法（现有样本里的实例） |
| --- | --- |
| A | `通路：Reqable 系统代理（端口 9000）→ iOS App 的 CFNetwork 请求`（`app-signin-samples.txt`） |
| A + B | `采集方式：App 自身 FXTrace*.log + Reqable 系统代理`（`app-rights-token-samples.txt`，两条各留一份原始素材） |
| A（WebView 面） | 点名取证面是 **WebView** ⇒ 进程名 `com.apple.WebKit.Networking`（`app-live-ws-samples.txt`） |
| D | 写"活体探针"并附探针脚本名，不用"抓包"混称 |

另外两栏别漏：**采集时刻**（口令级、窗口级的证据没有时刻就归不了因 —— 准则 §5.3、#139）与
**原始素材路径**（`har/…` 不入库，但要写出名字，好知道当时有多少、还剩什么）。

---

## 7. 待拍板 / 待确认（都不阻塞上面任何一步）

1. 三份抓包脚本里 `applog-capture.py` 有可执行位，`reqable-capture.py` 与
   `reqable-detail-masked.py` **没有**（`./scripts/reqable-capture.py` 会 `permission denied`，实测）。
   ⇒ 补齐 x 位，还是全部统一按 `python3 scripts/…` 写？**本文先统一写 `python3 …`。**
2. 这几份是 python，而 [`AGENTS.md`](../AGENTS.md) §3.5 定的是"新脚本一律 zx"。
   ⇒ 存量要不要收口成 `.js`？**不收口的话，本文 §4-E 那条"重建工具"就会再多一个例外。**
3. `status` 超时与 MCP 连接器超时是否稳定复现、开关在 Reqable 哪里 —— **都还没核**，
   复现方法就是照 §1.1 开好后各跑一次。

---

## 8. 与其它权威的分工

| 争什么 | 以谁为准 | 别处怎么处理 |
| --- | --- | --- |
| **用什么工具、命令、前置条件** | **本文件** | 准则只写纪律并指过来 |
| 何时抓 / 抓多少 / 停止条件 / 合规红线 | [`capture-guidelines.md`](./capture-guidelines.md) | 本文不复述纪律 |
| 脱敏怎么做、哪些值能进库 | [`sanitize-guide.md`](./sanitize-guide.md) + [`credential-boundary.md`](./credential-boundary.md) | 本文只写"哪个子命令不打码" |
| `fixture extract` 的参数与语义 | [`cli-usage.md`](./cli-usage.md) | 本文只写它在流程第 6 步 |
| 轮询接口占了九成、设备指纹的证据 | `client-profiles-fingerprint-and-landing.md` §7 | 本文只给窗口结论并指过去 |
| 开播窗口那份核对清单 | **`scripts/probe-live-room.js` 里的 `ITEMS`** | `status.md` 只放指针（#116） |
