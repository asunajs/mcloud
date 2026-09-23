/**
 * 兑换中心 —— **抢兑优先布局**（组件指南 §7.2：「倒计时在最上，其余依次下移」）。
 *
 * 五块，顺序即 §7.2 定的顺序：
 *  ① 抢兑倒计时（最上）；
 *  ② 云朵与概览（`stats`，移动端竖排）；
 *  ③ 奖品列表（桌面 `table` / 移动卡片，§5.3 双形态）；
 *  ④ 手动兑换表单 + 二次确认（`RiskHigh` ⇒ §5.8 必须弹确认并写清影响面）；
 *  ⑤ 最近兑换记录。
 *
 * # ⚠ 后端现状：这一页有两处"未就绪"，页面上如实显示、不伪造
 *
 * 1. **执行面没接**：`internal/server/run_dispatch.go` 的 `runBuilders` 只登记了
 *    `sign` / `live-room` / `auth-refresh` 三条，**`exchange` 不在表里**
 *    （该文件注释原文：「表里没有的两条（`exchange` / `cloud-snapshot`）是
 *    **真的还没接**」）。⇒ 从这一页点"兑换"会回 **501**，页面把后端那句
 *    原样显示出来，不改写成"操作失败"这种含糊话。
 *
 * 2. **奖品列表没有接口**：旧项目走独立轨 `GET /api/exchange/prizes`
 *    （见 `docs/business-feature-inventory.md` §exchange），本仓库尚未建。
 *    ⇒ 列表区显示空态 + 说明，**不拿占位数据充数**。
 *
 * # 参数面从哪来
 *
 * 表单**完全按 `/api/capabilities` 里 `exchange` 那条的 `params` 声明渲染**
 * （组件指南 §5.6：参数表单由 `FieldSpec` 生成，"新增能力零前端改动"是硬指标）。
 * 所以这里没有一份写死的字段表 —— 后端改声明，界面跟着变。
 * 键名也以声明为准：多送一个键后端会 400（`不认识的参数：…`）。
 *
 * # ⚠ 与注册表的一处差异（不是本页能决定的）
 *
 * `docs/business-feature-inventory.md` §exchange 记的旧项目参数是三个
 * （`prizeIdOrName` / `offset` / `skipWait`），而本仓库注册表里只有
 * `prizeIdOrName` + `skipWait`（**没有 `offset`**）。本页按**注册表**渲染 ——
 * 差异登记在 `docs/status.md`，不在这里替后端补一个字段。
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
import {
  PHASE_BADGE,
  PHASE_LABEL,
  countdownText,
  countdownTone,
  fmtClock,
  nextExchangeWindow,
} from '../ui/exchange-window'
import { IconAlert, IconCloud, IconExchange } from '../ui/icons'
import { fmtTime } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/** 确认弹窗的 id（`ui/dialog.tsx` 靠 id 找 DOM，见那个文件头）。 */
const CONFIRM_ID = 'exchange-confirm'

/** 表单值。键名与注册表声明的 `key` 一致 —— 送错键后端 400。 */
interface FormState {
  accounts: string[]
  accountMode: string
  prizeIdOrName: string
  skipWait: boolean
}

