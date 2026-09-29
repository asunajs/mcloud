/**
 * 性能对拍（lite 侧）。工作量与 vue-side.tsx **逐条对齐**：
 * 挂载 1000 行 / 改一处文本 ×100 / 改三处文本 ×100（不 batch）/ 追加 100 行 ×10 /
 * 反转 ×10 / 卸载。
 *
 * 写法就是编译器该产出的形态（模板串 + 细粒度 effect + 直接监听）。
 */
import { createComponent, createFor, effect, mount, on, ref, setText, template, type Ref } from '../src/index'

interface Item {
  id: number
  label: string
  n: number
}
interface BenchApi {
  mount(sel: string): void
  unmount(): void
  update1(): void
  update3(): void
  append(k: number): void
  reverse(): void
  reshuffle(): void
}

const rowTpl = template('<li class="row"><span></span></li>')
const Row = (props: { label: () => string; n: () => number }) => {
  const li = rowTpl()
  effect(() => setText(li.firstChild as Node, `${props.label()}:${props.n()}`))
  return li
}

const appTpl = template('<div class="bench"><span id="t0"></span><span id="t1"></span><span id="t2"></span><ul id="rows"></ul></div>')

/** 组件体内建的信号，挂到模块级供 runner 驱动。 */
const st: { a?: Ref<number>; b?: Ref<number>; c?: Ref<number>; items?: Ref<Item[]> } = {}
const holder: { unmount?: () => void } = {}

const App = () => {
  const a = ref(0)
  const b = ref(0)
  const c = ref(0)
  const src = (window as unknown as { benchData: { items: Item[] } }).benchData.items
  const items = ref(src.map((x) => ({ ...x })))
  st.a = a
  st.b = b
  st.c = c
  st.items = items

  const el = appTpl() as Element
  // 路径由编译器静态算好：.bench 的子节点依次是 3 个 span + ul#rows
  const t0 = el.firstChild as Node
  const t1 = t0.nextSibling as Node
  const t2 = t1.nextSibling as Node
  const ul = t2.nextSibling as Element

  effect(() => setText(t0, a.value))
  effect(() => setText(t1, b.value))
  effect(() => setText(t2, c.value))
  createFor(
    ul,
    () => items.value,
    (it) => createComponent(Row, { label: () => it.label, n: () => it.n }),
    (it) => it.id,
  )
  on(ul, 'click', () => a.value++)
  return el
}

const mk = (items: Item[], k: number): Item[] =>
  Array.from({ length: k }, (_, i) => ({ id: 1e6 + items.length + i, label: 'y', n: i }))

// biome-ignore lint/suspicious/noExplicitAny: runner 通过 window 拿控制器
;(window as any).liteApi = {
  mount: (sel: string) => {
    holder.unmount = mount(App, sel)
  },
  unmount: () => holder.unmount?.(),
  update1: () => st.a && st.a.value++,
  // ⚠ 三次独立写：本实现同步刷新 ⇒ 刷 3 遍（Vue 靠微任务合成 1 遍）
  update3: () => {
    if (st.a && st.b && st.c) {
      st.a.value++
      st.b.value++
      st.c.value++
    }
  },
  append: (k: number) => {
    if (st.items) st.items.value = [...st.items.value, ...mk(st.items.value, k)]
  },
  reverse: () => {
    if (st.items) st.items.value = [...st.items.value].reverse()
  },
  // 元素身份全不变、只是换了个数组 ⇒ 纯 diff 成本，理论上零次 DOM 操作
  reshuffle: () => {
    if (st.items) st.items.value = [...st.items.value]
  },
} satisfies BenchApi
