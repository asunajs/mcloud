/**
 * 「历史」页 —— **历史 + 日志两个 tab**（2026-10-08 用户口径）。
 *
 * # 为什么合
 *
 * 用户原话：「合并这两个（截图圈着侧栏的「**历史**」与「**日志**」），并单独给推送一个」✓
 * ⇒ 侧栏**一减一加**，仍是 10 条 ✓（见 `ui/nav.ts` 的清单注释）。
 * 两页本来就只隔一层：历史是"每次运行的记录 + **那次运行**的日志"，日志是
 * "**整个进程**的日志"（`logs-page.tsx` 文件头那段分工 ✓）—— 同一个问题的两个视角 ✓。
 *
 * # 两个地址都留着（**别删 `/logs`**）
 *
 * `/history` = 历史 tab、`/logs` = 日志 tab ✓。`ui/nav.ts` 的 `PATH_ALIASES` 把
 * `/logs` 也归到 `history` 这一页 ⇒ 书签 / 浏览器历史 / 旧文档里的 `/logs` 照旧打得开，
 * 不 404、也不被甩回总览 ✓（与 `/accounts/push` 那条老地址兼容同一个口径 ✓）。
 *
 * # ⚠⚠ 只渲染当前 tab（这条是硬要求）
 *
 * 两个页面各自都不轻：历史页会 `GET /api/runs`，日志页**每 2 秒**轮询 `/api/logs`
 * 并做窗口化渲染 ✓。把它们**同时**塞进渲染树，就等于"看历史的时候也在轮询日志" ✗
 * —— 泄漏量测（`regress/leak.mjs`）与性能都会跟着变差 ✓。
 * 所以这里是 `? :` 二选一：切 tab 时旧的那份被整棵卸载（`onUnmounted` 跑、
 * `clearInterval` 生效 ✓），新的那份才建 ✓。
 *
 * # ⚠ tab 由**地址**决定，不是本地 `ref`
 *
 * 这样"点 tab = `navigate()` 换地址"与"直接开 `/logs`"是**同一条路** ✓ ——
 * 本地 ref 的话，直接开 `/logs` 会先落在历史 tab（地址与界面不一致 ✗），
 * 而且浏览器后退也切不动 ✓。
 * 推进这个读数的是 `ui/router.ts` 的 `currentPath`（唯一写入点是 `app.tsx` 的
 * `onNavigate` ✓）。
 *
 * # ⚠ 为什么容器**不**跟着地址重建
 *
 * `app.tsx` 的页面槽只在 `PageId` 变时才重跑，而 `/history` 与 `/logs` 都归 `history`
 * ⇒ 切 tab 时容器一直活着，只有里面那个子页面换 ✓。
 * 反过来（把 `currentPath` 读进页面槽、让槽重建容器）在"从别的页切去 `/logs`"时
 * 会多建一份历史页、白跑一次 `/api/runs` ✗ —— 见 `app.tsx` 的 `ROUTES.history` 那条 ⚠。
 *
 * # tab 的样式
 *
 * 用本仓自建的**分组 tab**（`join` + `btn join-item`，见 `tasks-page.tsx` /
 * `exchange-page.tsx` ✓），**不用** daisyUI 的 `.tab` —— 它在 `web/src/app.css` 的
 * `@theme` 里被排除过，实测退化成"一串连在一起的纯文字"（padding/边框全 0 ✗）。
 */
import { ref, watch } from '@asunajs/lite'

import type { PageId } from '../ui/nav'
import { currentPath, navigate } from '../ui/router'
import { HistoryPage } from './history-page'
import { LogsPage } from './logs-page'

export interface HistoryLogsPageProps {
  onGo: (id: PageId) => void
}

/** 两个 tab 的 id。 */
type Tab = 'history' | 'logs'

/** 地址 → tab（`/logs` 以外一律算历史 ✓；容器只在被这两个地址选中时才存在）。 */
const tabOf = (path: string): Tab => (path === '/logs' ? 'logs' : 'history')

/** tab → 地址。 */
const hrefOf = (tab: Tab): string => (tab === 'logs' ? '/logs' : '/history')

export const HistoryLogsPage = (props: HistoryLogsPageProps) => {
  /**
   * ⚠ 初值在**组件体**里读 `currentPath`：组件体只跑一次、不在任何 effect 里
   * ⇒ 不会给外壳那条页面槽挂上依赖 ✓（这正是"容器不跟着地址重建"的关键 ✓）。
   */
  const tab = ref<Tab>(tabOf(currentPath.value))

  /**
   * ⚠ 只在地址落回**本页这两个**时才动 tab：切去 `/settings` 之类的地址时
   * `currentPath` 也会变，而那一刻这一页马上就要被卸载 —— 若不过滤，会先按
   * 新地址把「历史」tab 建出来、白跑一次 `/api/runs` ✗。
   */
  watch(currentPath, (p) => {
    if (p === '/history' || p === '/logs') tab.value = tabOf(p)
  })

  /** 点 tab = 换地址（应用内跳转一律走 `navigate()` ✓）。 */
  const goTab = (t: Tab) => navigate(hrefOf(t))

  return (
    <div class="flex flex-col gap-4">
      {/*
        `role=tablist` + `aria-selected`：门禁按它认"现在是哪个 tab" ✓，
        屏幕阅读器也靠它 ✓（与 `tasks-page.tsx` 那一排同款 ✓）。
      */}
      <div role="tablist" class="join w-full overflow-x-auto" aria-label="历史与日志">
        {(
          [
            ['history', '历史'],
            ['logs', '日志'],
          ] as const
        ).map(([id, label]) => (
          <button
            type="button"
            role="tab"
            aria-selected={tab.value === id}
            class={`btn join-item btn-sm min-h-11 whitespace-nowrap md:min-h-0 ${
              tab.value === id ? 'btn-primary' : 'btn-soft'
            }`}
            onClick={() => goTab(id)}
          >
            {label}
          </button>
        ))}
      </div>

      {/*
        ⚠ 二选一（不是"两份都渲染、各用 CSS 藏一份"✗）：只有当前 tab 那一个在跑 ✓。
        这里不需要 `key`（本框架没有 diff 复用那一步：槽重跑就是新建实例 ✓）。
      */}
      {tab.value === 'history' ? <HistoryPage onGo={props.onGo} /> : <LogsPage />}
    </div>
  )
}
