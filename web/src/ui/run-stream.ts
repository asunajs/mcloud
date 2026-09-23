/**
 * 事件流（SSE）的**纯**解析：一行 `data:` 文本 → 可判别的联合。
 *
 * # 为什么单独一个文件（§9 约定 2：纯函数放 `src/ui/`）
 *
 * 页面要管的是"订阅几条流、什么时候关"，而这里只管"这一条说了什么"。
 * 分开之后，本文件不碰 DOM、不碰 Vue，出问题时可以只对着这份输入输出想。
 *
 * # 形状的权威
 *
 * `internal/core/run.go` 的 `RunEvent.MarshalJSON` —— 五种 `type`，字段名混着
 * camelCase（`runId`）与 snake_case（`manual_steps`）。那边是**冻结基准**
 * （与旧实现的 event 载荷对拍过），所以这里**照抄**，不"顺手统一成 camelCase"。
 *
 * # 未知 `type` 返回 null 而不是抛
 *
 * 后端将来加一种事件（比如 `output`）时，旧前端该**继续显示**已经收到的日志，
 * 而不是整条流白屏。丢掉一条不认识的事件、界面上少一行，是代价最小的那一支。
 */

export interface RunStartedEvent {
  type: 'started'
  runId: string
  capability: string
}

export interface RunLogEvent {
  type: 'log'
  runId: string
  /** `info` / `warn` / `error`。未知值界面按 info 渲染（见 [LEVEL_CLASS]）。 */
  level: string
  message: string
}

export interface RunProgressEvent {
  type: 'progress'
  runId: string
  done: number
  total: number
}

/** 需要人去做的事。**不是**失败：程序做不了的那一步要交给用户。 */
export interface ManualStep {
  capability: string
  /** 掩码账号 id；无账号上下文的任务是 null。 */
  account: string | null
  instruction: string
  /** 去哪做（后端给的是枚举码，界面只显示文案）。 */
  target: string
}

export interface RunFinishedEvent {
  type: 'finished'
  runId: string
  /** `success` / `partial_success` / `manual_required` / `failed`（snake_case，与后端同源）。 */
  outcome: string
  /** 空的时候后端给 `[]` 而不是 `null`，但**这里仍要挡住 null**：那是旧数据的形状。 */
  manual_steps: ManualStep[] | null
  summary: string | null
}

export interface RunErroredEvent {
  type: 'errored'
  runId: string
  message: string
}

export type RunEvent =
  | RunStartedEvent
  | RunLogEvent
  | RunProgressEvent
  | RunFinishedEvent
  | RunErroredEvent

/** 一条日志的累计视图（页面按 run 攒着一份，渲染与断流都要用）。 */
export interface RunStreamState {
  /** `pending` = 已订阅但一条事件都没到；`open` = 在跑；`done` = 收到终局；`lost` = 连接断了。 */
  phase: 'pending' | 'open' | 'done' | 'lost'
  lines: RunLogEvent[]
  progress: { done: number; total: number } | null
  finished: RunFinishedEvent | null
  errored: RunErroredEvent | null
}

export const newStreamState = (): RunStreamState => ({
  phase: 'pending',
  lines: [],
  progress: null,
  finished: null,
  errored: null,
})

/**
 * 把一行 SSE 的 `data` 解成事件。解不出、不是对象、`type` 不认识 ⇒ null。
 *
 * ⚠ 不用 `JSON.parse` 抛异常当分支：EventSource 在流末尾与心跳处都可能给到
 * 空串或非 JSON（反代插的一行注释），那些**不是**要报给用户的事。
 */
export const parseRunEvent = (raw: string): RunEvent | null => {
  if (raw === '') return null
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    return null
  }
  if (typeof value !== 'object' || value === null) return null
  const record = value as Record<string, unknown>
  switch (record.type) {
    case 'started':
      return {
        type: 'started',
        runId: text(record.runId),
        capability: text(record.capability),
      }
    case 'log':
      return {
        type: 'log',
        runId: text(record.runId),
        level: text(record.level),
        message: text(record.message),
      }
    case 'progress':
      return {
        type: 'progress',
        runId: text(record.runId),
        done: number0(record.done),
        total: number0(record.total),
      }
    case 'finished':
      return {
        type: 'finished',
        runId: text(record.runId),
        outcome: text(record.outcome),
        manual_steps: Array.isArray(record.manual_steps)
          ? (record.manual_steps as ManualStep[])
          : null,
        summary: typeof record.summary === 'string' ? record.summary : null,
      }
    case 'errored':
      return {
        type: 'errored',
        runId: text(record.runId),
        message: text(record.message),
      }
    default:
      return null
  }
}

