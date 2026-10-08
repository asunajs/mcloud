/**
 * 弹窗骨架：原生 `<dialog>` + daisyUI 的 `modal`（组件指南 §5.8）。
 *
 * # 为什么靠 id 找元素而不是模板 ref
 *
 * lite **没有模板 ref** 这条路 —— `ref=` 不是编译器支持的属性，写上去会被当成普通属性
 * 静默编错（实测产物：`_$setAttr(el, "ref", fn)`，把函数串写进 DOM；
 * 见 `asunajs/lite` 的 `docs/guide.md` §10），而 `showModal()` 要在**点击那一刻**
 * 打到真实 DOM 元素上。`<dialog>` 的 id 是唯一的，
 * 由调用方给 —— 于是"开哪个窗"是一句 DOM 查找，不需要实例状态。
 *
 * # `onClose` 不是可选装饰
 *
 * `dialog.close()`、点遮罩、按 ESC 三条路都会派发 `close`。
 * 关闭动作一律走事件，不去包每个 `close()` 调用点。
 */

import { useSlots } from '@asunajs/lite'

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
      {/*
        # 窄屏上"没有出口"是个真 bug（2026-10-01 用户实测：手机上加账号弹窗点不到取消）

        三个原因叠在一起，缺一条都还会踩：
        1. daisyUI 给 `.modal-box` 的是 `max-height: 100vh` —— 手机浏览器的 `100vh`
           **包含地址栏那块**，于是弹窗底部（含按钮）被切到屏幕外；
        2. 遮罩是取消的唯一入口，而弹窗几乎铺满屏 ⇒ 遮罩只剩顶上一条细缝，点不到；
        3. 添加账号那个弹窗**本来就没有取消按钮**（只有各方式自己的提交键）。

        ⇒ 这里一次性给所有弹窗补上：**高度用 `dvh`**（真实可见视口）、
        **标题栏 + ✕ 常驻不滚动**、**只有内容区滚**。
        ✕ 是唯一一条"任何高度下都够得着"的出口，所以它不能跟着内容滚走。

        ⚠ 内容外面那层 `flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto`：
        `min-h-0` 不能省（否则 flex 子项不肯收缩，滚动条出不来），
        `gap-3` 是把原来 modal-box 上的间距**搬进来**（否则消费者那些直接子节点会挤在一起）。
      */}
      <div
        class={`modal-box flex max-h-[calc(100dvh-2rem)] flex-col gap-3 overflow-hidden ${SIZE_CLASS[props.size ?? 'lg']}`}
      >
        <div class="flex items-center justify-between gap-2">
          <h3 class="text-base font-medium">{props.title}</h3>
          <button
            type="button"
            class="btn btn-sm btn-ghost btn-circle -mr-2"
            aria-label="关闭"
            onClick={() => closeDialog(props.id)}
          >
            ✕
          </button>
        </div>
        <div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">{slots.default?.()}</div>
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
