# 实现进度与现状须知

> 状态：**实况**（这里写的是"现在能跑什么、还差什么"，不是计划）
>
> ⚠ **2026-09-20：Rust 侧已冻结到 `legacy/`；2026-09-21 该目录已整体剥离为同级仓库
> [`../mcloud-rust/`](../../mcloud-rust)。**  
> 下面那张表描述的是**冻结那一刻**的 Rust 实现 —— 它仍然有效、也仍然是对拍基准，  
> 但代码与产物都不再位于本仓库；"进度"从此按 Go 侧算。迁移方案与顺序见
> [`go-migration-plan.md`](./go-migration-plan.md)。  
> Go 侧进度见下方「Go 侧迁移实况」一节。  
> ⚠ 剥离带来一条**已终结的约束**（2026-09-22 改口径）：`test/golden/` 与
> `test/fixtures/legacy/` 剥离时两侧各留下一份实体副本（原来 `legacy/test` 是指向本仓库的符号链接），
> 没有门禁盯着两边 —— 这在并行开发期是风险，但 Rust 侧**已冻结不再开发**，
> 所以本仓库改向量的 `expect*` **只**对本仓库生效，不存在"必须两边都落"；
> 对拍口径以本仓库内的黄金向量为唯一真相源（同 `internal/core/version.go` 的同步义务终结口径）。
> **耻辱柱**：原句"改一份向量的 `expect*` 必须两边都落"（2026-09-21 剥离时写）在 Rust 冻结的
> 第二天就已失效，却继续指挥人双写 —— 2026-09-22 用户指出后改写。教训：**剥离冻结当天就该重审所有"双边义务"**。
> 动了哪些文件、为什么：[`../mcloud-rust/docs/freeze-record.md`](../../mcloud-rust/docs/freeze-record.md) 的「剥离补记」。
>
> ✅ **剥离后两侧各跑过一次全量门禁（2026-09-21，剥离当日）**：Go 侧 `bash scripts/gates-go.sh` →
> **九项门禁全绿**（927 个测试函数、`-race` 全量）；Rust 侧在同级仓库 `bash scripts/gates.sh` →
> **七项门禁全绿**、**714 个测试通过**（与冻结记录里那个数字一致，说明搬动没碰行为）。
> ⚠ Rust 侧**第一次**跑报了 30 条 `读夹具 … No such file` —— 那是搬过来的 `target/` 里
> 烘着旧绝对路径的产物（`env!` 是编译期常量），`cargo clean -p` 重编后消失。
> 判据见 [`../mcloud-rust/docs/freeze-record.md`](../../mcloud-rust/docs/freeze-record.md) 的「剥离补记」。
>
> - 最后核对：2026-09-19 收工（Rust 侧）；Go 侧 2026-09-20 起
> - **2026-09-22**：这一轮只动文档（重排 + 去重 + 把 `mcloud-<crate>::…` 那套 Rust 旧写法逐条核到 Go 落点），
>   **本文的进度结论没有变化**。写作约定（Go 落点怎么标、归档怎么读）在
>   [`README.md`](./README.md) 末尾两节；逐项 Go 现状的核对记录在
>   [`business-feature-inventory.md`](./business-feature-inventory.md) 抬头那条 09-22 块里。
> - **2026-09-22（同日第二轮）**：**门禁体系从 bash 迁到 zx** ——
>   `gates-go` / `gates-web` / `gates-scope` / `check-filenames` / `check-go-deps` /
>   `install-hooks` / `shots` 全部改为 `.js`（旧 `.sh` 已删），依赖钉在仓库根
>   `package.json`（zx），克隆后先 `npm install`。剩下的 bash 只有 `.githooks/`
>   两个薄壳。验收口径与逐条差异测试见 [`dev-gates.md`](./dev-gates.md) 抬头「迁移补记」。
> - 规划（**写于动工前**，进度不要以它为准）：[`rust-rewrite-plan.md`](../../mcloud-rust/docs/rust-rewrite-plan.md)
> - 实现顺序（先做哪项能力）：同文档 §7.1
> - 门禁怎么跑：[`dev-gates.md`](./dev-gates.md)

---



> 📦 **过程性实况已移出**：每一轮迁移当时记下的 692 行流水在
> [`archive/migration-log.md`](./archive/migration-log.md)（2026-09-22 逐字搬移，未改写）。
> 本文从此只放**结论**：现在能跑什么、还差什么、按任务先读哪篇。


## 一句话总览

| 层            | 状态                                                                                                                                           |
| ------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| CLI：初始化 / 登录 | ✅ 可用（扫码 + 短信，PC 端与 App 端）                                                                                                                    |
| CLI：签到（云朵中心） | ✅ 端到端跑通（含领取、复活卡）                                                                                                                             |
| 上游接口层        | ⚠ **逐块重写中** —— 只有 `login` 与 `app_signin` 是完整的，其余仍是骨架                                                                                         |
| 服务入口         | ✅ **Go 侧已接线**：API 四组业务路由（含 SSE）+ 前端产物与 SPA 回退；**15 条路由逐条对位冻结基准**（有断言盯着）。⚠ `POST /api/runs` **基准里也没有**（P4 功能，非迁移缺口） |
| WebUI        | ⚠ 登录与初始化是真接口，**主界面仍是 P0 spike 的占位数据**                                                                                                        |
| 运行时层         | ✅ 事件模型 / 生命周期 / 事件总线 / `run_events` 表 + 编排已落地；**2026-09-23 起调度循环就在服务进程内**（`internal/server/scheduler.go`，到点按 `schedules` 触发）。⚠ 只有"任务组合（pipeline）"那一类目标**还没接上执行**，到点会记一句 `error` 而不是静默 |
| 夹具回放（L2）     | 🟡 **回放器已就绪**（`internal/io/fixture.go`，17 条测试）；已有消费者 `TestEveryArchivedFixtureIsLeakFree`（31 份抓包夹具全量自检），⭕ 但上游协议层**还没真的加载这批夹具跑一遍**（见下方「夹具回放」） |

> **追加（2026-09-22 第三轮，只补充不改上面那几行）**：上表「WebUI」那一行是这一轮**之前**的状态。
> 现在**账号这一条线是真的**：粘贴 auth 添加、启用开关、删除、扫码出码 + 轮询、App 端短信发码 + 校验，
> 全部走真接口（详见文末「追加：账号写入面与登录会话接进 WebUI」）。
> 仍是占位的只剩 `placeholder-page.tsx` 那六个页面（任务 / 兑换 / 定时 / 历史 / 调试台 / 设置）。

---

## 按任务指路

| 你要做的事       | 读哪篇                                                                |
| ----------- | ------------------------------------------------------------------ |
| 跑门禁 / 看分层约束 | [`dev-gates.md`](./dev-gates.md)                                   |
| 敲命令行 / 查参数  | [`cli-usage.md`](./cli-usage.md)（编排层的规格在 [`signin-cli-spec.md`](./signin-cli-spec.md)） |
| 加文件 / 起新目录  | [`repo-layout.md`](./repo-layout.md)                               |
| 实现某个业务能力    | [`business-feature-inventory.md`](./business-feature-inventory.md) |
| 写某个上游接口（字段、状态机、错误码） | [`upstream/`](./upstream/) 里对应那个端的那一篇（五篇按端分） |
| 写前端页面 / 组件  | [`webui-design-guide.md`](./webui-design-guide.md) → [`webui-components.md`](./webui-components.md) → [`webui-code-guide.md`](./webui-code-guide.md) |
| 动迁移剩下的步骤   | [`go-migration-plan.md`](./go-migration-plan.md)；**先翻一遍** [`migration-pitfalls.md`](./migration-pitfalls.md)（那张编号表是活的，别重新踩） |
| 看当初为什么这么判 / 历史流水 | [`archive/`](./archive/) —— 归档不删，但**别当现状读** |
| 全部文档的索引与"谁说了算" | [`README.md`](./README.md)                                         |


## 逐项须知

### 服务入口：Go 侧已接线，前端产物与 API 都在

> ⚠ 这一节早先写的是 Rust 侧（冻结那一刻）的样子：`cargo run -p mcloud-server`
> 起 API + 前端产物，但路由只有 `/api/health` / `/api/setup` / `/api/session`。
> 那是**旧**状态 —— 下面按 **Go 侧**重写。

`go run ./cmd/mcloud-server`（默认 `127.0.0.1:8787`）现在能起来，并同时提供：

- **API**：健康 / 初始化 / 会话，加上 accounts、capabilities、runs（含 SSE 事件流）、
  schedules 四组业务路由 —— 白名单免登录（只有 `/api/health`、`/api/setup`
  与 `POST /api/session`），其余一律要会话；
- **前端产物**：`web/dist` 的静态文件 + SPA 回退（非 `/api` 的未知路径交给
  `index.html`，前端路由自己处理）。目录不存在时**只挂 API**，并在启动横幅里说明 ——
  开发期先 `cd web && npm run dev` 是常态。

命令行五个选项与 Rust 侧逐项对齐（`--db` / `--host` / `--port` / `--web-dir` /
`--secure-cookie`），退出码同一套口径：0 正常、1 运行错误、2 用法错误。

⭕ **迁移侧没有缺口**：15 条路由逐条对位冻结基准（`internal/server/routes_test.go` 盯着），
`POST /api/runs` **基准里也没有**（`routes/mod.rs` 写明"要等 P4"）。
⚠ 所以"WebUI 主界面仍是占位数据"的**原因不是缺接口**，而是**跑任务那条路本身还没建**
（P4/P5 的功能开发）—— 别把它记成迁移没做完。`openapi.json` 导出同理，是新功能。

> 2026-09-21 追加：Go 侧在冻结基准之外**自扩 1 条** —— `GET /api/status`
> （`internal/server/routes.go` 的 `getStatus`：产品名+版本、`schedulerRunning`、最近一次 run）。
> 它**不进** `routes_test.go` 那张 15 条对位表 —— 那张表只回答"基准要求有的都在吗"，
> 把自己扩的混进去会让"移植漏了"与"我们改坏了"两种失效分不开；
> 它自己的断言在 `internal/server/status_test.go`（含"未登录必须 401"、"版本必须与
> `/api/version` 同源"、"没 run 时是 null 而不是 `{}`"）。
> 动机是 WebUI 侧栏底部那一行：原先那里把 `name` 与 `role` 并排显示，
> 而初始用户这两个字段字面都是 `admin`（详见 [`webui-design-guide.md`](./webui-design-guide.md) §4.5）。
> ⚠ `schedulerRunning` 自 2026-09-23 起是**运行时实况**（调度循环已在服务进程内，
> 开关位由循环自己起停），早先那句"恒 false"随那条"不装调度循环"的口径一起作废 ——
> 见本文件末尾那一轮。接口宁可显式报 false 也不省略这一项：省略的表现是界面一片绿
> 而绿的意思是"在跑"。反过来，常驻一句"未启用"也会在真跑着的进程上撒谎。

> 更早（2026-09-17）时 `mcloud-server` 只有 lib、没有 `serve` 命令，CLI 能跑而 WebUI  
> 没有后端可连，前端只能靠 `vite dev` 单独起。这一条已经过去了。

> 2026-09-21 追加（**静态那一层加了压缩与缓存头**，同样是基准之外的自扩）：
> 迁移过来的静态是裸 `http.FileServer`，产物**原样**发出去 —— 实测
> `web/dist/assets/index-*.css` 走线 **80,213 字节**，而它 gzip 后 **12,771 字节**
> （-84%）。差的**不是 tree shaking**：产物里没有未用到的组件（逐类名核过，
> 181 个类名都在被引用），是**没人压**。
> ⚠ **别把这一条记成"迁移漏了"**：同一份 CSS 打在冻结基准的 `mcloud-server`（8787）上，
> 实测 `content-length: 80213` 且**没有** `Content-Encoding`/`Cache-Control` ——
> 基准自己也没压。这一层是按"用户实际下多少字节"改的，不是对位。落点在 `internal/server/static.go`，
> 只用标准库 `compress/gzip`（骨架期"零第三方依赖"那条没破）。
> 三件一起做的：**gzip**（按扩展名判，png/woff2 这类内部已压的不碰）、
> **`Cache-Control`**（`/assets/*` 是 Vite 内容哈希名 ⇒ `immutable` 一年；
> 固定名的首页/回退 ⇒ `no-cache`，可存但必须带 `Last-Modified` 回源）、
> **`Vary: Accept-Encoding`**（无条件给：不压也是一次"按头而定"的应答）。
> ⚠ 两处**顺序/分支**上的坑各由一条测试钉住：
> ① 压缩只包**静态分支**，不能包整个 handler —— `GET /api/runs/{id}/events` 是 SSE，
> gzip 的块缓冲会把事件攒着，现象是"进度要等很久才一次性全出来"
> （`TestAPIResponsesCarryNoCompressionHeaders`）；
> ② 缓存与压缩的判据要按**真正送出去的文件**，不是请求路径 ——
> `/assets/` 是个目录、走 SPA 回退送出的是首页，按路径判会给首页打上一年的 `immutable`
> （`TestTheAssetsDirectoryFallbackIsNotTreatedAsAnAsset`）。
> 另外 304 那条：压缩流只在 `WriteHeader(200)` 时建，否则 `Close` 会吐出一段
> 20 字节的**空 gzip 流**，现象是"304 带了一个 body"（`TestAConditionalHitStaysABodyless304`）。
> ⚠ `http.ServeContent` **不造 ETag**（只尊重调用方预设的），所以回源校验走 `Last-Modified`；
> 将来若自己加 ETag，压缩变体必须弱化成 `W/`。判据：`internal/server/static_test.go` 新增 10 条。

### CLI：已能完成"从零到登录"

`mcloud user init` → `mcloud login qr`（终端二维码）/ `mcloud login sms`。

短信登录有两个端：`--client pc`（默认）与 `--client app`（iOS 客户端形态）。  
两端**不是同一件事的两种参数** —— 请求体加解密方式、`Authorization` 前缀、  
设备指纹形状都不同（差异见 [`client-profiles.md`](./client-profiles.md)）。

登录态落在本地库的 `account_state`（凭据与展示信息**分表**）；  
设备指纹写在 `data/device.txt`（**换掉它等于换一台设备**，服务端会定位不到会话）。

### 签到（云朵中心）已端到端跑通

`mcloud signin --account <掩码id>`。这是第一个按新抓包写完整的**业务**接口块 ——  
`internal/upstream/app_signin*.go`（请求构造 + 响应解析）+ `internal/app/signin.go`（编排）。

协议、六个要点与坑见 [`cli-usage.md`](./cli-usage.md) §签到。

### 上游接口层按新抓包逐块重写

目前 `login`（扫码 / 短信 / 账密 + 退出登录）与 `app_signin`（签到）是完整的，  
它们是"按新流量写接口"的头两块；其余云盘业务接口仍是骨架（`Upstream` + `Envelope` + `base`）。

