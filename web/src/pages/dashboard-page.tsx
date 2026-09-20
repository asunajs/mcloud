/**
 * 总览页。
 *
 * **内容仍是 P0 spike 的占位数据** —— 真接口在 P6 接。这里刻意把占位数据标出来，
 * 免得哪天被当成已实现。
 *
 * 结构（设计指南 §5.1、§5.2、§7.2）：
 *  ① `stats` 指标卡一排 —— 移动端竖排、桌面横排；
 *  ② 账号列表 —— 桌面 `table`、移动端卡片流（同一个数据源，不写两套逻辑）。
 */

import { computed, ref } from 'vue'
import type { User } from '../api'
import type { PageId } from '../ui/nav'

interface Account {
  id: string
  name: string
  cloud: number
  todayGain: number
  healthy: boolean
}

/**
 * 类名查表 —— 这不是风格偏好，是硬约束。
 *
 * Tailwind 只识别源码中**字面量**出现的类名。写成 `badge-${tone}` 会在
 * 生产构建后静默失效（样式没了但不报错，极难排查）。见设计指南 §9.1①。
 */
const balanceBadge = {
  high: 'badge-soft badge-success',
  mid: 'badge-soft badge-warning',
  low: 'badge-soft badge-error',
} as const

export interface DashboardPageProps {
  user: User | null
  onGo: (id: PageId) => void
}

export const DashboardPage = (props: DashboardPageProps) => {
  const accounts = ref<Account[]>([
    { id: 'acc-1', name: '账号2', cloud: 1286, todayGain: 12, healthy: true },
    { id: 'acc-2', name: '账号1', cloud: 486, todayGain: -214, healthy: false },
  ])

  const total = computed(() => accounts.value.reduce((sum, a) => sum + a.cloud, 0))
  const todayGain = computed(() => accounts.value.reduce((sum, a) => sum + a.todayGain, 0))
  const healthyCount = computed(() => accounts.value.filter((a) => a.healthy).length)

  const toneOf = (cloud: number): keyof typeof balanceBadge =>
    cloud >= 1000 ? 'high' : cloud >= 500 ? 'mid' : 'low'

  /** 涨跌符号。**涨红跌绿**是业务约定（设计指南 §2.4），不要按 daisyUI 默认语义改。 */
  const gainText = (n: number) => (n > 0 ? `+${n}` : String(n))
  const gainClass = (n: number) => (n < 0 ? 'text-success' : 'text-error')

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="text-base font-medium">账号云朵总览</h1>
        <span class="badge badge-ghost">占位数据（P6 接真实接口）</span>
      </div>

      {/* 指标卡：移动端必须竖排，横排会挤到不可读（§5.1） */}
      <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
        <div class="stat">
          <div class="stat-title">云朵总数</div>
          <div class="stat-value text-2xl">{total.value}</div>
          <div class={`stat-desc ${gainClass(todayGain.value)}`}>今日 {gainText(todayGain.value)}</div>
        </div>
        <div class="stat">
          <div class="stat-title">账号数</div>
          <div class="stat-value text-2xl">{accounts.value.length}</div>
          <div class="stat-desc">健康 {healthyCount.value} / {accounts.value.length}</div>
        </div>
        <div class="stat">
          <div class="stat-title">下次任务</div>
          <div class="stat-value text-2xl">—</div>
          <div class="stat-desc">定时任务上线后显示</div>
        </div>
      </div>

      {/* 桌面端表格 */}
      <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
        <table class="table table-sm">
          <thead>
            <tr>
              <th>账号</th>
              <th>云朵</th>
              <th>今日增量</th>
              <th>状态</th>
              <th class="w-20" />
            </tr>
          </thead>
          <tbody>
            {accounts.value.map((a) => (
              <tr>
                <td class="font-medium">{a.name}</td>
                <td>
                  <span class={`badge ${balanceBadge[toneOf(a.cloud)]}`}>{a.cloud}</span>
                </td>
                <td class={gainClass(a.todayGain)}>{gainText(a.todayGain)}</td>
                <td>
                  <span class="flex items-center gap-2 text-sm">
                    <span
                      class={a.healthy ? 'status status-success' : 'status status-error'}
                      aria-hidden="true"
                    />
                    <span class="opacity-70">{a.healthy ? '正常' : '异常'}</span>
                  </span>
                </td>
                <td>
                  <button
                    type="button"
                    class="btn btn-xs btn-soft btn-neutral"
                    onClick={() => props.onGo('accounts')}
                  >
                    详情
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* 移动端卡片流（同一数据，不写两套逻辑） */}
      <div class="grid gap-3 md:hidden">
        {accounts.value.map((a) => (
          <div class="card bg-base-100 shadow-sm">
            <div class="card-body gap-2 p-4">
              <div class="flex items-center justify-between gap-2">
                <span class="font-medium">{a.name}</span>
                <span class={`badge ${balanceBadge[toneOf(a.cloud)]}`}>{a.cloud}</span>
              </div>
              <div class="flex items-center justify-between text-sm">
                <span class="opacity-70">今日增量</span>
                <span class={gainClass(a.todayGain)}>{gainText(a.todayGain)}</span>
              </div>
              <div class="flex items-center justify-between text-sm">
                <span class="opacity-70">状态</span>
                <span class="flex items-center gap-2">
                  <span
                    class={a.healthy ? 'status status-success' : 'status status-error'}
                    aria-hidden="true"
                  />
                  {a.healthy ? '正常' : '异常'}
                </span>
              </div>
            </div>
          </div>
        ))}
      </div>

      <p class="text-xs opacity-60">
        账号的增删改查将在 P6 接入真实接口后开放。
      </p>
    </div>
  )
}
