# docs/ 索引

> 这一页只做两件事：**把你导向正确的那一篇**，以及**说清两篇重复时谁说了算**。
> 具体结论一律不在这里复述 —— 复述就会过期。

---

## 该读哪篇

| 你现在要做什么 | 先读这个 |
| --- | --- |
| 看项目全貌 / 为什么重写 | [`../README.md`](../README.md)（只做导航，短） |
| 看**现在能跑什么、还差什么** | [`status.md`](./status.md) —— 一页总览 + 按任务指路 |
| 敲命令 / 查参数 | [`cli-usage.md`](./cli-usage.md) —— CLI 操作手册 |
| 改签到那条链路的**编排逻辑** | [`signin-cli-spec.md`](./signin-cli-spec.md) —— `mcloud signin` 的编排层规格 |
| 改**刷新 auth / 换 token**（该不该刷、哪一端的头、四个触发面口径、哪些还没实测） | [`auth-refresh.md`](./auth-refresh.md) —— 参考实现（旧项目）把它当**每轮任务收尾的常规动作**，本仓同样到点就换，没有"先批准"那道闸（§1）；⚠ 要留神的是 PC 那一支**形状未实测**（§8 待验证） |
| 改代码 / 跑门禁 / 搞清分层 | [`dev-gates.md`](./dev-gates.md) —— Go 侧十条门禁 + 分层铁律 |
| 加文件 / 起新目录 / 查命名 | [`repo-layout.md`](./repo-layout.md) —— 目录结构 + kebab-case 规则 |
| 动**迁移**相关的东西（还剩哪些步、当初踩过什么） | [`go-migration-plan.md`](./go-migration-plan.md) + [`migration-pitfalls.md`](./migration-pitfalls.md) |
| 实现某个**业务能力**（签到、领取、抢兑…） | [`business-feature-inventory.md`](./business-feature-inventory.md) —— 旧 TS 项目 19 项逐条拆到"照着能写代码" |
| 参考 Go 项目 caiyun 的实现 | [`caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md)（§0 ~ §2 总览与能力逐条）—— 上面那篇的对照篇；抢兑与运行时见 [`caiyun-go-exchange-and-runtime.md`](./caiyun-go-exchange-and-runtime.md)，取舍与参数面见 [`caiyun-go-parameters-and-decisions.md`](./caiyun-go-parameters-and-decisions.md) |
| 接**新的端**（安卓 / 邮箱 / 小程序） | [`client-profiles.md`](./client-profiles.md)（§1 ~ §4 端差异与请求头）+ [`client-profiles-fingerprint-and-landing.md`](./client-profiles-fingerprint-and-landing.md)（§5 ~ §8 设备指纹 · 落地形状 · 未知项） |
| 写任何带"**码**"的请求（sourceid / channel / 设备码） | [`device-and-source-codes.md`](./device-and-source-codes.md) —— 这些码是**调用参数**，不是端常量 |
| 做**登录**（PC 客户端 / PC Web / App） | [`upstream/`](./upstream/) 那一族，按端选（见下表） |
| 写**抓包文档**（提交前必读） | [`sanitize-guide.md`](./sanitize-guide.md) —— 脱敏 SOP + 守卫用法 + 泄露后如何改写历史 |
| 判断某个值**能不能进版本库** | [`credential-boundary.md`](./credential-boundary.md) —— 界线是"能不能进 GitHub" |
| 写**前端页面 / 组件** | [`webui-design-guide.md`](./webui-design-guide.md)（视觉与交互）→ 组件与移动端见 [`webui-components.md`](./webui-components.md) → 代码组织与验收见 [`webui-code-guide.md`](./webui-code-guide.md) |

---

## 全部文档

### 现状与规划

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`status.md`](./status.md) | 现在能跑什么、还差什么；按任务指路 | **实况**（最后核对 2026-09-22） |
| [`go-migration-plan.md`](./go-migration-plan.md) | **Rust → Go 的执行方案**：冻结策略、目录与包映射、十条门禁、顺序与每步退出判据 | **执行中** |
| [`migration-pitfalls.md`](./migration-pitfalls.md) | 实测踩坑登记（`§6.1` 那张表的**活文档**：编号沿用，新条目接在最后。⚠ **当前条数看文件抬头**，这里不抄数字） | **活文档** |
| [`business-feature-inventory.md`](./business-feature-inventory.md) | 旧 TS 项目 19 项能力逐条拆解 + 运行时层缺口 + 硬编码清单 | 编排中（抬头有 2026-09-22 的 Go 侧现状核对） |
| [`caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md) | **① 总览 · 四条公共语义 · 能力逐条（§0 ~ §2）** | 2026-09-22 拆三篇 |
| [`caiyun-go-exchange-and-runtime.md`](./caiyun-go-exchange-and-runtime.md) | **② 抢兑子系统与运行时层（§3 ~ §4）** —— 下单响应分支表、分类→重试决策表、锁与令牌桶、cron 与事件 | 同上，`§` 编号连续 |
| [`caiyun-go-parameters-and-decisions.md`](./caiyun-go-parameters-and-decisions.md) | **③ 参数面 · 硬编码 · 冲突 · 搬 / 不搬 · 验收步（§5 ~ §9）** —— 含 §8.4 那张**缺陷登记表**（`C-*` / `R-*` / `T-*` 编号，另两篇引用缺陷时用它） | 同上，`§` 编号连续 |

