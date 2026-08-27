//! 数据资产化卡片组件（ADR-0010 R12/R13；v4.2 设计语言 v2 迁移，M8 清理死代码）：
//! - TrendCard：综合分趋势（近 90 天折线）+ 公司均分对比（条形）
//! - ReviewCurveCard：复习记忆率（每日自评分布堆叠条）
//! 各自自取数；图表为轻量自绘 SVG（无外部依赖，数据量小）。

import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { apiGet } from '../api/client'
import type { ReviewCurve, ScoreTrend } from '../api/types'

function Card({ title, sub, children }: { title: string; sub?: string; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-border bg-card" aria-label={title} style={{ marginTop: 16 }}>
      <div className="flex flex-wrap items-baseline gap-2 border-b border-border px-3 py-2.5">
        <h2 className="text-sm font-semibold">{title}</h2>
        {sub && <span className="font-mono text-xs text-muted-foreground">{sub}</span>}
      </div>
      <div className="p-3">{children}</div>
    </section>
  )
}

// ---------- 综合分趋势 ----------

export function TrendCard() {
  const [trend, setTrend] = useState<ScoreTrend | null>(null)
  const [err, setErr] = useState('')
  useEffect(() => {
    apiGet('/api/stats/trend').then(setTrend).catch((e) => setErr(e.message))
  }, [])
  const line = trend?.by_date ?? []
  const bars = trend?.by_company ?? []
  return (
    <Card title={`综合分趋势 · 近 90 天`} sub={line.length ? `${line.length} 天` : undefined}>
      {err && <p className="text-xs text-muted-foreground">{err}</p>}
      {!trend ? (
        <p className="text-sm text-muted-foreground">加载中…</p>
      ) : line.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          暂无分析数据——去 <Link to="/questions" className="text-primary underline underline-offset-2">题库</Link> 触发分析
        </p>
      ) : (
        <>
          <LineChart data={line.map((p) => ({ label: p.date.slice(5), value: Math.round(p.avg_score) }))} height={180} color="var(--c-primary)" />
          <div className="mt-3 border-t border-border pt-3">
            <div className="mb-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">公司均分对比</div>
            {bars.length === 0 ? (
              <p className="text-sm text-muted-foreground">暂无数据。</p>
            ) : (
              <Bars data={bars.map((b) => ({ label: b.company, value: Math.round(b.avg_score ?? 0), count: b.count }))} height={150} />
            )}
          </div>
        </>
      )}
    </Card>
  )
}

// ---------- 复习记忆率 ----------

export function ReviewCurveCard() {
  const [curve, setCurve] = useState<ReviewCurve | null>(null)
  const [err, setErr] = useState('')
  useEffect(() => {
    apiGet('/api/stats/review-curve').then(setCurve).catch((e) => setErr(e.message))
  }, [])
  if (!curve)
    return (
      <Card title="复习记忆率">
        <p className="text-sm text-muted-foreground">{err || '加载中…'}</p>
      </Card>
    )
  const total = curve.totals.remembered + curve.totals.fuzzy + curve.totals.forgot
  const memRate = total ? Math.round((curve.totals.remembered / total) * 100) : 0
  return (
    <Card title="复习记忆率" sub={`连续 ${curve.streak_days} 天`}>
      <div className="mb-2.5 flex flex-wrap items-baseline gap-x-4 gap-y-1 text-sm">
        <span>
          记得 <b className="font-mono tabular-nums text-success">{curve.totals.remembered}</b>
        </span>
        <span>
          模糊 <b className="font-mono tabular-nums">{curve.totals.fuzzy}</b>
        </span>
        <span>
          忘了 <b className="font-mono tabular-nums text-destructive">{curve.totals.forgot}</b>
        </span>
        <span>
          记忆率 <b className="font-mono tabular-nums text-primary">{memRate}%</b>
        </span>
      </div>
      {curve.daily.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          暂无复习记录——去 <Link to="/review" className="text-primary underline underline-offset-2">复习</Link>
        </p>
      ) : (
        <StackedBars
          data={curve.daily.map((d) => ({
            label: d.date.slice(5),
            remembered: d.remembered,
            fuzzy: d.fuzzy,
            forgot: d.forgot,
          }))}
          height={150}
        />
      )}
    </Card>
  )
}

// ---------- 轻量自绘图表（SVG，token 取色） ----------

