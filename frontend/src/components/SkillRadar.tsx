import { useMemo } from 'react'
import type { RadarDimension } from '../api/types'

interface SkillRadarProps {
  dimensions: RadarDimension[]
}

/** 8–9 字根域名对半折成两行，禁止省略号。 */
function wrapDomain(name: string): string[] {
  const chars = [...name]
  if (chars.length <= 5) return [name]
  const n = Math.ceil(chars.length / 2)
  return [chars.slice(0, n).join(''), chars.slice(n).join('')]
}

export default function SkillRadar({ dimensions }: SkillRadarProps) {
  const vbW = 580
  const vbH = 540
  const cx = vbW / 2
  const cy = vbH / 2
  const radius = 128

  const points = useMemo(() => {
    if (dimensions.length === 0) return []
    const total = dimensions.length
    return dimensions.map((d, i) => {
      const angle = (Math.PI * 2 * i) / total - Math.PI / 2
      const val = Math.max(8, Math.min(100, d.score))
      const r = (radius * val) / 100
      const labelR = radius + 52
      return {
        ...d,
        angle,
        x: cx + r * Math.cos(angle),
        y: cy + r * Math.sin(angle),
        axisX: cx + radius * Math.cos(angle),
        axisY: cy + radius * Math.sin(angle),
        lx: cx + labelR * Math.cos(angle),
        ly: cy + labelR * Math.sin(angle),
        lines: wrapDomain(d.name),
      }
    })
  }, [dimensions, cx, cy, radius])

  if (dimensions.length < 3) {
    return (
      <div className="flex h-56 w-full items-center justify-center text-sm text-foreground">
        至少 3 个主知识域才生成雷达
      </div>
    )
  }

  const polygonPoints = points.map((p) => `${p.x},${p.y}`).join(' ')
  const levels = [0.25, 0.5, 0.75, 1.0]

  return (
    <div className="mx-auto w-full max-w-xl">
      <svg
        viewBox={`0 0 ${vbW} ${vbH}`}
        className="h-auto w-full"
        preserveAspectRatio="xMidYMid meet"
        role="img"
        aria-labelledby="radar-title"
      >
        <title id="radar-title">能力雷达，各维 0–100</title>
        {levels.map((level) => {
          const gridPoints = points
            .map((_, i) => {
              const angle = (Math.PI * 2 * i) / points.length - Math.PI / 2
              const r = radius * level
              return `${cx + r * Math.cos(angle)},${cy + r * Math.sin(angle)}`
            })
            .join(' ')
          return (
            <polygon
              key={level}
              points={gridPoints}
              className="fill-none stroke-border"
              strokeWidth={level === 1 ? 1.25 : 1}
            />
          )
        })}
        {points.map((p, i) => (
          <line
            key={`axis-${i}`}
            x1={cx}
            y1={cy}
            x2={p.axisX}
            y2={p.axisY}
            className="stroke-border"
            strokeWidth="1"
          />
        ))}
        <polygon
          points={polygonPoints}
          className="fill-accent/15 stroke-accent"
          strokeWidth="2"
          strokeLinejoin="round"
        />
        {points.map((p, i) => {
          const isLeft = p.lx < cx - 16
          const isRight = p.lx > cx + 16
          const isTop = p.ly < cy - 24
          const textAnchor = isLeft ? 'end' : isRight ? 'start' : 'middle'
          const lineH = 15
          const blockH = p.lines.length * lineH + 14
          const startY = isTop ? p.ly - blockH + lineH : isLeft || isRight ? p.ly - blockH / 2 + lineH : p.ly + 6
          return (
            <g key={`lab-${i}`}>
              <circle cx={p.x} cy={p.y} r="3.5" className="fill-accent stroke-background" strokeWidth="1.5" />
              <text
                x={p.lx}
                y={startY}
                textAnchor={textAnchor}
                className="fill-foreground"
                style={{ fontSize: 13, fontWeight: 600 }}
              >
                {p.lines.map((line, li) => (
                  <tspan key={li} x={p.lx} dy={li === 0 ? 0 : lineH}>
                    {line}
                  </tspan>
                ))}
                <tspan
                  x={p.lx}
                  dy={lineH}
                  className="fill-foreground"
                  style={{ fontSize: 12, fontFamily: 'ui-monospace, "Fira Code", monospace', fontWeight: 600 }}
                >
                  {p.score}
                </tspan>
              </text>
            </g>
          )
        })}
      </svg>
      <table className="sr-only">
        <caption>能力雷达各维分数</caption>
        <thead>
          <tr>
            <th>知识域</th>
            <th>分数</th>
          </tr>
        </thead>
        <tbody>
          {dimensions.map((d) => (
            <tr key={d.key}>
              <td>{d.name}</td>
              <td>{d.score}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