> **2026-09-22 追加：`exchange`（兑换）已抓到真机实拍，但尚未写进代码。**
> 三条接口（`exchangeList` / `exchangeV3` / `getSlide`）的请求与响应**逐字**在
> [`capture/app-exchange-samples.txt`](./capture/app-exchange-samples.txt) §5，
> `puzzleOffset` 的量纲与"服务端容忍度 `[4, 41)` 像素"都已实测。
> ⭐ **2026-09-22 补**：`puzzleOffset` 的**三算法对拍**已做 —— 原项目 `matchSlide`（跑原仓库
> 那份 `match.ts`，非转写）给 `321`，本项目两个探针给 `320`/`320`，**三个独立口径差 1px**；
> 1px 归属未定（补列实验已排除"循环边界"这一嫌疑），但**被 `[4,41)` 的抖动要求完全吞掉**。
> 题面图曾丢过一次，已从 Reqable 的大响应体缓存恢复、并在 §7 第 3 步记明"去哪捞"；
> 临时探针 `cmd/exchange-probe/` 已删（`cmd/` 只剩两个交付物）。
> ⇒ 它与上面那两块**不同**：**信息齐了、代码没写** —— 卡点是规划把它排在最高风险档
> （并发抢兑 + 幂等），**不是信息不足**。⚠ **别把"有实拍"记成"已实现"。**
> ⭐ **2026-09-22 再补（回放阶段）**：用仓库自己的 `app.OpenSignInSession` 建会话
> （**不借**抓包会话）打**售罄品**（零豆），答掉了 `capture/app-exchange-samples.txt` §6 的三条：
> `puzzleOffset` **必填**（空串 / `null` → `{"code":501,"msg":"滑块验证参数不能为空"}`）、
> `501` 是 §1.3 那张表里没有的**新码**、以及 ⭐ **校验顺序 = 参数存在性 → 滑块正确性 → 库存/次数**；
> 另有用户**口述**确认"所有商品都要滑块"（⚠ **口述，非样本**）。
> §6 第 8 条当时**仍无证据**（**滑块闸排在最前** ⇒ 不先解出正确 `puzzleOffset` 就看不到
> 库存/次数那一层的响应；而 `limit != 0` 的那个奖品属于**真机账号** —— 抓包借的 `jwttoken`
> 已过期、账号不在库里）⇒ ✅ **已于同日第四轮答出，见下面「四补」。**
> ⭐⭐ **2026-09-22 三补（解算器落地）**：Go 侧落了**滑块解算器**（临时工具
> `cmd/exchange-replay/`，**未跟踪**；⚠ **第四轮起算法已升格到 `internal/core/slide.go`**，
> 那个探针挪去 `target/attic/` —— 见下「四补」），两个互相独立的口径 —— ①仿射残差 `1-R²` 最小；
> ②**原项目 `matchSlide` 的忠实移植**（在证据图上**逐字复现**原版得分
> `41699/33011/32069/31912/31506` 与答案 `321`；⚠ 关键是照抄它的**整数灰度**
> `(r*77+g*150+b*29)>>8`、`>>2 + min(255,…)` 截断，以及"colormap PNG 按**调色板索引**
> 当灰度"这个**看着像 bug** 的口径 —— 不照抄就对不上任何已记录的数字）。
> **实时提交：`0` 抖的两次都过了滑块闸**（回的是业务码），`+10` 抖那次被拒（`514`）
> ⇒ ⚠⚠ **容忍度修正**：原来记的 `[4, 41)` **不对**（`0` 也通过，"必须离真值至少 4px"
> 是从 n=1 反推的）；实测约 **±4~9px**，**不要加抖** ——
> 原建议"主动加 4~40px 的抖"**会把本来能过的提交弄失败**。
> ⚠⚠ **新增一道闸**：`407 非移动用户不可领奖` —— **账号级**（两件不同奖品回同一个码），
> 排在库存/次数之前。库里那个 `191`（电信）账号**兑不了任何奖** ⇒
> **§6 第 8 条必须先有一个移动号码的账号**才可能答出。
> ✅ **一次豆都没花**（两次提交前后 `infoV3.total` 始终 `7434`）。
> ⭐⭐ **2026-09-22 四补（§6 第 8 条答完，仍是零豆）**：
> - **"处理 1" 已落地**：解算算法**升格**进 `internal/core/slide.go`（纯计算、无 IO），
>   测试 5 条在 `internal/core/slide_test.go` —— 其中一条是**真机证据图的对拍锁**
>   （`321` / 得分前五 `41699/33011/32069/31912/31506` / 轮廓点 `310`）。
>   ⚠ **反向测试验证过它真的会红**：去掉"调色板索引当灰度" ⇒ 得分变 `27400 / 20988 / …`、
>   答案变 `319`；去掉 `>>2 + min(255,…)` ⇒ 得分涨到 `173160 / …`。
>   ⇒ 那两条"看着像 bug"的上游口径**从此有测试钉着**，不再是"只有文档记着"。
>   探针本体挪出 `cmd/` 到 `target/attic/exchange-replay/`（`target/` 已 gitignore）
>   ⇒ 不再进任何 Go 门禁的包集合（`GO_PKGS` 是显式的），`cmd/` 回到只剩两个交付物。
> - **"用上那份凭据"已落地**：真机那个账号的**会话**确实不可复现，但它的**凭据**在
>   `data/legacy-source/asign.json`（旧项目迁移备份，已 gitignore）里 ⇒ 落进**副本库**建会话，
>   `infoV3.total` = **`8705`**，与 `§5.4` 表里"真机 App（兑换后）"**逐字对上**。
>   ⚠ 反面教训：先在 Reqable 的 `box/data.mdb`（2.1 GB）里翻凭据，`exchangeV3` 命中 **0**、
>   唯一一条 `Basic` 凭据早过期（实测 `暂无权限！`）⇒ **"抓包缓存里没有"≠"手上没有凭据"**。
> - **三发提交、三次零豆**（前后 `total` 恒 `8705`），答出两个**新码**：
>   **`415 奖品单日已耗尽`**（库存/次数那一层）、**`2301 重复兑奖`**（个人兑奖状态那一层
>   ⇒ ⭐ **服务端有幂等语义**）。
>   ⚠⚠ **`2301` 是四位**（其余码全是三位）⇒ 别按"码都是三位"的形状写解析。
>   校验顺序补成**五道闸**：`501 → 514 → 407 → 415 → 2301`。
>   ⚠ **`420` / `421`（用户级配额）仍无样本** —— 它**没有零豆替代**。
> - ⇒ 兑换的**信息已齐**（§6 九条只剩第 3 条 `smsCode` 明确搁置），卡点回到
>   "**排期与风险档**"，不再是"信息不足"。⚠ **别把"信息齐了"记成"已实现"。**
> - 详见 `capture/app-exchange-samples.txt` §5.5。
> **门禁（第三轮那次）**：当时 `cmd/exchange-replay/` 还在 `cmd/` 下，跑
> `node scripts/gates-go.js` ⇒ **九项门禁全绿**（断言 167 条、golden、jsontags 75+77 全过；
> 墙钟 1 m 04 s ~ 1 m 50 s，随负载浮动）。⚠ 第一次跑是 **8/9**，唯一红项是那个新文件没 `gofmt`
> ⇒ 顺带证明**加了 `cmd/` 之后 `deps` / `filenames` / `idents` 都不红**。
> ⚠ 那一轮后续又改了三份文档 ⇒ **改完在最终树上重跑过，仍九项全绿**。
> 详见 `capture/app-exchange-samples.txt` §5.3.1。
> ⭐ **第四轮的门禁**：探针挪出 `cmd/` 之后（`cmd/` 只剩两个交付物）重跑 ⇒ **九项门禁全绿**
> （断言 167、golden 11/11 有消费者、jsontags 75+81）。⚠ 首次也是 **8/9**，唯一红项是
> `internal/core/slide.go` 的四个新结构体没登记 `check-go-jsontags.cjs` 的 `UNTAGGED_ALLOWED`
> —— 它们**不进 JSON**（对外只有 `puzzleOffset` 一个整数进请求体，且那个 body 是**按实拍键序手工拼**的），
> 已补理由登记。
> 与参考项目（caiyun）的逐条对照见
> [`caiyun-go-exchange-and-runtime.md`](./caiyun-go-exchange-and-runtime.md) §3.9。
> ⭐⭐ **2026-09-22 五补（零豆只读：把 `limit` 取值的字段依据钉死）**：
> - **一次提交都没发**：只调 `exchangeList`（探针的 `list` 模式），账号仍是那个移动号码，
>   库走**副本** `target/replay.db`（不碰 `data/mcloud.db`）⇒ 零豆、无副作用。
> - ⭐ **`limit` 取值有字段依据了**：`probability2` 里的 `monthGroupLimit` 是个**数组** ——
>   四件"音乐会员月卡"（`251230051/052/053/057`）共享一个"每月只能兑其中一件"的组，
>   配合 `limitMonthQuantity: 1` ⇒ 我兑了 `251230053` ⇒ 它自己 `limit 2`（**这件**已兑）、
>   同组三件 `limit 4`（**本组**已兑）。⚠ 不是"我兑过这一类"那种模糊说法。
> - ⭐ **修正四补里那句"`420`/`421` 没有零豆替代"**：`421`（**每月**限兑）**不必**同件兑两次 ——
>   `limit == 4` 本身就是"本月组内名额用掉了" ⇒ 探针候选 **`251230052`**（`limit 4` + 日余 49）；
>   ⚠ 但它**只被一道闸挡 ⇒ 不是零豆**（可能真扣 1800 豆）⇒ **要拍板**，本轮**没发**。
>   `420`（每日限兑）那半仍无零豆替代。
> - ⭐ `infoV3` 复核：`total` 仍是 **`8705`**（隔了时间也没"延迟扣豆"）；
>   同响应里 `beforeTotal = 8027` ≠ `8705` ⇒ 再证 §1.5。
> - ⚠⚠ 探针自己犯了个**静默错**（已修）：`probability2` 里 `limitMonthQuantity` 是**数字**、
>   `monthGroupLimit` 是**数组** ⇒ 按"都是数字"读会**静默归零**（坑表 **#105**）。
> - 详见 `capture/app-exchange-samples.txt` **§5.6**（含逐字响应与挑探针的取舍表）。
> **门禁（第五轮）**：`node scripts/gates-go.js` ⇒ **九项全绿**（断言 167、golden 11/11 有消费者、
> jsontags 75+81、脱敏 11 个文件）。⚠ 跑了**两次**：**1m59s**（改文档前）、**1m17s**（改完文档后的
> **最终树**，缓存热）⇒ ⭐ **以第二次为准** —— "绿"只对**跑的那个版本**成立。
> ⚠ **范围要说清**：跑的是 `GO_PKGS`（`./cmd/... ./internal/...`），而**工作区里同时有另一路
> 在飞的改动**（登录 / 二维码 / 账号写入面）⇒ 这次绿说明的是"**工作区**绿"，
> 不是"只有我这块绿"。本轮我**没动 `internal/` 与 `cmd/`** —— 改的是 `target/attic/` 下的探针
> （不在 `GO_PKGS` 里，故不参与 `build` / `test`）与几份文档。

旧 TS 的 12 个接口模块**一律不从旧代码搬迁** —— 旧 HAR 与旧 TS 都会作废，  
搬了只是把旧形态再抄一遍。新 HAR 抓好后：`mcloud fixture extract` → 按新流量写接口  
→ 用夹具离线回放并断言请求逐字段正确。

### ⚠ 夹具回放（L2）仍只有一个消费者

此前本文件写着"`mcloud-upstream` 的 L2 夹具回放已跑通  
（`../mcloud-rust/crates/mcloud-upstream/tests/l2_replay.rs`）"—— **那个文件不存在**，  
`../mcloud-rust/crates/mcloud-upstream/tests/` 是个**空目录**。

现状是：`mcloud fixture extract` 往 `test/fixtures/http/` 写，消费者目前**只有一条**  
—— `io` 包的 `TestEveryArchivedFixtureIsLeakFree`（把 31 份夹具全量加载跑脱敏自检）。  
上游协议层**还没有**真的加载这批夹具把响应解析一遍，所以"回放器已跑通"这句话  
仍然**不能**按"L2 回放已接上"来理解。这是规划 §6.3 里要补的一环。

已确认可用的样本（写回放时直接照着接）：

| 接口                                      | 在哪个夹具                  |
| --------------------------------------- | ---------------------- |
| `startSignIn` / `infoV3` / `doTaskPost` | `3d.json`              |
| `taskListV3`                            | `每月阅读任务.json`          |
| `receiveRevivalReward`                  | `复活.json`              |
| `receiveV3`                             | **没有任何夹具**（只能靠单测 + 真机） |

### 旧 TS 与线上实际请求本就不一致，重写时要逐条核对

上一轮对拍换来的经验，**别默认"照 TS 抄"就是对的**：

| 接口                            | TS 写法                                | 真实流量                                      |
| ----------------------------- | ------------------------------------ | ----------------------------------------- |
| `visitlog/journaling`         | 参数放在 **query**                       | 参数放在**表单体**                               |
| `signin/task/taskListV2`      | `clientVersion=11.2.1`、`group=month` | `clientVersion=11.4.0`、`group=new`        |
| `auth-service/auth/tyrzLogin` | body 带 `type`/`channel`              | body 无这两个字段，`marketName=newsign_139mail`  |
| `market/signin/page/infoV3`   | 不带端口的 `/market/...`                  | `caiyun.feixin.10086.cn:7071/ycloud/...`  |
| `noteRefreshToken`            | `note.mcloud.139.com/noteServer/...` | `note-njs.yun.139.com/yun-note/...`（另一个域） |

另有：`refreshToken` 的真实流量是**加密载荷**（原实现注释却说是明文 JSON 接口）；  
`auth/refreshToken`、`querySpecTokenV2` 这类接口的 `code` 是**字符串**不是数字。

> ✅ 上面"加密载荷"这条 **2026-09-20 已定案**（此前只有结论、没有钥匙）：  
> 带 `hcy-cool-flag` 的请求与响应都是  
> `base64(IV(16 字节二进制) ‖ AES-192-CBC/PKCS7("2olBaQGYnEKoStYomsd1n7ax", IV, 明文))`，  
> 双向；**396 条实拍零例外**。钥匙就藏在旧项目  
> `packages/core/src/utils/encrypt.ts:3` 的 `REFRESH_TOKEN_KEY`，*而它导出的解码函数    
> **全仓无人调用** —— 见 [`upstream/app-business-api.md`](./upstream/app-business-api.md) §8.0。
> ⚠ 它与发码链路的 `uDjGSA8auFlbwDkY`（AES-128）**不是同一把**，别互搬。
>
> ✅ **同日再更正：这类信封其实有不止一把钥匙，按「谁发的」分** ——  
> 上面那把 AES-192 是 **App 原生**（`FXHttpBaseAccessor`）用的；  
> **H5 / WebView**（`coolFlagPivotal`）用另一把 **AES-256**  
> `h7DLzCTAjn+altjy2a9E9FSt7AC6pmx1`，格式完全同形。  
> 判据：`x-yun-uni` + `x-yun-api-version` ⇒ 原生；`origin: mcloudlocal://` + `sec-fetch-*` ⇒ WebView。  
> ⚠ **钥匙是「谁发的」的函数，不是版本号** —— 别按"最新版就该用最新那把"去猜。  
> 见 `docs/upstream/app-business-api.md` §8.1。

---

## 历史流水账（已折叠，只留结论）

测试数一路涨上来的过程本身不构成"当前状态"，故压成下表。当前数见下方「门禁实况」。

| 时间         | 测试数     | 当轮新增了什么                                                             |
| ---------- | ------- | ------------------------------------------------------------------- |
| 2026-09-17 | 241     | 当时的一帧                                                               |
| ——         | 493     | 接完 smid（设备指纹绑账号）那条线，自贡献 46 条（上游算法 25 + 生命周期 13 + 存储 8）              |
| ——         | 516     | App 签到线：上游接口层 + 编排层 + 存储层（`account_state.phone`，schema v6）          |
| ——         | 523     | 云朵余额采集（`mcloud-app::cloud` 5 条 + `set_account_cloud` 窄写 2 条）        |
| ——         | 533     | `receiveV3` 领取（请求构造 / 解析 / 错误码 10 条）                                |
| ——         | 541     | `revival` 复活卡（8 条，含一条读黄金向量的）                                        |
| ——         | 542     | 查明 H5 `deviceId` 根因并接线（新增一条把签到页 `_smConf` 与 Rust 常量钉在一起的守卫）         |
| ——         | 546     | `Authorization` 改为整串存整串用（schema v7），新增 4 条                          |
| ——         | 554     | CLI 账号选择（下标语义）补 9 条用例                                               |
| 2026-09-19 | **586** | 把云端那 9 个提交拉进本地（`mcloud-app` 组装层、App 签到全链路、schema v7 等），并修掉两处 clippy |

> 这里曾经把 554 写成"当前数"—— 那是**拉取云端之前**本机的数。  
> 云端那 9 个提交带进来的 `mcloud-app` 等新模块另有自己的 lib 测试，落地后实跑是 **586**。

**门禁实况（2026-09-21，Go 侧）**：**922 个测试通过**，**九条门禁全绿**  
（黄金向量那段是 **167 条断言**，见 [`dev-gates.md`](./dev-gates.md)）。  
⚠ 上面「当前数见下方门禁实况」指的就是这一段 —— 它早先写的是 **Rust 侧**
（2026-09-19：586 个 / 七条），那是**冻结前**的数，别拿它当现状。
Go 侧逐包：core 270 / upstream 157 / io 150 / app 118 / cli 116 / server 44 /
exec 35 / capabilities 32。

**门禁实况（2026-09-22，Go 侧全量 + web 侧首跑）**：**1052 个测试通过**，
Go **九条门禁全绿**（golden 段 **167 条断言**），web **两条全绿**（`gates-web.sh` 两条合计
墙钟 2.4 s：`typecheck` + `vite build`）。逐包：core 289 / upstream 208 / io 156 / cli 135 / app 131 /
**server 62**（昨天 44 —— 静态层的压缩与缓存头补了 10 条）/ capabilities 36 / exec 35。  
⚠ 这个数是**工作区**实跑，而工作区里有并行会话在飞的改动 ⇒ 它是"这一刻的树绿不绿"，
不是"某一个提交带进来的数"。  
**同日起 `pre-commit` 改成按暂存范围派发**（此前写死五条）：恒跑 `filenames` `idents`，
触及 `*.go` 加 `fmt deps jsontags`，触及 `web/**` 加 `typecheck`；
`pre-push` **不看范围**，跑 Go 九条 + web 两条。分类器与安全前提见
[`dev-gates.md`](./dev-gates.md)「在哪里跑」与 [`.githooks/README.md`](../.githooks/README.md)。
⇒ **提交档不含编译与测试**，"提交前必须全绿"仍然要求手动跑上面那两个全量。

---

## 追加：账号写入面与登录会话接进 WebUI（2026-09-22 第三轮）

上面那张「门禁实况」是本轮**开始前**的数。这一轮把"账号从哪来"这件事从命令行搬进了界面，
范围由用户点定：**全做**（粘贴 auth 添加 + 扫码/短信登录 + 删除 + 启停，二维码要能真扫）。

**后端新增 8 条路由**（`internal/server/auth.go` 注册；它们**不在**冻结基准那 15 条里，
所以在 `routes_test.go` 里单列一张 `goAdded` 表，与对位断言互不干扰）：

| 面 | 路由 |
| --- | --- |
| 写账号 | `POST /api/accounts`（粘贴 auth 整串）、`PUT /api/accounts/{id}/enabled`、`DELETE /api/accounts/{id}` |
| 扫码登录 | `POST /api/login/qr`、`GET /api/login/qr/{id}`（轮询）、`GET /api/login/qr/{id}/image.svg` |
| 短信登录（App 端） | `POST /api/login/sms/app`（发码）、`POST /api/login/sms/app/{id}/verify`（校验） |
| 短信登录（PC 端） | `POST /api/login/sms/pc`（发码，带 `platform`）、`POST /api/login/sms/pc/{id}/verify`（校验） |

> **补记（2026-09-22 第四轮）**：上面那句"8 条"是第三轮当时的数，加完 PC 端短信这一对
> 之后是 **10 条**。PC 那条不是新协议 —— `app.SmsSendCode` / `app.SmsLogin` 早就存在，
> CLI 的 `mcloud login sms --client PC` 一直在用，缺的只是 HTTP 面与界面这一层。
> 顺带补了两处：`loginKindPcSMS` 要一起进 `sweepLocked` 那条短信 TTL（否则这种会话
> 永远清不掉、把 `maxLiveSessionsPerUser` 吃满），以及 `VerifyAppCode` 缺一个
> "会话不是这一类"的守卫 —— 拿 PC 或扫码的会话 id 走 App 填码口会**空指针 panic**
> （`entry.profile` 为 nil），这条在第三轮就埋着，是新增的跨端用例撞出来的。
> 现在两端都给 404，口径与"看不见即不存在"一致。

分层的落点（自下往上）：

- `internal/io`：`SetAccountEnabled` 窄写 + `DeleteAccount` 事务删（一次删掉账号及其流水）。
- `internal/capabilities/auth_ingest.go`：把一整串 auth 拆成"账号 + 登录态"。
  ⚠ 四条"这串用不了"的拒绝从 `KindParse` 改成了 **`KindBadRequest`** ——
  `Parse` 的语义是"**上游**响应不合形状"，落 500 且只回类别文案；
  而粘贴进来的串不合格是**用户可改**的，落到 400 才递得出人话。
- `internal/app/qr.go`：`QRText` / `QRSVG` —— 终端的 ASCII 码与界面的 SVG **同一个矩阵**，
  CLI 与 WebUI 共用一份渲染，不再各写一遍。SVG 是 1 个背景 `rect` + 1 条 `path`（约 20 KB）。
- `internal/app/login_hub.go`：会话表。对外的 `LoginSessionView` **只有四个阶段**
  （`waiting` / `scanned` / `succeeded` / `failed`），上游那些 `200059541` 之类的码留在能力层，
  失败时以人话文本带出；⚠ 结构体**没有** JSON tag 也不给加（凭据边界，见下）。
