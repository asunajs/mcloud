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
 * 所以目标是**能等到的那一刻**（默认给的下一个 0 点）时改成"提前两分钟触发 + 任务自己等到它"；
 * 而调度器是"宁可漏跑不可重跑"，**服务那一刻不在，就不会补跑**。
 *
 * ⚠ 默认时刻是**猜的**：按这件露出的口径（有今日份额 ⇒ 下一个 0 点；只有每月名额 ⇒ 下月 1 日）。
 * 实测到的一款按自然日 0 点复位（抓包样本 §7），但**复位时刻因活动而异**（用户口径 2026-10-02：
 * 「也不一定是 0 点，有些也不是」）⇒ 界面不写"0 点开抢"，只说清猜法与依据。
 */

import { onMounted, ref } from '@asunajs/lite'
import {
  listAccounts,
  listExchangePrizes,
  listExchangeSubscriptions,
  runExchange,
  saveSchedule,
  toggleExchangeSubscription,
  type AccountSummary,
  type ExchangePrizeView,
  type ExchangePrizesResponse,
  type ExchangeRunOutcome,
  type ExchangeSubscriptionView,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { ConfirmDialog, Dialog, closeDialog, openDialog } from '../ui/dialog'
import { IconStarFilled, IconStarOutline } from '../ui/icons'
import { pushToast } from '../ui/toast'
import { fmtEpochMs, fmtUpstreamTime } from '../ui/time'
import { Select } from '../ui/select'

type LoadState = 'loading' | 'error' | 'ready'

/**
 * 「下单前再确认一次」这个偏好的存储键。
 *
 * ⚠ 存 localStorage 而**不是**账号设置：它是**这个浏览器**的偏好，不是账号属性 ——
 * 换台机器、换个浏览器，本来就该重新确认一次（而账号设置会跟着账号走遍所有设备）。
 * 名字风格跟主题偏好那个 `mcloud-theme` 一致。
 */
const CONFIRM_KEY = 'mcloud-exchange-confirm'

/**
 * 「开启本地判定」这个偏好的存储键。
 *
 * # ⚠ 为什么要**正着说**（用户口径 2026-10-02）
 *
 * 上一版这个开关叫「本地判不能兑也照样发单」—— 那是**双重否定** ✗，
 * 和旁边那个陈述句（「下单前再确认一次」✓）摆在一起，一个陈述、一个反问，
 * 读的人要先在脑子里转一道弯才明白拨过去会发生什么。
 * 现在改成"**开启本地判定**"✓：**开 = 本地先判、判不过就不发**（＝安全的那一侧 ✓），
 * **关 = 抢光/未开始/已兑过也**真发下单请求 ✓（可能真扣豆 ✓）。
 *
 * ⚠ 用「判定」而不是「验证」：这个仓里「验证」已经被**滑块验证**占了 ✗，
 * 写"开启本地验证"会让人以为是滑块那件事。
 *
 * 默认**开**（＝现在的安全默认：判不过就如实说"一枪未发"、不扣豆 ✓）。
 */
const LOCAL_JUDGE_KEY = 'mcloud-exchange-local-judge'

const readLocalJudgePref = () => {
  try {
    // 缺省 `true`：与"关掉才是危险动作"这个方向一致（写法同 `readConfirmPref`）。
    return localStorage.getItem(LOCAL_JUDGE_KEY) !== '0'
  } catch {
    return true
  }
}

const writeLocalJudgePref = (on: boolean) => {
  try {
    localStorage.setItem(LOCAL_JUDGE_KEY, on ? '1' : '0')
  } catch {
    /* 存不进去只影响"下次还记不记得"，不影响这次点下去的行为 */
  }
}

/** 读偏好，**默认开**。localStorage 在读不到时（隐私模式等）当默认值处理。 */
const readConfirmPref = (): boolean => {
  try {
    return localStorage.getItem(CONFIRM_KEY) !== '0'
  } catch {
    return true
  }
}

const writeConfirmPref = (on: boolean) => {
  try {
    localStorage.setItem(CONFIRM_KEY, on ? '1' : '0')
  } catch {
    // 写不进去只意味着"这次的选择下次不记得了"，不该因此报错打断兑换。
  }
}

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

/**
 * 定时弹窗的**占位默认时刻**（不是"正确时刻"）。
 *
 * ⚠⚠ 别把它读成"份额一定在 0 点复位"。用户口径（2026-10-02）：
 * 「也不一定是 0 点，有些也不是」—— 复位时刻**因活动而异**。
 * 我们实测到的那一款（`251230032`）是自然日 0 点（抓包样本 §7），
 * 但有的按活动自己的投放时刻（例如活动窗口写 `08:00` 那种）。
 *
 * 所以这里只按**这件自己露出来的口径**给一个猜测，并且在弹窗里当面说清：
 *
 * * 有「今日份额」（`dailyCount`）⇒ 猜每日口径 ⇒ 下一个 0 点；
 * * 看不出每日、但有「本组每月名额」（`monthQuantity`）⇒ 猜每月口径 ⇒ 下月 1 日 0 点；
 * * 都看不出来 ⇒ 下一个 0 点（纯占位）。
 */
const guessResetAt = (prize: ExchangePrizeView | null): Date => {
  const d = new Date()
  const monthlyOnly = prize?.dailyCount == null && (prize?.monthQuantity ?? 0) > 0
  if (monthlyOnly) {
    d.setHours(0, 0, 0, 0)
    d.setMonth(d.getMonth() + 1, 1) // 下个月 1 日 00:00
    return d
  }
  d.setHours(24, 0, 0, 0) // 下一个自然日 00:00（跨月/跨年由 Date 自己进位）
  return d
}

/** **下一个 0 点**（本地）—— 只作初始占位，理由见 [`guessResetAt`]。 */
const nextMidnight = (): Date => {
  const d = new Date()
  d.setHours(24, 0, 0, 0)
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
 * 由"目标时刻 + 提前量"算出 cron 与人话说明。
 *
 * # 刻意的取舍
 *
 * **目标正好落在整点 ⇒ 让任务自己等**：调度器 30 秒一跳，直接按分钟触发最多晚 30 秒，
 * 而"抢"里 30 秒等于输。所以提前量 ≥60 秒且目标是整点时，发 `skipWait=false`，
 * 由任务内部等到那一刻再发枪（`plan_exchange_wait`）。
 *
 *    ⚠ 这个"等"**只有目标本身是那个时刻时才有意义** —— 实测份额按自然日复位
 *    （0 点，抓包样本 §7），而 21:00 / 22:00 这类整点**不刷新**（§5/§6）。
 *    默认给的是下一个 0 点，别的整点填了也只是"等到那个整点"，不是放量时刻。
 *
 * ⚠ **这里不排"重试"**（用户口径 2026-10-02：「不要重试输入，默认重试一次无间隔」✓）：
 * 重试是**任务内部**的事 —— `514`（滑块被上游判错）时重取题、**立刻**再发一枪
 * （`SLIDER_ROUNDS = 2`，与参考实现 `task.ts:29-42` 同款 ✓）。cron 表达不了秒，
 * 在排期里写"每分钟再试"既不准也没必要 ✗。
 */
const buildPlan = (atLocal: string, leadSec: number): PlanComputed => {
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
  const minutes = [trigger.getMinutes()]
  // 目标正好在整点、且提前量够 ⇒ 交给任务等整点（更准）。
  const skipWait = !(target.getMinutes() === 0 && lead >= 60)
  const expr = `${minutes.join(',')} ${hour} ${trigger.getDate()} ${trigger.getMonth() + 1} *`
  const parts = skipWait
    ? [`${fmtLocal(trigger)} 触发，立刻发一枪（调度器 30 秒一跳，可能晚 ≤30 秒）`]
    : [`${fmtLocal(trigger)} 触发，任务自己等到 ${fmtLocal(target)} 整点再发枪`]
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
 * tab 栏的一项。
 *
 * ⚠ **刻意不带计数徽章**。第一版每个 tab 后面挂了 `可兑/共`（`13/17`），
 * 用户口径（2026-10-01，附截图）：「用户其实并不理解后面的数字，甚至也不关心他是啥」。
 * 那些数字只是噪声 —— "哪一组里有我能兑的"这件事**折叠状态本来就说了**：
 * 有可兑件的组默认展开、整组不可兑的默认收起，扫一眼就知道。
 */
interface GroupTab {
  key: string
  label: string
}

const groupTabs = (prizes: ExchangePrizeView[]): GroupTab[] => [
  { key: 'all', label: '全部' },
  ...groupViews(prizes).map((g) => ({ key: g.key, label: groupTitle(g) })),
]

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
 * `@asunajs/lite` 的 `compiler.ts` 的 `mapCallback` **刻意不认块体回调**
 * （`xs.map((x) => { const a = …; return <Row/> })` 会落回通用路径，
 * 于是里面的 `key` 没人认领 ⇒ 直接报"这个 key 什么都不做"）。
 * 列表行的回调因此只能写成**表达式体**，"先算再渲染"和"两种行"只能靠组件拆开。
 */
/**
 * 订阅星星（清单里**两种行**共用：能兑的 [`PrizeRow`] 与被挡住的 [`BlockedRow`]）。
 *
 * ⚠⚠ **被挡住的那件也必须能订阅** —— 订阅的用处恰恰是"盯着它什么时候又能兑了" ✓。
 * 只让可兑的能订阅，等于把最需要盯的那一类排除在外 ✗。
 * 这一条是被门禁逼出来的：装置里"已抢光"那件没有星星 ⇒ 那条断言直接失败 ✓
 * （不然它会以"界面上看着挺对"的样子留很久）。
 *
 * 为什么是**图标**按钮：这一行已经挤着「兑换/定时/已抢光」，再加一颗文字按钮
 * 窄屏就换行了 ✗ —— 而"订没订"用**空心/实心**表达就够 ✓。`title` 与 `aria-label`
 * 都写：鼠标能看、读屏能念 ✓（门禁也是按 `aria-label` 找它的 ✓）。
 */
const SubscribeStar = (props: {
  subscribed: boolean
  /** **这一件**正在切换订阅 ⇒ 禁用，避免连点。 */
  busy: boolean
  /** 两种行的按钮尺寸不同（`PrizeRow`/订阅卡用 `sm`、`BlockedRow` 用 `xs`）✓。 */
  size: 'sm' | 'xs'
  /**
   * ⚠ **不接收 prize**（用户口径 2026-10-02：「不要取消订阅，直接用星星」）：
   * 订阅卡里那一行的"取消"只有 `prizeId` 可用（那一件可能已经不在当前账号的
   * 清单里 ⇒ 根本没有 `ExchangePrizeView` ✓）。让调用方自己绑参数，
   * 这颗星星才能在**三个**地方通用 ✓（清单两处 + 订阅卡一处）。
   */
  onToggle: () => void
}) => (
  <button
    type="button"
    class={`btn ${props.size === 'sm' ? 'btn-sm' : 'btn-xs'} ${
      props.subscribed ? 'btn-soft text-primary' : 'btn-ghost opacity-60'
    }`}
    disabled={props.busy}
    aria-pressed={props.subscribed}
    aria-label={props.subscribed ? '取消订阅' : '订阅'}
    title={
      props.subscribed
        ? '已订阅：它会固定在最上面。点一下取消。'
        : '订阅：把它固定到页面最上面，不用每次翻分组'
    }
    onClick={() => props.onToggle()}
  >
    {props.subscribed ? <IconStarFilled class="size-4" /> : <IconStarOutline class="size-4" />}
  </button>
)

const PrizeRow = (props: {
  prize: ExchangePrizeView
  /** **这一行**就是正在跑的那件 ⇒ 显示「兑换中…」。 */
  busy: boolean
  /**
   * **有任意一次兑换在跑** ⇒ 这一行的按钮 disable（用户口径 2026-10-02 第 3 条：
   * 「我点击这个已抢光按钮 ⇒ 兑换按钮应该是 disable，已抢光按钮也应该是 disable」）。
   *
   * ⚠ 与 `busy` 是**两件事**，上一版把它们合成一个（传"任意一行在跑"）✗ ⇒
   * 运行时**每一行**都显示「兑换中…」✗（用户截图里四颗一起变，就是这么来的）。
   */
  locked: boolean
  onExchange: (prize: ExchangePrizeView) => void
  onSchedule: (prize: ExchangePrizeView) => void
  /** 这一件订没订（服务端那份列表说了算 ✓）。 */
  subscribed: boolean
  /** **这一件**正在切换订阅（`null` 之外的值）⇒ 那颗星星要禁用，避免连点。 */
  subBusy: boolean
  onToggleSubscribe: (prize: ExchangePrizeView) => void
}) => (
  <div class="flex items-center justify-between gap-3 py-2">
    <div class="flex min-w-0 flex-col">
      <span class="truncate text-sm">{props.prize.name ?? `奖品 ${props.prize.prizeId}`}</span>
      {/* 数字行**不**截断：窄屏宁可换行，也别把"共 400,000"吃掉（名字那行截断即可）。 */}
      <span class="text-xs opacity-60">{stockText(props.prize)}</span>
    </div>
    <div class="flex shrink-0 items-center gap-1">
      {/* 订阅入口（共用件，理由见 `SubscribeStar` 的注释） */}
      <SubscribeStar
        subscribed={props.subscribed}
        busy={props.subBusy}
        size="sm"
        onToggle={() => props.onToggleSubscribe(props.prize)}
      />
      <button
        type="button"
        // ⚠ 忙碌时**必须掉掉 `btn-primary`**：2026-10-02 用户截图反馈「点击兑换后按钮
        // 颜色不对」—— 当时忙碌态也挂着 primary，于是「兑换中…」看起来是
        // **一颗禁用的主色按钮**（紫蓝、灰得发脏 ✗），与旁边那些中性禁用按钮不一致。
        // 现在忙碌 = 中性 `btn` + `disabled`（与「已抢光」禁用态同款 ✓）。
        class={props.busy ? 'btn btn-sm' : 'btn btn-sm btn-primary'}
        disabled={props.busy || props.locked}
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

/**
 * 顶部「我订阅的」里的一行。
 *
 * ⚠ 与清单里的 [`PrizeRow`] **分开写**，不是懒得复用：那一边只答"这一件现在能不能兑"，
 * 这一边还得多答一句"**它还在不在这个账号的清单里**" —— 订阅可以比活动活得久 ✓
 * （下架了、或换了个云盘账号）。这一句正是"订阅"最容易骗人的地方：
 * 装作它还在，用户点兑换才发现没有 ✗。
 *
 * `key` 挂在这一行的 `<li>` 上（lite 的 `.map` 要求 key 落在返回的 JSX 元素上 ✓）。
 */
const SubscribedRow = (props: {
  sub: ExchangeSubscriptionView
  /** 当前账号清单里的对应项；`null` = 这个账号的清单里没有它。 */
  live: ExchangePrizeView | null
  /** 这一行正在切换订阅 ⇒ 两颗按钮都禁用（避免连点）。 */
  busy: boolean
  onExchange: (prize: ExchangePrizeView) => void
  /**
   * 定时抢兑。
   *
   * ⚠ **顶部这一块也必须能定时**（用户口径 2026-10-02：「订阅怎么没有定时选项」）——
   * 订阅盯的恰恰是"现在还不能兑、到点才放货"的那些 ✓，而定时就是为它们存在的 ✓。
   * 只给清单里的行配「定时」、订阅这块不配，等于把订阅最该有的动作砍掉了 ✗。
   */
  onSchedule: (prize: ExchangePrizeView) => void
  onUnsubscribe: (sub: ExchangeSubscriptionView) => void
}) => {
  // 取成 `const` 再进闭包：TS 对 `const` 的收窄能穿过闭包 ✓，
  // 于是下面那颗按钮里不必写 `!`（本仓不用非空断言，见 conventions）。
  const live = props.live
  return (
    <li class="flex items-center justify-between gap-3 py-2">
      <div class="flex min-w-0 flex-col">
        <span class="truncate text-sm">
          {/* 清单里还在 ⇒ 用**实时**名字（可能改过名）；不在 ⇒ 用订阅时的快照 ✓ */}
          {live !== null ? (live.name ?? `奖品 ${live.prizeId}`) : props.sub.name || `奖品 ${props.sub.prizeId}`}
        </span>
        <span class="text-xs opacity-60">
          {live === null
            ? '这个账号的清单里现在没有这一件（可能已下架）'
            : `${stockText(live)}${live.blocks ? ` · ${live.planReason}` : ''}`}
        </span>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        {/*
          ⚠ 取消订阅**就是这颗星星**（用户口径 2026-10-02：「不要取消订阅，直接用星星」）
          —— 文字按钮已经删掉 ✗。位置放在**最前**，与清单里那两处**完全一致** ✓：
          一处学会，三处都能用 ✓；这一行本来就窄，少一颗文字按钮也松快 ✓。
        */}
        <SubscribeStar
          subscribed={true}
          busy={props.busy}
          size="sm"
          onToggle={() => props.onUnsubscribe(props.sub)}
        />
        {live === null ? null : (
          <button
            type="button"
            class="btn btn-sm btn-primary"
            disabled={live.blocks || props.busy}
            title={live.blocks ? live.planReason : ''}
            onClick={() => props.onExchange(live)}
          >
            兑换
          </button>
        )}
        {/*
          「定时」的判据与清单里**同一条**（`canSchedule`）：可兑的能定时 ✓，
          以及那些"会随时间变"的档（未开始 / 抢光 / 今日份额空 / 明日可兑）也能 ✓ ——
          而"已结束 / 已兑过"这类定时没有意义，就不给 ✗。
          ⚠ `live === null`（这件不在当前账号的清单里）时**不给**：定时要把
          "这一件 + 这个账号"落成一条定时任务，手上没有这一件的清单读数就不该假装能排 ✗。
        */}
        {live !== null && canSchedule(live) ? (
          <button
            type="button"
            class="btn btn-sm"
            disabled={props.busy}
            onClick={() => props.onSchedule(live)}
          >
            定时
          </button>
        ) : null}
      </div>
    </li>
  )
}

/**
 * 一条**被挡住**的件：状态一句话，完整原因放 `title`，不再占正文。
 *
 * # ⚠「已抢光」在特定情况下要变成**按钮**（2026-10-02 用户口径）
 *
 * 用户原话：「文案显示已抢光，只要用户开启了取消二次确认，就直接显示按钮，
 * 而不是一个已抢光文本（按钮文本还是已抢光）」。
 *
 * 判据刻意**按那句文案**判（`statusPhrase(p) === '已抢光'`），而不是按 `plan` 名 ——
 * 用户说的就是"显示已抢光的那些"，按文案判不会因为后端多一个状态就漏掉。
 *
 * ⚠ 为什么只放**抢光**这一类：抢光是**会随时间变**的（份额放出来就又能兑）⇒
 * 让人点一下试试是合理的 ✓。而「未开始 / 已结束」不是这种性质（点下去注定被拒 ✗），
 * 它们仍是一句话，旁边留「定时」就够了。
 */
const BlockedRow = (props: {
  prize: ExchangePrizeView
  onSchedule: (prize: ExchangePrizeView) => void
  onExchange: (prize: ExchangePrizeView) => void
  /**
   * 这一行**能不能点**（＝用户关掉了「开启本地判定」）。
   *
   * ⚠ 判据挂**本地判定**，不是挂"下单前再确认一次"：前者才是"要不要让本地挡下"，
   * 后者只管"点下去要不要再问一遍"。2026-10-02 用户截图反馈：
   * 「我没开启本地判定，但是按钮不可点击」—— 上一版写成**无条件 disabled** ✗，
   * 于是关掉本地判定也没法强制发单（那个开关就落不到任何一档上了）。
   */
  canForce: boolean
  /** **有任意一次兑换在跑** ⇒ 这枚（本来可点的）按钮也 disable（用户口径第 3 条）。 */
  locked: boolean
  /** 这一件订没订（服务端那份列表说了算 ✓）。 */
  subscribed: boolean
  /** **这一件**正在切换订阅 ⇒ 那颗星星禁用。 */
  subBusy: boolean
  onToggleSubscribe: (prize: ExchangePrizeView) => void
}) => (
  <div class="flex items-center justify-between gap-3 py-1.5">
    <div class="flex min-w-0 flex-col">
      <span class="truncate text-sm opacity-80">{props.prize.name ?? `奖品 ${props.prize.prizeId}`}</span>
      <span class="text-xs opacity-50">{stockText(props.prize)}</span>
    </div>
    <div class="flex shrink-0 items-center gap-1">
      {/* ⚠ 被挡住的那件**也要能订阅** —— 订阅的用处就是盯着它什么时候又能兑 ✓。
          这条是门禁逼出来的（装置里那件没有星星 ⇒ 断言直接失败 ✓），
          详见 `SubscribeStar` 的注释。 */}
      <SubscribeStar
        subscribed={props.subscribed}
        busy={props.subBusy}
        size="xs"
        onToggle={() => props.onToggleSubscribe(props.prize)}
      />
      {/*
        判据没过的档位 = **一枚真按钮，但 disabled**（2026-10-02 用户口径第二次修正）。

        ⚠ 之前这里有两种形态：`exchangeDirect`（＝用户关掉了"下单前再确认一次"）时
        渲染一枚**能点**的 `btn btn-xs`（点了直接下单），否则渲染一个 `<span>` 徽标。
        用户 2026-10-02 截图反馈两件事：**① 灰色那枚"禁止按钮颜色不对"**（因为它压根不是
        按钮 ✗，是个徽标 ✗）；**② 白色那几枚"已抢光"还能再点** ✗。
        ⇒ 两条都归到同一个修法：**所有判据没过的档都渲染 `disabled` 的 `btn`** ✓
        —— 形态对了（daisyUI 的禁用配色 ✓）、也真的**点不动**了 ✓。

        ⚠ "抢光了还想试一下"这个需求**没有丢**：它现在归给「开启本地判定」那个开关
        （关掉 = 所有档都真发下单请求）。一页只留**一处**表达"我要强制发单"，
        比"每一行各自能点"更不容易误解 ✓。

        ⚠ `title` 保留：把"为什么不能兑"当面写出来（本文件设计说明第 ① 条）。
      */}
      {/* 本地判定**开** ⇒ 真的 `disabled` 按钮（点不动 ✓、禁用色由 daisyUI 管 ✓）。
          ⚠ `title` 挂**外层 span**：`disabled` 按钮浏览器**不触发 hover** ✗，
          挂它身上那句"为什么不能兑"永远看不到（本文件设计说明第 ① 条）。
          本地判定**关** ⇒ 同款**可点**按钮（点了走 `askExchange` ⇒ 真发单 ✓）
          —— 这样"强制发单"只有**一处**入口（那个开关），这一行如实反映它 ✓。 */}
      {props.canForce ? (
        <button
          type="button"
          class="btn btn-xs"
          disabled={props.locked}
          title={props.prize.planReason}
          onClick={() => props.onExchange(props.prize)}
        >
          {statusPhrase(props.prize)}
        </button>
      ) : (
        <span title={props.prize.planReason}>
          <button type="button" class="btn btn-xs" disabled>
            {statusPhrase(props.prize)}
          </button>
        </span>
      )}
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
  /**
   * 下单前是否再确认一次（**默认开**）。
   *
   * 兑换是**不可逆**的（真扣 AI豆、扣了不退），所以默认保留确认；
   * 但"抢兑"抢的是名额，每次多一次点击就是慢一步 ⇒ 给一个**关掉它的开关**
   * （用户口径 2026-10-02：「兑换这里加个取消二次弹窗按钮」）。
   */
  const confirmFirst = ref(readConfirmPref())
  /** 本地判定：**开** = 判不过就不发（安全 ✓）；**关** = 照样真发（可能真扣豆 ✓）。 */
  const localJudge = ref(readLocalJudgePref())
  /** 上一次兑换的结论。 */
  const result = ref<ExchangeRunOutcome | null>(null)

  // ── 定时抢兑弹窗的状态 ────────────────────────────────────────────────
  /** 正在给哪一件排定时。 */
  const planning = ref<ExchangePrizeView | null>(null)
  const planAccount = ref('')
  const planAt = ref(toLocalInput(nextMidnight()))
  const planLead = ref(120)

  /**
   * **传输延迟补偿**（毫秒）—— 用户口径 2026-10-02：「10ms 是本地与云端的传输时间差」。
   *
   * ⚠ 它**不是**"提前多少开始跑"（那是 `planLead`，秒、给调度器用的），
   * 而是**任务内部**"整点前多少毫秒发枪"（与参考实现 `standalone.ts:213` 的
   * `offset = 10` 同值）⇒ 提前这 10ms 发出，让请求**正好在整点抵达**云端 ✓。
   */
  const planOffset = ref(10)
  const planError = ref('')
  const planSaving = ref(false)

  /**
   * 用户手动折/展过的组（键 = `groupId` 的字符串形态）。
   *
   * ⚠ 为什么要单独记一份：`<details>` 的原生开合状态活在 **DOM** 上，而清单
   * **每读一次就重建这一批节点**（兑完一件就重读）⇒ 不记的话用户折好的组每次都被弹回去。
   * `open` 属性只负责"第一次看见时的默认值"（有可兑件就展开、整组都不可兑就收起）。
   */
  const groupOpen = ref<Record<string, boolean>>({})

  /**
   * 当前选中的分组 tab（`'all'` = 全部）。
   *
   * 分组有两层用法，**两层都留着**（用户口径 2026-10-01：「分组可折叠」+「分组可以通过 tab 切换」）：
   * tab 决定**看哪一组**（一次只渲染一组 ⇒ 列表天然短），
   * 而"全部"那一页里每个组仍然可以单独折叠。
   */
  const activeGroup = ref<string>('all')

  /** 第一次看见某个组时的默认开合。 */
  const groupOpenOf = (key: string, hasOpen: boolean): boolean =>
    groupOpen.value[key] ?? hasOpen

  /**
   * 记下用户的开合。
   *
   * ⚠ 值没变就直接返回：把 `open` 写回 DOM 会**再触发一次 `toggle` 事件**，
   * 不设这道闸就是"事件 → 改状态 → 改属性 → 事件"的回环。
   */
  const onGroupToggle = (key: string, open: boolean) => {
    if (groupOpen.value[key] === open) return
    groupOpen.value = { ...groupOpen.value, [key]: open }
  }

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
      // 选中的那个组可能已经不在了（活动下线 / 换了账号）⇒ 回落「全部」，
      // 否则界面会停在一个永远空的 tab 上。
      if (
        activeGroup.value !== 'all' &&
        !list.value.prizes.some(
          (p) => (p.groupId == null ? 'none' : String(p.groupId)) === activeGroup.value,
        )
      ) {
        activeGroup.value = 'all'
      }
    } catch (e) {
      list.value = null
      listError.value = e instanceof Error ? e.message : String(e)
    }
  }

  // ─────────────────────────────────────────────────────────────────────
  // 订阅：钉在顶部的那几件（用户口径 2026-10-02）
  // ─────────────────────────────────────────────────────────────────────

  /**
   * 我订阅的那几件。**服务端**存（`GET /api/exchange/subscriptions`）——
   * 换浏览器、换设备也还在 ✓。
   *
   * ⚠ "跟账号走"指的是**登录账号**，不是被操作的云盘账号：同一个人换个云盘账号，
   * 想看的东西还是那几件 ✓（所以服务端按 owner 分表，见 `UserStore` 那条注释）。
   */
  const subscriptions = ref<ExchangeSubscriptionView[]>([])
  /** 正在切换的那一件（`null` = 没在切）。两颗入口（星星 / 顶部取消）共用它。 */
  const subBusy = ref<number | null>(null)
  /**
   * 订阅这条链**自己的**错误。
   *
   * ⚠ 与清单的 `listError` **分开**：两者该分别说清 ✓（清单挂了是"这页没数据"，
   * 订阅挂了只是"顶上那块没读到"）。
   */
  const subError = ref('')

  const loadSubscriptions = async () => {
    try {
      subscriptions.value = (await listExchangeSubscriptions()).items
      subError.value = ''
    } catch (e) {
      // ⚠ 铁律：**不许吞错**（见 `mcloud-tasks` 模块头）—— 读不到就如实说一句 ✓。
      // 但也**不**把整页拖垮：清单才是这一页的主线，订阅只是顶上那块附加信息 ✓。
      subError.value = e instanceof Error ? e.message : String(e)
    }
  }

  const toggleSubscribe = async (prize: ExchangePrizeView) => {
    if (subBusy.value !== null) return
    subBusy.value = prize.prizeId
    try {
      // 名字 / 组名作为**快照**存进服务端：清单里没有这件时（下架、换账号），
      // 顶上那一行仍要是人看得懂的名字，而不是一串 `prizeId` ✓。
      subscriptions.value = (
        await toggleExchangeSubscription({
          prizeId: prize.prizeId,
          name: prize.name ?? '',
          groupId: prize.groupId,
          groupTitle: prize.groupTitle,
        })
      ).items
      subError.value = ''
    } catch (e) {
      subError.value = e instanceof Error ? e.message : String(e)
    } finally {
      subBusy.value = null
    }
  }

  /** 从顶上那块取消订阅：**只发 id** 就够（当初那个名字不一定还留着 ✓）。 */
  const unsubscribe = async (s: ExchangeSubscriptionView) => {
    if (subBusy.value !== null) return
    subBusy.value = s.prizeId
    try {
      subscriptions.value = (await toggleExchangeSubscription({ prizeId: s.prizeId })).items
      subError.value = ''
    } catch (e) {
      subError.value = e instanceof Error ? e.message : String(e)
    } finally {
      subBusy.value = null
    }
  }

  const isSubscribed = (prizeId: number): boolean =>
    subscriptions.value.some((s) => s.prizeId === prizeId)

  /** 订阅里那件在**当前账号**清单里的对应项（`null` = 这个账号的清单里没有它）。 */
  const liveOf = (s: ExchangeSubscriptionView): ExchangePrizeView | null =>
    list.value === null ? null : (list.value.prizes.find((p) => p.prizeId === s.prizeId) ?? null)

  const load = async () => {
    state.value = 'loading'
    try {
      await loadAccounts()
      await loadPrizes()
      // 订阅与清单是**两条独立的链**：订阅读挂了只在顶上那块说一句 ✓
      // （`loadSubscriptions` 自己吞下错误并记进 `subError`，不会把这里抛断 ✓）。
      await loadSubscriptions()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : String(e)
      state.value = 'error'
    }
  }

  onMounted(load)

  /** 点"兑换"：先记下这一件；**关了确认就直接下单**。 */
  const askExchange = (prize: ExchangePrizeView) => {
    pending.value = prize
    if (!confirmFirst.value) {
      // `doExchange` 自己会收尾（关弹窗、清 pending、刷新清单），
      // 所以这里不重复那几步 —— 两条路走同一段代码，行为才不会分叉。
      void doExchange()
      return
    }
    openDialog(CONFIRM_ID)
  }

  const doExchange = async () => {
    const prize = pending.value
    if (!prize) return
    busyPrizeId.value = prize.prizeId
    try {
      result.value = await runExchange(account.value, {
        prizeIdOrName: String(prize.prizeId),
        // 开关是"正着说"的 ⇒ 传给后端的是它的**反面**（`force` = 不走本地判定）。
        force: !localJudge.value,
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
    // 按**这件自己露出的口径**猜一个占位时刻（每日 ⇒ 0 点、只有每月名额 ⇒ 下月 1 日），
    // 弹窗里会写明"是猜的、复位时刻因活动而异"。见 `guessResetAt` 的注释。
    planAt.value = toLocalInput(guessResetAt(prize))
    planLead.value = 120
    planOffset.value = 10
    planError.value = ''
    openDialog(PLAN_ID)
  }

  const submitSchedule = async () => {
    const prize = planning.value
    if (!prize) return
    /**
     * ⚠ 弹窗优先 + 就近（2026-10-08 用户口径「所有的提示都是优先弹窗式」）：
     * 这一句除了画在弹窗里（就在「加到定时」上面），还要上一条 toast ——
     * 弹窗里的红字在长表单里会被看漏，而错误类 toast 不自动消失（见 `ui/toast.ts`）。
     */
    const planFail = (msg: string): void => {
      planError.value = msg
      pushToast('error', msg)
    }
    const computed = buildPlan(planAt.value, planLead.value)
    if (computed.error) {
      planFail(computed.error)
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
        body: {
          prizeIdOrName: String(prize.prizeId),
          skipWait: computed.skipWait,
          // 传输延迟补偿（毫秒）—— 与任务参数 `offset` 同名（ExchangeTask::params()）。
          offset: planOffset.value,
        },
        enabled: true,
        // ⚠ 新定时**不推**（空数组 = 一条规则都不选 / 跑完就推 ✓）——
        // 抢兑这条定时由这一页顺手建出来，没有给用户挑推送的界面 ✓。
        pushRules: [],
        pushEvents: [],
      })
      closeDialog(PLAN_ID)
      planning.value = null
      pushToast('success', '已加到定时 —— 到「定时」页可以看到下次触发时间')
    } catch (e) {
      planFail(e instanceof Error ? e.message : String(e))
    } finally {
      planSaving.value = false
    }
  }

  /**
   * 顶部「我订阅的」—— 用户口径 2026-10-02：
   * 「把想要的内容订阅到显示在上面方便直接看」。
   *
   * ⚠ 必须在**最上面**（他截图里红框那个位置）：订阅的全部意义就是"不用在长清单里翻" ✓，
   * 埋在清单底下 ✗ 等于没有 —— 这条判据写进了门禁（比较**卡片序号**，不是"页面里有这行字"✗）。
   */
  const subscribedCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-2">
        <h2 class="card-title text-base">
          我订阅的
          {subscriptions.value.length > 0 ? (
            <span class="badge badge-sm badge-ghost">{subscriptions.value.length} 件</span>
          ) : null}
        </h2>

        {subError.value ? (
          <p class="flex flex-wrap items-center gap-2 text-sm text-error">
            订阅没读到：{subError.value}
            <button type="button" class="btn btn-xs" onClick={() => void loadSubscriptions()}>
              重试
            </button>
          </p>
        ) : null}

        {subscriptions.value.length === 0 ? (
          <p class="text-sm opacity-60">
            还没有订阅。在下面任意一件奖品上点<b class="font-medium">星星</b>，它会固定到这一块
            —— 以后不用每次翻分组去找它。
          </p>
        ) : (
          <ul class="flex flex-col divide-y divide-base-200">
            {subscriptions.value.map((s) => (
              <SubscribedRow
                key={String(s.prizeId)}
                sub={s}
                live={liveOf(s)}
                busy={subBusy.value === s.prizeId}
                onExchange={askExchange}
                onSchedule={askSchedule}
                onUnsubscribe={unsubscribe}
              />
            ))}
          </ul>
        )}
      </div>
    </div>
  )

  return (
    <div class="flex flex-col gap-4">
      {/* ⚠ 顺序是刻意的（用户口径 2026-10-02）：**订阅在最上面** ——
          它就是"我盯着的那几件，一眼看到还能不能兑" ✓。 */}
      {subscribedCard()}
      {/* ── 账号 + 刷新 ─────────────────────────────────────────────── */}
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <h2 class="card-title text-base">兑换</h2>
          {/* ⚠ 这句话必须**跟着开关变**：关掉确认之后还写"点下去要再确认一次"
              就是骗人 —— 用户点下去会直接扣豆，而界面说会先问他。 */}
          <p class="text-sm opacity-70">
            {confirmFirst.value
              ? '挑一件兑换。下单会真的扣AI豆，所以点下去要再确认一次；抢不到的可以加个定时。'
              : '挑一件兑换。下单会真的扣AI豆 —— 你已经关掉二次确认，点下去就直接扣。'}
          </p>
          {/* ⚠⚠ 这一处曾经被误判成"框架的 checked 绑定坏了"（2026-10-02）：
              当时门禁的断言用 `parentElement.textContent` 去认那颗复选框，而
              textContent 一路冒泡 ⇒ `.find` 撞上了**别的**复选框 ⇒ 诊断读到的
              `checked=false` 根本不是它。改成按"紧邻兄弟的文字"精确匹配之后，
              `box.checked` 与状态**完全一致** ⇒ **框架没问题，是我的选择器错了**。
              这个控件因此留下当**回归样本**：门禁里那条断言直接读 `box.checked`。
              教训：诊断"读到的值不对"时，先确认**读的是不是那个东西**。 */}
          <label class="flex w-fit cursor-pointer items-center gap-2 text-xs opacity-70">
            <input
              type="checkbox"
              class="toggle toggle-sm"
              checked={confirmFirst.value}
              onChange={(e) => {
                const on = (e.target as HTMLInputElement).checked
                confirmFirst.value = on
                writeConfirmPref(on)
              }}
            />
            <span>下单前再确认一次</span>
          </label>

          {/* ⚠ 第二个开关，紧挨着上一个，而且**与它同向**（都是"开着 = 做那件事"✓）。
              上一个管"下单前问不问"，这个管"本地要不要先判一道"。
              它不改变"发不发请求"本身（请求本来就发），改变的是
              **请求会不会真的落到下单那一枪**。 */}
          <label
            class="flex w-fit cursor-pointer items-center gap-2 text-xs opacity-70"
            title="开着 = 本地先判一道，判不过（抢光/未开始/已兑过）就如实说「一枪未发」、不扣豆；关掉 = 这些档也真发下单请求，可能真扣AI豆"
          >
            <input
              type="checkbox"
              class="toggle toggle-sm"
              checked={localJudge.value}
              onChange={(e) => {
                const on = (e.target as HTMLInputElement).checked
                localJudge.value = on
                writeLocalJudgePref(on)
              }}
            />
            <span>
              开启本地判定
              {localJudge.value ? '' : '（已关：抢光也会真发单）'}
            </span>
          </label>

          {accounts.value.length === 0 ? (
            <div class="alert alert-warning">
              <span>还没有已登录的账号 —— 兑换要用账号的票去下单。</span>
            </div>
          ) : (
            <fieldset class="fieldset">
              <legend class="fieldset-legend">账号</legend>
              <Select
                id="exchange-account"
                class="w-full md:w-80"
                ariaLabel="账号"
                placeholder="请选择账号"
                value={account.value}
                options={accounts.value.map((a) => ({
                  value: a.id,
                  label: a.nickname || a.label || a.id,
                }))}
                onChange={(v) => {
                  account.value = v
                  // 换了账号，上一件的结论就不属于这个账号了 —— 清掉，
                  // 否则"刚兑成功"那条提示会挂在一个没兑过的账号下面。
                  result.value = null
                  // 分组也是按账号投放的：选中的 tab 与折叠状态都属于上一个账号的清单，
                  // 留着会让新账号直接停在一个它根本没有的组上。
                  activeGroup.value = 'all'
                  groupOpen.value = {}
                  void loadPrizes()
                }}
              />
            </fieldset>
          )}

          <div class="flex flex-col gap-2 text-xs opacity-60">
            {list.value ? (
              <span>
                清单读取于 {fmtEpochMs(list.value.nowMs)}
                {list.value.balance != null
                  ? ` · 余额 ${fmtNum(list.value.balance)} 颗 AI豆（判据含余额）`
                  : ''}
              </span>
            ) : null}
            {/*
              ⚠ 只有**真没读到**余额时才解释。常态是读到了 ⇒ 不说话。
              用户口径（2026-10-02）：「这是为啥这么说，无余额你就后端请求啊」——
              所以后端现在默认读余额（多一枪只读 `infoV3`），这句只在读失败时出现。
            */}
            {list.value?.balanceNote ? (
              <span class="text-warning">{list.value.balanceNote}</span>
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
                {/* ⚠ 键名是 `order_id`（后端 `detail_payload` 里就叫这个 ✓），
                    这里原先写的是 `oid` ✗ ⇒ 永远取不到 ⇒ 界面一直显示「订单号 —」。
                    ⚠ 上游**响应**里那个 10 位订单号确实叫 `oid` ✓（抓包 §6 记录第 4 问 ✓），
                    但那是**解析层**的事：`mcloud-upstream` 把它读成 `ExchangeOrder.order_id` ✓，
                    到界面这一层已经是 `order_id` 了 ✓。别把两层的名字搞混 ✓。 */}
                订单号 {String(result.value.order.order_id ?? '—')}
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
        // 编译器的三元分支只认"JSX / null"（`@asunajs/lite` 的 `compiler.ts` 的 `branchOk`）——
        // 直接把 `.map(...)` 当分支放，`key` 就不会被认领（报"这个 key 什么都不做"）。
        <div class="flex flex-col gap-4">
          {/*
            分组 tab：一次只看一组（另有「全部」）。**只有一个组名，没有数字**
            —— 用户口径（2026-10-01，附截图）：「用户其实并不理解后面的数字，甚至也不关心他是啥」。

            ⚠⚠ 为什么不用 daisyUI 的 `tabs` / `tab` 类：本项目的 `app.css` 把 `tab` 放进了
            `@plugin "daisyui"` 的 **exclude 清单**（它已经换成本仓自建实现）。
            排除掉的组件**样式会静默消失** —— 实测 `.tab` 的 padding / 边框 / `::after`
            全是 0 / none，界面退化成"一串连在一起的纯文字"，正是用户说的"比较丑"。
            项目自己那套等价物是 **`join` + `btn join-item`**（见 `tasks-page.tsx` 的任务分组 tab），
            这里照用同一套词汇，全站一致、也不多花一个字节的 CSS。
          */}
          <div role="tablist" class="join w-full overflow-x-auto" aria-label="奖品分组">
            {groupTabs(list.value.prizes).map((t) => (
              <button
                key={t.key}
                type="button"
                role="tab"
                aria-selected={t.key === activeGroup.value}
                class={`btn join-item btn-sm min-h-11 whitespace-nowrap md:min-h-0 ${t.key === activeGroup.value ? 'btn-primary' : 'btn-soft'}`}
                onClick={() => (activeGroup.value = t.key)}
              >
                {t.label}
              </button>
            ))}
          </div>
          {groupViews(list.value.prizes)
            .filter((g) => activeGroup.value === 'all' || g.key === activeGroup.value)
            .map((g) => (
              /*
                每个组**都能折**（用户口径 2026-10-01）。默认：有可兑件的展开、
                整组都不可兑的收起；**手动选中的那个 tab** 也默认展开（否则点进去只见一条标题）。
                用户手动折过之后状态记在 `groupOpen` 里，刷新清单不会弹回去。
              */
              <details
                key={g.key}
                open={groupOpenOf(g.key, g.open.length > 0 || activeGroup.value === g.key)}
                onToggle={(e) => onGroupToggle(g.key, (e.target as HTMLDetailsElement).open)}
                class="disclosure disclosure-arrow bg-base-100 shadow-sm"
              >
              <summary class="disclosure-title">
                <GroupHead group={g} openCount={g.open.length} />
              </summary>
              <div class="disclosure-content flex flex-col gap-2">
                {g.open.length > 0 ? (
                  <div class="flex flex-col divide-y divide-base-200">
                    {g.open.map((p) => (
                      <div key={p.prizeId}>
                        <PrizeRow
                          prize={p}
                          busy={busyPrizeId.value === p.prizeId}
                          locked={busyPrizeId.value !== null}
                          onExchange={askExchange}
                          onSchedule={askSchedule}
                          subscribed={isSubscribed(p.prizeId)}
                          subBusy={subBusy.value === p.prizeId}
                          onToggleSubscribe={toggleSubscribe}
                        />
                      </div>
                    ))}
                  </div>
                ) : null}
                {g.blocked.length > 0 ? (
                  <details class="disclosure disclosure-arrow border-t border-base-200">
                    <summary class="disclosure-title min-h-0 py-2 text-sm opacity-70">
                      另有 {g.blocked.length} 件不可兑（{reasonsSummary(g.blocked)}）
                    </summary>
                    <div class="disclosure-content flex flex-col divide-y divide-base-200">
                      {g.blocked.map((p) => (
                        <div key={p.prizeId}>
                          <BlockedRow
                            prize={p}
                            onSchedule={askSchedule}
                            onExchange={askExchange}
                            canForce={!localJudge.value}
                            locked={busyPrizeId.value !== null}
                            subscribed={isSubscribed(p.prizeId)}
                            subBusy={subBusy.value === p.prizeId}
                            onToggleSubscribe={toggleSubscribe}
                          />
                        </div>
                      ))}
                    </div>
                  </details>
                ) : null}
              </div>
            </details>
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
          <Select
            id="plan-account"
            class="w-full"
            ariaLabel="账号"
            placeholder="请选择账号"
            value={planAccount.value}
            options={accounts.value.map((a) => ({
              value: a.id,
              label: a.nickname || a.label || a.id,
            }))}
            onChange={(v) => (planAccount.value = v)}
          />
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">抢兑时刻</legend>
          <input
            type="datetime-local"
            class="input w-full"
            value={planAt.value}
            onInput={(e) => (planAt.value = (e.target as HTMLInputElement).value)}
          />
          {/* ⚠ 这段说明**从界面上删掉了**（用户口径 2026-10-02：「不需要说明」）✗，
              但知识留着，就放这儿：
              默认值是**按这件露出的口径猜的** —— 有「今日份额」⇒ 下一个 0 点；
              只有「本组每月名额」⇒ 下月 1 日 0 点。⚠ **不保证对**：份额复位的时刻
              **因活动而异** —— 我们实测到的那款是自然日 0 点（抓包样本 §7），
              但也有活动照自己的投放时刻（比如 08:00）。用户按自己了解的时刻改。 */}
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
            <legend class="fieldset-legend">传输延迟补偿（毫秒）</legend>
            <input
              type="number"
              class="input w-full"
              min="0"
              max="1000"
              value={String(planOffset.value)}
              onInput={(e) => (planOffset.value = Number((e.target as HTMLInputElement).value) || 0)}
            />
          </fieldset>
        </div>

        {buildPlan(planAt.value, planLead.value).error ? (
          <div class="alert alert-warning">
            <span>{buildPlan(planAt.value, planLead.value).error}</span>
          </div>
        ) : (
          <div class="rounded-box flex flex-col gap-1 bg-base-200 p-3 text-xs">
            <span>{buildPlan(planAt.value, planLead.value).human}</span>
            <span class="opacity-70">
              cron <code>{buildPlan(planAt.value, planLead.value).expr}</code>
              {' · '}时区 {browserTz()}
            </span>
          </div>
        )}

        {/* ⚠ 界面上删掉了（同上）。知识：调度器是「**宁可漏跑不可重跑**」——
            服务在那一刻不在就**不会补跑**；「定时」页能看到下次触发时间、也能停掉它。 */}

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
            disabled={planSaving.value || buildPlan(planAt.value, planLead.value).error !== null}
            onClick={submitSchedule}
          >
            {planSaving.value ? '保存中…' : '加到定时'}
          </button>
        </div>
      </Dialog>
    </div>
  )
}
