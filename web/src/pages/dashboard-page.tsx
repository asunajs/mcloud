/**
 * 总览页：真实数据来自 `GET /api/accounts` + 每账号 `GET /api/accounts/{id}/cloud-stats`。
 *
 * 结构（设计指南 §5.1、§5.2、§7.2）：
 *  ① `stats` 指标卡一排 —— 移动端竖排、桌面横排；
 *  ② 账号列表 —— 桌面 `table`、移动端卡片流（同一个数据源，不写两套逻辑）。
 *
 * 口径：
 *  - 云朵当前值优先用账号行上的 `cloudCurrent`，为空则回落到最近一条日快照；
 *  - "今日增量"只认 `date === 今天` 的那条 `cloudDiff` —— 没有就是"未采集"，
 *    **不拿昨天的差值冒充今天的**（切日按服务端时区，见后端 `cloudStats` 注释）；
 *  - 健康 = 启用 && 未过期 && 无连续失败（与账号页同一套判据）。
 */

import { computed, onMounted, ref } from 'vue'
import {
  ApiError,
  getCloudStats,
  listAccounts,
  type Account,
  type CloudStatRow,
} from '../api'
import type { PageId } from '../ui/nav'

interface AccountRow {
  account: Account
  /** 云朵当前值；null = 从没采到过。 */
  cloud: number | null
  /** 今日增量；null = 今天还没有快照。 */
  todayGain: number | null
}

type LoadState = 'loading' | 'error' | 'ready'

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

const toneOf = (cloud: number): keyof typeof balanceBadge =>
  cloud >= 1000 ? 'high' : cloud >= 500 ? 'mid' : 'low'

/** 浏览器本地日期 → `YYYY-MM-DD`（与服务端 `date` 列同形）。 */
const todayStr = () => new Date().toLocaleDateString('en-CA')

const cloudOf = (account: Account, stats: CloudStatRow[]): number | null => {
  if (account.cloudCurrent !== null) return account.cloudCurrent
  if (stats.length === 0) return null
  // 后端按日期升序给，最后一条即最近快照；仍自己取 max 以防形状变化。
  const latest = stats.reduce((m, s) => (s.date > m.date ? s : m), stats[0])
  return latest.cloudCount
}

const gainOf = (stats: CloudStatRow[]): number | null => {
  const today = todayStr()
  const row = stats.find((s) => s.date === today)
  return row ? row.cloudDiff : null
}

export interface DashboardPageProps {
  user: User | null
  onGo: (id: PageId) => void
}

import type { User } from '../api'

