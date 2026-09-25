/**
 * 任务中心（`#/tasks`）—— 一页挑任务、排顺序、存成预设、开跑。
 *
 * # 与兑换中心那一页的分工
 *
 * 那边只服务**一个**能力（`exchange`），参数键是固定的，所以 `fieldValue` 里
 * 可以把键名一个个写死。这里要服务**任意**能力：参数表是从 `/api/capabilities`
 * 读来的声明**现算**的，控件按 `kind` 通用渲染 ——
 * 「新增能力零前端改动」（§5.6）在这个页面才是真的。
 *
 * # 参数表单编辑的是"清单里选中的那一步"，不是一份独立的临时表单
 *
 * 早先这里是"点卡片选中能力 ⇒ 填参数 ⇒ 立即执行"。清单化之后多了一个坑：
 * 同一个表单如果有两个出处（"临时预览"的那一份 与 清单里的那一份），用户改了参数
 * 不知道改到哪儿去了。所以取消临时态：**点一张卡片就是把它加进清单**（已经在里面
 * 就跳过去选中），表单直接读写那一步的 `body`；要单跑一步用表单里那枚「只跑这一步」。
 *
 * # 卡片墙列**全部**能力（含 `hidden`）
 *
 * 一份预设里可能既要有签到、也要有 `auth-refresh`（收尾刷令牌）与 `exchange`
 * （带参数兑换），所以 `hidden` 在这里不再是过滤条件，只是卡片上一句「另有专属入口」
 * （判据与语义都写在 `ui/task-groups.ts`）。分组 tab 留着当**筛子**、不当**隔板**：
 * 「全部」那一栏在最左，也是默认落点。
 *
 * # 「受理」不等于「跑完」
 *
 * `POST /api/runs` 回 **202** + `runId` 列表；`POST /api/pipelines/{id}/run` 回 **202**
 * + `{pipelineId, name, steps}`（**没有** runId 配对 —— 每一步的 runId 要等上一步跑完
 * 才分配得出来）。两处都只说"已受理"，真实结果稍后落库（**查询类能力甚至不落库** ——
 * `core.KindQuery` 声明它不写运行历史）。
 */

import { computed, onMounted, onUnmounted, ref } from 'vue'
import {
  ApiError,
  listAccounts,
  listCapabilities,
  listRuns,
  triggerRun,
  type Account,
  type CapabilityField,
  type CapabilityRow,
  type RunRecord,
} from '../api'
import { ConfirmDialog, closeDialog, openDialog } from '../ui/dialog'
import { IconAlert, IconTask } from '../ui/icons'
import { LIST_LEVEL_PARAM_KEYS, stepBodyForWire } from '../ui/pipeline-draft'
import {
  acceptedText,
  ownEntryBadge,
  readOnlyBadge,
  riskBadge,
  tabsFor,
  type CapabilityGroup,
} from '../ui/task-groups'
import { fmtTime } from '../ui/time'
import { pipelineDialogs, presetBar, runListCard, useRunList } from './task-run-list'

type LoadState = 'loading' | 'error' | 'ready'

/** 单步确认弹窗的 id（预设那两条在 `task-run-list.tsx` 里，各自独立）。 */
const CONFIRM_ID = 'task-confirm'

/** 最近运行只显示这么多条 —— 完整历史在「历史与日志」那一页。 */
const RECENT_LIMIT = 5

/**
 * `select` 的候选值在线格式上是 **JSON 值**而不是字符串
 * （`api.ts` 的 `CapabilityField.options` 注释写明：`value` 是 `json.RawMessage`）。
 * 所以 `<option>` 的 `value` 用 `JSON.stringify` 存，回读时再解 ——
 * 直接把 `opt.value` 塞进 `value` 属性会看到界面上一对引号。
 *
 * 与 `exchange-page.tsx` 里那两个同族；哪天要统一，抽成 `ui/field-spec.tsx`。
 */
const optionToken = (v: unknown): string => JSON.stringify(v)

const tokenValue = (token: string): unknown => {
  try {
    return JSON.parse(token)
  } catch {
    // 解不开就当字符串用：宁可送一个原样的值，也不要在这里静默变成 undefined。
    return token
  }
}

