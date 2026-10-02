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
 * # 为什么值是 `props.value` + `props.onInput` 而不是内部 state ✓
 *
 * 两边的**归属**不同：任务配置页存的是"这个任务的一份配置"✓，编排页存的是
 * "某一步的 `body`"✓（而且改它必须**整体换掉那个步骤对象** —— `ref` 是浅响应 ✓）。
 * ⇒ 组件只负责**画**与**上报** ✓，值存在谁那里由调用方决定 ✓。
 */

import type { TaskParamSpec } from '../api'

export const ParamField = (props: {
  spec: TaskParamSpec
  /** 当前值（JSON 原样：bool / number / string / null）。 */
  value: unknown
  onInput: (v: unknown) => void
}) => {
  const spec = props.spec
  const cur = props.value
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
            checked={cur === true}
            onChange={(e) => props.onInput((e.target as HTMLInputElement).checked)}
          />
          <span class="text-sm opacity-70">{cur === true ? '开' : '关'}</span>
        </label>
      ) : spec.kind === 'number' ? (
        <label class="input flex min-w-0 items-center gap-2">
          <input
            type="number"
            class="grow"
            min={spec.min !== undefined ? String(spec.min) : undefined}
            max={spec.max !== undefined ? String(spec.max) : undefined}
            value={cur === undefined || cur === null ? '' : String(cur)}
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
          value={typeof cur === 'string' ? cur : ''}
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
