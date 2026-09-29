# lite —— 给本项目的轻量前端框架（设计）

> **一句话**：把 Vue Vapor 那套"无虚拟 DOM + 编译期优化"的做法，按**本项目实际用到的语法子集**重新实现一遍，
> 运行时从 **16.34 KB gzip 降到 1.85 KB**（约 1/9），挂载快 10 倍以上。

| 指标 | Vue Vapor | lite | 说明 |
|---|---|---|---|
| 运行时（gzip） | 16,344 B（地板） | **1,892 B** | 空应用地板，实测见 §5 |
| 挂载 1000 行 | 19.7 ms | **1.2 ms** | ⚠ 每次跑都在动，看量级别看小数，见 §6.1 |
| 纯 diff（2000 行同序重排） | 3.6 ms | **1.7 ms** | "零次 DOM 操作"的那一项 |
| 追加 100 行 ×10 | 3.4 ms | **2.6 ms** | |
| 整表反转 2000 行 ×10 | 3.4 ms | 8.6 ms | ⚠ 唯一明显劣化项（缺 LIS），见 §6.4 |

> 📌 这一节是**结论摘要**，随每次实测更新；表格里的数字来自 headless Chrome best-of-3。
> 完整验收清单（9 路由逐字符一致 / 3 个认证态 / 交互 45 步 / 逐像素一致 / demo 57 条）
> 在 §11，复现命令是 `node lite/regress/all.mjs`。
| 包体（主产物 JS） | 154 KB raw | 待接编译器后量 | 现在是"运行时 + 手写目标形态"阶段 |

**状态**：**运行时 + 编译器都已实现并验收**。运行时 1514 B gzip；
编译器（`web/lite/compiler.ts` + `web/lite/vite.ts`）把真实 TSX 编成目标形态，
TSX 版 demo 的 **20 条行为断言全过**（跑的是编译产物，不是手写目标形态，见 §4.5）。
剩下的工作是**迁一个真实页面**并做像素回归（§10）。

---


## 目录

> 这份文档按"设计取舍 → 验收 → 踩坑史"的顺序写；踩坑史（§11、§12）**按时间追加**，
> 保留我们判断错误又纠正的过程，不要当成结论清单。
> 想快速上手读 [`lite-guide.md`](lite-guide.md)（使用教程）；想改运行时/编译器再读本文。

