/**
 * 任务编排：把多个任务串成一条链，一次触发、按顺序跑完。
 *
 * # 这页真正要讲清楚的一件事：**两层失败语义**
 *
 * 后端有两层开关（见 `mcloud_core::Pipeline::should_abort_after`）：
 *
 * - 编排级 `stopOnError`：「失败时中止整条链」——整个流程还要不要往下走。
 * - 步骤级 `continueOnError`：「这一步失败也继续」——这一步的失败算不算数。
 *
 * 判定顺序是**步骤级优先**：步骤说继续就继续，否则才看编排级。
 * 两层缺一不可 —— 只有编排级就没法跳过非关键步骤，只有步骤级就没法在关键
 * 步骤失败时止损。这是本页最容易被忽略、也最影响结果形状的设置，
 * 所以它单独占一块，而不是混在步骤列表里。
 *
 * # 移动端形态（规范 §7.2）
 *
 * 列表桌面走表格、窄屏切卡片流（同历史页）；编辑器单列，步骤块纵向排列。
 * 步骤的增删改序是**结构性**改动：`steps` 单独是一个 ref，每次换成**新数组**
 * （浅响应的 `ref` 只认引用换没换，原地 push/splice 不会让任何人重跑）。
 *
 * # 网络层
 *
 * 直接用 `api.ts` 导出的 `request<T>` —— 本页**不再**自己复刻一份。
 * 复刻的代价是两处语义迟早分叉（尤其是"后端错误原文透出"这条）。
 */

import { onMounted, ref, onUnmounted } from 'lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  listAccounts,
  listCapabilities,
  PIPELINE_TASK,
  request,
  saveSchedule,
  type AccountSummary,
  type TaskMeta,
} from '../api'
import { EmptyState } from '../ui/empty-state'
// ⚠ 参数控件与**任务配置页**共用一份（`ui/param-field.tsx`）—— 两处各写一份迟早分叉。
import { ParamField } from '../ui/param-field'
import { closeDialog, ConfirmDialog, Dialog, openDialog } from '../ui/dialog'
import { showError } from '../ui/error-dialog'
import { pushToast } from '../ui/toast'
import { fmtEpochMs } from '../ui/time'
import { Select } from '../ui/select'

type LoadState = 'loading' | 'error' | 'ready'

/** 与后端 `PipelineStep` 的 wire 形态一致（camelCase）。 */
interface PipelineStep {
  task: string
  body: unknown
  continueOnError: boolean
}

/** 与后端 `Pipeline` 的 wire 形态一致。`lastRunMs` 为 null 表示从未执行。 */
interface Pipeline {
  id: string
  name: string
  /** ⚠ **已废弃**：账号不属于编排（用户口径 2026-10-02：编排不绑账号，
   * 账号由**触发方**给 —— 手动运行 / 定时排期各带自己那份）。后端老数据里还有，
   * 所以类型保留为可选 ✓；界面既不显示也不提交 ✓。 */
  accounts?: string[]
  /** ⚠ **已废弃**，同上：执行方式也跟着账号走触发方。 */
  accountMode?: 'sequential' | 'parallel'
  stopOnError: boolean
  steps: PipelineStep[]
  enabled: boolean
  lastRunMs: number | null
}

/** 提交给 `PUT /api/pipelines/{id}` 的体。刻意不含 `lastRunMs`：那是服务端维护的。 */
interface PipelineInput {
  name: string
  /** ⚠ **已废弃**：账号不属于编排（用户口径 2026-10-02：编排不绑账号，
   * 账号由**触发方**给 —— 手动运行 / 定时排期各带自己那份）。后端老数据里还有，
   * 所以类型保留为可选 ✓；界面既不显示也不提交 ✓。 */
  accounts?: string[]
  /** ⚠ **已废弃**，同上：执行方式也跟着账号走触发方。 */
  accountMode?: 'sequential' | 'parallel'
  stopOnError: boolean
  steps: { task: string; body: unknown; continueOnError: boolean }[]
  enabled: boolean
}

/** 编辑期的一份草稿。`uid` 只在前端用，给步骤列表当稳定的 key。 */
interface StepDraft {
  uid: number
  task: string
  /** 原样带回后端 —— 本页不编辑参数，但**不能**把已有参数丢掉。 */
  body: unknown
  continueOnError: boolean
}

/**
 * 草稿的**非列表**部分。
 *
 * ⚠ 步骤数组**不在这里**（见下面 `steps` 那条 ref）：本框架的 `ref` 是浅响应，
 * 把数组塞进草稿对象里再 `draft.value.steps.push(…)`，谁都不会醒 ——
 * 那正是"点了添加步骤、界面不动"的成因。数组要响应，就得让它**自己**是那个 `.value`。
 */
