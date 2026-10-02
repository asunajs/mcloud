# lite —— 给本项目的轻量前端框架（设计与现状）

> **一句话**：无虚拟 DOM + 编译期优化的自研前端框架。TSX 在构建时被折成
> "模板串 + 每个动态位置一条 effect"，运行时只留本项目真用到的那几个原语。
>
> **它已经是默认构建**（2026-09-29 切换，2026-09-30 拆掉 Vue 兼容层）：
> `web/vite.config.ts` 里就是 lite 的插件，业务源码写 `from 'lite'`，
> 仓库里**不再有任何 Vue 依赖**。

| 指标 | 现在 | 怎么量 |
|---|---|---|
| 运行时 gzip（全量导出） | **2,456 B**（raw 5,598） | `node web/lite/size.mjs` |
| 运行时 gzip（不含只有 demo/bench 用的内部件） | **2,442 B**（raw 5,555） | 同上 |
| 主产物 JS（单 chunk：业务 + 运行时 + 编译胶水） | **120.65 kB** / gzip **32.92 kB** | `cd web && npm run build` |
| └ 其中运行时占 | gzip **2.44 kB** ≈ 全量的 **7.4%** | 上面两行相除 |
| 主产物 CSS（Tailwind + daisyUI） | 47.81 kB / gzip **9.91 kB** | 同上 |
| 挂载 1000 行（编译产物） | **2.5–5.9 ms**（best of 3） | `node web/lite/bench/run.mjs` |
| 反转 2000 行 ×10 | **8.6–12 ms** ⚠ 唯一劣化项 | 同上 |
| 门禁（默认 / `--full`） | **≈33 s / ≈77 s** | `node scripts/gates-web.mjs` |

> 📌 表里的数字是**编译产物**的读数。以前引的"挂载 1.2 ms"是手写目标形态的裸速，
> 口径偏乐观，已作废（见 §6 与 [`lite-postmortem.md`](lite-postmortem.md)）。

## 这三份文档怎么分工

* **[`lite-guide.md`](lite-guide.md)** —— 把 lite 用到**新项目**的教程（拷哪几个文件、
  tsconfig、JSX 类型、语法子集、常见坑）。**想写代码读这份。**
* **本文件** —— 设计与取舍、体积/性能读数、验收命令。**想改运行时或编译器读这份。**
* **[`lite-postmortem.md`](lite-postmortem.md)** —— 踩坑史（§11–§12 的编号沿用原文件），
  按时间追加。**别照中间那几条判断动手**，里面有几条归因后来被推翻。

---

## 1. 为什么自研

不是为了"不要依赖"，是为了**体积与首屏**。迁移前的起点（`docs/architecture.md` §10.6 有全程）：

* 主产物 JS 154,441 B（gzip 48,718），其中 Vue 运行时占 **43,656 B** —— 空应用地板，
  本项目一行代码都没写它就在那儿。
* 普查（§2）显示真正用到的 Vue API 只有 **6 个**，其余（Teleport / Transition /
  KeepAlive / Suspense / 异步组件 / 指令 / computed）**一处都没用**。

现在的结果：主产物 JS 153.8 kB → **120.8 kB**（gzip 48.4 → **33.1 kB**，约 −32%），
CSS 与类名一字未动。**性能不是自研的理由** —— 2,000 行的列表这个项目根本没有。

## 2. 语法子集：由普查决定，不是拍脑袋

2026-09-29 对 `web/src` 全量 28 个文件、7,203 行的普查。这份表是"为什么只做这么小"的依据，
框架范围由它定，不随后来加的护栏改变：

| 当时的 Vue API | 用量 | lite |
|---|---|---|
| `ref` | 14 个文件 / 66 处 | ✅ |
| `onMounted` / `onUnmounted` | 11 / 5 个文件 | ✅ |
| `watch` | 1 处（`app.tsx` 盯登录态） | ✅（初始不触发） |
| `useSlots` | 1 处（`dialog.tsx` 的 `slots.default?.()`） | ✅ |
| `computed` / `reactive` / `nextTick` / `provide` / `inject` / `toRef` | **0** | ❌ 不做 |
| 指令（`v-if` 等） / 模板引用 / `class` 数组·对象 / `style` 对象 | **0** | ❌ 不做 |

TSX 用到的形态（同样是普查）：静态 `class` 579 处、动态 `class` 41+18、事件 `onXxx` 110、
`key=` 31、`{...obj}` 展开 16（全是模块级常量）、三元条件 97、`.map()` 41、片段 14。

**结论：这个框架要支持的东西非常少** —— 4 个响应式原语 + 一套 JSX 形态。
这是 2.7 KB 能成立的前提，也是"编译器敢把路径算进源码"的前提。

## 3. 运行时

四个文件，没有第五个：

```
lite/src/signal.ts      响应式核心：ref / effect / batch / watch + 组件作用域归属 + 循环护栏
lite/src/dom.ts         template / 各类 setter / insert / remove / lazySlot + 按节点记账
lite/src/control.ts     createFor（键控列表）
lite/src/component.ts   createComponent / mount / onMounted / onUnmounted / useSlots
```

导出**分三组，别当成同一回事**（`lite/src/index.ts` 的文件头有同样的注释）：

