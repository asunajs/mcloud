/**
 * lite 的**验收样例**：这份文件是**真实 TSX**，由 `lite/vite.ts` 编译，
 * 跑的还是那 22 条行为断言 —— 也就是说，断言验证的是**编译器的产物**，
 * 不是手写的目标形态。
 *
 * 写法刻意与项目现状一致（`ref` / `.value` / `onMounted` / `watch` / `useSlots` / TSX）。
 */

import { batch, createVaporApp, mount, onMounted, onUnmounted, ref, useSlots, watch } from '../src/index'

/** 自测用的计数器。 */
const demo = { mounted: 0, unmounted: 0, mountedInDoc: false, late: 0, watches: [] as string[] }

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

const LateMounted = () => {
  onMounted(() => {
    demo.late++
  })
  return <p id="late-body">late</p>
}

/**
 * **没有 `key` 的 `.map()`**，而且它后面还有一个静态兄弟 ⇒ 那个兄弟就是它的锚点。
 * 这一格专门守着"锚点被当成 `createFor` 的 key 传进去"那个 bug（`r is not a function`）。
 */
const Keyless = () => (
  <div id="kbox">
    {['x', 'y'].map((s) => (
      <b id={`k-${s}`}>{s}</b>
    ))}
    <i id="k-after">end</i>
  </div>
)

