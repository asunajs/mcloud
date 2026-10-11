/**
 * 定时任务：列表 + 新建/编辑 + 启停 + 删除。
 *
 * # 这个页面最要紧的一件事：**下次执行时间由服务端算**
 *
 * 5 段还是 6 段、周日的 0 与 7、日与周的 OR 语义 —— 这些细节各客户端
 * 各实现一遍必然分叉。所以 `nextFireAt` 直接取后端给的值。
 * 服务端算不出（表达式坏了）时给 `null`，这里显示「—」，**绝不自己猜一个**。
 *
 * # 未启用就不显示"下次执行时间"
 *
 * 后端对未启用的定时返回 `nextFireAt: null`，这里也不去本地推算 ——
 * 给一个"下次执行时间"就是在骗人。
 *
 * # 校验：前端先挡一道，后端是权威
 *
 * 前端只做"段数对不对"这一条（能立刻反馈的），其余交给后端 ——
 * 后端的错误文本**原样透出**，不翻译、不改写。
 */

import { onMounted, onUnmounted, ref } from '@asunajs/lite'
import { setPageRefresh } from '../ui/page-refresh'
import { accountName } from '../account-names'
import {
  deleteSchedule,
  listAccounts,
  listCapabilities,
  listPipelines,
  listPushEvents,
  listPushRules,
  listSchedules,
  PIPELINE_TASK,
  saveSchedule,
  type AccountSummary,
  type Pipeline,
  type PushEventOption,
  type PushRule,
  type Schedule,
  type ScheduleInput,
  type TaskMeta,
} from '../api'
import { AccountPicker } from '../ui/account-picker'
import { ConfirmDialog, Dialog, closeDialog, openDialog } from '../ui/dialog'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { IconAlert } from '../ui/icons'
import { fmtEpochMs } from '../ui/time'
import { pushToast } from '../ui/toast'
import { Select } from '../ui/select'
import { PushEventPicker } from '../ui/push-event-picker'
import { PushRuleSelect } from '../ui/push-rule-select'

type LoadState = 'loading' | 'error' | 'ready'

/** 新建时的默认时区。与后端默认值一致。 */
const DEFAULT_TZ = 'Asia/Shanghai'

/**
 * 常用表达式。点击即填入 —— 比让用户去背 cron 语法友好得多。
 *
 * 值用**字面量**写死，不做拼接：Tailwind 不参与这里，但保持
 * "不在运行期拼配置"的习惯，读起来也更直接。
 */
const PRESETS: readonly { expr: string; label: string }[] = [
  { expr: '0 8 * * *', label: '每天 08:00' },
  { expr: '0 0 8 * * *', label: '每天 08:00（带秒，6 段）' },
  { expr: '30 7 * * 1-5', label: '工作日 07:30' },
  { expr: '0 0 1 * *', label: '每月 1 日 00:00' },
]

/** 时区选项。只列后端**确实支持**的，不做自由输入 —— 写错的时区后端会报错。 */
const TIMEZONES: readonly string[] = ['Asia/Shanghai', 'UTC', 'Asia/Tokyo']

/**
 * 校验 cron 的段数。
 *
 * 只查这一条：它是用户最容易写错的，而且本地就能判断。
 * 语义是否合法（`0 0 0 30 2 *` 永不触发、`99 99 * * *` 不合法）交给后端 ——
 * 前端再实现一遍解析器只会与后端分叉。
 */
const exprFieldError = (expr: string): string | null => {
  const n = expr.trim().split(/\s+/).filter(Boolean).length
  if (n === 5 || n === 6) return null
  return `cron 表达式需要 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周），当前 ${n} 段`
}

/**
 * 上次结果的徽章样式。`null`（从未运行）单独一档，不跟任何结果混。
 *
 * ⚠ **平涂（`badge-soft`）**—— 与 `ui/history.ts` / `ui/badge.ts` / `dashboard-page`
 * 同一套写法；这一页原先用的是**实心** `badge` ✗，是全仓唯一的例外。
 * 用户口径（2026-10-02 附图）：「黑色模式下太亮了」——
 * 实心亮绿铺在深底上就是一块发光的牌子 ✗；soft 是 8% 语义色混底色，
 * 字用语义色本身，深浅两个主题看起来才是一套东西。
 */
const statusBadge = (s: Schedule['lastStatus']): { cls: string; text: string } => {
  switch (s) {
    case 'ok':
      return { cls: 'badge badge-soft badge-success badge-sm', text: '成功' }
    case 'error':
      return { cls: 'badge badge-soft badge-error badge-sm', text: '失败' }
    case 'skipped':
      return { cls: 'badge badge-soft badge-warning badge-sm', text: '已跳过' }
    default:
      return { cls: 'badge badge-ghost badge-sm', text: '从未运行' }
  }
}

