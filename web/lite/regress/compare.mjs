/**
 * 迁移回归：**同一套 API fixture 下**，把 lite 包和 Vue 包的同一页面拿来逐节点比对。
 *
 * 体积和编译通过都不能回答"两个包渲染出的 DOM 是不是同一个" —— 这个脚本就是那个判据。
 *
 * 用法（先各自构建）：
 *   npx vite build --config vite.config.lite.ts     # → /tmp/lite-app
 *   npm run build                                    # → web/dist
 *   node lite/regress/compare.mjs --route settings [--budget 8000] [--timeout 90000]
 *
 * ⚠ 抓 DOM 这件事本身有三个坑（都踩过，注释在 `dump()` 里）：CLOEXEC 的文件 fd、
 * 被孙子进程拖住的管道、以及同步 API 阻塞事件循环把本进程的 fixture 服务锁死。
 */
import { execFileSync, spawn } from 'node:child_process'
import fs from 'node:fs'
import http from 'node:http'
import path from 'node:path'

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name)
  return i > 0 ? process.argv[i + 1] : fallback
}
const rawRoute = arg('--route', 'settings')
/**
 * 路由规范化：应用的 `parseHash` 比的是 `#/settings` 这种**带斜杠**的 href
 * （`NAV_ITEMS` 里就是 `#/history`），而命令行里写 `--route settings` 更顺手。
 * ⚠ 少了这一步，`#settings` 匹配不上任何一项 ⇒ 应用回落到 dashboard，
 * 于是"每个页面都通过"其实测的是同一个页面（踩过：8 个路由的字节数一模一样才发现）。
 */
const route = `#/${rawRoute.replace(/^#\/?/, '')}`
const liteDir = arg('--lite', '/tmp/lite-app')
const vueDir = arg('--vue', 'dist')

const run = { id: 1, task: 'daily-checkin', startedAt: '2026-09-29T00:00:00Z', finishedAt: '2026-09-29T00:00:09Z', ok: true, outcome: 'ok', summary: '打卡 3 个账号', step: 'done', artifacts: [] }
/** 启动链路要的 4 个接口 + 首屏几个列表；两侧**完全一样**。 */
const FIXTURES = {
  '/api/setup': { initialized: true, minPasswordLen: 8 },
  '/api/session': { userId: 'u-1', name: 'catlair', kind: 'web' },
  '/api/version': { name: 'mcloud-gost', version: '0.1.0' },
  '/api/status': { name: 'mcloud-gost', version: '0.1.0', taskCount: 18, lastRun: run, schedulerRunning: true, runningCount: 0, scheduleCount: 2, pipelineCount: 1 },
  '/api/capabilities': { tasks: [] },
  '/api/runs': [run],
  '/api/schedules': [],
  '/api/pipelines': [],
  '/api/accounts': [],
}

const serve = (root, port) =>
  new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const url = (req.url ?? '/').split('?')[0]
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

// 同步取路径：此时还没有任何请求在飞，阻塞几毫秒无妨（异步取会让下面拿到空串）
const chromePath = execFileSync('bash', ['-c', 'ls -d ~/.cache/puppeteer/chrome-headless-shell/*/chrome-headless-shell-*/chrome-headless-shell | head -1'], { encoding: 'utf8' }).trim()

/**
 * 抓 DOM。必须：`spawn` + **读 stdout 流** + 等 **`exit`**（而不是等管道关闭）。
 *
 * 三条都踩过：
 * 1. stdout 指向 `fs.openSync` 的 fd ⇒ Node 的句柄带 `CLOEXEC`，chrome 继承不到，
 *    表现是**退出码 0、输出 0 字节**（bash 里重定向到文件却正常，极易误判成"页面没渲染"）；
 * 2. `execFileSync`/`execFile` 的管道 ⇒ chrome 的 renderer/gpu 子进程继承了管道，
 *    主进程退出后也不关，回调永不触发 ⇒ ETIMEDOUT；
 * 3. `execFileSync` 阻塞事件循环，而 fixture 服务就在本进程里 ⇒ 两者互相锁死。
 */
const dump = (port, name) =>
  new Promise((resolve) => {
    const child = spawn(
      chromePath,
      [
        '--user-data-dir=/tmp/lite-chrome-profile',
        '--headless', '--disable-gpu', '--hide-scrollbars', '--window-size=1280,900',
        `--virtual-time-budget=${arg('--budget', '8000')}`,
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
    }, Number(arg('--timeout', '90000')))
    child.on('exit', (code) => {
      clearTimeout(timer)
      resolve(code ? out + `\n<!-- chrome exit ${code} -->` : out)
    })
  })

/** 取 `#app` 子树（按 div 标签配平），去注释锚点、折叠标签间空白、抹掉框架专有的挂载标记。 */
function appSubtree(html) {
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

fs.mkdirSync('/tmp/shots', { recursive: true })
/**
 * `--from-raw`：直接用上一次落盘的原始抓取（`/tmp/lite-raw.*.html`）。
 * 调**比对规则**（归一化、差异报告）时不必再跑一遍 chrome —— 抓一次几秒钟，调试期很划算。
 */
const fromRaw = process.argv.includes('--from-raw')
let raw
if (fromRaw) {
  raw = { lite: fs.readFileSync('/tmp/lite-raw.lite.html', 'utf8'), vue: fs.readFileSync('/tmp/lite-raw.vue.html', 'utf8') }
} else {
  const a = await serve(liteDir, 48151)
  const b = await serve(vueDir, 48152)
  raw = { lite: await dump(48151, 'lite'), vue: await dump(48152, 'vue') }
  // 原始抓取一律落盘：失败时想细看（例如 side 里塞了诊断节点）不必再跑一遍
  for (const [name, html] of Object.entries(raw)) fs.writeFileSync(`/tmp/lite-raw.${name}.html`, html)
  a.close()
  b.close()
}

const lite = appSubtree(raw.lite)
const vue = appSubtree(raw.vue)
console.log(`页面 ${route}（lite ${liteDir} vs Vue ${vueDir}）`)
console.log(`  lite #app ${lite.length} B    Vue #app ${vue.length} B`)
if (lite && lite === vue) {
  console.log('  ✅ 逐字符一致；截图在 /tmp/shots/')
} else if (!lite || !vue) {
  console.log('  ✗ 有一侧没抓到 #app：')
  console.log('     lite 原始抓取: ' + JSON.stringify(raw.lite.slice(0, 200)))
  console.log('     vue  原始抓取: ' + JSON.stringify(raw.vue.slice(0, 200)))
  process.exitCode = 1
} else {
  const i = [...lite].findIndex((c, k) => c !== vue[k])
  console.log(`  ✗ 不一致，首个差异在第 ${i} 个字符`)
  console.log(`     lite: ${JSON.stringify(lite.slice(Math.max(0, i - 50), i + 70))}`)
  console.log(`     vue : ${JSON.stringify(vue.slice(Math.max(0, i - 50), i + 70))}`)
  process.exitCode = 1
}
