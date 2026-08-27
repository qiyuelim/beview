import { useEffect, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { apiDelete, apiGet, apiPost, apiPut } from '../api/client'
import type { DrillView, InterviewerPersona } from '../api/types'
import { ChatsCircle, Plus, Trash, Users } from '@phosphor-icons/react'
import { PageHeader } from '../components/PageHeader'
import { EmptyState } from '../components/EmptyState'
import { FormField } from '../components/FormField'
import { SemBadge, type BadgeSem } from '../components/SemBadge'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Skeleton } from '@/components/ui/skeleton'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

const KIND_LABEL: Record<string, string> = { interview: '模拟面试' }
const STATUS_LABEL: Record<string, string> = { ongoing: '进行中', finished: '已完成', aborted: '已放弃' }
const STATUS_SEM: Record<string, BadgeSem> = { ongoing: 'info', finished: 'pass', aborted: 'neutral' }

export default function Drills() {
  const [items, setItems] = useState<DrillView[]>([])
  const [err, setErr] = useState('')
  const [loading, setLoading] = useState(true)
  const [delTarget, setDelTarget] = useState<DrillView | null>(null)
  // M5b：面试官人格网格——平铺响应式（2–4 列），内置在前/自定义在后，点击直达建场并锁定人格
  const [personas, setPersonas] = useState<InterviewerPersona[]>([])
  const [personaMgrOpen, setPersonaMgrOpen] = useState(false)
  const [editingPersona, setEditingPersona] = useState<Partial<InterviewerPersona> | null>(null)
  const [personaForm, setPersonaForm] = useState({
    name: '',
    title: '',
    persona_prompt: '',
    difficulty_hint: '',
    focus_tags: '',
    temperature_hint: '0.5',
  })
  const [personaErr, setPersonaErr] = useState('')
  const [delPersonaTarget, setDelPersonaTarget] = useState<InterviewerPersona | null>(null)
  const navigate = useNavigate()

  async function loadPersonas() {
    const resp = await apiGet('/api/personas')
    setPersonas((resp.items as InterviewerPersona[]) ?? [])
  }

  function openNewPersona() {
    setEditingPersona({})
    setPersonaErr('')
    setPersonaForm({ name: '', title: '', persona_prompt: '', difficulty_hint: '', focus_tags: '', temperature_hint: '0.5' })
  }

  function openEditPersona(p: InterviewerPersona) {
    setEditingPersona(p)
    setPersonaErr('')
    setPersonaForm({
      name: p.name,
      title: p.title || '',
      persona_prompt: p.persona_prompt,
      difficulty_hint: p.difficulty_hint || '',
      focus_tags: p.focus_tags.join(', '),
      temperature_hint: p.temperature_hint != null ? String(p.temperature_hint) : '0.5',
    })
  }

  async function savePersona() {
    if (!personaForm.name.trim() || !personaForm.persona_prompt.trim()) return
    const rawTemp = personaForm.temperature_hint.trim()
    const temp = rawTemp === '' ? null : Number(rawTemp)
    if (temp != null && (!Number.isFinite(temp) || temp < 0.3 || temp > 0.9)) {
      setPersonaErr('采样温度须在 0.3–0.9 之间')
      return
    }
    setPersonaErr('')
    const payload = {
      name: personaForm.name.trim(),
      title: personaForm.title.trim() || null,
      persona_prompt: personaForm.persona_prompt,
      difficulty_hint: personaForm.difficulty_hint.trim() || null,
      temperature_hint: temp,
      focus_tags: personaForm.focus_tags.split(',').map((t) => t.trim()).filter(Boolean),
    }
    if (editingPersona?.id) {
      await apiPut(`/api/personas/${editingPersona.id}`, payload)
    } else {
      await apiPost('/api/personas', payload)
    }
    await loadPersonas()
    setEditingPersona(null)
  }

  async function delPersona() {
    if (!delPersonaTarget) return
    await apiDelete(`/api/personas/${delPersonaTarget.id}`)
    await loadPersonas()
    setDelPersonaTarget(null)
  }

  useEffect(() => {
    Promise.all([apiGet('/api/drills'), apiGet('/api/personas')])
      .then(([drillItems, personaResp]) => {
        setItems(drillItems)
        setPersonas((personaResp.items as InterviewerPersona[]) ?? [])
      })
      .catch((e) => setErr(e.message))
      .finally(() => setLoading(false))
  }, [])

  async function del() {
    if (!delTarget) return
    await apiDelete(`/api/drills/${delTarget.id}`)
    setItems((prev) => prev.filter((x) => x.id !== delTarget.id))
    setDelTarget(null)
  }

  if (loading) {
    return (
      <div>
        <PageHeader title="陪练" meta={<span>共 … 场</span>} actions={<Button asChild><Link to="/drills/new"><Plus weight="bold" className="size-4" aria-hidden /> 新建陪练</Link></Button>} />
        <div className="space-y-2">
          {Array.from({ length: 3 }).map((_, i) => (
            <Skeleton key={i} className="h-16 w-full" />
          ))}
        </div>
      </div>
    )
  }

  return (
    <div>
      <PageHeader
        title="陪练"
        meta={<span>共 {items.length} 场</span>}
        actions={
          <Button asChild>
            <Link to="/drills/new">
              <Plus weight="bold" className="size-4" aria-hidden /> 新建陪练
            </Link>
          </Button>
        }
      />

      {/* M5b：面试官人格网格——平铺响应式（2–4 列），内置在前/自定义在后，点击直达建场 */}
      {personas.length > 0 && (
        <section aria-label="面试官人格" className="mb-6">
          <div className="mb-2 flex items-center justify-between">
            <h2 className="text-sm font-semibold text-foreground">选择你的面试官</h2>
            <div className="flex items-center gap-2">
              <span className="text-xs text-muted-foreground">点击卡片直达建场 · 选人即选侧重</span>
              <Button size="sm" variant="ghost" onClick={() => setPersonaMgrOpen(true)} className="h-7 px-2 text-xs">
                <Users className="size-3.5" aria-hidden /> 管理
              </Button>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-4">
            {personas.map((p) => (
              <button
                key={p.id}
                type="button"
                onClick={() => navigate(`/drills/new?persona=${p.id}`)}
                className={`surface-interactive group flex h-full min-h-[120px] flex-col rounded-xl border p-3 text-left ${
                  p.builtin ? 'border-border bg-card' : 'border-border-strong bg-card'
                }`}
              >
                <div className="flex items-start justify-between gap-1">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-semibold text-foreground">{p.name}</div>
                    {p.title && <div className="mt-0.5 truncate text-xs text-muted-foreground">{p.title}</div>}
                  </div>
                  <span
                    className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-bold ${
                      p.builtin ? 'bg-secondary text-secondary-foreground' : 'bg-primary/10 text-primary'
                    }`}
                  >
                    {p.builtin ? '内置' : '自定义'}
                  </span>
                </div>
                {p.difficulty_hint && (
                  <div className="mt-2 line-clamp-2 text-xs leading-5 text-muted-foreground">{p.difficulty_hint}</div>
                )}
                {p.focus_tags.length > 0 && (
                  <div className="mt-auto flex flex-wrap gap-1 pt-2">
                    {p.focus_tags.slice(0, 3).map((t) => (
                      <span key={t} className="rounded bg-accent/15 px-1.5 py-0.5 text-[10px] text-accent">
                        {t}
                      </span>
                    ))}
                  </div>
                )}
              </button>
            ))}
          </div>
        </section>
      )}
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {items.length === 0 ? (
        <EmptyState
          icon={<ChatsCircle className="size-11" />}
          title="还没有陪练场次"
          hint="开一场模拟面试：逐题追问，当场判分。"
          action={
            <Button asChild>
              <Link to="/drills/new">
                <Plus weight="bold" className="size-4" aria-hidden /> 新建陪练
              </Link>
            </Button>
          }
        />
      ) : (
        <ul className="space-y-2">
          {items.map((d) => {
            const kindLabel = KIND_LABEL[d.kind] || d.kind
            const showKind = d.title !== kindLabel
            return (
              <li key={d.id} className="surface-interactive rounded-xl border border-border bg-card p-4">
                <div className="flex items-start gap-3">
                  <Link to={`/drills/${d.id}`} className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="text-sm font-semibold">{d.title}</span>
                      {showKind && (
                        <span className="rounded-full bg-muted px-1.5 py-px text-xs text-muted-foreground">{kindLabel}</span>
                      )}
                      <SemBadge sem={STATUS_SEM[d.status] ?? 'neutral'}>
                        {STATUS_LABEL[d.status] || d.status}
                      </SemBadge>
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
                      {d.position && (
                        <span>
                          岗位 <b>{d.position}</b>
                        </span>
                      )}
                      {d.direction && (
                        <span>
                          方向 <b>{d.direction}</b>
                        </span>
                      )}
                      <span>
                        题目 <b className="font-mono tabular-nums">{d.question_count}</b>
                      </span>
                      <span>
                        消息 <b className="font-mono tabular-nums">{d.message_count}</b>
                      </span>
                      {d.score != null && (
                        <span>
                          总分 <b className="font-mono tabular-nums">{d.score}</b>
                        </span>
                      )}
                      <span className="font-mono">{new Date(d.started_at).toLocaleString()}</span>
                    </div>
                  </Link>
                  <Button
                    size="icon"
                    variant="ghost"
                    className="size-8 shrink-0 text-muted-foreground hover:text-destructive"
                    onClick={() => setDelTarget(d)}
                    aria-label="删除陪练"
                    title="删除陪练"
                  >
                    <Trash className="size-4" aria-hidden />
                  </Button>
                </div>
              </li>
            )
          })}
        </ul>
      )}

      <ConfirmDialog
        open={delTarget !== null}
        onOpenChange={(v) => !v && setDelTarget(null)}
        destructive
        title={`删除陪练「${delTarget?.title ?? ''}」？`}
        description="其对话消息将删除，已沉淀的题目保留。"
        confirmLabel="删除"
        onConfirm={del}
      />

      {/* 面试官管理模态框 */}
      <Dialog open={personaMgrOpen} onOpenChange={(v: boolean) => { setPersonaMgrOpen(v); if (!v) setEditingPersona(null) }}>
        <DialogContent className="max-w-3xl max-h-[85vh] flex flex-col">
          <DialogHeader>
            <DialogTitle className="flex items-center justify-between">
              <span>面试官管理</span>
              {!editingPersona && (
                <Button size="sm" onClick={openNewPersona}>
                  <Plus className="size-3.5" aria-hidden /> 新增自定义面试官
                </Button>
              )}
            </DialogTitle>
            <DialogDescription>内置面试官不可编辑；自定义面试官支持新增、编辑、删除（软删除，历史场次仍显示）。</DialogDescription>
          </DialogHeader>

          <div className="flex-1 overflow-y-auto">
            {editingPersona ? (
              <div className="space-y-4 py-2">
                {personaErr && (
                  <p role="alert" className="rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
                    {personaErr}
                  </p>
                )}
                <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                  <FormField label="名称" htmlFor="persona-name" required>
                    <Input
                      id="persona-name"
                      value={personaForm.name}
                      onChange={(e) => setPersonaForm({ ...personaForm, name: e.target.value })}
                      placeholder="如：温和技术导师"
                    />
                  </FormField>
                  <FormField label="头衔" htmlFor="persona-title">
                    <Input
                      id="persona-title"
                      value={personaForm.title}
                      onChange={(e) => setPersonaForm({ ...personaForm, title: e.target.value })}
                      placeholder="如：资深前端工程师"
                    />
                  </FormField>
                </div>
                <FormField label="人格提示词" htmlFor="persona-prompt" required>
                  <Textarea
                    id="persona-prompt"
                    rows={6}
                    className="font-mono text-xs"
                    value={personaForm.persona_prompt}
                    onChange={(e) => setPersonaForm({ ...personaForm, persona_prompt: e.target.value })}
                    placeholder="描述这位面试官的性格、提问风格、关注点..."
                  />
                </FormField>
                <FormField label="难度说明" htmlFor="persona-diff">
                  <Input
                    id="persona-diff"
                    value={personaForm.difficulty_hint}
                    onChange={(e) => setPersonaForm({ ...personaForm, difficulty_hint: e.target.value })}
                    placeholder="如：注重基础 · 循序渐进"
                  />
                </FormField>
                <FormField label="侧重标签" htmlFor="persona-tags" hint="逗号分隔">
                  <Input
                    id="persona-tags"
                    value={personaForm.focus_tags}
                    onChange={(e) => setPersonaForm({ ...personaForm, focus_tags: e.target.value })}
                    placeholder="如：系统设计, 数据库, 缓存"
                  />
                </FormField>
                <FormField
                  label="采样温度"
                  htmlFor="persona-temp"
                  required
                  error={personaErr.includes('温度') ? personaErr : undefined}
                  hint="0.3–0.9，越低越严谨、越高越发散"
                >
                  <Input
                    id="persona-temp"
                    type="number"
                    min={0.3}
                    max={0.9}
                    step={0.05}
                    value={personaForm.temperature_hint}
                    onChange={(e) => setPersonaForm({ ...personaForm, temperature_hint: e.target.value })}
                  />
                </FormField>
                <div className="flex gap-2 pt-2">
                  <Button onClick={savePersona} disabled={!personaForm.name.trim() || !personaForm.persona_prompt.trim()}>保存</Button>
                  <Button variant="secondary" onClick={() => setEditingPersona(null)}>取消</Button>
                </div>
              </div>
            ) : (
              <div className="space-y-2 py-2">
                {personas.map((p) => (
                  <div key={p.id} className="flex items-start justify-between gap-3 rounded-lg border border-border p-3">
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-semibold text-foreground">{p.name}</span>
                        {p.title && <span className="text-xs text-muted-foreground">· {p.title}</span>}
                        <SemBadge sem={p.builtin ? 'neutral' : 'info'}>{p.builtin ? '内置' : '自定义'}</SemBadge>
                      </div>
                      {p.difficulty_hint && <p className="mt-1 text-xs text-muted-foreground">{p.difficulty_hint}</p>}
                      {p.focus_tags.length > 0 && (
                        <div className="mt-1.5 flex flex-wrap gap-1">
                          {p.focus_tags.map((t) => (
                            <span key={t} className="rounded bg-accent/15 px-1.5 py-0.5 text-[10px] text-accent">{t}</span>
                          ))}
                        </div>
                      )}
                    </div>
                    {!p.builtin && (
                      <div className="flex shrink-0 gap-1">
                        <Button size="sm" variant="ghost" className="h-7 px-2 text-xs" onClick={() => openEditPersona(p)}>编辑</Button>
                        <Button size="sm" variant="ghost" className="h-7 px-2 text-xs text-destructive hover:text-destructive" onClick={() => setDelPersonaTarget(p)}>删除</Button>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={delPersonaTarget !== null}
        onOpenChange={(v) => !v && setDelPersonaTarget(null)}
        destructive
        title={`删除面试官「${delPersonaTarget?.name ?? ''}」？`}
        description="历史场次会显示「已删除的面试官」。新建陪练从当前列表选择。"
        confirmLabel="删除"
        onConfirm={delPersona}
      />
    </div>
  )
}
