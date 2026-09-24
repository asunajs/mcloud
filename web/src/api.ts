/**
 * 与后端的最小 API 客户端。
 *
 * 只做三件事：同源 cookie、JSON 形状、把后端的 `{code, message}` 变成可显示的文案。
 * 类型在 P5 会由 `openapi.json` 生成（方案 §5），届时这里的手写类型全部替换 ——
 * 这也是为什么此处**不**下功夫写复杂抽象：它注定要被生成物取代。
 */

/** 后端错误响应的形状（`{code, message}`，见 `internal/server/error.go` 的 `APIError.Write`）。 */
export interface ApiErrorBody {
  code: string
  message: string
}

/** 接口调用失败：带上后端的 code，调用方可以据此分支。 */
export class ApiError extends Error {
  readonly code: string
  readonly status: number

  constructor(status: number, body: ApiErrorBody) {
    // 后端 4xx 的文案是写给人看的，直接透出；5xx 只会是类别文案
    super(body.message || '请求失败')
    this.name = 'ApiError'
    this.status = status
    this.code = body.code || 'unknown'
  }
}

export interface User {
  id: string
  name: string
  role: string
}

export interface SetupStatus {
  needsSetup: boolean
  /** 口令长度下限。**由后端给**，前端不硬编 —— 否则会漂移成"前端说可以、后端拒绝"。 */
  minPasswordLen: number
}

export interface SessionInfo {
  user: User
  expiresAt?: string
}

export interface SetupResult {
  user: User
  claimed: { accounts: number; schedules: number; pipelines: number }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    // 同源请求浏览器本来就会带 cookie，写出来是为了让"会话靠 cookie 携带"这件事显式
    credentials: 'same-origin',
    headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
    ...init,
  })

  if (!response.ok) {
    let body: ApiErrorBody = { code: 'unknown', message: `HTTP ${response.status}` }
    try {
      const parsed = (await response.json()) as Partial<ApiErrorBody>
      body = { code: parsed.code ?? body.code, message: parsed.message ?? body.message }
    } catch {
      // 不是 JSON（比如反代返回了 HTML 错误页）—— 保留上面那句兜底文案
    }
    throw new ApiError(response.status, body)
  }

  if (response.status === 204) {
    return undefined as T
  }
  return (await response.json()) as T
}

/** 是否需要初始化首个管理员。 */
export const setupStatus = () => request<SetupStatus>('/api/setup')

/** 建首个管理员（成功后即处于已登录状态）。 */
export const runSetup = (name: string, password: string) =>
  request<SetupResult>('/api/setup', { method: 'POST', body: JSON.stringify({ name, password }) })

/** 登录。 */
export const signIn = (name: string, password: string) =>
  request<SessionInfo>('/api/session', { method: 'POST', body: JSON.stringify({ name, password }) })

/** 云盘账号一行（对位后端 `routes.go` 的 `accountPayload`）。 */
export interface Account {
  id: string
  /** 展示名（昵称 → 脱敏手机号 → id），**由后端挑好**，前端不再自己选。 */
  displayName: string
  nickname: string | null
  phoneMasked: string | null
  /** `pc` / `mobile`；旧导入的数据可能是 null。 */
  platform: string | null
  enabled: boolean
  sortOrder: number
  cloudCurrent: number | null
  cloudUpdatedAt: string | null
  /** 令牌到期时间（epoch 毫秒）。 */
  authExpire: number
  /** "是否过期"由**服务端时钟**给结论，前端不拿本地时钟再比一次（§5.9 同理）。 */
  authExpired: boolean
  lastOkAt: string | null
  failStreak: number
  note: string | null
}

export const listAccounts = () => request<{ accounts: Account[] }>('/api/accounts')

/**
 * 一天的云朵快照（差值由采集侧算好落库，见后端 `cloudStats` 注释）。
 *
 * ⚠ 两个差值是 `number | null`：**null 表示这一条没有基准**（该账号第一次采到数），
 * 不是"增量为 0"。渲染时必须分开 —— `0` 念"今天没涨"，`null` 念"还没得比"，
 * 混起来的表现是新加的号被读成"签了没给豆"。
 */
