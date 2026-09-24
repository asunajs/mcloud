/**
 * 推送渠道的**编辑表单**（设置页那一枚弹窗的正文）。
 *
 * # 为什么是一个"返回节点的函数"而不是 Vapor 组件
 *
 * 代码指南 §9.2④：组件的 setup 只跑一次，在函数体里读的 props 会停在第一次那一份。
 * 这一窗要跟着"当前编辑哪一家"换整张字段表，所以它写成**在子节点位置调用**的函数
 * （`{NotifyChannelForm(props)}`），与 `schedules-page.tsx` 的 `editDialog()` 同一形态：
 * 外层那个 effect 收走 `shape()` 里的读取 ⇒ 换一家就重建这一段。
 *
 * # 输入框是非受控的
 *
 * 每格的值存在 [DraftStore]（普通对象，不放 ref）里，输入框只在**创建那一下**读一次初始值，
 * 之后以 DOM 为准。放进 ref 会变成"每敲一个字重建整窗"；重建在 Vapor 下是否表现为
 * 输入框当场失焦 ⇒ **未实测**（怎么测见 `ui/notify-form.ts` 顶部那条）。
 * 非受控这一支真机核验过：连续填写后读回的值对得上，窗口不重建。
 *
 * 因此 `formError` / `busy` / `reveal` 这三样**必须**走各自的 getter（独立子节点位置）：
 * 把它们并进 `shape()`，一次报错就会把用户填了一半的格子全部重建。
 */

import type { NotifyChannelView, NotifyFieldView } from '../api'
import type { DraftStore } from '../ui/notify-form'
import { channelLabel, fieldHint, fieldLabel } from '../ui/notify-channels'

/** 这一窗当前要画的那一家（结构信息；随"编辑哪家"变，不随打字变）。 */
export interface ChannelShape {
  name: string
  view: NotifyChannelView
  /** 对象型九家共用的一份草稿。 */
  store: DraftStore
  /** 只有 `customPost` 用：逐条草稿（数组序 = 提交序）。 */
  rows: DraftStore[]
  /** 现有几条（只用来写"删掉的是第几条"这类话）。 */
  existing: number
}

export interface ChannelFormProps {
  /** 返回 null = 这一窗当前没在编辑任何一家（弹窗没开）。 */
  shape: () => ChannelShape | null
  formError: () => string
  busy: () => boolean
  reveal: () => boolean
  onSave: () => void
  onDelete: () => void
  onCancel: () => void
  onToggleReveal: (on: boolean) => void
  onAddRow: () => void
  onRemoveRow: (index: number) => void
}

/** 一格的控件那一条（label + 输入 + 说明）。 */
const fieldRow = (
  field: NotifyFieldView,
  store: DraftStore,
  missingName: string,
  reveal: boolean,
) => {
  const draft = store[field.name]
  if (!draft) return null
  const label = fieldLabel(field.name)
  const required = missingName !== '' && missingName === field.name
  const hint = fieldHint(field.name)
  return (
    <label class="flex flex-col gap-1 text-sm">
      <span class="opacity-70">
        {label}
        {required ? <span class="text-error">（必填，还没填）</span> : null}
      </span>
      {control(field, draft, reveal)}
      {hint ? (
        <span class="break-words text-xs leading-5 opacity-60">{hint}</span>
      ) : null}
    </label>
  )
}

/** 三种档位 × 三种值类型 → 具体控件。判据全在视图给的 `kind` / `type` 上，不认字段名。 */
const control = (field: NotifyFieldView, draft: { text: string; bool: boolean | null }, reveal: boolean) => {
  if (field.kind === 'secret') {
    const length = field.length ?? 0
    return (
      <input
        class="input w-full"
        type={reveal ? 'text' : 'password'}
        autocomplete="off"
        spellcheck={false}
        aria-label={fieldLabel(field.name)}
        placeholder={
          field.present ? `已配置（${length} 位）：留空表示不改` : '还没填（这一格的值不回显）'
        }
        value={draft.text}
        onInput={(e) => (draft.text = (e.target as HTMLInputElement).value)}
      />
    )
  }
  if (field.kind === 'keys') {
    const keys = field.keys ?? []
    return (
      <>
        <textarea
          class="textarea w-full font-mono"
          rows={2}
          spellcheck={false}
          aria-label={fieldLabel(field.name)}
          placeholder='形如 {"topic":"xx"}；留空 = 不改'
          value={draft.text}
          onInput={(e) => (draft.text = (e.target as HTMLTextAreaElement).value)}
        />
        <span class="break-words text-xs leading-5 opacity-60">
          {keys.length === 0
            ? '现在一个键都没有。'
            : `现在有 ${keys.length} 个键：${keys.join('、')}（值不回显）。`}
          提交这一格 = 整块替换。要清空就只写 {'{}'}。
        </span>
      </>
    )
  }
  if (field.type === 'boolean') {
    return (
      <span class="flex items-center gap-2">
        <input
          type="checkbox"
          class="toggle toggle-primary min-h-11"
          aria-label={fieldLabel(field.name)}
          checked={draft.bool ?? field.value === true}
          onChange={(e) => (draft.bool = (e.target as HTMLInputElement).checked)}
        />
        <span class="text-xs opacity-60">
          {draft.bool === null ? '没动过：保持库里那一份' : draft.bool ? '提交为「是」' : '提交为「否」'}
        </span>
      </span>
    )
  }
  return (
    <input
      class="input w-full"
      type={field.type === 'number' ? 'number' : 'text'}
      inputmode={field.type === 'number' ? 'numeric' : undefined}
      spellcheck={field.type === 'number' ? false : undefined}
      aria-label={fieldLabel(field.name)}
      value={draft.text}
      onInput={(e) => (draft.text = (e.target as HTMLInputElement).value)}
    />
  )
}

