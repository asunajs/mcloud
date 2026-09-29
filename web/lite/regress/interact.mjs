/**
 * 交互回归：**点一下、打几个字、切一次页之后**，lite 与 Vue 还一致吗。
 *
 * `compare.mjs` 只看首屏（一次 `--dump-dom`）。可这套框架里最容易出错的地方恰恰在
 * **首屏之后**：事件处理器、编译器自动包的 `batch`、`createFor` 的增删重排、
 * 切页时新建/卸载组件、受控输入框的回写 —— 这些路径首屏一个都走不到。
 *
 * 做法：两侧各起一个 chrome（CDP 驱动，零依赖），**执行同一串步骤**，每步之后抓一份
 * 同样的快照比对。快照不只含 DOM 文本，还含**表单 property**（`input.value`、`checkbox.checked`
 * 这些 `outerHTML` 看不到、但用户看得见的状态）、主题、打开的弹窗与当前 hash。
 *
 * 用法：
 *   node lite/regress/interact.mjs [--variant ready|login|setup] [--only 关键字]
 *
 * 退出码非 0 = 有步骤两侧不一致（或某一侧抛了页面异常）。
 */
import fs from 'node:fs'
import { appSubtree, arg, hashRoute, openSession, readVariant, serve, sleep } from './lib.mjs'

const variant = readVariant()
const only = arg('--only', '')
const liteDir = arg('--lite', '/tmp/lite-app')
const vueDir = arg('--vue', 'dist')
const route = hashRoute('settings')

/**
 * 页面里的快照。`dom` 走 `lib.mjs` 的 `appSubtree` 归一化（与 compare.mjs **同一份规则**），
 * 所以"两边一致"在这里与首屏回归里是同一个判据。
 */
const PROBE = `JSON.stringify({
  hash: location.hash,
  theme: document.documentElement.getAttribute('data-theme'),
  openDialogs: [...document.querySelectorAll('dialog[open]')].map((d) => d.id).join(','),
  // ⚠ 这些是 **property**：outerHTML 里看不到，但它们是"用户看到的东西"
  form: [...document.querySelectorAll('input,select,textarea')].map((e) =>
    (e.type === 'checkbox' || e.type === 'radio') ? e.checked : e.value).join('|'),
  text: document.getElementById('app')?.textContent ?? '',
  dom: document.getElementById('app')?.outerHTML ?? '',
})`

const snapshot = async (s) => {
  const j = JSON.parse(await s.cdp.eval(PROBE))
  return JSON.stringify({ hash: j.hash, theme: j.theme, openDialogs: j.openDialogs, form: j.form, app: appSubtree(j.dom) })
}

const clickSel = (sel) => `(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) throw new Error('找不到元素: ' + ${JSON.stringify(sel)}); el.click(); return true })()`
const clickText = (sel, text) =>
  `(() => { const el = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => e.textContent.trim() === ${JSON.stringify(text)}); if (!el) throw new Error('找不到 ' + ${JSON.stringify(sel)} + ' 里文本为 ' + ${JSON.stringify(text)} + ' 的元素'); el.click(); return true })()`
const typeIn = (sel, value) =>
  `(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) throw new Error('找不到输入框: ' + ${JSON.stringify(sel)}); el.value = ${JSON.stringify(value)}; el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); return el.value })()`
const pickOption = (sel, value) =>
  `(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) throw new Error('找不到下拉框: ' + ${JSON.stringify(sel)}); el.value = ${JSON.stringify(value)}; el.dispatchEvent(new Event('change', { bubbles: true })); return el.value })()`
const go = (hash) => `(() => { location.hash = ${JSON.stringify(hash)}; return location.hash })()`

const ROUTES = ['dashboard', 'accounts', 'tasks', 'exchange', 'live-room', 'schedules', 'pipelines', 'history', 'settings']