export interface CloudStatRow {
  /** `YYYY-MM-DD`，按**服务端本地时区**切日。 */
  date: string
  cloudCount: number
  cloudDiff: number | null
  cloudDiffWeek: number | null
}

/** 某个账号最近 `days` 天的云朵日快照。id 里有 `*` 等字符，必须 encode。 */
export const getCloudStats = (id: string, days = 7) =>
  request<{ stats: CloudStatRow[] }>(
    `/api/accounts/${encodeURIComponent(id)}/cloud-stats?days=${days}`,
  )

/** 当前身份；未登录时抛 `ApiError`（code = `unauthorized`）。 */
export const currentSession = () => request<SessionInfo>('/api/session')

/** 登出。 */
export const signOut = () => request<void>('/api/session', { method: 'DELETE' })

/**
 * 后端自述（对位 `routes.go` 的 `getStatus`）。侧栏底部那一行用它。
 *
 * ⚠ `lastRun.status` 是**存储层的生命周期**（running / done / failed），
 * 不等于业务结论 —— `done` 可以是"全成 / 部分成 / 需人工"三种。
 * 所以那一行只把它当**文字**显示，不拿它决定红绿（后端 `runPayload` 同一条警告）。
 */
export interface ServiceStatus {
  name: string
  version: string
  /**
   * 调度循环在不在跑。**现在是运行时实况**（后端起了循环才为真），不再是常量。
   *
   * 它为假时定时任务页会顶一条黄色提示：那一页列出的"下次执行"仍然算得出来，
   * 只是到点没有进程去跑 —— 少这一句，列表看着就像在工作。
   */
  schedulerRunning: boolean
  /** 最近一次执行；库里一条记录都没有时为 null。 */
  lastRun: {
    id: string
    capability: string
    status: string
    startedAt: string
    finishedAt: string | null
  } | null
}

/** 拉一次后端自述。这条请求成功与否本身就是"连没连上后端"的判据。 */
export const serviceStatus = () => request<ServiceStatus>('/api/status')

/**
 * 添加一个账号：把一串**已经在用户手里**的 `auth` 交给后端。
 *
 * ⚠ 这条的入参就是凭据本体，所以三条纪律一起成立：
 *  - 不进任何日志/缓存键（不写 console，不写 localStorage，不放进 URL）；
 *  - 响应是后端的**展示形状**（`Account`），不是入参的回显；
 *  - 失败文案由后端给，那些句子是我们自己写的，**不含**用户贴进来的那串。
 */
export const addAccount = (auth: string) =>
  request<Account>('/api/accounts', { method: 'POST', body: JSON.stringify({ auth }) })

/** 启用/停用一个账号。**只**动那一列（整行覆盖会把昵称、排序一起抹掉）。 */
export const setAccountEnabled = (id: string, enabled: boolean) =>
  request<Account>(`/api/accounts/${encodeURIComponent(id)}/enabled`, {
    method: 'PUT',
    body: JSON.stringify({ enabled }),
  })

/** 删除账号，连同它的凭据与历史（连带范围由后端存储层定）。 */
export const deleteAccount = (id: string) =>
  request<void>(`/api/accounts/${encodeURIComponent(id)}`, { method: 'DELETE' })

/** 一次刷新的结论（对位后端 `auth_refresh.go` 的 `refreshAuthPayload`）。**里面没有凭据**。 */
export interface RefreshAuthResult {
  accountId: string
  /** `pc` / `mobile`：库里那一串 `Authorization` 的第一段，原样回传。 */
  end: string
  /**
   * 判据档位（稳定英文码）：`needed` / `not-yet` / `manual-only`。中文由界面翻。
   *
   * ⚠ 这条路由**固定直接换 token**，所以 `not-yet` 只会和 `rotated: true` 一起出现
   * （= "还没到批量那条路用的提前量，但按你这次点击换了"）。界面不看它，
   * 只看 `rotated` 与那一行的"已过期 / 还在期内"。
   */
  decision: string
  /** 真的换成了新凭据。`false` + `decision` 说明"没打上游"或"不需要打"。 */
  rotated: boolean
  /** epoch **毫秒**；null = 那一侧根本没有过期信息。 */
  beforeExpire: number | null
  afterExpire: number | null
}

