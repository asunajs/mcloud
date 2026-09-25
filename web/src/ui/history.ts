/**
 * 运行结果的状态标签与徽章 —— 查表，**不拼接**类名。
 *
 * 状态语义（与后端 `TaskOutcome` 一致）：
 * - `success` 成功
 * - `skipped` 跳过（活动未开启/条件不满足）—— **不是失败**，不该染红
 * - `failed`  失败
 */

import type { TaskOutcome } from '../api'

export const RUN_STATUS_LABEL: Record<TaskOutcome['status'], string> = {
  success: '成功',
  skipped: '跳过',
  failed: '失败',
}

export const statusLabel = (status: TaskOutcome['status']): string =>
  RUN_STATUS_LABEL[status] ?? status

/**
 * 徽章样式查表。
 *
 * ⚠ `skipped` 刻意用裸 `badge-soft`（中性）而不是 warning ——
 * 活动没开是正常状态，标成黄色会让人以为需要处理。
 * ⚠ 颜色类必须与基类分开写：值里只给修饰符，模板里写 `badge ${...}`。
 */
export const RUN_STATUS_BADGE: Record<TaskOutcome['status'], string> = {
  success: 'badge-soft badge-success',
  skipped: 'badge-soft',
  failed: 'badge-soft badge-error',
}

export const statusBadge = (status: TaskOutcome['status']): string =>
  RUN_STATUS_BADGE[status] ?? 'badge-soft'

/** 取结果的一句话摘要。 */
export const outcomeText = (outcome: TaskOutcome): string =>
  outcome.status === 'success' ? outcome.summary : outcome.reason
