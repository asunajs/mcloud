/**
 * 定时任务：列表 + 新建/编辑 + 启停 + 删除。
 *
 * # 这个页面最要紧的一件事：**下次执行时间由服务端算**
 *
 * 5 段还是 6 段、周日的 0 与 7、日与周的 OR 语义 —— 这些细节各客户端
 * 各实现一遍必然分叉。所以 `nextFireAt` 直接取后端给的值。
 * 服务端算不出（表达式坏了）时给 `null`，这里显示「—」，**绝不自己猜一个**。
 *
 * # 未启用就不显示"下次执行时间"
 *
 * 后端对未启用的定时返回 `nextFireAt: null`，这里也不去本地推算 ——
 * 给一个"下次执行时间"就是在骗人。
 *
 * # 校验：前端先挡一道，后端是权威
 *
 * 前端只做"段数对不对"这一条（能立刻反馈的），其余交给后端 ——
 * 后端的错误文本**原样透出**，不翻译、不改写。
 */

import { onMounted, onUnmounted, ref } from 'lite'
import { accountName } from '../account-names'
import {
  deleteSchedule,
  listCapabilities,
  listSchedules,
  saveSchedule,
  type Schedule,
  type ScheduleInput,
  type TaskMeta,
} from '../api'
import { ConfirmDialog, Dialog, closeDialog, openDialog } from '../ui/dialog'
import { EmptyState } from '../ui/empty-state'
import { showError } from '../ui/error-dialog'
import { IconAlert } from '../ui/icons'
import { fmtEpochMs } from '../ui/time'
import { pushToast } from '../ui/toast'

type LoadState = 'loading' | 'error' | 'ready'

/** 新建时的默认时区。与后端默认值一致。 */
const DEFAULT_TZ = 'Asia/Shanghai'

/**
 * 常用表达式。点击即填入 —— 比让用户去背 cron 语法友好得多。
 *
 * 值用**字面量**写死，不做拼接：Tailwind 不参与这里，但保持
 * "不在运行期拼配置"的习惯，读起来也更直接。
 */
const PRESETS: readonly { expr: string; label: string }[] = [
  { expr: '0 8 * * *', label: '每天 08:00' },
  { expr: '0 0 8 * * *', label: '每天 08:00（带秒，6 段）' },
  { expr: '30 7 * * 1-5', label: '工作日 07:30' },
  { expr: '0 0 1 * *', label: '每月 1 日 00:00' },
]

/** 时区选项。只列后端**确实支持**的，不做自由输入 —— 写错的时区后端会报错。 */
const TIMEZONES: readonly string[] = ['Asia/Shanghai', 'UTC', 'Asia/Tokyo']

/**
 * 校验 cron 的段数。
 *
 * 只查这一条：它是用户最容易写错的，而且本地就能判断。
 * 语义是否合法（`0 0 0 30 2 *` 永不触发、`99 99 * * *` 不合法）交给后端 ——
 * 前端再实现一遍解析器只会与后端分叉。
 */
const exprFieldError = (expr: string): string | null => {
  const n = expr.trim().split(/\s+/).filter(Boolean).length
  if (n === 5 || n === 6) return null
  return `cron 表达式需要 5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周），当前 ${n} 段`
}

/** 上次结果的徽章样式。`null`（从未运行）单独一档，不跟任何结果混。 */
const statusBadge = (s: Schedule['lastStatus']): { cls: string; text: string } => {
  switch (s) {
    case 'ok':
      return { cls: 'badge badge-success badge-sm', text: '成功' }
    case 'error':
      return { cls: 'badge badge-error badge-sm', text: '失败' }
    case 'skipped':
      return { cls: 'badge badge-warning badge-sm', text: '已跳过' }
    default:
      return { cls: 'badge badge-ghost badge-sm', text: '从未运行' }
  }
}

