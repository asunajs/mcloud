/**
 * 登录 / 初始化页。
 *
 * 形态（设计指南 §7.2）：**单列表单，卡片 `w-full max-w-sm`，页面垂直居中。**
 * 移动端与桌面端是同一个布局，刻意不做左右分栏 —— 登录页只做一件事，
 * 分栏会让人先找"哪边是表单"。
 *
 * 设计要点（逐条都能追溯到设计指南）：
 *  - 主题切换按钮**常驻右上角**，且**登录页也有**（§3.2）——
 *    否则用户得先登录才能把刺眼的浅色换成深色，本末倒置。
 *  - 主行动只有一个实心按钮（§2.3①），其余一律 `-soft` / `-ghost`。
 *  - 错误用 `alert-soft alert-error` 而非实心（§3.4），不压过表单本身。
 *  - 字段级校验失败**内联在字段下方**，不整个塞进顶部 alert（§6.2）——
 *    顶部 alert 留给"提交整体失败"，两者职责不同。
 */

import { computed, ref } from 'vue'
import { ApiError, runSetup, signIn, type User } from '../api'
import { IconAlert, IconCloud, IconEye, IconEyeOff, IconMoon, IconSun } from '../ui/icons'
import type { Theme } from '../ui/theme'

export interface AuthPageProps {
  /** `setup` = 建首个管理员；`login` = 常规登录。 */
  mode: 'setup' | 'login'
  minPasswordLen: number
  theme: Theme
  onToggleTheme: () => void
  onDone: (user: User, notice: string) => void
}

