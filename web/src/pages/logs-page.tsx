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
import { accountName, accountNames, refreshAccountNames } from '../account-names'
import { currentIsAdmin } from '../ui/auth-signal'
import { IconSearch } from '../ui/icons'
import { reuseLogLines } from '../ui/log-identity'
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

/**
 * **类别**档位（2026-10-06 用户口径：「这种应该分为运维日志，且只有管理员可见」）。
 *
 * * `task` —— 某次运行的日志（后端 `trace_event` 打的，带账号 ✓）；
 * * `ops`  —— 实例自己的：认证失败、路径被拒、调度器、上游 http。
 *
 * ⚠ 「运维」那一档**只有管理员能拿**（`/api/logs?category=ops` 对普通用户回 403 ✓）
 * ⇒ 下面按 `currentIsAdmin` 决定要不要铺这个按钮 ✓（铺了就是让人点一下撞 403 ✗）。
 */
const CATEGORIES: ReadonlyArray<readonly [string, string]> = [
  ['', '全部'],
  ['task', '任务'],
  ['ops', '运维'],
]

/**
 * 类别按钮的一项。
 *
 * ⚠ 刻意用**对象 + 普通标识符**（而不是让 `.map(([id, label]) => …)` 解构元组）：
 * lite 编译器认不出解构参数里那个 `key`，会报"这里的 `key` 什么都不做"✗
 * （2026-10-06 实测：构建直接失败 ✓）—— 同一页里 `accOptions()` 那行就是对象形状 ✓。
 */
interface CatOption {
  id: string
  label: string
}

/** 级别的高低（与后端 `logbuf::rank_of` **同一个口径** ✓，别各定一套 ✗）。 */
const RANK: Record<string, number> = { error: 4, warn: 3, info: 2, debug: 1 }

/**
 * 正文里账号那一格：`account="0f6050105ab111f4"` ✓。
 *
 * 它是**服务端**盖上去的（`crates/mcloud-app/src/run_labels.rs` ✓ —— 那一层记着
 * "这个 run 是哪个账号"，给每一行都补上 ✓），不是页面从上下文猜的 ✓。
 *
 * ⚠ 2026-10-06 起：账号的**身份**读 `/api/logs` 返回的**结构化**那一格
 * （`ServerLogLine.account` ✓，环里存的时候顺手留了一份 ✓）。
 * 这个正则只留下来干一件事 —— **把正文里那格摘掉**（见 `partsOf`：它已经被提到
 * 行首了，留在正文里就是同一件事写两遍，而且写的是哈希 ✗）。
 *
 * （在这之前身份也得靠这个正则从文本里认 ✓ —— 那时环里没有结构化字段，
 *   `logbuf` 只存 level / target / message ✓。）
 */
const ACCOUNT_RE = / ?account="?([^"\s]+)"?/

/** 这一行是哪个账号的（没有就是空串 —— **不编造** ✓）。 */
const accIdOf = (l: ServerLogLine): string => l.account ?? ''

/**
 * 渲染一行要用的东西（**纯**的那几格，见 [`rowOf`] 的缓存说明 ✓）。
 *
 * ⚠ 没有人名：它是**信号**里来的，必须留在渲染槽里现取 ✓。
 */
interface Row {
  /** 日志行对象本身 —— 既是 `key`，也是"这行是谁"的凭据 ✓。 */
  l: ServerLogLine
  /** 高度缓存键（= 渲染出来的那串文本 ✓）。 */
  k: string
  before: string
  accId: string
  after: string
}

