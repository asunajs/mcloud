/**
 * 直播小红花页：**看余额 / 看直播时间 / 输入口令 + 账号批量领取**。
 *
 * 对应需求里的前三条（第四、五条：兑换与自动抓取——兑换那一页另做；
 * 自动抓取那条链后端**已经**会在到点自己触发
 * （`GET /api/status` 的 `schedulerRunning` 现在是运行时实况，不再是常量），
 * 而"定时任务"那一页 2026-09-23 起能建任务了：目标选"直播口令 → 小红花"即可）。
 *
 * # 三块各自的数据来源，口径不同，别混
 *
 *  1. **状态区**（余额 + 门 + 房间日程）走 `GET /api/accounts/{id}/live-room`：
 *     只读，但**会打上游八枪**（门那三步 + 两个页签 + 云盘侧建会话三步 + 一次余额）。
 *     会话那三步不是冗余：余额那一族在云盘域，只认云盘侧的 JWT（判据见后端
 *     `app.ProbeLiveRoom` 的补记）。所以只在进页面、换账号、点"重新看"时调，**不轮询**；
 *  2. **运行区**走 `POST /api/runs` + 每个账号一条 SSE：202 先回来、结果靠事件流；
 *  3. **历史区**走 `GET /api/runs`，按 `capability` 筛出这一项。
 *
 * # ⚠ 这一页会**投口令**（`draw` 是写操作），所以处处按"不可逆"办
 *
 * 每场直播只有两条固定口令（小概率多一条随机的），而全世界共用它们 ——
 * 所以**不存在**"谁用掉了别人就没得用"。真正的不可逆在另一处：`draw` 是**写操作**，
 * 投出去就动了**这个账号**的兑换状态。
 * 由此定下三条界面上的硬规矩：
 *  - 状态区**绝不**触发跑一趟（那是原项目"顺手跑一次看看"的做法）；
 *  - 起跑前必过确认弹窗，且把影响面写清楚（§6.4）；
 *  - 「重新看」与「开始领取」是两枚不同的按钮，不复用同一句文案。
 *
 * 反之 `200112`（该口令已兑换）说的是**这一号自己把这条口令投过一次**，
 * 是正常逻辑：兑换状态**按账号**算，不同账号投同一条口令**各自都会成功**，
 * 不存在"别人用掉了我就只能拿到 200112"那种全局配额（用户 2026-09-22 两次纠正这一点）。
 * ⚠ 也**不要**把它读成"口令被消耗"（2026-09-23 再纠一次）—— 口令是全场通用的，
 * 见 `internal/upstream/live_flower.go` 文件头五条事实②。
 */

import { computed, onMounted, onUnmounted, ref } from 'vue'
import {
  ApiError,
  listAccounts,
  listRuns,
  probeLiveRoom,
  runEventsUrl,
  triggerRun,
  type Account,
  type LiveRoomProbe,
  type RunRecord,
} from '../api'
import { closeDialog, ConfirmDialog, openDialog } from '../ui/dialog'
import { fmtTime } from '../ui/time'
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

type LoadState = 'loading' | 'error' | 'ready'

/** 弹窗 id 在整站唯一（§5.8）。 */
const CONFIRM_ID = 'dlg-live-room-run'

/** 这一页只做这一项能力；参数键的权威是后端的能力声明，不是这里。 */
const CAPABILITY = 'live-room'

/**
 * 听弹幕时长的缺省。**与声明里那个 `60` 同一个人填**（CLI 的 `--listen-seconds` 是第三处）：
 * 留空不送这一栏时后端用自己的默认值，界面上这一栏只是把那个默认值**显式摆出来**，
 * 免得出现"界面显示 60、实际听了别的数"。
 */
const DEFAULT_LISTEN_SECONDS = 60

/**
 * 房间状态 → 中文 + badge 类（对位后端 `statusToken` 那四个稳定英文码）。
 *
 * ⚠ 类名**整条写成字面量**：Tailwind 靠扫源码取类名，拼出来的字符串不进产物（§9.1①）。
 * `unknown` 不是"没匹配上就没事"——上游哪天加一档状态，这里要**看得见**，
 * 所以它的文案是"状态未知"而不是套用"未开始"。
 */
const ROOM_STATUS: Record<string, { label: string; badge: string }> = {
  live: { label: '直播中', badge: 'badge-soft badge-success' },
  'not-started': { label: '预告', badge: 'badge-soft badge-info' },
  ended: { label: '已结束', badge: 'badge-soft' },
  unknown: { label: '状态未知', badge: 'badge-soft badge-warning' },
}

const roomStatus = (token: string) => ROOM_STATUS[token] ?? ROOM_STATUS.unknown

