/**
 * 历史与日志（`#/history`）—— 执行结果列表 + 单条运行的日志。
 *
 * # 与「任务中心」的分工
 *
 * 那边是**发起**（`POST /api/runs` 回 202，只给 runId 配对），这边是**结果**：
 * 列表来自 `GET /api/runs`，日志来自 `GET /api/runs/{id}/events`（SSE）。
 * 任务中心只显示所选能力最近 5 次，完整历史在这里 ——
 * 那一页的「最近运行」卡片末尾写着"完整历史在那一页"，指的就是这里。
 *
 * # ⭐ 已经跑完的 run **也**能回放日志
 *
 * `routes.go` 的 `runEvents` 是「先订阅总线、再读库」：库里那份来自
 * `RunEventsAfter`，而 `app/run.go` 的 `runOne` 在**每个账号跑完的边界**把
 * `started` / 全部 `log` / `finished` / `progress` 一整批落库。
 * ⇒ 点一条历史记录，SSE 会把那一趟的日志按 `seq` 顺序**重放**一遍，
 * 最后一条是终局事件 ⇒ 服务端结束响应、前端按契约 `close()`。
 * 这一页因此不需要第二套"读日志"接口，也不需要把日志塞进 `RunRecord`。
 *
 * # 为什么**没有**进度条（§6.3 那条的偏离，理由在这里）
 *
 * §6.3 写的是"进度用 `progress` 或 `radial-progress`"。但后端只在**每个账号
 * 收尾时**发一条 `progress`（`app/run.go:315`），而那一条排在 `finished`
 * **之后** —— 2026-09-23 实拍同一趟的四条事件，序号是
 * `started(1) → errored(2) → finished(3) → progress(4)`。
 * 前端收到 `finished` 就 `close()` 了（那条契约）⇒ 进度条会**永远是 0%**，
 * 比不给更糟。
 * 一批多个账号的"整体进度"在界面上另有表达：每个账号是**独立一行、独立一条流**，
 * 数一下列表里有几条 `进行中` 就是进度。
 * 这一条要动的话得先改后端（在跑的过程中发 `progress`），不是前端的事。
 *
 * # `status` 不是结论
 *
 * 列表徽标是生命周期（`running` / `done` / `failed`），`done` 只表示"跑完了"。
 * 业务结论（全成 / 部分成 / 需人工 / 失败）只在 `finished` 事件里，
 * 所以它渲染在日志卡片上、由 `outcomeLabel` 出，**不**在列表里替后端编一个。
 */

import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue'
import { ApiError, listCapabilities, listRuns, runEventsUrl, type RunRecord } from '../api'
import {
  LIMIT_MAX,
  LIMIT_STEP,
  capabilityOptions,
  cloudGainText,
  elapsedMillis,
  emptyLogText,
  filterRuns,
  manualTargetLabel,
  nextLimit,
  statusBadge,
  statusLabel,
  statusOptions,
} from '../ui/history'
import { IconAlert, IconHistory } from '../ui/icons'
import type { PageId } from '../ui/nav'
import {
  applyEvent,
  formatElapsed,
  isTerminalEvent,
  levelClass,
  markLost,
  newStreamState,
  outcomeBadge,
  outcomeLabel,
  parseRunEvent,
  type RunStreamState,
} from '../ui/run-stream'
import { fmtTime } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/**
 * 日志区的元素 id。用 `document.getElementById` 而不是模板 ref ——
 * 与 `ui/dialog.tsx` 同一个理由（Vapor 下模板 ref 的语义与 VDOM 那套不同，
 * 而这个页面只需要"拿到那个滚动容器"这一件事）。
 */
const LOG_ID = 'history-log'
/** 日志卡片本体的 id：选中一条记录后要把它滚进视野。 */
const LOG_CARD_ID = 'history-log-card'

/** 一条事件都没到过时的重订阅节奏（只有"还在跑"的那一支会走）。 */
const RESUBSCRIBE_MS = 1000
const RESUBSCRIBE_MAX = 30

/** 列表轮询间隔：**只在有 `进行中` 的记录时才跑**（见 [pollTimer] 的注释）。 */
const POLL_MS = 5000
/** 已用时长的刷新节奏（§6.3）。 */
const TICK_MS = 1000

