import { useState } from 'react'
import { apiPost } from '../api/client'
import { AuthShell } from '../components/AuthShell'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

export default function Login({ onLogin }: { onLogin: () => void }) {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [err, setErr] = useState('')
  const [busy, setBusy] = useState(false)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setErr('')
    setBusy(true)
    try {
      await apiPost('/api/login', { username, password })
      onLogin()
    } catch (ex: any) {
      setErr(ex.message || '登录失败')
    } finally {
      setBusy(false)
    }
  }

  return (
    <AuthShell subtitle="登录">
      <form onSubmit={submit} aria-label="登录" className="space-y-3 rounded-lg border border-border bg-card p-5">
        <FormField label="用户名" htmlFor="lg-user">
          <Input
            id="lg-user"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoFocus
            autoComplete="username"
          />
        </FormField>
        <FormField label="密码" htmlFor="lg-pass">
          <Input
            id="lg-pass"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="current-password"
          />
        </FormField>
        {err && (
          <p role="alert" className="text-sm font-medium text-destructive">
            {err}
          </p>
        )}
        <Button type="submit" disabled={busy} className="w-full">
          {busy ? '登录中…' : '登录'}
        </Button>
      </form>
    </AuthShell>
  )
}
