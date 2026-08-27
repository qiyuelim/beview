import { useCallback, useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { BookOpen, CheckCircle, Sparkle } from '@phosphor-icons/react'
import { apiGet, apiPost, apiStream } from '../api/client'
import type { DailyProgress, ReviewQueueItem, ReviewStats } from '../api/types'
import Markdown from '../components/Markdown'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'

type Eval = 'remembered' | 'fuzzy' | 'forgot'
const EVAL_LABEL: Record<Eval, string> = { remembered: '记得', fuzzy: '模糊', forgot: '忘了' }

const OFFLINE_QUEUE_KEY = 'beview_offline_queue'
const PENDING_GRADES_KEY = 'beview_offline_pending_grades'

export default function Review() {
  const [queue, setQueue] = useState<ReviewQueueItem[]>([])
  const [stats, setStats] = useState<ReviewStats | null>(null)
  const [daily, setDaily] = useState<DailyProgress | null>(null)
  const [err, setErr] = useState('')
  const [idx, setIdx] = useState(0)
  const [flipped, setFlipped] = useState(false)
  const [recall, setRecall] = useState('')
  const [doneCount, setDoneCount] = useState(0)
  const [explain, setExplain] = useState<string | null>(null) // null=未开, ''=流式中, 文本=内容
  const [explainErr, setExplainErr] = useState('')
  const [reconnecting, setReconnecting] = useState(false)
  const [resetOpen, setResetOpen] = useState(false)
  const [isOffline, setIsOffline] = useState(!navigator.onLine)
  const explainRunning = useRef(false)

  // 离线打分同步
  const syncOfflineGrades = useCallback(async () => {
    try {
      const raw = localStorage.getItem(PENDING_GRADES_KEY)
      if (!raw) return
      const pending: Array<{ qid: number; result: Eval; answer?: string }> = JSON.parse(raw)
      if (pending.length === 0) return

      const remaining = []
      for (const item of pending) {
        try {
          await apiPost(`/api/review/${item.qid}/grade`, {
            result: item.result,
            answer: item.answer,
          })
        } catch {
          remaining.push(item)
        }
      }
      if (remaining.length > 0) {
        localStorage.setItem(PENDING_GRADES_KEY, JSON.stringify(remaining))
      } else {
        localStorage.removeItem(PENDING_GRADES_KEY)
        toast.success('已恢复网络，离线复习记录已自动同步')
      }
    } catch {
      // 忽略同步解析错误
    }
  }, [])

  const load = useCallback(async () => {
    try {
      const [q, s, d] = await Promise.all([
        apiGet('/api/review/queue'),
        apiGet('/api/review/stats'),
        apiGet('/api/points/daily'),
      ])
      setQueue(q)
      setStats(s)
      setDaily(d)
      setIdx(0)
      setFlipped(false)
      setRecall('')
      setIsOffline(false)
      // 缓存到本地供离线使用
      localStorage.setItem(OFFLINE_QUEUE_KEY, JSON.stringify(q))
      syncOfflineGrades()
    } catch {
      // 离线降级
      setIsOffline(true)
      const cached = localStorage.getItem(OFFLINE_QUEUE_KEY)
      if (cached) {
        setQueue(JSON.parse(cached))
      }
    }
  }, [syncOfflineGrades])

  useEffect(() => {
    load().catch((e) => setErr(e.message))

    const handleOnline = () => {
      setIsOffline(false)
      syncOfflineGrades()
      load()
    }
    const handleOffline = () => setIsOffline(true)

    window.addEventListener('online', handleOnline)
    window.addEventListener('offline', handleOffline)
    return () => {
      window.removeEventListener('online', handleOnline)
      window.removeEventListener('offline', handleOffline)
    }
  }, [load, syncOfflineGrades])

  const card = queue[idx] || null

  async function grade(result: Eval) {
    if (!card) return

    if (isOffline || !navigator.onLine) {
      // 离线暂存
      const raw = localStorage.getItem(PENDING_GRADES_KEY) || '[]'
      const list = JSON.parse(raw)
      list.push({ qid: card.question_id, result, answer: recall.trim() || undefined })
      localStorage.setItem(PENDING_GRADES_KEY, JSON.stringify(list))
      toast.info(`已离线记录「${EVAL_LABEL[result]}」，联网后自动同步`)
    } else {
      try {
        const r = await apiPost(`/api/review/${card.question_id}/grade`, {
          result,
          answer: recall.trim() || undefined,
        })
        toast.success(`「${EVAL_LABEL[result]}」→ ${r.interval_days} 天后再复习`)
      } catch {
        // 请求失败自动进入离线暂存
        const raw = localStorage.getItem(PENDING_GRADES_KEY) || '[]'
        const list = JSON.parse(raw)
        list.push({ qid: card.question_id, result, answer: recall.trim() || undefined })
        localStorage.setItem(PENDING_GRADES_KEY, JSON.stringify(list))
        toast.info(`网络波动，已离线记录「${EVAL_LABEL[result]}」`)
      }
    }

    setDoneCount((n) => n + 1)
    setExplain(null)
    setExplainErr('')
    if (idx + 1 < queue.length) {
      setIdx(idx + 1)
      setFlipped(false)
      setRecall('')
    } else {
      setIdx(0)
      setQueue([])
      setFlipped(false)
      setRecall('')
    }
  }

  async function resetAll() {
    try {
      await apiPost('/api/review/reset')
      toast.success('已全部重置，队列已更新')
      setResetOpen(false)
      load().catch(() => {})
    } catch (e: any) {
      setErr(e.message)
      setResetOpen(false)
    }
  }

  async function runExplain() {
    if (!card || explainRunning.current) return
    explainRunning.current = true
    setExplain('')
    setExplainErr('')
    try {
      await apiStream(
        `/api/review/${card.question_id}/explain`,
        { focus: recall.trim() || undefined },
        (ev, data) => {
          if (ev === 'delta') {
            try {
              const d = JSON.parse(data)
              setExplain((prev) => (prev === null ? '' : prev) + (d.text || ''))
            } catch {
              /* ignore */
            }
          } else if (ev === 'error') {
            try {
              setExplainErr(JSON.parse(data).message || '讲解失败')
            } catch {
              setExplainErr('讲解失败')
            }
          }
        },
        {
          onReconnect: (attempt) => {
            setReconnecting(true)
            setExplainErr(`连接中断，正在重连（第 ${attempt} 次）…`)
          },
        },
      )
    } catch (e: any) {
      setExplainErr(e.message)
    } finally {
      explainRunning.current = false
      setReconnecting(false)
    }
  }

  const due = stats?.due ?? 0
  const done = (stats?.done_today ?? 0) + doneCount
  const memRate = (() => {
    const total = (stats?.remembered ?? 0) + (stats?.fuzzy ?? 0) + (stats?.forgot ?? 0)
    return total ? Math.round(((stats?.remembered ?? 0) / total) * 100) : null
  })()

  return (
    <div className="mx-auto w-full max-w-[640px]">
      <PageHeader
        title="复习"
        meta={
          <>
            <span>
              今日待复习 <b className="font-mono tabular-nums">{due}</b> · 已完成{' '}
              <b className="font-mono tabular-nums">{done}</b>
            </span>
            {memRate !== null && (
              <span>
                · 记忆率 <b className="font-mono tabular-nums">{memRate}%</b>
              </span>
            )}
            {stats?.streak_days ? (
              <span>
                · 连续 <b className="font-mono tabular-nums">{stats.streak_days}</b> 天
              </span>
            ) : null}
          </>
        }
        actions={
          <>
            <Button size="sm" variant="ghost" className="text-destructive hover:bg-destructive/10" onClick={() => setResetOpen(true)} title="全部重置复习间隔（维护操作）">
              全部重置
            </Button>
            <Button size="sm" variant="ghost" asChild>
              <Link to="/review/wrong">错题本</Link>
            </Button>
          </>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {daily && (
        <Section title="今日任务 · 积分" className="mb-3" sub={<span>真实面试另计：录题 +100/题、建批次 +300、轮次通过 +200</span>}>
          <div className="space-y-1">
            <div className="flex items-baseline justify-between text-sm">
              <span>复习一张卡（+5）</span>
              <b className="font-mono tabular-nums">{daily.cards_today} 张</b>
            </div>
            <div className="flex items-baseline justify-between text-sm">
              <span>今日队列 100%（+20）</span>
              {daily.queue_done ? (
                <b className="font-mono tabular-nums text-success">✓ 已达标</b>
              ) : (
                <b className="font-mono tabular-nums">
                  {daily.done_today} / {Math.max(daily.due_today, 1)}
                </b>
              )}
            </div>
            <div className="flex items-baseline justify-between text-sm">
              <span>完成一场陪练（+30）</span>
              <b className="font-mono tabular-nums">{daily.drills_today} 场</b>
            </div>
          </div>
        </Section>
      )}

      {isOffline && (
        <div className="mb-3 rounded-md bg-warning/10 px-3 py-2 text-xs font-semibold text-warning">
          ⚡ 当前处于离线复习模式，自评记录将在恢复联网后自动同步
        </div>
      )}

      <div
        className="mb-3 h-2 overflow-hidden rounded-full bg-muted"
        role="progressbar"
        aria-label="今日进度"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={due ? Math.min(100, Math.round((done / Math.max(due, 1)) * 100)) : 0}
      >
        <div
          className="h-full rounded-full bg-primary transition-[width]"
          style={{ width: `${due ? Math.min(100, Math.round((done / Math.max(due, 1)) * 100)) : 0}%` }}
        />
      </div>

      {!card ? (
        <Section className="py-8">
          <div className="flex flex-col items-center gap-2 text-center">
            <CheckCircle className="size-8 text-success" weight="fill" aria-hidden />
            {due === 0 ? (
              <>
                <p className="text-sm font-medium">今日复习队列已清空。</p>
                <p className="text-sm text-muted-foreground">
                  去 <Link to="/questions" className="text-primary underline underline-offset-2">题目</Link>{' '}
                  分析几道新题，或去 <Link to="/review/wrong" className="text-primary underline underline-offset-2">错题本</Link>{' '}
                  巩固一下。
                </p>
              </>
            ) : (
              <>
                <p className="text-sm font-medium">正在刷新队列…</p>
                <Button onClick={() => load().catch((e) => setErr(e.message))}>再刷一次</Button>
              </>
            )}
          </div>
        </Section>
      ) : (
        <div className="relative">
          <Section key={card.question_id}>
            {/* 卡元信息 */}
            <div className="mb-2.5 flex flex-wrap items-center gap-1.5 text-xs">
              <span className="rounded-full bg-muted px-2 py-0.5">{card.company || '模拟'}</span>
              {card.difficulty != null && (
                <span className="rounded-full bg-muted px-2 py-0.5 text-warning">{'★'.repeat(card.difficulty)}</span>
              )}
              {card.source === 'ai_drill' && (
                <span className="rounded-full bg-secondary px-2 py-0.5 text-secondary-foreground">AI 模拟</span>
              )}
              {card.tags.map((t) => (
                <span key={t} className="rounded-full bg-muted px-2 py-0.5 text-muted-foreground">
                  {t}
                </span>
              ))}
              <span className="ml-auto font-mono tabular-nums text-muted-foreground">
                {idx + 1} / {queue.length}
              </span>
            </div>

            {!flipped ? (
              <div className="space-y-3">
                <div
                  className="max-h-[45vh] overflow-y-auto overscroll-contain touch-auto pr-1"
                >
                  <h2 className="text-base font-semibold leading-7">{card.content}</h2>
                </div>
                <Textarea
                  placeholder="主动回忆：先试着在心里或这里写下要点（可不填）"
                  value={recall}
                  onChange={(e) => setRecall(e.target.value)}
                  rows={4}
                />
                <div className="grid grid-cols-2 gap-2 sm:flex sm:flex-wrap sm:items-center">
                  <Button className="h-12 sm:h-9 sm:w-auto" onClick={() => setFlipped(true)}>翻面看答案</Button>
                  <Button variant="ghost" className="h-12 sm:h-9 sm:w-auto" onClick={runExplain} disabled={explainRunning.current}>
                    <Sparkle className="size-4" aria-hidden /> AI 讲解
                  </Button>
                </div>
              </div>
            ) : (
              <div className="space-y-3">
                <div
                  className="max-h-[45vh] overflow-y-auto overscroll-contain touch-auto pr-1"
                >
                  {card.ref_answer ? (
                    <>
                      <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">参考答案</div>
                      <div className="text-sm leading-7">
                        <Markdown text={card.ref_answer} />
                      </div>
                    </>
                  ) : (
                    <p className="text-sm text-muted-foreground">
                      该题暂无参考答案——可点「AI 讲解」生成，或去{' '}
                      <Link to={`/questions/${card.question_id}`} className="text-primary hover:underline">
                        题目详情
                      </Link>{' '}
                      补分析
                    </p>
                  )}
                </div>
                <div className="grid grid-cols-3 gap-2 sm:flex sm:flex-wrap sm:items-center">
                  <Button size="default" className="h-12 sm:h-9 border border-success/40 bg-success/10 text-success hover:bg-success/20 sm:w-auto" onClick={() => grade('remembered')}>
                    记得
                  </Button>
                  <Button size="default" className="h-12 sm:h-9 border border-warning/40 bg-warning/10 text-warning hover:bg-warning/20 sm:w-auto" onClick={() => grade('fuzzy')}>
                    模糊
                  </Button>
                  <Button size="default" variant="destructive" className="h-12 sm:h-9 sm:w-auto" onClick={() => grade('forgot')}>
                    忘了
                  </Button>
                  <Button size="default" variant="ghost" className="col-span-3 sm:col-span-1 h-12 sm:h-9 sm:ml-auto sm:w-auto" onClick={runExplain} disabled={explainRunning.current}>
                    <Sparkle className="size-4" aria-hidden /> AI 讲解
                  </Button>
                </div>
              </div>
            )}

            {explain !== null && (
              <div className="mt-3 rounded-md bg-muted/50 p-3">
                <div className="mb-1 flex items-center gap-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  <BookOpen className="size-3.5" aria-hidden /> AI 讲解
                </div>
                {explainErr && (
                  <p role="alert" className="mb-1 text-sm font-medium text-destructive">
                    {explainErr}
                  </p>
                )}
                {reconnecting && <p className="text-xs text-muted-foreground">已重连，正在恢复输出…</p>}
                {explain === '' && !explainErr && <p className="text-sm text-muted-foreground">正在讲解…</p>}
                {explain !== '' && (
                  <div className="text-sm leading-7">
                    <Markdown text={explain} />
                    <span className="inline-block h-4 w-0.5 animate-pulse bg-primary align-middle" aria-hidden />
                  </div>
                )}
              </div>
            )}
          </Section>
        </div>
      )}

      <ConfirmDialog
        open={resetOpen}
        onOpenChange={setResetOpen}
        destructive
        title="全部重置复习队列？"
        description="所有题目的复习间隔回到 1 天、立即到期。"
        confirmLabel="全部重置"
        onConfirm={resetAll}
      />
    </div>
  )
}
