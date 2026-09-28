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
import {
  IconClock,
  IconDashboard,
  IconExchange,
  IconFlower,
  IconHistory,
  IconPipeline,
  IconSettings,
  IconTask,
  IconUser,
} from './icons'

export type PageId =
  | 'dashboard'
  | 'accounts'
  | 'tasks'
  | 'exchange'
  | 'live-room'
  | 'schedules'
  | 'pipelines'
  | 'history'
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
   * 返回类型必须是 `JSX.Element`（由各图标显式标注），
   * 否则这里是 `unknown`，TSX 里 `<Icon />` 会报 TS2786。
   */
  icon: (props: { class?: string }) => JSX.Element
}

/**
 * 页面清单。顺序即侧栏顺序（按使用频率排：总览与任务最常用）。
 *
 * ⚠ **已经用满 9 条**（规范里"9 条封顶"是硬上限）。
 * 再加页面**不能**直接在这里开第 10 条 —— 要么并进已有页，
 * 要么先改规范。这正是那条上限存在的意义：逼一次"它真的该是独立一页吗"。
 */
export const NAV_ITEMS: readonly NavItem[] = [
  {
    id: 'dashboard',
    label: '总览',
    title: '总览',
    blurb: '账号状态、AI豆统计与最近运行一览。',
    href: '#/',
    icon: IconDashboard,
  },
  {
    id: 'accounts',
    label: '账号',
    title: '账号',
    blurb: '登录移动云盘账号（扫码 / 短信 / 账号密码），查看已登录的账号。',
    href: '#/accounts',
    icon: IconUser,
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
    id: 'exchange',
    // 2 个字：dock 与侧栏在窄屏都放得下。
    label: '兑换',
    title: '兑换奖品',
    blurb: '用AI豆兑换奖品（网易云音乐月卡等）。下单会真的扣豆，点之前会再确认一次。',
    href: '#/exchange',
    icon: IconExchange,
  },
  {
    id: 'live-room',
    // 2 个字：dock 与侧栏在窄屏都放得下。叫「直播」而不是「直播小红花」——
    // 侧栏那一列是**入口**，全名在页面标题里。
    label: '直播',
    title: '直播小红花',
    blurb: '收直播弹幕里的口令、再把口令换成小红花。小红书那条口令必须人工粘进来。',
    href: '#/live-room',
    icon: IconFlower,
  },
  {
    id: 'schedules',
    label: '定时',
    title: '定时任务',
    blurb: '按 cron 表达式自动执行任务，可指定时区与账号范围。',
    href: '#/schedules',
    icon: IconClock,
  },
  {
    id: 'pipelines',
    label: '编排',
    title: '任务编排',
    blurb: '把多个任务串成一条链，按顺序执行；可配置失败时继续还是中止。',
    href: '#/pipelines',
    icon: IconPipeline,
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
 * 放 5 格（规范上限）：总览、任务、定时、历史、设置 —— 都是手机上常看的。
 * **编排不进 dock**：它是"配置一次、之后自动跑"的东西，日常打开频率低，
 * 从侧栏进完全够用。dock 每一格都要对得起它占的位置。
 * 刻意**不放"更多"**：它点开的是侧栏抽屉，而抽屉在 navbar 左上角的汉堡包
 * 那里已经有入口 —— 重复一格不换来任何新的可达性。
 */
export const DOCK_IDS: readonly PageId[] = [
  'dashboard',
  'tasks',
  'schedules',
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
