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

import { effect } from './signal'

/** 渲染结果统一成节点数组：**不引入任何包装元素**，所以 CSS 选择器与布局与原来逐像素一致。 */
export type Nodes = Node[]

export function createNodes(v: unknown): Nodes {
  if (v == null || v === true || v === false) return []
  if (Array.isArray(v)) return v.flat(9) as Nodes
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

/** 批量插入。`anchor` 为 null 即追加到末尾。 */
export function insert(parent: Node, nodes: Nodes, anchor: Node | null = null): void {
  for (const n of nodes) parent.insertBefore(n, anchor)
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
  effect(() => {
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
}

/** 写文本。同值不写 —— 避免无谓的布局/样式重算。 */
export function setText(node: Node, v: unknown): void {
  const s = v == null || v === false || v === true ? '' : String(v)
  if (node.textContent !== s) node.textContent = s
}

/**
 * 普通属性。`null`/`false` 移除、`true` 置空值 —— 与 JSX 里布尔属性的直觉一致。
 * `class` 走 `className`（比 `setAttribute` 少一次字符串解析）。
 */
/** 类名单独一个函数（Vapor 也是 `setClass`）：走 `className` 比 `setAttribute` 少一次解析。 */
export function setClass(node: Node, v: unknown): void {
  const el = node as Element
  const s = v == null || v === false ? '' : String(v)
  if (el.className !== s) el.className = s
}

export function setAttr(node: Node, name: string, v: unknown): void {
  const el = node as Element
  if (v == null || v === false) el.removeAttribute(name)
  else if (v === true) el.setAttribute(name, '')
  else el.setAttribute(name, String(v))
}

/** DOM 属性（`value` / `checked` / `disabled` 这类"改属性比改标签更对"的）。 */
export function setProp(node: Node, name: string, v: unknown): void {
  // biome-ignore lint/suspicious/noExplicitAny: 属性名与节点类型都由编译器静态决定
  ;(node as any)[name] = v == null ? '' : v
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
