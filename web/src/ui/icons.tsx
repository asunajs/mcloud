/**
 * 图标：内联 SVG，不引图标库。
 *
 * 规范里 `unplugin-icons` 尚未落地，而本项目只需要十来个图标，
 * 内联 SVG 反而更可控：无额外依赖、无构建插件、随 `currentColor` 自动跟随主题。
 *
 * 约定：
 * - 一律 `fill="currentColor"` —— 颜色交给外层文字色，这样 daisyUI 语义色
 *   与明暗主题都能自动生效。
 * - `aria-hidden` 用**布尔** `true`（写成字符串会 TS2322）。
 * - 返回类型显式标 `JSX.Element`，否则导航表里当值用时会推成 `unknown`（TS2786）。
 * - 尺寸由调用方给（`size-5` / `size-[1.2em]` / `size-4`），图标本身只兜底。
 *
 * ⚠ **刻意不做 `<Svg>{children}</Svg>` 那样的包装组件**：Vapor 下 JSX 的子节点
 * 编译成插槽（`_createComponent` 的第三个实参），包装组件的 `props.children`
 * 恒为 `undefined` —— 不报错、类型也过（除非显式声明 children 字段，那样又会
 * 反过来要求调用方传 props），只是渲染成空。所以这里用**共享属性对象**展开，
 * 每个图标自己写 `<svg>`。
 */

import type { JSX } from 'vue-jsx-vapor'

interface IconProps {
  class?: string
}

/** 24×24 网格的公共属性，展开进每个图标。 */
const BASE = {
  xmlns: 'http://www.w3.org/2000/svg',
  viewBox: '0 0 24 24',
  fill: 'currentColor',
  'aria-hidden': true,
} as const

export const IconDashboard = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M3 3h8v8H3V3Zm10 0h8v5h-8V3ZM3 13h8v8H3v-8Zm10 3h8v5h-8v-5Z" />
  </svg>
)

export const IconTask = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M9 16.2 4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4L9 16.2Z" />
  </svg>
)

export const IconHistory = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M13 3a9 9 0 1 0 8.5 12h-2.1A7 7 0 1 1 13 5v3l4-4-4-4v3Zm-1 5v5l4 2 .8-1.3-3.3-1.7V8H12Z" />
  </svg>
)

export const IconSettings = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm9 4c0-.6-.1-1.2-.2-1.8l2-1.5-2-3.4-2.3 1a7 7 0 0 0-3-1.7L15 2H9l-.5 2.6a7 7 0 0 0-3 1.7l-2.3-1-2 3.4 2 1.5a8 8 0 0 0 0 3.6l-2 1.5 2 3.4 2.3-1a7 7 0 0 0 3 1.7L9 22h6l.5-2.6a7 7 0 0 0 3-1.7l2.3 1 2-3.4-2-1.5c.1-.6.2-1.2.2-1.8Z" />
  </svg>
)

/** 用户：账号页。 */
export const IconUser = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M12 12a5 5 0 1 0 0-10 5 5 0 0 0 0 10Zm0 2c-4.4 0-8 2.2-8 5v1h16v-1c0-2.8-3.6-5-8-5Z" />
  </svg>
)

export const IconMenu = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M3 6h18v2H3V6Zm0 5h18v2H3v-2Zm0 5h18v2H3v-2Z" />
  </svg>
)

export const IconClose = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M18.3 5.7 12 12l6.3 6.3-1.4 1.4L10.6 13.4 4.3 19.7 2.9 18.3 9.2 12 2.9 5.7 4.3 4.3l6.3 6.3 6.3-6.3 1.4 1.4Z" />
  </svg>
)

/** 月亮：浅色主题下显示，表示"切到深色"。 */
export const IconMoon = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9Z" />
  </svg>
)

/** 太阳：深色主题下显示，表示"切到浅色"。 */
export const IconSun = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M12 17a5 5 0 1 1 0-10 5 5 0 0 1 0 10Zm0-13 2 2h-4l2-2Zm0 16 2 2h-4l2-2ZM4 12l2-2v4l-2-2Zm16 0-2-2v4l2-2ZM6.3 6.3l2.8.6-2.2 2.2-.6-2.8Zm11.4 11.4-2.8-.6 2.2-2.2.6 2.8Zm0-11.4-.6 2.8-2.2-2.2 2.8-.6ZM6.3 17.7l.6-2.8 2.2 2.2-2.8.6Z" />
  </svg>
)

/** 品牌标记。 */
export const IconBrand = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M7 18a4.5 4.5 0 0 1-.4-9A5.5 5.5 0 0 1 17.3 9.6 4.2 4.2 0 0 1 17 18H7Z" />
  </svg>
)

/** 警告三角，用于错误提示条前。 */
export const IconAlert = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M12 2 1 21h22L12 2Zm1 15h-2v-2h2v2Zm0-4h-2V8h2v5Z" />
  </svg>
)

/** 时钟：定时任务。 */
export const IconClock = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2Zm0 18a8 8 0 1 1 8-8 8 8 0 0 1-8 8Zm1-13h-2v6l5 3 1-1.7-4-2.3V7Z" />
  </svg>
)

/** 登出：门框 + 向外指的箭头。 */
export const IconLogout = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M10 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h5v-2H5V5h5V3Zm5.6 3.6L14.2 8l3 3H9v2h8.2l-3 3 1.4 1.4L21 12l-5.4-5.4Z" />
  </svg>
)

/** 任务编排：自上而下依次执行的步骤链。 */
export const IconPipeline = (p: IconProps): JSX.Element => (
  <svg {...BASE} class={p.class ?? 'size-5'}>
    <path d="M4 3h16v4H4V3Zm0 7h16v4H4v-4Zm0 7h16v4H4v-4Z" />
  </svg>
)
