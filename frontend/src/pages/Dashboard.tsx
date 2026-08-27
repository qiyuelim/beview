import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { CaretRight, Sparkle } from '@phosphor-icons/react'
import { apiGet } from '../api/client'
import type { Application, Dashboard as DashData, ReviewStats } from '../api/types'
import { cn } from '@/lib/utils'
import { PageHeader } from '../components/PageHeader'
import { SemBadge, type BadgeSem } from '../components/SemBadge'
import { CalendarCard } from '../components/CalendarCard'
import { ActivityTimelineCard } from '../components/ActivityTimelineCard'
import { FunnelCard } from '../components/FunnelCard'
import ApplicationInsightsCard from '../components/ApplicationInsightsCard'
import { EmptyState } from '../components/EmptyState'
import { Button } from '@/components/ui/button'

/** 求职台（v4 IA，ADR-0011 R2；v4.2 设计语言 v2 迁移）：漏斗 + 今日待办 + 周目标 + 时间线 + 总览日历 */

const STATUS_SEM: Record<string, BadgeSem> = {
  applied: 'neutral',
  callback: 'warn',
  interviewing: 'info',
  offer: 'pass',
  rejected: 'neutral',
  withdrawn: 'neutral',
}
const STATUS_LABEL: Record<string, string> = {
  applied: '已投递',
  callback: '有回音',
  interviewing: '进行中',
  offer: 'Offer',
  rejected: '未通过',
  withdrawn: '已撤回',
}

function DashCard({
  title,
  sub,
  action,
  children,
  className,
}: {
  title: string
  sub?: string
  action?: React.ReactNode
  children: React.ReactNode
  className?: string
}) {
  return (
    <section
      className={cn('rounded-xl border border-border bg-card', className)}
      aria-label={title}
    >
      <header className="flex items-center gap-2 border-b border-border px-4 py-2.5">
        <h2 className="text-[13px] font-semibold tracking-wide text-heading">{title}</h2>
        {sub ? <span className="text-xs text-muted-foreground">{sub}</span> : null}
        {action ? <div className="ml-auto">{action}</div> : null}
      </header>
      <div className="p-4">{children}</div>
    </section>
  )
}

