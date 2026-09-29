# lite 使用教程

lite 是本仓自研的前端框架，代码在 `web/lite/`：`src/` 是运行时，`compiler.ts` + `vite.ts` 是 TSX 编译插件。
它只需要 Node + Vite，**跟 Rust 没有关系**。

它抄的是 Vue 的**源语法**（`ref` / `.value` / TSX / `onMounted`…），但**没有虚拟 DOM、没有重渲染**：
模板在编译期变成一个本地节点，之后只有"读过某个状态的那一处 DOM"会被更新。

你会写到的 API 就这几类（`web/lite/src/index.ts` 的全部导出）：

| 分类 | API |
|---|---|
| 状态 | `ref` `computed` `watch` `batch` `effect` `field` `createStore` |
| 组件 | 普通函数 + `useSlots` `onMounted` `onUnmounted` `mount` `createVaporApp` `defineVaporComponent` |
| 编译器生成的 helper | `template` `setNodes` `insert` `setClass` `setAttr` `setProp` `setValue` `on` `spread` `createFor` `createComponent` `createNodes` `lazySlot` `remove` `onRemove` `renderEffect` |

第三类**只有编译产物在用**，业务代码不要手写（原因见 §9）。

---

## 1. 上手：一个计数器

```tsx
import { createVaporApp, ref } from '../lite/src/index'

const Counter = () => {
  const n = ref(0)
  return (
    <div>
      <span>{n.value}</span>
      <button onClick={() => n.value++}>+1</button>
    </div>
  )
}

createVaporApp(Counter).mount('#app')
```

没有组件注册、没有依赖声明、没有 `setup()`。点一下按钮时发生的事：

* `onClick` 编译成一次 `addEventListener('click', …)`；
* `{n.value}` 编译成一个只写这个文本节点的绑定：`_$setNodes(_n0, () => n.value, _n1)`；
* 写 `n.value` 是同步的，那个文本节点立刻就变了。

**import 写什么**：`../lite/src/index` 是运行时入口（`web/lite/src/index.ts`）。
构建期也可以用裸名 `lite`（本仓 `vite.config.ts` 配了 alias）；但裸名要过 `npm run typecheck`，
得在 `tsconfig.json` 里补 `paths`，否则 tsc 解析不到（报 `TS2307`）。
⚠ 本仓 `web/src/**` 里那些 `from 'vue'` 是迁移期的兼容写法（alias 把它们指到 lite）—— **新代码不用跟**。

完整的可跑样例在 `web/lite/demo/main.tsx`：`cd web && node lite/demo/run.mjs`
会编译它、在无头 Chrome 里跑 57 条行为断言。

---

## 2. 组件

### 2.1 组件就是函数，而且只跑一次

签名 `(props, ctx) => 节点`。**函数体只跑一次**（`web/lite/src/component.ts:43-70`），
之后没有"父组件更新 ⇒ 子组件重跑"这回事，更新由组件内部那些细粒度绑定各自负责。

⇒ 直接后果：**在组件体顶层算出来的值是死的**。

```tsx
// ✗ 永远停在第一句：这里只求值一次
const label = loading.value ? '正在加载…' : '完成'
return <span>{label}</span>

// ✓ 写进 JSX：读发生在绑定里，值变了会更新
return <span>{loading.value ? '正在加载…' : '完成'}</span>
```

`defineVaporComponent(fn)` 是**恒等函数**（`web/lite/src/component.ts:103`），
存在只为兼容旧写法 —— 新代码直接写箭头函数就行。

### 2.2 props

props 是普通对象；**动态 prop 在编译期写成 getter**（`web/lite/compiler.ts:205-209`）：

```tsx
<Row label={it.label} />
// 编译成：createComponent(Row, { get "label"() { return it.label } })
```

所以"读 `props.label` 的那一下"才是建立依赖的地方：

* 在 JSX 里读：`{props.label}` ✔
* `const { label } = props` ✗ —— 组件体那一次把 getter 求值成了死值，之后不再更新（而且不报错）。
* `props.label` 就是**值**，不是函数。不要写 `props.label()`。

### 2.3 子节点 = 插槽

`<Panel>…</Panel>` 的子节点编译成 `createComponent` 的第三个参数 `{ default: () => … }`，
子组件用 `useSlots()` 取：

```tsx
const Panel = (props: { title: string }) => {
  const slots = useSlots()
  return (
    <section>
      <h3>{props.title}</h3>
      {slots.default?.()}
    </section>
  )
}
```

* **`props.children` 永远是 `undefined`** —— 子节点不进 props，别用它。
* 插槽是**惰性**的：不调用 `slots.default?.()` 就不创建。
* JSX 也可以当普通 prop 传：`<Dialog impact={<span>…</span>} />`，编译器会把这段 JSX 一起编掉。

