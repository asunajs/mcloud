/**
 * 回归脚本的**共同底座**：fixture 服务、Chrome 路径、DOM 归一化、两种抓取方式。
 *
 * 两个脚本各走一条抓取路线，都是踩过坑才定下来的：
 *
 * * `dumpDom`（`compare.mjs` 用）—— `--dump-dom` **一次**抓完初始渲染。最省事、最不容易错，
 *   缺点是抓不到"交互之后"的状态；
 * * `openSession`（`interact.mjs` 用）—— 走 **CDP**（DevTools 协议，零依赖：Node 自带
 *   `WebSocket`），能点、能输入、能等 DOM 稳定，用于回答"点完这一下，两侧还一致吗"。
 *
 * ⚠ `appSubtree` 必须两个脚本**同一份**：它定义了"什么算渲染结果"（去注释锚点、
 * 去框架标记、折叠标签间空白）。各写一份迟早会漂移，而漂移的表现是"都过了但测的不是一回事"。
 */
import { execFileSync, spawn } from 'node:child_process'
import fs from 'node:fs'
import http from 'node:http'
import path from 'node:path'
import zlib from 'node:zlib'

export const arg = (name, fallback) => {
  const i = process.argv.indexOf(name)
  return i > 0 ? process.argv[i + 1] : fallback
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/**
 * fixture 数据。⚠ **形状照 `web/src/api.ts` 的类型写**，不是随手编的：
 * 上一版把 `RunRecord` 写成了 `{startedAt, finishedAt}`（真实字段是 `started_at_ms` /
 * `duration_ms`），于是仪表盘上显示 `耗时NaN分NaN秒` —— 两侧当然"一致"，
 * 但那是在**测一个空壳**。列表为空时各页只渲染空态，`createFor` 的增删重排、
 * 带数据的弹窗、带选项的下拉框一个都走不到。
 */
const ACCOUNT = { id: '13800000000', nickname: '主力号', pc_platform: 'windows', device_id: 'dev-abc123', expire: 1790000000000 }
const ACCOUNT2 = { id: '13900000001', nickname: null, pc_platform: null, device_id: 'dev-def456', expire: 0 }
const RUN = {
  run_id: 1024,
  task: 'daily-checkin',
  account: '138****0000',
  started_at_ms: 1790581000000,
  duration_ms: 9200,
  outcome: { status: 'success', summary: '打卡 3 个账号' },
  details: { artifacts: [{ kind: 'cloud-file', account: '138****0000', id: 'file-7f3a' }] },
}
const SCHEDULE = {
  id: 'sch-1',
  name: '每天签到',
  task: 'daily-checkin',
  accounts: [ACCOUNT.id],
  expr: '0 8 * * *',
  timezone: 'Asia/Shanghai',
  body: {},
  enabled: true,
  lastFireMs: 1790581000000,
  lastRunMs: 1790581009200,
  lastStatus: 'ok',
  lastError: null,
  runCount: 12,
  nextFireAt: 1790667400000,
}
const PIPELINE = {
  id: 'pipe-1',
  name: '早班',
  accounts: [ACCOUNT.id, ACCOUNT2.id],
  accountMode: 'sequential',
  stopOnError: false,
  steps: [
    { task: 'daily-checkin', continueOnError: false },
    { task: 'live-room', body: { listenSeconds: 30 }, continueOnError: true },
  ],
  enabled: true,
  lastRunMs: 1790582000000,
}

/** 启动链路要的 4 个接口 + 首屏几个列表；两侧**完全一样**。 */
export const FIXTURES = {
  '/api/setup': { initialized: true, minPasswordLen: 8 },
  '/api/session': { userId: 'u-1', name: 'catlair', kind: 'web' },
  '/api/version': { name: 'mcloud-gost', version: '0.1.0' },
  '/api/status': { name: 'mcloud-gost', version: '0.1.0', taskCount: 18, lastRun: RUN, schedulerRunning: true, runningCount: 0, scheduleCount: 1, pipelineCount: 1 },
  '/api/capabilities': {
    tasks: [
      { name: 'daily-checkin', title: '每日签到', description: '签到并领取当日奖励', hidden: false, params: [] },
      { name: 'live-room', title: '直播口令', description: '听弹幕领小红花', hidden: false, params: ['listenSeconds'] },
      { name: 'internal-probe', title: '内部探针', description: '不该出现在界面上', hidden: true, params: [] },
    ],
  },
  '/api/runs': [RUN, { ...RUN, run_id: 1023, task: 'live-room', duration_ms: 61000, outcome: { status: 'failed', reason: '口令无效' }, details: null }],
  '/api/schedules': [SCHEDULE],
  '/api/pipelines': [PIPELINE],
  '/api/accounts': [ACCOUNT, ACCOUNT2],
}

/**
 * 认证闸的三个状态都要测 —— **首屏到底显示哪个界面，全看它**：
 *
 * | variant | `/api/setup` | `/api/session` | 应用该显示 |
 * |---|---|---|---|
 * | `ready`（默认） | 200 已初始化 | 200 | 完整外壳（drawer/dock/页面） |
 * | `login` | 200 已初始化 | **401** | `AuthPage mode=login` |
 * | `setup` | 200 **未初始化** | **503** | `AuthPage mode=setup`（建管理员向导） |
 *
 * ⚠ 这两条响应的**形状要照后端的真实响应写**（`crates/mcloud-server/src/auth_gate.rs`）：
 * `error` 是**字符串**、`code` 在**顶层**（`api.ts` 读的就是 `body.code`）。
 * 写成 `{error:{code}}` 时 `classifyAuthFailure(503, undefined)` 认不出来 ⇒
 * 落到 `probeSession` 的 catch 兜底 ⇒ 显示的是**登录**页 ——
 * 那样"setup 态通过"其实是登录态通过（两个 variant 字节数一样才发现）。
 */
export const VARIANTS = {
  ready: {},
  login: { '/api/session': { status: 401, body: { error: '未登录', code: 'unauthorized' } } },
  setup: {
    '/api/setup': { status: 200, body: { initialized: false, minPasswordLen: 8 } },
    '/api/session': { status: 503, body: { error: '实例尚未初始化，请先创建管理员', code: 'setup_required' } },
  },
}

export const readVariant = () => {
  const v = arg('--variant', 'ready')
  if (!(v in VARIANTS)) throw new Error(`未知 --variant：${v}（可选 ${Object.keys(VARIANTS).join(' / ')}）`)
  return v
}

/** fixture 服务：已知路径按 variant 覆盖，其余 `/api/*` 回 404，非 API 路径当静态文件发。 */
export const serve = (root, port, variant) =>
  new Promise((resolve) => {
    const overrides = VARIANTS[variant]
    const server = http.createServer((req, res) => {
      const url = (req.url ?? '/').split('?')[0]
      const override = overrides[url]
      if (override) {
        res.writeHead(override.status, { 'content-type': 'application/json' })
        res.end(JSON.stringify(override.body))
        return
      }
      if (FIXTURES[url] !== undefined) {
        res.writeHead(200, { 'content-type': 'application/json' })
        res.end(JSON.stringify(FIXTURES[url]))
        return
      }
      if (url.startsWith('/api/')) {
        res.writeHead(404, { 'content-type': 'application/json' })
        res.end(JSON.stringify({ error: { code: 'not_found', message: 'fixture 未提供: ' + url } }))
        return
      }
      let f = path.join(root, url)
      if (!fs.existsSync(f) || fs.statSync(f).isDirectory()) f = path.join(root, 'index.html')
      const type = f.endsWith('.js') ? 'text/javascript' : f.endsWith('.css') ? 'text/css' : 'text/html'
      res.writeHead(200, { 'content-type': type })
      fs.createReadStream(f).pipe(res)
    })
    server.listen(port, '127.0.0.1', () => resolve(server))
  })

/** 应用认得的 hash 路由：`parseHash` 比的是 `#/settings` 这种**带斜杠**的 href。 */
export const hashRoute = (route) => `#/${String(route).replace(/^#\/?/, '')}`

// 同步取路径：此时还没有任何请求在飞，阻塞几毫秒无妨（异步取会让调用方拿到空串）
export const chromePath = execFileSync('bash', ['-c', 'ls -d ~/.cache/puppeteer/chrome-headless-shell/*/chrome-headless-shell-*/chrome-headless-shell | head -1'], { encoding: 'utf8' }).trim()

/**
 * 取 `#app` 子树（按 div 标签配平），去注释锚点、折叠标签间空白、抹掉框架专有的挂载标记。
 *
 * ⚠ 注释要**去掉**：lite 用 `<!---->` 当动态槽占位（§11.2 #6），Vapor 也会打自己的标记，
 * 它们不属于渲染结果。
 */
export function appSubtree(html) {
  const i = html.indexOf('<div id="app"')
  if (i < 0) return ''
  let depth = 0
  let end = i
  for (const m of html.slice(i).matchAll(/<(\/?)div\b[^>]*>/g)) {
    depth += m[1] ? -1 : 1
    if (depth === 0) {
      end = i + m.index + m[0].length
      break
    }
  }
  return html
    .slice(i, end)
    .replace(/<!--.*?-->/gs, '')
    .replace(/\s+data-v-app(="")?/g, '')
    // Vue 的 scoped 属性（`data-v-1a2b3c`）与 `data-v-app` 都是框架挂的标记，不属于渲染结果
    .replace(/\s+data-v-[0-9a-f]+(="")?/g, '')
    .replace(/>\s+</g, '><')
    .trim()
}

/**
 * 抓 DOM（`--dump-dom` 一次性）。必须：`spawn` + **读 stdout 流** + 等 **`exit`**（而不是等管道关闭）。
 *
 * 三条都踩过：
 * 1. stdout 指向 `fs.openSync` 的 fd ⇒ Node 的句柄带 **CLOEXEC**，chrome 继承不到，
 *    表现是**退出码 0、输出 0 字节**（bash 里重定向到文件却正常，极易误判成"页面没渲染"）；
 * 2. `execFileSync`/`execFile` 的管道 ⇒ chrome 的 renderer/gpu 子进程继承了管道，
 *    主进程退出后也不关，回调永不触发 ⇒ ETIMEDOUT；
 * 3. `execFileSync` 阻塞事件循环，而 fixture 服务就在本进程里 ⇒ 两者互相锁死。
 */
export const dumpDom = ({ port, route, name, budget = '8000', timeout = '90000', profile = '/tmp/lite-chrome-profile' }) =>
  new Promise((resolve) => {
    const child = spawn(
      chromePath,
      [
        `--user-data-dir=${profile}`,
        '--headless', '--disable-gpu', '--hide-scrollbars', '--window-size=1280,900',
        `--virtual-time-budget=${budget}`,
        `--screenshot=/tmp/shots/${name}.png`,
        '--dump-dom',
        `http://127.0.0.1:${port}/${route}`,
      ],
      { stdio: ['ignore', 'pipe', 'ignore'] },
    )
    let out = ''
    child.stdout.on('data', (d) => {
      out += d
    })
    // 护栏：被测应用若有渲染死循环，chrome 永远不退出 —— 到点就杀，并把这件事标进结果
    const timer = setTimeout(() => {
      child.kill('SIGKILL')
      resolve(out + '\n<!-- chrome 超时被杀：应用可能在渲染死循环 -->')
    }, Number(timeout))
    child.on('exit', (code) => {
      clearTimeout(timer)
      resolve(code ? out + `\n<!-- chrome exit ${code} -->` : out)
    })
  })

/**
 * 起一个可交互的页面（CDP）。零依赖：Node 自带 `WebSocket`。
 *
 * 为什么要它：`--dump-dom` 只能看**首屏**。事件处理器、`batch`、列表增删、
 * 切页时新建/卸载组件这些路径，全在"点一下之后" —— 那才是这套框架与 Vue 差别最大的地方。
 */
export const launchChrome = ({ url, debugPort, profile }) =>
  spawn(
    chromePath,
    [
      `--remote-debugging-port=${debugPort}`,
      `--user-data-dir=${profile}`,
      '--headless', '--disable-gpu', '--hide-scrollbars', '--window-size=1280,900',
      '--no-first-run', '--no-default-browser-check',
      url,
    ],
    { stdio: ['ignore', 'ignore', 'ignore'] },
  )

const waitForTarget = async (port, timeout = 20000) => {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()
      const page = list.find((t) => t.type === 'page' && t.webSocketDebuggerUrl)
      if (page) return page
    } catch {
      // 端点还没起来，继续等
    }
    await sleep(120)
  }
  throw new Error(`CDP 端点没起来（port=${port}）—— chrome 可能启动失败了`)
}

