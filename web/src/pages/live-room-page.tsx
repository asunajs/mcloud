/**
 * 直播口令 → 小红花。选账号 → 填口令 → 跑 → 实时看日志与结果。
 *
 * 形态照 `tasks-page.tsx`（同一条 `POST /api/runs` + SSE），
 * 移动端优先（`docs/cli-webui-design.md`）：表单全宽、卡片流、44px 触控目标。
 *
 * # ⚠ 三条产品事实决定了这个页面的样子
 *
 * 1. **小红书那条口令没法自动获取，必须人工粘进来。** 那边没有可鉴权的接口，
 *    所以"手工口令"这一栏是**必需入口**，不是"可选补充"。页面把这句话写在
 *    输入框正下方，而不是塞进折叠说明里 —— 它是用户理解"为什么我还要手打"
 *    的唯一来源。
 * 2. **"没领"不是失败。** 这条链最常见的结局就是"今天没开播"，
 *    与签到里"今天已经签过了"同一档。所以结果区分**四态**，
 *    其中 `idle` 用中性色，**不染红**；且三种"没领"的原因**分开写**
 *    （今天没有直播 / 有 N 间在播但没有一间是那个主播 / 获取口令但不领取），
 *    因为它们的下一步完全不同。
 * 3. **「口令已失效（本场已结束）」与「无效口令」排查方向完全不同。**
 *    前者是"口令对、但直播结束了"，后者是"口令不认识（多半抄错了）"。
 *    界面上**必须分开两栏**，合并成一句"口令错误"就会把人引到错的一边。
 *
 * # 与 `tasks-page` 一致的交互契约
 *
 * 提交期间**只禁用提交按钮**，页面其余部分照常可操作；并且明确告诉用户
 * 可以关掉页面 —— 任务在后端继续跑完（已经领到的不会撤回）。
 */

