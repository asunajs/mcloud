/**
 * 应用外壳。
 *
 * 布局骨架照抄设计规范 §4.3（`drawer` 三层结构），**不要改顺序**。
 * 这里只负责：导航状态、主题开关、后端自述、把页面塞进 `main` —— 不长内容。
 */

import { createComponent, onMounted, onUnmounted, ref, watch } from 'lite'
import { getCurrentSession, getStatus, logoutSession, type ServiceStatus } from './api'
import { authState, currentUser, noteAuthOk, noteAuthRequired } from './ui/auth-signal'
import { DOCK_IDS, NAV_ITEMS, navItem, parsePath, type PageId } from './ui/nav'
import { navigate } from './ui/router'
import { refreshNow } from './ui/page-refresh'
import { IconBrand, IconClose, IconLogout, IconMenu, IconMoon, IconSun, IconRefresh } from './ui/icons'
import { currentTheme, toggleTheme, type Theme } from './ui/theme'
import { fmtEpochMs } from './ui/time'
import { ToastView } from './ui/toast-view'
import { ErrorDialogHost } from './ui/error-dialog-view'
import { ConfirmDialog, closeDialog, openDialog } from './ui/dialog'
import { AuthPage } from './pages/auth-page'

/**
 * 页面路由表：**每个页面一个动态 `import()`**（Vite 据此切出独立 chunk）。
 *
 * ## 为什么要拆
 *
 * 改之前 10 个页面全是静态 import，于是那 8 个非首屏页面的代码
 * （实测渲染长度 **119.6 kB / 模块图 191.0 kB = 62.6%**）全被塞进首屏那**一个**
 * chunk（130,575 B raw / 34,901 B gzip）。而首屏真正需要的只有"外壳 + 当前这一页"。
 *
 * 拆开之后：初始 chunk 只剩外壳 + 当页；切页时才取那一块（本机 ~1 ms，
 * 因为服务端发的是构建期 `.gz` 边车，见 `web/scripts/gzip-sidecars.mjs`）。
 *
 * ## 为什么每项自己包 `createComponent`
 *
 * 各页 props 形状不同（只有总览页与历史页要 `onGo`）。写成一张
 * `Component<Record<string, unknown>>` 表，要么每个调用点多一次 `as`，
 * 要么把 `onGo` 强塞给不接受的页面。放进各自的闭包里，类型是**自然**对的。
 *
 * ⚠ `AuthPage` **刻意不懒加载**：登录/初始化页正是"未登录时的首屏"，
 * 懒加载等于给用户打开就要看的那一屏多加一次往返。
 */
type RouteCtx = { go: (id: PageId) => void }

const ROUTES: Record<PageId, (ctx: RouteCtx) => Promise<() => unknown>> = {
  dashboard: async (ctx) => {
    const { DashboardPage } = await import('./pages/dashboard-page')
    return () => createComponent(DashboardPage, { onGo: ctx.go })
  },
  accounts: async () => {
    const { AccountsPage } = await import('./pages/accounts-page')
    return () => createComponent(AccountsPage, {})
  },
  tasks: async () => {
    const { TasksPage } = await import('./pages/tasks-page')
    return () => createComponent(TasksPage, {})
  },
  exchange: async () => {
    const { ExchangePage } = await import('./pages/exchange-page')
    return () => createComponent(ExchangePage, {})
  },
  'live-room': async () => {
    const { LiveRoomPage } = await import('./pages/live-room-page')
    return () => createComponent(LiveRoomPage, {})
  },
  schedules: async () => {
    const { SchedulesPage } = await import('./pages/schedules-page')
    return () => createComponent(SchedulesPage, {})
  },
  pipelines: async () => {
    const { PipelinesPage } = await import('./pages/pipelines-page')
    return () => createComponent(PipelinesPage, {})
  },
  history: async (ctx) => {
    const { HistoryPage } = await import('./pages/history-page')
    return () => createComponent(HistoryPage, { onGo: ctx.go })
  },
  logs: async () => {
    const { LogsPage } = await import('./pages/logs-page')
    return () => createComponent(LogsPage, {})
  },
  settings: async () => {
    const { SettingsPage } = await import('./pages/settings-page')
    return () => createComponent(SettingsPage, {})
  },
}

