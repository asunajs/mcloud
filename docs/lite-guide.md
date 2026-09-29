# 把 lite 迁进你的新项目

这份教程写给**第一次接触 lite、要在自己的新项目里用它的你** —— 不是框架维护者。
lite 是一个"无虚拟 DOM + 编译期绑定"的 TSX 框架：运行时零依赖，只需要 Node + Vite，
TSX 由一个小插件在编译期折成运行时调用，之后只有"读过某个状态的那一处 DOM"会被更新。

框架本身的**设计取舍与踩坑史**不在本文，见 [`docs/lite-framework.md`](lite-framework.md)（那篇是给要改框架的人看的）。

**先说结论：** 把 `lite/` 目录拷进项目 → 装 `vite` + `typescript@^5` → 接插件、配 tsconfig →
用自带的 `lite/jsx.d.ts` 拿到 JSX 类型。**全程不需要 Vue**（本文第 4 节里每个结论都有跑过的命令）。

---

## 0. 迁移前要知道的前提

这些是"接了才知道"的东西，先看一遍能省掉大半天：

1. **需要真 DOM，没有 SSR / 水合。** `template()` 走 `document.createElement('template')`，
   `mount()` 也要 `document` —— 首屏就是真节点。
   ⚠ 这些源码是**给打包器看的**：模块内部用的是无扩展名的相对导入（`./signal`），
   Node 直接 `import` 会先报 `Cannot find module '…/lite/src/signal'`（实测）——
   所以也别指望"先在 Node 里 import 一下试试"。
2. **目标环境 ES2022 起步。** 运行时用了 `<template>` + `cloneNode`、`?.` / `??`；构建目标写 `es2022`。
3. **没有重渲染。** 组件函数体**只跑一次**，之后更新由编译期生成的细粒度绑定各自负责。
   ⇒ 组件体顶层算出来的值是**死的**，派生值必须写进 JSX（§6.1）。
4. **更新是同步的。** 写 `.value` 那一刻 DOM 就变了，没有 `nextTick`、没有微任务批处理。
   一次改多个信号要合成一次刷新，用 `batch()`（§7）。
5. **状态惯例是模块级 `ref`。** "建一个只导出 ref 的模块，谁 import 谁用"就是这个框架的状态管理；
   模块级 ref 是**全应用唯一**的 —— 同一页面挂两个 App 实例会共享同一份状态。
   要按实例隔离，就在组件体内 `ref()`（组件体只跑一次，正好一个实例一份）。
6. **类型检查需要 `typescript@^5`。** 编译器用的是 TypeScript 5 的 JS API（`ts.createSourceFile` 等）。
   实测装成 `typescript@7` 时：`vite build` 报
   `TypeError: Cannot read properties of undefined (reading 'ESNext')`，`tsc --noEmit` 报 110 条错。
   按 §1 的命令装（`typescript@^5`）就不会踩到。
7. **没有这些东西**：SSR、水合、Teleport / Transition / Suspense / 异步组件 / 指令、`provide` / `inject`、
   `reactive`、`nextTick`、事件委托、模板引用（`ref=`）。全表见 §10、§12。
8. **构建工具**：官方只提供 Vite 插件。想接别的（rollup / esbuild / webpack），
   得自己调 `lite/compiler.ts` 的 `compile()` —— 那是另一件事，本文不覆盖。

---

## 1. 拿到框架：拷哪几个文件

把整个 `lite/` 目录拷进你的项目，最终长这样（教程后面都按这个路径写）：

```
your-project/
├─ lite/
│  ├─ src/
│  │  ├─ index.ts       ← 对外 API 的唯一入口，业务代码只 import 这里
│  │  ├─ signal.ts      ← 响应式核心：ref / effect / batch / watch / computed / field
│  │  ├─ dom.ts         ← DOM 层：template / insert / setNodes / setClass …（编译产物调它）
│  │  ├─ control.ts     ← createFor：`.map()` 列表的键控复用
│  │  ├─ component.ts   ← 组件、插槽、onMounted / onUnmounted、mount / createVaporApp
│  │  └─ store.ts       ← createStore（可选模块，用不上可以整份删）
│  ├─ compiler.ts       ← TSX → 运行时调用（唯一需要 typescript 的文件）
│  ├─ vite.ts           ← Vite 插件（import 上面的 compiler.ts）
│  └─ jsx.d.ts          ← JSX 类型声明（只有类型，零运行时代码）★ 见 §4
├─ src/…                ← 你自己的代码
├─ index.html
├─ vite.config.ts
└─ tsconfig.json
```