- [2. 语法子集：由普查决定，不是拍脑袋](#2-语法子集由普查决定不是拍脑袋)
- [3. API 与 Vue Vapor 的对照（**词汇表照抄 Vapor**）](#3-api-与-vue-vapor-的对照词汇表照抄-vapor)
- [4. 编译插件：要生成什么（**已实现**）](#4-编译插件要生成什么已实现)
  - [4.1 静态结构 → 模板串](#41-静态结构--模板串)
  - [4.2 动态绑定 → 一个 effect（静态表达式不包）](#42-动态绑定--一个-effect静态表达式不包)
  - [4.3 事件 / 组件 / 条件 / 列表 / 插槽](#43-事件--组件--条件--列表--插槽)
  - [4.4 自动 batch 是编译器的事，不是运行时的](#44-自动-batch-是编译器的事不是运行时的)
  - [4.5 验收：TSX 版 demo 跑 20 条断言全过](#45-验收tsx-版-demo-跑-20-条断言全过)
- [5. 体积（实测，同一套工具链）](#5-体积实测同一套工具链)
- [6. 性能](#6-性能)
  - [6.1 实测（headless Chrome，1000/2000 行，best of 3）](#61-实测headless-chrome10002000-行best-of-3)
  - [6.2 挂载为什么快 8 倍](#62-挂载为什么快-8-倍)
  - [6.3 两处**实测抓出来的**性能缺陷（已修）](#63-两处实测抓出来的性能缺陷已修)
  - [6.4 已知劣化项与原因](#64-已知劣化项与原因)
  - [6.5 测量方法的局限（别把这张表当定论）](#65-测量方法的局限别把这张表当定论)
- [7. 迁移方案](#7-迁移方案)
- [8. 风险与明确的"不做"](#8-风险与明确的不做)
- [9. 复现命令（都在仓库里）](#9-复现命令都在仓库里)
- [10. 下一步](#10-下一步)
- [11. 迁移演练（2026-09-29）：整包接上 lite](#11-迁移演练2026-09-29整包接上-lite)
  - [11.1 前一轮的判断错在哪（记下来，省得下次再错）](#111-前一轮的判断错在哪记下来省得下次再错)
  - [11.2 一共修了七处（全是"静默错"）](#112-一共修了七处全是静默错)
  - [11.3 JSX 空白语义：这次是**量出来的**，不是猜的](#113-jsx-空白语义这次是量出来的不是猜的)
  - [11.4 交互回归：首屏之后才是这套框架与 Vue 差别最大的地方](#114-交互回归首屏之后才是这套框架与-vue-差别最大的地方)
  - [11.5 回归工具（都留在仓库里）](#115-回归工具都留在仓库里)
  - [11.6 交互回归又抓出两处（首屏回归看不见的那种）](#116-交互回归又抓出两处首屏回归看不见的那种)
  - [11.7 一处**已知的故意不同**（Vapor 自己的缺陷，不照抄）](#117-一处已知的故意不同vapor-自己的缺陷不照抄)
  - [11.8 下一步](#118-下一步)
- [12. 切换之后（2026-09-29，真机上跑起来）](#12-切换之后2026-09-29真机上跑起来)
  - [12.1 真机报出的错：被移除子树的 effect 不会销毁](#121-真机报出的错被移除子树的-effect-不会销毁)
  - [12.2 `all.mjs` 在切换之后会自己骗自己](#122-allmjs-在切换之后会自己骗自己)
  - [12.3 真机那个 insertBefore 错：根因与护栏（2026-09-29 补）](#123-真机那个-insertbefore-错根因与护栏2026-09-29-补)
  - [12.4 真机第二个症状："越刷新内容越多" —— 陈旧 effect 必须自毁](#124-真机第二个症状越刷新内容越多--陈旧-effect-必须自毁)
  - [12.5 真机"越刷新内容越多"的真正根因：碎片槽的内容与占位脱钩](#125-真机越刷新内容越多的真正根因碎片槽的内容与占位脱钩)

## 1. 为什么自研，而不是继续用 Vue

不是为了"不要依赖"，是为了**体积与首屏**：主产物 JS 154,441 B（gzip 48,718），其中
Vue 运行时占 **43,656 B（空应用地板，见 architecture.md §10.6）**。这块地板对本项目是纯开销：

* 我们用到的 Vue API **只有 6 个**（下一节的普查表），其余（Teleport / Transition /
  KeepAlive / Suspense / 异步组件 / 指令 / computed）**一处都没用**，却都在产物里留了位置。
* 按 Vue 官方修好 async 组件之前不能拆包（architecture.md §10.5），
  首屏那 154 KB 里能砍的只剩运行时这一块。

## 2. 语法子集：由普查决定，不是拍脑袋

对 `web/src` 全量 28 个文件、7,203 行的普查结果：

| Vue API | 用量 | lite 是否实现 |
|---|---|---|
| `ref` | 14 个文件 / 66 处 | ✅ |
| `onMounted` | 11 个文件 | ✅ |
| `onUnmounted` | 5 个文件 | ✅ |
| `watch` | 1 处（`app.tsx` 盯登录态） | ✅（初始不触发，与 Vue 同语义） |
| `useSlots` | 1 处（`dialog.tsx` 的 `slots.default?.()`） | ✅ |
| `defineVaporComponent` | 1 处（`app.tsx` 根组件） | ✅（保留为恒等函数） |
| `computed` / `reactive` / `nextTick` / `provide` / `inject` / `effectScope` / `toRef` | **0** | ❌ 不实现（`computed` 留了实现，约 100B，不要就删） |

TSX 用到的形态（同样是普查，不是猜）：

| 形态 | 用量 | 备注 |
|---|---|---|
| 静态 `class="…"` | 579 | 直接进模板串 |
| 动态 `class={…}`（模板串 / 三元） | 41 + 18 | 一个 effect |
| `class` 数组 / 对象 | **0** | 不做 |
| `style={…}` | **0** | 不做 |
| 事件 `onXxx` | 110 | 直接 `addEventListener` |
| `key=` | 31 | 键控复用 |
| `{...obj}` 展开 | 16（全是 `<svg {...BASE}>`，模块级常量） | 编译期判定为静态时一次性铺 |
| `ref=`（模板引用） | **0** | 不做 |
| `{cond ? <A/> : null}` | 97 | 三元，不是 `&&`（`{cond && …}` 出现 **0** 次） |
| `list.map(…)` | 41 | 列表 |
| 片段 `<>…</>` | 14 | |
| 指令 / 宏（`v-if` / `v-model` / `v-show`…） | **0** | 全项目没有一处 |
| 内置标签 | div/span/button/p/input/table 系/svg… 共 25 种 | |

**结论：这个框架要支持的东西非常少** —— 6 个 API + 一套 JSX 形态。这也是 1.4 KB 能成立的前提。

## 3. API 与 Vue Vapor 的对照（**词汇表照抄 Vapor**）

源语法**就是 Vue 的**：`ref` / `.value` / `onMounted` / `onUnmounted` / `watch` / `useSlots` /
`defineVaporComponent` / TSX —— 28 个源文件**只需要改 import 那一行**。

编译产物那一层也照着 Vapor 的词汇表来（我把它自己的编译输出调出来对照过，见 §4.3）：

| Vapor 编译产物里叫 | lite | 差异与原因 |
|---|---|---|
| `_template(html, n)` | `template(html)` | 我不传插槽计数：**路径在编译期算好**（`el.firstChild.nextSibling`），不靠运行期走位 |
| `_renderEffect(fn)` | `renderEffect`（= `effect` 的别名） | 同一实现，两个名字都有，免得两套词 |
| `_on(el, "click", h)` | `on(el, "click", h)` | 一致 |
| `_setClass` / `_setAttr` / `_setText` | `setClass` / `setAttr` / `setText` | 一致 |
| `_setNodes(target, () => v)` | `setNodes(parent, () => v, anchor?)` | 签名不同：我传父节点+锚点，不用 fragment 对象 |
| `_createComponent(C, { label: () => v }, slots, true)` | `createComponent(C, { label: () => v }, slots)` | props**同样是函数**（一致）；少一个 `singleRoot` 标记 |
| `_createIf(cond, then, else)` | `createIf(parent, cond, then, else?)` | 签名不同（同上）；**末尾注释锚点由运行时自建** |
| `_createKeyedFragment(keyFn, render)` | `createFor(parent, list, render, keyFn?)` | 语义一致；我名字更直白，因为签名本身不同 |
| `_child` / `_next` / `_txt`（运行期走位） | **没有对应物** | 这是**唯一真正的架构分歧**：见下 |

**关于那一处分歧**：Vapor 把动态位置交给运行期（`template` 记下 n 个插槽，`child/next/txt` 走到它们）；
lite 让**编译器把路径算进源码**。取舍是——lite 的运行时更小（少三个走位函数），
代价是生成的代码更长一点。对本项目（约 1,100 个节点）两种都不构成瓶颈，等编译器写完再按实测定。

## 4. 编译插件：要生成什么（**已实现**）

`web/lite/compiler.ts`（用仓库已有的 `typescript` 解析 TSX，**不引入任何新依赖**）
+ `web/lite/vite.ts`（Vite 插件，`enforce: 'pre'` —— 必须先于 esbuild 拿到带 JSX 的源码）。
结构**照 Solid 的 `babel-plugin-jsx-dom-expressions`** 来：我把它的真实编译输出调出来逐条对过
（`/tmp/solid-probe`，`npm install --no-save` 装到 /tmp、不碰仓库依赖），采纳了四条：
锚点=后面的静态兄弟（`null` 表示末尾）、每个 JSX 表达式编译成立即执行的箭头函数、
动态 prop 用 getter、条件分支不需要专门原语。**两处有意不同**：`.map()` 编成键控
`createFor`（Solid 的 `.map` 是朴素数组 diff，键控要写 `<For>`；本项目源码就是 `.map` + `key=`），
以及暂不做事件委托（Solid 用 `$$click` + `delegateEvents`，见 §8）。

规则如下，每条都配项目里的真实例子。

### 4.1 静态结构 → 模板串

```tsx
// 源（src/ui/stat-row.tsx 的形态）
<div class="stats stats-vertical md:stats-horizontal"><div class="stat"><div class="stat-title">{item.title}</div></div></div>
```
```js
// 生成：模块级只解析一次；动态位置留空，路径在编译期算好
const _t0 = template('<div class="stats stats-vertical md:stats-horizontal"><div class="stat"><div class="stat-title"></div></div></div>')
```

### 4.2 动态绑定 → 一个 effect（静态表达式不包）

```tsx
<span class="stat-value">{item.value}</span>
<div class={on ? 'text-success' : 'text-base-content'}>
```
```js
effect(() => setText(_v0, item.value()))          // 读 props 函数 ⇒ 自动建立依赖
effect(() => setClass(_v1, on() ? 'text-success' : 'text-base-content'))
```

**编译期优化**：表达式里**不含响应式读取**（没有 `.value`、没有 props 函数调用、没有状态模块标识符）时
不包 effect，直接写一次 —— 项目里 579 处静态 class 就是这么省掉的。

### 4.3 事件 / 组件 / 条件 / 列表 / 插槽

```tsx
<button class="btn" onClick={() => run(id)} disabled={busy}>跑</button>
```
```js
on(_b0, 'click', () => batch(() => run(id())))    // ⚠ 自动包 batch，见 §6.3
effect(() => setProp(_b0, 'disabled', busy()))
```

```tsx
<Row key={it.id} label={it.label} />
{show ? <p>有</p> : null}
{items.value.map((it) => <Row key={it.id} .../>)}
<Dialog title={t}><div>…</div></Dialog>
```
```js
createComponent(Row, { label: () => it().label })          // 动态 props = 函数（同 Vapor）
createIf(_p0, () => show.value, () => _t1())               // 三元 → createIf
createFor(_ul, () => items.value, (it) => createComponent(Row, {...}), (it) => it.id)
createComponent(Dialog, { title: () => t.value }, { default: () => [...] })   // 插槽 = 函数
```

### 4.4 自动 batch 是编译器的事，不是运行时的

lite **不引入调度器**（省约 300B）：信号一写就让受影响的 effect 立刻跑。
代价是"连续写 N 个信号 ⇒ 刷 N 遍"，所以**编译器给每个事件处理器包一层 `batch()`**。
实测这一条很值：不加 batch 的"连续改三处 ×100"是 0.2ms，Vue（微任务调度）是 0.0ms；
项目的 110 个处理器全是事件驱动的，包起来就与 Vue 对齐，而运行时不必背调度器。

### 4.5 验收：TSX 版 demo 跑 20 条断言全过

`web/lite/demo/main.tsx` 是**真实 TSX**（`ref` / `.value` / `onMounted` / `onUnmounted` /
`watch` / `useSlots` / 插槽 / 列表 / 条件 / 事件），由插件编译后跑那套行为断言：

```
$ node lite/demo/run.mjs          # 编译 → 无头 Chrome → 打印断言（退出码非 0 = 有失败）
✓ 全过（53 条）      # 含：动态绑定、动态 class、插槽铺进子组件、onMounted 晚于入文档、
                     # 键控复用的重排（旧节点 isConnected）、卸载钩子、条件分支、列表标题联动、
                     # 片段槽反复切换不重复、非空元素自闭合、SVG 上的 class、动态属性名、
                     # 无 key 的 .map() 锚点、JSX 空白语义（见 §11.3）
```

编译器在写的过程中**被验证抓出 4 个真 bug**（都是"读代码看不出来、跑起来才暴露"）；
接上真实应用后又抓出 7 个，见 §11.2：
嵌套 JSX 语句泄进同一作用域导致 `_n0` 重复声明；物化路径时漏了中间跳板节点导致父变量回退成根节点；
`childNodes[i]` 用了逻辑序号而非 DOM 下标（锚点错位，渲染成 `:a1`）；
以及新建列表行"还没入文档 ⇒ `nextSibling` 为 `null`"被误判成"已就位"。

## 5. 体积（实测，同一套工具链）

`node lite/size.mjs`（Vite 8 / Rolldown / es2022 / 默认压缩器生产构建后 gzip）：

| 裁剪 | raw | gzip |
|---|---|---|
| 全量（含 `computed` + `store`） | 4,778 | **2,049** |
| 去掉 `store`（项目用不到，状态就是模块级 `ref`） | 4,474 | 1,933 |
| 再删 `computed`（普查 0 处使用） | 4,360 | **1,892** |

对照：Solid 的核心约 7 KB gzip ⇒ **1.85 KB 是它的 1/3.8**；Vue Vapor 地板 16,344 B ⇒ **1/8.6**。

⚠ 比上一轮涨了约 330 B gzip：都是**修 §11.2 那些静默 bug** 要付的钱 ——
`setValue`（property + attribute 都写）、`setAttr` 的布尔属性白名单、
以及"每次插入后放行已入文档的挂载钩子"（`queueMount` / `flushMounted`）。
省是省在这里，贵也贵在这里：**这几百字节买到的是"行为与 Vue 逐字符一致"**。
后来又涨了 ~50 B：`createFor` 改成"新建的行整段正序插入"（见 §11.6）。

⚠ 口径提醒（与 CSS 那轮同一个坑）：**raw 才等于二进制差**（`rust-embed` 是未压缩内嵌），
gzip 才是浏览器实际下载量。这里的 3.3 KB raw 省下的是二进制；对首屏是 16.3→1.5 KB gzip。

## 6. 性能

### 6.1 实测（headless Chrome，1000/2000 行，best of 3）

| 操作 | lite | Vue Vapor | 比值 |
|---|---|---|---|
| 挂载 1000 行 | **1.2 ms** | 19.7 ms | **16.4× 快** |
| 改一处文本 ×100 | 0.1 ms | 0.1 ms | 打平（触到计时器精度） |
| 改三处文本 ×100（不 batch） | 0.2 ms | <0.05 ms | 打平 |
| 追加 100 行 ×10 | **2.6 ms** | 3.4 ms | **1.3× 快** |
| 同序重排 2000 行 ×10（纯 diff） | **1.7 ms** | 3.6 ms | **2.1× 快** |
| 反转 2000 行 ×10 | 8.6 ms | 3.4 ms | ⚠ 慢 2.5×（缺 LIS） |
| 卸载 | 0.1 ms | <0.05 ms | 打平 |

⚠ 这张表**每次跑都在动**（挂载那一项在 1.2～1.7 ms 之间跳，Vue 侧 14～20 ms），
所以别把某一格的比值当结论 —— 看的是量级与"哪几项真的快"。
`createFor` 改成整段正序插入（§11.6）之后专门复测过：追加与同序重排都**没有**退化。

### 6.2 挂载为什么快 8 倍

因为没有中间层：Vue 每个列表项要建 `createKeyedFragment` + `_createComponent` + `setInsertionState`，
lite 就是 `cloneNode` + 一个 effect + 一次 `insertBefore`。

### 6.3 两处**实测抓出来的**性能缺陷（已修）

1. **`createFor` 的"已就位"判据写错了** ⇒ 每次更新把整个列表 `insertBefore` 搬一遍。
   第一版用正序 + `nodes[0] === cursor`：放完一行 cursor 推进到它的 `nextSibling`，
   而下一行的首节点根本不在那儿 ⇒ 判据永远为真。
   实测"同序重排 2000 行 ×10"要 **10.1 ms**（Vue 3.5）——而那一项**本该是零次 DOM 操作**。
   改成**倒序迭代 + 组尾判定**（`组尾.nextSibling === cursor`）后降到 3.6 ms。
2. **每轮重建 `Map`、每行三次 Map 操作** ⇒ 追加/反转的固定开销。
   改成跨轮复用 `Map`/`Set`（1 次 get + 1 次 add/行）后：追加 9.3→3.2 ms（打平），
   纯 diff 3.6→**1.2 ms（反超 Vue）**。
3. （设计层面）**同步刷新的代价**：不 batch 的连续写会刷多遍 ⇒ 由编译器自动包 `batch()` 解决（§4.4）。

### 6.4 已知劣化项与原因

* **整表反转慢 2.2×**：Vue 的 keyed diff 用最长递增子序列算"最小移动集"，极端逆序时移动次数更少；
  我是逐行搬。绝对值上 2000 行反转 0.75 ms/次，对本项目的列表规模（几十行）无感。
  若将来要治：加一个"逆序检测 + 整体翻转"的快路径。
* 无调度器 ⇒ 非事件路径连续写多个信号会多刷几遍（§4.4 由编译器兜住）。

### 6.5 测量方法的局限（别把这张表当定论）

best of 3、headless Chrome 单机、子毫秒项触到 `performance.now()` 精度；
Vue 侧是"结构等价"的 TSX（同样 1000 行、同样 class），不是逐字节相同的业务代码。

## 7. 迁移方案

1. **应用侧**：`from 'vue'` → `from '../lite'`。其余一行不改（`ref`/`.value`/`onMounted`/
   `watch`/`useSlots`/`defineVaporComponent` 语义一致；组件仍然是普通箭头函数）。
2. **构建侧**：把 `vue-jsx-vapor/vite` 换成 lite 的编译插件（同一份 TSX 源）。
   ⚠ `app.tsx` 的 `defineVaporComponent` 保留 —— lite 里它是恒等函数。
3. **不需要改的**：CSS 与类名一字不动（§4 的生成代码不引入包装节点，布局与选择器不受影响）。
4. **回归方式**：沿用 CSS 那轮的做法 —— 按真实标记搭对照页 + 计算样式逐属性比对 +
   与原基线逐像素比对（已证明能抓到 1 个色阶的差异）。

## 8. 风险与明确的"不做"

| 项 | 说明 |
|---|---|
| 编译器未实现 | **最大风险**。输出规格在 §4 已钉死，实现按 §9 分两步走 |
| 异步组件 / 代码分割 | **不做**：Vue 3.6-rc 上跑不通（architecture.md §10.5），换了框架也不解决——那是上游的事 |
| SSR / 水合 | 不做，本项目是纯客户端 |
| 指令、模板引用、`class` 数组/对象、`style` 对象 | 项目里 0 处使用，**一律不做**（这是"小"的来源） |
| 事件委托 | 暂不做。110 个处理器直接挂；列表行多时值得做（估计 +250B，收益待测） |
| 只测过 Chrome | Safari/Firefox 未验；用的都是标准 API，但没实测 |

## 9. 复现命令（都在仓库里）

```bash
cd web
node lite/size.mjs          # 运行时体积（raw + gzip，三种裁剪）
node lite/demo/build.mjs    # → /tmp/lite-demo.html，页面自测 22 条
node lite/bench/build.mjs   # → /tmp/lite-bench.html，与 Vue Vapor 同页对拍 7 项
```

看结果（macOS 无 `timeout`；用仓库已有的 headless Chrome）：

```bash
"$SHELL_BIN" --headless --disable-gpu --virtual-time-budget=3000 \
  --dump-dom file:///tmp/lite-demo.html | grep -A3 'id="result"'
```

## 10. 下一步

1. ~~写编译器~~ **已完成**（§4.5，20 条断言全过）；
2. **迁一个页面**（最小的是 `settings`）→ 与 Vue 版逐像素比对；
3. 逐页迁移，每页一道像素回归；`api.ts` 与 `ui/*` 不动；
4. 视情况补：事件委托、列表逆序快路径；
5. ⚠ 全程不改 CSS；产物体积每步用 `node lite/size.mjs` + 主产物构建复量。

## 11. 迁移演练（2026-09-29）：整包接上 lite

**手法**：`web/vite.config.lite.ts` 用 `resolve.alias` 把 `vue` / `vue-jsx-vapor` 指到 lite 运行时
—— `src/**` **一个字节都没改**；`web/lite/regress/compare.mjs` 给 lite 包与 Vue 包喂
**同一套 API fixture**（`/api/setup` `/api/session` `/api/version` `/api/status` + 几个列表），
再用无头 Chrome 取 `#app` 子树逐字符比对。

**编译与体积（已达成）**：

| | Vue 包 | lite 包 |
|---|---|---|
| JS raw | 153,771 B | **119,355 B**（−22.4%） |
| JS gzip | 48,412 B | **31,985 B**（−33.9%） |

（比上一轮涨了约 8 KB raw：`<!---->` 占位注释（§11.2 #6）约 7 B × 每个动态子节点。
逐字符一致的那 9 个页面，就是靠它换来的。）

18/18 真实源文件编译通过、产物零残留 JSX（用 TypeScript 重新解析产物来查，不信正则）；
整包 `vite build` 通过、无控制台报错。

**渲染回归（已达成）**：同一套 fixture 下，**9 个页面全部逐字符一致**：

| 路由 | lite `#app` | Vue `#app` |
|---|---|---|
| `#/dashboard` | 10,427 B | 10,427 B |
| `#/accounts` | 10,881 B | 10,881 B |
| `#/tasks` | 9,800 B | 9,800 B |
| `#/exchange` | 9,545 B | 9,545 B |
| `#/live-room` | 11,146 B | 11,146 B |
| `#/schedules` | 12,895 B | 12,895 B |
| `#/pipelines` | 10,610 B | 10,610 B |
| `#/history` | 10,517 B | 10,517 B |
| `#/settings` | 9,377 B | 9,377 B |
| 认证闸 `setup`（建管理员向导） | 1,394 B | 1,394 B |
| 认证闸 `login` | 1,010 B | 1,010 B |

⚠ 这些数字比上一轮**大**，不是因为产物变大，而是因为 fixture **换成真实形状**了
（`RunRecord` 的正确字段、非空的账号/计划/流水线、`hidden` 任务要过滤掉、三个认证态）——
列表为空时各页只渲染空态，等于在测空壳；现在列表、下拉、弹窗里都有数据。
仪表盘上那句 `耗时NaN分NaN秒` 就是旧 fixture 字段名写错留下的痕迹。

**交互回归（已达成）**：`interact.mjs` 用 CDP 在两侧执行**同一串步骤**（44 步 ready /
10 步 setup / 8 步 login），每步之后比对同一份快照（`#app` 子树 + 表单 property + 主题 +
打开的弹窗 + hash）：**全部一致**（其中 2 步是"已知的故意不同"，见 §11.7），
整页截图也**逐像素一致**。覆盖：

* 切全部 9 条路由、点抽屉与 dock、主题来回切；
* 确认弹窗的两条去向：取消 / 确认（后者 DELETE 打到 404 ⇒ 走 `showError`，
  错误弹窗的内容也逐字符比过）、关掉错误弹窗；
* 账号页四种登录方式来回切 + 打字、行内「移除」；
* 计划表单（文本 / 下拉 / 勾选）、直播页（textarea / 数字 / 开关）、任务页下拉回写；
* ⭐ **闸门翻转**：`login` 变体里"口令错 ⇒ 错误提示"然后"口令对 ⇒ `authState` 翻 `ready`"、
  `setup` 变体里"短口令 ⇒ 本地校验拦下"然后"提交成功 ⇒ 放行"，两侧都从认证页换成完整外壳，
  再在外壳里继续切页/切主题/展抽屉 —— **这正是当初整包挂掉的那条路径**。

⚠ fixture 现在要**按方法区分**：`GET /api/session` 是"我登录了吗"（401），
`POST /api/session` 是**登录动作**（口令对回 200）。只按 URL 匹配的话登录永远失败，
"登录成功后放行"这条最要紧的路径就测不到（而它正是当初整包挂掉的地方）。
`lib.mjs` 的 fixture 因此支持三种写法：`{status, body}`、**key 带方法**、
以及 `{handler(body)}`（同一端点按请求内容给不同结果）。

（比对前会抹掉注释锚点与 `data-v-app` / `data-v-<hash>` 这类**框架标记** ——
两边挂的标记不同，但它们不属于渲染结果；截图落在 `/tmp/shots/`。）

### 11.1 前一轮的判断错在哪（记下来，省得下次再错）

上一轮把"页面白屏"归因到**片段里的动态成员不响应式**，并把惰性槽方案回退了。
**那个归因是错的**：`lazySlot` 机制本身是好的（片段用例至今全过），真正的第一个异常是

```
TypeError: Cannot read properties of undefined (reading 'firstChild')   ← 抛在 effect 里
```

而应用写 `authState` 的地方（`api.ts` / `app.tsx`）正好在 `try/catch` 里 ——
异常被**吞掉**，页面全空，`window.onerror` 一声不响。探针里那个"只有第一个槽重跑"的读数
也是假象：第二个槽的 effect 建到一半就抛了，看起来就像"没订阅"。

⇒ **教训**：这个框架里"错了但看起来没错"是常态（顺序错、少一个属性、整块不渲染，
都不一定抛到调用方）。所以 ① 每个静默失败都要有断言守着；② 抓异常不能只挂 `window.onerror`，
这次是靠**临时**把 effect 的异常写进 DOM（`--dump-dom` 带出来）才看到栈的。

### 11.2 一共修了七处（全是"静默错"）

| # | 症状（应用侧） | 根因 | 修法 |
|---|---|---|---|
| 1 | `#app` 全空，effect 抛 `firstChild` of undefined | `<label … />` 这种**非空元素的自闭合写法**生成的是 `<label …>` —— HTML 里没有自闭合，后面的兄弟被解析成它的子节点 ⇒ 编译期算的 `childNodes[i]` 整体错位 | 非空元素一律写闭合标签；空元素带子节点直接报错 |
| 2 | 同上（第二层） | `SVGElement.className` 是**只读**的 `SVGAnimatedString`，无条件写它抛 `TypeError`（本项目图标全是 `<svg>`） | `setClass` 只在 `typeof className === 'string'` 时走 `className`，否则 `setAttribute` |
| 3 | 设置页永远停在"加载中" | `onMounted` 只在 `mount()` 那一刻 flush 一次；**之后**动态建出来的组件（切页、条件分支里的页面）的钩子永远不跑 | 挂载钩子按**实例自己**的节点登记，`insert()` 每次插完就放行"已入文档"的那些 |
| 4 | 弹窗把文本节点当父节点：`HierarchyRequestError` | 相邻文本在 HTML 解析后**合成一个节点**，`childNodes` 下标按"每段文本各占一位"数就会错位 | 用 `textTail` 标记按合并后的下标算 |
| 5 | `aria-current` 等动态属性**根本没写上去** | 生成的是 `setAttr(el, value)` —— **属性名漏了**：`true` 被当成属性名、值成了 undefined ⇒ 走 removeAttribute，静默无操作 | 补上名字（`class`/`value` 是两参数形式，其余三参数） |
| 6 | 抽屉页脚两行顺序反了 | 相邻两个动态子节点都"后面没有静态兄弟可当锚点"⇒ 都追加到末尾，顺序取决于**谁的 effect 后跑**（`backend` 是异步来的） | **每个**动态子节点自带 `<!---->` 占位注释并拿它当锚点 —— 位置钉死，重跑多少次都插在同一处 |
| 7 | 账号/直播页整页不渲染，`TypeError: r is not a function` | 无 `key` 的 `.map()` 生成 `createFor(parent, list, render, anchor)` ⇒ **锚点落进了 `key` 形参**，运行期去调一个节点 | `key` 的位置显式占住（没有 key 就写 `null`） |

另外四处**保真度**修正（不崩，但产物与 Vue 不一致，靠逐字符比对才发现）：

* `value` 要**同时**写 property 与 attribute（`setValue`）—— 只写 property 时
  `<input value={x} readonly>` 这类只读展示框在 DOM 里根本看不到值；
* `aria-hidden={true}` 要序列化成 `"true"`（原来写成 `""`，那不是合法 ARIA 值）；
* `false` 只在**布尔属性**上表示"移除"（`spellcheck={false}` 是 `spellcheck="false"`）；
* 字面量属性（`rows={3}`）**折进模板串**——顺带让属性顺序与 Vue 一致
  （静态属性在模板里按源码顺序，动态属性由 setter 之后追加，折不折决定它排第几）。

### 11.3 JSX 空白语义：这次是**量出来的**，不是猜的

`vue-jsx-vapor` 的空白规则与 Babel 的 JSX 清理**不一样**。为了不再猜，把各种形状的 JSX
过一遍**真实插件**，看它生成的 `template("…")`：

| 源码文本 | Vue 生成 |
|---|---|
| `\n    第一行\n    第二行\n  ` | `第一行\n    第二行`（**行间换行与缩进原样保留**） |
| `\n    正文\n  ` | `正文` |
| `，\n    它们…\n  ` | `，\n    它们…` |
| `a  <b>c</b>  d`（单行） | `a  <b>c</b>  d`（单行原样，**不折叠**） |
| `  `（单行纯空白） | ` `（折成一个空格） |
| `\n    <b>x</b>\n    <i>y</i>\n  ` | `<b>x</b><i>y</i>`（纯空白跨行**整段丢掉**） |

⇒ 规则：整段纯空白时"单行折成一个空格、跨行丢掉"；否则**只裁含换行的首尾空白段**。
`\n  耗时 {x}` 里 `耗时 ` 后面那个空格是**行内空格**，必须留着 ——
第一版按 Babel 的算法把它 trim 掉了，逐字符比对才露馅。

### 11.4 交互回归：首屏之后才是这套框架与 Vue 差别最大的地方

`compare.mjs` 只看首屏（一次 `--dump-dom`）。可事件处理器、编译器自动包的 `batch`、
`createFor` 的增删重排、切页时新建/卸载组件、受控输入框的回写 —— **首屏一条都走不到**。

`lite/regress/interact.mjs` 补上这一段：两侧各起一个 chrome（**CDP 驱动，零依赖** ——
Node 自带 `WebSocket`），执行同一串步骤，每步之后比对同一份快照。快照刻意不只看 DOM HTML，
还含**表单 property**（`input.value` / `checkbox.checked` / `select.value` ——
`outerHTML` 里看不到，但用户看得见）、主题、打开的弹窗与 hash。

写它当场就抓出两处首屏永远测不到的差异（§11.6），也顺手证明了几个陷阱：

* 单步失败不能把整轮带崩：把异常记成该步结果继续跑，最后单独列出"哪几步没真执行"；
* `parseHash` 要 `#/settings`（带斜杠），写 `#settings` 会静默回落 dashboard
  —— "八个路由字节数一模一样"才发现，脚本现在自动补斜杠；
* 每个侧都要**等 DOM 稳定**（连续 3 次探测一致）再看结果，否则 150 ms 的取数延迟会被当成差异。

**逐像素比对**也挂在它上面：DOM 逐字符一致 + CSS 同名同哈希（实测两侧 CSS **字节相同**，
`index-C69a_put.css` 同一个哈希）时像素必然一致 —— 但"必然"不是判据，而且这条闸门能兜住
"DOM 一样、渲染不一样"的类型（内联样式、`:hover`/`:focus` 状态、动画停在半路）。

⚠ 踩过的坑：**不能**用 `compare.mjs` 的 `--screenshot` 做像素比对。按钮的
`transition: background-color` 与虚拟时间预算配合不稳 —— 同一条命令跑两次，
一次 0 个不同像素、一次 1,564 个，而最大色差只有 5 阶（截到了过渡中的不同帧）。
所以像素比对放在 CDP + **真实时间**这一侧，截图前先注入
`*{transition:none!important;animation:none!important}`。结果：**逐像素一致（1280×900）**。

### 11.5 回归工具（都留在仓库里）

* `node lite/demo/run.mjs` —— 一条命令：编译 TSX demo → 无头 Chrome → 打印 **53 条**断言
  （退出码非 0 = 有失败）。以前是手敲 chrome 命令行，既容易漏步骤又要重新踩抓取那三个坑；
* `node lite/regress/compare.mjs --route <页面> [--variant ready|login|setup]` —— 同 fixture 下逐字符比对；
  `--from-raw` 复用上次抓的 `/tmp/lite-raw.*.html`（只调比对规则时不必再跑 chrome），
  原始抓取**一律落盘**；
* `node lite/regress/interact.mjs [--variant …] [--only 关键字]` —— 交互回归（见 §11.4），
  差异时把两侧完整快照落 `/tmp/interact/`；**顺带做逐像素比对**（截图前注入
  "禁用所有 transition/animation" 的样式，见下）；
* 两者共用 `lite/regress/lib.mjs`（fixture 服务、`appSubtree` 归一化、两种抓取方式）：
  **归一化规则必须同一份**，各写一份迟早漂移，而漂移的表现是"都过了但测的不是一回事"。

### 11.6 交互回归又抓出两处（首屏回归看不见的那种）

| # | 症状 | 根因 | 修法 |
|---|---|---|---|
| 8 | 任务页/计划页的 `<select>` 默认选中的项与 Vue 不同 | **`value` 写得太早**：`<select>` 没有独立的选中状态，`value` 是在选项集合上算出来的。先写值（那一刻还没有选项）时属性虽是 `''`，随后插入 `<option>` 会触发浏览器的**自动选中**；Chrome 实测「反着插两个 option」自动选中的是**最后一个** | `<select>` 的 `value` 绑定挪到**子节点绑定之后**（Vapor 也是这么排的：`Z(map…)` 然后 `R(()=>H(e,i.value))`） |
| 9 | 计划页"执行的任务"下拉框两侧默认项不同（应用并未显式赋值时） | `createFor` 倒序遍历时**顺手倒着插**新行 —— 倒序只是为了"看到的都已就位"，不该决定插入顺序。见 §11.7 的 `select` 说明：`append(b); insertBefore(a,b)` ⇒ `value === 'b'` | 连续新建的行攒成一段，到段边界再**正序**、以"这一段后面那个节点"为锚整段插入；复用检测仍走倒序（同序重排 1.7 ms 的性能优势没丢） |

⚠ 第 9 条改的是**所有列表**的插入顺序。这正是"逐字符一致"之外还需要**交互**回归的理由：
首屏两侧可能都对（因为应用随后显式赋了值），而交互之后浏览器替我们"顺手"选的那一项才暴露顺序差。

### 11.7 一处**已知的故意不同**（Vapor 自己的缺陷，不照抄）

计划页里"填名称"会让 **Vue 包的下拉框显示变空**，lite 不会 —— 这不是我们的 bug：

```
Vapor 生成的代码（dist 里读出来的）：
  R(() => { H(名称输入, c.value), H(下拉框, l.value), … })   ← 整个模板的动态属性共用**一个** renderEffect
```

于是"改名称"会**顺带重写下拉框的值**；而此状态下 `fTask` 是 `''`（应用只在"编辑已有计划"时才赋值），
`select.value = ''` 匹配不到任何选项 ⇒ `selectedIndex = -1`、下拉框显示空白。
lite 的属性绑定是细粒度的，改名称不会动下拉框，于是它保留浏览器自动选中的「每日签到」——
**用户看到的是不空的那一个**。功能上两者一致（提交时 `fTask` 都是 `''`，都会提示"请选择任务"）。

要"逐字符一致"就得把这份粗粒度照抄过来 —— 那是**故意做得更差**，不做。
`interact.mjs` 里用一张 `EXPECTED_DIVERGENT` 表把这两步标成"已知差异（有意）"，
并在输出里打印原因；除了这两步，任何一步不一致都会让脚本退出码非 0。

### 11.8 下一步

1. ~~逐像素比对~~ ✅ **已做**：`interact.mjs` 每轮结束时比对整页截图，逐像素一致
   （两侧 CSS 实测字节相同）；`compare.mjs` 那句 `--screenshot` 保留当存档，不参与判定；
2. ~~切默认构建配置~~ ✅ **已切**（2026-09-29，用户在场时执行）：`vite.config.ts` 换成
   lite 的插件与 alias，原 Vue 配置**原样留名**为 `vite.config.vue.ts`，
   一条命令即可退回：`npx vite build --config vite.config.vue.ts` + `cargo build`。
   切换后 3000 上的真实服务实测已在发 lite 产物（`dist/assets/index-Ch0ks5Ve.js`，
   DOM 里没有 `data-v-app`），`web/src/**` 仍未改。
   ⚠ 这份 debug 服务是**从 `web/dist` 读盘**的（实测重编译前后 PID 未变、资源哈希已换），
   所以"要不要重启服务"与"前端换不换"在这里是两件事 —— AGENTS.md 里那条
   "重编译会 SIGKILL 服务"的风险这次没有触发；但**别指望每次都这样**；
3. 迁移期的"不删老配置"原则：`web/src/**` 到现在**一个字节都没改**，随时可以退回 Vue 构建。
3. ~~事件委托 / LIS~~ —— **量过之后决定不做**，理由如下（不是"没空做"）：

   * **LIS（整表反转的快速路径）**：那是 2000 行的合成基准（8.6 ms vs Vue 3.4 ms）。
     真实规模是另一回事 —— 全仓 `createFor` 站点 38 个，而最大的列表是
     "已注册任务 18 项"、历史记录若干条、账号/计划/编排各自几条，都是**几十**量级，
     在那一档整表反转是亚毫秒。为它加 ~300 B gzip 与一段 LIS 记账代码，
     是拿"每个用户都付的字节"换"谁都不会遇到的场景"。
     **什么情况下该回来做**：某一页的列表 routinely 上千行，且会出现整段顺序颠倒
     （不是"追加"，追加已经是 1.3× 快于 Vue）。
   * **事件委托**：省的是"每个元素一个 listener"的内存。本仓 38 个站点 × 几十行的量级下，
     listener 总数是几百个（每个 ~几十字节）—— 而 Vapor 也是逐元素 `addEventListener`，
     所以不委托**不影响与 Vue 的产物一致性**（逐字符/交互/像素三条闸门都证明了这一点），
     委托反而要额外付 ~250 B gzip 并引入"事件目标穿过 shadow/stopPropagation"这类边界。
     挂载性能已经 16× 快于 Vue，这里没有要还的债。



## 12. 切换之后（2026-09-29，真机上跑起来）

### 12.1 真机报出的错：被移除子树的 effect 不会销毁

用户切到 3000 上的 lite 版之后，控制台出现：

```
Failed to execute 'insertBefore' on 'Node': The node before which the new node
is to be inserted is not a child of this node.
```

根因是**架构性的漏**，不是某个页面的 bug：`remove()` 只跑了 `onUnmounted` 那类清理，
**没有销毁 effect**。于是被移除子树（切页、条件分支、列表删行）里的 effect 还订阅着
**全局**信号（`authState` / toast / 后端状态）；信号一变它们照样重跑，
拿着已经不在文档里的 `parent`/`anchor` 去 `insertBefore`。

修法：`dom.ts` 按"谁拥有这个节点"给 effect 记账（`owners`），`remove()` 时
`disposeTree()` 把整棵子树里登记过的 effect **销毁**（`Effect.dispose()` 解绑所有依赖并停用）。
`setNodes` 与 `createFor` 建 effect 时都登记；`createFor` 因此也随列表父节点一起销毁。

⚠ **三条闸门为什么没照出来**：fixture 数据是静止的（加载一次就不动了），
而这条要"移除子树 **之后** 全局信号再变"才会触发 —— 真机上 toast / 状态更新
随时在发生。这是"fixture 换成真机响应"那条 TODO 的直接证据。

⚠ **诚实交代**：这条修复（effect 销毁）**没有**解决用户在真机上报的那个错 ——
`index-BIlSdkDA.js` 上照旧复现。真正的那条是**锚点脱开**，见 §12.3。
（effect 销毁本身仍然是必须的：它防的是"陈旧 effect 继续跑"。）

### 12.2 `all.mjs` 在切换之后会自己骗自己

切换后 `npm run build` 变成了 lite 构建 ⇒ `all.mjs` 里"构建 Vue 侧"那一步其实又构建了一份
lite，于是比对变成 **lite 跟 lite 比**，必然全绿。已改为显式
`npx vite build --config vite.config.vue.ts --outDir /tmp/vue-app`，并把 `--vue /tmp/vue-app`
传给两个比对脚本。

**这是同一天第三次遇到"测试脚本自己的假绿"**（前两次：fixture body 形状写错导致整个变体在渲染登录页、
`THREW` 判定顺序导致 18 步没执行却被印成 ✅）。教训收敛成一句：
**每条闸门都要问"它红了会不会真的红"**，光看它是绿的不算验证。


### 12.3 真机那个 insertBefore 错：根因与护栏（2026-09-29 补）

用户给的堆栈就是运行时那两行：

```js
function m(e, t, n = null) { for (let r of t) e.insertBefore(r, n); x(), v() }
```

⇒ `n !== null` 而 `n.parentNode !== e`：**我们持有的锚点已经不在那个父节点里了**
（锚点是模板里的占位注释，构建时抓下来的节点引用）。抛在这里的后果不只是顺序不对 ——
**整次更新被打断**，后面的 `flushSlots/flushMounted` 都不跑，页面停在半更新状态。

**为什么前三轮一次都没照出来**：
* fixture 数据静止（不会在"摘锚点之后"再触发重渲染）；
* 无头 Chrome 用**干净 profile**，没有第三方，锚点根本不会被摘 —— 我据此说过
  "实测 0 异常"，那句话是**失真**的，撤回;
* demo 里前两版用例都用**纯文本**动态子节点 —— 那条路径走的是 `setText`，
  **碰不到 `insert`**，所以"摘掉防御也不变红"。

**现在有一条真能变红的用例**（`lite/demo/main.tsx`，55 条断言里的一条）：
把动态子节点做成**组件**（`{flag ? <Child/> : null}`，组件才走 createFor/insert），
渲染后人为摘掉它的占位锚点，再 `flag=false → true` 触发一次真正的插入。
摘掉 `insert` 里那两行防御，它就报出**与真机逐字相同**的 `NotFoundError`；装上就过。

**修法与代价**：锚点失效时**退化为追加到末尾**并 `console.warn` 一次（带锚点类型与父节点，
下次能直接认出是谁动的）。取舍写在代码注释里：顺序可能不完美，但比整页炸掉强得多。


### 12.4 真机第二个症状："越刷新内容越多" —— 陈旧 effect 必须自毁

`insert` 的"退化为追加"当时把崩溃换成了**更糟的静默错误**：内容被别处整块替换后，
陈旧 effect 里 `cur` 已经全部脱离文档（`remove(cur)` 成了空操作），它每次变化又把
**新的一份**插进去 ⇒ 刷一次多一份。**响亮地炸掉比悄悄插重复内容好** —— 这个取舍我当时判反了。

真正的修法（`dom.ts` 的 `setNodes` / `control.ts` 的 `createFor`）：**effect 重跑前先看
"我上次插进去的节点还在不在文档里"，全都不在 ⇒ 它已经不属于我们，自己 `dispose()` 掉再返回**。
判据必须在做任何事之前（不能先 remove 再判断）。父节点往往还活着，所以"按父节点记账"的
销毁机制管不到这一类 —— 这两道防线是互补的。

⚠ 这个 bug 的**变红用例我至今没写出来**，三次都栽在同一个坑：动态子节点写成**纯文本**
（走 `setText`，根本不经过 `insert`）。要触发 `insert` 必须满足三条：
① 动态子节点是**组件/元素**；② 锚点脱开**之前已经真插过一次**（`cur` 非空）；
③ 父节点还活着。下次照这三条写，别再试错。


### 12.5 真机"越刷新内容越多"的真正根因：碎片槽的内容与占位脱钩

**先说结论**：§12.3 与 §12.4 里的两次判断都不完整 ——
① 归因"浏览器扩展动 DOM"是**错的**（`runtime.lastError: disconnected port object`
只是扩展自己的噪音，与本 bug 无关）；② 把 `insert` 改成"锚点失效就追加到末尾"
**把崩溃换成了更糟的静默重复插入**，正是用户看到"越刷越多"的直接原因。**响亮的崩溃好过
悄悄多插内容** —— 这个取舍当时判反了，已改回"跳到位置"以外的处理。

真正的根因（在无头里用**真实后端**稳定复现后定位）：

```
初始 266 节点 / 19 卡 → 418/38 → 570/57 → 722/76
每刷一次多一整套页面块（+152 = 19 卡 × 8 节点），div.flex.flex-col.gap-4 变成 3 份
```

`lazySlot` 只把**占位文本节点**交给调用方，碎片内容由槽自己那条 effect 插在占位**后面**。
父槽 re-run 时 `remove(cur)` **只摘掉占位**，碎片内容原地留下；新一轮再 push 新占位 +
新 effect、又插一份 ⇒ 每刷一次多一整套。（这也解释了 warn 里的 `anchor: '#text'`：
占位就是 `document.createTextNode('')`。）

**修法**：按占位节点记账 `{fn, nodes, eff}`；`remove()` 遇到占位就把它的 effect 与
**它插进去的所有节点**一起收掉（递归：碎片里可能还嵌槽）；`disposeTree` 同样处理子树里
嵌着的槽。本地 fixture 复验：刷新 3 次，节点/卡片数纹丝不动（140 / 3）。

⚠ **方法论教训（这轮最值钱的东西）**：我先后给过两个"修复"，都建立在**没有复现的推断**上，
结果让用户多验了两轮错误版本。核心路径的改动必须满足：**先量出可复现的数字**（这里就是
`+19 卡/次`）→ 定位到具体代码路径 → 改 → **同一个测量脚本量到不变**，才推。
"我读代码觉得是这里"不构成证据。


## 13. 与实现不符之处：以教程附录 A 为准

这份文档是**设计稿 + 踩坑史**，几处写法在实现过程中被改掉了，而正文没有全部回改。
写代码前请对照 [`lite-guide.md`](lite-guide.md)（使用教程 —— 它是**面向"把 lite 用到新项目"**
写的，与本仓无关的部分可以直接照用）。已知至少这几处本文与实现不符、且最容易踩：

* **props 不是函数而是 getter**（编译产物是 `get "label"() { return … }`）——
  本文 §3/§4.2/§4.3 里"props 同样是函数"的说法是旧稿；后果是 **props 不能解构**（解构会丢响应性）；
* **编译器不生成 `setText`**：文本子节点一律走 `setNodes`（本文 §4.2 的示例用了 `setText`）；
* `compiler.ts` 顶部注释声称某些写法"直接抛错"，实测只有**组件 spread** 与**空元素带子节点**
  真的抛；`class` 数组 / `class`·`style` 对象 / `ref=` / `v-if` / `v-model` / 小写 `onclick`
  都是**静默编错**（教程 §6 有对照表）；
* 断言条数已从本文 §4.5/§11.5 的 20/53/55 涨到 **57 条**（`node lite/demo/run.mjs`）。

⚠ 教程里的 `路径:行号` 引用是**写作时**的版本；源码改过之后请按**函数名**去找，别对行号。