/** `customPost`：整块替换的那几条，每条一份同样的字段表。 */
const customPostRows = (props: ChannelFormProps, shape: ChannelShape) => (
  <div class="flex flex-col gap-3">
    <p class="break-words text-sm leading-6">
      这一家是整块替换：保存时以界面上这几条为准。地址与请求头不回显，所以每条的 url
      都要重填一遍（这是这一家独有的代价，见 `docs/pending-decisions.md` Q30、Q31）。
    </p>
    {shape.rows.map((store, index) => (
      <fieldset class="fieldset gap-2 rounded-box border border-base-300 p-3">
        <legend class="fieldset-legend">
          第 {String(index + 1)} 条
          {index < shape.existing ? `（库里原有第 ${String(index + 1)} 条）` : '（新加）'}
        </legend>
        {shape.view.fields.map((field) => fieldRow(field, store, '', props.reveal()))}
        {shape.rows.length > 1 ? (
          <button
            type="button"
            class="btn btn-soft btn-sm min-h-11 md:min-h-0 self-start text-error"
            onClick={() => props.onRemoveRow(index)}
          >
            删掉这一条
          </button>
        ) : null}
      </fieldset>
    ))}
    <button
      type="button"
      class="btn btn-soft btn-sm min-h-11 md:min-h-0 self-start"
      onClick={props.onAddRow}
    >
      再加一条
    </button>
  </div>
)

/** 弹窗正文。调用点必须在**子节点位置**（见文件顶部那条）。 */
export const NotifyChannelForm = (props: ChannelFormProps) => {
  const shape = props.shape()
  if (!shape) return null
  const isCustomPost = shape.name === 'customPost'
  return (
    <>
      <p class="break-words text-sm leading-6 opacity-70">
        留空的格子不会被改动（凭据本来就不回显，所以这里没有"清空单格"这回事）；
        要整家去掉，用下面那枚「删除这一家」。
      </p>

      {isCustomPost ? (
        customPostRows(props, shape)
      ) : (
        <fieldset class="fieldset gap-3">
          <legend class="fieldset-legend">这一家的字段</legend>
          {shape.view.fields.map((field) =>
            fieldRow(field, shape.store, shape.view.missing, props.reveal()),
          )}
        </fieldset>
      )}

      {/* 「显示已填的格子」：值本来就不出网，这一位只影响**这一趟输入**看不看得见。 */}
      <label class="flex min-h-11 cursor-pointer items-center gap-2 text-sm">
        <input
          type="checkbox"
          class="checkbox checkbox-sm checkbox-primary"
          checked={props.reveal()}
          aria-label="显示凭据那一格的输入内容"
          onChange={(e) => props.onToggleReveal((e.target as HTMLInputElement).checked)}
        />
        <span>显示我这次填的凭据（默认用圆点挡住）</span>
      </label>

      {/* 错误与进行中各走**独立**的子节点位置：并进上面那段会让一次报错重建全部输入框。 */}
      {props.formError() ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{props.formError()}</span>
        </div>
      ) : null}

      <div class="modal-action flex-wrap gap-2">
        <button
          type="button"
          class="btn btn-soft min-h-11 md:min-h-0"
          onClick={props.onCancel}
        >
          取消
        </button>
        {shape.view.configured ? (
          <button
            type="button"
            class="btn btn-soft text-error min-h-11 md:min-h-0"
            disabled={props.busy()}
            onClick={props.onDelete}
          >
            删除这一家
          </button>
        ) : null}
        <button
          type="button"
          class="btn btn-primary min-h-11 md:min-h-0"
          disabled={props.busy()}
          onClick={props.onSave}
        >
          {props.busy() ? '保存中…' : '保存这一家'}
        </button>
      </div>
    </>
  )
}

/**
 * 弹窗标题：「填写 pushplus」/「修改 企业微信」。
 *
 * 措辞分开是有用的——「填写」说明这家还没配，「修改」说明下面那些留空的格子**不会**
 * 动到库里那一份。判据取视图的 `configured`，不取"草稿里填了没"（那会随打字变）。
 */
export const channelDialogTitle = (view: NotifyChannelView | null, name: string): string => {
  if (!view) return '配置推送渠道'
  return `${view.configured ? '修改' : '填写'} ${channelLabel(name)}`
}
