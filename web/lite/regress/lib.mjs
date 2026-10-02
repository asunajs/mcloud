/**
 * 验收脚本的**共同底座**：fixture 服务、Chrome 路径、DOM 归一化、CDP 会话。
 *
 * * `serve` —— 一份 fixture 后端 + 静态产物，端口随脚本给；
 * * `openSession` —— 起 chrome 并接上 **CDP**（DevTools 协议，零依赖：Node 自带
 *   `WebSocket`），能点、能输入、能等 DOM 稳定，用于回答"点完这一下，界面对不对"。
 *
 * ⚠ `appSubtree` 是"什么算渲染结果"的唯一定义（去注释锚点、折叠标签间空白）：
 * 每步快照都过它，于是"界面变了没有"有一个稳定口径，而不是各处自己剪一段 outerHTML。
 */
import { execFileSync, spawn } from 'node:child_process'
import { createHash } from 'node:crypto'

/**
 * 与 Web 侧 `ui/password.ts`、Rust 侧 `client_password_digest` **同一套值**。
 * 用它来断言"前端那一次哈希真的发生了"（见下面登录/建管理员两条 handler）。
 */
const sha256Hex = (v) => createHash('sha256').update(v, 'utf8').digest('hex')
import fs from 'node:fs'
import http from 'node:http'
import path from 'node:path'