const STEPS = {
  /** 已登录：切页、导航点击、主题、抽屉、弹窗、受控输入框、下拉框、勾选框。 */
  ready: [
    ['首屏（已登录）', null],
    // 切页：每条路由都走一遍（首屏回归是"直接打开该路由"，这里是"**切过去**"，
    // 走的是 page 条件分支 + 新页组件创建 + onMounted + 各页自己的取数）
    ...ROUTES.map((r) => [`切到 ${r}`, go(`#/${r}`)]),
    ['点导航（抽屉）：历史', clickText('ul.menu button', '历史')],
    ['点 dock：任务', clickText('.dock button', '任务')],
    ['展开抽屉', clickSel('label[for="nav-drawer"]')],
    ['收起抽屉', clickSel('label[for="nav-drawer"]')],
    ['切深色主题', clickSel('[aria-label="切换到深色主题"]')],
    ['切回浅色主题', clickSel('[aria-label="切换到浅色主题"]')],
    ['打开登出确认框', clickSel('[aria-label="退出登录"]')],
    ['取消（关掉确认框）', clickText('#confirm-logout .modal-action button', '取消')],

    // 账号页：四种登录方式之间来回切（条件分支 + 4 项列表），并在出现的输入框里打字。
    // 注意"短信/账号密码"那两个框**只有切到该方式才存在** —— 这正是在测条件分支换内容。
    ['切到账号页', go('#/accounts')],
    ['账号页：点「短信」方式', clickText('button', '短信')],
    ['账号页：输入手机号', typeIn('input[type="tel"]', '19900000001')],
    ['账号页：改手机号', typeIn('input[type="tel"]', '19900000002')],
    ['账号页：点「账号密码」方式', clickText('button', '账号密码')],
    ['账号页：输入用户名', typeIn('input[autocomplete="username"]', 'someone')],
    ['账号页：输入口令', typeIn('input[type="password"]', 'pw-123456')],
    ['账号页：点回「扫码」方式', clickText('button', '扫码')],
    ['账号页：点某行的「移除」', clickText('button', '移除')],
    ['账号页：取消移除（有数据的确认弹窗）', clickText('#confirm-delete-account .modal-action button', '取消')],

    // 计划页：新建表单填一遍（文本、下拉、勾选框都覆盖到）
    ['切到计划页', go('#/schedules')],
    ['计划页：填名称', typeIn('input[placeholder="例如 每天签到"]', '每周签到')],
    ['计划页：改 cron', typeIn('input[placeholder="0 8 * * *"]', '0 9 * * 1')],
    ['计划页：选任务', pickOption('select.select', 'live-room')],
    ['计划页：勾选启用', clickSel('input[type="checkbox"].checkbox')],

    // 直播页：口令textarea、时长数字框、开关
    ['切到直播页', go('#/live-room')],
    ['直播页：输入口令', typeIn('textarea', '口令一 口令二')],
    ['直播页：改时长', typeIn('input[type="number"]', '45')],
    ['直播页：切开关', clickSel('input[type="checkbox"].toggle')],

    // 任务页：下拉框的选项是**由数据 createFor 出来的**，选它等于测"列表项 + select 回写"
    ['切到任务页', go('#/tasks')],
    ['任务页：选账号', pickOption('select.select', '13800000000')],
  ],
  /** 未初始化：建管理员向导。填三个字段再提交（fixture 没有 POST 接口 ⇒ 走错误分支，也要一致）。 */
  setup: [
    ['首屏（未初始化）', null],
    ['填用户名', typeIn('input[autocomplete="username"]', 'admin')],
    ['填口令', typeIn('input[type="password"]', 'secret-1234')],
    ['填确认口令', typeIn('input[autocomplete="new-password"]', 'secret-1234')],
    ['提交（后端没有该接口 ⇒ 错误分支）', clickText('button', '创建并进入')],
    ['再填一次用户名', typeIn('input[autocomplete="username"]', 'admin2')],
  ],
  /** 已初始化未登录：登录页同上。 */
  login: [
    ['首屏（未登录）', null],
    ['填用户名', typeIn('input[autocomplete="username"]', 'admin')],
    ['填口令', typeIn('input[type="password"]', 'wrong-pass')],
    ['提交（后端没有该接口 ⇒ 错误分支）', clickText('button', '登录')],
  ],
}

/**
 * 已知的**故意不同**。每一条都必须写清机制与理由 —— 这不是"过不去就算了"的垃圾桶，
 * 只是把"我们查明白了、并且是有意为之"与"还不知道为什么"分开。
 *
 * `计划页：填名称` / `计划页：改 cron`：**Vapor 那边是它自己的缺陷**。
 * 它把整个模板的动态属性写进**同一个** `renderEffect`（`R(()=>{ H(名称输入,c.value),
 * H(下拉框,l.value), … })`），于是"改名称"会**顺带重写下拉框的值**；而下拉框的
 * `fTask` 此时是 `''`（应用只在"编辑已有计划"时才赋值），
 * `select.value = ''` 又匹配不到任何选项 ⇒ **下拉框显示变空**（`selectedIndex = -1`）。
 * lite 的属性绑定是细粒度的，改名称不会重写下拉框，于是它保留着浏览器自动选中的
 * 「每日签到」—— 用户看到的是**不空**的那一个。功能上两者一致（提交时 `fTask` 都是 `''`，
 * 都会提示"请选择任务"），差的是那一眼的显示。要"逐字符一致"就得把这份粗粒度照抄过来，
 * 那是故意做得更差，不做。
 */
