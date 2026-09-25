/**
 * 预设运行列表（后端叫「任务组合」/ pipeline）的草稿态与纯函数 —— 不依赖 Vue（§9 约定 2）。
 *
 * # 为什么这一族要有"草稿"这一层
 *
 * 后端存的是一份**有序步骤**（`core.Pipeline`：`steps[{task, body, continue_on_error}]`
 * + 组合级的账号列表 / 失败策略），界面要的是一句"我勾了这几条、按这个顺序、改过这两个参数"。
 * 两者差的不只是字段名：库里的 `task` 是能力 code，界面上还得有标题、得有"这一行现在
 * 是不是正在被编辑"、还得容得下**能力表里已经查不到的那些存量 code**（改了声明、
 * 或从旧数据导入）。所以中间放一份 `DraftStep`，序列化只在
 * [payloadFromRunList] / [runListFromPipeline] 这**两个**边界发生 ——
 * 与 `ui/notify-form.ts` 同一形状：视图态 ↔ 出网体的来回一次，只有一处会写错。
 *
 * # 两条"不报错但会洗数据"的线，都靠这一层兜住
 *
 * ① 写侧键名是 **snake_case**（`account_ids` / `stop_on_error` / `continue_on_error`），
 * 读侧外层是 **camelCase**（`accountIds` / `stopOnError`）。写成 `accountIds` 提交，
 * Go 那边解不进 ⇒ **200 照样回**，只是那一行的账号列表被洗成空 —— 而空 = 跑全部账号。
 * ② 每一步 body 里的 `accounts` / `accountMode` **存了也不算**：执行时
 * `pipelineRequests` 会用组合级列表覆盖 `accounts`、用 `sequential` 硬写 `accountMode`
 * （`internal/server/pipeline_run.go:293-300`）。所以表单里**不给**这两格位置，
 * 账号只在清单级选一次 —— 界面上留一个"每步还能各选各的账号"的假控件，
 * 是让用户填了永远不生效的东西。
 */

import type {
  CapabilityField,
  CapabilityRow,
  PipelinePayload,
  PipelineRow,
  PipelineStepPayload,
} from '../api'
import type { Badge } from './task-groups'

/** 内置「一键全跑」的保留 id（后端 `PipelineRunAllID`）：库里**没有**它的行。 */
export const BUILT_IN_PIPELINE_ID = 'all'

/** 名字与 id 的长度上限，逐字对后端 `maxNameLen` / `maxIDLen`（超了是 400，界面先挡）。 */
export const MAX_PIPELINE_NAME = 100
export const MAX_PIPELINE_ID = 64

/** id 允许的字符集，对 `validateScheduleID`：字母、数字与 `- _ .`。 */
const ID_ALLOWED = /^[A-Za-z0-9._-]+$/

/**
 * 每一步 body 里**不该出现**的两个键：见文件头②。
 * 它们仍在能力声明里（`accountSelector()` 给的），单跑那一条路要用，组合里由清单级接管。
 */
export const LIST_LEVEL_PARAM_KEYS: readonly string[] = ['accounts', 'accountMode']

/** 清单里的一步。`uid` 只活在浏览器里：它是"哪一行正在被编辑"的指针，不出网。 */
export interface DraftStep {
  uid: string
  /** 能力 code，= 后端的 `task`。 */
  code: string
  /**
   * 能力表里查不到这个 code 时**仍然要显示**的那一份（库里存量）。
   * 判据：找不到能力就写"未知能力 xxx"，而不是写空标题 —— 空的行会让人以为清单坏了，
   * 而真实情况是这条能力被摘掉了，用户要看的就是"哪一步"。
   */
  title: string
  /** 这一步传给能力的参数，键名必须是声明里的。 */
  body: Record<string, unknown>
  /** 三态：`null` = 跟随组合级的 `stopOnError`。 */
  continueOnError: boolean | null
  /**
   * 库里这一格**原本不是对象**时的原文（`PipelineStep.Body` 接受任意 JSON 值）。
   * 空串 = 正常。非空 ⇒ 界面上要说清楚"这一步的参数没能带进表单，保存后不再保留"。
   */
  rawBody: string
}

