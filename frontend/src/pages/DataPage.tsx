import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { TrendCard, ReviewCurveCard } from './Stats'
import { PageHeader } from '../components/PageHeader'
import { FunnelCard } from '../components/FunnelCard'
import { SkillRadarPanel } from '../components/SkillRadarPanel'
import { apiGet } from '../api/client'
import type { SkillGraphData } from '../api/types'
import { ChartLineUp } from '@phosphor-icons/react'

/** 押题命中闭环度量（票03）：source=predicted 题目的复习命中分布 */
interface PredictionHitRate {
  total: {
    predicted_count: number
    reviewed_count: number
    hit_rate_percent: number
  }
  by_position: {
    position_id: number | null
    position_title: string | null
    predicted_count: number
    reviewed_count: number
    hit_rate_percent: number
  }[]
}

interface FsrsMemoryData {
  total_cards: number
  avg_retention: number
  distribution: {
    solid: number
    good: number
    fading: number
    risk: number
  }
  due_next_7_days: number[]
  fitted: boolean
}

/** 数据大盘（v5.1 重塑：能力诊断 + FSRS 记忆衰减预测 + 趋势 + 求职漏斗） */
export default function DataPage() {
  const [skillData, setSkillData] = useState<SkillGraphData | null>(null)
  const [fsrsData, setFsrsData] = useState<FsrsMemoryData | null>(null)
  const [predictionData, setPredictionData] = useState<PredictionHitRate | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    Promise.all([
      apiGet('/api/skills/tree').catch(() => null),
      apiGet('/api/stats/fsrs-memory').catch(() => null),
      apiGet('/api/stats/prediction-hit-rate').catch(() => null),
    ])
      .then(([skill, fsrs, pred]) => {
        if (skill) setSkillData(skill)
        if (fsrs) setFsrsData(fsrs)
        if (pred) setPredictionData(pred)
      })
      .finally(() => setLoading(false))
  }, [])

  return (
    <div className="space-y-6">
      <PageHeader
        title="数据大盘"
        meta={<span>能力画像 · 记忆衰减预测 · 表现趋势 · 投递漏斗</span>}
      />

      {/* 4 大核心 KPI 仪表卡 */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <div className="rounded-xl border border-border bg-card px-4 py-3">
          <div className="flex items-end justify-between gap-2">
            <span className="font-mono text-[1.75rem] font-semibold leading-none tabular-nums text-heading">
              {skillData?.radar?.length
                ? Math.round(skillData.radar.reduce((acc, r) => acc + r.score, 0) / skillData.radar.length)
                : 0}
            </span>
            <span className="pb-0.5 text-[11px] font-medium text-foreground">分</span>
          </div>
          <div className="mt-2 flex items-center justify-between gap-2">
            <span className="text-[13px] font-medium text-foreground">技能平均掌握度</span>
            <ChartLineUp className="size-4 text-foreground" aria-hidden />
          </div>
        </div>

        <div className="rounded-xl border border-border bg-card px-4 py-3">
          <div className="flex items-end justify-between gap-2">
            <span className="font-mono text-[1.75rem] font-semibold leading-none tabular-nums text-heading">
              {fsrsData?.avg_retention ?? 100}
            </span>
            <span className="pb-0.5 text-[11px] font-medium text-foreground">%</span>
          </div>
          <div className="mt-2 text-[13px] font-medium text-foreground">记忆留存率</div>
          <div className="mt-0.5 text-xs text-muted-foreground">
            {fsrsData?.fitted ? '已按复习记录拟合' : '样本不足，用默认参数'}
          </div>
        </div>

        <div className="rounded-xl border border-border bg-card px-4 py-3">
          <div className="flex items-end justify-between gap-2">
            <span className="font-mono text-[1.75rem] font-semibold leading-none tabular-nums text-heading">
              {fsrsData?.total_cards ?? 0}
            </span>
            <span className="pb-0.5 text-[11px] font-medium text-foreground">张</span>
          </div>
          <div className="mt-2 text-[13px] font-medium text-foreground">已收录真题卡片</div>
        </div>

        <div className="rounded-xl border border-border bg-card px-4 py-3">
          <div className="flex items-end justify-between gap-2">
            <span className="font-mono text-[1.75rem] font-semibold leading-none tabular-nums text-heading">
              {fsrsData?.distribution?.risk ?? 0}
            </span>
            <span className="pb-0.5 text-[11px] font-medium text-foreground">道</span>
          </div>
          <div className="mt-2 text-[13px] font-medium text-foreground">遗忘预警题目</div>
        </div>
      </div>

      <div className="grid gap-4">
        {loading ? (
          <p className="py-8 text-center text-sm text-muted-foreground">正在计算能力画像…</p>
        ) : (
          <SkillRadarPanel dimensions={skillData?.radar ?? []} />
        )}

        {/* FSRS 记忆衰减预测与 7 天到期负荷 */}
        <section className="rounded-xl border border-border bg-card" aria-label="FSRS 记忆预测">
          <div className="flex items-center justify-between border-b border-border px-4 py-2.5">
            <div>
              <h2 className="text-[13px] font-semibold tracking-wide text-heading">FSRS 记忆衰减模型预测</h2>
              <p className="text-xs text-muted-foreground mt-0.5">
                FSRS 个性化拟合权重 · 遗忘曲线留存率与到期压力预测
              </p>
            </div>
            <span className="rounded-md bg-secondary border border-border-strong px-2 py-0.5 font-mono text-xs font-semibold text-heading">
              平均留存率 {fsrsData?.avg_retention ?? 100}%
            </span>
          </div>

          <div className="space-y-4 p-3">
            {/* 4 档记忆分层 */}
            <div>
              <div className="text-xs font-medium text-muted-foreground mb-2">当前题库记忆牢固度分层</div>
              <div className="grid grid-cols-2 gap-2 text-xs sm:grid-cols-4">
                <div className="rounded-lg border border-success/30 bg-success/5 p-2.5">
                  <div className="text-success font-semibold">牢固 (≥90%)</div>
                  <div className="font-mono text-lg font-bold mt-1 text-foreground">
                    {fsrsData?.distribution?.solid ?? 0}
                  </div>
                </div>
                <div className="rounded-lg border border-primary/30 bg-primary/5 p-2.5">
                  <div className="text-primary font-semibold">熟练 (70-90%)</div>
                  <div className="font-mono text-lg font-bold mt-1 text-foreground">
                    {fsrsData?.distribution?.good ?? 0}
                  </div>
                </div>
                <div className="rounded-lg border border-warning/30 bg-warning/5 p-2.5">
                  <div className="text-warning font-semibold">需巩固 (50-70%)</div>
                  <div className="font-mono text-lg font-bold mt-1 text-foreground">
                    {fsrsData?.distribution?.fading ?? 0}
                  </div>
                </div>
                <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-2.5">
                  <div className="text-destructive font-semibold">遗忘风险 (&lt;50%)</div>
                  <div className="font-mono text-lg font-bold mt-1 text-foreground">
                    {fsrsData?.distribution?.risk ?? 0}
                  </div>
                </div>
              </div>
            </div>

            {/* 未来 7 天到期负荷柱状图 */}
            <div className="pt-2 border-t border-border/60">
              <div className="flex items-center justify-between text-xs font-medium text-muted-foreground mb-2">
                <span>未来 7 天到期复习压力分布</span>
                <span className="text-[11px] font-mono">
                  共计 {fsrsData?.due_next_7_days?.reduce((a, b) => a + b, 0) ?? 0} 题待复习
                </span>
              </div>
              <div className="grid grid-cols-7 gap-1.5 pt-2 items-end h-24">
                {(fsrsData?.due_next_7_days ?? [0, 0, 0, 0, 0, 0, 0]).map((count, i) => {
                  const maxCount = Math.max(1, ...(fsrsData?.due_next_7_days ?? [1]))
                  const heightPercent = Math.max(12, Math.round((count / maxCount) * 100))
                  const dayLabel = i === 0 ? '今天' : i === 1 ? '明天' : `+${i}天`
                  return (
                    <div key={i} className="flex flex-col items-center gap-1 h-full justify-end">
                      <span className="font-mono text-[10px] text-muted-foreground font-semibold">
                        {count}
                      </span>
                      <div
                        className={`w-full rounded-t transition-all ${
                          i === 0
                            ? 'bg-primary'
                            : count > 5
                            ? 'bg-warning/80'
                            : 'bg-muted-foreground/30'
                        }`}
                        style={{ height: `${heightPercent}%` }}
                      />
                      <span className="text-[10px] text-muted-foreground">{dayLabel}</span>
                    </div>
                  )
                })}
              </div>
            </div>
          </div>
        </section>

        {/* 押题命中闭环度量（票03）：数据单向流动的最后一环 */}
        <section className="rounded-lg border border-border bg-card" aria-label="押题命中率">
          <div className="flex items-center justify-between border-b border-border px-3 py-2.5">
            <div>
              <h2 className="text-sm font-semibold text-foreground">岗位押题命中率</h2>
              <p className="text-xs text-muted-foreground mt-0.5">
                押题沉淀入题库并复习后的「记得」占比——回答“押题到底准不准”
              </p>
            </div>
            {(predictionData?.total.predicted_count ?? 0) > 0 && (
              <span className="rounded-md bg-secondary border border-border-strong px-2 py-0.5 font-mono text-xs font-semibold text-heading">
                命中率 {predictionData!.total.hit_rate_percent}%
              </span>
            )}
          </div>

          {(predictionData?.total.predicted_count ?? 0) > 0 ? (
            <div className="space-y-3 p-3">
              <div className="flex items-baseline gap-4 text-xs text-muted-foreground">
                <span>
                  已复习 <span className="font-mono font-semibold text-foreground">{predictionData!.total.reviewed_count}</span>
                  {' / '}
                  押题共 <span className="font-mono font-semibold text-foreground">{predictionData!.total.predicted_count}</span> 题
                </span>
                <Link
                  to="/questions?source=predicted"
                  className="underline-offset-2 hover:text-foreground hover:underline"
                >
                  查看全部押题 →
                </Link>
              </div>
              {predictionData!.by_position.length > 0 && (
                <div className="grid gap-2 sm:grid-cols-2">
                  {predictionData!.by_position.slice(0, 4).map((p) => (
                    <Link
                      key={p.position_id ?? 'none'}
                      to={p.position_id != null ? `/questions?source=predicted&position_id=${p.position_id}` : '/questions?source=predicted'}
                      className="rounded-lg border border-border bg-background p-3 transition-colors hover:border-border-strong"
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span className="truncate text-xs font-medium text-foreground">
                          {p.position_title || '未归属岗位'}
                        </span>
                        <span className="shrink-0 font-mono text-sm font-bold text-heading">
                          {p.hit_rate_percent}%
                        </span>
                      </div>
                      <div className="mt-1 text-[10px] text-muted-foreground">
                        已复习 {p.reviewed_count} / {p.predicted_count} 题
                      </div>
                    </Link>
                  ))}
                </div>
              )}
            </div>
          ) : (
            <div className="mt-4 rounded-lg border border-dashed border-border p-6 text-center text-xs text-muted-foreground">
              暂无押题数据——从岗位详情页发起 AI 考点预测并沉淀入题库后，这里会展示命中率
            </div>
          )}
        </section>
      </div>

      {/* 第二行：求职漏斗全景 */}
      <FunnelCard />

      {/* 第三行：分析趋势与记忆率分布 */}
      <div className="grid gap-4 lg:grid-cols-2">
        <TrendCard />
        <ReviewCurveCard />
      </div>
    </div>
  )
}
