/**
 * 后端客户端。
 *
 * 所有网络访问集中在这里，页面不直接 `fetch` —— 便于统一错误形态与类型。
 * 类型目前手写；接口冻结后换成 openapi 生成（与 mcloud 的未决项 2 同一处置）。
 *
 * 开发期走 Vite 代理（同源），生产期后端直接发前端产物，也是同源 ——
 * 刻意不用 CORS，避免"开发能用、打包上去就不行"。
 */

import { classifyAuthFailure, noteAuthOk, noteAuthRequired } from './ui/auth-signal'
import type { LogKind } from './ui/log-panel'
// 转出给页面用：页面只 import `api` 就够了，不必知道状态存在哪个模块里
export type { AuthState } from './ui/auth-signal'
export { currentUser } from './ui/auth-signal'

/** 任务分组键（界面分栏用，不影响执行）。 */
export type TaskGroup = 'signin' | 'live' | 'app' | 'device'

/**
 * 一个参数的说明 —— 前端据此画"任务配置"表单。
 *
 * ⚠ 从前 `/api/capabilities` 只给**参数名**（`string[]`），类型/默认值全靠前端猜；
 * 猜错的表现是"填了没用"。现在类型、默认值、取值范围都由后端给。
 */
export interface TaskParamSpec {
  name: string
  /** 界面上的标签。 */
  title: string
  kind: 'bool' | 'number' | 'text'
  /** 默认值（JSON）。用于"从没配过"时的预填。 */
  default: unknown
  min?: number
  max?: number
  unit?: string
  help?: string
}

export interface TaskMeta {
  name: string
  title: string
  description: string
  hidden: boolean
  /** 分组键。 */
  group: TaskGroup
  /**
   * 分组的中文名 —— **后端给**。
   *
   * 本仓栽过一次：任务页曾把签到族那 8 个任务名硬编码在前端当分组依据，
   * 而后端的注册表才是权威。前端每多一份这类"键→名字"的表，就多一处迟早漂移的地方。
   */
  groupLabel: string
  params: TaskParamSpec[]
}

/** 任务结果。`skipped` 不是失败 —— 活动未开启或条件不满足。 */
export type TaskOutcome =
  | { status: 'success'; summary: string }
  | { status: 'skipped'; reason: string }
  /**
   * **部分成功**：有成的、也有没成的（后端的 `TaskOutcome::Partial`）。
   *
   * ⚠ 它与 success / failed **都不是一回事**：写"成功"会把失败盖住 ✗
   * （2026-10-03 用户实测：「下面的 614 失败还直接说成功」），写"失败"又把
   * "其实领到了几笔"说成全败 ✗。文案走 `summary`（两边都在里面）。
   */
  | { status: 'partial'; summary: string }
  | { status: 'failed'; reason: string }

/**
 * 一次运行的**生命周期**状态（后端 `mcloud_core::RunStatus`，serde 小写）。
 *
 * ⚠ 它和 `TaskOutcome` **不是一回事**，别合并（`crates/mcloud-core/src/run.rs`
 * 的文件头写着这条分工）：
 * - `status` 说"这次运行结束了没有"（`running` / `done` / `cancelled`）；
 * - `outcome` 说"业务上成没成"（success / skipped / partial / failed）。
 *
 * 混起来会出事的地方正是 `cancelled`：用户自己点的「中止」在业务结论上是 `failed`
 * （`runner.rs` 把 `Error::Cancelled` 收成 `TaskOutcome::failed`），
 * 而后端**明确**说它不该报红、不该计入错误率（`RunRecord::needs_attention`
 * 把 `Cancelled` 排除在外）⇒ 界面必须先看 `status`，别直接拿 `outcome.status` 染红 ✗。
 */
export type RunStatus = 'running' | 'done' | 'cancelled'

/**
 * 运行日志的一行（**回看**用，来自 `RunRecord.logs`）。
 *
 * ⚠ 与实时面板的 `LogLine` **不是**同一个类型，别合并：这里 `level` 是后端那四个
 * 取值（`debug/info/warn/error` ✓），而面板只认三档 ✓ ⇒ 渲染时把 `debug` 并进
 * `info` ✓ —— 回看里"调试行"与"普通行"同色是对的 ✓。
 */
export interface RunLogLine {
  level: 'debug' | 'info' | 'warn' | 'error'
  /** 分类（界面按它上徽章与颜色 ✓）。老记录没有这一格 ⇒ 当 `info` 看 ✓。 */
  kind?: LogKind
  message: string
  at_ms: number
}

/** 一条留在用户网盘上、还没清理的产物。 */
export interface LeftoverArtifact {
  /** 产物类型。今天是 `cloud-file`，刻意开放（将来会有外链、笔记等）。 */
  kind: string
  /** 账号（掩码标识，**不是**凭据）。 */
  account: string
  /** 上游侧的 `fileId` —— 手工清理时要用它。 */
  id: string
}

export interface RunRecord {
  run_id: number
  task: string
  account: string
  started_at_ms: number
  duration_ms: number
  outcome: TaskOutcome
  /**
   * 生命周期状态。与 `outcome` 的分工见 [`RunStatus`]。
   *
   * ⚠ 写成可选是给**老记录**留的：这一格是后加的，后端读回来时 `#[serde(default)]`
   * 回填成 `done` ✓；界面侧一律走 `ui/history.ts` 的 `runStatusOf` 判它，
   * **别**在各页自己写 `r.status === 'cancelled'` 的判空（漏一处就是一处"中止显示成失败"✗）。
   */
  status?: RunStatus
  /**
   * 这次运行的日志行（**回看**用；最多 200 行 ✓）。
   *
   * ⚠ **列表接口不带它** —— `/api/runs` 会剥掉 ✗（100 条各带 200 行就不是列表
   * 该有的体量了 ✓，见 `routes/runs.rs::list`）⇒ 这里必须是可选的 ✓；
   * 要日志就按 id 取详情（`getRun(run_id)` ✓，它原样带 ✓）。
   */
  logs?: RunLogLine[]
  /**
   * 结构化明细。
   *
   * 今天只有一格：`artifacts` —— 这次运行**留在用户网盘上、还没清理**的文件。
   * 非空时后端还会发一条 warn 日志。
   *
   * ⚠ 它是"跑完那一刻还没清"的快照，不是"最终没清"：
   * 链尾清理干净的任务这里就是 `null`。
   */
  details?:
    | {
        artifacts?: LeftoverArtifact[]
        /**
         * 直播那一次的**结构化明细**（后端落地后才有）。
         *
         * ⚠ 它可能在**老记录里不存在** —— 那时 `outcome.summary` 里只有一句话。
         * 历史页必须能区分这两种情况：没有它就别渲染"领到 0 个"的假明细 ✗
         * （那比不显示更糟：用户会以为真的一个都没领到）。
         */
        liveRoom?: LiveRoomReport
        /**
         * 兑换那一次的**结构化明细**（后端落地后才有）。
         *
         * ⚠ 它是**扁平**的（不像 `liveRoom` 那样套一层）：后端 `detail_payload`
         * 直接把这几个键铺在 `details` 上，并用 `kind: "exchange"` 标识自己。
         * 判据要**同时**看 `task === 'exchange'` 与 `kind === 'exchange'` ——
         * 老记录里两者都可能不在（那时只有一句文本小结）。
         */
        kind?: string
        status?: string
        plan?: string
        plan_reason?: string
        code?: number | null
        message?: string
        rounds?: number
        attempts?: number
        offset?: number | null
        balance?: number | null
        after_balance?: number | null
        /** `skipWait=false` 那条路上"等到了什么时候"（unix 毫秒）。 */
        waited_until_ms?: number | null
        device_id_failed?: string
        error?: string
        prize?: { prize_id: number; name: string | null; price: number }
        order?: {
          order_id: string
          prize_id: number
          prize_name: string | null
          cost: number
          insert_time: number
          expire_time: number
        } | null
      }
    | null
}

export interface RunResponse {
  /** 兼容字段：多账号时是**第一个**账号那次运行的 id。 */
  run_id: number
  /** 本次点击涉及的**全部** run id（单账号时就是 `[run_id]`）。 */
  run_ids: number[]
  task: string
  /** 兼容字段：多账号时是**第一个**账号。 */
  account: string
  /** 本次点击涉及的**全部**账号（顺序 = 后端的执行顺序，串行）。 */
  accounts: string[]
  outcome: TaskOutcome
  /** 同步执行时是全部账号的合计耗时。 */
  duration_ms: number
}

/** 运行事件，与后端 `RunEvent` 的 `type` 标签一一对应。 */
export type RunEvent =
  | { type: 'started'; run_id: number; task: string; account: string; at_ms: number }
  | {
      type: 'log'
      run_id: number
      level: 'debug' | 'info' | 'warn' | 'error'
      /**
       * 分类 ✓（`info/success/fail/warn/error/start/debug`）。
       *
       * ⚠ 可选：老后端 / 老记录没有这一格 ⇒ 渲染时按 `level` 兜底 ✓
       * （兜底只能分出三档，会丢掉"成功/失败"的区别 ✓）。
       */
      kind?: LogKind
      message: string
      at_ms: number
    }
  | { type: 'progress'; run_id: number; step: number; total: number; label: string }
  | { type: 'finished'; run_id: number; outcome: TaskOutcome; duration_ms: number }

/** 后端返回的错误体。 */
export interface ApiErrorBody {
  error: string
  /**
   * 可判别的机器码，例如 `setup_required`。
   *
   * ⚠ 与 `error` 分工明确：`error` 是**给人看的**（可以随时改字），
   * 这个字段是给**代码判断**的。让代码去匹配 `error` 的文案，
   * 后端改一个字就会让判断静默失效。
   */
  code?: string
}

/**
 * 带 HTTP 状态与后端原文的 API 错误。
 *
 * 刻意**保留后端原文**而不是统一成「操作失败」：上游的提示往往已经说清了
 * 该看哪里，翻译成通用文案等于把信息丢掉。
 */