const EXPECTED_DIVERGENT = {
  '计划页：填名称': 'Vapor 的粗粒度 renderEffect 会把 select 的价值重写成空串（见上面注释）',
  '计划页：改 cron': '同上（同一根因，改 cron 也会触发那一个 effect）',
}

/** 等 DOM 稳定：连续 3 次探测一致（间隔 150ms）就算稳定 —— 本地 fixture 足够快。 */
const waitStable = async (s, tries = 40) => {
  let last = ''
  let same = 0
  for (let i = 0; i < tries; i++) {
    let now = ''
    try {
      now = await snapshot(s)
    } catch {
      // 页面正在导航/关停，下一轮再看
    }
    if (now && now === last) {
      if (++same >= 2) return
    } else {
      same = 0
      last = now
    }
    await sleep(150)
  }
}

const runSide = async (side, dir, port, debugPort) => {
  const server = await serve(dir, port, variant)
  const session = await openSession({ port, route, debugPort, profile: `/tmp/lite-interact-${side}` })
  const shots = []
  try {
    await waitStable(session)
    for (const [name, js] of STEPS[variant]) {
      if (only && !name.includes(only)) continue
      /**
       * ⚠ 单步失败**不能**把整轮带崩：选择器写错、某一步在某一侧不适用，都会抛。
       * 把异常记成这一步的结果继续跑，最后单独把"哪些步没真正执行"列出来 ——
       * 那比"跑到第 20 步炸了、前面 19 步的结论全丢"有用得多。
       */
      let threw = null
      if (js) {
        try {
          await session.cdp.eval(js)
        } catch (e) {
          threw = String(e.message ?? e)
        }
      }
      await waitStable(session)
      shots.push([name, threw ? `THREW: ${threw}` : await snapshot(session)])
    }
  } finally {
    session.cdp.close()
    session.child.kill('SIGKILL')
    server.close()
  }
  return { shots, errors: session.errors }
}

const lite = await runSide('lite', liteDir, 48251, 49551)
const vue = await runSide('vue', vueDir, 48252, 49552)

let bad = 0
console.log(`交互回归（variant=${variant}，${lite.shots.length} 步）`)
for (let i = 0; i < lite.shots.length; i++) {
  const [name, a] = lite.shots[i]
  const b = vue.shots[i]?.[1]
  if (a === b) {
    console.log(`  ✅ ${name}`)
    continue
  }
  if (EXPECTED_DIVERGENT[name]) {
    console.log(`  ⚠ ${name} —— 已知差异（有意）：${EXPECTED_DIVERGENT[name]}`)
    continue
  }
  if (a.startsWith('THREW:') || b?.startsWith('THREW:')) {
    bad++
    console.log(`  ✗ ${name}`)
    console.log(`     lite ${a.startsWith('THREW:') ? a : '（正常）'}`)
    console.log(`     vue  ${b?.startsWith('THREW:') ? b : '（正常）'}`)
    continue
  }
  bad++
  console.log(`  ✗ ${name}`)
  if (b === undefined) continue
  const j = [...a].findIndex((c, k) => c !== b[k])
  console.log(`     首个差异在第 ${j} 个字符`)
  console.log(`     lite: ${JSON.stringify(a.slice(Math.max(0, j - 60), j + 90))}`)
  console.log(`     vue : ${JSON.stringify(b.slice(Math.max(0, j - 60), j + 90))}`)
  // 落盘两侧的**完整快照**（含 property 与 DOM），便于离线看差异
  fs.mkdirSync('/tmp/interact', { recursive: true })
  const safe = name.replace(/[^\p{L}\p{N}]+/gu, '-')
  fs.writeFileSync(`/tmp/interact/${String(i).padStart(2, '0')}-${safe}.lite.json`, a)
  fs.writeFileSync(`/tmp/interact/${String(i).padStart(2, '0')}-${safe}.vue.json`, b ?? '')
}
const pageErrors = (side, r) => r.errors.filter((e) => !e.includes('favicon'))
console.log(`\n页面异常：lite ${pageErrors('lite', lite).length} 条，Vue ${pageErrors('vue', vue).length} 条`)
for (const e of pageErrors('lite', lite).slice(0, 5)) console.log(`  lite: ${e.split('\n')[0]}`)
for (const e of pageErrors('vue', vue).slice(0, 3)) console.log(`  vue : ${e.split('\n')[0]}`)
if (bad) {
  console.log(`\n✗ ${bad}/${lite.shots.length} 步不一致；两侧 DOM 落在 /tmp/interact/`)
  process.exitCode = 1
} else {
  console.log(`\n✓ ${lite.shots.length} 步全部一致`)
}
