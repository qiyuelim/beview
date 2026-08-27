import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { PaperPlaneTilt, Plus, PushPin } from '@phosphor-icons/react'
import { apiGet, apiPost } from '../api/client'
import type { Application, ApplicationStatus, Position } from '../api/types'
import { APP_STATUS } from '../api/types'
import StageTimeline from '../components/StageTimeline'
import { FunnelCard } from '../components/FunnelCard'
import { PageHeader } from '../components/PageHeader'
import { SemBadge, type BadgeSem } from '../components/SemBadge'
import { EmptyState } from '../components/EmptyState'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { toast } from 'sonner'

const COLUMNS: ApplicationStatus[] = ['applied', 'interviewing', 'offer', 'rejected', 'withdrawn']

// 状态语义（词表对齐）：绿=通过/Offer、琥珀=待定、红=失败、蓝=信息、灰=中性
const STATUS_SEM: Record<ApplicationStatus, BadgeSem> = {
  applied: 'neutral',
  interviewing: 'info',
  offer: 'pass',
  rejected: 'danger',
  withdrawn: 'neutral',
}
const STATUS_DOT: Record<ApplicationStatus, string> = {
  applied: 'bg-muted-foreground',
  interviewing: 'bg-info',
  offer: 'bg-success',
  rejected: 'bg-destructive',
  withdrawn: 'bg-border-strong',
}

type BatchAction = 'rejected' | 'withdrawn' | 'delete'
const BATCH_LABEL: Record<BatchAction, string> = {
  rejected: '标记未通过',
  withdrawn: '放弃投递',
  delete: '删除投递',
}

