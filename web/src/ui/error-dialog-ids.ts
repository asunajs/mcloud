/**
 * 错误弹窗的 DOM id。
 *
 * 单独一个文件是为了**打断循环依赖**：store（`error-dialog.ts`）要拿这个 id
 * 去 `openDialog()`，而渲染组件（`error-dialog-view.tsx`）要用 store 的状态 ——
 * 两边互相 import 会绕成一个环。id 是两边唯一共用的东西，抽出来最干净。
 */
export const ERROR_DIALOG_ID = 'error-dialog'
