/**
 * 组件层。
 *
 * 组件就是**普通箭头函数** `(props, ctx) => 节点`，没有选项对象、没有装饰器。
 * 编译期把 `<Foo a={1}>x</Foo>` 折成 `createComponent(Foo, { a: 1 }, { default: () => [...] })`。
 *
 * 两个关键设计：
 *
 * 1. **props 是普通对象，动态值由编译期写成 getter**。于是子组件在自己的
 *    effect 里读 `props.title` 就自动建立了依赖 —— 运行期**一行 props 比较代码都不需要**，
 *    也没有"父组件重渲染 ⇒ 子组件跟着重渲染"这回事（根本没有重渲染）。
 * 2. **插槽就是函数**（`{ default: () => 节点 }`）。`slots.default?.()` 照旧可用，
 *    而且天然是惰性的：调用方不调用就不创建。
 *
 * 生命周期钩子只有两个（普查：`onMounted` 11 处、`onUnmounted` 5 处，没有别的）。
 */

import { type Nodes, insert, onRemove, createNodes, remove, queueMount } from './dom'
import { ownedEffects } from './signal'

export interface Slots {
  default?: () => unknown
  [name: string]: (() => unknown) | undefined
}

export type Component<P = Record<string, unknown>> = (props: P, ctx: { slots: Slots }) => unknown

interface Instance {
  mounts: (() => void)[]
  unmounts: (() => void)[]
  slots: Slots
}

/** 当前正在渲染的组件实例。同步渲染 ⇒ 一个模块级变量就够，不需要上下文栈。 */
let current: Instance | null = null

/**
 * 造一个组件实例，返回它的节点。
 *
 * 注意这里**没有**"组件要不要更新"的判断 —— 组件的函数体一次就渲染完了，
 * 之后的更新由它内部那些细粒度 effect 各自负责。这是与虚拟 DOM 框架最大的差别：
 * 父组件的变化不会让子组件的函数体重跑。
 */
export function createComponent<P>(Comp: Component<P>, props: P, slots: Slots = {}): Nodes {
  const inst: Instance = { mounts: [], unmounts: [], slots }
  const prev = current
  current = inst
  let nodes: Nodes
  try {
    /**
     * ⚠⚠ 组件体里那些**公开 `effect()`** 建的 effect 必须有归属 —— 这是 2026-10-02
     * 那轮内存泄漏的**根因** ✓。
     *
     * 症状：切页若干轮之后 DOM 节点与监听器一路涨、GC 之后堆不回落；堆快照的回溯链是
     * `模块级信号 → subs → Effect(fn) → 闭包 → 已脱离文档的 DOM` ✓。
     *
     * 为什么：`compiler.ts` 把**动态属性**编成公开的 `effect(() => setClass(…))`
     * （见它 `case 'attr'` 那条 ✓），而公开 `effect()` 只把自己交给 `signal.ts` 的
     * `scope`（由 `ownedEffects()` 设 ✓）—— 可 `createComponent` **从来没建过作用域** ✗，
     * 全仓 `ownedEffects` 只有 `control.ts` 里那一处（那是**列表行**的收口 ✓，粒度更细 ✓）。
     * ⇒ 组件体里的 effect **谁都不管**，页面卸载时没人 `dispose` ✗。
     *
     * ⚠ 只有"读了**长命**信号"的那些才显形（模块级的 `opened` / `activeGroup` 这类）：
     * 长命信号攥着 effect ⇒ effect 攥着已脱离文档的 DOM ⇒ 谁都回收不了 ✗。
     * 只读页面内局部信号的 effect 会跟着那棵树一起被 GC ✓ —— 所以不是每一页都在漏 ✓
     * （实测 accounts / pipelines / history 的 Δ 就是 0 ✓）。
     *
     * ⚠ 与 `dom.ts` 那套"按节点记账"**不冲突**：`setNodes` / `createFor` 走内部的
     * `newEffect()`（**不查** `scope` ✓），列表删一行仍然只停那一行 ✓。
     *
     * ⚠ `dispose()` 是幂等的 ✓，所以这里**不做去重**：一条 effect 只可能被登记一次 ✓
     * （它就是在这一段里建的 ✓）。
     */
    nodes = createNodes(
      ownedEffects((e) => inst.unmounts.push(() => e.dispose()), () => Comp(props, { slots })),
    )
  } finally {
    current = prev
  }
  /**
   * 挂载钩子按**实例自己**的节点登记，不并进父实例。
   *
   * ⚠ 并进父实例是错的：动态建的组件（切页、条件分支、列表里的）在它被创建时
   * 父实例的钩子**早就跑完了**，于是这些钩子再也不会被执行 —— 症状是子页面永远停在
   * 初始状态（`SettingsPage` 的 `onMounted(() => load())` 不跑 ⇒ 一直"加载中"）。
   * 登记到自己的节点上，则由 `insert` 在**它的节点进文档**时逐个放行。
   */
  if (inst.mounts.length) {
    queueMount(nodes, () => {
      for (const m of inst.mounts) m()
    })
  }
  if (inst.unmounts.length) onRemove(nodes, () => {
    for (const u of inst.unmounts) u()
  })
  return nodes
}

/** 挂载后执行（此时节点已在文档里，能查到 `document`）。 */
export function onMounted(cb: () => void): void {
  if (current) current.mounts.push(cb)
  // 不在组件里调用（不该出现）：没有节点可等，立刻跑，别让它永远排队
  else queueMount([], cb)
}

/** 卸载时执行。`onUnmounted` 里那些 `removeEventListener` 靠它收尾。 */
export function onUnmounted(cb: () => void): void {
  current?.unmounts.push(cb)
}

/** 取当前组件的插槽（项目里 `dialog.tsx` 那一处 `useSlots()` 用）。 */
export const useSlots = (): Slots => current?.slots ?? {}

/**
 * 挂到容器上。先建树、再插入、最后统一跑挂载钩子。
 *
 * 返回卸载器，但**项目里没人用它**：整棵树就一次 `mount(App, '#app')`，
 * 页面切换靠信号，没有"换个根再挂一次"的场景。留着是因为它只要一行，
 * 而真要做主题/热替换时没它就得改运行时。
 */
export function mount(App: Component, target: Element | string): () => void {
  const el = typeof target === 'string' ? document.querySelector(target) : target
  if (!el) return () => {}
  const nodes = createComponent(App, {}, {})
  // `insert` 自己会在插完之后跑挂载钩子（含之后动态建出来的组件的）
  insert(el, nodes)
  // 返回卸载器：`remove` 会顺带跑 onUnmounted 的钩子
  return () => remove(nodes)
}
