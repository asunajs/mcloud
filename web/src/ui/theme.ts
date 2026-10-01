/**
 * 主题切换。
 *
 * 两个主题（light / dark）都支持**手动切换**，选择持久化到 localStorage。
 *
 * 三条约定（改动前先读）：
 *
 * 1. **唯一的状态来源是 `<html data-theme>`**。CSS 侧刻意没用 daisyUI 的
 *    `--prefersdark` —— 那是"系统偏好深色时自动切"，会抢手动选择的状态
 *    （用户选了浅色、系统主题一变就被顶回去）。「跟随系统」只作为
 *    **初始值**在这里算一次，之后不再参与。
 *
 * 2. **必须在首屏渲染前应用**，否则会看到先浅后深的一闪（FOUC）。
 *    这段逻辑在 `index.html` 里另有一份同步副本 —— 两处必须保持一致。
 *
 * 3. **localStorage 可能被隐私模式禁用**，读写都要 try/catch：
 *    拿不到就退化成"本次会话有效的切换"，而不是整个页面报错。
 */

export type Theme = 'light' | 'dark'

/** 持久化用的键名。`index.html` 的内联脚本也用它，改名要同步。 */
const STORAGE_KEY = 'mcloud-theme'

/** 切换的下一个主题。**纯函数**，独立出来便于直接测。 */
export const nextTheme = (current: Theme): Theme => (current === 'dark' ? 'light' : 'dark')

/** 初始主题：用户存过的优先，否则跟随系统偏好。 */
export const initialTheme = (): Theme => {
  try {
    const saved = localStorage.getItem(STORAGE_KEY)
    if (saved === 'light' || saved === 'dark') return saved
  } catch {
    // 忽略：读不到就按系统偏好走
  }
  return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

/** 读**当前实际生效**的主题（以 DOM 为准，不是以我们的记忆为准）。 */
export const currentTheme = (): Theme => {
  const attr = document.documentElement.dataset.theme
  return attr === 'dark' ? 'dark' : 'light'
}

/** 切换并按需持久化。供顶栏那个按钮调用 —— **唯一的主题入口**。
 *
 * （2026-10-01 曾有过第二个入口：设置页的「外观」两档。用户判定那是重复入口
 *  ——"主题没用"—— 于是连同为它引入的共享信号模块一起删了。要再加第二个入口，
 *  记得先想清楚两个入口怎么不分叉。）*/
export const toggleTheme = (): Theme => {
  const next = nextTheme(currentTheme())
  applyTheme(next)
  return next
}

/** 应用主题：写 DOM（立即生效）+ 尽力持久化（失败不影响本次切换）。 */
export const applyTheme = (theme: Theme): void => {
  document.documentElement.setAttribute('data-theme', theme)
  try {
    localStorage.setItem(STORAGE_KEY, theme)
  } catch {
    // 隐私模式：不持久化也比抛错好 —— 切换本身仍然生效
  }
}
