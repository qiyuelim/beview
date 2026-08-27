import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { CaretRight, MapPin, PencilSimple, Plus } from '@phosphor-icons/react'
import { apiGet, apiPatch, apiPost } from '../api/client'
import { APP_STATUS, QUESTION_TYPE_LABELS, type CompanySummary, type Position } from '../api/types'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { EmptyState } from '../components/EmptyState'
import { SemBadge, type BadgeSem } from '../components/SemBadge'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'

/** 部门聚合：有部门的一组，无部门的归「未分部门」；组内按创建时间倒序（接口已排） */
function groupByDepartment(positions: Position[]): Record<string, Position[]> {
  const groups: Record<string, Position[]> = {}
  for (const p of positions) {
    const key = p.department?.trim() || ''
    ;(groups[key] ??= []).push(p)
  }
  // 有部门的组在前，组名字典序；未分部门垫底
  return Object.fromEntries(
    Object.entries(groups).sort(([a], [b]) => {
      if (!a) return 1
      if (!b) return -1
      return a.localeCompare(b, 'zh')
    }),
  )
}

const STATUS_SEM: Record<string, BadgeSem> = {
  applied: 'neutral',
  callback: 'warn',
  interviewing: 'info',
  offer: 'pass',
  rejected: 'danger',
  withdrawn: 'neutral',
}

