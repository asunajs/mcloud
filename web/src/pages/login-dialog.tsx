/**
 * 登录移动云盘账号：扫码与短信两条路，一个弹窗里切。
 *
 * 短信那条**界面上是三端（手机 App / macOS / Windows），后端只有两条路由**
 * （`/api/login/sms/app` 与 `/api/login/sms/pc`，后者带 `platform`）：两端档案、
 * 滑块归谁解、请求体是否加密都不一样，CLI 那边同理（`mcloud login sms --client PC|APP`）。
 *
 * # 为什么前端要自己轮询（而不是后端推）
 *
 * 一次登录最长 120 秒，期间只有四个阶段值要通知，而"页面刷新之后还能续上"
 * 靠的是服务端那张会话表 —— 既然状态已经在服务端了，问比推少一套机制。
 * 判据是 `phase`：`succeeded` / `failed` 是终局，别的继续问。
 *
 * # 凭据边界（这一页是"往系统里塞凭据"的那个入口）
 *
 *  - **手机号与验证码是入参，不是出参**：不进 URL、不进日志、失败文案不插值它们。
 *    ⚠ **发码那条响应里的 `accountId` 是空的** —— 服务端只在登录落库后才填它
 *    （`internal/app/login_hub.go`：`SendAppCode` 给的是 `LoginWaiting` + 配额，
 *    `VerifyAppCode` 成功才回 `AccountID`）。所以界面上**没有**"已发到 199****2716"
 *    这种回显，只能写"验证码已发出"；别拿 `accountId` 去拼掩码手机号。
 *  - 输入框一律 `autocomplete="off"` + 提交即清空：登录页不该把手机号留在
 *    浏览器自动填充里，也不该在页面重开后还看得见上一次的粘贴内容；
 *  - 二维码载荷（`qrUrl`）**可以**显示：那串是"给手机扫的 URL"，含设备指纹但不含凭据。
 *
 * # 布局（2026-09-22 重做，三条规矩都是"改之前哪里不对"推出来的）
 *
 *  - **控件一律整行**：原先"登录端"那个下拉居中浮在一整行里，上下不贴东西，看着像掉队。
 *    现在每个控件都 `w-full` 并配 `fieldset-legend` 的标签，整页是一条竖直的表单流
 *    （表单形态见设计指南 §5.6、间距见 §3.5）。
 *  - **一屏只有一个实心按钮**（§3.4）：发码前"发送验证码"是唯一主行动；发出之后主行动
 *    换成"提交验证码"，发码那枚退成 `-soft`。改之前两枚都是实心并排立着，重点反而没了。
 *  - **发码之后要有一句"码已经出去了"**：手机号是入参用完即弃（发码成功即清空输入框），
 *    那一格空着、按钮灰着，不解释一句用户只会以为刚才没点动。
 *
 * ⚠ 滑块那一支的文案**与 CLI 对齐**：短信是那个 H5 页面**自己发的**
 * （流程图见 `docs/upstream/app-sms-login.md` §3 与 `internal/cli/run_login.go` 的同款提示），
 * 所以是"过完滑块回来**填**码"，不是"回来**重新发**码" —— 后者会让用户白刷一次今日配额。
 * 改之前这里写的就是"重新发码"，与 `internal/server/login.go` 那句
 * "那一支的语义是去这个地址拖一下，再回来填码"也自相矛盾。
 */

import { onUnmounted, ref } from 'vue'
import {
  ApiError,
  loginSession,
  qrImageHref,
  sendAppLoginCode,
  sendPcLoginCode,
  startQrLogin,
  verifyAppLoginCode,
  verifyPcLoginCode,
  type LoginPhase,
} from '../api'
import { closeDialog, Dialog } from '../ui/dialog'

export interface LoginDialogProps {
  /** 每个弹窗一个唯一 id（组件指南 §5.8）。 */
  id: string
  /** 登录成功并落库之后回调（父页面据此刷新列表）。 */
  onLoggedIn: (accountId: string) => void
}

