/**
 * 前端内存泄漏量测（真浏览器 + CDP）。
 *
 * # 为什么要有这个脚本
 *
 * 用户口径（2026-10-02）：「前端 js 内存占用又**从 4M 提升到了 8M**」。
 * 当场的证据是 Safari 计数器（JS 堆 10.6 MB / DOM nodes 8654）。这个脚本把它
 * 变成**可复现的读数**：切页若干轮之后，看还能不能回到基线。
 *
 * # 判据（两条，缺一不可）
 *
 * 1. `Runtime.getHeapUsage`：**必须每步先 `HeapProfiler.collectGarbage`** ——
 *    不 GC 读到的是"还没回收的垃圾"，会虚高好几 M，看着像泄漏其实不是 ✗。
 * 2. `Memory.getDOMCounters`：`nodes` 是**包含脱离文档**的节点的 ⇒ 和
 *    `document.querySelectorAll('*').length` 一比就能判断"是留在文档里"还是
 *    "脱离文档却还被引用"。后者才是 JS 保留泄漏（实测过一次：CDP 数到 3438，
 *    文档里只有 318）。
 *
 * # ⚠ 停留时间必须够
 *
 * 每页停留 `DWELL_MS`（默认 2500）。停太短测的是"请求还在路上"的中间态 ——
 * 那是另一个现象，会把它误判成泄漏 ✗。
 *
 * # 用法
 *
 * ```bash
 * cd web && node lite/regress/leak.mjs --fixture   # 夹具模式：谁都能跑 ✓（推荐）
 * cd web && node lite/regress/leak.mjs             # 真服务模式：要 /tmp/mcloud-cookie.txt ✗
 * ```
 *
 * ⚠ 两种模式的**读数不能直接互比**（接口数据不同 ⇒ 渲染出的行数不同 ✓）；
 *   要比"修之前 / 修之后"，两次都用**同一种**模式 ✓。
 *
 * ⚠ 调试端口用 9480（**不要用门禁的 9491/9496**：撞端口会让门禁连到这个残留浏览器上，
 *   2026-10-02 就因此误红过一次）。
 */
import fs from 'node:fs'
import http from 'node:http'
import os from 'node:os'
import path from 'node:path'
import { gotoJs, openSession, routeUrl, serve, sleep } from './lib.mjs'

const WEB = path.resolve(import.meta.dirname, '..', '..')
const PORT = 48980
const DWELL_MS = 2500
const IDLE_MS = 800
/**
 * `--rounds N` 优先于 `LEAK_ROUNDS`（门禁走 `--rounds 2`：够抓线性泄漏、又别太慢 ✓）。
 */
const roundsArg = process.argv.indexOf('--rounds')
const ROUNDS = Number(roundsArg >= 0 ? process.argv[roundsArg + 1] : (process.env.LEAK_ROUNDS ?? 5))
/** 每次访问允许的净增长（节点数）。低于它就是噪声（字体/滚动条/门禁自身的探针）。 */
const NODE_BUDGET = 40
/**
 * 夹具模式（`node lite/regress/leak.mjs --fixture`，或 `LEAK_FIXTURE=1`）：
 * **不连真服务、不要 cookie** ⇒ 谁都能一键复现 ✓（理由见下面 `try` 里那段）。
 */
const FIXTURE = process.argv.includes('--fixture') || process.env.LEAK_FIXTURE === '1'
/** 静态产物与接口的来源端口（夹具模式下由内核分配 ⇒ 不能是 `const` ✓）。 */
let port = PORT

/**
 * 每页访问 ROUNDS 次，记录 Δ；整轮跑一遍全部路由，记录总 Δ。
 *
 * ⚠ `tasks/config` 是任务页的**子视图**（不是独立 PageId），但它在路由上是一条真地址，
 * 而且它自带一批**模块级**状态（草稿表 / 选中项）与该页最大的那张表单（推送十条渠道），
 * 正是最容易漏的那一类 ⇒ 2026-10-05 一并纳入量测（这一页第一版就踩过一次：
 * 卸载期间写信号，每进出一次就多留一份整页 DOM，见 `pages/config-page.tsx` 文件头）。
 */
