/** 性能对拍（Vue Vapor 侧）。工作量与 lite-side.ts **逐条对齐**。 */
import { createVaporApp, ref, type Ref } from 'vue'

interface Item {
  id: number
  label: string
  n: number
}

const Row = (props: { label: string; n: number }) => (
  <li class="row">
    <span>
      {props.label}:{props.n}
    </span>
  </li>
)

const st: { a?: Ref<number>; b?: Ref<number>; c?: Ref<number>; items?: Ref<Item[]> } = {}
const holder: { app?: { unmount(): void } } = {}

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

  return (
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
}

const mk = (items: Item[], k: number): Item[] =>
  Array.from({ length: k }, (_, i) => ({ id: 1e6 + items.length + i, label: 'y', n: i }))

// biome-ignore lint/suspicious/noExplicitAny: runner 通过 window 拿控制器
;(window as any).vueApi = {
  mount: (sel: string) => {
    holder.app = createVaporApp(App).mount(sel) as unknown as { unmount(): void }
  },
  unmount: () => holder.app?.unmount(),
  update1: () => st.a && st.a.value++,
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
  reshuffle: () => {
    if (st.items) st.items.value = [...st.items.value]
  },
}
