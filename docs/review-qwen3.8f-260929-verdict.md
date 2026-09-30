# 复核意见 —— 对 `review-qwen3.8f-260929.md` 的逐条裁决

> 复核日期 **2026-09-30**，复核基线 `HEAD = 5c564c9`（拆 Vue 兼容层之后）。
> 原 review 写于 **2026-09-29**，隔了一天仓库又被改过一大轮 —— 所以它的结论里
> 有一批**当时成立、现在过期**。裁决词表：
>
> | 裁决 | 意思 |
> |---|---|
> | **成立** | 我在当前 HEAD 上亲眼看到证据，问题还在 |
> | **已过期** | 当时大概是对的，但当前 HEAD 已经不是那样了（给出是谁改的） |
> | **不成立** | 复核实测与结论相反 |
> | **部分成立** | 主体对、细节错（坐标漂移、数量对不上） |
> | **无法判定** | 需要真机/上游读数，本轮拿不到 |
>
> 方法：能跑的都真跑（`grep` 计数、`git log -S`、`node web/lite/size.mjs`、
> `gates-web.mjs --full`、无头 Chrome 打 DOM），不采信"作者自称 ★ 复现过"。
> 三条硬约束：**不跑 `cargo build` / `cargo test --workspace`**（会链接覆写
> `target/debug/mcloud-server` ⇒ SIGKILL 用户的 launchd 服务 + TCC 弹窗）。

---

## 0. 总评

**这份 review 值得留档**，理由不是"它全对"，而是它做对了三件一般 AI 审读不会做的事：

1. **每条都给 `文件:行号`**，且区分"逐行/跑命令复现过（★）"与"分路审读转述" ——
   证据强度是可复核的，不是一片形容词；
2. **单列 §4「被推翻的结论」**，包括**它自己的假阴性**（`key={` 扫法漏了字符串形态
   `key="setup"`）。自我证伪比结论本身更值钱；
3. **主动划出 §6「未证实的格子」**（双同名头、`90001`、TLS 套件、`code` 表……），
   明确说"别当结论用"。这在"AI 综述"里极罕见，也正好是本仓
   `docs/conventions.md` 要求的"失败关闭"气质。

我**拒绝**的部分集中在两类：**过期**（隔天拆了 Vue 兼容层，§1.1/§1.2/§1.4 大半作废）
与**少量事实错误**（把 `tracing` 的 `data_dir="data"` 说成没有引号；把"触控 10 处"
写成 16 个引用）。**其余我认可，并已按"能撤就改"落了 7 条**（见 §4）。

---

## 1. Web 部分

### 1.1 到期作废的（原 review 的最重两条都在这）

| 原结论 | 裁决 | 证据 |
|---|---|---|
| §1.1 默认运行时是 lite，类型门却还是 Vue 的 | **已过期** | `web/tsconfig.json:13-17` 已无 `jsxImportSource`（改 `jsx: preserve` + `lite/jsx.d.ts` + `paths.lite`）；`vite.config.ts:27-29` 只剩一条 `lite` alias；`package.json` **无 dependencies**；`grep "from 'vue'"` 零命中；门禁从 2 条扩到 4 条 |
| §1.2 ★ **阻断**：流水线页步骤列表增删/改序不上屏 | **已过期（当时成立）** | `5c564c9` 修复：`stepsRev` 计数器删除、三个函数改成整数组替换（`pipelines-page.tsx:259/264/270`）；`compiler.ts:199-203` 对非 `.map` 位置的 `key=` **抛错**（不再静默丢）；`regress/interact.mjs:159-167` 补了「添加/上移/移除步骤」断言 |
| §1.3 `gates-web.mjs` 依赖 cwd ⇒ 假失败且零日志 | **已过期** | `gates-web.mjs:30/67/78` 全部改成 `import.meta.url` 与显式 `cwd` |
| §1.3 `signal.ts` 循环更新无防护 / effect 不释放 | **已过期** | `signal.ts:67-68/103-106` 重入上限、`:217-233` drain 轮次上限、`:41-52` + `:191-195` 作用域与 `ownedEffects` 释放（`5c564c9` +148 行） |
| §1.3 `interact.mjs` 收异常却不判负 | **已过期** | `interact.mjs:326-333` `bad += pageErrors.length`，`:340` 判负 |
| §1.3 体积宣称自相矛盾（1,892 / 1,514） | **已过期** | 表头已改 **2,808 / 2,757**，`:227-231` 专段交代"旧值是还带 Vue 壳时的读数"。我实跑 `size.mjs` 得同值 ✓ |
| §1.4 非 `.map` 位置 `key=`（7 处） | **已过期** | `grep 'key="' web/src` 零命中；`app.tsx` 相关注释已改对 |
| §1.4 `ref` 内原地变异喂渲染 | **已过期** | `grep -E '\.value\.(push\|splice\|sort\|reverse\|shift\|unshift)\('` 零命中 |
| §1.6 "regress 双运行时逐字符 + 逐像素对比" | **部分成立** | 那段机制本身已被删（`lite-postmortem.md:179` 有交代），其余称赞项（SSE 生命周期、`isAuthActionFailure`、颜色查表、原生 `<dialog>`）都还在 |

