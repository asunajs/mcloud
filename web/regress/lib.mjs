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
// ⚠ 无头 chrome 的公共参数（含可选的 --no-sandbox）只有一份来源，见该文件头
import { headlessFlags } from '../scripts/chrome-flags.mjs'
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

/**
 * 服务端日志（`/api/logs`）—— **会长**的一份，而且**分两段**（2026-10-09 分页 ✓）。
 *
 * 为什么必须会长：日志页每 2s 轮询一次，而「贴底时新行要跟到底部」这条
 * 只有内容真的变多才量得出来 —— 静态名单下 `scrollHeight` 不变，
 * "还在底部"这句在坏代码里也可能成立 ⇒ 门禁绿得没有意义 ✗。
 * 每次请求多两行，正好模拟"进程还在打日志" ✓。
 *
 * # 为什么是"内存窗口 + 盘上全量"两层（**别退回单层** ✗）
 *
 * 真后端的 `/api/logs` 有两条路（`crates/mcloud-server/src/logbuf.rs::page` ✓）：
 * 不带 `before` = **最新** `limit` 行 ✓、带 `before=<seq>` = `seq < before` 的最后 `limit` 行 ✓
 * （内存环只装最新的那些、更早的在 `data/server-logs/*.jsonl` 上按需读 ✓）。
 * 装置这边**必须照着分层**：单层（只有一份清单）时"实时那一份"就是全部 ⇒
 * 翻页**永远翻不出东西**（`before` = 最旧那一行 ⇒ 返回空 ✓）——
 * 那样门禁里"点了按钮、行数变大"这条**根本立不起来**（装置测不到 = 没测 ✓）。
 *
 * ⚠ `SERVER_LOG_ROWS` 必须**大于**日志页一次要的行数（`RING_LINES` = 2000 ✓）：
 * 实时那一份只给最新 2000 行，剩下的才轮得到「加载更早」去翻 ✓。
 * 夹具取 2600 ⇒ 头一次翻页能翻回满 500 行、第二次翻到 100 行（**不满 ⇒ 到头了** ✓）——
 * 于是"按钮消失"那条能在**两次点击**内走完 ✓。
 *
 * ⚠ 与 `acctState`/`taskCfg` 同一个理由要有 `reset`：同一次进程里跑两个变体时
 * 别把上一轮攒的行带过去 ✓。
 */
const logsState = { all: [], seq: 0 }
/** 盘上总量（含"内存窗口"里那些 ✓）—— 必须 > 2000，见上面那段 ✓。 */
const SERVER_LOG_ROWS = 2600
/**
 * 一行服务端日志。
 *
 * ⚠ 每一行都带 `account="…"`（两个夹具账号轮流 ✓）—— 这是**照真后端那个形状**给的：
 * `mcloud-app` 的 sink 会给一趟运行的**每一行**盖上账号（`crates/mcloud-app/src/run_labels.rs` ✓），
 * 而日志页要把那串主键显示成**人名**、并支持按账号筛 ✓（用户口径 2026-10-05：
 * 「页面上看不到是那个账号的日志啊」）。
 * ⇒ 装置里要是没有这一格，"显示成人名"那条断言就成了空话 ✗。
 *
 * ⚠ 字段落在**正文末尾**（`message account="…"` ✓）而不是拼在前面：真后端就是这么排的
 * （`RingLayer` 的 `Fields::finish` 先 message 再 k=v ✓）—— 页面得能处理"账号在尾巴上" ✓。
 *
 * ⚠ `seq`（2026-10-09 ✓）：**翻页游标** ✓（`?before=<seq>` ✓）。它必须是**全局单调**的
 * （真后端还保证跨重启单调 ✓）—— 夹具里就是"这一行是第几条" ✓。
 * 少了这一格，页面算不出游标 ⇒ 翻页无从谈起 ✗（本文件开头那条"别把夹具写成空壳"的教训）。
 */
const logLine = (seq, at) => ({
  level: seq % 7 === 0 ? 'warn' : 'info',
  target: 'mcloud_engine::scheduler',
  message: `第 ${seq} 行 —— 门禁用（量滚动位置） account="${seq % 2 === 0 ? ACCOUNT.id : ACCOUNT2.id}"`,
  at_ms: at,
  // ⚠ 2026-10-06：`/api/logs` 的每一行多了这两格（后端 `ServerLog` ✓）——
  // `category`（`task`/`ops`，判据是"谁打的" ✓）与 `account`（正文那格的**结构化**副本 ✓）。
  // 少了它们，页面就认不出这一行是哪个账号的（身份改读结构化那格了 ✓）⇒
  // 门禁当场红（"账号筛选行"那三条断言）—— 这正是它该抓的 ✓。
  category: 'task',
  account: seq % 2 === 0 ? ACCOUNT.id : ACCOUNT2.id,
  seq,
})
const resetLogsState = () => {
  logsState.all = Array.from({ length: SERVER_LOG_ROWS }, (_, i) =>
    logLine(i + 1, 1790581000000 + i * 1000),
  )
  logsState.seq = SERVER_LOG_ROWS + 1
}

/** 一页最多多少行（照 `crates/mcloud-server/src/logbuf.rs::MAX_PAGE` ✓，超出会被夹 ✓）。 */
const SERVER_LOG_MAX_PAGE = 5000

/**
 * 按契约切一页服务端日志（`limit` / `before` / `level` / `q` / `category` ✓，
 * 见 `web/src/api.ts::listServerLogs`）。
 *
 * ⚠ 两条路的语义照 `logbuf::page` ✓：不带 `before` = **最新** `limit` 行（实时那一份 ✓）；
 * 带 `before` = `seq < before` 的最后 `limit` 行（往上翻 ✓）。**都升序** ✓。
 * ⚠ 筛选判据照 `logbuf::keeps` ✓（级别是"**不低于**"、类别、关键词匹配正文与来源）——
 * 少一条的表现是"翻上去的那几页突然不过滤了"，而页面上看不出来 ✗。
 */
export const sliceServerLogs = (all, { limit, before, level, q, category }) => {
  // 与 `logbuf::rank_of` 同一个口径 ✓（`trace` 并入最低那档 ✓）
  const rank = { error: 4, warn: 3, info: 2, debug: 1, trace: 0 }
  const floor = rank[level] ?? 0
  const needle = String(q ?? '').trim().toLowerCase()
  const keep = (l) =>
    (rank[l.level] ?? 0) >= floor &&
    (!category || category === 'all' || l.category === category) &&
    (needle === '' ||
      l.message.toLowerCase().includes(needle) ||
      l.target.toLowerCase().includes(needle))
  const n = Math.max(
    1,
    Math.min(Number.isFinite(limit) && limit > 0 ? limit : 500, SERVER_LOG_MAX_PAGE),
  )
  const matched = all.filter(keep)
  return before == null ? matched.slice(-n) : matched.filter((l) => l.seq < before).slice(-n)
}

