/**
 * 设置页。
 *
 * 2026-10-01 重做（用户口径「系统设置页面」）。
 *
 * # 这一版改掉了什么
 *
 * - **删掉「外观（主题）」**：顶栏那个按钮就够了，摆在设置里是重复入口
 *   （用户原话："主题没用"）。连带 `ui/theme-signal.ts` 一起回退 ——
 *   那个共享信号的全部理由就是"两个入口别分叉"，只剩一个入口时它是多余的抽象。
 * - **账户安全从"只有读数"变成能操作**：改口令、口令下限、会话时长。
 *   后端这次一起补上了（`PUT /api/admin/password`、`GET/PUT /api/admin/security`）——
 *   之前这里写着"需要后端接口（现在没有）"，用户说"把后端也补上"。
 * - **补「存储信息」与「重启服务」**：数据目录/日志占用/二进制路径，以及一个真的
 *   会优雅重启服务的按钮（后端 `GET /api/system/storage`、`POST /api/system/restart`）。
 *
 * # 2026-10-08：**推送那两块搬走了**（用户口径「并单独给推送一个」）
 *
 * 「推送渠道」与「推送规则」两张卡**整块**搬去新的「推送」页（`pages/push-page.tsx` ✓），
 * 这一页**删掉**它们（不是藏起来 ✗）。留下的卡片顺序：
 * 服务 → 口令 → 口令策略 | 会话控制 → 存储信息 → **事件推送** → 服务操作 → 关于 ✓。
 * 之所以搬：那两块**藏在设置页里、要往下滚才看得见**，用户 2026-10-07 的原话是
 * 「**我没有看到配置推送规则的页面**」✗ —— 而配置推送本来就该有自己的门 ✓。
 *
 * # 2026-10-08（同日，第二次口径）：**新增「事件推送」卡**
 *
 * 用户原话：「在**设置**里，单独为这种**面板行为**增加一个可配置推送规则的入口」✓
 * —— 登录新设备 / 登录失败 / 耗时异常 / 连续失败这四类**没有定时可绑**，
 * 所以入口在这里，而不是在「推送」页或定时表单里 ✓。
 * ⚠ 它是**行为**那一侧（「什么时候发」）：每行**选规则**（可多选 ✓）＋ 节流
 * （触发方式 / 冷却 / 每日上限 ✓，保守默认见 `api.ts::PANEL_THROTTLE_DEFAULTS` ✓）。
 *
 * # 2026-10-08（同日，第三次口径）：**八张卡拆成顶部四节 tab**
 *
 * 用户原话：「**设置页面内容太多了，也没有分页面**」✓ —— 这一页从 2026-10-01 起一路
 * 加卡（服务 / 口令 / 策略 / 会话 / 存储 / 事件推送 / 服务操作 / 关于 = **8 张**），
 * 一屏堆下来只能靠滚，找哪一块全靠眼力 ✗。
 *
 * 分法（用户选的）：**顶部 tab 切换**，四节 —— 服务 / 安全 / 事件推送 / 关于 ✓。
 * ⚠ **一张卡都没丢**（8 张照旧，只是换了位置 ✓），卡的**内容与顺序**一个字没改 ✓：
 *
 * | tab | 装哪几张 |
 * |---|---|
 * | 服务 | 版本/服务信息 · 存储信息 · 服务操作 |
 * | 安全 | 修改管理员口令 · 口令策略 · 会话控制 |
 * | 事件推送 | 「事件推送」卡（面板行为入口，2026-10-08 第二次口径那一条 ✓）|
 * | 关于 | 关于 |
 *
 * # ⚠⚠ tab 的样式：`join` + `btn join-item`，**不用 daisyUI 的 `.tab`**
 *
 * 与 `tasks-page.tsx` 的分组 tab、`history-logs-page.tsx` 的「历史/日志」同款 ✓。
 * `.tab` 在本仓**被排除过**（见 `app.css` 顶部 `@theme` 那段注释：它是 daisyUI 组件，
 * 本仓只保留手写的那批）—— 用了它**不报错、只是静默不生成样式**，
 * 界面退化成"一串连在一起的纯文字"✗。
 *
 * # 地址：选中哪一节**进 URL**
 *
 * `/settings`（服务，**老地址照旧落第一节** ✓）/ `/settings/security` / `/settings/events`
 * / `/settings/about` ✓。`ui/nav.ts::parsePath` 的"子路由按第一段归属"天然把它们都
 * 归到 `settings` 这一页 ⇒ **不用动路由表** ✓，刷新 / 书签 / 浏览器后退都能用 ✓。
 * 三个新段都不带 `.` ✓（`static_files.rs` 的 SPA 回落把"末段带点"的当静态资源 ✗）。
 *
 * ⚠ 与 `history-logs-page.tsx` 同一条口径：tab 由**地址**决定（`watch(currentPath)` ✓），
 * 不是本地 `ref` —— 否则"直接开 `/settings/security`"会先落在服务节（地址与界面不一致 ✗），
 * 而且浏览器后退也切不动 ✓。
 *
 * ⚠ **只渲染当前那一节**（不是"四节都渲染、各用 CSS 藏"✗）：省掉同时挂着的四份控件，
 * 也顺带把「安全节那几张卡不在服务节里」这件事变成**结构性**的 ✓（门禁就钉这条）。
 *
 * # 2026-10-09：**「存储信息」卡能改容量上限了**
 *
 * 用户口径：「日志和历史不要全加入内存，使用本地记录，**让用户设置最大大小**」✓ ——
 * 后端把运行日志改成按需读盘 + 分页，并加了**五个**上限（`PUT /api/admin/storage` ✓）；
 * 这一页负责**看得见、改得动**：日志占用写成「占用 / 上限」（`556 KB / 64 MB` ✓）、
 * 服务端日志那行同理（`1.2 MB / 64 MB（12 个文件）` ✓），
 * 下面**五个**数字框 + 一个保存键 ✓。⚠ 卡**没有**多加一张（`存储信息` 仍是同一张卡 ✓）——
 * 那张「八张卡一张没丢」的门禁钉子就是防这个的 ✗。
 *
 * # 为什么账户安全那块只认"当前口令"而不认身份
 *
 * 会话被盗时这一条拦不住 —— 但那是会话本身的语义：能改口令的会话本来就有全部权限。
 * 真正的防护是"改完把**所有**旧会话踢掉"（后端做的事），包括发起这次请求的那张，
 * 然后给当前浏览器换一张新的。
 */

import { onMounted, onUnmounted, ref, watch } from '@asunajs/lite'
import { setPageRefresh } from '../ui/page-refresh'
import { currentPath, navigate } from '../ui/router'
import {
  changePassword,
  emptyPanelBindings,
  exportConfig,
  getAbout,
  getPushPanel,
  getSecurity,
  getStatus,
  getStorage,
  getVersion,
  listPushEvents,
  listPushRules,
  PANEL_BEHAVIOR_KEYS,
  PANEL_BEHAVIOR_LABEL,
  putPushPanel,
  putSecurity,
  putStorageLimits,
  restartService,
  type AboutView,
  type PanelBindings,
  type PushEventOption,
  type PushRule,
  type SecurityView,
  type ServiceStatus,
  type StorageLimits,
  type StorageView,
  type ThrottleMode,
  type VersionInfo,
} from '../api'
import { ConfirmDialog, closeDialog, openDialog } from '../ui/dialog'
import { showError } from '../ui/error-dialog'
import { pushToast } from '../ui/toast'
import { credentialFor } from '../ui/password'
import { StatRow, type StatItem } from '../ui/stat-row'
import { fmtClock, fmtEpochMs, fmtUptime } from '../ui/time'
import { runStatusOf, statusBadge, statusLabel } from '../ui/history'
import { IconAlert, IconBrand } from '../ui/icons'
import { PushRuleSelect } from '../ui/push-rule-select'
import { Select } from '../ui/select'

type LoadState = 'loading' | 'error' | 'ready'

/** 「运行时间」得自己往前走，30 秒一格（与总览页同一个口径）。 */
const TICK_MS = 30_000

// ── 顶部四节 tab（2026-10-08 用户口径「设置页面内容太多了，也没有分页面」）──────

/** 四节的 id。顺序即 tab 顺序，也是门禁认的顺序 ✓。 */
type Tab = 'service' | 'security' | 'events' | 'about'

/** tab 的标签（顺序照用户口径：服务 / 安全 / 事件推送 / 关于 ✓）。 */
const TABS: readonly (readonly [Tab, string])[] = [
  ['service', '服务'],
  ['security', '安全'],
  ['events', '事件推送'],
  ['about', '关于'],
]

/**
 * 地址 → tab。
 *
 * ⚠ **老地址 `/settings` 必须落在第一节（服务）** ✓ —— 书签、浏览器历史、
 * 侧栏 / dock 上那条「设置」（`ui/nav.ts` 的 `href: '/settings'` ✓）走的都是它，
 * 落到空节或 404 都是把用户的东西弄丢 ✗。
 * 认不出的段（`/settings/xxx`）也一律回服务节：宁可多显示一节，也不要白屏 ✓。
 */
const tabOf = (path: string): Tab => {
  const seg = path.startsWith('/settings/') ? path.slice('/settings/'.length) : ''
  return seg === 'security' || seg === 'events' || seg === 'about' ? seg : 'service'
}