const PAGES = ['accounts', 'accounts/push', 'tasks', 'tasks/config', 'exchange', 'live-room', 'schedules', 'pipelines', 'history', 'settings']

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.svg': 'image/svg+xml',
}

/** 量测要的接口：真实服务 + 已登录的 cookie。 */
const COOKIE_FILE = process.env.MCLOUD_COOKIE_FILE ?? '/tmp/mcloud-cookie.txt'

function cookie() {
  const raw = fs.readFileSync(COOKIE_FILE, 'utf8')
  const line = raw.split('\n').find((l) => l.includes('mcloud_session'))
  if (!line) throw new Error(`${COOKIE_FILE} 里没有 mcloud_session —— 先去登录一次，把 cookie 存进去`)
  return line.trim().split('\t').pop()
}

const server = http.createServer((req, res) => {
  if (req.url.startsWith('/api')) {
    const up = http.request(
      { host: '127.0.0.1', port: 3000, path: req.url, method: req.method,
        headers: { ...req.headers, host: '127.0.0.1:3000', cookie: `mcloud_session=${cookie()}` } },
      (r) => { res.writeHead(r.statusCode ?? 502, r.headers); r.pipe(res) },
    )
    up.on('error', () => res.writeHead(502).end('proxy error'))
    req.pipe(up)
    return
  }
  const p = new URL(req.url, 'http://x').pathname
  const f = path.join(WEB, 'dist', p === '/' ? 'index.html' : p)
  if (fs.existsSync(f) && fs.statSync(f).isFile()) {
    res.writeHead(200, { 'content-type': MIME[path.extname(f)] ?? 'application/octet-stream' })
    fs.createReadStream(f).pipe(res)
    return
  }
  res.writeHead(404).end('not found')
})