/**
 * 存储信息（数据目录 / 日志占用 / **容量上限**）—— 上限那一份是**有状态**的。
 *
 * ⚠ 为什么必须有状态（`PUT` 改它、`GET` 读它 ✓）：用户口径 2026-10-09
 * 「日志和历史不要全加入内存，使用本地记录，**让用户设置最大大小**」—— 要验的是
 * "改了这些数 ⇒ 服务端记住了、界面读数跟着变" ✓。回一份静态值就只能证明
 * "按钮点了有反应" ✗（本仓在"装置测不到 = 没测"上栽过：`acctState` / `pushPanelState`
 * 都是同一个理由 ✓）。
 *
 * ⚠ `logUsage.maxBytes` **由 `limits.logTotalMb` 现算**（不是各写一份常数）——
 * 这样门禁把上限改成 128 之后，卡片上那行「占用 / 上限」必须跟着变成 128 MB，
 * 而那个数**只能**来自服务端那份状态 ✓（页面把用户输的数原样回显也能"看着对"，
 * 那是假绿 ✗）。
 * ⚠ `serverLogUsage`（2026-10-09 加的第二份 ✓）**同一个口径**：`maxBytes` 由
 * `limits.serverLogTotalMb` 现算 ✓ —— 它管的是**另一个目录**（`data/server-logs/` ✓）。
 *
 * ⚠⚠ **五个**上限（2026-10-09 ✓）：前三个是运行那一侧（历史条数 / 单次运行的行数 /
 * `data/logs/` 总量），后两个是**服务端日志**（内存窗口行数 / `data/server-logs/` 总量）✓。
 * 少一格的表现是"界面上改了、装置里没这一格" ⇒ 那一条断言静默失效 ✗。
 */
const storageState = {
  limits: {
    historyLimit: 200,
    logLimitPerRun: 2000,
    logTotalMb: 64,
    serverLogMemLines: 2000,
    serverLogTotalMb: 64,
  },
}
/** 日志占用（固定）：556 KiB / 139 个文件 —— 让界面算出 `556 KB / 64 MB` 那样的人话 ✓。 */
const LOG_BYTES = 569344
const LOG_FILES = 139
/**
 * 服务端日志占用（固定，**与上面那两份不同的数** ✓）：1.2 MB / 12 个文件。
 *
 * ⚠ 数刻意**不一样**：两份读数用同一组数时，"界面把两份搞混了"（把运行日志那份
 * 显示在服务端日志那行上）照样绿 ✗ —— 那是这类"多一行读数"最可能出的错 ✓。
 */
const SERVER_LOG_BYTES = 1258291
const SERVER_LOG_FILES = 12
const resetStorageState = () => {
  storageState.limits = {
    historyLimit: 200,
    logLimitPerRun: 2000,
    logTotalMb: 64,
    serverLogMemLines: 2000,
    serverLogTotalMb: 64,
  }
}
/** 当前状态按后端的口径算一遍（`GET` / `PUT` 的返回**同一个形状** ✓）。 */
const storageView = () => ({
  dataDir: '/tmp/mcloud-fixture/data',
  logDir: '/tmp/mcloud-fixture/data/logs',
  binary: '/tmp/mcloud-fixture/target/release/mcloud-server',
  dataBytes: 20480,
  logBytes: LOG_BYTES,
  files: [
    { name: 'accounts.json', bytes: 4096 },
    { name: 'users.json', bytes: 512 },
  ],
  logUsage: { files: LOG_FILES, bytes: LOG_BYTES, maxBytes: storageState.limits.logTotalMb * 1024 * 1024 },
  serverLogUsage: {
    files: SERVER_LOG_FILES,
    bytes: SERVER_LOG_BYTES,
    maxBytes: storageState.limits.serverLogTotalMb * 1024 * 1024,
  },
  limits: { ...storageState.limits },
})

/**
 * 存储上限的**范围**（与后端一致 ✓，见 `web/src/api.ts::StorageLimits`）——
 * 装置这一层也要卡一遍：真后端会回 400 + 人话 ✓，装置不卡的话"前端把越界值发出去了"
 * 照样绿 ✗。
 */
const LIMIT_RANGE = {
  historyLimit: [20, 2000],
  logLimitPerRun: [200, 20000],
  logTotalMb: [1, 4096],
  serverLogMemLines: [200, 20000],
  serverLogTotalMb: [1, 4096],
}

/**
 * 某次运行的日志（`GET /api/runs/{id}/logs` 的数据源）。
 *
 * ⚠ 为什么要**按游标真分页**（而不是回一份固定行数的清单）：用户口径
 * 「大日志做虚拟加载或者分页」⇒ 要验的是"点「加载更早」取到的**确实是更早的**" ✓。
 * 装置若不认 `before`，页面上"行数变多了"照样成立 ⇒ 一条空转 ✗。
 *
 * 行里那个 `seq` 就是**行号**（1 起 ✓）⇒ 门禁能一眼看出"往上翻之后第一行变小了" ✓。
 * 字段形状照 `web/src/api.ts::LogLine`（camelCase：`runId`/`level`/`kind`/`message`/`atMs`/`seq` ✓）。
 */
const RUN_LOG_LINES = 800
const fixtureRunLogs = (runId) =>
  Array.from({ length: RUN_LOG_LINES }, (_, i) => ({
    runId,
    level: i % 9 === 0 ? 'warn' : 'info',
    // ⚠ 分类那格**照真后端给**（`mcloud_core::LogLine` 2026-10-09 起带 `kind` ✓）——
    // 装置少给一格的表现是"界面上永远只有三档颜色"，而看不出来是装置抠的 ✗。
    kind: i % 5 === 0 ? 'success' : 'info',
    // 正文里带**行号**：门禁据此断言"翻上去之后首行是第 1 行"（而不是只数行数 ✓）。
    message: `夹具运行日志 ${String(i + 1).padStart(4, '0')} —— 行号就是 seq`,
    atMs: 1790581000000 + i * 1000,
    seq: i + 1,
  }))

/**
 * 按契约切一页（`limit` / `before` / `after` ✓，见 `web/src/api.ts::getRunLogs`）。
 *
 * ⚠ 全局那份夹具与 `bulk.mjs` 的私有变体**共用它**：两边各写一份"分页"迟早分叉，
 * 而分页正是这次要验的东西 ✗。
 * ⚠ `limit` 缺省 500、上限 = 用户设的 `logLimitPerRun`（与真后端一致 ✓）——
 * 上限那一格从 `storageState` 读，于是"改小了上限 ⇒ 一次能翻回来的行变少"也成立 ✓。
 */
export const sliceRunLogs = (all, { limit, before, after }) => {
  const cap = storageState.limits.logLimitPerRun
  const n = Math.min(Number.isFinite(limit) && limit > 0 ? limit : 500, cap)
  if (before != null) return all.filter((l) => l.seq < before).slice(-n)
  if (after != null) return all.filter((l) => l.seq > after).slice(0, n)
  return all.slice(-n)
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
  // 手建的定时：这一格**恒为 `null`**（只有「凭据自动刷新」那条写它 ✓，
  // 见 `crates/mcloud-app/src/auth_refresh_auto.rs` 的 `note_of` ✓）
  lastNote: null,
  runCount: 12,
  nextFireAt: 1790667400000,
  // 绑了一条（**已停用**的）规则：编辑弹窗里那一格要显示它、并标出「已停用」✓
  // ⚠ 2026-10-08 起是**数组**（用户口径「一个行为可以选择多个规则」✓）。
  pushRules: ['rule-3'],
  // 结局条件**空数组 = 跑完就推** ✓ —— 夹具给空的那一种，
  // 门禁再在交互里勾上「运行失败」验另一头 ✓。
  pushEvents: [],
}
/**
 * 那条「自动直播定时」——它**就是一条普通定时**（用户口径：「它是用户的定时，
 * 只是系统会继续维护它」✓），只是多两格：
 *
 * * `managedBy: "live-auto"` ⇒ 定时页要给它标「系统维护」并说明"时刻由系统维护" ✓；
 * * `managedWanted: true` ⇒ **用户意愿**（定时页上那个开关翻的就是它 ✓，
 *   与设置页/直播页那张卡上的开关**同一格** ✓）。
 *
 * ⚠ 形状照 `web/src/api.ts` 的 `Schedule` 写（少一格的表现是那一处静默变默认值，
 * 而界面上看不出来 ✗）。`expr` 照后端 `one_shot_expr` 的产物写：
 * `"2026-10-08 11:00:00"` ⇒ `"0 0 11 8 10 *"` ✓。
 */
