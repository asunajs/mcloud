# findings-261004-web

- 基线: HEAD 2ca9cf7
- 范围: `web/`（自研 lite 框架 + TSX + Tailwind v4 自维护 `@theme`）：`web/src/`、`web/lite/`、`web/scripts/`，以及对照只读的 `scripts/gates-web.mjs` / `.github/workflows/ci.yml`。**只读**：未改任何源码、未碰 `web/dist`、未跑 `npm run build`。
- 方法: 通读 `web/src/pages/*.tsx`（12 页）、`web/src/ui/*`（高频件）、`web/src/api.ts`、`web/lite/compiler.ts`、`web/lite/src/dom.ts`、`web/lite/regress/{interact,leak,narrow,compiler}.mjs`、`scripts/gates-web.mjs`、`web/src/app.css`；把 `web/dist/assets/index-CWnlhps5.css` 当**只读**产物反查 utility；用 `node --experimental-transform-types` 直接 import `web/lite/compiler.ts` 做编译实验（不落盘、不构建、不写 dist）；跑 `cd web && npm --cache /tmp/npmcache-dsh run typecheck`（仅类型检查）。**每条行号都先 `grep -n` 确认**。

## 摘要（≤8 行）

1. **Q1 竞态**：病灶是"多处 `load()` 连代次号/`AbortController` 都没有"，而单请求超时是 **30 s**（`api.ts:270`）。最重的是日志页（2 s 一拍 ⇒ 最多 15 个并发在飞）；另有仪表盘"今日AI豆明细"切档竞态、账号页扫码轮询的跨会话污染。
2. **Q2 XSS/注入：阴性**。`web/src/**` 零 `innerHTML` / `dangerouslySetInnerHTML` / 动态 `<script>` / `href="javascript:"`；全仓唯一 `innerHTML` 在 `web/lite/src/dom.ts:39`，吃的是编译期静态模板串（无插值）。40 处模板字面量 `class` 全来自查表常量。
3. **Q3 `@theme` 缺口**：浅色侧逐个反查产物 CSS **没有**"用了却不生成"的 utility；缺口在**深色那半** —— `[data-theme="dark"]` 漏写 `--color-neutral`，`.status-neutral` 直接吃它 ⇒ 深色下"调度没在跑"那颗点 **1.11:1**，等于看不见。
4. **Q4 lite 边界**：页面写法与硬边界（`.map` 的 `key` 位置、三元只认 JSX/null）**不冲突**。但 `compiler.ts:42-44` 承诺"`(item) => navLink(item.id)` 一样进 `createFor`" —— **实测为假**，而这正是 `app.tsx:602` 的写法（框架自己的 `dom.ts:381` 注释也把它算在 `setNodes` 那类）。属"文档骗人"，不是"渲染错"。
5. **Q5 门禁**：`gates-web.mjs` 的清单比文档写的多、`docs/web/lite-framework.md:456` 的表已过期；**仪表盘页在三个交互门里一条内容钉子都没有**，日志页不进 `ROUTES`/`leak PAGES`/`narrow ROUTES`（最像泄漏源的一页恰在泄漏门之外）。
6. **"跳过"不会在摘要里被读成 pass**（明写 `skipped(无浏览器)`），**但退出码是 0、末行印"全绿（commit 档 N 条）"** ⇒ 只看退出码的钩子/调用方会把"没跑过"当"通过"；CI 干脆没调 `gates-web.mjs`，自己内联了三条（第二份清单）。
7. **Q6 死代码**：无"零 import 的模块"；有 1 个**导出即死**的函数（`initialTheme`，连模块内都没人调），另有 1 个在 daisyUI 移除后**已跑不动**的脚本仍被现役文档当作"现算 CSS 体积"的命令。
8. 已实测手段：grep 全量、产物 CSS 反查、编译器真编译 5 组输入、对比度计算、typecheck。所有浏览器门（`interact`/`leak`/`narrow`/`demo`/`bench`）**未跑**。