/**
 * 这条定时是不是**系统维护**的（用户口径 2026-10-08：「为直播增加一个自动设置定时的
 * 功能」⇒ 后端给它落了 `managedBy: "live-auto"` ✓）。
 *
 * # 为什么要把它标出来（而不是让它长得跟别的定时一样）
 *
 * 那条定时**不是用户建的**：它的**时刻**由系统按日程重算
 *（= 下一场直播开始 + 1 小时 ✓，见 `mcloud-core::Schedule::managed_by` ✓）。
 * ⇒ 用户在这一页把时刻改掉，下次重算会被**覆盖** ✗ —— 不说清就是坑 ✓。
 *
 * # ⚠ 但**开关没有第二套**
 *
 * 翻这条定时的「停用/启用」**就是**翻用户的意愿（后端
 * `routes/schedules.rs` 里 `managed_by.is_some() ⇒ managed_wanted = input.enabled` 那一行，
 * 已有用例钉住"两处是同一格" ✓）。所以这一页**照旧**发
 * `PUT /api/schedules/{id}` ✓ —— 千万别在这里另造一个开关 ✗（两个开关会各说各话）。
 */
const isManaged = (s: Schedule) => s.managedBy === 'live-auto'

/** 「系统维护」徽章 —— 与「已启用/已停用」并列，不是替代它 ✓。 */
const MANAGED_BADGE = 'badge badge-soft badge-info badge-sm'

/**
 * 从定时的 `body` 里读出 `pipelineId`。
 *
 * 只有 `task === PIPELINE_TASK` 的定时才有这一格（后端 `upsert` 里那句校验要求它必须
 * 存在 ✓，否则连建都建不出来）。`Schedule.body` 在类型上是 `unknown` ⇒ 这里**逐层收窄**
 * 再读：直接 `body.pipelineId` 读不到，而"读不出来就当空串"的后果是**编辑一条已绑编排的
 * 定时时，那一格显示成没选** ✗ —— 用户一保存就把绑定丢了（静默的数据丢失，本页最忌讳的一类）。
 */
const pipelineIdOf = (s: Schedule): string => {
  const b = s.body
  if (b === null || typeof b !== 'object') return ''
  const v = (b as { pipelineId?: unknown }).pipelineId
  return typeof v === 'string' ? v : ''
}

