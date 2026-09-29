/**
 * 轻提示 store（≈20 行，不引依赖）。
 *
 * 规范（§5.11）：右上角、自动消失 3 秒、**错误不自动消失**、一屏最多 2 条。
 *
 * 约定：
 * - 位置 `toast-end toast-top`（右上角）—— 不用底部，会和移动端 dock 打架。
 * - 错误必须手动关：它往往带着"该去看哪"的信息，3 秒不够读完。
 */

import { ref } from 'lite'

export type ToastLevel = 'info' | 'success' | 'warning' | 'error'

export interface Toast {
  id: number
  level: ToastLevel
  message: string
}

/** 一屏最多堆 2 条，多了替换最旧的（§5.11）。 */
const MAX_TOASTS = 2
/** 非错误提示的存活时间。 */
const AUTO_DISMISS_MS = 3000

export const toasts = ref<Toast[]>([])
let nextId = 1

/** 关掉指定提示。 */
export const dismissToast = (id: number): void => {
  toasts.value = toasts.value.filter((t) => t.id !== id)
}

/**
 * 弹一条提示。
 *
 * 返回其 id，便于调用方在动作完成时提前关掉（例如"处理中"→"完成"）。
 */
export const pushToast = (level: ToastLevel, message: string): number => {
  const id = nextId++
  // 超出上限时丢掉最旧的一条
  const next = [...toasts.value, { id, level, message }]
  toasts.value = next.length > MAX_TOASTS ? next.slice(next.length - MAX_TOASTS) : next

  // 错误不自动消失（§5.11）：它带的信息通常需要用户读完再决定下一步
  if (level !== 'error') {
    setTimeout(() => dismissToast(id), AUTO_DISMISS_MS)
  }
  return id
}

/** 提示条的样式查表。**字面量**，不可拼接（§9.1①）。 */
export const TOAST_CLASS: Record<ToastLevel, string> = {
  info: 'alert-soft alert-info',
  success: 'alert-soft alert-success',
  warning: 'alert-soft alert-warning',
  error: 'alert-soft alert-error',
}