## 发现

### wb-1 [高] 日志页每 2 s 轮询无代次/abort：旧响应覆盖新数据，还能把已恢复的页面打回 error

- 位置: `web/src/pages/logs-page.tsx:75`（`const load = async ()`）、`:101`（`lines.value = await listServerLogs(...)`）、`:105`（`state.value = 'error'`）、`:116`（`setInterval(() => void load(), 2000)`）；超时口径 `web/src/api.ts:270`（`REQUEST_TIMEOUT_MS = 30_000`）、`:280`（`signal: init?.signal ?? AbortSignal.timeout(...)`）
- 证据: 这是全仓**唯一**每 2 s 打网络的轮询。`load()` 里没有任何"只认最后一次"的判断（`grep -n "AbortController\|generation\|seq" web/src/pages/logs-page.tsx` ⇒ 无匹配），而单请求超时上限 30 s = 间隔的 **15 倍** ⇒ 慢后端下会有多个 `load()` 同时在飞。每个 `load()` 结尾都无条件写 `lines.value` / `state.value`：先发出的**旧**请求后到时会把新一屏整份盖掉；交叉情形（旧请求失败、新请求成功）里 `:105` 还会把已渲染好的页面打成 `error`。
- 复现: `grep -n "setInterval\|listServerLogs\|state.value = \|AbortController" web/src/pages/logs-page.tsx`
- 影响: 慢后端/抖动网络下日志页可能显示比上一次更旧的内容，`ready`/`error` 交替闪；用户无法判断哪一屏是真的。同一段代码还是那页唯一"每 2 s 重建 800 行 DOM"的地方（见 wb-5：它同时在泄漏门之外）。
- 建议: `load()` 入口取模块级递增 `seq`，`await` 回来先 `if (seq !== latest) return`；或把 `AbortController` 存在 timer 同级变量、下一拍先 `abort()` 上一拍（`api.ts:280` 已支持 `init.signal` 透传）。
- 状态: 仅静态 · 置信度 高

### wb-2 [中] 仪表盘「今日AI豆明细」切档/换号无并发防护：旧响应写进新档位（含"旧号数据 + 新号标题"）

- 位置: `web/src/pages/dashboard-page.tsx:645`（`const loadBeanDetail = async ()`）、`:649`（`beanDetail.value = await getBeanRecords(beanDetailAccount.value, beanDetailDays.value)`）、`:663`（`openBeanDetail`）、`:668`（换号时 `beanDetail.value = null`）、`:674`（`switchBeanRange`）；档位 Tab 在 `:1031` 起
- 证据: `switchBeanRange(days)` 先写 `beanDetailDays.value = days`，再 `void loadBeanDetail()`；每次调用都**新建**请求，没有取消/代次判断 ⇒ 连点「7 天 → 30 天」时两个请求都在飞，先发的 7 天响应若后到，就会把 7 天的记录填进标着「30 天」的 Tab（Tab 高亮跟着 `beanDetailDays` 走，所以**标题与内容不一致**）。`openBeanDetail` 那句"清掉上一个账号的数据"（`:668`，注释写着"旧号的流水 + 新号的标题是最坏的一种显示"）只挡住同步顺序，**挡不住在飞的旧请求**：换号后旧号响应后到，同一种坏显示照样出现。
- 复现: `grep -n "loadBeanDetail\|switchBeanRange\|beanDetail.value = await\|beanDetailDays.value = days" web/src/pages/dashboard-page.tsx`
- 影响: 明细弹窗可能展示"不是这个档位/不是这个账号"的数据，而标题、Tab、合计都在说另一件事 —— 用户据此做的判断是错的，且没有任何迹象。
- 建议: 与 wb-1 同一修法（代次号）；或把 `account+days` 当请求键，回来时比对当前键再写入。
- 状态: 仅静态 · 置信度 高

### wb-3 [中] 账号页扫码轮询不校验会话归属：作废会话的响应会污染新会话的界面