/** 整份清单（一张待保存的预设）。 */
export interface RunList {
  /** 已有的 id；空串 = 还没存过（新建），保存时由 [nextPipelineId] 造一个。 */
  id: string
  name: string
  description: string
  /** 空数组 = 全部启用账号（与能力声明那句 help 同义，不是"0 个目标"）。 */
  accountIds: string[]
  stopOnError: boolean
  steps: DraftStep[]
  /** 载入自内置那一版 ⇒ 界面要挡住"存回 `all`"（后端 400），改成另存。 */
  builtIn: boolean
  /** 载入自一份并行执行的老数据：组合只支持串行，保存会把它改成串行（界面上要说）。 */
  wasParallel: boolean
}

export const emptyRunList = (): RunList => ({
  id: '',
  name: '',
  description: '',
  accountIds: [],
  stopOnError: false,
  steps: [],
  builtIn: false,
  wasParallel: false,
})

// ─────────────────────────────────────────────────────────────────────────────
// 默认值与单步
// ─────────────────────────────────────────────────────────────────────────────

let uidCounter = 0

/**
 * 行内稳定 key。
 *
 * ⚠ 不能用数组下标：上/下移之后下标会变，"哪一行在编辑"会跟到别的行上去
 * （Vapor 没有重渲染，这种错不会在任何一次编译里露出来）。
 */
export const newStepUid = (): string => {
  uidCounter += 1
  return `s${Date.now().toString(36)}-${uidCounter.toString(36)}`
}

/**
 * 一个参数的初始值 —— 与任务中心那张表单**同一套**规则（原来写在页面里，
 * 现在清单也要造 body，两边必须一致，否则"卡片上看到的默认值"与"存进预设的默认值"分叉）。
 *
 * ⚠ `number` 与文本一样先给**空串**，但空串**不是**要送出去的值：出线时要按
 * "省略那一栏"处理，见 [stepBodyForWire]。类型也不在这里猜 —— 表单里存的是文本，
 * 换算放在唯一的那一个出口。
 */
export const defaultValueFor = (f: CapabilityField): unknown => {
  if (f.default !== undefined) return f.default
  if (f.kind === 'bool') return false
  if (f.kind === 'multi_select') return []
  return ''
}

/** [numberWire] 的三种结论：省略、给数字、给的不是数字。 */
export type NumberWire =
  | { kind: 'drop' }
  | { kind: 'value'; value: number }
  | { kind: 'bad'; text: string }

/**
 * `number` 那一档：把表单里的文本翻成要送的 JSON 值。
 *
 * 为什么必须翻：后端那一格走 `runParams.number()` ⇒ `json.Unmarshal` 到 `int`，
 * **送字符串就 400**（`internal/server/runs_test.go` 的
 * `TestWrongParameterTypeSaysWhichField` 拿 `{"listenSeconds":"60"}` 钉着这一条）。
 * 界面这边只能收文本（输入框里"45"就是三个字符），所以类型换算得有一个出口 ——
 * 放在这一层是因为清单、单跑那一路、以及存进预设的那一格**都从同一个 body 读**，
 * 两边各写一遍就会分叉。
 *
 * 空串 ⇒ `drop`：省略那一栏 = 用声明里的默认值（同文件那条
 * `TestListenSecondsDefaultComesFromTheDeclaration`）。
 * 解不出数 ⇒ `bad` 并把原文带着，好让 [runListIssues] 点名是哪一栏；
 * 不在输入过程中把用户打的字改掉（打到一半是"-"或"1e"都算正常）。
 */
export const numberWire = (raw: unknown): NumberWire => {
  if (typeof raw === 'number') {
    return Number.isFinite(raw) ? { kind: 'value', value: raw } : { kind: 'bad', text: String(raw) }
  }
  const text = String(raw ?? '').trim()
  if (text === '') return { kind: 'drop' }
  const n = Number(text)
  return Number.isFinite(n) ? { kind: 'value', value: n } : { kind: 'bad', text }
}

/**
 * 一步的 body → 出线形状：剔掉清单级那两格，并把 `number` 档换成数字（空的省略）。
 *
 * 能力查不到时**原样送**（只剔清单级那两格）：那一格的键名没有出处，
 * 这里替它造默认值等于瞎填，而 [runListIssues] 已经把"能力不存在"报出去了。
 */
