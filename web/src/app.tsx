/**
 * 应用外壳。
 *
 * 布局骨架照抄设计规范 §4.3（`drawer` 三层结构），**不要改顺序**。
 * 这里只负责：导航状态、主题开关、后端自述、把页面塞进 `main` —— 不长内容。
 */

import { defineVaporComponent, onMounted, onUnmounted, ref, watch } from 'vue'
import { getCurrentSession, getStatus, logoutSession, type ServiceStatus } from './api'
import { authState, currentUser, noteAuthOk, noteAuthRequired } from './ui/auth-signal'
import { DOCK_IDS, NAV_ITEMS, navItem, parseHash, type PageId } from './ui/nav'
import { IconBrand, IconClose, IconLogout, IconMenu, IconMoon, IconSun } from './ui/icons'
import { currentTheme, toggleTheme, type Theme } from './ui/theme'
import { fmtEpochMs } from './ui/time'
import { ToastView } from './ui/toast-view'
import { ErrorDialogHost } from './ui/error-dialog-view'
import { ConfirmDialog, closeDialog, openDialog } from './ui/dialog'
import { DashboardPage } from './pages/dashboard-page'
import { AccountsPage } from './pages/accounts-page'
import { TasksPage } from './pages/tasks-page'
import { ExchangePage } from './pages/exchange-page'
import { LiveRoomPage } from './pages/live-room-page'
import { SchedulesPage } from './pages/schedules-page'
import { PipelinesPage } from './pages/pipelines-page'
import { HistoryPage } from './pages/history-page'
import { SettingsPage } from './pages/settings-page'
import { AuthPage } from './pages/auth-page'

