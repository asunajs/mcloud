# lite 使用教程（写业务代码用）

> **修订（2026-09-29，晚于本文写作）**：正文与附录里凡提到"**锚点护栏不覆盖文本快路径**"的地方
> （§7.2、附录 A6、附录 B 末条）**已过时** —— 该漏洞已修：`setNodes` 的文本分支不再裸调
> `parent.insertBefore(t, anchor)`，改为走 `insert()`（`web/lite/src/dom.ts`），
> 因此护栏现在覆盖全部插入路径。其余结论不变。


> 面向"要在 `web/src/**` 里写页面/组件的人"。
> 设计取舍、性能数据、以及那一长串踩坑史在 `docs/lite-framework.md`（设计文档），本文不重复，
> 只在需要时指过去。**两份文档冲突时以代码为准** —— 已知的冲突集中列在附录 A。
>
> 代码块的出处约定：
> * 标【demo】的逐字来自 `web/lite/demo/main.tsx` —— 它是真跑的 TSX，
>   `cd web && node lite/demo/run.mjs` 会在无头 Chrome 里跑它那 57 条断言（实测 57/57 全过）。
> * 标【片段】的是为讲解拼起来的片段（不是仓库里的现成文件），
>   但本文写作时用**同一套编译器 + 无头 Chrome** 跑过，附录 B 记了方法。
> * 行号指向本文写作时的版本；源码改了就对函数名，别对行号。

lite 抄的是 Vue 的**源语法**（`ref` / `.value` / `onMounted` / `onUnmounted` / `watch` / `useSlots` /
`defineVaporComponent` / TSX），但它**不是 Vue**：没有虚拟 DOM、没有重渲染、没有
provide/inject、没有 nextTick、没有指令。

---

## 0. 先建立三个心智模型

这三条决定了你写代码时会踩哪些坑，先记住：

1. **组件是普通函数，函数体只跑一次**（`web/lite/src/component.ts:36-42`）。
   `createComponent()` 调一次 `Comp(props, { slots })` 就完事（`web/lite/src/component.ts:49`），
   之后**没有任何"父组件更新 ⇒ 子组件重跑"**这回事。
   所以"在组件体顶层算好的值"是死的 —— 这条在 `web/src/app.tsx:130-136` 有血泪注释。
2. **状态是 `ref`；写 `.value` 只改读过它的那一处 DOM**。
   绑定是编译期生成的细粒度 `effect`（`web/lite/src/dom.ts:262-305`），不存在 diff、不存在 vnode。
3. **更新是同步的**：`.value = x` 立刻刷 DOM，没有微任务队列、没有 `nextTick`
   （`web/lite/src/signal.ts:70-74`）。一次事件里连写多个信号时，编译器已经替你包了 `batch`
   （`web/lite/compiler.ts:568-570`）；**非事件路径**（`await` 回来之后、定时器里）要自己包。

---

## 1. 最小可运行例子

```tsx
// 【片段】形状取自 demo 的 Row 与 App（web/lite/demo/main.tsx:15-23、web/lite/demo/main.tsx:36-88），
// 压成最小可跑的单组件版
import { createVaporApp, ref } from 'vue'   // 为什么是 'vue' 见 §8.2

const Counter = () => {
  const n = ref(0)
  return (
    <div>
      <span id="n">{n.value}</span>
      <button id="inc" onClick={() => n.value++}>+1</button>
    </div>
  )
}

createVaporApp(Counter).mount('#app')
```

这一段就是完整可跑的应用：不需要注册组件、不需要声明依赖、没有 `setup()` 包装。
`{n.value}` 那处会被编译成"往这个文本节点写值"的 effect，点按钮改的就是它
（实测：点一下 `#n` 从 `0` 变 `1`）。

入口文件在仓库里是 `web/src/main.ts`：

```ts
// web/src/main.ts（现状，一行都不用改）
import { createVaporApp } from 'vue'
import App from './app'
createVaporApp(App).mount('#app')
```

⚠ 想拿**卸载句柄**就得用 `mount()` 而不是 `createVaporApp().mount()`：
后者把 `mount()` 的返回值丢掉了（`web/lite/src/component.ts:109-114`），
而 `mount(App, target)` 返回 `() => remove(nodes)`（`web/lite/src/component.ts:99-100`）。

---

## 2. 组件怎么写

### 2.1 组件就是箭头函数

```tsx
// 【demo】web/lite/demo/main.tsx:15-23
const Row = (props: { label: string; n: number }) => {
  onMounted(() => demo.mounted++)
  onUnmounted(() => demo.unmounted++)
  return (
    <li class={props.n % 2 ? 'odd' : 'even'}>
      {props.label}:{props.n}
    </li>
  )
}
```

签名是 `(props, ctx) => 节点`（`Component` 类型见 `web/lite/src/component.ts:25`）：
没有选项对象、没有装饰器、没有 `setup()`、没有泛型组件声明。

**`defineVaporComponent` 是恒等函数**（`web/lite/src/component.ts:103`：`<T>(fn: T): T => fn`）。
保留它纯粹是为了让 `web/src/app.tsx:29` 那处包装不用改 ——
本框架里**所有函数组件地位相同**，没有"打了标记才算组件"这回事，加不加它行为一样。

