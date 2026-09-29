/**
 * 状态管理（可选模块）。
 *
 * 本项目现在的"状态管理"就是**模块级 `ref`**（`ui/auth-signal.ts` 导出
 * `authEpoch` / `currentUser`），够用且零成本。这里给的是一层更顺手的写法：
 * 把普通对象的字段原地换成 getter/setter，于是 `s.count++` 直接是响应式的。
 *
 * 关键在于**不用 Proxy**：Proxy 每次读写都过陷阱（慢），而且要求运行期知道键名。
 * 本项目的状态键都是静态的、数量也少，`defineProperty` 一次性搞定，代价只有
 * 每字段两个闭包。
 *
 * 如果哪天确定不需要它，整份文件删掉即可 —— 运行时的其余部分一行都不用改。
 */

import { field } from './signal'

/** 把一个普通对象变成响应式（**原地改**，返回同一个引用）。 */
export function createStore<T extends Record<string, unknown>>(init: T): T {
  for (const k of Object.keys(init)) {
    const f = field(init[k])
    Object.defineProperty(init, k, { get: f.get, set: f.set, enumerable: true, configurable: true })
  }
  return init
}