export class ApiError extends Error {
  readonly status: number
  /** 后端的机器码（如 `setup_required`），没有则 `undefined`。 */
  readonly code: string | undefined
  constructor(status: number, message: string, code?: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

/**
 * 底层请求：统一处理 JSON 解析、错误原文透出与 204。
 *
 * 导出是为了让页面复用同一份语义 —— 复制一份到页面里迟早会分叉。
 */
/**
 * 这次失败是不是"认证动作**自己**的正常失败"（而非"整个应用该跳登录页"）。
 *
 * ⚠ 必须**按方法区分**，不能只看路径：
 *
 * | 请求 | 401 的含义 | 该不该跳登录页 |
 * |---|---|---|
 * | `POST /api/session`（登录） | 用户名或口令错 | **否** —— 用户就在登录页上 |
 * | `POST /api/setup`（建管理员） | 已初始化/校验失败 | **否** —— 同理 |
 * | `GET /api/session`（查会话） | 没登录 | **是** —— 这正是启动时的判据 |
 *
 * 只看路径的话，启动时那句 `GET /api/session` 的 401 会被吞掉，
 * 于是应用永远停在"正在检查"上。
 *
 * 不排除 POST 的话，口令输错一次就会让登录页自己重渲染、把表单清掉。
 */
function isAuthActionFailure(method: string, path: string): boolean {
  const m = method.toUpperCase()
  if (path === '/api/session') return m === 'POST' || m === 'DELETE'
  if (path === '/api/setup') return m === 'POST'
  return false
}

/**
 * 请求超时（毫秒）。
 *
 * ⚠ **为什么必须有。** 页面的 `busy` / `submitting` / `saving` 都是在
 * `try/finally` 里复位的，而 `fetch` **默认没有超时**：请求悬挂时 `finally`
 * 永远不跑 ⇒ 按钮永远是禁用态、界面看着"卡死"，用户只能刷页。
 *
 * 取 30 秒：上游偶尔确实慢（网盘列表、直播），要留够；但也不能无限等。
 * SSE 不走这里 —— 它有自己的长连接与重连。
 */
const REQUEST_TIMEOUT_MS = 30_000

export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response
  try {
    response = await fetch(path, {
      credentials: 'same-origin',
      headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
      ...init,
      // 调用方自己传了 signal 就尊重它（那种情况由调用方负责超时）。
      signal: init?.signal ?? AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    })
  } catch (e) {
    // fetch 超时只抛一个 DOMException（message 是英文的 "The operation was aborted"），
    // 直接冒到界面等于没说清发生了什么，这里换成能读懂的一句。
    if (e instanceof DOMException && (e.name === 'TimeoutError' || e.name === 'AbortError')) {
      throw new ApiError(
        0,
        e.name === 'TimeoutError'
          ? `请求超时（${REQUEST_TIMEOUT_MS / 1000} 秒没有响应），请重试`
          : '请求已取消',
      )
    }
    throw e
  }

  const text = await response.text()
  let data: unknown = null
  if (text) {
    try {
      data = JSON.parse(text)
    } catch {
      // 后端异常时可能返回非 JSON（例如反向代理的 HTML 错误页），
      // 保留原文片段便于排查，而不是抛一句无信息的解析错误
      if (!response.ok) {
        throw new ApiError(response.status, `响应不是 JSON（HTTP ${response.status}）：${text.slice(0, 200)}`)
      }
      throw new ApiError(response.status, `响应不是 JSON：${text.slice(0, 200)}`)
    }
  }

  if (!response.ok) {
    const body = data && typeof data === 'object' ? (data as ApiErrorBody) : null
    const message = body && 'error' in body ? String(body.error) : `HTTP ${response.status}`
    const code = body && typeof body.code === 'string' ? body.code : undefined

    // ⭐ 认证失败要在**这一层**统一识别。
    //
    // 放在这里而不是各页面：每个页面都要处理的话，漏一个就是那一个页面
    // 永远卡在错误态；而"漏了"没有任何测试会红。
    //
    // ⚠ 登录接口自己**排除在外**：它返回 401 表示"用户名或口令错误"，
    // 那是这个动作的正常失败，不是"整个应用要跳登录页"——
    // 而用户此刻**就在**登录页上。
    const method = (init?.method ?? 'GET').toUpperCase()
    if (!isAuthActionFailure(method, path)) {
      const authFailure = classifyAuthFailure(response.status, code)
      if (authFailure) noteAuthRequired(authFailure)
    }

    throw new ApiError(response.status, message, code)
  }

  if (response.status === 204) return undefined as T
  return data as T
}

// ── 认证（WebUI 登录闸）──────────────────────────────────────────────
//
// ⚠ 这一层与下面的 `/api/login/*`（云盘账号登录）是**两件事**：
// 这里管"谁能进这个 WebUI"，那边管"往账号列表里加一个云盘账号"。

export interface SetupStatus {
  initialized: boolean
  /**
   * 口令长度下限。**后端给**，前端不硬编 ——
   * 硬编一份必然与后端漂移，而漂移的表现是"前端说可以、后端拒绝"。
   */
  minPasswordLen: number
  /**
   * 登录时该发什么凭据（后端自述，用户口径 2026-10-01 的"前端先 hash 一次"）：
   *
   * - `sha256` —— 发 `SHA-256(明文)` 的 hex（新档，目标形态）
   * - `plain`  —— 发**明文**（库里还是老档：argon2 直接吃明文；后端验过会当场升级）
   *
   * ⚠ 声明成可选：**旧后端不带这个字段**。前端在缺失时按 `plain` 走 ——
   * 那是对"新老两种后端"都成立的那条路（新后端收到明文会自己补一次摘要）。
   */
  passwordScheme?: PasswordScheme
}

/** 口令凭据档位，与后端 `SetupStatus.password_scheme` 同义。 */
export type PasswordScheme = 'sha256' | 'plain'

export interface AuthSession {
  userId: string
  name: string
  kind: 'web' | 'cli'
  /** 是不是**实例管理员**（= 创建最早的那个用户 ✓）—— 见 `ui/auth-signal.ts`。 */
  isAdmin: boolean
}

export const getSetupStatus = () => request<SetupStatus>('/api/setup')

/**
 * 首次建管理员。成功后后端会**直接下发会话 cookie**，无需再登录一次。
 *
 * 这里显式置 `ready` —— 而不是在 `request` 里"任何成功都置 ready"。
 * 理由是公开接口（`/api/version`、`/api/health`）**未初始化时也返回 200**，
 * 那样会让应用从"要建管理员"错误地翻成"放行"，然后每个页面各自
 * 撞一鼻子 503。放行只能由**明确的认证动作**宣布。
 *
 * ⚠ `credential` 是**口令凭据**，不是明文：调用方先按 `/api/setup` 自述的
 * `passwordScheme` 过一遍 `ui/password.ts` 的 `credentialFor`（见那里的取舍）。
 */
export const createAdmin = async (username: string, credential: string) => {
  const s = await request<AuthSession>('/api/setup', {
    method: 'POST',
    body: JSON.stringify({ username, password: credential }),
  })
  noteAuthOk(s.name, s.isAdmin)
  return s
}

/** 登录。`credential` 同上：凭据，不是明文。 */
/**
 * 账户安全（口令策略 / 改口令）。
 *
 * 与 `/api/setup` 的 `passwordScheme` 同一套档位：库里是老档时这两年
 * 前端要发明文，服务端验过会顺手升级（见 `ui/password.ts`）。
 */
export interface SecurityView {
  /** 口令长度下限（可改）。⚠ 只对**明文**生效：摘要恒 64 位，量不出长度。 */
  minPasswordLen: number
  /** 新建会话的有效期（小时）。改它只影响之后新建的会话。 */
  sessionTtlHours: number
  passwordScheme?: PasswordScheme
}

export const getSecurity = () => request<SecurityView>('/api/admin/security')

/**
 * 改策略。
 *
 * ⚠ **两个字段都要给**（后端 PUT = 整体替换）：少给会 422。
 * 这样"以为改了、其实没改"不可能发生 —— 调用方把当前值一起发回来。
 */
export const putSecurity = (patch: { minPasswordLen: number; sessionTtlHours: number }) =>
  request<SecurityView>('/api/admin/security', {
    method: 'PUT',
    body: JSON.stringify(patch),
  })

/** 改口令的结果：被结束的旧会话数 + 当前用户（后端会顺手换一张新 cookie）。 */
export interface PasswordChanged {
  userId: string
  name: string
  kind: 'web' | 'cli'
  /** 这次改口令顺带结束了多少个**旧**会话（不含马上发的那张新令牌）。 */
  sessionsEnded: number
}

/**
 * 改口令。`current` / `next` 都是**凭据**（不是明文）——
 * 调用方先过 `ui/password.ts` 的 `credentialFor`。
 *
 * ⚠ 当前口令**必填**（后端也必填）：用户口径 2026-10-01
 * ——「你口令更改怎么取消了旧口令」。
 */
export const changePassword = async (current: string, next: string) => {
  const r = await request<PasswordChanged>('/api/admin/password', {
    method: 'PUT',
    body: JSON.stringify({ currentPassword: current, newPassword: next }),
  })
  // 后端换了一张新 cookie ⇒ 会话仍然有效，本地状态跟着刷新一下
  // ⚠ 这一处**不传** isAdmin：`PasswordChanged` 里没有那一格，而且**改口令不会
  // 改变"是不是管理员"**（那由创建时间定 ✓）⇒ 保持信号原值才是对的 ✓。
  noteAuthOk(r.name)
  return r
}

/**
 * 「关于」卡片的数据（`GET /api/system/about`）。
 *
 * ⚠ **没有"最新版本"这一项**：上游仓库是私有的（匿名访问 404），浏览器端没有凭据
 * 去查发布 ⇒ 这里给的是**本次构建实际包含的改动**，那是本机真拿得到的东西。
 */
export interface AboutChange {
  hash: string
  subject: string
}

export interface AboutView {
  name: string
  version: string
  /** 短提交号；构建机上没有 git 时是空串。 */
  commit: string
  /** 构建时工作区有未提交改动（release 会把它们一起编进线上）。 */
  dirty: boolean
  /** 构建时间（本机时区，形如 `2026-10-01 19:20 CST`）。 */
  buildTime: string
  profile: 'release' | 'debug' | string
  /** 本次构建包含的最近若干条改动。 */
  changes: AboutChange[]
}

export const getAbout = () => request<AboutView>('/api/system/about')

/** 一个数据文件的大小。 */
export interface FileSize {
  name: string
  bytes: number
}

/**
 * 一个日志目录的**用量与上限**（运行日志 `data/logs/` 与服务端日志 `data/server-logs/`
 * 共用这个形状 ✓）。
 *
 * ⚠ 2026-10-09 新增（用户口径「日志和历史不要全加入内存，使用本地记录，让用户设置最大大小」✓）：
 * `maxBytes` = 该目录的总量上限（来自用户设的 [`StorageLimits`] 那一格 ✓），
 * `bytes` = 当前占用 ✓。两个数要**一起**给 —— 只报占用的话，用户看不出"还剩多少"✗，
 * 而"该不该调大"正是他来这里要回答的问题 ✓。
 */
export interface LogUsage {
  /** 日志目录里有多少个文件。 */
  files: number
  /** 当前占用（字节）。 */
  bytes: number
  /** 上限（字节）。 */
  maxBytes: number
}

/**
 * 用户可设的**五个**上限 —— 也是 `PUT /api/admin/storage` 的**请求体**。
 *
 * ⚠ 前三个是 2026-10-09 加的（运行历史与运行日志 ✓），后两个是同一天补的
 * **服务端日志**（WebUI「日志」页那份：调度器 / 上游 http / 认证 ✓）——
 * 用户口径「日志和历史不要全加入内存，使用本地记录，让用户设置最大大小」✓。
 *
 * ⚠⚠ 2026-10-09 后端改成**给哪格改哪格**（不是整体替换 ✗）：请求体里出现的格子
 * 才改，一**格都不给**才回 400 ✓。调用方**五格一起发**最省事 ✓ ——
 * 界面上那五个框本来就是"当前值"，一起发回去与"整体替换"等价 ✓，
 * 而"以为改了、其实没改"也不可能发生 ✓。
 *
 * 范围（**后端也卡一遍**，前端先卡一道只是为了不白跑一次往返 ✓）：
 * `historyLimit` 20~2000 / `logLimitPerRun` 200~20000 / `logTotalMb` 1~4096 /
 * `serverLogMemLines` 200~20000 / `serverLogTotalMb` 1~4096。
 */
export interface StorageLimits {
  /** 运行历史留多少条（超了从最旧的开始丢）。 */
  historyLimit: number
  /**
   * **一次运行**最多留多少行日志。
   *
   * ⚠ 它同时是 `GET /api/runs/{id}/logs` 的 `limit` 上限 ✓ ——
   * 也就是说这个数决定"那次运行的日志总共还剩多少行可翻" ✓（不是"每页多少行"✗）。
   */
  logLimitPerRun: number
  /** 运行日志目录（`data/logs/`）总量上限（MB）。 */
  logTotalMb: number
  /**
   * **服务端日志**在内存里留多少行（200~20000，缺省 2000 ✓）。
   *
   * ⚠ 它是**窗口**不是全量：更早的行在磁盘上（`data/server-logs/<日期>.jsonl` ✓），
   * 「日志」页往上翻时按需读 ✓（所以调小它不会让用户"看不到"历史，只是少一层内存缓存 ✓）。
   *
   * ⚠⚠ **可选**只为一件事：这一格是**刚加的**，而后端那条线要重启才生效 ⇒
   * "前端已经有、本地那个后端还没有"会短暂存在 ✓（`StorageView.logUsage` 那一段写过
   * 同一条）。那几分钟里这一格缺席 ⇒ 界面**不铺**这两个框（而不是写一个 `undefined` 进
   * 输入框、再让保存必然 400 ✗，见 `settings-page.tsx::applyLimits` ✓）。
   */
  serverLogMemLines?: number
  /**
   * 服务端日志的**磁盘**总量上限（MB，1~4096）—— 超出按最旧的**整天文件**淘汰 ✓。
   *
   * ⚠ 可选的理由同上（与 `serverLogMemLines` 同一格状态：要么都有、要么都没有 ✓）。
   */
  serverLogTotalMb?: number
}

/** 存储信息（数据目录 / 日志占用 / 二进制 / 容量上限）。 */
export interface StorageView {
  dataDir: string
  logDir: string
  binary: string
  /** 数据文件合计（**不含**日志）。 */
  dataBytes: number
  /** `data/logs` 递归合计。 */
  logBytes: number
  files: FileSize[]
  /**
   * **运行日志**目录的用量与上限（2026-10-09 新增）。
   *
   * ⚠ **可选**：这一格是刚加的，而前端改完 `npm run build` + 刷新就生效、
   * 后端那条线要重启才生效 ⇒ "界面上先有、后端还没有"是常态 ✓。
   * 写成必填会让**整张存储卡**（乃至这一页 `Promise.all` 的整页取数）当场炸 ✗ ——
   * 缺这一格时界面退回只报占用（`logBytes` ✓），**不编**一个上限出来 ✓。
   */
  logUsage?: LogUsage
  /**
   * **服务端日志**（WebUI「日志」页那份）的用量与上限 —— 与 [`StorageView.logUsage`]
   * 同一形状、同一取舍（可选 ✓，缺了只报占用 ✓）。
   *
   * ⚠ 两份别混：`logUsage` 管的是 `data/logs/`（**运行**日志 ✓），
   * 这一格管的是 `data/server-logs/`（整个进程的日志 ✓）—— 两个目录、两个上限 ✓。
   */
  serverLogUsage?: LogUsage
  /** 同上：老后端没有这一格 ⇒ 那一块编辑器整个不铺（见 `settings-page.tsx::storageCard`）✓。 */
  limits?: StorageLimits
}

export const getStorage = () => request<StorageView>('/api/system/storage')

/**
 * 改存储上限（**管理员**，`PUT /api/admin/storage`）。
 *
 * ⚠ 后端是**给哪格改哪格**（2026-10-09 改的；在这之前是"整体替换、少一格 400" ✗）：
 * 调用方把**当前五格**一起发回来最省事 ✓（少发的那格会保持服务端的旧值，
 * 于是"界面上改了、其实没发"这类 bug 会以"保存后又被回填回去"的形式现形 ✓）。
 *
 * 200 时回的是**与 [`getStorage`] 完全相同形状**的那一份（改完的即时状态 ✓）⇒
 * 调用方拿它直接替换本地读数即可 ✓（不必再补一次 GET）。
 *
 * 400 时 `request()` 抛出的 `Error.message` **就是**后端那句人话
 * （例如"服务端日志总量上限要在 1~4096 MB 之间"）—— 原样显示，别自己翻译一遍：
 * 翻译只会把"是哪一格错了"这条线索弄丢 ✗。
 */
export const putStorageLimits = (body: StorageLimits) =>
  request<StorageView>('/api/admin/storage', { method: 'PUT', body: JSON.stringify(body) })

/**
 * 重启服务。
 *
 * 后端回 **202** 后 300ms 才优雅退出，交给 launchd / systemd 重拉 ——
 * 所以这个 Promise 正常返回**不代表服务已经重启完**，调用方要自己轮询 `/api/health`。
 */
export const restartService = () =>
  request<unknown>('/api/system/restart', { method: 'POST', body: '{}' })

export const loginSession = async (username: string, credential: string) => {
  const s = await request<AuthSession>('/api/session', {
    method: 'POST',
    body: JSON.stringify({ username, password: credential }),
  })
  noteAuthOk(s.name, s.isAdmin)
  return s
}

export const logoutSession = () => request<void>('/api/session', { method: 'DELETE' })

/** 查当前会话。401 会经 `request` 让应用转到登录页 —— 这是启动时的判据。 */
export const getCurrentSession = () => request<AuthSession>('/api/session')

// ── 具体接口 ────────────────────────────────────────────────────────────

export interface VersionInfo {
  name: string
  version: string
}

export interface ServiceStatus {
  name: string
  version: string
  /**
   * 服务进程**开始服务**的时刻（epoch 毫秒）—— 「运行时间」= 现在 − 它。
   *
   * ⚠ 后端给的是**时刻**而不是"已经跑了多少秒"：页面不轮询，一个标量读完就开始变假，
   * 而时刻可以让界面自己往前算（总览每 30 秒重算一次，不用再问后端）。
   */
  startedAtMs: number
  taskCount: number
  lastRun: RunRecord | null
  /** 调度循环是否**真的**在跑。运行时实况，不是配置项。 */
  schedulerRunning: boolean
  runningCount: number
  scheduleCount: number
  pipelineCount: number
}

export const getVersion = () => request<VersionInfo>('/api/version')

export const getStatus = () => request<ServiceStatus>('/api/status')

export const listCapabilities = () => request<{ tasks: TaskMeta[] }>('/api/capabilities')

/**
 * 一条口令在**共享池**里的可复用性。
 *
 * ⚠ 只有三种：`unknown`（抓到了还没验过）、`valid`（验过、能用）、
 * `invalid`（口令错**或已过期** —— 用户口径：过期视同无效，原因留在 `reason` 里）。
 */
export type KoulingState = 'unknown' | 'valid' | 'invalid'

/** 口令从哪来。手工粘贴的**验证通过后也进池**（用户口径 2026-10-01）。 */
export type KoulingSource = 'danmaku' | 'xiaohongshu' | 'manual'

export interface PoolCode {
  value: string
  state: KoulingState
  source: KoulingSource
  /** 被判无效/有效的原因（人的说法，直接显示）。 */
  reason?: string | null
  firstSeenMs: number
  validAtMs?: number | null
}

/** 池的当前状态（`GET /api/kouling`）。 */
export interface KoulingPoolView {
  /** 本周期开始的时刻（5 小时 TTL / 判定门关了再宽限 10 分钟 ⇒ 翻篇）。 */
  cycleAtMs: number
  /** **有效**口令条数 —— 界面与"要不要再抓"的唯一判据。 */
  validCount: number
  /** 够 2 条了 ⇒ 全场不需要再抓口令。 */
  stopScraping: boolean
  codes: PoolCode[]
  /** 下一场的开播时间 —— 预告场给的**本地**字符串，界面**原样**显示（它没有时区）。 */
  nextStart: string | null
  /**
   * 下一场是**哪一间**（房间标题）。
   *
   * 用户口径（2026-10-02）：「你这 10 月 2 号不是目标直播间吧」——
   * 只写时间根本分不出是不是自己那场 ⇒ 后端把标题一起给（与 `nextStart` 同源同一次选择）。
   */
  nextTitle: string | null
  /**
   * 下一场是**哪个主播**（上游给的昵称，原样）。
   *
   * 用户口径 2026-10-02：「直播要看主播名字」—— 光有时间与场次名，
   * 还是分不出"这是不是我那个直播间"（上一个 bug 正是如此 ✗）。
   * 与 `nextStart` / `nextTitle` **同源同一次选择** ✓。
   *
   * ⚠ 可选：老后端还没有这个字段（前端先上 ✓，不写 `undefined` 就不渲染 ✓）。
   */
  nextAnchor?: string | null
  /** 上面那个日程是什么时候探测到的（`null` = 还没探测过 —— 别渲染成空白 ✗）。 */
  scheduleAtMs: number | null
  /** 观测到的场次（**新的在前**）。`endedAtMs: null` = 还在播。 */
  sessions: { startedAtMs: number; endedAtMs: number | null }[]
}

/** 读共享口令池。**全实例共享**（跨 mcloud 用户、跨重启）。 */
export const getKoulingPool = () => request<KoulingPoolView>('/api/kouling')

/** 一个任务的参数配置（任务页「配置」那一页）。 */
export interface TaskConfigView {
  name: string
  specs: TaskParamSpec[]
  /** 已保存的值。缺省键后端已经补成默认值，前端不必自己算。 */
  params: Record<string, unknown>
}

export const getTaskConfig = (task: string) =>
  request<TaskConfigView>(`/api/tasks/${encodeURIComponent(task)}/config`)

/** 保存任务参数。**PUT = 整体替换**：没给的键回落到默认值。 */
export const putTaskConfig = (task: string, params: Record<string, unknown>) =>
  request<TaskConfigView>(`/api/tasks/${encodeURIComponent(task)}/config`, {
    method: 'PUT',
    body: JSON.stringify({ params }),
  })

export const listRuns = () => request<RunRecord[]>('/api/runs')

/**
 * 一个账号**今天**的AI豆进出。
 *
 * ⚠ 账是**按账号**记的：不能把几个账号加总成一个"总AI豆" ——
 * 加出来的数**哪个账号都用不了**（豆不能跨账号花）。所以这一层没有"合计"字段，
 * 界面也一个一个账号地摆（第一版按"总额"设计，是错的）。
 */
export interface BeanTodayAccount {
  /** 账号主键（哈希）。显示要走 `accountName(id)`，别把哈希摆到界面上。 */
  account: string
  /** 今日净增（进 − 出）。**可负** —— 兑换扣豆也在这条流水里。 */
  net: number
  /** 今日收入合计（正数）。 */
  gained: number
  /** 今日支出合计（取正数）。 */
  spent: number
  /** 今日读到几笔流水。 */
  count: number
}

/**
 * 今日AI豆 —— 由**云朵记录**（上游 `cloudRecordV2` 的收支流水）按账号汇总而来。
 *
 * ⚠⚠ **后端还没有这个端点**（2026-09-30）。前端先把契约接上：
 * 端点一旦落地，总览那张卡**一行都不用改**。缺的三块与口径见
 * `docs/task-inventory.md` §5。
 *
 * # 口径
 *
 * - **一个账号一份**（见 [`BeanTodayAccount`]）。这一层刻意**没有** `net`/`gained`
 *   这样的总额字段：豆是按账号记的，合计出来没有使用场景。
 * - "今天"按**服务端**时区算（与 `Schedule.timezone` 同一套），并把那个时刻用
 *   `nowMs` 回给前端。⚠ 前端**不要**拿本地时钟自己算日界：那会在跨时区/漂移时
 *   把"今天"算到别的日子上，而界面上看不出来（组件指南 §5.9 同一条理由）。
 * - `failed` 非空 ⇒ **这份读数是不完整的**。界面必须**逐个账号**说出是谁没读到，
 *   **不能**把它当成 0 混进那一行（"没读到"与"今天没进出"是两件事）。
 *   账号主键照 [`AccountSummary.id`] 那一套（哈希，只当键用）。
 */
export interface BeanToday {
  /** 服务端算"今天"用的时刻（epoch 毫秒）。 */
  nowMs: number
  /** 每个账号一份。空数组 = 一个账号都没配。 */
  accounts: BeanTodayAccount[]
  /** 没读到流水的账号，以及**后端原话**的原因（原样透出）。 */
  failed: { account: string; error: string }[]
}

/**
 * `GET /api/beans/today` —— 今日AI豆（云朵记录汇总）。
 *
 * ⚠ 2026-10-03 更正：这条注释原先写「后端未实现」，**已经不成立** ——
 * 端点在 `crates/mcloud-server/src/routes/beans.rs` 实现并已在 `routes/mod.rs` merge
 * （review §3 D7 / §5 C5）。读不到时的原因看响应（未登录 401、上游读不到 502 那一族），
 * 不要再按"一定是 404"去查。
 */
export const getBeanToday = () => request<BeanToday>('/api/beans/today')

/**
 * 明细里的一笔流水（`GET /api/beans/records`）—— **上游原话**，只做了时间归一。
 *
 * ⚠ 与 [`BeanTodayAccount`] 是两条链：那个是**汇总**，这个是**逐笔**。
 * 数据源同一个（上游 `cloudRecordV2` 的流水），**不是**本仓的运行记录 ——
 * 后者是"我们做过什么"，流水是"上游账上发生了什么"，对不上是常态。
 */
export interface BeanRecord {
  /** `insertTime` 归一成的 epoch 毫秒（**服务端**算的，前端不自己算）。 */
  atMs: number | null
  /**
   * 原样的 `insertTime`。`atMs` 为 `null`（上游换过格式 / 没有这个字段）时显示它 ——
   * 明细列表里"少了一笔"与"这一笔时间看不懂"必须分得开。
   */
  rawTime: string | null
  /** 正负号就是进出（**可负** = 扣豆）。 */
  num: number
  summary: string
  mark: string
}

/**
 * 一个账号的 AI豆流水明细（**不限于今日**）。
 *
 * # 窗口
 *
 * `days` 是**自然日**：`1` = 今天（东八区日界起）、`7` = 含今天共 7 天、
 * `0` = 全部。日界由**服务端**算并把 `nowMs` / `sinceMs` 回给前端 ——
 * 前端不要拿本地时钟自己算（跨时区/漂移时会把窗口挪一天，而界面上看不出来）。
 */
export interface BeanRecords {
  /** 回显：这一份明细是哪个账号的。 */
  account: string
  /** 服务端算窗口用的时刻（epoch 毫秒）。 */
  nowMs: number
  /** 回显请求的窗口档位（`0` = 全部）。 */
  days: number
  /** 窗口下界（epoch 毫秒）；`days = 0` 时为 `null`。 */
  sinceMs: number | null
  /** 按时间**倒序**（新的在前）。 */
  records: BeanRecord[]
  /** 时间解不出的笔数（**已经在 `records` 里**，不是被丢掉的）。 */
  undated: number
  /** 还有没读到的（撞上页数/条数上限）⇒ 这一份可能不全，界面必须说出来。 */
  hasMore: boolean
}

/**
 * `GET /api/beans/records?account=<id>&days=<n>` —— **一个**账号的 AI豆流水明细。
 *
 * 与 [`getBeanToday`] 的两条硬区别（见 `crates/mcloud-server/src/routes/beans.rs`）：
 * 一次只读一个账号；失败**不**吞成空列表 —— 读不到就是 4xx/5xx，
 * 界面要如实说"没读到"，不能显示成"这个号没有流水"。
 */
export const getBeanRecords = (account: string, days: number): Promise<BeanRecords> =>
  request<BeanRecords>(`/api/beans/records?account=${encodeURIComponent(account)}&days=${days}`)

// ── 上游状态（**按需取**）────────────────────────────────────────────────
//
// ⚠ 这一族**不落运行历史、不占一条任务**：页面点一下 ⇒ 后端现去上游取一份 ⇒ 回给前端 ✓
// （用户口径 2026-10-07：「这个表单独放一个专门查看上游各种状态的页面，**需要主动获取**，
// 而不是跟着其他任务一起」）。
//
// ⚠ 它们读的都是**上游账上的读数**，与本仓的运行记录无关 —— 后者是"我们做过什么"，
// 前者是"上游那边现在是什么样"，对不上是常态 ✓。

/** 一条 App 任务（后端 `app_task_matrix::items_json` 逐字对应 ✓）。 */
export interface UpstreamAppTaskItem {
  taskId: number | null
  name: string | null
  description: string | null
  groupId: string | null
  state: string | null
  limitType: string | null
  enable: number | null
  currStep: number | null
  process: number | null
  marketName: string | null
  /** 玩法 615 那一族的**门**就在这一格里 ✓。 */
  buttonAppLink: string | null
  /**
   * `buttonProperty.out.text` —— **上游自己给这条任务、这个状态写的那句话** ✓。
   *
   * ⚠ 别当状态词用：它是**逐条**的（实拍同一趟 `WAIT` 的两条，一条「去完成」一条「开启中」✓）。
   */
  buttonOutText: string | null
  /**
   * `button.out.day` —— **上游给的"进行到第几天"**（**字符串**，实拍 `"5"` ✓）。
   *
   * ⚠ 进度**不在** `process` 里：实拍那条「连续31天开启邮箱APP通知」`process` 是 0，
   * 而 `day` 是 `"5"` ⇒ 只读 `process` 就会"App 有进度、这里没有" ✗。
   */
  buttonOutDay: string | null
  /** `button.out.canReceive` —— 上游说"现在能不能领"（`1` 才算能领 ✓）。 */
  buttonCanReceive: number | null
  /** **进行中**那一档文案（实拍 551/1079 是「开启中」✓）。 */
  buttonTextMiddle: string | null
  /** **已完成**那一档文案（实拍 550 是「已领取」✓）。 */
  buttonTextFinish: string | null
  /** `actionMiddle` —— 空/非空区分"在不在进行中" ✓（实拍 551/1079 是 `"jump"`、550 是 `""`）。 */
  buttonActionMiddle: string | null
}

/** 状态分布。三个数**分开**报 —— `unknown` 不能并进"未完成" ✗。 */
export interface UpstreamStateCounts {
  done: number
  waiting: number
  unknown: number
}

/** `GET /api/upstream/app-task-list?account=<id>` —— App 任务那张表（**16 发只读** ✓）。 */
export interface UpstreamAppTaskList {
  account: string
  fetchedAtMs: number
  /** 要拉几枪（常量 16 ✓）。 */
  fetches: number
  /** 真成了几枪 ✓ —— 少于 `fetches` 时清单**少了几组**，界面必须说出来。 */
  succeeded: number
  rawCount: number
  dedupedCount: number
  stateCounts: UpstreamStateCounts
  items: UpstreamAppTaskItem[]
  /** 没读到的那几组（空表 = 十六组全成 ✓）。 */
  failures: string[]
}

export const getUpstreamAppTaskList = (account: string): Promise<UpstreamAppTaskList> =>
  request<UpstreamAppTaskList>(
    `/api/upstream/app-task-list?account=${encodeURIComponent(account)}`,
  )

/** 日历里的一格（`infoV3` 的 `cal[]` ✓）。 */
export interface UpstreamSignInDay {
  day: number
  signed: boolean
  /** 非本月的格子是上下月的补位，**不算今天** ✓。 */
  currentMonth: boolean
  points: number
  today: boolean
}

/** `GET /api/upstream/signin-history?account=<id>` —— 签到历史（当月日历）。 */
export interface UpstreamSignInHistory {
  account: string
  fetchedAtMs: number
  total: number
  signCount: number
  monthDays: number
  /** 今天签到**能拿**多少（不是已到手 ✓）。 */
  signInPoints: number
  toReceive: number | null
  days: UpstreamSignInDay[]
  /**
   * **AI豆中心当下可做的事**（`receiveList` ✓）。
   *
   * ⚠ 它与 `days` 来自**同一发 `infoV3`** ⇒ 报出来不多花一枪 ✓。
   * 2026-10-07 之前这是一条独立任务（`task-list`）—— 用户否了（「这个不是说没用吗」）：
   * 它只读、什么都不做，而数据本来就在这一发里 ✗。
   */
  receiveList: UpstreamReceiveItem[]
  /** 签到翻倍机会这一趟能不能用 ✓。 */
  multipleAvailable: boolean
}

/** `receiveList` 里的一条（AI豆中心那本账上"可做的事" ✓）。 */
export interface UpstreamReceiveItem {
  /** 上游**原始码**（0 现在可领 / 1 已到期可领 / 2 下月可领）—— 认不出的码也原样带出来 ✓。 */
  cloudType: number
  cloudNum: number
  /** 中文档位（**后端**给的，前端别各写一套映射 ✗）。 */
  label: string
  recordId: string | null
  acceptDate: string | null
}

export const getUpstreamSigninHistory = (account: string): Promise<UpstreamSignInHistory> =>
  request<UpstreamSignInHistory>(
    `/api/upstream/signin-history?account=${encodeURIComponent(account)}`,
  )

/** 一条奖品记录（`flag == 1` ⇒ 待领取 ✓）。 */
export interface UpstreamPrize {
  prizeName: string
  pending: boolean
  market: string
  oid: string
  prizeId: number
  expireTime: string | null
  insertTime: string | null
}

/** `GET /api/upstream/pending-prizes?account=<id>` —— 待领取奖品（**只查第一页** ✓）。 */
export interface UpstreamPendingPrizes {
  account: string
  fetchedAtMs: number
  pendingCount: number
  /** ⚠ 最多 10 条（只查第一页）⇒ 界面必须说"只查了第一页" ✗ 别读成"全部就这些"。 */
  records: UpstreamPrize[]
}

export const getUpstreamPendingPrizes = (account: string): Promise<UpstreamPendingPrizes> =>
  request<UpstreamPendingPrizes>(
    `/api/upstream/pending-prizes?account=${encodeURIComponent(account)}`,
  )

export const getRun = (id: number) => request<RunRecord>(`/api/runs/${id}`)

/**
 * 一行**服务端**日志（`/api/logs` ✓）—— 与"某次运行的日志"不是一回事：
 * 这是整个进程的（调度器 ✓、凭据续期 ✓、上游 http ✓）。
 */
export interface ServerLogLine {
  level: string
  /** 来源模块（`mcloud_engine::scheduler` 这种 ✓）—— 排障时"谁在说话"很关键 ✓ */
  target: string
  message: string
  at_ms: number
  /**
   * `'task'`（某次运行的日志）/ `'ops'`（实例自己的：认证失败、路径被拒、调度器、上游 http）。
   *
   * 判据在**后端**（`logbuf::category_of`：只有 `mcloud_app::run_labels::trace_event`
   * 打的算任务日志 ✓）。两档**可见性不同**：任务日志按账号归属、运维日志只有管理员 ✓。
   */
  category: string
  /**
   * 这一行属于哪个账号（`message` 里 `account="…"` 那格的**结构化**副本 ✓）；没有就是 `null`。
   *
   * ⚠ 2026-10-06 起不用再从正文里正则抠了（原先 `logs-page.tsx` 就是那么干的 ✓）——
   * 那是"环里没有结构化字段"时代的将就 ✓。
   */
  account: string | null
  /**
   * **全局单调**序号（2026-10-09 新增）—— 翻页游标 ✓（`?before=<seq>` ✓）。
   *
   * ⚠⚠ 它**跨重启单调**（起点是"启动时刻毫秒 << 12"再自增 ✓，见
   * `crates/mcloud-server/src/logbuf.rs::ServerLog::seq`）—— 游标一旦会重复，
   * 往上翻页就会取错页、甚至翻不动 ✗。
   *
   * ⚠ 它**不是"第几行"**：中间会跳号（进程重启、以及被内存窗口淘汰掉的行 ✓）——
   * 游标只需要"可比大小"，不需要连续 ✓。所以界面上**别**拿它算行号 ✗。
   */
  seq: number
}

/**
 * 取一页服务端日志。
 *
 * `level` 的语义是"**不低于**它" ✓（与后端 `logbuf::rank_of` 同一口径 ✓）。
 *
 * # 两条路（2026-10-09 冻结的契约 ✓）
 *
 * | 参数 | 给什么 |
 * |---|---|
 * | 不给 `before` | **最近** `limit` 行 ✓（打开页面时的第一屏 / 实时轮询 ✓） |
 * | `before=<seq>` | `seq < before` 的最后 `limit` 行 ✓（**往上翻页** ✓） |
 *
 * 两条路都**升序**返回 ✓，且 `level` / `q` / `category` 在两条路上**都生效** ✓
 * ⇒ 翻历史时要把当前筛选条件一起带上，否则"翻上去的那几页突然不过滤了"✗。
 *
 * ⚠ 返回条数 `< limit` ⇒ **到头了**（这是后端给的唯一判据 ✓）——
 * 别拿"要了 500 就一定有 500"当判据 ✗：后端会把 `limit` 夹到自己的上限，
 * 而且末尾那一页天然不足 ✓。
 */
export const listServerLogs = (params: {
  limit?: number
  /** 往上翻页的游标：`seq < before` 的最后 `limit` 行 ✓（= 当前最上面那一行的 `seq` ✓）。 */
  before?: number
  level?: string
  q?: string
  /** `'all'`（缺省）/ `'task'` / `'ops'` —— ⚠ `'ops'` 只有管理员能拿（普通用户 403 ✓）。 */
  category?: string
}): Promise<ServerLogLine[]> => {
  const sp = new URLSearchParams()
  if (params.limit) sp.set('limit', String(params.limit))
  // ⚠ `before` 用 `!= null` 判（不是 `if (params.before)`）：`0` 是**合法**游标，
  // 而 `if (0)` 会把它悄悄丢掉 ⇒ 那一页变成"最近 limit 行"（整页重复 ✗）。
  if (params.before != null) sp.set('before', String(params.before))
  if (params.level) sp.set('level', params.level)
  if (params.q) sp.set('q', params.q)
  if (params.category) sp.set('category', params.category)
  return request<ServerLogLine[]>(`/api/logs?${sp.toString()}`)
}

/**
 * 启动一次运行（一个账号，或**多账号/全部**）。
 *
 * `background = true` 时**立刻**返回真实的 run id（多账号时是一组），任务在后台跑。
 * 这是"界面上能点中止"的前提：同步执行的话请求还没返回，
 * 用户根本没有可中止的时机。
 *
 * # 多账号（用户口径 2026-10-01）
 *
 * 「任何（包括直播小红花）应该允许配置多账号/全部，点击运行后后端保证串行执行」。
 *
 * * 传**字符串** ⇒ 单账号，请求体照旧是 `{ account }`（兼容）；
 * * 传**数组** ⇒ 请求体是 `{ accounts }`；**空数组 = 当前用户名下的全部账号**
 *   （与 `ScheduleInput.accounts` 同一口径）。
 *
 * ⚠ 后端保证**串行**：一个账号跑完再跑下一个（上游限流最怕并发打过去），
 * 因此这里不要并发调用本函数去"加速"。
 */
export const startRun = (
  task: string,
  accounts: string | string[],
  body: Record<string, unknown> = {},
  background = false,
) =>
  request<RunResponse>('/api/runs', {
    method: 'POST',
    body: JSON.stringify(
      typeof accounts === 'string'
        ? { task, account: accounts, body, background }
        : { task, accounts, body, background },
    ),
  })

/** SSE 事件流地址。交给 `EventSource` —— 它自己带同源 cookie。 */
export const runEventsUrl = (runId: number) => `/api/runs/${encodeURIComponent(String(runId))}/events`

/**
 * 订阅某次运行的事件流。
 *
 * 返回取消函数；调用方**必须**在卸载/结束时调用它 ——
 * `EventSource` 断线会自动重连，不显式关闭就会一直挂着。
 *
 * # 终局即关闭
 *
 * 收到 `finished` 后本函数会**自动** `close()`，并调用 `onTerminal`。
 * 这不是可选的优化：`EventSource` 按规范会自动重连，不关的话
 * 一次已结束的运行会被无限重连（每次带 `Last-Event-ID`，
 * 服务端补发 0 条再关），每个打开的页面都挂着一条永久连接。
 */
/**
 * 活着的 SSE 连接（页面级）。
 *
 * # 为什么要登记 + 在 `pagehide` 里显式关掉
 *
 * 用户口径（2026-10-03）：「刷新页面，js 堆栈内存会递增」。**Chrome 里复现不出来** ——
 * 实测：每次刷新前都开一条 SSE（`readyState=1`），6 次刷新的堆 0.79 MB、
 * 脱离文档的节点 794 个，**一个不动** ✓（口径与 `regress/leak.mjs` 一致）。
 * 所以这不是"我们代码里有个 JS 泄漏"。
 *
 * 但 WebKit 与 Blink 不同：**老文档还挂着未关闭的连接时，它的销毁可能被推迟**
 * ⇒ 上一页的堆留在进程里 ⇒ 内存面板看起来"每次刷新都涨" ✗。
 * 这条登记处就是堵这条路：`pagehide`（刷新/关标签/跳走都会发）里把所有流**主动关掉**，
 * 让老文档**没有任何理由**被留住 ✓。
 *
 * ⚠ 这是**加固**，不是"修好了一个已复现的 bug" —— 别把它写成后者。
 * 它零风险（关掉本来就要死的连接）、且能让"浏览器保留老文档"这条路径彻底不成立。
 */
const activeStreams = new Set<() => void>()
let pagehideHooked = false

/** 页面要走了：把所有流关掉。也导出给测试用（否则这段只能靠真刷新验证）。 */
export const closeAllStreams = (): void => {
  for (const stop of activeStreams) stop()
  activeStreams.clear()
}

export function subscribeRun(
  runId: number,
  handlers: {
    onEvent?: (ev: RunEvent) => void
    onError?: (err: Event) => void
    /** 终局（已结束）。此时流已被关闭，调用方不必再关。 */
    onTerminal?: (ev: RunEvent) => void
    /**
     * 断线重连时带上"已看到的序号"。
     *
     * 服务端据此只补发新的。传 `null` 表示从头开始。
     */
    lastEventId?: number | null
  } = {},
): () => void {
  const url = new URL(runEventsUrl(runId), window.location.origin)
  if (handlers.lastEventId != null) {
    // 语义是"**已经看到**的序号"，服务端返回它之后的。
    // 传成"下一个要取"会静默跳过一条。
    url.searchParams.set('after', String(handlers.lastEventId))
  }

  const source = new EventSource(url.toString())
  let closed = false

  const close = () => {
    if (closed) return
    closed = true
    source.close()
    activeStreams.delete(close)
  }
  activeStreams.add(close)
  if (!pagehideHooked) {
    // 只挂一次：这条监听器是**页面级**的，每条流各挂一遍就变成"监听器随流增长"了 ✗。
    // ⚠ 用 `pagehide` 而不是 `unload`：`unload` 会破坏 Safari 的往返缓存，
    // 而 `pagehide` 在刷新/关标签/跳走时都会发 ✓。
    pagehideHooked = true
    window.addEventListener('pagehide', closeAllStreams)
  }

  const handle = (raw: MessageEvent) => {
    let ev: RunEvent
    try {
      ev = JSON.parse(raw.data) as RunEvent
    } catch {
      // 单条事件解析失败不应中断整条流：反代插入的心跳/注释行
      // 不是要报给用户的事
      return
    }
    handlers.onEvent?.(ev)
    if (ev.type === 'finished') {
      close()
      handlers.onTerminal?.(ev)
    }
  }

  // 后端用 `event:` 名区分类型，需分别注册
  for (const name of ['started', 'log', 'progress', 'finished'] as const) {
    source.addEventListener(name, handle as EventListener)
  }
  if (handlers.onError) source.onerror = handlers.onError

  return close
}

// ── 定时任务 ────────────────────────────────────────────────────────────

/** 定时任务。`nextFireAt` 由服务端算 —— 各客户端各算一遍必然分叉。 */
export interface Schedule {
  id: string
  name: string
  task: string
  accounts: string[]
  expr: string
  timezone: string
  body: unknown
  enabled: boolean
  /** **水位线**：已处理过的触发时刻。不是"上次运行时间"。 */
  lastFireMs: number | null
  lastRunMs: number | null
  lastStatus: 'ok' | 'error' | 'skipped' | null
  lastError: string | null
  runCount: number
  /** 下次触发时间。**未启用时为 null** —— 给一个就是在骗人。 */
  nextFireAt: number | null
  /**
   * 绑定的推送规则 id（**多选** ✓，用户口径「一个行为可以选择多个规则」）。
   *
   * ⚠ **可选**是给老数据/别的调用方留的：没有这一格时当**空数组**读 ✓
   * （空 = 不推 ✓）。写成必填的话，老数据一读就 `undefined`，界面会把它显示成一个坏值 ✗。
   */
  pushRules?: string[]
  /**
   * 这一条定时**什么时候推**（运行结局条件 ✓）。
   *
   * ⚠ **空数组 = 跑完就推** ✓（用户口径：「全不勾 = 跑完就推」）——
   * 不是"不推" ✗。只含**运行类**事件（`kind: "run"` ✓）。
   */
  pushEvents?: string[]
  /**
   * **系统维护标记**：非空 = 这条定时由某个系统功能维护
   * （今天只有 `"live-auto"` = 「自动直播定时」✓）。
   *
   * ⚠ 可选是给**手建的**定时留的（它们这一格恒为 `None`，后端
   * `skip_serializing_if` 直接不落这一格 ✓）—— 写成必填会让老数据一读就是坏值 ✗。
   * 界面据此给它标「系统维护」并说明"时刻由系统维护" ✓。
   */
  managedBy?: string
  /**
   * **用户意愿**（只对 `managedBy` 非空的那条有意义 ✓）：用户还想不想要这条自动定时。
   *
   * ⚠ 它与 `enabled` **不是一回事**，别合并（见 `mcloud-core::Schedule::managed_wanted`）：
   * `enabled` 是**算出来的**（有下一场才为真 ✓），这一格是**用户选的**。
   * ⚠ 定时页上翻那条定时的开关 ⇒ 后端就是改这一格 ✓（**同一格**，不是第二套开关 ✓）。
   */
  managedWanted?: boolean
}

export interface ScheduleInput {
  name: string
  task: string
  accounts: string[]
  expr: string
  timezone: string
  body?: unknown
  enabled: boolean
  /** 绑定的推送规则 id（多选 ✓）。空数组 = 不推 ✓。 */
  pushRules: string[]
  /** 运行结局条件（多选 ✓）。空数组 = 跑完就推 ✓。 */
  pushEvents: string[]
}

/**
 * 排期里表示"这一条跑的是**编排**"的约定任务名 —— 必须与后端
 * `mcloud_engine::scheduler::PIPELINE_TASK` 一致 ✓。
 *
 * ⚠ 编排**不是**注册表里的任务 ✗（它是一条链）⇒ 排期靠"约定名 + `body.pipelineId`"
 * 表达 ✓。放这里是因为**两页**都要用它（编排页写 ✓、定时页显示 ✓），
 * 各写一份字符串迟早漂移 ✗。
 */
export const PIPELINE_TASK = 'pipeline'

export const listSchedules = () => request<Schedule[]>('/api/schedules')

export const saveSchedule = (id: string, input: ScheduleInput) =>
  request<Schedule>(`/api/schedules/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })

export const deleteSchedule = (id: string) =>
  request<{ ok: boolean }>(`/api/schedules/${encodeURIComponent(id)}`, { method: 'DELETE' })

// ── 任务编排 ────────────────────────────────────────────────────────────

export interface PipelineStep {
  task: string
  body?: unknown
  /** 这一步失败也继续后续步骤。默认 false —— 多数步骤是有依赖的。 */
  continueOnError: boolean
}

export interface Pipeline {
  id: string
  name: string
  accounts: string[]
  accountMode: 'sequential' | 'parallel'
  /** 步骤失败时是否中止整条链。 */
  stopOnError: boolean
  steps: PipelineStep[]
  enabled: boolean
  lastRunMs: number | null
  /** 绑定的推送规则 id（多选 ✓）。可选/缺席的理由与 `Schedule.pushRules` 逐条相同。 */
  pushRules?: string[]
  /** 运行结局条件（多选 ✓，空 = 跑完就推 ✓）。同 `Schedule.pushEvents`。 */
  pushEvents?: string[]
}

export interface PipelineInput {
  name: string
  accounts: string[]
  accountMode: 'sequential' | 'parallel'
  stopOnError: boolean
  steps: PipelineStep[]
  enabled: boolean
  /** 绑定的推送规则 id（多选 ✓）。空数组 = 不推 ✓。 */
  pushRules: string[]
  /** 运行结局条件（多选 ✓）。空数组 = 跑完就推 ✓。 */
  pushEvents: string[]
}

export const listPipelines = () => request<Pipeline[]>('/api/pipelines')

export const savePipeline = (id: string, input: PipelineInput) =>
  request<Pipeline>(`/api/pipelines/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })

