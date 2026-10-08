/**
 * 上游状态 —— **按需**查看上游账上的读数。
 *
 * # 这一页为什么存在（用户口径 2026-10-07）
 *
 * 原话：「这个表单独放一个专门查看上游各种状态的页面，**需要主动获取**，而不是跟着其他
 * 任务一起，比如云朵历史，待领取奖励，签到历史等等都可以放这个页面」✓。
 *
 * 两个要点，都写死在这一页的写法里：
 *
 * 1. **按需取**：进页面**什么都不拉**，每一格各自一个「拉取」按钮 ⇒ 点一下才去上游 ✓。
 *    这一条是冲着"以前只能跑一条只读任务、再去历史里翻"那个模型来的 ✗ ——
 *    任务是要**跑出结果**的，只读盘点不是任务（用户原话：「app-task-matrix 这个任务
 *    本身就没有意义」✓）。
 * 2. **不落运行历史**：后端走 `Runner::http_for` 而**不**走 `Runner::run`
 *    （见 `crates/mcloud-server/src/routes/upstream.rs` 模块头 ✓）⇒ 这一页取多少次
 *    都不会在历史里堆出一串"任务记录" ✓。
 *
 * # 这些数是什么
 *
 * 全是**上游账上的读数**，与本仓的运行记录无关：后者是"我们做过什么"，
 * 前者是"上游那边现在是什么样"，两者对不上是常态 ✓（`api.ts` 那一族注释写着同一句）。
 *
 * # 账号是**单选**
 *
 * 这些票/会话都是**按账号**建的（不能共用 ✓），而且一次看一个号才有意义
 * ⇒ 用 [`Select`] 单选，不是任务页那个多选账号器 ✓。
 */

import { onMounted, ref, useSlots } from '@asunajs/lite'

import {
  getBeanRecords,
  getUpstreamAppTaskList,
  getUpstreamPendingPrizes,
  getUpstreamSigninHistory,
  listAccounts,
  type AccountSummary,
  type BeanRecords,
  type UpstreamAppTaskList,
  type UpstreamPendingPrizes,
  type UpstreamSignInHistory,
} from '../api'
import { AppTaskListView } from '../ui/app-task-list'
import { EmptyState } from '../ui/empty-state'
import { IconChevronDown, IconClose } from '../ui/icons'
import { setPageRefresh } from '../ui/page-refresh'
import { Select } from '../ui/select'
import { fmtEpochMs } from '../ui/time'

/** 一格的状态机。**每一格各一份** —— 一格读不到不该把别的格打没 ✓。 */
type CellState = 'idle' | 'loading' | 'ready' | 'error'

/**
 * 一格的骨架：**可折叠**的标题 + 「拉取」按钮 + 内容。
 *
 * ⚠ 「拉取」按钮**不能**放进 `<summary>`：点 `<summary>` 里的按钮会**连带**把这一格
 * 折起来（浏览器对 summary 的激活行为不区分是不是按钮 ✗）⇒ 按钮放在内容区顶部 ✓。
 *
 * ⚠ 内容走**默认插槽**而不是 `props.children` —— lite 下 JSX 子节点编译成第三个实参，
 * `props.children` 永远是 undefined ✗（`ui/dialog.tsx` 文件头写着同一条 ✓）。
 */
