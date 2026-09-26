/**
 * 后端客户端。
 *
 * 所有网络访问集中在这里，页面不直接 `fetch` —— 便于统一错误形态与类型。
 * 类型目前手写；接口冻结后换成 openapi 生成（与 mcloud 的未决项 2 同一处置）。
 *
 * 开发期走 Vite 代理（同源），生产期后端直接发前端产物，也是同源 ——
 * 刻意不用 CORS，避免"开发能用、打包上去就不行"。
 */

export interface TaskMeta {
  name: string
  title: string
  description: string
  hidden: boolean
  params: string[]
}

/** 任务结果。`skipped` 不是失败 —— 活动未开启或条件不满足。 */
export type TaskOutcome =
  | { status: 'success'; summary: string }
  | { status: 'skipped'; reason: string }
  | { status: 'failed'; reason: string }

export interface RunRecord {
  run_id: number
  task: string
  account: string
  started_at_ms: number
  duration_ms: number
  outcome: TaskOutcome
}

export interface RunResponse {
  run_id: number
  task: string
  account: string
  outcome: TaskOutcome
  duration_ms: number
}

/** 运行事件，与后端 `RunEvent` 的 `type` 标签一一对应。 */
export type RunEvent =
  | { type: 'started'; run_id: number; task: string; account: string; at_ms: number }
  | {
      type: 'log'
      run_id: number
      level: 'debug' | 'info' | 'warn' | 'error'
      message: string
      at_ms: number
    }
  | { type: 'progress'; run_id: number; step: number; total: number; label: string }
  | { type: 'finished'; run_id: number; outcome: TaskOutcome; duration_ms: number }

/** 后端返回的错误体。 */
export interface ApiErrorBody {
  error: string
}

/**
 * 带 HTTP 状态与后端原文的 API 错误。
 *
 * 刻意**保留后端原文**而不是统一成「操作失败」：上游的提示往往已经说清了
 * 该看哪里，翻译成通用文案等于把信息丢掉。
 */
export class ApiError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

/**
 * 底层请求：统一处理 JSON 解析、错误原文透出与 204。
 *
 * 导出是为了让页面复用同一份语义 —— 复制一份到页面里迟早会分叉。
 */
export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    credentials: 'same-origin',
    headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
    ...init,
  })

  const text = await response.text()
  let data: unknown = null
  if (text) {
    try {
      data = JSON.parse(text)
    } catch {
      // 后端异常时可能返回非 JSON（例如反向代理的 HTML 错误页），
      // 保留原文片段便于排查，而不是抛一句无信息的解析错误
      if (!response.ok) {
        throw new ApiError(response.status, `响应不是 JSON（HTTP ${response.status}）：${text.slice(0, 200)}`)
      }
      throw new ApiError(response.status, `响应不是 JSON：${text.slice(0, 200)}`)
    }
  }

  if (!response.ok) {
    const message =
      data && typeof data === 'object' && 'error' in data
        ? String((data as ApiErrorBody).error)
        : `HTTP ${response.status}`
    throw new ApiError(response.status, message)
  }

  if (response.status === 204) return undefined as T
  return data as T
}

// ── 具体接口 ────────────────────────────────────────────────────────────

export interface VersionInfo {
  name: string
  version: string
}

export interface ServiceStatus {
  name: string
  version: string
  taskCount: number
  lastRun: RunRecord | null
  /** 调度循环是否**真的**在跑。运行时实况，不是配置项。 */
  schedulerRunning: boolean
  runningCount: number
  scheduleCount: number
  pipelineCount: number
}

export const getVersion = () => request<VersionInfo>('/api/version')

export const getStatus = () => request<ServiceStatus>('/api/status')

export const listCapabilities = () => request<{ tasks: TaskMeta[] }>('/api/capabilities')

export const listRuns = () => request<RunRecord[]>('/api/runs')

export const getRun = (id: number) => request<RunRecord>(`/api/runs/${id}`)

/**
 * 启动一次运行。
 *
 * `background = true` 时**立刻**返回真实的 run id，任务在后台跑。
 * 这是"界面上能点中止"的前提：同步执行的话请求还没返回，
 * 用户根本没有可中止的时机。
 */
export const startRun = (
  task: string,
  account: string,
  body: Record<string, unknown> = {},
  background = false,
) =>
  request<RunResponse>('/api/runs', {
    method: 'POST',
    body: JSON.stringify({ task, account, body, background }),
  })

/** SSE 事件流地址。交给 `EventSource` —— 它自己带同源 cookie。 */
export const runEventsUrl = (runId: number) => `/api/runs/${encodeURIComponent(String(runId))}/events`

/**
 * 订阅某次运行的事件流。
 *
 * 返回取消函数；调用方**必须**在卸载/结束时调用它 ——
 * `EventSource` 断线会自动重连，不显式关闭就会一直挂着。
 *
 * # 终局即关闭
 *
 * 收到 `finished` 后本函数会**自动** `close()`，并调用 `onTerminal`。
 * 这不是可选的优化：`EventSource` 按规范会自动重连，不关的话
 * 一次已结束的运行会被无限重连（每次带 `Last-Event-ID`，
 * 服务端补发 0 条再关），每个打开的页面都挂着一条永久连接。
 */
