/**
 * 设置页「导出配置」那枚按钮 —— **常驻门禁**（2026-10-12 用户口径：
 * 「**导出功能最好也能在 webui 使用**」✓）。
 *
 * # 它钉什么（6 条）
 *
 * ① 服务节「服务操作」卡里那枚按钮在、且**点了真的发出请求**（`GET /api/config/export` ✓）——
 *    "按钮点了有反应"这种断言最会骗人：只看 DOM 变没变的话，一个
 *    `window.open('about:blank')` 也能过 ✗；
 * ② 拿到的响应是 **200 + `application/json`**（不是登录页 / 不是 SPA 回落的
 *    `index.html` —— 那种错法的表现就是"下载了一个 HTML"✗）；
 * ③ 响应体**是一份 JSON**，且顶层带敏感标记 `containsCredentials: true` ✓
 *    （这文件等于登录态，标记没了用户就不知道别乱发 ✗）；
 * ④ 下载文件名**取自服务端** `Content-Disposition`（装置这边发的是
 *    `mcloud-config-test.json` ✓）—— 前端不读那个头的话，这里会读到回落名
 *    `mcloud-config.json` ⇒ 红 ✓；
 * ⑤ 走的是 `<a download>` + 对象 URL（`blob:` ✓），**不是**把当前页面导航走
 *    （后者会把界面顶掉，用户点完就"页面变成一串 JSON"✗）；
 * ⑥ 成功之后界面给了一句人话（`已导出配置：…` ✓）、且**页面 0 条异常** ✓。
 *
 * # 装置写法
 *
 * 照本仓口径：`serve()` + 一个**本脚本私有**的变体（`__configExport` ✓，
 * 不塞进 `lib.mjs` 的公共变体 —— 两条写入线同时改那个文件最容易打架 ✗）。
 *
 * ⚠ 下载那一半**必须**在页面里把 `HTMLAnchorElement.prototype.click` 换掉：
 * 无头 chrome 里点一个 `<a download>` 要么被静默拦掉、要么真写一个文件到默认目录
 *（那会污染跑门禁的那台机器 ✗）。换掉之后我们既能拿到"它想存成什么名字"，
 * 又不产生任何落盘 ✓。
 *
 * ⚠ 发出去的请求用 **CDP 的 `Network` 域**看（不是页面里的 `window.fetch` 桩）：
 * 桩只能证明"我们自己的代码调了自己"，证明不了"真的发了这条 HTTP" ✓。
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { openSession, serve, sleep, VARIANTS } from './lib.mjs'

const DIST = new URL('../dist/', import.meta.url).pathname
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-config-export-'))

/** 装置发的那份导出文档（形状照真后端：十一类 + 敏感标记 ✓）。 */
const EXPORT_DOC = {
  mcloudConfig: 1,
  exportedAt: '2026-10-12 16:30:00',
  containsCredentials: true,
  notice: '这份文件等于你的登录态……',
  owner: { id: 'test-user', name: 'admin' },
  'push-rules': [],
  'push-panel': {},
  'exchange-subscriptions': [],
  'auth-refresh': '03:00',
  pipelines: [],
  schedules: [],
  settings: {},
  accounts: [
    {
      id: '13800000000',
      nickname: '主力号',
      auth: { raw: 'raw-1', platform: 'android', phone: '13800000000', token: 'tok-1', expire: 1790000000000 },
      device_id: 'B-1',
    },
  ],
  'account-state': { '13800000000': { cur_month_backup: '202610' } },
  'account-smid': { '13800000000': { fingerprint_json: '{"screen":"1920x1080"}', device_id: 'B-1' } },
  'pc-device': '62AA2BDA29F99744536E142A83CA5B03-ENDIN',
}

/** 装置给的文件名（前端**必须**读 `Content-Disposition` 才拿得到它 ✓）。 */
const SERVER_FILENAME = 'mcloud-config-test.json'

/** 本脚本私有的变体 ✓（在 `ready` 之上只加这一条）。 */
const variant = {
  ...VARIANTS.ready,
  'GET /api/config/export': {
    status: 200,
    headers: {
      'content-type': 'application/json; charset=utf-8',
      'content-disposition': `attachment; filename="${SERVER_FILENAME}"; filename*=UTF-8''${SERVER_FILENAME}`,
    },
    body: EXPORT_DOC,
  },
}

// ⚠ 注册进 `VARIANTS`（`serve()` 只从那里取 ✓，与 `search.mjs` 同一手法）
VARIANTS.__configExport = variant

const results = []
const check = (name, ok, detail) => {
  results.push({ name, ok: !!ok, detail })
  console.log(`  ${ok ? '通过' : '未通过'}  ${name}${ok || detail === undefined ? '' : ` —— ${detail}`}`)
}

