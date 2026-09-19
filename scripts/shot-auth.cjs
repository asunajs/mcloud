/**
 * 截图并**注入会话 cookie**，用于拍需要登录的页面。
 *
 * 与 shot.cjs 的差别只有两点：
 *   1. 先 Network.setCookie 把会话 cookie 塞进去，再导航；
 *   2. 导航后等更久一点，让 hash 路由和首帧渲染完成。
 *
 * Edge headless 的窗口最小宽度约 492px，所以 `--window-size=390` 拍不出真正的
 * 移动端 —— 必须走 CDP 的 Emulation.setDeviceMetricsOverride。
 *
 * 用法：node shot-auth.cjs <baseUrl> <hash> <out.png> <width> <height> <cookie> [--dark]
 */

const http = require('node:http')
const fs = require('node:fs')

const [, , baseUrl, hash, out, wArg, hArg, cookie, ...flags] = process.argv
if (!baseUrl || !out) {
  console.error('usage: node shot-auth.cjs <baseUrl> <hash> <out.png> <w> <h> <cookie> [--dark]')
  process.exit(2)
}
const width = Number(wArg || 1280)
const height = Number(hArg || 900)
const dark = flags.includes('--dark')

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
          reject(new Error(`bad json from ${method} ${path}: ${body.slice(0, 200)}`))
        }
      })
    })
    req.on('error', reject)
    req.setTimeout(2000, () => req.destroy(new Error('timeout')))
    req.end()
  })

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

class Cdp {
  constructor(ws) {
    this.ws = ws
    this.id = 0
    this.pending = new Map()
    this.listeners = new Map()
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data)
      if (msg.id !== undefined) {
        const p = this.pending.get(msg.id)
        if (p) {
          this.pending.delete(msg.id)
          msg.error ? p.reject(new Error(JSON.stringify(msg.error))) : p.resolve(msg.result)
        }
        return
      }
      const ls = this.listeners.get(msg.method)
      if (ls) ls.forEach((fn) => fn(msg.params))
    })
  }
  send(method, params = {}) {
    const id = ++this.id
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      this.ws.send(JSON.stringify({ id, method, params }))
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id)
          reject(new Error(`CDP timeout: ${method}`))
        }
      }, 20000)
    })
  }
  on(method, fn) {
    if (!this.listeners.has(method)) this.listeners.set(method, [])
    this.listeners.get(method).push(fn)
  }
  once(method, timeoutMs = 15000) {
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error(`event timeout: ${method}`)), timeoutMs)
      this.on(method, (p) => {
        clearTimeout(t)
        resolve(p)
      })
    })
  }
}

;(async () => {
  let target
  try {
    target = await request('/json/new?about:blank', 'PUT')
  } catch (e) {
    const list = await request('/json/list')
    target = list.find((t) => t.type === 'page')
    if (!target) throw new Error(`无法新建标签页：${e.message}`)
  }

  const ws = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((res, rej) => {
    ws.addEventListener('open', res)
    ws.addEventListener('error', () => rej(new Error('ws error')))
  })

  const cdp = new Cdp(ws)
  await cdp.send('Page.enable')
  await cdp.send('Runtime.enable')
  await cdp.send('Network.enable')

  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: 1,
    mobile: width < 768,
  })
  if (dark) {
    await cdp.send('Emulation.setEmulatedMedia', {
      features: [{ name: 'prefers-color-scheme', value: 'dark' }],
    })
  }

  // ★ 关键：先注入会话 cookie，否则会被登录门挡回去
  if (cookie && cookie !== '-') {
    const [name, ...rest] = cookie.split('=')
    const value = rest.join('=')
    const parsed = new URL(baseUrl)
    const r = await cdp.send('Network.setCookie', {
      name,
      value,
      domain: parsed.hostname,
      path: '/',
      httpOnly: true,
    })
    console.log(`cookie ${name}=${value.slice(0, 8)}… -> ${JSON.stringify(r)}`)
  }

  const loaded = cdp.once('Page.loadEventFired')
  await cdp.send('Page.navigate', { url: `${baseUrl}${hash || ''}` })
  await loaded
  await sleep(2000)

  const probe = await cdp.send('Runtime.evaluate', {
    expression:
      'JSON.stringify({w:innerWidth,h:innerHeight,hash:location.hash,body:document.body.innerText.slice(0,120)})',
    returnByValue: true,
  })
  console.log(`probe: ${probe.result.value}`)

  const shot = await cdp.send('Page.captureScreenshot', { format: 'png' })
  fs.writeFileSync(out, Buffer.from(shot.data, 'base64'))
  console.log(`wrote ${out} (${fs.statSync(out).size} bytes) @ ${width}x${height}${dark ? ' [dark]' : ''}`)

  ws.close()
  process.exit(0)
})().catch((e) => {
  console.error('FAILED:', e.message)
  process.exit(1)
})