- 位置: `web/src/pages/accounts-page.tsx:399`（`const pollOnce = async ()`）、`:400`（`const sid = qrSessionId.value`）、`:406`（`const r = await pollQrLogin(sid)`）、`:408`/`:420`（写 `qrPhase`）、`:426`（`await load()`）；对照 `:379`（`beginQr`）、`:386`（写新 `session_id`）、`:352`（`abandonQr`）、`:344`（`stopPolling`）
- 证据: `pollOnce` 把 `sid` 捕获在闭包开头，但 `await` 回来后**只**判断 `phase`，**从不**校验 `sid === qrSessionId.value`。`abandonQr()` 只做 `stopPolling()` + 清状态 + `cancelQrLogin(sid)`，**取消不掉已经在飞的那次 `pollQrLogin`**。于是"作废旧码 → 立刻出新码"这条真实操作里，旧会话的响应可以落在新会话的界面上 —— `logged_in` 分支尤其糟（`:417-426` 把 `qrImage`/`qrNote` 清空、`qrPhase` 置 `idle`、还 `await load()`）：用户刚出的新二维码会**凭空消失**，同时看到一条属于上一个会话的"已添加账号 X"回执。
- 复现: `grep -n "const pollOnce\|const sid = qrSessionId.value\|await pollQrLogin\|qrPhase.value = " web/src/pages/accounts-page.tsx`
- 影响: 出码/重出码时界面状态与真实会话错位（码消失、回执串会话），排查成本高（看起来像"后端把码吃了"）。
- 建议: `await` 之后加 `if (sid !== qrSessionId.value) return`；或在 `abandonQr` 里用 `AbortController` 真取消在飞请求。
- 状态: 仅静态 · 置信度 高（机理确定；触发需要"在飞时重出码"这一具体操作）

### wb-4 [中] 深色主题漏注册 `--color-neutral`：`.status-neutral` 在深色下对比度 1.11:1（状态点消失）

- 位置: `web/src/app.css:442`（`.status-neutral { background-color: var(--color-neutral); }`）、`:113`（`[data-theme="dark"] {` 起始，块到 `:144` 为止）、浅色定义在 `:39`（`--color-neutral: #09090b;`）与 `:40`（`-content`）；消费方 `web/src/app.tsx:343`、`web/src/pages/dashboard-page.tsx:413-422`
- 证据: 深色块重写了 `--color-base-*` / `--color-primary*` / 四个语义色 + 它们的 `-content`，**唯独没有 `--color-neutral` / `--color-neutral-content`**（`grep -n "color-neutral" web/src/app.css` 只命中 `:39`/`:40` 两行浅色定义）。深色下 `--color-base-100: #0f172a`（`:114`）而 `--color-neutral` 仍是 `#09090b` ⇒ 对比度 **1.114:1**（浅色下 19.9:1；`#09090b` vs 深色 `base-200 #07111f` 只有 1.051:1）。这正是 app.css 顶部那条"两份必须成对写"的纪律被破的一处。
- 复现: `grep -n "color-neutral" web/src/app.css && sed -n '113,144p' web/src/app.css`
- 影响: 深色主题下"引擎/调度没在跑"与"还在问后端"这两颗点等于不存在，而它们承载的正是"引擎到底在不在跑"的唯一视觉信号（`dashboard-page.tsx:404-412` 的注释明确说绿=在跑、灰=没在跑，且**不用红**）。
- 建议: 在 `[data-theme="dark"]` 里成对补 `--color-neutral`（深色该**提亮**，例如 `#3f3f46` 一档）与 `--color-neutral-content`；补完按同一纪律复查整个 dark 块是否还有只写了一半的令牌。
- 状态: 已实测（对比度由脚本计算，未用浏览器量 `getComputedStyle`）· 置信度 高

### wb-5 [中] 门禁覆盖缺口：仪表盘零内容钉子；日志页不在任何浏览器门的清单里；"跳过"退出码为 0