/**
 * 换一个新 token：会**打一次上游**，并把库里那套凭据轮换掉。
 *
 * ⚠ `platform` 只在走 PC 那一支时才有意义（库里 `accounts.platform` 记的是**端别**
 * `pc`/`mobile`，不是操作系统，所以 Windows/macOS 只能由点这一枚的人拍板；
 * 留空即后端默认 windows）。响应回的是后端的展示形状（`Account`），不是入参回显；
 * 新 token 只落库，**不在这条响应里**。
 *
 * 上游最慢实测 28.4 秒且**不自动重试**（重试会白打一遍，而且刷新是轮换真凭据的），
 * 所以调用方要自己挡住连点。
 */
export const refreshAccountAuth = (id: string, platform: '' | 'windows' | 'macos' = '') =>
  request<{ refresh: RefreshAuthResult; account: Account }>(
    `/api/accounts/${encodeURIComponent(id)}/refresh-auth`,
    { method: 'POST', body: JSON.stringify({ platform }) },
  )

/**
 * 登录移动云盘账号：两条路共用的一张"会话表"。
 *
 * 为什么是轮询而不是 SSE：一次登录最长 120 秒，而这期间只有四个阶段值要通知；
 * 页面刷新后要能续上，而续上靠的正是服务端那张表（见 `internal/server/login.go`）。
 */
export type LoginPhase = 'waiting' | 'scanned' | 'succeeded' | 'failed'

/** 一次登录会话的结论。**里面没有任何凭据**，成功只给掩码形状的 accountId。 */
export interface LoginSession {
  phase: LoginPhase
  accountId: string | null
  message: string
  /** 还能再领几次验证码；null = 上游没给这个配额。只在短信那条路有值。 */
  remainGetTimes: number | null
  solvedSlider: boolean
  /** 需要人工过滑块时的兜底地址；空串 = 不需要。 */
  captchaUrl: string
  /** 短信那条路由发码响应带回的会话 id。 */
  sessionId?: string
}

/** 出码（扫码那条路）。成功后立刻有一条后台轮询在跑，所以它是"真的产生了资源"。 */
export const startQrLogin = (platform: '' | 'windows' | 'macos' = '') =>
  request<{ sessionId: string; qrUrl: string }>('/api/login/qr', {
    method: 'POST',
    body: JSON.stringify({ platform }),
  })

/** 那张码的 SVG 地址。给 `<img src>` 用，后端已经打了 `no-store`。 */
export const qrImageHref = (sessionId: string) =>
  `/api/login/qr/${encodeURIComponent(sessionId)}/image.svg`

/** 问一次会话结论（扫码与短信共用这个出口）。 */
export const loginSession = (sessionId: string) =>
  request<LoginSession>(`/api/login/qr/${encodeURIComponent(sessionId)}`)

/**
 * 发一次 App 端短信验证码。
 *
 * ⚠ `phone` 是**完整**手机号（掩码号发不出短信）：它是入参，
 * 不进 URL、不进日志、不出现在任何响应里 —— 界面上那句"已发到 199****2716"
 * 由响应带回的 `accountId`（掩码形状）渲染。
 */
export const sendAppLoginCode = (phone: string) =>
  request<LoginSession>('/api/login/sms/app', {
    method: 'POST',
    body: JSON.stringify({ phone }),
  })

/** 用验证码换登录。重复提交是幂等的（后端挡住第二次落库）。 */
export const verifyAppLoginCode = (sessionId: string, code: string) =>
  request<LoginSession>(`/api/login/sms/app/${encodeURIComponent(sessionId)}/verify`, {
    method: 'POST',
    body: JSON.stringify({ code }),
  })

