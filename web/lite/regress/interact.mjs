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
 *   node lite/regress/interact.mjs [--variant ready|login|setup] [--only 关键字] [--dir dist]
 *
 * 退出码非 0 = 有步骤没执行 / 断言不符 / 页面抛过异常。
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { appSubtree, arg, hashRoute, openSession, readVariant, serve, sleep } from './lib.mjs'

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
const route = hashRoute('settings')

/**
 * 页面里的快照。`dom` 走 `lib.mjs` 的 `appSubtree` 归一化，所以"变了没有"有一个稳定口径。
 *
 * ⚠ 表单那些是 **property**：`outerHTML` 看不到，但它们是用户看得见的状态
 * （受控输入框有没有被回写成旧值，全靠这里抓到）。
 */
const PROBE = `JSON.stringify({
  hash: location.hash,
  theme: document.documentElement.getAttribute('data-theme'),
  openDialogs: [...document.querySelectorAll('dialog[open]')].map((d) => d.id).join(','),
  form: [...document.querySelectorAll('input,select,textarea')].map((e) =>
    (e.type === 'checkbox' || e.type === 'radio') ? e.checked : e.value).join('|'),
  app: (document.getElementById('app')?.outerHTML ?? ''),
})`

const snapshot = async (s) => {
  const j = JSON.parse(await s.cdp.eval(PROBE))
  return JSON.stringify({ hash: j.hash, theme: j.theme, openDialogs: j.openDialogs, form: j.form, app: appSubtree(j.app) })
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
const go = (hash) => `(() => { location.hash = ${JSON.stringify(hash)}; return location.hash })()`

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
const not = (expr, needle) => ({ expr, want: needle, kind: 'not' })

const matchCheck = (got, { want, kind }) => (kind === 'eq' ? got === want : kind === 'has' ? String(got).includes(want) : !String(got).includes(want))

const ROUTES = ['dashboard', 'accounts', 'tasks', 'exchange', 'live-room', 'schedules', 'pipelines', 'history', 'settings']

const STEPS = {
  /** 已登录：切页、导航、抽屉、主题、弹窗、受控输入框、下拉框、勾选框、编排页的增删重排。 */
  ready: [
    { name: '首屏（已登录）', do: null, check: eq('!!document.querySelector("#app .dock")', true) },
    // 切页：每条路由都走一遍（首屏回归是"直接打开该路由"，这里是"**切过去**"，
    // 走的是 page 条件分支 + 新页组件创建 + onMounted + 各页自己的取数）
    ...ROUTES.map((r) => ({ name: `切到 ${r}`, do: go(`#/${r}`), check: eq('location.hash', `#/${r}`), changed: true })),
    { name: '点导航（抽屉）：历史', do: clickText('ul.menu button', '历史'), check: eq('location.hash', '#/history') },
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
    { name: '点 dock：任务', do: clickText('.dock button', '任务'), check: eq('location.hash', '#/tasks') },
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
    { name: '切到账号页', do: go('#/accounts'), check: eq('location.hash', '#/accounts') },
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
    // 由 `lite/regress/narrow.mjs` 按 375 宽单独验 —— 这里验不了视口）。
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

    // 计划页：新建表单填一遍（文本、下拉、勾选框都覆盖到）
    { name: '切到计划页', do: go('#/schedules'), check: eq('location.hash', '#/schedules') },
    { name: '计划页：填名称', do: typeIn('input[placeholder="例如 每天签到"]', '每周签到'), check: eq('document.querySelector("input[placeholder=\\"例如 每天签到\\"]").value', '每周签到') },
    { name: '计划页：改 cron', do: typeIn('input[placeholder="0 8 * * *"]', '0 9 * * 1'), check: eq('document.querySelector("input[placeholder=\\"0 8 * * *\\"]").value', '0 9 * * 1') },
    // ⚠ 这条同时盯住"选中的值没被别的绑定回写成空串" —— 上一代对拍里 Vue 侧正是回写成了空
    { name: '计划页：选任务', do: pickOption('#sched-task', 'live-room'), check: eq('document.querySelector("#sched-task").dataset.value', 'live-room') },
    { name: '计划页：勾选启用', do: clickSel('input[type="checkbox"].checkbox'), check: eq('document.querySelector("input[type=checkbox].checkbox").checked', false), changed: true },
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

    // 编排页：步骤列表的**增 / 删 / 改序**（本框架里最容易出错的一块 —— createFor + 位置敏感行）
    { name: '切到编排页', do: go('#/pipelines'), check: eq('location.hash', '#/pipelines') },
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
    { name: '编排页：取消（回列表，不发请求）', do: clickText('#app .card-actions button', '取消'), check: not(APP_TEXT, '共 2 步'), changed: true },

    // 直播页：口令 textarea、时长数字框、开关
    { name: '切到直播页', do: go('#/live-room'), check: eq('location.hash', '#/live-room') },
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

    // 任务页：账号选择器（2026-10-01 起是「多选 + 全部账号」；2026-10-02 起换成
    // TDesign 口径的自建下拉 `ui/select.tsx`）。
    // 选项同样是**由数据 createFor 出来的**，点它等于测"列表项 + 勾选回写"。
    //
    // 选择器结构：面板里 `.select-option`，**第 0 项**是「全部账号」（独立开关），
    // 第 1 项起才是逐个账号；面板挂在 `body`（或弹窗里）的 `.select-portal` 上。
    { name: '切到任务页', do: go('#/tasks'), check: eq('location.hash', '#/tasks') },
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

    // 设置页：账户安全 / 存储信息 / 重启 —— 这三块 2026-10-01 一起补了后端。
    { name: '切到设置页', do: go('#/settings'), check: eq('location.hash', '#/settings'), changed: true },
    { name: '设置页：存储信息报到数据目录', do: null, check: has(APP_TEXT, '数据目录') },
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
      name: '设置页：保存策略 ⇒ 有成功提示',
      do: clickText('button', '保存策略'),
      check: has(APP_TEXT, '策略已保存'),
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

    // ── 账号停用（2026-10-01 用户口径："账号需要增加一个停用功能，
    //    这样在其他功能下拉菜单就不显示"）──
    { name: '账号页：有停用按钮', do: go('#/accounts'), check: has(APP_TEXT, '停用'), changed: true },
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
      do: go('#/exchange'),
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
      do: go('#/accounts'),
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
      name: '账号页：设置弹窗里也没有「运行结果推送」（先取消显示）',
      do: null,
      check: eq(
        `(() => {
           const d = document.querySelector('dialog#account-settings')
           if (!d || !d.open) return 'not-open'
           return d.textContent.includes('运行结果推送')
         })()`,
        false,
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
      do: go('#/tasks'),
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
      name: '任务配置页：能从任务卡进到单独那一页',
      do: `(() => {
        const card = [...document.querySelectorAll('#app .card')].find((c) => c.textContent.includes('直播口令'))
        ;[...card.querySelectorAll('button')].find((b) => b.textContent.trim() === '配置').click()
        return true
      })()`,
      check: has(APP_TEXT, '任务配置'),
      changed: true,
    },
    {
      name: '任务配置页：改「听弹幕时长」为 123 并保存',
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
        location.hash = '#/tasks'
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

    /**
     * ⚠ 登出确认放在**最后**：它是破坏性的（确认后应用回到登录页），
     * 放在中间会让后面每一步都找不到元素。第一版就踩了：18 步"两侧一致"，
     * 其实是**两侧都没执行**。
     */
    { name: '回到设置页', do: go('#/settings'), check: eq('location.hash', '#/settings') },
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
    { name: '提交（本地校验拦下：太短）', do: clickText('button', '创建并进入'), check: eq('!!document.querySelector("#app .alert-error")', true) },
    { name: '改成长口令', do: typeNth(PWDS, 0, 'secret-1234'), check: eq(nthValue(PWDS, 0), 'secret-1234') },
    { name: '确认口令跟上', do: typeNth(PWDS, 1, 'secret-1234'), check: eq(nthValue(PWDS, 1), 'secret-1234') },
    { name: '提交成功 ⇒ 闸门放行（外壳出现）', do: clickText('button', '创建并进入'), check: eq('!!document.querySelector("#app .dock")', true), changed: true },
    { name: '放行后：切到历史页', do: go('#/history'), check: eq('location.hash', '#/history'), changed: true },
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
    { name: '提交（口令错 ⇒ 错误分支）', do: clickText('button', '登录'), check: has(APP_TEXT, '用户名或口令不正确') },
    { name: '填对口令', do: typeNth(PWDS, 0, 'right-pass'), check: eq(nthValue(PWDS, 0), 'right-pass') },
    { name: '提交成功 ⇒ 闸门放行（外壳出现）', do: clickText('button', '登录'), check: eq('!!document.querySelector("#app .dock")', true), changed: true },
    { name: '放行后：切到账号页', do: go('#/accounts'), check: eq('location.hash', '#/accounts'), changed: true },
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
