#!/usr/bin/env node
/**
 * 「**大体量**」验收 —— 分页与窗口化**只有数据量够大时才现形**。
 *
 * # 为什么单开这一条（2026-10-05 用户口径：「任务记录分页，详细日志虚拟加载」）
 *
 * 提交档那几条用的是 `ready` 夹具：**4 条运行记录 / 60 行服务端日志**。这个量级下：
 *
 * * 历史页不翻页也**一屏放得下** ⇒ 分页写错（页码算错、翻页不换内容、换档位不回第 1 页）
 *   全都"看起来正常" ✗；
 * * 日志页 60 行全铺也才 1200 个节点 ⇒ **窗口化失效**（退回全铺）根本量不出来 ✗。
 *
 * 而真实数据是 **113 条运行记录 / 日志环 2000 行 / 单次运行 600 行日志**（本机实测 ✓）。
 * 所以这里就地造一份大体量夹具，只量"**该省的地方有没有省、该稳的地方稳不稳**"：
 *
 * 1. `/logs`：2000 行只铺**窗口内那几十行**，且**节点数**（DevTools 性能监视器那个口径）
 *    与总行数脱钩 ✓；滚动条长度/
 *    位置仍是**真实总高**（垫片撑住了 ✓）；首屏贴底（看最新的 ✓）；**滚到中间跨一次
 *    轮询位置不动、容器还是同一个节点**；贴底时跨轮询仍贴底；
 * 2. `/logs`：**2.5 秒内的 `/api/logs` 请求数必须是个位数** —— 这是"页面反复重挂载"
 *    那个坑的直接钉子（实测踩到过 **2 秒 218 次** ✗，起因是在挂载路径上写了信号）；
 * 3. 历史页：一页 20 条、页码读数对、翻页真换内容、**换档位回第 1 页**；
 * 4. 「运行日志」弹窗：600 行只铺窗口内那几十行。
 *
 * ⚠ 交互细节（点按钮的时序、两侧文本一致）仍归 `interact.mjs` ✓，这里**不重复**
 * —— 这一条只回答"数据一大，上面四件事还成不成立" ✓。
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { FIXTURES, gotoJs, openSession, serve, sleep, VARIANTS } from './lib.mjs'

const DIST = new URL('../../dist/', import.meta.url).pathname
const PORT = 48931
// ⚠ `openSession` 的 `profile` 必须显式传（漏传会在 cwd 下建一个叫 `undefined` 的用户目录）
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-bulk-'))

/**
 * 体量：照本机真实数据取整 —— **2000 行日志就是服务端的环容量**（`logbuf.rs` 的
 * `RING_CAPACITY` ✓，日志页 2026-10-05 起也**取满环** ✓），113 条运行取 100，
 * 单次运行日志取 600 ✓。
 */
const LOG_ROWS = 2000
const RUN_COUNT = 100
const RUN_LOG_ROWS = 600
/** 窗口外还铺出来的行数上限：真实实现是"视口 + overscan"（几十行）✓，全铺会到 2000 ✗。 */
const MAX_ROWS = 80
/**
 * DOM 节点数上限（**"性能监视器"那个口径**：`Memory.getDOMCounters` 的 `nodes`，
 * 含脱离文档但仍活着的 ✓）。
 *
 * ⚠ 这是用户最初那张截图直接量的东西（2026-10-05：DevTools 性能监视器
 * **DOM nodes 30,124** ✗），所以窗口化到底成不成，最终要看它 ✓：
 * 2000 行全铺是**万级** ✗，窗口化之后应当与"总行数"**无关**（几百到一千出头 ✓）。
 * 留 1500 的余量：整页除了日志行还有外壳、工具栏、垫片 ✓。
 */
const MAX_NODES = 1500

/**
 * 大体量夹具（**就地定义**，不塞进 `lib.mjs`：只有这一条门禁要它 ✓）。
 *
 * ⚠ 用 `VARIANTS.ready` 当底座：登录态、账号、任务那些基础 fixture 全都要 ✓，
 * 这里只把三个"会随数据量放大"的端点换掉 ✓。
 */
const logs = []
for (let i = 0; i < LOG_ROWS; i++) {
  logs.push({
    level: i % 7 === 0 ? 'warn' : 'info',
    target: 'mcloud_engine::scheduler',
    // 长度不一：日志会折行 ⇒ 行高不能假定固定 20px（窗口化按内容量高 ✓）
    message: `第 ${i} 行 —— 大体量验收用的日志，长一点以便测折行：${'x'.repeat(i % 40)}`,
    at_ms: 1790581000000 + i * 1000,
  })
}
const runLogs = []
for (let i = 0; i < RUN_LOG_ROWS; i++) {
  runLogs.push({
    level: 'info',
    kind: i % 5 === 0 ? 'success' : 'info',
    message: `运行日志第 ${i} 行`,
    at_ms: 1790581000000 + i * 1000,
  })
}
const runs = []
for (let i = 0; i < RUN_COUNT; i++) {
  const base = FIXTURES['/api/runs'][i % FIXTURES['/api/runs'].length]
  runs.push({ ...base, run_id: 9000 + i, started_at_ms: base.started_at_ms - i * 60000 })
}

