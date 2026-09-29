/**
 * 登录 / 初始化页。
 *
 * # 为什么一个页面管两件事
 *
 * "建管理员"与"登录"用的是**同一个表单**（用户名 + 口令），
 * 差别只在文案与调用的接口。拆成两个页面会复制一份表单与校验，
 * 而两份必然会漂移（改了一边忘了另一边）。
 *
 * 后端也是这么分的：未初始化回 **503 `setup_required`**、
 * 已初始化未登录回 **401**。前端据此选标题与动作。
 *
 * # 这个页面**不进导航**
 *
 * 它是闸门，不是功能页。侧栏与 dock 都不该出现"登录" ——
 * 登录后它就没有意义了。所以 `nav.ts` 里没有它的条目。
 *
 * # 口令下限由后端给
 *
 * `minPasswordLen` 从 `GET /api/setup` 读，**不硬编** ——
 * 硬编一份必然与后端漂移，而漂移的表现是"前端说可以、后端拒绝"。
 * 后端没答上来时用 8 兜底（与 `MIN_PASSWORD_LEN` 一致），
 * 并让后端最终说了算。
 */

import { onMounted, ref } from 'lite'
import {
  createAdmin,
  getSetupStatus,
  loginSession,
  type AuthState,
} from '../api'
import { IconAlert, IconBrand } from '../ui/icons'

export interface AuthPageProps {
  /** `setup` = 建管理员；`login` = 登录。 */
  mode: Extract<AuthState, 'setup' | 'login'>
}

