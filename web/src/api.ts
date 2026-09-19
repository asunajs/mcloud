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

/** 当前身份；未登录时抛 `ApiError`（code = `unauthorized`）。 */
export const currentSession = () => request<SessionInfo>('/api/session')

/** 登出。 */
export const signOut = () => request<void>('/api/session', { method: 'DELETE' })
