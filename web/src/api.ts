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

/** 任务分组键（界面分栏用，不影响执行）。 */
export type TaskGroup = 'signin' | 'live' | 'app' | 'device'

/**
 * 一个参数的说明 —— 前端据此画"任务配置"表单。
 *
 * ⚠ 从前 `/api/capabilities` 只给**参数名**（`string[]`），类型/默认值全靠前端猜；
 * 猜错的表现是"填了没用"。现在类型、默认值、取值范围都由后端给。
 */
export interface TaskParamSpec {
  name: string
  /** 界面上的标签。 */
  title: string
  kind: 'bool' | 'number' | 'text'
  /** 默认值（JSON）。用于"从没配过"时的预填。 */
  default: unknown
  min?: number
  max?: number
  unit?: string
  help?: string
}

export interface TaskMeta {
  name: string
  title: string
  description: string
  hidden: boolean
  /** 分组键。 */
  group: TaskGroup
  /**
   * 分组的中文名 —— **后端给**。
   *
   * 本仓栽过一次：任务页曾把签到族那 8 个任务名硬编码在前端当分组依据，
   * 而后端的注册表才是权威。前端每多一份这类"键→名字"的表，就多一处迟早漂移的地方。
   */
  groupLabel: string
  params: TaskParamSpec[]
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

/**
 * 请求超时（毫秒）。
 *
 * ⚠ **为什么必须有。** 页面的 `busy` / `submitting` / `saving` 都是在
 * `try/finally` 里复位的，而 `fetch` **默认没有超时**：请求悬挂时 `finally`
 * 永远不跑 ⇒ 按钮永远是禁用态、界面看着"卡死"，用户只能刷页。
 *
 * 取 30 秒：上游偶尔确实慢（网盘列表、直播），要留够；但也不能无限等。
 * SSE 不走这里 —— 它有自己的长连接与重连。
 */
const REQUEST_TIMEOUT_MS = 30_000

export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response
  try {
    response = await fetch(path, {
      credentials: 'same-origin',
      headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
      ...init,
      // 调用方自己传了 signal 就尊重它（那种情况由调用方负责超时）。
      signal: init?.signal ?? AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    })
  } catch (e) {
    // fetch 超时只抛一个 DOMException（message 是英文的 "The operation was aborted"），
    // 直接冒到界面等于没说清发生了什么，这里换成能读懂的一句。
    if (e instanceof DOMException && (e.name === 'TimeoutError' || e.name === 'AbortError')) {
      throw new ApiError(
        0,
        e.name === 'TimeoutError'
          ? `请求超时（${REQUEST_TIMEOUT_MS / 1000} 秒没有响应），请重试`
          : '请求已取消',
      )
    }
    throw e
  }

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
  /**
   * 登录时该发什么凭据（后端自述，用户口径 2026-10-01 的"前端先 hash 一次"）：
   *
   * - `sha256` —— 发 `SHA-256(明文)` 的 hex（新档，目标形态）
   * - `plain`  —— 发**明文**（库里还是老档：argon2 直接吃明文；后端验过会当场升级）
   *
   * ⚠ 声明成可选：**旧后端不带这个字段**。前端在缺失时按 `plain` 走 ——
   * 那是对"新老两种后端"都成立的那条路（新后端收到明文会自己补一次摘要）。
   */
  passwordScheme?: PasswordScheme
}

/** 口令凭据档位，与后端 `SetupStatus.password_scheme` 同义。 */
export type PasswordScheme = 'sha256' | 'plain'

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
 *
 * ⚠ `credential` 是**口令凭据**，不是明文：调用方先按 `/api/setup` 自述的
 * `passwordScheme` 过一遍 `ui/password.ts` 的 `credentialFor`（见那里的取舍）。
 */
export const createAdmin = async (username: string, credential: string) => {
  const s = await request<AuthSession>('/api/setup', {
    method: 'POST',
    body: JSON.stringify({ username, password: credential }),
  })
  noteAuthOk(s.name)
  return s
}

/** 登录。`credential` 同上：凭据，不是明文。 */
/**
 * 账户安全（口令策略 / 改口令）。
 *
 * 与 `/api/setup` 的 `passwordScheme` 同一套档位：库里是老档时这两年
 * 前端要发明文，服务端验过会顺手升级（见 `ui/password.ts`）。
 */
export interface SecurityView {
  /** 口令长度下限（可改）。⚠ 只对**明文**生效：摘要恒 64 位，量不出长度。 */
  minPasswordLen: number
  /** 新建会话的有效期（小时）。改它只影响之后新建的会话。 */
  sessionTtlHours: number
  passwordScheme?: PasswordScheme
}

export const getSecurity = () => request<SecurityView>('/api/admin/security')

/**
 * 改策略。
 *
 * ⚠ **两个字段都要给**（后端 PUT = 整体替换）：少给会 422。
 * 这样"以为改了、其实没改"不可能发生 —— 调用方把当前值一起发回来。
 */
export const putSecurity = (patch: { minPasswordLen: number; sessionTtlHours: number }) =>
  request<SecurityView>('/api/admin/security', {
    method: 'PUT',
    body: JSON.stringify(patch),
  })

/** 改口令的结果：被结束的旧会话数 + 当前用户（后端会顺手换一张新 cookie）。 */
export interface PasswordChanged {
  userId: string
  name: string
  kind: 'web' | 'cli'
  /** 这次改口令顺带结束了多少个**旧**会话（不含马上发的那张新令牌）。 */
  sessionsEnded: number
}

/**
 * 改口令。`current` / `next` 都是**凭据**（不是明文）——
 * 调用方先过 `ui/password.ts` 的 `credentialFor`。
 *
 * ⚠ 当前口令**必填**（后端也必填）：用户口径 2026-10-01
 * ——「你口令更改怎么取消了旧口令」。
 */
export const changePassword = async (current: string, next: string) => {
  const r = await request<PasswordChanged>('/api/admin/password', {
    method: 'PUT',
    body: JSON.stringify({ currentPassword: current, newPassword: next }),
  })
  // 后端换了一张新 cookie ⇒ 会话仍然有效，本地状态跟着刷新一下
  noteAuthOk(r.name)
  return r
}