export default defineVaporComponent(() => {
  const page = ref<PageId>(parseHash(location.hash))
  const theme = ref<Theme>(currentTheme())
  const drawerOpen = ref(false)

  /** 后端自述。三态：还没问 / 问到了 / 问不到。 */
  const backend = ref<ServiceStatus | null>(null)
  const backendFailed = ref(false)

  const loggingOut = ref(false)

  const onHashChange = () => {
    page.value = parseHash(location.hash)
    // 移动端点了导航就把抽屉收起来，否则内容被盖住
    drawerOpen.value = false
  }

  const onToggleTheme = () => {
    theme.value = toggleTheme()
  }

  const go = (id: PageId) => {
    location.hash = navItem(id).href
  }

  const loadStatus = async () => {
    backendFailed.value = false
    try {
      backend.value = await getStatus()
    } catch {
      // 这里只关心"问到没有"。红的语义是"后端挂了"，
      // 所以取不到才置红；不要把业务失败也染进来。
      backendFailed.value = true
    }
  }

  /**
   * 首屏探一次登录态。
   *
   * ⭐ 判据是 `GET /api/session`，**不是** `GET /api/setup`。
   * 两者的区别很要紧：
   *
   * | 接口 | 未初始化 | 已初始化未登录 | 已登录 |
   * |---|---|---|---|
   * | `GET /api/setup` | 200 `{initialized:false}` | 200 `{initialized:true}` | 同左 |
   * | `GET /api/session` | **503** | **401** | 200 |
   *
   * 只用 `/api/setup` 的话它**任何时候都回 200**，于是"已初始化但没登录"
   * 无法与"已登录"区分 —— 应用会以为可以进，然后每个页面各撞一次 401。
   *
   * 这里**不自己判状态码**：`request` 已在 401/503 时推进 `authState`
   * （见 `api.ts` 的 `classifyAuthFailure`）。好处是"哪些码算认证失败"
   * 只有**一个**地方定义。
   */
  const probeSession = async () => {
    try {
      const me = await getCurrentSession()
      // ⚠ 成功路径**必须**显式放行。
      //
      // `request` 只在失败时推进状态（401/503），成功时它什么都不做 ——
      // 这是刻意的：`/api/version`、`/api/health` 这类公开接口未初始化时
      // 也回 200，如果"任何成功都放行"，应用会从"要建管理员"错误地翻成
      // "可用"，然后每个页面各撞一鼻子 503。
      //
      // 所以"放行"只能由**明确的认证动作**宣布。这里就是其中一处。
      //
      // 漏掉这一句的后果：带着有效 cookie 刷新页面会**永远停在转圈上** ——
      // 而它不报错、不抛异常，网络面板里全是 200，极难查。
      noteAuthOk(me.name)
    } catch {
      // 401/503 已由 `request` 推进状态。其余错误（如后端 500）不该被
      // 当成"去登录" —— 那是后端有问题。但也不能一直停在转圈上，
      // 所以兜底置成 login：那里至少有个能操作的面板，口令错了会有明确报错。
      if (authState.value === 'checking') noteAuthRequired('login')
    }
  }

  onMounted(() => {
    window.addEventListener('hashchange', onHashChange)
    void probeSession()
  })

  /**
   * 登录态一旦放行，补拉一次后端自述。
   *
   * 不能只靠 `onMounted`：那时还没登录，`/api/status` 回 503，
   * 侧栏底部会一直停在"后端连接中断" —— 而用户明明刚登录成功。
   */
  watch(authState, (s) => {
    if (s === 'ready') void loadStatus()
  })
  onUnmounted(() => {
    window.removeEventListener('hashchange', onHashChange)
  })

  /**
   * 侧栏底部那一行：只报**后端确实知道**的事实（产品名+版本、最近一次执行）。
   * 不放账号数/任务数 —— 那是各页面的内容，搬进常驻侧栏等于把同一个数字
   * 出现在两个地方。取的时机只有"进页面时"一次，**不轮询**：
   * 每分钟自刷只会让人以为那个时间戳在动。
   *
   * ⚠⚠ 这三件必须是**函数**，不能是 `const x = ...` 算好的值。
   *
   * Vapor 没有重渲染：setup 只跑一次，所以 `const` 只被求值一次 ——
   * 那一刻 `backend` 还是 `null`（请求刚发出去），于是侧栏**永远**
   * 停在"正在问后端…"。而它不报错、网络面板里 `/api/status` 明明回了 200。
   *
   * 同一文件里的 `themeToggle()` / `navLink()` 都是函数，就是这个道理。
   */
  const backendDot = () =>
    backendFailed.value
      ? 'status status-error'
      : backend.value
        ? 'status status-success'
        : 'status status-neutral animate-pulse'

  const backendHead = () =>
    backendFailed.value
      ? '后端连接中断'
      : backend.value
        ? `${backend.value.name} v${backend.value.version}`
        : '正在问后端…'

  const backendLine2 = () =>
    !backendFailed.value && backend.value
      ? backend.value.lastRun
        ? `最近执行 ${fmtEpochMs(backend.value.lastRun.started_at_ms)}`
        : '尚无执行记录'
      : ''

  /**
   * 登出。
   *
   * ⚠ 先清空本地显示名，再让 `authState` 归到 `login`。
   * 顺序反了的话，外壳已经拆掉、而 `currentUser` 还留着上一个人的名字 ——
   * 下一次登录的第一个瞬间会闪一下旧名字。
   *
   * 失败也要**照样**把界面切到登录页：cookie 可能已经过期，
   * 那这时"停在原地"是错的，用户会以为自己还在登录状态。
   */
  const doLogout = async () => {
    loggingOut.value = true
    try {
      await logoutSession()
    } catch {
      // 忽略：登出是幂等的，后端说"本来就没登录"也算成功
    } finally {
      loggingOut.value = false
      currentUser.value = ''
      closeDialog('confirm-logout')
      noteAuthRequired('login')
    }
  }

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

  const navLink = (id: PageId) => {
    const item = navItem(id)
    const Icon = item.icon
    const active = page.value === id
    return (
      <li>
        {/*
          `min-h-11 md:min-h-0`：窄屏 44px 触控目标（规范 §6.1 硬要求），
          桌面回落 daisyUI 默认的 33px 密度。
          daisyUI 的 menu 条目默认只有 33px —— 不补这一句，
          移动端抽屉里唯一的导航入口就在点不中的高度上。
          类名全部是字面量，没有拼接（§9.1①）。
        */}
        <button
          type="button"
          class={`min-h-11 md:min-h-0 ${active ? 'menu-active' : ''}`}
          aria-current={active ? 'page' : undefined}
          onClick={() => go(id)}
        >
          <Icon class="size-[1.15em]" />
          {item.label}
        </button>
      </li>
    )
  }

  // ── 认证闸 ────────────────────────────────────────────────────────────
  //
  // ⚠⚠ 这里**绝不能**写成 setup 顶层的 `if (...) return <X/>`。
  //
  // Vapor 没有重渲染：**setup 只跑一次**，所以顶层 `return` 只被求值一次，
  // 那一刻 `authState` 还是 `checking` ⇒ 永远返回那个转圈，
  // 登录闸再怎么更新状态都不会换屏。
  //
  // 症状极具迷惑性：网络面板里 `/api/session` 明明回了 503、
  // `authState` 也确实变成了 `setup`，**但界面就是不动** ——
  // 因为没有报错、没有异常，只是"渲染决定被冻住了"。
  //
  // 正确写法是把判断放进 JSX 的表达式里（下面的三元），
  // Vapor 会为它建立响应式绑定。同一文件里切页那段就是这么写的。
  //
  // 未放行时**整个外壳都不渲染**：侧栏、dock、导航全不出现。
  // 渲染了再遮住的话，用户能点到导航、能触发请求，然后每个请求都 401 ——
  // 那是"看起来能用但处处失败"，比干脆不给点更糟。
  const authGate = () => {
    if (authState.value === 'checking') {
      return (
        <div class="flex min-h-screen items-center justify-center bg-base-200">
          <span class="loading loading-lg text-primary" />
        </div>
      )
    }
    if (authState.value === 'setup') {
      // `key` 不能省：`setup` 与 `login` 是同一个组件的两个实例。
      // 没有 key 时 Vapor 可能复用实例，而 `isSetup` 是在 setup 里
      // 算好的一次性值，复用会让它停留在旧模式上。
      return <AuthPage key="setup" mode="setup" />
    }
    if (authState.value === 'login') {
      return <AuthPage key="login" mode="login" />
    }
    return null
  }

  return (
    <>
      {authGate()}

      {authState.value !== 'ready' ? null : (
    <div class="drawer lg:drawer-open">
      {/*
        drawer-toggle 是 checkbox，靠 <label for> 驱动 —— 不用 JS 控制显隐。
        ⚠ `drawer-open` 是"大屏常驻分栏"的 modifier，**不是**"当前展开"；
        它只能以 `lg:` 前缀出现，绝不能条件挂载（挂了会把内容区挤瘪）。
      */}
      <input
        id="nav-drawer"
        type="checkbox"
        class="drawer-toggle"
        checked={drawerOpen.value}
        onChange={(e) => {
          drawerOpen.value = (e.target as HTMLInputElement).checked
        }}
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
            <span class="px-1 text-base font-medium">{navItem(page.value).title}</span>
          </div>
          <div class="navbar-end gap-1">
            {themeToggle()}
            {/*
              登出放在 navbar 而不是侧栏：侧栏在窄屏是抽屉，
              而"我登不出去"是个必须随时可达的动作。
            */}
            <button
              type="button"
              class="btn btn-square btn-ghost btn-sm"
              aria-label="退出登录"
              title="退出登录"
              onClick={() => openDialog('confirm-logout')}
            >
              <IconLogout class="size-4" />
            </button>
          </div>
        </div>

        {/* pb-24 是为移动端 dock 让位；md 以上没有 dock，用普通内边距 */}
        <main class="flex-1 bg-base-200 p-4 pb-24 md:p-6 md:pb-6">
          {page.value === 'dashboard' ? (
            <DashboardPage onGo={go} />
          ) : page.value === 'accounts' ? (
            <AccountsPage />
          ) : page.value === 'tasks' ? (
            <TasksPage />
          ) : page.value === 'exchange' ? (
            /*
              `key` 与下面几页同理：Vapor 没有重渲染，setup 只跑一次。
              这一页的状态里有**上一次兑换的结论**与选中的账号 ——
              实例被复用会让"切走再切回来"看到上一个账号下的旧结论。
            */
            <ExchangePage key={page.value} />
          ) : page.value === 'live-room' ? (
            /*
              `key` 与下面几页同理：Vapor 没有重渲染，setup 只跑一次。
              这一页**尤其**需要 —— 它自己的状态里有口令与运行结果，
              实例被复用会让"切走再切回来"看到上一趟的残留。
            */
            <LiveRoomPage key={page.value} />
          ) : page.value === 'schedules' ? (
            <SchedulesPage key={page.value} />
          ) : page.value === 'pipelines' ? (
            <PipelinesPage key={page.value} />
          ) : page.value === 'history' ? (
            <HistoryPage onGo={go} />
          ) : (
            /**
             * `key` 不是装饰：Vapor 没有重渲染，setup 只跑一次，
             * 落在同一个 else 分支里的组件切页时不会重建实例。
             */
            <SettingsPage key={page.value} />
          )}
        </main>

        {/*
          移动端底部导航（daisyUI 5 的 `dock`，旧名 `btm-nav` 已废弃）。
          全放真实页面，不放「更多」—— 它点开的是抽屉，而抽屉在 navbar
          左上角的汉堡包那里已经有入口，重复一格不换来任何可达性。
        */}
        <div class="dock dock-md safe-bottom z-30 border-t border-base-300 bg-base-100 md:hidden">
          {DOCK_IDS.map((id) => {
            const item = navItem(id)
            const Icon = item.icon
            const active = page.value === id
            return (
              <button
                type="button"
                key={id}
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
          <div class="flex items-center justify-between gap-2 p-3">
            <span class="flex items-center gap-2 px-1">
              <span class="text-primary">
                <IconBrand class="size-5" />
              </span>
              <span class="font-medium">mcloud-gost</span>
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
            {NAV_ITEMS.map((item) => navLink(item.id))}
          </ul>

          <div class="border-t border-base-300 p-3 text-xs leading-5">
            <div class="flex items-center gap-2 px-1">
              <span class={backendDot()} aria-hidden="true" />
              <span class="font-medium">{backendHead()}</span>
            </div>
            {backendLine2() ? <div class="px-1 opacity-70">{backendLine2()}</div> : null}
            {currentUser.value ? (
              <div class="px-1 opacity-70">以 {currentUser.value} 的身份</div>
            ) : null}
          </div>
        </div>
      </div>

      <ToastView />

      {/* 动作失败弹窗：全局唯一（各页面用 showError(...) 触发） */}
      <ErrorDialogHost />

      <ConfirmDialog
        id="confirm-logout"
        title="退出登录"
        impact={
          <span>
            将结束{' '}
            <b>{currentUser.value || '当前'}</b>{' '}
            在本浏览器的登录状态。正在运行的任务<strong>不受影响</strong>，
            它们在后端继续跑；重新登录即可回到这里。
          </span>
        }
        confirmLabel="退出"
        onConfirm={() => void doLogout()}
        busy={loggingOut.value}
      />
    </div>
      )}
    </>
  )
})
