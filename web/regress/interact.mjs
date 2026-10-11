/**
 * 交互验收：**点一下、打几个字、切一次页之后**，界面还是对的吗。
 *
 * 以前这条是"两侧对拍"（lite vs Vue 逐字符比）。2026-09-30 起不再有 Vue 那一侧，
 * 于是判据从"和它一样吗"换成三类**绝对断言**：
 *
 * 1. **动作必须真的执行** —— 找不到元素就抛，抛了直接判负。
 *    （旧版栽过：选择器写错 ⇒ 两侧都"没找到" ⇒ 逐字符比对反而全绿，44 步一步没跑。）
 * 2. **声明了 `check` 的步骤要在页面里读到期望值** —— 文本、属性、表单 property。
 * 3. **全程 0 条页面异常** —— `Runtime.exceptionThrown` 与非 favicon 的 `console.error`。
 *
 * 另有 `changed: true`：要求这一步**真的改变了快照**，用来抓"点了但界面不动"
 * —— 那正是编排页步骤列表踩过的浅响应坑（`pipelines-page.tsx` 的 `steps` 注释）。
 *
 * 用法：
 *   node regress/interact.mjs [--variant ready|login|setup] [--only 关键字] [--dir dist]
 *
 * 退出码非 0 = 有步骤没执行 / 断言不符 / 页面抛过异常。
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { appSubtree, arg, gotoJs, openSession, readVariant, routeUrl, serve, sleep } from './lib.mjs'

const variant = readVariant()
const only = arg('--only', '')
const dir = arg('--dir', 'dist')
/**
 * fixture 服务的端口。**默认 0 = 由内核分配一个空闲端口**。
 *
 * 写死 48251 的坑：只要那个端口上还有东西（上一份验收的进程没死透，或者同一个工作区里
 * 另一个人/另一个 agent 正在跑同一支脚本），`serve()` 的 `listen` 就是 EADDRINUSE，
 * 而它**死在任何 stdout 之前** ⇒ 三个变体全是一声闷响的「✗ 抛错」。
 * 真实端口从 `server.address().port` 读，别再假设就是这里写的那个数。
 */
const PORT = Number(arg('--port', '0'))
/**
 * CDP 调试端口。0 = 内核分配，起完从 profile 的 `DevToolsActivePort` 读回来。
 * （旧版写死 `PORT + 1300`：端口一旦动态化这个式子就没意义了，而且写死本身也会撞。）
 */
const DEBUG = Number(arg('--debug-port', '0'))
/**
 * chrome 的 profile，**每次跑都必须独立**。
 *
 * 写死 `/tmp/lite-interact` 的坑：后起的 chrome 发现 profile 被占，会把 URL 转交给
 * 前一个实例然后自己退出，于是调试端口永远等不到 —— 又是一声闷响。
 * 默认在 tmp 下开一个一次性目录（跑完删掉）；`--profile` 传进来的**不删**（那是调用方的）。
 */
const PROFILE = arg('--profile', '') || fs.mkdtempSync(path.join(os.tmpdir(), 'lite-interact-'))
/** 只有我们自己建的临时 profile 才由我们删。 */
const ownedProfile = !arg('--profile', '')
const route = routeUrl('settings')

/**
 * 页面里的快照。`dom` 走 `lib.mjs` 的 `appSubtree` 归一化，所以"变了没有"有一个稳定口径。
 *
 * ⚠ 表单那些是 **property**：`outerHTML` 看不到，但它们是用户看得见的状态
 * （受控输入框有没有被回写成旧值，全靠这里抓到）。
 */
const PROBE = `JSON.stringify({
  path: location.pathname,
  theme: document.documentElement.getAttribute('data-theme'),
  openDialogs: [...document.querySelectorAll('dialog[open]')].map((d) => d.id).join(','),
  form: [...document.querySelectorAll('input,select,textarea')].map((e) =>
    (e.type === 'checkbox' || e.type === 'radio') ? e.checked : e.value).join('|'),
  app: (document.getElementById('app')?.outerHTML ?? ''),
})`

const snapshot = async (s) => {
  const j = JSON.parse(await s.cdp.eval(PROBE))
  return JSON.stringify({ path: j.path, theme: j.theme, openDialogs: j.openDialogs, form: j.form, app: appSubtree(j.app) })
}

// ── 动作与断言的写法 ─────────────────────────────────────────────────────────
const clickSel = (sel) => `(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) throw new Error('找不到元素: ' + ${JSON.stringify(sel)}); el.click(); return true })()`
const clickText = (sel, text) =>
  `(() => { const el = [...document.querySelectorAll(${JSON.stringify(sel)})].find((e) => e.textContent.trim() === ${JSON.stringify(text)}); if (!el) throw new Error('找不到 ' + ${JSON.stringify(sel)} + ' 里文本为 ' + ${JSON.stringify(text)} + ' 的元素'); if (el.disabled) throw new Error(${JSON.stringify(text)} + ' 是禁用的'); el.click(); return true })()`
const typeIn = (sel, value) =>
  `(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) throw new Error('找不到输入框: ' + ${JSON.stringify(sel)}); el.value = ${JSON.stringify(value)}; el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); return el.value })()`
/**
 * 同一个选择器命中**多个**时按序号取。
 *
 * 建管理员向导里非用不可：口令与确认口令都是 `type=password`，而且 setup 态下
 * 两个框的 `autocomplete` **都是 `new-password`**（`auth-page.tsx:171` 把口令框也标成
 * new-password 了 —— 浏览器不该回填旧口令）。于是 `input[autocomplete=new-password]`
 * 命中的是**第一个**（口令框），照它写"填确认口令"就等于把口令又填了一遍、
 * 确认框留空 ⇒ 永远"两次输入的口令不一致"，闸门从没放行过。
 * 两侧对拍的时代这一步是绿的：**两侧都卡在同一个地方，逐字符比对照样一致**。
 */
const typeNth = (sel, n, value) =>
  `(() => { const el = document.querySelectorAll(${JSON.stringify(sel)})[${n}]; if (!el) throw new Error('第 ${n + 1} 个 ${JSON.stringify(sel)} 不存在'); el.value = ${JSON.stringify(value)}; el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); return el.value })()`
const nthValue = (sel, n) => `document.querySelectorAll(${JSON.stringify(sel)})[${n}]?.value ?? '(没有第 ${n + 1} 个)'`
/**
 * 选一个下拉项。
 *
 * ⚠ 2026-10-02 起下拉是**自建组件**（`ui/select.tsx`），没有 `select.value` 可写、
 * 也没有 `change` 事件可派发 ⇒ 只能"点开触发器 → 点那一项"。
 * 判据仍读 `data-value`（组件在触发器上留的可读出口）。
 */
const pickOption = (sel, value) =>
  `(() => {
     const t = document.querySelector(${JSON.stringify(sel)});
     if (!t) throw new Error('找不到下拉框: ' + ${JSON.stringify(sel)});
     t.click();
     const o = [...document.querySelectorAll('.select-panel .select-option')]
       .find((x) => x.dataset.value === ${JSON.stringify(value)});
     if (!o) throw new Error('下拉里没有选项: ' + ${JSON.stringify(value)});
     o.click();
     return document.querySelector(${JSON.stringify(sel)}).dataset.value;
   })()`
const go = (route) => gotoJs(route)

/** 向导里那两个口令框（setup 态下 `autocomplete` 撞车，只能按序号取，见 `typeNth`）。 */
const PWDS = '#app input[type="password"]'

/**
 * 页面侧的小工具，起完 chrome 注入一次（hash 路由不重载文档，`window` 上的东西一直在）。
 *
 * 为什么要注入而不是每条表达式各写一遍：步骤行的定位条件有点挑 ——
 * 步骤行是 `div.rounded-box` **且**里面有「上移」按钮（同页另有一块 `bg-base-200`，
 * 历史页/总览页也共用这个类）。写一次、到处用，选择器错了只改一处。
 */
const HELPERS = `(() => {
  window.__stepRows = () => [...document.querySelectorAll('#app div.rounded-box')]
    .filter((d) => [...d.querySelectorAll('button')].some((b) => b.textContent.trim() === '上移'));
  window.__stepBtn = (n, text) => {
    const rows = window.__stepRows();
    if (!rows[n]) throw new Error('没有第 ' + (n + 1) + ' 步（当前 ' + rows.length + ' 行）');
    const b = [...rows[n].querySelectorAll('button')].find((x) => x.textContent.trim() === text);
    if (!b) throw new Error('第 ' + (n + 1) + ' 步里没有按钮「' + text + '」');
    if (b.disabled) throw new Error('第 ' + (n + 1) + ' 步的「' + text + '」是禁用的');
    return b;
  };
  // ⚠ 步骤里的任务下拉也是自建组件（ui/select.tsx）⇒ 读 data-value
  window.__stepOrder = () => window.__stepRows().map((d) => d.querySelector('.select').dataset.value).join(',');
  window.__stepBadges = () => window.__stepRows().map((d) => d.querySelector('.badge').textContent.replace(/\\s+/g, '')).join(',');
  /**
   * 某个弹窗里"客户端形态"那几个按钮有几个。
   * ⚠ 弹窗里还有一排 join（方式 tab），所以按**文字**过滤，
   * 不能只数 .join button。
   */
  window.__platformBtns = (id) => [...document.querySelectorAll('#' + id + ' .join button')]
    .filter((b) => ['Windows', 'macOS'].includes(b.textContent.trim())).length;
  /** 那颗 ✕ 是否**确实**落在视口里（窄屏弹窗"没有出口"就是这么漏掉的）。 */
  window.__closeBtnInViewport = (id) => {
    const b = document.querySelector('#' + id + ' button[aria-label="关闭"]');
    if (!b) return '没有关闭键';
    const r = b.getBoundingClientRect();
    return r.top >= 0 && r.bottom <= window.innerHeight && r.width > 0 && r.height > 0;
  };
  window.__stepDisabled = (n, text) => {
    const rows = window.__stepRows();
    const b = [...rows[n].querySelectorAll('button')].find((x) => x.textContent.trim() === text);
    return b ? b.disabled : '没有这个按钮';
  };
  return true
})()`

const APP_TEXT = `document.getElementById('app')?.textContent ?? ''`
/** 断言：在页面里求值 `expr`，要求它等于 / 包含 / 不包含 `want`。 */
const eq = (expr, want) => ({ expr, want, kind: 'eq' })
const has = (expr, needle) => ({ expr, want: needle, kind: 'has' })
/**
 * 「**不**包含」——`matchCheck` 的默认分支就是它 ✓（`kind` 既不是 `eq` 也不是 `has` 时
 * 走 `!includes` ✓）。
 *
 * ⚠ 这里必须给一个**对象**（带 `expr` ✓）：早先我按 `has(...)` 的样子写成裸字符串，
 * CDP 那边直接 `Failed to deserialize params.expression` ✗ —— 断言求值抛错也是失败 ✓。
 */
const hasNot = (expr, needle) => ({ expr, want: needle, kind: 'hasNot' })
const not = (expr, needle) => ({ expr, want: needle, kind: 'not' })

const matchCheck = (got, { want, kind }) => (kind === 'eq' ? got === want : kind === 'has' ? String(got).includes(want) : !String(got).includes(want))

/**
 * 全路由清单（每一步都是"**切过去**"，不是"直接打开"）。
 *
 * ⚠ 2026-10-08 跟着导航改（用户口径「合并这两个（历史与日志），并单独给推送一个」✓）：
 * `logs` **留着**（它不再是导航项，但仍是真地址 `/logs` = 「历史」页的第二个 tab ✓，
 * 而且那条地址**必须继续认** ⇒ 顺带走一遍 ✓）；`push` **加上**（新页 ✓）。
 */
const ROUTES = ['dashboard', 'accounts', 'tasks', 'exchange', 'live-room', 'schedules', 'pipelines', 'history', 'logs', 'push', 'settings']

/**
 * 设置页顶部四节 tab 的读数（2026-10-08 用户口径「设置页面内容太多了，也没有分页面」✓）：
 * 四节的名字 + 当前选中那一节 + 地址。
 *
 * ⚠ 三个一起读是**故意的**：只读"选中的是哪一节"分不出"地址跟着没跟着换"
 * （tab 是本地状态、地址不动的那种写法照样绿 ✗），只读地址又分不出"tab 亮没亮" ✓。
 * ⚠ 这段在**模板字符串**里：绝不能出现反引号（踩过 —— 一个反引号就把模板提前结束，
 * 整个文件 SyntaxError，门禁只报一句"抛错"✗）。
 */
const SETTINGS_TAB_STATE = `(() => {
  const tabs = [...document.querySelectorAll('#app [role="tablist"] [role="tab"]')]
  const sel = tabs.find((t) => t.getAttribute('aria-selected') === 'true')
  return [tabs.map((t) => t.textContent.trim()).join(','), sel ? sel.textContent.trim() : 'none', location.pathname].join(' | ')
})()`

/** 设置页当前那一节的 `.card-title` 列表（"哪几张卡在这一节里"就靠它数 ✓）。 */
const SETTINGS_CARD_LIST = `[...document.querySelectorAll('#app .card')].map((c) => (c.querySelector('.card-title')?.textContent ?? '').trim())`
/** 同上，拼成一个字符串（顺序也在里面 ✓）。 */
const SETTINGS_CARDS = `${SETTINGS_CARD_LIST}.join(',')`

/**
 * 滚到**日志页**列表的最上面，并**等窗口真的落在那儿**（2026-10-09 分页那几步要用 ✓）。
 *
 * ⚠⚠ 为什么要"等"而不是"睡 400ms"：那一页是**窗口化**的（只铺看得见的那几十行 ✓）
 * ⇒ "最上面那一行"只能先把 `scrollTop` 归零、让窗口挪到 0 才读得到 ✓；而"挪"要经过
 * 一次滚动事件 + 一次渲染，机器一忙（推送档 7 条并行跑）就要几百毫秒 ⇒ 固定睡一觉会读到
 * **窗口还没挪过去**时那一段行（实测：`topDrop` 读到 **6** 而不是 500 ✗ ——
 * 那不是功能坏了，是**量错了** ✓，而"量错"比"红"更坏：它会让人去查一个不存在的问题 ✗）。
 *
 * ⚠ 判据用**上垫片的高度**（`win().top` ✓）：窗口起点是 0 时它就是 0px ✓ ——
 * 这是"窗口真的在最上面"的**直接**证据（比"睡够时间"稳 ✓，也比"第一行的行号是几"
 * 可用：那个数正是要量的东西，不能拿来当前置条件 ✗）。
 */
const logsToTop = `(async () => {
  const box = document.getElementById('logs-scroll')
  if (!box) throw new Error('没有滚动容器 #logs-scroll')
  box.scrollTop = 0
  box.dispatchEvent(new Event('scroll', { bubbles: true }))
  const atTop = () => {
    const spacer = box.firstElementChild
    return !!spacer && spacer.getBoundingClientRect().height === 0
  }
  for (let i = 0; i < 100; i++) {
    if (atTop()) return true
    await new Promise((r) => setTimeout(r, 50))
  }
  throw new Error('窗口没挪到最上面（上垫片还撑着 —— 页面卡住了？）')
})()`