/** 极简 CDP 客户端：`send` 一次调用、`eval` 在页面里求值、`on` 收事件。 */
class Cdp {
  constructor(ws) {
    this.ws = ws
    this.seq = 0
    this.waiting = new Map()
    this.handlers = new Map()
    ws.onmessage = (m) => {
      const msg = JSON.parse(m.data)
      if (msg.id) {
        const w = this.waiting.get(msg.id)
        if (!w) return
        this.waiting.delete(msg.id)
        msg.error ? w.rej(new Error(JSON.stringify(msg.error))) : w.res(msg.result)
      } else {
        for (const h of this.handlers.get(msg.method) ?? []) h(msg.params)
      }
    }
  }

  static async connect(wsUrl) {
    const ws = new WebSocket(wsUrl)
    await new Promise((res, rej) => {
      ws.onopen = res
      ws.onerror = () => rej(new Error(`连不上 CDP：${wsUrl}`))
    })
    return new Cdp(ws)
  }

  send(method, params = {}) {
    const id = ++this.seq
    this.ws.send(JSON.stringify({ id, method, params }))
    return new Promise((res, rej) => {
      this.waiting.set(id, { res, rej })
      setTimeout(() => {
        if (this.waiting.delete(id)) rej(new Error(`CDP 调用超时：${method}`))
      }, 20000)
    })
  }

