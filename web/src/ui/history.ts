/**
 * 历史与日志（`#/history`）的**纯**显示规则：状态文案、云朵变化、筛选项、分页步进。
 *
 * # 为什么不并进 `run-stream.ts`
 *
 * 那份管的是"一条事件说了什么"（SSE 解析）；这里管的是"一条**历史记录**怎么显示、
 * 列表怎么筛"。两者唯一的交集是 `RunRecord`，而它的形状由 `api.ts` 对着后端
 * `routes.go` 的 `runPayload` 定死。
 *
 * # ⭐ 本文件最要紧的一条：`status` **不是**业务结论
 *
 * 后端 `runs.status` 只有三态（`running` / `done` / `failed`），它是**生命周期**：
 * `done` 只表示"跑完了"，可以是全成 / 部分成 / 需人工三种 ——
 * `core.RunOutcome.ToLegacyStatus` 把后两种都映射成 `done`，注释里那句原话是
 * "把它写成 Failed 会让历史里出现一堆'失败'，而它们其实只是在等用户"。
 *
 * 所以 [statusLabel] 刻意写「已结束」而**不是**「成功」，[statusBadge] 对 `done`
 * 也刻意给中性灰而**不是**绿：业务结论只出现在事件流的 `finished` 里，
 * 那一份由 `run-stream.ts` 的 `outcomeLabel` / `outcomeBadge` 出。
 * 在列表里替后端补一个"成功/失败"，就是编造一个它没给过的结论。
 *
 * # 两个调用点
 *
 *   - `pages/history-page.tsx`（列表徽标、筛选项）
 *   - `app.tsx` 侧栏底部那一行「最近执行 … · <状态>」
 *
 * 那张表**只此一份**：早先 `app.tsx` 自己抄了一张三行的，加第四档时两处必然漂移，
 * 而漂移的表现是"侧栏显示 `running` 这种原文码、历史页显示中文"。
 */

import type { RunRecord } from '../api'

/** 生命周期三态 → 中文。查表而不是 `if` 链（设计指南 §9.1①）。 */
export const RUN_STATUS_LABEL: Record<string, string> = {
  running: '进行中',
  done: '已结束',
  failed: '失败',
}

/** 未知值回显原文，不替后端编一个中文名（与 `outcomeLabel` 同一条纪律）。 */
export const statusLabel = (status: string): string => RUN_STATUS_LABEL[status] ?? status

/**
 * 生命周期 → badge 类。
 *
 * ⚠ 基类 `badge` **必须**带上：`badge-soft` 只是修饰。只给修饰类会渲染成一串
 * 裸文字，而 typecheck 与 build **都还是绿的**（组件指南 §7.4 补记③ 那条坑）。
 *
 * ⚠ `done` 是中性灰不是绿 —— 理由见文件头。
 */
export const RUN_STATUS_BADGE: Record<string, string> = {
  running: 'badge badge-soft badge-info',
  done: 'badge badge-soft',
  failed: 'badge badge-soft badge-error',
}

export const statusBadge = (status: string): string =>
  RUN_STATUS_BADGE[status] ?? 'badge badge-soft'

/**
 * 云朵变化。**只给符号，不给颜色**。
 *
 * 与 `dashboard-page.tsx` 里那个 `gainText` 是同一条口径，那边的注释写明了理由：
 * "云朵减少多半是正常花费，把它染成'异常色'会让人误以为账号出问题了"。
 * （那边是页内闭包、这里是导出的一份，两处同形；要统一时两边都该搬进 `ui/`。）
 *
 * ⚠ 顺带一条容易写反的：设计指南 §2.4 是**涨红跌绿**（`text-error` 是涨），
 * 与 daisyUI 直觉相反。本函数因此**干脆不给颜色类**，免得踩那一脚。
 */
export const cloudGainText = (gained: number | null): string =>
  gained === null ? '—' : gained > 0 ? `+${gained}` : String(gained)

/**
 * 这一趟用了多久（毫秒）。
 *
 * 还在跑就拿 `nowMs` 当终点 —— 那是组件指南 §6.3 要的"显示**已用时长**"。
 * 时间解不出来时给 0 而不是 NaN：NaN 会让 `formatElapsed` 输出 "NaN 秒"。
 */
export const elapsedMillis = (run: RunRecord, nowMs: number): number => {
  const start = Date.parse(run.startedAt)
  if (Number.isNaN(start)) return 0
  const end = run.finishedAt === null ? nowMs : Date.parse(run.finishedAt)
  if (Number.isNaN(end)) return 0
  return Math.max(0, end - start)
}