**必须一起拷的**：

| 文件 | 少了会怎样 |
|---|---|
| `lite/src/` 六个文件 | 它们互相 import，只拷 `index.ts` 起不来 |
| `lite/compiler.ts` | `vite.ts` 直接 import 它，构建期就报模块找不到 |
| `lite/vite.ts` | 没插件 ⇒ JSX 没人折，esbuild 会按 React 语义去转（§3） |
| `lite/jsx.d.ts` | `tsc --noEmit` 对每个标签报 `TS7026`（§4） |

**可以不用拷的**：`demo/`、`bench/`、`regress/`、`size.mjs` —— 那是框架自己的验收样例与体积统计，
跟你的项目无关。其中 `demo/main.tsx` 是最全的用法样例，想照着抄可以留着看
（直接跑它需要无头 Chrome，属于可选）。

**运行时零依赖**：`lite/src/**` 不 import 任何外部包，只用 DOM 与 ES2022。
**构建期唯一依赖是 `typescript`**（`compiler.ts` 用它解析 TSX），版本必须 `^5`（§0.6）：

```bash
npm i -D vite typescript@^5
```

> 验证环境：**Node 24.16.0 + npm 11.13.0 + vite 8.3.1 + typescript 5.9.3**。
> 更老的 Vite 也能用（插件 API 很基础），但 §3.2 里那条 alias 差异要注意。

业务代码用到的 API 就是下面这些（`lite/src/index.ts` 的全部导出）：

| 分类 | API |
|---|---|
| 状态 | `ref` `computed` `watch` `batch` `effect` `field` `createStore` |
| 组件 | 普通函数组件 + `useSlots` `onMounted` `onUnmounted` `mount` `createVaporApp` `defineVaporComponent` |
| 编译器生成的 helper | `template` `setNodes` `setText` `setClass` `setAttr` `setProp` `setValue` `on` `spread` `createFor` `createComponent` `createNodes` `lazySlot` `insert` `remove` `onRemove` `renderEffect` |

第三类**只有编译产物在用**，业务代码不要手写。

---

## 2. 接进 Vite

`vite.config.ts` 最小可跑片段：

```ts
// vite.config.ts
import { defineConfig } from 'vite'
import lite from './lite/vite.ts'

export default defineConfig({
  plugins: [lite({ runtime: 'lite' })],
  resolve: {
    // 让裸名 'lite' 在构建期解析到运行时入口（tsconfig 的 paths 只管类型）
    alias: [{ find: /^lite$/, replacement: new URL('./lite/src/index.ts', import.meta.url).pathname }],
  },
  build: { target: 'es2022' },
})
```

要点：

* **TSX 是插件编译的，不是 esbuild 编译的。** 插件 `enforce: 'pre'`，先于 Vite 自己的 esbuild
  拿到**还带着 JSX 的**源码，把 JSX 折掉、保留 TS 语法，再交给 esbuild 去类型。
  所以**不需要 Vue、不需要 vue-jsx-vapor、不需要 `@vitejs/plugin-vue`**：
  实测一个零 Vue 工程 build 出单 chunk `index-*.js 8.31 kB`（gzip 3.76 kB，含两个组件 + 列表 + 表单）。
* 插件默认只处理 `.tsx`，并跳过 `node_modules`；`include` 选项可以改。
* `runtime` 决定**生成代码 import 谁**。推荐裸名 + alias：编译产物可能落在任意深度的源文件里，
  用相对路径会随文件位置解析到不同的地方。配了裸名，就要 §3 的 `paths` 与这里的 `alias` 都在。
* `build.target: 'es2022'` 与运行时用到的语法对齐（§0.2）。
* 开发/构建就是普通 Vite 用法：`npx vite`（dev）、`npx vite build`、`npx vite preview`。
* CSS 方案与 lite 无关（Tailwind / 手写 CSS / CSS Modules 都行）。注意 `class` 只能传字符串（§10）。

