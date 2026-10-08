/**
 * 路径路由 —— **干净 URL，不用 `#`**。
 *
 * # 为什么改（用户口径 2026-10-03：「先去掉#号」）
 *
 * `#/tasks` 那种 URL 有两个实际问题：分享/收藏时那一段像"锚点"而不像地址，
 * 以及反代/日志里 `#` 后面那一截**根本不会发给服务端**（排查问题时看不见）。
 *
 * # 服务端前提（**已经满足**，不需要动 Rust）
 *
 * `crates/mcloud-server/src/static_files.rs` 的 SPA 回落：认不出、且**末段不带点**
 * 的路径一律发 `index.html` ⇒ 直接刷新 `/tasks` 不会 404 ✓。
 *
 * ⚠ **代价要记住**：那条回落规则把"末段带点"的路径当静态资源（有意为之，
 * 防止发版后旧 `index.html` 去要已经不存在的旧 chunk —— 见该文件里的实测记录）。
 * 所以路由段里**不能出现 `.`**。今天全部安全：任务名是 Rust 标识符、
 * 账号 id 是 16 位十六进制 ✓。**新增路由时要过一遍这条**。
 *
 * ⚠ 另一个代价：hash 路由天然与"挂在哪个路径下"无关，干净路径不是 ——
 * 本仓的 WebUI 由服务端**根路径**发（`web/dist`），没有子路径挂载的用法 ✓；
 * 哪天要挂到 `/mcloud/` 下，得给 `index.html` 加 `<base>` 并让这里的 href 跟着走。
 */

import { ref } from '@asunajs/lite'

/**
 * 当前地址（`location.pathname`）的**信号化副本**。
 *
 * # 它解决什么
 *
 * 一个导航项底下可能有**多个 tab**（2026-10-08 起「历史」页就是：`/history` 与
 * `/logs` 都归 `history` 这一页 ✓）—— 光看 `PageId` 分不出该显示哪一个 tab ✗。
 * 让那个页面 `watch` 它，就能"地址一变、tab 跟着换" ✓。
 *
 * # ⚠ 它**不是**切页的依据
 *
 * 切页只有一条路：`app.tsx` 那条 `popstate` 监听器 + `parsePath()` ✓。
 * 这个信号只是**读数**，不参与"该渲染哪一页"的判断 ✓（谁拿它去切页，
 * 就等于把切页逻辑拆成两份，两边迟早分叉 ✗）。
 *
 * # 谁来推进
 *
 * 只有 `app.tsx` 的 `onNavigate`（浏览器前进/后退、手敲地址、以及 `navigate()`
 * 补发的那次 `popstate` 都会经过它 ✓）—— **单一写入点** ✓。
 */
export const currentPath = ref(location.pathname)

/**
 * 跳到一个应用内路径。
 *
 * ⚠ `pushState` **不会**触发 `popstate`（浏览器只在"用户真的导航"时发它）。
 * 所以这里自己广播一次 —— 好处是**切页只有一条代码路径**：
 * 浏览器前进/后退、手敲地址、以及页面里点按钮，最后都落到
 * `app.tsx` 那条唯一的 `popstate` 监听器上。
 *
 * 少了这一步的症状很好认：点导航按钮**页面不动**，但按一下浏览器后退却动了
 * —— 因为"改 URL"与"换页面"被拆成了两件事。
 */
export const navigate = (href: string): void => {
  // 已经在目标上就别往历史里塞重复项：否则用户按后退会"原地不动"一次，
  // 那是历史栈被污染，不是他点错了。
  if (location.pathname + location.search === href) return
  history.pushState({}, '', href)
  window.dispatchEvent(new PopStateEvent('popstate'))
}
