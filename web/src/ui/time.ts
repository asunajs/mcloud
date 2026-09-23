/** 时间的显示格式。**只此一份**：侧栏与账号页都调它。
 *
 * 各页面自己写一遍 `toLocaleString` 的后果是同一屏里两处"最近"长得不一样，
 * 而用户以为那是两个不同的时间口径。
 */

/** 那一套显示口径。**只此一份**：`fmtTime` 与 `fmtEpochMs` 共用它，
 *  分成两处写的后果是同一屏里"到期"和"最近成功"长得不一样，而用户以为那是两个时间口径。 */
const SHORT: Intl.DateTimeFormatOptions = {
  hour12: false,
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
}

/** rfc3339 → 本地短日期时间；null 给破折号而不是空。 */
export const fmtTime = (iso: string | null): string =>
  iso ? new Date(iso).toLocaleString('zh-CN', SHORT) : '—'

/**
 * epoch **毫秒** → 同一口径；0 / null 给破折号。
 *
 * 单独一条是因为令牌到期时刻走的是毫秒数而不是 rfc3339（对位后端 `accountPayload`
 * 的 `authExpire`），而接口返回的时间戳字段走 rfc3339 —— 两种单位同屏时
 * 必须落到同一个显示口径，否则看不出差了一天。
 */
export const fmtEpochMs = (ms: number | null): string =>
  ms && ms > 0 ? new Date(ms).toLocaleString('zh-CN', SHORT) : '—'