### 2.2 props 是 getter，所以**别解构**

编译器把动态 prop 写成 **getter**（`web/lite/compiler.ts:205-209`）。`<Row label={it.label} />` 生成的是：

```js
_$createComponent(Row, { get "label"() { return it.label } })
```

于是"**读 `props.label` 的那一下**"才是建立依赖的地方。这带来两条硬规矩：

* 在 JSX 里读：`{props.title}` —— 读发生在绑定的 effect 里，值变了会更新。
  demo 里"列表标题随数据更新"那条断言守的就是它（`web/lite/demo/main.tsx:179`）。
* **不要** `const { title } = props`：那会在组件体那一次把 getter 求值成一个死值，
  之后永远不更新（而且不报错）。

⚠ 另有一处与 Vue 不同：**动态 prop 不是函数**。`props.label` 就是值，不要写成 `props.label()`。
（设计文档 §3/§4.2/§4.3 里"props 同样是函数"的说法已经过期，见附录 A。）

### 2.3 子节点走**插槽**，不走 `props.children`

`<Panel>…</Panel>` 里写的内容编译成 `createComponent` 的第三个实参 `{ default: () => … }`
（`web/lite/compiler.ts:215-216`），子组件用 `useSlots()` 取：

```tsx
// 【demo】web/lite/demo/main.tsx:26-34
const Panel = (props: { title: string }) => {
  const slots = useSlots()
  return (
    <section class="panel">
      <h3>{props.title}</h3>
      {slots.default?.()}
    </section>
  )
}
```

* **`props.children` 永远是 `undefined`**（实测过）：JSX 子节点根本不进 props。
  仓库里 `web/src/ui/dialog.tsx:61-64` 就是为这条写的注释。
* 插槽是**惰性**的：调用方不调用 `slots.default?.()` 就不创建（`web/lite/src/component.ts:11-13`）。
* 调用处可以传 JSX 当 prop：`<ConfirmDialog impact={<span>…</span>} />`
  （`web/src/app.tsx:414-428`），编译器会递归把这段 JSX 编译掉（`web/lite/compiler.ts:229-245`）。

### 2.4 组件的"事件"就是函数 prop

元素上 `onClick={fn}` 会变成真事件监听；**组件**上 `onConfirm={fn}` 只是一条普通 prop
（`web/lite/compiler.ts:189-219` 里没有对 `on*` 做特判）：

```js
_$createComponent(ConfirmDialog, { get "onConfirm"() { return () => doLogout() } })
```

所以 `web/src/ui/dialog.tsx:110` 的 `onClick={props.onConfirm}` 是子组件自己把 prop 接到 DOM 上的。
没有 `emit`、没有 `defineEmits`。

### 2.5 `key` 只在 `.map()` 里有意义

`key` 在组件（`web/lite/compiler.ts:195`）和元素（`web/lite/compiler.ts:293`）上都被**直接丢掉**，
既不进 props 也不进 DOM。它唯一的用途是列表项复用（§4.2）。

---

## 3. 状态：ref / effect / watch / batch / computed

### 3.1 `ref` 是唯一的响应式原语，模块级 ref 就是全局状态

本项目没有 store 的概念：**共享状态 = 一个只导出 ref 的模块**。
两个现成例子：`web/src/ui/auth-signal.ts`（`authState` / `authEpoch` / `currentUser`）、
`web/src/ui/toast.ts`（`toasts`）。`web/src/ui/auth-signal.ts:1-13` 写清了为什么要单独一个模块（避免 import 成环）。

`ref` 的边界（`web/lite/src/signal.ts:87-102`）：

* 读写必须走 `.value` —— 它是访问器属性，不是普通方法。
* 同值写入会被 `Object.is` 挡掉，不触发任何 DOM 写（`web/lite/src/signal.ts:96-102`）。
* **没有 `reactive`/Proxy**：把 ref 放进一个普通对象里没问题，但那个对象的**别的**字段
  不会因此变响应式 —— 要那个效果就用 `createStore`（§3.6）。

### 3.2 `effect` 是"注册一个副作用"，业务代码基本不需要

```ts
const e = effect(() => { /* 立刻跑一次，之后依赖变了再跑 */ })
e.dispose()   // web/lite/src/signal.ts:50-54
```

* 组件里的 DOM 绑定**全是** effect，但那是编译器生成的，业务代码不用手写。
* 手写 effect 的边界：**编译器生成的 effect 有归属记账**（挂在节点上，节点被移除时一起销毁，
  `web/lite/src/dom.ts:54-58`、`web/lite/src/dom.ts:240-254`）；**手写的没有** —— 要自己在 `onUnmounted` 里 `dispose()`，
  否则它会带着死锚点/死引用一直跑（设计文档 §12.1、§12.4 讲的就是这类现象）。
* `renderEffect` 只是 `effect` 的别名（`web/lite/src/index.ts:20-21`），为对齐 Vue Vapor 的词汇表，
  业务代码用不着。

