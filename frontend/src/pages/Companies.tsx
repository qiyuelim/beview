import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { CaretRight, Buildings, Plus } from '@phosphor-icons/react'
import { apiGet, apiPost } from '../api/client'
import type { CompanySummary } from '../api/types'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { EmptyState } from '../components/EmptyState'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

/** 面试 · 公司列表（ADR-0012 D4）：公司卡片 → 公司详情 → 岗位卡片 → 岗位详情 */
export default function Companies() {
  const [companies, setCompanies] = useState<CompanySummary[]>([])
  const [err, setErr] = useState('')
  const [name, setName] = useState('')
  const [creating, setCreating] = useState(false)

  async function load() {
    setCompanies(await apiGet('/api/companies'))
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
  }, [])

  async function create() {
    if (!name.trim()) return
    setErr('')
    try {
      await apiPost('/api/companies', { name: name.trim() })
      setName('')
      setCreating(false)
      await load()
    } catch (e: any) {
      setErr(e.message)
    }
  }

  return (
    <div>
      <PageHeader
        title="企业"
        meta={<span>公司与岗位</span>}
        actions={
          <Button onClick={() => setCreating((v) => !v)} className="h-10 min-h-[40px] px-4 font-medium">
            <Plus weight="bold" className="size-4" aria-hidden /> 新建公司
          </Button>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {creating && (
        <Section className="mb-3 rounded-xl">
          <form
            className="flex flex-wrap items-end gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              create()
            }}
          >
            <FormField label="公司名称" htmlFor="co-name" className="w-full sm:w-64">
              <Input
                id="co-name"
                placeholder="公司名称"
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="h-10 text-sm"
              />
            </FormField>
            <div className="flex items-center gap-2 pb-px">
              <Button type="submit" disabled={!name.trim()} className="h-10 px-4">
                创建
              </Button>
              <Button type="button" variant="ghost" onClick={() => setCreating(false)} className="h-10 px-4">
                取消
              </Button>
            </div>
          </form>
        </Section>
      )}

      {companies.length === 0 ? (
        <EmptyState
          icon={<Buildings className="size-10" />}
          title="还没有公司"
          hint="新建公司后即可挂岗位、记面试。"
          action={<Button onClick={() => setCreating(true)} className="h-11 px-5">新建公司</Button>}
        />
      ) : (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {companies.map((c) => (
            <Link
              to={`/companies/${c.id}`}
              key={c.id}
              className="surface-interactive group rounded-xl border border-border bg-card p-4"
              aria-label={`打开 ${c.name}`}
            >
              <div className="flex items-center gap-2.5">
                <span className="grid size-10 shrink-0 place-items-center rounded-lg bg-primary font-semibold text-primary-foreground text-sm">
                  {c.name.slice(0, 1)}
                </span>
                <span className="min-w-0 truncate text-base font-semibold text-foreground">{c.name}</span>
                <CaretRight className="ml-auto size-4 shrink-0 text-muted-foreground" aria-hidden />
              </div>
              <p className="mt-2 line-clamp-2 min-h-[2.5rem] text-xs leading-5 text-muted-foreground">
                {c.description || '未填写描述'}
              </p>
              <div className="mt-3 flex items-center gap-2 border-t border-border/40 pt-2.5">
                <span className="rounded-md bg-muted px-2.5 py-0.5 text-xs text-muted-foreground font-medium">
                  {c.position_count} 岗位
                </span>
                <span className="rounded-md bg-muted px-2.5 py-0.5 text-xs text-muted-foreground font-medium">
                  {c.application_count} 投递
                </span>
              </div>
            </Link>
          ))}
        </div>
      )}
    </div>
  )
}
