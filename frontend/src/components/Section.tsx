import { cn } from '@/lib/utils'

// ADR-0015 D2 组合层：区块卡片容器（标题行 + 内容区）。
export function Section({
  title,
  sub,
  action,
  children,
  className,
}: {
  title?: React.ReactNode
  sub?: React.ReactNode
  action?: React.ReactNode
  children: React.ReactNode
  className?: string
}) {
  return (
    <section className={cn('rounded-xl border border-border bg-card', className)} aria-label={typeof title === 'string' ? title : undefined}>
      {(title || action) && (
        <header className="flex flex-wrap items-center gap-2 border-b border-border px-4 py-2.5">
          <h2 className="text-[13px] font-semibold tracking-wide text-heading">{title}</h2>
          {sub ? <span className="text-xs text-muted-foreground">{sub}</span> : null}
          {action ? <div className="ml-auto flex flex-wrap items-center gap-2">{action}</div> : null}
        </header>
      )}
      <div className="p-4">{children}</div>
    </section>
  )
}