- 凭据边界：`auth` 整串**只进不出** —— 请求体读进来、落库，响应里只有掩码账号 id（`19****00`）；
  测试里那条守卫断言错误文本**不回显**贴进去的串。

**前端**：`web/src/ui/dialog.tsx`（原生 `<dialog>` + daisyUI `modal`，`openDialog(id)` / `closeDialog(id)`
靠唯一 id 找元素而不是模板 ref —— Vapor 下这是最省事的一条路）、`web/src/pages/login-dialog.tsx`
（扫码 / 短信两个 tab，`POLL_MS=1500`，终态或关闭都停轮询）、`accounts-page.tsx` 重写
（三个弹窗 + 启停 + 删除确认，窄写走 `busyId` 防连点）。
⚠ 本轮踩到一条新的静默失效（Vapor 的子节点是**插槽**不是 `props.children`），
判据与修法记在 [`webui-code-guide.md`](./webui-code-guide.md) §9.2 的补记里。

**实机核验**（Go 服务跑在 8793，临时库 `/tmp/mcloud-live-8793.db`，避并行会话的 8787/8788/8791/8792；
只投合成 `199` 号段）：粘贴 auth → 列表出现 `19****00`；启停开关点两下（停用 → 启用），
行的状态文案与徽标跟着翻转，**硬刷新一次**（换 query 重进）仍是「已停用」，再点回来也回得来；
删除确认弹窗正文渲染出影响面并真的少一行；贴一串不成形的文本 → 界面递出人话
「auth 解析不出来：不是 base64，或解出来不是文本」（400 透传，且**不回显**那串本身 —— 上面那条
`KindBadRequest` 改动就是为了这一步不落成 500 的类别文案）；出码后 `<img>` 加载该 SVG（`naturalWidth=520`），
状态文案停在"等待扫码"，**关窗后 5 s 内新增 0 次轮询**；控制台零错误。
⚠ 短信那条只验到界面渲染（手机号输入框 + 发码按钮），**没有**真发验证码 —— 那会打到真实号码上。

**移动端 375×667 的量法（2026-09-22，§7 那条硬要求）**：Qoder 内置浏览器**不能改窗口尺寸**，
而 `window.open` 被拦、`browser-use` 的点击在这条路上会因
`NATIVE_BROWSER_VIEWPORT_UNAVAILABLE` 失败 —— 可行的办法是**同源 `iframe`**
（媒体查询按 iframe 自己的视口算，cookie 同源共享，于是能真跑一遍流程）：
量到 `scrollWidth=375`（无横向溢出）、`.modal-box` 宽 **343px**（= 375 − 2rem，§5.8 那条窄屏上限生效）、
输入框与按钮各 **40px**、关闭提示的 `×` 从 `btn-xs` 改成 **`md:btn-xs`** 之后在 375 与 493 两档都是 40px
（产物里 `.md\:btn-xs` 确实被 `@media` 包住，`matchMedia('(min-width:48rem)')` 为假时不生效）。
⚠ **一条规范与现实的出入，留给用户定**：§6.1 写的是"触控目标 ≥44px"，
而 daisyUI 的默认 `btn` / `input` 高 **40px** —— 全站既有按钮都是 40px（刷新、详情、取消、确认添加…），
本轮只在启停那一行用 `min-h-11` 撑到 44px。要么把判据改成"≥40px，关键行用 `min-h-11`"，
要么全局给 `.btn` / `.input` 加 `min-h:2.75rem`，两者不该由一次功能改动顺手定。

**门禁实况（2026-09-22 同日第三轮，Go 全量 + web 全量，手动跑）**：
`node scripts/gates-go.js` → **九条全绿**（墙钟 54.2 s，`test` 48.3 s 带 `-race`）；
`node scripts/gates-web.js` → **两条全绿**（typecheck 1.8 s + build 1.0 s）。
**1091 条 PASS、0 失败**（含子测试；顶层测试函数 976 个）。逐包（含子测试）：
core 289 / upstream 208 / io 163 / app 138 / cli 135 / **server 80**（上轮 62）/ **capabilities 43**（上轮 36）/ exec 35。
⇒ 比上一轮那条 1052 多 **39** 条，全部来自这一轮：server +18、app +7、capabilities +7、io +7。
本轮修掉两条自造的门禁红：`auth_ingest.go` 未 `gofmt`、`LoginSessionView` 进 `jsontags` 白名单
（理由写在 `scripts/check-go-jsontags.cjs` 里，与 `loginSessionPayload` 那条注释同源）。

---
### 补记（2026-09-22 第四轮，只动 web 两个文件）

**登录弹窗 · 短信那栏的端别选择合并成一个三选**：原来是"先选 PC/App，选 PC 再选 Win/Mac"两级，
现在一列 **手机 App / macOS 客户端 / Windows 客户端**（`aria-label="登录端"`）。
理由：那两级其实是同一个决定，拆两级会让"PC"看起来像一个有端档案的档，而它没有。
**界面三端 → 后端仍两条路由**：App 走 `/api/login/sms/app`（体里只有 `phone`），
mac/win 走 `/api/login/sms/pc`（体里 `phone` + `platform: macos|windows`）。
提交验证码那一步回到**发码时实际走的那条**（`smsSentVia`），不读选择器 ——
否则"发完码改另一端别"会拿 App 会话去撞 PC 口子，后端给 404，表现成"验证码收到却提交不了"。
扫码那栏的 `platformSelect` 保持不动（只有它用）。

**启用 / 停用终于不同色**：`enabledToggle` 那句此前两个状态共用**同一个字面量**
`text-xs opacity-70`，所以"停用"看着和"启用"一模一样（用户指出的就是这个）。
现在查表上色：`badge-soft badge-success badge-sm` / `badge-ghost badge-sm`。
用 `badge-*` 而不是 `text-success` —— §2.4 硬规则一把那两位留给涨跌。
顺手把 `healthOf` 上面那句注释修正成与代码一致（原文写"红=不可用（过期/停用）"，
而停用其实走中性点：主动关闭不是故障）。

**修一条自造即在眼前发现的 bug**：`reset()` 只 `stopCooldown()` 没清 `cooldown.value`，
interval 已停 ⇒ 倒计时**冻在剩余秒数**，重开弹窗后"发送验证码"永远灰着，整栏作废到刷新页面。
现补 `cooldown.value = 0`。

**核验口径与两条工具限制**（Go 侧未动，故本轮只跑 `node scripts/gates-web.js` → **两条全绿**：
typecheck 3.9 s + build 1.7 s；跑在临时库 + 8790，验完即拆，没碰 8788 那个在用的库）：
- 三端路由用**打桩 fetch** 断言（记 URL 与请求体）：app→`/sms/app` 无 `platform`、
  mac→`/sms/pc` `platform=macos`、win→`/sms/pc` `platform=windows`；**全程没真发验证码**。
- 真实行的渲染色用 `getComputedStyle` 读：启用 `lab(74.5 -60.8 19.4)` 绿字 / 停用中性底 —— 差别可见。
  账号列表同样是**打桩 `/api/accounts`**（临时库里没有账号，也不去造真凭据）。
- ⚠ **这个内置浏览器不给 `dialog.close()` 派发 `close` 事件**（连手写的裸 `<dialog>` 也不派发，
  与被测代码无关）。所以"关窗即停轮询/清空"这一类只能靠**手动 `dispatchEvent(new Event('close'))`** 验，
  本轮就是这么验到冷却冻结那条的。它在真实 Chrome/Edge 里正常，**不要**把这写成应用缺陷。
- ⚠ 截图仍走不通（`NATIVE_BROWSER_VIEWPORT_UNAVAILABLE`，可见性为 hidden），
  视觉结论只能来自计算值，不是像素。

## 抓包：直播 / 小红花（`kouling` + `red-flower`，2026-09-22）

实拍文档 [`docs/capture/app-live-redflower-samples.txt`](./capture/app-live-redflower-samples.txt)
§5。设备 iPad（`com.apple.WebKit.Networking`）+ Reqable `*:9000`，账号 `159****7542`。

**小红花那六个接口全部抓到**（`m.mcloud.139.com/ycloud/liveRoomFeedback/…`）：

- `user/info` ⭐ 余额字段 = **`flowerCnt: 235`**（另 `redFlowerValidDate: "2026年12月31日"`、
  `isQuarterDrawed`、`prizeId`/`secondPrizeId`、`hasNotReceivedRedFlower`）；
  两次调用**都是 235** ⇒ 两发 `draw` 失败，余额**一分未动**。
- `firstParticipate/give` → **`200110`**（已参与过，无首次资格）—— 旧项目的判据**实拍确认**。
- `redFlower/draw?code=<urlencoded>` → ⭐ **`200108`**（`口令不对耶…`）= **无效口令**，
  **旧项目没记过这个码**（它只笼统写"其他数字 ⇒ 无效"）。
  ⚠ **口令在 query string，body 空**（`content-length: 0`）。
- `live/info` → `result: []`（空数组，因为没开播）。
- `visitlog/journaling` → ⭐ **form-urlencoded**（不是 JSON），
  `module=uservisit&optkeyword=National_LiveRoomFeedback_flowerBtn&sourceid=1007&marketName=National_LiveRoomFeedback`
  ⇒ 9-17 那批夹具里"一条都没有"是**当时没抓全**，不是走了别的域。
  ⚠ 同一接口也服务别的活动 ⇒ **`marketName` 才是区分活动的键**。
- `getRedFlowerTop10List` → 排行榜（别人家的 `flowerCnt`，第一名 2055）—— 别当自己的余额。

**直播只走到房间心跳**（`v.cmcc-cs.cn/cmlive`，纯 H5 页面 `room?id=147606&cmliveChannelId=134&targetSourceId=017027`）：

- `POST /cmlive/api/room/front/roomBeat` body `{"roomId":"147606","channelId":"134","sourceApp":0}`
  → `rtnCode: "0"`（**字符串**）、`object.status: 0`（**未开播**）、
  **`expectStartTime: "2026-09-23 14:30:00"`**、**`chatRoomId: null`**、
  `anchor.nickname: "中国移动云盘"` ✅（门的精确相等判据今天仍成立）。
- ⭐ 直播侧凭据是**独立的一套 JWT**（`Authorization` 头，payload 含 `phone` / `userId: 47776930`，
  有效期约 30 天）—— **不是**签到/兑换那串 `pc:` / `mobile:` 凭据。
- ⏸ **WS / 弹幕 / 口令结构性抓不到**：`chatRoomId` 为 null ⇒ JS SDK 根本不会建 WS。
  ⚠ **这是"结构性不存在"，不是抓漏了**（坑表 #107）。

⇒ **下一次窗口：2026-09-23 14:30 之后**（`expectStartTime`），到时补
WS 签名头（`systemmd5` / `tokenmd5`）、`member/apply` 的 `version`、以及一条真口令。
⚠ **`draw` 的 `200112` 不在这一趟的清单里了**（2026-09-23 改）：它要"同一账号把同一条
**正确**口令投第二次"才出得来，而探针**只收口令、不投口令**（见下面那节五条事实）。

⚠ 本轮**没动 `internal/` / `cmd/`**，故未跑 Go 门禁。坑表加 **#106 / #107**（共 107 条）。

### 补：「门」的实证（同一天稍后，只读探针）

⭐ **抓包没抓到 ≠ 那个请求不能打**。`tabContentList` 是 GET，不需要开播 ⇒
用临时探针 `target/attic/live-probe`（全程只读）主动走了一遍
`querySpecTokenV2(toSourceId=017027)` → `cmpassLoginByCmToken` → `tabList` → `tabContentList`。

- ⭐ **库里那份凭据在直播这一族上也认**（`017027` 换得到 SSO token）；
- `cmpassLoginByCmToken` 的 `object` **就是**那串 JWT（180 字符），直接当 `Authorization`；
- 全程 `rtnCode` 都是**字符串 `"0"`** ✅ 再证两套响应约定；
- 两个 tab：`127 盘盘福利`、`129 精彩回放`，**各 8 间房**；
- ⭐⭐ **`status` 三档实拍确认**：`1` 直播中 / `6` 已结束 / **`0` 未开播**；
- ⭐⭐ **今天 15:2x「中国移动云盘」没有一间 `status===1`** ⇒ 门判定 **false**
  （147606 是 `status=0`，`expectStartTime 2026-09-23 14:30`）；
- ⚠⭐ **只看 `status===1` 会误判**（127 里就有「福建移动」在播）⇒ 主播名那个条件才是真干活的；
  ⚠ **`channelId` 不能当过滤条件**（返回房间有 `29/51/243/134` 四种）。

坑表加 **#108**（`Envelope.Payload()` 只认 `/ycloud` 的 `data`/`result`；`json.Number` 静默归零）。

## 直播口令 → 抽小红花：**已实现**（2026-09-22，六步链 + 双源收口令 + WS 帧状态机）

⚠ **这一层没有实拍可依**：那天 `chatRoomId: null` ⇒ WS 结构性地不存在（#107），
所以 WS / 弹幕 / 口令那一块是**照原项目 + HAR 写的**（用户明确要求的口径）。
下游能证的部分全部有实拍：六步链的响应体逐字取自 §5.1，门的四步由只读探针实证过。

### ⭐ 口令（`kouling`）的五条事实 —— 2026-09-23 用户纠正后定稿

> ⚠⚠ 早先代码注释、探针脚本、`cli-usage.md` 里有一句"**投第二次会回 `200112`（口令被消耗）**"，
> 那是把"口令一次性 / 会被消耗"这个**未验证的前提**当成了结论，**已逐处纠正**。
> 下面五条是纠正后的口径；改这一族之前先读一遍。

1. **只在本次直播有效**：直播时间外不可用，**上一场的口令也不能用**。
   上游原话就是 `200108` 那句「口令需当场直播获取才有效哦」。
2. **全场通用**：所有用户共用同一批口令 ⇒ **不存在"消耗口令"这回事**，
   也不存在"我把口令用掉了别人就没得用"。
3. **每场两个固定口令**（小概率会多一个随机口令）⇒ 收集那一步拿到 **2~3 条**是正常形状；
   别把"只收到 1 条"当成已收全，也别把"收到 3 条"当成有脏数据。
4. **进直播间就会有人发口令**：任意用户（**包括自己和别人**）进入移动云盘直播间后，
   系统会**以该用户的名义**发一条口令 ⇒ 听弹幕收到的口令**署名是谁都能用**，
   别按"发信人是不是我"去过滤。
5. **另一条口令在别的平台（小红书）**：那边没法鉴权、**不做自动化** ——
   由人去小红书找别人分享出来的那条，再手工喂进来（CLI 的 `--codes`）。
   ⇒ 这就是为什么"听弹幕"永远拿不全两条：**`--codes` 不是可选项，是另一条的必需入口**。

#### `200112` 的语义（与上面第 2 条**不矛盾**，别混为一谈）

- ⭐ `200112` = **同一账号**第二次请求同一条**正确**的口令 ⇒ 该账号已用这条口令换过花。
  ⇒ 它说的是**按账号去重**，**不是**"口令被消耗"。两个不同账号投同一条口令**各自都会成功**。
- 它**说明口令本身是对的**（否则会回 `200108`）⇒ **不该**算无效口令
  （不进 `InvalidCodes`、不触发重听、不从共享缓存剔除）。
- 接口归属 = **`redFlower/draw?code=`**（"请求口令"那一步）。
- ⚠ **证据强度**：用户口径（2026-09-23）+ 原项目 `tasks/red-flower/standalone.ts:72` 字面量，
  **仍零实拍**。哪天拍到样本，把这里从"用户口径"改成"实拍"。

**这一轮（2026-09-23）纠正的落点** —— "消耗口令"这个错前提曾一路写进注释 / 探针 / 文档 / CLI / UI：

- 代码：`internal/upstream/live_flower.go`（文件头五条事实 + 码表归属）、
  `internal/app/live_room.go` / `live_room_task.go` / `live_room_probe.go`、
  `internal/cli/args_live.go`、`internal/capabilities/registry.go`、`internal/server/live_room_probe.go`；
- 探针：`scripts/probe-live-room.js` 删掉"投两次抓 `200112`"那条核对项与 `--code` 开关（清单 **6 → 5 条**）；
- 文档：本节、`docs/cli-usage.md`（新增「口令的五条事实」）、
  `docs/capture/app-live-redflower-samples.txt`、`docs/capture/probe-live-room-2026-09-22.md`（加更正补记）、
  `docs/business-feature-inventory.md`；
- 界面：`web/src/pages/live-redflower-page.tsx`（"不消耗"→"一个口令都不投"、"收不回来"→"写操作"）。
- 坑表加 **#132**（与 **#131** 是同一格的两个反向错，见坑表末尾那节）。

**分层与落点**（`docs/README.md` 有索引；这一族的坑见坑表 #109–#120）：

| 层 | 文件 | 内容 |
| --- | --- | --- |
| 纯层 · 协议 | `internal/upstream/live_msync.go` | 手写 MSync 编解码（原项目用 protobufjs，**没有** `.proto`、不生成代码）。⚠ `Pov` 是 `*uint64` —— proto3"默认值不写"有个**指针例外**（#109） |
| 纯层 · 帧 | `internal/upstream/live_ws.go` | SockJS 四种帧 + `KoulingListener` 状态机（PROVISION / 心跳 / **ack 之后补 UNREAD** / 跨帧拼接 / 全局去重） |
| 纯层 · 解析 | `internal/upstream/live_kouling.go`、`live_flower.go`、`live_room.go` | 口令四分支提取（Go RE2 **没有前瞻**，改成显式扫描器）、小红书四级候选、六步链的响应分类、门的判定 |
| 连接 | `internal/io/websocket.go`、`internal/core/websocket.go` | 手写 RFC 6455 文本帧客户端（零第三方依赖）。**掩码 / accept 校验 / 服务端掩码帧拒收**三处逐字节断言 |
| 编排 | `internal/app/live_room.go`（纯函数）、`live_room_drive.go`（驱动） | 六步状态机（`Next` **不推进**）、`RetryPlan` / `PickNewCodes` / `JudgeKoulingGate`、进房链 + 双源并发收口令 |
| 入口 | `internal/cli/args_live.go`、`run_live_room.go`；能力 `live-room` | `mcloud live-room`（`--kouling-only` 只读 / `--skip-kouling --codes` 只投 / `--json`） |

**几个刻意的取舍**（都写进了代码注释，这里只列结论）：