/** 轮询间隔。1.5 秒：登录最长 120 秒 ⇒ 一次会话最多 80 个请求，够快也不吵。 */
const POLL_MS = 1500

/** 重新发码的冷却。上游自己也有配额（`remainGetTimes`），这一层只是不让手抖连点。 */
const RESEND_COOLDOWN_S = 60

const PHASE_HINT: Record<string, string> = {
  waiting: '等待扫码 …',
  scanned: '已扫码，请在手机上点确认',
  succeeded: '登录成功，账号已添加',
  failed: '登录没成',
}

/**
 * "发送验证码"按钮的两档强度（§3.4：一屏只有一个实心按钮）。
 *
 * 还没发码时它是唯一的主行动（实心）；码发出去之后主行动换成"提交验证码"，
 * 这一枚必须退成 `-soft`，否则两枚实心并排立着 —— 那正是重做前那个界面。
 *
 * 两档都写成**完整字面量**：Tailwind 只认源码里字面出现的类名，
 * `btn-${x}` 这类拼接在生产构建后会被静默丢掉（代码指南 §9.1①）。
 */
const SEND_BTN = {
  primary: 'btn btn-primary w-full',
  secondary: 'btn btn-soft btn-primary w-full',
} as const

type Tab = 'qr' | 'sms'

