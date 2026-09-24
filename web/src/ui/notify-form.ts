/**
 * 推送表单的**纯逻辑**：脱敏视图 → 控件草稿 → `PUT /api/notify/config` 的请求体。
 *
 * 这里不含 Vue 依赖（`ui/` 的约定，代码指南 §9 约定 2）：判据都写成可单读的函数，
 * 页面只负责摆控件。
 *
 * # 这一份逻辑为什么值得单独存在
 *
 * 后端的写侧是**逐字段合并**（`internal/server/notify_config.go` 抬头）：请求体里没出现的键
 * 保持库里那一份。于是"这一格要不要进请求体"就是**数据保全**级别的判断 ——
 * 判错的方向是"用户只想换 bark 的 key，却把别家没在表单里的字段洗掉了"，
 * 而且那种丢失**不报任何错**。所以它不能散在 JSX 的事件回调里。
 *
 * 三条判据（都来自后端，不在这里另立）：
 *  1. **留空 = 不改**：`secret` / 字符串 / 数字 / JSON 四档都是"空白 ⇒ 这一格不出现在体里"。
 *     清空**单个字段**做不到（后端分不清"忘了填"与"要清空"，登记在 `docs/pending-decisions.md`），
 *     要清空整家走 `null`；
 *  2. **布尔格是三态**：`*bool` 的 `false` 是有效值，所以"没动过"必须能表达 —— 用 `null`；
 *  3. **不认识的键 = 400**：键名逐字用视图给的 `fields[].name`（后端那张表的 json tag），
 *     这里不写死任何字段名。
 */

import type {
  NotifyChannelView,
  NotifyConfigPatch,
  NotifyConfigView,
  NotifyFieldView,
} from '../api'
import { fieldLabel } from './notify-channels'

/**
 * 一格的草稿。
 *
 * ⚠ 它是**可变对象、不放 ref**：输入框是非受控的（初始值来自视图，之后以 DOM 为准），
 * 每次改动只写进这个对象。放进 ref 会让"每敲一个字"触发一次重渲染（响应式的定义就含这一条），
 * 而 Vapor 下重建这段 DOM 会不会表现为"输入框当场失焦"⇒ **未实测**；要测它就把 [DraftStore]
 * 换成 `ref` 后真机连打几个字、量 `document.activeElement` 还在不在这一格。
 * 当前这一支（非受控）真机核验过：连续填写后读回的值对得上、窗口不重建。
 */
export interface FieldDraft {
  text: string
  /** 开关格专用：`null` = 没动过（不提交）。文本类档位忽略这一格。 */
  bool: boolean | null
}

export type DraftStore = Record<string, FieldDraft>

/** 按后端的清单开一份空白草稿（每格"没填"）。 */
export const newStore = (fields: NotifyFieldView[]): DraftStore => {
  const store: DraftStore = {}
  for (const field of fields) store[field.name] = { text: '', bool: null }
  return store
}

/** `customPost` 的一条：草稿数组的下标就是提交时的条目序（整块替换，没有"同一条"的身份）。 */
export const newStoresForRows = (
  fields: NotifyFieldView[],
  rowCount: number,
): DraftStore[] => {
  const rows: DraftStore[] = []
  for (let index = 0; index < Math.max(rowCount, 1); index += 1) rows.push(newStore(fields))
  return rows
}

/** 一格能建出三种结论：不提交 / 提交这个值 / 这一格现在填的东西后端收不下。 */
export type FieldBuild =
  | { status: 'skip' }
  | { status: 'ok'; value: unknown }
  | { status: 'bad'; error: string }

/**
 * 一格 → 请求体里的那一项（或"跳过"）。
 *
 * 分派只看视图给的 `kind` / `type`，不看字段名：后端加一格时这里不必改。
 */
export const buildField = (field: NotifyFieldView, draft: FieldDraft): FieldBuild => {
  const label = fieldLabel(field.name)
  const text = draft.text.trim()
  switch (field.kind) {
    case 'secret':
      // 留空 = 不改：界面上这一格本来就**没有**原值可回显（只出"几个字符"），
      // 所以把空串当成"要清空"会等于送一个后端分不清的语义。
      if (text === '') return { status: 'skip' }
      return { status: 'ok', value: draft.text }
    case 'keys':
      if (text === '') return { status: 'skip' }
      // 空对象 `{}` 是**有意义**的提交：后端的判据是"nil = 保持、非 nil 的空 map = 清空"。
      return parseObject(text, label)
    default:
      return buildPlain(field, draft, label)
  }
}

