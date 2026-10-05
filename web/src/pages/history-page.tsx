/**
 * 历史与日志页。
 *
 * # 这次改版（2026-10-01，用户口径「运行日志任务美化」）
 *
 * 改版前这一页有两个真问题，都不是"好不好看"：
 *
 * 1. **状态躲起来了。**「结果」列放的是 `outcomeText`（摘要，如"打卡 3 个账号"），
 *    成功/失败只能靠徽章**颜色**猜 —— 而 `skipped` 是刻意的中性色（活动没开不是故障），
 *    于是"跳过"跟"成功"在界面上一模一样。⇒ 现在**状态徽章自己占一层**，
 *    摘要降到它下面的小字。
 * 2. **记录一多就只能从头翻。** 没有筛选、没有搜索。⇒ 加状态档位
 *    （**成功 / 部分成功 / 跳过 / 已中止 / 失败**；`TaskOutcome.status` 里**没有**"运行中"，
 *    不凭截图里那个参考页面去编一档出来 —— 「已中止」是例外：它不是编的，
 *    后端 `RunStatus::Cancelled` 就是这么一档，见 `ui/history.ts` 的 `RunStatusView`）
 *    + 一个搜索框（任务名 / 账号 / 运行号 / 摘要）。
 *    全部在**前端**过滤：`/api/runs` 一次把记录取全，不新增接口、不改后端。
 *
 * 布局沿用规范 §7.2：桌面表格（列多、要横向比较），窄屏**卡片流**。
 */

import { onMounted, ref, onUnmounted } from 'lite'
import { Dialog, openDialog } from '../ui/dialog'
import { LogPanel, type LogLine } from '../ui/log-panel'
import { setPageRefresh } from '../ui/page-refresh'
import { getRun, listRuns, type RunLogLine, type RunRecord } from '../api'
// 直播的结构化明细：与直播页**共用同一份**渲染（从前只有"当前这次"能看到）。
import { LiveRoomResult } from '../ui/live-room-result'
import { toLiveRoomRunView } from '../api'
import { accountName, refreshAccountNames } from '../account-names'
import { EmptyState } from '../ui/empty-state'
import { IconClose, IconSearch } from '../ui/icons'
import { fmtDuration, fmtEpochMs } from '../ui/time'
import { outcomeText, runStatusOf, statusBadge, statusLabel, type RunStatusView } from '../ui/history'
import type { PageId } from '../ui/nav'

export interface HistoryPageProps {
  onGo: (id: PageId) => void
}

type LoadState = 'loading' | 'error' | 'ready'

/** 状态档位。`all` 不是一种状态，是"不筛"。 */
type StatusFilter = 'all' | RunStatusView

/**
 * 一行"这次运行在用户网盘里留下了什么"。
 *
 * # 为什么值得单独占一列
 *
 * 旧项目只在小结里念一句"网盘里留了 N 个待清理文件"，却**不说是哪 N 个** ——
 * 用户想去手工清理时无从下手。所以这里把 `fileId` 直接给出来：
 * 它是清理那一枪唯一认的标识。
 *
 * # 为什么空的时候什么都不显示
 *
 * 绝大多数运行是干净的（链尾自己收尾）。给每一行都挂一个"无"字
 * 只会把真正有遗留的那几行淹掉 —— 而这一列的全部价值就是让人**一眼看见**
 * 那几行。
 */
/** 运行日志弹窗的 id（`openDialog` / `Dialog` 两边都用它 ✓）。 */
const RUN_LOG_DIALOG_ID = 'run-log-dialog'

/**
 * 后端那四个级别 → 面板认的三档。
 *
 * ⚠ `debug` 并进 `info` 是刻意的：**回看**时"调试行"与"普通行"同色是对的 ✓
 * —— 实时面板也是这么归的 ✓。两处别归得不一样 ✗（同一份日志两种颜色最费解 ✓）。
 */
const toLogLines = (lines: RunLogLine[]): LogLine[] =>
  lines.map((l) => ({
    // ⚠ 优先用**后端给的分类** ✓（info/success/fail/start…）；只有老记录没有它时
    // 才按级别兜底 ✓ —— 兜底出来的只能三分，颜色会丢掉"成功/失败"的区别 ✓。
    kind: l.kind ?? (l.level === 'error' ? 'error' : l.level === 'warn' ? 'warn' : 'info'),
    text: l.message,
    at: l.at_ms,
  }))

