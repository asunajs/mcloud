/**
 * 响应式核心。
 *
 * 设计取舍（都是为了小 + 够用）：
 *
 * 1. **同步 + 批量**，不做微任务调度队列。本项目的依赖链只有"信号 → DOM 写入"这一层，
 *    没有 computed 套 computed 的深链，同步执行省掉整套调度器（约 300B）。
 *    需要一次改多个信号再统一刷 DOM 时用 `batch()`。
 * 2. **不用 Proxy**。信号是类实例，读写走访问器。Proxy 每次读写都要过陷阱，
 *    既慢又占字节，而本项目根本没有"运行时才知道的键"。
 * 3. **依赖记账用双向 Set**：effect 记着自己的依赖（重跑前要清），信号记着自己的订阅者
 *    （变更时要通知）。两边都留一份，才能在重跑时精确解绑。
 *
 * 这里刻意**只有 4 个 API**（ref / effect / batch / watch）—— 普查显示全项目就用了这些：
 * `ref` 66 处、`watch` 1 处、`computed` **0 处**、`reactive` 0 处、`nextTick` 0 处。
 * ⇒ `computed` 与 `field`/`createStore` 那一族（0 处使用）在 2026-09-30 随"不必兼容 Vue"
 * 一起删了，省下多少字节由 `node lite/size.mjs` 量（它保留着"全量 vs 裁剪后"两档口径）。
 * 要加回来的话它是**独立的一段**（派生值 = 一条订阅 fn 的 effect + 一个 RefImpl 输出），
 * 运行时其余部分一行都不用改。
 *
 * 名字也不背 Vue 的包袱：`ref` / `.value` 是这里唯一留下的旧名，因为它读起来就是
 * "一个可写的格子"，而换掉的收益远小于代价（21 个文件改 import）。
 * 只为"从 Vue 迁过来不用动代码"而留的壳（`createVaporApp` / `defineVaporComponent` /
 * `renderEffect`）已经删了 —— 入口直接写 `mount(App, '#app')`。
 */

/** 当前正在运行的 effect。用模块级单值而不是给每个函数传参：少一层参数、少一堆字节。 */
let active: Effect | null = null

/**
 * 当前的**组件作用域**：这段时间内新建的 effect 交给它（组件卸载时统一销毁）。
 *
 * 为什么需要：`watch` / `computed` 建的 effect 不属于任何 DOM 节点，走不到 `dom.ts`
 * 那套"按节点记账、节点被摘走时销毁"的路子。少了这一层，组件里写的 `watch` 在它卸载后
 * 还活着 —— 回调读到的是旧实例的状态，而没人再负责停它。
 *
 * ⚠ DOM 绑定（`setNodes` / `createFor`）**不走这里**：它们已按节点记账，那一份粒度更细
 * （列表删一行就该停那一行的 effect，而不是等整页卸载）。所以运行时内部用 `newEffect()`，
 * 公开的 `effect()` 才挂作用域。
 */
let scope: ((e: Effect) => void) | null = null

/** 在 `fn` 执行期间把新建的 effect 登记给 `add`（嵌套安全：进时存、出时恢复）。 */
export function ownedEffects<T>(add: (e: Effect) => void, fn: () => T): T {
  const prev = scope
  scope = add
  try {
    return fn()
  } finally {
    scope = prev
  }
}

/** batch 深度。>0 时变更只入队，归零时统一排空（同一个 effect 一轮只跑一次）。 */
let depth = 0
const pending = new Set<Effect>()

/**
 * effect 重入的上限：`A → B → A` 这种互写在这里变成一条可读的抛错，而不是把栈打爆。
 *
 * 为什么阈值是 100：正常嵌套只有一两层（effect 里写信号 ⇒ 订阅者立刻跑）。
 *
 * ⚠ 它**不指名**是哪个 effect（要指名得给每个 effect 存标签，那是白付的字节）。
 * 它比 `RangeError: Maximum call stack size exceeded` 有用的地方是**有边界、能停下**：
 * 爆栈时中途的 DOM 写入已经把现场冲掉了，而这条错误至少告诉你"是互写，不是数据太多"。
 */
let nesting = 0
const MAX_NESTING = 100

export class Effect {
  /** 我依赖了哪些信号 —— 重跑前要逐个解绑，否则依赖会越滚越大。 */
  readonly deps = new Set<RefImpl<unknown>>()

  /** 已经销毁：不再跑，也不再被通知。见 `dispose()`。 */
  disposed = false

  constructor(readonly fn: () => void) {
    this.run()
  }

  /**
   * 销毁：解绑所有依赖并停用。
   *
   * ⚠⚠ 不销毁会**报错**，不只是泄漏：被移除的子树（切页、条件分支、列表删行）里的
   * effect 还订阅着**全局**信号（`authState` / toast / 后端状态）。信号一变它们就重跑，
   * 拿着**已经不在文档里的** `parent`/`anchor` 去 `insertBefore`：
   *
   * ```
   * Failed to execute 'insertBefore' on 'Node': The node before which the new node
   * is to be inserted is not a child of this node.
   * ```
   *
   * 谁负责销毁：节点被移除时由 `dom.ts` 的 `remove()` 统一做（它按"谁拥有这个节点"记账）。
   */
  dispose(): void {
    this.disposed = true
    for (const d of this.deps) d.subs.delete(this)
    this.deps.clear()
  }