- ⚠ **不照抄原项目那条"门失败即放行"** —— 它会让"凭据坏了"表现成"门开着、却一条口令也听不到"，
  真正的原因被盖住。这里如实返回，要不要继续由 `--kouling-only` 这类开关表达。
- ⭐ **门查两个页签**（`127 盘盘福利` + `129 精彩回放`）并合并去重：实拍里 147608（国庆场）
  **只在 129 里**，只查 127 会在"房间还没进福利页签"时判成"没开播"——那是**漏掉整场口令**那一侧的错误。
  原项目只查 127。
- ⭐ **门的两种"没有"分开报**：`LiveCount == 0`（谁都没在播）vs `LiveCount > 0 && !Active`
  （别的主播在播，实拍 147490「福建移动」）。两者的下一步完全不同（一个等、一个别等）。
- ⭐ **进账 = `user/info` 两次读数相减**，不是把口令的 `flowerNum` 加起来 ——
  后者会漏掉 `firstParticipate/give` 那几朵（它不在任何口令的结果里）。
- ⭐ **重听一次**只在有口令被判 `200108` 时做，且**排除已经试过的**（含第一次就成功的）——
  不排除的话第二次会把同一批再投一遍，而**同一账号**对同一条**正确**口令的第二次请求
  会回 `200112`（已兑换），看起来像"口令坏了"。
  ⚠ 理由**不是**"口令会被消耗"（口令全场通用，见下面那节五条事实）。
  ⚠ 合并两轮结果时 `InvalidCodes` 取**并集**（#114）。
- **退出码**：「今天没开播」退 **0**（门开不开由服务端数据决定，与"今天已经签过了"同一档）；
  只有"门那三步走不通""六步链解析失败"才退 1。

**未验证清单**（下次开播窗口 2026-09-23 14:30 之后补齐）：
⚠ 2026-09-22 复核：前 4 条已**拿原项目源码逐字对拍**（见文末那条补记），
所以它们现在是"**抄得对、但服务端认不认仍未知**" —— **仍未实拍**，开播后照样要核。

⭐⭐ **开播后请跑 `node scripts/probe-live-room.js --run`**（2026-09-22 加）——
下面这 6 条的**权威清单在脚本里**（`ITEMS`），脚本会把能自动核的核掉、
把要抓包的步骤与判据打出来，并把原始输出**追加**到
[`capture/probe-live-room-<日期>.md`](./capture/probe-live-room-2026-09-22.md)。
⚠ **别在这里再抄一份可执行清单** —— 坑表 **#116** 就是"手写清单声称覆盖全部、
实际漏了却不报错"。

⚠⚠ **要抓包的那三条，取证面是 App（`/Applications/mCloud.app`），不是 CLI** ——
`internal/io/client.go` 的 transport 没设 `Proxy`（Go 里 = 不走代理），WS 也是手写直连
⇒ 跑 CLI 时 Reqable 里**一条记录都没有**（坑表 **#120**）。
`node scripts/probe-live-room.js --wire` 会把前置条件与四条步骤打出来。

- WS 握手签名头（`systemmd5` / `tokenmd5`）的**真实取值**（现在按算式实现，取的是 48 字节 `DEVICE_INFO`）——✅ 算式已与 `apis/live-room-chat.ts` 逐字对拍；
- `member/apply` 的 `version`（`LiveApplyVersion = "v23.0.2"` 是**唯一**取自原项目、没有实拍的常量）——✅ 已与原件对拍；
- 一条**真口令**；`draw` 的 `200112`（**同一账号**第二次请求同一条**正确**口令 ⇒ 已兑换，
  **至今没有样本**）——❌ **只能等实拍**（原件里也没有样本）；
  ⚠ 它**出不来**的另一个原因：探针只收口令、不投口令（2026-09-23 改），所以"投两次"这条造样本的路已删；
- `visitlog/journaling` 的 5 个 `optkeyword` —— 只有 `_flowerBtn` 有实拍；✅ 另 4 条的名字与**顺序**已与 `tasks/red-flower/standalone.ts` 对拍（同名同序），`sourceid="1007"` 与 body 参数顺序也对上；
- WS 帧的**真实顺序**与 `command` 取值、以及"ack 之后必须补 UNREAD"这条（原项目如此，实拍没有）——⚠ 见坑表 **#117**：原项目里**两份实现互相矛盾**，Go 跟的是被任务调用的那份（`tasks/live-room-kouling.ts`），已逐字对拍。

**反向测试 56/56**：`.workbuddy-ai/tmp/reverse-live-room.py` 覆盖编解码（含 `pov` 零值写入）、
四条口令分支、WS 握手三处、帧状态机、驱动层的结构性跳过与双源容错、以及 CLI 层的用法闸 /
退出码 / 两轮合并。**每一条都确认目标用例真的变红**（判据见坑表 #111：编译失败 / 用例没跑 /
断言不承重是**三件事**，脚本要分开报）。

**门禁实况**：`node scripts/gates-go.js` → **九项全绿**（墙钟 26.4s；
build 1.3s / vet 0.5s / fmt 0.1s / test 17.7s / deps 3.5s / filenames 0.2s / idents 1.7s / golden 0.9s / jsontags 0.4s），
`golden` 断言 167 条全绿、`jsontags` 77 个结构体字段齐全 + 107 个按白名单不进 JSON。
新增的 5 条未验证豁免（`msyncFrames` / `msyncInbound` / `memberApplySignatures` /
`koulingExtract` / `xiaohongshuExtract`）都点名了 Go 侧的消费者，理由里写明
**MSync 的期望值由原项目自己的 protobufjs 产出**，本脚本再写第二份编码器是"更差的证据"。

坑表加 **#109–#115**（共 115 条）。`cli-usage.md` 加了 `live-room` 一行；
`TestCommandTreeMatchesTheDocumentedSurface` 的判据就取自那张表 —— 加子命令**必须**同步改文档。

## 补记（2026-09-22，直播/小红花落地后的**文档与守卫漂移**复核）

这一轮改掉了两个**被别处按数字引用**的东西（坑表条数 108 → 117、注册表条目 3 → 4），
于是把"引用它们的每一处"逐个核了一遍。**代码侧改了两处，文档侧改了六处**：

| 位置 | 原文 | 现状 | 处理 |
| --- | --- | --- | --- |
| `internal/capabilities/registry.go` 注释 | "这里先登记 3 项" | 注册表已 **4** 项 | 原句保留 + 带日期补记（§3.2：注释与代码矛盾先修矛盾） |
| `internal/cli/args_test.go` 的 smoke 清单 | **10 条**（叶子实为 **14**） | 补齐为 14 条 | ⭐ 见坑表 **#116**；并新增 `TestEveryLeafActuallyRunsIsComplete` 机器判据 |
| `docs/go-migration-plan.md` 步 4 | "Go 侧实测 3 项" | 对位组仍 3、全表 4 | 带日期补记，写明"**注册表条目数从此不等于迁移进度**" |
| `README.md` | "注册表 Go 侧 3 项" | 同上 | 带日期补记 |
| `README.md` | "`cli` 的 **12** 个叶子命令" | 实为 **14** | 改成 14 并**指向权威判据**（不再写死一个会再烂的数字） |
| `AGENTS.md` | `#1–#115` | 117 | 直接改（那是索引，不是记录） |
| `docs/migration-pitfalls.md` | 抬头 `#1 … #115`；那一节标题写"**五条**"（表里实有 7 条） | 117 / 九条 | 直接改（抬头与标题是**摘要**，写错就是错） |
| `docs/status.md` | 本节上面那句 `#109–#115（共 115 条）` | 117 | 原记录**保留**，本条即补记（§2.6） |

⭐ **这一轮真正学到的一条**（已登记为 **#116**）：`TestEveryLeafActuallyRuns` 的名字是
「每个叶子都真的跑起来了」，而它跑的是一份**手写清单** —— 实测当时只有 10 条、
叶子有 14 条，`smid reset` / `smid refresh` / `auth refresh` / `user init` 四条
**没人跑**，而**九条门禁全绿、没有任何输出**。加 `live-room` 的人（我）注册了命令、
改了文档表、写了 18 条用例，**唯独漏了这份清单**。
⇒ 修法不是"把清单补齐"（那只会再烂一次），而是补一条**清单 vs 命令树**的机器判据，
并给排除项加白名单 + **反向检查**（白名单挂了不在树上的路径也红）。
`user init` 的排除理由是**实测**的：裸跑会打印 `设置口令: ` 然后**在 stdin 上等输入**
（直接挂住，由 `TestUserInit*` 四条覆盖）。

**反向测试 60/60**（原 57 条 + 新增 3 条，打上面那条新判据的三个面：
清单漏叶子 / 白名单过期 / `commandPathOf` 只取第一个 token）。

## 补记（2026-09-22，刷新 auth 全链路落地：存储 → 上游 → 能力 → 驱动 → CLI → HTTP → WebUI）

**这一条链路"跨层才看得见"的事实全部收进新篇 [`auth-refresh.md`](./auth-refresh.md)**
（证据分档、判据三档与那三个单位陷阱、端别为什么不是参数、各触发面口径（当时三面，2026-09-23 起四面）、凭据边界、未验证清单）。
`cli-usage.md` 里 `auth refresh` 那一行早就指向它，此前是**悬空链接**，本轮补上正文。
另：`capture/app-rights-token-samples.txt` §4 那条"token 与 `authToken` 关系未验证"
**已按同一份日志复核并推翻**（刷新响应里那个 `token` 编成整串后在后续请求头里逐字命中），
按"只增改不删"以带日期的补记写在原段后面，旧结论保留。

**本轮新增的对外契约**：`POST /api/accounts/{id}/refresh-auth`（`{leadDays?, platform?}` →
`{refresh, account}`），`refresh.decision` 走 `RefreshDecision.Code()` 那三个**稳定英文码**
（`needed` / `not-yet` / `manual-only`，越界值给 `unknown`）；`internal/server/routes_test.go`
的 `goAdded` 登记了这条路由。**没到点也是 200**：那两档是正常结论，界面靠 `rotated` 分绿/灰。

**凭据边界**：新 token 只落库。`refreshAuthPayload` 是手写 map，`RefreshReport` /
`RefreshedCredential` / `RefreshTokenResult` / `RefreshOutcome` / `RefreshDeps` / `AuthRefreshArgs`
六个结构体**整个不给 tag** 并登记进 `scripts/check-go-jsontags.cjs` 白名单 ——
理由里最硬的一条：`core.SecretValue` 的 `String()` 只挡打印、**不挡 `encoding/json`**。

**实机核验**（Go 版跑在 **8789**，二进制 `target/mcloud-server-verify`、库是 `data/mcloud.db` 的
**副本** `target/ui-verify.db`，都不碰 8788 那个在飞的实例与共用库；核验完已删副本与自建的二进制）：

- 两端各点一次"刷新 auth"→ 库里两行的到期时刻**逐字未变**、网络面板只有那一条 POST（200）、
  结论 `not-yet` ⇒ **零上游请求**（默认提前量 7 天，两行都还剩约 29 天）；
- PC 那行给 Windows/macOS 二选一、手机那行免选并说明原因；换行开弹窗时正文里的账号名与到期时刻**跟着换**
  （Vapor 下把弹窗写在页面作用域里就是为这条）；把副本库那列改成 `-1` 后，弹窗多出一句"已显示过期"的提示；
- 桌面（同源 iframe 1280）与移动（375）两档：375 下 `scrollWidth=375` 无横向溢出、
  两个动作按钮 **44px**（`btn` 默认 40px 那条出入仍归上面"留给用户定"，本轮没自改判据，只在 `webui-components.md` §6.1 加了指回这里的补记）。
- ⚠ **`rotated=true` 那条界面分支没在浏览器里看过**：它要一次真实的 token 轮换（手动点一次或调度到点跑一次都会发生），
  没发生过 ⇒ 只算未验，**不构成"先别做"的理由**（旧项目把刷新挂在每轮任务收尾，§1 的参考实现）。绿条的**渲染**用"启用"那枚代理验过（同一个 `NOTICE_ALERT` 表）。

**门禁实况（本轮手动全量）**：`node scripts/gates-go.js` → **7/9 绿**（墙钟 1m48s；
build 4.5s / vet 1.5s / test 1m35s 带 `-race` / deps 5.7s / filenames 0.3s / idents 0.5s / golden 0.1s 全过），
红的两条是 `fmt` 与 `jsontags`，**条目全部落在并行会话那批未跟踪文件上**
（`internal/upstream/live_*.go`、`internal/app/live_room.go`、`internal/io/websocket.go`）——
按 §2 不动别人的在飞改动，所以本轮**没有**替他们 `gofmt` 或登记白名单。
本轮自己的面：`go test -count=1 ./internal/...` 除上面那批之外的包全绿，
`internal/app` 里刷新那 5 条单跑 PASS（该包当前红的是 `TestRedFlowerNoQualificationDoesNotAbortBatch`，他们的）。
`node scripts/gates-web.js` → **2 条全绿**（typecheck 5.3s + build 1.9s）。

## 补记（2026-09-22，拿原项目源码**逐字复核**直播/小红花那 4 条「未验证」）

原项目 **在磁盘上**（`~/Documents/workspace/mcloud-sign`，`MCLOUD_SIGN_SRC` 那个约定路径），
所以「未验证清单」里有 4 条**不需要等开播**就能复核 —— 复核方式是**逐字对拍原实现**，
不是升级成实拍（那是两件事，下面分开写）。

| 项 | 原实现的出处 | 对拍结果 |
| --- | --- | --- |
| `systemmd5` | `apis/live-room-chat.ts` `applyRoom`：`md5(concat([deviceInfo, ts10]))` | ✅ 与 `upstream.SystemMD5` 一致（含"第一段是**解码字节**"这条） |
| `tokenmd5` | 同上：`md5(\`Bearer ${t.slice(0,10)}${ts10}${t.slice(10)}\`)` | ✅ 与 `upstream.TokenMD5` 一致（含短 token 的边界） |
| `member/apply` 的 `version` | 同上：字面量 `"v23.0.2"` | ✅ 与 `LiveApplyVersion` 一致 |
| 5 条 `optkeyword` | `tasks/red-flower/standalone.ts:40-44` | ✅ **同名同序**；`sourceid` 默认值 `"1007"` 与 body 的**参数顺序**也一致 |

⇒ 这 4 条从"照抄原项目、无实拍"变成"**照抄已复核**"，但**仍未实拍** ——
开播窗口之后要核的仍是"服务端认不认"，而不是"抄得对不对"。

⭐ **本轮真正的收获是坑表 #117**：审 WS 层时先读到的是
`dist/…/live-room/client.js`（`LiveRoomClient`），它与 Go 侧**两处都对不上**
（心跳回 `[""]` vs `"h"`、UNREAD 走 500 ms 定时 vs 收到 `command===3`），
看着像抄错了。真相是**原项目里有两份互相矛盾的实现**：真正被任务调用的
`tasks/live-room-kouling.ts` 与 Go **逐字一致**，而 `client.js` 的 `.ts` 源**已被删**
（只在 `dist/` 留了编译产物，`src/` 里连调用点都没有）⇒ 它比任务那份**更容易被先读到**。
⚠ **差一步就把对的代码改错**。判据：对拍前先顺着入口的 import 找**被调用的那一份**，
别从 `dist/` 里挑文件读。

同一轮还修掉一处**自己写的**数字错：`DEVICE_INFO` 的字节数在**同一个文件**里
被写成 40 与 48 两种（真值 = 96 个 hex 字符 = **48 字节**，`len(hex)/2`）。
已统一为 48，并把"别按看起来估"写进注释。另外 `--listen-seconds` 的默认值
原注释写"60 秒取自原项目" —— **不成立**：原项目**主路径**是 `timeout=0`
（上限 **180 s** + **收到第一个口令就返回**），60 s 只是**重试那趟**的值。
Go 这里默认 60 s 是**本项目自己的选择**、且**不做早退**（听满才收得齐多个口令），
注释已按这个口径重写。

**门禁实况（本轮手动全量）**：`node scripts/gates-go.js` → **九项全绿**（墙钟 1m15s；
build 1.8s / vet 0.7s / fmt 0.1s / test 1m05s / deps 3.9s / filenames 0.3s / idents 1.5s / golden 0.9s / jsontags 0.4s）。
⚠ 上面那节（auth 刷新那一轮）记的"7/9 绿、红在并行会话的未跟踪文件上"
**现已消解** —— 那批文件就是直播这一层的，它们已 gofmt 并登记白名单。
**反向测试 60/60**（`.workbuddy-ai/tmp/reverse-live-room.py`）。

## 补记（2026-09-22，「只听第一间」拍板 + 开播后核对脚本）

### 一、「只听第一间」是**取舍**，不是漏了循环

原项目 `tasks/live-room-kouling.ts` 的 `getWebSocketKouling` 是
`for (const room of rooms)` —— 把该主播**每一间**在播房间逐间 `await` 听一遍。
用户 2026-09-22 拍板**保持现状**：Go 侧只取 `Rooms[0]`。

⚠ 所以它是**拿"漏口令"换"墙钟时间"**：同时开两间时，第一间之外的口令会被整场漏掉 ——
而这正是本命令另一处（两个页签都查）**特意避开**的那个错。既然有代价，就必须
**说出来**：`run_live_room.go` 在房间数 > 1 时打一句
「⚠ 该主播有 N 间在播，本次**只听第一间**」。不说的话用户看到的是
「在播房间 147606，听 60 秒」，会以为已经听全了。

用例 `TestLiveRoomListensToOnlyTheFirstRoom` 钉**两半**（只进了一间 / 而且说出来了）。
⚠ 它是**少数**需要读**请求体**的用例：WS 地址形状是
`/ws/<serverId>/<sessionId>/websocket`，**里面没有房间号** ⇒
"到底进了哪一间"只能从 `roomBeat` 的请求体看（为此加了 `liveRoomRoutes.bodyMatching`）。
⚠ 用例里那条"拨号次数 == 1"**未被独立断言**（它被 `roomBeat` 计数那条盖住），
已在注释里声明 —— 不是"没红过所以不重要"。

### 二、`--json` 补了 `draws[].upstreamCode`

**理由**：`otherCodes` 是个**集合** —— `200112`（已兑换）与任何没见过的码都落进去 ⇒
在输出上**分不出抓到的是哪一个**。而未验证清单里正有一条「`200112` 至今没有样本」。
没有这个字段，那条**只能靠代理抓包**才能关掉（最容易一直拖下去的一条）。
⚠ **只出码、不出 `message`**：后者是上游原文，jsontags 白名单里写明它
"能自动 marshal 就离顺手把整个结构写进日志/响应只差一步"。

