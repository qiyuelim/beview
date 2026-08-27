import { useState } from 'react'
import { Check, Sparkle, Lightbulb } from '@phosphor-icons/react'
import { apiPost } from '../api/client'
import { SemBadge } from './SemBadge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

/**
 * 票06（ADR-0021）：AI 简历优化变更集面板。
 * 三段流：意图输入 → diff 卡片逐条采纳/整批应用 → 应用结果。
 * 快照兜底由后端在应用前自动生成（ADR-0021 D2），前端只提示不重复实现。
 */

interface Change {
  action: 'update' | 'add' | 'remove' | string
  module: string
  old_value?: unknown
  new_value?: unknown
  reason?: string
}

interface Proposal {
  summary: string
  changes: Change[]
}

interface RejectedOp {
  index: number
  action: string
  module: string
  reason: string
}

/** 模块名 → 中文展示名（白名单，ADR-0021） */
const MODULE_LABELS: Record<string, string> = {
  name: '姓名', summary: '个人简介', gender: '性别', age: '年龄', phone: '电话',
  email: '邮箱', city: '城市', years: '工作年限', political: '政治面貌',
  intent_position: '期望职位', intent_city: '期望城市', intent_salary: '期望薪资',
  education: '教育经历', experience: '工作经历', projects: '项目经历',
  skills: '技能特长', certificates: '证书荣誉', self_evaluation: '自我评价', links: '链接',
}

const ACTION_META: Record<string, { label: string; sem: 'info' | 'pass' | 'danger' }> = {
  update: { label: '修改', sem: 'info' },
  add: { label: '新增', sem: 'pass' },
  remove: { label: '移除', sem: 'danger' },
}

/** 值渲染：字符串直显；对象按「键：值」行展开（紧凑可读，不做 JSON 原样倾倒） */
function ValueView({ v, tone }: { v: unknown; tone: 'old' | 'new' | 'anchor' }) {
  if (v === null || v === undefined || v === '') return <span className="text-muted-foreground">—</span>
  let lines: string[]
  if (typeof v === 'string') {
    lines = [v]
  } else if (Array.isArray(v)) {
    lines = v.map((x) => (typeof x === 'string' ? x : JSON.stringify(x)))
  } else if (typeof v === 'object') {
    const obj = v as Record<string, unknown>
    lines = Object.entries(obj).map(([k, val]) => `${k}：${String(val)}`)
  } else {
    lines = [String(v)]
  }
  const cls =
    tone === 'old' ? 'text-muted-foreground line-through' : tone === 'anchor' ? 'text-destructive/80' : 'text-foreground'
  return (
    <span className={`block text-xs leading-5 ${cls}`}>
      {lines.map((l, i) => (
        <span key={i} className="block break-all">{l}</span>
      ))}
    </span>
  )
}