- 位置: `web/lite/regress/interact.mjs:167`（`const ROUTES = ['dashboard', …]`）、`:175`（该循环唯一的断言是 `location.pathname`）、`:1022`（日志页那 3 条滚动钉子）；`web/lite/regress/leak.mjs:63`（`PAGES` 8 页，**无 `logs`**）；`web/lite/regress/narrow.mjs:164-175`（同样 9 条路由，**无 `logs`**）；`scripts/gates-web.mjs:180-184`（`browserMissing` ⇒ `results.push({ name, ok: true, skipped: true })`）、`:210-211`、`:221-222`
- 证据:
  - **dashboard**（内容最多的一页：指标卡、运行表、今日AI豆、明细弹窗、引擎条）在整个 `interact.mjs` 里**只出现在 `:167` 那个数组里** —— `grep -n "dashboard" web/lite/regress/interact.mjs` 仅此一行，即断言只到"路径变成了 /dashboard"。它的内容、AI豆明细弹窗、档位 Tab（wb-2 的现场）、引擎条状态点都没有钉子。
  - **logs 页**有 3 条滚动钉子（`:1022` 起，`ready` 档），但它**不在** `interact.ROUTES`、**不在** `leak.PAGES`、**也不在** `narrow.ROUTES` ⇒ 那一页的横向溢出与"每 2 s 重建 800 行"的净增长从来没被量过 —— 恰是 wb-1 的现场。
  - 跳过语义：`browserMissing()` 把跳过的门记成 `ok: true` 后直接 `return`（`:136`/`:163`），于是 `:210` 的 `failed` 为空 ⇒ **`process.exit(0)` + `:222` 打印"全绿（commit 档 N 条）"**。摘要与末行都有 ⚠ 提示（这点做对了），但只看退出码的调用方读到的就是"通过"。
  - CI 更远：`.github/workflows/ci.yml` **没有**调 `scripts/gates-web.mjs`，而是自己内联 `npm run typecheck` / `npm run build` / `node lite/regress/compiler.mjs` 三条（其注释自认"五条前端闸门里 CI 只覆盖这三条"）⇒ 门禁清单存在**两份**（`gates-web.mjs` 的 `GATES` 与 ci.yml 的 steps），会各自漂移。`docs/web/lite-framework.md:456` 那张表也已旧（仍把 `all.mjs` 列成门、默认含 `narrow.mjs`、用旧名 `--full`）。
- 复现: `grep -n "dashboard\|logs" web/lite/regress/interact.mjs web/lite/regress/leak.mjs web/lite/regress/narrow.mjs | head` 与 `sed -n '180,190p;205,225p' scripts/gates-web.mjs`
- 影响: 最容易坏的一页（仪表盘）与最像泄漏源的一页（日志）都在浏览器门的盲区；"门全绿"在被跳过的机器上并不代表这些交互被验过。
- 建议: ① 给 dashboard 补**内容级**钉子（指标卡有读数、运行表有行、AI豆明细能开-切档-关）；② 把 `logs` 加进 `narrow.ROUTES` 与 `leak.PAGES`（或显式写明为什么不加）；③ 跳过的门让退出码非 0（或由调用方 `grep -q skipped` 把关）；④ `ci.yml` 改调 `gates-web.mjs --commit`，消掉第二份清单。
- 状态: 已实测（读脚本 + grep）· 置信度 高

### wb-6 [中] `compiler.ts` 文件头承诺 ≠ 实现：`(item) => navLink(item.id)` 这类"调用体 `.map`"根本不进 `createFor`

