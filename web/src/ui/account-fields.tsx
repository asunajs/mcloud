/**
 * 每账号设置里的「任务参数」+「功能开关」四格 —— **工厂函数**，不是组件。
 *
 * # 为什么抽成共享模块
 *
 * 这四格原先只长在 `pages/accounts-page.tsx` 里。「配置中心」要按**作用域**分成
 * 「公共配置」与「账号私有配置」，账号私有 = `AccountSettings` 里**按账号算**的那几格。
 * ⚠ 2026-10-08 起 `AccountSettings` **没有** `notify` 那一格了（推送渠道收成登录
 * 用户级 ⇒ 见 `./push-editor`，由「推送」页（`pages/push-page.tsx`）用 ✓）—— 所以本模块就是
 * `AccountSettings` 的**全部**格子 ✓。抽成模块的唯一目的：
 * **渲染与本地校验只有这一份** —— 抄第二份迟早分叉
 * （本仓栽过一次：签到族名单硬编码在前端，两处各改各的，"能跑的任务"和
 * "列出来的任务"对不上）。
 *
 * # 为什么是工厂而不是组件
 *
 * 同 `./push-editor`：这四格的值**归属**因页面而异（账号页存的是"某个账号的
 * 一份设置"，配置中心存的是草稿），工厂只负责**画**与**攒 payload**，
 * 值存在编辑器自己的 ref 里、由调用方决定"什么时候 load / 什么时候 build /
 * 什么时候快照"。组件做不到这件事：组件体只跑一次，props 不会更新。
 *
 * # ⚠ 为什么内部值一律走 thunk（`get: () => string`），不是当场读
 *
 * lite 是编译期的"指哪儿改哪儿"：一个 `{expr}` 只重建**读到过变化信号**的那一小块。
 * - 普通函数被 `{taskFields()}` / `{featureFields()}` 这样调用时，调用本身发生在一条
 *   effect 里；若在这里**当场**读出值（`const v = fBackupWait.value`）再当字符串
 *   传下去，那条 effect 就订阅了它 —— 敲一个字整块表单重建、焦点全丢
 *   （2026-10-03 review §2.2 记的就是这个真 bug，搬迁时**必须保持这个形状**）。
 *   写成 `value={get()}` 时读发生在**属性自己的 effect** 里，只更新那一个输入框。
 * - 组件不行：组件的 props 是在组件体里读的，而组件体只跑一次、不在任何 effect 里，
 *   读到的值不会更新（本框架没有虚拟 DOM 的那层 diff）。
 *
 * ⚠ 数字一律以**字符串**存：原样回显用户敲的东西，解析放到**提交那一刻**
 * （`build()`）—— 边敲边解析的话，"1." 这种中间态会被判成非法，用户还没敲完
 * 就先看到红字。
 */

import { ref } from '@asunajs/lite'
import type { AccountSettings } from '../api'

// ── 每账号设置的纯函数（不碰 DOM，可单测）───────────────────────────────

/**
 * 解析一个"必须是整数"的输入。
 *
 * 空串**不当 0**：`backupWaitSecs` 的 0 有明确语义（"不等"），替用户拍板成 0
 * 等于悄悄改掉一项行为。留空请他显式写 0。
 *
 * ⚠ 这里**只**管"是不是整数"，**不管范围** —— 范围以后端为准
 * （`AccountSettingsDto::BACKUP_WAIT_SECS_MAX` 等），前端再实现一遍必然分叉。
 */
const parseIntField = (label: string, raw: string): number => {
  const t = raw.trim()
  if (t === '') throw new Error(label + ' 不能留空（要 0 就写 0）')
  if (!/^-?\d+$/.test(t)) throw new Error(label + ' 必须是整数，当前是「' + t + '」')
  const n = Number(t)
  if (!Number.isSafeInteger(n)) throw new Error(label + ' 超出可表示的范围')
  return n
}

/**
 * 逗号 / 空白分隔的**任务 id**。
 *
 * 非数字当场报错：后端对 `["签到"]` 只会回一句 serde 的类型错误，用户看不懂
 * "为什么我填了它说格式不对"。旧形状（任务名）是**后端读旧库时丢弃**的，不是这里。
 */
const parseSkipTasks = (raw: string): number[] => {
  const out: number[] = []
  for (const part of raw.split(/[\s,，、;；]+/)) {
    const t = part.trim()
    if (t === '') continue
    if (!/^-?\d+$/.test(t)) throw new Error('跳过任务只能填任务 id（数字），「' + t + '」不是')
    out.push(Number(t))
  }
  return out
}