/**
 * navbar 上那个「刷新当前页」按钮（用户口径 2026-10-03：「刷新做成图标放上面」）。
 *
 * # 它点的还是**那一页原来的 `load()`**
 *
 * 页面自己把 `load()` 登记给 `ui/page-refresh.ts`，这里只是触发器 ⇒ 任务页正在看的
 * 日志、勾好的账号、编排页填了一半的表单、滚动位置**都不会丢** ✓。
 * ⚠ 别改成「重新挂载当前页」—— 那等于把这些状态全清掉 ✗。
 *
 * # 为什么这里不读信号、也没有转圈/置灰
 *
 * 登记的写入发生在**页面挂载的那一刻**，而那正是外壳在渲染这一页的过程中 ⇒
 * 那一层若用 lite 信号，会**重入外壳的渲染**，把页面槽位的替换逻辑打断：
 * 切页时旧页面不再被替换、新页面被追加 ✗（实测卡片数 64 → 108 → 153 → …一路累加，
 * 真 DOM 验收从 6 条全绿掉到 28 条失败）。所以那里用的是**普通变量**，
 * 点击时现读（`refreshNow()`）—— 动作永远是对的，代价是外观没法跟着加载态变，
 * 于是干脆不做转圈/置灰：页面自己的 `load()` 是幂等的，重复点不会出问题 ✓。
 * 完整机理与两条实测见 `ui/page-refresh.ts` 的文件头。
 *
 * 每个页面都登记（九个页面 + 任务配置子页），所以这里不必做「没登记就隐藏」。
 */
const RefreshButton = (): JSX.Element => (
  <button
    type="button"
    class="btn btn-square btn-ghost btn-sm"
    aria-label="刷新当前页"
    title="刷新当前页"
    onClick={() => refreshNow()}
  >
    <IconRefresh class="size-5" />
  </button>
)

