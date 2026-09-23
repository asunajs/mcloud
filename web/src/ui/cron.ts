/**
 * 时刻 ↔ cron 的映射 —— 定时任务表单那一块的全部算术，**纯函数**（代码指南 §9.2 把
 * "时刻 → cron 映射"点名列在"必须能在组件外测"的那一档）。
 *
 * # 段数：这一族一律出 **6 段**
 *
 * `秒 分 时 日 月 周`。它是旧项目（`HMSToCron`）与本仓 `core.ParseCron` 都认的形态；
 * 5 段的 POSIX 形态解析器也收，但界面不回一份"看着像却少一段"的东西给用户核对。
 *
 * # 为什么这里**不**做校验
 *
 * 合法性只有一个权威：服务端 `internal/core/cron.go`。它有四处语义前端复现一遍就是
 * 分叉的候选 ——
 *  1. 5 段与 6 段的分流（`30 7 * * *` 与 `30 7 * * * *` 不是同一件事）；
 *  2. 日与周**同时**被限定时取 OR（Vixie cron），反直觉；
 *  3. 周日的两种写法 `0` 与 `7`；
 *  4. "语法对但永远不触发"（`0 0 0 30 2 *`）—— 这条前端根本判不出来，
 *     而它的后果最阴：存下去不会报错，只会每天安静地什么都不做。
 *
 * 所以错都由 `PUT` 那一下带回（后端那句中文是可读的，例如"cron 的分字段取值 75
 * 超出范围 0~59"），界面原样显示在弹窗里。**不**在这里先判一遍再编一句自己的措辞。
 */

/** 界面上的四个频率档。`custom` 是唯一能表达"预设给不出的形状"的那一档。 */
export type CronPreset = 'daily' | 'weekly' | 'hourly' | 'custom'

/** 频率档的标签。**字面量查表**，不拼接（Tailwind 那条静默失效同源，见 §9.1①）。 */
export const CRON_PRESET_LABEL: Record<CronPreset, string> = {
  daily: '每天',
  weekly: '每周',
  hourly: '每小时',
  custom: '自定义',
}

/** 下拉顺序：常用的在前。 */
export const CRON_PRESETS: readonly CronPreset[] = ['daily', 'weekly', 'hourly', 'custom']

/** 周几。键是 cron 里那个数字（`7` 在读侧归一为 `0`）。 */
export const WEEKDAY_LABEL: Record<string, string> = {
  '0': '周日',
  '1': '周一',
  '2': '周二',
  '3': '周三',
  '4': '周四',
  '5': '周五',
  '6': '周六',
}

/** 下拉顺序：按国人习惯从周一排起，周日收尾。 */
export const WEEKDAY_VALUES: readonly string[] = ['1', '2', '3', '4', '5', '6', '0']

/**
 * 表单里的一份"频率草稿"。
 *
 * 数字一律存**字符串**：它们直接来自 `<input>`，存成 number 要在每个 `onInput` 里
 * 处理 `NaN`（清空输入框就是 NaN），而清空恰恰是用户改时刻时的中间态。
 */
export interface CronDraft {
  preset: CronPreset
  /** `HH:MM`，来自 `<input type="time">`；`hourly` 与 `custom` 不用它。 */
  time: string
  /** 每小时的第几分钟（`hourly` 那一档用）。 */
  minute: string
  /** `0`~`6`；`weekly` 那一档用。 */
  weekday: string
  /** `custom` 那一档：用户手写的那串。 */
  cron: string
}

export const emptyCronDraft = (): CronDraft => ({
  preset: 'daily',
  time: '07:30',
  minute: '0',
  weekday: '1',
  cron: '',
})

/** `HH:MM` → `{小时, 分钟}`；形状不对返回 null，**不猜**。 */
const splitTime = (text: string): { hour: number; minute: number } | null => {
  const m = /^(\d{1,2}):(\d{2})$/.exec(text.trim())
  if (!m) return null
  const hour = Number(m[1])
  const minute = Number(m[2])
  if (hour > 23 || minute > 59) return null
  return { hour, minute }
}

const toMinute = (text: string): number | null => {
  const n = Number(text.trim())
  return Number.isInteger(n) && n >= 0 && n <= 59 ? n : null
}