import { onMounted, onUnmounted, ref } from 'vue'
import type { JSX } from 'vue-jsx-vapor'
import {
  LIVE_IDLE_REASON,
  LIVE_ROOM_TASK,
  cancelRun,
  listAccounts,
  listCapabilities,
  splitKouling,
  startRun,
  subscribeRun,
  toLiveRoomRunView,
  type AccountSummary,
  type LiveRoomIdleReason,
  type LiveRoomParams,
  type LiveRoomRunView,
  type RunEvent,
  type TaskMeta,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { LogPanel, type LogLine } from '../ui/log-panel'

type LoadState = 'loading' | 'error' | 'ready'

/**
 * 听弹幕时长的缺省。**与后端 `params_from_body` 里那个 60 同一个人填**：
 * 留空不送这一栏时后端用自己的默认值，界面把那个默认值**显式摆出来**，
 * 免得出现"界面显示 60、实际听了别的数"。
 */
const DEFAULT_LISTEN_SECONDS = 60

/** 前端校验的结论。`null` = 可以跑。 */
interface Validation {
  field: 'codes' | 'listen' | 'switches'
  message: string
}

export const LiveRoomPage = () => {
  const state = ref<LoadState>('loading')
  /** 整页加载失败。**留在页内**配「重试」—— 弹窗会挡住入口。 */
  const errorMessage = ref('')

  const accounts = ref<AccountSummary[]>([])
  const account = ref('')
  /** 能力声明（后端给的 title/description/params）。拿它证明后端真的注册了这条任务。 */
  const task = ref<TaskMeta | null>(null)
  /** 后端自述的默认听写时长 —— 模板里那个 60 是**兜底**，不是权威。 */
  const defaultListen = ref(DEFAULT_LISTEN_SECONDS)

  // ── 表单 ──────────────────────────────────────────────────────────
  const codesRaw = ref('')
  const listenSeconds = ref(DEFAULT_LISTEN_SECONDS)
  const koulingOnly = ref(false)
  const skipKouling = ref(false)

  // ── 运行 ──────────────────────────────────────────────────────────
  const running = ref(false)
  const cancelling = ref(false)
  const lines = ref<LogLine[]>([])
  const elapsedMs = ref(0)
  const currentRunId = ref<number | null>(null)
  /** 本次运行的终局结论。跑完才显示。 */
  const result = ref<LiveRoomRunView | null>(null)
  /** 提交一次运行期间为真 —— **只**用来禁用提交按钮。 */
  const submitting = ref(false)

  let stopStream: (() => void) | null = null
  let ticker: number | null = null
  let startedAt = 0

  // ─────────────────────────────────────────────────────────────────────
  // 取数
  // ─────────────────────────────────────────────────────────────────────

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [caps, accts] = await Promise.all([listCapabilities(), listAccounts()])
      task.value = caps.tasks.find((t) => t.name === LIVE_ROOM_TASK) ?? null
      accounts.value = accts
      // 只有一个账号时**预选**它：少一次点击，而且"就是那个号"没有歧义。
      // 多个账号时不猜 —— 猜错就是拿另一个号跑了一遍真任务（会真的去领）。
      if (accts.length === 1 && !account.value) {
        account.value = accts[0].id
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
    if (koulingOnly.value && skipKouling.value) {
      return {
        field: 'switches',
        // 后端原文：`koulingOnly 与 skipKouling 互斥：获取口令但不领取，就不该再关掉弹幕来源`
        message: '「获取口令但不领取」与「只用给定口令」互斥，两者不能同时打开。',
      }
    }
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
    currentRunId.value = null
    stopTicker()
    // EventSource 断线会自动重连，必须显式关掉
    stopStream?.()
    stopStream = null
  }

  /**
   * 请求中止当前运行。
   *
   * 中止是**协作式**的：后端只置标志，任务在安全点自己退出。
   * 所以这里不立刻改成"已停止"，而是显示"正在中止"，等真正的 finished 到达
   * 才收尾 —— 否则界面说停了、而任务还在领。
   */
  const cancel = async () => {
    const id = currentRunId.value
    if (id == null) return
    cancelling.value = true
    try {
      const resp = await cancelRun(id)
      if (!resp.cancelled) {
        // 任务刚好跑完 —— 正常竞态，不是错误
        push('warn', '任务已经结束，无需中止')
        cancelling.value = false
        return
      }
      push('warn', '已请求中止，等待任务在安全点退出…')
    } catch (e) {
      showError('中止失败', e instanceof Error ? e.message : '中止请求失败')
      cancelling.value = false
    }
  }

  const onEvent = (ev: RunEvent) => {
    switch (ev.type) {
      case 'started':
        push('info', `任务开始：${ev.task} / ${ev.account}`)
        break
      case 'log':
        push(ev.level === 'error' ? 'error' : ev.level === 'warn' ? 'warn' : 'info', ev.message)
        break
      case 'progress':
        push('info', `[${ev.step}/${ev.total}] ${ev.label}`)
        break
      case 'finished': {
        // ⚠ 终局事件的 outcome 与 run 记录里的 outcome 是**同一份**，
        // 所以这里直接用事件里的那一句；结构化明细（details）要等历史落盘，
        // 对刚刚结束的这一次「立即回读」多半读不到 —— 所以**结果区只依赖事件**。
        //
        // ⚠⚠ 整段包在 `try` 里，且 `finish()` 放在 `finally`：
        // 解析终局结论时**任何**异常（后端字段形状变了、某个字段缺失…）
        // 都不能让 `finish()` 被跳过 —— 那会让 `running` 永远为真，
        // 页面**永久停在"执行中"**、按钮不恢复、日志里没有终局行，
        // 用户只能刷新。这是我实测踩到过的失败模式（stub 少了一个字段就复现）。
        try {
          const view = toLiveRoomRunView(null, ev.outcome)
          result.value = view
          push(
            view.kind === 'failed' ? 'error' : 'info',
            `结束（${ev.duration_ms}ms）：${
              view.kind === 'idle' ? `没领 —— ${view.text}` : view.text
            }`,
          )
        } catch (e) {
          // 结论解析失败**不影响任务本身**：它已经在后端跑完了。
          // 如实说"这一趟的结果没解析出来"，别把它说成任务失败。
          push('warn', `这一趟的结果没能解析出来（去「历史」页看原始记录）：${String(e)}`)
        } finally {
          finish()
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
      showError(
        bad.field === 'switches' ? '参数冲突' : bad.field === 'codes' ? '缺少口令' : '参数不合法',
        bad.message,
      )
      return
    }
    if (!account.value.trim()) {
      showError(
        '请先选择一个账号',
        '这条链要一个已登录的云盘账号才能跑。请在上方「账号」里选一个，再点运行。',
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
      // 送**全部四个键**（连没改过的也送）：漏一个就用后端默认值，
      // 那会让"界面上显示的数字"与"实际跑的数"分家。
      //
      // `codes` 送**数组**而不是原始文本：数组是后端明确接受的一种形态，
      // 而且它已经按同一口径去重了 —— 用原始文本的话，界面上算出的"几条口令"
      // 与后端拆出来的可能不是一个数。
      codes: codeList(),
      listenSeconds: Number(listenSeconds.value) || 0,
      koulingOnly: koulingOnly.value,
      skipKouling: skipKouling.value,
    }

    try {
      // 后台执行：**立刻**拿到 run id，这样才能订阅事件并请求中止。
      // 同步执行的话请求还没返回，用户根本没有可中止的时机。
      //
      // ⚠ `params` 要展开成普通对象：`startRun` 收的是 `Record<string, unknown>`
      // （JSON 体的形状），直接传带名字的 interface 会缺索引签名（TS2345）。
      const resp = await startRun(LIVE_ROOM_TASK, account.value.trim(), { ...params }, true)
      currentRunId.value = resp.run_id
      // 先订阅再提示：订阅会把已发布的事件补进队列，
      // 因此"启动到订阅"之间的事件不会丢
      stopStream = subscribeRun(resp.run_id, { onEvent })
      push('info', `已提交，运行号 ${resp.run_id}`)
    } catch (e) {
      showError('无法启动「直播口令小红花」', e instanceof Error ? e.message : '请求失败')
      finish()
    } finally {
      // ⚠ 只放开提交按钮 —— `running` 才是"这一趟还在跑"。
      // 把整个表单锁住会让用户在等待期间连日志都看不全。
      submitting.value = false
    }
  }

  onMounted(() => void load())
  onUnmounted(() => {
    stopTicker()
    stopStream?.()
  })

  const elapsedText = () => `${(elapsedMs.value / 1000).toFixed(0)} 秒`

  // ─────────────────────────────────────────────────────────────────────
  // 渲染：表头与参数卡
  // ─────────────────────────────────────────────────────────────────────

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">直播小红花</h1>
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
        <select
          class="select w-full"
          value={account.value}
          onChange={(e) => {
            account.value = (e.target as HTMLSelectElement).value
          }}
        >
          <option value="" disabled>
            选择一个账号
          </option>
          {accounts.value.map((a) => (
            <option key={a.id} value={a.id}>
              {a.nickname || a.id}
              {a.pc_platform ? `（${a.pc_platform}）` : ''}
            </option>
          ))}
        </select>
      )}
      <p class="text-xs leading-5 opacity-60">
        一个账号跑一趟。多个账号请分别跑 —— 每个号的兑换状态是<b class="font-medium">各算各的</b>。
      </p>
    </fieldset>
  )

  /**
   * 手工口令 —— 本页最重要的一栏。
   *
   * ⚠ 那句"小红书那条必须人工粘"是**产品事实**，不是使用提示：
   * 小红书那边没有可鉴权的接口，自动收集永远拿不全两条口令。
   * 所以这一栏不是"可选补充"，而是**必需入口**。
   */
  const codesField = () => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">手工口令</legend>
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
          <b class="font-medium">小红书那条口令没法自动获取，必须人工粘进来。</b>
          直播间弹幕里的口令能自动收，小红书那条不能（那边无法鉴权）——
          所以这一栏是<b class="font-medium">必需入口</b>，不是可选补充。
        </span>
      </div>
      <p class="text-xs leading-5 opacity-60">
        分隔符是<b class="font-medium">空格 / 换行 / 逗号（含中文逗号、顿号）</b>，
        与后端同一口径；重复的会自动去重。已识别{' '}
        <b class="font-medium">{codeList().length}</b> 条。
      </p>
      <p class="text-xs leading-5 opacity-60">
        留空 = 只用弹幕自动收集（这时就<b class="font-medium">拿不到</b>小红书那一条）。
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
      <legend class="fieldset-legend">听弹幕时长（秒）</legend>
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
   * 两个开关。
   *
   * ⚠ 它们**互斥**。界面上不把它们做成单选按钮，因为后端是两个独立的布尔 ——
   * 前端改形状会让"你到底发了什么"在后端日志里对不上。所以还是两个 toggle，
   * 但一个打开时把另一个**立刻关掉并说明**，而不是等提交时骂人。
   */
  const switchesField = () => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">领不领</legend>
      <div class="flex flex-col gap-2">
        <label class="flex min-h-11 cursor-pointer items-center gap-2">
          <input
            type="checkbox"
            class="toggle toggle-primary"
            checked={koulingOnly.value}
            onChange={(e) => {
              koulingOnly.value = (e.target as HTMLInputElement).checked
              // 互斥：打开这个就把另一个关掉（后端会因此 400，不该让用户撞上去）
              if (koulingOnly.value) skipKouling.value = false
              revalidate()
            }}
          />
          <span class="text-sm">获取口令但不领取（只获取口令并打印，不拿去领小红花）</span>
        </label>
        <label class="flex min-h-11 cursor-pointer items-center gap-2">
          <input
            type="checkbox"
            class="toggle toggle-primary"
            checked={skipKouling.value}
            onChange={(e) => {
              skipKouling.value = (e.target as HTMLInputElement).checked
              if (skipKouling.value) koulingOnly.value = false
              revalidate()
            }}
          />
          <span class="text-sm">只用给定口令（不进直播间、不收弹幕，必须已填口令）</span>
        </label>
      </div>
      <p class="text-xs leading-5 opacity-60">
        两者<b class="font-medium">互斥</b>：都不领了，就不该再关掉弹幕来源。
        一个都不开 = 默认行为（用弹幕自动获取口令，并连同手里的口令一起去领）。
      </p>
      {validation.value?.field === 'switches' ? (
        <p class="text-xs leading-5 text-error">{validation.value.message}</p>
      ) : null}
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
              执行中，已用 {elapsedText()} · 可关闭页面，任务在后端继续
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
  const gainedCard = (view: LiveRoomRunView) => (
    <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
      <div class="stat">
        <div class="stat-title">本次进账</div>
        <div class="stat-value text-2xl">
          {view.gained == null ? '—' : `${view.gained > 0 ? '+' : ''}${view.gained}`}
          <span class="ml-1 text-base font-normal opacity-70">朵</span>
        </div>
        <div class="stat-desc">
          {/* 进账 = 终值 − 起始，**由后端给**。前端把两朵花加起来会漏掉
              「首次参与惊喜奖」那几朵 —— 它不在任何一条口令的结果里。 */}
          {view.balanceBefore != null && view.balanceAfter != null
            ? `余额 ${view.balanceBefore} → ${view.balanceAfter}（终值 − 起始，后端算的）`
            : '后端这一趟没给余额读数'}
        </div>
      </div>
      <div class="stat">
        <div class="stat-title">拿去领取的口令</div>
        <div class="stat-value text-2xl">{view.attempted}</div>
        <div class="stat-desc">
          成功 {view.succeeded.length} · 已兑换 {view.redeemed.length} · 已失效{' '}
          {view.expired.length} · 无效 {view.invalid.length} · 未分类 {view.other.length}
        </div>
      </div>
    </div>
  )

  /**
   * 一类口令一栏。
   *
   * ⚠ 这里**故意做成逐类一栏**而不是一个合并列表：把「已失效」和「无效」
   * 显示在一起、或者合并成一句"口令错误"，就会让用户去查**错的方向**
   * （抄错了口令 vs 直播已经结束了）。空的那几类**不显示** ——
   * 每类都挂一个"无"会把真正有内容的那一栏淹掉。
   *
   * ⚠⚠ 判空写成**三元**，而不是 `if (...) return null` 之后再把结果内联到
   * JSX 的子节点位置上 —— 后者会让 Vapor 崩：
   *
   * 在子节点位置写 `{helper(...)}`、而 helper 返回 `null` 时，Vapor 的
   * `setNodes` 会去读 `null.parentNode` ⇒
   * `TypeError: Cannot read properties of null (reading 'parentNode')`，
   * 于是**整块结果区渲染不出来**（控制台只有一条 pageerror，页面看着"没反应"）。
   *
   * 症状是实测出来的：`typecheck` 与 `build` **都不会报**这个错 ——
   * 它是运行期的渲染行为，只有真开浏览器跑一次才看得见。
   */
  const group = (label: string, hint: JSX.Element, badge: string, codes: { code: string }[]) =>
    codes.length === 0 ? null : (
      <div class="flex flex-col gap-1">
        <div class="flex items-center gap-2">
          <span class={`badge ${badge}`}>{label}</span>
          <span class="text-xs opacity-60">{codes.length} 条</span>
        </div>
        <p class="text-xs leading-5 opacity-60">{hint}</p>
        <ul class="flex flex-col gap-0.5">
          {codes.map((c) => (
            <li key={c.code} class="font-mono text-xs break-all opacity-80">
              {c.code}
            </li>
          ))}
        </ul>
      </div>
    )

  const codeBreakdown = (view: LiveRoomRunView) => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <h2 class="card-title text-base">口令结果</h2>
        {/* 成功 */}
        {view.succeeded.length > 0 ? (
          <div class="flex flex-col gap-1">
            <div class="flex items-center gap-2">
              <span class="badge badge-soft badge-success">成功</span>
              <span class="text-xs opacity-60">{view.succeeded.length} 条</span>
            </div>
            <p class="text-xs leading-5 opacity-60">这几条换到了花。</p>
            <ul class="flex flex-col gap-0.5">
              {view.succeeded.map((c) => (
                <li key={c.code} class="font-mono text-xs break-all opacity-80">
                  {c.code}
                  {c.flowerNum != null ? `（+${c.flowerNum}）` : ''}
                  {c.prizeName ? ` · ${c.prizeName}` : ''}
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        {/* 已兑换 —— 不是失败 */}
        {view.redeemed.length > 0 ? group(
          '已兑换',
          <span>
            这个号已经用这条口令领过了，不是出错。
          </span>,
          'badge-soft',
          view.redeemed,
        ) : null}
        {/*
          ⚠⚠ 这两栏**必须分开**，不能合并成"口令错误"：
          「已失效」= 口令是对的、但本场直播结束了（下一步：等下一场）；
          「无效」  = 这条口令上游不认识（下一步：查口令从哪抄来的）。
        */}
        {view.expired.length > 0 ? group(
          '口令已失效（本场已结束）',
          <span>
            口令本身是<b class="font-medium">对的</b>，但这一场直播已经结束了 ——
            上游不再收。下一步是<b class="font-medium">等下一场</b>，
            而不是去查口令抄得对不对。
          </span>,
          'badge-soft badge-warning',
          view.expired,
        ) : null}
        {view.invalid.length > 0 ? group(
          '无效口令',
          <span>
            这条口令上游不认识，多半是从别处抄来的、或者上一场的废值。
            口令只在<b class="font-medium">当场</b>的直播里有效。
          </span>,
          'badge-soft badge-error',
          view.invalid,
        ) : null}
        {view.other.length > 0 ? group(
          '未分类',
          <span>
            上游回了一个我们还没分类的业务码。它既不是"口令抄错了"也不是"直播结束了"
            —— 排查方向是上游改了什么。
          </span>,
          'badge-soft badge-warning',
          view.other,
        ) : null}
        {view.succeeded.length === 0 &&
        view.redeemed.length === 0 &&
        view.expired.length === 0 &&
        view.invalid.length === 0 &&
        view.other.length === 0 ? (
          <p class="text-sm opacity-70">
            这一趟没有拿去领取的口令 —— 所以下面是空的。原因见上面那条结论。
          </p>
        ) : null}
      </div>
    </div>
  )

  /**
   * "没领"的那三条路。
   *
   * ⚠ 措辞**逐条不同**，来自后端 `idle_summary()`：三种"没领"的下一步
   * 完全不一样（等 / 别等 / 本来就没打算领）。混成一句"没有可领的口令"
   * 会让人去查**收集**那一步，而那一步根本没被走到过。
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
              <span class="font-medium">运行号 {currentRunId.value ?? ''}</span>
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

  return (
    <div class="flex flex-col gap-4">
      {header()}
      {intro()}

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-spinner loading-lg text-primary" />
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
          {task.value == null ? notRegistered() : null}
          {result.value ? resultSection(result.value) : null}
          {lines.value.length > 0 ? <LogPanel lines={lines.value} /> : null}
        </>
      ) : null}
    </div>
  )
}
