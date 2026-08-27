import { useEffect, useMemo, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'
import { apiGet, apiPost } from '../api/client'
import type { Application, DrillDetail, InterviewerNotes, InterviewerPersona, QuestionRow } from '../api/types'
import { onAiEvent, startAiJob, trackRunning } from '../ai/jobs'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { toast } from 'sonner'
import { cn } from '@/lib/utils'

const selectCls =
  'h-9 w-full rounded-md border border-input bg-card px-2 text-sm focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

const STEPS = ['选择面试官', '背景与考点', '智能备课', '进入考场'] as const

const NOTE_SECTIONS: { label: string; key: keyof Pick<InterviewerNotes, 'job_requirements' | 'candidate_facts' | 'risk_signals' | 'next_followups'> }[] = [
  { label: '岗位要求', key: 'job_requirements' },
  { label: '候选人事实', key: 'candidate_facts' },
  { label: '风险信号', key: 'risk_signals' },
  { label: '建议追问', key: 'next_followups' },
]

export default function DrillNew() {
  const [sp] = useSearchParams()
  const [position, setPosition] = useState(sp.get('position') || '')
  const [direction, setDirection] = useState(sp.get('direction') || '')
  const [target, setTarget] = useState(5)
  const [title, setTitle] = useState(sp.get('title') || '')
  const [refs, setRefs] = useState(sp.get('refs') || '')
  const [apps, setApps] = useState<Application[]>([])
  const lockedPersonaId = sp.get('persona') ? Number(sp.get('persona')) : null
  const [personas, setPersonas] = useState<InterviewerPersona[]>([])
  const [selectedPersonaId, setSelectedPersonaId] = useState<number | ''>(lockedPersonaId ?? '')
  const [appId, setAppId] = useState<number | ''>('')
  const [questions, setQuestions] = useState<QuestionRow[]>([])
  const [qSearch, setQSearch] = useState('')
  const [selectedQids, setSelectedQids] = useState<number[]>([])
  const [dossierSummary, setDossierSummary] = useState(sp.get('dossier') || '')
  const [matching, setMatching] = useState(false)
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)
  const [resumeParsed, setResumeParsed] = useState(true)
  const [parseDeferred, setParseDeferred] = useState(false)
  const [parseGateOpen, setParseGateOpen] = useState(false)
  const navigate = useNavigate()
  // 票 09：四步向导；?persona= 直达跳过 Step1 并锁定
  const [step, setStep] = useState(() => (lockedPersonaId != null ? 1 : 0))
  const [createdId, setCreatedId] = useState<number | null>(null)
  const [prepDrill, setPrepDrill] = useState<DrillDetail | null>(null)

  useEffect(() => {
    const sName = sp.get('skill_name')
    const sTag = sp.get('tag') || sp.get('tags')
    const summary = sName || sTag || sp.get('dossier')
    if (summary) setDossierSummary(summary)
  }, [sp])

  useEffect(() => {
    apiGet('/api/personas')
      .then((v) => setPersonas(v.items ?? []))
      .catch(() => {})
    apiGet('/api/resume')
      .then((r) => {
        const p = r?.parsed
        const has =
          p &&
          typeof p === 'object' &&
          ((typeof p.name === 'string' && p.name.trim()) ||
            (Array.isArray(p.experience) && p.experience.length > 0) ||
            (Array.isArray(p.projects) && p.projects.length > 0) ||
            (Array.isArray(p.skills) && p.skills.length > 0))
        setResumeParsed(!!has)
      })
      .catch(() => setResumeParsed(false))
  }, [])

  useEffect(() => {
    apiGet('/api/applications')
      .then((list: Application[]) => {
        const active = list.filter((a) => !['offer', 'rejected', 'withdrawn'].includes(a.status))
        setApps(active)
        const targetAppId = sp.get('app_id') ? Number(sp.get('app_id')) : active.length > 0 ? active[0].id : ''
        if (targetAppId) setAppId(targetAppId)
      })
      .catch(() => {})

    apiGet('/api/questions')
      .then((list: QuestionRow[]) => {
        const qList = list || []
        setQuestions(qList)
        const sIdsStr = sp.get('skill_ids')
        const sIds: number[] = sIdsStr
          ? sIdsStr.split(',').map(Number).filter(Boolean)
          : sp.get('skill_id')
            ? [Number(sp.get('skill_id'))]
            : []
        const sTag = (sp.get('tag') || sp.get('tags') || sp.get('skill_name') || '').toLowerCase()
        if (sIds.length > 0 || sTag) {
          const matched = qList.filter((q) => {
            if (sIds.length > 0 && q.skill_id && sIds.includes(q.skill_id)) return true
            if (sTag && q.tags?.some((t) => t.toLowerCase() === sTag || t.toLowerCase().includes(sTag))) return true
            if (sTag && q.skill_name && (q.skill_name.toLowerCase() === sTag || q.skill_name.toLowerCase().includes(sTag)))
              return true
            return false
          })
          if (matched.length > 0) setSelectedQids(matched.map((m) => m.id))
        }
      })
      .catch(() => {})
  }, [sp])

  const selectedApp = apps.find((a) => a.id === appId)
  const selectedPersona = personas.find((p) => p.id === selectedPersonaId)

  const sortedPersonas = useMemo(() => {
    const classic = personas.filter((p) => p.builtin && p.name === '经典面试官')
    const otherBuiltin = personas.filter((p) => p.builtin && p.name !== '经典面试官')
    const custom = personas.filter((p) => !p.builtin)
    return [...classic, ...otherBuiltin, ...custom]
  }, [personas])

  const dynamicTagCounts = useMemo(() => {
    const tagMap = new Map<string, number>()
    const pool = selectedApp?.company
      ? questions.filter((q) => q.company && q.company.toLowerCase().includes(selectedApp.company!.toLowerCase()))
      : questions
    const finalPool = pool.length > 0 ? pool : questions
    for (const q of finalPool) {
      for (const t of q.tags || []) {
        const trimmed = t.trim()
        if (trimmed) tagMap.set(trimmed, (tagMap.get(trimmed) || 0) + 1)
      }
    }
    return Array.from(tagMap.entries())
      .map(([name, count]) => ({ name, count }))
      .sort((a, b) => b.count - a.count)
      .slice(0, 16)
  }, [questions, selectedApp])

  function autoMatchQuestions() {
    const kw =
      qSearch.trim().toLowerCase() ||
      dossierSummary.trim().toLowerCase() ||
      selectedApp?.position?.toLowerCase() ||
      selectedApp?.company?.toLowerCase() ||
      position.trim().toLowerCase() ||
      direction.trim().toLowerCase()
    if (!kw) {
      toast.info('请在题库搜索框输入关键词，或填写考点侧重/岗位方向')
      return
    }
    setMatching(true)
    try {
      const tokens = kw
        .split(/[\s,，、/／()（）+＋-]+/)
        .map((s) => s.trim().toLowerCase())
        .filter((s) => s.length >= 2)
      if (tokens.length === 0) tokens.push(kw)
      const scored = questions
        .map((q) => {
          const c = (q.content || '').toLowerCase()
          const tags = (q.tags || []).map((t) => t.toLowerCase())
          const comp = (q.company || '').toLowerCase()
          const sName = (q.skill_name || '').toLowerCase()
          let score = 0
          for (const t of tokens) {
            if (c.includes(t)) score += 2
            if (tags.some((tag) => tag.includes(t))) score += 3
            if (comp.includes(t)) score += 2
            if (sName.includes(t)) score += 3
          }
          return { id: q.id, score }
        })
        .filter((s) => s.score > 0)
        .sort((a, b) => b.score - a.score)
      const topMatches = scored.slice(0, 5).map((s) => s.id)
      if (topMatches.length > 0) {
        setSelectedQids((prev) => Array.from(new Set([...prev, ...topMatches])))
        toast.success(`已匹配 ${topMatches.length} 道关联真题`)
      } else {
        toast.info('本地题库中未匹配到高关联度真题，可手动勾选')
      }
    } finally {
      setMatching(false)
    }
  }

  function toggleQuestion(id: number) {
    setSelectedQids((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]))
  }

  function toggleTag(tagName: string) {
    const matched = questions.filter((q) => q.tags && q.tags.some((t) => t.toLowerCase() === tagName.toLowerCase()))
    const matchedIds = matched.map((q) => q.id)
    if (matchedIds.length === 0) {
      setDossierSummary(tagName)
      return
    }
    const allSelected = matchedIds.every((id) => selectedQids.includes(id))
    if (allSelected) {
      setSelectedQids((prev) => prev.filter((id) => !matchedIds.includes(id)))
      toast.info(`已取消勾选 ${tagName} 关联的 ${matchedIds.length} 道题目`)
    } else {
      setSelectedQids((prev) => Array.from(new Set([...prev, ...matchedIds])))
      toast.success(`已圈定 ${tagName} 关联的 ${matchedIds.length} 道题目`)
    }
  }

  function buildCreatePayload() {
    const sIdsStr = sp.get('skill_ids')
    const sIds = sIdsStr
      ? sIdsStr.split(',').map(Number).filter(Boolean)
      : sp.get('skill_id')
        ? [Number(sp.get('skill_id'))]
        : undefined
    const tagsParam = sp.get('tag') ? [sp.get('tag')!] : sp.get('tags') ? sp.get('tags')!.split(',') : undefined
    const hasDossier = dossierSummary.trim() || selectedQids.length > 0 || (sIds && sIds.length > 0) || (tagsParam && tagsParam.length > 0)
    return {
      kind: 'interview' as const,
      title: title || undefined,
      position: position.trim() || undefined,
      direction: direction.trim() || undefined,
      persona_id: selectedPersonaId === '' ? undefined : selectedPersonaId,
      target_questions: target,
      references: refs.trim() || undefined,
      application_id: appId === '' ? undefined : appId,
      dossier: hasDossier
        ? {
            summary: dossierSummary.trim() || undefined,
            question_ids: selectedQids.length > 0 ? selectedQids : undefined,
            skill_id: sp.get('skill_id') ? Number(sp.get('skill_id')) : undefined,
            skill_ids: sIds && sIds.length > 0 ? sIds : undefined,
            tags: tagsParam,
          }
        : undefined,
    }
  }

  async function refreshPrep(id: number) {
    const d: DrillDetail = await apiGet(`/api/drills/${id}`)
    setPrepDrill(d)
    trackRunning(d.ai_jobs)
    return d
  }

  async function createAndStartPrep() {
    setErr('')
    setBusy(true)
    try {
      let id = createdId
      if (id == null) {
        const d = await apiPost('/api/drills', buildCreatePayload())
        id = d.id as number
        setCreatedId(id)
      }
      try {
        await startAiJob('interview_prep', id, `/api/drills/${id}/interview_prep`)
      } catch (e: unknown) {
        const msg = e instanceof Error ? e.message : String(e)
        setErr(msg)
      }
      await refreshPrep(id)
      setStep(2)
    } catch (e: unknown) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const prepRunning = !!prepDrill?.ai_jobs?.some((j) => j.kind === 'interview_prep')
  const prepNotes = prepDrill?.interview_state ?? null

  useEffect(() => {
    if (step !== 2 || createdId == null) return
    return onAiEvent((ev) => {
      if (ev.kind === 'interview_prep' && ev.target_id === createdId && ev.status !== 'running') {
        refreshPrep(createdId).catch(() => {})
      }
    })
  }, [step, createdId])

  useEffect(() => {
    if (step !== 2 || !prepRunning || createdId == null) return
    let cancelled = false
    ;(async () => {
      for (;;) {
        if (cancelled) return
        await new Promise((r) => setTimeout(r, 1500))
        if (cancelled) return
        try {
          const d = await refreshPrep(createdId)
          if (!d.ai_jobs?.some((j) => j.kind === 'interview_prep')) return
        } catch {
          /* 断线重试 */
        }
      }
    })()
    return () => {
      cancelled = true
    }
  }, [step, prepRunning, createdId])

  function enterSession() {
    if (createdId == null) return
    navigate(`/drills/${createdId}`)
  }

  async function goNext() {
    if (step === 0) {
      if (selectedPersonaId === '') {
        setErr('请选择面试官')
        return
      }
      setErr('')
      setStep(1)
      return
    }
    if (step === 1) {
      const app = apps.find((a) => a.id === appId)
      const jdReady = !!app?.jd_interpret && (app.jd_interpret.overall || (app.jd_interpret.cautions ?? []).length > 0)
      if (!parseDeferred && (!resumeParsed || !jdReady)) {
        setParseGateOpen(true)
        return
      }
      await createAndStartPrep()
      return
    }
    if (step === 2) {
      setErr('')
      setStep(3)
    }
  }

  function goBack() {
    if (step === 1 && lockedPersonaId != null) return
    if (step <= 0 || step >= 2) return
    setErr('')
    setStep(0)
  }

  const filteredQuestions = questions.filter((q) => {
    if (!qSearch.trim()) return true
    const term = qSearch.toLowerCase()
    return (
      q.content.toLowerCase().includes(term) ||
      (q.company && q.company.toLowerCase().includes(term)) ||
      (q.position && q.position.toLowerCase().includes(term)) ||
      (q.skill_name && q.skill_name.toLowerCase().includes(term)) ||
      (q.tags && q.tags.some((t) => t.toLowerCase().includes(term)))
    )
  })

  const selectedTags = Array.from(
    new Set(questions.filter((q) => selectedQids.includes(q.id)).flatMap((q) => q.tags || [])),
  )

  return (
    <div>
      <PageHeader title="新建陪练" meta={<span>选面试官 · 关联投递 · 备课开考</span>} />
      <nav aria-label="建场步骤" className="mb-6 grid grid-cols-4 gap-2">
        {STEPS.map((s, i) => (
          <div key={s} className="flex flex-col items-center gap-1.5">
            <div
              className={cn(
                'flex size-8 items-center justify-center rounded-full text-xs font-bold',
                i <= step ? 'bg-primary text-primary-foreground' : 'bg-muted text-muted-foreground',
              )}
            >
              {i + 1}
            </div>
            <span
              className={cn(
                'text-center text-[11px] font-medium leading-tight sm:text-xs',
                i <= step ? 'text-foreground' : 'text-muted-foreground',
              )}
            >
              {s}
            </span>
          </div>
        ))}
      </nav>
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {step === 0 && (
        <Section title="选择面试官" sub="选人即选侧重；经典面试官为均衡默认风格">
          <div className="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-4">
            {sortedPersonas.map((p) => {
              const selected = selectedPersonaId === p.id
              return (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => setSelectedPersonaId(p.id)}
                  className={cn(
                    'flex min-h-[120px] flex-col rounded-xl border p-3 text-left transition-all',
                    selected ? 'chip-selected' : 'border-border bg-card hover:border-primary/50',
                  )}
                >
                  <span className="text-sm font-semibold text-foreground">{p.name}</span>
                  {p.title && <span className="mt-0.5 text-xs text-muted-foreground">{p.title}</span>}
                  {p.difficulty_hint && <span className="mt-2 text-xs text-foreground">{p.difficulty_hint}</span>}
                  {p.focus_tags.length > 0 && (
                    <span className="mt-auto pt-2 font-mono text-[10px] text-muted-foreground">
                      {p.focus_tags.join(' / ')}
                    </span>
                  )}
                </button>
              )
            })}
          </div>
        </Section>
      )}

      {step === 1 && (
        <Section>
          <div className="space-y-3">
            {parseDeferred && (
              <div className="rounded-lg border border-warning/40 bg-warning/10 px-3 py-2.5 text-sm text-foreground" role="status">
                你选择了稍后解析。本次备课将按岗位名称通用出题，无法针对履历或 JD 要点深挖。
              </div>
            )}
            {lockedPersonaId != null && (
              <FormField label="面试官人格" htmlFor="dn-persona">
                <div
                  id="dn-persona"
                  className="flex h-9 items-center justify-between rounded-md border border-border bg-muted/50 px-3 text-sm text-foreground"
                >
                  <span>
                    {personas.find((p) => p.id === lockedPersonaId)?.name ?? `人格 #${lockedPersonaId}`}
                    {(() => {
                      const p = personas.find((x) => x.id === lockedPersonaId)
                      return p ? (
                        <span className="ml-2 font-mono text-[10px] text-muted-foreground">
                          {p.focus_tags.join(' / ')}
                          {p.temperature_hint != null && ` · 温度 ${p.temperature_hint}`}
                        </span>
                      ) : null
                    })()}
                  </span>
                  <span className="text-[10px] text-muted-foreground">已锁定</span>
                </div>
              </FormField>
            )}

            {appId === '' && (
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <FormField label="岗位" htmlFor="dn-pos">
                  <Input id="dn-pos" placeholder="如：后端工程师" value={position} onChange={(e) => setPosition(e.target.value)} />
                </FormField>
                <FormField label="方向" htmlFor="dn-dir">
                  <Input id="dn-dir" placeholder="如：Java / Go / 算法" value={direction} onChange={(e) => setDirection(e.target.value)} />
                </FormField>
              </div>
            )}

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <FormField label="目标题数" htmlFor="dn-target">
                <Input
                  id="dn-target"
                  type="number"
                  min={1}
                  max={20}
                  value={target}
                  onChange={(e) => setTarget(Number(e.target.value) || 5)}
                />
              </FormField>
              <FormField label="标题" htmlFor="dn-title" hint="可选，留空自动生成「场景 · 岗位 · 时间」">
                <Input id="dn-title" placeholder="如：美团一面复盘" value={title} onChange={(e) => setTitle(e.target.value)} />
              </FormField>
            </div>

            <FormField
              label="关联投递"
              htmlFor="dn-app"
              hint="默认关联最新进行中投递，出题以该岗 JD 为纲，并结合你的解析简历；选「不关联」才需填岗位方向"
            >
              <select
                id="dn-app"
                value={appId}
                onChange={(e) => setAppId(e.target.value === '' ? '' : Number(e.target.value))}
                className={selectCls}
              >
                <option value="">不关联</option>
                {apps.map((a) => (
                  <option key={a.id} value={a.id}>
                    {(a.company ?? '未关联公司') + (a.position ? ` · ${a.position}` : '')}
                  </option>
                ))}
              </select>
            </FormField>

            <div className="space-y-3 rounded-lg border border-border bg-card/60 p-3.5">
              <div className="flex items-center justify-between">
                <span className="text-sm font-semibold text-foreground">考官题本考点侧重</span>
                <span className="rounded bg-muted px-2 py-0.5 font-mono text-xs text-muted-foreground">
                  已选 <b className="font-bold text-primary">{selectedQids.length}</b> 道重点真题
                </span>
              </div>
              <FormField
                label="考点侧重与考官指令关键词"
                htmlFor="dn-dossier-summary"
                hint="结合你的解析简历与该岗 JD，指导面试官重点考核的方向"
              >
                <Input
                  id="dn-dossier-summary"
                  placeholder="例如：重点考核 Redis 缓存设计、分布式锁、并发…"
                  value={dossierSummary}
                  onChange={(e) => setDossierSummary(e.target.value)}
                />
              </FormField>
            </div>

            <div className="space-y-3 rounded-lg border border-border bg-card/60 p-3.5">
              <div className="flex items-center justify-between">
                <span className="text-sm font-semibold text-foreground">题库真题</span>
                {selectedQids.length > 0 && (
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-6 px-2 text-xs text-muted-foreground hover:text-destructive"
                    onClick={() => setSelectedQids([])}
                  >
                    清空已选 ({selectedQids.length})
                  </Button>
                )}
              </div>

              {dynamicTagCounts.length > 0 && (
                <div className="space-y-1.5">
                  <div className="text-[11px] font-medium text-muted-foreground">按题库高频知识标签快速选入：</div>
                  <div className="flex flex-wrap gap-1.5">
                    {dynamicTagCounts.map((tag: { name: string; count: number }) => {
                      const matched = questions.filter((q) => q.tags && q.tags.some((t) => t.toLowerCase() === tag.name.toLowerCase()))
                      const isAllSelected = matched.length > 0 && matched.every((q) => selectedQids.includes(q.id))
                      return (
                        <button
                          key={tag.name}
                          type="button"
                          onClick={() => toggleTag(tag.name)}
                          className={cn(
                            'flex items-center gap-1 rounded px-2 py-0.5 text-xs transition-colors',
                            isAllSelected ? 'chip-selected' : 'border border-border bg-muted/40 text-foreground hover:border-primary hover:bg-muted',
                          )}
                        >
                          <span>{tag.name}</span>
                          <span className="font-mono text-[10px] opacity-75">({tag.count})</span>
                        </button>
                      )
                    })}
                  </div>
                </div>
              )}

              {questions.length > 0 && (
                <div className="space-y-2 border-t border-border/60 pt-1">
                  <div className="flex min-w-[240px] flex-1 items-center gap-1.5">
                    <Input
                      placeholder="搜索题库题目或输入关键词…"
                      className="h-8 flex-1 text-xs"
                      value={qSearch}
                      onChange={(e) => setQSearch(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          e.preventDefault()
                          autoMatchQuestions()
                        }
                      }}
                    />
                    <Button type="button" variant="secondary" size="sm" className="h-8 shrink-0 text-xs" disabled={matching} onClick={autoMatchQuestions}>
                      {matching ? '匹配中…' : '智能匹配真题'}
                    </Button>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <Button
                      size="sm"
                      variant="outline"
                      className="h-7 px-2 text-xs"
                      onClick={() => {
                        const currentIds = filteredQuestions.map((q) => q.id)
                        setSelectedQids(Array.from(new Set([...selectedQids, ...currentIds])))
                      }}
                    >
                      全选结果 ({filteredQuestions.length})
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      className="h-7 px-2 text-xs"
                      onClick={() => setSelectedQids(filteredQuestions.slice(0, 5).map((q) => q.id))}
                    >
                      选前 5 题
                    </Button>
                  </div>

                  <div className="max-h-48 divide-y divide-border/60 overflow-y-auto rounded border border-border/80 text-xs">
                    {filteredQuestions.slice(0, 40).map((q) => {
                      const selected = selectedQids.includes(q.id)
                      return (
                        <div
                          key={q.id}
                          onClick={() => toggleQuestion(q.id)}
                          className={cn(
                            'flex cursor-pointer items-start gap-2 p-2 transition-colors',
                            selected ? 'chip-selected border-l-2 border-l-primary' : 'text-muted-foreground hover:bg-muted/50',
                          )}
                        >
                          <input type="checkbox" checked={selected} onChange={() => {}} className="pointer-events-none mt-0.5 rounded border-border" />
                          <div className="min-w-0 flex-1">
                            <div className="line-clamp-1 font-medium text-foreground">{q.content}</div>
                            <div className="mt-0.5 flex items-center gap-1.5 text-[11px] text-muted-foreground">
                              {q.company && <span>{q.company}</span>}
                              {q.tags && q.tags.map((t) => <span key={t}>#{t}</span>)}
                              {q.analyzed && <span className="text-success">· 含参考答案</span>}
                            </div>
                          </div>
                        </div>
                      )
                    })}
                  </div>

                  {selectedTags.length > 0 && (
                    <div className="flex flex-wrap items-center gap-1.5 pt-1 text-[11px] text-muted-foreground">
                      <span>已覆盖核心考点：</span>
                      {selectedTags.slice(0, 5).map((tag) => (
                        <span key={tag} className="rounded border border-border-strong bg-secondary px-1.5 py-0.5 font-medium text-heading">
                          #{tag}
                        </span>
                      ))}
                      {selectedTags.length > 5 && (
                        <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                          +{selectedTags.length - 5} 项细分标签
                        </span>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>

            <FormField
              label="补充参考内容（自由文本）"
              htmlFor="dn-refs"
              hint="可选：粘贴岗位 JD、杂乱面经笔记或项目资料，AI 将作为出题背景材料"
            >
              <Textarea
                id="dn-refs"
                rows={3}
                value={refs}
                onChange={(e) => setRefs(e.target.value)}
                placeholder="粘贴补充的 JD 文本 / 个人项目说明 / 杂乱面经…"
              />
            </FormField>
          </div>
        </Section>
      )}

      {step === 2 && (
        <Section title="智能备课" sub={prepNotes ? '考纲已生成' : '提取 JD 诉求 → 对齐候选人经历 → 制定追问策略'}>
          {parseDeferred && (
            <p className="mb-3 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-sm text-foreground" role="status">
              简历或 JD 尚未解析，本次使用通用备课。
            </p>
          )}
          {!prepNotes && (
            <ol className="mb-4 grid grid-cols-1 gap-2 sm:grid-cols-3">
              {['提取 JD 诉求', '对齐候选人经历', '制定追问策略'].map((label, i) => (
                <li
                  key={label}
                  className={cn(
                    'rounded-lg border px-3 py-2 text-sm font-medium',
                    prepRunning ? 'border-primary/40 bg-primary/5 text-foreground' : 'border-border text-muted-foreground',
                  )}
                >
                  <span className="mr-1.5 font-mono text-xs">{i + 1}</span>
                  {label}
                </li>
              ))}
            </ol>
          )}
          {prepNotes ? (
            <div className="grid gap-3 sm:grid-cols-2">
              {NOTE_SECTIONS.map(({ label, key }) => {
                const items = prepNotes[key] ?? []
                if (items.length === 0) return null
                return (
                  <div key={key} className="rounded-xl border border-border bg-card p-3.5 shadow-sm">
                    <p className="text-sm font-semibold text-foreground">{label}</p>
                    <ul className="mt-2 list-disc space-y-1 pl-4 text-sm leading-6">
                      {items.map((it, i) => (
                        <li key={i}>{it}</li>
                      ))}
                    </ul>
                  </div>
                )
              })}
            </div>
          ) : (
            <p className="text-sm text-foreground">{prepRunning ? '考官正在备课…' : '备课尚未完成，可等待或直接开考。'}</p>
          )}
        </Section>
      )}

      {step === 3 && (
        <section className="rounded-xl border border-border bg-card p-5 shadow-sm" aria-label="进入考场">
          <h2 className="text-sm font-semibold text-foreground">进入考场</h2>
          <div className="mt-4 grid gap-4 sm:grid-cols-2">
            <div className="rounded-lg border border-border p-4">
              <div className="text-lg font-semibold text-foreground">{selectedPersona?.name ?? '经典面试官'}</div>
              {selectedPersona?.title && <div className="mt-0.5 text-sm text-foreground">{selectedPersona.title}</div>}
              <div className="mt-3 flex flex-wrap gap-1.5">
                {selectedPersona?.difficulty_hint && (
                  <span className="rounded-full border border-border px-2 py-0.5 text-xs">{selectedPersona.difficulty_hint}</span>
                )}
                {selectedPersona?.temperature_hint != null && (
                  <span className="rounded-full border border-border px-2 py-0.5 font-mono text-xs">温度 {selectedPersona.temperature_hint}</span>
                )}
              </div>
            </div>
            <div className="rounded-lg border border-border p-4 text-sm">
              <div className="font-medium text-foreground">
                {selectedApp ? `${selectedApp.company ?? '未关联公司'}${selectedApp.position ? ` · ${selectedApp.position}` : ''}` : '未关联投递'}
              </div>
              <div className="mt-2 tabular-nums text-foreground">目标 {target} 题 · 已选 {selectedQids.length} 道真题</div>
              {dossierSummary.trim() && <div className="mt-2 text-foreground">{dossierSummary}</div>}
              <div className="mt-2">{prepNotes ? '考纲笔记已就绪' : '笔记未完成，开考后仍可生成'}</div>
            </div>
          </div>
        </section>
      )}

      <div className="mt-5 flex flex-wrap items-center justify-between gap-2">
        <div>
          {step === 1 && lockedPersonaId == null && (
            <Button variant="ghost" onClick={goBack} disabled={busy}>
              上一步
            </Button>
          )}
          {step === 2 && (
            <Button variant="ghost" onClick={enterSession} disabled={createdId == null}>
              跳过等待，直接开考
            </Button>
          )}
        </div>
        {step < 3 ? (
          <Button className="h-11 px-8 text-base font-semibold" onClick={goNext} disabled={busy || (step === 0 && selectedPersonaId === '')}>
            {busy ? '创建中…' : '下一步'}
          </Button>
        ) : (
          <Button className="h-11 px-8 text-base font-semibold" onClick={enterSession} disabled={createdId == null}>
            立即开考 →
          </Button>
        )}
      </div>

      <Dialog open={parseGateOpen} onOpenChange={setParseGateOpen}>
        <DialogContent className="max-w-md">
          <DialogHeader>
            <DialogTitle>建议先完成解析</DialogTitle>
            <DialogDescription>
              备课使用已解析的简历要点和 JD 解读。当前
              {!resumeParsed ? ' 简历尚未 AI 解析' : ''}
              {!resumeParsed && !apps.find((a) => a.id === appId)?.jd_interpret ? '，且' : ''}
              {!apps.find((a) => a.id === appId)?.jd_interpret ? ' 该岗尚未 JD 解读' : ''}
              。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="flex flex-wrap gap-2 sm:justify-end">
            {!resumeParsed && (
              <Button asChild variant="outline">
                <Link to="/resume">去解析简历</Link>
              </Button>
            )}
            {!!apps.find((a) => a.id === appId)?.position_id && !apps.find((a) => a.id === appId)?.jd_interpret && (
              <Button asChild variant="outline">
                <Link to={`/positions/${apps.find((a) => a.id === appId)?.position_id}`}>去解读 JD</Link>
              </Button>
            )}
            <Button
              variant="ghost"
              onClick={() => {
                setParseDeferred(true)
                setParseGateOpen(false)
                createAndStartPrep()
              }}
            >
              稍后继续
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