/**
 * 「日志」入口。
 *
 * ⚠ 桌面表格与移动卡片**两处都要放** ✓ —— 不然后端辛苦存下来的日志只在宽屏
 * 看得见（窄屏那条验收会看这个 ✓，`history-page` 里别的明细格也是这么办的 ✓）。
 */
const LogButton = (props: { record: RunRecord; onOpen: (r: RunRecord) => void }) => (
  <button
    type="button"
    class="btn btn-xs btn-soft w-fit"
    aria-label={`查看日志：${props.record.task} #${props.record.run_id}`}
    onClick={() => props.onOpen(props.record)}
  >
    日志
  </button>
)

const LeftoverCell = (props: { record: RunRecord }) => {
  const items = props.record.details?.artifacts ?? []
  if (items.length === 0) return null

  return (
    <div class="flex flex-col gap-1">
      <span class="badge badge-warning badge-sm w-fit">遗留 {items.length} 个文件</span>
      <ul class="flex flex-col gap-0.5">
        {items.map((a) => (
          // 给出账号与 ID：清理要用该账号的凭据按 ID 删。
          // ⚠ 账号那一格走 accountName：主键是哈希，摆出来认不出是哪个号；
          // 产物 ID 保持原样，它本来就是拿来操作的东西。
          <li key={`${a.account}/${a.id}`} class="font-mono text-xs opacity-70">
            {accountName(a.account)} · {a.id}
          </li>
        ))}
      </ul>
    </div>
  )
}

/**
 * 直播那一次的**结构化明细**（领到的花 + 逐条口令的落点）。
 *
 * # 这一格是补上的缺口
 *
 * 这套明细原来只长在直播页上 ⇒ **只有"当前这次"看得到** ✗：历史里每一次直播
 * 都只剩一行小结（`outcome.summary`），于是用户想回看"上一场到底领到没有、
 * 哪条口令过期了、为什么"时，界面上无处可看。
 *
 * ⚠ 判据是"**这是直播任务、并且后端给了结构化明细**"：光看 `task` 不够 ——
 * 老版本的记录只有文本小结（`details.liveRoom` 不在），那时渲染出来是
 * 一块"领到 0 个"的假明细，比不显示更糟 ✗。
 */
const LiveRoomCell = (props: { record: RunRecord }) => {
  const r = props.record
  if (r.task !== 'live-room' || r.details?.liveRoom == null) return null
  return (
    <div class="min-w-0">
      <LiveRoomResult view={toLiveRoomRunView(r)} />
    </div>
  )
}

/**
 * 兑换那一次的明细 —— **后端给什么就显示什么**（用户口径：所有内容都要进记录）。
 *
 * ⚠ 判据是"这是兑换任务 **且** 后端给了结构化明细"：只有老记录才两者都不在，
 * 那时这里什么都不渲染（只有一句文本小结），**不编**假明细 ✗。
 */
const ExchangeCell = (props: { record: RunRecord }) => {
  const r = props.record
  const d = r.details
  if (r.task !== 'exchange' || d?.kind !== 'exchange') return null
  const o = d.order
  return (
    <div class="flex min-w-0 flex-col gap-0.5 text-xs">
      <span class="opacity-80">
        {d.status ?? '—'} · {d.plan ?? '—'}
        {d.plan_reason ? `（${d.plan_reason}）` : ''}
      </span>
      <span class="opacity-60">
        上游码 {d.code ?? '—'} · 轮次 {d.rounds ?? 0} · 发单 {d.attempts ?? 0} · offset{' '}
        {d.offset ?? '—'}
      </span>
      <span class="opacity-60">
        余额 {d.balance ?? '—'} → {d.after_balance ?? '—'}
      </span>
      {d.waited_until_ms != null ? (
        <span class="opacity-60">等到 {fmtEpochMs(d.waited_until_ms)}</span>
      ) : null}
      {d.prize ? (
        <span class="opacity-80">
          奖品 {d.prize.name ?? `#${d.prize.prize_id}`} · {d.prize.price} 豆
        </span>
      ) : null}
      {o ? (
        <span class="opacity-80">
          订单 {o.order_id} · {o.prize_name ?? `#${o.prize_id}`} · 花 {o.cost} 豆
        </span>
      ) : (
        <span class="opacity-60">没有订单</span>
      )}
      {d.message ? <span class="opacity-60">上游说：{d.message}</span> : null}
      {d.device_id_failed ? (
        <span class="text-error">deviceId 没接上：{d.device_id_failed}</span>
      ) : null}
      {d.error ? <span class="text-error">{d.error}</span> : null}
    </div>
  )
}

