import { useEffect, useState } from 'react'
import { apiGet } from '../api/client'
import type { Funnel } from '../api/types'
import { Skeleton } from '@/components/ui/skeleton'

const FUNNEL_LABEL: Record<string, string> = {
  applied: '已投递',
  callback: '有回音',
  interviewing: '进行中',
  offer: 'Offer',
}

// ADR-0015 D2 组合层：求职漏斗卡（数据：GET /api/stats/funnel）。
// 求职台与投递看板共用；转化率行属「确有价值的非核心 metadata」白名单。
export function FunnelCard({ className }: { className?: string }) {
  const [funnel, setFunnel] = useState<Funnel | null>(null)
  const [err, setErr] = useState('')

  useEffect(() => {
    apiGet('/api/stats/funnel')
      .then(setFunnel)
      .catch((e: any) => setErr(e.message))
  }, [])

  return (
    <section className={className} aria-label="求职漏斗">
      <div className="rounded-xl border border-border bg-card">
        <header className="flex items-center gap-2 border-b border-border px-4 py-2.5">
          <h2 className="text-[13px] font-semibold tracking-wide text-heading">求职漏斗</h2>
          <span className="text-xs text-muted-foreground">达到过某阶段</span>
        </header>
        <div className="p-4">
          {err ? (
            <p role="alert" className="text-sm text-destructive">
              {err}
            </p>
          ) : funnel === null ? (
            <Skeleton className="h-16 w-full" />
          ) : (
            <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-4">
              {funnel.funnel.map((st, i) => (
                <div key={st.stage} className="rounded-md bg-muted/60 px-3 py-2.5">
                  <div className="font-mono text-2xl font-bold leading-none tabular-nums">{st.count}</div>
                  <div className="mt-1 text-xs font-medium">{FUNNEL_LABEL[st.stage] ?? st.stage}</div>
                  {i < funnel.conversion.length && (
                    <div className="mt-0.5 text-xs text-muted-foreground">
                      ↓ {funnel.conversion[i]?.rate ?? 0}%
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </section>
  )
}
