import { accountName, refreshAccountNames } from '../account-names'
import { hashPath } from '../ui/nav'
/**
 * 任务中心：选账号 → 运行任务 → 实时看日志。
 *
 * 移动端形态（规范 §7.2）：参数表单全宽，卡片流。
 *
 * 长任务走 SSE 实时日志；提交期间只禁用提交按钮，**页面其余部分保持可操作**，
 * 并明确告诉用户可以关页面（任务在后端继续跑）。
 */

import { onUnmounted, onMounted, ref } from 'lite'
import {
  cancelRun,
  listAccounts,
  getTaskConfig,
  listCapabilities,
  startRun,
  subscribeRun,
  type AccountSummary,
  type RunEvent,
  type TaskMeta,
  type TaskGroup,
  request,
} from '../api'
import { TaskConfigPage } from './task-config-page'
import { AccountPicker } from '../ui/account-picker'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { LogPanel, type LogLine } from '../ui/log-panel'

type LoadState = 'loading' | 'error' | 'ready'

/**
 * 签到族（P1）那八条。
 *
 * 它们共用**同一条 H5 会话**（签到页 `newsignin`），一起验收最省事：
 * 跑通 `sign` 之后其余几条基本只是换请求。
 */

/**
 * 当前选中的分组（tab）。
 *
 * ⚠ 放在**模块级**而不是组件里：切到别的页再回来时，用户不该被丢回第一个 tab。
 * 也只是会话内记住，不落盘 —— 这点偏好不值得占一个存储位置。
 */
const activeGroup = ref<string>('')

