/**
 * 设置页。
 *
 * 2026-10-01 重做（用户口径「系统设置页面」）。
 *
 * # 这一版改掉了什么
 *
 * - **删掉「外观（主题）」**：顶栏那个按钮就够了，摆在设置里是重复入口
 *   （用户原话："主题没用"）。连带 `ui/theme-signal.ts` 一起回退 ——
 *   那个共享信号的全部理由就是"两个入口别分叉"，只剩一个入口时它是多余的抽象。
 * - **账户安全从"只有读数"变成能操作**：改口令、口令下限、会话时长。
 *   后端这次一起补上了（`PUT /api/admin/password`、`GET/PUT /api/admin/security`）——
 *   之前这里写着"需要后端接口（现在没有）"，用户说"把后端也补上"。
 * - **补「存储信息」与「重启服务」**：数据目录/日志占用/二进制路径，以及一个真的
 *   会优雅重启服务的按钮（后端 `GET /api/system/storage`、`POST /api/system/restart`）。
 *
 * # 为什么账户安全那块只认"当前口令"而不认身份
 *
 * 会话被盗时这一条拦不住 —— 但那是会话本身的语义：能改口令的会话本来就有全部权限。
 * 真正的防护是"改完把**所有**旧会话踢掉"（后端做的事），包括发起这次请求的那张，
 * 然后给当前浏览器换一张新的。
 */

import { onMounted, onUnmounted, ref } from 'lite'
import { setPageRefresh } from '../ui/page-refresh'
import {
  changePassword,
  getAbout,
  getSecurity,
  getStatus,
  getStorage,
  getVersion,
  putSecurity,
  restartService,
  type AboutView,
  type SecurityView,
  type ServiceStatus,
  type StorageView,
  type VersionInfo,
} from '../api'
import { ConfirmDialog, closeDialog, openDialog } from '../ui/dialog'
import { pushToast } from '../ui/toast'
import { credentialFor } from '../ui/password'
import { StatRow, type StatItem } from '../ui/stat-row'
import { fmtClock, fmtEpochMs, fmtUptime } from '../ui/time'
import { runStatusOf, statusBadge, statusLabel } from '../ui/history'
import { IconBrand } from '../ui/icons'
import { Select } from '../ui/select'

type LoadState = 'loading' | 'error' | 'ready'

/** 「运行时间」得自己往前走，30 秒一格（与总览页同一个口径）。 */
const TICK_MS = 30_000

/** 会话时长的可选项（小时）。后端允许 1 小时 ~ 90 天。 */
const TTL_CHOICES = [
  { hours: 1, label: '1 小时' },
  { hours: 24, label: '1 天' },
  { hours: 72, label: '3 天' },
  { hours: 168, label: '7 天' },
  { hours: 720, label: '30 天' },
]

