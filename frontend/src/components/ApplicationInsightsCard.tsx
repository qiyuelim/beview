import { useCallback, useEffect, useState } from 'react'
import { apiGet } from '../api/client'
import { onJobDone, startAiJob, trackRunning, useAiJobs, isRunning } from '../ai/jobs'
import { Button } from '@/components/ui/button'
import { Sparkle } from '@phosphor-icons/react'
import { Section } from './Section'

/**
 * 票07：投递全局智能洞察卡。
 * - 数据：GET /api/applications/insights（最新一次四段报告 + running 态恢复通道）
 * - 触发：startAiJob('app_insights', 0)（后端同键幂等去重，ADR-0013 D2）
 * - 完成：onJobDone 回调重拉数据；空态给引导文案（提示真实），绝不渲染空报告
 */

interface Insight {
  created_at: string
  summary: string
  observations: string[]
  recommendations: string[]
  priority: { action: string; reason: string }[]
}

const KIND = 'app_insights' as const

export default function ApplicationInsightsCard() {
  const [insight, setInsight] = useState<Insight | null>(null)
  const [loading, setLoading] = useState(true)
  const [err, setErr] = useState('')
  const [expanded, setExpanded] = useState(false)
  const aiJobs = useAiJobs()
  const generating = isRunning(aiJobs, KIND, 0)

  const load = useCallback(async () => {
    try {
      const d = await apiGet('/api/applications/insights')
      setInsight(d.insight)
      trackRunning(d.ai_jobs) // 刷新恢复：把 running 任务重新纳入跟踪
    } catch {
      /* 静默降级：卡片显示引导态 */
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    load()
  }, [load])

  // running 态期间注册完成回调（含刷新后经 trackRunning 恢复的场景）
  useEffect(() => {
    if (!generating) return
    return onJobDone(KIND, 0, (ok) => {
      if (ok) {
        load()
      } else {
        setErr('洞察生成失败，请检查模型配置后重试')
        load()
      }
    })
  }, [generating, load])

  async function generate() {
    setErr('')
    try {
      await startAiJob(KIND, 0, '/api/applications/insights')
    } catch (e: any) {
      setErr(e.message || '发起洞察失败')
    }
  }

  function fmtDate(iso: string) {
    try {
      return new Date(iso).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
    } catch {
      return ''
    }
  }

  return (
    <Section
      title="投递智能洞察"
      sub={insight ? <span>生成于 {fmtDate(insight.created_at)}</span> : undefined}
      action={
        <Button variant="outline" size="sm" onClick={generate} disabled={generating}>
          <Sparkle weight="fill" className="size-4 text-primary" />
          {generating ? '分析中…' : insight ? '重新生成' : '生成洞察'}
        </Button>
      }
    >
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}
      {loading ? (
        <div className="h-16 animate-pulse rounded-lg bg-muted/50" />
      ) : insight ? (
        <div className="space-y-3">
          <p className="text-sm leading-6 text-foreground">{insight.summary}</p>
          {(expanded ? insight.priority : insight.priority.slice(0, 2)).map((p, i) => (
            <div key={i} className="rounded-lg border border-warning/30 bg-warning/5 p-2.5">
              <div className="text-sm font-semibold text-foreground">{p.action}</div>
              <p className="mt-0.5 text-sm leading-6 text-foreground">{p.reason}</p>
            </div>
          ))}
          {expanded && insight.observations.length > 0 && (
            <div>
              <div className="mb-1 text-sm font-medium text-foreground">观察</div>
              <ul className="list-disc space-y-1 pl-5 text-sm leading-6">
                {insight.observations.map((o, i) => (
                  <li key={i}>{o}</li>
                ))}
              </ul>
            </div>
          )}
          {expanded && insight.recommendations.length > 0 && (
            <div>
              <div className="mb-1 text-sm font-medium text-foreground">建议</div>
              <ul className="list-disc space-y-1 pl-5 text-sm leading-6">
                {insight.recommendations.map((r, i) => (
                  <li key={i}>{r}</li>
                ))}
              </ul>
            </div>
          )}
          {(insight.observations.length > 0 || insight.recommendations.length > 0 || insight.priority.length > 2) && (
            <Button size="sm" variant="ghost" onClick={() => setExpanded((v) => !v)}>
              {expanded ? '收起' : '展开完整报告'}
            </Button>
          )}
        </div>
      ) : (
        !generating && (
          <p className="text-sm text-muted-foreground">暂无投递洞察。跟进投递后点击「生成洞察」。</p>
        )
      )}
    </Section>
  )
}
