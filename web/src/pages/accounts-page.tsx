/**
 * 账号管理页：真实数据来自 `GET /api/accounts`，三条写接口都在这页上。
 *
 * 形态按设计指南执行：桌面 `table`、移动卡片流（§5.3，同一数据源）；
 * badge/status 类名一律查字面量表（§9.1①，拼接会静默失效）；
 * 空态给"下一步做什么"而不是「暂无数据」（§5.10）。
 *
 * # 三条写面各自的判据
 *
 *  - **粘贴 auth 添加**：输入框是 `type="password"`（那串东西能直接冒充登录），
 *    提交后**立刻清空**，成功与失败都不留；响应只当"展示字段"用，不回显入参；
 *  - **启用开关**：后端走窄写（只动 `enabled` 一列），所以这里也**只**改本地那一列；
 *  - **删除**：走确认弹窗，且把影响面写出来（§6.4）—— 删的是凭据与历史，
 *    不只是列表里那一行；
 *  - **刷新 auth**（第四条写面）：也走弹窗，但**不是**确认弹窗 —— 它要收一个入参
 *    （PC 那一支的端档案 windows/macos，库里没记），而确认弹窗的插槽里没有输入位。
 *    新 token **只落库**：这一条的响应里只有掩码形状的账号 id 与到期时刻，
 *    所以界面上没有任何一处需要"用完即弃"的入参。
 *
 * ⚠ 完整手机号不出现在这一页的任何位置：后端给的 `displayName`/`id` 已是掩码形状。
 */

import { onMounted, ref } from 'vue'
import {
  addAccount,
  ApiError,
  deleteAccount,
  listAccounts,
  refreshAccountAuth,
  setAccountEnabled,
  type Account,
  type RefreshAuthResult,
} from '../api'
import { LoginDialog } from './login-dialog'
import { closeDialog, ConfirmDialog, Dialog, openDialog } from '../ui/dialog'
import { IconEye, IconEyeOff } from '../ui/icons'
import { fmtEpochMs, fmtTime } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/** 四个弹窗的 id。唯一性由 daisyUI 那条"每个 modal 一个唯一 id"要求（§5.8）。 */
const ADD_ID = 'dlg-add-account'
const LOGIN_ID = 'dlg-login-account'
const DELETE_ID = 'dlg-delete-account'
const REFRESH_ID = 'dlg-refresh-account'

/** 余额档位（§5.4 的查表，与总览页同一套阈值，不另起一表）。 */
const balanceBadge = {
  high: 'badge-soft badge-success',
  mid: 'badge-soft badge-warning',
  low: 'badge-soft badge-error',
} as const

const toneOf = (cloud: number): keyof typeof balanceBadge =>
  cloud >= 1000 ? 'high' : cloud >= 500 ? 'mid' : 'low'

/**
 * 启用 / 停用那一行的底色。**两态必须不同色**，所以这张表存在的意义就是
 * "别把两种状态写成同一个字面量"（此前两边共用 `text-xs opacity-70`，停用看不出来）。
 * `ghost` 而非 `error`：停用是用户的主动选择，不是故障（红留给过期/不可用，§5.4）。
 */
const enabledBadge = {
  on: 'badge-soft badge-success badge-sm',
  off: 'badge-ghost badge-sm',
} as const

/**
 * 顶部那条"这次操作成了"的色调。
 *
 * **必须有这一档**：启用与停用都套绿色的话，用户扫一眼分不出刚才那下是开还是关
 * （2026-09-22 用户报的就是这个："启用和停用是同一个颜色，无区别"）。
 *
 * `neutral` 既不是失败也不是警告，而是"这是一次**主动**的状态变更" ——
 * 与行内 [enabledBadge] 的 `off` 同一口径：停用是用户的选择、不是故障，
 * 红/黄留给"登录过期 / 不可用 / 连续失败"（§5.4）。
 *
 * 两个值都写成**完整字面量**（代码指南 §9.1①：拼接式 Tailwind 扫不到，静默失效）。
 */
type NoticeTone = 'success' | 'neutral'

const NOTICE_ALERT: Record<NoticeTone, string> = {
  success: 'alert alert-soft alert-success text-sm',
  neutral: 'alert alert-soft text-sm',
}

