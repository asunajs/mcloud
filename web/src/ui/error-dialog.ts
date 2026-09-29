/**
 * 全局错误弹窗 store。
 *
 * # 为什么不复用 `pushToast('error', …)`
 *
 * 右上角的提示条是**旁路**信息：用户可以不看。而"点了运行却没跑起来"
 * 这类失败是**用户当前动作的直接答案** —— 它必须出现在视线正中，
 * 且必须被确认一次才能继续。塞进角落会让人以为任务跑起来了。
 *
 * §5.11 说"错误不自动消失"，那正是为了让人读到；弹窗是这条规则的终点形态。
 *
 * # 什么该走这里，什么不该
 *
 * ✅ **独立动作失败**：点了按钮、没有表单在场（运行任务、启停定时、删除…）。
 * ❌ **整页加载失败**：那种要配「重试」按钮留在页内，弹窗反而挡住入口。
 * ❌ **表单/弹窗内的校验与保存失败**：错误要贴着待改的字段（§6.4），
 *    而且 /定时任务/编排/ 的保存本来就发生在 `<dialog>` 里 ——
 *    再套一层 `<dialog>` 是无效结构，ESC 与遮罩会打架。
 */

import { ref } from 'lite'
import { ERROR_DIALOG_ID } from './error-dialog-ids'
import { closeDialog, openDialog } from './dialog'

export interface ErrorDialogState {
  /** 标题写**哪件事**失败了，不写「错误」。 */
  title: string
  message: string
}

/** 当前要展示的错误；`null` = 不展示。同一时刻只留一条（后到的覆盖先到的）。 */
export const errorDialog = ref<ErrorDialogState | null>(null)

/**
 * 弹一个错误弹窗。
 *
 * `title` 用**动作**命名（「运行失败」「无法启用」），不要用「出错了」——
 * 用户要一眼知道是哪一步断的。
 *
 * 这里先写状态再 `showModal()`：`<dialog>` 由 `ErrorDialogHost` **常驻**挂载，
 * 所以元素一定已在 DOM 里，打开是同步且可靠的，不需要等渲染。
 */
export const showError = (title: string, message: string): void => {
  errorDialog.value = { title, message }
  openDialog(ERROR_DIALOG_ID)
}

/**
 * 关掉弹窗。
 *
 * ⚠ 两件事都要做，缺一不可：
 * - 清状态 —— 否则下次打开会先闪一眼上次的旧文案；
 * - `closeDialog()` —— `<dialog>` 是常驻元素，清状态**不会**让它消失
 *   （实测：只清状态时 `open` 属性仍在，弹窗留在屏幕上、内容变空）。
 */
export const dismissError = (): void => {
  errorDialog.value = null
  closeDialog(ERROR_DIALOG_ID)
}