export const LoginDialog = (props: LoginDialogProps) => {
  const tab = ref<Tab>('qr')
  const error = ref('')
  const busy = ref(false)

  // ── 扫码 ────────────────────────────────────────────────────────────────
  const sessionId = ref('')
  const qrUrl = ref('')
  const phase = ref<LoginPhase | ''>('')
  const platform = ref<'windows' | 'macos'>('windows')
  let pollTimer: number | undefined

  const stopPoll = () => {
    if (pollTimer !== undefined) {
      window.clearInterval(pollTimer)
      pollTimer = undefined
    }
  }

  /** 收尾：停轮询 + 停冷却，并把两个输入清空。关闭与卸载走的是同一条路。 */
  const reset = () => {
    stopPoll()
    stopCooldown()
    // 只停表不清零会把倒计时**冻在剩余秒数上**：`stopCooldown` 清掉了 interval，
    // 于是重开弹窗后那一段既不减到 0、又让"发送验证码"一直 disabled —— 整栏作废到刷新页面。
    cooldown.value = 0
    sessionId.value = ''
    qrUrl.value = ''
    phase.value = ''
    phone.value = ''
    code.value = ''
    smsSession.value = ''
    smsSentVia.value = ''
    smsEnd.value = 'app'
    remain.value = null
    captchaUrl.value = ''
    solvedSlider.value = false
    busy.value = false
  }

  const onResult = (p: LoginPhase, accountId: string | null, message: string) => {
    phase.value = p
    // 终局才停轮询：waiting / scanned 意味着"还得接着问"
    if (p === 'succeeded' || p === 'failed') stopPoll()
    if (p === 'succeeded') {
      // 成功后**不**把 accountId 留在界面上：父页面刷新列表，那一行自己会出来
      props.onLoggedIn(accountId ?? '')
      closeDialog(props.id)
      return
    }
    if (p === 'failed') error.value = message || '登录没成'
  }

  const startPolling = (id: string) => {
    stopPoll()
    pollTimer = window.setInterval(async () => {
      try {
        const view = await loginSession(id)
        onResult(view.phase, view.accountId, view.message)
      } catch (e) {
        // 会话过期/被别人拿走都会落到 404：那是"这次登录结束了"，继续问没有意义
        stopPoll()
        error.value = e instanceof ApiError ? e.message : '问不到这次登录的状态了'
      }
    }, POLL_MS)
  }

  const onAskQr = async () => {
    error.value = ''
    busy.value = true
    phase.value = ''
    try {
      const result = await startQrLogin(platform.value)
      sessionId.value = result.sessionId
      qrUrl.value = result.qrUrl
      phase.value = 'waiting'
      startPolling(result.sessionId)
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : '出码失败'
    } finally {
      busy.value = false
    }
  }

  // ── 短信 ────────────────────────────────────────────────────────────────
  const phone = ref('')
  const code = ref('')
  const smsSession = ref('')
  const remain = ref<number | null>(null)
  const captchaUrl = ref('')
  const solvedSlider = ref(false)
  const cooldown = ref(0)

  /**
   * 界面上选的登录端（还没发码之前随时可改）。
   *
   * 三选而不是"先选 PC、再选 Win/Mac"：这两级其实是**同一个决定**（我要以哪个客户端
   * 的身份登录），拆两级会让"PC"这一级看起来像一个可点的档，而它没有对应端档案。
   * 默认给手机 App：那条是端到端实测跑通过的一条（PC 短信这轮才接上界面）。
   */
  const smsEnd = ref<'app' | 'mac' | 'win'>('app')
  /**
   * 这一次会话**实际**是按哪条路由发出去的（后端只有两条：`/sms/app` 与 `/sms/pc`）。
   *
   * 与上面那个分开是有意的：填码那一步必须回到发码那一条路由上。若直接读 `smsEnd`，
   * "发完码再把选择器改成另一端"就会拿 App 的会话去走 PC 的口子
   * （后端现在给 404，但表现是"验证码明明收到却提交不了"，排查的人不会想到选择器）。
   */
  const smsSentVia = ref<'app' | 'pc' | ''>('')
  let cooldownTimer: number | undefined

  const stopCooldown = () => {
    if (cooldownTimer !== undefined) {
      window.clearInterval(cooldownTimer)
      cooldownTimer = undefined
    }
  }

  const startCooldown = () => {
    cooldown.value = RESEND_COOLDOWN_S
    stopCooldown()
    cooldownTimer = window.setInterval(() => {
      cooldown.value -= 1
      if (cooldown.value <= 0) stopCooldown()
    }, 1000)
  }

  const onSendCode = async () => {
    error.value = ''
    busy.value = true
    try {
      const view =
        smsEnd.value === 'app'
          ? await sendAppLoginCode(phone.value.trim())
          : await sendPcLoginCode(phone.value.trim(), smsEnd.value === 'mac' ? 'macos' : 'windows')
      // 入参用完即弃：界面上不留完整手机号
      phone.value = ''
      smsSession.value = view.sessionId ?? ''
      smsSentVia.value = smsEnd.value === 'app' ? 'app' : 'pc'
      remain.value = view.remainGetTimes
      captchaUrl.value = view.captchaUrl
      solvedSlider.value = view.solvedSlider === true
      phase.value = view.phase
      // 滑块那一支**不是失败**（上游只是要人拖一下，短信由那个 H5 页面发出），
      // 所以这里既不进 error（界面那条 alert 自己带地址与按钮，两处都写就是
      // 同一条消息说两遍，§6.3 同屏不给两个 alert）、也不起冷却。
      if (!view.captchaUrl) startCooldown()
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : '发码失败'
    } finally {
      busy.value = false
    }
  }

  const onVerify = async () => {
    error.value = ''
    busy.value = true
    try {
      const view =
        smsSentVia.value === 'pc'
          ? await verifyPcLoginCode(smsSession.value, code.value.trim())
          : await verifyAppLoginCode(smsSession.value, code.value.trim())
      code.value = ''
      onResult(view.phase, view.accountId, view.message)
      if (view.phase === 'succeeded') {
        stopCooldown()
        cooldown.value = 0
      }
    } catch (e) {
      error.value = e instanceof ApiError ? e.message : '验证码没通过'
    } finally {
      busy.value = false
    }
  }

  onUnmounted(reset)

  // ── 视图 ────────────────────────────────────────────────────────────────
  /** 两个页签等分整行：`flex-1` 是给 `.tab` 补的（它默认只按内容宽，见 daisyUI 的 `tab.css`）。 */
  const tabButton = (id: Tab, label: string) => (
    <button
      type="button"
      role="tab"
      aria-selected={tab.value === id}
      class={tab.value === id ? 'tab tab-active flex-1' : 'tab flex-1'}
      onClick={() => {
        tab.value = id
        error.value = ''
      }}
    >
      {label}
    </button>
  )

  const qrPane = () => (
    <div class="flex flex-col gap-3">
      <fieldset class="fieldset">
        {/* PC 端档案（`clientType`/`cpid`/版本号）。只有扫码这一条用它：
            短信那条把端别并进自己的三选里了。 */}
        <legend class="fieldset-legend">客户端</legend>
        <select
          class="select w-full"
          aria-label="客户端类型"
          value={platform.value}
          onChange={(e) =>
            (platform.value = (e.target as HTMLSelectElement).value as 'windows' | 'macos')
          }
        >
          <option value="windows">Windows 客户端</option>
          <option value="macos">macOS 客户端</option>
        </select>
      </fieldset>

      <button
        type="button"
        class="btn btn-primary w-full"
        disabled={busy.value}
        onClick={() => void onAskQr()}
      >
        {/* 已经出过码也得能再来一次：一张码挂 120 秒，扫输了不用关掉重开 */}
        {sessionId.value === '' ? '生成二维码' : '换一张码'}
      </button>

      {sessionId.value === '' ? (
        <p class="text-sm opacity-70">用「移动云盘」App 扫码并确认，成功后账号自动出现在列表里。</p>
      ) : (
        <div class="flex flex-col items-center gap-3">
          <img
            src={qrImageHref(sessionId.value)}
            width={224}
            height={224}
            alt="登录二维码"
            class="rounded-box border border-base-300 bg-base-100 p-2"
          />
          <div class="flex h-6 items-center gap-2 text-sm">
            {phase.value === 'succeeded' || phase.value === 'failed' ? null : (
              <span class="loading loading-spinner loading-xs text-primary" />
            )}
            <span>{PHASE_HINT[phase.value] ?? '等待扫码 …'}</span>
          </div>
          {/* 兜底链接：有的手机扫不出这张码（屏太暗、摄像头太旧），手输地址还能走 */}
          <details class="w-full">
            <summary class="cursor-pointer text-xs opacity-60">扫不出来？看这串地址</summary>
            <code class="mt-1 block w-full break-all rounded-box bg-base-200 p-2 text-xs">
              {qrUrl.value}
            </code>
          </details>
        </div>
      )}
    </div>
  )

  /**
   * 手机号那一格下面的说明（§5.10：空着的时候要说清下一步）。
   *
   * 发码成功后输入框按凭据纪律被清空，所以这里必须替它说一句"码已经发出去了" ——
   * 否则用户看到的是一个空框加一枚灰按钮，只会以为刚才那下没点动。
   * 三种状态各说各的；都没得说时这一行**不占位置**（不写"请输入手机号"这种废话）。
   */
  const phoneHint = () => {
    if (captchaUrl.value !== '') return '短信由滑块页面发出，收到后把验证码填在下面。'
    if (smsSession.value !== '') return '验证码已发出；要换号，直接改这里再点发送。'
    if (remain.value !== null) return `今天还能领 ${remain.value} 次。`
    return ''
  }

  const smsPane = () => {
    const hint = phoneHint()
    return (
      <div class="flex flex-col gap-3">
        <fieldset class="fieldset">
          {/* 一个三选，不是"先 PC 再 Win/Mac"两级：这两级是同一个决定，
              拆两级会让"PC"看起来像一个可选端档案，而它没有。
              发码之后锁住：填码那一步必须回到发码的那条路由上（见 `smsSentVia`）。 */}
          <legend class="fieldset-legend">登录端</legend>
          <select
            class="select w-full"
            aria-label="登录端"
            value={smsEnd.value}
            disabled={smsSession.value !== ''}
            onChange={(e) =>
              (smsEnd.value = (e.target as HTMLSelectElement).value as 'app' | 'mac' | 'win')
            }
          >
            <option value="app">手机 App</option>
            <option value="mac">macOS 客户端</option>
            <option value="win">Windows 客户端</option>
          </select>
        </fieldset>

        <fieldset class="fieldset">
          <legend class="fieldset-legend">手机号</legend>
          <input
            class="input w-full"
            type="tel"
            inputmode="numeric"
            autocomplete="off"
            placeholder="11 位手机号"
            value={phone.value}
            onInput={(e) => (phone.value = (e.target as HTMLInputElement).value)}
          />
          {hint === '' ? null : <p class="fieldset-label">{hint}</p>}
        </fieldset>

        {/* 强度跟着"现在该做哪一步"走：没发码时它是主行动，发出去之后让位给"提交验证码"。 */}
        <button
          type="button"
          class={smsSession.value === '' ? SEND_BTN.primary : SEND_BTN.secondary}
          disabled={busy.value || cooldown.value > 0 || phone.value.trim().length === 0}
          onClick={() => void onSendCode()}
        >
          {cooldown.value > 0 ? `重新发送 ${cooldown.value}s` : '发送验证码'}
        </button>

        {/* App 那条的滑块兜底（PC 那条由后端自动解，解不动直接失败，不给地址）。
            文案与 CLI 对齐：**短信由那个 H5 页面自己发出**，所以下一步是回来**填**码。 */}
        {captchaUrl.value === '' ? null : (
          <div class="flex flex-col gap-2">
            <div class="alert alert-soft alert-warning text-sm" role="alert">
              <span class="break-words">
                上游要求先过滑块：请在打开的页面里拖动拼图完成验证，短信由那个页面发出。
              </span>
            </div>
            <a class="btn btn-soft w-full" href={captchaUrl.value} target="_blank" rel="noreferrer">
              打开滑块页面
            </a>
          </div>
        )}

        {solvedSlider.value && captchaUrl.value === '' ? (
          <p class="text-xs opacity-60">刚才自动过了一次滑块校验。</p>
        ) : null}

        {smsSession.value === '' ? null : (
          <fieldset class="fieldset">
            <legend class="fieldset-legend">验证码</legend>
            <input
              class="input w-full"
              type="text"
              inputmode="numeric"
              autocomplete="one-time-code"
              maxlength={8}
              value={code.value}
              onInput={(e) => (code.value = (e.target as HTMLInputElement).value)}
            />
          </fieldset>
        )}

        {smsSession.value === '' ? null : (
          <button
            type="button"
            class="btn btn-primary w-full"
            disabled={busy.value || code.value.trim() === ''}
            onClick={() => void onVerify()}
          >
            提交验证码
          </button>
        )}
      </div>
    )
  }

  return (
    <Dialog id={props.id} title="登录移动云盘账号" size="md" onClose={reset}>
      <div role="tablist" class="tabs tabs-box w-full">
        {tabButton('qr', '扫码登录')}
        {tabButton('sms', '短信登录')}
      </div>

      {error.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{error.value}</span>
        </div>
      ) : null}

      {tab.value === 'qr' ? qrPane() : smsPane()}

      <div class="modal-action">
        {/* 不在这儿 `reset()`：ESC / 遮罩 / 这个按钮三条关闭路都会派发 `close`，
            而 `Dialog` 的 `onClose` 已经挂了 reset —— 在这里再停一次定时器
            等于把同一件事写两处，漏一处就是"某种关法还在后台轮询"。 */}
        <button type="button" class="btn btn-soft" onClick={() => closeDialog(props.id)}>
          关闭
        </button>
      </div>
    </Dialog>
  )
}
