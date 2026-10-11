/**
 * 账号页：云盘账号的登录与列表。
 *
 * # 与"登录闸"是两件事
 *
 * 这个页面管的是**要跑任务的云盘账号**（`accounts.json`），
 * 不是本实例的管理员（那是登录闸，见 `api.ts` 里 `/api/session` 那组）。
 *
 * # 平台选择放在最上面，且**必须回显**
 *
 * mac 与 win 在请求头、UA 与**落盘数据**上都不一样（`pc_platform` 会存进
 * `accounts.json`）。所以选错平台不是"界面显示不对"，而是这个账号以后
 * 每次请求都用错身份 —— 上游只会回一个含义模糊的业务码。
 * 因此：① 不给默认值靠用户自己看，而是**显式**给两个按钮；
 * ② 选完在按钮下方把"会影响什么"写出来。
 */

import { onMounted, onUnmounted, ref, watch } from '@asunajs/lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  ApiError,
  cancelQrLogin,
  deleteAccount,
  getAccountSettings,
  getPcDevice,
  importAccount,
  listAccounts,
  loginWithPassword,
  pollQrLogin,
  putAccountSettings,
  refreshAuth,
  rotatePcDevice,
  startAppSmsLogin,
  startQrLogin,
  startSmsLogin,
  submitAppSmsLogin,
  submitSmsLogin,
  updateAccount,
  type AccountSettings,
  type AccountSummary,
  type PcPlatform,
  type RefreshResponse,
  type RefreshResult,
  type SliderPayload,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { ConfirmDialog, Dialog, closeDialog, openDialog } from '../ui/dialog'
import { showError } from '../ui/error-dialog'
import { IconAlert } from '../ui/icons'
import { pushToast } from '../ui/toast'
import { createAccountFields } from '../ui/account-fields'
import { pathParts } from '../ui/nav'
import { currentPath, navigate } from '../ui/router'
import { fmtEpochMs } from '../ui/time'
import { AccountConfigPage } from './account-config-page'
// 上游状态：**子视图**，不是导航项（用户 2026-10-07：「这个不要放 nav 里」✓）
import { UpstreamPage } from './upstream-page'

type LoadState = 'loading' | 'error' | 'ready'
type Method = 'qr' | 'sms' | 'app' | 'password' | 'auth'
type QrPhase = 'idle' | 'waiting' | 'scanned' | 'failed'

/**
 * 从凭据串里**猜**平台前缀，**只**用来决定"要不要让用户选客户端形态"。
 *
 * ⚠ 这不是校验：真正的判定在服务端（`accounts::platform_for`），这里猜错的
 * 后果只是多显示/少显示一个选择器。猜不出来一律按"要选"处理 —— PC 是常见情况，
 * 而服务端对 `pc` 凭据**要求**形态，少显示会让提交必然失败。
 */
const credentialPrefix = (raw: string): string => {
  const text = raw.trim().replace(/^Basic\s+/i, '')
  if (!text) return ''
  try {
    return atob(text).split(':', 1)[0] ?? ''
  } catch {
    // 不是合法 base64 ⇒ 交给服务端报错（它的错话更准）
    return ''
  }
}

/** 两种平台的人话名字。`Record` 而不是函数：加平台时 TS 会报缺一项。 */
const PLATFORM_LABEL: Record<PcPlatform, string> = {
  windows: 'Windows',
  macos: 'macOS',
}


/**
 * 账号设置弹窗里那一块（**功能开关**）暂时**不显示**。
 *
 * 用户口径（2026-10-02）：「这些设置先取消显示，设置后面单独弄」。
 *
 * ⚠ 代码**原样留着**：里面是上游字段、取值范围与校验（重做那一版时有依据 ✓），
 * 只是**不渲染** ⇒ 要让它回来，把这里改成 `true` 就行 ✓。
 * ⚠ **别删代码** ✗ —— 用户说的是"先取消显示"，不是"不要了"。
 *
 * ⚠⚠ 原来这里还藏着**运行结果推送**那一块，2026-10-08 按用户口径**整块删掉**了 ✗
 * （「**很显然我说的和云盘账户没有关系**，为啥你这里云盘账户有配置」＋「**丢弃现有情况，
 * 重写**」）：推送渠道收成了**登录用户级**（「推送」页 `/push` ✓），
 * 弹窗里那一块与账号无关 ⇒ 不是"先藏起来"，是**不该在这儿** ✓。
 * 「先取消显示、别删代码」那条只对**功能开关**（真的按账号算）继续有效 ✓。
 */
const SHOW_ACCOUNT_SETTINGS_BLOCKS = false

// ⚠ 本地校验（`parseIntField` / `parseSkipTasks`）已搬去 `../ui/account-fields`
// —— 账号页与配置中心复用**同一份**，别在这里再写第二份（两份实现迟早分叉）。

/**
 * 把账号相关动作（读/写设置、刷新凭据……）的失败翻成**能分辨**的一句话。
 *
 * ⚠ 三种状态**不能**都落到「网络错误」：
 * - **404** = 账号不存在**或**不属于当前登录用户（后端对两种情形回同一个 404，
 *   不泄露存在性）—— 用户该做的是刷新列表 / 重新登录，不是"再点一次"；
 * - **400** = 请求体被后端拒绝（未知键、越界、渠道缺字段、`HH:MM` 形状不对），
 *   **后端原话必须显出来**，否则用户只看到一句无从下手的"保存失败"；
 * - **401** = 登录态失效（`request` 已经让整个应用转去登录页），这里补一句说明。
 *
 * `status === 0` 是 `request` 自己造的"超时/取消"，那时 `message` 已经是人话。
 *
 * ⚠ **502 那一档对"刷新凭据"有特殊含义**：后端在有账号没刷成时回 502，
 * 而 body 里**仍然**是逐条结局（见 [`partialRefresh`] ✓）——
 * 所以刷新那条路要**先**试 `partialRefresh`，解析不出来才走这里 ✓。
 */
const describeSettingsError = (e: unknown, action: string): string => {
  if (e instanceof ApiError) {
    if (e.status === 404) {
      return '这个账号不存在，或者不属于当前登录用户（后端对两种情形回的是同一个 404）。请刷新账号列表后再试。'
    }
    if (e.status === 401) return '登录状态已失效，请重新登录后再' + action + '。'
    if (e.status === 400) return '后端拒绝了这次请求：' + e.message
    if (e.status === 0) return e.message
    return action + '失败（HTTP ' + e.status + '）：' + e.message
  }
  return e instanceof Error ? e.message : action + '失败'
}

/**
 * 从一次**失败**的刷新里把逐条结局捞回来（捞不到回 `null` ✓）。
 *
 * # 为什么后端在 502 时还把结局放在 body 里
 *
 * 「有账号没刷成」这件事**必须让用户看见**（是哪一个、为什么 ✓），
 * 而 502 只是个状态码 —— 把 body 丢掉，用户拿到的就是一句"服务器出错了" ✗。
 * 前端这边同理：`request()` 把 body 原文塞进 `Error.message` ✓ ⇒ 这里试着解析它 ✓。
 *
 * ⚠ 解析失败**不报错、不抛**（回 `null` 让调用方走通用错误提示 ✓）：
 * 一个形状对不上的 body 只该让提示"降级"，不该让页面崩 ✓。
 */
const partialRefresh = (e: unknown): RefreshResponse | null => {
  if (!(e instanceof ApiError) || e.status !== 502) return null
  try {
    const parsed = JSON.parse(e.message) as RefreshResponse
    if (parsed && Array.isArray(parsed.results)) return parsed
  } catch {
    // 不是 JSON（或形状不对）⇒ 交给通用提示 ✓
  }
  return null
}

