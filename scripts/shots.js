#!/usr/bin/env node
// 真实设备视口下的 WebUI 截图 / 布局体检 —— scripts/shots.sh 的 zx 版
// （2026-09-22 迁移，逐条对照 bash 版，行为与文案保持一致）。
//
// 为什么需要这个脚本（而不是直接 `msedge --headless --screenshot`）：
//   Edge / Chrome 的 headless **窗口**有最小宽度限制（实测约 492px）。
//   传 `--window-size=390,844` 会被静默钳到 492，
//   于是截出来的"移动端"其实是 492px 宽的桌面端 —— 卡片看起来"溢出屏幕"，
//   但那不是 CSS 的错，是截图工具的错。
//   实证：在 `--window-size=390` 下，页面里 `window.innerWidth` 仍是 492。
//
//   正确做法是走 CDP 的 `Emulation.setDeviceMetricsOverride` —— 它改的是
//   **渲染视口**而不是操作系统窗口，能真正拿到 390×844。
//   脚本每次都会回读 `innerWidth/innerHeight` 复核，不让你靠猜。
//
// 用法：
//   node scripts/shots.js                 # 全流程：起服务 → 建管理员 → 截图 → 停服务
//   node scripts/shots.js inspect         # 只做布局体检（不写图）
//
// 产物落在 data/（已在 .gitignore 里，不会入库）。
//
// ⚠ 下面 EDGE / NODE / 服务二进制这几个路径**钉死在那台 Windows 机器**上
//   （与旧 bash 版一字不差）；换机器时第一步探测就会拒跑并说明原因。
//   服务二进制仍是迁移前的 `target/debug/mcloud-server.exe` 落点 ——
//   本次迁移刻意不顺手改它："转语言"与"换被截对象"是两件事，混在一笔里说不清。
import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import process from 'node:process'
import { $, cd, sleep } from 'zx'

cd(new URL('..', import.meta.url).pathname)

const PORT = process.env.MCLOUD_PORT || '8787'
const DB = 'data/preview.db'
const EDGE = '/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'
const NODE = 'C:/Users/Administrator/.workbuddy-ai/binaries/node/versions/22.22.2-2/node.exe'
const PROFILE = 'E:/workspace/mcloud/data/cdp-profile'
const TMP = 'data/tmp'
const MODE = process.argv[2] ?? 'shots'

// 系统装了 HTTP 代理，会把 localhost 的请求也拦下来 —— curl 必须绕开
process.env.NO_PROXY = '127.0.0.1,localhost'
process.env.no_proxy = '127.0.0.1,localhost'

try {
  fs.accessSync(EDGE, fs.constants.X_OK)
} catch {
  console.error(`找不到 Edge：${EDGE}`)
  console.error('（agent-browser 的 Chromium 下载在本机不通，所以用系统自带的 Edge）')
  process.exit(1)
}

fs.mkdirSync('data', { recursive: true })
fs.mkdirSync(TMP, { recursive: true })

// ── 清理上一轮残留 ────────────────────────────────────────────────
// 数据库被上一个服务进程占着时 rm 会失败，所以先按端口找进程杀掉。
// （bash 版只看 stdout 的 netstat 输出；Windows 上 netstat 全走 stdout，一致。）
const net = await $`netstat -ano`.nothrow().quiet()
const line = `${net.stdout}`
  .split('\n')
  .find((l) => l.includes(`:${PORT} `) && l.includes('LISTENING'))
if (line) {
  console.log(`端口 ${PORT} 被占用，先清掉旧进程…`)
  const pid = line.trim().split(/\s+/).pop()
  if (pid) {
    spawnSync('powershell.exe', ['-NoProfile', '-Command', `Stop-Process -Id ${pid} -Force`], {
      stdio: 'ignore',
    })
    await sleep(1000)
  }
}
spawnSync('rm', ['-f', DB, `${DB}-shm`, `${DB}-wal`], { stdio: 'ignore' })
spawnSync('rm', ['-rf', PROFILE], { stdio: 'ignore' })

// ── 起服务 ────────────────────────────────────────────────────────
// 后台进程用 child_process 而不是 zx：zx 的 ProcessPromise 要 await 才收敛，
// 而这里要的就是"起了就走、最后 kill"的 bash `&` 语义。
const srvLog = fs.openSync('data/srv.log', 'w')
const srv = spawn('./target/debug/mcloud-server.exe', ['--db', DB, '--port', PORT], {
  stdio: ['ignore', srvLog, srvLog],
})

