#!/usr/bin/env node
/**
 * 窄屏（375）布局验收 —— **真按手机宽度开一个视口**，不是把 `#app` 收窄。
 *
 * # 为什么单开这一条
 *
 * 2026-10-01 用户连报两个**只有窄屏才现形**的问题：
 *
 * 1. **账号列表那一行**：四个控件把文字列挤成一列竖排单字（"设备/ENDIN/凭据/27/天后"
 *    一个字一行）。根因是 `min-w-0 flex-1` 的文字列 flex-basis 为 0、可以被压到 40px，
 *    而 `flex-wrap` 因此**永远轮不到换行**。
 * 2. **添加账号弹窗**：daisyUI 给 `.modal-box` 的是 `max-height: 100vh`，而手机浏览器的
 *    `100vh` **包含地址栏那块** ⇒ 底部按钮落在屏幕外；这个弹窗又**没有取消键**，
 *    遮罩只剩顶上一条缝 ⇒ 用户的原话是"都没法点取消"。
 *
 * `interact.mjs` 是固定宽屏跑交互的，抓不到这一类；而这类恰恰是"改的时候容易顺手弄坏、
 * 坏了只有手机用户看得见"。所以这里按 375 宽真开一次，只断言**布局约束**
 * （交互细节仍归 `interact.mjs`，别在这里重复）：
 *
 * 1. 没有横向溢出（文档 `scrollWidth` 不超视口）；
 * 2. 账号行那行「设备 … · 凭据 …」要有像个样的宽度（不是被挤成一条竖线）；
 * 3. 弹窗高度不超**可见**视口，且标题栏的 ✕ 在视口内（任何高度都够得着）；
 * 4. 五个方式 tab 每格宽度 ≥ 44（组件指南 §6.1 的触控目标）。
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { hashRoute, openSession, serve, sleep } from './lib.mjs'

const WIDTH = 375
const DIST = new URL('../../dist/', import.meta.url).pathname
const PORT = 48921
// ⚠ 用 `mkdtempSync` 而不是固定路径：两次跑撞同一个 profile 会互相踩（`interact.mjs` 同款）。
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-narrow-'))

const cases = []
const check = (name, ok, detail = '') => {
  cases.push({ name, ok })
  console.log(`  ${ok ? '✅' : '✗'} ${name}${ok || !detail ? '' : `\n     ${detail}`}`)
}

let server
let session
try {
  server = await serve(DIST, PORT, 'ready')
  session = await openSession({
    port: PORT,
    route: hashRoute('accounts'),
    debugPort: 9491,
    profile: PROFILE,
    width: WIDTH,
  })
  const { cdp } = session
  /**
   * ⚠ `openSession({ width })` **不会**改视口（那是 Chrome 窗口尺寸，实测 `innerWidth`
   * 仍是 1280 —— 这条门禁第一次跑就把自己抓出来了）。真视口要显式下发设备指标，
   * 然后**重载**让布局按新宽度重排。
   */
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width: WIDTH,
    height: 812,
    deviceScaleFactor: 2,
    mobile: true,
  })
  await cdp.send('Page.reload', { ignoreCache: true })
  for (let i = 0; i < 60; i++) {
    if (await cdp.eval(`(document.getElementById('app')?.textContent ?? '').includes('设备 ')`)) break
    await sleep(100)
  }
  await sleep(200)
  const probe = async (expr) => JSON.parse(await cdp.eval(`JSON.stringify(${expr})`))

  // ① 账号页：无横向溢出 + 那一行元信息没被挤扁
  const list = await probe(`(() => {
    const li = document.querySelector('#app ul li.card')
    const meta = li ? [...li.querySelectorAll('span')].find((s) => s.textContent.includes('设备 ')) : null
    return {
      vw: window.innerWidth,
      overflow: document.documentElement.scrollWidth > window.innerWidth + 1,
      metaW: meta ? Math.round(meta.getBoundingClientRect().width) : 0,
      metaH: meta ? Math.round(meta.getBoundingClientRect().height) : 0,
    }
  })()`)
  check(`视口宽度就是手机宽度（${list.vw}px）`, list.vw === WIDTH, `实际 ${list.vw}`)
  check('账号页无横向溢出', list.overflow === false)
  // 被挤扁时宽度掉到几十像素、高度却撑成好几行 —— 两条一起看才判得准
  check(
    '账号行的「设备/凭据」没被挤成竖排',
    list.metaW >= 200 && list.metaH <= 60,
    `实际 ${list.metaW}×${list.metaH}`,
  )

  // ② 弹窗：高度不超可见视口、✕ 够得着、tab 够宽
  await cdp.eval(
    `[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '添加账号')?.click()`,
  )
  await sleep(350)
  const dlg = await probe(`(() => {
    const d = document.getElementById('add-account')
    const box = d.querySelector('.modal-box')
    const close = d.querySelector('button[aria-label="关闭"]')
    const r = close ? close.getBoundingClientRect() : null
    const labels = ['扫码', '短信', '手机端', '账密', '凭据']
    const tabs = [...d.querySelectorAll('.join button')].filter((b) => labels.includes(b.textContent.trim()))
    return {
      open: d.open,
      boxH: box ? Math.round(box.getBoundingClientRect().height) : 0,
      vh: window.innerHeight,
      closeIn: r ? r.top >= 0 && r.bottom <= window.innerHeight && r.width > 0 && r.height > 0 : false,
      tabCount: tabs.length,
      tabMin: tabs.length ? Math.min(...tabs.map((b) => Math.round(b.getBoundingClientRect().width))) : 0,
      // 折行的 tab 会变高（min-h-11 是 44）—— 3 字的「手机端」第一版就折成了两行
      tabMaxH: tabs.length ? Math.max(...tabs.map((b) => Math.round(b.getBoundingClientRect().height))) : 0,
    }
  })()`)
  check('添加弹窗能打开', dlg.open === true)
  check('弹窗高度不超可见视口', dlg.boxH > 0 && dlg.boxH <= dlg.vh, `弹窗 ${dlg.boxH} / 视口 ${dlg.vh}`)
  check('弹窗的 ✕ 在视口内（有出口）', dlg.closeIn === true)
  check(
    '五个方式 tab 都在，且每格 ≥44px',
    dlg.tabCount === 5 && dlg.tabMin >= 44,
    `共 ${dlg.tabCount} 格，最窄 ${dlg.tabMin}px`,
  )
  check('tab 文案不折行（折行=那格窄到装不下）', dlg.tabMaxH <= 48, `最高 ${dlg.tabMaxH}px`)

  /**
   * ⚠ 弹窗**底部**也要够得着：max-height 100vh 那个 bug 的表现就是"底部按钮在屏幕外"。
   * 这里滚到底，确认内容区真的能滚、且滚到底后最后一个控件在视口内。
   */
  await cdp.eval(
    `(() => { const s = document.querySelector('#add-account .overflow-y-auto'); if (s) s.scrollTop = s.scrollHeight })()`,
  )
  await sleep(250)
  const bottom = await probe(`(() => {
    const box = document.querySelector('#add-account .modal-box')
    const last = [...document.querySelectorAll('#add-account button')].pop()
    const r = last ? last.getBoundingClientRect() : null
    return {
      scrollable: box ? box.scrollHeight > box.clientHeight + 1 || document.querySelector('#add-account .overflow-y-auto').scrollHeight > 0 : false,
      lastIn: r ? r.top >= 0 && r.bottom <= window.innerHeight : false,
    }
  })()`)
  check('弹窗内容滚到底后，最后一个控件仍在视口内', bottom.lastIn === true)

  // ③ 总览页顺带看一眼（那一页元素最多，最容易顶出横向滚动）
  await cdp.eval(`document.getElementById('add-account').close()`)
  await cdp.eval(`location.hash = '#/dashboard'`)
  await sleep(700)
  const dash = await probe(
    `({ overflow: document.documentElement.scrollWidth > window.innerWidth + 1 })`,
  )
  check('总览页窄屏无横向溢出', dash.overflow === false)
} catch (e) {
  check('窄屏验收自身跑通', false, String(e && e.stack ? e.stack : e))
} finally {
  try {
    session?.cdp?.close()
  } catch {
    // 已断开
  }
  try {
    session?.child?.kill()
  } catch {
    // 已经死了
  }
  try {
    server?.close()
  } catch {
    // 已经关了
  }
  /**
   * ⚠⚠ 清理**绝不能影响判据**（2026-10-01 实测踩到）：
   * `SIGKILL` 之后 chrome 还会再写一两个文件，裸 `rmSync` 会抛 `ENOTEMPTY` ——
   * 那次 10 条约束**全过**，却因为这一下把整条门禁记成 FAILED（提交门随机变红）。
   * `maxRetries`/`retryDelay` 正是为 ENOTEMPTY/EBUSY/EPERM 这类比赛准备的；
   * 外面再包一层 try/catch：删不掉就打印一句，**退出码仍按断言走**。
   */
  try {
    fs.rmSync(PROFILE, { recursive: true, force: true, maxRetries: 8, retryDelay: 60 })
  } catch (e) {
    console.log(`⚠ 临时 profile 没能删掉（不影响判据）：${PROFILE} —— ${String(e.message).split('\n')[0]}`)
  }
}

const bad = cases.filter((c) => !c.ok).length
console.log(bad ? `\n✗ ${bad}/${cases.length} 条窄屏约束不满足` : `\n✓ ${cases.length} 条窄屏约束全部满足`)
process.exitCode = bad ? 1 : 0