/**
 * 发一次 PC 端短信验证码。
 *
 * 与 App 那条的差别都在后端：PC 端要 `platform`（Windows / macOS 两份端档案），
 * 滑块由后端自动解，所以这一支**不会**带回 `captchaUrl`。
 * 入参口径同上：完整手机号只进不出。
 */
export const sendPcLoginCode = (phone: string, platform: '' | 'windows' | 'macos' = '') =>
  request<LoginSession>('/api/login/sms/pc', {
    method: 'POST',
    body: JSON.stringify({ phone, platform }),
  })

/** PC 端填码。幂等口径与 [verifyAppLoginCode] 同一条。 */
export const verifyPcLoginCode = (sessionId: string, code: string) =>
  request<LoginSession>(`/api/login/sms/pc/${encodeURIComponent(sessionId)}/verify`, {
    method: 'POST',
    body: JSON.stringify({ code }),
  })

// ─────────────────────────────────────────────────────────────────────────────
// 执行面：触发、订阅、历史
//
// 形状的三个权威分别是后端 `runs.go`（受理）、`core.RunEvent.MarshalJSON`
// （事件）、`routes.go` 的 `runPayload`（历史行）。这边**不推断**任何一位：
// 例如 `status` 是存储层生命周期、业务结论只在 `finished` 事件里 ——
// 在历史那一档补一个假的"成功/失败"就是替后端编造它没给过的结论。
// ─────────────────────────────────────────────────────────────────────────────

/** 一条历史运行（对位 `runPayload`）。 */
export interface RunRecord {
  id: string
  /** 掩码账号 id；批量编排那种没有归属账号的是 null。 */
  accountId: string | null
  capability: string
  /** `running` / `done` / `failed` —— **生命周期**，不是业务结论（见上）。 */
  status: string
  startedAt: string
  finishedAt: string | null
  cloudGained: number | null
  cloudAfter: number | null
  /** 那一趟的结论一句话（由任务自己写）。后端给的是 `*string` ⇒ 可能是 null。 */
  message: string | null
}

export const listRuns = (limit = 50) =>
  request<{ limit: number; runs: RunRecord[] }>(`/api/runs?limit=${limit}`)

/** `POST /api/runs` 的受理响应：202 **先于**结果，配对就是库里那一条。 */
export interface RunAccepted {
  capability: string
  runs: { accountId: string; runId: string }[]
}

/**
 * 起跑一趟。参数按**能力声明**的 key 送，多一个键后端就 400（`不认识的参数：…`）。
 *
 * ⚠ 这条**不是**幂等的：`live-room` 会把口令投出去。调用方必须挡住连点，
 * 并且在确认弹窗里把影响面说清楚（§6.4）。
 */
export const triggerRun = (capability: string, params: Record<string, unknown>) =>
  request<RunAccepted>('/api/runs', {
    method: 'POST',
    body: JSON.stringify({ capability, params }),
  })

/**
 * 事件流地址。给 `EventSource` 用 —— 它自己带 cookie（同源），
 * 重连时带 `Last-Event-ID`，所以这里不手工拼 `?after=`。
 */
export const runEventsUrl = (runId: string) => `/api/runs/${encodeURIComponent(runId)}/events`

// ─────────────────────────────────────────────────────────────────────────────
// 直播小红花：只读探针
// ─────────────────────────────────────────────────────────────────────────────

/** 一个直播间（对位 `liveRoomProbePayload` 的 `rooms[]`）。 */
export interface LiveRoomRow {
  roomId: string
  title: string
  /** 上游原始数字码。 */
  status: number
  /** 稳定英文码：`not-started` / `live` / `ended` / `unknown`。中文由界面翻。 */
  statusToken: string
  /** 预告开播时间，上游给的**本地**字符串（`2026-09-23 14:30:00`）；非预告为空串。 */
  expectStartTime: string
  anchor: string
}

