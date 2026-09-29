/**
 * 迁移回归：**同一套 API fixture 下**，把 lite 包和 Vue 包的同一页面拿来逐节点比对。
 *
 * 为什么需要它：光标级/体积级的证据都不能回答"两个包渲染出的 DOM 是不是同一个"。
 * 这个脚本先给两边喂**完全一样**的接口响应，再比 `#app` 子树。
 *
 * 用法（先各自构建）：
 *   npx vite build --config vite.config.lite.ts     # → /tmp/lite-app
 *   npm run build                                    # → web/dist（Vue 包）
 *   node lite/regress/compare.mjs --route '#settings'
 *
 * ⚠ 现状（2026-09-29）：**两边不一致** —— lite 侧卡在 loading 态，
 * 根因是**片段（fragment）里的动态成员只在挂载时求值一次**（详见
 * docs/lite-framework.md §11）。这个脚本就是那次的现场复现工具。
 */
import { execFileSync, spawn } from 'node:child_process'
import fs from 'node:fs'
import http from 'node:http'
import path from 'node:path'

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name)
  return i > 0 ? process.argv[i + 1] : fallback
}
const route = arg('--route', '#settings')
const liteDir = arg('--lite', '/tmp/lite-app')
const vueDir = arg('--vue', 'dist')

// ── fixture：启动链路要的 4 个接口 + 首屏几个列表 ────────────────────────────
const run = { id: 1, task: 'daily-checkin', startedAt: '2026-09-29T00:00:00Z', finishedAt: '2026-09-29T00:00:09Z', ok: true, outcome: 'ok', summary: '打卡 3 个账号', step: 'done', artifacts: [] }
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

const chrome = execFileSync('bash', ['-lc', 'ls -d ~/.cache/puppeteer/chrome-headless-shell/*/chrome-headless-shell-*/chrome-headless-shell | head -1'], { encoding: 'utf8' }).trim()

/**
 * ⚠ `timeout` 护栏不能省：**被测应用若有渲染死循环，Chrome 的虚拟时间永远走不完**，
 * 没有这个上限 `execFileSync` 会一直挂着（实测卡了 7 分钟，只能人工中断）。
 * macOS 没有 `timeout` 命令，所以用 Node 自己的超时。
 */
const dump = (port, name) => {
  try {
    return execFileSync(chrome, ['--headless', '--disable-gpu', '--hide-scrollbars', '--window-size=1280,900', '--virtual-time-budget=8000', `--screenshot=/tmp/shots/${name}.png`, '--dump-dom', `http://127.0.0.1:${port}/#${route.replace(/^#/, '')}`], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], timeout: 90_000 })
  } catch (e) {
    return `<!--超时或被中断：${(e as Error).message}-->`
  }
}

/** 取 `#app` 子树（按标签配平），去注释锚点、折叠标签间空白。 */
function appSubtree(html) {
  const i = html.indexOf('<div id="app"')
  if (i < 0) return ''
  let depth = 0
  let end = i
  for (const m of html.slice(i).matchAll(/<(\/?)div\b[^>]*>/g)) {
    depth += m[1] ? -1 : 1
    if (depth === 0) { end = i + m.index + m[0].length; break }
  }
  return html.slice(i, end).replace(/<!--.*?-->/gs, '').replace(/>\s+</g, '><').trim()
}

fs.mkdirSync('/tmp/shots', { recursive: true })
const a = await serve(liteDir, 48151)
const b = await serve(vueDir, 48152)
const lite = appSubtree(dump(48151, 'lite'))
const vue = appSubtree(dump(48152, 'vue'))
a.close()
b.close()

console.log(`页面 ${route}`)
console.log(`  lite #app ${lite.length} B    Vue #app ${vue.length} B`)
if (lite && lite === vue) {
  console.log('  ✅ 逐字符一致（截图同样在 /tmp/shots/）')
} else if (!lite || !vue) {
  console.log('  ✗ 有一侧没渲染出来（看 /tmp/shots/*.png 与控制台）')
} else {
  const firstDiff = [...lite].findIndex((c, i) => c !== vue[i])
  console.log(`  ✗ 不一致，首个差异在第 ${firstDiff} 个字符`)
  console.log(`     lite: ${JSON.stringify(lite.slice(Math.max(0, firstDiff - 40), firstDiff + 60))}`)
  console.log(`     vue : ${JSON.stringify(vue.slice(Math.max(0, firstDiff - 40), firstDiff + 60))}`)
  process.exitCode = 1
}
