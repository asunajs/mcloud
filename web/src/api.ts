/**
 * 与后端的最小 API 客户端。
 *
 * 只做三件事：同源 cookie、JSON 形状、把后端的 `{code, message}` 变成可显示的文案。
 * 类型在 P5 会由 `openapi.json` 生成（方案 §5），届时这里的手写类型全部替换 ——
 * 这也是为什么此处**不**下功夫写复杂抽象：它注定要被生成物取代。
 */

/** 后端错误响应的形状（见 `mcloud-server/src/error.rs`）。 */
export interface ApiErrorBody {
  code: string
  message: string
}

/** 接口调用失败：带上后端的 code，调用方可以据此分支。 */
export class ApiError extends Error {
  readonly code: string
  readonly status: number

  constructor(status: number, body: ApiErrorBody) {
    // 后端 4xx 的文案是写给人看的，直接透出；5xx 只会是类别文案
    super(body.message || '请求失败')
    this.name = 'ApiError'
    this.status = status
    this.code = body.code || 'unknown'
  }
}

export interface User {
  id: string
  name: string
  role: string
}

export interface SetupStatus {
  needsSetup: boolean
  /** 口令长度下限。**由后端给**，前端不硬编 —— 否则会漂移成"前端说可以、后端拒绝"。 */
  minPasswordLen: number
}

export interface SessionInfo {
  user: User
  expiresAt?: string
}

export interface SetupResult {
  user: User
  claimed: { accounts: number; schedules: number; pipelines: number }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    // 同源请求浏览器本来就会带 cookie，写出来是为了让"会话靠 cookie 携带"这件事显式
    credentials: 'same-origin',
    headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
    ...init,
  })

  if (!response.ok) {
    let body: ApiErrorBody = { code: 'unknown', message: `HTTP ${response.status}` }
    try {
      const parsed = (await response.json()) as Partial<ApiErrorBody>
      body = { code: parsed.code ?? body.code, message: parsed.message ?? body.message }
    } catch {
      // 不是 JSON（比如反代返回了 HTML 错误页）—— 保留上面那句兜底文案
    }
    throw new ApiError(response.status, body)
  }

  if (response.status === 204) {
    return undefined as T
  }
  return (await response.json()) as T
}

/** 是否需要初始化首个管理员。 */
export const setupStatus = () => request<SetupStatus>('/api/setup')

/** 建首个管理员（成功后即处于已登录状态）。 */
export const runSetup = (name: string, password: string) =>
  request<SetupResult>('/api/setup', { method: 'POST', body: JSON.stringify({ name, password }) })

/** 登录。 */
export const signIn = (name: string, password: string) =>
  request<SessionInfo>('/api/session', { method: 'POST', body: JSON.stringify({ name, password }) })

/** 云盘账号一行（对位后端 `routes.go` 的 `accountPayload`）。 */
export interface Account {
  id: string
  /** 展示名（昵称 → 脱敏手机号 → id），**由后端挑好**，前端不再自己选。 */
  displayName: string
  nickname: string | null
  phoneMasked: string | null
  /** `pc` / `mobile`；旧导入的数据可能是 null。 */
  platform: string | null
  enabled: boolean
  sortOrder: number
  cloudCurrent: number | null
  cloudUpdatedAt: string | null
  /** 令牌到期时间（epoch 毫秒）。 */
  authExpire: number
  /** "是否过期"由**服务端时钟**给结论，前端不拿本地时钟再比一次（§5.9 同理）。 */
  authExpired: boolean
  lastOkAt: string | null
  failStreak: number
  note: string | null
}

export const listAccounts = () => request<{ accounts: Account[] }>('/api/accounts')

/** 一天的云朵快照（差值由采集侧算好落库，见后端 `cloudStats` 注释）。 */
export interface CloudStatRow {
  /** `YYYY-MM-DD`，按**服务端本地时区**切日。 */
  date: string
  cloudCount: number
  cloudDiff: number
  cloudDiffWeek: number
}

/** 某个账号最近 `days` 天的云朵日快照。id 里有 `*` 等字符，必须 encode。 */
export const getCloudStats = (id: string, days = 7) =>
  request<{ stats: CloudStatRow[] }>(
    `/api/accounts/${encodeURIComponent(id)}/cloud-stats?days=${days}`,
  )

/** 当前身份；未登录时抛 `ApiError`（code = `unauthorized`）。 */
export const currentSession = () => request<SessionInfo>('/api/session')

/** 登出。 */
export const signOut = () => request<void>('/api/session', { method: 'DELETE' })

/**
 * 后端自述（对位 `routes.go` 的 `getStatus`）。侧栏底部那一行用它。
 *
 * ⚠ `lastRun.status` 是**存储层的生命周期**（running / done / failed），
 * 不等于业务结论 —— `done` 可以是"全成 / 部分成 / 需人工"三种。
 * 所以那一行只把它当**文字**显示，不拿它决定红绿（后端 `runPayload` 同一条警告）。
 */
export interface ServiceStatus {
  name: string
  version: string
  /** 调度循环在不在跑。**当前后端恒报 false**：定时任务存得下来但不会自己触发。 */
  schedulerRunning: boolean
  /** 最近一次执行；库里一条记录都没有时为 null。 */
  lastRun: {
    id: string
    capability: string
    status: string
    startedAt: string
    finishedAt: string | null
  } | null
}

/** 拉一次后端自述。这条请求成功与否本身就是"连没连上后端"的判据。 */
export const serviceStatus = () => request<ServiceStatus>('/api/status')
