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
 * 布局骨架照抄设计指南 §4.3（`drawer` 三层结构），**不要改顺序**。
 */

import { computed, defineVaporComponent, onMounted, onUnmounted, ref } from 'vue'
import {
  currentSession,
  serviceStatus,
  setupStatus,
  signOut,
  type ServiceStatus,
  type User,
} from './api'
import { AuthPage } from './pages/auth-page'
import { AccountsPage } from './pages/accounts-page'
import { DashboardPage } from './pages/dashboard-page'
import { ExchangePage } from './pages/exchange-page'
import { HistoryPage } from './pages/history-page'
import { LiveRedFlowerPage } from './pages/live-redflower-page'
import { PlaceholderPage } from './pages/placeholder-page'
import { SchedulesPage } from './pages/schedules-page'
import { TasksPage } from './pages/tasks-page'
import { statusLabel } from './ui/history'
import { IconClose, IconMenu, IconMoon, IconSun } from './ui/icons'
import { DOCK_IDS, NAV_ITEMS, navItem, parseHash, type PageId } from './ui/nav'
import { fmtTime } from './ui/time'
import { currentTheme, toggleTheme, type Theme } from './ui/theme'

type View = 'loading' | 'setup' | 'login' | 'app'

/** 后端自述那一行的取数状态。`pending` 必须**独立存在**：
 *  把它并进"失败"，首屏会闪一下红点，而红点是"后端挂了"的意思。 */
type BackendState = 'pending' | 'up' | 'down'

/** `lastRun.status` 的中文口径**只有一份** —— 在 `ui/history.ts` 里，
 *  与「历史与日志」页共用（那边列表上的徽标就是同一组值）。
 *  这里曾经自己抄了一张三行的表：加第四档时两处必然漂移，
 *  而漂移的表现是侧栏显示 `running` 这种原文码、历史页显示中文。 */