- 位置: `web/lite/compiler.ts:42-44`（文件头承诺）、`:566`（`.map` 分支的进入条件）、`:734`（`const isJsxNode = (n) => ast.isJsxElement(n) || …`）、`:610`（`throw new Error('条件分支只支持 JSX 或 null')`）、`:203`（未认领 `key` 的抛错文案）；`web/lite/regress/compiler.mjs:66-70`（唯一那条 `createFor` 负例，只覆盖**块体**）；对照写法 `web/src/app.tsx:602`
- 证据: 文件头写着"**不要求回调返回 JSX 字面量**：`(item) => navLink(item.id)`、带 `return` 的块体一样进 `createFor`（……少了这一步，导航那一列每次切页都会整块重建）"。实测（`node --experimental-transform-types` 直接 import `web/lite/compiler.ts`；不落盘、不构建）：

  ```
  call-body-map   <ul>{NAV_ITEMS.map((item) => navLink(item.id))}</ul>
      => createFor: false | setNodes: true
  jsx-body-map    <ul>{NAV_ITEMS.map((item) => <li>{item.id}</li>)}</ul>
      => createFor: true  | setNodes: true
  block-body-map  <ul>{NAV_ITEMS.map((item) => { return <li>{item.id}</li> })}</ul>
      => createFor: false | setNodes: true
  ```

  即：`:566` 的进入条件是 `isJsxNode(cb.expr)`，调用体既不是 JSX 也不是三元 ⇒ 走通用路径。**`app.tsx:602` 正是这个形状**，而框架自己在 `web/lite/src/dom.ts:381` 的注释里也把它算在 `setNodes(parent, () => items.map(…))` 那一类（并记录过它曾导致"每次切页 +28 个 effect / +465 个节点"的泄漏）。`regress/compiler.mjs` 只有块体那条负例，**调用体没有任何钉子** ⇒ 文件头这句可以继续漂移。
  同源的小坑（实测）：`{ok ? xs.map((x) => <li key={x}>{x}</li>) : null}` 会**抛错**，但抛的是 `:203` 那句"这里的 `key` 什么都不做：它只对 `.map()` 返回的那个元素有意义" —— 而 `key` **恰恰**写在 `.map()` 返回的元素上；真实原因是"这个 `.map` 在条件分支里，编译器不认它是列表"（`:563` 的 `branchOk` 只认 JSX/null）。文案会把下一个人引向错方向。
- 复现: `cd web && node --experimental-transform-types --input-type=module -e "const {compile}=await import('./lite/compiler.ts');const o=compile('export const App=()=> <ul>{N.map((i)=>f(i))}</ul>',{filename:'c.tsx'}).code;console.log('createFor:',o.includes('createFor'))"`
- 影响: 读文件头的人会以为导航列是键控复用的（并据此做性能判断/重构）；真要在那儿加 `key` 又会在别处撞上语义错误的报错。属"能编译但行为与文档不符"，不是渲染错。
- 建议: 改文件头那句（"只有回调**本体是 JSX** 才进 createFor"），并给 `regress/compiler.mjs` 补两条：调用体 ⇒ `noOk: ['createFor(']`；三元里的 `.map` + `key` ⇒ 抛错且文案改成"条件分支里的 `.map()` 不建列表，`key` 无效"。
- 状态: 已实测（真编译 5 组输入）· 置信度 高

### wb-7 [中] 仪表盘/计划页的"手动刷新 × 轮询"可并发，后完成者胜（与 wb-1 同源）