export const TasksPage = () => {
  /**
   * 当前 hash 的路径段。
   *
   * ⚠ **本页自己盯着 hash**，而不是让路由表来管：`parseHash` 把
   * `#/tasks/x/config` 与 `#/tasks` 都归到「任务」这一页（配置页不是独立导航项），
   * 于是从列表切到配置时 PageId **没变**，`app.tsx` 那条动态 import 不会再跑一次。
   * 想让它工作只有两条路：把配置页做成第 10 条导航项（规范里 9 条封顶，不行），
   * 或者本页自己听 hash 换视图（就是这里）。切换时列表**不会**被卸载重建 ——
   * 用户从配置页点回来，看到的还是原来那份列表和滚动位置。
   */
  const path = ref<string[]>(hashPath(location.hash))
  const onHash = () => {
    path.value = hashPath(location.hash)
  }

  /** 配置视图对应哪个任务；不在配置子路由上就是 `null`。 */
  const configTask = (): string | null => {
    const seg = path.value
    if (seg[0] !== 'tasks' || seg[2] !== 'config') return null
    return decodeURIComponent(seg[1] ?? '')
  }

  const state = ref<LoadState>('loading')
  /** 加载失败（整页替换）。**留在页内** —— 它要配「重试」，弹窗会挡住入口。 */
  const errorMessage = ref('')

  const tasks = ref<TaskMeta[]>([])
  /** 已登录的云盘账号。跑任务要选一个（或多个），而不是手打手机号。 */
  const accounts = ref<AccountSummary[]>([])
  /**
   * 已勾选的账号 id。
   *
   * ⚠ 「全部账号」用**单独的开关** `allAccounts` 表达，而不是"把每一项都勾上"：
   * 后端的空数组语义是"**我名下的全部账号**"（与定时同一口径），以后加了账号也仍然成立。
   */
  const picked = ref<string[]>([])
  /** `true` = 全部账号 ⇒ 请求发 `accounts: []`。 */
  const allAccounts = ref(false)
  const running = ref(false)
  const lines = ref<LogLine[]>([])
  /** 已用时长，1 秒一跳；用户要能看到"还在动"。 */
  const elapsedMs = ref(0)
  /**
   * 本次点击涉及的全部 run id（一个账号一条）。
   *
   * 用户口径（2026-10-01）：「任何（包括直播小红花）应该允许配置多账号/全部，
   * 点击运行后后端保证串行执行」⇒ 后端**串行**跑，但每条 run 是独立记录、独立事件流，
   * 因此这里是一组 id：订阅要逐条订，中止也要逐条发（只中止第一个的话，其余账号照跑）。
   */
  const currentRunIds = ref<number[]>([])
  /** 已经结束的 run 数：多账号时**全部**结束才算这一批跑完。 */
  const finishedRuns = ref(0)
  /** 正在跑哪个任务（用于把卡片上的按钮变成"执行中"）。 */
  const runningTask = ref<string | null>(null)
  /** 本次运行的终局结论（跑完后显示在日志上方）。 */
  const outcome = ref<{ ok: boolean; text: string } | null>(null)
  /** 已发出中止请求，等待任务在安全点退出。 */
  const cancelling = ref(false)

  let stopStreams: (() => void)[] = []
  let ticker: number | null = null
  let startedAt = 0

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [caps, accts] = await Promise.all([listCapabilities(), listAccounts()])
      tasks.value = caps.tasks.filter((t) => !t.hidden)
      accounts.value = accts
      // 只有一个账号时**预选**它：少一次点击，而且"就是那个号"没有歧义。
      // 多个账号时不猜 —— 猜错就是拿另一个号跑了一遍真任务。
      if (accts.length === 1 && picked.value.length === 0 && !allAccounts.value) {
        picked.value = [accts[0].id]
      }
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  const push = (kind: LogLine['kind'], text: string) => {
    lines.value = [...lines.value, { kind, text, at: Date.now() }]
  }

  const stopTicker = () => {
    if (ticker !== null) {
      clearInterval(ticker)
      ticker = null
    }
  }

  const finish = () => {
    running.value = false
    cancelling.value = false
    currentRunIds.value = []
    finishedRuns.value = 0
    runningTask.value = null
    stopTicker()
    // EventSource 断线会自动重连，必须显式关掉（每条 run 一条流）
    for (const stop of stopStreams) stop()
    stopStreams = []
  }

  /**
   * 请求中止本次点击涉及的全部运行。
   *
   * 中止是**协作式**的：后端只置标志，任务在安全点自己退出。
   * 所以这里不立刻把界面改成"已停止"，而是显示"正在中止"，
   * 等真正的 finished 事件到达才收尾 —— 否则界面说停了、任务还在跑。
   *
   * ⚠ 多账号时**逐条**发：只中止第一个的话其余账号照跑。
   * 还没轮到的账号会带着已取消的令牌轮到自己，在安全点直接收尾（不会白跑一遍）。
   */
  const cancel = async () => {
    const ids = currentRunIds.value
    if (ids.length === 0) return
    cancelling.value = true
    try {
      const results = await Promise.all(ids.map((id) => cancelRun(id)))
      if (!results.some((r) => r.cancelled)) {
        // 任务刚好跑完 —— 正常竞态，不是错误
        push('warn', '任务已经结束，无需中止')
        cancelling.value = false
        return
      }
      push(
        'warn',
        ids.length > 1
          ? `已请求中止 ${ids.length} 个账号，等待各自在安全点退出…`
          : '已请求中止，等待任务在安全点退出…',
      )
    } catch (e) {
      showError('中止失败', e instanceof Error ? e.message : '中止请求失败')
      cancelling.value = false
    }
  }

  /**
   * 一条 run 的事件回调。
   *
   * ⚠ 多账号时每条流都带**自己的账号前缀**：日志是同一个面板，
   * 不带前缀两三个账号的进度会交错成一团，看不出谁是谁。
   */
  const onEventFor = (accountLabel: string) => (ev: RunEvent) => {
    const tag = currentRunIds.value.length > 1 ? `[${accountLabel}] ` : ''
    const say = (kind: LogLine['kind'], text: string) => push(kind, tag + text)
    switch (ev.type) {
      case 'started':
        say('info', `任务开始：${ev.task} / ${accountName(ev.account)}`)
        break
      case 'log':
        say(
          ev.level === 'error' ? 'error' : ev.level === 'warn' ? 'warn' : 'info',
          ev.message,
        )
        break
      case 'progress':
        say('info', `[${ev.step}/${ev.total}] ${ev.label}`)
        break
      case 'finished': {
        // 三种结局分别显示：失败要说清原因，跳过要说清**为什么跳过**
        // （跳过不是失败，不该染红）。
        const o = ev.outcome
        const text =
          o.status === 'success' ? o.summary : o.status === 'skipped' ? o.reason : o.reason
        outcome.value = { ok: o.status === 'success', text }
        say(
          o.status === 'failed' ? 'error' : 'info',
          `结束（${ev.duration_ms}ms）：${o.status === 'skipped' ? `跳过 —— ${text}` : text}`,
        )
        finishedRuns.value += 1
        // 多账号时**全部**结束才算这一批跑完；否则继续等队列里的下一个
        // （后端是串行的，所以此刻剩下的账号其实是"排队中"）。
        if (finishedRuns.value >= currentRunIds.value.length) {
          finish()
        } else {
          say('info', `还剩 ${currentRunIds.value.length - finishedRuns.value} 个账号排队中`)
        }
        break
      }
      default:
        // 后端新增事件类型时，旧前端不该整页报错 —— 忽略即可
        break
    }
  }

  const run = async (task: string) => {
    lines.value = []
    // 「全部账号」在协议上是 `accounts: []`（与定时同一口径）
    const ids = allAccounts.value ? [] : picked.value
    if (!allAccounts.value && ids.length === 0) {
      // 没选账号就点运行 —— 这是**当前动作的直接答案**，弹窗告知。
      // 页内那行提示在表单上方，卡片列表拉长后用户根本看不到。
      showError(
        '请先选择账号',
        '任务需要一个已登录的账号才能跑。请在上方「账号」里勾选，或勾上「全部账号」。',
      )
      return
    }

    running.value = true
    runningTask.value = task
    outcome.value = null
    startedAt = Date.now()
    elapsedMs.value = 0
    ticker = window.setInterval(() => {
      elapsedMs.value = Date.now() - startedAt
    }, 1000)

    try {
      // 参数从「任务配置」那一页取（用户口径：任务页保持简单，参数单独一页）。
      // 每次运行现拉一次，而不是在内存里存一份 —— 后者会出现"配置页改了、
      // 这次运行还用着旧参数"，而那种不一致没有任何迹象。
      const cfg = await getTaskConfig(task)
      // 后台执行：**立刻**拿到 run id（多账号时是一组），这样才能订阅事件并请求中止。
      // 同步执行的话请求还没返回，用户根本没有可中止的时机。
      const resp = await startRun(task, ids, cfg.params, true)
      // ⚠ 必须容忍**老后端**（只有 `run_id`/`account`，没有这组新字段）：
      // 门禁的夹具就是老形状，而且升级顺序上"前端先上、后端后上"也真实存在。
      const runIds = resp.run_ids?.length ? resp.run_ids : [resp.run_id]
      const labels = resp.accounts?.length ? resp.accounts : [resp.account]
      currentRunIds.value = runIds
      finishedRuns.value = 0
      // 先订阅再提示：订阅会把已发布的事件补进队列，
      // 因此"启动到订阅"之间的事件不会丢。**每条 run 各订一条**。
      for (const stop of stopStreams) stop()
      stopStreams = runIds.map((id, i) =>
        subscribeRun(id, { onEvent: onEventFor(accountName(labels[i] ?? '')) }),
      )
      push(
        'info',
        ids.length === 0
          ? `已提交 ${runIds.length} 个账号：${labels.map(accountName).join('、')} · 后端按顺序一个一个跑`
          : `已提交，运行号 ${resp.run_id}`,
      )
    } catch (e) {
      showError(`无法启动「${task}」`, e instanceof Error ? e.message : '请求失败')
      finish()
    }
  }

  onMounted(() => {
    // 编排那一格的**计数**要一进来就有 ✓（不点它也是 0 会让人以为没有编排）。
    void loadPipelines()
    window.addEventListener('hashchange', onHash)
    // 先把"id → 名字"填上，否则运行记录里显示的是一串哈希
    void refreshAccountNames()
    void load()
  })
  onUnmounted(() => {
    window.removeEventListener('hashchange', onHash)
    stopTicker()
    for (const stop of stopStreams) stop()
  })

  const elapsedText = () => `${(elapsedMs.value / 1000).toFixed(0)} 秒`

  const header = () => (
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

  const form = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <h2 class="card-title text-base">运行参数</h2>
        <fieldset class="fieldset">
          <legend class="fieldset-legend">账号</legend>
          {accounts.value.length === 0 ? (
            // 没账号时**给出下一步**，而不是一个会失败的输入框：
            // 手打手机号也没用 —— 任务要的是本地已登录的那份凭据。
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <span>
                还没有已登录的账号。请先到「账号」页登录一个云盘账号，再回来跑任务。
              </span>
            </div>
          ) : (
            <div class="flex flex-col gap-2">
              {/*
                账号选择器（任务页与直播页共用同一个组件）。
                用户口径（2026-10-01）：「任何（包括直播小红花）应该允许配置多账号/全部，
                点击运行后后端保证串行执行」——「全部」在后端是 `accounts: []`。
              */}
              <AccountPicker
                accounts={accounts.value}
                all={allAccounts.value}
                picked={picked.value}
                onToggleAll={() => (allAccounts.value = !allAccounts.value)}
                onToggle={(id) =>
                  (picked.value = picked.value.includes(id)
                    ? picked.value.filter((x) => x !== id)
                    : [...picked.value, id])
                }
              />
              <p class="text-xs opacity-60">
                勾多个账号时，后端按顺序一个一个跑（一个跑完再跑下一个）。
                勾「全部账号」= 你名下的所有账号，以后加了账号也算在内。
              </p>
            </div>
          )}
        </fieldset>
        {running.value ? (
          <div class="flex flex-wrap items-center justify-between gap-2">
            <span class="text-xs opacity-70">
              执行中，已用 {elapsedText()}
              {currentRunIds.value.length > 1
                ? ` · 已完成 ${finishedRuns.value}/${currentRunIds.value.length} 个账号（按顺序串行）`
                : ''}
              {' '}· 可关闭页面，任务在后端继续
            </span>
            <button
              type="button"
              class="btn btn-error btn-sm min-h-11 md:min-h-0"
              disabled={cancelling.value}
              onClick={() => void cancel()}
            >
              {cancelling.value ? '正在中止…' : '中止'}
            </button>
          </div>
        ) : null}
      </div>
    </div>
  )

  /**
   * 分组的**展示顺序**。
   *
   * ⚠ 这只是"先看哪一组"的偏好，不是语义映射 —— 分组名由后端的 `groupLabel` 给。
   * 后端要是加了新组，这里没列到也会被排到末尾显示（不会消失、更不会报错）。
   */
  const GROUP_ORDER: TaskGroup[] = ['signin', 'live', 'app', 'device']

  /**
   * 「编排」这一格**不是**任务组 —— 它来自另一份数据（`/api/pipelines`）✓。
   *
   * 用 `__` 前缀的伪键，与后端给的 `TaskMeta.group`（`signin`/`live`/…）不会撞 ✓。
   * ⚠ 这里只做**总览**：运行/编辑/新建留在编排页 ✓ —— 两页各写一份运行逻辑迟早分叉
   * （这页已有「运行参数 · 账号」，那边也有账号选择器 ✓，已经记着统一到
   * `ui/account-picker` ✓）。给一个明确入口就够，不再复制一套 ✓。
   */
  const PIPELINES_TAB = '__pipelines'
  type PipelineRow = {
    id: string
    name: string
    steps: unknown[]
    enabled: boolean
    lastRunMs?: number | null
  }
  const pipelineRows = ref<PipelineRow[]>([])

  const loadPipelines = async () => {
    try {
      pipelineRows.value = await request<PipelineRow[]>('/api/pipelines')
    } catch (e) {
      showError('读不到编排列表', e instanceof Error ? e.message : '加载失败')
    }
  }

  /** 按分组切一刀，顺序按 `GROUP_ORDER`，没列到的组按出现顺序接在后面。 */
  const groups = (): { key: string; label: string; items: TaskMeta[] }[] => {
    const buckets = new Map<string, { label: string; items: TaskMeta[] }>()
    for (const t of tasks.value) {
      const key = t.group || 'other'
      const bucket = buckets.get(key) ?? { label: t.groupLabel || '其它', items: [] }
      bucket.items.push(t)
      buckets.set(key, bucket)
    }
    const keys = [...buckets.keys()]
    keys.sort((a, b) => {
      const ia = GROUP_ORDER.indexOf(a as TaskGroup)
      const ib = GROUP_ORDER.indexOf(b as TaskGroup)
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib)
    })
    return keys.map((k) => ({ key: k, label: buckets.get(k)!.label, items: buckets.get(k)!.items }))
  }

  /**
   * 当前要显示的那一组。
   *
   * 没选过、或者选的那一组这次没出现（比如那个任务被摘了）⇒ 落到第一组。
   * 不这样兜底的话，tab 会"一个都不亮、内容一片空白"。
   */
  const currentGroup = (): { key: string; label: string; items: TaskMeta[] } | null => {
    const all = groups()
    if (all.length === 0) return null
    return all.find((g) => g.key === activeGroup.value) ?? all[0]!
  }

  /**
   * 分组导航那一排 tab。
   *
   * ⚠⚠ **必须留在 `taskCards()` 之外** —— 2026-10-02 用户两次报「你的 tabs 呢」，
   * 根因就是这一排原本写在 `taskCards()` 的**最后一个分支**里：一旦切到「编排」
   * （`activeGroup === PIPELINES_TAB`）或任务目录为空，整排**直接不渲染** ⇒
   * 「点了编排就再也切不回去」，只能刷新。
   *
   * 为什么之前没测出来：门禁/夹具进页面时 `activeGroup` 是空串 ⇒ 走的是最后一个分支，
   * tab 排当然在 ✓。用户是**先点了「编排」**才没了 —— 状态相关，夹具没覆盖到。
   *
   * 判据（截图量像素，2026-10-02）：用户那张里这一排的文字暗像素 **0** 个、
   * 卡片间空档只有 **34px**（两个 `gap-4`）；我这边同一位置 **161** 个暗像素、空档 **65px**。
   */
  const groupTabs = () => (
    <div role="tablist" class="join w-full overflow-x-auto" aria-label="任务分组">
      {/* ⚠ 「编排」的数据源与任务组**不同**（`/api/pipelines`）✓ */}
      <button
        type="button"
        role="tab"
        aria-selected={activeGroup.value === PIPELINES_TAB}
        class={`btn join-item btn-sm min-h-11 whitespace-nowrap md:min-h-0 ${activeGroup.value === PIPELINES_TAB ? 'btn-primary' : 'btn-soft'}`}
        onClick={() => {
          activeGroup.value = PIPELINES_TAB
          void loadPipelines()
        }}
      >
        编排
        <span class="ml-1 text-xs opacity-70">{pipelineRows.value.length}</span>
      </button>
      {groups().map((g) => (
        <button
          key={g.key}
          type="button"
          role="tab"
          /*
            ⚠ 在「编排」页时**不能**用 `currentGroup()?.key` 直接比：那个函数带兜底
            （选中的组这次没出现就落到第一组），而 `__pipelines` 不在分组里 ⇒
            它会返回第一组 ⇒ **两个 tab 同时亮**（2026-10-02 探针实测到）。
          */
          aria-selected={
            activeGroup.value === PIPELINES_TAB ? false : currentGroup()?.key === g.key
          }
          class={`btn join-item btn-sm min-h-11 whitespace-nowrap md:min-h-0 ${
            activeGroup.value !== PIPELINES_TAB && currentGroup()?.key === g.key
              ? 'btn-primary'
              : 'btn-soft'
          }`}
          onClick={() => {
            activeGroup.value = g.key
          }}
        >
          {g.label}
          <span class="ml-1 text-xs opacity-70">{g.items.length}</span>
        </button>
      ))}
    </div>
  )

  const taskCards = () =>
    activeGroup.value === PIPELINES_TAB ? (
      <div class="grid gap-3 md:grid-cols-2 lg:grid-cols-3">
        {pipelineRows.value.map((p) => (
          <div key={p.id} class="card bg-base-100 shadow-sm">
            <div class="card-body">
              <h3 class="card-title text-base">
                {p.name}
                <span class="badge badge-soft badge-sm">{p.enabled ? '已启用' : '已停用'}</span>
              </h3>
              <p class="text-sm opacity-70">{p.steps.length} 步</p>
              <div class="card-actions justify-end">
                <button
                  type="button"
                  class="btn btn-soft btn-sm"
                  onClick={() => {
                    location.hash = '#/pipelines'
                  }}
                >
                  打开编排页
                </button>
              </div>
            </div>
          </div>
        ))}
        {pipelineRows.value.length === 0 ? (
          <div class="card bg-base-100 shadow-sm">
            <div class="card-body">
              <EmptyState
                title="还没有编排。"
                hint="编排就是「把多个任务串成一条链」—— 到编排页新建一条。"
                action={
                  <button
                    type="button"
                    class="btn btn-soft"
                    onClick={() => {
                      location.hash = '#/pipelines'
                    }}
                  >
                    去编排页
                  </button>
                }
              />
            </div>
          </div>
        ) : null}
      </div>
    ) : tasks.value.length === 0 ? (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <EmptyState
            title="后端还没有注册任何任务。"
            hint="任务由后端注册表提供，检查后端是否正常启动，或稍后重试。"
            action={
              <button type="button" class="btn btn-soft" onClick={() => void load()}>
                重试
              </button>
            }
          />
        </div>
      </div>
    ) : (
      <div class="grid gap-3 md:grid-cols-2 lg:grid-cols-3">
        {(currentGroup()?.items ?? []).map((t) => (
                <div key={t.name} class="card bg-base-100 shadow-sm">
                  <div class="card-body">
                    <h3 class="card-title text-base">
                      {t.title}
                      {/* 参数不在这一页填（那会把页面撑复杂）：只标出来 + 给个入口 */}
                      {t.params.length > 0 ? (
                        <span class="badge badge-soft badge-sm">可配置</span>
                      ) : null}
                    </h3>
                    <p class="break-words text-sm opacity-70">{t.description}</p>
                    <p class="font-mono text-xs opacity-50">{t.name}</p>
                    <div class="card-actions items-center justify-end gap-2">
                      {t.params.length > 0 ? (
                        <button
                          type="button"
                          class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                          onClick={() => {
                            location.hash = `#/tasks/${encodeURIComponent(t.name)}/config`
                          }}
                        >
                          配置
                        </button>
                      ) : null}
                      <button
                        type="button"
                        class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                        disabled={running.value}
                        onClick={() => void run(t.name)}
                      >
                        {runningTask.value === t.name ? '执行中…' : '运行'}
                      </button>
                    </div>
              </div>
            </div>
          ))}
        </div>
    )

  // 配置子路由：整页换成配置视图（用户口径："给任务出个单独的配置页面"）
  const task = configTask()
  if (task !== null) {
    return (
      <TaskConfigPage
        task={task}
        onBack={() => {
          location.hash = '#/tasks'
        }}
      />
    )
  }

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
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (
        <>
          {form()}
          {outcome.value ? (
            <div
              class={`alert alert-soft text-sm ${
                outcome.value.ok ? 'alert-success' : 'alert-warning'
              }`}
              role="status"
            >
              <span class="break-words">
                {outcome.value.ok ? '✅ ' : '⚠ '}
                {outcome.value.text}
              </span>
            </div>
          ) : null}
          {/*
            ⚠ tab 排在 `taskCards()` **外面**：分组导航必须一直在（切到「编排」也要能切回来）。
            原来的写法把它塞进了 `taskCards()` 的最后一个分支，于是"点了编排就回不去"
            —— 2026-10-02 用户报的「你的 tabs 呢」就是这个。见 `groupTabs()` 的注释。
          */}
          <div class="flex flex-col gap-4">
            {groupTabs()}
            {taskCards()}
          </div>
          {lines.value.length > 0 ? <LogPanel lines={lines.value} /> : null}
        </>
      ) : null}
    </div>
  )
}
