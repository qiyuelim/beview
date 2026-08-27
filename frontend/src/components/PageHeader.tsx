import { cn } from '@/lib/utils'

// ADR-0015 D2 组合层：页头。标题行 + 可选副题/元信息（灰字白名单：元数据）+ 右侧动作区。
export function PageHeader({
  title,
  meta,
  actions,
  className,
}: {
  title: React.ReactNode
  /** 时间、来源、计数等元数据或状态徽标（非 muted 的长文说明） */
  meta?: React.ReactNode
  actions?: React.ReactNode
  className?: string
}) {
  return (
    <header
      className={cn(
        'mb-5 flex flex-wrap items-end gap-x-4 gap-y-2 border-b border-border pb-3',
        className,
      )}
    >
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
        <h1 className="truncate text-[1.375rem] font-semibold tracking-tight text-heading">{title}</h1>
        {meta ? <div className="flex items-center gap-2 text-sm text-muted-foreground">{meta}</div> : null}
      </div>
      {actions ? <div className="ml-auto flex flex-wrap items-center gap-2">{actions}</div> : null}
    </header>
  )
}
