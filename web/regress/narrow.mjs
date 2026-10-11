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
const DIST = new URL('../dist/', import.meta.url).pathname
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
    // ⚠ 2026-10-08 起「日志」是「历史」页的第二个 tab（用户口径「合并这两个」✓）——
    // 但它仍是**一条真地址**（`/logs` ✓），而且那一页的控件最宽（账号/类别/级别三排 + 搜索框 ✓）
    // ⇒ 两条都走一遍，别只走 `/history` ✗。
    'logs',
    // ⭐ 推送独立成页（用户口径「并单独给推送一个」✓）：规则表本身、以及规则编辑器里
    // 那些 join 输入框（平台凭据）+ 长 webhook URL —— 320 宽下最容易顶出横向滚动 ✓。
    // ⚠ 2026-10-08 起凭据**内嵌在规则编辑器里**（渠道卡没了 ✗），但"窄屏顶不顶"这件事
    // 一条都没少 ✓。
    'push',
    // ⚠ 同日「设置」页新加了「事件推送」卡（四行 × 规则多选 + 三个节流控件 ✓）——
    // 一行里挤三个数字/下拉最容易在 320 宽顶出去 ⇒ 这一页也必须在清单里 ✓。
    //
    // ⚠⚠ 2026-10-08（同日第三次口径）：「设置」拆成**顶部四节 tab**（服务 / 安全 /
    // 事件推送 / 关于 ✓）⇒ 一次只渲染一节 ✗。所以**光走 `/settings` 只量到服务节**
    // —— 而最容易顶破的恰恰是另外两节：安全节的三个口令框 + 并排的两张卡、
    // 事件推送节那四行（每行一个多选 + 三个控件 ✓）。
    // ⇒ **每一条真地址都走一遍**（`/settings/security`、`/settings/events`、
    // `/settings/about` 都是真地址 ✓，见 `settings-page.tsx` 的文件头）。
    // 只走 `/settings` 等于把这三节的窄屏验收**悄悄丢掉** ✗（本仓最忌讳的那种）。
    'settings',
    'settings/security',
    'settings/events',
    'settings/about',
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
   * ④′ 设置页那四节 tab **本身**：320 / 375 下四格都要在、都要点得中、都不折行。
   *
   * ⚠ 上面 ④ 那条只量"页面有没有横向溢出" —— 它认不出"这一排自己横向滚、
   * 第 4 格被推出屏幕"这种形态（`overflow-x-auto` 时页面确实不溢出 ✓）。
   * 所以这里把标签**逐个量一遍**：四个都在（顺序照用户口径 ✓）、每格宽 ≥44
   * （组件指南 §6.1 的触控目标）、高 ≤48（折行 = 那格窄到装不下）。
   *
   * ⚠ 判据与用户口径**对齐**（「可换行或横向滚动 ✓」）：放得下最好，放不下就必须
   * **这一排自己滚**（`overflow-x-auto` ✓）—— 绝不能把**页面**顶宽 ✗。
   * 实测：四个标签（2~4 字）在 320 下**本来就放得下**（不需要滚 ✓）。
   */
  for (const w of [320, 375]) {
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: w,
      height: 812,
      deviceScaleFactor: 2,
      mobile: true,
    })
    await cdp.eval(gotoJs('settings'))
    await sleep(500)
    const tabs = await probe(`(() => {
      const box = document.querySelector('#app [role="tablist"]')
      const list = [...document.querySelectorAll('#app [role="tablist"] [role="tab"]')]
      return {
        n: list.length,
        labels: list.map((b) => b.textContent.trim()).join(','),
        minW: list.length ? Math.min(...list.map((b) => Math.round(b.getBoundingClientRect().width))) : 0,
        maxH: list.length ? Math.max(...list.map((b) => Math.round(b.getBoundingClientRect().height))) : 0,
        clipped: box ? box.scrollWidth > box.clientWidth + 1 : true,
        scrollable: box ? getComputedStyle(box).overflowX === 'auto' : false,
        sw: document.documentElement.scrollWidth,
        vw: window.innerWidth,
      }
    })()`)
    check(
      `${w}px · 设置页四节 tab 都在、顺序对（服务 / 安全 / 事件推送 / 关于）`,
      tabs.n === 4 && tabs.labels === '服务,安全,事件推送,关于',
      JSON.stringify(tabs),
    )
    check(
      `${w}px · 设置页四节 tab 每格 ≥44px 宽、不折行`,
      tabs.minW >= 44 && tabs.maxH <= 48,
      `最窄 ${tabs.minW} / 最高 ${tabs.maxH}`,
    )
    check(
      `${w}px · 设置页四节 tab 不把页面顶宽（放不下时靠这一排自己滚）`,
      tabs.sw <= tabs.vw + 1 && (tabs.clipped === false || tabs.scrollable === true),
      `scrollWidth ${tabs.sw} / 视口 ${tabs.vw} / 需滚 ${tabs.clipped} / overflow-x ${tabs.scrollable}`,
    )
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
    // 「账号配置」页（`/accounts/settings`，2026-10-05 从配置中心拆出来的）—— 账号级设置 +
    // 功能开关（⚠ 2026-10-08 起**渠道表单整块搬走了** ✗ —— 用户口径「很显然我说的
    // 和云盘账户没有关系」；它先落在设置页，当天又搬进独立的「推送」页 `/push` ✓；
    // 所以这里只量账号侧剩下那两块 ✓）。
    // ⚠ 它现在是**独立一页**（账号页的子视图），
    // 不再是配置中心的页签 ⇒ 这里直接换地址进来（窄屏同样要能"列表 ↔ 详情"下钻）。
    await cdp.eval(gotoJs('accounts/settings'))
    await sleep(1200)
    const push = await probe(`(() => ({
      overflow: document.documentElement.scrollWidth > window.innerWidth + 1,
      sw: document.documentElement.scrollWidth, vw: window.innerWidth,
      rail: !!document.getElementById('acct-rail'), detail: !!document.getElementById('acct-detail'),
      blocks: 0,
    }))()`)
    check(`${w}px · 账号配置列表屏无横向溢出`, push.overflow === false, `scrollWidth ${push.sw} / 视口 ${push.vw}`)
    // ⚠ 别断言"一定先落在列表屏"：`drillOpen` 是**模块级**信号（会话内保留），
    // 320 那一轮点过一项之后，375 这一轮进来就是详情屏 —— 那不是 bug，是"记得你上次看到哪" ✓。
    // 所以这里只要求"列表与详情至少有一个"，再把详情弄出来量。
    check(`${w}px · 账号配置两屏至少有一屏（不是白屏）`, push.rail === true || push.detail === true, JSON.stringify(push))
    await cdp.eval(`document.querySelector('#acct-rail button')?.click()`)
    await sleep(1200)
    const pushDetail = await probe(`(() => ({
      overflow: document.documentElement.scrollWidth > window.innerWidth + 1,
      sw: document.documentElement.scrollWidth, vw: window.innerWidth,
      blocks: ['账号级设置', '功能开关'].filter((t) => (document.getElementById('acct-detail')?.textContent ?? '').includes(t)).length,
      pushHere: [...(document.getElementById('acct-detail')?.querySelectorAll('h4') ?? [])].some((h) => h.textContent.trim() === '推送渠道'),
    }))()`)
    check(`${w}px · 账号配置详情无横向溢出`, pushDetail.overflow === false, `scrollWidth ${pushDetail.sw} / 视口 ${pushDetail.vw}`)
    // ⚠ 反方向也量一下：账号侧**不该**再有推送渠道 —— 只钉"两块在"的话，把渠道在两边
    //   各留一份照样绿 ✗，而"两边各一份"正是这次要拆掉的东西 ✓。
    check(
      `${w}px · 账号配置详情两块都在、且没有推送渠道`,
      pushDetail.blocks === 2 && pushDetail.pushHere === false,
      JSON.stringify(pushDetail),
    )
    // 回「任务配置」页：右栏分「公共配置 / 账号私有配置」两半，账号那一行也不能挤破
    await cdp.eval(gotoJs('tasks/config'))
    await sleep(1200)
    // ⚠ 窄屏换页之后落在**列表**那一屏（`drillOpen=false`）⇒ 不点一项就没有 `#config-detail`，
    // 上面那句 `halves: 0` 就是这么来的（第一次写漏了这一步，红了一条）
    await cdp.eval(`document.querySelector('#config-rail button')?.click()`)
    await sleep(900)
    const taskSide = await probe(`(() => ({
      overflow: document.documentElement.scrollWidth > window.innerWidth + 1,
      sw: document.documentElement.scrollWidth, vw: window.innerWidth,
      pub: (document.getElementById('config-detail')?.textContent ?? '').includes('公共配置'),
      priv: (document.getElementById('config-detail')?.textContent ?? '').includes('账号私有配置'),
    }))()`)
    check(`${w}px · 任务详情（公共配置）无横向溢出`, taskSide.overflow === false, `scrollWidth ${taskSide.sw} / 视口 ${taskSide.vw}`)
    // ⚠ 「账号私有配置」那一半 2026-10-05 按用户口径整块删掉了 ⇒ 这里反过来钉住"它不在"
    check(`${w}px · 任务详情只有公共配置（私有那半已删）`, taskSide.pub === true && taskSide.priv === false, JSON.stringify(taskSide))
  }

  /**
   * ⑤′ 320 宽下**错误提示**的形态（2026-10-08 用户口径「所有的提示都是优先弹窗式」）。
   *
   * 这一条盯两件在窄屏上才会现形的事：
   * ① **不溢出**：错误类 toast 不自动消失 ⇒ 它会一直挂在右上角，长文案若顶出视口，
   *    整页就多一条横向滚动条（daisyUI 的 `.toast` 自带 `max-width: calc(100vw - 2rem)` ✓，
   *    这条是它的门禁）；
   * ② **不压住主操作**：推送页的「保存」就在卡片标题行右端，而 toast 在右上角 ——
   *    量两者的矩形**有没有交叠**。容器是 `pointer-events-none`（`ui/toast-view.tsx`），
   *    但"点得到"不等于"看得见"：真交叠了照样是挡住 ✗。
   *
   * ⚠ 触发方式走**真按钮**（手机上先「新建规则」进详情，再点「保存」撞本地校验），
   * 不直接往 store 里塞状态 —— 那样量到的是"我塞的那份"，不是应用真会给的形态 ✗。
   */
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width: 320,
    height: 812,
    deviceScaleFactor: 2,
    mobile: true,
  })
  await cdp.eval(gotoJs('push'))
  await sleep(800)
  const toastProbe = JSON.parse(
    await cdp.eval(`(async () => {
      // 窄屏先落在规则列表那一屏 ⇒ 先点「新建规则」进详情（名称空、平台没配）
      const mk = [...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '新建规则')
      if (mk) mk.click()
      await new Promise((r) => setTimeout(r, 400))
      const detail = document.getElementById('push-detail')
      const save = detail ? [...detail.querySelectorAll('button')].find((b) => b.textContent.trim() === '保存') : null
      if (save) save.click()
      for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 100))
      const toast = document.querySelector('#app .toast .alert')
      const inline = detail ? detail.querySelector('.alert-error, .text-error') : null
      const tr = toast ? toast.getBoundingClientRect() : null
      const sr = save ? save.getBoundingClientRect() : null
      const overlap =
        tr && sr
          ? Math.max(0, Math.min(tr.right, sr.right) - Math.max(tr.left, sr.left)) *
            Math.max(0, Math.min(tr.bottom, sr.bottom) - Math.max(tr.top, sr.top))
          : -1
      return JSON.stringify({
        vw: window.innerWidth,
        hasToast: !!toast,
        text: toast ? toast.textContent.trim().slice(0, 40) : '',
        toastLeft: tr ? Math.round(tr.left) : -1,
        toastRight: tr ? Math.round(tr.right) : -1,
        hasInline: !!inline,
        saveFound: !!save,
        overlap,
        docOverflow: document.documentElement.scrollWidth > window.innerWidth + 1,
      })
    })()`),
  )
  check(
    '320px · 推送页保存撞校验 ⇒ 错误 toast 出现（文案对得上）',
    toastProbe.hasToast === true && toastProbe.text.includes('请填写规则名称'),
    JSON.stringify(toastProbe),
  )
  check(
    '320px · 错误 toast 不溢出视口',
    toastProbe.toastLeft >= 0 && toastProbe.toastRight <= toastProbe.vw && toastProbe.docOverflow === false,
    `toast ${toastProbe.toastLeft}~${toastProbe.toastRight} / 视口 ${toastProbe.vw} / 页面溢出 ${toastProbe.docOverflow}`,
  )
  check(
    '320px · 错误 toast 不压住「保存」（主操作）',
    toastProbe.saveFound === true && toastProbe.overlap === 0,
    `交叠面积 ${toastProbe.overlap}`,
  )
  check('320px · 就近那一句内联也在', toastProbe.hasInline === true)

  /**
   * ⑤″ 定时弹窗里的**「编排」那一格**（2026-10-11 加）。
   *
   * 用户口径：「在定时界面找不到编排任务，虽然可以通过编排界面定时」⇒ 定时页的
   * 目标里补上了**编排** ✓，选中之后多出「要跑的编排」那一格（与「执行的任务」
   * 同款的下拉 ✓）。
   *
   * ⚠ **为什么必须单开这一段**：这一格长在**弹窗里**，而上面那个全路由循环走的是
   * 关闭态（`<dialog>` 没 `open` 时 `display: none`）—— 弹窗内容**不参与**
   * `documentElement.scrollWidth` ✗ ⇒ 光靠"路由无溢出"**量不到它**
   * （装置测不到 = 没测 ✓）。所以这里真把弹窗开起来、真选一次编排再量。
   *
   * 判据三条，缺一条都可能是"看着没事"：① 页面不横向溢出 ✓；② **弹窗自己的内容区**
   * 不横向溢出（这一格真顶破时先破的是它 ✓）；③ 那一格**在视口内**（只判 ② 的话，
   * 它被推到屏幕外、弹窗自己横向滚照样"不溢出" ✗）。
   */
  for (const w of [320, 375]) {
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: w,
      height: 812,
      deviceScaleFactor: 2,
      mobile: true,
    })
    await cdp.eval(gotoJs('schedules'))
    await sleep(800)
    const schedDlg = JSON.parse(
      await cdp.eval(`(async () => {
        const mk = [...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '新建定时')
        if (mk) mk.click()
        await new Promise((r) => setTimeout(r, 350))
        const d = document.getElementById('schedule-dialog')
        const task = document.querySelector('#sched-task')
        if (task) {
          task.click()
          await new Promise((r) => setTimeout(r, 250))
          const opt = [...document.querySelectorAll('.select-portal .select-option')]
            .find((o) => o.dataset.value === 'pipeline')
          if (opt) opt.click()
          await new Promise((r) => setTimeout(r, 300))
        }
        const box = d ? d.querySelector('.modal-box') : null
        const pipe = document.querySelector('#sched-pipeline')
        const pr = pipe ? pipe.getBoundingClientRect() : null
        // ⚠ 这里**必须重新查一遍** #sched-task：选了编排之后这个下拉的 props 变了
        // ⇒ lite 会把它**重建**成新节点（ui/select.tsx 里那条"组件会因 props 变化
        // 被卸载 + 新建" ✓）—— 拿着点之前那个引用读，读到的是**已经摘掉的旧节点**
        // （实测：界面明明铺出了编排那一格，旧引用却还写着 daily-checkin ✗）。
        const taskNow = document.querySelector('#sched-task')
        return JSON.stringify({
          open: !!d && d.open,
          picked: taskNow ? taskNow.dataset.value : '',
          hasPipe: !!pipe,
          pipeRight: pr ? Math.round(pr.right) : -1,
          boxScrollW: box ? box.scrollWidth : -1,
          boxClientW: box ? box.clientWidth : -1,
          docOverflow: document.documentElement.scrollWidth > window.innerWidth + 1,
          vw: window.innerWidth,
        })
      })()`),
    )
    check(
      `${w}px · 定时弹窗能选到「编排」并铺出「要跑的编排」那一格`,
      schedDlg.open === true && schedDlg.picked === 'pipeline' && schedDlg.hasPipe === true,
      JSON.stringify(schedDlg),
    )
    check(
      `${w}px · 定时弹窗选「编排」后无横向溢出`,
      schedDlg.docOverflow === false && schedDlg.boxScrollW <= schedDlg.boxClientW + 1,
      `页面溢出 ${schedDlg.docOverflow} / 弹窗内容 ${schedDlg.boxScrollW} ≤ ${schedDlg.boxClientW}`,
    )
    check(
      `${w}px · 「要跑的编排」那一格在视口内`,
      schedDlg.pipeRight >= 0 && schedDlg.pipeRight <= schedDlg.vw + 1,
      `右边界 ${schedDlg.pipeRight} / 视口 ${schedDlg.vw}`,
    )
    await cdp.eval(`document.getElementById('schedule-dialog')?.close()`)
    await sleep(250)
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
    // ⚠ 「并排的卡填满所在格」那条判据**不在这里**量 —— 并排那一对在**安全节**
    //（2026-10-08 拆 tab ✓，见下面那段注释）。
    const paths = [...document.querySelectorAll('#app .grid')].find((g) => (g.textContent || '').includes('数据目录'))
    const cols = paths
      ? new Set([...paths.children].map((c) => Math.round(c.getBoundingClientRect().left))).size
      : 0
    return {
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
   *
   * ⚠⚠ 2026-10-08 拆 tab 之后，全页**唯一**并排的一对（「口令策略 | 会话控制」）
   * 在**安全节**（`/settings/security` ✓）—— 服务节里一张卡都不在 grid 格里
   * （服务操作**故意**不跟存储信息并排，见 `settings-page.tsx` 的注释 ✓）。
   * 所以这条必须在**安全节**量：在服务节量的话它读到 0 张网格卡、
   * `unfilled.length === 0` **永远成立** ⇒ 又是一条空转 ✗（本仓在这上面栽过好几次）。
   * ⇒ 判据里带上"确实有**两张**并排的卡、且同一行" ✓（证明这条不是空转）。
   */
  await cdp.eval(gotoJs('settings/security'))
  await sleep(900)
  const wideSec = await probe(`(() => {
    const cells = []
    for (const c of document.querySelectorAll('#app .card')) {
      const cell = c.parentElement
      const box = cell && cell.parentElement
      if (!box || !getComputedStyle(box).display.includes('grid')) continue
      const ch = Math.round(c.getBoundingClientRect().height)
      const gh = Math.round(cell.getBoundingClientRect().height)
      const r = c.getBoundingClientRect()
      cells.push({
        title: (c.querySelector('.card-title')?.textContent ?? '?').trim().slice(0, 10),
        ch, gh,
        top: Math.round(r.top),
        left: Math.round(r.left),
      })
    }
    return {
      titles: cells.map((x) => x.title).join(','),
      // 两张卡**同一行**（top 相同）且左右分开 ⇒ 真的并排（不是上下堆着）
      sideBySide: cells.length === 2 && cells[0].top === cells[1].top && cells[0].left !== cells[1].left,
      unfilled: cells.filter((x) => x.gh - x.ch > 2).map((x) => x.title + ' ' + x.ch + '/' + x.gh),
    }
  })()`)
  check(
    '1440 宽下安全节确实并排着两张卡（下面那条判据不是空转）',
    wideSec.sideBySide === true,
    JSON.stringify(wideSec),
  )
  check(
    '并排的卡填满所在格（不留悬空的半张卡）',
    wideSec.unfilled.length === 0,
    wideSec.unfilled.length ? '没填满的卡：' + JSON.stringify(wideSec.unfilled) : '',
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
