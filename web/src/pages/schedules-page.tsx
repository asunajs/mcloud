/**
 * 定时任务页：列表 + 建/改/删/启停。
 *
 * # 这一页接的是哪三条路由
 *
 * `GET/PUT/DELETE /api/schedules`（对位 `routes.go` 的 `listSchedules` /
 * `putSchedule` / `deleteSchedule`），再加两份只读的配合数据：
 * `/api/capabilities`（目标选项 + 参数键名）、`/api/accounts`（勾账号）、
 * `/api/status`（调度循环在不在跑）。
 *
 * # 三件界面上必须说清、但很容易糊掉的事
 *
 *  1. **下次执行时间是服务端算的**（`nextFireAt`），前端不参与 —— cron 的段数分流、
 *     日/周取 OR、周日 `0` 与 `7` 这四处语义只要前端复现一遍就会分叉，
 *     所以连"这条 cron 合法吗"都不在这里判（详见 `ui/cron.ts` 顶部那段）；
 *  2. **`PUT` 是全量覆盖**，所以"只改启用状态"也得把整行送回去（见 [draftOfRow]）。
 *     后端替我们留住了 `created_at`、水位与 `miss_policy`，界面上就别再摆"部分保存"的姿态；
 *  3. **`miss_policy` 这一栏只能看不能改**：请求体里没有它（`scheduleBody` 那一族），
 *     所以这一页显示它，但**不**提供编辑入口 —— 摆一个改不动的下拉是骗人。
 *
 * # 为什么两个弹窗都写在本页作用域里
 *
 * 编辑窗：与账号页那条同理（`accounts-page.tsx` 的 `refreshDialog`）—— Vapor 下**子组件**
 * 的元素 props 只求值一次，弹窗正文要跟着"当前编辑哪一行"走，所以整窗由本页的
 * `editDialog()` 在子节点位置调用（切换编辑行 ⇒ 外层 `setNodes` 重建这一片，标题与正文跟着换）。
 *
 * 删除窗：不用 `ui/dialog` 的 `ConfirmDialog`，理由只有一条 —— 它的两枚按钮是 daisyUI
 * 默认高 **40px**，够不到 §6.1 的 44。本页按 `webui-components.md` §7.4 那条补记的既有先例
 * （"新页面只垫自己那几枚，不动共享件"），把这两枚写成 `min-h-11 md:min-h-0`；
 * 共享件要不要全局加高仍是留给用户拍板的那一条。
 * ⚠ 量这种高度要等弹窗**开场那一下缩放**过去再读：`modal-box` 打开的头几帧带 0.95 倍缩放，
 * 当场读会把 44 读成 42。§7.4 那条补记写的 38 与本轮等到的 40 差同一个比例，
 * ⚠ 但那条当时的测法没复核，这里只登记现象、不改那个数字。
 *
 * ⚠ 耻辱柱（2026-09-23）：本处曾写"用 `impact` prop 传文案会停在第一次那一行，实机连着开
 * 两次验证过"。那是**点完立刻同步读 `innerText`** 造成的误测 —— Vapor 的更新在微任务之后才落。
 * 隔 120ms 再读，同一个 `ConfirmDialog` 连着换两行都显示正确 ⇒ prop 里那片 JSX 的**子节点
 * 位置**仍自带 effect。"只求值一次"这句的确切边界本轮没测完（值 prop 会不会跟新没验），
 * 要一份状态稳定可读，写成本页元素上的**属性**（`disabled={…}`）与**子节点**最稳。
 *
 * ⚠ 由此有一条写码纪律：本页这些"返回一片节点"的辅助函数里，**读 ref 只能发生在
 * JSX 子节点位置**。子节点位置的读取编译成 `createIf` / `setNodes` / `renderEffect`
 * 的**局部** effect；而函数体顶上的 `const x = draft.value.preset` 会被调用点那个
 * `setNodes(() => editDialog())` 收走依赖 —— 于是每敲一个字整弹窗重建，输入框失焦。
 * （删除窗的 `<dialog>` 本体不被重建，是实测过的：连着换两行，`<p>` 与 `<dialog>`
 * 两个节点的引用都没变。）
 */

