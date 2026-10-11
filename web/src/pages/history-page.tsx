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
 *
 * # 2026-10-09：运行日志弹窗改成**分页取**
 *
 * 用户口径：「大日志做虚拟加载或者分页，避免一次加载太多」✓ —— 弹窗不再借
 * `getRun()` 里那份"最近 200 行"（那是**取不到的更早的行** ✗），改成打
 * `/api/runs/{id}/logs`：首屏最近 500 行，往上翻用 `before=<第一行的 seq>` 拼 ✓。
 * 详见 `openRunLog()` / `loadEarlier()` 上面那两段（**行对象身份**与**滚动位置**
 * 那两条讲究都在那里 ✓）。
 */

import { batch, onMounted, ref, onUnmounted, type Ref } from '@asunajs/lite'
import { Dialog, openDialog } from '../ui/dialog'
import { LogPanel, type LogLine } from '../ui/log-panel'
import { setPageRefresh } from '../ui/page-refresh'
// ⚠ `ESTIMATE`：面板对**没量过**的行就是按它算高度的（`ui/virtual-rows.ts` ✓）——
// 往上拼接时要预挪滚动位置，用的必须是**同一个数**，否则两边对不上 ✓。
import { ESTIMATE } from '../ui/virtual-rows'
// ⚠ 接口那一行的类型与面板那一行**同名不同物**（前者是 `runId/level/message/atMs/seq`，
// 后者是 `kind/text/at` ✓）⇒ 这里把接口那个起个别名，免得两处混用 ✗。
import { getRunLogs, listRuns, type LogLine as ApiLogLine, type RunRecord } from '../api'
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
 * 面板里的一行 + 它在**这次运行里**的 `seq`。
 *
 * ⚠ 为什么把 `seq` 挂在行对象上，而不是另开一份 `seq[]` 与它并行：
 * 「加载更早」的游标就是**当前第一行**的 `seq`（`before` ✓），两者必须永远指向同一行 ——
 * 拆成两个数组等于有两份真相，任何一处漏更新都会静默取错页 ✗。
 * 多一个字段对 `LogPanel` 没有影响（它只读 `kind`/`text`/`at` ✓）。
 */
type RunLogRow = LogLine & { seq: number }

/**
 * 接口那四个级别 → 面板认的三档（**分类优先** ✓）。
 *
 * ⚠ `debug` 并进 `info` 是刻意的：**回看**时"调试行"与"普通行"同色是对的 ✓
 * —— 实时面板也是这么归的 ✓。两处别归得不一样 ✗（同一份日志两种颜色最费解 ✓）。
 *
 * ⚠ 分类那一格（`kind`）是 2026-10-09 才进 `mcloud_core::LogLine` 的
 * ⇒ 老记录没有它，那时按级别兜底 ✓（兜底只能分出三档，会丢掉"成功/失败"的区别 ✓）。
 */
const toLogRow = (l: ApiLogLine): RunLogRow => ({
  kind: l.kind ?? (l.level === 'error' ? 'error' : l.level === 'warn' ? 'warn' : 'info'),
  text: l.message,
  at: l.atMs,
  seq: l.seq,
})

/**
 * 首屏一次取多少行。
 *
 * ⚠ 它与实时面板的 `MAX_LOG_LINES`（2000）不是一回事，别合并：那个是"一次运行最多留多少行"
 * （内存里的上限 ✓），这个是"**一屏**先取多少"（`ui/log-panel.tsx` 的文件头解释过
 * "上限是 2000 而不是 500" 的理由 ✓）—— 分页之后，取少一点只是多翻一次页，
 * 而取太多就把"别一次加载太多"这条口径丢了 ✗。
 *
 * ⚠ 后端会拿用户设的 `logLimitPerRun` 夹一下（要多了不会报错、只会少给 ✓）⇒
 * **不能**拿"要了 500 就一定有 500"当"还有更早的"的判据 ✗（见 `fetchLogPage`）。
 */
const LOG_PAGE = 500