  on(method, cb) {
    const list = this.handlers.get(method) ?? []
    list.push(cb)
    this.handlers.set(method, list)
  }

  async eval(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })
    if (r.exceptionDetails) {
      throw new Error(`页面里求值抛错：${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`)
    }
    return r.result.value
  }

  close() {
    try {
      this.ws.close()
    } catch {
      // 关不掉就算了：进程本来就要被杀
    }
  }
}

/** 打开一个可交互页面：起 chrome → 等 CDP → 连接 → 收异常事件。 */
export async function openSession({ port, route, debugPort, profile, width = 1280 }) {
  const url = `http://127.0.0.1:${port}/${route}`
  const child = launchChrome({ url, debugPort, profile })
  const target = await waitForTarget(debugPort)
  const cdp = await Cdp.connect(target.webSocketDebuggerUrl)
  await cdp.send('Page.enable')
  await cdp.send('Runtime.enable')
  const errors = []
  cdp.on('Runtime.exceptionThrown', (p) => errors.push(p.exceptionDetails?.exception?.description ?? p.exceptionDetails?.text ?? '未知异常'))
  // 页面里的 `console.error` 也要收：应用有把错误吞掉的路径，日志是唯一线索
  cdp.on('Runtime.consoleAPICalled', (p) => {
    if (p.type === 'error') errors.push('console.error: ' + p.args.map((a) => a.value ?? a.description ?? '').join(' '))
  })
  return { child, cdp, errors, width }
}

