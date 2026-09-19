import { computed, defineVaporComponent, ref } from 'vue'
import { ApiError, currentSession, runSetup, setupStatus, signIn, signOut, type User } from './api'

/**
 * 应用外壳：**先过会话门禁**，再谈页面。
 *
 * 三个状态都是后端定的，前端不自己猜：
 *  - `GET /api/setup` 说 `needsSetup` → 初始化页（此时后端**只**暴露它，别的接口一律 503）；
 *  - 有会话 → 主界面；
 *  - 401 → 登录页。
 *
 * 鉴权放在这里而不是每个页面里：漏掉一处就是"某个页面能未登录打开"，
 * 而这种漏法在页面多起来之后必然发生。
 *
 * 主界面内容仍是 P0 spike 的占位数据 —— 真正的页面（账号 / 任务 / 兑换 / 定时 / 历史）
 * 在 P6 落地。这里刻意不把占位数据做得像真的，免得哪天被当成实现。
 */

type View = 'loading' | 'setup' | 'login' | 'app'

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
 * 生产构建后静默失效（样式没了但不报错，极难排查）。
 */
const balanceBadge = {
  high: 'badge-success',
  mid: 'badge-warning',
  low: 'badge-error',
} as const

export default defineVaporComponent(() => {
  const view = ref<View>('loading')
  const user = ref<User | null>(null)
  const minPasswordLen = ref(8)

  const name = ref('')
  const password = ref('')
  const again = ref('')
  const busy = ref(false)
  const error = ref('')
  const notice = ref('')

  /** 探一次"该显示哪个页面"。未登录是正常路径，不是错误。 */
  const bootstrap = async () => {
    try {
      const status = await setupStatus()
      minPasswordLen.value = status.minPasswordLen
      if (status.needsSetup) {
        view.value = 'setup'
        return
      }
      const session = await currentSession()
      user.value = session.user
      view.value = 'app'
    } catch {
      // 401（未登录）与后端没起来都落到这里。两者对用户来说下一步一样：去登录页。
      view.value = 'login'
    }
  }

  const submit = async () => {
    error.value = ''
    notice.value = ''
    if (view.value === 'setup' && password.value !== again.value) {
      error.value = '两次输入的口令不一致'
      return
    }
    busy.value = true
    try {
      if (view.value === 'setup') {
        const result = await runSetup(name.value, password.value)
        user.value = result.user
        const { accounts, schedules, pipelines } = result.claimed
        if (accounts + schedules + pipelines > 0) {
          notice.value = `已认领旧数据里原本没有归属的数据：账号 ${accounts}、定时任务 ${schedules}、组合 ${pipelines}`
        }
        view.value = 'app'
      } else {
        const session = await signIn(name.value, password.value)
        user.value = session.user
        view.value = 'app'
      }
      password.value = ''
      again.value = ''
    } catch (err) {
      error.value = err instanceof ApiError ? err.message : '请求失败，请检查服务是否在运行'
    } finally {
      busy.value = false
    }
  }

  const logout = async () => {
    try {
      await signOut()
    } catch {
      // 会话可能早就失效了：登出的目的是"回到登录页"，失败不该把它变成错误弹窗
    }
    user.value = null
    name.value = ''
    error.value = ''
    notice.value = ''
    view.value = 'login'
  }

  const minLenHint = computed(() => `至少 ${minPasswordLen.value} 位`)

  // ── 占位主界面（P6 替换）──
  const accounts = ref<Account[]>([
    { id: 'acc-1', name: '账号2', cloud: 1286, todayGain: 12, healthy: true },
    { id: 'acc-2', name: '账号1', cloud: 486, todayGain: -214, healthy: false },
  ])
  const total = computed(() => accounts.value.reduce((sum, a) => sum + a.cloud, 0))
  const toneOf = (cloud: number): keyof typeof balanceBadge =>
    cloud >= 1000 ? 'high' : cloud >= 500 ? 'mid' : 'low'

  void bootstrap()

  const authForm = (title: string, hint: string, showConfirm: boolean) => (
    <div class="flex min-h-screen items-center justify-center bg-base-200 p-4">
      <div class="card w-full max-w-sm bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h1 class="card-title text-base">{title}</h1>
          <p class="text-sm opacity-70">{hint}</p>

          {error.value ? (
            <div class="alert alert-error text-sm" role="alert">
              {error.value}
            </div>
          ) : null}

          <label class="form-control">
            <span class="label-text">用户名</span>
            <input
              class="input input-bordered mt-1"
              autocomplete="username"
              value={name.value}
              onInput={(e) => (name.value = (e.target as HTMLInputElement).value)}
            />
          </label>

          <label class="form-control">
            <span class="label-text">
              口令 <span class="opacity-60">（{minLenHint.value}）</span>
            </span>
            <input
              class="input input-bordered mt-1"
              type="password"
              autocomplete={showConfirm ? 'new-password' : 'current-password'}
              value={password.value}
              onInput={(e) => (password.value = (e.target as HTMLInputElement).value)}
            />
          </label>

          {showConfirm ? (
            <label class="form-control">
              <span class="label-text">再输一次</span>
              <input
                class="input input-bordered mt-1"
                type="password"
                autocomplete="new-password"
                value={again.value}
                onInput={(e) => (again.value = (e.target as HTMLInputElement).value)}
              />
            </label>
          ) : null}

          <button class="btn btn-primary mt-2" disabled={busy.value} onClick={() => void submit()}>
            {busy.value ? '处理中…' : showConfirm ? '创建管理员并进入' : '登录'}
          </button>

          {showConfirm ? (
            <p class="text-xs opacity-60">
              这个口令只用于登录本系统，与移动云盘账号无关；它没有找回入口，忘了要直接改库。
            </p>
          ) : null}
        </div>
      </div>
    </div>
  )

  const dashboard = () => (
    <div class="min-h-screen bg-base-200">
      <div class="navbar bg-base-100 shadow-sm">
        <div class="navbar-start">
          <span class="text-lg font-medium">mcloud</span>
        </div>
        <div class="navbar-end gap-2">
          <span class="badge badge-ghost">
            {user.value?.name}（{user.value?.role}）
          </span>
          <button class="btn btn-sm btn-ghost" onClick={() => void logout()}>
            退出登录
          </button>
        </div>
      </div>

      <main class="p-4 md:p-6">
        {notice.value ? (
          <div class="alert alert-info mb-4 text-sm" role="status">
            {notice.value}
          </div>
        ) : null}

        <div class="mb-4 flex flex-wrap items-center gap-3">
          <h1 class="text-base">账号云朵总览</h1>
          <span class="badge badge-primary">{total.value} 云朵</span>
          <span class="badge badge-ghost">占位数据（P6 接真实接口）</span>
        </div>

        {/* 桌面端表格，窄屏切卡片 —— daisyUI 的响应式靠 Tailwind 前缀，不做两套结构 */}
        <div class="hidden overflow-x-auto rounded-box bg-base-100 p-2 md:block">
          <table class="table table-sm">
            <thead>
              <tr>
                <th>账号</th>
                <th>云朵</th>
                <th>今日增量</th>
                <th>状态</th>
              </tr>
            </thead>
            <tbody>
              {accounts.value.map((a) => (
                <tr>
                  <td>{a.name}</td>
                  <td>
                    <span class={`badge ${balanceBadge[toneOf(a.cloud)]}`}>{a.cloud}</span>
                  </td>
                  <td class={a.todayGain < 0 ? 'text-error' : 'text-success'}>
                    {a.todayGain > 0 ? `+${a.todayGain}` : a.todayGain}
                  </td>
                  <td>
                    <span class={`status ${a.healthy ? 'status-success' : 'status-error'}`} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <div class="grid gap-3 md:hidden">
          {accounts.value.map((a) => (
            <div class="card bg-base-100 shadow-sm">
              <div class="card-body p-4">
                <div class="flex items-center justify-between">
                  <span class="font-medium">{a.name}</span>
                  <span class={`badge ${balanceBadge[toneOf(a.cloud)]}`}>{a.cloud}</span>
                </div>
                <div class="text-sm opacity-70">
                  今日增量 {a.todayGain > 0 ? `+${a.todayGain}` : a.todayGain}
                </div>
              </div>
            </div>
          ))}
        </div>
      </main>
    </div>
  )

  const loading = () => (
    <div class="flex min-h-screen items-center justify-center bg-base-200">
      <span class="loading loading-spinner loading-lg" />
    </div>
  )

  return (
    <>
      {view.value === 'loading' ? loading() : null}
      {view.value === 'setup'
        ? authForm('初始化首个管理员', '这个实例还没有管理员，先建一个。', true)
        : null}
      {view.value === 'login' ? authForm('登录', '用本系统的账号登录。', false) : null}
      {view.value === 'app' ? dashboard() : null}
    </>
  )
})
