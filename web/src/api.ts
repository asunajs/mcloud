/**
 * 后端 REST 与 SSE 封装。
 *
 * 所有网络访问集中在这里，页面组件不直接 fetch —— 便于统一处理
 * 错误形态、脱敏提示与类型。
 */

export interface TaskMeta {
  name: string
  title: string
  description: string
  hidden: boolean
  params: string[]
}

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
  | { type: 'log'; run_id: number; level: 'debug' | 'info' | 'warn' | 'error'; message: string; at_ms: number }
  | { type: 'progress'; run_id: number; step: number; total: number; label: string }
  | { type: 'finished'; run_id: number; outcome: TaskOutcome; duration_ms: number }

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(path, {
    headers: { 'content-type': 'application/json' },
    ...init,
  })
  const text = await resp.text()
  let data: unknown = null
  if (text) {
    try {
      data = JSON.parse(text)
    } catch {
      // 后端异常时可能返回非 JSON，保留原文便于排查
      throw new Error(`响应不是 JSON（HTTP ${resp.status}）：${text.slice(0, 200)}`)
    }
  }
  if (!resp.ok) {
    const msg =
      data && typeof data === 'object' && 'error' in data
        ? String((data as { error: unknown }).error)
        : `HTTP ${resp.status}`
    throw new Error(msg)
  }
  return data as T
}

export const api = {
  version: () => request<{ name: string; version: string }>('/api/version'),
  capabilities: () => request<{ tasks: TaskMeta[] }>('/api/capabilities'),
  runs: () => request<RunRecord[]>('/api/runs'),
  run: (task: string, account: string, body: Record<string, unknown> = {}) =>
    request<RunResponse>('/api/runs', {
      method: 'POST',
      body: JSON.stringify({ task, account, body }),
    }),
}

/**
 * 订阅某次运行的事件流。
 *
 * 返回取消函数；调用方**必须**在组件卸载时调用它，否则 EventSource
 * 会一直挂着，连接数持续增长。
 */
export function subscribeRun(
  runId: number,
  handlers: {
    onEvent?: (ev: RunEvent) => void
    onError?: (err: Event) => void
  } = {},
): () => void {
  const es = new EventSource(`/api/runs/${runId}/events`)

  const handle = (raw: MessageEvent) => {
    try {
      handlers.onEvent?.(JSON.parse(raw.data) as RunEvent)
    } catch {
      // 单条事件解析失败不应中断整条流
    }
  }

  // 后端用 event: 名区分类型，需分别注册
  for (const name of ['started', 'log', 'progress', 'finished'] as const) {
    es.addEventListener(name, handle as EventListener)
  }
  if (handlers.onError) es.onerror = handlers.onError

  return () => es.close()
}