> **已随 Rust 代码剥离（2026-09-21）**：动工前的规划快照与两份 Rust 时期的文档住在同级仓库
> [`../../mcloud-rust/docs/`](../../mcloud-rust/docs) ——
> [`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md)（技术选型、分层、19 项风险序、实现顺序 §7.1）、
> [`rust-to-golang-migration.md`](../../mcloud-rust/docs/rust-to-golang-migration.md)（要不要迁 Go 的评估篇，**已决策**）、
> [`freeze-record.md`](../../mcloud-rust/docs/freeze-record.md)（冻结记录）、
> [`dev-gates.md`](../../mcloud-rust/docs/dev-gates.md)（Rust 侧七条）。
> 那边另存了一份 [`go-migration-plan.md`](../../mcloud-rust/docs/go-migration-plan.md) **只读副本**，
> 只为让 Rust 侧的人看得懂上下文 —— 那份不更新。
> 那些是**历史快照**，别再往里加东西；现行方案只有本仓库这一份 `go-migration-plan.md`。

### CLI 与签到

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`cli-usage.md`](./cli-usage.md) | 子命令、签到七个要点、退出码、迁移 | **规范** |
| [`signin-cli-spec.md`](./signin-cli-spec.md) | `mcloud signin` 编排层规格（状态机、参数、副作用边界） | **规范**（§1–§5 为现行；历史段见 `archive/`） |
| [`auth-refresh.md`](./auth-refresh.md) | **刷新 auth**（换 token）这一条链路：证据分档（App 实拍 / PC 未实测）、判据三档、单位陷阱、CLI + HTTP + WebUI + 调度四面口径、凭据边界 | **规范 + 证据**（2026-09-22 落地，2026-09-23 接上调度那一面） |

### 上游协议（`docs/upstream/`，按端分文件、互为姊妹篇）

| 文档 | 端 / 链路 | 状态 |
| --- | --- | --- |
| [`upstream/login-protocol.md`](./upstream/login-protocol.md) | **PC 客户端**扫码登录（二维码本地生成、`thirdlogin` 轮询状态机） | 已确认（流量 + 客户端源码双向） |
| [`upstream/logout-sms-login.md`](./upstream/logout-sms-login.md) | **PC 客户端**退出 + 短信登录 | 已确认 |
| [`upstream/web-qr-login.md`](./upstream/web-qr-login.md) | **PC Web** 扫码登录（CDP 实测，Web 端 `7.18.0`） | 已确认 |
| [`upstream/app-sms-login.md`](./upstream/app-sms-login.md) | **iOS App** 短信登录（反汇编 `MCLoginCryptoUtil` + 活体探针） | 已确认 |
| [`upstream/app-business-api.md`](./upstream/app-business-api.md) | **iOS App** 业务接口的 `signature` / `encodeData` / `hcy-cool-flag` 三把锁 | 已实测全量验证 |

> 这五篇共用同一套证据口径：**只记实测，推断显式标注**，采集时间与环境写在抬头。
> 新增一篇时照抄这个抬头格式（见文末「抬头模板」）。

### 跨端与参数

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`client-profiles.md`](./client-profiles.md) | **① 端差异与请求头**（§1 结论 · §2 证据来源 · §3 按 UA 分组的实测头 · §4 PC 代码级证据） | 2026-09-22 拆两篇；PC + iOS App 已落地，其余端待实现 |
| [`client-profiles-fingerprint-and-landing.md`](./client-profiles-fingerprint-and-landing.md) | **② 设备指纹与落地形状**（§5 三种指纹 · §6 落地形状 · §7 未知项 · §8 与代码的关系） | 同上，`§` 编号连续 |
| [`device-and-source-codes.md`](./device-and-source-codes.md) | 设备码 / 来源码 / 渠道码的分类判据与取值谱 | 结论已定 |

### 前端（三篇，同一个项目的三份）

| 文档 | 覆盖 | 状态 |
| --- | --- | --- |
| [`webui-design-guide.md`](./webui-design-guide.md) | §一–§四：这份文件管什么、设计基调、主题与色彩、布局骨架 | **规范** |
| [`webui-components.md`](./webui-components.md) | §五–§八：组件类名、交互、移动端细则、动效 | **规范** |
| [`webui-code-guide.md`](./webui-code-guide.md) | §九–§十二：代码组织、落地流程、验收清单、未决项 | **规范** |

> 三篇是 2026-09-22 从一篇 1092 行的文档里拆出来的，**§ 编号沿用拆分前那一版** ——
> 别处写的 `webui §4.5`、`§九` 仍然指得到。互相引用见每篇抬头。

### 流程与运维

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`dev-gates.md`](./dev-gates.md) | Go 侧十条门禁 + 分层铁律（依赖方向的可执行断言） | **规范**（改代码前必读） |
| [`repo-layout.md`](./repo-layout.md) | 目录结构 + 文件命名（kebab-case 及三类豁免） | **规范**（加文件前必读） |
| [`credential-boundary.md`](./credential-boundary.md) | 凭据边界：哪些值能进 GitHub、哪些只能进日志 | **规范** |
| [`sanitize-guide.md`](./sanitize-guide.md) | 抓包脱敏 SOP、标识守卫用法、真实值泄露后如何改写历史 | 最后核对 2026-09-19 |

### `docs/archive/`（归档：不删，但**不当现状读**）

| 文档 | 是什么 | 为什么在这儿 |
| --- | --- | --- |
| [`archive/migration-log.md`](./archive/migration-log.md) | 逐日实况流水 | 从 `status.md` 拆出；现状只看 `status.md` |
| [`archive/go-migration-crate-notes.md`](./archive/go-migration-crate-notes.md) | 逐 crate 的迁移要点 | 从 `go-migration-plan.md` 拆出；crate 已随基准冻结 |
| [`archive/signin-cli-spec-history.md`](./archive/signin-cli-spec-history.md) | 签到规格 §0 / §6 / §7 / §8 | 从 `signin-cli-spec.md` 拆出；现行只看 §1–§5 |
| [`archive/docs-audit-20260919.md`](./archive/docs-audit-20260919.md) | 一次性文档体检报告 | 结论已消化进各篇 |

> 归档的规矩：**只进不出、只追加不改写**。里面凡引用 `§6.1 #N`、`mcloud-<crate>::…` 处，
> 说的都是 Rust 基准那张表和那些旧路径（现在的写法见下面「代码落点怎么写」）。

### `capture/` 原始素材

| 文件 | 内容 |
| --- | --- |
| `apiServer.js` | **PC 客户端源码**（`app.asar` 的 `out/renderer/api/apiServer.js`）—— 协议对照用 |
| `pcclient-api.js` | **同上姊妹文件**（同目录 `api.js`） |
| `app-api-families.txt` | App 业务接口的两套互斥形态（A：`x-yun-uni` 无 `signature`；B：反之） |
| `app-signin-samples.txt` | App 签到链路抓包样本 |
| `app-rights-token-samples.txt` | App 权益 / token / 待领取族样本（`queryAvailableBenefit`、`queryUserBenefits`、`refreshToken`、`queryRights`、`pointsNotClaimed`） |
| `app-exchange-samples.txt` | App **兑换**族样本（含 `§5.5`/`§5.6`）—— 判据：上游码**只判等、不判形状**（`2301` 是四位） |
| `app-live-redflower-samples.txt` | App **直播口令 / 小红花**族样本（`§5.x`）—— ⚠ `§6` 那条"WS 结构性地抓不到"的判据（`chatRoomId: null`）在这里；实现落点见 [`status.md`](./status.md) |
| `probe-live-room-<日期>.md` | **脚本产出**（`scripts/probe-live-room.js`）—— 开播后每次核对都往这里**追加**一次运行记录与原始输出。⚠ 清单的权威版本在**脚本里**，这里只是记录（坑表 #116） |
| `app-sms-login-samples.txt` | App 短信登录抓包样本（**含 `§0` 的取证方法**） |
| `web-qr-login-samples.txt` | PC Web 扫码登录样本 |
| `logout-sms-requests.txt` | PC 客户端退出 + 短信请求 |
| `thirdlogin-requests.csv` / `thirdlogin-responses.csv` | `thirdlogin` 的请求 / 响应全量导出 |

> 上面前两个 `.js` 是**上游客户端源码**，在 `scripts/check-identifiers.cjs` 的
> `VENDOR_SOURCE_PATHS` 里按**来源**豁免。**新增素材要逐个判断并加进那个清单**，
> 别因为"在 `capture/` 目录下"就默认安全（该清单漏过一个，见
> [`sanitize-guide.md`](./sanitize-guide.md) §二）。

---

## 权威关系（谁说了算）

同一件事在两篇里各写一份，迟早对不上。规则是：**下面每一行的"以谁为准"那一篇写全，其余各篇只指路不复述。**

| 争什么 | 以谁为准 | 别处怎么处理 |
| --- | --- | --- |
| **实现进度**、当前测试数、当前能力边界 | [`status.md`](./status.md) | 各篇抬头写"现状以 `status.md` 为准"，正文不抄数字 |
| **门禁条数与怎么跑** | [`dev-gates.md`](./dev-gates.md) | `go-migration-plan.md` §5 只写"为什么是这十条" |
| **文件放哪 / 叫什么名** | [`repo-layout.md`](./repo-layout.md) | 脚本 `check-filenames.js` 的注释指向它 |
| **CLI 参数与语义** | [`cli-usage.md`](./cli-usage.md) | `signin-cli-spec.md` 只写编排层，参数不重复定义 |
| **签到链路的编排规格**（状态机、副作用边界） | [`signin-cli-spec.md`](./signin-cli-spec.md) §1–§5 | 历史段在 `archive/`，只作证据不作现行 |
| **迁移还剩哪些步、每步退出判据** | [`go-migration-plan.md`](./go-migration-plan.md) | 逐 crate 要点在 `archive/` |
| **踩过的坑（编号登记表）** | [`migration-pitfalls.md`](./migration-pitfalls.md) | 别处写 `§6.1 #92` 就是本文的 `#92`；**新条目接在最后** |
| **单项能力的实现规格** | [`business-feature-inventory.md`](./business-feature-inventory.md) | 对照篇只写"caiyun 怎么做"，不覆写规格 |
| **上游协议细节**（字段、状态机、错误码） | `docs/upstream/` 里对应那一篇 | 盘点与 CLI 文档只引用，不重抄抓包 |
| **跨端头 / 设备串** | [`client-profiles.md`](./client-profiles.md) + [`client-profiles-fingerprint-and-landing.md`](./client-profiles-fingerprint-and-landing.md) | 与 `upstream/` 的分工：那五篇是一条链路一次取证，本篇是**横向对比** |
| **码值（sourceid / channel / 设备码）** | [`device-and-source-codes.md`](./device-and-source-codes.md) | 代码注释里那几处"见设备码文档"都指它 |
| **哪些值能进版本库** | [`credential-boundary.md`](./credential-boundary.md)（界线）+ [`sanitize-guide.md`](./sanitize-guide.md)（做法） | 一条定界线、一条定操作，不合并 |
| **前端长什么样 / 怎么写** | `webui-design-guide.md` → `webui-components.md` → `webui-code-guide.md` | 技术选型那条仍以 [`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md) §2.2（历史快照）为出处，现行约束看 `webui-design-guide.md` |
| **实现顺序**（先做哪项能力） | [`go-migration-plan.md`](./go-migration-plan.md) | `rust-rewrite-plan.md` §7.1 是历史快照 |

---

## 代码落点怎么写（2026-09-22 定）

Rust 剥离之后，仓库里同时出现过两套路径写法，读者分不清哪个是现在的东西。定为：

1. **指本仓库的 Go 代码**：写 `pkg.Symbol`（`upstream.AppProfile`、`app.RecordCloud`、
   `core.NextCloudStats`），需要文件时用 `internal/upstream/app_client.go` 这种**从仓库根算起**的路径。
   包名以 `go doc` 里的为准（注意 `internal/io` 的包名是 `mcloudio`）。
2. **指 Rust 基准**：写"Rust 侧"四字 + 同级仓库路径 `../mcloud-rust/crates/…`（markdown 链接里要写
   `../../mcloud-rust/…`，因为本文在 `docs/` 下）。
3. **`mcloud-<crate>::xxx` 这种写法只保留在历史记录里**：`archive/` 整棵、`migration-pitfalls.md` 里
   明说"Rust 侧"的条目、以及各篇**订正块引的旧原句**。除此之外别再新增 ——
   2026-09-22 已把当时剩下的 35 处逐条核过 Go 落点并改掉。

---

## 抬头模板（新增文档时照抄）

`upstream/` 那一族的写法最好用，**建议推广**：

```markdown
# 标题（**当前落地状态**）

> 状态：**已确认** | 提案 | 编排中。证据链 = …（列出几条独立证据）
>
> - 采集时间：YYYY-MM-DD
> - 采集环境：<OS / 客户端版本>
> - 采集方式：<抓包工具 / CDP / 反汇编 / 活体探针>
> - 配套文档：同族那几篇，逐个列出（`docs/upstream/…`、`docs/…`）
> - 依据 `../../mcloud-rust/docs/rust-rewrite-plan.md` §7.0：**本文件全部来自实测，未落笔任何猜测性接口**。
```

两条硬要求：

1. **状态要点名**：是"已实测确认"还是"提案"还是"编排中"，别让读者猜。
2. **不确定的显式标注**，最好集中成一节「未知项（不猜，等证据）」——
   [`client-profiles-fingerprint-and-landing.md`](./client-profiles-fingerprint-and-landing.md) §7 是范本。