| 组 | 名字 | 谁用 |
|---|---|---|
| 编译器 import 的 | `batch` `effect` `ref` `watch` `createComponent` `createFor` `lazySlot` `on` `remove` `setAttr` `setClass` `setNodes` `setProp` `setValue` `spread` `template` | 改名字 = 编译产物断（`compiler.ts` 里那些 `this.h('…')`） |
| 业务代码手写的 | `mount` `onMounted` `onUnmounted` `useSlots`，类型 `Component` / `Slots` / `Ref` | 页面与 UI 单元 |
| 运行时内部件 | `createNodes` `insert` `onRemove` | 只有 demo / bench / 排障该碰（曾经还有个 `setText`：编译器不发、运行时不用、demo 也没调 ⇒ 2026-09-30 删了死码） |

### 3.1 响应式模型：同步刷新 + `batch`，没有调度器

信号一写，受影响的 effect 立刻跑。理由：本项目的依赖链只有"信号 → DOM 写入"这一层，
同步执行省掉整套调度器（约 300 B）。要一次改多个信号再统一刷 DOM 就写 `batch()` ——
**事件处理器由编译器自动包**（§4.4），人写的代码只在明显要合批时手动包。

`batch` 的排空是**再入安全**的：排空期间仍算批量（`depth++` 包住这一轮），
所以"batch 里再改 batch"不会当场同步刷，一轮里同一个 effect 最多跑一次。
旧写法在 `depth` 归零后才跑队列，一次事件能把 DOM 写好几遍。

### 3.2 effect 的两种归属（这是运行时的骨架）

* **按节点**（`dom.ts` 的 `owners`）：`setNodes` / `createFor` 建的绑定 effect。
  节点被 `remove()` 摘走时 `disposeTree()` 连带销毁 —— 列表删一行就停那一行的 effect。
* **按组件实例**（`signal.ts` 的 `ownedEffects`）：公开的 `effect()` 与 `watch()` 建的。
  它们不属于任何 DOM 节点（`watch` 只回调、不写 DOM），走不到第一条路，
  于是组件卸载时统一销毁。运行时内部用 `newEffect()`（不挂作用域），公开的 `effect()` 才挂。

两条防线**互补**，都不许多：按父节点记账管不到"父还活着、子树已被别处整块换掉"这一类，
所以 `setNodes` / `createFor` 的 effect 在**重跑之前**先看"我上次插进去的节点还在不在文档里"，
全都不在 ⇒ 它已经不属于我们，自己 `dispose()` 再返回。判据必须在做任何事之前
（不能先 remove 再判断）。这一条防的是"越刷新内容越多"那种**静默重复插入**，
见 `lite-postmortem.md` §12.4–§12.5。

### 3.3 循环护栏

`A → B → A` 这种互写会变成一条**可读的抛错**，而不是把栈打爆：
`MAX_NESTING = 100`（effect 重入层数）与 batch 队列的轮数上限各一道。
错误**不指名**是哪条 effect（给每条存标签是白付的字节），它只把范围收窄到
"是互写，不是数据太多"；这已经比 `RangeError: Maximum call stack size exceeded` 强 ——
爆栈时中途的 DOM 写入早就把现场冲掉了。

## 4. 编译插件

`lite/ast.ts` + `lite/compiler.ts`（用 **`oxc-parser`** 解析 TSX：`ast.ts` 负责把 ESTree 适配成
编译器要的那几个接口，`compiler.ts` 只写代码生成；2026-10-01 之前解析用的是 `typescript` 包，见 §4.6）
+ `lite/vite.ts`（Vite 插件，`enforce: 'pre'` —— 必须先于下游转换器拿到带 JSX 的源码）。