/**
 * `select` 的候选值在线格式上是 **JSON 值**而不是字符串
 * （`api.ts` 的 `CapabilityField.options` 注释写明：`value` 是 `json.RawMessage`）。
 * 所以 `<option>` 的 `value` 用 `JSON.stringify` 存，回读时再解 —— 直接把
 * `opt.value` 塞进 `value` 属性会看到界面上一对引号。
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

export const ExchangePage = () => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  /**
   * ⚠ 提交失败**必须与加载失败分开**，不能共用一个 ref。
   *
   * 上面那个 `errorMessage` 只在 `state === 'error'` 时渲染，而提交失败时
   * `state` 还是 `'ready'` —— 共用的话错误**一条都不显示**（点完确认框就
   * 无声无息地关掉，像什么都没发生）。实测 501 就是这么被吞掉的。
   *
   * 两者本就该分开：加载失败 → 整页换成错误态；提交失败 → 页面照常可用，
   * 只在表单旁说一句后端原话。
   */
  const submitError = ref('')

  const capability = ref<CapabilityRow | null>(null)
  const accounts = ref<Account[]>([])
  const runs = ref<RunRecord[]>([])

  /** 当前时刻（每秒推进一次），只用来驱动倒计时；不参与任何请求。 */
  const now = ref(new Date())
  let timer: ReturnType<typeof setInterval> | null = null

  const form = reactive<FormState>({
    accounts: [],
    accountMode: 'sequential',
    prizeIdOrName: '',
    skipWait: true,
  })

  /** 提交中的标志：挡住连点（`triggerRun` 不幂等，见 `api.ts` 那条警告）。 */
  const submitting = ref(false)

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [caps, accountList, runList] = await Promise.all([
        listCapabilities(),
        listAccounts(),
        listRuns(50),
      ])
      capability.value = caps.all.find((c) => c.code === 'exchange') ?? null
      accounts.value = accountList.accounts
      // 只留兑换这一条能力的记录 —— 通用执行历史在「历史与日志」那一页。
      runs.value = runList.runs.filter((r) => r.capability === 'exchange')
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => {
    void load()
    timer = setInterval(() => (now.value = new Date()), 1000)
  })
  onUnmounted(() => {
    if (timer !== null) clearInterval(timer)
  })

  // ───────────────────────────────────────────────────────────────────────────
  // 倒计时
  // ───────────────────────────────────────────────────────────────────────────

  const window = computed(() => nextExchangeWindow(now.value))

  /**
   * 本机时钟是否可信 —— **不判**，只如实标注。
   *
   * 组件指南 §5.9 要求"距开抢时间是服务端给的"，但抢兑窗口是**固定规则**
   * （每个整点），本仓库还没有给出权威时刻的接口，所以这里由本机时钟推。
   * 与其假装它是权威的，不如在界面上把这件事说清楚。
   */
  const CLOCK_NOTE = '按本机时间推算；本机时钟有偏差时倒计时也会偏。'

  // ───────────────────────────────────────────────────────────────────────────
  // 概览数字
  // ───────────────────────────────────────────────────────────────────────────

  const cloudTotal = computed(() =>
    accounts.value.reduce((sum, a) => sum + (a.cloudCurrent ?? 0), 0),
  )
  const anyCloud = computed(() => accounts.value.some((a) => a.cloudCurrent !== null))
  const lastRun = computed(() => runs.value[0] ?? null)

  // ───────────────────────────────────────────────────────────────────────────
  // 参数表单（按声明渲染）
  // ───────────────────────────────────────────────────────────────────────────

  const params = computed<CapabilityField[]>(() => capability.value?.params ?? [])

  /** 声明里那一栏是不是"账号多选"（`accountSelector()` 给的第一项）。 */
  const isAccountField = (f: CapabilityField) => f.key === 'accounts'

  const fieldValue = (f: CapabilityField): unknown => {
    if (f.key === 'accounts') return form.accounts
    if (f.key === 'accountMode') return form.accountMode
    if (f.key === 'prizeIdOrName') return form.prizeIdOrName
    if (f.key === 'skipWait') return form.skipWait
    return undefined
  }

  const setField = (f: CapabilityField, v: unknown) => {
    if (f.key === 'accounts') form.accounts = v as string[]
    else if (f.key === 'accountMode') form.accountMode = String(v)
    else if (f.key === 'prizeIdOrName') form.prizeIdOrName = String(v)
    else if (f.key === 'skipWait') form.skipWait = v === true
  }

  const toggleAccount = (id: string, on: boolean) => {
    form.accounts = on ? [...form.accounts, id] : form.accounts.filter((x) => x !== id)
  }

  /**
   * 必填校验。**只校验声明里 `required` 的那一栏**，不额外发明规则
   * （旧项目就是"不校验存在性/类型/库存"，见 business-feature-inventory §exchange）。
   */
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
    () => state.value === 'ready' && capability.value !== null && missingRequired.value.length === 0,
  )

  /** 影响面：具体到"几个账号 × 哪个奖品"。§6.4：只写「确定吗？」等于没确认。 */
  const impactText = computed(() => {
    const targets = form.accounts.length === 0 ? accounts.value.map((a) => a.displayName) : null
    const count = targets ? targets.length : form.accounts.length
    const names = targets
      ? targets.map((n) => n).join('、')
      : form.accounts
          .map((id) => accounts.value.find((a) => a.id === id)?.displayName ?? id)
          .join('、')
    return {
      count,
      /* ⚠ 兜底值**不带括号**：外层模板自己会套「（…）」。
         这里再带一层就渲染成「（（没有账号））」。 */
      names: names || '',
      prize: form.prizeIdOrName.trim() || '（未填）',
      skipWait: form.skipWait,
    }
  })

  const askConfirm = () => {
    if (!canSubmit.value || submitting.value) return
    openDialog(CONFIRM_ID)
  }

  const doSubmit = async () => {
    if (submitting.value) return
    submitting.value = true
    submitError.value = ''
    try {
      // 只送声明里有的键 —— 多一个后端就 400。
      const payload: Record<string, unknown> = {}
      for (const f of params.value) {
        const v = fieldValue(f)
        if (v !== undefined) payload[f.key] = v
      }
      await triggerRun('exchange', payload)
      closeDialog(CONFIRM_ID)
      errorMessage.value = ''
      await load()
    } catch (e) {
      // ⚠ 后端没接执行时这里是 501。**原样显示后端那句话**，
      // 不改写成"操作失败"—— 那句话才是排查的入口。
      submitError.value = e instanceof ApiError ? e.message : '请求失败'
      closeDialog(CONFIRM_ID)
    } finally {
      submitting.value = false
    }
  }

  // ───────────────────────────────────────────────────────────────────────────
  // 渲染
  // ───────────────────────────────────────────────────────────────────────────

  const head = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="text-base font-medium">兑换中心</h1>
        <span class={PHASE_BADGE[window.value.phase]}>{PHASE_LABEL[window.value.phase]}</span>
      </div>
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

  /** ① 抢兑倒计时 —— §7.2 要求它在最上。 */
  const countdownCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-2">
        <div class="flex flex-wrap items-baseline justify-between gap-2">
          <span class="text-sm opacity-70">距下一次开抢</span>
          <span class={`font-mono text-2xl ${countdownTone(window.value.msToOpen)}`}>
            {countdownText(window.value.msToOpen)}
          </span>
        </div>
        <div class="flex flex-wrap gap-x-4 gap-y-1 text-xs opacity-60">
          <span>开抢时刻 {fmtClock(window.value.nextHour)}</span>
          <span>预热开始 {fmtClock(window.value.preheatAt)}</span>
          <span>窗口关闭 {fmtClock(window.value.closeAt)}</span>
        </div>
        <p class="break-words text-xs opacity-60">{CLOCK_NOTE}</p>
      </div>
    </div>
  )

  const statsRow = () => (
    <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
      <div class="stat">
        <div class="stat-title">云朵合计</div>
        <div class="stat-value text-2xl">{anyCloud.value ? cloudTotal.value : '—'}</div>
        <div class="stat-desc">{anyCloud.value ? `${accounts.value.length} 个账号` : '尚未采集'}</div>
      </div>
      <div class="stat">
        <div class="stat-title">奖品列表</div>
        <div class="stat-value text-2xl">—</div>
        <div class="stat-desc">接口待建</div>
      </div>
      <div class="stat">
        <div class="stat-title">最近兑换</div>
        <div class="stat-value text-2xl">
          {lastRun.value ? fmtTime(lastRun.value.startedAt) : '—'}
        </div>
        <div class="stat-desc">{lastRun.value ? lastRun.value.status : '还没有记录'}</div>
      </div>
    </div>
  )

  /**
   * ③ 奖品列表 —— 后端接口未建，如实空态（§5.10：空态要给出下一步，不写"暂无数据"）。
   */
  const prizeCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <h2 class="card-title text-base">可兑奖品</h2>
        <div class="flex flex-col items-center gap-2 py-8 text-center">
          <span class="text-base-content/30">
            <IconExchange class="size-8" />
          </span>
          <p class="text-sm opacity-70">奖品列表接口还没有建。</p>
          <p class="max-w-prose break-words text-xs opacity-60">
            它走的是
            <code class="mx-1 rounded bg-base-200 px-1 py-0.5">
              GET /ycloud/signin/page/exchangeList
            </code>
            ，本仓库尚无对应路由。在那之前，直接在下面按 <b>奖品 ID 或名称</b> 兑换即可。
          </p>
        </div>
      </div>
    </div>
  )

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
                还没有云盘账号。留空代表「全部账号」，但现在一个都没有 ——
                先去「账号」页添加，再回来兑换。
              </p>
            ) : (
              accounts.value.map((a) => (
                <label class="flex min-h-11 cursor-pointer items-center gap-2 md:min-h-0">
                  <input
                    type="checkbox"
                    class="checkbox checkbox-sm"
                    checked={form.accounts.includes(a.id)}
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
              checked={form.skipWait}
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
            value={optionToken(form.accountMode)}
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

    // 默认按文本处理（`text` / 未识别的 kind）。
    return (
      <fieldset class="fieldset">
        <legend class="fieldset-legend">
          {f.label}
          {f.required ? <span class="text-error"> *</span> : null}
        </legend>
        {/*
          ⚠ 这里**不给 placeholder**：`f.help` 就在下一行显示，两处写同一句话
          等于把同一件事说两遍（实机上看得很清楚，一行灰字重复）。
          提示只留下面那一处 —— 它是声明给的原文，改声明即改提示。
        */}
        <input
          class="input input-bordered w-full"
          value={f.key === 'prizeIdOrName' ? form.prizeIdOrName : ''}
          /* ⚠ 文本输入必须用 `onInput`，**不是** `onChange`。
             Vue 的 `onChange` 绑的是原生 `change` —— 只在**失焦**时触发，
             而 React 的 `onChange` 是每次输入都触发。照 React 的直觉写，
             表现是"字打进去了、DOM 的 value 也是对的，但组件不知道"：
             必填提示不消失、提交按钮一直是灰的，**点一下别处才活过来**。
             本项目其余文本输入（auth-page / schedules / accounts）都走 `onInput`，
             checkbox 与 select 才是 `onChange`（那两个的原生事件本就是"选完即触发"）。 */
          onInput={(e) => setField(f, (e.target as HTMLInputElement).value)}
        />
        {f.help ? <p class="break-words text-xs opacity-60">{f.help}</p> : null}
      </fieldset>
    )
  }

  /** ④ 手动兑换表单。 */
  const formCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <div class="flex flex-wrap items-center gap-2">
          <h2 class="card-title text-base">手动兑换</h2>
          <span class="badge badge-soft badge-error">不可逆</span>
        </div>

        {capability.value === null ? (
          <div class="alert alert-soft alert-warning text-sm" role="alert">
            <span class="break-words">
              能力表里没有 <code>exchange</code> 这一条，表单无法按声明渲染。
            </span>
          </div>
        ) : (
          <>
            {params.value.map((f) => fieldControl(f))}

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

            <div class="card-actions justify-end">
              <button
                type="button"
                class="btn btn-error min-h-11 md:min-h-0"
                disabled={!canSubmit.value || submitting.value}
                onClick={askConfirm}
              >
                立即兑换
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  )

  /** ⑤ 最近兑换记录。 */
  const runCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <h2 class="card-title text-base">最近兑换</h2>
        {runs.value.length === 0 ? (
          <p class="py-6 text-center text-sm opacity-70">
            还没有兑换记录。执行面尚未接通时，这里不会有数据 ——
            那不是"没兑过"，而是"这一条路还没打开"。
          </p>
        ) : (
          <>
            {/* 桌面表格 */}
            <div class="hidden overflow-x-auto rounded-box md:block">
              <table class="table table-sm">
                <thead>
                  <tr>
                    <th>时间</th>
                    <th>账号</th>
                    <th>状态</th>
                    <th>结论</th>
                  </tr>
                </thead>
                <tbody>
                  {runs.value.map((r) => (
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
              {runs.value.map((r) => (
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
          {countdownCard()}
          {statsRow()}
          {prizeCard()}
          {formCard()}
          {runCard()}

          <p class="flex items-start gap-2 text-xs opacity-60">
            <IconCloud class="mt-0.5 size-4 shrink-0" />
            <span class="break-words">
              云朵与抢兑规则见
              <code class="mx-1 rounded bg-base-200 px-1 py-0.5">docs/business-feature-inventory.md</code>
              §exchange；命令行那条路也还在：
              <code class="mx-1 rounded bg-base-200 px-1 py-0.5">
                mcloud signin --exchange --prize 奖品
              </code>
            </span>
          </p>
        </>
      ) : null}

      <ConfirmDialog
        id={CONFIRM_ID}
        title="确认兑换"
        confirmLabel="确认兑换"
        busy={submitting.value}
        onConfirm={() => void doSubmit()}
        impact={
          <div class="flex flex-col gap-1">
            <p>
              {/* ⚠ 这一行**不能换行**：JSX 会把换行折成空格，中文里
                  「个账号 （A、B） 兑换」那些空格很显眼。括号由这里独占给出。 */}
              将对 <b>{impactText.value.count}</b> 个账号{impactText.value.names ? `（${impactText.value.names}）` : ''}兑换 <b>{impactText.value.prize}</b>。
            </p>
            {impactText.value.count === 0 ? (
              /* 0 账号不是"留空 = 全部"的常态，是**必然失败**的一单，得说破。 */
              <p class="text-warning">
                当前没有任何云盘账号，这一单不会成功 —— 先去「账号」页添加。
              </p>
            ) : null}
            {/* ⚠ 别把 markdown 的 `**粗体**` 写进 HTML —— 它不会渲染，只会原样显示星号。 */}
            <p class="text-error">兑换会<b>真实消耗云朵</b>，且不可撤销。</p>
            <p class="opacity-70">
              {impactText.value.skipWait
                ? '已勾选「跳过等待」：不等开抢窗口，立刻下单（仅在测试时使用）。'
                : '会等到整点开抢窗口再下单。'}
            </p>
          </div>
        }
      />
    </div>
  )
}
