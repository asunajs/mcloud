/**
 * 任务配置页（`#/tasks/<任务名>/config`）。
 *
 * # 为什么要单独一页
 *
 * 用户口径（2026-10-01）：**任务页保持简单，参数挪到单独的配置页**。
 * 原来的任务页把参数字段混在任务卡片里（而且实际只印了一行参数名、请求体恒为 `{}`，
 * 根本填不了），19 个任务里只要有两个要参数，那一页就没法一眼看完了。
 *
 * # 这里存的是什么
 *
 * **默认参数**：任务页点「运行」时带上它。定时规则与编排步骤各自带自己的 `body`，
 * 不受这里影响 —— 一次运行真正收到什么参数，只取决于它的发起方。
 *
 * # 为什么表单按 `specs` 生成
 *
 * 参数的类型、默认值、取值范围全由后端给（`TaskParamSpec`）。前端**不维护**任何
 * "参数名 → 控件/类型"的表：本仓栽过一次（签到族名单硬编码在前端），
 * 每多一份这类表就多一处迟早漂移的地方。
 */

import { onMounted, onUnmounted, ref } from 'lite'
import { setPageRefresh } from '../ui/page-refresh'
import { getTaskConfig, putTaskConfig, type TaskConfigView } from '../api'
// ⚠ 参数控件与**编排页**共用一份（`ui/param-field.tsx`）—— 两处各写一份迟早分叉。
import { ParamField } from '../ui/param-field'
import { pushToast } from '../ui/toast'
import { IconClose } from '../ui/icons'

export interface TaskConfigPageProps {
  /** 任务名（注册表里的 `name`）。 */
  task: string
  /** 回任务页。 */
  onBack: () => void
}

type LoadState = 'loading' | 'error' | 'ready'

export const TaskConfigPage = (props: TaskConfigPageProps) => {
  const state = ref<LoadState>('loading')
  /** 被本页盖住的那份刷新登记（任务页的），卸载时还回去。 */
  let prevRefresh: ReturnType<typeof setPageRefresh> = null
  const errorMessage = ref('')
  const view = ref<TaskConfigView | null>(null)
  const values = ref<Record<string, unknown>>({})
  const saving = ref(false)

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const v = await getTaskConfig(props.task)
      view.value = v
      values.value = { ...v.params }
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '读不到这个任务的配置'
      state.value = 'error'
    }
  }

  /**
   * 登记「刷新」给外壳 navbar（与各页同一条约定，见 `ui/page-refresh.ts`）。
   *
   * # ⚠⚠ 为什么要 `queueMicrotask` 把它推到挂载之后
   *
   * 这一页是**子视图**（任务页里切出来的）。lite 里子组件的 `onMounted` **先于**
   * 父页面（任务页）的 `onMounted` 跑 ⇒ 若在这里直接登记，紧接着父页面那份会把它
   * **盖掉** ✗ ⇒ 表现是「在配置页点刷新，刷的是任务列表、不是这份配置」
   * （实测发出的是 `/api/capabilities` + `/api/accounts`，而不是 `/api/tasks/<名>/config` ✗）。
   * 推到微任务里就落在外层之后 ⇒ 子视图这份生效 ✓。
   *
   * ⚠ 这个推迟之所以安全，是因为登记写的是**普通变量、不是信号**
   * （`ui/page-refresh.ts` 文件头写着为什么不能用信号：挂载期间写信号会重入外壳渲染
   * ⇒ 切页时旧页面不被替换而被追加 ✗）。**若哪天有人把它改回信号，这段必须一起改** ✗。
   *
   * 卸载时把外层那份**还回去**（`prev`）而不是置 `null` —— 置 null 会让回到任务列表后
   * 那个刷新图标点了没反应 ✗。`alive` 处理"还没轮到这个微任务就被卸载"的竞态：
   * 那种情况下不能登记，否则图标会指向一个已经没了的页面 ✗。
   */
  let alive = true
  onMounted(() => {
    queueMicrotask(() => {
      if (alive) prevRefresh = setPageRefresh({ run: () => void load() })
    })
    void load()
  })
  onUnmounted(() => {
    alive = false
    setPageRefresh(prevRefresh)
  })



  const save = async () => {
    saving.value = true
    try {
      const saved = await putTaskConfig(props.task, values.value)
      view.value = saved
      values.value = { ...saved.params }
      pushToast('success', '已保存 —— 任务页点「运行」时会带上这份参数')
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '保存失败')
    } finally {
      saving.value = false
    }
  }

  /** 恢复默认：只改表单，**不落盘** —— 想生效还得点保存（用户随时能反悔）。 */
  const resetDefaults = () => {
    const specs = view.value?.specs ?? []
    const next: Record<string, unknown> = {}
    for (const s of specs) next[s.name] = s.default
    values.value = next
    pushToast('info', '已填回默认值，点「保存」才会生效')
  }

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex min-w-0 items-center gap-2">
          <button
            type="button"
            class="btn btn-sm btn-ghost min-h-11 md:min-h-0"
            aria-label="回任务页"
            onClick={() => props.onBack()}
          >
            <IconClose class="size-4" />
          </button>
          <h1 class="min-w-0 truncate text-base font-medium">
            任务配置 · {view.value?.name ?? props.task}
          </h1>
        </div>
      </div>

      {state.value === 'loading' ? (
        <div class="flex justify-center py-16">
          <span class="loading loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-4">
            <p class="text-sm opacity-70">
              这里存的是这个任务的默认参数：任务页点「运行」时会带上它。
              定时任务与编排步骤各自带自己的参数，不受这一页影响。
            </p>

            {(view.value?.specs ?? []).length === 0 ? (
              <p class="text-sm opacity-70">这个任务没有可配置的参数。</p>
            ) : (
              <div class="grid min-w-0 gap-4 md:grid-cols-2">
                {(view.value?.specs ?? []).map((s) => (
                  <div key={s.name} class="min-w-0">
                    {/* ⚠ `get` 是 thunk：在槽里写 `value={values.value[s.name]}`
                        会让这个槽订阅值信号 ⇒ 每敲一个字整块重建、焦点全丢
                        （2026-10-03 review §2.2；正解见 `accounts-page.tsx:1192-1199`）。 */}
                    <ParamField
                      spec={s}
                      get={() => values.value[s.name]}
                      onInput={(v) => {
                        values.value = { ...values.value, [s.name]: v }
                      }}
                    />
                  </div>
                ))}
              </div>
            )}

            <div class="card-actions justify-end gap-2">
              <button
                type="button"
                class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                disabled={saving.value}
                onClick={() => resetDefaults()}
              >
                填回默认值
              </button>
              <button
                type="button"
                class="btn btn-sm btn-primary min-h-11 md:min-h-0"
                disabled={saving.value}
                onClick={() => void save()}
              >
                {saving.value ? '保存中…' : '保存'}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}
