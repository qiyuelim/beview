import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { CircleNotch, Coin, MapPin, Sparkle } from '@phosphor-icons/react'
import { apiDelete, apiGet, apiPatch, apiPost, apiPut } from '../api/client'
import type { Application, ApplicationStatus } from '../api/types'
import { isRunning, onJobDone, startAiJob, trackRunning, useAiJobs } from '../ai/jobs'
import { APP_STATUS } from '../api/types'
import StagePipeline from '../components/StagePipeline'
import Markdown from '../components/Markdown'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { SemBadge, type BadgeSem } from '../components/SemBadge'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import { toast } from 'sonner'

interface AppEvent {
  id: number
  /** status（投递状态流转）| round（面试轮次跟踪，反馈六） */
  kind?: 'status' | 'round'
  from_status: string | null
  to_status: string
  source: string
  note: string | null
  created_at: string
}
interface RoundBrief {
  id: number
  name: string
  sort_order: number
  date: string | null
  form: string | null
  passed: string
  created: string | null
  question_count: number
}
const SOURCE_LABEL: Record<string, string> = {
  create: '创建投递',
  manual: '确认流转',
  auto: '自动推进',
}

const STATUS_SEM: Record<ApplicationStatus, BadgeSem> = {
  applied: 'neutral',
  interviewing: 'info',
  offer: 'pass',
  rejected: 'danger',
  withdrawn: 'neutral',
}

/** 投递详情（ADR-0012；v4.2 设计语言 v2 迁移）：
 *  状态由面试进展推导（首场自动推进，轮次结果确认流），删除为页底文本按钮。 */
