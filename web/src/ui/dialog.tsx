/**
 * 弹窗骨架：原生 `<dialog>` + daisyUI 的 `modal`（组件指南 §5.8）。
 *
 * # 为什么靠 id 找元素而不是模板 ref
 *
 * Vapor 模式下 `showModal()` 要在**点击那一刻**打到真实 DOM 元素上，
 * 而 Vapor 的模板 ref 与 VDOM 那套语义不同。`<dialog>` 的 id 是唯一的，
 * 由调用方给 —— 于是"开哪个窗"是一句 DOM 查找，不需要实例状态。
 *
 * # `onClose` 不是可选装饰
 *
 * `dialog.close()`、点遮罩、按 ESC 三条路都会派发 `close`。
 * 关闭动作一律走事件，不去包每个 `close()` 调用点。
 */

import { useSlots } from 'vue'
import type { JSX } from 'vue-jsx-vapor'

/** 打开一个弹窗（找不到元素时静默：那是 id 写错，不该在运行时抛一句英文给用户）。 */
export const openDialog = (id: string) => {
  const node = document.getElementById(id)
  if (node instanceof HTMLDialogElement) node.showModal()
}

/** 关闭一个弹窗。 */
export const closeDialog = (id: string) => {
  const node = document.getElementById(id)
  if (node instanceof HTMLDialogElement) node.close()
}

/**
 * 宽度档位。类名**字面量**必须留在本文件的表里：Tailwind 靠扫源码取类名，
 * 拼出来的 `max-w-${size}` 不会进产物（§9.1① 的静默失效）。
 */
export type DialogSize = 'sm' | 'md' | 'lg'

const SIZE_CLASS: Record<DialogSize, string> = {
  sm: 'max-w-sm',
  md: 'max-w-md',
  lg: 'max-w-lg',
}

export interface DialogProps {
  id: string
  title: string
  /** 宽度档位，默认 `lg`（= daisyUI 自带的 32rem）。 */
  size?: DialogSize
  /** 关掉时（ESC / 遮罩 / 显式 close）要打的那一下。 */
  onClose?: () => void
}

/**
 * 弹窗本体。
 *
 * 宽度只由 `SIZE_CLASS` 那一档给，**不再**自己写 `w-full` 加一个按视口宽度
 * 算出来的 max-w 任意值：那一句会把 daisyUI `.modal-box` 自带的
 * `width:91.6667%; max-width:32rem` 整个顶掉，于是宽屏下弹窗撑满屏幕。
 * 窄屏不贴边这件事 daisyUI 已经管着。
 * （顺带：任意值写在注释里也会被 Tailwind 当成候选类名，凭空生成一条死 CSS。）
 *
 * ⚠ 内容走**默认插槽**而不是 `props.children`。Vapor 下 JSX 的子节点编译成
 * `_createComponent(Comp, props, () => …)` 的**第三个实参**（插槽），
 * 所以 `props.children` 永远是 undefined —— 不报错、类型也过，只是弹窗里
 * 什么都不渲染。
 */
export const Dialog = (props: DialogProps) => {
  const slots = useSlots()
  return (
    <dialog id={props.id} class="modal" onClose={() => props.onClose?.()}>
      <div class={`modal-box flex flex-col gap-3 ${SIZE_CLASS[props.size ?? 'lg']}`}>
        <h3 class="text-base font-medium">{props.title}</h3>
        {slots.default?.()}
      </div>
      <form method="dialog" class="modal-backdrop">
        <button>关闭</button>
      </form>
    </dialog>
  )
}

export interface ConfirmDialogProps {
  id: string
  title: string
  /** **影响面**，要具体到数字/名字（§6.4：只写「确定吗？」等于没确认）。 */
  impact: JSX.Element
  /** 确认按钮文案：**写动词**，不写「确定」（§5.8）。 */
  confirmLabel: string
  onConfirm: () => void
  busy?: boolean
}

/**
 * 危险动作确认弹窗。
 *
 * 主行动用 `btn-error`（实心）而不是 `btn-primary`：这一条点下去是**删数据**，
 * 让它与"保存"长一样等于把误点概率摆上桌 —— 实心只有一个，
 * 而这里唯一该抢那个位置的正是不可逆的那一下。
 */
export const ConfirmDialog = (props: ConfirmDialogProps) => (
  <Dialog id={props.id} title={props.title} size="sm">
    <div class="text-sm leading-6">{props.impact}</div>
    <div class="modal-action">
      <button type="button" class="btn btn-soft" onClick={() => closeDialog(props.id)}>
        取消
      </button>
      <button
        type="button"
        class="btn btn-error"
        disabled={props.busy === true}
        onClick={props.onConfirm}
      >
        {props.confirmLabel}
      </button>
    </div>
  </Dialog>
)
