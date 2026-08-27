import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useEffect, useState } from 'react'

// ADR-0015 D2 组合层：确认弹窗。替代散落的内联确认条；
// destructive 动作可选输入确认词（如「删除投递」场景）。
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel = '确认',
  cancelLabel = '取消',
  destructive,
  confirmKeyword,
  onConfirm,
  busy,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: React.ReactNode
  description?: React.ReactNode
  confirmLabel?: string
  cancelLabel?: string
  destructive?: boolean
  /** 非空时需输入该词才能确认 */
  confirmKeyword?: string
  onConfirm: () => void
  busy?: boolean
}) {
  const [typed, setTyped] = useState('')
  useEffect(() => {
    if (!open) setTyped('')
  }, [open])
  const locked = confirmKeyword !== undefined && typed !== confirmKeyword

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          {description ? <DialogDescription>{description}</DialogDescription> : null}
        </DialogHeader>
        {confirmKeyword !== undefined ? (
          <Input
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            placeholder={`输入「${confirmKeyword}」以确认`}
            aria-label="确认词"
          />
        ) : null}
        <DialogFooter className="gap-2">
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            {cancelLabel}
          </Button>
          <Button
            variant={destructive ? 'destructive' : 'default'}
            onClick={async () => {
              const res = onConfirm() as any
              if (res && typeof res.then === 'function') {
                try {
                  await res
                  onOpenChange(false)
                } catch {
                  // 出错保持开启以便展示错误
                }
              }
            }}
            disabled={busy || locked}
          >
            {confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
