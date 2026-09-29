/**
 * DOM 层。
 *
 * 核心思路：**编译期把静态结构变成一行 HTML 字符串，运行期只做动态那几下**。
 *
 * * 静态结构 → `template()` 在**模块加载时**解析一次，之后每个实例只 `cloneNode`。
 * * 动态文本/属性 → 编译期生成 `effect(() => ...)`，只写那一个节点、那一个属性。
 * * 事件 → 编译期知道事件名，运行期就是一次 `addEventListener`，没有包装层。
 *
 * **没有虚拟 DOM**：整棵树自始至终都是真实节点，更新是"指哪儿改哪儿"，
 * 不存在 diff、也没有 vnode 对象。
 *
 * 路径也是编译期算好的：`el.firstChild.nextSibling` 这类取值由编译器生成源码，
 * 运行时不带任何"按标记找插槽"的遍历代码（这是省字节的大头 —— Solid 用
 * `<!--$-->` 注释锚点，我们连那个都省了）。
 */

import { type Effect, effect } from './signal'

/** 渲染结果统一成节点数组：**不引入任何包装元素**，所以 CSS 选择器与布局与原来逐像素一致。 */
export type Nodes = Node[]

export function createNodes(v: unknown): Nodes {
  if (v == null || v === true || v === false) return []
  // 数组里可能夹着 null/false（片段成员的 `cond ? x : null`），先滤掉再铺
  if (Array.isArray(v)) return v.flat(9).filter(Boolean) as Nodes
  return [v instanceof Node ? v : document.createTextNode(String(v))]
}

/**
 * 把一个静态 HTML 串变成"克隆工厂"。
 *
 * 编译期生成、模块级只调用一次；`cloneNode(true)` 比 `innerHTML` 快一个量级，
 * 而且不用每次重新解析。
 */
export function template(html: string): () => Node {
  const box = document.createElement('template')
  box.innerHTML = html
  const content = box.content.firstChild as Node
  return () => content.cloneNode(true) as Node
}

/**
 * 谁拥有哪个 effect：`parent` 被移除时，它名下的 effect 全部销毁。
 *
 * ⚠⚠ 这条记账是**必需**的，不是优化。少了它，被移除子树里的 effect 还订阅着全局信号
 * （`authState` / toast / 后端状态），信号一变就拿着**已不在文档里**的 parent/anchor
 * 去 `insertBefore`，直接抛
 * `Failed to execute 'insertBefore' on 'Node': … is not a child of this node`。
 * 真机（切页 + 全局状态更新）就会撞到 —— fixture 数据静止，所以三条闸门都没照出来。
 */
const owners = new Map<Node, Effect[]>()

function own(parent: Node, eff: Effect): void {
  const list = owners.get(parent)
  if (list) list.push(eff)
  else owners.set(parent, [eff])
}

/** 销毁一棵（已被移除的）子树里登记过的所有 effect。 */
function disposeTree(node: Node): void {
  const list = owners.get(node)
  if (list) {
    owners.delete(node)
    for (const eff of list) eff.dispose()
  }
  for (const child of node.childNodes) disposeTree(child)
}

/** 批量插入。`anchor` 为 null 即追加到末尾。 */
export function insert(parent: Node, nodes: Nodes, anchor: Node | null = null): void {
  for (const n of nodes) parent.insertBefore(n, anchor)
  flushSlots()
  flushMounted()
}

/**
 * 挂载钩子排队：**节点进了文档**才跑（`onMounted` 语义与 Vue 对齐）。
 *
 * ⚠⚠ 不能只在"应用挂载那一刻"flush 一次。组件不只在首屏被创建 —— **条件分支翻转、
 * 列表插入、切页**都会在之后建出新组件（`SettingsPage` 就是 `authState` 变成 `ready`
 * 之后才在抽屉里建的）。只 flush 一次的话，这些组件的 `onMounted` **永远不跑**：
 * 页面停在"加载中"、按钮一直是 disabled，而且**一声不响**（没有异常）。
 *
 * 所以每次 `insert` 之后都试着 flush；还没进文档的（父节点自己还没被插入）留到下一轮。
 */
const mounts: { node: Node | undefined; cb: () => void }[] = []

export { own }

export function queueMount(nodes: Nodes, cb: () => void): void {
  const node = nodes[0]
  // 组件渲染成空（`null`）时没有"进文档"可言，直接跑，别让它永远排在队里
  if (!node) cb()
  else mounts.push({ node, cb })
}

let flushing = false

/** 跑掉"已经进文档"的挂载钩子。有界多轮：钩子里还会建组件（子先父后）。 */
function flushMounted(): void {
  if (flushing) return
  flushing = true
  try {
    for (let pass = 0; pass < 8 && mounts.length; pass++) {
      const list = mounts.splice(0)
      for (const m of list) {
        if (m.node?.isConnected) m.cb()
        else mounts.push(m)
      }
    }
  } finally {
    flushing = false
  }
}