/** tab → 地址（服务节用**老地址** `/settings` ✓，其余各自一段 ✓）。 */
const hrefOf = (tab: Tab): string => (tab === 'service' ? '/settings' : `/settings/${tab}`)

/** 会话时长的可选项（小时）。后端允许 1 小时 ~ 90 天。 */
const TTL_CHOICES = [
  { hours: 1, label: '1 小时' },
  { hours: 24, label: '1 天' },
  { hours: 72, label: '3 天' },
  { hours: 168, label: '7 天' },
  { hours: 720, label: '30 天' },
]

/**
 * **五个**存储上限的**范围**（与后端一致 ✓，见 `api.ts::StorageLimits`）。
 *
 * ⚠ 前端先卡一道只是为了不白跑一次往返 —— **权威在后端**（它会回一句人话的 400 ✓）。
 * 两边范围写不一样的表现是"前端说可以、后端拒绝"（或反过来，白等一次请求 ✗）。
 *
 * ⚠ 后两格是**服务端日志**（WebUI「日志」页那份 ✓）：`serverLogMemLines` 是**内存**里
 * 留多少行（它不影响"能不能看到历史"—— 更早的在磁盘上、翻页时按需读 ✓）、
 * `serverLogTotalMb` 是那个目录的**磁盘**总量 ✓。
 */
const LIMIT_RANGE = {
  historyLimit: [20, 2000],
  logLimitPerRun: [200, 20000],
  logTotalMb: [1, 4096],
  serverLogMemLines: [200, 20000],
  serverLogTotalMb: [1, 4096],
} as const

/** 一位小数，**整数就不带小数点**（`64` / `556`，而不是 `64.0` / `556.0`）。 */
const round1 = (v: number): string => (Number.isInteger(v) ? String(v) : v.toFixed(1))

/**
 * 字节数说人话。只到 MB —— 这个量级的目录报到 GB 反而看不出差别。
 *
 * ⚠ 整数不带 `.0`（见 [`round1`]）：这一块要写成「占用 / 上限」那样的一行
 * （`556 KB / 64 MB` ✓），两串数并排时那个 `.0` 只是噪音 ✓。
 */