// ── 冻结接口 ────────────────────────────────────────────────────────────

/** 四格的**原始文本**（草稿快照用；数字一律以字符串存，见上面"为什么"）。 */
export interface AccountFieldsSnapshot {
  backupWaitSecs: string
  refreshTokenDays: string
  skipTasks: string
  aiAvatarDailyLimit: string
}

/** `build()` 的产物：正好是 `AccountSettings` 里那四格。 */
export type AccountBehavior = Pick<
  AccountSettings,
  'backupWaitSecs' | 'refreshTokenDays' | 'skipTasks' | 'aiAvatarDailyLimit'
>

export interface AccountFields {
  /** 从 GET/PUT 回来的一份设置铺进表单。逐格容错（`== null` ⇒ 空串 / 空表）。 */
  load(s: AccountSettings): void
  /**
   * 攒成 PUT 需要的那四格；**本地能判的当场抛可读错误**（空、不是整数、跳过任务不是数字）。
   * ⚠ 只判"是不是整数 / 是不是数字"，**范围一律留给后端**（前端再实现一遍必然分叉）。
   */
  build(): AccountBehavior
  /** 「任务参数」那一组：备份等待秒数 / 刷新阈值天数 / 跳过任务 id（账号页那个**可见**块用它）。 */
  taskFields(): JSX.Element
  /**
   * 「账号级设置」那一组（备份等待 / 刷新阈值）—— **不含"跳过的任务 id"**。
   *
   * ⚠ 为什么单列一个方法：`skipTasks` 是**按任务**的东西（"这个账号跳过哪些任务"），
   * 它的家是「任务配置」里那个**按账号**的开关列表（2026-10-05 用户口径：
   * 「每个任务的配置分为公共配置和账号私有配置」）；而配置中心那一栏
   * 与账号页的设置弹窗都用下面这个**不含 skip** 的版本，免得同一格在两个地方各编一份
   * （同一页里两个编辑器写同一格 ⇒ 谁后保存谁把对方盖掉 ✗）。
   *
   * ⚠ 这段原来还写着"配置中心的「推送配置」那一栏" —— 那一栏**早就没了** ✗
   * （推送凭据 2026-10-08 收进了规则自己身上 ✓，见 `api.ts::PushRule.config` ✓），
   * 留着只会把人指到一个不存在的入口 ✗。
   */
  accountFields(): JSX.Element
  /** 「功能开关」那一组：AI 头像每日上限（账号页那块**被 `SHOW_ACCOUNT_SETTINGS_BLOCKS` 隐藏**，配置中心页照常显示）。 */
  featureFields(): JSX.Element
  /** 草稿快照 / 还原（调用方切账号时用）。 */
  snapshot(): AccountFieldsSnapshot
  restore(s: AccountFieldsSnapshot): void
}

/**
 * 造一个"四格"编辑器。
 *
 * ⚠ 必须在**组件体里**调（`const acct = createAccountFields()`）—— 内部的 ref 要靠
 * lite 在组件求值时登记；在模块顶层调只会在页面加载时建一份、所有账号共用。
 */
