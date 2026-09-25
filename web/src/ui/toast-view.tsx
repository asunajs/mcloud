/**
 * 提示条渲染。与 `toast.ts` 的 store 分离：那个管状态，这个管样子。
 *
 * 位置固定在右上角（§5.11）—— 不用底部，会和移动端 dock 打架。
 */

import { dismissToast, toasts, TOAST_CLASS } from './toast'
import { IconClose } from './icons'

export const ToastView = () => {
  if (toasts.value.length === 0) return null

  return (
    <div class="toast toast-end toast-top z-50">
      {toasts.value.map((t) => (
        <div key={t.id} class={`alert ${TOAST_CLASS[t.level]} text-sm`} role="status">
          <span class="break-words">{t.message}</span>
          <button
            type="button"
            class="btn btn-square btn-ghost btn-xs"
            aria-label="关闭提示"
            onClick={() => dismissToast(t.id)}
          >
            <IconClose class="size-3" />
          </button>
        </div>
      ))}
    </div>
  )
}