export const SchedulesPage = () => {
  const state = ref<LoadState>('loading')
  /** 加载失败（整页替换）。**留在页内** —— 它要配「重试」。 */
  const errorMessage = ref('')
  /**
   * 表单失败（内联在表单旁）。
   *
   * 这一条**保持内联**：`save()` 发生在「新建/编辑定时任务」那个 `<dialog>` 里，
   * 弹出错误弹窗就是 `<dialog>` 套 `<dialog>` —— 无效结构，ESC 与遮罩会打架；
   * 而且「请填写名称」这类错误必须贴着要改的字段，弹窗把它推远了。
   * 页面里**独立动作**（启停开关）的失败走 `showError`。
   */
  const submitError = ref('')

  const items = ref<Schedule[]>([])
  const tasks = ref<TaskMeta[]>([])
  /** 计时器：每分钟刷一次列表，让"下次执行"与"上次结果"跟上。 */
  let ticker: number | null = null

  // ── 表单状态 ──────────────────────────────────────────────────────────
  const dialogId = 'schedule-dialog'
  const editingId = ref<string | null>(null)
  const fName = ref('')
  const fTask = ref('')
  const fAccounts = ref('')
  const fExpr = ref('')
  const fTimezone = ref(DEFAULT_TZ)
  const fEnabled = ref(true)
  const saving = ref(false)

  // ── 删除确认 ──────────────────────────────────────────────────────────
  const confirmId = 'schedule-delete'
  const pendingDelete = ref<Schedule | null>(null)
  const deleting = ref(false)

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      // 任务清单用于下拉框：**不硬编码任务名** —— 任务由后端注册表提供
      const [schedules, caps] = await Promise.all([listSchedules(), listCapabilities()])
      items.value = schedules
      tasks.value = caps.tasks.filter((t) => !t.hidden)
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /** 只刷新列表，不动 loading 态 —— 用于计时器与动作后的静默刷新。 */
  const refresh = async () => {
    try {
      items.value = await listSchedules()
    } catch {
      // 静默失败：页面已经有一份数据，刷不到不该把它换成错误页
    }
  }

  // ── 表单开合 ──────────────────────────────────────────────────────────

  const openCreate = () => {
    editingId.value = null
    fName.value = ''
    // 默认选第一个任务，省掉一次点击；没有任务时留空由校验拦
    fTask.value = tasks.value[0]?.name ?? ''
    fAccounts.value = ''
    fExpr.value = '0 8 * * *'
    fTimezone.value = DEFAULT_TZ
    fEnabled.value = true
    submitError.value = ''
    openDialog(dialogId)
  }

  const openEdit = (s: Schedule) => {
    editingId.value = s.id
    fName.value = s.name
    fTask.value = s.task
    fAccounts.value = s.accounts.join(', ')
    fExpr.value = s.expr
    fTimezone.value = s.timezone
    fEnabled.value = s.enabled
    submitError.value = ''
    openDialog(dialogId)
  }

  const save = async () => {
    submitError.value = ''
    if (!fName.value.trim()) {
      submitError.value = '请填写名称'
      return
    }
    if (!fTask.value) {
      submitError.value = '请选择要执行的任务'
      return
    }
    const exprErr = exprFieldError(fExpr.value)
    if (exprErr) {
      submitError.value = exprErr
      return
    }

    const input: ScheduleInput = {
      name: fName.value.trim(),
      task: fTask.value,
      // 逗号分隔，去掉空项：留空 = 所有已启用账号（后端语义）
      accounts: fAccounts.value
        .split(',')
        .map((x) => x.trim())
        .filter(Boolean),
      expr: fExpr.value.trim(),
      timezone: fTimezone.value,
      enabled: fEnabled.value,
    }

    saving.value = true
    try {
      // id 由前端生成（编辑时沿用原 id）：服务端不发明 id ——
      // 那样"改一个任务"与"建一个任务"就分不出来了
      const id = editingId.value ?? newId()
      await saveSchedule(id, input)
      closeDialog(dialogId)
      pushToast('success', editingId.value ? '已保存修改' : '已创建定时任务')
      await refresh()
    } catch (e) {
      // 后端错误原样透出
      submitError.value = e instanceof Error ? e.message : '保存失败'
    } finally {
      saving.value = false
    }
  }

  const toggleEnabled = async (s: Schedule) => {
    try {
      await saveSchedule(s.id, {
        name: s.name,
        task: s.task,
        accounts: s.accounts,
        expr: s.expr,
        timezone: s.timezone,
        enabled: !s.enabled,
      })
      pushToast('success', s.enabled ? '已停用' : '已启用')
      await refresh()
    } catch (e) {
      // 失败必须回滚界面：乐观改了却不回滚，用户会以为改成功了。
      // 报错走弹窗 —— 启停是列表上的独立动作，没有表单在场，
      // 往 `submitError` 里写等于把错误丢进一个此刻不存在的地方（旧代码正是如此，
      // 而且还是重复报了两遍：内联一份 + 角落 toast 一份）。
      const reason = e instanceof Error ? e.message : '操作失败'
      showError(s.enabled ? '无法停用定时任务' : '无法启用定时任务', reason)
      await refresh()
    }
  }

  const askDelete = (s: Schedule) => {
    pendingDelete.value = s
    openDialog(confirmId)
  }

  const doDelete = async () => {
    const target = pendingDelete.value
    if (!target) return
    deleting.value = true
    try {
      await deleteSchedule(target.id)
      closeDialog(confirmId)
      pendingDelete.value = null
      pushToast('success', '已删除')
      await refresh()
    } catch (e) {
      // 例如被编排引用时的拒绝 —— 原样透出。
      // 走弹窗：删除是独立动作，而这里的原因（"被 X 引用"）是用户**必须读到**
      // 才知道下一步该改哪，丢进 3 秒后仍在角落的 toast 里等于没说。
      closeDialog(confirmId)
      pendingDelete.value = null
      showError('删除定时任务失败', e instanceof Error ? e.message : '删除失败')
    } finally {
      deleting.value = false
    }
  }

  onMounted(() => {
    void load()
    // 一分钟一跳："下次执行时间"与"上次结果"要能自己跟上，
    // 否则用户盯着一个过期的时刻看
    ticker = window.setInterval(() => void refresh(), 60_000)
  })

  // 必须清掉：页面切走后计时器还在跑，就是一条看不见的持续请求
  onUnmounted(() => {
    if (ticker !== null) {
      clearInterval(ticker)
      ticker = null
    }
  })

  // ── 渲染 ──────────────────────────────────────────────────────────────

  const header = () => (
    <div class="flex flex-wrap items-center justify-between gap-2">
      <h1 class="text-base font-medium">定时任务</h1>
      <div class="flex items-center gap-2">
        <button
          type="button"
          class="btn btn-sm btn-soft min-h-11 md:min-h-0"
          disabled={state.value === 'loading'}
          onClick={() => void load()}
        >
          刷新
        </button>
        <button
          type="button"
          class="btn btn-sm btn-primary min-h-11 md:min-h-0"
          disabled={state.value !== 'ready'}
          onClick={openCreate}
        >
          新建定时
        </button>
      </div>
    </div>
  )

  /** 一行/一卡共用的字段。表格与卡片两种形态都从这里取，避免两处不一致。 */
  const summaryOf = (s: Schedule) => (
    <>
      <div class="flex flex-wrap items-center gap-2">
        <span class="font-medium">{s.name}</span>
        {s.enabled ? (
          <span class="badge badge-sm">已启用</span>
        ) : (
          <span class="badge badge-ghost badge-sm">已停用</span>
        )}
        {(() => {
          const b = statusBadge(s.lastStatus)
          return <span class={b.cls}>{b.text}</span>
        })()}
      </div>
      <div class="text-xs opacity-70">
        任务 <code class="break-all">{s.task}</code> · 表达式 <code>{s.expr}</code> · {s.timezone}
      </div>
      <div class="text-xs opacity-70">
        账号：{s.accounts.length === 0 ? '全部已启用账号' : s.accounts.map(accountName).join('、')}
      </div>
      <div class="text-xs">
        <span class="opacity-60">下次执行：</span>
        {s.enabled ? fmtEpochMs(s.nextFireAt) : '已停用，不会执行'}
      </div>
      <div class="text-xs opacity-60">
        上次执行：{fmtEpochMs(s.lastRunMs)} · 累计触发 {s.runCount} 次
      </div>
      {s.lastError ? (
        // 后端已脱敏；原样显示，不改写、不截断到看不见
        <div class="alert alert-soft alert-error text-xs" role="alert">
          <IconAlert class="size-4 shrink-0" />
          <span class="break-words">{s.lastError}</span>
        </div>
      ) : null}
    </>
  )

  const actionsOf = (s: Schedule) => (
    <div class="flex flex-wrap gap-2">
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        onClick={() => openEdit(s)}
      >
        编辑
      </button>
      <button
        type="button"
        class="btn btn-sm btn-soft min-h-11 md:min-h-0"
        onClick={() => void toggleEnabled(s)}
      >
        {s.enabled ? '停用' : '启用'}
      </button>
      <button
        type="button"
        class="btn btn-sm btn-error min-h-11 md:min-h-0"
        onClick={() => askDelete(s)}
      >
        删除
      </button>
    </div>
  )

  const list = () =>
    items.value.length === 0 ? (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body">
          <EmptyState
            title="还没有定时任务。"
            hint="点右上角「新建定时」创建一条，或先挑一个常用表达式：每天 08:00 自动签到。"
            action={
              tasks.value.length > 0 ? (
                <button type="button" class="btn btn-primary" onClick={openCreate}>
                  新建定时
                </button>
              ) : undefined
            }
          />
        </div>
      </div>
    ) : (
      <>
        {/* 窄屏：卡片流。表格在右边界会给每个单元格分配空间，长错误文本会把它撑破 */}
        <div class="grid gap-3 md:hidden">
          {items.value.map((s) => (
            <div key={s.id} class="card bg-base-100 shadow-sm">
              <div class="card-body gap-2">
                {summaryOf(s)}
                {actionsOf(s)}
              </div>
            </div>
          ))}
        </div>

        {/* 宽屏：表格 */}
        <div class="hidden card bg-base-100 shadow-sm md:block">
          <div class="card-body">
            <div class="overflow-x-auto">
              <table class="table">
                <thead>
                  <tr>
                    <th>名称</th>
                    <th>任务</th>
                    <th>下一次</th>
                    <th>上次结果</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {items.value.map((s) => (
                    <tr key={s.id}>
                      <td>
                        <div class="font-medium">{s.name}</div>
                        <div class="text-xs opacity-60">
                          <code>{s.expr}</code> · {s.timezone}
                        </div>
                        {s.lastError ? (
                          <div class="mt-1 text-xs text-error break-words">{s.lastError}</div>
                        ) : null}
                      </td>
                      <td class="text-xs">
                        <code>{s.task}</code>
                        <div class="opacity-60">
                          {s.accounts.length === 0 ? '全部账号' : `${s.accounts.length} 个账号`}
                        </div>
                      </td>
                      <td class="text-xs">
                        {s.enabled ? fmtEpochMs(s.nextFireAt) : '—'}
                        {!s.enabled ? <div class="opacity-60">已停用</div> : null}
                      </td>
                      <td>
                        {(() => {
                          const b = statusBadge(s.lastStatus)
                          return (
                            <div class="flex flex-col gap-1">
                              <span class={b.cls}>{b.text}</span>
                              <span class="text-xs opacity-60">{fmtEpochMs(s.lastRunMs)}</span>
                            </div>
                          )
                        })()}
                      </td>
                      <td>{actionsOf(s)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </>
    )

  const form = () => (
    <>
      <fieldset class="fieldset">
        <legend class="fieldset-legend">名称</legend>
        <input
          class="input w-full"
          placeholder="例如 每天签到"
          value={fName.value}
          onInput={(e) => {
            fName.value = (e.target as HTMLInputElement).value
          }}
        />
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">执行的任务</legend>
        <select
          class="select w-full"
          value={fTask.value}
          onChange={(e) => {
            fTask.value = (e.target as HTMLSelectElement).value
          }}
        >
          {tasks.value.map((t) => (
            <option key={t.name} value={t.name}>
              {t.title}（{t.name}）
            </option>
          ))}
        </select>
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">账号（账号 id，逗号分隔；留空 = 全部已启用账号）</legend>
        <input
          class="input w-full"
          placeholder="留空表示对所有已启用账号执行"
          value={fAccounts.value}
          onInput={(e) => {
            fAccounts.value = (e.target as HTMLInputElement).value
          }}
        />
        {/*
          ⚠ 这里**必须**写"账号 id"，不能写"手机号"（2026-09-30 修正）。
          后端拿这串**原样**当 `AccountId` 比对，而 AccountId 是手机号的 SHA-256 前 16 位
          （`0f6050105ab111f4`）。原文案写着"手机号"，于是照着填的人得到的是
          **匹配 0 个账号 ⇒ 定时任务静默不跑**（不报错、不提示，最难查的一类）。
          列表里显示的名字也会误导（那是 `accountName()` 渲染的昵称/掩码，不是可填的值），
          所以两处一起改。
        */}
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">cron 表达式</legend>
        <input
          class="input w-full font-mono"
          placeholder="0 8 * * *"
          value={fExpr.value}
          onInput={(e) => {
            fExpr.value = (e.target as HTMLInputElement).value
          }}
        />
        <p class="text-xs opacity-60">
          5 段（分 时 日 月 周）或 6 段（秒 分 时 日 月 周）。时区由下一项决定。
        </p>
        <div class="mt-1 flex flex-wrap gap-1">
          {PRESETS.map((p) => (
            <button
              key={p.expr}
              type="button"
              class="btn btn-xs btn-soft min-h-9"
              onClick={() => {
                fExpr.value = p.expr
              }}
            >
              {p.label}
            </button>
          ))}
        </div>
      </fieldset>

      <fieldset class="fieldset">
        <legend class="fieldset-legend">时区</legend>
        <select
          class="select w-full"
          value={fTimezone.value}
          onChange={(e) => {
            fTimezone.value = (e.target as HTMLSelectElement).value
          }}
        >
          {TIMEZONES.map((tz) => (
            <option key={tz} value={tz}>
              {tz}
            </option>
          ))}
        </select>
      </fieldset>

      <label class="label cursor-pointer justify-start gap-3">
        <input
          type="checkbox"
          class="checkbox"
          checked={fEnabled.value}
          // 用 onChange 而不是 onInput：复选框没有"输入中"的中间态
          onChange={(e) => {
            fEnabled.value = (e.target as HTMLInputElement).checked
          }}
        />
        <span class="text-sm">启用（停用后不会自动执行）</span>
      </label>

      {submitError.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{submitError.value}</span>
        </div>
      ) : null}
    </>
  )

  return (
    <div class="flex flex-col gap-4">
      {header()}

      {state.value === 'loading' ? (
        <div class="flex justify-center py-10">
          <span class="loading" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body">
            <EmptyState
              title="加载失败。"
              hint={errorMessage.value}
              action={
                <button type="button" class="btn btn-soft" onClick={() => void load()}>
                  重试
                </button>
              }
            />
          </div>
        </div>
      ) : null}

      {state.value === 'ready' ? list() : null}

      {/* 子节点走 Dialog 内部的默认插槽（lite 里子节点不进 props：`props.children` 恒为 undefined） */}
      <Dialog
        id={dialogId}
        title={editingId.value ? '编辑定时任务' : '新建定时任务'}
        size="lg"
      >
        {form()}
        <div class="modal-action">
          <button type="button" class="btn btn-soft" onClick={() => closeDialog(dialogId)}>
            取消
          </button>
          <button
            type="button"
            class="btn btn-primary"
            disabled={saving.value}
            onClick={() => void save()}
          >
            {saving.value ? '保存中…' : '保存'}
          </button>
        </div>
      </Dialog>

      <ConfirmDialog
        id={confirmId}
        title="删除定时任务"
        impact={
          <span>
            将删除「{pendingDelete.value?.name ?? ''}」。
            {pendingDelete.value?.enabled ? '它当前是启用状态，删除后不会再自动执行。' : ''}
            已产生的运行历史不受影响。
          </span>
        }
        confirmLabel="删除"
        busy={deleting.value}
        onConfirm={() => void doDelete()}
      />
    </div>
  )
}

/** 生成一个本地 id。只含后端允许的字符集。 */
const newId = (): string =>
  `sch-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
