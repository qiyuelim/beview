import { APP_STATUS, type ApplicationStatus } from '../api/types'

export interface Stage {
  name: string
  passed: string // pending / pass / fail
}

/**
 * 统一节点时间线（B组 #3：四处展示逻辑归一）：
 * 投递 → 各轮（绿✓通过 / 红✗未过 / 灰待定）→ 终态。
 * compact 模式（看板卡内）：省略「投递」「终态」节点，只展示轮次进展。
 * 设计语言 v2（ADR-0015）：语义 token，双主题自适应。
 */
export default function StageTimeline({
  stages,
  status,
  compact = false,
}: {
  stages?: Stage[] | null
  status: ApplicationStatus
  compact?: boolean
}) {
  const list = stages ?? []
  const sep = <span className="mx-0.5 h-px w-3 shrink-0 bg-border-strong" aria-hidden />
  const node = (label: React.ReactNode, tone?: 'ok' | 'bad') => (
    <span
      className={`whitespace-nowrap text-xs ${
        tone === 'ok'
          ? 'font-medium text-success'
          : tone === 'bad'
            ? 'font-medium text-destructive'
            : 'text-muted-foreground'
      }`}
    >
      {label}
    </span>
  )
  return (
    <div className="flex flex-wrap items-center gap-y-0.5" role="img" aria-label="面试进展">
      {!compact && node('投递')}
      {list.map((s, i) => (
        <span key={i} className="flex items-center">
          {(i > 0 || !compact) && sep}
          {node(
            <>
              {s.name}
              {s.passed === 'pass' ? ' ✓' : s.passed === 'fail' ? ' ✗' : ''}
            </>,
            s.passed === 'pass' ? 'ok' : s.passed === 'fail' ? 'bad' : undefined,
          )}
        </span>
      ))}
      {!compact && (
        <span className="flex items-center">
          {sep}
          {node(
            APP_STATUS[status],
            status === 'offer' ? 'ok' : status === 'rejected' || status === 'withdrawn' ? 'bad' : undefined,
          )}
        </span>
      )}
    </div>
  )
}
