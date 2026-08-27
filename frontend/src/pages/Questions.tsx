import { useEffect, useRef, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router-dom'
import { apiDelete, apiGet, apiPatch, apiPost } from '../api/client'
import { ASSESSMENT_DIMENSION_LABELS, type AssessmentDimension, flattenSkillTree, type QuestionRow } from '../api/types'
import { onBatchItemDone, setGlobalBatchAnalysis, useGlobalBatchAnalysis } from '../ai/jobs'
import { CaretDown, Funnel, ListChecks, MagnifyingGlass, Plus, Sparkle, Star, Trash, X } from '@phosphor-icons/react'
import { PageHeader } from '../components/PageHeader'
import { EmptyState } from '../components/EmptyState'
import { SemBadge } from '../components/SemBadge'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { toast } from 'sonner'
import { cn } from '@/lib/utils'
import { FormField } from '../components/FormField'

interface Ctx {
  companies: { id: number; name: string }[]
  rounds: { id: number; name: string }[]
  tags: string[]
  skills: { id: number; name: string; path: string }[]
}

const selectCls =
  'h-10 sm:h-9 w-full rounded-lg sm:rounded-md border border-input bg-card px-2.5 sm:px-2 text-sm text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'

function FilterChip({
  label,
  value,
  display,
  options,
  onChange,
}: {
  label: string
  value: string
  display?: string
  options: { value: string; label: string }[]
  onChange: (v: string) => void
}) {
  const active = Boolean(value)
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn(
            'inline-flex h-10 min-h-[44px] items-center gap-1 rounded-full border px-3 text-xs font-medium sm:h-9 sm:min-h-0',
            active ? 'chip-selected' : 'border-border bg-card text-foreground hover:bg-muted',
          )}
        >
          <span className="max-w-[10rem] truncate">{active ? display || label : label}</span>
          {active ? (
            <span
              role="button"
              tabIndex={0}
              aria-label={`清除${label}`}
              className="grid size-4 place-items-center rounded-full hover:bg-muted"
              onPointerDown={(e) => {
                e.preventDefault()
                e.stopPropagation()
                onChange('')
              }}
            >
              <X className="size-3" aria-hidden />
            </span>
          ) : (
            <CaretDown className="size-3.5 text-muted-foreground" aria-hidden />
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="max-h-72 w-56 overflow-y-auto p-1">
        <button
          type="button"
          className="w-full rounded px-2 py-2 text-left text-sm hover:bg-muted"
          onClick={() => onChange('')}
        >
          全部{label}
        </button>
        {options.map((o) => (
          <button
            key={o.value}
            type="button"
            className={cn('w-full rounded px-2 py-2 text-left text-sm hover:bg-muted', value === o.value && 'chip-selected')}
            onClick={() => onChange(o.value)}
          >
            {o.label}
          </button>
        ))}
      </PopoverContent>
    </Popover>
  )
}

export default function Questions() {
  const [sp, setSp] = useSearchParams()
  const nav = useNavigate()
  const [rows, setRows] = useState<QuestionRow[]>([])
  const [ctx, setCtx] = useState<Ctx>({ companies: [], rounds: [], tags: [], skills: [] })
  const [err, setErr] = useState('')
  const [loading, setLoading] = useState(true)
  const [df, setDf] = useState({ company: '', round: '', tag: '', skill_id: '', question_type: '', analyzed: '', starred: '', q: '' })
  const [filterDrawerOpen, setFilterDrawerOpen] = useState(false)
  const [drawerDraft, setDrawerDraft] = useState(df)
  const dfRef = useRef(df)
  dfRef.current = df
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const [multi, setMulti] = useState(false)
  const [sel, setSel] = useState<Set<number>>(new Set())
  const [batch, setBatch] = useState<any>(null)
  const [delOpen, setDelOpen] = useState(false)
  // 用户裁决 4：混选/全已分析时弹窗分流，而非服务端静默过滤
  const [batchConfirm, setBatchConfirm] = useState<{ all: number; keep: number } | null>(null)
  const [analyzingIds, setAnalyzingIds] = useState<Set<number>>(new Set())

  // 标签清洗聚合状态（用户裁决 3：防止标签无限裂变，支持 merge-to-skill 迁移）
  interface TagCleanupGroup {
    canonical: string
    aliases: string[]
    note?: string
    target_skill_id?: number | null
    target_skill_name?: string | null
  }
  const [cleanupOpen, setCleanupOpen] = useState(false)
  const [cleaning, setCleaning] = useState(false)
  const [cleanupProposals, setCleanupProposals] = useState<TagCleanupGroup[]>([])
  const [selectedGroupIdxs, setSelectedGroupIdxs] = useState<Set<number>>(new Set())

  const globalBatch = useGlobalBatchAnalysis()
  const lastNotifiedBatchJobIdRef = useRef<number | null>(null)

  // 监听全局批量分析事件（0 轮询）
  useEffect(() => {
    if (globalBatch) {
      setBatch(globalBatch)
      if (globalBatch.status === 'done' || globalBatch.status === 'cancelled' || globalBatch.status === 'error') {
        setAnalyzingIds(new Set())
        if (globalBatch.status === 'done' && lastNotifiedBatchJobIdRef.current !== globalBatch.jobId) {
          lastNotifiedBatchJobIdRef.current = globalBatch.jobId
          toast.success(`批量分析完成：成功 ${globalBatch.ok} 题，失败 ${globalBatch.failed} 题`)
          load()
        }
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [globalBatch])

  // 监听单题分析完成 SSE 事件（局部极速 Patch，0 额外 GET 请求）
  useEffect(() => {
    const unsub = onBatchItemDone((qid, ok) => {
      setAnalyzingIds((prev) => {
        const next = new Set(prev)
        next.delete(qid)
        return next
      })
      if (ok) {
        // 增量刷新该题目的分析状态
        setRows((prev) =>
          prev.map((r) => (r.id === qid ? { ...r, analyzed: true } : r)),
        )
      }
    })
    return () => {
      unsub()
    }
  }, [])

  // 外部导航（如轮次子页带 ?round=）时同步草稿
  useEffect(() => {
    setDf({
      company: sp.get('company') || '',
      round: sp.get('round') || '',
      tag: sp.get('tag') || '',
      skill_id: sp.get('skill_id') || '',
      question_type: sp.get('question_type') || '',
      analyzed: sp.get('analyzed') || '',
      starred: sp.get('starred') || '',
      q: sp.get('q') || '',
    })
  }, [sp])

  useEffect(() => {
    // 反馈七#4：公司筛选需含系统公司（模拟面试=陪练沉淀题归属），否则选中也无法命中
    Promise.all([
      apiGet('/api/companies?include_system=true'),
      apiGet('/api/tags'),
      apiGet('/api/skills').then((res: any) => flattenSkillTree(res.tree || [])).catch(() => []),
    ])
      .then(([companies, tags, skills]) => setCtx((c) => ({ ...c, companies, tags, skills: skills || [] })))
      .catch((e) => setErr(e.message))
  }, [])

  // C组 #2 根因修复：级联跟随「草稿」的公司选择实时刷新（此前依赖 URL 参数，点查询才生效，
  // 导致选了公司轮次仍为空）；未选公司时加载全部轮次，下拉恒有可选内容。
  const roundCompany = filterDrawerOpen ? drawerDraft.company : df.company
  useEffect(() => {
    const url = roundCompany ? `/api/rounds/all?company=${roundCompany}` : '/api/rounds/all'
    apiGet(url)
      .then((rows: any[]) =>
        setCtx((c) => ({
          ...c,
          rounds: rows.map((r) => ({
            id: r.round_id,
            name: [r.position, r.round_name].filter(Boolean).join(' · '),
          })),
        })),
      )
      .catch(() => setCtx((c) => ({ ...c, rounds: [] })))
  }, [roundCompany])

  async function load() {
    setLoading(true)
    setErr('')
    const p = new URLSearchParams()
    // source / position_id：票03 押题命中率卡片点击穿透（?source=predicted&position_id=N）
    for (const k of ['company', 'round', 'tag', 'skill_id', 'question_type', 'analyzed', 'starred', 'q', 'source', 'position_id'] as const) {
      const v = sp.get(k)
      if (v) p.set(k, v)
    }
    const s = p.toString()
    try {
      const d = await apiGet(`/api/questions${s ? `?${s}` : ''}`)
      setRows(d)
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setLoading(false)
    }
  }
  useEffect(() => {
    load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sp])

  function commitFilters(next: typeof df) {
    const n = new URLSearchParams()
    for (const [k, v] of Object.entries(next)) if (v) n.set(k, v)
    for (const k of ['source', 'position_id'] as const) {
      const v = sp.get(k)
      if (v) n.set(k, v)
    }
    setSp(n, { replace: true })
  }

  function patchFilter(patch: Partial<typeof df>, debounce = false) {
    const next = { ...dfRef.current, ...patch }
    dfRef.current = next
    setDf(next)
    if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current)
    if (debounce) {
      searchDebounceRef.current = setTimeout(() => commitFilters(next), 300)
    } else {
      commitFilters(next)
    }
  }

  function setDfKey(k: keyof typeof df, v: string) {
    const extra = k === 'company' ? { round: '' } : {}
    patchFilter({ [k]: v, ...extra }, k === 'q')
  }

  function resetFilters() {
    if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current)
    const empty = { company: '', round: '', tag: '', skill_id: '', question_type: '', analyzed: '', starred: '', q: '' }
    dfRef.current = empty
    setDf(empty)
    commitFilters(empty)
  }

  function toggleMulti() {
    setMulti((m) => !m)
    setSel(new Set())
  }
  function toggleRow(id: number) {
    setSel((prev) => {
      const n = new Set(prev)
      if (n.has(id)) n.delete(id)
      else n.add(id)
      return n
    })
  }

  async function bulkDelete() {
    if (sel.size === 0) return
    const count = sel.size
    await apiDelete('/api/questions', { ids: [...sel] })
    setSel(new Set())
    setMulti(false)
    setDelOpen(false)
    toast.success(`已删除 ${count} 道题`)
    await load()
  }

  async function batchAnalyze() {
    if (sel.size === 0) return
    const selRows = rows.filter((r) => sel.has(r.id))
    const analyzedN = selRows.filter((r) => r.analyzed).length
    const freshN = sel.size - analyzedN
    if (analyzedN > 0 && freshN > 0) {
      setBatchConfirm({ all: analyzedN, keep: freshN }) // 混选：三选弹窗
      return
    }
    if (analyzedN > 0) {
      setBatchConfirm({ all: analyzedN, keep: 0 }) // 全部已分析：覆盖重评需确认
      return
    }
    await runBatch('unanalyzed', freshN)
  }

  async function runBatch(mode: 'unanalyzed' | 'all', total: number) {
    setErr('')
    const targetIds = Array.from(sel)
    setBatch({ id: 0, total, done: 0, ok: 0, failed: 0, status: 'running' })
    setGlobalBatchAnalysis({ jobId: 0, total, done: 0, ok: 0, failed: 0, status: 'running' })
    
    // 立即退出选择模式并标记正在分析的题目
    setAnalyzingIds(new Set(targetIds))
    setSel(new Set())
    setMulti(false)

    try {
      const r = await apiPost('/api/questions/batch-analyze', { ids: targetIds, mode })
      const jobId = r.job_id
      setBatch((prev: any) => {
        if (prev && (prev.status === 'done' || prev.status === 'cancelled')) return prev
        return { id: jobId, total, done: prev ? prev.done : 0, ok: prev ? prev.ok : 0, failed: prev ? prev.failed : 0, status: 'running' }
      })
    } catch (e: any) {
      setErr(e.message)
      setBatch(null)
      setAnalyzingIds(new Set())
      setGlobalBatchAnalysis(null)
    }
  }

  async function cancelBatch() {
    if (!batch || batch.id === 0) return
    try {
      await apiDelete(`/api/questions/batch-analyze/${batch.id}`)
      toast('已取消批量分析')
      const updated = { ...batch, status: 'cancelled' as const }
      setBatch(updated)
      setAnalyzingIds(new Set())
      setGlobalBatchAnalysis({
        jobId: batch.id,
        total: batch.total,
        done: batch.done,
        ok: batch.ok,
        failed: batch.failed,
        status: 'cancelled',
      })
      await load()
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function toggleStar(row: QuestionRow) {
    await apiPatch(`/api/questions/${row.id}`, { starred: !row.starred })
    await load()
  }

  async function handleStartTagCleanup() {
    setCleaning(true)
    try {
      const unmapped = await apiGet('/api/skills/unmapped-tags')
      if (!unmapped || unmapped.length === 0) {
        toast.info('题库中暂无可聚合清洗的自由标签')
        return
      }
      toast.loading('AI 正在深度分析自由标签并生成规范化清洗建议...', { id: 'tag-cleanup' })
      const res = await apiPost('/api/skills/tags/cleanup/propose', {})
      toast.dismiss('tag-cleanup')
      if (res?.groups && res.groups.length > 0) {
        setCleanupProposals(res.groups)
        setSelectedGroupIdxs(new Set(res.groups.map((_: any, idx: number) => idx)))
        setCleanupOpen(true)
      } else {
        toast.info('标签规范度良好，未发现需要合并的近义或冗余标签')
      }
    } catch (e: any) {
      toast.dismiss('tag-cleanup')
      toast.error(e.message || '获取标签清洗建议失败')
    } finally {
      setCleaning(false)
    }
  }

  async function handleApplyTagCleanup() {
    if (selectedGroupIdxs.size === 0) {
      toast.info('请至少选择一组聚合建议')
      return
    }
    const groupsToApply = cleanupProposals
      .filter((_, idx) => selectedGroupIdxs.has(idx))
      .map((g) => ({
        canonical: g.canonical,
        aliases: g.aliases,
        target_skill_id: g.target_skill_id ?? null,
      }))

    setCleaning(true)
    try {
      const res = await apiPost('/api/skills/tags/cleanup/apply', { groups: groupsToApply })
      toast.success(`标签清洗成功：重映射 ${res.remapped} 处标签关联，清理 ${res.removed_tags} 个冗余标签`)
      setCleanupOpen(false)
      setCleanupProposals([])
      const [, tags] = await Promise.all([load(), apiGet('/api/tags')])
      setCtx((c) => ({ ...c, tags }))
    } catch (e: any) {
      toast.error(e.message || '应用标签清洗失败')
    } finally {
      setCleaning(false)
    }
  }

  const analyzedRows = rows.filter((r) => r.analyzed)
  const avgScore = analyzedRows.length
    ? Math.round(analyzedRows.reduce((sum, r) => sum + (r.last_score ?? 0), 0) / analyzedRows.length)
    : null

  return (
    <div>
      <PageHeader
        title="题库"
        meta={<span>真题、押题与追问</span>}
        actions={
          <>
            <Button
              variant="outline"
              onClick={handleStartTagCleanup}
              disabled={cleaning}
              title="利用 AI 分析题库中的分散自由标签并聚合规范化"
            >
              <Sparkle weight="fill" className="size-4 text-primary" aria-hidden />
              {cleaning ? '分析标签中…' : '标签清洗'}
            </Button>
            <Button variant="outline" onClick={toggleMulti} aria-pressed={multi}>
              {multi ? '取消选择' : '选择'}
            </Button>
            <Button asChild>
              <Link to="/new">
                <Plus weight="bold" className="size-4" aria-hidden /> 录入题目
              </Link>
            </Button>
          </>
        }
      />

      <section className="mb-3 rounded-xl border border-border bg-card p-4" aria-label="筛选">
        <div className="flex flex-wrap items-center gap-2">
          <div className="relative min-w-[190px] flex-1">
            <MagnifyingGlass
              className="absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
              aria-hidden
            />
            <Input
              className="h-10 pl-8 text-sm sm:h-9"
              placeholder="搜索关键词…"
              value={df.q}
              onChange={(e) => setDfKey('q', e.target.value)}
              aria-label="搜索"
            />
          </div>
          <div className="hidden md:contents">
          <FilterChip
            label="公司"
            value={df.company}
            display={ctx.companies.find((c) => String(c.id) === df.company)?.name}
            options={ctx.companies.map((c) => ({ value: String(c.id), label: c.name }))}
            onChange={(v) => setDfKey('company', v)}
          />
          <FilterChip
            label="技能"
            value={df.skill_id}
            display={ctx.skills.find((s) => String(s.id) === df.skill_id)?.name}
            options={ctx.skills.map((s) => ({ value: String(s.id), label: s.name }))}
            onChange={(v) => setDfKey('skill_id', v)}
          />
          <FilterChip
            label="考察维度"
            value={df.question_type}
            display={ASSESSMENT_DIMENSION_LABELS[df.question_type as AssessmentDimension]}
            options={(Object.keys(ASSESSMENT_DIMENSION_LABELS) as AssessmentDimension[]).map((dim) => ({
              value: dim,
              label: ASSESSMENT_DIMENSION_LABELS[dim],
            }))}
            onChange={(v) => setDfKey('question_type', v)}
          />
          <FilterChip
            label="分析状态"
            value={df.analyzed}
            display={df.analyzed === 'true' ? '已分析' : df.analyzed === 'false' ? '未分析' : undefined}
            options={[
              { value: 'true', label: '已分析' },
              { value: 'false', label: '未分析' },
            ]}
            onChange={(v) => setDfKey('analyzed', v)}
          />
          </div>
          {df.round && (
            <button
              type="button"
              className="chip-selected inline-flex h-10 min-h-[44px] items-center gap-1 rounded-full px-3 text-xs sm:h-9 sm:min-h-0"
              onClick={() => setDfKey('round', '')}
            >
              {ctx.rounds.find((r) => String(r.id) === df.round)?.name || '轮次'}
              <X className="size-3" aria-hidden />
            </button>
          )}
          {df.starred && (
            <button
              type="button"
              className="chip-selected inline-flex h-10 min-h-[44px] items-center gap-1 rounded-full px-3 text-xs sm:h-9 sm:min-h-0"
              onClick={() => setDfKey('starred', '')}
            >
              已收藏
              <X className="size-3" aria-hidden />
            </button>
          )}
          {df.tag && (
            <button
              type="button"
              className="chip-selected inline-flex h-10 min-h-[44px] items-center gap-1 rounded-full px-3 text-xs sm:h-9 sm:min-h-0"
              onClick={() => setDfKey('tag', '')}
            >
              #{df.tag}
              <X className="size-3" aria-hidden />
            </button>
          )}
          <Button
            size="sm"
            variant="outline"
            className="h-10 min-h-[44px] px-3 sm:h-9 sm:min-h-0"
            onClick={() => {
              setDrawerDraft(df)
              setFilterDrawerOpen(true)
            }}
            aria-expanded={filterDrawerOpen}
          >
            <Funnel className="size-4" aria-hidden />
            <span className="md:hidden">筛选</span>
            <span className="hidden md:inline">全部筛选</span>
            {[df.company, df.round, df.skill_id, df.question_type, df.analyzed, df.starred, df.tag].filter(Boolean).length >
              0 && (
              <span className="ml-1 grid size-4 place-items-center rounded-full bg-primary text-[10px] font-bold text-primary-foreground">
                {[df.company, df.round, df.skill_id, df.question_type, df.analyzed, df.starred, df.tag].filter(Boolean).length}
              </span>
            )}
          </Button>
          <Button size="sm" variant="ghost" onClick={resetFilters}>
            重置
          </Button>
        </div>
      </section>

      {filterDrawerOpen && (
        <div
          className="fixed inset-0 z-50 flex justify-end bg-black/50"
          onClick={() => setFilterDrawerOpen(false)}
        >
          <div
            role="dialog"
            aria-label="全部筛选"
            className="flex h-full w-full max-w-md flex-col border-l border-border bg-card shadow-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between border-b border-border px-4 py-3">
              <h3 className="text-sm font-semibold text-foreground">全部筛选</h3>
              <Button size="sm" variant="ghost" onClick={() => setFilterDrawerOpen(false)} aria-label="关闭筛选">
                <X className="size-4" />
              </Button>
            </div>
            <div className="flex-1 space-y-3 overflow-y-auto p-4">
              <FormField label="公司" htmlFor="qf-company">
                <select
                  id="qf-company"
                  value={drawerDraft.company}
                  onChange={(e) => setDrawerDraft((d) => ({ ...d, company: e.target.value, round: '' }))}
                  className={selectCls}
                >
                  <option value="">全部公司</option>
                  {ctx.companies.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
              </FormField>
              <FormField label="技能" htmlFor="qf-skill">
                <select id="qf-skill" value={drawerDraft.skill_id} onChange={(e) => setDrawerDraft((d) => ({ ...d, skill_id: e.target.value }))} className={selectCls}>
                  <option value="">全部技能</option>
                  {ctx.skills.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name}
                    </option>
                  ))}
                </select>
              </FormField>
              <FormField label="考察维度" htmlFor="qf-dim">
                <select id="qf-dim" value={drawerDraft.question_type} onChange={(e) => setDrawerDraft((d) => ({ ...d, question_type: e.target.value }))} className={selectCls}>
                  <option value="">全部考察维度</option>
                  {(Object.keys(ASSESSMENT_DIMENSION_LABELS) as AssessmentDimension[]).map((dim) => (
                    <option key={dim} value={dim}>
                      {ASSESSMENT_DIMENSION_LABELS[dim]}
                    </option>
                  ))}
                </select>
              </FormField>
              <FormField label="分析状态" htmlFor="qf-analyzed">
                <select id="qf-analyzed" value={drawerDraft.analyzed} onChange={(e) => setDrawerDraft((d) => ({ ...d, analyzed: e.target.value }))} className={selectCls}>
                  <option value="">不限</option>
                  <option value="true">已分析</option>
                  <option value="false">未分析</option>
                </select>
              </FormField>
              <FormField label="轮次" htmlFor="qf-round" hint="与上方筛选一并生效">
                <select id="qf-round" value={drawerDraft.round} onChange={(e) => setDrawerDraft((d) => ({ ...d, round: e.target.value }))} className={selectCls}>
                  <option value="">全部轮次</option>
                  {ctx.rounds.map((r) => (
                    <option key={r.id} value={r.id}>
                      {r.name}
                    </option>
                  ))}
                </select>
              </FormField>
              <FormField label="收藏" htmlFor="qf-starred">
                <select id="qf-starred" value={drawerDraft.starred} onChange={(e) => setDrawerDraft((d) => ({ ...d, starred: e.target.value }))} className={selectCls}>
                  <option value="">全部</option>
                  <option value="true">仅收藏</option>
                </select>
              </FormField>
              <FormField label="标签" htmlFor="qf-tag">
                <select id="qf-tag" value={drawerDraft.tag} onChange={(e) => setDrawerDraft((d) => ({ ...d, tag: e.target.value }))} className={selectCls}>
                  <option value="">全部标签</option>
                  {ctx.tags.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </FormField>
            </div>
            <div className="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
              <Button
                variant="ghost"
                className="h-10"
                onClick={() => {
                  resetFilters()
                  setFilterDrawerOpen(false)
                }}
              >
                重置
              </Button>
              <Button
                className="h-10"
                onClick={() => {
                  if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current)
                  dfRef.current = drawerDraft
                  setDf(drawerDraft)
                  commitFilters(drawerDraft)
                  setFilterDrawerOpen(false)
                }}
              >
                应用
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 顶部独立批量分析进度卡片 */}
      {batch && batch.status === 'running' && (
        <section className="mb-3 rounded-xl border border-primary/40 bg-primary/5 p-3.5 shadow-sm" aria-label="批量分析进度">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2">
              <span className="relative flex size-2.5">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-primary opacity-75" />
                <span className="relative inline-flex size-2.5 rounded-full bg-primary" />
              </span>
              <span className="text-sm font-semibold text-foreground">
                ⚡ 正在批量 AI 深度分析中 ({batch.done} / {batch.total})
              </span>
              <span className="text-xs text-muted-foreground">
                成功 {batch.ok} · 失败 {batch.failed}
              </span>
            </div>
            <Button size="sm" variant="outline" className="h-8 text-xs text-destructive border-destructive/30 hover:bg-destructive/10" onClick={cancelBatch}>
              取消分析
            </Button>
          </div>
          <div className="mt-2.5 h-1.5 w-full overflow-hidden rounded-full bg-muted">
            <div
              className="h-full bg-primary transition-all duration-300"
              style={{ width: `${Math.max(5, (batch.done / Math.max(1, batch.total)) * 100)}%` }}
            />
          </div>
        </section>
      )}

      {/* 多选批量工具条 */}
      {multi && (
        <section className="mb-3 flex flex-wrap items-center gap-2 rounded-xl border border-primary/40 bg-card px-3.5 py-3 shadow-sm" aria-label="批量操作">
          <span className="text-xs font-medium text-foreground">
            已勾选 <b className="font-mono text-sm text-primary">{sel.size}</b> 道题
          </span>
          <Button size="sm" variant="outline" className="h-8 text-xs" onClick={() => setSel(new Set(rows.map((r) => r.id)))} disabled={rows.length === 0}>
            全选本页
          </Button>
          <Button size="sm" variant="ghost" className="h-8 text-xs" onClick={() => setSel(new Set())}>
            全不选
          </Button>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              className="h-8 text-xs"
              onClick={batchAnalyze}
              disabled={sel.size === 0 || batch?.status === 'running'}
            >
              <Sparkle weight="fill" className="size-3.5" aria-hidden /> 批量分析
            </Button>
            <Button size="sm" variant="destructive" className="h-8 text-xs" onClick={() => setDelOpen(true)} disabled={sel.size === 0}>
              <Trash className="size-3.5" aria-hidden /> 删除选中
            </Button>
            <Button size="sm" variant="ghost" className="h-8 text-xs text-muted-foreground hover:text-foreground" onClick={toggleMulti}>
              ✕ 退出
            </Button>
          </div>
        </section>
      )}

      {/* 结果概览（核心计数，正常对比度） */}
      {!loading && rows.length > 0 && (
        <div className="mb-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs font-medium">
          <span>共 <b className="font-mono tabular-nums">{rows.length}</b> 条</span>
          <span>已分析 <b className="font-mono tabular-nums">{analyzedRows.length}</b></span>
          <span>均分 <b className="font-mono tabular-nums">{avgScore ?? '—'}</b></span>
          <span>未补答 <b className="font-mono tabular-nums">{rows.filter((r) => !r.my_answer).length}</b></span>
          <span>收藏 <b className="font-mono tabular-nums">{rows.filter((r) => r.starred).length}</b></span>
        </div>
      )}

      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {loading ? (
        <div className="space-y-2.5">
          {Array.from({ length: 4 }).map((_, i) => (
            <Skeleton key={i} className="h-28 w-full rounded-xl" />
          ))}
        </div>
      ) : rows.length === 0 ? (
        <EmptyState
          icon={<ListChecks className="size-10" />}
          title="没有符合条件的题目"
          hint="调整筛选，或录入一道新题。"
          action={
            <Button asChild className="h-11 px-5">
              <Link to="/new">录入题目</Link>
            </Button>
          }
        />
      ) : (
        <ul className="space-y-2.5">
          {rows.map((row) => (
            <li
              key={row.id}
              className={`rounded-xl border bg-card transition-colors duration-150 ${
                multi && sel.has(row.id)
                  ? 'border-primary ring-1 ring-primary/40'
                  : 'border-border hover:border-border-strong hover:bg-muted/40'
              }`}
            >
              <div
                className="flex cursor-pointer gap-3 p-3.5 sm:p-4"
                onClick={() => nav(`/questions/${row.id}`)}
                title="进入题目详情"
              >
                {multi && (
                  <div className="flex items-center pr-1" onClick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      className="size-5 shrink-0 accent-[var(--primary)]"
                      checked={sel.has(row.id)}
                      onChange={() => toggleRow(row.id)}
                      aria-label={`选择 ${row.content.slice(0, 20)}`}
                    />
                  </div>
                )}
                <span
                  className="grid size-9 shrink-0 place-items-center rounded-lg bg-secondary font-semibold text-secondary-foreground text-xs"
                  aria-hidden
                >
                  {row.company ? row.company.slice(0, 1).toUpperCase() : row.source === 'ai_drill' ? 'AI' : '?'}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-baseline gap-x-2 text-xs">
                    <span className="font-medium text-foreground">
                      {[row.company || (row.source === 'ai_drill' ? 'AI 模拟' : '未归属'), row.department, row.position]
                        .filter(Boolean)
                        .join(' · ')}
                    </span>
                    {row.asked_at && <span className="text-muted-foreground">· {row.asked_at}</span>}
                    {row.last_difficulty != null && (
                      <span className="text-muted-foreground">
                        · 难度 <b className="font-mono">{row.last_difficulty}</b>
                      </span>
                    )}
                    <span className="ml-auto font-mono text-muted-foreground">{row.created_at.slice(0, 10)}</span>
                  </div>
                  <div className="mt-1 text-sm font-medium leading-6 text-foreground line-clamp-2">
                    {row.starred && (
                      <Star className="mr-1 inline size-3.5 -translate-y-px fill-warning text-warning" weight="fill" aria-hidden />
                    )}
                    {row.content}
                  </div>
                  <div className="mt-2 flex flex-wrap items-center gap-1.5">
                    {row.tags.map((t) => (
                      <span key={t} className="rounded-md bg-muted px-2 py-0.5 text-xs text-muted-foreground font-medium">
                        #{t}
                      </span>
                    ))}
                    {row.followup_count != null && row.followup_count > 0 && (
                      <span className="rounded-md bg-muted px-2 py-0.5 text-xs text-primary font-semibold">
                        💬 {row.followup_count} 追问
                      </span>
                    )}
                    {row.question_type && (
                      <span className="rounded-md bg-secondary/80 px-2 py-0.5 text-xs text-secondary-foreground font-medium">
                        {ASSESSMENT_DIMENSION_LABELS[row.question_type as AssessmentDimension] || row.question_type}
                      </span>
                    )}
                    {analyzingIds.has(row.id) ? (
                      <span className="inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-xs font-semibold animate-pulse chip-accent-selected">
                        <Sparkle weight="fill" className="size-3 animate-spin text-accent" />
                        AI 分析中…
                      </span>
                    ) : (
                      <SemBadge sem={row.analyzed ? 'pass' : 'warn'}>
                        {row.analyzed ? '已分析' : '未分析'}
                      </SemBadge>
                    )}
                    {row.last_score != null ? (
                      <span
                        className={`rounded-md px-2 py-0.5 font-mono text-xs font-semibold ${
                          row.last_score >= 80
                            ? 'bg-success/15 text-success'
                            : row.last_score >= 60
                            ? 'bg-warning/15 text-warning'
                            : 'bg-destructive/15 text-destructive'
                        }`}
                        title="AI 回答综合评分 (0-100)"
                      >
                        {row.last_score} 分
                      </span>
                    ) : (
                      <span className="rounded-md bg-muted/60 px-1.5 py-0.5 text-[11px] text-muted-foreground">
                        未评分
                      </span>
                    )}
                    <div className="ml-auto flex items-center">
                      <button
                        className={`inline-flex min-h-[36px] min-w-[44px] items-center justify-center gap-1 rounded-lg border border-border/60 px-2.5 py-1.5 text-xs transition-colors hover:bg-muted active:scale-95 ${
                          row.starred ? 'border-warning/30 bg-warning/10 text-warning font-medium' : 'text-muted-foreground'
                        }`}
                        onClick={(e) => {
                          e.stopPropagation()
                          toggleStar(row)
                        }}
                        aria-label={row.starred ? '取消收藏' : '收藏'}
                      >
                        <Star className="size-4" weight={row.starred ? 'fill' : 'regular'} aria-hidden />
                        <span>{row.starred ? '已收藏' : '收藏'}</span>
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}

      {/* 移动端浮动录题按钮 (FAB) */}
      <Link
        to="/new"
        className="fixed bottom-6 right-6 z-40 flex size-14 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-lg transition-transform active:scale-95 md:hidden"
        aria-label="录入新题"
      >
        <Plus weight="bold" className="size-6" />
      </Link>

      {/* 批量删除确认 */}
      {/* 批量分析确认（用户裁决 4）：混选三选 / 全已分析二选 */}
      {batchConfirm && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm">
          <div className="w-full max-w-md rounded-lg border border-border bg-card p-6 shadow-xl space-y-4">
            <h3 className="text-base font-bold text-foreground">批量分析确认</h3>
            {batchConfirm.keep > 0 ? (
              <p className="text-sm text-muted-foreground">
                已选 <b className="font-mono">{batchConfirm.all + batchConfirm.keep}</b> 道题：
                <b className="text-foreground">{batchConfirm.keep}</b> 题未分析、
                <b className="text-foreground">{batchConfirm.all}</b> 题已分析。重新分析会新增评价记录并覆盖展示。
              </p>
            ) : (
              <p className="text-sm text-muted-foreground">
                选中的 <b className="font-mono">{batchConfirm.all}</b> 道题均已分析。
                重新分析将新增评价记录并覆盖现有展示，继续吗？
              </p>
            )}
            <div className="flex flex-wrap justify-end gap-2">
              {batchConfirm.keep > 0 && (
                <button
                  onClick={() => {
                    const total = batchConfirm.keep
                    setBatchConfirm(null)
                    runBatch('unanalyzed', total)
                  }}
                  className="rounded-md border border-border px-3 py-1.5 text-xs font-medium text-foreground hover:bg-muted"
                >
                  仅分析未分析的 {batchConfirm.keep} 题
                </button>
              )}
              <button
                onClick={() => {
                  const total = sel.size
                  setBatchConfirm(null)
                  runBatch('all', total)
                }}
                className="rounded-md bg-primary px-3 py-1.5 text-xs font-semibold text-primary-foreground hover:bg-primary/90"
              >
                重新分析全部（覆盖）
              </button>
              <button
                onClick={() => setBatchConfirm(null)}
                className="rounded-md border border-border px-3 py-1.5 text-xs font-medium text-muted-foreground hover:bg-muted"
              >
                取消
              </button>
            </div>
          </div>
        </div>
      )}
      {/* 标签清洗聚合建议弹窗（用户裁决 3） */}
      {cleanupOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm">
          <div className="flex max-h-[85vh] w-full max-w-2xl flex-col rounded-lg border border-border bg-card shadow-2xl">
            <div className="flex items-center justify-between border-b border-border px-5 py-4">
              <div>
                <h3 className="text-base font-bold text-foreground">🏷️ 标签聚合清洗建议</h3>
                <p className="text-xs text-muted-foreground mt-0.5">
                  AI 已对未建树的自由标签进行技术语义归组。请勾选您认可的聚合方案（规范名将保留，别名将被合并）：
                </p>
              </div>
              <button
                onClick={() => setCleanupOpen(false)}
                className="rounded-md p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
              >
                ✕
              </button>
            </div>

            <div className="flex-1 overflow-y-auto p-5 space-y-3">
              {cleanupProposals.map((group, idx) => {
                const checked = selectedGroupIdxs.has(idx)
                return (
                  <div
                    key={idx}
                    className={`rounded-lg p-3.5 transition-colors ${
                      checked
                        ? 'chip-selected'
                        : 'border border-border bg-muted/20 opacity-60'
                    }`}
                  >
                    <label className="flex items-start gap-3 cursor-pointer">
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(e) => {
                          setSelectedGroupIdxs((prev) => {
                            const next = new Set(prev)
                            if (e.target.checked) next.add(idx)
                            else next.delete(idx)
                            return next
                          })
                        }}
                        className="mt-1 size-4 accent-[var(--primary)]"
                      />
                      <div className="min-w-0 flex-1 space-y-1.5">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="rounded bg-primary/15 px-2 py-0.5 font-semibold text-xs text-primary">
                            保留规范名：{group.canonical}
                          </span>
                          {group.target_skill_name && (
                            <span className="rounded bg-success/15 text-success border border-success/30 px-1.5 py-0.5 font-medium text-xs">
                              ↳ 挂靠技能：{group.target_skill_name}
                            </span>
                          )}
                          <span className="text-xs text-muted-foreground">← 合并并吸纳：</span>
                          {group.aliases.map((alias) => (
                            <span
                              key={alias}
                              className="rounded bg-muted border border-border/80 px-1.5 py-0.5 text-xs line-through text-muted-foreground"
                            >
                              #{alias}
                            </span>
                          ))}
                        </div>
                        {group.note && (
                          <p className="text-xs text-muted-foreground leading-relaxed">
                            💡 {group.note}
                          </p>
                        )}
                      </div>
                    </label>
                  </div>
                )
              })}
            </div>

            <div className="flex items-center justify-between border-t border-border px-5 py-3.5 bg-muted/30">
              <span className="text-xs text-muted-foreground">
                已选中 <b className="font-mono text-primary font-bold">{selectedGroupIdxs.size}</b> / {cleanupProposals.length} 组聚合建议
              </span>
              <div className="flex items-center gap-2">
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => setCleanupOpen(false)}
                  disabled={cleaning}
                >
                  取消
                </Button>
                <Button
                  size="sm"
                  onClick={handleApplyTagCleanup}
                  disabled={cleaning || selectedGroupIdxs.size === 0}
                >
                  {cleaning ? '正在合并应用…' : '应用合并'}
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}

      <ConfirmDialog
        open={delOpen}
        onOpenChange={setDelOpen}
        destructive
        title={`删除选中的 ${sel.size} 道题？`}
        description="将级联删除其分析记录。"
        confirmLabel="删除"
        onConfirm={bulkDelete}
      />
    </div>
  )
}