export const deletePipeline = (id: string) =>
  request<{ ok: boolean }>(`/api/pipelines/${encodeURIComponent(id)}`, { method: 'DELETE' })

/** 后台执行一条编排。立即返回，进度去运行历史看。 */
export const runPipeline = (id: string) =>
  request<{ pipeline: string; started: boolean }>(
    `/api/pipelines/${encodeURIComponent(id)}/run`,
    { method: 'POST' },
  )

// ── 日志与中止 ──────────────────────────────────────────────────────────

export interface LogLine {
  runId: number
  level: 'debug' | 'info' | 'warn' | 'error'
  /**
   * 分类（界面按它上徽章与颜色 ✓）。
   *
   * ⚠ 可选是给**老记录**留的：这一格 2026-10-09 才进 `mcloud_core::LogLine`
   * （此前只有 `RunRecord.logs` 那份内联日志带分类，落盘那份只有级别 ✓）⇒
   * 缺它就按 `level` 兜底（那只能分出三档，会丢掉"成功/失败"的区别 ✓）。
   */
  kind?: LogKind
  message: string
  atMs: number
  seq: number
}

/**
 * 取某次运行的日志（**分页**，2026-10-09 用户口径「大日志做虚拟加载或者分页，
 * 避免一次加载太多」✓）。
 *
 * 三个游标**互斥**（给两个以上是调用方的错，后端只认一个）：
 *
 * | 参数 | 拿到的 |
 * |---|---|
 * | 都不给 | **最后** `limit` 行（`limit` 缺省 500 ✓） |
 * | `before=<seq>` | `seq < before` 的最后 `limit` 行 —— **往上翻页**用 ✓ |
 * | `after=<seq>` | `seq > after` 的最前 `limit` 行 —— **断线续传**用 ✓ |
 *
 * ⚠ `limit` 的**上限是用户设的 [`StorageLimits.logLimitPerRun`]**（后端夹一下 ✓）——
 * 要多了不会报错，只会少给 ⇒ 调用方**不能**拿"要了 500 就一定有 500"当判据 ✗。
 * 判"还有没有更早的"只有一条：**返回条数 < `limit`** ✓。
 *
 * ⚠ `after` / `before` 的语义都是"**已经看到**的那一行的 `seq`"，不是"下一个要取"——
 * 混淆会静默跳过（`after`）或重复（`before`）一条 ✓（旧签名那个位置参数就是 `after` ✓，
 * 现在收进 opts 里，别再留一个半死不活的位置参数 ✗）。
 */