export default function Applications() {
  const [apps, setApps] = useState<Application[]>([])
  const [companies, setCompanies] = useState<{ id: number; name: string }[]>([])
  const [err, setErr] = useState('')
  const [creating, setCreating] = useState(false)
  const [mobileTab, setMobileTab] = useState<'all' | ApplicationStatus>('all')

  async function load() {
    const [a, c] = await Promise.all([apiGet('/api/applications'), apiGet('/api/companies')])
    setApps(a)
    setCompanies(c)
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
  }, [])

  // 反馈 #5：看板纯展示。状态由进展推导（首场面试自动进行中；Offer/未过在详情页确认流；
  // 放弃/批量操作在下方「全部投递」管理区），卡片不提供状态下拉。
  const byStatus = (s: ApplicationStatus) => apps.filter((a) => a.status === s)
  const mobileList = mobileTab === 'all' ? apps : byStatus(mobileTab)

  // ---- 全部投递管理列表（ADR-0014 §9） ----
  const [manageOpen, setManageOpen] = useState(false)
  const [checked, setChecked] = useState<Set<number>>(new Set())
  // 筛选草稿态：改动不立即生效，点「查询」确认才应用（与题库筛选同模式，反馈六#3）
  const [df, setDf] = useState({ company: '', status: '' })
  const [applied, setApplied] = useState({ company: '', status: '' })
  const [mBusy, setMBusy] = useState(false)
  // 批量操作确认（替代 window.confirm；ADR-0015 D2 ConfirmDialog）
  const [pending, setPending] = useState<BatchAction | null>(null)
  const filtered = apps.filter(
    (a) =>
      (!applied.status || a.status === applied.status) &&
      (!applied.company || String(a.company_id ?? '') === applied.company),
  )
  function toggleCheck(id: number) {
    setChecked((prev) => {
      const n = new Set(prev)
      if (n.has(id)) n.delete(id)
      else n.add(id)
      return n
    })
  }
  async function runBatch(action: BatchAction) {
    const ids = Array.from(checked)
    if (ids.length === 0) return
    setMBusy(true)
    try {
      if (action === 'delete') {
        await apiPost('/api/applications/batch-delete', { ids })
        toast.success(`已删除 ${ids.length} 条投递（题目已保留）`)
      } else {
        const r = await apiPost('/api/applications/batch-status', { ids, status: action })
        const okN = r.succeeded?.length ?? 0
        const failN = r.failed?.length ?? 0
        toast.success(`已${BATCH_LABEL[action]} ${okN} 条${failN ? `，跳过 ${failN} 条` : ''}`)
      }
      setChecked(new Set())
      setPending(null)
      await load()
    } catch (e: any) {
      toast.error(e.message)
    } finally {
      setMBusy(false)
    }
  }

  return (
    <div>
      <PageHeader
        title="投递看板"
        meta={<span>按面试进展更新状态</span>}
        actions={
          <Button onClick={() => setCreating(true)} className="h-10 min-h-[40px] px-4 font-medium">
            <Plus weight="bold" className="size-4" aria-hidden /> 新建投递
          </Button>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      <CreateApplicationModal
        open={creating}
        companies={companies}
        onClose={() => setCreating(false)}
        onCreated={async () => {
          setCreating(false)
          await load()
        }}
      />

      {apps.length === 0 ? (
        <EmptyState
          icon={<PaperPlaneTilt className="size-10" />}
          title="还没有投递"
          hint="从公司、岗位和 JD 开始记第一份投递。"
          action={<Button onClick={() => setCreating(true)} className="h-11 px-5">新建第一份投递</Button>}
        />
      ) : (
        <>
          {/* 移动端状态切换分段标签栏 (375px ~ 767px) */}
          <div className="mb-3 flex items-center gap-1.5 overflow-x-auto pb-1 md:hidden">
            <button
              type="button"
              onClick={() => setMobileTab('all')}
              className={`flex min-h-[38px] shrink-0 cursor-pointer items-center gap-1.5 rounded-full px-3.5 py-1.5 text-xs font-semibold transition-colors duration-150 ${
                mobileTab === 'all'
                  ? 'bg-primary text-primary-foreground'
                  : 'border border-border bg-card text-foreground hover:bg-muted'
              }`}
            >
              <span>全部</span>
              <span className="font-mono text-[11px] opacity-80">({apps.length})</span>
            </button>
            {COLUMNS.map((s) => {
              const count = byStatus(s).length
              const active = mobileTab === s
              return (
                <button
                  key={s}
                  type="button"
                  onClick={() => setMobileTab(s)}
                  className={`flex min-h-[38px] shrink-0 cursor-pointer items-center gap-1.5 rounded-full px-3.5 py-1.5 text-xs font-semibold transition-colors duration-150 ${
                    active
                      ? 'bg-primary text-primary-foreground'
                      : 'border border-border bg-card text-foreground hover:bg-muted'
                  }`}
                >
                  <span className={`size-1.5 rounded-full ${STATUS_DOT[s]}`} />
                  <span>{APP_STATUS[s]}</span>
                  <span className="font-mono text-[11px] opacity-80">({count})</span>
                </button>
              )
            })}
          </div>

          {/* 移动端卡片纵向流式列表 (md:hidden) */}
          <div className="space-y-3 md:hidden">
            {mobileList.length === 0 ? (
              <div className="rounded-xl border border-dashed border-border py-12 text-center text-xs text-muted-foreground">
                当前状态下暂无投递记录
              </div>
            ) : (
              mobileList.map((a) => (
                <Link
                  key={a.id}
                  to={`/applications/${a.id}`}
                  className="surface-interactive block rounded-xl border border-border bg-card p-4"
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0 flex-1">
                      <h3 className="truncate text-base font-semibold text-foreground">
                        {a.position || '未填岗位'}
                      </h3>
                      <div className="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
                        <span className="font-medium text-foreground">{a.company ?? '未关联公司'}</span>
                        {(a.department || a.location) && (
                          <span>· {[a.department, a.location].filter(Boolean).join(' / ')}</span>
                        )}
                      </div>
                    </div>
                    <SemBadge sem={STATUS_SEM[a.status] ?? 'neutral'}>
                      {APP_STATUS[a.status] ?? a.status}
                    </SemBadge>
                  </div>

                  {/* 进度节点步进器 */}
                  {(a.interview_stages?.length ?? 0) > 0 && (
                    <div className="mt-3 border-t border-border/60 pt-2.5">
                      <StageTimeline stages={a.interview_stages} status={a.status} compact />
                    </div>
                  )}

                  {a.note && (
                    <div className="mt-2.5 flex items-start gap-1.5 text-xs text-muted-foreground">
                      <PushPin className="mt-0.5 size-3.5 shrink-0 text-warning" aria-hidden />
                      <span className="line-clamp-2">{a.note}</span>
                    </div>
                  )}

                  <div className="mt-2.5 flex items-center justify-between text-[11px] text-muted-foreground border-t border-border/40 pt-2 font-mono tabular-nums">
                    <span>投递于 {a.applied_at.slice(0, 10)}</span>
                    {a.salary && <span className="font-semibold text-success">{a.salary}</span>}
                  </div>
                </Link>
              ))
            )}
          </div>

          {/* 桌面端/平板多列看板 (md:grid) */}
          <div className="hidden md:grid md:grid-cols-2 xl:grid-cols-5 gap-3">
            {COLUMNS.map((s) => {
              const list = byStatus(s)
              return (
                <section key={s} aria-label={APP_STATUS[s]}>
                  <div className="mb-2 flex items-center gap-1.5 px-0.5">
                    <span className={`size-2 rounded-full ${STATUS_DOT[s]}`} aria-hidden />
                    <span className="text-sm font-semibold text-foreground">{APP_STATUS[s]}</span>
                    <span className="ml-auto font-mono text-xs tabular-nums text-muted-foreground">
                      {list.length}
                    </span>
                  </div>
                  <div className="space-y-2">
                    {list.length === 0 && (
                      <div className="rounded-xl border border-dashed border-border py-6 text-center text-xs text-muted-foreground">
                        空
                      </div>
                    )}
                    {list.map((a) => (
                      <Link
                        key={a.id}
                        to={`/applications/${a.id}`}
                        className="surface-interactive block rounded-xl border border-border bg-card p-3"
                      >
                        <div className="flex items-baseline justify-between gap-2">
                          <span className="truncate text-sm font-semibold text-foreground">
                            {a.position || '未填岗位'}
                          </span>
                        </div>
                        <div className="mt-0.5 flex items-center justify-between gap-2">
                          <span className="min-w-0 truncate text-xs text-muted-foreground">
                            {a.company ?? '未关联公司'}
                          </span>
                          {s === 'offer' && a.salary && (
                            <SemBadge sem="pass" className="shrink-0">
                              {a.salary}
                            </SemBadge>
                          )}
                        </div>
                        {(a.department || a.channel || a.location) && (
                          <div className="mt-0.5 truncate text-xs text-muted-foreground">
                            {[a.department, a.channel, a.location].filter(Boolean).join(' · ')}
                          </div>
                        )}
                        {/* 进行中：统一节点时间线（compact：只展示轮次 ✓✗·，终态即所在列不重复） */}
                        {(a.interview_stages?.length ?? 0) > 0 && (
                          <div className="mt-1.5">
                            <StageTimeline stages={a.interview_stages} status={s} compact />
                          </div>
                        )}
                        {a.note && s !== 'interviewing' && (
                          <div className="mt-1.5 flex items-start gap-1 text-xs text-muted-foreground">
                            <PushPin className="mt-px size-3 shrink-0" aria-hidden />
                            <span className="line-clamp-2">{a.note}</span>
                          </div>
                        )}
                        <div className="mt-1.5 font-mono text-[11px] tabular-nums text-muted-foreground">
                          {a.applied_at.slice(0, 10)}
                        </div>
                      </Link>
                    ))}
                  </div>
                </section>
              )
            })}
          </div>
        </>
      )}

      {/* 求职漏斗（低频概览并入所属页） */}
      <FunnelCard className="mt-4" />

      {/* 全部投递 · 批量管理（ADR-0014 §9） */}
      <section className="mt-4 rounded-lg border border-border bg-card" aria-label="全部投递">
        <header className="flex items-center gap-2 border-b border-border px-3 py-2.5">
          <h2 className="text-sm font-semibold">全部投递（{filtered.length}）</h2>
          <Button
            size="sm"
            variant="outline"
            className="ml-auto"
            onClick={() => setManageOpen((v) => !v)}
          >
            {manageOpen ? '收起批量管理' : '批量管理'}
          </Button>
        </header>

        <form
          className="flex flex-col gap-2 border-b border-border px-3 py-2.5 md:flex-row md:flex-wrap md:items-end"
          onSubmit={(e) => {
            e.preventDefault()
            setApplied({ ...df })
          }}
        >
          <FormField label="公司" htmlFor="mf-company" className="w-full md:w-44">
            <select
              id="mf-company"
              value={df.company}
              onChange={(e) => setDf((f) => ({ ...f, company: e.target.value }))}
              className="h-9 w-full rounded-md border border-input bg-card px-2 text-sm"
            >
              <option value="">全部公司</option>
              {companies.map((c) => (
                <option key={c.id} value={String(c.id)}>
                  {c.name}
                </option>
              ))}
            </select>
          </FormField>
          <FormField label="状态" htmlFor="mf-status" className="w-full md:w-36">
            <select
              id="mf-status"
              value={df.status}
              onChange={(e) => setDf((f) => ({ ...f, status: e.target.value }))}
              className="h-9 w-full rounded-md border border-input bg-card px-2 text-sm"
            >
              <option value="">全部状态</option>
              {COLUMNS.map((c) => (
                <option key={c} value={c}>
                  {APP_STATUS[c]}
                </option>
              ))}
            </select>
          </FormField>
          <div className="ml-auto flex items-center gap-2">
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => {
                const empty = { company: '', status: '' }
                setDf(empty)
                setApplied(empty)
              }}
            >
              重置
            </Button>
            <Button type="submit" size="sm">
              查询
            </Button>
          </div>
        </form>

        <div className="p-3">
          {manageOpen && (
            <div
              className="mb-3 flex flex-wrap items-center gap-2 rounded-md bg-muted px-3 py-2"
              role="toolbar"
              aria-label="批量操作"
            >
              <span className="text-sm">
                已选 <b className="font-mono tabular-nums">{checked.size}</b> 条
              </span>
              <Button size="sm" variant="ghost" onClick={() => setChecked(new Set(filtered.map((a) => a.id)))}>
                全选本页
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setChecked(new Set())}>
                清除
              </Button>
              <div className="ml-auto flex flex-wrap items-center gap-2">
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={mBusy || checked.size === 0}
                  onClick={() => setPending('rejected')}
                >
                  批量未通过
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={mBusy || checked.size === 0}
                  onClick={() => setPending('withdrawn')}
                >
                  批量放弃
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={mBusy || checked.size === 0}
                  onClick={() => setPending('delete')}
                >
                  批量删除
                </Button>
              </div>
            </div>
          )}

          {apps.length === 0 ? (
            <p className="text-sm text-muted-foreground">还没有投递。</p>
          ) : filtered.length === 0 ? (
            <p className="text-sm text-muted-foreground">该筛选下暂无投递</p>
          ) : (
            <table className="w-full text-sm" aria-label="全部投递列表">
              <thead>
                <tr className="border-b border-border text-left text-xs text-muted-foreground">
                  {manageOpen && <th className="w-8 px-2 py-2" />}
                  <th className="px-2 py-2 font-medium">公司 · 岗位 / 部门</th>
                  <th className="px-2 py-2 font-medium">状态</th>
                  <th className="px-2 py-2 font-medium">投递日期</th>
                  <th className="px-2 py-2 font-medium">轮次</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((a) => (
                  <tr key={a.id} className="border-b border-border last:border-0 hover:bg-muted/50">
                    {manageOpen && (
                      <td className="px-2 py-2">
                        <input
                          type="checkbox"
                          checked={checked.has(a.id)}
                          onChange={() => toggleCheck(a.id)}
                          aria-label={`选择 ${a.company ?? ''} ${a.position ?? ''}`}
                          className="size-4 accent-[var(--primary)]"
                        />
                      </td>
                    )}
                    <td className="max-w-0 px-2 py-2">
                      <Link
                        to={`/applications/${a.id}`}
                        className="block truncate font-medium hover:text-primary"
                      >
                        {[a.company ?? '未关联公司', a.position].filter(Boolean).join(' · ')}
                        {a.department && (
                          <span className="font-normal text-muted-foreground"> / {a.department}</span>
                        )}
                      </Link>
                    </td>
                    <td className="px-2 py-2">
                      <SemBadge sem={STATUS_SEM[a.status]}>{APP_STATUS[a.status]}</SemBadge>
                    </td>
                    <td className="whitespace-nowrap px-2 py-2 font-mono text-xs tabular-nums text-muted-foreground">
                      {a.applied_at.slice(0, 10)}
                    </td>
                    <td className="px-2 py-2 font-mono text-xs tabular-nums text-muted-foreground">
                      {a.interview_stages?.length ?? 0}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </section>

      {/* 批量操作确认（终态会被服务端跳过；删除不可恢复、题目迁回收站） */}
      <ConfirmDialog
        open={pending !== null}
        onOpenChange={(v) => !v && setPending(null)}
        destructive
        busy={mBusy}
        title={`批量${BATCH_LABEL[pending ?? 'rejected']}`}
        description={
          pending === 'delete'
            ? `确认删除选中的 ${checked.size} 条投递？关联题目会保留并移入回收站，此操作不可恢复。`
            : `确认将选中的 ${checked.size} 条投递「${BATCH_LABEL[pending ?? 'rejected']}」？（终态投递会被跳过）`
        }
        confirmLabel={pending ? BATCH_LABEL[pending] : '确认'}
        onConfirm={() => pending && runBatch(pending)}
      />
    </div>
  )
}

/// 新建投递浮窗（反馈 #1：公司下拉 → 岗位下拉级联，无岗位则内联新建；同岗可重复投递）
function CreateApplicationModal({
  open,
  companies,
  onClose,
  onCreated,
}: {
  open: boolean
  companies: { id: number; name: string }[]
  onClose: () => void
  onCreated: () => void
}) {
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)
  const [companyId, setCompanyId] = useState<number | ''>('')
  const [companyName, setCompanyName] = useState('') // 无任何公司时的兜底输入
  const [positions, setPositions] = useState<Position[]>([])
  const [posLoading, setPosLoading] = useState(false)
  const [positionId, setPositionId] = useState<number | '__new__' | ''>('')
  const [np, setNp] = useState({ title: '', department: '', location: '', jd_text: '', salary: '' })
  const [channel, setChannel] = useState('')
  const [note, setNote] = useState('')

  // 选公司 -> 加载其岗位
  useEffect(() => {
    if (!open || !companyId) {
      setPositions([])
      return
    }
    setPosLoading(true)
    apiGet(`/api/companies/${companyId}/positions`)
      .then((rows: Position[]) => setPositions(rows))
      .catch(() => setPositions([]))
      .finally(() => setPosLoading(false))
  }, [open, companyId])

  useEffect(() => {
    if (open) {
      setErr('')
      setCompanyId(companies.length === 1 ? companies[0].id : '')
      setCompanyName('')
      setPositionId('')
      setNp({ title: '', department: '', location: '', jd_text: '', salary: '' })
      setChannel('')
      setNote('')
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  async function submit() {
    setErr('')
    setBusy(true)
    try {
      const body: any = {
        channel: channel.trim() || null,
        note: note.trim() || null,
      }
      if (companyId) {
        body.company_id = companyId
      } else {
        body.company_name = companyName.trim()
      }
      if (!companyId || positionId === '__new__') {
        body.position = np.title.trim()
        body.department = np.department.trim() || null
        body.location = np.location.trim() || null
        body.salary = np.salary.trim() || null
        body.jd_text = np.jd_text.trim() || null
      } else if (positionId !== '') {
        const p = positions.find((x) => x.id === positionId)
        body.position = p?.title ?? ''
        body.department = p?.department ?? null
      }
      await apiPost('/api/applications', body)
      onCreated()
    } catch (e: any) {
      setErr(e.message)
    } finally {
      setBusy(false)
    }
  }

  function isFormValid(): boolean {
    if (busy) return false
    // 场景 1: 新公司 -> 必须填写公司名，岗位按新岗位处理，必须填写岗位名
    if (companyId === '') {
      return Boolean(companyName.trim() && np.title.trim())
    }
    // 场景 2: 已有公司 + 录入新岗位 -> 必须填写岗位名
    if (positionId === '__new__') {
      return Boolean(np.title.trim())
    }
    // 场景 3: 已有公司 + 已有岗位 -> 必须选择岗位
    return positionId !== ''
  }

  const formInvalid = !isFormValid()

  // Radix Dialog 关闭态不渲染 DOM（等价旧 `if (!open) return null` 门卫，A组 #1 根因防御）
  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>新建投递</DialogTitle>
        </DialogHeader>
        {err && (
          <p role="alert" className="text-sm font-medium text-destructive">
            {err}
          </p>
        )}
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault()
            submit()
          }}
        >
          {/* 公司选择/录入 */}
          <FormField
            label="公司"
            required
            hint={companyId ? undefined : '输入新公司名称'}
          >
            <div className="space-y-2">
              <select
                className="w-full rounded-md border bg-background px-3 py-2 text-sm"
                value={companyId}
                onChange={(e) => {
                  const val = e.target.value
                  setCompanyId(val ? Number(val) : '')
                  setPositionId('')
                }}
              >
                <option value="">+ 输入新公司</option>
                {companies.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
              {!companyId && (
                <Input
                  placeholder="公司名称 *"
                  value={companyName}
                  onChange={(e) => setCompanyName(e.target.value)}
                  autoFocus
                />
              )}
            </div>
          </FormField>

          {/* 岗位选择/新建 */}
          {companyId && (
            <FormField label="岗位" required hint={posLoading ? '加载岗位中…' : undefined}>
              <select
                className="w-full rounded-md border bg-background px-3 py-2 text-sm"
                value={positionId}
                onChange={(e) => setPositionId(e.target.value === '__new__' ? '__new__' : (e.target.value ? Number(e.target.value) : ''))}
              >
                <option value="">选择已有岗位...</option>
                {positions.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.title}
                    {p.department ? ` · ${p.department}` : ''}
                  </option>
                ))}
                <option value="__new__">+ 录入新岗位...</option>
              </select>
            </FormField>
          )}

          {/* 新岗位字段（选了新公司或选了 + 录入新岗位 时展开） */}
          {(!companyId || positionId === '__new__') && (
            <div className="rounded-md border bg-muted/30 p-3 space-y-2.5">
              <p className="text-xs font-semibold text-muted-foreground">新岗位信息</p>
              <div className="grid grid-cols-2 gap-2">
                <Input
                  placeholder="岗位名称 *"
                  value={np.title}
                  onChange={(e) => setNp({ ...np, title: e.target.value })}
                />
                <Input
                  placeholder="部门（如：基础架构部）"
                  value={np.department}
                  onChange={(e) => setNp({ ...np, department: e.target.value })}
                />
              </div>
              <div className="grid grid-cols-2 gap-2">
                <Input
                  placeholder="工作地点（如：北京）"
                  value={np.location}
                  onChange={(e) => setNp({ ...np, location: e.target.value })}
                />
                <Input
                  placeholder="薪资范围（如：25k-40k）"
                  value={np.salary}
                  onChange={(e) => setNp({ ...np, salary: e.target.value })}
                />
              </div>
              <Textarea
                placeholder="粘贴 JD 描述（可选，将用于智能押题）..."
                rows={3}
                value={np.jd_text}
                onChange={(e) => setNp({ ...np, jd_text: e.target.value })}
              />
            </div>
          )}

          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <FormField label="渠道" htmlFor="ca-channel" hint="可选">
              <Input
                id="ca-channel"
                placeholder="内推 / 招聘网…"
                value={channel}
                onChange={(e) => setChannel(e.target.value)}
              />
            </FormField>
            <FormField label="备注 / 待跟进" htmlFor="ca-note" hint="可选">
              <Input
                id="ca-note"
                placeholder="例如：周五约面"
                value={note}
                onChange={(e) => setNote(e.target.value)}
              />
            </FormField>
          </div>

          <DialogFooter className="gap-2 sm:justify-between">
            <span className="text-xs text-muted-foreground">同一岗位可多次投递，互不干扰</span>
            <div className="flex items-center gap-2">
              <Button type="button" variant="outline" onClick={onClose} disabled={busy}>
                取消
              </Button>
              <Button type="submit" disabled={formInvalid}>
                <PaperPlaneTilt weight="fill" className="size-4" aria-hidden /> 投递
              </Button>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