> ### 4.6 解析器从 `typescript` 换到 `oxc-parser`（2026-10-01）
>
> 起因是用户一句"ts 就是为了编译，去掉"。前半句**是对的**：全仓只有 `compiler.ts`
> 一处 import `typescript`。但上一版把工作量说成了"要把 780 行重写" —— **估错了**，实测尺寸是：
>
> | 事实 | 数 |
> |---|---|
> | `compiler.ts` 总行数 | 799 |
> | 其中碰 `ts.` 的行 | **95**（12%） |
> | 用到 TS 的 printer（`createPrinter`/`printNode`/`getText`） | **0** —— 代码生成本来就是自写字符串拼接 |
> | 新增的适配层 `lite/ast.ts` | ~300 行（含注释） |
>
> ⇒ 换解析器换的是**一条接缝**：`ts.` → `ast.` 一次机械改名，剩下的差异由 `lite/ast.ts`
> 一次 **normalize 遍历**抹平（详见该文件头部）。
>
> **三类真实差异**（都是实测踩出来的，不是推演）：
>
> 1. **字段名**：`Literal.value` vs TS 的 `.text`、`CallExpression.callee` vs `.expression`、
>    `JSXOpeningElement.name` vs `.tagName`、`Property.key/value` vs `.name/.initializer`…
> 2. **包了一层**：TS 的 `openingElement.attributes` 是个 **JsxAttributes 节点**（数组在
>    `.properties` 上），TS 的参数是 **ParameterDeclaration**（模式在 `.name` 上）——
>    ⇒ 适配层**造**同形状节点补上，否则那三处 `opening.attributes.properties` 直接读到 undefined。
> 3. **语义口径**：`{/* 注释 */}` / `{}` / `{ }` 三种空表达式容器，TS 一律给
>    `expression === undefined`（编译器靠它跳过），oxc 给 `JSXEmptyExpression` 节点
>    ⇒ 不镜像就会**多吐一个 `<!---->` 锚点**（黄金样本抓出来的）。
>
> ⚠ 另外记一条**与解析器无关、纯 JS 的坑**：适配层收集节点时写了
> `if (Array.isArray(v)) for (…) if (…) push(c)` 后面跟 `else if (…) push(v)` ——
> `else` 绑到了**内层** if 上，于是"数组子节点进得来、对象子节点永远进不来"
> （192 个节点只进来 7 个），表现是产物**静默退回原文件**。凡 `if` 里套 `for` 里再套 `if`，一律加花括号。
>
> **怎么证明换对了**（三件，缺一不可）：
>
> 1. **18 个 tsx 的产物与换之前逐字节一致**（换之前先把 TS 版产物存成黄金样本）—— 18/18。
> 2. `lite/regress/compiler.mjs` 的 **12 条编译期负例**行为不变（该抛的都抛）。
> 3. `node scripts/gates-web.mjs --full` 五条全绿，且 **`web/dist` 的 chunk hash 一字未变**
>    （`index-BZHZ-pGw.js`）—— 端到端等价的最强证据。
>
> **类型检查那条路**与编译器无关，但同样需要一个 TypeScript 实现：本仓改用
> `tsgo --noEmit`（`@typescript/native-preview`，原生实现，0.5 s），`typescript` 包已从
> `package.json`/lock 删除。换之前核对过 `tsc` 与 `tsmc`（那个 fork）的 `--listFiles`
> 逐行相同 —— 顺带把 `@ts-macro/tsc` 一起删了（§9.2）。
>
> **"tsgo 到底查不查"**（2026-10-01 被问过，实测对拍，不是嘴上保证）：
>
> * 往 `src/` 塞一个故意写错的探针文件（类型不符、strict null、`noUnusedLocals`、
>   找不到的名字、JSX 多余属性、跨行赋值），**tsgo 与 `typescript@5.9.3` 各报 7 条，
>   错误码、行列、文案逐条相同**（TS2322 / TS6133 / TS2552 / TS2322-JSX）。
> * 有错时 `npm run typecheck` **exit=1** ⇒ `gates-web` 那条门会红，不是"永远绿"的空门。
> * 覆盖面：两边 `--listFiles` 都是 **161** 条，差异只有标准库 `.d.ts` 的**安装路径**
>   （tsc@5 在 /tmp 的临时副本 vs tsgo 在 node_modules），**项目文件清单完全一致**。
>
* 覆盖面：两边 `--listFiles` 都是 **161** 条，项目文件清单逐行相同（102 个）。
>
> ⚠ 已知边界：tsgo 目前是 **7.0.0-dev 预览版**，上面对拍覆盖的是"本仓这批文件 + 7 类代表性错误"，
> 不等于穷举了原生实现与 `tsc` 的每一处差异（`--build` 增量、编辑器级 API 等未验证）。
>
> ⚠⚠ **用户口径（2026-10-01）：只要 tsgo。** 不把 `typescript`/`tsc` 加回来做"两条实现互校"——
> 宁可接受上面那条边界，也不要那个依赖。**别自作主张加兜底**；真要加，先问。
>
> ⚠ 不变的：`lite/vite.ts` 的 `enforce: 'pre'` 仍是"谁先谁后"的唯一判据；
> 编译期负例仍是这个框架最重要的门 —— 换解析器时正是它们与黄金样本兜住的。

结构照 Solid 的 `babel-plugin-jsx-dom-expressions`，两处有意不同：`.map()` 直接编成键控
`createFor`（Solid 的 `.map` 是朴素数组 diff，键控要写 `<For>`），以及不做事件委托（§8）。

### 4.1 静态结构 → 模板串，动态位置由**编译器算好路径**

```tsx
<li class={props.n % 2 ? 'odd' : 'even'} title={`n=${props.n}`}>{props.label}:{props.n}</li>
```
```js
const _t0 = _$template('<li><!---->:<!----></li>')   // 动态 class 不进模板；每个动态位置一个 <!----> 锚点
const _n0 = _t0(), _n1 = _n0.firstChild, _n2 = _n0.childNodes[2]   // ← 路径全在编译期算好
```

这是与 Vapor **唯一真正的架构分歧**：Vapor 把动态位置交给运行期走位
（`template(html, n)` + `child/next/txt`），lite 让编译器把 `childNodes[i]` 折进源码。
换来运行时少三个走位函数，代价是生成代码长一点 —— 本项目约 1,100 个节点，
两种都不是瓶颈。

### 4.2 动态绑定 → 一条 effect；静态表达式不包

```js
_$setNodes(_n1, () => props.label, _n2)                              // 子节点：setNodes 自己管 effect
_$effect(() => _$setClass(_n0, props.n % 2 ? 'odd' : 'even'))         // 属性：包一条 effect
_$effect(() => _$setAttr(_n0, 'title', `n=${props.n}`))
```

