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

export interface StatItem {
  title: string
  value: string
  /** 补充说明（今日增量等）。 */
  desc?: string
}

export const StatRow = (props: { items: StatItem[] }) => {
  if (props.items.length === 0) return null

  return (
    <div class="stats stats-vertical w-full bg-base-100 shadow-sm md:stats-horizontal">
      {props.items.map((item) => (
        <div key={item.title} class="stat">
          <div class="stat-title">{item.title}</div>
          <div class="stat-value text-2xl">{item.value}</div>
          {item.desc ? <div class="stat-desc">{item.desc}</div> : null}
        </div>
      ))}
    </div>
  )
}