/** 公司详情（ADR-0012 D4）：描述 + 岗位卡片网格；点岗位卡进岗位详情 */
export default function CompanyDetail() {
  const { id } = useParams()
  const nav = useNavigate()
  const [company, setCompany] = useState<CompanySummary | null>(null)
  const [positions, setPositions] = useState<Position[]>([])
  // 票04：公司高频考点画像（服务端聚合）
  interface TopicNameCount { name: string; count: number }
  interface TopicProfile {
    total_questions: number
    top_tags: TopicNameCount[]
    top_skills: TopicNameCount[]
    type_distribution: { question_type: string | null; count: number }[]
  }
  const [topicProfile, setTopicProfile] = useState<TopicProfile | null>(null)
  const [err, setErr] = useState('')
  const [nameEditing, setNameEditing] = useState(false)
  const [companyName, setCompanyName] = useState('')
  const [descEditing, setDescEditing] = useState(false)
  const [desc, setDesc] = useState('')
  const [addOpen, setAddOpen] = useState(false)
  const [form, setForm] = useState({ title: '', department: '', location: '', jd_text: '' })

  async function load() {
    const d = await apiGet(`/api/companies/${id}`)
    setCompany(d.company)
    setPositions(d.positions ?? [])
    setCompanyName(d.company?.name ?? '')
    setDesc(d.company?.description ?? '')
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
    apiGet(`/api/companies/${id}/topic-profile`)
      .then((d: TopicProfile) => setTopicProfile(d))
      .catch(() => setTopicProfile(null)) // 画像失败不打断主数据（提示真实：区块自行降级）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id])

  async function saveName() {
    if (!companyName.trim()) return
    setErr('')
    try {
      await apiPatch(`/api/companies/${id}`, { name: companyName.trim() })
      setNameEditing(false)
      await load()
      toast.success('公司名称已更新')
    } catch (e: any) {
      setErr(e.message)
      toast.error(`修改失败: ${e.message}`)
    }
  }

  async function saveDesc() {
    setErr('')
    try {
      await apiPatch(`/api/companies/${id}`, { description: desc.trim() || null })
      setDescEditing(false)
      await load()
      toast.success('已保存')
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function createPosition() {
    setErr('')
    try {
      const p = await apiPost(`/api/companies/${id}/positions`, {
        title: form.title.trim(),
        department: form.department.trim() || null,
        location: form.location.trim() || null,
        jd_text: form.jd_text.trim() || null,
      })
      setAddOpen(false)
      setForm({ title: '', department: '', location: '', jd_text: '' })
      nav(`/positions/${p.id}`)
    } catch (e: any) {
      setErr(e.message)
    }
  }

  if (!company) {
    return <div className="py-24 text-center text-muted-foreground">{err || '加载中…'}</div>
  }

  return (
    <div>
      <nav aria-label="面包屑" className="mb-2 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link to="/companies" className="hover:text-primary">
          企业
        </Link>
        <span aria-hidden>/</span>
        <span className="text-foreground">{company.name}</span>
      </nav>

      <PageHeader
        title={
          nameEditing ? (
            <div className="flex items-center gap-2">
              <Input
                value={companyName}
                onChange={(e) => setCompanyName(e.target.value)}
                className="max-w-xs font-semibold h-10"
                placeholder="公司名称"
                autoFocus
              />
              <Button size="sm" onClick={saveName} className="h-10 px-4">
                保存
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  setNameEditing(false)
                  setCompanyName(company.name)
                }}
                className="h-10 px-4"
              >
                取消
              </Button>
            </div>
          ) : (
            <span className="flex items-center gap-2">
              <span>{company.name}</span>
              <button
                type="button"
                onClick={() => setNameEditing(true)}
                className="flex size-8 cursor-pointer items-center justify-center rounded-md text-foreground hover:bg-muted"
                title="重命名公司"
                aria-label="重命名公司"
              >
                <PencilSimple className="size-4" />
              </button>
            </span>
          )
        }
        meta={<span>{company.position_count} 个岗位 · {company.application_count} 次投递</span>}
        actions={
          <Button onClick={() => setAddOpen((v) => !v)} className="h-10 min-h-[40px] px-4 font-medium">
            <Plus weight="bold" className="size-4" aria-hidden /> 新增岗位
          </Button>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {/* 公司描述 */}
      <Section
        title="公司描述"
        action={
          !descEditing && (
            <Button size="sm" variant="ghost" onClick={() => setDescEditing(true)} className="h-9 px-3 text-xs">
              <PencilSimple className="size-3.5" aria-hidden /> 编辑描述
            </Button>
          )
        }
        className="rounded-xl"
      >
        {!descEditing ? (
          <p className="text-sm leading-6 text-foreground">
            {company.description || <span className="text-muted-foreground">暂无描述，点击右上角添加。</span>}
          </p>
        ) : (
          <>
            <Textarea
              rows={3}
              value={desc}
              onChange={(e) => setDesc(e.target.value)}
              placeholder="例如：云厂商，面试偏底层，两轮技术一轮 HR"
              aria-label="公司描述"
              className="text-base sm:text-sm"
            />
            <div className="mt-2.5 flex items-center gap-2">
              <Button size="sm" onClick={saveDesc} className="h-9 px-4">
                保存
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setDescEditing(false)} className="h-9 px-4">
                取消
              </Button>
            </div>
          </>
        )}
      </Section>

      {/* 票04：公司高频考点画像（服务端聚合） */}
      <Section title="高频考点画像" className="mt-4 rounded-xl">
        {topicProfile && topicProfile.total_questions > 0 ? (
          <div className="space-y-4">
            <p className="text-xs text-muted-foreground">
              基于该公司名下 {topicProfile.total_questions} 道题（含面试沉淀与岗位押题）的服务端聚合
            </p>
            {(
              [
                { label: '高频标签', items: topicProfile.top_tags },
                { label: '关联技能', items: topicProfile.top_skills },
              ] as const
            ).map(
              (group) =>
                group.items.length > 0 && (
                  <div key={group.label}>
                    <div className="mb-1.5 text-xs font-medium text-muted-foreground">{group.label}</div>
                    <div className="flex flex-wrap gap-1.5">
                      {group.items.map((t) => (
                        <span
                          key={t.name}
                          className="rounded-full border border-border bg-muted/60 px-2 py-0.5 text-xs text-foreground"
                        >
                          {t.name}
                          <span className="ml-1 font-mono font-semibold text-heading">{t.count}</span>
                        </span>
                      ))}
                    </div>
                  </div>
                ),
            )}
            {topicProfile.type_distribution.length > 0 && (
              <div>
                <div className="mb-1.5 text-xs font-medium text-muted-foreground">题型分布</div>
                <div className="flex flex-wrap gap-1.5">
                  {topicProfile.type_distribution.map((t) => (
                    <span
                      key={t.question_type ?? 'none'}
                      className="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground"
                    >
                      {QUESTION_TYPE_LABELS[t.question_type as keyof typeof QUESTION_TYPE_LABELS] ?? t.question_type ?? '未分类'}
                      <span className="ml-1 font-mono">{t.count}</span>
                    </span>
                  ))}
                </div>
              </div>
            )}
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">
            暂无该公司的题目数据——录入或押题沉淀后这里会展示考点分布。
          </p>
        )}
      </Section>

      {/* 新增岗位表单 */}
      {addOpen && (
        <Section title="新增岗位" className="mt-4 rounded-xl">
          <form
            className="grid grid-cols-1 gap-3 sm:grid-cols-2"
            onSubmit={(e) => {
              e.preventDefault()
              createPosition()
            }}
          >
            <FormField label="岗位名称" htmlFor="np-title">
              <Input
                id="np-title"
                placeholder="例如：后端工程师"
                value={form.title}
                onChange={(e) => setForm((f) => ({ ...f, title: e.target.value }))}
                className="h-10 text-sm"
              />
            </FormField>
            <FormField label="部门" htmlFor="np-dept" hint="可选">
              <Input
                id="np-dept"
                placeholder="例如：基础架构部"
                value={form.department}
                onChange={(e) => setForm((f) => ({ ...f, department: e.target.value }))}
                className="h-10 text-sm"
              />
            </FormField>
            <FormField label="工作地点" htmlFor="np-loc" hint="可选">
              <Input
                id="np-loc"
                placeholder="例如：杭州"
                value={form.location}
                onChange={(e) => setForm((f) => ({ ...f, location: e.target.value }))}
                className="h-10 text-sm"
              />
            </FormField>
            <FormField label="JD 原文" htmlFor="np-jd" hint="可选" className="sm:col-span-2">
              <Textarea
                id="np-jd"
                rows={4}
                placeholder="粘贴职位描述…"
                value={form.jd_text}
                onChange={(e) => setForm((f) => ({ ...f, jd_text: e.target.value }))}
                className="text-base sm:text-sm"
              />
            </FormField>
            <div className="flex items-center gap-2 sm:col-span-2 pt-1">
              <Button type="submit" disabled={!form.title.trim()} className="h-10 px-5">
                创建岗位
              </Button>
              <Button type="button" variant="ghost" onClick={() => setAddOpen(false)} className="h-10 px-4">
                取消
              </Button>
            </div>
          </form>
        </Section>
      )}

      {/* 岗位卡片——按部门聚合（反馈四#4：部门是岗位属性） */}
      {positions.length === 0 ? (
        <EmptyState
          className="mt-4"
          icon={<Plus className="size-10" />}
          title="还没有岗位"
          hint="岗位挂 JD 与投递；同一公司可有多个岗位。"
          action={<Button onClick={() => setAddOpen(true)} className="h-11 px-5">新增岗位</Button>}
        />
      ) : (
        Object.entries(groupByDepartment(positions)).map(([dept, list]) => (
          <section key={dept || '__none__'} className="mt-4" aria-label={dept ? `${dept} 的岗位` : '未分部门的岗位'}>
            <h3 className="mb-2.5 text-sm font-semibold text-foreground">
              {dept || '未分部门'}
              <span className="ml-1.5 font-mono text-xs font-normal text-muted-foreground">{list.length}</span>
            </h3>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
              {list.map((p) => (
                <Link
                  to={`/positions/${p.id}`}
                  key={p.id}
                  className="surface-interactive rounded-xl border border-border bg-card p-4"
                  aria-label={`打开岗位 ${p.title}`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <b className="truncate text-base font-semibold text-foreground">{p.title}</b>
                    <CaretRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                  </div>
                  {p.location && (
                    <span className="mt-1 flex items-center gap-1 text-xs text-muted-foreground">
                      <MapPin className="size-3.5" aria-hidden /> {p.location}
                    </span>
                  )}
                  <div className="mt-3 flex items-center gap-2 border-t border-border/40 pt-2.5">
                    <span className="font-mono text-xs text-muted-foreground font-medium">{p.application_count} 次投递</span>
                    {p.latest_status && (
                      <SemBadge sem={STATUS_SEM[p.latest_status] ?? 'neutral'} className="ml-auto">
                        最新：{APP_STATUS[p.latest_status]}
                      </SemBadge>
                    )}
                  </div>
                </Link>
              ))}
            </div>
          </section>
        ))
      )}
    </div>
  )
}