生成的 helper 一律带 `_$` 前缀（`import { setNodes as _$setNodes } from 'lite'`），
所以**不会与业务源码里自己 import 的名字撞车**。

⚠ **两处与早期设计稿不同，以实现为准**：

* **props 是 getter 不是函数**：产物写 `get 'label'() { return it.label }`，组件里
  `props.label` 直接读 —— 但**不能解构**（解构会丢响应性，读到的是快照）。
* **文本子节点走 `setNodes`，不是 `setText`**：`setNodes` 内部才有"纯文本就只写文本"的
  `setText` 快路。`setText` 留在"运行时内部件"那一档，供 demo/bench 压。
  （早期设计稿的示例里写的是 `effect(() => setText(...))`，那不再是产物形状。）
* 反过来，**子节点绑定外面不再套 `effect`**（`setNodes` 自己就是那条 effect），
  而**属性绑定外面要套**（`effect(() => setClass(...))`）。

**编译期优化**：表达式里不含响应式读取时不包 effect，直接写一次（579 处静态 `class`
就是这么省的）；字面量属性（`rows={3}`）**折进模板串**。

### 4.3 事件 / 组件 / 条件 / 列表 / 插槽

```js
_$on(_n2, 'click', (e) => _$batch(() => handler(e)))          // 事件：直接 addEventListener + 自动 batch
_$createComponent(Row, { get 'label'() { return it.label } }) // 动态 props = getter
_$setNodes(_n0, () => show.value ? _t2() : null, _n3)         // 三元 ⇒ setNodes，**没有** createIf 这个原语
_$createFor(_n2, () => items.value, (it, _i) => _$createComponent(Row, {…}), (it, _i) => it.id, _n5, false)
_$setNodes(_n0, () => _$createComponent(Panel, { 'title': 't' }, { default: () => [ … ] }), _n1)  // 插槽 = 函数
```

`createFor` 的最后两个参数：

* **`anchor`** —— 每个动态子节点自带 `<!---->` 占位注释当锚点（位置钉死，重跑多少次都插在
  同一处）；没有锚点就是 `null` = 追加到末尾。
* **`positional`** —— 编译器**结构判断**渲染体里读没读 `.map()` 的索引参数。
  读了 ⇒ 重排时**重建**那一行（行内容真的依赖位置）；没读 ⇒ 重排时把行**搬走**，
  保住行内焦点与输入框光标。这是"不必兼容 Vue"之后才敢加的这一条 ——
  Vue 的 keyed diff 不区分这两种，只能一律重建。

### 4.4 自动 batch 是编译器的事，不是运行时的

运行时不背调度器（§3.1），所以**编译器给每个事件处理器包一层 `batch()`**。
110 个处理器全是事件驱动的，包起来就把"连续写 N 个信号 ⇒ 刷 N 遍"这条尾巴剪掉了。

### 4.5 错误面：哪些写法会拦、哪些会**静默编错**

⚠ 这张表是最容易踩的，写代码前看一遍（教程 §10 有同一份的展开）。

**会抛错**（带 `文件名:行号`）：

| 写法 | 报的那句 |
|---|---|
| `key` 写在非 `.map()` 返回的元素上（普通元素 / 组件 / 条件分支 / 片段） | 这里的 `key` 什么都不做… |
| `v-if` / `vIf` 这类指令式属性 | 不支持指令式属性（TSX 里没有 Vue 的指令） |
| `@click` 这类 Vue 事件简写 | 源码解析失败（TSX 本身就解析不过；报在 lite 这层，而不是让下游吐一句 `Unexpected token` 的残码） |
| 组件上的 `{...spread}` | 组件上的 {...spread} 未支持 |
| 空元素带子节点（`<img>…</img>` / `<br>x</br>`） | 空元素 `<img>` 不能带子节点（生成 HTML 无法表达） |
| 条件分支的分支不是 JSX 也不是 `null` | 条件分支只支持 JSX 或 null |
| 组件写在静态位置 | 组件只能出现在动态子节点位置 |

⚠ 反过来，**非空元素写自闭合是安全的**：`<label class="x" />` 生成的是
`<label class="x"></label>`（补出闭合标签）。这一条曾经真的是 bug ——
早期生成的是没有闭合标签的 `<label class="x">`，HTML 里没有自闭合，
于是把后面的兄弟全吞进它的子节点，编译期算的 `childNodes[i]` 整体错位
（`lite-postmortem.md` §11.2 第 1 条）。现在它由产物自己保证，不再是写作禁忌。

**会静默编错**（不抛，产物不对）：`class` 数组 / 对象、`style` 对象、模板引用 `ref=`。
项目里 0 处使用 ⇒ 没做检测。**别用**。（小写 `onclick` 同理：只有 `onXxx` 且 `X` 大写才是事件。）

块体 `.map()`（`{items.value.map((it) => { …; return <li/> })}`）**不建 `createFor`** ——
预处理落在 effect 之外，信号读不到会冻住。写成表达式体（真有前置语句就把列表挪出去）。

`xlink:href` / `data-*` / `aria-*` 这些**带冒号或连字符的真属性不是指令**，
负例用例专门守着"别把它们当 `v-` 拦掉"。