### 2.4 组件的"事件"就是函数 prop

元素上的 `onClick={fn}` 是真事件监听；**组件上**的 `onConfirm={fn}` 只是普通 prop，
由子组件自己接上：`<button onClick={props.onConfirm}>`。没有 `emit`、没有 `defineEmits`。

### 2.5 key

`key` 只对 `.map()` 有意义（§4.2）。写在其它位置的组件或元素上会被**静默丢掉**
（`web/lite/compiler.ts:195`、`web/lite/compiler.ts:293`）。

---

## 3. 状态

### ref

```ts
const n = ref(0)
n.value++
```

* 读写只能走 `.value`（它是访问器属性，不是普通方法）。
* 同值写入被 `Object.is` 挡掉，不会白刷 DOM（`web/lite/src/signal.ts:96-102`）。
* **没有 `reactive`、没有 Proxy**。共享状态的做法就是**模块级的 ref**：建一个只导出 ref 的模块，
  谁 import 谁用 —— 不需要 store 这个概念。要"对象的字段本身响应式"用 `createStore`（见下）。

### effect

```ts
const e = effect(() => { /* 立刻跑一次，之后依赖变了再跑 */ })
e.dispose()
```

组件里那些"改状态 ⇒ 改 DOM"的绑定全都是 effect，但**那是编译器生成的，业务代码基本不用手写**。

⚠ 编译器生成的 effect 挂在它插入的节点上，节点被移除时**连它一起销毁**；
**你自己手写的 effect 没有这个待遇** —— 要在 `onUnmounted` 里 `dispose()`。

`renderEffect` 只是 `effect` 的别名。

### watch

```ts
watch(authState, (value, oldValue) => { … })
```

**初始不触发**（先读一次当前值再比较，`web/lite/src/signal.ts:142-159`）、同值不触发。
只能盯一个 ref：没有 `watchEffect`，没有 `deep` / `immediate` 选项。

### batch

把一批变更合成一次刷新。**事件处理器里编译器已经自动包了**（`web/lite/compiler.ts:568-570`），
所以手写 `batch` 的场景只有"异步回调里连写多个信号"：

```ts
batch(() => { a.value = 1; b.value = 2 })
```

连写三个信号：不 batch 刷 3 次，包起来刷 1 次。

### computed

```ts
const double = computed(() => base.value * 2)
```

边界：

* **急切**：创建时就求值，不是"读到才算"。
* 每次依赖变更会算**两遍**：它的内部 effect 会订阅自己的输出（`web/lite/src/signal.ts:168-171`）。
* **没有 dispose 句柄**，卸载不了 ⇒ 别在组件里按实例建，放模块级。

### createStore

```ts
const s = createStore({ count: 0 })
s.count++      // 响应式
```

它把**初始化时已存在的键**原地换成 getter/setter（`web/lite/src/store.ts:17-23`），不用 Proxy。
边界：**之后新挂上去的键不响应**（`s.count = 1` 会更新，`s.later = 'x'` 不会）。

### field

`field(v)` 返回 `{ get, set }`，是不带 `.value` 的字段读写器 —— `createStore` 内部用它，
自己写状态一般用不上。

---

## 4. 控制流

### 4.1 条件：三元表达式

```tsx
{show.value ? <p>可见</p> : null}
```

* 编译成 `setNodes(parent, () => cond ? … : null, anchor)`。**没有** `createIf` 这种 API。
* 分支可以是 JSX / 片段 / 组件 / `null`；`false`、`undefined` 都等价于 `null`。
* 分支是普通表达式也行：`{n.value > 0 ? '有' : '无'}`。
* ⚠ **不要**在组件体顶层写 `if (…) return <X/>` 做分支 —— 那个 `return` 只求值一次，
  界面会永远冻在那一刻（理由见 §2.1）。判断要写进 JSX 表达式。

### 4.2 列表：`.map()` + `key=`

```tsx
<ul>
  {items.value.map((it) => (
    <Row key={it.id} label={it.label} />
  ))}
</ul>
```

这种写法会编成键控的 `createFor`（`web/lite/src/control.ts:34-109`）：

* **key 决定跨轮复用**：key 不变就搬动同一个节点，不重建。
* **没有 key 就用下标**。
* **同一个 key 但对象换了 ⇒ 重建那一行**（`web/lite/src/control.ts:71-75`）—— 所以 key 要用稳定 id，
  别用会变的东西（拿数组下标当 key，数据一重排就等于每行都在重建）。