/**
 * 片段（fragment）里的动态成员：**先出一个占位文本节点，等它进了文档再接管**。
 *
 * 为什么必须延后：`setNodes` 要知道父节点（内容插在占位节点之前），而片段在被消费方
 * 插入之前**没有父节点**。踩到的坑正是这个 —— `app.tsx` 的根返回是片段，
 * `{authState !== 'ready' ? null : <div class="drawer">…</div>}` 只在挂载时求值一次，
 * 之后 `authState` 变了没人重跑，应用**永远停在 loading**。
 *
 * Solid 的解法是把这种成员包成 `memo(...)`，由它的数组处理逻辑当响应式槽看待。
 * 这里换成"占位 + 插入后接管"：不需要观察者，也不需要调度器，**同步**建绑定。
 */
let pending: { node: Node; fn: () => unknown }[] = []

export function lazySlot(fn: () => unknown): Node {
  const ph = document.createTextNode('')
  pending.push({ node: ph, fn })
  return ph
}

/**
 * 接管"已经进文档"的槽；还没进文档的（父片段也还没被插入）留给下一轮。
 *
 * ⚠ 必须**有界多轮**而不是递归调用自己：槽的内容里可能还有槽（片段套片段），
 * 但嵌深有限；而递归版（在 `insert` 里直接再 flush）在真实应用上出现了
 * **重复插入** —— 同一页 loading 视图被追加 1,580 次、`#app` 涨到 102 KB，
 * Chrome 虚拟时间因此走不完（回归脚本卡了 7 分钟）。
 */
function flushSlots(): void {
  for (let pass = 0; pass < 8 && pending.length; pass++) {
    const list = pending
    pending = []
    for (const p of list) {
      const parent = p.node.parentNode
      if (parent) setNodes(parent, p.fn, p.node)
      else pending.push(p)
    }
  }
}

/**
 * 卸载登记的清理函数。
 *
 * 键用**节点数组的第一个节点**：组件卸载时它的节点整体被撤掉，
 * 用首节点就能把 `onUnmounted` 的钩子找回来。用 `Map` 而不是在节点上挂属性 ——
 * 不给 DOM 留任何自定义痕迹。
 */
const cleanups = new Map<Node, (() => void)[]>()

export function onRemove(nodes: Nodes, cb: () => void): void {
  const key = nodes[0]
  if (!key) {
    cb()
    return
  }
  const arr = cleanups.get(key)
  if (arr) arr.push(cb)
  else cleanups.set(key, [cb])
}

/**
 * 撤掉一批节点。**所有移除都要走这里** —— 它顺带把组件的卸载钩子跑了
 * （`onUnmounted` 里多半是 `removeEventListener`，漏跑就是内存泄漏）。
 */
export function remove(nodes: Nodes): void {
  // 先销毁这棵子树里的 effect，再摘节点：销毁只解绑订阅，不动 DOM
  for (const n of nodes) disposeTree(n)
  const key = nodes[0]
  if (key) {
    const arr = cleanups.get(key)
    if (arr) {
      cleanups.delete(key)
      for (const cb of arr) cb()
    }
  }
  for (const n of nodes) n.parentNode?.removeChild(n)
}

/**
 * 响应式子节点（Vapor 里叫 `setNodes`）：JSX 里 `{expr}` 这种"一整段内容"由它接管。
 *
 * 每次重跑先撤掉上一次铺进去的节点，再铺新的 —— 这段内容替换的语义下
 * 这就是最省字节的写法（不需要在节点间做 diff，因为这不是"列表"）。
 */
export function setNodes(parent: Node, fn: () => unknown, anchor: Node | null = null): void {
  let cur: Nodes = []
  const eff = effect(() => {
    const v = fn()
    /**
     * 文本快路径（Solid 的 `insertExpression` 同款）：值还是字符串、且位置上就是
     * 我们上次放的那个文本节点时，直接改 `.data`，不删不建。
     * 文本更新是最高频的绑定（改一个计数、刷一条日志），这条省下的是真实的 DOM 操作。
     */
    if (typeof v === 'string' || typeof v === 'number') {
      const only = cur[0]
      if (cur.length === 1 && only && only.nodeType === 3) {
        const t = String(v)
        if ((only as Text).data !== t) (only as Text).data = t
        return
      }
      const t = document.createTextNode(String(v))
      remove(cur)
      cur = [t]
      parent.insertBefore(t, anchor)
      return
    }
    remove(cur)
    cur = createNodes(v)
    insert(parent, cur, anchor)
  })
  // 归属登记：`parent` 被移除时这个 effect 一起销毁（否则它会带着死锚点继续重跑）
  own(parent, eff)
}

