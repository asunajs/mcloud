/**
 * 手写内联 SVG 图标。
 *
 * 为什么不用图标库（设计方案 §12 未决项 1 曾留着要装 `unplugin-icons` + Iconify）：
 *  1. 内联 SVG 是**字面量**，不经过 Tailwind 的类名扫描，不会有 §9.1① 的静默失效；
 *  2. 少一个构建插件与一整套图标包依赖，体积可控；
 *  3. 我们真正需要的图标不到 20 个，且形状都是通用图形。
 *
 * 约定（改动时请遵守，否则会不一致）：
 *  - 一律 `24×24` viewBox、`stroke="currentColor"`、`fill="none"` —— 颜色由外层文字色决定，
 *    这样**自动跟随主题**，不要在这里写死任何颜色。
 *  - `stroke-width={1.75}`（比默认 2 略细，配合 §2.3③ 的扁平风格更干净）。
 *  - 尺寸不由图标决定：调用方用 Tailwind 的 `size-[1.2em]` / `size-5` 控制。
 */

import type { JSX } from 'vue-jsx-vapor'

interface IconProps {
  class?: string
}

/**
 * 所有图标的公共属性。抽出来是为了改一处即全局生效。
 *
 * `aria-hidden` 必须是**布尔** `true` 而不是字符串 `'true'` ——
 * TSX 里它被类型化成 `Booleanish`，写字符串会报 TS2322。
 */
const base = {
  xmlns: 'http://www.w3.org/2000/svg',
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  'stroke-width': 1.75,
  'stroke-linecap': 'round',
  'stroke-linejoin': 'round',
  'aria-hidden': true,
} as const

/** 总览：四格。 */
export const IconDashboard = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <rect x="3" y="3" width="7" height="7" rx="1.5" />
    <rect x="14" y="3" width="7" height="7" rx="1.5" />
    <rect x="3" y="14" width="7" height="7" rx="1.5" />
    <rect x="14" y="14" width="7" height="7" rx="1.5" />
  </svg>
)

/** 账号：单人。 */
export const IconAccount = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <circle cx="12" cy="8" r="3.5" />
    <path d="M5 20a7 7 0 0 1 14 0" />
  </svg>
)

/** 任务：清单。 */
export const IconTask = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M9 6h11M9 12h11M9 18h11" />
    <path d="M4 6l1 1 1.5-1.5M4 12l1 1 1.5-1.5M4 18l1 1 1.5-1.5" />
  </svg>
)

/** 兑换：礼盒。 */
export const IconExchange = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <rect x="3" y="8" width="18" height="4" rx="1" />
    <path d="M5 12v8a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-8M12 8v13" />
    <path d="M12 8S10.5 3 8.5 3a2.5 2.5 0 0 0 0 5M12 8s1.5-5 3.5-5a2.5 2.5 0 0 1 0 5" />
  </svg>
)

/** 直播小红花：五瓣花（花心 + 五片花瓣圆）。 */
export const IconFlower = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <circle cx="12" cy="12" r="2.4" />
    <circle cx="12" cy="6.5" r="3.1" />
    <circle cx="17.2" cy="10.3" r="3.1" />
    <circle cx="15.2" cy="16.5" r="3.1" />
    <circle cx="8.8" cy="16.5" r="3.1" />
    <circle cx="6.8" cy="10.3" r="3.1" />
  </svg>
)

/** 定时任务：时钟。 */
export const IconSchedule = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M12 7.5V12l3 2" />
  </svg>
)

/** 历史：回溯箭头。 */
export const IconHistory = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M3.5 12a8.5 8.5 0 1 0 2.5-6" />
    <path d="M3 4v4h4" />
    <path d="M12 8v4l3 1.5" />
  </svg>
)

/** 调试台：终端。 */
export const IconDebug = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <path d="M7.5 9.5 10 12l-2.5 2.5M12.5 15h4" />
  </svg>
)

/** 设置：齿轮（简化为圆 + 六齿，避免长 path 影响可读性）。 */
export const IconSettings = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 3v2.5M12 18.5V21M21 12h-2.5M5.5 12H3M18.4 5.6l-1.8 1.8M7.4 16.6l-1.8 1.8M18.4 18.4l-1.8-1.8M7.4 7.4 5.6 5.6" />
  </svg>
)

/** 菜单：汉堡（移动端抽屉开关）。 */
export const IconMenu = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M4 7h16M4 12h16M4 17h16" />
  </svg>
)

/** 关闭：叉。 */
export const IconClose = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M6 6l12 12M18 6 6 18" />
  </svg>
)

/** 浅色主题指示：月亮（点击可切到深色）。 */
export const IconMoon = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5Z" />
  </svg>
)

/** 深色主题指示：太阳（点击可切到浅色）。 */
export const IconSun = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
  </svg>
)

/** 眼睛：显示口令。 */
export const IconEye = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z" />
    <circle cx="12" cy="12" r="3" />
  </svg>
)

/** 划掉的眼睛：隐藏口令。 */
export const IconEyeOff = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M10.7 6.2A8.9 8.9 0 0 1 12 6c6 0 9.5 6 9.5 6a15.6 15.6 0 0 1-3.1 3.8M6.4 7.7A15.7 15.7 0 0 0 2.5 12S6 18 12 18a8.8 8.8 0 0 0 3.4-.6" />
    <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2M3 3l18 18" />
  </svg>
)

/** 警示：三角叹号（错误提示用）。 */
export const IconAlert = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class}>
    <path d="M12 4.5 21 19.5H3L12 4.5Z" />
    <path d="M12 10v4M12 16.8v.2" />
  </svg>
)

/** 云朵：品牌标识。填充实心，用于 logo。 */
export const IconCloud = (props: IconProps): JSX.Element => (
  <svg {...base} class={props.class} fill="currentColor" stroke="none">
    <path d="M7 18a4.5 4.5 0 0 1-.4-9A5.5 5.5 0 0 1 17.3 9.6 4.2 4.2 0 0 1 17 18H7Z" />
  </svg>
)
