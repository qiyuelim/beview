import type { IconProps } from '@phosphor-icons/react'
import { cn } from '@/lib/utils'

// ADR-0015 D2 组合层：空态。图标 + 标题 + 可选引导 + 可选动作。
// hint 属灰字白名单「操作引导」，必须是真实可执行的下一步。
export function EmptyState({
  icon,
  title,
  hint,
  action,
  className,
}: {
  icon: React.ReactElement<IconProps>
  title: React.ReactNode
  hint?: React.ReactNode
  action?: React.ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border-strong px-6 py-12 text-center',
        className,
      )}
    >
      {cloneIcon(icon)}
      <div className="text-sm font-semibold">{title}</div>
      {hint ? <p className="max-w-sm text-sm text-muted-foreground">{hint}</p> : null}
      {action ? <div className="mt-2">{action}</div> : null}
    </div>
  )
}

function cloneIcon(icon: React.ReactElement<IconProps>) {
  return (
    <span className="text-muted-foreground [&_svg]:size-6">
      {icon.props.className === undefined
        ? { ...icon, props: { ...icon.props, 'aria-hidden': true } }
        : icon}
    </span>
  )
}