### 1.2 我认可、**已修**的（本次提交）

| 原结论 | 裁决 | 我做了什么 |
|---|---|---|
| §1.3 ★ `schedules-page` 把"手机号"当账号 id | **成立，且原 review 漏了放大器** | legend 改「账号（账号 **id**，逗号分隔）」并写清为什么；列表渲染改走 `accountName()`（原来直接 `join('、')` 摆哈希）。**放大器**是 `schedules-page.tsx:164` 的 `openEdit` 会把哈希回填进那个标着"手机号"的输入框 —— 照文案填手机号 ⇒ 匹配 0 个账号 ⇒ **定时任务静默不跑**；编辑一次 ⇒ 哈希进入本该填手机号的位置，越改越坏 |
| §1.3 `api.ts` 全前端无请求超时 | **成立** | `request()` 加 `AbortSignal.timeout(30s)`（调用方自带 `signal` 时尊重调用方），并把 `TimeoutError` 翻成「请求超时（30 秒没有响应），请重试」。**动机写进注释**：`busy/submitting/saving` 都在 `try/finally` 复位，悬挂 ⇒ `finally` 永不执行 ⇒ 按钮永久禁用，只能刷页 |
| §1.3 `account-names.ts` 的 `names` 非响应式 | **成立** | `new Map()` → `ref(new Map())`，并改成**整张新 Map 赋值**（内建 Map 原地 `set` 不触发信号）。症状"表格长期显示 16 位哈希、切页又好了"正是并发 `refreshAccountNames()` 后到时渲染已结束 |
| §1.3 `auth-signal.ts` 的 `authEpoch` 是死状态 | **成立** | 删除（声明 + 自增两处；全仓零读取方），原地留一段说明"死状态 + 像模像样的注释比没有更危险"。页面重拉数据实际靠 `authState` |
| §1.3 `index.html` boot 动画无失败兜底 | **成立** | `index.html` 内联脚本加三段兜底：`error`（capture，收得到 chunk 404 这类资源错误）、`unhandledrejection`、12 秒超时；判据是 `#app` 有无子节点（挂上了就什么都不做）。**实测**：把主 chunk 改成不存在的文件 ⇒ DOM 里出现"页面没能加载 / 脚本加载失败"；线上 3000 ⇒ `#app` 正常渲染、`#boot` 已移除、兜底文案未触发 |
| §1.4 `.map` 缺 `key`（`exchange-page` 两处） | **成立** | 补 `key={a.id}` / `key={prize.prizeId}`（原来是索引兜底 ⇒ 整表重建） |

### 1.3 我认可、**但本轮没修**的（按"能不能撤"排了序）