## 5. 体积（实测，`node web/lite/size.mjs`）

Vite 8 / Rolldown / es2022 / 默认压缩器生产构建后 gzip：

| 裁剪 | raw | gzip |
|---|---|---|
| 全量（= `src/index.ts` 导出的全部） | 5,598 | **2,456** |
| 再去掉只有 demo/bench 用的内部件 | 5,555 | **2,442** |

参照：Solid 核心 ≈7 KB gzip，Vue Vapor 的地板 16,344 B gzip ⇒ 现在是它们的 1/2.9 与 1/6.7。

⚠ **和上一版文档的差值要交代清楚**：设计稿引过 1,892 / 1,514 B，那是**还带着
`computed` / `store` / Vue 兼容壳**时的读数。那一轮把那三样删了，
同时加上三样：effect 的组件作用域归属、`batch` 的再入排空、循环护栏。
净 **+187 B gzip**（2,620 → 2,808），换来的是真机上那两类
"`insertBefore` 崩溃"与"越刷新内容越多"不会再出现。
**基线是同一台机器、同一工具链、上一个 commit 的 git worktree 量的**，不是估的。

⚠ **再往后（2026-09-30）又瘦了一轮：2,808 → 2,435（−373 B，−13%）**，只删了死码、没删任何功能：

1. **诊断代码改成编译期剥离**（`src/dev.ts` 的 `DEV = import.meta.env.DEV`）。
   `insert()` 里那段"锚点脱开"告警（长文案 + `new Error().stack` 抓栈）挂到
   `if (DEV)` 上，Vite 在 build 时把 `DEV` 换成 `false`、压缩器整块折掉 ——
   **开发态一条不少、生产产物一分不留**。（实测：先把诊断整段挖掉量得 2,471，
   正是现在这个数；也就是说这一条就是全部的 −337 B。）
2. 顺手去掉 `insert()` 那个只给告警用的"调用方标签"参数：四个调用点各传一个字符串
   （`setNodes:text` / `createFor:batch` …），生产构建折掉告警后**字符串仍留在产物里**
   （压缩器没法证明没人再读）。它们想回答的"谁调的"由告警里的 `stack` 直接给出，
   且更精确。
3. 两条 `循环更新` 守卫的文案缩短（它们是**生产路径**上的守卫，不能剥离，只能少写字）。
4. **删死码 `setText`**（−36 B）：编译器不发、运行时内部不调、demo/bench 也没在用，
   只有 `index.ts` 一句"留给 demo 压"的注释在撑门面。⚠ 这条**不影响用户下载量** ——
   它本来就被 DCE 掉了，删完主产物**逐字节不变**（同 hash、120,659 B / gzip 32,267 B）。
   删它是为了别让"注释说得像在用"的代码留着（同类教训见 `authEpoch`）。

⚠ 怎么**复现**这组读数：`node web/lite/size.mjs` 走的是 Vite build ⇒ `DEV=false`，
量到的就是"生产档"；开发档把传给 Vite 的 `define` 改成
`{'import.meta.env.DEV': 'true'}` 再量一次即可 —— **实测开发档 raw 5,840 / gzip 2,708**
（`锚点已不在父节点内`、`锚点脱开` 两条都在），即诊断部分值 **237 B gzip**，
只在开发态付费。

### 5.1 「1.5 KB 怎么变成 2.8 KB 的」——逐 commit 复测（同口径，2026-09-30 量的）

把 `d8b81e4..5c564c9` 里每个**动过 `web/lite/src/`** 的 commit 检出成 `git worktree`，
都用**今天 `size.mjs` 的第一个变体**（`export * from src/index.ts` ⇒ 工具链与裁剪口径
完全一致）重打一遍：

| 时间序 | commit | 那一版加/改了什么 | 全量 gzip | 全量 raw |
|---|---|---|---|---|
| 0 | `d8b81e4` 09-29 08:49 | 首版运行时（`computed`/`field`/`createStore`/Vapor 壳**都在里面**） | **1,662** | 3,764 |
| 1 | `8459883` | 编译器吃下全部 18 个真实源文件 | 1,693 | 3,839 |
| 2 | `60d2148` | 新原语 `lazySlot` + `flushSlots`（片段里"占位先交出去、内容晚一步铺"，diff 见该 commit 的 `dom.ts`） | 1,783 | 4,073 |
| 3 | `8bc977c` | 整包渲染 9 个页面：`setValue` property+attribute 双写、SVG 的 `className` 只读分支、`<select>` 绑定挪到子节点之后、`createFor` 正序插入段 | 2,001 | 4,644 |
| 4 | `b63d356` | 交互回归那一轮（脚本为主，运行时小修） | 2,049 | 4,778 |
| 5 | `746b9c3` | 被移除子树的 effect 要销毁（`owners` 按节点记账 + `disposeTree`） | 2,139 | 5,130 |
| 6 | `10caeb0` | 锚点脱开的护栏（不再让 `insertBefore` 抛穿整次更新） | 2,352 | 5,379 |
| 7 | `f4e1c04` | 陈旧 effect 自毁（"越刷新内容越多"第一道） | 2,421 | 5,568 |
| 8 | `749b1c0` | warn 带上调用点与堆栈 | 2,543 | 5,780 |
| 9 | `974466a` | 碎片槽内容与占位绑定（`lazySlot` 记账 `{fn,nodes,eff}`） | 2,620 | 5,980 |
| 10 | `258a7f6` | 堵上"文本快路径绕过锚点护栏"那个洞 | 2,620 | 5,985 |
| 11 | `5c564c9` 09-30 | **删**兼容壳（`computed`/`field`/`createStore`/`renderEffect`/`defineVaporComponent`）＋**加**组件作用域 `ownedEffects`、`batch` 再入排空、循环护栏 | 2,808 | 6,147 |
| 12 | `c689e9c` | 诊断代码改成**编译期剥离**（`src/dev.ts` 的 `DEV = import.meta.env.DEV`）＋去掉只给告警用的 `where` 参数＋缩短两条守卫文案＋删死码 `setText` | 2,435 | 5,531 |
| 13 | 本次 10-02 | **修内存泄漏**：三张登记表（`owners`/`slots`/`cleanups`）改 `WeakMap`、`disposeTree` 按**整棵子树**跑卸载钩子、`Effect.dispose()` 松开闭包、`own()` 清掉已销毁的 effect | **2,456** | 5,598 |