/**
 * 「运行日志」弹窗里**除面板之外**的那几块：谁 / 已显示 N 行 / 三个状态 / 「加载更早」。
 *
 * ⚠⚠ **它必须是一个组件**，不能把这几个 `{cond ? … : null}` 直接写在 `<Dialog>` 的
 * 插槽里 —— 这条是实测出来的，不是猜的：
 *
 * 组件插槽里的条件表达式是**在插槽函数体里直接求值**的（插槽没有占位注释可以包住它们，
 * 编译产物就是一串裸三元表达式 ✓，与"模板元素的动态子节点"不同 —— 那些会被
 * 包进 `setNodes` 的 effect ✓）。于是它们读到的信号（`logLines` / `logHasMore` …）
 * 会挂到**调用插槽的那个 effect** 上 ⇒ **每拼接一页日志都会重跑整个插槽**，
 * 而插槽里那个 `<LogPanel>` 会被**整个重建**：新滚动容器 ⇒ 滚动位置清零、
 * 打上记号的节点全丢 ✗（实测：面板 id 从 `log-panel-2` 跳到 `log-panel-4`，
 * 往上翻页之后用户看到的还是最顶上那几行 —— 也就是这次要避免的那件事本身 ✗）。
 *
 * 收进组件之后，读发生在这个组件**自己的**模板占位里（细粒度 effect ✓），
 * 兄弟节点 `LogPanel` 不再被牵连 ✓。
 * ⚠ `ui/log-panel.tsx` 文件头那条"传 thunk"的约定治的是"面板**内部**"，
 * 治不了"插槽里还读了别的信号" ✗ —— 两条要一起守。
 */
