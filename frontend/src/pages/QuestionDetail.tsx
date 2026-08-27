import { useEffect, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import {
  CaretLeft,
  CaretRight,
  PencilLine,
  Plus,
  Sparkle,
  Star,
  Trash,
  TreeStructure,
  X,
} from '@phosphor-icons/react'
import { apiDelete, apiGet, apiPatch, apiPost, apiPut } from '../api/client'
import { isRunning, onJobDone, startAiJob, trackRunning, useAiJobs } from '../ai/jobs'
import type { CommentRow, QuestionDetail, RoundLinkRow, SkillRow, SkillTreeNode } from '../api/types'
import Markdown from '../components/Markdown'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { SemBadge } from '../components/SemBadge'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'
import { cn } from '@/lib/utils'

import { ASSESSMENT_DIMENSION_LABELS, type AssessmentDimension } from '../api/types'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

function diffLabel(d: number | null): string {
  const m: Record<number, string> = { 1: '入门', 2: '简单', 3: '中等', 4: '较难', 5: '极难' }
  return d == null ? '—' : `${d} · ${m[d] ?? ''}`
}

/** 综合分 → 语义（量纲唯一：0–100；<60 红 / <80 琥珀 / 其余绿） */
function scoreCls(score: number | null): string {
  if (score == null) return 'text-muted-foreground'
  if (score < 60) return 'text-destructive'
  if (score < 80) return 'text-warning'
  return 'text-success'
}

export default function QuestionDetail() {
  const { id } = useParams()
  const [data, setData] = useState<QuestionDetail | null>(null)
  const [err, setErr] = useState('')
  const [comment, setComment] = useState('')
  const [newTag, setNewTag] = useState('')
  const [editMode, setEditMode] = useState(false)
  const [content, setContent] = useState('')
  const [editQuestionType, setEditQuestionType] = useState('')
  const [newAnswer, setNewAnswer] = useState('') // 「添加你的回复」草稿（x.com 回帖式新增回答入口）
  const [related, setRelated] = useState<{ id: number; content: string; last_score: number | null }[]>([])
  const [llmConfigured, setLlmConfigured] = useState(false)
  const [allRounds, setAllRounds] = useState<
    { round_id: number; round_name: string; session_id: number; company: string | null; department: string | null; position: string | null }[]
  >([])
  const [pickRound, setPickRound] = useState('')
  const [verIdx, setVerIdx] = useState(0)
  const [showRef, setShowRef] = useState(false)
  // C组 #6：AI 任务状态由全局中心提供（跨页/刷新不丢回显）
  const aiJobs = useAiJobs()
  const refBusy = isRunning(aiJobs, 'ref', Number(id))
  const analyzing = isRunning(aiJobs, 'analyze', Number(id))
  const [notice, setNotice] = useState('')
  const [refEditing, setRefEditing] = useState(false) // 手动编辑参考答案（LLM 不佳时兜底）
  const [refDraft, setRefDraft] = useState('')
  const [showTagInput, setShowTagInput] = useState(false)
  const [delOpen, setDelOpen] = useState(false)
  const [skills, setSkills] = useState<SkillRow[]>([])
  const [allSkillsTree, setAllSkillsTree] = useState<SkillTreeNode[]>([])
  const [skillModalOpen, setSkillModalOpen] = useState(false)
  const [selectedSkillIds, setSelectedSkillIds] = useState<number[]>([])

  const [followupContent, setFollowupContent] = useState('')
  const [followupAnswer, setFollowupAnswer] = useState('')
  const [addingFollowup, setAddingFollowup] = useState(false)
  const [submittingFollowup, setSubmittingFollowup] = useState(false)

  async function addFollowup() {
    if (!followupContent.trim()) return
    setSubmittingFollowup(true)
    try {
      await apiPost(`/api/questions/${id}/followups`, {
        content: followupContent.trim(),
        my_answer: followupAnswer.trim() || undefined,
      })
      setFollowupContent('')
      setFollowupAnswer('')
      setAddingFollowup(false)
      toast.success('已追加现场追问')
      await load()
    } catch (e: any) {
      toast.error(e.message || '追加追问失败')
    } finally {
      setSubmittingFollowup(false)
    }
  }

  async function load() {
    const d = await apiGet(`/api/questions/${id}`)
    setData(d)
    setContent(d.content)
    setEditQuestionType(d.question_type || '')
    setNewAnswer('')
    setVerIdx(0)
    trackRunning(d.ai_jobs) // 刷新/重进时恢复「进行中」跟踪
    try {
      const s = await apiGet('/api/settings/llm-config')
      setLlmConfigured(!!s.resolved)
    } catch {
      setLlmConfigured(false)
    }
  }

  async function loadSkills() {
    try {
      const sk = await apiGet(`/api/questions/${id}/skills`)
      setSkills(sk || [])
    } catch {
      setSkills([])
    }
  }

  async function openSkillModal() {
    try {
      const treeRes = await apiGet('/api/skills/tree')
      setAllSkillsTree(treeRes.tree || [])
      setSelectedSkillIds(skills.map((s) => s.id))
      setSkillModalOpen(true)
    } catch (e: any) {
      setErr(e.message || '加载技能列表失败')
    }
  }

  async function saveSkills() {
    try {
      await apiPost(`/api/questions/${id}/skills`, { skill_ids: selectedSkillIds })
      await loadSkills()
      await load()
      setSkillModalOpen(false)
    } catch (e: any) {
      setErr(e.message || '保存技能关联失败')
    }
  }

  useEffect(() => {
    load().catch((e) => setErr(e.message))
    loadSkills()
    apiGet('/api/rounds/all')
      .then(setAllRounds)
      .catch(() => {})
    apiGet(`/api/questions/${id}/related`)
      .then(setRelated)
      .catch(() => {})
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  // 任务完成回调：reload 数据展示结果（无论从哪个页面回来、刷新与否）
  useEffect(() => {
    const qid = Number(id)
    const offRef = onJobDone('ref', qid, (ok) => {
      if (!ok) setErr('题目分析失败，请重试')
      else setNotice('题目分析完成')
      load().catch(() => {})
    })
    const offAnalyze = onJobDone('analyze', qid, (ok) => {
      if (!ok) setErr('回答评价失败，请重试')
      else setNotice('回答评价完成')
      load().catch(() => {})
    })
    return () => {
      offRef()
      offAnalyze()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  async function analyze() {
    setErr('')
    try {
      await startAiJob('analyze', Number(id), `/api/questions/${id}/analyze`)
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function toggleStar() {
    if (!data) return
    await apiPatch(`/api/questions/${id}`, { starred: !data.starred })
    await load()
  }

  async function addComment() {
    if (!comment.trim()) return
    await apiPost(`/api/questions/${id}/comments`, { body: comment.trim() })
    setComment('')
    await load()
  }

  async function delComment(c: CommentRow) {
    await apiDelete(`/api/comments/${c.id}`)
    await load()
  }

  async function addTag() {
    if (!newTag.trim()) return
    const tags = [...(data?.tags ?? []), newTag.trim()]
    await apiPatch(`/api/questions/${id}`, { tags })
    setNewTag('')
    await load()
  }

  async function delTag(t: string) {
    const tags = (data?.tags ?? []).filter((x) => x !== t)
    await apiPatch(`/api/questions/${id}`, { tags })
    await load()
  }

  async function addAnswer() {
    // 新增回答：PATCH my_answer 即记历史 + 入复习队，成为当前回答；不做"编辑当前回答"
    const a = newAnswer.trim()
    if (!a) return
    await apiPatch(`/api/questions/${id}`, { my_answer: a })
    setNewAnswer('')
    await load()
  }

  async function addRoundLink() {
    if (!pickRound) return
    await apiPost(`/api/questions/${id}/round-links`, { round_id: Number(pickRound) })
    setPickRound('')
    await load()
  }

  async function removeRoundLink(rid: number) {
    await apiDelete(`/api/questions/${id}/round-links/${rid}`)
    await load()
  }

  async function analyzeIntrinsic() {
    // 一次性分析题目固有属性（标签 + 难度 + 参考答案），与你的回答无关；手动触发（基准 3）
    setErr('')
    try {
      await startAiJob('ref', Number(id), `/api/questions/${id}/ref`)
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function saveRef() {
    // 手动编辑参考答案（LLM 不佳/不详细时兜底）：就地改最近一条固有属性行，难度/标签不动
    const r = refDraft.trim()
    if (!r) return
    await apiPut(`/api/questions/${id}/ref`, { ref_answer: r })
    setRefEditing(false)
    await load()
  }

  async function saveEdit() {
    await apiPatch(`/api/questions/${id}`, {
      content: content.trim() || undefined,
      question_type: editQuestionType || null,
    })
    setEditMode(false)
    await load()
  }

  async function delQuestion() {
    await apiDelete(`/api/questions/${id}`)
    window.location.href = '/questions'
  }

  if (!data) {
    return <div className="py-24 text-center text-muted-foreground">{err || '加载中…'}</div>
  }
  // 题目固有属性（标签/难度/参考答案）：取最近一条 difficulty/ref_answer 非空的分析，与回答评价解耦
  const intrinsic = data.analyses.find((a) => a.difficulty != null || (a.ref_answer && a.ref_answer.trim()))

  // 回答版本序列（线性切换）：第 0 版 = 当前回答（主展示），其后为历史版本（去重内容）
  const curAns = data.my_answer ?? ''
  const seen = new Set<string>()
  const versions: { key: string; content: string; isCurrent: boolean; source?: string; ts?: string }[] = [
    { key: 'current', content: curAns, isCurrent: true },
  ]
  seen.add(curAns.trim())
  for (const a of data.answers) {
    const c = a.content.trim()
    if (c && !seen.has(c)) {
      seen.add(c)
      versions.push({ key: `h${a.id}`, content: a.content, isCurrent: false, source: a.source, ts: a.created_at })
    }
  }
  // 该回答版本对应的批注（评分 + 点评）：优先按 answer_snapshot 精确匹配最近一次分析，无精准匹配则回退到该题最近一条含评分/点评的分析
  const annotationFor = (content: string) => {
    if (!content.trim()) return undefined
    const exact = data.analyses.find((a) => a.answer_snapshot && a.answer_snapshot.trim() === content.trim())
    if (exact) return exact
    return data.analyses.find((a) => a.score != null || (a.feedback && a.feedback.trim().length > 0))
  }
  const ver = versions[Math.min(verIdx, versions.length - 1)]
  const annot = ver ? annotationFor(ver.content) : undefined
  const hasAnswer = curAns.trim().length > 0

  return (
    <div>
      <nav aria-label="面包屑" className="mb-2 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link to="/questions" className="hover:text-primary">
          题目
        </Link>
        <span aria-hidden>/</span>
        <span className="text-foreground">#{id}</span>
      </nav>

      <PageHeader
        title={
          <div className="flex items-center gap-2">
            <span>题目 · #{id}</span>
            {data.question_type && ASSESSMENT_DIMENSION_LABELS[data.question_type as AssessmentDimension] ? (
              <SemBadge sem="info">
                {ASSESSMENT_DIMENSION_LABELS[data.question_type as AssessmentDimension]}
              </SemBadge>
            ) : (
              <span className="rounded bg-muted/60 px-2 py-0.5 text-xs text-muted-foreground">
                未分类
              </span>
            )}
            {/* 票02：疑似重复双向徽章——点开可跳到对方题目 */}
            {(data.duplicates?.length ?? 0) > 0 && (
              <span
                className="inline-flex items-center gap-1 rounded-full border border-warning/40 bg-warning/10 px-2 py-0.5 text-xs font-medium text-foreground"
                title="内容归一化后与已有题目相同"
              >
                疑似重复
                {data.duplicates!.map((d) => (
                  <Link
                    key={d.id}
                    to={`/questions/${d.id}`}
                    className="rounded bg-warning/20 px-1 hover:bg-warning/30"
                  >
                    #{d.id}
                  </Link>
                ))}
              </span>
            )}
          </div>
        }
        meta={
          <>
            <span>录入于 {new Date(data.created_at).toLocaleString()}</span>
            {data.asked_at && <span>面试日期 {data.asked_at}</span>}
          </>
        }
        actions={
          <>
            <Button
              size="sm"
              variant="ghost"
              onClick={analyzeIntrinsic}
              disabled={refBusy || !llmConfigured}
              title={
                intrinsic
                  ? '重新分析题目固有属性（标签 + 难度 + 参考答案）'
                  : '一次性分析题目固有属性：标签 + 难度 + 参考答案（与你的回答无关）'
              }
            >
              <Sparkle className="size-4" aria-hidden />
              {refBusy ? '分析中…' : intrinsic ? '重新分析' : '分析题目'}
            </Button>
            <Button
              size="sm"
              variant={data.starred ? 'secondary' : 'ghost'}
              onClick={toggleStar}
            >
              <Star className="size-4" weight={data.starred ? 'fill' : 'regular'} aria-hidden />
              {data.starred ? '已收藏' : '收藏'}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setEditMode(!editMode)}>
              <PencilLine className="size-4" aria-hidden /> 编辑
            </Button>
            <Button size="sm" variant="ghost" className="text-destructive hover:bg-destructive/10" onClick={() => setDelOpen(true)}>
              <Trash className="size-4" aria-hidden /> 删除
            </Button>
          </>
        }
      />

      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}
      {(notice || refBusy || analyzing) && (
        <div
          className="mb-3 flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-sm"
          role="status"
        >
          <span className="min-w-0 flex-1">
            {refBusy
              ? 'AI 分析中…完成后自动回显。'
              : analyzing
                ? '评价回答中…完成后自动回显。'
                : notice}
          </span>
          {notice && (
            <Button size="sm" variant="ghost" onClick={() => setNotice('')}>
              知道了
            </Button>
          )}
        </div>
      )}

      <div className="grid gap-4 lg:grid-cols-3">
        <main className="space-y-4 lg:col-span-2">
          {/* 题目卡片（帖子） */}
          <Section>
            {editMode ? (
              <div className="space-y-3">
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="ed-content" className="text-sm font-medium">
                    题目内容
                  </label>
                  <Textarea
                    id="ed-content"
                    rows={4}
                    value={content}
                    onChange={(e) => setContent(e.target.value)}
                    placeholder="尽量还原面试官的原话…"
                  />
                </div>
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="ed-qtype" className="text-sm font-medium">
                    考察维度
                  </label>
                  <select
                    id="ed-qtype"
                    className="h-9 w-full rounded-md border border-input bg-card px-2 text-sm text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                    value={editQuestionType}
                    onChange={(e) => setEditQuestionType(e.target.value)}
                  >
                    <option value="">未分类（默认）</option>
                    {(Object.keys(ASSESSMENT_DIMENSION_LABELS) as AssessmentDimension[]).map((dim) => (
                      <option key={dim} value={dim}>{ASSESSMENT_DIMENSION_LABELS[dim]}</option>
                    ))}
                  </select>
                </div>
                <div className="flex items-center gap-2">
                  <Button onClick={saveEdit}>保存修改</Button>
                  <Button variant="ghost" onClick={() => setEditMode(false)}>
                    取消
                  </Button>
                </div>
              </div>
            ) : (
              <>
                <h2 className="text-base font-semibold leading-7">{data.content}</h2>
                <div className="mt-2 flex flex-wrap items-center gap-1.5">
                  {intrinsic?.difficulty != null && (
                    <span className="rounded-full bg-muted px-2 py-0.5 text-xs" title="题目难度（固有属性，与回答评价无关）">
                      难度 {diffLabel(intrinsic.difficulty)}
                    </span>
                  )}
                  {data.tags.map((t) => (
                    <span key={t} className="inline-flex items-center gap-0.5 rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                      #{t}
                      <button className="grid size-3 place-items-center rounded-full hover:text-destructive" onClick={() => delTag(t)} aria-label={`删除标签 ${t}`}>
                        <X className="size-3" aria-hidden />
                      </button>
                    </span>
                  ))}
                  {showTagInput ? (
                    <Input
                      className="h-6 w-32 px-1.5 text-xs"
                      autoFocus
                      placeholder="标签，回车添加"
                      value={newTag}
                      onChange={(e) => setNewTag(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          addTag()
                          setShowTagInput(false)
                        } else if (e.key === 'Escape') {
                          setShowTagInput(false)
                          setNewTag('')
                        }
                      }}
                      onBlur={() => {
                        setShowTagInput(false)
                        setNewTag('')
                      }}
                      aria-label="添加标签"
                    />
                  ) : (
                    <button
                      className="grid size-5 place-items-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                      onClick={() => {
                        setNewTag('')
                        setShowTagInput(true)
                      }}
                      aria-label="添加标签"
                      title="添加标签"
                    >
                      <Plus className="size-3.5" aria-hidden />
                    </button>
                  )}
                </div>

                {/* v5 技能图谱挂靠 */}
                <div className="mt-3 flex flex-wrap items-center gap-1.5 border-t border-border/40 pt-2.5">
                  <span className="text-xs text-muted-foreground">挂靠技能:</span>
                  {skills.length === 0 ? (
                    <span className="text-xs text-muted-foreground">未关联任何技能节点</span>
                  ) : (
                    skills.map((s) => (
                      <span
                        key={s.id}
                        className="inline-flex items-center gap-1 rounded bg-secondary border border-border px-2 py-0.5 font-mono text-xs font-medium text-heading"
                        title={s.path}
                      >
                        <TreeStructure className="size-3 text-muted-foreground" />
                        {s.name}
                      </span>
                    ))
                  )}
                  <button
                    onClick={openSkillModal}
                    className="ml-1 inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
                    title="管理该题挂靠的技能知识点"
                  >
                    <Plus className="size-3" />
                    <span>设置技能</span>
                  </button>
                </div>
              </>
            )}
          </Section>

          {/* 回答与批注：回答 = 回复，批注 = 回复的回复 */}
          <Section
            title="回答与批注"
            action={
              <div className="flex items-center gap-1">
                <Button
                  size="icon"
                  variant="ghost"
                  className="size-7"
                  onClick={() => setVerIdx(Math.max(0, verIdx - 1))}
                  disabled={verIdx === 0}
                  aria-label="上一个回答"
                  title="上一个回答"
                >
                  <CaretLeft className="size-4" aria-hidden />
                </Button>
                <span className="font-mono text-xs tabular-nums text-muted-foreground">
                  {verIdx + 1} / {versions.length}
                </span>
                <Button
                  size="icon"
                  variant="ghost"
                  className="size-7"
                  onClick={() => setVerIdx(Math.min(versions.length - 1, verIdx + 1))}
                  disabled={verIdx >= versions.length - 1}
                  aria-label="下一个回答"
                  title="下一个回答"
                >
                  <CaretRight className="size-4" aria-hidden />
                </Button>
              </div>
            }
          >
            <div className="flex gap-3">
              <span className="grid size-8 shrink-0 place-items-center rounded-full bg-secondary text-xs font-bold text-secondary-foreground">
                我
              </span>
              <div className="min-w-0 flex-1">
                <div className="text-xs text-muted-foreground">
                  {ver.isCurrent
                    ? '当前回答'
                    : `历史回答 · ${ver.ts ? new Date(ver.ts).toLocaleString() : ''}`}
                </div>
                <div className="mt-1 text-sm leading-7">
                  {ver.content ? <Markdown text={ver.content} /> : <span className="text-muted-foreground">还没有回答。</span>}
                </div>
                {/* 评价回答：放在回答旁 */}
                {ver.isCurrent && (
                  <div className="mt-1.5">
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-7 px-2 text-xs"
                      onClick={analyze}
                      disabled={analyzing || !hasAnswer || !llmConfigured}
                      title={
                        !hasAnswer
                          ? '先在下方「添加你的回复」写下回答'
                          : !llmConfigured
                            ? '请先在设置页配置 LLM'
                            : annot
                              ? '重新评价当前回答'
                              : '评价当前回答'
                      }
                    >
                      <Sparkle className="size-3.5" aria-hidden />
                      {analyzing ? '评价中…' : annot ? '重新评价' : '评价回答'}
                    </Button>
                    {!intrinsic?.ref_answer && hasAnswer && !analyzing && (
                      <p className="mt-0.5 text-[11px] text-muted-foreground">
                        未生成参考答案——将基于题面独立评审本回答。
                      </p>
                    )}
                  </div>
                )}
                {/* 回复的回复：批注（评分 + 点评） */}
                <div className="mt-2 rounded-md bg-muted/50 p-2.5">
                  {annot ? (
                    <>
                      <div className="flex flex-wrap items-baseline gap-2">
                        <span className={`font-mono text-lg font-bold tabular-nums ${scoreCls(annot.score)}`}>
                          {annot.score ?? '—'}
                        </span>
                        <span className="ml-auto font-mono text-xs text-muted-foreground">
                          {annot.provider} / {annot.model} · {new Date(annot.created_at).toLocaleString()}
                        </span>
                      </div>
                      <div className="mt-1.5 text-sm leading-7">
                        <Markdown text={annot.feedback ?? ''} />
                      </div>
                    </>
                  ) : (
                    <p className="text-sm text-muted-foreground">
                      {ver.content.trim()
                        ? '该回答尚未评价——点「评价回答」生成批注。'
                        : '在下方「添加你的回复」写下回答后，点「评价回答」即可得到评分与点评。'}
                    </p>
                  )}
                </div>
              </div>
            </div>

            {/* x.com「添加你的回复」：新增回答入口 */}
            <div className="mt-3">
              <Textarea
                rows={3}
                value={newAnswer}
                onChange={(e) => setNewAnswer(e.target.value)}
                placeholder="添加你的回复…（记入历史并成为当前回答，可随即评价）"
              />
              <div className="mt-1.5 flex justify-end">
                <Button size="sm" onClick={addAnswer} disabled={!newAnswer.trim()}>
                  回复
                </Button>
              </div>
            </div>
          </Section>

          {/* 连续追问（推特式一级追问，不计入独立题数） */}
          <Section
            title="现场连续追问"
            sub={<span className="font-mono text-xs">{data.followups?.length ?? 0} 轮</span>}
          >
            <div className="space-y-3">
              {(!data.followups || data.followups.length === 0) && !addingFollowup && (
                <p className="text-xs text-muted-foreground">该题暂无现场追问记录。若面试官在此题后继续深挖，可追加记录。</p>
              )}

              {data.followups && data.followups.length > 0 && (
                <div className="space-y-2.5">
                  {data.followups.map((f, idx) => (
                    <div key={f.id} className="rounded-lg border border-border/80 bg-muted/30 p-3 space-y-1.5">
                      <div className="flex items-center justify-between text-xs">
                        <span className="font-semibold text-primary">💬 追问 #{idx + 1}</span>
                        {f.last_score != null && (
                          <span className="rounded bg-secondary border border-border-strong px-1.5 py-0.5 font-mono text-[11px] font-semibold text-heading">
                            得分 {f.last_score}
                          </span>
                        )}
                      </div>
                      <p className="text-sm font-medium text-foreground">{f.content}</p>
                      {f.my_answer ? (
                        <div className="rounded bg-card p-2 text-xs text-muted-foreground border border-border/60">
                          <span className="font-semibold text-foreground mr-1">回答：</span>
                          {f.my_answer}
                        </div>
                      ) : (
                        <p className="text-[11px] text-muted-foreground italic">未记录现场回答</p>
                      )}
                      {/* 追问评价（随主题目首次分析捆绑生成，用户裁决 2a） */}
                      {f.last_feedback && (
                        <div className="rounded border border-primary/20 bg-primary/5 p-2 text-xs">
                          <span className="font-semibold text-primary mr-1">AI 点评：</span>
                          <span className="text-foreground">{f.last_feedback}</span>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}

              {addingFollowup ? (
                <div className="rounded-lg border border-border bg-card/60 p-3 space-y-2">
                  <span className="text-xs font-semibold">追加现场追问</span>
                  <Input
                    placeholder="面试官追问题干，例如：那在并发冲突时如何保证幂等？"
                    value={followupContent}
                    onChange={(e) => setFollowupContent(e.target.value)}
                    className="text-xs h-8"
                  />
                  <Textarea
                    rows={2}
                    placeholder="你的回答（可选）"
                    value={followupAnswer}
                    onChange={(e) => setFollowupAnswer(e.target.value)}
                    className="text-xs"
                  />
                  <div className="flex items-center gap-2 pt-1">
                    <Button size="sm" onClick={addFollowup} disabled={submittingFollowup || !followupContent.trim()}>
                      {submittingFollowup ? '保存中…' : '保存追问'}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => setAddingFollowup(false)}>
                      取消
                    </Button>
                  </div>
                </div>
              ) : (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-7 text-xs"
                  onClick={() => setAddingFollowup(true)}
                >
                  <Plus className="mr-1 h-3.5 w-3.5" />
                  追加现场追问
                </Button>
              )}
            </div>
          </Section>

          {/* 评论 */}
          <Section title="评论">
            <div className="flex gap-2">
              <Input
                placeholder="写下你的补充与思考…"
                value={comment}
                onChange={(e) => setComment(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && addComment()}
              />
              <Button onClick={addComment} disabled={!comment.trim()}>
                评论
              </Button>
            </div>
            <ul className="mt-3 divide-y divide-border">
              {data.comments.length === 0 && (
                <li className="py-2 text-sm text-muted-foreground">暂无评论</li>
              )}
              {data.comments.map((c) => (
                <li key={c.id} className="flex items-start gap-2 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="font-mono text-xs text-muted-foreground">
                      {new Date(c.created_at).toLocaleString()}
                    </div>
                    <div className="text-sm leading-6">{c.body}</div>
                  </div>
                  <Button
                    size="icon"
                    variant="ghost"
                    className="size-7 shrink-0 text-muted-foreground hover:text-destructive"
                    onClick={() => delComment(c)}
                    aria-label="删除评论"
                  >
                    <Trash className="size-4" aria-hidden />
                  </Button>
                </li>
              ))}
            </ul>
          </Section>
        </main>

        <aside className="space-y-4">
          {/* 参考答案 */}
          <Section
            title="参考答案"
            sub={<span className="font-mono">{intrinsic?.ref_answer ? '已生成' : '未生成'}</span>}
          >
            {intrinsic?.ref_answer ? (
              <>
                <p className="mb-2 text-sm text-muted-foreground">
                  {intrinsic.ref_answer.length > 90 ? intrinsic.ref_answer.slice(0, 90) + '…' : intrinsic.ref_answer}
                </p>
                <Button size="sm" onClick={() => setShowRef(true)}>
                  查看 / 编辑
                </Button>
              </>
            ) : (
              <div className="text-sm">
                <p className="text-muted-foreground">还没有参考答案——可一键生成标签、难度与参考答案。</p>
                <div className="mt-2">
                  <Button size="sm" variant="secondary" onClick={analyzeIntrinsic} disabled={refBusy || !llmConfigured}>
                    <Sparkle className="size-4" aria-hidden />
                    {refBusy ? '分析中…' : '生成参考答案'}
                  </Button>
                  {!llmConfigured && <p className="mt-1.5 text-xs text-muted-foreground">需先在设置页配置 LLM</p>}
                </div>
              </div>
            )}
          </Section>

          {/* 关联面试 */}
          <Section
            title="关联面试"
            sub={<span className="font-mono">{data.round_links.length} 场</span>}
          >
            {data.round_links.length === 0 ? (
              <p className="text-sm text-muted-foreground">尚未关联其它面试。</p>
            ) : (
              <div className="flex flex-wrap gap-1.5">
                {data.round_links.map((l: RoundLinkRow) => (
                  <Link
                    key={l.round_id}
                    to={`/rounds/${l.round_id}`}
                    className="inline-flex items-center gap-0.5 rounded bg-muted px-1.5 py-0.5 text-xs hover:bg-muted/80"
                    title={`${l.company || '未归属'} · ${l.round_name}${l.passed === 'pass' ? '（通过）' : l.passed === 'fail' ? '（未通过）' : ''}`}
                  >
                    {l.company || '未归属'} · {l.round_name}
                    <button
                      className="grid size-3 place-items-center rounded-full hover:text-destructive"
                      onClick={(e) => {
                        e.preventDefault()
                        removeRoundLink(l.round_id)
                      }}
                      aria-label="解除关联"
                    >
                      <X className="size-3" aria-hidden />
                    </button>
                  </Link>
                ))}
              </div>
            )}
            <div className="mt-2.5 flex items-center gap-2">
              <select
                className="h-9 min-w-0 flex-1 rounded-md border border-input bg-card px-2 text-sm"
                value={pickRound}
                onChange={(e) => setPickRound(e.target.value)}
                aria-label="关联到面试"
              >
                <option value="">关联到另一个面试…</option>
                {allRounds
                  .filter((r) => !data.round_links.some((l) => l.round_id === r.round_id) && r.round_id !== data.round_id)
                  .map((r) => (
                    <option key={r.round_id} value={r.round_id}>
                      {r.company} · {r.department || r.position || `#${r.session_id}`} · {r.round_name}
                    </option>
                  ))}
              </select>
              <Button onClick={addRoundLink} disabled={!pickRound}>
                关联
              </Button>
            </div>
          </Section>

          {/* 推荐关联题目（离线）：相关标签 ∩ 评分最低 5 条 */}
          <Section
            title="推荐关联题目"
            sub={<span className="font-mono">{related.length} 条</span>}
          >
            {related.length === 0 ? (
              <p className="text-sm text-muted-foreground">暂无同标签题目。</p>
            ) : (
              <ul className="divide-y divide-border">
                {related.map((r) => (
                  <li key={r.id}>
                    <Link to={`/questions/${r.id}`} className="flex items-center gap-2 py-1.5 hover:text-primary">
                      <span className="min-w-0 flex-1 truncate text-sm">
                        {r.content.length > 40 ? r.content.slice(0, 40) + '…' : r.content}
                      </span>
                      <span className={`font-mono text-sm font-semibold tabular-nums ${scoreCls(r.last_score)}`}>
                        {r.last_score ?? '—'}
                      </span>
                    </Link>
                  </li>
                ))}
              </ul>
            )}
          </Section>
        </aside>
      </div>

      {/* 删除确认 */}
      <ConfirmDialog
        open={delOpen}
        onOpenChange={setDelOpen}
        destructive
        title="删除该题目？"
        description="将级联删除其分析。"
        confirmLabel="删除"
        onConfirm={delQuestion}
      />

      {/* 参考答案浮窗：查看 + 手动编辑 + 重新分析 */}
      <Dialog open={showRef} onOpenChange={setShowRef}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              <span className="inline-flex items-center gap-1.5">
                <Sparkle className="size-4" aria-hidden /> 参考答案
              </span>
            </DialogTitle>
          </DialogHeader>
          {intrinsic?.ref_answer ? (
            refEditing ? (
              <>
                <Textarea
                  rows={8}
                  value={refDraft}
                  onChange={(e) => setRefDraft(e.target.value)}
                  placeholder="编辑/补充参考答案…"
                />
                <DialogFooter className="gap-2">
                  <Button
                    variant="ghost"
                    onClick={() => {
                      setRefEditing(false)
                      setRefDraft(intrinsic.ref_answer ?? '')
                    }}
                  >
                    取消
                  </Button>
                  <Button onClick={saveRef} disabled={!refDraft.trim()}>
                    保存参考答案
                  </Button>
                </DialogFooter>
              </>
            ) : (
              <>
                <div className="max-h-[50vh] overflow-y-auto text-sm leading-7">
                  <Markdown text={intrinsic.ref_answer} />
                </div>
                <DialogFooter className="gap-2">
                  <Button
                    variant="ghost"
                    onClick={() => {
                      setRefDraft(intrinsic.ref_answer ?? '')
                      setRefEditing(true)
                    }}
                  >
                    <PencilLine className="size-4" aria-hidden /> 编辑参考答案
                  </Button>
                  <Button variant="ghost" onClick={analyzeIntrinsic} disabled={refBusy}>
                    <Sparkle className="size-4" aria-hidden />
                    {refBusy ? '分析中…' : '重新分析'}
                  </Button>
                </DialogFooter>
              </>
            )
          ) : (
            <div className="py-4 text-center">
              <p className="mb-2 text-sm text-muted-foreground">还没有参考答案</p>
              <Button size="sm" variant="secondary" onClick={analyzeIntrinsic} disabled={refBusy || !llmConfigured}>
                <Sparkle className="size-4" aria-hidden />
                {refBusy ? '分析中…' : '生成参考答案'}
              </Button>
              {!llmConfigured && <p className="mt-1.5 text-xs text-muted-foreground">需先在设置页配置 LLM</p>}
            </div>
          )}
        </DialogContent>
      </Dialog>

      {/* 技能挂靠管理弹窗 */}
      <Dialog open={skillModalOpen} onOpenChange={setSkillModalOpen}>
        <DialogContent className="max-w-lg">
          <DialogHeader>
            <DialogTitle>设置题目挂靠技能</DialogTitle>
          </DialogHeader>
          <div className="max-h-[60vh] space-y-3 overflow-y-auto pr-1">
            <p className="text-xs text-muted-foreground">勾选该题目所属的知识点，将在技能图谱与能力雷达中自动沉淀统计：</p>
            {allSkillsTree.length === 0 ? (
              <div className="py-8 text-center text-xs text-muted-foreground">暂无技能知识树，请先前往「图谱」页初始化</div>
            ) : (
              <div className="space-y-3">
                {allSkillsTree.map((root) => (
                  <div key={root.id} className="rounded-md border border-border/60 p-2.5">
                    <div className="text-xs font-bold text-foreground">{root.name}</div>
                    <div className="mt-2 flex flex-wrap gap-1.5">
                      {root.children.map((child) => {
                        const checked = selectedSkillIds.includes(child.id)
                        return (
                          <button
                            key={child.id}
                            type="button"
                            onClick={() => {
                              setSelectedSkillIds((prev) =>
                                checked ? prev.filter((id) => id !== child.id) : [...prev, child.id]
                              )
                            }}
                            className={cn(
                              'inline-flex items-center gap-1 rounded px-2 py-1 text-xs font-medium transition-colors',
                              checked
                                ? 'bg-primary text-primary-foreground'
                                : 'border border-border bg-card text-muted-foreground hover:bg-muted hover:text-foreground'
                            )}
                          >
                            <TreeStructure className="size-3" />
                            {child.name}
                          </button>
                        )
                      })}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
          <DialogFooter className="gap-2">
            <Button variant="ghost" onClick={() => setSkillModalOpen(false)}>
              取消
            </Button>
            <Button onClick={saveSkills}>保存技能挂靠</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
