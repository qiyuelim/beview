import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router-dom'
import { CaretLeft, CaretRight } from '@phosphor-icons/react'
import { apiGet } from '../api/client'
import { cn } from '@/lib/utils'
import { Skeleton } from '@/components/ui/skeleton'

// ADR-0015 D6 总览日历：面试轮次的只读投影（未来全部+近30天，GET /api/calendar/events）。
// 月网格 + 今日高亮 + 点事件跳详情；不可创建/编辑。<640px 退化为「即将到来」议程列表。

interface CalEvent {
  kind: 'round'
  id: number
  date: string // YYYY-MM-DD
  name: string
  passed: string // pending | pass | fail
  company: string | null
  position: string | null
  form: string | null
}

const WEEKDAYS = ['一', '二', '三', '四', '五', '六', '日']

function passedStyle(passed: string): string {
  switch (passed) {
    case 'pass':
      return 'bg-success/10 text-success'
    case 'fail':
      return 'bg-destructive/10 text-destructive'
    default:
      return 'bg-info/10 text-info'
  }
}

function eventLabel(e: CalEvent): string {
  return [e.company ?? '未命名公司', e.position].filter(Boolean).join('·') + ` ${e.name}`
}

export function CalendarCard() {
  const [events, setEvents] = useState<CalEvent[] | null>(null)
  const [err, setErr] = useState('')
  const now = new Date()
  const [view, setView] = useState({ y: now.getFullYear(), m: now.getMonth() })
  // 选中的日（YYYY-MM-DD）：点击格子在下方展示当日议程，议程项点击即跳详情
  const [selected, setSelected] = useState<string>(formatDate(now))

  useEffect(() => {
    apiGet('/api/calendar/events')
      .then((d: { events: CalEvent[] }) => setEvents(d.events))
      .catch((e: any) => {
        // 后端未就绪（404）给中性提示；其余错误才用警示色（提示真实）
        if (e?.status === 404) setEvents([])
        else setErr(e.message)
      })
  }, [])

  const byDate = useMemo(() => {
    const map = new Map<string, CalEvent[]>()
    for (const e of events ?? []) {
      const list = map.get(e.date) ?? []
      list.push(e)
      map.set(e.date, list)
    }
    return map
  }, [events])

  // 月网格：周一起始，前补上月尾，总格数补满整周
  const cells = useMemo(() => {
    const first = new Date(view.y, view.m, 1)
    const lead = (first.getDay() + 6) % 7 // 周一=0
    const daysInMonth = new Date(view.y, view.m + 1, 0).getDate()
    const total = Math.ceil((lead + daysInMonth) / 7) * 7
    return Array.from({ length: total }, (_, i) => {
      const d = new Date(view.y, view.m, i - lead + 1)
      return { date: d, inMonth: d.getMonth() === view.m }
    })
  }, [view])

  const todayKey = formatDate(now)
  function shift(delta: number) {
    setView(({ y, m }) => {
      const nm = m + delta
      return { y: y + Math.floor(nm / 12), m: ((nm % 12) + 12) % 12 }
    })
  }

  const monthEvents = (events ?? []).filter((e) => {
    const [y, m] = e.date.split('-').map(Number)
    return y === view.y && m - 1 === view.m
  })

  return (
    <section className="rounded-xl border border-border bg-card" aria-label="面试日程">
      <header className="flex items-center gap-1 border-b border-border px-4 py-2.5">
        <h2 className="text-[13px] font-semibold tracking-wide text-heading">
          {view.y} 年 {view.m + 1} 月
        </h2>
        <div className="ml-auto flex items-center gap-1">
          <button
            onClick={() => shift(-1)}
            aria-label="上一月"
            className="grid size-8 cursor-pointer place-items-center rounded-md text-foreground transition-colors duration-150 hover:bg-muted"
          >
            <CaretLeft className="size-4" aria-hidden />
          </button>
          <button
            onClick={() => setView({ y: now.getFullYear(), m: now.getMonth() })}
            className="cursor-pointer rounded-md px-2.5 py-1 text-xs font-semibold text-foreground transition-colors duration-150 hover:bg-muted"
          >
            今
          </button>
          <button
            onClick={() => shift(1)}
            aria-label="下一月"
            className="grid size-8 cursor-pointer place-items-center rounded-md text-foreground transition-colors duration-150 hover:bg-muted"
          >
            <CaretRight className="size-4" aria-hidden />
          </button>
        </div>
      </header>

      {err ? (
        <p role="alert" className="px-3 py-4 text-sm text-destructive">
          日历加载失败：{err}
        </p>
      ) : events === null ? (
        <div className="space-y-2 p-3.5">
          <Skeleton className="h-40 w-full rounded-lg" />
        </div>
      ) : (
        <>
          {/* 月网格（≥640px） */}
          <div className="hidden px-2 pb-2 sm:block">
            <div className="grid grid-cols-7">
              {WEEKDAYS.map((w) => (
                <div key={w} className="py-1.5 text-center text-xs font-medium text-muted-foreground">
                  {w}
                </div>
              ))}
            </div>
            <div className="grid grid-cols-7">
              {cells.map(({ date, inMonth }, i) => {
                const key = formatDate(date)
                const dayEvents = byDate.get(key) ?? []
                const isToday = key === todayKey
                const isSelected = key === selected
                return (
                  <button
                    key={i}
                    type="button"
                    onClick={() => setSelected(key)}
                    aria-label={`${key} 日程 ${dayEvents.length} 项`}
                    aria-pressed={isSelected}
                    className={cn(
                      'flex min-h-[60px] flex-col items-center border-t border-border p-1 text-center transition-colors lg:min-h-[68px]',
                      !inMonth && 'bg-muted/30',
                      isSelected ? 'chip-accent-selected-strong' : 'hover:bg-muted/60',
                    )}
                  >
                    <span
                      className={cn(
                        'inline-grid size-5 place-items-center rounded-full text-xs tabular-nums',
                        isToday
                          ? 'bg-primary font-semibold text-primary-foreground'
                          : inMonth
                            ? 'text-foreground'
                            : 'text-muted-foreground',
                      )}
                    >
                      {date.getDate()}
                    </span>
                    {dayEvents.slice(0, 2).map((e) => (
                      <span
                        key={e.id}
                        title={eventLabel(e)}
                        className={cn(
                          'mt-0.5 w-full truncate rounded px-1 py-px text-center text-[11px] leading-4 font-medium',
                          passedStyle(e.passed),
                        )}
                      >
                        {e.name}
                      </span>
                    ))}
                    {dayEvents.length > 2 && (
                      <span className="mt-0.5 block w-full px-1 text-center text-[11px] text-muted-foreground">
                        +{dayEvents.length - 2}
                      </span>
                    )}
                  </button>
                )
              })}
            </div>
            {monthEvents.length === 0 && (
              <p className="border-t border-border py-3 text-center text-xs text-muted-foreground">
                本月无面试日程
              </p>
            )}

            {/* 选中日议程：格子太小看不全，聚合到此处完整展示；条目点击即跳轮次详情 */}
            <div className="border-t border-border px-3 py-2">
              <h3 className="text-xs font-medium text-muted-foreground">{selected} 日程</h3>
              {(byDate.get(selected)?.length ?? 0) === 0 ? (
                <p className="py-2 text-xs text-muted-foreground">当天无面试安排</p>
              ) : (
                <ul className="mt-1 divide-y divide-border">
                  {(byDate.get(selected) ?? []).map((e) => (
                    <li key={e.id}>
                      <Link
                        to={`/rounds/${e.id}`}
                        className="flex items-center gap-2 py-1.5 group"
                        title={eventLabel(e)}
                      >
                        <span
                          className={cn(
                            'size-2 shrink-0 rounded-full',
                            e.passed === 'pass' ? 'bg-success' : e.passed === 'fail' ? 'bg-destructive' : 'bg-info',
                          )}
                          aria-hidden
                        />
                        <span className="truncate text-sm font-medium group-hover:text-primary">{e.name}</span>
                        <span className="ml-auto truncate text-xs text-muted-foreground">
                          {[e.company, e.position].filter(Boolean).join(' · ')}
                        </span>
                      </Link>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>

          <ul className="divide-y divide-border sm:hidden">
            {monthEvents.length === 0 ? (
              <li className="px-3.5 py-4 text-sm text-foreground">本月无面试日程</li>
            ) : (
              [...monthEvents]
                .sort((a, b) => a.date.localeCompare(b.date))
                .map((e) => (
                <li key={e.id}>
                  <Link to={`/rounds/${e.id}`} className="flex min-h-[44px] items-center gap-2.5 px-3.5 py-3 transition-colors duration-150 hover:bg-muted/50">
                    <span className="w-12 shrink-0 font-mono text-xs tabular-nums font-semibold text-foreground">
                      {e.date.slice(5)}
                    </span>
                    <span className={cn('truncate text-sm font-medium flex-1', passedStyle(e.passed).split(' ')[1])}>
                      {e.name}
                    </span>
                    <span className="truncate text-xs text-muted-foreground">{e.company}</span>
                    <CaretRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                  </Link>
                </li>
              ))
            )}
          </ul>
        </>
      )}
    </section>
  )
}

function formatDate(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, '0')
  const day = String(d.getDate()).padStart(2, '0')
  return `${d.getFullYear()}-${m}-${day}`
}
