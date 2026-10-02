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

import { onMounted, onUnmounted, ref } from 'lite'
import {
  ApiError,
  NOTIFY_CHANNELS,
  cancelQrLogin,
  deleteAccount,
  getAccountSettings,
  getPcDevice,
  importAccount,
  listAccounts,
  loginWithPassword,
  pollQrLogin,
  putAccountSettings,
  rotatePcDevice,
  startAppSmsLogin,
  startQrLogin,
  startSmsLogin,
  submitAppSmsLogin,
  submitSmsLogin,
  updateAccount,
  type AccountSettings,
  type AccountSummary,
  type NotifyChannelKey,
  type PcPlatform,
  type PushConfig,
  type PushLogLevel,
  type SliderPayload,
} from '../api'
import { EmptyState } from '../ui/empty-state'
import { ConfirmDialog, Dialog, closeDialog, openDialog } from '../ui/dialog'
import { showError } from '../ui/error-dialog'
import { pushToast } from '../ui/toast'
import { Select } from '../ui/select'

type LoadState = 'loading' | 'error' | 'ready'
type Method = 'qr' | 'sms' | 'app' | 'password' | 'auth'
type QrPhase = 'idle' | 'waiting' | 'scanned' | 'failed'

/**
 * 从凭据串里**猜**平台前缀，**只**用来决定"要不要让用户选客户端形态"。
 *
 * ⚠ 这不是校验：真正的判定在服务端（`accounts::platform_for`），这里猜错的
 * 后果只是多显示/少显示一个选择器。猜不出来一律按"要选"处理 —— PC 是常见情况，
 * 而服务端对 `pc` 凭据**要求**形态，少显示会让提交必然失败。
 */
const credentialPrefix = (raw: string): string => {
  const text = raw.trim().replace(/^Basic\s+/i, '')
  if (!text) return ''
  try {
    return atob(text).split(':', 1)[0] ?? ''
  } catch {
    // 不是合法 base64 ⇒ 交给服务端报错（它的错话更准）
    return ''
  }
}

/** 两种平台的人话名字。`Record` 而不是函数：加平台时 TS 会报缺一项。 */
const PLATFORM_LABEL: Record<PcPlatform, string> = {
  windows: 'Windows',
  macos: 'macOS',
}


// ── 每账号设置的常量（与后端 DTO 一一对应）─────────────────────────────

/** 推送标题的默认值 —— 与后端 `mcloud_core::push::DEFAULT_TITLE` 同字面量。 */
const DEFAULT_NOTIFY_TITLE = 'mcloud 运行推送'

/** 推送渠道的人话名字。`Record` ⇒ 加渠道时 TS 会报缺一项。 */
const CHANNEL_LABEL: Record<NotifyChannelKey, string> = {
  pushplus: 'PushPlus',
  serverChan: 'Server 酱',
  workWeixinBot: '企业微信机器人',
  dingTalk: '钉钉机器人',
  customPost: '自定义 POST',
  workWeixin: '企业微信应用',
  tgBot: 'Telegram Bot',
  bark: 'Bark',
  twoIm: '回逍（2Im）',
  email: '邮箱',
}

/**
 * 渠道的**必填**字段 —— 与后端 `validate_notify` 逐条对应。
 *
 * 判据只有一条：这些字段**全空** ⇒ 这个渠道算"没配"，PUT 时整条不带
 * （⚠ **不发空对象**：`{token:""}` 会被后端当成"填了但为空"⇒ 400，而用户只是没填）。
 * 填了一部分就照发 —— 后端会指名缺哪一格，那比前端自己猜准。
 */
const CHANNEL_REQUIRED: Record<NotifyChannelKey, readonly string[]> = {
  pushplus: ['token'],
  serverChan: ['token'],
  workWeixinBot: ['url'],
  dingTalk: ['token'],
  customPost: ['url'],
  workWeixin: ['corpid', 'corpsecret', 'agentid'],
  tgBot: ['token', 'chat_id'],
  bark: ['key'],
  twoIm: ['key', 'sid'],
  email: ['host', 'from', 'pass'],
}

/**
 * 渠道里必须是 **JSON 数字**的字段（后端是 `i64` / `u16`）。
 *
 * 界面只能给字符串，提交时转 —— 不转的话后端回的是
 * `invalid type: string, expected i64`，一句用户看不懂的话。
 * `chat_id` 也在里面，但它可以是 `@channelname`：不是整数的就原样留着（后端收 `Value`）。
 */
const CHANNEL_NUMERIC: Partial<Record<NotifyChannelKey, readonly string[]>> = {
  workWeixin: ['agentid'],
  tgBot: ['chat_id'],
  email: ['port'],
}

/**
 * 账号设置弹窗里那两块（**功能开关** / **运行结果推送**）暂时**不显示**。
 *
 * 用户口径（2026-10-02）：「这些设置先取消显示，设置后面单独弄」。
 *
 * ⚠ 代码**原样留着**：里面是上游字段、取值范围与校验（重做那一版时有依据 ✓），
 * 只是**不渲染** ⇒ 要让它回来，把这里改成 `true` 就行 ✓。
 * ⚠ **别删代码** ✗ —— 用户说的是"先取消显示"，不是"不要了"。
 */
const SHOW_ACCOUNT_SETTINGS_BLOCKS = false

/** 每个渠道一句话说明（配在哪、有什么坑）。 */
const CHANNEL_HINT: Record<NotifyChannelKey, string> = {
  pushplus: 'PushPlus（微信推送）：在 pushplus.plus 拿 token，一条 token 就是一个收件人。',
  serverChan: 'Server 酱：sct.ftqq.com 拿 SendKey（SCT 开头），填进来即可。',
  workWeixinBot: '企业微信**群机器人**：群设置 → 群机器人 → 添加 → 复制 Webhook 地址。',
  dingTalk:
    '钉钉**群机器人**：群设置 → 智能群助手 → 添加机器人 → 自定义 → 拿 Webhook 里的 access_token；安全设置选了「加签」时还要填密钥。',
  customPost:
    '自定义 POST：给你自己的接口发一条 HTTP 请求。URL 里可以用 {title} / {text} 占位符。',
  workWeixin:
    '企业微信**应用消息**：要企业 ID（corpid）、应用密钥（corpsecret）与应用 ID（agentid）三样；与上面的「机器人」不是一回事。',
  tgBot:
    'Telegram Bot：@BotFather 拿 token；chat_id 是收件会话（群 id 是负数，也可以写 @channelname）。',
  bark: 'Bark（iOS 推送）：App 首页那串 key。',
  twoIm: '回逍（2Im）：key 与 sid 都要填。',
  email:
    '⚠ 本版**不会真的发信**（后端还没实现 SMTP）。配了只会存下来、不会生效 —— 摆在这里是为了让旧配置看得见、改得动。',
}

/** 推送日志级别选项。级别越小越严重，过滤规则是 `level <= max`。 */
const MIN_LEVELS: readonly { value: PushLogLevel; label: string }[] = [
  { value: 'error', label: 'error —— 只推错误（最安静）' },
  { value: 'warn', label: 'warn —— 错误 + 警告' },
  { value: 'info', label: 'info —— 默认：再带上信息与成功' },
  { value: 'debug', label: 'debug —— 全推（最吵）' },
]

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

