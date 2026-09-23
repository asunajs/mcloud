/**
 * 任务中心（`#/tasks`）—— 配置并触发各能力。
 *
 * # 与兑换中心那一页的分工
 *
 * 那边只服务**一个**能力（`exchange`），参数键是固定的，所以 `fieldValue` 里
 * 可以把键名一个个写死。这里要服务**任意**能力：参数表是从 `/api/capabilities`
 * 读来的声明**现算**的，控件按 `kind` 通用渲染 ——
 * 「新增能力零前端改动」（§5.6）在这个页面才是真的。
 *
 * # 只列 `hidden === false` 的能力
 *
 * `exchange` 与 `auth-refresh` 都有自己的入口（兑换中心页 / 账号页那枚按钮），
 * 混进通用列表会让同一枚「立即执行」背上两个语义。但它们**仍是合法的调度目标**
 * —— 定时任务选目标走的是另一条路，不看 `hidden`。判据在 `ui/task-groups.ts`。
 *
 * # 「受理」不等于「跑完」
 *
 * `POST /api/runs` 回的是 **202**，带的是 `runId` 列表，不是结果。
 * 所以这里只说"已受理"，真实结果稍后落库（**查询类能力甚至不落库** ——
 * `core.KindQuery` 声明它不写运行历史）。
 */