const STEPS = {
  /** 已登录：切页、导航、抽屉、主题、弹窗、受控输入框、下拉框、勾选框、编排页的增删重排。 */
  ready: [
    { name: '首屏（已登录）', do: null, check: eq('!!document.querySelector("#app .dock")', true) },
    // 切页：每条路由都走一遍（首屏回归是"直接打开该路由"，这里是"**切过去**"，
    // 走的是 page 条件分支 + 新页组件创建 + onMounted + 各页自己的取数）
    ...ROUTES.map((r) => ({ name: `切到 ${r}`, do: go(r), check: eq('location.pathname', `/${r}`), changed: true })),
    { name: '点导航（抽屉）：历史', do: clickText('ul.menu button', '历史'), check: eq('location.pathname', '/history') },
    /**
     * ── 「历史 / 日志」两个 tab（2026-10-08 用户口径「合并这两个」）────────────────
     *
     * 这一组钉三件事：
     * ① **老地址 `/logs` 照旧认**（书签 / 浏览器历史 / 旧文档里的链接不能 404，
     *    也不能把人甩回总览 ✗）—— 直接开它必须落在**日志 tab** 上 ✓；
     * ② tab 真的**在切**（不是只有地址变）：切回「历史」时日志页那套控件**必须整个不在**
     *    （`#logs-scroll` 为 null ✓）—— 这条正是"两个大组件别同时塞进渲染树"的钉子：
     *    同时挂着的后果是"看历史的时候也在每 2 秒轮询 `/api/logs`" ✗；
     * ③ 切回来之后历史页还在（两个 tab 各自的功能一个都没丢 ✓）。
     *
     * ⚠ 收尾必须回到 `/history`：下面那几条历史页断言（直播明细 / 兑换明细）就在这个位置 ✓。
     */
    {
      name: '老地址 /logs 直接打开 ⇒ 落在「日志」tab（书签不能 404）',
      do: go('logs'),
      check: eq(
        `(() => {
           const sel = document.querySelector('#app [role="tablist"] [role="tab"][aria-selected="true"]')
           return [location.pathname, sel ? sel.textContent.trim() : 'none',
                   !!document.getElementById('logs-scroll')].join('/')
         })()`,
        '/logs/日志/true',
      ),
      changed: true,
    },
    {
      name: '历史页：点「历史」tab ⇒ 回 /history，且日志那一块整个卸载（不同时渲染）',
      do: clickText('#app [role="tablist"] button', '历史'),
      check: eq(
        `(() => {
           const sel = document.querySelector('#app [role="tablist"] [role="tab"][aria-selected="true"]')
           return [location.pathname, sel ? sel.textContent.trim() : 'none',
                   !!document.getElementById('logs-scroll'),
                   document.querySelectorAll('#app table tbody tr').length > 0].join('/')
         })()`,
        '/history/历史/false/true',
      ),
      changed: true,
    },
    {
      name: '历史页：点「日志」tab ⇒ 地址变 /logs、日志控件出现（tab 真的在切）',
      do: clickText('#app [role="tablist"] button', '日志'),
      check: eq(
        `(() => {
           const sel = document.querySelector('#app [role="tablist"] [role="tab"][aria-selected="true"]')
           return [location.pathname, sel ? sel.textContent.trim() : 'none',
                   !!document.getElementById('logs-scroll')].join('/')
         })()`,
        '/logs/日志/true',
      ),
      changed: true,
    },
    {
      // ⚠ 收尾：下面几条历史页的断言要用这一页 ✓（也顺带钉"切回来是干净的一页"）
      name: '历史页：切回「历史」tab（后面的历史断言要用）',
      do: clickText('#app [role="tablist"] button', '历史'),
      check: eq('location.pathname', '/history'),
      changed: true,
    },
    {
      // ⭐ 「历史页复用结构化明细」的直接证据：**过去**那一场直播的逐条口令，
      // 现在能在历史里看到（从前只有一行小结，明细打不开）。
      name: '历史页：过去的直播能看到逐条口令明细',
      do: null,
      check: eq(
        `(() => { const t = document.getElementById('app').textContent; return t.includes('历史口令甲') + '/' + t.includes('历史口令乙') })()`,
        'true/true',
      ),
    },
    {
      // ⭐⭐ 「**所有内容都要进记录**」（用户口径 2026-10-02）：兑换那一次的
      // 结构化明细（后端 `details` 是**铺平**的：kind/status/plan/code/rounds/
      // attempts/offset/balance/after_balance/waited/device_id_failed/error/
      // prize/order）必须在历史页**全部**看得见。
      // 这条把用户回看时真正要问的几项都钉住 —— 少渲染任何一个它就红。
      name: '历史页：兑换那次的明细全都进记录（码/轮次/发单/offset/余额/订单/奖品）',
      do: null,
      check: eq(
        `(() => {
           const t = document.getElementById('app').textContent;
           const need = ['ORD-1025', '上游码 0', '轮次 1', '发单 2', 'offset 87',
                         '10496', '8696', '小红花', '1800'];
           const miss = [];
           for (const k of need) { if (t.indexOf(k) < 0) miss.push(k); }
           return miss.length === 0 ? 'ok' : '缺这些字段：' + miss.join(' / ');
         })()`,
        'ok',
      ),
    },
    {
      // ⭐ 「本号已领过」必须**带主语**（用户口径 2026-10-02：不要造成歧义）。
      // 光写"已领过"会被读成"这条口令被用掉了" —— 而口令是全场通用、人人可领的，
      // 那个误解会把人引向完全错误的方向（以为要去找新口令）。
      name: '历史页：200112 写成「本号已领过」（带主语，不歧义）',
      do: null,
      check: has(APP_TEXT, '本号已领过'),
    },
    /**
     * ── 历史页：「运行日志」弹窗**分页取**（2026-10-09 用户口径「大日志做虚拟加载
     *    或者分页，避免一次加载太多」✓）────────────────────────────────────────
     *
     * 钉三件事，⚠ **都要真的取到更早的行**，不能只断言"按钮在"：
     * ① 首屏只取**最近 500 行**（夹具那次运行共 800 行 ✓）—— 一次全取就没有分页可言 ✗；
     * ② 点「加载更早的 500 行」⇒ 界面读数变成 **800 行**、按钮消失（到头了 ✓）；
     * ③ 滚到顶之后**第一行是第 0001 行**，而且窗口里那几行是**连续**的 ✓ ——
     *    只数行数的话，"`before` 写成别的游标 ⇒ 同一段行取了两遍"会得到 1000 行，
     *    行数照样"变多了" ✗（夹具正文里带行号，正是为了能逐字对上 ✓）。
     *
     * ⚠ 面板是窗口化的（只铺看得见的那几十行 ✓）⇒ 要读"第一行"得先滚到顶。
     * 滚动放在 `do` 里、断言放在 `check` 里（`check` 不许改页面 ✓）。
     */
    {
      name: '历史页：开「运行日志」弹窗 ⇒ 首屏只取最近 500 行（夹具共 800 行）',
      do: `(() => {
        const b = document.querySelector('#app button[aria-label="查看日志：live-room #1024"]')
        if (!b) throw new Error('找不到那次运行的「日志」按钮')
        b.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('run-log-dialog')
           if (!d || !d.open) return 'not-open'
           const t = d.textContent
           return [t.includes('已显示 500 行'), t.includes('加载更早的 500 行'),
                   d.querySelectorAll('[data-k]').length > 0].join('/')
         })()`,
        'true/true/true',
      ),
      changed: true,
    },
    {
      /**
       * ⚠ 往上拼接**不许**把已有行整批换新对象（`createFor` 按"key 与 item 都没变"复用行 ✓）——
       * 换新对象的表现是"已经渲染的每一行都被重建"，用户那边是**正在选中的文字被清掉、
       * 滚动位置丢掉** ✗（`ui/log-panel.tsx` 与 `ui/virtual-rows.ts` 文件头那条契约 ✓）。
       *
       * 判据与 `bulk.mjs` ⑥′ 同款：给窗口**中间**那一行打一个 **JS 属性** ⇒
       * 翻页之后按它自己的 `data-k` 找回那一行，属性还在 = 同一个节点被留住了 ✓
       * （打中间那一行：翻页前后**看到的是同一段内容** —— 页面上那一下滚动补偿
       * 就是为了这个 ✓，所以那一行必定还在窗口里 ✓）。
       */
      name: '历史页：往上翻页前，给窗口中间那一行打个记号',
      do: `(() => {
        const box = document.querySelector('#run-log-dialog .h-64')
        if (!box) throw new Error('找不到日志滚动容器')
        const rows = [...box.querySelectorAll('[data-k]')]
        if (rows.length < 5) throw new Error('面板里行太少（' + rows.length + '）')
        const el = rows[Math.floor(rows.length / 2)]
        el.__keep = 1
        window.__runLogKey = el.dataset.k
        return true
      })()`,
      check: eq(`!!window.__runLogKey`, true),
    },
    {
      name: '历史页：点「加载更早的 500 行」⇒ 真的取到更早的行（800 行，按钮消失）',
      do: `(async () => {
        const b = [...document.querySelectorAll('#run-log-dialog button')]
          .find((x) => x.textContent.trim() === '加载更早的 500 行')
        if (!b) throw new Error('没有「加载更早」按钮')
        b.click()
        for (let i = 0; i < 60; i++) {
          const t = document.getElementById('run-log-dialog')?.textContent ?? ''
          if (t.includes('已显示 800 行')) break
          await new Promise((r) => setTimeout(r, 100))
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('run-log-dialog')
           const t = d ? d.textContent : ''
           return [t.includes('已显示 800 行'), t.includes('加载更早')].join('/')
         })()`,
        'true/false',
      ),
      changed: true,
    },
    {
      name: '历史页：往上翻页**不重建已有行**（打了记号的那一行还是同一个节点）',
      do: null,
      check: eq(
        `(() => {
           const box = document.querySelector('#run-log-dialog .h-64')
           const el = [...box.querySelectorAll('[data-k]')].find((d) => d.dataset.k === window.__runLogKey)
           return [!!el, el ? el.__keep === 1 : false].join('/')
         })()`,
        'true/true',
      ),
    },
    {
      // ⚠ 滚到顶（`scrollTop = 0` + 补一次 `scroll` 事件）—— 面板据此重算窗口，
      // 第一行才会真的铺出来（窗口化只画看得见的那几十行 ✓）。
      name: '历史页：把日志滚到顶 ⇒ 第一行是第 0001 行，窗口里那几行连续（没有重复取）',
      do: `(() => {
        const box = document.querySelector('#run-log-dialog .h-64')
        if (!box) throw new Error('找不到日志滚动容器')
        box.scrollTop = 0
        box.dispatchEvent(new Event('scroll', { bubbles: true }))
        return true
      })()`,
      check: eq(
        `(() => {
           const box = document.querySelector('#run-log-dialog .h-64')
           if (!box) return 'no-box'
           const rows = [...box.querySelectorAll('[data-k]')]
           const nums = rows.map((r) => Number((r.textContent.match(/夹具运行日志 (\\d{4})/) ?? [])[1]))
           const contiguous = nums.length >= 5 && nums.every((n, i) => i === 0 || n === nums[i - 1] + 1)
           return [nums[0], contiguous, rows.length > 0].join('/')
         })()`,
        '1/true/true',
      ),
    },
    {
      name: '历史页：关掉运行日志弹窗（后面几步要用干净的页面）',
      do: `document.getElementById('run-log-dialog').close()`,
      check: eq('document.querySelectorAll("dialog[open]").length', 0),
      changed: true,
    },
    { name: '点 dock：任务', do: clickText('.dock button', '任务'), check: eq('location.pathname', '/tasks') },
    { name: '展开抽屉', do: clickSel('label[for="nav-drawer"]'), check: eq("document.getElementById('nav-drawer').checked", true), changed: true },
    { name: '收起抽屉', do: clickSel('label[for="nav-drawer"]'), check: eq("document.getElementById('nav-drawer').checked", false), changed: true },
    { name: '切换主题', do: clickSel('[aria-label^="切换到"]'), check: eq("!!document.documentElement.getAttribute('data-theme')", true) },
    { name: '再切回主题', do: clickSel('[aria-label^="切换到"]'), check: eq("!!document.documentElement.getAttribute('data-theme')", true) },
    { name: '打开登出确认框', do: clickSel('[aria-label="退出登录"]'), check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'confirm-logout'), changed: true },
    { name: '取消（关掉确认框）', do: clickText('#confirm-logout .modal-action button', '取消'), check: eq('document.querySelectorAll("dialog[open]").length', 0), changed: true },

    // 账号页：**添加账号收在弹窗里**（2026-10-01 重构）—— 先开弹窗，再在弹窗内
    // 切方式、打字。这几条同时盯住"弹窗能开、能关"这条路，以及新的「凭据」方式
    // （pc 前缀要选客户端形态、mobile 前缀不要）。
    // ⚠ 上面那句不是形式：重构前这几步是在**页面常驻表单**里点的，改成弹窗后
    // 若还按老选择器点，就会点在 `dialog:not([open])` 里的不可见元素上 —— 用例照样
    // "过"，但它验的东西已经不是用户能做的事了。
    { name: '切到账号页', do: go('accounts'), check: eq('location.pathname', '/accounts') },
    { name: '账号页：开「添加账号」弹窗', do: clickText('#app button', '添加账号'), check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'add-account'), changed: true },
    { name: '账号页：弹窗里点「短信」方式', do: clickText('#add-account button', '短信'), check: eq('!!document.querySelector("#add-account input[type=tel]")', true), changed: true },
    { name: '账号页：输入手机号', do: typeIn('#add-account input[type="tel"]', '19900000001'), check: eq('document.querySelector("#add-account input[type=tel]").value', '19900000001') },
    { name: '账号页：改手机号', do: typeIn('#add-account input[type="tel"]', '19900000002'), check: eq('document.querySelector("#add-account input[type=tel]").value', '19900000002') },
    { name: '账号页：点「账密」方式', do: clickText('#add-account button', '账密'), check: eq('!!document.querySelector("#add-account input[type=password]")', true), changed: true },
    { name: '账号页：输入用户名', do: typeIn('#add-account input[type="text"].input', 'someone'), check: eq('document.querySelector("#add-account input[type=text].input").value', 'someone') },
    { name: '账号页：输入口令', do: typeIn('#add-account input[type="password"]', 'pw-123456'), check: eq('document.querySelector("#add-account input[type=password]").value', 'pw-123456') },
    { name: '账号页：切到「凭据」方式', do: clickText('#add-account button', '凭据'), check: eq('!!document.querySelector("#add-account textarea")', true), changed: true },
    { name: '账号页：pc 凭据 ⇒ 有形态选择', do: typeIn('#add-account textarea', 'Basic cGM6MTk5MDAwMDAwMDE6YXxifGN8MTc5MzAwMDAwMDAwMHxl'), check: eq('window.__platformBtns("add-account")', 2), changed: true },
    { name: '账号页：mobile 凭据 ⇒ 无形态选择', do: typeIn('#add-account textarea', 'Basic bW9iaWxlOjE5OTAwMDAwMDAxOmF8YnxjfDE3OTMwMDAwMDAwMDB8ZQ=='), check: eq('window.__platformBtns("add-account")', 0), changed: true },
    // 弹窗的出口：标题栏那颗 ✕ 必须存在且尺寸非零（**真**窄屏的"够不够得着"
    // 由 `regress/narrow.mjs` 按 375 宽单独验 —— 这里验不了视口）。
    { name: '账号页：弹窗有可点的出口（✕）', do: null, check: eq('window.__closeBtnInViewport("add-account")', true) },
    { name: '账号页：关掉添加弹窗', do: 'document.getElementById("add-account").close()', check: eq('document.querySelectorAll("dialog[open]").length', 0), changed: true },

    // 编辑弹窗：昵称/设备号预填，凭据框**空**（空 = 不改 —— 凭据从不回显）
    { name: '账号页：点某行的「编辑」', do: clickText('#app ul li.card button', '编辑'), check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'edit-account'), changed: true },
    { name: '账号页：编辑弹窗预填昵称', do: null, check: eq('document.querySelector("#edit-account input[type=text]").value', '主力号') },
    { name: '账号页：编辑弹窗的凭据框是空的', do: null, check: eq('document.querySelector("#edit-account textarea").value', '') },
    { name: '账号页：改昵称', do: typeIn('#edit-account input[type="text"]', '改个名'), check: eq('document.querySelector("#edit-account input[type=text]").value', '改个名') },
    { name: '账号页：关掉编辑弹窗', do: 'document.getElementById("edit-account").close()', check: eq('document.querySelectorAll("dialog[open]").length', 0), changed: true },
    { name: '账号页：点某行的「移除」', do: clickText('button', '移除'), check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'confirm-delete-account'), changed: true },
    { name: '账号页：取消移除（有数据的确认弹窗）', do: clickText('#confirm-delete-account .modal-action button', '取消'), check: eq('document.querySelectorAll("dialog[open]").length', 0), changed: true },
    // 确认删除 ⇒ DELETE 打到 fixture 的 404 ⇒ 走 `showError` 那条反馈路径
    { name: '账号页：确认移除（后端 404 ⇒ 错误弹窗）', do: clickText('#confirm-delete-account .modal-action button', '移除'), check: has('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'error-dialog'), changed: true },
    { name: '账号页：关掉错误弹窗', do: clickText('#error-dialog .modal-action button', '知道了'), check: eq('document.querySelectorAll("dialog[open]").length', 0), changed: true },

    // ⭐⭐ 手动刷新**强制**（2026-10-11 用户口径：「**点击后强制刷新，不检测时间**」✓）：
    // 每账号的「刷新」与工具行的「刷新全部凭据」**都要**在请求体里带 `force: true` ✓。
    //
    // ⚠ 判据取的是**发出去的请求体**（不是"弹窗关了"/"toast 出来了" ✓）——
    // 后者在"漏传 force ⇒ 后端按天数跳过"时**照样成立**（界面同样回一条
    // "未到提前量、跳过"的 toast）⇒ 那样等于没测到这次改动 ✓。
    // ⚠ 装置那侧（`lib.mjs` 的 `POST /api/auth/refresh`）只回一份形状对的结局；
    // "`force: true` 真的越过了天数判据"由 `crates/mcloud-server/tests/routes.rs` 钉 ✓。
    { name: '账号页：点某行的「刷新」⇒ 开确认框', do: clickText('#app ul li.card button', '刷新'), check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'confirm-refresh-one'), changed: true },
    {
      // ⚠⚠ 文案**只问一句**（2026-10-11 用户口径：界面「啥也不说」✓）。
      // 这条同时把**错话**钉死：刷新**不**让上游那串旧的失效 ⇒
      // "作废 / 不可逆 / 撤不回来 / 还剩多少天"这类字**一个都不许**再出现 ✗
      //（旧文案正是那么写的 —— 那是**前提错**，不是措辞重 ✗）。
      name: '账号页：刷新确认框只问一句（不解释会怎样、不说旧的没了）',
      do: null,
      check: eq(
        `(() => {
           const t = document.getElementById('confirm-refresh-one')?.textContent ?? ''
           return [t.includes('确定要刷新'), t.includes('作废'), t.includes('不可逆'), t.includes('撤不回来'), t.includes('还剩多少天')].join('/')
         })()`,
        'true/false/false/false/false',
      ),
    },
    {
      name: '账号页：确认刷新 ⇒ 请求体含 force:true（不是"有反应"就算过）',
      do: `(async () => {
        const posts = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (url === '/api/auth/refresh') posts.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          ;[...document.querySelectorAll('#confirm-refresh-one .modal-action button')]
            .find((b) => b.textContent.trim() === '刷新').click()
          for (let i = 0; i < 80 && posts.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__refreshOnePost = posts.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__refreshOnePost ?? [])[0]
           return p ? p.url + ' ' + JSON.stringify(p.body) : '没发出 /api/auth/refresh 请求'
         })()`,
        '/api/auth/refresh {"account":"13800000000","force":true}',
      ),
      changed: true,
    },
    { name: '账号页：单个刷新后确认框关掉', do: null, check: eq('document.querySelectorAll("dialog[open]").length', 0) },
    { name: '账号页：点「刷新全部凭据」⇒ 开确认框', do: clickText('#app button', '刷新全部凭据'), check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'confirm-refresh-all'), changed: true },
    {
      // ⚠ 同上面那条：「刷新全部」的确认框也只问一句 ✓（错话同样钉死 ✗）。
      name: '账号页：刷新全部确认框也只问一句（不解释会怎样）',
      do: null,
      check: eq(
        `(() => {
           const t = document.getElementById('confirm-refresh-all')?.textContent ?? ''
           return [t.includes('确定要刷新'), t.includes('作废'), t.includes('不可逆'), t.includes('撤不回来'), t.includes('还剩多少天')].join('/')
         })()`,
        'true/false/false/false/false',
      ),
    },
    {
      name: '账号页：确认全部刷新 ⇒ 请求体含 all:true + force:true',
      do: `(async () => {
        const posts = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (url === '/api/auth/refresh') posts.push({ url, init })
          return orig(input, init)
        }
        try {
          ;[...document.querySelectorAll('#confirm-refresh-all .modal-action button')]
            .find((b) => b.textContent.trim() === '全部刷新').click()
          for (let i = 0; i < 80 && posts.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__refreshAllPost = posts.map((p) => ({ url: p.url, body: JSON.parse(p.init.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__refreshAllPost ?? [])[0]
           return p ? p.url + ' ' + JSON.stringify(p.body) : '没发出 /api/auth/refresh 请求'
         })()`,
        '/api/auth/refresh {"all":true,"force":true}',
      ),
      changed: true,
    },
    { name: '账号页：全部刷新后确认框关掉', do: null, check: eq('document.querySelectorAll("dialog[open]").length', 0) },

    // 计划页：新建表单填一遍（文本、下拉、勾选框都覆盖到）
    { name: '切到计划页', do: go('schedules'), check: eq('location.pathname', '/schedules') },
    { name: '计划页：填名称', do: typeIn('input[placeholder="例如 每天签到"]', '每周签到'), check: eq('document.querySelector("input[placeholder=\\"例如 每天签到\\"]").value', '每周签到') },
    { name: '计划页：改 cron', do: typeIn('input[placeholder="0 8 * * *"]', '0 9 * * 1'), check: eq('document.querySelector("input[placeholder=\\"0 8 * * *\\"]").value', '0 9 * * 1') },
    // ⚠ 这条同时盯住"选中的值没被别的绑定回写成空串" —— 上一代对拍里 Vue 侧正是回写成了空
    { name: '计划页：选任务', do: pickOption('#sched-task', 'live-room'), check: eq('document.querySelector("#sched-task").dataset.value', 'live-room') },
    // ⚠ 选择器按 **id** 认（2026-10-08）：表单里多了一排「运行结局条件」勾选框
    //（`ui/push-event-picker.tsx` ✓），它们也是 `input.checkbox` ⇒
    // 用 `input[type=checkbox].checkbox` 会点中第一个**结局**框而不是「启用」✗。
    { name: '计划页：勾选启用', do: clickSel('#sched-enabled'), check: eq('document.querySelector("#sched-enabled").checked', false), changed: true },
    {
      // ⭐ 用户口径（2026-10-02）：「这里账号和任务应该用新的多选框」
      // ⇒ 定时弹窗里**账号**那一栏不再是裸文本输入 ✗，而是与任务页/直播页**同一套**
      //   多选框（`ui/account-picker.tsx` ⇒ 建在 `ui/select.tsx` 上）✓。
      //
      // ⚠ 判据要能区分"换成了多选框"和"那一栏整个没了" ✗ ⇒ 三条一起判：
      //   ① 多选框在 ✓ ② 旧的账号文本输入没了 ✓ ③ 旧文案"逗号分隔"也没了 ✓。
      // ⚠ 顺带钉住"任务那一栏仍是**单选**下拉" ✓（用户 2026-10-02 选的就是这条 ✓）——
      //   免得以后有人顺手把它也改成多选（那要动后端 `Schedule.task` 那个单值字段 ✗）。
      name: '定时页：账号换成新的多选框（不再是"逗号分隔"的文本输入）',
      do: clickText('#app button', '新建定时'),
      check: eq(
        `(() => {
           const d = document.querySelector('dialog#schedule-dialog')
           if (!d || !d.open) return 'not-open'
           const picker = d.querySelector('[aria-label="账号"]')
           const task = d.querySelector('#sched-task')
           const bad = [...d.querySelectorAll('input.input')].filter((i) => (i.placeholder || '').includes('账号'))
           return [!!picker, bad.length === 0, d.textContent.includes('逗号分隔'), task.dataset.multiple === 'true'].join('/')
         })()`,
        'true/true/false/false',
      ),
      changed: true,
    },
    {
      // ⭐ 点开 ⇒ 面板里要有「全部账号」那个**独立开关** ✓
      // （后端口径：空数组 = 我名下全部 ✓，**不是**"把每一项都勾上"✗ —— 勾每一项会把名单锁死）。
      name: '定时页：账号多选框展开后有「全部账号」这一行',
      do: `(() => {
        const t = document.querySelector('dialog#schedule-dialog [aria-label="账号"]')
        if (!t) return '没有账号选择器'
        t.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const p = document.querySelector('.select-portal')
           if (!p) return 'no-panel'
           return p.textContent.includes('全部账号')
         })()`,
        true,
      ),
      changed: true,
    },
    {
      // TDesign 那套浮层的行为之一：**点外部就收起** ✓（面板挂在 `body` 上，不在文档流里 ✓）
      //
      // ⚠ 必须发 **`mousedown`**，不能只发 `click` ✗：`ui/select.tsx` 收面板靠的是
      // `document.addEventListener('mousedown', …, true)` —— 真浏览器里点一下会
      // 先 mousedown 再 click ✓，而合成事件只有 click ⇒ 面板**不会关** ✗
      //（实测就是这么红的：是**测试**用错了事件，不是应用坏了 ✓）。
      name: '定时页：点外面 ⇒ 账号面板收起、弹窗关掉',
      do: `(() => {
        document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
        const d = document.getElementById('schedule-dialog')
        if (d) d.close()
        return true
      })()`,
      check: eq('document.querySelectorAll(".select-portal").length', 0),
      changed: true,
    },
    {
      // ⭐ 推送规则那一格（2026-10-08：**多选** ✓，用户口径「**不是规则去命中行为，
      // 而是行为选择规则，一个行为可以选择多个规则**」）。
      // 装置里这条定时绑的是 `rule-3`（一条**已停用**的规则）⇒ 编辑时那一格必须
      // 显示它绑的是谁、并标出「已停用」；**显示成空就是错的** ✗
      //（那会让用户以为这条定时没绑规则，而实际上它绑着）。
      // ⚠ 顺带钉住它现在是**多选控件**（`data-multiple` ✓）—— 只判"值对"的话，
      // 哪天被改回单选**照样绿** ✗。
      name: '定时页：编辑已绑规则的定时 ⇒ 多选里显示它绑的规则（含「已停用」）',
      do: clickText('#app button', '编辑'),
      check: eq(
        `(() => {
           const t = document.querySelector('#sched-push-rule')
           if (!t) return 'no-select'
           return [t.dataset.multiple, t.dataset.value, t.textContent.includes('已停用')].join('/')
         })()`,
        'true/rule-3/true',
      ),
      changed: true,
    },
    {
      // ⭐⭐ **新增①**：定时绑**两条**规则 ⇒ 保存体里 `pushRules` 是两条。
      // 先选的那条是装置里本来就绑着的 `rule-3`（已停用）✓，再补一条 `rule-1` ✓。
      name: '定时页：再选一条规则（凑两条）',
      do: `(() => {
        const t = document.querySelector('#sched-push-rule')
        if (!t) throw new Error('没有推送规则多选框')
        t.click()
        const opt = [...document.querySelectorAll('.select-portal .select-option')]
          .find((o) => o.textContent.includes('失败才叫我'))
        if (!opt) throw new Error('面板里没有「失败才叫我」那一项')
        opt.click()
        return true
      })()`,
      check: eq(`document.querySelector('#sched-push-rule').dataset.value`, 'rule-3,rule-1'),
      changed: true,
    },
    {
      // ⭐ **新增④**：运行结局勾「失败」⇒ 保存体里 `pushEvents` 只有 `runFailed`。
      // ⚠ 夹具里这条定时的 `pushEvents` 是**空数组**（= 跑完就推 ✓）⇒ 这里才有的可勾 ✓。
      name: '定时页：运行结局勾上「运行失败」',
      do: `(() => {
        const box = document.getElementById('sched-push-events')
        if (!box) throw new Error('没有运行结局勾选框')
        const l = [...box.querySelectorAll('label')].find((x) => x.textContent.includes('运行失败'))
        if (!l) throw new Error('没有「运行失败」那一项')
        l.querySelector('input').click()
        return true
      })()`,
      check: eq(`document.getElementById('sched-push-events').dataset.value`, 'runFailed'),
      changed: true,
    },
    {
      // ⭐⭐ 本组最要紧的一条：**保存体**里两条规则都在、结局条件只有勾中的那一个。
      // ⚠ 判据取的是**请求体**而不是"弹窗关了" —— 后者在"整份写回写歪了"时照样成立 ✗。
      name: '定时页：保存 ⇒ 体里 pushRules 两条、pushEvents 只有 runFailed',
      do: `(async () => {
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          ;[...document.querySelectorAll('#schedule-dialog .modal-action button')]
            .find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__schedPut = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__schedPut ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body ?? {}
           return [
             p.url,
             (b.pushRules ?? []).length,
             (b.pushRules ?? []).join(','),
             JSON.stringify(b.pushEvents),
           ].join('/')
         })()`,
        '/api/schedules/sch-1/2/rule-3,rule-1/["runFailed"]',
      ),
      changed: true,
    },
    {
      name: '定时页：保存后编辑弹窗关掉（列表照旧）',
      do: null,
      check: eq('document.querySelectorAll("dialog[open]").length', 0),
    },
    {
      /*
        ⭐⭐ 2026-10-08 用户口径「所有的提示都是优先弹窗式」的**第二处**正面用例：
        定时弹窗是个长表单（名称 → 任务 → cron → 时区 → 推送规则 → 结局条件 → 启用），
        以前校验失败那句画在**最底下**，而「保存」在弹窗底部 —— 一屏之隔，等于没提示 ✗。
        现在判三样：内联那句在、它在**「名称」那一格之前**（= 弹窗顶部）、toast 文案对得上 ✓。
      */
      name: '定时页：空名称点保存 ⇒ 内联在弹窗顶部（不是表单底部）+ toast',
      do: `(async () => {
        ;[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '新建定时').click()
        await new Promise((r) => setTimeout(r, 300))
        const d = document.getElementById('schedule-dialog')
        if (!d) throw new Error('没打开定时弹窗')
        // ⚠ 名称要**显式清空**：openCreate() 不重置 fName（上一条用例刚填过"每周签到"）——
        //    不清空的话这一下会**真的存下一条定时**（不是本用例要测的东西，
        //    还会给后面的列表计数留下一条多余的记录 ✗）。
        const inp = d.querySelector('input[placeholder="例如 每天签到"]')
        if (!inp) throw new Error('弹窗里没有「名称」那一格')
        inp.value = ''
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        await new Promise((r) => setTimeout(r, 150))
        ;[...d.querySelectorAll('.modal-action button')].find((b) => b.textContent.trim() === '保存').click()
        for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 100))
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('schedule-dialog')
           const alert = d.querySelector('.alert-error')
           const nameInput = d.querySelector('input[placeholder="例如 每天签到"]')
           const toast = [...document.querySelectorAll('#app .toast .alert')].map((x) => x.textContent).join('|')
           return [
             !!alert && alert.textContent.includes('请填写名称'),
             alert && nameInput ? alert.getBoundingClientRect().top < nameInput.getBoundingClientRect().top : false,
             toast.includes('请填写名称'),
           ].join('/')
         })()`,
        'true/true/true',
      ),
      changed: true,
    },
    {
      // 收尾：关掉弹窗 + 收掉那条不自动消失的错误 toast（列表与后续用例不受影响 ✓）
      name: '定时页：关掉刚打开的弹窗与错误 toast',
      do: `(async () => {
        const d = document.getElementById('schedule-dialog')
        ;[...d.querySelectorAll('.modal-action button')].find((b) => b.textContent.trim() === '取消').click()
        await new Promise((r) => setTimeout(r, 200))
        // ⚠ 点**全部** ✕：错误类 toast 不自动消失，一屏最多两条（见 ui/toast.ts）——
        //    只点第一条的话，第二条会一直挂到后面那些用例的截图上 ✗
        ;[...document.querySelectorAll('#app .toast button[aria-label="关闭提示"]')].forEach((b) => b.click())
        await new Promise((r) => setTimeout(r, 200))
        return true
      })()`,
      check: eq(
        `[document.querySelectorAll('dialog[open]').length, document.querySelectorAll('#app .toast .alert').length].join('/')`,
        '0/0',
      ),
      changed: true,
    },
    {
      // ⭐⭐ 那条**系统维护**的定时（「自动直播定时」，载荷里 `managedBy: "live-auto"` ✓）
      // 必须一眼看得出来 + 说明"时刻由系统维护" ✓（用户口径 2026-10-08）。
      //
      // ⚠ 判据取**那一行自己**的徽章，不是整页文本里有这三个字就完事：
      // 整页文本的判据在"徽章长到别的行上"时照样绿 ✗。
      // ⚠ 宽屏是表格（`tr`）、窄屏是卡片（`.grid > .card`）—— 两套 DOM 各取一次，
      // 谁在视口里都认得出 ✓（本脚本默认 1280 ⇒ 走表格那一支 ✓）。
      name: '定时页：那条自动定时带「系统维护」徽章，并说明时刻由系统维护',
      do: null,
      check: eq(
        `(() => {
           const host = [...document.querySelectorAll('#app tr')].find((r) => r.textContent.includes('直播自动定时'))
             ?? [...document.querySelectorAll('#app .grid > .card')].find((c) => c.textContent.includes('直播自动定时'))
           if (!host) return 'no-host'
           const badges = [...host.querySelectorAll('.badge')].map((b) => b.textContent.trim())
           return [badges.includes('系统维护'), host.textContent.includes('时刻由系统维护')].join('/')
         })()`,
        'true/true',
      ),
    },

    // ── 定时页：目标里要有「编排」（用户口径 2026-10-11）────────────────────────
    //
    // 用户原话：「**在定时界面找不到编排任务，虽然可以通过编排界面定时**」——
    // 编排**不是**注册表里的任务（它是一条链 ✓，见 `api.ts` 的 `PIPELINE_TASK`）⇒
    // 它不在 `/api/capabilities` 里，而定时弹窗那个下拉**只列任务** ⇒
    // 定时页压根建不出"跑编排"的定时 ✗（路只通一半：编排页能反向建 ✓）。
    //
    // 这一组把另一半补上，并钉住 **建 / 启停 / 改 / 删** 四条路**都能走** ✓
    // （只钉"下拉里多了个选项"是不够的：建完进不了列表、或者建完删不掉，
    //  用户要的这件事就只做了一半 ✗）。
    {
      name: '定时页：打开新建弹窗（下面几条要用）',
      do: `(async () => {
        ;[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '新建定时').click()
        await new Promise((r) => setTimeout(r, 200))
        return true
      })()`,
      check: eq('document.getElementById("schedule-dialog").open', true),
      changed: true,
    },
    {
      // ⭐ 目标下拉里必须有「编排」那一项（值 = `pipeline`，与后端 `PIPELINE_TASK` 同 ✓）；
      // 选中它之后要多出「要跑的编排」那一格 —— 两件事一起判：
      // 只判"多了一格"的话，那一格长在一个**选不到编排**的页面上照样绿 ✗。
      name: '定时页：目标里能选到「编排」⇒ 多出「要跑的编排」那一格',
      do: pickOption('#sched-task', 'pipeline'),
      check: eq(
        `(() => {
           const t = document.querySelector('#sched-task')
           return [t.dataset.value, !!document.querySelector('#sched-pipeline')].join('/')
         })()`,
        'pipeline/true',
      ),
      changed: true,
    },
    {
      // ⭐⭐ 装置里有**两条**编排（「早班」/「晚班」）—— 这一条钉的是"下拉里列的是
      // **用户的编排**、显示的是**名字**、而且**能挑**" ✓。
      // ⚠ 只有一条的话，「把第一条硬编码进去」那种实现**照样过** ✗
      //（本仓在"装置测不到 = 没测"上栽过）。所以这里**特意挑第二条** ✓。
      name: '定时页：编排下拉列出用户的编排（两条、带名字），并能挑中第二条',
      do: `(() => {
        const t = document.querySelector('#sched-pipeline')
        if (!t) throw new Error('没有「要跑的编排」下拉')
        t.click()
        const opts = [...document.querySelectorAll('.select-portal .select-option')]
        window.__pipeOpts = opts.map((o) => o.dataset.value + ':' + o.textContent.trim()).join(',')
        const hit = opts.find((o) => o.dataset.value === 'pipe-2')
        if (!hit) throw new Error('下拉里没有「晚班」那条编排')
        hit.click()
        return true
      })()`,
      check: eq(
        `[window.__pipeOpts, document.querySelector('#sched-pipeline').dataset.value,
          document.querySelector('#sched-pipeline').textContent.trim()].join(' | ')`,
        'pipe-1:早班,pipe-2:晚班 | pipe-2 | 晚班',
      ),
      changed: true,
    },
    {
      // ⭐⭐ 保存体：`task` 是编排、`body.pipelineId` 是**挑中的那一条**（不是第一条 ✓）。
      // ⚠ 判据取**请求体**：这一条正是"选了编排却把 `body` 留成上一个任务的参数"那个
      // 坑的钉子（后端会回「这条定时要跑编排，但 body 里没有 pipelineId」✗）。
      name: '定时页：建一条编排定时 ⇒ 请求体是 task=pipeline + body.pipelineId=挑中的那条',
      do: `(async () => {
        const inp = document.querySelector('#schedule-dialog input[placeholder="例如 每天签到"]')
        if (!inp) throw new Error('弹窗里没有「名称」那一格')
        inp.value = '晚间编排'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        await new Promise((r) => setTimeout(r, 120))
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          ;[...document.querySelectorAll('#schedule-dialog .modal-action button')]
            .find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__schedPipePut = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
          // 再等一会儿：保存之后还有一次 refresh（列表要跟着更新 ✓）
          await new Promise((r) => setTimeout(r, 600))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__schedPipePut ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body ?? {}
           return [
             p.url.startsWith('/api/schedules/sch-'),
             b.task,
             (b.body ?? {}).pipelineId,
             b.enabled,
           ].join('/')
         })()`,
        'true/pipeline/pipe-2/true',
      ),
      changed: true,
    },
    {
      // ⭐⭐ 建出来的那条**真的进了列表** —— 装置那份定时表是**有状态**的 ✓
      //（以前 `PUT` 只对 `sch-1` 一条做了回显 ⇒ 新建这条路**根本没被接住** ✗）。
      // ⚠ 列表里要能看出**是哪一条编排**（只写"编排"两个字等于没说 ✓）。
      // ⚠ 「启用」这一头取"**没有**「已停用」"：宽屏是表格、表格里**没有**「已启用」
      //    那个徽章（只有停用时会多一行「已停用」✓，见 `schedules-page.tsx` 的两个分支）——
      //    写死判「已启用」在表格形态下必红 ✗。
      name: '定时页：新建的编排定时出现在列表里，并显示绑的是哪一条编排',
      do: null,
      check: eq(
        `(() => {
           const row = [...document.querySelectorAll('#app table tbody tr')].find((r) => r.textContent.includes('晚间编排'))
           if (!row) return '列表里没有新建那条'
           return [row.textContent.includes('编排：晚班'), row.textContent.includes('已停用')].join('/')
         })()`,
        'true/false',
      ),
    },
    {
      // ⭐ **启停**这条路对编排定时必须也能走（2026-10-07 报过的 bug：不带 `body` ⇒
      // 后端回「body 里没有 pipelineId」⇒ **连停用都做不到** ✗）。
      // 判据取请求体：`body` 原样带回、`enabled` 翻成 false ✓。
      name: '定时页：编排定时能停用（请求体里 body 原样带回、enabled 翻成 false）',
      do: `(async () => {
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          const row = [...document.querySelectorAll('#app table tbody tr')].find((r) => r.textContent.includes('晚间编排'))
          if (!row) throw new Error('列表里没有那条编排定时')
          const btn = [...row.querySelectorAll('button')].find((b) => b.textContent.trim() === '停用')
          if (!btn) throw new Error('那一行没有「停用」')
          btn.click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__schedPipeToggle = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
          await new Promise((r) => setTimeout(r, 600))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__schedPipeToggle ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body ?? {}
           const row = [...document.querySelectorAll('#app table tbody tr')].find((r) => r.textContent.includes('晚间编排'))
           return [b.task, (b.body ?? {}).pipelineId, b.enabled, !!row && row.textContent.includes('已停用')].join('/')
         })()`,
        'pipeline/pipe-2/false/true',
      ),
      changed: true,
    },
    {
      // ⭐ **改**这条路：编辑编排定时 ⇒ 「要跑的编排」那一格必须**回填**成它绑的那条
      //（`body.pipelineId` ✓）。显示成空就是错的 —— 用户一保存就把绑定丢了 ✗
      //（静默的数据丢失，这一页最忌讳的一类）。
      name: '定时页：编辑编排定时 ⇒ 编排那一格回填成它绑的那条',
      do: `(() => {
        const row = [...document.querySelectorAll('#app table tbody tr')].find((r) => r.textContent.includes('晚间编排'))
        if (!row) throw new Error('列表里没有那条编排定时')
        const btn = [...row.querySelectorAll('button')].find((b) => b.textContent.trim() === '编辑')
        if (!btn) throw new Error('那一行没有「编辑」')
        btn.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const t = document.querySelector('#sched-pipeline')
           if (!t) return 'no-select'
           const inp = document.querySelector('#schedule-dialog input[placeholder="例如 每天签到"]')
           return [t.dataset.value, t.textContent.trim(), inp ? inp.value : '(没有名称那一格)'].join('/')
         })()`,
        'pipe-2/晚班/晚间编排',
      ),
      changed: true,
    },
    {
      // ⭐ **改**这条路要走完（不只是"回填对"）：在编辑弹窗里**换一条编排**再保存 ⇒
      // 请求体里的 `body.pipelineId` 必须是**换过的那条** ✓，列表跟着显示新的那条 ✓。
      // ⚠ 只钉"回填对"是不够的：保存时若把 `body` 拼回老值（或干脆不带），
      // 用户改了等于没改，而界面看着"保存成功" ✗（最坏的一类）。
      name: '定时页：编辑里换一条编排并保存 ⇒ 请求体换成新那条、列表跟着变',
      do: `(async () => {
        const t = document.querySelector('#sched-pipeline')
        if (!t) throw new Error('没有「要跑的编排」下拉')
        t.click()
        const opt = [...document.querySelectorAll('.select-portal .select-option')]
          .find((o) => o.dataset.value === 'pipe-1')
        if (!opt) throw new Error('下拉里没有「早班」那条编排')
        opt.click()
        await new Promise((r) => setTimeout(r, 250))
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          ;[...document.querySelectorAll('#schedule-dialog .modal-action button')]
            .find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__schedPipeEdit = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
          await new Promise((r) => setTimeout(r, 600))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__schedPipeEdit ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body ?? {}
           const row = [...document.querySelectorAll('#app table tbody tr')].find((r) => r.textContent.includes('晚间编排'))
           return [
             b.task,
             (b.body ?? {}).pipelineId,
             !!row && row.textContent.includes('编排：早班'),
             document.getElementById('schedule-dialog').open,
           ].join('/')
         })()`,
        'pipeline/pipe-1/true/false',
      ),
      changed: true,
    },
    {
      // ⭐ **删**这条路：编排定时是**用户自己建的**（`managedBy` 空 ✓）⇒ 该能删 ✓。
      // 删除是不可逆动作 ⇒ 先弹确认框（本仓的规矩 ✓）。
      name: '定时页：删编排定时 ⇒ 先弹确认框',
      do: `(() => {
        const row = [...document.querySelectorAll('#app table tbody tr')].find((r) => r.textContent.includes('晚间编排'))
        if (!row) throw new Error('列表里没有那条编排定时')
        const btn = [...row.querySelectorAll('button')].find((b) => b.textContent.trim() === '删除')
        if (!btn) throw new Error('那一行没有「删除」')
        btn.click()
        return true
      })()`,
      check: eq('document.getElementById("schedule-delete").open', true),
      changed: true,
    },
    {
      name: '定时页：确认删除 ⇒ 它从列表里消失（装置那份定时表也少一条）',
      do: `(async () => {
        ;[...document.querySelectorAll('#schedule-delete .modal-action button')]
          .find((b) => b.textContent.trim() === '删除').click()
        for (let i = 0; i < 40; i++) await new Promise((r) => setTimeout(r, 100))
        return true
      })()`,
      check: eq(
        `(() => {
           const rows = [...document.querySelectorAll('#app table tbody tr')]
           return [rows.length, rows.filter((r) => r.textContent.includes('晚间编排')).length].join('/')
         })()`,
        '2/0',
      ),
      changed: true,
    },

    // 编排页：步骤列表的**增 / 删 / 改序**（本框架里最容易出错的一块 —— createFor + 位置敏感行）
    { name: '切到编排页', do: go('pipelines'), check: eq('location.pathname', '/pipelines') },
    { name: '编排页：点「编辑」（草稿带 2 步）', do: clickText('button', '编辑'), check: has(APP_TEXT, '共 2 步'), changed: true },
    { name: '编排页：初始顺序 = 签到 → 直播', do: null, check: eq('window.__stepOrder()', 'daily-checkin,live-room') },
    // ⭐⭐ 步骤卡里必须能看到**参数控件**（2026-10-02 重构）。
    // 从前这一块是空的 ✗：步骤的 `body` 只"原样带回、不能编辑"✗ ⇒ 编排里的步骤
    // 永远跑任务自己的默认配置，想改一个参数只能跑去任务页。
    // 实测（重构后）：签到 + 直播两步里共 **4** 个控件（数字框 / 开关）✓。
    // 判据取 `>= 4`：少一个就说明参数表单没渲染（或某个参数的控件类型掉了）✗。
    { name: '编排页：步骤卡里能看到参数控件（表单，不再是只读 body）', do: null, check: eq(
        `(() => { const box = [...document.querySelectorAll('#app div')].find(d => d.className === 'flex flex-col gap-3');
           if (!box) return 'no-box';
           const n = box.querySelectorAll('textarea,input[type=number],input[type=checkbox]').length;
           return n >= 4 ? 'ok' : 'ctl=' + n; })()`,
        'ok') },
    { name: '编排页：第 1 步的「上移」禁用', do: null, check: eq('window.__stepDisabled(0, "上移")', true) },
    { name: '编排页：添加步骤', do: clickText('button', '添加步骤'), check: has(APP_TEXT, '共 3 步'), changed: true },
    { name: '编排页：新步落在末尾', do: null, check: eq('window.__stepOrder()', 'daily-checkin,live-room,') },
    { name: '编排页：第 3 步的「下移」禁用', do: null, check: eq('window.__stepDisabled(2, "下移")', true) },
    // 把第 2 步提到最前 ⇒ 步号必须跟着位置走（positional 行重建），不能留在原节点上
    { name: '编排页：第 2 步上移', do: 'window.__stepBtn(1, "上移").click()', check: eq('window.__stepOrder()', 'live-room,daily-checkin,'), changed: true },
    { name: '编排页：重排后步号仍按位置', do: null, check: eq('window.__stepBadges()', '第1步,第2步,第3步') },
    { name: '编排页：移除末尾那步', do: 'window.__stepBtn(2, "移除").click()', check: has(APP_TEXT, '共 2 步'), changed: true },
    {
      // ⭐ 编排上那一格（与定时上那一格**不是同一处**：后端是两张表、两条记录）。
      // 装置里 `pipe-1` 绑的是 `rule-1` ⇒ 编辑时多选框要显示「失败才叫我」，
      // 显示成空就是错的 ✗。同样钉住它是**多选**（`data-multiple` ✓）。
      name: '编排页：编辑已绑规则的编排 ⇒ 多选里显示它绑的规则',
      do: null,
      check: eq(
        `(() => {
           const t = document.querySelector('#pipe-push-rule')
           if (!t) return 'no-select'
           return [t.dataset.multiple, t.dataset.value, t.textContent.includes('失败才叫我')].join('/')
         })()`,
        'true/rule-1/true',
      ),
    },
    {
      // ⭐ **新增④的另一半**：结局条件的**回填**那条路 —— 装置里 `pipe-1` 存的是
      // `['runFailed','runOk']` ⇒ 那排勾选框必须把这**两个**勾上 ✓。
      // ⚠ 只判"勾选框在"是不够的：把回填写成"一律全不勾"照样绿 ✗。
      name: '编排页：结局条件把存着的两个勾上（runFailed / runOk）',
      do: null,
      check: eq(
        `(() => {
           const box = document.getElementById('pipe-push-events')
           if (!box) return 'no-picker'
           const on = [...box.querySelectorAll('input[type=checkbox]')]
             .filter((i) => i.checked)
             .map((i) => i.parentElement.textContent.trim())
           return [box.dataset.value, on.join(',')].join('/')
         })()`,
        'runFailed,runOk/运行失败,运行成功',
      ),
    },
    {
      // ⚠ 只画**运行类**：面板行为（登录/性能）没有定时可绑 ⇒ 出现在这里只会误导 ✗
      // （用户口径：那四类在「设置 → 事件推送」里 ✓）。
      name: '编排页：结局条件里没有登录/性能那几类（只有运行类）',
      do: null,
      check: eq(
        `(() => {
           const box = document.getElementById('pipe-push-events')
           if (!box) return 'no-picker'
           const t = box.textContent
           return [box.querySelectorAll('input[type=checkbox]').length, /登录|耗时|连续失败/.test(t)].join('/')
         })()`,
        '4/false',
      ),
    },
    { name: '编排页：取消（回列表，不发请求）', do: clickText('#app .card-actions button', '取消'), check: not(APP_TEXT, '共 2 步'), changed: true },

    // 直播页：口令 textarea、时长数字框、开关
    { name: '切到直播页', do: go('live-room'), check: eq('location.pathname', '/live-room') },
    { name: '直播页：输入口令', do: typeIn('textarea', '口令一 口令二'), check: eq('document.querySelector("textarea").value', '口令一 口令二') },
    { name: '直播页：改时长', do: typeIn('input[type="number"]', '45'), check: eq('document.querySelector("input[type=number]").value', '45') },
    { name: '直播页：切开关', do: clickSel('input[type="checkbox"].toggle'), check: eq('document.querySelector("input[type=checkbox].toggle").checked', true), changed: true },
    {
      // ⭐ 开关的**开/关必须一眼分得出来**（2026-10-02 用户口径：颜色差别太小）。
      // 断言用"同一颗开关开态 vs 关态的**计算后背景色不同**"，不去比对具体色值 ——
      // 色值会随主题变，而"两者必须不同"这条不变式不该跟着变。
      // 关态用一个临时克隆量，避免把页面上的开关真拨回去（那会污染后面的步骤）。
      name: '直播页：开关"开"与"关"的背景色不同（不是只差旋钮位置）',
      do: null,
      check: eq(
        `(() => {
           const b = document.querySelector('#app input[type=checkbox].toggle');
           if (!b) return 'no-toggle';
           const on = getComputedStyle(b).backgroundColor;
           const c = b.cloneNode(true);
           c.checked = false;
           b.parentElement.appendChild(c);
           const off = getComputedStyle(c).backgroundColor;
           c.remove();
           return on !== off && on !== 'rgba(0, 0, 0, 0)';
         })()`,
        true,
      ),
    },
    // ── 共享口令池卡片（2026-10-01：口令从"页面参数"变成"全实例共享资源"）──
    {
      name: '直播页：池卡片显示有效条数',
      do: null,
      check: eq(
        `(() => { const t = document.getElementById('app').textContent; return t.includes('共享口令池') + '/' + t.includes('2 条有效') })()`,
        'true/true',
      ),
    },
    {
      // 装置里放了两条有效 ⇒ 界面必须明说"不必再抓"（这是用户口径的核心一句）
      name: '直播页：够 2 条有效 ⇒ 明说不需要再抓',
      do: null,
      check: has(APP_TEXT, '全场都不需要再抓口令'),
    },
    {
      // 下一场读的是**预告场**的 expectStartTime —— 而判定门只认 status==1，
      // 所以这个时间能显示出来，就证明"日程没跟着门一起丢"（Go 参考实现的坑）。
      //
      // ⚠⚠ 2026-10-02 用户口径：「下一场直播时间应该移动到上面，显眼」
      // ⇒ **位置本身成了判据**：它必须在「运行参数」卡片**之前** ✓。
      // 只断言"页面里有这行字"是不够的 ✗ —— 它原先就埋在池卡片最底下、
      // 照样能被 `includes` 查到 ✓（这正是那条断言漏掉的形状 ✗）。
      name: '直播页：下一场在顶部（排在「运行参数」之前）且带主播名与场次名',
      do: null,
      check: eq(
        `(() => {
           const cards = Array.from(document.querySelectorAll('.card'));
           const iNext = cards.findIndex((c) => c.textContent.includes('下一场直播'));
           const iForm = cards.findIndex((c) => c.textContent.includes('运行参数'));
           const t = document.getElementById('app').textContent;
           return [
             iNext >= 0,
             iForm >= 0,
             iNext < iForm,
             t.includes('2026-09-23 14:30:00'),
             t.includes('中国移动云盘'),
             t.includes('云盘AI助职场加速！看直播赢好礼！'),
           ].join('/');
         })()`,
        'true/true/true/true/true/true',
      ),
    },
    {
      // 场次列表：进行中的那条要写"还在播"，结束的那条要有结束时刻。
      name: '直播页：显示观测到的场次（含"还在播"）',
      do: null,
      check: has(APP_TEXT, '还在播'),
    },
    {
      // 过期的那条必须**看得出来**是无效、且原因在（"过期视同无效，但原因留着"）
      name: '直播页：过期口令显示为无效并带上原因',
      do: null,
      check: eq(
        `(() => { const t = document.getElementById('app').textContent; return t.includes('过期的老口令') + '/' + t.includes('口令已过期') })()`,
        'true/true',
      ),
    },

    // ── 直播页：「自动直播定时」那张卡（用户口径 2026-10-08）────────────────────
    //
    // 这张卡只让用户改两件事（**开关**与**账号多选**），时刻由后端算 ✓。
    // 四条钉子：① 卡在、显示算出的时刻；② 关开关 ⇒ 体里 `enabled:false` 且**回填**成已停用；
    // ③ 选两个账号 ⇒ 体里 `accounts` 两个；④「没有下一场」那一态写"已停用"而不是空白 ✓。
    //
    // ⚠ 判据一律取 **PUT 的请求体** + **回填后的界面**两样：只判"点了有反应"的话，
    // "体里发错值"与"拿本地草稿当结果（没回填）"都照样绿 ✗。
    {
      // ① 卡在，且**显示后端算出的那一刻** —— 夹具那份 `at` 是 `2026-10-08 11:00:00`
      //（后端口径：下一场 10:00 + 1 小时 ✓）。
      // ⚠ 顺带钉住那句说明（时刻怎么来的 / 到点干什么）：它是这张卡唯一
      // "用户不点任何东西也要读到"的东西 ✓。
      name: '直播页：有「自动直播定时」卡，显示算出的时刻与说明句',
      do: null,
      check: eq(
        `(() => {
           const t = document.getElementById('app').textContent
           return [t.includes('自动直播定时'), t.includes('2026-10-08 11:00:00'),
                   t.includes('下一场直播开始 + 1 小时'), t.includes('检测口令 + 领取')].join('/')
         })()`,
        'true/true/true/true',
      ),
    },
    {
      // ② 关开关 ⇒ `PUT` 体里 `enabled:false`，且界面**拿返回的那份**回填成已停用 ✓。
      // ⚠ 判据里的 `box.checked` 是**回填后**的开关（后端回 `enabled:false` ✓）——
      // 只看请求体的话，"发对了但界面没回填"照样绿 ✗。
      name: '直播页：关掉自动定时开关并保存 ⇒ 体里 enabled:false，界面回填成已停用',
      do: `(async () => {
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          document.getElementById('auto-live-enabled').click()
          document.getElementById('auto-live-save').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          // 等响应回来：回填用的就是它（不等的话下一步读到的还是旧界面）
          await new Promise((r) => setTimeout(r, 400))
          window.__autoPut = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__autoPut ?? [])[0]
           if (!p) return '没发出 PUT'
           const box = document.getElementById('auto-live-enabled')
           const card = box.closest('.card')
           return [p.url, p.body.enabled, box.checked, card.textContent.includes('已停用')].join('/')
         })()`,
        '/api/live-room/auto-schedule/false/false/true',
      ),
      changed: true,
    },
    {
      // ③ 账号多选：把「全部账号」那一行**取消** ⇒ 名单变成**逐个勾上**（两个都在）✓。
      // ⚠ 后端口径：空数组 = 全部已启用账号 ⇒ 取消"全部"必须落成**显式名单**，
      // 留空的话与"全部"是同一个值、用户会以为没生效 ✗（与定时页同一条处置 ✓）。
      name: '直播页：取消「全部账号」⇒ 名单变成两个账号都在',
      do: `(() => {
        const t = document.getElementById('auto-live-accounts')
        if (!t) throw new Error('没有自动定时那个账号选择器')
        t.click()
        const all = document.querySelector('.select-panel .select-option')
        if (!all) throw new Error('面板里没有「全部账号」那一行')
        all.click()
        // 关掉浮层：下一步要点保存（真浏览器里点保存也会先关它 ✓）
        document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
        return true
      })()`,
      check: eq(
        `document.getElementById('auto-live-accounts').dataset.value`,
        '13800000000,13900000001',
      ),
      changed: true,
    },
    {
      // ③′ 保存 ⇒ 体里 `accounts` 是**那两个**（不是空数组）✓，且界面回填成同一份 ✓。
      // ⚠ 这里**不判 `changed`**：开关与名单都没变（上一趟刚改过），界面本来就不该动 ✓。
      name: '直播页：保存 ⇒ 体里 accounts 是两个账号（不是空数组）',
      do: `(async () => {
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          document.getElementById('auto-live-save').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          await new Promise((r) => setTimeout(r, 400))
          window.__autoPut = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__autoPut ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body ?? {}
           return [p.url, (b.accounts ?? []).length, (b.accounts ?? []).join(','),
                   document.getElementById('auto-live-accounts').dataset.value].join('/')
         })()`,
        '/api/live-room/auto-schedule/2/13800000000,13900000001/13800000000,13900000001',
      ),
    },
    {
      // ④ 准备「没有下一场」那一态：打开开关（②里关掉了）+ 只留**第二个**账号
      //（夹具按账号给下一场：主力号有、第二个号没有 ✓ ⇒ 算不出 ⇒ 后端停用它 ✓）。
      name: '直播页：把自动定时开关打开（准备验「没有下一场」那一态）',
      do: `document.getElementById('auto-live-enabled').click()`,
      check: eq(`document.getElementById('auto-live-enabled').checked`, true),
      changed: true,
    },
    {
      name: '直播页：账号只留第二个（第一个号没有下一场）',
      do: `(() => {
        const t = document.getElementById('auto-live-accounts')
        if (!t) throw new Error('没有账号选择器')
        t.click()
        const opt = [...document.querySelectorAll('.select-panel .select-option')]
          .find((o) => o.dataset.value === '13800000000')
        if (!opt) throw new Error('面板里没有主力号那一项')
        opt.click()
        document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
        return true
      })()`,
      check: eq(`document.getElementById('auto-live-accounts').dataset.value`, '13900000001'),
      changed: true,
    },
    {
      // ④′ ⭐ 本组最要紧的一条：**算不出下一场**时，界面必须写"已停用"、并把后端那句
      // 人话（note ✓）原样显示 —— **留空就是坑** ✗（用户分不清"还没算"与"算出来是空"）。
      // ⚠ 同时钉住"旧时刻**不再**显示"与"开关被回填成关" ✓（后端回 `enabled:false` ✓）。
      name: '直播页：算不出下一场 ⇒ 界面写「已停用」+ note，不留空白',
      do: `(async () => {
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          document.getElementById('auto-live-save').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          await new Promise((r) => setTimeout(r, 400))
          window.__autoPut = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const box = document.getElementById('auto-live-enabled')
           const card = box.closest('.card')
           const t = card.textContent
           const p = (window.__autoPut ?? [])[0]
           return [t.includes('已停用'), t.includes('还没算出下一场'),
                   t.includes('2026-10-08 11:00:00'), box.checked,
                   (p && (p.body.accounts ?? []).join(',')) || '没发出 PUT'].join('/')
         })()`,
        'true/true/false/false/13900000001',
      ),
      changed: true,
    },

    // 任务页：账号选择器（2026-10-01 起是「多选 + 全部账号」；2026-10-02 起换成
    // TDesign 口径的自建下拉 `ui/select.tsx`）。
    // 选项同样是**由数据 createFor 出来的**，点它等于测"列表项 + 勾选回写"。
    //
    // 选择器结构：面板里 `.select-option`，**第 0 项**是「全部账号」（独立开关），
    // 第 1 项起才是逐个账号；面板挂在 `body`（或弹窗里）的 `.select-portal` 上。
    { name: '切到任务页', do: go('tasks'), check: eq('location.pathname', '/tasks') },
    {
      name: '任务页：选账号',
      do: `(() => {
        document.querySelector('#account-picker').click()
        const items = [...document.querySelectorAll('.select-panel .select-option')]
        items[1].click()
        return true
      })()`,
      check: eq(
        `(() => {
          const items = [...document.querySelectorAll('.select-panel .select-option')]
          return items[1].getAttribute('aria-selected') === 'true' &&
                 items[0].getAttribute('aria-selected') === 'false' &&
                 document.querySelector('#account-picker').dataset.value.length > 0
        })()`,
        true,
      ),
    },
    {
      // 「全部账号」是**独立开关**（后端口径 `accounts: []` = 我名下全部），
      // 勾上时逐个账号的勾选框会禁用 —— 这里锁住这个语义。
      name: '任务页：勾「全部账号」⇒ 摘要变成全部账号，逐个勾选被禁用',
      do: `(() => {
        document.querySelectorAll('.select-panel .select-option')[0].click()
        return true
      })()`,
      check: eq(
        `(() => {
          const items = [...document.querySelectorAll('.select-panel .select-option')]
          const trigger = document.querySelector('#account-picker').textContent
          return items[0].getAttribute('aria-selected') === 'true' &&
                 items[1].className.includes('select-option-off') &&
                 trigger.includes('全部账号')
        })()`,
        true,
      ),
      changed: true,
    },
    {
      // ⚠ 分两步，且第二步**不要再点**。
      //
      // ① 取消「全部账号」那一刻，逐个账号的勾选框**还是 disabled** 的
      //    （框架的属性写回与这次点击在同一个 tick），紧接着点它等于点了个禁用控件；
      // ② 更要紧的是：`picked`（逐个勾选那份状态）**没被"全部"清掉** ——
      //    前面已经勾过第一个号了，所以取消「全部」之后它**本来就是选中的**。
      //    这时再点一次等于取消勾选，后面的运行用例就会变成"没选账号"。
      //    ⇒ 这一步只**断言**状态，不做动作。
      name: '任务页：取消「全部账号」',
      do: `(() => {
        document.querySelectorAll('.select-panel .select-option')[0].click()
        return true
      })()`,
      check: eq(
        `(() => {
          const items = [...document.querySelectorAll('.select-panel .select-option')]
          return items[0].getAttribute('aria-selected') === 'false' &&
                 !items[1].className.includes('select-option-off')
        })()`,
        true,
      ),
    },
    {
      name: '任务页：取消「全部账号」后，逐个勾选的那份状态还在（第一个号仍选中）',
      do: null,
      check: eq(
        `(() => {
          const items = [...document.querySelectorAll('.select-panel .select-option')]
          const trigger = document.querySelector('#account-picker').textContent
          return items[0].getAttribute('aria-selected') === 'false' &&
                 items[1].getAttribute('aria-selected') === 'true' &&
                 !trigger.includes('全部账号')
        })()`,
        true,
      ),
    },

    /**
     * ── 设置页：顶部四节 tab（2026-10-08 第三次口径）────────────────────────
     *
     * 用户原话：「**设置页面内容太多了，也没有分页面**」✓ ⇒ 八张卡拆成四节
     * （服务 / 安全 / 事件推送 / 关于 ✓），**卡的顺序与内容一个字没改**，只是换了位置 ✓。
     *
     * 这一组钉四件事（⚠ 全是"结构性"的判据，不是"页面里有没有这几个字"）：
     * ① 四节 tab 都在 ✓，**老地址 `/settings` 落在第一节（服务）** ✓（侧栏 / dock /
     *    书签 / 浏览器历史走的都是它，落到空节就是把用户的东西弄丢 ✗）；
     * ② 点「安全」⇒ **地址变 `/settings/security`**、且服务那几张卡**整个不在** ✓
     *    —— 只判"安全卡在"的话，四节全渲染、各用 CSS 藏一份照样绿 ✗；
     * ③ 点回「服务」⇒ 服务那几张回来、安全那几张不在（切得回去 ✓）；
     * ④ 直接开 `/settings/security` ⇒ 落在安全节 ✓（书签 / 刷新 / 后退走的就是这条）。
     *
     * ⚠ 下面凡是钉"某一节的某张卡"的步骤，都**先切到那一节** ✓（这一页从 2026-10-08 起
     * 一次只渲染一节）。
     */
    { name: '切到设置页', do: go('settings'), check: eq('location.pathname', '/settings'), changed: true },
    {
      name: '设置页：四节 tab 都在（服务 / 安全 / 事件推送 / 关于），老地址 /settings 落在「服务」节',
      do: null,
      check: eq(SETTINGS_TAB_STATE, '服务,安全,事件推送,关于 | 服务 | /settings'),
    },
    {
      // ⭐ 服务节装的是原来那三张：版本/服务信息 · 存储信息 · 服务操作 ✓（顺序照旧 ✓）。
      // ⚠ 服务卡与关于卡的标题都带产品名 —— 服务卡那个 h2 里还嵌着版本徽章
      //（`.card-title` 的 textContent 是 `mcloudv0.1.0` ✓），关于卡那个只有 `mcloud` ✓
      // ⇒ 区分这两节靠的是**后面那几张卡**（存储信息 / 服务操作 vs 更新说明）✓。
      name: '设置页：服务节装的是 服务信息 · 存储信息 · 服务操作（顺序照旧）',
      do: null,
      check: eq(SETTINGS_CARDS, 'mcloudv0.1.0,存储信息,服务操作'),
    },
    { name: '设置页：存储信息报到数据目录', do: null, check: has(APP_TEXT, '数据目录') },
    /**
     * ── 存储信息卡：**日志与历史的容量上限**（2026-10-09 用户口径
     *    「日志和历史不要全加入内存，使用本地记录，**让用户设置最大大小**」✓）──────
     *
     * 钉五件事：
     * ① 两份日志占用都写成「**占用 / 上限**」（夹具：运行日志 556 KiB / 64 MB ✓、
     *    服务端日志 1.2 MB / 64 MB ✓）—— 只报占用的话，用户看不出"还剩多少、
     *    该不该调大"，而那是他打开这张卡要回答的问题 ✗；
     * ② **五个**框按**服务端那份状态**回填（200 / 2000 / 64 / 2000 / 64 ✓），
     *    且两格新说明写着"内存 vs 磁盘"（调小内存那格**不丢历史** ✓ —— 说反了
     *    会让人以为"调小 = 丢日志" ✗）；
     * ③ 越界先被**前端**挡住（弹窗 + 就近 inline，且不发请求 ✓）；
     * ④ 改完保存 ⇒ 五个框回填服务端的值、**两份**「占用 / 上限」都跟着变 ✓。
     *
     * ⚠ ④ 里"跟着变"那一半是**关键**：两个 `maxBytes` 在装置那边分别是
     * `limits.logTotalMb` / `limits.serverLogTotalMb × 1 MiB` **现算**的
     * （`lib.mjs::storageView` ✓）⇒ 页面若只把用户输的数在本地回显、
     * 没真发出去/没读响应，那两行不会变 ✗。
     * ⚠ 两份读数用的是**不同的数**（556 KiB/139 个文件 vs 1.2 MB/12 个文件 ✓）——
     * 同一个数的话，"界面把两份搞混了"照样绿 ✗。
     */
    {
      name: '设置页：两份日志占用都写成「占用 / 上限」（556 KB / 64 MB、1.2 MB / 64 MB），五个上限框都在',
      do: null,
      check: eq(
        `(() => {
           const card = [...document.querySelectorAll('#app .card')]
             .find((c) => (c.querySelector('.card-title')?.textContent ?? '').trim() === '存储信息')
           if (!card) return 'no-card'
           const t = card.textContent
           return [t.includes('556 KB / 64 MB'), t.includes('139 个文件'),
                   t.includes('1.2 MB / 64 MB'), t.includes('12 个文件'),
                   // 两格新说明的**要义**：内存那格讲清"更早的按需从磁盘读"（不丢历史 ✓）、
                   // 磁盘那格讲清淘汰粒度是"整天文件" ✓
                   t.includes('更早的按需从磁盘读'), t.includes('整天文件'),
                   document.querySelectorAll('#app input[id^=storage-]').length].join('/')
         })()`,
        'true/true/true/true/true/true/5',
      ),
    },
    {
      name: '设置页：五个上限按服务端的值回填（200 / 2000 / 64 / 2000 / 64）',
      do: null,
      check: eq(
        `[document.getElementById('storage-history-limit').value,
          document.getElementById('storage-log-per-run').value,
          document.getElementById('storage-log-total-mb').value,
          document.getElementById('storage-server-log-mem-lines').value,
          document.getElementById('storage-server-log-total-mb').value].join('/')`,
        '200/2000/64/2000/64',
      ),
    },
    {
      // ⚠ 越界先在前端挡住：装置那边越界会回 400（照真后端校验 ✓）⇒ 前端没挡的话
      // 这一步会看到错误弹窗、卡片上也不会出现范围提示，两条断言一起红 ✓。
      name: '设置页：上限越界 ⇒ 前端先挡（错误弹窗 + 就近 inline）',
      do: `(() => {
        const el = document.getElementById('storage-history-limit')
        el.value = '5'
        el.dispatchEvent(new Event('input', { bubbles: true }))
        ;[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '保存上限').click()
        return true
      })()`,
      check: eq(
        `(() => {
           const card = [...document.querySelectorAll('#app .card')]
             .find((c) => (c.querySelector('.card-title')?.textContent ?? '').trim() === '存储信息')
           return [document.querySelectorAll('dialog[open]').length,
                   [...document.querySelectorAll('dialog[open]')].map((d) => d.id).join(','),
                   card.textContent.includes('20~2000')].join('/')
         })()`,
        '1/error-dialog/true',
      ),
      changed: true,
    },
    {
      name: '设置页：关掉错误弹窗',
      do: clickText('#error-dialog .modal-action button', '知道了'),
      check: eq('document.querySelectorAll("dialog[open]").length', 0),
      changed: true,
    },
    {
      /**
       * ⭐⭐ 「保存体对得上」：装置**照冻结的契约**校验（**给哪格改哪格**、一格都不给才 400、
       * 越界回 400 + 人话 ✓）⇒ 这一步能走到"成功提示"，就说明页面确实把用户输的数
       * **发出去了** ✓。
       *
       * ⚠⚠ **五格一起改**是刻意的（不只是"改一格"）：装置那边只覆盖**给了的**那几格 ✓
       * ⇒ 页面若只发改过的那一格（或漏发几格），响应回读之后**没发的那几格会跳回旧值**，
       * 下面那五条读数当场红 ✓ —— 这正是"少发一格"这类 bug 唯一能现形的时机 ✓。
       *
       * ⚠ 断言里那五条一起读是**故意的**：只判"有成功提示"的话，
       * "本地回显 + 假成功"照样绿 ✗；只判五个框的值的话，那本来就是本地草稿 ✗。
       */
      name: '设置页：改五个上限 ⇒ 保存成功、回填服务端的值、两份占用/上限那行都跟着变',
      do: `(() => {
        const set = (id, v) => { const el = document.getElementById(id); el.value = v; el.dispatchEvent(new Event('input', { bubbles: true })) }
        set('storage-history-limit', '500')
        set('storage-log-per-run', '3000')
        set('storage-log-total-mb', '128')
        set('storage-server-log-mem-lines', '5000')
        set('storage-server-log-total-mb', '256')
        ;[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '保存上限').click()
        return true
      })()`,
      check: eq(
        `(() => {
           const card = [...document.querySelectorAll('#app .card')]
             .find((c) => (c.querySelector('.card-title')?.textContent ?? '').trim() === '存储信息')
           const t = card ? card.textContent : ''
           return [document.getElementById('storage-history-limit').value,
                   document.getElementById('storage-log-per-run').value,
                   document.getElementById('storage-log-total-mb').value,
                   document.getElementById('storage-server-log-mem-lines').value,
                   document.getElementById('storage-server-log-total-mb').value,
                   t.includes('556 KB / 128 MB'),
                   t.includes('1.2 MB / 256 MB'),
                   (document.getElementById('app').textContent ?? '').includes('存储上限已保存')].join('/')
         })()`,
        '500/3000/128/5000/256/true/true/true',
      ),
      changed: true,
    },
    {
      name: '设置页：点「安全」⇒ 地址变 /settings/security，只有安全那三张卡在',
      do: clickText('#app [role="tablist"] button', '安全'),
      check: eq(
        `[${SETTINGS_TAB_STATE}, ${SETTINGS_CARDS}].join(' # ')`,
        '服务,安全,事件推送,关于 | 安全 | /settings/security # 修改管理员口令,口令策略,会话控制',
      ),
      changed: true,
    },
    {
      // ⚠ 反方向也要钉：服务节那两张**必须整个不在**（不是被 CSS 藏着 ✓）——
      // 只判"安全卡在安全节里"的话，"四节全渲染"那种写法照样绿 ✗。
      name: '设置页：安全节里没有服务那两张卡（结构性，不是藏起来）',
      do: null,
      check: eq(
        `${SETTINGS_CARD_LIST}.filter((x) => x === '存储信息' || x === '服务操作').length`,
        0,
      ),
    },
    {
      // 用户口径 2026-10-01：「你口令更改怎么取消了旧口令」⇒ 当前口令必须留着。
      // 三个框 = 当前口令 + 新口令 + 确认（1 个 current-password + 2 个 new-password）。
      name: '设置页：改口令是三个框（当前 + 新 + 确认）',
      do: null,
      check: eq(
        `document.querySelectorAll('#app input[autocomplete="current-password"]').length + '/' +
         document.querySelectorAll('#app input[autocomplete="new-password"]').length`,
        '1/2',
      ),
    },
    {
      name: '设置页：两次新口令不一致 ⇒ 前端先挡（只提示、不发请求）',
      do: `(() => {
        const set = (el, v) => { el.value = v; el.dispatchEvent(new Event('input', { bubbles: true })) }
        // ⚠ 当前口令也要填：不填的话先撞上"请填写当前口令"，测不到"两次不一致"这一条
        set(document.querySelector('#app input[autocomplete="current-password"]'), 'current-pass')
        const f = document.querySelectorAll('#app input[autocomplete="new-password"]')
        set(f[0], 'first-pass')
        set(f[1], 'second-pass')
        ;[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '更新口令').click()
        return f.length
      })()`,
      check: has(APP_TEXT, '两次输入的新口令不一致'),
      changed: true,
    },
    {
      // ⭐ 「关于」是**第四节**（2026-10-08 拆 tab ✓）—— 下面两条关于卡的断言得先切过来。
      name: '设置页：点「关于」⇒ 地址变 /settings/about，只有关于那一张卡',
      do: clickText('#app [role="tablist"] button', '关于'),
      check: eq(
        `[${SETTINGS_TAB_STATE}, ${SETTINGS_CARDS}].join(' # ')`,
        '服务,安全,事件推送,关于 | 关于 | /settings/about # mcloud',
      ),
      changed: true,
    },
    {
      // 版权声明：版本号来自 /api/version（fixture 是 0.1.0），年份是当年
      name: '设置页：关于卡有版权声明（带许可与年份）',
      do: null,
      // ⚠ 不能写 `APP_TEXT`：它只是 `has()` 用的**字面量占位**，在 `eq()` 里是个
      // 未定义标识符 —— 症状是"断言不符"而不是报错，很容易看错方向。这里写全。
      check: eq(
        `/Copyright © ${new Date().getFullYear()} catlair · Apache-2\.0/.test(document.getElementById('app').textContent)`,
        true,
      ),
    },
    {
      // 更新说明这块：列的是**这次构建带了什么**（装置给了两条）
      name: '设置页：关于卡列出更新说明（来自构建记录）',
      do: null,
      check: eq(
        `/feat\\(accounts\\): 账号停用/.test(document.getElementById('app').textContent)`,
        true,
      ),
    },
    {
      /**
       * ⭐⭐ 新增③：**直接开 `/settings/security` ⇒ 落在「安全」节** ✓。
       *
       * 这条走的是"书签 / 刷新 / 浏览器历史"那条路（`gotoJs` 就是 `navigate()` 的模拟 ✓）：
       * tab 由**地址**决定（`watch(currentPath)` ✓），所以地址一变界面就跟着变 ✓ ——
       * 写成"本地 `ref` 记住上一次点的哪个"的话，这里会先落在服务节（地址与界面不一致 ✗）。
       * ⚠ 顺带把「安全」那三张卡也钉一遍：这一节是**切过来**的，不是"进页时碰巧在" ✓。
       */
      name: '设置页：直接开 /settings/security ⇒ 落在「安全」节（书签 / 刷新走的就是它）',
      do: go('settings/security'),
      check: eq(
        `[${SETTINGS_TAB_STATE}, ${SETTINGS_CARDS}].join(' # ')`,
        '服务,安全,事件推送,关于 | 安全 | /settings/security # 修改管理员口令,口令策略,会话控制',
      ),
      changed: true,
    },
    {
      name: '设置页：保存策略 ⇒ 有成功提示',
      do: clickText('button', '保存策略'),
      check: has(APP_TEXT, '策略已保存'),
      changed: true,
    },
    {
      /**
       * ⭐⭐ 新增②：**点回「服务」⇒ 服务那几张在、安全那几张不在** ✓。
       * 判据两头都判（服务那三张的**顺序** + 安全那三张一个都不在 ✓）——
       * 只判前者的话，"四节全渲染"照样绿 ✗。
       */
      name: '设置页：点回「服务」⇒ 服务那几张在、安全那几张不在（切得回去）',
      do: clickText('#app [role="tablist"] button', '服务'),
      check: eq(
        `[${SETTINGS_TAB_STATE}, ${SETTINGS_CARDS}, ${SETTINGS_CARD_LIST}.filter((x) => x === '修改管理员口令' || x === '口令策略' || x === '会话控制').length].join(' # ')`,
        '服务,安全,事件推送,关于 | 服务 | /settings # mcloudv0.1.0,存储信息,服务操作 # 0',
      ),
      changed: true,
    },
    /**
     * ⭐⭐ 2026-10-08 用户口径「**并单独给推送一个**」⇒ 那两块**整块搬去「推送」页** ✓。
     *
     * ⚠ 这条钉的是"设置页**没有**它们"，而不只是"推送页有"：只钉后者的话，
     * 把卡在两边各留一份照样绿 ✗ —— 而"两个入口迟早分叉"正是这次要拆掉的东西 ✓。
     * ⚠ 判据同时要求这一页**真的渲染出来了**（下面那串卡片标题在），否则"没有推送卡"
     * 在**整页错误态**下同样成立 ⇒ 一条空转 ✗（本文件里栽过这种空转，见账号配置页那条）。
     */
    {
      name: '设置页：**没有**「推送渠道」/「推送规则」两块（整块搬去「推送」页，不是藏起来）',
      do: null,
      check: eq(
        `(() => {
           const titles = [...document.querySelectorAll('#app .card')]
             .map((c) => (c.querySelector('.card-title')?.textContent ?? '').trim())
           const t = document.getElementById('app').textContent
           return [
             titles.some((x) => x === '推送渠道' || x === '推送规则'),
             t.includes('启用推送'),
             !!document.querySelector('#app #push-min-level'),
             titles.includes('推送渠道'),
           ].join('/')
         })()`,
        'false/false/false/false',
      ),
    },
    {
      // ⭐ 第三节就是「事件推送」那一节（用户口径：**留在设置里当一节** ✓）——
      // 下面那一大段事件推送的断言都得先切过来 ✓。
      name: '设置页：点「事件推送」⇒ 地址变 /settings/events，只有事件推送那一张卡',
      do: clickText('#app [role="tablist"] button', '事件推送'),
      check: eq(
        `[${SETTINGS_TAB_STATE}, ${SETTINGS_CARDS}, ${SETTINGS_CARD_LIST}.filter((x) => x === '存储信息' || x === '修改管理员口令').length].join(' # ')`,
        '服务,安全,事件推送,关于 | 事件推送 | /settings/events # 事件推送 # 0',
      ),
      changed: true,
    },
    {
      /**
       * ⭐ 2026-10-08（同日第二次口径）：设置页**新增「事件推送」卡** ——
       * 用户原话「在**设置**里，单独为这种**面板行为**增加一个可配置推送规则的入口」✓。
       *
       * 判据要**四行都在**（登录新设备 / 登录失败 / 耗时异常 / 连续失败 ✓），
       * 而且要写明"这几类没有定时可绑" ✓ —— 只判"卡在"的话，
       * 一张只画了一行的卡照样绿 ✗。
       */
      name: '设置页：有「事件推送」卡，四行都在、写明"没有定时可绑"',
      do: null,
      check: eq(
        `(() => {
           const card = [...document.querySelectorAll('#app .card')]
             .find((c) => (c.querySelector('.card-title')?.textContent ?? '').trim() === '事件推送')
           if (!card) return 'no-card'
           const t = card.textContent
           // ⚠ 行标题读的是**后端事件清单**里的 label（kind === 'panel' 那几条 ✓）——
           // 所以这里用的必须是后端那两句措辞（'WebUI 新登录' / '登录连续失败' ✓），
           // 不是"行为"那一侧 BehaviorKey::label() 的措辞 ✗。
           // ⚠⚠ 本段在**模板字符串里面**：绝不能出现反引号（踩过 —— 一个反引号就把模板
           // 提前结束，整个文件 SyntaxError，门禁只报一句"抛错" ✗）。
           const rows = ['WebUI 新登录', '登录连续失败', '耗时异常', '连续失败'].filter((x) => t.includes(x))
           return [rows.length, t.includes('没有定时可绑'), card.querySelectorAll('[id^=panel-rules-]').length].join('/')
         })()`,
        '4/true/4',
      ),
    },
    {
      /**
       * ⭐ 节流**三个控件**每行都要有（触发方式 / 冷却 / 每日上限 ✓），
       * 而且默认值是**保守口径**（用户口径：「登录，性能，这些可能频繁触发，注意」✓）：
       * 登录那两行 `every`、性能那两行 `onChange`，冷却 30 分钟、上限 10 条 ✓。
       * ⚠ 只判"控件在"是不够的：默认值写成"不冷却 / 不限"照样绿 ✗。
       */
      name: '设置页：事件推送每行三个节流控件，默认值是保守口径（含 onChange / 30 分钟 / 10 条）',
      do: null,
      check: eq(
        `(() => {
           const keys = ['loginNew', 'loginFailed', 'perfSlow', 'perfFailStreak']
           const modes = keys.map((k) => document.getElementById('panel-mode-' + k)?.dataset.value ?? 'no')
           const cool = document.querySelectorAll('#app input[aria-label$="冷却分钟"]').length
           const caps = document.querySelectorAll('#app input[aria-label$="每日上限"]').length
           const one = document.getElementById('panel-cooldown-loginFailed')?.value
           const cap = document.getElementById('panel-cap-loginFailed')?.value
           return [modes.join(','), cool, caps, one, cap].join('/')
         })()`,
        'every,every,onChange,onChange/4/4/30/10',
      ),
    },
    {
      /**
       * ⭐ 「耗时异常多一个**倍数**、连续失败多一个**条数**」（用户口径 ✓）——
       * 而且**只有这两行**有（别的行为带一格阈值没有意义 ✓）。
       * ⚠ 两格阈值是可空的：`null` = **用户没动过** ⇒ 界面必须**留空**（预填一个 3
       * 就是拿默认值冒充用户的选择 ✗）。夹具里 `perfSlow` 是 2.5、`perfFailStreak` 是 4
       * ⇒ 这两格要**回填**出来 ✓。
       */
      name: '设置页：事件推送的倍数/条数两格只在性能那两行，且把存着的值回填出来',
      do: null,
      check: eq(
        `(() => {
           const slow = document.querySelector('input[aria-label="耗时异常倍数"]')
           const fail = document.querySelector('input[aria-label="连续失败条数"]')
           const n = document.querySelectorAll('input[aria-label$="倍数"],input[aria-label$="条数"]').length
           return [n, slow ? slow.value : 'no-slow', fail ? fail.value : 'no-fail'].join('/')
         })()`,
        '2/2.5/4',
      ),
    },
    {
      /**
       * ⭐ 「恢复正常那一条永远会发」这句**必须在卡里**（用户口径的硬要求 ✓）：
       * 不说的话用户会以为设备/任务一直坏着 ✗ —— 冷却与上限只挡重复告警 ✓。
       */
      name: '设置页：事件推送卡写明「恢复正常那一条永远会发」',
      do: null,
      check: eq(
        `(() => {
           const card = [...document.querySelectorAll('#app .card')]
             .find((c) => (c.querySelector('.card-title')?.textContent ?? '').trim() === '事件推送')
           if (!card) return 'no-card'
           return [card.textContent.includes('恢复正常那一条永远会发'), card.textContent.includes('不受它们影响')].join('/')
         })()`,
        'true/true',
      ),
    },
    {
      /**
       * ⭐⭐ **八张卡一张没丢** —— 拆 tab 最容易出的事故就是"某张卡跟着旧位置一起没了" ✗，
       * 而它在**单节**的断言里看不出来（一次只渲染一节 ✓）。
       *
       * 所以这条把四节**挨个点一遍**、把每一节的 `.card-title` 收下来，一次判完：
       * 四节的卡**分别是哪几张**（顺序也在里面 ✓）＋ **加起来还是 8 张** ✓。
       * 分配（用户口径 ✓）：服务 = 服务信息 · 存储信息 · 服务操作；
       * 安全 = 修改管理员口令 · 口令策略 · 会话控制；事件推送 = 事件推送；关于 = 关于 ✓。
       *
       * ⚠ 点击放在 `do` 里、断言放在 `check` 里（`check` **不许改页面**：它跑在快照之后，
       * 改了会让下一步的"界面变了没有"误判 ✗）。收尾停在「关于」节，下一步就切去推送页了 ✓。
       * ⚠ 两端标题都是产品名（`mcloud`）—— 服务节与关于节靠**后面那几张**区分 ✓。
       */
      name: '设置页：四节各装哪几张卡，加起来还是 8 张（一张没丢）',
      do: `(async () => {
        const read = () => [...document.querySelectorAll('#app .card')]
          .map((c) => (c.querySelector('.card-title')?.textContent ?? '').trim())
        const out = {}
        for (const label of ['服务', '安全', '事件推送', '关于']) {
          const b = [...document.querySelectorAll('#app [role="tablist"] button')]
            .find((x) => x.textContent.trim() === label)
          if (!b) throw new Error('没有 tab：' + label)
          b.click()
          await new Promise((r) => setTimeout(r, 120))
          out[label] = read()
        }
        window.__settingsTabs = out
        return true
      })()`,
      check: eq(
        `(() => {
           const m = window.__settingsTabs ?? {}
           const keys = ['服务', '安全', '事件推送', '关于']
           const lists = keys.map((k) => (m[k] ?? []).join(','))
           const total = keys.reduce((n, k) => n + (m[k] ?? []).length, 0)
           return lists.join(' / ') + ' / 共 ' + total + ' 张'
         })()`,
        'mcloudv0.1.0,存储信息,服务操作 / 修改管理员口令,口令策略,会话控制 / 事件推送 / mcloud / 共 8 张',
      ),
      changed: true,
    },

    // ── 「推送」页（独立一页，2026-10-08 用户口径「并单独给推送一个」）────────
    //
    // ⚠⚠ 2026-10-08 同一天**改了两回**：
    // ① 先「推翻重做」（用户口径「我设置一个推送规则，**这个规则配置推送的 token，
    //    包括推送哪几个平台**」✓）：**规则自带凭据** ⇒ 原来那张独立的「推送渠道」卡
    //    **删掉** ✗（`GET/PUT /api/push/config` 也没了 ✗），渠道表单**内嵌**在规则编辑器里 ✓。
    // ② 再按用户口径「可以参考这种布局，**左边是规则，右边是配置**」把"表格 + 弹窗编辑器"
    //    **整个换成账号页那套 master-detail** ✗：左栏 `#push-rail`（规则列表 + 新建 + 每行删除 ✓）、
    //    右栏 `#push-detail`（选中规则的完整配置 + 右上「放弃修改 / 保存」✓）。
    //    ⚠ 弹窗**编辑器**不要了；删除确认那个 `#push-rule-delete` **留着**（用户口径「删前确认」✓）。
    // ③ 静默时段整块删掉 ✗（用户口径「**不要静默时间**」）—— 那两格时间选择与说明都没了 ✓，
    //    请求体里也不该再有 `quiet` ✓。
    { name: '切到推送页', do: go('push'), check: eq('location.pathname', '/push'), changed: true },
    {
      /**
       * ⭐ 新布局的两根锚：左栏 `#push-rail`、右栏 `#push-detail` 都要在。
       * ⚠ 同时钉住**旧布局真的没了** —— 表格（`#app table`）与弹窗编辑器
       * （`#push-rule-dialog`）都不该再出现 ✗（只判"新布局在"的话，两套并存照样绿 ✗）。
       * ⚠ 这一步**不设 `changed`**：它只等页面加载完（`go` 那一步已经算变了 ✓）。
       */
      name: '推送页：新布局就位（左栏 #push-rail + 右栏 #push-detail；旧的表格 / 弹窗编辑器都没了）',
      do: `(async () => {
        for (let i = 0; i < 40; i++) {
          if (document.querySelector('#push-rail') && document.querySelector('#push-detail')) return true
          await new Promise((r) => setTimeout(r, 100))
        }
        return true
      })()`,
      check: eq(
        `(() => {
           return [
             !!document.getElementById('push-rail'),
             !!document.getElementById('push-detail'),
             document.querySelectorAll('#app table').length,
             !!document.getElementById('push-rule-dialog'),
           ].join('/')
         })()`,
        'true/true/0/false',
      ),
    },
    {
      /**
       * ⭐ 左栏三行，每行三样都要真的画出来（名字 / 平台摘要 / 启用状态 ✓）。
       * 夹具三条**故意三种形态**：① 两个平台 + 启用；② 一个平台（Bark）+ 启用；③ 两个平台 + **停用**。
       * ⚠ 只判条数的话，"平台摘要写成死文案""启用状态不画"都照样绿 ✗。
       */
      name: '推送页：左栏列出 3 条规则，每行都有名字 / 平台摘要 / 启用状态',
      do: null,
      check: eq(
        `(() => {
           const rail = document.getElementById('push-rail')
           if (!rail) return 'no-rail'
           const rows = [...rail.querySelectorAll('button')].filter((b) => /失败才叫我|夜里别吵|什么都推/.test(b.textContent))
           const txt = rows.map((r) => r.textContent)
           return [
             rows.length,
             txt.some((x) => x.includes('失败才叫我') && x.includes('PushPlus') && x.includes('企业微信机器人') && x.includes('启用')),
             txt.some((x) => x.includes('夜里别吵') && x.includes('Bark') && x.includes('启用')),
             txt.some((x) => x.includes('什么都推') && x.includes('钉钉机器人') && x.includes('停用')),
           ].join('/')
         })()`,
        '3/true/true/true',
      ),
    },
    {
      /**
       * ⭐⭐ 「左边规则、右边配置」最要紧的一条：**点左栏 ⇒ 右栏换成它**。
       * ⚠ 判据取 `#push-detail` 里的**名称输入框**（不是"整页文字里有 夜里别吵" ——
       * 左栏那一行本来就有它，只判文字等于什么都没测 ✗）。
       * ⚠ 这一步点的是**第二条**（进页时默认选中的是第一条 ⇒ 点它才**真的会变** ✓；
       * 点第一条的话界面与进页时一模一样，`changed` 会误报"点了但不动"✗）。
       */
      name: '推送页：点左栏「夜里别吵」⇒ 右栏出现它的配置（名称回填、Bark 一张卡片）',
      do: `(() => {
        const b = [...document.querySelectorAll('#push-rail button')].find((x) => x.textContent.includes('夜里别吵'))
        if (!b) throw new Error('左栏没有「夜里别吵」那一行')
        b.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           if (!d) return 'no-detail'
           const name = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').trim() === '名称')?.querySelector('input')
           const det = [...d.querySelectorAll('details')]
           return [
             d.querySelector('.card-title')?.textContent?.trim(),
             name ? name.value : 'no-name',
             det.length,
             (det[0]?.querySelector('summary')?.textContent ?? '').includes('Bark'),
           ].join('/')
         })()`,
        '夜里别吵/夜里别吵/1/true',
      ),
      changed: true,
    },
    {
      /**
       * ⭐ 再点第一条：**token 默认打码**（读接口刻意不打码 ⇒ 界面必须当密文 ✓），
       * 而且两个平台卡片都在 ✓。
       */
      name: '推送页：点左栏「失败才叫我」⇒ 右栏出现它的配置，PushPlus token 默认打码',
      do: `(() => {
        const b = [...document.querySelectorAll('#push-rail button')].find((x) => x.textContent.includes('失败才叫我'))
        if (!b) throw new Error('左栏没有「失败才叫我」那一行')
        b.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           if (!d) return 'no-detail'
           const pp = [...d.querySelectorAll('details')].find((x) => (x.querySelector('summary')?.textContent ?? '').includes('PushPlus'))
           const inp = pp?.querySelector('input')
           window.__credMasked = inp ? inp.type : 'no-input'
           return [
             d.querySelector('.card-title')?.textContent?.trim(),
             d.querySelectorAll('details').length,
             window.__credMasked,
           ].join('/')
         })()`,
        '失败才叫我/2/password',
      ),
      changed: true,
    },
    {
      // ⭐ 打码防的是**站在屏幕边上的人**，不是自己 ⇒ 查看必须**一键可达** ✓。
      name: '推送页：点「显示」后 token 明文（打码一键可达）',
      do: `(() => {
        const d = document.getElementById('push-detail')
        const pp = [...d.querySelectorAll('details')].find((x) => (x.querySelector('summary')?.textContent ?? '').includes('PushPlus'))
        if (!pp) throw new Error('右栏没有 PushPlus 那张卡片')
        pp.open = true
        const btn = [...pp.querySelectorAll('button')].find((b) => b.textContent.trim() === '显示')
        if (!btn) throw new Error('PushPlus 卡片里没有「显示」按钮')
        btn.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           const pp = [...d.querySelectorAll('details')].find((x) => (x.querySelector('summary')?.textContent ?? '').includes('PushPlus'))
           const inp = pp?.querySelector('input')
           return [window.__credMasked, inp.type, inp.value].join('/')
         })()`,
        'password/text/fixture-pushplus-token',
      ),
      changed: true,
    },
    {
      /**
       * ⭐⭐ 本组最要紧的一条：改一处保存 ⇒ **真的**走 `PUT /api/push/rules/{id}`，
       * 而且 `config` 里**两个平台都在**（整份写回 ⇒ 少一个就是抹掉用户没碰过的凭据 ✗）。
       * ⚠ 顺带钉住：请求体里**没有** `quiet` ✗（静默时段整块删了 ✓）、
       * 也**不该**再有 `events` / `channels` ✗（它们归**行为**那一侧 ✓）。
       */
      name: '推送页：改标题并保存 ⇒ PUT /api/push/rules/rule-1，config 里两个平台都在、没有 quiet',
      do: `(async () => {
        const d = document.getElementById('push-detail')
        const box = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').includes('推送标题'))
        const inp = box?.querySelector('input')
        if (!inp) throw new Error('右栏里没有「推送标题」那一格')
        inp.value = '门禁改过的标题'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        await new Promise((r) => setTimeout(r, 200))
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          const save = [...d.querySelectorAll('button')].find((b) => b.textContent.trim() === '保存')
          if (!save) throw new Error('右栏右上没有「保存」')
          save.click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__rulePutA = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__rulePutA ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body ?? {}
           const c = b.config ?? {}
           return [
             p.url,
             c.title,
             !!c.pushplus,
             !!c.workWeixinBot,
             'quiet' in b,
             'events' in b,
             'channels' in b,
           ].join('/')
         })()`,
        '/api/push/rules/rule-1/门禁改过的标题/true/true/false/false/false',
      ),
      changed: true,
    },
    {
      // ⭐⭐ 上面那条只证明"发出去的体对"；这条证明**装置真的存下来了**：
      // 点 navbar 的刷新（页面重新 `GET /api/push/rules` ✓）之后右栏还是刚存的那份 ✓。
      // ⚠ 不刷新就断言是**不够**的：PUT 的响应被原样回填也能让界面显示新标题，
      // 而装置里那份可能压根没存 ✗（与 `pushRulesState` 那条同一个理由 ✓）。
      // ⚠ 也**不设 `changed`**：刷新后这一页与刷新前**一模一样**（那正是"刷新成功"的意思 ✓）。
      name: '推送页：刷新后右栏「推送标题」还是刚存的那份（PUT 真的进了装置）',
      do: clickSel('[aria-label="刷新当前页"]'),
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           const box = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').includes('推送标题'))
           return box?.querySelector('input')?.value ?? 'no-input'
         })()`,
        '门禁改过的标题',
      ),
    },
    {
      /**
       * ⭐ 「放弃修改」要真的能回到上次保存的那一份。
       * ⚠ 不设 `changed`：改了又放弃，**净效果是没变** ⇒ 拿"界面变了"当判据会误报 ✗
       * （判据是"改过之后值确实回到了存的那一份" ✓）。
       */
      name: '推送页：改标题后点「放弃修改」⇒ 回到刚存的那一份',
      do: `(async () => {
        const d = document.getElementById('push-detail')
        const box = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').includes('推送标题'))
        const inp = box?.querySelector('input')
        if (!inp) throw new Error('右栏里没有「推送标题」那一格')
        inp.value = '不要保存这个'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        await new Promise((r) => setTimeout(r, 150))
        const back = [...d.querySelectorAll('button')].find((b) => b.textContent.trim() === '放弃修改')
        if (!back) throw new Error('右栏右上没有「放弃修改」')
        if (back.disabled) throw new Error('「放弃修改」是禁用的（脏判定没生效）')
        back.click()
        await new Promise((r) => setTimeout(r, 150))
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           const box = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').includes('推送标题'))
           return box?.querySelector('input')?.value ?? 'no-input'
         })()`,
        '门禁改过的标题',
      ),
    },
    {
      // ⭐ 汇总方式**这一版可改**（旧表格页只在列里显示 ✗）—— 改成「每一步一条」
      // ⇒ 请求体里 `digest: "perStep"` ✓。
      name: '推送页：汇总方式改成「每一步一条」并保存 ⇒ PUT 体里 digest=perStep',
      do: `(async () => {
        const sel = document.getElementById('push-digest')
        if (!sel) throw new Error('右栏没有汇总方式那一格')
        sel.click()
        const opt = [...document.querySelectorAll('.select-portal .select-option')].find((o) => o.textContent.includes('每一步一条'))
        if (!opt) throw new Error('面板里没有「每一步一条」')
        opt.click()
        await new Promise((r) => setTimeout(r, 200))
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          const d = document.getElementById('push-detail')
          ;[...d.querySelectorAll('button')].find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__rulePutB = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__rulePutB ?? [])[0]
           if (!p) return '没发出 PUT'
           return [p.url, p.body?.digest, p.body?.enabled].join('/')
         })()`,
        '/api/push/rules/rule-1/perStep/true',
      ),
      changed: true,
    },
    {
      // ⭐ 「仅错误 / 最低级别 / 启用」几格都是**这条规则自己的** —— 切一条设过的规则
      // ⇒ 控件必须显示**存的那一份** ✓（也证明"切规则真的换了一整份表单"✓）。
      name: '推送页：点左栏「什么都推（先停用）」⇒ 右栏回填它自己的那份（仅错误关、级别 debug、启用关）',
      do: `(() => {
        const b = [...document.querySelectorAll('#push-rail button')].find((x) => x.textContent.includes('什么都推'))
        if (!b) throw new Error('左栏没有「什么都推」那一行')
        b.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           const l = [...d.querySelectorAll('label')].find((x) => x.textContent.includes('只在出错时推'))
           const t = l?.querySelector('input[type=checkbox]')
           const sel = document.getElementById('push-min-level')
           const en = [...d.querySelectorAll('label')].find((x) => x.textContent.includes('启用这条规则'))
           const enBox = en?.querySelector('input[type=checkbox]')
           return [
             t ? t.checked : 'no-toggle',
             sel ? sel.dataset.value : 'no-select',
             enBox ? enBox.checked : 'no-enable',
           ].join('/')
         })()`,
        'false/debug/false',
      ),
      changed: true,
    },

    // ── 新建 / 删除（整条闭环）────────────────────────────────────────────
    {
      // ⭐ 新建 ⇒ **右栏出现空配置**（名称空、平台一个都没配 ✓）。
      name: '推送页：点「新建规则」⇒ 右栏出现空配置（名称空、0 个平台卡片）',
      do: clickText('#push-rail button', '新建规则'),
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           if (!d) return 'no-detail'
           const name = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').trim() === '名称')?.querySelector('input')
           return [
             d.querySelector('.card-title')?.textContent?.trim(),
             name ? name.value : 'no-name',
             d.querySelectorAll('details').length,
           ].join('/')
         })()`,
        '新建推送规则//0',
      ),
      changed: true,
    },
    {
      // ⭐ 一个平台都没配就点保存 ⇒ **本地拦下**，不该打一枪再回错误 ✓
      name: '推送页：新建规则一个平台都不配就保存 ⇒ 本地拦下（不发 PUT）',
      do: `(async () => {
        const d = document.getElementById('push-detail')
        const box = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').trim() === '名称')
        const inp = box?.querySelector('input')
        if (!inp) throw new Error('右栏里没有「名称」那一格')
        inp.value = '门禁建的规则'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        await new Promise((r) => setTimeout(r, 150))
        window.__noPut = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') window.__noPut.push(url)
          return orig(input, init)
        }
        try {
          ;[...d.querySelectorAll('button')].find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r, 100))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      /*
        ⚠⚠ 2026-10-08 改判据（用户口径「所有的提示都是优先弹窗式」）：
        原来只断言"右栏文本里有那句红字" —— 而它当时画在**表单最底部**，
        用户点右上角的「保存」之后根本看不到（他截图报的就是这个 ✗）。
        现在三样一起判：
        ① 那句原文仍在（就近的内联）；
        ② 它出现在**「名称」那一格之前**（= 卡片标题下面，而不是表单底部）——
           用 `getBoundingClientRect().top` 比位置，比"页面里有这行字"强得多 ✗；
        ③ **toast 也出现了**，且文案对得上（弹窗优先那一半 ✓）。
      */
      check: eq(
        `(() => {
           const d = document.getElementById('push-detail')
           const alert = [...d.querySelectorAll('.alert-error')].find((a) => a.textContent.includes('一个平台都没配'))
           const nameBox = [...d.querySelectorAll('fieldset')].find((f) => (f.querySelector('legend')?.textContent ?? '').trim() === '名称')
           const nameInput = nameBox?.querySelector('input')
           const toast = [...document.querySelectorAll('#app .toast .alert')].map((x) => x.textContent).join('|')
           return [
             (window.__noPut ?? []).length,
             !!alert,
             alert && nameInput ? alert.getBoundingClientRect().top < nameInput.getBoundingClientRect().top : false,
             toast.includes('一个平台都没配'),
           ].join('/')
         })()`,
        '0/true/true/true',
      ),
      changed: true,
    },
    {
      // 错误类 toast **不自动消失**（`ui/toast.ts` 的口径：那句话要读完才能决定下一步）⇒
      // 门禁里也得自己收掉，否则它会一直挂在后面那些用例的截图上。
      // 顺带把「✕ 关得掉」这条也钉住 ✓。
      name: '推送页：错误 toast 得手动关（点 ✕ 才消失）',
      do: `(async () => {
        ;[...document.querySelectorAll('#app .toast button[aria-label="关闭提示"]')].forEach((b) => b.click())
        await new Promise((r) => setTimeout(r, 200))
        return true
      })()`,
      check: eq(`document.querySelectorAll('#app .toast .alert').length`, 0),
      changed: true,
    },
    {
      // ⭐⭐ 真保存一次：装置那份规则表是**有状态**的（`lib.mjs` 里 PUT 按前缀匹配），
      // 所以这条能证明"建一条 → 左栏多一条"整条路 ✓（只测右栏画得对是不够的）。
      // ⚠ 也钉住"凭据真的进了请求体"（`config.bark.key` ✓）。
      name: '推送页：加一个 Bark 平台、填上 key 再保存 ⇒ 体里 config.bark.key 对得上、左栏变 4 条',
      do: `(async () => {
        const d = document.getElementById('push-detail')
        const add = [...d.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Bark')
        if (!add) throw new Error('右栏没有「+ 加平台」里的 Bark')
        add.click()
        await new Promise((r) => setTimeout(r, 200))
        const det = [...d.querySelectorAll('details')].find((x) => (x.querySelector('summary')?.textContent ?? '').includes('Bark'))
        if (!det) throw new Error('加了 Bark 之后没有它的卡片')
        det.open = true
        const inp = det.querySelector('input')
        if (!inp) throw new Error('Bark 卡片里没有输入框')
        inp.value = 'fixture-new-bark-key'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        await new Promise((r) => setTimeout(r, 200))
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          ;[...d.querySelectorAll('button')].find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__rulePutC = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__rulePutC ?? [])[0]
           if (!p) return '没发出 PUT'
           const c = p.body?.config ?? {}
           const rail = document.getElementById('push-rail')
           const rows = [...rail.querySelectorAll('button')].filter((b) => /失败才叫我|夜里别吵|什么都推|门禁建的规则/.test(b.textContent))
           return [
             p.body?.name,
             c.bark?.key,
             (p.url ?? '').startsWith('/api/push/rules/rule-'),
             rows.length,
           ].join('/')
         })()`,
        '门禁建的规则/fixture-new-bark-key/true/4',
      ),
      changed: true,
    },
    {
      // ⚠ 删除前必须说清**影响面** —— 只说"确定吗"等于没确认。
      // 这条删的是刚建的、**没绑**的规则 ⇒ 文案要走"没绑、没有别的东西受影响"那一支 ✓
      name: '推送页：删除刚建的（没绑的）规则 ⇒ 确认框说清"没有别的东西受影响"',
      do: `(() => {
        const rail = document.getElementById('push-rail')
        const row = [...rail.querySelectorAll('button')].find((b) => b.textContent.includes('门禁建的规则'))
        const wrap = row?.closest('div')
        const del = wrap ? [...wrap.querySelectorAll('button')].find((b) => b.textContent.trim() === '删除') : null
        if (!del) throw new Error('「门禁建的规则」那一行没有删除键')
        del.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('push-rule-delete')
           if (!d || !d.open) return 'no-dialog'
           return d.textContent.includes('没有绑到任何定时 / 编排 / 事件推送上')
         })()`,
        true,
      ),
      changed: true,
    },
    {
      name: '推送页：确认删除 ⇒ 左栏回到 3 条',
      do: clickText('#push-rule-delete .modal-action button', '删除'),
      check: eq(
        `(() => {
           const rail = document.getElementById('push-rail')
           return [...rail.querySelectorAll('button')].filter((b) => /失败才叫我|夜里别吵|什么都推|门禁建的规则/.test(b.textContent)).length
         })()`,
        3,
      ),
      changed: true,
    },
    {
      // ⭐ 另一支文案：删一条**绑着**的规则 —— 必须点名"绑着谁"、并说清
      // 删掉只是"**少这一条规则**"（一个行为可以绑多条 ✓，不是"不推了"✗）。
      // 这条只**开框看文案**、最后取消，不真删（后面的步骤还要用这条规则 ✓）。
      name: '推送页：删除一条绑着的规则 ⇒ 确认框点名"绑着谁"并说清只是少这一条',
      do: `(() => {
        const rail = document.getElementById('push-rail')
        const row = [...rail.querySelectorAll('button')].find((b) => b.textContent.includes('失败才叫我'))
        const wrap = row?.closest('div')
        const del = wrap ? [...wrap.querySelectorAll('button')].find((b) => b.textContent.trim() === '删除') : null
        if (!del) throw new Error('「失败才叫我」那一行没有删除键')
        del.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const d = document.getElementById('push-rule-delete')
           if (!d || !d.open) return 'no-dialog'
           return [
             d.textContent.includes('编排「早班」'),
             d.textContent.includes('事件「连续失败」'),
             d.textContent.includes('会少这一条规则'),
           ].join('/')
         })()`,
        'true/true/true',
      ),
      changed: true,
    },
    {
      name: '推送页：取消删除 ⇒ 规则还在（3 条）',
      do: clickText('#push-rule-delete .modal-action button', '取消'),
      check: eq(
        `(() => {
           const rail = document.getElementById('push-rail')
           return [...rail.querySelectorAll('button')].filter((b) => /失败才叫我|夜里别吵|什么都推|门禁建的规则/.test(b.textContent)).length
         })()`,
        3,
      ),
      changed: true,
    },

    {
      // ⚠ 重启那两块**留在设置页**（它们与推送无关 ✓）—— 上面这一大段在「推送」页，
      //   所以先切回去，否则下面两步会找不到「重启服务」按钮 ✗。
      name: '切回设置页（重启那两块留在这一页）',
      do: go('settings'),
      check: eq('location.pathname', '/settings'),
      changed: true,
    },
    {
      name: '设置页：重启按钮开的是确认框（不是直接重启）',
      do: clickText('button', '重启服务'),
      check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'confirm-restart'),
    },
    {
      name: '设置页：取消重启 ⇒ 框关掉、服务没动',
      do: clickText('#confirm-restart .modal-action button', '取消'),
      check: eq('!!document.querySelector("dialog#confirm-restart[open]")', false),
    },
    {
      /**
       * ⚠ 下面那一大段是「事件推送」**那一节**里的控件（2026-10-08 拆 tab 之后，
       * 它默认不在屏幕上 ✗）—— 不切过去的话每一条都会"找不到元素"，
       * 而那种红看着像"功能坏了"，其实是**用例没切 tab** ✗。
       * ⚠ 顺带钉住"重启那一节没被这次切换弄丢"：`#confirm-restart` 是**页级**的
       * （不在任何一节里 ✓），切节不影响它的确认框与 `restarting` 状态 ✓。
       */
      name: '设置页：切到「事件推送」节（下面那一大段在这一节里）',
      do: clickText('#app [role="tablist"] button', '事件推送'),
      check: eq(SETTINGS_TAB_STATE, '服务,安全,事件推送,关于 | 事件推送 | /settings/events'),
      changed: true,
    },

    /**
     * ⭐⭐ **新增②③**：设置页那个「事件推送」入口**真的能配**（用户口径：「在**设置**里，
     * 单独为这种**面板行为**增加一个可配置推送规则的入口」✓）。
     *
     * 这里钉三件事：
     * ① 给「登录失败」**选一条规则** ✓；
     * ② 把它的**冷却**改成 5 分钟 ✓；
     * ③ 把**触发方式**改成「只在状态变化时推」（新增③ ⇒ 体里 `mode: "onChange"` ✓）。
     * ⚠ 判据取的是**请求体**（`PUT /api/push/panel` ✓），不是"点了有反应"——
     * 后者在"整份写回写歪了"时照样成立 ✗。
     * ⚠ 而且必须**刷新后读得回来**：夹具那份是**有状态**的（`pushPanelState` ✓），
     * 只信 PUT 的响应回填是测不出"真的存了"的 ✗。
     */
    {
      name: '设置页：事件推送「登录失败」那一行选上一条规则',
      do: `(() => {
        const t = document.getElementById('panel-rules-loginFailed')
        if (!t) throw new Error('没有「登录失败」的规则多选框')
        t.click()
        const opt = [...document.querySelectorAll('.select-portal .select-option')]
          .find((o) => o.textContent.includes('失败才叫我'))
        if (!opt) throw new Error('面板里没有「失败才叫我」那一项')
        opt.click()
        return true
      })()`,
      check: eq(`document.getElementById('panel-rules-loginFailed').dataset.value`, 'rule-1'),
      changed: true,
    },
    {
      name: '设置页：事件推送「登录失败」冷却改成 5 分钟',
      do: `(() => {
        const inp = document.getElementById('panel-cooldown-loginFailed')
        if (!inp) throw new Error('没有「登录失败 冷却分钟」那一格')
        inp.value = '5'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        return true
      })()`,
      check: eq(`document.getElementById('panel-cooldown-loginFailed').value`, '5'),
      changed: true,
    },
    {
      // ⭐ **新增③**：选「只在状态变化时推」⇒ 体里 `mode: "onChange"` ✓。
      // 装置里 `loginFailed` 的默认是 `every`（保守默认：登录失败本来就少见 ✓）
      // ⇒ 这里改它才有意义 ✓。
      name: '设置页：事件推送「登录失败」触发方式改成「只在状态变化时推」',
      do: `(() => {
        const sel = document.getElementById('panel-mode-loginFailed')
        if (!sel) throw new Error('没有触发方式下拉')
        sel.click()
        const opt = [...document.querySelectorAll('.select-portal .select-option')]
          .find((o) => o.textContent.includes('只在状态变化时推'))
        if (!opt) throw new Error('面板里没有「只在状态变化时推」那一项')
        opt.click()
        return true
      })()`,
      check: eq(`document.getElementById('panel-mode-loginFailed').dataset.value`, 'onChange'),
      changed: true,
    },
    {
      // ⭐ 顺带把**条数**也改一处 ⇒ 证明它真的进了请求体（不是只在界面上画着 ✓）。
      name: '设置页：事件推送「连续失败」条数改成 5',
      do: `(() => {
        const inp = document.getElementById('panel-fail-streak')
        if (!inp) throw new Error('没有「连续失败条数」那一格')
        inp.value = '5'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        return true
      })()`,
      check: eq(`document.getElementById('panel-fail-streak').value`, '5'),
      changed: true,
    },
    {
      name: '设置页：保存事件推送 ⇒ PUT /api/push/panel 体里对得上',
      do: `(async () => {
        const card = [...document.querySelectorAll('#app .card')]
          .find((c) => (c.querySelector('.card-title')?.textContent ?? '').trim() === '事件推送')
        if (!card) throw new Error('没有「事件推送」卡')
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          ;[...card.querySelectorAll('button')].find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__panelPut = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__panelPut ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body ?? {}
           const lf = b.loginFailed ?? {}
           const th = lf.throttle ?? {}
           return [
             p.url,
             (lf.rules ?? []).join(','),
             th.mode,
             th.cooldownSecs,
             th.dailyCap,
             (b.perfFailStreak?.rules ?? []).join(','),
             b.perfSlow?.throttle?.mode ?? 'none',
             b.perfSlow?.slowFactor,
             b.perfFailStreak?.failStreak,
             // ⚠ 非性能行必须是**显式 null**（不是漏发、也不是 0）——
             // 漏了会让后端把它当"沿用旧值"，0 会被当成"设成 0"✗。
             b.loginFailed?.slowFactor === null ? 'null' : String(b.loginFailed?.slowFactor),
           ].join('/')
         })()`,
        '/api/push/panel/rule-1/onChange/300/10/rule-1/onChange/2.5/5/null',
      ),
      changed: true,
    },
    {
      // ⭐⭐ 刷新（重新 `GET /api/push/panel` ✓）⇒ 刚存的那些还在 ✓。
      // ⚠ 不刷新就断言是**不够**的：PUT 的响应被原样回填也能让界面显示新值，
      // 而装置里那份可能压根没存 ✗。
      name: '设置页：刷新（重新 GET）⇒ 事件推送仍是刚存的那份',
      do: clickSel('[aria-label="刷新当前页"]'),
      check: eq(
        `(() => {
           const sel = document.getElementById('panel-mode-loginFailed')
           const t = document.getElementById('panel-rules-loginFailed')
           const inp = document.getElementById('panel-cooldown-loginFailed')
           const fail = document.querySelector('input[aria-label="连续失败条数"]')
           return [
             sel ? sel.dataset.value : 'no-select',
             t ? t.dataset.value : 'no-picker',
             inp ? inp.value : 'no-input',
             fail ? fail.value : 'no-fail',
           ].join('/')
         })()`,
        'onChange/rule-1/5/5',
      ),
      // ⚠ 同上：刷新后内容一致 ⇒ 不能拿"界面变了"当判据 ✗。
    },

    // ── 账号停用（2026-10-01 用户口径："账号需要增加一个停用功能，
    //    这样在其他功能下拉菜单就不显示"）──
    { name: '账号页：有停用按钮', do: go('accounts'), check: has(APP_TEXT, '停用'), changed: true },
    {
      name: '账号页：停用主号 ⇒ 卡片出现「已停用」',
      do: `(() => {
        const li = [...document.querySelectorAll('#app li.card')].find((el) => el.textContent.includes('主力号'))
        if (!li) return '没有主号那张卡'
        ;[...li.querySelectorAll('button')].find((b) => b.textContent.trim() === '停用').click()
        return true
      })()`,
      // ⚠ 不判文案：成功提示里也写着"已停用 XXX" —— 那会让"启用后"的断言永远为真。
      // 判**卡片本身**有没有被压暗（`opacity-60`）：唯一无歧义的状态。
      check: eq(
        `(() => { const li = [...document.querySelectorAll('#app li.card')].find((el) => el.textContent.includes('主力号')); return li ? li.className.includes('opacity-60') : 'no-card' })()`,
        true,
      ),
      changed: true,
    },
    {
      // ⭐ 用户要的就是这一条：**别的页面的下拉里不再出现它**
      name: '兑换页：账号下拉里已经没有停用的那个号',
      do: go('exchange'),
      // 自建下拉的选项只在**展开时**存在 ⇒ 这条得自己点开看一眼再收起
      check: eq(
        `(() => {
           const t = document.querySelector('#exchange-account')
           t.click()
           const hit = [...document.querySelectorAll('.select-panel .select-option')]
             .some((o) => o.textContent.includes('主力号'))
           t.click()
           return hit
         })()`,
        false,
      ),
      changed: true,
    },
    {
      // ⭐⭐ 这条口径在 2026-10-02 **改过一次**，断言跟着改（旧版写的是"必须不是按钮"✗）。
      //
      // 用户原话（截图反馈两件事）：① 灰色那枚"禁止按钮颜色不对" —— 因为它压根
      // **不是按钮** ✗，是个徽标 ✗；② 白色那几枚"已抢光"**还能再点** ✗。
      // ⇒ 现在的口径：**所有判据没过的档都渲染 `disabled` 的 `btn`** ✓
      //   —— 形态对（禁用配色交给 daisyUI ✓）、而且真的点不动 ✓。
      //
      // ⚠⚠ 这条断言**曾经是空转的**：它查"页面里有没有 `已抢光` 按钮"，而当时夹具里
      // 兑换清单只有一个账号配了、门禁中途又**停用了主力号** ⇒ 清单 404 ⇒ 那一页
      // 一件奖品都没有 ⇒ "不是按钮"永远成立 ✗。补上第二个账号的清单之后它才真的开始
      // 判东西 —— 又一次「装置测不到 = 没测」（门禁全绿而功能是坏的）。
      name: '兑换页：默认（本地判定开）时「已抢光」是**禁用**按钮（形态对、点不动）',
      do: null,
      check: eq(
        `(() => {
           const b = [...document.querySelectorAll('#app button')].filter((x) => x.textContent.trim() === '已抢光')
           if (b.length === 0) return 'no-button'
           return b.every((x) => x.disabled)
         })()`,
        true,
      ),
    },
    {
      // ⭐⭐ 用户口径（2026-10-02）：「把想要的内容订阅到显示在上面方便直接看」
      // ⇒ **位置本身成了判据**：订阅那块必须排在「兑换」卡片之前 ✓。
      // 只断言"页面里有订阅这两个字"是不够的 ✗ —— 埋在清单底下照样能被 includes 查到
      // （直播页那条断言就是这么漏的，见「下一场」那条的注释）。
      //
      // ⚠ 这段注释在**模板字符串外面**；里面的表达式**绝不能出现反引号**（踩过）。
      name: '兑换页：订阅区在顶部（排在「兑换」卡片之前）',
      do: null,
      check: eq(
        `(() => {
           const cards = Array.from(document.querySelectorAll('#app .card'));
           const iSub = cards.findIndex((c) => c.textContent.includes('我订阅的'));
           const iEx = cards.findIndex((c) => c.textContent.includes('下单前再确认一次'));
           return [iSub >= 0, iEx >= 0, iSub < iEx].join('/');
         })()`,
        'true/true/true',
      ),
    },
    {
      // 顶上那行显示**名字**（这件现在还在清单里 ⇒ 用实时名字，与订阅时同名 ✓）
      name: '兑换页：顶部列出已订阅的那件',
      do: null,
      check: has(APP_TEXT, '移动云盘100万tokens叠加包'),
    },
    {
      // ⭐ 用户口径（2026-10-02）：「订阅怎么没有定时选项」——
      // 订阅盯的恰恰是"现在还不能兑、到点才放货"的那些 ✓，而**定时就是为它们存在的** ✓。
      // ⇒ 订阅那一行必须有「定时」，且判据与清单里**同一条**（`canSchedule`）✓。
      name: '兑换页：订阅那一行也有「定时」（与清单里同一条判据）',
      do: null,
      check: eq(
        `(() => {
           const card = Array.from(document.querySelectorAll('#app .card')).find((c) => c.textContent.includes('我订阅的'))
           if (!card) return 'no-card'
           const has = [...card.querySelectorAll('button')].some((b) => b.textContent.trim() === '定时')
           return has
         })()`,
        true,
      ),
    },
    {
      // ⚠ 口径钉子（用户原话 2026-10-02：「**不要取消订阅，直接用星星**」）：
      // 顶部那行**不许**出现文字版「取消订阅」按钮 ✗ —— 免得以后又被"补回来"。
      // ⚠ 这条必须在**取消那一件之前**跑：卡片空了它就成了空转 ✗（本仓栽过这种）。
      name: '兑换页：顶部那行没有文字「取消订阅」按钮（只用星星）',
      do: null,
      check: eq(
        `(() => {
           const card = Array.from(document.querySelectorAll('#app .card')).find((c) => c.textContent.includes('我订阅的'))
           if (!card) return 'no-card'
           if (card.querySelectorAll('li').length === 0) return 'empty-card'
           return [...card.querySelectorAll('button')].some((b) => b.textContent.trim() === '取消订阅')
         })()`,
        false,
      ),
    },
    {
      // ⭐ 点星星 ⇒ 服务端记住 ⇒ 顶部**多一件**。
      // 这是"订阅"这个功能的全部价值，也是装置**有状态**的理由：
      // 回一份静态列表就只能证明"按钮点了有反应" ✗。
      name: '兑换页：点星星订阅另一件 ⇒ 顶部多一件',
      do: `(() => {
        const b = document.querySelector('#app button[aria-label="订阅"]')
        if (!b) return '没有可订阅的星星'
        b.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const card = Array.from(document.querySelectorAll('#app .card')).find((c) => c.textContent.includes('我订阅的'))
           if (!card) return 'no-card'
           return card.querySelectorAll('li').length
         })()`,
        2,
      ),
      changed: true,
    },
    {
      // ⭐ 从顶上取消 ⇒ 那一件消失（回到 1 件）。
      //
      // ⚠ 入口是**星星**（用户口径 2026-10-02：「不要取消订阅，直接用星星」）——
      // 顶部那行**没有**文字版「取消订阅」了 ✗，与清单里那两处是**同一个入口** ✓。
      name: '兑换页：顶部那颗星星就是取消订阅（点了 ⇒ 那一件消失）',
      do: `(() => {
        const card = Array.from(document.querySelectorAll('#app .card')).find((c) => c.textContent.includes('我订阅的'))
        if (!card) return 'no-card'
        const b = card.querySelector('button[aria-label="取消订阅"]')
        if (!b) return '顶部那行没有星星'
        b.click()
        return true
      })()`,
      check: eq(
        `(() => {
           const card = Array.from(document.querySelectorAll('#app .card')).find((c) => c.textContent.includes('我订阅的'))
           if (!card) return 'no-card'
           return card.querySelectorAll('li').length
         })()`,
        1,
      ),
      changed: true,
    },
    {
      // ⭐ 兑换页的「下单前再确认一次」开关（2026-10-02 用户口径：加个取消二次弹窗的按钮）。
      name: '兑换页：有「下单前再确认一次」这个开关',
      do: null,
      check: has(APP_TEXT, '下单前再确认一次'),
    },
    {
      // ⭐⭐ 断言的是**开关状态与那句说明的耦合**（2026-10-02 用户口径：不要造成歧义）。
      // 关掉确认之后还写"点下去要再确认一次"就是骗人（用户点下去直接扣豆）。
      //
      // ⚠ 读**真实 DOM 的 checked**，不是读文案 —— 文案是从状态渲染的，
      //   读文案测不出"状态对、DOM 不对"这一类。
      // ⚠ 选择器要**精确**：曾经用 parentElement.textContent 去认，而它一路冒泡，
      //   `.find` 撞上了别的复选框（诊断显示那个 parent 里还有 DIALOG ⇒ 根本不是这个
      //   label）⇒ 断言测的是别的东西。现在按"紧邻兄弟的文字"认，只认自己那一个。
      // ⚠⚠ 这段注释在**模板字符串里面**：里面**绝不能出现反引号**（踩过 ——
      //   一个反引号就把模板提前结束，整个文件 SyntaxError，症状是门禁报错退出）。
      name: '兑换页：说明与开关状态一致（关了就不说"要再确认一次"）',
      do: null,
      check: eq(
        `(() => {
           const all = document.querySelectorAll('#app input[type=checkbox]');
           let box = null;
           for (const i of all) {
             const s = i.nextElementSibling;
             if (s && s.textContent.includes('下单前再确认一次')) box = i;
           }
           if (!box) return 'no-switch';
           return box.checked === document.getElementById('app').textContent.includes('要再确认一次');
         })()`,
        true,
      ),
    },
    {
      name: '账号页：回到账号页准备启用',
      do: go('accounts'),
      check: eq(
        `(() => { const li = [...document.querySelectorAll('#app li.card')].find((el) => el.textContent.includes('主力号')); return li ? li.className.includes('opacity-60') : 'no-card' })()`,
        true,
      ),
      changed: true,
    },
    {
      name: '账号页：再点启用 ⇒ 恢复可选（停用是可逆的）',
      do: `(() => {
        const li = [...document.querySelectorAll('#app li.card')].find((el) => el.textContent.includes('主力号'))
        if (!li) return '没有主号那张卡'
        ;[...li.querySelectorAll('button')].find((b) => b.textContent.trim() === '启用').click()
        return true
      })()`,
      changed: true,
    },
    {
      // ⭐⭐ 用户口径（2026-10-02）：「这些设置先取消显示，设置后面单独弄」
      // ⇒ 账号设置弹窗里那两块（**功能开关** / **运行结果推送**）**不该再渲染** ✓。
      //
      // ⚠⚠ 这条**必须同时证明"弹窗真的开了、数据真的读到了"** ✗ —— 否则"页面上没有
      // 功能开关"在**弹窗没开**或**读设置失败（错误态）**时**同样成立** ⇒ 又是一条空转 ✗
      // （今天已经栽过一次：兑换清单 404 ⇒ 那条断言一直是空的）。
      // 所以判据里带上"任务参数那块在" ✓（同一个弹窗、同一个表单、这次没被藏 ✓）。
      name: '账号页：设置弹窗里没有「功能开关」（先取消显示，且弹窗确实开着）',
      do: clickText('#app li.card button', '设置'),
      check: eq(
        `(() => {
           const d = document.querySelector('dialog#account-settings')
           if (!d || !d.open) return 'not-open'
           const t = d.textContent
           return [t.includes('刷新凭据的剩余天数阈值'), t.includes('功能开关')].join('/')
         })()`,
        'true/false',
      ),
      changed: true,
    },
    {
      /**
       * ⭐⭐ 2026-10-08 用户口径（「**很显然我说的和云盘账户没有关系**」＋「**丢弃现有
       * 情况，重写**」）：推送渠道整块从账号侧删掉 ✗ ⇒ 弹窗里**连渠道表单的影子都不该有** ✓。
       *
       * ⚠ 判据不能只看"有没有「运行结果推送」这五个字"：把标题换个说法、表单照留，
       * 这条照样绿 ✗。所以按**表单自己的锚**判 —— 推送编辑器的总开关文案「启用推送」、
       * 推送标题那一格、以及只有它才有的下拉 id `push-min-level` ✓。
       */
      name: '账号页：设置弹窗里也没有推送渠道表单（整块删掉，不是藏起来）',
      do: null,
      check: eq(
        `(() => {
           const d = document.querySelector('dialog#account-settings')
           if (!d || !d.open) return 'not-open'
           const t = d.textContent
           return [
             t.includes('运行结果推送'),
             t.includes('启用推送'),
             t.includes('推送标题'),
             !!d.querySelector('#push-min-level'),
           ].join('/')
         })()`,
        'false/false/false/false',
      ),
    },
    {
      // 收尾：弹窗开着会挡住后面的步骤 ⇒ 关掉（并顺手钉住"关得掉"）
      name: '账号页：关掉设置弹窗',
      do: `document.getElementById('account-settings').close()`,
      check: eq('document.querySelectorAll("dialog[open]").length', 0),
      changed: true,
    },

    // ── 任务页：分组 + 参数挪到单独的配置页（2026-10-01 用户口径）──
    {
      name: '任务页：分组是 tab（可见的组各一个）',
      do: go('tasks'),
      // 装置里有 5 个任务、其中 `internal-probe` 是隐藏的 ⇒ 任务组是 3 个
      // （signin / live / device）。隐藏任务那一组**不该**留下一个空 tab。
      //
      // ⚠ 总数是 **4**：任务页最前面多了一格「编排」✓（2026-10-02）—— 它的数据源
      // 不是 `/api/capabilities` 而是 `/api/pipelines` ✓，所以它不算"任务组"，
      // 但**是**一个 tab。这一条钉的是"tab 的数量"，编排那格必须算进来 ✓。
      check: eq(`document.querySelectorAll('#app [role="tab"]').length`, 4),
      changed: true,
    },
    {
      /**
       * ⚠ 选中的 tab 必须**看得出**是选中的。
       *
       * 判"实际背景色不一样"而不是"有没有某个类名"：本项目 `app.css` 里有一份
       * daisyUI 组件的 `exclude` 清单，用到被排除的组件时**类名照样在 DOM 上、
       * 样式却静默消失**（2026-10-01 实际踩到：`tab-active` 不在产物 CSS 里，
       * 三个组名渲染成一行没有任何样式的文字，而任何"类名在不在"的断言都会通过）。
       */
      name: '任务页：选中的 tab 与未选中的**看起来不一样**',
      do: null,
      check: eq(
        `(() => {
          const tabs = [...document.querySelectorAll('#app [role="tab"]')]
          if (tabs.length < 2) return 'tabs<2'
          const bg = (el) => getComputedStyle(el).backgroundColor
          const fg = (el) => getComputedStyle(el).color
          return bg(tabs[0]) !== bg(tabs[1]) || fg(tabs[0]) !== fg(tabs[1])
        })()`,
        true,
      ),
    },
    {
      /**
       * ⭐⭐ 2026-10-02 用户两次报「你的 tabs 呢」，**根因就钉在这一条**。
       *
       * tab 排原本写在 `taskCards()` 的最后一个分支里 ⇒ **一切到「编排」整排消失**，
       * 用户再也切不回去（两张截图都是这个状态：编排卡片在、tab 排没了）。
       *
       * ⚠ 为什么原来门禁全绿也没发现：进页面时 `activeGroup` 是**空串** ⇒ 走的是
       * "有分组"那个分支 ⇒ tab 排当然在 ✓。**这是状态相关的**，夹具只覆盖了初始态 ✗。
       * ⇒ 这条必须**先点一下「编排」**再断言（只断言"DOM 里有 tab"是抓不到的 ✗）。
       */
      name: '任务页：切到「编排」后 tab 排仍在（否则再也切不回去）',
      do: `(() => {
        const t = [...document.querySelectorAll('#app [role="tab"]')].find((x) => x.textContent.includes('编排'))
        if (!t) throw new Error('没有「编排」tab')
        t.click()
        return true
      })()`,
      check: eq(
        `(() => {
          const tabs = [...document.querySelectorAll('#app [role="tab"]')]
          const on = tabs.filter((t) => t.getAttribute('aria-selected') === 'true')
          // ⚠ 同时钉住"只能亮一个"：currentGroup() 有兜底（选中的组没出现就落到第一组），
          // 在「编排」页它会返回第一组 ⇒ 不特殊处理就会**两个 tab 同时亮** ✗
          return tabs.length + '/' + on.length + '/' + (on[0] ? on[0].textContent.trim().slice(0, 2) : '无')
        })()`,
        '4/1/编排',
      ),
      changed: true,
    },
    {
      // 再切回去：分组卡片必须回来（钉住"来回都能走"）
      name: '任务页：从「编排」切回分组',
      do: `(() => {
        const t = [...document.querySelectorAll('#app [role="tab"]')].find((x) => !x.textContent.includes('编排'))
        t.click()
        return true
      })()`,
      check: eq(
        `document.querySelectorAll('#app [role="tab"]').length + '/' + [...document.querySelectorAll('#app [role="tab"]')].filter((t) => t.getAttribute('aria-selected') === 'true').length`,
        '4/1',
      ),
      changed: true,
    },
    {
      // 一次只看一组：默认那组在，"别组"的任务不在（堆叠版本会两组都在）
      name: '任务页：默认只显示第一组',
      do: null,
      check: eq(
        `(() => { const t = document.getElementById('app').textContent; return t.includes('每日签到') + '/' + t.includes('直播口令') })()`,
        'true/false',
      ),
    },
    {
      // 留在"直播小红花"这一组：下面几步要用到那张卡
      name: '任务页：切 tab ⇒ 换一组内容',
      do: `(() => {
        const tab = [...document.querySelectorAll('#app [role="tab"]')].find((t) => t.textContent.includes('直播小红花'))
        tab.click()
        return true
      })()`,
      check: eq(
        `(() => { const t = document.getElementById('app').textContent; return t.includes('直播口令') + '/' + t.includes('每日签到') })()`,
        'true/false',
      ),
      changed: true,
    },
    {
      name: '任务页：有参数的任务给「配置」入口',
      do: null,
      check: eq(
        `[...document.querySelectorAll('#app .card')].some((c) => c.textContent.includes('直播口令') && [...c.querySelectorAll('button')].some((b) => b.textContent.trim() === '配置'))`,
        true,
      ),
    },
    {
      name: '配置中心：能从任务卡带着任务名进来（右栏就是「公共配置」）',
      do: `(() => {
        const card = [...document.querySelectorAll('#app .card')].find((c) => c.textContent.includes('直播口令'))
        ;[...card.querySelectorAll('button')].find((b) => b.textContent.trim() === '配置').click()
        return true
      })()`,
      check: eq(
        // ⚠ 这里**不能**引用 node 侧那颗 `APP_TEXT`（它本身就是一段页面表达式的字符串）——
        // 写进内联表达式里会被当成页面变量 ⇒ `ReferenceError: APP_TEXT is not defined` ✗。
        `(() => {
           const t = document.getElementById('app')?.textContent ?? ''
           // ⚠ 2026-10-05 最终形状：这一页**没有页签**、也**没有「账号私有配置」那一半**了
           // （推送拆去 /accounts/settings，私有那半按用户口径整块删掉）⇒ 右栏只有公共配置。
           return [t.includes('公共配置'), t.includes('账号私有配置'), t.includes('直播口令')].join('/')
         })()`,
        'true/false/true',
      ),
      changed: true,
    },
    {
      name: '配置中心（公共栏）：改「听弹幕时长」为 123 并保存',
      do: `(() => {
        const inp = document.querySelector('#app input[type="number"]')
        inp.value = '123'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        ;[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '保存').click()
        return true
      })()`,
      check: has(APP_TEXT, '已保存'),
      changed: true,
    },
    {
      // ⭐ 装置只在 listenSeconds === 123 时回 200 ⇒ 这一步真的钉住了
      //   "配置页存的东西进了运行请求"，而不只是"按钮点了有反应"。
      name: '任务页：运行 ⇒ 请求真的带上了配置页存的参数',
      do: `(() => {
        history.pushState({}, '', '/tasks')
        window.dispatchEvent(new PopStateEvent('popstate'))
        return true
      })()`,
      check: has(APP_TEXT, '直播口令'),
      changed: true,
    },
    {
      name: '任务页：选账号 → 运行直播任务（带参数才放行）',
      do: `(() => {
        const card = [...document.querySelectorAll('#app .card')].find((c) => c.textContent.includes('直播口令'))
        // 账号在共用选择器里（面板第 0 项是「全部账号」，第 1 项起是逐个账号）。
        // 已经勾好的话不要再点 —— 再点一次会取消勾选。
        const trigger = document.querySelector('#account-picker')
        trigger.click()
        const items = [...document.querySelectorAll('.select-panel .select-option')]
        const picked = items[1].getAttribute('aria-selected') === 'true'
        if (!picked && items[0].getAttribute('aria-selected') === 'false') items[1].click()
        trigger.click() // 收起面板，别挡着下面那个「运行」按钮
        ;[...card.querySelectorAll('button')].find((b) => b.textContent.trim() === '运行').click()
        return true
      })()`,
      check: has(APP_TEXT, '已提交'),
      changed: true,
    },

    // ── 配置中心（`/tasks/config`，2026-10-05）────────────────────────────
    //
    // 用户口径两轮定死：
    // ① 「上面切换任务配置还是推送配置，下面左边列出所有配置大项，右边主窗口是具体配置」；
    // ② 「我说的是**每个任务的配置**分为公共和账号私有」（并举例：每个账号的推送不一样、
    //    上传路径可能不一样）。
    // ⇒ 顶部两栏（任务配置 / 推送配置）**都要在**；而"公共 / 账号私有"是**任务那一栏内部**
    //    的分法（右边那半），不是整页的分法。
    //
    // ⚠ 断言都盯着 `#config-rail` / `#config-detail` 两个锚，不能只搜整页文字：
    // 选中项的名字**两边都印**（左边那一行、右边标题），搜全页等于没验"右栏换成了它"。
    //
    // ⚠⚠ 「谁写到哪一份」这条**只能量发出去的请求体**：夹具里
    // `PUT /api/accounts/{id}/settings` 是**回显**（`lib.mjs`，不落盘）⇒
    // "存完再 GET 看看"在夹具里恒等于"没存" ✗。所以下面用 `window.fetch` 拦一次
    // （`__put`），量的是**真发出去的 URL 与 body** —— 那正是这条口径的判据 ✓。
    {
      name: '配置中心：从任务页进得去，左栏列任务、右栏只有「公共配置」',
      do: `[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '配置中心').click()`,
      check: eq(
        `(() => {
           const rail = document.querySelector('#config-rail')?.textContent ?? ''
           const detail = document.querySelector('#config-detail')?.textContent ?? ''
           return [location.pathname, rail.includes('可配置任务'), rail.includes('直播口令'),
                   detail.includes('公共配置'), detail.includes('所有账号共用'),
                   detail.includes('账号私有配置'),
                   detail.includes('听弹幕时长')].join('/')
         })()`,
        '/tasks/config/true/true/true/true/false/true',
      ),
      changed: true,
    },
    {
      name: '配置中心：改一个参数 ⇒ 亮「有未保存修改」',
      do: `(() => {
        const inp = document.querySelector('#config-detail input[type="number"]')
        if (!inp) throw new Error('右栏没有数字参数')
        inp.value = '77'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        return true
      })()`,
      check: has(`document.querySelector('#config-detail')?.textContent ?? ''`, '有未保存修改'),
      changed: true,
    },
    {
      // ⭐ 草稿那条路的钉子：放弃修改 = **回到上次保存的那一份**（前面那步存的是 123），
      // 而不是"填回参数默认值"（60）—— 两者在界面上很像，但语义完全不同。
      name: '配置中心：点「放弃修改」⇒ 回到已保存的 123（不是默认值 60）',
      do: `[...document.querySelectorAll('#config-detail button')].find((b) => b.textContent.trim() === '放弃修改').click()`,
      check: eq(
        `(() => {
           const t = document.querySelector('#config-detail')?.textContent ?? ''
           const inp = document.querySelector('#config-detail input[type="number"]')
           return t.includes('有未保存修改') + '/' + (inp ? inp.value : '没有输入框')
         })()`,
        'false/123',
      ),
      changed: true,
    },
    {
      // ⭐ 反方向：任务那一栏保存**只带 params**，不许把 `notify` / `backupWaitSecs` 这类
      // 账号级字段混进去（两栏分栏之后这是最容易串的一条）。顺带把 123 还回去。
      name: '配置中心：任务栏保存 ⇒ 只发任务参数（不带账号级字段），并还原 123',
      do: `(async () => {
        const inp = document.querySelector('#config-detail input[type="number"]')
        if (!inp) throw new Error('任务栏没有数字参数')
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          for (const v of ['456', '123']) {
            inp.value = v
            inp.dispatchEvent(new Event('input', { bubbles: true }))
            await new Promise((r) => setTimeout(r, 200))
            ;[...document.querySelectorAll('#config-detail button')].find((b) => b.textContent.trim() === '保存').click()
            await new Promise((r) => setTimeout(r, 700))
          }
          window.__put2 = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const ps = window.__put2 ?? []
           if (ps.length < 2) return 'PUT 次数不对：' + ps.length
           const keys = Object.keys(ps[0].body).sort().join(',')
           const leaked = ['notify', 'backupWaitSecs', 'refreshTokenDays', 'skipTasks', 'aiAvatarDailyLimit']
             .filter((k) => k in ps[0].body).join(',')
           return [ps[0].url, keys, leaked, ps[1].body.params.listenSeconds].join('/')
         })()`,
        '/api/tasks/live-room/config/params//123',
      ),
      changed: true,
    },
    {
      // ⭐ 账号配置页（`/accounts/settings`）的入口：账号页右上那个按钮。
      // ⚠ 它原来是任务配置页「账号私有配置」那半的「改它的推送」跳过来的 —— 那一半
      // 2026-10-05 按用户口径**整块删掉**了 ⇒ 现在这条入口是账号页的按钮 ✓。
      name: '账号配置页：从账号页的「账号配置」按钮进得去',
      do: `(async () => {
        ;[...document.querySelectorAll('#app button')].find((b) => b.textContent.trim() === '账号')?.click()
        await new Promise((r) => setTimeout(r, 900))
        const b = [...document.querySelectorAll('#app button')].find((x) => x.textContent.trim() === '账号配置')
        if (!b) throw new Error('账号页没有「账号配置」按钮')
        b.click()
        for (let i = 0; i < 60; i++) {
          if (document.querySelector('#acct-detail') && !document.querySelector('#acct-detail .loading')) break
          await new Promise((r) => setTimeout(r, 200))
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const rail = document.querySelector('#acct-rail')?.textContent ?? ''
           const detail = document.querySelector('#acct-detail')?.textContent ?? ''
           return [location.pathname, rail.includes('主力号'), detail.includes('账号级设置')].join('/')
         })()`,
        '/accounts/settings/true/true',
      ),
      changed: true,
    },
    {
      /**
       * ⭐ 老地址兼容（2026-10-08 改名）：这一页从 `/accounts/push` 改成
       * `/accounts/settings` ✓ —— 推送渠道搬去「推送」页（`/push`）之后，路径里那个
       * `push` 就名不副实了 ✗。
       *
       * ⚠ 钉的是"**老链接照旧能打开**" ✓：书签 / 浏览器历史 / 旧文档里的
       * `/accounts/push` 不该 404，也不该把人甩回列表页 ✗ —— 与"老数据行为不变"
       * 同一个口径 ✓（改名只该影响**新**链接 ✓）。
       *
       * ⚠ 这里**故意不**断言"地址被改写成新的" ✗：本仓的做法是**不重定向**，
       * 而是让视图判定同时认这两个路径 ✓（少一次跳转、也不动历史记录 ✓）。
       */
      name: '账号配置页：老地址 /accounts/push 照旧打得开（改名后的兼容 ✓）',
      do: `(async () => {
        history.pushState({}, '', '/accounts/push')
        window.dispatchEvent(new PopStateEvent('popstate'))
        await new Promise((r) => setTimeout(r, 900))
        return true
      })()`,
      check: eq(
        `(() => {
           const detail = document.querySelector('#acct-detail')?.textContent ?? ''
           return [location.pathname, detail.includes('账号级设置')].join('/')
         })()`,
        '/accounts/push/true',
      ),
      changed: true,
    },
    {
      /**
       * ⭐⭐ 2026-10-08 用户口径（「**很显然我说的和云盘账户没有关系**，为啥你这里云盘
       * 账户有配置」＋「**丢弃现有情况，重写**」）：**推送渠道整块从账号侧删掉** ✗ ——
       * 它现在是**登录用户级**的（「推送」页 `/push` ✓）。
       *
       * ⚠ 这条钉的是"账号侧**没有**推送"，而不只是"推送页**有**推送"：只钉后者的话，
       * 把渠道在两边各留一份照样绿 ✗ —— 而"两边各一份"正是用户这次要拆掉的东西 ✓。
       * ⚠ 同时必须证明**这一页真的渲染出来了**（账号级设置 + 功能开关在），
       * 否则"页面上没有推送渠道"在**整页错误态**下同样成立 ⇒ 一条空转 ✗
       * （本文件里已经栽过一次：兑换清单 404 ⇒ 断言一直是空的 ✓）。
       */
      name: '账号配置页：只有账号级设置 + 功能开关，**没有**推送渠道（推送已收成登录用户级）',
      do: null,
      check: eq(
        `(() => {
           const d = document.getElementById('acct-detail')
           if (!d) return 'no-detail'
           const detail = d.textContent
           // ⚠ 判据走**小标题**（h4）+ 推送编辑器自己的锚，不搜整页文字：
           //   这一页里那句指路（"推送渠道不在这里：… 推送 页里配"）**故意**带着
           //   这几个字 ✓ —— 搜整页文字会把那句指路本身判成"这一页有推送渠道" ✗。
           const heads = [...d.querySelectorAll('h4, .card-title')].map((x) => x.textContent.trim())
           const nav = document.querySelector('#app [aria-current="page"]')?.textContent?.trim() ?? ''
           return [heads.includes('推送渠道'), heads.includes('运行结果推送'),
                   detail.includes('账号级设置'), detail.includes('功能开关'),
                   detail.includes('备份任务等待时间'), detail.includes('AI 头像每日上限'),
                   !!d.querySelector('#push-min-level'), nav].join('/')
         })()`,
        'false/false/true/true/true/true/false/账号',
      ),
    },
    {
      // ⭐⭐ 账号那一栏最贵的一条：账号设置保存时，**没显示在界面上的格子也必须原样写回去**
      // （`PUT` 是整体替换）。夹具那份 ACCOUNT 是 20/10/[117]/10（2026-10-08 起
      // `AccountSettings` **没有** `notify` 那一格了 ✓）。
      name: '账号配置页：保存 ⇒ 发出去的是这个账号的**全部四格**（没碰的原样、且不带 notify）',
      do: `(async () => {
        // 先切回主力号（上一步选中了另一个账号），再改一格
        const btns = [...document.querySelectorAll('#acct-rail button')]
        btns[0].click()
        await new Promise((r) => setTimeout(r, 900))
        const box = [...document.querySelectorAll('#acct-detail fieldset')]
          .find((f) => (f.querySelector('legend')?.textContent ?? '').includes('备份任务等待时间'))
        const inp = box?.querySelector('input')
        if (!inp) throw new Error('账号级设置里没有「备份任务等待时间」那一格')
        inp.value = '9'
        inp.dispatchEvent(new Event('input', { bubbles: true }))
        await new Promise((r) => setTimeout(r, 300))
        const put = []
        const orig = window.fetch
        window.fetch = async (input, init) => {
          const url = typeof input === 'string' ? input : input.url
          if (((init && init.method) || 'GET').toUpperCase() === 'PUT') put.push({ url, body: init && init.body })
          return orig(input, init)
        }
        try {
          ;[...document.querySelectorAll('#acct-detail button')].find((b) => b.textContent.trim() === '保存').click()
          for (let i = 0; i < 80 && put.length === 0; i++) await new Promise((r) => setTimeout(r, 100))
          window.__put = put.map((p) => ({ url: p.url, body: JSON.parse(p.body) }))
        } finally {
          window.fetch = orig
        }
        return true
      })()`,
      check: eq(
        `(() => {
           const p = (window.__put ?? [])[0]
           if (!p) return '没发出 PUT'
           const b = p.body
           // ⭐ notify 必须不在请求体里：2026-10-08 起账号设置没有这一格了 ✗，
           //   而多带一格会被后端的 deny_unknown_fields 判 400（整体替换的语义下，
           //   前端"顺手把旧字段带上"正是这次要拆掉的那种错 ✗）。
           return [p.url, b.backupWaitSecs, b.refreshTokenDays, (b.skipTasks ?? []).join(','),
                   b.aiAvatarDailyLimit, 'notify' in b, Object.keys(b).sort().join(',')].join('/')
         })()`,
        '/api/accounts/13800000000/settings/9/10/117/10/false/aiAvatarDailyLimit,backupWaitSecs,refreshTokenDays,skipTasks',
      ),
      changed: true,
    },
    // ── 日志页（`/logs`）：**滚动位置**（2026-10-04 用户报「服务端日志页面出日志会强制滚动到顶部」✗）──
    //
    // 那一页每 2 s 轮询一次把整份列表换掉，而卡片原先写在条件槽里 ⇒ `setNodes`
    // 每次换一颗**新容器**（节点一换，`scrollTop` 天然是 0）⇒ 用户被弹回最旧那屏 ✗。
    // 修法是"卡片常驻 + `load()` 里记/写滚动位置"（`web/src/pages/logs-page.tsx`），
    // 下面两步就是它的钉子：**各自真等一次轮询**（2.4 s）再断言 ✓。
    //
    // ⚠ 夹具那份 `/api/logs` 挂在 **`VARIANTS.ready`** 上（`lib.mjs` 的 `logsState`）——
    // 所以下面这几步只属于 **ready 档** ✓（别的变体里那条路径是 404）。
    // 没有这一条"贴底跟随"就量不出来（静态名单下"还在底部"这句在坏代码里也可能成立 ✗）。
    // 所以下面 `do` 里**先断言容器真的可滚**：夹具一旦没给日志，"通过"就成了一句空话
    // ⇒ 宁可当场红 ✗。
    // ⚠ 只量 `scrollTop` 也不够：真坏法是"换了节点"（新节点的 `scrollTop` 天然是 0），
    // 所以"还是不是同一个容器"（`__probe` 标记）必须一起量 ✓ —— 实测：
    // 把卡片改回条件渲染（其余不动）后，**中间那条**靠 `__probe` 才红 ✗；
    // 而"贴底"那条会**照过**（`load()` 的写回把新容器重新钉到了底部）⇒
    // 两条一起才是完整的钉子，别删任何一条 ✓。
    //
    // ⚠⚠ 2026-10-09 分页时"贴底"那条的**判据换了**（不是放宽，是**跟着夹具的模型换**）：
    // 夹具现在照真后端分**两层**（内存窗口 + 盘上 ✓，见 `lib.mjs::logsState`）——
    // 实时那一份是"最新 `limit` 行"，一旦总量超过 2000，**窗口大小就恒定了**
    // ⇒ 每拍新来两行时 `scrollHeight` **不再变**（老夹具只有 60 行、窗口还在长，
    // 所以那时"变高"是有效的判据 ✓）。现在改成量**内容**：底部那一行的行号必须**变大**
    // （新行真的来了 ✓）＋ 位置仍贴着底（跟随没丢 ✓）—— 比"总高变大"更直接 ✓。
    {
      /**
       * ⚠ 2026-10-09：这一条的 `do` 从"只导航"改成"**导航 + 等首屏铺出来**"。
       *
       * 为什么必须等：夹具那份 `/api/logs` 现在照真后端的**窗口**给（最新 2000 行 ✓，
       * 见 `lib.mjs::logsState`）—— 一次 300 KB 上下，机器一忙（推送档 7 条并行、
       * 或别的写入线在编 Rust）**导航完立刻查**就会读到还没铺的空列表 ✗
       * （实测：负载高时读到 `/logs/false`，而页面本身是好的 ✓）。
       * 断言**一个字没改**（还是"路径对 + 列表里铺了 20 行以上" ✓），
       * 只是把"睡够就算"换成"**轮询到条件成立**"（本仓的门禁口径 ✓）。
       */
      name: '切到日志页（服务端全量日志，夹具每次请求多两行）',
      do: `(async () => {
        ${go('logs')}
        for (let i = 0; i < 160; i++) {
          if (document.querySelectorAll('#logs-scroll > div[data-k]').length > 20) break
          await new Promise((r) => setTimeout(r, 50))
        }
        return true
      })()`,
      check: eq('location.pathname + "/" + (document.querySelectorAll("#logs-scroll > div").length > 20)', '/logs/true'),
      changed: true,
    },
    {
      name: '日志页：滚到中间后跨一次轮询不被弹回顶部',
      do: `(async () => {
        const el = document.getElementById('logs-scroll');
        if (!el) throw new Error('没有滚动容器 #logs-scroll');
        const max = el.scrollHeight - el.clientHeight;
        if (max < 200) throw new Error('日志页没有可滚动的内容（夹具没给 /api/logs？）');
        // ⚠ 位置要取**滚动范围**的一半，不是 scrollHeight 的一半 ——
        // 后者在"视口本身就占了半屏"时几乎等于底部（实测 632/634）⇒ 会被"贴底跟随"
        // 那条分支正确地接管，读数就成了"没弹回顶部、但走了 42px"✗。
        el.__probe = 1;
        el.scrollTop = Math.round(max / 2);
        window.__logsTop = el.scrollTop;
        await new Promise((r) => setTimeout(r, 2400));
        return el.scrollTop;
      })()`,
      check: eq(
        `(() => { const el = document.getElementById('logs-scroll'); return el.__probe === 1 && el.scrollTop === window.__logsTop })()`,
        true,
      ),
    },
    {
      name: '日志页：贴底后跨一次轮询仍贴着底（底部那一行换成更新的了）',
      do: `(async () => {
        const el = document.getElementById('logs-scroll');
        if (!el) throw new Error('没有滚动容器 #logs-scroll');
        el.scrollTop = el.scrollHeight;
        el.dispatchEvent(new Event('scroll', { bubbles: true }));
        await new Promise((r) => setTimeout(r, 300));
        const rows = [...el.querySelectorAll('div[data-k]')];
        const last = rows[rows.length - 1];
        window.__bottomSeq = Number((last ? last.textContent.match(/第 (\\d+) 行/) : [])?.[1]);
        if (!(window.__bottomSeq > 0)) throw new Error('没读到最下面那一行的行号');
        await new Promise((r) => setTimeout(r, 2400));
        return true;
      })()`,
      check: eq(
        `(() => {
           const el = document.getElementById('logs-scroll');
           const rows = [...el.querySelectorAll('div[data-k]')];
           const last = rows[rows.length - 1];
           const seq = Number((last ? last.textContent.match(/第 (\\d+) 行/) : [])?.[1]);
           // ① 新行真的来了（底部那一行更靠后 ✓）；② 视图仍贴着底（跟随没丢 ✓）
           return JSON.stringify({
             newer: seq > window.__bottomSeq,
             pinned: el.scrollHeight - el.scrollTop - el.clientHeight < 4,
           });
         })()`,
        JSON.stringify({ newer: true, pinned: true }),
      ),
    },

    /**
     * ── 日志页：**每一行是哪个账号的**（用户口径 2026-10-05：「页面上看不到是那个账号的
     * 日志啊」）──
     *
     * 夹具那份日志**每一行**都带 `account="…"`（两个夹具账号轮流 ✓，见 `lib.mjs` 的
     * `logLine` ✓），这是照真后端的形状给的：`mcloud-app` 的 sink 会给一趟运行的每一行
     * 盖上账号（`crates/mcloud-app/src/run_labels.rs` ✓）。
     *
     * 页面上应当看到的是**人名**（夹具里 `13800000000` 的人名是「主力号」✓），
     * 而且**每一行**都要有 —— 只有第一行有（真后端改之前就是那样 ✗）就白搭 ✓。
     */
    {
      name: '日志页：行首标出账号，且显示的是人名（不是主键）',
      do: `(async () => { await new Promise((r) => setTimeout(r, 300)); return true })()`,
      check: eq(
        `(() => {
          const rows = [...document.querySelectorAll('#logs-scroll > div[data-k]')];
          if (rows.length < 5) return 'rows=' + rows.length;
          const chips = rows.map((r) => r.querySelector('[data-acc]'));
          return JSON.stringify({
            mapped: chips.some((c) => c && c.textContent.trim() === '主力号'),
            everyRow: chips.every((c) => c !== null),
            leaked: rows.some((r) => r.textContent.includes('account=')),
          });
        })()`,
        JSON.stringify({ mapped: true, everyRow: true, leaked: false }),
      ),
    },
    {
      name: '日志页：两个账号 ⇒ 出现账号筛选行（只有一个账号时不铺，免得是摆设）',
      do: 'true',
      check: eq(
        `(() => {
          const all = document.querySelector('[aria-label="账号：全部"]');
          const a = document.querySelector('[aria-label="账号：主力号"]');
          const b = document.querySelector('[aria-label="账号：13900000001"]');
          return JSON.stringify({ all: !!all, a: !!a, b: !!b });
        })()`,
        JSON.stringify({ all: true, a: true, b: true }),
      ),
    },
    /**
     * ── 日志页：**类别**那一行（2026-10-06 用户口径：「这种应该分为运维日志，
     * 且只有管理员可见吧」）──
     *
     * 夹具那份日志全是 `category: 'task'`（`lib.mjs` 的 `logLine` ✓）⇒
     * 点「运维」应当**一行都不剩**（不是"没反应"✗），点回「任务」行要全回来 ✓。
     * 这一条同时钉住了三件事：类别那一行真的铺出来了、`category` 那一格真的进了
     * 过滤（漏了它 ⇒ 点运维还是满屏行 ✗）、以及缓存键里有它（漏了 ⇒ 切了没反应 ✗）。
     *
     * ⚠ 「运维」这颗按钮**只有管理员才有**（`currentIsAdmin` ✓）—— 夹具的
     * `/api/session` 给了 `isAdmin: true` ✓，所以这里能断言它存在 ✓。
     */
    {
      name: '日志页：类别行铺出来了（管理员能看见「运维」）',
      do: 'true',
      check: eq(
        `(() => {
          const all = document.querySelector('[aria-label="类别：全部"]');
          const task = document.querySelector('[aria-label="类别：任务"]');
          const ops = document.querySelector('[aria-label="类别：运维"]');
          return JSON.stringify({ all: !!all, task: !!task, ops: !!ops });
        })()`,
        JSON.stringify({ all: true, task: true, ops: true }),
      ),
    },
    {
      name: '日志页：点「运维」⇒ 夹具里没有运维行 ⇒ 一行不剩',
      do: `(async () => {
        const btn = document.querySelector('[aria-label="类别：运维"]');
        if (!btn) throw new Error('没找到类别按钮「运维」');
        btn.click();
        await new Promise((r) => setTimeout(r, 300));
        return true;
      })()`,
      check: eq(
        `document.querySelectorAll('#logs-scroll > div[data-k]').length === 0`,
        true,
      ),
      changed: true,
    },
    {
      name: '日志页：点回「任务」⇒ 行都回来',
      do: `(async () => {
        const btn = document.querySelector('[aria-label="类别：任务"]');
        if (!btn) throw new Error('没找到类别按钮「任务」');
        btn.click();
        await new Promise((r) => setTimeout(r, 300));
        return true;
      })()`,
      check: eq(
        `document.querySelectorAll('#logs-scroll > div[data-k]').length > 5`,
        true,
      ),
      changed: true,
    },
    {
      name: '日志页：点某账号 ⇒ 只剩它的行',
      do: `(async () => {
        const btn = document.querySelector('[aria-label="账号：主力号"]');
        if (!btn) throw new Error('没找到账号筛选按钮「主力号」');
        btn.click();
        await new Promise((r) => setTimeout(r, 300));
        return true;
      })()`,
      check: eq(
        `(() => {
          const rows = [...document.querySelectorAll('#logs-scroll > div[data-k]')];
          if (rows.length === 0) return 'no-rows';
          const ids = new Set(rows.map((r) => r.querySelector('[data-acc]')?.dataset.acc ?? ''));
          return JSON.stringify({ only: ids.size === 1 && ids.has('13800000000') });
        })()`,
        JSON.stringify({ only: true }),
      ),
      changed: true,
    },
    {
      name: '日志页：点「全部」⇒ 两个账号的行都回来',
      do: `(async () => {
        const btn = document.querySelector('[aria-label="账号：全部"]');
        if (!btn) throw new Error('没找到账号筛选按钮「全部」');
        btn.click();
        await new Promise((r) => setTimeout(r, 300));
        return true;
      })()`,
      check: eq(
        `(() => {
          const rows = [...document.querySelectorAll('#logs-scroll > div[data-k]')];
          const ids = new Set(rows.map((r) => r.querySelector('[data-acc]')?.dataset.acc ?? ''));
          return JSON.stringify({ both: ids.has('13800000000') && ids.has('13900000001') });
        })()`,
        JSON.stringify({ both: true }),
      ),
      changed: true,
    },

    /**
     * ── 日志页：**分页**（2026-10-09 用户口径「大日志做虚拟加载或者分页，
     *    避免一次加载太多」✓）──────────────────────────────────────────────
     *
     * 夹具那边是**两层**（`lib.mjs` 的 `logsState` ✓）：盘上 **2600** 行，
     * 而不带 `before` 的实时那一份只给**最新 2000 行**（= 页面要的 `RING_LINES` ✓）——
     * 这正是真后端的样子（内存环 + 盘上按需读 ✓）。于是：
     *
     * * 首屏「已显示 **2000** 行」（live 只有那么多 ✓）；
     * * 点一次「加载更早的 500 行」⇒ 取回**满 500 行**（seq 更小的那一段 ✓）
     *   ⇒ 计数 **2500**、最上面那一行的行号**小了 500** ✓；
     * * 再点一次 ⇒ 只剩 100 行出头（**不满一页** ✓）⇒ 按钮撤掉（到头了 ✓）。
     *
     * ⚠ 钉四件事，**都要真的取到更早的行**，不能只断言"按钮在"：
     * ① 按钮在、计数写着「已显示 N 行」；
     * ② 点一次 ⇒ 计数**变大**、而且最上面那一行**更旧**（只数行数的话，
     *    "`before` 传错 ⇒ 同一段行又取了一遍"也会让行数变多 ✗ —— 夹具正文里带行号
     *    正是为了能逐字对上 ✓，与历史页那条同一个手法 ✓）；
     * ③ ⭐ **跨一次轮询之后，翻上来的历史行还在最上面** —— 这一条是这个功能的命门：
     *    轮询只许动 `live` ✓，冲掉 `older` 的表现正是"用户翻上去看完历史、
     *    下一次轮询把历史全抹了"✗（`logs-page.tsx` 文件头第 1 条 ✓）；
     * ④ 翻到不满一页 ⇒ 按钮**整颗撤掉**（`hasMore` ✓）。
     *
     * ⚠ 面板是窗口化的（只铺看得见的那几十行 ✓）⇒ 要读"最上面那一行"得先滚到顶；
     * 滚动与等待放在 `do` 里、断言放在 `check` 里（`check` 不许改页面 ✓）。
     */
    {
      name: '日志页：分页按钮在，且计数写着「已显示 N 行」',
      do: 'true',
      check: eq(
        `(() => {
           const t = document.getElementById('app').textContent ?? '';
           const btn = [...document.querySelectorAll('#app button')]
             .find((b) => b.textContent.trim() === '加载更早的 500 行');
           return JSON.stringify({ btn: !!btn, count: /已显示 \\d+ 行/.test(t) });
         })()`,
        JSON.stringify({ btn: true, count: true }),
      ),
    },
    {
      name: '日志页：点「加载更早的 500 行」⇒ 计数变大、最上面那行更旧（真取到更早的行）',
      do: `(async () => {
        const readCount = () =>
          Number((document.getElementById('app').textContent.match(/已显示 (\\d+) 行/) ?? [])[1]);
        const readTop = () => {
          const row = document.querySelector('#logs-scroll > div[data-k]');
          return Number((row ? row.textContent.match(/第 (\\d+) 行/) : [])?.[1]);
        };
        // 先滚到顶（并**等窗口真的落在那儿** ✓）：窗口化只铺看得见的那几十行，
        // "最上面那一行"得先让它出来才读得到 ✓
        await ${logsToTop};
        window.__countBefore = readCount();
        window.__topBefore = readTop();
        if (!(window.__countBefore > 0) || !(window.__topBefore > 0)) {
          throw new Error('没读到「已显示 N 行」/ 最上面那一行：' + window.__countBefore + '/' + window.__topBefore);
        }
        const btn = [...document.querySelectorAll('#app button')]
          .find((b) => b.textContent.trim() === '加载更早的 500 行');
        if (!btn) throw new Error('没有「加载更早」按钮');
        btn.click();
        for (let i = 0; i < 80; i++) {
          if (readCount() > window.__countBefore) break;
          await new Promise((r) => setTimeout(r, 100));
        }
        // 拼接之后视图停在"原来那段内容"上 ⇒ 要再滚到顶才看得到新拼上去的那几行 ✓
        await ${logsToTop};
        window.__countAfter = readCount();
        window.__topAfter = readTop();
        return true;
      })()`,
      check: eq(
        `(() => {
           const grew = window.__countAfter - window.__countBefore;
           // 夹具那边：实时那一份 2000 行 + 翻回来的满 500 行 ⇒ 正好 500 ✓；
           // 而最上面那一行的行号要**恰好小 500**（"更早的那一段" ✓，不是同一段取两遍 ✗）
           return JSON.stringify({ by: grew, topDrop: window.__topBefore - window.__topAfter });
         })()`,
        JSON.stringify({ by: 500, topDrop: 500 }),
      ),
      changed: true,
    },
    {
      /**
       * ⭐⭐ 这一条是这个功能的**命门**（`logs-page.tsx` 文件头第 1 条 ✓）：
       * 轮询每 2 s 换一次 `live`，**不许**把用户翻出来的 `older` 冲掉 ——
       * 冲掉的表现就是"翻上去看完历史，下一次轮询历史全没了"✗。
       *
       * 判据取"最上面那一行的行号还等于翻页之后那个"（而不是"行数没变"）：
       * 行数在轮询里**本来就该变**（live 每拍多两行 ✓），而"最上面那一行"只有
       * 在 `older` 被抹掉时才会突然变回实时窗口的最旧那一行 ✓。
       */
      name: '日志页：跨一次轮询后，翻上来的历史行还在最上面（轮询只动 live）',
      do: `(async () => {
        // 2.4 s > 那一拍 2 s ⇒ 至少跨过一次轮询 ✓
        await new Promise((r) => setTimeout(r, 2400));
        await ${logsToTop};
        return true;
      })()`,
      check: eq(
        `(() => {
           const row = document.querySelector('#logs-scroll > div[data-k]');
           const top = Number((row ? row.textContent.match(/第 (\\d+) 行/) : [])?.[1]);
           const n = Number((document.getElementById('app').textContent.match(/已显示 (\\d+) 行/) ?? [])[1]);
           return JSON.stringify({ kept: top === window.__topAfter, liveGrew: n >= window.__countAfter });
         })()`,
        JSON.stringify({ kept: true, liveGrew: true }),
      ),
    },
    {
      name: '日志页：再点一次 ⇒ 回不满一页（到头了）⇒ 按钮整颗撤掉',
      do: `(async () => {
        const find = () => [...document.querySelectorAll('#app button')]
          .find((b) => b.textContent.trim().startsWith('加载更早'));
        const btn = find();
        if (!btn) throw new Error('没有「加载更早」按钮');
        btn.click();
        for (let i = 0; i < 80; i++) {
          if (!find()) break;
          await new Promise((r) => setTimeout(r, 100));
        }
        return true;
      })()`,
      check: eq(
        `(() => {
           const t = document.getElementById('app').textContent ?? '';
           const btn = [...document.querySelectorAll('#app button')]
             .some((b) => b.textContent.trim().startsWith('加载更早'));
           const n = Number((t.match(/已显示 (\\d+) 行/) ?? [])[1]);
           // ⚠ 三条一起读：按钮撤了（到头 ✓）、计数还在（它是"翻到底了"的证据 ✓）、
           // 而且第二次翻页**真的又取到了行**（只判按钮消失的话，"游标算错 ⇒ 取回空页"
           // 也会让按钮消失 ✗ —— 那样两次点击加起来只翻了一页，功能是半坏的 ✓）
           return JSON.stringify({ btn, count: /已显示 \\d+ 行/.test(t), grew: n > window.__countAfter });
         })()`,
        JSON.stringify({ btn: false, count: true, grew: true }),
      ),
      changed: true,
    },

    /**
     * ⚠ 登出确认放在**最后**：它是破坏性的（确认后应用回到登录页），
     * 放在中间会让后面每一步都找不到元素。第一版就踩了：18 步"两侧一致"，
     * 其实是**两侧都没执行**。
     */
    { name: '回到设置页', do: go('settings'), check: eq('location.pathname', '/settings') },
    { name: '再开一次登出确认框', do: clickSel('[aria-label="退出登录"]'), check: eq('[...document.querySelectorAll("dialog[open]")].map((d) => d.id).join(",")', 'confirm-logout') },
    { name: '确认登出 ⇒ 闸门回登录页', do: clickText('#confirm-logout .modal-action button', '退出'), check: eq('!!document.querySelector("input[autocomplete=username]")', true), changed: true },
    // 登出失败（fixture 的 POST /api/session 404）被 `doLogout` 吞掉是**故意的**：
    // cookie 可能本来就没了吧，停在原地会让用户以为自己还在登录态。所以这里没有错误弹窗，
    // 断言的是"外壳已经拆干净"——drawer/dock 都不在了。
    { name: '登出后：外壳已拆掉', do: null, check: eq('!!document.querySelector("#app .dock")', false) },
  ],
  /**
   * 未初始化：建管理员向导。**先填一个短口令撞本地校验**，再填合法口令提交成功 ——
   * 后者会让 `authState` 翻成 `ready`，也就是**闸门放行**：`{authGate()}` 那个片段槽
   * 从"向导"换成"完整外壳"（几十个组件 + 各自的 onMounted + 取数）。
   * 这条正是当初整包挂掉的路径，必须正面测一遍，而不是只测"提交失败"。
   */
  setup: [
    { name: '首屏（未初始化）', do: null, check: eq('!!document.querySelector("input[autocomplete=username]")', true) },
    { name: '填用户名', do: typeIn('input[autocomplete="username"]', 'admin'), check: eq('document.querySelector("input[autocomplete=username]").value', 'admin') },
    { name: '填短口令（撞本地校验）', do: typeNth(PWDS, 0, 'abc'), check: eq(nthValue(PWDS, 0), 'abc') },
    { name: '填确认口令', do: typeNth(PWDS, 1, 'abc'), check: eq(nthValue(PWDS, 1), 'abc') },
    {
      // ⚠ 2026-10-08：本地校验失败改成"**弹窗优先 + 就近**"⇒
      // ① 按钮上方那一句内联还在 ✓ ② toast 也出现、文案对得上 ✓
      //（`ToastView` 2026-10-08 起挂在**根上**，所以未登录态也有 toast —— 这一条正是那个前提的门禁 ✓）
      name: '提交（本地校验拦下：太短 ⇒ 内联 + toast 都要有）',
      do: clickText('button', '创建并进入'),
      check: eq(
        `(() => {
           const inline = document.querySelector('#app .alert-error')
           const toast = [...document.querySelectorAll('#app .toast .alert')].map((x) => x.textContent).join('|')
           return [!!inline && inline.textContent.includes('口令至少'), toast.includes('口令至少')].join('/')
         })()`,
        'true/true',
      ),
    },
    {
      // 同上：错误类 toast 不自动消失，用例自己收掉（也钉住 ✕ 有效 ✓）
      name: '关掉那条错误 toast',
      do: `(async () => {
        ;[...document.querySelectorAll('#app .toast button[aria-label="关闭提示"]')].forEach((b) => b.click())
        await new Promise((r) => setTimeout(r, 200))
        return true
      })()`,
      check: eq(`document.querySelectorAll('#app .toast .alert').length`, 0),
      changed: true,
    },
    { name: '改成长口令', do: typeNth(PWDS, 0, 'secret-1234'), check: eq(nthValue(PWDS, 0), 'secret-1234') },
    { name: '确认口令跟上', do: typeNth(PWDS, 1, 'secret-1234'), check: eq(nthValue(PWDS, 1), 'secret-1234') },
    { name: '提交成功 ⇒ 闸门放行（外壳出现）', do: clickText('button', '创建并进入'), check: eq('!!document.querySelector("#app .dock")', true), changed: true },
    { name: '放行后：切到历史页', do: go('history'), check: eq('location.pathname', '/history'), changed: true },
    // ⚠ 放行后外壳是**异步**长起来的，主题按钮可能还没出现 —— 但本框架的挂载是同步的，
    // 而 `waitStable` 已经等过一轮，所以这里要求它**必须**在（不在就是没放行完全）
    { name: '放行后：切主题', do: clickSel('[aria-label^="切换到"]'), check: eq("!!document.documentElement.getAttribute('data-theme')", true) },
    // ── 历史页：筛选与搜索（2026-10-01 改版新增）──
    // fixture 里是 **4** 条（3 成功 + 1 失败）——
    // 第 3 条是 2026-10-02 为「历史页复用结构化明细」加的（带 `details.liveRoom`）；
    // 第 4 条是同一天为**兑换**加的（`details` 是**铺平**的那一份，`kind: exchange`）——
    // 用户口径「所有内容都要进记录」⇒ 加记录时这条计数断言会红，那是设计如此。
    // ⚠ 胶囊的可见文字带计数（「失败 1」），
    // 所以按 `aria-label` 点它 —— `clickText` 是精确匹配。
    { name: '历史页：四条记录都在', do: null, check: eq('document.querySelectorAll("#app table tbody tr").length', 4) },
    { name: '历史页：切「失败」档只剩 1 条', do: clickSel('[aria-label="筛选：失败"]'), check: eq('document.querySelectorAll("#app table tbody tr").length', 1), changed: true },
    { name: '历史页：搜索无结果 ⇒ 是"没有符合条件"而不是"还没有记录"', do: typeIn('#app input[type=search]', 'zzz'), check: has(APP_TEXT, '没有符合条件的记录'), changed: true },
    { name: '历史页：清除筛选回到全部', do: clickText('button', '清除筛选'), check: eq('document.querySelectorAll("#app table tbody tr").length', 4), changed: true },
  ],
  /**
   * 已初始化未登录：口令错走错误分支，口令对则**闸门从登录页翻到外壳**
   * （`AuthPage` 卸载、drawer 建起来）。
   */
  login: [
    { name: '首屏（未登录）', do: null, check: eq('!!document.querySelector("input[autocomplete=username]")', true) },
    { name: '填用户名', do: typeIn('input[autocomplete="username"]', 'admin'), check: eq('document.querySelector("input[autocomplete=username]").value', 'admin') },
    { name: '填错口令', do: typeNth(PWDS, 0, 'wrong-pass'), check: eq(nthValue(PWDS, 0), 'wrong-pass') },
    {
      // ⚠ 2026-10-08：登录失败同样"弹窗优先 + 就近"⇒ 按钮上方那句 + 一条 toast 都要在 ✓
      name: '提交（口令错 ⇒ 错误分支：内联 + toast）',
      do: clickText('button', '登录'),
      check: eq(
        `(() => {
           const inline = (document.getElementById('app')?.textContent ?? '').includes('用户名或口令不正确')
           const toast = [...document.querySelectorAll('#app .toast .alert')].map((x) => x.textContent).join('|')
           return [inline, toast.includes('用户名或口令不正确')].join('/')
         })()`,
        'true/true',
      ),
    },
    { name: '填对口令', do: typeNth(PWDS, 0, 'right-pass'), check: eq(nthValue(PWDS, 0), 'right-pass') },
    { name: '提交成功 ⇒ 闸门放行（外壳出现）', do: clickText('button', '登录'), check: eq('!!document.querySelector("#app .dock")', true), changed: true },
    { name: '放行后：切到账号页', do: go('accounts'), check: eq('location.pathname', '/accounts'), changed: true },
    { name: '放行后：展开抽屉', do: clickSel('label[for="nav-drawer"]'), check: eq("document.getElementById('nav-drawer').checked", true), changed: true },
  ],
}