const AUTO_SCHEDULE = {
  id: 'auto-live-u-1',
  name: '直播自动定时',
  task: 'live-room',
  // 空 = 全部已启用账号（与那张卡的初值一致 ✓）
  accounts: [],
  expr: '0 0 11 8 10 *',
  timezone: 'Asia/Shanghai',
  body: {},
  enabled: true,
  lastFireMs: null,
  lastRunMs: null,
  lastStatus: null,
  lastError: null,
  lastNote: null,
  runCount: 0,
  // ⚠ 与 `/api/live-room/auto-schedule` 那份 `at`（`AUTO_LIVE_AT` = 2026-10-08 11:00:00
  // 东八区 ✓）**必须是同一刻**：两处读的是同一条定时（`expr` 也照它写 ✓）——
  // 夹具里差一个数，界面就会一边显示 11:00、一边显示别的时间 ✗（不报错，但看着像 bug）。
  nextFireAt: 1791428400000,
  pushRules: [],
  pushEvents: [],
  managedBy: 'live-auto',
  managedWanted: true,
}

/**
 * 那条「**凭据自动刷新**」定时（`auto-auth-refresh-<owner>` ✓，每个用户开箱就有 ✓）。
 *
 * # 为什么装置里非要有一条它（2026-10-11）
 *
 * 它**全跳过**时后端记的是 `lastStatus: "ok"`（跳过是正常结局 ✓）、
 * `lastError: null` ⇒ 光看徽章是「成功」，**看不出"今天为什么没刷"** ✗。
 * 补的那一格是 `lastNote`（`crates/mcloud-app/src/auth_refresh_auto.rs` 的 `note_of` ✓），
 * 界面上必须**真的把那句话画出来** ✓ —— 而"画没画出来"只有装置里有一条带这格的
 * 定时才量得到（本仓在"装置测不到 = 没测"上栽过 ✗）。
 *
 * ⚠ `lastNote` 照 `note_of` 对**一个账号、还剩 12 天**的产物逐字写
 *（`未到提前量，跳过（还剩 12 天）` ✓）—— 自己编一句的话，
 * 门禁钉住的就是装置、不是后端 ✓。
 *
 * ⚠ `task` 是 `auth-refresh`：它**不是**注册表里的任务（见 `mcloud_core::AUTH_REFRESH_TASK` ✓）
 * ⇒ 这一页照原样显示这个任务名（与真后端一样 ✓），不是夹具写错了 ✗。
 */
const REFRESH_SCHEDULE = {
  id: 'auto-auth-refresh-u-1',
  name: '凭据自动刷新',
  task: 'auth-refresh',
  // 空 = 该用户**全部已启用账号**（`Schedule::accounts` 的既有语义 ✓）
  accounts: [],
  // 每天 03:00（东八区墙钟 ⇒ 6 段表达式 ✓，与 `daily_expr` 的产物同形 ✓）
  expr: '0 0 3 * * *',
  timezone: 'Asia/Shanghai',
  body: {},
  enabled: true,
  lastFireMs: 1790581000000,
  // ⚠ 凭据刷新**不产生 `RunRecord`** ⇒ `lastRunMs` 恒为 `null`（后端传 `None` ✓）：
  // 界面上「上次执行」显示成"从未执行"是**对的**，真实结论在 `lastStatus`/`lastNote` 上 ✓
  //（照真后端写，别为了好看编一个时刻 ✗）。
  lastRunMs: null,
  lastStatus: 'ok',
  lastError: null,
  lastNote: '未到提前量，跳过（还剩 12 天）',
  runCount: 7,
  nextFireAt: 1790672400000,
  pushRules: [],
  pushEvents: [],
  managedBy: 'auth-refresh',
  managedWanted: true,
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
  // 绑了一条推送规则 ⇒ 「推送」页那张规则卡的「绑定」列要能显示出它 ✓
  // （「没绑 = 不生效」是那一块最要紧的一句话，装置里两种都要有）
  pushRules: ['rule-1'],
  // ⚠ 给**非空**的一种：编辑时那排勾选框必须把存着的两个**勾上**（回填那条路 ✓）
  pushEvents: ['runFailed', 'runOk'],
}

/**
 * 第二条编排。
 *
 * ⚠ **为什么非要两条**：定时页那个「要跑的编排」下拉要钉住的是"**列出用户自己的编排、
 * 显示名字、能挑**" ✓ —— 装置里只有一条的话，「把第一条硬编码进去」那种实现**照样过** ✗
 * （本仓在"装置测不到 = 没测"上栽过）。
 *
 * ⚠ 它**不绑任何规则**、也**不绑任何定时** ⇒ 推送页那几条"绑着谁 / 没绑给谁"的断言
 * 一条都不受影响 ✓（`push-page.tsx` 的引用列是遍历这份清单算出来的 ✓）。
 * 它排在 `PIPELINE` **后面** ⇒ 编排页里"第一张卡"仍然是「早班」✓（门禁靠 `clickText`
 * 取第一个「编辑」✓）。
 */
const PIPELINE2 = {
  id: 'pipe-2',
  name: '晚班',
  accounts: [],
  accountMode: 'parallel',
  stopOnError: true,
  steps: [{ task: 'receive', continueOnError: false }],
  enabled: true,
  lastRunMs: null,
  pushRules: [],
  pushEvents: [],
}

/**
 * 推送规则（2026-10-08 用户口径「推翻重做」：**规则自带平台与凭据** ✓）。
 *
 * ⚠ 三条是**故意配成三种形态**的，正好把「推送」页左栏那几格的显示都覆盖到：
 * ① 绑在编排上、**两个平台**（其中一个是带密钥的 webhook URL ⇒ 打码那条路 ✓）、启用；
 * ② **没绑**、启用、`digest: perRun`；
 * ③ 绑在定时上、**已停用**、`digest: perStep`（汇总方式那一格要能看出区别 ✓）。
 *
 * 字段照 `web/src/api.ts` 的 `PushRule`（camelCase）✓ —— 少一格的表现是
 * 那一格静默变默认值，而界面上看不出来。
 *
 * ⚠ **规则里没有** `events` / `channels` / `slowFactor` / `failStreak` 了 ✗
 * （那些归**行为**那一侧 ✓）—— 夹具留着它们只会让"界面没读"变成看不见的谎话 ✓。
 * ⚠ 也**没有** `quiet`（静默时段）✗ —— 2026-10-08 用户口径「不要静默时间」，整块删了 ✓。
 */
const PUSH_RULES = [
  {
    id: 'rule-1',
    name: '失败才叫我',
    config: {
      title: 'mcloud 运行推送',
      onlyError: true,
      minLevel: 'info',
      pushplus: { token: 'fixture-pushplus-token' },
      workWeixinBot: { url: 'https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=fixture-key' },
    },
    digest: 'perRun',
    enabled: true,
    owner: 'u-1',
  },
  {
    id: 'rule-2',
    name: '夜里别吵',
    config: {
      title: '夜里别吵',
      onlyError: false,
      minLevel: 'warn',
      bark: { key: 'fixture-bark-key' },
    },
    digest: 'perRun',
    enabled: true,
    owner: 'u-1',
  },
  {
    id: 'rule-3',
    name: '什么都推（先停用）',
    config: {
      title: '什么都推',
      onlyError: false,
      minLevel: 'debug',
      dingTalk: { token: 'fixture-dingtalk-token' },
      email: { host: 'smtp.example.com', from: 'a@example.com', pass: 'fixture-mail-pass' },
    },
    digest: 'perStep',
    enabled: false,
    owner: 'u-1',
  },
]