const Cell = (props: {
  title: string
  hint: string
  busy: boolean
  onLoad: () => void
}) => {
  const slots = useSlots()
  /**
   * 这一格折不折。
   *
   * ⚠ 自己记一份：`open` 是**属性**，重渲染会照 `props` 重写一遍 ⇒ 不记的话
   * "用户折起来的格子"会在任何一次重渲染时自己弹开 ✗（`exchange-page.tsx` 同款 ✓）。
   * 默认**展开** —— 折起来只是"我看够了"，不该是默认把内容藏起来 ✓。
   */
  const open = ref(true)
  return (
    <details
      open={open.value}
      onToggle={(e) => (open.value = (e.target as HTMLDetailsElement).open)}
      class="disclosure card bg-base-100 shadow-sm"
    >
      {/*
        ⚠⚠ **不能用 `disclosure-arrow`**（2026-10-07 用户截图：箭头跑到框外面去了 ✗）。
        那一档的箭头是 `summary::after` + `float: right`（`app.css:203-216`），而它只对
        **行盒**有效：标题这一块是 flex/两行 ⇒ 浮动元素没地方放，被挤到 summary 之外 ✗。
        ⇒ 这里改成**显式元素**：summary 里套一个 `flex justify-between`，箭头自己画、
        自己按开合转 —— 位置与状态都不再依赖那条浮动规则 ✓。
      */}
      <summary class="disclosure-title">
        <span class="flex items-start justify-between gap-2">
          <span class="flex min-w-0 flex-col">
            <span class="text-base font-medium">{props.title}</span>
            <span class="text-xs font-normal opacity-60">{props.hint}</span>
          </span>
          <IconChevronDown
            class={`size-4 shrink-0 opacity-60 transition-transform ${open.value ? '' : '-rotate-90'}`}
          />
        </span>
      </summary>
      <div class="disclosure-content flex flex-col gap-3">
        <div class="flex justify-end">
          <button
            type="button"
            class="btn btn-sm btn-primary"
            disabled={props.busy}
            onClick={props.onLoad}
          >
            {props.busy ? '正在取…' : '拉取'}
          </button>
        </div>
        {slots.default?.()}
      </div>
    </details>
  )
}

/** 三格共用的状态行（还没取 / 读不到）。 */
const CellStatus = (props: { state: CellState; error: string; idle: string }) => (
  <div class="flex flex-col gap-1">
    {props.state === 'idle' ? <p class="text-sm opacity-70">{props.idle}</p> : null}
    {props.state === 'error' ? (
      <p class="break-words text-sm text-error">{props.error}</p>
    ) : null}
  </div>
)

// ══════════════════════════ ① App 任务清单 ══════════════════════════

const AppTaskCell = (props: { account: string }) => {
  const state = ref<CellState>('idle')
  const error = ref('')
  const data = ref<UpstreamAppTaskList | null>(null)

  const load = async () => {
    state.value = 'loading'
    error.value = ''
    try {
      data.value = await getUpstreamAppTaskList(props.account)
      state.value = 'ready'
    } catch (e) {
      error.value = e instanceof Error ? e.message : '取不到'
      state.value = 'error'
    }
  }

  /**
   * ⚠ 交给子组件的那一份（`null` 时给一份空的 ✓）。
   *
   * 不能靠在 JSX 里"再调一次 `data.value` 并指望 TS 收窄"——那是两次读，
   * 中间没有类型上的联系 ✗。
   */
  const view = (): UpstreamAppTaskList =>
    data.value ?? {
      account: props.account,
      fetchedAtMs: 0,
      fetches: 0,
      succeeded: 0,
      rawCount: 0,
      dedupedCount: 0,
      stateCounts: { done: 0, waiting: 0, unknown: 0 },
      items: [],
      failures: [],
    }

  return (
    <Cell
      title="App 任务清单"
      hint="2 个市场 × 8 个 group 各拉一次，去重后就是这张表（只读）"
      busy={state.value === 'loading'}
      onLoad={() => void load()}
    >
        <CellStatus
          state={state.value}
          error={error.value}
          idle="还没取。点「拉取」现去上游取一份（16 发只读请求）。"
        />
        {state.value === 'ready' ? (
          <div class="flex flex-col gap-2">
            <span class="text-xs opacity-60">
              取于 {fmtEpochMs(view().fetchedAtMs)}
            </span>
            {view().failures.length > 0 ? (
              <p class="break-words text-xs text-warning">
                ⚠ 有 {view().failures.length} 组没读到 ⇒ 这份清单**少了几组**里的条目
                （不是"表里没有"）：{view().failures.join('；')}
              </p>
            ) : null}
            <AppTaskListView matrix={view()} />
          </div>
        ) : null}
    </Cell>
  )
}

// ══════════════════════════ ② 签到历史 ══════════════════════════

