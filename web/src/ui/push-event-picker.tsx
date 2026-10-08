/**
 * 「运行结局条件」勾选框 —— 定时 / 编排两处表单**共用同一份口径**。
 *
 * # 它是什么（用户口径 2026-10-08）
 *
 * 「行为 = 什么时候发」里，**运行类**那一半就是这一排：
 * 失败 / 部分成功 / 成功 / 取消（四个勾选框 ✓）。
 * ⚠ **全不勾 = 跑完就推** ✓ —— 不是"不推" ✗（用户原话：
 * 「全不勾 = 跑完就推」✓）。
 *
 * # 名单从哪来
 *
 * 从 `GET /api/push/events` 里筛 `kind === 'run'` 的那几条 ✓ ——
 * 前端**不抄**第二份名单（后端加/改事件时这里自动跟上 ✓）。
 * 只画运行类：面板行为那四条（登录/性能）归「设置 → 事件推送」✓，
 * 摆到这里只会让用户以为"登录失败也能绑定时" ✗。
 *
 * ⚠ 值走 `value={props.value}`（组件属性是**编译期 getter** ✓），理由同
 * `push-rule-select.tsx` 文件头 —— 别在调用方函数体里先读成数组再传。
 */
import type { PushEventOption } from '../api'

export interface PushEventPickerProps {
  /** 稳定标识（门禁按它取这一块）。 */
  id: string
  /** **运行类**事件清单（调用方已按 `kind === 'run'` 筛过 ✓）。 */
  events: PushEventOption[]
  /** 勾上的事件键。空数组 = 跑完就推 ✓。 */
  value: string[]
  onChange: (value: string[]) => void
}

/** 全不勾时那句话 —— 两处必须一字不差，所以只写在这一处。 */
export const PUSH_EVENTS_ALL_LABEL = '跑完就推（不看结局）'

export function PushEventPicker(props: PushEventPickerProps) {
  /** ⚠ 换**新数组**：浅响应的 `ref` 只认引用换没换，原地 push/splice 没人会醒 ✗。 */
  const toggle = (key: string) => {
    props.onChange(
      props.value.includes(key) ? props.value.filter((k) => k !== key) : [...props.value, key],
    )
  }

  return (
    <div
      id={props.id}
      class="grid min-w-0 gap-1 md:grid-cols-2"
      /*
        `data-value`：当前勾选的**可读出口**（与 `ui/select.tsx` 那条同一个理由）——
        门禁要断言"勾的到底是哪几个"，而这些是散着的 checkbox、没有 `select.value` 可读。
      */
      data-value={props.value.join(',')}
    >
      {props.events.map((o) => (
        <label key={o.key} class="flex min-h-11 cursor-pointer items-center gap-3 md:min-h-0">
          <input
            type="checkbox"
            class="checkbox checkbox-sm"
            checked={props.value.includes(o.key)}
            onChange={() => toggle(o.key)}
          />
          <span class="text-sm">{o.label}</span>
        </label>
      ))}
    </div>
  )
}
