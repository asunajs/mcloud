/**
 * 配置中心「搜索框」那条支路 —— **常驻门禁**（第 103 行 ④ / 第 99 行 ③ 欠下的那一条）。
 *
 * # 为什么它必须单开一条装置
 *
 * 搜索框是「**超过 6 项才出现**」的（`ui/search-box` 那条口径 ✓），而常驻夹具里
 * 只有 **1 个可配置任务 / 2 个账号** ⇒ 那条分支在 `gates-web` 的射程之外 ✗
 * （2026-10-05 是用一份**一次性**装置 `target/oneoff/search-check.mjs` 验的 ⇒
 * 一次性装置会烂、会被清、别人跑不到 ✓ 这一条就是把它搬成常驻的 ✓）。
 *
 * 这里自起一个「大清单」变体：**8 个可配置任务 / 7 个账号** ✓。
 *
 * # 它钉什么（8 条）
 *
 * ① 项数 > 6 ⇒ 搜索框出现；② 左栏列出全部 8 项；③④⑤ **打字时 `input` 还是最初那个
 * 节点、焦点还在、字没被回写** —— 这三条是 lite 里"槽重建 ⇒ input 被换"那类
 * **静默坑**的显形处（用户在真机上只会看到"打着字光标跳走" ✗，而门禁若只比
 * 文本快照是看不出来的 ✗）；⑥ 列表被筛掉（只剩命中的那项 + **当前选中**那项，
 * 后者是 `config-page.tsx:184` 有意保留的 ✓）；⑦ 清空后 8 项都回来；
 * ⑧ 375 窄屏下带搜索框**无横向溢出** ✓；⑨⑩⑪⑫ **账号侧那一栏**（`/accounts/push` 的
 * `#acct-rail`，7 个账号）同一组判据（搜索框出现 / 列全 7 项 / input 同一节点 /
 * 焦点与字 / 筛到只剩「账号3」✓）；⑬ 页面 0 异常 ✓。
 *
 * ⚠ **账号侧那一栏的 375 窄屏没钉**（只有任务侧钉了 ✓）：那一页正由另一条写入线
 * 拆着，等它落地再补一条即可 ✓。
 *
 * ⚠ 装置写法照本仓口径：`serve()` + `VARIANTS` 里一个**本脚本私有**的变体
 * （`__search` ✓，不塞进 `lib.mjs` 的公共变体 —— 两条写入线同时改那个文件最容易打架 ✗）。
 * 动态路径（`/api/tasks/<name>/config`）在本仓是**精确匹配**的（`lib.mjs:727` ✓）
 * ⇒ 这里**按清单循环生成**每一条具体路径 ✓（比给夹具加通配规则安全 ✓）。
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { openSession, serve, sleep, VARIANTS } from './lib.mjs'

const DIST = new URL('../../dist/', import.meta.url).pathname
const PROFILE = fs.mkdtempSync(path.join(os.tmpdir(), 'mcloud-search-'))

/** 8 个可配置任务（> 6 ⇒ 搜索框出现 ✓）。 */
const TASKS = Array.from({ length: 8 }, (_, i) => ({
  name: `task-${i + 1}`,
  title: `任务 ${i + 1}`,
  description: `第 ${i + 1} 个可配置任务`,
  hidden: false,
  group: i < 4 ? 'signin' : 'device',
  groupLabel: i < 4 ? 'AI豆中心' : '消息与设备',
  params: [
    { name: 'n', title: '次数', kind: 'number', default: i, min: 0, max: 99, unit: '次', help: '测试用' },
  ],
}))

/** 7 个账号（推送栏那一侧 ✓）。 */
const ACCOUNTS = Array.from({ length: 7 }, (_, i) => ({
  id: `1380000000${i}`,
  nickname: i === 6 ? null : `账号${i + 1}`,
  label: `13*****${i}`,
  pc_platform: null,
  device_id: `dev-${i}`,
  expire: 0,
  enabled: true,
}))

/** 本脚本私有的「大清单」变体 ✓。 */
const variant = {
  ...VARIANTS.ready,
  'GET /api/capabilities': { status: 200, body: { tasks: TASKS } },
  'GET /api/accounts': { status: 200, body: ACCOUNTS },
}
for (const t of TASKS) {
  variant[`GET /api/tasks/${t.name}/config`] = {
    status: 200,
    body: { name: t.name, specs: t.params, params: { n: t.params[0].default } },
  }
}
for (const a of ACCOUNTS) {
  variant[`GET /api/accounts/${a.id}/settings`] = {
    status: 200,
    body: { backupWaitSecs: 0, refreshTokenDays: 0, skipTasks: [], aiAvatarDailyLimit: 0, notify: null },
  }
}
VARIANTS.__search = variant

