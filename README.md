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

当前状态（2026-09-17）：**241 个测试通过**，其余四项全绿。
其中 `mcloud-upstream` 的 L2 夹具回放已跑通（`crates/mcloud-upstream/tests/l2_replay.rs`）——
它吃的 `test/fixtures/http/` 是**本机**测试数据，不进版本库（见下），没有时这组用例自动跳过。

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
| `login sms` | 短信验证码登录：发码 → 输验证码。被风控要求滑块时**自动过**（缺口位置由服务端加密下发，见 `docs/upstream-logout-sms-login.md` §2.2.1） |
| `migrate` | 把旧项目 mcloud-sign 的数据迁移到 SQLite（`--dry-run` 预演） |
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

## 目录结构

```
crates/         7 个 crate（见上表）
scripts/        门禁与夹具生成器
  gen-*.cjs       —— 夹具一律从真实产物生成，不手写期望值
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

- **服务入口尚未实现。** `mcloud-server` 目前只有 lib，没有 `serve` 命令；
  axum 路由、SSE、`openapi.json` 导出与 `rust-embed` 内嵌前端都在 P5。
  所以现阶段 **CLI 能跑，WebUI 没有后端可连**，前端只能靠 `vite dev` 单独起。
- **CLI 已能完成"从零到登录"**：`mcloud user init` → `mcloud login qr`（终端二维码）/ `mcloud login sms`。
  登录态落在本地库的 `account_state`（凭据与展示信息**分表**，见 schema 说明）；
  设备指纹写在 `data/device.txt`（**换掉它等于换一台设备**，服务端会定位不到会话）。
- **上游接口层按新抓包逐块重写。** 目前只有 `login`（扫码/短信/账密 + 退出登录）是完整的，
  它是"按新流量写接口"的第一块；其余云盘业务接口仍是骨架（`Upstream` + `Envelope` + `base`）。
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

## 第三方内容

`.workbuddy/skills/daisyui/` 是 vendor 进来的 daisyUI 官方 agent skill（MIT）。
来源、固定版本、更新方式与完整许可声明见 [`THIRD-PARTY-NOTICES.md`](./THIRD-PARTY-NOTICES.md)。

## 许可

[Apache-2.0](./LICENSE)