let fail = 0
let session
/** 夹具模式：**不需要真服务、也不需要 cookie** ✓（见下面那段注释）。 */
let fixture = null
try {
  if (FIXTURE) {
    /**
     * ⚠ 为什么非要这条：原来的写法要 `/tmp/mcloud-cookie.txt` 里的**真会话 cookie** ✗，
     * 于是这组读数**只有本机登录过的人能复现** ✗ —— 而"改完拿什么证明改好了"靠的就是它 ✓。
     * 2026-10-02 就栽过：诊断用的探针脚本跑完删了 ⇒ 泄漏结论**一度无法一键复现** ✗。
     *
     * 夹具走门禁那一套 `serve(dist, 0, 'ready')` ✓（端口由内核分配 ✓、不碰 3000 ✓）。
     * ⚠ 调试端口避开门禁的 9491/9496 ✓（撞端口会让门禁连到这个残留浏览器上 ✗）。
     */
    fixture = await serve(path.join(WEB, 'dist'), 0, 'ready')
    port = fixture.address().port
  } else {
    await new Promise((r) => server.listen(PORT, '127.0.0.1', r))
    port = PORT
  }
  session = await openSession({
    port,
    route: routeUrl('dashboard'),
    debugPort: FIXTURE ? 9482 : 9480,
    profile: fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-leak-')),
    width: 1280,
  })
  const { cdp } = session
  const ev = async (expr) => {
    const r = await cdp.send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text ?? 'evaluate 挂了')
    return r.result?.value
  }
  const sample = async () => {
    await cdp.send('HeapProfiler.collectGarbage')
    await sleep(150)
    await cdp.send('HeapProfiler.collectGarbage')
    await sleep(150)
    const u = await cdp.send('Runtime.getHeapUsage')
    const d = await cdp.send('Memory.getDOMCounters').catch(() => ({}))
    const inDoc = await ev(`document.querySelectorAll('*').length`)
    return { mb: u.usedSize / 1048576, nodes: d.nodes ?? 0, listeners: d.jsEventListeners ?? 0, inDoc }
  }

  /**
   * ⚠⚠ **导航前必须等应用挂载完**。
   *
   * `openSession` 只做到"CDP 连上"，**不等页面 load 完**（见 lib.mjs 里那段）。
   * 而本门禁是**并行**跑的（`gates-web.mjs --push` 里七条要 Chrome 的门禁同时起）⇒ 机器忙时
   * 第一个 `gotoJs()` 会在文档还是 `about:blank` 的时刻执行，`history.pushState`
   * 于是抛 `SecurityError` —— 表现是整条门禁在**预热第一页**就崩，报一句光秃秃的
   * `Uncaught`，看不出是哪一页、也看不出原因 ✗（2026-10-03 实测：单独跑全绿、
   * 并行跑必崩，就是这个）。
   *
   * 判据取 `#app` 有第一个子元素：lite 挂载完才会有内容，比 `readyState` 更贴应用。
   */
  const waitForApp = async (timeout = 20000) => {
    const t0 = Date.now()
    for (;;) {
      const ok = await ev(`document.readyState === 'complete' && !!document.querySelector('#app')?.firstElementChild`)
      if (ok) return
      if (Date.now() - t0 > timeout) throw new Error('等待应用挂载超时（20s）—— 页面没起来')
      await sleep(200)
    }
  }
  await waitForApp()

  /**
   * ⚠ **必须先预热**：每个页面第一次加载会一次性留下东西（懒加载的 chunk、模块状态、
   * 缓存 —— 实测 accounts +509 / exchange +387 个节点），那是**一次性成本**，不是泄漏。
   * 不预热就会把它误报成"每次访问漏 170 个" ✗（实测：5 个来回与 12 个来回的 Δ 完全一样，
   * 正说明它是一次性的）。预热之后每页的 Δ 才是"每访问一次漏多少" ✓。
   */
  for (const p of PAGES) {
    await ev(gotoJs(p))
    await sleep(DWELL_MS)
  }
  await ev(gotoJs('dashboard'))
  await sleep(IDLE_MS)
  console.log(`每页访问 ${ROUNDS} 次（每次停留 ${DWELL_MS}ms，已预热），Δ 为 GC 之后的净增长：\n`)
  console.log('页面          Δ节点   Δ监听   Δ堆(MB)')
  for (const page of PAGES) {
    await ev(gotoJs('dashboard'))
    await sleep(IDLE_MS)
    const a = await sample()
    for (let i = 0; i < ROUNDS; i++) {
      await ev(gotoJs(page))
      await sleep(DWELL_MS)
      await ev(gotoJs('dashboard'))
      await sleep(IDLE_MS)
    }
    await sleep(400)
    const b = await sample()
    const per = (b.nodes - a.nodes) / ROUNDS
    const ok = per <= NODE_BUDGET
    if (!ok) fail++
    const sign = (n) => (n >= 0 ? '+' : '') + n
    console.log(
      `${page.padEnd(11)} ${sign(b.nodes - a.nodes).padStart(6)}  ${sign(b.listeners - a.listeners).padStart(6)}  ${sign(+(b.mb - a.mb).toFixed(2)).padStart(7)}   ${ok ? '✓' : `✗ 每次访问留下 ${per.toFixed(0)} 个节点`}`,
    )
  }

  const before = await sample()
  for (let r = 0; r < ROUNDS; r++) {
    for (const p of PAGES) {
      await ev(gotoJs(p))
      await sleep(DWELL_MS * 0.4)
    }
  }
  await sleep(600)
  const after = await sample()
  console.log(`\n全路由 ${ROUNDS} 轮：节点 ${before.nodes} → ${after.nodes}，监听 ${before.listeners} → ${after.listeners}，堆 ${before.mb.toFixed(2)} → ${after.mb.toFixed(2)} MB`)
  console.log(`（文档里实际只有 ${after.inDoc} 个元素 —— 两者差得越多，说明"脱离文档却还活着"的越多）`)
  if (after.nodes - before.nodes > NODE_BUDGET * ROUNDS * 2) fail++
} finally {
  try { session?.child.kill('SIGKILL') } catch { /* 已死 */ }
  server.close()
  try { fixture?.close() } catch { /* 已关 */ }
}

if (fail) {
  console.log(`\n✗ ${fail} 项超预算（每次访问净增 > ${NODE_BUDGET} 节点就算泄漏）`)
  process.exit(1)
}
console.log('\n✅ 都在预算内')