/** 等 DOM 稳定：连续两次探测一致就算稳定 —— 本地 fixture 足够快。 */
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
      if (++same >= 1) return
    } else {
      same = 0
      last = now
    }
    await sleep(80)
  }
}

/**
 * 截图前**先关掉所有 transition / animation**：否则同一份代码连跑两次会截到过渡中的
 * 不同帧，人工看图时容易被"看着不一样"带走。关掉之后截图是确定的，便于事后核对。
 */
const NO_ANIM = `(() => { const s = document.createElement('style'); s.textContent = '*,*::before,*::after{transition:none !important;animation:none !important}'; document.head.appendChild(s); return true })()`

const shoot = async (session, file) => {
  const r = await session.cdp.send('Page.captureScreenshot', { format: 'png' })
  fs.mkdirSync('/tmp/shots', { recursive: true })
  fs.writeFileSync(file, Buffer.from(r.data, 'base64'))
}

const firstLine = (e) => String(e?.message ?? e).split('\n')[0]

/** SIGKILL 之后**等它真的退出**再往下走 —— 理由见下面 `removeProfile`。 */
const killAndWait = (child, ms = 2000) =>
  new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode) {
      resolve()
      return
    }
    const timer = setTimeout(resolve, ms)
    child.once('exit', () => {
      clearTimeout(timer)
      resolve()
    })
    try {
      child.kill('SIGKILL')
    } catch {
      // 已经死了
    }
  })