import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
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
import {
  acceptedText,
  groupCapabilities,
  readOnlyBadge,
  riskBadge,
  type CapabilityGroup,
} from '../ui/task-groups'
import { fmtTime } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/** 确认弹窗的 id（`ui/dialog.tsx` 靠 id 找 DOM，见那个文件头）。 */
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

  /** 当前 tab（`core.Group` 的文本值）。 */
  const activeGroup = ref('')
  /** 当前选中的能力 code；`null` = 还没选。 */
  const selectedCode = ref<string | null>(null)
  /** 参数值。键名就是声明里的 `key` —— 送错键后端 400（`不认识的参数：…`）。 */
  const form = reactive<Record<string, unknown>>({})
  const submitting = ref(false)

  let timer: ReturnType<typeof setInterval> | null = null

  const groups = computed<CapabilityGroup[]>(() => groupCapabilities(capabilities.value))

  const activeItems = computed<CapabilityRow[]>(
    () => groups.value.find((g) => g.key === activeGroup.value)?.items ?? [],
  )

  const selected = computed<CapabilityRow | null>(
    () => capabilities.value.find((c) => c.code === selectedCode.value) ?? null,
  )

  const params = computed<CapabilityField[]>(() => selected.value?.params ?? [])

  const recentRuns = computed<RunRecord[]>(() =>
    runs.value.filter((r) => r.capability === selectedCode.value).slice(0, RECENT_LIMIT),
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

      // 分组变了（或首次进来）就把 tab 落回第一个**有能力的**组。
      const list = groups.value
      if (!list.some((g) => g.key === activeGroup.value)) {
        activeGroup.value = list[0]?.key ?? ''
      }
      // 选中的能力可能已被摘掉（改了声明、或它本来就在别的组）⇒ 重新落位。
      if (!capabilities.value.some((c) => c.code === selectedCode.value)) {
        selectCapability(activeItems.value[0] ?? null)
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

  // ── 选择与表单 ──────────────────────────────────────────────────────────

  /**
   * 换一个能力 ⇒ 表单**整体重建**。
   *
   * ⚠ 不能"保留旧值、只补新键"：两个能力的参数键会撞名（`accounts` / `accountMode`
   * 是 `accountSelector()` 给的，几乎每个能力都有），撞上之后上一个能力填的
   * 奖品名就会跟着送进下一个能力的请求里 —— 而后端只认自己声明的键，多一个就 400。
   */
  const selectCapability = (cap: CapabilityRow | null) => {
    selectedCode.value = cap?.code ?? null
    submitError.value = ''
    notice.value = ''
    for (const k of Object.keys(form)) delete form[k]
    if (!cap) return
    for (const f of cap.params) {
      if (f.default !== undefined) form[f.key] = f.default
      else if (f.kind === 'bool') form[f.key] = false
      else if (f.kind === 'multi_select') form[f.key] = []
      else form[f.key] = ''
    }
  }

  /** 切 tab 时清空选择 —— 选中的能力未必属于新 tab。 */
  const switchGroup = (key: string) => {
    activeGroup.value = key
    const first = groups.value.find((g) => g.key === key)?.items[0] ?? null
    selectCapability(first)
  }

  /** 声明里那一栏是不是"账号多选"（`accountSelector()` 给的第一项）。 */
  const isAccountField = (f: CapabilityField) => f.key === 'accounts'

  const fieldValue = (f: CapabilityField): unknown => form[f.key]

  const setField = (f: CapabilityField, v: unknown) => {
    form[f.key] = v
  }

  const toggleAccount = (id: string, on: boolean) => {
    const cur = Array.isArray(form.accounts) ? (form.accounts as string[]) : []
    form.accounts = on ? [...cur, id] : cur.filter((x) => x !== id)
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
    () => state.value === 'ready' && selected.value !== null && missingRequired.value.length === 0,
  )

  /**
   * 确认框里那句"将对 N 个账号"。
   *
   * ⚠ 留空 = **全部账号**（与 `accountSelector()` 声明的语义一致），
   * 所以"一个都没勾"不等于"0 个目标"，那是 `accounts.value.length` 个。
   */
  const targetAccountCount = computed(() => {
    const picked = Array.isArray(form.accounts) ? (form.accounts as string[]).length : 0
    return picked === 0 ? accounts.value.length : picked
  })

  // ── 执行 ────────────────────────────────────────────────────────────────

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

  const doSubmit = async () => {
    const cap = selected.value
    if (cap === null || submitting.value) return
    submitting.value = true
    submitError.value = ''
    notice.value = ''
    try {
      // 只送声明里有的键 —— 多一个后端就 400。
      const payload: Record<string, unknown> = {}
      for (const f of cap.params) {
        const v = fieldValue(f)
        if (v !== undefined) payload[f.key] = v
      }
      const accepted = await triggerRun(cap.code, payload)
      closeDialog(CONFIRM_ID)
      notice.value = acceptedText(cap.title, accepted.runs.map((r) => r.runId))
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
    if (isAccountField(f)) {
      return (
        <fieldset class="fieldset">
          <legend class="fieldset-legend">{f.label}</legend>
          <div class="flex flex-col gap-2">
            {accounts.value.length === 0 ? (
              /* 空态要给出下一步（§5.10），不写"暂无数据"。 */
              <p class="break-words text-sm opacity-70">
                还没有云盘账号。留空代表「全部账号」，但现在一个都没有 —— 先去「账号」页添加，再回来跑任务。
              </p>
            ) : (
              accounts.value.map((a) => (
                <label class="flex min-h-11 cursor-pointer items-center gap-2 md:min-h-0">
                  <input
                    type="checkbox"
                    class="checkbox checkbox-sm"
                    checked={
                      Array.isArray(form.accounts) && (form.accounts as string[]).includes(a.id)
                    }
                    onChange={(e) =>
                      toggleAccount(a.id, (e.target as HTMLInputElement).checked)
                    }
                  />
                  <span class="break-words text-sm">{a.displayName}</span>
                </label>
              ))
            )}
          </div>
          {f.help ? <p class="break-words text-xs opacity-60">{f.help}</p> : null}
        </fieldset>
      )
    }

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

    // `json` 与 `text` / `number` 都按文本处理：后端收的是 `json.RawMessage`，
    // 送字符串它自己解；这里替它猜类型反而会在"用户想送数字"时送错。
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
          onClick={() => switchGroup(g.key)}
        >
          {g.label}
        </button>
      ))}
    </div>
  )

  /** 能力卡片列表：点一张就选中它（选中态给描边，不靠 hover）。 */
  const cardList = () => (
    <div class="grid gap-3 md:grid-cols-2">
      {activeItems.value.map((cap) => {
        const on = cap.code === selectedCode.value
        const risk = riskBadge(cap.risk)
        const ro = readOnlyBadge(cap.kind)
        return (
          <button
            type="button"
            class={
              on
                ? 'card cursor-pointer border-2 border-primary bg-base-100 text-left shadow-sm'
                : 'card cursor-pointer border-2 border-transparent bg-base-100 text-left shadow-sm'
            }
            onClick={() => selectCapability(cap)}
          >
            <div class="card-body gap-2 p-4">
              <div class="flex flex-wrap items-center gap-2">
                <span class="card-title text-sm">{cap.title}</span>
                <span class={risk.cls}>{risk.label}</span>
                {ro ? <span class={ro.cls}>{ro.label}</span> : null}
              </div>
              <p class="break-words text-xs opacity-70">{cap.description}</p>
              <code class="text-xs opacity-50">{cap.code}</code>
            </div>
          </button>
        )
      })}
    </div>
  )

  /** 参数表单**全宽**（§7.2 对任务中心的规定）。 */
  const formCard = () => {
    const cap = selected.value
    if (cap === null) {
      return (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body items-center gap-2 py-8 text-center">
            <span class="text-base-content/30">
              <IconTask class="size-8" />
            </span>
            <p class="text-sm opacity-70">还没有可执行的能力。</p>
            <p class="max-w-prose break-words text-xs opacity-60">
              这一栏是照 <code>/api/capabilities</code> 的声明渲染的；
              一个都没读到，说明后端没返回可见能力（或全部被 <code>hidden</code> 了）。
            </p>
          </div>
        </div>
      )
    }
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-4">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">执行「{cap.title}」</h2>
            <span class={riskBadge(cap.risk).cls}>{riskBadge(cap.risk).label}</span>
          </div>

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
              class="btn btn-primary min-h-11 md:min-h-0"
              disabled={!canSubmit.value || submitting.value}
              onClick={askConfirm}
            >
              {submitting.value ? '执行中…' : '立即执行'}
            </button>
          </div>
        </div>
      </div>
    )
  }

  /** 当前能力的最近几次运行（完整历史在「历史与日志」页）。 */
  const recentCard = () => {
    const cap = selected.value
    if (cap === null) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">最近运行</h2>
            <span class="text-xs opacity-60">只看「{cap.title}」最近 {RECENT_LIMIT} 次</span>
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
                  这条能力是<b>只读</b>的：它不写运行历史、不产生日志噪音，所以跑完也不会在这里多出一行 —— 那是设计如此，不是没跑。
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
          {groups.value.length === 0 ? (
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <span class="break-words">
                后端没有返回任何<b>可见</b>能力。要么注册表是空的，要么全被
                <code class="mx-1">hidden</code> 了 —— 后者是设计如此，那些能力有自己的入口。
              </span>
            </div>
          ) : (
            <>
              {tabsRow()}
              {cardList()}
              {formCard()}
              {recentCard()}
            </>
          )}
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
              将对 <b>{targetAccountCount.value}</b> 个账号执行{' '}
              <b>{selected.value?.title ?? '（未选中）'}</b>。
            </p>
            {selected.value?.risk === 'high' ? (
              <p class="text-error">这条能力有<b>不可逆</b>的写操作，执行后无法撤销。</p>
            ) : null}
          </div>
        }
      />
    </div>
  )
}