/** 小红花余额；null = 这次没读到（配 `balanceError` 一起看）。 */
export interface RedFlowerBalance {
  flowerCnt: number
  /** 中文串（`2026年12月31日`），**不要**拿去 `Date.parse`。 */
  validDate: string
  hasNotReceived: number
  quarterDrawn: number
  prizeId: number
  secondPrizeId: number
}

/** `GET /api/accounts/{id}/live-room` 的结论。**里面没有任何凭据**。 */
export interface LiveRoomProbe {
  accountId: string
  /** 门：那个主播现在有没有在播的房间（= 现在去听有没有口令）。 */
  koulingLive: boolean
  /** **所有**在播房间数（不限主播）。与 `koulingLive` 一起才分得开两种"没有"。 */
  liveCount: number
  /** 只列那个主播的房间，**含预告场**（预告的时间就是"下一场几点"）。 */
  rooms: LiveRoomRow[]
  /** 后端自述要听的那个主播昵称。 */
  anchor: string
  balance: RedFlowerBalance | null
  balanceError: string
}

export const probeLiveRoom = (id: string) =>
  request<LiveRoomProbe>(`/api/accounts/${encodeURIComponent(id)}/live-room`)

// ─────────────────────────────────────────────────────────────────────────────
// 兑换选品（只读）
//
// 权威形状在 `internal/server/exchange_prizes.go` 的 `exchangePrizesPayload`。
// 两条与直觉相反的口径，界面必须照它说话：
//
//   - **`plan` / `reason` 来自后端**，不是前端算的：`reason` 是判据的中文原文，
//     全仓库只有一处（`internal/app/exchange_plan.go` 的 `Reason()`）。
//     前端**不再抄第二份话术表**，只留一张「`plan` → 颜色」的映射。
//   - **不含余额判断**：后端用的是 `PlanExchangeWithoutBalance`，所以 `canOrder`
//     比下单前的真判据**宽** —— 余额不够要到了那一步才会被挡。界面上不能把它
//     念成"保证能兑"。
//
// 五格 `number | null`：上游对同一语义有时给串、有时不给键（`internal/upstream/app_exchange.go`
// 的注释里那条"缺了不等于 0"），null 一律显示成 `—`，**不折成 0**。
// ─────────────────────────────────────────────────────────────────────────────

/** 一件奖品（对位 `prizes[]`）。 */
export interface ExchangePrizeRow {
  /** 下单时回填 `prizeIdOrName` 用它：纯数字在后端按 prizeId 定位。 */
  prizeId: number
  /** 分组键，**字符串**形态（上游顶层键就是串）；后端认不出所属组时为空串。 */
  groupId: string
  name: string | null
  /** 所需云朵数。上游同一份响应里有 `pOrder` / `POrder` 两种拼法，后端已合并。 */
  price: number | null
  /** 上游那五档状态原值；null = 上游没给这一键。 */
  limit: number | null
  /** 稳定英文码（后端 `ExchangePlan.Slug()`），十七档 + 兜底 `unknown`。 */
  plan: string
  /** 判据原文（中文），后端唯一一处。 */
  reason: string
  /** 只有 `plan === 'go'` 才为真；⚠ 不含余额那一挡，见上面那段。 */
  canOrder: boolean
  count: number | null
  totalCount: number | null
  dailyRemainderCount: number | null
  minRemainderCount: number | null
  onLine: number | null
  /** 上游原样串（形态只有一档实拍）；解不出来时后端不挡，界面也只照登。 */
  startTime: string | null
  endTime: string | null
}

/** `GET /api/accounts/{id}/exchange-prizes` 的结论。**里面没有任何凭据**。 */
export interface ExchangePrizeList {
  accountId: string
  /** 后端出这份读数的时刻（RFC3339 UTC）。清单是**时刻量**，界面要显示它是何时读的。 */
  readAt: string
  /** 分组键，已按**数值**升序（`2` 在 `10` 前，不是字典序）。 */
  groups: string[]
  total: number
  /** 摊平后的全部件，顺序与 `groups` 一致。 */
  prizes: ExchangePrizeRow[]
}