export const stepBodyForWire = (
  body: Readonly<Record<string, unknown>>,
  cap?: CapabilityRow,
): Record<string, unknown> => {
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(body)) {
    if (LIST_LEVEL_PARAM_KEYS.includes(k)) continue
    const f = cap?.params.find((x) => x.key === k)
    if (f?.kind === 'number') {
      const wire = numberWire(v)
      if (wire.kind === 'value') out[k] = wire.value
      else if (wire.kind === 'bad') out[k] = wire.text
      // `drop` ⇒ 这个键不送，让后端用声明里的默认值。
      continue
    }
    out[k] = v
  }
  return out
}

/** 按能力声明造一份"全是默认值"的 body，并剔掉清单级的那两格（见文件头②）。 */
export const defaultBodyFor = (cap: CapabilityRow): Record<string, unknown> => {
  const body: Record<string, unknown> = {}
  for (const f of cap.params) {
    if (LIST_LEVEL_PARAM_KEYS.includes(f.key)) continue
    body[f.key] = defaultValueFor(f)
  }
  return body
}

/** 由能力造一步（带默认参数）。 */
export const makeStep = (cap: CapabilityRow, uid: string = newStepUid()): DraftStep => ({
  uid,
  code: cap.code,
  title: cap.title,
  body: defaultBodyFor(cap),
  continueOnError: null,
  rawBody: '',
})

// ─────────────────────────────────────────────────────────────────────────────
// 读：PipelineRow → RunList
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 把库里那一格的 `body` 读成表单能用的对象。
 *
 * ⚠ 只认"是对象且不是数组"：数组在这里也算不合格 —— `json.Unmarshal` 到
 * `map[string]json.RawMessage` 对数组是**报错**的，所以把它当对象渲染会让用户
 * 存一份永远 400 的预设。
 */
export const readStepBody = (raw: unknown): { body: Record<string, unknown>; text: string } => {
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    const out: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
      // 清单级那两格即使存量里有，也不带进表单：它们执行时会被覆盖，留在表单里是噪音。
      if (LIST_LEVEL_PARAM_KEYS.includes(k)) continue
      out[k] = v
    }
    return { body: out, text: '' }
  }
  if (raw === undefined || raw === null) return { body: {}, text: '' }
  let text = ''
  try {
    text = JSON.stringify(raw)
  } catch {
    text = String(raw)
  }
  return { body: {}, text }
}

/** 由一份已存的组合还原清单；能力表用来补标题与"这一步还存不存在"。 */
export const runListFromPipeline = (
  row: PipelineRow,
  caps: ReadonlyMap<string, CapabilityRow>,
): RunList => {
  const steps: DraftStep[] = []
  for (const s of row.steps ?? []) {
    const cap = caps.get(s.task)
    const { body, text } = readStepBody(s.body)
    steps.push({
      uid: newStepUid(),
      code: s.task,
      // 能力表查不到也要有名字可显示 —— 见 [DraftStep.title]。
      title: cap?.title ?? `未知能力 ${s.task}`,
      // 查不到能力时**不**替它造默认 body：那份 body 的键名没有出处，存回去就是瞎填。
      body: cap ? mergeDeclared(cap, body) : body,
      continueOnError: s.continue_on_error ?? null,
      rawBody: text,
    })
  }
  return {
    id: row.id,
    name: row.name,
    description: row.description ?? '',
    accountIds: [...(row.accountIds ?? [])],
    stopOnError: row.stopOnError,
    steps,
    builtIn: row.builtIn,
    wasParallel: row.accountMode !== 'sequential',
  }
}

/**
 * 把存回来的 body 与"声明里的键"对齐一遍：缺的补默认值，多余的**留着**。
 *
 * 多余的要留：[runListIssues] 靠它报"第 N 步带了声明里没有的键"，
 * 静默丢掉等于把一个必然 400 的预设改成一条看不出问题的数据。
 */
const mergeDeclared = (cap: CapabilityRow, body: Record<string, unknown>) => {
  const out: Record<string, unknown> = { ...body }
  for (const f of cap.params) {
    if (LIST_LEVEL_PARAM_KEYS.includes(f.key)) continue
    if (!(f.key in out)) out[f.key] = defaultValueFor(f)
  }
  return out
}