export const getRunLogs = (
  runId: number,
  opts: { limit?: number; before?: number; after?: number } = {},
): Promise<LogLine[]> => {
  const sp = new URLSearchParams()
  if (opts.limit != null) sp.set('limit', String(opts.limit))
  if (opts.before != null) sp.set('before', String(opts.before))
  if (opts.after != null) sp.set('after', String(opts.after))
  const q = sp.toString()
  return request<LogLine[]>(`/api/runs/${runId}/logs${q ? `?${q}` : ''}`)
}

/**
 * 请求中止一次运行。
 *
 * 中止是**协作式**的：返回 `cancelled: true` 只表示请求已受理，
 * 不表示已经停了。`cancelled: false` 通常意味着任务刚好跑完 ——
 * 那是正常竞态，不是错误。
 */
export const cancelRun = (runId: number) =>
  request<{ cancelled: boolean; running: boolean }>(`/api/runs/${runId}/cancel`, {
    method: 'POST',
  })

// ── 云盘账号登录 ────────────────────────────────────────────────────────
//
// 注意与 `/api/session` 的区别：那个登的是**本实例管理员**，
// 这里登的是**要跑任务的云盘账号**。两者的凭据存在不同的文件里。

/** 平台形态。mac 与 win 在请求头、UA 与落盘数据上都不一样。 */
export type PcPlatform = 'windows' | 'macos'

