import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { CheckCircle, CheckSquare, Lightning, MapPin, PaperPlaneTilt, PencilLine, Sparkle, Square } from '@phosphor-icons/react'
import { apiGet, apiPatch, apiPost } from '../api/client'
import { isRunning, onJobDone, startAiJob, trackRunning, useAiJobs } from '../ai/jobs'
import { APP_STATUS, type Position, type PositionApplication, type PositionPredictResponse } from '../api/types'
import StageTimeline from '../components/StageTimeline'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { SemBadge, type BadgeSem } from '../components/SemBadge'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'

const STATUS_SEM: Record<string, BadgeSem> = {
  applied: 'neutral',
  callback: 'warn',
  interviewing: 'info',
  offer: 'pass',
  rejected: 'danger',
  withdrawn: 'neutral',
}

/** 岗位详情（ADR-0012 D4）：JD / 地点 / 该岗所有投递 + 发起投递 */
export default function PositionDetail() {
  const { id } = useParams()
  const nav = useNavigate()
  const [position, setPosition] = useState<Position | null>(null)
  const [apps, setApps] = useState<PositionApplication[]>([])
  const [err, setErr] = useState('')
  const [editing, setEditing] = useState(false)
  const [form, setForm] = useState({ title: '', department: '', location: '', jd_text: '' })
  const [applying, setApplying] = useState(false)
  const [applyForm, setApplyForm] = useState({ channel: '', note: '' })

  const [predictData, setPredictData] = useState<PositionPredictResponse | null>(null)
  const [selectedIndexes, setSelectedIndexes] = useState<number[]>([])
  const [ingesting, setIngesting] = useState(false)
  const [drilling, setDrilling] = useState(false)
  const aiJobs = useAiJobs()
  const pid = Number(id)
  const interpreting = isRunning(aiJobs, 'jd_interpret', pid)
  const predicting = isRunning(aiJobs, 'position_predict', pid)

  async function load() {
    const p = await apiGet(`/api/positions/${id}`)
    setPosition(p)
    setForm({ title: p.title, department: p.department ?? '', location: p.location ?? '', jd_text: p.jd_text ?? '' })
    setApps(await apiGet(`/api/positions/${id}/applications`))
    const next = p.predict_result ?? null
    setPredictData(next)
    if (next?.questions?.length) {
      setSelectedIndexes(next.questions.map((_: unknown, i: number) => i))
    }
    trackRunning(p.ai_jobs)
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  useEffect(() => {
    if (!Number.isFinite(pid)) return
    const offInterpret = onJobDone('jd_interpret', pid, (ok) => {
      if (ok) {
        toast.success('JD 解读完成，本岗投递将共享')
        load().catch(() => {})
      } else {
        setErr('JD 解读失败，请重试')
      }
    })
    const offPredict = onJobDone('position_predict', pid, (ok) => {
      if (ok) {
        toast.success('岗位押题完成')
        load().catch(() => {})
      } else {
        setErr('岗位押题失败，请重试')
      }
    })
    return () => {
      offInterpret()
      offPredict()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  async function savePosition() {
    setErr('')
    try {
      await apiPatch(`/api/positions/${id}`, {
        title: form.title.trim() || null,
        department: form.department.trim() || null,
        location: form.location,
        jd_text: form.jd_text,
      })
      setEditing(false)
      toast.success('岗位已保存')
      await load()
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function apply() {
    setErr('')
    try {
      const a = await apiPost('/api/applications', {
        company_id: position?.company_id,
        position: position?.title,
        channel: applyForm.channel.trim() || null,
        note: applyForm.note.trim() || null,
      })
      nav(`/applications/${a.id}`)
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function handlePredict() {
    if (!position?.jd_text || !position.jd_text.trim()) {
      toast.error('请先填写岗位 JD 职位描述')
      return
    }
    setErr('')
    try {
      await startAiJob('position_predict', pid, `/api/positions/${id}/predict`)
    } catch (e: any) {
      setErr(e.message || '生成押题失败')
    }
  }

  // 一键流转沉淀入自录题库
  async function handleIngest() {
    if (!predictData || selectedIndexes.length === 0) return
    setIngesting(true)
    try {
      const selectedQs = selectedIndexes.map((i) => predictData.questions[i]).filter(Boolean)
      const res = await apiPost(`/api/positions/${id}/predict/ingest`, { questions: selectedQs })
      toast.success(`已沉淀 ${res.created_count} 道题目入自录题库并加入今日复习队列`)
    } catch (e: any) {
      toast.error(e.message || '沉淀入库失败')
    } finally {
      setIngesting(false)
    }
  }

  // 一键以此押题发起针对性模拟练习
  async function handleDrill() {
    if (!predictData || selectedIndexes.length === 0) return
    setDrilling(true)
    try {
      const selectedQs = selectedIndexes.map((i) => predictData.questions[i]).filter(Boolean)
      const res = await apiPost(`/api/positions/${id}/predict/drill`, {
        title: `${position?.company} · ${position?.title} 考前专项押题模拟`,
        questions: selectedQs,
      })
      toast.success('已生成专属考官题本模拟面试')
      nav(`/drills/${res.drill_id}`)
    } catch (e: any) {
      toast.error(e.message || '发起模考失败')
    } finally {
      setDrilling(false)
    }
  }

  if (!position) {
    return <div className="py-24 text-center text-muted-foreground">{err || '加载中…'}</div>
  }

  return (
    <div>
      <nav aria-label="面包屑" className="mb-2 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link to="/companies" className="hover:text-primary">
          企业
        </Link>
        <span aria-hidden>/</span>
        <Link to={`/companies/${position.company_id}`} className="hover:text-primary">
          {position.company}
        </Link>
        <span aria-hidden>/</span>
        <span className="text-foreground">{position.title}</span>
      </nav>

      <PageHeader
        title={position.title}
        meta={
          <>
            <span className="font-semibold text-foreground">{position.company}</span>
            {position.department && (
              <span className="rounded bg-secondary border border-border px-2 py-0.5 text-xs font-medium text-heading">
                {position.department}
              </span>
            )}
            {position.location && (
              <span className="inline-flex items-center gap-1">
                <MapPin className="size-3.5" aria-hidden /> {position.location}
              </span>
            )}
            <span>· 共 {apps.length} 次投递</span>
          </>
        }
        actions={
          <>
            {!editing && (
              <Button variant="outline" onClick={() => setEditing(true)}>
                <PencilLine className="size-4" aria-hidden /> 编辑岗位
              </Button>
            )}
            <Button onClick={() => setApplying((v) => !v)}>
              <PaperPlaneTilt weight="fill" className="size-4" aria-hidden /> 发起投递
            </Button>
          </>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {/* 发起投递（本次管道记录） */}
      {applying && (
        <Section title="发起投递" className="mb-4">
          <p className="mb-2 text-xs text-muted-foreground">
            为该岗位新建一条投递记录；JD 与地点沿用岗位信息。
          </p>
          <form
            className="grid grid-cols-1 gap-3 sm:grid-cols-2"
            onSubmit={(e) => {
              e.preventDefault()
              apply()
            }}
          >
            <FormField label="渠道" htmlFor="ap-channel" hint="可选">
              <Input
                id="ap-channel"
                placeholder="内推 / 招聘网…"
                value={applyForm.channel}
                onChange={(e) => setApplyForm((f) => ({ ...f, channel: e.target.value }))}
              />
            </FormField>
            <FormField label="备注 / 待跟进" htmlFor="ap-note" hint="可选">
              <Input
                id="ap-note"
                placeholder="例如：周五约面"
                value={applyForm.note}
                onChange={(e) => setApplyForm((f) => ({ ...f, note: e.target.value }))}
              />
            </FormField>
            <div className="flex items-center gap-2 sm:col-span-2">
              <Button type="submit">
                <PaperPlaneTilt weight="fill" className="size-4" aria-hidden /> 投递
              </Button>
              <Button type="button" variant="ghost" onClick={() => setApplying(false)}>
                取消
              </Button>
            </div>
          </form>
        </Section>
      )}

      {/* JD（岗位属性，可编辑） */}
      <Section
        title="岗位 JD"
        action={
          !editing ? (
            <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
              编辑
            </Button>
          ) : (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setEditing(false)
                setForm({
                  title: position.title,
                  department: position.department ?? '',
                  location: position.location ?? '',
                  jd_text: position.jd_text ?? '',
                })
              }}
            >
              取消
            </Button>
          )
        }
      >
        {!editing ? (
          <p className="whitespace-pre-wrap break-words text-sm leading-7">
            {position.jd_text || '未填写 JD——点「编辑」粘贴职位描述'}
          </p>
        ) : (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <FormField label="岗位名称" htmlFor="pd-title">
              <Input id="pd-title" value={form.title} onChange={(e) => setForm((f) => ({ ...f, title: e.target.value }))} />
            </FormField>
            <FormField label="部门" htmlFor="pd-dept">
              <Input id="pd-dept" value={form.department} onChange={(e) => setForm((f) => ({ ...f, department: e.target.value }))} />
            </FormField>
            <FormField label="工作地点" htmlFor="pd-loc">
              <Input id="pd-loc" value={form.location} onChange={(e) => setForm((f) => ({ ...f, location: e.target.value }))} />
            </FormField>
            <FormField label="JD 原文" htmlFor="pd-jd" className="sm:col-span-2">
              <Textarea
                id="pd-jd"
                rows={8}
                value={form.jd_text}
                onChange={(e) => setForm((f) => ({ ...f, jd_text: e.target.value }))}
              />
            </FormField>
            <div className="sm:col-span-2">
              <Button size="sm" onClick={savePosition}>
                保存
              </Button>
            </div>
          </div>
        )}
      </Section>

      <Section
        title="JD 深度解读"
        className="mt-4"
        action={
          <Button
            size="sm"
            variant="outline"
            disabled={interpreting || !position.jd_text?.trim()}
            onClick={async () => {
              setErr('')
              try {
                await startAiJob('jd_interpret', Number(id), `/api/positions/${id}/interpret`)
              } catch (e: any) {
                setErr(e.message)
              }
            }}
          >
            <Sparkle weight="fill" className="size-3.5" />
            {interpreting ? '解读中…' : position.jd_interpret ? '重新解读' : '生成解读'}
          </Button>
        }
      >
        <p className="mb-2 text-sm text-muted-foreground">岗位固有属性，同岗下所有投递共享；匹配度仍在各投递详情评估。</p>
        {position.jd_interpret?.overall ? (
          <div className="space-y-2">
            <p className="text-sm leading-7 text-foreground">{position.jd_interpret.overall}</p>
            {(position.jd_interpret.cautions ?? []).length > 0 && (
              <ul className="list-disc space-y-1 pl-5 text-sm">
                {position.jd_interpret.cautions!.map((c, i) => (
                  <li key={i}>{c}</li>
                ))}
              </ul>
            )}
          </div>
        ) : (
          <p className="text-sm text-foreground">{interpreting ? '正在解读岗位 JD…' : '填写 JD 后生成一次，本岗投递共享。'}</p>
        )}
      </Section>

      <Section
        title="岗位精准押题与考点预测"
        className="mt-4"
        action={
          <Button
            size="sm"
            variant="outline"
            onClick={handlePredict}
            disabled={predicting || !position.jd_text?.trim()}
            title={!position.jd_text?.trim() ? '请先在上方补充岗位 JD 文本' : '结合 JD 业务场景预测高频面试题'}
          >
            <Sparkle weight="fill" className="size-3.5" />
            {predicting ? '预测中…' : predictData ? '重新预测' : 'AI 考点精准押题'}
          </Button>
        }
      >
        {predicting && !predictData ? (
          <p className="text-sm text-foreground">正在结合 JD 预测高频考点…完成后自动回显。</p>
        ) : !predictData ? (
          <p className="text-sm text-foreground">
            {position.jd_text?.trim()
              ? '结合本岗技术栈与业务场景预测高频考题；完成后可沉淀入题库或发起模考。'
              : '填写 JD 后再预测。'}
          </p>
        ) : (
          <div className="space-y-4">
            {predicting && (
              <p className="text-sm text-foreground">正在生成新一版押题…当前列表为已保存结果，完成后更新。</p>
            )}
            {predictData.summary && (
              <div className="rounded-md border border-border bg-muted/40 p-3 text-xs leading-6 text-foreground">
                <span className="font-bold text-primary">考点综述：</span>
                {predictData.summary}
              </div>
            )}

            {predictData.text_fallback ? (
              <div className="whitespace-pre-wrap break-words rounded-md bg-muted/30 p-3 font-mono text-xs leading-6">
                {predictData.text_fallback}
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center justify-between text-xs text-muted-foreground">
                  <span>共预测 {predictData.questions.length} 道高频考题（已勾选 {selectedIndexes.length} 题）</span>
                  <button
                    onClick={() => {
                      if (selectedIndexes.length === predictData.questions.length) {
                        setSelectedIndexes([])
                      } else {
                        setSelectedIndexes(predictData.questions.map((_, i) => i))
                      }
                    }}
                    className="hover:text-foreground"
                  >
                    {selectedIndexes.length === predictData.questions.length ? '取消全选' : '全选'}
                  </button>
                </div>

                <div className="grid gap-3">
                  {predictData.questions.map((q, idx) => {
                    const isSelected = selectedIndexes.includes(idx)
                    return (
                      <div
                        key={idx}
                        onClick={() => {
                          setSelectedIndexes((prev) =>
                            isSelected ? prev.filter((i) => i !== idx) : [...prev, idx]
                          )
                        }}
                        className={`cursor-pointer rounded-lg p-3.5 transition-all ${
                          isSelected
                            ? 'chip-selected'
                            : 'border border-border bg-card hover:border-border/80'
                        }`}
                      >
                        <div className="flex items-start gap-2.5">
                          <div className="mt-0.5 text-primary">
                            {isSelected ? (
                              <CheckSquare weight="fill" className="size-4" />
                            ) : (
                              <Square className="size-4 text-muted-foreground" />
                            )}
                          </div>
                          <div className="min-w-0 flex-1 space-y-1.5">
                            <div className="flex flex-wrap items-center gap-1.5">
                              <span className="rounded bg-secondary border border-border-strong px-1.5 py-0.5 font-mono text-[10px] font-bold text-heading">
                                {q.category || '技术考点'}
                              </span>
                              {q.probability != null && (
                                <span className="rounded bg-success/10 px-1.5 py-0.5 font-mono text-[10px] font-bold text-success">
                                  考察概率 {q.probability}%
                                </span>
                              )}
                            </div>
                            <div className="text-sm font-semibold text-foreground">{q.content}</div>
                            {q.focus_points && q.focus_points.length > 0 && (
                              <div className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
                                <span className="text-[10px]">考点：</span>
                                {q.focus_points.map((pt, pidx) => (
                                  <span key={pidx} className="rounded bg-muted px-1.5 py-0.2 font-mono text-[10px]">
                                    {pt}
                                  </span>
                                ))}
                              </div>
                            )}
                            {q.sample_direction && (
                              <div className="text-xs text-muted-foreground">
                                <span className="text-foreground/80 font-medium">回答亮点思路：</span>
                                {q.sample_direction}
                              </div>
                            )}
                          </div>
                        </div>
                      </div>
                    )
                  })}
                </div>

                {/* 资产流转操作栏 */}
                <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border pt-3">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={selectedIndexes.length === 0 || ingesting}
                    onClick={handleIngest}
                    title="将勾选的押题批量沉淀入自录题库，并自动排入今日复习队列"
                  >
                    <CheckCircle className="size-3.5" />
                    {ingesting ? '正在沉淀入库…' : `沉淀入题库并待复习 (${selectedIndexes.length})`}
                  </Button>
                  <Button
                    size="sm"
                    disabled={selectedIndexes.length === 0 || drilling}
                    onClick={handleDrill}
                    title="以此押题集快速开启一场全真模拟陪练"
                  >
                    <Lightning weight="bold" className="size-3.5" />
                    {drilling ? '正在生成模考…' : `以此押题发起针对模考 (${selectedIndexes.length})`}
                  </Button>
                </div>
              </div>
            )}
          </div>
        )}
      </Section>

      {/* 该岗所有投递 */}
      <Section title="投递记录" className="mt-4">
        {apps.length === 0 ? (
          <p className="text-sm text-muted-foreground">还没有投递记录，可点击上方「发起投递」开启面试管道</p>
        ) : (
          <div className="grid gap-3 sm:grid-cols-1">
            {apps.map((a) => (
              <div
                key={a.id}
                className="group relative rounded-xl border border-border bg-card p-4"
              >
                <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border/60 pb-3">
                  <div className="flex items-center gap-2">
                    <SemBadge sem={STATUS_SEM[a.status] ?? 'neutral'} className="text-xs px-2.5 py-0.5">
                      {APP_STATUS[a.status]}
                    </SemBadge>
                    <span className="font-mono text-xs text-muted-foreground">
                      投递于 {a.applied_at.slice(0, 10)}
                    </span>
                    {a.channel && (
                      <span className="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                        {a.channel}
                      </span>
                    )}
                  </div>
                  <div className="flex items-center gap-2">
                    {a.salary && (
                      <span className="rounded bg-success/10 px-2 py-0.5 font-mono text-xs font-semibold text-success">
                        {a.salary}
                      </span>
                    )}
                    <Link
                      to={`/applications/${a.id}`}
                      className="inline-flex min-h-[36px] items-center gap-1 rounded-lg border border-border bg-muted/30 px-3 py-1.5 text-xs font-medium text-foreground transition-colors duration-150 hover:bg-muted"
                    >
                      进入详情 →
                    </Link>
                  </div>
                </div>

                {/* 节点时间线（统一组件）：投递 → 各轮(✓✗·) → 终态 */}
                <div className="mt-3">
                  <StageTimeline stages={a.interview_stages} status={a.status} />
                </div>
              </div>
            ))}
          </div>
        )}
      </Section>
    </div>
  )
}