// ─────────────────────────────────────────────────────────────────────────────
// 写：RunList → 请求体
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 清单 → `PUT /api/pipelines/{id}` 请求体（键名一律 snake_case，见文件头①）。
 *
 * `caps` 是"能力 code → 声明"那张表：只有它能判断哪一栏是 `number`，
 * 而那一栏必须送 JSON 数字（理由见 [numberWire]）。传空表等于不换算 ——
 * 所以调用方**都要**传，别为了少一个参数把类型问题放回后端。
 */
export const payloadFromRunList = (
  list: RunList,
  caps: ReadonlyMap<string, CapabilityRow>,
): PipelinePayload => {
  const steps: PipelineStepPayload[] = list.steps.map((s) => ({
    task: s.code,
    body: stepBodyForWire(s.body, caps.get(s.code)),
    continue_on_error: s.continueOnError,
  }))
  return {
    name: list.name.trim(),
    description: list.description.trim() === '' ? null : list.description.trim(),
    steps,
    account_ids: [...list.accountIds],
    // 组合只支持串行（`sequentialOnly`），这里不给"留空让后端兜"的空间。
    account_mode: 'sequential',
    stop_on_error: list.stopOnError,
  }
}

/**
 * 新建一份预设的 id。
 *
 * 不用中文名：id 的合法字符集是字母数字与 `- _ .`（`validateScheduleID`），
 * 而 id 会进 URL 与日志。名字进 `name`，那里才有中文的位置。
 */
export const nextPipelineId = (taken: readonly string[]): string => {
  const used = new Set(taken)
  for (let i = 0; i < 100; i += 1) {
    const id = `preset-${Date.now().toString(36)}${i === 0 ? '' : `-${i}`}`
    if (
      id.length <= MAX_PIPELINE_ID &&
      id !== BUILT_IN_PIPELINE_ID &&
      ID_ALLOWED.test(id) &&
      !used.has(id)
    ) {
      return id
    }
  }
  // 撞满 100 次只可能是时钟不往前走；给一个带随机尾巴的，别在界面上抛 undefined。
  return `preset-${Math.random().toString(36).slice(2, 10)}`
}

// ─────────────────────────────────────────────────────────────────────────────
// 清单的增删与**重排**（用户要的那"编辑执行顺序"）
// ─────────────────────────────────────────────────────────────────────────────

/** 加一步（`at` 省略 = 末尾）。返回新数组：改引用才能让 `ref` 的订阅者看到。 */
export const insertStep = (
  steps: readonly DraftStep[],
  step: DraftStep,
  at?: number,
): DraftStep[] => {
  const next = [...steps]
  const i = at === undefined ? next.length : Math.max(0, Math.min(at, next.length))
  next.splice(i, 0, step)
  return next
}

export const removeStep = (steps: readonly DraftStep[], uid: string): DraftStep[] =>
  steps.filter((s) => s.uid !== uid)

export const findStep = (steps: readonly DraftStep[], uid: string | null): DraftStep | null =>
  steps.find((s) => s.uid === uid) ?? null

/**
 * 上移 / 下移一格；`delta` 为 `-1` / `+1`，越界**原地不动**（返回同一个数组，
 * 免得界面上一枚"到顶了的按钮"把清单引用换掉、把滚动位置一起换掉）。
 *
 * 只做相邻交换，不做任意拖拽：仓库里没有可复用的拖拽组件，自研一个要处理触屏与
 * 键盘两条路，而那不是"能编辑执行顺序"的必要条件 —— 上下移两枚按钮在窄屏上也比
 * 拖拽点得中（§7.4「所有按钮可点中」）。
 */
export const moveStep = (
  steps: readonly DraftStep[],
  index: number,
  delta: number,
): DraftStep[] => {
  const to = index + delta
  if (index < 0 || index >= steps.length || to < 0 || to >= steps.length) return [...steps]
  const next = [...steps]
  const a = next[index]
  next[index] = next[to]
  next[to] = a
  return next
}

/** 这一行还能不能上移 / 下移（按钮的 disabled 判据，别让前端自己数边界）。 */
export const canMove = (length: number, index: number, delta: number): boolean =>
  index + delta >= 0 && index + delta < length