export interface StartQrResponse {
  session_id: string
  /** 二维码载荷（原始链接）。排障与自渲染用。 */
  payload: string
  /** 二维码图片（SVG 的 data URL），直接塞进 `<img src=...>`。 */
  image_svg: string
  platform: PcPlatform
  platform_label: string
  poll_interval_ms: number
  ttl_seconds: number
  device_id: string
}

export interface LoginOutcome {
  phase: 'not_scanned' | 'scanned' | 'logged_in' | 'failed'
  terminal: boolean
  reason?: string
  retry_after_ms?: number
  /** 成功时的**掩码**账号名（完整手机号不出现在响应里）。 */
  account?: string
  platform?: PcPlatform
}

/** 出码。**不发上游请求**：二维码是服务端本地拼的。 */
export const startQrLogin = (platform: PcPlatform) =>
  request<StartQrResponse>('/api/login/qr', {
    method: 'POST',
    body: JSON.stringify({ platform }),
  })

/** 问一次"扫了没"。用 POST：它会推进服务端会话状态。 */
export const pollQrLogin = (sessionId: string) =>
  request<LoginOutcome>('/api/login/qr/poll', {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId }),
  })

/** 放弃一次扫码（离开页面时调用，让服务端把会话丢掉）。 */
export const cancelQrLogin = (sessionId: string) =>
  request<{ cancelled: boolean }>(`/api/login/qr/${encodeURIComponent(sessionId)}`, {
    method: 'DELETE',
  })

export interface SliderPayload {
  background: string
  puzzle: string
  width: number | null
  /** 已解出的答案。上游把答案加密在题面里，不需要图像识别。 */
  answer?: string
}

export interface SmsStartResponse {
  phase: 'sent' | 'need_slider' | 'slider_exhausted'
  remain_get_times?: number
  slider?: SliderPayload
}

/**
 * 发验证码。
 *
 * ⚠ `sliderAnswer` 必须在**需要滑块时回填**（用上一次响应里 `slider.answer`）：
 * 上游在需要滑块时**不发**短信，只有带上答案的那一次才会真的把短信发出去。
 * 不带它就再点一次，拿回来的是同一道题 —— 表现是"自动完成没反应"。
 */
export const startSmsLogin = (phone: string, platform: PcPlatform, sliderAnswer?: string) =>
  request<SmsStartResponse>('/api/login/sms/start', {
    method: 'POST',
    body: JSON.stringify({ phone, platform, slider_answer: sliderAnswer }),
  })

export const submitSmsLogin = (input: {
  phone: string
  platform: PcPlatform
  code: string
  sliderAnswer?: string
}) =>
  request<LoginOutcome>('/api/login/sms', {
    method: 'POST',
    body: JSON.stringify({
      phone: input.phone,
      platform: input.platform,
      code: input.code,
      slider_answer: input.sliderAnswer,
    }),
  })

export const loginWithPassword = (username: string, password: string, platform: PcPlatform) =>
  request<LoginOutcome>('/api/login/password', {
    method: 'POST',
    body: JSON.stringify({ username, password, platform }),
  })

// ── App 端（手机客户端）短信登录 ──────────────────────────────────────
//
// 与上面那条 PC 短信链路**不是同一个协议**：App 用另一套密钥与设备头，
// 请求体还要加密。所以端点分开，前端也分开两个函数 —— 合成一个带
// `platform: 'app'` 的函数会让"我在登哪一端"变得看不出来。

export interface AppSmsStartResponse {
  phase: 'sent' | 'need_captcha' | 'slider_exhausted'
  remain_get_times?: number
  captcha_type?: number
  /** 已解出的滑块答案（可直接回填重发，不用用户去拖）。 */
  slider_answer?: string
  slider_background?: string
  slider_puzzle?: string
  slider_width?: number | null
}

/** 发验证码（App 链路）。`sliderAnswer` 与 PC 那支同一个理由，必须回填。 */
export const startAppSmsLogin = (phone: string, sliderAnswer?: string) =>
  request<AppSmsStartResponse>('/api/login/app/sms/start', {
    method: 'POST',
    body: JSON.stringify({ phone, slider_answer: sliderAnswer }),
  })

export const submitAppSmsLogin = (input: {
  phone: string
  code: string
  sliderAnswer?: string
}) =>
  request<LoginOutcome>('/api/login/app/sms', {
    method: 'POST',
    body: JSON.stringify({
      phone: input.phone,
      code: input.code,
      slider_answer: input.sliderAnswer,
    }),
  })

export const getPcDevice = () => request<{ device_id: string }>('/api/login/device')

export const rotatePcDevice = () =>
  request<{ device_id: string }>('/api/login/device/rotate', { method: 'POST' })

/** 列出已登录的云盘账号。 */
export interface AccountSummary {
  /** 账号主键：手机号的 SHA-256 前 16 位。⚠ **只当键用，别拿去显示**（认不出是哪个号）。 */
  id: string
  /** 给界面看的名字（掩码手机号，如 `19****01`；没有手机号时回落昵称）。 */
  label: string
  nickname: string | null
  /** `windows` / `macos`；`null` = 非 PC 端登录的账号。 */
  pc_platform: PcPlatform | null
  device_id: string
  /** 凭据到期的 epoch 毫秒；`0` = 解析不出。 */
  expire: number
  /**
   * 是否启用。**停用的账号不出现在各处"挑账号"的下拉里**。
   *
   * 过滤发生在**服务端**（`GET /api/accounts` 默认只给启用的）——
   * 前端各页面不需要、也不应该各写一遍过滤：漏掉的那一处正是"停用了还能选"。
   */
  enabled: boolean
}

/**
 * 列出云盘账号。
 *
 * ⚠ **默认只给启用的**（各处下拉要的就是这个）—— 账号页要能看全部（否则停用之后
 * 就没法启用回来），所以它显式传 `includeDisabled`。
 */
export const listAccounts = (opts?: { includeDisabled?: boolean }) =>
  request<AccountSummary[]>(
    opts?.includeDisabled ? '/api/accounts?includeDisabled=true' : '/api/accounts',
  )

export const deleteAccount = (id: string) =>
  request<void>(`/api/accounts/${encodeURIComponent(id)}`, { method: 'DELETE' })

/**
 * 用**一条凭据**建立或更新账号（`POST /api/accounts`）。
 *
 * 这是"手上只有 auth"那条路：不走扫码/短信/账密。账号主键是**手机号的哈希**，
 * 所以同一个号的凭据再次导入会落到同一个账号上 —— 服务端此时**只换凭据**，
 * 昵称 / 设备号 / 每账号设置都保留，并在响应里用 `created: false` 说明。
 *
 * @param input.auth 原始凭据：`Basic base64("<平台>:<手机号>:<token>")`，
 *   `Basic ` 前缀可有可无（服务端用同一套解析，见 `mcloud-upstream::auth::parse_auth`）。
 * @param input.platform **仅在凭据前缀是 `pc` 时需要**：它决定后续每一次请求的
 *   UA 与设备头，而凭据本身区分不出 win/mac（两种都是 `pc`）。
 */
export const importAccount = (input: { auth: string; platform?: PcPlatform; nickname?: string }) =>
  request<{ account: AccountSummary; created: boolean }>('/api/accounts', {
    method: 'POST',
    body: JSON.stringify(input),
  })

/**
 * 改一个账号的昵称 / 设备号 / 凭据 / 启停（`PATCH /api/accounts/{id}`）。
 *
 * # 为什么是部分更新
 *
 * 这里有一个字段是**凭据**，而凭据从不发给客户端（[`AccountSummary`] 里没有它）
 * ⇒ "读出来、改一处、写回去"这条路不存在，只能"给什么改什么"：
 * **字段缺席 = 不改**，`nickname: ''` = 清空昵称。
 */