/**
 * 需人工介入的地点 → 中文。后端给的是枚举码（`core.ManualTarget`），界面只显示文案。
 *
 * ⚠ `unspecified` 是**刻意保留**的一档（后端注释：抓包与旧文案里总有归不进去的情形，
 * 强行归类会把"我们没搞清"伪装成"我们知道"）⇒ 这里也照实写「未指明」，
 * 不要为了好看把它并进别的档。
 */
export const MANUAL_TARGET_LABEL: Record<string, string> = {
  mobile_app: '移动云盘 App',
  web: '移动云盘网页端',
  webui: '本系统 WebUI',
  cli: '本系统命令行',
  unspecified: '未指明',
}

export const manualTargetLabel = (target: string): string => MANUAL_TARGET_LABEL[target] ?? target

/** 一个筛选项：能力码 + 显示名 + 当前页里的条数。 */
export interface CapabilityOption {
  code: string
  /** 能力的显示名；拿不到声明时回落成 code（**不**编一个中文名）。 */
  label: string
  count: number
}

/**
 * 由**当前这一页记录**反推筛选项，而不是去要一份全量能力表。
 *
 * 理由：筛选项里出现一个"这次一条记录都没有"的能力，点进去就是空列表 ——
 * 那是把用户往死路上带。反过来"这一页有记录的能力"必然点得进去，计数也顺手有了。
 *
 * 排序按条数降序：用得多的能力排前面，省一次找。
 */
export const capabilityOptions = (
  runs: readonly RunRecord[],
  titles: Record<string, string>,
): CapabilityOption[] => {
  const counts = new Map<string, number>()
  for (const run of runs) {
    counts.set(run.capability, (counts.get(run.capability) ?? 0) + 1)
  }
  return [...counts.entries()]
    .map(([code, count]) => ({ code, label: titles[code] ?? code, count }))
    .sort((a, b) => b.count - a.count || a.code.localeCompare(b.code))
}

/**
 * 出现过的生命周期值。
 *
 * 先按 [RUN_STATUS_LABEL] 的键序（读起来才是"进行中 → 已结束 → 失败"这个自然的顺序），
 * 再把**不认识**的排到后面 —— 后端将来加一档时，那一档要能筛得到，
 * 而不是静默消失（"筛选项里没有"和"一条都没有"在界面上长得一样）。
 */
export const statusOptions = (runs: readonly RunRecord[]): string[] => {
  const seen = new Set(runs.map((run) => run.status))
  const known = Object.keys(RUN_STATUS_LABEL).filter((status) => seen.has(status))
  const unknown = [...seen].filter((status) => !(status in RUN_STATUS_LABEL)).sort()
  return [...known, ...unknown]
}

/** 列表筛选。空串 = 不筛。 */
export const filterRuns = (
  runs: readonly RunRecord[],
  capability: string,
  status: string,
): RunRecord[] =>
  runs.filter(
    (run) =>
      (capability === '' || run.capability === capability) &&
      (status === '' || run.status === status),
  )

/**
 * 日志区**一行都没有**时里面写什么。
 *
 * ⚠ 三态必须分开说，不能用一句"暂无日志"糊过去 —— 三件事的下一步完全不同：
 *
 *   - `pending`：还在连，等着就行；
 *   - `lost`：**没取到**，而原因在日志区外面那条提示里 ⇒ 这里只能指过去，
 *     写"没有日志输出"是**错的**（那是"跑过、但确实没打日志"，与"没拿到"是两回事）；
 *   - 其余（`open` / `done`）：真的没有日志输出，比如那趟在第一步就失败了。
 *
 * 2026-09-23 实机踩到：`lost` 那一支原先也走"这一趟没有日志输出"，
 * 于是同一张卡片里上面写"没有日志输出"、下面那条 alert 写"没有回放出任何日志"，两句打架。
 */
export const emptyLogText = (phase: string): string => {
  if (phase === 'pending') return '正在连接事件流…'
  if (phase === 'lost') return '没有取到日志 —— 原因见下面那条说明。'
  return '这一趟没有日志输出。'
}

/** 步进一次多要多少条（对位 `api.ts` 的 `listRuns(limit)`）。 */
export const LIMIT_STEP = 50

/**
 * 一次最多能要多少条 —— 对位后端 `routes.go` 的 `maxRuns`。
 *
 * ⚠ 这是个**必须跟着后端改**的常量：`clampLimit` 会把超出的值夹回 200，
 * 于是"再看 50 条"点到第 5 次之后就不再变多，而界面会以为还有更多。
 * [nextLimit] 到顶就回 null，界面据此把那枚按钮收起来。
 */
export const LIMIT_MAX = 200

/** 下一档 limit；已到顶回 null。 */
export const nextLimit = (current: number): number | null => {
  if (current >= LIMIT_MAX) return null
  return Math.min(current + LIMIT_STEP, LIMIT_MAX)
}