export const arg = (name, fallback) => {
  const i = process.argv.indexOf(name)
  return i > 0 ? process.argv[i + 1] : fallback
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/**
 * fixture 数据。⚠ **形状照 `web/src/api.ts` 的类型写**，不是随手编的：
 * 上一版把 `RunRecord` 写成了 `{startedAt, finishedAt}`（真实字段是 `started_at_ms` /
 * `duration_ms`），于是仪表盘上显示 `耗时NaN分NaN秒` —— 界面照样"跑通了"，
 * 但那是在**测一个空壳**。列表为空时各页只渲染空态，`createFor` 的增删重排、
 * 带数据的弹窗、带选项的下拉框一个都走不到。
 */
const ACCOUNT = { id: '13800000000', nickname: '主力号', pc_platform: 'windows', device_id: 'dev-abc123', expire: 1790000000000, enabled: true }
const ACCOUNT2 = { id: '13900000001', nickname: null, pc_platform: null, device_id: 'dev-def456', expire: 0, enabled: true }

/**
 * 账号在 ready 变体里是**有状态**的（停用之后别处就该看不到它）。
 * `serve()` 每次启动时重置 —— 否则同一次进程里起两个服务会串状态。
 */
const acctState = { accounts: [] }

/**
 * 兑换页的**订阅**（有状态）。
 *
 * 与 `acctState` 同一个理由：这个功能的价值全在"点星星 → 服务端记住 → 顶部那块出现"
 * —— 回一份静态值就只能证明"按钮点了有反应" ✗，证明不了用户要的那件事 ✓。
 *
 * ⚠ 初值**非空**：顶部那块"有内容"的样子才是要验收的（门禁会点星星加一件、
 * 再从顶部取消一件，两边的数都要跟着动 ✓）。
 */
const subState = {
  items: [
    {
      prizeId: 251230053,
      name: '移动云盘100万tokens叠加包',
      groupId: 1,
      groupTitle: 'AI豆兑换',
      subscribedAtMs: 1_790_000_000_000,
    },
  ],
}

/**
 * 任务的默认参数（任务配置页存的那份）。
 *
 * 有状态是**故意**的：这个功能的价值全在"配置页存了 → 任务页运行时带上"，
 * 回一份静态值就只证明"按钮点了有反应"。
 */
const taskCfg = {}
const resetTaskCfg = () => {
  // ⚠ `daily-checkin` **没有参数**了（2026-10-02 删掉 `dryRun`「试运行」）。
  taskCfg['daily-checkin'] = {}
  taskCfg['live-room'] = { listenSeconds: 60, codes: '' }
}
const SPECS = {
  'daily-checkin': [
    // 这条任务现在没有任何参数（删掉了 dryRun「试运行」）。
    // 装置里留空数组是有意的：正好让门禁盯住"没有参数的任务，配置页也要能打开"。
  ],
  'live-room': [
    { name: 'listenSeconds', title: '听弹幕时长', kind: 'number', default: 60, min: 5, max: 1800, unit: '秒', help: '听多久弹幕来抓口令' },
    { name: 'codes', title: '口令', kind: 'text', default: '', help: '多个用逗号或换行分隔' },
  ],
}
const configView = (name) => ({ name, specs: SPECS[name] ?? [], params: taskCfg[name] ?? {} })
const resetAcctState = () => {
  acctState.accounts = [ACCOUNT, ACCOUNT2].map((a) => ({ ...a }))
}
const RUN = {
  run_id: 1024,
  task: 'daily-checkin',
  account: '138****0000',
  started_at_ms: 1790581000000,
  duration_ms: 9200,
  outcome: { status: 'success', summary: '打卡 3 个账号' },
  details: { artifacts: [{ kind: 'cloud-file', account: '138****0000', id: 'file-7f3a' }] },
}
const SCHEDULE = {
  id: 'sch-1',
  name: '每天签到',
  task: 'daily-checkin',
  accounts: [ACCOUNT.id],
  expr: '0 8 * * *',
  timezone: 'Asia/Shanghai',
  body: {},
  enabled: true,
  lastFireMs: 1790581000000,
  lastRunMs: 1790581009200,
  lastStatus: 'ok',
  lastError: null,
  runCount: 12,
  nextFireAt: 1790667400000,
}
const PIPELINE = {
  id: 'pipe-1',
  name: '早班',
  accounts: [ACCOUNT.id, ACCOUNT2.id],
  accountMode: 'sequential',
  stopOnError: false,
  steps: [
    { task: 'daily-checkin', continueOnError: false },
    { task: 'live-room', body: { listenSeconds: 30 }, continueOnError: true },
  ],
  enabled: true,
  lastRunMs: 1790582000000,
}

/**
 * 启动链路要的 4 个接口 + 首屏几个列表；三个 variant 共用同一份。
 *
 * ⚠ **每页要的接口都得在这儿**，少一个的表现不是"那一块空着"而是**整页错误态**：
 * `accounts-page.tsx` 的 `load()` 是 `Promise.all([listAccounts(), getPcDevice()])`，
 * 于是 `/api/login/device` 缺失时账号列表一行都不渲染（先前就是这样 —— 点「移除」
 * 命中的其实是弹窗里那颗按钮，四条断言一起红）。
 * 加页面时先把它 `load()` 里并发取的端点全列进这张表。
 */
/**
 * 兑换清单。**两个账号共用同一份** —— 上游是按活动投放的，与账号无关 ✓。
 *
 * ⚠ 为什么要给**两个**账号都配上：门禁中途会**停用主力号**，兑换页随之切到另一个
 * 账号 —— 只配一个账号的话，那一页会静默变成「没有奖品」，于是**所有兑换断言都在
 * 空转** ✗（本仓在「装置测不到 = 没测」上栽过：门禁全绿而功能是坏的）。
 */
const EXCHANGE_PRIZES = {
  prizes: [
    {
      prizeId: 251230053,
      name: '移动云盘100万tokens叠加包',
      price: 50,
      groupId: 1,
      plan: 'go',
      planReason: '可以下单',
      blocks: false,
      limit: 1,
      count: 399210,
      totalCount: 400000,
      dailyCount: 5000,
      dailyRemainderCount: 5,
      minRemainderCount: 1,
      onLine: 1,
      startTime: null,
      endTime: null,
      groupTitle: 'AI豆兑换',
      groupSubTitle: '(月卡每月多选一限兑)',
      monthQuantity: 1,
      groupPrizeIds: null,
    },
    {
      prizeId: 251230054,
      name: '哔哩哔哩会员月卡',
      price: 100,
      groupId: 1,
      plan: 'min_remainder_zero',
      planReason: '这件的余量阈值已经是 0（H5 在这一档显示「奖品被抢光啦」）',
      blocks: true,
      limit: 1,
      count: 43739,
      totalCount: 100000,
      dailyCount: 100,
      dailyRemainderCount: 0,
      minRemainderCount: 0,
      onLine: 1,
      startTime: null,
      endTime: null,
      groupTitle: 'AI豆兑换',
      groupSubTitle: '(月卡每月多选一限兑)',
      monthQuantity: 1,
      groupPrizeIds: null,
    },
  ],
}

export const FIXTURES = {
  // `passwordScheme` 决定登录页发摘要还是发明文（见 `ui/password.ts`）
  '/api/setup': { initialized: true, minPasswordLen: 8, passwordScheme: 'sha256' },
  '/api/session': { userId: 'u-1', name: 'admin', kind: 'web' },
  // 账户安全（改口令 / 策略）与存储信息：设置页那两块要它们
  '/api/admin/security': { minPasswordLen: 8, sessionTtlHours: 168, passwordScheme: 'sha256' },
  '/api/system/storage': {
    dataDir: '/tmp/mcloud-fixture/data',
    logDir: '/tmp/mcloud-fixture/data/logs',
    binary: '/tmp/mcloud-fixture/target/release/mcloud-server',
    dataBytes: 20480,
    logBytes: 1024,
    files: [
      { name: 'accounts.json', bytes: 4096 },
      { name: 'users.json', bytes: 512 },
    ],
  },
  '/api/version': { name: 'mcloud', version: '0.1.0' },
  // 「关于」卡片（构建期从 git 抓的那几项）
  '/api/system/about': {
    name: 'mcloud',
    version: '0.1.0',
    commit: 'abc1234',
    dirty: false,
    buildTime: '2026-10-01 19:20 CST',
    profile: 'release',
    changes: [
      { hash: 'abc1234', subject: 'feat(accounts): 账号停用' },
      { hash: 'def5678', subject: 'feat(web): 设置页重做' },
    ],
  },
  // `startedAtMs` 是「运行时间」的来源（总览那条引擎条）：给一个**固定**的两天前，
  // 界面就会稳定显示「2 天」—— 用 `Date.now()` 的话每次跑门禁的渲染结果都不一样。
  '/api/status': { name: 'mcloud', version: '0.1.0', startedAtMs: 1790581000000, taskCount: 18, lastRun: RUN, schedulerRunning: true, runningCount: 0, scheduleCount: 1, pipelineCount: 1 },
  '/api/capabilities': {
    tasks: [
      {
        name: 'daily-checkin', title: '每日签到', description: '签到并领取当日奖励',
        hidden: false, group: 'signin', groupLabel: 'AI豆中心',
        params: [],
      },
      {
        name: 'receive', title: '领取AI豆', description: '领取待领的AI豆',
        hidden: false, group: 'signin', groupLabel: 'AI豆中心', params: [],
      },
      {
        name: 'live-room', title: '直播口令', description: '听弹幕领小红花',
        hidden: false, group: 'live', groupLabel: '直播小红花',
        params: [
          { name: 'listenSeconds', title: '听弹幕时长', kind: 'number', default: 60, min: 5, max: 1800, unit: '秒', help: '听多久弹幕来抓口令' },
          { name: 'codes', title: '口令', kind: 'text', default: '', help: '多个用逗号或换行分隔' },
        ],
      },
      {
        name: 'msg-push', title: '消息推送', description: '把运行结果推到你配的渠道',
        hidden: false, group: 'device', groupLabel: '消息与设备', params: [],
      },
      {
        name: 'internal-probe', title: '内部探针', description: '不该出现在界面上',
        hidden: true, group: 'app', groupLabel: '应用与 AI', params: [],
      },
    ],
  },
  /**
   * 共享口令池：装置里放**两条有效**（正是"够了、别再抓"的那条判据），
   * 外加一条已过期 —— 界面必须同时显示"够 2 条"与"过期视同无效"。
   */
  '/api/kouling': {
    cycleAtMs: 1_790_000_000_000,
    validCount: 2,
    stopScraping: true,
    codes: [
      { value: '中秋口令甲', state: 'valid', source: 'danmaku', reason: '已验证有效', firstSeenMs: 1_790_000_000_000, validAtMs: 1_790_000_010_000 },
      { value: '中秋口令乙', state: 'valid', source: 'xiaohongshu', reason: '已验证有效（这个账号已领过）', firstSeenMs: 1_790_000_020_000, validAtMs: 1_790_000_030_000 },
      { value: '过期的老口令', state: 'invalid', source: 'manual', reason: '口令已过期', firstSeenMs: 1_790_000_040_000, validAtMs: null },
    ],
    nextStart: '2026-09-23 14:30:00',
    // ⚠ 主播名与场次名要跟时间**同一次选择**给（用户口径 2026-10-02）：
    // 只写"09-23 14:30"分不出是不是自己那个直播间 —— 上游列表里混着别的主播 ✓。
    nextAnchor: '中国移动云盘',
    nextTitle: '云盘AI助职场加速！看直播赢好礼！',
    scheduleAtMs: 1_790_000_050_000,
    sessions: [
      { startedAtMs: 1_790_000_060_000, endedAtMs: null },
      { startedAtMs: 1_789_960_000_000, endedAtMs: 1_789_963_600_000 },
    ],
  },
  '/api/runs': [
    RUN,
    { ...RUN, run_id: 1023, task: 'live-room', duration_ms: 61000, outcome: { status: 'failed', reason: '口令无效' }, details: null },
    {
      // ⭐ 兑换那一次的**结构化明细**：后端 `detail_payload` 把字段**铺平**在
      // `details` 上（不像 liveRoom 那样套一层），用 `kind: "exchange"` 标识自己。
      // 历史页要把它**全部**渲染出来（用户口径：所有内容都要进记录）。
      ...RUN,
      run_id: 1025,
      task: 'exchange',
      duration_ms: 12000,
      outcome: { status: 'success', summary: '兑换成功：小红花（花 1800 AI豆）' },
      details: {
        kind: 'exchange',
        reached: true,
        status: 'ordered',
        plan: 'available',
        plan_reason: '可兑',
        code: 0,
        message: 'success',
        rounds: 1,
        attempts: 2,
        offset: 87,
        balance: 10496,
        after_balance: 8696,
        waited_until_ms: 1_789_797_599_990,
        device_id_failed: '',
        error: '',
        prize: { prize_id: 251230053, name: '小红花', price: 1800 },
        order: {
          order_id: 'ORD-1025',
          prize_id: 251230053,
          prize_name: '小红花',
          cost: 1800,
          insert_time: 1_789_797_600_000,
          expire_time: 1_789_883_999_000,
        },
      },
    },
    {
      // ⭐ 直播那一次的**结构化明细**（`details.liveRoom`）：
      // 历史页必须把它渲染出来。从前这套明细只长在直播页上 ⇒
      // 过去的每一次直播都只剩一行小结，用户回看时无处可看。
      ...RUN,
      run_id: 1024,
      task: 'live-room',
      duration_ms: 66000,
      outcome: { status: 'success', summary: '进账 3 朵小红花' },
      details: {
        liveRoom: {
          balanceBefore: 10,
          balanceAfter: 13,
          gained: 3,
          codes: ['历史口令甲', '历史口令乙', '历史口令丙'],
          draws: [
            { code: '历史口令甲', kind: 'succeeded', flowerNum: 3, prizeName: '小红花' },
            { code: '历史口令乙', kind: 'kouling_expired' },
            { code: '历史口令丙', kind: 'already_redeemed' },
          ],
          expiredCodes: ['历史口令乙'],
        },
      },
    },
  ],
  '/api/schedules': [SCHEDULE],
  '/api/pipelines': [PIPELINE],
  '/api/accounts': [ACCOUNT, ACCOUNT2],
  /**
   * 今日AI豆（云朵记录汇总）。形状照 `web/src/api.ts` 的 `BeanToday`。
   *
   * ⚠ **一个账号一份**：豆按账号记账，不合计（合计出来哪个账号都用不了）。
   * 这里给两笔，正好覆盖净增的**正负两种**长相。
   *
   * ⚠⚠ **后端还没有这个端点**（2026-09-30，见 `web/src/api.ts` 的 `BeanToday`
   * 与 `docs/task-inventory.md` §5）。这里放 fixture 是**故意的**：总览那张卡
   * "有读数"的长相（每个账号一行、读不到的单独一行染黄）只有喂了数据才走得到，
   * 否则门禁里它一直停在"待接入"态 —— 那条渲染路径等于没验。
   * 端点落地后把这段注释删掉即可，**不用**删这条 fixture。
   */
  '/api/beans/today': {
    nowMs: 1790581000000,
    accounts: [
      { account: ACCOUNT.id, net: 30, gained: 120, spent: 90, count: 7 },
      { account: ACCOUNT2.id, net: -200, gained: 0, spent: 200, count: 1 },
    ],
    failed: [],
  },
  /**
   * 本机设备指纹（`getPcDevice`）与换一台（`rotatePcDevice`）。
   * 缺前者 ⇒ 账号页 `Promise.all` 整体失败 ⇒ 列表一行都不渲染（见上面那段 ⚠）。
   */
  '/api/login/device': { device_id: 'dev-abc123' },
  'POST /api/login/device/rotate': { device_id: 'dev-rotated-9f2c' },
  /**
   * 每账号设置的**读**（`GET /api/accounts/{id}/settings`）。形状**照 `web/src/api.ts` 的
   * `AccountSettings`**（camelCase，与后端 `AccountSettingsDto` 一一对应）—— 随手编形状
   * 等于让界面"跑通了但测的是空壳"（本文件开头那条 ⚠ 就是这个教训）。
   *
   * 两个账号刻意给**相反**的长相，好让两条路都走得到：
   * - `ACCOUNT`：`notify` 非空、配了两个渠道（其中 `workWeixinBot.url` 是**带密钥的
   *   webhook**，用来看"打码显示"那条路有没有把它当密文）；`skipTasks` 非空。
   * - `ACCOUNT2`：`notify: null` + 全默认 ⇒ "+ 加渠道"与"一个渠道都没配"那条路。
   *
   * ⚠ **写**（`PUT`）不在这里 —— 见 `VARIANTS.ready`。`FIXTURES` 这一路的值"就是 body"，
   * 放 `handler` 进去会被序列化成 `{status, body}` 发回客户端（实测踩到）。
   */
  // ⭐ 兑换页的**奖品清单**（2026-10-02 补）：没有它，无头浏览器里的兑换页只会说
  // 「清单没读到」⇒ 一行「兑换 / 已抢光」都渲染不出来 ⇒ 行内按钮的配色就只能靠
  // 用户截图 + 我猜 ✗（今天在这上面栽了两回）。两行覆盖两种形态：
  // 可兑（`plan: go`）与抢光（`plan: min_remainder_zero`，`minRemainderCount: 0`）。
  /**
   * 兑换页的订阅（**静态**一份，给不点星星的变体看；`ready` 里那份是有状态的 ✓）。
   */
  '/api/exchange/subscriptions': {
    items: [
      {
        prizeId: 251230053,
        name: '移动云盘100万tokens叠加包',
        groupId: 1,
        groupTitle: 'AI豆兑换',
        subscribedAtMs: 1_790_000_000_000,
      },
    ],
  },
  '/api/accounts/13800000000/exchange-prizes': EXCHANGE_PRIZES,
  '/api/accounts/13900000001/exchange-prizes': EXCHANGE_PRIZES,
  '/api/accounts/13800000000/settings': {
    backupWaitSecs: 20,
    refreshTokenDays: 10,
    skipTasks: [117],
    aiAvatarEnabled: false,
    aiAvatarDailyLimit: 10,
    redpackEnabled: true,
    playAiEnabled: true,
    koulingEnabled: false,
    notify: {
      title: 'mcloud 运行推送',
      onlyError: false,
      minLevel: 'info',
      pushplus: { token: 'fixture-pushplus-token' },
      workWeixinBot: { url: 'https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=fixture-key' },
    },
  },
  '/api/accounts/13900000001/settings': {
    backupWaitSecs: 0,
    refreshTokenDays: 0,
    skipTasks: [],
    aiAvatarEnabled: false,
    aiAvatarDailyLimit: 10,
    redpackEnabled: true,
    playAiEnabled: true,
    koulingEnabled: false,
    notify: null,
  },
}

/**
 * 认证闸的三个状态都要测 —— **首屏到底显示哪个界面，全看它**：
 *
 * | variant | `/api/setup` | `/api/session` | 应用该显示 |
 * |---|---|---|---|
 * | `ready`（默认） | 200 已初始化 | 200 | 完整外壳（drawer/dock/页面） |
 * | `login` | 200 已初始化 | **401** | `AuthPage mode=login` |
 * | `setup` | 200 **未初始化** | **503** | `AuthPage mode=setup`（建管理员向导） |
 *
 * ⚠ 这两条响应的**形状要照后端的真实响应写**（`crates/mcloud-server/src/auth_gate.rs`）：
 * `error` 是**字符串**、`code` 在**顶层**（`api.ts` 读的就是 `body.code`）。
 * 写成 `{error:{code}}` 时 `classifyAuthFailure(503, undefined)` 认不出来 ⇒
 * 落到 `probeSession` 的 catch 兜底 ⇒ 显示的是**登录**页 ——
 * 那样"setup 态通过"其实是登录态通过（两个 variant 字节数一样才发现）。
 */
export const VARIANTS = {
  /**
   * `ready` 多一条**保存设置**的 `PUT`：请求体即响应体。
   *
   * ⚠ 它必须在 `VARIANTS` 里而**不是** `FIXTURES` 里：`FIXTURES` 那一路是
   * "值**就是** body"（`plain = true`），`handler` 的返回值会被当成 body 整个
   * 序列化成 `{status, body}` —— 客户端拿到的就不是那份设置了。实测踩到：
   * 界面读回一个没有 `skipTasks` 的对象，报 `Cannot read properties of undefined`。
   */
  ready: {
    /**
     * 兑换订阅：GET 读那份有状态的值、POST **toggle** 改它。
     *
     * ⚠ 装置这边也要按**真后端那条规则**实现（已订就取消、没订就追加）——
     * 否则门禁测的是"装置自己的逻辑"，而不是"服务端判切换"这件事 ✓。
     */
    'GET /api/exchange/subscriptions': {
      handler: () => ({ status: 200, body: { items: subState.items } }),
    },
    'POST /api/exchange/subscriptions/toggle': {
      handler: (body) => {
        const id = body?.prizeId
        const has = subState.items.some((s) => s.prizeId === id)
        subState.items = has
          ? subState.items.filter((s) => s.prizeId !== id)
          : [
              ...subState.items,
              {
                prizeId: id,
                name: body?.name ?? '',
                groupId: body?.groupId ?? null,
                groupTitle: body?.groupTitle ?? null,
                subscribedAtMs: Date.now(),
              },
            ]
        return { status: 200, body: { items: subState.items } }
      },
    },
    // 任务配置：GET 读、PUT 存（都改那份有状态的值）
    'GET /api/tasks/daily-checkin/config': { handler: () => ({ status: 200, body: configView('daily-checkin') }) },
    'GET /api/tasks/live-room/config': { handler: () => ({ status: 200, body: configView('live-room') }) },
    'PUT /api/tasks/daily-checkin/config': {
      handler: (body) => ({ status: 200, body: saveCfg('daily-checkin', body) }),
    },
    'PUT /api/tasks/live-room/config': {
      handler: (body) => ({ status: 200, body: saveCfg('live-room', body) }),
    },
    /**
     * 运行：**装置替后端把"参数有没有真带上"这件事判了**。
     *
     * `listenSeconds` 必须是配置页里存的 123（见门禁那两步），否则回 400。
     * 这比"点了按钮有反应"强得多：它钉住的是"配置页存的东西真的进了运行请求"，
     * 而那正是这个功能存在的全部理由。
     */
    'POST /api/runs': {
      handler: (body) => {
        if (body?.task === 'live-room' && body?.body?.listenSeconds !== 123) {
          return {
            status: 400,
            body: {
              error: `运行没带上配置页存的参数（收到 ${JSON.stringify(body?.body)}）`,
              code: 'config',
            },
          }
        }
        return { status: 200, body: { run_id: 4242 } }
      },
    },
    /**
     * 账号列表：**有状态**的一份（区别于 `FIXTURES` 里那份静态的）。
     *
     * 停用是"改一处、别处都跟着变"的功能，所以装置必须真的把状态记住：
     * 账号页点「停用」→ PATCH 改这份状态 → 任务/兑换页再拉列表时它就没了。
     * 若这里回一份静态名单，门禁只能证明"按钮点了有反应"，证明不了用户要的那件事。
     */
    'GET /api/accounts': {
      handler: (_body, req) => {
        const url = String(req?.url ?? '')
        const all = url.includes('includeDisabled=true')
        return { status: 200, body: acctState.accounts.filter((a) => all || a.enabled) }
      },
    },
    // 停用 / 启用：改上面那份状态，返回改完的那一条（页面据此重画卡片）
    'PATCH /api/accounts/13800000000': {
      handler: (body) => setFixtureEnabled('13800000000', body),
    },
    'PATCH /api/accounts/13900000001': {
      handler: (body) => setFixtureEnabled('13900000001', body),
    },
    // 策略 PUT 会把 body 原样回显（页面据此更新读数）
    'PUT /api/admin/security': {
      handler: (body) => ({ status: 200, body: { ...body, passwordScheme: 'sha256' } }),
    },
    // 改口令：默认成功。⚠ 真断言在 `routes.rs`（踢会话/换 cookie 那是后端的事）
    'PUT /api/admin/password': {
      handler: () => ({
        status: 200,
        body: { userId: 'u-1', name: 'admin', kind: 'web', sessionsEnded: 2 },
      }),
    },
    // 重启：202 受理（测试装置里没有真进程可重启）。真断言同样在 Rust 侧。
    'POST /api/system/restart': { status: 202, body: { ok: true } },
    'PUT /api/accounts/13800000000/settings': { handler: (body) => ({ status: 200, body }) },
    'PUT /api/accounts/13900000001/settings': { handler: (body) => ({ status: 200, body }) },
  },
  login: {
    '/api/session': { status: 401, body: { error: '未登录', code: 'unauthorized' } },
    /**
     * ⚠ key 带方法：`GET /api/session` 是"我登录了吗"（401），`POST /api/session` 是**登录动作**。
     * 同一个 URL 两种语义，只按 URL 匹配的话登录永远失败 —— 那样"登录成功后闸门放行"
     * 这条最要紧的路径就测不到（而它正是当初整包挂掉的地方）。
     */
    'POST /api/session': {
      /**
       * ⚠ 后端自述 `passwordScheme: 'sha256'` ⇒ 前端发来的**必须是**
       * `SHA-256('right-pass')` 的 hex，不能是明文。
       *
       * 这就是"前端真的做了那一次哈希"的端到端证据：哪天有人把它退化成发明文，
       * 登录闸立刻红（而且报错会直接说破这一点，而不是含糊的"口令不正确"）。
       */
      handler: (body) =>
        body?.password === sha256Hex('right-pass')
          ? { status: 200, body: { userId: 'u-1', name: 'admin', kind: 'web' } }
          : {
              status: 401,
              body: {
                error:
                  typeof body?.password === 'string' && /^[0-9a-f]{64}$/.test(body.password)
                    ? '用户名或口令不正确'
                    : 'HTTP 测试装置：口令不是 SHA-256 摘要（前端那一次哈希没做？）',
                code: 'invalid_credentials',
              },
            },
    },
  },
  setup: {
    '/api/setup': { status: 200, body: { initialized: false, minPasswordLen: 8, passwordScheme: 'sha256' } },
    '/api/session': { status: 503, body: { error: '实例尚未初始化，请先创建管理员', code: 'setup_required' } },
    'POST /api/setup': {
      // 建管理员这条路上同样必须是摘要（与登录那条同一个理由）
      handler: (body) =>
        typeof body?.password === 'string' && /^[0-9a-f]{64}$/.test(body.password)
          ? { status: 200, body: { userId: 'u-1', name: 'admin', kind: 'web' } }
          : {
              status: 400,
              body: { error: 'HTTP 测试装置：口令不是 SHA-256 摘要', code: 'invalid_credentials' },
            },
    },
  },
}

/** 存任务参数（`PUT /api/tasks/{name}/config` 用）。 */
const saveCfg = (name, body) => {
  const given = body && typeof body === 'object' && body.params ? body.params : {}
  taskCfg[name] = { ...(taskCfg[name] ?? {}), ...given }
  return configView(name)
}

/** 改装置里某个账号的启停（`PATCH /api/accounts/{id}` 用）。 */
const setFixtureEnabled = (id, body) => {
  const a = acctState.accounts.find((x) => x.id === id)
  if (!a) return { status: 404, body: { error: 'fixture 里没有这个账号', code: 'not_found' } }
  if (body && typeof body.enabled === 'boolean') a.enabled = body.enabled
  return { status: 200, body: a }
}

export const readVariant = () => {
  const v = arg('--variant', 'ready')
  if (!(v in VARIANTS)) throw new Error(`未知 --variant：${v}（可选 ${Object.keys(VARIANTS).join(' / ')}）`)
  return v
}

/**
 * fixture 服务：已知路径按 variant 覆盖，其余 `/api/*` 回 404，非 API 路径当静态文件发。
 *
 * `port = 0` ⇒ 由内核分配一个空闲端口（门禁走这条）；真实端口从 `server.address().port` 读。
 */
export const serve = (root, port, variant) =>
  new Promise((resolve, reject) => {
    resetAcctState()
    resetTaskCfg()
    const overrides = VARIANTS[variant]
    /**
     * 一条 fixture 可以是 `{status, body}`，也可以是 `{handler(body) → {status, body}}` ——
     * 后者用来表达"同一个端点按请求内容给不同结果"（登录成功/口令错）。
     * 匹配顺序：**方法+路径** 优先于 仅路径。
     */
    /**
     * ⚠⚠ **两种形状**：`VARIANTS`/带方法的覆盖是 `{status, body}`，
     * 而 `FIXTURES` 里的值**直接就是 body**。把后者当 `{status, body}` 用，
     * 会发出"200 + 空体"—— 应用那边 `JSON.parse('')` 拿不到东西、
     * `/api/session` 于是判成未登录，**整个 ready 变体都在渲染登录页**，
     * 而应用停在登录页 ⇒ 每一步都"没报错"，其实是**什么都没测到**。测试脚本自己的 bug 最会骗人。
     */
    const send = (req, res, entry, plain) => {
      const respond = (r) => {
        res.writeHead(plain ? 200 : (r.status ?? 200), { 'content-type': 'application/json' })
        res.end(JSON.stringify(plain ? r : r.body))
      }
      if (!entry.handler) {
        respond(entry)
        return
      }
      let raw = ''
      req.on('data', (d) => {
        raw += d
      })
      req.on('end', () => {
        let parsed = null
        try {
          parsed = raw ? JSON.parse(raw) : null
        } catch {
          // 非 JSON 体：handler 自己处理 null
        }
        // ⚠ 第二个参数是原始 req：**带查询串的 GET** 需要它才能按查询串分岔
        // （`GET /api/accounts?includeDisabled=true` 与不带参数是两种结果）。
        // 原来只传 body，于是"停用后下拉里没有它"这条根本量不出来 —— 装置测不到
        // 就等于没测（本仓在这上面栽过：门禁全绿而功能是坏的）。
        respond(entry.handler(parsed, req))
      })
    }
    const server = http.createServer((req, res) => {
      const url = (req.url ?? '/').split('?')[0]
      const key = `${req.method} ${url}`
      const override = overrides[key] ?? overrides[url]
      if (override) {
        send(req, res, override, false)
        return
      }
      const fixture = FIXTURES[key] ?? FIXTURES[url]
      if (fixture !== undefined) {
        send(req, res, fixture, true)
        return
      }
      if (url.startsWith('/api/')) {
        /**
         * ⚠ 形状与上面 `VARIANTS` 那条同一条规矩：**`error` 是字符串、`code` 在顶层**
         * （`api.ts` 读的是 `String(body.error)`）。写成 `{error:{code,message}}` 的话
         * 界面上会出现 `[object Object]` —— 看着像应用的 bug，其实是 fixture 自己造的。
         */
        res.writeHead(404, { 'content-type': 'application/json' })
        res.end(JSON.stringify({ error: `fixture 未提供：${url}`, code: 'not_found' }))
        return
      }
      let f = path.join(root, url)
      if (!fs.existsSync(f) || fs.statSync(f).isDirectory()) f = path.join(root, 'index.html')
      const type = f.endsWith('.js') ? 'text/javascript' : f.endsWith('.css') ? 'text/css' : 'text/html'
      res.writeHead(200, { 'content-type': type })
      fs.createReadStream(f).pipe(res)
    })
    /**
     * ⚠ **必须**接 `'error'`。不接时 `listen` 失败（最典型：端口被别的进程占着）
     * 会成为**未处理的 'error' 事件**，Node 当场把进程打死 —— 而且死在**任何 stdout 之前**，
     * 上层只能看到一句光秃秃的「✗ 抛错」加一个空行（2026-10-01 三个认证态全这样，
     * 真因是 `EADDRINUSE: address already in use 127.0.0.1:48251`）。
     * 接上之后，这类失败至少会带着原话报出来。
     */
    server.on('error', reject)
    /**
     * `port = 0` ⇒ 内核分配空闲端口。门禁默认走这条：同一个工作区里多人/多 agent
     * **并发**跑验收时不会互撞。调用方拿到 server 后用 `server.address().port` 取真实端口。
     */
    server.listen(port, '127.0.0.1', () => resolve(server))
  })

/** 应用认得的 hash 路由：`parseHash` 比的是 `#/settings` 这种**带斜杠**的 href。 */
export const hashRoute = (route) => `#/${String(route).replace(/^#\/?/, '')}`

// 同步取路径：此时还没有任何请求在飞，阻塞几毫秒无妨（异步取会让调用方拿到空串）
export const chromePath = execFileSync('bash', ['-c', 'ls -d ~/.cache/puppeteer/chrome-headless-shell/*/chrome-headless-shell-*/chrome-headless-shell | head -1'], { encoding: 'utf8' }).trim()

/**
 * 取 `#app` 子树（按 div 标签配平），去注释锚点、折叠标签间空白。
 *
 * ⚠ 注释要**去掉**：lite 用 `<!---->` 当动态槽占位（§11.2 #6），它不属于渲染结果。
 */
export function appSubtree(html) {
  const i = html.indexOf('<div id="app"')
  if (i < 0) return ''
  let depth = 0
  let end = i
  for (const m of html.slice(i).matchAll(/<(\/?)div\b[^>]*>/g)) {
    depth += m[1] ? -1 : 1
    if (depth === 0) {
      end = i + m.index + m[0].length
      break
    }
  }
  return html
    .slice(i, end)
    .replace(/<!--.*?-->/gs, '')
    .replace(/>\s+</g, '><')
    .trim()
}

/**
 * 读 chrome 写进 profile 的 `DevToolsActivePort`（首行就是它实际监听的端口）。
 *
 * ⚠ 为什么读文件、而不是自己 `listen(0)` 探一个空闲端口再用：
 * `--remote-debugging-port=0` 时端口由**内核**分配，分配结果只有这里知道；
 * 自己试探再关掉去用，中间那段就是竞态（并发跑两份验收时正好会撞）。
 * 文件还没写出来时返回 0。
 */
const readDevToolsPort = (profile) => {
  try {
    const n = Number(fs.readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0].trim())
    return Number.isInteger(n) && n > 0 ? n : 0
  } catch {
    return 0 // chrome 还没起来 / 还没写出来
  }
}