/**
 * 事件清单。**照后端 `PushEvent::ALL` 的顺序与 `default_selected()` 抄**。
 *
 * ⚠ 2026-10-08 起每一格多一个 `kind`（`run` = 运行类、`panel` = 面板行为类 ✓）——
 * 界面按它把事件分两处：定时/编排的结局条件只画 `run` ✓、
 * 「设置 → 事件推送」那四行只画 `panel` ✓。少这一格的表现是**两处都画全部** ✗。
 */
const PUSH_EVENTS = [
  { key: 'runFailed', label: '运行失败', defaultSelected: true, kind: 'run' },
  { key: 'runPartial', label: '部分成功', defaultSelected: true, kind: 'run' },
  { key: 'runOk', label: '运行成功', defaultSelected: false, kind: 'run' },
  { key: 'runCancelled', label: '运行被中止', defaultSelected: false, kind: 'run' },
  // ⚠ 面板类这四条的字面量照后端 `PushEvent::label()` 抄 ✓（不是"登录新设备"——
  // 那是**行为** `BehaviorKey::label()` 的措辞 ✗）：界面上的行标题读的就是这里的 label ✓，
  // 夹具写另一个措辞，门禁就会去钉一个真后端永远不会出现的字符串 ✗。
  { key: 'loginNew', label: 'WebUI 新登录', defaultSelected: false, kind: 'panel' },
  { key: 'loginFailed', label: '登录连续失败', defaultSelected: true, kind: 'panel' },
  { key: 'perfSlow', label: '耗时异常', defaultSelected: true, kind: 'panel' },
  { key: 'perfFailStreak', label: '连续失败', defaultSelected: true, kind: 'panel' },
]

/** 渠道清单。照 `PushChannelKey::ALL` 的顺序与文案抄。 */
const PUSH_CHANNELS = [
  { key: 'pushPlus', label: 'PushPlus' },
  { key: 'serverChan', label: 'Server 酱' },
  { key: 'workWeixin', label: '企业微信应用' },
  { key: 'workWeixinBot', label: '企业微信机器人' },
  { key: 'tgBot', label: 'Telegram Bot' },
  { key: 'bark', label: 'Bark' },
  { key: 'twoIm', label: '双卡短信' },
  { key: 'dingTalk', label: '钉钉机器人' },
  { key: 'email', label: '邮件' },
  { key: 'customPost', label: '自定义 POST' },
]

/**
 * 推送规则那份**有状态**的值。
 *
 * 为什么有状态：这一层最要紧的闭环是「建一条 → 它出现在表里」。回一份静态数组
 * 只能证明"弹窗画得对"，证明不了保存那条路 —— 与 `taskCfg` / `subState` 同一条理由 ✓。
 */
const pushRulesState = { rules: [] }
const resetPushRulesState = () => {
  // ⚠ **深拷贝**：`config` 是嵌套对象，浅拷贝会让夹具与断言共享同一份引用
  // （界面改了它，下一轮 `reset` 拿到的还是被改过的那份 ✗）。
  pushRulesState.rules = JSON.parse(JSON.stringify(PUSH_RULES))
}

/**
 * 四个**面板行为**的绑定那份**有状态**的值（`GET/PUT /api/push/panel`）。
 *
 * ⚠⚠ 2026-10-08 用户口径「推翻重做」：`/api/push/config`（用户级渠道配置）**删掉了** ✗
 * —— 凭据现在挂在每条规则自己身上（`PushRule.config` ✓）。
 * 取而代之的是这一份：**行为选规则**（用户口径「一个行为可以选择多个规则」✓）。
 *
 * ⚠ 它必须**有状态**：`PUT` 之后 `GET` 要读得到 —— 否则"保存生效"这条根本量不出来，
 * 而"回填 PUT 的响应"那种写法照样绿 ✗（与 `pushRulesState` 同一条理由 ✓）。
 *
 * ⚠⚠ **形状照真后端**（`mcloud-core::push::BehaviorBinding` ✓，2026-10-08 落盘的那一份）：
 * * `rules` 空时后端是 `skip_serializing_if = "Vec::is_empty"` ⇒ **键压根不在** ✗
 *   ⇒ 夹具里那三行**故意不给 `rules`**，用来钉住"读的时候 `?? []`"那条（不写就会
 *   在运行时炸 `.includes`，而类型检查看不出来 ✗）；
 * * `slowFactor` / `failStreak` 是**可空**的，且**只在对应那一行有意义** ✓
 *   （`None` = 用户没动过 ⇒ 跟随内置默认 3.0 / 3 ✓）。
 *
 * 初始值：`perfFailStreak` 故意绑上 `rule-1`（它同时绑在编排上 ⇒
 * 「推送」页那一列要能把两种绑定**都**列出来 ✓）；其余三行**不绑** ✓
 * （`rule-2` 因此保持"未绑定"，那一格的反例还在 ✓）。
 * 节流给的是**保守默认**（`api.ts::PANEL_THROTTLE_DEFAULTS` ✓）。
 */
const PUSH_PANEL = {
  loginNew: { throttle: { mode: 'every', cooldownSecs: 1800, dailyCap: 10 } },
  loginFailed: { throttle: { mode: 'every', cooldownSecs: 1800, dailyCap: 10 } },
  perfSlow: {
    throttle: { mode: 'onChange', cooldownSecs: 1800, dailyCap: 10 },
    // 明确设过的阈值 ⇒ 界面上那一格要回填成 2.5（不是空 ✗）
    slowFactor: 2.5,
  },
  perfFailStreak: {
    rules: ['rule-1'],
    throttle: { mode: 'onChange', cooldownSecs: 1800, dailyCap: 10 },
    // 同上：明确设过的条数 ⇒ 回填成 4 ✓
    failStreak: 4,
  },
}
const pushPanelState = { bindings: null }
const resetPushPanelState = () => {
  pushPanelState.bindings = JSON.parse(JSON.stringify(PUSH_PANEL))
}

/**
 * 「直播自动定时」（`GET/PUT /api/live-room/auto-schedule`）那份**有状态**的值。
 *
 * # 为什么必须有状态
 *
 * 这张卡最要紧的闭环是「改开关/账号 ⇒ `PUT` 重算 ⇒ 界面回填**新状态**」——
 * 回一份静态值只能证明"按钮点了有反应" ✗，证明不了用户要的那件事
 * （与 `pushPanelState` / `taskCfg` 同一条理由 ✓）。
 *
 * # 重算照**真后端**的语义（`mcloud-app::live_auto` ✓）
 *
 * `enabled = 用户想要 且 算得出下一场`；算不出 ⇒ `at: null` + `enabled: false`
 * + 一句 note ✓（用户口径：「查不到『下一场』时 ⇒ 停用（条目留着）」✓）。
 *
 * ⚠⚠ **"算得出下一场"在夹具里按账号给**：主力号有下一场、第二个号**没有** ✓ ——
 * 这样"只选第二个号"就能走到「没有下一场」那一态（`interact.mjs` ④ ✓）。
 * 真后端今天池是**实例级**的 ⇒ 两个号拿到同一个值（"取最早"退化成取那一个 ✓，
 * 见 `mcloud-core::live_auto` 的模块头）—— 夹具在这里**走在后端前面**一步，
 * 等后端真按账号算时这条正好也覆盖到 ✓。这一点写在这里，免得后人以为夹具写错了 ✗。
 */