export const AuthPage = (props: AuthPageProps) => {
  const username = ref('')
  const password = ref('')
  const confirm = ref('')
  const busy = ref(false)
  const error = ref('')

  /**
   * 口令下限。初始值与后端常量一致，随后由 `GET /api/setup` 覆盖。
   *
   * ⚠ 初始值只是"还没问到后端时的兜底"，**不是**权威值。
   */
  const minLen = ref(8)

  const isSetup = props.mode === 'setup'

  onMounted(async () => {
    try {
      const s = await getSetupStatus()
      minLen.value = s.minPasswordLen
    } catch {
      // 问不到就用兜底值。这里**不报错** —— 用户此刻要做的是登录，
      // 而"读不到口令下限"不该挡住他。
    }
  })

  /**
   * 输入框里按回车就提交。
   *
   * ⚠ 事件名是 `onKeydown`（**小写 d**），不是 React 的 `onKeyDown` ——
   * 与 `readonly`（不是 `readOnly`）同一类命名差异。写错时 TS 会报
   * "Property 'onKeyDown' does not exist ... Did you mean 'onKeydown'?"。
   */
  const onEnter = (e: { key: string }) => {
    if (e.key === 'Enter') void submit()
  }

  /** 前端先做一遍校验，把明显的问题挡在请求之前。 */
  const localError = (): string => {
    if (!username.value.trim()) return '请填写用户名'
    if (!password.value) return '请填写口令'
    if (isSetup) {
      if (password.value.length < minLen.value) {
        return `口令至少 ${minLen.value} 位`
      }
      if (password.value !== confirm.value) return '两次输入的口令不一致'
    }
    return ''
  }

  /**
   * 提交。
   *
   * ⚠ 用 `onClick` 而不是 `<form onSubmit>`：本项目其它页面全是这个写法，
   * 为一个"回车提交"引入一套与全站不同的写法不值得 ——
   * 而 `<form>` 里没写 `onSubmit` 时回车会**刷新整页**，那比没有更糟。
   *
   * 回车提交由两个输入框各自的 `onKeyDown` 接住（见下）。
   *
   * （历史备注：这里以前还写着"Vapor 的提交事件类型 `FormEvent<HTMLFormElement>`
   * 与原生 `Event` 不兼容，要多一层断言"。lite 的 JSX 声明里事件属性走
   * `on${string}` 那条索引签名 ⇒ 参数就是 `any`（见 `lite/jsx.d.ts`），
   * 要精确类型就自己标注 `onSubmit={(e: SubmitEvent) => …}` —— 那个包袱
   * 2026-09-30 随 Vue 兼容层一起没了。）
   */
  const submit = async () => {
    error.value = ''

    const bad = localError()
    if (bad) {
      error.value = bad
      return
    }

    busy.value = true
    try {
      if (isSetup) {
        await createAdmin(username.value.trim(), password.value)
      } else {
        await loginSession(username.value.trim(), password.value)
      }
      // 成功后不在这里跳转：`authState` 变了，外壳会自己把页面换掉。
      // 手动 `location.hash = ...` 反而会与那一步打架。
    } catch (err) {
      // 后端原文直接透出（规范：不翻译成"操作失败"）——
      // 它已经说清了是口令短、还是不匹配。
      error.value = err instanceof Error ? err.message : '登录失败'
      // 口令错时清掉口令框，但不碰用户名（大概率只是打错了口令）
      password.value = ''
      confirm.value = ''
    } finally {
      busy.value = false
    }
  }

  return (
    <div class="flex min-h-screen items-center justify-center bg-base-200 p-4">
      <div class="w-full max-w-sm">
        <div class="mb-6 flex flex-col items-center gap-2">
          <span class="text-primary">
            <IconBrand class="size-10" />
          </span>
          <h1 class="text-lg font-medium">mcloud-gost</h1>
          <p class="text-center text-sm opacity-70">
            {isSetup ? '首次使用，请创建管理员账号' : '请登录以继续'}
          </p>
        </div>

        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-3">
            {isSetup ? (
              <p class="text-xs leading-5 opacity-70">
                这个账号只用来进入本管理界面，与云盘账号无关。
                云盘账号在「账号」页添加。
              </p>
            ) : null}

            <fieldset class="fieldset">
              <legend class="fieldset-legend">用户名</legend>
              <input
                class="input w-full"
                autocomplete="username"
                value={username.value}
                onInput={(e) => {
                  username.value = (e.target as HTMLInputElement).value
                }}
                onKeydown={onEnter}
              />
            </fieldset>

            <fieldset class="fieldset">
              <legend class="fieldset-legend">口令</legend>
              <input
                class="input w-full"
                type="password"
                autocomplete={isSetup ? 'new-password' : 'current-password'}
                value={password.value}
                onInput={(e) => {
                  password.value = (e.target as HTMLInputElement).value
                }}
                onKeydown={onEnter}
              />
              {isSetup ? (
                <p class="label">至少 {minLen.value} 位</p>
              ) : null}
            </fieldset>

            {isSetup ? (
              <fieldset class="fieldset">
                <legend class="fieldset-legend">确认口令</legend>
                <input
                  class="input w-full"
                  type="password"
                  autocomplete="new-password"
                  value={confirm.value}
                  onInput={(e) => {
                    confirm.value = (e.target as HTMLInputElement).value
                  }}
                  onKeydown={onEnter}
                />
              </fieldset>
            ) : null}

            {error.value ? (
              <div class="alert alert-soft alert-error text-sm" role="alert">
                <IconAlert class="size-4 shrink-0" />
                <span class="break-words">{error.value}</span>
              </div>
            ) : null}

            <button
              type="button"
              class="btn btn-primary min-h-11 w-full md:min-h-0"
              disabled={busy.value}
              onClick={() => void submit()}
            >
              {busy.value ? (
                <span class="loading loading-sm" />
              ) : null}
              {isSetup ? '创建并进入' : '登录'}
            </button>
          </div>
        </div>

        {isSetup ? (
          <p class="mt-4 text-center text-xs leading-5 opacity-60">
            口令用 argon2id 加盐存储，本机数据库里没有明文。
          </p>
        ) : null}
      </div>
    </div>
  )
}
