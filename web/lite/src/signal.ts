/**
 * 响应式核心。
 *
 * 设计取舍（都是为了小 + 够用）：
 *
 * 1. **同步 + 批量**，不做微任务调度队列。本项目的依赖链只有"信号 → DOM 写入"这一层，
 *    没有 computed 套 computed 的深链，同步执行省掉整套调度器（约 300B）。
 *    需要一次改多个信号再统一刷 DOM 时用 `batch()`。
 * 2. **不用 Proxy**。信号是类实例；`store` 的字段用 `defineProperty` 变成 getter/setter。
 *    Proxy 每次读写都要过陷阱，既慢又占字节，而本项目根本没有"运行时才知道的键"。
 * 3. **依赖记账用双向 Set**：effect 记着自己的依赖（重跑前要清），信号记着自己的订阅者
 *    （变更时要通知）。两边都留一份，才能在重跑时精确解绑。
 *
 * 这里刻意**只有 4 个 API**（ref / effect / batch / watch）—— 普查显示全项目就用了这些：
 * `ref` 66 处、`watch` 1 处、`computed` **0 处**、`reactive` 0 处、`nextTick` 0 处。
 */

/** 当前正在运行的 effect。用模块级单值而不是给每个函数传参：少一层参数、少一堆字节。 */
let active: Effect | null = null

/** batch 深度。>0 时变更只入队，归零时统一跑一遍（同一个 effect 只会跑一次）。 */
let depth = 0
const pending = new Set<Effect>()

class Effect {
  /** 我依赖了哪些信号 —— 重跑前要逐个解绑，否则依赖会越滚越大。 */
  readonly deps = new Set<RefImpl<unknown>>()

  constructor(readonly fn: () => void) {
    this.run()
  }

  run(): void {
    for (const d of this.deps) d.subs.delete(this)
    this.deps.clear()

    const prev = active
    active = this
    try {
      this.fn()
    } finally {
      active = prev
    }
  }

  /** 被通知时：批量中先入队，否则立刻跑。 */
  notify(): void {
    if (depth) pending.add(this)
    else this.run()
  }
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
    // 复制一份再遍历：订阅者可能在回调里退订（例如条件分支把某个子树删了）
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
 * 不带 `.value` 的字段读写器。`store`（见 store.ts）用它把普通对象的字段
 * 换成 getter/setter —— 于是 `s.count++` 也是响应式的，而**不需要 Proxy**。
 */
export function field<T>(v: T) {
  const r = new RefImpl(v)
  return { get: () => r.value, set: (n: T) => (r.value = n) }
}

/** 注册一个副作用：立刻跑一次，之后依赖变了自动重跑。 */
export function effect(fn: () => void): void {
  new Effect(fn)
}

/** 把一批变更合成一次刷新。 */
export function batch<T>(fn: () => T): T {
  depth++
  try {
    return fn()
  } finally {
    if (!--depth) {
      const q = [...pending]
      pending.clear()
      for (const e of q) e.run()
    }
  }
}

/**
 * 监听一个信号，**初始不触发**（与 Vue 的 `watch` 语义一致 —— 项目里那一处
 * 正是靠"初始不触发"避免在未登录时打 `/api/status`）。
 *
 * 实现上先读一次当前值（此时没有 active effect，所以不建依赖），
 * 再把它放进一个 effect 里比较。
 */
export function watch<T>(source: Ref<T>, cb: (value: T, oldValue: T) => void): void {
  let old = source.value
  new Effect(() => {
    const v = source.value
    if (!Object.is(v, old)) {
      const prev = old
      old = v
      cb(v, prev)
    }
  })
}

/**
 * 派生值。**普查显示项目一处都没用** —— 列在这里是为了说明"按需裁剪"的边界：
 * 这段大约 100B（gzip），不想要就整段删掉，运行时其余部分一行都不用改。
 */
export function computed<T>(fn: () => T): Ref<T> {
  const out = new RefImpl(undefined as T)
  // 不追踪 out.value 的写入：写它不该反过来建立依赖
  new Effect(() => {
    const v = fn()
    if (!Object.is(v, out.value)) out.value = v
  })
  return out as unknown as Ref<T>
}