const RunLogHeader = (props: {
  /** 看的是哪一次运行。 */
  who: Ref<string>
  state: Ref<'loading' | 'ready' | 'error'>
  /** 取日志/取更早失败那句（**已经取到的行不清空** ⇒ 它可能和行同时在场 ✓）。 */
  error: Ref<string>
  /** 已取到多少行（**thunk**：这里读一下就等于订阅它，而这一读必须发生在本组件内部 ✓）。 */
  count: () => number
  hasMore: Ref<boolean>
  loadingMore: Ref<boolean>
  onLoadEarlier: () => void
}) => (
  <div class="flex flex-col gap-2">
    <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
      <p class="text-xs opacity-60">{props.who.value}</p>
      {/*
        ⚠ 「已显示 N 行」是**往上翻页到底取到了没有**唯一写在界面上的证据 ✓
        （只给按钮、不给计数的话，"点了没反应"与"真的到头了"分不开 ✗）。
      */}
      {props.state.value === 'ready' ? (
        <span class="text-xs opacity-60">已显示 {props.count()} 行</span>
      ) : null}
    </div>
    {props.state.value === 'loading' ? <p class="text-sm opacity-70">正在取日志 …</p> : null}
    {props.error.value ? <p class="break-words text-sm text-error">{props.error.value}</p> : null}
    {props.state.value === 'ready' && props.count() === 0 ? (
      <p class="text-sm opacity-70">这次运行没有留下日志。</p>
    ) : null}
    {/*
      「加载更早」放在**面板上方**：更早的行是拼到上面的（时间序 ✓），而且它在滚动容器
      外面 ⇒ 用户滚日志时它不会被滚走 ✓。⚠ 只有"上一页回满了"才给（`hasMore` ✓）——
      没有更多还摆一个按钮，点下去只是白跑一次请求 ✗。
    */}
    {props.state.value === 'ready' && props.hasMore.value ? (
      <div class="flex justify-center">
        <button
          type="button"
          class="btn btn-sm btn-soft min-h-11 md:min-h-0"
          disabled={props.loadingMore.value}
          onClick={() => props.onLoadEarlier()}
        >
          {props.loadingMore.value ? '正在取更早的日志 …' : `加载更早的 ${LOG_PAGE} 行`}
        </button>
      </div>
    ) : null}
  </div>
)

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
   * 运行日志（**回看**，分页取）。
   *
   * # 2026-10-09 改了什么（原来错在哪）
   *
   * 原来这里 `getRun(run_id)` 把**整条运行记录**取回来、只读它的 `logs`（后端最多 200 行 ✓）——
   * 两条毛病：① 那 200 行是"最近 200 行"，**更早的压根取不到**（用户要看全 ✗）；
   * ② 为了日志把整条记录（含结构化明细）也拉了一遍 ✓。
   * 现在只打 `/api/runs/{id}/logs`：首屏取**最近 500 行**，往上翻用
   * `before=<当前第一行的 seq>` 一页一页拼 ✓（用户口径：「大日志做虚拟加载或者分页，
   * 避免一次加载太多」✓）。渲染仍走 `LogPanel`（窗口化 ✓）。
   *
   * ⚠⚠ `logLines` 是**拼接**出来的数组，**已有行的对象身份必须保持不变** ——
   * `LogPanel` 的 `createFor` 靠"key 与 item 都没变"复用行节点（`ui/log-panel.tsx`
   * 与 `ui/virtual-rows.ts` 的文件头写着这条契约 ✓）：每取一页就把整批行
   * `toLogRow(...)` 重造一遍的话，**已经渲染的每一行都会被重建** ⇒
   * 用户正选中的文字与滚动位置全丢 ✗（那正是这次要避免的东西 ✓）。
   */
  const logLines = ref<RunLogRow[]>([])
  const logState = ref<'loading' | 'ready' | 'error'>('loading')
  const logWho = ref('')
  const logError = ref('')
  /** 当前看的是哪次运行（「加载更早」要用 ✓）。 */
  const logRunId = ref(0)
  /**
   * 还有没有更早的。
   *
   * ⚠ 判据**只有一条**：上一次取回来的条数 `<` 请求的条数（后端没给"总共有多少行"，
   * 也不该去猜 ✗）。⚠ 用户把 `logLimitPerRun` 设得比 `LOG_PAGE` 小时，后端会夹到那个上限
   * —— 那一页自然"不满"，于是不再给按钮 ✓，这是对的：那种设置下**那次运行的日志
   * 总共就只留了那么多行** ✓。
   */
  const logHasMore = ref(false)
  const logLoadingMore = ref(false)

  /**
   * 弹窗里那个滚动容器。
   *
   * ⚠ `ui/log-panel.tsx` 里的 id 是 `log-panel-<自增号>`（一个面板一个 id ✓）⇒ 只能按前缀找。
   * ⚠ **不去改 `log-panel.tsx`**（两页共用的件）：需要"补回滚动高度"的只有**往上翻页**
   * 这一个动作，而它只发生在这里 ✓（改共用件要连带考虑实时面板那一侧 ✗）。
   */
  const logScroller = (): HTMLElement | null =>
    document.querySelector<HTMLElement>(`#${RUN_LOG_DIALOG_ID} [id^="log-panel-"]`)

  /**
   * 取一页日志。
   *
   * ⚠ `before` 缺席 = 首屏（后端给**最后** `LOG_PAGE` 行 ✓）；给了 = 往**上**取
   * （`seq < before` 的最后 `LOG_PAGE` 行 ✓）。
   * ⚠ 它**只取**、不动 `logLines` —— 往上拼接要与滚动位置**同一批**生效，所以拼接由
   * `loadEarlier()` 在 `batch` 里做 ✓（理由见那里）。
   */
  const fetchLogPage = async (runId: number, before?: number): Promise<RunLogRow[]> => {
    const rows = await getRunLogs(
      runId,
      before == null ? { limit: LOG_PAGE } : { limit: LOG_PAGE, before },
    )
    // ⚠ 回满一页 ⇒ 可能还有更早的；不满 ⇒ 到头了（后端**唯一**给的判据 ✓，别自己猜 ✗）
    logHasMore.value = rows.length === LOG_PAGE
    // ⚠ 只把**新取回来的这一页**转成新对象（已有行的对象身份由调用方保住 ✓）
    return rows.map(toLogRow)
  }

  const openRunLog = async (r: RunRecord) => {
    /**
     * ⚠ 先把上一次的行清掉：不清的话，取新那次运行的日志**期间**面板里还挂着**上一个运行**
     * 的日志 ✗（标题写着这次是哪个运行、内容却是别次的 —— 那比空着更容易看错 ✓）。
     */
    logLines.value = []
    logState.value = 'loading'
    logWho.value = `${r.task} #${r.run_id}`
    logError.value = ''
    logHasMore.value = false
    logLoadingMore.value = false
    logRunId.value = r.run_id
    // 先开弹窗再取（网络那一下不该让界面"没反应" ✓）
    openDialog(RUN_LOG_DIALOG_ID)
    try {
      logLines.value = await fetchLogPage(r.run_id)
      logState.value = 'ready'
    } catch (e) {
      logError.value = e instanceof Error ? e.message : String(e)
      logState.value = 'error'
    }
    /**
     * 打开就把视图拉回顶部。
     *
     * ⚠ 面板实例是**跨次复用**的（这正是"弹窗插槽里只许有组件"那条的收益 ✓）⇒
     * 上一次看别的那次运行时滚到的位置会留着，而"新打开一次日志却停在中间"很难解释 ✗。
     * 第一次打开时它本来就是 0，这一句是空操作 ✓。
     */
    requestAnimationFrame(() => {
      const box = logScroller()
      if (box) box.scrollTop = 0
    })
  }

  /**
   * 往上翻一页（「加载更早的 N 行」）。
   *
   * # ⚠⚠ 拼接与滚动位置必须**同一批**生效（这是这一页最容易写错的一处）
   *
   * 面板的窗口是**按滚动位置算**的（`ui/log-panel.tsx` 的 `win()` ✓）。往**上**拼一页之后，
   * 原来那一行在数组里的下标整个往后挪了 `page.length`，它在**像素坐标**上也跟着往下挪了
   * `page.length × 行高`。于是：
   *
   * * 先拼、下一帧再挪滚动位置 ⇒ 中间那一帧按**旧**位置算窗口 ⇒ 用户正在看的那几行被判成
   *   "滚出窗口"而**摘掉**，下一帧再按新位置**重建** ✗ —— 实测过：面板里打上记号的节点
   *   全没了、id 从 `log-panel-2` 跳到 `log-panel-4`（选中文字与滚动位置一起丢 ✓，
   *   而那正是这次要避免的东西 ✗）。
   * * 所以这里用 `batch()` 把"改行数组"与"挪滚动位置"合成一批：两个信号一起落，
   *   渲染只发生**一次**，窗口直接落在**同一段内容**上 ✓（行对象身份没变 ⇒ 节点被复用 ✓）。
   *
   * ⚠ 还有两个细节，都是"不做就白做"的：
   * 1. **挪之前先给容器垫高**：此刻 DOM 还没长（这一批还没铺 ✓），直接赋一个超出当前
   *    可滚范围的 `scrollTop` 会被浏览器**钳住** ✗（用户滚得越靠下钳得越狠）。垫一层临时
   *    下边距把可滚范围撑开，挪到位后立刻撤掉 —— 全在同一帧里，用户看不到这一层 ✓。
   * 2. **`scrollTop` 的信号要同步跟上**：面板读的是它**自己的** `scrollTop` 信号，而原生
   *    `scroll` 事件是**异步**派的 ⇒ 赶不上这一批 ✗。这里手动派一个 `scroll`，让
   *    `syncScroll` 在批内把它同步过来 ✓（面板的 `onScroll` 是普通监听器 ⇒ 同步调用 ✓）。
   *
   * ⚠ 失败**不清空**已经取到的行（清掉等于把用户正在看的东西弄丢 ✗）：只把错误说出来 ✓。
   * ⚠ 错误只**就近**显示，不再套一层弹窗 —— 这里本来就在一个 `<dialog>` 里，
   * 再弹一个会与 ESC / 遮罩打架（`ui/error-dialog.ts` 文件头那条 ✓）。
   */
  const loadEarlier = async () => {
    const first = logLines.value[0]
    if (!first) return
    logLoadingMore.value = true
    logError.value = ''
    const box = logScroller()
    const was = box ? { top: box.scrollTop, height: box.scrollHeight } : null
    try {
      const page = await fetchLogPage(logRunId.value, first.seq)
      // 新行都还没量过 ⇒ 面板自己也是按 `ESTIMATE` 算它们的高度的（`ui/virtual-rows.ts` ✓）
      // ⇒ 用同一个数预挪，两边才对得上 ✓
      const added = page.length * ESTIMATE
      if (box && was) box.style.paddingBottom = `${added}px`
      batch(() => {
        logLines.value = [...page, ...logLines.value]
        if (box && was) {
          box.scrollTop = was.top + added
          box.dispatchEvent(new Event('scroll'))
        }
      })
      if (box && was) {
        box.style.paddingBottom = ''
        // 真实高度落定后再钉一次（预挪用的是估计值；通常是个空操作 ✓）
        requestAnimationFrame(() => {
          box.scrollTop = box.scrollHeight - was.height + was.top
        })
      }
    } catch (e) {
      logError.value = `取更早的日志失败：${e instanceof Error ? e.message : String(e)}`
    } finally {
      logLoadingMore.value = false
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
          而 `props.children` 永远是 undefined ✗（`ui/dialog.tsx` 文件头写着这条 ✓）。

          ⚠⚠ 这个插槽里**只许有组件**（`RunLogHeader` + `LogPanel`）—— 一个信号都不许直接读：
          插槽体里的条件表达式会挂到"调用插槽的那个 effect"上，一改就重跑整个插槽、
          把 `<LogPanel>` 整个重建（见 `RunLogHeader` 上面那段实测 ✓）。 */}
      <Dialog id={RUN_LOG_DIALOG_ID} title="运行日志">
        <RunLogHeader
          who={logWho}
          state={logState}
          error={logError}
          count={() => logLines.value.length}
          hasMore={logHasMore}
          loadingMore={logLoadingMore}
          onLoadEarlier={() => void loadEarlier()}
        />
        <LogPanel get={() => logLines.value} />
      </Dialog>

    </div>
  )
}