/** 等 `DevToolsActivePort` 出现；chrome 起不来时它会一直不出现 —— 超时即判启动失败。 */
const waitForDevToolsPort = async (profile, timeout = 20000) => {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    const p = readDevToolsPort(profile)
    if (p) return p
    await sleep(60)
  }
  throw new Error(`chrome 没写出 DevToolsActivePort（profile=${profile}）—— 它很可能启动失败了`)
}

/**
 * 起一个可交互的页面（CDP）。零依赖：Node 自带 `WebSocket`。
 *
 * 为什么要它：`--dump-dom` 只能看**首屏**。事件处理器、`batch`、列表增删、
 * 切页时新建/卸载组件这些路径，全在"点一下之后" —— 那正是这套框架最容易出错的地方。
 */
export const launchChrome = ({ url, debugPort, profile }) =>
  spawn(
    chromePath,
    [
      `--remote-debugging-port=${debugPort}`,
      `--user-data-dir=${profile}`,
      '--headless', '--disable-gpu', '--hide-scrollbars', '--window-size=1280,900',
      '--no-first-run', '--no-default-browser-check',
      url,
    ],
    { stdio: ['ignore', 'ignore', 'ignore'] },
  )

const waitForTarget = async (port, timeout = 20000) => {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()
      const page = list.find((t) => t.type === 'page' && t.webSocketDebuggerUrl)
      if (page) return page
    } catch {
      // 端点还没起来，继续等
    }
    await sleep(120)
  }
  throw new Error(`CDP 端点没起来（port=${port}）—— chrome 可能启动失败了`)
}

