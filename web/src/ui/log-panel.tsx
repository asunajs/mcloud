/**
 * 运行日志面板。
 *
 * 固定高度 + 可滚动（规范 §7.2 「历史与日志」：SSE 日志区固定高度可滚动）。
 * `overscroll-contain` 防止 iOS 上滚到边界带动整页。
 *
 * ⚠ 本组件**只画**：行数与截断由调用方的 [`pushLog`] 管（两页共用同一个上限）。
 */

// 每行都要带时刻 —— 用户 2026-10-04 口径：「下面的日志没有时间」✗。
// ⚠ 用 `fmtClockMs`（`HH:MM:SS.mmm` ✓）而不是 `fmtEpochMs`（带月日 ✓）：同一个运行里的
// 行都发生在几分钟内，带日期只是噪音 ✗；跨天那两个位置由页面自己决定要不要
// 带日期 ✓（见 `logs-page.tsx` ✓）。
import { onUnmounted, ref } from '@asunajs/lite'
import { fmtClockMs } from './time'
import { createVirtualRows } from './virtual-rows'

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
 * 日志最多留多少行 —— **一次运行**（每条 SSE 事件一行，只截头部：最新的在最后 ✓）。
 *
 * # 2026-10-05：500 → 2000（口径变了，不是"调参数"）
 *
 * 原来定 500 的两条理由，第一条**已经被窗口化作废**：
 *
 * 1. ✗ 作废：「超出部分既看不见」—— 那是"全铺进 DOM ⇒ 只有最后十几行真在文档里"的
 *    时代 ✓。现在面板是**窗口化**的（`ui/virtual-rows.ts` ✓）且能滚到底：滚动条长度
 *    就是**真实总高**（门禁 `regress/bulk.mjs` 拿 600 行断言过 ✓）⇒ 留下的行
 *    **每一行都翻得到** ✓。留 500 行等于"把这次运行前 3/4 的日志藏起来" ✗，
 *    而用户的口径是「想看到**所有**日志」（2026-10-04 ✓）。
 * 2. ✓ 仍然成立：**内存与每次追加的复制成本**。但按 2000 行算，上界也就
 *    ~200 KB（一行几十到几百字节 ✓），而 `pushLog` 每次 `[...lines]` 是 O(n)
 *    ⇒ 一次跑满也就 ~200 万次指针拷贝（毫秒级 ✓）。所以"有上限"要留 ✓、
 *    "上限是 500"不必留 ✓。
 *
 * 同款做法见 `toast.ts` 的 `MAX_TOASTS`、`dashboard-page` 的 `RECENT_LIMIT`
 * （那两个的可见上限**没有**窗口化兜底 ⇒ 保持原值 ✓）。
 */
export const MAX_LOG_LINES = 2000

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
/**
 * 每个面板实例一个自增 id（`document.getElementById` 要它 —— lite 没有 ref ✓）。
 * ⚠ 用**计数器**而不是写死一个字符串：同页万一挂两份，写死就会撞 id ✗。
 */
let panelSeq = 0

/**
 * 窗口化状态（**模块级单例**）：高度缓存按"渲染出来的那串文本"存 ✓，
 * 两个面板（不同页面）先后用同一份缓存是好事 ✓（切页回来不用重量）。
 */
const vr = createVirtualRows()

