import type { RadarDimension } from '../api/types'
import SkillRadar from './SkillRadar'
import { Section } from './Section'

/** 能力雷达：六域全称与分数标在顶点外侧。 */
export function SkillRadarPanel({
  dimensions,
  extra,
}: {
  dimensions: RadarDimension[]
  extra?: React.ReactNode
}) {
  return (
    <Section
      title="能力雷达"
      sub={<span className="font-mono tabular-nums">{dimensions.length} 维</span>}
      action={extra}
    >
      {dimensions.length < 3 ? (
        <p className="py-8 text-center text-sm text-foreground">至少 3 个主知识域才生成雷达</p>
      ) : (
        <SkillRadar dimensions={dimensions} />
      )}
    </Section>
  )
}
