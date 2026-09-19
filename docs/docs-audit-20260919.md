# 文档体检报告（2026-09-19）

> **本报告是一次性产物，不必长期维护** —— 它记录的是"这一天文档长什么样"。
> 下面每条都在**当次就修掉了**（见「七、已修清单」），保留此文是为了让后来人
> 知道**当初哪里错了、为什么错** —— 尤其是 2.1 那种"图与表自相矛盾"的形态，
> 它不会自己暴露，只能靠对代码。

范围：`README.md` + `docs/` 全部 13 篇 Markdown（合计约 5,300 行）+ `docs/capture/` 素材。
方法：**每条结论都对着代码/脚本/实跑结果核对过**，不是通读观感。

---

## 一、总评

文档质量**整体偏高**，一个明显特征：绝大多数文档都自带"状态 + 证据口径 + 未知项"抬头，
而且敢写"我不知道"（`client-profiles.md` §7 是一张明明白白的未决清单）。
这种做法在重写项目里很珍贵 —— 大部分文档的失败模式是"看起来很全，但没说哪句是猜的"。

但拉取云端代码之后，**只有 `README.md` 被更新了**，其余文档的"现状"章节停在了各自写就的那天。
所以问题集中在**状态类表述过期**，而不是内容错。

按严重度分三档：

| 档 | 数量 | 性质 |
| --- | --- | --- |
| 🔴 真错（会误导实现） | 3 | 说得与代码相反，照着做会写错代码 |
| 🟡 过期（数字/状态旧了） | 6 | 方向没错，但当时的一帧被当成常驻事实 |
| 🔵 待优化（结构/一致性） | 5 | 不影响正确性，但增加维护成本 |

---

## 二、🔴 真错：与代码直接矛盾

### 2.1 `README.md` 分层图里有一条**自相矛盾**的重复行

第 20–25 行的依赖图：

```
mcloud-app           → core, upstream, capabilities, io          组装层（两个产物共用）
mcloud-server        → core, capabilities, exec, io, app         产物之一（HTTP/WebUI）
mcloud-cli           → core, upstream, capabilities, io, app     产物之一（命令行）
mcloud-cli           → core, capabilities, server         可执行入口      ← 这一行
```

`mcloud-cli` 出现了**两次**，依赖列表完全不同。

**实测的真相**（`crates/mcloud-cli/Cargo.toml`）：

```toml
mcloud-core, mcloud-capabilities, mcloud-upstream, mcloud-app, mcloud-io
# **刻意不依赖 mcloud-server**
```

即第一行是对的，第二行是**上一轮重构前的残留**。而且它不是"无害的废话" ——
它描述的正是 `scripts/check-deps.sh` 里 `FORBIDDEN_EDGES=("mcloud-cli:mcloud-server")`
**明令禁止**的那条边。表里的约束行（"**不得**依赖 `mcloud-server`"）与它直接打架。

同一个图下面还有一句同样过期的话：

> `mcloud-cli` 则**不**直依赖 `mcloud-io`，它只做参数解析。

实际上 cli **是**依赖 `mcloud-io` 的（组装层的必要组成），这句话反了。

### 2.2 `docs/business-feature-inventory.md` §0 的"当前 Rust 侧"整段过期

第 40 行原文：

> **当前 Rust 侧可跑的东西**：登录（PC 短信/扫码、App 短信）、账号会话与归属（schema v4）、
> 设备指纹 smid（v5）、旧数据迁移、`mcloud-app::cloud` / `signin` 的上游封装（并行工作流，**未提交**）。
> **19 项业务能力：0 项可执行**

三处与现状不符：

| 文档说 | 实际 |
| --- | --- |
| schema v4 / v5 | **v7**（`mcloud-io/src/store/schema.rs:22` 是 `SCHEMA_VERSION: i64 = 7`） |
| "并行工作流，未提交" | **已提交**（云端 9 个提交已落地，本地又叠了 `0fd68ec`） |
| "未提"隐含 `signin` 链路未完成 | App 签到链路**已全链路完成**（`abeda14`），含 smid 设备指纹 |

