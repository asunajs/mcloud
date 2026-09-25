import { onMounted, ref } from 'vue'
import { api, subscribeRun, type RunEvent, type TaskMeta } from '../api'
import { RunLog, type LogLine } from '../components/run-log'

/**
 * 任务页：列出可用任务，填写账号后运行，并实时展示事件流。
 */
export function Tasks() {
  const tasks = ref<TaskMeta[]>([])
  const account = ref('')
  const loading = ref(false)
  const error = ref('')
  const lines = ref<LogLine[]>([])
  const running = ref(false)

  async function load() {
    try {
      const r = await api.capabilities()
      tasks.value = r.tasks
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    }
  }

  function push(kind: LogLine['kind'], text: string) {
    lines.value = [...lines.value, { kind, text, at: Date.now() }]
  }

  function onEvent(ev: RunEvent) {
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
      case 'finished':
        push(
          ev.outcome.status === 'failed' ? 'error' : 'info',
          `结束（${ev.duration_ms}ms）：${
            ev.outcome.status === 'success' ? ev.outcome.summary : ev.outcome.reason
          }`,
        )
        running.value = false
        break
    }
  }

  async function run(task: string) {
    error.value = ''
    lines.value = []
    if (!account.value.trim()) {
      error.value = '请先填写账号'
      return
    }
    loading.value = true
    running.value = true
    try {
      const resp = await api.run(task, account.value.trim())
      // 先订阅再等结果：事件是同步产生的，订阅晚一步会漏掉
      const stop = subscribeRun(resp.run_id, { onEvent })
      onEvent({
        type: 'finished',
        run_id: resp.run_id,
        outcome: resp.outcome,
        duration_ms: resp.duration_ms,
      })
      setTimeout(stop, 500)
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
      running.value = false
    } finally {
      loading.value = false
    }
  }

  onMounted(load)

  return (
    <div class="space-y-4">
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <h2 class="card-title text-base">运行任务</h2>
          <div class="flex flex-wrap items-end gap-3">
            <label class="form-control">
              <span class="label-text">账号</span>
              <input
                class="input input-bordered input-sm w-56"
                placeholder="手机号"
                value={account.value}
                onInput={(e) => {
                  account.value = (e.target as HTMLInputElement).value
                }}
              />
            </label>
          </div>
          {error.value && <div class="alert alert-error text-sm py-2">{error.value}</div>}
        </div>
      </div>

      <div class="grid gap-3 md:grid-cols-2 lg:grid-cols-3">
        {tasks.value.map((t) => (
          <div key={t.name} class="card bg-base-100 shadow-sm">
            <div class="card-body py-4">
              <h3 class="font-medium">
                {t.title}
                {t.hidden && <span class="badge badge-ghost badge-sm ml-2">隐藏</span>}
              </h3>
              <p class="text-sm opacity-70">{t.description}</p>
              <div class="card-actions justify-end">
                <button
                  class="btn btn-primary btn-sm"
                  disabled={loading.value || running.value}
                  onClick={() => run(t.name)}
                >
                  运行
                </button>
              </div>
            </div>
          </div>
        ))}
      </div>

      <RunLog lines={lines.value} />
    </div>
  )
}
