/**
 * 对**真服务**的验收（不是夹具）—— `/logs` 页那条"轮询不重建行"的钉子。
 *
 * ```bash
 * MCLOUD_ADMIN_PASS=<管理员口令> node lite/regress/live-logs.mjs
 * # 可选：MCLOUD_ADMIN_USER（默认 admin）、MCLOUD_LIVE_PORT（默认 3000）
 * ```
 *
 * # ⚠ 它**不是门禁**（别加进 `gates-web.mjs`）
 *
 * 三条理由：① 它要**真服务**在跑（:3000 那份）；② 它要**口令**（仓库里不许有秘密 ✗）；
 * ③ 它读的是**真日志** —— 条数与内容每次都不同，没法像夹具那样钉死读数 ✓。
 * 没给口令时它**明说"没跑"**再退出 0（本仓口径：跳过 ≠ 通过 ✓）。
 *
 * # 它回答什么
 *
 * `bulk.mjs` 那条钉子用的是夹具（静态 2000 行）；这一条补的是**真机上**同一个问题的
 * 两个可见后果：跨一次真轮询（2 s 一拍）之后，① 同一行的 **DOM 节点**还是原来那个，
 * ② **正在选中的文字没被清掉** —— 后者是用户唯一能直接看见的那条 ✓。
 *
 * ⚠ 全程**只读**：只登录、只进 `/logs`、只读 DOM ✓（不点任何会写上游的按钮 ✓）。
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { openSession, sleep } from './lib.mjs'

const PORT = Number(process.env.MCLOUD_LIVE_PORT ?? 3000)
const USER = process.env.MCLOUD_ADMIN_USER ?? 'admin'
const PASS = process.env.MCLOUD_ADMIN_PASS ?? ''

if (!PASS) {
  console.log(
    '⊘ 跳过（没给 MCLOUD_ADMIN_PASS）—— 这一条要真服务的管理员口令，**跳过 ≠ 通过** ✓\n' +
      '  用法：MCLOUD_ADMIN_PASS=<口令> node lite/regress/live-logs.mjs',
  )
  process.exit(0)
}

const results = []
const check = (name, ok, extra = '') => {
  results.push({ name, ok })
  console.log(`${ok ? '✅' : '✗ '} ${name}${extra ? `\n      ${extra}` : ''}`)
}

const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-live-'))
const session = await openSession({ port: PORT, route: '/', profile: PROFILE, width: 1280 })
const { cdp, errors } = session
const probe = async (expr) => JSON.parse(await cdp.eval(`JSON.stringify(${expr})`))
const waitFor = async (expr, ms = 20000) => {
  const t = Date.now()
  while (Date.now() - t < ms) {
    if (await cdp.eval(expr)) return true
    await sleep(150)
  }
  return false
}

try {
  // ── ① 先证明"闸真的在"，再登录 ──
  // ⚠ 异步那段必须**在页面里**就 await 掉再 stringify：`JSON.stringify(<Promise>)` 只会得到 `{}` ✗
  const login = await cdp
    .eval(`(async () => {
      const noAuth = await fetch('/api/logs?limit=1').then((r) => r.status).catch(() => -1)
      const r = await fetch('/api/session', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ username: ${JSON.stringify(USER)}, password: ${JSON.stringify(PASS)} }),
      })
      let body = ''
      try { body = (await r.text()).slice(0, 160) } catch {}
      return JSON.stringify({ noAuth, status: r.status, body })
    })()`)
    .then(JSON.parse)
  check(`未登录时 /api/logs 是 401（闸在 ✓，实得 ${login.noAuth}）`, login.noAuth === 401)
  check(`管理员登录 POST /api/session ⇒ 2xx（实得 ${login.status}）`, login.status >= 200 && login.status < 300, login.body)

  // ── ② 进日志页（真后端 / 真日志）──
  await cdp.eval(`location.assign('/logs')`)
  await sleep(1500)
  const ready = await waitFor(`document.querySelectorAll('#logs-scroll [data-k]').length > 0`)
  const a = await probe(`(() => {
    const s = document.getElementById('logs-scroll')
    const rows = [...document.querySelectorAll('#logs-scroll [data-k]')]
    return {
      path: location.pathname,
      rendered: rows.length,
      allHaveK: rows.length > 0 && rows.every((r) => (r.dataset.k || '').length > 0),
      dataI: document.querySelectorAll('[data-i]').length,
      scrollH: s ? s.scrollHeight : 0,
      firstText: rows[0] ? rows[0].textContent.slice(0, 70) : '',
    }
  })()`)
  check('日志页铺出来了（真日志）', ready && a.rendered > 0, JSON.stringify(a))
  check('行带 `data-k`、整页 0 个 `data-i`（新契约 ✓）', a.allHaveK && a.dataI === 0, `rendered=${a.rendered} dataI=${a.dataI}`)

  // ── ③ 跨一次**真轮询**：节点复用 + 选中不丢 ──
  const reqs = async () =>
    Number(await cdp.eval(`performance.getEntriesByType('resource').filter((e) => e.name.includes('/api/logs')).length`))
  const n0 = await reqs()
  const tagged = await probe(`(() => {
    const rows = [...document.querySelectorAll('#logs-scroll [data-k]')]
    if (rows.length < 3) return { ok: false, rows: rows.length }
    const el = rows[Math.floor(rows.length / 2)]
    el.__keep = 1
    window.__keepKey = el.dataset.k
    const r = document.createRange()
    r.selectNodeContents(el)
    const s = getSelection()
    s.removeAllRanges()
    s.addRange(r)
    return { ok: true, key: (el.dataset.k || '').slice(0, 50), selLen: String(s).length }
  })()`)
  await sleep(2600)
  const after = await probe(`(() => {
    const el = [...document.querySelectorAll('#logs-scroll [data-k]')].find((d) => d.dataset.k === window.__keepKey)
    const s = getSelection()
    return { found: !!el, kept: el ? el.__keep === 1 : false, connected: el ? el.isConnected : false, selLen: String(s).length }
  })()`)
  const n1 = await reqs()
  check(`这 2.6 s 里真发生过轮询（/api/logs ${n0} → ${n1}）`, n1 > n0, `${n0} → ${n1}`)
  check('跨一次真轮询：同一行的**节点被复用**（打的标记还在）', tagged.ok && after.found && after.kept, JSON.stringify({ tagged, after }))
  check(
    '跨一次真轮询：**选中的文字没被清掉**（用户能看见的那条）',
    tagged.ok && after.selLen > 0 && after.selLen === tagged.selLen,
    `选中 ${tagged.selLen} 字 → 轮询后 ${after.selLen} 字`,
  )
  check('页面 0 条未捕获异常 / console.error', errors.length === 0, errors.slice(0, 3).join(' / '))
} finally {
  const bad = results.filter((r) => !r.ok)
  console.log(`\n${results.length - bad.length}/${results.length} 条通过`)
  try {
    session.child.kill('SIGKILL')
  } catch {}
  try {
    session.cdp.close()
  } catch {}
  /**
   * ⚠ 收尾**一律 best-effort**：Chrome 被杀之后还会往 profile 里写最后几笔，
   * 这时 `rmSync` 会抛 `ENOTEMPTY` —— 而那个异常会**盖掉**上面刚设的退出码
   * （实测：8/8 全绿却吐一段栈 ✗）。删不掉就留着（临时目录，系统自己会清 ✓）。
   */
  await sleep(300)
  try {
    fs.rmSync(PROFILE, { recursive: true, force: true, maxRetries: 3 })
  } catch {}
  if (bad.length) process.exitCode = 1
}
