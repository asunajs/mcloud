/**
 * 空态（组件指南 §5.10）。
 *
 * 规范原话：**不要**只写「暂无数据」四个字 —— 它没告诉用户下一步做什么。
 * 所以这里 `hint` 是必填的，且调用方应当尽量给一个可点的动作。
 */


export const EmptyState = (props: {
  title: string
  /** 下一步该做什么。必填 —— 逼调用方想清楚，而不是甩一句"暂无数据"。 */
  hint: string
  action?: JSX.Element
}) => (
  <div class="flex flex-col items-center gap-3 py-10 text-center">
    <p class="text-sm opacity-70">{props.title}</p>
    <p class="max-w-md text-xs opacity-60">{props.hint}</p>
    {props.action}
  </div>
)
