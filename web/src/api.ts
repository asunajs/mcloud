/**
 * 后端客户端。
 *
 * 所有网络访问集中在这里，页面不直接 `fetch` —— 便于统一错误形态与类型。
 * 类型目前手写；接口冻结后换成 openapi 生成（与 mcloud 的未决项 2 同一处置）。
 *
 * 开发期走 Vite 代理（同源），生产期后端直接发前端产物，也是同源 ——
 * 刻意不用 CORS，避免"开发能用、打包上去就不行"。
 */

import { classifyAuthFailure, noteAuthOk, noteAuthRequired } from './ui/auth-signal'
// 转出给页面用：页面只 import `api` 就够了，不必知道状态存在哪个模块里
export type { AuthState } from './ui/auth-signal'
export { currentUser } from './ui/auth-signal'

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

/** 一条留在用户网盘上、还没清理的产物。 */
export interface LeftoverArtifact {
  /** 产物类型。今天是 `cloud-file`，刻意开放（将来会有外链、笔记等）。 */
  kind: string
  /** 账号（掩码标识，**不是**凭据）。 */
  account: string
  /** 上游侧的 `fileId` —— 手工清理时要用它。 */
  id: string
}

export interface RunRecord {
  run_id: number
  task: string
  account: string
  started_at_ms: number
  duration_ms: number
  outcome: TaskOutcome
  /**
   * 结构化明细。
   *
   * 今天只有一格：`artifacts` —— 这次运行**留在用户网盘上、还没清理**的文件。
   * 非空时后端还会发一条 warn 日志。
   *
   * ⚠ 它是"跑完那一刻还没清"的快照，不是"最终没清"：
   * 链尾清理干净的任务这里就是 `null`。
   */
  details?: { artifacts?: LeftoverArtifact[] } | null
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
  /**
   * 可判别的机器码，例如 `setup_required`。
   *
   * ⚠ 与 `error` 分工明确：`error` 是**给人看的**（可以随时改字），
   * 这个字段是给**代码判断**的。让代码去匹配 `error` 的文案，
   * 后端改一个字就会让判断静默失效。
   */
  code?: string
}

/**
 * 带 HTTP 状态与后端原文的 API 错误。
 *
 * 刻意**保留后端原文**而不是统一成「操作失败」：上游的提示往往已经说清了
 * 该看哪里，翻译成通用文案等于把信息丢掉。
 */
export class ApiError extends Error {
  readonly status: number
  /** 后端的机器码（如 `setup_required`），没有则 `undefined`。 */
  readonly code: string | undefined
  constructor(status: number, message: string, code?: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

/**
 * 底层请求：统一处理 JSON 解析、错误原文透出与 204。
 *
 * 导出是为了让页面复用同一份语义 —— 复制一份到页面里迟早会分叉。
 */
/**
 * 这次失败是不是"认证动作**自己**的正常失败"（而非"整个应用该跳登录页"）。
 *
 * ⚠ 必须**按方法区分**，不能只看路径：
 *
 * | 请求 | 401 的含义 | 该不该跳登录页 |
 * |---|---|---|
 * | `POST /api/session`（登录） | 用户名或口令错 | **否** —— 用户就在登录页上 |
 * | `POST /api/setup`（建管理员） | 已初始化/校验失败 | **否** —— 同理 |
 * | `GET /api/session`（查会话） | 没登录 | **是** —— 这正是启动时的判据 |
 *
 * 只看路径的话，启动时那句 `GET /api/session` 的 401 会被吞掉，
 * 于是应用永远停在"正在检查"上。
 *
 * 不排除 POST 的话，口令输错一次就会让登录页自己重渲染、把表单清掉。
 */
function isAuthActionFailure(method: string, path: string): boolean {
  const m = method.toUpperCase()
  if (path === '/api/session') return m === 'POST' || m === 'DELETE'
  if (path === '/api/setup') return m === 'POST'
  return false
}

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
    const body = data && typeof data === 'object' ? (data as ApiErrorBody) : null
    const message = body && 'error' in body ? String(body.error) : `HTTP ${response.status}`
    const code = body && typeof body.code === 'string' ? body.code : undefined

    // ⭐ 认证失败要在**这一层**统一识别。
    //
    // 放在这里而不是各页面：每个页面都要处理的话，漏一个就是那一个页面
    // 永远卡在错误态；而"漏了"没有任何测试会红。
    //
    // ⚠ 登录接口自己**排除在外**：它返回 401 表示"用户名或口令错误"，
    // 那是这个动作的正常失败，不是"整个应用要跳登录页"——
    // 而用户此刻**就在**登录页上。
    const method = (init?.method ?? 'GET').toUpperCase()
    if (!isAuthActionFailure(method, path)) {
      const authFailure = classifyAuthFailure(response.status, code)
      if (authFailure) noteAuthRequired(authFailure)
    }

    throw new ApiError(response.status, message, code)
  }

  if (response.status === 204) return undefined as T
  return data as T
}

// ── 认证（WebUI 登录闸）──────────────────────────────────────────────
//
// ⚠ 这一层与下面的 `/api/login/*`（云盘账号登录）是**两件事**：
// 这里管"谁能进这个 WebUI"，那边管"往账号列表里加一个云盘账号"。

