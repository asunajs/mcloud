/**
 * 兑换（抢兑）：按**活动/品类组**看清单 → 挑一件 → 二次确认（真扣豆）/ 定时抢兑。
 *
 * # 这一页与别处最大的不同：写操作**真的扣豆**
 *
 * 所以三件事是硬要求，不是修饰：
 * ① 点"兑换"先弹**二次确认**，确认框里写出**具体这一件的名字与价格**
 *    （只写"确定吗"等于没确认），并写清**同组名额会被占掉**（见下）；
 * ② 判据没过的档位按钮 **disabled**，并把"为什么不能兑"当面写出来 ——
 *    那正是 `plan.reason()` 存在的意义；
 * ③ 不同档位**文案分档**，别把"本月已兑"与"余额不足"显示成同一句
 *    （前者要等下个月，后者充豆就行，用户的下一步动作完全不同）。
 *
 * # 为什么按组分区，而不是平铺一条长清单（2026-10-01 重排）
 *
 * 实拍一份清单是 **51 件 / 8 个组**，平铺的问题是**同一组的共享事实被逐件复读**：
 *
 * ① **活动时间窗是组的属性**：同组逐字节相同、组间不同（实拍组 0 是
 *    `07-01 09:30 ~ 12-31 23:59`，组 7 只到 `10-31`，组 11 从 `05-08` 起）。
 *    逐件显示"活动时间"等于把同一句话印 17 遍。
 * ② **「本月多选一限兑」也是组的规则**，而且是**上游原话**
 *    （`prizeGroup.json` 的 `subTitle`，实拍组 2「音乐类会员」）。
 * ③ **组名同样来自上游**（同上的 `title`）—— 我们不编"音乐类"这种品类名。
 *
 * ⇒ 分区标题栏承载这三样；组内每件只留"名字 / 价格 / 现量 / 状态"。
 * 被挡住的件**折进一行**（"另有 N 件不可兑（原因）"），整组都不可兑时整组折起来。
 *
 * # 「抢光」到底是什么意思（别和"池子空了"混为一谈）
 *
 * 实拍 `251230032`（白银会员月卡）是 `minRemainderCount = 0`（H5 判"奖品被抢光啦"）
 * 而 `count = 43,739` —— **池子里还有货，是这一档/今天的份额没了**。
 * 所以卡片上把两件事分开写：`现量 4.3 万 / 共 40 万`（池子）与「已抢光」（我还抢得到吗）。
 * 只写一句"不可兑"，等于把这两件事都说丢了。
 *
 * # 定时抢兑：这一页的主路径
 *
 * 兑换的真实用法是"等到某个时刻再抢"，所以每件**可兑或会随时间变化**的件
 * 都有一个「定时」入口：它把"这一件 + 这个账号 + 这个时刻"落成一条**普通定时任务**
 * （`PUT /api/schedules/<id>`，`body = { prizeIdOrName, skipWait }`），
 * 之后在「定时」页里管理 —— **不在这页另造一套定时的 UI**。
 *
 * ⚠ 两个当面的限制（弹窗里也写）：调度器 30 秒一跳，直接按分钟触发最多晚 30 秒，
 * 所以目标是**整点**时改成"提前两分钟触发 + 任务自己等到整点"；
 * 而调度器是"宁可漏跑不可重跑"，**服务那一刻不在，就不会补跑**。
 */

