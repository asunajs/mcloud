/**
 * 指标卡组（组件指南 §5.1）。
 *
 * 两条硬要求：
 * - **移动端必须 `stats-vertical`**，桌面 `md:stats-horizontal` ——
 *   窄屏横排会挤到不可读。写法是先竖后横，不是反过来。
 * - 数值用 `text-2xl`，**不要默认更大那一档字号** —— 我们的数字（AI豆数）
 *   没有电商金额那么重要，字号再大一档一屏只能放两个。
 *   （注释里**不要**把那一档类名原样写出来：Tailwind 扫的是源码原文，
 *    写出来就会凭空生成一条没人用的 CSS —— 见 `app.css` 的 exclude 注释。）
 */

import { toneClass, type BadgeTone } from './badge'

export interface StatItem {
  title: string
  value: string
  /** 补充说明（今日增量等）。 */
  desc?: string
  /**
   * 卡片右上角那枚状态徽章。
   *
   * 只给**语义色调**，类名由 `ui/badge.ts` 的查表给出 —— 调用方拼接类名的话
   * Tailwind 扫不到那条字面量，样式会静默消失（§9.1①）。
   */
  badge?: StatBadge
}

export interface StatBadge {
  text: string
  tone: BadgeTone
}

export const StatRow = (props: { items: StatItem[] }) => {
  if (props.items.length === 0) return null

  return (
    <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
      {props.items.map((item) => (
        <div key={item.title} class="stat">
          {/* 标题与徽章同一行两端对齐：徽章是"这一格现在的处境"，
              和标题一起在第一眼的高度上（数值在下一行）。 */}
          <div class="stat-title flex items-center justify-between gap-2">
            <span>{item.title}</span>
            {item.badge ? (
              <span class={`badge badge-sm ${toneClass(item.badge.tone)}`}>{item.badge.text}</span>
            ) : null}
          </div>
          <div class="stat-value text-2xl">{item.value}</div>
          {item.desc ? <div class="stat-desc">{item.desc}</div> : null}
        </div>
      ))}
    </div>
  )
}
