import { computed, defineVaporComponent, ref } from 'vue'

/**
 * P0 spike：验证 Vapor + TSX + daisyUI 链路。
 *
 * 这个文件的存在意义是**把三个风险点一次性证伪或证实**：
 *  1. `defineVaporComponent` 返回 JSX 的写法能否被 vue-jsx-vapor 编译；
 *  2. daisyUI 的类名能否被 Tailwind 4 正确生成（含响应式前缀）；
 *  3. 原生 `<dialog>` 这类"非组件"的交互能否在 Vapor 树里正常驱动。
 *
 * 真正的页面（仪表盘 / 账号 / 任务 / 兑换 / 定时 / 历史 / 调试台 / 设置）
 * 在 P6 落地，届时替换本文件。
 */

/**
 * 类名查表 —— 这不是风格偏好，是硬约束。
 *
 * Tailwind 只识别源码中**字面量**出现的类名。写成 `btn-${variant}` 会在
 * 生产构建后静默失效（样式没了但不报错，极难排查）。
 * 所以动态变体一律走查表，且表里的值必须是完整字面量。
 */
const btnTheme = {
  primary: 'btn-primary',
  error: 'btn-error',
  ghost: 'btn-ghost',
} as const

type BtnVariant = keyof typeof btnTheme

/** 账号卡片的颜色随余额档位变化 —— 同样走查表，不拼接。 */
const balanceBadge = {
  high: 'badge-success',
  mid: 'badge-warning',
  low: 'badge-error',
} as const

interface Account {
  id: string
  name: string
  cloud: number
  todayGain: number
  healthy: boolean
}

export default defineVaporComponent(() => {
  const accounts = ref<Account[]>([
    { id: 'acc-1', name: '账号2', cloud: 1286, todayGain: 12, healthy: true },
    { id: 'acc-2', name: '账号1', cloud: 486, todayGain: -214, healthy: false },
  ])

  const modalOpen = ref(false)

  const total = computed(() => accounts.value.reduce((sum, a) => sum + a.cloud, 0))

  const toneOf = (cloud: number): keyof typeof balanceBadge =>
    cloud >= 1000 ? 'high' : cloud >= 500 ? 'mid' : 'low'

  const addAccount = () => {
    const n = accounts.value.length + 1
    accounts.value.push({
      id: `acc-${n}`,
      name: `新账号${n}`,
      cloud: 0,
      todayGain: 0,
      healthy: true,
    })
  }

  return (
    <div class="min-h-screen bg-base-200">
      <div class="navbar bg-base-100 shadow-sm">
        <div class="navbar-start">
          <span class="text-lg font-medium">mcloud</span>
        </div>
        <div class="navbar-end gap-2">
          <span class="badge badge-ghost">Vapor 模式</span>
          <button
            class="btn btn-sm btn-primary"
            onClick={() => (modalOpen.value = true)}
          >
            快照说明
          </button>
        </div>
      </div>

      <main class="p-4 md:p-6">
        <div class="mb-4 flex flex-wrap items-center gap-3">
          <h1 class="text-base">账号云朵总览</h1>
          <span class="badge badge-primary">{total} 云朵</span>
          <button
            class={`btn btn-sm ${btnTheme.primary}`}
            onClick={addAccount}
          >
            添加账号
          </button>
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

      {/* daisyUI 的 modal 支持用 modal-open 类强制打开，不需要 DOM ref 调 showModal()，
          这样在 Vapor 里零额外机制即可驱动。 */}
      <dialog class={modalOpen.value ? 'modal modal-open' : 'modal'}>
        <div class="modal-box">
          <h3 class="text-base font-medium">云朵日快照</h3>
          <p class="py-3 text-sm opacity-80">
            后端按 Asia/Shanghai 切分自然日，每个账号每天一条快照，
            并计算日增量与周增量。数据存 SQLite，重启不丢。
          </p>
          <div class="modal-action">
            <button class={`btn btn-sm ${btnTheme.ghost}`} onClick={() => (modalOpen.value = false)}>
              知道了
            </button>
          </div>
        </div>
      </dialog>
    </div>
  )
})