export const AuthPage = (props: AuthPageProps) => {
  const name = ref('')
  const password = ref('')
  const again = ref('')
  const busy = ref(false)
  /** 提交整体失败（网络/后端拒绝）。 */
  const submitError = ref('')
  /** 字段级错误，键为字段名。 */
  const fieldError = ref<{ name?: string; password?: string; again?: string }>({})
  const showPassword = ref(false)

  const isSetup = props.mode === 'setup'
  const minLenHint = computed(() => `至少 ${props.minPasswordLen} 位`)

  const themeToggle = () => (
    <button
      type="button"
      class="btn btn-square btn-ghost btn-sm"
      aria-label={props.theme === 'dark' ? '切换到浅色主题' : '切换到深色主题'}
      title={props.theme === 'dark' ? '切换到浅色主题' : '切换到深色主题'}
      onClick={props.onToggleTheme}
    >
      {props.theme === 'dark' ? <IconSun class="size-4" /> : <IconMoon class="size-4" />}
    </button>
  )

  /**
   * 提交前的前端校验。
   *
   * 只拦"后端一定会拒"或"明显笔误"的情况，**不重复后端的规则** ——
   * 真正的判定权在后端，这里只是少跑一趟网络。
   */
  const validate = (): boolean => {
    const next: typeof fieldError.value = {}
    if (!name.value.trim()) {
      next.name = '请输入用户名'
    }
    if (password.value.length < props.minPasswordLen) {
      next.password = `口令${minLenHint.value}`
    }
    if (isSetup && password.value !== again.value) {
      next.again = '两次输入的口令不一致'
    }
    fieldError.value = next
    return Object.keys(next).length === 0
  }

  const submit = async () => {
    submitError.value = ''
    if (busy.value || !validate()) {
      return
    }
    busy.value = true
    try {
      if (isSetup) {
        const result = await runSetup(name.value, password.value)
        const { accounts, schedules, pipelines } = result.claimed
        const claimed = accounts + schedules + pipelines
        props.onDone(
          result.user,
          claimed > 0
            ? `已认领旧数据里原本没有归属的数据：账号 ${accounts}、定时任务 ${schedules}、组合 ${pipelines}`
            : '',
        )
      } else {
        const session = await signIn(name.value, password.value)
        props.onDone(session.user, '')
      }
    } catch (err) {
      submitError.value = err instanceof ApiError ? err.message : '请求失败，请检查服务是否在运行'
    } finally {
      busy.value = false
    }
  }

  /**
   * 回车提交 —— 登录表单里这是最省事的操作，不给反而显得别扭。
   *
   * 这里**只取 `e.key`**，所以刻意不给参数标类型：标了反而会撞上
   * "Vue 的键盘事件类型不是全局那个 `KeyboardEvent`"的坑（会报 TS2322 / TS2315）。
   * 让 TSX 从属性位置自己推断是最稳的写法。
   */
  const submitOnEnter = () => {
    void submit()
  }

  /** 字段级错误的行内提示。放在输入框正下方，紧贴它说明的那个字段。 */
  const fieldNote = (message?: string) =>
    message ? (
      <p class="mt-0.5 flex items-center gap-1 text-xs text-error">
        <IconAlert class="size-3 shrink-0" />
        {message}
      </p>
    ) : null

  /**
   * 输入框的类名随校验状态变。
   *
   * 这里用**查表**而不是 `input-${state}` —— Tailwind 只认字面量（§9.1①）。
   */
  const inputClass = (invalid: boolean, extra = '') =>
    invalid
      ? `input input-bordered input-error w-full ${extra}`
      : `input input-bordered w-full ${extra}`

  return (
    <div class="relative flex min-h-screen items-center justify-center bg-base-200 p-4">
      <div class="absolute right-3 top-3">{themeToggle()}</div>

      <div class="w-full max-w-sm">
        {/* 品牌区：登录页是唯一没有侧栏的地方，需要自己说明"这是什么系统" */}
        <div class="mb-6 flex flex-col items-center gap-2 text-center">
          <span class="flex size-12 items-center justify-center rounded-box bg-primary text-primary-content">
            <IconCloud class="size-7" />
          </span>
          <h1 class="text-lg font-medium">mcloud</h1>
          <p class="text-sm opacity-70">移动云盘签到与云朵管理</p>
        </div>

        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-4">
            <div class="flex flex-col gap-1">
              <h2 class="text-base font-medium">{isSetup ? '初始化首个管理员' : '登录'}</h2>
              <p class="text-sm opacity-70">
                {isSetup ? '这个实例还没有管理员，先建一个。' : '用本系统的账号登录。'}
              </p>
            </div>

            {submitError.value ? (
              <div class="alert alert-soft alert-error text-sm" role="alert">
                <IconAlert class="size-4 shrink-0" />
                <span>{submitError.value}</span>
              </div>
            ) : null}

            <label class="fieldset">
              <span class="fieldset-legend">用户名</span>
              <input
                class={inputClass(Boolean(fieldError.value.name))}
                autocomplete="username"
                autofocus
                value={name.value}
                onInput={(e) => (name.value = (e.target as HTMLInputElement).value)}
                onKeydown={submitOnEnter}
              />
              {fieldNote(fieldError.value.name)}
            </label>

            <label class="fieldset">
              <span class="fieldset-legend">
                口令<span class="ml-1 opacity-60">（{minLenHint.value}）</span>
              </span>
              <div class="relative">
                <input
                  class={inputClass(Boolean(fieldError.value.password), 'pr-10')}
                  type={showPassword.value ? 'text' : 'password'}
                  autocomplete={isSetup ? 'new-password' : 'current-password'}
                  value={password.value}
                  onInput={(e) => (password.value = (e.target as HTMLInputElement).value)}
                  onKeydown={submitOnEnter}
                />
                {/* 口令显示开关：手机上没有鼠标，打错了"看不见"比"被别人看见"更常见 */}
                <button
                  type="button"
                  class="btn btn-square btn-ghost btn-sm absolute right-1 top-1/2 -translate-y-1/2"
                  aria-label={showPassword.value ? '隐藏口令' : '显示口令'}
                  title={showPassword.value ? '隐藏口令' : '显示口令'}
                  onClick={() => (showPassword.value = !showPassword.value)}
                >
                  {showPassword.value ? <IconEyeOff class="size-4" /> : <IconEye class="size-4" />}
                </button>
              </div>
              {fieldNote(fieldError.value.password)}
            </label>

            {isSetup ? (
              <label class="fieldset">
                <span class="fieldset-legend">再输一次</span>
                <input
                  class={inputClass(Boolean(fieldError.value.again))}
                  type={showPassword.value ? 'text' : 'password'}
                  autocomplete="new-password"
                  value={again.value}
                  onInput={(e) => (again.value = (e.target as HTMLInputElement).value)}
                  onKeydown={submitOnEnter}
                />
                {fieldNote(fieldError.value.again)}
              </label>
            ) : null}

            <button
              type="button"
              class="btn btn-primary w-full"
              disabled={busy.value}
              onClick={() => void submit()}
            >
              {busy.value ? (
                <>
                  <span class="loading loading-spinner loading-sm" />
                  {isSetup ? '创建中…' : '登录中…'}
                </>
              ) : isSetup ? (
                '创建管理员并进入'
              ) : (
                '登录'
              )}
            </button>

            {isSetup ? (
              <p class="text-xs opacity-60">
                这个口令只用于登录本系统，与移动云盘账号无关；它没有找回入口，忘了要直接改库。
              </p>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  )
}