export default function ApplicationDetail() {
  const { id } = useParams()
  const nav = useNavigate()
  const [app, setApp] = useState<Application | null>(null)
  const [events, setEvents] = useState<AppEvent[]>([])
  const [rounds, setRounds] = useState<RoundBrief[]>([])
  const [addRoundOpen, setAddRoundOpen] = useState(false)
  const [newRound, setNewRound] = useState({ name: '', date: '', form: '' })
  // JD 解读 / 匹配度页签；岗位 JD 原文只读查看
  const [jdTab, setJdTab] = useState<'interpret' | 'match'>('interpret')
  const [jdOpen, setJdOpen] = useState(false)
  const [jdText, setJdText] = useState('')
  // 投递信息编辑
  const [infoEditing, setInfoEditing] = useState(false)
  const [infoForm, setInfoForm] = useState({ department: '', channel: '', note: '', salary: '' })
  const [interpret, setInterpret] = useState<any>(null)
  const [match, setMatch] = useState<any>(null)
  // C组 #6：解读/匹配度任务状态由全局中心提供（跨页/刷新不丢回显）
  const aiJobs = useAiJobs()
  const interpretTargetId = app?.position_id ?? 0
  const aiBusyKind = isRunning(aiJobs, 'jd_interpret', interpretTargetId)
    ? 'interpret'
    : isRunning(aiJobs, 'jd_match', Number(id))
      ? 'match'
      : ''
  const [offerConfirm, setOfferConfirm] = useState(false)
  const [pendingRoundMark, setPendingRoundMark] = useState<{
    roundId: number
    roundName: string
    action: 'pass' | 'fail'
  } | null>(null)
  const [err, setErr] = useState('')
  const [roundErr, setRoundErr] = useState('')
  const [busy, setBusy] = useState(false)
  // 反馈七#3：删除投递需手动输入「确认删除」
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [withdrawOpen, setWithdrawOpen] = useState(false)

  const lastRound = rounds.length > 0 ? rounds[rounds.length - 1] : null
  const addRoundDisabledReason =
    lastRound?.passed === 'pending'
      ? `「${lastRound.name}」还未标记结果，需先标记通过才能添加下一面`
      : lastRound?.passed === 'fail'
      ? `「${lastRound.name}」未通过，无法添加下一面；如需继续请先在轮次中复核结果`
      : ''

  async function load() {
    const d = await apiGet(`/api/applications/${id}`)
    setApp(d.application)
    setEvents(d.events ?? [])
    setRounds(d.rounds ?? [])
    setInterpret(d.application?.jd_interpret ?? null)
    setMatch(d.application?.jd_match ?? null)
    trackRunning(d.ai_jobs) // 刷新/重进时恢复「进行中」跟踪
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  // 解读/匹配度任务完成：reload 展示落库结果
  useEffect(() => {
    const aid = Number(id)
    const pid = app?.position_id
    const offI = onJobDone('jd_interpret', pid ?? aid, (ok) => {
      if (!ok) setErr('JD 解读失败，请重试')
      else toast.success('JD 解读完成（本岗共享）')
      load().catch(() => {})
    })
    const offM = onJobDone('jd_match', aid, (ok) => {
      if (!ok) setErr('匹配度评估失败，请重试')
      else toast.success('匹配度评估完成')
      load().catch(() => {})
    })
    return () => {
      offI()
      offM()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, app?.position_id])

  function openInfoEdit() {
    setInfoForm({
      department: app?.department ?? '',
      channel: app?.channel ?? '',
      note: app?.note ?? '',
      salary: app?.salary ?? '',
    })
    setInfoEditing(true)
  }

  async function saveInfo() {
    setErr('')
    setBusy(true)
    try {
      await apiPatch(`/api/applications/${id}`, {
        department: infoForm.department.trim() || null,
        channel: infoForm.channel.trim() || null,
        note: infoForm.note.trim() || null,
        salary: infoForm.salary.trim() || null,
      })
      setInfoEditing(false)
      toast.success('已保存')
      await load()
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
    }
  }

  async function applyStatus(status: ApplicationStatus, msg: string) {
    setErr('')
    try {
      await apiPatch(`/api/applications/${id}`, { status })
      setOfferConfirm(false)
      await load()
      toast.success(msg)
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function confirmRoundMark(choice: 'pass' | 'fail-only' | 'fail-reject') {
    if (!pendingRoundMark) return
    setErr('')
    setBusy(true)
    try {
      const passed = choice === 'pass' ? 'pass' : 'fail'
      await apiPatch(`/api/rounds/${pendingRoundMark.roundId}`, { passed })
      if (choice === 'fail-reject' && app) {
        await apiPatch(`/api/applications/${app.id}`, { status: 'rejected' })
      }
      setPendingRoundMark(null)
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

  function openJdModal() {
    if (!app) return
    setErr('')
    apiGet(`/api/positions/${app.position_id}`)
      .then((p) => {
        setJdText(p.jd_text ?? '')
        setJdOpen(true)
      })
      .catch((e: any) => setErr(e.message))
  }

  async function runAi(kind: 'interpret' | 'match') {
    setErr('')
    try {
      if (kind === 'interpret') {
        if (!app?.position_id) return
        await startAiJob('jd_interpret', app.position_id, `/api/positions/${app.position_id}/interpret`)
      } else {
        await startAiJob('jd_match', Number(id), `/api/applications/${id}/match`)
      }
      // 完成后由 onJobDone 回调 reload
    } catch (e: any) {
      setErr(e.message)
    }
  }

  /** 差距项记入题库：选轮次 -> POST /api/questions；按匹配度差距一键生成主攻薄弱点的陪练 */
  async function makeGapPaper() {
    const gaps = (match?.gaps ?? []).filter(Boolean)
    if (gaps.length === 0) return
    setErr('')
    setBusy(true)
    try {
      const d = await apiPost('/api/drills', {
        kind: 'interview',
        title: `主攻薄弱点 · ${app?.position ?? '岗位'}`,
        position: app?.position ?? undefined,
        application_id: Number(id),
        references:
          '主攻以下薄弱点（来自简历-JD 匹配度评估），陪练题目优先围绕它们出：\n' +
          gaps.map((g: string) => `- ${g}`).join('\n'),
      })
      nav(`/drills/${d.id}`)
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
    }
  }

  if (!app) {
    return (
      <div className="py-24 text-center text-muted-foreground">
        {err || '加载中…'}
      </div>
    )
  }
  const terminal = app.status === 'offer' || app.status === 'rejected' || app.status === 'withdrawn'
  // 当前轮 = 最新创建且未出结果的轮次（无需用户指定，不预设最后一面）
  const currentRound = [...rounds].reverse().find((r) => r.passed === 'pending') ?? null
  const currentIndex = currentRound ? rounds.findIndex((r) => r.id === currentRound.id) : -1

  async function doDelete() {
    setBusy(true)
    try {
      await apiDelete(`/api/applications/${id}`)
      nav('/applications')
    } catch (e: any) {
      setErr(e.message)
      setBusy(false)
    }
  }

  return (
    <div>
      {/* 面包屑：投递 → 公司 → 岗位（反馈 #9） */}
      <nav aria-label="面包屑" className="mb-2 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link to="/applications" className="hover:text-primary">
          投递
        </Link>
        <span aria-hidden>/</span>
        <span className="text-foreground">{app.company ?? '未关联公司'}</span>
        {app.position_id && (
          <>
            <span aria-hidden>/</span>
            <Link to={`/positions/${app.position_id}`} className="hover:text-primary">
              {app.position}
            </Link>
          </>
        )}
      </nav>

      <PageHeader
        title={app.position || '未填岗位'}
        meta={
          <>
            <SemBadge sem={STATUS_SEM[app.status]}>{APP_STATUS[app.status]}</SemBadge>
            {app.company && (
              <Link to={`/companies/${app.company_id}`} className="hover:underline">
                {app.company}
              </Link>
            )}
            {app.location && (
              <span className="inline-flex items-center gap-1">
                <MapPin className="size-3.5" aria-hidden /> {app.location}
              </span>
            )}
            {app.channel && <span>渠道 {app.channel}</span>}
            <span>投递于 {app.applied_at.slice(0, 10)}</span>
          </>
        }
        actions={
          <Button variant="outline" asChild>
            <Link to={`/positions/${app.position_id}`}>查看岗位</Link>
          </Button>
        }
      />

      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {app.salary && (
        <div className="mb-3 flex items-center gap-1.5 rounded-md border border-success/30 bg-success/10 px-3 py-2 text-sm">
          <Coin className="size-4 text-success" weight="fill" aria-hidden />
          Offer 薪资：<b>{app.salary}</b>
        </div>
      )}

      {/* 进度 Pipeline（反馈七#1）：原生裸放页面顶部 */}
      {rounds.length > 0 && (
        <div className="mb-4" aria-label="面试进度条">
          <StagePipeline stages={rounds} currentIndex={currentIndex} />
        </div>
      )}

      <div className="grid gap-4 lg:grid-cols-3">
        <main className="space-y-4 lg:col-span-2">
          {/* 投递信息 */}
          <Section
            title="投递信息"
            action={
              !infoEditing ? (
                <Button size="sm" variant="outline" onClick={openInfoEdit}>
                  编辑
                </Button>
              ) : (
                <Button size="sm" variant="ghost" onClick={() => setInfoEditing(false)}>
                  取消
                </Button>
              )
            }
          >
            {!infoEditing ? (
              <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
                <dt className="text-muted-foreground">部门</dt>
                <dd>{app.department || '—'}</dd>
                <dt className="text-muted-foreground">渠道</dt>
                <dd>{app.channel || '—'}</dd>
                <dt className="text-muted-foreground">薪资</dt>
                <dd>{app.salary || '—'}</dd>
                <dt className="text-muted-foreground">备注/待跟进</dt>
                <dd>{app.note || '—'}</dd>
              </dl>
            ) : (
              <form
                className="grid grid-cols-1 gap-3 sm:grid-cols-2"
                onSubmit={(e) => {
                  e.preventDefault()
                  saveInfo()
                }}
              >
                <FormField label="所属部门" htmlFor="inf-department">
                  <Input
                    id="inf-department"
                    placeholder="例如：基础架构部"
                    value={infoForm.department}
                    onChange={(e) => setInfoForm((f) => ({ ...f, department: e.target.value }))}
                  />
                </FormField>
                <FormField label="渠道" htmlFor="inf-channel">
                  <Input
                    id="inf-channel"
                    value={infoForm.channel}
                    onChange={(e) => setInfoForm((f) => ({ ...f, channel: e.target.value }))}
                  />
                </FormField>
                <FormField label="薪资" htmlFor="inf-salary" hint="offer 后填写，如 25k·16薪">
                  <Input
                    id="inf-salary"
                    value={infoForm.salary}
                    onChange={(e) => setInfoForm((f) => ({ ...f, salary: e.target.value }))}
                  />
                </FormField>
                <FormField label="备注 / 待跟进" htmlFor="inf-note">
                  <Input
                    id="inf-note"
                    value={infoForm.note}
                    onChange={(e) => setInfoForm((f) => ({ ...f, note: e.target.value }))}
                  />
                </FormField>
                <div className="sm:col-span-2">
                  <Button type="submit" disabled={busy}>
                    保存
                  </Button>
                </div>
              </form>
            )}
          </Section>

          {/* 面试进度概览：Pipeline 锚点 + 纵向轮次列表；标记结果收口在轮次详情页 */}
          <Section
            title="面试进度"
            action={
              <>
                {app.status === 'interviewing' && (
                  <Button size="sm" variant="outline" onClick={() => setOfferConfirm((v) => !v)}>
                    整场通过 · 标记 Offer
                  </Button>
                )}
                {!terminal && (
                  <span title={addRoundDisabledReason || undefined} className="inline-block">
                    <Button
                      size="sm"
                      disabled={!!addRoundDisabledReason}
                      onClick={() => {
                        if (addRoundDisabledReason) return
                        setRoundErr('')
                        setAddRoundOpen((v) => !v)
                      }}
                    >
                      {addRoundOpen ? '收起' : '+ 添加面试'}
                    </Button>
                  </span>
                )}
              </>
            }
          >
            {offerConfirm && (
              <div
                className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-card p-3 text-sm shadow-sm"
                role="alertdialog"
                aria-label="确认 Offer"
              >
                <span className="min-w-0 flex-1">
                  确认整场通过并标记 Offer？待定轮次将自动补标为通过，此操作不可回退。
                </span>
                <Button size="sm" disabled={busy} onClick={() => applyStatus('offer', '已标记 Offer')}>
                  确认
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setOfferConfirm(false)}>
                  取消
                </Button>
              </div>
            )}

            {!terminal && rounds.length === 0 && !addRoundOpen && (
              <span title={addRoundDisabledReason || undefined} className="inline-block">
                <Button
                  disabled={!!addRoundDisabledReason}
                  onClick={() => {
                    if (addRoundDisabledReason) return
                    setRoundErr('')
                    setAddRoundOpen(true)
                  }}
                >
                  + 添加第一场面试
                </Button>
              </span>
            )}

            {addRoundOpen && (
              <form
                className="mb-3 grid grid-cols-1 gap-3 sm:grid-cols-3 rounded-lg border border-border bg-card/60 p-3.5"
                onSubmit={async (e) => {
                  e.preventDefault()
                  setRoundErr('')
                  try {
                    await apiPost(`/api/applications/${id}/rounds`, {
                      name: newRound.name.trim() || null,
                      date: newRound.date || null,
                      form: newRound.form.trim() || null,
                    })
                    setNewRound({ name: '', date: '', form: '' })
                    setAddRoundOpen(false)
                    await load()
                  } catch (e: any) {
                    setRoundErr(e.message)
                  }
                }}
              >
                {roundErr && (
                  <div className="sm:col-span-3 rounded-md bg-destructive/10 border border-destructive/20 px-3 py-2 text-xs text-destructive">
                    {roundErr}
                  </div>
                )}
                <FormField label="轮次名称" htmlFor="nr-name">
                  <Input
                    id="nr-name"
                    value={newRound.name}
                    onChange={(e) => setNewRound((f) => ({ ...f, name: e.target.value }))}
                    placeholder={`如：${['一面', '二面', '三面'][rounds.length] ?? `第${rounds.length + 1}轮`}`}
                  />
                </FormField>
                <FormField label="日期" htmlFor="nr-date">
                  <Input
                    id="nr-date"
                    type="date"
                    value={newRound.date}
                    onChange={(e) => setNewRound((f) => ({ ...f, date: e.target.value }))}
                  />
                </FormField>
                <FormField label="形式" htmlFor="nr-form">
                  <select
                    id="nr-form"
                    value={newRound.form}
                    onChange={(e) => setNewRound((f) => ({ ...f, form: e.target.value }))}
                    className="h-9 w-full rounded-md border border-input bg-card px-2 text-sm"
                  >
                    <option value="">未定</option>
                    <option value="现场">现场</option>
                    <option value="视频">视频</option>
                    <option value="电话">电话</option>
                  </select>
                </FormField>
                <div className="sm:col-span-3 flex items-center justify-between gap-2">
                  <Button type="submit">添加面试</Button>
                  <Button type="button" variant="ghost" onClick={() => setAddRoundOpen(false)}>取消</Button>
                </div>
              </form>
            )}

            {/* 纵向轮次列表：当前卡浮起，历史卡轻量；状态与操作严格分离 */}
            <div className="space-y-2">
              {rounds.map((r) => {
                const isCurrent = r.id === currentRound?.id
                const statusSem: BadgeSem = r.passed === 'pass' ? 'pass' : r.passed === 'fail' ? 'danger' : isCurrent ? 'info' : 'neutral'
                const statusLabel = r.passed === 'pass' ? '已通过' : r.passed === 'fail' ? '未通过' : isCurrent ? '进行中' : '待定'
                return (
                  <div
                    key={r.id}
                    onClick={() => nav(`/rounds/${r.id}`)}
                    title={isCurrent ? '进入当前轮' : '查看本轮详情'}
                    className={`cursor-pointer rounded-lg border p-3 transition-all ${
                      isCurrent
                        ? 'border-border-strong bg-card shadow-sm'
                        : 'border-border bg-card hover:border-border-strong'
                    }`}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="min-w-0">
                        <div className="text-sm font-semibold">{r.name || `第 ${r.sort_order} 轮`}</div>
                        <div className="truncate text-xs text-muted-foreground">
                          第 {r.sort_order} 轮
                          {[r.form, r.date].filter(Boolean).length > 0 && ` · ${[r.form, r.date].filter(Boolean).join(' · ')}`}
                        </div>
                      </div>
                      <div className="flex items-center gap-3 shrink-0">
                        {r.passed === 'pending' && (
                          <div className="flex items-center gap-2 text-sm" onClick={(e) => e.stopPropagation()}>
                            <button
                              type="button"
                              disabled={busy}
                              className="text-foreground hover:text-success focus-visible:outline-none focus-visible:underline disabled:opacity-50"
                              onClick={(e) => {
                                e.stopPropagation()
                                setPendingRoundMark({
                                  roundId: r.id,
                                  roundName: r.name || `第 ${r.sort_order} 轮`,
                                  action: 'pass',
                                })
                              }}
                            >
                              通过
                            </button>
                            <span className="text-border" aria-hidden>
                              ·
                            </span>
                            <button
                              type="button"
                              disabled={busy}
                              className="text-foreground hover:text-destructive focus-visible:outline-none focus-visible:underline disabled:opacity-50"
                              onClick={(e) => {
                                e.stopPropagation()
                                setPendingRoundMark({
                                  roundId: r.id,
                                  roundName: r.name || `第 ${r.sort_order} 轮`,
                                  action: 'fail',
                                })
                              }}
                            >
                              未过
                            </button>
                          </div>
                        )}
                        <SemBadge sem={statusSem}>{statusLabel}</SemBadge>
                      </div>
                    </div>
                    {pendingRoundMark?.roundId === r.id && (
                      <div
                        className="mt-2.5 border-t border-border pt-2.5"
                        role="group"
                        aria-label="确认轮次结果"
                        onClick={(e) => e.stopPropagation()}
                      >
                        {pendingRoundMark.action === 'pass' ? (
                          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-sm">
                            <span className="text-foreground">确认「{pendingRoundMark.roundName}」通过？选定后不可变更。</span>
                            <button
                              type="button"
                              disabled={busy}
                              className="font-medium text-foreground hover:underline disabled:opacity-50"
                              onClick={() => confirmRoundMark('pass')}
                            >
                              确认
                            </button>
                            <button type="button" className="text-muted-foreground hover:text-foreground" onClick={() => setPendingRoundMark(null)}>
                              取消
                            </button>
                          </div>
                        ) : (
                          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-sm">
                            <span className="text-foreground">确认「{pendingRoundMark.roundName}」未通过？</span>
                            {app.status === 'interviewing' && (
                              <button
                                type="button"
                                disabled={busy}
                                className="font-medium text-destructive hover:underline disabled:opacity-50"
                                onClick={() => confirmRoundMark('fail-reject')}
                              >
                                整场淘汰并同步投递
                              </button>
                            )}
                            <button
                              type="button"
                              disabled={busy}
                              className="font-medium text-foreground hover:underline disabled:opacity-50"
                              onClick={() => confirmRoundMark('fail-only')}
                            >
                              仅本轮记未过
                            </button>
                            <button type="button" className="text-muted-foreground hover:text-foreground" onClick={() => setPendingRoundMark(null)}>
                              取消
                            </button>
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                )
              })}
            </div>
          </Section>

          {/* 状态流水 */}
          <Section
            title="状态流水"
            sub={<span className="font-mono tabular-nums">{events.length} 条</span>}
          >
            <ul className="divide-y divide-border">
              {events.map((e, idx) => {
                const nestedAuto =
                  e.kind === 'round' &&
                  events[idx + 1]?.kind !== 'round' &&
                  events[idx + 1]?.source === 'auto'
                const skipAsChild = events[idx - 1]?.kind === 'round' && e.source === 'auto' && e.kind !== 'round'
                if (skipAsChild) return null
                const renderStatus = (ev: AppEvent) => (
                  <span className="min-w-0 flex-1 text-sm">
                    {ev.from_status ? `${APP_STATUS[ev.from_status as ApplicationStatus] ?? ev.from_status} → ` : ''}
                    <b>{APP_STATUS[ev.to_status as ApplicationStatus] ?? ev.to_status}</b>
                    <span className="text-muted-foreground"> · {SOURCE_LABEL[ev.source] ?? ev.source}</span>
                  </span>
                )
                return e.kind === 'round' ? (
                  <li key={e.id} className="py-1.5">
                    <div className="flex items-baseline gap-2">
                      <span className="size-1.5 shrink-0 self-center rounded-full bg-info" aria-hidden />
                      <span className="min-w-0 flex-1 text-sm">{e.note ?? '轮次更新'}</span>
                      <span className="shrink-0 font-mono text-xs tabular-nums text-muted-foreground">
                        {e.created_at.slice(0, 16).replace('T', ' ')}
                      </span>
                    </div>
                    {nestedAuto && events[idx + 1] && (
                      <div className="ml-4 mt-1 flex items-baseline gap-2 border-l border-border pl-3 text-sm">
                        <span className="text-foreground">
                          自动推进进入「{APP_STATUS[events[idx + 1].to_status as ApplicationStatus] ?? events[idx + 1].to_status}」
                          <span className="text-muted-foreground"> ← 触发自{e.note ?? '添加面试'}</span>
                        </span>
                      </div>
                    )}
                  </li>
                ) : (
                  <li key={e.id} className="flex items-baseline gap-2 py-1.5">
                    <span className="size-1.5 shrink-0 self-center rounded-full bg-muted-foreground" aria-hidden />
                    {renderStatus(e)}
                    <span className="shrink-0 font-mono text-xs tabular-nums text-muted-foreground">
                      {e.created_at.slice(0, 16).replace('T', ' ')}
                    </span>
                  </li>
                )
              })}
            </ul>
          </Section>

          {/* 危险区（反馈 #9：删除用文本按钮） */}
          <Section
            title="危险操作"
            className="border-border-strong"
          >
            <div className="flex flex-wrap items-center gap-2">
              {!terminal && app.status !== 'applied' && (
                <Button
                  size="sm"
                  variant="outline"
                  className="border-destructive/40 text-destructive hover:bg-destructive/10"
                  disabled={busy}
                  onClick={() => setWithdrawOpen(true)}
                >
                  放弃投递
                </Button>
              )}
              <Button
                size="sm"
                variant="ghost"
                className="text-destructive hover:bg-destructive/10"
                onClick={() => {
                  setDeleteOpen(true)
                }}
              >
                删除投递
              </Button>
            </div>
          </Section>
        </main>

        <aside className="space-y-4">
          <OverallAnalysis aid={Number(id)} status={app.status} />
          <Section
            title="JD 解读与匹配度"
            action={
              <>
                <Button size="sm" variant="ghost" onClick={openJdModal}>
                  岗位 JD
                </Button>
                {jdTab === 'interpret' && interpret && (
                  <Button size="sm" variant="ghost" onClick={() => runAi('interpret')} disabled={aiBusyKind !== ''}>
                    重新生成
                  </Button>
                )}
                {jdTab === 'match' && match && (
                  <Button size="sm" variant="ghost" onClick={() => runAi('match')} disabled={aiBusyKind !== ''}>
                    重新评估
                  </Button>
                )}
              </>
            }
          >
            {aiBusyKind !== '' && (
              <div className="mb-3 flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-sm" role="status">
                <CircleNotch className="size-4 animate-spin text-muted-foreground" aria-hidden />
                <span>
                  {aiBusyKind === 'interpret' ? 'JD 解读中…' : '匹配度评估中…'}完成后自动回显。
                </span>
              </div>
            )}
            <Tabs value={jdTab} onValueChange={(v) => setJdTab(v as 'interpret' | 'match')}>
              <TabsList className="mb-3">
                <TabsTrigger value="interpret">JD 解读</TabsTrigger>
                <TabsTrigger value="match">匹配度</TabsTrigger>
              </TabsList>
              <TabsContent value="interpret">
                {!interpret ? (
                  <div className="rounded-md border border-dashed border-border-strong p-3">
                    <p className="mb-2 text-sm text-foreground">同岗投递共享解读；匹配度按本投递简历。</p>
                    <Button size="sm" onClick={() => runAi('interpret')} disabled={aiBusyKind !== ''}>
                      <Sparkle weight="fill" className="size-4" aria-hidden /> 生成 JD 解读
                    </Button>
                  </div>
                ) : (
                  <div className="space-y-2">
                    <p className="text-sm leading-7">{interpret.overall}</p>
                    <AiList title="注意点" items={interpret.cautions} danger />
                  </div>
                )}
              </TabsContent>
              <TabsContent value="match">
                {!match ? (
                  <div className="rounded-md border border-dashed border-border-strong p-3">
                    <p className="mb-2 text-sm text-foreground">评估简历与该岗 JD 的匹配度。</p>
                    <Button size="sm" onClick={() => runAi('match')} disabled={aiBusyKind !== ''}>
                      <Sparkle weight="fill" className="size-4" aria-hidden /> 评估匹配度
                    </Button>
                  </div>
                ) : (
                  <div className="space-y-3">
                    <h3 className="text-sm font-semibold">
                      匹配度 · <span className="font-mono text-lg tabular-nums text-primary">{match.score}</span> / 100
                    </h3>
                    {match.summary && <p className="text-sm leading-7">{match.summary}</p>}
                    <AiList title="优势" items={match.strengths} />
                    <AiList title="差距清单" items={match.gaps} />
                    <Button size="sm" variant="secondary" onClick={makeGapPaper} disabled={busy}>
                      按差距生成陪练
                    </Button>
                  </div>
                )}
              </TabsContent>
            </Tabs>
          </Section>
          {app.note && (
            <Section title="备注">
              <p className="whitespace-pre-wrap text-sm leading-7">{app.note}</p>
            </Section>
          )}
        </aside>
      </div>

      {/* 放弃投递确认 */}
      <ConfirmDialog
        open={withdrawOpen}
        onOpenChange={setWithdrawOpen}
        destructive
        busy={busy}
        title="放弃投递？"
        description="状态将变更为「放弃」，投递和相关面试数据仍将保留。"
        confirmLabel="确认放弃"
        onConfirm={async () => {
          await apiPatch(`/api/applications/${id}`, { status: 'withdrawn' })
          await load()
          toast.success('已放弃该投递')
        }}
      />

      {/* 删除投递确认：手动输入「确认删除」才能提交；题目会迁入回收站保留 */}
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        destructive
        busy={busy}
        title="删除投递"
        description={
          <>
            将删除该投递与其面试轮次；<b>关联题目会保留</b>并移入回收站。此操作不可恢复。
          </>
        }
        confirmLabel="永久删除"
        confirmKeyword="确认删除"
        onConfirm={doDelete}
      />

      <Dialog open={jdOpen} onOpenChange={setJdOpen}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>岗位 JD · {app.position ?? ''}</DialogTitle>
          </DialogHeader>
          {jdText.trim() ? (
            <pre className="max-h-[60vh] overflow-auto whitespace-pre-wrap break-words rounded-md border border-border bg-muted/30 p-3 text-sm leading-7">
              {jdText}
            </pre>
          ) : (
            <p className="text-sm text-foreground">该岗位尚未填写 JD。到岗位详情粘贴职位描述。</p>
          )}
          <DialogFooter>
            <Button asChild>
              <Link to={`/positions/${app.position_id}`}>去岗位详情</Link>
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

function AiList({
  title,
  items,
  danger,
  onItem,
}: {
  title: string
  items?: string[]
  danger?: boolean
  onItem?: (item: string) => void
}) {
  const list = (items ?? []).filter(Boolean)
  if (list.length === 0) return null
  return (
    <div>
      <h3 className={`mb-1 text-xs font-semibold uppercase tracking-wide ${danger ? 'text-destructive' : 'text-muted-foreground'}`}>
        {title}
      </h3>
      <ul className="list-disc space-y-1 pl-5 text-sm leading-7">
        {list.map((x, i) => (
          <li key={i}>
            {x}
            {onItem && (
              <Button size="sm" variant="ghost" className="ml-2 h-6 px-2 text-xs" onClick={() => onItem(x)}>
                记入题库
              </Button>
            )}
          </li>
        ))}
      </ul>
    </div>
  )
}

/** 投递整体分析（ADR-0014 + proposal §1.7）：终态解锁；AI 结构化复盘 + 手写区（manual_content，AI 永不覆盖）。 */
function OverallAnalysis({ aid, status }: { aid: number; status: string }) {
  const terminal = ['offer', 'rejected', 'withdrawn'].includes(status)
  const aiJobs = useAiJobs()
  const running = isRunning(aiJobs, 'overall', aid)
  const [analysis, setAnalysis] = useState<any>(null)
  const [loaded, setLoaded] = useState(false)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState('')
  const [showReport, setShowReport] = useState(false)

  async function load() {
    try {
      const v = await apiGet(`/api/applications/${aid}/overall-analysis`)
      setAnalysis(v.analysis ?? null)
      trackRunning(v.ai_jobs)
    } catch {
      /* 静默：未生成时无数据 */
    } finally {
      setLoaded(true)
    }
  }

  useEffect(() => {
    if (!terminal) return
    load()
    return onJobDone('overall', aid, (ok) => {
      if (!ok) setErr('AI 整体复盘失败，请重试')
      load()
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [aid, terminal])

  function runAi() {
    setErr('')
    startAiJob('overall', aid, `/api/applications/${aid}/overall-analysis/ai`).catch((e) => setErr(e.message))
  }

  const manual: string = analysis?.manual_content ?? ''
  const [draftManual, setDraftManual] = useState<string>('')
  useEffect(() => setDraftManual(manual), [manual])

  async function saveManual() {
    setBusy(true)
    try {
      await apiPut(`/api/applications/${aid}/overall-analysis`, { content: draftManual })
      await load()
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
    }
  }

  if (!terminal) {
    return (
      <Section title="整体分析">
        <p className="text-sm text-muted-foreground">
          投递走向终态（Offer / 拒 / 弃）后解锁——届时结合各轮复盘做整体分析。
        </p>
      </Section>
    )
  }

  const hasStructured = analysis && typeof analysis === 'object' && (analysis.summary || analysis.report)

  return (
    <Section
      title="整体分析"
      action={
        <>
          <Button
            size="sm"
            variant={hasStructured ? 'ghost' : 'default'}
            onClick={runAi}
            disabled={running}
          >
            <Sparkle weight="fill" className="size-4" aria-hidden />
            {running ? 'AI 复盘进行中…' : 'AI 整体复盘'}
          </Button>
          {(analysis?.content || manual || draftManual.trim()) && (
            <Button
              size="sm"
              variant={analysis?.summary ? 'ghost' : 'default'}
              disabled={busy}
              onClick={saveManual}
            >
              {busy ? '保存中…' : '保存手写'}
            </Button>
          )}
        </>
      }
    >
      {running && (
        <p className="mb-2 flex items-center gap-2 text-sm text-muted-foreground">
          <CircleNotch className="size-4 animate-spin" aria-hidden />
          正在基于 JD、简历与各轮第一手回答做跨轮归因…完成后自动回显。
        </p>
      )}
      {err && (
        <p role="alert" className="mb-2 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {!hasStructured && loaded && !running && (
        <p className="mb-2 text-sm text-muted-foreground">
          点「AI 整体复盘」：跨轮次系统性归因——表现评级、能力矩阵、关键失分点与下一场行动方案。
        </p>
      )}

      {hasStructured && <OverallStructured a={analysis} showReport={showReport} setShowReport={setShowReport} />}

      {/* 手写区：AI 永不覆盖（迁移为 manual_content） */}
      <div className="mt-3">
        <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          手写笔记{manual ? '（已保存）' : ''} · AI 不覆盖
        </div>
        <Textarea
          rows={4}
          value={draftManual}
          placeholder="整个面试流程走完后的整体分析：这家公司看重什么、自己的差距、下次策略…"
          onChange={(e) => setDraftManual(e.target.value)}
        />
      </div>
    </Section>
  )
}

/** 评级 → 语义类（词表：优秀/良好/高/中高=绿，一般/中/中低=琥珀，偏弱/低=红） */
const GRADE_CLASS: Record<string, string> = {
  优秀: 'text-success',
  良好: 'text-success',
  高: 'text-success',
  中高: 'text-success',
  一般: 'text-warning',
  中: 'text-warning',
  中低: 'text-warning',
  偏弱: 'text-destructive',
  低: 'text-destructive',
}

function OvBadges({ items }: { items: { label: string; value?: string }[] }) {
  const list = items.filter((x) => x.value)
  if (list.length === 0) return null
  return (
    <div className="mb-2.5 flex flex-wrap gap-1.5">
      {list.map((x) => (
        <span
          key={x.label}
          className={`rounded-full border border-border px-2 py-0.5 text-xs font-medium ${GRADE_CLASS[x.value!] ?? 'text-muted-foreground'}`}
        >
          {x.label} · {x.value}
        </span>
      ))}
    </div>
  )
}

function OvList({
  title,
  items,
  tone,
}: {
  title: string
  items?: string[]
  tone?: 'pass' | 'warn' | 'danger'
}) {
  const list = (items ?? []).filter(Boolean)
  if (list.length === 0) return null
  const toneCls =
    tone === 'pass' ? 'text-success' : tone === 'warn' ? 'text-warning' : tone === 'danger' ? 'text-destructive' : 'text-muted-foreground'
  return (
    <div>
      <div className={`mb-1 text-xs font-semibold uppercase tracking-wide ${toneCls}`}>{title}</div>
      <ul className="list-disc space-y-1 pl-5 text-sm leading-7">
        {list.map((x, i) => (
          <li key={i}>{x}</li>
        ))}
      </ul>
    </div>
  )
}

/** 结构化渲染：proposal §1.7 各区块 */
function OverallStructured({
  a,
  showReport,
  setShowReport,
}: {
  a: any
  showReport: boolean
  setShowReport: (v: boolean) => void
}) {
  return (
    <div>
      <OvBadges
        items={[
          { label: '综合表现', value: a.performance },
          { label: '岗位匹配', value: a.match },
          { label: '置信度', value: a.confidence },
        ]}
      />
      {a.summary && (
        <p className="mb-2.5 text-sm leading-7">
          <Markdown text={String(a.summary)} />
        </p>
      )}

      <div className="grid grid-cols-2 gap-3">
        <OvList title="最大优势" items={a.strengths} tone="pass" />
        <OvList title="最大风险" items={a.risks} tone="warn" />
      </div>
      <div className="mt-2">
        <OvList title="最关键失分点" items={a.loss_points} tone="danger" />
      </div>

      {/* 保留 vs 重练 对照 */}
      {((a.keep_answers ?? []).length > 0 || (a.retrain_answers ?? []).length > 0) && (
        <div className="mt-3 grid grid-cols-2 gap-3">
          <div>
            <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-success">值得保留的回答</div>
            {(a.keep_answers ?? []).filter(Boolean).map((x: string, i: number) => (
              <p key={i} className="mb-1 rounded-r-md border-l-[3px] border-success bg-muted/60 px-2.5 py-1.5 text-xs leading-6">
                {x}
              </p>
            ))}
          </div>
          <div>
            <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-warning">最应重练的回答</div>
            {(a.retrain_answers ?? []).filter(Boolean).map((x: string, i: number) => (
              <p key={i} className="mb-1 rounded-r-md border-l-[3px] border-warning bg-muted/60 px-2.5 py-1.5 text-xs leading-6">
                {x}
              </p>
            ))}
          </div>
        </div>
      )}

      {/* 能力矩阵 */}
      {(a.ability_matrix ?? []).filter((x: any) => x?.ability).length > 0 && (
        <div className="mt-3">
          <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">岗位能力矩阵</div>
          <table className="w-full border-collapse text-xs">
            <thead>
              <tr>
                <th className="py-1 pr-2 text-left font-semibold text-muted-foreground">能力</th>
                <th className="py-1 pr-2 text-left font-semibold text-muted-foreground">重要性</th>
                <th className="py-1 pr-2 text-left font-semibold text-muted-foreground">证据</th>
                <th className="py-1 text-left font-semibold text-muted-foreground">风险</th>
              </tr>
            </thead>
            <tbody>
              {(a.ability_matrix as any[]).map((row, i) => {
                const weak = row.importance === '高' && (!row.evidence || row.evidence === '无')
                return (
                  <tr key={i} className={`border-t border-border ${weak ? 'bg-destructive/5' : ''}`}>
                    <td className="py-1.5 pr-2 align-top">
                      {row.ability}
                      {weak && (
                        <b className="ml-1.5 text-destructive">重要但证明不足</b>
                      )}
                    </td>
                    <td className="py-1.5 pr-2 align-top">{row.importance}</td>
                    <td className="py-1.5 pr-2 align-top text-muted-foreground">{row.evidence || '—'}</td>
                    <td className="py-1.5 align-top text-muted-foreground">{row.risk || '—'}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}

      {/* 行动方案 */}
      {(a.improvements ?? []).filter((x: any) => x?.action).length > 0 && (
        <div className="mt-3">
          <div className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">下一场行动方案</div>
          {(a.improvements as any[])
            .slice()
            .sort((x: any, y: any) => (x.priority ?? 9) - (y.priority ?? 9))
            .map((x: any, i: number) => {
              const bar = x.priority === 1 ? 'border-danger' : x.priority === 2 ? 'border-warning' : 'border-info'
              return (
                <div key={i} className={`mb-1.5 rounded-r-md border-l-[3px] bg-muted/60 px-3 py-2 ${bar}`}>
                  <b className="text-sm">
                    P{x.priority ?? '?'} · {x.problem}
                  </b>
                  {x.action && <p className="mt-0.5 text-xs leading-6">→ {x.action}</p>}
                </div>
              )
            })}
        </div>
      )}

      {/* 完整报告折叠 */}
      {a.report && (
        <div className="mt-3">
          <Button size="sm" variant="ghost" onClick={() => setShowReport(!showReport)}>
            {showReport ? '收起完整报告' : '查看完整报告'}
          </Button>
          {showReport && (
            <div className="mt-2 rounded-md bg-muted/60 p-3">
              <Markdown text={String(a.report)} />
            </div>
          )}
        </div>
      )}
    </div>
  )
}