### 三、`scripts/probe-live-room.js`（开播后一键核对）

`--run` 跑能自动核的两条（在播/门 + 真口令，**只读**）；
`--run --code <口令>` 追加「同一口令投**两次**抓 `200112`」（⚠ 写操作、不可逆）；
`--wire` 检查抓包前置条件并打印三条要抓的判据。
退出码 **0 = 核完 / 1 = 前置成立但有项没拿到 / 2 = 前置不成立（没在播）** ——
⚠ 2 与 1 必须分开：把"今天没开播"记成"核过了没拿到"，下次真开播时会以为已经核完了。

**首次运行（2026-09-22 19:22，窗口前）**：两个启用账号都是 `gateOpen:false`、
`liveCount:0` ⇒ 退出码 2，报告落 [`capture/probe-live-room-2026-09-22.md`](./capture/probe-live-room-2026-09-22.md)。
即"**没开播**"这一支已被实测走通（与 9-22 抓包那轮一致）。

### 四、坑表加 **#118 / #119 / #120**

**#118：素材（夹具）本身的语法错，在没人消费它的时候零信号**。上一轮加的两间房夹具
多了一个 `}`，而它是 `const` 块里的一段字符串 ⇒ `go build` / `vet` / `gofmt` /
九条门禁**都不看它**。这一轮第一次被跑，报的是
「`[直播·页签 127] 响应体不是合法 JSON`」—— **指向页签那一步**，
读起来像"上游换了信封形状"，代价是一整轮排查方向。
⇒ 夹具写完**当场过一遍解析器**；更根本的是**别让夹具在 `const` 块里过夜**
（加夹具与写消费者同一轮做完）。

**#119：`scripts/fmt.js` 声称"末尾参数限定扫描子路径"，实现却是追加 ⇒ 限定失效**。
⚠ **本轮实际踩到了**：想只格式化自己新写的两个脚本，跑了
`node scripts/fmt.js scripts/probe-live-room.js scripts/gen-live-room-vectors.cjs`，
结果把 **`web/src/pages/accounts-page.tsx`（另一个会话在飞的改动）也重排了**。
⚠ 内容是**格式**改动（语义中性、且该仓库本就要求跑格式化），但**已混进别人的工作树差异**，
撤不回来 —— 已在 §2 的协作口径下如实记下，改动只按格式处理、**未碰其内容**。
已修：给了子路径就 `targets = extra`，不给才用全仓 glob。
**验法**（这条才是关键）：传一个单文件看它扫几个 —— 修前 **34** 个、修后 **1** 个；
只验"退出码 0"等于没验（#93 那一族）。修完全仓 `--check` → 34 个文件全部已格式化。

**#120：`--wire` 那三条的取证面是 App，不是 CLI** —— 这条是**本轮自我复核时发现的**，
上一轮写进脚本的第一版方案（"跑一遍 `mcloud live-room`，用 Reqable 抓"）**根本不通**：

- `internal/io/client.go` 的 `http.Transport` 是**显式构造**的、**没设 `Proxy` 字段** ——
  Go 里那是"**不走代理**"，**不会**回落到 `http.ProxyFromEnvironment`
  （旁证：`internal/server/run_test.go` 自己写过 `&http.Transport{Proxy: nil}` 并注释
  "**默认** transport 会读 `HTTP_PROXY`"）。WS 也是手写直连。
  ⇒ 跑 CLI 时 Reqable 里**一条记录都没有**，而那个现象最容易被读成"CLI 没发请求"。
- 顺带查出前置检查用了 `pgrep -f Reqable`，它会命中
  `/Applications/Reqable.app/Contents/Helpers/mcp-server` —— 而那个 helper 是
  `reqable-capture.py` **自己拉起来的** ⇒ 该判据**恒真**（#93 那一族）。已改成
  落在 GUI 二进制 `Contents/MacOS/Reqable` 上。
- ⭐ 更根本的一层：这几条问的是"**真实客户端**怎么做"，
  抓我们自己 CLI 的帧是**循环论证**（只能证明"我发了我以为的东西"）。
  ⇒ 取证面是 **App**（`/Applications/mCloud.app`，已确认装在机上），
  与既有样本（`capture/app-*-samples.txt`）同一路。脚本里的抓包步骤已按此重写。

**反向测试 67/67**（本轮新增 #61–#67：只听第一间三个判据面 + `roomBeat` 请求体 +
`upstreamCode` 三个判据面）。

### 五、`probe-live-room.js` 的退出码**只有两档**（坑表 **#121**）

自查那个脚本时发现的：文件头写着三档（`0` 核完 / `1` 前置成立但有项没拿到 / `2` 没在播），
而末尾的实现是 `process.exit(notLive ? 2 : 0)` —— **`1` 永远出不来**。
后果：**"门开着、主播这一段一条口令都没喊"会退 `0`、被记成"核完了"** ——
而它恰恰是最该重跑一次的处境，窗口又只有几小时。这与 `2` 的初衷
（"别把没开播记成核过"）是**同一族的错，只是换了扇门**。

⚠⚠ 它**天然躲过所有测试**：`0` / `1` 只在真开播时出现，那天造不出来 ⇒
三条分支里唯一能跑的是 `2`。**"读代码觉得对"正是这条的成因。**

已修：
- 分档写成**一个变量 + 一处 `exit`**，并把**理由连档位一起打**
  （`退出码 2：前置不成立（现在没在播）⇒ 后几条不适用，不是"核过了没拿到"`）——
  到那天一眼能看出"是哪一档、该不该是这一档"。
- ⚠ 顺带修掉一处**自相矛盾**：前置不成立时脚本仍在打「❌ 试过却没拿到的有 2 条」，
  而上一句已说"后两条**不适用**" ⇒ 两句话互相打脸，会把"今天没开播"渲染成失败
  （#113 同族）。现在这两个块都只在**前置成立**时才打。
- **"没试"与"试了没成"分开**：`manual`（要 `--code`）/ `wire`（要抓包）**不算失败** ——
  把前者算成失败会让"退 0"**永远达不到**，三档当场退化成两档。

**已实测**：`--run --listen 1` → 退 **2**（门没开 `liveCount=0`），且不再打那句矛盾的 ❌。
⚠ **`0` / `1` 两档仍未实测**（要真开播才造得出来）—— 这一条**到 9-23 才关得掉**，
与那 6 条同一个窗口。

**顺带把脚本与 CLI 的契约核了一遍**（免得开播那天才发现对不上）：
- 脚本读的键名与 `liveRoomJSON` 逐个对上：外层 `liveRoom`、`gateOpen`、`liveCount`、
  `kouling`、`draws[].upstreamCode`。
- `probeLiveNow` 确实带了 `--kouling-only` ⇒ **前置检查是只读的**，不会误投。
- "跑两次抓 `200112`"的前提成立：`splitLiveCodes` → `app.MergeKouling` **真的去重**
  （`seen` map，重复即 `continue`）⇒ 同一个口令写两遍会被合成一条，只能跑两次命令。
- 三个 argv 都过 `validateLiveRoomArgs`（`--kouling-only` 与 `--skip-kouling` 互斥、
  `--skip-kouling` 需 `--codes`、`--listen-seconds > 0`）。
- ⚠ 报告文件 [`capture/probe-live-room-2026-09-22.md`](./capture/probe-live-room-2026-09-22.md)
  里除 19:22 那次真跑，还多了**几条 `--listen 1` 的自查运行**（params 行可分辨）——
  按"运行记录**只增不删**"的口径**保留**，不回溯删。
- ⚠ 同一份报告里出现过**两个"退出码"**（raw 块里是 **CLI 的** `0`、末尾是**脚本的** `2`），
  主体不同却同名 ⇒ 已改成「**脚本退出码**」/「**CLI 退出码**」，并把脚本档位**写进报告**、
  挪到 `appendReport` **之前**算（原先报告里根本没有脚本自己的档位）。
  早期几条记录由旧版生成，文件头加了一行说明它们那个裸「退出码 N」是 CLI 的。

### 六、退出码：`--account` 越界在两条子命令上分了不同的档（**已决 → 越界/负都退 2**；⚠ `0` 两条**不同**，见本节末尾）

扫"分档里有没有不可达的那一档"（#121）时，顺手比了各命令对**同一类用户错误**的档位。

判据（原项目）：`crates/mcloud-cli/src/main.rs` 的 `main` **只有两档** ——
`Ok(()) => ExitCode::SUCCESS`，其余一律 `ExitCode::FAILURE`；**`2` 只可能来自 clap 自己**。
而 `SigninArgs::account` 是 `Option<usize>`（**无符号**）⇒ 原项目里：

- `--account -1` → **clap 解析错**（负数进不了 `usize`）⇒ **2**
- `--account 99` → **应用层错**（`select_signin_targets` 返回 `Err`；Rust 单测
  `signin_account_index_out_of_range_reports_the_total` 直接调那个函数）⇒ **1**

实测（二进制编在 `/tmp`，**只用越界 / 非法值**，不触发任何写操作）：

| 用例 | 实得 | 原项目应为 | |
| --- | --- | --- | --- |
| `live-room --account 99` | **2** | 1 | ✗ |
| `live-room --account -1` | 2 | 2 | ✓ |
| `signin --account 99` | 1 | 1 | ✓ |
| `signin --account -1` | **1** | 2 | ✗ |
| `signin --account 0` | 1 | 1 | ✓ |

⇒ **同一条 `--account`（同一套下标语义）在两条子命令上分了不同的档**：
越界时 `live-room` 退 2，而 `signin` / `smid` 退 1（`run_smid_test.go:272` 的断言
写的就是"账号不存在是运行错误"）。而两边**代码注释都写了理由，理由还相反**：

- `run_live_room_test.go:531`：「越界是"调用写错了"，不是"这次没跑成" —— 退出码必须分开，
  否则脚本分不清该不该重试」
- `run_signin.go`（旧第 249 行）：「这也是**运行**错误（退 1），不是用法错误」
  ⚠ 这条注释**已随本次改动重写**，原文已不在代码里 —— 留在这里是"当时的理由"

⚠⚠ 而"沿用原项目"在这里**不能直接当判据**：原项目的 `1` **不是决定，是只能** ——
clap 管不到库里有几个账号，所以那一档它给不出 `2`。要区分
「原项目**决定**这样」与「原项目**只能**这样」。

**已决（2026-09-22，按实际需求定，不对标 Rust）** ——

用户口径：「对齐这个看实际需求啊，你实现，之前都说了不要对标 rust」。
于是判据取 **退出码契约自己那两句**（`ErrUsage` 的注释）：
脚本靠 `1` / `2` 的差判断"是我的调用写错了"还是"这次真的没跑成"。
"库里没有第 N 个账号"**重试不会变好** —— 只能改调用（换个下标，或先登录一个）
⇒ 它是"调用写错了"那一档 ⇒ **`2`**。

**已改**：`run_signin.go` 两处（下标 `<= 0`、越界）都带上 `ErrUsage`；
`live-room` 本来就是 `2`，不动。⚠ 判据**不是**"原项目退几"：
原项目那条 `1` 是"clap 管不到库里有几个账号"的**副产品**（坑表 **#122**）。

**未改**：**按 id 点**的 `--account`（`smid` / `auth`）不存在时仍退 **`1`** ——
它的入参是**名字**而不是**位置**，且 `smid` 与 `auth` 两条**彼此一致**。
⚠ 要不要合成一档，留到 `auth` 那笔在飞的工作落地后一并看
（现在只动 `smid` 反而会让它与 `auth` 分叉）。已写进
[`cli-usage.md`](./cli-usage.md) 的「退出码约定」一节。

**实测矩阵**（二进制编在 `target/`，**只用越界 / 非法值**，不触发任何写操作）：

| 用例 | 改前 | 改后 |
| --- | --- | --- |
| `signin --account 0` | 1 | **2** |
| `signin --account -1` | 1 | **2** |
| `signin --account 99` | 1 | **2** |
| `live-room --account 99` | 2 | 2（未动） |
| `live-room --account -1` | 2 | 2（未动） |
| `live-room --account 0` | **0**（空库） | **0**（空库；未动，**合法**：`0` = 全部启用账号） |
| `smid refresh --account <不存在 id>` | 1 | 1（未动，见上） |

**反向测试**：把 `run_signin.go` 那两处 `ErrUsage` 撤掉 ⇒ 三条用例**全红**
（`应退 2，实得 1`）；还原后逐字节一致（sha256 `6f4eccab…`）、复跑绿。

⚠ 顺带：那条用例**名字**原本叫 `TestSigninAccountZeroIsARunError`，
改完口径后名字与断言**矛盾** ⇒ 已改名 `…IsAUsageError`，注释也改成
"判据是改调用能不能好"，不再引用 Rust 的 `usize`。

⭐ **顺带一条干净的负结果**：CLI 自己的**三档**（`0` 正常 / `1` 运行错误 / `2` 用法错误）
**全部实证可达**，且**早就有测试钉着**（`ExitUsage` 在 `server/run_test.go:186`、
`ExitRun` 在 `:290`「库打不开」与 `run_smid_test.go:271`）⇒ 这一族在 CLI 上**没有**
#121 那种缺陷，本次手跑只是冗余确认。**没有替 fmt.js 的 `exit 1` 做同样的证明** ——
那要往共享工作树里放一个故意未格式化的文件，代价大于收益，如实记为**未验**。

#### ⚠ 收尾复核又查出：`--account 0` 两条命令**语义不同**（**未改**，交用户定）

写 §十一 时为了核对"越界"那两档，顺手把 `0` 也实测了一遍 —— 结果**两条命令不一样**：

| 用例 | 实得 | 含义 |
| --- | --- | --- |
| `signin --account 0` | **2** | 下标从 1 起 ⇒ 写 `0` 是**调用写错了** |
| `live-room --account 0` | **不是 `2`**（空库实测 `0`） | **合法**：`0` = "**全部启用账号**" |
| 两条的 `--account -1` / `--account 99` | **2** | 为负 / 越界（本节正文那两档，已对齐） |

⚠ 表里 `live-room --account 0` 那格的 `0` 是**空库**下的值（没有账号可跑）⇒ 判据是
"**它没被判成用法错误**"，**不是**"它退 `0`"；有账号时它会真的跑起来。
⚠⚠ **而我在 `cli-usage.md` 的「退出码约定」里一度把它写成了"两条的 `0` 都退 `2`"** ——
那是**过度概括**：一句话把一条**真陷阱**盖住了（同名 flag 的 `0` 语义不同）。已订正
（见 `cli-usage.md` 与坑表 **#122** 的收尾）。⚠ 这个错的形状值得记：**我把"两条命令我实测过
越界"推广成了"两条命令的 `--account` 语义一致"** —— 而 `0` 我**没测**。

**为什么没改**：两边**各自的理由都写在自己代码里**，属于 #122 说的"两边都有理由"那种：

- `signin` 侧（`internal/cli/args.go`）：Go 的零值分不开"没给 `--account`"与"给了 `0`"，
  所以把后者判成**用户的错**（`0` 是合法的第 0 个账号不存在）。
- `live-room` 侧（`internal/cli/args_live.go` 的 `--help` 文本 + `run_live_room.go` 的
  `index == 0` 分支 + `internal/server/runs.go` 的"留空 = 全部启用"引用了它）：
  把 `0` 当作**缺省的显式写法**。

⇒ 按 #122 的纪律**不单方面改外部行为**。**建议**：把 `live-room` 也对齐成 `2` ——
想要"全部"本来就不给这个开关，而 `live-room` 那条链里有**不可逆的写操作**（`draw` 抽小红花），
把可能是笔误的 `0` 静默展开成"全部账号"是更危险的那一侧。**等用户定**。

⚠ 顺带一处**注释歧义**（未暂存、属另一个会话在飞的文件，只加了一句澄清、没改行为）：
`run_live_room.go:199` 的「与 `mcloud signin --account` **同一口径**」容易被读成
"`0` 两边也一样"，而事实相反 —— 已补一句限定它只指"下标从 1 起"那半句。

### 七、黄金向量的**出处与可复现性**（正面结论）

WS 那一段是"照原项目 + HAR"写的，**当天没开播 ⇒ 没有实拍**，
于是它身上唯一**承重**的证据就是 `test/golden/live-room-vectors.json` 里那些**逐字节**的期望值。
所以这轮去查它是不是**同义反复**（坑表 **#93** 那一族：判据的证据由被检查的东西自己提供）。

**结论：不是同义反复，而且可复现。**

- 生成器**不是"再写一遍算法"**：`protobufjs` 用的是**原项目 `node_modules` 里那一份**（8.7.1），
  口令 / 小红书两段用**原项目自己的 `tsx` 直接跑 TS 源码**
  （`packages/core/src/live-room/{kouling,xiaohongshu}.ts`）⇒ 期望值是**原实现真跑出来的**，
  不是手写的。⚠ 这条有前车之鉴：第一版把这两段的 `expect` 手写了，
  34 条里**有 3 条与真实现不符** —— 手写的期望值进了黄金向量就是**一条假基准**，
  它会把"Go 侧照抄了原实现"判成红、或更糟把"抄错了"判成绿。所以现在**找不到原实现就直接退出**。
- `$evidence` 记了"基准是哪一份源码"，可复核：`kouling.ts` sha256 `f9f01c75…`、
  `xiaohongshu.ts` sha256 `d7b9483e…`，外加 `protobufjs 8.7.1` 与 `tsx` 的口径。
- **实测可复现**：`NODE_PATH=<原项目>/node_modules node scripts/gen-live-room-vectors.cjs`
  → 与工作区那份**逐字节一致**（sha256 `a00a09aa…`，501 行，`diff` 无差异）。
  段数：`msyncFrames 3` / `msyncInbound 3` / `memberApplySignatures 3` /
  `koulingExtract 36` / `xiaohongshuExtract 5`。
  ⇒ "逐字节对拍"从一条**不可复跑**的断言，升级成一条**可复跑**的断言
  （⚠ 生成器的输出路径是**写死**的 ⇒ 复跑会覆盖仓库里那份，先备份再跑）。

**顺带修掉生成器里三处废弃做法的残迹**（都与代码 / `$evidence` **自相矛盾**，属同一族）：