- 位置: `web/src/pages/dashboard-page.tsx:335`（`const load = async ()`）、`:385`（`setPageRefresh({ run: () => void load() })`）、`:390`（30 s 本地时钟 ticker，不碰网络）；`web/src/pages/schedules-page.tsx:187`（`const refresh = async ()`）、`:319`（`setPageRefresh`）、`:323`（`setInterval(() => void refresh(), 60_000)`）
- 证据: 两页的取数入口都既被页面刷新图标/按钮（`setPageRefresh`）触发，又被定时器触发，而 `load()`/`refresh()` 同样没有代次号或 abort（`grep -n "AbortController\|generation" web/src/pages/dashboard-page.tsx web/src/pages/schedules-page.tsx` ⇒ 无匹配）。`dashboard` 的 `load()` 一进来就 `state.value = 'loading'`（`:336`）并清 `errorMessage`，收尾无条件写五个 `ref` + `loadedAt`；两次交错时界面可能"倒带"回旧数据，或停在别人触发的 `loading` 上（`loadedAt` 也会被先返回的那次写老）。
  已排除一条疑点：切页时 `onUnmounted` 里的 `setPageRefresh(null)` **不会**清掉新页面刚登记的刷新动作 —— `dom.ts:413-415` 的顺序是先 `remove(cur)`（⇒ `disposeTree` ⇒ 同步跑旧页 `onUnmounted`）再 `createNodes`+`insert`（⇒ 新页 `onMounted`），顺序是对的。页面级那 5 处回到 `null` 在当前顺序下安全（"登记返回上一份"的约定只在 `task-config-page.tsx:84/90` 真正用上）。
- 复现: `grep -n "setPageRefresh\|setInterval\|const load = async\|const refresh = async" web/src/pages/dashboard-page.tsx web/src/pages/schedules-page.tsx`
- 影响: 手动刷新期间数据可能短暂回到旧值、`state` 三态闪；比 wb-1 轻（间隔 60 s 且非唯一入口），但机制完全相同。
- 建议: 与 wb-1 合并到一处修（在 `api.ts` 层做"同一 path 只认最后一次"，或各页统一加代次号），别每页各写一遍。
- 状态: 仅静态 · 置信度 中

### wb-8 [低] `daisyui-size-probe.mjs` 在 daisyUI 移除后已跑不动，现役文档仍把它当作"现算 CSS 体积"的命令

- 位置: `web/scripts/daisyui-size-probe.mjs:353`（`readdirSync(join(WEB, 'node_modules/daisyui/components'))`）、`:368`/`:372`/`:381`（对 `@plugin "daisyui"` / `@plugin "daisyui/theme"` 做 `stripPlugin`）；文档 `docs/web/lite-framework.md:16`、`:22`；另 `web/package.json:5` 的 description 仍写"+ daisyUI"
- 证据: `web/package.json` 的 `devDependencies` 里**没有** `daisyui`，`ls web/node_modules/daisyui` ⇒ `No such file or directory`；而探针 `:353` 要读它的 `components/` 目录、`:368+` 要在 `app.css` 里找已经不存在的 `@plugin "daisyui"`（`web/src/app.css` 头部注释：2026-10-03 已移除 daisyUI）。它的五个变体全部围绕"去掉 daisyUI 省多少"设计 ⇒ 现在既读不到组件目录、也 strip 不到插件。
  `web/lite/regress/all.mjs` **不是**死代码：它 `:18-21` 自己说明"`scripts/gates-web.mjs` 现在不调它了"，是有意保留的手工入口；但 `docs/web/lite-framework.md:456` 仍把它列成"lite 真 DOM 验收"这道门，与 `gates-web.mjs` 现在的清单不符（同样过期的还有 `--full` 这个旧名与"默认含 narrow"）。
- 复现: `ls web/node_modules/daisyui; grep -n "daisyui" web/package.json web/scripts/daisyui-size-probe.mjs | head`
- 影响: 照文档"现算 CSS 体积"的人会撞一个与本次改动无关的失败，或（更糟）把探针里的历史读数当现状 —— 那正是当初移除 daisyUI 的判据来源。
- 建议: 删掉探针（或改成"量自维护 `@theme` 的产物体积"）；把 `lite-framework.md` 的 §7 门禁表与"主产物 CSS"两处改成现状（两档 + `--push`，`demo/run.mjs` + `bench/run.mjs` 取代 `all.mjs`）；顺手改 `package.json` 的 description。
- 状态: 已实测（依赖与文件系统读数；未运行探针本身）· 置信度 高

### wb-9 [低] `ui/theme.ts` 的 `initialTheme()` 导出即死，注释指向一处已搬走的逻辑

