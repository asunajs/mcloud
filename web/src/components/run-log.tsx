export interface LogLine {
  kind: 'info' | 'warn' | 'error'
  text: string
  at: number
}

const tone: Record<LogLine['kind'], string> = {
  info: 'text-base-content',
  warn: 'text-warning',
  error: 'text-error',
}

/** 运行日志面板。 */
export function RunLog(props: { lines: LogLine[] }) {
  if (props.lines.length === 0) return null

  return (
    <div class="card bg-neutral text-neutral-content shadow-sm">
      <div class="card-body py-3">
        <h2 class="card-title text-sm">运行日志</h2>
        <pre class="text-xs leading-relaxed whitespace-pre-wrap max-h-80 overflow-auto">
          {props.lines.map((l, i) => (
            <div key={i} class={tone[l.kind]}>
              {l.text}
            </div>
          ))}
        </pre>
      </div>
    </div>
  )
}