export const updateAccount = (
  id: string,
  patch: {
    nickname?: string
    deviceId?: string
    auth?: string
    platform?: PcPlatform
    /** 启用 / 停用（不给 = 不改）。 */
    enabled?: boolean
  },
) =>
  request<AccountSummary>(`/api/accounts/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: JSON.stringify(patch),
  })

// ── 每账号设置（GET/PUT /api/accounts/{id}/settings）────────────────────
//
// 权威定义在 `crates/mcloud-core/src/account.rs` 的 `AccountSettingsDto`。这里的形状
// 与它**逐字段一一对应**，三条口径照抄：
//
// 1. **线格式是 camelCase**（与 /api/schedules、/api/runs 一致）—— 不是落盘那份
//    `AccountSettings` 的 snake_case。敲错键后端回 400，不会静默忽略。
// 2. `PUT` 是**整体替换**：每个字段都必须显式给出。
//    缺席没有"保持不变"的意思 ⇒ 客户端必须"先 GET 读全、改一处、再整体写回"。
//    ⚠ 这里**没有** `notify` 那一格了（2026-10-08 用户口径：「很显然我说的和云盘账户
//    没有关系」＋「丢弃现有情况，重写」）—— 推送凭据收进了**规则自己**身上
//    （`PushRule.config` ✓），见下面「推送规则（用户级）」那一段 ✓。
// 3. 未知键 ⇒ 400（`deny_unknown_fields`）；范围只在后端校验，前端给的是提示。

/** 推送日志级别。与后端 `PushLogLevel` 一一对应（小写）。 */
export type PushLogLevel = 'error' | 'warn' | 'info' | 'debug'

/**
 * 渠道配置里**具名字段之外**的键。
 *
 * 后端对应的是各处 `#[serde(flatten)] extra`：TS 的既定语义是"多余字段原样展开进
 * 请求体"。它不是笔误（所以后端刻意不校验），而**整体替换**意味着客户端回写时
 * 必须把这些键**原样带上** —— 丢了就是"用户没改过的字段被静默抹掉"。
 *
 * ⇒ 下面每个渠道类型都带一条索引签名，界面直接持有 GET 回来的那个对象本身。
 */
export interface TokenChannelConfig {
  token: string
  [extra: string]: unknown
}

/** 企业微信**应用**（两步：先换 access_token，再发消息）。 */
export interface WorkWeixinConfig {
  msgtype?: string
  touser?: string
  agentid?: number
  corpsecret?: string
  corpid?: string
  [extra: string]: unknown
}

/** 企业微信**机器人**（一个 webhook URL 打到底）。 */
export interface WorkWeixinBotConfig {
  url: string
  msgtype?: string
  [extra: string]: unknown
}

/** Telegram Bot。 */
export interface TgBotConfig {
  apiHost?: string
  token: string
  /**
   * ⚠ 后端是 `Value`（TS 里 chat_id 也可以是非数字的字符串，例如 `@channel`），
   * 所以这里**不收窄**成 number。
   */
  chat_id?: string | number
  disable_web_page_preview?: boolean
  proxy?: string
  [extra: string]: unknown
}

/** Bark（iOS）。 */
export interface BarkConfig {
  key: string
  level?: string
  [extra: string]: unknown
}

/** 回逍（2Im）的 query。 */
export interface TwoImQuery {
  plat?: string
  group?: string
  icon?: unknown
  [extra: string]: unknown
}

/** 回逍（2Im）。 */
export interface TwoImConfig {
  key: string
  sid: string
  query?: TwoImQuery
  msgtype?: string
  [extra: string]: unknown
}

/** 钉钉机器人。`secret` 是加签密钥（可选）。 */
export interface DingTalkConfig {
  token: string
  secret?: string
  [extra: string]: unknown
}

/**
 * 邮箱。
 *
 * ⚠ **本版不会真的发信**（后端 `mcloud-app::push::EMAIL_NOT_IMPLEMENTED`）。
 * 界面照 DTO 把字段列全，并在渠道说明里写明这一点 —— 否则用户会以为配了就生效。
 */
export interface EmailConfig {
  pass: string
  from: string
  port?: number
  host: string
  to?: string
  [extra: string]: unknown
}

/** 自定义 POST 的一条模板。`url` 支持 `{title}` / `{text}` 占位符。 */
export interface PushApiData {
  url: string
  method?: string
  headers?: Record<string, string>
  timeout?: number
  data?: Record<string, unknown>
  /** `data` 的备选（后端 `data || body`）。 */
  body?: Record<string, unknown>
  [extra: string]: unknown
}

/** 后端允许**单个模板或模板数组**（`CustomPost` 那个 untagged 枚举）。 */
export type CustomPost = PushApiData | PushApiData[]

/**
 * 推送渠道的顶层键 —— 顺序即界面里的推荐顺序（最常用的排前面）。
 *
 * ⚠ 这个名单同时是**未知键校验**的名单：界面只发这里面的键。后端会拒绝别的。
 */
export const NOTIFY_CHANNELS = [
  'pushplus',
  'serverChan',
  'workWeixinBot',
  'dingTalk',
  'customPost',
  'workWeixin',
  'tgBot',
  'bark',
  'twoIm',
  'email',
] as const

export type NotifyChannelKey = (typeof NOTIFY_CHANNELS)[number]

/**
 * 一条规则**自己的**平台与凭据（`PushConfig`）。
 *
 * ⚠ 语义（2026-10-08 重做后）：这是 `PushRule.config` 那一格 ——
 * **这条规则用哪几个平台发、token / corpid / 自定义 POST 模板填在哪** ✓。
 * 它**不再是**"你这个登录用户的渠道配置" ✗（那个接口已经删了 ✗）——
 * 所以两条规则可以各配各的 token，互不影响 ✓。
 *
 * 渠道字段缺省 / `null` = 那个平台在这条规则里**不启用**。
 */
export interface PushConfig {
  title: string
  onlyError: boolean
  minLevel: PushLogLevel
  pushplus?: TokenChannelConfig | null
  serverChan?: TokenChannelConfig | null
  workWeixin?: WorkWeixinConfig | null
  workWeixinBot?: WorkWeixinBotConfig | null
  tgBot?: TgBotConfig | null
  bark?: BarkConfig | null
  twoIm?: TwoImConfig | null
  dingTalk?: DingTalkConfig | null
  email?: EmailConfig | null
  customPost?: CustomPost | null
}

/**
 * 一个账号的设置 —— `GET` 的响应体，也是 `PUT` 的请求体（**整体替换**）。
 *
 * ⚠ 这里**没有**推送渠道（`notify`）那一格：2026-10-08 用户口径（「很显然我说的和
 * 云盘账号没有关系」＋「丢弃现有情况，重写」）把它整个删掉了 ✗ —— 推送凭据现在
 * 挂在**每条规则自己**身上（`PushRule.config` ✓），与云盘账号无关 ✓。
 */
export interface AccountSettings {
  /** 备份任务等待时间（秒）。`0` = 不等。后端上限 3600。 */
  backupWaitSecs: number
  /** 剩余多少天时刷新凭证。后端范围 0..=3650。 */
  refreshTokenDays: number
  /** 跳过的任务 **id**（数字）。旧形状是任务名，后端读旧库时会**丢弃**它们。 */
  skipTasks: number[]
  /** AI 新头像每日轮数上限。后端上限 1000。 */
  aiAvatarDailyLimit: number
}

/**
 * 读某账号的设置。
 *
 * ⚠ 别人的账号与不存在的账号回**同一个** 404（后端不区分，免得泄露存在性）——
 * 界面不能把 404 说成"网络错误"。
 */
export const getAccountSettings = (id: string) =>
  request<AccountSettings>(`/api/accounts/${encodeURIComponent(id)}/settings`)

/**
 * **整体替换**某账号的设置。
 *
 * 每个字段都必须给出。返回值是落盘后回读的那一份，
 * 调用方应当拿它**回填界面** —— 后端可能做过归一化。
 */
export const putAccountSettings = (id: string, settings: AccountSettings) =>
  request<AccountSettings>(`/api/accounts/${encodeURIComponent(id)}/settings`, {
    method: 'PUT',
    body: JSON.stringify(settings),
  })

// ── 推送规则（用户级）───────────────────────────────────────────────────
//
// **两层**（2026-10-08 用户口径「推翻重做」✓）：
// ① **规则** = 「怎么发」：名字 + `config`（**这条规则自己的平台与凭据** ✓）+
//    标题 / 仅错误 / 等级下限（都在 `config` 里 ✓）+ 汇总方式 ✓。
//    规则**自带凭据** ⇒ **没有**独立的"推送渠道"那一层 ✗
//    （`GET/PUT /api/push/config` 已经**删掉** ✗）。
// ② **行为** = 「什么时候发」，是**主动方**：定时 / 编排（`pushRules` 多选 ✓）与
//    面板行为（`PanelBindings` ✓）—— **行为选规则**（可多选 ✓），规则**不**反向持有绑定 ✓。
//
// ⚠ 规则**只按 id 定位**，请求体里没有 `owner`：归属由服务端定
// （沿用老行的、否则写当前用户）—— 让请求体带 owner 就等于让任何一个登录用户
// 把规则塞给别人 ✗（后端 `PushRuleInput` 的注释写着这条 ✓）。
//
// ⚠ 读接口**刻意不打码**（后端取舍见 `routes/push.rs`）：`PushConfig` 里
// 有第三方渠道的 token/secret。界面必须把它们当密文：默认打码、可切换查看、**不进 console**。

/**
 * 事件键（行为那一侧用的）。
 *
 * ⚠ **刻意是 `string`，不是字面量联合**：事件的权威名单由后端给
 * （`GET /api/push/events` ✓，它自己从 `PushEvent::ALL` 生成 ✓）。
 * 前端再抄一份联合类型，就是**第二份名单** —— 后端加事件时它不会报错，
 * 只会让"新事件"在这一侧变成类型谎言 ✗（后端路由的注释也写着"不让前端各写一份字符串" ✓）。
 */
export type PushEventKey = string

/**
 * 事件的两类（`GET /api/push/events` 的 `kind` ✓）。
 *
 * - `run` = **运行类**：挂在定时 / 编排的 `pushEvents` 上（空 = 跑完就推 ✓）；
 * - `panel` = **面板行为类**：没有定时可绑，只能在「设置 → 事件推送」里配规则 ✓
 *   （登录新设备 / 登录失败 / 耗时异常 / 连续失败 ✓）。
 */
export type PushEventKind = 'run' | 'panel'

/** 渠道键。同上：名单由 `GET /api/push/channels` 给，前端不抄第二份。 */
export type PushChannelKey = string

/** 汇总方式：一次运行推一条（默认）还是每一步推一条。 */
export type PushDigestMode = 'perRun' | 'perStep'

// ⚠ 这里原先有 `PushQuietHours`（`from` / `to` 两个 `HH:MM`，允许跨零点）——
// 2026-10-08 用户口径「**不要静默时间**」⇒ 随整块功能一起删掉 ✗
// （Rust 侧 `QuietHours` / `PushRule.quiet` / 投递层那句判断、界面那两格时间一起删 ✓）。

/**
 * 一条推送规则（后端 `PushRule` 的线格式，camelCase）。
 *
 * ⚠ 规则里**没有**"什么时候发"的字段 ✗（没有事件多选、没有耗时倍数/连续失败条数 ✗）——
 * 那些归**行为**（`Schedule.pushEvents` / `Pipeline.pushEvents` / `PanelBindings` ✓）。
 * ⚠ 也**没有** `quiet`（静默时段）✗ —— 那个功能整个删了 ✓。
 */
export interface PushRule {
  id: string
  name: string
  /** **这条规则自己的平台与凭据** ✓（勾哪几个平台、token / corpid / 自定义 POST 模板都在这里 ✓）。 */
  config: PushConfig
  digest: PushDigestMode
  enabled: boolean
  /** 归属。界面不显示、也不提交（服务端定 ✓）。 */
  owner?: string | null
}

/**
 * `PUT /api/push/rules/{id}` 的请求体 —— **不含 `id` / `owner`**。
 *
 * ⚠ 缺字段会**退回默认值**（`digest` → `perRun` ✗）⇒ 编辑时照旧
 * "读全、改一处、整份写回" ✓，尤其 `digest` 界面不编辑但要**原样带回** ✗。
 */
export interface PushRuleInput {
  name: string
  config: PushConfig
  digest: PushDigestMode
  enabled: boolean
}

/** `GET /api/push/events` 的一项（界面按 `kind` 分两处用）。 */
export interface PushEventOption {
  /** 落进行为里的那个键（serde 的 camelCase 名 ✓）。 */
  key: string
  label: string
  /** 新建行为时默认勾上（后端 `PushEvent::default_selected()` ✓）。 */
  defaultSelected: boolean
  /** `run` = 运行类（定时/编排的结局条件 ✓）；`panel` = 面板行为类 ✓。 */
  kind: PushEventKind
}

/** `GET /api/push/channels` 的一项。 */
export interface PushChannelOption {
  key: string
  label: string
}

/**
 * 面板行为的节流方式。
 *
 * - `every` = 每次触发都推；
 * - `onChange` = **只在状态变化时推**（登录/失败/耗时异常/连续失败这四类可能
 *   **频繁触发**，默认口径见 [`PANEL_THROTTLE_DEFAULTS`] ✓）。
 */
export type ThrottleMode = 'every' | 'onChange'

/** 面板行为的三格节流。`cooldownSecs` / `dailyCap` 为 `0` = 不设限 ✓。 */
export interface PushThrottle {
  mode: ThrottleMode
  cooldownSecs: number
  dailyCap: number
}

/**
 * 一个面板行为（「设置 → 事件推送」里的一行）：**行为选规则**（可多选 ✓）。
 *
 * ⚠ **`rules` 为空 = 这一行不推** ✓（不是"全推"✗）。
 * ⚠ 后端对空数组是 `skip_serializing_if = "Vec::is_empty"` ⇒ **`rules` 这一格可能压根不在** ✗
 * ⇒ 读的时候一律 `?? []`（写成必填的话，`.includes` 会在运行时炸 —— 类型检查看不出来 ✗）。
 */
export interface BehaviorBinding {
  rules?: string[]
  throttle: PushThrottle
  /**
   * 「耗时异常」的倍数判据：这次耗时超过同一任务历史**中位数**的这么多倍算异常 ✓。
   *
   * ⚠ **只有 `perfSlow` 这一格有意义** ✓（别的行为留 `null` ✓）。
   * `null` / 缺省 = **用户没动过** ⇒ 跟随内置默认（3.0 ✓）；
   * `Some(3)` 与 `null` 今天的判定结果一样，但界面分得清"没配过"与"配成了 3" ✓
   * —— 与 `PushConfig` 的渠道字段（`null` = 没配 ✓）同一条口径：用可空表达"用户没设"，
   * 而不是拿一个默认值冒充用户的选择 ✗。
   */
  slowFactor?: number | null
  /**
   * 「连续失败」的条数判据：连着栽这么多回算异常 ✓。
   *
   * ⚠ **只有 `perfFailStreak` 这一格有意义** ✓。`null` / 缺省 = 跟随内置默认（3 ✓）。
   */
  failStreak?: number | null
}

/**
 * 四个面板行为的绑定（`GET/PUT /api/push/panel` ✓）。
 *
 * ⚠ 这四类**没有定时可绑** —— 它们不是运行结局，而是实例级事件，
 * 所以入口在「设置」页里单独一块 ✓（用户口径 ✓）。
 */
export interface PanelBindings {
  loginNew: BehaviorBinding
  loginFailed: BehaviorBinding
  perfSlow: BehaviorBinding
  perfFailStreak: BehaviorBinding
}

/**
 * 新建面板绑定时的**保守默认**（用户口径：登录/性能这些可能频繁触发 ✓）。
 *
 * - 登录新设备 / 登录失败：`every`（这两件本来就少见，漏掉比多推更糟 ✓）；
 * - 耗时异常 / 连续失败：`onChange`（**只在状态变化时推** —— 否则一次慢跑会刷屏 ✗）。
 *
 * ⚠ 四格都带 `cooldownSecs: 1800`（30 分钟）与 `dailyCap: 10` ——
 * 用户口径「登录，性能，这些可能频繁触发，注意」✓。界面上可改，`0` = 不设限 ✓。
 */
export const PANEL_THROTTLE_DEFAULTS: Record<keyof PanelBindings, PushThrottle> = {
  loginNew: { mode: 'every', cooldownSecs: 1800, dailyCap: 10 },
  loginFailed: { mode: 'every', cooldownSecs: 1800, dailyCap: 10 },
  perfSlow: { mode: 'onChange', cooldownSecs: 1800, dailyCap: 10 },
  perfFailStreak: { mode: 'onChange', cooldownSecs: 1800, dailyCap: 10 },
}

/**
 * 四个面板行为的**显示顺序与文案**。
 *
 * ⚠ 顺序固定（登录新设备 → 登录失败 → 耗时异常 → 连续失败 ✓）：
 * 它是界面上的行序，读起来才是"登录两件、性能两件" ✓。
 * 标签的权威仍是 `GET /api/push/events`（按 `kind === 'panel'` 取 ✓），
 * 这里只在后端没给那一项时兜底 ✓。
 */
export const PANEL_BEHAVIOR_KEYS = [
  'loginNew',
  'loginFailed',
  'perfSlow',
  'perfFailStreak',
] as const satisfies readonly (keyof PanelBindings)[]

/**
 * 四行的**兜底文案**（`GET /api/push/events` 里没给那一项时才用 ✓）。
 *
 * ⚠ 只是兜底：标签的权威是后端（`kind === 'panel'` 那几条 ✓）——
 * 后端改了措辞，界面跟着改 ✓。字面量照后端 `PushEvent::label()` 抄 ✓
 *（`loginNew` 是「WebUI 新登录」、`loginFailed` 是「登录连续失败」✓）——
 * 两边写不一样的措辞，用户会在"推送"与"设置"两页看到两个名字 ✗。
 */
export const PANEL_BEHAVIOR_LABEL: Record<keyof PanelBindings, string> = {
  loginNew: 'WebUI 新登录',
  loginFailed: '登录连续失败',
  perfSlow: '耗时异常',
  perfFailStreak: '连续失败',
}

/** 空绑定 = 这一行不推（`rules: []` ✓，节流用保守默认 ✓，两格阈值留空 = 跟随内置默认 ✓）。 */
export const emptyPanelBindings = (): PanelBindings => ({
  loginNew: { rules: [], throttle: { ...PANEL_THROTTLE_DEFAULTS.loginNew }, slowFactor: null, failStreak: null },
  loginFailed: { rules: [], throttle: { ...PANEL_THROTTLE_DEFAULTS.loginFailed }, slowFactor: null, failStreak: null },
  perfSlow: { rules: [], throttle: { ...PANEL_THROTTLE_DEFAULTS.perfSlow }, slowFactor: null, failStreak: null },
  perfFailStreak: { rules: [], throttle: { ...PANEL_THROTTLE_DEFAULTS.perfFailStreak }, slowFactor: null, failStreak: null },
})

/** 当前用户的推送规则列表。 */
export const listPushRules = () => request<PushRule[]>('/api/push/rules')

/**
 * 建或改一条规则（id 在路径里，幂等）。
 *
 * 返回值是**落盘后回读的那一份** —— 调用方应当拿它回填，而不是信自己发出去的那份。
 */
export const savePushRule = (id: string, input: PushRuleInput) =>
  request<PushRule>(`/api/push/rules/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })

/**
 * 删一条规则。
 *
 * ⚠ 别人的 id 与不存在的 id **一样回 404**（后端不区分，免得泄露存在性）——
 * 界面不能把 404 说成"网络错误"。
 *
 * ⚠ 删掉之后，**绑着它的行为只是少了一条规则** ✓：定时 / 编排的 `pushRules` 里那个
 * 悬空 id **不会被清掉**（没有级联删除 ✗），面板绑定的 `rules` 同理 ⇒ 界面上要能
 * 看出这一点（见 `schedules-page.tsx` / `push-rule-select.tsx` 的"规则已删除"兜底项 ✓）。
 */
export const deletePushRule = (id: string) =>
  request<{ ok: boolean }>(`/api/push/rules/${encodeURIComponent(id)}`, { method: 'DELETE' })

/** 事件清单（按 `kind` 分两处用：`run` 进定时/编排、`panel` 进设置页那块 ✓）。 */
export const listPushEvents = () => request<PushEventOption[]>('/api/push/events')

/**
 * 渠道清单。
 *
 * ⚠ **新设计里界面不再需要它**：规则**自带凭据**（`PushRule.config` ✓），
 * 平台上不上由 `config` 里那个键在不在决定 —— 不再有"这条规则只用哪几个渠道"那一格 ✗。
 * 接口留着是因为它是后端 `PushChannelKey::ALL` 的读口（§2 冻结的接口 ✓），
 * 前端不抄第二份名单 ✓。
 */
export const listPushChannels = () => request<PushChannelOption[]>('/api/push/channels')

/**
 * 四个**面板行为**的绑定（登录新设备 / 登录失败 / 耗时异常 / 连续失败 ✓）。
 *
 * ⚠ 这四类**没有定时可绑**，入口在「设置」页的「事件推送」卡里 ✓（用户口径 ✓）。
 */
export const getPushPanel = () => request<PanelBindings>('/api/push/panel')

/**
 * **整份替换**四个面板行为的绑定。
 *
 * 返回值是落盘后回读的那一份 —— 调用方应当拿它**回填界面**（后端可能做过归一化 ✓）。
 */
export const putPushPanel = (bindings: PanelBindings) =>
  request<PanelBindings>('/api/push/panel', {
    method: 'PUT',
    body: JSON.stringify(bindings),
  })

// ⚠ `POST /api/push/test`（试发）**刻意不包一层函数**：后端这一版明确回
// `501 Not Implemented`（发送那条线还没接过来，见 `routes/push.rs::test_send` ✓）。
// 包一个 `testPush()` 出来，界面上迟早会有人接一个"测试发送"按钮 ——
// 而用户点它只会拿到一句报错 ✗。要做就等后端接线之后一起做 ✓。

// ── 直播口令 → 小红花 ─────────────────────────────────────────────────
//
// 这一族**没有专属端点**：它就是一条普通任务，走 `POST /api/runs` 与那条 SSE。
// 所以这里**刻意只放类型与纯函数**，不包一层 `runLiveRoom()` ——
// 多了那层包装，页面就分不清"我调的是通用运行"还是"某个私有接口"，
// 而后端换端点时两处都要改。

/** 直播口令任务的**唯一**能力名（与后端 `Task::name()` / `/api/capabilities` 一致）。 */
export const LIVE_ROOM_TASK = 'live-room'

/**
 * 一次直播运行的参数，与后端 `LiveRoomParams` 一一对应。
 *
 * ⚠ 键名是后端 `LiveRoomRun::params_from_body` 认的那四个（**驼峰**，
 * 与 Rust 侧的 `listen_seconds` 不同名）—— 按 Rust 字段名发过去会被静默忽略、
 * 退回默认值，而界面上显示的数字还是用户填的那个。
 */
export interface LiveRoomParams {
  /**
   * 手工口令。后端同时接受"一栏自由文本"与字符串数组，
   * 这里给数组（拆分交给 [`splitKouling`]，它必须与后端 `split_kouling` 同口径）。
   */
  codes: string[]
  /** 自动收集时听弹幕的时长（秒）。**必须 > 0**。 */
  listenSeconds: number
  /** 只用给定口令（不开弹幕）。为真时 `codes` 必须非空。 */
  skipKouling: boolean
}

/**
 * 口令分隔符 —— 换行 / 回车 / 逗号 / 顿号 / **空白**。
 *
 * # ⚠ 与后端 `split_kouling` 的关系：前端是它的**超集**
 *
 * 后端按 `['\n','\r',',','，','、']` 拆。而这一页的输入框与需求都写着
 * "多条用**空格**、换行或逗号分隔"，所以前端**多**拆一个空白。
 *
 * 这不是口径漂移，而是**故意的**：把 `"A B"` 当成一条口令发下去，
 * 后端会原样拿它去领 —— 它必然回 `200108 无效口令`，而用户看到的是
 * "我明明填了两条，怎么只领了一条、还说无效"。前端先拆开，
 * 后端拿到的就是两条正常口令。
 *
 * 反过来（后端拆得比前端多）才是危险的，而那种情况不存在。
 * 顿号/空白都用 `\s` 之外的显式列举，避免把全角空格等意外卷进来。
 */
export function splitKouling(raw: string): string[] {
  const out: string[] = []
  for (const field of raw.split(/[\s,，、]+/)) {
    const code = field.trim()
    if (code === '' || out.includes(code)) continue
    out.push(code)
  }
  return out
}

// ── 结果的结构化明细（后端 `RedFlowerReport`）──────────────────────────
//
// ⚠ 这些字段**还不是后端当前的对外形状**：今天 `RunRecord.details` 只有
// `artifacts` 一格，小红花的报告**只以一句话**（`summary_line()`）出现在
// outcome 里。下面是"后端把报告整体塞进 `details` 之后"的读法 ——
// 页面在字段缺失时**逐段降级**，不会因为多一层而整块空掉。
//
// 这样写而不是写死一句话，是因为两个判据只能从这里拿：
//   - 「口令已失效（本场已结束）」与「无效口令」**必须分开说**；
//   - 「进账多少朵」要用 ⑥−① 的读数，不能把口令的 `flowerNum` 加起来
//     （那会漏掉③首次参与奖那几朵）。

/**
 * 上游对一条口令的判定。
 *
 * ⚠ `kouling_expired` 与 `invalid` **不是一回事**，界面上必须分开显示：
 * 前者是"口令认识、但本场直播已经结束了"，后者是"这条口令不认识（多半抄错了）"。
 * 把它们合并成一句"口令错误"会把排查方向指向**完全错误**的一边。
 */
export type LiveRoomDrawKind =
  | 'succeeded'
  | 'already_redeemed'
  | 'kouling_expired'
  | 'invalid'
  | 'other'

/** 逐口令的一次领取记录。 */
export interface LiveRoomDrawRecord {
  code: string
  /** 上游的判定类别。见 [`LiveRoomDrawKind`]。 */
  kind: LiveRoomDrawKind
  /** 成功时的花朵数。 */
  flowerNum?: number
  /** 奖品名（成功时上游会给）。 */
  prizeName?: string
}

/** 一份小红花报告。字段名与后端 `RedFlowerReport` 的导出形状对应。 */
export interface LiveRoomReport {
  balanceBefore?: number
  balanceAfter?: number
  /** 本次进账（⑥−①）。后端给了就用后端的。 */
  gained?: number
  draws?: LiveRoomDrawRecord[]
  invalidCodes?: string[]
  expiredCodes?: string[]
  otherCodes?: string[]
  /** 这一趟实际拿去领取的口令（保序）。 */
  codes?: string[]
}

/** 全部"没领"的原因 —— 不同原因**下一步完全不同**，所以是枚举而不是布尔。 */
export type LiveRoomIdleReason =
  /** 今天没有直播（一间都没在播）。 */
  | 'no-live'
  /** 有 N 间在播，但没有一间是「中国移动云盘」。 */
  | 'other-anchor-live'
  /** 门开了但没收到口令。 */
  | 'no-codes'
  /** 说不清（既没有领取记录，也没有可判别的措辞）。 */
  | 'unknown'

/**
 * 读一趟的结果。
 *
 * ⚠⚠ **"没领"不是失败**。直播这条链最常见的结局恰恰是"今天没开播"，
 * 把它染红等于让用户每天看一次假警 —— 与签到里"今天已经签过了"同一档。
 * 所以这里把 `idle` 单独归一类，颜色由页面查表，不在类型里定。
 */
export interface LiveRoomRunView {
  /**
   * 结论：
   * - `idle`     没领（门没开 / 没收到口令）—— **正常**；
   * - `success`  领了且全成；
   * - `partial`  领了、成了一部分；
   * - `failed`   领了、一个都没成。
   */
  kind: 'idle' | 'success' | 'partial' | 'failed'
  /** 一句人话（后端原文或本地合成）。 */
  text: string
  /** `idle` 时"为什么没领"；其余为 `null`。 */
  idle: LiveRoomIdleReason | null
  /** 本次进账几朵；读不到时 `null`（**不是 0** —— 0 是一句断言）。 */
  gained: number | null
  /** 终值/起始余额，读不到时 `null`。 */
  balanceAfter: number | null
  balanceBefore: number | null
  /** 五类口令结果，分开列。 */
  succeeded: LiveRoomDrawRecord[]
  redeemed: LiveRoomDrawRecord[]
  expired: LiveRoomDrawRecord[]
  invalid: LiveRoomDrawRecord[]
  other: LiveRoomDrawRecord[]
  /** 实际拿去领取的口令数。 */
  attempted: number
}

/**
 * `TaskOutcome` 的结论文本（与 `ui/history.ts` 同一口径）。
 *
 * 返回 `undefined` 时由调用方兜底 —— 后端三个分支各自带 `summary` / `reason`，
 * 形状对不上时这里会取到 `undefined`。
 */
const outcomeTextOf = (o: TaskOutcome): string | undefined =>
  o.status === 'success' || o.status === 'partial' ? o.summary : o.reason

/** 后端小结里"进账 N"那一格 —— 把报告压成一句话时的**唯一**可用来源。 */
const GAINED_IN_TEXT = /进账\s*(-?\d+)/

/** "成功 1/2" 那一格 —— 只取**分子**（成了几条）。 */
const SUCCESS_IN_TEXT = /成功\s*(\d+)\s*\/\s*\d+/

/** "成功 1/2" 那一格 —— 只取**分母**（一共领了几条）。 */
const DRAWN_IN_TEXT = /成功\s*\d+\s*\/\s*(\d+)/

/** 后端 `notes_suffix` 里那三类口令标签 —— **原样**，用于在文本里定位。 */
const EXPIRED_LABEL = '口令已失效（本场已结束）'
const INVALID_LABEL = '无效口令'
const OTHER_LABEL = '未分类口令'

/**
 * 从一句话里读出某个标签后面跟着的那串口令。
 *
 * ⚠⚠ 这里**不能**用"读到 `）` 就停"那种写法：`口令已失效（本场已结束）AAAA`
 * 这个标签**自己就带一对括号**，一读到 `）` 就停会把它截成
 * `（本场已结束` —— 于是界面上"口令"那一栏显示的是一个括号片段，
 * 而真正的口令 `AAAA` 被吞掉。这是我实测踩到的（第一版就是这么写的）。
 *
 * 正确做法是拿**已知的标签与分隔符**当边界：
 * 到下一个 `；`、或者下一个已知标签、或者字符串结尾为止。
 */
const codesAfterLabel = (text: string, label: string): string[] => {
  const at = text.indexOf(label)
  if (at < 0) return []
  let tail = text.slice(at + label.length)

  // 到 `；` 为止（`notes_suffix` 用 `；` 连接三段）
  const semi = tail.indexOf('；')
  if (semi >= 0) tail = tail.slice(0, semi)

  // 到下一个已知标签为止（它们可能不带 `；`，例如最后一段以 `）` 收尾）
  for (const other of [EXPIRED_LABEL, INVALID_LABEL, OTHER_LABEL, '首次参与', '复活花']) {
    if (other === label) continue
    const next = tail.indexOf(other)
    if (next >= 0) tail = tail.slice(0, next)
  }

  // 去掉尾巴上的收尾括号与空白
  tail = tail.replace(/[）)]+\s*$/, '')

  return tail
    .split('/')
    .map((v) => v.trim())
    .filter((v) => v !== '')
}

/**
 * 把一次运行的记录翻成页面要看的视图。
 *
 * # 为什么判据**有一部分**落在文本上
 *
 * 后端今天只把结论压成一句话（`RedFlowerReport::summary_line()`），
 * 结构化字段还没进 `details`。要界面上做到"四类口令分开显示、没领不染红"，
 * 就只能先读那句话 —— 而这**不是猜措辞**：那几句是后端写死的模板
 * （`无效口令 …` / `口令已失效（本场已结束）…` / `未分类口令 …` / `进账 N`），
 * 后端的注释里也明确说了它们是**用户可见的**。
 *
 * `details.liveRoom` 一旦落地则**结构化字段优先**，文本只作兜底 ——
 * 所以这里每一步都是"有结构化的用结构化的，没有才读文本"。
 */
export function toLiveRoomRunView(
  record: RunRecord | null,
  outcome?: TaskOutcome,
): LiveRoomRunView {
  const o = outcome ?? record?.outcome
  // ⚠⚠ `outcomeTextOf` 必须容错。后端的 `failed` / `skipped` 分支带的是 `reason`、
  // `success` 带的是 `summary`；一旦形状对不上（旧后端、字段改名、
  // 或者中间有人转发时丢了字段），`o.summary` 就是 `undefined`，
  // 紧接着 `.match(...)` 抛 TypeError。
  //
  // 这里抛错的**代价特别大**：`onEvent` 的 `finished` 分支在 `finish()` 之前
  // 就断了，于是 `running` 永远为真 —— 页面**永久停在"执行中"**、
  // 日志里没有任何终局行、按钮也不再恢复。用户唯一的出路是刷新页面。
  // 所以兜底成空串，让这一趟至少能正常收尾（结论退化成"这一趟没有拿去领取的口令"）。
  const text = o ? (outcomeTextOf(o) ?? '') : ''
  const details = (record?.details ?? null) as { liveRoom?: LiveRoomReport } | null
  const report = details?.liveRoom ?? null

  const gainedFromText = (() => {
    const m = text.match(GAINED_IN_TEXT)
    return m ? Number(m[1]) : null
  })()
  // 余额那一对从"小红花 235 → 245（进账 10…）"里读
  const pair = text.match(/(\d+)\s*→\s*(\d+)/)
  const balanceBefore = report?.balanceBefore ?? (pair ? Number(pair[1]) : null)
  const balanceAfter = report?.balanceAfter ?? (pair ? Number(pair[2]) : null)
  const gained = report?.gained ?? gainedFromText

  const draws = report?.draws ?? []
  const bucket = (kind: LiveRoomDrawKind): LiveRoomDrawRecord[] =>
    draws.filter((d) => d.kind === kind)

  const succeeded = bucket('succeeded')
  const redeemed = bucket('already_redeemed')

  // ⚠ 这三类是**分开**的，任何一步都不许合并：
  //   `expired`  = 口令对、但本场已结束  → 排查方向「直播结束了」
  //   `invalid`  = 口令不认识（多半抄错）→ 排查方向「口令来源对不对」
  //   `other`    = 上游回了个没分类的码  → 排查方向「上游改了什么」
  const expired =
    report?.expiredCodes != null
      ? report.expiredCodes.map((code) => ({ code, kind: 'kouling_expired' as const }))
      : bucket('kouling_expired')
  const invalid =
    report?.invalidCodes != null
      ? report.invalidCodes.map((code) => ({ code, kind: 'invalid' as const }))
      : bucket('invalid')
  const other =
    report?.otherCodes != null
      ? report.otherCodes.map((code) => ({ code, kind: 'other' as const }))
      : bucket('other')

  // 文本兜底：三类各有自己的固定标签，**按标签分别取**，不混成一个列表。
  const expiredFinal =
    expired.length > 0
      ? expired
      : codesAfterLabel(text, EXPIRED_LABEL).map((code) => ({
          code,
          kind: 'kouling_expired' as const,
        }))
  const invalidFinal =
    invalid.length > 0
      ? invalid
      : codesAfterLabel(text, INVALID_LABEL).map((code) => ({ code, kind: 'invalid' as const }))
  const otherFinal =
    other.length > 0
      ? other
      : codesAfterLabel(text, OTHER_LABEL).map((code) => ({ code, kind: 'other' as const }))

  const attempted = report != null
    ? draws.length
    : (() => {
        const m = text.match(DRAWN_IN_TEXT)
        return m ? Number(m[1]) : 0
      })()

  const base = {
    text,
    gained,
    balanceAfter,
    balanceBefore,
    succeeded,
    redeemed,
    expired: expiredFinal,
    invalid: invalidFinal,
    other: otherFinal,
    attempted,
  }

  // ── 结论分类 ──
  //
  // ⚠ 顺序要紧。**先认"没领"**，再认失败与成功：
  // 后端在"没领"时回的是 **success**（见 `LiveRoomAccountResult::outcome`），
  // 所以"成功=绿"这条映射对这条链是**错的**，光看 `outcome.status` 会把
  // "今天没开播"显示成一个绿色的成功。
  // ⚠ 这里从前还认一句"只获取不领取"（对应已删的伪需求 `koulingOnly`）——
  // 它是**从中文文案里 substring 推出来的**，所以文案一删它就自然失效、
  // 留着只会是一条永远匹配不上的死分支。
  const idle: LiveRoomIdleReason | null = text.includes('今天没有直播')
    ? 'no-live'
    : /有\s*\d+\s*间在播/.test(text)
      ? 'other-anchor-live'
      : text.includes('没收到口令')
        ? 'no-codes'
        : null

  if (o?.status === 'skipped') {
    return { kind: 'idle', idle: 'unknown', ...base }
  }
  if (idle != null) {
    return { kind: 'idle', idle, ...base }
  }
  if (o?.status === 'failed') {
    return { kind: 'failed', idle: null, ...base }
  }
  if (attempted === 0) {
    // 既没有"没领"的措辞、也没有领取记录：说不清。归 idle 而**不是** failed ——
    // 判成失败会制造一条假警，而"没领"才是这条链的常态。
    return { kind: 'idle', idle: 'unknown', ...base }
  }

  /**
   * 成了几条。
   *
   * ⚠⚠ 必须**优先读文本里那个"成功 s/t"**，而不是只看结构化的 `succeeded`。
   *
   * 后端今天把报告压成一句话，`details.liveRoom` 还没落地 —— 那时
   * `succeeded` 是**空数组**，而空数组不等于"零条成功"。
   * 只看它的话，一句 `成功 1/3` 会被判成 `failed`（0 条成功），
   * 于是界面上写着「失败」、旁边却写着「进账 +15 朵」——
   * 自相矛盾，而且把一次**部分成功**误报成失败。这是我实测踩到的。
   */
  const successFromText = (() => {
    const m = text.match(SUCCESS_IN_TEXT)
    return m ? Number(m[1]) : null
  })()
  const okCount =
    successFromText != null
      ? successFromText
      : succeeded.length > 0
        ? succeeded.length
        : redeemed.length

  // 结构化字段没来时，只能靠后端的固定尾巴判「部分成功 / 一个都没成」。
  if (report == null) {
    if (text.includes('一个口令都没成')) return { kind: 'failed', idle: null, ...base }
    if (text.includes('部分成功')) return { kind: 'partial', idle: null, ...base }
  }
  if (okCount === 0 && attempted > 0) return { kind: 'failed', idle: null, ...base }
  if (okCount < attempted) return { kind: 'partial', idle: null, ...base }
  return { kind: 'success', idle: null, ...base }
}

/** "为什么没领"的说法 —— 查表，**不拼字符串**。 */
export const LIVE_IDLE_REASON: Record<LiveRoomIdleReason, string> = {
  'no-live': '今天没有直播 —— 该主播一间都没在播，没有口令可收（等下一场即可，不是失败）',
  'other-anchor-live':
    '有别的房间在播，但没有一间是「中国移动云盘」—— 这一场不会再有口令了，不用等',
  'no-codes': '门开了，但这一趟没收到口令 —— 弹幕里没人发，小红书那边暂时也没读到',
  unknown: '这一趟没有拿去领取的口令',
}

// ── 兑换（抢兑）──────────────────────────────────────────────────────
//
// ⚠ 这一族与上面的直播/签到**不是**同一条链：清单与取题是只读的，
// 而**兑换是真花豆的写操作** —— 界面必须二次确认，且不可兑的档位要挡住。

/** 清单里的一件奖品（后端按判据算好档位再发过来）。 */
export interface ExchangePrizeView {
  prizeId: number
  name: string | null
  /** 兑换所需AI豆数。`null` = 上游没给这一格（不是 0）。 */
  price: number | null
  groupId: number | null
  /**
   * 判据档位（`plan_exchange` 的 slug）。
   *
   * ⚠ 只读那条路由**默认带余额读数**（`balanceAware: true`），所以
   * 「余额不足」这一档判得出来。只有**那次余额查询失败**时才退回无余额口径
   * （`balanceAware: false` + 一句 `balanceNote`）。
   */
  plan: string
  /** 为什么不能兑（判据本身的中文复述）。`blocks` 为真时才有意义。 */
  planReason: string
  /** 判据没过 ⇒ 界面上的按钮要 disabled。 */
  blocks: boolean
  limit: number | null
  count: number | null
  /**
   * 上游 `totalCount`：这一场活动的**总投放量**。
   *
   * ⚠ 与 `count`（当前剩余）配起来才是"现量/总量"。**别**把 `count` 单独叫"现量"
   * 再和"已抢光"并排显示：2026-10-01 实拍 `251230032` 是 `count = 43,739` 而
   * `minRemainderCount = 0`（H5 判抢光）—— 池子里还有货、**今日份额**没了。
   */
  totalCount: number | null
  /** 上游 `dailyCount`：**今日投放量**（配 `dailyRemainderCount` = 今日还剩几份）。 */
  dailyCount: number | null
  dailyRemainderCount: number | null
  minRemainderCount: number | null
  onLine: number | null
  startTime: string | null
  endTime: string | null
  /**
   * 这一件所属**活动/品类组**的名字，取自另一条静态接口 `prizeGroup.json`。
   *
   * 组是"共享状态"的单位（同组每月共用一个名额 ⇒ `limit == 4`），
   * 而组名是**上游给的**（实拍"音乐类会员""视频类会员"）—— 界面直接显示，别自己编。
   * `null` = 上游没给或那一枪没取到（这时分区标题退回"第 N 组"）。
   */
  groupTitle: string | null
  /** 该组的限兑规则，**上游原话**（实拍 `(月卡每月多选一限兑)`）。 */
  groupSubTitle: string | null
  /**
   * 上游 `quota.monthQuantity`：这个组**每月能兑几件**（实拍音乐类是 `1`）。
   *
   * ⚠ 只用于显示：判据链不读它，上游也没给"本组已用几件"，别拿它算"还剩几个名额"。
   */
  monthQuantity: number | null
  /** 上游 `quota.groupPrizeIds`：与这一件**互斥**的其它 prizeId。`null` = 上游没给。 */
  groupPrizeIds: string[] | null
}

export interface ExchangePrizesResponse {
  prizes: ExchangePrizeView[]
  /**
   * 这一份判据是否把**余额**算进去了。
   *
   * 今天恒为 `false`：清单那条路由**不查** `infoV3`（账号级请求，
   * 多打一枪多一分风控噪声），所以档位里没有「余额不足」这一档。
   */
  /**
   * 这一份判据里**有没有**余额读数。
   *
   * `true` = 读了（常态，后端多打一枪只读 `infoV3`）⇒「余额不足」判得出来。
   * `false` = 那次余额查询**失败**了，退回无余额口径 ⇒ 界面该把 `balanceNote` 显示出来。
   */
  balanceAware: boolean
  /** 读到的余额（AI豆）；`null` = 这次没读到。 */
  balance: number | null
  /** 没读到余额时的人话说明；**读到了就是 `null`**（没问题就不说话）。 */
  balanceNote: string | null
  /**
   * 组名那一枪（`prizeGroup.json`）**拿到了没有**；`null` = 拿到了。
   *
   * `string` 时清单照常显示，只是分区标题退回"第 N 组" —— 组名是文案，
   * 不该因为它读失败就让整份清单失败。原因原样显示（别在前端另写一句）。
   */
  groupsNote: string | null
  /** 后端判据用的时刻（界面要显示"这份清单是什么时候的"）。 */
  nowMs: number
}

/** 读奖品清单 + 每件的判据档位。**只读，不花豆。** */
export function listExchangePrizes(account: string): Promise<ExchangePrizesResponse> {
  return request<ExchangePrizesResponse>(
    `/api/accounts/${encodeURIComponent(account)}/exchange-prizes`,
  )
}

/**
 * 一条订阅：用户钉在兑换页顶部的一件奖品。
 *
 * ⚠ `name` 是**订阅那一刻的快照**（用户口径 2026-10-02：按单个奖品订阅 + 存服务端）——
 * 上游清单会变（下架、换活动、换云盘账号），那时顶部仍要显示得出人看得懂的一行，
 * 而不是一串 `prizeId` ✓。清单里还在时，页面用**实时**名字覆盖它 ✓。
 */
export interface ExchangeSubscriptionView {
  prizeId: number
  name: string
  groupId: number | null
  groupTitle: string | null
  subscribedAtMs: number
}

export interface ExchangeSubscriptionsResponse {
  items: ExchangeSubscriptionView[]
}

/** 我订阅了哪几件（**服务端**存：换浏览器、换设备也还在 ✓）。 */
export function listExchangeSubscriptions(): Promise<ExchangeSubscriptionsResponse> {
  return request<ExchangeSubscriptionsResponse>('/api/exchange/subscriptions')
}

/**
 * 订阅 / 取消订阅一件（**切换**，不是"加"）。
 *
 * ⚠ 判断依据在**服务端**：它手里那份列表才是准的 ⇒ 两个标签页各点一下不会变成
 * "都加了一遍"或"都删掉了" ✗。所以这里只发"哪一件"，回的是**切换之后的整份列表** ✓。
 *
 * 取消时只给 `prizeId` 就够（`name` 可省）：顶部那几行要能一键取消，
 * 而它手上不一定还留着当初的名字 ✓。
 */
export function toggleExchangeSubscription(input: {
  prizeId: number
  name?: string
  groupId?: number | null
  groupTitle?: string | null
}): Promise<ExchangeSubscriptionsResponse> {
  return request<ExchangeSubscriptionsResponse>('/api/exchange/subscriptions/toggle', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

/** 一次兑换的读数（后端从运行记录里读回来的那一份）。 */
export interface ExchangeRunOutcome {
  runId: number
  /** `ordered` / `already-exchanged` / `skipped` / `unknown` / … */
  status: string
  plan: string
  planReason: string
  /** 给人读的一句话小结（后端定的，界面直接用）。 */
  summary: string
  /** 业务码。`0` = 成交。 */
  code: number
  /** 上游那句 `msg`（可能是空串）。 */
  message: string
  rounds: number
  /** 真正发出去了几枪**下单**请求。`0` = 一枪未发。 */
  attempts: number
  /**
   * 本地解出的 `puzzleOffset`。
   *
   * `null` = 没解出来（那时 `attempts` 必为 0：算不出就不发）。
   */
  offset: number | null
  balance: number
  afterBalance: number | null
  order: Record<string, unknown> | null
  deviceIdFailed: string
  error: string
}

export interface ExchangeRequestInput {
  /** `prizeId` 的数字串，或奖品名（后端按精确相等匹配）。 */
  prizeIdOrName: string
  /**
   * 跳过"等到下一个整点再重判"。
   *
   * 界面默认 `true`：用户点了按钮就期望**现在**有结果，
   * 让 HTTP 请求挂在那儿等一小时是不可接受的。
   */
  skipWait?: boolean
}

/**
 * 兑一件奖品。**真花豆。**
 *
 * 后端会走并发闸（同一 `task:account` 不会并发），所以连点两下
 * 第二下会拿到「同一任务正在运行中」而不是扣两次豆。
 */
export function runExchange(
  account: string,
  // ⚠ `force` 不放进 `ExchangeRequestInput`：那是"这次兑哪件、跳不跳等待"的入参，
  // 而 `force` 是"**本地判成不能兑也照样真发下单请求**"的开关（用户口径 2026-10-02：
  // 所有档都放行、点击就是真发请求）。放在这里当附加字段，界面显式传。
  input: ExchangeRequestInput & { force?: boolean; offset?: number },
): Promise<ExchangeRunOutcome> {
  return request<ExchangeRunOutcome>(`/api/accounts/${encodeURIComponent(account)}/exchange`, {
    method: 'POST',
    body: JSON.stringify({
      prizeIdOrName: input.prizeIdOrName,
      skipWait: input.skipWait ?? true,
      force: input.force ?? false,
      // 传输延迟补偿（毫秒）：本地与云端的传输时间差，默认 10（与参考实现同值）。
      offset: input.offset ?? 10,
    }),
  })
}

// ── 直播自动定时（`/api/live-room/auto-schedule`）────────────────────────
//
// 用户口径（2026-10-08）：「为直播增加一个自动设置定时的功能，设置为**下次直播开始
// 时间的一个小时后**」✓。**每个登录用户一条**，落盘就是那条普通定时
// （`data/schedules.json` 里的 `auto-live-<owner>` ✓，所以它**同时**出现在定时页里 ✓）。
//
// ⚠ 这里**只搬后端算好的那一刻**，前端一个字节都不推算 ✗：
// 「下一场」来自共享口令池（`GET /api/kouling` 那份日程 ✓），自己再推一遍
// 至少要重走一次上游鉴权，而且两处判据迟早分叉（上一版分叉的后果用户见过：
// 「16:31 还显示 15:00」✗）。同理**不缓存 `at` 自己推算下一次** ✗ ——
// 想看最新的就 `PUT` 一次（后端**立刻重算**并回新状态 ✓），或等直播任务跑完 ✓。

/**
 * 「自动直播定时」的当前状态（`GET`/`PUT /api/live-room/auto-schedule` 的响应体，
 * 与后端 `mcloud_app::live_auto::AutoLiveState` 一一对应 ✓）。
 */
export interface LiveAutoSchedule {
  /**
   * **现在能不能跑** = 用户想要 **且** 算得出下一场 ✓。
   *
   * ⚠ 它与那条定时自己的 `enabled` **同一格**（卡片上那个开关显示的就是调度器
   * 会看到的那一格 ✓）—— 不是"另一份状态" ✗。
   */
  enabled: boolean
  /**
   * 算出来的触发时刻：**下一场直播开始 + 1 小时**（东八区墙钟串，形如
   * `"2026-10-08 11:00:00"` ✓）。
   *
   * `null` = 现在算不出来（没探测过 / 场次都过去了 / 已被停用 ✓）——
   * ⚠ 界面上必须写清"已停用"，**留空就是坑** ✗。
   */
  at: string | null
  /**
   * 这条定时作用的云盘账号 id（用户多选 ✓）。
   *
   * ⚠ **空数组 = 全部已启用账号** ✓（与 `Schedule.accounts` 同一条语义 ✓）——
   * 不是"一个都不选" ✗。界面上「全部账号」那一行对应的就是这个值 ✓。
   */
  accounts: string[]
  /**
   * 一句人话，说清"为什么现在是这个状态"（界面**原样**显示 ✓）。
   *
   * ⚠ 只在**说不清**的时候才有：正常有下一场时是 `null` ✓
   * （那行时刻已经说明了一切，再挂一句就是噪音 ✗）。
   */
  note: string | null
}

/**
 * `PUT` 的请求体 —— **缺席 = 保持原样** ✓（后端的双 `Option` 语义，见
 * `routes/live_room.rs::AutoLiveInput`）。
 *
 * ⚠ 两格都是可选的，别把它们写成必填：只想改账号时**不带** `enabled` 才对
 *（带 `enabled: false` 会把用户"想要"的意愿一起改掉 ✗，而那是另一件事）。
 */
export interface LiveAutoScheduleInput {
  /** 不给 = 不动用户的意愿 ✓。 */
  enabled?: boolean
  /** 不给 = 不动 ✓；给 `[]` = 清空 ⇒ 回到"全部已启用账号" ✓。 */
  accounts?: string[]
}

/**
 * 读当前状态。**只读、不重算**（后端不写盘 ✓）—— 想看最新时刻就 `PUT` 一次。
 */
export const getLiveAutoSchedule = () =>
  request<LiveAutoSchedule>('/api/live-room/auto-schedule')

/**
 * 改开关 / 账号多选，后端**立刻重算一次**并回**新状态** ✓。
 *
 * ⚠ 返回的就是新状态 ⇒ 界面**必须拿它回填**，别自己猜（自己猜的那份与后端
 * 刚写下去的那份一旦不一致，用户看到的就是"我点了没反应" ✗）。
 * ⚠ 账号不是自己的 ⇒ 400，错误原话要**原样**透出（`request()` 已经保住了 ✓）。
 */
export const saveLiveAutoSchedule = (input: LiveAutoScheduleInput) =>
  request<LiveAutoSchedule>('/api/live-room/auto-schedule', {
    method: 'PUT',
    body: JSON.stringify(input),
  })