const fmtBytes = (n: number): string => {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${round1(n / 1024)} KB`
  return `${round1(n / (1024 * 1024))} MB`
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

// ── 「事件推送」那四行的草稿（2026-10-08 用户口径）────────────────────────

/**
 * 一行的草稿。
 *
 * ⚠ 冷却 / 上限在界面上是**分钟**与**条数**，落盘是 `cooldownSecs` / `dailyCap`
 * （见 `api.ts::PushThrottle` ✓）—— 提交时才换算。
 * 两格都存**字符串**：数字框"正在输入"的空串不该被 `Number()` 吃成 0 ✗。
 */
interface PanelRowDraft {
  rules: string[]
  mode: ThrottleMode
  /** 冷却（分钟）。`'0'` = 不冷却 ✓。 */
  cooldownMins: string
  /** 每日上限（条）。`'0'` = 不限 ✓。 */
  dailyCap: string
  /**
   * 「耗时异常」的倍数判据（只有那一行有这一格 ✓）。
   * **空串 = 用户没动过** ⇒ 提交 `null` = 跟随内置默认 3.0 ✓。
   */
  slowFactor: string
  /** 「连续失败」的条数判据（只有那一行有这一格 ✓）。空串 = 跟随内置默认 3 ✓。 */
  failStreak: string
}

/** 触发方式的两个选项（用户口径：每次触发都推 / **只在状态变化时推** ✓）。 */
const THROTTLE_MODES: { value: ThrottleMode; label: string }[] = [
  { value: 'every', label: '每次触发都推' },
  { value: 'onChange', label: '只在状态变化时推' },
]

/** 每行一句话说明（界面上那几个字，不是 markdown ⇒ 别写 `**粗体**` ✗）。 */
const PANEL_BEHAVIOR_HINT: Record<string, string> = {
  loginNew: 'WebUI 有新设备登录时',
  loginFailed: '登录连续失败时',
  perfSlow: '某次运行耗时远超同一任务的历史中位数时',
  perfFailStreak: '同一任务连续失败时',
}

/** 服务端那份 → 草稿。秒 → 分钟（整数，避免 1800 秒显示成 30.000001 ✓）。 */
const panelDraftOf = (b: PanelBindings): Record<string, PanelRowDraft> => {
  const out: Record<string, PanelRowDraft> = {}
  for (const k of PANEL_BEHAVIOR_KEYS) {
    out[k] = {
      // ⚠ `?? []`：后端对空数组是 `skip_serializing_if` ⇒ 这一格可能压根不在 ✗
      rules: [...(b[k].rules ?? [])],
      mode: b[k].throttle.mode,
      cooldownMins: String(Math.round(b[k].throttle.cooldownSecs / 60)),
      dailyCap: String(b[k].throttle.dailyCap),
      // `null` / 缺省 = 用户没动过 ⇒ 输入框留空（**不写 3**：那会假装用户设过 ✗）
      slowFactor: b[k].slowFactor == null ? '' : String(b[k].slowFactor),
      failStreak: b[k].failStreak == null ? '' : String(b[k].failStreak),
    }
  }
  return out
}

/** 非负整数；空串 / 乱填 → `0`（= 不设限 ✓，与后端同义）。 */
const nonNegInt = (s: string): number => {
  const n = Number(s)
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0
}

/** 正数；**空串 / 乱填 → `null`**（= 用户没动过 ⇒ 跟随内置默认 ✓，不是 0 ✗）。 */
const positiveOrNull = (s: string): number | null => {
  if (s.trim() === '') return null
  const n = Number(s)
  return Number.isFinite(n) && n > 0 ? n : null
}

/** 草稿 → 线格式（分钟 → 秒 ✓；空阈值 → `null` = 跟随内置默认 ✓）。 */
const panelBindingsOf = (d: Record<string, PanelRowDraft>): PanelBindings => {
  const out = {} as PanelBindings
  for (const k of PANEL_BEHAVIOR_KEYS) {
    const row = d[k]
    out[k] = {
      rules: [...(row?.rules ?? [])],
      throttle: {
        mode: row?.mode ?? 'every',
        cooldownSecs: nonNegInt(row?.cooldownMins ?? '0') * 60,
        dailyCap: nonNegInt(row?.dailyCap ?? '0'),
      },
      // ⚠ 两格阈值**只在自己那一行有意义**：别的行一律 `null` ✓
      //（界面不给它们输入框，用户也就不会"设过" ✗）。
      slowFactor: k === 'perfSlow' ? positiveOrNull(row?.slowFactor ?? '') : null,
      failStreak: k === 'perfFailStreak' ? positiveOrNull(row?.failStreak ?? '') : null,
    }
  }
  return out
}

export const SettingsPage = () => {
  /**
   * 当前那一节（顶部四节 tab ✓）。
   *
   * ⚠ 初值在**组件体**里读 `currentPath`：组件体只跑一次、且不在任何 effect 里
   * ⇒ 不会给外壳那条页面槽挂上地址依赖 ✓（否则"从别的页切到 `/settings/security`"
   * 会先按旧地址把这一页建一遍 ✗）。与 `history-logs-page.tsx` 同款 ✓。
   */
  const tab = ref<Tab>(tabOf(currentPath.value))

  /**
   * ⚠ 只在地址落回**本页这四个**时才动 tab：切去 `/push` 之类的地址时 `currentPath`
   * 也会变，而那一刻这一页马上就要被卸载 ⇒ 不过滤就是白重建一次当前节 ✗。
   */
  watch(currentPath, (p) => {
    if (p === '/settings' || p.startsWith('/settings/')) tab.value = tabOf(p)
  })

  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  const version = ref<VersionInfo | null>(null)
  const status = ref<ServiceStatus | null>(null)
  const security = ref<SecurityView | null>(null)
  const about = ref<AboutView | null>(null)
  const storage = ref<StorageView | null>(null)
  const now = ref(Date.now())

  // 改口令表单（三个框：当前 + 新 + 确认）
  const curPwd = ref('')
  const newPwd = ref('')
  const confirmPwd = ref('')
  const changing = ref(false)

  // 策略表单（与已存值分开：改到一半时页面上要能看出"还没保存"）
  const minLen = ref(8)
  const ttlHours = ref(168)
  const savingPolicy = ref(false)

  /**
   * 存储上限那**五个**框（2026-10-09 用户口径「日志和历史不要全加入内存，使用本地记录，
   * 让用户设置最大大小」✓）。
   *
   * ⚠ 五格都存**字符串**：数字框"正在输入"的空串不该被 `Number()` 吃成 0 ✗
   * （空串会变成"0 条"、看着像用户设的，其实他正在改 ✓ —— 与事件推送那几格同款）。
   *
   * ⚠ 后两格是**服务端日志**（WebUI「日志」页那份：调度器 / 上游 http / 认证 ✓）——
   * 与前三格（运行历史 / 运行日志）管的是**两个不同的目录** ✓，别混 ✓。
   */
  const limitHistory = ref('')
  const limitPerRun = ref('')
  const limitTotalMb = ref('')
  const limitServerLogMem = ref('')
  const limitServerLogTotalMb = ref('')
  const savingLimits = ref(false)
  /** 保存失败那句（**就近**贴在保存键旁边；同时也会弹一次窗，见 `saveLimits()` ✓）。 */
  const limitsError = ref('')

  // 重启
  const restarting = ref(false)
  const restartWaited = ref(0)

  /**
   * 导出配置（2026-10-12 用户口径：「**导出功能最好也能在 webui 使用**」✓）。
   *
   * ⚠ 只放**导出**，不放导入：导入是**覆盖式**操作（文件里有的那一类整类替换，
   * 连账号与凭据一起 ✓），一键覆盖风险高 ⇒ 界面只放导出，导入走
   * `mcloud config import <文件>`（用户口径「导入不要做界面」✓）。
   */
  const exporting = ref(false)

  // ── 事件推送（**面板行为**：登录 / 性能那四类，2026-10-08 用户口径）────────
  //
  // ⚠ 这一块是**行为**那一侧（「什么时候发」）：每行选**规则**（可多选 ✓）
  // ＋ 节流三格 ✓。规则本身（平台 / 凭据 / 标题）在「推送」页里建 ✓。
  /** 当前用户的规则清单（多选要用）。 */
  const panelRules = ref<PushRule[]>([])
  /** 事件清单（取 `kind === 'panel'` 的那四条当行标题 ✓）。 */
  const panelEvents = ref<PushEventOption[]>([])
  /**
   * 四行的草稿。
   *
   * ⚠ 冷却 / 上限在界面上是**分钟**与**条数**，落盘是 `cooldownSecs` / `dailyCap`
   * （`api.ts::PushThrottle` ✓）—— 提交时才换算，中间态存字符串
   * （数字框"正在输入"的空串不该被 `Number()` 吃成 0 ✗）。
   */
  const panelDraft = ref<Record<string, PanelRowDraft>>(panelDraftOf(emptyPanelBindings()))
  const panelSaving = ref(false)
  const panelError = ref('')

  let ticker: ReturnType<typeof setInterval> | undefined

  /**
   * 这一页的取数。
   *
   * ⚠ 这里**每一个**并发端点都必须在 fixture（`web/regress/lib.mjs`）里有一份：
   * 少一个的表现不是"那一块空着"，而是**整页错误态** ✗（这一页的注释写着这条）。
   *
   * ⚠ 2026-10-08：推送那几件（渠道 / 规则 / 事件 / 渠道清单 / 定时 / 编排）**整块搬去
   * 「推送」页**了（`pages/push-page.tsx` ✓）—— 这里不能再顺手拉它们：白花的往返是小事，
   * 那些端点在别的变体里缺一个就会把**设置页**打成整页错误 ✗。
   */
  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [v, s, sec, st, ab, prs, pevs, pn] = await Promise.all([
        getVersion(),
        getStatus(),
        getSecurity(),
        getStorage(),
        getAbout(),
        listPushRules(),
        listPushEvents(),
        getPushPanel(),
      ])
      version.value = v
      status.value = s
      security.value = sec
      storage.value = st
      about.value = ab
      panelRules.value = prs
      panelEvents.value = pevs
      panelDraft.value = panelDraftOf(pn)
      panelError.value = ''
      minLen.value = sec.minPasswordLen
      ttlHours.value = sec.sessionTtlHours
      // ⚠ 老后端没有 `limits` ⇒ 五个框**留空**（那一块编辑器整个不铺，见 `storageCard()` ✓）——
      // 拿 `0` 或默认值顶上就是在假装"服务端是这么设的" ✗。
      applyLimits(st.limits)
      limitsError.value = ''
      now.value = Date.now()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => {
    setPageRefresh({ run: () => void load() })
    void load()
    ticker = setInterval(() => {
      now.value = Date.now()
    }, TICK_MS)
  })

  onUnmounted(() => clearInterval(ticker))
  /**
   * ⚠ 刷新动作的**注销必须挂在组件这一层**，不能写在某张卡的渲染函数里
   * （2026-10-08 拆 tab 时收口的）：原来那句在 `serviceCard()` 里 —— 只有**服务节**
   * 被渲染过才会登记。于是"直接开 `/settings/security`"再离开这一页时，
   * 登记处留着这一页的 `load()` ⇒ navbar 的刷新图标在**别的页**上点的还是这一页 ✗。
   */
  onUnmounted(() => setPageRefresh(null))

  /** 指标卡。读数全来自 `/api/status`，一个都不编。 */
  const metrics = (): StatItem[] => {
    const s = status.value
    if (!s) return []
    const running = s.schedulerRunning
    return [
      {
        title: '调度器',
        value: running ? '运行中' : '已停止',
        badge: running ? { text: '正常', tone: 'ok' } : { text: '已停', tone: 'warn' },
      },
      {
        title: '运行时间',
        value: fmtUptime(now.value - s.startedAtMs),
        desc: `自 ${fmtClock(s.startedAtMs)}`,
      },
      { title: '已注册任务', value: String(s.taskCount) },
      { title: '定时规则', value: String(s.scheduleCount) },
    ]
  }

  /** 一行「标签 —— 值」。值可换行，不挤标签（窄屏也放得下）。 */
  const kv = (label: string, value: JSX.Element | string) => (
    <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-sm">
      <span class="opacity-60">{label}</span>
      <span class="min-w-0 break-words text-right">{value}</span>
    </div>
  )

  const serviceCard = () => {
    const s = status.value
    if (!s) return null
    const last = s.lastRun

  return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">
              {version.value?.name ?? s.name}
              <span class="badge badge-soft badge-sm">v{version.value?.version ?? s.version}</span>
            </h2>
            <span class="badge badge-soft badge-success badge-sm">服务在线</span>
          </div>

          <StatRow items={metrics()} />

          <div class="flex flex-col gap-1.5">
            {kv('编排', `${s.pipelineCount} 条`)}
            {kv('正在运行', `${s.runningCount} 个`)}
            {kv(
              '最近一次运行',
              last ? (
                <span class="flex flex-wrap items-center justify-end gap-2">
                  <span class="opacity-80">
                    {fmtEpochMs(last.started_at_ms)} · {last.task}
                  </span>
                  <span class={`badge badge-sm ${statusBadge(runStatusOf(last))}`}>
                    {statusLabel(runStatusOf(last))}
                  </span>
                </span>
              ) : (
                '还没有跑过'
              ),
            )}
          </div>
        </div>
      </div>
    )
  }

  // ── 账户安全 ──────────────────────────────────────────────────────────

  /** 改口令的本地校验：能在前端挡住的就别浪费一次往返。 */
  const passwordProblem = (): string => {
    if (!curPwd.value) return '请填写当前口令'
    if (!newPwd.value) return '请填写新口令'
    if (newPwd.value.length < minLen.value) return `新口令至少 ${minLen.value} 位`
    if (newPwd.value !== confirmPwd.value) return '两次输入的新口令不一致'
    if (newPwd.value === curPwd.value) return '新口令与当前口令相同'
    return ''
  }

  const submitPassword = async () => {
    const bad = passwordProblem()
    if (bad) {
      pushToast('warning', bad)
      return
    }
    changing.value = true
    try {
      const scheme = security.value?.passwordScheme ?? 'sha256'
      const [cur, next] = await Promise.all([
        credentialFor(curPwd.value, scheme),
        credentialFor(newPwd.value, scheme),
      ])
      const r = await changePassword(cur, next)
      curPwd.value = ''
      newPwd.value = ''
      confirmPwd.value = ''
      // ⚠ 后端换了新 cookie，所以这一次**不会**被登出
      pushToast('success', `口令已更新，已结束 ${r.sessionsEnded} 个旧会话`)
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '改口令失败')
    } finally {
      changing.value = false
    }
  }

  const savePolicy = async () => {
    savingPolicy.value = true
    try {
      const next = await putSecurity({
        minPasswordLen: Number(minLen.value),
        sessionTtlHours: Number(ttlHours.value),
      })
      security.value = next
      minLen.value = next.minPasswordLen
      ttlHours.value = next.sessionTtlHours
      pushToast('success', '策略已保存（对之后设的口令与新登录生效）')
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '保存失败')
    } finally {
      savingPolicy.value = false
    }
  }

  /**
   * 改口令。
   *
   * ⚠ 「当前口令」这一格**必须留**（用户口径 2026-10-01："你口令更改怎么取消了旧口令"）：
   * 参照的系统只填新口令，是因为它的会话本来就是全部权限；本仓照样验旧口令 ——
   * 挡住"别人趁你没锁屏、坐下就改掉口令"。后端也是必填（`PUT /api/admin/password`）。
   *
   * 三个框**不挤一行**：当前口令整行，新/确认并排。
   */
  const passwordCard = () => {
    const sec = security.value
    if (!sec) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h2 class="card-title text-base">修改管理员口令</h2>
          {/*
            三个框**不挤一行**（用户口径："改密码不一定一行，你现在有三个输入"）：
            验证那格整行，新/确认并排 —— 正好两组语义各占一行。
          */}
          <div class="grid min-w-0 gap-3 md:grid-cols-2">
            <fieldset class="fieldset min-w-0 md:col-span-2">
              <legend class="fieldset-legend">当前口令</legend>
              <input
                class="input w-full min-w-0"
                type="password"
                autocomplete="current-password"
                value={curPwd.value}
                onInput={(e) => (curPwd.value = (e.target as HTMLInputElement).value)}
              />
            </fieldset>
            <fieldset class="fieldset min-w-0">
              <legend class="fieldset-legend">新口令</legend>
              <input
                class="input w-full min-w-0"
                type="password"
                autocomplete="new-password"
                value={newPwd.value}
                onInput={(e) => (newPwd.value = (e.target as HTMLInputElement).value)}
              />
            </fieldset>
            <fieldset class="fieldset min-w-0">
              <legend class="fieldset-legend">确认新口令</legend>
              <input
                class="input w-full min-w-0"
                type="password"
                autocomplete="new-password"
                value={confirmPwd.value}
                onInput={(e) => (confirmPwd.value = (e.target as HTMLInputElement).value)}
              />
            </fieldset>
          </div>
          <p class="text-xs opacity-60">
            口令长度至少 {sec.minPasswordLen} 个字符；改完所有旧会话都会失效（包括其它设备）。
          </p>
          <div class="card-actions justify-end">
            <button
              type="button"
              class="btn btn-sm btn-primary min-h-11 md:min-h-0"
              disabled={changing.value}
              onClick={() => void submitPassword()}
            >
              更新口令
            </button>
          </div>
        </div>
      </div>
    )
  }

  /**
   * 密码策略 + 口令存储读数。
   *
   * ⚠ **没有**「含字母 / 含数字 / 含特殊符号」那几档（参照图里有）：
   * 本项目的口令在浏览器里已经做过一次 SHA-256，服务端拿到的是 64 位摘要 ——
   * 它**量不出**明文里有几个数字、几个符号。做出来就只能挂在前端，而前端拦不住
   * `curl`，那是**假开关**。真要做，得先放弃"明文不出浏览器"。
   */
  const policyCard = () => {
    const sec = security.value
    if (!sec) return null
    const upgraded = (sec.passwordScheme ?? 'sha256') !== 'plain'
    const setMin = (v: number) => (minLen.value = Math.min(64, Math.max(4, v)))
    // ⚠ `h-full` + 下面 `card-actions mt-auto`：并排的两张卡**底边必须齐**
    // （用户口径："不要这种不平的布局" —— 一高一矮、按钮错位就是"不平"）
    return (
      <div class="card h-full bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">口令策略</h2>
            <span
              class={`badge badge-sm ${upgraded ? 'badge-soft badge-success' : 'badge-soft badge-warning'}`}
            >
              {upgraded ? '两段哈希' : '待升级'}
            </span>
          </div>

          <div class="flex flex-wrap items-center justify-between gap-3">
            <div class="min-w-0">
              <p class="text-sm">最小长度</p>
              <p class="text-xs opacity-60">限制口令的最低字符数（4~64）</p>
            </div>
            {/* 步进器：数字框配 −/+ 两个按钮，与参照图一致 */}
            <div class="join shrink-0">
              <button
                type="button"
                class="btn join-item"
                aria-label="最小长度减一"
                onClick={() => setMin(Number(minLen.value) - 1)}
              >
                −
              </button>
              <input
                class="input join-item w-16 min-w-0 text-center"
                type="number"
                min="4"
                max="64"
                value={String(minLen.value)}
                aria-label="最小长度"
                onInput={(e) => (minLen.value = Number((e.target as HTMLInputElement).value))}
              />
              <button
                type="button"
                class="btn join-item"
                aria-label="最小长度加一"
                onClick={() => setMin(Number(minLen.value) + 1)}
              >
                +
              </button>
            </div>
          </div>
          <p class="text-xs opacity-60">
            只对明文这条路径生效：浏览器那份摘要恒 64 位，服务端量不出长度。
          </p>

          <div class="flex flex-col gap-1.5 border-t border-base-300 pt-3">
            {kv(
              '口令存储',
              <span class="font-mono text-xs">SHA-256（浏览器）→ Argon2id（服务端）</span>,
            )}
            {upgraded ? null : (
              <p class="text-xs opacity-60">
                这个库还是老档（Argon2id 直接吃明文）：下次登录成功时会自动升级。
              </p>
            )}
          </div>

          <div class="card-actions mt-auto justify-end">
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={savingPolicy.value}
              onClick={() => void savePolicy()}
            >
              保存策略
            </button>
          </div>
        </div>
      </div>
    )
  }

  /** 会话控制：有效期 + 一句该提醒的话（参照图那块黄色提示的同位）。 */
  const sessionCard = () => {
    const sec = security.value
    if (!sec) return null
    return (
      <div class="card h-full bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h2 class="card-title text-base">会话控制</h2>
          <fieldset class="fieldset min-w-0">
            <legend class="fieldset-legend">会话有效期</legend>
            <Select
              id="session-ttl"
              class="w-full min-w-0"
              ariaLabel="会话有效期"
              value={String(ttlHours.value)}
              options={TTL_CHOICES.map((c) => ({ value: String(c.hours), label: c.label }))}
              onChange={(v) => {
                ttlHours.value = Number(v)
              }}
            />
            <p class="text-xs opacity-60">只影响之后新建的会话，已发出的不会被缩短。</p>
          </fieldset>

          <div class="alert alert-soft alert-warning text-xs">
            会话越长越省事，也意味着那台设备上的登录状态有效越久。公用电脑上建议选 1 小时，
            用完随手点右上角退出。
          </div>

          <div class="card-actions mt-auto justify-end">
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={savingPolicy.value}
              onClick={() => void savePolicy()}
            >
              保存会话设置
            </button>
          </div>
        </div>
      </div>
    )
  }

  // ── 存储信息 ──────────────────────────────────────────────────────────

  const pathRow = (label: string, value: string) => (
    <div class="flex flex-col gap-1">
      <span class="text-xs opacity-60">{label}</span>
      <span class="break-all font-mono text-xs">{value}</span>
    </div>
  )

  /**
   * 「日志占用」那一行（**运行**日志：`data/logs/` ✓）。
   *
   * ⚠ 新后端给了上限 ⇒ 写成 `占用 / 上限`（`556 KB / 64 MB` ✓）—— 只报占用的话，
   * 用户看不出"还剩多少、该不该调大"，而那是他打开这张卡要回答的问题 ✓。
   * 老后端没有 `logUsage` ⇒ **只报占用**（退回 `logBytes`）：少报一个数，
   * 也别编一个上限出来 ✗（编出来的那个数看着像真的，比缺一个数糟得多）。
   */
  const logUsageText = (st: StorageView): string => {
    const u = st.logUsage
    if (!u) return fmtBytes(st.logBytes)
    return `${fmtBytes(u.bytes)} / ${fmtBytes(u.maxBytes)}（${u.files} 个文件）`
  }

  /**
   * 「服务端日志」那一行（WebUI「日志」页那份：`data/server-logs/` ✓）。
   *
   * ⚠ 与 [`logUsageText`] **两个目录、两个上限**，别合并成一行 ✗ ——
   * 它们各删各的（运行日志按"单次运行的行数"、服务端日志按"最旧的整天文件" ✓）。
   * ⚠ 老后端没有这一格时**不编数**：`StorageView` 里没有 `serverLogBytes` 可退回
   * ⇒ 这一行整个不铺（见 JSX ✓）—— 摆一行"服务端日志 ——"只会让人以为占用是 0 ✗。
   */
  const serverLogUsageText = (st: StorageView): string => {
    const u = st.serverLogUsage
    if (!u) return ''
    return `${fmtBytes(u.bytes)} / ${fmtBytes(u.maxBytes)}（${u.files} 个文件）`
  }

  /**
   * 服务端**有没有**给"服务端日志"那两格上限（`limits.serverLogMemLines` /
   * `limits.serverLogTotalMb` ✓）。
   *
   * ⚠ 为什么需要它：后端那条线**重启才生效** ⇒ "前端已经有这两格、本地那个后端还没有"
   * 会短暂存在 ✓（`api.ts::StorageLimits` 那两格写成可选就是为这个）。
   * 那几分钟里**不能**硬铺这两个框：填进去的是字符串 `"undefined"`
   * （`String(undefined)` ✗），而保存时 `Number('undefined')` 是 `NaN` ⇒ 换来一句 400 ✓
   * —— 那正是"别摆一个必然失败的按钮"要避免的东西（`ui/error-dialog.ts` 的口径 ✓）。
   * 老后端下退回**今天这个样子**（三个框 ✓），新后端下五个框 ✓。
   */
  const hasServerLogLimits = (): boolean => {
    const l = storage.value?.limits
    return typeof l?.serverLogMemLines === 'number' && typeof l?.serverLogTotalMb === 'number'
  }

  /** 把后端回读的那一份填回**框里**（回填的是**服务端的值**：它可能做过归一化 ✓）。 */
  const applyLimits = (l: StorageLimits | undefined) => {
    if (!l) return
    limitHistory.value = String(l.historyLimit)
    limitPerRun.value = String(l.logLimitPerRun)
    limitTotalMb.value = String(l.logTotalMb)
    // ⚠⚠ 只填**服务端真给了的**那两格（判据同 `hasServerLogLimits` ✓）—— 少了这一道，
    // 老后端下这两个框会变成 `undefined` 那串字 ✓（理由见上面那段）。
    if (typeof l.serverLogMemLines === 'number') limitServerLogMem.value = String(l.serverLogMemLines)
    if (typeof l.serverLogTotalMb === 'number')
      limitServerLogTotalMb.value = String(l.serverLogTotalMb)
  }

  /**
   * 上限的**本地**校验：返回一句人话（哪一格、该在什么范围），没问题就空串。
   *
   * ⚠ 空串 / 乱填 / 小数一律拦下：`Number('')` 是 `0`、`Number('abc')` 是 `NaN`，
   * 直接发出去只会换来后端一句 400 —— 而这里本来就看得出来是哪一格 ✗。
   * ⚠ 后端的范围才是权威（`api.ts::StorageLimits` ✓），两边**同一组数** ✓。
   * ⚠ 服务端日志那两格**只在后端给了它们时才查**（否则那两个框根本没铺，
   * 查它们等于"用户没动过的格子把保存拦下来" ✗ —— 见 `hasServerLogLimits` ✓）。
   */
  const limitsProblem = (): string => {
    const rows: (readonly [string, string, readonly [number, number]])[] = [
      ['历史保留条数', limitHistory.value, LIMIT_RANGE.historyLimit],
      ['单次运行日志行数', limitPerRun.value, LIMIT_RANGE.logLimitPerRun],
      ['日志总量上限', limitTotalMb.value, LIMIT_RANGE.logTotalMb],
    ]
    if (hasServerLogLimits()) {
      rows.push(['服务端日志内存行数', limitServerLogMem.value, LIMIT_RANGE.serverLogMemLines])
      rows.push(['服务端日志总量上限', limitServerLogTotalMb.value, LIMIT_RANGE.serverLogTotalMb])
    }
    for (const [label, raw, [min, max]] of rows) {
      const n = Number(raw.trim())
      if (raw.trim() === '' || !Number.isInteger(n) || n < min || n > max) {
        return `${label}要填 ${min}~${max} 之间的整数（现在填的是「${raw.trim()}」）`
      }
    }
    return ''
  }

  /**
   * 保存五个上限。
   *
   * ⚠ **五格一起发**（2026-10-09 起后端是"给哪格改哪格、全不给才 400" ✓）——
   * 只发改过的那一格虽然也能成，但"少发的那格保持服务端旧值"会让
   * "界面上改了、其实没发出去"这类 bug 躲过去 ✗（五格一起发 = 与整体替换等价 ✓，
   * 与 `savePolicy()` 同一条口径 ✓）。
   *
   * 反馈按仓库规范：失败**弹窗优先 + 就近 inline**（`showError` + 卡里那句 ✓）、
   * 成功走轻量 toast ✓。这里的失败是"用户当前动作的直接答案"，塞进右上角等于没提示
   * （见 `ui/error-dialog.ts` 文件头那段分工 ✓）。
   */
  const saveLimits = async () => {
    const bad = limitsProblem()
    if (bad) {
      limitsError.value = bad
      showError('存储上限没保存', bad)
      return
    }
    savingLimits.value = true
    limitsError.value = ''
    try {
      const next = await putStorageLimits({
        historyLimit: Number(limitHistory.value),
        logLimitPerRun: Number(limitPerRun.value),
        logTotalMb: Number(limitTotalMb.value),
        // ⚠ 服务端日志那两格**只在后端支持时才发**（`hasServerLogLimits` ✓）：
        // 老后端那边它们是"不认识的字段"，发过去只会被静默丢掉（`StoragePatch` 没有
        // `deny_unknown_fields` ✓）—— 而那种"看着保存成功、其实没落盘"最费解 ✗。
        ...(hasServerLogLimits()
          ? {
              serverLogMemLines: Number(limitServerLogMem.value),
              serverLogTotalMb: Number(limitServerLogTotalMb.value),
            }
          : {}),
      })
      // 用**后端回读的那一份**替换本地读数（它同时带上新的 `logUsage` —— 占用/上限那行
      // 会立刻跟着变 ✓，用户不用刷新就能看出"上限真的改了" ✓）。
      storage.value = next
      applyLimits(next.limits)
      pushToast('success', '存储上限已保存')
    } catch (e) {
      // ⚠ 后端那句人话**原样**显示（它指名了是哪一格 ✗ 别自己翻译）
      const msg = e instanceof Error ? e.message : '保存失败'
      limitsError.value = msg
      showError('存储上限没保存', msg)
    } finally {
      savingLimits.value = false
    }
  }

  const storageCard = () => {
    const st = storage.value
    if (!st) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-4">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">存储信息</h2>
            <span class="badge badge-soft badge-sm">JSON 文件</span>
          </div>

          <div class="grid min-w-0 gap-4 md:grid-cols-3">
            {pathRow('数据目录', st.dataDir)}
            {pathRow('日志目录', st.logDir)}
            {pathRow('二进制', st.binary)}
          </div>

          <div class="flex flex-col gap-1.5">
            {kv('数据占用', fmtBytes(st.dataBytes))}
            {kv('日志占用', logUsageText(st))}
            {/*
              ⚠ **服务端日志**（WebUI「日志」页那份 ✓）—— 与上面那行「日志占用」
              （运行日志 `data/logs/` ✓）是**两个目录、两个上限** ✓，所以各占一行。
              ⚠ 老后端没有这一格 ⇒ **整行不铺**（不编一个占用数出来 ✓，理由见
              `serverLogUsageText` 上面那段）。
            */}
            {st.serverLogUsage ? kv('服务端日志', serverLogUsageText(st)) : null}
          </div>

          {/*
            ⚠ 数据文件**不写成表格**：表格在 1500 宽的卡里会变成"名字在最左、
            大小在最右"的一条长线（用户截图抱怨的正是这个）。三列成格之后，
            每格的"名字 —— 大小"自己成对，眼睛不用横穿屏幕。
            想知道"哪张表在长"时，逐项读数仍然都在。
          */}
          <div class="grid min-w-0 gap-x-4 gap-y-2 md:grid-cols-3">
            {st.files.map((f) => (
              <div
                key={f.name}
                class="flex min-w-0 items-baseline justify-between gap-2 border-b border-base-300 pb-1"
              >
                <span class="truncate font-mono text-xs">{f.name}</span>
                <span class="shrink-0 font-mono text-xs tabular-nums opacity-70">
                  {fmtBytes(f.bytes)}
                </span>
              </div>
            ))}
          </div>

          {/*
            ⭐ 日志与历史的**容量上限**（2026-10-09 用户口径「日志和历史不要全加入内存，
            使用本地记录，**让用户设置最大大小**」＋「大日志做虚拟加载或者分页」✓）。
            **五个**数字框 + 一个保存键 —— 日志现在按需读盘、分页显示（不再整份进内存 ✓），
            这五个数决定**磁盘上留多少 / 内存里留多少** ✓。

            ⚠ 前三个是**运行**那一侧（历史条数 / 单次运行的行数 / `data/logs/` 总量 ✓）、
            后两个是**服务端日志**（`data/server-logs/`：内存窗口行数 + 磁盘总量 ✓）——
            五个框摆在一起是因为它们是**同一件事**（"留多少"）的五个旋钮 ✓。

            ⚠ 这一卡**只有管理员能改**，而这不需要前端再加一道闸：`GET /api/system/storage`
            本身就是管理员接口（后端 `require_admin`，安全审计 SEC-18 ✓）——
            卡能渲染出来 ⇒ 看的人就是管理员 ✓。再造第二套判据（`currentIsAdmin`）
            只会有"两处判得不一样"的隐患 ✗，与这一页别处（口令策略 / 会话）也是同款 ✓。

            ⚠ `st.limits` 缺席 = **老后端**（这一格是刚加的）⇒ 编辑器整个不铺，
            只留一句说明：铺出来就是一个必然 400/404 的保存键 ✗
            （`ui/error-dialog.ts` 的口径：别摆一个必然失败的按钮 ✓）。
          */}
          {st.limits ? (
            <div class="flex flex-col gap-3 border-t border-base-300 pt-3">
              <div class="flex flex-wrap items-center justify-between gap-2">
                <h3 class="text-sm font-medium">日志与历史的容量上限</h3>
                <button
                  type="button"
                  class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                  disabled={savingLimits.value}
                  onClick={() => void saveLimits()}
                >
                  {savingLimits.value ? '保存中…' : '保存上限'}
                </button>
              </div>

              {/*
                ⚠ 失败那句紧贴**保存键所在的这一行下面**（用户点保存时视线就在这里）；
                弹窗那一份由 `saveLimits()` 的 `showError()` 给 ✓（规范：弹窗优先 + 就近 inline）。
              */}
              {limitsError.value ? (
                <div class="alert alert-soft alert-error text-sm" role="alert">
                  <IconAlert class="size-4 shrink-0" />
                  <span class="break-words">{limitsError.value}</span>
                </div>
              ) : null}

              <div class="grid min-w-0 gap-3 md:grid-cols-3">
                <fieldset class="fieldset min-w-0">
                  <legend class="fieldset-legend">历史保留条数</legend>
                  <input
                    id="storage-history-limit"
                    class="input w-full min-w-0"
                    type="number"
                    min="20"
                    max="2000"
                    step="1"
                    aria-label="历史保留条数"
                    value={limitHistory.value}
                    onInput={(e) => (limitHistory.value = (e.target as HTMLInputElement).value)}
                  />
                  <p class="text-xs opacity-60">运行记录最多留多少条（20~2000），超了从最旧的丢。</p>
                </fieldset>
                <fieldset class="fieldset min-w-0">
                  <legend class="fieldset-legend">单次运行日志行数</legend>
                  <input
                    id="storage-log-per-run"
                    class="input w-full min-w-0"
                    type="number"
                    min="200"
                    max="20000"
                    step="1"
                    aria-label="单次运行日志行数"
                    value={limitPerRun.value}
                    onInput={(e) => (limitPerRun.value = (e.target as HTMLInputElement).value)}
                  />
                  <p class="text-xs opacity-60">
                    一次运行最多留多少行（200~20000），也是回看时能往上翻的总量。
                  </p>
                </fieldset>
                <fieldset class="fieldset min-w-0">
                  <legend class="fieldset-legend">日志总量上限（MB）</legend>
                  <input
                    id="storage-log-total-mb"
                    class="input w-full min-w-0"
                    type="number"
                    min="1"
                    max="4096"
                    step="1"
                    aria-label="日志总量上限（MB）"
                    value={limitTotalMb.value}
                    onInput={(e) => (limitTotalMb.value = (e.target as HTMLInputElement).value)}
                  />
                  <p class="text-xs opacity-60">运行日志目录最多占多少磁盘（1~4096 MB），超了从最旧的丢。</p>
                </fieldset>
                {/*
                  ⚠⚠ 下面两格是**服务端日志**（WebUI「日志」页那份 ✓）——
                  用户口径 2026-10-09：「日志和历史不要全加入内存，使用本地记录」✓。
                  ⚠ 两格的说明**必须写清"内存 vs 磁盘"**：内存那一格调小**不会**让用户
                  看不到历史（更早的在盘上、翻页时按需读 ✓）—— 说反了会让人以为
                  "调小 = 丢日志" ✗。
                  ⚠ 两块都挂在 `hasServerLogLimits()` 上（后端还没这两格 ⇒ 不铺 ✓，
                  理由见那个函数上面那段 ✓）。
                */}
                {hasServerLogLimits() ? (
                  <fieldset class="fieldset min-w-0">
                    <legend class="fieldset-legend">服务端日志内存行数</legend>
                    <input
                      id="storage-server-log-mem-lines"
                      class="input w-full min-w-0"
                      type="number"
                      min="200"
                      max="20000"
                      step="1"
                      aria-label="服务端日志内存行数"
                      value={limitServerLogMem.value}
                      onInput={(e) =>
                        (limitServerLogMem.value = (e.target as HTMLInputElement).value)
                      }
                    />
                    <p class="text-xs opacity-60">
                      内存里只留最近这些行（200~20000）；更早的按需从磁盘读。
                    </p>
                  </fieldset>
                ) : null}
                {hasServerLogLimits() ? (
                  <fieldset class="fieldset min-w-0">
                    <legend class="fieldset-legend">服务端日志总量上限（MB）</legend>
                    <input
                      id="storage-server-log-total-mb"
                      class="input w-full min-w-0"
                      type="number"
                      min="1"
                      max="4096"
                      step="1"
                      aria-label="服务端日志总量上限（MB）"
                      value={limitServerLogTotalMb.value}
                      onInput={(e) =>
                        (limitServerLogTotalMb.value = (e.target as HTMLInputElement).value)
                      }
                    />
                    <p class="text-xs opacity-60">
                      服务端日志目录最多占多少磁盘（1~4096 MB），超出按最旧的整天文件淘汰。
                    </p>
                  </fieldset>
                ) : null}
              </div>

              <p class="text-xs leading-5 opacity-60">
                日志与历史都<strong>按需读盘</strong>（不再整份进内存）：这几个数决定磁盘与内存里留多少，
                不影响界面上一次显示多少 —— 回看时按页取。
              </p>
            </div>
          ) : (
            <p class="text-xs opacity-60">
              这份后端还没有容量设置（升级后这里可以改日志与历史各留多少）。
            </p>
          )}
        </div>
      </div>
    )
  }

  // ── 事件推送（面板行为）────────────────────────────────────────────────

  /** 一行的名字：以后端那份事件清单为准（`kind === 'panel'` ✓），认不出才用兜底文案。 */
  const panelEventLabel = (k: string): string =>
    panelEvents.value.find((o) => o.kind === 'panel' && o.key === k)?.label ??
    PANEL_BEHAVIOR_LABEL[k as keyof PanelBindings] ??
    k

  /** 改一行的草稿（⚠ 整体换新对象 —— 浅响应的 `ref` 只认引用换没换 ✗）。 */
  const setPanelRow = (k: string, patch: Partial<PanelRowDraft>) => {
    panelDraft.value = {
      ...panelDraft.value,
      [k]: { ...panelDraft.value[k], ...patch },
    }
  }

  /**
   * 保存四个面板行为的绑定。
   *
   * ⚠ 失败信息**原样显示**：后端校验不过时它指名了是哪一格，自己再翻译一遍只会把线索弄丢 ✗。
   */
  const savePanel = async () => {
    panelError.value = ''
    panelSaving.value = true
    try {
      // 用**后端回读的那一份**回填（它可能做过归一化 ✓）
      panelDraft.value = panelDraftOf(await putPushPanel(panelBindingsOf(panelDraft.value)))
      pushToast('success', '事件推送已保存')
    } catch (e) {
      const msg = e instanceof Error ? e.message : '保存失败'
      panelError.value = msg
      // ⚠ 弹窗优先（2026-10-08 用户口径）：错误类 toast 不自动消失，见 `ui/toast.ts`。
      // 同一句还会画在这张卡的**标题下面**（保存键就在标题行上 ✓）——
      // 曾经它画在卡片最底下、隔着两段说明文字 ✗
      pushToast('error', msg)
    } finally {
      panelSaving.value = false
    }
  }

  /** 「事件推送」里的一行：规则多选 + 三个节流控件 ✓。 */
  const panelRow = (k: string) => {
    const row = panelDraft.value[k]
    if (!row) return null
    const label = panelEventLabel(k)
    return (
      <div class="flex flex-col gap-3 rounded-box bg-base-200 p-3">
        <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <h3 class="text-sm font-medium">{label}</h3>
          <span class="text-xs opacity-60">{PANEL_BEHAVIOR_HINT[k] ?? ''}</span>
        </div>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">用哪几条规则</legend>
          <PushRuleSelect
            id={`panel-rules-${k}`}
            rules={panelRules.value}
            value={row.rules}
            onChange={(v) => setPanelRow(k, { rules: v })}
          />
          <p class="text-xs leading-5 opacity-60">
            一个都不选 = 这一类不推。可以选多条：每条规则各按自己的平台与凭据发。
          </p>
        </fieldset>

        <div class="grid min-w-0 gap-3 md:grid-cols-3">
          <fieldset class="fieldset min-w-0">
            <legend class="fieldset-legend">触发方式</legend>
            <Select
              id={`panel-mode-${k}`}
              class="w-full min-w-0"
              ariaLabel={`${label} 触发方式`}
              value={row.mode}
              options={THROTTLE_MODES.map((m) => ({ value: m.value, label: m.label }))}
              onChange={(v) => setPanelRow(k, { mode: v as ThrottleMode })}
            />
          </fieldset>
          <fieldset class="fieldset min-w-0">
            <legend class="fieldset-legend">冷却（分钟）</legend>
            <input
              id={`panel-cooldown-${k}`}
              class="input w-full min-w-0"
              type="number"
              min="0"
              step="1"
              aria-label={`${label} 冷却分钟`}
              value={row.cooldownMins}
              onInput={(e) => setPanelRow(k, { cooldownMins: (e.target as HTMLInputElement).value })}
            />
            <p class="text-xs opacity-60">这段时间内同一类不再重复推。0 = 不冷却。</p>
          </fieldset>
          <fieldset class="fieldset min-w-0">
            <legend class="fieldset-legend">每日上限（条）</legend>
            <input
              id={`panel-cap-${k}`}
              class="input w-full min-w-0"
              type="number"
              min="0"
              step="1"
              aria-label={`${label} 每日上限`}
              value={row.dailyCap}
              onInput={(e) => setPanelRow(k, { dailyCap: (e.target as HTMLInputElement).value })}
            />
            <p class="text-xs opacity-60">一天最多推这么多条。0 = 不限。</p>
          </fieldset>
        </div>

        {/*
          ⭐ **判据**（用户口径：「耗时异常多一个"倍数"、连续失败多一个"条数"」✓）。
          ⚠ 只有这两行有：别的行为带一格阈值没有意义（后端也只在对应的那一格读它 ✓）。
          ⚠ **留空 = 用户没动过** ⇒ 提交 `null` = 跟随内置默认（3.0 / 3 ✓）——
          预填一个 3 会让界面假装用户设过 ✗。
        */}
        {k === 'perfSlow' ? (
          <fieldset class="fieldset min-w-0 md:max-w-xs">
            <legend class="fieldset-legend">耗时异常倍数</legend>
            <input
              id="panel-slow-factor"
              class="input w-full min-w-0"
              type="number"
              min="1"
              step="0.5"
              aria-label="耗时异常倍数"
              placeholder="跟随默认 3"
              value={row.slowFactor}
              onInput={(e) => setPanelRow(k, { slowFactor: (e.target as HTMLInputElement).value })}
            />
            <p class="text-xs opacity-60">
              这次耗时超过同一任务历史中位数的这么多倍算异常。留空 = 跟随默认 3。
            </p>
          </fieldset>
        ) : null}
        {k === 'perfFailStreak' ? (
          <fieldset class="fieldset min-w-0 md:max-w-xs">
            <legend class="fieldset-legend">连续失败条数</legend>
            <input
              id="panel-fail-streak"
              class="input w-full min-w-0"
              type="number"
              min="1"
              step="1"
              aria-label="连续失败条数"
              placeholder="跟随默认 3"
              value={row.failStreak}
              onInput={(e) => setPanelRow(k, { failStreak: (e.target as HTMLInputElement).value })}
            />
            <p class="text-xs opacity-60">
              同一任务连着失败这么多回算异常。留空 = 跟随默认 3。
            </p>
          </fieldset>
        ) : null}
      </div>
    )
  }

  /**
   * 「事件推送」卡（2026-10-08 用户口径：**面板行为**的配置入口在设置里 ✓）。
   *
   * ⚠ 开头那句必须说清"这几类没有定时可绑" —— 否则用户会去定时表单里找它们 ✗。
   * ⚠ 还要写清「**恢复正常那一条永远会发**」：冷却与上限只挡"还在坏着"的重复告警，
   * 不挡"好了"那一条 —— 不说的话用户会以为它一直坏着 ✗。
   */
  const panelCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h2 class="card-title text-base">事件推送</h2>
          <div class="flex flex-wrap items-center gap-2">
            <span class="badge badge-soft badge-sm">面板行为</span>
            <button
              type="button"
              class="btn btn-sm btn-primary min-h-11 md:min-h-0"
              disabled={panelSaving.value}
              onClick={() => void savePanel()}
            >
              {panelSaving.value ? '保存中…' : '保存'}
            </button>
          </div>
        </div>

        {/*
          ⚠⚠ 失败那一句**放在标题行下面**（保存键就在那一行），不是卡片底部 ——
          用户点保存时视线在标题那一行，红字隔着两段说明跑到最底下等于没提示
          （2026-10-08 用户口径「所有的提示都是优先弹窗式」✓）。
          弹窗那一份由 `savePanel()` 里的 `pushToast('error', …)` 给 ✓。
        */}
        {panelError.value ? (
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <IconAlert class="size-4 shrink-0" />
            <span class="break-words">{panelError.value}</span>
          </div>
        ) : null}

        <p class="text-xs leading-5 opacity-60">
          这几类是<strong>没有定时可绑</strong>的事件（登录、性能），所以入口在这里：
          每一行**选规则**（可多选）—— 至于"怎么发"（平台、凭据、标题），
          在「推送」页里建的那一条条规则上。
        </p>
        <p class="text-xs leading-5 opacity-60">
          冷却与每日上限只挡<strong>重复</strong>告警：<strong>恢复正常那一条永远会发</strong>，
          不受它们影响。
        </p>

        <div class="flex flex-col gap-3">
          {/*
            ⚠ `key` 必须挂在 **`.map()` 返回的那个元素**上（lite 的 `createFor` 靠它复用 ✓）——
            `panelRow()` 是普通函数调用，把 key 写在它内部会被编译期直接拒掉
            （"这里的 key 什么都不做"✗，实测踩到）。所以外面套一层 `contents` 包装：
            它不参与布局（`display: contents`），只负责当那个"被 map 出来的元素" ✓。
          */}
          {PANEL_BEHAVIOR_KEYS.map((k) => (
            <div key={k} class="contents">
              {panelRow(k)}
            </div>
          ))}
        </div>
      </div>
    </div>
  )

  // ── 服务操作 ──────────────────────────────────────────────────────────

  /**
   * 重启：先让后端受理，再**轮询**等它回来。
   *
   * ⚠ 那两下之间服务是真的不在：`restartService` 成功**不代表**重启完成
   * （后端 300ms 后才优雅退出）。所以这里必须自己等 `/api/status` 重新答话 ——
   * 不等的话界面会在"服务正下线"的窗口里显示一堆网络错误。
   */
  const doRestart = async () => {
    closeDialog('confirm-restart')
    restarting.value = true
    restartWaited.value = 0
    try {
      await restartService()
    } catch {
      // 服务已经下线 ⇒ 请求被截断是**预期**的，不是失败
    }
    for (let i = 0; i < 90; i++) {
      await sleep(1000)
      restartWaited.value = i + 1
      try {
        await getStatus()
        break
      } catch {
        // 还没起来，接着等
      }
    }
    restarting.value = false
    pushToast('success', '服务已重新上线')
    await load()
  }

  /**
   * 把一段文本存成文件（浏览器那一侧唯一要做的事）。
   *
   * ⚠ 用 `<a download>` + 对象 URL，而不是 `window.location = url`：
   * 后者会把**当前页面**导航走（拿到 JSON 就把界面顶掉了 ✗），
   * 而 `download` 属性让浏览器只存文件、不动页面 ✓。
   * ⚠ 对象 URL 用完要释放（它一直占着那份内容 ✓）—— 放到下一个宏任务里释放：
   * 当场 `revoke` 在个别浏览器上会把刚发起的下载掐掉 ✓。
   */
  const saveTextFile = (filename: string, text: string): void => {
    const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }))
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    document.body.appendChild(a)
    a.click()
    a.remove()
    setTimeout(() => URL.revokeObjectURL(url), 0)
  }

  /**
   * 导出配置（`GET /api/config/export` ✓）。
   *
   * ⚠ 导的是**当前登录用户自己**那份（per-owner ✓，服务端按会话认人）；
   * 文件名由服务端 `Content-Disposition` 给（带用户名与时间戳 ✓）。
   *
   * ⚠ 失败走 `showError`（弹窗）：它是**独立动作**（点了按钮、没有表单在场 ✓，
   * 与「重启服务」「运行任务」同一条口径 ✓）；而"连不上后端"那句已经是中文，
   * 由 `api.ts::request` 那一层统一翻好 ✓，这里**原样透出**（再翻一遍只会把线索弄丢 ✗）。
   */
  const doExport = async () => {
    exporting.value = true
    try {
      const { filename, json } = await exportConfig()
      saveTextFile(filename, json)
      pushToast('success', `已导出配置：${filename}`)
    } catch (e) {
      showError('导出配置失败', e instanceof Error ? e.message : '导出失败')
    } finally {
      exporting.value = false
    }
  }

  const actionCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3 md:flex-row md:items-center md:justify-between">
        <div class="min-w-0">
          <h2 class="card-title text-base">服务操作</h2>
          <p class="mt-1 text-sm opacity-70">
            重启 = 让服务进程优雅退出，由 launchd / systemd 重新拉起。
            正在跑的任务会被中止；数据在退出前已经落盘。
          </p>
          {/*
            ⚠ 「导出配置」那三句要说全（用户口径 2026-10-12 ✓）：
            ① 这文件**含凭据**（等于登录态）⇒ 别乱发、别进版本库 ✓；
            ② 它是给**另一台机器**用的 ⇒ 目标机器上怎么用（`mcloud config import <文件>`）✓；
            ③ **导入在 CLI 里**（界面只放导出 ✓）—— 不说的话用户会在这页上找导入按钮 ✗。
          */}
          <p class="mt-1 text-sm opacity-70">
            导出配置 = 把本账号名下的配置与**账号凭据**打成一个 JSON 下载下来，
            拷到另一台机器上用 <code>mcloud config import &lt;文件&gt;</code> 导入，
            那边不用重新登录就能跑任务。这份文件等于你的登录态，别乱发、也别提交进版本库。
          </p>
        </div>
        {restarting.value ? (
          <div class="flex shrink-0 flex-wrap items-center gap-2 text-sm" role="status">
            <span class="loading loading-sm text-primary" />
            <span>重启中… 已等待 {restartWaited.value} 秒</span>
          </div>
        ) : (
          <div class="card-actions shrink-0">
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={exporting.value || state.value !== 'ready'}
              onClick={() => void doExport()}
            >
              {exporting.value ? (
                <>
                  <span class="loading loading-sm" />
                  导出中…
                </>
              ) : (
                '导出配置'
              )}
            </button>
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={state.value !== 'ready'}
              onClick={() => openDialog('confirm-restart')}
            >
              重启服务
            </button>
          </div>
        )}
      </div>
    </div>
  )

  /**
   * 「关于」卡片：品牌 + 版本信息 + 更新说明 + 版权声明。
   *
   * # 数据都是从哪来的
   *
   * - 名字/版本：`GET /api/system/about`（编译期 `CARGO_PKG_VERSION`）；
   * - 构建时间 / 提交号 / 是否脏：**构建期**从 `git`、`date` 抓下来写进二进制
   *   （`crates/mcloud-server/build.rs`）—— 运行期不需要仓库在场；
   * - 更新说明：本次构建包含的最近 12 条提交。
   *
   * ⚠ **为什么不做「检查更新」**：上游仓库是私有的（匿名访问 404），而这个服务在
   * 浏览器端没有任何凭据去查发布。摆一个必然失败的按钮，用户只会以为是自己网络的问题。
   *
   * ⚠ 版权行的署名用的是本机 launchd 标签 / 工作区里的 `catlair`，年份取当年
   * （写死 2026 的话，过一年这个声明自己就过期了）。
   */
  const aboutCard = () => {
    const ab = about.value
    const year = new Date().getFullYear()
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-4">
          <div class="flex flex-wrap items-center gap-3">
            <span class="flex size-11 shrink-0 items-center justify-center rounded-box bg-primary text-primary-content">
              <IconBrand class="size-6" />
            </span>
            <div class="min-w-0">
              <h2 class="card-title text-base">{ab?.name ?? 'mcloud'}</h2>
              <p class="text-xs opacity-60">中国移动云盘自动化工具（自用）</p>
            </div>
            <div class="ml-auto flex flex-wrap items-center justify-end gap-2">
              <span class="badge badge-soft badge-sm">v{ab?.version ?? '—'}</span>
              {ab?.commit ? (
                <span class="badge badge-soft badge-sm font-mono">{ab.commit}</span>
              ) : null}
              {ab?.profile ? <span class="badge badge-soft badge-sm">{ab.profile}</span> : null}
              {ab?.dirty ? (
                <span
                  class="badge badge-soft badge-warning badge-sm"
                  title="构建时工作区有未提交改动 —— release 会把它们一起编进线上，所以线上跑的不等于任何一个提交"
                >
                  有未提交改动
                </span>
              ) : null}
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            {kv('构建时间', ab?.buildTime || '—')}
            {kv('版本', `${ab?.name ?? 'mcloud'} ${ab?.version ?? ''}`.trim())}
          </div>

          {/* 更新说明 */}
          <div class="flex flex-col gap-1.5 border-t border-base-300 pt-3">
            <h3 class="text-sm font-medium">更新说明</h3>
            <p class="text-xs opacity-60">
              本次构建包含的改动（来自构建机上的 git 记录）：
            </p>
            {ab && ab.changes.length > 0 ? (
              <ul class="flex flex-col gap-1">
                {ab.changes.map((c) => (
                  <li key={c.hash} class="flex min-w-0 items-baseline gap-2 text-xs">
                    <span class="shrink-0 font-mono opacity-60">{c.hash}</span>
                    <span class="min-w-0 break-words">{c.subject}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p class="text-xs opacity-60">构建机上没有 git 记录，这一栏是空的。</p>
            )}
            <p class="text-xs opacity-60">
              上游仓库是私有的，这个界面没有凭据去联网查发布 —— 所以这里不提供
              「检查更新」，只列本机这份构建实际带了什么。
            </p>
          </div>

          <footer class="border-t border-base-300 pt-3 text-xs opacity-60">
            <p>{`${ab?.name ?? 'mcloud'} ${ab?.version ?? ''}`.trim() + ` · Copyright © ${year} catlair · Apache-2.0`}</p>
            <p class="mt-1">按 Apache-2.0 许可发布，按「现状」提供，不含任何担保。</p>
          </footer>
        </div>
      </div>
    )
  }

  /**
   * 顶部四节 tab。
   *
   * ⚠ 用本仓自建的**分组 tab**（`join` + `btn join-item`，与 `tasks-page.tsx` 的分组 tab、
   * `history-logs-page.tsx` 的「历史 / 日志」同款 ✓），**不用** daisyUI 的 `.tab` ——
   * 它在 `app.css` 顶部 `@theme` 那段注释里被排除过：用了**静默不生成样式**，
   * 界面退化成"一串连在一起的纯文字"（实测 padding / 边框 / `::after` 全为 0/none ✗）。
   *
   * ⚠ `overflow-x-auto` + `whitespace-nowrap`：320 宽下宁可这一排**自己横向滚**
   * （页面本身绝不横向溢出 ✓），也不要让标签折行、或把内容顶出去 ——
   * 与 `tasks-page.tsx` 那一排同一条口径 ✓。
   * 实测：四个标签（2~4 字）在 320 宽下**本来就放得下**，这一句是兜底 ✓。
   */
  const tabs = () => (
    <div role="tablist" class="join w-full overflow-x-auto" aria-label="设置分组">
      {/*
        ⚠ **解构参数 + 不写 `key`** —— 这不是漏了，是照 `history-logs-page.tsx` 那一排
        固定 tab 的写法 ✓：lite 的 `.map()` 只认**单个标识符参数**的表达式体
        （`compiler.js::mapCallback`：`ps.some((p) => !isIdentifier(p.name))` ⇒ 解构一律走
        通用路径），而 `key` 只有那条 `for` 路径会认领 —— 写在这里编译期直接抛
        「这里的 key 什么都不做」✗（实测踩到）。
        通用路径的代价是"换 tab 时这一排整块重建"，四个按钮，无所谓 ✓。
      */}
      {TABS.map(([id, label]) => (
        <button
          type="button"
          role="tab"
          aria-selected={tab.value === id}
          class={`btn join-item btn-sm min-h-11 whitespace-nowrap md:min-h-0 ${
            tab.value === id ? 'btn-primary' : 'btn-soft'
          }`}
          // 点 tab = 换地址（应用内跳转一律走 `navigate()` ✓）——
          // 于是"点 tab"与"直接开那个地址"是**同一条路** ✓（浏览器后退也切得动 ✓）。
          onClick={() => navigate(hrefOf(id))}
        >
          {label}
        </button>
      ))}
    </div>
  )

  return (
    /**
     * 桌面形态：**用满宽度，但要有结构**。
     *
     * 第一版（2026-10-01）我把设置页做成了单列，于是 1512 宽的屏上每张卡都拉满 ——
     * 表格里"文件名在最左、大小在最右"，眼睛要横穿整屏（用户原话：
     * "pc端这样太宽了视觉体验差"），随后他贴了另一套系统的同页做参照：
     * **两张卡并排、路径三列、读数成格**。
     *
     * 所以这里的口径是：
     * - `max-w-7xl`：超大屏别无限拉（2560 宽的显示器上不然每行字都太长）；
     * - 卡内用 `grid` 分栏：路径 / 数据文件各占三列，安全节里「口令策略 | 会话控制」
     *   在 `lg` 起并排（⚠ 服务操作**不跟存储信息并排**，理由见 `actionCard()` 上面那段）；
     * - 窄屏一切照旧堆叠（`grid` 的默认单列）。
     */
    <div class="mx-auto flex w-full max-w-7xl flex-col gap-4">

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (
        <>
          {tabs()}

          {/*
            ⚠ 四节**只渲染当前那一节**（不是四份都渲染、各用 CSS 藏一份 ✗）——
            省掉同时挂着的三份无关控件，也把"别的节的卡不在这里"变成**结构性**的事实 ✓
            （门禁就钉这一条：点「安全」之后服务那几张卡**必须整个不在** ✓）。
            本框架没有 diff 复用那一步（槽重跑就是新建实例 ✓）⇒ 这里不需要 `key`（加了会抛错 ✗）。
          */}
          {tab.value === 'service' ? (
            <>
              {serviceCard()}
              {storageCard()}
              {/*
                ⚠ 「服务操作」**不跟存储信息并排**（第一版并排过）：它只有一段话 + 一个按钮，
                而存储信息有 7 个数据文件 —— 等高会对着一张几乎空的卡，不等高就是"不平"。
                它改成一整行、按钮放右边，两边都不空。
              */}
              {actionCard()}
            </>
          ) : null}

          {tab.value === 'security' ? (
            <>
              {passwordCard()}
              {/* 「口令策略 | 会话控制」并排 —— 与参照图同一结构（两件互不相关的事，别叠成一列） */}
              <div class="grid min-w-0 gap-4 lg:grid-cols-2">
                <div class="min-w-0">{policyCard()}</div>
                <div class="min-w-0">{sessionCard()}</div>
              </div>
            </>
          ) : null}

          {/*
            ⭐ 「事件推送」卡（面板行为的配置入口 ✓）—— 用户口径 2026-10-08 第二次：
            它**留在设置里**（登录 / 性能这四类**没有定时可绑** ✓）；第三次口径（拆 tab）
            也只是把它挪成**一节**，卡的**内容一个字没改** ✓。
          */}
          {tab.value === 'events' ? panelCard() : null}

          {tab.value === 'about' ? aboutCard() : null}

          {/*
            ⚠ 2026-10-08：「推送渠道」与「推送规则」两张卡**整块搬去「推送」页**了
            （`pages/push-page.tsx` ✓，用户口径「并单独给推送一个」）——
            这里**删掉**、不是藏起来 ✗：留在这一页就等于有两个配置推送的入口，
            迟早两边分叉 ✓。四节里也**没有**它们 ✓。
          */}
        </>
      ) : null}

      <ConfirmDialog
        id="confirm-restart"
        title="重启服务"
        impact={
          <span>
            将让 mcloud 服务进程优雅退出，由托管方重新拉起 ——
            <strong>正在跑的任务会被中止</strong>。这段时间这个页面会连不上后端，
            起来之后自动恢复（通常几秒到二十几秒；刚重新编译过的话，第一次启动可能要等系统授权）。
          </span>
        }
        confirmLabel="重启"
        onConfirm={() => void doRestart()}
        busy={restarting.value}
      />
    </div>
  )
}