export const TasksPage = () => {
  const state = ref<LoadState>('loading')
  /** 加载失败（整页换成错误态）。 */
  const errorMessage = ref('')
  /**
   * 执行失败 —— ⚠ **必须与加载失败分开**。
   *
   * 加载失败要把整页换掉；执行失败只该在表单旁说一句，页面其余部分照常可用。
   * 共用一个 ref 的话，因为渲染条件是 `state === 'error'`，而执行失败时
   * `state` 还是 `'ready'` ⇒ **一条都不显示**（点完确认框无声无息地关掉）。
   */
  const submitError = ref('')
  /** 受理成功的一句话。 */
  const notice = ref('')

  const capabilities = ref<CapabilityRow[]>([])
  const accounts = ref<Account[]>([])
  const runs = ref<RunRecord[]>([])

  /** 当前 tab（`core.Group` 的文本值；`''` = 「全部」）。 */
  const activeGroup = ref('')
  const submitting = ref(false)

  /**
   * 编排区（预设条 + 有序清单）的状态与动作。
   * `refresh` 回指本文件的 `load`：组合自己**没有**进度视图，跑完只能回读一次历史
   * 才看得到结局（这一格将来怎么补，记在 `docs/pending-decisions.md`）。
   */
  const m = useRunList({ capabilities, accounts, refresh: () => load() })

  let timer: ReturnType<typeof setInterval> | null = null

  const groups = computed<CapabilityGroup[]>(() => tabsFor(capabilities.value))

  const activeItems = computed<CapabilityRow[]>(
    () => groups.value.find((g) => g.key === activeGroup.value)?.items ?? [],
  )

  /** 表单编辑的就是清单里选中那一步；没有选中行 ⇒ 没有表单可填。 */
  const selected = computed<CapabilityRow | null>(() => m.selectedCap.value)

  /**
   * 声明里的参数，**剔掉清单级那两格**。
   *
   * 判据在 `internal/server/pipeline_run.go:293-300`：组合执行时会用清单级账号
   * 覆盖 `accounts`、把 `accountMode` 硬写成 `sequential`。留着这两格 = 给用户
   * 两格"填了永远不生效"的输入。
   */
  const params = computed<CapabilityField[]>(() =>
    (selected.value?.params ?? []).filter((f) => !LIST_LEVEL_PARAM_KEYS.includes(f.key)),
  )

  const recentRuns = computed<RunRecord[]>(() =>
    runs.value.filter((r) => r.capability === selected.value?.code).slice(0, RECENT_LIMIT),
  )

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [caps, accountList, runList] = await Promise.all([
        listCapabilities(),
        listAccounts(),
        listRuns(50),
      ])
      capabilities.value = caps.all
      accounts.value = accountList.accounts
      runs.value = runList.runs

      // 预设那一族**单独容错**：读不到不该把整页换成错误态（理由见 `useRunList`）。
      void m.reloadPipelines()

      // 首次进来或分组变了 ⇒ 落回第一个 tab（只要有能力，那就是「全部」）。
      if (!groups.value.some((g) => g.key === activeGroup.value)) {
        activeGroup.value = groups.value[0]?.key ?? ''
      }
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => {
    void load()
    // 任务跑完是异步的，隔一会儿回来看一眼 —— 不订阅 SSE（那在「历史与日志」页）。
    timer = setInterval(() => void load(), 10_000)
  })

  onUnmounted(() => {
    if (timer !== null) clearInterval(timer)
  })

  // ── 表单绑定（读写都落在选中那一步的 body 上）──────────────────────────────

  const fieldValue = (f: CapabilityField): unknown => m.editBody()[f.key]

  const setField = (f: CapabilityField, v: unknown) => {
    m.editBody()[f.key] = v
  }

  const missingRequired = computed(() =>
    params.value
      .filter((f) => f.required)
      .filter((f) => {
        const v = fieldValue(f)
        if (f.kind === 'multi_select') return !Array.isArray(v) || v.length === 0
        if (f.kind === 'bool') return false
        return v === undefined || v === null || String(v).trim() === ''
      })
      .map((f) => f.label),
  )

  const canSubmit = computed(
    () =>
      state.value === 'ready' &&
      m.selectedStep.value !== null &&
      selected.value !== null &&
      missingRequired.value.length === 0,
  )

  // ── 只跑这一步 ────────────────────────────────────────────────────────────

  const askConfirm = () => {
    if (!canSubmit.value || submitting.value) return
    const cap = selected.value
    if (cap === null) return
    // 无副作用的能力不弹确认框 —— 那不是"危险"，弹了只是噪音。
    if (!cap.needsConfirmation) {
      void doSubmit()
      return
    }
    openDialog(CONFIRM_ID)
  }

  /**
   * 单跑选中的那一步。
   *
   * 账号与执行方式取**清单级**那一份，与整条链跑起来时同一口径（后端那两行覆盖
   * 就在 `pipeline_run.go:293-300`）。如果这里改用步骤自己 body 里的 `accounts`，
   * 就会出现"清单上选了 A、单跑却打了 B"。
   */
  const doSubmit = async () => {
    const cap = selected.value
    const step = m.selectedStep.value
    if (cap === null || step === null || submitting.value) return
    submitting.value = true
    submitError.value = ''
    notice.value = ''
    try {
      // 只送声明里有的键 —— 多一个后端就 400；`number` 档在这里换成 JSON 数字，
      // 空的省略（送字符串过去必 400，判据见 `ui/pipeline-draft.ts` 的 [numberWire]）。
      const payload: Record<string, unknown> = stepBodyForWire(step.body, cap)
      payload.accounts = [...m.list.value.accountIds]
      payload.accountMode = 'sequential'
      const accepted = await triggerRun(cap.code, payload)
      closeDialog(CONFIRM_ID)
      notice.value = acceptedText(
        cap.title,
        accepted.runs.map((r) => r.runId),
      )
      await load()
    } catch (e) {
      // ⚠ 501 也走这里（`run_dispatch` 没接这条能力时）。**原样显示后端那句话**，
      // 不改写成"操作失败" —— 那句话才是排查的入口。
      submitError.value = e instanceof ApiError ? e.message : '请求失败'
      closeDialog(CONFIRM_ID)
    } finally {
      submitting.value = false
    }
  }

  // ── 控件 ────────────────────────────────────────────────────────────────

  /** 单个参数控件。按 `kind` 分支，控件一律 `w-full`（§5.6）。 */
  const fieldControl = (f: CapabilityField) => {
    if (f.kind === 'bool') {
      return (
        <fieldset class="fieldset">
          <label class="flex min-h-11 cursor-pointer items-center gap-2 md:min-h-0">
            <input
              type="checkbox"
              class="checkbox checkbox-sm"
              checked={fieldValue(f) === true}
              onChange={(e) => setField(f, (e.target as HTMLInputElement).checked)}
            />
            <span class="text-sm">{f.label}</span>
          </label>
          {f.help ? <p class="break-words text-xs opacity-60">{f.help}</p> : null}
        </fieldset>
      )
    }

    if (f.kind === 'select' && f.options && f.options.length > 0) {
      return (
        <fieldset class="fieldset">
          <legend class="fieldset-legend">{f.label}</legend>
          <select
            class="select w-full"
            value={optionToken(fieldValue(f))}
            onChange={(e) => setField(f, tokenValue((e.target as HTMLSelectElement).value))}
          >
            {f.options.map((o) => (
              <option value={optionToken(o.value)}>{o.label}</option>
            ))}
          </select>
          {f.help ? <p class="break-words text-xs opacity-60">{f.help}</p> : null}
        </fieldset>
      )
    }

    if (f.kind === 'multi_select' && f.options && f.options.length > 0) {
      const cur = Array.isArray(fieldValue(f)) ? (fieldValue(f) as unknown[]) : []
      return (
        <fieldset class="fieldset">
          <legend class="fieldset-legend">{f.label}</legend>
          <div class="flex flex-col gap-2">
            {f.options.map((o) => {
              const token = optionToken(o.value)
              return (
                <label class="flex min-h-11 cursor-pointer items-center gap-2 md:min-h-0">
                  <input
                    type="checkbox"
                    class="checkbox checkbox-sm"
                    checked={cur.some((v) => optionToken(v) === token)}
                    onChange={(e) => {
                      const next = e.target as HTMLInputElement
                      const kept = cur.filter((v) => optionToken(v) !== token)
                      setField(f, next.checked ? [...kept, tokenValue(token)] : kept)
                    }}
                  />
                  <span class="break-words text-sm">{o.label}</span>
                </label>
              )
            })}
          </div>
          {f.help ? <p class="break-words text-xs opacity-60">{f.help}</p> : null}
        </fieldset>
      )
    }

    if (f.kind === 'number') {
      // 表单里存文本，出线时才换算成 JSON 数字（判据与理由见 [numberWire]）。
      //
      // ⚠ 显示这一格**不能只认 string**：声明里的默认值与库里存回来的是 **JSON 数字**，
      // 只认 string 会把"默认 60"渲染成一个空框 —— 用户以为没填，实际送出去的是 60。
      const shown = fieldValue(f)
      return (
        <fieldset class="fieldset">
          <legend class="fieldset-legend">
            {f.label}
            {f.required ? <span class="text-error"> *</span> : null}
          </legend>
          <input
            class="input input-bordered w-full"
            inputmode="numeric"
            value={shown === undefined || shown === null ? '' : String(shown)}
            onInput={(e) => setField(f, (e.target as HTMLInputElement).value)}
          />
          {f.help ? <p class="break-words text-xs opacity-60">{f.help}</p> : null}
        </fieldset>
      )
    }

    // `json` 与 `text` 按文本处理：后端收的是 `json.RawMessage`，送字符串它自己解。
    return (
      <fieldset class="fieldset">
        <legend class="fieldset-legend">
          {f.label}
          {f.required ? <span class="text-error"> *</span> : null}
        </legend>
        {/* ⚠ 文本输入用 `onInput`，**不是** `onChange`：Vue 的 `onChange` 绑的是
            原生 `change` —— 只在**失焦**时触发。照 React 直觉写，表现是"字打进去了、
            DOM 的 value 也对，但组件不知道"：必填提示不消失、按钮一直灰着。 */}
        <input
          class="input input-bordered w-full"
          value={typeof fieldValue(f) === 'string' ? (fieldValue(f) as string) : ''}
          onInput={(e) => setField(f, (e.target as HTMLInputElement).value)}
        />
        {f.help ? <p class="break-words text-xs opacity-60">{f.help}</p> : null}
      </fieldset>
    )
  }

  // ── 渲染 ────────────────────────────────────────────────────────────────

  const head = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">任务中心</h1>
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        disabled={state.value === 'loading'}
        onClick={() => void load()}
      >
        刷新
      </button>
    </div>
  )

  /** tabs 横滑分组（§7.2）。⚠ 类名**整条字面量**，不做字符串拼接（§9.1①）。 */
  const tabsRow = () => (
    <div role="tablist" class="tabs tabs-box w-full overflow-x-auto">
      {groups.value.map((g) => (
        <button
          type="button"
          role="tab"
          aria-selected={g.key === activeGroup.value}
          class={
            /* ⚠ `min-h-11 md:min-h-0`：窄屏把 tab 补到 44px（§7.4「所有按钮可点中」），
               桌面回落 daisyUI 的 40px 密度。类名**整条字面量**，不拼接（§9.1①）。 */
            g.key === activeGroup.value
              ? 'tab tab-active min-h-11 whitespace-nowrap md:min-h-0'
              : 'tab min-h-11 whitespace-nowrap md:min-h-0'
          }
          onClick={() => {
            // 换 tab **不动**清单与选中行：这一栏只是筛子。选中的那一步哪怕不在当前
            // tab 里，也还在清单里、还在原来那个序号上 —— 清掉的话"在别的组挑一项"
            // 就会把正在编辑的那一步踢出表单。
            activeGroup.value = g.key
          }}
        >
          {g.label}
        </button>
      ))}
    </div>
  )

  /**
   * 能力卡片墙：**一张卡片 = 清单里的一项**。
   *
   * 点整张卡就是"要跑它"（加到清单末尾并选中）；已经在清单里则跳过去选中。
   * 右上角那个序号既是"它在清单里的第几步"，也是"再点一次不会多出一行"的预告 ——
   * 要故意跑两次，用表单里那枚「再加一次这一步」。
   *
   * ⚠ 外壳是 `<button type="button">` 而不是带 `onClick` 的 `<div>`：这一枚是**整页的主交互**，
   * 用 div 它就不进无障碍树（可访问性快照里只有一串 StaticText，看不出可点），键盘也到不了。
   * `p-0` 抵掉按钮自带的内边距（内距由里面的 `card-body p-4` 给）；`text-left` 抵掉
   * 按钮默认居中的文字。
   */
  const cardList = () => (
    <div class="grid gap-3 md:grid-cols-2">
      {activeItems.value.map((cap) => {
        const order = m.orderOf(cap.code)
        const on = selected.value?.code === cap.code
        const risk = riskBadge(cap.risk)
        const ro = readOnlyBadge(cap.kind)
        const own = ownEntryBadge(cap.hidden)
        return (
          <button
            type="button"
            aria-pressed={on}
            class={
              on
                ? 'card cursor-pointer border-2 border-primary bg-base-100 p-0 text-left shadow-sm'
                : 'card cursor-pointer border-2 border-transparent bg-base-100 p-0 text-left shadow-sm'
            }
            onClick={() => m.useCapability(cap)}
          >
            <div class="card-body gap-2 p-4">
              <div class="flex flex-wrap items-center gap-2">
                <span class="card-title text-sm">{cap.title}</span>
                <span class={risk.cls}>{risk.label}</span>
                {ro ? <span class={ro.cls}>{ro.label}</span> : null}
                {own ? <span class={own.cls}>{own.label}</span> : null}
                {order === null ? null : (
                  <span class="badge badge-sm badge-primary">第 {order} 步</span>
                )}
              </div>
              <p class="break-words text-xs opacity-70">{cap.description}</p>
              <code class="text-xs opacity-50">{cap.code}</code>
            </div>
          </button>
        )
      })}
    </div>
  )

  /** 选中那一步的参数（§7.2：任务中心的表单**全宽**）。 */
  const formCard = () => {
    const cap = selected.value
    const step = m.selectedStep.value
    if (cap === null || step === null) {
      return (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body items-center gap-2 py-8 text-center">
            <span class="text-base-content/30">
              <IconTask class="size-8" />
            </span>
            <p class="text-sm opacity-70">还没有选中任何一步。</p>
            <p class="max-w-prose break-words text-xs opacity-60">
              在下面的卡片墙点一张，它会带着默认参数进清单；参数在这一栏改，改完就是
              清单里那一步的值。整张表照 <code>/api/capabilities</code> 的声明渲染 ——
              一条都没读到，说明后端没返回能力。
            </p>
          </div>
        </div>
      )
    }
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-4">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">
              第 {m.selectedOrder.value} 步 · {cap.title} 的参数
            </h2>
            <span class={riskBadge(cap.risk).cls}>{riskBadge(cap.risk).label}</span>
          </div>

          {step.rawBody ? (
            // 存量数据里这一步的 `body` 不是一个对象（`PipelineStep.Body` 收任意 JSON 值）。
            // 不写这一句，用户会以为"参数不见了是我的错"。
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <span class="break-words">
                这一步原先带的参数不是一个对象（原文 <code>{step.rawBody}</code>
                ），填不进下面的格子； 已按默认值处理，<b>保存之后原文不再保留</b>。
              </span>
            </div>
          ) : null}

          {params.value.length === 0 ? (
            <p class="text-sm opacity-70">这条能力没有参数，直接执行即可。</p>
          ) : (
            <div class="flex flex-col gap-2">{params.value.map((f) => fieldControl(f))}</div>
          )}

          {missingRequired.value.length > 0 ? (
            <p class="text-xs opacity-60">还差：{missingRequired.value.join('、')}</p>
          ) : null}

          {/* 后端原话照登（501 时就是"这条路还没接"），不翻译成"操作失败"。 */}
          {submitError.value ? (
            <div class="alert alert-soft alert-error text-sm" role="alert">
              <IconAlert class="size-4" />
              <span class="break-words">{submitError.value}</span>
            </div>
          ) : null}

          {notice.value ? (
            <div class="alert alert-soft alert-success text-sm" role="status">
              <span class="break-words">{notice.value}</span>
            </div>
          ) : null}

          <div class="card-actions justify-end">
            <button
              type="button"
              class="btn btn-soft min-h-11 md:min-h-0"
              onClick={() => m.addAnother()}
            >
              再加一次这一步
            </button>
            <button
              type="button"
              class="btn btn-primary min-h-11 md:min-h-0"
              disabled={!canSubmit.value || submitting.value}
              onClick={askConfirm}
            >
              {submitting.value ? '执行中…' : '只跑这一步'}
            </button>
          </div>
        </div>
      </div>
    )
  }

  /** 当前那一步对应能力的最近几次运行（完整历史在「历史与日志」页）。 */
  const recentCard = () => {
    const cap = selected.value
    if (cap === null) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">最近运行</h2>
            <span class="text-xs opacity-60">
              只看「{cap.title}」最近 {RECENT_LIMIT} 次
            </span>
          </div>

          {recentRuns.value.length === 0 ? (
            <div class="flex flex-col gap-1 py-4">
              <p class="text-sm opacity-70">还没有这一条能力的运行记录。</p>
              {cap.kind === 'query' ? (
                // ⚠ 查询类**不写运行历史**（`core.KindQuery` 的声明）——
                // 不提前说清楚，看起来就像"按钮没生效"。
                //
                // ⚠⚠ 这里只能用**行注释**，不能用 JSX 的花括号注释：本行处在 `? (`
                // 之后的**表达式位置**，花括号注释在那里是一个独立表达式（值为
                // undefined），于是下面的 `<p>` 成了第二个 ⇒ `Expected ')'`，
                // typecheck 与 build **一起**红。
                //
                // ⚠ 正文也不换行：JSX 把换行折成空格，中文里"噪音， 所以"那个空格很显眼。
                <p class="break-words text-xs opacity-60">
                  这条能力是<b>只读</b>
                  的：它不写运行历史、不产生日志噪音，所以跑完也不会在这里多出一行 ——
                  那是设计如此，不是没跑。
                </p>
              ) : null}
            </div>
          ) : (
            <>
              {/* 桌面表格 */}
              <div class="hidden overflow-x-auto md:block">
                <table class="table table-sm">
                  <thead>
                    <tr>
                      <th>开始</th>
                      <th>账号</th>
                      <th>状态</th>
                      <th>结论</th>
                    </tr>
                  </thead>
                  <tbody>
                    {recentRuns.value.map((r) => (
                      <tr>
                        <td class="whitespace-nowrap">{fmtTime(r.startedAt)}</td>
                        <td class="break-words">{r.accountId ?? '—'}</td>
                        <td>
                          <span class="badge badge-soft">{r.status}</span>
                        </td>
                        <td class="break-words">{r.message ?? '—'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              {/* 移动卡片（每项带标签，§7.3） */}
              <div class="grid gap-3 md:hidden">
                {recentRuns.value.map((r) => (
                  <div class="rounded-box bg-base-200 p-3">
                    <div class="flex items-center justify-between gap-2 text-sm">
                      <span class="font-medium">{fmtTime(r.startedAt)}</span>
                      <span class="badge badge-soft">{r.status}</span>
                    </div>
                    <div class="mt-1 flex items-center justify-between gap-2 text-sm">
                      <span class="opacity-70">账号</span>
                      <span class="break-words">{r.accountId ?? '—'}</span>
                    </div>
                    <div class="mt-1 flex items-center justify-between gap-2 text-sm">
                      <span class="opacity-70">结论</span>
                      <span class="break-words text-right">{r.message ?? '—'}</span>
                    </div>
                  </div>
                ))}
              </div>
            </>
          )}
        </div>
      </div>
    )
  }

  return (
    <div class="flex flex-col gap-4">
      {head()}

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-spinner loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <IconAlert class="size-4" />
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (
        <>
          {presetBar(m)}
          {runListCard(m)}

          {groups.value.length <= 1 ? (
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <span class="break-words">
                后端没有返回任何能力。这一栏是照 <code class="mx-1">/api/capabilities</code>
                的声明渲染的 —— 注册表空了，或这条服务没装配能力表。
              </span>
            </div>
          ) : (
            <>
              <h2 class="text-base font-medium">挑任务：点一张卡片就加进上面那份清单</h2>
              {tabsRow()}
              {cardList()}
            </>
          )}

          {formCard()}
          {recentCard()}
        </>
      ) : null}

      <ConfirmDialog
        id={CONFIRM_ID}
        title="确认执行"
        confirmLabel="确认执行"
        busy={submitting.value}
        onConfirm={() => void doSubmit()}
        impact={
          /* ⚠ 不能写成 `selected.value ? <div/> : null` —— `impact` 的类型是
             `JSX.Element`（`dialog.tsx`），**不收 null**。三元写法的另一支是 null，
             typecheck 会报 TS2322，而 build 照样过（esbuild 不做类型检查）。
             做法与兑换页一致：**始终给一个 div**，条件放到它内部。 */
          <div class="flex flex-col gap-1">
            {/* ⚠ 不换行：JSX 会把换行折成空格，中文里"个账号 执行"那个空格很显眼。 */}
            <p>
              将对 <b>{m.targetCount.value}</b> 个账号执行<b>这一步</b>：
              <b>{selected.value?.title ?? '（未选中）'}</b>。清单里其它步骤不会跟着跑。
            </p>
            {selected.value?.risk === 'high' ? (
              <p class="text-error">
                这条能力有<b>不可逆</b>的写操作，执行后无法撤销。
              </p>
            ) : null}
          </div>
        }
      />

      {pipelineDialogs(m)}
    </div>
  )
}