| 原结论 | 裁决 | 为什么先不动 |
|---|---|---|
| §1.3 显示链 8 份内联 `nickname \|\| label \|\| id` | **成立** | 建议做法（导出 `pick` 替 7 处）是纯重构，动 7 个页面却在界面上零变化 —— 收益比不划算，等有别的理由碰这些文件时一起做 |
| §1.3 `api.ts` 契约缺口（`RunRecord` 缺 `status`、`RunEvent` 缺 `Details`） | **成立** | 加类型成员本身 1 行，但 `RunEvent` 是联合类型，两页的 `switch` 若带穷尽检查会连带报错 ⇒ 属于"补类型 + 补 UI 分支"一起来的一笔 |
| §1.3 触控目标 ≥44px | **部分成立** | 约定确在（`app.tsx:202`），但 review 说"10 处"却列了 16 个引用；被引处多是 `btn-sm`，要统一补 `min-h-11 md:min-h-0` 面很广，且需要真机点按验收 |
| §1.5 四个"上帝组件"与三态加载样板 ×9 | **成立** | 结构性重构，属独立一笔 |
| 新发现：`new URL(import.meta.url).pathname` 9 处 | **成立** | 路径含空格/非 ASCII 会坏、Windows 上是 `/C:/…`。但 CI 里跑它的只有 ubuntu（Windows 那台只编 Rust、用现成 dist）⇒ 当前无实际影响，记为"将来支持 Windows 开发时再改 `fileURLToPath`" |
| 新发现：`dialog.tsx:62` 等 4 处仍把机制归因 Vue/Vapor | **部分成立** | 其中 3 处已是明确的"历史备注"（`main.ts:5`、`auth-page.tsx:99`、`live-room-page.tsx:649`）✓ 只有 `dialog.tsx:62` 是以现在时说的，需要改写 |

### 1.4 我**拒绝**的

| 原结论 | 裁决 | 证据 |
|---|---|---|
| §3.5#12 "`usage.md:41` 日志样例 `data_dir="data"`，实际输出 `data_dir=data`" | **不成立** | 实跑 release 二进制：`data_dir="/tmp/logprobe"` —— `tracing` 对 `&str` 走 `Debug`，**一定带引号**。文档写的是对的 |
| §1.5 "三态加载样板 ×8 页" | **部分成立** | 实数是 **9** 页；且 review 把 `live-room-page` 850 行写成 846、`pipelines` 841 写成 855（我在当前 HEAD 量的是 846 / 855）——不影响结论，但坐标与计数确实漂了 |
| §1.3 "触控目标 10 处违背" | **部分成立** | 列了 16 个引用，与"10 处"对不上 |

---

## 2. Rust 部分

### 2.0 先校准可信度

我对 Rust 侧做了**一次便宜但独立的抽查**（全部 grep/read 级），挑的都是 review 标 ★ 的：

| 抽查项 | review 的主张 | 我看到的 | 结论 |
|---|---|---|---|
| `Error::AuthExpired` | "全仓零构造点" | 只出现在 `core/error.rs:17`（定义）、`:70`（`is_auth_failure` 的 match 臂）、`server/error.rs:27`（另一个 match 臂）—— **没有任何构造** | **成立** |
| `is_auth_failure()` | "零调用者" | 全仓只有 `core/error.rs:68` 的定义 | **成立** |
| `EventHub` 的 `forget`/`unsubscribe` | "生产路径零调用点" | 唯一的 `.forget(` 是 `engine/runner.rs:278` 的 `self.artifacts.forget(...)` —— **不同对象**，不是 EventHub | **成立** |
| `deny_unknown_fields` | "全仓 0 处" | `grep -rc` 无任何非零命中 | **成立** |
| `remove_account` | "只级联 state + smid，不动 schedules/pipelines" | `json_store.rs:513-524`：只有 `state.remove` / `smid.remove` + 各自 persist | **成立** |

⇒ **五条全中**。这不能证明它全对，但足以说明它的 ★ 不是随手标的，Rust 侧的其余条目
应当按"待复核"而不是"待怀疑"来对待。以下按我能给出的把握分级。

### 2.1 我认可、且认为应当**优先修**的三条（后果不可逆或长期累积）