export default function ResumeOptimizeDialog({
  onClose,
  onApplied,
}: {
  onClose: () => void
  /** 应用成功后由父级刷新工作副本与留档列表 */
  onApplied: () => void
}) {
  const [phase, setPhase] = useState<'intent' | 'review' | 'done'>('intent')
  const [intent, setIntent] = useState('')
  const [proposal, setProposal] = useState<Proposal | null>(null)
  const [accepted, setAccepted] = useState<boolean[]>([])
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState('')
  const [result, setResult] = useState<{ applied: number; rejected: RejectedOp[] } | null>(null)

  async function propose() {
    setBusy(true)
    setErr('')
    try {
      const r = await apiPost('/api/resume/optimize/propose', { intent: intent.trim() || null })
      setProposal(r as Proposal)
      setAccepted((r as Proposal).changes.map(() => true))
      setPhase('review')
    } catch (e: any) {
      setErr(e.message || '生成变更集失败')
    } finally {
      setBusy(false)
    }
  }

  async function applyAccepted() {
    if (!proposal) return
    const subset = proposal.changes.filter((_, i) => accepted[i])
    if (subset.length === 0) return
    setBusy(true)
    setErr('')
    try {
      // 提交时携带完整操作对象，保证旧值断言 verbatim 到达服务端重校验
      const changes = proposal.changes.filter((_, i) => accepted[i])
      const r = await apiPost('/api/resume/optimize/apply', { changes })
      setResult(r as { applied: number; rejected: RejectedOp[] })
      setPhase('done')
      onApplied()
    } catch (e: any) {
      setErr(e.message || '应用失败')
    } finally {
      setBusy(false)
    }
  }

  const acceptedCount = accepted.filter(Boolean).length

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) onClose() }}>
      <DialogContent className="flex max-h-[85vh] sm:max-w-2xl flex-col p-6">
        {/* 标题栏 */}
        <DialogHeader className="border-b border-border pb-3">
          <DialogTitle className="flex items-center gap-2 text-base font-bold text-foreground">
            <Sparkle weight="fill" className="size-5 text-primary shrink-0" />
            <span>AI 优化简历 · 变更集</span>
          </DialogTitle>
        </DialogHeader>

        <div className="mt-4 flex-1 space-y-3 overflow-y-auto px-1">
          {err && (
            <p role="alert" className="rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
              {err}
            </p>
          )}

          {phase === 'intent' && (
            <>
              <p className="text-xs leading-5 text-muted-foreground">
                描述你的优化意图（可选），AI 将产出一份结构化变更集——每条变更独立展示旧值与新值，
                由你逐条采纳后才写入工作副本。应用前系统会自动留存「变更前快照」作为兜底。
              </p>
              <textarea
                rows={4}
                className="w-full rounded-md border border-border bg-background p-3 text-sm text-foreground placeholder:text-muted-foreground"
                placeholder="例如：突出项目成果、针对后端岗位精简自我评价…"
                value={intent}
                onChange={(e) => setIntent(e.target.value)}
              />
            </>
          )}

          {phase === 'review' && proposal && (
            <>
              <div className="rounded-lg border border-info/30 bg-info/5 p-3 text-xs leading-5 text-foreground">
                {proposal.summary}
              </div>
              <div className="space-y-2">
                {proposal.changes.map((ch, i) => (
                  <div
                    key={i}
                    className={`rounded-lg border p-3 transition-colors ${
                      accepted[i] ? 'border-border bg-background' : 'border-border bg-muted/30 opacity-60'
                    }`}
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex items-center gap-1.5">
                        <SemBadge sem={ACTION_META[ch.action]?.sem ?? 'neutral'}>
                          {ACTION_META[ch.action]?.label ?? ch.action}
                        </SemBadge>
                        <span className="text-xs font-semibold text-heading">
                          {MODULE_LABELS[ch.module] ?? ch.module}
                        </span>
                      </div>
                      <label className="flex cursor-pointer select-none items-center gap-1.5 text-xs font-medium text-muted-foreground">
                        <input
                          type="checkbox"
                          checked={accepted[i]}
                          onChange={(e) =>
                            setAccepted((prev) => prev.map((v, j) => (j === i ? e.target.checked : v)))
                          }
                          className="size-4 accent-[var(--accent)]"
                        />
                        采纳
                      </label>
                    </div>
                    <div className="mt-2 grid gap-1.5">
                      {(ch.action === 'update' || ch.action === 'remove') && (
                        <ValueView v={ch.old_value} tone={ch.action === 'update' ? 'old' : 'anchor'} />
                      )}
                      {(ch.action === 'update' || ch.action === 'add') && <ValueView v={ch.new_value} tone="new" />}
                    </div>
                    {ch.reason && (
                      <div className="mt-1.5 flex items-center text-[10px] text-muted-foreground">
                        <Lightbulb className="size-3.5 inline mr-1 text-primary shrink-0" />
                        <span>{ch.reason}</span>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </>
          )}

          {phase === 'done' && result && (
            <div className="space-y-2">
              <div className="flex items-center gap-2 rounded-lg border border-success/30 bg-success/5 p-3 text-sm font-medium text-foreground">
                <Check weight="bold" className="size-4 text-success" />
                已应用 {result.applied} 条变更，工作副本已更新并自动留存快照
              </div>
              {result.rejected.length > 0 && (
                <div className="rounded-lg border border-warning/40 bg-warning/10 p-3">
                  <div className="text-xs font-semibold text-foreground">
                    {result.rejected.length} 条未通过服务端校验（未落库）
                  </div>
                  <ul className="mt-1.5 space-y-1">
                    {result.rejected.map((r) => (
                      <li key={r.index} className="text-xs text-muted-foreground">
                        #{r.index + 1} {MODULE_LABELS[r.module] ?? r.module}：{r.reason}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          )}
        </div>

        {/* 底部操作区 */}
        <div className="mt-4 flex flex-wrap items-center justify-end gap-2 border-t border-border pt-3">
          {phase === 'intent' && (
            <Button onClick={propose} disabled={busy}>
              {busy ? '正在生成变更集…' : '生成变更集'}
            </Button>
          )}
          {phase === 'review' && proposal && (
            <>
              <Button
                variant="ghost"
                size="sm"
                disabled={busy}
                onClick={() => setAccepted(proposal.changes.map(() => false))}
              >
                全不选
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => setAccepted(proposal.changes.map(() => true))}
              >
                全选
              </Button>
              <Button onClick={applyAccepted} disabled={busy || acceptedCount === 0}>
                {busy ? '应用中…' : `应用已采纳的 ${acceptedCount} 条`}
              </Button>
            </>
          )}
          {phase === 'done' && (
            <Button onClick={onClose}>完成</Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