export const SchedulesPage = () => {
  const state = ref<LoadState>('loading')
  /** 加载失败（整页替换）。**留在页内** —— 它要配「重试」。 */
  const errorMessage = ref('')
  /**
   * 表单失败（内联在表单旁）。
   *
   * 这一条**保持内联**：`save()` 发生在「新建/编辑定时任务」那个 `<dialog>` 里，
   * 弹出错误弹窗就是 `<dialog>` 套 `<dialog>` —— 无效结构，ESC 与遮罩会打架；
   * 而且「请填写名称」这类错误必须贴着要改的字段，弹窗把它推远了。
   * 页面里**独立动作**（启停开关）的失败走 `showError`。
   */
  const submitError = ref('')

  const items = ref<Schedule[]>([])
  const tasks = ref<TaskMeta[]>([])
  /**
   * 编排清单（`GET /api/pipelines` ✓）。
   *
   * # 为什么定时页要读它
   *
   * 编排**不是**注册表里的任务（它是一条链 ✓，见 `api.ts` 的 `PIPELINE_TASK`）⇒
   * 它不在 `/api/capabilities` 里 ⇒ 只列任务的下拉**永远选不到编排** ✗。
   * 后果就是用户报的那句：「在定时界面找不到编排任务，虽然可以通过编排界面定时」——
   * 路只通一半（编排页能反向建 ✓，定时页建不出来 ✗）。
   *
   * 这一份同时供两处用：选到编排时那个**挑哪一条**的下拉 ✓，以及列表里把
   * `body.pipelineId` 显示成编排的名字 ✓（只显示"编排"两个字，用户不知道是哪条 ✗）。
   */
  const pipelines = ref<Pipeline[]>([])
  /** 账号清单：账号那个**多选框**要用（「全部账号（N 个）」里的 N 也来自它 ✓）。 */
  const accounts = ref<AccountSummary[]>([])
  /** 推送规则清单：「推送规则」那一格的多选要用（用户口径 2026-10-08 的「行为选规则」✓）。 */
  const rules = ref<PushRule[]>([])
  /** **运行类**事件清单（结局条件那排勾选框 ✓）。面板行为那几条不在这里 ✗。 */
  const runEvents = ref<PushEventOption[]>([])
  /** 计时器：每分钟刷一次列表，让"下次执行"与"上次结果"跟上。 */
  let ticker: number | null = null

  // ── 表单状态 ──────────────────────────────────────────────────────────
  const dialogId = 'schedule-dialog'
  const editingId = ref<string | null>(null)
  const fName = ref('')
  const fTask = ref('')
  /**
   * 选到「编排」时要跑的是**哪一条**编排（`Schedule.body.pipelineId` ✓）。
   *
   * ⚠ 它**不是** `fBody` 的一部分：`fBody` 是"别的任务的参数，原样带回" ✓，
   * 而这一格是本页真正会编辑的东西 ⇒ 保存时按 `fTask` 分岔拼 `body` ✓（见 `save()`）。
   */
  const fPipelineId = ref('')
  const fAccounts = ref('')
  /**
   * 账号那一栏在界面上的形态（用户口径 2026-10-02：「这里账号和任务应该用新的多选框」）。
   *
   * 存储仍是**逗号分隔的账号 id 串** —— 后端字段就是这个 ✓、存量数据不用迁 ✓，
   * 只在**渲染与交互**这一层折成多选框要的 `all` / `picked` 两半 ✓。
   */
  const accountsAll = (): boolean => fAccounts.value.trim() === ''
  const accountsPicked = (): string[] =>
    fAccounts.value
      .split(',')
      .map((s) => s.trim())
      .filter((s) => s !== '')
  /**
   * 勾/取消「全部账号」。
   *
   * ⚠ 勾上 = **清空名单**（后端口径：空数组 = 我名下全部 ✓，以后加了账号也算 ✓）；
   * 取消 = **把当前已启用的账号全勾上**，而不是留一个空名单 ✗ ——
   * 空名单与"全部"在后端**是同一个值**，取消之后名单还是空的，用户会以为没生效 ✗
   * （用户口径 2026-10-01：「直接无需提示的全选」）。
   */
  const toggleAllAccounts = () => {
    fAccounts.value = accountsAll() ? accounts.value.map((a) => a.id).join(',') : ''
  }
  const toggleAccount = (id: string) => {
    const cur = accountsPicked()
    fAccounts.value = (cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id]).join(',')
  }
  const fExpr = ref('')
  const fTimezone = ref(DEFAULT_TZ)
  const fEnabled = ref(true)
  /**
   * 绑定的推送规则 id（**多选** ✓，用户口径「一个行为可以选择多个规则」）。
   *
   * ⚠ **必须与 `fBody` 一样"原样带回"**（见下面 `fBody` 那段说明 ✓）：
   * 漏一处的表现是"改个名字顺手把绑定丢了" ✗。
   * 空数组 = **不推** ✓（不是"全推"，也不是"回落到某个默认配置"✗）。
   */
  const fPushRules = ref<string[]>([])
  /**
   * 运行结局条件（**多选** ✓）。
   *
   * ⚠ **空数组 = 跑完就推** ✓（用户口径：「全不勾 = 跑完就推」）——
   * 不是"不推" ✗。只含**运行类**事件（`kind === 'run'` ✓）。
   */
  const fPushEvents = ref<string[]>([])
  const saving = ref(false)

  // ── 删除确认 ──────────────────────────────────────────────────────────
  const confirmId = 'schedule-delete'
  const pendingDelete = ref<Schedule | null>(null)
  const deleting = ref(false)

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      // 任务清单用于下拉框：**不硬编码任务名** —— 任务由后端注册表提供
      // 账号清单用于多选框（只取**已启用**的：`listAccounts()` 默认就不含停用的 ✓）
      // 推送规则清单用于「推送规则」那一格（不硬编码规则名 ✓）
      // 编排清单：任务下拉里的「编排」选中后要挑是哪一条（同样不硬编码 ✓）
      const [schedules, caps, accts, pushRules, pushEvents, pipes] = await Promise.all([
        listSchedules(),
        listCapabilities(),
        listAccounts(),
        listPushRules(),
        listPushEvents(),
        listPipelines(),
      ])
      items.value = schedules
      tasks.value = caps.tasks.filter((t) => !t.hidden)
      accounts.value = accts
      rules.value = pushRules
      pipelines.value = pipes
      // ⚠ 只留**运行类**：面板行为（登录/性能）没有定时可绑 ⇒ 摆进这张表单只会误导 ✗
      runEvents.value = pushEvents.filter((e) => e.kind === 'run')
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /** 只刷新列表，不动 loading 态 —— 用于计时器与动作后的静默刷新。 */
  const refresh = async () => {
    try {
      items.value = await listSchedules()
    } catch {
      // 静默失败：页面已经有一份数据，刷不到不该把它换成错误页
    }
  }

  // ── 编排 ──────────────────────────────────────────────────────────────

  /** 编排的名字。查不到（别人删了 / 不是自己的）就**退回 id** —— 显示成空是骗人 ✗。 */
  const pipelineName = (id: string) => pipelines.value.find((p) => p.id === id)?.name ?? id

  /**
   * 「执行的任务」下拉的选项。
   *
   * ⚠ **编排得手工补上**：它不在 `/api/capabilities` 里（见 `pipelines` 那段说明 ✓），
   * 所以以前这个下拉里只有任务 ⇒ 定时页建不出"跑编排"的定时 ✗。
   * 标签形状与任务那几条**保持一致**（`标题（名字）` ✓），不新造一套 ✓。
   */
  const taskOptions = () => [
    ...tasks.value.map((t) => ({ value: t.name, label: `${t.title}（${t.name}）` })),
    { value: PIPELINE_TASK, label: '编排（pipeline）' },
  ]

  /**
   * 列表里那一格显示什么：跑编排的定时要显示**是哪一条**，只写"编排"等于没说 ✓。
   *
   * ⚠ 用冒号而不是括号：编排的名字**自己就常带括号**（默认那条叫「每日一键（默认）」✓）
   * ⇒ 套起来会变成「编排（每日一键（默认））」这种双层括号 ✗。
   */
  const taskLabel = (s: Schedule) =>
    s.task === PIPELINE_TASK ? `编排：${pipelineName(pipelineIdOf(s))}` : s.task

  // ── 表单开合 ──────────────────────────────────────────────────────────

  /**
   * **原样带过去的 `body`**。
   *
   * # 为什么要有这一格（2026-10-07 用户实测报的 bug）
   *
   * 这个表单**没有参数控件**（参数在「任务配置」页编 ✓）⇒ 它以前**不发 `body`** ✗。
   * 后果两条：
   * * 停用/启用一条 `task: "pipeline"` 的定时 ⇒ 后端那句校验
   *   「这条定时要跑编排，但 body 里没有 pipelineId」⇒ **连停用都做不到** ✗
   *   （用户截图报的就是它 ✓）；
   * * 编辑一条带参数的定时（例如 `live-room` 的口令）⇒ 参数**被清空** ✗（静默的数据丢失 ✗）。
   *
   * 现在：新建是 `{}` ✓；编辑时**原样带回去** ✓（后端也做了同一件事：没给就沿用旧值 ✓，
   * 两边都堵上 —— 页面之外还有别的调用方 ✓）。
   *
   * ⚠ **唯一例外是"跑编排"**：那时 `body` 由 `fPipelineId` 现拼（`{ pipelineId }` ✓，
   * 见 `save()`）—— 编排那一格是**本页真的会改**的东西，不是"别的任务的参数"✓。
   */
  const fBody = ref<unknown>({})

  const openCreate = () => {
    editingId.value = null
    fBody.value = {}
    // 默认选第一个任务，省掉一次点击；没有任务时留空由校验拦
    fTask.value = tasks.value[0]?.name ?? ''
    // 编排那一格同理：默认选第一条（用户真要跑编排时，多半就是最上面那条 ✓）
    fPipelineId.value = pipelines.value[0]?.id ?? ''
    fAccounts.value = ''
    fExpr.value = '0 8 * * *'
    fTimezone.value = DEFAULT_TZ
    fEnabled.value = true
    // 新定时**默认一条规则都不选** ✓（不推 —— 多这一格不会改变任何东西）
    fPushRules.value = []
    // 结局条件**默认全不勾** = 跑完就推 ✓（用户口径）
    fPushEvents.value = []
    submitError.value = ''
    openDialog(dialogId)
  }

  const openEdit = (s: Schedule) => {
    editingId.value = s.id
    fBody.value = s.body ?? {}
    fName.value = s.name
    fTask.value = s.task
    // ⚠ **必须回填**：跑编排的定时如果这一格留空，用户一保存就把绑定丢了 ✗
    //（后端会回「body 里没有 pipelineId」，而那是**保存之后**才知道的）
    fPipelineId.value = pipelineIdOf(s)
    fAccounts.value = s.accounts.join(', ')
    fExpr.value = s.expr
    fTimezone.value = s.timezone
    fEnabled.value = s.enabled
    // ⚠ 别的调用方可能**没有这两格** ⇒ 当"不推 / 跑完就推"读 ✓（`?? []`）
    fPushRules.value = [...(s.pushRules ?? [])]
    fPushEvents.value = [...(s.pushEvents ?? [])]
    submitError.value = ''
    openDialog(dialogId)
  }

  const save = async () => {
    /**
     * ⚠⚠ 呈现口径（2026-10-08 用户口径「所有的提示都是优先弹窗式」）：
     * 失败**先弹 toast**（错误类不自动消失，见 `ui/toast.ts`），
     * 再把同一句放在**弹窗标题下面**（`form()` 的第一句）——
     * 那里就是用户点「保存」时看的地方。
     * ❌ 曾经只画在表单**最底下**（一堆字段之后），等于没提示。
     */
    const fail = (msg: string) => {
      submitError.value = msg
      pushToast('error', msg)
    }
    submitError.value = ''
    if (!fName.value.trim()) {
      fail('请填写名称')
      return
    }
    if (!fTask.value) {
      fail('请选择要执行的任务')
      return
    }
    // 选了编排却没挑是哪一条 ⇒ 后端会回「body 里没有 pipelineId」（400 ✓）。
    // 前端先挡一道：这条**本地就能判**（不用等一个来回），而且错的是这一格 ✓。
    if (fTask.value === PIPELINE_TASK && !fPipelineId.value) {
      fail('请选择要执行的编排')
      return
    }
    const exprErr = exprFieldError(fExpr.value)
    if (exprErr) {
      fail(exprErr)
      return
    }

    const input: ScheduleInput = {
      name: fName.value.trim(),
      task: fTask.value,
      // 逗号分隔，去掉空项：留空 = 所有已启用账号（后端语义）
      accounts: fAccounts.value
        .split(',')
        .map((x) => x.trim())
        .filter(Boolean),
      expr: fExpr.value.trim(),
      timezone: fTimezone.value,
      /**
       * ⚠ **跑编排时 `body` 就是 `{ pipelineId }`**（后端 `upsert` 里那句校验的判据 ✓）——
       * 这里**不能**把"原样带回"的 `fBody` 发出去：那是**上一个任务**的参数，后端会回
       * 「这条定时要跑编排，但 body 里没有 pipelineId」✗。
       * 反过来，别的任务照旧原样带回（见 `fBody` 那段说明 ✓），一个字都不动 ✓。
       */
      body: fTask.value === PIPELINE_TASK ? { pipelineId: fPipelineId.value } : fBody.value,
      enabled: fEnabled.value,
      // ⚠ 这两格也要**原样带回** ✓：空数组 = 不推 / 跑完就推（后端同义 ✓）
      pushRules: fPushRules.value,
      pushEvents: fPushEvents.value,
    }

    saving.value = true
    try {
      // id 由前端生成（编辑时沿用原 id）：服务端不发明 id ——
      // 那样"改一个任务"与"建一个任务"就分不出来了
      const id = editingId.value ?? newId()
      await saveSchedule(id, input)
      closeDialog(dialogId)
      pushToast('success', editingId.value ? '已保存修改' : '已创建定时任务')
      await refresh()
    } catch (e) {
      // 后端错误原样透出（弹窗 + 标题下那一句，见 `save()` 开头）
      fail(e instanceof Error ? e.message : '保存失败')
    } finally {
      saving.value = false
    }
  }

  const toggleEnabled = async (s: Schedule) => {
    try {
      /*
        ⚠ **系统维护的那条也走这里，不另造开关**（见 `isManaged` 的说明 ✓）：
        后端对 `managedBy` 非空的条目会把这一格 `enabled` 落成**用户意愿**
        （`routes/schedules.rs` 里 `managed_by.is_some() ⇒ managed_wanted = input.enabled` ✓，
        已有用例钉住"两处是同一格"✓）⇒ 直播页那张卡上的开关与这里的开关
        改的是**同一格**，不会各说各话 ✓。
      */
      await saveSchedule(s.id, {
        name: s.name,
        task: s.task,
        accounts: s.accounts,
        expr: s.expr,
        timezone: s.timezone,
        // ⚠ 这里以前**没带 body** ✗ ⇒ 一条 `task: "pipeline"` 的定时连停用都会被后端拒掉
        //（「body 里没有 pipelineId」✓，2026-10-07 用户截图报的就是它 ✓）。
        // 启停只该翻一个布尔位，别的字段**原样带回去** ✓。
        body: s.body,
        // ⚠ 这两格**同样**要带（多选）：启停只该翻一个布尔位，别的字段**原样带回去** ✓
        pushRules: [...(s.pushRules ?? [])],
        pushEvents: [...(s.pushEvents ?? [])],
        enabled: !s.enabled,
      })
      pushToast('success', s.enabled ? '已停用' : '已启用')
      await refresh()
    } catch (e) {
      // 失败必须回滚界面：乐观改了却不回滚，用户会以为改成功了。
      // 报错走弹窗 —— 启停是列表上的独立动作，没有表单在场，
      // 往 `submitError` 里写等于把错误丢进一个此刻不存在的地方（旧代码正是如此，
      // 而且还是重复报了两遍：内联一份 + 角落 toast 一份）。
      const reason = e instanceof Error ? e.message : '操作失败'
      showError(s.enabled ? '无法停用定时任务' : '无法启用定时任务', reason)
      await refresh()
    }
  }

  const askDelete = (s: Schedule) => {
    pendingDelete.value = s
    openDialog(confirmId)
  }

  const doDelete = async () => {
    const target = pendingDelete.value
    if (!target) return
    deleting.value = true
    try {
      await deleteSchedule(target.id)
      closeDialog(confirmId)
      pendingDelete.value = null
      pushToast('success', '已删除')
      await refresh()
    } catch (e) {
      // 例如被编排引用时的拒绝 —— 原样透出。
      // 走弹窗：删除是独立动作，而这里的原因（"被 X 引用"）是用户**必须读到**
      // 才知道下一步该改哪，丢进 3 秒后仍在角落的 toast 里等于没说。
      closeDialog(confirmId)
      pendingDelete.value = null
      showError('删除定时任务失败', e instanceof Error ? e.message : '删除失败')
    } finally {
      deleting.value = false
    }
  }

  onMounted(() => {
  setPageRefresh({ run: () => void load() })
    void load()
    // 一分钟一跳："下次执行时间"与"上次结果"要能自己跟上，
    // 否则用户盯着一个过期的时刻看
    ticker = window.setInterval(() => void refresh(), 60_000)
  })

  // 必须清掉：页面切走后计时器还在跑，就是一条看不见的持续请求
  onUnmounted(() => {
    setPageRefresh(null)
    if (ticker !== null) {
      clearInterval(ticker)
      ticker = null
    }
  })

  // ── 渲染 ──────────────────────────────────────────────────────────────

  const header = () => (
    <div class="flex flex-wrap items-center justify-end gap-2">
      {/* 标题由外壳那个 sticky navbar 显示（`app.tsx` 读 `navItem(id).title`）——
          页面里**不要再写一遍**：2026-10-03 之前八页各写了一次，于是每页顶部
          都是同一句话挨着出现两遍（连 `text-base font-medium` 都一模一样）✗。
          这里只放动作（添加/新建…）——「刷新」已搬到外壳 navbar 的图标上
          （见 `ui/page-refresh.ts`）。 */}
      <div class="flex items-center gap-2">
        <button
          type="button"
          class="btn btn-sm btn-primary min-h-11 md:min-h-0"
          disabled={state.value !== 'ready'}
          onClick={openCreate}
        >
          新建定时
        </button>
      </div>
    </div>
  )

  /** 一行/一卡共用的字段。表格与卡片两种形态都从这里取，避免两处不一致。 */
  const summaryOf = (s: Schedule) => (
    <>
      <div class="flex flex-wrap items-center gap-2">
        <span class="font-medium">{s.name}</span>
        {s.enabled ? (
          <span class="badge badge-sm">已启用</span>
        ) : (
          <span class="badge badge-ghost badge-sm">已停用</span>
        )}
        {/* ⚠ 与上面那两格**并列**（不是替代）：它是"谁在维护"、它们是"现在跑不跑" ✓ */}
        {isManaged(s) ? <span class={MANAGED_BADGE}>系统维护</span> : null}
        {(() => {
          const b = statusBadge(s.lastStatus)
          return <span class={b.cls}>{b.text}</span>
        })()}
      </div>
      <div class="text-xs opacity-70">
        任务 <code class="break-all">{taskLabel(s)}</code> · 表达式 <code>{s.expr}</code> · {s.timezone}
      </div>
      <div class="text-xs opacity-70">
        账号：{s.accounts.length === 0 ? '全部已启用账号' : s.accounts.map(accountName).join('、')}
      </div>
      <div class="text-xs">
        <span class="opacity-60">下次执行：</span>
        {s.enabled ? fmtEpochMs(s.nextFireAt) : '已停用，不会执行'}
      </div>
      {isManaged(s) ? (
        // ⚠ 这句话是这一页对那条自动定时**唯一**要补的信息（见 `isManaged` 的说明）：
        // 时刻由系统维护 ⇒ 改它会被覆盖；而开关与账号归用户 ✓。
        <div class="text-xs leading-5 opacity-70">
          时刻由<b class="font-medium">系统维护</b>：它按「下一场直播开始 + 1 小时」重算，
          你在这里改时刻会被下次重算覆盖。开关与账号归你（翻开关就是改"还想不想要"）。
        </div>
      ) : null}
      <div class="text-xs opacity-60">
        上次执行：{fmtEpochMs(s.lastRunMs)} · 累计触发 {s.runCount} 次
      </div>
      {s.lastError ? (
        // 后端已脱敏；原样显示，不改写、不截断到看不见
        <div class="alert alert-soft alert-error text-xs" role="alert">
          <IconAlert class="size-4 shrink-0" />
          <span class="break-words">{s.lastError}</span>
        </div>
      ) : null}
    </>
  )

  const actionsOf = (s: Schedule) => (
    <div class="flex flex-wrap gap-2">
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        onClick={() => openEdit(s)}
      >
        编辑
      </button>
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        onClick={() => void toggleEnabled(s)}
      >
        {s.enabled ? '停用' : '启用'}
      </button>
      {/*
        ⚠ **平涂**（`btn-soft`），不是实心 —— 与 `pipelines` / `accounts` 同一条约定：
        表格里的危险动作保持克制，**真正的确认在弹窗里那一下实心 `btn-error`**
        （见 `ui/dialog.tsx`）。
        用户口径（2026-10-02 附图「按钮配色问题」）：这一行原本是全仓唯一的实心红，
        整行最扎眼的反而变成了「删除」✗。
      */}
      <button
        type="button"
        class="btn btn-sm btn-soft btn-error min-h-11 md:min-h-0"
        onClick={() => askDelete(s)}
      >
        删除
      </button>
    </div>
  )

  const list = () =>
    items.value.length === 0 ? (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <EmptyState
            title="还没有定时任务。"
            hint="点右上角「新建定时」创建一条，或先挑一个常用表达式：每天 08:00 自动签到。"
            action={
              // ⚠ 编排也算一个可定的目标（用户口径 2026-10-11）⇒ 一条任务都没有、
              // 但有编排时，这个按钮**照样要给** —— 否则空页面上没有任何入口 ✗
              tasks.value.length > 0 || pipelines.value.length > 0 ? (
                <button type="button" class="btn btn-primary" onClick={openCreate}>
                  新建定时
                </button>
              ) : undefined
            }
          />
        </div>
      </div>
    ) : (
      <>
        {/* 窄屏：卡片流。表格在右边界会给每个单元格分配空间，长错误文本会把它撑破 */}
        <div class="grid gap-3 md:hidden">
          {items.value.map((s) => (
            <div key={s.id} class="card bg-base-100 shadow-sm">
              <div class="card-body gap-2">
                {summaryOf(s)}
                {actionsOf(s)}
              </div>
            </div>
          ))}
        </div>

        {/* 宽屏：表格 */}
        <div class="hidden card bg-base-100 shadow-sm md:block">
          <div class="card-body">
            <div class="overflow-x-auto">
              <table class="table">
                <thead>
                  <tr>
                    <th>名称</th>
                    <th>任务</th>
                    <th>下一次</th>
                    <th>上次结果</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {items.value.map((s) => (
                    <tr key={s.id}>
                      <td>
                        <div class="font-medium">{s.name}</div>
                        {/* ⚠ 宽屏那一份也要标（卡片流与表格是两套 DOM ✓）——
                            只标一处的话，"系统维护"在宽屏下根本看不见 ✗ */}
                        {isManaged(s) ? (
                          <div class="mt-1">
                            <span class={MANAGED_BADGE}>系统维护</span>
                          </div>
                        ) : null}
                        <div class="text-xs opacity-60">
                          <code>{s.expr}</code> · {s.timezone}
                        </div>
                        {isManaged(s) ? (
                          <div class="text-xs opacity-60">时刻由系统维护，改它会被下次重算覆盖</div>
                        ) : null}
                        {s.lastError ? (
                          <div class="mt-1 text-xs text-error break-words">{s.lastError}</div>
                        ) : null}
                      </td>
                      <td class="text-xs">
                        <code>{taskLabel(s)}</code>
                        <div class="opacity-60">
                          {s.accounts.length === 0 ? '全部账号' : `${s.accounts.length} 个账号`}
                        </div>
                      </td>
                      <td class="text-xs">
                        {s.enabled ? fmtEpochMs(s.nextFireAt) : '—'}
                        {!s.enabled ? <div class="opacity-60">已停用</div> : null}
                      </td>
                      <td>
                        {(() => {
                          const b = statusBadge(s.lastStatus)
                          return (
                            <div class="flex flex-col gap-1">
                              <span class={b.cls}>{b.text}</span>
                              <span class="text-xs opacity-60">{fmtEpochMs(s.lastRunMs)}</span>
                            </div>
                          )
                        })()}
                      </td>
                      <td>{actionsOf(s)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </>
    )

  const form = () => (
    <>
      {/*
        ⚠⚠ 失败那一句**放在最前**（弹窗标题下面），不是表单底部：
        用户点「保存」时视线在弹窗标题那一行，红字跑到一堆字段之后等于没提示
        （2026-10-08 用户口径「所有的提示都是优先弹窗式」✓）。
        弹窗那一份由 `save()` 里的 `pushToast('error', …)` 给 ✓。
      */}
      {submitError.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <IconAlert class="size-4 shrink-0" />
          <span class="break-words">{submitError.value}</span>
        </div>
      ) : null}

      <fieldset class="fieldset">
        <legend class="fieldset-legend">名称</legend>
        <input
          class="input w-full"
          placeholder="例如 每天签到"
          value={fName.value}
          onInput={(e) => {
            fName.value = (e.target as HTMLInputElement).value
          }}
        />
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">执行的任务</legend>
        <Select
          id="sched-task"
          class="w-full"
          ariaLabel="执行的任务"
          placeholder="请选择任务"
          value={fTask.value}
          options={taskOptions()}
          onChange={(v) => {
            fTask.value = v
          }}
        />
      </fieldset>

      {/*
        ⭐ 「哪一条编排」——**只在选到编排时出现** ✓。

        用户口径（2026-10-11）：「在定时界面找不到编排任务，虽然可以通过编排界面定时」
        ⇒ 定时页的目标里要能选到编排 ✓，而且选了之后要能挑**是哪一条**（列出用户自己的
        编排、显示名字 ✓）。控件沿用本页既有那套 `Select` ✓（与上面「执行的任务」
        同一个形状），不新造一套 ✗。

        ⚠ 条件分支只认 JSX / `null`（lite 的硬约束 ✓）⇒ 这里写成三元 + `null` ✓。
      */}
      {fTask.value === PIPELINE_TASK ? (
        <fieldset class="fieldset">
          <legend class="fieldset-legend">要跑的编排</legend>
          <Select
            id="sched-pipeline"
            class="w-full"
            ariaLabel="要跑的编排"
            filterable
            placeholder="请选择编排"
            emptyText="还没有编排"
            value={fPipelineId.value}
            options={pipelines.value.map((p) => ({ value: p.id, label: p.name }))}
            onChange={(v) => {
              fPipelineId.value = v
            }}
          />
          <p class="text-xs leading-5 opacity-60">
            {pipelines.value.length === 0
              ? '还没有编排 —— 先到「编排」页把几个任务串成一条链，再回来给它定时。'
              : '到点把这条链整条跑一遍（步骤由编排决定，账号用上面选的那份）。'}
          </p>
        </fieldset>
      ) : null}

      <fieldset class="fieldset">
        <legend class="fieldset-legend">账号</legend>
        {/*
          多选框（用户口径 2026-10-02：「这里账号和任务应该用新的多选框」）。

          ⚠ 换掉的是**裸文本输入**（原来那句是"账号 id，逗号分隔；留空 = 全部已启用账号"✗）。
          那一栏本身就是最坑的一处：后端拿这串**原样**当 `AccountId` 比对，而 `AccountId`
          是手机号的 SHA-256 前 16 位（`0f6050105ab111f4`）—— 照文案填手机号 ⇒
          **匹配 0 个账号 ⇒ 定时任务静默不跑**（不报错、不提示，最难查的一类）✗。
          改成勾选之后，值只可能来自**真实存在的账号** ✓ ⇒ 这一整类失败模式消失 ✓
          （不用再靠一句警告文案去防 ✓）。

          ⚠ 与任务页/直播页共用 `ui/account-picker.tsx` ⇒ 「全部」的口径只有一处
          （后端空数组 = 我名下全部 ✓），不会三处各写一遍。
        */}
        <AccountPicker
          accounts={accounts.value}
          all={accountsAll()}
          picked={accountsPicked()}
          onToggleAll={toggleAllAccounts}
          onToggle={toggleAccount}
        />
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">cron 表达式</legend>
        <input
          class="input w-full font-mono"
          placeholder="0 8 * * *"
          value={fExpr.value}
          onInput={(e) => {
            fExpr.value = (e.target as HTMLInputElement).value
          }}
        />
        <p class="text-xs opacity-60">
          5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周）。时区由下一项决定。
        </p>
        <div class="mt-1 flex flex-wrap gap-1">
          {PRESETS.map((p) => (
            <button
              key={p.expr}
              type="button"
              class="btn btn-xs btn-soft min-h-9"
              onClick={() => {
                fExpr.value = p.expr
              }}
            >
              {p.label}
            </button>
          ))}
        </div>
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">时区</legend>
        <Select
          id="sched-timezone"
          class="w-full"
          ariaLabel="时区"
          filterable
          value={fTimezone.value}
          options={TIMEZONES.map((tz) => ({ value: tz, label: tz }))}
          onChange={(v) => {
            fTimezone.value = v
          }}
        />
      </fieldset>

      {/*
        推送（用户口径 2026-10-08：「**行为选择规则，一个行为可以选择多个规则**」✓）。
        一个都不选 = **不推**；结局条件全不勾 = **跑完就推** ✓。
      */}
      <fieldset class="fieldset">
        <legend class="fieldset-legend">推送规则（可多选）</legend>
        <PushRuleSelect
          id="sched-push-rule"
          rules={rules.value}
          value={fPushRules.value}
          onChange={(v) => {
            fPushRules.value = v
          }}
        />
        <p class="text-xs leading-5 opacity-60">
          一个都不选 = 不推。选了规则就按**每条规则自己**的平台、凭据、标题发
          —— 规则在「推送」页里建。可多选，每条规则各发一份。
        </p>
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">运行结局条件</legend>
        <PushEventPicker
          id="sched-push-events"
          events={runEvents.value}
          value={fPushEvents.value}
          onChange={(v) => {
            fPushEvents.value = v
          }}
        />
        <p class="text-xs leading-5 opacity-60">
          只在勾中的结局下推。<strong>全不勾 = 跑完就推</strong>（不看结局）。
        </p>
      </fieldset>

      <label class="label cursor-pointer justify-start gap-3">
        <input
          id="sched-enabled"
          type="checkbox"
          class="checkbox"
          checked={fEnabled.value}
          // 用 onChange 而不是 onInput：复选框没有"输入中"的中间态
          onChange={(e) => {
            fEnabled.value = (e.target as HTMLInputElement).checked
          }}
        />
        <span class="text-sm">启用（停用后不会自动执行）</span>
      </label>
    </>
  )

  return (
    <div class="flex flex-col gap-4">
      {header()}

      {state.value === 'loading' ? (
        <div class="flex justify-center py-10">
          <span class="loading" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="加载失败。"
              hint={errorMessage.value}
              action={
                <button type="button" class="btn btn-soft" onClick={() => void load()}>
                  重试
                </button>
              }
            />
          </div>
        </div>
      ) : null}

      {state.value === 'ready' ? list() : null}

      {/* 子节点走 Dialog 内部的默认插槽（lite 里子节点不进 props：`props.children` 恒为 undefined） */}
      <Dialog
        id={dialogId}
        title={editingId.value ? '编辑定时任务' : '新建定时任务'}
        size="lg"
      >
        {form()}
        <div class="modal-action">
          <button type="button" class="btn btn-soft" onClick={() => closeDialog(dialogId)}>
            取消
          </button>
          <button
            type="button"
            class="btn btn-primary"
            disabled={saving.value}
            onClick={() => void save()}
          >
            {saving.value ? '保存中…' : '保存'}
          </button>
        </div>
      </Dialog>

      <ConfirmDialog
        id={confirmId}
        title="删除定时任务"
        impact={
          <span>
            将删除「{pendingDelete.value?.name ?? ''}」。
            {pendingDelete.value?.enabled ? '它当前是启用状态，删除后不会再自动执行。' : ''}
            已产生的运行历史不受影响。
          </span>
        }
        confirmLabel="删除"
        busy={deleting.value}
        onConfirm={() => void doDelete()}
      />
    </div>
  )
}

/** 生成一个本地 id。只含后端允许的字符集。 */
const newId = (): string =>
  `sch-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
