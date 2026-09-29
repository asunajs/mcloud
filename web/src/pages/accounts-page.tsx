/**
 * 账号页：云盘账号的登录与列表。
 *
 * # 与"登录闸"是两件事
 *
 * 这个页面管的是**要跑任务的云盘账号**（`accounts.json`），
 * 不是本实例的管理员（那是登录闸，见 `api.ts` 里 `/api/session` 那组）。
 *
 * # 平台选择放在最上面，且**必须回显**
 *
 * mac 与 win 在请求头、UA 与**落盘数据**上都不一样（`pc_platform` 会存进
 * `accounts.json`）。所以选错平台不是"界面显示不对"，而是这个账号以后
 * 每次请求都用错身份 —— 上游只会回一个含义模糊的业务码。
 * 因此：① 不给默认值靠用户自己看，而是**显式**给两个按钮；
 * ② 选完在按钮下方把"会影响什么"写出来。
 */

import { onMounted, onUnmounted, ref } from 'vue'
import {
  cancelQrLogin,
  deleteAccount,
  getPcDevice,
  listAccounts,
  loginWithPassword,
  pollQrLogin,
  rotatePcDevice,
  startAppSmsLogin,
  startQrLogin,
  startSmsLogin,
  submitAppSmsLogin,
  submitSmsLogin,
  type AccountSummary,
  type PcPlatform,
  type SliderPayload,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { ConfirmDialog, closeDialog, openDialog } from '../ui/dialog'
import { showError } from '../ui/error-dialog'
import { pushToast } from '../ui/toast'

type LoadState = 'loading' | 'error' | 'ready'
type Method = 'qr' | 'sms' | 'app' | 'password'
type QrPhase = 'idle' | 'waiting' | 'scanned' | 'failed'

/** 两种平台的人话名字。`Record` 而不是函数：加平台时 TS 会报缺一项。 */
const PLATFORM_LABEL: Record<PcPlatform, string> = {
  windows: 'Windows',
  macos: 'macOS',
}

export const AccountsPage = () => {
  // ── 账号列表 ─────────────────────────────────────────────────────────
  const state = ref<LoadState>('loading')
  const loadError = ref('')
  const accounts = ref<AccountSummary[]>([])
  const deviceId = ref('')
  /** 待确认删除的账号（确认弹窗用）。 */
  const pendingDelete = ref<AccountSummary | null>(null)
  const deleting = ref(false)

  // ── 登录表单 ─────────────────────────────────────────────────────────
  const method = ref<Method>('qr')
  const platform = ref<PcPlatform>('windows')
  /**
   * 动作失败：内联显示在表单旁，不用弹窗。
   *
   * 这些失败都发生在**用户正在填的表单**里（手机号、验证码、账密、出码），
   * 错误必须贴着待改的字段，弹窗会把字段推远（§6.4）。
   * 列表上的独立动作（删除、换设备指纹）走 `showError`。
   */
  const formError = ref('')
  /**
   * 成功回执。
   *
   * ⚠ 与右上角的 toast **并存**，不是重复：toast 3 秒就没了（§5.11），
   * 而加完账号那一刻人的注意力在"账号列表有没有多一条"上 —— 实测就是
   * 「明明加成功了却像什么都没发生」。这一条**留在页面上**，
   * 直到下一次尝试登录（那时它已经过时了）才清掉。
   */
  const formOk = ref('')
  const busy = ref(false)

  // 扫码
  const qrImage = ref('')
  const qrSessionId = ref('')
  const qrPhase = ref<QrPhase>('idle')
  const qrNote = ref('')
  let pollTimer: number | null = null

  // 短信
  const phone = ref('')
  const smsCode = ref('')
  const smsSent = ref(false)
  const smsRemain = ref<number | null>(null)
  const slider = ref<SliderPayload | null>(null)

  // 账密
  const username = ref('')
  const password = ref('')

  // App 端（手机客户端）短信
  const appPhone = ref('')
  const appCode = ref('')
  const appSent = ref(false)
  const appRemain = ref<number | null>(null)
  /**
   * App 的滑块题：与 PC 的 `slider` 分开存。
   *
   * 两端的题面字段名不同（PC 是 `slider` 嵌套对象，App 是平铺的几个字段），
   * 共用一个 ref 会让"切了方法之后把上一端的答案带过去"变成一个静默的错。
   */
  const appSlider = ref<{ answer?: string; background?: string; width?: number | null } | null>(
    null,
  )

  const load = async () => {
    state.value = 'loading'
    loadError.value = ''
    try {
      const [list, dev] = await Promise.all([listAccounts(), getPcDevice()])
      accounts.value = list
      deviceId.value = dev.device_id
      state.value = 'ready'
    } catch (e) {
      loadError.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  /** 停掉轮询。离开页面、切方法、出码失败都要调 —— 否则会一直问下去。 */
  const stopPolling = () => {
    if (pollTimer !== null) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }

  /** 放弃当前扫码会话（服务端也把状态丢掉）。 */
  const abandonQr = () => {
    stopPolling()
    const sid = qrSessionId.value
    qrSessionId.value = ''
    qrImage.value = ''
    qrPhase.value = 'idle'
    qrNote.value = ''
    // 重新出码 = 一次新的尝试 ⇒ 上一条成功回执已经过时（与其它三条路径同口径）。
    formOk.value = ''
    if (sid) void cancelQrLogin(sid).catch(() => undefined)
  }

  const onMethodChange = (next: Method) => {
    if (next === method.value) return
    abandonQr()
    formError.value = ''
    smsSent.value = false
    slider.value = null
    // App 那一路的状态也要清 —— 否则切回来时会把上一端的滑块答案带着走，
    // 而那个答案是**另一道题**的，提交上去只会浪费一次尝试（接口计失败次数）。
    appSent.value = false
    appSlider.value = null
    method.value = next
  }

  // ── 扫码 ─────────────────────────────────────────────────────────────

  const beginQr = async () => {
    formError.value = ''
    busy.value = true
    abandonQr()
    try {
      const r = await startQrLogin(platform.value)
      qrImage.value = r.image_svg
      qrSessionId.value = r.session_id
      qrPhase.value = 'waiting'
      qrNote.value = `有效期 ${r.ttl_seconds} 秒`
      // 轮询间隔由服务端给：客户端写死一个数会和状态机分叉
      pollTimer = window.setInterval(() => void pollOnce(), r.poll_interval_ms)
    } catch (e) {
      formError.value = e instanceof Error ? e.message : '出码失败'
      qrPhase.value = 'failed'
    } finally {
      busy.value = false
    }
  }

  const pollOnce = async () => {
    const sid = qrSessionId.value
    if (!sid) {
      stopPolling()
      return
    }
    try {
      const r = await pollQrLogin(sid)
      if (r.phase === 'scanned') {
        qrPhase.value = 'scanned'
        qrNote.value = '已扫码，请在手机上确认'
        return
      }
      if (r.phase === 'logged_in') {
        stopPolling()
        qrSessionId.value = ''
        // ⚠ 这三格必须一起清。只清 `qrSessionId` 会留下"二维码还在、
        // 那句『已扫码，请在手机上确认』也还在"的界面 —— 而账号其实已经加好了，
        // 用户看到的是一个**已经完成**的流程仍停在半路。（实测踩过。）
        qrImage.value = ''
        qrNote.value = ''
        qrPhase.value = 'idle'
        const ok = `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`
        // ⚠ 与其它三条登录路径一致：3 秒的 toast 之外再留一条页面回执。
        // 扫码这条尤其需要 —— 人的注意力那时在手机上，toast 很容易整条错过。
        formOk.value = ok
        pushToast('success', ok)
        await load()
        return
      }
      if (r.phase === 'failed') {
        if (r.terminal) {
          stopPolling()
          qrPhase.value = 'failed'
          qrNote.value = r.reason ?? '扫码登录失败'
        } else {
          // 非终态（限流）：退避后接着问，**不**让用户重开一张码
          qrNote.value = r.reason ?? '稍后自动重试…'
          stopPolling()
          const wait = r.retry_after_ms ?? 5000
          pollTimer = window.setTimeout(() => {
            pollTimer = window.setInterval(() => void pollOnce(), 1000)
            void pollOnce()
          }, wait)
        }
      }
    } catch (e) {
      // 网络抖动不该终止会话：记一句、下一拍再问。
      // 只有服务端明确说"会话没了"（404）才停 —— 那时重开一张码才有意义。
      const status = (e as { status?: number }).status
      if (status === 404) {
        stopPolling()
        qrPhase.value = 'failed'
        qrNote.value = '会话已失效，请重新出码'
      } else {
        qrNote.value = '网络不稳，正在重试…'
      }
    }
  }

  // ── 短信 ─────────────────────────────────────────────────────────────

  /**
   * 发验证码。
   *
   * `sliderAnswer` 由本函数**自己**回填：上游把答案直接给了出来（`puzzleLeft`
   * 解出来就是坐标），所以这件事实质上是**全自动**的 —— 拿到 `need_slider`
   * 就带答案重发一次，用户只需要等那条短信。
   *
   * ⚠ 只在**第一次**（`sliderAnswer` 为空）自动重试**一次**：答案本身错了、
   * 或上游连着要两次，再自动打下去就成了对着上游刷请求。
   */
  const sendSms = async (sliderAnswer?: string) => {
    formError.value = ''
    if (!phone.value.trim()) {
      formError.value = '请填写手机号'
      return
    }
    busy.value = true
    try {
      const r = await startSmsLogin(phone.value.trim(), platform.value, sliderAnswer)
      if (r.phase === 'sent') {
        smsSent.value = true
        smsRemain.value = r.remain_get_times ?? null
        slider.value = null
        pushToast('success', '验证码已发送')
      } else {
        const answer = r.slider?.answer
        if (r.phase === 'need_slider' && answer && !sliderAnswer) {
          // 自动过关：不显示图、不要人点。
          pushToast('info', '已自动通过滑块验证，正在发送验证码')
          await sendSms(answer)
          return
        }
        // 只剩两种可能：题面解不出来，或自动重试后上游还要滑块。
        // 图留着只作**诊断**（人一眼看出卡在滑块上），但话必须说清
        // 是工具侧的问题 —— 不能让用户以为该自己去量像素。
        slider.value = r.slider ?? null
        smsSent.value = false
        formError.value =
          r.phase === 'slider_exhausted'
            ? '今日滑块验证次数已用完，请明天再试'
            : '滑块题自动解不开（上游没给答案），本次没发出短信'
      }
    } catch (e) {
      formError.value = e instanceof Error ? e.message : '发送验证码失败'
    } finally {
      busy.value = false
    }
  }

  const submitSms = async () => {
    formError.value = ''
    formOk.value = ''
    if (!smsCode.value.trim()) {
      formError.value = '请填写验证码'
      return
    }
    busy.value = true
    try {
      // ⚠ **不**把滑块答案带进这一枪：后端"带了就先重发一次验证码"，
      // 而验证码在上一步（点了「自动完成」之后）**已经发出来了** ⇒
      // 带上去会让用户白发一条短信、并让刚收到的那条失效。
      const r = await submitSmsLogin({
        phone: phone.value.trim(),
        platform: platform.value,
        code: smsCode.value.trim(),
      })
      pushToast('success', `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`,
      )
      formOk.value = `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`
      smsSent.value = false
      smsCode.value = ''
      slider.value = null
      await load()
    } catch (e) {
      formError.value = e instanceof Error ? e.message : '登录失败'
    } finally {
      busy.value = false
    }
  }

  // ── App 端（手机客户端）短信 ─────────────────────────────────────────

  /** 发验证码（App 链路）。与 PC 那支同一个道理：滑块由本函数自动过。 */
  const sendAppSms = async (sliderAnswer?: string) => {
    formError.value = ''
    if (!appPhone.value.trim()) {
      formError.value = '请填写手机号'
      return
    }
    busy.value = true
    try {
      const r = await startAppSmsLogin(appPhone.value.trim(), sliderAnswer)
      if (r.phase === 'sent') {
        appSent.value = true
        appRemain.value = r.remain_get_times ?? null
        appSlider.value = null
        pushToast('success', '验证码已发送')
      } else {
        const answer = r.slider_answer
        // ⚠ App 这一族的档位名是 `need_captcha`，**不是** PC 那个 `need_slider`
        // —— 两条链路的名字不一样，别顺手抄。
        if (r.phase === 'need_captcha' && answer && !sliderAnswer) {
          // 自动过关：不显示图、不要人点。
          pushToast('info', '已自动通过滑块验证，正在发送验证码')
          await sendAppSms(answer)
          return
        }
        appSlider.value = {
          answer: r.slider_answer,
          background: r.slider_background,
          width: r.slider_width,
        }
        appSent.value = false
        formError.value =
          r.phase === 'slider_exhausted'
            ? '这道滑块题已作废，请重新发送验证码'
            : '滑块题自动解不开（上游没给答案），本次没发出短信'
      }
    } catch (e) {
      formError.value = e instanceof Error ? e.message : '发送验证码失败'
    } finally {
      busy.value = false
    }
  }

  const submitAppSms = async () => {
    formError.value = ''
    formOk.value = ''
    if (!appCode.value.trim()) {
      formError.value = '请填写验证码'
      return
    }
    busy.value = true
    try {
      const r = await submitAppSmsLogin({
        phone: appPhone.value.trim(),
        code: appCode.value.trim(),
      })
      // App 账号没有 PC 平台，所以提示语里说"手机端"而不是某个平台名 ——
      // 说成 Windows 会让人以为它按 PC 的 UA 发请求。
      pushToast('success', `已添加账号 ${r.account ?? ''}（手机端）`)
      formOk.value = `已添加账号 ${r.account ?? ''}（手机端）`
      appSent.value = false
      appCode.value = ''
      appSlider.value = null
      await load()
    } catch (e) {
      formError.value = e instanceof Error ? e.message : '登录失败'
    } finally {
      busy.value = false
    }
  }

  // ── 账密 ─────────────────────────────────────────────────────────────

  const submitPassword = async () => {
    formError.value = ''
    formOk.value = ''
    if (!username.value.trim() || !password.value) {
      formError.value = '请填写用户名与口令'
      return
    }
    busy.value = true
    try {
      const r = await loginWithPassword(username.value.trim(), password.value, platform.value)
      pushToast('success', `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`,
      )
      formOk.value = `已添加账号 ${r.account ?? ''}（平台：${r.platform ? PLATFORM_LABEL[r.platform] : '未知'}）`
      password.value = ''
      await load()
    } catch (e) {
      formError.value = e instanceof Error ? e.message : '登录失败'
    } finally {
      busy.value = false
    }
  }

  // ── 删除与设备 ───────────────────────────────────────────────────────

  const askDelete = (a: AccountSummary) => {
    pendingDelete.value = a
    openDialog('confirm-delete-account')
  }

  const doDelete = async () => {
    const a = pendingDelete.value
    if (!a) return
    deleting.value = true
    try {
      await deleteAccount(a.id)
      closeDialog('confirm-delete-account')
      pushToast('success', `已移除 ${a.nickname || a.label || a.id}`)
      await load()
    } catch (e) {
      showError(
        `无法移除账号 ${a.nickname || a.label || a.id}`,
        e instanceof Error ? e.message : '移除失败',
      )
    } finally {
      deleting.value = false
      pendingDelete.value = null
    }
  }

  const doRotateDevice = async () => {
    try {
      const r = await rotatePcDevice()
      deviceId.value = r.device_id
      pushToast('success', '已更换设备指纹（旧的扫码会话立即失效）')
    } catch (e) {
      // 独立动作，没有表单在场；且原因（例如上游拒绝）需要被读到
      showError('无法更换设备指纹', e instanceof Error ? e.message : '更换失败')
    }
  }

  onMounted(() => void load())
  onUnmounted(() => stopPolling())

  /** 到期时间的说法。`0` = 解析不出，与"已过期"不是一回事。 */
  const expireText = (ms: number) => {
    if (!ms) return '未知'
    const left = ms - Date.now()
    if (left <= 0) return '已过期'
    const days = Math.floor(left / 86_400_000)
    if (days >= 1) return `${days} 天后`
    const hours = Math.floor(left / 3_600_000)
    return hours >= 1 ? `${hours} 小时后` : '不到 1 小时'
  }

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h1 class="text-base font-medium">账号</h1>
        <button
          type="button"
          class="btn btn-sm btn-soft min-h-11 md:min-h-0"
          disabled={state.value === 'loading'}
          onClick={() => void load()}
        >
          刷新
        </button>
      </div>

      {/* ── 已登录账号 ─────────────────────────────────────────────── */}
      {state.value === 'loading' ? (
        <div class="flex justify-center py-10">
          <span class="loading loading-lg text-primary" />
        </div>
      ) : null}

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{loadError.value}</span>
          <button type="button" class="btn btn-sm btn-soft" onClick={() => void load()}>
            重试
          </button>
        </div>
      ) : null}

      {state.value === 'ready' && accounts.value.length === 0 ? (
        <EmptyState
          title="还没有账号"
          hint="用下面的任意一种方式登录一个移动云盘账号，登录后就能对它跑任务。"
        />
      ) : null}

      {accounts.value.length > 0 ? (
        <ul class="flex flex-col gap-2">
          {accounts.value.map((a) => (
            <li class="card card-border bg-base-100">
              <div class="card-body flex-row flex-wrap items-center gap-3 p-3">
                <div class="flex min-w-0 flex-1 flex-col">
                  <span class="truncate font-medium">{a.nickname || a.label || a.id}</span>
                  <span class="text-xs opacity-70">
                    设备 {a.device_id} · 凭据 {expireText(a.expire)}
                  </span>
                </div>
                {/* 身份必须显示：它决定这个账号以后每次请求用哪套头和 UA。
                    `pc_platform` 为空 = App 端登录（或更早的历史数据）。 */}
                {a.pc_platform ? (
                  <span class="badge badge-soft badge-primary badge-sm">
                    {PLATFORM_LABEL[a.pc_platform]}
                  </span>
                ) : (
                  <span
                    class="badge badge-soft badge-sm"
                    title="App（手机端）登录的账号：用 mobile 前缀与 App 设备头"
                  >
                    手机端
                  </span>
                )}
                <button
                  type="button"
                  class="btn btn-sm btn-soft btn-error min-h-11 md:min-h-0"
                  onClick={() => askDelete(a)}
                >
                  移除
                </button>
              </div>
            </li>
          ))}
        </ul>
      ) : null}

      {/* ── 添加账号 ───────────────────────────────────────────────── */}
      <div class="card card-border bg-base-100">
        <div class="card-body gap-4 p-4">
          <h2 class="card-title text-sm">添加账号</h2>

          {/* 平台：必须选，且必须说清影响。
              手机端**没有**这个选择 —— App 只有一种形态，摆一个选不了或
              选了没用的开关只会让人以为它有意义。 */}
          {method.value === 'app' ? (
            <div class="alert alert-soft text-sm" role="status">
              <span>
                手机端登录用 App 自己的身份（<code>mobile</code> 前缀）与设备头，
                不存在 Windows / macOS 之分，所以这里不需要选客户端形态。
              </span>
            </div>
          ) : (
            <div class="flex flex-col gap-2">
              <span class="text-xs font-medium opacity-70">客户端形态</span>
              <div class="join">
                {(['windows', 'macos'] as const).map((p) => (
                  <button
                    type="button"
                    class={`btn join-item btn-sm min-h-11 md:min-h-0 ${platform.value === p ? 'btn-primary' : 'btn-soft'}`}
                    onClick={() => (platform.value = p)}
                  >
                    {PLATFORM_LABEL[p]}
                  </button>
                ))}
              </div>
              <p class="text-xs opacity-70">
                这会随账号存进 <code>accounts.json</code>，之后每次请求的
                UA、设备串与渠道号都按它来。选错的话，账号以后一直用错的身份发请求。
              </p>
            </div>
          )}

          {/* 登录方式。
              ⚠ 用 `join` + `btn` 而**不是** daisyUI 的 `tabs` / `tabs-box`：
              后者会把整个 tab 组件（含四套朝向变体与带框那套，实测 14.5KB）
              拖进产物，而全项目只有这一处是"选一个"的语义。
              写法与上面「客户端形态」保持一致 —— 同一页里两个同级选择器，
              不该一个用 tabs、一个用 join。 */}
          <div class="join">
            {(
              [
                ['qr', '扫码'],
                ['sms', '短信'],
                ['app', '手机端'],
                ['password', '账号密码'],
              ] as const
            ).map(([id, label]) => (
              <button
                type="button"
                aria-pressed={method.value === id}
                class={`btn join-item btn-sm min-h-11 md:min-h-0 ${method.value === id ? 'btn-primary' : 'btn-soft'}`}
                onClick={() => onMethodChange(id)}
              >
                {label}
              </button>
            ))}
          </div>

          {formOk.value ? (
            <div class="alert alert-soft alert-success text-sm" role="status">
              <span class="break-words">{formOk.value}</span>
            </div>
          ) : null}

          {formError.value ? (
            <div class="alert alert-soft alert-error text-sm" role="alert">
              <span class="break-words">{formError.value}</span>
            </div>
          ) : null}

          {/* 扫码 */}
          {method.value === 'qr' ? (
            <div class="flex flex-col items-center gap-3">
              {qrImage.value ? (
                // 白底容器：深色主题下二维码必须保持黑码白底（扫码器假设深色是数据）
                <div class="rounded-box bg-white p-3">
                  <img src={qrImage.value} alt="登录二维码" class="h-48 w-48" />
                </div>
              ) : (
                <p class="text-sm opacity-70">点下面的按钮生成二维码。</p>
              )}
              {qrNote.value ? (
                <p
                  class={`text-sm ${qrPhase.value === 'failed' ? 'text-error' : 'opacity-70'}`}
                >
                  {qrNote.value}
                </p>
              ) : null}
              <div class="flex flex-wrap justify-center gap-2">
                <button
                  type="button"
                  class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                  disabled={busy.value}
                  onClick={() => void beginQr()}
                >
                  {qrImage.value ? '重新出码' : '生成二维码'}
                </button>
                {qrSessionId.value ? (
                  <button
                    type="button"
                    class="btn btn-soft btn-sm min-h-11 md:min-h-0"
                    onClick={abandonQr}
                  >
                    取消
                  </button>
                ) : null}
              </div>
              <p class="text-xs opacity-70">用移动云盘 App 扫码，然后在手机上确认。</p>
            </div>
          ) : null}

          {/* 短信 */}
          {method.value === 'sms' ? (
            <div class="flex flex-col gap-3">
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">手机号</span>
                <input
                  type="tel"
                  class="input w-full"
                  placeholder="19900000001"
                  value={phone.value}
                  onInput={(e) => {
                    phone.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <div class="flex flex-wrap gap-2">
                <button
                  type="button"
                  class="btn btn-soft btn-sm min-h-11 md:min-h-0"
                  disabled={busy.value}
                  onClick={() => void sendSms()}
                >
                  {smsSent.value ? '重新发送' : '发送验证码'}
                </button>
                {smsRemain.value != null ? (
                  <span class="self-center text-xs opacity-70">
                    今日还可发送 {smsRemain.value} 次
                  </span>
                ) : null}
              </div>

              {/*
                ⚠ 这里**刻意不显示题面图、也不给按钮**。
                滑块答案不是"识别"出来的 —— 上游把它加密在 `puzzleLeft` 里，
                解出来就是坐标（见 `SliderPuzzle::answer`）。所以能自动过的时候
                这张图纯属干扰：它会让人以为"得自己去量像素"。
                走到这一格只剩"上游这次没给答案"，那是工具侧解不开，
                要说清楚并让人重发，而不是把图甩出来让人自己想办法。
              */}
              {slider.value && !slider.value.answer ? (
                <div class="alert alert-soft alert-warning text-xs" role="status">
                  <span>
                    上游这次没把滑块答案给出来，工具侧解不开。这不是让你去核对图片 ——
                    答案本来就在题面里加密着。请稍后再发一次验证码。
                  </span>
                </div>
              ) : null}

              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">验证码</span>
                <input
                  type="text"
                  inputmode="numeric"
                  class="input w-full"
                  placeholder="6 位数字"
                  value={smsCode.value}
                  onInput={(e) => {
                    smsCode.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <button
                type="button"
                class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                disabled={busy.value}
                onClick={() => void submitSms()}
              >
                登录
              </button>
            </div>
          ) : null}

          {/* 手机端（App）短信 */}
          {method.value === 'app' ? (
            <div class="flex flex-col gap-3">
              <p class="text-xs opacity-70">
                与上面的「短信」**不是同一条链路**：手机端走 App 自己的密钥与设备头，
                登录后凭据以 <code>mobile</code> 前缀落盘。
              </p>
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">手机号</span>
                <input
                  type="tel"
                  class="input w-full"
                  placeholder="19900000001"
                  value={appPhone.value}
                  onInput={(e) => {
                    appPhone.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <div class="flex flex-wrap gap-2">
                <button
                  type="button"
                  class="btn btn-soft btn-sm min-h-11 md:min-h-0"
                  disabled={busy.value}
                  onClick={() => void sendAppSms()}
                >
                  {appSent.value ? '重新发送' : '发送验证码'}
                </button>
                {appRemain.value != null ? (
                  <span class="self-center text-xs opacity-70">
                    今日还可发送 {appRemain.value} 次
                  </span>
                ) : null}
              </div>

              {/* 同 PC 那一格：不显示题面图、不给按钮 —— 滑块由发码函数自动过。 */}
              {appSlider.value && !appSlider.value.answer ? (
                <div class="alert alert-soft alert-warning text-xs" role="status">
                  <span>
                    上游这次没把滑块答案给出来，工具侧解不开。请稍后再发一次验证码。
                  </span>
                </div>
              ) : null}

              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">验证码</span>
                <input
                  type="text"
                  inputmode="numeric"
                  class="input w-full"
                  placeholder="6 位数字"
                  value={appCode.value}
                  onInput={(e) => {
                    appCode.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <button
                type="button"
                class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                disabled={busy.value}
                onClick={() => void submitAppSms()}
              >
                登录
              </button>
            </div>
          ) : null}

          {/* 账密 */}
          {method.value === 'password' ? (
            <div class="flex flex-col gap-3">
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">手机号</span>
                <input
                  type="text"
                  class="input w-full"
                  value={username.value}
                  onInput={(e) => {
                    username.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">口令</span>
                <input
                  type="password"
                  class="input w-full"
                  value={password.value}
                  onInput={(e) => {
                    password.value = (e.target as HTMLInputElement).value
                  }}
                />
              </label>
              <p class="text-xs opacity-70">
                口令只在本次请求里用于计算摘要，**不会**存进浏览器或数据库。
              </p>
              <button
                type="button"
                class="btn btn-primary btn-sm min-h-11 md:min-h-0"
                disabled={busy.value}
                onClick={() => void submitPassword()}
              >
                登录
              </button>
            </div>
          ) : null}
        </div>
      </div>

      {/* ── 设备指纹 ───────────────────────────────────────────────── */}
      <div class="card card-border bg-base-100">
        <div class="card-body gap-2 p-4">
          <h2 class="card-title text-sm">本机设备指纹</h2>
          <code class="break-all text-xs opacity-70">{deviceId.value || '（尚未生成）'}</code>
          <p class="text-xs opacity-70">
            扫码时它作为 <code>dID</code> 上报，服务端据此定位会话。
            它跨重启稳定；换掉会让旧的扫码会话立即失效，但已登录账号不受影响。
          </p>
          <button
            type="button"
            class="btn btn-soft btn-sm self-start min-h-11 md:min-h-0"
            onClick={() => void doRotateDevice()}
          >
            换一台设备
          </button>
        </div>
      </div>

      <ConfirmDialog
        id="confirm-delete-account"
        title="移除账号"
        impact={
          <span>
            将从本实例移除 <b>{pendingDelete.value?.nickname || pendingDelete.value?.label || pendingDelete.value?.id}</b>{' '}
            及其本地凭据。云端的数据不受影响，重新登录即可恢复。
          </span>
        }
        confirmLabel="移除"
        busy={deleting.value}
        onConfirm={() => void doDelete()}
      />
    </div>
  )
}
