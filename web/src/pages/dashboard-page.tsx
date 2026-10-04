/**
 * 总览页：**一眼看清现状**。
 *
 * 四段，从上到下回答四个问题：
 *
 * 1. **引擎条**（`engineBar`）—— 定时调度在不在跑、当下有没有任务在跑、
 *    下一次自动运行是什么时候。它是全页唯一的"整体状态"，所以占满一整行、放最上面。
 * 2. **指标卡**（`StatRow` + `metrics`）—— 今天跑得怎么样、账号凭据还撑不撑得住、
 *    定时与编排配了几条、网盘里有没有该清的东西。四格，一屏看完。
 * 3. **最近的定时任务 + 今日AI豆**（并排，2:1）—— 左边回答"接下来会自己动吗"，
 *    右边回答"今天AI豆进出多少"。引擎条只说"循环在不在跑"，前者的细节由这一段给。
 * 4. **最近运行**（`recentRuns`）—— **只取最近 5 条**。
 *
 * # 为什么最近运行只取 5 条
 *
 * 这一页以前把 `/api/runs` 返回的**全部**记录（后端上限 100 条）铺成一张表 ⇒
 * 总览成了第二个历史页，第一屏被几十行表格顶下去，"一眼看清现状"反而做不到。
 * 看全量是历史与日志页的事，这里给入口（「查看全部」）就够。
 *
 * # 数据口径：只用现有接口能拿到的
 *
 * 四张卡各有出处，**没有一格是前端推断的**：
 *
 * | 卡 | 出处 |
 * |---|---|
 * | 今日运行 | `/api/runs`（`started_at_ms` 落在本地今天） |
 * | 账号凭据 | `/api/accounts` 的 `expire`（epoch 毫秒；`0` = 解析不出） |
 * | 定时与编排 | `/api/schedules` + `/api/status` 的 `pipelineCount` |
 * | 网盘遗留 | `/api/runs` 的 `details.artifacts` |
 *
 * ⚠ `/api/runs` **最多回 100 条**（`store.runs(100)`）⇒ 今天真的跑过 100 次以上时
 * 「今日运行」会少算。本项目的定时粒度是分钟级，一天上百次属于配置已经不正常，
 * 所以没有为它单独加一个"今日统计"接口 —— 这条口子写在这里，不假装它不存在。
 *
 * # 不做轮询
 *
 * 取数时机只有两个：进页面、点「刷新」。数据**年龄**由「更新于 22:31」直接摆出来 ——
 * 比每分钟自刷一次更诚实：自刷会让人以为那个时间戳在动，其实只是又采了一次样。
 *
 * # ⚠ 写 `load()` 的顺序不能动：`state` 最后写
 *
 * lite 没有重渲染（`docs/web/lite-guide.md` §6.1），`state.value === 'ready'` 那条分支
 * 只在 `state` 变化时重铺一次，铺的时候就把 `status` / `runs` / … 读成了**那一刻的值**。
 * 所以五个信号必须先写完、再置 `state`；写反了各卡会永远停在上一轮 ——
 * 而它不报错，只是数字不动。
 */

import { onMounted, onUnmounted, ref } from 'lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  getBeanRecords,
  getBeanToday,
  getStatus,
  listAccounts,
  listRuns,
  listSchedules,
  type AccountSummary,
  type BeanRecord,
  type BeanRecords,
  type BeanToday,
  type RunRecord,
  type Schedule,
  type ServiceStatus,
} from '../api'
import { accountName, rememberAccountNames } from '../account-names'
import { StatRow, type StatBadge, type StatItem } from '../ui/stat-row'
import { toneClass } from '../ui/badge'
import { Dialog, openDialog } from '../ui/dialog'
import { EmptyState } from '../ui/empty-state'
import { fmtClock, fmtDuration, fmtEpochMs, fmtUptime } from '../ui/time'
import { outcomeText, statusBadge, statusLabel } from '../ui/history'
import type { PageId } from '../ui/nav'

export interface DashboardPageProps {
  onGo: (id: PageId) => void
}

/** 三态显式声明：加载中 / 出错 / 就绪。 */
type LoadState = 'loading' | 'error' | 'ready'

/** 总览只展示最近这几条运行 —— 全量在历史页。 */
const RECENT_LIMIT = 5

/** 总览只列接下来要跑的那几条定时 —— 全量（含停用的）在定时页。 */
const UPCOMING_LIMIT = 3

/**
 * 凭据"快到期"的门槛（天）。
 *
 * 7 天是"发现了还来得及找时间重新登录"的量级；与账号页把到期时间摆在卡片上同口径，
 * 只是这里要提前吼一声，所以门槛必须**明显大于**用户随手打开这一页的间隔。
 */
const EXPIRY_WARN_DAYS = 7

const DAY_MS = 86_400_000

/** 明细弹窗的 id（`openDialog` 与 `<Dialog>` 两边共用 ✓）。 */
const BEAN_DETAIL_DIALOG_ID = 'bean-records'

/**
 * 明细的窗口档位。`days` 与 `GET /api/beans/records?days=` **同一个口径**：
 * **自然日**，`1` = 今天（东八区日界起），`0` = 全部。
 *
 * ⚠ 档位写在这里而不是散在按钮上：`days` 是要发给后端的**值**，
 * 摆在一起才能一眼看出"界面上这几档"与"后端认哪几档"是同一组。
 */
const BEAN_RANGES = [
  { days: 1, label: '今日' },
  { days: 7, label: '近 7 天' },
  { days: 30, label: '近 30 天' },
  { days: 0, label: '全部' },
] as const

