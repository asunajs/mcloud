/**
 * 任务中心：选账号 → 运行任务 → 实时看日志。
 *
 * 移动端形态（规范 §7.2）：参数表单全宽，卡片流。
 *
 * 长任务走 SSE 实时日志；提交期间只禁用提交按钮，**页面其余部分保持可操作**，
 * 并明确告诉用户可以关页面（任务在后端继续跑）。
 */

import { onUnmounted, onMounted, ref } from 'vue'
import {
  cancelRun,
  listCapabilities,
  startRun,
  subscribeRun,
  type RunEvent,
  type TaskMeta,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { LogPanel, type LogLine } from '../ui/log-panel'

type LoadState = 'loading' | 'error' | 'ready'

export const TasksPage = () => {
  const state = ref<LoadState>('loading')
  /** 加载失败（整页替换）。 */
  const errorMessage = ref('')
  /** 动作失败（表单旁内联）。与上面分开，否则动作失败时整页什么都不显示。 */
  const submitError = ref('')

  const tasks = ref<TaskMeta[]>([])
  const account = ref('')
  const running = ref(false)
  const lines = ref<LogLine[]>([])
  /** 已用时长，1 秒一跳；用户要能看到"还在动"。 */
  const elapsedMs = ref(0)
  /** 当前运行的 run id，用于请求中止。 */
  const currentRunId = ref<number | null>(null)
  /** 已发出中止请求，等待任务在安全点退出。 */
  const cancelling = ref(false)

  let stopStream: (() => void) | null = null
  let ticker: number | null = null
  let startedAt = 0

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const r = await listCapabilities()
      tasks.value = r.tasks.filter((t) => !t.hidden)
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

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
   * 所以这里不立刻把界面改成"已停止"，而是显示"正在中止"，
   * 等真正的 finished 事件到达才收尾 —— 否则界面说停了、任务还在跑。
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
      submitError.value = e instanceof Error ? e.message : '中止请求失败'
      cancelling.value = false
    }
  }

  const onEvent = (ev: RunEvent) => {
    switch (ev.type) {
      case 'started':
        push('info', `任务开始：${ev.task} / ${ev.account}`)
        break
      case 'log':
        push(
          ev.level === 'error' ? 'error' : ev.level === 'warn' ? 'warn' : 'info',
          ev.message,
        )
        break
      case 'progress':
        push('info', `[${ev.step}/${ev.total}] ${ev.label}`)
        break
      case 'finished':
        push(
          ev.outcome.status === 'failed' ? 'error' : 'info',
          `结束（${ev.duration_ms}ms）：${
            ev.outcome.status === 'success' ? ev.outcome.summary : ev.outcome.reason
          }`,
        )
        finish()
        break
      default:
        // 后端新增事件类型时，旧前端不该整页报错 —— 忽略即可
        break
    }
  }

  const run = async (task: string) => {
    submitError.value = ''
    lines.value = []
    if (!account.value.trim()) {
      submitError.value = '请先填写账号（手机号）'
      return
    }

    running.value = true
    startedAt = Date.now()
    elapsedMs.value = 0
    ticker = window.setInterval(() => {
      elapsedMs.value = Date.now() - startedAt
    }, 1000)

    try {
      // 后台执行：**立刻**拿到 run id，这样才能订阅事件并请求中止。
      // 同步执行的话请求还没返回，用户根本没有可中止的时机。
      const resp = await startRun(task, account.value.trim(), {}, true)
      currentRunId.value = resp.run_id
      // 先订阅再提示：订阅会把已发布的事件补进队列，
      // 因此"启动到订阅"之间的事件不会丢
      stopStream = subscribeRun(resp.run_id, { onEvent })
      push('info', `已提交，运行号 ${resp.run_id}`)
    } catch (e) {
      submitError.value = e instanceof Error ? e.message : '请求失败'
      finish()
    }
  }

  onMounted(() => void load())
  onUnmounted(() => {
    stopTicker()
    stopStream?.()
  })

  const elapsedText = () => `${(elapsedMs.value / 1000).toFixed(0)} 秒`

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">任务中心</h1>
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

  const form = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <h2 class="card-title text-base">运行参数</h2>
        <fieldset class="fieldset">
          <legend class="fieldset-legend">账号（手机号）</legend>
          <input
            class="input w-full"
            placeholder="例如 13800000000"
            value={account.value}
            // 用 onInput 而不是 onChange：Vue 的 onChange 是原生 change，只在失焦时触发
            onInput={(e) => {
              account.value = (e.target as HTMLInputElement).value
            }}
          />
        </fieldset>
        {submitError.value ? (
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">{submitError.value}</span>
          </div>
        ) : null}
        {running.value ? (
          <div class="flex flex-wrap items-center justify-between gap-2">
            <span class="text-xs opacity-70">
              执行中，已用 {elapsedText()} · 可关闭页面，任务在后端继续
            </span>
            <button
              type="button"
              class="btn btn-error btn-sm min-h-11 md:min-h-0"
              disabled={cancelling.value}
              onClick={() => void cancel()}
            >
              {cancelling.value ? '正在中止…' : '中止'}
            </button>
          </div>
        ) : null}
      </div>
    </div>
  )

  const taskCards = () =>
    tasks.value.length === 0 ? (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <EmptyState
            title="后端还没有注册任何任务。"
            hint="任务由后端注册表提供，检查后端是否正常启动，或稍后重试。"
            action={
              <button type="button" class="btn btn-soft" onClick={() => void load()}>
                重试
              </button>
            }
          />
        </div>
      </div>
    ) : (
      <div class="grid gap-3 md:grid-cols-2 lg:grid-cols-3">
        {tasks.value.map((t) => (
          <div key={t.name} class="card bg-base-100 shadow-sm">
            <div class="card-body">
              <h3 class="card-title text-base">{t.title}</h3>
              <p class="break-words text-sm opacity-70">{t.description}</p>
              <p class="text-xs opacity-50">标识：{t.name}</p>
              <div class="card-actions justify-end">
                <button
                  type="button"
                  class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                  disabled={running.value}
                  onClick={() => void run(t.name)}
                >
                  {running.value ? '执行中…' : '运行'}
                </button>
              </div>
            </div>
          </div>
        ))}
      </div>
    )

  return (
    <div class="flex flex-col gap-4">
      {header()}

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
          {taskCards()}
          {lines.value.length > 0 ? <LogPanel lines={lines.value} /> : null}
        </>
      ) : null}
    </div>
  )
}
