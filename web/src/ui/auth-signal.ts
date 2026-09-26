/**
 * 认证闸的**共享信号** —— 零依赖，只放状态。
 *
 * # 为什么单独一个模块
 *
 * `api.ts` 需要在被拒时通知"该去登录了"，而登录页又需要调 `api.ts` 的
 * `request`。两边直接互相 import 就成环 —— ESM 能处理一部分环，
 * 但环里的顶层初始化顺序**取决于哪个模块先被求值**，
 * 于是"刷新页面时好用、点导航进来就白屏"这类症状会随机出现。
 *
 * 把状态放进这个**不 import 任何东西**（除了 vue 的 `ref`）的模块，
 * 两个方向就都只依赖它，环消失。
 *
 * # 三态 + 一态
 *
 * - `checking` —— 还没问后端（首屏那一下）
 * - `setup`    —— 后端说"没初始化"，要建管理员
 * - `login`    —— 已初始化，但没登录
 * - `ready`    —— 放行
 *
 * ⚠ `setup` 与 `login` 必须分开：它们的**界面完全不同**
 * （建管理员 vs 登录）。后端也是分开回 503 / 401 的，
 * 前端这里合并就等于把后端那份区分丢了。
 */

import { ref } from 'vue'

export type AuthState = 'checking' | 'setup' | 'login' | 'ready'

/** 当前认证状态。 */
export const authState = ref<AuthState>('checking')

/**
 * 认证"代次"。每次从"被拒"变成"放行"就 +1。
 *
 * 页面用它当 `key`（或 watch 它）来在登录成功后重新拉数据 ——
 * 否则登录前失败的那些请求把页面留在了错误态，
 * 而用户明明已经登录了。
 */
export const authEpoch = ref(0)

/**
 * 后端要求认证。
 *
 * 由 `api.ts` 的 `request` 在收到 401 / 503 时调用。
 * ⚠ 它**只推进状态**，不抛错、不跳转 —— 抛错的那一方是 `request` 自己，
 * 调用方仍然要处理自己那个 Promise 的失败。这里改的只是"整个应用该显示什么"。
 */
export const noteAuthRequired = (state: 'setup' | 'login'): void => {
  // 已经是这个状态就别再写：同一个页面上并发的几个请求会同时撞 401，
  // 每次写都会触发一次重渲染，而它们表达的是同一件事。
  if (authState.value === state) return
  authState.value = state
}

/**
 * 当前登录者的显示名。空串 = 还不知道。
 *
 * 放在这里而不是各组件里：`app.tsx` 要显示它，而设置它的地方有三个
 * （初始化、登录、刷新时探测）。三处各记一份必然出现"刚登录时是空的、
 * 刷新一下才出来"—— 而那正是最初写错的样子。
 */
export const currentUser = ref('')

/** 后端放行。`user` 给定时顺带记住是谁。 */
export const noteAuthOk = (user?: string): void => {
  if (user !== undefined) currentUser.value = user
  if (authState.value === 'ready') return
  authState.value = 'ready'
  authEpoch.value += 1
}

/**
 * 从后端错误里判断"是不是认证问题"，是就推进状态。
 *
 * 判据用**状态码**（401 / 503）而不是错误文案：
 * 文案是给人看的，改一个字就会让这段逻辑静默失效。
 *
 * 503 只在带 `code: "setup_required"` 时才算 —— 后端 503 也可能
 * 表示别的服务不可用，那时该显示"后端有问题"而不是"请建管理员"。
 */
export const classifyAuthFailure = (status: number, code: unknown): 'setup' | 'login' | null => {
  if (status === 401) return 'login'
  if (status === 503 && code === 'setup_required') return 'setup'
  return null
}
