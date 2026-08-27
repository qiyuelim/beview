import { useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { Check, Plus, Sparkle, X } from '@phosphor-icons/react'
import { apiDelete, apiGet, apiPatch, apiPost, apiPut } from '../api/client'
import { APP_STATUS } from '../api/types'
import Markdown from '../components/Markdown'
import { isRunning, onJobDone, startAiJob, trackRunning, useAiJobs } from '../ai/jobs'
import StageTimeline from '../components/StageTimeline'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { SemBadge, type BadgeSem } from '../components/SemBadge'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'

interface DetailQuestion {
  id: number
  content: string
  my_answer: string | null
  first_answer: { content: string; source: string; created_at: string } | null
  score: number | null
  feedback: string | null
}

const STATUS_SEM: Record<string, BadgeSem> = {
  applied: 'neutral',
  callback: 'warn',
  interviewing: 'info',
  offer: 'pass',
  rejected: 'danger',
  withdrawn: 'neutral',
}

const selectCls =
  'h-9 w-full rounded-md border border-input bg-card px-2 text-sm focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

/** 轮次子页（反馈 #4）：本场全部题目（含第一手真实回答）+ AI 综合复盘 + 心得备注 */
export default function RoundDetail() {
  const { id } = useParams()
  const nav = useNavigate()
  const [round, setRound] = useState<any>(null)
  const [app, setApp] = useState<any>(null)
  const [stages, setStages] = useState<{ name: string; passed: string }[]>([])
  const [questions, setQuestions] = useState<DetailQuestion[]>([])
  const [retro, setRetro] = useState<any>(null)
  const [err, setErr] = useState('')
  // 结果标记（B组 #4：快捷按钮 + 内联确认，确认即落库并锁定）
  const [pendingAction, setPendingAction] = useState<'pass' | 'fail' | null>(null)
  const [busy, setBusy] = useState(false)
  // 反馈 #10：编辑轮次信息（名称/日期/形式；结果锁定只锁 passed）
  const [editOpen, setEditOpen] = useState(false)
  const [editForm, setEditForm] = useState({ name: '', date: '', form: '' })
  // 删除本轮确认
  const [delOpen, setDelOpen] = useState(false)
  // C组 #6：复盘生成中状态由全局中心提供
  const aiJobs = useAiJobs()
  const isRetroRunning = isRunning(aiJobs, 'retrospective', Number(id))

  async function load() {
    const d = await apiGet(`/api/rounds/${id}/detail`)
    setRound(d.round)
    setApp(d.application ?? null)
    setStages(d.stages ?? [])
    setQuestions(d.questions ?? [])
    setRetro(d.retrospective ?? null)
    trackRunning(d.ai_jobs) // 刷新恢复「复盘生成中」跟踪
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  // 复盘任务完成：reload 展示落库草稿
  useEffect(() => {
    const rid = Number(id)
    return onJobDone('retrospective', rid, (ok) => {
      if (!ok) setErr('AI 复盘生成失败，请重试')
      else toast.success('AI 复盘已生成')
      load().catch(() => {})
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  /** 确认落库：通过仅标记本轮；未过可选投递同步标未过（后端硬锁：选定后不可变更） */
  async function confirmMark(choice: 'pass' | 'fail-only' | 'fail-reject') {
    setErr('')
    setBusy(true)
    try {
      const passed = choice === 'pass' ? 'pass' : 'fail'
      await apiPatch(`/api/rounds/${id}`, { passed })
      if (choice === 'fail-reject' && app) {
        await apiPatch(`/api/applications/${app.id}`, { status: 'rejected' })
      }
      setPendingAction(null)
      await load()
      toast.success(
        choice === 'pass'
          ? '已标记通过'
          : choice === 'fail-reject'
            ? '已标记未通过，投递已同步'
            : '本轮已记未通过',
      )
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
    }
  }

  async function doDelete() {
    setErr('')
    setBusy(true)
    try {
      await apiDelete(`/api/rounds/${id}`)
      nav(`/applications/${app.id}`)
    } catch (e: any) {
      setErr(e.message)
      setBusy(false)
    }
  }

  if (!round) {
    return <div className="py-24 text-center text-muted-foreground">{err || '加载中…'}</div>
  }

  return (
    <div>
      <nav aria-label="面包屑" className="mb-2 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link to="/applications" className="hover:text-primary">
          投递
        </Link>
        {app && (
          <>
            <span aria-hidden>/</span>
            <Link to={`/applications/${app.id}`} className="hover:text-primary">
              {app.company ?? '未关联公司'} · {app.position ?? '未填岗位'}
            </Link>
          </>
        )}
        <span aria-hidden>/</span>
        <span className="text-foreground">第 {round.sort_order} 轮</span>
      </nav>

      {/* B组 #3：统一节点时间线（当前轮次进展一屏可见） */}
      {app && <StageTimeline stages={stages} status={app.status} />}

      <PageHeader
        title={`第 ${round.sort_order} 轮 · ${round.name}`}
        meta={
          <>
            {round.passed === 'pass' && (
              <SemBadge sem="pass">
                <Check weight="bold" className="size-3" aria-hidden /> 通过
              </SemBadge>
            )}
            {round.passed === 'fail' && (
              <SemBadge sem="danger">
                <X weight="bold" className="size-3" aria-hidden /> 未通过
              </SemBadge>
            )}
            {round.passed === 'pending' && <SemBadge sem="neutral">待定</SemBadge>}
            {app && (
              <SemBadge sem={STATUS_SEM[app.status] ?? 'neutral'}>
                投递状态：{APP_STATUS[app.status as keyof typeof APP_STATUS] ?? app.status}
              </SemBadge>
            )}
            {round.form && <span>· {round.form}</span>}
            {round.date && <span className="font-mono">· {round.date}</span>}
          </>
        }
        actions={
          <>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                setEditForm({ name: round.name ?? '', date: round.date ?? '', form: round.form ?? '' })
                setEditOpen((v) => !v)
              }}
            >
              编辑信息
            </Button>
            {round.passed === 'pending' && (
              <Button size="sm" variant="ghost" className="text-destructive hover:bg-destructive/10" onClick={() => setDelOpen(true)}>
                删除本轮
              </Button>
            )}
            {round.passed === 'pending' ? (
              <div className="flex items-center gap-2 text-sm">
                <button
                  type="button"
                  disabled={busy}
                  className="px-1.5 py-1 text-foreground hover:text-success hover:underline disabled:opacity-50"
                  onClick={() => setPendingAction('pass')}
                >
                  通过
                </button>
                <span className="text-border" aria-hidden>
                  ·
                </span>
                <button
                  type="button"
                  disabled={busy}
                  className="px-1.5 py-1 text-foreground hover:text-destructive hover:underline disabled:opacity-50"
                  onClick={() => setPendingAction('fail')}
                >
                  未过
                </button>
              </div>
            ) : (
              <span className="text-xs text-muted-foreground">结果已选定 · 不可变更</span>
            )}
          </>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {/* 结果确认条（B组 #4：确认才落库，后端硬锁同值幂等） */}
      {pendingAction === 'pass' && (
        <div className="mb-3 flex flex-wrap items-baseline gap-x-3 gap-y-1 rounded-md border border-border bg-card px-3 py-2.5 text-sm" role="alertdialog" aria-label="确认通过">
          <span className="min-w-0 flex-1">标记本轮为「通过」？选定后不可变更。</span>
          <button type="button" disabled={busy} className="font-medium hover:underline disabled:opacity-50" onClick={() => confirmMark('pass')}>
            确认
          </button>
          <button type="button" className="text-muted-foreground hover:text-foreground" onClick={() => setPendingAction(null)}>
            取消
          </button>
        </div>
      )}
      {pendingAction === 'fail' && (
        <div className="mb-3 flex flex-wrap items-baseline gap-x-3 gap-y-1 rounded-md border border-border bg-card px-3 py-2.5 text-sm" role="alertdialog" aria-label="确认未通过">
          <span className="min-w-0 flex-1">
            标记本轮为「未通过」？选定后不可变更。
            {app?.status === 'interviewing' ? '建议将投递同步标为未通过。' : ''}
          </span>
          {app?.status === 'interviewing' && (
            <button type="button" disabled={busy} className="font-medium text-destructive hover:underline disabled:opacity-50" onClick={() => confirmMark('fail-reject')}>
              整场淘汰并同步投递
            </button>
          )}
          <button type="button" disabled={busy} className="font-medium hover:underline disabled:opacity-50" onClick={() => confirmMark('fail-only')}>
            仅本轮记未过
          </button>
          <button type="button" className="text-muted-foreground hover:text-foreground" onClick={() => setPendingAction(null)}>
            取消
          </button>
        </div>
      )}

      {/* 轮次信息编辑（反馈 #10） */}
      {editOpen && (
        <Section title="编辑轮次信息" className="mb-4">
          <form
            className="grid grid-cols-1 gap-3 sm:grid-cols-3"
            onSubmit={async (e) => {
              e.preventDefault()
              setErr('')
              setBusy(true)
              try {
                await apiPatch(`/api/rounds/${id}`, {
                  name: editForm.name.trim() || null,
                  date: editForm.date || null,
                  form: editForm.form || null,
                })
                setEditOpen(false)
                await load()
                toast.success('轮次信息已更新')
              } catch (e: any) {
                setErr(e.message)
              } finally {
                setBusy(false)
              }
            }}
          >
            <FormField label="轮次名称" htmlFor="er-name">
              <Input id="er-name" value={editForm.name} onChange={(e) => setEditForm((f) => ({ ...f, name: e.target.value }))} />
            </FormField>
            <FormField label="日期" htmlFor="er-date">
              <Input id="er-date" type="date" value={editForm.date} onChange={(e) => setEditForm((f) => ({ ...f, date: e.target.value }))} />
            </FormField>
            <FormField label="形式" htmlFor="er-form">
              <select
                id="er-form"
                value={editForm.form}
                onChange={(e) => setEditForm((f) => ({ ...f, form: e.target.value }))}
                className={selectCls}
              >
                <option value="">未定</option>
                <option value="现场">现场</option>
                <option value="视频">视频</option>
                <option value="电话">电话</option>
              </select>
            </FormField>
            <div className="flex items-center gap-2 sm:col-span-3">
              <Button type="submit" disabled={busy}>
                保存
              </Button>
              <Button type="button" variant="ghost" onClick={() => setEditOpen(false)}>
                取消
              </Button>
            </div>
          </form>
        </Section>
      )}

      {/* 本场题目：第一手真实回答 + 判分 */}
      <Section
        title={`本场题目（${questions.length}）`}
        className="mb-4"
        action={
          <div className="flex items-center gap-2">
            <Button size="sm" asChild>
              <Link to={`/new?round_id=${id}`}>
                <Plus weight="bold" className="size-4 mr-1" aria-hidden /> 录入真题
              </Link>
            </Button>
            <Button size="sm" variant="ghost" asChild>
              <Link to={`/questions?round=${id}`}>去题库看全部</Link>
            </Button>
          </div>
        }
      >
        {questions.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            本轮还没有录题——点右上角「录入真题」记题。
          </p>
        ) : (
          <div className="space-y-3">
            {questions.map((q, i) => (
              <div key={q.id} className="rounded-md border border-border p-2.5">
                <div className="flex items-center justify-between gap-2">
                  <span className="min-w-0 text-sm font-medium">
                    <span className="mr-1 font-mono text-muted-foreground">{i + 1}</span>
                    <Link to={`/questions/${q.id}`} className="hover:text-primary">
                      {q.content}
                    </Link>
                  </span>
                  {q.score != null && (
                    <span
                      className={`shrink-0 font-mono text-sm font-bold tabular-nums ${q.score < 60 ? 'text-destructive' : q.score < 80 ? 'text-warning' : 'text-success'}`}
                      title="综合分"
                    >
                      {q.score}
                    </span>
                  )}
                </div>
                {q.first_answer ? (
                  <div className="mt-1.5">
                    <div className="mb-0.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      第一手真实回答
                      {q.first_answer.source !== 'interview'
                        ? `（${q.first_answer.source === 'manual' ? '手动补答' : q.first_answer.source}）`
                        : ''}
                    </div>
                    <p className="text-sm leading-6 text-muted-foreground">{q.first_answer.content}</p>
                  </div>
                ) : (
                  <p className="mt-1 text-xs text-muted-foreground">未留作答记录。</p>
                )}
                {q.feedback && (
                  <details className="mt-1.5">
                    <summary className="cursor-pointer text-xs text-muted-foreground">AI 点评</summary>
                    <div className="mt-1 text-sm leading-7">
                      <Markdown text={q.feedback} />
                    </div>
                  </details>
                )}
              </div>
            ))}
          </div>
        )}
      </Section>

      {/* 综合复盘（AI 基于第一手真实回答；人类心得不被覆盖） */}
      <RetroPanel
        rid={Number(id)}
        retro={retro}
        busy={isRetroRunning}
        onSaved={(v) => {
          setRetro(v)
          toast.success('已保存')
        }}
        onToast={toast.success}
        className="mt-4"
      />

      {/* 删除本轮确认 */}
      <ConfirmDialog
        open={delOpen}
        onOpenChange={setDelOpen}
        destructive
        busy={busy}
        title={`删除误创建的「${round.name || `第 ${round.sort_order} 轮`}」？`}
        description="该轮次及其关联记录将被删除，不可恢复。"
        confirmLabel="删除"
        onConfirm={doDelete}
      />
    </div>
  )
}

/** 综合复盘面板：AI 草稿（weaknesses/advice 展示）+ 手动编辑 + 心得备注 */
function RetroPanel({
  rid,
  retro,
  busy,
  onSaved,
  onToast,
  className,
}: {
  rid: number
  retro: any
  /** AI 复盘任务进行中（全局任务中心派生，父组件传入） */
  busy?: boolean
  onSaved: (v: any) => void
  onToast: (msg: string) => void
  className?: string
}) {
  const [draft, setDraft] = useState<any>(retro ?? null)
  const [saving, setSaving] = useState(false)
  const [err, setErr] = useState('')
  const [checked, setChecked] = useState<Record<number, boolean>>({})
  useEffect(() => setDraft(retro ?? null), [retro])

  async function aiDraft() {
    // C组 #6：受理后由全局中心跟踪，完成经父组件 reload 回显；期间 busy 派生态锁定按钮
    try {
      await startAiJob('retrospective', rid, `/api/rounds/${rid}/retrospective/ai`)
      onToast('AI 复盘生成中…完成后自动回显')
    } catch (e: any) {
      setErr(e.message)
    }
  }
  async function save() {
    setSaving(true)
    setErr('')
    try {
      const v = await apiPut(`/api/rounds/${rid}/retrospective`, {
        overall: draft?.overall ?? '',
        problems: draft?.problems ?? [],
        improvements: draft?.improvements ?? [],
        notes: draft?.notes ?? '',
      })
      onSaved(v)
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setSaving(false)
    }
  }
  async function toReview() {
    const items = Object.entries(checked).filter(([, v]) => v).map(([i]) => draft.improvements[Number(i)])
    if (items.length === 0) return
    setSaving(true)
    try {
      const r = await apiPost(`/api/rounds/${rid}/retrospective/to-review`, { items })
      onToast(`已把 ${r.created} 条改进项记入题库并加入复习队列`)
      setChecked({})
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setSaving(false)
    }
  }

  return (
    <Section title="综合复盘" className={className} action={
      <Button size="sm" variant="secondary" onClick={aiDraft} disabled={busy}>
        <Sparkle weight="fill" className="size-4" aria-hidden />
        {busy ? 'AI 复盘生成中…' : 'AI 综合评价'}
      </Button>
    }>
      {!draft ? (
        <p className="text-sm text-muted-foreground">
          点「AI 综合评价」：结合本场每道题的第一手真实回答与判分，给出表现评级、能力证据、薄弱点与改进项。
        </p>
      ) : (
        <div className="space-y-3">
          <RetroStructured draft={draft} />
          {err && (
            <p role="alert" className="rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
              {err}
            </p>
          )}
          <FormField label="整体表现" htmlFor="rp-overall">
            <Textarea rows={2} id="rp-overall" value={draft.overall ?? ''} onChange={(e) => setDraft((d: any) => ({ ...d, overall: e.target.value }))} />
          </FormField>
          <FormField label="问题清单（每行一条）" htmlFor="rp-problems">
            <Textarea rows={2} id="rp-problems" value={(draft.problems ?? []).join('\n')} onChange={(e) => setDraft((d: any) => ({ ...d, problems: e.target.value.split('\n') }))} />
          </FormField>
          <FormField label="改进项（每行一条，可勾选转入复习队列）" htmlFor="rp-impr">
            <Textarea rows={2} id="rp-impr" value={(draft.improvements ?? []).join('\n')} onChange={(e) => setDraft((d: any) => ({ ...d, improvements: e.target.value.split('\n') }))} />
          </FormField>
          {(draft.improvements ?? []).filter(Boolean).length > 0 && (
            <div className="space-y-1 rounded-md bg-muted/50 p-2">
              {(draft.improvements ?? []).map((it: string, i: number) =>
                it.trim() ? (
                  <label key={i} className="flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      className="size-4 accent-[var(--primary)]"
                      checked={!!checked[i]}
                      onChange={(e) => setChecked((c) => ({ ...c, [i]: e.target.checked }))}
                    />
                    {it}
                  </label>
                ) : null,
              )}
              <Button
                size="sm"
                className="mt-1"
                onClick={toReview}
                disabled={busy || saving || Object.values(checked).every((v) => !v)}
              >
                勾选项转入复习队列
              </Button>
            </div>
          )}
          <FormField label="我的心得备注（手工保留）" htmlFor="rp-notes">
            <AutoTextarea
              rows={3}
              value={draft.notes ?? ''}
              onChange={(e) => setDraft((d: any) => ({ ...d, notes: e.target.value }))}
              placeholder="记录面试现场感受、面试官风格、自己的临场问题…"
            />
          </FormField>
          <div className="flex items-center gap-2">
            <Button size="sm" onClick={save} disabled={busy || saving || !draft.overall?.trim()}>
              保存复盘
            </Button>
          </div>
        </div>
      )}
    </Section>
  )
}

/** 自动增高 textarea（与简历编辑器同款交互） */
function AutoTextarea(props: {
  rows?: number
  value: string
  onChange: (e: React.ChangeEvent<HTMLTextAreaElement>) => void
  placeholder?: string
}) {
  const ref = useRef<HTMLTextAreaElement | null>(null)
  useEffect(() => {
    const el = ref.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight}px`
  }, [props.value])
  return (
    <Textarea
      ref={ref as any}
      rows={props.rows ?? 3}
      value={props.value}
      onChange={props.onChange}
      placeholder={props.placeholder}
      className="min-h-0 resize-none overflow-hidden"
    />
  )
}

/** ---------- 单场复盘结构化渲染（proposal §2.4，含旧数据兼容，ADR-0015 卡片化） ---------- */

const GRADE_SEM: Record<string, BadgeSem> = {
  优秀: 'pass',
  良好: 'pass',
  一般: 'warn',
  偏弱: 'danger',
  高: 'pass',
  中: 'warn',
  低: 'danger',
}

function GradeBadges({ draft }: { draft: any }) {
  const items = [
    { label: '整体表现', value: draft.performance },
    { label: '岗位匹配', value: draft.match },
    { label: '评估置信度', value: draft.confidence },
  ].filter((x) => typeof x.value === 'string' && x.value)
  if (items.length === 0) return null
  return (
    <div className="flex flex-wrap items-center gap-2">
      {items.map((x) => (
        <div key={x.label} className="inline-flex items-center gap-1.5 rounded-lg border border-border bg-card px-2.5 py-1 text-xs shadow-xs">
          <span className="text-muted-foreground">{x.label}:</span>
          <SemBadge sem={GRADE_SEM[x.value] ?? 'neutral'}>{x.value}</SemBadge>
        </div>
      ))}
    </div>
  )
}

function StrengthCards({ strengths }: { strengths: any[] }) {
  const list = (strengths ?? []).filter((x) => x && typeof x === 'object')
  if (list.length === 0) return null
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-1.5 text-xs font-semibold text-success">
        <span className="size-2 rounded-full bg-success" aria-hidden />
        最强表现 / 加分亮点
      </div>
      <div className="grid grid-cols-1 gap-2.5">
        {list.map((x, i) => (
          <div key={i} className="rounded-lg border border-success/30 bg-success/5 p-3.5 shadow-xs">
            <div className="font-semibold text-sm text-foreground">{x.point}</div>
            {x.evidence && <p className="mt-1 text-xs text-muted-foreground leading-relaxed"><b>证据：</b>{x.evidence}</p>}
            {x.why_plus && <p className="mt-1 text-xs text-success leading-relaxed"><b>为什么加分：</b>{x.why_plus}</p>}
          </div>
        ))}
      </div>
    </div>
  )
}

/** weaknesses 新 schema 为对象数组；旧数据是字符串数组——渲染兼容 */
function WeaknessList({ weaknesses }: { weaknesses: any[] }) {
  const list = (weaknesses ?? []).filter(Boolean)
  if (list.length === 0) return null
  const isLegacy = list.every((x) => typeof x === 'string')
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-1.5 text-xs font-semibold text-destructive">
        <span className="size-2 rounded-full bg-destructive" aria-hidden />
        薄弱点 / 扣分归因
      </div>
      {isLegacy ? (
        <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-3.5 shadow-xs">
          <ul className="list-disc space-y-1.5 pl-4 text-xs leading-relaxed text-foreground">
            {list.map((w: string, i: number) => (
              <li key={i}>{typeof w === 'string' ? w : ''}</li>
            ))}
          </ul>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-2.5">
          {list.map((x: any, i: number) => (
            <div key={i} className="rounded-lg border border-destructive/30 bg-destructive/5 p-3.5 shadow-xs">
              <div className="font-semibold text-sm text-foreground">{x.question}</div>
              {x.problem && <p className="mt-1 text-xs text-destructive leading-relaxed"><b>问题：</b>{x.problem}</p>}
              {x.impact && <p className="mt-1 text-xs text-muted-foreground leading-relaxed"><b>影响：</b>{x.impact}</p>}
              {x.better && <p className="mt-1 text-xs text-primary font-medium leading-relaxed"><b>更好方向：</b>{x.better}</p>}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

function AbilityTable({ abilities }: { abilities: any[] }) {
  const list = (abilities ?? []).filter((x) => x && typeof x === 'object')
  if (list.length === 0) return null
  const semOf = (s: string): BadgeSem =>
    s === '高' ? 'pass' : s === '中' ? 'warn' : s === '低' ? 'danger' : 'neutral'
  return (
    <div className="space-y-2">
      <div className="text-xs font-semibold text-muted-foreground">能力证据表</div>
      <div className="overflow-hidden rounded-lg border border-border bg-card shadow-xs">
        <table className="w-full border-collapse text-xs">
          <thead>
            <tr className="border-b border-border bg-muted/40">
              <th className="py-2 px-3 text-left font-semibold text-muted-foreground">能力维度</th>
              <th className="py-2 px-3 text-left font-semibold text-muted-foreground">考察确认</th>
              <th className="py-2 px-3 text-left font-semibold text-muted-foreground">证据强度</th>
              <th className="py-2 px-3 text-left font-semibold text-muted-foreground">潜在风险</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {list.map((a, i) => (
              <tr key={i} className="transition-colors hover:bg-muted/20">
                <td className="py-2.5 px-3 font-medium text-foreground">{a.ability}</td>
                <td className="py-2.5 px-3">
                  {a.tested ? (
                    <span className="inline-flex items-center gap-1 text-success font-medium">
                      <Check className="size-3.5" weight="bold" aria-hidden /> 已考察
                    </span>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </td>
                <td className="py-2.5 px-3">
                  <SemBadge sem={semOf(a.evidence_strength)}>{a.evidence_strength || '无证据'}</SemBadge>
                </td>
                <td className="py-2.5 px-3 text-muted-foreground">{a.risk || '—'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

function InterviewerView({ view }: { view: any }) {
  if (!view || typeof view !== 'object') return null
  const hasContent =
    (view.positive?.length || 0) + (view.doubts?.length || 0) + (view.unverified?.length || 0) > 0
  if (!hasContent) return null

  return (
    <div className="space-y-2">
      <div className="text-xs font-semibold text-muted-foreground">面试官视角推断（AI 心理画像推演）</div>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        {(view.positive ?? []).length > 0 && (
          <div className="rounded-lg border border-success/30 bg-success/5 p-3 shadow-xs">
            <div className="font-semibold text-xs text-success mb-1.5 flex items-center gap-1">
              <span className="size-1.5 rounded-full bg-success" /> 可能认可
            </div>
            <ul className="list-disc space-y-1 pl-4 text-xs leading-relaxed text-foreground">
              {view.positive.map((x: string, i: number) => (
                <li key={i}>{x}</li>
              ))}
            </ul>
          </div>
        )}
        {(view.doubts ?? []).length > 0 && (
          <div className="rounded-lg border border-warning/30 bg-warning/5 p-3 shadow-xs">
            <div className="font-semibold text-xs text-warning mb-1.5 flex items-center gap-1">
              <span className="size-1.5 rounded-full bg-warning" /> 可能有疑虑
            </div>
            <ul className="list-disc space-y-1 pl-4 text-xs leading-relaxed text-foreground">
              {view.doubts.map((x: string, i: number) => (
                <li key={i}>{x}</li>
              ))}
            </ul>
          </div>
        )}
        {(view.unverified ?? []).length > 0 && (
          <div className="rounded-lg border border-border bg-card p-3 shadow-xs">
            <div className="font-semibold text-xs text-muted-foreground mb-1.5 flex items-center gap-1">
              <span className="size-1.5 rounded-full bg-muted-foreground" /> 未验证清楚
            </div>
            <ul className="list-disc space-y-1 pl-4 text-xs leading-relaxed text-foreground">
              {view.unverified.map((x: string, i: number) => (
                <li key={i}>{x}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </div>
  )
}

function RetroStructured({ draft }: { draft: any }) {
  return (
    <div className="space-y-4">
      <GradeBadges draft={draft} />

      {draft.overall && (
        <div className="rounded-lg border border-border bg-card p-4 shadow-xs">
          <div className="mb-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground">整场综合结论</div>
          <div className="text-sm leading-relaxed text-foreground">
            <Markdown text={String(draft.overall)} />
          </div>
        </div>
      )}

      <StrengthCards strengths={draft.strengths} />
      <WeaknessList weaknesses={draft.weaknesses} />
      <AbilityTable abilities={draft.abilities} />
      <InterviewerView view={draft.interviewer_view} />

      {(draft.problems ?? []).filter((x: unknown) => typeof x === 'string' && x).length > 0 && (
        <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4 shadow-xs">
          <div className="mb-2 text-xs font-semibold text-destructive">问题清单归纳</div>
          <ul className="list-disc space-y-1.5 pl-4 text-xs leading-relaxed text-foreground">
            {(draft.problems as string[]).map((x, i) => x.trim() && <li key={i}>{x}</li>)}
          </ul>
        </div>
      )}

      {draft.advice && (
        <div className="rounded-lg border border-primary/30 bg-primary/5 p-4 shadow-xs">
          <div className="mb-1 text-xs font-semibold text-primary">综合改进建议</div>
          <div className="text-sm leading-relaxed text-foreground">
            <Markdown text={String(draft.advice)} />
          </div>
        </div>
      )}
    </div>
  )
}
