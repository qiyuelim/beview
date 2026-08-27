import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { apiGet, apiPost } from '../api/client'
import type { WrongItem } from '../api/types'
import { Trophy } from '@phosphor-icons/react'
import { PageHeader } from '../components/PageHeader'
import { EmptyState } from '../components/EmptyState'
import { Button } from '@/components/ui/button'
import { toast } from 'sonner'

export default function ReviewWrong() {
  const [items, setItems] = useState<WrongItem[]>([])
  const [err, setErr] = useState('')

  async function load() {
    setItems(await apiGet('/api/review/wrong'))
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
  }, [])

  async function relearn(qid: number) {
    await apiPost(`/api/review/${qid}/relearn`)
    await load()
  }

  return (
    <div className="mx-auto w-full max-w-[800px]">
      <nav aria-label="面包屑" className="mb-2 flex items-center gap-1.5 text-sm text-muted-foreground">
        <Link to="/review" className="hover:text-primary">
          复习
        </Link>
        <span aria-hidden>/</span>
        <span className="text-foreground">错题本</span>
      </nav>

      <PageHeader
        title="错题本"
        meta={<span>自评忘了或判分低于 60 · 共 {items.length} 道</span>}
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {items.length === 0 ? (
        <EmptyState
          icon={<Trophy className="size-10 text-warning" />}
          title="没有错题，继续保持！"
          hint="自评忘了或判分低于 60 的题会进入错题本。"
        />
      ) : (
        <ul className="space-y-2">
          {items.map((x) => (
            <li key={x.question_id} className="rounded-lg border border-border bg-card p-3">
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <div className="text-sm font-medium leading-6">
                    <Link to={`/questions/${x.question_id}`} className="hover:text-primary">
                      {x.content}
                    </Link>
                    {x.source === 'ai_drill' && (
                      <span className="ml-1.5 rounded-full bg-secondary px-1.5 py-px text-xs text-secondary-foreground">AI 模拟</span>
                    )}
                  </div>
                  <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
                    <span>{x.company || '—'}</span>
                    <span>
                      复习 <b className="font-mono tabular-nums">{x.review_count}</b> 次
                    </span>
                    <span>
                      判分{' '}
                      <b className={`font-mono tabular-nums ${x.score != null && x.score < 60 ? 'text-destructive' : ''}`}>
                        {x.score ?? '—'}
                      </b>
                    </span>
                    {x.last_result === 'forgot' && (
                      <span className="rounded-full bg-destructive px-1.5 py-px text-xs text-white">忘了</span>
                    )}
                    {x.tags.slice(0, 3).map((t) => (
                      <span key={t} className="rounded bg-muted px-1.5 py-px text-xs">
                        #{t}
                      </span>
                    ))}
                  </div>
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  className="shrink-0"
                  onClick={() =>
                    relearn(x.question_id).catch((e) => toast.error(e.message))
                  }
                >
                  重练
                </Button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