const results = []
const check = (name, ok, detail = '') => {
  results.push({ name, ok })
  console.log(`  ${ok ? '✅' : '✗ '} ${name}${ok ? '' : `   ${detail}`}`)
}

let server
let session
try {
  server = await serve(DIST, 0, '__search')
  const port = server.address().port
  // ⚠ 配置中心是**任务页里的一个视图**（`/tasks/config` ✓）：`#config-rail` 在
  // `pages/config-page.tsx`，由 `tasks-page.tsx` 挂载 ✓。`/settings` 是**另一页**
  // （`pages/settings-page.tsx`）⇒ 走错页就什么都找不到 ✗（我第一版就踩了这个 ✗）。
  session = await openSession({ port, route: '/tasks/config', profile: PROFILE, width: 1280 })
  const { cdp } = session
  const ev = (expr) => cdp.eval(expr)
  await sleep(1800)

  // ── ①② 搜索框出现 + 8 项都在 ──
  const rail = () =>
    ev(`(() => {
      const r = document.getElementById('config-rail')
      const inp = r?.querySelector('input[type="search"]')
      return {
        items: r ? r.querySelectorAll('button').length : 0,
        hasSearch: !!inp,
        texts: r ? [...r.querySelectorAll('button')].map((b) => b.textContent.replace(/\\s+/g, ' ').trim()).join(' | ') : '',
      }
    })()`)
  const a = await rail()
  check('大清单（8 个任务）下搜索框出现', a.hasSearch === true, JSON.stringify(a))
  check('左栏列出全部 8 项', a.items === 8, `实际 ${a.items}`)

  // ── ③④⑤⑥ 打字：节点/焦点/字都不能变，列表要真筛 ──
  const typed = await ev(`(async () => {
    const inp = document.querySelector('#config-rail input[type="search"]')
    inp.focus()
    const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set
    for (const ch of '任务 7') {
      set.call(inp, inp.value + ch)
      inp.dispatchEvent(new Event('input', { bubbles: true }))
      await new Promise((r) => setTimeout(r, 60))
    }
    await new Promise((r) => setTimeout(r, 260))
    const now = document.querySelector('#config-rail input[type="search"]')
    return {
      sameNode: now === inp,
      focused: document.activeElement === now,
      value: now.value,
      items: document.querySelectorAll('#config-rail button').length,
      texts: [...document.querySelectorAll('#config-rail button')].map((b) => b.textContent.replace(/\\s+/g, ' ').trim()).join(' | '),
    }
  })()`)
  check('打字后 input 还是同一个节点（没被重建）', typed.sameNode === true, JSON.stringify(typed))
  check('打字后焦点还在输入框里（光标不丢）', typed.focused === true, JSON.stringify(typed))
  check('打字内容没被回写（「任务 7」完整）', typed.value === '任务 7', `实际 ${JSON.stringify(typed.value)}`)
  /**
   * ⚠ 期望是 **2 项**而不是 1 项 —— 这是**正确行为**，不是漏筛：
   * `config-page.tsx:184` 那条 `t.name === pickedTask.value ||` 让**当前选中的那一项
   * 永远留着**（否则右边详情会"悬空"✗）。初始选中的是 `task-1` ⇒ 结果是
   * 「任务 1（选中，保留）+ 任务 7（命中）」✓。
   * ⚠ 2026-10-05 那份一次性脚本当时断言的是 1 项 —— 它那时是绿的，说明那条
   * 「保留选中项」是**后来才加的** ✓（一次性装置烂掉就是这个样子 ✗）。
   */
  check(
    '列表被筛掉（只剩「任务 7」+ 当前选中那项），没有任务 2',
    typed.items === 2 && typed.texts.includes('任务 7') && !typed.texts.includes('任务 2'),
    JSON.stringify(typed),
  )

  // ── ⑦ 清空 ⇒ 8 项都回来 ──
  //
  // ⚠ 原来这里还有三条「切到『推送配置』tab ⇒ 搜索词被清掉 / 推送栏 7 个账号也有搜索框」
  // （2026-10-05 那份一次性脚本的 ⑧⑨⑩ ✓）。**现在做不了**：推送那一半已被拆去
  // `pages/account-config-page.tsx`（本页只剩「公共配置」、**没有 tab 了** ✗），
  // 而账号侧那栏（`#acct-rail`，同样 >6 项才出搜索框 ✓）正由**另一条写入线**在拆
  // （工作区里未提交 ✗）⇒ 现在钉它等于给别人的在途改动上锁 ✗。
  // 留给那边落地后补：`#acct-rail input[type="search"]` + 7 个账号那两条 ✓。
  const cleared = await ev(`(async () => {
    const inp = document.querySelector('#config-rail input[type="search"]')
    const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set
    set.call(inp, '')
    inp.dispatchEvent(new Event('input', { bubbles: true }))
    await new Promise((r) => setTimeout(r, 300))
    return { items: document.querySelectorAll('#config-rail button').length, value: inp.value }
  })()`)
  check('清空后 8 项都回来', cleared.items === 8, JSON.stringify(cleared))

  // ── ⑪ 375 窄屏：带搜索框也不能横向溢出 ──
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width: 375,
    height: 812,
    deviceScaleFactor: 2,
    mobile: true,
  })
  await ev(`(() => { history.pushState({}, '', '/tasks/config'); window.dispatchEvent(new PopStateEvent('popstate')); return true })()`)
  await sleep(1200)
  const narrow = await ev(
    `(() => ({ sw: document.documentElement.scrollWidth, vw: window.innerWidth, hasSearch: !!document.querySelector('#config-rail input[type="search"]') }))()`,
  )
  check('375 下（带搜索框）无横向溢出', narrow.sw <= narrow.vw + 1, JSON.stringify(narrow))

  // ── ⑨⑩⑪ 账号侧那一栏（`/accounts/push`）：同一件事的另一份实现 ✓ ──
  //
  // ⚠ 2026-10-06 之前这条**没钉**（那一页正由另一条写入线拆着 ✗）。现在钉上：
  // 它是**另一份**"大清单 + 搜索框"实现（`account-config-page.tsx` 的 `accountItems()` ✓），
  // 而两份实现最容易各修各的 ⇒ 同一组判据要在两处都成立 ✓。
  await ev(`(() => { history.pushState({}, '', '/accounts/push'); window.dispatchEvent(new PopStateEvent('popstate')); return true })()`)
  await sleep(1200)
  const acctRail = await ev(`(() => {
    const r = document.getElementById('acct-rail')
    return { items: r ? r.querySelectorAll('button').length : 0, hasSearch: !!(r && r.querySelector('input[type="search"]')) }
  })()`)
  check('账号侧（7 个账号）搜索框也出现', acctRail.hasSearch === true, JSON.stringify(acctRail))
  check('账号侧列出全部 7 项', acctRail.items === 7, `实际 ${acctRail.items}`)
  const acctTyped = await ev(`(async () => {
    const inp = document.querySelector('#acct-rail input[type="search"]')
    inp.focus()
    const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set
    for (const ch of '账号3') {
      set.call(inp, inp.value + ch)
      inp.dispatchEvent(new Event('input', { bubbles: true }))
      await new Promise((r) => setTimeout(r, 60))
    }
    await new Promise((r) => setTimeout(r, 260))
    const now = document.querySelector('#acct-rail input[type="search"]')
    const texts = [...document.querySelectorAll('#acct-rail button')].map((b) => b.textContent.replace(/\s+/g, ' ').trim()).join(' | ')
    return {
      sameNode: now === inp,
      focused: document.activeElement === now,
      value: now.value,
      items: document.querySelectorAll('#acct-rail button').length,
      texts,
    }
  })()`)
  check('账号侧：打字后 input 还是同一节点', acctTyped.sameNode === true, JSON.stringify(acctTyped))
  check('账号侧：焦点还在、字没被回写', acctTyped.focused === true && acctTyped.value === '账号3', JSON.stringify(acctTyped))
  // ⚠ 账号侧这一栏是**纯过滤**（`accountItems()` 里没有"保留选中项"那条 ✓）⇒ 只剩 1 项 ✓
  //（任务侧那一栏**有**那条 ⇒ 是 2 项 ✓，两边判据不同，别抄 ✗）
  check('账号侧：筛到只剩「账号3」', acctTyped.items === 1 && acctTyped.texts.includes('账号3'), JSON.stringify(acctTyped))

  check('页面 0 条未捕获异常 / console.error', session.errors.length === 0, session.errors.join(' / '))

  const bad = results.filter((r) => !r.ok).length
  console.log(bad ? `\n✗ ${results.length - bad}/${results.length} 条通过` : `\n✓ 搜索框那条路 ${results.length} 条全过`)
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
  // ⚠ 收尾一律 best-effort：SIGKILL 之后 chrome 还会写一两笔，裸 rmSync 会抛 ENOTEMPTY ✗
  await sleep(300)
  try {
    fs.rmSync(PROFILE, { recursive: true, force: true, maxRetries: 8, retryDelay: 60 })
  } catch {
    // 临时目录，系统自己会清
  }
}