/**
 * 点开明细时的默认档位 = 「近 7 天」。
 *
 * ⚠ **不是「今日」**：总览那张卡上摆的已经是今天的净增，点开再看一遍等于点了个寂寞；
 * 「不限于今日」正是这个入口存在的理由（用户口径 2026-10-04）。要看今天，
 * 弹窗里第一档就是。
 */
const BEAN_DEFAULT_DAYS = 7

// ── 纯函数：把接口数据算成卡上的数 ────────────────────────────────────
//
// 都写成纯函数（`now` 从外面传）而不是在 JSX 里就地算：
// 一是可读，二是"今日""还剩几天"这类判断依赖当前时刻，集中一处才好复核。

/** 本地时区"今天 0 点"的毫秒时间戳 —— 「今日」按用户所在时区算，不按 UTC。 */
const startOfTodayMs = (now: number): number => {
  const d = new Date(now)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

interface TodayRuns {
  total: number
  success: number
  /** 部分成功：有成有败（`TaskOutcome::Partial`）。⚠ 别并进 `success` ✗ */
  partial: number
  skipped: number
  failed: number
}

/** 今天的运行统计。按**开始**时刻归日：跨零点跑的那一次算在开始的那一天。 */
const todayRuns = (runs: RunRecord[], now: number): TodayRuns => {
  const from = startOfTodayMs(now)
  const t: TodayRuns = { total: 0, success: 0, partial: 0, skipped: 0, failed: 0 }
  for (const r of runs) {
    if (r.started_at_ms < from) continue
    t.total++
    t[r.outcome.status]++
  }
  return t
}

/** 「未知」（`expire <= 0`）**单独一档**：它不是"有效"，也不是"过期"。 */
interface AccountHealth {
  total: number
  expired: number
  soon: number
  unknown: number
}

const accountHealth = (list: AccountSummary[], now: number): AccountHealth => {
  const h: AccountHealth = { total: list.length, expired: 0, soon: 0, unknown: 0 }
  for (const a of list) {
    if (a.expire <= 0) {
      h.unknown++
    } else if (a.expire <= now) {
      h.expired++
    } else if (a.expire - now <= EXPIRY_WARN_DAYS * DAY_MS) {
      h.soon++
    }
  }
  return h
}

interface Leftover {
  /** 还留在网盘上的文件个数。 */
  files: number
  /** 留下这些东西的运行次数（一次可能留好几个文件）。 */
  runs: number
}

const leftoverStat = (runs: RunRecord[]): Leftover => {
  let files = 0
  let n = 0
  for (const r of runs) {
    const arts = r.details?.artifacts ?? []
    if (arts.length === 0) continue
    files += arts.length
    n++
  }
  return { files, runs: n }
}

/** 下一次自动运行。**只认启用的定时** —— 没启用就没有"下次"。 */
const nextSchedule = (list: Schedule[]): Schedule | null => {
  let best: Schedule | null = null
  let bestAt = Number.POSITIVE_INFINITY
  for (const s of list) {
    if (!s.enabled || !s.nextFireAt) continue
    if (s.nextFireAt < bestAt) {
      bestAt = s.nextFireAt
      best = s
    }
  }
  return best
}

/**
 * 接下来会跑的那几条，最靠前的在前。
 *
 * ⚠ 判据与 [`nextSchedule`] 同一把尺子（`enabled && nextFireAt`）：没启用的定时
 * 后端给的 `nextFireAt` 就是 `null`（见 `api.ts` 那条注释），不挡住它就会把
 * "永远不触发"排到最前面 —— 而那正是这一格要回答的反面。
 */
const upcomingSchedules = (list: Schedule[], limit: number): Schedule[] =>
  list
    .filter((s) => s.enabled && s.nextFireAt)
    .sort((a, b) => (a.nextFireAt ?? 0) - (b.nextFireAt ?? 0))
    .slice(0, limit)

/**
 * 「还有多久」。
 *
 * 这一格的全部意义就是"是不是马上要跑了"，所以给相对值；绝对时刻在下面另起
 * 一行摆着 —— 两者不互相替代（相对值看不出是几点，绝对时刻要心算差值）。
 */
const fmtUntil = (at: number, now: number): string => {
  const left = at - now
  if (left <= 0) return '即将触发'
  const mins = Math.floor(left / 60_000)
  if (mins < 1) return '不到 1 分钟'
  if (mins < 60) return `${mins} 分钟后`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours} 小时后`
  return `${Math.floor(hours / 24)} 天后`
}

/**
 * 这条定时**上一次触发**的结局。
 *
 * 与运行记录同口径：`skipped` 不染红也不染黄（活动没开、条件不满足都是正常结局）。
 * `lastStatus` 为 `null` 表示还没触发过 —— 也如实说出来，不装作"一切正常"。
 */
const lastRunBadge = (s: Schedule): StatBadge => {
  if (s.lastStatus === 'ok') return { text: '上次成功', tone: 'ok' }
  if (s.lastStatus === 'error') return { text: '上次失败', tone: 'bad' }
  if (s.lastStatus === 'skipped') return { text: '上次跳过', tone: 'plain' }
  return { text: '还没跑过', tone: 'plain' }
}

// ── 徽章：写的是"这一格现在的处境"，不是把数值再说一遍 ────────────────

const todayBadge = (t: TodayRuns): StatBadge => {
  if (t.total === 0) return { text: '暂无记录', tone: 'plain' }
  if (t.failed > 0) return { text: `${t.failed} 次失败`, tone: 'bad' }
  // 部分成功**必须单独说**：并进"全部成功"就是把没成的那些盖住 ✗
  // （2026-10-03 用户实测：「下面的 614 失败还直接说成功」）。
  if (t.partial > 0) return { text: `${t.partial} 次部分成功`, tone: 'warn' }
  // ⚠ 跳过**不染黄**：活动没开/条件不满足是正常结局，不是待处理项
  // （与 `ui/history.ts` 里 skipped 用中性徽章同一条口径）。
  if (t.skipped > 0) return { text: `${t.skipped} 次跳过`, tone: 'plain' }
  return { text: '全部成功', tone: 'ok' }
}

const accountBadge = (h: AccountHealth): StatBadge => {
  if (h.total === 0) return { text: '未配置', tone: 'plain' }
  if (h.expired > 0) return { text: `${h.expired} 个已过期`, tone: 'bad' }
  if (h.soon > 0) return { text: `${h.soon} 个将到期`, tone: 'warn' }
  if (h.unknown > 0) return { text: `${h.unknown} 个未知`, tone: 'plain' }
  return { text: '凭据正常', tone: 'ok' }
}

const accountDesc = (h: AccountHealth): string => {
  if (h.total === 0) return '去账号页登录一个'
  if (h.expired > 0) return '需重新登录该账号'
  if (h.soon > 0) return '建议尽快刷新凭据'
  if (h.unknown > 0) return '到期时间解析不出'
  return `${EXPIRY_WARN_DAYS} 天内无到期`
}

export const DashboardPage = (props: DashboardPageProps) => {
  const state = ref<LoadState>('loading')
  /** 整页加载失败（整页替换）。与动作失败分开 —— 共用一个 ref 会让动作失败什么都不显示。 */
  const errorMessage = ref('')
  const status = ref<ServiceStatus | null>(null)
  const runs = ref<RunRecord[]>([])
  const schedules = ref<Schedule[]>([])
  const accounts = ref<AccountSummary[]>([])
  /**
   * 今日AI豆（云朵记录汇总）。
   *
   * ⚠ 2026-10-03 更正：端点**已经落地**（`crates/mcloud-server/src/routes/beans.rs`，
   * 见 review §3 D7 / §5 C5）—— 原先这段写「后端还没有」，会把下一个排查的人
   * 引向"有一发白发的请求"。现在读到 `null` 就是**真的读不到**，
   * 原因在 `beansError` 里（未登录 401 / 上游 502 / 解析失败那一族）。
   */
  const beans = ref<BeanToday | null>(null)
  /** 云朵记录读不到的原因。**单独一格** —— 它不该把整页打成"后端连不上"。 */
  const beansError = ref('')
  /**
   * 上一次取数**成功**的时刻。
   *
   * 失败时它不动 —— 那一刻界面上的数据确实还是上一份，把时间推到现在就是在说
   * "这份数据是刚拉的"。
   */
  const loadedAt = ref(0)
  /**
   * 页面上的"此刻"，只服务「运行时间」那一格。
   *
   * 单独一个信号、而不是渲染时到处 `Date.now()`：lite 的绑定只在**信号**变化时重算，
   * 直接读时钟的表达式算一次就冻住 —— 而那一格显示的就是"过去了多久"。
   */
  const nowTick = ref(Date.now())
  /** 「运行时间」的定时器句柄。组件卸载即停（下面 `onUnmounted`）。 */
  let ticker: ReturnType<typeof setInterval> | undefined

  // ── AI豆明细弹窗（**不限于今日**）──────────────────────────────────────
  //
  // 点开某个账号看"这些数是怎么来的"。⚠ 读的是**上游流水**
  // （`GET /api/beans/records` → 上游 `cloudRecordV2`），**不是**本仓的运行记录 /
  // 明细账本 —— 后者是"我们做过什么"，流水是"上游账上发生了什么"，
  // 两者对不上是常态（重试、手动操作、别的端），这一格要回答的是后者。
  //
  // 四个信号各管一件事：账号是谁 / 看哪一档 / 数据 / 三态。
  // **数据与错误分开**：共用一个 ref 会让"上次的数据 + 这次的错误"混着显示。
  const beanDetailAccount = ref('')
  const beanDetailDays = ref<number>(BEAN_DEFAULT_DAYS)
  const beanDetail = ref<BeanRecords | null>(null)
  const beanDetailState = ref<LoadState>('loading')
  const beanDetailError = ref('')

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      // 四个端点并发取。**任一失败就是整页错误**，不做"部分成功"：
      // 缺一角时凑出来的数字等于替后端下一个它没给过的结论
      // （最典型的"0 个账号"，真相往往是那一次请求没回来）。
      //
      // ⚠ 第 5 条（今日AI豆）**故意破了这条规矩**：它自己 `.catch` 成一个值，
      // 所以永远不会让这个 `Promise.all` 拒绝。理由：它只是**一格读数**，
      // 而整页错误态的语义是"后端连不上"—— 让一格读数把整页打成红屏，
      // 是把"这一格还没有"说成了"服务挂了"。它读不到时由卡片自己说清楚。
      const [s, r, sch, acc, bean] = await Promise.all([
        getStatus(),
        listRuns(),
        listSchedules(),
        listAccounts(),
        getBeanToday()
          .then((b) => ({ ok: true as const, bean: b }))
          .catch((e: unknown) => ({
            ok: false as const,
            message: e instanceof Error ? e.message : '读取失败',
          })),
      ])
      status.value = s
      runs.value = r
      schedules.value = sch
      accounts.value = acc
      if (bean.ok) {
        beans.value = bean.bean
        beansError.value = ''
      } else {
        beans.value = null
        beansError.value = bean.message
      }
      // 顺手把"账号主键 → 名字"的缓存填上：运行记录里存的是哈希主键，
      // 不填的话列表那一列全是认不出的十六进制。
      // ⚠ 用**同一份**列表，而不是再调 `refreshAccountNames()` 发第二次请求 ——
      // 两次请求可能一次成一次败，那样指标卡与列表会各说一套。
      rememberAccountNames(acc)
      loadedAt.value = Date.now()
      // ⚠ 必须最后写。理由见文件头那段"写 `load()` 的顺序不能动"。
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => {
  setPageRefresh({ run: () => void load() })
    void load()
    // 「运行时间」得自己往前走：这一格显示的正是"已经过去多久"，
    // 只在取数那一刻算一次的话，页面开着十分钟它还写「2 分」—— 那比不显示更糟。
    // 30 秒一格够用（显示精度到分钟），且**不碰网络**（不是轮询）。
    ticker = setInterval(() => {
      nowTick.value = Date.now()
    }, 30_000)
  })
  onUnmounted(() => {
    setPageRefresh(null)
    clearInterval(ticker)
  })

  // ── 引擎条 ────────────────────────────────────────────────────────────

  /**
   * 状态点。
   *
   * 绿 = 调度在跑，灰 = 没在跑，**不用红** —— 这一页红的语义是"后端连不上"
   * （见 `app.tsx` 侧栏那一行），拿它表示"调度没开"会让人以为服务挂了、跑去重启。
   *
   * 在跑时外面套一圈**扩散**（`animate-ping`）：静态绿点看不出"是活的"，
   * 而这一条的全部意思就是"有个程序正在跑"。
   * ⚠ **不复用侧栏那颗点的 `animate-pulse`**：那里 pulse 的语义是"还在问后端"（pending），
   * 两个语义共用一个动画会让人以为引擎也在加载。
   * `motion-reduce:animate-none` 给"减少动态效果"的用户留一个安静的绿点 —— 信息量不减。
   */
  const engineDot = () => (
    <span class="relative inline-flex size-2" aria-hidden="true">
      {status.value?.schedulerRunning ? (
        <span class="status status-success absolute animate-ping motion-reduce:animate-none" />
      ) : null}
      <span
        class={
          status.value?.schedulerRunning ? 'status status-success relative' : 'status status-neutral relative'
        }
      />
    </span>
  )

  /**
   * 「运行时间」。
   *
   * ⚠ 读 `nowTick` 而不是 `Date.now()`：这个绑定只在**信号**变化时重算，
   * 直接调 `Date.now()` 会永远停在取数那一刻 —— 而"不再增长"恰恰是这一格要否定的东西。
   */
  const uptimeText = () => {
    const s = status.value
    if (!s) return '—'
    return fmtUptime(nowTick.value - s.startedAtMs)
  }

  /**
   * ⚠ 只有**没在跑**那一支换了措辞：`schedulerRunning` 说的是**调度循环**的实况，
   * 而手动跑任务走的是另一条路（`POST /api/runs`）—— 所以那里写「定时调度未运行」，
   * 不写"引擎停了"（那会冤枉手动运行这条仍然好用的路）。
   */
  const engineHead = () => (status.value?.schedulerRunning ? '任务引擎守护中' : '定时调度未运行')

  const engineSub = () => {
    const s = status.value
    if (!s) return ''
    if (!s.schedulerRunning) return '手动跑任务不受影响；定时任务不会自动触发'
    return s.runningCount > 0
      ? `已注册 ${s.taskCount} 个任务 · 正在运行 ${s.runningCount} 个`
      : `已注册 ${s.taskCount} 个任务 · 当前空闲`
  }

  /**
   * 「哪个定时 · 什么时候」。只给时间的话用户还得回定时页对是哪一条。
   *
   * ⚠ 它与下面「最近的定时任务」第一行**故意重合**：这一条是**汇总**
   * （抬头一眼看到"下一次什么时候"），那一段是**细节**（还有哪几条、上次跑成没成）。
   * 拿掉这里的话，条子右边会空着 —— 而"会不会自己动"正是这一条要回答的问题。
   */
  const nextTaskText = () => {
    // ⚠ 调度循环没在跑时，`nextFireAt` 是那一刻算出来的**残留值**。把它摆出来
    // 就是在承诺一件不会发生的事（下面那条 warn 明说了不会触发），所以直接改口径。
    if (status.value && !status.value.schedulerRunning) return '不会触发'
    const s = nextSchedule(schedules.value)
    if (!s || !s.nextFireAt) return '暂无计划'
    return `${s.name} · ${fmtEpochMs(s.nextFireAt)}`
  }

  const engineBar = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body flex-row flex-wrap items-center justify-between gap-4 p-4">
        <div class="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1">
          {engineDot()}
          <div class="flex min-w-0 flex-col">
            <span class="font-medium">{engineHead()}</span>
            <span class="break-words text-xs opacity-70">{engineSub()}</span>
          </div>
          {/*
            运行时间紧挨着「守护中」那句话 —— 它就是那句话的证据：
            时长的**增长**伪造不了（状态点只看当下，看不出"是刚起来还是跑了三天"）。
            ⚠ 它是全页唯一会自己动的数字（每 30 秒重算），其余都是取数那一刻的快照。
            竖线只在 ≥md 出现：窄屏换行后那一竖会孤零零地飘在行首。
          */}
          <div class="flex items-baseline gap-1 md:border-l md:border-base-300 md:pl-4">
            <span class="text-xs opacity-60">运行时间</span>
            <span class="text-sm font-medium">{uptimeText()}</span>
          </div>
        </div>
        <div class="flex flex-col gap-0.5 md:items-end">
          <span class="text-xs opacity-60">下次任务</span>
          <span class="break-words font-medium">{nextTaskText()}</span>
        </div>
      </div>
    </div>
  )

  // ── 四张指标卡 ────────────────────────────────────────────────────────

  const metrics = (): StatItem[] => {
    const now = Date.now()
    const today = todayRuns(runs.value, now)
    const health = accountHealth(accounts.value, now)
    const left = leftoverStat(runs.value)
    const enabled = schedules.value.filter((s) => s.enabled).length

    return [
      {
        title: '今日运行',
        value: `${today.total} 次`,
        badge: todayBadge(today),
        // ⚠ `partial` 有数就要露出来：`value` 用的是 `total`，
        // 这里只写"成功/失败"的话那些运行在明细里就凭空消失了 ✗
        desc:
          today.total === 0
            ? '今天还没跑过任务'
            : `成功 ${today.success} · 失败 ${today.failed}${
                today.partial > 0 ? ` · 部分成功 ${today.partial}` : ''
              }`,
      },
      {
        title: '账号凭据',
        value: `${health.total} 个`,
        badge: accountBadge(health),
        desc: accountDesc(health),
      },
      {
        title: '定时与编排',
        value: `${schedules.value.length} 条`,
        // 一条都没配时说「启用 0」等于让人对着一个空集合读一个数；直接说"未配置"。
        badge:
          schedules.value.length === 0
            ? { text: '未配置', tone: 'plain' }
            : { text: `启用 ${enabled}`, tone: 'plain' },
        desc: `任务编排 ${status.value?.pipelineCount ?? 0} 条`,
      },
      {
        title: '网盘遗留',
        value: `${left.files} 个`,
        // 遗留文件是**要人动手**的：链尾没清干净时它会一直堆在用户网盘里。
        badge: left.files > 0 ? { text: '需清理', tone: 'warn' } : { text: '无遗留', tone: 'ok' },
        desc: left.files > 0 ? `${left.runs} 次运行留下` : '未发现残留文件',
      },
    ]
  }

  // ── 最近的定时任务 ────────────────────────────────────────────────────
  //
  // 「接下来会不会自己动」是这一页另一半的答案（引擎条只说"循环在不在跑"）。
  // 只列启用的前几条：停用的、以及第 4 条之后的，都在定时页。

  const upcomingPanel = () => {
    const rows = upcomingSchedules(schedules.value, UPCOMING_LIMIT)
    return (
      <div class="card bg-base-100 shadow-sm md:col-span-2">
        <div class="card-body">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">最近的定时任务</h2>
            <button
              type="button"
              class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
              onClick={() => props.onGo('schedules')}
            >
              管理定时
            </button>
          </div>

          {rows.length === 0 ? (
            <EmptyState
              title="没有启用中的定时任务。"
              hint="去定时页按 cron 配一条，到点会自动跑 —— 这里就会显示它下次什么时候跑。"
              action={
                <button type="button" class="btn btn-primary" onClick={() => props.onGo('schedules')}>
                  去定时页
                </button>
              }
            />
          ) : (
            <ul class="flex flex-col gap-2">
              {rows.map((s) => (
                <li key={s.id} class="rounded-box bg-base-200 p-3">
                  <div class="flex flex-wrap items-center justify-between gap-2">
                    <span class="break-words font-medium">{s.name}</span>
                    {/* 徽章给的是**相对**时间 —— "还有多久"是这一行要回答的问题 */}
                    <span class="badge badge-soft badge-sm">
                      {fmtUntil(s.nextFireAt ?? 0, Date.now())}
                    </span>
                  </div>
                  <p class="mt-1 break-words text-xs opacity-70">
                    {s.task} · {s.expr} · {s.accounts.length} 个账号
                  </p>
                  <div class="mt-1 flex flex-wrap items-center gap-2">
                    <span class={`badge badge-sm ${toneClass(lastRunBadge(s).tone)}`}>
                      {lastRunBadge(s).text}
                    </span>
                    <span class="text-xs opacity-60">
                      下次 {fmtEpochMs(s.nextFireAt)}
                      {s.lastRunMs ? ` · 上次 ${fmtEpochMs(s.lastRunMs)}` : ''}
                    </span>
                  </div>
                  {/* 上次失败时说清**为什么** —— 只有状态词的话用户还得去定时页翻。 */}
                  {s.lastStatus === 'error' && s.lastError ? (
                    <p class="mt-1 break-words text-xs opacity-70">{s.lastError}</p>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    )
  }

  // ── 今日AI豆（**按账号**） ────────────────────────────────────────────
  //
  // 读数来自**云朵记录**（上游 `cloudRecordV2` 的收支流水）—— 它回答的是
  // "今天进出多少"，不是"现在还剩多少"（余额是另一条口径，要另打一枪 `infoV3`）。
  //
  // ⚠⚠ **一个账号一份，绝不合计**：豆是按账号记的，加总出来的数**哪个账号都用不了**
  // （豆不能跨账号花）。第一版按"总AI豆"设计是错的 —— 这一格的形状就是为了
  // 让"哪个号今天进了多少"一眼对上号。
  //
  // ⚠ 端点**早已落地**（`crates/mcloud-server/src/routes/beans.rs`，见
  // `docs/task-inventory.md` §5）—— 读到 `null` 就是**真的读不到**，
  // 原因在 `beansError` 里（未登录 401 / 上游 502 / 解析失败那一族）。

  /** 净增带符号：`+30` / `-200`。"今天扣了豆"和"今天没动"必须一眼分得开。 */
  const fmtSigned = (n: number): string => (n > 0 ? `+${n}` : String(n))

  // ── 明细：点开某个账号看**逐笔流水**（不限于今日）────────────────────

  /** 当前档位的人话（`0` = 全部 ⇒ 不写"近 0 天"）。 */
  const beanRangeLabel = (): string =>
    BEAN_RANGES.find((r) => r.days === beanDetailDays.value)?.label ?? `${beanDetailDays.value} 天`

  /** 弹窗标题。账号名走 `accountName`（哈希不上界面 ✓）。 */
  const beanDetailTitle = (): string => `${accountName(beanDetailAccount.value)} · AI豆明细`

  /**
   * 取这一档的明细。
   *
   * 失败**不退回空列表**：后端读不到上游时给的是 4xx/5xx，这里如实进 `error` 态 ——
   * 显示成"没有流水"是把"我不知道"说成了"没有"（与那张卡的 `failed` 同一条道理）。
   */
  const loadBeanDetail = async () => {
    beanDetailState.value = 'loading'
    beanDetailError.value = ''
    try {
      beanDetail.value = await getBeanRecords(beanDetailAccount.value, beanDetailDays.value)
      beanDetailState.value = 'ready'
    } catch (e) {
      beanDetailError.value = e instanceof Error ? e.message : '读取失败'
      beanDetailState.value = 'error'
    }
  }

  /**
   * 点某一行账号 ⇒ 开弹窗 + 取数。
   *
   * ⚠ **先开弹窗再取数**（与历史页「运行日志」同一条）：那一下是网络请求，
   * 等它回来再开窗的话，用户点完会有几百毫秒"没反应"。
   */
  const openBeanDetail = (account: string) => {
    beanDetailAccount.value = account
    beanDetailState.value = 'loading'
    beanDetailError.value = ''
    // 清掉上一个账号的数据：换号时"旧号的流水 + 新号的标题"是最坏的一种显示。
    beanDetail.value = null
    openDialog(BEAN_DETAIL_DIALOG_ID)
    void loadBeanDetail()
  }

  /** 切档位：**弹窗不关**，只把这一档的数据取回来。 */
  const switchBeanRange = (days: number) => {
    if (beanDetailDays.value === days) return
    beanDetailDays.value = days
    void loadBeanDetail()
  }

  /** 一笔流水的时间：归一得出就按服务端口径格式化，否则**照原样摆出来**。 */
  const beanRecordTime = (r: BeanRecord): string => {
    if (r.atMs) return fmtEpochMs(r.atMs)
    if (r.rawTime) return `${r.rawTime}（时间未识别）`
    return '上游没给时间'
  }

  /**
   * 明细抬头那句话。
   *
   * ⚠ `hasMore` 时**不能**把"收/支"说成这一档的合计 —— 那一份是截断过的；
   * 所以措辞换成"已列出最近 N 笔（还有更多没读到）"。
   */
  const beanDetailSummary = (): string => {
    const d = beanDetail.value
    if (!d) return ''
    const gain = d.records.filter((r) => r.num > 0).reduce((s, r) => s + r.num, 0)
    const spent = d.records.filter((r) => r.num < 0).reduce((s, r) => s - r.num, 0)
    const head = d.hasMore
      ? `已列出最近 ${d.records.length} 笔（还有更多没读到）`
      : `共 ${d.records.length} 笔`
    const undated = d.undated > 0 ? ` · ${d.undated} 笔时间读不出，已按原样列出` : ''
    return `${head} · 收 ${gain} · 支 ${spent}${undated}`
  }

  /** 一句话说清这一格的口径（数据从哪来、窗口怎么算）。 */
  const beanDetailHint = (): string =>
    `窗口按东八区自然日算（${beanRangeLabel()}）· 数据来自上游流水，不是本机的运行记录`

  /** 弹窗正文：三态 + 「这一档没有流水」。 */
  const beanDetailBody = () => {
    if (beanDetailState.value === 'loading') {
      return <p class="text-sm opacity-70">正在读上游流水 …</p>
    }
    if (beanDetailState.value === 'error') {
      return (
        <div class="flex flex-col items-start gap-2">
          <p class="break-words text-sm text-error">没读到：{beanDetailError.value}</p>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void loadBeanDetail()}>
            重试
          </button>
        </div>
      )
    }
    const d = beanDetail.value
    if (!d) return <p class="text-sm opacity-70">还没取到明细</p>
    if (d.records.length === 0) {
      return <p class="text-sm opacity-70">这一档没有流水。</p>
    }
    return (
      <div class="flex flex-col gap-2">
        <p class="text-xs opacity-60">{beanDetailSummary()}</p>
        {d.records.map((r, i) => (
          <div key={i} class="flex items-start justify-between gap-3 rounded-box bg-base-200 p-3">
            <div class="flex min-w-0 flex-col">
              <span class="break-words text-sm font-medium">{r.summary || '（上游没给摘要）'}</span>
              <span class="break-words text-xs opacity-60">
                {beanRecordTime(r)}
                {r.mark ? ` · ${r.mark}` : ''}
              </span>
            </div>
            {/* 进出用**符号 + 颜色**两条线说：只给颜色的话，色觉障碍的用户分不出哪笔是扣豆。 */}
            <span class={`shrink-0 font-medium ${r.num < 0 ? 'text-error' : 'text-success'}`}>
              {fmtSigned(r.num)}
            </span>
          </div>
        ))}
      </div>
    )
  }

  /** 徽章先说**这份读数可不可信**（有账号没读到 ⇒ 它是不完整的），再说账号数。 */
  const beanBadge = (): StatBadge => {
    // ⚠ 写「没读到」而不是原来的「待接入」：端点**早已落地**（见本段上面的说明），
    // 所以这一格非空就是**真读不到**（未登录 / 上游 502 / 解析失败那一族）——
    // 「待接入」会把一次真故障说成"功能还没做"，而它旁边现在就是「查看明细」入口。
    if (beansError.value) return { text: '没读到', tone: 'warn' }
    const b = beans.value
    if (!b) return { text: '无读数', tone: 'plain' }
    if (b.failed.length > 0) return { text: `${b.failed.length} 个账号没读到`, tone: 'warn' }
    if (b.accounts.length === 0) return { text: '未配置账号', tone: 'plain' }
    if (b.accounts.every((a) => a.count === 0)) return { text: '今日无流水', tone: 'plain' }
    return { text: `${b.accounts.length} 个账号`, tone: 'plain' }
  }

  /**
   * 卡片正文。三种长相：**没接上**（把后端原话透出来）/ **一个账号都没配** /
   * **一个账号一行**。
   *
   * 读不到的账号**单独列出来、并写出原因** —— 不能按 0 混进它自己那一行：
   * 「没读到」与「今天没进出」是两件事，混了就是把"我不知道"说成了"没有"。
   */
  const beansBody = () => {
    if (beansError.value) {
      // 错误文本一律原样透出（不改写）：这样一眼认得出是哪一类（401 / 502 / 解析）。
      // ⚠ 措辞是「读不到」而不是「未接入」：端点早已落地（见本段上面的说明），
      // 这里非空就**是真读不到** —— 说成"没接上"会把故障读成"功能还没做"。
      // ⚠ 必须显式允许换行 —— `.stat-desc` 自带 `nowrap`，
      // 长句子会顶破卡片、在窄屏上顶出横向滚动条（1280 档实测过）。
      return (
        <div class="stat-desc whitespace-normal break-words">
          云朵记录读不到（{beansError.value}）
        </div>
      )
    }
    const b = beans.value
    if (!b) return <p class="text-xs opacity-70">还没取到读数</p>
    if (b.accounts.length === 0 && b.failed.length === 0) {
      return <p class="text-xs opacity-70">还没有账号 —— 去账号页登录一个。</p>
    }
    return (
      <div class="flex flex-col gap-2">
        {b.accounts.map((a) => (
          /*
            ⚠ 整行是一个 **`<button>`**，不是"div + 里面一个小链接"：
            这一行的全部意义就是"点开看明细"，把可点区域收成一个小链接，
            在窄屏上会变成 44px 触控目标的反面教材（规范 §6.1）。
            `text-left` 不能省 —— 按钮默认居中，那会让流水数字与账号名对不齐。
            ⚠⚠ `cursor-pointer` 也**不能省**：这一行不是 `btn`/`dock>button` 那套
            组件类，而 Tailwind v4 的 preflight **不再给 `<button>` 加
            `cursor: pointer`**（v3 有 ✓）⇒ 少了它，鼠标移上去是**箭头**，
            整行"可点"这件事就说不出来了（产物实测：CSS 里只有 `.btn` /
            `.dock>button` / `.menu li>button:hover` 这些组件类才给手型 ✓）。
          */
          <button
            key={a.account}
            type="button"
            class="cursor-pointer rounded-box w-full bg-base-200 p-3 text-left transition-colors hover:bg-base-300"
            onClick={() => openBeanDetail(a.account)}
            aria-label={`查看 ${accountName(a.account)} 的AI豆明细`}
          >
            <div class="flex flex-wrap items-center justify-between gap-2">
              <span class="min-w-0 break-words text-sm font-medium">{accountName(a.account)}</span>
              <span class="font-medium">{fmtSigned(a.net)}</span>
            </div>
            <p class="mt-0.5 text-xs opacity-60">
              收 {a.gained} · 支 {a.spent} · {a.count} 笔
              <span class="ml-1 opacity-80">查看明细 ›</span>
            </p>
          </button>
        ))}
        {b.failed.map((f) => (
          <div key={f.account} class="rounded-box bg-base-200 p-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <span class="min-w-0 break-words text-sm font-medium">{accountName(f.account)}</span>
              <span class="badge badge-soft badge-warning badge-sm">没读到</span>
            </div>
            <p class="mt-0.5 break-words text-xs opacity-60">{f.error}</p>
          </div>
        ))}
      </div>
    )
  }

  const beansPanel = () => (
    /*
      `md:self-start`：不拉伸的话栅格会把它拉到与左邻同高，内容顶在上面、下面空一大片。
      内容高度 + 标题与左邻标题对齐，那片空白就没了（窄屏本来就是单列，无影响）。

      排版复用指标卡那套（`stat-title` / `stat-desc`）：同一屏里"小灰标题 + 小灰说明"
      只该有一套长相。
    */
    <div class="card bg-base-100 shadow-sm md:self-start">
      <div class="card-body">
        <div class="stat-title flex items-center justify-between gap-2">
          <span>今日AI豆</span>
          <span class={`badge badge-sm ${toneClass(beanBadge().tone)}`}>{beanBadge().text}</span>
        </div>
        {/* 一句话说清"为什么这里没有一个总数"：豆按账号记账，合计出来哪个账号都用不了。
            这条是需求方点出来的 —— 别哪天又被"优化"成一个总额。 */}
        <p class="text-xs opacity-60">按账号记账，不合计</p>
        {beansBody()}
      </div>
    </div>
  )

  // ── 最近运行 ──────────────────────────────────────────────────────────

  const recentRuns = () => {
    if (runs.value.length === 0) {
      return (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="还没有任何执行记录。"
              hint="先去任务中心挑一个任务跑一次，结果会出现在这里。"
              action={
                <button type="button" class="btn btn-primary" onClick={() => props.onGo('tasks')}>
                  去任务中心
                </button>
              }
            />
          </div>
        </div>
      )
    }

    const rows = runs.value.slice(0, RECENT_LIMIT)

    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">最近运行</h2>
            {/*
              入口指向历史页，而不是在这里再铺一张长表：
              两页看同一份数据不是冗余，是**粒度**不同（这里 5 条看趋势，那里全量看细节）。
              `min-h-11 md:min-h-0`：窄屏 44px 触控目标（组件指南 §6.1）。
            */}
            <button
              type="button"
              class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
              onClick={() => props.onGo('history')}
            >
              查看全部
            </button>
          </div>

          {/* 桌面：表格 ≥768px */}
          <div class="hidden overflow-x-auto rounded-box bg-base-100 md:block">
            <table class="table table-sm">
              <thead>
                <tr>
                  <th>任务</th>
                  <th>账号</th>
                  <th>结果</th>
                  <th>耗时</th>
                  <th>时间</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r) => (
                  <tr key={r.run_id}>
                    <td>{r.task}</td>
                    <td class="break-words">{accountName(r.account)}</td>
                    {/*
                      ⚠ 结果这一格是「状态词 + 一句话摘要」两行，不是历史页那种"只给摘要"。
                      只给摘要的话，**"成功"这两个字在表里根本不出现** ——
                      全靠徽章的颜色区分，色觉障碍的用户读到的是"打卡 3 个账号"
                      和"上游返回 90001"两串看不出褒贬的字。总览只有 5 行，
                      多一行换"一眼看清"是划算的；历史页密度优先，那一页不动。
                    */}
                    <td>
                      <div class="flex flex-col items-start gap-0.5">
                        <span class={`badge badge-sm ${statusBadge(r.outcome.status)}`}>
                          {statusLabel(r.outcome.status)}
                        </span>
                        <span class="break-words text-xs opacity-60">{outcomeText(r.outcome)}</span>
                      </div>
                    </td>
                    <td>{fmtDuration(r.duration_ms)}</td>
                    <td>{fmtEpochMs(r.started_at_ms)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* 移动：卡片流 <768px。切卡片后**每个值都要带标签** */}
          <div class="grid gap-3 md:hidden">
            {rows.map((r) => (
              <div key={r.run_id} class="rounded-box bg-base-200 p-3">
                <div class="flex items-center justify-between gap-2">
                  <span class="font-medium">{r.task}</span>
                  <span class={`badge ${statusBadge(r.outcome.status)}`}>
                    {statusLabel(r.outcome.status)}
                  </span>
                </div>
                <p class="mt-1 break-words text-xs opacity-70">账号 {accountName(r.account)}</p>
                <p class="break-words text-xs opacity-70">{outcomeText(r.outcome)}</p>
                <p class="mt-1 text-xs opacity-60">
                  耗时 {fmtDuration(r.duration_ms)} · {fmtEpochMs(r.started_at_ms)}
                </p>
              </div>
            ))}
          </div>
        </div>
      </div>
    )
  }

  // ── 页头 ──────────────────────────────────────────────────────────────

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      {/* 标题由外壳那个 sticky navbar 显示（见 `app.tsx`）—— 这里**不再重复**。
          这一行只留"数据的年龄"与右侧的刷新：不轮询，所以它是"上面那些数字
          是什么时候的"的唯一答案。 */}
      {loadedAt.value ? <span class="text-xs opacity-60">更新于 {fmtClock(loadedAt.value)}</span> : null}
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
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (
        <>
          {engineBar()}

          {/* 只在真出问题时出现。`schedulerRunning` 是运行时实况，不是配置项 ——
              所以这句说的是"现在不会自动跑"，而不是"你没配"。 */}
          {status.value?.schedulerRunning === false ? (
            <div class="alert alert-soft alert-warning text-sm" role="status">
              <span class="break-words">
                定时调度循环没在运行 —— 定时任务到点不会自动触发（手动跑任务不受影响）。
              </span>
            </div>
          ) : null}

          <StatRow items={metrics()} />

          {/* 并排：定时任务占 2/3（它信息量大），今日AI豆占 1/3（就一个数）。
              窄屏回落成单列 —— 两格各占整宽，读数不至于被挤到换行。 */}
          <div class="grid gap-3 md:grid-cols-3">
            {upcomingPanel()}
            {beansPanel()}
          </div>

          {recentRuns()}
        </>
      ) : null}

      {/*
        AI豆明细弹窗。⚠ 放在 `ready` 分支**外面**：`openDialog` 是按 id 找真实 DOM 的
        （`ui/dialog.tsx` 文件头），节点必须一直在 —— 挂在 `ready` 里面时，
        整页重载（`load()` 把 `state` 置 `loading`）会把开着的弹窗连节点一起拆掉。
      */}
      <Dialog id={BEAN_DETAIL_DIALOG_ID} title={beanDetailTitle()}>
        {/* 档位 + 刷新。切档**不关窗**，只换数据（见 `switchBeanRange`）。 */}
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="join">
            {BEAN_RANGES.map((it) => (
              <button
                key={it.days}
                type="button"
                class={`btn join-item btn-sm min-h-11 whitespace-nowrap md:min-h-0 ${
                  beanDetailDays.value === it.days ? 'btn-primary' : 'btn-soft'
                }`}
                onClick={() => switchBeanRange(it.days)}
              >
                {it.label}
              </button>
            ))}
          </div>
          <button type="button" class="btn btn-sm btn-ghost" onClick={() => void loadBeanDetail()}>
            刷新
          </button>
        </div>
        <p class="text-xs opacity-60">{beanDetailHint()}</p>
        {beanDetailBody()}
      </Dialog>
    </div>
  )
}