// ─────────────────────────────────────────────────────────────────────────────
// 保存前的本地预检（后端 `pipelineRequests` 的镜像）
// ─────────────────────────────────────────────────────────────────────────────

/** 必填那一格算不算"没填"—— 与任务中心那张表单同一判据。 */
const isEmptyValue = (f: CapabilityField, v: unknown): boolean => {
  if (f.kind === 'multi_select') return !Array.isArray(v) || v.length === 0
  if (f.kind === 'bool') return false
  return v === undefined || v === null || String(v).trim() === ''
}

/**
 * 清单**保存前**的问题（人话数组，空 = 可以存）。
 *
 * 为什么要本地做一遍：后端在 `putPipeline` 里**存之前**就跑预检（一个都跑不了的组合
 * 不该进库），所以这里没挡住的毛病会变成一次 400 —— 那句话照样显示得出来，
 * 但用户已经点了"保存"，而这一屏有十几格输入，逐格回找比当场点名慢得多。
 * 判据与 `pipelineRequests` / `pipelineSteps` 对齐：空清单、能力不存在、键名不在声明里、
 * 必填未填（后端不查必填，但组合到点跑起来缺必填会整步失败，值得提前说）。
 * 同一条能力出现多次**不是**问题：带不同参数跑两次是合法用法（后端不去重，
 * `steps` 数组顺序就是执行顺序）。
 */
export const runListIssues = (
  list: RunList,
  caps: ReadonlyMap<string, CapabilityRow>,
): string[] => {
  const out: string[] = []
  if (list.name.trim() === '') out.push('给这份清单起个名字。')
  if (list.steps.length === 0) out.push('清单还是空的：先在下面的卡片里选几项。')
  list.steps.forEach((s, i) => {
    const cap = caps.get(s.code)
    if (!cap) {
      out.push(`第 ${i + 1} 步「${s.title}」在能力表里已经找不到了，先删掉它。`)
      return
    }
    if (s.rawBody !== '')
      out.push(`第 ${i + 1} 步（${cap.title}）原先带的参数不是一个对象，已按默认值处理。`)
    const declared = new Set(cap.params.map((f) => f.key))
    const extra = Object.keys(s.body).filter((k) => !declared.has(k))
    if (extra.length > 0) {
      out.push(`第 ${i + 1} 步（${cap.title}）带了声明里没有的参数：${extra.join('、')}`)
    }
    const missing = cap.params
      .filter((f) => f.required && !LIST_LEVEL_PARAM_KEYS.includes(f.key))
      .filter((f) => isEmptyValue(f, s.body[f.key]))
      .map((f) => f.label)
    if (missing.length > 0) {
      out.push(`第 ${i + 1} 步（${cap.title}）还差：${missing.join('、')}`)
    }
    // `number` 档填了非数字：后端会 400（`TestWrongParameterTypeSaysWhichField`），
    // 而那句 400 要等点击那一下才出现。存进预设更糟 —— 它到点才炸，那时人已经忘了填过什么。
    const notNumber = cap.params
      .filter((f) => f.kind === 'number')
      .filter((f) => numberWire(s.body[f.key]).kind === 'bad')
      .map((f) => f.label)
    if (notNumber.length > 0) {
      out.push(`第 ${i + 1} 步（${cap.title}）的「${notNumber.join('、')}」要填数字。`)
    }
  })
  return out
}

/** 这一份清单能不能点保存 / 运行。 */
export const runListReady = (list: RunList, caps: ReadonlyMap<string, CapabilityRow>): boolean =>
  runListIssues(list, caps).length === 0

// ─────────────────────────────────────────────────────────────────────────────
// 显示用的零碎
// ─────────────────────────────────────────────────────────────────────────────

/**
 * 组合的结局徽标。`null` 单独有一档：内置那一版没有行、也没有那四列记账，
 * 所以"没结果"对它是**常态**，不能念成"没跑过"（更不该念成出错）。
 *
 * 类名一律**整条写全**（含基类 `badge`）—— 只给修饰类会丢掉基类而渲染成裸文字，
 * typecheck 与 build 全绿（`docs/webui-code-guide.md` §7.4 补记③）。
 */
