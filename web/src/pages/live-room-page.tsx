import { accountName, refreshAccountNames } from '../account-names'
/**
 * 直播口令 → 小红花。选账号 → 填口令 → 跑 → 实时看日志与结果。
 *
 * 形态照 `tasks-page.tsx`（同一条 `POST /api/runs` + SSE），
 * 移动端优先（`docs/design/cli-webui-design.md`）：表单全宽、卡片流、44px 触控目标。
 *
 * # ⚠ 三条产品事实决定了这个页面的样子
 *
 * 1. **口令有两个自动来源：直播间弹幕 与 小红书。** 收集器**并发**跑这两个，
 *    所以"没收到口令"要看两个来源的结果，不是只看弹幕。
 *    ⚠ 小红书**没有"它自己那条口令"** —— 是**别人把口令分享到小红书**，
 *    我们从那儿读回来。写"小红书那条口令没法自动获取"是**双重误导**：
 *    既暗示小红书有一条专属口令，又暗示它拿不到（其实拿得到，只是低频）。
 *    手工那一栏是**第三条入口**（自动两条都没到手、或你想指定某条时用），
 *    所以它写在输入框正下方，而不是塞进折叠说明里。
 * 2. **"没领"不是失败。** 这条链最常见的结局就是"今天没开播"，
 *    与签到里"今天已经签过了"同一档。所以结果区分**四态**，
 *    其中 `idle` 用中性色，**不染红**；且三种"没领"的原因**分开写**
 *    （今天没有直播 / 有 N 间在播但没有一间是那个主播 / 门开了没收到口令），
 *    因为它们的下一步完全不同。
 * 3. **「口令已失效（本场已结束）」与「无效口令」排查方向完全不同。**
 *    前者是"口令对、但直播结束了"，后者是"口令不认识（多半抄错了）"。
 *    界面上**必须分开两栏**，合并成一句"口令错误"就会把人引到错的一边。
 *
 * # 与 `tasks-page` 一致的交互契约
 *
 * 提交期间**只禁用提交按钮**，页面其余部分照常可操作；并且明确告诉用户
 * 可以关掉页面 —— 任务在后端继续跑完（已经领到的不会撤回）。
 *
 * # 「自动直播定时」那张卡（用户口径 2026-10-08）
 *
 * 用户原话：「为直播增加一个自动设置定时的功能，设置为**下次直播开始时间的一个小时后**」。
 * 它落盘就是一条**普通定时**（`data/schedules.json` 里的 `auto-live-<owner>` ✓），
 * 所以它**同时**出现在「定时」页里、带着 `managedBy: "live-auto"` ✓。
 * ⚠ 这一页只**显示**后端算好的 `at`：时刻由系统算（用户手改会被下次重算覆盖 ✓），
 * 用户能改的只有**开关**与**账号多选** ✓ —— 前端一个字节都不推算，见 `autoCard()` ✓。
 */

import { onMounted, onUnmounted, ref } from '@asunajs/lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  LIVE_IDLE_REASON,
  LIVE_ROOM_TASK,
  cancelRun,
  getKoulingPool,
  getLiveAutoSchedule,
  getTaskConfig,
  listAccounts,
  listCapabilities,
  saveLiveAutoSchedule,
  splitKouling,
  startRun,
  subscribeRun,
  toLiveRoomRunView,
  type AccountSummary,
  type KoulingPoolView,
  type LiveAutoSchedule,
  type LiveRoomIdleReason,
  type LiveRoomParams,
  type LiveRoomRunView,
  type PoolCode,
  type RunEvent,
  type TaskMeta,
} from '../api'
// 结构化明细（领到的花 + 逐条口令落点）与历史页**共用一份** ——
// 从前它长在这一页里，于是过去的每次直播都只剩一行小结，明细打不开。
import { codeBreakdown, gainedCard } from '../ui/live-room-result'
import { AccountPicker } from '../ui/account-picker'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { LogPanel, pushLog, type LogLine } from '../ui/log-panel'
import { pushToast } from '../ui/toast'

type LoadState = 'loading' | 'error' | 'ready'

/**
 * 听弹幕时长的缺省。**与后端那个 180 同一个人填**（Rust 侧的定义只有一处：
 * `mcloud_tasks::live_room::DEFAULT_LISTEN_SECONDS`；用户口径 2026-10-08
 * 「默认180」，原先是 60）：
 * 留空不送这一栏时后端用自己的默认值，界面把那个默认值**显式摆出来**，
 * 免得出现"界面显示 180、实际听了别的数"。
 *
 * ⚠ 下面那句说明文字（「默认 X 秒」）读的就是这个常量 —— 它**不会**跟着
 * 后端 `ParamSpec.default` 走（`defaultListen` 从来没有从后端刷过），
 * 所以后端改了缺省值，这里必须同步改，否则页面上会写着一个过期的数 ✗。
 */
const DEFAULT_LISTEN_SECONDS = 180

/** 前端校验的结论。`null` = 可以跑。 */
interface Validation {
  field: 'codes' | 'listen'
  message: string
}