---

## 3. tsconfig

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "jsx": "preserve",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    "isolatedModules": true,
    "verbatimModuleSyntax": true,
    "allowImportingTsExtensions": true,
    "paths": {
      "lite": ["./lite/src/index.ts"]
    }
  },
  "include": ["src", "lite", "vite.config.ts"]
}
```

### 3.1 `"jsx": "preserve"` 是必须的

两层原因，第二层是坑：

1. **类型检查**：非 `preserve`（比如从 React 模板抄来的 `"react-jsx"`）时，tsc 会按 React 语义去找
   `react/jsx-runtime`。实测：
   ```
   src/main.tsx(59,5): error TS2875: This JSX tag requires the module path 'react/jsx-runtime' to exist,
   but none could be found. Make sure you have types for the appropriate package installed.
   ```
2. **构建侧**：Vite 的 esbuild 也会读 tsconfig 的 `jsx` 设置。正常情况下插件已经把 JSX 折掉了，
   轮不到 esbuild；可一旦插件没覆盖到（文件不是 `.tsx`、被 `include` 排除、插件顺序被改），
   JSX 就会落到 esbuild 手里被按 React 语义转成 `_jsx(...)`，**运行期才炸**。
   ⚠ 实测把 `jsx` 写成 `react-jsx` 时 `npx vite build` **照样成功、产物 hash 都一样** ——
   所以"我 build 过了"不能当类型对的证据，**这个错只在 `tsc --noEmit` 里暴露**。

### 3.2 裸名 `lite` 要配两处，相对路径不用

两种写法都支持，**实测可以在同一个工程里混用**（一个文件裸名、另一个文件相对路径，一次
tsc + build 全绿）：

| 写法 | 需要配什么 |
|---|---|
| `import { ref } from 'lite'` | tsconfig `paths`（给 tsc）+ Vite `resolve.alias`（给构建/浏览器） |
| `import { ref } from '../lite/src/index'` | 什么都不用配；插件也要跟着设 `runtime: '../lite/src/index'` |

* 裸名少一处就报错，报错长得不一样：
  * 只有 tsconfig 没配 `paths` → `error TS2307: Cannot find module 'lite' or its corresponding type declarations.`
  * 只有 Vite 没配 `alias` → `Error: [vite]: Rolldown failed to resolve import "lite" from "/…/src/main.tsx".`
* 一个版本差异：**Vite 8 会自己读 tsconfig 的 `paths`**（实测只配 `paths`、不配 `alias` 也能 build）；
  更早的 Vite 不读（这个能力是后加的，本文没测旧版本）。两处都配最稳，配了不会冲突。
* `paths` 里的路径按 tsconfig 所在目录解析，`baseUrl` 可以不写。
* 相对路径的代价：插件生成代码里的 import 也是相对路径，**按每个源文件的深度**解析 ——
  只有你的 TSX 都在同一层目录时才省心，这也是推荐裸名的原因。

### 3.3 别的几项

* `lib` 必须含 `DOM`（运行时需要真 DOM）。
* `allowImportingTsExtensions` + `noEmit`：因为 `vite.config.ts` 里写的是 `import lite from './lite/vite.ts'`
  （`vite.ts` 里又 `import { compile } from './compiler.ts'`）—— 显式写 `.ts` 扩展名需要这个开关，且必须不 emit。
* `include` 必须覆盖 `lite/jsx.d.ts`：它靠 `include` 进程序，**不靠 `types` 字段**（`"types": []` 也照样生效）。
  漏掉它 ⇒ 每个标签 `TS7026`（§4）。
* `strict` 建议开着：`lite/jsx.d.ts` 会让 props 类型照常被校验，写错会当场报错（§4 有负向验证）。

---

## 4. JSX 类型：迁移时最容易卡住的一步 ⭐

**背景**：`lite/src/**` 与 `compiler.ts` 里**没有任何 JSX 类型声明**（没有 `namespace JSX`）——
类型检查是 tsc 的事，lite 的编译器管不着。没有声明时，每个标签都报：

```
error TS7026: JSX element implicitly has type 'any' because no interface 'JSX.IntrinsicElements' exists.
```

`lite/jsx.d.ts` 补的就是这一层：**只有类型、没有一行运行时代码**，覆盖面 = 编译器实际支持的子集：

* `IntrinsicElements`：HTML / SVG / 自定义元素，**属性宽松**（`class={…}`、`disabled={false}`、
  `data-*`、`aria-*` 一律放行）；
* 函数组件：`type ElementType = string | ((props: any, ctx: any) => unknown)` —— 标签合法性由它把关，
  而 **props 仍按你声明的类型校验**；返回值不卡（`cond ? <A /> : null` 是合法组件返回值）；
* 片段、`key`；
* 子节点：`ElementChildrenAttribute` + `LibraryManagedAttributes` 让
  `<Panel title="x">…</Panel>` 直接可用 —— **props 上不用、也不该**声明 `children`
  （lite 的子节点是插槽，不会进 props，见 §6.3）。

里面有两处是"不写就报错"的细节，值得知道原因：

* `type Element = unknown`：JSX 表达式的值可以是节点 / 节点数组 / `null`。收紧了就会把条件分支写法卡死。
* `[attr: \`on${string}\`]: ((e: any) => void) | undefined`：属性表只有笼统索引签名时，
  `onClick={(e) => …}` 里的 `e` 拿不到上下文类型，strict 下报
  `TS7006: Parameter 'e' implicitly has an 'any' type`。事件参数写成 `any` 是**故意的** ——
  运行期给的是原生事件对象（`Event` / `MouseEvent` / `InputEvent`…），类型系统推不出来；
  要精确类型就自己标注：`onInput={(e: InputEvent) => …}`。

### 路径 ①：用 lite 自带的类型（推荐，零 Vue 依赖）

做法：`include` 里带上 `lite`，**不要写 `jsxImportSource`**。就这两件事。

**跑过的命令与结果**（`/tmp` 里新建的最小工程：拷 `lite/` 进去、`npm i -D vite typescript@^5`、
**没有安装 Vue / vue-jsx-vapor**，业务代码是一个 `.map()` + 插槽 + 片段 + 表单的小页面）：

```
$ npx tsc --noEmit
（无输出，退出码 0）

$ npx vite build
vite v8.3.1 building client environment for production...
transforming...
✓ 11 modules transformed.
rendering chunks...
computing gzip size...
dist/index.html                0.25 kB │ gzip: 0.21 kB
dist/assets/index-DbpC-hsm.js  8.31 kB │ gzip: 3.76 kB

✓ built in 50ms
```

把产物塞进无头 Chrome 跑了一遍（`chrome-headless-shell --dump-dom file://…/inline.html`）：
键控列表、片段、插槽、SVG spread、条件行、`disabled`、`<select>` 的 value 全部渲染正确 ——
这份类型能配出一个**真能跑**的新工程，不只是"能过类型检查"。

**类型不是形同虚设**（同一工程里放了一个故意写错的负向样例）：

```
$ npx tsc -p tsconfig.negative.json
negative/negative.tsx(11,10): error TS2322: Type 'number' is not assignable to type 'string'.
negative/negative.tsx(13,6): error TS2322: Type '{ label: string; }' is not assignable to type
  'IntrinsicAttributes & { label: string; n: number; } & { children?: unknown; }'.
  Property 'n' is missing in type '{ label: string; }' but required in type '{ label: string; n: number; }'.
negative/negative.tsx(15,25): error TS2322: Type 'unknown' is not assignable to type 'string'.
（退出码 2）
```

### 路径 ②：沿用 `vue-jsx-vapor` 的类型（存量 Vue 项目过渡用）

做法：`"jsxImportSource": "vue-jsx-vapor"`，**不要** include `lite/jsx.d.ts`。
这条路是给"已经有一整套 Vue + vue-jsx-vapor 类型、暂时不想动"的项目准备的 ——
新项目走它等于**为了类型背上 Vue**，这正是路径 ① 存在的理由。

**跑过的命令与结果**（`/tmp` 里另建一个同源工程，`jsxImportSource` 指向 `vue-jsx-vapor`，
`node_modules` 软链到已有的 vue 3.6.0-rc.9 + vue-jsx-vapor 3.2.25，**没有新装 Vue**）：

```
$ npx tsc --noEmit
（无输出，退出码 0）

$ npx tsc --noEmit --listFiles | grep -i vue-jsx-vapor
node_modules/@vue-jsx-vapor/runtime/dist/types.d.ts
node_modules/@vue-jsx-vapor/runtime/dist/h.d.ts
node_modules/@vue-jsx-vapor/runtime/dist/jsx.d.ts
…（确实在用 vue-jsx-vapor 的 JSX 命名空间）

$ npx vite build
✓ built in 59ms
dist/assets/index-DbpC-hsm.js  8.31 kB │ gzip: 3.76 kB     ← 与路径 ① 同 hash（源码相同，构建与类型来源无关）
```

**反向对照**（证明上面那次通过不是碰巧）：把 `jsxImportSource` 删掉、也不 include lite 的声明，
立刻回到 `TS7026` —— 说明通过确实是 vue-jsx-vapor 的类型在起作用：

```
$ npx tsc -p tsconfig.nojsxsource.json
src/main.tsx(21,5): error TS7026: JSX element implicitly has type 'any' because no interface
  'JSX.IntrinsicElements' exists.
（退出码 2）
```

**未验证**：`vue-jsx-vapor` 的宏能力（那套要配 `@ts-macro/tsc`）没在新工程里试。
本教程只用 `tsc --noEmit` 验证过"JSX 类型这条路走得通"。

### 两条路怎么选

| | 路径 ①（`lite/jsx.d.ts`） | 路径 ②（`vue-jsx-vapor`） |
|---|---|---|
| 依赖 | 只多一个 `lite/jsx.d.ts` | 多 `vue` + `vue-jsx-vapor` |
| 适合 | **新项目**、非 Vue 项目 | 已有 Vue 类型体系、只把 TSX 换成 lite |
| 实测 | `tsc` 0 错、build 通过、浏览器渲染正确 | `tsc` 0 错、build 通过 |

两种声明同时存在也不会报错（实测退出码 0），但没必要，二选一更清楚。

---

## 5. 第一个组件：计数器

```tsx
// src/main.tsx
import { createVaporApp, ref } from 'lite'

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
// 等价写法：mount(Counter, '#app')
```

没有组件注册、没有依赖声明、没有 `setup()`。点一下按钮时发生的事：

* `onClick` 编译成一次 `addEventListener('click', …)`；
* `{n.value}` 编译成一个只写这个文本节点的绑定：`_$setNodes(_n0, () => n.value, _n1)`；
* 写 `n.value` 是同步的，那个文本节点立刻就变了。

---

## 6. 组件

### 6.1 组件就是函数，而且只跑一次

签名 `(props, ctx) => 节点`。**函数体只跑一次**，之后没有"父组件更新 ⇒ 子组件重跑"这回事，
更新由组件内部那些细粒度绑定各自负责。

⇒ 直接后果：**在组件体顶层算出来的值是死的**。

```tsx
// ✗ 永远停在第一句：这里只求值一次
const label = loading.value ? '正在加载…' : '完成'
return <span>{label}</span>

// ✓ 写进 JSX：读发生在绑定里，值变了会更新
return <span>{loading.value ? '正在加载…' : '完成'}</span>
```

`defineVaporComponent(fn)` 是**恒等函数**，存在只为兼容迁移期的旧写法 —— 新代码直接写箭头函数就行。

### 6.2 props

props 是普通对象；**动态 prop 在编译期写成 getter**：

```tsx
<Row label={it.label} />
// 编译成：createComponent(Row, { get "label"() { return it.label } })
```

所以"读 `props.label` 的那一下"才是建立依赖的地方：

* 在 JSX 里读：`{props.label}` ✔
* `const { label } = props` ✗ —— 组件体那一次把 getter 求值成了死值，之后不再更新（而且不报错）。
* `props.label` 就是**值**，不是函数。不要写 `props.label()`。

### 6.3 子节点 = 插槽

`<Panel>…</Panel>` 的子节点编译成 `createComponent` 的第三个参数 `{ default: () => … }`，
子组件用 `useSlots()` 取：

```tsx
import { useSlots } from 'lite'

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

* **`props.children` 永远是 `undefined`** —— 子节点不进 props，别读它（类型上也不用声明，见 §4）。
* 插槽是**惰性**的：不调用 `slots.default?.()` 就不创建。
* JSX 也可以当普通 prop 传：`<Dialog impact={<span>…</span>} />`，编译器会把这段 JSX 一起编掉。

### 6.4 组件的"事件"就是函数 prop

元素上的 `onClick={fn}` 是真事件监听；**组件上**的 `onConfirm={fn}` 只是普通 prop，
由子组件自己接上：`<button onClick={props.onConfirm}>`。没有 `emit`、没有 `defineEmits`。

### 6.5 key

`key` 只对 `.map()` 有意义（§8.2）。写在其它位置的组件或元素上会被**静默丢掉**。

---

## 7. 状态

`lite/src/index.ts` 导出的状态 API 就这些：`ref` `computed` `watch` `batch` `effect` `field` `createStore`。

### ref

```ts
const n = ref(0)
n.value++
```

* 读写只能走 `.value`（它是访问器属性，不是普通方法）。
* 同值写入被 `Object.is` 挡掉，不会白刷 DOM。
* **没有 `reactive`、没有 Proxy。** 共享状态的做法就是**模块级 ref**：建一个只导出 ref 的模块，
  谁 import 谁用 —— 不需要 store 这个概念。要"对象的字段本身响应式"用 `createStore`（见下）。
  ⚠ 模块级 ref 是全应用唯一的：同页多实例会共享（§0.5）。要按实例隔离就在组件体内建 ref。

### effect

```ts
const e = effect(() => { /* 立刻跑一次，之后依赖变了再跑 */ })
e.dispose()
```

组件里那些"改状态 ⇒ 改 DOM"的绑定全都是 effect，但**那是编译器生成的，业务代码基本不用手写**。

⚠ 编译器生成的 effect 挂在它插入的节点上，节点被移除时**连它一起销毁**；
**你自己手写的 effect 没有这个待遇** —— 要在 `onUnmounted` 里 `dispose()`。

`renderEffect` 只是 `effect` 的别名（编译产物用的名字）。

### watch

```ts
watch(authState, (value, oldValue) => { … })
```

**初始不触发**（先读一次当前值再比较）、同值不触发。只能盯一个 ref：
没有 `watchEffect`，没有 `deep` / `immediate` 选项。

### batch

把一批变更合成一次刷新。**事件处理器里编译器已经自动包了**，
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
* 每次依赖变更会算**两遍**（它内部的 effect 会订阅自己的输出）。
* **没有 dispose 句柄**，卸载不了 ⇒ 别在组件里按实例建，放模块级。

### createStore

```ts
const s = createStore({ count: 0 })
s.count++      // 响应式
```

它把**初始化时已存在的键**原地换成 getter/setter，不用 Proxy。
边界：**之后新挂上去的键不响应**（`s.count = 1` 会更新，`s.later = 'x'` 不会）。

### field

`field(v)` 返回 `{ get, set }`，是不带 `.value` 的字段读写器 —— `createStore` 内部用它，
自己写状态一般用不上。

---

## 8. 控制流

### 8.1 条件：三元表达式

```tsx
{show.value ? <p>可见</p> : null}
```

* 编译成 `setNodes(parent, () => cond ? … : null, anchor)`。**没有** `createIf` 这种 API。
* 分支可以是 JSX / 片段 / 组件 / `null`；`false`、`undefined` 都等价于 `null`。
* 分支是普通表达式也行：`{n.value > 0 ? '有' : '无'}`。
* ⚠ **不要**在组件体顶层写 `if (…) return <X/>` 做分支 —— 那个 `return` 只求值一次，
  界面会永远冻在那一刻（理由见 §6.1）。判断要写进 JSX 表达式。

### 8.2 列表：`.map()` + `key=`

```tsx
<ul>
  {items.value.map((it) => (
    <Row key={it.id} label={it.label} />
  ))}