/**
 * 删掉本次运行的一次性 chrome profile。
 *
 * ⚠ 三条讲究，全是 2026-10-01 踩出来的：
 * 1. `SIGKILL` 之后 chrome 还可能再写一两个文件，裸 `rmSync` 会 `ENOTEMPTY`
 *    （readdir 与 rmdir 之间又冒出一个文件）⇒ 交给 Node 自己重试，
 *    `maxRetries`/`retryDelay` 正是为 ENOTEMPTY/EBUSY/EPERM 这类比赛准备的。
 * 2. 删不掉**不算验收失败**：判据是页面行为，而它只是个 tmp 目录。
 * 3. 但**也不许静默**：留一行 ⚠，别让 tmp 里的垃圾变成无人认领的谜。
 */
const removeProfile = () => {
  if (!ownedProfile) return
  try {
    fs.rmSync(PROFILE, { recursive: true, force: true, maxRetries: 8, retryDelay: 60 })
  } catch (e) {
    console.log(`⚠ 临时 profile 没能删掉（不影响判据）：${PROFILE} —— ${firstLine(e)}`)
  }
}

const server = await serve(dir, PORT, variant)
// ⚠ 端口由内核给（PORT=0 时），必须从这里读回来交给 chrome 加载
const port = server.address().port
/**
 * 起不来也要**收尸**：fixture 服务与一次性 profile 都别留给下一次运行。
 * （旧版这两行不在 try 里，`openSession` 一抛，服务与 profile 就留在那儿了。）
 */