let session
let server
try {
  server = await serve(DIST, 0, '__configExport')
  const port = server.address().port
  session = await openSession({ port, route: '/settings', profile: PROFILE, width: 1280 })
  const { cdp } = session
  const ev = (expr) => cdp.eval(expr)

  /**
   * ⚠ 先开 `Network` 域、再注册监听（顺序反了会漏掉已经飞出去的那条请求 ✓）。
   * ⚠ 判据按 **URL 尾** 认，不认整串（夹具端口是内核给的，写死端口必然烂 ✓）。
   */
  const requests = []
  const responses = []
  cdp.on('Network.requestWillBeSent', (p) => {
    if (p.request.url.endsWith('/api/config/export')) requests.push(p)
  })
  cdp.on('Network.responseReceived', (p) => {
    if (p.response.url.endsWith('/api/config/export')) responses.push(p)
  })
  await cdp.send('Network.enable')

  await sleep(1800)

  // ── 把下载那一下换掉：只记"它想存成什么名字"，不真落盘 ──
  const stubbed = await ev(`(() => {
    window.__dl = null
    HTMLAnchorElement.prototype.click = function () {
      window.__dl = { name: this.getAttribute('download'), href: this.href }
    }
    return true
  })()`)
  check('页面侧已接管 <a download> 的点击（不产生真实下载）', stubbed === true)

  // ── 按钮在不在（服务节「服务操作」卡里，文本**精确**是「导出配置」✓）──
  const btn = await ev(`(() => {
    const b = [...document.querySelectorAll('#app button')].find((x) => x.textContent.trim() === '导出配置')
    if (!b) return { found: false }
    const card = b.closest('.card')
    return {
      found: true,
      cardTitle: (card?.querySelector('.card-title')?.textContent ?? '').trim(),
      disabled: b.disabled,
    }
  })()`)
  check('服务节「服务操作」卡里有「导出配置」按钮', btn.found === true, JSON.stringify(btn))
  check('那枚按钮在「服务操作」卡里、且可点', btn.cardTitle === '服务操作' && btn.disabled === false, JSON.stringify(btn))

  // ── 点它 ──
  const clicked = await ev(`(() => {
    const b = [...document.querySelectorAll('#app button')].find((x) => x.textContent.trim() === '导出配置')
    if (!b) throw new Error('找不到「导出配置」按钮')
    b.click()
    return true
  })()`)
  check('点了「导出配置」', clicked === true)

  // 等请求 + 下载两件事都落地（本地夹具很快，给 6 秒够）
  //
  // ⚠ 成功提示是**3 秒后自动消失**的（`ui/toast.ts::AUTO_DISMISS_MS` ✓）⇒
  // 在**轮询里**就把它记下来，而不是等轮询结束再问（那时可能已经没了 ✗）。
  let dl = null
  let toastSeen = false
  for (let i = 0; i < 60; i++) {
    dl = await ev('window.__dl')
    if (!toastSeen) {
      toastSeen = await ev(`document.body.textContent.includes('已导出配置')`)
    }
    if (dl && requests.length > 0 && responses.length > 0) break
    await sleep(100)
  }

  // ① 请求真的发出去了
  const req = requests[0]
  check('确实发出了 GET /api/config/export', !!req && req.request.method === 'GET', JSON.stringify(requests.map((r) => `${r.request.method} ${r.request.url}`)))
  check('只发了一条（没有重复请求）', requests.length === 1, `实际 ${requests.length} 条`)

  // ② 响应是 200 + JSON（不是登录页 / 不是 SPA 回落的 HTML）
  const resp = responses[0]
  const ctype = resp?.response?.headers?.['content-type'] ?? resp?.response?.headers?.['Content-Type'] ?? ''
  check('响应 200', resp?.response?.status === 200, `实际 ${resp?.response?.status}`)
  check('响应是 application/json（不是 HTML 错误页）', String(ctype).startsWith('application/json'), String(ctype))
  check(
    '响应头里有 Content-Disposition（服务端给的文件名靠它）',
    String(resp?.response?.headers?.['content-disposition'] ?? '').includes(SERVER_FILENAME),
    JSON.stringify(resp?.response?.headers ?? {}),
  )

  // ③ 响应体是一份 JSON + 敏感标记
  let bodyOk = false
  let bodyDetail = '没拿到响应体'
  if (resp) {
    try {
      const raw = await cdp.send('Network.getResponseBody', { requestId: resp.requestId })
      const parsed = JSON.parse(raw.body)
      bodyOk = parsed.containsCredentials === true && Array.isArray(parsed.accounts) && parsed.accounts.length === 1
      bodyDetail = `containsCredentials=${parsed.containsCredentials} accounts=${parsed.accounts?.length}`
    } catch (e) {
      bodyDetail = String(e?.message ?? e)
    }
  }
  check('响应体是 JSON，且带 containsCredentials: true（含凭据标记）', bodyOk, bodyDetail)

  // ④⑤ 走的是 <a download> + blob（页面没被导航走）
  check('下载走 <a download> + 对象 URL（blob:）', !!dl && String(dl.href).startsWith('blob:'), JSON.stringify(dl))
  check(
    `下载文件名取自服务端的 Content-Disposition（${SERVER_FILENAME}）`,
    dl?.name === SERVER_FILENAME,
    JSON.stringify(dl),
  )
  const pathname = await ev('location.pathname')
  check('页面还停在设置页（没被导航成那一串 JSON）', pathname === '/settings', String(pathname))

  // ⑥ 成功之后给了一句人话
  check('界面上给了「已导出配置：…」这句', toastSeen === true)

  check('页面 0 条未捕获异常 / console.error', session.errors.length === 0, session.errors.join(' / '))

  const bad = results.filter((r) => !r.ok).length
  console.log(bad ? `\n未通过 ${bad}/${results.length} 条` : `\n导出配置那条路 ${results.length} 条全过`)
  if (bad) process.exitCode = 1
} finally {
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
  // ⚠ 收尾一律 best-effort：SIGKILL 之后 chrome 还会写一两笔，裸 rmSync 会抛 ENOTEMPTY
  await sleep(300)
  try {
    fs.rmSync(PROFILE, { recursive: true, force: true, maxRetries: 8, retryDelay: 60 })
  } catch {
    // 临时目录，系统自己会清
  }
}