写这段时的语境是"两份功能盘点刚写完、实现还在飞"，现在语境消失了。

### 2.3 `docs/rust-rewrite-plan.md` 的"六项门禁"名不副实

第 957 行的进度表把六项门禁写成：

```
cargo test / clippy -D warnings / fmt --check / 分层 / 产物解耦 / 标识守卫
```

而 README「门禁」一节的六项是：

```
cargo test / clippy / fmt --check / check-deps.sh / check-identifiers.cjs / check-golden-vectors.cjs
```

差异在于：`rust-rewrite-plan` 把 `check-deps.sh` **拆成了"分层 + 产物解耦"两项**来凑够六条，
从而**漏掉了黄金向量守卫**（`check-golden-vectors.cjs`，122 条断言）。
它引用的计数"554 个测试"也是拉取前的数（现为 586）。

这条属于"两处各说各的六项"，最容易被后来人当成"六项 = 反正是那六项"糊过去。

---

## 三、🟡 过期：数字与状态

| # | 位置 | 现状 | 应更新为 |
| --- | --- | --- | --- |
| 3.1 | `README.md:244` | `crates/  7 个 crate（见上表）` | **8 个**（`mcloud-app` 已加入，上表列的就是 8 个 —— 表格与目录说明自相矛盾） |
| 3.2 | `README.md:77` | 554 个测试通过 | **586 个**（2026-09-19 实跑） |
| 3.3 | `docs/rust-rewrite-plan.md:957` | 554 个测试 | 586（同上，且该行自称"每次都会当天过期"，那就更该更新或标注日期） |
| 3.4 | `docs/client-profiles.md:638` | 落库 `account_state.*`（**schema v5**） | v7（smid 两列确实是 v5 加的，但读者会以为当前就是 v5） |
| 3.5 | `docs/business-feature-inventory.md:331,333` | `schema 已有 v4 归属字段` / `schema v5 已完成` | 同 3.4，应显式写"（现为 v7）" |
| 3.6 | `docs/caiyun-go-feature-inventory.md:383` | `smid_fingerprint`/`smid_device_id`（schema v5） | 同上 |

> 3.4–3.6 这几处严格说**没写错**（那两列确实是 v5 引入的），但句式是"（schema v5）"这种
> 省略式，读起来像在描述当前版本。建议统一改成"v5 引入，现为 v7"的写法。

---

## 四、🔵 待优化：结构与一致性

### 4.1 两篇文档没有任何入径（孤儿）

| 文档 | 被引用次数 | 问题 |
| --- | --- | --- |
| `upstream-web-qr-login.md` | **0** | 563 行的联网登录协议文档，README 与其它文档都没链它 |
| `ltsc-store-yingyongbao-guide.md` | **0** | 与 mcloud 项目无关的 Windows 运维文档，混在 `docs/` 里 |

`upstream-web-qr-login.md` 尤其可惜 —— 它和 `upstream-login-protocol.md`（PC 客户端）、
`upstream-logout-sms-login.md` 是同一族的证据，但后两者互相链了，它没有。
**建议**：在 `upstream-login-protocol.md` 与 `upstream-logout-sms-login.md` 顶部补"同族文档"互链，
并在 README 的文档索引段加一条。

`ltsc-store-yingyongbao-guide.md` 属于"本机环境搭建"而非项目内容，
建议移到 `docs/ops/`（或干脆移出仓库），避免污染协议文档的检索路径。

### 4.2 `docs/` 缺一个索引

13 篇文档、5,300 行，没有 `docs/README.md`。当前唯一的入口是 `README.md` 里那段
"现状须知"，但它是**叙述式**的（"要参考 Go 项目前先读 …"），
不是一张能扫到全部文档的清单 —— 4.1 那两个孤儿正是这样漏掉的。

