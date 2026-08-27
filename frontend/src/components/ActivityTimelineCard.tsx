import { useEffect, useState } from 'react'
import {
  Buildings,
  ChartBar,
  CheckCircle,
  ChatsCircle,
  Coin,
  PaperPlaneTilt,
} from '@phosphor-icons/react'
import type { IconProps } from '@phosphor-icons/react'
import { apiGet } from '../api/client'
import type { TimelineItem } from '../api/types'
import { Skeleton } from '@/components/ui/skeleton'

// ADR-0015 D2 组合层：活动流时间线（= 词表「时间线」，非审计日志）。
// 取代 Stats.tsx 内 legacy 版本在求职台的使用；DataPage 迁移时一并切换（M7）。
const TYPE_ICON: Record<string, React.ReactElement<IconProps>> = {
  review_done: <CheckCircle weight="fill" className="size-4 text-success" />,
  drill: <ChatsCircle className="size-4 text-info" />,
  point: <Coin weight="fill" className="size-4 text-warning" />,
  application: <PaperPlaneTilt className="size-4 text-info" />,
  session: <Buildings className="size-4 text-info" />,
  stats: <ChartBar className="size-4 text-muted-foreground" />,
}

export function ActivityTimelineCard({ limit = 12 }: { limit?: number }) {
  const [items, setItems] = useState<TimelineItem[] | null>(null)
  const [err, setErr] = useState('')
  useEffect(() => {
    apiGet('/api/dashboard/activity')
      .then((t: { items: TimelineItem[] }) => setItems(t.items.slice(0, limit)))
      .catch((e: any) => setErr(e.message))
  }, [limit])

  return (
    <section className="rounded-xl border border-border bg-card" aria-label="时间线">
      <header className="border-b border-border px-4 py-2.5">
        <h2 className="text-[13px] font-semibold tracking-wide text-heading">时间线</h2>
        <p className="text-xs text-muted-foreground">最近都做了什么</p>
      </header>
      <div className="p-4">
        {err ? (
          <p role="alert" className="text-sm text-destructive">
            {err}
          </p>
        ) : items === null ? (
          <Skeleton className="h-32 w-full" />
        ) : items.length === 0 ? (
          <p className="py-2 text-sm text-muted-foreground">还没有活动</p>
        ) : (
          <ol className="relative ml-1 space-y-3 border-l border-border pl-4">
            {items.map((it, i) => (
              <li key={i} className="relative">
                <span
                  className="absolute -left-[21px] top-1 grid size-3 place-items-center rounded-full bg-card ring-1 ring-border-strong"
                  aria-hidden
                >
                  {TYPE_ICON[it.type] ?? (
                    <span className="block size-1.5 rounded-full bg-muted-foreground" />
                  )}
                </span>
                <div className="text-sm font-medium leading-snug">{it.title}</div>
                {/* 元数据行：公司/详情/得分/时间 —— 灰字白名单「元数据」 */}
                <div className="mt-0.5 flex flex-wrap items-baseline gap-x-2 text-xs text-muted-foreground">
                  {it.company && <span>{it.company}</span>}
                  {it.detail && <span>{it.detail}</span>}
                  {it.score != null && <span>得分 {it.score}</span>}
                  <span className="ml-auto font-mono tabular-nums">{it.date}</span>
                </div>
              </li>
            ))}
          </ol>
        )}
      </div>
    </section>
  )
}
