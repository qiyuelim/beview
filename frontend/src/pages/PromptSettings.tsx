import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { apiGet, apiPut } from '../api/client'
import { PageHeader } from '../components/PageHeader'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { SemBadge } from '../components/SemBadge'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { toast } from 'sonner'

interface PromptItem {
  key: string
  name: string
  description: string
  value: string
  is_custom: boolean
}

export default function PromptSettings() {
  const [prompts, setPrompts] = useState<PromptItem[]>([])
  const [loading, setLoading] = useState(true)
  const [editing, setEditing] = useState<PromptItem | null>(null)
  const [draft, setDraft] = useState('')
  const [saving, setSaving] = useState(false)

  async function loadPrompts(silent = false): Promise<PromptItem[]> {
    if (!silent) setLoading(true)
    try {
      const d = await apiGet('/api/settings/prompts')
      const items: PromptItem[] = d.prompts ?? []
      setPrompts(items)
      return items
    } catch (e: any) {
      toast.error(e.message || '加载提示词失败')
      return []
    } finally {
      if (!silent) setLoading(false)
    }
  }

  useEffect(() => {
    loadPrompts()
  }, [])

  function openEdit(p: PromptItem) {
    setEditing(p)
    setDraft(p.value)
  }

  async function save() {
    if (!editing) return
    setSaving(true)
    try {
      await apiPut('/api/settings/prompts', { key: editing.key, value: draft })
      await loadPrompts()
      toast.success(draft.trim() === '' ? '已恢复内置默认' : '已保存为自定义提示词')
      setEditing(null)
    } catch (e: any) {
      toast.error(e.message || '保存失败')
    } finally {
      setSaving(false)
    }
  }

  async function resetDefault() {
    if (!editing) return
    setSaving(true)
    try {
      await apiPut('/api/settings/prompts', { key: editing.key, value: '' })
      const items = await loadPrompts(true)
      const updated = items.find((p) => p.key === editing.key)
      if (updated) {
        setEditing(updated)
        setDraft(updated.value)
      }
      toast.success('已恢复内置默认')
    } catch (e: any) {
      toast.error(e.message || '恢复默认失败')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="mx-auto w-full">
      <PageHeader
        title="提示词管理"
        meta={<span className="font-mono">{prompts.length} 个 LLM 出口</span>}
        actions={
          <Link to="/settings">
            <Button variant="ghost" size="sm">返回设置</Button>
          </Link>
        }
      />

      <p className="mb-4 text-sm text-foreground">
        每个 AI 能力对应一份提示词，展示当前实际生效内容。
        JSON 输出格式约束需保留，改动会导致解析失败；可一键恢复内置默认。
      </p>

      {loading ? (
        <div className="py-20 text-center text-sm text-muted-foreground">加载中…</div>
      ) : (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {prompts.map((p) => (
            <button
              key={p.key}
              onClick={() => openEdit(p)}
              className="group flex flex-col rounded-xl border border-border bg-card p-4 text-left shadow-sm transition-all hover:border-primary/50 hover:shadow-md"
            >
              <div className="flex items-start justify-between gap-2">
                <h3 className="text-sm font-semibold text-foreground group-hover:text-primary">{p.name}</h3>
                <SemBadge sem={p.is_custom ? 'info' : 'neutral'}>{p.is_custom ? '自定义' : '内置'}</SemBadge>
              </div>
              <p className="mt-1.5 line-clamp-2 text-xs text-muted-foreground">{p.description}</p>
              <div className="mt-3 font-mono text-[10px] text-muted-foreground/70">{p.key}</div>
            </button>
          ))}
        </div>
      )}

      <Dialog open={editing !== null} onOpenChange={(v: boolean) => !v && setEditing(null)}>
        <DialogContent className="max-w-2xl max-h-[90vh] flex flex-col">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              {editing?.name}
              {editing && <SemBadge sem={editing.is_custom ? 'info' : 'neutral'}>{editing.is_custom ? '自定义' : '内置默认'}</SemBadge>}
            </DialogTitle>
            <DialogDescription>{editing?.description}</DialogDescription>
          </DialogHeader>

          <div className="mt-4 flex-1 overflow-hidden">
            <Textarea
              rows={20}
              className="h-full min-h-[300px] font-mono text-xs"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              aria-label="编辑提示词"
            />
          </div>

          <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-border pt-4">
            <Button onClick={save} disabled={saving}>
              {saving ? '保存中…' : '保存提示词'}
            </Button>
            <Button
              variant="ghost"
              onClick={resetDefault}
              disabled={!editing?.is_custom}
              title={!editing?.is_custom ? '已是内置默认' : '清空自定义，恢复内置默认'}
            >
              恢复默认
            </Button>
            <Button variant="secondary" onClick={() => setEditing(null)}>
              取消
            </Button>
            <span className="ml-auto font-mono text-[10px] text-muted-foreground">{editing?.key}</span>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  )
}
