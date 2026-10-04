/**
 * 服务端**全量**日志。
 *
 * 与「历史」页的分工（用户 2026-10-04 口径：「我其实想要一个能看所有日志的
 * 窗口，**不仅仅是每次任务**」✓）：
 * * 历史页 = 每次运行的记录，以及**那次运行**的日志 ✓；
 * * 本页   = **整个进程**看到的日志 ✓（调度器 ✓、凭据续期 ✓、上游 http ✓）。
 *
 * 数据来自 `/api/logs` ✓ —— 内存里的环形缓冲（最近 2000 行 ✓，见
 * `crates/mcloud-server/src/logbuf.rs` ✓）。⚠ 它是"最近这些"而不是"全部历史"：
 * 完整历史在日志文件里 ✓；这一页解决的是"不开终端就能看" ✓。
 */
import { onMounted, onUnmounted, ref } from 'lite'

import { listServerLogs, type ServerLogLine } from '../api'
import { IconSearch } from '../ui/icons'
import { MARK, TONE, type LogKind } from '../ui/log-panel'

/** 级别档位。⚠ 语义是"**不低于**它" ✓（与后端 `min_level` 同一口径 ✓）。 */
const LEVELS: ReadonlyArray<readonly [string, string]> = [
  ['', '全部'],
  ['debug', '调试'],
  ['info', '信息'],
  ['warn', '警告'],
  ['error', '错误'],
]

/**
 * 服务端日志的级别 → 面板那套分类 ✓（两边同一套标记与颜色 ✓）。
 * `trace` 并入 `debug` ✓ —— 面板没有单独一档给它 ✓，而它本来就最不常开 ✓。
 */
const kindOf = (level: string): LogKind =>
  level === 'error' || level === 'warn' || level === 'info' || level === 'debug' ? level : 'debug'

export const LogsPage = () => {
  const lines = ref<ServerLogLine[]>([])
  const level = ref('')
  const q = ref('')
  const state = ref<'loading' | 'ready' | 'error'>('loading')
  const errorMessage = ref('')
  let timer: ReturnType<typeof setInterval> | undefined

  const load = async () => {
    try {
      lines.value = await listServerLogs({ limit: 1000, level: level.value, q: q.value })
      state.value = 'ready'
    } catch (e) {
      errorMessage.value = e instanceof Error ? e.message : String(e)
      state.value = 'error'
    }
  }

  onMounted(() => {
    void load()
    // 这页是"看现在正在发生什么" ✓ ⇒ 定时刷一次就够 ✓。
    // ⚠ 刻意**不**做 SSE：日志是只读的、丢一条也不影响判断 ✓；
    // 而"逐条不能丢"那种需求是 `/api/runs/{id}/events` 的活儿 ✓（别抢它的活 ✗）。
    timer = setInterval(() => void load(), 2000)
  })
  onUnmounted(() => {
    if (timer) clearInterval(timer)
  })

  return (
    <div class="flex flex-col gap-4">
      <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
        <div class="flex flex-wrap items-center gap-1.5">
          {LEVELS.map(([id, label]) => (
            <button
              type="button"
              aria-pressed={level.value === id}
              aria-label={`级别：${label}`}
              class={`btn btn-sm min-h-11 md:min-h-0 ${level.value === id ? 'btn-primary' : 'btn-soft'}`}
              onClick={() => {
                level.value = id
                void load()
              }}
            >
              {label}
            </button>
          ))}
        </div>

        <label class="input flex min-h-11 w-full items-center gap-2 md:min-h-0 md:w-64">
          <IconSearch class="h-4 w-4 opacity-60" />
          <input
            class="grow"
            placeholder="搜索正文 / 来源模块"
            value={q.value}
            onInput={(e) => (q.value = (e.target as HTMLInputElement).value)}
          />
        </label>
      </div>

      {state.value === 'error' ? (
        <div class="alert alert-soft alert-error text-sm" role="alert">
          <span class="break-words">{errorMessage.value}</span>
        </div>
      ) : null}

      {state.value === 'ready' && lines.value.length === 0 ? (
        <p class="text-sm opacity-70">没有匹配的日志（缓冲只留最近 2000 行 ✓）。</p>
      ) : null}

      {lines.value.length > 0 ? (
        <div class="card bg-base-100 shadow-sm">
          <div class="card-body gap-2">
            <div class="max-h-[70vh] overflow-y-auto overscroll-contain rounded-box bg-base-200 p-3 font-mono text-xs leading-5">
              {lines.value.map((l, i) => (
                // ⚠ 标记、正文、来源拼成**一个**表达式 ✓（lite 编译器对"元素旁边的
                // 裸文本节点"支持最薄 ✓ —— 这个坑在别的页面踩过 ✓）。
                <div key={i} class={`break-words ${TONE[kindOf(l.level)]}`}>
                  {`${MARK[kindOf(l.level)]}  ${l.message}  ·  ${l.target}`}
                </div>
              ))}
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}