export const LiveRoomPage = () => {
  const state = ref<LoadState>('loading')
  /** 整页加载失败。**留在页内**配「重试」—— 弹窗会挡住入口。 */
  const errorMessage = ref('')

  const accounts = ref<AccountSummary[]>([])
  /**
   * 已勾选的账号 id。
   *
   * ⚠ 「全部账号」用**单独的开关** `allAccounts` 表达，而不是"把每一项都勾上"：
   * 后端的空数组语义是"**我名下的全部账号**"（与定时、任务页同一口径），
   * 以后加了账号也仍然成立。
   */
  const picked = ref<string[]>([])
  /** `true` = 全部账号 ⇒ 请求发 `accounts: []`。 */
  const allAccounts = ref(false)
  /** 能力声明（后端给的 title/description/params）。拿它证明后端真的注册了这条任务。 */
  const task = ref<TaskMeta | null>(null)
  /** 共享口令池（**全实例共享** ⇒ 这一页首先要回答的是"池里现在有什么"）。 */
  const pool = ref<KoulingPoolView | null>(null)
  /** 后端自述的默认听写时长 —— 模板里那个 60 是**兜底**，不是权威。 */
  const defaultListen = ref(DEFAULT_LISTEN_SECONDS)

  // ── 自动直播定时（`/api/live-room/auto-schedule`）────────────────────
  /**
   * 后端算好的那一份状态。`null` = 后端没有这条接口（老后端 / 读失败）
   * ⇒ **整张卡不画** —— 画一张只能报错的卡，用户拿它没办法（与池卡片同一条处置 ✓）。
   */
  const autoState = ref<LiveAutoSchedule | null>(null)
  /**
   * 卡上的**草稿**（开关 + 账号多选）。
   *
   * ⚠⚠ **时刻不在这里**：它是后端算出来的（`autoState.at` ✓），前端只负责显示 ✓。
   * 把 `at` 读进一个 ref 再自己推算下一次，就是"两处各算一遍迟早分叉"那条老路 ✗
   * （模块头那段产品事实里"16:31 还显示 15:00"就是它的后果）。
   */
  const autoEnabled = ref(false)
  /** 已勾选的账号 id；`autoAll` 为真时忽略（后端口径：空数组 = 全部已启用账号 ✓）。 */
  const autoPicked = ref<string[]>([])
  /** `true` = 全部账号 ⇒ 请求发 `accounts: []` ✓。 */
  const autoAll = ref(false)
  const autoSaving = ref(false)

  // ── 表单 ──────────────────────────────────────────────────────────
  const codesRaw = ref('')
  const listenSeconds = ref(DEFAULT_LISTEN_SECONDS)
  const skipKouling = ref(false)
  /** 配置页那份**只灌一次**（见 `load()` 里的注释：刷新不该冲掉用户刚填的东西 ✗）。 */
  let formSeeded = false

  // ── 运行 ──────────────────────────────────────────────────────────
  const running = ref(false)
  const cancelling = ref(false)
  const lines = ref<LogLine[]>([])
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
  /**
   * 结果卡片那一行「运行号」的文案。
   *
   * ⚠ 为什么要单独存一份、而不是直接读 `currentRunIds`：收尾时 `finish()` 会把
   * `currentRunIds` 清空，而结果卡片正是那一刻才挂上去的 —— 直接读的话那一行
   * 永远是空的（"运行号 " 后面什么都没有）。所以这一批的标识要在**提交成功后**
   * 就存下来，只由下一趟运行覆盖。
   */
  const lastRunLabel = ref('')
  /** 本次运行的终局结论。跑完才显示。 */
  const result = ref<LiveRoomRunView | null>(null)
  /** 提交一次运行期间为真 —— **只**用来禁用提交按钮。 */
  const submitting = ref(false)

  let stopStreams: (() => void)[] = []
  let ticker: number | null = null
  let startedAt = 0

  // ─────────────────────────────────────────────────────────────────────
  // 取数
  // ─────────────────────────────────────────────────────────────────────

  /** 单独刷池：跑完一次运行之后它必然变了，用户也会想立刻看到。 */
  const refreshPool = async () => {
    try {
      pool.value = await getKoulingPool()
    } catch {
      // 池读不到**不该**让整页报错：运行本身照样可用（池由后端维护）。
      pool.value = null
    }
  }

  /**
   * 把后端那一份状态**原样**铺到卡上（`GET` 与 `PUT` 的返回**同一个形状** ✓）。
   *
   * ⚠ `PUT` 之后必须走这里回填，**不能**拿本地那份草稿当结果：
   * 后端刚刚重算过（`at` 可能变了、也可能因为"算不出下一场"把 `enabled` 按成 `false` ✓），
   * 自己猜的话用户看到的就是"我点了没反应" ✗。
   */
  const applyAuto = (s: LiveAutoSchedule | null) => {
    autoState.value = s
    if (s == null) return
    autoEnabled.value = s.enabled
    autoPicked.value = [...s.accounts]
    // 空数组 = 全部已启用账号（不是"一个都没选"✗）⇒ 界面上的「全部账号」那一行亮起来 ✓
    autoAll.value = s.accounts.length === 0
  }

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [caps, accts, cfg, auto] = await Promise.all([
        listCapabilities(),
        listAccounts(),
        // ⚠ 配置页存的那份也拉回来（与任务页**同一口径** ✓）：以前这里不拉，
        // 于是"配置页把听弹幕时长改成 300、从直播页跑仍然发 180" ✗
        //（任务页每次运行都现拉一次配置，它注释里写着理由：「配置页改了、这次运行还用着
        // 旧参数」那种不一致没有任何迹象 ✗）。拉不到就退回默认值，**不**让整页失败 ✓。
        getTaskConfig(LIVE_ROOM_TASK).catch(() => null),
        // ⚠ 自动定时那一份**单独兜底**：这条接口是后加的，旧后端会 404 ——
        // 让整页跟着失败等于"少一张卡"变成"整页打不开" ✗。读不到就不画那张卡 ✓。
        getLiveAutoSchedule().catch(() => null),
      ])
      void refreshPool()
      task.value = caps.tasks.find((t) => t.name === LIVE_ROOM_TASK) ?? null
      accounts.value = accts
      applyAuto(auto)
      /**
       * ⚠ **只在首次**灌进表单：`load()` 也是「刷新」按钮的入口，每次刷新都灌一遍
       * 会把用户刚填的口令/时长冲掉 ✗（那是"看着像刷新、其实丢输入"）。
       * 之后要拿配置页的新值，重新进这一页即可 ✓。
       */
      if (!formSeeded && cfg) {
        formSeeded = true
        const p = (cfg.params ?? {}) as Record<string, unknown>
        if (typeof p.listenSeconds === 'number' && p.listenSeconds > 0) listenSeconds.value = p.listenSeconds
        if (typeof p.codes === 'string' && p.codes.trim() !== '') codesRaw.value = p.codes
        if (typeof p.skipKouling === 'boolean') skipKouling.value = p.skipKouling
      }
      // 只有一个账号时**预选**它：少一次点击，而且"就是那个号"没有歧义。
      // 多个账号时不猜 —— 猜错就是拿另一个号跑了一遍真任务（会真的去领）。
      if (accts.length === 1 && picked.value.length === 0 && !allAccounts.value) {
        picked.value = [accts[0].id]
      }
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  // ─────────────────────────────────────────────────────────────────────
  // 表单校验（**前端先挡**，别等后端 400）
  // ─────────────────────────────────────────────────────────────────────

  /** 拆好的口令。多处要用，所以是一处计算、多处引用。 */
  const codeList = () => splitKouling(codesRaw.value)

  /**
   * 三条后端也会判的规则，这里**先判一遍**。
   *
   * 为什么不省这一步：后端回的是 400 + 一句话，用户看到的是"跑都没跑起来"；
   * 而这三个错都是**填表时就能看出来**的。校验放前面，错误就能贴着字段说。
   *
   * ⚠ 判据与后端 `LiveRoomParams::check` **逐条对齐**，连顺序都一样 ——
   * 顺序不同会出现"前端说 A 错、后端说 B 错"，用户改完 A 还是跑不起来。
   */
  const validate = (): Validation | null => {
    if (skipKouling.value && codeList().length === 0) {
      return {
        field: 'codes',
        // 后端原文：`skipKouling 关掉了唯一的口令来源，必须同时给 codes`
        message:
          '打开「只用给定口令」就等于关掉了唯一的口令来源，上面那一栏至少要填一条口令。',
      }
    }
    if (!Number.isFinite(listenSeconds.value) || listenSeconds.value <= 0) {
      return {
        field: 'listen',
        // 后端原文：`listenSeconds 必须是正数`
        // ⚠ 这一条**不看** `skipKouling`：时长非正在任何时候都不是合法输入
        // （0 秒会让那次听弹幕立刻超时，表现成一句方向错的通知）。
        message: '听弹幕时长必须是正数（秒）。留空或填 0 都会被后端拒绝。',
      }
    }
    return null
  }

  const validation = ref<Validation | null>(null)

  /** 用户一改就重算高亮，但**不**主动骂人 —— 只在点运行时才弹出来。 */
  const revalidate = () => {
    if (validation.value !== null) validation.value = validate()
  }

  // ─────────────────────────────────────────────────────────────────────
  // 运行
  // ─────────────────────────────────────────────────────────────────────

  const push = (kind: LogLine['kind'], text: string) => {
    // ⚠ 走 `pushLog`：它按 `MAX_LOG_LINES` 截断（日志无上限追加会白占内存 +
    // 拖慢每次追加的复制 —— 2026-10-03 review §2.1；⚠ 上限本身 2026-10-05 从 500
    // 提到 2000：面板窗口化之后"看不见"那条理由不成立了，见 `ui/log-panel.tsx` ✓）
    lines.value = pushLog(lines.value, kind, text)
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
    stopTicker()
    // EventSource 断线会自动重连，必须显式关掉（每条 run 一条流）
    for (const stop of stopStreams) stop()
    stopStreams = []
  }

  /**
   * 请求中止本次点击涉及的全部运行。
   *
   * 中止是**协作式**的：后端只置标志，任务在安全点自己退出。
   * 所以这里不立刻改成"已停止"，而是显示"正在中止"，等真正的 finished 到达
   * 才收尾 —— 否则界面说停了、而任务还在领。
   *
   * ⚠ 多账号时**逐条**发：只中止第一个的话其余账号照跑。
   * 还没轮到的账号会带着已取消的令牌轮到自己，在安全点直接收尾（不会白领一遍）。
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
        say(ev.level === 'error' ? 'error' : ev.level === 'warn' ? 'warn' : 'info', ev.message)
        break
      case 'progress':
        say('info', `[${ev.step}/${ev.total}] ${ev.label}`)
        break
      case 'finished': {
        // ⚠ 终局事件的 outcome 与 run 记录里的 outcome 是**同一份**，
        // 所以这里直接用事件里的那一句；结构化明细（details）要等历史落盘，
        // 对刚刚结束的这一次「立即回读」多半读不到 —— 所以**结果区只依赖事件**。
        //
        // ⚠⚠ 整段包在 `try` 里，且收尾逻辑放在 `finally`：
        // 解析终局结论时**任何**异常（后端字段形状变了、某个字段缺失…）
        // 都不能让收尾被跳过 —— 那会让 `running` 永远为真，
        // 页面**永久停在"执行中"**、按钮不恢复、日志里没有终局行，
        // 用户只能刷新。这是我实测踩到过的失败模式（stub 少了一个字段就复现）。
        //
        // 多账号时结果卡片只有一份，谁最后结束就显示谁 —— 这是**有意的**：
        // 每一趟的明细都在「历史」页，卡片上是"刚跑完那趟"的快照。
        try {
          const view = toLiveRoomRunView(null, ev.outcome)
          result.value = view
          say(
            // `partial`（有成有败）染黄：既不报红，也不能跟普通成功一个色 ✗
            view.kind === 'failed' ? 'error' : ev.outcome.status === 'partial' ? 'warn' : 'info',
            `结束（${ev.duration_ms}ms）：${
              view.kind === 'idle' ? `没领 —— ${view.text}` : view.text
            }`,
          )
        } catch (e) {
          // 结论解析失败**不影响任务本身**：它已经在后端跑完了。
          // 如实说"这一趟的结果没解析出来"，别把它说成任务失败。
          say('warn', `这一趟的结果没能解析出来（去「历史」页看原始记录）：${String(e)}`)
        } finally {
          finishedRuns.value += 1
          // 多账号时**全部**结束才算这一批跑完；否则继续等队列里的下一个
          // （后端是串行的，所以此刻剩下的账号其实是"排队中"）。
          if (finishedRuns.value >= currentRunIds.value.length) {
            finish()
          } else {
            say('info', `还剩 ${currentRunIds.value.length - finishedRuns.value} 个账号排队中`)
          }
        }
        break
      }
      default:
        // 后端新增事件类型时，旧前端不该整页报错 —— 忽略即可
        break
    }
  }

  const run = async () => {
    const bad = validate()
    validation.value = bad
    if (bad) {
      // 没选账号与参数不合法都是**当前动作的直接答案**，走弹窗告知
      // （页内那行提示在表单下方，卡片拉长后用户根本看不到）。
      showError(bad.field === 'codes' ? '缺少口令' : '参数不合法', bad.message)
      return
    }
    // 「全部账号」在协议上是 `accounts: []`（与定时、任务页同一口径）
    const ids = allAccounts.value ? [] : picked.value
    if (!allAccounts.value && ids.length === 0) {
      // 没选账号就点运行 —— 这是**当前动作的直接答案**，弹窗告知。
      // 页内那行提示在表单下方，卡片拉长后用户根本看不到。
      showError(
        '请先选择账号',
        '这条链要一个已登录的云盘账号才能跑。请在上方「账号」里勾选，或勾上「全部账号」。',
      )
      return
    }
    lines.value = []
    result.value = null
    submitting.value = true
    running.value = true
    startedAt = Date.now()
    elapsedMs.value = 0
    ticker = window.setInterval(() => {
      elapsedMs.value = Date.now() - startedAt
    }, 1000)

    const params: LiveRoomParams = {
      // 送**全部三个键**（连没改过的也送）：漏一个就用后端默认值，
      // 那会让"界面上显示的数字"与"实际跑的数"分家。
      //
      // `codes` 送**数组**而不是原始文本：数组是后端明确接受的一种形态，
      // 而且它已经按同一口径去重了 —— 用原始文本的话，界面上算出的"几条口令"
      // 与后端拆出来的可能不是一个数。
      codes: codeList(),
      listenSeconds: Number(listenSeconds.value) || 0,
      skipKouling: skipKouling.value,
    }

    try {
      // 后台执行：**立刻**拿到 run id（多账号时是一组），这样才能订阅事件并请求中止。
      // 同步执行的话请求还没返回，用户根本没有可中止的时机。
      //
      // ⚠ `params` 要展开成普通对象：`startRun` 收的是 `Record<string, unknown>`
      // （JSON 体的形状），直接传带名字的 interface 会缺索引签名（TS2345）。
      const resp = await startRun(LIVE_ROOM_TASK, ids, { ...params }, true)
      // ⚠ 必须容忍**老后端**（只有 `run_id`/`account`，没有这组新字段）：
      // 门禁夹具就是老形状，升级顺序上"前端先上、后端后上"也真实存在。
      const runIds = resp.run_ids?.length ? resp.run_ids : [resp.run_id]
      const labels = resp.accounts?.length ? resp.accounts : [resp.account]
      currentRunIds.value = runIds
      finishedRuns.value = 0
      // 多账号时结果卡片只留一句"共 N 个运行号"（单个则照旧显示那一个 id）
      lastRunLabel.value =
        runIds.length > 1 ? `共 ${runIds.length} 个运行号` : `运行号 ${runIds[0] ?? ''}`
      // 先订阅再提示：订阅会把已发布的事件补进队列，
      // 因此"启动到订阅"之间的事件不会丢。**每条 run 各订一条流**。
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
      showError('无法启动「直播口令小红花」', e instanceof Error ? e.message : '请求失败')
      finish()
    } finally {
      // ⚠ 只放开提交按钮 —— `running` 才是"这一趟还在跑"。
      // 把整个表单锁住会让用户在等待期间连日志都看不全。
      submitting.value = false
    }
  }

  // ─────────────────────────────────────────────────────────────────────
  // 自动直播定时：保存
  // ─────────────────────────────────────────────────────────────────────

  /**
   * 勾/取消「全部账号」。
   *
   * ⚠ 取消时**把当前已启用的账号全勾上**，而不是留一个空名单 ✗ ——
   * 空名单与"全部"在后端**是同一个值**（`accounts: []` ✓），取消之后名单还是空的，
   * 用户会以为没生效 ✗（与定时页那条同一处置 ✓）。
   */
  const toggleAutoAll = () => {
    if (autoAll.value) {
      autoAll.value = false
      autoPicked.value = accounts.value.map((a) => a.id)
      return
    }
    autoAll.value = true
  }

  const toggleAutoAccount = (id: string) => {
    autoPicked.value = autoPicked.value.includes(id)
      ? autoPicked.value.filter((x) => x !== id)
      : [...autoPicked.value, id]
  }

  /**
   * 保存开关 + 账号多选。后端**立刻重算**，返回的就是新状态 ⇒ 回填它 ✓。
   *
   * ⚠ 只发这两格（**时刻不在这里** ✗）：`at` 是算出来的，用户改了也会被下次重算覆盖，
   * 所以这一页压根不给它输入框 —— 那正是"写了也白写"的那种控件 ✗。
   * ⚠ `accounts: []` 是**有意义的**（= 全部已启用账号 ✓），所以不能用
   * `autoPicked.value.length ? … : undefined` 那种"空就不发"的写法：
   * 那会让"全部账号"这个选择**发不出去** ✗。
   */
  const saveAuto = async () => {
    autoSaving.value = true
    try {
      applyAuto(
        await saveLiveAutoSchedule({
          enabled: autoEnabled.value,
          accounts: autoAll.value ? [] : autoPicked.value,
        }),
      )
      pushToast('success', '已保存自动直播定时')
    } catch (e) {
      // 账号不是自己的那种 400：后端原话要**原样**显示（它写着哪个 id、为什么 ✗）
      showError('保存「自动直播定时」失败', e instanceof Error ? e.message : '保存失败')
    } finally {
      autoSaving.value = false
    }
  }

  onMounted(() => {
  setPageRefresh({ run: () => void load() })
    // 先把"id → 名字"填上，否则运行记录里显示的是一串哈希
    void refreshAccountNames()
    void load()
  })
  onUnmounted(() => {
  setPageRefresh(null)
    stopTicker()
    for (const stop of stopStreams) stop()
  })

  const elapsedText = () => `${(elapsedMs.value / 1000).toFixed(0)} 秒`

  // ─────────────────────────────────────────────────────────────────────
  // 渲染：表头与参数卡
  // ─────────────────────────────────────────────────────────────────────

  /**
   * 这一页的来龙去脉。
   *
   * 写在最上面而不是折叠区：用户要知道"这条链什么时候有用"
   * （只有主播开播那一段时间），否则他会在大半夜点很多次，
   * 每次都得到一句"今天没有直播"。
   */
  const intro = () => (
    <p class="text-sm leading-6 opacity-70">
      进直播间收口令、再把口令换成小红花。这条链只在
      <b class="font-medium">主播开播的那一段时间</b>里有用。
    </p>
  )

  /**
   * 下一场直播 —— **放在最上面**（用户口径 2026-10-02：
   * 「下一场直播时间应该移动到上面，显眼」）。
   *
   * ⚠ 它原先埋在「共享口令池」卡片的最底下 ✗。而这一页真正要先看的就是"什么时候开播"：
   * 口令**只在开播那段时间有效**（见模块头的产品事实 2），所以时间是"现在该不该点运行"
   * 的唯一依据 —— 正是上面那句"这条链只在主播开播的那一段时间里有用" ✓。
   * ⇒ 移到顶部并放大；全页**只有这一处**（池卡片里那份已删 ✓，两处早晚不一致 ✗）。
   */
  const nextSession = () => {
    const p = pool.value
    // 池子还没读到（或读失败）⇒ 不占位：下面那张「共享口令池」卡片会给出错误与重试 ✓
    if (p == null) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-1">
          <p class="text-xs font-semibold opacity-60">下一场直播</p>
          {p.nextStart ? (
            <>
              <p class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                <span class="font-mono text-xl font-semibold">{p.nextStart}</span>
                {/* ⚠ 主播名字（用户口径 2026-10-02）✗ 光有时间和场次名仍分不出
                    是不是自己那个直播间 —— 后端给的就是**目标主播**的昵称 ✓，
                    与时间/标题同一次选择 ✓。 */}
                {p.nextAnchor ? <span class="text-base font-medium">{p.nextAnchor}</span> : null}
              </p>
              {/* ⚠ 标题要跟着时间一起显示（用户口径 2026-10-02）：
                  只写"10-02 15:00"根本看不出是不是自己要看的那个直播间 ——
                  后端按**目标主播**筛、取最早一场，标题就是那一场的场次名 ✓。 */}
              {p.nextTitle ? <p class="break-words text-sm opacity-80">{p.nextTitle}</p> : null}
              <p class="text-xs opacity-50">
                目标主播最早那一场；时间是上游给的本地时间，原样显示。
              </p>
            </>
          ) : (
            <p class="text-sm opacity-60">
              {p.scheduleAtMs == null
                ? '还没有探测过场次 —— 跑一次直播任务就会带回来。'
                : '上游这次没有给下一场的时间。'}
            </p>
          )}
        </div>
      </div>
    )
  }

  const accountField = () => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">账号</legend>
      {accounts.value.length === 0 ? (
        // 没账号时**给出下一步**，而不是一个会失败的输入框：
        // 手打手机号也没用 —— 任务要的是本地已登录的那份凭据。
        <div class="alert alert-soft alert-warning text-sm" role="alert">
          <span>还没有已登录的账号。请先到「账号」页登录一个云盘账号，再回来跑。</span>
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
        </div>
      )}
    </fieldset>
  )

  /**
   * 手工口令 —— 第三条入口（可留空）。
   *
   * ⚠ 别写成"小红书那条口令必须人工粘"：口令的自动来源是**弹幕**与**小红书**
   * 两个（收集器并发跑，小红书那条链不需要鉴权、只是低频）。
   * 小红书**没有它自己那条口令** —— 是**别人把口令分享到那儿**。
   * 那样写会让人以为小红书有一条专属口令、而且自动拿不到，两件都不是事实。
   */
  const codesField = () => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">手工口令（可留空）</legend>
      <textarea
        class="textarea w-full"
        rows={3}
        autocomplete="off"
        spellcheck={false}
        aria-label="手工口令，多条用空格、换行或逗号分隔"
        placeholder="把口令粘进来，多条用空格、换行或逗号分隔"
        value={codesRaw.value}
        onInput={(e) => {
          codesRaw.value = (e.target as HTMLTextAreaElement).value
          revalidate()
        }}
      />
      <div class="alert alert-soft alert-info text-sm" role="alert">
        <span class="break-words">
          口令会自动从<b class="font-medium">直播间弹幕</b>和<b class="font-medium">小红书</b>两个来源收
          （<b class="font-medium">小红书那边不是它自己有口令</b>，是别人把口令分享到那儿，我们读回来）。
          这一栏是<b class="font-medium">补</b>：自动那两条都没到手、或你想指定某条时用，留空也行。
        </span>
      </div>
      <p class="text-xs leading-5 opacity-60">
        分隔符是<b class="font-medium">空格 / 换行 / 逗号（含中文逗号、顿号）</b>，
        与后端同一口径；重复的会自动去重。已识别{' '}
        <b class="font-medium">{codeList().length}</b> 条。
      </p>
      <p class="text-xs leading-5 opacity-60">
        留空 = 让弹幕与小红书两个来源自动收集。
        ⚠ 口令只在<b class="font-medium">本次直播</b>有效：上一场的口令是废值，
        抄来的会回「口令无效」。
      </p>
      {validation.value?.field === 'codes' ? (
        <p class="text-xs leading-5 text-error">{validation.value.message}</p>
      ) : null}
    </fieldset>
  )

  const listenField = () => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">最多听多久（秒）</legend>
      <input
        class="input w-full md:w-40"
        type="number"
        min={1}
        step={1}
        inputmode="numeric"
        aria-label="听弹幕时长（秒）"
        value={String(listenSeconds.value)}
        onInput={(e) => {
          listenSeconds.value = Number((e.target as HTMLInputElement).value)
          revalidate()
        }}
      />
      <p class="text-xs leading-5 opacity-60">
        默认 {defaultListen.value} 秒。必须<b class="font-medium">大于 0</b>
        —— 填 0 会让听弹幕立刻超时，然后报一句方向完全错的通知。
        只在"口令留空 = 自动收集"时才真正生效。
      </p>
      {validation.value?.field === 'listen' ? (
        <p class="text-xs leading-5 text-error">{validation.value.message}</p>
      ) : null}
    </fieldset>
  )

  /**
   * 唯一的开关：**只听我手工列的这几条**。
   *
   * ⚠ 这里原本还有第二个开关「获取口令但不领取」（用户 2026-10-01 判为**伪需求**，
   * 已删）。它的问题是双重的：① 它与"边听边领"直接矛盾 —— 抓取的价值就在当场
   * 把口令验成有效；② 它的文案是从 CLI 搬来的（"打印"在浏览器里没有对应物），
   * 而且一句话把同一件事说了三遍。**一个需要括号自我解释的开关，本身就该被砍。**
   */
  const switchesField = () => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">口令从哪来</legend>
      <div class="flex flex-col gap-2">
        <label class="flex min-h-11 cursor-pointer items-center gap-2">
          <input
            type="checkbox"
            class="toggle toggle-primary"
            checked={skipKouling.value}
            onChange={(e) => {
              skipKouling.value = (e.target as HTMLInputElement).checked
              revalidate()
            }}
          />
          <span class="text-sm">只用手工列的这几条，不听弹幕</span>
        </label>
      </div>
      <p class="text-xs leading-5 opacity-60">
        默认（不开）＝ 先用共享口令池里的口令；池里有效的不够两条时，边听弹幕边领，
        当场把口令验出有效/无效。开了就是"我自己有口令，别去听"。
      </p>
    </fieldset>
  )

  const form = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <h2 class="card-title text-base">运行参数</h2>
        {accountField()}
        {codesField()}
        {listenField()}
        {switchesField()}


        {/*
          提交区。
          ⚠ 提交期间**只**禁用这一颗按钮（`submitting`），`running` 只影响文案 ——
          页面其余部分保持可操作，这是与 `tasks-page` 一致的那条契约。
        */}
        <div class="modal-action items-center justify-between gap-2">
          {running.value ? (
            <span class="text-xs opacity-70">
              执行中，已用 {elapsedText()}
              {currentRunIds.value.length > 1
                ? ` · 已完成 ${finishedRuns.value}/${currentRunIds.value.length} 个账号（按顺序串行）`
                : ''}
              {' '}· 可关闭页面，任务在后端继续
            </span>
          ) : (
            <span class="text-xs opacity-70">
              点下去会真的去领小红花。
            </span>
          )}
          <span class="flex items-center gap-2">
            {running.value ? (
              <button
                type="button"
                class="btn btn-error btn-sm min-h-11 md:min-h-0"
                disabled={cancelling.value}
                onClick={() => void cancel()}
              >
                {cancelling.value ? '正在中止…' : '中止'}
              </button>
            ) : null}
            <button
              type="button"
              class="btn btn-primary min-h-11 md:min-h-0"
              disabled={submitting.value || accounts.value.length === 0}
              onClick={() => void run()}
            >
              {submitting.value ? '提交中…' : running.value ? '再跑一趟' : '开始领取'}
            </button>
          </span>
        </div>
      </div>
    </div>
  )

  /**
   * 「自动直播定时」卡（用户口径 2026-10-08：「为直播增加一个自动设置定时的功能，
   * 设置为**下次直播开始时间的一个小时后**」✓）。
   *
   * # 卡上那三句话一句都不能省
   *
   * 1. **时刻 = 下一场直播开始 + 1 小时** —— 用户原话里唯一被指定的规则 ✓；
   * 2. **到点跑直播任务（检测口令 + 领取）** —— 开了它到底会干什么 ✓；
   * 3. ⚠ **时刻由系统算、你改它会被下次重算覆盖；开关和账号是你说了算** ——
   *    这条最要紧：那条定时**同时**出现在「定时」页里，用户在那儿改时刻**改得动**
   *    （它就是一条普通定时 ✓），而下次直播任务跑完重算会把它覆盖回去 ⇒
   *    不写清就是坑 ✗（口径见 `mcloud-core::Schedule::managed_by` 的注释 ✓）。
   *
   * # ⚠ `at == null` 必须写"已停用"，不能留空
   *
   * 留空的话用户分不清"还没算出来"与"算出来是空的"✗ —— 而这两种的下一步完全不同
   * （前者等下一场、后者要去看 note 里那句人话 ✓）。
   * `note` 是后端给的人话（没有下一场 / 用户关了 / id 被手建占用 ✓），**原样**显示 ✓。
   *
   * # ⚠ 这一页**不算时间**
   *
   * `at` 直接取后端给的串 ✓。前端不缓存它、也不自己推算"下一次"—— 上游的日程
   * 每次直播任务跑完都会变，自己推一份出来迟早与后端分叉 ✗
   * （上一版分叉的后果用户见过：「16:31 还显示 15:00」✗）。
   */
  const autoCard = () => {
    const s = autoState.value
    // 后端没有这条接口（老后端 / 读失败）⇒ 整张卡不画（见 `autoState` 那段说明 ✓）
    if (s == null) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h2 class="card-title text-base">自动直播定时</h2>

          <label class="flex min-h-11 cursor-pointer items-center gap-2">
            <input
              id="auto-live-enabled"
              type="checkbox"
              class="toggle toggle-primary"
              aria-label="自动直播定时开关"
              checked={autoEnabled.value}
              onChange={(e) => {
                autoEnabled.value = (e.target as HTMLInputElement).checked
              }}
            />
            <span class="text-sm">到点自动跑一趟直播任务</span>
          </label>

          <fieldset class="fieldset">
            <legend class="fieldset-legend">作用在哪些账号</legend>
            {accounts.value.length === 0 ? (
              // 没账号时**给出下一步**，而不是一个会失败的输入框（与上面「账号」那一栏同处置）
              <div class="alert alert-soft alert-warning text-sm" role="alert">
                <span>还没有已登录的账号。请先到「账号」页登录一个云盘账号。</span>
              </div>
            ) : (
              /*
                ⚠ **必须给 id**：这一页上有**两个**账号选择器（上面那个是"跑这一趟用哪些"），
                而 `Select` 靠 id 认人（面板挂在 body 上、展开状态是模块级的 ✓）——
                两个同 id 会互相认错，展开/收起与回调都串台 ✗（见 `ui/account-picker.tsx` ✓）。
              */
              <AccountPicker
                id="auto-live-accounts"
                accounts={accounts.value}
                all={autoAll.value}
                picked={autoPicked.value}
                onToggleAll={toggleAutoAll}
                onToggle={toggleAutoAccount}
              />
            )}
          </fieldset>

          <div class="flex flex-col gap-1">
            <p class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <span class="text-xs font-semibold opacity-60">当前算出的时刻</span>
              {s.at ? (
                <span class="font-mono text-xl font-semibold">{s.at}</span>
              ) : (
                // ⚠ 不留空（见上面那段说明）
                <span class="text-sm font-medium">已停用</span>
              )}
            </p>
            {s.note ? (
              <p class="break-words text-xs leading-5 opacity-70">{s.note}</p>
            ) : s.at == null ? (
              <p class="break-words text-xs leading-5 opacity-70">现在没有下一场，已停用。</p>
            ) : null}
          </div>

          <p class="text-xs leading-5 opacity-60">
            时刻 = <b class="font-medium">下一场直播开始 + 1 小时</b>；到点会拿上面这些账号跑一趟
            <b class="font-medium">直播任务（检测口令 + 领取）</b>。
            ⚠ <b class="font-medium">时刻由系统算</b>：你在「定时」页改它，会被下次重算覆盖；
            <b class="font-medium">开关和账号是你说了算</b>，改完点保存即可。
            算不出下一场时自动停用，条目留着，下次算出来再自动启用。
          </p>

          <div class="modal-action items-center justify-between gap-2">
            <span class="text-xs opacity-70">
              这一条在「定时」页里也看得到，标着「系统维护」。
            </span>
            <button
              id="auto-live-save"
              type="button"
              class="btn btn-primary min-h-11 md:min-h-0"
              disabled={autoSaving.value || accounts.value.length === 0}
              onClick={() => void saveAuto()}
            >
              {autoSaving.value ? '保存中…' : '保存'}
            </button>
          </div>
        </div>
      </div>
    )
  }

  // ─────────────────────────────────────────────────────────────────────
  // 渲染：结果
  // ─────────────────────────────────────────────────────────────────────

  /**
   * 结论卡片。
   *
   * ⚠⚠ 颜色**查表**，不拼接，而且 `idle` 刻意是**中性**：
   * 直播这条链最常见的结局就是"今天没开播"，把它染红/染黄等于每天报一次假警 ——
   * 与签到里"今天已经签过了"同一档（`ui/history.ts` 里 `skipped` 用裸
   * `badge-soft` 也是这个理由）。
   */
  const RESULT_TONE: Record<LiveRoomRunView['kind'], { card: string; badge: string; label: string }> =
    {
      idle: { card: 'alert-soft', badge: 'badge-soft', label: '没领' },
      success: { card: 'alert-soft alert-success', badge: 'badge-soft badge-success', label: '成功' },
      partial: { card: 'alert-soft alert-warning', badge: 'badge-soft badge-warning', label: '部分成功' },
      failed: { card: 'alert-soft alert-error', badge: 'badge-soft badge-error', label: '失败' },
    }

  /**
   * 一朵小花的数字。
   *
   * ⚠ 读不到时显示 `—` 而**不是 0**：0 是一句断言（"这次一朵都没进"），
   * 而读不到只是读不到。两者在界面上长得一样的话，用户会把"没读到"当成"没进账"。
   */
  const idleNote = (reason: LiveRoomIdleReason) => (
    <p class="text-sm leading-6 break-words">{LIVE_IDLE_REASON[reason]}</p>
  )

  const resultSection = (view: LiveRoomRunView) => {
    const tone = RESULT_TONE[view.kind]
    return (
      <div class="flex flex-col gap-3">
        <div class={`alert ${tone.card} text-sm`} role="status">
          <div class="flex flex-col gap-1">
            <span class="flex items-center gap-2">
              <span class={`badge ${tone.badge}`}>{tone.label}</span>
              <span class="font-medium">{lastRunLabel.value}</span>
            </span>
            <span class="break-words">{view.text}</span>
            {view.idle != null ? idleNote(view.idle) : null}
          </div>
        </div>

        {gainedCard(view)}
        {codeBreakdown(view)}

        {view.kind === 'idle' && view.idle === 'no-live' ? (
          <div class="alert alert-soft text-sm">
            <span>
              等下一场即可。想让它自动跑，可以在「定时」页建一条定时 ——
              目标选这条「直播口令小红花」，口令填在自定义参数里。
            </span>
          </div>
        ) : null}
      </div>
    )
  }

  // ─────────────────────────────────────────────────────────────────────
  // 渲染：页面
  // ─────────────────────────────────────────────────────────────────────

  /**
   * 后端没注册这条任务时的整页空态。
   *
   * 这不是"理论上不会发生"：任务注册在 `crates/mcloud-tasks` 里，
   * 而这份前端产物是可以被旧后端服务的（反过来也一样）。
   * 直接放一个会 400 的运行按钮，用户只会看到"未知任务"四个字。
   */
  const notRegistered = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <EmptyState
          title="后端没有注册「直播口令小红花」这条任务。"
          hint="它由后端任务注册表提供。检查后端版本是否包含直播口令一族，或稍后重试。"
          action={
            <button type="button" class="btn btn-soft" onClick={() => void load()}>
              重试
            </button>
          }
        />
      </div>
    </div>
  )

  /**
   * 共享口令池。
   *
   * 用户口径（2026-10-01）：口令是**全实例共享**的资源 —— 哪个账号抓到的、
   * 或者谁手工粘进来并验证通过的，验证通过后**所有账号都能用**；
   * 有效够 2 条，全场都不必再抓。
   *
   * ⚠ 所以这一页的主语变了：从前是"你要填什么口令"，现在是"**池里现在有什么**"。
   * 手工那一栏仍在（它是小红书那条的唯一入口），但它的定位从"必填"变成
   * "补一条进去，验过就归大家用"。
   */
  const poolCard = () => {
    const p = pool.value
    if (p == null) return null
    const stateBadge = (c: PoolCode) =>
      c.state === 'valid' ? 'badge-success' : c.state === 'invalid' ? 'badge-error' : 'badge-ghost'
    const stateText = (c: PoolCode) =>
      c.state === 'valid' ? '有效' : c.state === 'invalid' ? '无效' : '待验证'
    const sourceText = (c: PoolCode) =>
      c.source === 'danmaku' ? '弹幕' : c.source === 'xiaohongshu' ? '小红书' : '手工'
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">
              共享口令池
              <span class={`badge badge-sm ${p.stopScraping ? 'badge-success' : 'badge-ghost'}`}>
                {p.validCount} 条有效
              </span>
            </h2>
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              onClick={() => void refreshPool()}
            >
              刷新
            </button>
          </div>
          <p class="break-words text-sm opacity-70">
            {p.stopScraping
              ? '已经够 2 条有效口令 —— 全场都不需要再抓口令，直接用池里的。'
              : '有效口令不足 2 条：这次运行会边听弹幕边领，当场把口令验成有效/无效。抓到的或手工粘的，验证通过后所有账号都能用。'}
          </p>
          {p.codes.length === 0 ? (
            <p class="text-sm opacity-60">这个周期还没有任何口令。</p>
          ) : (
            <ul class="flex flex-col gap-2">
              {p.codes.map((c) => (
                <li key={c.value} class="flex min-w-0 flex-wrap items-center gap-2 text-sm">
                  <span class={`badge badge-sm ${stateBadge(c)}`}>{stateText(c)}</span>
                  <span class="min-w-0 break-all font-mono text-xs">{c.value}</span>
                  <span class="text-xs opacity-50">{sourceText(c)}</span>
                  {c.reason ? <span class="text-xs opacity-50">{c.reason}</span> : null}
                </li>
              ))}
            </ul>
          )}
          {/* ⚠ 「下一场」**不在这里**：已按用户口径（2026-10-02）移到页面最上面 ✓
              （见 `nextSession()`）—— 这一页真正要先看的就是"什么时候开播"，
              埋在池卡片最底下 ✗ 等于没有。全页只有那一处 ✓。 */}
          {p.sessions.length === 0 ? null : (
            <ul class="flex flex-col gap-1">
              {p.sessions.map((s) => (
                <li key={String(s.startedAtMs)} class="text-xs opacity-70">
                  {new Date(s.startedAtMs).toLocaleString('zh-CN')}
                  {s.endedAtMs == null
                    ? ' 起（还在播）'
                    : ' – ' + new Date(s.endedAtMs).toLocaleString('zh-CN')}
                </li>
              ))}
            </ul>
          )}
          <p class="text-xs opacity-50">
            周期自 {new Date(p.cycleAtMs).toLocaleString('zh-CN')} 起；5 小时、或判定门关上
            10 分钟后自动翻篇。
          </p>
        </div>
      </div>
    )
  }

  return (
    <div class="flex flex-col gap-4">
      {/* ⚠ 顺序是**刻意的**（用户口径 2026-10-02）：先给"什么时候开播" ✓，
          再给"这页是干什么的" —— 反过来会让用户先读一段说明才看到时间 ✗。 */}
      {nextSession()}
      {intro()}

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
          {autoCard()}
          {poolCard()}
          {task.value == null ? notRegistered() : null}
          {result.value ? resultSection(result.value) : null}
          {/* ⚠ thunk：传数组会让这个槽订阅日志 ⇒ 每条事件整块面板重建（含滚动位置丢失） */}
          <LogPanel get={() => lines.value} />
        </>
      ) : null}
    </div>
  )
}
