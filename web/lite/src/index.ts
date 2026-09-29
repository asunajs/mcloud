/**
 * lite —— 给本项目用的轻量前端框架（Vapor 同路：无虚拟 DOM + 编译期优化）。
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
 * 项目里没出现过的语法，这里就不实现。
 */

export { batch, computed, effect, field, type Ref, ref, watch } from './signal'
// Vapor 的编译产物里，副作用写作 `renderEffect`；本实现它就是 effect，导出个别名免得两套词
export { effect as renderEffect } from './signal'
export { createNodes, insert, on, onRemove, remove, setAttr, setClass, setNodes, setProp, setText, spread, template } from './dom'
export { createFor } from './control'
export { type Component, createComponent, defineVaporComponent, mount, onMounted, onUnmounted, type Slots, useSlots } from './component'
export { createStore } from './store'