- 位置: `web/src/ui/theme.ts:29`（`export const initialTheme = (): Theme => {…}`）、`:16-18`（注释"这段逻辑在 `index.html` 里另有一份同步副本"）、`:24-25`（"「跟随系统」只作为**初始值**在这里算一次"）
- 证据: `grep -rn "initialTheme" web/src web/lite` ⇒ **只有 `web/src/ui/theme.ts:29` 这一行**（连模块内部都没人调用）；`app.tsx:15` 只 import 了 `currentTheme, toggleTheme, type Theme`。实际首屏初始值由 `index.html` 的内联脚本算（文件头自己也承认有一份副本）⇒ 这个函数（连同它里面 `matchMedia` 那条分支与读 localStorage 的 `try/catch`）是纯死的，而注释仍在说"在这里算一次"，会把读者引到一个不存在的调用点。
- 复现: `grep -rn "initialTheme" web/src web/lite; grep -n "ui/theme'" web/src/app.tsx`
- 影响: 无功能影响；但"改主题初始化"的人会改错地方（改 TS 这份而线上走 `index.html`），或以为存在一条自动跟随系统的路径。
- 建议: 删掉 `initialTheme`（连同注释里那句定位），或改成注释指向 `index.html` 并注明"这里是历史副本，别在这儿改"。
- 状态: 已实测（grep 全量）· 置信度 高

### wb-10 [低] XSS / 注入面：`web/src/**` 未发现任何可注入点（阴性结论）

- 位置: `web/lite/src/dom.ts:39`（全仓唯一的 `innerHTML` 写入）
- 证据: `grep -rn "innerHTML\|outerHTML\|insertAdjacentHTML\|dangerouslySetInnerHTML\|document.write\|createElement('script')\|javascript:" web/src web/lite --include=*.ts --include=*.tsx --include=*.mjs` 共 4 条命中，**没有一条是 sink**：`web/lite/src/dom.ts:39`（`template()` 的静态模板，编译期生成、无插值）、`:34`/`:479` 与 `web/lite/demo/main.tsx:431`、`web/lite/regress/interact.mjs:66` 都是注释或**只读**的 `outerHTML` 断言。另查动态类名：`grep -rn 'class={\s*`' web/src --include=*.tsx` 命中的 40 处全部来自查表常量（`TONE_CLASS` / `statusBadge` / `TOAST_CLASS` / `SIZE_CLASS` 等），没有把接口数据直接拼成类名或标签名；页面槽位由 `app.tsx` 里**固定的** `ROUTES` 映射 `await import()`，路径不决定模块名。
- 复现: `grep -rn "innerHTML\|dangerouslySetInnerHTML\|javascript:\|document.write" web/src web/lite --include=*.ts --include=*.tsx`
- 影响: 无（阴性；列出来是为了让下一个人不必再查一遍）。
- 建议: 保持；若将来给 lite 增加运行期 `html()` 原语，需在文档里显式写明"这个入口不做转义"。
- 状态: 已实测（grep 全量）· 置信度 高

## 未覆盖 / 未验证

- **没跑任何浏览器门**（`interact` / `leak` / `narrow` / `demo` / `bench`）—— 它们会起浏览器并写 `web/dist` 之外的临时产物；凡涉及运行时行为的结论都标了「仅静态」。
- **未实测**：深色下 `status-neutral` 的 `getComputedStyle`（只算了对比度）；wb-1/wb-2/wb-3 的竞态**未在真浏览器里构造**（需要人为拉长某次响应）。
- 未审查：`web/lite/demo/**` 与 `bench/**` 的断言质量；`web/lite/src/*.ts` 的运行时实现（除 `dom.ts` 的相关片段）；`exchange-page.tsx`（≈1400 行）与 `accounts-page.tsx`（≈2000 行）只做了定向 grep + 局部通读；`web/src/ui/select.tsx` 的浮层逻辑未通读。
- 未做：Rust 侧、`web/dist` 产物正确性、CSP/响应头（属服务端）。