/**
 * 「关于」卡片的数据（`GET /api/system/about`）。
 *
 * ⚠ **没有"最新版本"这一项**：上游仓库是私有的（匿名访问 404），浏览器端没有凭据
 * 去查发布 ⇒ 这里给的是**本次构建实际包含的改动**，那是本机真拿得到的东西。
 */
export interface AboutChange {
  hash: string
  subject: string
}

export interface AboutView {
  name: string
  version: string
  /** 短提交号；构建机上没有 git 时是空串。 */
  commit: string
  /** 构建时工作区有未提交改动（release 会把它们一起编进线上）。 */
  dirty: boolean
  /** 构建时间（本机时区，形如 `2026-10-01 19:20 CST`）。 */
  buildTime: string
  profile: 'release' | 'debug' | string
  /** 本次构建包含的最近若干条改动。 */
  changes: AboutChange[]
}

export const getAbout = () => request<AboutView>('/api/system/about')

/** 一个数据文件的大小。 */
export interface FileSize {
  name: string
  bytes: number
}

/** 存储信息（数据目录 / 日志占用 / 二进制）。 */
export interface StorageView {
  dataDir: string
  logDir: string
  binary: string
  /** 数据文件合计（**不含**日志）。 */
  dataBytes: number
  /** `data/logs` 递归合计。 */
  logBytes: number
  files: FileSize[]
}

export const getStorage = () => request<StorageView>('/api/system/storage')

/**
 * 重启服务。
 *
 * 后端回 **202** 后 300ms 才优雅退出，交给 launchd / systemd 重拉 ——
 * 所以这个 Promise 正常返回**不代表服务已经重启完**，调用方要自己轮询 `/api/health`。
 */
export const restartService = () =>
  request<unknown>('/api/system/restart', { method: 'POST', body: '{}' })

