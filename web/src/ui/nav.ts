/**
 * 页面清单与导航配置 —— **唯一来源**。
 *
 * 侧栏、底部 dock、页面标题都从这里读，避免"加了页面但某处忘了改"。
 * 纯数据 + 纯函数，不依赖 Vue，可被 vitest 直接测（§9 约定 2）。
 *
 * 与设计指南的对应关系：
 *  - 侧栏导航项 9 条封顶、不设二级菜单 → §4.1
 *  - dock 只放 5 项，且**全是页面**（总览/账号/任务/兑换/历史）→ §4.4、§7.2
 */

import type { JSX } from 'vue-jsx-vapor'
import {
  IconAccount,
  IconDashboard,
  IconDebug,
  IconExchange,
  IconFlower,
  IconHistory,
  IconSchedule,
  IconSettings,
  IconTask,
} from './icons'

export type PageId =
  | 'dashboard'
  | 'accounts'
  | 'tasks'
  | 'redflower'
  | 'exchange'
  | 'schedules'
  | 'history'
  | 'debug'
  | 'settings'

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
   * 返回类型必须是 `JSX.Element`（由各个图标显式标注），
   * 否则这里是 `unknown`，TSX 里 `<Icon />` 会报 TS2786。
   */
  icon: (props: { class?: string }) => JSX.Element
}

/**
 * 9 个页面。顺序即侧栏顺序（按使用频率排：总览与账号最常用）。
 *
 * 补记（2026-09-22）：这一栏原先记的是"设计方案说 9 个、实际清单 8 个 ——
 * 「登录/初始化」不是侧栏页面（它是门禁），不该占一个导航位"。那句仍然成立，
 * 现在满 9 条是**新加了一页**（直播小红花），不是把登录页放了回来。
 *
 * ⚠ 9 条是 §4.1 的封顶数：再加页面要么并进已有页，要么先改设计指南，
 * 不要静默开第 10 条。
 */
export const NAV_ITEMS: readonly NavItem[] = [
  {
    id: 'dashboard',
    label: '总览',
    title: '总览',
    blurb: '云朵总数、今日增量与账号健康度一览。',
    href: '#/',
    icon: IconDashboard,
  },
  {
    id: 'accounts',
    label: '账号',
    title: '账号管理',
    blurb: '扫码/短信登录、粘贴 auth 添加、启停、删除与云朵余额。',
    href: '#/accounts',
    icon: IconAccount,
  },
  {
    id: 'tasks',
    label: '任务',
    title: '任务中心',
    blurb: '配置并触发签到、抽奖等任务。',
    href: '#/tasks',
    icon: IconTask,
  },
  {
    id: 'redflower',
    label: '小红花',
    title: '直播小红花',
    blurb: '看直播间余额与排期，按口令批量领取小红花。',
    href: '#/live-redflower',
    icon: IconFlower,
  },
  {
    id: 'exchange',
    label: '兑换',
    title: '兑换中心',
    blurb: '用云朵兑换权益，抢兑倒计时。',
    href: '#/exchange',
    icon: IconExchange,
  },
  {
    id: 'schedules',
    label: '定时',
    title: '定时任务',
    blurb: '定时任务的启停、下次执行时间与执行记录。',
    href: '#/schedules',
    icon: IconSchedule,
  },
  {
    id: 'history',
    label: '历史',
    title: '历史与日志',
    blurb: '任务执行结果与实时日志。',
    href: '#/history',
    icon: IconHistory,
  },
  {
    id: 'debug',
    label: '调试台',
    title: '调试台',
    blurb: '直接调用上游接口，核对参数与响应。',
    href: '#/debug',
    icon: IconDebug,
  },
  {
    id: 'settings',
    label: '设置',
    title: '设置',
    blurb: '实例配置、站点地址与安全选项。',
    href: '#/settings',
    icon: IconSettings,
  },
] as const

/**
 * 底部 dock 里直接点得到的 5 项，**全是页面**。
 *
 * 原先第 5 项是「更多」，点开的是侧栏抽屉 —— 而抽屉在 navbar 左上角的汉堡包
 * 那里已经有一个入口了（同一个 `label for="nav-drawer"`）。
 * ⇒ 一个只重复既有导航的 dock 位，不值一格；这一格给账号。
 * 其余 4 个页面（小红花 / 定时 / 调试台 / 设置）仍只走抽屉，不加第二条入口。
 */
export const DOCK_IDS: readonly PageId[] = [
  'dashboard',
  'accounts',
  'tasks',
  'exchange',
  'history',
] as const

/** 把 `#/accounts` 这样的 hash 解析成 PageId；无法识别时回落到总览。 */
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
