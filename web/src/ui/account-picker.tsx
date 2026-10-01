/**
 * 账号选择器：**多选 + 全部**（"运行任务"这张表单共用）。
 *
 * # 为什么要单独一个组件
 *
 * 用户口径（2026-10-01）：「任何（包括直播小红花）应该允许配置多账号/全部，
 * 点击运行后后端保证串行执行」。任务页与直播页各有一张"选账号再运行"的表单，
 * 两处必须是**同一套控件、同一套口径**（「全部」在后端是 `accounts: []`），
 * 否则迟早一处改了、另一处没改。
 *
 * # 为什么不是 `<select multiple>`
 *
 * 移动端原生多选要靠"按住 + 点选"，几乎不可用（本仓窄屏是主要形态之一）。
 * 这里用 `<details>` 收起一个复选框列表：收起时只占一行，展开时逐项可点。
 *
 * ⚠ 「全部」是一个**独立开关**，不是"把每一项都勾上" —— 后端空数组的语义是
 * "**我名下的全部账号**"，账号以后增加了也仍然成立（勾每一项就锁死了当时的名单）。
 */
import type { AccountSummary } from '../api'

export interface AccountPickerProps {
  accounts: AccountSummary[]
  /** `true` = 全部账号（后端会收到 `accounts: []`）。 */
  all: boolean
  /** 已勾选的账号 id（`all` 为真时忽略）。 */
  picked: string[]
  onToggleAll: () => void
  onToggle: (id: string) => void
}

export function AccountPicker(props: AccountPickerProps) {
  const label = (a: AccountSummary) => a.nickname || a.label || a.id
  const nameOf = (id: string) => {
    const hit = props.accounts.find((a) => a.id === id)
    return hit ? label(hit) : id
  }
  const summary = () =>
    props.all
      ? `全部账号（${props.accounts.length} 个）`
      : props.picked.length === 0
        ? '未选择'
        : props.picked.map(nameOf).join('、')

  return (
    <details class="collapse collapse-arrow border-base-300 bg-base-100 border">
      <summary class="collapse-title min-h-0 py-2 text-sm">
        <span class="opacity-60">已选：</span>
        <span>{summary()}</span>
      </summary>
      <div class="collapse-content flex flex-col gap-1">
        <label class="label cursor-pointer justify-start gap-2 py-1">
          <input
            type="checkbox"
            class="checkbox checkbox-sm"
            checked={props.all}
            onChange={() => props.onToggleAll()}
          />
          <span class="label-text">
            全部账号（{props.accounts.length} 个）— 一个跑完再跑下一个
          </span>
        </label>
        {props.accounts.map((a) => (
          <label key={a.id} class="label cursor-pointer justify-start gap-2 py-1 pl-6">
            <input
              type="checkbox"
              class="checkbox checkbox-sm"
              checked={!props.all && props.picked.includes(a.id)}
              disabled={props.all}
              onChange={() => props.onToggle(a.id)}
            />
            <span class="label-text">
              {label(a)}
              {a.pc_platform ? `（${a.pc_platform}）` : ''}
            </span>
          </label>
        ))}
      </div>
    </details>
  )
}