export const loginSession = async (username: string, credential: string) => {
  const s = await request<AuthSession>('/api/session', {
    method: 'POST',
    body: JSON.stringify({ username, password: credential }),
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
  /**
   * 服务进程**开始服务**的时刻（epoch 毫秒）—— 「运行时间」= 现在 − 它。
   *
   * ⚠ 后端给的是**时刻**而不是"已经跑了多少秒"：页面不轮询，一个标量读完就开始变假，
   * 而时刻可以让界面自己往前算（总览每 30 秒重算一次，不用再问后端）。
   */
  startedAtMs: number
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

/** 一个任务的参数配置（任务页「配置」那一页）。 */
export interface TaskConfigView {
  name: string
  specs: TaskParamSpec[]
  /** 已保存的值。缺省键后端已经补成默认值，前端不必自己算。 */
  params: Record<string, unknown>
}

export const getTaskConfig = (task: string) =>
  request<TaskConfigView>(`/api/tasks/${encodeURIComponent(task)}/config`)

/** 保存任务参数。**PUT = 整体替换**：没给的键回落到默认值。 */
export const putTaskConfig = (task: string, params: Record<string, unknown>) =>
  request<TaskConfigView>(`/api/tasks/${encodeURIComponent(task)}/config`, {
    method: 'PUT',
    body: JSON.stringify({ params }),
  })

export const listRuns = () => request<RunRecord[]>('/api/runs')

/**
 * 一个账号**今天**的AI豆进出。
 *
 * ⚠ 账是**按账号**记的：不能把几个账号加总成一个"总AI豆" ——
 * 加出来的数**哪个账号都用不了**（豆不能跨账号花）。所以这一层没有"合计"字段，
 * 界面也一个一个账号地摆（第一版按"总额"设计，是错的）。
 */
export interface BeanTodayAccount {
  /** 账号主键（哈希）。显示要走 `accountName(id)`，别把哈希摆到界面上。 */
  account: string
  /** 今日净增（进 − 出）。**可负** —— 兑换扣豆也在这条流水里。 */
  net: number
  /** 今日收入合计（正数）。 */
  gained: number
  /** 今日支出合计（取正数）。 */
  spent: number
  /** 今日读到几笔流水。 */
  count: number
}

/**
 * 今日AI豆 —— 由**云朵记录**（上游 `cloudRecordV2` 的收支流水）按账号汇总而来。
 *
 * ⚠⚠ **后端还没有这个端点**（2026-09-30）。前端先把契约接上：
 * 端点一旦落地，总览那张卡**一行都不用改**。缺的三块与口径见
 * `docs/task-inventory.md` §5。
 *
 * # 口径
 *
 * - **一个账号一份**（见 [`BeanTodayAccount`]）。这一层刻意**没有** `net`/`gained`
 *   这样的总额字段：豆是按账号记的，合计出来没有使用场景。
 * - "今天"按**服务端**时区算（与 `Schedule.timezone` 同一套），并把那个时刻用
 *   `nowMs` 回给前端。⚠ 前端**不要**拿本地时钟自己算日界：那会在跨时区/漂移时
 *   把"今天"算到别的日子上，而界面上看不出来（组件指南 §5.9 同一条理由）。
 * - `failed` 非空 ⇒ **这份读数是不完整的**。界面必须**逐个账号**说出是谁没读到，
 *   **不能**把它当成 0 混进那一行（"没读到"与"今天没进出"是两件事）。
 *   账号主键照 [`AccountSummary.id`] 那一套（哈希，只当键用）。
 */
export interface BeanToday {
  /** 服务端算"今天"用的时刻（epoch 毫秒）。 */
  nowMs: number
  /** 每个账号一份。空数组 = 一个账号都没配。 */
  accounts: BeanTodayAccount[]
  /** 没读到流水的账号，以及**后端原话**的原因（原样透出）。 */
  failed: { account: string; error: string }[]
}

/** `GET /api/beans/today` —— ⚠ 后端未实现，见 [`BeanToday`]。 */
export const getBeanToday = () => request<BeanToday>('/api/beans/today')

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

/**
 * 发验证码。
 *
 * ⚠ `sliderAnswer` 必须在**需要滑块时回填**（用上一次响应里 `slider.answer`）：
 * 上游在需要滑块时**不发**短信，只有带上答案的那一次才会真的把短信发出去。
 * 不带它就再点一次，拿回来的是同一道题 —— 表现是"自动完成没反应"。
 */
export const startSmsLogin = (phone: string, platform: PcPlatform, sliderAnswer?: string) =>
  request<SmsStartResponse>('/api/login/sms/start', {
    method: 'POST',
    body: JSON.stringify({ phone, platform, slider_answer: sliderAnswer }),
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

// ── App 端（手机客户端）短信登录 ──────────────────────────────────────
//
// 与上面那条 PC 短信链路**不是同一个协议**：App 用另一套密钥与设备头，
// 请求体还要加密。所以端点分开，前端也分开两个函数 —— 合成一个带
// `platform: 'app'` 的函数会让"我在登哪一端"变得看不出来。

export interface AppSmsStartResponse {
  phase: 'sent' | 'need_captcha' | 'slider_exhausted'
  remain_get_times?: number
  captcha_type?: number
  /** 已解出的滑块答案（可直接回填重发，不用用户去拖）。 */
  slider_answer?: string
  slider_background?: string
  slider_puzzle?: string
  slider_width?: number | null
}

/** 发验证码（App 链路）。`sliderAnswer` 与 PC 那支同一个理由，必须回填。 */
export const startAppSmsLogin = (phone: string, sliderAnswer?: string) =>
  request<AppSmsStartResponse>('/api/login/app/sms/start', {
    method: 'POST',
    body: JSON.stringify({ phone, slider_answer: sliderAnswer }),
  })

export const submitAppSmsLogin = (input: {
  phone: string
  code: string
  sliderAnswer?: string
}) =>
  request<LoginOutcome>('/api/login/app/sms', {
    method: 'POST',
    body: JSON.stringify({
      phone: input.phone,
      code: input.code,
      slider_answer: input.sliderAnswer,
    }),
  })

export const getPcDevice = () => request<{ device_id: string }>('/api/login/device')

export const rotatePcDevice = () =>
  request<{ device_id: string }>('/api/login/device/rotate', { method: 'POST' })

/** 列出已登录的云盘账号。 */
export interface AccountSummary {
  /** 账号主键：手机号的 SHA-256 前 16 位。⚠ **只当键用，别拿去显示**（认不出是哪个号）。 */
  id: string
  /** 给界面看的名字（掩码手机号，如 `19****01`；没有手机号时回落昵称）。 */
  label: string
  nickname: string | null
  /** `windows` / `macos`；`null` = 非 PC 端登录的账号。 */
  pc_platform: PcPlatform | null
  device_id: string
  /** 凭据到期的 epoch 毫秒；`0` = 解析不出。 */
  expire: number
  /**
   * 是否启用。**停用的账号不出现在各处"挑账号"的下拉里**。
   *
   * 过滤发生在**服务端**（`GET /api/accounts` 默认只给启用的）——
   * 前端各页面不需要、也不应该各写一遍过滤：漏掉的那一处正是"停用了还能选"。
   */
  enabled: boolean
}

/**
 * 列出云盘账号。
 *
 * ⚠ **默认只给启用的**（各处下拉要的就是这个）—— 账号页要能看全部（否则停用之后
 * 就没法启用回来），所以它显式传 `includeDisabled`。
 */
export const listAccounts = (opts?: { includeDisabled?: boolean }) =>
  request<AccountSummary[]>(
    opts?.includeDisabled ? '/api/accounts?includeDisabled=true' : '/api/accounts',
  )

export const deleteAccount = (id: string) =>
  request<void>(`/api/accounts/${encodeURIComponent(id)}`, { method: 'DELETE' })

/**
 * 用**一条凭据**建立或更新账号（`POST /api/accounts`）。
 *
 * 这是"手上只有 auth"那条路：不走扫码/短信/账密。账号主键是**手机号的哈希**，
 * 所以同一个号的凭据再次导入会落到同一个账号上 —— 服务端此时**只换凭据**，
 * 昵称 / 设备号 / 每账号设置都保留，并在响应里用 `created: false` 说明。
 *
 * @param input.auth 原始凭据：`Basic base64("<平台>:<手机号>:<token>")`，
 *   `Basic ` 前缀可有可无（服务端用同一套解析，见 `mcloud-upstream::auth::parse_auth`）。
 * @param input.platform **仅在凭据前缀是 `pc` 时需要**：它决定后续每一次请求的
 *   UA 与设备头，而凭据本身区分不出 win/mac（两种都是 `pc`）。
 */
export const importAccount = (input: { auth: string; platform?: PcPlatform; nickname?: string }) =>
  request<{ account: AccountSummary; created: boolean }>('/api/accounts', {
    method: 'POST',
    body: JSON.stringify(input),
  })

/**
 * 改一个账号的昵称 / 设备号 / 凭据 / 启停（`PATCH /api/accounts/{id}`）。
 *
 * # 为什么是部分更新
 *
 * 这里有一个字段是**凭据**，而凭据从不发给客户端（[`AccountSummary`] 里没有它）
 * ⇒ "读出来、改一处、写回去"这条路不存在，只能"给什么改什么"：
 * **字段缺席 = 不改**，`nickname: ''` = 清空昵称。
 */
export const updateAccount = (
  id: string,
  patch: {
    nickname?: string
    deviceId?: string
    auth?: string
    platform?: PcPlatform
    /** 启用 / 停用（不给 = 不改）。 */
    enabled?: boolean
  },
) =>
  request<AccountSummary>(`/api/accounts/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: JSON.stringify(patch),
  })

// ── 每账号设置（GET/PUT /api/accounts/{id}/settings）────────────────────
//
// 权威定义在 `crates/mcloud-core/src/account.rs` 的 `AccountSettingsDto`。这里的形状
// 与它**逐字段一一对应**，三条口径照抄：
//
// 1. **线格式是 camelCase**（与 /api/schedules、/api/runs 一致）—— 不是落盘那份
//    `AccountSettings` 的 snake_case。敲错键后端回 400，不会静默忽略。
// 2. `PUT` 是**整体替换**：每个字段都必须显式给出（`notify: null` = 不配推送）。
//    缺席没有"保持不变"的意思 ⇒ 客户端必须"先 GET 读全、改一处、再整体写回"。
// 3. 未知键 ⇒ 400（`deny_unknown_fields`）；范围只在后端校验，前端给的是提示。

/** 推送日志级别。与后端 `PushLogLevel` 一一对应（小写）。 */
export type PushLogLevel = 'error' | 'warn' | 'info' | 'debug'

/**
 * 渠道配置里**具名字段之外**的键。
 *
 * 后端对应的是各处 `#[serde(flatten)] extra`：TS 的既定语义是"多余字段原样展开进
 * 请求体"。它不是笔误（所以后端刻意不校验），而**整体替换**意味着客户端回写时
 * 必须把这些键**原样带上** —— 丢了就是"用户没改过的字段被静默抹掉"。
 *
 * ⇒ 下面每个渠道类型都带一条索引签名，界面直接持有 GET 回来的那个对象本身。
 */
export interface TokenChannelConfig {
  token: string
  [extra: string]: unknown
}

/** 企业微信**应用**（两步：先换 access_token，再发消息）。 */
export interface WorkWeixinConfig {
  msgtype?: string
  touser?: string
  agentid?: number
  corpsecret?: string
  corpid?: string
  [extra: string]: unknown
}

/** 企业微信**机器人**（一个 webhook URL 打到底）。 */
export interface WorkWeixinBotConfig {
  url: string
  msgtype?: string
  [extra: string]: unknown
}

/** Telegram Bot。 */
export interface TgBotConfig {
  apiHost?: string
  token: string
  /**
   * ⚠ 后端是 `Value`（TS 里 chat_id 也可以是非数字的字符串，例如 `@channel`），
   * 所以这里**不收窄**成 number。
   */
  chat_id?: string | number
  disable_web_page_preview?: boolean
  proxy?: string
  [extra: string]: unknown
}

/** Bark（iOS）。 */
export interface BarkConfig {
  key: string
  level?: string
  [extra: string]: unknown
}

/** 回逍（2Im）的 query。 */
export interface TwoImQuery {
  plat?: string
  group?: string
  icon?: unknown
  [extra: string]: unknown
}

/** 回逍（2Im）。 */
export interface TwoImConfig {
  key: string
  sid: string
  query?: TwoImQuery
  msgtype?: string
  [extra: string]: unknown
}

/** 钉钉机器人。`secret` 是加签密钥（可选）。 */
export interface DingTalkConfig {
  token: string
  secret?: string
  [extra: string]: unknown
}

/**
 * 邮箱。
 *
 * ⚠ **本版不会真的发信**（后端 `mcloud-app::push::EMAIL_NOT_IMPLEMENTED`）。
 * 界面照 DTO 把字段列全，并在渠道说明里写明这一点 —— 否则用户会以为配了就生效。
 */
export interface EmailConfig {
  pass: string
  from: string
  port?: number
  host: string
  to?: string
  [extra: string]: unknown
}

/** 自定义 POST 的一条模板。`url` 支持 `{title}` / `{text}` 占位符。 */
export interface PushApiData {
  url: string
  method?: string
  headers?: Record<string, string>
  timeout?: number
  data?: Record<string, unknown>
  /** `data` 的备选（后端 `data || body`）。 */
  body?: Record<string, unknown>
  [extra: string]: unknown
}

/** 后端允许**单个模板或模板数组**（`CustomPost` 那个 untagged 枚举）。 */
export type CustomPost = PushApiData | PushApiData[]

/**
 * 推送渠道的顶层键 —— 顺序即界面里的推荐顺序（最常用的排前面）。
 *
 * ⚠ 这个名单同时是**未知键校验**的名单：界面只发这里面的键。后端会拒绝别的。
 */
export const NOTIFY_CHANNELS = [
  'pushplus',
  'serverChan',
  'workWeixinBot',
  'dingTalk',
  'customPost',
  'workWeixin',
  'tgBot',
  'bark',
  'twoIm',
  'email',
] as const

export type NotifyChannelKey = (typeof NOTIFY_CHANNELS)[number]

/** 推送配置（`PushConfig`）。渠道字段缺省 / `null` = 那个渠道不启用。 */
export interface PushConfig {
  title: string
  onlyError: boolean
  minLevel: PushLogLevel
  pushplus?: TokenChannelConfig | null
  serverChan?: TokenChannelConfig | null
  workWeixin?: WorkWeixinConfig | null
  workWeixinBot?: WorkWeixinBotConfig | null
  tgBot?: TgBotConfig | null
  bark?: BarkConfig | null
  twoIm?: TwoImConfig | null
  dingTalk?: DingTalkConfig | null
  email?: EmailConfig | null
  customPost?: CustomPost | null
}

/**
 * 一个账号的设置 —— `GET` 的响应体，也是 `PUT` 的请求体（**整体替换**）。
 *
 * ⚠ `notify` 里有第三方渠道的 token/secret（后端**刻意不打码**读接口，取舍见
 * `mcloud-server::routes::accounts::put_settings`）。界面必须把它们当密文：
 * 默认打码、可切换查看、**不进 console**。
 */
export interface AccountSettings {
  /** 备份任务等待时间（秒）。`0` = 不等。后端上限 3600。 */
  backupWaitSecs: number
  /** 剩余多少天时刷新凭证。后端范围 0..=3650。 */
  refreshTokenDays: number
  /** 跳过的任务 **id**（数字）。旧形状是任务名，后端读旧库时会**丢弃**它们。 */
  skipTasks: number[]
  aiAvatarEnabled: boolean
  /** AI 新头像每日轮数上限。后端上限 1000。 */
  aiAvatarDailyLimit: number
  redpackEnabled: boolean
  playAiEnabled: boolean
  koulingEnabled: boolean
  /** `null` = 这个账号**一个渠道都不推**（后端没有进程级兜底）。 */
  notify: PushConfig | null
}

/**
 * 读某账号的设置。
 *
 * ⚠ 别人的账号与不存在的账号回**同一个** 404（后端不区分，免得泄露存在性）——
 * 界面不能把 404 说成"网络错误"。
 */
export const getAccountSettings = (id: string) =>
  request<AccountSettings>(`/api/accounts/${encodeURIComponent(id)}/settings`)

/**
 * **整体替换**某账号的设置。
 *
 * 每个字段都必须给出（`notify: null` = 清空推送）。返回值是落盘后回读的那一份，
 * 调用方应当拿它**回填界面** —— 后端可能做过归一化。
 */
export const putAccountSettings = (id: string, settings: AccountSettings) =>
  request<AccountSettings>(`/api/accounts/${encodeURIComponent(id)}/settings`, {
    method: 'PUT',
    body: JSON.stringify(settings),
  })

// ── 直播口令 → 小红花 ─────────────────────────────────────────────────
//
// 这一族**没有专属端点**：它就是一条普通任务，走 `POST /api/runs` 与那条 SSE。
// 所以这里**刻意只放类型与纯函数**，不包一层 `runLiveRoom()` ——
// 多了那层包装，页面就分不清"我调的是通用运行"还是"某个私有接口"，
// 而后端换端点时两处都要改。

/** 直播口令任务的**唯一**能力名（与后端 `Task::name()` / `/api/capabilities` 一致）。 */
export const LIVE_ROOM_TASK = 'live-room'

/**
 * 一次直播运行的参数，与后端 `LiveRoomParams` 一一对应。
 *
 * ⚠ 键名是后端 `LiveRoomRun::params_from_body` 认的那四个（**驼峰**，
 * 与 Rust 侧的 `listen_seconds` 不同名）—— 按 Rust 字段名发过去会被静默忽略、
 * 退回默认值，而界面上显示的数字还是用户填的那个。
 */
export interface LiveRoomParams {
  /**
   * 手工口令。后端同时接受"一栏自由文本"与字符串数组，
   * 这里给数组（拆分交给 [`splitKouling`]，它必须与后端 `split_kouling` 同口径）。
   */
  codes: string[]
  /** 自动收集时听弹幕的时长（秒）。**必须 > 0**。 */
  listenSeconds: number
  /** 获取口令但不领取。 */
  koulingOnly: boolean
  /** 只用给定口令（不开弹幕）。为真时 `codes` 必须非空。 */
  skipKouling: boolean
}

/**
 * 口令分隔符 —— 换行 / 回车 / 逗号 / 顿号 / **空白**。
 *
 * # ⚠ 与后端 `split_kouling` 的关系：前端是它的**超集**
 *
 * 后端按 `['\n','\r',',','，','、']` 拆。而这一页的输入框与需求都写着
 * "多条用**空格**、换行或逗号分隔"，所以前端**多**拆一个空白。
 *
 * 这不是口径漂移，而是**故意的**：把 `"A B"` 当成一条口令发下去，
 * 后端会原样拿它去领 —— 它必然回 `200108 无效口令`，而用户看到的是
 * "我明明填了两条，怎么只领了一条、还说无效"。前端先拆开，
 * 后端拿到的就是两条正常口令。
 *
 * 反过来（后端拆得比前端多）才是危险的，而那种情况不存在。
 * 顿号/空白都用 `\s` 之外的显式列举，避免把全角空格等意外卷进来。
 */
export function splitKouling(raw: string): string[] {
  const out: string[] = []
  for (const field of raw.split(/[\s,，、]+/)) {
    const code = field.trim()
    if (code === '' || out.includes(code)) continue
    out.push(code)
  }
  return out
}

// ── 结果的结构化明细（后端 `RedFlowerReport`）──────────────────────────
//
// ⚠ 这些字段**还不是后端当前的对外形状**：今天 `RunRecord.details` 只有
// `artifacts` 一格，小红花的报告**只以一句话**（`summary_line()`）出现在
// outcome 里。下面是"后端把报告整体塞进 `details` 之后"的读法 ——
// 页面在字段缺失时**逐段降级**，不会因为多一层而整块空掉。
//
// 这样写而不是写死一句话，是因为两个判据只能从这里拿：
//   - 「口令已失效（本场已结束）」与「无效口令」**必须分开说**；
//   - 「进账多少朵」要用 ⑥−① 的读数，不能把口令的 `flowerNum` 加起来
//     （那会漏掉③首次参与奖那几朵）。

/**
 * 上游对一条口令的判定。
 *
 * ⚠ `kouling_expired` 与 `invalid` **不是一回事**，界面上必须分开显示：
 * 前者是"口令认识、但本场直播已经结束了"，后者是"这条口令不认识（多半抄错了）"。
 * 把它们合并成一句"口令错误"会把排查方向指向**完全错误**的一边。
 */
export type LiveRoomDrawKind =
  | 'succeeded'
  | 'already_redeemed'
  | 'kouling_expired'
  | 'invalid'
  | 'other'

/** 逐口令的一次领取记录。 */
export interface LiveRoomDrawRecord {
  code: string
  /** 上游的判定类别。见 [`LiveRoomDrawKind`]。 */
  kind: LiveRoomDrawKind
  /** 成功时的花朵数。 */
  flowerNum?: number
  /** 奖品名（成功时上游会给）。 */
  prizeName?: string
}

/** 一份小红花报告。字段名与后端 `RedFlowerReport` 的导出形状对应。 */
export interface LiveRoomReport {
  balanceBefore?: number
  balanceAfter?: number
  /** 本次进账（⑥−①）。后端给了就用后端的。 */
  gained?: number
  draws?: LiveRoomDrawRecord[]
  invalidCodes?: string[]
  expiredCodes?: string[]
  otherCodes?: string[]
  /** 这一趟实际拿去领取的口令（保序）。 */
  codes?: string[]
}

/** 全部"没领"的原因 —— 不同原因**下一步完全不同**，所以是枚举而不是布尔。 */
export type LiveRoomIdleReason =
  /** 获取口令但不领取：收完就结束。 */
  | 'kouling-only'
  /** 今天没有直播（一间都没在播）。 */
  | 'no-live'
  /** 有 N 间在播，但没有一间是「中国移动云盘」。 */
  | 'other-anchor-live'
  /** 门开了但没收到口令。 */
  | 'no-codes'
  /** 说不清（既没有领取记录，也没有可判别的措辞）。 */
  | 'unknown'

/**
 * 读一趟的结果。
 *
 * ⚠⚠ **"没领"不是失败**。直播这条链最常见的结局恰恰是"今天没开播"，
 * 把它染红等于让用户每天看一次假警 —— 与签到里"今天已经签过了"同一档。
 * 所以这里把 `idle` 单独归一类，颜色由页面查表，不在类型里定。
 */
export interface LiveRoomRunView {
  /**
   * 结论：
   * - `idle`     没领（门没开 / 获取口令但不领取 / 没收到口令）—— **正常**；
   * - `success`  领了且全成；
   * - `partial`  领了、成了一部分；
   * - `failed`   领了、一个都没成。
   */
  kind: 'idle' | 'success' | 'partial' | 'failed'
  /** 一句人话（后端原文或本地合成）。 */
  text: string
  /** `idle` 时"为什么没领"；其余为 `null`。 */
  idle: LiveRoomIdleReason | null
  /** 本次进账几朵；读不到时 `null`（**不是 0** —— 0 是一句断言）。 */
  gained: number | null
  /** 终值/起始余额，读不到时 `null`。 */
  balanceAfter: number | null
  balanceBefore: number | null
  /** 五类口令结果，分开列。 */
  succeeded: LiveRoomDrawRecord[]
  redeemed: LiveRoomDrawRecord[]
  expired: LiveRoomDrawRecord[]
  invalid: LiveRoomDrawRecord[]
  other: LiveRoomDrawRecord[]
  /** 实际拿去领取的口令数。 */
  attempted: number
}

/**
 * `TaskOutcome` 的结论文本（与 `ui/history.ts` 同一口径）。
 *
 * 返回 `undefined` 时由调用方兜底 —— 后端三个分支各自带 `summary` / `reason`，
 * 形状对不上时这里会取到 `undefined`。
 */
const outcomeTextOf = (o: TaskOutcome): string | undefined =>
  o.status === 'success' ? o.summary : o.reason

/** 后端小结里"进账 N"那一格 —— 把报告压成一句话时的**唯一**可用来源。 */
const GAINED_IN_TEXT = /进账\s*(-?\d+)/

/** "成功 1/2" 那一格 —— 只取**分子**（成了几条）。 */
const SUCCESS_IN_TEXT = /成功\s*(\d+)\s*\/\s*\d+/

/** "成功 1/2" 那一格 —— 只取**分母**（一共领了几条）。 */
const DRAWN_IN_TEXT = /成功\s*\d+\s*\/\s*(\d+)/

/** 后端 `notes_suffix` 里那三类口令标签 —— **原样**，用于在文本里定位。 */
const EXPIRED_LABEL = '口令已失效（本场已结束）'
const INVALID_LABEL = '无效口令'
const OTHER_LABEL = '未分类口令'

/**
 * 从一句话里读出某个标签后面跟着的那串口令。
 *
 * ⚠⚠ 这里**不能**用"读到 `）` 就停"那种写法：`口令已失效（本场已结束）AAAA`
 * 这个标签**自己就带一对括号**，一读到 `）` 就停会把它截成
 * `（本场已结束` —— 于是界面上"口令"那一栏显示的是一个括号片段，
 * 而真正的口令 `AAAA` 被吞掉。这是我实测踩到的（第一版就是这么写的）。
 *
 * 正确做法是拿**已知的标签与分隔符**当边界：
 * 到下一个 `；`、或者下一个已知标签、或者字符串结尾为止。
 */
const codesAfterLabel = (text: string, label: string): string[] => {
  const at = text.indexOf(label)
  if (at < 0) return []
  let tail = text.slice(at + label.length)

  // 到 `；` 为止（`notes_suffix` 用 `；` 连接三段）
  const semi = tail.indexOf('；')
  if (semi >= 0) tail = tail.slice(0, semi)

  // 到下一个已知标签为止（它们可能不带 `；`，例如最后一段以 `）` 收尾）
  for (const other of [EXPIRED_LABEL, INVALID_LABEL, OTHER_LABEL, '首次参与', '复活花']) {
    if (other === label) continue
    const next = tail.indexOf(other)
    if (next >= 0) tail = tail.slice(0, next)
  }

  // 去掉尾巴上的收尾括号与空白
  tail = tail.replace(/[）)]+\s*$/, '')

  return tail
    .split('/')
    .map((v) => v.trim())
    .filter((v) => v !== '')
}

/**
 * 把一次运行的记录翻成页面要看的视图。
 *
 * # 为什么判据**有一部分**落在文本上
 *
 * 后端今天只把结论压成一句话（`RedFlowerReport::summary_line()`），
 * 结构化字段还没进 `details`。要界面上做到"四类口令分开显示、没领不染红"，
 * 就只能先读那句话 —— 而这**不是猜措辞**：那几句是后端写死的模板
 * （`无效口令 …` / `口令已失效（本场已结束）…` / `未分类口令 …` / `进账 N`），
 * 后端的注释里也明确说了它们是**用户可见的**。
 *
 * `details.liveRoom` 一旦落地则**结构化字段优先**，文本只作兜底 ——
 * 所以这里每一步都是"有结构化的用结构化的，没有才读文本"。
 */
export function toLiveRoomRunView(
  record: RunRecord | null,
  outcome?: TaskOutcome,
): LiveRoomRunView {
  const o = outcome ?? record?.outcome
  // ⚠⚠ `outcomeTextOf` 必须容错。后端的 `failed` / `skipped` 分支带的是 `reason`、
  // `success` 带的是 `summary`；一旦形状对不上（旧后端、字段改名、
  // 或者中间有人转发时丢了字段），`o.summary` 就是 `undefined`，
  // 紧接着 `.match(...)` 抛 TypeError。
  //
  // 这里抛错的**代价特别大**：`onEvent` 的 `finished` 分支在 `finish()` 之前
  // 就断了，于是 `running` 永远为真 —— 页面**永久停在"执行中"**、
  // 日志里没有任何终局行、按钮也不再恢复。用户唯一的出路是刷新页面。
  // 所以兜底成空串，让这一趟至少能正常收尾（结论退化成"这一趟没有拿去领取的口令"）。
  const text = o ? (outcomeTextOf(o) ?? '') : ''
  const details = (record?.details ?? null) as { liveRoom?: LiveRoomReport } | null
  const report = details?.liveRoom ?? null

  const gainedFromText = (() => {
    const m = text.match(GAINED_IN_TEXT)
    return m ? Number(m[1]) : null
  })()
  // 余额那一对从"小红花 235 → 245（进账 10…）"里读
  const pair = text.match(/(\d+)\s*→\s*(\d+)/)
  const balanceBefore = report?.balanceBefore ?? (pair ? Number(pair[1]) : null)
  const balanceAfter = report?.balanceAfter ?? (pair ? Number(pair[2]) : null)
  const gained = report?.gained ?? gainedFromText

  const draws = report?.draws ?? []
  const bucket = (kind: LiveRoomDrawKind): LiveRoomDrawRecord[] =>
    draws.filter((d) => d.kind === kind)

  const succeeded = bucket('succeeded')
  const redeemed = bucket('already_redeemed')

  // ⚠ 这三类是**分开**的，任何一步都不许合并：
  //   `expired`  = 口令对、但本场已结束  → 排查方向「直播结束了」
  //   `invalid`  = 口令不认识（多半抄错）→ 排查方向「口令来源对不对」
  //   `other`    = 上游回了个没分类的码  → 排查方向「上游改了什么」
  const expired =
    report?.expiredCodes != null
      ? report.expiredCodes.map((code) => ({ code, kind: 'kouling_expired' as const }))
      : bucket('kouling_expired')
  const invalid =
    report?.invalidCodes != null
      ? report.invalidCodes.map((code) => ({ code, kind: 'invalid' as const }))
      : bucket('invalid')
  const other =
    report?.otherCodes != null
      ? report.otherCodes.map((code) => ({ code, kind: 'other' as const }))
      : bucket('other')

  // 文本兜底：三类各有自己的固定标签，**按标签分别取**，不混成一个列表。
  const expiredFinal =
    expired.length > 0
      ? expired
      : codesAfterLabel(text, EXPIRED_LABEL).map((code) => ({
          code,
          kind: 'kouling_expired' as const,
        }))
  const invalidFinal =
    invalid.length > 0
      ? invalid
      : codesAfterLabel(text, INVALID_LABEL).map((code) => ({ code, kind: 'invalid' as const }))
  const otherFinal =
    other.length > 0
      ? other
      : codesAfterLabel(text, OTHER_LABEL).map((code) => ({ code, kind: 'other' as const }))

  const attempted = report != null
    ? draws.length
    : (() => {
        const m = text.match(DRAWN_IN_TEXT)
        return m ? Number(m[1]) : 0
      })()

  const base = {
    text,
    gained,
    balanceAfter,
    balanceBefore,
    succeeded,
    redeemed,
    expired: expiredFinal,
    invalid: invalidFinal,
    other: otherFinal,
    attempted,
  }

  // ── 结论分类 ──
  //
  // ⚠ 顺序要紧。**先认"没领"**，再认失败与成功：
  // 后端在"没领"时回的是 **success**（见 `LiveRoomAccountResult::outcome`），
  // 所以"成功=绿"这条映射对这条链是**错的**，光看 `outcome.status` 会把
  // "今天没开播"显示成一个绿色的成功。
  const idle: LiveRoomIdleReason | null = text.includes('只获取不领取')
    ? 'kouling-only'
    : text.includes('今天没有直播')
      ? 'no-live'
      : /有\s*\d+\s*间在播/.test(text)
        ? 'other-anchor-live'
        : text.includes('没收到口令')
          ? 'no-codes'
          : null

  if (o?.status === 'skipped') {
    return { kind: 'idle', idle: 'unknown', ...base }
  }
  if (idle != null) {
    return { kind: 'idle', idle, ...base }
  }
  if (o?.status === 'failed') {
    return { kind: 'failed', idle: null, ...base }
  }
  if (attempted === 0) {
    // 既没有"没领"的措辞、也没有领取记录：说不清。归 idle 而**不是** failed ——
    // 判成失败会制造一条假警，而"没领"才是这条链的常态。
    return { kind: 'idle', idle: 'unknown', ...base }
  }

  /**
   * 成了几条。
   *
   * ⚠⚠ 必须**优先读文本里那个"成功 s/t"**，而不是只看结构化的 `succeeded`。
   *
   * 后端今天把报告压成一句话，`details.liveRoom` 还没落地 —— 那时
   * `succeeded` 是**空数组**，而空数组不等于"零条成功"。
   * 只看它的话，一句 `成功 1/3` 会被判成 `failed`（0 条成功），
   * 于是界面上写着「失败」、旁边却写着「进账 +15 朵」——
   * 自相矛盾，而且把一次**部分成功**误报成失败。这是我实测踩到的。
   */
  const successFromText = (() => {
    const m = text.match(SUCCESS_IN_TEXT)
    return m ? Number(m[1]) : null
  })()
  const okCount =
    successFromText != null
      ? successFromText
      : succeeded.length > 0
        ? succeeded.length
        : redeemed.length

  // 结构化字段没来时，只能靠后端的固定尾巴判「部分成功 / 一个都没成」。
  if (report == null) {
    if (text.includes('一个口令都没成')) return { kind: 'failed', idle: null, ...base }
    if (text.includes('部分成功')) return { kind: 'partial', idle: null, ...base }
  }
  if (okCount === 0 && attempted > 0) return { kind: 'failed', idle: null, ...base }
  if (okCount < attempted) return { kind: 'partial', idle: null, ...base }
  return { kind: 'success', idle: null, ...base }
}

/** "为什么没领"的说法 —— 查表，**不拼字符串**。 */
export const LIVE_IDLE_REASON: Record<LiveRoomIdleReason, string> = {
  'kouling-only': '获取口令但不领取 —— 口令收完就结束，这一趟一个口令都没拿去领',
  'no-live': '今天没有直播 —— 该主播一间都没在播，没有口令可收（等下一场即可，不是失败）',
  'other-anchor-live':
    '有别的房间在播，但没有一间是「中国移动云盘」—— 这一场不会再有口令了，不用等',
  'no-codes': '门开了，但这一趟没收到口令 —— 弹幕里没人发，或小红书那条还没贴进来',
  unknown: '这一趟没有拿去领取的口令',
}

// ── 兑换（抢兑）──────────────────────────────────────────────────────
//
// ⚠ 这一族与上面的直播/签到**不是**同一条链：清单与取题是只读的，
// 而**兑换是真花豆的写操作** —— 界面必须二次确认，且不可兑的档位要挡住。

/** 清单里的一件奖品（后端按判据算好档位再发过来）。 */
export interface ExchangePrizeView {
  prizeId: number
  name: string | null
  /** 兑换所需AI豆数。`null` = 上游没给这一格（不是 0）。 */
  price: number | null
  groupId: number | null
  /**
   * 判据档位（`plan_exchange` 的 slug）。
   *
   * ⚠ 只读那条路由用的是**无余额**那一档（`balanceAware: false`），
   * 所以它判不出「余额不足」—— 见 `ExchangePrizesResponse.balanceNote`。
   */
  plan: string
  /** 为什么不能兑（判据本身的中文复述）。`blocks` 为真时才有意义。 */
  planReason: string
  /** 判据没过 ⇒ 界面上的按钮要 disabled。 */
  blocks: boolean
  limit: number | null
  count: number | null
  /**
   * 上游 `totalCount`：这一场活动的**总投放量**。
   *
   * ⚠ 与 `count`（当前剩余）配起来才是"现量/总量"。**别**把 `count` 单独叫"现量"
   * 再和"已抢光"并排显示：2026-10-01 实拍 `251230032` 是 `count = 43,739` 而
   * `minRemainderCount = 0`（H5 判抢光）—— 池子里还有货、**今日份额**没了。
   */
  totalCount: number | null
  /** 上游 `dailyCount`：**今日投放量**（配 `dailyRemainderCount` = 今日还剩几份）。 */
  dailyCount: number | null
  dailyRemainderCount: number | null
  minRemainderCount: number | null
  onLine: number | null
  startTime: string | null
  endTime: string | null
  /**
   * 这一件所属**活动/品类组**的名字，取自另一条静态接口 `prizeGroup.json`。
   *
   * 组是"共享状态"的单位（同组每月共用一个名额 ⇒ `limit == 4`），
   * 而组名是**上游给的**（实拍"音乐类会员""视频类会员"）—— 界面直接显示，别自己编。
   * `null` = 上游没给或那一枪没取到（这时分区标题退回"第 N 组"）。
   */
  groupTitle: string | null
  /** 该组的限兑规则，**上游原话**（实拍 `(月卡每月多选一限兑)`）。 */
  groupSubTitle: string | null
  /**
   * 上游 `quota.monthQuantity`：这个组**每月能兑几件**（实拍音乐类是 `1`）。
   *
   * ⚠ 只用于显示：判据链不读它，上游也没给"本组已用几件"，别拿它算"还剩几个名额"。
   */
  monthQuantity: number | null
  /** 上游 `quota.groupPrizeIds`：与这一件**互斥**的其它 prizeId。`null` = 上游没给。 */
  groupPrizeIds: string[] | null
}

export interface ExchangePrizesResponse {
  prizes: ExchangePrizeView[]
  /**
   * 这一份判据是否把**余额**算进去了。
   *
   * 今天恒为 `false`：清单那条路由**不查** `infoV3`（账号级请求，
   * 多打一枪多一分风控噪声），所以档位里没有「余额不足」这一档。
   */
  balanceAware: boolean
  /** 上面那件事的人话说明，界面上直接显示，别自己编。 */
  balanceNote: string
  /**
   * 组名那一枪（`prizeGroup.json`）**拿到了没有**；`null` = 拿到了。
   *
   * `string` 时清单照常显示，只是分区标题退回"第 N 组" —— 组名是文案，
   * 不该因为它读失败就让整份清单失败。原因原样显示（别在前端另写一句）。
   */
  groupsNote: string | null
  /** 后端判据用的时刻（界面要显示"这份清单是什么时候的"）。 */
  nowMs: number
}

/** 读奖品清单 + 每件的判据档位。**只读，不花豆。** */
export function listExchangePrizes(account: string): Promise<ExchangePrizesResponse> {
  return request<ExchangePrizesResponse>(
    `/api/accounts/${encodeURIComponent(account)}/exchange-prizes`,
  )
}

/** 一次兑换的读数（后端从运行记录里读回来的那一份）。 */
export interface ExchangeRunOutcome {
  runId: number
  /** `ordered` / `already-exchanged` / `skipped` / `unknown` / … */
  status: string
  plan: string
  planReason: string
  /** 给人读的一句话小结（后端定的，界面直接用）。 */
  summary: string
  /** 业务码。`0` = 成交。 */
  code: number
  /** 上游那句 `msg`（可能是空串）。 */
  message: string
  rounds: number
  /** 真正发出去了几枪**下单**请求。`0` = 一枪未发。 */
  attempts: number
  /**
   * 本地解出的 `puzzleOffset`。
   *
   * `null` = 没解出来（那时 `attempts` 必为 0：算不出就不发）。
   */
  offset: number | null
  balance: number
  afterBalance: number | null
  order: Record<string, unknown> | null
  deviceIdFailed: string
  error: string
}

export interface ExchangeRequestInput {
  /** `prizeId` 的数字串，或奖品名（后端按精确相等匹配）。 */
  prizeIdOrName: string
  /**
   * 跳过"等到下一个整点再重判"。
   *
   * 界面默认 `true`：用户点了按钮就期望**现在**有结果，
   * 让 HTTP 请求挂在那儿等一小时是不可接受的。
   */
  skipWait?: boolean
}

/**
 * 兑一件奖品。**真花豆。**
 *
 * 后端会走并发闸（同一 `task:account` 不会并发），所以连点两下
 * 第二下会拿到「同一任务正在运行中」而不是扣两次豆。
 */
export function runExchange(account: string, input: ExchangeRequestInput): Promise<ExchangeRunOutcome> {
  return request<ExchangeRunOutcome>(`/api/accounts/${encodeURIComponent(account)}/exchange`, {
    method: 'POST',
    body: JSON.stringify({
      prizeIdOrName: input.prizeIdOrName,
      skipWait: input.skipWait ?? true,
    }),
  })
}