### 3.3 `watch`：初始**不**触发

```ts
// web/src/app.tsx:117-119 的真实用法：登录态一放行就补拉后端状态
watch(authState, (s) => { if (s === 'ready') void loadStatus() })
```

实现上先读一次当前值再比较（`web/lite/src/signal.ts:142-159`），所以：

* **初始不触发** —— 这正是 `web/src/app.tsx` 要的（未登录时不去打 `/api/status`）。
* 同值写入不触发（`Object.is` 比较）。
* `watch` 只能盯**一个 ref**，没有 `watchEffect`、没有 `{ deep, immediate }` 选项。

### 3.4 `batch`：把一批变更合成一次刷新

```ts
// 【demo】web/lite/demo/main.tsx:57-64
<button onClick={() => batch(() => {
  items.value = [...items.value, { id: Date.now(), label: 'x', n: items.value.length + 1 }]
  count.value++
})}>加</button>
```

实测（`node` 里直接跑运行时）：连写三个信号，不 `batch` → effect 跑 **3** 次；`batch` 包起来 → **1** 次。
记住：**事件处理器里编译器已经自动包了 `batch`**（`web/lite/compiler.ts:568-570`），
所以业务代码手写 `batch` 的场景只有"异步回调里连写多个信号"。

### 3.5 `computed`：有，但只用它做模块级派生值

```ts
export const double = computed(() => base.value * 2)   // web/lite/src/signal.ts:161-173
```

边界（都是实测的）：

* **急切**：创建时就求值，不是"读到才算"。
* **每次依赖变更跑两遍**：它的内部 effect 会订阅**自己的输出**（`web/lite/src/signal.ts:168-171` 里读了
  `out.value`，而那时 `active` 正是它自己）。实测：创建时跑 2 次，之后每次变更 +2 次。
* **没有句柄、不能 dispose**（返回值只是个 `Ref`）：所以别在组件里按实例建 `computed`
  —— 组件卸载了它的 effect 还在。放在模块级。
* 项目现状是**一处都没用**（`web/src` 里 grep 不到 `computed`，`web/lite/src/signal.ts:161-163` 也这么写）；
  设计文档把它列为"约 100B，不要就整段删掉"的可裁剪项。**能用，但先问自己要不要**。

### 3.6 `createStore`：可选模块，本项目没用

```ts
const s = createStore({ count: 0 })   // web/lite/src/store.ts:17-23；s.count++ 就是响应式的
```

* 它的做法是把**已存在的键**原地换成 getter/setter（`web/lite/src/store.ts:19-22`），不用 Proxy。
* 边界：**只有初始化时已存在的键才是响应式的**。实测：`createStore({n:0})` 之后
  `s.n = 1` 会触发 effect；再挂一个新键 `s.k = 'x'` 不触发。
* `web/src` 里 0 处使用 —— "状态就是模块级 ref"（`web/lite/src/store.ts:3-5` 自己也是这么说的）。

---

## 4. 控制流

### 4.1 条件：三元表达式，不是 `v-if`

```tsx
// 【demo】web/lite/demo/main.tsx:85
{show.value ? <p id="branch">可见</p> : null}
```

* 编译器把它编成 `setNodes(parent, () => cond ? …, anchor)`（`web/lite/compiler.ts:457-462`）
  —— **没有** `createIf` 这种原语（`web/lite/compiler.ts:30-31`）。
* 两个分支都可以是 JSX / 片段 / 组件 / `null`；`false` 与 `undefined` 等价于 `null`
  （`isNullish`，`web/lite/compiler.ts:606`）。
* 分支是**普通表达式**（不是 JSX）也行：`{n.value > 0 ? '有' : '无'}` 走通用路径
  （`web/lite/compiler.ts:494`）。分支是别的三元也能用（`exprWithJsx` 会递归编译里面的 JSX，
  `web/lite/compiler.ts:229-245`）。
* ⚠ **不要**在组件体顶层写 `if (…) return <X/>`：函数体只跑一次，那个 `return` 只被求值一次，
  界面就永远冻在那一刻。要写在 JSX 表达式里（`web/src/app.tsx:221-257` 有一大段注释讲这个，
  `authGate()` 那个函数就是正确写法）。

### 4.2 列表：`.map()` + `key=`，**且回调必须是"表达式体直接返回 JSX"**

```tsx
// 【demo】web/lite/demo/main.tsx:78-84
<Panel title={`共 ${items.value.length} 条`}>
  <ul id="list">
    {items.value.map((it) => (
      <Row key={it.id} label={it.label} n={it.n} />
    ))}
  </ul>
</Panel>
```

这条路会编成键控 `createFor`（`web/lite/compiler.ts:464-491`）：

```js
_$createFor(_n0, () => items.value, (it, _i) => …, (it, _i) => it.id, _n1)
```

`key` 的作用与边界（`web/lite/src/control.ts:34-109`）：

* 按 key 跨轮复用行；**没有 key 就用下标**（`web/lite/src/control.ts:69`：`key ? key(item, i) : i`）。
* 同一个 key 但**对象换了** ⇒ 重建这一行（`web/lite/src/control.ts:71-75`）。
  所以 key 别用会重复的下标式；用 `it.id` 这种稳定标识。
