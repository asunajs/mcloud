/**
 * 总览页：后端信息 + 最近运行。
 *
 * 移动端形态（规范 §7.2）：**单列卡片流**（`grid gap-3`），指标卡竖排。
 */

import { onMounted, ref } from 'vue'
import { getStatus, listRuns, type RunRecord, type ServiceStatus } from '../api'
import { StatRow, type StatItem } from '../ui/stat-row'
import { EmptyState } from '../ui/empty-state'
import { fmtDuration, fmtEpochMs } from '../ui/time'
import { outcomeText, statusBadge, statusLabel } from '../ui/history'
import type { PageId } from '../ui/nav'

export interface DashboardPageProps {
  onGo: (id: PageId) => void
}

/** 三态显式声明：加载中 / 出错 / 就绪。 */
type LoadState = 'loading' | 'error' | 'ready'

export const DashboardPage = (props: DashboardPageProps) => {
  const state = ref<LoadState>('loading')
  /** 加载失败（整页替换）。与动作失败分开 —— 共用一个 ref 会让动作失败什么都不显示。 */
  const errorMessage = ref('')
  const status = ref<ServiceStatus | null>(null)
  const runs = ref<RunRecord[]>([])

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [s, r] = await Promise.all([getStatus(), listRuns()])
      status.value = s
      runs.value = r
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => void load())

  const stats = (): StatItem[] => {
    if (!status.value) return []
    const items: StatItem[] = [
      { title: '已注册任务', value: String(status.value.taskCount) },
      { title: '执行记录', value: String(runs.value.length) },
    ]
    if (status.value.lastRun) {
      items.push({
        title: '最近一次',
        value: statusLabel(status.value.lastRun.outcome.status),
        desc: fmtEpochMs(status.value.lastRun.started_at_ms),
      })
    }
    return items
  }

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">总览</h1>
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

  const recentList = () => {
    if (runs.value.length === 0) {
      return (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="还没有任何执行记录。"
              hint="先去任务中心挑一个任务跑一次，结果会出现在这里。"
              action={
                <button
                  type="button"
                  class="btn btn-primary"
                  onClick={() => props.onGo('tasks')}
                >
                  去任务中心
                </button>
              }
            />
          </div>
        </div>
      )
    }

    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <h2 class="card-title text-base">最近运行</h2>

          {/* 桌面：表格 ≥768px */}
          <div class="hidden overflow-x-auto rounded-box bg-base-100 md:block">
            <table class="table table-sm">
              <thead>
                <tr>
                  <th>任务</th>
                  <th>账号</th>
                  <th>结果</th>
                  <th>耗时</th>
                  <th>时间</th>
                </tr>
              </thead>
              <tbody>
                {runs.value.map((r) => (
                  <tr key={r.run_id}>
                    <td>{r.task}</td>
                    <td class="break-words">{r.account}</td>
                    <td>
                      <span class={`badge ${statusBadge(r.outcome.status)}`}>
                        {outcomeText(r.outcome)}
                      </span>
                    </td>
                    <td>{fmtDuration(r.duration_ms)}</td>
                    <td>{fmtEpochMs(r.started_at_ms)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* 移动：卡片流 <768px。切卡片后**每个值都要带标签** */}
          <div class="grid gap-3 md:hidden">
            {runs.value.map((r) => (
              <div key={r.run_id} class="rounded-box bg-base-200 p-3">
                <div class="flex items-center justify-between gap-2">
                  <span class="font-medium">{r.task}</span>
                  <span class={`badge ${statusBadge(r.outcome.status)}`}>
                    {statusLabel(r.outcome.status)}
                  </span>
                </div>
                <p class="mt-1 break-words text-xs opacity-70">账号 {r.account}</p>
                <p class="break-words text-xs opacity-70">{outcomeText(r.outcome)}</p>
                <p class="mt-1 text-xs opacity-60">
                  耗时 {fmtDuration(r.duration_ms)} · {fmtEpochMs(r.started_at_ms)}
                </p>
              </div>
            ))}
          </div>
        </div>
      </div>
    )
  }

  return (
    <div class="flex flex-col gap-4">
      {header()}

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-lg text-primary" />
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
          <StatRow items={stats()} />
          {recentList()}
        </>
      ) : null}
    </div>
  )
}