const AUTO_LIVE_AT = '2026-10-08 11:00:00'
/** 每个账号「下一场 + 1 小时」的墙钟串（`null` = 这个号**没有**下一场 ✓）。 */
const AUTO_LIVE_NEXT = { [ACCOUNT.id]: AUTO_LIVE_AT, [ACCOUNT2.id]: null }
const liveAutoState = { wanted: true, accounts: [] }
const resetLiveAutoState = () => {
  liveAutoState.wanted = true
  // 空数组 = **全部已启用账号**（`Schedule::accounts` 的语义 ✓，不是"一个都没选"✗）
  liveAutoState.accounts = []
}
/** 把当前状态按后端的口径算一遍（`GET` 与 `PUT` 的返回**同一个形状** ✓）。 */
const liveAutoView = () => {
  const ids = liveAutoState.accounts.length > 0 ? liveAutoState.accounts : Object.keys(AUTO_LIVE_NEXT)
  // 取**最早**那一场（字典序 = 时间序 ✓，与后端 `earliest_plus_one_hour` 同口径 ✓）
  const at =
    ids
      .map((id) => AUTO_LIVE_NEXT[id])
      .filter((v) => typeof v === 'string' && v !== '')
      .sort()[0] ?? null
  const enabled = liveAutoState.wanted && at !== null
  const note = !liveAutoState.wanted
    ? '已按你的设置关闭（定时条目留着，随时可以再打开）'
    : at === null
      ? '还没算出下一场（口令池里没有未来的场次）⇒ 已停用，条目留着，下次算出来自动启用'
      : null
  // ⚠ 停用时 `at` 必须是 `null`（后端 `read_auto_state` 就是这么给的 ✓）——
  // 给一个"看着像时刻"的值就是在骗人 ✗
  return { enabled, at: enabled ? at : null, accounts: liveAutoState.accounts, note }
}

/**
 * 定时那份**有状态**的值（`GET/PUT/DELETE /api/schedules…`）。
 *
 * # 为什么必须要有状态（2026-10-11 加）
 *
 * 定时页这一轮新增的闭环是「**选到编排 ⇒ 建一条 ⇒ 它出现在列表里 ⇒ 能删**」✓
 * （用户口径：「在定时界面找不到编排任务」✓）。以前 `GET /api/schedules` 是
 * `FIXTURES` 里一份**静态数组**、而 `PUT` 只对 `sch-1` 一条精确路径做了回显 ⇒
 * 新建那条（id 由前端生成 ✓）**根本没有装置能接** ✗ ⇒ "建出来的那条出现在列表里"
 * 这句话**量不出来**（回显一个响应只能证明"按钮点了有反应"✗，本仓在这一点上栽过）。
 *
 * # 语义照真后端（`crates/mcloud-server/src/routes/schedules.rs` ✓）
 *
 * * `PUT` 是 **upsert**：同 id 覆盖、没有就新增 ✓；
 * * **服务端那几格不许前端覆盖**（`owner` / `managedBy` / 记账字段 ✓）——
 *   所以覆盖是"老行打底 + 请求体那几格" ✓，不是整份替换 ✗；
 * * `task: "pipeline"` 时 **`body.pipelineId` 必须在** ✓（否则回 400 + 人话，
 *   就是界面上"选了编排却没挑是哪一条"兜住的那一句 ✓）。
 */