export default function Dashboard() {
  const [dash, setDash] = useState<DashData | null>(null)
  const [review, setReview] = useState<ReviewStats | null>(null)
  const [apps, setApps] = useState<Application[]>([])
  const [err, setErr] = useState('')

  useEffect(() => {
    Promise.all([
      apiGet('/api/dashboard'),
      apiGet('/api/review/stats'),
      apiGet('/api/applications'),
    ])
      .then(([d, r, a]) => {
        setDash(d)
        setReview(r)
        setApps(a)
      })
      .catch((e) => setErr(e.message))
  }, [])

  const unanswered = dash?.unanswered ?? []
  const unanalyzed = dash?.unanalyzed ?? []
  const unanalyzedCount = dash?.summary.unanalyzed ?? unanalyzed.length
  const ongoing = apps.filter((a) => ['applied', 'callback', 'interviewing'].includes(a.status))
  const today = new Date().toLocaleDateString('zh-CN', {
    month: 'long',
    day: 'numeric',
    weekday: 'long',
  })

  return (
    <div className="space-y-4">
      <PageHeader title="求职台" meta={<span>{today}</span>} />

      {err && (
        <p role="alert" className="rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        {(
          [
            { to: '/review', label: '今日待复习', num: review === null ? null : review.due, unit: '张卡' },
            { to: '/applications', label: '进行中投递', num: ongoing.length, unit: '家' },
            { to: '/questions?analyzed=false', label: '待 AI 分析', num: dash === null ? null : unanalyzedCount, unit: '道' },
            { to: '/review', label: '连续打卡', num: review === null ? null : review.streak_days, unit: '天' },
          ] as const
        ).map((m) => (
          <Link
            key={m.label}
            to={m.to}
            className="surface-interactive rounded-xl border border-border bg-card px-4 py-3"
          >
            <div className="flex items-end justify-between gap-2">
              <span className="font-mono text-[1.75rem] font-semibold leading-none tabular-nums tracking-tight text-heading">
                {m.num === null ? '·' : m.num}
              </span>
              <span className="pb-0.5 text-[11px] font-medium text-foreground">{m.unit}</span>
            </div>
            <div className="mt-2 text-[13px] font-medium text-foreground">{m.label}</div>
          </Link>
        ))}
      </div>

      <div className="grid gap-4 lg:grid-cols-3">
        <div className="space-y-4 lg:col-span-2">
          <DashCard
            title="今日复习"
            action={
              <div className="flex items-center gap-2">
                <Button asChild size="sm">
                  <Link to="/review">开始刷题</Link>
                </Button>
                <Button asChild size="sm" variant="outline">
                  <Link to="/review/wrong">错题本</Link>
                </Button>
              </div>
            }
          >
            <p className="text-sm text-foreground">
              {review === null
                ? '加载中…'
                : review.due > 0
                  ? `${review.due} 张卡待复习 · 今日已完成 ${review.done_today}`
                  : `今日队列已清 · 已完成 ${review.done_today} 张`}
            </p>
          </DashCard>

          <DashCard title="进行中的投递">
            {ongoing.length === 0 ? (
              <EmptyState
                icon={<Sparkle className="size-6" />}
                title="还没有进行中的投递"
                hint="到投递看板新建，或把已有投递推进到面试。"
                action={
                  <Button asChild size="sm" variant="outline" className="h-10">
                    <Link to="/applications">去投递看板</Link>
                  </Button>
                }
                className="border-border py-8"
              />
            ) : (
              <ul className="divide-y divide-border">
                {ongoing.slice(0, 6).map((a) => {
                  const stages = a.interview_stages ?? []
                  const today = new Date().toISOString().slice(0, 10)
                  const dated = stages.filter((s) => s.date)
                  const upcoming = dated
                    .filter((s) => s.passed === 'pending' && (s.date ?? '') >= today)
                    .sort((x, y) => (x.date ?? '').localeCompare(y.date ?? ''))
                  const nextDate = upcoming[0]?.date ?? dated.find((s) => s.passed === 'pending')?.date
                  let countdown: string | null = null
                  if (nextDate) {
                    const diff = Math.round((new Date(nextDate + 'T00:00:00').getTime() - Date.now()) / 86400000)
                    countdown = diff > 0 ? `${diff} 天后面试` : diff === 0 ? '今天面试' : `已过 ${-diff} 天`
                  }
                  return (
                    <li key={a.id}>
                      <Link
                        to={`/applications/${a.id}`}
                        className="flex min-h-[52px] items-center gap-2.5 py-3 transition-colors hover:text-primary"
                      >
                        <SemBadge sem={STATUS_SEM[a.status] ?? 'neutral'}>
                          {STATUS_LABEL[a.status] ?? a.status}
                        </SemBadge>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm font-medium text-foreground">
                            {a.company ?? '未关联公司'}
                            {a.position ? ` · ${a.position}` : ''}
                          </span>
                          {stages.length > 0 && (
                            <span className="mt-1 flex items-center gap-1">
                              {stages.map((s, i) => (
                                <span
                                  key={i}
                                  title={s.name}
                                  className={cn(
                                    'size-1.5 rounded-full',
                                    s.passed === 'pass'
                                      ? 'bg-success'
                                      : s.passed === 'fail'
                                        ? 'bg-destructive'
                                        : 'bg-info',
                                  )}
                                />
                              ))}
                              <span className="ml-1 text-xs text-muted-foreground">{stages[stages.length - 1]?.name}</span>
                            </span>
                          )}
                        </span>
                        {countdown && (
                          <span className="shrink-0 text-xs font-medium text-foreground">{countdown}</span>
                        )}
                        <CaretRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                      </Link>
                    </li>
                  )
                })}
              </ul>
            )}
          </DashCard>

          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
            {(
              [
                { to: '/questions?analyzed=false', num: unanalyzedCount, label: '道题未分析', loading: dash === null },
                {
                  to: unanswered[0] ? `/questions/${unanswered[0].id}` : '/questions',
                  num: unanswered.length,
                  label: '道题未补答',
                  loading: dash === null,
                },
              ] as const
            ).map((t) => (
              <Link
                key={t.label}
                to={t.to}
                className="surface-interactive flex min-h-[48px] items-center gap-3 rounded-xl border border-border px-3.5 py-3"
              >
                <span className="w-7 text-center font-mono text-lg font-bold tabular-nums text-foreground">
                  {t.loading ? '·' : t.num}
                </span>
                <span className="flex-1 text-sm font-medium text-foreground">{t.label}</span>
                <CaretRight className="size-4 text-muted-foreground" aria-hidden />
              </Link>
            ))}
            <Link
              to="/drills/new"
              className="surface-interactive flex min-h-[48px] items-center gap-3 rounded-xl border border-border px-3.5 py-3 sm:col-span-2"
            >
              <Sparkle weight="fill" className="size-5 text-primary" aria-hidden />
              <span className="flex-1 text-sm font-semibold text-foreground">来一场模拟面试</span>
              <CaretRight className="size-4 text-muted-foreground" aria-hidden />
            </Link>
          </div>

          <ApplicationInsightsCard />
          <FunnelCard />
          <ActivityTimelineCard />
        </div>

        <div className="space-y-4">
          <CalendarCard />
        </div>
      </div>
    </div>
  )
}