interface Draft {
  id: string
  isNew: boolean
  name: string
  stopOnError: boolean
  enabled: boolean
}

const DELETE_DIALOG_ID = 'delete-pipeline'
/** 「运行」前先选账号（用户口径 2026-10-02：**账号属于触发方**，编排不绑账号）。 */
const RUN_DIALOG_ID = 'run-pipeline'
/** 给这条编排排一条定时（`task: "pipeline"` + `body.pipelineId`）。 */
const SCHED_DIALOG_ID = 'pipeline-schedule'

// ── 网络 ────────────────────────────────────────────────────────────────

const listPipelines = () => request<Pipeline[]>('/api/pipelines')

const savePipeline = (id: string, input: PipelineInput) =>
  request<Pipeline>(`/api/pipelines/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })

const removePipeline = (id: string) =>
  request<{ ok: boolean }>(`/api/pipelines/${encodeURIComponent(id)}`, { method: 'DELETE' })

/**
 * 跑一条编排。
 *
 * ⚠ 账号**不取编排里存的那份**（已废弃字段 ✗）—— 由**触发方**给 ✓：
 * 手动运行在这里给 ✓，定时运行在排期的 `accounts` 里给 ✓。
 * 空数组 = 当前用户全部已启用账号 ✓（与后端「留空 = 全部」同义 ✓）。
 */
const runPipeline = (id: string, accounts: string[]) =>
  request<{ pipeline: string; started: boolean }>(
    `/api/pipelines/${encodeURIComponent(id)}/run`,
    // ⚠ 与同页 `savePipeline` 一致：body 要自己序列化（`request` 收的是 `BodyInit`）。
    { method: 'POST', body: JSON.stringify({ accounts }) },
  )

// ── 查表（类名一律字面量，绝不拼接）────────────────────────────────────────


const ENABLED_LABEL: Record<'on' | 'off', string> = { on: '已启用', off: '已停用' }

const ENABLED_BADGE: Record<'on' | 'off', string> = {
  on: 'badge-soft badge-success',
  off: 'badge-soft',
}



const onOff = (v: boolean): 'on' | 'off' => (v ? 'on' : 'off')


export const PipelinesPage = () => {
  const state = ref<LoadState>('loading')
  /** 加载失败（整页替换）。**留在页内** —— 它要配「重试」。 */
  const errorMessage = ref('')
  /**
   * 保存/校验失败（表单内联）。**保持内联**：错误要贴着待改的步骤，
   * 且它出现在草稿表单里，用户正在往下改。页面里独立动作
   * （运行、删除）的失败走 `showError`。
   */
  const submitError = ref('')

  const pipelines = ref<Pipeline[]>([])
  const tasks = ref<TaskMeta[]>([])
  const draft = ref<Draft | null>(null)
  /** 删除确认框里要显示的那条。 */
  const deleteTarget = ref<Pipeline | null>(null)

  const saving = ref(false)
  const deleting = ref(false)
  /** 正在提交运行请求的那条（POST 立刻返回，所以这个状态只闪一下）。 */
  const runningId = ref('')

  /**
   * 草稿的步骤数组。
   *
   * 它**单独成一个 ref**，而不是挂在 `draft.value.steps` 上 —— 浅响应的 `ref`
   * 只在"换掉 `.value` 那一个引用"时通知，所以增/删/改序一律写成整数组替换
   * （见 `addStep` / `removeStep` / `moveStep`）。
   *
   * 原来这里是一个 `stepsRev` 计数器 + 原地 `push`/`splice`：计数器**没有**被渲染读过，
   * 于是点「添加步骤」界面不动（整块步骤列表的 `createFor` 只订阅了数组本身）。
   * 换成数组 ref 之后不需要计数器，步数、空态提示、每行的禁用态也都跟着活了。
   */
  const steps = ref<StepDraft[]>([])

  let uidSeq = 1
  let idSeq = 1

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      // 任务清单必须一起拿：步骤的任务下拉是**从能力表生成**的，不硬编码任务名
      const [ps, caps] = await Promise.all([listPipelines(), listCapabilities()])
      pipelines.value = ps
      tasks.value = caps.tasks.filter((t) => !t.hidden)
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => {
    setPageRefresh({ run: () => void load() })
    void load()
  })
  onUnmounted(() => setPageRefresh(null))

  // ── 草稿 ──────────────────────────────────────────────────────────────

  const emptyStep = (): StepDraft => ({
    uid: uidSeq++,
    task: '',
    body: {},
    // 用户口径 2026-10-02：「默认设置为失败继续」——新加的步骤默认就继续，
    // 想让某一步"失败即止"再单独关掉它。
    continueOnError: true,
  })

  const startNew = () => {
    submitError.value = ''
    steps.value = [emptyStep()]
    draft.value = {
      id: `p-${Date.now().toString(36)}-${idSeq++}`,
      isNew: true,
      name: '',
      // 后端当前只接受 sequential；parallel 在下拉里显式禁用并说明原因
      stopOnError: true,
      enabled: true,
    }
  }

  const startEdit = (p: Pipeline) => {
    submitError.value = ''
    // 参数（body）原样带回，本页不编辑它但绝不能丢
    steps.value = p.steps.map((s) => ({
      uid: uidSeq++,
      task: s.task,
      body: s.body,
      continueOnError: s.continueOnError,
    }))
    draft.value = {
      id: p.id,
      isNew: false,
      name: p.name,
      stopOnError: p.stopOnError,
      enabled: p.enabled,
    }
  }

  const cancelEdit = () => {
    draft.value = null
    steps.value = []
    submitError.value = ''
  }

  // ⚠⚠ 下面三个都是**整数组替换**，不是 push/splice：浅响应的 ref 只认引用换了没有，
  // 原地改数组不触发任何人 —— 那会让步骤列表停在旧内容上（本文件以前就是这样，见 `steps` 的注释）。

  const addStep = () => {
    if (!draft.value) return
    steps.value = [...steps.value, emptyStep()]
  }

  const removeStep = (index: number) => {
    if (!draft.value) return
    steps.value = steps.value.filter((_, i) => i !== index)
  }

  const moveStep = (index: number, delta: number) => {
    if (!draft.value) return
    const target = index + delta
    if (target < 0 || target >= steps.value.length) return
    const next = [...steps.value]
    const [moved] = next.splice(index, 1)
    next.splice(target, 0, moved)
    steps.value = next
  }

  /** 下拉选项：能力表 + （若当前值已不在表里）把原值兜底显示出来，不静默改数据。 */
  const taskChoices = (current: string): { value: string; label: string }[] => {
    const list = tasks.value.map((t) => ({ value: t.name, label: t.title }))
    if (current && !list.some((c) => c.value === current)) {
      list.push({ value: current, label: `${current}（后端未注册）` })
    }
    return list
  }

  /**
   * 客户端预检。**返回第一条**失败原因，带上具体步号 ——
   * "有步骤没填"这种话等于没说是哪一步。
   * 服务端才是权威校验，它的话一律原样透传。
   */
  const validate = (d: Draft): string => {
    if (!d.name.trim()) return '请填写编排名称'
    if (steps.value.length === 0) return '至少要有一个步骤'
    for (let i = 0; i < steps.value.length; i++) {
      if (!steps.value[i].task) return `第 ${i + 1} 步还没有选择任务`
    }
    return ''
  }

  const toInput = (d: Draft): PipelineInput => ({
    name: d.name.trim(),
    // ⚠ 编排级「失败时中止整条链」已按用户口径**砍掉**（2026-10-02）：
    // 只保留步骤级开关。这里恒为 false，免得老编排的 true 又把"两层"带回来。
    stopOnError: false,
    steps: steps.value.map((s) => ({
      task: s.task,
      body: s.body ?? {},
      continueOnError: s.continueOnError,
    })),
    enabled: d.enabled,
  })

  const save = async () => {
    const d = draft.value
    if (!d) return
    submitError.value = ''
    const invalid = validate(d)
    if (invalid) {
      submitError.value = invalid
      return
    }

    saving.value = true
    const isNew = d.isNew
    try {
      await savePipeline(d.id, toInput(d))
      draft.value = null
      pushToast('success', isNew ? '编排已创建' : '编排已保存')
      await load()
    } catch (e) {
      // 服务端原文（含"第 N 步的任务不存在：x"这类预检结论）
      submitError.value = e instanceof Error ? e.message : '保存失败'
    } finally {
      saving.value = false
    }
  }

  // ── 运行 / 删除 ────────────────────────────────────────────────────────

  /**
   * 触发运行。
   *
   * 后端是 **202 语义**：立刻返回，链在后台跑。所以这里**不显示会一直转的
   * 进度**，只做一次"已提交"的瞬时确认，并指路运行历史。
   */
  const run = async (p: Pipeline) => {
    runningId.value = p.id
    try {
      await runPipeline(p.id, runAccounts.value)
      pushToast('success', '已在后台开始，可在运行历史查看进度')
    } catch (e) {
      showError(`无法运行「${p.name}」`, e instanceof Error ? e.message : '运行请求失败')
    } finally {
      runningId.value = ''
    }
  }

  // ── 「运行」先选账号 ──────────────────────────────────────────────────
  /** 本次运行要用的账号（弹窗里选的；空 = 全部 ✓）。 */
  const runAccounts = ref<string[]>([])
  /** 可选账号清单（打开弹窗时拉一次）。 */
  const runChoices = ref<AccountSummary[]>([])
  /** 正在为哪条编排选账号。 */
  const runTarget = ref<Pipeline | null>(null)

  const openRun = async (p: Pipeline) => {
    runTarget.value = p
    runAccounts.value = []
    if (runChoices.value.length === 0) {
      try {
        runChoices.value = await listAccounts()
      } catch (e) {
        showError('读不到账号列表', e instanceof Error ? e.message : '加载失败')
        return
      }
    }
    // ⚠ **默认全选**（用户口径 2026-10-02）✗ 不做"留空 = 全部"这种隐式约定：
    // 界面上看到的勾选状态就是**会发生什么** ✓，不需要靠一句提示去解释空表 ✓。
    runAccounts.value = runChoices.value.map((a) => a.id)
    openDialog(RUN_DIALOG_ID)
  }

  /** 勾/取消一个账号 —— ⚠ 换**新数组**：`ref` 是浅响应，原地 push/splice 没人会醒 ✗。 */
  const toggleRunAccount = (id: string) => {
    runAccounts.value = runAccounts.value.includes(id)
      ? runAccounts.value.filter((x) => x !== id)
      : [...runAccounts.value, id]
  }

  const confirmRun = async () => {
    const p = runTarget.value
    if (!p) return
    closeDialog(RUN_DIALOG_ID)
    await run(p)
  }

  // ── 「定时」给这条编排排一条定时 ──────────────────────────────────────
  const schedTarget = ref<Pipeline | null>(null)
  const schedAccounts = ref<string[]>([])
  const schedExpr = ref('0 8 * * *')
  const schedTz = ref(
    // 浏览器时区作为缺省 ✓（用户就在这个时区里点按钮）。
    Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Shanghai',
  )
  const schedBusy = ref(false)

  const openSched = async (p: Pipeline) => {
    schedTarget.value = p
    schedAccounts.value = []
    schedBusy.value = false
    if (runChoices.value.length === 0) {
      try {
        runChoices.value = await listAccounts()
      } catch (e) {
        showError('读不到账号列表', e instanceof Error ? e.message : '加载失败')
        return
      }
    }
    // 同运行弹窗：**默认全选** ✓（不靠"空 = 全部"）。
    schedAccounts.value = runChoices.value.map((a) => a.id)
    openDialog(SCHED_DIALOG_ID)
  }

  const toggleSchedAccount = (id: string) => {
    schedAccounts.value = schedAccounts.value.includes(id)
      ? schedAccounts.value.filter((x) => x !== id)
      : [...schedAccounts.value, id]
  }

  const confirmSched = async () => {
    const p = schedTarget.value
    if (!p) return
    schedBusy.value = true
    try {
      await saveSchedule(`sch-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`, {
        name: `编排 ${p.name}`,
        task: PIPELINE_TASK,
        accounts: schedAccounts.value,
        expr: schedExpr.value.trim(),
        // ⚠ 不能给 `undefined`（类型是 `string` ✓）；空了就回落上海 ✓ —— 发一个空串
        // 会让后端的时区解析失败 ✗，而"时区没填"根本不该是一次失败。
        timezone: schedTz.value.trim() || 'Asia/Shanghai',
        body: { pipelineId: p.id },
        enabled: true,
      })
      closeDialog(SCHED_DIALOG_ID)
      pushToast('success', '已加定时 —— 到「定时」页能看到下次触发时间')
    } catch (e) {
      // 后端的建时校验（cron 写法、pipelineId 是否存在/属于你）在这里原样透出 ✓
      showError('加定时失败', e instanceof Error ? e.message : '保存失败')
    } finally {
      schedBusy.value = false
    }
  }

  const askDelete = (p: Pipeline) => {
    deleteTarget.value = p
    openDialog(DELETE_DIALOG_ID)
  }

  const confirmDelete = async () => {
    const p = deleteTarget.value
    if (!p) return
    deleting.value = true
    try {
      await removePipeline(p.id)
      closeDialog(DELETE_DIALOG_ID)
      pushToast('success', `已删除编排「${p.name}」`)
      await load()
    } catch (e) {
      // 被定时引用时后端拒绝删除，并列出引用它的定时任务名 —— 原样抛出。
      // 走弹窗：那份"被谁引用"的名单正是用户接下来要去改的东西，必须读到。
      closeDialog(DELETE_DIALOG_ID)
      showError(
        `无法删除编排「${p.name}」`,
        e instanceof Error ? e.message : '删除失败',
      )
    } finally {
      deleting.value = false
    }
  }

  // ── 渲染 ──────────────────────────────────────────────────────────────

  const header = () => (
    <div class="flex flex-wrap items-center justify-end gap-2">
      {/* 标题由外壳那个 sticky navbar 显示（`app.tsx` 读 `navItem(id).title`）——
          页面里**不要再写一遍**：2026-10-03 之前八页各写了一次，于是每页顶部
          都是同一句话挨着出现两遍（连 `text-base font-medium` 都一模一样）✗。
          这里只放动作（添加/新建…）——「刷新」已搬到外壳 navbar 的图标上
          （见 `ui/page-refresh.ts`）。 */}
      <div class="flex flex-wrap items-center gap-2">
        {/*
          全页**只有一个**实心 btn-primary：
          - 没有编排时，primary 是空态里的「新建编排」；
          - 有编排且没在编辑时，primary 是这里的「新建编排」；
          - 正在编辑时，primary 是表单里的「保存」。
          三种情况互斥，所以任何时候都只有一个。
        */}
        {!draft.value && pipelines.value.length > 0 ? (
          <button
            type="button"
            class="btn btn-primary btn-sm min-h-11 md:min-h-0"
            disabled={tasks.value.length === 0}
            onClick={startNew}
          >
            新建编排
          </button>
        ) : null}
      </div>
    </div>
  )

  /** 某个任务的参数表（从能力表取；选空/选了个不存在的 ⇒ 空数组 ✗ 不报错）。 */
  const specsFor = (task: string) => tasks.value.find((t) => t.name === task)?.params ?? []

  /**
   * 某个任务的**默认参数**（后端给的 `default`，JSON 原样）。
   *
   * ⚠ 换任务时用它**整份替换** `body`（而不是留着旧的键 ✗）：不同任务的参数名不同，
   * 留着上一个任务的键，就会变成"界面上看不见、却会随编排发给后端"的垃圾 ✗。
   * 同时这也让表单**一进来就显示真正会用的值** ✓ —— 比留空更准确（空 ≠ 用默认值，
   * 空是"没给"，两者在有些任务上并不等价）。
   */
  const specDefaults = (task: string) => {
    const out: Record<string, unknown> = {}
    for (const p of specsFor(task)) {
      if (p.default !== undefined) out[p.name] = p.default
    }
    return out
  }

  /** 一条编排的操作区（表格与卡片共用）。 */
  const actions = (p: Pipeline) => (
    <>
      <button
        type="button"
        class="btn btn-soft btn-sm min-h-11 md:min-h-0"
        disabled={runningId.value === p.id}
        onClick={() => void openRun(p)}
      >
        {runningId.value === p.id ? '提交中…' : '运行'}
      </button>
      <button
        type="button"
        class="btn btn-soft btn-sm min-h-11 md:min-h-0"
        disabled={draft.value !== null}
        onClick={() => void openSched(p)}
      >
        定时
      </button>
      <button
        type="button"
        class="btn btn-ghost btn-sm min-h-11 md:min-h-0"
        disabled={draft.value !== null}
        onClick={() => startEdit(p)}
      >
        编辑
      </button>
      {/* 危险动作保持**平涂**（无渐变）；真正的确认在弹窗里那一下实心 btn-error */}
      <button
        type="button"
        class="btn btn-soft btn-error btn-sm min-h-11 md:min-h-0"
        onClick={() => askDelete(p)}
      >
        删除
      </button>
    </>
  )

  const pipelineList = () => {
    if (pipelines.value.length === 0) {
      return (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="还没有编排。"
              hint="新建一条，把「签到 → 领AI豆」串起来。"
              action={
                <button
                  type="button"
                  class="btn btn-primary min-h-11 md:min-h-0"
                  disabled={tasks.value.length === 0}
                  onClick={startNew}
                >
                  新建编排
                </button>
              }
            />
          </div>
        </div>
      )
    }

    return (
      <>
        {/* 桌面表格 ≥768px */}
        <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
          <table class="table table-sm">
            <thead>
              <tr>
                <th>名称</th>
                <th>步骤</th>
                <th>状态</th>
                <th>上次执行</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {pipelines.value.map((p) => (
                <tr key={p.id}>
                  <td class="break-words font-medium">{p.name}</td>
                  {/* 只报步数：两层失败语义已按用户口径砍掉（2026-10-02），
                      失败与否现在只由**每一步**自己的开关决定 ✓。 */}
                  <td class="whitespace-nowrap">{p.steps.length} 步</td>
                  <td>
                    <span class={`badge ${ENABLED_BADGE[onOff(p.enabled)]}`}>
                      {ENABLED_LABEL[onOff(p.enabled)]}
                    </span>
                  </td>
                  <td>{fmtEpochMs(p.lastRunMs)}</td>
                  <td>
                    <div class="flex flex-wrap gap-2">{actions(p)}</div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {/* 移动卡片流 <768px —— 每个值都带标签 */}
        <div class="grid gap-3 md:hidden">
          {pipelines.value.map((p) => (
            <div key={p.id} class="card bg-base-100 shadow-sm">
              <div class="card-body p-3">
                <div class="flex items-center justify-between gap-2">
                  <span class="break-words font-medium">{p.name}</span>
                  <span class={`badge ${ENABLED_BADGE[onOff(p.enabled)]}`}>
                    {ENABLED_LABEL[onOff(p.enabled)]}
                  </span>
                </div>
                <p class="break-words text-xs opacity-70">{p.steps.length} 步</p>
                <p class="text-xs opacity-60">上次执行 {fmtEpochMs(p.lastRunMs)}</p>
                <div class="card-actions justify-end">{actions(p)}</div>
              </div>
            </div>
          ))}
        </div>
      </>
    )
  }

  /**
   * 步骤列表本体。
   *
   * 读的是 `steps.value`（组件级 ref），不是草稿对象上的数组 —— 这样"增/删/改序"
   * 换掉引用时，这一块才会重跑。行内的 `第 N 步` 与两个移动按钮的 `disabled`
   * 都是位置的函数，编译器因此把这一列判成"位置敏感"（`createFor` 的 `positional`）：
   * 重排时那几行重建，顺序与步号一定对得上。
   */
  const stepEditor = () => (
    <div class="flex flex-col gap-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h3 class="text-sm font-medium">步骤（按这个顺序执行）</h3>
        <span class="text-xs opacity-60">共 {steps.value.length} 步</span>
      </div>

      {steps.value.length === 0 ? (
        <p class="text-xs opacity-60">还没有步骤。点下面的「添加步骤」加第 1 步。</p>
      ) : null}

      <div class="flex flex-col gap-3">
        {steps.value.map((s, i) => (
          <div key={s.uid} class="rounded-box bg-base-200 p-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <span class="badge badge-soft">第 {i + 1} 步</span>
              <div class="flex flex-wrap gap-1">
                <button
                  type="button"
                  class="btn btn-ghost btn-sm min-h-11 md:min-h-0"
                  disabled={i === 0}
                  onClick={() => moveStep(i, -1)}
                >
                  上移
                </button>
                <button
                  type="button"
                  class="btn btn-ghost btn-sm min-h-11 md:min-h-0"
                  disabled={i === steps.value.length - 1}
                  onClick={() => moveStep(i, 1)}
                >
                  下移
                </button>
                <button
                  type="button"
                  class="btn btn-soft btn-error btn-sm min-h-11 md:min-h-0"
                  onClick={() => removeStep(i)}
                >
                  移除
                </button>
              </div>
            </div>

            <fieldset class="fieldset">
              <legend class="fieldset-legend">任务</legend>
              <Select
                id={`step-task-${i}`}
                size="sm"
                class="w-full"
                ariaLabel="任务"
                placeholder="请选择任务"
                value={s.task}
                options={[
                  { value: '', label: '请选择任务' },
                  ...taskChoices(s.task).map((c) => ({ value: c.value, label: c.label })),
                ]}
                onChange={(task) => {
                  // ⚠ 整份换掉（含 `body`）—— 见 `specDefaults` 上那段说明。
                  steps.value = steps.value.map((x, k) =>
                    k === i ? { ...x, task, body: specDefaults(task) } : x,
                  )
                }}
              />
            </fieldset>

            {/*
              ⚠ 参数**按任务生成**（`TaskMeta.params` ← 后端 `ParamSpec`）；前端不维护
              任何任务知识 ✓。从前这一块是空的 —— 步骤的 `body` 只"原样带回、不能编辑"✗，
              于是编排里的步骤永远用**任务自己的默认配置**，想改一个参数只能去任务页 ✗。

              ⚠⚠ 改值必须**整体换掉那个步骤对象**（以及 `body`）：`steps` 是 `ref`，
              浅响应只认"引用换没换"✓ —— 原地 `s.body[x] = v` 谁都不会醒 ✗
              （同一页的步骤增删也是靠换新数组 ✓，见文件头）。
            */}
            {specsFor(s.task).length > 0 ? (
              <div class="mt-2 flex flex-col gap-2">
                {specsFor(s.task).map((spec) => (
                  <ParamField
                    key={spec.name}
                    spec={spec}
                    get={() =>
                      (s.body as Record<string, unknown> | null | undefined)?.[spec.name]
                    }
                    onInput={(v) => {
                      /*
                        ⚠⚠ **原地改 `body`，不写 `steps.value`**（2026-10-03 review §2.2）。

                        两件事叠在一起才是"敲一个字整张步骤卡重建、焦点全丢"：
                        ① 传**值** ⇒ 参数块那一层重建（已由 `get` thunk 修掉）；
                        ② 写 `steps.value` ⇒ 列表槽重跑 ⇒ `createFor` 按**对象身份**判行
                           ⇒ 这里每次 `{ ...x, body }` 都是**新对象** ⇒ 整行（含任务下拉、
                           全部参数控件）拆掉重造 ✗。

                        改完之后不需要重渲染：`<input>` 里的字是用户自己敲进去的，
                        `body` 改了就行（保存路径读的就是这些**同一个**对象）。
                        切任务那条路径**照旧**换新对象 —— 那时 specs 真的变了，重建是对的 ✓。
                      */
                      const step = steps.value[i]
                      if (!step) return
                      step.body = {
                        ...((step.body as Record<string, unknown> | null) ?? {}),
                        [spec.name]: v,
                      }
                    }}
                  />
                ))}
              </div>
            ) : null}

            <label class="flex items-center gap-3">
              <input
                type="checkbox"
                class="toggle toggle-sm"
                checked={s.continueOnError}
                onChange={(e) => {
                  s.continueOnError = (e.target as HTMLInputElement).checked
                }}
              />
              <span class="text-sm">这一步失败也继续</span>
            </label>
          </div>
        ))}
      </div>

      <button
        type="button"
        class="btn btn-soft btn-sm min-h-11 md:min-h-0"
        onClick={addStep}
      >
        添加步骤
      </button>
    </div>
  )

  const form = (d: Draft) => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <h2 class="card-title text-base">{d.isNew ? '新建编排' : '编辑编排'}</h2>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">名称</legend>
          <input
            class="input w-full"
            placeholder="例如 早班：签到 → 领AI豆"
            value={d.name}
            // onInput 而不是 onChange：Vue 的 onChange 是原生 change，只在失焦时触发
            onInput={(e) => {
              d.name = (e.target as HTMLInputElement).value
            }}
          />
        </fieldset>


        <label class="flex items-center gap-3">
          <input
            type="checkbox"
            class="toggle toggle-sm"
            checked={d.enabled}
            onChange={(e) => {
              d.enabled = (e.target as HTMLInputElement).checked
            }}
          />
          <span class="text-sm">启用这条编排</span>
        </label>


        {stepEditor()}

        {submitError.value ? (
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">{submitError.value}</span>
          </div>
        ) : null}

        <div class="card-actions justify-end">
          <button
            type="button"
            class="btn btn-soft min-h-11 md:min-h-0"
            disabled={saving.value}
            onClick={cancelEdit}
          >
            取消
          </button>
          <button
            type="button"
            class="btn btn-primary min-h-11 md:min-h-0"
            disabled={saving.value}
            onClick={() => void save()}
          >
            {saving.value ? '保存中…' : '保存'}
          </button>
        </div>
      </div>
    </div>
  )

  return (
    <div class="flex flex-col gap-4">
      {header()}

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
          <button
            type="button"
            class="btn btn-sm btn-soft min-h-11 md:min-h-0"
            onClick={() => void load()}
          >
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (
        <>
          {tasks.value.length === 0 ? (
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <span class="break-words">
                后端没有注册任何任务，暂时无法新建编排。请检查后端是否正常启动，或稍后重试。
              </span>
            </div>
          ) : null}

          {draft.value ? form(draft.value) : pipelineList()}
        </>
      ) : null}

      {/*
        删除确认：内容按 `deleteTarget` 反应式更新，所以弹窗本体只挂一次。
        lite 的 props 是**编译期 getter**（`<X label={t.value}/>` 编成
        `get "label"() { return … }`）⇒ 父级那个 ref 一变，子组件再读 `props.label`
        拿到的就是新值，**不需要**（也没有"重建组件实例"这条路可走）。
      */}
      <Dialog id={RUN_DIALOG_ID} title="运行编排" size="md">
        <p class="text-sm">
          给「<strong>{runTarget.value?.name ?? ''}</strong>」选账号（默认全选）。
        </p>
        <div class="flex justify-end gap-2">
          <button
            type="button"
            class="btn btn-soft btn-xs"
            onClick={() => (runAccounts.value = runChoices.value.map((a) => a.id))}
          >
            全选
          </button>
          <button
            type="button"
            class="btn btn-soft btn-xs"
            onClick={() => (runAccounts.value = [])}
          >
            清空
          </button>
        </div>
        <p class="text-xs opacity-60">已选 {runAccounts.value.length} / {runChoices.value.length}</p>
        <div class="flex flex-col gap-1">
          {runChoices.value.map((a) => (
            <label key={a.id} class="flex min-h-11 cursor-pointer items-center gap-3">
              <input
                type="checkbox"
                class="checkbox checkbox-sm"
                checked={runAccounts.value.includes(a.id)}
                onChange={() => toggleRunAccount(a.id)}
              />
              {/* ⚠ 显示 `label`（掩码手机号）✗ 不要用 `id` —— 它是哈希，认不出是哪个号。 */}
              <span class="text-sm">{a.label}</span>
            </label>
          ))}
          {runChoices.value.length === 0 ? (
            <p class="text-xs opacity-60">没有可用账号。</p>
          ) : null}
        </div>
        <div class="modal-action">
          <button type="button" class="btn btn-ghost" onClick={() => closeDialog(RUN_DIALOG_ID)}>
            取消
          </button>
          <button
            type="button"
            class="btn btn-primary"
            disabled={runAccounts.value.length === 0}
            onClick={() => void confirmRun()}
          >
            开始运行
          </button>
        </div>
      </Dialog>

      <Dialog id={SCHED_DIALOG_ID} title="给编排加定时" size="md">
        <p class="text-sm">
          让「<strong>{schedTarget.value?.name ?? ''}</strong>」按下面的时间**自动跑一遍** ✓ ——
          用的就是下面选的账号（默认全选）。
        </p>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">cron 表达式</legend>
          <input
            class="input w-full font-mono"
            value={schedExpr.value}
            onInput={(e) => (schedExpr.value = (e.target as HTMLInputElement).value)}
          />
          <p class="text-xs opacity-60">5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周）。</p>
          <div class="flex flex-wrap gap-2">
            {/*
              ⚠ 用**对象数组 + 具名参数**，不要用 `[[…]].map(([a, b]) => …)`：
              lite 编译器靠"返回值就是那个 JSX 元素"来认 `.map` 的 `key`，
              解构参数会让它认不出 ✗（实测报错："这里的 key 什么都不做" — 本页 945 行）。
            */}
            {[
              { expr: '0 8 * * *', label: '每天 08:00' },
              { expr: '0 0 * * *', label: '每天 00:00' },
              { expr: '30 7 * * 1-5', label: '工作日 07:30' },
            ].map((preset) => (
              <button
                key={preset.expr}
                type="button"
                class="btn btn-soft btn-sm"
                onClick={() => (schedExpr.value = preset.expr)}
              >
                {preset.label}
              </button>
            ))}
          </div>
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">时区</legend>
          <input
            class="input w-full"
            value={schedTz.value}
            onInput={(e) => (schedTz.value = (e.target as HTMLInputElement).value)}
          />
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">账号</legend>
          <div class="flex justify-end gap-2">
            <button
              type="button"
              class="btn btn-soft btn-xs"
              onClick={() => (schedAccounts.value = runChoices.value.map((a) => a.id))}
            >
              全选
            </button>
            <button
              type="button"
              class="btn btn-soft btn-xs"
              onClick={() => (schedAccounts.value = [])}
            >
              清空
            </button>
          </div>
          <p class="text-xs opacity-60">
            已选 {schedAccounts.value.length} / {runChoices.value.length}
          </p>
          <div class="flex flex-col gap-1">
            {runChoices.value.map((a) => (
              <label key={a.id} class="flex min-h-11 cursor-pointer items-center gap-3">
                <input
                  type="checkbox"
                  class="checkbox checkbox-sm"
                  checked={schedAccounts.value.includes(a.id)}
                  onChange={() => toggleSchedAccount(a.id)}
                />
                <span class="text-sm">{a.label}</span>
              </label>
            ))}
            {runChoices.value.length === 0 ? (
              <p class="text-xs opacity-60">没有可用账号。</p>
            ) : null}
          </div>
        </fieldset>

        <div class="modal-action">
          <button type="button" class="btn btn-ghost" onClick={() => closeDialog(SCHED_DIALOG_ID)}>
            取消
          </button>
          <button
            type="button"
            class="btn btn-primary"
            disabled={schedBusy.value || schedAccounts.value.length === 0}
            onClick={() => void confirmSched()}
          >
            {schedBusy.value ? '保存中…' : '加定时'}
          </button>
        </div>
      </Dialog>

      <ConfirmDialog
        id={DELETE_DIALOG_ID}
        title="删除编排"
        impact={
          <span>
            将删除编排「{deleteTarget.value?.name ?? ''}」。删除后不可恢复；
            若它正被定时任务引用，后端会拒绝删除并列出引用它的定时任务名。
          </span>
        }
        confirmLabel="删除编排"
        onConfirm={() => void confirmDelete()}
        busy={deleting.value}
      />
    </div>
  )
}