VARIANTS.__bulk = {
  ...VARIANTS.ready,
  'GET /api/runs': { handler: () => ({ status: 200, body: runs }) },
  'GET /api/runs/9000': { handler: () => ({ status: 200, body: { ...runs[0], logs: runLogs } }) },
  'GET /api/logs': { handler: () => ({ status: 200, body: logs }) },
}

/**
 * ⚠ 先做一次**纯源码的交叉核对**（不碰浏览器 ✓）：前端的 `RING_LINES` 与服务端
 * `logbuf.rs` 的 `RING_CAPACITY` 必须一样、而且**本门禁的夹具大小就是它** ✓。
 *
 * 为什么要有这一条：这两个数**在各自的构建单元里**（前端 TS / Rust ✓，没有共享常量
 * ⇒ 只能靠对账 ✓）。漂了的后果不是崩溃而是**静默少给**（服务端 `limit.clamp` 会把多要的
 * 截掉 ⇒ 用户以为"全都在"，其实只拿到环里的一部分 ✗）—— 正是这一类"功能全对、数不对"
 * 只有门禁能看出来 ✓。
 *
 * ⚠ 它跟着浏览器门禁跑（没 chrome 时整条是"⊘ 跳过 不算通过"✓）⇒ 这一小段也一起被跳过 ✗。
 * 挪进纯 Node 的门禁就得同时改 CI 的 web job 清单，权衡之后先放这里 ✓。
 */
const WEB_DIR = new URL('../../', import.meta.url).pathname
const REPO_DIR = new URL('../../../', import.meta.url).pathname
const readConst = (file, re) => {
  const src = fs.readFileSync(file, 'utf8')
  const m = src.match(re)
  return m ? Number(m[1]) : Number.NaN
}
const rustRing = readConst(path.join(REPO_DIR, 'crates/mcloud-server/src/logbuf.rs'), /RING_CAPACITY\s*:\s*usize\s*=\s*(\d+)/)
const pageRing = readConst(path.join(WEB_DIR, 'src/pages/logs-page.tsx'), /RING_LINES\s*=\s*(\d+)/)

const cases = []
const check = (name, ok, detail = '') => {
  cases.push({ name, ok })
  console.log(`  ${ok ? '✅' : '✗'} ${name}${ok || !detail ? '' : `\n     ${detail}`}`)
}

check(
  `前端 RING_LINES(${pageRing}) 与服务端 RING_CAPACITY(${rustRing}) 一致`,
  pageRing === rustRing && Number.isFinite(pageRing),
  '两个数在各自的构建单元里，只能对账；漂了是"静默少给"而不是崩溃',
)
check(`本门禁的夹具大小 LOG_ROWS(${LOG_ROWS}) 就是环容量(${rustRing})`, LOG_ROWS === rustRing, `LOG_ROWS=${LOG_ROWS}`)