  run(): void {
    if (this.disposed) return
    if (nesting >= MAX_NESTING) {
      // 抛出去之前先把状态还原：`active`/`nesting` 由 finally 管，但这条 fn 不该再跑
      // ⚠ 文案刻意**短**：这是生产路径上的守卫（不能像诊断那样被 DEV 折掉），
      // 而长文案是直接进产物的字节。要点保住：是谁出的问题（循环更新）+ 往哪查
      // （信号互相写）+ 上限值（`MAX_NESTING`，拼进去便于对读数）。
      throw new Error('[lite] 循环更新：effect 复入超过 ' + MAX_NESTING + ' 层，已中断（检查两个信号是否互相写）')
    }
    for (const d of this.deps) d.subs.delete(this)
    this.deps.clear()

    const prev = active
    active = this
    nesting++
    try {
      this.fn()
    } finally {
      nesting--
      active = prev
    }
  }

  /** 被通知时：批量中先入队，否则立刻跑。 */
  notify(): void {
    if (depth) pending.add(this)
    else this.run()
  }
}

/**
 * 建一个 effect。**内部用**（`dom.ts` 的绑定、`control.ts` 的列表走这里）。
 *
 * 与公开的 `effect()` 差在哪：不挂组件作用域。那些 effect 已经有更细的归属了
 * （`dom.ts` 的 `owners`：按**节点**记账，节点被摘走时销毁），再往组件实例的数组里
 * 塞一份纯属重复记账。
 */
export function newEffect(fn: () => void): Effect {
  return new Effect(fn)
}

class RefImpl<T> {
  readonly subs = new Set<Effect>()

  constructor(private v: T) {}

  /**
   * ⚠ 必须是**访问器** `get value()`。第一版把 `get()`/`set()` 写成普通方法，
   * `.value` 就退化成一个数据属性 —— 读写全绕过依赖记账，
   * 症状是"初始渲染对、之后点什么都没反应"（自测就是这么抓出来的）。
   */
  get value(): T {
    if (active) {
      // 双向记账：effect 记得我，我记得 effect
      this.subs.add(active)
      active.deps.add(this as RefImpl<unknown>)
    }
    return this.v
  }

  set value(n: T) {
    // Object.is：NaN 与 +0/-0 的边界与 Vue 一致，顺带避免"同值重设"触发无谓的 DOM 写
    if (Object.is(n, this.v)) return
    this.v = n
    // ⚠ 订阅者可能在回调里退订（例如条件分支把某个子树删了），所以不能原地遍历。
    // 但 0 个与 1 个是绝大多数情形（本项目每个信号平均不到 1.5 个订阅者），
    // 为它们各分配一个数组是白付的 GC 压力 —— 只有两个以上才复制。
    const n0 = this.subs.size
    if (!n0) return
    if (n0 === 1) {
      const only = this.subs.values().next().value as Effect
      if (!only.disposed) only.notify()
      return
    }
    for (const e of [...this.subs]) e.notify()
  }
}

/** 信号。刻意沿用 Vue 的 `ref` 名字与 `.value` 读写 —— 迁移时只改 import 那一行。 */
export interface Ref<T> {
  value: T
}

export function ref<T>(value: T): Ref<T> {
  return new RefImpl(value) as unknown as Ref<T>
}

/**
 * 注册一个副作用：立刻跑一次，之后依赖变了自动重跑。返回句柄以便**销毁**（见 `Effect.dispose`）。
 *
 * 在组件里调用时它会自动挂到该组件的销毁清单上（见 `ownedEffects`）——
 * 组件卸载即停，不需要调用方自己记得 dispose。运行时内部的绑定不走这条路。
 */
export function effect(fn: () => void): Effect {
  const e = new Effect(fn)
  scope?.(e)
  return e
}

/** 把一批变更合成一次刷新。 */
export function batch<T>(fn: () => T): T {
  depth++
  try {
    return fn()
  } finally {
    if (!--depth) drain()
  }
}

/**
 * 排空 batch 队列。
 *
 * 两条要点：
 * 1. **排空期间仍然算批量**（`depth++` 包住这一轮）：跑一个 effect 时写信号，
 *    变更会进队列而不是立刻同步跑 ⇒ 一轮里同一个 effect 最多跑一次。
 *    旧写法在 `depth` 已经归零之后才跑队列，"batch 里再改 batch"会当场同步刷，
 *    一次事件把 DOM 写好几遍。
 * 2. **循环有界**：队列一直不空就是有人互写，抛错而不是转到浏览器卡死。
 */
function drain(): void {
  let round = 0
  while (pending.size) {
    if (++round > MAX_NESTING) {
      pending.clear()
      throw new Error('[lite] 循环更新：batch 队列排不空（超过 ' + MAX_NESTING + ' 轮，检查信号是否互相写）')
    }
    const q = [...pending]
    pending.clear()
    depth++
    try {
      for (const e of q) if (!e.disposed) e.run()
    } finally {
      depth--
    }
  }
}

/**
 * 监听一个信号，**初始不触发**（与 Vue 的 `watch` 语义一致 —— 项目里那一处
 * 正是靠"初始不触发"避免在未登录时打 `/api/status`）。
 *
 * 实现上先读一次当前值（此时没有 active effect，所以不建依赖），
 * 再把它放进一个 effect 里比较。
 *
 * 返回 effect 句柄：在组件里创建时它会随组件销毁（见 `ownedEffects`），
 * 在组件外面创建（例如模块级）就用这个句柄自己停。
 */
export function watch<T>(source: Ref<T>, cb: (value: T, oldValue: T) => void): Effect {
  let old = source.value
  const e = new Effect(() => {
    const v = source.value
    if (!Object.is(v, old)) {
      const prev = old
      old = v
      cb(v, prev)
    }
  })
  scope?.(e)
  return e
}