/**
 * 刷新**没换成**时那两档的人话。
 *
 * 换成了（`rotated`）走另一句，不看这张表 —— 界面对这次操作的口径是
 * **"点一下就换"**，后端那条路由固定直接换，所以"到没到提前量"这一位在界面上
 * 没有读者（提前量只活在 CLI `auth refresh --all` 那条批量路上）。
 *
 * 键仍是后端 `capabilities.RefreshDecision.Code()` 的稳定英文码：中文那句
 * `String()` 明说"不给机器解析"。查不到就走兜底：后端加一档时这里表现为
 * "文案变笼统"，不是白屏。
 */
const REFRESH_NOT_ROTATED: Record<string, string> = {
  'manual-only': '这一支刷不动：库里那串读不出可用的过期时刻，只能人工换 auth' +
    '（重新登录一次，或把新的整串贴进来）',
  'not-yet': '这一次没有换。界面这一路应当直接换 token，走到这句说明后端的口径变了，' +
    '按上面那一行显示的原因处理',
}

/**
 * 行内那枚"刷新 auth"的两档尺寸。**桌面 `btn-xs`、移动 `min-h-11`**。
 *
 * 移动那一档不是装饰：daisyUI 5.7 的 `.btn` 实测只有 **40px** 高（2026-09-22 在 483px
 * 视口上量的），而 §6.1 要 ≥44px；`btn-xs` 更是只有约 24px。
 * 两档都写成完整字面量（代码指南 §9.1①：拼接式 Tailwind 扫不到，静默失效）。
 */
const REFRESH_BTN: Record<'desktop' | 'mobile', string> = {
  desktop: 'btn btn-soft btn-xs',
  mobile: 'btn btn-soft min-h-11',
}

const platformLabel = (p: string | null): string =>
  p === 'mobile' ? '手机' : p === 'pc' ? 'PC' : '未知'

/**
 * 健康度：绿=正常，黄=可用但有告警，红=不可用（登录过期）。
 * 停用不给红：那是主动关闭而非故障，所以用中性点（currentColor），靠文字区分。
 */
const healthOf = (a: Account): { dot: string; label: string } => {
  if (!a.enabled) return { dot: 'status', label: '已停用' }
  if (a.authExpired) return { dot: 'status status-error', label: '登录已过期' }
  if (a.failStreak > 0)
    return { dot: 'status status-warning', label: `连续失败 ${a.failStreak} 次` }
  return { dot: 'status status-success', label: '正常' }
}

const cloudCell = (a: Account) =>
  a.cloudCurrent === null ? (
    <span class="badge badge-ghost">未采集</span>
  ) : (
    <span class={`badge ${balanceBadge[toneOf(a.cloudCurrent)]}`}>{a.cloudCurrent}</span>
  )

const healthCell = (a: Account) => {
  const h = healthOf(a)
  return (
    <span class="flex items-center gap-2 text-sm">
      <span class={h.dot} aria-hidden="true" />
      <span class="opacity-70">{h.label}</span>
    </span>
  )
}

