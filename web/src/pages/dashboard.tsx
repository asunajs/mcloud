import { onMounted, ref } from 'vue'
import { api, type RunRecord } from '../api'

/** 概览页：后端版本与最近运行记录。 */
export function Dashboard() {
  const version = ref('')
  const runs = ref<RunRecord[]>([])
  const error = ref('')

  onMounted(async () => {
    try {
      const v = await api.version()
      version.value = `${v.name} ${v.version}`
      runs.value = await api.runs()
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    }
  })

  return (
    <div class="space-y-4">
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body py-4">
          <div class="text-sm opacity-70">后端</div>
          <div class="text-lg font-medium">{version.value || '连接中…'}</div>
        </div>
      </div>

      {error.value && <div class="alert alert-error text-sm">{error.value}</div>}

      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <h2 class="card-title text-base">最近运行</h2>
          {runs.value.length === 0 ? (
            <p class="text-sm opacity-60">暂无记录</p>
          ) : (
            <div class="overflow-x-auto">
              <table class="table table-sm">
                <thead>
                  <tr>
                    <th>任务</th>
                    <th>账号</th>
                    <th>结果</th>
                    <th>耗时</th>
                  </tr>
                </thead>
                <tbody>
                  {runs.value.map((r) => (
                    <tr key={r.run_id}>
                      <td>{r.task}</td>
                      <td>{r.account}</td>
                      <td>
                        <span
                          class={
                            'badge badge-sm ' +
                            (r.outcome.status === 'success'
                              ? 'badge-success'
                              : r.outcome.status === 'failed'
                                ? 'badge-error'
                                : 'badge-ghost')
                          }
                        >
                          {r.outcome.status === 'success' ? r.outcome.summary : r.outcome.reason}
                        </span>
                      </td>
                      <td>{r.duration_ms}ms</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