/** 一个账号这一趟的累计视图。`stream` 整个换掉（不原地改），否则 Vapor 收不到更新。 */
interface RunRow {
  accountId: string
  name: string
  runId: string
  /** 本地起跑时刻（毫秒），只用来算"已用时长"。 */
  startedAtMs: number
  /** 收到终局事件那一刻；null = 还没结束（时长要跟着心跳走）。 */
  endedAtMs: number | null
  stream: RunStreamState
}

export const LiveRedFlowerPage = () => {
  const state = ref<LoadState>('loading')
  const accounts = ref<Account[]>([])
  const errorMessage = ref('')

  /** 状态区看的是**哪一个**账号（余额按账号各一份，没有"全局余额"这种东西）。 */
  const probeAccountId = ref('')
  const probe = ref<LiveRoomProbe | null>(null)
  const probeState = ref<LoadState>('loading')
  const probeError = ref('')

  // ── 表单：与能力声明的五个键一一对应（多一个键后端就 400） ──
  const codes = ref('')
  const listenSeconds = ref(DEFAULT_LISTEN_SECONDS)
  const koulingOnly = ref(false)
  const skipKouling = ref(false)
  /** 选中的账号 id。空数组 = 让后端按"全部启用账号"缺省（声明里那句话）。 */
  const selected = ref<string[]>([])

  const submitting = ref(false)
  const actionError = ref('')
  const rows = ref<RunRow[]>([])
  const history = ref<RunRecord[]>([])

  /**
   * 心跳。只为"已用时长"那一格存在（§6.3 要显示已用时长）：
   * 一条直播链要听满几十秒，没有它在动，用户会以为页面死了去刷新——
   * 而刷新会关掉 SSE，那一趟的日志就再也接不上（历史页还能看到结果）。
   */
  const tick = ref(0)
  let ticker: number | undefined

  /** 每条流的 `EventSource`。关页面挡不住后端那一趟（那是真实行为），但连接必须自己关。 */
  const sources = new Map<string, EventSource>()

  /**
   * "还没轮到这一趟"时排下的重订阅定时器（键 = runId）。
   *
   * 为什么需要它：`runs` 那一行是后台 goroutine 在**轮到那个账号时**才写的
   * （`app.Runner.runOne` 的 `InsertRun`），而账号是串行跑的 —— 于是 202 给出的
   * 第 2..N 个 runId 天然有一段"配对已在手上、事件流还查不到"的空窗，
   * 长度 = 前面所有账号的耗时之和。这一窗口的表现是 SSE 回 404、`EventSource`
   * 按规范**不会重连**，卡片就永远停在"连接中断"，而那趟其实跑得好好的。
   * ⇒ 一条事件都没到过的那一支不算断流，隔一秒重订阅。
   */
  const retryTimers = new Map<string, number>()

  /** 重订阅的间隔与次数上限。上限给到一小时：一批几十个账号、每个听满几十秒时，排到后面等得起。 */
  const RESUBSCRIBE_MS = 1000
  const RESUBSCRIBE_MAX = 3600

  const enabledAccounts = computed(() => accounts.value.filter((a) => a.enabled))

  /** 这一趟要点下去的账号数：勾选为空 = 后端会跑全部启用账号，界面必须按同一口径显示。 */
  const targetCount = computed(
    () => (selected.value.length > 0 ? selected.value.length : enabledAccounts.value.length),
  )

  /** 拆口令（与后端 `SplitKouling` 同一口径：换行或逗号分隔），只为确认弹窗数个数。 */
  const codeList = computed(() =>
    codes.value
      .split(/[\n,，]/)
      .map((v) => v.trim())
      .filter((v) => v !== ''),
  )

  /**
   * 影响面。**具体到数字**（§6.4：只写"确定吗？"等于没确认）。
   *
   * 三个开关组合出三种说法，其中"只听不投"那一支必须**明说这次一个口令都不投**——
   * 否则用户会以为点一下也把口令投了出去（动了这个账号的兑换状态），
   * 而那次点击恰恰是安全的那一种。
   */
  const impact = computed(() => {
    const n = targetCount.value
    if (koulingOnly.value) {
      return (
        <div class="flex flex-col gap-2">
          <span>
            这一趟<b class="font-medium">只听不投</b>：对 <b class="font-medium">{n}</b> 个账号各收一次口令并打印，
            <b class="font-medium">一个口令都不投给上游</b>。
          </span>
          <span class="opacity-70">
            自动收口令会占住听的时间（每号约 {listenSeconds.value} 秒），期间这一号的其它任务不会插进来。
          </span>
        </div>
      )
    }
    const how =
      codeList.value.length > 0
        ? `把填进来的 ${codeList.value.length} 个口令逐个投给这 ${n} 个账号`
        : '先自动收口令（弹幕 + 小红书），再把收到的口令投给这 ' + n + ' 个账号'
    return (
      <div class="flex flex-col gap-2">
        <span>
          这一趟会<b class="font-medium">真的投口令</b>：{how}。
        </span>
        <span class="opacity-70">
          投口令是<b class="font-medium">写操作</b>：每场直播全世界共用那两条口令，而兑换状态<b class="font-medium">按账号</b>算
          —— 同一条口令投给两个不同账号，两边都是各自的第一次，<b class="font-medium">不会互相挡</b>。
          只有<b class="font-medium">同一号</b>把同一条再投一次才回「该口令已兑换」，那是它自己用过了。
        </span>
        {skipKouling.value ? (
          <span class="opacity-70">已打开「只用给定口令」：这一次不进直播间、不收口令，直接投上面填的那些。</span>
        ) : null}
      </div>
    )
  })

  // ─────────────────────────────────────────────────────────────────────────
  // 取数
  // ─────────────────────────────────────────────────────────────────────────

  const loadProbe = async (id: string) => {
    if (id === '') {
      probe.value = null
      probeState.value = 'ready'
      probeError.value = ''
      return
    }
    probeState.value = 'loading'
    probeError.value = ''
    try {
      probe.value = await probeLiveRoom(id)
      probeState.value = 'ready'
    } catch (e) {
      probe.value = null
      probeState.value = 'error'
      // 401 **不**翻成"登录已过期"：那串是"所有主动挡你的行为"，原因可能是别的。
      probeError.value = e instanceof ApiError ? e.message : '无法连接后端'
    }
  }

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [accountResult, runResult] = await Promise.all([listAccounts(), listRuns(50)])
      accounts.value = [...accountResult.accounts].sort((a, b) => a.sortOrder - b.sortOrder)
      history.value = runResult.runs.filter((r) => r.capability === CAPABILITY)
      state.value = 'ready'
      // 默认看第一个**启用中**的账号。
      //
      // ⚠ 这里一度按 `platform === 'mobile'` 挑，理由是"PC 端那一号会回 90001 未登录"。
      // **那个理由是错的**：90001 是凭据域搞错了（六步链要云盘侧 `jwttoken`，
      // 见 `internal/app/live_room_probe.go` 的补记），与账号是 PC 还是手机无关 ——
      // 修好之后两个平台标签的账号都读得到余额（2026-09-22 实跑：`15****` 235 朵、
      // `19****` 110 朵）。多一层"优先手机端"的排序只会让人以为平台标签有关系。
      probeAccountId.value = enabledAccounts.value[0]?.id ?? ''
      await loadProbe(probeAccountId.value)
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /** 历史那一档：**这一页跑完**的趟要出现在下面，否则用户只能去历史页找。 */
  const refreshHistory = async () => {
    try {
      const result = await listRuns(50)
      history.value = result.runs.filter((r) => r.capability === CAPABILITY)
    } catch {
      // 历史是**补充**信息：拿不到不该把已经跑完的那趟说成失败（失败文案在卡片上）
    }
  }

  // ─────────────────────────────────────────────────────────────────────────
  // 事件流
  // ─────────────────────────────────────────────────────────────────────────

  /**
   * 改一条流。**整行换掉**而不是原地改 `row.stream`：Vapor 下只有 ref 的引用变了
   * 才重渲染那些卡片（与账号页 `accounts.value = [...]` 同一条纪律）。
   * `endedAtMs` 只在终局那一次给，其它调用点不传就留着原值。
   */
  const patchStream = (
    runId: string,
    fn: (s: RunStreamState) => RunStreamState,
    endedAtMs?: number,
  ) => {
    rows.value = rows.value.map((row) =>
      row.runId === runId
        ? { ...row, stream: fn(row.stream), endedAtMs: endedAtMs ?? row.endedAtMs }
        : row,
    )
  }

  const closeSource = (runId: string) => {
    const timer = retryTimers.get(runId)
    if (timer !== undefined) {
      window.clearTimeout(timer)
      retryTimers.delete(runId)
    }
    sources.get(runId)?.close()
    sources.delete(runId)
  }

  /**
   * 订阅一个账号的事件流。`attempt` 只在重订阅那条路上自增（见 [retryTimers] 的注释）。
   *
   * ⚠ 收到终局事件**必须自己 `close()`**（后端 `runEvents` 的注释里那条契约）：
   * `EventSource` 会按规范自动重连，不关就是一个跑完的 run 被一直重连、
   * 每次带 `Last-Event-ID` 让服务端重放 0 条再关闭。
   *
   * ⚠ 404 分两种，而它们**长得一样**：
   *   - 一条事件都没到过 ⇒ 多半只是"还没轮到这一趟落库"（串行队列后面的账号），
   *     那不是断流 ⇒ 重订阅；
   *   - 已经收到过事件才断 ⇒ 真的是连接断了，报"没连上"，
   *     具体原因留给同一条卡片上的那一趟的最终状态（历史区会补上）。
   *   被账号锁挡下的那一趟根本没有记录，所以它会一路重订阅到次数上限才落到"没连上"——
   *   ⚠ 这一支**没有断言**（web 侧没有测试运行器），要靠真机核验。
   */
  const subscribe = (row: RunRow, attempt = 0) => {
    const source = new EventSource(runEventsUrl(row.runId))
    sources.set(row.runId, source)
    source.onmessage = (message) => {
      const event = parseRunEvent(message.data)
      if (!event) return
      patchStream(row.runId, (s) => applyEvent(s, event))
      if (isTerminalEvent(event)) {
        patchStream(row.runId, (s) => applyEvent(s, event), Date.now())
        closeSource(row.runId)
        void refreshHistory()
        if (sources.size === 0) {
          stopTicker()
          // 最后一趟跑完就把余额重新读一次：六步链的⑥就是"投完再看余额"，
          // 而界面上那一格是**探针**给的，不重新读会一直停在起跑前那个数。
          void loadProbe(probeAccountId.value)
        }
      }
    }
    source.onerror = () => {
      const runId = row.runId
      closeSource(runId)
      const current = rows.value.find((r) => r.runId === runId)
      if (current && current.stream.phase === 'pending' && attempt < RESUBSCRIBE_MAX) {
        retryTimers.set(
          runId,
          window.setTimeout(() => {
            retryTimers.delete(runId)
            const latest = rows.value.find((r) => r.runId === runId)
            if (latest) subscribe(latest, attempt + 1)
          }, RESUBSCRIBE_MS),
        )
        return
      }
      patchStream(runId, markLost)
      if (sources.size === 0) stopTicker()
    }
  }

  // ─────────────────────────────────────────────────────────────────────────
  // 起跑
  // ─────────────────────────────────────────────────────────────────────────

  const askStart = () => {
    actionError.value = ''
    if (targetCount.value === 0) {
      actionError.value = '没有可跑的账号：先在账号页添加并启用一个'
      return
    }
    if (skipKouling.value && codeList.value.length === 0) {
      actionError.value = '打开「只用给定口令」时，上面那一栏至少要填一个口令'
      return
    }
    if (koulingOnly.value && skipKouling.value) {
      actionError.value = '「只听不投」与「只用给定口令」互斥，两者不能同时打开'
      return
    }
    openDialog(CONFIRM_ID)
  }

  const start = async () => {
    closeDialog(CONFIRM_ID)
    submitting.value = true
    actionError.value = ''
    try {
      // 送**全部六个键**（连没改过的也一样）：漏一个就用后端默认值，
      // 那会让"界面上显示的数字"与"实际跑的数"分家（见 DEFAULT_LISTEN_SECONDS 那条）。
      const accepted = await triggerRun(CAPABILITY, {
        accounts: selected.value,
        accountMode: 'sequential',
        codes: codes.value,
        listenSeconds: Number(listenSeconds.value) || 0,
        koulingOnly: koulingOnly.value,
        skipKouling: skipKouling.value,
      })
      const nameOf = (id: string) =>
        accounts.value.find((a) => a.id === id)?.displayName ?? id
      rows.value = accepted.runs.map((r) => ({
        accountId: r.accountId,
        name: nameOf(r.accountId),
        runId: r.runId,
        startedAtMs: Date.now(),
        endedAtMs: null,
        stream: newStreamState(),
      }))
      startTicker()
      rows.value.forEach((r) => subscribe(r))
    } catch (e) {
      actionError.value = e instanceof ApiError ? e.message : '发起失败：无法连接后端'
    } finally {
      submitting.value = false
    }
  }

  const startTicker = () => {
    if (ticker !== undefined) return
    ticker = window.setInterval(() => (tick.value += 1), 1000)
  }

  const stopTicker = () => {
    if (ticker !== undefined) {
      window.clearInterval(ticker)
      ticker = undefined
    }
  }

  /**
   * 还在跑的趟：停心跳与放开按钮都看它。
   *
   * ⚠ `lost`（连接断了）**不算**在跑：后端那一趟可能确实还在跑，但我们已经
   * 没有事实来源了。把它算成"在跑"的界面表现是"卡片都在、按钮永远灰着"，
   * 而用户什么也做不了 —— 不如让他再点一次，让卡片与历史说话。
   */
  const active = computed(
    () => rows.value.some((r) => r.stream.phase === 'pending' || r.stream.phase === 'open'),
  )

  const elapsedOf = (row: RunRow) => {
    void tick.value // 让这一格跟着心跳重算（Vapor 下不读就不会建依赖）
    return formatElapsed((row.endedAtMs ?? Date.now()) - row.startedAtMs)
  }

  /**
   * 换账号看：**只**重新探针，不碰表单。
   *
   * 表单与"看哪个账号的余额"是两件事：把口令清空会让人以为刚才白填了，
   * 而后端那一趟本来就用的是表单里那串，与上面看的是谁的余额无关。
   */
  const onPickProbeAccount = (id: string) => {
    probeAccountId.value = id
    void loadProbe(id)
  }

  onMounted(() => {
    void load()
  })
  onUnmounted(() => {
    // 关连接，但**不**取消后端那一趟（它已经往上游投了口令，半途掐掉会留下一条
    // 永远 `running` 的记录，而真实动作已经发生了 —— 后端那边特意脱开了请求上下文）
    retryTimers.forEach((timer) => window.clearTimeout(timer))
    retryTimers.clear()
    sources.forEach((source) => source.close())
    sources.clear()
    stopTicker()
  })

  // ─────────────────────────────────────────────────────────────────────────
  // 渲染
  // ─────────────────────────────────────────────────────────────────────────

  /** 门的结论：**两种"没有"必须分得开**（等开播 / 别等了），所以看两个数一起判。 */
  const gateView = computed(() => {
    const p = probe.value
    if (!p) return null
    if (p.koulingLive) {
      return { badge: 'badge-soft badge-success', text: `现在正在播，可以去听口令（${p.anchor}）` }
    }
    if (p.liveCount > 0) {
      return {
        badge: 'badge-soft badge-warning',
        text: `有 ${p.liveCount} 间在播，但没有一间是「${p.anchor}」—— 这场没有口令环节，等下一场`,
      }
    }
    return { badge: 'badge-soft', text: '现在一间都没在播，今天没有这场直播' }
  })

  /** 下一场：预告场那个时间（后端刻意把预告留在日程里，就是为了这一格）。 */
  const nextStart = computed(() => {
    const p = probe.value
    if (!p) return ''
    const preview = p.rooms.find((r) => r.statusToken === 'not-started' && r.expectStartTime !== '')
    return preview ? preview.expectStartTime : ''
  })

  const accountSelect = () => (
    <label class="flex flex-col gap-1 text-sm md:flex-row md:items-center">
      <span class="opacity-70">看哪个账号</span>
      <select
        class="select w-full md:w-64"
        aria-label="选择要查看余额与直播日程的账号"
        value={probeAccountId.value}
        onChange={(e) => onPickProbeAccount((e.target as HTMLSelectElement).value)}
      >
        {accounts.value.length === 0 ? <option value="">（还没有账号）</option> : null}
        {accounts.value.map((a) => (
          <option value={a.id}>
            {a.displayName}
            {a.enabled ? '' : '（已停用）'}
          </option>
        ))}
      </select>
      <button
        type="button"
        class="btn btn-soft btn-sm min-h-11 md:min-h-0"
        disabled={probeState.value === 'loading' || probeAccountId.value === ''}
        onClick={() => void loadProbe(probeAccountId.value)}
      >
        {probeState.value === 'loading' ? '看上游中…' : '重新看'}
      </button>
    </label>
  )

  /** 第一屏：余额 + 门 + 下一场（§5.1，窄屏竖排）。 */
  const statsCards = () => {
    const p = probe.value
    const gate = gateView.value
    return (
      <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
        <div class="stat">
          <div class="stat-title">小红花余额</div>
          <div class="stat-value text-2xl">
            {p?.balance ? p.balance.flowerCnt : probeState.value === 'loading' ? '…' : '—'}
          </div>
          <div class="stat-desc">
            {p?.balance ? `有效期至 ${p.balance.validDate}` : (p?.balanceError ?? '还没读到')}
          </div>
        </div>
        <div class="stat">
          <div class="stat-title">这一场</div>
          <div class="stat-value text-2xl">
            {probeState.value === 'loading' ? (
              <span class="loading loading-spinner loading-sm" />
            ) : (
              <span class={`badge ${gate?.badge ?? 'badge-soft'}`}>
                {probeState.value === 'error' ? '读不到' : (gate?.text.slice(0, 2) ?? '—')}
              </span>
            )}
          </div>
          <div class="stat-desc break-words">{gate?.text ?? (p ? '' : '先选一个账号')}</div>
        </div>
        <div class="stat">
          <div class="stat-title">下一场</div>
          <div class="stat-value text-2xl">{nextStart.value || '—'}</div>
          <div class="stat-desc">
            {nextStart.value ? '主播预告的开播时间，按上游给的本地时间原样显示' : '上游没给预告时间'}
          </div>
        </div>
      </div>
    )
  }

  /** 上游说"这一号还有没领走的"才提示；读不到余额时不提示（那不等于"领完了"）。 */
  const hasNotReceivedHint = () => {
    const b = probe.value?.balance
    if (!b) return null
    if (b.hasNotReceived === 1) {
      return <div class="alert alert-soft alert-info text-sm">上游说这一号还有没领走的小红花。</div>
    }
    return null
  }

  /** 房间日程：桌面表格、移动卡片（§5.3），同一数据源。 */
  const roomTable = () => (
    <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
      <table class="table table-sm">
        <thead>
          <tr>
            <th>房间</th>
            <th>状态</th>
            <th>预告开播</th>
            <th>主播</th>
          </tr>
        </thead>
        <tbody>
          {(probe.value?.rooms ?? []).map((r) => (
            <tr>
              <td class="font-medium break-words">{r.title}</td>
              <td>
                <span class={`badge ${roomStatus(r.statusToken).badge}`}>
                  {roomStatus(r.statusToken).label}
                </span>
              </td>
              <td class="text-sm opacity-70">{r.expectStartTime || '—'}</td>
              <td class="text-sm opacity-70">{r.anchor}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )

  /** 移动卡片：切了卡片每项必须带标签（§7.3）。 */
  const roomCards = () => (
    <div class="grid gap-3 md:hidden">
      {(probe.value?.rooms ?? []).map((r) => (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-2 p-4">
            <div class="flex items-center justify-between gap-2">
              <span class="break-words font-medium">{r.title}</span>
              <span class={`badge ${roomStatus(r.statusToken).badge}`}>
                {roomStatus(r.statusToken).label}
              </span>
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">预告开播</span>
              <span class="break-words">{r.expectStartTime || '—'}</span>
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">主播</span>
              <span class="break-words">{r.anchor}</span>
            </div>
          </div>
        </div>
      ))}
    </div>
  )

  const probeSection = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3 p-4 md:p-6">
        <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
          <h2 class="card-title text-base">直播状态与余额</h2>
          {accountSelect()}
        </div>
        {probeState.value === 'error' ? (
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">
              这次没能问出来。{probeError.value} —— 门坏了不会显示成"今天没开播"，
              那两类结论得分开（下一步：去账号页刷新那一行的 auth，或用短信重新登录一次）。
            </span>
          </div>
        ) : probeState.value === 'ready' && probeAccountId.value === '' ? (
          <div class="alert alert-soft text-sm">
            <span>还没有可看的账号。先去「账号」页登录一个（这条链走的是手机 App 端那支）。</span>
          </div>
        ) : (
          <div class="flex flex-col gap-3">
            {statsCards()}
            {hasNotReceivedHint()}
            {probe.value && probe.value.rooms.length > 0 ? (
              <div class="flex flex-col gap-3">
                {roomTable()}
                {roomCards()}
              </div>
            ) : (
              <p class="text-sm opacity-70">
                {probe.value
                  ? `上游的日程里没有「${probe.value.anchor}」的房间。开播之后这一栏会列出那几间。`
                  : '正在读上游…'}
              </p>
            )}
          </div>
        )}
        <p class="text-xs leading-5 opacity-60">
          上面这一块只<b class="font-medium">读</b>：它打八次上游，但一条写操作都不发 ——
          看一眼不会把口令用掉。（门走直播侧的 <code>Authorization</code>，
          余额走云盘侧的 <code>jwttoken</code>，两套凭据各换一次，所以是八次不是五次。）
        </p>
      </div>
    </div>
  )

  const formSection = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3 p-4 md:p-6">
        <h2 class="card-title text-base">领取小红花</h2>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">口令</legend>
          <textarea
            class="textarea w-full"
            rows={3}
            autocomplete="off"
            spellcheck={false}
            placeholder="留空 = 自动从弹幕与小红书收集；多个用换行或逗号分隔"
            value={codes.value}
            onInput={(e) => (codes.value = (e.target as HTMLTextAreaElement).value)}
          />
          {/* 见上面 742 行的注释：`fieldset-label` 是 `display:flex`，会把 `<b>` 压成竖排单字符列。 */}
          <p class="text-xs leading-5 opacity-60">
            口令只能<b class="font-medium">当场</b>从直播里拿到：从别处抄来的会回「口令不对耶」。
            每场直播全世界共用那两个口令。
          </p>
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">听弹幕时长（秒）</legend>
          <input
            class="input w-full md:w-40"
            type="number"
            min={1}
            step={1}
            aria-label="听弹幕时长（秒）"
            value={listenSeconds.value}
            onInput={(e) => (listenSeconds.value = Number((e.target as HTMLInputElement).value))}
          />
          <p class="fieldset-label">默认 60。只在"口令留空 = 自动收集"时才生效。</p>
        </fieldset>

        {/* 两个开关各占一行：daisyUI 的 toggle 本体小，靠 label 把触点撑到 44px（§6.1） */}
        <div class="flex flex-col gap-2">
          <label class="flex min-h-11 cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              class="toggle toggle-primary"
              checked={koulingOnly.value}
              onChange={(e) => (koulingOnly.value = (e.target as HTMLInputElement).checked)}
            />
            <span class="text-sm">只听不投（只收口令并打印，一个口令都不投给上游）</span>
          </label>
          <label class="flex min-h-11 cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              class="toggle toggle-primary"
              checked={skipKouling.value}
              onChange={(e) => (skipKouling.value = (e.target as HTMLInputElement).checked)}
            />
            <span class="text-sm">只用给定口令（不进直播间、不收口令，必须先在上一栏填口令）</span>
          </label>
        </div>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">跑哪些账号</legend>
          {enabledAccounts.value.length === 0 ? (
            <p class="text-sm opacity-70">没有启用中的账号。去「账号」页启用一个再来。</p>
          ) : (
            <div class="flex flex-col gap-1">
              {enabledAccounts.value.map((a) => (
                <label class="flex min-h-11 cursor-pointer items-center gap-2">
                  <input
                    type="checkbox"
                    class="checkbox checkbox-sm"
                    checked={selected.value.includes(a.id)}
                    aria-label={`跑 ${a.displayName}`}
                    onChange={(e) => toggleAccount(a.id, (e.target as HTMLInputElement).checked)}
                  />
                  <span class="text-sm break-words">{a.displayName}</span>
                </label>
              ))}
            </div>
          )}
          {/* 别套 `fieldset-label`：那个 class 是 `display:flex`，会把"全角空格 / 标点 / `<b>`"
              各当成一个 flex 项目；窄容器里 `<b>` 被压成单字符列、汉字逐字竖排——就是 742 那段
              截图里"手机/App/端、无/关/联、全/部/启/用"那种诡异排版。换成正常的 block 段落样式。 */}
          <p class="text-xs leading-5 opacity-60">
            一个都不勾 = 跑<b class="font-medium">全部启用</b>的账号。
            这一条链要的是那支<b class="font-medium">手机 App 端</b>的登录态（门与六步链都从它换凭据），
            与账号列表上那个「PC 端 / 手机端」标签<b class="font-medium">无关</b> —— 标签说的是"这个号当初怎么加进来的"，
            而库里每一行都同时存着两支刷新凭据。真缺那一支时卡片会回一句未登录，不是静默跳过。
          </p>
        </fieldset>

        {actionError.value ? (
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">{actionError.value}</span>
          </div>
        ) : null}

        <div class="modal-action">
          <button
            type="button"
            class="btn btn-primary min-h-11 md:min-h-0"
            disabled={submitting.value || active.value || enabledAccounts.value.length === 0}
            onClick={askStart}
          >
            {submitting.value ? '发起中…' : '开始领取'}
          </button>
        </div>
        <p class="text-xs leading-5 opacity-60">
          {active.value
            ? '这一趟正在跑（账号之间是串行的，防风控）。'
            : '点下去会弹一次确认：投口令是写操作，会动这个账号的兑换状态。'}
        </p>
      </div>
    </div>
  )

  const toggleAccount = (id: string, on: boolean) => {
    // 整个数组换掉：原地 push/splice 在 Vapor 下不会让依赖它的 `targetCount` 重算
    selected.value = on ? [...selected.value, id] : selected.value.filter((v) => v !== id)
  }

  const runCards = () => (
    <div class="grid gap-3">
      {rows.value.map((row) => (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-2 p-4">
            <div class="flex flex-wrap items-center justify-between gap-2">
              <span class="break-words font-medium">{row.name}</span>
              <span class="flex items-center gap-2">
                {row.stream.phase === 'done' && row.stream.finished ? (
                  <span class={`badge ${outcomeBadge(row.stream.finished.outcome)}`}>
                    {outcomeLabel(row.stream.finished.outcome)}
                  </span>
                ) : row.stream.phase === 'done' && row.stream.errored ? (
                  <span class="badge badge-soft badge-error">执行出错</span>
                ) : row.stream.phase === 'lost' ? (
                  <span class="badge badge-soft badge-warning">连接中断</span>
                ) : (
                  <span class="badge badge-soft">
                    {row.stream.phase === 'pending' ? '排队中' : '进行中'}
                  </span>
                )}
                <span class="text-xs opacity-70 tabular-nums">{elapsedOf(row)}</span>
              </span>
            </div>

            {row.stream.progress ? (
              <progress
                class="progress progress-primary w-full"
                value={row.stream.progress.done}
                max={Math.max(1, row.stream.progress.total)}
              />
            ) : null}

            {row.stream.errored ? (
              <div class="text-sm break-words text-error">{row.stream.errored.message}</div>
            ) : null}
            {row.stream.finished?.summary ? (
              <div class="text-sm break-words">{row.stream.finished.summary}</div>
            ) : null}
            {(row.stream.finished?.manual_steps ?? []).map((step) => (
              <div class="alert alert-soft alert-warning text-sm">
                <span class="break-words">{step.instruction}</span>
              </div>
            ))}

            {/* 日志区固定高度可滚动（§7.2）：一条链几十行，不封顶会把页面撑成一屏滚不完 */}
            <ul class="max-h-64 overflow-y-auto rounded-box bg-base-200 p-2 text-xs leading-5 font-mono">
              {row.stream.lines.length === 0 ? (
                <li class="opacity-60">
                  {row.stream.phase === 'lost'
                    ? '没连上事件流。这一趟可能仍在后台跑，结果到「历史」页看。'
                    : '等上游回话…'}
                </li>
              ) : (
                row.stream.lines.map((line) => (
                  <li class={`break-words ${levelClass(line.level)}`}>{line.message}</li>
                ))
              )}
            </ul>

            <div class="text-xs opacity-60">
              可以关掉本页，这一趟会在后端继续跑完（口令已经投出去的那些不会撤回）。
            </div>
          </div>
        </div>
      ))}
    </div>
  )

  /** 历史那一档：只列这一项能力，桌面表格 + 移动卡片（§5.3）。 */
  const historySection = () => {
    if (history.value.length === 0) {
      return (
        <p class="text-sm opacity-70">
          这一项还没有跑过。跑完一趟之后，每一趟的结论会同时出现在这里与「历史」页。
        </p>
      )
    }
    return (
      <div class="flex flex-col gap-3">
        <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
          <table class="table table-sm">
            <thead>
              <tr>
                <th>账号</th>
                <th>开始</th>
                <th>结束</th>
                <th>结论</th>
              </tr>
            </thead>
            <tbody>
              {history.value.map((r) => (
                <tr>
                  <td class="break-words">{r.accountId ?? '—'}</td>
                  <td class="text-sm">{fmtTime(r.startedAt)}</td>
                  <td class="text-sm opacity-70">{fmtTime(r.finishedAt)}</td>
                  <td class="text-sm break-words">{r.message ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div class="grid gap-3 md:hidden">
          {history.value.map((r) => (
            <div class="card bg-base-100 shadow-sm">
              <div class="card-body gap-2 p-4">
                <div class="flex items-center justify-between gap-2">
                  <span class="break-words font-medium">{r.accountId ?? '—'}</span>
                  <span class="badge badge-soft">{r.status}</span>
                </div>
                <div class="flex items-center justify-between text-sm">
                  <span class="opacity-70">开始</span>
                  <span>{fmtTime(r.startedAt)}</span>
                </div>
                <div class="flex items-center justify-between text-sm">
                  <span class="opacity-70">结束</span>
                  <span>{fmtTime(r.finishedAt)}</span>
                </div>
                <div class="text-sm break-words">{r.message ?? ''}</div>
              </div>
            </div>
          ))}
        </div>
      </div>
    )
  }

  const confirmDialog = () => (
    <ConfirmDialog
      id={CONFIRM_ID}
      title="确认跑这一趟"
      impact={impact.value}
      confirmLabel={koulingOnly.value ? '开始收集' : '确认投口令'}
      busy={submitting.value}
      onConfirm={() => void start()}
    />
  )

  return (
    <div class="flex flex-col gap-4">
      <p class="text-sm opacity-70">
        进直播间收口令、再把口令换成小红花。这条链只在<b class="font-medium">主播开播的那一段时间</b>
        里有用；投口令是<b class="font-medium">写操作</b>（口令全场通用，投过也不影响别人）。
      </p>

      {state.value === 'loading' ? (
        <div class="flex justify-center py-10">
          <span class="loading loading-spinner loading-md" />
        </div>
      ) : state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-soft btn-sm min-h-11 md:min-h-0" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : (
        <div class="flex flex-col gap-4">
          {probeSection()}
          {formSection()}
          {rows.value.length > 0 ? (
            <div class="flex flex-col gap-2">
              <h2 class="text-base font-medium">这一趟</h2>
              {runCards()}
              <div class="flex items-center justify-between gap-2">
                <p class="text-xs leading-5 opacity-60">
                  定时跑这一条：到点了后端<b class="font-medium">会</b>自己触发
                  （侧栏那一行的"定时调度未启用"只在它真没跑时出现）。
                  建任务在"定时任务"那一页，目标选"直播口令 → 小红花"；
                  这一项能力的默认是<b class="font-medium">错过不补</b> —— 那一场过了再打就是白连一次。
                </p>
                <button
                  type="button"
                  class="btn btn-soft btn-sm min-h-11 md:min-h-0"
                  disabled={active.value}
                  onClick={() => {
                    rows.value.forEach((r) => closeSource(r.runId))
                    rows.value = []
                    stopTicker()
                  }}
                >
                  收起这些卡片
                </button>
              </div>
            </div>
          ) : null}
          <div class="flex flex-col gap-2">
            <h2 class="text-base font-medium">最近的这一项</h2>
            {historySection()}
          </div>
        </div>
      )}
      {confirmDialog()}
    </div>
  )
}
