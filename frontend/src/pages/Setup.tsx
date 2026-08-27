import { useState } from 'react'
import { apiPost } from '../api/client'
import { AuthShell } from '../components/AuthShell'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

export default function Setup({ onDone }: { onDone: () => void }) {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [err, setErr] = useState('')
  const [confirmErr, setConfirmErr] = useState('')
  const [busy, setBusy] = useState(false)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setErr('')
    setConfirmErr('')
    if (password !== confirm) {
      setConfirmErr('两次密码不一致')
      return
    }
    setBusy(true)
    try {
      await apiPost('/api/setup', { username, password })
      onDone()
    } catch (ex: any) {
      setErr(ex.message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <AuthShell subtitle="创建管理员">
      <form
        onSubmit={submit}
        aria-label="创建管理员"
        className="space-y-3 rounded-lg border border-border bg-card p-5"
      >
        <p className="text-sm text-foreground">
          创建管理员账号后进入 Beview。
        </p>
        <FormField label="用户名" htmlFor="su-user">
          <Input
            id="su-user"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoFocus
            autoComplete="username"
          />
        </FormField>
        <FormField label="密码" htmlFor="su-pass" hint="至少 6 位" required>
          <Input
            id="su-pass"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="new-password"
          />
        </FormField>
        <FormField label="确认密码" htmlFor="su-confirm" error={confirmErr || undefined}>
          <Input
            id="su-confirm"
            type="password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            autoComplete="new-password"
          />
        </FormField>
        {err && (
          <p role="alert" className="text-sm font-medium text-destructive">
            {err}
          </p>
        )}
        <Button type="submit" disabled={busy} className="w-full">
          {busy ? '创建中…' : '创建管理员'}
        </Button>
      </form>
    </AuthShell>
  )
}