export interface SetupStatus {
  initialized: boolean
  /**
   * 口令长度下限。**后端给**，前端不硬编 ——
   * 硬编一份必然与后端漂移，而漂移的表现是"前端说可以、后端拒绝"。
   */
  minPasswordLen: number
}

export interface AuthSession {
  userId: string
  name: string
  kind: 'web' | 'cli'
}

export const getSetupStatus = () => request<SetupStatus>('/api/setup')

/**
 * 首次建管理员。成功后后端会**直接下发会话 cookie**，无需再登录一次。
 *
 * 这里显式置 `ready` —— 而不是在 `request` 里"任何成功都置 ready"。
 * 理由是公开接口（`/api/version`、`/api/health`）**未初始化时也返回 200**，
 * 那样会让应用从"要建管理员"错误地翻成"放行"，然后每个页面各自
 * 撞一鼻子 503。放行只能由**明确的认证动作**宣布。
 */
export const createAdmin = async (username: string, password: string) => {
  const s = await request<AuthSession>('/api/setup', {
    method: 'POST',
    body: JSON.stringify({ username, password }),
  })
  noteAuthOk(s.name)
  return s
}

export const loginSession = async (username: string, password: string) => {
  const s = await request<AuthSession>('/api/session', {
    method: 'POST',
    body: JSON.stringify({ username, password }),
  })
  noteAuthOk(s.name)
  return s
}

export const logoutSession = () => request<void>('/api/session', { method: 'DELETE' })

/** 查当前会话。401 会经 `request` 让应用转到登录页 —— 这是启动时的判据。 */
export const getCurrentSession = () => request<AuthSession>('/api/session')

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

// ── 云盘账号登录 ────────────────────────────────────────────────────────
//
// 注意与 `/api/session` 的区别：那个登的是**本实例管理员**，
// 这里登的是**要跑任务的云盘账号**。两者的凭据存在不同的文件里。

/** 平台形态。mac 与 win 在请求头、UA 与落盘数据上都不一样。 */
export type PcPlatform = 'windows' | 'macos'

export interface StartQrResponse {
  session_id: string
  /** 二维码载荷（原始链接）。排障与自渲染用。 */
  payload: string
  /** 二维码图片（SVG 的 data URL），直接塞进 `<img src=...>`。 */
  image_svg: string
  platform: PcPlatform
  platform_label: string
  poll_interval_ms: number
  ttl_seconds: number
  device_id: string
}

export interface LoginOutcome {
  phase: 'not_scanned' | 'scanned' | 'logged_in' | 'failed'
  terminal: boolean
  reason?: string
  retry_after_ms?: number
  /** 成功时的**掩码**账号名（完整手机号不出现在响应里）。 */
  account?: string
  platform?: PcPlatform
}

/** 出码。**不发上游请求**：二维码是服务端本地拼的。 */
export const startQrLogin = (platform: PcPlatform) =>
  request<StartQrResponse>('/api/login/qr', {
    method: 'POST',
    body: JSON.stringify({ platform }),
  })

/** 问一次"扫了没"。用 POST：它会推进服务端会话状态。 */
export const pollQrLogin = (sessionId: string) =>
  request<LoginOutcome>('/api/login/qr/poll', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId }),
  })

/** 放弃一次扫码（离开页面时调用，让服务端把会话丢掉）。 */
export const cancelQrLogin = (sessionId: string) =>
  request<{ cancelled: boolean }>(`/api/login/qr/${encodeURIComponent(sessionId)}`, {
    method: 'DELETE',
  })

export interface SliderPayload {
  background: string
  puzzle: string
  width: number | null
  /** 已解出的答案。上游把答案加密在题面里，不需要图像识别。 */
  answer?: string
}

export interface SmsStartResponse {
  phase: 'sent' | 'need_slider' | 'slider_exhausted'
  remain_get_times?: number
  slider?: SliderPayload
}

export const startSmsLogin = (phone: string, platform: PcPlatform) =>
  request<SmsStartResponse>('/api/login/sms/start', {
    method: 'POST',
    body: JSON.stringify({ phone, platform }),
  })

export const submitSmsLogin = (input: {
  phone: string
  platform: PcPlatform
  code: string
  sliderAnswer?: string
}) =>
  request<LoginOutcome>('/api/login/sms', {
    method: 'POST',
    body: JSON.stringify({
      phone: input.phone,
      platform: input.platform,
      code: input.code,
      slider_answer: input.sliderAnswer,
    }),
  })

export const loginWithPassword = (username: string, password: string, platform: PcPlatform) =>
  request<LoginOutcome>('/api/login/password', {
    method: 'POST',
    body: JSON.stringify({ username, password, platform }),
  })

export const getPcDevice = () => request<{ device_id: string }>('/api/login/device')

export const rotatePcDevice = () =>
  request<{ device_id: string }>('/api/login/device/rotate', { method: 'POST' })

/** 列出已登录的云盘账号。 */
export interface AccountSummary {
  id: string
  nickname: string | null
  /** `windows` / `macos`；`null` = 非 PC 端登录的账号。 */
  pc_platform: PcPlatform | null
  device_id: string
  /** 凭据到期的 epoch 毫秒；`0` = 解析不出。 */
  expire: number
}

export const listAccounts = () => request<AccountSummary[]>('/api/accounts')

export const deleteAccount = (id: string) =>
  request<void>(`/api/accounts/${encodeURIComponent(id)}`, { method: 'DELETE' })
