/**
 * 抢兑时间窗 —— **纯函数**，不碰 DOM、不读全局时钟。
 *
 * 放在 `ui/` 而不是页面里，是按代码指南 §10 第 4 步（逻辑抽成纯函数）与
 * §9 约定 2（`ui/` 里的逻辑全为纯函数，可被 vitest 直接测）。
 * ⚠ 当前仓库**尚未引入 vitest**（代码指南 §十二 未决项 3），所以本文件暂时
 * 只有"可测"这个性质、没有对应用例 —— 这是**登记在案的缺口**，不是"测过了"。
 *
 * # 规则出处（不要在这里重新发明）
 *
 * `docs/business-feature-inventory.md` §`exchange` 抢兑 · 时间窗，逐字：
 *
 *   `getMsToNextExchangeTime` ⇒ 有效窗口 `hh:59:40.000 ~ (hh+1):00:15.000`
 *   （`PREHEAT_SECOND=40`）；`getMsToNextHour(now, offset)` ⇒ `nowMs<20000` 时
 *   不再等待，否则等到 `hh+1:00:00 − offset`。组合：cron 触发 → 等 `:59:40`
 *   → 预热（2 埋点 + 1 滑块 + 图像匹配）→ 等整点前 10ms → 下单。
 *
 * ⇒ 三档状态：
 *   - `idle`     —— 距下一个整点还早（窗口之外）；
 *   - `preheat`  —— 整点前 40 秒起（该做埋点与滑块预热的时刻）；
 *   - `open`     —— 整点后 15 秒内（真正能下单的那一小段）。
 *
 * # ⚠ 为什么这里用本机时钟，而组件指南 §5.9 说"不要用本地时钟算差值"
 *
 * §5.9 那条的适用场景是**服务端给了绝对时刻**（定时任务页的 `nextFireAt`），
 * 前端只做展示倒推。抢兑窗口不是那种数据 —— 它是**固定规则**（每个整点），
 * 没有"服务端版本"可拿，所以只能由本机时钟推。
 *
 * ⚠ 由此带来一个**已知偏差**：本机时钟漂移会直接反映到倒计时上。
 * 页面顶部如实标注了这一点，不把它说成权威时刻。
 * 若后端将来给出权威的下一次开抢时刻，应当改用它，本文件退为回退路径。
 */

/** 预热提前量（秒）。上游 `PREHEAT_SECOND`。 */
export const PREHEAT_SECOND = 40

/** 开抢窗口的尾巴（秒）。整点后这么久之内仍算"窗口内"。 */
export const WINDOW_TAIL_SECOND = 15

/** 窗口所处的那一档。 */
export type WindowPhase = 'idle' | 'preheat' | 'open'

export interface ExchangeWindow {
  /** 当前档位。 */
  phase: WindowPhase
  /** 下一个**开抢时刻**（整点）。`open` 档下是刚刚过去的那个整点。 */
  nextHour: Date
  /** 距开抢的毫秒数；`open` 档下为负（表示整点已过）。 */
  msToOpen: number
  /** 本次窗口的开始（= 整点前 40 秒）。 */
  preheatAt: Date
  /** 本次窗口的结束（= 整点后 15 秒）。 */
  closeAt: Date
}

/** 取整点：把分/秒/毫秒清零。 */
const hourStartOf = (now: Date): Date => {
  const d = new Date(now.getTime())
  d.setMinutes(0, 0, 0)
  return d
}

/**
 * 算当前时刻落在哪一档、以及距下一次开抢还有多久。
 *
 * ⚠ 参数 `now` 必须由调用方给（不在这里 `new Date()`）：这是它能被直接测的
 * 前提 —— 传一个固定时刻进去就能断言结论。
 */
