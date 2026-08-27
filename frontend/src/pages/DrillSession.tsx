import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { apiGet, apiStream } from '../api/client'
import type { DrillDetail, DrillMessage } from '../api/types'
import { onAiEvent, startAiJob, trackRunning } from '../ai/jobs'
import { Sparkle } from '@phosphor-icons/react'
import Markdown from '../components/Markdown'
import { PageHeader } from '../components/PageHeader'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { getStoredStreamSpeedRate, setStoredStreamSpeedRate, useSmoothStream } from '../hooks/useSmoothStream'

const KIND_LABEL: Record<string, string> = { interview: '模拟面试' }

export default function DrillSession() {
  const { id } = useParams()
  const [d, setD] = useState<DrillDetail | null>(null)
  const [err, setErr] = useState('')
  const [input, setInput] = useState('')
  const [busy, setBusy] = useState(false)
  const [speedRate, setSpeedRate] = useState<number>(() => getStoredStreamSpeedRate())
  const smooth = useSmoothStream({ rateCharsPerSec: speedRate })
  const [activeAction, setActiveAction] = useState<string>('')
  const [thinking, setThinking] = useState('') // 思考过程增量（reasoning_content，可折叠展示）
  const [pendingMsg, setPendingMsg] = useState('') // 用户刚发送的消息：立即上屏，AI 回复后由 load() 接管
  const [reconnecting, setReconnecting] = useState(false)
  const [usedHintLevel, setUsedHintLevel] = useState<number>(0)
  const [notesOpen, setNotesOpen] = useState(false)

  const prepRunning = !!d?.ai_jobs?.some((j) => j.kind === 'interview_prep')
  // M4：报告流后置——回答轮先流出续接，「考官即时点评」占位卡由后置 feedback 事件原位填充
  const [liveEval, setLiveEval] = useState<{ score: number | null; feedback: string } | null>(null)
  const loadedId = useRef<number | null>(null) // 当前已加载的场次 id：切换场次时重置本地状态防串场
  const scrollRef = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    const dd = await apiGet(`/api/drills/${id}`)
    if (loadedId.current !== dd.id) {
      loadedId.current = dd.id
      setThinking('')
    }
    setD(dd)
    trackRunning(dd.ai_jobs) // 刷新/重进时恢复「备课中」跟踪（ADR-0013 D3）
  }, [id])

  // 面试官笔记（V6-M3）：统一走 startAiJob 受理（同目标重复触发由后端幂等去重）
  const startPrep = useCallback(async () => {
    if (!id) return
    try {
      await startAiJob('interview_prep', Number(id), `/api/drills/${id}/interview_prep`)
      await load()
    } catch (e: unknown) {
      setErr(e instanceof Error ? e.message : String(e))
    }
  }, [id, load])

  // 笔记终态：事件驱动（ADR-0013）——后端 ai_jobs 收尾时经 SSE 广播 interview_prep done/failed，
  // 订阅即停等待并刷新；下方轮询仅作 SSE 断线兑底（与 paper_grading 同款双保险）。
  const drillId = Number(id)
  useEffect(() => {
    if (!Number.isFinite(drillId)) return
    return onAiEvent((ev) => {
      if (ev.kind === 'interview_prep' && ev.target_id === drillId && ev.status !== 'running') {
        load().catch(() => {})
      }
    })
  }, [drillId, load])

  useEffect(() => {
    if (!prepRunning) return
    let cancelled = false
    ;(async () => {
      for (;;) {
        if (cancelled) return
        await new Promise((r) => setTimeout(r, 1500))
        if (cancelled) return
        try {
          await load()
          if (!d?.ai_jobs?.some((j) => j.kind === 'interview_prep')) return
        } catch {
          /* 断线重试 */
        }
      }
    })()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prepRunning])


  useEffect(() => {
    load().catch((e) => setErr(e.message))
  }, [load])

  useEffect(() => {
    if (!d) return
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight })
  }, [d?.messages.length, smooth.displayedText, thinking])

  if (!d) {
    return <div className="py-24 text-center text-muted-foreground">{err || '加载中…'}</div>
  }

  async function startInterview() {
    if (busy || !d || d.status !== 'ongoing') return
    setBusy(true)
    smooth.clear()
    setThinking('')
    setErr('')
    setUsedHintLevel(0)
    setActiveAction('start')
    try {
      await apiStream(
        `/api/drills/${id}/messages`,
        { content: '', action: 'start' },
        (ev, data) => {
          if (ev === 'delta') {
            try {
              const dd = JSON.parse(data)
              if (dd.text) smooth.appendChunk(dd.text)
            } catch {
              /* ignore */
            }
          } else if (ev === 'thinking') {
            try {
              const dd = JSON.parse(data)
              setThinking((prev) => prev + (dd.text || ''))
            } catch {
              /* ignore */
            }
          } else if (ev === 'error') {
            try {
              setErr(JSON.parse(data).message || '出题失败')
            } catch {
              setErr('出题失败')
            }
          }
        },
        {
          onReconnect: (attempt) => {
            setReconnecting(true)
            setErr(`连接中断，正在重连（第 ${attempt} 次）…`)
          },
        },
      )
      smooth.finishStream()
      await smooth.waitUntilDrained()
      await load()
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
      setActiveAction('')
      smooth.clear()
      setThinking('')
      setReconnecting(false)
    }
  }

  async function triggerAction(action: 'hint' | 'finish' | 'skip') {
    if (busy || !d || d.status !== 'ongoing') return
    setBusy(true)
    smooth.clear()
    setThinking('')
    setErr('')
    setActiveAction(action)
    if (action === 'hint') {
      setUsedHintLevel((prev) => Math.max(prev, 1))
    }
    try {
      await apiStream(
        `/api/drills/${id}/messages`,
        { content: '', action },
        (ev, data) => {
          if (ev === 'delta') {
            try {
              const dd = JSON.parse(data)
              if (dd.text) smooth.appendChunk(dd.text)
            } catch {
              /* ignore */
            }
          } else if (ev === 'thinking') {
            try {
              const dd = JSON.parse(data)
              setThinking((prev) => prev + (dd.text || ''))
            } catch {
              /* ignore */
            }
          } else if (ev === 'error') {
            try {
              setErr(JSON.parse(data).message || '请求失败')
            } catch {
              setErr('请求失败')
            }
          }
        },
        {
          onReconnect: (attempt) => {
            setReconnecting(true)
            setErr(`连接中断，正在重连（第 ${attempt} 次）…`)
          },
        },
      )
      smooth.finishStream()
      await smooth.waitUntilDrained()
      await load()
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
      setActiveAction('')
      smooth.clear()
      setThinking('')
      setReconnecting(false)
    }
  }

  async function send() {
    const text = input.trim()
    if (!text || busy || !d || d.status !== 'ongoing') return
    setInput('')
    setBusy(true)
    smooth.clear()
    setThinking('')
    setPendingMsg(text) // 立即上屏用户消息，不等 AI 回复
    setErr('')
    setLiveEval(null)
    const currentHintLevel = usedHintLevel
    try {
      await apiStream(
        `/api/drills/${id}/messages`,
        { content: text, action: 'answer', hint_level: currentHintLevel },
        (ev, data) => {
          if (ev === 'delta') {
            try {
              const dd = JSON.parse(data)
              if (dd.text) smooth.appendChunk(dd.text)
            } catch {
              /* ignore */
            }
          } else if (ev === 'feedback') {
            // M4：评分点评为后置报告流，原位填充评估卡（不打断续接阅读）
            try {
              const dd = JSON.parse(data)
              setLiveEval({ score: dd.score ?? null, feedback: dd.feedback || '' })
            } catch {
              /* ignore */
            }
          } else if (ev === 'thinking') {
            try {
              const dd = JSON.parse(data)
              setThinking((prev) => prev + (dd.text || ''))
            } catch {
              /* ignore */
            }
          } else if (ev === 'error') {
            try {
              setErr(JSON.parse(data).message || 'AI 回复失败')
            } catch {
              setErr('AI 回复失败')
            }
          }
        },
        {
          onReconnect: (attempt) => {
            setReconnecting(true)
            setErr(`连接中断，正在重连（第 ${attempt} 次）…`)
          },
        },
      )
      smooth.finishStream()
      await smooth.waitUntilDrained()
      setPendingMsg('')
      await load()
    } catch (e: any) {
      setPendingMsg('')
      setErr(e.message)
    } finally {
      setBusy(false)
      setActiveAction('')
      smooth.clear()
      setThinking('')
      setReconnecting(false)
    }
  }

  const chatSummary = d.messages.find((m) => m.kind === 'summary')?.content || ''

  return (
    <div className="mx-auto w-full max-w-[760px]">
      <nav aria-label="面包屑" className="mb-2 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link to="/drills" className="hover:text-primary">
          陪练
        </Link>
        <span aria-hidden>/</span>
        <span className="text-foreground">{d.title}</span>
      </nav>

      <PageHeader
        title={d.title}
        meta={
          <>
            {d.title !== KIND_LABEL[d.kind] && (
              <span className="rounded-full bg-muted px-2 py-0.5 text-xs">{KIND_LABEL[d.kind] || d.kind}</span>
            )}
            {d.position && <span>{d.position}</span>}
            {d.persona_label && (
              <span className="rounded-full bg-accent/15 px-2 py-0.5 text-xs text-accent">
                🎭 {d.persona_label}
              </span>
            )}
            {d.direction && <span>方向 {d.direction}</span>}
            {d.status === 'finished' && (
              <span className="font-medium text-success">
                已完成{d.score != null ? ` · 总分 ${d.score}` : ''}
              </span>
            )}
            {d.status === 'ongoing' && <span className="font-medium text-info">进行中</span>}
          </>
        }
        actions={
          <div className="flex items-center gap-2">
            <div className="flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-xs text-muted-foreground shadow-sm">
              <span>⚡ 语速</span>
              <input
                type="range"
                min="10"
                max="210"
                step="10"
                value={speedRate}
                onChange={(e) => {
                  const v = Number(e.target.value)
                  setSpeedRate(v)
                  setStoredStreamSpeedRate(v)
                }}
                className="h-1.5 w-16 sm:w-24 cursor-pointer accent-primary"
                title="调整 AI 吐字语速（10~200 字/秒，最右端为无限制立即输出）"
              />
              <span className="font-mono text-[11px] w-12 sm:w-14 text-right text-foreground font-medium">
                {speedRate >= 210 ? '∞ 无限制' : `${speedRate} 字/s`}
              </span>
            </div>
            <Button size="sm" variant="outline" asChild>
              <a href={`/api/drills/${id}/transcript`} download>
                导出对话
              </a>
            </Button>
          </div>
        }
      />
      {d.kind === 'interview' && (
        <section aria-label="面试官笔记" className="mb-3 flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border bg-card px-3 py-2">
          <div className="min-w-0 text-sm">
            <span className="font-medium text-foreground">{d.persona_label ?? '面试官'}</span>
            {d.interview_state ? (
              <span className="ml-2 rounded-full border border-border px-2 py-0.5 text-xs">
                面试官已生成 {[d.interview_state.job_requirements, d.interview_state.candidate_facts, d.interview_state.risk_signals, d.interview_state.next_followups].filter((a) => a && a.length > 0).length} 项考点笔记
              </span>
            ) : (
              <span className="ml-2 text-xs text-muted-foreground">{prepRunning ? '备课中…' : '尚未生成笔记'}</span>
            )}
          </div>
          <div className="flex items-center gap-2">
            {d.interview_state && (
              <Button size="sm" variant="outline" className="h-8" onClick={() => setNotesOpen(true)}>
                查看笔记
              </Button>
            )}
            <Button size="sm" variant="ghost" className="h-8" onClick={startPrep} disabled={prepRunning}>
              {prepRunning ? '备课中…' : d.interview_state ? '重新生成' : '生成笔记'}
            </Button>
          </div>
        </section>
      )}
      {notesOpen && d.interview_state && (
        <div className="fixed inset-0 z-50 flex justify-end bg-black/50" onClick={() => setNotesOpen(false)}>
          <div
            role="dialog"
            aria-label="面试官笔记"
            className="flex h-full w-full max-w-md flex-col border-l border-border bg-card shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between border-b border-border px-4 py-3">
              <h3 className="text-sm font-semibold">面试官笔记</h3>
              <Button size="sm" variant="ghost" onClick={() => setNotesOpen(false)}>关闭</Button>
            </div>
            <div className="flex-1 space-y-4 overflow-y-auto p-4">
              {([
                ['岗位要求', 'job_requirements'],
                ['候选人事实', 'candidate_facts'],
                ['风险信号', 'risk_signals'],
                ['建议追问', 'next_followups'],
              ] as const).map(([label, key]) => {
                const items = d.interview_state?.[key] ?? []
                if (items.length === 0) return null
                return (
                  <div key={key}>
                    <p className="text-sm font-semibold text-foreground">{label}</p>
                    <ul className="mt-1 list-disc space-y-1 pl-4 text-sm leading-6">
                      {items.map((it, i) => (
                        <li key={i}>{it}</li>
                      ))}
                    </ul>
                  </div>
                )
              })}
            </div>
          </div>
        </div>
      )}
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {/* 考官专属题本 (Interviewer Dossier) 预览 */}
      {d.dossier && (
        <details className="mb-3 rounded-lg border border-border bg-card p-3 text-xs leading-6">
          <summary className="cursor-pointer font-bold text-foreground hover:text-primary">
            📋 考官专属参考题本 (Interviewer Dossier)
            {d.dossier.questions && ` · 包含 ${d.dossier.questions.length} 道针对性真题`}
          </summary>
          <div className="mt-2 space-y-2 text-muted-foreground border-t border-border pt-2">
            {d.dossier.summary && (
              <div>
                <span className="font-semibold text-foreground">考核侧重：</span>
                {d.dossier.summary}
              </div>
            )}
            {d.dossier.questions && d.dossier.questions.length > 0 && (
              <div className="space-y-1.5">
                <span className="font-semibold text-foreground">重点参考题目：</span>
                <ul className="list-inside list-decimal space-y-1 pl-1">
                  {d.dossier.questions.map((q, idx) => (
                    <li key={idx}>
                      <span className="text-foreground">{q.content}</span>
                      {q.ref_answer && (
                        <div className="pl-4 text-[11px] text-muted-foreground/80">
                          参考标准：{q.ref_answer}
                        </div>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        </details>
      )}

          <div className="rounded-lg border border-border bg-card">
            <div ref={scrollRef} className="max-h-[60vh] space-y-3 overflow-y-auto p-3">
              {d.messages
                .filter((m) => m.kind !== 'start' && m.kind !== 'control' && !(m.kind === 'answer' && (m.content.trim() === '开始' || m.content.trim() === 'start')))
                .map((m) => (
                  <ChatBubble
                    key={m.id}
                    m={m}
                    onUnlockHintLevel={(lvl) => setUsedHintLevel((prev) => Math.max(prev, lvl))}
                  />
                ))}
              {pendingMsg && (
                <div className="flex justify-end">
                  <div className="max-w-[80%] whitespace-pre-wrap rounded-lg bg-muted px-3 py-2 text-sm leading-6">
                    {pendingMsg}
                  </div>
                </div>
              )}
              {/* 思考中微型状态指示器（彻底隐藏思维链冗余文本） */}
              {(busy || thinking) && !smooth.displayedText && (
                <div className="flex items-center gap-2 text-xs text-muted-foreground py-1">
                  <AiAvatar />
                  <div className="flex items-center gap-2 rounded-full border border-border/60 bg-muted/40 px-3 py-1 animate-pulse">
                    <span className="inline-block size-1.5 rounded-full bg-primary" />
                    <span>{thinking ? '面试官正在推演考点与追问逻辑…' : '面试官正在评估你的作答…'}</span>
                  </div>
                </div>
              )}
              {smooth.displayedText && (
                activeAction === 'hint' ? (
                  <HintCard
                    content={smooth.displayedText}
                    onUnlockLevel={(lvl) => setUsedHintLevel((prev) => Math.max(prev, lvl))}
                  />
                ) : activeAction === 'finish' ? null : (
                  <div className="flex items-start gap-2">
                    <AiAvatar />
                    <div className="min-w-0 flex-1">
                      <div className="mb-0.5 text-xs font-semibold text-muted-foreground">面试官</div>
                      <div className="text-sm leading-7">
                        <Markdown text={smooth.displayedText} />
                        <span className="inline-block h-4 w-0.5 animate-pulse bg-primary align-middle" aria-hidden />
                      </div>
                    </div>
                  </div>
                )
              )}
              {/* M4：报告流占位卡——评估中… / 评估结果原位填充 */}
              {busy && activeAction === 'answer' && (
                liveEval ? (
                  <div className="flex items-start gap-2">
                    <AiAvatar />
                    <div className="rounded-xl bg-muted/60 border border-border p-3 text-xs shadow-sm">
                      <div className="flex items-center gap-1.5">
                        <span className="font-semibold text-primary">📝 考官即时点评</span>
                        {liveEval.score != null && (
                          <span
                            className={`font-mono font-bold px-1.5 py-0.5 rounded text-[10px] ${
                              liveEval.score >= 80
                                ? 'bg-success/15 text-success'
                                : liveEval.score >= 60
                                ? 'bg-warning/15 text-warning'
                                : 'bg-destructive/15 text-destructive'
                            }`}
                          >
                            {liveEval.score} 分
                          </span>
                        )}
                      </div>
                      <div className="mt-1.5 text-sm leading-7 text-foreground">{liveEval.feedback}</div>
                    </div>
                  </div>
                ) : (
                  <div className="ml-9 flex items-center gap-2 py-1 text-xs text-muted-foreground">
                    <span className="inline-block size-1.5 animate-pulse rounded-full bg-primary" aria-hidden />
                    考官评估中…
                  </div>
                )
              )}
              {d.messages.length === 0 && !smooth.displayedText && (
                <div className="flex flex-col items-center justify-center gap-3 py-14 text-center">
                  <div className="flex size-14 items-center justify-center rounded-full bg-accent/15 text-accent dark:bg-accent/25">
                    <Sparkle weight="fill" className="size-7" />
                  </div>
                  <div className="space-y-1">
                    <h3 className="text-base font-semibold">模拟面试已就绪</h3>
                    <p className="text-sm text-muted-foreground">
                      AI 面试官将围绕岗位核心考点与技能图谱，展开逐题深度专业考核与追问。
                    </p>
                  </div>
                  <Button size="lg" onClick={startInterview} disabled={busy} className="mt-2 gap-2 font-semibold">
                    <Sparkle weight="bold" className="size-4" />
                    {busy ? '面试官正在出题…' : '🚀 开始模拟面试'}
                  </Button>
                </div>
              )}
              {reconnecting && <div className="text-xs text-muted-foreground">已重连，正在恢复输出…</div>}
              {d.status === 'finished' && d.messages.length === 0 && (
                <div className="text-xs text-muted-foreground">本场已结束。</div>
              )}
            </div>
            {d.status === 'finished' ? (
              <div className="border-t border-border px-3 py-3 text-center text-sm text-muted-foreground">
                🎉 本场模拟面试已结束 · 完整复盘总结见下方报告
              </div>
            ) : (d.messages.length > 0 || smooth.displayedText) ? (
              <div className="border-t border-border p-3 sm:p-4 bg-card/50">
                {/* 移动端快捷动作胶囊 (≥40px 舒适触控) */}
                {d.messages.length > 0 && (
                  <div className="mb-2.5 flex flex-wrap items-center gap-2 overflow-x-auto pb-1">
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => triggerAction('hint')}
                      className="flex min-h-[40px] items-center gap-1.5 rounded-full border border-warning/40 bg-warning/10 px-3.5 py-2 text-xs font-semibold text-warning transition-all hover:bg-warning/20 active:scale-95 disabled:opacity-50"
                    >
                      💡 请求分级提示
                    </button>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => triggerAction('finish')}
                      className="flex min-h-[40px] items-center gap-1.5 rounded-full border border-border bg-muted/60 px-3.5 py-2 text-xs font-semibold text-muted-foreground transition-all hover:bg-muted hover:text-foreground active:scale-95 disabled:opacity-50"
                    >
                      🏁 结束并生成复盘
                    </button>
                  </div>
                )}

                <div className="flex items-end gap-2.5">
                  <Textarea
                    className="min-h-[50px] flex-1 rounded-xl text-base sm:text-sm leading-relaxed"
                    placeholder="输入你的回答…"
                    rows={2}
                    value={input}
                    disabled={busy}
                    onChange={(e) => setInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault()
                        send()
                      }
                    }}
                  />
                  <Button onClick={send} disabled={busy || !input.trim()} className="h-[50px] min-w-[72px] shrink-0 rounded-xl font-semibold shadow-sm">
                    发送
                  </Button>
                </div>
                <div className="mt-2 text-xs text-muted-foreground">
                  每题答完 AI 即时判分；低分题自动进错题本，AI 出的题自动进题库与复习队列。
                </div>
              </div>
            ) : null}
          </div>
          {/* 整场总结独立复盘卡片：状态机驱动（已沉淀总结或正在生成收尾总结） */}
          {(chatSummary || (activeAction === 'finish' && smooth.displayedText)) && (
            <section className="mt-4 rounded-xl border border-border bg-card p-5 shadow-sm" aria-label="本场复盘报告">
              <div className="flex items-center justify-between border-b border-border pb-3">
                <div className="flex items-center gap-2">
                  <span className="text-base">📋</span>
                  <h2 className="text-sm font-semibold text-foreground">本场复盘报告</h2>
                </div>
                <span className="rounded-md bg-success/10 px-2 py-0.5 font-mono text-xs font-semibold text-success">
                  {chatSummary ? `已完成 ${d.messages.filter((m) => m.kind === 'question').length} 题考核` : '正在生成复盘报告…'}
                </span>
              </div>
              <div className="mt-3 text-sm leading-7 text-foreground">
                <Markdown text={chatSummary || smooth.displayedText} />
                {!chatSummary && smooth.displayedText && (
                  <span className="inline-block h-4 w-0.5 animate-pulse bg-primary align-middle" aria-hidden />
                )}
              </div>
            </section>
          )}
    </div>
  )
}

/** 结构化 3 级阶梯式提示卡片 */
function HintCard({
  content,
  onUnlockLevel,
}: {
  content: string
  onUnlockLevel?: (lvl: number) => void
}) {
  const [unlocked, setUnlocked] = useState<number>(1)

  const sections = useMemo(() => {
    const l1Match = content.match(/###\s*Level 1[^\n]*\n([\s\S]*?)(?=###\s*Level 2|$)/i)
    const l2Match = content.match(/###\s*Level 2[^\n]*\n([\s\S]*?)(?=###\s*Level 3|$)/i)
    const l3Match = content.match(/###\s*Level 3[^\n]*\n([\s\S]*?)$/i)

    if (!l1Match && !l2Match && !l3Match) {
      return { l1: content, l2: '', l3: '', structured: false }
    }
    return {
      l1: l1Match ? l1Match[1].trim() : '',
      l2: l2Match ? l2Match[1].trim() : '',
      l3: l3Match ? l3Match[1].trim() : '',
      structured: true,
    }
  }, [content])

  function handleUnlock(lvl: number) {
    setUnlocked(lvl)
    onUnlockLevel?.(lvl)
  }

  if (!sections.structured) {
    return (
      <div className="rounded-xl border border-warning/30 bg-warning/10 p-3.5 text-xs leading-6 shadow-sm">
        <div className="mb-1 flex items-center gap-1.5 font-semibold text-warning">
          <span>💡 技术思考提示</span>
        </div>
        <Markdown text={content} />
      </div>
    )
  }

  return (
    <div className="rounded-xl border border-warning/30 bg-warning/5 p-3.5 text-xs leading-6 space-y-2.5 shadow-sm">
      <div className="flex items-center justify-between border-b border-warning/20 pb-1.5">
        <span className="font-semibold text-warning flex items-center gap-1">
          💡 三级阶梯式提示
        </span>
        <span className="text-[10px] text-muted-foreground font-mono">
          已解锁 {unlocked} / 3 层
        </span>
      </div>

      {/* Level 1: 思考方向 */}
      <div className="space-y-1">
        <div className="font-semibold text-foreground flex items-center gap-1">
          <span className="rounded bg-warning/20 px-1.5 py-0.2 font-mono text-[10px] text-warning">Level 1</span>
          <span>思考方向与切入点</span>
        </div>
        <div className="text-muted-foreground pl-1">
          <Markdown text={sections.l1} />
        </div>
      </div>

      {/* Level 2: 核心考点 */}
      {sections.l2 && (
        <div className="border-t border-border/50 pt-2 space-y-1">
          {unlocked >= 2 ? (
            <>
              <div className="font-semibold text-foreground flex items-center gap-1">
                <span className="rounded bg-warning/30 px-1.5 py-0.2 font-mono text-[10px] text-warning">Level 2</span>
                <span>核心考点与机制</span>
              </div>
              <div className="text-muted-foreground pl-1">
                <Markdown text={sections.l2} />
              </div>
            </>
          ) : (
            <div className="flex items-center justify-between py-1.5 bg-background/50 rounded-lg px-2.5 border border-border/40">
              <span className="text-muted-foreground text-[11px]">🔒 Level 2: 核心考点提示</span>
              <button
                type="button"
                onClick={() => handleUnlock(2)}
                className="flex min-h-[36px] items-center rounded-lg bg-warning/15 hover:bg-warning/25 px-2.5 py-1 text-[11px] font-medium text-warning transition-all active:scale-95"
              >
                🔓 解锁展开 (适度扣除思考分)
              </button>
            </div>
          )}
        </div>
      )}

      {/* Level 3: 关键解法 */}
      {sections.l3 && (
        <div className="border-t border-border/50 pt-2 space-y-1">
          {unlocked >= 3 ? (
            <>
              <div className="font-semibold text-foreground flex items-center gap-1">
                <span className="rounded bg-destructive/15 px-1.5 py-0.2 font-mono text-[10px] text-destructive">Level 3</span>
                <span>关键解法与骨架</span>
              </div>
              <div className="text-muted-foreground pl-1">
                <Markdown text={sections.l3} />
              </div>
            </>
          ) : unlocked === 2 ? (
            <div className="flex items-center justify-between py-1.5 bg-background/50 rounded-lg px-2.5 border border-border/40">
              <span className="text-muted-foreground text-[11px]">🔒 Level 3: 关键解法深层提示</span>
              <button
                type="button"
                onClick={() => handleUnlock(3)}
                className="flex min-h-[36px] items-center rounded-lg bg-destructive/15 hover:bg-destructive/25 px-2.5 py-1 text-[11px] font-medium text-destructive transition-all active:scale-95"
              >
                🔓 解锁展开 (主要解法依赖)
              </button>
            </div>
          ) : (
            <div className="flex items-center justify-between py-1.5 bg-background/30 rounded-lg px-2.5 text-muted-foreground/60 text-[11px]">
              <span>🔒 Level 3: 关键解法（需先解锁 Level 2）</span>
            </div>
          )}
        </div>
      )}
    </div>
  )
}


function AiAvatar() {
  return (
    <span className="grid size-7 shrink-0 place-items-center rounded-full bg-secondary text-secondary-foreground" aria-hidden>
      <Sparkle weight="fill" className="size-3.5" />
    </span>
  )
}

/** grok 式消息：用户右灰泡；AI 无框正文 + 小头像；总结不在线程内渲染 */
function ChatBubble({
  m,
  onUnlockHintLevel,
}: {
  m: DrillMessage
  onUnlockHintLevel?: (lvl: number) => void
}) {
  if (m.role === 'user') {
    if (m.kind === 'start' || m.kind === 'control' || m.content.trim() === '开始' || m.content.trim() === 'start') return null
    return (
      <div className="flex flex-col items-end gap-1.5">
        <div className="max-w-[92%] sm:max-w-[85%] whitespace-pre-wrap rounded-2xl rounded-tr-sm bg-primary/10 border border-primary/20 dark:bg-primary/20 dark:border-primary/35 px-3.5 py-2.5 text-sm leading-relaxed text-foreground shadow-sm">
          {m.content}
        </div>
        {m.feedback && (
          <details className="group max-w-[92%] sm:max-w-[85%] rounded-xl bg-muted/60 border border-border p-3 text-xs text-foreground shadow-sm">
            <summary className="flex min-h-[32px] cursor-pointer items-center justify-between gap-2 border-b border-border/50 pb-1.5 select-none">
              <div className="flex items-center gap-1.5">
                <span className="font-semibold text-primary">📝 考官即时点评</span>
                <span className="text-[10px] text-muted-foreground group-open:hidden">（点击展开）</span>
              </div>
              <div className="flex items-center gap-1.5">
                {m.score != null && (
                  <span
                    className={`font-mono font-bold px-1.5 py-0.5 rounded text-[10px] ${
                      m.score >= 80
                        ? 'bg-success/15 text-success'
                        : m.score >= 60
                        ? 'bg-warning/15 text-warning'
                        : 'bg-destructive/15 text-destructive'
                    }`}
                  >
                    {m.score} 分
                  </span>
                )}
              </div>
            </summary>
            <div className="text-muted-foreground leading-relaxed mt-2.5">
              <Markdown text={m.feedback} />
            </div>
            <div className="mt-3 pt-2 border-t border-border/40 flex items-center justify-between text-[11px]">
              <span className="text-muted-foreground">已自动同步至能力雷达</span>
              <Link to="/questions" className="font-medium text-primary hover:underline inline-flex items-center gap-0.5">
                <span>前往题库查看沉淀 →</span>
              </Link>
            </div>
          </details>
        )}
      </div>
    )
  }
  if (m.kind === 'summary' || m.kind === 'score' || m.kind === 'control') return null

  // 辅助提示卡片（三级阶梯式）
  if (m.kind === 'hint') {
    return <HintCard content={m.content} onUnlockLevel={onUnlockHintLevel} />
  }

  // 追问 (probe) 或 主考题 (question)
  const isProbe = m.kind === 'probe' || m.intent === 'followup_probe'
  // M4：追问理由封闭枚举 -> 徽章语义色（ADR-0023 D2）
  const PROBE_BADGE: Record<string, { label: string; cls: string }> = {
    depth_probe: { label: '深挖', cls: 'bg-primary/10 text-primary' },
    clarification: { label: '澄清', cls: 'bg-info/10 text-info' },
    edge_case: { label: '边界', cls: 'bg-warning/10 text-warning' },
    contradiction: { label: '矛盾', cls: 'bg-destructive/10 text-destructive' },
    breadth_pivot: { label: '拓展', cls: 'bg-success/10 text-success' },
  }
  const badge = isProbe && m.meta?.reason ? PROBE_BADGE[m.meta.reason] : undefined
  return (
    <div className="flex items-start gap-2">
      <AiAvatar />
      <div className="min-w-0 flex-1">
        <div className="mb-0.5 flex items-center gap-2">
          <span className="text-xs font-semibold text-muted-foreground">面试官</span>
          {isProbe ? (
            <>
              <span className="rounded bg-warning/10 px-1.5 py-0.2 font-mono text-[10px] font-bold text-warning">
                💬 深度追问
              </span>
              {m.meta?.anchor_keyword && badge && (
                <span
                  className={`rounded px-1.5 py-0.2 text-[10px] font-medium ${badge.cls}`}
                  title={`锚点：${m.meta.anchor_keyword}`}
                >
                  ⚓ {badge.label} · {m.meta.anchor_keyword}
                </span>
              )}
            </>
          ) : (
            <span className="rounded bg-secondary border border-border-strong px-1.5 py-0.2 font-mono text-[10px] font-bold text-heading">
              🎯 核心考题
            </span>
          )}
        </div>
        <div className="text-sm leading-7">
          <Markdown text={m.content} />
        </div>
      </div>
    </div>
  )
}
