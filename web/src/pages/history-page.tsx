/**
 * 历史与日志页。
 *
 * 移动端形态（规范 §7.2）：**卡片流**，每条带标签。
 * 桌面用表格，窄屏**必须**切卡片 —— 列多的表在窄屏不可用。
 */

import { onMounted, ref } from 'vue'
import { listRuns, type RunRecord } from '../api'
import { EmptyState } from '../ui/empty-state'
import { fmtDuration, fmtEpochMs } from '../ui/time'
import { outcomeText, statusBadge, statusLabel } from '../ui/history'
import type { PageId } from '../ui/nav'

export interface HistoryPageProps {
  onGo: (id: PageId) => void
}

type LoadState = 'loading' | 'error' | 'ready'

/**
 * 一行"这次运行在用户网盘里留下了什么"。
 *
 * # 为什么值得单独占一列
 *
 * 旧项目只在小结里念一句"网盘里留了 N 个待清理文件"，却**不说是哪 N 个** ——
 * 用户想去手工清理时无从下手。所以这里把 `fileId` 直接给出来：
 * 它是清理那一枪唯一认的标识。
 *
 * # 为什么空的时候什么都不显示
 *
 * 绝大多数运行是干净的（链尾自己收尾）。给每一行都挂一个"无"字
 * 只会把真正有遗留的那几行淹掉 —— 而这一列的全部价值就是让人**一眼看见**
 * 那几行。
 */
const LeftoverCell = (props: { record: RunRecord }) => {
  const items = props.record.details?.artifacts ?? []
  if (items.length === 0) return null

  return (
    <div class="flex flex-col gap-1">
      <span class="badge badge-warning badge-sm">遗留 {items.length} 个文件</span>
      <ul class="flex flex-col gap-0.5">
        {items.map((a) => (
          // 给出账号与 ID：清理要用该账号的凭据按 ID 删
          <li key={`${a.account}/${a.id}`} class="font-mono text-xs opacity-70">
            {a.account} · {a.id}
          </li>
        ))}
      </ul>
    </div>
  )
}

export const HistoryPage = (props: HistoryPageProps) => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  const runs = ref<RunRecord[]>([])

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      runs.value = await listRuns()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => void load())

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">历史与日志</h1>
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

  const body = () => {
    if (runs.value.length === 0) {
      return (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="还没有任何执行记录。"
              hint="去任务中心跑一次任务，结果会按时间倒序出现在这里。"
              action={
                <button type="button" class="btn btn-primary" onClick={() => props.onGo('tasks')}>
                  去任务中心
                </button>
              }
            />
          </div>
        </div>
      )
    }

    return (
      <>
        {/* 桌面表格 ≥768px */}
        <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
          <table class="table table-sm">
            <thead>
              <tr>
                <th>运行号</th>
                <th>任务</th>
                <th>账号</th>
                <th>结果</th>
                <th>耗时</th>
                <th>时间</th>
                <th>网盘遗留</th>
              </tr>
            </thead>
            <tbody>
              {runs.value.map((r) => (
                <tr key={r.run_id}>
                  <td>{r.run_id}</td>
                  <td>{r.task}</td>
                  <td class="break-words">{r.account}</td>
                  <td>
                    <span class={`badge ${statusBadge(r.outcome.status)}`}>
                      {outcomeText(r.outcome)}
                    </span>
                  </td>
                  <td>{fmtDuration(r.duration_ms)}</td>
                  <td>{fmtEpochMs(r.started_at_ms)}</td>
                  <td>
                    <LeftoverCell record={r} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {/* 移动卡片流 <768px —— 每项带标签 */}
        <div class="grid gap-3 md:hidden">
          {runs.value.map((r) => (
            <div key={r.run_id} class="card bg-base-100 shadow-sm">
              <div class="card-body p-3">
                <div class="flex items-center justify-between gap-2">
                  <span class="font-medium">{r.task}</span>
                  <span class={`badge ${statusBadge(r.outcome.status)}`}>
                    {statusLabel(r.outcome.status)}
                  </span>
                </div>
                <p class="break-words text-xs opacity-70">账号 {r.account}</p>
                <p class="break-words text-xs opacity-70">{outcomeText(r.outcome)}</p>
                <p class="text-xs opacity-60">
                  运行号 {r.run_id} · 耗时 {fmtDuration(r.duration_ms)}
                </p>
                <p class="text-xs opacity-60">时间 {fmtEpochMs(r.started_at_ms)}</p>
                <LeftoverCell record={r} />
              </div>
            </div>
          ))}
        </div>
      </>
    )
  }

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

      {state.value === 'ready' ? body() : null}
    </div>
  )
}