const schedState = { schedules: [] }
const resetSchedState = () => {
  // 深拷贝：`body` / `accounts` / `pushRules` 都是嵌套的 ⇒ 浅拷贝会让夹具与断言共享引用 ✗
  schedState.schedules = JSON.parse(JSON.stringify([SCHEDULE, AUTO_SCHEDULE, REFRESH_SCHEDULE]))
}
/** 新建一条时服务端**自己**补的那几格（`nextFireAt` 由服务端算 ✓，别让页面猜 ✗）。 */
const schedDefaults = (id) => ({
  id,
  lastFireMs: null,
  lastRunMs: null,
  lastStatus: null,
  lastError: null,
  // ⚠ 这一格照真后端写（`Schedule` 那一列没有 `skip_serializing_if` ⇒ **总是在** ✓）：
  // 少一格的表现是页面读成 `undefined` —— 而"空就不显示"恰好把它兜住了 ✗
  // ⇒ 装置里少了它，页面渲染得**看不出来**（那正是"装置测不到 = 没测" ✗）。
  lastNote: null,
  runCount: 0,
  // 与 `SCHEDULE` 同一个值：装置里"下次执行"是个**固定**时刻，
  // 用 `Date.now()` 会让每次跑门禁渲染出来的东西都不一样 ✗
  nextFireAt: SCHEDULE.nextFireAt,
})

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
  // ⚠ `isAdmin: true` 是**照实**给的（2026-10-06 加的那一格 ✓）：门禁全程按管理员跑
  // （设置页那些实例级动作、日志页的「运维」页签都只对管理员铺 ✓）——
  // 不写这一格的话 `currentIsAdmin` 默认 false ⇒ 界面上少一颗按钮，
  // 而"少一颗按钮"不会让任何断言变红 ⇒ 又一处**静默空转** ✗。
  '/api/session': { userId: 'u-1', name: 'admin', kind: 'web', isAdmin: true },
  // 账户安全（改口令 / 策略）与存储信息：设置页那两块要它们
  '/api/admin/security': { minPasswordLen: 8, sessionTtlHours: 168, passwordScheme: 'sha256' },
  /**
   * 存储信息（**静态**一份，给 `login` / `setup` 两个变体兜底；`ready` 里那份是
   * **有状态**的 ✓，见 `storageState` / `VARIANTS.ready`）。
   *
   * ⚠ 形状与有状态那份**必须一致**（同一份 `storageView()` 算出来的 ✓）——
   * 两边各写一遍就是"某个变体里少一格、界面上少一块"的经典来源 ✗。
   */
  '/api/system/storage': storageView(),
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
  '/api/schedules': [SCHEDULE, AUTO_SCHEDULE, REFRESH_SCHEDULE],
  '/api/pipelines': [PIPELINE, PIPELINE2],
  '/api/accounts': [ACCOUNT, ACCOUNT2],
  /**
   * 推送的**两张纯清单**。
   *
   * ⚠ `GET /api/push/rules` 与 `GET/PUT /api/push/panel` **不在**这里：它们都是
   * **有状态**的（要能"建一条 → 它出现在表里"、"改一行 → 保存读得回来"），
   * 所以走 `VARIANTS.ready` 里的 handler ✓（`FIXTURES` 那一路的值是死的）。
   * 事件/渠道两张是纯清单，静态就对 ✓。
   */
  '/api/push/events': PUSH_EVENTS,
  '/api/push/channels': PUSH_CHANNELS,
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
   * ⚠⚠ 2026-10-08 起这里**没有 `notify` 那一格了** ✗（用户口径「**很显然我说的和云盘
   * 账户没有关系**」＋「**丢弃现有情况，重写**」）：推送凭据现在挂在**每条规则自己**身上
   * （`PushRule.config` ✓，见上面 `PUSH_RULES`），`/api/push/config` 那条接口**已删** ✗，
   * 不再挂账号、也不再挂登录用户 ✓。留着 `notify` 只会让界面"读到一个不存在的格子"
   * 而毫无感觉（本仓最忌讳的那种）。
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
    aiAvatarDailyLimit: 10,
  },
  '/api/accounts/13900000001/settings': {
    backupWaitSecs: 0,
    refreshTokenDays: 0,
    skipTasks: [],
    aiAvatarDailyLimit: 10,
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
     * 服务端日志（`/logs` 那一页）：**每次请求多两行** ✓，且**按契约分页** ✓。
     *
     * 为什么必须会长：那一页每 2 s 轮询一次，而「贴底时新行要跟到底部」这条
     * 只有内容真的变多才量得出来（见 `logsState`）✓。
     * ⚠ 分页那一半（`before` / `limit` / 筛选 ✓）走 [`sliceServerLogs`]：
     * 不切的话「加载更早」在装置里**永远取不到东西** ⇒ 那几条断言成了空转 ✗
     * （装置测不到 = 没测 ✓）。
     */
    'GET /api/logs': {
      handler: (_body, req) => {
        logsState.all.push(
          logLine(logsState.seq++, Date.now()),
          logLine(logsState.seq++, Date.now()),
        )
        const q = new URL(String(req?.url ?? ''), 'http://x').searchParams
        const num = (k) => (q.get(k) == null ? undefined : Number(q.get(k)))
        return {
          status: 200,
          body: sliceServerLogs(logsState.all, {
            limit: num('limit'),
            before: num('before'),
            level: q.get('level') ?? '',
            q: q.get('q') ?? '',
            category: q.get('category') ?? '',
          }),
        }
      },
    },
    /**
     * 兑换订阅：GET 读那份有状态的值、POST **toggle** 改它。
     *
     * ⚠ 装置这边也要按**真后端那条规则**实现（已订就取消、没订就追加）——
     * 否则门禁测的是"装置自己的逻辑"，而不是"服务端判切换"这件事 ✓。
     */
    'GET /api/exchange/subscriptions': {
      handler: () => ({ status: 200, body: { items: subState.items } }),
    },
    /**
     * 推送规则：读那份**有状态**的值 ✓。
     *
     * ⚠ `PUT` / `DELETE` 不在这里 —— 规则的 id 是**前端生成**的
     * （`rule-<base36>-<随机>` ✓），装置里没法按精确路径列一条 ⇒ 走下面
     * `serve()` 里的**前缀匹配**分支 ✓。
     */
    'GET /api/push/rules': {
      handler: () => ({ status: 200, body: pushRulesState.rules }),
    },
    /**
     * 面板行为的绑定（**行为选规则**，2026-10-08 用户口径「推翻重做」✓）：
     * `GET` 读那份有状态的值、`PUT` **整份替换**它 ✓。
     *
     * ⚠ 这里**必须**是有状态的：`PUT` 之后 `GET` 要读得到 ✓ —— 否则"保存生效"
     * 这条量不出来（"回填 PUT 的响应"那种写法照样绿 ✗）。
     *
     * ⚠ 真后端会校验 `rules` 里的 id 存不存在 —— 装置这一层只保证
     * "存进去什么、读出来就是什么"（校验的断言在 Rust 侧 ✓）。
     */
    'GET /api/push/panel': {
      handler: () => ({ status: 200, body: pushPanelState.bindings }),
    },
    'PUT /api/push/panel': {
      handler: (body) => {
        pushPanelState.bindings = body
        return { status: 200, body }
      },
    },
    /**
     * 「直播自动定时」（直播页那张卡 ✓）：`GET` 读那份有状态的值、
     * `PUT` 改开关/账号并**立刻重算**、回**新状态** ✓。
     *
     * ⚠ 缺席 = 保持原样（后端 `AutoLiveInput` 的双 `Option` ✓）：
     * `body.enabled` 不是布尔就不动意愿 ✓、`body.accounts` 不是数组就不动名单 ✓ ——
     * 混起来的话"只想改账号"那一次会把用户的意愿**静默改掉** ✗
     *（定时页上已经踩过一次，见 `routes/live_room.rs::AutoLiveInput` 的注释 ✓）。
     */
    'GET /api/live-room/auto-schedule': {
      handler: () => ({ status: 200, body: liveAutoView() }),
    },
    'PUT /api/live-room/auto-schedule': {
      handler: (body) => {
        if (body && typeof body.enabled === 'boolean') liveAutoState.wanted = body.enabled
        // ⚠ 给了 `[]` 也**要**接（= 清空 = 全部已启用账号 ✓），不能当"没给"✗
        if (body && Array.isArray(body.accounts)) liveAutoState.accounts = body.accounts
        return { status: 200, body: liveAutoView() }
      },
    },
    /**
     * 定时：读那份**有状态**的值 ✓（写口走 `serve()` 里的**前缀匹配**分支，
     * 因为新建时 id 由**前端生成** ⇒ 这里列不出来 ✗，见 `schedState` ✓）。
     */
    'GET /api/schedules': {
      handler: () => ({ status: 200, body: schedState.schedules }),
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
    /**
     * 手动刷新凭据（`POST /api/auth/refresh` ✓ 2026-10-11）。
     *
     * ⚠ 这一格要量的是**请求体里有没有 `force`**（断言在 `interact.mjs` ✓），
     * 不是服务端行为 —— 服务端那侧由 `crates/mcloud-server/tests/routes.rs` 钉 ✓。
     * 所以这里只回一份**形状对**的逐条结局（照 `web/src/api.ts` 的 `RefreshResponse` ✓），
     * 让界面把"toast + 重读列表"那条正常路径走完 ✓。
     *
     * ⚠ 按请求体回**对应的账号**：`{all:true}` 要回**每一个**（界面把逐条明细
     * 拼成 toast 第二行 ✓）—— 回死一份的话，"全部"那颗按钮那条路等于没验 ✗。
     */
    'POST /api/auth/refresh': {
      handler: (body) => {
        const ids = body && body.all === true ? acctState.accounts.map((a) => a.id) : [body?.account]
        const results = ids.filter(Boolean).map((id) => {
          const a = acctState.accounts.find((x) => x.id === id)
          return {
            account: id,
            label: a?.nickname ?? a?.label ?? id,
            kind: 'refreshed',
            newExpireMs: 1793000000000,
            // 手动这条路**一律强制**（`force: true` ✓）⇒ 逐条都带 `forced` ✓
            forced: true,
          }
        })
        return {
          status: 200,
          body: { results, refreshed: results.length, skipped: 0, manual: 0, failed: 0 },
        }
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
    /**
     * 存储信息 + **容量上限**（2026-10-09 用户口径「让用户设置最大大小」✓）：
     * `GET` 读那份有状态的值、`PUT` 改它并回**改完的即时状态** ✓。
     *
     * ⚠ 为什么必须有状态：门禁要钉的是"改了这些数 ⇒ 服务端记住了、卡片读数跟着变" ✓ ——
     * 回一份静态值就只能证明"按钮点了有反应" ✗（本仓在"装置测不到 = 没测"上栽过）。
     *
     * ⚠⚠ 校验照**冻结的契约**来（2026-10-09 后端改的 ✓）：**给哪格改哪格**、
     * **一格都不给**才回 **400**（"整体替换、少一格就 400"是**旧**语义 ✗）。
     * ⚠ 这一点在装置里很要紧：前端现在**五格一起发** ✓ ⇒ 只发了改过那一格的实现
     * 会在这里**留下旧值** ⇒ 门禁里"保存后五个框都是新值"那条当场红 ✓
     * （比"装置不卡"强：那样测的是"按钮有反应"，不是"值真的到了服务端" ✗）。
     * ⚠ 越界回 **400 + 人话**（`{error}` 是字符串 ✓，`api.ts` 读的就是它 ✓）。
     */
    'GET /api/system/storage': {
      handler: () => ({ status: 200, body: storageView() }),
    },
    'PUT /api/admin/storage': {
      handler: (body) => {
        const keys = Object.keys(LIMIT_RANGE)
        const given = keys.filter((k) => body != null && body[k] !== undefined)
        if (given.length === 0) {
          return {
            status: 400,
            body: { error: '至少要给一格上限（historyLimit / logLimitPerRun / logTotalMb / serverLogMemLines / serverLogTotalMb）', code: 'no_limit' },
          }
        }
        for (const k of given) {
          const v = body[k]
          const [min, max] = LIMIT_RANGE[k]
          if (typeof v !== 'number' || !Number.isInteger(v) || v < min || v > max) {
            return {
              status: 400,
              body: {
                error: `${k} 要在 ${min}~${max} 之间（收到 ${JSON.stringify(v)}）`,
                code: 'invalid_limit',
              },
            }
          }
        }
        // 只覆盖**给了的**那几格（其余保持原样 ✓ —— 这就是"给哪格改哪格" ✓）
        for (const k of given) storageState.limits[k] = body[k]
        return { status: 200, body: storageView() }
      },
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
    resetLogsState()
    resetStorageState()
    resetPushRulesState()
    resetPushPanelState()
    resetLiveAutoState()
    resetSchedState()
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
      /**
       * ⚠⚠ `overrides.__inject`：**按路由注入一次失败**（不是路由 ⇒ `overrides[key]`
       * 永远查不到它 ✓，与 `__sseLogs` 同一套保留键手法 ✓）。
       *
       * 键是 `"METHOD /path"`，值两种：
       * * `{ drop: true }` —— **传输层直接断开**（不写任何响应，`destroy` 掉 socket ✓）：
       *   浏览器那边就是 `TypeError: Failed to fetch` ✓。**只有这条路**能复现
       *   "服务没起 / 正在重启"（在页面里把 `window.fetch` 换掉测的是我们自己的桩 ✗）；
       * * `{ status, body }` —— 照 `VARIANTS` 那条形状回一份错（例如 500 + 后端那句话 ✓）。
       *
       * ⚠ 开关是**按请求现读**的 ⇒ 脚本可以在**跑到一半**改它（`interact.mjs` 的
       * `pre` 钩子 ✓）。这是"连不上后端"与"HTTP 500 各显示什么"那两条钉子
       * 唯一能复现的地方 —— 前端那两条分支（`api.ts::request` 的 catch 与 `!ok`）
       * 只有真造出这两种失败才走得到 ✓。
       */
      const injected = overrides?.__inject?.[key]
      if (injected) {
        if (injected.drop) {
          // ⚠ 不写任何响应、直接拆连接：这才是浏览器眼里"连不上"的样子
          //（回 502 之类的**不是** —— 那会走 HTTP 错误那条路，测错分支 ✗）。
          req.socket.destroy()
          return
        }
        send(req, res, injected, false)
        return
      }
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
      /**
       * 运行事件流（SSE）。
       *
       * ⚠ 为什么必须**只发注释、不发事件**：
       * 它要造的只是"任务还在跑 ⇒ 连接开着"这个状态。真发 `log` 事件会让
       * 日志面板多出几行 ⇒ 门禁里那些 DOM 快照比较跟着变 ✗（而它们要测的是别的事）。
       * SSE 规范里 `: 注释行` 是心跳，**不会**触发任何事件 ✓ 所以这里正合适。
       *
       * ⚠ 加上它的理由：以前夹具没有这条 ⇒ 应用点「运行」后 EventSource 拿到 404、
       * 流根本没开成 ⇒ **"边跑任务边刷新"这个场景一条都复现不出来** ✗，
       * 而这正是用户报"刷新后 JS 堆递增"时的真实用法（见 task-log 第 34 行）。
       */
      const events = url.match(/^\/api\/runs\/(\d+)\/events$/)
      if (events) {
        res.writeHead(200, {
          'content-type': 'text/event-stream',
          'cache-control': 'no-cache',
          connection: 'keep-alive',
        })
        const beat = setInterval(() => res.write(': ping\n\n'), 1000)
        req.on('close', () => clearInterval(beat))
        /**
         * ⚠ 上面那条「只发注释、不发事件」是**默认**；这里留一个**按变体开关**的口子：
         * `overrides.__sseLogs`（不是路由，`overrides[key]` 永远查不到它 ✓）打开后才真发
         * `log` 事件 —— `bulk.mjs` 那条「**流式追加**日志时老行要被搬动而不是重建」的钉子
         * 需要它 ✓。开关只挂在 `bulk.mjs` 自己的 `__bulk` 变体上 ⇒ `interact.mjs` 的
         * `ready`/`login`/`setup`（它们逐帧在比**整页快照** ✗）一条都不受影响 ✓。
         *
         * 事件形状照 `web/src/api.ts` 的 `RunEvent`（`type:'log'` + `level`/`kind`/`message`/`at_ms` ✓）。
         */
        const sse = overrides?.__sseLogs
        if (sse) {
          const runId = Number(events[1])
          const timers = []
          for (let i = 0; i < (sse.lines ?? 4); i++) {
            timers.push(
              setTimeout(() => {
                const ev = {
                  type: 'log',
                  run_id: runId,
                  level: 'info',
                  kind: i % 2 === 0 ? 'success' : 'info',
                  message: `夹具流式第 ${i} 行 —— 追加用（要能看见「新行进来、老行不动」✓）`,
                  at_ms: 1790581000000 + i * 1000,
                }
                res.write(`event: log\ndata: ${JSON.stringify(ev)}\n\n`)
              }, (sse.gapMs ?? 300) * (i + 1)),
            )
          }
          req.on('close', () => timers.forEach((t) => clearTimeout(t)))
        }
        return
      }
      /**
       * 定时的写口（`PUT` / `DELETE /api/schedules/{id}`）。
       *
       * ⚠ 为什么是**前缀匹配**而不是 `FIXTURES` / `VARIANTS` 里的一条精确路径：
       * 新建时 id 由**前端生成**（`sch-<base36>-<随机>` ✓，见
       * `schedules-page.tsx::newId`）—— 装置里根本列不出来 ✗。
       * 与下面推送规则那条**同一个理由** ✓（以前这里只对 `sch-1` 一条做了回显，
       * 于是"新建一条 ⇒ 它出现在列表里"**根本没被装置接住** ✗）。
       *
       * ⚠ 语义照真后端（`routes/schedules.rs::upsert` ✓）：`PUT` 是 upsert、
       * **服务端那几格不许前端覆盖**（老行打底 ✓）；`task: "pipeline"` 而
       * `body.pipelineId` 缺失 ⇒ **400 + 人话** ✓（界面上"选了编排却没挑是哪一条"
       * 那句校验就是照它写的 ✓）。`DELETE` 幂等回 `{ok:true}` ✓。
       */
      const schedMatch = url.match(/^\/api\/schedules\/([^/]+)$/)
      if (schedMatch) {
        const id = decodeURIComponent(schedMatch[1])
        if (req.method === 'DELETE') {
          const at = schedState.schedules.findIndex((s) => s.id === id)
          if (at >= 0) schedState.schedules.splice(at, 1)
          res.writeHead(200, { 'content-type': 'application/json' })
          res.end(JSON.stringify({ ok: true }))
          return
        }
        if (req.method === 'PUT') {
          let raw = ''
          req.on('data', (d) => {
            raw += d
          })
          req.on('end', () => {
            let b = null
            try {
              b = raw ? JSON.parse(raw) : null
            } catch {
              /* 非 JSON 体：按空体处理（真后端会回 400，装置只服务正常路径 ✓） */
            }
            const at = schedState.schedules.findIndex((s) => s.id === id)
            const prev = at >= 0 ? schedState.schedules[at] : schedDefaults(id)
            const body = b ?? {}
            if (body.task === 'pipeline' && !(body.body && typeof body.body.pipelineId === 'string')) {
              res.writeHead(400, { 'content-type': 'application/json' })
              res.end(
                JSON.stringify({
                  error: '这条定时要跑编排，但 body 里没有 pipelineId',
                  code: 'config',
                }),
              )
              return
            }
            // ⚠ 老行打底 + 请求体那几格（`owner` / `managedBy` 这类**服务端**的格
            // 前端压根不发 ⇒ 只有"打底"才留得住 ✓）
            const saved = {
              ...prev,
              id,
              name: body.name,
              task: body.task,
              accounts: body.accounts ?? [],
              expr: body.expr,
              timezone: body.timezone ?? 'Asia/Shanghai',
              body: body.body ?? {},
              enabled: body.enabled !== false,
              pushRules: body.pushRules ?? [],
              pushEvents: body.pushEvents ?? [],
            }
            if (at >= 0) schedState.schedules[at] = saved
            else schedState.schedules.push(saved)
            res.writeHead(200, { 'content-type': 'application/json' })
            res.end(JSON.stringify(saved))
          })
          return
        }
      }
      /**
       * 推送规则的写口（`PUT` / `DELETE /api/push/rules/{id}`）。
       *
       * ⚠ 为什么是**前缀匹配**而不是 `FIXTURES` / `VARIANTS` 里的一条精确路径：
       * id 是**前端生成的**（`rule-<base36>-<随机>` ✓，见 `push-page.tsx::newRuleId`）
       * —— 装置里根本列不出来 ✗。
       *
       * ⚠ 语义要**照真后端**：`PUT` 是 upsert（幂等：同 id 覆盖、没有就新增 ✓）、
       * 请求体里**没有 `id`/`owner`**（id 在路径里、owner 由服务端定 ✓）。
       * 装成"整体替换一个不存在的对象"就测不出"新建"这条路 ✓。
       */
      const ruleMatch = url.match(/^\/api\/push\/rules\/([^/]+)$/)
      if (ruleMatch) {
        const id = decodeURIComponent(ruleMatch[1])
        const at = pushRulesState.rules.findIndex((r) => r.id === id)
        if (req.method === 'DELETE') {
          if (at < 0) {
            // 别人的/不存在的**一样回 404**（后端不区分 ✓）
            res.writeHead(404, { 'content-type': 'application/json' })
            res.end(JSON.stringify({ error: `推送规则不存在：${id}`, code: 'not_found' }))
            return
          }
          pushRulesState.rules.splice(at, 1)
          res.writeHead(200, { 'content-type': 'application/json' })
          res.end(JSON.stringify({ ok: true }))
          return
        }
        if (req.method === 'PUT') {
          let raw = ''
          req.on('data', (d) => {
            raw += d
          })
          req.on('end', () => {
            let body = null
            try {
              body = raw ? JSON.parse(raw) : null
            } catch {
              /* 非 JSON：下面按空体处理（与真后端一样回 400 更严，但装置只服务正常路径） */
            }
            const saved = { ...(body ?? {}), id, owner: 'u-1' }
            if (at < 0) pushRulesState.rules.push(saved)
            else pushRulesState.rules[at] = saved
            res.writeHead(200, { 'content-type': 'application/json' })
            res.end(JSON.stringify(saved))
          })
          return
        }
      }
      /**
       * 某次运行的日志（`GET /api/runs/{id}/logs?limit=&before=&after=`）—— **分页**取
       * （2026-10-09 用户口径「大日志做虚拟加载或者分页，避免一次加载太多」✓）。
       *
       * ⚠ 为什么走**前缀匹配**而不是 `FIXTURES` / `VARIANTS` 里的精确路径：run id 是
       * 列表里来的、还会被别的变体重写（`bulk.mjs` 就有自己那份 100 条运行 ✓）⇒
       * 列不出来 ✗。与下面推送规则那条同一个理由 ✓。
       *
       * ⚠ 变体**可以按精确路径接管**（`overrides['GET /api/runs/9000/logs']` ✓）——
       * `bulk.mjs` 就用它换成自己那份很长的日志（窗口化那条钉子要的是"够长"，
       * 与这里这份"分页"夹具是两件事 ✓）。不接管才用这份 800 行 ✓。
       */
      const runLogsMatch = url.match(/^\/api\/runs\/(\d+)\/logs$/)
      if (runLogsMatch) {
        const override = overrides[`GET ${url}`]
        if (override) {
          send(req, res, override, false)
          return
        }
        const q = new URL(String(req.url ?? ''), 'http://x').searchParams
        const num = (k) => (q.get(k) == null ? undefined : Number(q.get(k)))
        res.writeHead(200, { 'content-type': 'application/json' })
        res.end(
          JSON.stringify(
            sliceRunLogs(fixtureRunLogs(Number(runLogsMatch[1])), {
              limit: num('limit'),
              before: num('before'),
              after: num('after'),
            }),
          ),
        )
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

/**
 * 应用认得的路径：`parsePath` 比的是 `/settings` 这种**带斜杠**的路径
 * （2026-10-03 起去掉了 `#` —— 用户口径「先去掉#号」）。
 */
export const routeUrl = (route) => `/${String(route).replace(/^#?\/?/, '')}`

/**
 * "跳到某个页面"的求值片段 —— **模拟 `ui/router.ts` 的 `navigate()`**。
 *
 * ⚠ 必须**两件事一起做**：`pushState` 改地址，再补发一次 `popstate`。
 * 只改地址的话应用不会切页（`pushState` 自己不发事件），
 * 而夹具脚本会以为"跳过去了"然后去断言一个还没渲染的页面 ✗。
 */
export const gotoJs = (route) =>
  `(() => { const to = ${JSON.stringify(routeUrl(route))}; history.pushState({}, '', to); window.dispatchEvent(new PopStateEvent('popstate')); return location.pathname })()`

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
      // ⚠ 公共前缀（`--headless` [+ `--no-sandbox`] `--disable-gpu`）只有一份来源：
      // `../scripts/chrome-flags.mjs`。别再往这里塞机器相关的 flag —— 各处各写一份的结果是
      // 2026-10-05 三条写入线相互覆盖、相互还原，打了三次架 ✗。
      ...headlessFlags(),
      '--hide-scrollbars', '--window-size=1280,900',
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
  /**
   * ⚠⚠ `profile` **必须**显式传 —— 它没有默认值，漏传就成了
   * `--user-data-dir=undefined`，而 Chrome 会把 `undefined` 当成**相对当前目录**的
   * 路径 ⇒ 在跑脚本的那个目录（通常就是 `web/`）下建出一个叫 `undefined` 的
   * Chrome 用户目录（实测 2 MB，`Default/`、`GPUCache/`、`Session Storage/` 一整套）。
   *
   * 2026-10-02 真攒出来过一个：`git status` 里躺着 `?? web/undefined/`，
   * 而当时正要 `git add -A` 做发布提交 —— 差一点就把 2 MB 垃圾提进仓库。
   * 静默造垃圾比报错贵得多 ⇒ 这里早失败，并说清为什么。
   */
  if (!profile) {
    throw new Error(
      'openSession 必须传 profile（漏传时 chrome 会在当前目录下建一个叫 "undefined" 的用户目录）',
    )
  }
  // ⚠ 这里用 `routeUrl` 归一化：调用方传 `tasks` / `/tasks` / `#/tasks` 都行。
  // 直接拼 `${port}/${route}` 的话，传 `/tasks` 会得到 `//tasks`（双斜杠）——
  // 静态服务器认不出那个路径，于是发回落页，夹具却看不出来（页面照样能开）。
  const url = `http://127.0.0.1:${port}${routeUrl(route)}`
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
