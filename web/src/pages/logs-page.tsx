/**
 * 服务端**全量**日志。
 *
 * 与「历史」页的分工（用户 2026-10-04 口径：「我其实想要一个能看所有日志的
 * 窗口，**不仅仅是每次任务**」✓）：
 * * 历史页 = 每次运行的记录，以及**那次运行**的日志 ✓；
 * * 本页   = **整个进程**看到的日志 ✓（调度器 ✓、凭据续期 ✓、上游 http ✓）。
 *
 * 数据来自 `/api/logs` ✓ —— 内存里的环形缓冲（最近 2000 行 ✓，见
 * `crates/mcloud-server/src/logbuf.rs` ✓）。⚠ 它是"最近这些"而不是"全部历史"：
 * 完整历史在日志文件里 ✓；这一页解决的是"不开终端就能看" ✓。
 */
import { onMounted, onUnmounted, ref } from 'lite'

import { listServerLogs, type ServerLogLine } from '../api'
import { IconSearch } from '../ui/icons'
import { MARK, TONE, type LogKind } from '../ui/log-panel'
import { createVirtualRows } from '../ui/virtual-rows'
import { fmtStampMs } from '../ui/time'

/** 级别档位。⚠ 语义是"**不低于**它" ✓（与后端 `min_level` 同一口径 ✓）。 */
const LEVELS: ReadonlyArray<readonly [string, string]> = [
  ['', '全部'],
  ['debug', '调试'],
  ['info', '信息'],
  ['warn', '警告'],
  ['error', '错误'],
]

/**
 * 服务端日志的级别 → 面板那套分类 ✓（两边同一套标记与颜色 ✓）。
 * `trace` 并入 `debug` ✓ —— 面板没有单独一档给它 ✓，而它本来就最不常开 ✓。
 */
const kindOf = (level: string): LogKind =>
  level === 'error' || level === 'warn' || level === 'info' || level === 'debug' ? level : 'debug'

/** 级别的高低（与后端 `logbuf::rank_of` **同一个口径** ✓，别各定一套 ✗）。 */
const RANK: Record<string, number> = { error: 4, warn: 3, info: 2, debug: 1 }

/** 滚动容器的 id（`load()` / 窗口化都靠它拿容器 ✓）。 */
const SCROLL_ID = 'logs-scroll'

/**
 * 窗口化渲染的状态（**模块级单例**：这一页同时只会挂一份 ✓）。
 *
 * 放模块级而不是组件体里：组件体每次重渲染都会跑 ✓ ⇒ 建在那里等于每帧新建一份、
 * 高度缓存全丢（表现是"滚动时行高一直在估、垫片总在变"✗）。放这里还顺带让缓存在
 * **切页往返**之间留着 ✓。
 */
const vr = createVirtualRows()

/**
 * 取数的代次号（**只认最后一次** ✓）。
 *
 * 为什么必须有它：这一页每 2 s 打一次 `/api/logs`，而单请求超时上限是 **30 s**
 * （`api.ts` 的 `REQUEST_TIMEOUT_MS`）—— 慢后端/抖动网络下一屏还没回来，
 * 后面几拍已经发出去 ⇒ 同时有好几个 `load()` 在飞。
 * 各拍的收尾是**无条件**写 `lines.value` / `state.value` 的，于是：
 * * 先发的**旧**响应后到 ⇒ 把新一屏整份盖回旧内容 ✗；
 * * 旧的那拍超时失败、新的那拍已经成功 ⇒ `state.value = 'error'`
 *   把**已经渲染好**的页面重新打成红屏 ✗（用户看到的是"刚才还好好的"）。
 *
 * 放在**模块级**而不是组件里：页面来回切时组件会重建，而上一份实例发出去的
 * 请求还在飞 —— 模块级才不会因为换了一份闭包就认不出它已经过期 ✓。
 * 只用一个自增整数（不引 `AbortController`）：这里要的是"过期响应别写信号"，
 * 省下的那次带宽不是重点，多一个取消路径反而多一处要维护的状态 ✗。
 */
let loadSeq = 0

