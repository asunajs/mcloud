# mcloud

移动云盘自动化工具。**Rust 重写版** —— 由原 Node/TypeScript 项目
[`mcloud-sign`](https://github.com/catlair/mcloud-sign) 重写而来。

## 为什么重写

原实现是 TypeScript，长期受三件事困扰：任务逻辑与 IO 纠缠导致难以单测、依赖体积大、
发布要靠 esbuild 打包。Rust 版的目标是把**纯业务逻辑**与**IO 实现**彻底分开，
让"任务行为是否正确"这件事可以在没有网络的情况下穷尽测试。

## 分层铁律

依赖方向（箭头左侧依赖于右侧，与各 crate 的 `Cargo.toml` 一致）：

```
mcloud-core          （无内部依赖）
mcloud-upstream      → core
mcloud-capabilities  → core, upstream
mcloud-exec          → core, capabilities
mcloud-io            → core        （+ reqwest / rusqlite / rand：全项目唯一实现 IO 的地方）
mcloud-app           → core, upstream, capabilities, io          组装层（两个产物共用）
mcloud-server        → core, capabilities, exec, io, app         产物之一（HTTP/WebUI）
mcloud-cli           → core, upstream, capabilities, io, app     产物之一（命令行）
mcloud-cli           → core, capabilities, server         可执行入口
```

`mcloud-server` **必须**依赖 `mcloud-io` —— 反过来（没人接上 IO 实现）是一个静默的死配置，
编译能过、跑起来什么都没发生。`mcloud-cli` 则**不**直依赖 `mcloud-io`，它只做参数解析。

| crate | 角色 | 约束 |
| --- | --- | --- |
| `mcloud-core` | 领域类型、纯函数、注入 trait 定义 | **禁止任何 IO**：不得依赖 reqwest、rusqlite、mcloud-io |
| `mcloud-upstream` | 上游接口（请求构造、响应解析、签名） | 同上。因此密码学放在 core 而非 io |
| `mcloud-capabilities` | 能力注册表（CLI 与 WebUI 的唯一契约来源） | 同上 |
| `mcloud-exec` | 任务编排 | 同上 |
| `mcloud-io` | reqwest / rusqlite / 系统随机 | 唯一实现 IO 的地方 |
| `mcloud-app` | 组装层 | **必须**依赖 mcloud-io；迁移策略、能力注册表 JSON、登录驱动都在这里 |
| `mcloud-server` | 产物：HTTP/WebUI | **必须**依赖 mcloud-io，否则是"没人接上 IO 实现"的静默死配置 |
| `mcloud-cli` | 产物：命令行 | **不得**依赖 `mcloud-server`（否则会把 axum 拖进命令行产物） |

外部世界一律走注入 trait，共 4 个：`HttpClient`、`Clock`、`StateStore`、`Random`。
`Random` 是必需的 —— smid 构造混用了 `randomUUID` 与 `Math.random`，
不注入则 fixture 回放与黄金向量对拍无法复现。

上表不是文档约定，是**可执行断言**：`scripts/check-deps.sh` 会真的读每个 `Cargo.toml`
并断言依赖方向，违反即非零退出 —— 而不是只在文档里写着"请勿依赖"。

> 现状：仓库**还没有 CI 配置**，也没有 git hook。所以这五条门禁目前靠改完手动跑一遍，
> 不是自动拦截。加上 CI 是待办。

## 门禁

改完代码跑这六条，全绿才算完成：

```bash
cargo test                                          # 单元测试
cargo clippy --all-targets -- -D warnings           # 静态检查（warnings 视为错误）
cargo fmt --check                                   # 格式
bash scripts/check-deps.sh                          # 分层铁律（见上）
node scripts/check-identifiers.cjs                  # 仓库里不得有真实标识（见下）
node scripts/check-golden-vectors.cjs               # 黄金向量能算出自己（见下）
```

第 6 条是**抓包侧**的自检：`test/golden/*.json` 是要进版本库的产物，
但"有人手工改坏一个字符"只有真算一遍才发现。它用 Node `crypto` 独立实现算
**106 条**向量，与 Rust 侧互为交叉验证 —— 一边过一边不过，说明其中一边错了。
它同时做两件 Rust 侧做不到的事：对**每个** golden 文件跑脱敏扫描（Rust 只扫
`include_str!` 进来的那些），以及列出**没有任何 Rust 测试读取**的向量文件
（`app-business-crypto-vectors.json` 与 `login-crypto-vectors.json` 目前是这种状态）。

覆盖范围是显式的：没有校验器的段会被**打印出来**，不假装通过。

> 上面那句"106 条"是当时的实况。**2026-09-19 追加**：此刻是 **122 条断言全绿**，
> 新增的两段是 smid 那条线的 `§smidString` 与 `§smidPriId`（各 3 条，对拍旧 TS）。

当前状态（2026-09-19 收工）：**554 个测试通过**，六项门禁全绿。
同日接完 smid（设备指纹绑账号）那条线后是 **493 个**（smid 那条线自己贡献 46 个：
上游算法 25 + 生命周期 13 + 存储 8）；随后 App 签到线又加了上游接口层、编排层
与存储层（`account_state.phone`，schema v6）的用例，到 **516**；
再接上云朵余额采集（`mcloud-app::cloud` 5 条 + `set_account_cloud` 窄写 2 条），到 **523**；
接着 `receiveV3` 领取（上游层请求构造 / 解析 / 错误码 10 条），到 **533**；
然后 `revival` 复活卡（上游层请求构造 / 解析 / 错误码 8 条，含一条读黄金向量的），到 **541**；
接着查明 H5 `deviceId` 的根因并接线（新增一条把签到页 `_smConf` 与 Rust 常量钉在一起的守卫），到 **542**；
然后把 `Authorization` 从"拆开存、拼回去"改成**整串存、整串用**（schema v7，见 §现状须知 7），
新增 4 条：窄写"只填空缺"2 条 + 整串优先 1 条 + 迁移回填的幂等与不降级 1 条，到 **546**；
最后给 CLI 的账号选择（下标语义）补上 9 条用例 —— 零参数只跑启用账号且下标用全列表位置、
`--account` 数停用账号、下标从 1 开始、越界报总数、`--phone` 空库也能选中、`--list` 互斥、
顶层与 `mcloud signin` 两种写法等价、不带子命令补出来的确实是签到 —— 到 **554**。
上一条实况是 2026-09-17 的 **241 个测试**、其余四项全绿。

> ⚠ **更正（2026-09-19）**：本节此前写着"`mcloud-upstream` 的 L2 夹具回放已跑通
> （`crates/mcloud-upstream/tests/l2_replay.rs`）"—— **那个文件不存在**，
> `crates/mcloud-upstream/tests/` 是个**空目录**。
> 现状是：`mcloud fixture extract` 往 `test/fixtures/http/` 写，但**没有任何消费者**，
> 所以那 31 个夹具目前一个断言都没跑。这是规划 §6.3 里要补的一环，
> **别按"已经有了"来理解**。
> 已确认可用的样本（写回放时直接照着接）：
> `startSignIn` / `infoV3` / `doTaskPost` 在 `3d.json`，
> `taskListV3` 在 `每月阅读任务.json`，`receiveRevivalReward` 在 `复活.json`；
> **`receiveV3` 没有任何夹具**（那一条只能靠单测 + 真机）。

## 凭据边界 —— 界线是"能不能进 GitHub"，不是"能不能进日志"

这条界线决定了哪些值可以出现在哪里，**两侧规则刻意不同**，不要互相套用：

| 场景 | `userDomainId` / 手机号 | 理由 |
| --- | --- | --- |
| 日志、运行期输出（库 / `Debug` / CLI） | **保留** | 本地排障资产，且不可直接复用 |
| 进版本库的一切（夹具、黄金向量、源码字面量、注释示例、base64） | **必须脱敏** | 仓库全量留痕，推上去就收不回来 |

因此：

- **合成值统一是 `199` 号段**：手机号 → `19900000001` 起，`userDomainId` → `1990000000000000001` 起。
  "这个形状的值必须以 199 开头"是一条**可机检的不变量**，两道守卫都据此断言。
- **`fixture extract` 会把标识形状的数字重映射**（不是打码）：判据与守卫的 `SHAPES` 逐条对齐
  （11 位或 19 位、以 `1` 开头、两侧不是字母数字）。用重映射而不是掩码，
  是因为撞上这个形状的不只是号码 —— 实测 `taskId` 就是 19 位 `1…`，
  而它要参与响应解析，掩码会让它失去形状。映射由原值派生（sha256），
  **确定性的**：同输入同输出，否则夹具每次重新生成的 diff 全是噪声。
- **掩码同理**。掩码是从号码派生的，由真实号生成的掩码会泄漏前 2 后 2 位。
  示例一律用合成号派生（`19900000001` → `19****01`）。
  掩码含 `*`，不是连续数字串，形状类守卫抓不到 —— 这一类只能靠人工审。
- **header 走白名单，不走黑名单**（`sanitize::is_allowed_header`）。黑名单在真实流量上被证伪：
  `cookieToken` / `cookieTokenKey` / `x-yun-tid` / `hecaiyundata2021jssdkcross`
  这些**名字不在名单里**的 cookie / 追踪头带着可直接复用的凭据原样通过，
  自检报出三万余处残留 —— 一份夹具都写不出来。往黑名单里逐个加名字是打地鼠，
  而"夹具需要哪些 header"是有限的，所以判据只能是**登记过的才放行**。
- 同理，`sanitize_json` 在键名黑名单之外**还按形状兜住值**：键名由上游决定，
  无法预先知道哪个字段会装凭据（实测 `{"xxx":"<base64 的 mobile:手机号:token>"}` 很常见）。
  **脱敏与自检必须共用同一套"长串"字符集，只差阈值**（脱敏 32 / 自检 40）：
  两侧定义不一致时，门禁会恒红且无从修复（实测 `-` 差一个字符就让带连字符的 CDN URL 被切成碎片逐个报警）。
- **`auth` 是 base64**，里面的号码字面扫描看不见。两道守卫都会**解码后复查**。
  历史教训：正因为少了这一步，迁移相关的测试字面量里藏过真实手机号，而当时两道只扫字面的守卫一个都没报。
- `scripts/check-identifiers.cjs` 的 `ALLOWED` 表登记刻意非 `199` 的测试构造值 ——
  **每条都要写明理由，真实值一律不准登记**。
- **写抓包 / 协议文档前先读 [`docs/sanitize-guide.md`](./docs/sanitize-guide.md)**：
  那是"**换值不换形**"的具体做法（哪些必须换、哪些恰好不能换、什么算过度脱敏），
  以及真实值已经进了提交时该怎么改写历史。

**注意方向**：`crates/mcloud-core/src/sanitize.rs`（HAR → fixture）遮蔽 `userDomainId` 是**对的**，
它是"进版本库的静态文件"；别因为"日志里可以留"就去放宽它。

## 用法

```bash
cargo run -p mcloud-cli -- help
```

### 最常用的一条：什么都不带

```bash
cargo run -p mcloud-cli            # = mcloud signin，对**全部启用**的账号各签一次
cargo run -p mcloud-cli -- --list            # 先看下标：1、2、3…
cargo run -p mcloud-cli -- --account 2       # 只跑下标 2 那个
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

### WebUI

```bash
cd web && npm install && npm run build      # 前端产物到 web/dist
cd .. && cargo run -p mcloud-server         # API + 前端，默认 http://127.0.0.1:8787
```

首次打开是**初始化页**（建首个管理员）；之后就一直是登录页。
前端开发用 `cd web && npm run dev`（vite 5173 → 后端 8787 已配好代理）。

会话走 cookie：`HttpOnly` + `SameSite=Lax`，库里只存 `sha256(token)`，
登出 = 删掉那一行，**立刻生效**。口令用于登录**本系统**，与移动云盘账号无关。

子命令一览：

| 子命令 | 作用 |
| --- | --- |
| `user init` / `user list` | 建首个管理员（网页端要用，也是历史账号的认领触发点）/ 看看有哪些人 |
| `login qr` | 扫码登录移动云盘账号：终端出码，手机扫（`--qr-out` 可另存一张图片） |
| `login sms` | 短信验证码登录：发码 → 输验证码。被风控要求滑块时**自动过**（缺口位置由服务端加密下发，见 `docs/upstream-logout-sms-login.md` §2.2.1）。⚠ `--client app` 那个"自动过"是**借 PC 端那套**取题与编码、换 App 的头发出去（围魏救赵）；App 自身的滑块**未实现自动化**，真机走的仍是开 WebView 由 H5 发码那条（见 `docs/capture/app-sms-login-samples.txt` §2.2 的澄清） |
| `migrate` | 把旧项目 mcloud-sign 的数据迁移到 SQLite（`--dry-run` 预演） |
| `signin` | 签到（云朵中心）。**不带任何参数就是对全部启用账号各签一次**；`--account <下标>` 只跑一个（`--list` 查下标），`--phone <完整手机号>` 给老账号补一次。另有 `--status-only` / `--force` / `--json`。**纯协议**，不开浏览器、不开 WebView；成败只看回查 `infoV3`（见下方"现状须知"） |
| `smid show` / `reset` / `refresh` | 看每个账号绑定的**设备特征**（cMPassport 风控那套，旧项目拿它的结果当请求头 `deviceid` 发）、`reset` 换一个（旧的 `deviceId` 当场作废，`--report` 顺手再上报）、`refresh` 不换特征只再上报一次。输出**只有摘要**：UA 与 `canvas` 识别位不落终端。⚠ 本项目目前**还没有任何请求发这个头**，取值的那一半已就位（见 `docs/client-profiles.md` §5.4 ③） |
| `fixture extract` | 从 HAR 流量包抽取脱敏 fixture（用于离线回放）。脱敏有残留则 fail closed，拒绝写出 |
| `capabilities` | 打印能力注册表（`--json` 与 `GET /api/capabilities` 同形状） |
| `version` | 打印版本与运行时信息（`--json` 与 `GET /api/version` 一致） |

**退出码约定**（沿用原项目）：只有**运行错误**影响退出码；业务层面的 fail 属 info 级，不影响退出码。
无 error/fatal 时退出码 0 是正确设计。

### CLI 不需要"登录本系统"

数据库就在本机：能跑 CLI 的人本来就能直接 `sqlite3` 读库，
给他加一道"你是谁"**不增加任何安全性**，却让本地运维变别扭。
所以 CLI 一律用 `Scope::Local`（不过滤，能操作所有人的账号）。

隔离是给**网络上的用户**准备的（WebUI：A 看不到 B 的账号）—— 那里用户拿不到库文件，
边界才有意义。过滤逻辑只有一份：查询写 `(?n IS NULL OR owner = ?n)`，
两种范围共用同一句，所以"某条查询漏了归属条件"在结构上不可能发生。

**WebUI 那侧确实要**：`POST /api/session` 登录、每个请求带 cookie 解析出 `UserId`，
再据此取 `Scope::User(谁)`。两条硬规则（见 `docs/rust-rewrite-plan.md` §3.7）：

- **没初始化就只暴露初始化**：库里没有用户时，除 `/api/setup` 与健康检查外一律 `503`；
- **其余接口一律要会话**，且**白名单**判定豁免（新加的路由默认是"需要登录"）。

CLI 新建的云盘账号会归**库里首个用户**（没有用户则保持无主，等 `user init` 时认领）——
这不是权限检查，而是"账号必须有主人"：否则网页端按隔离规则看不到它，
会出现"扫码成功了、网页里却没有这个账号"。

## 旧数据迁移

```bash
cargo run -p mcloud-cli -- migrate --repo-root <旧项目路径> --dry-run
```

`--dry-run` 是"在内存库上跑**完全相同**的路径再丢掉库"，不是另写一套只检查不写入的代码
（两者会各自演化，预演就会失去意义）。

硬底线：

- **解析不到的账号引用绝不回落到"全部账号"** —— 那会把"过去没跑"变成"对所有人跑"。
  定时任务导入为**停用**，组合（无 `enabled` 字段）**整条跳过**。
- 掩码 `AccountId` 撞车必须在**任何写入之前**中止（连台账都不落）。
- 源目录**只读**；"归档"是为了让新项目自带可离线重放的原始数据，**不是回滚用的 `.bak`**。

迁移还会**回填凭据**（`asign.json` 的 `auth` → `account_state.authorization`，见 §现状须知 7）。
报告里的 `凭据回填 N` 是**本次实际补上**的个数 —— 没有缺口可补的不计入，
所以第二次运行会看到 `0`，那个数字才有意义。

## 目录结构

```
crates/         7 个 crate（见上表）
scripts/        门禁与夹具生成器
  gen-*.cjs       —— 夹具一律从真实产物生成，不手写期望值
  check-*.cjs     —— 门禁（分层 / 标识 / 黄金向量）
  applog-capture.py / reqable-capture.py —— 抓包侧工具
  h5-device-id.py —— **取证用**：从 WebView 的 localStorage 里读官方 App 实发的
                     `deviceId`（只读）。日常**不需要它** —— 那个值 Rust 自己就能算
                     （`mcloud smid refresh`）。留着是为了"拿真值对拍"。
                     带 `--self-test`（合成库往返 + 形状正反例 + 归属排序），改动后跑一遍
test/
  fixtures/     夹具
    legacy/       旧数据夹具（脱敏产物，进版本库）
    http/         `mcloud fixture extract` 现抽的 HTTP 夹具 —— **本机测试数据，不入库**
                  （来源 HAR 含登录凭据，且每人抓到的流量不同；干净与否由 L2 用例的
                  `audit_leaks` 兜底，而不是靠提交前人眼审）
  golden/       与 TS 原实现对拍的黄金向量（进版本库）
web/            WebUI：Vue Vapor（无虚拟 DOM）+ TSX + daisyUI
proto/          （规划中）
```

## 文件命名

| 范围 | 规则 | 例 |
| --- | --- | --- |
| 非 Rust 文件（文档 / 前端 / 脚本 / 夹具 / JSON） | **kebab-case**：全小写、`-` 分隔，不用空格与下划线 | `rust-rewrite-plan.md`、`app.tsx`、`gen-auth-vectors.cjs` |
| Rust 源文件 | **snake_case** | `test_support.rs`、`reqwest_client.rs` |
| 生态固定名 | 不改 | `README.md`、`LICENSE`、`Cargo.toml`、`THIRD-PARTY-NOTICES.md` |

Rust 那条不是风格偏好而是**语言强制**：文件名就是模块名，而 `mod` 只接受标识符，`mod my-mod;` 不合法。
夹具名由 `mcloud fixture extract` 的 `slugify` 统一（HAR 名实测带中文/空格/下划线/大写；中文保留，其余折成 `-`）。

## 现状须知

- **服务入口尚未实现。**（当时的实况，2026-09-17，原文留着）`mcloud-server` 那时只有 lib，
  没有 `serve` 命令；axum 路由、SSE、`openapi.json` 导出与 `rust-embed` 内嵌前端都在 P5。
  所以那时 **CLI 能跑，WebUI 没有后端可连**，前端只能靠 `vite dev` 单独起。
- **已更新（2026-09-19）：服务能起了，但只有"会话"这一层。** `cargo run -p mcloud-server` 起 API + 前端产物，
  路由目前只有 `/api/health`、`/api/setup`、`/api/session`（白名单免登录，其余一律要会话）。
  跑任务的那一批（accounts / capabilities / runs / schedules）、SSE 与 `openapi.json`
  导出仍在 P5 —— 所以 WebUI 登录与初始化是真接口，主界面仍是 P0 spike 的占位数据。
- **CLI 已能完成"从零到登录"**：`mcloud user init` → `mcloud login qr`（终端二维码）/ `mcloud login sms`。
  短信登录有两个端：`--client pc`（默认）与 `--client app`（iOS 客户端形态）。
  两端**不是同一件事的两种参数** —— 请求体加解密方式、`Authorization` 前缀、
  设备指纹形状都不同（差异见 `docs/client-profiles.md`）。
  登录态落在本地库的 `account_state`（凭据与展示信息**分表**，见 schema 说明）；
  设备指纹写在 `data/device.txt`（**换掉它等于换一台设备**，服务端会定位不到会话）。
- **签到（云朵中心）已端到端跑通（2026-09-19）**：`mcloud signin --account <掩码id>`。
  这是第一个按新抓包写完整的**业务**接口块 —— `mcloud-upstream::app_signin`（请求构造 +
  响应解析）+ `mcloud-app::signin`（编排）。链路是"换 SSO token → 取服务端时间戳 →
  签名建会话 → 查 `infoV3` → `startSignIn` → **回查** `infoV3`"，
  全程纯协议、无浏览器、无 WebView（协议与坑见 `docs/capture/app-signin-samples.txt`）。
  六个要记住的：  1. **成败只看回查 `infoV3`。** `startSignIn` 的 `todaySignIn` 是调用**前**的快照 ——
     实测 `todaySignIn:false` 的那次恰恰把今天签上了；只看 `code == 0` 也不行，
     上游对"今天已经签过"同样回 `success`。
  2. **账号用 `--account <下标>` 给**（`--list` 查编号）。**v7 起签到只用库里存的整串
     `Authorization`**（见下面第 7 条），所以正常情况下什么参数都不用给。
     `--phone <完整手机号>` 只留给"库里既没有整串、也没有完整手机号"的老行 ——
     用它补一次就会**记进库里**（**只记号码**，见第 7 条最后一段），下次不必再输。
  3. **端不匹配那道闸只管"现拼"那条路**：签到用整串时前缀是登录时拿到的原文，
     不存在配错的可能 —— **实测（2026-09-19）`pc:` 前缀的整串在签到接口上完全有效**
     （旧项目也从不区分：它的 `asign.json` 里两个账号一个 `mobile:` 一个 `pc:`，
     走的是同一条签到路径）。只有**退路**（用 `phone` + `jwt_token` 现拼）才把前缀
     硬编码成 `mobile`，那时若账号是用 `--client pc` 登录的，会被明确挡下来并提示重登 ——
     这个组合没实测过，宁可拒绝也不要发出去试探（失败会长得像"未登录"）。
  4. **每次查询都会顺手记一次余额**（`mcloud-app::cloud::record_cloud`）：
     `infoV3.beforeTotal` → `accounts.cloud_current` + `cloud_stats` 日快照
     （日增 / 周增在 `mcloud-core::pure::next_cloud_stats` 里算，是纯函数）。
     注意那个 `before` 指的是**领取前**、不是"签到前"—— 签到只是打卡、云朵要另外领，
     所以它签到前后**不变**，不是"签到完余额就该 +3"。落库失败只打一句 `stderr` 警告，
     **不影响签到结论**（余额是顺带采集的副产品，不该把一次成功的签到变成失败）。
  5. **领云朵是另一个接口（`receiveV3`），而且必须带 H5 那个 `deviceId`**（`--receive`）。
     实测三档（2026-09-19，只换 `deviceId` 一项）：**不带 → `615`；内容错一个字符 → `614`；
     真值 → `code=0`**。⇒ **上游校验的是内容，不只是"非空"**，"随便编一个"走不通。
     ⚠ 那两个码的 msg 都是"活动太火爆啦…"，会把人的思路带到"限流 / 退避重试"上，
     而真实原因是**参数问题** —— 重试一万次也没用。
     ⚠ 这个 `deviceId` **就是 `smid` 那条线换回来的 `"B"+detail.deviceId`** ——
     **纯 Rust 就能拿到，不需要浏览器、也不需要装官方 App**。
     （曾经记成"它是另一个东西、构造不出来（密钥未知）"，那是把两件事看混了。
     真相：签到页的 `fp.min.js` 是**数美设备指纹**，`window.SMSdk` 是**它自己定义**的
     （所以 App 二进制里搜不到这个名字），`getDeviceId()` 返回
     `'B' + localStorage['SMID']`，而 `SMID` 就是它往
     `slw.h5cmpassport.com:9090/deviceprofile/v4` 上报特征后拿回的 `detail.deviceId`。
     那套配置 —— organization / appId / RSA 公钥 / host —— 写在签到页 HTML 的
     `window._smConf` 里，与 `mcloud-upstream::smid` 的常量**逐字相同**。
     守卫：`smid::tests::the_signin_page_fingerprint_sdk_is_this_same_client`。）

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
  6. **复活卡 / 回馈卡是第三个接口（`receiveRevivalReward`），而且它不可逆**（`--revive`）。
     H5 原话：「关闭弹窗视为放弃机会，**无法补发**」—— 所以它单独一个开关、默认不开，
     绝不跟着签到顺手跑。请求形状极简：`POST` + **空对象体 `{}`**，
     且是整页**唯一**一个 `showLoading: false` 的接口（H5 要自己画卡片弹窗）。
     ⚠ 它**不需要** `deviceId`（H5 定义里没有 `isDeviceId`），
     但老抓包里带了个 `deviceid` 头 —— **两个证据不一致，未验证**，
     所以参数留成可选：不给就照 H5 定义，给了就补那个头。
     ⚠ 响应里的 `totalClouds` **没有**接进余额采集：H5 文案说复活卡产出的是**AI豆**，
     与 `infoV3.beforeTotal`（云朵余额）可能不是一个池子，现有实拍分不出来。
  另有两个自检用参数：`--status-only`（只查不签）与 `--force`（已签也发一次
  `startSignIn`，用来确认请求被上游接受）；`--json` 给脚本/定时任务消费。
  ⚠ `--receive` / `--revive` 都是**写请求**，它们的结果也会进 `--json`
  （`receive` / `revive` 两个键）—— 别把它们写成"只看不写"的开关。
  7. **`Authorization` 是整串存、整串用（schema v7）—— 不要再拆开再拼。**
     它的形状是 `Basic base64("<前缀>:<手机号>:<authToken>")`，登录时**一次就拿到整串**，
     所以 `account_state.authorization` 存的就是它**原文**，发请求时原样用出去。
     旧项目 `mcloud-sign` 一直这么做（它的 `asign.json` 里 `auth` 存的就是整串），
     照搬即可，没有重新发明的必要。
     早先的写法是"只留第三段 token，用的时候拿手机号现拼" —— 那是**绕远路且会错**：
     ① 必须凑齐手机号，缺了就只能让用户每次在命令行上补（`--phone`）；
     ② 前缀被硬编码成 `mobile`，猜错**静默**发出错误的头，表现为 `90001` 未登录。
     ⇒ 读侧**整串优先**，`phone` 列降级成"标识"（打印、排障），**不参与拼头**；
     `phone` + `jwt_token` 现拼只作为 v7 之前老行的退路。
     ⇒ 老行由 `migrate` 回填：`asign.json` 的 `auth` → `account_state.authorization`。
     这条回填**按状态判**（"这一列空着就填"）而不是按源文件哈希判 —— 后者对已经导入过的库
     永远不会再跑，会让同一个版本的程序产出两种库形态。只填空缺的语义让它幂等，
     于是"每次都跑"是安全的：**重登过的新凭据不会被旧配置覆盖回去**（有测试钉住）。

     ⚠ **这一列只装「拿到的」，不装「拼出来的」。** 合法来源只有两个：登录响应里的
     `auth`、旧配置里的 `auth` —— 都是原样给的。**本地合成的串一律不许写进去**：
     读侧会**优先采信**整串，而整串路径**没有前缀可校验**（前缀就在串里），
     于是一串拼错的凭据落库后会被永久当成原文使用，现象只是"忽然回 `90001` 未登录"。
     典型反例正是 `--phone` 那条退路：它拼出来的前缀固定是 `mobile`，
     拿它去补一个 `pc:` 端账号，存下去就是**永久错的**。
     ⇒ 所以 `--phone` **只记手机号**（那是用户给的事实），不记那次现拼的串；
     下次照旧现拼、前缀闸照旧生效，而用户同样不必输第二遍。
- **上游接口层按新抓包逐块重写。** 目前 `login`（扫码/短信/账密 + 退出登录）与
  `app_signin`（签到）是完整的，
  它们是"按新流量写接口"的头两块；其余云盘业务接口仍是骨架（`Upstream` + `Envelope` + `base`）。
  旧 TS 的 12 个接口模块**一律不从旧代码搬迁** —— 旧 HAR 与旧 TS 都会作废，
  搬了只是把旧形态再抄一遍。新 HAR 抓好后：`mcloud fixture extract` → 按新流量写接口
  → 用夹具离线回放并断言请求逐字段正确。
- **旧 TS 与线上实际请求本就不一致，重写时要逐条核对**（上一轮对拍换来的经验，
  别默认"照 TS 抄"就是对的）：

  | 接口 | TS 写法 | 真实流量 |
  | --- | --- | --- |
  | `visitlog/journaling` | 参数放在 **query** | 参数放在**表单体** |
  | `signin/task/taskListV2` | `clientVersion=11.2.1`、`group=month` | `clientVersion=11.4.0`、`group=new` |
  | `auth-service/auth/tyrzLogin` | body 带 `type`/`channel` | body 无这两个字段，`marketName=newsign_139mail` |
  | `market/signin/page/infoV3` | 不带端口的 `/market/...` | `caiyun.feixin.10086.cn:7071/ycloud/...` |
  | `noteRefreshToken` | `note.mcloud.139.com/noteServer/...` | `note-njs.yun.139.com/yun-note/...`（另一个域） |

  另有：`refreshToken` 的真实流量是**加密载荷**（原实现注释却说是明文 JSON 接口）；
  `auth/refreshToken`、`querySpecTokenV2` 这类接口的 `code` 是**字符串**不是数字。
- **`docs/rust-rewrite-plan.md` 已同步进本仓库**（原 `mcloud-sign` 仓库不再需要它）。
  它是动工前的规划快照，进度以本文为准；同步时里面的真实手机号已替换为合成号段
  —— 原仓库是公开的，这类值不该留在里面。
- **接第二个端（安卓 / iOS / 邮箱端 / 小程序…）之前先读
  [`docs/client-profiles.md`](./docs/client-profiles.md)**：
  各端差异**主要在请求头**（请求体只有 `clientType`/`cpid`/`version` 随端变化）。
  该文档记录了实测依据（709 条夹具按 UA 分组）、PC 端每个头的代码级来源、
  设备指纹的 14 段字段表，以及"哪些还不知道、**不要猜**"的清单 ——
  所以新增接口时不要把某一端的头硬编码进 `mcloud-upstream`。
- **写任何带"码"的请求前先读
  [`docs/device-and-source-codes.md`](./docs/device-and-source-codes.md)**：
  `toSourceId` / `channelSrc` / `clientType` / `sourceChannel` 这类码
  **不是端常量，是调用方按场景选的参数** —— 同一个 PC 客户端文件里同一条接口就有三个
  合法值（`001003` / `001005` / `001248`），旧项目为拿不同任务会轮流冒充
  PC / 安卓 / iPhone / Edge-on-Linux。该文档给了取值谱、分类判据
  （每端常量 / 每调用参数 / 每请求随机 / 服务路由）与"哪些码会**整套换头**"的清单。
  一句话判据：**这个码能在同一端的两个接口上取不同值吗？能，就不许钉进端档案。**
- **动工实现某个业务能力前先读
  [`docs/business-feature-inventory.md`](./docs/business-feature-inventory.md)**：
  旧项目 19 项能力逐条拆到"照着能写代码"的粒度 —— 上游请求序列、判据分支、副作用分级
  （哪些会在你网盘真建/真删文件、哪些真扣云朵、哪些产现金）、幂等缺口、依赖与共享状态，
  外加运行时层缺什么（run 与并发闸 / 调度 / 组合 / 推送 / CLI / WebUI）与硬编码常量清单。
  文末那张「动代码前必须实测确认的冲突」是上面几条文档要求的**具体检查表**，
  实现顺序仍以 `docs/rust-rewrite-plan.md` §7.1 为准，该文档只补细节不改顺序。
- **要参考 Go 项目 caiyun 的实现前先读
  [`docs/caiyun-go-feature-inventory.md`](./docs/caiyun-go-feature-inventory.md)**：
  它是上一篇的对照篇，对象是方案 §2 点名的参考项目（Go + Vue3 + MySQL + Redis，61,215 行）。
  24 条任务目录、抢兑子系统（下单响应分支表 / 12 类错误→重试决策表 / 三层锁与重复下单面）、
  运行时层（"一个自然日"的确切语义、熔断状态机、6 类令牌、事件可靠投递、跨任务态）与参数面全量，
  末尾给**搬 / 不搬 / 先修再搬**总表 + 带 `file:line` 的真实缺陷清单。
  一句话判据：**它值得搬的是判定内核与不变式，不是 Redis/MySQL 的实现形态** ——
  本项目仍是单进程 + SQLite（§2.4 的不照搬清单在该文 §8.2 逐项落到替代物）。

## 第三方内容

`.workbuddy/skills/daisyui/` 是 vendor 进来的 daisyUI 官方 agent skill（MIT）。
来源、固定版本、更新方式与完整许可声明见 [`THIRD-PARTY-NOTICES.md`](./THIRD-PARTY-NOTICES.md)。

## 许可

[Apache-2.0](./LICENSE)