* 删行会跑那一行组件的 `onUnmounted`。
* 新建的行是**正序**插入的。这不是审美问题：`<select>` 的默认选中项由"第一个被插进来的选项"
  决定，所以插入顺序会被用户看到。

⚠ **只有一种形状会编成 `createFor`**：箭头函数，且**函数体直接是一个 JSX 表达式**。
其它形状会**退化成一次性的 `setNodes`**（key 被忽略、依赖一变整块删掉重建）：

| 写法 | 编译结果 |
|---|---|
| `{xs.value.map((x) => (<Row key={x.id} />))}` | `createFor`：键控复用 ✔ |
| `{xs.value.map((x) => { const a = …; return (<Row key={x.id} />) })}` | `setNodes`：整块重建（输入框状态、组件实例、`onMounted` 全部重来） |
| `{xs.value.map((x) => rowNode(x))}`（调辅助函数返回 JSX） | 同上 |
| `{xs.map((x) => <Row />)}`（源是静态数组） | 同上，但没有响应式依赖 ⇒ 只渲染一次 |

片段作为 `.map()` 的返回值时**不能带 key**（片段没有属性，`web/lite/compiler.ts:474`），会按下标复用。

---

## 5. 属性、事件、ref、spread

### 属性

* 字面量（字符串 / 数字）在编译期**折进模板串**，运行期一次写入都没有。
  ⚠ `false` / `null` 不折：布尔属性写进 HTML 是"存在即为真"，`disabled={false}` 折进去会把按钮**打开**。
* 动态值一个 effect 只写那一个属性。
* 分派规则（编译期决定，`web/lite/compiler.ts:556-563`）：
  * `class` → `setClass`（SVG 元素走 `setAttribute`，因为 `SVGElement.className` 只读）；
  * `value` → `setValue`（property 与 attribute **都写**）；
  * `checked` `selected` `disabled` `open` `multiple` `readonly` `required` `muted` → `setProp`；
  * 其余 → `setAttr`：`null` 移除属性；`false` 只有少数布尔属性算"移除"，其余序列化成 `"false"`
    （所以 `aria-hidden={true}` 得到 `aria-hidden="true"`，不是空串）。
* **`class` 只接字符串/三元**，**`style` 只接字符串**（§6 有后果）。
* 动态文本优先**原地改 `.data`**（不重建节点）；换内容时才删旧建新。
* `<select>` 上的 `value` 不需要你操心顺序：编译器把它排到子节点绑定**之后**
  （`web/lite/compiler.ts:432-446`），浏览器自动选中的那一项不会被覆盖掉。

### 事件

```tsx
<button onClick={(e) => submit(e)}>提交</button>
```

* 判据：属性名以 `on` 开头且**第三个字符大写**；事件名取剩下部分的小写（`onClick` → `click`）。
* 处理器收到原生 `Event`，并被自动包一层 `batch`。
* 没有事件委托：每个元素一个 listener。
* ⚠ `onclick={fn}`（小写）**不是**事件，会变成一个属性，值是函数源码 —— 静默失效。

### 模板引用（`ref=`）

**不支持**。`ref={el}` 不会被特殊处理，只会给元素加一个字符串属性。
要拿元素用 `document.getElementById` / `querySelector`。

### spread

`{...obj}` 只支持**元素**（`<svg {...BASE}>` 这种写法）；写在**组件**上会直接抛错（`web/lite/compiler.ts:193`）。

---

## 6. 语法子集：哪些写法会出错

编译器**只保证报两种错**：

* 组件上的 `{...spread}`；
* 空元素带子节点（`<img>…</img>`，`web/lite/compiler.ts:425-427`）。

其余"子集外"的写法**不一定报错**，会静默编成一个错的值。写之前扫一眼这张表：

| 写法 | 结果 |
|---|---|
| `class={['a','b']}` | 不报错 → `class="a,b"`（对象同理 → `class="[object Object]"`） |
| `style={{ color: 'red' }}` | 不报错 → `style="[object Object]"`（字符串可以，等价于直接写 `style="…"`） |
| `ref={el}` | 不报错 → 元素上多一个字符串属性 |
| `v-if="x"` / `v-model="x"` / `vShow={…}` | 不报错 → 原样变成元素属性，行为为零 |
| `onclick={fn}`（小写） | 不报错 → 属性值是函数源码 |
| `<label class="x" />`（非空元素自闭合） | 正常，生成 `<label class="x"></label>` |
| `{cond && <b>x</b>}` | 能用（`false` 时渲染为空），但建议统一写三元 |
| 异步组件 / 代码分割 / SSR / 水合 / Teleport / Transition / Suspense / KeepAlive / 指令 | 不支持 |
| `provide` / `inject` / `reactive` / `nextTick` / `effectScope` / `toRef` | 没有导出：类型检查会过，`vite build` 报 `[MISSING_EXPORT]` |

