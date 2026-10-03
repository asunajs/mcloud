/**
 * 单个任务参数的控件 —— **两页共用**：任务配置页 ✓ 与任务编排页的步骤卡 ✓。
 *
 * # 为什么抽出来（而不是各写一份）
 *
 * 参数的类型、默认值、取值范围**全由后端给**（[`TaskParamSpec`]）：`kind` 只有
 * `bool` / `number` / `text` 三种 ✗，前端**不维护**任何任务知识 ✓。两份实现迟早分叉 ——
 * 那时候"任务配置页能改、编排页改不了"或"范围提示少一半"就是这么来的 ✗。
 * 编排页自己的文件头也写着同一条取向（"本页不再自己复刻一份"✓）。
 *
 * # 为什么值是 `props.get()` + `props.onInput` 而不是内部 state ✓
 *
 * 两边的**归属**不同：任务配置页存的是"这个任务的一份配置"✓，编排页存的是
 * "某一步的 `body`"✓。⇒ 组件只负责**画**与**上报** ✓，值存在谁那里由调用方决定 ✓。
 *
 * # ⚠⚠ 为什么是 `get: () => unknown`（thunk）而不是 `value: unknown`
 *
 * 2026-10-03 review §2.2：原先传的是**值**，于是**每敲一个字都丢焦点** ✗。
 * 机制（与本仓 `accounts-page.tsx:1192-1199` 那条注释同一件事）：
 * 传值时调用方必须在槽里**当场读**出它（`value={values.value[s.name]}`）⇒
 * 那个槽订阅了值信号 ⇒ 每敲一个字整块重建（含 `<input>` 节点）⇒ 焦点与光标全丢 ✗。
 * 传 thunk 时槽里读到的只是**一个函数**，真正的读发生在**这个组件自己的属性槽**里
 * ⇒ 每次按键只重跑那几个属性槽，`<input>` 节点原地不动 ⇒ 焦点与光标都在 ✓
 * （`setValue` 有 `old !== s` 的判等，值相同时连赋值都不做，见 `lite/src/dom.ts:483`）。
 */

import type { TaskParamSpec } from '../api'

export const ParamField = (props: {
  spec: TaskParamSpec
  /**
   * 取当前值（JSON 原样：bool / number / string / null）。
   *
   * ⚠ 必须是**函数**：见文件头"为什么是 thunk"。传值会在调用方的槽里建立订阅，
   * 每敲一个字重建整块、焦点全丢 ✗。
   */
  get: () => unknown
  onInput: (v: unknown) => void
}) => {
  const spec = props.spec
  return (
    <fieldset class="fieldset min-w-0">
      <legend class="fieldset-legend">
        {spec.title}
        <span class="ml-1 font-mono text-xs opacity-40">{spec.name}</span>
      </legend>

      {spec.kind === 'bool' ? (
        <label class="flex min-h-11 cursor-pointer items-center gap-3">
          <input
            type="checkbox"
            class="toggle"
            checked={props.get() === true}
            onChange={(e) => props.onInput((e.target as HTMLInputElement).checked)}
          />
          <span class="text-sm opacity-70">{props.get() === true ? '开' : '关'}</span>
        </label>
      ) : spec.kind === 'number' ? (
        <label class="input flex min-w-0 items-center gap-2">
          <input
            type="number"
            class="grow"
            min={spec.min !== undefined ? String(spec.min) : undefined}
            max={spec.max !== undefined ? String(spec.max) : undefined}
            value={
              props.get() === undefined || props.get() === null ? '' : String(props.get())
            }
            onInput={(e) => {
              const raw = (e.target as HTMLInputElement).value
              props.onInput(raw === '' ? null : Number(raw))
            }}
          />
          {spec.unit ? <span class="text-xs opacity-50">{spec.unit}</span> : null}
        </label>
      ) : (
        <textarea
          class="textarea w-full min-w-0"
          rows={3}
          value={typeof props.get() === 'string' ? (props.get() as string) : ''}
          onInput={(e) => props.onInput((e.target as HTMLTextAreaElement).value)}
        />
      )}

      {spec.help ? <p class="text-xs opacity-60">{spec.help}</p> : null}
      {spec.kind === 'number' && (spec.min !== undefined || spec.max !== undefined) ? (
        <p class="text-xs opacity-40">
          范围 {spec.min ?? '-'} ~ {spec.max ?? '-'}
        </p>
      ) : null}
    </fieldset>
  )
}
