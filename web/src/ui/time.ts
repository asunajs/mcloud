/** 时间的显示格式。**只此一份**：侧栏与账号页都调它。
 *
 * 各页面自己写一遍 `toLocaleString` 的后果是同一屏里两处"最近"长得不一样，
 * 而用户以为那是两个不同的时间口径。
 */

/** rfc3339 → 本地短日期时间；null 给破折号而不是空。 */
export const fmtTime = (iso: string | null): string =>
  iso
    ? new Date(iso).toLocaleString('zh-CN', {
        hour12: false,
        month: '2-digit',
        day: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
      })
    : '—'