- 文件头原写"**必须**能 `require` 到原项目的**编译产物**（`packages/core/dist/…/kouling.js`）"，
  而同文件 `$evidence` 写的是"走 `packages/core/**src**` 的 TS 源码" ⇒ 已改成正解，
  并把**为什么不用 `dist`** 写进去：那份产物**真的过期过**（dist 2026-08-01 / src 2026-08-26），
  那一版缺 `isQuestion` 过滤 ⇒ 9 条"观众提问"用例的期望值会变成"**应该**解出口令"，
  后果不是"多跑一次"，而是**把正确的 Go 实现判成红**。
- 引用了一个**不存在的函数** `[loadOriginal]`（真名是 `runOriginal`）。
- 小标题「原项目的**编译产物**（口令 / 小红书）」→「原项目的**源码**」。

### 八、黄金向量**逐段都有断言吗** —— 发现一处"看着有覆盖、其实没有"（坑表 **#123**）

接着上一节问：那五段（`msyncFrames` / `msyncInbound` / `memberApplySignatures` /
`koulingExtract` / `xiaohongshuExtract`）**是不是每段都真的被断言**，还是有的只是躺在文件里。

**正向结论**：五段**都**有消费者 —— `live_room_vectors_test.go` 逐段对拍，
其中 `msyncFrames` 是**逐字节**比较（最硬的那条）。

⚠ **但 `constants` 段只覆盖了一半**：9 个字段里 **6 个没人读**
（`AppKey` / `LiveAppKey` / `Domain` / `UserAgent` / `DeviceInfoHex` / `ProtoTypesUsed`），
`msyncFrames[].expect.deviceId` 也声明了却没人比。细查后**大部分是良性的**：

- `LiveAppKey` / `SDKAppKey` / `LiveDomain` / `LiveMSyncUserAgent` / `Provision*` /
  `LiveIdentificationPrefix` —— 被**逐字节**对拍**间接**钉住（常量写错 ⇒ 编出的字节
  不等于向量 ⇒ 红），**不需要**再单独比一遍。
- `deviceId` / `crypto` —— 原实现**硬编码成 0**（`proto.ts:188` / `:192`）⇒ proto3
  默认值不写 ⇒ 由字节相等覆盖。⚠ Go 侧连 `deviceId` 字段都**没有**，那是**有意的**
  （已把理由写进结构体注释，免得后人以为漏了）。

⚠⚠ **但 `LiveDeviceInfoHex` 不在任何一条链上**：编码用例取的是**向量里的**
`frame.DeviceInfoHex`，**不是**这个常量 ⇒ 这个**生产常量**
（`live_room_drive.go` 算 member/apply 的 `systemMd5`、`live_msync.go` 填 `MSync.deviceInfo`）
**改错了没有任何用例会红**。而它恰恰就是同一个文件里被写成过 40 / 48 两种字节数的那个常量。

**已修**：新增 `TestLiveRoomConstantsMatchVectors`，把五个常量**显式**钉到向量的
`constants` 段，并钉住 `LiveDeviceInfo()` 的 **48 字节**；顺带在比较前先判 `want == ""`
（段名对不上时比较会**静默变成空比较**）。

**反向测试**：把 `LiveDeviceInfoHex` 末位 `b→c`（**仍是合法 hex，编得过**）⇒
该用例**红在断言上**（不是红在编译上，见坑表 #111 那三分法）；
还原后文件逐字节一致（sha256 `deb2e41f…`）、复跑绿。

**这条的判据已登记为坑表 #123**：黄金文件 / 夹具里**声明了字段 ≠ 有断言**；
对**间接**钉住的常量要顺着链**走到头**确认它真在链上 —— 区别在于用例用的是
**常量**还是**向量里的值**。

### 九、`docs/` 的表格有"渲染时静默丢一列"（坑表 **#124**）

接着第八节核表格完整性时发现的 —— **不是本轮引入的**，是既有缺陷。

**判据是渲染，不是数管道符**：

- 第一版脚本直接 `split('|')` ⇒ 报 **42 处**，**绝大多数是假阳性** ——
  `\|` 是**转义**、不算分隔符（这一版我差点当成结论报出去）。
- 按 markdown 规则（`(?<!\\)\|`）重扫 ⇒ **19 处**。
- 用**真实渲染器**（`markdown-it`，装在隔离的 node 工作区里）渲一遍才定性。

**实证**：`migration-pitfalls.md` 的「步 1 已实测出来的二十七条」那张表，
表头是 **3 格**（`现象|根因|做法`），而从 `#15` 起的行是 **4 格**（多一个 `#` 列）
⇒ 整块被当成**一张表**，渲出来**每行都是 3 个 `<td>`**
⇒ `#15–#27` 那 **13 行**的**「做法」列全部不渲染**（`#72–#74` 同病，合计 **16 行**）。

⚠ 源文件里内容**都在**（grep 找得到）⇒ **"文件里有"与"读得到"是两件事**。
⚠⚠ 而想当然的"**插一个空行**"是**错的**：那 13 行**没有自己的表头**，
插空行只会把它们降级成**普通段落** —— 比原样更糟。正解是**补表头 + 分隔行**。

**没有单方面改**：同一张表里同时存在"`#` 独占一格"与"`#` 并进现象格"两种写法
（`#72–#74` 是前者、`#75–#78` 是后者），改哪种都要动 ~30 行，且**意图有两种合理读法**
⇒ 按 **#122** 的教训**只记录、交用户定**。

**另 3 处是"行少一格"**（渲染会补空、不丢内容，但内容会**左移一格**）：
`caiyun-go-parameters-and-decisions.md:42,43`（3 格 vs 表头 4）、
`webui-code-guide.md:232`（2 格 vs 表头 3）。⚠ 这两处同样**未改**。

#### ⚠ 补记（2026-09-22 收尾）：**#124 那一行自己就是受害者**，而且我又犯了第二次

做 §十一 时顺手用**源文件**口径复核了一遍全仓表格（渲染口径查不出来 —— markdown-it 会把
多余的格**截断**掉，所以每行渲出来都"等于"表头格数）。结果查出**第 17 行**：

| 位置 | 症状 | 真因 |
| --- | --- | --- |
| 坑表 **#124 那一行自己** | 表头 4 格 / 本行 **7 格** ⇒ 渲染时**末三列全丢** | 格子里（**含行内代码**）写了 3 处**未转义**的竖线：`` `split('\|')` ``、`` `现象\|根因\|做法` `` |

⚠⚠ 也就是说：**"数管道符数不出来"那条判据，连它自己那一行都没管住**。而更难看的是 ——
**我修它的时候，在新写的那句解释里又犯了第二次**（写了 `` `|` 一律写成 `` 的对照），
是复核脚本第二次报警才收住。

**已修**（只做转义，不改任何语义）：3 处竖线改写成 `\|` —— 渲染出来仍是干净的竖线。
⚠ 顺手实测排除了另一个看着更漂亮的写法：`&#124;` **不行**（行内代码里实体不解析，
会原样显示 `&#124;`）。⇒ **写侧规则**：格子里（含行内代码）的竖线一律写 `\|`。

**现在的状态**：源文件口径下，`migration-pitfalls.md` 只剩**既有那 16 行**（`#15–#27` 的 13 行
+ `#72–#74` 的 3 行，就是本节记下的那个"多一个 `#` 列"），`cli-usage.md` / `status.md` /
`AGENTS.md` 各 **0 行**。

**⭐ 已落地（2026-09-22 夜）：做成了第 10 条门禁** —— `scripts/check-md-tables.cjs`。
三处取舍都记下来，因为它们各自都是"换一种做法就会变成另一类错"：

- **那 16 行"怎么办" ⇒ 不动，改成豁免**。`EXEMPT` 按**行首编号**认、**不按行号**（行号会漂），
  并配**过期豁免反向检查**（名单里某条若已不存在 ⇒ 门禁同样失败；过期豁免会**盖住**以后新增的同号
  缺陷，见 **#116**）。理由：那 16 行要改就得在"补表头"与"把 `#` 并进第一格"两种读法里选一种，
  **没有单方面改**（见上）。
- **只拦"多于表头"一个方向**。格数**少于**表头时 GFM 会补空格子、**不丢内容**，那是**另一类**缺陷
  （漏一列 / 行被硬换行拆开）⇒ 两个方向塞进一条门禁，只会让这条门禁的判据与它要防的伤害脱钩 ——
  那正是 **#127** 记的错。已查到的"少于"那几处（`caiyun-go-parameters-and-decisions.md` 两行、
  `webui-code-guide.md` 一行被硬换行拆开）**未改**，记在这里。
- **判据在源文件上，不在渲染结果上**（渲染后每行都被截断成"等于表头"，那个口径**永远看不出**这个缺陷）。

门禁数 9 → **10**，随之同步 8 处：`gates-go.js` / `gates-scope.js` / `.githooks/README.md` /
`dev-gates.md` / `docs/README.md` / `go-migration-plan.md` / `README.md` / `AGENTS.md`。
⚠ 其中一处**不是抄数字**：分类器原来那三张名单里**都没有它** ⇒ "**只改文档**"的提交
（恰恰是它该拦的场景）跑不到它 ⇒ 进 `ALWAYS`，现在恒跑三条。详见本节后的补记。

### 十、提交前把**暂存区**物化出来编一遍 —— 发现暂存漏了一个依赖（坑表 **#126**）

前面几节都说"门禁绿"。⚠ **但门禁跑在**工作区**上，而提交送出去的是**暂存区** —— 两者不是一回事。**

判据：`git checkout-index -a --prefix=/tmp/staged-tree/` 把**索引**物化成一棵独立树，再 `go build ./...`。

**第一次物化：当场编不过**

```
internal/app/live_room.go:432:13:       err.WithPrefix undefined (type *core.Error has no field or method WithPrefix)
internal/app/live_room_drive.go:45:19:  err.WithPrefix undefined
```

根因：**我暂存漏了一个被依赖的改动** —— `internal/core/errors.go` 的 `WithPrefix`（+23 行）
一直只在**工作区**、从没 `git add` 过（它的注释还点明是"小红花六步链"要用的，正是我这轮的东西）。
⚠ 这个洞门禁**永远看不见**（工作区里那个方法在，`go build` 全绿）。**已补暂存。**

**第二次物化：`internal/app` 过了，`internal/cli` 编不过**

```
internal/cli/cli.go:278:18: undefined: newAuthCommand
```

⚠⚠ 暂存的 `internal/cli/cli.go` 里**混进了另一个会话的注册行**（`newAuthCommand`），
而它的定义在**未跟踪**的 `internal/cli/args_auth.go` 里。

**第三次（把未跟踪文件也拷进去）：`internal/capabilities` 编不过**

```
internal/capabilities/auth_refresh.go:169:16: undefined: authFromParts
internal/capabilities/auth_refresh.go:248:23: store.RotateAccountAuth undefined
```

⇒ 那个会话的改动**自己也还没自洽**（`RotateAccountAuth` 在**未暂存**的
`internal/core/traits.go` / `internal/io/store.go` 里）。

**结论**：**工作区编得过**（门禁只有 `fmt` / `jsontags` 红），**但暂存子集编不过** ——
缺的是**别的会话**的东西，不是我的 ⇒ **别提交子集**；等对方那笔落地，或整棵树一起提。

⚠ 另：我这 36 个暂存文件里有 **3 个**被别的会话在工作区改过（`git diff --cached` ∩ `git diff`）：

- `internal/cli/run_live_room.go`（+77 / −236，**还在改**，mtime 21:16）
- `internal/cli/run_live_room_test.go`（+3 / −2）
- `docs/capture/app-live-redflower-samples.txt`（+97 / −1）

⇒ 提交我的暂存版本 = 提交**过时**版本（不会丢对方的工作区改动，但那一笔内容不是最新的）。

### 十一、`--account` 对齐之后门禁**红了一条毫不相干的用例** —— 判据自己分不出层（坑表 **#127**）

把 `--account <越界>` 从退 `1` 改成退 `2`（§六 的收尾）之后，九条门禁里 `test` 红了，
红的却是 **互斥表**那条（`TestExclusivePairsMirrorTheRustTable`）：

```
args_test.go:192: [--account 3 --status-only --json --db …/state.db] 参数合法，却被判成用法错误：
  stderr="错误: 下标 3 超出范围：库里一共 0 个账号（用 `mcloud signin --list` 看清单）: 用法错误"
```

读起来像"**我的改动把一条合法参数判错了**"。真因是那条判据**本来就分不出层**：
它声称的是"**解析器**放行了它"，实现却写成 `code != ExitUsage` —— 而 `2` 同时属于
**解析期**（cobra 的 flag-group 校验）与 **RunE 期**（手工判的用法错误，如
`--records-all` 缺 `--records`）。⇒ **判据的表达力 < 它要判定的事物的维度**，
而失配时**红的地方不是真因所在**（登记为坑表 **#127**）。

**改的是判据、不是放宽它** —— 新增 `assertParsesCleanly`（只到解析期），
accepted 那半**两条判据都留**：

| | 改前 | 改后 |
| --- | --- | --- |
| accepted 那半 | `Main(...)` ⇒ **一条**判据：`code != ExitUsage` | **两条**：① `cmd.ParseFlags(argv)` + `cmd.ValidateFlagGroups()` 都不得报错（**层**）；② 仍端到端跑一遍、不得退 `2`（**契约**）—— 除非 argv 点了账号下标 |
| rejected 那半 | 端到端退 `2` + stdout 干净 | **不变** |
| `assertAccepted` | 两个用例共用 | **保留**（accepted 那半 ②、以及钉"库空退 0"的 `TestRecordsAllRequiresRecords`） |

⚠⚠ 为什么**不能只留 ①**：解析期放行**不等于**"跑起来也不会被判成用法错" —— 后者是**另一层**
的事（`--records-all` 缺 `--records` 就在 RunE 里判）。只留 ① 会让将来有人往 `accepted` 里
塞一条 RunE 层会被判错的组合时**静默放行**。⚠ 而 ② 对 `--account 3` 不成立（本用例的库是
**空的**，任何下标都越界 ⇒ 那**就是**用法错误，#126）⇒ 这个**例外具名写进条件**
（`namesAnAccountIndex`），**不是**把整条用例的端到端断言一起砍掉。

⚠ 两半判据不同层是**有意的**：rejected 那半钉的是**契约**（"非法就该退 2"），
accepted 那半**层与契约各钉一次** —— 契约有码可钉，**层没有**，所以层那一半只能直接调。

**验证（三条反向测试 + 门禁）**：
1. `go test ./internal/cli/ -run 'TestExclusivePairsMirrorTheRustTable|TestRecordsAllRequiresRecords|TestSigninAccount' -count=1` ⇒ **ok**；`./internal/cli/` 全量 ⇒ **ok**。
2. **反向 ①（层那半承重）**：往 `signinExclusive` 多插一对 `{"account","status-only"}` ⇒
   断言**红在断言上**（不是编译错），且 **root 与 `signin` 两个 head 都红**
   （顺带证明两侧都注册了分组）。还原后 `internal/cli/args.go` sha256 `a02f7cf8…` 逐字一致。
3. **反向 ②（契约那半承重）**：往 `accepted` 塞一条 `{"--records-all"}`（RunE 层会被判错）
   ⇒ 当场红，报 `--records-all 需要同时给 --records`，两个 head 都红
   ⇒ **这正是"只留 ①"会漏掉的那种输入**。
4. **反向 ③（例外承重且够窄）**：把例外去掉（`if true`）⇒ 红**只**落在 `--account 3` 那两条
   ⇒ 例外是必要的、也没有多豁免别的。还原后 `internal/cli/args_test.go` sha256 `e76475e8…` 逐字一致。
5. **九条门禁**：改动后首次（冷缓存）**`test` 红**（就是上面那条，2m23s）；
   修完重跑 **9/9 全绿**（39.2s，热缓存；test 29.1s）。

⚠ 顺带修掉一处**重号**：坑表里 **#125 被用了两次**（`auth` 那一轮一条、我这条一条）。
我这条按"接在最后"的约定**让号到 #126** —— 改自己、不动对方的引用（对方的号已被
`docs/auth-refresh.md` §8 引用，改它要多动一份不属于本笔工作的文件）。
同时把"直播/小红花"那一节的标题从十七条**改回十六条**（#125 本就不在那一节里，是我上一轮数错了）。

## 补记（2026-09-22 夜，手动刷新那一支改成"点即刷"；提前量只归批量那条路）

起因是界面上点了"刷新 auth"却回一句"还没到提前量，这一次没打上游"。用户的口径重复了多轮，
结论收成一句：**把提前量从这条路由里拿掉，前端不管这个，后端也不判断，点一下就直接换 token**；
"到点才换"属于批量扫描那一路（CLI `auth refresh --all`）。
⚠ 这一段当时还写着"定时由**外部**触发、这个后端不装调度循环"，2026-09-23 已被用户的
新口径推翻并落地（"调度是需要的"⇒ 循环进服务进程，见本文件末尾那一轮）；
**手动那一支的"点即刷"不变** —— 它拿的是 `force`，与到不到点无关。

### 一、代码落点（每一层都留了反例）

| 层 | 改了什么 | 反向改坏它谁会红 |
| --- | --- | --- |
| `capabilities` | `RefreshAuth` 末尾加 `force bool`：只有 `not-yet` 这一档可被越过，`manual-only` 越不过；`Decision` **照实**返回，不新增第四个对外码 | `TestForceCrossesTheLeadWindow`（剩 30 天 + force ⇒ 打 1 次且 `decision` 仍是 `not-yet`；`expire<0` + force ⇒ 0 次 0 写库） |
| `app` | `RefreshDeps.Force`；`AuthRefresher.Refresh` 签名加末位 `force` | `TestAuthRefresherForceIgnoresTheLeadWindow`（同一份 45 天库存两侧对钉：给 force 打 1 次并写库 / 不给 ⇒ `requests==0 && rotations==0`） |
| `server` | 这条路由**没有提前量入参**：body 只剩 `platform`，固定 `force=true`；`leadDays` **收而不理**（老缓存里的 JS 还会带它，判 400 就变成"点了说参数不合法"） | `TestRefreshAuthRotatesEvenBeforeTheLeadWindow`（原 `…SendsNothingBeforeTheLeadWindow` 反向改名重做）、`TestRefreshAuthIgnoresALeadDaysField`（`leadDays` 0/-1/7 都 200 且各打 1 次；`platform:"android"` 仍 400） |
| `cli` | 参数面加 `--force`（`--lead-days` 保留给批量那一路）；结果行多一档"按 --force 换的：还剩 N 天，本未到提前量" | `TestAuthRefreshForceCrossesTheLeadWindow` |
| `web` | 弹窗**只判已过期 / 未过期**，不打提前量；`REFRESH_DECISION` 三档表换成 `REFRESH_NOT_ROTATED`（只留没换成时那两句） | 网页侧没有测试运行器（AGENTS §1.6）⇒ 只有 typecheck + build |