function LineChart({ data, height, color }: { data: { label: string; value: number }[]; height: number; color: string }) {
  const W = 560
  const H = height
  const PAD = { t: 10, r: 6, b: 22, l: 30 }
  const iw = W - PAD.l - PAD.r
  const ih = H - PAD.t - PAD.b
  const max = Math.max(100, ...data.map((d) => d.value))
  const pts = data.map((d, i) => {
    const x = PAD.l + (data.length === 1 ? iw / 2 : (i / (data.length - 1)) * iw)
    const y = PAD.t + ih - (d.value / max) * ih
    return { x, y, ...d }
  })
  return (
    <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', height: 'auto' }} role="img" aria-label="综合分趋势">
      {[0, 0.5, 1].map((g) => {
        const y = PAD.t + ih * (1 - g)
        return (
          <g key={g}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y} y2={y} stroke="var(--c-line)" strokeWidth={1} />
            <text x={PAD.l - 4} y={y + 3} textAnchor="end" fontSize={9} fill="var(--c-text-3)">
              {Math.round(max * g)}
            </text>
          </g>
        )
      })}
      {pts.length > 0 && (
        <polyline points={pts.map((p) => `${p.x},${p.y}`).join(' ')} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" />
      )}
      {pts.map((p, i) => (
        <circle key={i} cx={p.x} cy={p.y} r={2.6} fill={color} />
      ))}
      {pts
        .filter((_, i) => pts.length <= 12 || i % Math.ceil(pts.length / 8) === 0 || i === pts.length - 1)
        .map((p, i) => (
          <text key={i} x={p.x} y={H - 6} textAnchor="middle" fontSize={8.5} fill="var(--c-text-3)">
            {p.label}
          </text>
        ))}
    </svg>
  )
}

function Bars({ data, height }: { data: { label: string; value: number; count: number }[]; height: number }) {
  const W = 560
  const H = height
  const PAD = { t: 10, r: 6, b: 22, l: 30 }
  const iw = W - PAD.l - PAD.r
  const ih = H - PAD.t - PAD.b
  const max = Math.max(100, ...data.map((d) => d.value))
  const bw = Math.min(56, (iw / Math.max(data.length, 1)) * 0.6)
  return (
    <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', height: 'auto' }} role="img" aria-label="公司均分对比">
      {[0, 0.5, 1].map((g) => {
        const y = PAD.t + ih * (1 - g)
        return (
          <g key={g}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y} y2={y} stroke="var(--c-line)" strokeWidth={1} />
            <text x={PAD.l - 4} y={y + 3} textAnchor="end" fontSize={9} fill="var(--c-text-3)">
              {Math.round(max * g)}
            </text>
          </g>
        )
      })}
      {data.map((d, i) => {
        const x = PAD.l + (i + 0.5) * (iw / Math.max(data.length, 1)) - bw / 2
        const h = (d.value / max) * ih
        const y = PAD.t + ih - h
        return (
          <g key={i}>
            <rect x={x} y={y} width={bw} height={Math.max(h, 1)} rx={3} fill="var(--c-primary)" opacity={0.82} />
            <text x={x + bw / 2} y={y - 4} textAnchor="middle" fontSize={9} fill="var(--c-text-2)">
              {d.value}
              {d.count > 1 ? `·${d.count}` : ''}
            </text>
            <text x={x + bw / 2} y={H - 6} textAnchor="middle" fontSize={8.5} fill="var(--c-text-3)">
              {d.label.length > 6 ? d.label.slice(0, 6) + '…' : d.label}
            </text>
          </g>
        )
      })}
    </svg>
  )
}

function StackedBars({ data, height }: { data: { label: string; remembered: number; fuzzy: number; forgot: number }[]; height: number }) {
  const W = 560
  const H = height
  const PAD = { t: 10, r: 6, b: 22, l: 30 }
  const iw = W - PAD.l - PAD.r
  const ih = H - PAD.t - PAD.b
  const max = Math.max(1, ...data.map((d) => d.remembered + d.fuzzy + d.forgot))
  const bw = Math.min(44, (iw / Math.max(data.length, 1)) * 0.6)
  return (
    <svg viewBox={`0 0 ${W} ${H}`} style={{ width: '100%', height: 'auto' }} role="img" aria-label="每日复习结果分布">
      {[0, 0.5, 1].map((g) => {
        const y = PAD.t + ih * (1 - g)
        return (
          <g key={g}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y} y2={y} stroke="var(--c-line)" strokeWidth={1} />
            <text x={PAD.l - 4} y={y + 3} textAnchor="end" fontSize={9} fill="var(--c-text-3)">
              {Math.round(max * g)}
            </text>
          </g>
        )
      })}
      {data.map((d, i) => {
        const x = PAD.l + (i + 0.5) * (iw / Math.max(data.length, 1)) - bw / 2
        const segs: [string, number][] = [
          ['var(--c-pass)', d.remembered / max],
          ['var(--c-text-3)', d.fuzzy / max],
          ['var(--c-danger)', d.forgot / max],
        ]
        let y = PAD.t + ih
        return (
          <g key={i}>
            {segs.map(([color, h]) => {
              const yy = y - h * ih
              const rect = (
                <rect x={x} y={yy} width={bw} height={Math.max(h * ih, 1)} rx={1.5} fill={color} opacity={0.85} />
              )
              y = yy
              return rect
            })}
            <text x={x + bw / 2} y={H - 6} textAnchor="middle" fontSize={8.5} fill="var(--c-text-3)">
              {d.label}
            </text>
          </g>
        )
      })}
    </svg>
  )
}