* 重排是**搬节点**不是重建 —— demo 用"旧节点还在文档里（`isConnected`）"守着这条
  （`web/lite/demo/main.tsx:164-167`）；删行会跑 `onUnmounted`（`web/lite/demo/main.tsx:169-172`）。
* **新建的行是正序插入的**（`web/lite/src/control.ts:56-66`、`web/lite/src/control.ts:87-89`）。这不是审美问题：
  `<select>` 没有独立的选中状态，选项插进一个还没选中项的下拉框时浏览器会自动选中**第一个**
  插入的选项，于是"倒着插"会让默认选中项变成最后一个（`web/lite/src/control.ts:25-32` 有实测记录，
  设计文档 §11.6 第 9 条）。

⚠ **支持子集只有这一种形状**：回调必须是箭头函数，且**函数体直接是一个 JSX 表达式**。
下面两种都会**退化成普通 `setNodes`**（`web/lite/compiler.ts:466-469` 判 `body` 是不是 JSX），
我在编译探针里验证过：

| 写法 | 编译结果 | 后果 |
|---|---|---|
| `{xs.value.map((x) => (<Row key={x.id} />))}` | `createFor` | 键控复用 ✔ |
| `{xs.value.map((x) => { const a = …; return (<Row key={x.id} />) })}` | `setNodes(() => xs.map(…), anchor)` | `key` 被忽略；依赖一变**整块删掉重建**（`web/lite/src/dom.ts:297-299`），里面的 `<input>` 状态、组件实例、`onMounted` 全部重来 |
| `{xs.value.map((x) => rowNode(x))}`（调辅助函数返回 JSX） | 同上 | 同上 |
| `{xs.map((x) => <Row/>)}`（源是静态数组） | 同上 | 没有依赖 ⇒ 只渲染一次（静态列表，没问题） |

注意：**片段作为 `.map()` 的返回值时没有 key** —— 片段不能带属性，编译器显式写 `null`
（`web/lite/compiler.ts:474`、`web/lite/compiler.ts:583-584`），此时按下标复用。

---

## 5. 动态属性、事件、模板引用

### 5.1 属性：静态字面量折进模板，动态的包 effect

* 字符串/数字字面量直接折进 `template()` 的 HTML 串（`web/lite/compiler.ts:312-316`），
  运行期一次写入都没有。⚠ `false`/`null` **不折**（否则 `disabled={false}` 会变成"属性存在⇒打开"）。
* 动态值包一个 effect，只写那一个属性（`web/lite/compiler.ts:555-566`）。
* 分类（编译期决定，`web/lite/compiler.ts:556-563`）：
  * `class` → `setClass`（**SVG 元素走 `setAttribute`**，因为 `SVGElement.className` 是只读的 ——
    `web/lite/src/dom.ts:320-334`，本项目图标全是 `<svg>`）；
  * `value` → `setValue`（**property 与 attribute 都写**，`web/lite/src/dom.ts:365-373`）；
  * `checked`/`selected`/`disabled`/`open`/`multiple`/`readonly`/`required`/`muted` → `setProp`
    （`web/lite/compiler.ts:56`）；
  * 其余 → `setAttr`（`web/lite/src/dom.ts:344-350`：`null` 移除；`false` 只在少数布尔属性上表示移除，
    否则序列化成 `"false"` —— 所以 `aria-hidden={true}` 是 `aria-hidden="true"`）。

### 5.2 事件：`onXxx` → `addEventListener`

```tsx
// 【demo】web/lite/demo/main.tsx:54-56
<button id="inc" onClick={() => count.value++}>+1</button>
```

* 判据是 `name` 以 `on` 开头且**第三个字符是大写**（`web/lite/compiler.ts:318`）；
  事件名取小写（`web/lite/compiler.ts:319`）。处理器收到原生 `Event`。
* 处理器被自动包 `batch`，并原样收到事件对象（`web/lite/compiler.ts:568-570`）。
* 没有事件委托（`web/lite/compiler.ts:33-36`），每个元素一个 listener。
* ⚠ 小写 `onclick={fn}` **不会**被当成事件 —— 它会走 `setAttr`，把函数序列化成属性字符串
  （实测：属性值变成 `() => go()` 的源码）。写错的大小写是**静默**失效。

### 5.3 `{...spread}` 只支持元素

`<svg {...BASE}>` 是项目里的真实用法（16 处）：编译期只在表达式含响应式读取时才包 effect
（`web/lite/src/dom.ts:380-383`）。**组件上**的 `{...spread}` 会**直接抛错**
（`web/lite/compiler.ts:193`：`组件上的 {...spread} 未支持`）。

### 5.4 模板引用（`ref=`）**不支持**

`ref={el}` 不会被特殊处理，会走 `setAttr`，结果是元素上多一个
`ref="[object HTMLInputElement]"` 之类的属性（实测）。

本项目的替代做法是**用 id 找元素**，理由写在 `web/src/ui/dialog.tsx:4-14`：

