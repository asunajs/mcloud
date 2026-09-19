/**
 * 布局体检：在真实设备视口下，量出关键元素的几何与越界情况。
 *
 * 用途是替代"人眼看截图猜"——尤其是窄屏下容易把**截图工具的钳位**误判成 CSS 溢出。
 * 用法：node scripts/inspect-layout.cjs <url> <w> <h> [cookie]
 */

const http = require('node:http')

const [, , url, wArg, hArg, cookie] = process.argv
const width = Number(wArg || 1280)
const height = Number(hArg || 900)
const CDP_PORT = 9333
const CDP_HOST = '127.0.0.1'

const request = (path, method = 'GET') =>
  new Promise((resolve, reject) => {
    const req = http.request({ host: CDP_HOST, port: CDP_PORT, path, method }, (res) => {
      let body = ''
      res.on('data', (c) => (body += c))
      res.on('end', () => {
        try {
          resolve(JSON.parse(body))
        } catch (e) {
          reject(new Error(`bad json: ${body.slice(0, 200)}`))
        }
      })
    })
    req.on('error', reject)
    req.setTimeout(2000, () => req.destroy(new Error('timeout')))
    req.end()
  })

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

const EXPR = `(() => {
  const vw = innerWidth, vh = innerHeight;
  const out = { viewport: [vw, vh], scroll: [document.documentElement.scrollWidth, document.documentElement.scrollHeight], overflowX: [], zeroSize: [], dock: null, sidebarFooter: null };

  // 横向越界（真正该修的：内容比视口宽）
  document.querySelectorAll('body *').forEach(el => {
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.right > vw + 1) {
      out.overflowX.push({ tag: el.tagName.toLowerCase(), cls: (el.getAttribute('class')||'').slice(0,60), right: Math.round(r.right) });
    }
  });

  // 零尺寸但承担交互的元素（通常是漏了类名导致没渲染出来）
  document.querySelectorAll('.dock, .drawer-side, .navbar, main, .card, .stat, .table').forEach(el => {
    const r = el.getBoundingClientRect();
    out.zeroSize.push({ cls: (el.getAttribute('class')||'').slice(0,40), w: Math.round(r.width), h: Math.round(r.height) });
  });

  const d = document.querySelector('.dock');
  if (d) { const r = d.getBoundingClientRect(); out.dock = { top: Math.round(r.top), bottom: Math.round(r.bottom), h: Math.round(r.height), vh, overflowsBottom: r.bottom > vh + 1 }; }

  const sf = document.querySelector('.drawer-side .border-t');
  if (sf) { const r = sf.getBoundingClientRect(); const b = sf.querySelector('.badge'); const br = b && b.getBoundingClientRect(); out.sidebarFooter = { top: Math.round(r.top), bottom: Math.round(r.bottom), h: Math.round(r.height), badge: br ? { left: Math.round(br.left), right: Math.round(br.right), w: Math.round(br.width) } : null }; }

  // 正文是否被 dock 遮挡
  const m = document.querySelector('main');
  if (m) { const r = m.getBoundingClientRect(); out.main = { bottom: Math.round(r.bottom), padBottom: getComputedStyle(m).paddingBottom }; }

  return JSON.stringify(out, null, 1);
})()`

;(async () => {
  let target
  try {
    target = await request('/json/new?about:blank', 'PUT')
  } catch {
    const list = await request('/json/list')
    target = list.find((t) => t.type === 'page')
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((res, rej) => {
    ws.addEventListener('open', res)
    ws.addEventListener('error', () => rej(new Error('ws error')))
  })

  let id = 0
  const pending = new Map()
  const listeners = new Map()
  ws.addEventListener('message', (ev) => {
    const m = JSON.parse(ev.data)
    if (m.id !== undefined) {
      const p = pending.get(m.id)
      if (p) {
        pending.delete(m.id)
        m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result)
      }
      return
    }
    const ls = listeners.get(m.method)
    if (ls) ls.forEach((f) => f(m.params))
  })
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const mid = ++id
      pending.set(mid, { resolve, reject })
      ws.send(JSON.stringify({ id: mid, method, params }))
      setTimeout(() => {
        if (pending.has(mid)) {
          pending.delete(mid)
          reject(new Error('timeout ' + method))
        }
      }, 20000)
    })
  const once = (method) =>
    new Promise((resolve) => {
      if (!listeners.has(method)) listeners.set(method, [])
      listeners.get(method).push(resolve)
    })

  await send('Page.enable')
  await send('Runtime.enable')
  await send('Network.enable')
  await send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: 1,
    mobile: width < 768,
  })

  if (cookie && cookie !== '-') {
    const [name, ...rest] = cookie.split('=')
    const parsed = new URL(url)
    await send('Network.setCookie', {
      name,
      value: rest.join('='),
      domain: parsed.hostname,
      path: '/',
      httpOnly: true,
    })
  }

  const loaded = once('Page.loadEventFired')
  await send('Page.navigate', { url })
  await loaded
  await sleep(1800)

  const r = await send('Runtime.evaluate', { expression: EXPR, returnByValue: true })
  console.log(r.result.value)
  ws.close()
  process.exit(0)
})().catch((e) => {
  console.error('FAILED:', e.message)
  process.exit(1)
})