/** 筛选按钮上的一个账号：`id` 是主键（点它写进 `account` ✓），`name` 是给人看的名字 ✓。 */
interface AccOption {
  id: string
  name: string
}

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
  /**
   * 「只看某个账号」（空串 = 全部 ✓）。
   *
   * 为什么要有它：一趟事往往是**几个账号轮流跑**（调度器一次跑一批 ✓），
   * 环里这些行是混在一起的 ✓。用户口径（2026-10-05）：「页面上看不到是那个账号的日志啊」
   * ⇒ 除了给每行标出账号（见 `partsOf`），还得能**只看那一个** ✓。
   */
  const account = ref('')
  /**
   * 「只看某一类」（空串 = 两档都要 ✓）。
   *
   * ⚠ 与级别/账号一样是**本地**筛：一次取全之后点按钮立刻出结果 ✓
   * （理由见 `visible()` 上面那段 ✓）。
   */
  const category = ref('')
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
   * 一次取全之后，点按钮/打字是**立刻**出结果 ✓；
   * 而"每点一下发一次请求"既慢 ✓，又会让刚看到的那屏内容被整体换掉 ✓
   * —— 排查时最烦的就是"内容在我眼前跳走" ✗。
   *
   * ⚠⚠ **取满环（[`RING_LINES`]）**，2026-10-05 改的：原先这里写的是 **800**，理由是
   * "那是**浏览器**要渲染的 DOM 行数" ✗ —— 那条理由**已经被窗口化作废**了 ✓：
   * 现在 DOM 里只有"视口 + overscan"那几十行，**与总行数脱钩** ✓（实测同一条断言下
   * 800 行与 2000 行的节点数一样 ✓，见 `lite/regress/bulk.mjs` 的节点数那一条）。
   * 而用户要的本来就是「能看**所有**日志的窗口」（2026-10-04 口径 ✓）⇒ 没理由只给
   * 环里的前 40% ✗。
   */
  /**
   * 每拍取多少行 = 服务端环形缓冲的容量（`crates/mcloud-server/src/logbuf.rs` 的
   * `RING_CAPACITY` ✓）。
   *
   * ⚠ 这边**不 import 那个常量**（前端/Rust 两个构建单元之间没有共享常量 ✓），所以
   * 它是个**会漂的副本** —— 但漂了也不出事：服务端 `limit` 会 `clamp(1, RING_CAPACITY)`
   * （`logbuf.rs` ✓）⇒ 要多了只会**多给不了**、不会报错 ✓，而"环比这个数大"的唯一后果
   * 是最后那几行看不到（用户下次刷新就赶上了 ✓）。
   */
  const RING_LINES = 2000
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
     * 这里再显式记/写一次，是因为**内容会长**（每拍多几行 ✓）：总高变了、窗口边界也可能动，
     * 中途高度会短暂按估计值算 ⇒ 位置会被钳 ⇒ 写回去才是确定的 ✓。
     * ⚠ 原先这里还有半句"行本身也是整批换的（轮询回来的行都是新对象 ⇒ 每行都重建）"——
     * 那半句**已经不作数**了：现在逐行沿用上一拍的对象身份（`ui/log-identity.ts` ✓），
     * 没变的行不会被重建 ✓。但**上面这条理由不变**：会长的是数组、不是行 ✓。
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
      const next = await listServerLogs({ limit: RING_LINES })
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
      /**
       * ⚠⚠ **逐行沿用上一拍的对象身份**（`ui/log-identity.ts` ✓）—— 别改回 `= next` ✗。
       *
       * 每拍回来的都是刚 `JSON.parse` 出来的新对象，而 `createFor` 的 `key` 判的是
       * item **身份**（`key={p.l}` ✓）⇒ 直接 `= next` 会让**一个字都没变的行**也换身份
       * ⇒ 视口里那几十行每 2 s 拆掉重建 ✗。后果里最刺眼的是**正在选中的文字被清掉**
       * （这一页的用途就是"一边跑一边看、顺手复制一行去查" ✗）。
       * 复用之后：没变的行还是原来那个对象 ⇒ 节点不换、选中不丢 ✓。
       * ⚠ 返回的是**新数组**（信号要变 ✓），只有元素对象被复用 ✓。
       */
      lines.value = reuseLogLines(lines.value, next)
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
     * ⚠ 账号名要**单独拉一次**（`/api/accounts` ✓）—— 环里给的是主键（手机号哈希 ✓），
     * 而这一页要把主键显示成人名 ✓（见 `partsOf`）。
     *
     * ⚠ 这一句**不违反**下面那条"挂载路径上不许写信号"：`refreshAccountNames` 是
     * `async` ✓，写 `names` 信号发生在 `await` 回来**之后** ✓，不在渲染路径上 ✓
     * （`void` 掉 Promise 就够，别 `await` ✗ —— 那会把首屏数据也拖到账号列表之后 ✗）。
     */
    void refreshAccountNames()
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
   * 本地过滤后的可见行 ✓（级别"**不低于**" ✓ + 账号 ✓ + 关键词 ✓）。
   *
   * ⚠ 结果按 (`lines` 身份 / 级别 / 账号 / 关键词) 缓存一次：窗口化之后这个函数**每帧要被
   * 调用好几次**（垫片、切片、行数各一次），而开了搜索时每次过滤都要对整环（2000 行）做
   * `toLowerCase` ✗ ⇒ 不缓存的话光是滚动就在做几千次字符串分配 ✓。
   * ⚠ 缓存键里**必须**带上账号（漏了就是"切了账号但列表没变"这类幽灵 ✗）。
   */
  let visCache:
    | {
        src: ServerLogLine[]
        level: string
        q: string
        acc: string
        cat: string
        out: ServerLogLine[]
      }
    | undefined
  const visible = (): ServerLogLine[] => {
    const src = lines.value
    const lv = level.value
    const qq = q.value
    const aa = account.value
    const cc = category.value
    if (
      visCache &&
      visCache.src === src &&
      visCache.level === lv &&
      visCache.q === qq &&
      visCache.acc === aa &&
      visCache.cat === cc
    )
      return visCache.out
    const floor = RANK[lv] ?? 0
    const needle = qq.trim().toLowerCase()
    const out = src.filter((l) => {
      if ((RANK[l.level] ?? 0) < floor) return false
      // ⚠ 类别那一格**必须**进缓存键（同账号那条理由：漏了就是"切了类别列表没变"这类幽灵 ✗）
      if (cc !== '' && l.category !== cc) return false
      const id = accIdOf(l)
      if (aa !== '' && id !== aa) return false
      if (needle === '') return true
      return (
        l.message.toLowerCase().includes(needle) ||
        l.target.toLowerCase().includes(needle) ||
        // ⚠ 账号名也要能搜到：用户在界面上看到的是**人名** ✓，搜不到才奇怪 ✗
        (id !== '' && accountName(id).toLowerCase().includes(needle))
      )
    })
    visCache = { src, level: lv, q: qq, acc: aa, cat: cc, out }
    return out
  }

  /**
   * 一行渲染出来的那串文本 —— **既是展示内容，也是行高的缓存键** ✓
   * （同一串文本 + 同一个容器宽度 ⇒ 折行结果一样 ✓，所以拿它当键是准的）。
   */
  const textOf = (l: ServerLogLine): string =>
    `${fmtStampMs(l.at_ms)}  ${MARK[kindOf(l.level)]}  ${l.message}  ·  ${l.target}`

  /**
   * 一行真正渲染出来的三段：**前缀**（时间 + 级别）/ **账号**（可能没有）/ **正文 + 来源**。
   *
   * ⚠ 为什么把账号从正文里**摘出来**、提到行首：它本来挂在正文末尾
   * （`…（上游次数没动 ⇒ 不算记上） account="0f6050105ab111f4"` ✓），
   * 既是**一串用户认不出的哈希** ✗，又在最不显眼的位置 ✗。提到行首、定宽
   * （`w-20 truncate` ✓）之后，同一个账号的行在视觉上对齐成一条竖线 ⇒
   * 扫一眼就能按账号分组 ✓ —— 这正是用户报的「页面上看不到是那个账号的日志啊」要的 ✓。
   *
   * ⚠ 三段都是**元素**（`<span>` ✓），没有"元素旁边的裸文本" —— 那是 lite 编译器
   * 支持最薄的写法 ✗（这一页上一版正是靠"拼成一个表达式"绕开的 ✓）。
   * 行高缓存键仍用 [`textOf`]（原始文本 ✓）：账号那格是**定宽**的，
   * 换不换人名都不影响折行 ⇒ 不必为"名字后到"重算高度 ✓。
   */
  const partsOf = (l: ServerLogLine): Row => {
    const m = l.message.match(ACCOUNT_RE)
    // 身份读**结构化**那格；正则只负责把正文里那格摘掉 ✓（老后端没那格时兜回正则 ✓）
    const id = l.account ?? m?.[1] ?? ''
    // 摘掉正文里那格：它已经被提到行首了，留着就是同一件事写两遍（而且写的是哈希 ✗）
    const body = m ? l.message.replace(m[0], '') : l.message
    return {
      l,
      // `k` 就是这一行的高度缓存键（= 渲染出来的那串文本 ✓，与 `keysOf()` 同一份口径 ✓）
      k: textOf(l),
      before: `${fmtStampMs(l.at_ms)}  ${MARK[kindOf(l.level)]}  `,
      accId: id,
      after: `${body}  ·  ${l.target}`,
    }
  }

  /**
   * 每行一个**稳定的行对象** —— 这是"轮询不重建行"的**全部机关**所在。
   *
   * `createFor` 的复用条件是"**key 与 item 都没变**"（`lite/src/control.ts:94`）：
   * key 用 `p.l`（日志行对象 ✓，`ui/log-identity.ts` 让它跨拍稳定 ✓）只满足一半，
   * item 若还是每帧现造的包装对象 ⇒ 照样整批重建 ✗。所以这里**按日志行对象缓存行对象** ✓
   * （日志行对象稳定 ⇒ 行对象也稳定 ⇒ 节点被复用 ✓）。
   *
   * ⚠ 缓存里**只放纯函数算出来的东西**（时间/级别/正文/来源 ✓）：账号**人名**不进来 ——
   * 它来自 `/api/accounts`、可能**后到**（`account-names.ts` 记过"名字不出现"那个坑 ✗），
   * 缓存住就等于把那个 bug 请回来 ✓。所以人名在渲染槽里现取（见下面 JSX 的注释 ✓）。
   *
   * ⚠ 上界：环里最多 2000 行，而缓存按**见过的**行累计 ⇒ 给个上限、到顶整体清掉
   * （与 `virtual-rows.ts` 的 `MAX_CACHE` 同一取向：代价只是重新算一遍看得见的那几十行 ✓）。
   */
  const ROW_CACHE_MAX = 4000
  const rowCache = new Map<ServerLogLine, Row>()
  const rowOf = (l: ServerLogLine): Row => {
    const hit = rowCache.get(l)
    if (hit) return hit
    if (rowCache.size > ROW_CACHE_MAX) rowCache.clear()
    const row = partsOf(l)
    rowCache.set(l, row)
    return row
  }

  /**
   * 当前数据里**出现过**的账号（去重 ✓、带上人名 ✓）—— 筛选按钮按它铺 ✓。
   *
   * ⚠ 缓存键要**两个**：`lines` 的身份 ✓ + **名字缓存的 Map 身份** ✓
   * （`accountNames` 每次更新都换一张新 Map ✓）。只按 `lines` 缓存的话，
   * 账号列表**后到**时这份结果不会重算 ⇒ 筛选按钮一直停在哈希上 ✗
   * （`account-names.ts` 模块头记的就是这一类"名字不出现"✓）。
   */
  let accCache: { src: ServerLogLine[]; names: Map<string, string>; out: AccOption[] } | undefined
  const accOptions = (): AccOption[] => {
    const src = lines.value
    const nm = accountNames.value
    if (accCache && accCache.src === src && accCache.names === nm) return accCache.out
    const seen = new Map<string, string>()
    for (const l of src) {
      const id = accIdOf(l)
      if (id !== '' && !seen.has(id)) seen.set(id, accountName(id))
    }
    const out = [...seen].map(([id, name]) => ({ id, name }))
    accCache = { src, names: nm, out }
    return out
  }

  /** 高度缓存键（= 每行渲染出来的文本）—— 同样按"过滤结果身份"缓存一次 ✓（见 `visible`）。 */
  let keysCache: { vis: ServerLogLine[]; keys: string[] } | undefined
  const keysOf = (): string[] => {
    const vis = visible()
    if (keysCache && keysCache.vis === vis) return keysCache.keys
    keysCache = { vis, keys: vis.map((l) => rowOf(l).k) }
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

  /**
   * 这次真正要铺的行。
   *
   * ⚠ **不带下标**：行自带 `data-k`（自己的高度缓存键 ✓）⇒ 不依赖"第几行" ⇒
   * 位置一变（轮询每拍都在挪 ✓）也不会被判成"位置敏感"而重建 ✓
   * （`virtual-rows.ts` 文件头那条契约变更 ✓）。
   */
  const slice = (): Row[] => {
    const w = win()
    return visible().slice(w.start, w.end).map(rowOf)
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
      const changed = vr.measure(el)
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

  /**
   * 铺出来的**类别**档位 —— 「运维」那一档只给管理员 ✓（理由见 [`CATEGORIES`]）。
   *
   * ⚠ 抽成函数而**不是**在 JSX 里写 `.filter(..).map(..)`：lite 编译器只认
   * **直接**挂在那里的 `.map()`（链一个 `.filter()` 它会报"这里的 `key` 什么都不做"✗
   * —— 2026-10-06 实测踩到，构建直接失败 ✓）。
   */
  const catOptions = (): CatOption[] =>
    CATEGORIES.filter(([id]) => id !== 'ops' || currentIsAdmin.value).map(([id, label]) => ({
      id,
      label,
    }))

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
      <div class="flex flex-col gap-3 md:flex-row md:items-start md:justify-between">
        <div class="flex flex-col gap-1.5">
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
          {/*
            类别：任务（某次运行的）vs 运维（实例自己的）。

            ⚠ 「运维」这一档**只给管理员铺**：普通用户请求它会被后端回 403
            （`server_logs.rs` 的 `require_admin` ✓）⇒ 铺出来就是让人点一下撞报错 ✗。
            ⚠ `currentIsAdmin` 默认 `false`（探到之前按非管理员渲染 ✓ 失败关闭）——
            代价是管理员首帧会看不到这个按钮，下一拍（会话探测回来）就出来了 ✓。
          */}
          <div class="flex flex-wrap items-center gap-1.5">
            <span class="text-xs opacity-60">类别</span>
            {catOptions().map((c) => (
              <button
                key={c.id}
                type="button"
                aria-pressed={category.value === c.id}
                aria-label={`类别：${c.label}`}
                class={`btn btn-xs min-h-9 md:min-h-0 ${category.value === c.id ? 'btn-primary' : 'btn-soft'}`}
                onClick={() => (category.value = c.id)}
              >
                {c.label}
              </button>
            ))}
          </div>
          {/*
            ⚠ 只在**真的出现过两个以上账号**时才铺这一行：只有一个账号时它是个
            永远只有一种选法的摆设 ✗（而这一页平时就是这么用的 ✓）。
          */}
          {accOptions().length > 1 ? (
            <div class="flex flex-wrap items-center gap-1.5">
              <span class="text-xs opacity-60">账号</span>
              <button
                type="button"
                aria-pressed={account.value === ''}
                aria-label="账号：全部"
                class={`btn btn-xs min-h-9 md:min-h-0 ${account.value === '' ? 'btn-primary' : 'btn-soft'}`}
                onClick={() => (account.value = '')}
              >
                全部
              </button>
              {accOptions().map((o) => (
                <button
                  key={o.id}
                  type="button"
                  aria-pressed={account.value === o.id}
                  aria-label={`账号：${o.name}`}
                  // 名字可能重名/被截断 ⇒ 悬停时把**主键**也给人看 ✓（排障时要的就是它 ✓）
                  title={o.id}
                  class={`btn btn-xs min-h-9 md:min-h-0 ${account.value === o.id ? 'btn-primary' : 'btn-soft'}`}
                  onClick={() => (account.value = o.id)}
                >
                  {o.name}
                </button>
              ))}
            </div>
          ) : null}
        </div>

        <label class="input flex min-h-11 w-full items-center gap-2 md:min-h-0 md:w-64">
          <IconSearch class="h-4 w-4 opacity-60" />
          <input
            class="grow"
            placeholder="搜索正文 / 来源 / 账号"
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
        <p class="text-sm opacity-70">{`没有匹配的日志（账号 / 级别 / 关键词任一不匹配都会没有 ✓；内存里只留最近 ${RING_LINES} 行 ✓，这一页取的就是那全部 ${RING_LINES} 行 ✓）。`}</p>
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
              // ⚠ `data-k` 是**这一行自己的**高度缓存键 —— 别改回"绝对下标 `data-i`" ✗：
              // 读下标会让编译器把这一行判成位置敏感（`positional`），而轮询每拍都在挪位置
              // ⇒ 一个字都没变的行也会被整批重建 ✗（用户看到的是选中的文字每 2 s 被清掉 ✗）。
              <div key={p.l} data-k={p.k} class={`break-words ${TONE[kindOf(p.l.level)]}`}>
                <span>{p.before}</span>
                {/*
                  ⚠ 三段都是**元素**（`<span>` ✓）—— 别把 `p.before` / `p.after` 写成裸文本 ✗：
                  那是 lite 编译器支持最薄的写法（这一页上一版就是靠"拼成一个表达式"绕开的 ✓）。

                  账号那格**定宽 + 截断**（`inline-block w-20 truncate` ✓）：不定宽的话
                  名字长短不一，每一行的正文都会跟着错位，一屏日志看起来像锯齿 ✗。
                  点它 = 只看这个账号 ✓（与上面那排筛选按钮同一个信号 ✓）。
                */}
                {p.accId !== '' ? (
                  <span
                    data-acc={p.accId}
                    class="mr-1 inline-block w-20 cursor-pointer truncate align-bottom font-medium text-primary"
                    // ⚠ 人名**在这里现取**（不在 `rowOf` 里缓存）：它来自 `/api/accounts`，
                    // 可能比这一屏日志**后到** ⇒ 缓存住就永远停在哈希上了 ✗
                    //（`account-names.ts` 文件头记的就是这一类"名字不出现"✓）。
                    title={`只看这个账号：${accountName(p.accId)}（${p.accId}）`}
                    onClick={() => (account.value = p.accId)}
                  >
                    {accountName(p.accId)}
                  </span>
                ) : null}
                <span>{p.after}</span>
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