**建议**：加 `docs/README.md`，一张表列全 13 篇：文件名 / 一句话 / 状态（已确认·提案·编排中）/ 证据链。
顺带把"哪篇是权威"讲清（现在"两文冲突时以 X 为准"这句话散在三篇文档里）。

### 4.3 `apiServer.js` 的命名与豁免不对称

`docs/README.md` 的文件命名规则要求非 Rust 文件一律 kebab-case，
而 `docs/capture/` 下有两个**违反它**的文件：

| 文件 | 是否在 `VENDOR_SOURCE_PATHS` |
| --- | --- |
| `apiServer.js` | ✓ 是（`check-identifiers.cjs:125`） |
| `pcclient-api.js` | ✗ **不是** |

两者都是上游客户端源码素材（`apiServer.js` 来自 PC 客户端 asar 的
`out/renderer/api/apiServer.js`；`pcclient-api.js` 来自同目录的 `api.js`，
首行就是 `console.log('************api.js')`）。
**豁免清单漏了第二个** —— 目前它没报错只是因为它恰巧没含真实标识，
属于"运气好"，不是"守住了"。

> 命名上两者都带驼峰，但既然是**上游原名**，改名的收益低于"读文档时能对上号"的收益，
> 所以问题不在命名，而在**豁免清单没与素材同步**。

### 4.4 交叉引用里的路径失效

`docs/rust-rewrite-plan.md` 通篇引用 `/Users/catlair/mobile/workspace/mcloud`（macOS 路径），
而当前实际工作目录是 `E:\workspace\mcloud`。这本身**不算错**（该文档自述是"动工前的规划快照"，
写于 macOS 上），但同族的其它文档也照抄了这个路径：

| 位置 | 内容 |
| --- | --- |
| `business-feature-inventory.md:3`、`device-and-source-codes.md:78` | 旧项目 `/Users/catlair/Documents/workspace/mcloud-sign` |
| `caiyun-go-feature-inventory.md:5`、`rust-rewrite-plan.md:8,1080` | caiyun clone 于 `/tmp/caiyun-ref` |

这些是**证据出处**，路径本身有留存价值（说明"当时是在哪读的"）。
但 `/tmp/caiyun-ref` 这种路径已经不存在了，建议加一句"（本次核查时的 clone 路径，
commit `1b2cf07` 才是权威锚点）" —— 现在该文档已经写了 commit，只差说明"路径会失效、commit 不会"。

### 4.5 抬头格式不统一

13 篇文档用了 5 种不同的抬头写法：

| 写法 | 用在哪 |
| --- | --- |
| `> 状态：**已确认**。来源为…` | `upstream-login-protocol.md` |
| `> 状态：PC 端已落地…；iOS App 端已落地…` | `client-profiles.md` |
| `> **这份文档是什么**：…` | `business-feature-inventory.md` |
| `> **定位**：…` | `caiyun-go-feature-inventory.md` |
| 无抬头，直接进正文 | `sanitize-guide.md`、`ltsc-*.md` |

`upstream-*` 那一族（4 篇）有很统一的三段式（状态 / 证据链 / 采集时间与环境），
值得**推广成全库模板**。`sanitize-guide.md` 是 SOP，本不需要"证据链"，
但它缺"最后更新"信息 —— 而它管着提交前必跑的守卫，改了守卫却不更新这份 SOP 是最容易漏的。

---

## 五、建议的动手顺序

按"投入产出比"排（也正是本次实际执行的顺序）：

1. **修 🔴 三处**（半小时内）—— 都是改几句话，但每一处都能直接误导实现
2. **补 `docs/README.md` 索引**（解决 4.1 的两个孤儿 + 统一"哪篇权威"）
3. **刷新 🟡 六个数字/版本**（纯替换）
4. **给豁免清单补 `pcclient-api.js`**（4.3，一行）
5. **统一抬头模板**（4.5，量最大、收益最慢）