let server
let session
try {
  server = await serve(DIST, PORT, '__bulk')
  session = await openSession({ port: PORT, route: '/logs', profile: PROFILE, width: 1280 })
  const { cdp } = session

  const probe = async (expr) => JSON.parse(await cdp.eval(`JSON.stringify(${expr})`))
  /** 轮询等一个条件成立（别用"睡够就行"：那是 flaky 的温床 ✓）。 */
  const waitFor = async (expr, timeoutMs = 8000) => {
    const deadline = Date.now() + timeoutMs
    while (Date.now() < deadline) {
      if (await cdp.eval(expr)) return true
      await sleep(100)
    }
    return false
  }

  const LOGS = `(() => {
    const el = document.getElementById('logs-scroll')
    if (!el) return { missing: true }
    const rows = [...el.querySelectorAll('[data-i]')]
    return {
      rendered: rows.length,
      first: Number(rows[0]?.dataset.i ?? -1),
      last: Number(rows[rows.length - 1]?.dataset.i ?? -1),
      top: el.scrollTop,
      height: el.scrollHeight,
      client: el.clientHeight,
      probe: el.__probe ?? 0,
    }
  })()`

  // ── ① /logs：800 行只铺窗口内那几十行 ──
  const ready = await waitFor(`document.querySelectorAll('#logs-scroll [data-i]').length > 0`, 10000)
  const a = await probe(LOGS)
  check(`日志页大体量下仍有内容（夹具 ${LOG_ROWS} 行 = 服务端的环容量）`, ready && !a.missing && a.rendered > 0, JSON.stringify(a))
  check(
    `只铺窗口内那几十行（渲染 ${a.rendered} 行 / 总 ${LOG_ROWS} 行，上限 ${MAX_ROWS}）`,
    a.rendered > 0 && a.rendered <= MAX_ROWS,
    JSON.stringify(a),
  )
  check(
    '滚动条长度仍是**真实总高**（垫片撑住了，不是只画可见的那点）',
    a.height > LOG_ROWS * 18,
    `scrollHeight=${a.height}（800 行 × 18px 下限）`,
  )
  check('首屏在底部（这一页是"看最新"）', a.height - a.top - a.client <= 4, JSON.stringify(a))
  check('铺出来的最后一行就是最新那行', a.last >= LOG_ROWS - 1 - MAX_ROWS, `last=${a.last}`)

  /**
   * ⚠⚠ **DOM 节点数**（`Memory.getDOMCounters` 的 `nodes`，含脱离文档但活着的 ✓）——
   * 这就是用户最初那张截图（DevTools 性能监视器 **30,124** ✗）量的东西，窗口化到底
   * 成不成最终看它 ✓。先强制 GC 两次（口径与"性能监视器"对齐：它**不**强制 GC，
   * 所以这里只会更宽松、不会更严 ✗）。
   */
  await cdp.send('HeapProfiler.collectGarbage')
  await cdp.send('HeapProfiler.collectGarbage')
  const counters = await cdp.send('Memory.getDOMCounters')
  const inDoc = Number(await cdp.eval(`document.querySelectorAll('*').length`))
  check(
    `DOM 节点数与总行数脱钩（${LOG_ROWS} 行只占 ${counters.nodes} 个节点，上限 ${MAX_NODES}）`,
    counters.nodes > 0 && counters.nodes <= MAX_NODES,
    `nodes=${counters.nodes} 文档内=${inDoc} 脱离=${counters.nodes - inDoc}`,
  )

  // ── ② 页面不许反复重挂载（那个坑的症状：每挂一次又 load() ⇒ 请求风暴） ──
  const reqs = async () =>
    Number(await cdp.eval(`performance.getEntriesByType('resource').filter((e) => e.name.includes('/api/logs')).length`))
  const n0 = await reqs()
  await sleep(2500)
  const n1 = await reqs()
  check(`2.5 s 内 /api/logs 请求数是个位数（实测踩过 2 秒 218 次 ✗）`, n1 - n0 >= 1 && n1 - n0 <= 6, `${n0} → ${n1}`)

  // ── ③ 滚到中间：跨一次轮询位置不动，且容器还是同一个节点 ──
  const set = await probe(`(() => {
    const el = document.getElementById('logs-scroll')
    el.__probe = 1
    el.scrollTop = Math.round((el.scrollHeight - el.clientHeight) / 2)
    window.__bulkTop = el.scrollTop
    return { top: el.scrollTop, height: el.scrollHeight }
  })()`)
  await sleep(2600)
  const mid = await probe(LOGS)
  check(
    '滚到中间后跨一次轮询不被弹走（位置不动 + 容器同一个节点）',
    mid.probe === 1 && mid.top === (await cdp.eval('window.__bulkTop')),
    `set=${JSON.stringify(set)} now=${JSON.stringify(mid)}`,
  )
  check('窗口跟着滚（渲染出的下标在中段，不是钉在 0）', mid.first > 50 && mid.last < LOG_ROWS, `i=${mid.first}..${mid.last}`)

  // ── ④ 贴底跟随 ──
  await cdp.eval(`(() => { const el = document.getElementById('logs-scroll'); el.scrollTop = el.scrollHeight; return true })()`)
  await sleep(2600)
  const bot = await probe(LOGS)
  check('贴底后跨一次轮询仍贴着底', bot.height - bot.top - bot.client <= 4, JSON.stringify(bot))

  // ── ⑤ 历史页：一页 20 条 + 翻页 + 换档位回第 1 页 ──
  const HIST = `(() => {
    const rows = document.querySelectorAll('#app table tbody tr')
    const btn = (t) => [...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === t)
    return {
      rows: rows.length,
      firstId: rows[0]?.querySelector('td .font-mono')?.textContent ?? '',
      page: [...document.querySelectorAll('#app span')].find((s) => s.textContent.includes('页 · 共'))?.textContent.trim() ?? '',
      prevDisabled: btn('上一页')?.disabled,
      nextDisabled: btn('下一页')?.disabled,
    }
  })()`
  await cdp.eval(gotoJs('history'))
  const histReady = await waitFor(`document.querySelectorAll('#app table tbody tr').length > 0`, 10000)
  let h = await probe(HIST)
  check('历史页 100 条里一页只铺 20 条', histReady && h.rows === 20, JSON.stringify(h))
  check(
    '页码读数对（100 条 ⇒ 5 页）',
    h.page.includes(`第 1 / ${Math.ceil(RUN_COUNT / 20)} 页`) && h.page.includes(`共 ${RUN_COUNT} 条`),
    h.page,
  )
  check('第 1 页「上一页」禁用、「下一页」可用', h.prevDisabled === true && h.nextDisabled === false, JSON.stringify(h))
  const firstId = h.firstId
  await cdp.eval(`(() => { [...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '下一页').click(); return true })()`)
  await sleep(300)
  h = await probe(HIST)
  check('翻到第 2 页且内容真换了', h.page.includes('第 2 /') && h.rows === 20 && h.firstId !== firstId, `${firstId} → ${h.firstId}`)
  await cdp.eval(`(() => { const b = [...document.querySelectorAll('#app [aria-label]')].find((x) => (x.getAttribute('aria-label') || '').startsWith('筛选：')); b.click(); return true })()`)
  await sleep(300)
  h = await probe(HIST)
  check('换档位自动回第 1 页（否则"停在第 5 页看到空列表"）', h.page.includes('第 1 /'), h.page)

  // ── ⑥ 「运行日志」弹窗：600 行只铺窗口内那几十行 ──
  await cdp.eval(`(() => { const b = [...document.querySelectorAll('#app table tbody tr:first-child button')].find((x) => x.textContent.includes('日志')); b.click(); return true })()`)
  const dialogReady = await waitFor(`document.querySelectorAll('dialog[open] [data-i]').length > 0`, 10000)
  const panel = await probe(`(() => {
    const box = document.querySelector('dialog[open] .h-64')
    return {
      title: document.querySelector('dialog[open] h2')?.textContent?.trim() ?? '',
      rendered: document.querySelectorAll('dialog[open] [data-i]').length,
      height: box?.scrollHeight ?? 0,
    }
  })()`)
  check('运行日志弹窗开起来了', dialogReady && panel.title === '运行日志', JSON.stringify(panel))
  check(`600 行只铺窗口内那几十行（渲染 ${panel.rendered} 行）`, panel.rendered > 0 && panel.rendered <= MAX_ROWS, JSON.stringify(panel))
  check('弹窗滚动条长度是真实总高（600 行 ≈ 12,000px）', panel.height > 6000, `scrollHeight=${panel.height}`)

  /**
   * ⚠ 异常要读 `session.errors` —— 那是 `openSession` 真正在收的两路
   * （`Runtime.exceptionThrown` + `console.error` ✓）。
   * 前一版我写的是 `window.__bulkErrors`，**没有任何东西会去设它** ⇒ 这条断言
   * 永远是 0 ⇒ "✅"是假的 ✗（假绿灯比红灯更坏）。
   */
  check('页面 0 条未捕获异常 / console.error', session.errors.length === 0, session.errors.join(' / '))
} finally {
  /**
   * ⚠⚠ 收尸**必须显式**：`openSession` 只返回 `{ child, cdp, errors, width }`，**没有** `close`
   * ⇒ 不 kill 的话这个 chrome 会一直挂着 ⇒ **node 进程也退不出去**（实测：断言全绿了，
   * 命令却一直不结束 ✗ —— 与 `leak.mjs` 的收尾同款 ✓）。
   */
  try {
    session?.child.kill('SIGKILL')
  } catch {
    // 已经死了
  }
  try {
    server?.close()
  } catch {
    // 已关
  }
  /**
   * ⚠ 清理**绝不能影响判据**：`SIGKILL` 之后 chrome 还会再写一两个文件，
   * 裸 `rmSync` 会抛 `ENOTEMPTY`（`narrow.mjs` 那次"10 条全过却记成 FAILED"就是这么来的 ✗）。
   */
  try {
    fs.rmSync(PROFILE, { recursive: true, force: true, maxRetries: 8, retryDelay: 60 })
  } catch (e) {
    console.log(`⚠ 临时 profile 没能删掉（不影响判据）：${PROFILE} —— ${String(e.message).split('\n')[0]}`)
  }
}

const bad = cases.filter((c) => !c.ok).length
console.log(bad ? `\n✗ ${bad}/${cases.length} 条大体量约束不满足` : `\n✓ ${cases.length} 条大体量约束全部满足`)
process.exitCode = bad ? 1 : 0