### 二、⚠ 一次"把没测的写成断言、还归因给用户"（坑表 **#125**）

本日我自己造了一句"刷新会顶掉这个端当前正登着的会话"，写进了 `internal/app/auth_refresh.go`
的 `Force` 注释、`docs/auth-refresh.md` 和弹窗措辞。实况是**当天一次真上游刷新都没打过**。
被用户质问"你真的测试过吗，还是瞎说的"之后，我又把它说成"**最早出自你上一条消息**"——
逐轮回查 `*.jsonl` 的 user 轮才定性：「会踢下线」与反向的「不影响其他端」**两句都不在**用户任何
一轮消息里，全部出自我的输出。三处已改：注释只陈述核实过的那件事（覆盖库里三列，窄写门禁断言
其余列不动），风险回到 `❓ 未实测`；`docs/auth-refresh.md` §8 的耻辱柱补上"归因"这一层；判据进坑表 #125。

### 三、顺手：帮助文本里的反引号（坑 #115 的**既有**第二处）

新加的 `--force` 帮助文本带着反引号，于是把 #115 那条判据从"盯 live-room 一张 help"**扩到整棵树**
（`TestNoFlagHelpTextUsesBackticks`）。它当场抓到一处**本轮之前就存在**的：
`internal/cli/args_login.go:135` 的 `smid reset --account` ⇒ 实测 `--help` 印出来是
`--account mcloud smid show   账号 id（…）`，正确形状是 `--account string`。已去掉反引号并复跑确认。
⚠ 判据读的是**注册好的 flag 文本**而不是跑每张 help：跑 help 要为每个叶子造可跑的库与参数，
正是 #116 那份手写清单烂掉的形状。

### 四、门禁与实况（照抄当轮 stdout）

- `node scripts/gates-go.js` ⇒ **九项门禁全绿**，最后一次总耗时 2m12s（逐条：build 2.1s，vet 0.5s，
  fmt 0.1s，test 2m00s，deps 8.3s，filenames 0.2s，idents 0.5s，golden 0.1s，jsontags 0.1s）。
  同一轮先前那次热缓存是 25.3s（test 19.3s）—— 差在 `go test` 的构建缓存，不是判据变了。
  ⚠ 中途有一次 `go test ./internal/server` **build failed**，缺的符号（`twoShotFailingClient` /
  `containsString` / `collectStoredAccounts`）全在**别人未跟踪的** `runs_test.go` 里 —— 按 §2.7 只归因不动手，
  等对方落定后复跑，我那 9 条 `TestRefreshAuth*` 用例此时才真跑过（全部 PASS）。
- `node scripts/gates-web.js` ⇒ **2/2 全绿**（typecheck 2.2s，build 1.0s）。
- **未提交**：工作树里有别人 staged 的 live-room 那一整片（`args_live.go` / `run_live_room*.go` /
  `args_test.go` / `cli.go`），按 §2.1 不替在飞的改动定格 commit；本文件只点名自己那几份。
- **8788 上跑的还是上一版**（PID 82300，19:34 那个二进制，仍会判提前量）⇒ 现在点还是会被拦。
  新版在 `target/mcloud-server.new`（21:46）。用户 2026-09-22 拍板**先不换、自己挑时间** ⇒ 未 kill 那个进程。
- ⛔ 仍然**零次真上游刷新**：`rotated=true` 那条界面分支只在假上游下验过；PC 支形状至今无抓包。

---

## 补记（2026-09-22 夜，第 10 条门禁 `md-tables` 落地 + 门禁条数全仓同步）

### 一、新门禁本体

`scripts/check-md-tables.cjs`（新增，**仓库级**；扫描面见 §五，现 **35 个文件**）。
判据 = 源文件里"某行单元格数 **>** 表头格数"（多出来的格渲染时被静默丢掉，见 **#124**）。
两条取舍（为什么"只拦一个方向"、为什么那 16 行改成豁免）写在 §九 那一节，此处不重复。

两条**反向测试**（都还原到字节一致）：

- 往 `docs/cli-usage.md` 一张表里注入一处未转义竖线 ⇒ `✗ docs/cli-usage.md:53 表头 2 格 / 本行 3 格（多 1 格）`；
- 往 `EXEMPT` 里加一个不存在的 `#99` ⇒ `✗ 豁免名单过期：docs/migration-pitfalls.md#99`。

### 二、⚠ 它原本**跑不到该跑的地方**（本轮真正的发现）

把门禁加进 `GATE_NAMES` 只保证"**全量跑**时它会跑"。而 `pre-commit` 是**按暂存范围派发**的
（`scripts/gates-scope.js` 三张名单：`ALWAYS` / `GO_STATIC` / `WEB_STATIC`）—— 新门禁
**三张名单里都没有**。表现是反直觉的：**只改文档**的提交（正是它该拦的场景）只跑 `filenames idents`，
而改 `.go` 的提交反而会"顺手"跑到它（因为那一档更宽）。

⇒ 修法：进 `ALWAYS`（恒跑三条）。判据与 `filenames` / `idents` **同构** —— 它扫的是**整个仓库**，
不是"这次暂存的那几个"，所以按范围派发没有意义；而"改一行 `.md` 却不跑文档门禁"正是
`gates-scope.js` 开头那条"**闸口的内容与被改的东西无关**"的**同一个错**，只是方向相反。

**端到端验证**（不是读代码，是真跑钩子）：直接执行 `.githooks/pre-commit`（它读当前暂存区），
逐条里出现 `md-tables 0.2s`；另用 `stdin` 档核了三档范围：

| 暂存内容 | `GATES_GO` |
| --- | --- |
| `docs/a.md` | `filenames idents md-tables`（3/10） |
| `internal/a.go` | `filenames idents md-tables fmt deps jsontags`（6/10） |
| 空 | `filenames idents md-tables` |

### 三、顺手改掉的一处**假命令**

`docs/go-migration-plan.md` §5.1 那张"与 Rust 侧七条的对应关系"表里，第 5/6 行的 Go 侧写的是
`bash scripts/check-go-deps.sh` / `bash scripts/check-filenames.sh` —— 而 **bash 版 2026-09-22 已删**
（`ls scripts/` 里只剩 `.js`）。既然本轮要往同一张表加第 10 行，就一并改成真名，并加一条说明：
§5.1 的命令名是**迁 zx 之后**的；在 `status.md` / 坑表里读到 `bash scripts/gates-go.sh` 这类写法
是**当时**的记录，**不是现在能跑的命令**。

### 四、门禁与实况（照抄当轮 stdout）

- `node scripts/gates-go.js` ⇒ **十项门禁全绿**。最后一次 **2026-09-23 07:39**，总耗时 **10.7s**
  （逐条：build 1.7s，vet 0.4s，fmt 0.1s，test 1.0s，deps 3.8s，filenames 0.3s，idents 1.5s，
  golden 1.3s，jsontags 0.4s，md-tables 0.2s）；07:34 那次是 26.7s（差在 `go test` 的构建缓存，
  不是判据变了）。⚠ **带时刻抄** —— 理由见下面那两条。
- ⚠ 同一晚早些时候它跑成 **9/10**，唯一红的是 `jsontags`：`internal/app/signin_run.go` 的
  `SignInRun` / `SignInAccountResult` **整个没有 `json` tag 且未登记**。**不是本轮改动引起的** ——
  该文件 **23:25:04** 才出现（`git status` = `??`，从未提交），比本轮任何改动都新；而
  `scripts/check-go-jsontags.cjs` 本身也正被对方改（`MM`，未暂存那半正是登记
  `live_room_task.go` / `live_room_probe.go`）⇒ 按 **§2.7** 只归因、**不动手**（改那个脚本
  会撞上对方正在写的同一段）。
  ⚠ 现在绿了是**对方自己登记完毕**（白名单现 **115** 条），不是我改的。记这一笔是为了说清
  "绿只对**跑的那个版本**成立"这句话反过来也成立：**红也只对那个版本成立** ——
  抄的时候必须连"哪一轮、什么条件"一起抄，否则过一会儿就分不清。
  ⚠ 顺带说清它**不是误报**：门禁要的是"**显式登记 + 写明理由**"，而 `SignInRun` 装的是
  `Store` / `Upstream` / `Clock`（根本不可序列化）、`SignInAccountResult` 出网走 `RunReport()`
  —— 登记是对的，**漏登记就该红**。
- ⛔⭐ **"全绿"必须带时刻 —— 这一节自己就是证据**：同一个晚上，同一条命令在本仓库先后跑出过
  **9/10**（`jsontags` 红，起因是对方在飞的 `signin_run.go`）与 **十项门禁全绿**
  （2026-09-23 07:34 复跑，26.7s）两种结果，中间还夹着下面那条"23:31 那次绿只有 4 分钟有效期"。
  ⇒ 只写"本轮全绿"在下一个人眼里**就是假的**；抄进本文件必须连**时刻 + 跑的范围**一起抄
  （与下面那条口径同一条，不另立）。
- **未提交**：本轮只 stage 自己的 **10 份**（`scripts/check-md-tables.cjs` 新增，`gates-go.js` /
  `gates-scope.js` / `.githooks/README.md` / `.githooks/pre-push` / `docs/dev-gates.md` /
  `docs/README.md` / `go-migration-plan.md` / `README.md` / `AGENTS.md`）。
  ⚠ `.githooks/pre-push` 里那句**横幅**（"推送门禁全绿：Go **十**条 + web 两条"）也算在内 ——
  它正是会被抄进本文件的那种句子，条数错了就再也说不清。
  `docs/status.md` 与 `docs/migration-pitfalls.md` 是 `MM`（工作树里有对方的在飞改动）⇒
  本轮追加**留在工作树、不 stage**。
- ⚠⚠ **23:31 那次绿只有 4 分钟有效期**：本条目写完、复跑门禁时（23:34–23:35）`build` / `vet` / `fmt` / `test`
  **又红了**，红的仍是 `internal/server/` 那一包，而**报错内容每次都不一样**
  （第一次缺 `sync` / `upstream` 两个 import，第二次变成 `liveRoomTask` 在 `runs.go` 与
  `run_dispatch.go` **重定义**、`capabilityLiveRoom` 未定义）。两份文件的 mtime 就在这一刻**还在往前走**
  （`runs.go` 23:34:43、`run_dispatch.go` 23:34:17）⇒ 这是**另一个会话正在把 `liveRoomTask` 从
  `runs.go` 搬进 `run_dispatch.go`、旧的那份还没删**的中间态，不是本轮改动引起的。
  按 **§2.1 / §2.7** 只归因、**不动手**：搬动别人正在改的文件 = 直接撞车。
  ⭐ 由此得一条口径：**共享工作树里"门禁绿"这句话必须带时刻与包范围**，
  只写"本轮全绿"在下一个人眼里就是假的（上面第四节已照此改成"照抄当轮 stdout + 时刻"）。

### 五、⚠ 自查又发现一处：新门禁的**扫描面**也是手写清单（坑表 **#131**）

§二 解决的是"它会跑"，**没解决"它扫哪些文件"** —— 那一份当时手写成
`AGENTS.md` + `README.md` + `docs/**`。把全仓的 `.md` 数一遍才发现漏了两份**手写且入库**的：

| 落在清单外的文件 | 为什么该在范围里 |
| --- | --- |
| `THIRD-PARTY-NOTICES.md` | 顶层、手写、入库的文档 |
| `.githooks/README.md` | 手写文档，而且**本轮我刚在里面改过表格** |

⇒ 这两份**从没被扫过**，而输出里那句"33 个文件通过"对它们是假的。与 **#92**（`ORDER` 抄一份 ⇒
加了门禁但默认不跑）、**#93**、以及 `TEXT_EXT` 漏过 `.go`/`.py`/`.tsx`/`.csv` 是**同一族**：
**手维护的清单，失效方式永远是"漏掉的不自报"**。

**改法**：扫描面换成 `git ls-files --cached --others --exclude-standard '*.md'`
（与 `check-filenames.js` 同一约定）。于是 `.gitignore` 成为"什么算仓库内容"的**唯一**出处 ——
而 `.gitignore` 里那两条注释本来就写着"忽略 = 也不再被守卫扫"（那是给 `check-identifiers.cjs`
加的）。⚠ **别改回 fs 遍历**：那会把 `.workbuddy-ai/` 的记忆文件重新拉进扫描面，正好把那条
已经修好的事又弄坏。第三方技能目录（**73 个** `.md`）按 `VENDOR_DIRS` 排除，**个数打印在输出里**。

**三条反向测试**（前两条都还原到字节一致）：

| 注入 | 期望 | 实测 |
| --- | --- | --- |
| 新建顶层 `zz-md-probe.md` 含坏表格（未跟踪、未忽略） | 红 | 红在 `zz-md-probe.md:5` —— **旧的手写清单会静默放过它** |
| 往 `.githooks/README.md` 注入一处未转义竖线 | 红 | 红在 `.githooks/README.md:74`（证明它真进了范围）；还原后 sha256 `06fe23e9…5657` 一致 |
| `GIT_DIR=/nonexistent` 跑门禁 | 退 1 | `✗ 拿不到仓库文件清单（git ls-files 失败）` + `fatal: not a git repository` |

**结果**：`33 → 35 个文件`，两份新进范围的都干净。⚠ 这条判据已登记为坑表 **#131**，
并同步了 `AGENTS.md` §0 的编号区间（`#1–#131`）与 skill 的索引。

## 补记（2026-09-22 夜，WebUI「领取小红花」那一轮：探针八枪 + `POST /api/runs` + 页面落地）

按用户的取舍顺序做：**先领取小红花，兑换之后再说**；本轮只到"批量投口令 + 看结果"，
兑换页与"自动抓取 + 定时任务"都**没动**（当时 `GET /api/status` 还恒报
`schedulerRunning:false`，页面文案里就明说了这一条链要定时任务才自动；
2026-09-23 调度循环接上后那句已过期，实况看本文件末尾那一轮）。

### 一、这一轮真正的收获是**两个凭据域**（判据 #128）

- 六步链与余额（`liveRoomFeedback/*`）要的是**云盘侧** `jwttoken`（`tyrzLogin` 那一份），
  不是门换到的直播侧 JWT。第一版按"听起来像直播"分派 ⇒ 实机两个账号都回 `90001 未登录`。
- ⚠ 埋点 `visitlog/journaling` **也**带 `jwttoken`：实拍 §5.1 那句"公共请求头（六个请求全带）"
  里就含它，它同在 `/ycloud` 域内。这一条是 `TestLiveRoomRunAccountSixStepsUseCloudJWT`
  **按头**断言抓出来的 —— 只看"请求发出去了"的断言对两种写法**都是绿的**。
- ⇒ 探针（`GET /api/accounts/{id}/live-room`）实际是**八枪**：门三步带 `Authorization`，
  云盘侧建会话三步 + 余额一带 `jwttoken`。CLI / 任务 / 探针三条入口共用同一个顺序（共用分派那条）。

### 二、WebUI 侧补上的两个缺口

1. `POST /api/runs` 之前**不存在**：后端早就有执行面（`app.Runner`），WebUI 却只能读状态。
   现在：202 先回（`{capability, runs:[{accountId, runId}]}`，`runId` **预分配**）+ 每账号一条 SSE。
2. ⭐ 事件流那条 **404 窗口**（判据 #129）：`runs` 行由后台 goroutine 在**轮到那个账号时**才写
   ⇒ 串行批次第 2..N 个必然有一段"ID 已发、行未落"，而 `EventSource` 对 404 **按规范不重连**
   ⇒ 卡片永远停在"连接中断"，那趟其实跑完了。修法是**前端**在 `phase==='pending'`（一条事件未收）
   时按 1s 退避重订阅（上限一小时）；**没有**把 `InsertRun` 提进 202 处理 —— 那是尊重既有那条
   "被互斥挡下的账号不写 runs 行"的选择，而前端这一支顺带覆盖了其余几种无记录窗口。
   测试侧 `waitForRunFinished` 同样改成**轮询穿过**窗口（此前它是**随机红**：每轮红的用例不一样）。

### 三、真机核验（web 侧没有测试运行器，这一节就是它的测试）

服务：`127.0.0.1:8794`（自己的二进制 `target/mcloud-server-live`、自己的**库副本** `target/verify.db`）
—— 没占 8788、没碰 `data/mcloud.db`。

| 形态 | 判据 | 结果 |
| --- | --- | --- |
| 两个账号跑完 | 两卡都出日志与结论，`235 → 235` / `110 → 110`（不存在口令 ⇒ 零消耗，与实拍 §5.1 同向） | ✅（改前第二卡是"连接中断"） |
| 窄屏 375×740（同源 iframe 量，内置浏览器改不了尺寸） | `scrollWidth == clientWidth == 375` ⇒ 无横向溢出；两张 `table` **都隐藏**、移动卡片显示；主按钮 `重新看` / `开始领取` **44px** | ✅ |
| 确认弹窗（375） | 文案：**每场直播全世界共用那两个口令，而兑换状态按账号算** —— 同一条口令投给两个不同账号，两边都是各自的第一次，不会互相挡；只有**同一号**把同一条再投一次才回「该口令已兑换」 | ✅（⚠ 初稿写的是"同一口令在第二个账号上多半回已兑换"，**23:50 被用户第二次纠正**，判据见 #131） |
| 桌面 1280×900 | 房间日程表 8 行、结果/历史表 4 行**都显示**；`md:hidden` 那两块（含 dock）**隐藏**；按钮回到 daisyUI 尺寸（32 / 40） | ✅ |
| 深色 | 语义色全部跟着换（`body` `lab(10.9…)` vs `card` `lab(13.3…)`、正文近白）；⚠ 全仓 `web/src` **没有**一条 `bg-gray-*` / `text-white` 这类锁色调的类 | ✅ |
| 界面文本 | 探针默认看**第一个启用中**的账号（此前"优先手机端"那条理由已被证伪并改掉）；账号选择里那个「PC 端」警示 badge **撤了** —— 平台标签与这条链无关 | ✅ |

⚠ 顺手抓到一个只有真机看得见的漏（判据 #130）：文案里写了 Markdown 的 `**无关**`，
页面上就原样印着星号 —— typecheck 与 build **全绿**。已改成 `<b class="font-medium">`。

### 四、门禁与实况（照抄当轮 stdout）

