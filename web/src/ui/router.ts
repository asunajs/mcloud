/**
 * 路径路由 —— **干净 URL，不用 `#`**。原语来自 `@asunajs/lite` 的 `createRouter`。
 *
 * # 本文件现在只剩"本应用的路由表"
 *
 * 2026-10-08 起，`pushState` / `popstate` / 历史栈去重 / 单一写入点这些**原语**由 lite
 * 提供（同一个组织下的 unpanel 也要用，两边不再各写一份 —— 见 unpanel 的 D23）。
 * 原先手写那 65 行的价值几乎全在注释里，那些知识已经搬进 lite 的 `src/router.ts` 文件头。
 *
 * 这里只声明**本应用拥有哪些地址**，以及导出页面/地址两个读数。
 *
 * # 为什么一个页面要写多个模式
 *
 * 一页常常不止一个地址：子视图（`/tasks/config`、`/accounts/settings`）与**改名后要
 * 继续认的老地址**（`/logs`）。lite 的路由支持"一个名字拥有多个模式"，第一个是规范
 * 地址（= `NAV_ITEMS` 里的 `href`），其余是"也归它" ✓。
 *
 * 原先"子路由按**第一段**归属"是 `ui/nav.ts::parsePath` 里的一条**隐式规则**，加路由时
 * 很容易漏掉；现在它在表里一眼可见。**代价**：没列进来的子路径不再自动归属，
 * 而是落到兜底的总览 —— 所以**新增子视图时要往这张表里加一条**。
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
 * 哪天要挂到 `/mcloud/` 下，得给 `index.html` 加 `<base>` 并让 lite 的路由跟着走。
 */

import { computed, createRouter, type ReadonlyRef } from '@asunajs/lite'
import { NAV_ITEMS, type PageId } from './nav'

/**
 * 一个页面**除规范地址之外**还拥有的模式。
 *
 * ⚠ 用**前缀模式**（末段 `*`）而不是把深度枚举出来 —— 子视图深度是**数据驱动**的
 * （`/accounts/<id>/<tab>/<sub>`），枚举必然漏，而漏掉的表现是**静默落到兜底页**
 * （地址对、界面却是另一页）。这不是假设：迁移时正是这么挂了 53 项验收
 * （先漏 4 段的 `/accounts/settings/<a>/<b>`，补完显式模式后又漏 `/settings/security`）。
 *
 * 于是"一页拥有它前缀下的整片地址"—— 这正是原先 `ui/nav.ts::parsePath` 那条
 * "子路由按**第一段**归属"的规则，只是从隐式变成显式、写在表里。
 */
const EXTRA_PATTERNS: Partial<Record<PageId, string[]>> = {
  // `/accounts/settings`、`/accounts/upstream`、老地址 `/accounts/push`
  accounts: ['/accounts/*'],
  // `/tasks/config`（全局任务配置）与 `/tasks/<任务名>/config`
  tasks: ['/tasks/*'],
  // `/settings/security`、`/settings/events`、`/settings/about`
  settings: ['/settings/*'],
  // ⚠ 2026-10-08 起「日志」并进「历史」页当第二个 tab，但 `/logs` 这个**地址**照旧认
  // —— 书签、浏览器历史、旧文档里的它不能 404，也不能把人甩回总览
  //（与 `/accounts/push` 那条老地址兼容同一个口径：改名只该影响**新**链接）。
  history: ['/logs'],
}

const routes = Object.fromEntries(
  NAV_ITEMS.map((item) => [item.id, [item.href, ...(EXTRA_PATTERNS[item.id] ?? [])]]),
) as Record<PageId, string[]>

export const router = createRouter({ routes, fallback: 'dashboard' })

/**
 * 当前地址（`location.pathname`）的**信号化副本**。
 *
 * # 它解决什么
 *
 * 一个导航项底下可能有**多个 tab**（「历史」页就是：`/history` 与 `/logs` 都归它 ✓）
 * —— 光看 `PageId` 分不出该显示哪一个 tab ✗。让那个页面 `watch` 它，就能
 * "地址一变、tab 跟着换" ✓。
 *
 * # ⚠ 它**不是**切页的依据
 *
 * 切页只看 `page`。这个信号只是**读数**，不参与"该渲染哪一页"的判断 ✓
 * （谁拿它去切页，就等于把切页逻辑拆成两份，两边迟早分叉 ✗）。
 *
 * # 谁来推进
 *
 * 从 lite 的路由派生（`computed`）—— 于是它和 `page` **同源**，
 * 原先"两句的先后顺序不能反"那条纪律**结构上不可能违反**了 ✓。
 */
export const currentPath: ReadonlyRef<string> = computed(() => router.route.value.path)

/**
 * 当前页面。
 *
 * 路由名就是 `PageId`（路由表由 `NAV_ITEMS` 生成 ⇒ 两者不可能对不上）✓。
 * 与 `currentPath` 同源，一起更新。
 */
export const page: ReadonlyRef<PageId> = computed(() => router.route.value.name as PageId)

/**
 * 跳到一个应用内路径。
 *
 * ⚠ 收的是**路径**而不是路由名：子视图（`/tasks/config`、`/tasks/<任务名>/config`）
 * 在调用处就是拼出来的字面量，按名字生成反而要绕（`/tasks/config` 与 `:name` 同形）。
 * lite 也提供了按名字跳的 `navigate(name, args)`，本仓暂未用 —— 要用时从 `router` 上取。
 *
 * ⚠ `pushState` **不会**触发 `popstate`（浏览器只在"用户真的导航"时发它）。
 * lite 的路由在 `push()` 里自己同步了一次 —— 好处是**切页只有一条代码路径**：
 * 浏览器前进/后退、手敲地址、以及页面里点按钮，最后都落到同一个 `route` 上。
 *
 * 少了这一步的症状很好认：点导航按钮**页面不动**，但按一下浏览器后退却动了
 * —— 因为"改 URL"与"换页面"被拆成了两件事。
 */
export const navigate = (href: string): void => {
  router.push(href)
}
