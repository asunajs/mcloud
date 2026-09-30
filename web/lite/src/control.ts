/**
 * 控制流。
 *
 * 这两件事（条件 / 列表）如果交给"运行时 diff 一棵树"来做，就得引入虚拟 DOM；
 * 交给编译期生成"只在必要的那个位置重建"才符合本项目"指哪儿改哪儿"的路子。
 *
 * 两者都用**父节点 + 锚点**的形态，而不是返回一个"片段对象"：
 * 少一层抽象就少一堆记账代码，而且**不产生任何包装节点**（CSS 选择器不受影响）。
 */

import { createNodes, insert, own, remove, type Nodes } from './dom'
import { newEffect } from './signal'

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
 *
 * ⚠⚠ 但**新建**的行必须**正序**插进去（倒序走是为了"看到的都已就位"，不是为了"倒着插"）。
 *
 * 倒着插不只是顺序难看，它有**可观测**的副作用：`<select>` 没有独立的选中状态，
 * 选项插进一个"还没有任何选中项"的下拉框时，浏览器会自动选中**第一个**被插入的选项
 * （Chrome 实测：`append(b); insertBefore(a,b)` ⇒ `value === 'b'`）。于是倒着插 = 默认选到
 * **最后一项**，而 Vapor 是整批正序插 = 默认选到**第一项** —— 计划页那个"执行的任务"下拉框
 * 就是这么一侧显示"每日签到"、另一侧显示"直播口令"的（交互回归抓出来的）。
 * 所以倒序走，但把连续新建的行攒成一段，到边界再用**正序**、以"这一段后面那个节点"为锚整段插入。
 */
export function createFor<T>(
  parent: Node,
  list: () => T[],
  render: (item: T, index: number) => unknown,
  key?: (item: T, index: number) => unknown,
  anchor: Node | null = null,
  /**
   * 这一行的内容**读到了索引参数** ⇒ 位置就是内容的一部分（`第 {i+1} 步`、
   * `disabled={i === 0}`）。由编译器判定并传进来，不用人写。
   *
   * 见下面 `row.i !== i` 那段的取舍。
   */
  positional = false,
): void {
  /** key → 行。跨轮复用同一个 Map/Set，别每轮重建（那是 O(n) 的分配）。 */
  const rows = new Map<unknown, { item: T; i: number; nodes: Nodes }>()
  const seen = new Set<unknown>()

  const eff = newEffect(() => {
    // ⚠⚠ 同上：列表整体被外部替换掉之后，每行都脱开了 ⇒ 这条 time effect 该退休，
    // 否则它会把整张表**再插一遍**（真机症状：越刷新内容越多）。
    if (rows.size) {
      let alive = false
      for (const row of rows.values()) for (const n of row.nodes) if (n.parentNode) { alive = true; break }
      if (!alive) {
        eff?.dispose()
        return
      }
    }
    const items = list()
    seen.clear()
    let cursor: Node | null = anchor
    /** 连续新建的一段（倒序攒着），`runAnchor` 是这一段**后面**那个节点。 */
    let run: Nodes[] = []
    let runAnchor: Node | null = null
    const flush = () => {
      // 倒序攒的，就倒着取出来 ⇒ 实际插入顺序是**正序**（见上面那段注释）
      for (let j = run.length - 1; j >= 0; j--) insert(parent, run[j], runAnchor)
      run = []
    }
    for (let i = items.length - 1; i >= 0; i--) {
      const item = items[i]
      const k = key ? key(item, i) : i
      let row = rows.get(k)
      /**
       * ⚠ **位置变了要不要重建，取决于这一行的内容读没读索引**（`positional`）。
       *
       * - 读了（`第 {i+1} 步`、`disabled={i === 0}`）：`render` 拿到的 `i` 是个**普通数字**，
       *   被烧进这一行的绑定闭包里。复用的话 DOM 顺序对了、**步号与禁用态却留在旧位置**
       *   —— 编排页的步骤块就是这么露馅的 ⇒ 必须重建。
       * - 没读：位置不进内容 ⇒ 复用。这一条**有断言钉着**
       *   （`demo/main.tsx`："重排是搬动而不是重建（旧节点仍在文档里）"），
       *   因为搬动保住的不是字节数，是**行内的真实状态**：输入框的光标与焦点、
       *   滚动位置、跑到一半的 CSS 过渡。全重建会把这些悄悄抹掉。
       *
       * 为什么不做成"响应式的 index"（Solid 的 `index()`）：那要把每个 `.map` 的第二个
       * 参数换成函数，21 个业务文件全得改，而本项目只有一处真的按位置出内容。
       * 判据在编译期就能算准（渲染体里扫一眼有没有读那个标识符），所以这里用标志位换
       * 0 行 API 变更；真有"复用 + 实时位置"的需求再上 accessor。
       */
      if (row && (row.item !== item || (positional && row.i !== i))) {
        // 同一个 key 但内容/位置换了 ⇒ 重建这一行
        remove(row.nodes)
        row = undefined
      }
      // ⚠ `fresh` 不能省：新建的行**还没进 DOM**，而"不在文档里"的节点
      // `nextSibling` 也是 `null` —— 与"已经是最后一个孩子"无法区分。
      // 少了这个标志，第一行会被判定为"已就位"而根本不插入，第二行再插到它前面
      // 就抛 `NotFoundError: … is not a child of this node`（实测踩过）。
      let fresh = false
      if (!row) {
        row = { item, i, nodes: createNodes(render(item, i)) }
        rows.set(k, row)
        fresh = true
      }
      seen.add(k)
      if (fresh) {
        if (!run.length) runAnchor = cursor
        run.push(row.nodes)
      } else {
        if (run.length) flush()
        const last = row.nodes[row.nodes.length - 1]
        if (!last || last.nextSibling !== cursor) insert(parent, row.nodes, cursor)
      }
      cursor = row.nodes[0] ?? cursor
    }
    if (run.length) flush()
    if (rows.size !== seen.size) {
      for (const [k, row] of rows) {
        if (!seen.has(k)) {
          remove(row.nodes)
          rows.delete(k)
        }
      }
    }
  })
  // 归属登记：列表的父节点被移除时，这个 effect 与它建的每一行一起销毁
  own(parent, eff)
}
