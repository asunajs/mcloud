/**
 * lite 的**验收样例**：这份文件是**真实 TSX**，由 `lite/vite.ts` 编译，
 * 跑的还是那 22 条行为断言 —— 也就是说，断言验证的是**编译器的产物**，
 * 不是手写的目标形态。
 *
 * 写法刻意与项目现状一致（`ref` / `.value` / `onMounted` / `watch` / `useSlots` / TSX）。
 */

import { batch, mount, onMounted, onUnmounted, ref, useSlots, watch } from '../src/index'

/** 自测用的计数器。 */
const demo = { mounted: 0, unmounted: 0, mountedInDoc: false, watches: [] as string[] }

// 原样：一个普通函数组件，两个动态绑定 + 两个生命周期钩子
const Row = (props: { label: string; n: number }) => {
  onMounted(() => demo.mounted++)
  onUnmounted(() => demo.unmounted++)
  return (
    <li class={props.n % 2 ? 'odd' : 'even'}>
      {props.label}:{props.n}
    </li>
  )
}

// 插槽：`useSlots()` + `slots.default?.()`
const Panel = (props: { title: string }) => {
  const slots = useSlots()
  return (
    <section class="panel">
      <h3>{props.title}</h3>
      {slots.default?.()}
    </section>
  )
}

const App = () => {
  const count = ref(0)
  const items = ref([
    { id: 1, label: 'a', n: 1 },
    { id: 2, label: 'b', n: 2 },
    { id: 3, label: 'c', n: 3 },
  ])
  const show = ref(true)

  watch(count, (v) => demo.watches.push(String(v)))
  onMounted(() => {
    demo.mountedInDoc = !!document.getElementById('list')
  })

  return (
    <div class="app">
      <div class="hd">
        <span id="count">{count.value}</span>
        <button id="inc" onClick={() => count.value++}>
          +1
        </button>
        <button
          id="add"
          onClick={() =>
            batch(() => {
              items.value = [...items.value, { id: Date.now(), label: 'x', n: items.value.length + 1 }]
              count.value++
            })
          }
        >
          加
        </button>
        <button id="rev" onClick={() => (items.value = [...items.value].reverse())}>
          反转
        </button>
        <button id="del" onClick={() => (items.value = items.value.slice(1))}>
          删首个
        </button>
        <button id="tog" onClick={() => (show.value = !show.value)}>
          切换
        </button>
      </div>
      <Panel title={`共 ${items.value.length} 条`}>
        <ul id="list">
          {items.value.map((it) => (
            <Row key={it.id} label={it.label} n={it.n} />
          ))}
        </ul>
      </Panel>
      {show.value ? <p id="branch">可见</p> : null}
    </div>
  )
}

/**
 * ⚠ **回归用例：组件返回片段**（fragment）。
 *
 * 真实应用的根组件就是这个形状（`app.tsx` 的 `return (<>…</>)`），而它一度只在挂载时
 * 求值一次 —— `authState` 变了没人重跑，应用永远卡在 loading。demo 原来的 20 条断言里
 * **没有一个组件返回片段**，所以漏了。这里把它钉住：既断言内容，也断言**没有重复节点**。
 */
const showFrag = ref(true)
const fragItems = ref([
  { id: 1, label: 'p' },
  { id: 2, label: 'q' },
])

const Split = () => (
  <>
    <h4 id="frag-head">片段头</h4>
    {showFrag.value ? <p id="frag-branch">A</p> : null}
    <ul id="frag-list">
      {fragItems.value.map((it) => (
        <li key={it.id}>{it.label}</li>
      ))}
    </ul>
  </>
)

// ── 断言（与手写版逐条一致）───────────────────────────────────────────────────
const out: string[] = []
const ok = (name: string, cond: boolean, extra = '') => out.push(`${cond ? 'PASS' : 'FAIL'}  ${name}${extra ? '  ← ' + extra : ''}`)

mount(App, '#app')

const $ = (s: string) => document.querySelector(s) as HTMLElement
const rows = () => [...document.querySelectorAll('#list > li')].map((n) => n.textContent)

