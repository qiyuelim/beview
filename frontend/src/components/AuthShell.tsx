import { BrandMark } from './BrandMark'

export function AuthShell({
  subtitle,
  children,
}: {
  subtitle: string
  children: React.ReactNode
}) {
  return (
    <div className="grid min-h-screen place-items-center bg-background px-4">
      <div className="w-full max-w-sm">
        <div className="mb-6 flex flex-col items-center gap-2 text-center">
          <BrandMark className="size-12 rounded-xl shadow-sm" />
          <div>
            <div className="text-lg font-semibold tracking-tight text-heading">Beview</div>
            <div className="text-xs tracking-wide text-heading/70">Be Ready, Review Better.</div>
            <div className="mt-1 text-sm text-foreground">{subtitle}</div>
          </div>
        </div>
        {children}
      </div>
    </div>
  )
}