```ts
// web/src/ui/dialog.tsx:20-29
export const openDialog = (id: string) => {
  const node = document.getElementById(id)
  if (node instanceof HTMLDialogElement) node.showModal()
}
```

---

## 6. 明确不支持 / 会静默编错的写法

编译器**只保证**报两种错：组件上的 spread（`web/lite/compiler.ts:193`）、
空元素带子节点（如 `<img>…</img>`，`web/lite/compiler.ts:425-427`）。
其余"子集外"的写法**不一定抛错** —— 这点与 `web/lite/compiler.ts:38-39` 的说明不一致（附录 A4），
下面这一列是我**在无头 Chrome 里实测**的结果：

| 写法 | 实测结果 |
|---|---|
| `class={['a','b']}` | 不报错。`setClass` 会 `String(v).trim()` ⇒ `class="a,b"`（`web/lite/src/dom.ts:320-334`） |
| `class={{ active: x }}` | 同上（同一条 `String(v)` 路径，推断也是 `class="[object Object]"`） |
| `style={{ color: 'red' }}` | 不报错 ⇒ `style="[object Object]"`。`style` 只有**字符串**能用（就是写 attribute） |
| `ref={el}` | 不报错 ⇒ `ref="[object HTMLInputElement]"` |
| `v-if="x"` / `v-model="x"` / `vShow={…}` | 不报错 ⇒ 原样变成元素上的属性（`v-if="x"`）。`visit()` 里那条 `v[A-Z]` 检查实测**没触发**（JSX 属性总在某个 JSX 元素里，而遇到 JSX 元素就整个 return 了，`web/lite/compiler.ts:628-636`） |
| `onclick={fn}`（小写） | 不报错 ⇒ 属性值是函数源码 |
| `<Foo {...p} />` | **抛错**（`web/lite/compiler.ts:193`） |
| `<img>…</img>` | **抛错**（`web/lite/compiler.ts:425-427`） |
| `<label class="x" />`（非空元素自闭合） | 正常 ⇒ 生成 `<label class="x"></label>`（HTML 里没有自闭合，见 `web/lite/compiler.ts:414-424`） |
| `{cond && <b>x</b>}` | 编成 `cond && (…)()`，`false` 时是空内容（`createNodes` 会滤掉布尔，`web/lite/src/dom.ts:24-26`）。项目里 0 处，**建议还是写三元** |
| 异步组件 / 代码分割 / SSR / 水合 | 不做（设计文档 §8） |
| Teleport / Transition / Suspense / KeepAlive / 指令 | 不做 |
| `provide` / `inject` / `reactive` / `nextTick` / `effectScope` / `toRef` | 没有导出。**注意**：从 `'vue'` 导入它们时，类型检查会过（那是真 vue 的类型），但 `vite build` 会报 `[MISSING_EXPORT] "nextTick" is not exported by "lite/src/index.ts"`（实测）—— 错在构建期，比错在运行期好 |

---

## 7. 与 Vue 的行为差异

### 7.1 比 Vapor 更"细"，这是刻意的

Vapor 把一个模板里的动态属性塞进**一个** `renderEffect`；lite 是**一个绑定一个 effect**。
设计文档 §11.7 记了一处因此产生的**已知故意不同**（`web/lite/regress/interact.mjs:166-169`
把它标进 `EXPECTED_DIVERGENT`，输出里会打印原因）：

> 计划页里"填名称"会让 **Vue 包的下拉框显示变空**，lite 不会 ——
> `select.value = ''` 匹配不到任何选项 ⇒ `selectedIndex = -1`。功能上两者一致
> （提交时都是 `''`，都会提示"请选择任务"），要"逐字符一致"就得把那份粗粒度照抄过来，
> 那是**故意做得更差**，不做。

### 7.2 真机上暴露过的三类问题（写业务代码时要顺着这些习惯写）

设计文档 §12 记录了三条，都属于"架构性的、不是某个页面的 bug"。理解它们能避免你写出同样的形状：

| 现象 | 根因 | 你要遵守的 |
|---|---|---|
| 被移除的子树里的 effect 还订阅着全局信号，信号一变就抛 `insertBefore … is not a child of this node`（§12.1） | `remove()` 只跑 `onUnmounted`，不销毁 effect | 现在由 `web/lite/src/dom.ts` 的 `owners` 记账自动销毁（`web/lite/src/dom.ts:54-70`）。**别指望"组件卸载了，我那条手写 effect 还在跑"** —— 手写 effect 也不会被自动销毁（§3.2） |
| "越刷新内容越多"（§12.4、§12.5） | 内容被别处整块替换后，陈旧 effect 继续把新的一份插进去 | `setNodes`/`createFor` 有"陈旧 effect 自毁"判据（`web/lite/src/dom.ts:274-277`、`web/lite/src/control.ts:48-55`）。**别在框架之外手动搬动这些节点** |
| 碎片槽（`<>…</>`）内容与占位脱钩 ⇒ 每刷一次多一整套（§12.5） | `lazySlot` 只把占位交给调用方 | 已修：按占位记账 `{fn, nodes, eff}`（`web/lite/src/dom.ts:174-192`、`web/lite/src/dom.ts:240-243`）。**返回多成员片段的组件**才会生成槽（`web/lite/demo/main.tsx:323-355`） |

