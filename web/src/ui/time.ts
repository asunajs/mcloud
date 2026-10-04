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

const CLOCK: Intl.DateTimeFormatOptions = {
  hour12: false,
  hour: '2-digit',
  minute: '2-digit',
}

/** 格式化毫秒级时间戳。0 / null / undefined ⇒ `—`。 */
export const fmtEpochMs = (ms: number | null | undefined): string => {
  if (!ms) return '—'
  return new Date(ms).toLocaleString('zh-CN', SHORT)
}

/**
 * 只要「时:分」。
 *
 * 给"**当下的**那个时间点"用（总览的「更新于 22:31」）—— 那种位置的读者只关心
 * "几分钟前"，再带上月日反而要多扫一眼。跨天时它确实会含糊（23:59 → 00:01），
 * 而这两个位置都紧挨着一份会自己变的数据，含糊的代价是零。
 */
export const fmtClock = (ms: number | null | undefined): string => {
  if (!ms) return '—'
  return new Date(ms).toLocaleTimeString('zh-CN', CLOCK)
}

const pad = (n: number, width = 2): string => String(n).padStart(width, '0')

/**
 * `HH:MM:SS.mmm` —— **日志专用**（用户 2026-10-04：「时间没有毫秒？」）。
 *
 * # 为什么日志非得有毫秒
 *
 * 一次运行里**同一秒**会有好几行 ✓ —— 上游 http 那种 `ms=201` 的请求几毫秒一条 ✓
 * ⇒ 只到秒的话**先后顺序根本分不出来** ✗，而排障时"先发了哪个请求"恰恰是关键 ✓。
 * 数据本来就是毫秒级 ✓（后端 `at_ms`、事件里的 `at_ms`、实时面板的 `Date.now()` ✓），
 * 只是没渲染出来 ✗。
 *
 * # 为什么不用 `toLocaleTimeString`（上面几个都走它 ✓）
 *
 * 两条都踩过：
 * ① `zh-CN` + `hour12: false` 在**午夜**会渲染成 `24:00:00` ✗（ICU 的经典口径 ✓）；
 * ② 毫秒在 `Intl` 里要 `fractionalSecondDigits` ✓，而它给不了**定长** ✗ ——
 *    日志里 `.5` 与 `.005` 右端对不齐 ✓，读起来像是两种东西 ✗。
 * 手写这两句反而**确定** ✓。
 */
export const fmtClockMs = (ms: number | null | undefined): string => {
  if (!ms) return '—'
  const d = new Date(ms)
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`
}

/**
 * `MM-DD HH:MM:SS.mmm` —— 给**全局**日志页用 ✓。
 *
 * 与 `fmtClockMs` 只差那个月日 ✓：服务端日志是**一个长跑进程**的流 ✓，
 * 跨天是常态 ✓（凌晨那几枪调度 ✓）⇒ 少了日期，`00:05` 是今天还是昨天就说不清 ✗。
 */
export const fmtStampMs = (ms: number | null | undefined): string => {
  if (!ms) return '—'
  const d = new Date(ms)
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${fmtClockMs(ms)}`
}

/**
 * 上游那种时间串（`2026-07-01 09-30-00`）→ 界面短格式（`07-01 09:30`）。
 *
 * ⚠ 上游的**时间部分也用连字符**（实拍 `09-30-00`），不是 `09:30:00` ——
 * 整串直接显示又长又容易看错（`07-01 09-30-00` 看着像三段数字）。
 * 这里先把连字符还原成冒号，再交给 `Date` 解析（**按本地时区**，与上游口径一致）。
 *
 * 解析不了就**原样返回**：宁可难看，也不要静默显示成空、或者编一个时间出来。
 * 只有**跨年**时才补上年份 —— 同年的活动窗口显示 `07-01 09:30` 就够了。
 */
export const fmtUpstreamTime = (raw: string | null | undefined): string => {
  const s = (raw ?? '').trim()
  if (s === '') return '—'
  const m = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2})[-:](\d{2})(?:[-:](\d{2}))?/.exec(s)
  if (!m) return s
  const [, y, mo, d, h, mi] = m
  const at = new Date(`${y}-${mo}-${d}T${h}:${mi}:00`)
  if (Number.isNaN(at.getTime())) return s
  const short = at.toLocaleString('zh-CN', SHORT)
  return at.getFullYear() === new Date().getFullYear() ? short : `${y}-${short}`
}

/**
 * 把一段**已经过去**的时长渲染成人能读的串（`2 小时 15 分`）。
 *
 * 与 [`fmtDuration`] 分工不同：那个是**单次运行**的耗时（毫秒级、要到秒），
 * 这个是**进程活了多久**（小时/天级）。拿 `fmtDuration` 显示运行时间会出现
 * "0分2秒"压在一个本该显示"3 天"的位置上。
 */
export const fmtUptime = (ms: number): string => {
  // 负值 = 时钟回拨，或后端那格是 0/缺字段。**不装作 0 秒**：报 `—` 更诚实。
  if (!Number.isFinite(ms) || ms < 0) return '—'
  const mins = Math.floor(ms / 60_000)
  if (mins < 1) return '不到 1 分钟'
  if (mins < 60) return `${mins} 分`
  const hours = Math.floor(mins / 60)
  const restMins = mins % 60
  if (hours < 24) return restMins > 0 ? `${hours} 小时 ${restMins} 分` : `${hours} 小时`
  const days = Math.floor(hours / 24)
  const restHours = hours % 24
  return restHours > 0 ? `${days} 天 ${restHours} 小时` : `${days} 天`
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
