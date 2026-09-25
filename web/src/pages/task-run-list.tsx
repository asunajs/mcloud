/**
 * 「预设运行列表」这一坨（任务中心的编排区）。
 *
 * # 为什么单独一个文件
 *
 * 任务中心那一页要同时装下三件事：能力卡片墙、有序清单的编辑器、预设的存/跑/删。
 * 后两件的**状态**（草稿、选中行、脏否、确认目标）与**渲染**是一体的，
 * 拆到 `ui/` 反而要把一堆 ref 当参数传来传去；所以这里放"状态 + 它的渲染"，
 * 纯规则（草稿 ↔ 出网体、重排、预检、徽标）留在 `ui/pipeline-draft.ts`（§9 约定 2）。
 * 页面侧 `tasks-page.tsx` 只留能力卡片墙与参数表单。
 *
 * # 三个不报错的坑，都在这文件里避过
 *
 * ① 不用子组件传 props：Vapor 下组件 setup 只跑一次，`props.x` 是快照
 * （`docs/webui-code-guide.md` §9.2④）。这里全部是**函数返回 JSX**，
 * 状态靠闭包里的 ref 读，压根没有 props 这一层。
 * ② 类名一律整条字面量（§9.1①）—— 连 `badge-${status}` 这种都没写。
 * ③ 清单里的一步**不给** `accounts` / `accountMode` 两个控件：执行时后端会用
 * 组合级列表覆盖它们（`internal/server/pipeline_run.go:293-300`），
 * 留在表单里就是"填了永远不生效"的那一格。账号只在**清单级**选一次。
 */

import { computed, ref, type Ref } from 'vue'
import {
  ApiError,
  deletePipeline,
  listPipelines,
  putPipeline,
  runPipeline,
  type Account,
  type CapabilityRow,
  type PipelineRow,
} from '../api'
import { ConfirmDialog, closeDialog, openDialog } from '../ui/dialog'
import { IconAlert } from '../ui/icons'
import {
  BUILT_IN_PIPELINE_ID,
  CONTINUE_OPTIONS,
  MAX_PIPELINE_NAME,
  continueLabel,
  emptyRunList,
  findStep,
  insertStep,
  makeStep,
  moveStep,
  newStepUid,
  nextPipelineId,
  payloadFromRunList,
  pipelineRunText,
  pipelineStatusBadge,
  removeStep as dropStep,
  runListFromPipeline,
  runListIssues,
  stepSummary,
  type DraftStep,
  type RunList,
} from '../ui/pipeline-draft'
import { fmtTime } from '../ui/time'

/** 删除确认与运行确认两个弹窗的 id（`ui/dialog.tsx` 靠 id 找 DOM）。 */
const DELETE_ID = 'pipeline-delete'
const RUN_ID = 'pipeline-run'

/** 清单编辑器要用的外部状态：能力表、账号表，以及"跑完要不要刷历史"。 */
export interface RunListDeps {
  capabilities: Ref<CapabilityRow[]>
  accounts: Ref<Account[]>
  /** 每次受理后要回读一次历史（组合本身没有进度视图，见 `pending-decisions.md`）。 */
  refresh: () => Promise<void>
}

/**
 * 编排区的全部状态与动作。
 *
 * ⚠ 起点是**空清单**，不是"内置那一版"。空清单配一句"从下面的卡片开始挑"，
 * 用户知道自己正在造东西；默认把内置那 14 步摆上来，则每次都像在改系统预设，
 * 而内置那一版**存不回原 id**（后端 400 掉 `all`），改完必然要另存 —— 起点就误导。
 */