⚠ 另外两条与设计文档措辞不完全对得上的地方，以代码为准：

* `insert()` 里那条**锚点脱开 ⇒ 退化为追加到末尾 + `console.warn` 一次**的护栏**仍然在**
  （`web/lite/src/dom.ts:91-112`），demo 也还在断言它（`web/lite/demo/main.tsx:291-321`）；
  而设计文档 §12.5 说这个取舍"已改回"（附录 A6）。
* 那条护栏**只覆盖 `insert()` 这条路径**。纯文本的动态子节点走的是 `setNodes` 里的
  **文本快路径**（`web/lite/src/dom.ts:284-296`）：优先直接改 `.data`，需要新建文本节点时它是
  **自己** `parent.insertBefore(t, anchor)`（`web/lite/src/dom.ts:291-295`），**不经过** `insert()`。
  我实测复现了这点：先把占位锚点摘掉、再让"空 → 文本"走一次，抛
  `NotFoundError: … is not a child of this node`。⇒ 别再往这个方向加防御性代码了，
  正路是"不要动框架持有的 DOM"。

### 7.3 其它差异

* **没有 `nextTick`**：更新同步完成，读 DOM 就是新值。要"等 DOM 好了再做事"用 `onMounted`
  （它在节点**进文档之后**才跑，`web/lite/src/dom.ts:114-133`、`web/lite/src/component.ts:61-65`）。
* **没有重渲染**：不是"性能更好"，是"这套语义不存在" —— `web/src/app.tsx:130-136`
  那段注释（"这三件必须是函数，不能是 `const x = …` 算好的值"）就是它的直接后果。
* `onUnmounted` 的触发条件是"组件的节点被 `remove()` 撤掉"（`web/lite/src/dom.ts:225-234`），
  与 Vue 的"组件实例卸载"含义一致，但**没有** `onBeforeUnmount`、`onUpdated` 之类的钩子。

---

## 8. 构建与运行

### 8.1 Vite 怎么接

`web/vite.config.ts` 现在就是 lite 配置（这份是默认配置）：

```ts
// web/vite.config.ts（节选）
import lite from './lite/vite.ts'
export default defineConfig({
  plugins: [tailwindcss(), lite({ runtime: 'lite' })],
  resolve: { alias: [
    { find: /^lite$/, replacement: new URL('./lite/src/index.ts', import.meta.url).pathname },
    { find: /^vue$/, replacement: new URL('./lite/src/index.ts', import.meta.url).pathname },
    { find: /^vue-jsx-vapor$/, replacement: new URL('./lite/src/index.ts', import.meta.url).pathname },
  ] },
})
```

* 插件必须 `enforce: 'pre'`（`web/lite/vite.ts:29`）—— 它要先于 Vite 自己的 esbuild 拿到
  **还带着 JSX 的**源码，把 JSX 折掉、保留 TS 语法，再交给 esbuild 去类型。
* 插件只处理 `.tsx`，并跳过 `node_modules`（`web/lite/vite.ts:26`、`web/lite/vite.ts:31`）。
* `runtime: 'lite'` 决定**生成代码里那个 import 从哪来**（`web/lite/vite.ts:25`）。
  必须写成裸名 + alias：生成代码可能落在任意深度的源文件里，相对路径会解析到不同地方。
* 生成代码里的 helper 全部带 `_$` 前缀（`web/lite/compiler.ts:137-140`），
  因为业务文件自己也会 import 同名函数 —— 不带前缀会报
  `Identifier 'batch' has already been declared`。

### 8.2 业务代码里 import 写什么

| 你想用的 | 写什么 | 依据 |
|---|---|---|
| `ref` / `computed` / `watch` / `onMounted` / `onUnmounted` / `useSlots` / `defineVaporComponent` / `createVaporApp` | `import { … } from 'vue'` | 构建时由上面的 alias 指到 lite；类型检查时用**真 vue** 的 `.d.ts`。`web/src` 现有 16 个文件全是这么写的 |
| lite 独有的（`batch` / `createStore` / `field` / `createFor` / `setNodes` …） | 相对路径，例如 `import { batch } from '../../lite/src/index'` | 实测 `tsc` 能解析 `<web>/lite/src/index.ts`（demo 就是这么写的：`from '../src/index'`，它能进 `web/tsconfig.json` 的 include） |

⚠ **裸名 `'lite'` 在类型检查侧解析不了**：`web/tsconfig.json` 里**没有 `paths`**，
而 `resolve.alias` 只作用于 Vite。实测（复刻 tsc 的模块解析）：从 `web/src/app.tsx`
解析 `'vue'` / `'vue-jsx-vapor'` 各自命中 `node_modules` 里的真包，
解析 `'lite'` → **UNRESOLVED**（`npm run typecheck` 会报 TS2307）。
要用 lite 独有 API 就用相对路径，或者给 tsconfig 加 `paths`（那要改配置，谨慎）。