import { computed, onMounted, ref } from 'vue'
import {
  ApiError,
  deleteSchedule,
  listAccounts,
  listCapabilities,
  listSchedules,
  putSchedule,
  serviceStatus,
  type Account,
  type CapabilityRow,
  type ScheduleDraft,
  type ScheduleRow,
  type ScheduleTarget,
} from '../api'
import { closeDialog, Dialog, openDialog } from '../ui/dialog'
import {
  buildCron,
  CRON_PRESETS,
  CRON_PRESET_LABEL,
  describeCron,
  emptyCronDraft,
  MISS_POLICY_LABEL,
  newScheduleId,
  readCron,
  WEEKDAY_LABEL,
  WEEKDAY_VALUES,
  type CronDraft,
} from '../ui/cron'
import { fmtTime } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/** 弹窗 id 全站唯一（§5.8）。 */
const EDIT_ID = 'dlg-schedule-edit'
const DELETE_ID = 'dlg-schedule-delete'

/**
 * 上次执行结果 → 中文 + badge 修饰类（基类 `badge` 由 `statusBadge` 那头的字面量给，
 * 与 `ENABLED_BADGE` 同一套写法）。
 *
 * 类名整条字面量（§9.1①）；一律 `badge-soft`，且**不给** `badge-neutral`
 * （暗色下黑字压黑底，实测 1.23:1，见组件指南 §5.4 那条补记）。
 * ⚠ 那一句"未知值回落到原始英文码"在**当前 HTTP 面上到不了**：存储层把不认识的
 * `last_status` 直接读成 nil（`internal/io/configstore_test.go` 里"未知状态应读成 nil"
 * 那条判据），于是界面走的是"从没跑过"那一支。`未知状态 <code>` 这一支留着是因为
 * 形状一旦放宽就会无声，但**本轮没在界面上验过它**（要验得先绕过后端那道读法）。
 */
const LAST_STATUS: Record<string, { label: string; badge: string }> = {
  ok: { label: '成功', badge: 'badge-soft badge-success' },
  error: { label: '失败', badge: 'badge-soft badge-error' },
  skipped: { label: '跳过', badge: 'badge-soft badge-warning' },
}

/** 启停那一档的两种状态必须**一眼分得开**（账号页同一件事被用户点过一次名）。 */
const ENABLED_BADGE = { on: 'badge-soft badge-success', off: 'badge-soft' } as const

const NOTICE_ALERT = {
  success: 'alert alert-soft alert-success text-sm',
  info: 'alert alert-soft alert-info text-sm',
} as const

type NoticeTone = keyof typeof NOTICE_ALERT

/** 目标是不是"任务组合"。这一页不能建也不能改它，只能原样带回去（见 `targetLine`）。 */
const isPipeline = (target: ScheduleTarget | null): boolean => target?.kind === 'pipeline'

/** 组合在本页的可见性：`GET /api/pipelines` 这条路由**还不存在**，所以列不出候选。 */
const pipelineNote = (id: string) => `任务组合 ${id}（这一页不提供改目标：组合列表还没有对外接口）`