export default () => {
  const page = ref<PageId>(parsePath(location.pathname))
  const theme = ref<Theme>(currentTheme())
  const drawerOpen = ref(false)
  /**
   * PC 上是否把侧边栏折成"图标轨道"（用户口径 2026-10-02：「pc端可以折叠侧边栏，点击logo展开」）。
   *
   * ⚠ 只管 PC：折叠那几段样式整段套在 `@media (min-width: 64rem)` 里
   * （见 app.css「PC 侧边栏折叠」一节）。移动端是抽屉，由上面那条 checkbox 控制，
   * 两者互不干扰 —— 所以移动端点 logo 不会有反应，那是**对的**。
   *
   * ⚠ 偏好存 localStorage：折叠是"我就这么用"的习惯，刷新一次就弹回来会让人反复折。
   */
  const sidebarRail = ref(localStorage.getItem('mcloud-sidebar') === 'rail')
  const toggleSidebar = () => {
    sidebarRail.value = !sidebarRail.value
    localStorage.setItem('mcloud-sidebar', sidebarRail.value ? 'rail' : 'full')
  }

  /** 后端自述。三态：还没问 / 问到了 / 问不到。 */
  const backend = ref<ServiceStatus | null>(null)
  const backendFailed = ref(false)

  const loggingOut = ref(false)

  /**
   * 地址变了（浏览器前进/后退、手敲地址、或 `navigate()` 广播）。
   *
   * ⚠ 名字从 `onHashChange` 改过来不是修辞：`pushState` 自己**不发**事件，
   * 所以 `navigate()` 里补发了一次 `popstate` —— 这条监听器就是**唯一**的切页入口。
   */
  const onNavigate = () => {
    page.value = parsePath(location.pathname)
    // 移动端点了导航就把抽屉收起来，否则内容被盖住
    drawerOpen.value = false
  }

  const onToggleTheme = () => {
    theme.value = toggleTheme()
  }

  const go = (id: PageId) => {
    navigate(navItem(id).href)
  }

  // ── 动态路由的装载状态（见文件头 `ROUTES`）────────────────────────────
  /**
   * 当前页面的**渲染闭包**（装载完成后才有值）。
   *
   * 存"能建出这一页"的闭包而不是组件本身：`createComponent` 要在槽的 effect 里
   * 被调用，才会得到正确的实例与 dispose 语义（与改之前那个三元链同一条口径）。
   */
  const pageRender = ref<(() => unknown) | null>(null)
  /** 装载失败（chunk 404 / 网络断）。不静默停在上一个转圈上。 */
  const pageFailed = ref(false)

  /**
   * 已装载过的页面：闭包复用，所以**切回来是同步的**（不再有一次往返）。
   *
   * 用普通 `Map` 而不是信号：它只是缓存，不参与渲染依赖。做成信号的话，
   * 每次装载都会多触发一轮无关的更新。
   */
  const routeCache = new Map<PageId, () => unknown>()

  const loadPage = async (id: PageId) => {
    pageFailed.value = false

    const hit = routeCache.get(id)
    if (hit) {
      pageRender.value = hit
      return
    }

    // 先清空：切页时旧页必须立刻让位给"加载中"，否则会看到上一页的残影。
    pageRender.value = null
    try {
      const render = await ROUTES[id]({ go })
      routeCache.set(id, render)
      // ⚠ 竞态：装载期间用户可能已经切走了。迟到的结果不能覆盖新页面 ——
      // 少了这句的表现是"快速连点两下，最终停在先点的那页"。
      if (page.value === id) pageRender.value = render
    } catch (e) {
      if (page.value === id) pageFailed.value = true
      console.error('[路由] 页面装载失败', id, e)
    }
  }

  /**
   * 悬停/聚焦时预取。切页在缓存命中时是同步的 —— 预取把"点下去才等"
   * 变成"手移过去就已经在路上"，而代价只是一次几毫秒的本地请求。
   */
  const prefetch = (id: PageId) => {
    if (routeCache.has(id) || page.value === id) return
    void ROUTES[id]({ go })
      .then((render) => routeCache.set(id, render))
      .catch(() => {
        // 预取失败无所谓：真点下去时 `loadPage` 会再试一次并把错误显示出来
      })
  }

  const pageLoading = () => (
    <div class="flex h-64 items-center justify-center">
      <span class="loading loading-lg text-primary" />
    </div>
  )

  const pageLoadFailed = () => (
    <div class="flex h-64 flex-col items-center justify-center gap-3">
      <p class="text-sm opacity-70">页面加载失败，请检查网络后重试</p>
      <button type="button" class="btn btn-sm btn-primary" onClick={() => void loadPage(page.value)}>
        重试
      </button>
    </div>
  )

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
      noteAuthOk(me.name, me.isAdmin)
    } catch {
      // 401/503 已由 `request` 推进状态。其余错误（如后端 500）不该被
      // 当成"去登录" —— 那是后端有问题。但也不能一直停在转圈上，
      // 所以兜底置成 login：那里至少有个能操作的面板，口令错了会有明确报错。
      if (authState.value === 'checking') noteAuthRequired('login')
    }
  }

  onMounted(() => {
    window.addEventListener('popstate', onNavigate)
    void probeSession()
    // 已经放行时（例如热重载后重挂）立刻装当前页；否则由下面那个 watch 负责。
    if (authState.value === 'ready') void loadPage(page.value)
  })

  /**
   * 登录态一旦放行，补拉一次后端自述。
   *
   * 不能只靠 `onMounted`：那时还没登录，`/api/status` 回 503，
   * 侧栏底部会一直停在"后端连接中断" —— 而用户明明刚登录成功。
   */
  watch(authState, (s) => {
    if (s === 'ready') {
      void loadStatus()
      // 放行的同时把当前页装上：未放行时整块外壳都不渲染，
      // 提前装载只会在登录页上白下一份永远不会显示的 chunk。
      void loadPage(page.value)
    }
  })

  /**
   * 切页就装载。
   *
   * 依赖放在 `page`（由 `onNavigate` 推进）而不是直接在 `go()` 里调 ——
   * 这样浏览器前进/后退、手敲地址、以及页面里其它地方的 `navigate()` 调用
   * 都走同一条路。少一处调用点，就少一处"某种方式切页不生效"。
   */
  watch(page, (id) => {
    void loadPage(id)
  })
  onUnmounted(() => {
    window.removeEventListener('popstate', onNavigate)
  })

  /**
   * 侧栏底部那一行：只报**后端确实知道**的事实（产品名+版本、最近一次执行）。
   * 不放账号数/任务数 —— 那是各页面的内容，搬进常驻侧栏等于把同一个数字
   * 出现在两个地方。取的时机只有"进页面时"一次，**不轮询**：
   * 每分钟自刷只会让人以为那个时间戳在动。
   *
   * ⚠⚠ 这三件必须是**函数**，不能是 `const x = ...` 算好的值。
   *
   * lite 没有重渲染：**组件体只跑一次**，所以 `const` 只被求值一次 ——
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
          // 预取：手移到导航上时那一块 chunk 就已经在下（见 `prefetch`）。
          onMouseEnter={() => prefetch(id)}
          onFocus={() => prefetch(id)}
        >
          <Icon class="size-[1.15em]" />
          {/* 折成图标轨道时这行藏起来（只留图标）。类名是字面量，不拼接。 */}
          <span class="rail-hide">{item.label}</span>
        </button>
      </li>
    )
  }

  // ── 认证闸 ────────────────────────────────────────────────────────────
  //
  // ⚠⚠ 这里**绝不能**写成组件体顶层的 `if (...) return <X/>`。
  //
  // lite 没有重渲染：**组件体只跑一次**，所以顶层 `return` 只被求值一次，
  // 那一刻 `authState` 还是 `checking` ⇒ 永远返回那个转圈，
  // 登录闸再怎么更新状态都不会换屏。
  //
  // 症状极具迷惑性：网络面板里 `/api/session` 明明回了 503、
  // `authState` 也确实变成了 `setup`，**但界面就是不动** ——
  // 因为没有报错、没有异常，只是"渲染决定被冻住了"。
  //
  // 正确写法是把判断放进 JSX 的表达式里（下面的三元），
  // lite 会为它建立响应式绑定（`setNodes`）。同一文件里切页那段就是这么写的。
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
      // 这里**不需要** `key`：整个 `{authGate()}` 是一个动态槽，`authState` 一变就重跑，
      // 重跑就是 `createComponent(AuthPage, …)` **新建一个实例**（旧的连同它的 effect 一起 dispose）。
      // 本框架没有"虚拟 DOM 比一比再决定复不复用"那一步，所以也就没有"得靠 key 逼它换实例"。
      // ⚠ 别把 key 加回来：编译器会直接抛错（`lite/compiler.ts` 的 `claimKey`）。
      return <AuthPage mode="setup" />
    }
    if (authState.value === 'login') {
      return <AuthPage mode="login" />
    }
    return null
  }

  return (
    <>
      {authGate()}

      {authState.value !== 'ready' ? null : (
    <div class={`drawer lg:drawer-open ${sidebarRail.value ? 'rail' : ''}`}>
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
            {/*
              页面标题**只在这里**显示一次（`navItem(id).title`）。
              ⚠ 页面里不要再写一遍 `<h1>` —— 2026-10-03 之前八页各写了一次，
              于是每页顶部同一句话挨着出现两遍 ✗。留在这里的理由：它是 sticky 的
              （滚动时还在），而窄屏侧栏是抽屉 ⇒ 它是"我在哪一页"的唯一指示 ✓。
            */}
            <span class="px-1 text-base font-medium">{navItem(page.value).title}</span>
          </div>
          <div class="navbar-end gap-1">
            <RefreshButton />
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
        {/*
          ⚠ 这一整串三元**不需要** `key`（也别加，加了编译器抛错）。
          它是**一个动态槽**：槽读到的信号一变就重跑 ⇒ 走到的分支
          `pageRender()` 里的 `createComponent(那一页, …)` **新建实例**，旧实例的节点被移除
          同时 `disposeTree`（连带它的 effect 与 `onUnmounted`）。所以"切走再切回来一定是干净的一页"
          由 `setNodes` 保证，不靠 diff 时的那一次 key 比对。

          槽现在读的是 `pageRender` / `pageFailed`（不再是 `page`）：切页时
          `loadPage` 会先把它置 `null`（转圈），装载完再放上新的闭包 ——
          两个信号都在这里被读到，所以"切页 → 转圈 → 换页"整条链都能推动槽重跑。
        */}
        <main class="flex-1 bg-base-200 p-4 pb-24 md:p-6 md:pb-6">
          {pageRender.value ? pageRender.value() : pageFailed.value ? pageLoadFailed() : pageLoading()}
        </main>

        {/*
          移动端底部导航（daisyUI 5 的 `dock`，旧名 `btm-nav` 已废弃）。
          全放真实页面，不放「更多」—— 它点开的是抽屉，而抽屉在 navbar
          左上角的汉堡包那里已经有入口，重复一格不换来任何可达性。
        */}
        <div class="dock dock-md safe-bottom z-30 border-t border-base-300 bg-base-100 md:hidden">
          {/*
            这里是**块体** `.map`（要先算 `item` / `Icon` 才能当标签名用），
            而块体不走 `createFor`：`return` 之前那几句（`const active = page.value === id`）
            是在 effect **之外**读信号的，列表一旦不重建，`active` 就冻在建行那一刻的值上。
            所以这一格走"整块重铺"那条路 —— 四颗按钮，代价固定。
            ⚠ 别在这儿加 `key`：这条路没有复用键，加了只是骗自己（编译器会抛错）。
          */}
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
          <div class="flex items-center justify-between gap-2 p-3">
            {/*
              点 logo 折叠 / 展开侧边栏（用户口径 2026-10-02）。
              ⚠ 它在移动端点了没反应，是对的设计：折叠那几段样式只在 lg 以上生效，
              移动端这里是抽屉，收起靠遮罩或右边那个 ✕。
            */}
            <button
              type="button"
              class="rail-logo flex items-center gap-2 rounded-field px-1"
              title={sidebarRail.value ? '展开侧边栏' : '折叠侧边栏'}
              aria-label={sidebarRail.value ? '展开侧边栏' : '折叠侧边栏'}
              aria-expanded={!sidebarRail.value}
              onClick={() => toggleSidebar()}
            >
              <span class="text-primary">
                <IconBrand class="size-5" />
              </span>
              <span class="rail-hide font-medium">mcloud</span>
            </button>
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
              <span class="rail-hide font-medium">{backendHead()}</span>
            </div>
            {backendLine2() ? <div class="rail-hide px-1 opacity-70">{backendLine2()}</div> : null}
            {currentUser.value ? (
              <div class="rail-hide px-1 opacity-70">以 {currentUser.value} 的身份</div>
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
}
