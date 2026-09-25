/**
 * 应用外壳。
 *
 * 布局骨架照抄设计规范 §4.3（`drawer` 三层结构），**不要改顺序**。
 * 这里只负责：导航状态、主题开关、后端自述、把页面塞进 `main` —— 不长内容。
 */

import { defineVaporComponent, onMounted, onUnmounted, ref } from 'vue'
import { getStatus, type ServiceStatus } from './api'
import { DOCK_IDS, NAV_ITEMS, navItem, parseHash, type PageId } from './ui/nav'
import { IconBrand, IconClose, IconMenu, IconMoon, IconSun } from './ui/icons'
import { currentTheme, toggleTheme, type Theme } from './ui/theme'
import { fmtEpochMs } from './ui/time'
import { ToastView } from './ui/toast-view'
import { DashboardPage } from './pages/dashboard-page'
import { TasksPage } from './pages/tasks-page'
import { HistoryPage } from './pages/history-page'
import { SettingsPage } from './pages/settings-page'

export default defineVaporComponent(() => {
  const page = ref<PageId>(parseHash(location.hash))
  const theme = ref<Theme>(currentTheme())
  const drawerOpen = ref(false)

  /** 后端自述。三态：还没问 / 问到了 / 问不到。 */
  const backend = ref<ServiceStatus | null>(null)
  const backendFailed = ref(false)

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

  onMounted(() => {
    window.addEventListener('hashchange', onHashChange)
    void loadStatus()
  })
  onUnmounted(() => {
    window.removeEventListener('hashchange', onHashChange)
  })

  /**
   * 侧栏底部那一行：只报**后端确实知道**的事实（产品名+版本、最近一次执行）。
   * 不放账号数/任务数 —— 那是各页面的内容，搬进常驻侧栏等于把同一个数字
   * 出现在两个地方。取的时机只有"进页面时"一次，**不轮询**：
   * 每分钟自刷只会让人以为那个时间戳在动。
   */
  const backendDot = backendFailed.value
    ? 'status status-error'
    : backend.value
      ? 'status status-success'
      : 'status status-neutral animate-pulse'

  const backendHead = backendFailed.value
    ? '后端连接中断'
    : backend.value
      ? `${backend.value.name} v${backend.value.version}`
      : '正在问后端…'

  const backendLine2 = !backendFailed.value && backend.value
    ? backend.value.lastRun
      ? `最近执行 ${fmtEpochMs(backend.value.lastRun.started_at_ms)}`
      : '尚无执行记录'
    : ''

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

  return (
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
          <div class="navbar-end gap-1">{themeToggle()}</div>
        </div>

        {/* pb-24 是为移动端 dock 让位；md 以上没有 dock，用普通内边距 */}
        <main class="flex-1 bg-base-200 p-4 pb-24 md:p-6 md:pb-6">
          {page.value === 'dashboard' ? (
            <DashboardPage onGo={go} />
          ) : page.value === 'tasks' ? (
            <TasksPage />
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
              <span class={backendDot} aria-hidden="true" />
              <span class="font-medium">{backendHead}</span>
            </div>
            {backendLine2 ? <div class="px-1 opacity-70">{backendLine2}</div> : null}
          </div>
        </div>
      </div>

      <ToastView />
    </div>
  )
})