⇒ 那 **+1,146 B gzip 不是"框架长大了"**：除了第 1~3 档是把语法子集吃全，
第 5~10 档每一档都对应一个**真机上崩过的现场**（详见
[`lite-postmortem.md`](lite-postmortem.md) §12）。第 11 档里"删"与"加"同时发生，
净 +188 B —— 也就是"把兼容壳换成护栏"。第 12 档是**第一次往回走**：
−337 B，且没删任何功能（开发态的告警一条不少，只是不再进生产产物）。

> **你记忆里那个"1.8k"**（准确值 **1,892 B**）出自第 4 档 `b63d356` 那张表的
> "**再删 `computed`**"那一档 —— 同一份源码的**全量**当时是 2,049。
> 所以旧数字本来就比全量小一档（它是"把 0 处使用的东西都摘掉之后"的读数）；
> 而同口径可比的起点是本表第 0 档：**1,662 B**。

⚠ 口径提醒：**raw 才等于二进制差**（`rust-embed` 未压缩内嵌进 `mcloud-server`），
gzip 才是浏览器实际下载量。

## 6. 性能（实测，`node web/lite/bench/run.mjs`）

**测的是编译产物**：`bench/bench.tsx` 是一份正常写法的 TSX，由 lite 插件编译。
以前这里引的是手写目标形态（`bench/lite-side.ts`，已删）—— 那量的是运行时裸速，
"挂载快 10 倍"就是这么来的，口径偏乐观。换成 TSX 之后，编译器产物的质量
（少不少一条 effect、路径有没有静态算好）一起进了测量，数字变差是**应该的**。

best of 3、headless Chrome、1000/2000 行的合成规模：

| 操作 | 编译产物 | 手写目标形态（旧读数，仅作参照） |
|---|---|---|
| 挂载 1000 行 | **2.5–5.9 ms** | 1.2 ms |
| 改一处文本 ×100 | ~0 ms（触到计时精度） | 0.1 ms |
| 改三处文本 ×100 | ~0 ms | 0.2 ms |
| 追加 100 行 ×10 | **4.2–7.0 ms** | 2.6 ms |
| 同序重排 2000 行 ×10（纯 diff） | **1.8–2.5 ms** | 1.7 ms |
| 反转 2000 行 ×10 | **8.6–12 ms** ⚠ | 8.6 ms |
| 卸载 | **4.2–5.0 ms** | 0.1 ms |

三条读数要说清：

1. **每次跑都在动**（同一台机器挂载在 2.5～8 ms 之间跳），看量级别看小数。
   `run.mjs` 退出码非 0 的条件不是"变慢了"，而是**断言红了**（27 条：行数、首末行文本、
   追加/重排是否保留节点身份、卸载后是否清空）—— 数字只有在断言全绿时才有意义。
   卸载那一项从 0.1 ms 变 4 ms 也是同一回事：手写版根本没给 `remove()` 记账。
2. **`batch` 与非 batch 在这档规模下测不出差别**（三项都在亚毫秒、触到
   `performance.now()` 精度）。要拉开差距得给每处文本挂几十条绑定，
   那已经不是本项目的形状 ⇒ 不追求。
3. **反转仍是劣化项**：逐行搬，没有"最小移动集"。绝对值 2000 行反转 ≈1 ms/次，
   而全仓最大的列表是"已注册任务 18 项"那种几十行量级 ⇒ 亚毫秒。
   **什么情况下该回来做**：某一页 routinely 上千行**且**会整段颠倒（追加不在此列）。

挂载为什么比手写版慢：每个列表项多了 `createComponent` + props getter + 一条行内 effect。
这就是"用编译器"的成本，也是这张表该用编译产物来量的理由。

## 7. 验收与门禁

`node scripts/gates-web.mjs`（默认 ≈56 s；`--full` ≈100 s）五条：