⚠ 还有一条容易误伤的：`'vue'` 的类型里**也有** `createComponent`（来自 `@vue/runtime-vapor`），
但签名与 lite 的完全不同（vue 返回 `VaporComponentInstance`，lite 返回 `Node[]`）。
**别手写编译器那一层的 helper**（`createComponent` / `setNodes` / `insert` / `on` / `template`）——
那是生成代码该用的东西。

### 8.3 构建顺序：先前端，后 Rust

```bash
cd web && npm run build        # = vite build，产物落 web/dist
cd .. && cargo build           # rust-embed 在【编译期】把 web/dist 嵌进二进制
```

`rust-embed` 的目录是 `crates/mcloud-server/src/static_files.rs:66-67` 的
`#[folder = "../../web/dist"]`。顺序反了会内嵌**旧**资源（白屏 / chunk 404），
而 `cargo build` 还会说 "Finished"。

* `web/dist` 不入库 ⇒ 干净检出后**必须**先 `npm run build`，否则资产集是空的
  （不 panic，只是没有页面可发，`crates/mcloud-server/src/static_files.rs:22-23`）。
* `web/lite/**` 的改动**不需要**重跑 `cargo build`：debug 构建下 rust-embed
  **每次 `get()` 都去读盘**（`crates/mcloud-server/src/static_files.rs:12-20`，2026-09-28 核实过）。
  ⚠ 但 release/交付构建仍是编译期内嵌，别把这条推广过去。
* ⚠⚠ 重编译 `target/debug/mcloud-server` 会 SIGKILL 正在跑的服务并触发 launchd 重拉
  —— 开发期自检优先用 `cargo check --workspace --all-targets`（详见 `AGENTS.md`）。

### 8.4 切回 Vue

```bash
cd web && npx vite build --config vite.config.vue.ts   # 原 Vue 配置原样留着，同样写 web/dist
cd .. && cargo build
```

`web/vite.config.vue.ts` 就是原来那份（`vue-jsx-vapor/vite` + `vue` 依赖），
`web/src/**` 一个字节都不用改（它 import 的仍然是 `'vue'`）。

### 8.5 门禁与回归

```bash
node scripts/gates-web.mjs          # 仓库根目录：typecheck + build（只有这两条）
cd web && node lite/demo/run.mjs    # 编译 TSX demo → 无头 Chrome → 57 条断言（实测 57/57）
cd web && node lite/regress/compare.mjs --route settings   # 与 Vue 包逐字符比对
cd web && node lite/regress/interact.mjs                   # 交互 + 逐像素比对（含已知故意差异）
```

`npm run typecheck` 是 `tsmc --noEmit`（`web/package.json`），
`web/tsconfig.json` 里 `"jsx": "preserve"` + `"jsxImportSource": "vue-jsx-vapor"`
—— 所以 **TSX 的类型来自 `vue-jsx-vapor`**，不是 lite。类型能过不代表 lite 支持（见 §6）。

---

## 9. 容易踩的坑（速查）

1. **组件体只跑一次**：顶层 `const x = 算出来的值` 是死的；要响应式就在 JSX 里读，
   或者写成函数（`web/src/app.tsx:130-157`）。
2. **别解构 props**（§2.2）。
3. **别在顶层 `if (…) return <X/>`** 做认证闸/路由 —— 写进 JSX 三元（`web/src/app.tsx:223-257`）。
4. **`key` 只在 `.map()` 里有意义**，且 `.map()` 回调必须**表达式体直接返回 JSX**（§4.2）。
5. **列表 key 用稳定 id**：同 key 换对象会重建那行（`web/lite/src/control.ts:71-75`）。
6. **纯文本的动态子节点走 `.data` 快路径**（不建节点、不经过 `insert()`）；
   **组件/元素**才走 `insert()`（才有锚点护栏）。`setText` 这个导出**编译器不生成**
   —— 它只在手写的目标形态（`web/lite/bench/lite-side.ts`）里用（附录 A3）。
7. **`<select>` 的 `value` 不用自己排顺序**：`<select>` 上的 value 绑定被编译器挪到
   子节点绑定**之后**（`web/lite/compiler.ts:432-446`），照 Vapor 的做法。你只要正常写
   `value={x}` + `.map()` 出 `<option>`（`web/src/pages/schedules-page.tsx:478-490`）。
8. **动态属性别忘了名字会漏**：这条是历史 bug（`web/lite/compiler.ts:559-562`），现在编译器生成的是
   三参数形式，你不用管；但**别手写** `setAttr` 之类的 helper。
9. **卸载后手写 effect 不会自动销毁**（§3.2）；`computed` 连 `dispose` 句柄都没有（§3.5）。
10. **事件名大小写**：`onClick` 是事件，`onclick` 是字符串属性（§5.2）。
11. **别动框架持有的 DOM**：占位注释锚点、模板克隆出来的子树 —— 第三方脚本/手写代码挪走它们
    会让更新退化甚至抛错（§7.2）。