export const DashboardPage = (props: DashboardPageProps) => {
  const state = ref<LoadState>('loading')
  const rows = ref<AccountRow[]>([])
  const errorMessage = ref('')

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const { accounts } = await listAccounts()
      // 单账号的历史拉失败不拖垮整页：那条的云朵/增量落到"未采集"。
      const histories = await Promise.allSettled(
        accounts.map((a) => getCloudStats(a.id, 7)),
      )
      rows.value = accounts.map((account, i) => {
        const h = histories[i]
        const stats = h.status === 'fulfilled' ? h.value.stats : []
        return { account, cloud: cloudOf(account, stats), todayGain: gainOf(stats) }
      })
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => void load())

  const total = computed(() =>
    rows.value.reduce((sum, r) => sum + (r.cloud ?? 0), 0),
  )
  /** 今日增量：一条"今天"的数都没有时显示破折号，而不是假装 0。 */
  const anyToday = computed(() => rows.value.some((r) => r.todayGain !== null))
  const todayGain = computed(() =>
    rows.value.reduce((sum, r) => sum + (r.todayGain ?? 0), 0),
  )
  const healthyCount = computed(
    () =>
      rows.value.filter(
        (r) => r.account.enabled && !r.account.authExpired && r.account.failStreak === 0,
      ).length,
  )

  /** 涨跌符号。**只给数，不给颜色** —— 云朵减少多半是正常花费，
   * 把它染成"异常色"会让人误以为账号出问题了（状态列只反映账号健康度）。 */
  const gainText = (n: number | null) =>
    n === null ? '—' : n > 0 ? `+${n}` : String(n)

  const cloudBadge = (cloud: number | null) =>
    cloud === null ? (
      <span class="badge badge-ghost">未采集</span>
    ) : (
      <span class={`badge ${balanceBadge[toneOf(cloud)]}`}>{cloud}</span>
    )

  const head = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="text-base font-medium">账号云朵总览</h1>
        {state.value === 'ready' ? (
          <span class="badge badge-soft badge-neutral">{rows.value.length}</span>
        ) : null}
      </div>
      <button
        type="button"
        class="btn btn-sm btn-soft"
        disabled={state.value === 'loading'}
        onClick={() => void load()}
      >
        刷新
      </button>
    </div>
  )

  const body = () => (
    <>
      {/* 指标卡：移动端必须竖排，横排会挤到不可读（§5.1） */}
      <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
        <div class="stat">
          <div class="stat-title">云朵总数</div>
          <div class="stat-value text-2xl">{total.value}</div>
          <div class="stat-desc">今日 {anyToday.value ? gainText(todayGain.value) : '—'}</div>
        </div>
        <div class="stat">
          <div class="stat-title">账号数</div>
          <div class="stat-value text-2xl">{rows.value.length}</div>
          <div class="stat-desc">
            健康 {healthyCount.value} / {rows.value.length}
          </div>
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
            {rows.value.map((r) => {
              const healthy =
                r.account.enabled && !r.account.authExpired && r.account.failStreak === 0
              return (
                <tr>
                  <td class="font-medium break-words">{r.account.displayName}</td>
                  <td>{cloudBadge(r.cloud)}</td>
                  <td>{gainText(r.todayGain)}</td>
                  <td>
                    <span class="flex items-center gap-2 text-sm">
                      <span
                        class={healthy ? 'status status-success' : 'status status-error'}
                        aria-hidden="true"
                      />
                      <span class="opacity-70">{healthy ? '正常' : '异常'}</span>
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
              )
            })}
          </tbody>
        </table>
      </div>

      {/* 移动端卡片流（同一数据，不写两套逻辑；每项带标签，§7.3） */}
      <div class="grid gap-3 md:hidden">
        {rows.value.map((r) => {
          const healthy =
            r.account.enabled && !r.account.authExpired && r.account.failStreak === 0
          return (
            <div class="card bg-base-100 shadow-sm">
              <div class="card-body gap-2 p-4">
                <div class="flex items-center justify-between gap-2">
                  <span class="break-words font-medium">{r.account.displayName}</span>
                  {cloudBadge(r.cloud)}
                </div>
                <div class="flex items-center justify-between text-sm">
                  <span class="opacity-70">今日增量</span>
                  <span>{gainText(r.todayGain)}</span>
                </div>
                <div class="flex items-center justify-between text-sm">
                  <span class="opacity-70">状态</span>
                  <span class="flex items-center gap-2">
                    <span
                      class={healthy ? 'status status-success' : 'status status-error'}
                      aria-hidden="true"
                    />
                    {healthy ? '正常' : '异常'}
                  </span>
                </div>
              </div>
            </div>
          )
        })}
      </div>

      <p class="text-xs opacity-60">
        添加账号走命令行：
        <code class="mx-1 rounded bg-base-200 px-1 py-0.5">mcloud login sms --client app --phone 手机号</code>
        或
        <code class="mx-1 rounded bg-base-200 px-1 py-0.5">mcloud login qr</code>
        ；云朵数在首次采集/签到后出现。
      </p>
    </>
  )

  return (
    <div class="flex flex-col gap-4">
      {head()}
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
      {state.value === 'ready' && rows.value.length === 0 ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body items-center gap-3 py-12 text-center">
            <p class="text-sm opacity-70">还没有云盘账号。</p>
            <p class="max-w-prose break-words text-xs opacity-60">
              运行
              <code class="mx-1 rounded bg-base-200 px-1 py-0.5">mcloud login sms --client app --phone 手机号</code>
              添加后回来刷新。
            </p>
          </div>
        </div>
      ) : null}
      {state.value === 'ready' && rows.value.length > 0 ? body() : null}
    </div>
  )
}
