/**
 * 日志列表的**窗口化（虚拟）渲染**：只画看得见的那几行。
 *
 * # 为什么需要（2026-10-05 用户口径：「详细日志虚拟加载」）
 *
 * 日志是**一行一行铺**的，而一屏只看得到十几行 ⇒ 全铺进 DOM 的代价是
 * 节点数与日志长度**线性增长**：`/logs` 页取满环（2000 行 ✓）、运行日志面板留 500 行 ✓。
 * 实测（当时取 800 行、无强制 GC，口径同 DevTools 的「性能监视器」）：不加窗时节点读数
 * 在 **3,039–7,839** 之间来回跳 —— 因为那一页每 2 s 换一批数据，而整批都是新对象
 * ⇒ 每 2 s 造/丢约 1.6k 个节点 ✗。加窗之后只有窗口内那几十行在动 ✓。
 *
 * # 做法（经典的"窗口 + 上下垫片"）
 *
 * * 只渲染 `[start, end)` 这一段 ✓，两端各放一个**垫片**把总高度撑住 ⇒
 *   滚动条的长度与位置仍然是**真实总高** ✓（不是"只画可见的、滚动条乱跳"✗）。
 * * 行高**按内容量一次、缓存起来**：日志会折行（`break-words`）⇒ **不能假设固定行高** ✗。
 *   没量过的行先用 [`ESTIMATE`] 估 ✓（`leading-5` = 20px ✓）。
 * * 量高在**渲染之后**（调用方的 `requestAnimationFrame` 里读 `getBoundingClientRect`），
 *   并且**只有高度真的变了**才让调用方重算窗口 ✓ —— 否则会"量一次 → 重渲染 → 再量"
 *   转个不停 ✗（`measure()` 的返回值就是这个信号）。
 *
 * ⚠ 本模块**不碰 DOM 结构**：它只算下标与像素、只读尺寸 ✓，铺节点是调用方的事 ✓。
 * ⚠ 调用方要保证每行带 `data-k` = **这一行自己的高度缓存键**（就是 `window()` 里
 * 那份 `keys` 的对应元素 ✓）—— 量高时靠它认行 ✓。
 *
 * # ⚠⚠ 2026-10-06：契约从"绝对下标 `data-i`"改成了"自带键 `data-k`"
 *
 * 起因（台账第 96 行 ②）：日志页每 2 s 轮询一次，而**一个字都没变的行**也每拍换一次
 * 节点 ✗ —— `createFor` 的复用条件是"**key 与 item 都没变**"（`@asunajs/lite` 的 `src/control.ts:94`），
 * 而那一页的 item 是每帧现造的包装对象、`data-i` 又要求行**读下标参数**
 * （读了 ⇒ 编译器判 `positional` ⇒ 位置一变就重建 ✗，而轮询每拍都在挪位置 ✗）。
 *
 * 改成"行自带自己的键"之后：行**不依赖下标** ⇒ 只要 item 身份稳（调用方按日志行对象
 * 缓存行对象 ✓），行节点就会被复用 ✓ —— 用户能看见的收益是**正在选中的日志文字不再被
 * 每 2 s 清掉** ✓（`regress/bulk.mjs` 有断言钉着 ✓）。
 */

/** 没量过时的估计行高（`text-xs leading-5` = 20px ✓，与 `log-panel` / `logs-page` 同一档）。 */
export const ESTIMATE = 20

/**
 * 窗口上下各多画几行。
 *
 * 为什么要有：滚轮/触控板是**按像素**滚的，留几行余量才不会在窗口边界上
 * 露出空白（"滚一格 → 有一瞬间是空的 → 下一帧才补上"✗）。6 行 ≈ 一行半屏 ✓。
 */
const OVERSCAN = 6

/**
 * 高度缓存的条数上限。
 *
 * 缓存键是**行文本** ⇒ 长期开着的页面会把它撑大（用户翻过的每一行都留一条）✓。
 * 撑到上限就整体清掉重来 —— 代价只是重新量一遍看得见的那几十行 ✓，
 * 好过让它无限涨（那是**内存**，与"省节点"是同一件事的两面 ✓）。
 */
const MAX_CACHE = 4000

export interface WindowRange {
  /** 要渲染的下标区间 `[start, end)` ✓ */
  start: number
  end: number
  /** 上垫片高度（px）= 前面那些行的总高 ✓ */
  top: number
  /** 下垫片高度（px）= 后面那些行的总高 ✓ */
  bottom: number
}

