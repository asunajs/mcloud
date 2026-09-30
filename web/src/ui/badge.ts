/**
 * 徽章的**语义色调 → 类名**。
 *
 * 为什么单独一个模块：色调是"这块数据现在的处境"，而**渲染它的地方不止一处**
 * （指标卡右上角、定时任务那几行的上次结果）。类名表写在各自的页面里，
 * 两份迟早会分叉 —— 一处改了颜色、另一处没改，而它不报错。
 *
 * ⚠ **查表，不拼接**（§9.1①）：Tailwind 扫的是源码原文，`badge-soft badge-${tone}`
 * 这种写法的产物里根本没有那条 CSS，样式会静默消失。
 *
 * ⚠ `plain` 刻意用裸 `badge-soft`（不给颜色类）：`badge-soft badge-neutral`
 * 在深色主题下只有 1.23:1（黑字压黑底，组件指南 §5.4 的实测）。
 */

/** 语义色调。`plain` = 中性（不做褒贬），例如"跳过""暂无记录"。 */
export type BadgeTone = 'ok' | 'warn' | 'bad' | 'plain'

const TONE_CLASS: Record<BadgeTone, string> = {
  ok: 'badge-soft badge-success',
  warn: 'badge-soft badge-warning',
  bad: 'badge-soft badge-error',
  plain: 'badge-soft',
}

/** 取色调对应的修饰类。**值里只有修饰符**，基类 `badge` 由模板写（`badge ${…}`）。 */
export const toneClass = (tone: BadgeTone): string => TONE_CLASS[tone]
