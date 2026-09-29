# lite —— 给本项目的轻量前端框架（设计）

> **一句话**：把 Vue Vapor 那套"无虚拟 DOM + 编译期优化"的做法，按**本项目实际用到的语法子集**重新实现一遍，
> 运行时从 **16.34 KB gzip 降到 1.43 KB**，挂载快 5～8 倍。

| 指标 | Vue Vapor（现状） | lite | 说明 |
|---|---|---|---|
| 运行时（gzip） | 16,344 B | **1514 B** | 空应用地板，实测见 §5 |
| 挂载 1000 行 | 14.1 ms | **1.7 ms** | 同机同浏览器，best of 3 |
| 纯 diff（2000 行同序重排） | 3.5 ms | **1.2 ms** | 零次 DOM 操作的那一项 |
| 追加 100 行 ×10 | 2.8 ms | 3.2 ms | 基本打平 |
| 整表反转 2000 行 ×10 | 3.4 ms | 7.6 ms | ⚠ 唯一明显劣化项，见 §6.4 |
| 包体（主产物 JS） | 154 KB raw | 待接编译器后量 | 现在是"运行时 + 手写目标形态"阶段 |

**状态**：**运行时 + 编译器都已实现并验收**。运行时 1514 B gzip；
编译器（`web/lite/compiler.ts` + `web/lite/vite.ts`）把真实 TSX 编成目标形态，
TSX 版 demo 的 **20 条行为断言全过**（跑的是编译产物，不是手写目标形态，见 §4.5）。
剩下的工作是**迁一个真实页面**并做像素回归（§10）。

---

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
$ node lite/demo/build.mjs && <headless chrome> --dump-dom file:///tmp/lite-demo.html
✓ 全过（20 条）      # 含：动态绑定、动态 class、插槽铺进子组件、onMounted 晚于入文档、
                     # 键控复用的重排（旧节点 isConnected）、卸载钩子、条件分支、列表标题联动
```

编译器在写的过程中**被验证抓出 4 个真 bug**（都是"读代码看不出来、跑起来才暴露"）：
嵌套 JSX 语句泄进同一作用域导致 `_n0` 重复声明；物化路径时漏了中间跳板节点导致父变量回退成根节点；
`childNodes[i]` 用了逻辑序号而非 DOM 下标（锚点错位，渲染成 `:a1`）；
以及新建列表行"还没入文档 ⇒ `nextSibling` 为 `null`"被误判成"已就位"。

## 5. 体积（实测，同一套工具链）

`node lite/size.mjs`（Vite 8 / Rolldown / es2022 / 默认压缩器生产构建后 gzip）：

| 裁剪 | raw | gzip |
|---|---|---|
| 全量（含 `computed` + `store`） | 3,764 | **1,662** |
| 去掉 `store`（项目用不到，状态就是模块级 `ref`） | 3,503 | 1,557 |
| 再删 `computed`（普查 0 处使用） | 3,389 | **1,514** |

对照：Solid 的核心约 7 KB gzip ⇒ **1.48 KB 是它的 1/5**；Vue Vapor 地板 16,344 B ⇒ **1/11**。

⚠ 口径提醒（与 CSS 那轮同一个坑）：**raw 才等于二进制差**（`rust-embed` 是未压缩内嵌），
gzip 才是浏览器实际下载量。这里的 3.3 KB raw 省下的是二进制；对首屏是 16.3→1.5 KB gzip。

## 6. 性能

### 6.1 实测（headless Chrome，1000/2000 行，best of 3）

| 操作 | lite | Vue Vapor | 比值 |
|---|---|---|---|
| 挂载 1000 行 | **1.7 ms** | 14.1 ms | **8.3× 快** |
| 改一处文本 ×100 | <0.05 ms | <0.05 ms | 打平（触到计时器精度） |
| 改三处文本 ×100（不 batch） | 0.1 ms | 0.1 ms | 打平 |
| 追加 100 行 ×10 | 3.0 ms | 2.9 ms | 基本打平 |
| 同序重排 2000 行 ×10（纯 diff） | **1.1 ms** | 3.5 ms | **3.2× 快** |
| 反转 2000 行 ×10 | 7.6 ms | 3.4 ms | ⚠ 慢 2.2× |
| 卸载 | <0.05 ms | <0.05 ms | 打平 |

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