/** 写文本。同值不写 —— 避免无谓的布局/样式重算。 */
export function setText(node: Node, v: unknown): void {
  const s = v == null || v === false || v === true ? '' : String(v)
  if (node.textContent !== s) node.textContent = s
}

/**
 * `setAttr` 里"`false` ⇒ 移除属性"的属性名白名单 —— 与 Vue 的 `isSpecialBooleanAttr` 同一份
 * （`checked`/`disabled`/`required` 这些**短路在 `setProp` 上**，根本不走这里）。
 */
const BOOL_ATTR = new Set(['allowfullscreen', 'formnovalidate', 'ismap', 'itemscope', 'nomodule', 'novalidate', 'readonly'])

/** 类名（Vapor 也叫 `setClass`）：HTML 元素走 `className`，比 `setAttribute` 少一次解析。 */
export function setClass(node: Node, v: unknown): void {
  const el = node as Element
  const s = v == null || v === false ? '' : String(v).trim()
  /**
   * ⚠⚠ 不能无条件写 `el.className`：**SVG 元素的 `className` 是只读的**
   * `SVGAnimatedString`，赋值直接抛 `TypeError`（本项目图标全是 `<svg>`，
   * 而 `class` 绑定是每个图标头上的第一个 effect ⇒ 一抛就是整个组件树建不出来）。
   * HTML 元素上 `className` 比 `setAttribute` 省一次解析，所以两路分开走。
   */
  if (typeof el.className === 'string') {
    if (el.className !== s) el.className = s
  } else if (el.getAttribute('class') !== s) {
    el.setAttribute('class', s)
  }
}

/**
 * 普通属性。`null` 移除；`false` 只有布尔属性才移除，其余一律 `String(v)`。
 *
 * ⚠ 原来把 `true` 写成 `""`（"布尔属性只需要存在"的直觉）。**不对**：这条路径收到 `true`
 * 的场景根本不是布尔属性 —— 布尔属性（`disabled`/`checked`/…）在编译器里走 `setProp`。
 * 走这里的是 `{...BASE}` 展开和 `aria-*`，而 `aria-hidden={true}` 必须序列化成
 * `aria-hidden="true"`（`""` 既不是合法 ARIA 值，也和 Vue 的产物对不上）。
 */
export function setAttr(node: Node, name: string, v: unknown): void {
  const el = node as Element
  // `false` 只在**布尔属性**上表示"移除"；其余要序列化成 `"false"`
  // （`spellcheck={false}`、`aria-hidden={false}` 都是这个语义）
  if (v == null || (v === false && BOOL_ATTR.has(name))) el.removeAttribute(name)
  else el.setAttribute(name, v === false ? 'false' : String(v))
}

/** DOM 属性（`checked` / `disabled` 这类"改属性比改标签更对"的）。 */
export function setProp(node: Node, name: string, v: unknown): void {
  // biome-ignore lint/suspicious/noExplicitAny: 属性名与节点类型都由编译器静态决定
  ;(node as any)[name] = v == null ? '' : v
}

/**
 * `value`（输入框 / 下拉框）。**property 与 attribute 都写** —— 与 Vapor 的 `setValue` 一致。
 *
 * ⚠ 只写 property 是不够的：`--dump-dom`、`outerHTML`、`getAttribute('value')`、以及
 * 一切"读标签"的代码都看不到值，而 `<input value={x} readonly>` 这种**只读展示框**
 * 恰恰是只靠它显示的（首屏回归里就是这条把它比出来的）。
 */
export function setValue(node: Node, v: unknown): void {
  const el = node as HTMLInputElement
  const s = v == null ? '' : String(v)
  // `<option>` 的 `value` 是**属性**语义（没有 property 回退），Vapor 也是单独判它
  const old = el.tagName === 'OPTION' ? el.getAttribute('value') : el.value
  if (old !== s) el.value = s
  if (v == null) el.removeAttribute('value')
  else el.setAttribute('value', s)
}

/** 事件。事件名由编译器从 `onClick` 折成 `click`。 */
export function on(node: Node, name: string, fn: (e: Event) => void): void {
  node.addEventListener(name, fn)
}

/**
 * `{...obj}` 展开。项目里 16 处全是 `<svg {...BASE}>`（模块级常量对象），
 * 所以编译期只在**表达式含响应式读取**时才包 effect，否则就是一次性铺属性。
 */
export function spread(node: Node, obj: Record<string, unknown>): void {
  const el = node as Element
  for (const k in obj) {
    const v = obj[k]
    if (k[0] === 'o' && k[1] === 'n') on(el, k.slice(2).toLowerCase(), v as (e: Event) => void)
    else if (k === 'class') setClass(el, v)
    else setAttr(el, k, v)
  }
}