/**
 * 「算用户还在看最新那几条」的容差（px）。
 *
 * 一行是 20px（`leading-5`）⇒ 32 约等于"一行半"：手指/滚轮的落点误差装得下 ✓，
 * 又不会把"正往上翻"误判成"还贴着底" ✓。
 */
const TAIL_SLACK = 32

export const LogsPage = () => {
  const lines = ref<ServerLogLine[]>([])
  const level = ref('')
  const q = ref('')
  const state = ref<'loading' | 'ready' | 'error'>('loading')
  const errorMessage = ref('')
  let timer: ReturnType<typeof setInterval> | undefined

  /**
   * 窗口化的输入：滚动位置 / 容器可视高度 / "量到新行高"的版本号（都在渲染槽里被读到 ✓）。
   *
   * ⚠⚠ 可视高度**必须在渲染之外量、缓存进信号**，不许在 `win()` 里现读 `clientHeight` ✗
   * （2026-10-05 实测踩过，症状很隐蔽）：渲染期读尺寸会**强制一次布局**，而这次布局正好
   * 落在"窗口正在换行"的中途 —— 那一刻总高还是短的 ⇒ 浏览器把 `scrollTop` 从 634 **钳到 34**
   * ✗，而信号里还写着 634 ⇒ DOM 与信号失配、首屏"跳到底"当场丢掉（轨迹实测：
   * `mut 91ms top=634` → `mut 92ms top=34` ✓）。
   * 只在 rAF / 滚动事件里量（那一刻布局已经稳定 ✓）就没有这个问题 ✓。
   * 初值 0 没关系：首帧只铺 overscan 那几行，下一次 rAF 量到真高度就补齐 ✓。
   */
  const scrollTop = ref(0)
  const viewport = ref(0)
  const heightsV = ref(0)
  /**
   * 上一次"钉底"写下去的 `scrollTop`（`NaN` = 还没钉过）。
   *
   * ⚠ 为什么不用一个 `atTail` 布尔量：**它会滞后** —— 滚动事件要到下一帧才来，
   * 而"钉底"发生在 `requestAnimationFrame` 里 ⇒ 用户（或门禁）刚把位置滚走、
   * 事件还没到的那一帧里，`atTail` 还是 true ⇒ 人会被**拽回底部** ✗
   * （门禁 `ready` 档那条"滚到中间不被弹走"就是这么红的 ✗）。
   * 换成"位置是否还等于我们上次钉的那个值"⇒ 拿 DOM 的**事实**判断，不吃滞后的亏 ✓。
   */
  let lastPin = Number.NaN

  /**
   * 取回来的行**先不过滤** ✓ —— 级别与关键词都在**本地**筛 ✓。
   *
   * ⚠ 为什么不在请求里带 `level`/`q`（接口支持 ✓，但这一页不用 ✗）：
   * 一次取全（≤800 行）之后，点按钮/打字是**立刻**出结果 ✓；
   * 而"每点一下发一次请求"既慢 ✓，又会让刚看到的那屏内容被整体换掉 ✓
   * —— 排查时最烦的就是"内容在我眼前跳走" ✗。
   *
   * ⚠ 800 而不是 2000：这是**浏览器**要渲染的 DOM 行数 ✓（环是 2000 ✓）。
   * 一屏看 800 行已经远超需要 ✓，多了只会让每 2 秒那次重建变卡 ✗。
   */
  /**
   * 滚动容器。⚠ 卡片是**常驻**的（见下面 JSX 那段注释 ✓）⇒ 这个节点自始至终是同一个 ✓，
   * 拿它读/写滚动位置才有意义 ✓。
   */
  const scroller = () => document.getElementById(SCROLL_ID)

  const load = async () => {
    // ⚠ 这一拍的编号（见模块顶部 `loadSeq` 的说明）：`await` 回来后先对一下，
    // 对不上说明后面已经又发过一拍 ⇒ 这一份是**过期**的，整份丢掉 ——
    // 连下面那段滚动位置也**不许**写（旧数据配旧位置，写回去反而把用户的视口搞乱 ✗）。
    const mine = ++loadSeq
    /**
     * ⚠⚠ **更新前先记下滚动位置**（2026-10-04 用户报：「服务端日志页面出日志会强制滚动到顶部」✗）。
     *
     * 怎么坏的：这一页每 2 秒把整份列表换掉 ✓，而原先那张卡片（连同里面这个滚动容器）
     * 是写在 `{visible().length > 0 ? … : null}` 里的 ⇒ 条件每次重算都**换一颗新节点**
     * ⇒ 新节点的 `scrollTop` 天然是 0 ⇒ 用户滚到哪儿都被弹回**最旧的那一屏** ✗。
     * 实测（夹具 300 行、无头 Chrome）：轮询前 `scrollTop=5434`、轮询后 `scrollTop=0`
     * 且容器已经不是同一个节点 ✓。JSX 那边把卡片改成常驻就是从根上治它 ✓。
     *
     * 这里再显式记/写一次，是因为**行本身**也是整批换的（`createFor` 按 item 身份复用，
     * 而轮询回来的行都是新对象 ⇒ 每行都重建），中途高度会短暂归零、位置会被钳 ⇒
     * 写回去才是确定的 ✓。
     *
     * 两条分支：
     * * 更新前就贴着底（或**首次**拿到数据、那时还没有容器 ✓）⇒ 更新后钉回底部 ✓
     *   —— 这一页是"看现在正在发生什么" ✓，新行一进来就该看得见 ✓；
     * * 否则把 `prevTop` **原样写回** ✓ —— 用户正往上翻的时候内容一变就把人弹走，
     *   是排查时最烦的 ✗。
     */
    let prevTop = 0
    let pinned = true
    try {
      const next = await listServerLogs({ limit: 800 })
      // 先判代次再写：写在判断前面，旧响应就已经把新数据盖掉了 ✗。
      if (mine !== loadSeq) return
      /**
       * ⚠⚠ **滚动位置的快照取在"响应到手"这一刻**，不是"发请求之前" ✗。
       *
       * 为什么（2026-10-05 门禁实测踩红）：一拍请求可能飞几十毫秒到 30 s（超时上限 ✓），
       * 这期间用户完全可能已经滚走 ✓。若拿**发请求前**的位置写回去，就是
       * "人刚滚到中间，一拍旧响应把人拖回底部" ✗ —— 与这一页要修的毛病
       * （「出日志会强制滚动到顶部」）是同一个形状，只是方向反过来 ✗。
       * 夹具 60 行、每拍加 2 行时正好踩中：`ready` 档那条"滚到中间不被弹走"。
       *
       * 取在到手这一刻还有个好处：此刻 DOM 里还是**旧内容** ⇒ 读到的
       * `scrollHeight`/`scrollTop` 正是"新行进来之前"的真实状态 ✓。
       */
      const before = scroller()
      prevTop = before?.scrollTop ?? 0
      pinned = before ? before.scrollHeight - before.scrollTop - before.clientHeight <= TAIL_SLACK : true
      lines.value = next
      state.value = 'ready'
      const after = scroller()
      if (after) {
        after.scrollTop = pinned ? after.scrollHeight : prevTop
        // 钉底就把"我们钉在哪"记下来（量高之后的补钉靠它判断用户有没有动过 ✓）
        if (pinned) lastPin = after.scrollTop
      }
      // 数据换了 ⇒ 可视高度（内容不足 70vh 时容器会缩）与行高都可能变 ⇒ 同步一次 ✓
      // （这里的信号写入在 `await` 之后 ✓ 不在渲染路径上 ✓ 安全）
      syncScroll()
    } catch (e) {
      // 同一道闸也要拦**失败**这一支：慢后端下"旧那拍超时"很常见，
      // 它落到 `state.value = 'error'` 就是把已经恢复的页面又打回红屏 ✗。
      if (mine !== loadSeq) return
      errorMessage.value = e instanceof Error ? e.message : String(e)
      state.value = 'error'
      // 失败这一支**不碰滚动位置**（页面内容没变，动它反而把人拽走 ✗），
      // 只把窗口的输入同步一下 ✓。
      syncScroll()
    }
  }

  onMounted(() => {
    void load()
    /**
     * ⚠⚠ **挂载路径上不许写任何信号**（这里曾经调了一次 `syncScroll()` ⇒ 事故）。
     *
     * 机理（`ui/page-refresh.ts` 文件头写过同一条，我这次又踩了一遍 ✗）：
     * 页面挂载的**那一刻**，外壳正在渲染这一页 ⇒ 此刻写信号会**重入外壳的渲染**
     * ⇒ 页面槽位的替换被打断 ⇒ **页面被重新挂载** ⇒ `onMounted` 又跑 `load()`
     * ⇒ 又写信号 …… 死循环。实测（vite dev + 夹具 800 行）：2 秒内打了
     * **218 次** `/api/logs` ✗，容器节点被整个换掉、卡片一直是 `hidden`
     * （每次响应的代次都已过期 ⇒ 永远画不出东西 ✗）。
     *
     * 现在挂载时**一句不写** ✓：
     * * 可视高度由 rAF / 滚动事件里量（见 `viewport` 的注释 ✓），挂载路径上不用碰 ✓；
     * * 首屏量高由 `win()` 末尾那次 `measureSoon()` 兜（rAF ⇒ 一定在渲染之后 ✓）；
     * * `scrollTop` 初值本来就是 0 ✓，真滚动时有 `onScroll` ✓；
     * * 数据回来后 `load()` 收尾那次 `syncScroll()` 是**在 `await` 之后**跑的，
     *   不在渲染路径上 ✓，可以照写。
     */
    // 这页是"看现在正在发生什么" ✓ ⇒ 定时刷一次就够 ✓。
    // ⚠ 刻意**不**做 SSE：日志是只读的、丢一条也不影响判断 ✓；
    // 而"逐条不能丢"那种需求是 `/api/runs/{id}/events` 的活儿 ✓（别抢它的活 ✗）。
    timer = setInterval(() => void load(), 2000)
  })
  onUnmounted(() => {
    if (timer) clearInterval(timer)
  })

  /**
   * 本地过滤后的可见行 ✓（级别"**不低于**" ✓ + 关键词 ✓，与后端同一口径 ✓）。
   *
   * ⚠ 结果按 (`lines` 身份 / 级别 / 关键词) 缓存一次：窗口化之后这个函数**每帧要被
   * 调用好几次**（垫片、切片、行数各一次），而开了搜索时每次过滤都要对 800 行做
   * `toLowerCase` ✗ ⇒ 不缓存的话光是滚动就在做几千次字符串分配 ✓。
   */
  let visCache: { src: ServerLogLine[]; level: string; q: string; out: ServerLogLine[] } | undefined
  const visible = (): ServerLogLine[] => {
    const src = lines.value
    const lv = level.value
    const qq = q.value
    if (visCache && visCache.src === src && visCache.level === lv && visCache.q === qq) return visCache.out
    const floor = RANK[lv] ?? 0
    const needle = qq.trim().toLowerCase()
    const out = src.filter(
      (l) =>
        (RANK[l.level] ?? 0) >= floor &&
        (needle === '' ||
          l.message.toLowerCase().includes(needle) ||
          l.target.toLowerCase().includes(needle)),
    )
    visCache = { src, level: lv, q: qq, out }
    return out
  }

  /**
   * 一行渲染出来的那串文本 —— **既是展示内容，也是行高的缓存键** ✓
   * （同一串文本 + 同一个容器宽度 ⇒ 折行结果一样 ✓，所以拿它当键是准的）。
   */
  const textOf = (l: ServerLogLine): string =>
    `${fmtStampMs(l.at_ms)}  ${MARK[kindOf(l.level)]}  ${l.message}  ·  ${l.target}`

  /** 高度缓存键（= 每行渲染出来的文本）—— 同样按"过滤结果身份"缓存一次 ✓（见 `visible`）。 */
  let keysCache: { vis: ServerLogLine[]; keys: string[] } | undefined
  const keysOf = (): string[] => {
    const vis = visible()
    if (keysCache && keysCache.vis === vis) return keysCache.keys
    keysCache = { vis, keys: vis.map(textOf) }
    return keysCache.keys
  }

  /** 这一帧要渲染哪一段（上垫片 / 行区间 / 下垫片）。**在渲染槽里调用** ✓。 */
  const win = () => {
    heightsV.value // ⚠ 订阅"高度表变了"：少了这一句，量出来的新行高不会逼窗口重算 ✗
    const w = vr.window(scrollTop.value, viewport.value, keysOf())
    /**
     * ⚠ 每次都排一次"渲染后量高"：行少时（首屏、弹窗刚开）**没有任何滚动事件**，
     * 若不在这里排，折了行的那些行高就永远是估计值 ⇒ 总高偏小 ⇒ 底部那几行
     * **滚不到** ✗（日志折行很常见 ✓）。rAF 里写 `heightsV` 是在渲染**之后** ✓，
     * 不在挂载路径上 ✓，安全。
     */
    measureSoon()
    return w
  }

  /** 这次真正要铺的行（带**绝对**下标 —— `data-i` 要靠它对上号 ✓）。 */
  const slice = () => {
    const w = win()
    return visible()
      .slice(w.start, w.end)
      .map((l, i) => ({ i: w.start + i, l, text: textOf(l) }))
  }

  /** 渲染之后量一次行高（`requestAnimationFrame` ⇒ 节点已经在文档里了 ✓）。 */
  let raf = 0
  const measureSoon = () => {
    if (raf) return
    raf = requestAnimationFrame(() => {
      raf = 0
      const el = scroller()
      if (!el) return // 容器不在（页面已卸载）⇒ 什么都不用做 ✓
      // ⚠ 尺寸只在这里（rAF，渲染之后）量 ✓ —— 渲染期量会强制布局、把滚动位置钳歪 ✗（见 `viewport`）
      if (viewport.value !== el.clientHeight) viewport.value = el.clientHeight
      const keys = keysOf()
      const changed = vr.measure(el, (i) => keys[i] ?? '')
      // 变了才重渲染（否则量→渲染→量 转不停 ✗）
      if (changed) heightsV.value++
      /**
       * ⚠ 补钉一次，条件是**两条 DOM 事实里任一成立**：
       *
       * * `nearTail`：现在还贴着底（容差见 `TAIL_SLACK` ✓）⇒ 钉一下没有副作用 ✓；
       * * `stillOurs`：位置**还停在 `load()` 上次钉的那个值上** ⇒ 用户没动过，
       *   只是行高量出来之后总高变了（没量过先用 20px 估 ✓）⇒ 补钉正是要治这个
       *   （实测漏钉会差约 80px ≈ 底部 4 行，而这一页的存在意义就是"看最新" ✗）。
       *
       * 用户一旦往上翻，两条都不成立 ⇒ **一次都不会碰滚动位置** ✓（原来的 bug）✓。
       */
      const nearTail = el.scrollHeight - el.scrollTop - el.clientHeight <= TAIL_SLACK
      const stillOurs = Math.abs(el.scrollTop - lastPin) <= 2 // lastPin 是 NaN 时恒 false ✓
      if (nearTail || stillOurs) {
        el.scrollTop = el.scrollHeight
        lastPin = el.scrollTop
      }
    })
  }

  /** 读一次容器的滚动位置与可视高度（滚动时 / 挂载后 / 每拍数据回来后各一次 ✓）。 */
  const syncScroll = () => {
    const el = scroller()
    if (!el) return
    // ⚠ 这里**在事件/await 之后**，布局是稳的 ⇒ 量尺寸安全 ✓（渲染期不行，见 `viewport`）
    if (viewport.value !== el.clientHeight) viewport.value = el.clientHeight
    if (scrollTop.value !== el.scrollTop) scrollTop.value = el.scrollTop
  }

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
        <div class="flex flex-wrap items-center gap-1.5">
          {LEVELS.map(([id, label]) => (
            <button
              type="button"
              aria-pressed={level.value === id}
              aria-label={`级别：${label}`}
              class={`btn btn-sm min-h-11 md:min-h-0 ${level.value === id ? 'btn-primary' : 'btn-soft'}`}
              onClick={() => (level.value = id)}
            >
              {label}
            </button>
          ))}
        </div>

        <label class="input flex min-h-11 w-full items-center gap-2 md:min-h-0 md:w-64">
          <IconSearch class="h-4 w-4 opacity-60" />
          <input
            class="grow"
            placeholder="搜索正文 / 来源模块"
            value={q.value}
            onInput={(e) => (q.value = (e.target as HTMLInputElement).value)}
          />
        </label>
      </div>

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
        </div>
      ) : null}

      {state.value === 'ready' && visible().length === 0 ? (
        <p class="text-sm opacity-70">没有匹配的日志（缓冲只留最近 2000 行 ✓，这里取的是最近 800 行 ✓）。</p>
      ) : null}

      {/*
        ⚠⚠ 这张卡片**常驻**，可见性走 `class`（`hidden`）—— 别改回 `{… ? <div> : null}` ✗。

        条件写在外面的后果（2026-10-04 用户报「出日志会强制滚动到顶部」）：
        `setNodes` 的语义是"先把上次铺的撤掉、再铺新的"（`lite/src/dom.ts`），
        于是**条件每重算一次就换一颗新节点** —— 连同里面那个滚动容器 ✓。
        新节点的 `scrollTop` 是 0 ⇒ 每 2 秒轮询一次就把用户弹回最旧的那一屏 ✗。
        改成常驻之后，容器节点从头到尾是同一个 ✓，滚动位置由 `load()` 显式记/写 ✓。

        ⚠ 这里必须用 `hidden` 而不是"给个空 class"：`.card` 自带 `display:flex`
        （`app.css` 的 `@layer components`），空 class 只会露出一个空盒子 ✗。
        而 `hidden` 住在 utilities 层、层序在后 ⇒ 压得住它 ✓（见 `app.css` 顶部那段层序说明）。
      */}
      <div class={`card bg-base-100 shadow-sm ${visible().length > 0 ? '' : 'hidden'}`}>
        <div class="card-body gap-2">
          <div
            id={SCROLL_ID}
            class="max-h-[70vh] overflow-y-auto overscroll-contain rounded-box bg-base-200 p-3 font-mono text-xs leading-5 [overflow-anchor:none]"
            onScroll={syncScroll}
          >
            {/*
              ⚠⚠ **窗口化**（2026-10-05 用户口径：「详细日志虚拟加载」）：只铺看得见的那几十行，
              没铺出来的用上下两个**垫片**把高度撑住 ⇒ 滚动条长度与位置仍是**真实总高** ✓。
              改之前是 800 行全铺（实测节点读数在 3,039–7,839 之间来回跳，因为每 2 s 换一批
              数据、整批都是新对象 ✗）。算法与取舍见 `ui/virtual-rows.ts` ✓。
            */}
            {/*
              ⚠⚠ 容器上的 `[overflow-anchor:none]` **不是可选项**（2026-10-05 实测）：
              窗口化会让"上垫片"随滚动从 0 长到几百像素，而 Chrome 的**滚动锚定**
              把这当成"上方内容变高了"，于是**替你加 scrollTop** —— 实测用户刚滚到
              中间（317px），4 ms 内就被顶到 597 → 634（= 底部）✗，表现就是
              "一滚就被弹回底部"（与这一页原先那个 bug 正好反向 ✗）。
              关掉锚定之后，垫片怎么变都不动用户的滚动位置 ✓。
            */}
            {/* 上垫片（前面那些行的总高） */}
            <div style={`height:${win().top}px`} />
            {slice().map((p) => (
              // ⚠ 文本拼成**一个**表达式 ✓（lite 编译器对"元素旁边的裸文本节点"支持最薄 ✓）；
              // `data-i` 是**绝对**下标 —— 量行高时靠它对上号 ✓（别改成可见区间里的相对下标 ✗）。
              <div key={p.l} data-i={p.i} class={`break-words ${TONE[kindOf(p.l.level)]}`}>
                {p.text}
              </div>
            ))}
            {/* 下垫片（后面那些行的总高） */}
            <div style={`height:${win().bottom}px`} />
          </div>
        </div>
      </div>
    </div>
  )
}