</ul>
```

这种写法会编成键控的 `createFor`：

* **key 决定跨轮复用**：key 不变就搬动同一个节点，不重建。
* **没有 key 就用下标。**
* **同一个 key 但对象换了 ⇒ 重建那一行** —— 所以 key 要用稳定 id，
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

片段作为 `.map()` 的返回值时**不能带 key**（片段没有属性），会按下标复用。

---

## 9. 属性、事件、spread

### 属性

* 字面量（字符串 / 数字）在编译期**折进模板串**，运行期一次写入都没有。
  ⚠ `false` / `null` 不折：布尔属性写进 HTML 是"存在即为真"，`disabled={false}` 折进去会把按钮**打开**。
* 动态值一个 effect 只写那一个属性。
* 分派规则（编译期决定）：
  * `class` → `setClass`（SVG 元素走 `setAttribute`，因为 `SVGElement.className` 只读）；
  * `value` → `setValue`（property 与 attribute **都写**）；
  * `checked` `selected` `disabled` `open` `multiple` `readonly` `required` `muted` → `setProp`；
  * 其余 → `setAttr`：`null` 移除属性；`false` 只有少数布尔属性算"移除"，其余序列化成 `"false"`
    （所以 `aria-hidden={true}` 得到 `aria-hidden="true"`，不是空串）。
* **`class` 只接字符串/三元**，**`style` 只接字符串**（§10 有后果）。
* 动态文本优先**原地改 `.data`**（不重建节点）；换内容时才删旧建新。
* `<select>` 上的 `value` 不需要你操心顺序：编译器把它排到子节点绑定**之后**，
  浏览器自动选中的那一项不会被覆盖掉。

### 事件

```tsx
<button onClick={(e: MouseEvent) => submit(e)}>提交</button>
```

* 判据：属性名以 `on` 开头且**第三个字符大写**；事件名取剩下部分的小写（`onClick` → `click`）。
* 处理器收到**原生** `Event`（不是合成事件），并被自动包一层 `batch`。
* 没有事件委托：每个元素一个 listener。
* ⚠ `onclick={fn}`（小写）**不是**事件，会变成一个属性，值是函数源码 —— 静默失效。
* 类型上事件参数是 `any`（§4），要精确类型就自己标注，例如 `(e: InputEvent) => …`。

### 模板引用（`ref=`）

**不支持**。`ref={el}` 不会被特殊处理，只会给元素加一个字符串属性。
要拿元素用 `document.getElementById` / `querySelector`。

### spread

`{...obj}` 只支持**元素**（`<svg {...BASE}>` 这种写法）；写在**组件**上会直接抛错。

---

## 10. 语法子集：哪些写法会出错

编译器**只保证报两种错**：

* 组件上的 `{...spread}`；
* 空元素带子节点（`<img>…</img>`）。

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
| `provide` / `inject` / `reactive` / `nextTick` / `effectScope` / `toRef` | 没有导出：`import` 它们直接报 `TS2305: Module '"lite"' has no exported member 'provide'`；若你是把 `from 'vue'` alias 过去的老代码，类型可能过得去、`vite build` 才报 `[MISSING_EXPORT]` |

---

## 11. 常见坑（速查）

1. **组件体只跑一次**：顶层别算响应式的值，写进 JSX（§6.1）。
2. **别解构 props**（§6.2）。
3. **别用顶层 `if (…) return <X/>`** 做分支（§8.1）。
4. **`key` 只在 `.map()` 里有意义**，而且回调必须是"表达式体直接返回 JSX"（§8.2）。
5. **`class` / `style` 只接字符串**（§10）。
6. **事件名第二个词首字母要大写**：`onClick` 是事件，`onclick` 是字符串属性（§9）。
7. **没有模板引用**，用 DOM 查找（§9）。
8. **手写的 `effect` 不会自动销毁**，`onUnmounted` 里自己 `dispose()`（§7）。
9. **`computed` 别按组件实例建**：没有 dispose 句柄，而且每次依赖变更算两遍（§7）。
10. **别手动增删 / 搬动框架生成的节点**：动态位置靠 `<!---->` 占位注释定位，模板子树是克隆出来的。
    把它们挪走会让后续更新插错位置。

---

## 12. 与 Vue 的差异（速查）

| | Vue / Vapor | lite |
|---|---|---|
| 更新模型 | 组件重渲染；Vapor 一个模板的动态属性共用一个 `renderEffect` | **没有重渲染**：一个绑定一个 effect，组件体只跑一次 |
| 组件体顶层的派生值 | 会跟着更新 | **不会**，必须写进 JSX |
| 更新时机 | 微任务批量刷 | **同步**刷；没有 `nextTick` |
| 属性绑定粒度 | Vapor 改一个属性会顺带重写同一模板里的其它动态属性 | 细粒度：改 A 不动 B |
| 卸载 | 组件实例卸载 | 节点被移除 ⇒ 该子树里的绑定销毁 + `onUnmounted` 跑；**手写 effect 除外** |
| provide/inject、指令、Teleport、Transition、Suspense、异步组件 | 有 | 没有 |
| 模板引用 | `ref=` | 没有，用 DOM 查找 |
| 事件对象 | 元素事件是原生；组件事件走 `emit` | 原生 `Event`；组件上的 `onXxx` 只是普通 prop（§6.4） |

> 如果你是从一个 Vue（含 Vapor）项目迁过来的：把 `import { … } from 'vue'` 改成 `from 'lite'`
> 基本就够；`ref` / `.value` / `onMounted` / `watch` / `.map()+key` 这套写法是刻意对齐的。
> 迁移期如果用 alias 把 `'vue'` 指到 lite，那只是过渡手段，**新代码直接 import lite**。

---

## 13. 附：本文里哪些是实测的

验证环境：macOS (arm64)、Node v24.16.0、npm 11.13.0、vite 8.3.1、typescript 5.9.3、
无头 Chrome（chrome-headless-shell 149）。

| 结论 | 怎么验证的 | 结果 |
|---|---|---|
| 零 Vue 的新工程能跑通 | `/tmp` 最小工程（`npm i -D vite typescript@^5`，无 Vue），`npx tsc --noEmit` + `npx vite build` | 0 错、build ✓（单 chunk 8.31 kB / gzip 3.76 kB） |
| 产物真的能跑 | 产物塞进无头 Chrome `--dump-dom` | 列表/键控/片段/插槽/SVG spread/条件行/`disabled`/`select` 全对 |
| `lite/jsx.d.ts` 不是形同虚设 | 负向样例：错类型、缺必填 prop、把 JSX 赋给 `string` | 3 条预期的 `TS2322` |
| `jsx` 必须 `preserve` | 改成 `react-jsx` 后 | `tsc` 报 `TS2875`；`vite build` **照样成功**（所以只能靠 tsc 拦） |
| 缺 `jsx.d.ts` 就报错 | `include` 不覆盖它 | 每个标签 `TS7026` |
| `types` 字段不影响它 | `"types": []` | `tsc` 0 错 |
| 路径 ② 可用 | 同源工程 + `jsxImportSource: "vue-jsx-vapor"`（软链到 vue 3.6.0-rc.9 / vue-jsx-vapor 3.2.25） | `tsc` 0 错、`--listFiles` 确认用到其 JSX 命名空间、build ✓ |
| 路径 ② 不是碰巧 | 去掉 `jsxImportSource` 且不含 lite 声明 | 立刻 `TS7026` |
| 裸名要配 alias/paths | 分别删掉 `resolve.alias` / tsconfig `paths` | `Rolldown failed to resolve import "lite"` / `TS2307` |
| Vite 8 自己读 `paths` | 只留 tsconfig `paths`、删掉 `resolve.alias` | build ✓（旧版 Vite 没测，仍建议两处都配） |
| TS 7 不能用 | 装 `typescript@7.0.2` | `vite build`：`TypeError: Cannot read properties of undefined (reading 'ESNext')`；`tsc`：110 条错 |

**没验证 / 不在本文范围内的**：`vite dev` 的 HMR 与 `vite preview`；Vite 8 以外的 Vite 版本、
以及其他构建工具（rollup / esbuild / webpack 插件）；SSR 与任何服务端渲染；
框架自带的 `demo/` + `regress/` 那套断言脚本（要无头 Chrome，本文的样例是另写的最小工程）；
`vue-jsx-vapor` 的宏（`@ts-macro/tsc`）路径。
