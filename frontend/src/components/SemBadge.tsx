import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'

// ADR-0015 D2 组合层：Badge 语义变体，对齐 docs/context.md 状态词表。
// 绿=通过、琥珀=待定/警告、红=失败/危险、蓝=信息/进行中、亮蓝=AI 来源、灰=中性/模拟。
const semStyles = {
  pass: 'bg-success text-success-foreground',
  warn: 'bg-warning text-warning-foreground',
  danger: 'bg-destructive text-destructive-foreground',
  info: 'bg-info text-info-foreground',
  ai: 'bg-secondary text-secondary-foreground',
  neutral: 'bg-muted text-foreground',
} as const

export type BadgeSem = keyof typeof semStyles

export function SemBadge({
  sem,
  className,
  ...props
}: React.ComponentProps<typeof Badge> & { sem: BadgeSem }) {
  return <Badge className={cn(semStyles[sem], className)} {...props} />
}