/** `plain` 那一档按 `type` 再分：文本 / 整数 / 布尔。 */
const buildPlain = (
  field: NotifyFieldView,
  draft: FieldDraft,
  label: string,
): FieldBuild => {
  if (field.type === 'boolean') {
    if (draft.bool === null) return { status: 'skip' }
    return { status: 'ok', value: draft.bool }
  }
  const text = draft.text.trim()
  if (text === '') return { status: 'skip' }
  if (field.type === 'number') {
    // 只收整数：`port` / `agentid` / `timeout` 三样在小数上都会被后端解不成目标类型，
    // 而那句 400（"字段形状不对"）不会指出是**哪一格**——在这里拦住更便宜。
    if (!/^-?\d+$/.test(text)) {
      return { status: 'bad', error: `${label} 要填整数（现在这串不是）` }
    }
    const value = Number(text)
    if (!Number.isSafeInteger(value)) {
      return { status: 'bad', error: `${label} 这个数字超出可表达的范围` }
    }
    return { status: 'ok', value }
  }
  return { status: 'ok', value: draft.text }
}

/** 自由键那一格：必须是 JSON 对象（数组、字符串、数字都不收 —— 与后端 `decodeNotifyObject` 同口径）。 */
const parseObject = (text: string, label: string): FieldBuild => {
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    // 不回填 parser 那句英文（它带用户写到一半的片段）：填到一半是常态。
    return { status: 'bad', error: `${label} 还不是合法 JSON：先写成完整的一串对象，例如 {"topic":"xx"}` }
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { status: 'bad', error: `${label} 必须是一个 JSON 对象（数组与标量都不收）` }
  }
  return { status: 'ok', value: parsed }
}

/** 一次建渠道请求体的结论：要么带走一份补丁，要么带着**这一家**的人话错误。 */
export type ChannelBuild =
  | { ok: true; patch: Record<string, unknown> }
  | { ok: false; error: string }

/**
 * 对象型渠道（九家）的请求体：逐格过 [buildField]，只留填了的那些。
 *
 * 一格都没填时**报**错而不是发出去：`{"bark":{}}` 到了后端会**建出**一家空配置
 * （`mergeNotifyChannel` 从"库里那份 + 零值"出发合并，再写回），
 * 于是整份配置的校验会报"缺少必填项"，而用户看到的是"我明明什么都没改"。
 */
export const buildChannelPatch = (
  view: NotifyChannelView,
  store: DraftStore,
): ChannelBuild => {
  const patch: Record<string, unknown> = {}
  for (const field of view.fields) {
    const draft = store[field.name]
    if (!draft) continue
    const built = buildField(field, draft)
    if (built.status === 'skip') continue
    if (built.status === 'bad') return { ok: false, error: built.error }
    patch[field.name] = built.value
  }
  if (Object.keys(patch).length === 0) {
    return {
      ok: false,
      error: '这一家没有填任何一格：留空表示"不改"。要清空这一家请用「删除这一家」。',
    }
  }
  return { ok: true, patch }
}

/** `customPost` 的结论：要么带走**整块替换**的那几条，要么带着人话错误。 */
export type CustomPostBuild =
  | { ok: true; rows: Record<string, unknown>[] }
  | { ok: false; error: string }

/**
 * `customPost`（整块替换）的那几条。
 *
 * 与对象型渠道**分两个函数**写，不是复制粘贴的失误：这一家没有"逐格合并"可言
 * （数组里的"同一条"没有可靠身份，按下标对齐会让"删掉第一条"把后面每条错位读到上一条的凭据），
 * 所以每条都得重填 `url` —— 那是这一家独有的代价，判据也只在这里成立一次。
 *
 * ⚠ 带错误的条目**照常进数组**（只打个标记），别学 `forEach` 里直接 `return`：
 *   那个 `return` 退出的是**回调本身**，于是 `rows.push(row)` 被跳掉 ——
 *   下面那条 `__error__` 扫描就成了永远命中不到的死代码，而后果不是"少一句提示"：
 *   少掉的正是这一条，后面两条判据读的都是**残缺的数组**，
 *   于是"headers 填了半截 JSON"会被念成「这一家一条都没有」（库里有条目时）
 *   或「第 N 条没有填地址」而且**条号错位**（第 2 条坏 ⇒ 报第 1 条）。
 *   真机实测来由见 `docs/webui-code-guide.md` §9.2 补记（推送设置页那一轮）第⑥条。
 */