export const SchedulesPage = () => {
  const state = ref<LoadState>('loading')
  const rows = ref<ScheduleRow[]>([])
  const accounts = ref<Account[]>([])
  const caps = ref<CapabilityRow[]>([])
  const errorMessage = ref('')

  /** null = 这一轮还没问到；不当成"没在跑"（首屏闪一条红警告是谎报）。 */
  const schedulerOn = ref<boolean | null>(null)

  /** 一次只让一个写操作在飞：`busyId` 记那一行（新建时是 `'new'`）。 */
  const busyId = ref('')
  const notice = ref<{ text: string; tone: NoticeTone } | null>(null)
  const actionError = ref('')

  // ── 编辑弹窗的那一份草稿 ──
  /** 空串 = 新建；否则是**已存在**那一行的 id（id 不可改，它就是主键）。 */
  const editingId = ref('')
  const nameDraft = ref('')
  /** 能力 code。`''` 只在编辑组合那一行出现（那时这一栏不显示）。 */
  const codeDraft = ref('')
  /** 编辑组合时**原样带回**的目标；不是组合时为 null。 */
  const pipelineDraft = ref<ScheduleTarget | null>(null)
  const cronDraft = ref<CronDraft>(emptyCronDraft())
  const accountsDraft = ref<string[]>([])
  /** 参数文本框。提交时才 `JSON.parse`，中途允许它是半截的（边改边报红字没法打字）。 */
  const bodyDraft = ref('{}')
  const enabledDraft = ref(true)
  /** 弹窗内的错误：**必须显示在弹窗里面** —— 页顶那条在遮罩背后看不见。 */
  const formError = ref('')

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      // 四份一起拉：目标下拉与参数键名缺一样都会让表单**填不对**，
      // 与其让某一格空着让人以为"没有能力"，不如整页报错重试。
      const [s, a, c, status] = await Promise.all([
        listSchedules(),
        listAccounts(),
        listCapabilities(),
        serviceStatus(),
      ])
      rows.value = s.schedules
      accounts.value = a.accounts
      caps.value = c.all
      schedulerOn.value = status.schedulerRunning
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => void load())

  /** 能力 code → 展示名。查不到（能力被摘了、导入来的旧串）就回落到 code 本身。 */
  const capTitle = (code: string) => caps.value.find((c) => c.code === code)?.title ?? code

  /** 声明的参数键名。空数组 = 这一项没有能力表可比（组合或已摘除的能力），此时不校验键。 */
  const paramKeys = (code: string): string[] =>
    caps.value.find((c) => c.code === code)?.params.map((p) => p.key) ?? []

  /**
   * 「执行」那一格。
   *
   * 没有目标的那一行**不写成"会执行失败"**：调度那边是直接跳过它
   * （`scheduler.go` 的 `fireScheduleIfDue` 第一道闸：`Target == nil` 连触发点都不判），
   * 记不进 `last_status`。
   */
  const targetText = (target: ScheduleTarget | null): string => {
    if (!target) return '（这一行没有目标，调度会跳过它）'
    if (isPipeline(target)) return pipelineNote(target.id ?? '')
    return capTitle(target.code ?? '')
  }

  const accountText = (row: ScheduleRow) => {
    if (row.accountIds.length === 0) return '全部账号'
    const names = row.accountIds.map(
      (id) => accounts.value.find((a) => a.id === id)?.displayName ?? id,
    )
    return names.join('、')
  }

  /**
   * 「下次执行」那一格。
   *
   * `nextFireAt` 为 null 有两种来路（后端 `schedulePayload` 写明）：停用、或这条 cron
   * 算不出触发点。停用先看 `enabled`，剩下的那种才说"不会触发"——
   * 反过来说就是谎报配置错了。
   */
  const nextFireText = (row: ScheduleRow) => {
    if (!row.enabled) return '已停用，不会触发'
    return row.nextFireAt ? fmtTime(row.nextFireAt) : '不会触发（或这条 cron 算不出）'
  }

  const missPolicyText = (row: ScheduleRow) =>
    row.missPolicy ? (MISS_POLICY_LABEL[row.missPolicy] ?? row.missPolicy) : '未单独设置'

  // ── 弹窗开关 ──
  const openNew = () => {
    editingId.value = ''
    nameDraft.value = ''
    codeDraft.value = caps.value.find((c) => !c.hidden)?.code ?? caps.value[0]?.code ?? ''
    pipelineDraft.value = null
    cronDraft.value = emptyCronDraft()
    accountsDraft.value = []
    bodyDraft.value = '{}'
    enabledDraft.value = true
    formError.value = ''
    openDialog(EDIT_ID)
  }

  /** 草稿上的一个补丁：**整串换掉**而不是改字段，Vapor 才收得到（与小红花页那条同理）。 */
  const patchCron = (patch: Partial<CronDraft>) => {
    cronDraft.value = { ...cronDraft.value, ...patch }
  }

  const openEdit = (row: ScheduleRow) => {
    editingId.value = row.id
    nameDraft.value = row.name
    codeDraft.value = row.target?.kind === 'capability' ? (row.target.code ?? '') : ''
    pipelineDraft.value = isPipeline(row.target) ? row.target : null
    cronDraft.value = readCron(row.cron)
    accountsDraft.value = [...row.accountIds]
    bodyDraft.value = JSON.stringify(row.body ?? {}, null, 2)
    enabledDraft.value = row.enabled
    formError.value = ''
    openDialog(EDIT_ID)
  }

  /** 文本框里的参数 → 对象。返回 null 表示"这一份不能送"，原因已经写进 [formError]。 */
  const parseBody = (): Record<string, unknown> | null => {
    const text = bodyDraft.value.trim()
    if (text === '') {
      formError.value = '参数不是一个 JSON 对象：至少写成 {}'
      return null
    }
    let parsed: unknown
    try {
      parsed = JSON.parse(text)
    } catch {
      // 不回抄 parser 那句英文（它带用户写到一半的片段），也不猜"少了个逗号"在哪：
      // 这一栏填到一半是常态，报红字只是说明它现在还不是 JSON。
      formError.value = '参数还不是合法 JSON：先写成完整的一串（什么都不填就写 {}）'
      return null
    }
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
      formError.value = '参数必须是一个 JSON 对象（数组与字符串都不收）'
      return null
    }
    // 键名这一道值得在前端挡：调度到点时 `checkParamKeys` 会拒未知键，
    // 而拒的结果是一行 `error` 记录 —— 也就是说存得下去、跑得必红。
    const declared = codeDraft.value ? paramKeys(codeDraft.value) : []
    if (declared.length > 0) {
      const unknown = Object.keys(parsed).filter((k) => !declared.includes(k))
      if (unknown.length > 0) {
        formError.value =
          `不认识的参数：${unknown.join('、')}` +
          `（这一项能填：${declared.join('、')}）`
        return null
      }
    }
    return parsed as Record<string, unknown>
  }

  const save = async () => {
    formError.value = ''
    if (nameDraft.value.trim() === '') {
      formError.value = '任务名不能为空'
      return
    }
    const cron = buildCron(cronDraft.value)
    if (cron === null) {
      formError.value = '频率没填完：时刻 / 分钟那一栏还空着或超出范围'
      return
    }
    if (pipelineDraft.value === null && codeDraft.value === '') {
      formError.value = '没有可执行的能力：先去账号页登录，或让后端注册一个能力'
      return
    }
    const body = parseBody()
    if (body === null) return

    const target: ScheduleTarget = pipelineDraft.value ?? {
      kind: 'capability',
      code: codeDraft.value,
    }
    const id = editingId.value || newScheduleId()
    busyId.value = id
    try {
      await putSchedule(id, {
        name: nameDraft.value.trim(),
        target,
        cron,
        account_ids: accountsDraft.value,
        body,
        enabled: enabledDraft.value,
      })
      closeDialog(EDIT_ID)
      notice.value = {
        text: editingId.value ? `已保存 ${nameDraft.value.trim()}` : `已新建 ${nameDraft.value.trim()}`,
        tone: 'success',
      }
      await load()
    } catch (e) {
      // 后端的 4xx 文案是中文且具体（"这个 cron 永远不会触发: …"、"任务 id 只能用…"），
      // 所以**原样**显示在弹窗里，不改写 —— 改写就是造第二份措辞。
      formError.value = e instanceof ApiError ? e.message : '保存失败'
    } finally {
      busyId.value = ''
    }
  }

  /**
   * 列表一行 → 一份完整的 `PUT` 请求体。
   *
   * `PUT` 是全量覆盖，所以"只翻启用状态"也得把整行送回去。三处必须原样带、不能"整理"：
   *  - `target`：组合那一种本页列不出候选，改不了就**别改**（见 `pipelineNote`）；
   *  - `body`：库里存的是什么就送什么（可能是数组 —— 那是旧导入的形状，
   *    到点会报"参数不是一个 JSON 对象"，但那是**已有事实**，不该被一次翻开关洗掉）；
   *  - `account_ids`：读侧的键是 `accountIds`（不对称在 `api.ts` 顶部记着）。
   */
  const draftOfRow = (row: ScheduleRow, enabled: boolean): ScheduleDraft => ({
    name: row.name,
    target: row.target ?? { kind: 'capability', code: '' },
    cron: row.cron,
    timezone: row.timezone,
    account_ids: row.accountIds,
    body: row.body ?? {},
    enabled,
  })

  /** 行内启停。**没有窄接口**（不像账号那条 `/enabled`），所以它是一次真覆盖。 */
  const setEnabled = async (row: ScheduleRow, enabled: boolean) => {
    busyId.value = row.id
    actionError.value = ''
    try {
      await putSchedule(row.id, draftOfRow(row, enabled))
      notice.value = {
        text: `${row.name} 已${enabled ? '启用' : '停用'}`,
        tone: 'info',
      }
      await load()
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '改启用状态失败'
    } finally {
      busyId.value = ''
    }
  }

  // ── 删除 ──
  const deleteRow = ref<ScheduleRow | null>(null)
  const deleting = ref(false)
  const deleteHint = computed(() => {
    const row = deleteRow.value
    if (!row) return ''
    return `删除「${row.name}」这一条定时任务。连带范围只有一行：执行历史与云盘账号都不跟着走，账号的启用状态也不动。`
  })
  const askDelete = (row: ScheduleRow) => {
    deleteRow.value = row
    openDialog(DELETE_ID)
  }
  const confirmDelete = async () => {
    const row = deleteRow.value
    if (!row) return
    deleting.value = true
    actionError.value = ''
    try {
      await deleteSchedule(row.id)
      closeDialog(DELETE_ID)
      deleteRow.value = null
      notice.value = { text: `已删除 ${row.name}`, tone: 'info' }
      await load()
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '删除失败'
    } finally {
      deleting.value = false
    }
  }

  const toggleAccount = (id: string, on: boolean) => {
    const rest = accountsDraft.value.filter((x) => x !== id)
    accountsDraft.value = on ? [...rest, id] : rest
  }

  // ── 渲染 ──
  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="text-base font-medium">定时任务</h1>
        {state.value === 'ready' ? <span class="badge badge-soft">{rows.value.length}</span> : null}
      </div>
      {/* 移动端这一枚收进底部操作条（§7.2），这里只给桌面 */}
      <div class="hidden items-center gap-2 md:flex">
        <button
          type="button"
          class="btn btn-sm btn-soft"
          disabled={state.value === 'loading'}
          onClick={() => void load()}
        >
          刷新
        </button>
        <button type="button" class="btn btn-sm btn-primary" onClick={openNew}>
          新建任务
        </button>
      </div>
    </div>
  )

  /**
   * 调度循环没在跑 ⇒ 这一页所有任务都不会自己动。
   *
   * 这一条**必须**在页面上：库里存着任务、界面列着"下次执行 07:30"，
   * 而进程没起循环时那句下次时间是纯装饰 —— 用户下一个动作就是去重启服务，
   * 而重启也解决不了（那是配置项，不是崩了）。
   */
  const schedulerAlert = () =>
    state.value === 'ready' && schedulerOn.value === false ? (
      <div class="alert alert-soft alert-warning text-sm" role="alert">
        <span class="break-words">
          后端这一轮的定时调度没有运行，下面这些"下次执行"只是按 cron 算出来的时刻，到点不会有人去跑。
        </span>
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

  const statusBadge = (row: ScheduleRow) => {
    if (!row.lastStatus) return <span class="badge badge-soft">从没跑过</span>
    const known = LAST_STATUS[row.lastStatus]
    const mod = known ? known.badge : 'badge-soft badge-warning'
    return (
      <span class={`badge ${mod}`}>
        {known ? known.label : `未知状态 ${row.lastStatus}`}
      </span>
    )
  }

  /** 上次结果 + 那句错误。错误**不靠 tooltip**（§6.2：移动端没有 hover）。 */
  const lastCell = (row: ScheduleRow) => (
    <div class="flex flex-col gap-1">
      <div class="flex items-center gap-2">
        {statusBadge(row)}
        <span class="text-xs opacity-70">
          {row.lastRunAt ? fmtTime(row.lastRunAt) : '—'}
        </span>
      </div>
      {row.lastError ? <span class="max-w-prose break-words text-xs">{row.lastError}</span> : null}
      <span class="text-xs opacity-60">错过时：{missPolicyText(row)}</span>
    </div>
  )

  const enabledToggle = (row: ScheduleRow) => (
    <label class="flex min-h-11 cursor-pointer items-center gap-2">
      <input
        type="checkbox"
        class="toggle toggle-primary"
        checked={row.enabled}
        disabled={busyId.value !== ''}
        aria-label={`启用 ${row.name}`}
        onChange={(e) => void setEnabled(row, (e.target as HTMLInputElement).checked)}
      />
      <span class={`badge ${ENABLED_BADGE[row.enabled ? 'on' : 'off']}`}>
        {row.enabled ? '已启用' : '已停用'}
      </span>
    </label>
  )

  /**
   * 行操作收进 dropdown（§5.7）：两枚并排在窄屏会挤，三枚必崩。
   *
   * ⚠ 那一枚"⋯"必须是**可聚焦**的：daisyUI 5.7 的 `.dropdown` 靠 `:focus-within`
   * 展开（`dropdown.css` 里 `.dropdown:not(details,.dropdown-open,.dropdown-hover:hover,:focus-within) .dropdown-content{display:none}`），
   * 而 `div` 天生不可聚焦 ⇒ 不给 `tabindex` 就是**点开不了**，且不报任何错。
   * 用 `<button>` 更语义化，但 §5.7 定的就是 `div[tabindex]` 那一套，先跟它一致。
   */
  const rowMenu = (row: ScheduleRow) => (
    <div class="dropdown dropdown-end">
      <div tabindex={0} role="button" class="btn btn-square btn-ghost min-h-11" aria-label={`${row.name} 的操作`}>
        ⋯
      </div>
      <ul tabindex={0} class="menu dropdown-content z-50 w-40 rounded-box bg-base-100 p-2 shadow">
        <li>
          {/* 菜单项默认只有 32px 高（daisyUI 的 `.menu` 条目），窄屏点不中 ⇒ §6.1 的 44；
              桌面回落到默认密度（`table-sm` 一行没那么高）。 */}
          <button type="button" class="min-h-11 md:min-h-0" onClick={() => openEdit(row)}>
            编辑
          </button>
        </li>
        <li>
          <button type="button" class="text-error min-h-11 md:min-h-0" onClick={() => askDelete(row)}>
            删除
          </button>
        </li>
      </ul>
    </div>
  )

  const desktopTable = () => (
    <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
      <table class="table table-sm">
        <thead>
          <tr>
            <th>任务</th>
            <th>执行</th>
            <th>频率</th>
            <th>账号</th>
            <th>下次执行</th>
            <th>上次结果</th>
            <th>启用</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {rows.value.map((row) => (
            <tr>
              <td class="font-medium break-words">{row.name}</td>
              <td class="break-words">{targetText(row.target)}</td>
              <td class="break-words">{describeCron(row.cron)}</td>
              <td class="max-w-[16rem] break-words">{accountText(row)}</td>
              <td class="break-words">{nextFireText(row)}</td>
              <td>{lastCell(row)}</td>
              <td>{enabledToggle(row)}</td>
              <td class="text-right">{rowMenu(row)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )

  /** 移动卡片：每项带标签（§7.3）。 */
  const mobileCards = () => (
    <div class="grid gap-3 pb-24 md:hidden">
      {rows.value.map((row) => (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-2 p-4">
            <div class="flex items-center justify-between gap-2">
              <span class="break-words font-medium">{row.name}</span>
              <span class="badge badge-soft">{row.enabled ? '已启用' : '已停用'}</span>
            </div>
            <div class="flex items-start justify-between gap-2 text-sm">
              <span class="shrink-0 opacity-70">执行</span>
              <span class="break-words text-right">{targetText(row.target)}</span>
            </div>
            <div class="flex items-center justify-between gap-2 text-sm">
              <span class="opacity-70">频率</span>
              <span class="break-words text-right">{describeCron(row.cron)}</span>
            </div>
            <div class="flex items-center justify-between gap-2 text-sm">
              <span class="opacity-70">账号</span>
              <span class="break-words text-right">{accountText(row)}</span>
            </div>
            <div class="flex items-center justify-between gap-2 text-sm">
              <span class="opacity-70">下次执行</span>
              <span class="break-words text-right">{nextFireText(row)}</span>
            </div>
            <div class="flex items-start justify-between gap-2 text-sm">
              <span class="shrink-0 opacity-70">上次结果</span>
              <div class="flex flex-col items-end gap-1">{lastCell(row)}</div>
            </div>
            <div class="flex items-center justify-between gap-2 border-t border-base-300 pt-2">
              {enabledToggle(row)}
              {rowMenu(row)}
            </div>
          </div>
        </div>
      ))}
    </div>
  )

  /**
   * 移动端底部固定操作条（§7.2 给这一页定的形态）。
   *
   * `bottom-[4.25rem]`（= 68px）是**量出来的**：外壳那圈 dock 实测占底部 64px
   * （493×530 视口下 `getBoundingClientRect()`：dock top=466 / height=64，
   * 操作条 bottom=462）⇒ 68px 把它顶在 dock 上方留 4px 缝，而不是盖住它
   * （盖住 = 移动端没法切页）。同理 `main` 已有 `pb-24` 给 dock 让位，
   * 卡片列表自己再垫 `pb-24` 给这一条让位。
   */
  const mobileBar = () => (
    <div class="fixed inset-x-0 bottom-[4.25rem] z-30 flex items-center gap-2 border-t border-base-300 bg-base-100 p-3 md:hidden">
      <button
        type="button"
        class="btn btn-primary min-h-11 flex-1"
        disabled={state.value === 'loading'}
        onClick={openNew}
      >
        新建任务
      </button>
      <button
        type="button"
        class="btn btn-soft min-h-11"
        disabled={state.value === 'loading'}
        onClick={() => void load()}
      >
        刷新
      </button>
    </div>
  )

  const empty = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body items-center gap-3 py-12 text-center">
        <p class="text-sm opacity-70">还没有定时任务。建一条，到点后端自己跑。</p>
        <button type="button" class="btn btn-primary min-h-11 md:min-h-0" onClick={openNew}>
          新建任务
        </button>
        <p class="max-w-prose text-left text-xs leading-5 opacity-60">
          定时任务只有这一个入口：命令行没有 schedule 那一族命令，组合（pipeline）那一种也还没有对外的
          读写接口，所以这一页建不了也改不了它。
        </p>
      </div>
    </div>
  )

  /** 频率那一块：四档各自只渲染自己那一样控件（切换即重建，正好是新控件的焦点态）。 */
  const cronFields = () => (
    <fieldset class="fieldset gap-2">
      <legend class="fieldset-legend">频率</legend>
      <select
        class="select w-full"
        aria-label="频率"
        value={cronDraft.value.preset}
        onChange={(e) => patchCron({ preset: (e.target as HTMLSelectElement).value as CronDraft['preset'] })}
      >
        {CRON_PRESETS.map((p) => (
          <option value={p}>{CRON_PRESET_LABEL[p]}</option>
        ))}
      </select>

      {cronDraft.value.preset === 'daily' || cronDraft.value.preset === 'weekly' ? (
        <label class="flex flex-col gap-1 text-sm">
          <span class="opacity-70">时刻</span>
          <input
            class="input w-full"
            type="time"
            aria-label="时刻"
            value={cronDraft.value.time}
            onInput={(e) => patchCron({ time: (e.target as HTMLInputElement).value })}
          />
        </label>
      ) : null}

      {cronDraft.value.preset === 'weekly' ? (
        <label class="flex flex-col gap-1 text-sm">
          <span class="opacity-70">周几</span>
          <select
            class="select w-full"
            aria-label="周几"
            value={cronDraft.value.weekday}
            onChange={(e) => patchCron({ weekday: (e.target as HTMLSelectElement).value })}
          >
            {WEEKDAY_VALUES.map((d) => (
              <option value={d}>{WEEKDAY_LABEL[d]}</option>
            ))}
          </select>
        </label>
      ) : null}

      {cronDraft.value.preset === 'hourly' ? (
        <label class="flex flex-col gap-1 text-sm">
          <span class="opacity-70">每小时的第几分钟</span>
          <input
            class="input w-full"
            type="number"
            min={0}
            max={59}
            aria-label="每小时的第几分钟"
            value={cronDraft.value.minute}
            onInput={(e) => patchCron({ minute: (e.target as HTMLInputElement).value })}
          />
        </label>
      ) : null}

      {cronDraft.value.preset === 'custom' ? (
        <label class="flex flex-col gap-1 text-sm">
          <span class="opacity-70">cron 表达式</span>
          <textarea
            class="textarea w-full font-mono"
            rows={2}
            spellcheck={false}
            aria-label="cron 表达式"
            placeholder="0 30 7 * * *"
            value={cronDraft.value.cron}
            onInput={(e) => patchCron({ cron: (e.target as HTMLTextAreaElement).value })}
          />
        </label>
      ) : null}

      {/* 送出的是哪一串要看得见：预设算出来的和用户手写的都摆出来，
          否则"每天 07:30"与库里那串之间没人能核对。 */}
      <p class="text-xs leading-5 opacity-60">
        这条表达式：
        <span class="break-words font-mono">{previewCron()}</span>
      </p>
      <p class="fieldset-label">
        合法性与下次执行时间由后端判（段数、日与周同时限定取"或"、周日写 0 还是 7 都在它那一份里）。
        写错或永远不触发会在保存那一下退回来说明。
      </p>
    </fieldset>
  )

  /** 预览那一串。`buildCron` 返回 null = 还没填完，此时显示破折号而不是半截表达式。 */
  const previewCron = () => buildCron(cronDraft.value) ?? '—'

  const editDialog = () => (
    <Dialog
      id={EDIT_ID}
      title={editingId.value ? '编辑定时任务' : '新建定时任务'}
      size="md"
      onClose={() => (formError.value = '')}
    >
      <label class="flex flex-col gap-1 text-sm">
        <span class="opacity-70">任务名</span>
        <input
          class="input w-full"
          aria-label="任务名"
          value={nameDraft.value}
          onInput={(e) => (nameDraft.value = (e.target as HTMLInputElement).value)}
        />
      </label>

      {pipelineDraft.value ? (
        <p class="text-sm leading-6 break-words">{pipelineNote(pipelineDraft.value.id ?? '')}</p>
      ) : (
        <fieldset class="fieldset">
          <legend class="fieldset-legend">执行</legend>
          <select
            class="select w-full"
            aria-label="执行的能力"
            value={codeDraft.value}
            onChange={(e) => (codeDraft.value = (e.target as HTMLSelectElement).value)}
          >
            {caps.value.map((c) => (
              <option value={c.code}>
                {c.title}
                {c.hidden ? '（不在任务中心，只给定时用）' : ''}
              </option>
            ))}
          </select>
          <p class="fieldset-label">
            隐藏的那些也在列里：定时任务按 code 找能力，不看"任务中心给不给选"那一位。
          </p>
        </fieldset>
      )}

      {cronFields()}

      <fieldset class="fieldset gap-2">
        <legend class="fieldset-legend">跑哪些账号</legend>
        {accounts.value.length === 0 ? (
          <p class="text-sm opacity-70">库里还没有账号：先去账号页登录一个。</p>
        ) : (
          accounts.value.map((a) => (
            <label class="flex min-h-11 cursor-pointer items-center gap-2 text-sm">
              <input
                type="checkbox"
                class="checkbox checkbox-sm checkbox-primary"
                checked={accountsDraft.value.includes(a.id)}
                aria-label={`账号 ${a.displayName}`}
                onChange={(e) =>
                  toggleAccount(a.id, (e.target as HTMLInputElement).checked)
                }
              />
              <span class="break-words">{a.displayName}</span>
              {a.enabled ? null : <span class="badge badge-soft">已停用</span>}
            </label>
          ))
        )}
        <p class="fieldset-label">
          一格都没勾 = 到点跑全部启用账号。勾了的会盖掉下面 JSON 里的 accounts 键。
        </p>
      </fieldset>

      <fieldset class="fieldset gap-2">
        <legend class="fieldset-legend">参数（JSON）</legend>
        <textarea
          class="textarea w-full font-mono"
          rows={5}
          spellcheck={false}
          aria-label="参数 JSON"
          value={bodyDraft.value}
          onInput={(e) => (bodyDraft.value = (e.target as HTMLTextAreaElement).value)}
        />
        <p class="fieldset-label break-words">
          能填的键：{keysHint()}。留 {'{}'} 即用各项自己的默认值。
        </p>
      </fieldset>

      <label class="flex min-h-11 cursor-pointer items-center gap-2">
        <input
          type="checkbox"
          class="toggle toggle-primary"
          checked={enabledDraft.value}
          aria-label="这条任务启用"
          onChange={(e) => (enabledDraft.value = (e.target as HTMLInputElement).checked)}
        />
        <span class="text-sm">{enabledDraft.value ? '保存后立即参与调度' : '先存着，不参与调度'}</span>
      </label>

      {formError.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{formError.value}</span>
        </div>
      ) : null}

      <div class="modal-action">
        <button type="button" class="btn btn-soft" onClick={() => closeDialog(EDIT_ID)}>
          取消
        </button>
        <button
          type="button"
          class="btn btn-primary min-h-11 md:min-h-0"
          disabled={busyId.value !== ''}
          onClick={() => void save()}
        >
          {busyId.value !== '' ? '保存中…' : '确认保存'}
        </button>
      </div>
    </Dialog>
  )

  /**
   * 删除确认窗。为什么不走 `ui/dialog` 的 `ConfirmDialog`：文件顶部那条（按钮高度，
   * 以及"不改共享件"的既有口径）。
   *
   * 两个动态量（影响面文案、进行中的禁用）都放在**子节点与属性位置**，于是
   * `<dialog>` 本体不被重建 —— `openDialog()` 找到的元素与显示出来的元素是同一个，
   * 换一行删除时只有 `<p>` 里的文字与按钮状态变。
   */
  const deleteDialog = () => (
    <Dialog id={DELETE_ID} title="删除这条定时任务？" size="sm">
      <p class="break-words text-sm leading-6">{deleteHint.value}</p>
      <div class="modal-action">
        <button
          type="button"
          class="btn btn-soft min-h-11 md:min-h-0"
          onClick={() => closeDialog(DELETE_ID)}
        >
          取消
        </button>
        <button
          type="button"
          class="btn btn-error min-h-11 md:min-h-0"
          disabled={deleting.value}
          onClick={() => void confirmDelete()}
        >
          {deleting.value ? '删除中…' : '确认删除'}
        </button>
      </div>
    </Dialog>
  )

  /** 声明键名那一串。**从能力表现取**，不在这里另写一份能力参数表。 */
  const keysHint = () => {
    const declared = codeDraft.value ? paramKeys(codeDraft.value) : []
    if (declared.length > 0) return declared.join('、')
    return '以这一项能力的声明为准（后端到点会按声明校键名，多一个就报不认识的参数）'
  }

  return (
    <div class="flex flex-col gap-4">
      {header()}
      {schedulerAlert()}
      {noticeLine()}
      {actionErrorAlert()}
      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-spinner loading-lg text-primary" />
        </div>
      ) : null}
      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}
      {state.value === 'ready' && rows.value.length === 0 ? empty() : null}
      {state.value === 'ready' && rows.value.length > 0 ? (
        <>
          {desktopTable()}
          {mobileCards()}
          {mobileBar()}
        </>
      ) : null}

      {editDialog()}
      {deleteDialog()}
    </div>
  )
}
