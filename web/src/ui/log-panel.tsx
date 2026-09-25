/**
 * 运行日志面板。
 *
 * 固定高度 + 可滚动（规范 §7.2 「历史与日志」：SSE 日志区固定高度可滚动）。
 * `overscroll-contain` 防止 iOS 上滚到边界带动整页。
 */

export interface LogLine {
  kind: 'info' | 'warn' | 'error'
  text: string
  at: number
}

/**
 * 行的着色查表。**字面量**，不拼接（§9.1①）。
 *
 * 这里刻意用语义色 + `opacity`，不用 `gray-*` 之类的具名色阶 ——
 * 那些不随主题变，深色模式下必然出现"浅字压深底"。
 */
const TONE: Record<LogLine['kind'], string> = {
  info: 'opacity-90',
  warn: 'text-warning',
  error: 'text-error',
}

export const LogPanel = (props: { lines: LogLine[] }) => {
  if (props.lines.length === 0) return null

  return (
    <div class="card bg-base-100 shadow-sm">
      <div class="card-body">
        <h2 class="card-title text-base">运行日志</h2>
        <div class="h-64 overflow-y-auto overscroll-contain rounded-box bg-base-200 p-3 font-mono text-xs leading-5">
          {props.lines.map((l, i) => (
            <div key={i} class={`break-words ${TONE[l.kind]}`}>
              {l.text}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