/** 草稿 → cron。凑不出一条完整表达式时返回 null，由调用方挡住提交。 */
export const buildCron = (draft: CronDraft): string | null => {
  switch (draft.preset) {
    case 'daily': {
      const t = splitTime(draft.time)
      return t ? `0 ${t.minute} ${t.hour} * * *` : null
    }
    case 'weekly': {
      const t = splitTime(draft.time)
      const d = Number(draft.weekday)
      return t && Number.isInteger(d) && d >= 0 && d <= 6 ? `0 ${t.minute} ${t.hour} * * ${d}` : null
    }
    case 'hourly': {
      const m = toMinute(draft.minute)
      return m === null ? null : `0 ${m} * * * *`
    }
    case 'custom': {
      const text = draft.cron.trim()
      return text === '' ? null : text
    }
  }
}

/** 拆段：只认 5 段与 6 段，其余一律 null（5 段按 POSIX 补一个秒位 `0`）。 */
const fields = (cron: string): string[] | null => {
  const parts = cron.trim().split(/\s+/).filter((p) => p !== '')
  if (parts.length === 6) return parts
  if (parts.length === 5) return ['0', ...parts]
  return null
}

const isNum = (text: string) => /^\d+$/.test(text)

/**
 * cron → 草稿。认不出的表达式一律落到 `custom` 那一档、原样把串交回输入框 ——
 * ⚠ 这**不是**"解析失败"，自定义那一档本来就是留给它的。
 *
 * 认不出预设时不去"修"它（比如把 `*` 补成具体值）：那会让用户存的东西
 * 在一次编辑后被静默改写。
 */
export const readCron = (cron: string): CronDraft => {
  const f = fields(cron)
  if (!f) return { ...emptyCronDraft(), preset: 'custom', cron: cron.trim() }
  const [sec, min, hour, dom, month, dow] = f
  const star = (t: string) => t === '*'
  const pad = (n: string) => n.padStart(2, '0')
  if (sec === '0' && isNum(min) && isNum(hour) && star(dom) && star(month) && star(dow)) {
    return {
      ...emptyCronDraft(),
      preset: 'daily',
      time: `${pad(hour)}:${pad(min)}`,
      cron: cron.trim(),
    }
  }
  if (sec === '0' && isNum(min) && isNum(hour) && star(dom) && star(month) && /^[0-7]$/.test(dow)) {
    return {
      ...emptyCronDraft(),
      preset: 'weekly',
      time: `${pad(hour)}:${pad(min)}`,
      // 周日两种写法归一：`7` 与 `0` 在解析器等价（见 cron.go 那条），界面只认 `0`。
      weekday: dow === '7' ? '0' : dow,
      cron: cron.trim(),
    }
  }
  if (sec === '0' && isNum(min) && star(hour) && star(dom) && star(month) && star(dow)) {
    return { ...emptyCronDraft(), preset: 'hourly', minute: min, cron: cron.trim() }
  }
  return { ...emptyCronDraft(), preset: 'custom', cron: cron.trim() }
}

/**
 * 一行中文说明，给列表那一列用。
 *
 * 只翻译**能整条对上预设**的表达式；对不上就写"自定义"并把原串放出来。
 * 不去做通用的 cron → 自然语言解释器：那等于在前端再实现一遍 cron 语义，
 * 而 §"为什么这里不做校验" 那四条差异会全部落进来。
 */
export const describeCron = (cron: string): string => {
  const draft = readCron(cron)
  switch (draft.preset) {
    case 'daily':
      return `每天 ${draft.time}`
    case 'weekly':
      return `每${WEEKDAY_LABEL[draft.weekday] ?? '周?'} ${draft.time}`
    case 'hourly':
      return `每小时第 ${draft.minute} 分`
    default:
      return `自定义：${draft.cron || '—'}`
  }
}

/** 补跑策略的中文。`null` 那一档**不说"跟随默认"**——默认值住在能力表里，这里替它说话会变成谎话。 */
export const MISS_POLICY_LABEL: Record<string, string> = {
  catch_up: '错过补跑',
  skip: '错过跳过',
}

/**
 * 新建一条任务的 id。
 *
 * 后端**不发明 id**（`PUT` 靠路径里的 id 区分"改"与"建"），所以前端得给一个。
 * 字符集与长度上限照 `validateScheduleID`：字母、数字与 `- _ .`，≤64 字符。
 * 前缀写死 `task-`：它会在 URL、日志与库里的同一列出现，带个能读懂的前缀比随机串好查。
 */
export const newScheduleId = (): string =>
  `task-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