export const LogPanel = (props: { get: () => LogLine[] }) => {
  const panelId = `log-panel-${++panelSeq}`
  /**
   * 窗口化的输入（与 `pages/logs-page.tsx` **同一套做法** ✓）：滚动位置 + 量高版本号。
   */
  const scrollTop = ref(0)
  /**
   * ⚠ 可视高度**在渲染之外量、存信号**（初值 0 = 首帧只铺 overscan ✓，下一帧补齐 ✓）。
   * 渲染期读 `clientHeight` 会强制布局、正好落在换行中途 ⇒ 浏览器把滚动位置钳歪 ✗
   * （`pages/logs-page.tsx` 里实测过：634 → 34 ✓）。
   */
  const viewport = ref(0)
  const heightsV = ref(0)
  const scroller = () => document.getElementById(panelId)

  /** 行渲染出来的那串文本 —— 既是展示内容，也是行高的缓存键 ✓。 */
  const textOf = (l: LogLine): string => `${fmtClockMs(l.at)}  ${MARK[l.kind]}  ${l.text}`

  let keysCache: { src: LogLine[]; keys: string[] } | undefined
  const keysOf = (): string[] => {
    const src = props.get()
    if (keysCache && keysCache.src === src) return keysCache.keys
    keysCache = { src, keys: src.map(textOf) }
    return keysCache.keys
  }

  const win = () => {
    heightsV.value // 订阅"高度表变了"（见 `virtual-rows.ts` 的说明）
    const w = vr.window(scrollTop.value, viewport.value, keysOf())
    // ⚠ 每次都排一次"渲染后量高"：面板刚开时没有滚动事件，不排的话折行行高
    // 永远是估计值 ⇒ 总高偏小 ⇒ 最底下那几行滚不到 ✗（rAF 在渲染之后 ✓ 安全）
    measureSoon()
    return w
  }

  /**
   * 这次要铺的行。
   *
   * ⚠ 直接铺**日志行对象本身**（不再包一层 `{i, l, text}`）：`createFor` 的复用条件是
   * "key 与 item 都没变"（`@asunajs/lite` 的 `src/control.ts:94`）⇒ 包一层就等于**每帧现造 item**
   * ⇒ 一个字都没变的行也会被重建 ✗。行自带 `data-k`（自己的高度缓存键 ✓）之后
   * 也不需要绝对下标了 ✓（见 `virtual-rows.ts` 文件头那条契约变更）。
   */
  const slice = (): LogLine[] => {
    const w = win()
    return props.get().slice(w.start, w.end)
  }

  let raf = 0
  const measureSoon = () => {
    if (raf) return
    raf = requestAnimationFrame(() => {
      raf = 0
      const el = scroller()
      if (!el) return
      if (viewport.value !== el.clientHeight) viewport.value = el.clientHeight
      if (vr.measure(el)) heightsV.value++
    })
  }

  const syncScroll = () => {
    const el = scroller()
    if (!el) return
    if (viewport.value !== el.clientHeight) viewport.value = el.clientHeight
    if (scrollTop.value !== el.scrollTop) scrollTop.value = el.scrollTop
    measureSoon()
  }

  /**
   * ⚠⚠ **这里刻意没有 `onMounted`**：挂载路径上写信号会重入父层的渲染 ⇒ 反复重挂载
   * （`logs-page.tsx` 上实测过：2 秒 218 次请求、容器节点被整个换掉 ✗）。
   * 面板本就不需要"挂载时同步"：`scrollTop` 初值 0 ✓、可视高度渲染时现读 ✓、
   * 量高由 `win()` 里的 rAF 兜 ✓。
   */
  onUnmounted(() => {
    if (raf) cancelAnimationFrame(raf)
    raf = 0
  })

  return (
    <>
      {/*
        ⚠⚠ 卡片**常驻**、可见性走 `class` —— 与 `pages/logs-page.tsx` 同一条理由 ✓。

        原先这里写的是 `{props.get().length === 0 ? null : <div class="card">…`：
        那个条件**读了日志信号** ⇒ 每来一条日志就重算 ⇒ `setNodes` 把整块（连同
        里面那个滚动容器）**换一颗新节点** ⇒ 用户往上翻时会被弹回顶部 ✗
        （2026-10-03 的 review 记过这个形状，当时只治了"行"没治"壳"✗）。
        常驻之后容器自始至终是同一个 ✓，滚动位置不再丢 ✓。
      */}
      <div class={`card bg-base-100 shadow-sm ${props.get().length === 0 ? 'hidden' : ''}`}>
        <div class="card-body">
          <h2 class="card-title text-base">运行日志</h2>
          <div
            id={panelId}
            class="h-64 overflow-y-auto overscroll-contain rounded-box bg-base-200 p-3 font-mono text-xs leading-5 [overflow-anchor:none]"
            onScroll={syncScroll}
          >
            {/* 窗口化：只铺看得见的那几十行，上下垫片撑住真实总高 ✓（见 `ui/virtual-rows.ts`） */}
            <div style={`height:${win().top}px`} />
            {slice().map((l) => (
              // ⚠ 标记与正文拼在**一个**表达式里：lite 编译器对"元素旁边的裸文本
              // 节点"支持得最薄 ✓，拼成一整串最稳 ✓。
              // ⚠ `key` 与 `data-k` 都是**这一行自己**的东西（对象身份 / 高度缓存键 ✓）——
              // 行因此不依赖下标 ⇒ 新行进来时旧行被**搬动**而不是重建 ✓
              //（用户往上翻时的选中状态、光标都不会被抹掉 ✓）。
              <div key={l} data-k={textOf(l)} class={`break-words ${TONE[l.kind]}`}>
                {textOf(l)}
              </div>
            ))}
            <div style={`height:${win().bottom}px`} />
          </div>
        </div>
      </div>
    </>
  )
}