export const listExchangePrizes = (id: string) =>
  request<ExchangePrizeList>(`/api/accounts/${encodeURIComponent(id)}/exchange-prizes`)

// ─────────────────────────────────────────────────────────────────────────────
// 定时任务 + 能力表
//
// 权威是后端那两处：`routes.go` 的 `schedulePayload`（对外形状）与 `scheduleBody`
// （收的形状）。这一族有一个**不对称**必须记住：
//
//   - 读出来是 `accountIds`（camelCase，跟其余展示字段一起手工拼的），
//   - 写回去是 `account_ids`（snake_case，Go struct tag 直译）。
//
// 两边都不是笔误，也不有一处兜底：把提交的键写成 `accountIds`，
// Go 那边解不进 `AccountIDs` ⇒ 请求**照样 200**，只是把那一行的账号列表洗成空
// （空 = "跑全部账号"），于是一条只管两个号的任务变成打所有号。
// ─────────────────────────────────────────────────────────────────────────────

/** 执行目标：`kind` 决定看 `code` 还是 `id`（对位 `core.ScheduleTarget` 的两个分支）。 */
export interface ScheduleTarget {
  /** `capability` / `pipeline`。 */
  kind: string
  code?: string
  id?: string
}

/** 一条定时任务（对位 `schedulePayload` 的那 15 个键）。 */
export interface ScheduleRow {
  id: string
  name: string
  target: ScheduleTarget | null
  cron: string
  timezone: string
  /** ⚠ 读侧 camelCase；写侧是 `account_ids`。见上面那段。 */
  accountIds: string[]
  /** 参数对象；库里可能是 null。 */
  body: unknown
  enabled: boolean
  /**
   * 下次触发（rfc3339），**由服务端按 cron + 时区算**。
   * null 有两种来路：这一行停用了、或那条 cron 永远不触发 —— 界面上都写"不会触发"，
   * 但**不**把"停用"这一种说成"配置错了"（那是同一格子的两件事）。
   */
  nextFireAt: string | null
  /** 调度循环写的水位："已处理过哪一个触发点"。 */
  lastFireAt: string | null
  /** 这一行**自己**选过的补跑策略；null = 跟随能力声明的默认（默认值不在这里说）。 */
  missPolicy: string | null
  createdAt: string | null
  lastRunAt: string | null
  /** `ok` / `error` / `skipped`；null = 从没跑过。 */
  lastStatus: string | null
  lastError: string | null
}

export const listSchedules = () => request<{ schedules: ScheduleRow[] }>('/api/schedules')

/**
 * 保存时的请求体（对位 `scheduleBody`）。
 *
 * id 在**路径**里：同一条路径重复提交是同一个结果，所以这条是 `PUT` 而不是 `POST`，
 * 而后端刻意不发明 id —— 那样"改一条"与"建一条"就分不出来了。
 */
export interface ScheduleDraft {
  name: string
  target: ScheduleTarget
  cron: string
  /** 留空即后端默认（`Asia/Shanghai`）。 */
  timezone?: string
  /** 空数组 = 跑全部账号。 */
  account_ids: string[]
  /**
   * 参数。后端解进 `any`，所以形状上什么都收；**语义上只认对象** ——
   * 到点时 `scheduleParams` 解不进对象就记一行 `error`（"这条定时任务的参数不是一个 JSON 对象"）。
   * 新建/编辑那条路由界面挡住（必须是对象）；行内翻启用那一条**原样带回库里那份**，
   * 因为旧导入进来的非对象参数是既成事实，不该被一次翻开关洗掉。
   */
  body: unknown
  enabled: boolean
}

