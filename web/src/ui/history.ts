/**
 * 运行结果的状态标签与徽章 —— 查表，**不拼接**类名。
 *
 * 状态语义（与后端 `TaskOutcome` 一致）：
 * - `success` 成功
 * - `skipped` 跳过（活动未开启/条件不满足）—— **不是失败**，不该染红
 * - `partial` 部分成功（有成有败）—— 也不该染红，但**必须与"成功"区分开** ✗
 * - `failed`  失败
 * - `cancelled` **已中止**（用户自己点的，来自生命周期 `RunStatus::Cancelled`）
 *   —— 与 `skipped` 一样是中性结局，**绝不**染红、也不计进失败数 ✗
 *
 * ⚠ 最后那一档不在 `TaskOutcome` 里，来自**另一个字段**（`RunRecord.status`）。
 * 两者的分工见 `api.ts` 的 `RunStatus`：`outcome` 说业务成没成，`status` 说这次
 * 运行是跑完了还是被中止了。一条被中止的运行，其 `outcome` 就是 `failed`
 * ⇒ 只读 `outcome.status` 的话，用户自己点的「中止」会显示成红色「失败」✗
 * （后端 `RunRecord::needs_attention` 明确把 `Cancelled` 排除在"要人管"之外）。
 */

import type { RunRecord, TaskOutcome } from '../api'

/**
 * 界面上真正要区分的那几档 = `TaskOutcome` 那四档 **加上** `cancelled`。
 *
 * 单独一个名字是因为它不只服务徽章：历史页的筛选档位与计数也按它分档
 * （否则被中止的那些既算进「失败」、又反过来在「已中止」里找不到）。
 */
export type RunStatusView = TaskOutcome['status'] | 'cancelled'

/**
 * 一条运行记录该按哪一档显示。
 *
 * ⚠ 只在 `status === 'cancelled'` 时覆盖 `outcome` —— 其余（`done`/`running`/老记录
 * 缺这一格）一律按 `outcome` 走，这正是这一格加进来之前的显示，别顺手改口径 ✗。
 */
export const runStatusOf = (record: Pick<RunRecord, 'status' | 'outcome'>): RunStatusView =>
  record.status === 'cancelled' ? 'cancelled' : record.outcome.status

export const RUN_STATUS_LABEL: Record<RunStatusView, string> = {
  success: '成功',
  skipped: '跳过',
  partial: '部分成功',
  failed: '失败',
  cancelled: '已中止',
}

export const statusLabel = (status: RunStatusView): string => RUN_STATUS_LABEL[status] ?? status

/**
 * 徽章样式查表。
 *
 * ⚠ `skipped` 刻意用裸 `badge-soft`（中性）而不是 warning ——
 * 活动没开是正常状态，标成黄色会让人以为需要处理。
 * ⚠ `cancelled` 同理用裸 `badge-soft`：它是**用户自己按的**，不是故障；
 * 文案「已中止」已经把区别说出来了，颜色不必再喊一遍（染红就等于把它算进失败 ✗）。
 * ⚠ 颜色类必须与基类分开写：值里只给修饰符，模板里写 `badge ${...}`。
 */
export const RUN_STATUS_BADGE: Record<RunStatusView, string> = {
  success: 'badge-soft badge-success',
  skipped: 'badge-soft',
  // ⚠ `partial` 用 warning 而不是 error：它**不是失败**（没到"整个挂了"），
  // 但也不能像 success 那样一片绿 —— 那正是用户踩到的"有失败却显得成功" ✗。
  partial: 'badge-soft badge-warning',
  failed: 'badge-soft badge-error',
  // 中性（与 skipped 同一档）。
  cancelled: 'badge-soft',
}

export const statusBadge = (status: RunStatusView): string => RUN_STATUS_BADGE[status] ?? 'badge-soft'

/** 取结果的一句话摘要（`partial` 与 `success` 一样，正文在 `summary` 里）。 */
export const outcomeText = (outcome: TaskOutcome): string =>
  outcome.status === 'success' || outcome.status === 'partial'
    ? outcome.summary
    : outcome.reason