export const pipelineStatusBadge = (status: string | null | undefined): Badge => {
  if (status === 'ok') return { label: '上次全跑完', cls: 'badge badge-soft badge-success' }
  if (status === 'partial')
    return { label: '上次有步骤没跑成', cls: 'badge badge-soft badge-warning' }
  if (status === 'error') return { label: '上次有步骤失败', cls: 'badge badge-soft badge-error' }
  return { label: '尚无结果', cls: 'badge badge-soft' }
}

/**
 * 这一步失败之后怎么办（三态里"跟随组合"那一档要把组合级的值念出来）。
 *
 * ⚠ 组合级默认是 `stopOnError === false` ⇒ 不跟随的那些步骤念"继续"。
 * 这一句必须跟着 `stopOnError` 变：写死成"跟随组合（默认继续）"之后，
 * 用户把开关翻成"停下"，行上的说明还在说继续。
 */
export const continueLabel = (continueOnError: boolean | null, stopOnError: boolean): string => {
  if (continueOnError === true) return '这步失败后继续'
  if (continueOnError === false) return '这步失败后停下'
  return stopOnError ? '跟随组合：失败就停下' : '跟随组合：失败继续跑'
}

/** 三态选项（给 `<select>` 用）。值就是 `boolean | null`，序列化时直接送。 */
export const CONTINUE_OPTIONS: readonly { label: string; value: boolean | null }[] = [
  { label: '跟随组合', value: null },
  { label: '这步失败后继续', value: true },
  { label: '这步失败后停下', value: false },
]

/**
 * 一行步骤的参数摘要：只念**跟默认值不一样**的那些格。
 *
 * 为什么不是"全念"：一条能力十几格参数、清单十几步，全念下来这一屏没人读，
 * 而"我到底改过哪一格"正是清单缩略态唯一需要回答的问题。
 */
export const stepSummary = (step: DraftStep, cap: CapabilityRow | undefined): string => {
  if (step.rawBody !== '') return `原先的参数不是一个对象：${step.rawBody}`
  if (!cap) return '能力已从表里摘除，参数无法解释'
  const parts: string[] = []
  for (const f of cap.params) {
    if (LIST_LEVEL_PARAM_KEYS.includes(f.key)) continue
    const v = step.body[f.key]
    const dflt = defaultValueFor(f)
    if (isBlank(v)) continue
    if (sameValue(v, dflt)) continue
    parts.push(`${f.label}=${shortText(v)}`)
  }
  return parts.length === 0 ? '全部用默认参数' : parts.join('、')
}

const isBlank = (v: unknown): boolean =>
  v === undefined ||
  v === null ||
  (typeof v === 'string' && v.trim() === '') ||
  (Array.isArray(v) && v.length === 0)

const sameValue = (a: unknown, b: unknown): boolean => {
  if (a === b) return true
  try {
    return JSON.stringify(a) === JSON.stringify(b)
  } catch {
    return false
  }
}

/** 摘要里的值：文本原样、布尔成人话、其余 JSON 化并截断（一行放不下就不是摘要了）。 */
const shortText = (v: unknown): string => {
  let text: string
  if (typeof v === 'boolean') text = v ? '开' : '关'
  else if (Array.isArray(v)) text = v.map((x) => shortText(x)).join('/')
  else if (typeof v === 'string') text = v
  else {
    try {
      text = JSON.stringify(v) ?? String(v)
    } catch {
      text = String(v)
    }
  }
  return text.length > 24 ? `${text.slice(0, 23)}…` : text
}

/**
 * 那枚按钮点下去的一句话：`POST /api/pipelines/{id}/run` 回 **202**，
 * 响应里只有 `{pipelineId, name, steps}` —— **没有** runId 配对（整条链的 runId
 * 要等上一步跑完才分配得出来）。
 *
 * ⚠ 所以这句话不许写成"共 N 个目标正在执行"：那个 N 是**步数**，
 * 与目标账号数在多大账号下差一个数量级（判据见 `api.ts` 的 [runPipeline] 注释）。
 */
export const pipelineRunText = (name: string, steps: number): string =>
  `已受理「${name}」的 ${steps} 步 —— 逐步执行需要时间，结局稍后出现在「历史与日志」与各条能力自己的记录里。`