/**
 * "算不算贴着底"的容差（px）。
 *
 * 给 48 而不是 0：一行日志约 20px，滚动位置不可能精确停在底部，
 * 差一行就不跟随的话，跑起来会看着像"卡住了"。
 */
const NEAR_BOTTOM_PX = 48

export interface HistoryPageProps {
  /** 跳别的页面（空态里的"去任务中心"要用）。 */
  onGo?: (id: PageId) => void
}

export const HistoryPage = (props: HistoryPageProps) => {
  const state = ref<LoadState>('loading')
  /** 加载失败（整页换成错误态）。 */
  const errorMessage = ref('')

  const runs = ref<RunRecord[]>([])
  /** 一次要多少条；点"再看 N 条"往上加（上限见 `LIMIT_MAX`）。 */
  const limit = ref(LIMIT_STEP)
  /**
   * 能力码 → 显示名，来自 `/api/capabilities`。
   *
   * ⚠ 这份**不是**关键路径：拿不到就回落成 code（见 `capabilityOptions`），
   * 列表照常可用。所以它的失败被吞掉 —— 为了一个更好看的标签让整页红掉不值。
   * 取 `all` 而不是 `visible`：`exchange` / `auth-refresh` 是 `hidden` 的，
   * 但它们**照样会有运行记录**，那一行也需要中文名。
   */
  const titles = ref<Record<string, string>>({})

  /** 筛选项。空串 = 不筛。 */
  const capFilter = ref('')
  const statusFilter = ref('')

  /** 当前选中的记录 id；`null` = 没选（日志卡片显示引导文案）。 */
  const selectedId = ref<string | null>(null)
  /** 选中那条的事件流累计视图。 */
  const stream = ref<RunStreamState>(newStreamState())
  /** 断流 / 回放为空时的说明（与"还没选"区分开）。 */
  const logNotice = ref('')
  /** 1 秒一跳的"现在"，只给已用时长用。 */
  const nowMs = ref(Date.now())

  /**
   * 日志区是否跟随到底部。
   *
   * 初值 `true`（刚打开一条就跟着走），用户往上滚一下就变 `false` ——
   * 不这么做，要么"看历史日志时被一直拽到底"，要么"跑着的任务不跟随"。
   * 由滚动容器自己的 `scroll` 事件维护（程序滚动也走那条路，
   * 滚到底之后它自然还是 `true`）。
   */
  const stickToBottom = ref(true)

  /** 当前那条流。同一时刻只订阅一条 —— 日志区只显示一条。 */
  let source: EventSource | null = null
  /** 这条流属于哪个 runId。用来挡住"切走之后迟到的回调"。 */
  let streamRunId = ''
  /** 重订阅定时器（一条事件都没到、而记录还在跑时用）。 */
  let retryTimer: number | null = null

  let pollTimer: ReturnType<typeof setInterval> | null = null
  let tickTimer: ReturnType<typeof setInterval> | null = null

  const capOptions = computed(() => capabilityOptions(runs.value, titles.value))
  const statuses = computed(() => statusOptions(runs.value))
  const filtered = computed(() => filterRuns(runs.value, capFilter.value, statusFilter.value))
  /**
   * 选中那条记录。
   *
   * ⚠⚠ **故意不是 `computed`**。写成 `computed(() => runs.value.find(...))` 的话，
   * 每拉一次列表（轮询 / 点刷新 / 流跑完后的那次补拉）都会 `find` 出一个**新对象**；
   * 而 `logCard()` 是在 JSX 里被**调用**的（`{logCard()}`），vapor 对"函数调用返回的
   * JSX"只能整块替换 —— 于是日志卡片连滚动容器一起被换掉，`scrollTop` 归零。
   * 2026-09-23 实机：60 行日志一路 `gap: 0` 贴在底部，流一结束就跳回第 1 行
   * （`sameNode: false`、`oldConnected: false`）。
   * ⇒ 改由 `syncSelected()` 手动同步，**内容没变就不换引用**。
   */
  const selected = ref<RunRecord | null>(null)

  /**
   * 两条记录"显示出来一模一样"吗。
   *
   * 整条序列化比，而不是手写字段清单 —— 手写那种**漏一个字段就显示陈旧值**，
   * 后端加字段时也没人会想起来改它。只在"刚拉完列表"这一条路径上比，一次一条。
   */
  const sameRun = (a: RunRecord, b: RunRecord): boolean => JSON.stringify(a) === JSON.stringify(b)

  /**
   * 把 `selected` 与刚拉回来的列表对齐（为什么不是 computed 见 `selected` 上面）。
   *
   * 内容没变 ⇒ **连引用都不换**，日志卡片原样留着（滚动位置也就留住了）。
   * 真变了 ⇒ 卡片会重建、`scrollTop` 归零，所以顺手把"原本贴着底"的补回去：
   * 跑完那一刻用户要看的是末尾，不是第一行。
   */
  const syncSelected = () => {
    const fresh =
      selectedId.value === null
        ? null
        : (runs.value.find((run) => run.id === selectedId.value) ?? null)
    if (fresh === null) {
      selected.value = null
      return
    }
    if (selected.value !== null && sameRun(selected.value, fresh)) return
    const wasAtBottom = stickToBottom.value
    selected.value = fresh
    if (wasAtBottom) void nextTick(scrollToBottom)
  }

  /**
   * 还有没有"进行中"的记录。轮询与 1 秒时钟都挂在它上面：
   * 一个跑完的列表不需要每 5 秒重取一次（那只是白烧请求）。
   */
  const hasRunning = computed(() => runs.value.some((run) => run.status === 'running'))

  /** 还能再多要几条；到顶回 null（那枚按钮就收起来）。 */
  const moreLimit = computed(() =>
    runs.value.length >= limit.value ? nextLimit(limit.value) : null,
  )

  /**
   * 筛选卡片底下那句话。
   *
   * ⚠ 整句在**一个表达式**里拼好，不写成 `本页 {n} 条 … {筛出 m 条}` ——
   * JSX 会把换行折成空格，中文里"条）， 筛出"那个空格很显眼（同一族的坑在
   * `tasks-page.tsx` 里也记了一条）。
   */
  const summaryText = computed(() => {
    const base = `本页 ${runs.value.length} 条（一次最多 ${LIMIT_MAX} 条）`
    return filtered.value.length === runs.value.length
      ? base
      : `${base}，筛出 ${filtered.value.length} 条`
  })

  const titleOf = (code: string): string => titles.value[code] ?? code

  // ── 取数 ────────────────────────────────────────────────────────────────

  /**
   * 拉一次列表。
   *
   * `quiet` 是给**轮询**用的：那种刷新不能把整页换成加载态，
   * 否则一个还在跑的列表会每 5 秒闪一次 spinner，而用户正在看它。
   */
  const load = async (opts?: { quiet?: boolean }) => {
    if (!opts?.quiet) {
      state.value = 'loading'
      errorMessage.value = ''
    }
    try {
      const [runList, caps] = await Promise.all([
        listRuns(limit.value),
        // 标签而已，失败就回落成 code（见 `titles` 的注释）。
        listCapabilities().catch(() => null),
      ])
      runs.value = runList.runs
      if (caps) {
        const map: Record<string, string> = {}
        for (const cap of caps.all) map[cap.code] = cap.title
        titles.value = map
      }
      state.value = 'ready'
      // 先对齐选中项（它决定日志卡片），再处理"被筛掉了"。
      syncSelected()
      syncSelection()
    } catch (e) {
      // 轮询失败**不**把整页打成错误态：手上那份数据还是好的，
      // 把它换成一句"请求失败"是净损失。真正的失败由下一次手动刷新暴露。
      if (opts?.quiet) return
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /**
   * 记录被筛掉、或被新一页挤出去之后，选中的那条就不在列表里了。
   *
   * 不清理的后果：列表里一条高亮的都没有，日志卡片却还在讲"某条记录"的日志 ——
   * 看起来像点错了。所以**离开列表就收起**。
   */
  const syncSelection = () => {
    if (selectedId.value === null) return
    if (!filtered.value.some((run) => run.id === selectedId.value)) {
      clearSelection()
    }
  }

  onMounted(() => {
    void load()
    // 轮询与时钟都只在"有东西在跑"时才有意义，所以在回调里判而不是在开关上判 ——
    // 开关要跟着数据变，而数据每 5 秒才变一次，容易漏掉"最后一条跑完了"的那一刻。
    pollTimer = setInterval(() => {
      if (hasRunning.value) void load({ quiet: true })
    }, POLL_MS)
    tickTimer = setInterval(() => {
      if (hasRunning.value) nowMs.value = Date.now()
    }, TICK_MS)
  })

  onUnmounted(() => {
    // 关连接，但**不**取消后端那一趟：任务在后台继续（§6.3 那句"可关闭页面"说的是实话）。
    closeStream()
    if (pollTimer !== null) clearInterval(pollTimer)
    if (tickTimer !== null) clearInterval(tickTimer)
  })

  // ── 事件流 ──────────────────────────────────────────────────────────────

  /** 关掉手上这条流（幂等）。**不**动 `streamRunId` —— 那是"归属"，由 `openStream` 管。 */
  const closeStream = () => {
    if (retryTimer !== null) {
      window.clearTimeout(retryTimer)
      retryTimer = null
    }
    source?.close()
    source = null
  }

  const isNearBottom = (): boolean => {
    const node = document.getElementById(LOG_ID)
    if (!node) return true
    return node.scrollHeight - node.scrollTop - node.clientHeight < NEAR_BOTTOM_PX
  }

  const scrollToBottom = () => {
    const node = document.getElementById(LOG_ID)
    if (node) node.scrollTop = node.scrollHeight
  }

  /**
   * 把日志区的滚动位置放回去。
   *
   * 为什么需要：`{logCard()}` 每次数据更新都会**重建整张卡片**（见 `logCard` 上面那段），
   * 新节点的 `scrollTop` 是 0 —— 用户正看着第 30 行，一来新日志就被弹回第 1 行。
   * 所以每次更新后按"更新前的意图"还原：
   * - 原本贴着底 ⇒ 继续贴底；
   * - 原本停在中间 ⇒ 回到**同一个 `scrollTop`**（多出来的行都在末尾，顶部内容的位置不变，
   *   所以同一个数值看到的还是同一段日志）。
   *
   * `stick` / `prevTop` 都要在**赋值之前**读好 —— 赋值一发生它们就归零了。
   */
  const restoreScroll = (stick: boolean, prevTop: number) => {
    const node = document.getElementById(LOG_ID)
    if (!node) return
    node.scrollTop = stick ? node.scrollHeight : prevTop
  }

  /**
   * 订阅一条 run 的事件流。
   *
   * ⚠ 收到终局事件**必须自己 `close()`**（`runEvents` 的注释里那条契约）：
   * `EventSource` 会按规范自动重连，不关就是一个跑完的 run 被一直重连、
   * 每次带 `Last-Event-ID` 让服务端重放 0 条再关闭。
   *
   * `attempt` 只在重订阅那条路上自增，用来兜"订阅上了但一条都没到"的窗口。
   */
  const openStream = (runId: string, attempt = 0) => {
    closeStream()
    streamRunId = runId
    const current = new EventSource(runEventsUrl(runId))
    source = current

    current.onmessage = (message) => {
      // 切走之后迟到的回调：丢掉，别把上一条的日志写进这一条。
      if (streamRunId !== runId) return
      const event = parseRunEvent(message.data)
      if (event === null) return
      // ⚠ 这两个都要在赋值**之前**读：赋值会重建卡片，`scrollTop` 随即归零。
      const stick = stickToBottom.value
      const prevTop = document.getElementById(LOG_ID)?.scrollTop ?? 0
      stream.value = applyEvent(stream.value, event)
      // ⚠ 必须等这一轮 DOM 更新落下去再量 scrollHeight，否则量到的是加这一行之前的。
      void nextTick(() => restoreScroll(stick, prevTop))
      if (isTerminalEvent(event)) {
        closeStream()
        // 终局事件到了，但 `runs.status` / `message` 是**另一条路**写进库的
        // （`UpdateRun`）⇒ 拉一次让列表上的状态跟着变。
        // ⚠ 这一拉**又会**重建卡片一次，所以补完数据要再放一次滚动位置 ——
        //    "跑完就跳回第 1 行"正是漏了这一步（2026-09-23 实机）。
        void load({ quiet: true }).then(() => void nextTick(() => restoreScroll(stick, prevTop)))
      }
    }

    current.onerror = () => {
      if (streamRunId !== runId) return
      const row = runs.value.find((run) => run.id === runId)
      // 一条事件都没到 + 记录还在跑 ⇒ 那不是"断了"，是"事件还没发出来"
      // （订阅那一刻 run 行刚插进去、第一条事件还没落）。隔一秒重订阅。
      if (
        stream.value.phase === 'pending' &&
        row?.status === 'running' &&
        attempt < RESUBSCRIBE_MAX
      ) {
        closeStream()
        retryTimer = window.setTimeout(() => {
          retryTimer = null
          if (streamRunId === runId) openStream(runId, attempt + 1)
        }, RESUBSCRIBE_MS)
        return
      }
      // 两种"没取到"要分开说 —— 它们在界面上长得一样，但下一步完全不同：
      //   - 一条都没到过：库里本来就没有这条的事件（进程重启前跑的、或没落库）；
      //   - 收到过才断：真的断了，下面是已经拿到的部分。
      const hadNothing = stream.value.phase === 'pending'
      stream.value = markLost(stream.value)
      logNotice.value = hadNothing
        ? '这条记录没有回放出任何日志。可能它是进程重启之前跑的（事件还没落库），或者那一趟在写库之前就退出了。'
        : '日志流中断了 —— 下面是已经收到的部分。'
      closeStream()
    }
  }

  // ── 选择 ────────────────────────────────────────────────────────────────

  const clearSelection = () => {
    closeStream()
    streamRunId = ''
    selectedId.value = null
    selected.value = null
    stream.value = newStreamState()
    logNotice.value = ''
  }

  const toggleSelect = (run: RunRecord) => {
    if (selectedId.value === run.id) {
      clearSelection()
      return
    }
    selectedId.value = run.id
    selected.value = run
    stream.value = newStreamState()
    logNotice.value = ''
    stickToBottom.value = true
    openStream(run.id)
    // 日志卡片在**列表上方**（否则移动端要滚过整张列表才看得到结果）。
    // 所以选中之后要把它带进视野，不然"点了没反应"。
    void nextTick(() => {
      document.getElementById(LOG_CARD_ID)?.scrollIntoView({ block: 'start' })
    })
  }

  const clearFilters = () => {
    capFilter.value = ''
    statusFilter.value = ''
    // 清筛选可能把选中的那条放回来，但**不**主动恢复选择：收起之后状态是干净的。
    syncSelection()
  }

  const loadMore = () => {
    const next = moreLimit.value
    if (next === null) return
    limit.value = next
    void load()
  }

  // ── 渲染 ────────────────────────────────────────────────────────────────

  const head = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">历史与日志</h1>
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

  const filtersCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <div class="grid gap-3 md:grid-cols-2">
          <fieldset class="fieldset">
            <legend class="fieldset-legend">能力</legend>
            <select
              class="select w-full"
              value={capFilter.value}
              onChange={(e) => {
                capFilter.value = (e.target as HTMLSelectElement).value
                syncSelection()
              }}
            >
              <option value="">全部能力（{runs.value.length}）</option>
              {capOptions.value.map((option) => (
                <option value={option.code}>
                  {option.label}（{option.count}）
                </option>
              ))}
            </select>
          </fieldset>
          <fieldset class="fieldset">
            <legend class="fieldset-legend">状态</legend>
            <select
              class="select w-full"
              value={statusFilter.value}
              onChange={(e) => {
                statusFilter.value = (e.target as HTMLSelectElement).value
                syncSelection()
              }}
            >
              <option value="">全部状态</option>
              {statuses.value.map((status) => (
                <option value={status}>{statusLabel(status)}</option>
              ))}
            </select>
          </fieldset>
        </div>
        <p class="text-xs opacity-60">{summaryText.value}</p>
      </div>
    </div>
  )

  /**
   * 日志卡片。**在列表上方** —— 见 `toggleSelect` 里那段。
   *
   * ⚠⚠ 这个 `{logCard()}` 有个已经量清楚的代价，别当成笔误：
   * vapor 对"函数调用返回的 JSX"只能**整块替换**，而它依赖函数体内读到的每一个 ref
   * （`selected` / `stream` / `logNotice` …）。所以**每来一条事件，整张卡片连同里面的
   * 滚动容器都会被换掉一次**，`scrollTop` 归零。
   * 2026-09-23 实机量到：日志从 18 行涨到 27 行（9 条事件）之间，容器节点已被换过
   * （`sameNode: false`）。跟随看着是好的，只是因为重建后补了一次滚动；
   * **流一结束（最后一次 `load()`）那一下没人补**，于是跳回第 1 行。
   * ⇒ 解法是"重建之后把滚动位置放回去"，见 `restoreScroll`。
   *
   * ⚠⚠⚠ 也**不能**把它改成组件（`<LogCard />`）来躲开重建 —— 试过，不行：
   * 组件函数体只在创建时执行一次，下面那个 `if (run === null) return …` 的早期返回
   * **不会再求值**。实机表现是：点了行（行高亮 `bg-base-200` 了）而卡片一直停在
   * "点下面任意一条记录…"的引导态。要用组件，就得把函数体里所有 `const run = …`
   * 这类一次性求值全部改写成 JSX 内的表达式（`{cond ? <A/> : <B/>}`），
   * 那是另一笔账，当前不值得。
   */
  const logCard = () => {
    const run = selected.value
    if (run === null) {
      /*
       * 没选中的时候**只占一行**，不是一张 py-8 的空卡：
       * 空卡会在窄屏上占掉大半屏，而它下面那句"点下面任意一条记录"反而看不见了。
       * 但也不能什么都不显示 —— 那样没人知道点一行会发生什么（§5.10 那条"给出下一步"）。
       */
      return (
        <div
          id={LOG_CARD_ID}
          class="scroll-mt-20 rounded-box bg-base-100 p-3 text-xs leading-5 opacity-70 shadow-sm"
        >
          {/* ⚠ 这句必须写成**一个表达式**，不能是裸 JSX 文本。
              实测（2026-09-23）：`oxfmt` 会把过长的 JSX 文本行折开，而
              `vue-jsx-vapor` 对 JSX 文本里的换行是**原样保留**的
              （编译产物里就是 `…。\n    落库…`），浏览器再把它渲染成一个空格 ——
              中文里"回放 —— 落库"那个空格很显眼。
              写成 `{'…'}` 之后 oxfmt 动不了它：它不会拆字符串字面量。 */}
          {
            '点下面任意一条记录，这里显示它的日志。已经跑完的那一趟也会完整回放 —— 落库的是一整批事件，不只是结论那一行。'
          }
        </div>
      )
    }

    const finished = stream.value.finished
    const errored = stream.value.errored
    const running = run.status === 'running'

    return (
      <div id={LOG_CARD_ID} class="card scroll-mt-20 bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="flex flex-wrap items-center gap-2">
              <h2 class="card-title text-base">{titleOf(run.capability)}</h2>
              {/* 生命周期徽标（中性口径），不是结论 */}
              <span class={statusBadge(run.status)}>{statusLabel(run.status)}</span>
              {/*
                ⚠ 结论徽标只在**真的收到 `finished`** 时才出，而它注定不会出现在失败那一支上：
                一趟失败会发**两条**终局事件 —— `Runner.runOne` 先发 `errored`，
                紧接着再发 `finished(failed)`（2026-09-23 实拍，同一趟的 seq 是
                `started(1) → errored(2) → finished(3) → progress(4)`）。
                前端按 `runEvents` 明写的契约在**第一条**终局事件上就 `close()`
                ⇒ `finished` 到不了前端，`outcome === 'failed'` 在客户端**不可达**。

                这一支**不补**一枚"执行出错"徽标：那会和上面的生命周期徽标撞成同一件事
                （`RunOutcome.ToLegacyStatus` 里**只有** `OutcomeFailed` 映射到
                `StatusFailed`，两者是 1:1）—— 并排显示两个"失败"等于同一件事写两遍，
                而这个仓库点过名的那种毛病。
                真正的信息是**原因**，由下面那条 `errored` 提示条给。

                （`live-redflower-page.tsx` 那边补了「执行出错」，因为它的行里
                没有生命周期徽标，不补就一个都没有。两页的取舍不同，理由在此。）
              */}
              {finished ? (
                <span class={`badge ${outcomeBadge(finished.outcome)}`}>
                  {outcomeLabel(finished.outcome)}
                </span>
              ) : null}
            </div>
            <button
              type="button"
              class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
              onClick={clearSelection}
            >
              收起
            </button>
          </div>

          {/* 每项带标签（§7.3），窄屏一行一项、桌面两列 */}
          <div class="grid gap-1 text-xs md:grid-cols-2">
            <div class="flex gap-2">
              <span class="w-14 shrink-0 opacity-60">账号</span>
              <span class="break-words">{run.accountId ?? '—'}</span>
            </div>
            <div class="flex gap-2">
              <span class="w-14 shrink-0 opacity-60">开始</span>
              <span>{fmtTime(run.startedAt)}</span>
            </div>
            <div class="flex gap-2">
              <span class="w-14 shrink-0 opacity-60">结束</span>
              <span>{fmtTime(run.finishedAt)}</span>
            </div>
            <div class="flex gap-2">
              <span class="w-14 shrink-0 opacity-60">用时</span>
              <span>{formatElapsed(elapsedMillis(run, nowMs.value))}</span>
            </div>
            <div class="flex gap-2">
              <span class="w-14 shrink-0 opacity-60">云朵</span>
              <span>{cloudGainText(run.cloudGained)}</span>
            </div>
            <div class="flex gap-2">
              <span class="w-14 shrink-0 opacity-60">runId</span>
              <span class="break-all font-mono">{run.id}</span>
            </div>
          </div>

          {/* 还在跑：给个动的东西 + 已用时长（§6.3）。没有进度条 —— 理由见文件头。 */}
          {running ? (
            <div class="flex flex-wrap items-center gap-2 text-xs opacity-70">
              <span class="loading loading-dots loading-sm" />
              <span>{`正在执行，已用 ${formatElapsed(elapsedMillis(run, nowMs.value))}。可以关闭这个页面，任务在后端继续跑，回来还能看到结果。`}</span>
            </div>
          ) : null}

          {/* 日志区：**固定高度**可滚动（§7.2）。
              用 `h-64` 而不是 `max-h-64`：后者会让这个盒子从 0 长到 256px，
              日志每来一行整页就跳一下。固定高度把版式钉住。 */}
          <div
            id={LOG_ID}
            class="h-64 overflow-y-auto overscroll-contain rounded-box bg-base-200 p-3 font-mono text-xs leading-5"
            onScroll={() => (stickToBottom.value = isNearBottom())}
          >
            {stream.value.lines.length === 0 ? (
              <p class="opacity-60">{emptyLogText(stream.value.phase)}</p>
            ) : (
              stream.value.lines.map((line) => (
                <div class={`break-words whitespace-pre-wrap ${levelClass(line.level)}`}>
                  {line.message}
                </div>
              ))
            )}
          </div>

          {logNotice.value ? (
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <IconAlert class="size-4" />
              <span class="break-words">{logNotice.value}</span>
            </div>
          ) : null}

          {errored ? (
            <div class="alert alert-soft alert-error text-sm" role="alert">
              <IconAlert class="size-4" />
              <span class="break-words">{errored.message}</span>
            </div>
          ) : null}

          {finished?.summary ? <p class="break-words text-sm">{finished.summary}</p> : null}

          {/* 需人工介入的待办：后端给枚举码，这里显示文案 + 去哪做 */}
          {(finished?.manual_steps ?? []).map((step) => (
            <div class="alert alert-soft alert-warning text-sm" role="status">
              <span class="break-words">
                {step.instruction}
                <span class="opacity-60">（{manualTargetLabel(step.target)}）</span>
              </span>
            </div>
          ))}

          {/*
            ⚠ 这一句只在「**收到过日志、但流断了**」时才出，三个条件缺一不可：
              - `phase === 'lost'`：流结束了、而且没等到终局事件。光判 `!== 'pending'`
                是不够的 —— `open`（还在跑）也会满足，于是日志正在滚的时候就先下了结论
                （2026-09-23 实机看到："这一条没有收到结论事件"出现在第 15 行还在进的时候）。
              - `lines.length > 0`：**一条都没收到**是另一回事，那由上面那条
                "没有回放出任何日志"的提示负责；两句一起出会打架。
              - `!running`：还在跑的那一趟本来就没有结论，那不是问题。
          */}
          {stream.value.phase === 'lost' && stream.value.lines.length > 0 && !running ? (
            <p class="break-words text-xs opacity-60">
              日志只收到了一部分，没有等到结论事件。列表上的「已结束」说的是"跑完了"，不是"成了"。
            </p>
          ) : null}
        </div>
      </div>
    )
  }

  const listCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h2 class="card-title text-base">执行记录</h2>
          {moreLimit.value !== null ? (
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={state.value === 'loading'}
              onClick={loadMore}
            >
              再看 {LIMIT_STEP} 条
            </button>
          ) : null}
        </div>

        {runs.value.length === 0 ? (
          /* 空态给出下一步（§5.10），不写"暂无数据" */
          <div class="flex flex-col items-center gap-2 py-8 text-center">
            <span class="text-base-content/30">
              <IconHistory class="size-8" />
            </span>
            <p class="text-sm opacity-70">还没有任何执行记录。</p>
            <p class="max-w-prose break-words text-xs opacity-60">
              去「任务中心」跑一条能力，结果会出现在这里。只读的查询类能力不写运行历史，所以它跑完也不会多出一行。
            </p>
            {props.onGo ? (
              <button
                type="button"
                class="btn btn-sm btn-soft btn-primary min-h-11 md:min-h-0"
                onClick={() => props.onGo?.('tasks')}
              >
                去任务中心
              </button>
            ) : null}
          </div>
        ) : filtered.value.length === 0 ? (
          /* 与"一条都没有"分开：这是筛出来的空，下一步是清筛选 */
          <div class="flex flex-col items-center gap-2 py-8 text-center">
            <p class="text-sm opacity-70">当前筛选下没有记录。</p>
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              onClick={clearFilters}
            >
              清空筛选
            </button>
          </div>
        ) : (
          <>
            {/* 桌面表格 */}
            <div class="hidden overflow-x-auto md:block">
              <table class="table table-sm">
                <thead>
                  <tr>
                    <th>开始</th>
                    <th>能力</th>
                    <th>账号</th>
                    <th>状态</th>
                    <th>云朵</th>
                    <th>结论</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {filtered.value.map((run) => {
                    const on = run.id === selectedId.value
                    return (
                      <tr
                        class={
                          on ? 'cursor-pointer bg-base-200' : 'cursor-pointer hover:bg-base-200'
                        }
                        onClick={() => toggleSelect(run)}
                      >
                        <td class="whitespace-nowrap">{fmtTime(run.startedAt)}</td>
                        <td class="break-words">{titleOf(run.capability)}</td>
                        <td class="break-words">{run.accountId ?? '—'}</td>
                        <td>
                          <span class={statusBadge(run.status)}>{statusLabel(run.status)}</span>
                        </td>
                        <td class="whitespace-nowrap">{cloudGainText(run.cloudGained)}</td>
                        <td class="break-words">{run.message ?? '—'}</td>
                        {/* 明确的动作提示：不靠 hover 才知道这一行能点（§6.2） */}
                        <td class="whitespace-nowrap text-xs opacity-60">
                          {on ? '收起' : '看日志'}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>

            {/* 移动卡片（每项带标签，§7.3）。整张卡是一枚按钮 ⇒ 触控目标远大于 44px。 */}
            <div class="grid gap-3 md:hidden">
              {filtered.value.map((run) => {
                const on = run.id === selectedId.value
                return (
                  <button
                    type="button"
                    class={
                      on
                        ? 'rounded-box border-2 border-primary bg-base-200 p-3 text-left'
                        : 'rounded-box border-2 border-transparent bg-base-200 p-3 text-left'
                    }
                    onClick={() => toggleSelect(run)}
                  >
                    <div class="flex flex-wrap items-center justify-between gap-2">
                      <span class="text-sm font-medium">{fmtTime(run.startedAt)}</span>
                      <span class={statusBadge(run.status)}>{statusLabel(run.status)}</span>
                    </div>
                    <div class="mt-2 flex gap-2 text-sm">
                      <span class="w-14 shrink-0 opacity-60">能力</span>
                      <span class="break-words">{titleOf(run.capability)}</span>
                    </div>
                    <div class="mt-1 flex gap-2 text-sm">
                      <span class="w-14 shrink-0 opacity-60">账号</span>
                      <span class="break-words">{run.accountId ?? '—'}</span>
                    </div>
                    <div class="mt-1 flex gap-2 text-sm">
                      <span class="w-14 shrink-0 opacity-60">云朵</span>
                      <span>{cloudGainText(run.cloudGained)}</span>
                    </div>
                    <div class="mt-1 flex gap-2 text-sm">
                      <span class="w-14 shrink-0 opacity-60">结论</span>
                      <span class="break-words">{run.message ?? '—'}</span>
                    </div>
                    <div class="mt-2 text-xs opacity-60">{on ? '收起日志' : '看日志'}</div>
                  </button>
                )
              })}
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
          {filtersCard()}
          {logCard()}
          {listCard()}
        </>
      ) : null}
    </div>
  )
}