export const useRunList = (deps: RunListDeps) => {
  const pipelines = ref<PipelineRow[]>([])
  const list = ref<RunList>(emptyRunList())
  /** 正在编辑哪一步（清单行的指针）；`null` = 清单空着。 */
  const selectedUid = ref<string | null>(null)
  /** 保存 / 运行那一下的忙位（按钮文案与 disabled 共用）。 */
  const busy = ref(false)
  /** 受理成功的一句话。 */
  const notice = ref('')
  /** 这一带的失败（与"整页加载失败"分开：那些只该在页面顶部换掉整页）。 */
  const error = ref('')
  /** 确认弹窗要动的那一份预设（删除 / 运行共用一个目标位）。 */
  const confirmTarget = ref<PipelineRow | null>(null)
  /** 确认弹窗是哪一种动作 —— 同一份 `impact` 文案要跟着换标题与按钮字。 */
  const confirmKind = ref<'delete' | 'run'>('run')

  const capsByCode = computed<Map<string, CapabilityRow>>(
    () => new Map(deps.capabilities.value.map((c) => [c.code, c])),
  )

  const issues = computed<string[]>(() => runListIssues(list.value, capsByCode.value))
  const canSave = computed(() => issues.value.length === 0 && !busy.value)

  const selectedStep = computed<DraftStep | null>(() =>
    findStep(list.value.steps, selectedUid.value),
  )

  /** 选中那一步在清单里的序号（1 起；`0` = 没选中）。表单标题用它，不靠"猜第几个同名行"。 */
  const selectedOrder = computed(
    () => list.value.steps.findIndex((s) => s.uid === selectedUid.value) + 1,
  )

  /**
   * 这张能力卡片对应第几步（`null` = 还没进清单）。
   *
   * 同名多步时取**最后一个**：那正是 [useCapability] 点卡片会跳过去的那一行，
   * 卡片上写的序号与点下去选中的行必须一致。
   */
  const orderOf = (code: string): number | null => {
    const steps = list.value.steps
    for (let i = steps.length - 1; i >= 0; i -= 1) {
      if (steps[i].code === code) return i + 1
    }
    return null
  }

  const selectedCap = computed<CapabilityRow | null>(() =>
    selectedStep.value === null ? null : (capsByCode.value.get(selectedStep.value.code) ?? null),
  )

  /**
   * 当前草稿对应的那一行（用来区分"改一份已有的"与"新建的"）。
   * 按 id 找：库里同名不同 id 是合法的，名字不是主键。
   */
  const savedRow = computed<PipelineRow | null>(
    () => pipelines.value.find((p) => p.id === list.value.id) ?? null,
  )

  /** 标题栏那一句：是内置、是已有、还是没存过的草稿。 */
  const listHeadline = computed(() => {
    if (list.value.steps.length === 0) return '还没有挑任务'
    if (list.value.id === '') return '未保存的清单'
    if (savedRow.value === null) return '未保存的清单'
    if (savedRow.value.builtIn) return `内置「${savedRow.value.name}」的副本`
    return `正在编辑「${savedRow.value.name}」`
  })

  /** 账号留空 = 全部启用账号（与能力声明那句 help 同义，不是"0 个目标"）。 */
  const targetCount = computed(() =>
    list.value.accountIds.length === 0 ? deps.accounts.value.length : list.value.accountIds.length,
  )

  const reloadPipelines = async () => {
    try {
      const rows = await listPipelines()
      pipelines.value = rows.pipelines
    } catch (e) {
      // ⚠ 不换成整页错误态：能力墙与参数表单跟这一族没关系，预设读不出来
      // 不该让用户连"单跑一条"都做不了。原话进这一带的小字。
      error.value = e instanceof ApiError ? e.message : '读不到预设列表'
    }
  }

  // ── 清单的编辑 ────────────────────────────────────────────────────────────

  const selectStep = (uid: string) => {
    selectedUid.value = uid
    error.value = ''
  }

  /**
   * 点一张能力卡片：已经在清单里就**跳到那一行**，不在就**加到末尾并选中**。
   *
   * ⚠ 不做"未加进清单也能先编辑"的临时预览态：那样同一个表单会有两个出处
   * （预览的那一份与清单里的那一份），而用户以为改的是清单。
   * 重复出现同一条能力是合法的（换参数跑两次），但要靠 [addAnother] 明说，
   * 免得点两下卡片就多出一行。
   */
  const useCapability = (cap: CapabilityRow) => {
    const existing = [...list.value.steps].reverse().find((s) => s.code === cap.code)
    if (existing) {
      selectStep(existing.uid)
      return
    }
    const step = makeStep(cap)
    list.value.steps = insertStep(list.value.steps, step)
    selectStep(step.uid)
  }

  /** 把当前选中的这一步**再复制一份**放在它后面（故意跑两次才用得到）。 */
  const addAnother = () => {
    const cur = selectedStep.value
    if (cur === null) return
    const at = list.value.steps.findIndex((s) => s.uid === cur.uid)
    const copy: DraftStep = { ...cur, uid: newStepUid(), body: { ...cur.body } }
    list.value.steps = insertStep(list.value.steps, copy, at + 1)
    selectStep(copy.uid)
  }

  const removeStep = (uid: string) => {
    const at = list.value.steps.findIndex((s) => s.uid === uid)
    if (at < 0) return
    list.value.steps = dropStep(list.value.steps, uid)
    if (selectedUid.value !== uid) return
    // 选中的行被删 ⇒ 落到邻居上，否则表单会对着一个不存在的步骤，
    // 表现是"还在显示参数、改哪儿都不进清单"。
    const next = list.value.steps[Math.min(at, list.value.steps.length - 1)] ?? null
    selectedUid.value = next?.uid ?? null
  }

  const move = (index: number, delta: number) => {
    list.value.steps = moveStep(list.value.steps, index, delta)
  }

  const setContinue = (uid: string, v: boolean | null) => {
    const step = findStep(list.value.steps, uid)
    if (step) step.continueOnError = v
  }

  const toggleAccount = (id: string, on: boolean) => {
    const cur = list.value.accountIds
    list.value.accountIds = on ? [...cur, id] : cur.filter((x) => x !== id)
  }

  const accountPicked = (id: string) => list.value.accountIds.includes(id)

  /** 这一步现在编辑中的参数对象（表单**直接读写这一格**，没有中间副本）。 */
  const editBody = (): Record<string, unknown> => {
    const step = selectedStep.value
    if (step === null) return {}
    return step.body
  }

  // ── 预设：读、另起、存、跑、删 ─────────────────────────────────────────────

  const newRunList = () => {
    list.value = emptyRunList()
    selectedUid.value = null
    error.value = ''
    notice.value = ''
  }

  const loadPreset = (row: PipelineRow) => {
    list.value = runListFromPipeline(row, capsByCode.value)
    selectedUid.value = list.value.steps[0]?.uid ?? null
    error.value = ''
    notice.value = ''
  }

  /**
   * 存这一份清单。**内置那一版走另存**：后端 400 掉 `PUT /api/pipelines/all`
   * （允许建同名行就等于让那枚"一键全跑"按钮偷偷跑别人的清单）。
   */
  const saveList = async (): Promise<string | null> => {
    if (issues.value.length > 0) {
      error.value = issues.value.join('；')
      return null
    }
    busy.value = true
    error.value = ''
    const targetId =
      list.value.builtIn || list.value.id === ''
        ? nextPipelineId(pipelines.value.map((p) => p.id))
        : list.value.id
    try {
      const saved = await putPipeline(targetId, payloadFromRunList(list.value, capsByCode.value))
      list.value.id = saved.pipeline.id
      list.value.builtIn = false
      await reloadPipelines()
      notice.value = `已保存预设「${saved.pipeline.name}」（${saved.pipeline.steps.length} 步）。`
      return saved.pipeline.id
    } catch (e) {
      // 后端在**入库之前**跑预检，所以这句话通常指名是哪一步 —— 原样透出，别翻译。
      error.value = e instanceof ApiError ? e.message : '保存失败'
      return null
    } finally {
      busy.value = false
    }
  }

  /**
   * 跑清单。**没存过就先存**：`POST /api/pipelines/{id}/run` 只认 id，
   * 后端没有"把请求体里这份临时清单跑一遍"这一档。
   *
   * ⚠ 不静默这么做会有另一件事：用户点了"运行"，界面上什么都没发生（400 "任务组合不存在"）。
   * 先存再跑是这一枚按钮唯一的诚实形状，界面上也写明"未保存，点运行会先存成预设"。
   */
  const runList = async () => {
    busy.value = true
    error.value = ''
    try {
      let id = list.value.id
      if (id === '' || list.value.builtIn || savedRow.value === null) {
        const savedId = await saveList()
        if (savedId === null) return
        id = savedId
      }
      const accepted = await runPipeline(id)
      notice.value = pipelineRunText(accepted.name, accepted.steps)
      await Promise.all([reloadPipelines(), deps.refresh()])
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : '运行失败'
    } finally {
      busy.value = false
    }
  }

  const askRun = (row: PipelineRow) => {
    confirmTarget.value = row
    confirmKind.value = 'run'
    openDialog(RUN_ID)
  }

  const doRun = async () => {
    const row = confirmTarget.value
    if (row === null) return
    busy.value = true
    error.value = ''
    try {
      const accepted = await runPipeline(row.id)
      closeDialog(RUN_ID)
      notice.value = pipelineRunText(accepted.name, accepted.steps)
      await Promise.all([reloadPipelines(), deps.refresh()])
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : '运行失败'
      closeDialog(RUN_ID)
    } finally {
      busy.value = false
    }
  }

  const askDelete = (row: PipelineRow) => {
    confirmTarget.value = row
    confirmKind.value = 'delete'
    openDialog(DELETE_ID)
  }

  const doDelete = async () => {
    const row = confirmTarget.value
    if (row === null) return
    busy.value = true
    error.value = ''
    try {
      await deletePipeline(row.id)
      closeDialog(DELETE_ID)
      // 删的正是当前在编辑的那一份 ⇒ 草稿回到空白：留着它，下一次保存会**重建**
      // 刚删掉的那一行（id 还在草稿里），那与用户点"删除"的意思正相反。
      if (list.value.id === row.id) newRunList()
      notice.value = `已删除预设「${row.name}」。`
      await reloadPipelines()
    } catch (e) {
      // 被定时任务引用时后端 400 并**指名那几条任务**（不级联删），原话照登。
      error.value = e instanceof ApiError ? e.message : '删除失败'
      closeDialog(DELETE_ID)
    } finally {
      busy.value = false
    }
  }

  return {
    pipelines,
    list,
    selectedUid,
    selectedStep,
    selectedOrder,
    orderOf,
    selectedCap,
    capsByCode,
    issues,
    canSave,
    savedRow,
    listHeadline,
    targetCount,
    busy,
    notice,
    error,
    confirmTarget,
    confirmKind,
    accounts: deps.accounts,
    reloadPipelines,
    selectStep,
    useCapability,
    addAnother,
    removeStep,
    move,
    setContinue,
    toggleAccount,
    accountPicked,
    editBody,
    newRunList,
    loadPreset,
    saveList,
    runList,
    askRun,
    doRun,
    askDelete,
    doDelete,
  }
}