| 门 | 命令 | 证明什么 |
|---|---|---|
| typecheck | `npm run typecheck`（**`tsgo --noEmit`**，原生 TS） | 类型与 JSX 命名空间对得上 |
| build | `npm run build` | 全量 TSX 编得过、产物能出 |
| lite 编译期负例 | `node lite/regress/compiler.mjs` | **该抛的都抛了**（§4.5 那 12 条：四种 `key` 位置、两种指令写法、`@click`、块体 map、positional 真假、带冒号的命名空间属性不该被拦） |
| 窄屏布局验收 | `node lite/regress/narrow.mjs` | **真 375 宽视口**下没有横向溢出、账号行没被挤扁、弹窗的 ✕ 够得着、方式 tab 每格 ≥44px（§4.6） |
| lite 真 DOM 验收 | `node lite/regress/all.mjs --skip-build [--quick]` | 真浏览器上的行为对 |

⚠ 没装无头 Chrome 的机器上**后两条**会印 `⊘ 跳过（本机没有无头 Chrome，这条**不算通过**）`
并在摘要里**明说这条没被验证过**，不会冒充绿。CI 的 ubuntu runner 上因此只覆盖前三条
（typecheck / build / 编译期负例都是纯 Node）⇒ **后两条是本地提交前的门**（约定 §9.2）。

> **typecheck 用原生 `tsc` 而不是 `@ts-macro/tsc`（2026-10-01）**
>
> 那个 fork 是迁移期（`ce17969`）留下的，全仓**没有任何 ts-macro 宏用法**，
> 而它是整条工具链里**对 TS 大版本最敏感**的一环：实测把 typescript 换成 7.0.2，
> `tsmc` 第一个崩（`ERR_PACKAGE_PATH_NOT_EXPORTED`，TS 连包导出路径都变了）。
>
> 换之前做过等价性核对，不是"都过了"就算：
> `tsc --noEmit` 与 `tsmc --noEmit` **退出码都是 0，且 `--listFiles` 逐行相同（各 158 个文件）**。
> 换完 5 条门禁全绿，产物 hash 不变（typecheck 不参与产物）。
>
> ⚠ 这**不等于** TS 的耦合解除了：`lite/compiler.ts` 仍然 `import ts from 'typescript'`，
> 而它在 Vite 配置的加载路径上 ⇒ `npm run build` 也依赖这个包（实测把包挪走，三条全崩）。
> 想去掉 TS 5.x 的绑定，要动的是"编译器建在哪棵 AST 上"，与这条无关。

`all.mjs` 包了四样：

* `demo/run.mjs` —— 编译真实 TSX（`ref` / `.value` / 钩子 / `watch` / 插槽 / 片段 / 键控列表）
  → 无头 Chrome → **62 条**行为断言，跑的是编译产物；
* `bench/run.mjs` —— 编译 `bench.tsx`（开发者写法：条件 / 循环 / 事件 / 一次改多个信号）
  → **27 条**行为断言 + 耗时表；判负条件是页面抛错、任一断言 FAIL、或挂载 1000 行 > 500 ms
  （那通常意味着列表从"搬节点"退化成"重建整表"）；
* `regress/interact.mjs --variant ready|login|setup` —— 起 fixture 服务 + CDP 驱动
  （零依赖，Node 自带 `WebSocket`），点完 **56 / 8 / 10 步**，每步做**绝对断言**并检查
  快照确实变了，**任何非 favicon 的页面异常都判负**；
* `regress/compiler.mjs` —— 见上表。

⚠ 断言条数只在这里写**一次**（本节），别散到各处脚本注释里 —— 上一版就是
`20 / 53 / 55 / 59` 四个数字散在四份文档里，源码早涨了两倍而注释一直在说谎。
实时数字看各脚本自己打印的那行（demo 的"✓ 全过（N 条）"）。

**判据是绝对的，不是"两侧一致"** —— 这条是本轮最重要的改动。
`regress/compare.mjs`（与 Vue 逐字符比对）整份删掉，理由写在
[`lite-postmortem.md`](lite-postmortem.md) §11.5 / §12.2：**两侧都坏掉时"一致"也绿**。
上一版就因此藏了三条从没真执行的 ✅：

* `setup` 变体两个口令框的 `autocomplete` 撞车 ⇒ 确认口令填进了口令框，
  **闸门从没放行过**，"放行之后外壳要出现"这条路径从没被测到；
* `/api/login/device` fixture 缺失 ⇒ 账号页 `Promise.all` 整体失败、整页错误态
  ⇒ 页面上唯一的「移除」按钮在**关着的弹窗**里，那一步是空操作；
* 「确认登出」的预期本身写错了 —— `doLogout` 吞掉失败、无论如何都翻回登录页。

现在这三处都有真断言守着。**教训**：每条闸门都要问"它红了会不会真的红"。

## 8. 风险与明确的"不做"