- `node scripts/gates-go.js` ⇒ `门禁未通过: jsontags`，**9/10**，总耗时 **34.3s**
  （逐条：build 2.3s，vet 0.6s，fmt 0.1s，test 27.8s，deps 3.0s，filenames 0.1s，idents 0.2s，
  golden 0.1s，jsontags 0.1s，md-tables 0.1s）。
  ⚠ 红的那条与本轮**无关**，归因见上一节补记（别人未跟踪的 `internal/app/signin_run.go`）——
  本轮**没有**"十项门禁全绿"这句话可抄。`build` 与 `test` 都过，本轮改的包（`app` / `server` / `cli` / `web`）全绿。
- `node scripts/gates-web.js` ⇒ **web 门禁 2 条全绿**，总耗时 **3.3s**（typecheck 2.4s，build 0.9s）。
- ⛔ **未提交**：工作树里有对方的在飞改动（`internal/app/signin_run.go` 未跟踪、
  `scripts/check-go-jsontags.cjs` 为 `MM`），且 `jsontags` 正红 ⇒ 本轮成果**留在工作树**，
  等对方落定再按 AGENTS.md §2 只点名自己那份文件提交。

### 五、遗留（本轮**没做**、也不该顺手做）

1. **兑换页**（用户明说"之后再说"）：`prizepool/list` 那 46 件与 `200103`（云朵不够）都已实拍，缺的是界面。
2. **自动抓取 + 定时任务**：口令**当场有效**（`200108` 原文），所以定时任务的合理形态是
   "开播后尽快跑"，不是"每天固定点跑"（当时 `schedulerRunning` 还恒 `false`；
   2026-09-23 起循环真在跑了，而 `live-room` 的默认正是**错过不补**，与这句判断同一头）。
3. ⚠ 全站那条 §7.4「移动端按钮 ≥44px」与现实有差：**共享**的 `ConfirmDialog` 按钮静止 **40px**
   （此处原写 38px：那是**弹窗开场那几帧**读到的，`modal-box` 带 0.95 倍缩放，2026-09-23 重量后改准，
   结论不变），daisyUI 的 `select` / `number` 输入 **40px**（本轮只把自己页的主按钮补到 44px：`min-h-11 md:min-h-0`）。
   这属于**规范 vs 现实**，改共享组件影响整站 ⇒ 按 AGENTS.md §4 提请用户拍板，不在本轮里静默统一。
4. ⚠ 成文里带尾号的掩码账号（`15****42` / `19****16`）本轮已在**自己这三份**里去掉尾号
   （改成 `15****` / `19****`）。⚠ 另有三份**已提交**的文件仍留着尾号形态：
   `docs/capture/app-sms-login-samples.txt`、`internal/app/migration_test.go`、
   `internal/io/legacy_import_guards_test.go` —— 后两份是测试里断言的显示掩码，
   改它等于改行为（要过 golden 门禁），按 §2 只报告不动手。

## 补记（2026-09-23 凌晨，调度循环进服务进程：S1–S5 那一轮）

推翻的旧口径是"**这个后端不装调度循环、定时由外部触发**"（上面 2026-09-22 那一轮还写着它）。
用户原话：**"调度是需要的，能力也是在待办里的"**，随后四项拍板：

| 拍板 | 结论 |
| --- | --- |
| 循环住在哪 | **服务进程内**（`mcloud-server` 起停，`schedulerRunning` 随之变真） |
| 后台触发以谁的身份 | **按 `schedules.owner` 反解 scope**；无主行不触发（既避免 `ScopeUser(nil)` panic，也不越权） |
| 错过的要不要补 | **"必须准时的不需要补，其他的需要"** ⇒ 每行一个策略位，缺省跟能力声明（准点型 `skip` / 日型 `catch_up`） |
| 本轮范围 | **循环 + 通用执行分派**（`schedules.target` → `RunPlan` + `RunTask`；`sign` 也接上执行） |

### 一、落点（每层都留了反例）

| 层 | 改了什么 | 反向改坏它谁会红 |
| --- | --- | --- |
| `core` / `io` | schema **v9**：`schedules.miss_policy` + `last_fire_at`；`core.MissPolicy` 一族（**无 Rust 基准**，是本地契约）；`MarkScheduleFire` 只碰水位，`RecordScheduleRun` 的 `ranAt=nil` 与 `lastError=nil` **两个 nil 不对称** | `io` 那侧的窄写断言 + `scripts/check-golden-vectors.cjs` 不含这一族（本地契约，不进对拍面） |
| `capabilities` | 每条能力显式声明 `MissPolicyOnMiss`（`sign` / `cloud-snapshot` / `auth-refresh` = catch_up；`exchange` / `live-room` = skip）；`ResolveMissPolicy` 按"离人更近的优先"：行覆盖 > 能力默认 > `MissSkip` | `TestEveryCapabilityDeclaresMissPolicy`（漏声明就红）、`TestScheduleRowOverridesTheCapabilityDefault` |
| `server` 分派 | `runBuilders` 三条（`live-room` / `sign` / `auth-refresh`）；`launchRun` 加**完成回调** `done func(app.RunSummary, error)`，HTTP 那一路传 nil | `TestRegisteredButUnwiredCapabilityIsNotImplemented`（回 202 会让前端订阅一条永远没事件的流）、`assertRunCapability` |
| `server` 循环 | `internal/server/scheduler.go`：30s tick + 60s 宽限；先跑一轮再起 ticker（重启不白等）；`ScopeLocal` **只**用来发现每行的 owner，之后读写全回该行 scope | `scheduler_test.go`（判点 / 补不补 / 水位先于起跑 / 三列记账时机，共 13 条） |
| `server` 状态 | `getStatus` 的 `schedulerRunning` 从常量改成 `Server.SchedulerRunning()`（`atomic.Bool`，循环自己起停） | `TestStatusReportsTheSchedulerState`（**两个方向**都钉：没起⇒假、起了⇒真；只测单向就等于把常量改回去也过） |
| `web` | 侧栏那句「定时调度未启用」只在真没跑时出现（逻辑本就派生自这个字段）；直播页那句"后端不会自己触发"改成实况 + "还没有任务页" | ⛔ 网页侧没有测试运行器（AGENTS §1.6）⇒ 只有 typecheck + build，加下面那次真机核验 |

### 二、三条容易写错的判据（都在 `scheduler.go` 文件头）

1. **水位先于起跑**：`MarkScheduleFire` 失败就不打上游。反过来的话，一次"上游已经收了口令、
   记账却失败"会在下一个 tick 再投一遍 —— 而投口令是**写操作**（会动这个账号的兑换状态）。
2. **错过的整串一次折叠**：错过时把水位抬到 `now`（不是那个点），**不回扫 backlog**。
   每秒一条 cron 停机一天，回扫就是八万六千次 `NextFire`。
3. **三列各有各的写入时机**：`last_run_at` 只在"真跑完一趟"之后写（被挡下、按策略弃掉、
   参数解不出、能力没接上四种都不写）；`last_error` 在 `ok` 时写 NULL 以清掉旧错误；
   `last_fire_at` 只有 `MarkScheduleFire` 能动，而 `UpsertSchedule` 的 `DO UPDATE` 刻意不含它。

### 三、核验

- `node scripts/gates-go.js` ⇒ **十项全绿**（build / vet / fmt / test / deps / filenames / idents /
  golden / jsontags / md-tables），总耗时 6.0s。⚠ 门禁已是**十**条，AGENTS.md §1.1 还写着"九条"，
  那是 `md-tables` 进来之后没跟着改的口径 —— 那份文件正被并行会话暂存，本轮只报告不动手。
- `go test -race ./internal/server/` ⇒ 绿（19.5s）。`node scripts/gates-web.js` ⇒ **2 条全绿**。
- **活体**（本机 8790 另起一份进程 + 一份全新库，**没碰** 8788 那个旧版、没碰 `data/*.db`）：
  `GET /api/status` ⇒ `schedulerRunning:true`；`PUT /api/schedules/probe` 建一条
  每分钟、目标为 `exchange`（**没接上执行**）的任务 ⇒ 下一个 tick 那一行变成
  `lastStatus:"error"` + `lastError:"能力「兑换奖品」还没有接上执行"` + `lastFireAt` 落上、
  `lastRunAt` 保持 null，服务日志与库里都没有任何出站请求痕迹 —— **零上游**是这条探针的选点理由。
- **真机（浏览器）**：登录后取抽屉那一行的 DOM 文本 ⇒ `mcloud v0.1.0` + `尚无执行记录`，
  整页已无「定时调度未启用」。⚠ 直播页那句新文案**没有**真机看过：它只在"这一趟有卡片"时才渲染，
  而探针库里没有云盘账号（要它就得打真上游）。

### 四、遗留（本轮没做、也不该顺手做）

1. **任务组合（`target.kind:"pipeline"`）还没接上执行**：到点记一句 `error`，不静默。
2. **`PUT /api/schedules` 只能"留"不能"设" `miss_policy`**：请求体没有这一栏，
   服务端把库里那一份原样带过去（免得一次编辑洗成跟随默认）。写它要等任务页一起做，
   且必须校验未知文本 ⇒ 400，不能静默变 NULL。
3. ~~**定时任务页仍是空态**：现在建任务只能直接调 `PUT /api/schedules`（本轮那条探针就是这么建的）~~
   ⇒ 2026-09-23 同日那一轮的补记里这一条已落地，见本文件末尾"定时任务页"那一轮。
4. **一份库只跑一个服务端进程**：`MarkScheduleFire` 不是原子认领，两个进程会都判到点。
   已把这条约束成文在 `scheduler.go` 文件头，没有偷偷加锁。
5. 8788 上仍是**旧版**二进制（用户："先不换，我自己挑时间"）；本轮新产物在 `target/mcloud-server`。

## 补记（2026-09-23，定时任务页：`#/schedules` 那一轮）

`docs/auth-refresh.md` §8 待办 #1 落地：这一页从 `PlaceholderPage` 换成真组件，
接 `GET/PUT/DELETE /api/schedules` + 两份配合数据（`/api/capabilities`、`/api/accounts`）与
`/api/status`（调度在不在跑）。**没动 Go 侧一行**：三条路由与循环都是上一轮的成品。

### 一、落点

| 文件 | 内容 | 反向改坏它，谁会红 |
| --- | --- | --- |
| `web/src/ui/cron.ts`（新） | 时刻 ↔ cron 的**映射与描述**（`buildCron` / `readCron` / `describeCron`），**刻意不含校验** | ⛔ web 侧没有测试运行器；判据是"前端一旦复现段数分流 / 日周取或 / 周日 0 与 7，就会与 `core.NextFire` 分叉" ⇒ 文件头把四处风险列成"不许在这里补"的理由 |
| `web/src/api.ts` | schedules 三条路由的类型与调用、能力表类型；顶部记着 `accountIds`（读）/ `account_ids`（写）那处不对称 | 键写成 `accountIds` 时请求**照样 200**、只是把账号列表洗成空 ⇒ 实机核验里"勾一个账号→保存→重读"那一步会红 |
| `web/src/pages/schedules-page.tsx`（新） | 列表（桌面 `table-sm` / 移动卡片 + 底部固定操作条）、建改删、行内启停 | 见下面那张真机表 |
| `web/src/app.tsx` | `schedules` 分支接上，`PlaceholderPage` 链里摘掉 | 路由不识别时整页回落空态 |
| `web/src/pages/live-redflower-page.tsx` | 那句"定时任务那一页还不能建任务"改成实况（建任务去任务页，目标选这一项） | 只有真机看得见（网页无测试） |

三件界面上刻意**不含**的东西，都写在 `schedules-page.tsx` 文件头：下次执行时间是服务端算的、
`PUT` 是全量覆盖（所以翻启停也得整行送回）、`miss_policy` **只显示不给编辑**（请求体里没这一栏）。

### 二、真机核验（web 侧没有测试运行器，这一节就是它的测试）

服务：`127.0.0.1:8794`（`target/mcloud-server` 现编现起 + 一份全新库 `/tmp/mcloud-sched-verify.db`）
—— 没占 8787 / 8788、没碰 `data/*.db`；cron 全落在会话窗口之外 ⇒ **零上游请求**。
账号只有库里那份合成号 `19****`；另手插一行 `pipelines` 专走"目标不可改"那一支。

| 形态 | 判据 | 结果 |
| --- | --- | --- |
| 列表四行 | 频率描述（每天 / 每周日 / 每小时 / 自定义串原样）、下次执行（含"已停用，不会触发"与闰日 `02/29`）、`错过时：错过跳过` 与 `未单独设置` 两样、组合行显示"不提供改目标" | ✅ |
| 建 / 改 | 新建落库并刷新列表；编辑组合行时 `target` 与 `createdAt` 原样保留 | ✅ |
| 参数键名 | 多写一个键 ⇒ 保存被挡，措辞与后端同源（"不认识的参数：…（这一项能填：…）"） | ✅ |
| 行内启停 | `PUT` 整行送回后 `accountIds` / `body` / `createdAt` / **`miss_policy`** 全不动，`nextFireAt` 转 null | ✅（`miss_policy='skip'` 一次翻开关后仍是 `skip` ⇒ "只能留、不能设"这条从界面侧也成立） |
| 删除 | 确认窗写具体任务名 + 连带范围只有一行；按钮动词"确认删除"；确认后计数 4→3、提示"已删除 …" | ✅ |
| 行菜单 | `.dropdown` 靠 `:focus-within` ⇒ 触发器必须 `tabindex=0`；实测聚焦后 `display:flex`、菜单项 **44px**（默认 32px 在窄屏点不中，已补 `min-h-11 md:min-h-0`） | ✅ |
| 窄屏 375×667（同源 iframe 量） | `scrollWidth == clientWidth == 375`；表格隐藏、卡片与底部操作条显示；操作条底边 599 / dock 顶 603 ⇒ 不互相盖 | ✅ |
| 桌面 1280×900（同源 iframe） | 表格 8 列显示、卡片与操作条隐藏、菜单项回到 33px 密度、无横向溢出 | ✅ |
| 两个主题 | 四档 `badge badge-soft`（无 / success / error / warning）对比度：浅 14.7~18.8、暗 **5.11~12.3**，全部 ≥4.5；无 `badge-neutral` | ✅ |

⚠ **没做**的一件事：内置浏览器那一路截图不可用（`NATIVE_BROWSER_VIEWPORT_UNAVAILABLE`），
所以本轮只有 **DOM 度量**（高度、颜色、`scrollWidth`），**没有**一张实拍图。

### 三、本轮的五个纠正（都是"看起来对、其实错"）

1. ⛔ **我误报了一次 Vapor 行为**：先写成"`ConfirmDialog` 的 `impact` prop 会停在第一次那一行，
   实机连着开两次验证过"。实况是**点完立刻同步读 `innerText`** —— Vapor 的更新在微任务之后才落。
   隔 120ms 再读，同一个组件连着换两行都显示正确。⇒ 结论改成"prop 里那片 JSX 的**子节点位置**仍自带 effect"；
   值 prop 会不会跟新**本轮没测**，要状态稳就写成本页元素的属性与子节点。（代码注释里留了同一条耻辱柱。）
2. ⭐ **弹窗高度要等开场那一下缩放过去再量**：`modal-box` 打开的头几帧带 0.95 倍缩放，
   当场读会把静止 **44** 读成 42、**40** 读成 38。⇒ 静止值本轮用"同一份 CSS 里现搭一颗 `.btn`"复现过：
   `btn` / `btn-soft` / `btn-error` 都是 **40px**。上一轮那条"ConfirmDialog 实测 38px"按此改成 40px
   （结论不变：仍够不到 44，仍归"规范 vs 现实留给用户拍板"）。
3. ⭐ 状态 badge 少写基类 `badge`（表里只给了 `badge-soft badge-*`）⇒ 渲染成**无样式的裸文字**，
   而 typecheck 与 build **全绿**。又一例"只有真机看得见"（同族：#130 那句 Markdown 星号）。
4. ⚠ `未知状态 <code>` 那一支在**当前 HTTP 面上到不了**：存储层把不认识的 `last_status` 读成 nil
   （`internal/io/configstore_test.go` 那条判据），界面于是走"从没跑过"。注释已改成"到不了 + 本轮没在界面验过"，
   不再写成一条活的回落行为。
5. ⚠ 页面顶部 `schedulerOn` 的初值取 **null**（不是 `false`）：`/api/status` 回来之前那句
   "定时调度没有运行"会在首帧闪出来 —— 那是**谎报**，也是本轮唯一一处"用类型挡住时序"的写法。

### 四、门禁与实况（照抄当轮 stdout）

- `node scripts/gates-web.js` ⇒ **web 门禁 2 条全绿**，总耗时 **17.0s**（逐条：typecheck 13.5s，build 3.5s）。
  ⚠ 中途一次红的不是本轮：那时并行会话刚落下未跟踪的 `web/src/pages/exchange-page.tsx`，
  两条 `TS6133`（声明未用）在**它那一份**里，本轮文件干净；等对方改完再跑就绿 —— 按 §2 只报告不动手。
- Go 侧本轮零改动，但 `docs/` 动了（`md-tables` 是 Go 那十条里的一条）⇒ 全量重跑：
  `node scripts/gates-go.js` ⇒ **十项门禁全绿**，总耗时 **26.5s**
  （逐条：build 6.0s，vet 1.7s，fmt 0.7s，test 5.3s，deps 11.0s，filenames 0.7s，idents 0.6s，
  golden 0.3s，jsontags 0.1s，md-tables 0.2s）。
- ⛔ **未提交**：工作树里另有对方的在飞改动（`web/src/pages/*`、`internal/**` 多处在改），
  按 AGENTS.md §2 只点名自己那份文件、等对方落定再谈提交。

### 五、遗留（本轮没做、也不该顺手做）

1. `miss_policy` 的**写入与校验**（`auth-refresh.md` §8 待办 #2）⇒ 打通后要一并补那个下拉。
2. `pipeline` 目标接上执行（同 #3）⇒ 界面上这一类仍是"只显示、保存时原样送回"。
3. **到点真跑一次真上游**至今没发生过（`auth-refresh.md` §8 前三条不变）⇒ 这一页列出的
   "下次执行"到点之后长什么样，仍只在 Go 测试的假上游里验过。
4. 弹窗 `busy` 那类**值 prop** 是否随父更新：没测。本页不依赖它（禁用与文案都写成本页元素的属性/子节点）。
5. dock 那几枚（33px）、主题切换（32px）、抽屉按钮（40px）仍在 44 以下 —— 那是**全局件**，
   与 §7.4 那条一起留给用户拍板，本轮没自改判据。