export const AccountsPage = () => {
  const state = ref<LoadState>('loading')
  const accounts = ref<Account[]>([])
  const errorMessage = ref('')
  /**
   * 顶部那条"这次操作成了/失败了"。写操作的反馈必须有，否则用户不知道要不要刷新。
   *
   * 文案与**色调一起存**：拆成两个 ref 迟早只更新其中一个，而那种错的表现是
   * "颜色和文字对不上"，比不显示更难查。赋值一律**整个换掉**（不原地改），
   * 这样 `notice` 这个 ref 的依赖收集才作数。
   */
  const notice = ref<{ text: string; tone: NoticeTone } | null>(null)
  /**
   * **写操作**的失败文案。与 `errorMessage`（整页取数失败）分开是必要的：
   * 后者只在 `state === 'error'` 时渲染，而"开关没生效"发生在列表已经好的时候 ——
   * 共用一个变量就等于这条失败**永远不显示**。
   */
  const actionError = ref('')

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const result = await listAccounts()
      accounts.value = [...result.accounts].sort((a, b) => a.sortOrder - b.sortOrder)
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => void load())

  const upsert = (row: Account) => {
    const at = accounts.value.findIndex((a) => a.id === row.id)
    accounts.value =
      at === -1
        ? [...accounts.value, row].sort((a, b) => a.sortOrder - b.sortOrder)
        : accounts.value.map((a) => (a.id === row.id ? row : a))
  }

  // ── 粘贴 auth 添加 ───────────────────────────────────────────────────────
  const authDraft = ref('')
  const showAuth = ref(false)
  const adding = ref(false)

  const submitAuth = async () => {
    const raw = authDraft.value
    // **先清空再发**：失败路径同样不能把这串留在输入框里（它是凭据本体），
    // 而"等响应回来再清"一旦抛在网络那一步就留下来了。
    authDraft.value = ''
    showAuth.value = false
    if (raw.trim() === '') {
      notice.value = null
      actionError.value = 'auth 是空的'
      return
    }
    adding.value = true
    actionError.value = ''
    try {
      const row = await addAccount(raw.trim())
      upsert(row)
      notice.value = { text: `已添加 ${row.displayName}`, tone: 'success' }
      closeDialog(ADD_ID)
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '添加失败'
    } finally {
      adding.value = false
    }
  }

  // ── 启用开关 ─────────────────────────────────────────────────────────────
  /** 正在写的那一行（禁用它的控件，避免连点把两次开关顺序发反）。 */
  const busyId = ref('')

  const toggleEnabled = async (a: Account, enabled: boolean) => {
    busyId.value = a.id
    actionError.value = ''
    try {
      upsert(await setAccountEnabled(a.id, enabled))
      // 开是"成功"（绿），关是"主动变更"（中性）—— 两态**必须不同色**，
      // 否则用户看不出刚才那下是开还是关（见 [NOTICE_ALERT] 的注释）。
      notice.value = {
        text: `${a.displayName} 已${enabled ? '启用' : '停用'}`,
        tone: enabled ? 'success' : 'neutral',
      }
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '开关没生效'
    } finally {
      busyId.value = ''
    }
  }

  // ── 删除 ─────────────────────────────────────────────────────────────────
  const deleteTarget = ref<Account | null>(null)
  /** 影响面那句是**文本**，单独一个 ref：Vapor 里组件元素的 props 只求值一次，
   *  而文本插值是响应式的（代码指南 §9.1 那条），所以弹窗正文靠这个 ref 更新。 */
  const deleteHint = ref('')
  const deleting = ref(false)

  const askDelete = (a: Account) => {
    deleteTarget.value = a
    deleteHint.value =
      `将删除账号 ${a.displayName} 的登录凭据、云朵采集记录与执行历史。` +
      '删掉之后要重新登录或重新粘贴 auth 才能找回来。'
    openDialog(DELETE_ID)
  }

  const confirmDelete = async () => {
    const target = deleteTarget.value
    if (!target) return
    deleting.value = true
    actionError.value = ''
    try {
      await deleteAccount(target.id)
      accounts.value = accounts.value.filter((a) => a.id !== target.id)
      notice.value = { text: `已删除 ${target.displayName}`, tone: 'success' }
      deleteTarget.value = null
      closeDialog(DELETE_ID)
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '删除失败'
    } finally {
      deleting.value = false
    }
  }

  // ── 刷新 auth（第四条写面）───────────────────────────────────────────────
  /**
   * 要刷的那一行（null = 还没点）。
   *
   * 与 `deleteTarget` 一样**把弹窗写在页面作用域里**、由这里的 ref 喂：Vapor 下
   * 子组件元素的 props 只求值一次（代码指南 §9.1），换成抽出去的 `<RefreshDialog
   * account={row}>` 就会"点第二行、弹窗显示第一行的名字"。
   */
  const refreshTarget = ref<Account | null>(null)
  /** PC 那一支的端档案。库里 `platform` 记的是**端别**（pc / mobile）不是 OS，所以只能由点这一枚的人拍板。 */
  const refreshPlatform = ref<'windows' | 'macos'>('windows')
  const refreshing = ref(false)
  /**
   * 失败文案**只能显示在弹窗里面**：页顶那条 `actionError` 在遮罩背后，
   * 弹窗开着时用户看不到它（§5.10 错误必须带可读原因 —— 藏起来等于没有）。
   */
  const refreshError = ref('')

  const askRefresh = (a: Account) => {
    refreshTarget.value = a
    refreshError.value = ''
    // 每次打开回到默认那一档：不重置就会"上一次选了 macOS，这一次连带着 Windows 那行也走 mac"
    refreshPlatform.value = 'windows'
    openDialog(REFRESH_ID)
  }

  /**
   * 结果那句：换成了就把**库里回读**的到期时刻带上，没换成才去翻判据档位。
   *
   * 到期时刻取那一行的 `authExpire`（`RefreshAuthResult` 里 `beforeExpire` /
   * `afterExpire` 是事后核对口径，不进文案）—— 界面其余那几列用的也是这一列，
   * 两处同一个数才不会"提示说 28 号、列表显示 25 号"。
   */
  const refreshLine = (r: RefreshAuthResult, row: Account) =>
    r.rotated
      ? `已换新 token，到期时间 ${fmtEpochMs(row.authExpire)}`
      : REFRESH_NOT_ROTATED[r.decision] ?? '刷新结束，这一行没有变化'

  const runRefresh = async () => {
    const target = refreshTarget.value
    if (!target) return
    refreshing.value = true
    refreshError.value = ''
    try {
      // 手机那一支不给 `platform`：后端在 App 支用库里那串自带的端档案，给了也是忽略。
      const { refresh, account } = await refreshAccountAuth(
        target.id,
        target.platform === 'mobile' ? '' : refreshPlatform.value,
      )
      // 列表那一行取**后端回读**的形状（含新的 `authExpire` / `authExpired`），不自己拼
      upsert(account)
      notice.value = {
        text: refreshLine(refresh, account),
        // 换成没换成是两件事：换了才是"成功"（绿），没换是"这次什么都没发生"（中性）
        tone: refresh.rotated ? 'success' : 'neutral',
      }
      closeDialog(REFRESH_ID)
    } catch (e) {
      refreshError.value = e instanceof ApiError ? e.message : '刷新失败'
    } finally {
      refreshing.value = false
    }
  }

  const refreshButton = (a: Account, size: 'desktop' | 'mobile') => (
    <button
      type="button"
      class={REFRESH_BTN[size]}
      disabled={refreshing.value}
      onClick={() => askRefresh(a)}
    >
      刷新 auth
    </button>
  )

  // 弹窗正文那几处读数一律**现取**（`refreshTarget.value?.…` 写在函数体里而不是解构成
  // 常量）：这些函数只被调用一次，取值发生在编译成的响应式 getter 里，取早了就是快照。
  /** 没选中任何行时给空串而不是 `undefined`：那会让弹窗里凭空出现 "undefined"。 */
  const refreshName = () => refreshTarget.value?.displayName ?? ''
  const refreshExpire = () => fmtEpochMs(refreshTarget.value?.authExpire ?? null)
  /**
   * 界面上关于"时机"的**唯一**一句话：已过期 / 还在期内。
   *
   * 刻意不提"还剩几天、到没到提前量"：那一位是批量扫（CLI `--all`）的省请求判据，
   * 而这一枚按钮的口径是"点一下就换"，把提前量摆出来只会让人以为不点它才怪
   * （2026-09-22 的报障原话："那个 45 天没到就不刷"）。
   */
  const refreshExpireState = () =>
    refreshTarget.value?.authExpired === true ? '已过期' : '还在期内'
  /** 端别是 `mobile` 才免选；`pc` 与 null（旧导入数据）都给选择，后端在 App 支会忽略它。 */
  const refreshIsApp = () => refreshTarget.value?.platform === 'mobile'
  const refreshExpiredHint = () =>
    refreshTarget.value?.authExpired === true
      ? '（这一行显示**已过期**。过期之后还换不换得动，本仓库没有实测记录 —— ' +
        '真失败了就走人工出口：重新登录一次，或把新的整串 auth 贴进来。）'
      : ''

  // ── 视图 ─────────────────────────────────────────────────────────────────
  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="text-base font-medium">账号管理</h1>
        <span class="badge badge-soft">{accounts.value.length}</span>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <button type="button" class="btn btn-sm btn-soft" onClick={() => openDialog(ADD_ID)}>
          粘贴 auth 添加
        </button>
        <button type="button" class="btn btn-sm btn-primary" onClick={() => openDialog(LOGIN_ID)}>
          登录云盘账号
        </button>
        <button
          type="button"
          class="btn btn-sm btn-soft"
          disabled={state.value === 'loading'}
          onClick={() => void load()}
        >
          刷新
        </button>
      </div>
    </div>
  )

  const loading = () => (
    <div class="flex justify-center py-16">
      <span class="loading loading-spinner loading-lg text-primary" />
    </div>
  )

  const errorAlert = () => (
    <div class="alert alert-soft alert-error text-sm" role="alert">
      <span class="break-words">{errorMessage.value}</span>
      <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
        重试
      </button>
    </div>
  )

  /** 空态：说清"从哪来两个入口"（§5.10）。 */
  const empty = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body items-center gap-3 py-12 text-center">
        <p class="text-sm opacity-70">还没有云盘账号。</p>
        <div class="flex flex-wrap items-center justify-center gap-2">
          <button type="button" class="btn btn-primary" onClick={() => openDialog(LOGIN_ID)}>
            扫码或短信登录
          </button>
          <button type="button" class="btn btn-soft" onClick={() => openDialog(ADD_ID)}>
            粘贴已有 auth
          </button>
        </div>
        <p class="max-w-prose text-xs opacity-60">
          命令行那条路也还在：
          <code class="ml-1 rounded bg-base-200 px-1 py-0.5">mcloud login qr</code>与
          <code class="ml-1 rounded bg-base-200 px-1 py-0.5">
            mcloud login sms --client app --phone 手机号
          </code>
        </p>
      </div>
    </div>
  )

  /** 启用开关。移动端靠 `label` 把触点撑到 44px（§6.1），不是靠 toggle 本体。 */
  const enabledToggle = (a: Account) => (
    <label class="flex min-h-11 cursor-pointer items-center gap-2">
      <input
        type="checkbox"
        class="toggle toggle-primary"
        checked={a.enabled}
        disabled={busyId.value === a.id}
        aria-label={`启用 ${a.displayName}`}
        onChange={(e) => void toggleEnabled(a, (e.target as HTMLInputElement).checked)}
      />
      {/* 两种状态必须一眼分得开：之前两边都是 `text-xs opacity-70`，"停用"看着像"启用"。
          用 §3.4 的 soft 徽标上色（`text-success` / `text-error` 那两位按 §2.4 只留给涨跌）。 */}
      <span class={`badge ${enabledBadge[a.enabled ? 'on' : 'off']}`}>
        {a.enabled ? '已启用' : '已停用'}
      </span>
    </label>
  )

  const deleteButton = (a: Account) => (
    <button
      type="button"
      class="btn btn-soft btn-error btn-xs"
      disabled={busyId.value === a.id || deleting.value}
      onClick={() => askDelete(a)}
    >
      删除
    </button>
  )

  const desktopTable = () => (
    <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
      <table class="table table-sm">
        <thead>
          <tr>
            <th>账号</th>
            <th>端</th>
            <th>云朵</th>
            <th>最近成功</th>
            <th>状态</th>
            <th>启用</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {accounts.value.map((a) => (
            <tr>
              <td class="font-medium break-words">{a.displayName}</td>
              <td>{platformLabel(a.platform)}</td>
              <td>{cloudCell(a)}</td>
              <td class="text-sm opacity-70">{fmtTime(a.lastOkAt)}</td>
              <td>{healthCell(a)}</td>
              <td>{enabledToggle(a)}</td>
              <td class="text-right">
                <div class="flex items-center justify-end gap-1">
                  {refreshButton(a, 'desktop')}
                  {deleteButton(a)}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )

  /** 移动卡片：切了卡片每项必须带标签（§7.3）。 */
  const mobileCards = () => (
    <div class="grid gap-3 md:hidden">
      {accounts.value.map((a) => (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-2 p-4">
            <div class="flex items-center justify-between gap-2">
              <span class="break-words font-medium">{a.displayName}</span>
              <span class="badge badge-soft">{platformLabel(a.platform)}</span>
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">云朵</span>
              {cloudCell(a)}
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">最近成功</span>
              <span>{fmtTime(a.lastOkAt)}</span>
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">状态</span>
              {healthCell(a)}
            </div>
            <div class="flex items-center justify-between gap-2 border-t border-base-300 pt-2">
              {enabledToggle(a)}
            </div>
            {/* 两个动作各占一枚。移动那一档必须带 `min-h-11`：daisyUI 的 `.btn` 实测 40px，
                而 §6.1 要 ≥44px（这两枚原先都是 40px，2026-09-22 加"刷新 auth"时量出来）。
                这里没收进 `dropdown`（§5.7 那条收口）—— 它是**两个**动作，不是"行内并排三个"，
                收进抽屉反而多一次点击；到第三个动作时再收。 */}
            <div class="flex items-center justify-end gap-2">
              {refreshButton(a, 'mobile')}
              <button
                type="button"
                class="btn btn-soft btn-error min-h-11"
                disabled={deleting.value}
                onClick={() => askDelete(a)}
              >
                删除
              </button>
            </div>
          </div>
        </div>
      ))}
    </div>
  )

  /** 粘贴 auth 的弹窗。输入框是 `password` 型：那串东西能直接冒充登录。 */
  const addDialog = () => (
    <Dialog
      id={ADD_ID}
      title="粘贴 auth 添加账号"
      size="md"
      onClose={() => {
        authDraft.value = ''
        showAuth.value = false
      }}
    >
      <label class="flex flex-col gap-1 text-sm">
        <span class="opacity-70">auth 整串（带不带 `Basic ` 前缀都行）</span>
        <div class="flex items-center gap-2">
          <input
            class="input w-full"
            type={showAuth.value ? 'text' : 'password'}
            autocomplete="off"
            spellcheck={false}
            placeholder="Basic cGM6MTk5…"
            value={authDraft.value}
            onInput={(e) => (authDraft.value = (e.target as HTMLInputElement).value)}
          />
          <button
            type="button"
            class="btn btn-square btn-ghost"
            aria-label={showAuth.value ? '隐藏' : '显示'}
            onClick={() => (showAuth.value = !showAuth.value)}
          >
            {showAuth.value ? <IconEyeOff class="size-4" /> : <IconEye class="size-4" />}
          </button>
        </div>
      </label>
      <p class="text-xs leading-5 opacity-60">
        这条只用于已经在你手里的那串（浏览器控制台或另一个部署）。 端别（pc /
        手机）照字符串自带的记，不做校验；已存在的账号会被拒绝而不是覆盖。
      </p>
      <div class="modal-action">
        {/* 取消只是关窗：`Dialog` 的 onClose 会把草稿清掉，所以这里不重复清 */}
        <button type="button" class="btn btn-soft" onClick={() => closeDialog(ADD_ID)}>
          取消
        </button>
        <button
          type="button"
          class="btn btn-primary"
          disabled={adding.value || authDraft.value.trim() === ''}
          onClick={() => void submitAuth()}
        >
          {adding.value ? '添加中…' : '确认添加'}
        </button>
      </div>
    </Dialog>
  )

  /**
   * 刷新 auth 的弹窗。
   *
   * 三个理由让它开在这一页的作用域里（而不是像 `LoginDialog` 那样抽成组件）：
   *  1. 正文要显示**当前选中那一行**的名字与到期时刻，而 Vapor 下子组件元素的 props
   *     只求值一次（代码指南 §9.1）—— 抽出去会变成"点第二行、弹窗写着第一行"；
   *  2. 失败文案必须显示在**弹窗里面**（见 [refreshError] 上面那条），页顶那条在遮罩背后；
   *  3. 它只有一屏的四五行字，没有第二个调用方。
   */
  const refreshDialog = () => (
    <Dialog
      id={REFRESH_ID}
      title="刷新这个账号的 auth"
      size="md"
      onClose={() => (refreshError.value = '')}
    >
      <p class="text-sm leading-6">
        这会打一次上游，用 <span class="font-medium break-words">{refreshName()}</span>{' '}
        现有的登录态换一个新 token。
      </p>
      <p class="text-sm leading-6">
        新 token 只落库：不显示在这里、不进地址栏、也不进日志。上游最慢实测约 30
        秒，期间下面那一枚是灰的，不用重复点。
      </p>
      <p class="text-sm leading-6">
        当前到期：{refreshExpire()}（{refreshExpireState()}）
        {refreshExpiredHint()}
      </p>

      {refreshIsApp() ? (
        <p class="text-xs leading-5 opacity-60">
          这一行是手机 App 端，端档案取库里那串自带的，不用选。
        </p>
      ) : (
        <fieldset class="fieldset">
          <legend class="fieldset-legend">PC 端档案</legend>
          <select
            class="select w-full"
            aria-label="PC 端档案"
            value={refreshPlatform.value}
            onChange={(e) =>
              (refreshPlatform.value = (e.target as HTMLSelectElement).value as 'windows' | 'macos')
            }
          >
            <option value="windows">Windows 客户端</option>
            <option value="macos">macOS 客户端</option>
          </select>
          {/* 为什么这一档非要人选：库里 `accounts.platform` 记的是**端别**（pc /
              mobile），不是操作系统，而 PC 那一支的请求头要按 windows / macOS 两套档案拼。
              选错的后果是**这次刷新失败**、库里那套原样不动（窄写只在上游成功之后跑）。 */}
          <p class="fieldset-label">
            库里没记操作系统，这一档由你拍板；若这一行其实是手机那支，后端会忽略它。
          </p>
        </fieldset>
      )}

      {refreshError.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{refreshError.value}</span>
        </div>
      ) : null}

      <div class="modal-action">
        {/* 同 `addDialog`：关窗不清错误文案，靠 `Dialog` 的 onClose 那一下统一清 */}
        <button type="button" class="btn btn-soft" onClick={() => closeDialog(REFRESH_ID)}>
          取消
        </button>
        <button
          type="button"
          class="btn btn-primary"
          disabled={refreshing.value}
          onClick={() => void runRefresh()}
        >
          {refreshing.value ? '刷新中…' : '开始刷新'}
        </button>
      </div>
    </Dialog>
  )

  const actionErrorAlert = () =>
    actionError.value ? (
      <div class="alert alert-soft alert-error text-sm" role="alert">
        <span class="break-words">{actionError.value}</span>
        <button
          type="button"
          class="btn btn-square btn-ghost md:btn-xs"
          aria-label="关闭提示"
          onClick={() => (actionError.value = '')}
        >
          ×
        </button>
      </div>
    ) : null

  const noticeLine = () =>
    notice.value ? (
      <div class={NOTICE_ALERT[notice.value.tone]} role="status">
        <span class="break-words">{notice.value.text}</span>
        <button
          type="button"
          class="btn btn-square btn-ghost md:btn-xs"
          aria-label="关闭提示"
          onClick={() => (notice.value = null)}
        >
          ×
        </button>
      </div>
    ) : null

  return (
    <div class="flex flex-col gap-4">
      {header()}
      {noticeLine()}
      {actionErrorAlert()}
      {state.value === 'loading' ? loading() : null}
      {state.value === 'error' ? errorAlert() : null}
      {state.value === 'ready' && accounts.value.length === 0 ? empty() : null}
      {state.value === 'ready' && accounts.value.length > 0 ? (
        <>
          {desktopTable()}
          {mobileCards()}
        </>
      ) : null}

      {addDialog()}
      {refreshDialog()}
      <LoginDialog
        id={LOGIN_ID}
        onLoggedIn={(id) => {
          notice.value = {
            text: id ? `已登录并添加 ${id}` : '已登录并添加账号',
            tone: 'success',
          }
          void load()
        }}
      />
      <ConfirmDialog
        id={DELETE_ID}
        title="删除这个账号？"
        impact={<span class="break-words">{deleteHint.value}</span>}
        confirmLabel="确认删除"
        busy={deleting.value}
        onConfirm={() => void confirmDelete()}
      />
    </div>
  )
}