export interface VirtualRows {
  /**
   * 算这一帧该渲染哪一段。**纯函数**（在渲染槽里调用 ✓）：同样的输入给同样的输出 ✓。
   *
   * `top` = 容器当前 `scrollTop`；`viewport` = 容器 `clientHeight`；
   * `keys` = 每一行的**高度缓存键**（调用方给，通常就是它渲染出来的那串文本 ✓）。
   */
  window(top: number, viewport: number, keys: string[]): WindowRange
  /**
   * 渲染后量高（量的是 `container` 里带 `data-k` 的那些行 ✓）。
   *
   * ⚠ **不需要调用方再给一份键**：行上那个 `data-k` 就是它的高度缓存键 ✓
   * （见文件头那条契约变更：行不许再依赖绝对下标 ✓）。
   *
   * 返回 `true` = **有行的高度和缓存不一样**（调用方据此把版本号 +1、重算窗口 ✓）；
   * 返回 `false` = 什么都没变（**别**重渲染，否则就是死循环 ✗）。
   */
  measure(container: HTMLElement | null): boolean
}

/**
 * 建一份窗口状态（**每个列表一份**：`LogPanel` 与日志页各建各的 ✓）。
 *
 * ⚠ 建在**组件体里**要小心：组件体重渲染会新建一份 ⇒ 缓存全丢（表现是"滚动时行高一直
 * 在估、垫片总在变"✗）。所以调用方应当把它建在组件**外面**（模块级）或只建一次 ✓ ——
 * 本项目这两个调用点都是"一页一个面板"，模块级单例够用 ✓（见两个调用点的注释）。
 */
export const createVirtualRows = (): VirtualRows => {
  const heights = new Map<string, number>()

  /** 行 → 高度（没量过就估 ✓）。 */
  const heightOf = (key: string): number => heights.get(key) ?? ESTIMATE

  /** 前缀和：`sums[i]` = 前 `i` 行的总高 ⇒ `sums[n]` 就是总高 ✓。 */
  let sums: number[] = [0]
  let cached: string[] = []

  /** 前缀和只在**键序列变了**时重算（每帧重算也才 O(n)，但没必要 ✓）。 */
  const ensure = (keys: string[]): void => {
    if (cached.length === keys.length && sums.length === keys.length + 1) {
      let same = true
      for (let i = 0; i < keys.length; i++) {
        if (cached[i] !== keys[i]) {
          same = false
          break
        }
      }
      if (same) return
    }
    sums = new Array(keys.length + 1)
    sums[0] = 0
    for (let i = 0; i < keys.length; i++) sums[i + 1] = sums[i] + heightOf(keys[i])
    cached = keys
  }

  const windowRange = (top: number, viewport: number, keys: string[]): WindowRange => {
    ensure(keys)
    const n = keys.length
    const total = sums[n]
    /**
     * 二分找**第一行**满足"它的底边 > top"（`sums[i + 1] > top`）✓。
     * 线性扫也行，但窗口是滚动的、每帧都算 ⇒ 二分省下的这点在长列表上是白捡的 ✓。
     */
    let lo = 0
    let hi = n
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (sums[mid + 1] <= top) lo = mid + 1
      else hi = mid
    }
    const start = Math.max(0, lo - OVERSCAN)
    let end = lo
    const limit = top + viewport
    while (end < n && sums[end] < limit) end++
    end = Math.min(n, end + OVERSCAN)
    return { start, end, top: sums[start], bottom: total - sums[end] }
  }

  const measure = (container: HTMLElement | null): boolean => {
    if (!container) return false
    if (heights.size > MAX_CACHE) heights.clear()
    let changed = false
    for (const el of container.querySelectorAll<HTMLElement>('[data-k]')) {
      const k = el.dataset.k
      // 空键 = 这一行没给键（不该发生 ✓）⇒ 跳过，别把一堆行都算成"同一个键" ✗
      if (!k) continue
      // 取整：亚像素差值（缩放/DPR）会让"变了"永远成立 ⇒ 每帧都重渲染 ✗
      const h = Math.round(el.getBoundingClientRect().height)
      if (h <= 0) continue
      if (heights.get(k) !== h) {
        heights.set(k, h)
        changed = true
      }
    }
    return changed
  }

  return { window: windowRange, measure }
}
