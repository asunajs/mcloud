/**
 * 页面清单与导航配置 —— **唯一来源**。
 *
 * ⚠ `href` 是**干净路径**（`/tasks`），不是 `#/tasks` —— 2026-10-03 用户口径
 * 「先去掉#号」。跳转一律走 `ui/router.ts` 的 `navigate()`，别自己写
 * `location.hash = …`（那已经是过去式）。
 *
 * 侧栏、底部 dock、页面标题都从这里读，避免"加了页面但某处忘了改"。
 * 纯数据 + 纯函数，不依赖 Vue。
 *
 * 与设计规范的对应关系：
 *  - 侧栏导航项 **9 条封顶**、不设二级菜单 → §4.1
 *  - dock 只放**页面**，不放"更多"（重复入口没有收益）→ §4.4
 *  - 只用一个断点（`md`，768px）→ §7.1
 */

import { IconBell, IconClock, IconDashboard, IconExchange, IconFlower, IconHistory, IconPipeline, IconSettings, IconTask, IconUser } from './icons'

/**
 * ⚠ 2026-10-08 起 **没有 `'logs'` 了**（用户口径：把「历史」与「日志」合成一条，
 * 另外把「推送」单独开一条 ⇒ 一减一加，侧栏仍是 10 条 ✓）。
 * 日志成了「历史」页的**第二个 tab** ✓；`/logs` 这个**地址**照旧认（见
 * `PATH_ALIASES`）—— 它只是不再是一条独立的导航项 ✓。
 */