---

## 六、附：核对过的项（确认**没有**问题）

避免这份报告被当成"什么都值得改"，以下都已核对且结论是**不用动**：

- **文档间链接全部有效** —— 逐个解析跨文档引用，无死链（含本次新增的族内互链）
- **`docs/capture/` 9 个素材文件齐全**，与文档引用一一对应
- **`sanitize-guide.md` 与守卫实现一致** —— `VENDOR_SOURCE_PATHS`、base64 二次扫描、
  "过度脱敏"判据、`--force-with-lease` 流程都对得上
- **`device-and-source-codes.md`、`upstream-app-business-api.md` 的"状态：已定/已实测"** 属实，
  内容与 `mcloud-upstream` 常量逐条对得上
- **`.gitignore` 与文档声明一致** —— `.workbuddy-ai/*` 忽略、`har/` 忽略、
  `test/fixtures/http/` 忽略，三条都在
- **README 的"门禁六条"**（第 54 行起）与实际脚本一致，无遗漏

---

## 七、已修清单（本次一并处理）

| 报告节 | 改了什么 | 落在哪 |
| --- | --- | --- |
| 2.1 | 删掉分层图里重复且与门禁矛盾的 `mcloud-cli → … server` 行；补"CLI 不得依赖 server"的正向说明；修掉"不直依赖 mcloud-io"那句反话 | `README.md` |
| 2.2 | 「当前 Rust 侧可跑的东西」整段订正到 schema v7 / 已提交状态，并注明上一版的语境 | `business-feature-inventory.md` |
| 2.3 | 门禁清单从"分层+产物解耦"两项改成实际六条命令（补回黄金向量守卫）；554→586 | `rust-rewrite-plan.md` |
| 3.1 | `7 个 crate` → `8 个` | `README.md` |
| 3.2 / 3.3 | 测试数 554 → 586（README 与方案各一处，均注明新旧 | `README.md`、`rust-rewrite-plan.md` |
| 3.4–3.6 | 三处 `(schema v5)` 改成"v5 引入，现为 v7" | `client-profiles.md`、`business-feature-inventory.md`、`caiyun-go-feature-inventory.md` |
| 4.1 | 登录协议五篇**互链成族**（原先只有 2 篇互链），两个孤儿文档由此有了入径 | 5 篇 `upstream-*.md` |
| 4.2 | **新增 `docs/README.md` 索引**：按"你要做什么"选文档 + 权威关系表 + 抬头模板 | `docs/README.md` |
| 4.3 | `pcclient-api.js` 补进 `VENDOR_SOURCE_PATHS`，并把这个漏项的教训写进注释与 SOP | `check-identifiers.cjs`、`sanitize-guide.md` |
| 4.5 | `sanitize-guide.md` 补"最后核对"标记（它管着提交前必跑的守卫，最容易改守卫不改文档） | `sanitize-guide.md` |

顺带在 `README.md` 的文档索引段加了指向 `docs/README.md` 的一行 ——
否则新索引自己也会变成孤儿（这正是 4.1 那个问题的成因）。

**未处理**（有意留下，属于"当事人自己决定"）：

- **4.1 后半**：`ltsc-store-yingyongbao-guide.md` 是否移出 `docs/`。它确实与项目无关，
  但移出仓库还是挪进 `docs/ops/` 是个取舍，索引里已标注"与项目无关"、
  已能满足"不污染检索"的目的，就不擅自搬了。
- **4.4**：`/Users/catlair/...`、`/tmp/caiyun-ref` 这类**证据出处路径**保留原样 ——
  它们记录了"当时在哪读的"，本身是证据的一部分；且文档已给 commit 作权威锚点。
- **本报告自身**：它是一次性产物，考虑后续移入 `docs/archive/` 或直接删除。