let session
try {
  session = await openSession({ port, route, debugPort: DEBUG, profile: PROFILE })
} catch (e) {
  server.close()
  removeProfile()
  throw e
}
let bad = 0
let checks = 0
try {
  await waitStable(session)
  // ⚠ 注入必须在**首屏之后**、且只注入一次：页面不重载，所以它一直有效
  await session.cdp.eval(HELPERS)
  await session.cdp.eval(NO_ANIM)
  await shoot(session, `/tmp/shots/interact-${variant}.png`)
  console.log(`交互验收（variant=${variant}，产物 ${dir}，${STEPS[variant].length} 步）`)
  let prev = await snapshot(session)
  for (const step of STEPS[variant]) {
    if (only && !step.name.includes(only)) continue
    /**
     * ⚠ 单步失败**不能**把整轮带崩：抛错记在这步的结论里继续跑，
     * 最后按"有几步没过"退出。那比"跑到第 20 步炸了、前面 19 步的结论全丢"有用。
     */
    const problems = []
    if (step.do) {
      try {
        await session.cdp.eval(step.do)
      } catch (e) {
        problems.push(`这步没真正执行：${firstLine(e)}`)
      }
    }
    await waitStable(session)
    const now = await snapshot(session)
    if (step.changed && now === prev) problems.push('界面没变（点了但不动）')
    if (step.check) {
      checks++
      let got
      let evalFailed = false
      try {
        got = await session.cdp.eval(step.check.expr)
      } catch (e) {
        evalFailed = true
        problems.push(`断言求值抛错：${firstLine(e)}`)
      }
      if (!evalFailed && !matchCheck(got, step.check)) {
        problems.push(`断言不符（${step.check.kind} ${JSON.stringify(step.check.want)}，读到 ${JSON.stringify(got)}）`)
      }
    }
    prev = now
    if (problems.length) {
      bad++
      console.log(`  ✗ ${step.name}`)
      for (const p of problems) console.log(`     ${p}`)
    } else {
      console.log(`  ✅ ${step.name}${step.check ? '' : '（只要求不抛错）'}`)
    }
  }
  // 收尾再截一张：出问题时先看图，比翻快照 JSON 快
  await shoot(session, `/tmp/shots/interact-${variant}-end.png`)
} finally {
  session.cdp.close()
  server.close()
  // ⚠ **先等 chrome 真的退出**再删 profile：不等就是上面 `removeProfile` 注释里那个 ENOTEMPTY
  await killAndWait(session.child)
  removeProfile()
}

/**
 * 页面异常一律判负 —— 这是"补门"的核心。
 *
 * 旧版把它放在最后打印但**不计入** `bad`，于是"应用抛了未捕获异常"可以全绿；
 * 而本框架里未捕获异常最典型的成因正是 effect 死循环（`MAX_NESTING` 那条护栏
 * 抛的就是它）—— 那必须让门禁红。
 * ⚠ favicon 404 会被算成 console.error，与框架无关，滤掉。
 */
const pageErrors = session.errors.filter((e) => !e.includes('favicon'))
if (pageErrors.length) {
  bad += pageErrors.length
  console.log(`\n页面异常：✗ ${pageErrors.length} 条`)
  for (const e of pageErrors.slice(0, 5)) console.log(`  ${firstLine(e)}`)
} else {
  console.log('\n页面异常：✅ 0 条')
}

if (bad) {
  console.log(`\n✗ ${bad} 项未通过（${checks} 条断言）；截图在 /tmp/shots/interact-${variant}*.png`)
  process.exitCode = 1
} else {
  console.log(`\n✓ ${STEPS[variant].length} 步全部通过（${checks} 条断言，0 条页面异常）`)
}