export type PageId =
  | 'dashboard'
  | 'accounts'
  | 'tasks'
  | 'exchange'
  | 'live-room'
  | 'schedules'
  | 'pipelines'
  | 'history'
  | 'push'
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
 * ⚠ 侧栏**现在 10 条**（这句原先写的是"已经用满 9 条"，与实际不符 —— 按实际记 ✓）。
 * 规矩照旧：**再加第 11 条之前先问一句"它真的该是独立一页吗"**。
 *
 * 2026-10-08 用户口径（原话「合并这两个（截图圈着侧栏的「历史」与「日志」），并单独给推送一个」）
 * ⇒ **一减一加**，仍是 10 条 ✓：
 * * 减：`logs` 不再是导航项，它成了「历史」页的第二个 tab ✓（地址 `/logs` 照旧认 ✓）；
 * * 加：`push` 独立成页 ✓ —— 渠道与规则两块从「设置」整块搬过去 ✓
 *   （用户口径 2026-10-07「我没有看到配置推送规则的页面」的根因就是它**不是导航项** ✗）。
 *
 * 2026-10-07 现成先例（同一个功能，答案换过一次）：用户先要「上游状态」独立成页，
 * 加进侧栏；看到界面后又说「**这个不要放 nav 里，看放哪里好点**」⇒ 改挂
 * **账号页的子视图** `/accounts/upstream`（`accounts-page.tsx` 那套 ✓，
 * 与 `/accounts/settings`、`/tasks/config` 同款）—— 侧栏回到 10 条 ✓。
 * ⇒ 判据不是"它重不重要"，而是"**它是不是一个独立的去处**"：上游状态是
 * "看某个账号在上游那边是什么样"，**跟着账号走**，所以它是账号页的一层，不是一条导航 ✓。
 */export const NAV_ITEMS: readonly NavItem[] = [
  {
    id: 'dashboard',
    label: '总览',
    title: '总览',
    blurb: '账号状态、AI豆统计与最近运行一览。',
    href: '/',
    icon: IconDashboard,
  },
  {
    id: 'accounts',
    label: '账号',
    title: '账号',
    blurb: '登录移动云盘账号（扫码 / 短信 / 账号密码），查看已登录的账号。',
    href: '/accounts',
    icon: IconUser,
  },
  {
    id: 'tasks',
    label: '任务',
    title: '任务中心',
    blurb: '选择账号并触发签到、领取等任务，实时查看执行日志。',
    href: '/tasks',
    icon: IconTask,
  },
  {
    id: 'exchange',
    // 2 个字：dock 与侧栏在窄屏都放得下。
    label: '兑换',
    title: '兑换奖品',
    blurb: '用AI豆兑换奖品（网易云音乐月卡等）。下单会真的扣豆，点之前会再确认一次。',
    href: '/exchange',
    icon: IconExchange,
  },
  {
    id: 'live-room',
    // 2 个字：dock 与侧栏在窄屏都放得下。叫「直播」而不是「直播小红花」——
    // 侧栏那一列是**入口**，全名在页面标题里。
    label: '直播',
    title: '直播小红花',
    blurb: '收直播弹幕与小红书里的口令、再把口令换成小红花。',
    href: '/live-room',
    icon: IconFlower,
  },
  {
    id: 'schedules',
    label: '定时',
    title: '定时任务',
    blurb: '按 cron 表达式自动执行任务，可指定时区与账号范围。',
    href: '/schedules',
    icon: IconClock,
  },
  {
    id: 'pipelines',
    label: '编排',
    title: '任务编排',
    blurb: '把多个任务串成一条链，按顺序执行；可配置失败时继续还是中止。',
    href: '/pipelines',
    icon: IconPipeline,
  },
  {
    id: 'history',
    label: '历史',
    title: '历史与日志',
    // ⚠ 2026-10-08 起这一条是**两个 tab**（历史 / 日志）—— 用户口径
    // 「合并这两个（历史与日志）」✓。blurb 里要把日志那半也说出来，
    // 不然"日志去哪了"会变成一个只能靠点开才知道的问题 ✗。
    blurb: '历次执行的结果与耗时；第二个 tab 是整个进程的服务端日志。',
    href: '/history',
    icon: IconHistory,
  },
  {
    id: 'push',
    label: '推送',
    title: '推送',
    blurb: '配推送渠道（PushPlus / 企业微信 / 邮件…）与推送规则，决定什么情况下推、推到哪。',
    href: '/push',
    icon: IconBell,
  },
  {
    id: 'settings',
    label: '设置',
    title: '设置',
    blurb: '账号管理与全局选项。',
    href: '/settings',
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
/**
 * 把 hash 切成路径段：`#/tasks/sign/config` → `['tasks','sign','config']`。
 *
 * 子路由（`/tasks/<任务名>/config`，任务配置页）靠它取参数 ——
 * 那类页面**不是**一条独立导航项：规范里"侧栏 9 条封顶"是硬上限，
 * 而配置页本来就属于「任务」，多开一条只会多一个不该出现的入口。
 */
/** 把 `location.pathname` 切成路径段（**不**解码，需要时由调用方 `decodeURIComponent`）。 */
export const pathParts = (pathname: string): string[] =>
  pathname.split('/').filter((x) => x.length > 0)

/**
 * 老地址 → 页面 id。
 *
 * ⚠ 只放"**曾经是一条导航项、现在不是了**"的地址。放进来就等于承诺它永远打得开 ✓
 * —— 这类地址散落在书签、浏览器历史与旧文档里，删掉就是给别人一个 404 ✗。
 * ⚠ 与新地址**同页**的老地址（`/accounts/push` → `/accounts/settings`）不走这里：
 * 那一对是子视图内部认的（见 `account-config-page.tsx` 的 `isPushPath` 那类判据 ✓）。
 */
const PATH_ALIASES: Record<string, PageId> = { '/logs': 'history' }

export const parsePath = (pathname: string): PageId => {
  const found = NAV_ITEMS.find((item) => item.href === pathname)
  if (found) return found.id
  /**
   * 老地址（不再是一条导航项，但**必须继续认**）。
   *
   * ⚠ `/logs` 2026-10-08 起并进「历史」页当第二个 tab ✓ —— 但书签、浏览器历史、
   * 旧文档里的 `/logs` 不能 404，也不能把人甩回总览 ✗（与 `/accounts/push`
   * 那条老地址兼容同一个口径 ✓：改名只该影响**新**链接 ✓）。
   */
  const alias = PATH_ALIASES[pathname]
  if (alias) return alias
  // 子路由按**第一段**归属：`/tasks/xxx/config` 仍是「任务」这一页。
  // 不做这一步的话它会被兜底成"总览"，用户点「配置」会莫名其妙跳到总览页。
  const [first] = pathParts(pathname)
  const parent = NAV_ITEMS.find((item) => item.href === `/${first ?? ''}`)
  return parent ? parent.id : 'dashboard'
}

/** 由 PageId 取配置。总览是兜底，所以一定找得到。 */
export const navItem = (id: PageId): NavItem => {
  const found = NAV_ITEMS.find((item) => item.id === id)
  if (!found) {
    throw new Error(`未知页面：${id}`)
  }
  return found
}
