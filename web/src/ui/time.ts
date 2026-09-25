/**
 * 时间格式化。纯函数，不依赖 Vue。
 *
 * 统一走 `zh-CN` 短格式（月-日 时:分），带秒会让表格变吵。
 * 取不到值时返回 `—` 而不是空串 —— 空串在表格里看起来像"漏渲染"。
 */

const SHORT: Intl.DateTimeFormatOptions = {
  hour12: false,
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
}

/** 格式化毫秒级时间戳。0 / null / undefined ⇒ `—`。 */
export const fmtEpochMs = (ms: number | null | undefined): string => {
  if (!ms) return '—'
  return new Date(ms).toLocaleString('zh-CN', SHORT)
}

/** 把耗时毫秒渲染成人能读的串。 */
export const fmtDuration = (ms: number): string => {
  if (ms < 1000) return `${ms}ms`
  const secs = ms / 1000
  if (secs < 60) return `${secs.toFixed(1)}s`
  const mins = Math.floor(secs / 60)
  const rest = Math.round(secs % 60)
  return `${mins}分${rest}秒`
}
