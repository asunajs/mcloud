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
import { currentSession, setupStatus, signOut, type User } from './api'
import { AuthPage } from './pages/AuthPage'
import { DashboardPage } from './pages/DashboardPage'
import { PlaceholderPage } from './pages/PlaceholderPage'
import { IconClose, IconMenu, IconMoon, IconMore, IconSun } from './ui/icons'
import { DOCK_IDS, NAV_ITEMS, navItem, parseHash, type PageId } from './ui/nav'
import { currentTheme, toggleTheme, type Theme } from './ui/theme'

type View = 'loading' | 'setup' | 'login' | 'app'

export default defineVaporComponent(() => {
  const view = ref<View>('loading')
  const user = ref<User | null>(null)
  const minPasswordLen = ref(8)
  /** 当前主题。以本地状态驱动图标，避免每次渲染都去读 DOM。 */
  const theme = ref<Theme>(currentTheme())

  /** 当前页面。用 hash 路由（无依赖、可刷新、可分享链接）。 */
  const page = ref<PageId>(parseHash(location.hash))
  /** 抽屉开关。用 `drawer-open` 类而不是 JS 控制显隐。 */
  const drawerOpen = ref(false)

  const notice = ref('')
  const error = ref('')

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
   *  - `drawer-side` 必须 `z-40`，否则被 navbar 的 `shadow-sm` 压住；
   *  - `lg:drawer-open` 加在**根 div** 上；
   *  - `min-h-screen` 在 `drawer-content`，且 `flex-col` + `main` 用 `flex-1`，
   *    否则窄屏时底部 dock 会浮在半空。
   */
  const shell = () => (
    <div class={drawerOpen.value ? 'drawer drawer-open lg:drawer-open' : 'drawer lg:drawer-open'}>
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
              class="btn btn-square btn-ghost lg:hidden"
              aria-label="打开导航菜单"
            >
              <IconMenu class="size-5" />
            </label>
            <span class="px-1 text-base font-medium">{current.value.title}</span>
          </div>
          <div class="navbar-end gap-1">
            {themeToggle()}
            <span class="badge badge-soft badge-neutral hidden sm:inline-flex">
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
          ) : (
            <PlaceholderPage id={page.value} onGo={go} />
          )}
        </main>

        {/*
          移动端底部导航（daisyUI 5 的 `dock`，旧名 `btm-nav` 已废弃）。
          只放 5 项 —— 9 个页面全塞进来会挤到不可点（§4.4）。
          第 5 项「更多」打开抽屉，让全部页面仍然可达。
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
          <button type="button" onClick={() => (drawerOpen.value = true)}>
            <IconMore class="size-[1.2em]" />
            <span class="dock-label">更多</span>
          </button>
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
              class="btn btn-square btn-ghost btn-sm lg:hidden"
              aria-label="关闭导航菜单"
            >
              <IconClose class="size-4" />
            </label>
          </div>
          <ul class="menu w-full flex-1 gap-1 p-3 pt-0">
            {NAV_ITEMS.map((item) => navLink(item.id, () => go(item.id)))}
          </ul>
          {/* 侧栏底部显示当前身份：窄屏收起抽屉后就看不到 navbar 上的用户名了 */}
          <div class="border-t border-base-300 p-3">
            <div class="flex items-center gap-2 px-1 text-sm">
              <span class="badge badge-soft badge-primary">{user.value?.name}</span>
              <span class="opacity-60">{user.value?.role}</span>
            </div>
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