| 结论 | 为什么排前面 |
|---|---|
| §3.1 ★ `auth_refresh.rs:173-180` 刷新成功后 `expire` 永不推进 | 若成立，`decide()` 下次仍判"该刷" ⇒ **反复真轮换凭据**。这是全篇唯一"每跑一轮就多一次不可逆动作"的条目。它给的解释很具体（把明文 `platform:phone:token` 喂给契约要求 base64 整串的 `parse_auth`，而 `node_like_b64_decode` 会**丢掉 `:`**），且指出既有测试覆盖三档判据却**没覆盖 `rotate()` 的 expire 再推导** —— 这种"测试正好差一格"的说法值得优先验 |
| §3.2 session_source.rs `90001` 刷新后重放非幂等写 | 唯一能让"下单/领奖"类写请求**双发**的路径，而重放安全的前提（"回 90001 ⇒ 上游没执行过"）本仓自己的 `describe()` 就说是"被挡下"而非"未执行" ⇒ **未证实且后果最重**。它自己列进了 §6 未证实格，处理得当 |
| §3.2 exchange/app-task/app-task-matrix 无服务端门 | "不可逆动作只有 UI 确认、没有服务端确认"，且 CLI 完全绕过登录闸 ⇒ 绕过专用页即可触发。这条即使细节有偏差，方向也值得当硬伤看 |

### 2.2 我认可、属于"工程债"的（按它的 §5 顺序即可）

`remove_account` 不级联 schedules/pipelines（2.0 已验）；`scheduler.rs:273` 那行顺序无回归保护
（我核到 `advance_watermark` 只在 `:259/:273` 被调、`:286` 定义，**没有测试引用 `tick()`/`fire()`**）；
`hub.rs` 内存单调增长（2.0 已验）；`write_atomic` 无 `fsync`；`json_store` 无 schema 版本；
`builder.rs:177` 的 `.expect()` 与 `panic="abort"` 组合；WS 握手无超时；`hub`/`sso` 取票无 single-flight；
`verify_password` 同步跑 argon2 且无节流；`users.json`/`sessions.json` 不在权限收紧清单。

### 2.3 我**拒绝**或降级的

| 原结论 | 裁决 | 理由 |
|---|---|---|
| §3.5#5 "TLS 装置空转（两套 client 同一后端，只剩两个 cookie jar）" | **成立，但我已在 `4611590` 处理了一半** | HTTP 侧与 WS 侧都已只编 `native-tls`（`cargo tree -i rustls` 直接报"没有这个包"）；`LEGACY_TLS_HOSTS` 现在确实是空转，**我没删**：删它要动请求路径的选择逻辑，属独立一笔。四处 rustls 旧注释里我改了两处（`client.rs` / `live_consts.rs`），`live_ws_client.rs` 与 `mcloud-upstream/Cargo.toml` 那两处**待改** |
| §3.5#6 "`release.yml`（275 行，从未跑过）" | **已过期** | 该工作流此后已跑 **4 次**全部三平台成功（`36576841340` 起）。它列的三个具体问题我分别处理了：tag 校验（已加早失败）、权限收窄（已只给 release job）、Linux 可移植性（已在 `usage.md` 说明 glibc/libssl 下限） |
| §3.5#8 `panic="abort"` + `strip=true` 的代价 | **成立，我接受这个批评** | 这是我用体积换来的（release 从 8.1 MiB 降到约 3.6 MiB，−50%）。**不打算回退**，但 `strip = true` 可以考虑放宽成 `strip = "debuginfo"`（保符号、崩溃报告可读、体积涨一点）—— **待用户定** |
| §3.5#12 "`usage.md:41` 实际输出 `data_dir=data`（无引号）" | **不成立** | 实跑 release 二进制的输出是 `data_dir="/tmp/logprobe"`；`tracing` 对 `&str` 走 `Debug`，**一定带引号** |

### 2.4 独立复核状态（如实交代）

我另派了一路**独立的 Rust 复核**（逐条对 §3.1–§3.5 取证），到本文写完时**尚未返回**，
所以 §2 里除 2.0 那五条外，**没有一条是我亲手验的** —— 都是"按它给的 file:line 读下来
判断可信"。这不是采信，是分级。Rust 侧要真正落刀前，先按 2.1 的三条各做一次
"先复现再改"（尤其 `auth_refresh` 的 expire 与 `90001` 重放）。

---

## 3. CI / 发版 / 文档（这部分涉及我自己的交付物，我逐条自己验）