const SignInCell = (props: { account: string }) => {
  const state = ref<CellState>('idle')
  const error = ref('')
  const data = ref<UpstreamSignInHistory | null>(null)

  const load = async () => {
    state.value = 'loading'
    error.value = ''
    try {
      data.value = await getUpstreamSigninHistory(props.account)
      state.value = 'ready'
    } catch (e) {
      error.value = e instanceof Error ? e.message : '取不到'
      state.value = 'error'
    }
  }

  /** 只画**本月**那些格子：上下月的补位格不算今天，画出来只会让人数错 ✓。 */
  const monthDays = (): UpstreamSignInHistory['days'] =>
    (data.value?.days ?? []).filter((d) => d.currentMonth)

  return (
    <Cell
      title="签到历史"
      hint="上游 `infoV3`：当月日历 + AI豆中心可做的事（同一发请求，只读）"
      busy={state.value === 'loading'}
      onLoad={() => void load()}
    >
        <CellStatus
          state={state.value}
          error={error.value}
          idle="还没取。点「拉取」现去上游取一份。"
        />
        {state.value === 'ready' ? (
          <div class="flex flex-col gap-3">
            <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs opacity-70">
              <span>余额 {data.value?.total ?? 0} 豆</span>
              <span>连续 {data.value?.signCount ?? 0} 天</span>
              <span>本月已签 {data.value?.monthDays ?? 0} 天</span>
              <span>今天可得 {data.value?.signInPoints ?? 0} 豆</span>
              {data.value?.toReceive != null ? (
                <span class="text-warning">待领取 {data.value.toReceive} 豆</span>
              ) : null}
              <span>取于 {fmtEpochMs(data.value?.fetchedAtMs ?? 0)}</span>
            </div>
            {/*
              AI豆中心可做的事 —— 与上面的日历**同一发 `infoV3`** ✓。
              这一块以前是一条独立任务（`task-list`），2026-10-07 用户口径
              「这个不是说没用吗」⇒ 摘掉那条任务、并进这一格 ✓。
            */}
            <div class="flex flex-col gap-1">
              <span class="text-xs opacity-60">AI豆中心可做的事</span>
              {data.value?.multipleAvailable === true ? (
                <span class="badge badge-sm badge-warning w-fit">签到翻倍机会可用</span>
              ) : null}
              {(data.value?.receiveList.length ?? 0) === 0 ? (
                <p class="text-sm opacity-70">当下没有可领的账目。</p>
              ) : (
                <div class="flex flex-col gap-1">
                  {(data.value?.receiveList ?? []).map((it) => (
                    <div
                      key={`${it.cloudType}-${it.recordId ?? ''}-${it.cloudNum}`}
                      class="flex items-center justify-between gap-2 rounded-box bg-base-200 px-3 py-2"
                    >
                      <span class="min-w-0 break-words text-sm">{it.label}</span>
                      <span class="shrink-0 text-sm tabular-nums text-success">
                        +{it.cloudNum} 豆
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
            {monthDays().length === 0 ? (
              <p class="text-sm opacity-70">上游没给当月日历（`cal` 是空的）。</p>
            ) : (
              <div class="flex flex-wrap gap-1">
                {monthDays().map((d) => (
                  <span
                    key={String(d.day)}
                    class={`badge badge-sm ${d.signed ? 'badge-success' : 'badge-ghost'}`}
                    title={d.signed ? `第 ${d.day} 天：已签` : `第 ${d.day} 天：没签`}
                  >
                    {d.day}
                    {d.today ? '·今' : ''}
                  </span>
                ))}
              </div>
            )}
          </div>
        ) : null}
    </Cell>
  )
}

// ══════════════════════════ ③ 待领取奖品 ══════════════════════════

const PrizeCell = (props: { account: string }) => {
  const state = ref<CellState>('idle')
  const error = ref('')
  const data = ref<UpstreamPendingPrizes | null>(null)

  const load = async () => {
    state.value = 'loading'
    error.value = ''
    try {
      data.value = await getUpstreamPendingPrizes(props.account)
      state.value = 'ready'
    } catch (e) {
      error.value = e instanceof Error ? e.message : '取不到'
      state.value = 'error'
    }
  }

  const pending = (): UpstreamPendingPrizes['records'] =>
    (data.value?.records ?? []).filter((r) => r.pending)

  return (
    <Cell
      title="待领取奖品"
      hint="上游 `signin/public/record`（只读，只查第一页 10 条）"
      busy={state.value === 'loading'}
      onLoad={() => void load()}
    >
        <CellStatus
          state={state.value}
          error={error.value}
          idle="还没取。点「拉取」现去上游取一份。"
        />
        {state.value === 'ready' ? (
          <div class="flex flex-col gap-2">
            <span class="text-xs opacity-70">
              待领取 {data.value?.pendingCount ?? 0} 个 · 这一页共{' '}
              {data.value?.records.length ?? 0} 条 · 取于 {fmtEpochMs(data.value?.fetchedAtMs ?? 0)}
            </span>
            {pending().length === 0 ? (
              <p class="text-sm opacity-70">这一页没有待领取的奖品。</p>
            ) : (
              <div class="flex flex-col gap-1">
                {pending().map((r) => (
                  <div
                    key={r.oid}
                    class="flex items-start justify-between gap-2 rounded-box bg-base-200 px-3 py-2"
                  >
                    <span class="min-w-0 break-words text-sm">{r.prizeName}</span>
                    <span class="shrink-0 text-xs opacity-60">
                      {r.expireTime != null ? `过期 ${r.expireTime}` : '没有过期时间'}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ) : null}
    </Cell>
  )
}

// ══════════════════════════ ④ 云朵记录（AI豆流水） ══════════════════════════

/** 窗口档位。**上游那一口按自然日取**（`days = 0` 是全部 ✓）。 */
const WINDOWS: { days: number; label: string }[] = [
  { days: 1, label: '今日' },
  { days: 7, label: '近 7 天' },
  { days: 30, label: '近 30 天' },
  { days: 0, label: '全部' },
]

const CloudCell = (props: { account: string }) => {
  const state = ref<CellState>('idle')
  const error = ref('')
  const data = ref<BeanRecords | null>(null)
  const days = ref(1)

  const load = async () => {
    state.value = 'loading'
    error.value = ''
    try {
      data.value = await getBeanRecords(props.account, days.value)
      state.value = 'ready'
    } catch (e) {
      error.value = e instanceof Error ? e.message : '取不到'
      state.value = 'error'
    }
  }

  /** 窗口 + 拉取**一次点完**（换档位本来就要重取 ✓）。 */
  const pick = (d: number) => {
    days.value = d
    void load()
  }

  return (
    <Cell
      title="云朵记录（AI豆流水）"
      hint="上游 `cloudRecordV2` 的逐笔流水（只读）"
      busy={state.value === 'loading'}
      onLoad={() => void load()}
    >
        <div class="join w-fit">
          {WINDOWS.map((w) => (
            <button
              key={String(w.days)}
              type="button"
              class={`btn btn-xs join-item ${days.value === w.days ? 'btn-primary' : 'btn-soft'}`}
              onClick={() => pick(w.days)}
            >
              {w.label}
            </button>
          ))}
        </div>
        <CellStatus
          state={state.value}
          error={error.value}
          idle="还没取。选一个窗口（或点「拉取」）现去上游取一份。"
        />
        {state.value === 'ready' ? (
          <div class="flex flex-col gap-2">
            <span class="text-xs opacity-70">
              共 {data.value?.records.length ?? 0} 笔
              {data.value?.hasMore === true ? '（还有没读到的，这一份不全）' : ''}
              {data.value != null && data.value.undated > 0
                ? ` · 其中 ${data.value.undated} 笔时间读不出`
                : ''}
              {' · '}取于 {fmtEpochMs(data.value?.nowMs ?? 0)}
            </span>
            {(data.value?.records.length ?? 0) === 0 ? (
              <p class="text-sm opacity-70">这个窗口里没有流水。</p>
            ) : (
              <div class="flex flex-col gap-1">
                {(data.value?.records ?? []).map((r) => (
                  <div
                    key={`${r.atMs ?? r.rawTime ?? ''}-${r.summary}-${r.num}`}
                    class="flex items-start justify-between gap-2 rounded-box bg-base-200 px-3 py-2"
                  >
                    <div class="flex min-w-0 flex-col">
                      <span class="break-words text-sm">{r.summary}</span>
                      <span class="text-xs opacity-60">
                        {r.atMs != null ? fmtEpochMs(r.atMs) : (r.rawTime ?? '时间读不出')}
                        {r.mark !== '' ? ` · ${r.mark}` : ''}
                      </span>
                    </div>
                    <span
                      class={`shrink-0 text-sm tabular-nums ${r.num < 0 ? 'text-error' : 'text-success'}`}
                    >
                      {r.num > 0 ? `+${r.num}` : String(r.num)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ) : null}
    </Cell>
  )
}

// ══════════════════════════ 页面 ══════════════════════════

export interface UpstreamPageProps {
  /** 回账号页（它是**账号页的子视图**，不是独立导航项 ✓）。 */
  onExit: () => void
}

export const UpstreamPage = (props: UpstreamPageProps) => {
  const state = ref<'loading' | 'ready' | 'error'>('loading')
  const errorMessage = ref('')
  const accounts = ref<AccountSummary[]>([])
  const picked = ref('')

  const load = async () => {
    try {
      accounts.value = await listAccounts()
      // 默认选第一个 —— 但**不**顺手去拉任何一格：这一页的规矩是"点了才取" ✓。
      if (picked.value === '' && accounts.value.length > 0) {
        picked.value = accounts.value[0].id
      }
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

  /** 选中的账号（没选就空串 —— 四格各自会以空账号请求并如实报错 ✓）。 */
  const account = (): string => picked.value

  return (
    <div class="mx-auto flex w-full max-w-7xl flex-col gap-4">
      {/*
        ⚠ 自己写一行标题：外壳那个 sticky navbar 显示的是 `navItem('accounts').title`
        （=「账号」✗）—— 这一页是账号页的**子视图**，不写的话用户不知道自己在哪一层 ✓。
        与 `account-config-page.tsx` 同款（返回按钮 + h1 + 一句说明 ✓）。
      */}
      <div class="flex flex-wrap items-center gap-2">
        <button
          type="button"
          class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
          aria-label="回账号页"
          onClick={() => props.onExit()}
        >
          <IconClose class="size-4" />
        </button>
        <h1 class="text-base font-medium">上游状态</h1>
        <span class="text-xs opacity-60">上游账上的读数（按需取）</span>
      </div>
      <div class="flex flex-col gap-1">
        <p class="text-sm opacity-70">
          这一页读的都是**上游账上的读数**，不是本仓的运行记录。每一格都要点「拉取」
          才去取一次 —— 不自动轮询、也不跟着别的任务跑。
        </p>
      </div>

      {state.value === 'error' ? (
        <div class="alert alert-error">
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' && accounts.value.length === 0 ? (
        <EmptyState
          title="还没有已登录的账号"
          hint="这些读数都是按账号取的（票与会话按账号建，不能共用）—— 先去「账号」页登录一个。"
        />
      ) : null}

      {state.value === 'ready' && accounts.value.length > 0 ? (
        <div class="flex flex-col gap-4">
          <div class="flex flex-wrap items-center gap-2">
            <span class="text-sm opacity-70">账号</span>
            <Select
              id="upstream-account"
              class="w-56"
              filterable
              ariaLabel="账号"
              placeholder="请选择账号"
              value={picked.value}
              options={accounts.value.map((a) => ({
                value: a.id,
                label: a.nickname || a.label || a.id,
              }))}
              onChange={(v) => (picked.value = v)}
            />
          </div>

          <AppTaskCell account={account()} />
          <SignInCell account={account()} />
          <PrizeCell account={account()} />
          <CloudCell account={account()} />
        </div>
      ) : null}
    </div>
  )
}
