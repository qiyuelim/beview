export function BrandMark({ className = 'size-8' }: { className?: string }) {
  return (
    <img
      src="/apple-touch-icon.png"
      alt=""
      className={className}
      width={64}
      height={64}
      draggable={false}
    />
  )
}