---

## 7. 常见坑（速查）

1. **组件体只跑一次**：顶层别算响应式的值，写进 JSX（§2.1）。
2. **别解构 props**（§2.2）。
3. **别用顶层 `if (…) return <X/>`** 做分支（§4.1）。
4. **`key` 只在 `.map()` 里有意义**，而且回调必须是"表达式体直接返回 JSX"（§4.2）。
5. **`class` / `style` 只接字符串**（§6）。
6. **事件名第二个词首字母要大写**：`onClick` 是事件，`onclick` 是字符串属性（§5）。
7. **没有模板引用**，用 DOM 查找（§5）。
8. **手写的 `effect` 不会自动销毁**，`onUnmounted` 里自己 `dispose()`（§3）。
9. **`computed` 别按组件实例建**：没有 dispose 句柄，而且每次依赖变更算两遍（§3）。
10. **别手动增删 / 搬动框架生成的节点**：动态位置靠 `<!---->` 占位注释定位，模板子树是克隆出来的。
    把它们挪走会让后续更新插错位置。

---

## 8. 与 Vue 的差异（速查）

| | Vue / Vapor | lite |
|---|---|---|
| 更新模型 | 组件重渲染；Vapor 一个模板的动态属性共用一个 `renderEffect` | **没有重渲染**：一个绑定一个 effect，组件体只跑一次 |
| 组件体顶层的派生值 | 会跟着更新 | **不会**，必须写进 JSX |
| 更新时机 | 微任务批量刷 | **同步**刷；没有 `nextTick` |
| 属性绑定粒度 | Vapor 改一个属性会顺带重写同一模板里的其它动态属性 | 细粒度：改 A 不动 B。这是刻意的 —— Vapor 那处粗粒度会把 `<select>` 的值重写成空串，不复刻这个缺陷 |
| 卸载 | 组件实例卸载 | 节点被移除 ⇒ 该子树里的绑定销毁 + `onUnmounted` 跑；**手写 effect 除外** |
| provide/inject、指令、Teleport、Transition、Suspense、异步组件 | 有 | 没有 |
| 模板引用 | `ref=` | 没有，用 DOM 查找 |

---

## 9. 接进 Vite

```ts
// vite.config.ts
import lite from './lite/vite.ts'

export default defineConfig({
  plugins: [lite({ runtime: 'lite' })],
  resolve: {
    alias: [{ find: /^lite$/, replacement: new URL('./lite/src/index.ts', import.meta.url).pathname }],
  },
})
```

* 插件 `enforce: 'pre'`：它要先于 Vite 自己的 esbuild 拿到**还带着 JSX 的**源码，
  把 JSX 折掉、保留 TS 语法，再交给 esbuild 去类型。
* 默认只处理 `.tsx`，并跳过 `node_modules`。
* `runtime` 决定**生成代码 import 谁**。建议写成裸名 + alias：生成代码可能落在任意深度的源文件里，
  用相对路径会解析到不同的地方。
* 生成代码里的 helper 全部带 `_$` 前缀，避免和业务代码自己的 import 撞名。

开发与构建就是普通的 Vite 用法：`npm run dev` 起开发服务器，`npm run build` 出产物。
本仓把产物嵌进后端二进制，所以交付时**先 `npm run build`、再编译后端** ——
顺序反了会嵌进旧资源（白屏 / chunk 404），而后端构建不会报错。这条只关乎本仓的打包方式，
与 lite 无关，细节见 `AGENTS.md`。

---

## 附：与 `docs/lite-framework.md` 的出入（以代码为准）

设计文档里这几处已经过期，读它的时候注意：

| 设计文档 | 代码 |
|---|---|
| §3 / §4.2 / §4.3：动态 prop 是**函数**（`item.value()`） | 是 **getter**（`web/lite/compiler.ts:205-209`）；写 `props.label` |
| §3：`_createIf` → `createIf(parent, cond, then, else?)` | 没有 `createIf`；条件就是 `setNodes`（`web/lite/compiler.ts:457-462`） |
| §4.2 示例里的 `setText(...)` | 编译器**不生成** `setText`；文本子节点走 `setNodes` 里的文本处理 |
| `web/lite/compiler.ts:38-39`：子集外写法"直接抛错" | 只有组件 spread 与空元素带子节点抛；其余**静默编错**（§6 表） |
| 断言条数写着 20 / 53 / 55 | 实际是 **57**（`web/lite/demo/main.tsx`） |
