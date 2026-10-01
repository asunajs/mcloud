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
const pickOption = (sel, value) =>
  `(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) throw new Error('找不到下拉框: ' + ${JSON.stringify(sel)}); el.value = ${JSON.stringify(value)}; el.dispatchEvent(new Event('change', { bubbles: true })); return el.value })()`
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
  window.__stepOrder = () => window.__stepRows().map((d) => d.querySelector('select').value).join(',');
  window.__stepBadges = () => window.__stepRows().map((d) => d.querySelector('.badge').textContent.replace(/\\s+/g, '')).join(',');
  /**
   * 某个弹窗里"客户端形态"那几个按钮有几个。
   * ⚠ 弹窗里还有一排 join（方式 tab），所以按**文字**过滤，
   * 不能只数 .join button。
   */
  window.__platformBtns = (id) => [...document.querySelectorAll('#' + id + ' .join button')]
    .filter((b) => ['Windows', 'macOS'].includes(b.textContent.trim())).length;
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
    { name: '账号页：点「账号密码」方式', do: clickText('#add-account button', '账号密码'), check: eq('!!document.querySelector("#add-account input[type=password]")', true), changed: true },
    { name: '账号页：输入用户名', do: typeIn('#add-account input[type="text"].input', 'someone'), check: eq('document.querySelector("#add-account input[type=text].input").value', 'someone') },
    { name: '账号页：输入口令', do: typeIn('#add-account input[type="password"]', 'pw-123456'), check: eq('document.querySelector("#add-account input[type=password]").value', 'pw-123456') },
    { name: '账号页：切到「凭据」方式', do: clickText('#add-account button', '凭据'), check: eq('!!document.querySelector("#add-account textarea")', true), changed: true },
    { name: '账号页：pc 凭据 ⇒ 有形态选择', do: typeIn('#add-account textarea', 'Basic cGM6MTk5MDAwMDAwMDE6YXxifGN8MTc5MzAwMDAwMDAwMHxl'), check: eq('window.__platformBtns("add-account")', 2), changed: true },
    { name: '账号页：mobile 凭据 ⇒ 无形态选择', do: typeIn('#add-account textarea', 'Basic bW9iaWxlOjE5OTAwMDAwMDAxOmF8YnxjfDE3OTMwMDAwMDAwMDB8ZQ=='), check: eq('window.__platformBtns("add-account")', 0), changed: true },
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
    { name: '计划页：选任务', do: pickOption('select.select', 'live-room'), check: eq('document.querySelector("select.select").value', 'live-room') },
    { name: '计划页：勾选启用', do: clickSel('input[type="checkbox"].checkbox'), check: eq('document.querySelector("input[type=checkbox].checkbox").checked', false), changed: true },

    // 编排页：步骤列表的**增 / 删 / 改序**（本框架里最容易出错的一块 —— createFor + 位置敏感行）
    { name: '切到编排页', do: go('#/pipelines'), check: eq('location.hash', '#/pipelines') },
    { name: '编排页：点「编辑」（草稿带 2 步）', do: clickText('button', '编辑'), check: has(APP_TEXT, '共 2 步'), changed: true },
    { name: '编排页：初始顺序 = 签到 → 直播', do: null, check: eq('window.__stepOrder()', 'daily-checkin,live-room') },
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

    // 任务页：下拉框的选项是**由数据 createFor 出来的**，选它等于测"列表项 + select 回写"
    { name: '切到任务页', do: go('#/tasks'), check: eq('location.hash', '#/tasks') },
    { name: '任务页：选账号', do: pickOption('select.select', '13800000000'), check: eq('document.querySelector("select.select").value', '13800000000') },

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
