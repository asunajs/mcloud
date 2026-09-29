/**
 * lite —— 给本项目用的轻量前端框架（无虚拟 DOM + 编译期优化，与 Vue Vapor 同路）。
 *
 * 语法子集**由普查决定**，不含"以后可能用得上"的东西：
 *
 * | API | 项目里的用量 | 为什么留 |
 * |---|---|---|
 * | `ref` | 66 处 | 唯一的响应式原语 |
 * | `onMounted` / `onUnmounted` | 11 / 5 处 | 拉数据、解绑全局事件 |
 * | `watch` | 1 处 | `app.tsx` 里盯登录态 |
 * | 组件（普通箭头函数） | 22 个 | 页面与 UI 单元 |
 * | 插槽 `slots.default?.()` | 1 处 | `dialog.tsx` |
 *
 * **没有** computed / reactive / provide-inject / nextTick / Teleport /
 * Transition / Suspense / 异步组件 / 指令（`v-if` 等一个都没用）——
 * 项目里没出现过的语法，这里就不实现。2026-09-30 起连 `computed`、`createStore`
 * 这两个"0 处使用"的也删了（当初只为兼容 Vue 而留），见 `signal.ts` 文件头。
 *
 * 下面分两组导出，**别把它们当成同一回事**：
 * 第一组是**编译器**往产物里 import 的（名单 = `lite/compiler.ts` 里所有 `this.h('…')`），
 * 名字一改编译产物就断；第二组是给**人**写的代码用的。
 */

// ── 编译器 import 的那批 ────────────────────────────────────────────────────
export { batch, effect, ref, type Ref, watch } from './signal'
export { createComponent } from './component'
export { createFor } from './control'
export { lazySlot, on, remove, setAttr, setClass, setNodes, setProp, setValue, spread, template } from './dom'

// ── 业务代码会手写到的那批 ─────────────────────────────────────────────────
export { type Component, mount, onMounted, onUnmounted, type Slots, useSlots } from './component'

// ── 运行时自己的内部件（demo / bench / 排障会直接调，业务代码不该用）────────
// `insert` / `createNodes` 是 `createFor` 与 `setNodes` 的实现细节；
// `setText` 是 `setNodes` 的"纯文本就只写文本"快路。留在这里是为了 demo 能直接压它们。
export { createNodes, insert, onRemove, setText } from './dom'