export const createAccountFields = (): AccountFields => {
  // 数字一律以**字符串**存：原样回显用户敲的东西，解析放到提交那一刻。
  const fBackupWait = ref('')
  const fRefreshDays = ref('')
  const fSkipTasks = ref('')
  const fAiAvatarLimit = ref('')

  // ── 渲染原语 ────────────────────────────────────────────────────────
  //
  // ⚠ 这一份是**本模块私有**的副本（不从别处 import）：`./push-editor` 里也各有一份
  // 同款 —— 那里不去复用账号页/本模块的渲染层，正是为了**别把两个页面的渲染层绑在
  // 一起**（那边改一个 class 不该牵动这里的布局）。值一律走 thunk，理由见文件头。

  /** 一行普通输入框。 */
  const textField = (
    label: string,
    get: () => string,
    set: (v: string) => void,
    hint: string,
    type: 'text' | 'number' = 'text',
  ) => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">{label}</legend>
      <input
        type={type}
        class="input w-full"
        autocomplete="off"
        spellcheck={false}
        value={get()}
        onInput={(e) => set((e.target as HTMLInputElement).value)}
      />
      <p class="text-xs leading-5 opacity-60">{hint}</p>
    </fieldset>
  )

  /**
   * 把 GET / PUT 回来的一份设置铺进表单。
   *
   * ⚠ 逐格容错：GET / PUT 回的应当是完整的 DTO，但真出现形状缺口时，
   * 一条 `Cannot read properties of undefined` 会把"哪一格不对"整个盖掉。
   * 缺数字 ⇒ 留空（提交时那条"不能留空"才是能读的错误），缺数组 ⇒ 空表。
   */
  const load = (s: AccountSettings): void => {
    fBackupWait.value = s.backupWaitSecs == null ? '' : String(s.backupWaitSecs)
    fRefreshDays.value = s.refreshTokenDays == null ? '' : String(s.refreshTokenDays)
    fSkipTasks.value = (s.skipTasks ?? []).join(', ')
    fAiAvatarLimit.value = s.aiAvatarDailyLimit == null ? '' : String(s.aiAvatarDailyLimit)
  }

  /** 攒成 PUT 需要的那四格（本地能判的当场抛，范围交给后端）。 */
  const build = (): AccountBehavior => ({
    backupWaitSecs: parseIntField('备份等待秒数', fBackupWait.value),
    refreshTokenDays: parseIntField('刷新阈值天数', fRefreshDays.value),
    skipTasks: parseSkipTasks(fSkipTasks.value),
    aiAvatarDailyLimit: parseIntField('AI 头像每日上限', fAiAvatarLimit.value),
  })

  /**
   * 「任务参数」那一组（备份等待 / 刷新阈值 / 跳过任务 id）。
   *
   * ⚠ 外层那个 `<div class="flex flex-col gap-2">` 与 `<h4>任务参数</h4>` 归调用方
   * （标题的层级是页面的排版决定），这里只出三行输入框。
   */
  const backupField = () =>
    textField(
      '备份任务等待时间（秒）',
      () => fBackupWait.value,
      (v) => (fBackupWait.value = v),
      '范围 0–3600（后端上限）：备份事件发出后等它落库的秒数，0 = 不等。',
      'number',
    )

  const refreshField = () =>
    textField(
      '刷新凭据的剩余天数阈值',
      () => fRefreshDays.value,
      (v) => (fRefreshDays.value = v),
      '范围 0–3650（后端上限）：凭据剩余天数低于它就刷新。',
      'number',
    )

  const taskFields = (): JSX.Element => (
    <>
      {backupField()}
      {refreshField()}
      {textField(
        '跳过的任务 id',
        () => fSkipTasks.value,
        (v) => (fSkipTasks.value = v),
        '填任务 id（数字），用逗号或空格分隔，最多 1000 条；留空 = 一个都不跳。⚠ 旧版本存的是任务名，后端读旧库时会丢弃它们。',
      )}
    </>
  )

  /** 账号级那两格（不含 skipTasks，见接口上那段 ⚠）。 */
  const accountFields = (): JSX.Element => (
    <>
      {backupField()}
      {refreshField()}
    </>
  )

  /**
   * 「功能开关」那一组（AI 头像每日上限）。
   *
   * ⚠ 账号页那块被 `SHOW_ACCOUNT_SETTINGS_BLOCKS` 隐藏着，但代码原样留着
   * （用户说的是"先取消显示"，不是"不要了"）；配置中心页照常显示同一份。
   */
  const featureFields = (): JSX.Element => (
    <>
      {textField(
        'AI 头像每日上限（轮）',
        () => fAiAvatarLimit.value,
        (v) => (fAiAvatarLimit.value = v),
        '范围 0–1000（后端上限）：每一轮都是一次上游调用。',
        'number',
      )}
    </>
  )

  /**
   * 草稿快照。
   *
   * 四格全是**字符串**，所以浅拷贝就够（没有内层对象会被后来的编辑动到）。
   */
  const snapshot = (): AccountFieldsSnapshot => ({
    backupWaitSecs: fBackupWait.value,
    refreshTokenDays: fRefreshDays.value,
    skipTasks: fSkipTasks.value,
    aiAvatarDailyLimit: fAiAvatarLimit.value,
  })

  /** 还原一份草稿（切账号回来时用）。 */
  const restore = (s: AccountFieldsSnapshot): void => {
    fBackupWait.value = s.backupWaitSecs
    fRefreshDays.value = s.refreshTokenDays
    fSkipTasks.value = s.skipTasks
    fAiAvatarLimit.value = s.aiAvatarDailyLimit
  }

  return { load, build, taskFields, accountFields, featureFields, snapshot, restore }
}