/** 极简 CDP 客户端：`send` 一次调用、`eval` 在页面里求值、`on` 收事件。 */
class Cdp {
  constructor(ws) {
    this.ws = ws
    this.seq = 0
    this.waiting = new Map()
    this.handlers = new Map()
    ws.onmessage = (m) => {
      const msg = JSON.parse(m.data)
      if (msg.id) {
        const w = this.waiting.get(msg.id)
        if (!w) return
        this.waiting.delete(msg.id)
        msg.error ? w.rej(new Error(JSON.stringify(msg.error))) : w.res(msg.result)
      } else {
        for (const h of this.handlers.get(msg.method) ?? []) h(msg.params)
      }
    }
  }

  static async connect(wsUrl) {
    const ws = new WebSocket(wsUrl)
    await new Promise((res, rej) => {
      ws.onopen = res
      ws.onerror = () => rej(new Error(`连不上 CDP：${wsUrl}`))
    })
    return new Cdp(ws)
  }

  send(method, params = {}) {
    const id = ++this.seq
    this.ws.send(JSON.stringify({ id, method, params }))
    return new Promise((res, rej) => {
      this.waiting.set(id, { res, rej })
      setTimeout(() => {
        if (this.waiting.delete(id)) rej(new Error(`CDP 调用超时：${method}`))
      }, 20000)
    })
  }