import { onMounted, ref } from 'lite'
import {
  listAccounts,
  listExchangePrizes,
  runExchange,
  saveSchedule,
  type AccountSummary,
  type ExchangePrizeView,
  type ExchangePrizesResponse,
  type ExchangeRunOutcome,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { ConfirmDialog, Dialog, closeDialog, openDialog } from '../ui/dialog'
import { pushToast } from '../ui/toast'
import { fmtEpochMs, fmtUpstreamTime } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/** 二次确认弹窗的 id。 */
const CONFIRM_ID = 'confirm-exchange'
/** 定时抢兑弹窗的 id。 */
const PLAN_ID = 'plan-exchange'

/**
 * 档位 → **一句话**状态（上游语义，别把同一件事说两遍）。
 *
 * ⚠ 档位 slug 是 `ExchangePlan::slug()` 那一套，**用下划线**
 * （`not_started` / `exchanged_this_prize`）。上一版这里比的是连字符
 * （`not-started`），于是那两个分支从来没命中过 —— 颜色一直是兜底色。
 */
const statusPhrase = (prize: ExchangePrizeView): string => {
  switch (prize.plan) {
    case 'go':
      return '可兑'
    case 'not_started':
      return '未开始'
    case 'ended':
      return '已结束'
    case 'min_remainder_zero':
      // H5 在这一档的原文是「奖品被抢光啦」；这里用短句，避免和"不可兑"重复。
      return '已抢光'
    case 'sold_out':
      return '池子已兑完'
    case 'daily_stock_empty':
      return '今日已抢完'
    case 'limit_tomorrow':
      return '明天再来'
    case 'exchanged_this_prize':
      return '本月已兑'
    case 'exchanged_this_group':
      return '同组本月已兑'
    case 'insufficient_cloud':
      return 'AI豆不够'
    case 'price_missing':
      return '上游没给价格'
    case 'limit_missing':
      return '上游没给可兑状态'
    case 'limit_three_conflict':
      return '上游文案冲突'
    default:
      return '暂不可兑'
  }
}

/**
 * 档位 → 徽章颜色。
 *
 * 只分四类：可兑、**要等**（时间窗没到）、**已兑过**（本月/本组用掉了）、
 * 其余不可兑。刻意不按 17 档一一配色 —— 用户要区分的是"我该做什么"。
 */
const badgeClass = (prize: ExchangePrizeView): string => {
  if (!prize.blocks) return 'badge badge-success'
  if (prize.plan === 'not_started') return 'badge badge-info'
  if (prize.plan === 'exchanged_this_prize' || prize.plan === 'exchanged_this_group') {
    return 'badge badge-warning'
  }
  return 'badge badge-ghost'
}

/**
 * 这一件**值不值得定时去抢**。
 *
 * 判据是"时间会不会改变结论"：抢光/今日没份/明天再来/还没开始 —— 都会随时间变；
 * 而"本月已兑过"与"上游没给判据字段"等到明天也一样，给个「定时」按钮只会骗人。
 */
const canSchedule = (prize: ExchangePrizeView): boolean =>
  !prize.blocks ||
  prize.plan === 'not_started' ||
  prize.plan === 'min_remainder_zero' ||
  prize.plan === 'daily_stock_empty' ||
  prize.plan === 'sold_out' ||
  prize.plan === 'limit_tomorrow'

/** 兑换结果的档位 → 提示条颜色。 */
const resultAlertClass = (status: string): string => {
  switch (status) {
    case 'ordered':
    case 'already-exchanged':
      return 'alert alert-success'
    case 'unknown':
    case 'slider-unresolved':
      return 'alert alert-warning'
    default:
      return 'alert alert-error'
  }
}

/** 千分位。池子是四位数以上，不加分隔符读起来费劲。 */
const fmtNum = (n: number): string => n.toLocaleString('zh-CN')

/** 浏览器时区（定时任务要带一个）。取不到就按本项目的主场时区。 */
const browserTz = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Shanghai'

const pad2 = (n: number): string => String(n).padStart(2, '0')

/** `Date` → `MM-DD HH:mm`（本地）。 */
const fmtLocal = (d: Date): string =>
  `${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`

/** `Date` → `datetime-local` 输入框要的 `YYYY-MM-DDTHH:mm`（本地）。 */
const toLocalInput = (d: Date): string =>
  `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}T${pad2(d.getHours())}:${pad2(d.getMinutes())}`

/** 下一个整点（本地）—— 只作**默认值**，不宣称那是"放量时刻"（见文件头）。 */
const nextHour = (): Date => {
  const d = new Date()
  d.setMinutes(0, 0, 0)
  d.setHours(d.getHours() + 1)
  return d
}

/** 一组奖品。组的三个属性都来自上游，`null` 时界面退回"第 N 组"。 */
interface PrizeGroup {
  key: string
  groupId: number | null
  title: string | null
  subTitle: string | null
  monthQuantity: number | null
  items: ExchangePrizeView[]
}

/** 按 `groupId` 聚合；**有可兑的组排前面**，其余按 groupId 数值升序。 */
const groupPrizes = (prizes: ExchangePrizeView[]): PrizeGroup[] => {
  const map = new Map<string, PrizeGroup>()
  for (const p of prizes) {
    const key = p.groupId == null ? 'none' : String(p.groupId)
    let g = map.get(key)
    if (!g) {
      g = {
        key,
        groupId: p.groupId,
        title: p.groupTitle,
        subTitle: p.groupSubTitle,
        monthQuantity: p.monthQuantity,
        items: [],
      }
      map.set(key, g)
    }
    g.items.push(p)
  }
  const hasOpen = (g: PrizeGroup) => g.items.some((x) => !x.blocks)
  return [...map.values()].sort((a, b) => {
    const da = hasOpen(a) ? 0 : 1
    const db = hasOpen(b) ? 0 : 1
    if (da !== db) return da - db
    return (a.groupId ?? Number.MAX_SAFE_INTEGER) - (b.groupId ?? Number.MAX_SAFE_INTEGER)
  })
}

/** 组标题：上游给什么念什么，没给才用组号兜底。 */
const groupTitle = (g: PrizeGroup): string =>
  g.title ?? (g.groupId == null ? '未分组' : `第 ${g.groupId} 组`)

/** 组内共享的活动窗口（同组逐字节相同 ⇒ 只说一次）。 */
const groupWindow = (g: PrizeGroup): string | null => {
  const s = g.items.find((i) => i.startTime)?.startTime ?? null
  const e = g.items.find((i) => i.endTime)?.endTime ?? null
  if (!s && !e) return null
  return `${fmtUpstreamTime(s)} ~ ${fmtUpstreamTime(e)}`
}

/** 一行的第二段：价格 + 池子（现量/总量）+ 今日份额。**各自标清是什么**。 */
const stockText = (p: ExchangePrizeView): string => {
  const bits: string[] = []
  if (p.price != null) bits.push(`${fmtNum(p.price)} 朵`)
  if (p.count != null) {
    bits.push(
      p.totalCount != null
        ? `现量 ${fmtNum(p.count)} / 共 ${fmtNum(p.totalCount)}`
        : `现量 ${fmtNum(p.count)}`,
    )
  }
  if (p.dailyRemainderCount != null && p.dailyCount != null) {
    bits.push(`今日 ${fmtNum(p.dailyRemainderCount)}/${fmtNum(p.dailyCount)}`)
  }
  return bits.join(' · ')
}

/** 被挡住的那些件，按"状态短语"去重成一句（重复的原因不逐件复读）。 */
const reasonsSummary = (items: ExchangePrizeView[]): string =>
  [...new Set(items.map(statusPhrase))].join('、')

/** 定时弹窗算出来的东西（纯函数，便于在界面上把结果**先给人看**）。 */
interface PlanComputed {
  expr: string
  human: string
  skipWait: boolean
  error: string | null
}

/**
 * 由"目标时刻 + 提前量 + 重试次数"算出 cron 与人话说明。
 *
 * # 两个刻意的取舍
 *
 * 1. **目标是整点 ⇒ 让任务自己等**：调度器 30 秒一跳，直接按分钟触发最多晚 30 秒，
 *    而"抢"里 30 秒等于输。所以提前量 ≥60 秒且目标是整点时，发 `skipWait=false`，
 *    由任务内部等到整点再发枪（`plan_exchange_wait` 的取向）。
 * 2. **重试用"分钟列表"表达**：cron 没法说"从这一刻起每分钟试 3 次"，
 *    但可以写成 `58,59,0 21 1 10 *`。跨小时的部分**排不进去**（小时列表×分钟列表
 *    会变成笛卡尔积，反而多触发），所以只用同一小时内的分钟，并在界面上说明。
 */
const buildPlan = (atLocal: string, leadSec: number, tries: number): PlanComputed => {
  if (atLocal.trim() === '') {
    return { expr: '', human: '', skipWait: true, error: '先选一个抢兑时刻' }
  }
  const target = new Date(atLocal)
  if (Number.isNaN(target.getTime())) {
    return { expr: '', human: '', skipWait: true, error: '这个时刻看不懂，重新选一个' }
  }
  if (target.getTime() <= Date.now()) {
    return { expr: '', human: '', skipWait: true, error: '这个时刻已经过去了' }
  }
  const lead = Math.max(0, Math.floor(leadSec))
  const trigger = new Date(target.getTime() - lead * 1000)
  const hour = trigger.getHours()
  const minutes: number[] = []
  for (let i = 0; i < Math.max(1, tries); i++) {
    const at = new Date(trigger.getTime() + i * 60_000)
    if (at.getHours() !== hour) break
    minutes.push(at.getMinutes())
  }
  // 目标正好在整点、且提前量够 ⇒ 交给任务等整点（更准）。
  const skipWait = !(target.getMinutes() === 0 && lead >= 60)
  const expr = `${minutes.join(',')} ${hour} ${trigger.getDate()} ${trigger.getMonth() + 1} *`
  const parts = skipWait
    ? [`${fmtLocal(trigger)} 触发，立刻发一枪（调度器 30 秒一跳，可能晚 ≤30 秒）`]
    : [`${fmtLocal(trigger)} 触发，任务自己等到 ${fmtLocal(target)} 整点再发枪`]
  parts.push(minutes.length > 1 ? `没成功就每分钟再试，共 ${minutes.length} 次` : '只试一次')
  if (minutes.length < tries) {
    parts.push('（跨小时的那几次 cron 表达不了，没排进去）')
  }
  return { expr, human: parts.join('；'), skipWait, error: null }
}

/**
 * 分组视图：在 `groupPrizes` 的基础上把"可兑 / 被挡住"分好。
 *
 * ⚠ 这一步**不能写在 JSX 里**（`.`map` 回调必须是表达式体，见 [`PrizeRow`] 那段），
 * 所以在这里先算成数据，再交给 `.map` 渲染。
 */
interface PrizeGroupView extends PrizeGroup {
  open: ExchangePrizeView[]
  blocked: ExchangePrizeView[]
}

const groupViews = (prizes: ExchangePrizeView[]): PrizeGroupView[] =>
  groupPrizes(prizes).map((g) => ({
    ...g,
    open: g.items.filter((p) => !p.blocks),
    blocked: g.items.filter((p) => p.blocks),
  }))

/**
 * 一个分区的标题栏：组名（**上游给的**）+ 该组限兑规则（上游原话）+ 件数
 * + 组内**共享**的活动窗口 + 本月名额。
 */
const GroupHead = (props: { group: PrizeGroup; openCount: number }) => (
  <div class="flex flex-col gap-1">
    <div class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
      <h3 class="card-title text-base">{groupTitle(props.group)}</h3>
      {props.group.subTitle ? (
        <span class="text-sm opacity-70">{props.group.subTitle}</span>
      ) : null}
      <span class="badge badge-sm badge-ghost">
        {props.openCount > 0 ? `${props.openCount} 件可兑` : `${props.group.items.length} 件`}
      </span>
      {props.group.monthQuantity != null ? (
        <span class="text-xs opacity-60">本组每月可兑 {props.group.monthQuantity} 件</span>
      ) : null}
    </div>
    {groupWindow(props.group) ? (
      <span class="text-xs opacity-60">活动 {groupWindow(props.group)}</span>
    ) : null}
  </div>
)

/**
 * 一条**可兑**的件。
 *
 * ⚠ 为什么写成组件、而不是页面里的 `const row = (p) => (…)`：
 * `web/lite/compiler.ts` 的 `mapCallback` **刻意不认块体回调**
 * （`xs.map((x) => { const a = …; return <Row/> })` 会落回通用路径，
 * 于是里面的 `key` 没人认领 ⇒ 直接报"这个 key 什么都不做"）。
 * 列表行的回调因此只能写成**表达式体**，"先算再渲染"和"两种行"只能靠组件拆开。
 */
const PrizeRow = (props: {
  prize: ExchangePrizeView
  busy: boolean
  onExchange: (prize: ExchangePrizeView) => void
  onSchedule: (prize: ExchangePrizeView) => void
}) => (
  <div class="flex items-center justify-between gap-3 py-2">
    <div class="flex min-w-0 flex-col">
      <span class="truncate text-sm">{props.prize.name ?? `奖品 ${props.prize.prizeId}`}</span>
      <span class="truncate text-xs opacity-60">{stockText(props.prize)}</span>
    </div>
    <div class="flex shrink-0 items-center gap-1">
      <button
        type="button"
        class="btn btn-sm btn-primary"
        disabled={props.busy}
        onClick={() => props.onExchange(props.prize)}
      >
        {props.busy ? '兑换中…' : '兑换'}
      </button>
      <button type="button" class="btn btn-sm" onClick={() => props.onSchedule(props.prize)}>
        定时
      </button>
    </div>
  </div>
)

/** 一条**被挡住**的件：状态一句话，完整原因放 `title`，不再占正文。 */
const BlockedRow = (props: {
  prize: ExchangePrizeView
  onSchedule: (prize: ExchangePrizeView) => void
}) => (
  <div class="flex items-center justify-between gap-3 py-1.5">
    <div class="flex min-w-0 flex-col">
      <span class="truncate text-sm opacity-80">{props.prize.name ?? `奖品 ${props.prize.prizeId}`}</span>
      <span class="truncate text-xs opacity-50">{stockText(props.prize)}</span>
    </div>
    <div class="flex shrink-0 items-center gap-1">
      <span class={badgeClass(props.prize)} title={props.prize.planReason}>
        {statusPhrase(props.prize)}
      </span>
      {canSchedule(props.prize) ? (
        <button type="button" class="btn btn-xs" onClick={() => props.onSchedule(props.prize)}>
          定时
        </button>
      ) : null}
    </div>
  </div>
)

export const ExchangePage = () => {
  const state = ref<LoadState>('loading')
  /** 整页级的加载失败（要配"重试"，所以留在页内，弹窗会挡住入口）。 */
  const errorMessage = ref('')

  const accounts = ref<AccountSummary[]>([])
  const account = ref('')

  const list = ref<ExchangePrizesResponse | null>(null)
  /** 清单本身读失败时的页内提示（与整页失败分开：账号还在，可以重试）。 */
  const listError = ref('')

  /** 正在兑换的那一件（用于把那一颗按钮变成"兑换中"并锁住别的）。 */
  const busyPrizeId = ref<number | null>(null)
  /** 待确认的那一件。 */
  const pending = ref<ExchangePrizeView | null>(null)
  /** 上一次兑换的结论。 */
  const result = ref<ExchangeRunOutcome | null>(null)

  // ── 定时抢兑弹窗的状态 ────────────────────────────────────────────────
  /** 正在给哪一件排定时。 */
  const planning = ref<ExchangePrizeView | null>(null)
  const planAccount = ref('')
  const planAt = ref(toLocalInput(nextHour()))
  const planLead = ref(120)
  const planTries = ref(3)
  const planError = ref('')
  const planSaving = ref(false)

  const loadAccounts = async () => {
    accounts.value = await listAccounts()
    if (account.value === '' && accounts.value.length > 0) {
      // ⚠ 用 `id`（路由的 `{id}` 就是它），**不是**手机号：
      // `AccountSummary` 里的 `id` 是账号标识（界面上常见 `19****01` 这种打码形态），
      // 拿手机号去当路径参数会 404。
      account.value = accounts.value[0]!.id
    }
  }

  const loadPrizes = async () => {
    if (account.value === '') {
      list.value = null
      return
    }
    listError.value = ''
    try {
      list.value = await listExchangePrizes(account.value)
    } catch (e) {
      list.value = null
      listError.value = e instanceof Error ? e.message : String(e)
    }
  }

  const load = async () => {
    state.value = 'loading'
    try {
      await loadAccounts()
      await loadPrizes()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : String(e)
      state.value = 'error'
    }
  }

  onMounted(load)

  /** 点"兑换"：先记下这一件，弹确认。 */
  const askExchange = (prize: ExchangePrizeView) => {
    pending.value = prize
    openDialog(CONFIRM_ID)
  }

  const doExchange = async () => {
    const prize = pending.value
    if (!prize) return
    busyPrizeId.value = prize.prizeId
    try {
      result.value = await runExchange(account.value, {
        prizeIdOrName: String(prize.prizeId),
      })
      closeDialog(CONFIRM_ID)
      pending.value = null
      // 兑完清单里的状态变了（余量、limit），**重读**才不是旧数。
      await loadPrizes()
    } catch (e) {
      // 后端把业务失败也折成了正常响应（那是结论不是错误），
      // 所以能走到这里的多半是"参数不成立/会话失效"这一类，照实弹出来。
      closeDialog(CONFIRM_ID)
      pending.value = null
      showError('兑换没跑起来', e instanceof Error ? e.message : String(e))
    } finally {
      busyPrizeId.value = null
    }
  }

  /**
   * 同组名额的提示（**不可逆**，必须在确认框里当面说）。
   *
   * ⚠ `groupPrizeIds` 实拍**包含自己**（`251230053` 的那份里有它自己）⇒
   * "同组另外 N 件"要减掉自己那一项，否则会多报一件。
   */
  const groupQuotaNote = (prize: ExchangePrizeView): string | null => {
    const name =
      prize.groupTitle ?? (prize.groupId == null ? '这一组' : `第 ${prize.groupId} 组`)
    const siblings = (prize.groupPrizeIds ?? []).filter((x) => x !== String(prize.prizeId))
    const otherCount = prize.groupPrizeIds == null ? 0 : siblings.length
    const bits: string[] = []
    if (prize.monthQuantity != null) bits.push(`「${name}」每月只能兑 ${prize.monthQuantity} 件`)
    if (otherCount > 0) bits.push(`同组另外 ${otherCount} 件本月将不能兑`)
    if (bits.length === 0) return null
    return `${bits.join('；')}。名额占掉不能撤。`
  }

  /** 点"定时"：把这一件带进弹窗。 */
  const askSchedule = (prize: ExchangePrizeView) => {
    planning.value = prize
    planAccount.value = account.value
    planAt.value = toLocalInput(nextHour())
    planLead.value = 120
    planTries.value = 3
    planError.value = ''
    openDialog(PLAN_ID)
  }

  const submitSchedule = async () => {
    const prize = planning.value
    if (!prize) return
    const computed = buildPlan(planAt.value, planLead.value, planTries.value)
    if (computed.error) {
      planError.value = computed.error
      return
    }
    planSaving.value = true
    planError.value = ''
    try {
      // id 由前端生成（与定时页同一条约定）：服务端不发明 id。
      const id = `sch-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
      await saveSchedule(id, {
        name: `抢兑 ${prize.name ?? `奖品 ${prize.prizeId}`}`,
        task: 'exchange',
        accounts: [planAccount.value],
        expr: computed.expr,
        timezone: browserTz(),
        // ⚠ 参数名必须与 `ExchangeTask::params()` 一致（`prizeIdOrName` / `skipWait`）。
        body: { prizeIdOrName: String(prize.prizeId), skipWait: computed.skipWait },
        enabled: true,
      })
      closeDialog(PLAN_ID)
      planning.value = null
      pushToast('success', '已加到定时 —— 到「定时」页可以看到下次触发时间')
    } catch (e) {
      planError.value = e instanceof Error ? e.message : String(e)
    } finally {
      planSaving.value = false
    }
  }

  return (
    <div class="flex flex-col gap-4">
      {/* ── 账号 + 刷新 ─────────────────────────────────────────────── */}
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <h2 class="card-title text-base">兑换</h2>
          <p class="text-sm opacity-70">
            挑一件兑换。下单会真的扣AI豆，所以点下去要再确认一次；抢不到的可以加个定时。
          </p>

          {accounts.value.length === 0 ? (
            <div class="alert alert-warning">
              <span>还没有已登录的账号 —— 兑换要用账号的票去下单。</span>
            </div>
          ) : (
            <fieldset class="fieldset">
              <legend class="fieldset-legend">账号</legend>
              <select
                class="select w-full md:w-80"
                value={account.value}
                onChange={(e) => {
                  account.value = (e.target as HTMLSelectElement).value
                  // 换了账号，上一件的结论就不属于这个账号了 —— 清掉，
                  // 否则"刚兑成功"那条提示会挂在一个没兑过的账号下面。
                  result.value = null
                  void loadPrizes()
                }}
              >
                {accounts.value.map((a) => (
                  <option key={a.id} value={a.id}>{a.nickname || a.label || a.id}</option>
                ))}
              </select>
            </fieldset>
          )}

          <div class="flex flex-col gap-2 text-xs opacity-60">
            {list.value ? (
              <span>
                清单读取于 {fmtEpochMs(list.value.nowMs)}。判据档位不含余额 ——
                {list.value.balanceNote}
              </span>
            ) : null}
            {/* 组名那一枪没成时照实说：清单是好的，只是分区标题退回"第 N 组"。 */}
            {list.value?.groupsNote ? (
              <span class="text-warning">组名没取到：{list.value.groupsNote}</span>
            ) : null}
          </div>

          <div class="card-actions justify-end">
            <button type="button" class="btn btn-sm" onClick={() => void loadPrizes()}>
              刷新清单
            </button>
          </div>
        </div>
      </div>

      {/* ── 上一次的结论 ────────────────────────────────────────────── */}
      {result.value ? (
        <div class={resultAlertClass(result.value.status)}>
          <div class="flex flex-col gap-1">
            <span class="font-medium">{result.value.summary}</span>
            <span class="text-xs opacity-80">
              档位 {result.value.plan}（{result.value.planReason}）
              {result.value.code !== 0 ? ` · 业务码 ${result.value.code}` : ''}
              {result.value.offset != null ? ` · 落点 ${result.value.offset}` : ''}
              {result.value.attempts === 0 ? ' · 一枪未发' : ` · 下单 ${result.value.attempts} 次`}
            </span>
            {result.value.order ? (
              <span class="text-xs opacity-80">
                订单号 {String(result.value.order.oid ?? '—')}
                {result.value.order.prizeName ? ` · ${String(result.value.order.prizeName)}` : ''}
              </span>
            ) : null}
            {result.value.error ? (
              <span class="text-xs opacity-80">说明：{result.value.error}</span>
            ) : null}
          </div>
        </div>
      ) : null}

      {/* ── 清单（按组分区）─────────────────────────────────────────── */}
      {state.value === 'loading' ? (
        <div class="flex justify-center py-10">
          <span class="loading loading-lg" />
        </div>
      ) : state.value === 'error' ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="读不到账号或清单。"
              hint={errorMessage.value}
              action={
                <button type="button" class="btn" onClick={load}>
                  重试
                </button>
              }
            />
          </div>
        </div>
      ) : listError.value !== '' ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="奖品清单没读到。"
              hint={listError.value}
              action={
                <button type="button" class="btn" onClick={loadPrizes}>
                  重读清单
                </button>
              }
            />
          </div>
        </div>
      ) : !list.value || list.value.prizes.length === 0 ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="这个账号现在没有可兑换的奖品。"
              hint="清单由上游按活动与账号投放 —— 换个账号看看，或等活动开场后再来（清单只读，随时可以刷新）。"
              action={
                <button type="button" class="btn" onClick={loadPrizes}>
                  刷新清单
                </button>
              }
            />
          </div>
        </div>
      ) : (
        // ⚠ 这一层容器不能省：`.map` 必须作为**某个 JSX 元素的子表达式**出现，
        // 编译器的三元分支只认"JSX / null"（`web/lite/compiler.ts` 的 `branchOk`）——
        // 直接把 `.map(...)` 当分支放，`key` 就不会被认领（报"这个 key 什么都不做"）。
        <div class="flex flex-col gap-4">
          {groupViews(list.value.prizes).map((g) => (
            <div key={g.key}>
            {/* 整组都不可兑 ⇒ 整组折起来（51 件的清单里这类组占大头）。 */}
            {g.open.length === 0 ? (
              <details class="collapse collapse-arrow bg-base-100 shadow-sm">
                <summary class="collapse-title">
                  <GroupHead group={g} openCount={0} />
                </summary>
                <div class="collapse-content flex flex-col divide-y divide-base-200">
                  {g.blocked.map((p) => (
                    <div key={p.prizeId}>
                      <BlockedRow prize={p} onSchedule={askSchedule} />
                    </div>
                  ))}
                </div>
              </details>
            ) : (
              <div class="card bg-base-100 shadow-sm">
                <div class="card-body gap-2">
                  <GroupHead group={g} openCount={g.open.length} />
                  <div class="flex flex-col divide-y divide-base-200">
                    {g.open.map((p) => (
                      <div key={p.prizeId}>
                        <PrizeRow
                          prize={p}
                          busy={busyPrizeId.value !== null}
                          onExchange={askExchange}
                          onSchedule={askSchedule}
                        />
                      </div>
                    ))}
                  </div>
                  {g.blocked.length > 0 ? (
                    <details class="collapse collapse-arrow border-t border-base-200">
                      <summary class="collapse-title min-h-0 py-2 text-sm opacity-70">
                        另有 {g.blocked.length} 件不可兑（{reasonsSummary(g.blocked)}）
                      </summary>
                      <div class="collapse-content flex flex-col divide-y divide-base-200">
                        {g.blocked.map((p) => (
                          <div key={p.prizeId}>
                            <BlockedRow prize={p} onSchedule={askSchedule} />
                          </div>
                        ))}
                      </div>
                    </details>
                  ) : null}
                </div>
              </div>
            )}
            </div>
          ))}
        </div>
      )}

      {/* ── 二次确认 ────────────────────────────────────────────────── */}
      <ConfirmDialog
        id={CONFIRM_ID}
        title="确认兑换"
        impact={
          <span>
            将用{' '}
            <strong>{pending.value?.name ?? `奖品 ${pending.value?.prizeId ?? ''}`}</strong>
            {pending.value?.price != null ? <> 兑换这一件，扣 <strong>{pending.value.price} 颗 AI豆</strong></> : ' 兑这一件'}
            。扣掉的豆不会退回，同一个月同一件也不能重复兑。
            {pending.value ? (
              groupQuotaNote(pending.value) ? (
                <span class="mt-2 block font-medium text-warning">
                  {groupQuotaNote(pending.value)}
                </span>
              ) : null
            ) : null}
          </span>
        }
        confirmLabel="扣豆兑换"
        busy={busyPrizeId.value !== null}
        onConfirm={doExchange}
      />

      {/* ── 定时抢兑 ────────────────────────────────────────────────── */}
      <Dialog id={PLAN_ID} title="定时抢兑" size="md">
        <p class="text-sm">
          给{' '}
          <strong>{planning.value?.name ?? `奖品 ${planning.value?.prizeId ?? ''}`}</strong>
          {planning.value?.price != null ? <>（{planning.value.price} 朵）</> : null}
          {' '}排一条定时任务 —— 到点点一次「兑换」，用的是下面这个账号。
        </p>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">账号</legend>
          <select
            class="select w-full"
            value={planAccount.value}
            onChange={(e) => (planAccount.value = (e.target as HTMLSelectElement).value)}
          >
            {accounts.value.map((a) => (
              <option key={a.id} value={a.id}>{a.nickname || a.label || a.id}</option>
            ))}
          </select>
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">抢兑时刻</legend>
          <input
            type="datetime-local"
            class="input w-full"
            value={planAt.value}
            onInput={(e) => (planAt.value = (e.target as HTMLInputElement).value)}
          />
          <p class="text-xs opacity-60">
            默认给了下一个整点，但那不是"放量时刻" —— 我们实测过 21:00 那个整点，
            抢光的件一件都没恢复（见抓包样本 §5：份额刷新与整点无关）。按你了解的时刻填。
          </p>
        </fieldset>

        <div class="grid grid-cols-1 gap-3 md:grid-cols-2">
          <fieldset class="fieldset">
            <legend class="fieldset-legend">提前触发（秒）</legend>
            <input
              type="number"
              class="input w-full"
              min="0"
              max="600"
              value={String(planLead.value)}
              onInput={(e) => (planLead.value = Number((e.target as HTMLInputElement).value) || 0)}
            />
          </fieldset>
          <fieldset class="fieldset">
            <legend class="fieldset-legend">没成功怎么办</legend>
            <select
              class="select w-full"
              value={String(planTries.value)}
              onChange={(e) => (planTries.value = Number((e.target as HTMLSelectElement).value))}
            >
              <option value="1">只试一次</option>
              <option value="3">每分钟再试，共 3 次</option>
              <option value="5">每分钟再试，共 5 次</option>
            </select>
            <p class="text-xs opacity-60">
              重复触发不会多扣豆：兑到之后这一件的判据会变成"本月已兑"，任务自己会跳过。
            </p>
          </fieldset>
        </div>

        {buildPlan(planAt.value, planLead.value, planTries.value).error ? (
          <div class="alert alert-warning">
            <span>{buildPlan(planAt.value, planLead.value, planTries.value).error}</span>
          </div>
        ) : (
          <div class="rounded-box flex flex-col gap-1 bg-base-200 p-3 text-xs">
            <span>{buildPlan(planAt.value, planLead.value, planTries.value).human}</span>
            <span class="opacity-70">
              cron <code>{buildPlan(planAt.value, planLead.value, planTries.value).expr}</code>
              {' · '}时区 {browserTz()}
            </span>
          </div>
        )}

        <p class="text-xs opacity-70">
          ⚠ 调度器是「宁可漏跑不可重跑」：服务在那一刻不在，就不会补跑。
          到「定时」页能看到下次触发时间，也能停掉它。
        </p>

        {planError.value !== '' ? (
          <div class="alert alert-error">
            <span>{planError.value}</span>
          </div>
        ) : null}

        <div class="modal-action">
          <button type="button" class="btn" onClick={() => closeDialog(PLAN_ID)}>
            取消
          </button>
          <button
            type="button"
            class="btn btn-primary"
            disabled={planSaving.value || buildPlan(planAt.value, planLead.value, planTries.value).error !== null}
            onClick={submitSchedule}
          >
            {planSaving.value ? '保存中…' : '加到定时'}
          </button>
        </div>
      </Dialog>
    </div>
  )
}
