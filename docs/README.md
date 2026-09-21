# docs/ 索引

## 该读哪篇

| 你现在要做什么 | 先读这个 |
| --- | --- |
| 看项目全貌 / 为什么重写 | [`../README.md`](../README.md)（只做导航，短） |
| 敲命令 / 查参数 | [`cli-usage.md`](./cli-usage.md) —— CLI 操作手册 |
| 改代码 / 跑门禁 / 搞清分层 | [`dev-gates.md`](./dev-gates.md) —— 七条门禁 + 分层铁律 |
| 加文件 / 起新目录 / 查命名 | [`repo-layout.md`](./repo-layout.md) —— 目录结构 + kebab-case 规则 |
| 看**现在能跑什么、还差什么** | [`status.md`](./status.md) —— 实现进度与逐项须知 |
| 实现某个**业务能力**（签到、领取、抢兑…） | [`business-feature-inventory.md`](./business-feature-inventory.md) —— 旧项目 19 项逐条拆到"照着能写代码" |
| 参考 Go 项目 caiyun 的实现 | [`caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md) —— 上面那篇的对照篇 |
| 接**新的端**（安卓 / 邮箱 / 小程序） | [`client-profiles.md`](./client-profiles.md) —— 各端差异真正大在请求头与设备串 |
| 写任何带"**码**"的请求 | [`device-and-source-codes.md`](./device-and-source-codes.md) —— 这些码是**调用参数**，不是端常量 |
| 做**登录**（PC 客户端 / PC Web / App） | 见下面「登录协议」一族，按端选 |
| 写**抓包文档**（提交前必读） | [`sanitize-guide.md`](./sanitize-guide.md) —— 脱敏 SOP + 守卫用法 + 泄露后如何改写历史 |
| 判断某个值**能不能进版本库** | [`credential-boundary.md`](./credential-boundary.md) —— 界线是"能不能进 GitHub" |
| 写**任何前端页面 / 组件** | [`webui-design-guide.md`](./webui-design-guide.md) —— 视觉与交互的**唯一依据**，含移动端硬要求 |

---

## 全部文档

### 规划与盘点

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`rust-rewrite-plan.md`](./rust-rewrite-plan.md) | 动工前的规划快照（技术选型、分层、19 项能力风险序、实现顺序 §7.1） | **提案**（写于动工前，进度以 README 为准） |
| [`business-feature-inventory.md`](./business-feature-inventory.md) | 旧 TS 项目 19 项能力逐条拆解 + 运行时层缺口 + 硬编码清单 | 编排中 |
| [`caiyun-go-feature-inventory.md`](./caiyun-go-feature-inventory.md) | Go 参考项目对照（24 条任务、抢兑子系统、搬/不搬总表） | 编排中 |
| [`go-migration-plan.md`](./go-migration-plan.md) | **Rust → Go 的执行方案**：冻结策略、目录与包映射、九条门禁、逐 crate 要点、顺序与每步退出判据 —— **与任何文档冲突时以它为准** | **执行中**（Rust 侧已冻结） |
| [`rust-to-golang-migration.md`](./rust-to-golang-migration.md) | 上面那篇的**评估篇**：规模/依赖对照/四条结构性难点/工作量与是否值得（数字为 2026-09-20 实测，会过期） | 评估（**已决策**） |

### 登录协议（按端分文件，互为姊妹篇）

| 文档 | 端 / 链路 | 状态 |
| --- | --- | --- |
| [`upstream-login-protocol.md`](./upstream-login-protocol.md) | **PC 客户端**扫码登录（二维码本地生成、`thirdlogin` 轮询状态机） | 已确认（流量 + 客户端源码双向） |
| [`upstream-logout-sms-login.md`](./upstream-logout-sms-login.md) | **PC 客户端**退出 + 短信登录 | 已确认 |
| [`upstream-web-qr-login.md`](./upstream-web-qr-login.md) | **PC Web** 扫码登录（CDP 实测，Web 端 `7.18.0`） | 已确认 |
| [`upstream-app-sms-login.md`](./upstream-app-sms-login.md) | **iOS App** 短信登录（反汇编 `MCLoginCryptoUtil` + 活体探针） | 已确认 |
| [`upstream-app-business-api.md`](./upstream-app-business-api.md) | **iOS App** 业务接口的 `signature` / `encodeData` / `hcy-cool-flag` 三把锁 | 已实测全量验证 |

> 这五篇共用同一套证据口径：**只记实测，推断显式标注**，采集时间与环境写在抬头。
> 新增一篇时照抄这个抬头格式（见文末「抬头模板」）。

### 跨端与参数

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`client-profiles.md`](./client-profiles.md) | 各端请求头 / 设备串差异、三种指纹（设备串 / smid / deviceShort）、未知项清单 | PC + iOS App 已落地；其余端待实现 |
| [`device-and-source-codes.md`](./device-and-source-codes.md) | 设备码 / 来源码 / 渠道码的分类判据与取值谱 | 结论已定 |

### 前端

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`webui-design-guide.md`](./webui-design-guide.md) | WebUI 设计规范：主题色彩、布局骨架、组件类名、移动端细则、验收清单 | **规范**（后续美工设计的唯一依据） |

### 流程与运维

| 文档 | 一句话 | 状态 |
| --- | --- | --- |
| [`dev-gates.md`](./dev-gates.md) | 七条门禁 + 分层铁律（依赖方向的可执行断言） | **规范**（改代码前必读） |
| [`repo-layout.md`](./repo-layout.md) | 目录结构 + 文件命名（kebab-case 及三类豁免） | **规范**（加文件前必读） |
| [`status.md`](./status.md) | 实现进度与现状须知：现在能跑什么、还差什么 | **实况**（最后核对 2026-09-19） |
| [`cli-usage.md`](./cli-usage.md) | CLI 操作手册：子命令、签到七个要点、退出码、迁移 | **规范** |
| [`credential-boundary.md`](./credential-boundary.md) | 凭据边界：哪些值能进 GitHub、哪些只能进日志 | **规范** |
| [`sanitize-guide.md`](./sanitize-guide.md) | 抓包脱敏 SOP、标识守卫用法、真实值泄露后如何改写历史 | 最后核对 2026-09-19 |
| [`docs-audit-20260919.md`](./docs-audit-20260919.md) | 本文档全库体检报告（过期项、矛盾项、待优化项） | 一次性产物，**可归档** |

### `capture/` 原始素材

| 文件 | 内容 |
| --- | --- |
| `apiServer.js` | **PC 客户端源码**（`app.asar` 的 `out/renderer/api/apiServer.js`）—— 协议对照用 |
| `pcclient-api.js` | **同上姊妹文件**（同目录 `api.js`） |
| `app-api-families.txt` | App 业务接口的两套互斥形态（A：`x-yun-uni` 无 `signature`；B：反之） |
| `app-signin-samples.txt` | App 签到链路抓包样本 |
| `app-rights-token-samples.txt` | App 权益 / token / 待领取族样本（`queryAvailableBenefit`、`queryUserBenefits`、`refreshToken`、`queryRights`、`pointsNotClaimed`） |
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

文档之间出现冲突时的判据，避免各篇自己写一份：

| 争什么 | 以谁为准 |
| --- | --- |
| **实现进度**、当前测试数、当前能力边界 | [`status.md`](./status.md) 的「一句话总览」与逐项须知 |
| **门禁条数与怎么跑** | [`dev-gates.md`](./dev-gates.md) |
| **文件放哪 / 叫什么名** | [`repo-layout.md`](./repo-layout.md) |
| **CLI 参数与语义** | [`cli-usage.md`](./cli-usage.md) |
| **实现顺序**（先做哪项能力） | [`rust-rewrite-plan.md`](./rust-rewrite-plan.md) §7.1 |
| **单项能力的实现规格** | [`business-feature-inventory.md`](./business-feature-inventory.md) |
| **上游协议细节**（字段、状态机、错误码） | 对应的 `upstream-*.md` |
| **跨端头 / 设备串** | [`client-profiles.md`](./client-profiles.md) |
| **哪些值能进版本库** | [`credential-boundary.md`](./credential-boundary.md)（界线）+ [`sanitize-guide.md`](./sanitize-guide.md)（做法） |
| **前端长什么样 / 怎么写** | [`webui-design-guide.md`](./webui-design-guide.md)（技术选型仍以 [`rust-rewrite-plan.md`](./rust-rewrite-plan.md) §2.2 为准） |

---

## 抬头模板（新增文档时照抄）

`upstream-*` 那一族的写法最好用，**建议推广**：

```markdown
# 标题（**当前落地状态**）

> 状态：**已确认** | 提案 | 编排中。证据链 = …（列出几条独立证据）
>
> - 采集时间：YYYY-MM-DD
> - 采集环境：<OS / 客户端版本>
> - 采集方式：<抓包工具 / CDP / 反汇编 / 活体探针>
> - 配套文档：`docs/xxx.md`（同族互链）
> - 依据 `docs/rust-rewrite-plan.md` §7.0：**本文件全部来自实测，未落笔任何猜测性接口**。
```

两条硬要求：

1. **状态要点名**：是"已实测确认"还是"提案"还是"编排中"，别让读者猜。
2. **不确定的显式标注**，最好集中成一节「未知项（不猜，等证据）」——
   [`client-profiles.md`](./client-profiles.md) §7 是范本。