let code = ''
for (let i = 0; i < 40; i++) {
  const r =
    await $`curl -s --noproxy '*' -o /dev/null -w %{http_code} http://127.0.0.1:${PORT}/api/setup`
      .nothrow()
      .quiet()
  code = `${r.stdout}${r.stderr}`.trim()
  if (code === '200') break
  await sleep(500)
}
if (code !== '200') {
  console.error('服务没起来，看 data/srv.log：')
  process.stderr.write(fs.readFileSync('data/srv.log'))
  srv.kill()
  process.exit(1)
}
console.log(`✓ 服务已就绪 http://127.0.0.1:${PORT}`)

// ── 建管理员并登录取会话 ──────────────────────────────────────────
await $`curl -s --noproxy '*' -X POST http://127.0.0.1:${PORT}/api/setup -H 'Content-Type: application/json' -d ${'{"name":"admin","password":"preview123"}'} -o /dev/null`
  .nothrow()
  .quiet()
// setup 已存在时返回 400，不影响；这里只关心下面的会话
await $`curl -s --noproxy '*' -D ${TMP}/hdr.txt -X POST http://127.0.0.1:${PORT}/api/session -H 'Content-Type: application/json' -d ${'{"name":"admin","password":"preview123"}'} -o /dev/null`
  .nothrow()
  .quiet()
let CK = ''
try {
  for (const l of fs.readFileSync(`${TMP}/hdr.txt`, 'utf8').split('\n')) {
    const m = l.match(/^\s*[Ss]et-cookie:.*mcloud_session=([^;;\r\n]*)/)
    if (m) {
      CK = m[1]
      break
    }
  }
} catch {
  // hdr.txt 不存在 = 请求根本没发出去，落到下面"拿不到会话 cookie"那条
}
if (CK === '') {
  console.error('拿不到会话 cookie，无法截需要登录的页面')
  srv.kill()
  process.exit(1)
}
console.log(`✓ 会话已建立（token ${CK.length} 字符）`)

// ── 起带调试端口的 Edge ───────────────────────────────────────────
const edgeLog = fs.openSync('data/edge.log', 'w')
const edge = spawn(
  EDGE,
  [
    '--headless=new',
    '--disable-gpu',
    '--no-sandbox',
    '--hide-scrollbars',
    '--remote-debugging-port=9333',
    `--user-data-dir=${PROFILE}`,
    'about:blank',
  ],
  { stdio: ['ignore', edgeLog, edgeLog] },
)

// 对应 bash 的 `trap cleanup EXIT`：正常结束与异常退出都要收掉两个子进程。
const cleanup = () => {
  edge.kill()
  srv.kill()
}
process.on('exit', cleanup)
process.on('SIGINT', () => process.exit(1))
process.on('SIGTERM', () => process.exit(1))

await sleep(3000)

const BASE = `http://127.0.0.1:${PORT}`
const COOKIE = `mcloud_session=${CK}`

if (MODE === 'inspect') {
  console.log('')
  console.log('########## 桌面 1280×900 ##########')
  await $`${NODE} scripts/inspect-layout.cjs ${BASE}/#/ 1280 900 ${COOKIE}`
  console.log('')
  console.log('########## 移动 390×844 ##########')
  await $`${NODE} scripts/inspect-layout.cjs ${BASE}/#/ 390 844 ${COOKIE}`
  process.exit(0) // 'exit' 事件里的 cleanup 照常跑，等价 trap EXIT
}

const shot = async (hash, out, w, h, extra) => {
  console.log(`── ${out} @ ${w}×${h} ${extra ?? ''}`)
  // bash 版没开 set -e：某张图失败只继续往下跑，这里用 nothrow 对齐。
  await $`${NODE} scripts/shot-auth.cjs ${BASE} ${hash} ${out} ${w} ${h} ${COOKIE} ${extra ?? ''}`.nothrow()
}

// 未登录态：故意不带 cookie（旧版这里直接调 .cjs 传 "-"，不走 shot()，保持一致）
await $`${NODE} scripts/shot-auth.cjs ${BASE} "#/" data/shot-1-login-desktop.png 1280 900 -`
await $`${NODE} scripts/shot-auth.cjs ${BASE} "#/" data/shot-2-login-mobile.png 390 844 -`

await shot('#/', 'data/shot-3-dash-desktop.png', 1280, 900)
await shot('#/', 'data/shot-4-dash-mobile.png', 390, 844)
await shot('#/accounts', 'data/shot-5-accounts.png', 1280, 900)
await shot('#/', 'data/shot-6-dash-dark.png', 1280, 900, '--dark')

console.log('')
console.log('── 产物 ──')
await $`ls -la data/shot-*.png`
