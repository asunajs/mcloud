/**
 * 「推送规则」选择器（**多选**）—— 定时 / 编排两处对话框**共用同一份口径**。
 *
 * # 2026-10-08：从单选改成多选（用户口径「**不是规则去命中行为，而是行为选择规则，
 * 一个行为可以选择多个规则**」✓）
 *
 * 所以这里不再有"（不绑 —— 用你自己的推送配置）"那一项 ✗：
 * **一个都不选 = 这个行为不推** ✓（规则自带凭据，"你自己那份渠道配置"那一层已经没有了 ✗）。
 *
 * # 为什么单独一个组件
 *
 * 这一格有两条容易写歪、而且写歪了**不报错**的规则：
 *
 * 1. **一个都不选 = 不推**（不是"全推"✗，也不是"回落成某个默认配置"✗）——
 *    两处各写一份，迟早在某一处变成"默认勾上第一条规则" ✗。
 * 2. **悬空引用要兜底**：规则被删掉之后，定时/编排上那个 id 还在（后端没有级联删除 ✗，
 *    投递时按 id 找不到就跳过那一条 ✓）。不兜底的话 `Select` 找不到对应项，
 *    那一项会**静默消失** —— 而保存时带回去的仍是那个旧 id ⇒ 显示的和会发生的不是一回事 ✗。
 *    与编排页步骤那个「后端未注册」兜底项同一条口径 ✓。
 *
 * 两处（`schedules-page.tsx` / `pipelines-page.tsx`）用的是同一套控件、同一句文案 ✓。
 *
 * ⚠ 值走 `value={props.value}`（组件属性是**编译期 getter** ✓）——
 * 不要在调用方函数体里先把值读成数组再传进来（那会丢焦点，见 `push-editor.tsx` 文件头 ✓）。
 */
import type { PushRule } from '../api'
import { Select } from './select'

export interface PushRuleSelectProps {
  /**
   * 稳定标识。
   *
   * ⚠ **必须给，且要在重建前后保持不变** —— `Select` 靠它认人（面板挂在 body 上、
   * 展开状态是模块级的，见 `ui/select.tsx` 文件头 ✓）。
   */
  id: string
  /** 当前用户的规则清单（后端 `GET /api/push/rules` ✓）。 */
  rules: PushRule[]
  /** 当前绑的规则 id（**多选** ✓）。空数组 = 不推。 */
  value: string[]
  onChange: (value: string[]) => void
}

/** 一个都没选时触发器上的字 —— 两处必须一字不差，所以只写在这一处。 */
export const PUSH_RULE_NONE_LABEL = '不推（一个规则都不选）'

/** 悬空引用（规则已删除）那一项的后缀。 */
export const PUSH_RULE_MISSING_LABEL = '（规则已删除 —— 保存后会把它去掉）'

export function PushRuleSelect(props: PushRuleSelectProps) {
  /**
   * 选项：规则清单 + （必要时）悬空引用兜底。
   *
   * ⚠ 停用的规则**照样列出来**、只在名字后面标一下：把停用的规则从下拉里藏掉，
   * 会让"这条定时绑的规则不见了"变成一个查不出的谜 ✗（后端也不会因为停用就清掉绑定）。
   */
  const options = () => {
    const list = props.rules.map((r) => ({
      value: r.id,
      label: r.enabled ? r.name : `${r.name}（已停用）`,
    }))
    for (const v of props.value) {
      if (v !== '' && !list.some((o) => o.value === v)) {
        list.push({ value: v, label: PUSH_RULE_MISSING_LABEL })
      }
    }
    return list
  }

  return (
    <Select
      id={props.id}
      multiple
      class="w-full"
      ariaLabel="推送规则"
      placeholder={PUSH_RULE_NONE_LABEL}
      value={props.value}
      options={options()}
      onChange={props.onChange}
    />
  )
}
