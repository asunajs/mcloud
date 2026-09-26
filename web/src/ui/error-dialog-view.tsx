/**
 * 错误弹窗渲染。与 `error-dialog.ts` 的 store 分离：那个管状态，这个管样子。
 *
 * # 只在根部挂一次
 *
 * 像 `ToastView` 一样全局唯一。每个页面各挂一份的话 `id` 会重复，
 * `showModal()` 就只打到先出现的那一个。
 *
 * # `<dialog>` **常驻**，只更新内容
 *
 * 这里刻意**不**做 `v-if` 式的条件挂载。原生 `showModal()` 要求元素已在
 * DOM 里，若让弹窗随内容出现/消失，"改状态 → 渲染 → 打开"就变成一场
 * 与 Vue 刷新时机赛跑（`queueMicrotask` 与 Vue 的调度队列谁先谁后并无保证）——
 * 输了就是点了按钮什么都不弹，且不报错。
 *
 * 常驻挂载后 `openDialog()` 是同步、可靠的，和本文件 `ConfirmDialog` /
 * `schedule-dialog` 的用法完全一致（那两处也都是常驻的）。
 * 内容用 `.value` 反应式绑定，所以"弹窗开着时又触发另一处失败"也会刷新文案。
 *
 * 没有错误时不渲染任何内容（`null` 插槽），弹窗只是躺在 DOM 里关着，不可见。
 */

import { Dialog } from './dialog'
import { dismissError, errorDialog } from './error-dialog'
import { ERROR_DIALOG_ID } from './error-dialog-ids'
import { IconAlert } from './icons'

export const ErrorDialogHost = () => (
  <Dialog
    id={ERROR_DIALOG_ID}
    // 标题取当前失败的动作；无错误时给个占位，反正不可见
    title={errorDialog.value?.title ?? '操作失败'}
    size="md"
    // ESC / 点遮罩 / 点按钮三条路都派发 close，统一在这一处收尾
    onClose={() => dismissError()}
  >
    {errorDialog.value ? (
      <>
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <IconAlert class="size-5 shrink-0" />
          <span class="break-words">{errorDialog.value.message}</span>
        </div>
        <div class="modal-action">
          {/*
            告示型弹窗**只有**「知道了」：这里没有可做的决定。
            放一个"取消"会暗示"这个弹窗本来可以不做" —— 而失败已经发生了。
          */}
          <button
            type="button"
            class="btn btn-primary min-h-11 md:min-h-0"
            onClick={() => dismissError()}
          >
            知道了
          </button>
        </div>
      </>
    ) : null}
  </Dialog>
)