12. **`class` 只接字符串/三元**，数组与对象会静默编错（§6）。
13. **`style` 只接字符串**，对象会变成 `[object Object]`（§6）。
14. **模板引用不支持**，用 id（§5.4）。
15. **别从 `'vue'` 导入编译器 helper**（§8.2）。

---

## 附录 A：与 `docs/lite-framework.md` 不一致的地方（以代码为准）

写这份教程时逐条对过源码与实测，下面这些**设计文档里的说法已经过期**：

| # | 设计文档怎么说 | 代码/实测是什么 |
|---|---|---|
| A1 | §3 的对照表写"props **同样是函数**（一致）"，§4.2 的例子是 `item.value()`，§4.3 是 `{ label: () => it().label }` | 编译期把动态 prop 写成 **getter**：`{ get "label"() { return it.label } }`（`web/lite/compiler.ts:205-209`；`web/lite/compiler.ts:28-29` 自己写着"Solid 用 getter、Vapor 用函数"）。业务代码写 `props.label`，**不是** `props.label()` |
| A2 | §3 的对照表把 `_createIf` 映射到 `createIf(parent, cond, then, else?)` | **没有** `createIf`：`web/lite/src/index.ts` 不导出它，`web/lite/compiler.ts:30-31` 明确说"运行时里没有 createIf"。条件就是 `setNodes`（`web/lite/compiler.ts:457-462`） |
| A3 | §4.2 的生成代码示例用 `setText(_v0, item.value())` | 编译器**不生成** `setText`：`emit()`（`web/lite/compiler.ts:550-590`）没有文本分支，文本子节点走 `setNodes`（`web/lite/src/dom.ts:262-305`，字符串命中 284-296 的文本快路径）。`setText` 只在 `web/lite/bench/lite-side.ts:28,56-58` 这种手写目标形态里用 |
| A4 | `web/lite/compiler.ts:38-39` 说子集外的写法（`v-*`、`class` 数组/对象、`style={}`、模板引用、组件 spread）"**直接抛错**，不静默编错" | 实测只有**组件 spread**（`web/lite/compiler.ts:193`）与**空元素带子节点**（`web/lite/compiler.ts:425-427`）会抛；`class` 数组/对象、`style` 对象、`ref=`、`v-if`/`v-model` **都不抛**，会静默编成一个错误的值（§6 表，无头 Chrome 实测） |
| A5 | §4.5 写"20 条断言"，§11.5 写"53 条"，§12.3 写"55 条" | `web/lite/demo/main.tsx` 里有 **57** 个 `ok(...)`；`node lite/demo/run.mjs` 实测输出 `✓ 全过（57 条）` |
| A6 | §12.5 说"锚点失效就追加到末尾"这个取舍"已改回" | `insert()` 里那段护栏**仍在**（`web/lite/src/dom.ts:91-112`：锚点不在父节点里就退化为追加 + `console.warn` 一次），`web/lite/demo/main.tsx:291-321` 也还在断言"锚点被摘掉时不抛错"。护栏只覆盖 `insert()` 路径，文本快路径绕开它（§7.2，我实测复现了 `NotFoundError`） |
| A7 | `web/lite/src/index.ts:14-16` 的头注释说"**没有** computed / reactive / …" | 同一文件 `web/lite/src/index.ts:19` 导出了 `computed`、`web/lite/src/index.ts:25` 导出了 `createStore`（`web/lite/src/store.ts`）。有，但项目里 **0 处**使用（`web/src` 里 grep 不到），`web/lite/src/signal.ts:161-163` 也把它注成"可整段删掉"的可裁剪项 |

---

## 附录 B：这份教程里的结论是怎么验证的

每条结论的验证手段分三类，正文里都指到了具体位置：

1. **读源码**（行号即证据）：`web/lite/src/*.ts`、`web/lite/compiler.ts`、`web/lite/vite.ts`、
   `web/src/**`、`crates/mcloud-server/src/static_files.rs`。
2. **编译器探针**：用仓库自带的 `typescript` 与 `web/node_modules/.bin/jiti` 直接调
   `compile()`（`web/lite/compiler.ts:622-641`），把文中的写法逐个编出来看产物
   —— §4.2 那张"支持子集"表、§5 的属性分类、§6 的"支持/抛错"两列都是这么定的。
3. **浏览器探针**：用同一个 lite 插件在 `/tmp` 里构建一个自包含页面，
   用仓库已有的 chrome-headless-shell `--dump-dom` 跑断言（与 `web/lite/demo/run.mjs` 同法）。
   §1 的最小例子、§2.3 的 `props.children === undefined`、§3.4 的 `batch` 次数、
   §3.5 的 `computed` 跑两遍、§3.6 的 store 新键、§4.2 的 select 默认选中项、
   §6 的那四条"静默编错"、§7.2 的文本快路径 `NotFoundError`，都是这么实测出来的。
   探针文件都在 `/tmp`，**没有**进仓库。

唯一没能实测、只能读代码推断的一条，正文里已标出：§7.2 那条"护栏不覆盖文本快路径"的**一般性**
结论（我复现的是"清空后重建文本"这一个具体路径；其它路径未逐一穷举）。