export const putSchedule = (id: string, draft: ScheduleDraft) =>
  request<{ schedule: ScheduleRow }>(`/api/schedules/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(draft),
  })

/**
 * 删一条任务。连带范围问过存储层：`DeleteSchedule` 只动 `schedules` 那一行，
 * 执行历史（`runs`）与账号都不跟着走 —— 界面上那句影响面照这个说。
 */
export const deleteSchedule = (id: string) =>
  request<void>(`/api/schedules/${encodeURIComponent(id)}`, { method: 'DELETE' })

/** 一个参数声明（对位 `core.FieldSpec`）。 */
export interface CapabilityField {
  key: string
  label: string
  /** `text` / `number` / `bool` / `select` / `multi_select` / `json`。 */
  kind: string
  required: boolean
  default?: unknown
  /**
   * ⚠ `value` 是 `json.RawMessage`（后端注释写明："保留**字面字节**，前端靠它回传"）——
   * 也就是它在线格式上是一个 **JSON 值**而不是字符串：`select` 那一项回的是
   * `{label:"串行", value:"sequential"}`，那个 `value` 解析后带引号。
   * 拿它当 `string` 直接送进 `<option value>` 会看到界面上一对引号。
   * 本页只用 `key` 做参数键名校验，没碰这一栏。
   */
  options?: { label: string; value: unknown }[]
  help?: string | null
}

/** 一个能力（对位 `app.CapabilityPayload`；⚠ 这一族的键是 camelCase）。 */
export interface CapabilityRow {
  code: string
  title: string
  group: string
  kind: string
  description: string
  /** true = 任务中心不给选，但**仍是合法的调度目标**（`auth-refresh` 就是这一档）。 */
  hidden: boolean
  runInBatch: boolean
  risk: string
  needsConfirmation: boolean
  defaultAccountMode: string
  params: CapabilityField[]
  output: CapabilityField[]
}

export interface CapabilitiesPayload {
  all: CapabilityRow[]
  visible: string[]
  batchable: string[]
  groups: string[]
}

/** 拉能力表。定时任务页要它做两件事：目标下拉的选项、参数 JSON 的合法键名。 */
export const listCapabilities = () => request<CapabilitiesPayload>('/api/capabilities')

// ─────────────────────────────────────────────────────────────────────────────
// 推送（通知）
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 一格的**档位**与**值类型**（对位后端 `notifyFieldPayload`）。
 *
 * 两者都要：`kind` 说"值给不给看"（`secret` 只出"填了没 + 几个字符"、`keys` 只出键名、
 * `plain` 原样出值），`type` 说"值长什么样"（`plain` 那一档背后是文本 / 数字 / 开关三种控件）。
 * 只看"有没有值"分不出后两者：`port` 是 `*int`，没填时 JSON 里根本没有这一格。
 */
export type NotifyFieldKind = 'secret' | 'keys' | 'plain'
export type NotifyValueType = 'string' | 'number' | 'boolean' | 'object'

/**
 * 视图里的一个字段：**数组**里的一项，自带 `name`。
 *
 * 顺序与格数由后端那张字段表决定（`internal/server/notify_view.go`），前端**不抄表** ——
 * 抄了会漂移，而两种漂移都不报错：后端新增一格界面画不出来，前端多写一格 PUT 被 400 拦下。
 *
 * ⚠ `secret` 那一支**没有值**，只有"填了没 + 几个字符"：这是整族接口的核心口径 ——
 * 界面上任何一格都拿不到已存的凭据原文，所以提交时**留空 = 不改**（见 [putNotifyConfig]）。
 */
export interface NotifyFieldView {
  name: string
  kind: NotifyFieldKind
  type: NotifyValueType
  /** `kind === 'secret'`：填了没（按去掉首尾空白判）。 */
  present?: boolean
  /** `kind === 'secret'`：几个字符（0 = 没填）。 */
  length?: number
  /** `kind === 'keys'`：已有几个键。 */
  count?: number
  /** `kind === 'keys'`：键名（**值不出网**）。 */
  keys?: string[]
  /** `kind === 'plain'`：原值。未填的指针格是 null。 */
  value?: unknown
}

/**
 * 一家渠道的视图。
 *
 * `fields` 对**没配**的那几家同样是整张清单（后端按零值出，于是空白表单画得出来）。
 * `customPost` 是数组型：`fields` 是共用的清单，值在 `items[].fields` 里逐条给。
 */
export interface NotifyChannelView {
  configured: boolean
  /** 缺哪个必填项（人话，如 `token`）；空串 = 不缺或压根没配。 */
  missing: string
  fields: NotifyFieldView[]
  /** 只有 `customPost` 有：现有几条。 */
  count?: number
  items?: { fields: NotifyFieldView[] }[]
}

/** 整份推送配置的视图（对位 `notifyPayloadOf` 那张表）。 */
export interface NotifyConfigView {
  enabled: boolean
  /** null = 没自定义标题（用 `defaultTitle`）。 */
  title: string | null
  onlyError: boolean
  /** ⚠ 空串是**有效值**（"没填 ⇒ 跟随 info"），原样回读，界面不许替它填。 */
  minLevel: string
  levels: string[]
  defaultTitle: string
  updatedAt: string | null
  /** 配了的（可能缺必填）。 */
  configured: string[]
  /** 必填齐、真发得到的那几家。 */
  sendable: string[]
  /** 十家的**渲染序**（= 后端发送序 `core.NotifyChannelOrder()`）。 */
  order: string[]
  channels: Record<string, NotifyChannelView>
}

export const getNotifyConfig = () =>
  request<{ config: NotifyConfigView }>('/api/notify/config')

/**
 * 保存推送配置。
 *
 * 三条写侧语义必须照做，否则会**静默洗掉用户配好的东西**（这是本族最贵的失效）：
 *  1. **逐字段合并**：这一格不出现在体里 ⇒ 保持库里那一份。所以表单**只提交改动过的格**，
 *     而不是把整份视图回传（视图本来就是脱敏的，回传也带不回凭据）。
 *  2. 要清空**一整家** ⇒ 把它的键置 `null`（清空单个字段做不到，见 `docs/pending-decisions.md` Q30）。
 *  3. 键名**必须逐字对后端**：不认识的键是 400（顶层、渠道名、字段名三处都拦），
 *     字段名是 snake_case 的 json tag（`chat_id` / `corpsecret` / `api_host`…），**不是**视图里
 *     那层 camelCase 的渠道名以外的东西。
 */
export type NotifyConfigPatch = {
  enabled?: boolean
  title?: string | null
  onlyError?: boolean
  minLevel?: string | null
  channels?: Record<string, unknown> | null
}

export const putNotifyConfig = (patch: NotifyConfigPatch) =>
  request<{ config: NotifyConfigView }>('/api/notify/config', {
    method: 'PUT',
    body: JSON.stringify(patch),
  })

/**
 * 去掉**一整家**渠道的配置。
 *
 * 单列一个函数是因为这是本族唯一一处"发 `null`"的地方，而它和"不发这一格"只差一个键：
 * 界面把它包成一个动词（删除），比让调用点手写 `{channels: {bark: null}}` 更难用错 ——
 * 用错的后果不是报错，是**静默清掉了那一家的凭据**。清空**单个**字段没有对应接口
 * （后端分不清"忘了填"与"要清空"，见 `docs/pending-decisions.md` Q30）。
 */
export const deleteNotifyChannel = (name: string) =>
  putNotifyConfig({ channels: { [name]: null } })

/** 一家渠道的试发结论（`reason` 由后端保证不含 URL 与凭据）。 */
export interface NotifyTestResult {
  channel: string
  /** `sent` / `failed` / `skipped`。 */
  outcome: string
  reason: string
}

export interface NotifyTestReport {
  title: string
  body: string
  results: NotifyTestResult[]
  summary: string
  /** 这一条**没有**经过的那些闸：试发成功不等于自动推会成功。 */
  gates: { enabled: boolean; onlyError: boolean; minLevel: string }
}

/**
 * 试发一条。`channels` 是**收窄**不是扩面：点了没配的渠道后端 400，
 * 前端也就不该给出这种选项（界面只在"已配的那几家"里挑）。
 */
export const runNotifyTest = (body: {
  title?: string
  message?: string
  channels?: string[]
}) =>
  request<NotifyTestReport>('/api/notify/test', {
    method: 'POST',
    body: JSON.stringify(body),
  })