ok('初始文本由绑定写入', $('#count')?.textContent === '0')
ok('初始列表 3 行且文本正确', rows().join('|') === 'a:1|b:2|c:3', rows().join('|'))
ok('动态 class 生效', (document.querySelector('#list > li') as HTMLElement)?.className === 'odd')
ok('插槽内容铺进了子组件', ($('section.panel > h3') as HTMLElement)?.textContent === '共 3 条')
ok('插槽位置正确（列表在 panel 里）', !!$('section.panel > ul#list'))
ok('onMounted 在入文档之后跑', demo.mountedInDoc)
ok('子组件挂载钩子都跑了', demo.mounted === 3, `mounted=${demo.mounted}`)
ok('条件分支初始渲染', !!document.getElementById('branch'))

$('#inc').click()
ok('点击后计数更新', $('#count')?.textContent === '1')
ok('watch 收到变更（初始不触发）', demo.watches.join(',') === '1', demo.watches.join(','))

const before = rows().length
$('#add').click()
ok('追加一行', rows().length === before + 1, rows().join('|'))
ok('batch 内两个信号都生效', $('#count')?.textContent === '2')
ok('追加没有重建已有行', rows().slice(0, 3).join('|') === 'a:1|b:2|c:3', rows().join('|'))

const firstNode = document.querySelector('#list > li')
$('#rev').click()
ok('反转后 DOM 顺序正确', rows().join('|') === 'x:4|c:3|b:2|a:1', rows().join('|'))
ok('重排是搬动而不是重建（旧节点仍在文档里）', !!firstNode?.isConnected)

const unmountedBefore = demo.unmounted
$('#del').click()
ok('删一行', rows().length === before, rows().join('|'))
ok('被删行的 onUnmounted 跑了', demo.unmounted === unmountedBefore + 1, `unmounted=${demo.unmounted}`)

$('#tog').click()
ok('条件切到 else 分支', !document.getElementById('branch'))
$('#tog').click()
ok('条件切回来', !!document.getElementById('branch'))

ok('列表标题随数据更新', ($('section.panel > h3') as HTMLElement)?.textContent === `共 ${rows().length} 条`, ($('section.panel > h3') as HTMLElement)?.textContent)

// ── 片段用例的断言 ───────────────────────────────────────────────────────────
mount(Split, '#frag')

const frag = () => document.getElementById('frag') as HTMLElement
const fragRows = () => [...document.querySelectorAll('#frag-list > li')].map((n) => n.textContent)
const fragCount = () => frag().childNodes.length

const baseCount = fragCount()
ok('片段：静态兄弟节点渲染了', document.getElementById('frag-head')?.textContent === '片段头')
ok('片段：动态成员（条件分支）渲染了', document.querySelectorAll('#frag-branch').length === 1)
ok('片段：列表渲染了', fragRows().join('|') === 'p|q', fragRows().join('|'))

showFrag.value = false
ok('片段：条件切换后分支消失', document.querySelectorAll('#frag-branch').length === 0)
showFrag.value = true
ok('片段：条件切回来只有一个', document.querySelectorAll('#frag-branch').length === 1)

for (let i = 0; i < 5; i++) showFrag.value = !showFrag.value
showFrag.value = true
ok('片段：反复切换后没有重复节点', fragCount() === baseCount, `childNodes ${fragCount()} vs ${baseCount}`)
ok('片段：反复切换后静态头仍只有一个', document.querySelectorAll('#frag-head').length === 1)

const fragFirst = document.querySelector('#frag-list > li')
fragItems.value = [...fragItems.value, { id: 3, label: 'r' }]
ok('片段：列表追加一行', fragRows().join('|') === 'p|q|r', fragRows().join('|'))
ok('片段：列表复用了已有节点', !!fragFirst?.isConnected && fragFirst === document.querySelector('#frag-list > li'))
ok('片段：追加后也没重复节点', fragCount() === baseCount, `childNodes ${fragCount()} vs ${baseCount}`)

const fails = out.filter((l) => l.startsWith('FAIL')).length
const pre = document.createElement('pre')
pre.id = 'result'
pre.textContent = `${out.join('\n')}\n\n${fails ? `✗ ${fails} 条失败` : `✓ 全过（${out.length} 条）`}`
document.body.appendChild(pre)
