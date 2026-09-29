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

import { onMounted, ref } from 'lite'
import { listCapabilities, request, type TaskMeta } from '../api'
import { EmptyState } from '../ui/empty-state'
import { closeDialog, ConfirmDialog, openDialog } from '../ui/dialog'
import { showError } from '../ui/error-dialog'
import { pushToast } from '../ui/toast'
import { fmtEpochMs } from '../ui/time'

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
  accounts: string[]
  accountMode: 'sequential' | 'parallel'
  stopOnError: boolean
  steps: PipelineStep[]
  enabled: boolean
  lastRunMs: number | null
}

/** 提交给 `PUT /api/pipelines/{id}` 的体。刻意不含 `lastRunMs`：那是服务端维护的。 */
interface PipelineInput {
  name: string
  accounts: string[]
  accountMode: 'sequential' | 'parallel'
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
  /** 账号用文本编辑，逗号/空格/顿号分隔；空 = 全部账号。 */
  accountsText: string
  accountMode: 'sequential' | 'parallel'
  stopOnError: boolean
  enabled: boolean
}

const DELETE_DIALOG_ID = 'delete-pipeline'

// ── 网络 ────────────────────────────────────────────────────────────────

const listPipelines = () => request<Pipeline[]>('/api/pipelines')

const savePipeline = (id: string, input: PipelineInput) =>
  request<Pipeline>(`/api/pipelines/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })

const removePipeline = (id: string) =>
  request<{ ok: boolean }>(`/api/pipelines/${encodeURIComponent(id)}`, { method: 'DELETE' })

const runPipeline = (id: string) =>
  request<{ pipeline: string; started: boolean }>(
    `/api/pipelines/${encodeURIComponent(id)}/run`,
    { method: 'POST' },
  )

// ── 查表（类名一律字面量，绝不拼接）────────────────────────────────────────

const ACCOUNT_MODE_LABEL: Record<Pipeline['accountMode'], string> = {
  sequential: '串行',
  parallel: '并行',
}

const ENABLED_LABEL: Record<'on' | 'off', string> = { on: '已启用', off: '已停用' }

const ENABLED_BADGE: Record<'on' | 'off', string> = {
  on: 'badge-soft badge-success',
  off: 'badge-soft',
}

const STOP_LABEL: Record<'stop' | 'continue', string> = {
  stop: '失败时中止整条链',
  continue: '失败也继续后续',
}

const STOP_BADGE: Record<'stop' | 'continue', string> = {
  stop: 'badge-soft badge-warning',
  continue: 'badge-soft',
}

const onOff = (v: boolean): 'on' | 'off' => (v ? 'on' : 'off')
const stopOrContinue = (v: boolean): 'stop' | 'continue' => (v ? 'stop' : 'continue')

/** 账号文本 → 数组。分隔符接受中英文逗号、顿号与空白。 */
const parseAccounts = (raw: string): string[] =>
  raw
    .split(/[\s,，、]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0)

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

  onMounted(() => void load())

  // ── 草稿 ──────────────────────────────────────────────────────────────

  const emptyStep = (): StepDraft => ({
    uid: uidSeq++,
    task: '',
    body: {},
    continueOnError: false,
  })

  const startNew = () => {
    submitError.value = ''
    steps.value = [emptyStep()]
    draft.value = {
      id: `p-${Date.now().toString(36)}-${idSeq++}`,
      isNew: true,
      name: '',
      accountsText: '',
      // 后端当前只接受 sequential；parallel 在下拉里显式禁用并说明原因
      accountMode: 'sequential',
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
      accountsText: p.accounts.join('、'),
      accountMode: p.accountMode,
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
    accounts: parseAccounts(d.accountsText),
    accountMode: d.accountMode,
    stopOnError: d.stopOnError,
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
      await runPipeline(p.id)
      pushToast('success', '已在后台开始，可在运行历史查看进度')
    } catch (e) {
      showError(`无法运行「${p.name}」`, e instanceof Error ? e.message : '运行请求失败')
    } finally {
      runningId.value = ''
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
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">任务编排</h1>
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
        <button
          type="button"
          class="btn btn-sm btn-soft min-h-11 md:min-h-0"
          disabled={state.value === 'loading'}
          onClick={() => void load()}
        >
          刷新
        </button>
      </div>
    </div>
  )

  /** 一条编排的操作区（表格与卡片共用）。 */
  const actions = (p: Pipeline) => (
    <>
      <button
        type="button"
        class="btn btn-soft btn-sm min-h-11 md:min-h-0"
        disabled={runningId.value === p.id}
        onClick={() => void run(p)}
      >
        {runningId.value === p.id ? '提交中…' : '运行'}
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
                <th>账号</th>
                <th>失败时</th>
                <th>状态</th>
                <th>上次执行</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {pipelines.value.map((p) => (
                <tr key={p.id}>
                  <td class="break-words font-medium">{p.name}</td>
                  <td>{p.steps.length} 步</td>
                  <td>{ACCOUNT_MODE_LABEL[p.accountMode]}</td>
                  <td>
                    <span class={`badge ${STOP_BADGE[stopOrContinue(p.stopOnError)]}`}>
                      {STOP_LABEL[stopOrContinue(p.stopOnError)]}
                    </span>
                  </td>
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
                <p class="text-xs opacity-70">
                  {p.steps.length} 步 · {ACCOUNT_MODE_LABEL[p.accountMode]}
                </p>
                <p class="break-words text-xs opacity-70">
                  失败时 {STOP_LABEL[stopOrContinue(p.stopOnError)]}
                </p>
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
              <select
                class="select select-sm w-full"
                value={s.task}
                onChange={(e) => {
                  s.task = (e.target as HTMLSelectElement).value
                }}
              >
                <option value="">请选择任务</option>
                {taskChoices(s.task).map((c) => (
                  <option key={c.value} value={c.value}>
                    {c.label}
                  </option>
                ))}
              </select>
            </fieldset>

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

        <fieldset class="fieldset">
          <legend class="fieldset-legend">账号</legend>
          <input
            class="input w-full"
            placeholder="留空 = 全部账号"
            value={d.accountsText}
            onInput={(e) => {
              d.accountsText = (e.target as HTMLInputElement).value
            }}
          />
          <p class="text-xs opacity-60">
            多个账号用逗号或空格分隔；留空表示对全部账号依次执行。
          </p>
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">账号执行方式</legend>
          {/*
            用**逐项独立**的原生 radio，而不是把整组包成一个分段控件：
            这里只有「并行」那一项该是禁用的，若整组共用一个禁用态，
            会连带把「串行」也染灰，让人误以为串行也不能选。
            所以禁用只作用在「并行」那一项上。
          */}
          <div class="flex flex-col gap-2">
            <label class="flex items-center gap-3">
              <input
                type="radio"
                class="radio radio-sm"
                name="pipeline-account-mode"
                value="sequential"
                checked={d.accountMode === 'sequential'}
                onChange={() => {
                  d.accountMode = 'sequential'
                }}
              />
              <span class="text-sm">串行</span>
            </label>
            {/*
              并行**不隐藏**：隐藏会让人以为"没这个功能"或"漏做了"。
              这里显式禁用 + 把原因写在选项上。真被提交（例如从别处存进来的数据）
              也会由后端 400 拦下，错误原文照抛。
            */}
            <label class="flex items-start gap-3">
              <input
                type="radio"
                class="radio radio-sm mt-0.5"
                name="pipeline-account-mode"
                value="parallel"
                disabled={true}
                checked={d.accountMode === 'parallel'}
                onChange={() => {
                  d.accountMode = 'parallel'
                }}
              />
              <span class="text-sm opacity-60">
                并行
                <span class="block text-xs opacity-80">
                  上游限流，暂不支持并行
                </span>
              </span>
            </label>
          </div>
          <p class="text-xs opacity-60">
            后端当前只接受串行执行；若提交并行，会原样显示后端返回的 400 报错。
          </p>
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

        {/*
          两层失败语义单独占一块：这是本页最核心的概念，
          混进步骤列表就等于埋掉了。
        */}
        <div class="rounded-box bg-base-200 p-3">
          <h3 class="text-sm font-medium">失败语义（两层）</h3>
          <p class="mt-1 text-xs leading-5 opacity-70">
            步骤级优先：某一步勾了「这一步失败也继续」，即使编排级开着「失败时中止整条链」，
            这一步失败也不会中断后续步骤。
          </p>

          <label class="mt-3 flex items-start gap-3">
            <input
              type="checkbox"
              class="toggle toggle-sm mt-0.5"
              checked={d.stopOnError}
              onChange={(e) => {
                d.stopOnError = (e.target as HTMLInputElement).checked
              }}
            />
            <span>
              <span class="block text-sm font-medium">失败时中止整条链</span>
              <span class="block text-xs opacity-60">
                编排级：任何一步失败后，不再开始后续步骤。
              </span>
            </span>
          </label>

          <p class="mt-2 text-xs opacity-60">
            每个步骤还有自己的「这一步失败也继续」开关 —— 那一层更具体，压过这里。
          </p>
        </div>

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
