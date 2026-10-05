#!/usr/bin/env node
/**
 * 布局验收（320 / 375 / 1440）—— **真按那个宽度开一个视口**，不是把 `#app` 收窄。
 *
 * 覆盖三件事：窄屏无横向溢出（全 9 条路由 × 320/375）、账号页那类"元素被挤扁"、
 * 弹窗够得着；外加桌面一条：**表单页内容要封顶**（别在 1512 宽的屏上拉满）。
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
import { gotoJs, openSession, routeUrl, serve, sleep } from './lib.mjs'

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
    route: routeUrl('accounts'),
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
  await cdp.eval(gotoJs('dashboard'))
  await sleep(700)
  const dash = await probe(
    `({ overflow: document.documentElement.scrollWidth > window.innerWidth + 1 })`,
  )
  check('总览页窄屏无横向溢出', dash.overflow === false)
  /**
   * ④ 全路由 × 两个窄宽度：**都要没有横向溢出**。
   *
   * ⚠ 2026-10-01 补这一段的理由：这条门禁原来只测账号页与总览页、且只测 375，
   * 于是**设置页在 320 宽下溢出**它一声没吭（grid 子项默认 `min-width: auto`
   * 不肯收缩，`p.label` 又是 flex/nowrap）。用户手机上看到的就是那一条。
   * 只测"刚好够用"的那一档宽度，等于只测了自己想测的那一档。
   */
  const ROUTES = [
    'dashboard',
    'accounts',
    'tasks',
    // 配置中心（任务页的子视图）：左栏是列表、右栏是表单 —— 窄屏靠"列表 ↔ 详情"下钻，
    // 两个形态都在这条门禁的射程里（下钻那一步在 ④ 之后单独量）。
    'tasks/config',
    'exchange',
    'live-room',
    'schedules',
    'pipelines',
    'history',
    'settings',
  ]
  for (const w of [320, 375]) {
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: w,
      height: 812,
      deviceScaleFactor: 2,
      mobile: true,
    })
    for (const r of ROUTES) {
      await cdp.eval(gotoJs(r))
      await sleep(420)
      const o = await probe(
        `({ sw: document.documentElement.scrollWidth, vw: window.innerWidth })`,
      )
      check(`${w}px · ${r} 无横向溢出`, o.sw <= o.vw + 1, `scrollWidth ${o.sw} / 视口 ${o.vw}`)
    }
  }

  /**
   * ⑤ 配置中心的**详情那一屏**（窄屏下钻之后）。
   *
   * ⚠ 上面那个全路由循环只看到 `#config-rail`（列表那一屏）—— 详情是 JS 状态换出来的，
   * 不点进不去。而详情里恰恰是本页最宽的东西（两列表单、渠道的 join 输入框、长 URL），
   * 所以"只测列表"等于没测。两件事一起量：**没有横向溢出** + **下钻是能回头的**
   * （详情铺开之后必须有一条返回键，否则手机上退不回列表 ✗）。
   */
  for (const w of [320, 375]) {
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: w,
      height: 812,
      deviceScaleFactor: 2,
      mobile: true,
    })
    await cdp.eval(gotoJs('tasks/config'))
    await sleep(500)
    // 列表 → 详情（点左栏第一项）
    await cdp.eval(`document.querySelector('#config-rail button')?.click()`)
    await sleep(500)
    const detail = await probe(`(() => {
      const d = document.getElementById('config-detail')
      const back = [...document.querySelectorAll('#app button')].find((b) => b.textContent.includes('全部'))
      return {
        opened: !!d,
        overflow: document.documentElement.scrollWidth > window.innerWidth + 1,
        backIn: back ? back.getBoundingClientRect().top >= 0 : false,
      }
    })()`)
    check(`${w}px · 配置中心详情无横向溢出`, detail.opened === true && detail.overflow === false, JSON.stringify(detail))
    check(`${w}px · 配置中心详情有返回键且够得着`, detail.backIn === true)
    // 返回列表（钉住"来回都能走"）
    await cdp.eval(`[...document.querySelectorAll('#app button')].find((b) => b.textContent.includes('全部'))?.click()`)
    await sleep(400)
    const backList = await probe(`({ rail: !!document.getElementById('config-rail'), detail: !!document.getElementById('config-detail') })`)
    check(`${w}px · 配置中心能退回列表`, backList.rail === true && backList.detail === false, JSON.stringify(backList))
    // 「账号私有配置」那一栏的详情 —— 渠道表单 + 任务行为 + 功能开关，本页最宽的一屏
    await cdp.eval(`[...document.querySelectorAll('#app [role="tab"]')].find((b) => b.textContent.includes('账号私有配置'))?.click()`)
    await sleep(400)
    await cdp.eval(`document.querySelector('#config-rail button')?.click()`)
    await sleep(900)
    const push = await probe(`(() => ({
      overflow: document.documentElement.scrollWidth > window.innerWidth + 1,
      sw: document.documentElement.scrollWidth, vw: window.innerWidth,
      blocks: ['运行结果推送', '任务行为', '功能开关'].filter((t) => (document.getElementById('config-detail')?.textContent ?? '').includes(t)).length,
    }))()`)
    check(`${w}px · 配置中心私有详情无横向溢出`, push.overflow === false, `scrollWidth ${push.sw} / 视口 ${push.vw}`)
    check(`${w}px · 私有详情三块都在（作用域口径）`, push.blocks === 3, JSON.stringify(push))
  }

  /**
   * ⑥ 桌面：设置页要**有结构地用宽度**。
   *
   * 用户原话："pc端这样太宽了视觉体验差"，并贴了另一套系统的同页做参照 ——
   * 两张卡并排、路径三列、读数成格。所以我**没有**把它缩成窄栏（那是修错方向），
   * 而是：`max-w-7xl` 封顶 + 卡内分栏。这条门禁钉住的就是那个"别拉满整屏"的下限：
   * 既不能无限宽，也不能退化回单列一条线。
   */
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width: 1440,
    height: 900,
    deviceScaleFactor: 1,
    mobile: false,
  })
  await cdp.eval(gotoJs('settings'))
  await sleep(900)
  const wide = await probe(`(() => {
    const cards = [...document.querySelectorAll('#app .card')]
    const ws = cards.map((c) => Math.round(c.getBoundingClientRect().width))
    const first = cards[0] ? cards[0].getBoundingClientRect() : null
    // 数据目录那一行所属的栅格实际分了几列（看子项的左边界有几种）
    // 卡 vs 它所在格：只对"父元素本身处在 grid 里"的卡判（页面上其它卡不在格子里）
    const unfilled = []
    for (const c of document.querySelectorAll('#app .card')) {
      const cell = c.parentElement
      const box = cell && cell.parentElement
      if (!box || !getComputedStyle(box).display.includes('grid')) continue
      const ch = Math.round(c.getBoundingClientRect().height)
      const gh = Math.round(cell.getBoundingClientRect().height)
      const title = c.querySelector('.card-title')
      if (gh - ch > 2) unfilled.push((title ? title.textContent.trim().slice(0, 10) : '?') + ' ' + ch + '/' + gh)
    }
    const paths = [...document.querySelectorAll('#app .grid')].find((g) => (g.textContent || '').includes('数据目录'))
    const cols = paths
      ? new Set([...paths.children].map((c) => Math.round(c.getBoundingClientRect().left))).size
      : 0
    return {
      unfilled,
      pathCols: cols,
      maxW: ws.length ? Math.max(...ws) : 0,
      left: first ? Math.round(first.left) : 0,
      right: first ? Math.round(first.right) : 0,
      vw: window.innerWidth,
    }
  })()`)
  check('1440 宽下设置页内容封顶（≤1280）', wide.maxW > 0 && wide.maxW <= 1280, `最宽卡片 ${wide.maxW}`)
  // 不断言"相对视口居中"：左侧有常驻侧栏，居中与否是设计取舍、不是约束。
  // 要钉的是"别贴着右边缘"—— 那才是"太宽了视觉体验差"的形态。
  check(
    '设置页内容右侧留了白（没顶到边）',
    wide.vw - wide.right >= 8,
    '左 ' + wide.left + ' / 右留白 ' + (wide.vw - wide.right),
  )
  // 桌面上必须真的**分了栏**：存储信息与日志表那种"关键信息各占一端"的长线正是被抱怨的形态
  check(
    '桌面存储信息的路径是分栏的（不是一条长线）',
    wide.pathCols >= 2,
    `路径列数 ${wide.pathCols}`,
  )
  /**
   * 并排的卡**必须填满它所在的那一格** —— 用户口径："不要这种不平的布局"。
   *
   * ⚠ 第一版我写的是"同一行内底边极差 ≤ 2px"，**它永远不会红**（实测：把 `h-full`
   * 摘掉照样通过）—— 因为并排两卡的内容高度本就接近。真正造成"不平"的是另一种：
   * **矮内容塞进高格子**（当时「服务操作」只有一段话，却和 7 个文件的「存储信息」并排），
   * 卡自己缩着、格子空一大片。所以判据要比的是**卡高 vs 格子高**。
   */
  check(
    '并排的卡填满所在格（不留悬空的半张卡）',
    wide.unfilled.length === 0,
    wide.unfilled.length ? '没填满的卡：' + JSON.stringify(wide.unfilled) : '',
  )

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