/**
 * 状态徽章。
 *
 * `w-fit` 不能省：放在纵向 flex 列里时，徽章会被拉满整列宽，
 * 看起来像一条色带而不是一枚标签。
 */
const StatusChip = (props: { record: RunRecord }) => (
  <span class={`badge badge-sm w-fit whitespace-nowrap ${statusBadge(runStatusOf(props.record))}`}>
    {statusLabel(runStatusOf(props.record))}
  </span>
)

export const HistoryPage = (props: HistoryPageProps) => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  const runs = ref<RunRecord[]>([])
  const filter = ref<StatusFilter>('all')
  const query = ref('')

  /**
   * 宽屏（≥768px）。
   *
   * ⚠⚠ **表格与卡片流只渲染一份**（2026-10-05 用户实测截图：DevTools 性能监视器里
   * DOM nodes **30,124** ✗）。改之前是 CSS 双份渲染（`hidden md:block` + `md:hidden`）
   * —— 两份**都真的在 DOM 里**，只是各自被 CSS 藏起来 ⇒ 这一页的节点数是**双倍** ✗。
   * 实测（夹具灌到真实体量、100 次运行、GC 两次后）：历史页 **21,915** 个节点，
   * 而最重的其它页只有 ~7k ⇒ 光这一条就砍掉近一半 ✓。
   *
   * ⚠ 768 这个数必须与 CSS 的 `md` **同一档**（`app.css` 的 `@theme` 断点）——
   * 对不上的后果是"两份都藏起来"（白屏）或"两份都显示"（双倍）✗ ⇒ 写在下面那个
   * `WIDE_QUERY` 里，只此一处 ✓。
   */
  const WIDE_QUERY = '(min-width: 768px)'
  const wide = ref(matchMedia(WIDE_QUERY).matches)
  /** 媒体查询对象（`onMounted` 里建、`onUnmounted` 里解绑 —— 不建在组件体里，避免每次重渲染都新建 ✓）。 */
  let mq: MediaQueryList | undefined
  const onWideChange = () => {
    if (mq) wide.value = mq.matches
  }

  /**
   * 分页：一页多少条、现在第几页。
   *
   * ⚠ 为什么必须分页（2026-10-05 用户口径「任务记录分页」）：`/api/runs` 是**全量**接口
   * （`listRuns()` 不带参数 ✓），而运行记录只会越攒越多（本机已经 **113** 条 ✓）
   * ⇒ 全量铺进 DOM 的话，节点数与历史长度**线性增长**（用一年就是十万级 ✗）。
   *
   * ⚠ 20 而不是 50：这一页不是"看一屏就够"的读数页，是要**逐条看**的记录页 ✓；
   * 20 条（移动端是 20 张卡片）翻页的手感比 50 条一屏的长滚动好，节点也只要 ~1/5 ✓。
   */
  const PAGE_SIZE = 20
  const page = ref(1)

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      runs.value = await listRuns()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => {
    setPageRefresh({ run: () => void load() })
    // 先把"id → 名字"填上，否则运行记录里显示的是一串哈希
    void refreshAccountNames()
    void load()
    // ⚠ 断点监听建在**挂载时**（不是组件体里）：组件体每次重渲染都会跑，建在那里等于
    // 每次新建一个 MediaQueryList 且旧的永远解绑不掉 ✗。
    mq = matchMedia(WIDE_QUERY)
    mq.addEventListener('change', onWideChange)
    // 组件体求值时到真正挂载之间窗口尺寸可能已经变了（首屏/旋转）⇒ 再读一次为准 ✓
    onWideChange()
  })
  onUnmounted(() => {
    setPageRefresh(null)
    mq?.removeEventListener('change', onWideChange)
    mq = undefined
  })

  /** 一条记录命中搜索词吗。搜四样东西：任务名、账号、运行号、结果摘要。 */
  const matchesQuery = (r: RunRecord): boolean => {
    const q = query.value.trim().toLowerCase()
    if (!q) return true
    return [r.task, accountName(r.account), `#${r.run_id}`, String(r.run_id), outcomeText(r.outcome)]
      .join(' ')
      .toLowerCase()
      .includes(q)
  }

  /** 当前该显示哪些记录。**在 JSX 里调用**（lite：派生值必须写在绑定里，不能提前算）。 */
  const visible = (): RunRecord[] =>
    runs.value.filter((r) => (filter.value === 'all' || runStatusOf(r) === filter.value) && matchesQuery(r))

  /** 一共几页（**至少 1 页**：0 条时也要让"第 1 / 1 页"读得通 ✓）。 */
  const pageCount = (): number => Math.max(1, Math.ceil(visible().length / PAGE_SIZE))

  /**
   * 当前页（**夹在 `[1, pageCount]` 里**）。
   *
   * ⚠ 为什么不在"筛选变了"时去改 `page.value`：那是**在渲染里写信号** ✗
   * （lite 的 effect 重入保护会抛"循环更新"）。夹一下更简单也更稳：
   * 筛到只剩 1 页时，`page.value` 哪怕是 5，`curPage()` 也是 1 ✓。
   */
  const curPage = (): number => Math.min(page.value, pageCount())

  /** 真正铺进 DOM 的那 20 条 ✓。 */
  const shown = (): RunRecord[] => {
    const start = (curPage() - 1) * PAGE_SIZE
    return visible().slice(start, start + PAGE_SIZE)
  }

  /** 翻页（只在边界外才写信号 ✓，免得把 `page` 写成越界值）。 */
  const goPage = (p: number) => {
    const next = Math.min(Math.max(1, p), pageCount())
    if (next !== page.value) page.value = next
  }

  /** 某一档有几条。计数直接摆在档位上 —— 不用点进去就知道哪一档有东西。 */
  const countOf = (id: StatusFilter): number =>
    id === 'all' ? runs.value.length : runs.value.filter((r) => runStatusOf(r) === id).length

  /**
   * 运行日志（**回看**）。
   *
   * ⚠ 列表接口**不带日志**（`/api/runs` 会剥掉 ✓，见 `routes/runs.rs::list`）
   * ⇒ 点开的这一刻才按 id 取详情 ✓。那一下是网络请求 ⇒ 必须有"正在取 / 取失败"
   * 两个状态 ✓：点了没反应会让人以为功能坏了 ✗。
   */
  const logLines = ref<LogLine[]>([])
  const logState = ref<'loading' | 'ready' | 'error'>('loading')
  const logWho = ref('')
  const logError = ref('')

  const openRunLog = async (r: RunRecord) => {
    logLines.value = []
    logState.value = 'loading'
    logWho.value = `${r.task} #${r.run_id}`
    logError.value = ''
    // 先开弹窗再取（网络那一下不该让界面"没反应" ✓）
    openDialog(RUN_LOG_DIALOG_ID)
    try {
      const full = await getRun(r.run_id)
      logLines.value = toLogLines(full.logs ?? [])
      logState.value = 'ready'
    } catch (e) {
      logError.value = e instanceof Error ? e.message : String(e)
      logState.value = 'error'
    }
  }

  const filtering = (): boolean => filter.value !== 'all' || query.value.trim() !== ''

  const clearFilters = () => {
    filter.value = 'all'
    query.value = ''
    page.value = 1
  }

  /**
   * 筛选与搜索。
   *
   * ⚠ 只在**有记录**时出现：一条记录都没有时摆四个"0"的档位，
   * 只会让人以为是自己筛没了。
   */
  const toolbar = () => (
    <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
      <div class="flex flex-wrap items-center gap-1.5">
        {(
          [
            ['all', '全部'],
            ['success', '成功'],
            ['partial', '部分成功'],
            ['skipped', '跳过'],
            // ⚠ 「已中止」单开一档（数据来源是 `RunRecord.status`，不是 `outcome`）：
            // 不单开的话，用户自己中止的那些既会被「失败」算进去 ✗，
            // 又只能在「全部」里翻到 —— 而这正是 sv-3 要治的误读。
            ['cancelled', '已中止'],
            ['failed', '失败'],
          ] as const
        ).map(([id, label]) => (
          // ⚠ 这里**不能**写 `key`：`.map` 的参数是解构的，编译器不把它当"键控列表"
          // （`mapCallback` 只认普通标识符参数），于是 `key` 无人认领 ⇒ 构建期直接报错。
          // 一样的东西在账号页那排方式 tab 上也是这么写的。
          <button
            type="button"
            aria-pressed={filter.value === id}
            // ⚠ 文案里带了计数（「失败 1」），所以按钮的可见文字随数据变 —— 门禁按
            // `aria-label` 找它（`clickText` 是精确匹配，按"失败"找不到"失败1"）。
            aria-label={`筛选：${label}`}
            class={`btn btn-sm min-h-11 md:min-h-0 ${filter.value === id ? 'btn-primary' : 'btn-soft'}`}
            onClick={() => {
              filter.value = id
              // 换了档位就回第 1 页：不然停在"第 5 页"而新档只有 1 页，看起来像没结果 ✗
              page.value = 1
            }}
          >
            {label}
            <span class="text-xs opacity-60">{countOf(id)}</span>
          </button>
        ))}
      </div>

      <div class="flex w-full items-center gap-2 md:w-auto">
        {/* daisyUI 5 的 input 支持把图标当子节点摆进同一行（`label.input` 包一层） */}
        <label class="input flex min-h-11 w-full items-center gap-2 md:min-h-0 md:w-64">
          <IconSearch class="size-4 opacity-40" />
          <input
            type="search"
            class="grow"
            placeholder="搜索任务名 / 账号 / 运行号"
            value={query.value}
            onInput={(e) => {
              query.value = (e.target as HTMLInputElement).value
              page.value = 1
            }}
          />
        </label>
        {query.value ? (
          <button
            type="button"
            class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
            aria-label="清空搜索"
            onClick={() => (query.value = '')}
          >
            <IconClose class="size-4" />
          </button>
        ) : null}
      </div>
    </div>
  )

  /** 有记录、但被筛没了 —— 跟"一条都没有"是两回事，给的出路也不同。 */
  const noMatch = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <EmptyState
          title="没有符合条件的记录。"
          hint="换个状态档位，或者把搜索词清掉 —— 记录本身还在。"
          action={
            <button type="button" class="btn btn-sm btn-soft" onClick={clearFilters}>
              清除筛选
            </button>
          }
        />
      </div>
    </div>
  )

  const empty = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <EmptyState
          title="还没有任何执行记录。"
          hint="去任务中心跑一次任务，结果会按时间倒序出现在这里。"
          action={
            <button type="button" class="btn btn-primary" onClick={() => props.onGo('tasks')}>
              去任务中心
            </button>
          }
        />
      </div>
    </div>
  )

  const body = () => {
    if (runs.value.length === 0) return empty()
    if (visible().length === 0) return noMatch()

    return (
      <div class="flex flex-col gap-3">
        {filtering() ? (
          <p class="text-xs opacity-60">
            筛选出 {visible().length} 条 / 共 {runs.value.length} 条
          </p>
        ) : null}

        {/*
          ⚠⚠ 表格 / 卡片流**二选一**（`wide`），不是"两份都渲染、各用 CSS 藏一份" ✗ ——
          后者两份都真的在 DOM 里，这一页的节点数直接翻倍（2026-10-05 实测 30,124 ✗）。
        */}
        {wide.value ? (
          <div class="overflow-x-auto rounded-box bg-base-100 p-1 shadow-sm">
            <table class="table table-sm">
              <thead>
                <tr class="text-xs">
                  <th>任务</th>
                  <th>账号</th>
                  <th>状态</th>
                  <th class="text-right">耗时</th>
                  <th>时间</th>
                  <th>明细</th>
                </tr>
              </thead>
              <tbody>
                {shown().map((r) => (
                  <tr key={r.run_id} class="hover">
                    <td>
                      {/* 运行号是标识符，不是用来横向比较的列 —— 降到任务名下面当次级信息 */}
                      <div class="flex flex-col">
                        <span class="font-medium">{r.task}</span>
                        <span class="font-mono text-xs opacity-50">#{r.run_id}</span>
                      </div>
                    </td>
                    <td class="whitespace-nowrap">{accountName(r.account)}</td>
                    <td>
                      <div class="flex flex-col items-start gap-0.5">
                        <StatusChip record={r} />
                        <span class="max-w-[24rem] text-xs opacity-60">{outcomeText(r.outcome)}</span>
                      </div>
                    </td>
                    <td class="text-right whitespace-nowrap tabular-nums">
                      {fmtDuration(r.duration_ms)}
                    </td>
                    <td class="whitespace-nowrap opacity-80">{fmtEpochMs(r.started_at_ms)}</td>
                    <td>
                      <div class="flex flex-col gap-2">
                        <LogButton record={r} onOpen={(x) => void openRunLog(x)} />
                        <LeftoverCell record={r} />
                        <LiveRoomCell record={r} />
                        <ExchangeCell record={r} />
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <div class="grid gap-2">
            {shown().map((r) => (
            <div key={r.run_id} class="card bg-base-100 shadow-sm">
              <div class="card-body gap-2 p-3">
                <div class="flex items-start justify-between gap-2">
                  <div class="flex min-w-0 flex-col">
                    <span class="truncate font-medium">{r.task}</span>
                    <span class="font-mono text-xs opacity-50">#{r.run_id}</span>
                  </div>
                  <span class="shrink-0">
                    <StatusChip record={r} />
                  </span>
                </div>
                <p class="break-words text-xs opacity-70">{outcomeText(r.outcome)}</p>
                <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs opacity-60">
                  <span class="break-words">{accountName(r.account)}</span>
                  <span>{fmtEpochMs(r.started_at_ms)}</span>
                  <span>耗时 {fmtDuration(r.duration_ms)}</span>
                </div>
                <LeftoverCell record={r} />
                {/* ⚠ 桌面那列有的，移动端也要有 —— 不然后端辛苦存下来的结构化明细
                    只在宽屏看得见（用户口径：所有内容都要进记录）。 */}
                <div class="flex flex-col gap-2">
                  <LogButton record={r} onOpen={(x) => void openRunLog(x)} />
                  <LiveRoomCell record={r} />
                  <ExchangeCell record={r} />
                </div>
              </div>
            </div>
            ))}
          </div>
        )}

        {/*
          分页（2026-10-05 用户口径「任务记录分页」）：记录是**逐条看**的东西，
          而它只会越攒越多 ⇒ 一页 20 条 ✓（见 `PAGE_SIZE`）。只在真的不止一页时出现 ✓。
        */}
        {pageCount() > 1 ? (
          <div class="flex flex-wrap items-center justify-between gap-2">
            <button
              type="button"
              class="btn btn-sm btn-soft"
              disabled={curPage() <= 1}
              onClick={() => goPage(curPage() - 1)}
            >
              上一页
            </button>
            <span class="text-xs opacity-70">
              {`第 ${curPage()} / ${pageCount()} 页 · 共 ${visible().length} 条`}
            </span>
            <button
              type="button"
              class="btn btn-sm btn-soft"
              disabled={curPage() >= pageCount()}
              onClick={() => goPage(curPage() + 1)}
            >
              下一页
            </button>
          </div>
        ) : null}
      </div>
    )
  }

  return (
    <div class="flex flex-col gap-4">

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

      {state.value === 'ready' && runs.value.length > 0 ? toolbar() : null}
      {state.value === 'ready' ? body() : null}

      {/* ⚠ 内容必须写在**默认插槽**里：Vapor 下 JSX 子节点编译成第三个实参，
          而 `props.children` 永远是 undefined ✗（`ui/dialog.tsx` 文件头写着这条 ✓）。 */}
      <Dialog id={RUN_LOG_DIALOG_ID} title="运行日志">
        <p class="text-xs opacity-60">{logWho.value}</p>
        {logState.value === 'loading' ? <p class="text-sm opacity-70">正在取日志 …</p> : null}
        {logState.value === 'error' ? (
          <p class="break-words text-sm text-error">{logError.value}</p>
        ) : null}
        {logState.value === 'ready' && logLines.value.length === 0 ? (
          <p class="text-sm opacity-70">这次运行没有留下日志。</p>
        ) : null}
        <LogPanel get={() => logLines.value} />
      </Dialog>
    </div>
  )
}