| 原结论 | 裁决 | 证据与我做的事 |
|---|---|---|
| §3.5#6 `release.yml` 顶层 `permissions: contents: write` 让 web/build job 白拿写权限 | **成立** | 顶层改 `contents: read`，写权限**只给 `release` job** |
| §3.5#6 tag 无校验（带 `/` 的 tag 会毁掉包名） | **成立** | `计算版本与包名` 里加早失败：`/` 或空格直接 `::error` 退出（否则斜杠会拼进 `mcloud-gost-v<VERSION>-<target>/` 的路径里） |
| §3.5#6 Linux 产物动态链 OpenSSL + glibc 下限未说明，与"拷一个二进制就能跑"冲突 | **成立** | `usage.md` 补一段：产物构建于 Ubuntu 24.04（glibc 2.39）+ 系统 OpenSSL ⇒ 需 glibc ≥ 2.39 与 `libssl3`，更老的发行版请自行构建；顺带修了 Windows 快速开始漏掉的那层 `cd` |
| §3.5#12 `usage.md` 漏 `login device` | **成立** | 已补（`cli.rs:74` 确有 `Some("device")` 分支） |
| §3.5#12 dev 代理写死 38123 而文档教 3000 | **成立** | `vite.config.ts:38` 目标确为 38123。已在 `usage.md` 增「如果你是来改前端的」一节：给出 `MCLOUD_ADDR=127.0.0.1:38123` 这条对齐命令 |
| §3.5#7 砍掉 push→main 后，main 上没有 fmt/clippy/test 兜底 | **成立** | 确是这样，且是用户"省 Actions 分钟数"的直接代价。已在 `ci.yml` 注释里写明替代方式（本地四条门禁；PR 上仍会自动跑） |
| §3.5#8 `panic="abort"` + `strip=true` 的代价：4 处 fire-and-forget `tokio::spawn` ⇒ 任一 panic 带走整个服务；崩溃报告只剩地址 | **成立，我接受这个批评** | 这是我在 `d670562`/`9d1fb30` 里换体积换来的。**但我不打算回退**：release 二进制约 3.6 MiB 是硬指标，`opt-level="z" + lto="fat" + panic="abort" + strip` 贡献了 −50%。可考虑把 `strip = true` 放宽成 `strip = "debuginfo"`（留符号、涨一点体积、崩溃报告可读）——**待用户定**，本轮未动 |
| §4 那两条"被推翻的结论"里与我有关的两条 | **裁决正确** | ① `release.yml` 冒烟"必红"不实 —— 与我的实测一致：Linux 那步因管道取 `grep` 状态而通过，**Windows 那步反而是因为 pwsh 末尾 `exit $LASTEXITCODE` 判失败**（我后来修的正是这个）；② `sha25sum` 拼写不成立 ✓ |
| §1.3 体积宣称 | **已过期**（详见 §1.1） | 另外它引用的量法 `node web/lite/size.mjs` 当时已经**跑不起来**：产物是 `lite.mjs`，而脚本只找 `.endsWith('.js')` ⇒ `find` 返回 `undefined` ⇒ `path.join(undefined)` 抛 `ERR_INVALID_ARG_TYPE`。**已修**（匹配 `.js`/`.mjs` + 找不到时抛可读错误），修完复现出文档里的 **6,147 raw / 2,808 gzip** 与 **5,995 / 2,757** |

---

## 4. 本次落地清单（代码）

| 文件 | 改了什么 |
|---|---|
| `web/lite/size.mjs` | 产物扩展名匹配 `.js` 与 `.mjs`；找不到时抛可读错误（原来崩在 `path.join(undefined)`） |
| `web/index.html` | boot 动画的三段失败兜底（`error`/`unhandledrejection`/12s 超时），判据 `#app` 有无子节点 |
| `web/src/api.ts` | `request()` 加 30 秒超时 + `TimeoutError` 中文化 |
| `web/src/account-names.ts` | 名字缓存改响应式信号，整张 Map 赋值 |
| `web/src/ui/auth-signal.ts` | 删掉死状态 `authEpoch`，留说明 |
| `web/src/pages/schedules-page.tsx` | legend 改"账号 id"并写明理由；列表走 `accountName()` |
| `web/src/pages/exchange-page.tsx` | 两处 `.map` 补 `key` |
| `docs/usage.md` | 补 `login device`、Windows `cd`、Linux glibc/libssl 下限、dev 代理 38123 的对齐命令 |
| `.github/workflows/release.yml` | 权限收窄（写权限只给 release job）+ tag 名字校验 |