export const AccountsPage = () => {
  /**
   * 当前地址的路径段。
   *
   * ⚠ **本页自己盯着地址**，而不是让路由表来管：路由表把 `/accounts/settings` 与
   * `/accounts` 都归到「账号」这一页（账号配置不是独立导航项 —— 侧栏条数封顶），
   * 于是从列表切到配置时 `page` **没变**，`app.tsx` 那条动态 import 不会再跑一次。
   * 想让它工作只有两条路：把账号配置做成第 10 条导航项（规范里封顶，不行），
   * 或者本页自己听地址变化换视图（就是这里，与 `tasks-page.tsx` 的配置中心同款）。
   *
   * ⚠ 2026-10-08 起盯的是**路由的 `currentPath`**，不再是本页自己挂的 `popstate`
   * 监听器 —— 原先那套依赖"`navigate()` 里补发一次合成 `popstate`"，而那个补发
   * 随着路由原语搬进 lite 已经没有了（lite 的 `push()` 只同步自己的信号）。
   * 改成 `watch(currentPath)` 反而更短，且**不必再手写摘监听器**（组件卸载即停）。
   */
  const path = ref<string[]>(pathParts(currentPath.value))
  watch(currentPath, (p) => {
    path.value = pathParts(p)
  })

  // ── 账号列表 ─────────────────────────────────────────────────────────
  const state = ref<LoadState>('loading')
  const loadError = ref('')
  const accounts = ref<AccountSummary[]>([])
  const deviceId = ref('')
  /** 待确认删除的账号（确认弹窗用）。 */
  const pendingDelete = ref<AccountSummary | null>(null)
  const deleting = ref(false)

  // ── 编辑账号（弹窗） ────────────────────────────────────────────────
  //
  // 三个字段都是"给了才改"：凭据**从不回显**（服务端也不发它），所以这里
  // 永远是空的 —— 空 = 不动它。这也正是这一层用 PATCH 而不是"整体替换"的原因。
  const editing = ref<AccountSummary | null>(null)
  const editNickname = ref('')
  const editDevice = ref('')
  const editAuth = ref('')
  const editPlatform = ref<PcPlatform>('windows')
  const editError = ref('')
  const savingEdit = ref(false)

  // ── 粘贴凭据添加（添加弹窗里的一个 tab） ────────────────────────────
  const importAuth = ref('')
  const importBusy = ref(false)

  // ── 登录表单 ─────────────────────────────────────────────────────────
  const method = ref<Method>('qr')
  const platform = ref<PcPlatform>('windows')
  /**
   * 动作失败：**弹窗优先 + 就近一句**（2026-10-08 用户口径「所有的提示都是优先弹窗式」）。
   *
   * ⚠ 与旧注释的口径差别：以前这里**只**内联（"错误必须贴着待改的字段，弹窗会把字段推远"）。
   * 现在两样都要 —— `pushToast('error', …)` 负责"一眼看见"（错误类 toast 不自动消失，
   * 见 `ui/toast.ts`），`formError` 那份内联负责"指到具体哪一格"。
   * 这些失败都发生在**用户正在填的表单**里（手机号、验证码、账密、出码）。
   * 列表上的独立动作（删除、换设备指纹）走 `showError`（那是 modal ✓）。
   */
  const formError = ref('')
  /** 失败的那一句同时上屏（toast）+ 落进 `formError`（就近）。 */
  const formFail = (msg: string): void => {
    formError.value = msg
    pushToast('error', msg)
  }
  /**
   * 成功回执。
   *
   * ⚠ 与右上角的 toast **并存**，不是重复：toast 3 秒就没了（§5.11），
   * 而加完账号那一刻人的注意力在"账号列表有没有多一条"上 —— 实测就是
   * 「明明加成功了却像什么都没发生」。这一条**留在页面上**，
   * 直到下一次尝试登录（那时它已经过时了）才清掉。
   */
  const formOk = ref('')
  const busy = ref(false)

  // 扫码
  const qrImage = ref('')
  const qrSessionId = ref('')
  const qrPhase = ref<QrPhase>('idle')
  const qrNote = ref('')
  let pollTimer: number | null = null

  // 短信
  const phone = ref('')
  const smsCode = ref('')
  const smsSent = ref(false)
  const smsRemain = ref<number | null>(null)
  const slider = ref<SliderPayload | null>(null)

  // 账密
  const username = ref('')
  const password = ref('')

  // App 端（手机客户端）短信
  const appPhone = ref('')
  const appCode = ref('')
  const appSent = ref(false)
  const appRemain = ref<number | null>(null)
  /**
   * App 的滑块题：与 PC 的 `slider` 分开存。
   *
   * 两端的题面字段名不同（PC 是 `slider` 嵌套对象，App 是平铺的几个字段），
   * 共用一个 ref 会让"切了方法之后把上一端的答案带过去"变成一个静默的错。
   */
  const appSlider = ref<{ answer?: string; background?: string; width?: number | null } | null>(
    null,
  )

  /** 正在启停的账号 id（按钮期间禁用，避免连点两下发两次相反的请求）。 */
  const enabling = ref<string | null>(null)

  // ── 一键刷新凭据（用户口径 2026-10-11：「可手动一键刷新」✓）───────────
  //
  // # 为什么要二次确认（判断 + 理由，写在代码里免得后人当"多此一举"删掉）
  //
  // 刷新**不是删数据**，但它会**真轮换上游那串凭据**（旧串当场作废 ✓，
  // 见 `mcloud_app::auth_refresh` 模块头：那是**不可逆**动作 ✓）。
  // 而这一页上"编辑 / 设置 / 停用 / 移除"四颗按钮挨着它 ——
  // 手滑一下的代价是"这个账号得重新登录"✗。所以两颗都过一道确认 ✓
  //（与「移除账号」「更换本机设备指纹」同一条处置 ✓；本仓口径：
  //  "重要的（刷全部 / 不可逆）走 modal 确认" ✓）。
  /**
   * 正在刷新的目标：`null` = 没在刷；`''` = **全部账号**；其余 = 那个账号 id。
   *
   * ⚠ 用"目标"而不是一个布尔：界面上有 N+1 个按钮（每个账号一个 + 全部一个），
   * 布尔量说不出"现在是哪一个在忙" —— 表现就是所有按钮一起转圈 ✓。
   */
  const refreshing = ref<string | null>(null)
  /** 待确认的刷新目标（`''` = 全部 ✓）。 */
  const pendingRefresh = ref<string | null>(null)

  const REFRESH_ALL_DIALOG = 'confirm-refresh-all'
  const REFRESH_ONE_DIALOG = 'confirm-refresh-one'

  /** 一颗按钮按下 ⇒ 先弹确认（`''` = 全部 ✓）。 */
  const askRefresh = (id: string) => {
    pendingRefresh.value = id
    openDialog(id === '' ? REFRESH_ALL_DIALOG : REFRESH_ONE_DIALOG)
  }

  /** 确认弹窗里那句"刷的是谁"（人话，不是空串 ✓）。 */
  const refreshTargetLabel = (id: string): string => {
    if (id === '') return `全部 ${accounts.value.length} 个账号`
    const a = accounts.value.find((x) => x.id === id)
    return a ? a.nickname || a.label || a.id : id
  }

  /**
   * 把逐条结局说成一句人话。
   *
   * ⚠ **四档不能合成一句"没刷"**：`skipped`（还剩 N 天，什么都不用做 ✓）与
   * `manual`（库里没有有效的过期时刻 ⇒ **要重新登录** ✓）的下一步完全不同 ✓
   * —— 合成一句，用户就不知道该不该管了 ✗（后端也是为此把四档分开的 ✓）。
   */
  const describeRefresh = (r: RefreshResult): string => {
    switch (r.kind) {
      case 'refreshed':
        return `已换新凭据（新过期 ${fmtEpochMs(r.newExpireMs ?? 0)}）`
      case 'skipped':
        return `未到提前量（还剩 ${(r.remainingDays ?? 0).toFixed(1)} 天），未打上游`
      case 'manual':
        return `需重新登录：库里没有有效的过期时刻`
      case 'failed':
        return `刷新失败：${r.error ?? '（上游没给原因）'}`
    }
  }

  /**
   * 真刷（一个账号 / 全部 ✓）。
   *
   * ⚠ 后端在有账号**刷新失败**时回 **502**，但 body 里**仍然是**那份逐条结局
   *（`request()` 把 body 原文放进 `Error.message` ✓）⇒ 这里在 catch 里也解析一次，
   * 别把"哪一个失败、为什么"丢掉 ✓。
   *
   * ⚠ 刷完**要重新读账号列表**：凭据换了 ⇒ 列表上那行「凭据 X 天后」得跟着变 ✓
   *（不重读的话，用户看到的是刷新前那个过期时间，会以为没生效 ✓）。
   */
  const doRefresh = async () => {
    const target = pendingRefresh.value
    if (target === null) return
    refreshing.value = target
    try {
      const out = await refreshAuth(target === '' ? { all: true } : { account: target })
      closeDialog(target === '' ? REFRESH_ALL_DIALOG : REFRESH_ONE_DIALOG)
      pendingRefresh.value = null
      reportRefresh(out, target)
      await load()
    } catch (e) {
      const partial = partialRefresh(e)
      if (partial) {
        closeDialog(target === '' ? REFRESH_ALL_DIALOG : REFRESH_ONE_DIALOG)
        pendingRefresh.value = null
        reportRefresh(partial, target)
        await load()
      } else {
        showError('刷新凭据失败', describeSettingsError(e, '刷新'))
      }
    } finally {
      refreshing.value = null
    }
  }

  /** 把一份逐条结局报到界面上（toast 说结论 + 弹窗列逐条原因 ✓）。 */
  const reportRefresh = (out: RefreshResponse, target: string) => {
    const who = refreshTargetLabel(target)
    const lines = out.results
      .map((r) => `${r.label || r.account}：${describeRefresh(r)}`)
      .join('\n')
    if (out.failed > 0) {
      // 失败与"需人工"都要用户去看 ⇒ 弹窗（错误 toast 不自动消失，但列不下逐条 ✓）
      showError('刷新凭据：有账号没刷成', `${who}\n\n${lines}`)
      return
    }
    // 成功走**轻量** toast（3 秒自动消失 ✓）——逐条明细放第二行，长了也读得完 ✓
    const summary =
      out.refreshed > 0
        ? `已刷新 ${out.refreshed} 个凭据`
        : `没有需要刷新的凭据（跳过 ${out.skipped} 个）`
    pushToast(out.manual > 0 ? 'warning' : 'success', `${summary}\n${lines}`)
  }

  const load = async () => {
    state.value = 'loading'
    loadError.value = ''
    try {
      // ⚠ 这里要**全部**（含停用）：账号页是唯一能启停的地方，过滤掉就没法启用回来。
      // 各处下拉用的是不带参数的 `listAccounts()`（默认只给启用的）。
      const [list, dev] = await Promise.all([
        listAccounts({ includeDisabled: true }),
        getPcDevice(),
      ])
      accounts.value = list
      deviceId.value = dev.device_id
      state.value = 'ready'
    } catch (e) {
      loadError.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /** 停掉轮询。离开页面、切方法、出码失败都要调 —— 否则会一直问下去。 */
  const stopPolling = () => {
    if (pollTimer !== null) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }

  /** 放弃当前扫码会话（服务端也把状态丢掉）。 */
  const abandonQr = () => {
    stopPolling()
    const sid = qrSessionId.value
    qrSessionId.value = ''
    qrImage.value = ''
    qrPhase.value = 'idle'
    qrNote.value = ''
    // 重新出码 = 一次新的尝试 ⇒ 上一条成功回执已经过时（与其它三条路径同口径）。
    formOk.value = ''
    if (sid) void cancelQrLogin(sid).catch(() => undefined)
  }

  const onMethodChange = (next: Method) => {
    if (next === method.value) return
    abandonQr()
    formError.value = ''
    smsSent.value = false
    slider.value = null
    // App 那一路的状态也要清 —— 否则切回来时会把上一端的滑块答案带着走，
    // 而那个答案是**另一道题**的，提交上去只会浪费一次尝试（接口计失败次数）。
    appSent.value = false
    appSlider.value = null
    method.value = next
  }

  // ── 扫码 ─────────────────────────────────────────────────────────────

  const beginQr = async () => {
    formError.value = ''
    busy.value = true
    abandonQr()
    try {
      const r = await startQrLogin(platform.value)
      qrImage.value = r.image_svg
      qrSessionId.value = r.session_id
      qrPhase.value = 'waiting'
      qrNote.value = `有效期 ${r.ttl_seconds} 秒`
      // 轮询间隔由服务端给：客户端写死一个数会和状态机分叉
      pollTimer = window.setInterval(() => void pollOnce(), r.poll_interval_ms)
    } catch (e) {
      formFail(e instanceof Error ? e.message : '出码失败')
      qrPhase.value = 'failed'
    } finally {
      busy.value = false
    }
  }

  const pollOnce = async () => {
    const sid = qrSessionId.value
    if (!sid) {
      stopPolling()
      return
    }
    try {
      const r = await pollQrLogin(sid)
      // ⚠⚠ **归属校验**：`sid` 是这一拍开始时捕获的，`await` 期间用户可能已经
      // 「重新出码」或切了平台（`abandonQr` + `beginQr`）—— 那时 `qrSessionId`
      // 已经是**另一张码**的了。不校验的话，作废会话的响应会落在新会话的界面上：
      // `logged_in` 那一支尤其糟（把刚出的新码清掉、还弹一条属于上一会话的回执 ✗）。
      // `abandonQr` 只 `cancelQrLogin(sid)` 取消服务端会话，**取消不掉**这次在飞的请求。
      if (sid !== qrSessionId.value) return
      if (r.phase === 'scanned') {
        qrPhase.value = 'scanned'
        qrNote.value = '已扫码，请在手机上确认'
        return
      }
      if (r.phase === 'logged_in') {
        stopPolling()
        qrSessionId.value = ''
        // ⚠ 这三格必须一起清。只清 `qrSessionId` 会留下"二维码还在、
        // 那句『已扫码，请在手机上确认』也还在"的界面 —— 而账号其实已经加好了，
        // 用户看到的是一个**已经完成**的流程仍停在半路。（实测踩过。）
        qrImage.value = ''
        qrNote.value = ''
        qrPhase.value = 'idle'
        const ok = `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`
        // ⚠ 与其它三条登录路径一致：3 秒的 toast 之外再留一条页面回执。
        // 扫码这条尤其需要 —— 人的注意力那时在手机上，toast 很容易整条错过。
        formOk.value = ok
        pushToast('success', ok)
        await load()
        return
      }
      if (r.phase === 'failed') {
        if (r.terminal) {
          stopPolling()
          qrPhase.value = 'failed'
          qrNote.value = r.reason ?? '扫码登录失败'
        } else {
          // 非终态（限流）：退避后接着问，**不**让用户重开一张码
          qrNote.value = r.reason ?? '稍后自动重试…'
          stopPolling()
          const wait = r.retry_after_ms ?? 5000
          pollTimer = window.setTimeout(() => {
            pollTimer = window.setInterval(() => void pollOnce(), 1000)
            void pollOnce()
          }, wait)
        }
      }
    } catch (e) {
      // 网络抖动不该终止会话：记一句、下一拍再问。
      // 只有服务端明确说"会话没了"（404）才停 —— 那时重开一张码才有意义。
      // ⚠ 同一道归属校验：作废会话的 404 不该把**新**会话的界面打成失败。
      if (sid !== qrSessionId.value) return
      const status = (e as { status?: number }).status
      if (status === 404) {
        stopPolling()
        qrPhase.value = 'failed'
        qrNote.value = '会话已失效，请重新出码'
      } else {
        qrNote.value = '网络不稳，正在重试…'
      }
    }
  }

  // ── 短信 ─────────────────────────────────────────────────────────────

  /**
   * 发验证码。
   *
   * `sliderAnswer` 由本函数**自己**回填：上游把答案直接给了出来（`puzzleLeft`
   * 解出来就是坐标），所以这件事实质上是**全自动**的 —— 拿到 `need_slider`
   * 就带答案重发一次，用户只需要等那条短信。
   *
   * ⚠ 只在**第一次**（`sliderAnswer` 为空）自动重试**一次**：答案本身错了、
   * 或上游连着要两次，再自动打下去就成了对着上游刷请求。
   */
  const sendSms = async (sliderAnswer?: string) => {
    formError.value = ''
    if (!phone.value.trim()) {
      formFail('请填写手机号')
      return
    }
    busy.value = true
    try {
      const r = await startSmsLogin(phone.value.trim(), platform.value, sliderAnswer)
      if (r.phase === 'sent') {
        smsSent.value = true
        smsRemain.value = r.remain_get_times ?? null
        slider.value = null
        pushToast('success', '验证码已发送')
      } else {
        const answer = r.slider?.answer
        if (r.phase === 'need_slider' && answer && !sliderAnswer) {
          // 自动过关：不显示图、不要人点。
          pushToast('info', '已自动通过滑块验证，正在发送验证码')
          await sendSms(answer)
          return
        }
        // 只剩两种可能：题面解不出来，或自动重试后上游还要滑块。
        // 图留着只作**诊断**（人一眼看出卡在滑块上），但话必须说清
        // 是工具侧的问题 —— 不能让用户以为该自己去量像素。
        slider.value = r.slider ?? null
        smsSent.value = false
        formFail(
          r.phase === 'slider_exhausted'
            ? '今日滑块验证次数已用完，请明天再试'
            : '滑块题自动解不开（上游没给答案），本次没发出短信',
        )
      }
    } catch (e) {
      formFail(e instanceof Error ? e.message : '发送验证码失败')
    } finally {
      busy.value = false
    }
  }

  const submitSms = async () => {
    formError.value = ''
    formOk.value = ''
    if (!smsCode.value.trim()) {
      formFail('请填写验证码')
      return
    }
    busy.value = true
    try {
      // ⚠ **不**把滑块答案带进这一枪：后端"带了就先重发一次验证码"，
      // 而验证码在上一步（点了「自动完成」之后）**已经发出来了** ⇒
      // 带上去会让用户白发一条短信、并让刚收到的那条失效。
      const r = await submitSmsLogin({
        phone: phone.value.trim(),
        platform: platform.value,
        code: smsCode.value.trim(),
      })
      pushToast('success', `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`,
      )
      formOk.value = `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`
      smsSent.value = false
      smsCode.value = ''
      slider.value = null
      await load()
    } catch (e) {
      formFail(e instanceof Error ? e.message : '登录失败')
    } finally {
      busy.value = false
    }
  }

  // ── App 端（手机客户端）短信 ─────────────────────────────────────────

  /** 发验证码（App 链路）。与 PC 那支同一个道理：滑块由本函数自动过。 */
  const sendAppSms = async (sliderAnswer?: string) => {
    formError.value = ''
    if (!appPhone.value.trim()) {
      formFail('请填写手机号')
      return
    }
    busy.value = true
    try {
      const r = await startAppSmsLogin(appPhone.value.trim(), sliderAnswer)
      if (r.phase === 'sent') {
        appSent.value = true
        appRemain.value = r.remain_get_times ?? null
        appSlider.value = null
        pushToast('success', '验证码已发送')
      } else {
        const answer = r.slider_answer
        // ⚠ App 这一族的档位名是 `need_captcha`，**不是** PC 那个 `need_slider`
        // —— 两条链路的名字不一样，别顺手抄。
        if (r.phase === 'need_captcha' && answer && !sliderAnswer) {
          // 自动过关：不显示图、不要人点。
          pushToast('info', '已自动通过滑块验证，正在发送验证码')
          await sendAppSms(answer)
          return
        }
        appSlider.value = {
          answer: r.slider_answer,
          background: r.slider_background,
          width: r.slider_width,
        }
        appSent.value = false
        formFail(
          r.phase === 'slider_exhausted'
            ? '这道滑块题已作废，请重新发送验证码'
            : '滑块题自动解不开（上游没给答案），本次没发出短信',
        )
      }
    } catch (e) {
      formFail(e instanceof Error ? e.message : '发送验证码失败')
    } finally {
      busy.value = false
    }
  }

  const submitAppSms = async () => {
    formError.value = ''
    formOk.value = ''
    if (!appCode.value.trim()) {
      formFail('请填写验证码')
      return
    }
    busy.value = true
    try {
      const r = await submitAppSmsLogin({
        phone: appPhone.value.trim(),
        code: appCode.value.trim(),
      })
      // App 账号没有 PC 平台，所以提示语里说"手机端"而不是某个平台名 ——
      // 说成 Windows 会让人以为它按 PC 的 UA 发请求。
      pushToast('success', `已添加账号 ${r.account ?? ''}（手机端）`)
      formOk.value = `已添加账号 ${r.account ?? ''}（手机端）`
      appSent.value = false
      appCode.value = ''
      appSlider.value = null
      await load()
    } catch (e) {
      formFail(e instanceof Error ? e.message : '登录失败')
    } finally {
      busy.value = false
    }
  }

  // ── 账密 ─────────────────────────────────────────────────────────────

  const submitPassword = async () => {
    formError.value = ''
    formOk.value = ''
    if (!username.value.trim() || !password.value) {
      formFail('请填写用户名与口令')
      return
    }
    busy.value = true
    try {
      const r = await loginWithPassword(username.value.trim(), password.value, platform.value)
      pushToast('success', `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`,
      )
      formOk.value = `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`
      password.value = ''
      await load()
    } catch (e) {
      formFail(e instanceof Error ? e.message : '登录失败')
    } finally {
      busy.value = false
    }
  }

  // ── 删除与设备 ───────────────────────────────────────────────────────

  const askDelete = (a: AccountSummary) => {
    pendingDelete.value = a
    openDialog('confirm-delete-account')
  }

  const doDelete = async () => {
    const a = pendingDelete.value
    if (!a) return
    deleting.value = true
    try {
      await deleteAccount(a.id)
      closeDialog('confirm-delete-account')
      pushToast('success', `已移除 ${a.nickname || a.label || a.id}`)
      await load()
    } catch (e) {
      showError(
        `无法移除账号 ${a.nickname || a.label || a.id}`,
        e instanceof Error ? e.message : '移除失败',
      )
    } finally {
      deleting.value = false
      pendingDelete.value = null
    }
  }

  /**
   * 启停一个账号。
   *
   * # 为什么不用确认框
   *
   * **可逆**：点错了一下再点回来即可。本仓的确认框留给不可逆的动作（移除账号、
   * 重启服务）—— 给可逆动作也套一层，用户学会的是"闭眼点确认"，那道框就白设了。
   */
  const toggleEnabled = async (a: AccountSummary) => {
    const next = !a.enabled
    const who = a.nickname || a.label || a.id
    enabling.value = a.id
    try {
      await updateAccount(a.id, { enabled: next })
      pushToast(
        'success',
        next ? `已启用 ${who}` : `已停用 ${who} —— 其它页面的下拉里不再出现它`,
      )
      await load()
    } catch (e) {
      showError(
        `${next ? '无法启用' : '无法停用'} ${who}`,
        e instanceof Error ? e.message : '操作失败',
      )
    } finally {
      enabling.value = null
    }
  }

  /**
   * 换设备指纹。
   *
   * ⚠⚠ 它**不可逆**：旧指纹一换，用旧指纹开着的扫码会话立刻失效（登录态要重来）。
   * 用户口径（2026-10-08）：「重要的直接弹窗」⇒ 点按钮先过一道 `ConfirmDialog`，
   * 不再点了就换 ✗（2026-10-08 之前就是直接换的）。
   */
  const rotateDialogId = 'confirm-rotate-device'

  const doRotateDevice = async () => {
    try {
      const r = await rotatePcDevice()
      closeDialog(rotateDialogId)
      deviceId.value = r.device_id
      pushToast('success', '已更换设备指纹（旧的扫码会话立即失效）')
    } catch (e) {
      // 独立动作，没有表单在场；且原因（例如上游拒绝）需要被读到
      closeDialog(rotateDialogId)
      showError('无法更换设备指纹', e instanceof Error ? e.message : '更换失败')
    }
  }

  // ── 编辑账号 ────────────────────────────────────────────────────────

  /** 打开编辑弹窗：把三个输入框预填成"当前值 / 空"。 */
  const askEdit = (a: AccountSummary) => {
    editing.value = a
    // 昵称预填是要**看见并改**的；设备号同理。凭据留空 —— 它从不回显，
    // 空 = 不改（这也是"换凭据"与"改名字"能在同一个弹窗里的原因）。
    editNickname.value = a.nickname ?? ''
    editDevice.value = a.device_id
    editAuth.value = ''
    editError.value = ''
    if (a.pc_platform) editPlatform.value = a.pc_platform
    openDialog('edit-account')
  }

  const doSaveEdit = async () => {
    const a = editing.value
    if (!a) return
    const patch: {
      nickname: string
      deviceId: string
      auth?: string
      platform?: PcPlatform
    } = {
      // 空串是**有意义**的：服务端把它读作"清空昵称"（回落成掩码手机号）。
      nickname: editNickname.value,
      deviceId: editDevice.value,
    }
    const pasted = editAuth.value.trim()
    if (pasted) {
      patch.auth = pasted
      patch.platform = editPlatform.value
    }
    savingEdit.value = true
    editError.value = ''
    try {
      await updateAccount(a.id, patch)
      closeDialog('edit-account')
      pushToast('success', `已保存 ${editNickname.value.trim() || a.label || a.id}`)
      await load()
    } catch (e) {
      // 弹窗**不关**：用户刚打的字还在里面，关掉等于让他重打一遍。
      const msg = e instanceof Error ? e.message : '保存失败'
      editError.value = msg
      // 弹窗优先（2026-10-08 用户口径）：错误类 toast 不自动消失，见 `ui/toast.ts`
      pushToast('error', msg)
    } finally {
      savingEdit.value = false
    }
  }

  // ── 粘贴凭据添加 ────────────────────────────────────────────────────

  const doImport = async () => {
    const raw = importAuth.value.trim()
    if (!raw) {
      formFail('请先粘贴一条凭据')
      formOk.value = ''
      return
    }
    importBusy.value = true
    formError.value = ''
    formOk.value = ''
    try {
      const r = await importAccount({
        auth: raw,
        // 形态只在 pc 凭据上才有意义；服务端对 pc **要求**它、对 mobile 忽略它。
        platform: credentialPrefix(raw) === 'mobile' ? undefined : platform.value,
      })
      const name = r.account.nickname || r.account.label || r.account.id
      // 说清发生了什么：同一个手机号再次导入是"换凭据"，不是"多了一个账号"。
      pushToast('success', r.created ? `已添加 ${name}` : `已更新 ${name} 的凭据`)
      const where = r.account.pc_platform ? PLATFORM_LABEL[r.account.pc_platform] : '手机端'
      formOk.value = r.created
        ? `已添加 ${name}（${where}）`
        : `${name} 的凭据已更新（昵称与设备号保留）`
      importAuth.value = ''
      closeDialog('add-account')
      await load()
    } catch (e) {
      formFail(e instanceof Error ? e.message : '导入失败')
    } finally {
      importBusy.value = false
    }
  }

  // ── 每账号设置（GET/PUT /api/accounts/{id}/settings）─────────────────
  //
  // # 「任务参数」/「功能开关」那几格是共享模块
  //
  // 备份等待 / 刷新阈值 / 跳过任务 / AI 头像上限**连同本地校验**一起搬去了
  // `../ui/account-fields`（工厂函数），这边只剩一个 `acct` 句柄：
  // `load` 铺数据、`build` 攒那几格、`taskFields()` / `featureFields()` 画两组表单。
  //
  // ⚠ 2026-10-08 起这里**没有**推送编辑器了：渠道收成**登录用户级**
  //（用户口径「很显然我说的和云盘账户没有关系」＋「丢弃现有情况，重写」✓），
  // 弹窗里那一块整块删掉 ⇒ 这个口只管账号级那几格 ✓。
  //
  // # 打开才拉数据
  //
  // 一堆字段，页面 `load()` 里顺手全拉一遍只会多一条失败路径（没有后端时
  // 每个账号各报一次错），而首屏一行都用不到它。所以读发生在**打开弹窗那一刻**。
  const SETTINGS_DIALOG = 'account-settings'
  const settingsAccount = ref<AccountSummary | null>(null)
  const settingsState = ref<'idle' | 'loading' | 'error' | 'ready'>('idle')
  /** 读 / 存 / 本地校验的失败原话（**后端原文优先**，见 `describeSettingsError`）。 */
  const settingsError = ref('')
  /** 保存成功回执。留在弹窗里直到关掉或下一次保存 —— 3 秒的 toast 太容易被错过。 */
  const settingsOk = ref('')
  const settingsSaving = ref(false)

  // 账号级那几格（任务参数 / 功能开关）在共享模块里 —— 账号页与配置中心复用同一份。
  const acct = createAccountFields()

  /**
   * 把 GET / PUT 回来的一份设置铺进表单。
   *
   * 交给共享模块的 `acct.load`（它自己逐格容错，理由见模块文件头）。
   */
  const applySettings = (s: AccountSettings): void => {
    acct.load(s)
  }

  const openSettings = (a: AccountSummary): void => {
    settingsAccount.value = a
    settingsState.value = 'loading'
    settingsError.value = ''
    settingsOk.value = ''
    openDialog(SETTINGS_DIALOG)
    void loadSettings(a)
  }

  const loadSettings = async (a: AccountSummary): Promise<void> => {
    settingsState.value = 'loading'
    settingsError.value = ''
    try {
      applySettings(await getAccountSettings(a.id))
      settingsState.value = 'ready'
    } catch (e) {
      settingsError.value = describeSettingsError(e, '读取')
      settingsState.value = 'error'
    }
  }

  const closeSettings = (): void => {
    // 换一个账号再打开时不该看到上一条回执/错误 —— 关掉就清干净。
    settingsOk.value = ''
    settingsError.value = ''
    settingsState.value = 'idle'
    settingsAccount.value = null
  }

  /** 攒成 PUT 的整份 DTO：账号级那几格来自共享模块（推送渠道不在这里了 ✓）。 */
  const buildSettings = (): AccountSettings => acct.build()

  const saveSettings = async (): Promise<void> => {
    const target = settingsAccount.value
    if (!target) return
    /**
     * ⚠ 弹窗优先 + 就近（2026-10-08 用户口径）：
     * `pushToast('error', …)` 让人一眼看见（错误类不自动消失，见 `ui/toast.ts`），
     * `settingsError` 那一句留在表单**顶上**（就在这张卡的标题下面）指到具体哪一格。
     * ⚠ 读失败（`loadSettings` 的 catch）**不走 toast**：那是"这一块没读出来"，
     * 页内带「重试」才是能操作的那一条（见 `ui/error-dialog.ts` 的口径 ✓）。
     */
    const settingsFail = (msg: string): void => {
      settingsError.value = msg
      pushToast('error', msg)
    }
    settingsError.value = ''
    settingsOk.value = ''
    let payload: AccountSettings
    try {
      payload = buildSettings()
    } catch (e) {
      // 本地能判的（空、不是整数、JSON 坏了）当场说，**范围**仍然留给后端。
      settingsFail(e instanceof Error ? e.message : '设置不完整')
      return
    }
    settingsSaving.value = true
    try {
      // 用**后端回读的那一份**回填：整体替换的语义下，"我以为存进去的"不算数。
      applySettings(await putAccountSettings(target.id, payload))
      settingsOk.value = (target.nickname || target.label || target.id) + ' 的设置已保存'
      pushToast('success', '账号设置已保存')
    } catch (e) {
      settingsFail(describeSettingsError(e, '保存'))
    } finally {
      settingsSaving.value = false
    }
  }

  // ── 每账号设置的渲染 ────────────────────────────────────────────────
  //
  // ⚠ 本文件**不再有**私有的 `textField`：四格连渲染原语一起搬进了
  // `../ui/account-fields`（那边带一份私有副本，理由与"值走 thunk"的说明见该文件头）。
  // 搬运时对比过：`textField` 在本文件已 0 引用，留着会被 `noUnusedLocals`
  // （`npm run typecheck`）判成死代码。

  const settingsForm = () => (
    <>
      {settingsOk.value ? (
        <div class="alert alert-soft alert-success text-sm" role="status">
          <span class="break-words">{settingsOk.value}</span>
        </div>
      ) : null}

      {settingsError.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{settingsError.value}</span>
        </div>
      ) : null}

      <div class="flex flex-col gap-2">
        <h4 class="text-sm font-medium">任务参数</h4>
        {acct.taskFields()}
      </div>

      {/*
        ⚠ 「功能开关」**暂时不显示** —— 开关与理由见文件头那个 `SHOW_ACCOUNT_SETTINGS_BLOCKS` ✓。
        包成 `<>` 是因为三元的分支只认"一个 JSX 元素或 null" ✓（lite 编译器的边界）。
        ⚠ 「运行结果推送」那一块 2026-10-08 **整块删掉了** ✗（渠道收成登录用户级 ⇒
        见「推送」页 `/push`），不是藏起来 —— 理由见文件头那段 ⚠⚠ ✓。
      */}
      {SHOW_ACCOUNT_SETTINGS_BLOCKS ? (
        <>
        <div class="flex flex-col gap-2">
          <h4 class="text-sm font-medium">功能开关</h4>
          {acct.featureFields()}
        </div>
        </>
      ) : null}

      <div class="modal-action">
        <button type="button" class="btn btn-soft" onClick={() => closeDialog(SETTINGS_DIALOG)}>
          关闭
        </button>
        <button
          type="button"
          class="btn btn-primary"
          disabled={settingsSaving.value}
          onClick={() => void saveSettings()}
        >
          {settingsSaving.value ? '保存中…' : '保存设置'}
        </button>
      </div>
    </>
  )

  /**
   * 弹窗正文：加载中 / 读失败（带重试）/ 表单。
   *
   * ⚠ 读失败的状态里**不关**弹窗：`settingsError` 里那句（404 / 401 / 400 / 网络）
   * 正是用户要读的东西，关掉它等于把唯一的线索扔掉。
   */
  const settingsBody = () => {
    if (settingsState.value === 'error') {
      return (
        <div class="flex flex-col gap-3">
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">{settingsError.value}</span>
          </div>
          <button
            type="button"
            class="btn btn-soft self-start"
            onClick={() => {
              const a = settingsAccount.value
              if (a) void loadSettings(a)
            }}
          >
            重试
          </button>
        </div>
      )
    }
    if (settingsState.value !== 'ready') {
      return (
        <div class="flex justify-center py-8">
          <span class="loading loading-lg text-primary" />
        </div>
      )
    }
    return settingsForm()
  }

  onMounted(() => {
  setPageRefresh({ run: () => void load() })
    void load()
  })
  onUnmounted(() => {
    setPageRefresh(null)
    stopPolling()
  })

  /** 到期时间的说法。`0` = 解析不出，与"已过期"不是一回事。 */
  const expireText = (ms: number) => {
    if (!ms) return '未知'
    const left = ms - Date.now()
    if (left <= 0) return '已过期'
    const days = Math.floor(left / 86_400_000)
    if (days >= 1) return `${days} 天后`
    const hours = Math.floor(left / 3_600_000)
    return hours >= 1 ? `${hours} 小时后` : '不到 1 小时'
  }

  /**
   * 添加弹窗里要不要显示「客户端形态」。
   *
   * - 手机端（App 登录）没有这个概念；
   * - 粘贴凭据时看**凭据自带的**前缀：`mobile` 的不需要，`pc`（或看不出前缀）
   *   的需要 —— 服务端对 `pc` 凭据**要求**它，不显示会让提交必然失败。
   */
  const needPlatform = () => {
    if (method.value === 'app') return false
    if (method.value === 'auth') return credentialPrefix(importAuth.value) !== 'mobile'
    return true
  }

  /**
   * 「客户端形态」选择器。
   *
   * ⚠ 它渲染在**各个方式自己的面板里**，而不是飘在 tab 栏上面（2026-10-01 用户
   * 用箭头画出来的正是这层归属）：挂在上面时它看起来像"管所有方式"，
   * 可它对手机端无意义、对 `mobile` 凭据也无意义 —— 一个会凭空消失的全局控件，
   * 比一个出现在该出现的地方的控件难理解得多。
   */
  const platformPicker = () => (
    <div class="flex flex-col gap-2">
      <span class="text-xs font-medium opacity-70">客户端形态</span>
      <div class="join">
        {(['windows', 'macos'] as const).map((p) => (
          <button
            type="button"
            class={`btn join-item btn-sm min-h-11 md:min-h-0 ${platform.value === p ? 'btn-primary' : 'btn-soft'}`}
            onClick={() => (platform.value = p)}
          >
            {PLATFORM_LABEL[p]}
          </button>
        ))}
      </div>
      <p class="text-xs opacity-70">
        这会随账号存进 <code>accounts.json</code>，之后每次请求的 UA、设备串与渠道号
        都按它来。选错的话，账号以后一直用错的身份发请求。
      </p>
    </div>
  )

  /** 手机端（App）没有客户端形态这一说：摆一个选不了或选了没用的开关只会让人以为它有意义。 */
  const appPlatformNote = () => (
    <div class="alert alert-soft text-sm" role="status">
      <span>
        手机端登录用 App 自己的身份（<code>mobile</code> 前缀）与设备头，
        不存在 Windows / macOS 之分，所以这里不需要选客户端形态。
      </span>
    </div>
  )

  /** 凭据自带 `mobile` 前缀时，形态同样无从可选（也不需要选）。 */
  const mobileCredentialNote = () => (
    <p class="text-xs opacity-70">
      这条凭据是 <code>mobile</code> 前缀（App 端身份），不需要客户端形态。
    </p>
  )

  // 子路由：账号配置（推送 + 账号级设置）—— 与任务页的配置中心同一套做法。
  // ⚠ 地址是 `/accounts/settings`（用户 2026-10-05 选的：挂账号页下面，不新开导航项）
  // ⚠ 2026-10-08 改名：这一页的地址从 `/accounts/push` 改成 `/accounts/settings`
  // —— 它的内容早就是"账号级设置 + 功能开关"了（推送渠道已搬到「推送」页 `/push` ✓），
  // 路径里那个 `push` 名不副实 ✗。
  //
  // ⚠ **老地址照旧能打开** ✓（`|| === 'push'` ✓）：书签 / 浏览器历史 / 旧文档里的链接
  // 不会 404 ✓ —— 与"老数据行为不变"同一个口径 ✓（改名只该影响**新**链接 ✓）。
  if (
    path.value[0] === 'accounts' &&
    (path.value[1] === 'settings' || path.value[1] === 'push')
  ) {
    return <AccountConfigPage onExit={() => navigate('/accounts')} />
  }

  // 子路由：上游状态（按需看上游账上的读数）。
  // ⚠ 地址是 `/accounts/upstream` —— 用户 2026-10-07 原话：「**这个不要放 nav 里，
  // 看放哪里好点**」✓。挂账号页下面是因为这些读数**都是按账号取的**
  //（票与会话按账号建，不能共用 ✓）⇒ 它跟着账号走，不该占一条导航 ✓。
  if (path.value[0] === 'accounts' && path.value[1] === 'upstream') {
    return <UpstreamPage onExit={() => navigate('/accounts')} />
  }

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-wrap items-center justify-end gap-2">
        {/* 标题由外壳那个 sticky navbar 显示（`app.tsx` 读 `navItem(id).title`）——
            页面里**不要再写一遍**：2026-10-03 之前八页各写了一次，于是每页顶部
            都是同一句话挨着出现两遍（连 `text-base font-medium` 都一模一样）✗。
            这里只放动作（添加/新建…）——「刷新」已搬到外壳 navbar 的图标上
          （见 `ui/page-refresh.ts`）。 */}
        <div class="flex items-center gap-2">
          {/*
            添加账号收进弹窗：四条登录路径（扫码/短信/手机端/账密）加表单本身有
            800 多行，常驻在页面上会把"我现在有哪几个账号"压到第一屏之外 ——
            而那一句话才是这一页的主语。表单在弹窗里、宽度也够。
          */}
          {/* 账号配置（推送 + 账号级设置）：推送是"每个账号一份"的东西 ⇒ 挂在这一页下面
              （`/accounts/settings`；侧栏 9 条封顶，不为它新开导航项） */}
          {/* 上游状态：按需看上游账上的读数（App 任务清单 / 签到历史 / 待领取奖品 /
              云朵记录）。**不是导航项** —— 挂在这一页下面，与「账号配置」并列 ✓ */}
          <button
            type="button"
            class="btn btn-sm btn-soft min-h-11 md:min-h-0"
            onClick={() => navigate('/accounts/upstream')}
          >
            上游状态
          </button>
          <button
            type="button"
            class="btn btn-sm btn-soft min-h-11 md:min-h-0"
            onClick={() => navigate('/accounts/settings')}
          >
            账号配置
          </button>
          {/*
            一键刷新凭据（用户口径 2026-10-11：「可手动一键刷新」✓）。
            ⚠ 放在「添加账号」**左边**、用 `btn-soft` 而不是 `btn-primary`：
            这一页的主行动是"加账号"（实心只有一个位置 ✓）。
            ⚠ 没有账号时**不画**它 —— 一个必然回"没有需要刷新的凭据"的按钮
            只会让人以为坏了 ✓（与列表空态那条处置一致 ✓）。
          */}
          {accounts.value.length > 0 ? (
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={refreshing.value !== null}
              onClick={() => askRefresh('')}
            >
              {refreshing.value === '' ? <span class="loading loading-xs" /> : null}
              刷新全部凭据
            </button>
          ) : null}
          <button
            type="button"
            class="btn btn-sm btn-primary min-h-11 md:min-h-0"
            onClick={() => openDialog('add-account')}
          >
            添加账号
          </button>
        </div>
      </div>

      {/* ── 已登录账号 ─────────────────────────────────────────────── */}
      {state.value === 'loading' ? (
        <div class="flex justify-center py-10">
          <span class="loading loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{loadError.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' && accounts.value.length === 0 ? (
        <EmptyState
          title="还没有账号"
          hint="点右上角「添加账号」，扫码、短信、账密或用一条凭据都行。"
        />
      ) : null}

      {accounts.value.length > 0 ? (
        <ul class="flex flex-col gap-2">
          {accounts.value.map((a) => (
            // 停用的整张压暗：一眼扫过去就知道"这个现在不参与"
            <li class={`card card-border bg-base-100${a.enabled ? '' : ' opacity-60'}`}>
              {/*
                窄屏（<md）**纵向两段**：名字 + 身份徽章 / 设备与到期 / 动作行。
                ⚠ 这是踩过的坑（2026-10-01 用户截图为证）：原来是
                `flex-row flex-wrap` + 文字列 `min-w-0 flex-1`，窄屏上四个控件
                **不肯换行** —— 文字列的 flex-basis 是 0，可以被压到 40px 宽，
                于是「设备 B58A…-ENDIN · 凭据 27 天后」被挤成**一列竖排单字**，
                完全没法读。⇒ 窄屏改成列方向；动作那一行等分铺满（44px 触控高度）。
              */}
              <div class="card-body gap-3 p-3 md:flex-row md:flex-wrap md:items-center">
                <div class="flex w-full min-w-0 flex-col md:w-auto md:flex-1">
                  <div class="flex min-w-0 items-center gap-2">
                    <span class="min-w-0 truncate font-medium">{a.nickname || a.label || a.id}</span>
                    {/* 身份必须显示：它决定这个账号以后每次请求用哪套头和 UA。
                        `pc_platform` 为空 = App 端登录（或更早的历史数据）。
                        窄屏把它放在名字旁边（动作行留给按钮），宽屏本来也在这一侧。 */}
                    {a.enabled ? null : (
                      <span
                        class="badge badge-soft badge-warning badge-sm shrink-0"
                        title="已停用：各处挑账号的下拉里不再出现它（已有定时规则照旧跑）"
                      >
                        已停用
                      </span>
                    )}
                    {a.pc_platform ? (
                      <span class="badge badge-soft badge-primary badge-sm shrink-0">
                        {PLATFORM_LABEL[a.pc_platform]}
                      </span>
                    ) : (
                      <span
                        class="badge badge-soft badge-sm shrink-0"
                        title="App（手机端）登录的账号：用 mobile 前缀与 App 设备头"
                      >
                        手机端
                      </span>
                    )}
                  </div>
                  {/* 设备号是 32 位十六进制加后缀：`break-all` 不能省，
                      否则它自己一条就能把窄屏顶出横向滚动条。 */}
                  <span class="break-all text-xs opacity-70">
                    设备 {a.device_id} · 凭据 {expireText(a.expire)}
                  </span>
                </div>
                {/*
                  编辑 / 设置 / 停用在前、移除在后：都是"对这个账号做点事"，
                  而移除不可逆 —— 让它留在最外侧（远离视线落点）。
                  停用**可逆**（同一颗按钮再点回来），所以它不需要确认框。
                  ⚠ **不要给这几个按钮加 `flex-1`**（2026-10-01 用户口径：
                  "这种按钮对于移动端来说太大了"）：拉满整行会变成一条压在卡片上的
                  按钮带，而本仓其它页面（定时/编排/历史）全是**内容宽度**的
                  `btn-sm min-h-11 md:min-h-0`。窄屏靠右排一行正合适，
                  触控高度仍是 44（§6.1 那条不变）。
                */}
                <div class="flex w-full items-center justify-end gap-2 md:w-auto">
                  {/*
                    刷新凭据（按账号 ✓）—— 放在「编辑」前：它改的是**凭据本身**，
                    与"改名字/设备"同一类动作，而"停用/移除"是另一类（影响可见性）✓。
                  */}
                  <button
                    type="button"
                    class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                    disabled={refreshing.value !== null}
                    onClick={() => askRefresh(a.id)}
                  >
                    {refreshing.value === a.id ? <span class="loading loading-xs" /> : null}
                    刷新
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                    onClick={() => askEdit(a)}
                  >
                    编辑
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                    onClick={() => openSettings(a)}
                  >
                    设置
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                    disabled={enabling.value === a.id}
                    onClick={() => void toggleEnabled(a)}
                  >
                    {a.enabled ? '停用' : '启用'}
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-soft btn-error min-h-11 md:min-h-0"
                    onClick={() => askDelete(a)}
                  >
                    移除
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      ) : null}

      {/*
        添加账号（弹窗）。
        四条登录路径加各自表单有 300 多行，常驻会把"我现在有哪几个账号"压到
        第一屏之外 —— 而那一句才是这一页的主语。弹窗里宽度也够放表单。
      */}
      <Dialog id="add-account" title="添加账号">
        <div class="flex flex-col gap-4">

          {/* 登录方式。
              ⚠ 用 `join` + `btn` 而**不是** daisyUI 的 `tabs` / `tabs-box`：
              后者会把整个 tab 组件（含四套朝向变体与带框那套，实测 14.5KB）
              拖进产物，而全项目只有这一处是"选一个"的语义。
              写法与上面「客户端形态」保持一致 —— 同一页里两个同级选择器，
              不该一个用 tabs、一个用 join。

              窄屏（<md）**铺满等分**：375 宽下五个标签挤在一起每格只剩 ~58px，
              实测「账号密码」那格会顶到边（用户截图里就是这么挤的）；
              铺满 + 把四字的「账号密码」缩成两字「账密」之后每格宽裕。
              ⚠ 缩短的是**这格**的文案，面板里该怎么解释还怎么解释。 */}
          <div class="join w-full md:w-auto">
            {(
              [
                ['qr', '扫码'],
                ['sms', '短信'],
                ['app', '手机端'],
                ['password', '账密'],
                ['auth', '凭据'],
              ] as const
            ).map(([id, label]) => (
              <button
                type="button"
                aria-pressed={method.value === id}
                class={`btn join-item btn-sm min-h-11 flex-1 whitespace-nowrap px-2 md:min-h-0 md:flex-none md:px-4 ${method.value === id ? 'btn-primary' : 'btn-soft'}`}
                onClick={() => onMethodChange(id)}
              >
                {label}
              </button>
            ))}
          </div>

          {formOk.value ? (
            <div class="alert alert-soft alert-success text-sm" role="status">
              <span class="break-words">{formOk.value}</span>
            </div>
          ) : null}

          {formError.value ? (
            <div class="alert alert-soft alert-error text-sm" role="alert">
              <span class="break-words">{formError.value}</span>
            </div>
          ) : null}

          {/* 扫码 */}
          {method.value === 'qr' ? (
            <div class="flex flex-col items-center gap-3">
              {needPlatform() ? <div class="w-full">{platformPicker()}</div> : null}
              {qrImage.value ? (
                // 白底容器：深色主题下二维码必须保持黑码白底（扫码器假设深色是数据）
                <div class="rounded-box bg-white p-3">
                  <img src={qrImage.value} alt="登录二维码" class="h-48 w-48" />
                </div>
              ) : (
                <p class="text-sm opacity-70">点下面的按钮生成二维码。</p>
              )}
              {qrNote.value ? (
                <p
                  class={`text-sm ${qrPhase.value === 'failed' ? 'text-error' : 'opacity-70'}`}
                >
                  {qrNote.value}
                </p>
              ) : null}
              <div class="flex flex-wrap justify-center gap-2">
                <button
                  type="button"
                  class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                  disabled={busy.value}
                  onClick={() => void beginQr()}
                >
                  {qrImage.value ? '重新出码' : '生成二维码'}
                </button>
                {qrSessionId.value ? (
                  <button
                    type="button"
                    class="btn btn-soft btn-sm min-h-11 md:min-h-0"
                    onClick={abandonQr}
                  >
                    取消
                  </button>
                ) : null}
              </div>
              <p class="text-xs opacity-70">用移动云盘 App 扫码，然后在手机上确认。</p>
            </div>
          ) : null}

          {/* 短信 */}
          {method.value === 'sms' ? (
            <div class="flex flex-col gap-3">
              {needPlatform() ? platformPicker() : null}
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">手机号</span>
                <input
                  type="tel"
                  class="input w-full"
                  placeholder="19900000001"
                  value={phone.value}
                  onInput={(e) => {
                    phone.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <div class="flex flex-wrap gap-2">
                <button
                  type="button"
                  class="btn btn-soft btn-sm min-h-11 md:min-h-0"
                  disabled={busy.value}
                  onClick={() => void sendSms()}
                >
                  {smsSent.value ? '重新发送' : '发送验证码'}
                </button>
                {smsRemain.value != null ? (
                  <span class="self-center text-xs opacity-70">
                    今日还可发送 {smsRemain.value} 次
                  </span>
                ) : null}
              </div>

              {/*
                ⚠ 这里**刻意不显示题面图、也不给按钮**。
                滑块答案不是"识别"出来的 —— 上游把它加密在 `puzzleLeft` 里，
                解出来就是坐标（见 `SliderPuzzle::answer`）。所以能自动过的时候
                这张图纯属干扰：它会让人以为"得自己去量像素"。
                走到这一格只剩"上游这次没给答案"，那是工具侧解不开，
                要说清楚并让人重发，而不是把图甩出来让人自己想办法。
              */}
              {slider.value && !slider.value.answer ? (
                <div class="alert alert-soft alert-warning text-xs" role="status">
                  <span>
                    上游这次没把滑块答案给出来，工具侧解不开。这不是让你去核对图片 ——
                    答案本来就在题面里加密着。请稍后再发一次验证码。
                  </span>
                </div>
              ) : null}

              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">验证码</span>
                <input
                  type="text"
                  inputmode="numeric"
                  class="input w-full"
                  placeholder="6 位数字"
                  value={smsCode.value}
                  onInput={(e) => {
                    smsCode.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <button
                type="button"
                class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                disabled={busy.value}
                onClick={() => void submitSms()}
              >
                登录
              </button>
            </div>
          ) : null}

          {/* 手机端（App）短信 */}
          {method.value === 'app' ? (
            <div class="flex flex-col gap-3">
              {appPlatformNote()}
              <p class="text-xs opacity-70">
                与上面的「短信」**不是同一条链路**：手机端走 App 自己的密钥与设备头，
                登录后凭据以 <code>mobile</code> 前缀落盘。
              </p>
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">手机号</span>
                <input
                  type="tel"
                  class="input w-full"
                  placeholder="19900000001"
                  value={appPhone.value}
                  onInput={(e) => {
                    appPhone.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <div class="flex flex-wrap gap-2">
                <button
                  type="button"
                  class="btn btn-soft btn-sm min-h-11 md:min-h-0"
                  disabled={busy.value}
                  onClick={() => void sendAppSms()}
                >
                  {appSent.value ? '重新发送' : '发送验证码'}
                </button>
                {appRemain.value != null ? (
                  <span class="self-center text-xs opacity-70">
                    今日还可发送 {appRemain.value} 次
                  </span>
                ) : null}
              </div>

              {/* 同 PC 那一格：不显示题面图、不给按钮 —— 滑块由发码函数自动过。 */}
              {appSlider.value && !appSlider.value.answer ? (
                <div class="alert alert-soft alert-warning text-xs" role="status">
                  <span>
                    上游这次没把滑块答案给出来，工具侧解不开。请稍后再发一次验证码。
                  </span>
                </div>
              ) : null}

              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">验证码</span>
                <input
                  type="text"
                  inputmode="numeric"
                  class="input w-full"
                  placeholder="6 位数字"
                  value={appCode.value}
                  onInput={(e) => {
                    appCode.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <button
                type="button"
                class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                disabled={busy.value}
                onClick={() => void submitAppSms()}
              >
                登录
              </button>
            </div>
          ) : null}

          {/* 账密 */}
          {method.value === 'password' ? (
            <div class="flex flex-col gap-3">
              {needPlatform() ? platformPicker() : null}
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">手机号</span>
                <input
                  type="text"
                  class="input w-full"
                  value={username.value}
                  onInput={(e) => {
                    username.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">口令</span>
                <input
                  type="password"
                  class="input w-full"
                  value={password.value}
                  onInput={(e) => {
                    password.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <p class="text-xs opacity-70">
                口令只在本次请求里用于计算摘要，**不会**存进浏览器或数据库。
              </p>
              <button
                type="button"
                class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                disabled={busy.value}
                onClick={() => void submitPassword()}
              >
                登录
              </button>
            </div>
          ) : null}

          {/*
            粘贴凭据。
            这条路**不做任何网络登录**：账号主键是手机号的哈希，服务端从凭据里
            就能算出来 —— 所以"从旧实例 / 插件 / 抓包里拿到的一条 auth"
            可以直接变成这里的账号。
          */}
          {method.value === 'auth' ? (
            <div class="flex flex-col gap-3">
              {needPlatform() ? platformPicker() : mobileCredentialNote()}
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">凭据</span>
                <textarea
                  class="textarea w-full font-mono text-xs"
                  rows={4}
                  placeholder="Basic <base64>，或只粘 base64 那段"
                  value={importAuth.value}
                  onInput={(e) => {
                    importAuth.value = (e.target as HTMLTextAreaElement).value
                  }}
                />
              </label>
              <p class="text-xs opacity-70">
                同一个手机号已经加过时，这里是换掉它的凭据，不是多一个账号 ——
                昵称、设备号与任务设置都会保留。凭据只落进本机的
                <code>accounts.json</code>，不会回显给界面。
              </p>
              <button
                type="button"
                class="btn btn-primary btn-sm self-start min-h-11 md:min-h-0"
                disabled={importBusy.value}
                onClick={() => void doImport()}
              >
                {importBusy.value ? '添加中…' : '用这条凭据添加'}
              </button>
            </div>
          ) : null}
        </div>
      </Dialog>

      {/* ── 设备指纹 ───────────────────────────────────────────────── */}
      <div class="card card-border bg-base-100">
        <div class="card-body gap-2 p-4">
          <h2 class="card-title text-sm">本机设备指纹</h2>
          <code class="break-all text-xs opacity-70">{deviceId.value || '（尚未生成）'}</code>
          <p class="text-xs opacity-70">
            扫码时它作为 <code>dID</code> 上报，服务端据此定位会话。
            它跨重启稳定；换掉会让旧的扫码会话立即失效，但已登录账号不受影响。
          </p>
          <button
            type="button"
            class="btn btn-soft btn-sm self-start min-h-11 md:min-h-0"
            onClick={() => openDialog(rotateDialogId)}
          >
            换一台设备
          </button>
        </div>
      </div>

      {/*
        编辑账号（弹窗）。
        三格：昵称、设备号、换凭据 —— 都是**给了才改**（服务端是 PATCH）。
        凭据输入框刻意**留空**：它从不回显，空就是"不动它"，
        所以"改个名字"不需要用户重新粘一遍凭据。
      */}
      <Dialog id="edit-account" title="编辑账号">
        {editing.value ? (
          <div class="flex flex-col gap-3">
            <p class="text-xs opacity-70">
              账号 <b>{editing.value.nickname || editing.value.label || editing.value.id}</b>
              {' · '}
              {editing.value.pc_platform ? PLATFORM_LABEL[editing.value.pc_platform] : '手机端'}
              {' · 凭据 '}
              {expireText(editing.value.expire)}
            </p>

            {/*
              ⚠⚠ 失败那一句放在**弹窗标题下面**（保存键在弹窗底部，但这里一进来就看得见），
              不再放弹窗最底下 —— 用户点「保存」时视线在底部，可读完后要改的字段在上面，
              两处都够不着才是问题（2026-10-08 用户口径「所有的提示都是优先弹窗式」✓）。
              弹窗那一份由 `doSaveEdit()` 里的 `pushToast('error', …)` 给 ✓。
            */}
            {editError.value ? (
              <div class="alert alert-soft alert-error text-sm" role="alert">
                <IconAlert class="size-4 shrink-0" />
                <span class="break-words">{editError.value}</span>
              </div>
            ) : null}

            <label class="flex flex-col gap-1">
              <span class="text-xs opacity-70">昵称（留空就用掩码手机号）</span>
              <input
                type="text"
                class="input w-full"
                maxlength={32}
                value={editNickname.value}
                onInput={(e) => {
                  editNickname.value = (e.target as HTMLInputElement).value
                }}
              />
            </label>

            <label class="flex flex-col gap-1">
              <span class="text-xs opacity-70">设备号</span>
              <input
                type="text"
                class="input w-full font-mono text-xs"
                value={editDevice.value}
                onInput={(e) => {
                  editDevice.value = (e.target as HTMLInputElement).value
                }}
              />
              <span class="text-xs opacity-70">
                上游按它认设备。不确定就别改 —— 换掉可能让这个账号的请求被上游拒绝。
              </span>
            </label>

            <label class="flex flex-col gap-1">
              <span class="text-xs opacity-70">换凭据（留空 = 不改）</span>
              <textarea
                class="textarea w-full font-mono text-xs"
                rows={3}
                placeholder="Basic <base64>，或只粘 base64 那段"
                value={editAuth.value}
                onInput={(e) => {
                  editAuth.value = (e.target as HTMLTextAreaElement).value
                }}
              />
            </label>

            {/* 换了 pc 凭据就得同时给它形态：服务端要求形态与凭据同源，
                否则这个账号以后每次请求都按错的平台发头。 */}
            {editAuth.value.trim() && credentialPrefix(editAuth.value) !== 'mobile' ? (
              <div class="flex flex-col gap-2">
                <span class="text-xs font-medium opacity-70">客户端形态</span>
                <div class="join">
                  {(['windows', 'macos'] as const).map((p) => (
                    <button
                      type="button"
                      class={`btn join-item btn-sm min-h-11 md:min-h-0 ${editPlatform.value === p ? 'btn-primary' : 'btn-soft'}`}
                      onClick={() => (editPlatform.value = p)}
                    >
                      {PLATFORM_LABEL[p]}
                    </button>
                  ))}
                </div>
              </div>
            ) : null}

            <div class="modal-action">
              <button
                type="button"
                class="btn btn-soft"
                onClick={() => closeDialog('edit-account')}
              >
                取消
              </button>
              <button
                type="button"
                class="btn btn-primary"
                disabled={savingEdit.value}
                onClick={() => void doSaveEdit()}
              >
                {savingEdit.value ? '保存中…' : '保存'}
              </button>
            </div>
          </div>
        ) : null}
      </Dialog>

      {/*
        每账号设置（弹窗）。
        结构：任务参数 → 功能开关 → 推送。推送里十条渠道**折叠**着，首屏只列
        已配置的那些，其余走「+ 加渠道」—— 十条渠道的表单全铺开有一屏半，
        而常用的一直是那几条。
        ⚠ 打开才 GET：见上面「打开才拉数据」那段。
      */}
      <Dialog
        id={SETTINGS_DIALOG}
        title={
          settingsAccount.value
            ? '账号设置 · ' +
              (settingsAccount.value.nickname || settingsAccount.value.label || settingsAccount.value.id)
            : '账号设置'
        }
        onClose={closeSettings}
      >
        {settingsBody()}
      </Dialog>

      {/*
        ⚠ 换设备指纹**不可逆**（旧指纹开着的扫码会话立即失效）⇒ 必须确认一次。
        影响面写具体：换的是什么、谁会失效、谁不受影响（§6.4：只说"确定吗"等于没确认）。
      */}
      <ConfirmDialog
        id={rotateDialogId}
        title="更换本机设备指纹"
        impact={
          <span>
            将把本机设备指纹换成新的一份。用<b>旧指纹</b>开着的扫码会话（二维码）
            会立即失效，得重新出码；<b>已登录的账号不受影响</b>。
            上游按设备指纹认设备，换掉后新的请求都会带新指纹。
          </span>
        }
        confirmLabel="换掉指纹"
        onConfirm={() => void doRotateDevice()}
      />

      {/*
        刷新凭据的两道确认（见 `askRefresh` 那段说明：**不可逆 + 手滑代价** ✓）。
        ⚠ 文案要说清"会换掉会话"与"旧串当场作废" —— 只写「确定吗？」等于没确认 ✓
        （本仓 §6.4：影响面要具体到数字/名字 ✓）。
      */}
      <ConfirmDialog
        id={REFRESH_ONE_DIALOG}
        title="刷新这个账号的凭据"
        impact={
          <span>
            将向云盘请求一份<b>新的凭据</b>，换掉{' '}
            <b>{refreshTargetLabel(pendingRefresh.value ?? '')}</b> 现在用的那一串。
            换完之后旧凭据当场作废（<b>这一步撤不回来</b>）；用新凭据登录的会话不受影响。
            还没到提前量时不会真的去刷，只会跳过。
          </span>
        }
        confirmLabel="刷新"
        busy={refreshing.value !== null}
        onConfirm={() => void doRefresh()}
      />

      <ConfirmDialog
        id={REFRESH_ALL_DIALOG}
        title="刷新全部账号的凭据"
        impact={
          <span>
            将对 <b>{refreshTargetLabel('')}</b> 逐个请求新凭据（<b>慢</b>，每个最慢约半分钟）。
            换完之后各自的旧凭据当场作废（<b>这一步撤不回来</b>）。
            没到提前量的账号会跳过、不会打上游。
          </span>
        }
        confirmLabel="全部刷新"
        busy={refreshing.value !== null}
        onConfirm={() => void doRefresh()}
      />

      <ConfirmDialog
        id="confirm-delete-account"
        title="移除账号"
        impact={
          <span>
            将从本实例移除 <b>{pendingDelete.value?.nickname || pendingDelete.value?.label || pendingDelete.value?.id}</b>{' '}
            及其本地凭据。云端的数据不受影响，重新登录即可恢复。
          </span>
        }
        confirmLabel="移除"
        busy={deleting.value}
        onConfirm={() => void doDelete()}
      />
    </div>
  )
}