/**
 * 逐像素比对两张 PNG 截图。**零依赖**：自己解 8 位 PNG（Chrome 截图就是 8 位、非隔行、RGB(A)）。
 *
 * 为什么需要它：DOM 逐字符一致 + CSS 同名同哈希，理论上像素必然一致 ——
 * 但"理论上"不是判据，而且这条能兜住"DOM 一样、渲染不一样"的类型（例如内联样式、
 * `:hover`/`:focus` 状态、动画停在半路）。
 *
 * ⚠⚠ **必须给足 `--virtual-time-budget`**（建议 ≥20 s）：8 s 时按钮的
 * `transition: background-color` 还没走完，两侧截到的是过渡中的不同帧 ——
 * 实测都是"按钮面上差几个色阶"，把预算加到 20 s 后 **0 个不同像素**。
 * 那是截图时机问题，不是渲染差异；别把它当成真差异去追。
 */
export function pngDiff(fileA, fileB) {
  const load = (buf) => {
    let pos = 8
    let w = 0
    let h = 0
    let ch = 3
    const idat = []
    while (pos < buf.length) {
      const len = buf.readUInt32BE(pos)
      const typ = buf.toString('ascii', pos + 4, pos + 8)
      pos += 8
      const body = buf.subarray(pos, pos + len)
      pos += len + 4
      if (typ === 'IHDR') {
        w = body.readUInt32BE(0)
        h = body.readUInt32BE(4)
        const ct = body[9]
        ch = ct === 6 ? 4 : ct === 2 ? 3 : ct === 0 ? 1 : ct === 4 ? 2 : 0
        if (!ch || body[8] !== 8 || body[12] !== 0) throw new Error('只支持 8 位非隔行 PNG')
      } else if (typ === 'IDAT') idat.push(body)
    }
    const raw = zlib.inflateSync(Buffer.concat(idat))
    const stride = w * ch
    const out = Buffer.alloc(h * stride)
    let prev = Buffer.alloc(stride)
    let p = 0
    for (let y = 0; y < h; y++) {
      const f = raw[p++]
      const line = Buffer.from(raw.subarray(p, p + stride))
      p += stride
      for (let i = 0; i < stride; i++) {
        const a = i >= ch ? line[i - ch] : 0
        const b = prev[i]
        const c = i >= ch ? prev[i - ch] : 0
        if (f === 1) line[i] = (line[i] + a) & 255
        else if (f === 2) line[i] = (line[i] + b) & 255
        else if (f === 3) line[i] = (line[i] + ((a + b) >> 1)) & 255
        else if (f === 4) {
          const pa = Math.abs(b - c)
          const pb = Math.abs(a - c)
          const pc = Math.abs(a + b - 2 * c)
          line[i] = (line[i] + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)) & 255
        }
      }
      line.copy(out, y * stride)
      prev = line
    }
    return { w, h, ch, px: out }
  }
  const a = load(fs.readFileSync(fileA))
  const b = load(fs.readFileSync(fileB))
  if (a.w !== b.w || a.h !== b.h || a.ch !== b.ch) return { diff: -1, reason: '尺寸/通道不同（布局差异）' }
  let diff = 0
  let maxDelta = 0
  let box = null
  for (let y = 0; y < a.h; y++) {
    for (let x = 0; x < a.w; x++) {
      const i = (y * a.w + x) * a.ch
      let same = true
      for (let k = 0; k < a.ch; k++) {
        if (a.px[i + k] !== b.px[i + k]) {
          same = false
          maxDelta = Math.max(maxDelta, Math.abs(a.px[i + k] - b.px[i + k]))
        }
      }
      if (same) continue
      diff++
      box = box ? [Math.min(box[0], x), Math.min(box[1], y), Math.max(box[2], x), Math.max(box[3], y)] : [x, y, x, y]
    }
  }
  return { diff, total: a.w * a.h, maxDelta, box, w: a.w, h: a.h }
}
