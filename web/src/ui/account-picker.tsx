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
 * # 为什么不是 `<select multiple>`，也不是原来的 `<details>`
 *
 * 2026-10-02 起换成 `ui/select.tsx`（TDesign 口径的自建下拉）：
 *  - 原生多选要靠"按住 + 点选"，移动端几乎不可用（本仓窄屏是主要形态之一）；
 *  - 原来的 `<details>` 面板**在文档流里**，展开会把卡片撑高、把下面的内容顶下去，
 *    也没有"点外部关闭"——而用户要的是 TDesign 那种浮层。
 *
 * ⚠ 「全部」是一个**独立开关**，不是"把每一项都勾上" —— 后端空数组的语义是
 * "**我名下的全部账号**"，账号以后增加了也仍然成立（勾每一项就锁死了当时的名单）。
 * 所以它走 `allRow`，而不是往 `options` 里塞一项。
 */
import type { AccountSummary } from '../api'
import { Select } from './select'

export interface AccountPickerProps {
  /**
   * 稳定标识。
   *
   * ⚠ **一页里放两个选择器时必须各给一个不同的 id**：`Select` 靠 id 认人
   * （面板挂在 `body` 上、展开状态是模块级的，见 `ui/select.tsx` 文件头 ✓）——
   * 两个同 id 的选择器会互相认错，展开/收起与回调都会串台 ✗
   * （直播页上「跑这一趟用哪些账号」与「自动定时作用在哪些账号」就是两个 ✓）。
   *
   * 缺省保持老值 `account-picker`（别的页面只有一个，不必改 ✓）。
   */
  id?: string
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

  return (
    <Select
      id={props.id ?? 'account-picker'}
      multiple
      filterable
      ariaLabel="账号"
      placeholder="请选择账号"
      emptyText="还没有已登录的账号"
      value={props.picked}
      /*
        ⚠ 上游接口是 `onToggle(id)`（逐项），而 `Select` 给的是**下一份名单**。
        面板一次只改一项 ⇒ `added` / `removed` 里必有一个，折回一次 toggle 即可。
        这样两个调用点（任务页 / 直播页）的 props 不用动，口径也不会分叉。
      */
      onChange={(next) => {
        const added = next.find((v) => !props.picked.includes(v))
        if (added !== undefined) {
          props.onToggle(added)
          return
        }
        const removed = props.picked.find((v) => !next.includes(v))
        if (removed !== undefined) props.onToggle(removed)
      }}
      allRow={{
        label: `全部账号（${props.accounts.length} 个）`,
        active: props.all,
        onToggle: props.onToggleAll,
      }}
      options={props.accounts.map((a) => ({
        value: a.id,
        label: label(a),
        hint: a.pc_platform ? a.pc_platform : undefined,
        // 勾了「全部账号」就锁住逐个勾选（后端口径：全部 = `accounts: []`）
        disabled: props.all,
      }))}
    />
  )
}