export default defineVaporComponent(() => {
  const view = ref<View>('loading')
  const user = ref<User | null>(null)
  const minPasswordLen = ref(8)
  /** 当前主题。以本地状态驱动图标，避免每次渲染都去读 DOM。 */
  const theme = ref<Theme>(currentTheme())

  /** 当前页面。用 hash 路由（无依赖、可刷新、可分享链接）。 */
  const page = ref<PageId>(parseHash(location.hash))
  /**
   * 抽屉开关。**只**驱动 checkbox 的 checked，不碰布局类名。
   *
   * 曾经在这里给根 div 加 `drawer-open` 来"打开抽屉"，那是错的：
   * daisyUI 5 的 `drawer-open` 是**常驻分栏**变体（`.drawer-open>.drawer-side{position:sticky;width:auto}`），
   * 而 `.drawer` 本身是 `grid` + `grid-auto-columns:max-content auto` ——
   * 窄屏挂上它，256px 侧栏就占掉一列真实栅格，`.drawer-content` 被压成剩下的一条，
   * 实测 493px 屏宽下内容区只剩 237px（文字挤成一团）。
   * 移动端展开态由 `#nav-drawer` 这个 checkbox 的 `:checked` 走**浮层**（overlay）分支，
   * `lg:drawer-open` 才留给桌面端的常驻分栏。
   */
  const drawerOpen = ref(false)

  const notice = ref('')
  const error = ref('')

  /** 后端自述（版本 / 最近一次执行 / 调度在不在跑），只给侧栏底部那一行用。 */
  const backend = ref<ServiceStatus | null>(null)
  const backendState = ref<BackendState>('pending')

  /**
   * 拉一次后端自述。**不轮询**：这一行的价值是"进页面时后端是什么样"，
   * 而每分钟刷一次只会让人以为那个时间戳在动。
   *
   * 失败要单独记状态：这一行显示的就是"连没连上后端"，
   * 拿不到数据本身**就是**要显示的内容，不能悄悄留着上一次的绿点。
   */
  const loadStatus = async () => {
    backendState.value = 'pending'
    try {
      backend.value = await serviceStatus()
      backendState.value = 'up'
    } catch {
      backendState.value = 'down'
    }
  }

  /** 侧栏那一行的第二句：**只在问到的时候才有内容**（取数状态已经由头部那句话承担）。 */
  const backendLine2 = computed(() => {
    const b = backend.value
    if (backendState.value !== 'up' || !b) return ''
    if (!b.lastRun) return '尚无执行记录'
    const label = statusLabel(b.lastRun.status)
    return `最近执行 ${fmtTime(b.lastRun.startedAt)} · ${label}`
  })

  /** 侧栏那一行的第三句：调度器没在跑就得说明 —— 不然"定时"这页看着像在工作。 */
  const backendLine3 = computed(() => {
    const b = backend.value
    return backendState.value === 'up' && !b?.schedulerRunning
      ? '定时调度未启用，任务不会自动触发'
      : ''
  })

  /** 侧栏那一行的点色。**只在真失败时变红**：
   *  红点的含义是"后端挂了"，首屏闪一下红会让人当场去重启服务。 */
  const backendDot = computed(() => {
    if (backendState.value === 'down') return 'status status-error'
    if (backendState.value === 'up') return 'status status-success'
    return 'status status-neutral animate-pulse'
  })

  const backendHead = computed(() => {
    if (backendState.value === 'down') return '后端连接中断'
    const b = backend.value
    return b ? `${b.name} v${b.version}` : '正在问后端…'
  })

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
      void loadStatus()
    } catch {
      // 401（未登录）与后端没起来都落到这里。两者对用户来说下一步一样：去登录页。
      view.value = 'login'
    }
  }

  void bootstrap()

  const onHashChange = () => {
    page.value = parseHash(location.hash)
    drawerOpen.value = false
  }

  onMounted(() => {
    window.addEventListener('hashchange', onHashChange)
  })
  onUnmounted(() => {
    window.removeEventListener('hashchange', onHashChange)
  })

  const go = (id: PageId) => {
    const item = navItem(id)
    if (location.hash !== item.href) {
      location.hash = item.href
    }
    page.value = id
    drawerOpen.value = false
  }

  const logout = async () => {
    try {
      await signOut()
    } catch {
      // 会话可能早就失效了：登出的目的是"回到登录页"，失败不该把它变成错误弹窗
    }
    user.value = null
    notice.value = ''
    error.value = ''
    // 侧栏那一行的状态也是"这个人的数据"，登出必须一起清掉 ——
    // 否则下一个人登录后先看到上一个人的"最近执行"，哪怕只有一瞬。
    backend.value = null
    backendState.value = 'pending'
    // 登出后清掉 hash，免得下次登录直接被送到子页面
    location.hash = ''
    page.value = 'dashboard'
    view.value = 'login'
  }

  /** 切换主题。图标随之变（浅色下显示"切到深色"的月亮，反之太阳）。 */
  const onToggleTheme = () => {
    theme.value = toggleTheme()
  }

  /**
   * 主题切换按钮。放在**所有视图**都能看到的地方（登录页也有）——
   * 否则用户得先登录才能把刺眼的浅色换成深色，这就本末倒置了。
   */
  const themeToggle = () => (
    <button
      type="button"
      class="btn btn-square btn-ghost btn-sm"
      aria-label={theme.value === 'dark' ? '切换到浅色主题' : '切换到深色主题'}
      title={theme.value === 'dark' ? '切换到浅色主题' : '切换到深色主题'}
      onClick={onToggleTheme}
    >
      {theme.value === 'dark' ? <IconSun class="size-4" /> : <IconMoon class="size-4" />}
    </button>
  )

  const current = computed(() => navItem(page.value))

  /**
   * 侧栏导航项。
   *
   * 激活态用 `menu-active`（daisyUI 语义类），**不自己拼颜色** —— 它随主题走。
   */
  const navLink = (id: PageId, onPick: () => void) => {
    const item = navItem(id)
    const Icon = item.icon
    const active = page.value === id
    return (
      <li>
        <button
          type="button"
          class={active ? 'menu-active' : ''}
          aria-current={active ? 'page' : undefined}
          onClick={onPick}
        >
          <Icon class="size-[1.15em]" />
          {item.label}
        </button>
      </li>
    )
  }

  /**
   * 主界面骨架：`drawer` 三层 + navbar + main + 移动端 dock。
   *
   * 结构照抄设计指南 §4.3，要点：
   *  - `drawer-toggle` 是 checkbox，靠 `label for` 驱动，不用 JS 控制显隐；
   *    移动端展开态来自它的 `:checked`（浮层），**不是**给根 div 挂 `drawer-open`
   *    （那是常驻分栏变体，窄屏挂上会把内容压进半屏，见 `drawerOpen` 的注释）；
   *  - `drawer-side` 必须 `z-40`，否则被 navbar 的 `shadow-sm` 压住；
   *  - `lg:drawer-open` 加在**根 div** 上，只用于桌面常驻分栏；
   *  - `min-h-screen` 在 `drawer-content`，且 `flex-col` + `main` 用 `flex-1`，
   *    否则窄屏时底部 dock 会浮在半空。
   */
  const shell = () => (
    <div class="drawer lg:drawer-open">
      <input
        id="nav-drawer"
        type="checkbox"
        class="drawer-toggle"
        checked={drawerOpen.value}
        onChange={(e) => (drawerOpen.value = (e.target as HTMLInputElement).checked)}
      />

      <div class="drawer-content flex min-h-screen flex-col">
        <div class="navbar sticky top-0 z-30 bg-base-100 shadow-sm">
          <div class="navbar-start gap-1">
            <label
              for="nav-drawer"
              class="btn btn-square btn-ghost drawer-button lg:hidden"
              aria-label="打开导航菜单"
            >
              <IconMenu class="size-5" />
            </label>
            <span class="px-1 text-base font-medium">{current.value.title}</span>
          </div>
          <div class="navbar-end gap-1">
            {themeToggle()}
            {/*
              身份**只在这一处**显示（侧栏底部让给后端状态了）。
              `role` 收进 title：初始用户 name 与 role 的字面都是 admin，
              并排显示等于同一件事写两遍 —— 那正是"左下角两个 admin"的来由。

              不给颜色类 = 中性灰。daisyUI 的 `badge-soft` 是"文字用原色 + 底色只掺 8%"，
              而 `neutral` 在 light/dark 两个主题里**都是深色** ——
              所以 `badge-soft badge-neutral` 在暗色下是黑字压黑底（实测 1.23:1）。
              裸 `badge-soft` 走 base-content/base-100，两个主题都够（14.7:1 / 12.2:1）。
            */}
            <span class="badge badge-soft" title={`角色：${user.value?.role ?? '未知'}`}>
              {user.value?.name}
            </span>
            <button
              type="button"
              class="btn btn-sm btn-ghost"
              onClick={() => void logout()}
              title="退出登录"
            >
              退出
            </button>
          </div>
        </div>

        {/* pb-24 是为移动端 dock 让位；md 以上没有 dock，用普通内边距 */}
        <main class="flex-1 bg-base-200 p-4 pb-24 md:p-6 md:pb-6">
          {notice.value ? (
            <div class="alert alert-soft alert-info mb-4 text-sm" role="status">
              <span>{notice.value}</span>
              <button
                type="button"
                class="btn btn-square btn-ghost btn-xs"
                aria-label="关闭提示"
                onClick={() => (notice.value = '')}
              >
                <IconClose class="size-3" />
              </button>
            </div>
          ) : null}
          {error.value ? (
            <div class="alert alert-soft alert-error mb-4 text-sm" role="alert">
              <span>{error.value}</span>
            </div>
          ) : null}

          {page.value === 'dashboard' ? (
            <DashboardPage user={user.value} onGo={go} />
          ) : page.value === 'accounts' ? (
            <AccountsPage />
          ) : page.value === 'redflower' ? (
            <LiveRedFlowerPage />
          ) : page.value === 'exchange' ? (
            <ExchangePage />
          ) : page.value === 'schedules' ? (
            <SchedulesPage />
          ) : page.value === 'tasks' ? (
            <TasksPage />
          ) : page.value === 'history' ? (
            <HistoryPage onGo={go} />
          ) : (
            /**
             * `key` 不是可有可无的装饰：空态页落在 `createIf` 的同一个 else 分支里，
             * 页面之间切换（调试台 → 设置 → 任务中心）**不会重跑 setup**，
             * 而 Vapor 没有重渲染，于是 `navItem(props.id)` 那份快照永远停在第一次进来的页面 ——
             * 实测表现为导航栏标题跟着变、正文的标题/图标/说明纹丝不动。
             * 给了 key，key 变了就重建实例，快照才是当前页。
             */
            <PlaceholderPage key={page.value} id={page.value} onGo={go} />
          )}
        </main>

        {/*
          移动端底部导航（daisyUI 5 的 `dock`，旧名 `btm-nav` 已废弃）。
          只放 5 项、且**全是页面**（§4.4）—— 9 个页面全塞进来会挤到不可点。
          这里没有「更多」格：它点开的是侧栏抽屉，而抽屉在 navbar 左上角的汉堡包
          那里已经有一个入口，重复一格不换来任何可达性。
        */}
        <div class="dock dock-md z-30 border-t border-base-300 bg-base-100 md:hidden">
          {DOCK_IDS.map((id) => {
            const item = navItem(id)
            const Icon = item.icon
            const active = page.value === id
            return (
              <button
                type="button"
                aria-current={active ? 'page' : undefined}
                class={active ? 'dock-active' : ''}
                onClick={() => go(id)}
              >
                <Icon class="size-[1.2em]" />
                <span class="dock-label">{item.label}</span>
              </button>
            )
          })}
        </div>
      </div>

      <div class="drawer-side z-40">
        <label for="nav-drawer" class="drawer-overlay" aria-label="关闭导航菜单" />
        <div class="flex min-h-full w-64 flex-col bg-base-100">
          {/* 侧栏顶部放品牌 + 关闭按钮（窄屏抽屉打开时需要一个明确的关闭入口） */}
          <div class="flex items-center justify-between gap-2 p-3">
            <span class="flex items-center gap-2 px-1">
              <span class="text-primary">
                <IconMenuBrand />
              </span>
              <span class="font-medium">mcloud</span>
            </span>
            <label
              for="nav-drawer"
              class="btn btn-square btn-ghost btn-sm drawer-button lg:hidden"
              aria-label="关闭导航菜单"
            >
              <IconClose class="size-4" />
            </label>
          </div>
          <ul class="menu w-full flex-1 gap-1 p-3 pt-0">
            {NAV_ITEMS.map((item) => navLink(item.id, () => go(item.id)))}
          </ul>
          {/*
            侧栏底部 = **后端自述**（`GET /api/status`）。
            这里原先放的是「用户名 badge + 角色」，而初始用户的 name 与 role
            字面都是 `admin`，于是左下角并排两个 admin（2026-09-21 用户点名）——
            同一件事写两遍没有任何信息量。身份收敛到 navbar 那一处。

            ⚠ 点的红绿**只**表示"这次问到了没有"，不表示业务成败：
            `lastRun.status` 是存储层生命周期（done 可以是"部分成/需人工"），
            拿它染色就是替后端编造一个它没给过的结论。
          */}
          <div class="border-t border-base-300 p-3 text-xs leading-5">
            <div class="flex items-center gap-2 px-1">
              <span class={backendDot.value} aria-hidden="true" />
              <span class="font-medium">{backendHead.value}</span>
            </div>
            {backendLine2.value ? <div class="px-1 opacity-70">{backendLine2.value}</div> : null}
            {backendLine3.value ? <div class="px-1 opacity-70">{backendLine3.value}</div> : null}
          </div>
        </div>
      </div>
    </div>
  )

  const loading = () => (
    <div class="flex min-h-screen items-center justify-center bg-base-200">
      <span class="loading loading-spinner loading-lg text-primary" />
    </div>
  )

  return (
    <>
      {view.value === 'loading' ? loading() : null}
      {view.value === 'setup' || view.value === 'login' ? (
        <AuthPage
          mode={view.value === 'setup' ? 'setup' : 'login'}
          minPasswordLen={minPasswordLen.value}
          theme={theme.value}
          onToggleTheme={onToggleTheme}
          onDone={(u, msg) => {
            user.value = u
            notice.value = msg
            view.value = 'app'
            void loadStatus()
          }}
        />
      ) : null}
      {view.value === 'app' ? shell() : null}
    </>
  )
})

/** 侧栏品牌标记。单独放这里是因为它只在侧栏用一次。 */
const IconMenuBrand = () => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    viewBox="0 0 24 24"
    fill="currentColor"
    class="size-5"
    aria-hidden="true"
  >
    <path d="M7 18a4.5 4.5 0 0 1-.4-9A5.5 5.5 0 0 1 17.3 9.6 4.2 4.2 0 0 1 17 18H7Z" />
  </svg>
)