  on(method, cb) {
    const list = this.handlers.get(method) ?? []
    list.push(cb)
    this.handlers.set(method, list)
  }

  async eval(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })
    if (r.exceptionDetails) {
      throw new Error(`页面里求值抛错：${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`)
    }
    return r.result.value
  }

  close() {
    try {
      this.ws.close()
    } catch {
      // 关不掉就算了：进程本来就要被杀
    }
  }
}

/**
 * 打开一个可交互页面：起 chrome → 等 CDP → 连接 → 收异常事件。
 *
 * `debugPort = 0` ⇒ 内核分配，起完从 profile 的 `DevToolsActivePort` 里读回来。
 */
export async function openSession({ port, route, debugPort = 0, profile, width = 1280 }) {
  const url = `http://127.0.0.1:${port}/${route}`
  const child = launchChrome({ url, debugPort, profile })
  /**
   * ⚠ 握手阶段抛错**必须收尸**。旧版这里直接 `await waitForTarget`，它一超时就
   * 留下一个没人管的 chrome：那个孤儿会一直占着 profile 与调试端口，
   * 于是**下一次**运行又以"CDP 端点没起来"挂掉，再留一个孤儿 —— 一次失败锁死后面每一次。
   */
  let target
  try {
    const dp = debugPort || (await waitForDevToolsPort(profile))
    target = await waitForTarget(dp)
  } catch (e) {
    try {
      child.kill('SIGKILL')
    } catch {
      // 已经死了
    }
    throw e
  }
  const cdp = await Cdp.connect(target.webSocketDebuggerUrl)
  await cdp.send('Page.enable')
  await cdp.send('Runtime.enable')
  const errors = []
  cdp.on('Runtime.exceptionThrown', (p) => errors.push(p.exceptionDetails?.exception?.description ?? p.exceptionDetails?.text ?? '未知异常'))
  // 页面里的 `console.error` 也要收：应用有把错误吞掉的路径，日志是唯一线索
  cdp.on('Runtime.consoleAPICalled', (p) => {
    if (p.type === 'error') errors.push('console.error: ' + p.args.map((a) => a.value ?? a.description ?? '').join(' '))
  })
  return { child, cdp, errors, width }
}
