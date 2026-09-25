/**
 * 页面清单与导航配置 —— **唯一来源**。
 *
 * 侧栏、底部 dock、页面标题都从这里读，避免"加了页面但某处忘了改"。
 * 纯数据 + 纯函数，不依赖 Vue。
 *
 * 与设计规范的对应关系：
 *  - 侧栏导航项 **9 条封顶**、不设二级菜单 → §4.1
 *  - dock 只放**页面**，不放"更多"（重复入口没有收益）→ §4.4
 *  - 只用一个断点（`md`，768px）→ §7.1
 */

import type { JSX } from 'vue-jsx-vapor'
import { IconDashboard, IconHistory, IconSettings, IconTask } from './icons'

export type PageId = 'dashboard' | 'tasks' | 'history' | 'settings'

interface NavItem {
  id: PageId
  /** 侧栏与 dock 上显示的名字（力求 2-3 字，窄屏才放得下）。 */
  label: string
  /** 页面标题，可更长。 */
  title: string
  /** 一句话说明这个页面干什么 —— 空态与页面副标题会用到。 */
  blurb: string
  href: string
  /**
   * 图标组件。
   *
   * 返回类型必须是 `JSX.Element`（由各图标显式标注），
   * 否则这里是 `unknown`，TSX 里 `<Icon />` 会报 TS2786。
   */
  icon: (props: { class?: string }) => JSX.Element
}

/**
 * 4 个页面。顺序即侧栏顺序（按使用频率排：总览与任务最常用）。
 *
 * 远未用满 9 条 —— 这是好事：规范里"9 条封顶"是上限不是目标。
 * 后续加页面时若超过 9 条，要么并进已有页，要么先改规范，不要静默开第 10 条。
 */
export const NAV_ITEMS: readonly NavItem[] = [
  {
    id: 'dashboard',
    label: '总览',
    title: '总览',
    blurb: '账号状态、云朵统计与最近运行一览。',
    href: '#/',
    icon: IconDashboard,
  },
  {
    id: 'tasks',
    label: '任务',
    title: '任务中心',
    blurb: '选择账号并触发签到、领取等任务，实时查看执行日志。',
    href: '#/tasks',
    icon: IconTask,
  },
  {
    id: 'history',
    label: '历史',
    title: '历史与日志',
    blurb: '历次执行的结果、耗时与失败原因。',
    href: '#/history',
    icon: IconHistory,
  },
  {
    id: 'settings',
    label: '设置',
    title: '设置',
    blurb: '账号管理与全局选项。',
    href: '#/settings',
    icon: IconSettings,
  },
] as const

/**
 * 底部 dock 里直接点得到的项，**全是页面**。
 *
 * 当前 4 个页面全部放进去（少于规范允许的 5 格，不凑数）。
 * 刻意**不放"更多"**：它点开的是侧栏抽屉，而抽屉在 navbar 左上角的汉堡包
 * 那里已经有入口 —— 重复一格不换来任何新的可达性。
 */
export const DOCK_IDS: readonly PageId[] = [
  'dashboard',
  'tasks',
  'history',
  'settings',
] as const

/** 把 `#/tasks` 这样的 hash 解析成 PageId；无法识别时回落到总览。 */
export const parseHash = (hash: string): PageId => {
  const found = NAV_ITEMS.find((item) => item.href === hash)
  return found ? found.id : 'dashboard'
}

/** 由 PageId 取配置。总览是兜底，所以一定找得到。 */
export const navItem = (id: PageId): NavItem => {
  const found = NAV_ITEMS.find((item) => item.id === id)
  if (!found) {
    throw new Error(`未知页面：${id}`)
  }
  return found
}
