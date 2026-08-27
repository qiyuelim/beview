import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { Check, Plus } from '@phosphor-icons/react'
import { apiGet, apiPost } from '../api/client'
import type { Application } from '../api/types'
import { flattenSkillTree } from '../api/types'
import { FormField } from './FormField'
import { SemBadge } from './SemBadge'
import { QUESTION_TYPES } from '../api/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'

interface R {
  id: number
  name: string
}

const NEW = '__new__'
const SELF = '__self__'

const selectCls =
  'h-9 w-full rounded-md border border-input bg-card px-2 text-sm text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

/**
 * 录入组件（v4：投递＝公司+岗位核心单元）：批量录题 + 内联新增投递/轮次。
 * 链路：选投递 → 选轮次 → 录题。面试完先快速记题目（答案后补）。
 */
export default function QuestionEntry({
  compact = false,
  onDone,
  initialRoundId,
  locked = false,
  lockedLabel,
}: {
  compact?: boolean
  onDone?: (ids: number[]) => void
  /** 从轮次详情进入时预选轮次（连带选好其投递） */
  initialRoundId?: number
  /** 锁定模式（反馈四#2）：绑定投递岗位与轮次，选择器不可编辑，顶部显示只读上下文 */
  locked?: boolean
  /** 锁定模式显示的上下文标签（公司 · 岗位 · 轮次） */
  lockedLabel?: string
}) {
  const [apps, setApps] = useState<Application[]>([])
  const [rounds, setRounds] = useState<R[]>([])
  const [appId, setAppId] = useState('')
  const [round, setRound] = useState('')
  const [content, setContent] = useState('')
  const [myAnswer, setMyAnswer] = useState('')
  const [tags, setTags] = useState('')
  const [skillId, setSkillId] = useState<number | ''>('')
  const [questionType, setQuestionType] = useState('')
  const [skills, setSkills] = useState<{ id: number; name: string }[]>([])
  const [followups, setFollowups] = useState<{ id: string; content: string; my_answer: string; tags: string }[]>([])
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)
  const [created, setCreated] = useState<number[]>([])
  // 票02：疑似重复提示（录入响应附带；可关闭，不阻塞录入）
  const [dupes, setDupes] = useState<{ id: number; content: string }[]>([])
  const [dupesDismissed, setDupesDismissed] = useState(false)

  // 自录题模式（ADR-0014 §18-19）：不关联公司/投递，挂 per-user「自录题库」固定轮次
  const [selfMode, setSelfMode] = useState(false)
  const [askedAt, setAskedAt] = useState('') // 提问日期（可选）
  // 内联新增投递
  const [newAppName, setNewAppName] = useState('')
  const [newAppPos, setNewAppPos] = useState('')
  // 内联新增轮次
  const [newRoundName, setNewRoundName] = useState('')

  // 预选：round -> application 反向链
  const prefill = useRef<{ roundId: number; applicationId?: number } | null>(
    initialRoundId ? { roundId: initialRoundId } : null,
  )

  async function loadApps() {
    const list = await apiGet('/api/applications')
    setApps(list)
    return list as Application[]
  }

  async function loadRounds(aid: string) {
    const d = await apiGet(`/api/applications/${aid}`)
    const rs: R[] = (d.rounds ?? []).map((r: any) => ({ id: r.id, name: r.name }))
    setRounds(rs)
    return rs
  }

  useEffect(() => {
    ;(async () => {
      const list = await loadApps().catch(() => [])
      apiGet('/api/skills')
        .then((res: any) => setSkills(flattenSkillTree(res.tree || [])))
        .catch(() => {})

      // 预选：由轮次反查所属投递
      if (prefill.current) {
        try {
          const all = await apiGet('/api/rounds/all')
          const t = (all as any[]).find((r) => r.round_id === prefill.current?.roundId)
          if (t && prefill.current) {
            prefill.current.applicationId = t.application_id
            setAppId(String(t.application_id))
          } else prefill.current = null
        } catch {
          prefill.current = null
        }
      }
      if (appId) await loadRounds(appId)
      void list
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!appId || appId === NEW) {
      setRounds([])
      return
    }
    ;(async () => {
      const rs = await loadRounds(appId)
      // 预选链：投递就位后选轮次，完成
      const p = prefill.current
      if (p?.applicationId != null && String(p.applicationId) === appId) {
        const target = rs.find((x) => x.id === p.roundId)
        if (target) {
          setRound(String(p.roundId))
          prefill.current = null
          return
        }
      }
      setRound('')
    })()
  }, [appId])

  const lines = content.split('\n').map((l) => l.trim()).filter(Boolean)

  async function createApplication() {
    if (!newAppName.trim()) return
    const r = await apiPost('/api/applications', {
      company_name: newAppName.trim(),
      position: newAppPos.trim() || null,
    })
    await loadApps()
    setAppId(String(r.id))
    setNewAppName('')
    setNewAppPos('')
  }
  async function createRound() {
    if (!newRoundName.trim() || !appId) return
    try {
      setErr('')
      const r = await apiPost(`/api/applications/${appId}/rounds`, { name: newRoundName.trim() })
      await loadRounds(appId)
      setRound(String(r.id))
      setNewRoundName('')
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function submit() {
    if (lines.length === 0) {
      setErr('请至少输入一道题目')
      return
    }
    if (!selfMode && !round) {
      setErr('请选择轮次')
      return
    }
    // 追问只随单题录入：多行内容会按批量模式拆成多题，无法挂追问——显式拦截而非静默丢弃
    const hasFollowups = followups.some((f) => f.content.trim())
    if (lines.length > 1 && hasFollowups) {
      setErr('批量录入（一次多题）不支持附带追问；请一次只录一题并附追问，或先清空追问区')
      setBusy(false)
      return
    }
    setBusy(true)
    setErr('')
    setCreated([])
    setDupes([])
    setDupesDismissed(false)
    const tagList = tags
      .split(/[,，\s]+/)
      .map((t) => t.trim())
      .filter(Boolean)
    const ids: number[] = []
    const asked = askedAt || null
    try {
      const formattedFollowups =
        lines.length === 1 && followups.length > 0
          ? followups
              .filter((f) => f.content.trim())
              .map((f) => ({
                content: f.content.trim(),
                my_answer: f.my_answer.trim() || null,
                tags: f.tags
                  ? f.tags
                      .split(/[,，\s]+/)
                      .map((t) => t.trim())
                      .filter(Boolean)
                  : null,
              }))
          : null

      for (const line of lines) {
        const body = {
          content: line,
          // 批量录入时答案留空，之后在题目详情里回顾补答（LLM 评分）
          my_answer: lines.length === 1 && myAnswer.trim() ? myAnswer.trim() : null,
          asked_at: asked,
          tags: tagList.length ? tagList : null,
          skill_id: skillId ? Number(skillId) : null,
          question_type: questionType || null,
          followups: formattedFollowups,
        }
        const path = selfMode || round === SELF ? '/api/questions/self' : `/api/questions`
        const payload = selfMode ? body : { ...body, round_id: Number(round) }
        const r = await apiPost(path, payload)
        ids.push(r.id)
        if (Array.isArray(r.duplicates)) {
          setDupes((prev) => [...prev, ...r.duplicates.filter((d: { id: number }) => !prev.some((p) => p.id === d.id))])
        }
      }
      setCreated(ids)
      onDone?.(ids)
      setContent('')
      setMyAnswer('')
      setTags('')
      setSkillId('')
      setQuestionType('')
      setFollowups([])
      setAskedAt('')
      const sentFollowups = formattedFollowups?.filter((f) => f.content.trim()).length ?? 0
      toast.success(`已录入 ${ids.length} 道题${sentFollowups > 0 ? `（含 ${sentFollowups} 条追问）` : ''}`)
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
    }
  }

  const selectedApp = apps.find((a) => String(a.id) === appId)

  return (
    <div>
      {locked && (
        <p className="mb-2.5 flex items-center gap-1.5 text-xs text-muted-foreground">
          <SemBadge sem="info">已绑定</SemBadge>
          {lockedLabel || '当前轮次'}（不可更改）
        </p>
      )}

      <div className="space-y-3">
        {!locked && (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <FormField label="投递（公司 · 岗位）" htmlFor="qe-app">
              <select
                id="qe-app"
                value={selfMode ? SELF : appId}
                onChange={(e) => {
                  const v = e.target.value
                  if (v === SELF) {
                    setSelfMode(true)
                    setAppId('')
                    setRound('')
                  } else {
                    setSelfMode(false)
                    setAppId(v)
                  }
                }}
                aria-label="投递"
                className={selectCls}
              >
                <option value="">选择投递</option>
                <option value={SELF}>✎ 自录题（不关联公司/投递）</option>
                {apps.map((a) => (
                  <option key={a.id} value={a.id}>
                    {[a.company ?? '未关联公司', a.position].filter(Boolean).join(' · ')}
                  </option>
                ))}
                <option value={NEW}>＋ 新增投递…</option>
              </select>
              {appId === NEW && (
                <div className="mt-2 flex flex-wrap items-center gap-2">
                  <Input
                    className="h-8 w-36"
                    autoFocus
                    placeholder="公司名称"
                    value={newAppName}
                    onChange={(e) => setNewAppName(e.target.value)}
                    onKeyDown={(e) => e.key === 'Enter' && createApplication()}
                  />
                  <Input
                    className="h-8 w-36"
                    placeholder="岗位（可空）"
                    value={newAppPos}
                    onChange={(e) => setNewAppPos(e.target.value)}
                  />
                  <Button size="sm" onClick={createApplication} disabled={!newAppName.trim()}>
                    创建
                  </Button>
                </div>
              )}
            </FormField>

            {selfMode ? (
              <FormField label="轮次" htmlFor="qe-round">
                <select id="qe-round" value={SELF} disabled aria-label="轮次" className={selectCls}>
                  <option value={SELF}>收藏题 · 自录题库</option>
                </select>
              </FormField>
            ) : (
              <FormField label="轮次" htmlFor="qe-round">
                <select
                  id="qe-round"
                  value={round}
                  onChange={(e) => setRound(e.target.value)}
                  disabled={!appId || appId === NEW}
                  aria-label="轮次"
                  className={selectCls}
                >
                  <option value="">选择轮次</option>
                  {rounds.map((r) => (
                    <option key={r.id} value={r.id}>
                      {r.name}
                    </option>
                  ))}
                  <option value={NEW}>＋ 新增轮次…</option>
                </select>
                {round === NEW && (
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <Input
                      className="h-8 w-44"
                      placeholder={`轮次名称（如：${['一面', '二面', '三面'][rounds.length] ?? `第${rounds.length + 1}轮`}）`}
                      value={newRoundName}
                      onChange={(e) => setNewRoundName(e.target.value)}
                      onKeyDown={(e) => e.key === 'Enter' && createRound()}
                    />
                    <Button size="sm" onClick={createRound} disabled={!newRoundName.trim()}>
                      创建
                    </Button>
                  </div>
                )}
              </FormField>
            )}
          </div>
        )}

        {selectedApp && !locked && (
          <p className="text-xs text-muted-foreground">
            录入到「{[selectedApp.company ?? '未关联公司', selectedApp.position].filter(Boolean).join(' · ')}」
          </p>
        )}

        <FormField label="提问日期（可选，实际被问到的日期）" htmlFor="qe-asked">
          <Input id="qe-asked" type="date" value={askedAt} onChange={(e) => setAskedAt(e.target.value)} className="w-44" />
        </FormField>

        <FormField
          label={lines.length > 1 ? `题目（已识别 ${lines.length} 道，每行一道）` : '题目（每行一道，可批量）'}
          htmlFor="qe-content"
        >
          <Textarea
            id="qe-content"
            rows={compact ? 4 : 6}
            value={content}
            onChange={(e) => setContent(e.target.value)}
            placeholder={'每行一道题，支持一次批量录入，如：\n讲一下 HashMap 的底层实现\n数据库索引为什么用 B+ 树'}
          />
        </FormField>

        {!compact && (
          <>
            <FormField label="我的现场回答（单题时填写，越全越好）" htmlFor="qe-answer">
              <Textarea
                id="qe-answer"
                rows={compact ? 4 : 6}
                value={myAnswer}
                onChange={(e) => setMyAnswer(e.target.value)}
                placeholder="批量录入时答案留空，之后在题目详情里回顾补答，再让 LLM 评分"
              />
            </FormField>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
              <FormField label="标签" htmlFor="qe-tags" hint="可选，逗号分隔，批量时统一应用">
                <Input
                  id="qe-tags"
                  value={tags}
                  onChange={(e) => setTags(e.target.value)}
                  placeholder="算法, 数据库"
                />
              </FormField>
              <FormField label="挂靠技能" htmlFor="qe-skill" hint="可选，知识树节点">
                <select
                  id="qe-skill"
                  value={skillId}
                  onChange={(e) => setSkillId(e.target.value === '' ? '' : Number(e.target.value))}
                  className={selectCls}
                >
                  <option value="">未分类技能</option>
                  {skills.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </select>
              </FormField>
              <FormField label="考察维度" htmlFor="qe-qtype" hint="可选，能力矩阵">
                <select
                  id="qe-qtype"
                  value={questionType}
                  onChange={(e) => setQuestionType(e.target.value)}
                  className={selectCls}
                >
                  <option value="">未分类（默认）</option>
                  {QUESTION_TYPES.map((t) => (
                    <option key={t.value} value={t.value}>
                      {t.label}
                    </option>
                  ))}
                </select>
              </FormField>
            </div>

            {/* 一级连续追问录入块（推特式一级追问，不计入总题数） */}
            {lines.length <= 1 && (
              <div className="rounded-lg border border-border bg-card/60 p-3 space-y-3">
                <div className="flex items-center justify-between">
                  <span className="text-xs font-semibold text-foreground">
                    💬 现场连续追问（可选，随主题目一同记录，不重复计题）
                  </span>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-7 text-xs"
                    onClick={() =>
                      setFollowups((prev) => [
                        ...prev,
                        { id: String(Date.now()), content: '', my_answer: '', tags: '' },
                      ])
                    }
                  >
                    <Plus className="mr-1 h-3.5 w-3.5" />
                    添加追问
                  </Button>
                </div>

                {followups.map((f, idx) => (
                  <div key={f.id} className="relative rounded border border-border/80 bg-background/80 p-2.5 space-y-2 text-xs">
                    <div className="flex items-center justify-between">
                      <span className="font-semibold text-muted-foreground">追问 #{idx + 1}</span>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="h-6 text-xs text-destructive hover:bg-destructive/10"
                        onClick={() => setFollowups((prev) => prev.filter((item) => item.id !== f.id))}
                      >
                        删除
                      </Button>
                    </div>
                    <Input
                      placeholder="追问题干，如：那在集群扩容时一致性哈希怎么迁移？"
                      value={f.content}
                      onChange={(e) =>
                        setFollowups((prev) =>
                          prev.map((item) => (item.id === f.id ? { ...item, content: e.target.value } : item))
                        )
                      }
                      className="h-8 text-xs"
                    />
                    <Textarea
                      rows={2}
                      placeholder="追问回答（可选）"
                      value={f.my_answer}
                      onChange={(e) =>
                        setFollowups((prev) =>
                          prev.map((item) => (item.id === f.id ? { ...item, my_answer: e.target.value } : item))
                        )
                      }
                      className="text-xs"
                    />
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>

      {err && (
        <p role="alert" className="mt-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      <div className="mt-4 flex flex-wrap items-center gap-2">
        <Button onClick={submit} disabled={busy || lines.length === 0}>
          <Plus weight="bold" className="size-4" aria-hidden />
          {busy ? '保存中…' : lines.length > 1 ? `保存 ${lines.length} 道题` : '保存题目'}
        </Button>
        {!compact && created.length > 0 && (
          <span className="inline-flex items-center gap-1 text-sm font-medium text-success">
            <Check className="size-4" weight="bold" aria-hidden /> 已录入 {created.length} 道
          </span>
        )}
      </div>

      {!compact && dupes.length > 0 && !dupesDismissed && (
        <div
          role="status"
          className="mt-3 rounded-lg border border-warning/40 bg-warning/10 p-3"
        >
          <div className="flex items-start justify-between gap-2">
            <div className="text-sm font-medium text-foreground">
              疑似与已有题目重复（已照常录入）
            </div>
            <button
              type="button"
              onClick={() => setDupesDismissed(true)}
              className="shrink-0 rounded px-1 text-xs text-muted-foreground hover:bg-muted"
              aria-label="关闭重复提示"
            >
              ✕
            </button>
          </div>
          <div className="mt-1.5 space-y-1">
            {dupes.map((d) => (
              <Link
                key={d.id}
                to={`/questions/${d.id}`}
                className="block truncate text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
              >
                #{d.id} {d.content}
              </Link>
            ))}
          </div>
        </div>
      )}

      {!compact && created.length > 1 && (
        <div className="mt-3 rounded-lg border border-border bg-card p-3">
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            本次录入（可点击去补答/分析）
          </div>
          <div className="flex flex-wrap gap-1.5">
            {created.map((id) => (
              <Link
                key={id}
                to={`/questions/${id}`}
                className="rounded bg-muted px-1.5 py-0.5 text-xs hover:bg-muted/80"
              >
                #{id} 查看 →
              </Link>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