/** 终局事件：收到其中之一，前端**必须**自己 `close()`（后端注释里那条契约）。 */
export const isTerminalEvent = (event: RunEvent): boolean =>
  event.type === 'finished' || event.type === 'errored'

/**
 * 把一条事件并进累计视图。**不改入参**，返回同一对象的引用以便调用方替换 ref。
 *
 * 日志只追加上限 [MAX_LINES] 行：一条直播链的日志能到几十行，而一次批量是
 * 账号数 × 几十 —— 不设上限的话，页面挂一小时会开始吃内存且滚不动。
 * 保留**最后**那些，因为出问题时人看的是最近发生了什么。
 */
export const MAX_LINES = 200

export const applyEvent = (state: RunStreamState, event: RunEvent): RunStreamState => {
  const next: RunStreamState = {
    phase: 'open',
    lines: state.lines,
    progress: state.progress,
    finished: state.finished,
    errored: state.errored,
  }
  switch (event.type) {
    case 'log':
      next.lines = [...state.lines, event]
      if (next.lines.length > MAX_LINES) {
        next.lines = next.lines.slice(next.lines.length - MAX_LINES)
      }
      return next
    case 'progress':
      next.progress = { done: event.done, total: event.total }
      return next
    case 'finished':
      next.finished = event
      next.phase = 'done'
      return next
    case 'errored':
      next.errored = event
      next.phase = 'done'
      return next
    default:
      // `started` 只把"订阅上了"这件事说清楚，界面从 pending 进 open
      return next
  }
}

/** 断流（连接错误 / 404）时的状态。区分"跑完了"与"没连上"是这一档的全部意义。 */
export const markLost = (state: RunStreamState): RunStreamState => ({
  ...state,
  phase: state.phase === 'done' ? 'done' : 'lost',
})

/**
 * 日志级别 → 文字类。
 *
 * ⚠ 类名必须是**字面量**并且集中在这张表里：Tailwind 靠扫源码取类名，
 * 拼出来的 `text-${level}` 不会进产物（§9.1① 那条静默失效）。
 * `info` 不给颜色类 = 跟随正文色，这是刻意的：满屏彩色日志没人看得下去。
 */
export const LEVEL_CLASS: Record<string, string> = {
  info: '',
  warn: 'text-warning',
  error: 'text-error',
}

/** 未知 level 落到 `info`（空串）而不是"没有样式"：后端加一档时界面不能变空白。 */
export const levelClass = (level: string): string => LEVEL_CLASS[level] ?? ''

/**
 * 业务结论 → 中文。查表而不是 `if` 链（设计指南 §9.1①）。
 *
 * ⚠ `finished` **不等于成功**：`outcome` 才是结论，而它有四态。
 * 空串（后端给了个我们不认识的码）时回显原文，不替后端编一个中文名。
 */
export const OUTCOME_LABEL: Record<string, string> = {
  success: '全部成功',
  partial_success: '部分成功',
  manual_required: '需要人工',
  failed: '失败',
}

export const outcomeLabel = (outcome: string): string => OUTCOME_LABEL[outcome] ?? outcome

/**
 * 结论 → badge 类（裸 `badge-soft` + 颜色，§5.4 那条"中性标记不给 `badge-neutral`"）。
 *
 * ⚠ 这里**不**用"红=坏、绿=好"两位（§2.4 把涨跌色让给了数字）：
 * `partial_success` 是"重跑或等下一轮"的正常流水，染成红会让人以为出了事故。
 */
export const OUTCOME_BADGE: Record<string, string> = {
  success: 'badge-soft badge-success',
  partial_success: 'badge-soft badge-info',
  manual_required: 'badge-soft badge-warning',
  failed: 'badge-soft badge-error',
}

export const outcomeBadge = (outcome: string): string => OUTCOME_BADGE[outcome] ?? 'badge-soft'

/** 已经跑了多久（`1 分 04 秒`）。用于 §6.3 那条"显示已用时长"。 */
export const formatElapsed = (millis: number): string => {
  const total = Math.max(0, Math.floor(millis / 1000))
  const minutes = Math.floor(total / 60)
  const seconds = total % 60
  if (minutes === 0) return `${seconds} 秒`
  return `${minutes} 分 ${String(seconds).padStart(2, '0')} 秒`
}

const text = (value: unknown): string => (typeof value === 'string' ? value : '')
const number0 = (value: unknown): number => (typeof value === 'number' && isFinite(value) ? value : 0)
