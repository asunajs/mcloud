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
 * 任务中心**列全部**能力，`hidden` 的也在内。
 *
 * 补记口径的来由（2026-09-25，"所有任务一页卡片化"那一轮）：这一页不再只是
 * "单跑一条能力"的表单，而是**编排预设运行列表**的地方 —— 一份清单里完全可能
 * 既要有签到、也要有 `auth-refresh`（收尾刷令牌）与 `exchange`（带参数兑换）。
 * 所以 `hidden` 在这一页**不再是过滤条件**，只是"它另有专属入口"的一句提示。
 *
 * ⚠ `hidden` 的语义仍然没变：它指"**有更合适的入口**"（`exchange` → 兑换中心页、
 * `auth-refresh` → 账号页那枚按钮），不是"废弃"。这一页把它们摆出来不等于
 * 那两个入口该撤 —— 撤掉的话，通用表单里那枚「立即执行」在用户心里是"**现在就换**"，
 * 而 `auth-refresh` 那条链承诺的恰好相反（到点才换、没到点一发请求都不发）。
 * 所以这一族**仍然**要在卡片上标出来，别让人以为在这里点就算强制刷新了。
 */
export const groupCapabilities = (all: readonly CapabilityRow[]): CapabilityGroup[] => {
  const buckets = new Map<string, CapabilityRow[]>()
  for (const cap of all) {
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

/**
 * 「全部」那个 tab 的 key。
 *
 * 空串而不是编一个 `core.Group` 里没有的单词：它**不是**一个组，是"不按组筛"。
 * 用假 code 的风险是哪天后端真加了一个同名组，两件事会静默合成一件事。
 */
export const ALL_GROUP_KEY = ''

/** 把所有能力并成一组（「全部」那个 tab 用）。 */
export const allInOneGroup = (all: readonly CapabilityRow[]): CapabilityGroup[] =>
  all.length === 0 ? [] : [{ key: ALL_GROUP_KEY, label: '全部', items: [...all] }]

/**
 * tabs 的清单：「全部」在最左，后面按 `GROUP_ORDER` 排。
 *
 * 默认停在「全部」正是"不管什么平台，一页卡片看全"这句话的落法 ——
 * 分组 tab 留着当**筛子**，不当**隔板**。
 */
export const tabsFor = (all: readonly CapabilityRow[]): CapabilityGroup[] => [
  ...allInOneGroup(all),
  ...groupCapabilities(all),
]

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
 * `hidden` 的能力标一句「另有专属入口」。
 *
 * 标它不是为了好看：`exchange` 在兑换中心页有倒计时与选品，`auth-refresh` 在账号页
 * 那枚按钮点下去是"**现在就换**" —— 而这一页的通用表单给不了那两个语义。
 * 不标出来，用户会以为这里的"立即执行"与那边的按钮是一回事。
 */
export const ownEntryBadge = (hidden: boolean): Badge | null =>
  hidden ? { label: '另有专属入口', cls: 'badge badge-soft' } : null

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
