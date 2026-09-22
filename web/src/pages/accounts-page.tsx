/**
 * 账号管理页：真实数据来自 `GET /api/accounts`。
 *
 * 形态按设计指南执行：桌面 `table`、移动卡片流（§5.3，同一数据源）；
 * badge/status 类名一律查字面量表（§9.1①，拼接会静默失效）；
 * 空态给"下一步做什么"而不是「暂无数据」（§5.10）。
 *
 * 添加/删除目前**没有 HTTP 接口**（登录仍走 CLI），所以空态的下一步
 * 是把命令原样告诉用户，而不是摆一个点了没反应的"添加"按钮。
 */

import { onMounted, ref } from 'vue'
import { ApiError, listAccounts, type Account } from '../api'
import { fmtTime } from '../ui/time'

type LoadState = 'loading' | 'error' | 'ready'

/** 余额档位（§5.4 的查表，与总览页同一套阈值，不另起一表）。 */
const balanceBadge = {
  high: 'badge-soft badge-success',
  mid: 'badge-soft badge-warning',
  low: 'badge-soft badge-error',
} as const

const toneOf = (cloud: number): keyof typeof balanceBadge =>
  cloud >= 1000 ? 'high' : cloud >= 500 ? 'mid' : 'low'

const platformLabel = (p: string | null): string =>
  p === 'mobile' ? '手机' : p === 'pc' ? 'PC' : '未知'

/** 健康度：绿=正常，红=不可用（过期/停用），黄=有告警但可用（§5.4）。 */
const healthOf = (a: Account): { dot: string; label: string } => {
  if (!a.enabled) return { dot: 'status', label: '已停用' }
  if (a.authExpired) return { dot: 'status status-error', label: '登录已过期' }
  if (a.failStreak > 0)
    return { dot: 'status status-warning', label: `连续失败 ${a.failStreak} 次` }
  return { dot: 'status status-success', label: '正常' }
}

const cloudCell = (a: Account) =>
  a.cloudCurrent === null ? (
    <span class="badge badge-ghost">未采集</span>
  ) : (
    <span class={`badge ${balanceBadge[toneOf(a.cloudCurrent)]}`}>{a.cloudCurrent}</span>
  )

const healthCell = (a: Account) => {
  const h = healthOf(a)
  return (
    <span class="flex items-center gap-2 text-sm">
      <span class={h.dot} aria-hidden="true" />
      <span class="opacity-70">{h.label}</span>
    </span>
  )
}

export const AccountsPage = () => {
  const state = ref<LoadState>('loading')
  const accounts = ref<Account[]>([])
  const errorMessage = ref('')

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const result = await listAccounts()
      accounts.value = [...result.accounts].sort((a, b) => a.sortOrder - b.sortOrder)
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof ApiError ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => void load())

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <h1 class="text-base font-medium">账号管理</h1>
        <span class="badge badge-soft">{accounts.value.length}</span>
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

  const loading = () => (
    <div class="flex justify-center py-16">
      <span class="loading loading-spinner loading-lg text-primary" />
    </div>
  )

  const errorAlert = () => (
    <div class="alert alert-soft alert-error text-sm" role="alert">
      <span class="break-words">{errorMessage.value}</span>
      <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
        重试
      </button>
    </div>
  )

  /** 空态：说清"去哪加"，命令原样给出（§5.10）。 */
  const empty = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body items-center gap-3 py-12 text-center">
        <p class="text-sm opacity-70">还没有云盘账号。</p>
        <p class="max-w-prose break-words text-xs opacity-60">
          登录目前走命令行：
          <code class="ml-1 rounded bg-base-200 px-1 py-0.5">
            mcloud login sms --client app --phone 手机号
          </code>
          或<code class="ml-1 rounded bg-base-200 px-1 py-0.5">mcloud login qr</code>
          ，成功后回到本页刷新即可看到。
        </p>
      </div>
    </div>
  )

  const desktopTable = () => (
    <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 shadow-sm md:block">
      <table class="table table-sm">
        <thead>
          <tr>
            <th>账号</th>
            <th>端</th>
            <th>云朵</th>
            <th>最近成功</th>
            <th>状态</th>
          </tr>
        </thead>
        <tbody>
          {accounts.value.map((a) => (
            <tr>
              <td class="font-medium break-words">{a.displayName}</td>
              <td>{platformLabel(a.platform)}</td>
              <td>{cloudCell(a)}</td>
              <td class="text-sm opacity-70">{fmtTime(a.lastOkAt)}</td>
              <td>{healthCell(a)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )

  /** 移动卡片：切了卡片每项必须带标签（§7.3）。 */
  const mobileCards = () => (
    <div class="grid gap-3 md:hidden">
      {accounts.value.map((a) => (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-2 p-4">
            <div class="flex items-center justify-between gap-2">
              <span class="break-words font-medium">{a.displayName}</span>
              <span class="badge badge-soft">{platformLabel(a.platform)}</span>
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">云朵</span>
              {cloudCell(a)}
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">最近成功</span>
              <span>{fmtTime(a.lastOkAt)}</span>
            </div>
            <div class="flex items-center justify-between text-sm">
              <span class="opacity-70">状态</span>
              {healthCell(a)}
            </div>
          </div>
        </div>
      ))}
    </div>
  )

  return (
    <div class="flex flex-col gap-4">
      {header()}
      {state.value === 'loading' ? loading() : null}
      {state.value === 'error' ? errorAlert() : null}
      {state.value === 'ready' && accounts.value.length === 0 ? empty() : null}
      {state.value === 'ready' && accounts.value.length > 0 ? (
        <>
          {desktopTable()}
          {mobileCards()}
        </>
      ) : null}
    </div>
  )
}