/** 数字串转 JSON 数字；不是整数就原样保留（例如 chat_id 写 `@channelname`）。 */
const toJsonScalar = (raw: string): string | number => {
  const t = raw.trim()
  return /^-?\d+$/.test(t) && Number.isSafeInteger(Number(t)) ? Number(t) : t
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

/**
 * 把设置的读写失败翻成**能分辨**的一句话。
 *
 * ⚠ 三种状态**不能**都落到「网络错误」：
 * - **404** = 账号不存在**或**不属于当前登录用户（后端对两种情形回同一个 404，
 *   不泄露存在性）—— 用户该做的是刷新列表 / 重新登录，不是"再点一次"；
 * - **400** = 请求体被后端拒绝（未知键、越界、渠道缺字段），**后端原话必须显出来**，
 *   否则用户只看到一句无从下手的"保存失败"；
 * - **401** = 登录态失效（`request` 已经让整个应用转去登录页），这里补一句说明。
 *
 * `status === 0` 是 `request` 自己造的"超时/取消"，那时 `message` 已经是人话。
 */
const describeSettingsError = (e: unknown, action: string): string => {
  if (e instanceof ApiError) {
    if (e.status === 404) {
      return '这个账号不存在，或者不属于当前登录用户（后端对两种情形回的是同一个 404）。请刷新账号列表后再试。'
    }
    if (e.status === 401) return '登录状态已失效，请重新登录后再' + action + '设置。'
    if (e.status === 400) return '后端拒绝了这份设置：' + e.message
    if (e.status === 0) return e.message
    return action + '账号设置失败（HTTP ' + e.status + '）：' + e.message
  }
  return e instanceof Error ? e.message : action + '账号设置失败'
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

  // ── 编辑账号（弹窗） ────────────────────────────────────────────────
  //
  // 三个字段都是"给了才改"：凭据**从不回显**（服务端也不发它），所以这里
  // 永远是空的 —— 空 = 不动它。这也正是这一层用 PATCH 而不是"整体替换"的原因。
  const editing = ref<AccountSummary | null>(null)
  const editNickname = ref('')
  const editDevice = ref('')
  const editAuth = ref('')
  const editPlatform = ref<PcPlatform>('windows')
  const editError = ref('')
  const savingEdit = ref(false)

  // ── 粘贴凭据添加（添加弹窗里的一个 tab） ────────────────────────────
  const importAuth = ref('')
  const importBusy = ref(false)

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

  /** 正在启停的账号 id（按钮期间禁用，避免连点两下发两次相反的请求）。 */
  const enabling = ref<string | null>(null)

  const load = async () => {
    state.value = 'loading'
    loadError.value = ''
    try {
      // ⚠ 这里要**全部**（含停用）：账号页是唯一能启停的地方，过滤掉就没法启用回来。
      // 各处下拉用的是不带参数的 `listAccounts()`（默认只给启用的）。
      const [list, dev] = await Promise.all([
        listAccounts({ includeDisabled: true }),
        getPcDevice(),
      ])
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

  /**
   * 启停一个账号。
   *
   * # 为什么不用确认框
   *
   * **可逆**：点错了一下再点回来即可。本仓的确认框留给不可逆的动作（移除账号、
   * 重启服务）—— 给可逆动作也套一层，用户学会的是"闭眼点确认"，那道框就白设了。
   */
  const toggleEnabled = async (a: AccountSummary) => {
    const next = !a.enabled
    const who = a.nickname || a.label || a.id
    enabling.value = a.id
    try {
      await updateAccount(a.id, { enabled: next })
      pushToast(
        'success',
        next ? `已启用 ${who}` : `已停用 ${who} —— 其它页面的下拉里不再出现它`,
      )
      await load()
    } catch (e) {
      showError(
        `${next ? '无法启用' : '无法停用'} ${who}`,
        e instanceof Error ? e.message : '操作失败',
      )
    } finally {
      enabling.value = null
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

  // ── 编辑账号 ────────────────────────────────────────────────────────

  /** 打开编辑弹窗：把三个输入框预填成"当前值 / 空"。 */
  const askEdit = (a: AccountSummary) => {
    editing.value = a
    // 昵称预填是要**看见并改**的；设备号同理。凭据留空 —— 它从不回显，
    // 空 = 不改（这也是"换凭据"与"改名字"能在同一个弹窗里的原因）。
    editNickname.value = a.nickname ?? ''
    editDevice.value = a.device_id
    editAuth.value = ''
    editError.value = ''
    if (a.pc_platform) editPlatform.value = a.pc_platform
    openDialog('edit-account')
  }

  const doSaveEdit = async () => {
    const a = editing.value
    if (!a) return
    const patch: {
      nickname: string
      deviceId: string
      auth?: string
      platform?: PcPlatform
    } = {
      // 空串是**有意义**的：服务端把它读作"清空昵称"（回落成掩码手机号）。
      nickname: editNickname.value,
      deviceId: editDevice.value,
    }
    const pasted = editAuth.value.trim()
    if (pasted) {
      patch.auth = pasted
      patch.platform = editPlatform.value
    }
    savingEdit.value = true
    editError.value = ''
    try {
      await updateAccount(a.id, patch)
      closeDialog('edit-account')
      pushToast('success', `已保存 ${editNickname.value.trim() || a.label || a.id}`)
      await load()
    } catch (e) {
      // 弹窗**不关**：用户刚打的字还在里面，关掉等于让他重打一遍。
      editError.value = e instanceof Error ? e.message : '保存失败'
    } finally {
      savingEdit.value = false
    }
  }

  // ── 粘贴凭据添加 ────────────────────────────────────────────────────

  const doImport = async () => {
    const raw = importAuth.value.trim()
    if (!raw) {
      formError.value = '请先粘贴一条凭据'
      formOk.value = ''
      return
    }
    importBusy.value = true
    formError.value = ''
    formOk.value = ''
    try {
      const r = await importAccount({
        auth: raw,
        // 形态只在 pc 凭据上才有意义；服务端对 pc **要求**它、对 mobile 忽略它。
        platform: credentialPrefix(raw) === 'mobile' ? undefined : platform.value,
      })
      const name = r.account.nickname || r.account.label || r.account.id
      // 说清发生了什么：同一个手机号再次导入是"换凭据"，不是"多了一个账号"。
      pushToast('success', r.created ? `已添加 ${name}` : `已更新 ${name} 的凭据`)
      const where = r.account.pc_platform ? PLATFORM_LABEL[r.account.pc_platform] : '手机端'
      formOk.value = r.created
        ? `已添加 ${name}（${where}）`
        : `${name} 的凭据已更新（昵称与设备号保留）`
      importAuth.value = ''
      closeDialog('add-account')
      await load()
    } catch (e) {
      formError.value = e instanceof Error ? e.message : '导入失败'
    } finally {
      importBusy.value = false
    }
  }

  // ── 每账号设置（GET/PUT /api/accounts/{id}/settings）─────────────────
  //
  // # 为什么"结构"与"值"分成两个 ref
  //
  // lite 是编译期的"指哪儿改哪儿"：一个 `{expr}` 只重建**读到过变化信号**的那一小块。
  // 所以渠道列表的**结构**（有哪些卡片）读 `settingsChannels`，卡片里的**值**读
  // `settingsNotify`。合成一个对象的话，往输入框敲一个字就会让整块渠道表单重建
  // ⇒ 每敲一下就丢一次焦点。
  //
  // # 为什么渠道配置留着**原始对象**
  //
  // PUT 是整体替换。而后端给每个渠道留了 `#[serde(flatten)] extra`（TS 的语义：多余
  // 字段原样展开进请求体），那些键我们**没有建模**。只挑认识的字段回写，等于把用户
  // 没碰过的字段静默抹掉 —— 所以界面持有 GET 回来的那个对象本身，只改看得见的键。
  //
  // # 打开才拉数据
  //
  // 十条渠道 + 一堆字段，页面 `load()` 里顺手全拉一遍只会多一条失败路径（没有后端时
  // 每个账号各报一次错），而首屏一行都用不到它。所以读发生在**打开弹窗那一刻**。
  const SETTINGS_DIALOG = 'account-settings'
  const settingsAccount = ref<AccountSummary | null>(null)
  const settingsState = ref<'idle' | 'loading' | 'error' | 'ready'>('idle')
  /** 读 / 存 / 本地校验的失败原话（**后端原文优先**，见 `describeSettingsError`）。 */
  const settingsError = ref('')
  /** 保存成功回执。留在弹窗里直到关掉或下一次保存 —— 3 秒的 toast 太容易被错过。 */
  const settingsOk = ref('')
  const settingsSaving = ref(false)

  // 全局字段。数字一律以**字符串**存：原样回显用户敲的东西，解析放到提交那一刻。
  const fBackupWait = ref('')
  const fRefreshDays = ref('')
  const fSkipTasks = ref('')
  const fAiAvatar = ref(false)
  const fAiAvatarLimit = ref('')
  const fRedpack = ref(false)
  const fPlayAi = ref(false)
  const fKouling = ref(false)

  // 推送
  const fNotifyOn = ref(false)
  const fNotifyTitle = ref(DEFAULT_NOTIFY_TITLE)
  const fOnlyError = ref(false)
  const fMinLevel = ref<PushLogLevel>('info')
  /** 渠道 → 原始配置对象（GET 回来的那份，见上面"留着原始对象"）。 */
  const settingsNotify = ref<Record<string, unknown>>({})
  /** 当前显示哪几张渠道卡片。结构与值分开存，理由见上面那段。 */
  const settingsChannels = ref<NotifyChannelKey[]>([])
  /** `customPost` 每条模板的请求体（JSON 文本），下标与模板一一对应。 */
  const templateBodies = ref<string[]>([])
  /** 打码字段的「显示」开关，按字段 id 记。 */
  const shownSecrets = ref<Record<string, boolean>>({})

  /** 取一个渠道的原始配置对象（`customPost` 是数组，单独处理）。 */
  const channelCfg = (key: NotifyChannelKey): Record<string, unknown> | null => {
    const v = settingsNotify.value[key]
    return v != null && typeof v === 'object' && !Array.isArray(v)
      ? (v as Record<string, unknown>)
      : null
  }

  /** 读渠道配置里的一个字段，按**文本**给输入框（数字也照原样）。 */
  const cfgText = (key: NotifyChannelKey, field: string): string => {
    const v = channelCfg(key)?.[field]
    return v == null ? '' : String(v)
  }

  /**
   * 改渠道配置里的一个字段。
   *
   * 空串 ⇒ **删掉这一格**：`""` 会被后端当成"填了但为空"（`require_non_empty` ⇒ 400），
   * 而用户的意思只是"这一格不填"。删掉它，可选字段才是真的缺席。
   */
  const setChannelField = (key: NotifyChannelKey, field: string, value: string): void => {
    const next: Record<string, unknown> = { ...(channelCfg(key) ?? {}) }
    if (value.trim() === '') delete next[field]
    else next[field] = value
    settingsNotify.value = { ...settingsNotify.value, [key]: next }
  }

  /** 回逍那两格在 `query` 里；没有别的键时把空的 `query` 删掉，别留一个空对象。 */
  const setTwoImQuery = (field: string, value: string): void => {
    const cfg: Record<string, unknown> = { ...(channelCfg('twoIm') ?? {}) }
    const q: Record<string, unknown> = { ...((cfg.query as Record<string, unknown>) ?? {}) }
    if (value.trim() === '') delete q[field]
    else q[field] = value
    if (Object.keys(q).length === 0) delete cfg.query
    else cfg.query = q
    settingsNotify.value = { ...settingsNotify.value, twoIm: cfg }
  }

  const twoImQueryText = (field: string): string => {
    const q = channelCfg('twoIm')?.query as Record<string, unknown> | undefined
    const v = q?.[field]
    return v == null ? '' : String(v)
  }

  /** `customPost` 允许单个模板或数组；界面统一按数组处理。 */
  const templateList = (): Record<string, unknown>[] => {
    const v = settingsNotify.value.customPost
    if (v == null) return []
    return Array.isArray(v) ? (v as Record<string, unknown>[]) : [v as Record<string, unknown>]
  }

  /**
   * 按**下标**迭代模板。
   *
   * ⚠ 列表项刻意是**数字**而不是模板对象：`createFor` 判"要不要重建这一行"用的是
   * 对象身份。改一个字段就是一次 `{...tpl}`，新对象 ⇒ 整行重建 ⇒ 输入框丢焦点。
   * 数字按下标比是同一份值，行被复用，只有字段绑定重跑。
   */
  const templateIndexes = (): number[] => templateList().map((_tpl, i) => i)

  const templateText = (index: number, field: string): string => {
    const v = templateList()[index]?.[field]
    return v == null ? '' : String(v)
  }

  const setTemplateField = (index: number, field: string, value: string): void => {
    const items = templateList().map((tpl, i) => {
      if (i !== index) return tpl
      const next: Record<string, unknown> = { ...tpl }
      if (value.trim() === '') delete next[field]
      else next[field] = value
      return next
    })
    settingsNotify.value = { ...settingsNotify.value, customPost: items }
  }

  /**
   * 改一条模板的请求体文本。
   *
   * ⚠ 刻意**不**写成 `templateBodies.value.map(...)`：两个数组（模板与请求体）
   * 一旦长度不一致，`map` 到不了那个下标 ⇒ 赋值出来还是"没有这一格" ⇒ 紧接着
   * 那条 `value={templateBodies.value[i] ?? ''}` 的绑定把用户刚敲的字**回写成空串**。
   * 实测踩到过一次（`addChannel` 漏了配对），症状是"打字打不进去"。
   * 这里先补齐长度，让"数组短了"不可能变成"输入框自己变空"。
   */
  const setTemplateBody = (index: number, text: string): void => {
    const next = [...templateBodies.value]
    while (next.length <= index) next.push('')
    next[index] = text
    templateBodies.value = next
  }

  /** 模板体在界面上是一段 JSON 文本；`data` 与 `body` 在后端是 `data || body`。 */
  const bodyTextOf = (tpl: Record<string, unknown>): string => {
    const v = tpl.data ?? tpl.body
    return v == null ? '' : JSON.stringify(v, null, 2)
  }

  const addTemplate = (): void => {
    const list = templateList()
    // 请求体数组与模板数组**必须等长**（见 `setTemplateBody` 的 ⚠）：先按模板对齐一份，
    // 再一起加一条，否则新加的那条从一出生就没有自己的请求体格子。
    const bodies = list.map((_tpl, i) => templateBodies.value[i] ?? '')
    settingsNotify.value = { ...settingsNotify.value, customPost: [...list, {}] }
    templateBodies.value = [...bodies, '']
  }

  const removeTemplate = (index: number): void => {
    settingsNotify.value = {
      ...settingsNotify.value,
      customPost: templateList().filter((_tpl, i) => i !== index),
    }
    templateBodies.value = templateBodies.value.filter((_t, i) => i !== index)
  }

  const addChannel = (key: NotifyChannelKey): void => {
    if (!settingsChannels.value.includes(key)) {
      settingsChannels.value = [...settingsChannels.value, key]
    }
    if (settingsNotify.value[key] == null) {
      settingsNotify.value = { ...settingsNotify.value, [key]: {} }
    }
    // `customPost` 的空数组会被后端判成"一个模板都没有"（⇒ 400）。新加的直接给一条
    // 空模板 —— 用户看到的是"该填 URL 了"，而不是"配了却被拒绝"。
    //
    // ⚠ 两个数组（模板 / 请求体文本）**必须同时就位**。上面那两行会把
    // `customPost` 设成 `{}`，而 `templateList()` 把它当**单模板**包成 `[{}]` ——
    // 所以"长度是 0 才补请求体"那个写法永远进不去，请求体数组留在空表上，
    // 结果就是"URL 打得进去、模板体那边一敲字就被回写成空串"。实测踩到过。
    if (key === 'customPost') {
      const list = templateList()
      if (list.length === 0) {
        settingsNotify.value = { ...settingsNotify.value, customPost: [{}] }
        templateBodies.value = ['']
      } else {
        templateBodies.value = list.map((_tpl, i) => templateBodies.value[i] ?? '')
      }
    }
  }

  const removeChannel = (key: NotifyChannelKey): void => {
    settingsChannels.value = settingsChannels.value.filter((k) => k !== key)
    const next = { ...settingsNotify.value }
    delete next[key]
    settingsNotify.value = next
    if (key === 'customPost') templateBodies.value = []
  }

  const availableChannels = (): NotifyChannelKey[] =>
    NOTIFY_CHANNELS.filter((k) => !settingsChannels.value.includes(k))

  /** 卡片标题旁那枚徽章：只看**必填字段填没填**（与提交时"算不算配了"同一条判据）。 */
  const channelState = (key: NotifyChannelKey): string => {
    if (key === 'customPost') {
      const n = templateList().filter((t) => String(t.url ?? '').trim() !== '').length
      return n === 0 ? '未填' : String(n) + ' 条模板'
    }
    const cfg = channelCfg(key)
    if (!cfg) return '未填'
    const missing = CHANNEL_REQUIRED[key].filter((f) => String(cfg[f] ?? '').trim() === '')
    return missing.length === 0 ? '已填好' : '缺 ' + missing.join('、')
  }

  const secretShown = (id: string): boolean => shownSecrets.value[id] === true

  const toggleSecret = (id: string): void => {
    shownSecrets.value = { ...shownSecrets.value, [id]: !secretShown(id) }
  }

  /**
   * 把 GET / PUT 回来的一份设置铺进表单。
   *
   * 渠道配置**原样拿住**（里面可能有我们没建模的键），另挑一份"有哪些渠道"给结构性 ref。
   */
  const applySettings = (s: AccountSettings): void => {
    // ⚠ 逐格容错：GET / PUT 回的应当是完整的 DTO，但真出现形状缺口时，
    // 一条 `Cannot read properties of undefined` 会把"哪一格不对"整个盖掉。
    // 缺数字 ⇒ 留空（提交时那条"不能留空"才是能读的错误），缺数组 ⇒ 空表。
    fBackupWait.value = s.backupWaitSecs == null ? '' : String(s.backupWaitSecs)
    fRefreshDays.value = s.refreshTokenDays == null ? '' : String(s.refreshTokenDays)
    fSkipTasks.value = (s.skipTasks ?? []).join(', ')
    fAiAvatar.value = s.aiAvatarEnabled === true
    fAiAvatarLimit.value = s.aiAvatarDailyLimit == null ? '' : String(s.aiAvatarDailyLimit)
    fRedpack.value = s.redpackEnabled
    fPlayAi.value = s.playAiEnabled
    fKouling.value = s.koulingEnabled

    const n = s.notify
    fNotifyOn.value = n != null
    fNotifyTitle.value = n?.title ?? DEFAULT_NOTIFY_TITLE
    fOnlyError.value = n?.onlyError ?? false
    fMinLevel.value = n?.minLevel ?? 'info'

    const raw = (n ?? {}) as Record<string, unknown>
    const next: Record<string, unknown> = {}
    const keys: NotifyChannelKey[] = []
    for (const key of NOTIFY_CHANNELS) {
      const v = raw[key]
      if (v == null) continue
      next[key] = v
      keys.push(key)
    }
    settingsNotify.value = next
    settingsChannels.value = keys
    templateBodies.value = templateList().map(bodyTextOf)
    shownSecrets.value = {}
  }

  const openSettings = (a: AccountSummary): void => {
    settingsAccount.value = a
    settingsState.value = 'loading'
    settingsError.value = ''
    settingsOk.value = ''
    // ⚠ 先清空：上一个账号的渠道绝不能在下一次 GET 回来之前留在界面上
    // —— 那会让人以为"这个账号也配了这些"，照着按保存就把别人的配置套过来了。
    settingsNotify.value = {}
    settingsChannels.value = []
    templateBodies.value = []
    shownSecrets.value = {}
    openDialog(SETTINGS_DIALOG)
    void loadSettings(a)
  }

  const loadSettings = async (a: AccountSummary): Promise<void> => {
    settingsState.value = 'loading'
    settingsError.value = ''
    try {
      applySettings(await getAccountSettings(a.id))
      settingsState.value = 'ready'
    } catch (e) {
      settingsError.value = describeSettingsError(e, '读取')
      settingsState.value = 'error'
    }
  }

  const closeSettings = (): void => {
    // 换一个账号再打开时不该看到上一条回执/错误 —— 关掉就清干净。
    settingsOk.value = ''
    settingsError.value = ''
    settingsState.value = 'idle'
    settingsAccount.value = null
  }

  /**
   * 把一条渠道配置整理成要发出去的形状；返回 `null` = 这个渠道**没配**、整条不发。
   *
   * 三条规矩：
   * - 必填字段**全空** ⇒ `null`（不发空对象）；
   * - 数字字段转 JSON 数字（见 `CHANNEL_NUMERIC`）；
   * - `customPost` 的模板体在这里解析 JSON，解不开就**指名是第几条**。
   */
  const buildChannel = (key: NotifyChannelKey): unknown => {
    if (key === 'customPost') {
      const items = templateList()
        .map((tpl, i) => {
          const next: Record<string, unknown> = { ...tpl }
          const rawBody = (templateBodies.value[i] ?? '').trim()
          delete next.data
          delete next.body
          if (rawBody !== '') {
            let parsed: unknown
            try {
              parsed = JSON.parse(rawBody)
            } catch {
              throw new Error('自定义 POST 第 ' + (i + 1) + ' 条的模板体不是合法 JSON')
            }
            if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
              throw new Error('自定义 POST 第 ' + (i + 1) + ' 条的模板体必须是一个 JSON 对象')
            }
            next.data = parsed as Record<string, unknown>
          }
          return next
        })
        .filter((t) => String(t.url ?? '').trim() !== '')
      return items.length > 0 ? items : null
    }

    const cfg = channelCfg(key)
    if (!cfg) return null
    const next: Record<string, unknown> = { ...cfg }
    for (const field of CHANNEL_NUMERIC[key] ?? []) {
      const cur = next[field]
      if (cur == null || String(cur).trim() === '') delete next[field]
      else next[field] = toJsonScalar(String(cur))
    }
    if (CHANNEL_REQUIRED[key].every((f) => String(next[f] ?? '').trim() === '')) return null
    return next
  }

  const buildNotifyPayload = (): PushConfig | null => {
    if (!fNotifyOn.value) return null
    const out: Record<string, unknown> = {
      title: fNotifyTitle.value.trim() === '' ? DEFAULT_NOTIFY_TITLE : fNotifyTitle.value.trim(),
      onlyError: fOnlyError.value,
      minLevel: fMinLevel.value,
    }
    let count = 0
    for (const key of settingsChannels.value) {
      const built = buildChannel(key)
      if (built == null) continue
      out[key] = built
      count++
    }
    if (count === 0) {
      // 后端对这份形状的原话是「notify 里没有任何渠道（全为 null）」⇒ 400。
      // 与其打一枪再回错误，不如在这里说同一件事（判据与后端一致，只是省一次往返）。
      throw new Error(
        '推送总开关开着，但一个渠道都没配。要么至少配一个，要么关掉总开关（关掉 = 这个账号不推送）。',
      )
    }
    return out as unknown as PushConfig
  }

  const buildSettings = (): AccountSettings => ({
    backupWaitSecs: parseIntField('备份等待秒数', fBackupWait.value),
    refreshTokenDays: parseIntField('刷新阈值天数', fRefreshDays.value),
    skipTasks: parseSkipTasks(fSkipTasks.value),
    aiAvatarEnabled: fAiAvatar.value,
    aiAvatarDailyLimit: parseIntField('AI 头像每日上限', fAiAvatarLimit.value),
    redpackEnabled: fRedpack.value,
    playAiEnabled: fPlayAi.value,
    koulingEnabled: fKouling.value,
    notify: buildNotifyPayload(),
  })

  const saveSettings = async (): Promise<void> => {
    const target = settingsAccount.value
    if (!target) return
    settingsError.value = ''
    settingsOk.value = ''
    let payload: AccountSettings
    try {
      payload = buildSettings()
    } catch (e) {
      // 本地能判的（空、不是整数、JSON 坏了）当场说，**范围**仍然留给后端。
      settingsError.value = e instanceof Error ? e.message : '设置不完整'
      return
    }
    settingsSaving.value = true
    try {
      // 用**后端回读的那一份**回填：整体替换的语义下，"我以为存进去的"不算数。
      applySettings(await putAccountSettings(target.id, payload))
      settingsOk.value = (target.nickname || target.label || target.id) + ' 的设置已保存'
      pushToast('success', '账号设置已保存')
    } catch (e) {
      settingsError.value = describeSettingsError(e, '保存')
    } finally {
      settingsSaving.value = false
    }
  }

  // ── 每账号设置的渲染 ────────────────────────────────────────────────
  //
  // 下面几个 `*Field` 都是**普通函数**（不是组件），并且**值一律走 thunk**
  // （`get: () => string`）。两条都不是风格问题：
  // - 普通函数被 `{channelFields(key)}` 这样调用时，调用本身发生在一条 effect 里；
  //   若在这里**当场**读出值（`const v = cfgText(...)`）再当字符串传下去，那条
  //   effect 就订阅了 `settingsNotify` —— 敲一个字整块表单重建、焦点全丢。
  //   写成 `value={get()}` 时读发生在**属性自己的 effect** 里，只更新那一个输入框。
  // - 组件不行：组件的 props 是在组件体里读的，而组件体只跑一次、不在任何 effect 里，
  //   读到的值不会更新（本框架没有虚拟 DOM 的那层 diff）。

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
   * 一行**打码**输入框（token / secret / 带密钥的 webhook URL）。
   *
   * 默认 `type=password`，右边那颗「显示」按字段 id 记住是否明文 —— 打码防的是
   * **站在屏幕边上的人**，不是自己，所以查看必须一键可达。
   *
   * ⚠ 这些值**不进 console**：本文件里没有任何 `console.*`，报错也只带字段名
   * （后端 `validate` 就是这么约束的：错误信息里不许出现字段值）。
   */
  const secretField = (
    id: string,
    label: string,
    get: () => string,
    set: (v: string) => void,
    hint: string,
  ) => (
    <fieldset class="fieldset">
      <legend class="fieldset-legend">{label}</legend>
      <div class="join w-full">
        <input
          type={secretShown(id) ? 'text' : 'password'}
          class="input join-item w-full font-mono text-xs"
          autocomplete="off"
          spellcheck={false}
          value={get()}
          onInput={(e) => set((e.target as HTMLInputElement).value)}
        />
        <button
          type="button"
          class="btn join-item btn-soft"
          aria-pressed={secretShown(id)}
          onClick={() => toggleSecret(id)}
        >
          {secretShown(id) ? '隐藏' : '显示'}
        </button>
      </div>
      <p class="text-xs leading-5 opacity-60">{hint}</p>
    </fieldset>
  )

  /** 一个开关（daisyUI `toggle`）。 */
  const switchField = (
    label: string,
    get: () => boolean,
    set: (v: boolean) => void,
    hint: string,
  ) => (
    <label class="flex cursor-pointer flex-col gap-1">
      <span class="flex min-h-11 items-center gap-3 md:min-h-0">
        <input
          type="checkbox"
          class="toggle toggle-primary"
          checked={get()}
          onChange={(e) => set((e.target as HTMLInputElement).checked)}
        />
        <span class="text-sm">{label}</span>
      </span>
      <span class="text-xs leading-5 opacity-60">{hint}</span>
    </label>
  )

  /**
   * 一个渠道的具体字段。
   *
   * ⚠ 分支**只看 `key`**（普通字符串），一个 ref 都不当场读 —— 所以这块结构建一次
   * 就定了，改字段不会重建它（见上面那段"值一律走 thunk"）。
   */
  const channelFields = (key: NotifyChannelKey) => {
    const get = (field: string) => () => cfgText(key, field)
    const set = (field: string) => (v: string) => setChannelField(key, field, v)

    if (key === 'pushplus' || key === 'serverChan') {
      return secretField(
        key + '.token',
        'Token',
        get('token'),
        set('token'),
        '在对应服务里生成的推送 token。留空 = 这个渠道不发。',
      )
    }
    if (key === 'workWeixinBot') {
      return (
        <>
          {secretField(
            'workWeixinBot.url',
            'Webhook URL',
            get('url'),
            set('url'),
            '群设置 → 群机器人 → 添加 → 复制 Webhook 地址。⚠ 这个 URL 本身就是密钥：拿到它就能往群里发消息。',
          )}
          {textField('消息类型 msgtype（可留空）', get('msgtype'), set('msgtype'), '留空 = 后端按 text 发。')}
        </>
      )
    }
    if (key === 'dingTalk') {
      return (
        <>
          {secretField(
            'dingTalk.token',
            'Access Token',
            get('token'),
            set('token'),
            'Webhook 地址里 access_token= 后面那一段。',
          )}
          {secretField(
            'dingTalk.secret',
            '加签密钥（可留空）',
            get('secret'),
            set('secret'),
            '只有机器人安全设置里选了「加签」才需要。',
          )}
        </>
      )
    }
    if (key === 'customPost') {
      return (
        <div class="flex flex-col gap-2">
          {templateIndexes().map((i) => (
            <div key={i} class="flex flex-col gap-2 rounded-box border border-base-300 p-3">
              {secretField(
                'customPost.' + i + '.url',
                '第 ' + (i + 1) + ' 条 URL',
                () => templateText(i, 'url'),
                (v) => setTemplateField(i, 'url', v),
                '支持 {title} / {text} 占位符。⚠ URL 里带密钥时要按密钥对待。',
              )}
              {textField(
                '第 ' + (i + 1) + ' 条方法（可留空）',
                () => templateText(i, 'method'),
                (v) => setTemplateField(i, 'method', v),
                '留空 = POST；后端会把它转成大写。GET 也允许。',
              )}
              <fieldset class="fieldset">
                <legend class="fieldset-legend">{'第 ' + (i + 1) + ' 条请求体模板（JSON，可留空）'}</legend>
                <textarea
                  class="textarea w-full font-mono text-xs"
                  rows={3}
                  value={templateBodies.value[i] ?? ''}
                  onInput={(e) => setTemplateBody(i, (e.target as HTMLTextAreaElement).value)}
                />
                <p class="text-xs leading-5 opacity-60">
                  必须是 JSON **对象**。里面的 {'{title}'} / {'{text}'} 会被替换（不转义）。
                  留空 = 不发模板体。⚠ headers / timeout 等本版没有列出来的字段会原样保留。
                </p>
              </fieldset>
              <button
                type="button"
                class="btn btn-sm btn-soft min-h-11 self-start md:min-h-0"
                onClick={() => removeTemplate(i)}
              >
                删掉这条
              </button>
            </div>
          ))}
          <button
            type="button"
            class="btn btn-sm btn-soft self-start min-h-11 md:min-h-0"
            onClick={addTemplate}
          >
            再加一条模板
          </button>
        </div>
      )
    }
    if (key === 'workWeixin') {
      return (
        <>
          {textField(
            'corpid（企业 ID）',
            get('corpid'),
            set('corpid'),
            '企业微信后台 → 我的企业 → 企业 ID。',
          )}
          {secretField(
            'workWeixin.corpsecret',
            'corpsecret（应用密钥）',
            get('corpsecret'),
            set('corpsecret'),
            '企业微信后台 → 应用管理 → 你的应用 → Secret。',
          )}
          {textField(
            'agentid（应用 ID）',
            get('agentid'),
            set('agentid'),
            '必须是数字；缺了发送侧会整条跳过。',
            'number',
          )}
          {textField('touser（可留空）', get('touser'), set('touser'), '收件人，留空 = 后端默认 @all。')}
        </>
      )
    }
    if (key === 'tgBot') {
      return (
        <>
          {secretField('tgBot.token', 'Bot Token', get('token'), set('token'), '@BotFather 给的 token。')}
          {textField(
            'chat_id',
            get('chat_id'),
            set('chat_id'),
            '收件会话：私聊是数字，群是负数，也可以写 @channelname。',
          )}
          {textField(
            'apiHost（可留空）',
            get('apiHost'),
            set('apiHost'),
            '自建反代时才填；留空 = 官方 api.telegram.org。',
          )}
          {textField('proxy（可留空）', get('proxy'), set('proxy'), '形如 http://127.0.0.1:7890。')}
        </>
      )
    }
    if (key === 'bark') {
      return (
        <>
          {secretField('bark.key', 'Key', get('key'), set('key'), 'Bark App 首页那串 key。')}
          {textField(
            'level（可留空）',
            get('level'),
            set('level'),
            '常见取值 active / timeSensitive / passive / critical；留空 = 后端默认。',
          )}
        </>
      )
    }
    if (key === 'twoIm') {
      return (
        <>
          {secretField('twoIm.key', 'Key', get('key'), set('key'), '回逍后台的 key。')}
          {secretField('twoIm.sid', 'Sid', get('sid'), set('sid'), '回逍后台的 sid。')}
          {textField('msgtype（可留空）', get('msgtype'), set('msgtype'), '留空 = 后端默认。')}
          {textField(
            '分组 group（可留空）',
            () => twoImQueryText('group'),
            (v) => setTwoImQuery('group', v),
            '回逍的群组标识；留空 = 不按分组发。',
          )}
        </>
      )
    }
    if (key === 'email') {
      return (
        <>
          {textField('SMTP 主机', get('host'), set('host'), '例如 smtp.qq.com。')}
          {textField('端口（可留空）', get('port'), set('port'), '必须是数字；留空 = 后端默认。', 'number')}
          {textField('发件人 from', get('from'), set('from'), '例如 you@qq.com。')}
          {secretField(
            'email.pass',
            'SMTP 口令 / 授权码',
            get('pass'),
            set('pass'),
            '多数邮箱要的是「授权码」，不是登录口令。',
          )}
          {textField('收件人 to（可留空）', get('to'), set('to'), '留空 = 发给 from 自己。')}
        </>
      )
    }
    // 十个渠道都在上面的表里。走到这里说明加渠道时漏了一档 ——
    // 宁可把话说出来，也别静默渲染成一块空白。
    return <p class="text-xs text-error">这个渠道还没有表单（代码里漏了一档）。</p>
  }

  /** 推送那一块（总开关之外的全部内容）。 */
  const notifyFields = () => (
    <>
      {textField(
        '推送标题',
        () => fNotifyTitle.value,
        (v) => (fNotifyTitle.value = v),
        '留空 = 用默认「mcloud 运行推送」。',
      )}
      {switchField(
        '只在出错时推',
        () => fOnlyError.value,
        (v) => (fOnlyError.value = v),
        '开了之后只有 error 类型的日志会推 —— fail / fatal 不算（后端判的就是类型恰好等于 error）。',
      )}

      <fieldset class="fieldset">
        <legend class="fieldset-legend">最低日志级别</legend>
        <Select
          id="push-min-level"
          class="w-full"
          ariaLabel="最低日志级别"
          value={fMinLevel.value}
          options={MIN_LEVELS.map((lv) => ({ value: lv.value, label: lv.label }))}
          onChange={(v) => (fMinLevel.value = v as PushLogLevel)}
        />
        <p class="text-xs leading-5 opacity-60">级别越小越严重：error 只留错误，debug 全留。</p>
      </fieldset>

      <div class="flex flex-col gap-2">
        <span class="text-xs font-medium opacity-70">渠道（点开一个才能改它）</span>
        <div class="flex flex-col gap-2">
          {settingsChannels.value.map((key) => (
            <details key={key} class="collapse collapse-arrow bg-base-200">
              <summary class="collapse-title text-sm font-medium">
                {CHANNEL_LABEL[key]}
                <span class="badge badge-soft badge-sm ml-2">{channelState(key)}</span>
              </summary>
              <div class="collapse-content flex flex-col gap-2">
                <p class="text-xs leading-5 opacity-60">{CHANNEL_HINT[key]}</p>
                {channelFields(key)}
                <button
                  type="button"
                  class="btn btn-sm btn-soft btn-error min-h-11 self-start md:min-h-0"
                  onClick={() => removeChannel(key)}
                >
                  删除这个渠道
                </button>
              </div>
            </details>
          ))}
        </div>
        {availableChannels().length > 0 ? (
          <div class="flex flex-wrap items-center gap-1">
            <span class="text-xs opacity-70">+ 加渠道：</span>
            {availableChannels().map((key) => (
              <button
                key={key}
                type="button"
                class="btn btn-xs btn-soft min-h-9"
                onClick={() => addChannel(key)}
              >
                {CHANNEL_LABEL[key]}
              </button>
            ))}
          </div>
        ) : (
          <p class="text-xs opacity-60">十个渠道都已经在列表里了。</p>
        )}
      </div>
    </>
  )

  const settingsForm = () => (
    <>
      {settingsOk.value ? (
        <div class="alert alert-soft alert-success text-sm" role="status">
          <span class="break-words">{settingsOk.value}</span>
        </div>
      ) : null}

      {settingsError.value ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{settingsError.value}</span>
        </div>
      ) : null}

      <div class="flex flex-col gap-2">
        <h4 class="text-sm font-medium">任务参数</h4>
        {textField(
          '备份任务等待时间（秒）',
          () => fBackupWait.value,
          (v) => (fBackupWait.value = v),
          '范围 0–3600（后端上限）：备份事件发出后等它落库的秒数，0 = 不等。',
          'number',
        )}
        {textField(
          '刷新凭据的剩余天数阈值',
          () => fRefreshDays.value,
          (v) => (fRefreshDays.value = v),
          '范围 0–3650（后端上限）：凭据剩余天数低于它就刷新。',
          'number',
        )}
        {textField(
          '跳过的任务 id',
          () => fSkipTasks.value,
          (v) => (fSkipTasks.value = v),
          '填任务 id（数字），用逗号或空格分隔，最多 1000 条；留空 = 一个都不跳。⚠ 旧版本存的是任务名，后端读旧库时会丢弃它们。',
        )}
      </div>

      {/*
        ⚠ 这两块**暂时不显示** —— 开关与理由见文件头那个 `SHOW_ACCOUNT_SETTINGS_BLOCKS` ✓。
        包成 `<>` 是因为三元的分支只认"一个 JSX 元素或 null" ✓（lite 编译器的边界）。
      */}
      {SHOW_ACCOUNT_SETTINGS_BLOCKS ? (
        <>
        <div class="flex flex-col gap-2">
          <h4 class="text-sm font-medium">功能开关</h4>
          {switchField(
            'AI 新头像',
            () => fAiAvatar.value,
            (v) => (fAiAvatar.value = v),
            '打开后每天给账号换一批 AI 头像。',
          )}
          {textField(
            'AI 头像每日上限（轮）',
            () => fAiAvatarLimit.value,
            (v) => (fAiAvatarLimit.value = v),
            '范围 0–1000（后端上限）：每一轮都是一次上游调用。',
            'number',
          )}
          {switchField(
            '红包派对',
            () => fRedpack.value,
            (v) => (fRedpack.value = v),
            '参与红包派对活动。',
          )}
          {switchField(
            '春日拍拍大作战',
            () => fPlayAi.value,
            (v) => (fPlayAi.value = v),
            '参与拍拍大作战活动。',
          )}
          {switchField(
            '直播口令',
            () => fKouling.value,
            (v) => (fKouling.value = v),
            '参与直播口令领小红花。',
          )}
        </div>

        <div class="flex flex-col gap-2">
          <h4 class="text-sm font-medium">运行结果推送</h4>
          {switchField(
            '启用推送',
            () => fNotifyOn.value,
            (v) => (fNotifyOn.value = v),
            '关掉 = 这个账号一个渠道都不推（保存时 notify 写 null，会清掉下面的渠道配置）。本仓没有进程级兜底，每个账号各配各的。',
          )}
          {fNotifyOn.value ? (
            notifyFields()
          ) : (
            <p class="text-xs leading-5 opacity-60">
              当前不推送任何渠道。打开上面的开关才会出现渠道配置。
            </p>
          )}
        </div>
        </>
      ) : null}

      <div class="modal-action">
        <button type="button" class="btn btn-soft" onClick={() => closeDialog(SETTINGS_DIALOG)}>
          关闭
        </button>
        <button
          type="button"
          class="btn btn-primary"
          disabled={settingsSaving.value}
          onClick={() => void saveSettings()}
        >
          {settingsSaving.value ? '保存中…' : '保存设置'}
        </button>
      </div>
    </>
  )

  /**
   * 弹窗正文：加载中 / 读失败（带重试）/ 表单。
   *
   * ⚠ 读失败的状态里**不关**弹窗：`settingsError` 里那句（404 / 401 / 400 / 网络）
   * 正是用户要读的东西，关掉它等于把唯一的线索扔掉。
   */
  const settingsBody = () => {
    if (settingsState.value === 'error') {
      return (
        <div class="flex flex-col gap-3">
          <div class="alert alert-soft alert-error text-sm" role="alert">
            <span class="break-words">{settingsError.value}</span>
          </div>
          <button
            type="button"
            class="btn btn-soft self-start"
            onClick={() => {
              const a = settingsAccount.value
              if (a) void loadSettings(a)
            }}
          >
            重试
          </button>
        </div>
      )
    }
    if (settingsState.value !== 'ready') {
      return (
        <div class="flex justify-center py-8">
          <span class="loading loading-lg text-primary" />
        </div>
      )
    }
    return settingsForm()
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

  /**
   * 添加弹窗里要不要显示「客户端形态」。
   *
   * - 手机端（App 登录）没有这个概念；
   * - 粘贴凭据时看**凭据自带的**前缀：`mobile` 的不需要，`pc`（或看不出前缀）
   *   的需要 —— 服务端对 `pc` 凭据**要求**它，不显示会让提交必然失败。
   */
  const needPlatform = () => {
    if (method.value === 'app') return false
    if (method.value === 'auth') return credentialPrefix(importAuth.value) !== 'mobile'
    return true
  }

  /**
   * 「客户端形态」选择器。
   *
   * ⚠ 它渲染在**各个方式自己的面板里**，而不是飘在 tab 栏上面（2026-10-01 用户
   * 用箭头画出来的正是这层归属）：挂在上面时它看起来像"管所有方式"，
   * 可它对手机端无意义、对 `mobile` 凭据也无意义 —— 一个会凭空消失的全局控件，
   * 比一个出现在该出现的地方的控件难理解得多。
   */
  const platformPicker = () => (
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
        这会随账号存进 <code>accounts.json</code>，之后每次请求的 UA、设备串与渠道号
        都按它来。选错的话，账号以后一直用错的身份发请求。
      </p>
    </div>
  )

  /** 手机端（App）没有客户端形态这一说：摆一个选不了或选了没用的开关只会让人以为它有意义。 */
  const appPlatformNote = () => (
    <div class="alert alert-soft text-sm" role="status">
      <span>
        手机端登录用 App 自己的身份（<code>mobile</code> 前缀）与设备头，
        不存在 Windows / macOS 之分，所以这里不需要选客户端形态。
      </span>
    </div>
  )

  /** 凭据自带 `mobile` 前缀时，形态同样无从可选（也不需要选）。 */
  const mobileCredentialNote = () => (
    <p class="text-xs opacity-70">
      这条凭据是 <code>mobile</code> 前缀（App 端身份），不需要客户端形态。
    </p>
  )

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h1 class="text-base font-medium">账号</h1>
        <div class="flex items-center gap-2">
          <button
            type="button"
            class="btn btn-sm btn-soft min-h-11 md:min-h-0"
            disabled={state.value === 'loading'}
            onClick={() => void load()}
          >
            刷新
          </button>
          {/*
            添加账号收进弹窗：四条登录路径（扫码/短信/手机端/账密）加表单本身有
            800 多行，常驻在页面上会把"我现在有哪几个账号"压到第一屏之外 ——
            而那一句话才是这一页的主语。表单在弹窗里、宽度也够。
          */}
          <button
            type="button"
            class="btn btn-sm btn-primary min-h-11 md:min-h-0"
            onClick={() => openDialog('add-account')}
          >
            添加账号
          </button>
        </div>
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
          hint="点右上角「添加账号」，扫码、短信、账密或用一条凭据都行。"
        />
      ) : null}

      {accounts.value.length > 0 ? (
        <ul class="flex flex-col gap-2">
          {accounts.value.map((a) => (
            // 停用的整张压暗：一眼扫过去就知道"这个现在不参与"
            <li class={`card card-border bg-base-100${a.enabled ? '' : ' opacity-60'}`}>
              {/*
                窄屏（<md）**纵向两段**：名字 + 身份徽章 / 设备与到期 / 动作行。
                ⚠ 这是踩过的坑（2026-10-01 用户截图为证）：原来是
                `flex-row flex-wrap` + 文字列 `min-w-0 flex-1`，窄屏上四个控件
                **不肯换行** —— 文字列的 flex-basis 是 0，可以被压到 40px 宽，
                于是「设备 B58A…-ENDIN · 凭据 27 天后」被挤成**一列竖排单字**，
                完全没法读。⇒ 窄屏改成列方向；动作那一行等分铺满（44px 触控高度）。
              */}
              <div class="card-body gap-3 p-3 md:flex-row md:flex-wrap md:items-center">
                <div class="flex w-full min-w-0 flex-col md:w-auto md:flex-1">
                  <div class="flex min-w-0 items-center gap-2">
                    <span class="min-w-0 truncate font-medium">{a.nickname || a.label || a.id}</span>
                    {/* 身份必须显示：它决定这个账号以后每次请求用哪套头和 UA。
                        `pc_platform` 为空 = App 端登录（或更早的历史数据）。
                        窄屏把它放在名字旁边（动作行留给按钮），宽屏本来也在这一侧。 */}
                    {a.enabled ? null : (
                      <span
                        class="badge badge-soft badge-warning badge-sm shrink-0"
                        title="已停用：各处挑账号的下拉里不再出现它（已有定时规则照旧跑）"
                      >
                        已停用
                      </span>
                    )}
                    {a.pc_platform ? (
                      <span class="badge badge-soft badge-primary badge-sm shrink-0">
                        {PLATFORM_LABEL[a.pc_platform]}
                      </span>
                    ) : (
                      <span
                        class="badge badge-soft badge-sm shrink-0"
                        title="App（手机端）登录的账号：用 mobile 前缀与 App 设备头"
                      >
                        手机端
                      </span>
                    )}
                  </div>
                  {/* 设备号是 32 位十六进制加后缀：`break-all` 不能省，
                      否则它自己一条就能把窄屏顶出横向滚动条。 */}
                  <span class="break-all text-xs opacity-70">
                    设备 {a.device_id} · 凭据 {expireText(a.expire)}
                  </span>
                </div>
                {/*
                  编辑 / 设置 / 停用在前、移除在后：都是"对这个账号做点事"，
                  而移除不可逆 —— 让它留在最外侧（远离视线落点）。
                  停用**可逆**（同一颗按钮再点回来），所以它不需要确认框。
                  ⚠ **不要给这几个按钮加 `flex-1`**（2026-10-01 用户口径：
                  "这种按钮对于移动端来说太大了"）：拉满整行会变成一条压在卡片上的
                  按钮带，而本仓其它页面（定时/编排/历史）全是**内容宽度**的
                  `btn-sm min-h-11 md:min-h-0`。窄屏靠右排一行正合适，
                  触控高度仍是 44（§6.1 那条不变）。
                */}
                <div class="flex w-full items-center justify-end gap-2 md:w-auto">
                  <button
                    type="button"
                    class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                    onClick={() => askEdit(a)}
                  >
                    编辑
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                    onClick={() => openSettings(a)}
                  >
                    设置
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-soft min-h-11 md:min-h-0"
                    disabled={enabling.value === a.id}
                    onClick={() => void toggleEnabled(a)}
                  >
                    {a.enabled ? '停用' : '启用'}
                  </button>
                  <button
                    type="button"
                    class="btn btn-sm btn-soft btn-error min-h-11 md:min-h-0"
                    onClick={() => askDelete(a)}
                  >
                    移除
                  </button>
                </div>
              </div>
            </li>
          ))}
        </ul>
      ) : null}

      {/*
        添加账号（弹窗）。
        四条登录路径加各自表单有 300 多行，常驻会把"我现在有哪几个账号"压到
        第一屏之外 —— 而那一句才是这一页的主语。弹窗里宽度也够放表单。
      */}
      <Dialog id="add-account" title="添加账号">
        <div class="flex flex-col gap-4">

          {/* 登录方式。
              ⚠ 用 `join` + `btn` 而**不是** daisyUI 的 `tabs` / `tabs-box`：
              后者会把整个 tab 组件（含四套朝向变体与带框那套，实测 14.5KB）
              拖进产物，而全项目只有这一处是"选一个"的语义。
              写法与上面「客户端形态」保持一致 —— 同一页里两个同级选择器，
              不该一个用 tabs、一个用 join。

              窄屏（<md）**铺满等分**：375 宽下五个标签挤在一起每格只剩 ~58px，
              实测「账号密码」那格会顶到边（用户截图里就是这么挤的）；
              铺满 + 把四字的「账号密码」缩成两字「账密」之后每格宽裕。
              ⚠ 缩短的是**这格**的文案，面板里该怎么解释还怎么解释。 */}
          <div class="join w-full md:w-auto">
            {(
              [
                ['qr', '扫码'],
                ['sms', '短信'],
                ['app', '手机端'],
                ['password', '账密'],
                ['auth', '凭据'],
              ] as const
            ).map(([id, label]) => (
              <button
                type="button"
                aria-pressed={method.value === id}
                class={`btn join-item btn-sm min-h-11 flex-1 whitespace-nowrap px-2 md:min-h-0 md:flex-none md:px-4 ${method.value === id ? 'btn-primary' : 'btn-soft'}`}
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
              {needPlatform() ? <div class="w-full">{platformPicker()}</div> : null}
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
              {needPlatform() ? platformPicker() : null}
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
              {appPlatformNote()}
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
              {needPlatform() ? platformPicker() : null}
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

          {/*
            粘贴凭据。
            这条路**不做任何网络登录**：账号主键是手机号的哈希，服务端从凭据里
            就能算出来 —— 所以"从旧实例 / 插件 / 抓包里拿到的一条 auth"
            可以直接变成这里的账号。
          */}
          {method.value === 'auth' ? (
            <div class="flex flex-col gap-3">
              {needPlatform() ? platformPicker() : mobileCredentialNote()}
              <label class="flex flex-col gap-1">
                <span class="text-xs opacity-70">凭据</span>
                <textarea
                  class="textarea w-full font-mono text-xs"
                  rows={4}
                  placeholder="Basic <base64>，或只粘 base64 那段"
                  value={importAuth.value}
                  onInput={(e) => {
                    importAuth.value = (e.target as HTMLTextAreaElement).value
                  }}
                />
              </label>
              <p class="text-xs opacity-70">
                同一个手机号已经加过时，这里是换掉它的凭据，不是多一个账号 ——
                昵称、设备号与任务设置都会保留。凭据只落进本机的
                <code>accounts.json</code>，不会回显给界面。
              </p>
              <button
                type="button"
                class="btn btn-primary btn-sm self-start min-h-11 md:min-h-0"
                disabled={importBusy.value}
                onClick={() => void doImport()}
              >
                {importBusy.value ? '添加中…' : '用这条凭据添加'}
              </button>
            </div>
          ) : null}
        </div>
      </Dialog>

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

      {/*
        编辑账号（弹窗）。
        三格：昵称、设备号、换凭据 —— 都是**给了才改**（服务端是 PATCH）。
        凭据输入框刻意**留空**：它从不回显，空就是"不动它"，
        所以"改个名字"不需要用户重新粘一遍凭据。
      */}
      <Dialog id="edit-account" title="编辑账号">
        {editing.value ? (
          <div class="flex flex-col gap-3">
            <p class="text-xs opacity-70">
              账号 <b>{editing.value.nickname || editing.value.label || editing.value.id}</b>
              {' · '}
              {editing.value.pc_platform ? PLATFORM_LABEL[editing.value.pc_platform] : '手机端'}
              {' · 凭据 '}
              {expireText(editing.value.expire)}
            </p>

            <label class="flex flex-col gap-1">
              <span class="text-xs opacity-70">昵称（留空就用掩码手机号）</span>
              <input
                type="text"
                class="input w-full"
                maxlength={32}
                value={editNickname.value}
                onInput={(e) => {
                  editNickname.value = (e.target as HTMLInputElement).value
                }}
              />
            </label>

            <label class="flex flex-col gap-1">
              <span class="text-xs opacity-70">设备号</span>
              <input
                type="text"
                class="input w-full font-mono text-xs"
                value={editDevice.value}
                onInput={(e) => {
                  editDevice.value = (e.target as HTMLInputElement).value
                }}
              />
              <span class="text-xs opacity-70">
                上游按它认设备。不确定就别改 —— 换掉可能让这个账号的请求被上游拒绝。
              </span>
            </label>

            <label class="flex flex-col gap-1">
              <span class="text-xs opacity-70">换凭据（留空 = 不改）</span>
              <textarea
                class="textarea w-full font-mono text-xs"
                rows={3}
                placeholder="Basic <base64>，或只粘 base64 那段"
                value={editAuth.value}
                onInput={(e) => {
                  editAuth.value = (e.target as HTMLTextAreaElement).value
                }}
              />
            </label>

            {/* 换了 pc 凭据就得同时给它形态：服务端要求形态与凭据同源，
                否则这个账号以后每次请求都按错的平台发头。 */}
            {editAuth.value.trim() && credentialPrefix(editAuth.value) !== 'mobile' ? (
              <div class="flex flex-col gap-2">
                <span class="text-xs font-medium opacity-70">客户端形态</span>
                <div class="join">
                  {(['windows', 'macos'] as const).map((p) => (
                    <button
                      type="button"
                      class={`btn join-item btn-sm min-h-11 md:min-h-0 ${editPlatform.value === p ? 'btn-primary' : 'btn-soft'}`}
                      onClick={() => (editPlatform.value = p)}
                    >
                      {PLATFORM_LABEL[p]}
                    </button>
                  ))}
                </div>
              </div>
            ) : null}

            {editError.value ? (
              <div class="alert alert-soft alert-error text-sm" role="alert">
                <span class="break-words">{editError.value}</span>
              </div>
            ) : null}

            <div class="modal-action">
              <button
                type="button"
                class="btn btn-soft"
                onClick={() => closeDialog('edit-account')}
              >
                取消
              </button>
              <button
                type="button"
                class="btn btn-primary"
                disabled={savingEdit.value}
                onClick={() => void doSaveEdit()}
              >
                {savingEdit.value ? '保存中…' : '保存'}
              </button>
            </div>
          </div>
        ) : null}
      </Dialog>

      {/*
        每账号设置（弹窗）。
        结构：任务参数 → 功能开关 → 推送。推送里十条渠道**折叠**着，首屏只列
        已配置的那些，其余走「+ 加渠道」—— 十条渠道的表单全铺开有一屏半，
        而常用的一直是那几条。
        ⚠ 打开才 GET：见上面「打开才拉数据」那段。
      */}
      <Dialog
        id={SETTINGS_DIALOG}
        title={
          settingsAccount.value
            ? '账号设置 · ' +
              (settingsAccount.value.nickname || settingsAccount.value.label || settingsAccount.value.id)
            : '账号设置'
        }
        onClose={closeSettings}
      >
        {settingsBody()}
      </Dialog>

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
