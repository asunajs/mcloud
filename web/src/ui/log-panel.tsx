/**
 * 运行日志面板。
 *
 * 固定高度 + 可滚动（规范 §7.2 「历史与日志」：SSE 日志区固定高度可滚动）。
 * `overscroll-contain` 防止 iOS 上滚到边界带动整页。
 *
 * ⚠ 本组件**只画**：行数与截断由调用方的 [`pushLog`] 管（两页共用同一个上限）。
 */

// 每行都要带时刻 —— 用户 2026-10-04 口径：「下面的日志没有时间」✗。
// ⚠ 用 `fmtClock`（`HH:MM:SS`）而不是 `fmtEpochMs`（带月日 ✓）：同一个运行里的
// 行都发生在几分钟内，带日期只是噪音 ✗；跨天那两个位置由页面自己决定要不要
// 带日期 ✓（见 `logs-page.tsx` ✓）。
import { fmtClock } from './time'

/**
 * 日志的**分类** —— 照老项目 `mcloud-sign` 的 consola `type` 体系 ✓
 * （后端 `LogKind` 一一对应 ✓，别各定一套 ✗）。
 *
 * ⚠ 与"严重级别"是两件事：级别管筛选/告警 ✓，分类管**这一行是什么** ✓。
 * 只有级别时「领到了 6 颗」和「这一笔没领到」都是 info ✗ ——
 * 用户 2026-10-04 原话：「info warn error **success** 等等标记都没有，
 * 更别说颜色」✓。
 */
export type LogKind = 'info' | 'success' | 'fail' | 'warn' | 'error' | 'start' | 'debug'

export interface LogLine {
  kind: LogKind
  text: string
  at: number
}

/**
 * 行的着色查表。**字面量**，不拼接（§9.1①）。
 *
 * 这里刻意用语义色 + `opacity`，不用 `gray-*` 之类的具名色阶 ——
 * 那些不随主题变，深色模式下必然出现"浅字压深底"。
 *
 * ⚠ 这些语义色必须在 `app.css` 的 `@theme` 里注册过 ✓，否则类名会
 * **静默不生成**（写了也不报错、就是没颜色 ✗，见 AGENTS.md 那条坑 ✓）。
 */
export const TONE: Record<LogKind, string> = {
  info: 'opacity-90',
  success: 'text-success',
  // `fail`（某一笔没成）与 `warn` 同色 ✓：都不是"整个挂了"✗，但也别一片绿 ✓
  fail: 'text-warning',
  warn: 'text-warning',
  error: 'text-error',
  start: 'text-info',
  debug: 'opacity-50',
}

/**
 * 行首的**文字标记** ✓ —— 与颜色**成对**给，别只给颜色 ✗。
 *
 * 理由很实在：截图里、暗色主题下、色觉差异的人眼里，颜色都可能分不出来 ✓，
 * 而 `SUCCESS`／`FAIL` 这几个字在任何情况下都读得出来 ✓。
 * 等宽字体下按最长那个（`SUCCESS`）对齐 ✓。
 */
export const MARK: Record<LogKind, string> = {
  info: 'INFO   ',
  success: 'SUCCESS',
  fail: 'FAIL   ',
  warn: 'WARN   ',
  error: 'ERROR  ',
  start: 'START  ',
  debug: 'DEBUG  ',
}

/**
 * 日志最多留多少行。
 *
 * 一次运行里日志是**无上限**追加的（每条 SSE 事件一行），而面板高度固定
 * ⇒ 超出部分既看不见、又白占内存与每次更新的遍历成本（2026-10-03 review §2.1）。
 * 500 行远多于那个 `h-64` 能显示的量（约 12 行），够回看一大段。
 * 同款做法见 `toast.ts` 的 `MAX_TOASTS`、`dashboard-page` 的 `RECENT_LIMIT`。
 */
export const MAX_LOG_LINES = 500

/** 追加一行，并按 [`MAX_LOG_LINES`] 截断（**只截头部**：最新的在最后）。 */
export function pushLog(lines: LogLine[], kind: LogLine['kind'], text: string): LogLine[] {
  const next = [...lines, { kind, text, at: Date.now() }]
  return next.length > MAX_LOG_LINES ? next.slice(next.length - MAX_LOG_LINES) : next
}

/**
 * 运行日志面板。
 *
 * ⚠⚠ `get` 必须是**函数**（thunk），不能是数组（2026-10-03 review §2.1）：
 * 传数组时调用方要在槽里当场读（`{lines.value.length > 0 ? <LogPanel lines={lines.value}/> : null}`）
 * ⇒ 那个槽订阅了日志信号 ⇒ **每来一条日志就把整块面板（含全部已显示的行、
 * 含那个滚动容器）拆掉重造** ✗。后果有三条：一次运行累计 O(L²)；
 * 多账号并发时 L 涨得更快；**用户往上翻日志会被弹回底部**（滚动容器换了新节点）。
 * 传 thunk 时槽里只读到一个函数，真正的读发生在本组件自己的槽里 ⇒
 * `.map` 那一层重跑、`createFor` 按**行对象身份**复用已有行，只追加新的 ✓
 * （行对象是 `pushLog` 新建的、旧的那些身份不变）。
 */
export const LogPanel = (props: { get: () => LogLine[] }) => (
  <>
    {props.get().length === 0 ? null : (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <h2 class="card-title text-base">运行日志</h2>
          <div class="h-64 overflow-y-auto overscroll-contain rounded-box bg-base-200 p-3 font-mono text-xs leading-5">
            {props.get().map((l, i) => (
              // ⚠ 标记与正文拼在**一个**表达式里：lite 编译器对"元素旁边的裸文本
              // 节点"支持得最薄 ✓，拼成一整串最稳 ✓。
              <div key={i} class={`break-words ${TONE[l.kind]}`}>
                {`${fmtClock(l.at)}  ${MARK[l.kind]}  ${l.text}`}
              </div>
            ))}
          </div>
        </div>
      </div>
    )}
  </>
)