/** 字节数说人话。只到 MB —— 这个量级的目录报到 GB 反而看不出差别。 */
const fmtBytes = (n: number): string => {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

export const SettingsPage = () => {
  const state = ref<LoadState>('loading')
  const errorMessage = ref('')
  const version = ref<VersionInfo | null>(null)
  const status = ref<ServiceStatus | null>(null)
  const security = ref<SecurityView | null>(null)
  const about = ref<AboutView | null>(null)
  const storage = ref<StorageView | null>(null)
  const now = ref(Date.now())

  // 改口令表单（三个框：当前 + 新 + 确认）
  const curPwd = ref('')
  const newPwd = ref('')
  const confirmPwd = ref('')
  const changing = ref(false)

  // 策略表单（与已存值分开：改到一半时页面上要能看出"还没保存"）
  const minLen = ref(8)
  const ttlHours = ref(168)
  const savingPolicy = ref(false)

  // 重启
  const restarting = ref(false)
  const restartWaited = ref(0)

  let ticker: ReturnType<typeof setInterval> | undefined

  const load = async () => {
    state.value = 'loading'
    errorMessage.value = ''
    try {
      const [v, s, sec, st, ab] = await Promise.all([
        getVersion(),
        getStatus(),
        getSecurity(),
        getStorage(),
        getAbout(),
      ])
      version.value = v
      status.value = s
      security.value = sec
      storage.value = st
      about.value = ab
      minLen.value = sec.minPasswordLen
      ttlHours.value = sec.sessionTtlHours
      now.value = Date.now()
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : '无法连接后端'
      state.value = 'error'
    }
  }

  onMounted(() => {
  setPageRefresh({ run: () => void load() })
    void load()
    ticker = setInterval(() => {
      now.value = Date.now()
    }, TICK_MS)
  })

  onUnmounted(() => clearInterval(ticker))

  /** 指标卡。读数全来自 `/api/status`，一个都不编。 */
  const metrics = (): StatItem[] => {
    const s = status.value
    if (!s) return []
    const running = s.schedulerRunning
    return [
      {
        title: '调度器',
        value: running ? '运行中' : '已停止',
        badge: running ? { text: '正常', tone: 'ok' } : { text: '已停', tone: 'warn' },
      },
      {
        title: '运行时间',
        value: fmtUptime(now.value - s.startedAtMs),
        desc: `自 ${fmtClock(s.startedAtMs)}`,
      },
      { title: '已注册任务', value: String(s.taskCount) },
      { title: '定时规则', value: String(s.scheduleCount) },
    ]
  }

  /** 一行「标签 —— 值」。值可换行，不挤标签（窄屏也放得下）。 */
  const kv = (label: string, value: JSX.Element | string) => (
    <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 text-sm">
      <span class="opacity-60">{label}</span>
      <span class="min-w-0 break-words text-right">{value}</span>
    </div>
  )

  const serviceCard = () => {
    const s = status.value
    if (!s) return null
    const last = s.lastRun
    onUnmounted(() => setPageRefresh(null))

  return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">
              {version.value?.name ?? s.name}
              <span class="badge badge-soft badge-sm">v{version.value?.version ?? s.version}</span>
            </h2>
            <span class="badge badge-soft badge-success badge-sm">服务在线</span>
          </div>

          <StatRow items={metrics()} />

          <div class="flex flex-col gap-1.5">
            {kv('编排', `${s.pipelineCount} 条`)}
            {kv('正在运行', `${s.runningCount} 个`)}
            {kv(
              '最近一次运行',
              last ? (
                <span class="flex flex-wrap items-center justify-end gap-2">
                  <span class="opacity-80">
                    {fmtEpochMs(last.started_at_ms)} · {last.task}
                  </span>
                  <span class={`badge badge-sm ${statusBadge(runStatusOf(last))}`}>
                    {statusLabel(runStatusOf(last))}
                  </span>
                </span>
              ) : (
                '还没有跑过'
              ),
            )}
          </div>
        </div>
      </div>
    )
  }

  // ── 账户安全 ──────────────────────────────────────────────────────────

  /** 改口令的本地校验：能在前端挡住的就别浪费一次往返。 */
  const passwordProblem = (): string => {
    if (!curPwd.value) return '请填写当前口令'
    if (!newPwd.value) return '请填写新口令'
    if (newPwd.value.length < minLen.value) return `新口令至少 ${minLen.value} 位`
    if (newPwd.value !== confirmPwd.value) return '两次输入的新口令不一致'
    if (newPwd.value === curPwd.value) return '新口令与当前口令相同'
    return ''
  }

  const submitPassword = async () => {
    const bad = passwordProblem()
    if (bad) {
      pushToast('warning', bad)
      return
    }
    changing.value = true
    try {
      const scheme = security.value?.passwordScheme ?? 'sha256'
      const [cur, next] = await Promise.all([
        credentialFor(curPwd.value, scheme),
        credentialFor(newPwd.value, scheme),
      ])
      const r = await changePassword(cur, next)
      curPwd.value = ''
      newPwd.value = ''
      confirmPwd.value = ''
      // ⚠ 后端换了新 cookie，所以这一次**不会**被登出
      pushToast('success', `口令已更新，已结束 ${r.sessionsEnded} 个旧会话`)
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '改口令失败')
    } finally {
      changing.value = false
    }
  }

  const savePolicy = async () => {
    savingPolicy.value = true
    try {
      const next = await putSecurity({
        minPasswordLen: Number(minLen.value),
        sessionTtlHours: Number(ttlHours.value),
      })
      security.value = next
      minLen.value = next.minPasswordLen
      ttlHours.value = next.sessionTtlHours
      pushToast('success', '策略已保存（对之后设的口令与新登录生效）')
    } catch (e) {
      pushToast('error', e instanceof Error ? e.message : '保存失败')
    } finally {
      savingPolicy.value = false
    }
  }

  /**
   * 改口令。
   *
   * ⚠ 「当前口令」这一格**必须留**（用户口径 2026-10-01："你口令更改怎么取消了旧口令"）：
   * 参照的系统只填新口令，是因为它的会话本来就是全部权限；本仓照样验旧口令 ——
   * 挡住"别人趁你没锁屏、坐下就改掉口令"。后端也是必填（`PUT /api/admin/password`）。
   *
   * 三个框**不挤一行**：当前口令整行，新/确认并排。
   */
  const passwordCard = () => {
    const sec = security.value
    if (!sec) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h2 class="card-title text-base">修改管理员口令</h2>
          {/*
            三个框**不挤一行**（用户口径："改密码不一定一行，你现在有三个输入"）：
            验证那格整行，新/确认并排 —— 正好两组语义各占一行。
          */}
          <div class="grid min-w-0 gap-3 md:grid-cols-2">
            <fieldset class="fieldset min-w-0 md:col-span-2">
              <legend class="fieldset-legend">当前口令</legend>
              <input
                class="input w-full min-w-0"
                type="password"
                autocomplete="current-password"
                value={curPwd.value}
                onInput={(e) => (curPwd.value = (e.target as HTMLInputElement).value)}
              />
            </fieldset>
            <fieldset class="fieldset min-w-0">
              <legend class="fieldset-legend">新口令</legend>
              <input
                class="input w-full min-w-0"
                type="password"
                autocomplete="new-password"
                value={newPwd.value}
                onInput={(e) => (newPwd.value = (e.target as HTMLInputElement).value)}
              />
            </fieldset>
            <fieldset class="fieldset min-w-0">
              <legend class="fieldset-legend">确认新口令</legend>
              <input
                class="input w-full min-w-0"
                type="password"
                autocomplete="new-password"
                value={confirmPwd.value}
                onInput={(e) => (confirmPwd.value = (e.target as HTMLInputElement).value)}
              />
            </fieldset>
          </div>
          <p class="text-xs opacity-60">
            口令长度至少 {sec.minPasswordLen} 个字符；改完所有旧会话都会失效（包括其它设备）。
          </p>
          <div class="card-actions justify-end">
            <button
              type="button"
              class="btn btn-sm btn-primary min-h-11 md:min-h-0"
              disabled={changing.value}
              onClick={() => void submitPassword()}
            >
              更新口令
            </button>
          </div>
        </div>
      </div>
    )
  }

  /**
   * 密码策略 + 口令存储读数。
   *
   * ⚠ **没有**「含字母 / 含数字 / 含特殊符号」那几档（参照图里有）：
   * 本项目的口令在浏览器里已经做过一次 SHA-256，服务端拿到的是 64 位摘要 ——
   * 它**量不出**明文里有几个数字、几个符号。做出来就只能挂在前端，而前端拦不住
   * `curl`，那是**假开关**。真要做，得先放弃"明文不出浏览器"。
   */
  const policyCard = () => {
    const sec = security.value
    if (!sec) return null
    const upgraded = (sec.passwordScheme ?? 'sha256') !== 'plain'
    const setMin = (v: number) => (minLen.value = Math.min(64, Math.max(4, v)))
    // ⚠ `h-full` + 下面 `card-actions mt-auto`：并排的两张卡**底边必须齐**
    // （用户口径："不要这种不平的布局" —— 一高一矮、按钮错位就是"不平"）
    return (
      <div class="card h-full bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">口令策略</h2>
            <span
              class={`badge badge-sm ${upgraded ? 'badge-soft badge-success' : 'badge-soft badge-warning'}`}
            >
              {upgraded ? '两段哈希' : '待升级'}
            </span>
          </div>

          <div class="flex flex-wrap items-center justify-between gap-3">
            <div class="min-w-0">
              <p class="text-sm">最小长度</p>
              <p class="text-xs opacity-60">限制口令的最低字符数（4~64）</p>
            </div>
            {/* 步进器：数字框配 −/+ 两个按钮，与参照图一致 */}
            <div class="join shrink-0">
              <button
                type="button"
                class="btn join-item"
                aria-label="最小长度减一"
                onClick={() => setMin(Number(minLen.value) - 1)}
              >
                −
              </button>
              <input
                class="input join-item w-16 min-w-0 text-center"
                type="number"
                min="4"
                max="64"
                value={String(minLen.value)}
                aria-label="最小长度"
                onInput={(e) => (minLen.value = Number((e.target as HTMLInputElement).value))}
              />
              <button
                type="button"
                class="btn join-item"
                aria-label="最小长度加一"
                onClick={() => setMin(Number(minLen.value) + 1)}
              >
                +
              </button>
            </div>
          </div>
          <p class="text-xs opacity-60">
            只对明文这条路径生效：浏览器那份摘要恒 64 位，服务端量不出长度。
          </p>

          <div class="flex flex-col gap-1.5 border-t border-base-300 pt-3">
            {kv(
              '口令存储',
              <span class="font-mono text-xs">SHA-256（浏览器）→ Argon2id（服务端）</span>,
            )}
            {upgraded ? null : (
              <p class="text-xs opacity-60">
                这个库还是老档（Argon2id 直接吃明文）：下次登录成功时会自动升级。
              </p>
            )}
          </div>

          <div class="card-actions mt-auto justify-end">
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={savingPolicy.value}
              onClick={() => void savePolicy()}
            >
              保存策略
            </button>
          </div>
        </div>
      </div>
    )
  }

  /** 会话控制：有效期 + 一句该提醒的话（参照图那块黄色提示的同位）。 */
  const sessionCard = () => {
    const sec = security.value
    if (!sec) return null
    return (
      <div class="card h-full bg-base-100 shadow-sm">
        <div class="card-body gap-3">
          <h2 class="card-title text-base">会话控制</h2>
          <fieldset class="fieldset min-w-0">
            <legend class="fieldset-legend">会话有效期</legend>
            <Select
              id="session-ttl"
              class="w-full min-w-0"
              ariaLabel="会话有效期"
              value={String(ttlHours.value)}
              options={TTL_CHOICES.map((c) => ({ value: String(c.hours), label: c.label }))}
              onChange={(v) => {
                ttlHours.value = Number(v)
              }}
            />
            <p class="text-xs opacity-60">只影响之后新建的会话，已发出的不会被缩短。</p>
          </fieldset>

          <div class="alert alert-soft alert-warning text-xs">
            会话越长越省事，也意味着那台设备上的登录状态有效越久。公用电脑上建议选 1 小时，
            用完随手点右上角退出。
          </div>

          <div class="card-actions mt-auto justify-end">
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={savingPolicy.value}
              onClick={() => void savePolicy()}
            >
              保存会话设置
            </button>
          </div>
        </div>
      </div>
    )
  }

  // ── 存储信息 ──────────────────────────────────────────────────────────

  const pathRow = (label: string, value: string) => (
    <div class="flex flex-col gap-1">
      <span class="text-xs opacity-60">{label}</span>
      <span class="break-all font-mono text-xs">{value}</span>
    </div>
  )

  const storageCard = () => {
    const st = storage.value
    if (!st) return null
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-4">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <h2 class="card-title text-base">存储信息</h2>
            <span class="badge badge-soft badge-sm">JSON 文件</span>
          </div>

          <div class="grid min-w-0 gap-4 md:grid-cols-3">
            {pathRow('数据目录', st.dataDir)}
            {pathRow('日志目录', st.logDir)}
            {pathRow('二进制', st.binary)}
          </div>

          <div class="flex flex-col gap-1.5">
            {kv('数据占用', fmtBytes(st.dataBytes))}
            {kv('日志占用', fmtBytes(st.logBytes))}
          </div>

          {/*
            ⚠ 数据文件**不写成表格**：表格在 1500 宽的卡里会变成"名字在最左、
            大小在最右"的一条长线（用户截图抱怨的正是这个）。三列成格之后，
            每格的"名字 —— 大小"自己成对，眼睛不用横穿屏幕。
            想知道"哪张表在长"时，逐项读数仍然都在。
          */}
          <div class="grid min-w-0 gap-x-4 gap-y-2 md:grid-cols-3">
            {st.files.map((f) => (
              <div
                key={f.name}
                class="flex min-w-0 items-baseline justify-between gap-2 border-b border-base-300 pb-1"
              >
                <span class="truncate font-mono text-xs">{f.name}</span>
                <span class="shrink-0 font-mono text-xs tabular-nums opacity-70">
                  {fmtBytes(f.bytes)}
                </span>
              </div>
            ))}
          </div>
        </div>
      </div>
    )
  }

  // ── 服务操作 ──────────────────────────────────────────────────────────

  /**
   * 重启：先让后端受理，再**轮询**等它回来。
   *
   * ⚠ 那两下之间服务是真的不在：`restartService` 成功**不代表**重启完成
   * （后端 300ms 后才优雅退出）。所以这里必须自己等 `/api/status` 重新答话 ——
   * 不等的话界面会在"服务正下线"的窗口里显示一堆网络错误。
   */
  const doRestart = async () => {
    closeDialog('confirm-restart')
    restarting.value = true
    restartWaited.value = 0
    try {
      await restartService()
    } catch {
      // 服务已经下线 ⇒ 请求被截断是**预期**的，不是失败
    }
    for (let i = 0; i < 90; i++) {
      await sleep(1000)
      restartWaited.value = i + 1
      try {
        await getStatus()
        break
      } catch {
        // 还没起来，接着等
      }
    }
    restarting.value = false
    pushToast('success', '服务已重新上线')
    await load()
  }

  const actionCard = () => (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body gap-3 md:flex-row md:items-center md:justify-between">
        <div class="min-w-0">
          <h2 class="card-title text-base">服务操作</h2>
          <p class="mt-1 text-sm opacity-70">
            重启 = 让服务进程优雅退出，由 launchd / systemd 重新拉起。
            正在跑的任务会被中止；数据在退出前已经落盘。
          </p>
        </div>
        {restarting.value ? (
          <div class="flex shrink-0 flex-wrap items-center gap-2 text-sm" role="status">
            <span class="loading loading-sm text-primary" />
            <span>重启中… 已等待 {restartWaited.value} 秒</span>
          </div>
        ) : (
          <div class="card-actions shrink-0">
            <button
              type="button"
              class="btn btn-sm btn-soft min-h-11 md:min-h-0"
              disabled={state.value !== 'ready'}
              onClick={() => openDialog('confirm-restart')}
            >
              重启服务
            </button>
          </div>
        )}
      </div>
    </div>
  )

  /**
   * 「关于」卡片：品牌 + 版本信息 + 更新说明 + 版权声明。
   *
   * # 数据都是从哪来的
   *
   * - 名字/版本：`GET /api/system/about`（编译期 `CARGO_PKG_VERSION`）；
   * - 构建时间 / 提交号 / 是否脏：**构建期**从 `git`、`date` 抓下来写进二进制
   *   （`crates/mcloud-server/build.rs`）—— 运行期不需要仓库在场；
   * - 更新说明：本次构建包含的最近 12 条提交。
   *
   * ⚠ **为什么不做「检查更新」**：上游仓库是私有的（匿名访问 404），而这个服务在
   * 浏览器端没有任何凭据去查发布。摆一个必然失败的按钮，用户只会以为是自己网络的问题。
   *
   * ⚠ 版权行的署名用的是本机 launchd 标签 / 工作区里的 `catlair`，年份取当年
   * （写死 2026 的话，过一年这个声明自己就过期了）。
   */
  const aboutCard = () => {
    const ab = about.value
    const year = new Date().getFullYear()
    return (
      <div class="card bg-base-100 shadow-sm">
        <div class="card-body gap-4">
          <div class="flex flex-wrap items-center gap-3">
            <span class="flex size-11 shrink-0 items-center justify-center rounded-box bg-primary text-primary-content">
              <IconBrand class="size-6" />
            </span>
            <div class="min-w-0">
              <h2 class="card-title text-base">{ab?.name ?? 'mcloud'}</h2>
              <p class="text-xs opacity-60">中国移动云盘自动化工具（自用）</p>
            </div>
            <div class="ml-auto flex flex-wrap items-center justify-end gap-2">
              <span class="badge badge-soft badge-sm">v{ab?.version ?? '—'}</span>
              {ab?.commit ? (
                <span class="badge badge-soft badge-sm font-mono">{ab.commit}</span>
              ) : null}
              {ab?.profile ? <span class="badge badge-soft badge-sm">{ab.profile}</span> : null}
              {ab?.dirty ? (
                <span
                  class="badge badge-soft badge-warning badge-sm"
                  title="构建时工作区有未提交改动 —— release 会把它们一起编进线上，所以线上跑的不等于任何一个提交"
                >
                  有未提交改动
                </span>
              ) : null}
            </div>
          </div>

          <div class="flex flex-col gap-1.5">
            {kv('构建时间', ab?.buildTime || '—')}
            {kv('版本', `${ab?.name ?? 'mcloud'} ${ab?.version ?? ''}`.trim())}
          </div>

          {/* 更新说明 */}
          <div class="flex flex-col gap-1.5 border-t border-base-300 pt-3">
            <h3 class="text-sm font-medium">更新说明</h3>
            <p class="text-xs opacity-60">
              本次构建包含的改动（来自构建机上的 git 记录）：
            </p>
            {ab && ab.changes.length > 0 ? (
              <ul class="flex flex-col gap-1">
                {ab.changes.map((c) => (
                  <li key={c.hash} class="flex min-w-0 items-baseline gap-2 text-xs">
                    <span class="shrink-0 font-mono opacity-60">{c.hash}</span>
                    <span class="min-w-0 break-words">{c.subject}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <p class="text-xs opacity-60">构建机上没有 git 记录，这一栏是空的。</p>
            )}
            <p class="text-xs opacity-60">
              上游仓库是私有的，这个界面没有凭据去联网查发布 —— 所以这里不提供
              「检查更新」，只列本机这份构建实际带了什么。
            </p>
          </div>

          <footer class="border-t border-base-300 pt-3 text-xs opacity-60">
            <p>{`${ab?.name ?? 'mcloud'} ${ab?.version ?? ''}`.trim() + ` · Copyright © ${year} catlair · Apache-2.0`}</p>
            <p class="mt-1">按 Apache-2.0 许可发布，按「现状」提供，不含任何担保。</p>
          </footer>
        </div>
      </div>
    )
  }

  return (
    /**
     * 桌面形态：**用满宽度，但要有结构**。
     *
     * 第一版（2026-10-01）我把设置页做成了单列，于是 1512 宽的屏上每张卡都拉满 ——
     * 表格里"文件名在最左、大小在最右"，眼睛要横穿整屏（用户原话：
     * "pc端这样太宽了视觉体验差"），随后他贴了另一套系统的同页做参照：
     * **两张卡并排、路径三列、读数成格**。
     *
     * 所以这里的口径是：
     * - `max-w-7xl`：超大屏别无限拉（2560 宽的显示器上不然每行字都太长）；
     * - 卡内用 `grid` 分栏：`lg` 起「存储信息 | 服务操作」并排，路径/数据文件各占三列；
     * - 窄屏一切照旧堆叠（`grid` 的默认单列）。
     */
    <div class="mx-auto flex w-full max-w-7xl flex-col gap-4">

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
        <>
          {serviceCard()}
          {passwordCard()}
          {/* 「口令策略 | 会话控制」并排 —— 与参照图同一结构（两件互不相关的事，别叠成一列） */}
          <div class="grid min-w-0 gap-4 lg:grid-cols-2">
            <div class="min-w-0">{policyCard()}</div>
            <div class="min-w-0">{sessionCard()}</div>
          </div>
          {/*
            ⚠ 「服务操作」**不跟存储信息并排**（第一版并排过）：它只有一段话 + 一个按钮，
            而存储信息有 7 个数据文件 —— 等高会对着一张几乎空的卡，不等高就是"不平"。
            它改成一整行、按钮放右边，两边都不空。
          */}
          {storageCard()}
          {actionCard()}
          {aboutCard()}
        </>
      ) : null}

      <ConfirmDialog
        id="confirm-restart"
        title="重启服务"
        impact={
          <span>
            将让 mcloud 服务进程优雅退出，由托管方重新拉起 ——
            <strong>正在跑的任务会被中止</strong>。这段时间这个页面会连不上后端，
            起来之后自动恢复（通常几秒到二十几秒；刚重新编译过的话，第一次启动可能要等系统授权）。
          </span>
        }
        confirmLabel="重启"
        onConfirm={() => void doRestart()}
        busy={restarting.value}
      />
    </div>
  )
}