> ### 8.0 框架不够用时：**"改框架"与"绕开"摆上秤，比一比再选**（用户口径 2026-10-01）
>
> 原文：「有些时候前端框架不够用，这时候可以在优化前端框架和避免这个问题上对比择优」。
>
> 意思是：**别默认绕**，也别默认改。三选一，并且把账算给人看：
>
> | 选项 | 付什么 | 赚什么 |
> |---|---|---|
> | **① 绕开**（改页面写法/换控件/改需求） | 只付这一个地方；可能留下"这行代码为什么这么怪"的注释债 | 框架与全站零风险 |
> | **② 改框架**（加语法/加运行时能力） | **每个用户都付字节**（§5 是逐字节量过的）+ 全部页面一起承担回归风险 | N 个调用点从此自然 |
> | **③ 先不做**（明确写进 §8 的"不做"） | —— | 逼出一个不需要该能力的方案 |
>
> 判据（本仓特有，别照搬别处）：
> 1. **字节是全局税**：lite 存在的理由就是体积（§1）。框架侧 +200 B ≈ 全站每个用户 +200 B；
>    页面侧绕一下 ≈ 只在那一个页面里绕。**同一个"绕"在第 3 处出现**，才是"该改框架"的证据
>    （第 1 处是巧合，第 2 处是重复，第 3 处是规律）。
> 2. **能撤的直接试，撤不回的才慎重**：页面级写法随时可改；**改运行时/编译器**动的是全体页面，
>    改完必须跑 §7 的全部门禁（编译期负例 12 条 + demo + bench + 真 DOM），代价不在同一条时间线上。
> 3. **"绕"要写明代价**：注释里写清"为什么这么怪"和"框架侧要什么才能不绕"
>    （例：`.map` 的 `key` 必须挂在 JSX 元素的子表达式位置 —— `compiler.ts:545` 的刻意约束）。
>
> 两个真例（2026-10-01，都在同一次改动里）：
> * **绕开**：lite 编译器不认"块体 `.map` 回调"，也不许把 `.map(...)` 当三元分支
>   （分支只认 JSX / null）。做法是**提成模块级组件 + 外面套一层容器 div**，
>   代价是页面里多两行注释；**没有**去放宽 `branchOk` —— 那是全体页面的语法边界。
> * **换词汇**：兑换页要"分组 tab"，直接写 daisyUI 的 `tabs` / `tab` 结果是**样式静默消失**
>   （`app.css` 的 `@plugin "daisyui"` **exclude** 了 `tab`，它已换成本仓自建实现）。
>   这里比了两条路：把 `tab` 从 exclude 删掉（≈ 再引一份组件 CSS + 多一套心智模型）
>   vs 换成**本仓已有的 `join` + `btn join-item`**（`tasks-page.tsx` 的任务分组 tab 就是它）。
>   ⇒ **选后者**：0 新增字节、全站同一套控件语言。判据就是上面第 1 条。

| 项 | 说明 |
|---|---|
| 只测过 Chrome | Safari / Firefox 未验；用的都是标准 API，但没有实测 |
| 异步组件 / 代码分割 | **不做**：单 chunk 是刻意的（`modulePreload` 垫片也因此关掉） |
| SSR / 水合 | 不做，本项目是纯客户端 |
| 指令、模板引用、`class` 数组/对象、`style` 对象 | 0 处使用，**不做**，也**不检测**（§4.5 的静默区） |
| 事件委托 | 不做：逐元素 listener 在本仓是几百个的量级，而委托要额外付 ~250 B 并引入"事件目标穿过 shadow / `stopPropagation`"这类边界 |
| LIS（整表反转快路径） | 不做，理由同 §6 第 3 条 —— 那是拿"每个用户都付的字节"换"谁都不会遇到的场景" |
| Vue 兼容层 | **已拆**（2026-09-30）：`createVaporApp` / `defineVaporComponent` / `renderEffect` / `computed` / `createStore` / `field`、`vue` 与 `vue-jsx-vapor` 两条 alias、`web/.npmrc` 的 `legacy-peer-deps`。入口现在直接 `mount(App, '#app')` |

## 9. 复现命令（都在仓库里）

```bash
cd web
node lite/size.mjs                   # 运行时体积（两档口径）
node lite/demo/run.mjs               # 编译 TSX demo → 无头 Chrome → 行为断言（条数看它自己那行）
node lite/bench/run.mjs              # 编译 bench.tsx → 无头 Chrome → 耗时表 + 27 条断言
node lite/regress/compiler.mjs       # 编译期负例 12 条（纯 Node，不需要浏览器）
node lite/regress/all.mjs            # 上面几条 + 三个认证态交互全跑
node lite/regress/all.mjs --quick    # 只跑 ready 一个认证态（改脚本时的快速回路）
node lite/regress/preview.mjs [48700]  # 起个静态服务看 dist —— 不碰 3000 那个 launchd 服务
```

临时产物只落 `/tmp` 与 `lite/regress/.check-tmp`、`lite/.size-tmp`（已在 `.gitignore`）。

## 10. 还剩什么

1. ~~写编译器 / 整包接上 lite / 切默认构建~~ ✅ 都完成，lite 现在就是默认运行时。
2. ~~拆 Vue 兼容层~~ ✅ 2026-09-30（本轮）。
3. **`api.ts` 的错误体形状**（本轮发现，**未改**）：后端约定是
   `{ error: "字符串", code: "…" }`，而 `api.ts` 用 `String(body.error)` ——
   上游真给对象时会渲染成 `[object Object]`。改它等于改应用行为，超出"优化框架"的范围，
   留作待办；本轮先把 fixture 改成与文档一致的形状，并把它记在这儿。
4. Safari 实测（§8 第一条风险，需要真机）。
