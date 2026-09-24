/**
 * 兑换选品列表的**纯渲染件**：`plan` → 颜色、桌面表格、移动卡片。
 *
 * # 为什么只有渲染、没有取数
 *
 * 状态（读哪个账号、加载中、后端原话）留在 `pages/exchange-page.tsx`：
 * 那一页的五块共享"当前选了哪几个账号"这份表单，取数逻辑挪进来就得把表单ref穿两层。
 * 这里只收一份已读好的数组和一个"选这件"的回调。
 *
 * # 话术**不在**这里
 *
 * 每一档"为什么不能兑"的那句中文出自后端
 * （`internal/app/exchange_plan.go` 的 `Reason()`，全仓库只有一处），
 * 这里只出**颜色**。理由与 `ui/exchange-window.ts` 的 `PHASE_BADGE` 同源：
 * 颜色是界面的事，话术是判据的事 —— 两样分开放，改判据不会漏改界面、
 * 改界面也不会不小心把"本月已兑"翻译成"稍后再试"。
 *
 * ⚠ 类名一律**完整字面量**（§9.1①：拼接式 Tailwind 静默失效）。
 */

import type { ExchangePrizeRow } from '../api'

/**
 * 后端 `ExchangePlan.Slug()` 那十七档 → badge 类名。
 *
 * 分四色，口径是"用户下一步做什么"：
 *  - `badge-success`：`go` —— 现在点得动。
 *  - `badge-error`：这一件**这次**没路（下架 / 售罄 / 余量 0 / 缺价 / 朵不够）。
 *  - `badge-warning`：**说不清**或**等得到**（`limit` 缺键或含义冲突、等明天、未到时间、
 *    名称撞两件）。撞不确定那一档要显眼，因为它是排查入口，不是"业务上不行"。
 *  - 默认软底：本月额度用掉那一类 —— 不是错，只是这一号这个月轮不到。
 *
 * ⚠ 表里**故意**没有 `insufficient_cloud`（余额不足）的专属色吗？有的，见下面那一行：
 * 这一档在本页**今天不会出现**（后端这一口不读余额，见 `api.ts` 那段注释），
 * 但档位表要做成**全函数** —— 将来这一口接上余额时，漏登记的那一档会静默落进
 * 默认软底，界面上"朵不够"和"本月已兑"长得一样，而这两种处置完全不同。
 */
export const PLAN_TONE: Record<string, string> = {
  go: 'badge-soft badge-success',
  sold_out: 'badge-soft badge-error',
  daily_stock_empty: 'badge-soft badge-error',
  min_remainder_zero: 'badge-soft badge-error',
  off_shelf: 'badge-soft badge-error',
  price_missing: 'badge-soft badge-error',
  insufficient_cloud: 'badge-soft badge-error',
  limit_tomorrow: 'badge-soft badge-warning',
  not_started: 'badge-soft badge-warning',
  limit_missing: 'badge-soft badge-warning',
  limit_unknown: 'badge-soft badge-warning',
  limit_three_conflict: 'badge-soft badge-warning',
  ambiguous: 'badge-soft badge-warning',
  not_found: 'badge-soft badge-warning',
  exchanged_this_prize: 'badge-soft',
  exchanged_this_group: 'badge-soft',
  ended: 'badge-soft',
}

/** 认不出的档（后端加了新档而这张表没跟上）走默认软底，**不**回 `success`。 */
export const planTone = (plan: string): string => PLAN_TONE[plan] ?? 'badge-soft'

/** 时刻量那一类格子：上游没给键 ≠ 0，所以 null 显示成 `—`。 */
const dash = (v: number | null): string => (v === null ? '—' : String(v))

const nameOf = (p: ExchangePrizeRow): string => p.name ?? `#${p.prizeId}`

/**
 * 桌面表格（§5.3：≥768px 用表格，窄屏另给卡片，不做横向滚动）。
 *
 * 列的顺序 = 决策顺序：先看是什么、要多少朵、还剩多少，最后才是"为什么不能兑"。
 * 三个余量里只上桌面**今日余量**与**剩余**两列：`totalCount`（总投放）与
 * `minRemainderCount`（最小余量档）是排查用的时刻量，界面摆两列会让真判据被淹。
 */
export const ExchangePrizeTable = (props: {
  prizes: ExchangePrizeRow[]
  onPick: (p: ExchangePrizeRow) => void
}) => (
  <div class="hidden overflow-x-auto rounded-box md:block">
    <table class="table table-sm">
      <thead>
        <tr>
          <th>奖品</th>
          <th>组</th>
          <th>需要</th>
          <th>今日余量</th>
          <th>剩余</th>
          <th>状态</th>
          <th> </th>
        </tr>
      </thead>
      <tbody>
        {props.prizes.map((p) => (
          <tr>
            <td class="break-words">
              {nameOf(p)}
              <span class="block font-mono text-xs opacity-50">{p.prizeId}</span>
            </td>
            <td class="whitespace-nowrap">{p.groupId === '' ? '—' : p.groupId}</td>
            <td class="whitespace-nowrap">{dash(p.price)} 朵</td>
            <td class="whitespace-nowrap">{dash(p.dailyRemainderCount)}</td>
            <td class="whitespace-nowrap">{dash(p.count)}</td>
            <td>
              <span class={`badge badge-sm ${planTone(p.plan)}`}>{p.reason}</span>
            </td>
            <td class="whitespace-nowrap">
              <button
                type="button"
                class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                onClick={() => props.onPick(p)}
              >
                选这件
              </button>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  </div>
)

/** 移动卡片（§7.3：每个值都带标签，不靠表头对齐）。 */
export const ExchangePrizeCards = (props: {
  prizes: ExchangePrizeRow[]
  onPick: (p: ExchangePrizeRow) => void
}) => (
  <div class="grid gap-3 md:hidden">
    {props.prizes.map((p) => (
      <div class="rounded-box bg-base-200 p-3">
        <span class="break-words text-sm font-medium">{nameOf(p)}</span>
        <div class="mt-1 flex items-center justify-between gap-2 text-sm">
          <span class="opacity-70">奖品 ID</span>
          <span class="font-mono">{p.prizeId}</span>
        </div>
        <div class="mt-1 flex items-center justify-between gap-2 text-sm">
          <span class="opacity-70">组</span>
          <span>{p.groupId === '' ? '—' : p.groupId}</span>
        </div>
        <div class="mt-1 flex items-center justify-between gap-2 text-sm">
          <span class="opacity-70">需要</span>
          <span>{dash(p.price)} 朵</span>
        </div>
        <div class="mt-1 flex items-center justify-between gap-2 text-sm">
          <span class="opacity-70">今日余量 / 剩余</span>
          <span>
            {dash(p.dailyRemainderCount)} / {dash(p.count)}
          </span>
        </div>
        {/* 状态那一格给**整行的后端原话**，不给 `plan` 那个英文码：
            §3.1 要求用户可见文字是中文，而这一页的话术权威在后端。 */}
        <div class="mt-2">
          <span class={`badge badge-sm ${planTone(p.plan)}`}>{p.reason}</span>
        </div>
        <div class="mt-2">
          <button
            type="button"
            class="btn btn-sm btn-soft min-h-11"
            onClick={() => props.onPick(p)}
          >
            选这件
          </button>
        </div>
      </div>
    ))}
  </div>
)