export const buildCustomPostRows = (
  view: NotifyChannelView,
  stores: DraftStore[],
): CustomPostBuild => {
  const rows: Record<string, unknown>[] = []
  stores.forEach((store, index) => {
    const row: Record<string, unknown> = {}
    for (const field of view.fields) {
      const draft = store[field.name]
      if (!draft) continue
      const built = buildField(field, draft)
      if (built.status === 'skip') continue
      if (built.status === 'bad') {
        row.__error__ = `第 ${String(index + 1)} 条：${built.error}`
        break
      }
      row[field.name] = built.value
    }
    rows.push(row)
  })
  for (const row of rows) {
    if (row.__error__ !== undefined) return { ok: false, error: String(row.__error__) }
  }
  // 地址不回显（整条 URL 就是凭据），所以每一条都得重填 —— 少了它这一条到后端只会得到
  // 一句指不出"第几条"的"缺少必填项"。
  const missingAt = rows.findIndex((row) => typeof row['url'] !== 'string' || row['url'] === '')
  if (missingAt >= 0) {
    return {
      ok: false,
      error: `第 ${String(missingAt + 1)} 条没有填地址：这一家是整块替换，地址不回显，每条都要重填 url。`,
    }
  }
  if (rows.length === 0 && (view.count ?? 0) === 0) {
    return {
      ok: false,
      error: '这一家一条都没有：留空表示"不改"。要清空这一家请用「删除这一家」。',
    }
  }
  return { ok: true, rows: rows.map((row) => stripMarker(row)) }
}

/** 丢掉内部标记键（只在判据那一趟里用，不该进请求体）。 */
const stripMarker = (row: Record<string, unknown>): Record<string, unknown> => {
  const out: Record<string, unknown> = {}
  for (const key of Object.keys(row)) if (key !== '__error__') out[key] = row[key]
  return out
}

/** 顶层四格（总开关 / 只推错误 / 门槛 / 标题）的草稿。 */
export interface TopDraft {
  enabled: boolean
  onlyError: boolean
  minLevel: string
  title: string
}

export const topDraftOf = (view: NotifyConfigView): TopDraft => ({
  enabled: view.enabled,
  onlyError: view.onlyError,
  minLevel: view.minLevel,
  title: view.title ?? '',
})

/** 有没有改动 —— 保存按钮的禁用判据（没改动时点它只会白写一次 `updatedAt`）。 */
export const topDirty = (draft: TopDraft, view: NotifyConfigView): boolean =>
  draft.enabled !== view.enabled ||
  draft.onlyError !== view.onlyError ||
  draft.minLevel !== view.minLevel ||
  draft.title.trim() !== (view.title ?? '').trim()

/**
 * 顶层 → 补丁。**只带改动了的那些格**。
 *
 * 标题那一格是这一族里唯一"空串有信息"的：留空 = 用默认标题（`TitleOrDefault` 本来就把
 * 空串折成默认值），所以清空它不损失什么，后端也据此把 `""` 判成清空而不是保持。
 */
export const buildTopPatch = (draft: TopDraft, view: NotifyConfigView): NotifyConfigPatch => {
  const patch: NotifyConfigPatch = {}
  if (draft.enabled !== view.enabled) patch.enabled = draft.enabled
  if (draft.onlyError !== view.onlyError) patch.onlyError = draft.onlyError
  if (draft.minLevel !== view.minLevel) patch.minLevel = draft.minLevel
  if (draft.title.trim() !== (view.title ?? '').trim()) patch.title = draft.title.trim()
  return patch
}

/** 门槛下拉的选项。 */
export const levelOptions = (
  view: NotifyConfigView,
  labelOf: (level: string) => string,
): { value: string; label: string }[] => {
  const options = view.levels.map((level) => ({ value: level, label: labelOf(level) }))
  if (!view.levels.includes(view.minLevel)) {
    // 库里存着后端不认识的值（导入的旧串）时**如实列出**：替它改成 info 会让"上次配置"
    // 在一次打开页面的过程中被改掉，而用户只点了"打开这一页"。
    options.unshift({
      value: view.minLevel,
      label:
        view.minLevel === ''
          ? '未设置（后端按 info 走）'
          : `当前值 ${view.minLevel}（后端不认这一档）`,
    })
  }
  return options
}
