/**
 * 弹窗骨架：原生 `<dialog>` + daisyUI 的 `modal`（组件指南 §5.8 已定这一形态）。
 *
 * # 为什么靠 id 找元素而不是模板 ref
 *
 * 这里是 Vapor 模式：`showModal()` 要在**点击那一刻**打到真实的 DOM 元素上，
 * 而 Vapor 的模板 ref 与 VDOM 那套语义不同（组件指南 §9 记过这个边界）。
 * `<dialog>` 的 id 是唯一的（组件指南 §5.8 那条"每个 modal 用唯一 id"），
 * 由调用方给 —— 于是"开哪个窗"这件事是一句 DOM 查找，不需要实例状态。
 *
 * # `onClose` 不是可选的装饰
 *
 * `dialog.close()`、点遮罩、按 ESC 三条路都会派发 `close`。
 * 登录弹窗必须在**任何一条**关闭路径上停掉轮询，所以关闭动作一律走事件，
 * 不去包每个 `close()` 调用点。
 */

import { useSlots } from 'vue'

/** 打开一个弹窗（找不到元素时静默：那是 id 写错，不该在运行时抛给用户一句英文）。 */
export const openDialog = (id: string) => {
  const node = document.getElementById(id)
  if (node instanceof HTMLDialogElement) node.showModal()
}

/** 关闭一个弹窗。 */
export const closeDialog = (id: string) => {
  const node = document.getElementById(id)
  if (node instanceof HTMLDialogElement) node.close()
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
 * 宽度档位。类名**字面量**必须留在本文件的表里：Tailwind 靠扫源码取类名，
 * 拼出来的 `max-w-${size}` 不会进产物（组件指南 §5.1 那条静默失效）。
 */
export type DialogSize = 'sm' | 'md' | 'lg'

const SIZE_CLASS: Record<DialogSize, string> = {
  sm: 'max-w-sm',
  md: 'max-w-md',
  lg: 'max-w-lg',
}

/**
 * 弹窗本体。
 *
 * 宽度只由 `SIZE_CLASS` 那一档给，**不再**写 `w-full max-w-[calc(100vw-2rem)]`：
 * 那一句把 daisyUI `.modal-box` 自带的 `width:91.6667%; max-width:32rem` 整个顶掉了，
 * 于是宽屏下弹窗撑满屏幕（2026-09-22 实机看到的就是这个）。窄屏不贴边这件事
 * daisyUI 的 `91.6667%` 已经管着，不需要那条"保护"。
 *
 * ⚠ 内容走**默认插槽**而不是 `props.children`。Vapor 下 JSX 的子节点编译成
 * `_createComponent(Comp, props, () => …)` 的**第三个实参**（插槽），
 * 所以 `props.children` 永远是 undefined —— 这里不报错、类型也过，只是弹窗里
 * 什么都不渲染。实际踩过，见 `docs/webui-code-guide.md` §9.1 那两条静默失效。
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
  /** **影响面**，要具体到数字/名字（组件指南 §6.4：只写「确定吗？」等于没确认）。 */
  impact: JSX.Element
  /** 确认按钮文案：**写动词**，不写「确定」（§5.8）。 */
  confirmLabel: string
  onConfirm: () => void
  busy?: boolean
}

/**
 * 危险动作确认弹窗。
 *
 * 主行动用 `btn-error`（实心）而不是 `btn-primary`：这一条按钮点下去是**删数据**，
 * 让它与"保存"长一样就等于把误点的概率摆上桌（设计指南 §2.3①：实心只有一个，
 * 而这里唯一该抢那个位置的正是不可逆的那一下）。
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