export const nextExchangeWindow = (now: Date): ExchangeWindow => {
  const hourStart = hourStartOf(now)
  // 本小时已经过去的秒数（带毫秒小数）。
  const secOfHour =
    now.getMinutes() * 60 + now.getSeconds() + now.getMilliseconds() / 1000

  const hourMs = 60 * 60 * 1000
  const secToNextHour = 3600 - secOfHour

  // 三档判据（边界归后一档：`>=` 预热、`<` 开抢）。
  const phase: WindowPhase =
    secOfHour >= 3600 - PREHEAT_SECOND ? 'preheat' : secOfHour < WINDOW_TAIL_SECOND ? 'open' : 'idle'

  // `open` 时整点**已经过去**，倒计时该显示"已开抢"而不是"距下一个整点"。
  const openNow = phase === 'open'
  const nextHour = openNow ? hourStart : new Date(hourStart.getTime() + hourMs)

  return {
    phase,
    nextHour,
    msToOpen: openNow ? -secOfHour * 1000 : secToNextHour * 1000,
    preheatAt: new Date(nextHour.getTime() - PREHEAT_SECOND * 1000),
    closeAt: new Date(nextHour.getTime() + WINDOW_TAIL_SECOND * 1000),
  }
}

/**
 * 倒计时的显示串。**返回纯文本**，不做数字滚动动画。
 *
 * ⚠ 这里与组件指南 §5.9 有一处**规范内冲突**：§5.9 要求抢兑时刻用 daisyUI 的
 * `countdown` 组件（它的效果是数字滚动），而 §8 动效表把"数字滚动"明确列进
 * **禁止**列（理由：运维工具一天看几十次，动效只增加等待感）。
 * 取 §8 —— 用等宽字体显示文本，不加滚动。冲突已记进 `docs/status.md` 待用户拍板。
 */
export const countdownText = (msToOpen: number): string => {
  if (msToOpen <= 0) return '已开抢'
  const total = Math.floor(msToOpen / 1000)
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const two = (n: number) => String(n).padStart(2, '0')
  // 超过 1 小时才显示小时段，否则 `00:` 是噪声。
  return h > 0 ? `${two(h)}:${two(m)}:${two(s)}` : `${two(m)}:${two(s)}`
}

/** 档位 → 中文标签。查表而不是 `if` 链（代码指南 §9.1① 的同类理由）。 */
export const PHASE_LABEL: Record<WindowPhase, string> = {
  idle: '等待中',
  preheat: '预热中',
  open: '开抢窗口',
}

/**
 * 档位 → badge 类名。**必须是完整字面量**，不能拼接（§9.1① 静默失效）。
 *
 * 用 `badge-soft`（§5.4：一律软底），不给 `badge-neutral`（暗色主题下黑字压黑底）。
 */
export const PHASE_BADGE: Record<WindowPhase, string> = {
  idle: 'badge-soft',
  preheat: 'badge-soft badge-warning',
  open: 'badge-soft badge-error',
}

/**
 * 档位 → 倒计时数字的颜色。三档，按 §5.9 的原话：
 * 「T-1 小时内**改变颜色**提示（`text-warning`），T-1 分钟内 `text-error`」。
 *
 * ⚠ 「**改变**颜色」这句是本函数的全部要点：**平时必须是正常色**。
 * 曾经写成 `> 60s ? 'text-warning'`，于是**任何时刻**都是黄的 ——
 * 等于把"快到点了"这个信号抹掉（每时每刻都在喊，就等于没喊）。
 */
export const countdownTone = (msToOpen: number): string => {
  if (msToOpen <= 60_000) return 'text-error'
  if (msToOpen <= 3600_000) return 'text-warning'
  return 'text-base-content'
}

/**
 * 秒级时刻（`HH:MM:SS`）—— **只在抢兑窗口这一处用**。
 *
 * 不并进 `ui/time.ts`：那边是"短日期时间"口径（到分钟），给侧栏与账号页共用；
 * 而这里三个时刻（开抢 / 预热开始 / 窗口关闭）的分量都在**秒**上
 * （`:59:40` / `:00:00` / `:00:15`），截到分钟会把三个不同的时刻显示成两个。
 */
export const fmtClock = (d: Date): string => {
  const two = (n: number) => String(n).padStart(2, '0')
  // 带日期：窗口跨整点时（`23:59:40 → 次日 00:00:15`）只显示 `00:00:00`
  // 会让人以为是**今天**的零点，而它其实是明天的。
  return `${two(d.getMonth() + 1)}/${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`
}