export function subscribeRun(
  runId: number,
  handlers: {
    onEvent?: (ev: RunEvent) => void
    onError?: (err: Event) => void
    /** 终局（已结束）。此时流已被关闭，调用方不必再关。 */
    onTerminal?: (ev: RunEvent) => void
    /**
     * 断线重连时带上"已看到的序号"。
     *
     * 服务端据此只补发新的。传 `null` 表示从头开始。
     */
    lastEventId?: number | null
  } = {},
): () => void {
  const url = new URL(runEventsUrl(runId), window.location.origin)
  if (handlers.lastEventId != null) {
    // 语义是"**已经看到**的序号"，服务端返回它之后的。
    // 传成"下一个要取"会静默跳过一条。
    url.searchParams.set('after', String(handlers.lastEventId))
  }

  const source = new EventSource(url.toString())
  let closed = false

  const close = () => {
    if (closed) return
    closed = true
    source.close()
  }

  const handle = (raw: MessageEvent) => {
    let ev: RunEvent
    try {
      ev = JSON.parse(raw.data) as RunEvent
    } catch {
      // 单条事件解析失败不应中断整条流：反代插入的心跳/注释行
      // 不是要报给用户的事
      return
    }
    handlers.onEvent?.(ev)
    if (ev.type === 'finished') {
      close()
      handlers.onTerminal?.(ev)
    }
  }

  // 后端用 `event:` 名区分类型，需分别注册
  for (const name of ['started', 'log', 'progress', 'finished'] as const) {
    source.addEventListener(name, handle as EventListener)
  }
  if (handlers.onError) source.onerror = handlers.onError

  return close
}

// ── 定时任务 ────────────────────────────────────────────────────────────

/** 定时任务。`nextFireAt` 由服务端算 —— 各客户端各算一遍必然分叉。 */
export interface Schedule {
  id: string
  name: string
  task: string
  accounts: string[]
  expr: string
  timezone: string
  body: unknown
  enabled: boolean
  /** **水位线**：已处理过的触发时刻。不是"上次运行时间"。 */
  lastFireMs: number | null
  lastRunMs: number | null
  lastStatus: 'ok' | 'error' | 'skipped' | null
  lastError: string | null
  runCount: number
  /** 下次触发时间。**未启用时为 null** —— 给一个就是在骗人。 */
  nextFireAt: number | null
}

export interface ScheduleInput {
  name: string
  task: string
  accounts: string[]
  expr: string
  timezone: string
  body?: unknown
  enabled: boolean
}

export const listSchedules = () => request<Schedule[]>('/api/schedules')

export const saveSchedule = (id: string, input: ScheduleInput) =>
  request<Schedule>(`/api/schedules/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })

export const deleteSchedule = (id: string) =>
  request<{ ok: boolean }>(`/api/schedules/${encodeURIComponent(id)}`, { method: 'DELETE' })

// ── 任务编排 ────────────────────────────────────────────────────────────

export interface PipelineStep {
  task: string
  body?: unknown
  /** 这一步失败也继续后续步骤。默认 false —— 多数步骤是有依赖的。 */
  continueOnError: boolean
}

export interface Pipeline {
  id: string
  name: string
  accounts: string[]
  accountMode: 'sequential' | 'parallel'
  /** 步骤失败时是否中止整条链。 */
  stopOnError: boolean
  steps: PipelineStep[]
  enabled: boolean
  lastRunMs: number | null
}

export interface PipelineInput {
  name: string
  accounts: string[]
  accountMode: 'sequential' | 'parallel'
  stopOnError: boolean
  steps: PipelineStep[]
  enabled: boolean
}

export const listPipelines = () => request<Pipeline[]>('/api/pipelines')

export const savePipeline = (id: string, input: PipelineInput) =>
  request<Pipeline>(`/api/pipelines/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })

export const deletePipeline = (id: string) =>
  request<{ ok: boolean }>(`/api/pipelines/${encodeURIComponent(id)}`, { method: 'DELETE' })

/** 后台执行一条编排。立即返回，进度去运行历史看。 */
export const runPipeline = (id: string) =>
  request<{ pipeline: string; started: boolean }>(
    `/api/pipelines/${encodeURIComponent(id)}/run`,
    { method: 'POST' },
  )

// ── 日志与中止 ──────────────────────────────────────────────────────────

export interface LogLine {
  runId: number
  level: 'debug' | 'info' | 'warn' | 'error'
  message: string
  atMs: number
  seq: number
}

/**
 * 取某次运行的日志。
 *
 * `after` 是"**已经看到**的序号"（不是"下一个要取"）——
 * 混淆会静默跳过一条。
 */
export const getRunLogs = (runId: number, after?: number) => {
  const q = after != null ? `?after=${after}` : ''
  return request<LogLine[]>(`/api/runs/${runId}/logs${q}`)
}

/**
 * 请求中止一次运行。
 *
 * 中止是**协作式**的：返回 `cancelled: true` 只表示请求已受理，
 * 不表示已经停了。`cancelled: false` 通常意味着任务刚好跑完 ——
 * 那是正常竞态，不是错误。
 */
export const cancelRun = (runId: number) =>
  request<{ cancelled: boolean; running: boolean }>(`/api/runs/${runId}/cancel`, {
    method: 'POST',
  })