const Split = () => (
  <>
    <h4 id="frag-head">片段头</h4>
    {showFrag.value ? <p id="frag-branch">A</p> : null}
    {showFrag.value ? <p id="frag-b2">B</p> : null}
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

/**
 * 与应用**同形状**的用例：第一个片段成员是一个**本地函数返回的 JSX**（`gate()`，
 * 对非目标状态返回 null），第二个成员是同一信号驱动的条件 —— app.tsx 的
 * `{authGate()}` + `{authState.value !== 'ready' ? null : <div class="drawer">}` 就是这样。
 */
const stage = ref<'checking' | 'ready'>('checking')
const gate = () => (stage.value === 'checking' ? <div id="stage-loading">L</div> : null)
const GateBox = () => (
  <>
    {gate()}
    {stage.value !== 'ready' ? null : <p id="stage-ready">R</p>}
    {stage.value !== 'ready' ? null : <LateMounted />}
  </>
)

// ── 片段用例的断言 ───────────────────────────────────────────────────────────
mount(Split, '#frag')

createVaporApp(GateBox).mount('#frag3')
stage.value = 'ready'
ok('门形状：第一个槽被清空', !document.getElementById('stage-loading'))
ok('门形状：第二个槽补上了内容', !!document.getElementById('stage-ready'))
// ⚠ 这个组件是**挂载之后**才建的：它自己的 `onMounted` 必须跑（否则页面永远停在初始态）
ok('动态建出来的组件：onMounted 也跑了', demo.late === 1, `late=${demo.late}`)

const frag = () => document.getElementById('frag') as HTMLElement
const fragRows = () => [...document.querySelectorAll('#frag-list > li')].map((n) => n.textContent)
const fragCount = () => frag().childNodes.length

const baseCount = fragCount()
ok('片段：静态兄弟节点渲染了', document.getElementById('frag-head')?.textContent === '片段头')
ok('片段：动态成员（条件分支）渲染了', document.querySelectorAll('#frag-branch').length === 1)
ok('片段：列表渲染了', fragRows().join('|') === 'p|q', fragRows().join('|'))

showFrag.value = false
ok('片段：条件切换后分支消失', document.querySelectorAll('#frag-branch').length === 0)
ok('片段：紧邻的第二个槽也跟着消失', document.querySelectorAll('#frag-b2').length === 0)
ok('片段：紧邻的第二个槽切回来', (() => { showFrag.value = true; const n = document.querySelectorAll('#frag-b2').length; return n === 1 })())
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

/**
 * 三个**在真实应用上踩到的**形状（都让设置页整体白屏，而且都不报错 —— 错误被应用侧的
 * `try/catch` 吞了，`window.onerror` 一声不响）：
 *
 * 1. `<label … />` 这种**非空元素的自闭合写法**：生成 HTML 时若不写闭合标签，解析器会把
 *    后面的兄弟节点**吞成它的子节点**，编译期算好的 `childNodes[i]` 全部错位一格；
 * 2. SVG 上的 `class` 绑定：`SVGElement.className` 是**只读的** `SVGAnimatedString`，
 *    无条件写它直接抛 `TypeError`（本项目图标全是 `<svg>`）；
 * 3. `将结束{' '}<b>{x}</b>` 这种**文本里夹元素**的写法：相邻文本在解析后合成一个节点，
 *    按"每段文本各占一位"数下标，就会拿到一个**文本节点当父节点**去 `insertBefore`。
 */
const miscText = ref('当前')
const Misc = () => (
  <div id="misc-box">
    <label class="misc-label" />
    <b id="misc-after-label">B</b>
    <svg id="misc-svg" class={miscText.value} aria-hidden={true} viewBox="0 0 4 4">
      <path d="M0 0" />
    </svg>
    <span id="misc-mixed">
      将结束{' '}
      <b id="misc-bold">{miscText.value}</b>{' '}
      在本浏览器
    </span>
    <span id="misc-hazard">前{miscText.value}后</span>
    <button id="misc-current" type="button" aria-current={miscText.value === '当前' ? 'page' : undefined}>
      C
    </button>
    {/* 空白语义：与 `vue-jsx-vapor` 的产物逐字符对齐（见 compiler.ts 的 jsxText 规则表） */}
    <span id="ws-a">耗时 {miscText.value}秒</span>
    <span id="ws-b">
      第一行
      第二行
    </span>
    <span id="ws-c">  a  b  </span>
  </div>
)

/**
 * ⚠ 真机上报过的一个错：**被移除的子树里 effect 不销毁**，于是它带着已经不存在的
 * 锚点继续重跑，抛
 * `Failed to execute 'insertBefore' on 'Node': … is not a child of this node`。
 * 这里刻意造出那个形状：先卸载子树，再改它读过的**全局**信号。
 */
const alive = ref(true)
const gCount = ref(0)
const Stale = () => <p id="stale-inner">{gCount.value}</p>
const StaleHost = () => <div id="stale-host">{alive.value ? <Stale /> : null}</div>
mount(StaleHost, '#stale')
alive.value = false
const staleGone = !document.getElementById('stale-inner')
gCount.value = 1
ok(
  '卸载后的子树不再重跑（没有 insertBefore 死锚点）',
  staleGone && !document.getElementById('stale-inner') && !document.getElementById('err'),
  `gone=${staleGone} err=${document.getElementById('err')?.textContent?.split('\n')[0] ?? '无'}`,
)

/**
 * ⚠⚠ 真机上报过的**同形**复现（任务中心点刷新）：我们持有的锚点**被第三方摘掉**、父节点还在，
 * 此时 `insertBefore` 抛 `… is not a child of this node` 并**打断整次更新**。
 * 这条路径以前没有护栏（前几轮"实测 0 异常"全在干净 profile 里跑的，根本没有第三方）。
 * 注意必须让动态子节点是**组件/元素**（纯文本走的是 setText，碰不到 insert）。
 * 摘掉运行时那两行防御后，这条必须**变红**，否则它就是摆设。
 */
const Flag = ref(true)
const FlagChild = () => <b id="c-child">C</b>
const FlagHost = () => (
  <p id="third-inner">
    {Flag.value ? <FlagChild /> : null}
    <span>x</span>
  </p>
)
mount(FlagHost, '#third')
const thirdEl = document.getElementById('third-inner')!
const anchorNode = [...thirdEl.childNodes].find((n) => n.nodeType === 8)
anchorNode?.remove() // 第三方动了 DOM：锚点脱开，父节点还在
let thirdErr = ''
try {
  Flag.value = false // 卸掉
  Flag.value = true // 再装上 ⇒ 用已脱开的锚点 insert
} catch (e) {
  thirdErr = String(e)
}
ok(
  '锚点被摘掉时不抛错（退化为追加到末尾）',
  !thirdErr && thirdEl.textContent?.includes('C'),
  thirdErr.slice(0, 70) || `anchor=${!!anchorNode} text=${thirdEl.textContent}`,
)

mount(Keyless, '#keyless')
ok(
  '无 key 的 .map()：锚点没被当成 key（顺序正确）',
  [...document.querySelectorAll('#kbox > *')].map((n) => n.id).join('|') === 'k-x|k-y|k-after',
  [...document.querySelectorAll('#kbox > *')].map((n) => n.id).join('|'),
)

mount(Misc, '#misc')
const miscBox = () => document.getElementById('misc-box') as HTMLElement
ok('自闭合非空元素：后面的兄弟还是兄弟', miscBox().children.length === 9, `children=${miscBox().children.length}`)
ok('自闭合非空元素：没把兄弟吞成子节点', document.querySelector('label.misc-label')?.children.length === 0)
ok('自闭合非空元素：兄弟顺序对', (miscBox().children[1] as HTMLElement)?.id === 'misc-after-label')
ok('SVG 上的 class 绑定生效（不抛只读错误）', document.getElementById('misc-svg')?.getAttribute('class') === '当前')
// 布尔 true 在 ARIA 上必须序列化成 "true"（`""` 不是合法 ARIA 值；本项目图标用 `{...BASE}` 展开它）
ok('aria-hidden={true} 序列化成 "true"', document.getElementById('misc-svg')?.getAttribute('aria-hidden') === 'true', JSON.stringify(document.getElementById('misc-svg')?.getAttribute('aria-hidden')))
ok('文本里夹元素：结构正确', document.getElementById('misc-mixed')?.textContent === '将结束 当前 在本浏览器', JSON.stringify(document.getElementById('misc-mixed')?.textContent))
ok('文本里夹元素：<b> 是 span 的直接子节点', (document.getElementById('misc-bold')?.parentNode as HTMLElement)?.id === 'misc-mixed')
ok('两侧都是文本的动态成员：顺序正确', document.getElementById('misc-hazard')?.textContent === '前当前后', JSON.stringify(document.getElementById('misc-hazard')?.textContent))

ok('空白：单行文本尾随空格保留', document.getElementById('ws-a')?.textContent === '耗时 当前秒', JSON.stringify(document.getElementById('ws-a')?.textContent))
ok('空白：跨行文本保留换行与缩进', (() => { const t = document.getElementById('ws-b')?.textContent ?? ''; return t.startsWith('第一行\n') && t.endsWith('第二行') })(), JSON.stringify(document.getElementById('ws-b')?.textContent))
ok('空白：单行内部空格原样保留', document.getElementById('ws-c')?.textContent === '  a  b  ', JSON.stringify(document.getElementById('ws-c')?.textContent))
ok('动态属性带上了属性名', document.getElementById('misc-current')?.getAttribute('aria-current') === 'page', JSON.stringify(document.getElementById('misc-current')?.outerHTML))
miscText.value = 'x2'
ok('动态属性的 undefined 会移除属性', document.getElementById('misc-current')?.getAttribute('aria-current') === null)
ok('SVG 上的 class 会更新', document.getElementById('misc-svg')?.getAttribute('class') === 'x2')
ok('文本里夹元素：内部的 `<b>` 跟着更新', document.getElementById('misc-bold')?.textContent === 'x2')
ok('两侧都是文本的动态成员：更新后顺序仍然对', document.getElementById('misc-hazard')?.textContent === '前x2后', JSON.stringify(document.getElementById('misc-hazard')?.textContent))
ok(
  '两侧都是文本的动态成员：两段静态文本没被并掉（占位注释把它们分开了）',
  (document.getElementById('misc-hazard')?.childNodes[0] as Text)?.data === '前' && (document.getElementById('misc-hazard')?.lastChild as Text)?.data === '后',
  `childNodes=${document.getElementById('misc-hazard')?.childNodes.length}`,
)

const fails = out.filter((l) => l.startsWith('FAIL')).length
const pre = document.createElement('pre')
pre.id = 'result'
pre.textContent = `${out.join('\n')}\n\n${fails ? `✗ ${fails} 条失败` : `✓ 全过（${out.length} 条）`}`
document.body.appendChild(pre)
