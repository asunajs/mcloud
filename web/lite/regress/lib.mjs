/**
 * 验收脚本的**共同底座**：fixture 服务、Chrome 路径、DOM 归一化、CDP 会话。
 *
 * * `serve` —— 一份 fixture 后端 + 静态产物，端口随脚本给；
 * * `openSession` —— 起 chrome 并接上 **CDP**（DevTools 协议，零依赖：Node 自带
 *   `WebSocket`），能点、能输入、能等 DOM 稳定，用于回答"点完这一下，界面对不对"。
 *
 * ⚠ `appSubtree` 是"什么算渲染结果"的唯一定义（去注释锚点、折叠标签间空白）：
 * 每步快照都过它，于是"界面变了没有"有一个稳定口径，而不是各处自己剪一段 outerHTML。
 */
import { execFileSync, spawn } from 'node:child_process'
import fs from 'node:fs'
import http from 'node:http'
import path from 'node:path'

export const arg = (name, fallback) => {
  const i = process.argv.indexOf(name)
  return i > 0 ? process.argv[i + 1] : fallback
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/**
 * fixture 数据。⚠ **形状照 `web/src/api.ts` 的类型写**，不是随手编的：
 * 上一版把 `RunRecord` 写成了 `{startedAt, finishedAt}`（真实字段是 `started_at_ms` /
 * `duration_ms`），于是仪表盘上显示 `耗时NaN分NaN秒` —— 界面照样"跑通了"，
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

/**
 * 启动链路要的 4 个接口 + 首屏几个列表；三个 variant 共用同一份。
 *
 * ⚠ **每页要的接口都得在这儿**，少一个的表现不是"那一块空着"而是**整页错误态**：
 * `accounts-page.tsx` 的 `load()` 是 `Promise.all([listAccounts(), getPcDevice()])`，
 * 于是 `/api/login/device` 缺失时账号列表一行都不渲染（先前就是这样 —— 点「移除」
 * 命中的其实是弹窗里那颗按钮，四条断言一起红）。
 * 加页面时先把它 `load()` 里并发取的端点全列进这张表。
 */
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
  /**
   * 本机设备指纹（`getPcDevice`）与换一台（`rotatePcDevice`）。
   * 缺前者 ⇒ 账号页 `Promise.all` 整体失败 ⇒ 列表一行都不渲染（见上面那段 ⚠）。
   */
  '/api/login/device': { device_id: 'dev-abc123' },
  'POST /api/login/device/rotate': { device_id: 'dev-rotated-9f2c' },
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
  login: {
    '/api/session': { status: 401, body: { error: '未登录', code: 'unauthorized' } },
    /**
     * ⚠ key 带方法：`GET /api/session` 是"我登录了吗"（401），`POST /api/session` 是**登录动作**。
     * 同一个 URL 两种语义，只按 URL 匹配的话登录永远失败 —— 那样"登录成功后闸门放行"
     * 这条最要紧的路径就测不到（而它正是当初整包挂掉的地方）。
     */
    'POST /api/session': {
      handler: (body) =>
        body?.password === 'right-pass'
          ? { status: 200, body: { userId: 'u-1', name: 'catlair', kind: 'web' } }
          : { status: 401, body: { error: '用户名或口令不正确', code: 'invalid_credentials' } },
    },
  },
  setup: {
    '/api/setup': { status: 200, body: { initialized: false, minPasswordLen: 8 } },
    '/api/session': { status: 503, body: { error: '实例尚未初始化，请先创建管理员', code: 'setup_required' } },
    'POST /api/setup': {
      handler: () => ({ status: 200, body: { userId: 'u-1', name: 'catlair', kind: 'web' } }),
    },
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
    /**
     * 一条 fixture 可以是 `{status, body}`，也可以是 `{handler(body) → {status, body}}` ——
     * 后者用来表达"同一个端点按请求内容给不同结果"（登录成功/口令错）。
     * 匹配顺序：**方法+路径** 优先于 仅路径。
     */
    /**
     * ⚠⚠ **两种形状**：`VARIANTS`/带方法的覆盖是 `{status, body}`，
     * 而 `FIXTURES` 里的值**直接就是 body**。把后者当 `{status, body}` 用，
     * 会发出"200 + 空体"—— 应用那边 `JSON.parse('')` 拿不到东西、
     * `/api/session` 于是判成未登录，**整个 ready 变体都在渲染登录页**，
     * 而应用停在登录页 ⇒ 每一步都"没报错"，其实是**什么都没测到**。测试脚本自己的 bug 最会骗人。
     */
    const send = (req, res, entry, plain) => {
      const respond = (r) => {
        res.writeHead(plain ? 200 : (r.status ?? 200), { 'content-type': 'application/json' })
        res.end(JSON.stringify(plain ? r : r.body))
      }
      if (!entry.handler) {
        respond(entry)
        return
      }
      let raw = ''
      req.on('data', (d) => {
        raw += d
      })
      req.on('end', () => {
        let parsed = null
        try {
          parsed = raw ? JSON.parse(raw) : null
        } catch {
          // 非 JSON 体：handler 自己处理 null
        }
        respond(entry.handler(parsed))
      })
    }
    const server = http.createServer((req, res) => {
      const url = (req.url ?? '/').split('?')[0]
      const key = `${req.method} ${url}`
      const override = overrides[key] ?? overrides[url]
      if (override) {
        send(req, res, override, false)
        return
      }
      const fixture = FIXTURES[key] ?? FIXTURES[url]
      if (fixture !== undefined) {
        send(req, res, fixture, true)
        return
      }
      if (url.startsWith('/api/')) {
        /**
         * ⚠ 形状与上面 `VARIANTS` 那条同一条规矩：**`error` 是字符串、`code` 在顶层**
         * （`api.ts` 读的是 `String(body.error)`）。写成 `{error:{code,message}}` 的话
         * 界面上会出现 `[object Object]` —— 看着像应用的 bug，其实是 fixture 自己造的。
         */
        res.writeHead(404, { 'content-type': 'application/json' })
        res.end(JSON.stringify({ error: `fixture 未提供：${url}`, code: 'not_found' }))
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
 * 取 `#app` 子树（按 div 标签配平），去注释锚点、折叠标签间空白。
 *
 * ⚠ 注释要**去掉**：lite 用 `<!---->` 当动态槽占位（§11.2 #6），它不属于渲染结果。
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
    .replace(/>\s+</g, '><')
    .trim()
}

/**
 * 起一个可交互的页面（CDP）。零依赖：Node 自带 `WebSocket`。
 *
 * 为什么要它：`--dump-dom` 只能看**首屏**。事件处理器、`batch`、列表增删、
 * 切页时新建/卸载组件这些路径，全在"点一下之后" —— 那正是这套框架最容易出错的地方。
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
