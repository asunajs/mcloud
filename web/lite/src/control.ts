/**
 * 控制流。
 *
 * 这两件事（条件 / 列表）如果交给"运行时 diff 一棵树"来做，就得引入虚拟 DOM；
 * 交给编译期生成"只在必要的那个位置重建"才符合本项目"指哪儿改哪儿"的路子。
 *
 * 两者都用**父节点 + 锚点**的形态，而不是返回一个"片段对象"：
 * 少一层抽象就少一堆记账代码，而且**不产生任何包装节点**（CSS 选择器不受影响）。
 */

import { createNodes, insert, remove, type Nodes } from './dom'
import { effect } from './signal'

/**
 * 列表渲染。编译期把 `list.map((x) => <Row/>)` 折成这里（Solid 是交给 `<For>` 组件，
 * 但本项目的源码写的就是 `.map` + `key=`，所以做成编译期原语）。
 *
 * 锚点语义与 Solid 的 `insert` 一致：**内容插在 `anchor` 之前**，`anchor === null` 表示
 * 追加到末尾 —— 这样列表放在元素中间也不会挤坏后面的静态兄弟节点，**不需要注释锚点**。
 *
 * ⚠ 迭代必须**倒序**，且"已就位"的判据是**组尾的下一个兄弟正好是 cursor**：
 * 正序 + `nodes[0] === cursor` 的写法看着对，实际每次更新把整列表 `insertBefore` 搬一遍
 * （实测"同序重排 2000 行"要 10ms，而那一项本该零次 DOM 操作）。倒序后降到 1.1ms。
 */
export function createFor<T>(
  parent: Node,
  list: () => T[],
  render: (item: T, index: number) => unknown,
  key?: (item: T, index: number) => unknown,
  anchor: Node | null = null,
): void {
  /** key → 行。跨轮复用同一个 Map/Set，别每轮重建（那是 O(n) 的分配）。 */
  const rows = new Map<unknown, { item: T; nodes: Nodes }>()
  const seen = new Set<unknown>()

  effect(() => {
    const items = list()
    seen.clear()
    let cursor: Node | null = anchor
    for (let i = items.length - 1; i >= 0; i--) {
      const item = items[i]
      const k = key ? key(item, i) : i
      let row = rows.get(k)
      if (row && row.item !== item) {
        // 同一个 key 但内容对象换了 ⇒ 重建这一行
        remove(row.nodes)
        row = undefined
      }
      // ⚠ `fresh` 不能省：新建的行**还没进 DOM**，而"不在文档里"的节点
      // `nextSibling` 也是 `null` —— 与"已经是最后一个孩子"无法区分。
      // 少了这个标志，第一行会被判定为"已就位"而根本不插入，第二行再插到它前面
      // 就抛 `NotFoundError: … is not a child of this node`（实测踩过）。
      let fresh = false
      if (!row) {
        row = { item, nodes: createNodes(render(item, i)) }
        rows.set(k, row)
        fresh = true
      }
      seen.add(k)
      const last = row.nodes[row.nodes.length - 1]
      if (fresh || !last || last.nextSibling !== cursor) insert(parent, row.nodes, cursor)
      cursor = row.nodes[0] ?? cursor
    }
    if (rows.size !== seen.size) {
      for (const [k, row] of rows) {
        if (!seen.has(k)) {
          remove(row.nodes)
          rows.delete(k)
        }
      }
    }
  })
}
