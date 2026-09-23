/**
 * 任务中心的分组、筛选与徽标 —— 纯数据 + 纯函数，不依赖 Vue（§9 约定 2）。
 *
 * 与 `exchange-window.ts` 同构：页面里凡是"能单独说清楚"的规则都搬到这里，
 * 让 `tasks-page.tsx` 只剩渲染。
 */

import type { CapabilityRow } from '../api'

/**
 * 分组的中文标签。
 *
 * ⚠ 键取自 `core.Group.String()`（`daily` / `activity` / `query` / `exchange` /
 * `maintenance`），与后端枚举**逐字对应** —— 这不是另起一套命名，是给同一批值配中文。
 *
 * ⚠⚠ 这张表要覆盖**全部**枚举值，而不是只覆盖"当前用得上"的那几个：
 * 少一个键的表现不是"那一组显示成英文"，而是分组时 `label` 兜底成 key、
 * 或者更糟 —— 若哪天有人把兜底改成过滤，那一组的能力会**凭空消失**。
 * 后端的 `TestGroupTruthTable` 钉着 5 个值，这里就跟着放 5 个。
 */
export const GROUP_LABEL: Record<string, string> = {
  daily: '每日',
  activity: '活动',
  query: '查询',
  exchange: '兑换',
  maintenance: '维护',
}

/** tabs 的显示顺序：按"多久用一次"排，日常在左。未列出的组排最后。 */
export const GROUP_ORDER: readonly string[] = [
  'daily',
  'activity',
  'query',
  'exchange',
  'maintenance',
]

/** 一组能力（一个 tab）。 */
export interface CapabilityGroup {
  /** `core.Group` 的文本值，如 `daily`。 */
  key: string
  /** 中文标签；未知组回落成 key 本身（宁可显示英文，也不要静默丢）。 */
  label: string
  items: CapabilityRow[]
}

/**
 * 任务中心**只列 `hidden === false`** 的能力。
 *
 * ⚠ `hidden` 的语义不是"废弃"，而是"**它有更合适的入口**"：
 * - `exchange` → 兑换中心页（那边有倒计时与奖品列表，混进通用列表既难用也危险）；
 * - `auth-refresh` → 账号页那枚按钮（通用列表那枚"立即执行"在用户心里是
 *   "**现在就换**"，而这条链承诺的恰好相反 —— 到点才换、没到点一行上游都不打）。
 *
 * ⚠⚠ 但它们**仍是合法的调度目标**：定时任务选目标走的是另一条路（不看 `hidden`）。
 * 所以"这里不显示"**绝不等于**"这条能力不能跑" —— 别顺手把过滤加到别处。
 */
export const visibleCapabilities = (all: readonly CapabilityRow[]): CapabilityRow[] =>
  all.filter((c) => !c.hidden)

/**
 * 按 `group` 归组，**丢掉空组**（没有能力的组不该占一个 tab），
 * 再按 [GROUP_ORDER] 排序；未登记的组按名字排在最后。
 */
export const groupCapabilities = (all: readonly CapabilityRow[]): CapabilityGroup[] => {
  const buckets = new Map<string, CapabilityRow[]>()
  for (const cap of visibleCapabilities(all)) {
    const list = buckets.get(cap.group)
    if (list) list.push(cap)
    else buckets.set(cap.group, [cap])
  }
  const rank = (key: string): number => {
    const i = GROUP_ORDER.indexOf(key)
    return i === -1 ? GROUP_ORDER.length : i
  }
  return [...buckets.entries()]
    .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
    .map(([key, items]) => ({ key, label: GROUP_LABEL[key] ?? key, items }))
}

/** 徽标：文案 + **整条**类名。 */
export interface Badge {
  label: string
  cls: string
}

/**
 * 风险徽标。
 *
 * ⚠ 类名一律**整条写全**（含基类 `badge`）。§7.4 那条补记③记着踩过的坑：
 * 表里只给修饰类（`badge-soft badge-success`）会**丢掉基类 `badge`**，
 * 渲染成无样式裸文字，而 typecheck 与 build **全绿**。
 */
const RISK_BADGE: Record<string, Badge> = {
  low: { label: '低风险', cls: 'badge badge-soft' },
  medium: { label: '中风险', cls: 'badge badge-soft badge-warning' },
  high: { label: '高风险', cls: 'badge badge-soft badge-error' },
}

export const riskBadge = (risk: string): Badge =>
  RISK_BADGE[risk] ?? { label: risk, cls: 'badge badge-soft' }

/**
 * 无副作用的能力标一个「只读」。
 *
 * 依据是 `core.KindQuery` 的声明：这一类**不写运行历史、不产生日志噪音**
 * ⇒ 用户点完它，下面"最近运行"里**不会**多出一行。不提前说清楚，
 * 那看起来就像"按钮没生效"。
 */
export const readOnlyBadge = (kind: string): Badge | null =>
  kind === 'query' ? { label: '只读', cls: 'badge badge-soft badge-info' } : null

/**
 * 执行完一句反馈：`POST /api/runs` 回的是 **202 受理**，不是结果。
 *
 * ⚠ 这两件事必须分开说。写成"执行成功"是把"受理"读成了"跑完" ——
 * 而真实结果是稍后才落库的（查询类甚至不落库）。
 */
export const acceptedText = (capability: string, runIds: readonly string[]): string => {
  if (runIds.length === 0) {
    // 受理了但一个 run 都没有：不是错误，是"没有可跑的目标"（例如零账号）。
    return `已受理「${capability}」，但没有可执行的目标（账号列表为空）。`
  }
  return `已受理「${capability}」，共 ${runIds.length} 个目标正在执行 —— 结果稍后出现在下面。`
}