export type RunListModel = ReturnType<typeof useRunList>

// ─────────────────────────────────────────────────────────────────────────────
// 渲染：预设条
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 已存的预设（含内置那一版）排成一条卡片带。
 *
 * 窄屏横滑（`overflow-x-auto` + `whitespace-nowrap` 那一族）而不是换行堆叠：
 * 这一带的用途是"挑一份来跑"，纵向堆会把下面的能力墙推到两屏开外。
 */
export const presetBar = (m: RunListModel) => (
  <div class="card bg-base-100 shadow-sm">
    <div class="card-body gap-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h2 class="card-title text-base">预设运行列表</h2>
        <button
          type="button"
          class="btn btn-sm btn-soft min-h-11 md:min-h-0"
          onClick={() => m.newRunList()}
        >
          新建一份
        </button>
      </div>

      {m.pipelines.value.length === 0 ? (
        /* 空态给下一步（§5.10），不写"暂无数据"。 */
        <p class="break-words text-xs opacity-70">
          还没有存过预设。先在下面的卡片里挑几项，再回这一栏点「保存为预设」。
        </p>
      ) : (
        <div class="flex gap-3 overflow-x-auto pb-1">
          {m.pipelines.value.map((p) => (
            <div
              class={
                p.id === m.list.value.id
                  ? 'w-64 shrink-0 rounded-box border-2 border-primary bg-base-200 p-3'
                  : 'w-64 shrink-0 rounded-box bg-base-200 p-3'
              }
            >
              <div class="flex items-start justify-between gap-2">
                <span class="break-words text-sm font-medium">{p.name}</span>
                {p.builtIn ? <span class="badge badge-soft badge-info">内置</span> : null}
              </div>
              <p class="mt-1 break-words text-xs opacity-60">
                {p.steps.length} 步 ·{' '}
                {p.accountIds.length === 0 ? '全部账号' : `${p.accountIds.length} 个账号`} ·{' '}
                {p.stopOnError ? '失败就停' : '失败继续'}
              </p>
              <p class="mt-1 break-words text-xs opacity-60">上次 {fmtTime(p.lastRunAt)}</p>
              <div class="mt-2 flex flex-wrap items-center gap-2">
                <span class={pipelineStatusBadge(p.lastStatus).cls}>
                  {pipelineStatusBadge(p.lastStatus).label}
                </span>
                {p.lastError ? (
                  <span class="break-words text-xs opacity-60">{p.lastError}</span>
                ) : null}
              </div>
              <div class="mt-3 flex flex-wrap gap-2">
                <button
                  type="button"
                  class="btn btn-xs min-h-11 md:min-h-0"
                  onClick={() => m.loadPreset(p)}
                >
                  {p.builtIn ? '载入为副本' : '编辑'}
                </button>
                <button
                  type="button"
                  class="btn btn-xs btn-primary min-h-11 md:min-h-0"
                  disabled={m.busy.value}
                  onClick={() => m.askRun(p)}
                >
                  运行
                </button>
                {p.builtIn || p.id === BUILT_IN_PIPELINE_ID ? null : (
                  <button
                    type="button"
                    class="btn btn-xs btn-soft min-h-11 md:min-h-0"
                    onClick={() => m.askDelete(p)}
                  >
                    删除
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  </div>
)

// ─────────────────────────────────────────────────────────────────────────────
// 渲染：有序清单
// ─────────────────────────────────────────────────────────────────────────────

/** 一行步骤。序号在左，标题与参数摘要在中，上下移与删除在右。 */
const stepRow = (m: RunListModel, step: DraftStep, index: number, total: number) => {
  const cap = m.capsByCode.value.get(step.code)
  const on = step.uid === m.selectedUid.value
  return (
    <li
      class={
        on
          ? 'flex flex-wrap items-start gap-2 rounded-box border-2 border-primary bg-base-200 p-3'
          : 'flex flex-wrap items-start gap-2 rounded-box bg-base-200 p-3'
      }
    >
      <span class="badge badge-neutral">{index + 1}</span>
      <button
        type="button"
        class="min-w-0 flex-1 cursor-pointer text-left"
        onClick={() => m.selectStep(step.uid)}
      >
        <span class="break-words text-sm font-medium">{step.title}</span>
        <code class="ml-2 text-xs opacity-50">{step.code}</code>
        <span class="block break-words text-xs opacity-70">{stepSummary(step, cap)}</span>
        <span class="block break-words text-xs opacity-50">
          {continueLabel(step.continueOnError, m.list.value.stopOnError)}
        </span>
      </button>
      {/* 上下移：`join` 把两枚并成一组，比散着放更看不出"这是一对"。
          到顶/到底那一下**禁用**而不是隐藏 —— 隐藏会让这一列宽度在滚动时跳动。 */}
      <div class="join">
        <button
          type="button"
          class="btn btn-xs btn-ghost join-item min-h-11 md:min-h-0"
          disabled={index === 0}
          aria-label={`上移第 ${index + 1} 步`}
          onClick={() => m.move(index, -1)}
        >
          ↑
        </button>
        <button
          type="button"
          class="btn btn-xs btn-ghost join-item min-h-11 md:min-h-0"
          disabled={index === total - 1}
          aria-label={`下移第 ${index + 1} 步`}
          onClick={() => m.move(index, 1)}
        >
          ↓
        </button>
      </div>
      <button
        type="button"
        class="btn btn-xs btn-soft min-h-11 md:min-h-0"
        onClick={() => m.removeStep(step.uid)}
      >
        移出
      </button>
    </li>
  )
}

/**
 * 清单本体：名字、失败策略、账号、有序步骤，与那三枚动作。
 *
 * 参数表单**不在**这里 —— 它在页面下方的「执行参数」那一栏（`tasks-page.tsx` 的 `formCard`），
 * 那一栏编辑的就是这里的**选中行**。
 */
export const runListCard = (m: RunListModel) => (
  <div class="card bg-base-100 shadow-sm">
    <div class="card-body gap-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h2 class="card-title text-base">运行清单</h2>
        <span class="text-xs opacity-60">{m.listHeadline.value}</span>
      </div>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">
          预设名称 <span class="text-error">*</span>
        </legend>
        <input
          class="input input-bordered w-full"
          maxlength={MAX_PIPELINE_NAME}
          value={m.list.value.name}
          onInput={(e) => {
            m.list.value.name = (e.target as HTMLInputElement).value
          }}
        />
        <p class="break-words text-xs opacity-60">名字只影响这一份预设；执行顺序看下面那串序号。</p>
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">说明（可选）</legend>
        <input
          class="input input-bordered w-full"
          value={m.list.value.description}
          onInput={(e) => {
            m.list.value.description = (e.target as HTMLInputElement).value
          }}
        />
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">目标账号</legend>
        <div class="flex flex-col gap-2">
          {m.accounts.value.length === 0 ? (
            <p class="break-words text-sm opacity-70">
              还没有云盘账号。留空代表「全部账号」，但现在一个都没有 ——
              先去「账号」页添加，再回来跑任务。
            </p>
          ) : (
            m.accounts.value.map((a) => (
              <label class="flex min-h-11 cursor-pointer items-center gap-2 md:min-h-0">
                <input
                  type="checkbox"
                  class="checkbox checkbox-sm"
                  checked={m.accountPicked(a.id)}
                  onChange={(e) => m.toggleAccount(a.id, (e.target as HTMLInputElement).checked)}
                />
                <span class="break-words text-sm">{a.displayName}</span>
              </label>
            ))
          )}
        </div>
        <p class="break-words text-xs opacity-60">
          一个都不勾 = 全部启用账号。这一档<b>覆盖</b>每一步里的账号选择：组合跑起来时，
          整份清单用同一批账号，所以每步的表单里不给账号那一格。
        </p>
      </fieldset>

      <fieldset class="fieldset">
        <label class="flex min-h-11 cursor-pointer items-center gap-2 md:min-h-0">
          <input
            type="checkbox"
            class="toggle toggle-sm"
            checked={m.list.value.stopOnError}
            onChange={(e) => {
              m.list.value.stopOnError = (e.target as HTMLInputElement).checked
            }}
          />
          <span class="text-sm">某一步失败就停下（默认继续跑后面的）</span>
        </label>
        <p class="break-words text-xs opacity-60">
          这一步单独设的地方在下面每一行的「失败后」那一栏；留在「跟随组合」就取上面这个开关。
          组合只支持串行执行 —— 一步跑完才走下一步。
        </p>
      </fieldset>

      {m.list.value.wasParallel ? (
        // 不是报错，是**改了用户的存量数据**的预告：组合级 `account_mode` 只能是串行
        // （`sequentialOnly`），而不写这一句的话，界面下方那句"这一步没跑"看着像执行坏了。
        <p class="break-words text-xs opacity-60">
          这一份原先存的是<b>并行</b>执行；任务组合只支持串行，保存后会变成串行。
        </p>
      ) : null}

      {m.list.value.steps.length === 0 ? (
        <div class="flex flex-col gap-1 rounded-box bg-base-200 px-4 py-6 text-center">
          <p class="text-sm opacity-70">清单是空的。</p>
          <p class="max-w-prose break-words text-xs opacity-60">
            在下面的能力卡片墙里点一下想要的项，它就会按点到的先后排进这一串； 用 ↑ ↓
            调顺序，点一行可以改它的参数。
          </p>
        </div>
      ) : (
        <>
          <ul class="flex flex-col gap-2">
            {m.list.value.steps.map((s, i) => stepRow(m, s, i, m.list.value.steps.length))}
          </ul>
          {m.selectedStep.value === null ? null : (
            <div class="flex flex-wrap items-end gap-2">
              <fieldset class="fieldset min-w-40">
                <legend class="fieldset-legend">这一步失败后</legend>
                <select
                  class="select select-sm w-full"
                  value={JSON.stringify(m.selectedStep.value.continueOnError)}
                  onChange={(e) => {
                    const raw = (e.target as HTMLSelectElement).value
                    m.setContinue(
                      m.selectedStep.value?.uid ?? '',
                      raw === 'null' ? null : raw === 'true',
                    )
                  }}
                >
                  {CONTINUE_OPTIONS.map((o) => (
                    <option value={JSON.stringify(o.value)}>{o.label}</option>
                  ))}
                </select>
              </fieldset>
              <button
                type="button"
                class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                onClick={() => m.addAnother()}
              >
                再加一次这一步
              </button>
            </div>
          )}
        </>
      )}

      {m.issues.value.length > 0 ? (
        <div class="flex flex-col gap-1 text-sm" role="alert">
          {m.issues.value.map((s) => (
            <span class="break-words text-error">{s}</span>
          ))}
        </div>
      ) : null}

      {m.error.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <IconAlert class="size-4" />
          <span class="break-words">{m.error.value}</span>
        </div>
      ) : null}

      {m.notice.value ? (
        <div class="alert alert-soft alert-success text-sm" role="status">
          <span class="break-words">{m.notice.value}</span>
        </div>
      ) : null}

      <div class="card-actions justify-end">
        <button
          type="button"
          class="btn btn-soft min-h-11 md:min-h-0"
          disabled={!m.canSave.value}
          onClick={() => void m.saveList()}
        >
          保存为预设
        </button>
        <button
          type="button"
          class="btn btn-primary min-h-11 md:min-h-0"
          disabled={!m.canSave.value}
          onClick={() => void m.runList()}
        >
          {m.busy.value ? '执行中…' : `运行这份清单（${m.targetCount.value} 个账号）`}
        </button>
      </div>
    </div>
  </div>
)

/**
 * 删除与运行那两次确认。
 *
 * `confirmTarget` 在 **JSX 子节点位置**读（`m.confirmTarget.value ? …`），不在函数开头
 * 抄成 `const row = m.confirmTarget.value` —— 后者会被调用点那个 `setNodes` 收走依赖，
 * 是 `schedules-page.tsx` 抬头那条纪律（"读 ref 只能发生在子节点与属性位置"）。
 *
 * ⚠ 但不要把这条读成"子节点位置读不到会显示成旧值"：本轮实机核过一次，点删除后弹窗里
 * 就是那一份预设的名字，点运行后就是那一条清单的步数 —— 子节点位置的读取自带 effect。
 * （"弹窗会停在第一次那一行"这一族说法在 `schedules-page.tsx` 的耻辱柱里有记录：
 * 那是**点完立刻同步读**造成的误测，隔一个微任务再读就对了。）
 */
export const pipelineDialogs = (m: RunListModel) => (
  <>
    <ConfirmDialog
      id={RUN_ID}
      title="确认运行这份清单"
      confirmLabel="开始运行"
      busy={m.busy.value}
      onConfirm={() => void m.doRun()}
      impact={
        <div class="flex flex-col gap-1">
          <p>
            将按顺序跑 <b>{m.confirmTarget.value?.steps.length ?? 0}</b> 步，作用于{' '}
            <b>
              {m.confirmTarget.value === null
                ? 0
                : m.confirmTarget.value.accountIds.length === 0
                  ? m.targetCount.value
                  : m.confirmTarget.value.accountIds.length}
            </b>{' '}
            个账号。
          </p>
          <p class="break-words opacity-70">
            清单里可能有签到、领奖、上传这类<b>写操作</b>，跑完不撤回。点下去之后回的是「已受理」，
            不是「跑完了」：整条链逐步执行，结局稍后出现在「历史与日志」。
          </p>
        </div>
      }
    />
    <ConfirmDialog
      id={DELETE_ID}
      title="确认删除这份预设"
      confirmLabel="删除预设"
      busy={m.busy.value}
      onConfirm={() => void m.doDelete()}
      impact={
        <div class="flex flex-col gap-1">
          <p>
            只删预设「{m.confirmTarget.value?.name ?? '—'}」这一行。执行历史不跟着走；
            正在引用它的定时任务会让这次删除<b>失败并指名那几条</b>，不会连带删掉它们。
          </p>
        </div>
      }
    />
  </>
)
