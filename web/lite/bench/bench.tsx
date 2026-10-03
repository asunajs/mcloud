/**
 * 性能基准的**被测对象**：一份正常写法的 TSX，由 `lite/vite.ts` 编译。
 *
 * ⚠ 这里刻意**不**手写模板串、`createFor`、`effect`。
 * 上一版 bench 用的是手写目标形态（`lite-side.ts`），量出来的是"运行时裸速"，
 * 把它当成"框架产品有多快"来引，口径偏乐观（见 `docs/review/review-qwen3.8f-260929.md`）。
 * 换成 TSX 之后，编译器的产物质量（少不少一次 effect、路径有没有静态算好）
 * 一起进了这条测量 —— 那才是要盯的数。
 *
 * 也不再有 Vue 那一列：lite 早就不背兼容包袱了，没有对照物时
 * "绝对耗时 + 行为断言"才是能变红的门（`run.mjs`）。
 */

import { batch, mount, ref } from '../src/index'

interface Item {
  id: number
  label: string
  n: number
}

/** 与旧版逐条对齐的工作量：1000 行起步。 */
const N = 1000
const seed = (): Item[] => Array.from({ length: N }, (_, i) => ({ id: i, label: 'x', n: i }))

/**
 * 信号放在**模块作用域**：runner 要从外面驱动写入。
 *
 * 真实应用不会这么写（信号在组件里），但 bench 要的正是"外部改一次 ref，
 * 编译产物花了多少钱"，而组件内的 `ref` 外部拿不到。
 */
const a = ref(0)
const b = ref(0)
const c = ref(0)
const items = ref<Item[]>(seed())

const Row = (props: { label: string; n: number }) => (
  <li class="row">
    {props.label}:{props.n}
  </li>
)

const App = () => (
  <div class="bench">
    <span id="t0">{a.value}</span>
    <span id="t1">{b.value}</span>
    <span id="t2">{c.value}</span>
    <ul id="rows">
      {items.value.map((it) => (
        <Row key={it.id} label={it.label} n={it.n} />
      ))}
    </ul>
  </div>
)

const holder: { unmount?: () => void } = {}
/** 追加行的 id 从这段取，保证跨轮不撞 key。 */
let nextId = 1_000_000

interface BenchApi {
  mount(sel: string): void
  unmount(): void
  reset(): void
  update1(): void
  update3(): void
  update3b(): void
  append(k: number): void
  reverse(): void
  reshuffle(): void
}

declare global {
  interface Window {
    liteApi: BenchApi
  }
}

window.liteApi = {
  mount: (sel: string) => {
    holder.unmount = mount(App, sel)
  },
  unmount: () => holder.unmount?.(),
  // 每轮开工前归零（不计进任何一项耗时）：不然第二轮挂的是 2000 行
  reset: () => {
    a.value = 0
    b.value = 0
    c.value = 0
    items.value = seed()
    nextId = 1_000_000
  },
  update1: () => {
    a.value++
  },
  /** 三次独立写入 —— 量的是"没有合批时刷几遍"。 */
  update3: () => {
    a.value++
    b.value++
    c.value++
  },
  /** 同样三次写入，但包进 `batch` —— 这两行的差值就是合批的收益。 */
  update3b: () => {
    batch(() => {
      a.value++
      b.value++
      c.value++
    })
  },
  append: (k: number) => {
    const from = nextId
    nextId += k
    items.value = [...items.value, ...Array.from({ length: k }, (_, i) => ({ id: from + i, label: 'y', n: i }))]
  },
  reverse: () => {
    items.value = [...items.value].reverse()
  },
  // 元素身份全不变、只是换了个数组 ⇒ 纯 diff 成本，理想情况下 0 次 DOM 写
  reshuffle: () => {
    items.value = [...items.value]
  },
}
