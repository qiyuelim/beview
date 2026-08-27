import { useEffect, useState, type ReactNode } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { apiGet, apiPatch, apiPost } from '../api/client'

import { CaretDown, CaretRight, Gear } from '@phosphor-icons/react'
import { PageHeader } from '../components/PageHeader'
import { FormField } from '../components/FormField'
import { SemBadge } from '../components/SemBadge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

interface PromptItem {
  key: string
  name: string
  description: string
  value: string
  is_custom: boolean
}

function SettingsGroup({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <section className={title ? 'mt-6' : undefined}>
      {title ? <h2 className="mb-2 px-1 text-sm font-semibold">{title}</h2> : null}
      <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-card">{children}</div>
    </section>
  )
}

function SettingsRow({
  title,
  description,
  value,
  onClick,
  href,
}: {
  title: string
  description?: string
  value?: ReactNode
  onClick?: () => void
  href?: string
}) {
  const body = (
    <>
      <div className="min-w-0 flex-1">
        <div className="text-sm font-medium text-foreground">{title}</div>
        {description ? <p className="mt-0.5 text-xs text-muted-foreground">{description}</p> : null}
      </div>
      <div className="flex shrink-0 items-center gap-1.5 text-sm text-foreground">
        {value}
        <CaretRight className="size-4 text-muted-foreground" aria-hidden />
      </div>
    </>
  )
  const cls =
    'flex w-full items-center gap-3 px-4 py-3.5 text-left transition-colors hover:bg-muted/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50'
  if (href) {
    return (
      <Link to={href} className={cls}>
        {body}
      </Link>
    )
  }
  return (
    <button type="button" onClick={onClick} className={cls}>
      {body}
    </button>
  )
}

export default function Settings() {
  const navigate = useNavigate()
  const [err, setErr] = useState('')
  const [resolvedText, setResolvedText] = useState('加载中…')

  const [oldPw, setOldPw] = useState('')
  const [newPw, setNewPw] = useState('')
  const [pwMsg, setPwMsg] = useState('')
  const [pwOpen, setPwOpen] = useState(false)
  const [calToken, setCalToken] = useState('')
  const [calMsg, setCalMsg] = useState('')
  const [prompts, setPrompts] = useState<PromptItem[]>([])

  async function loadPrompts() {
    const d = await apiGet('/api/settings/prompts')
    setPrompts(d.prompts ?? [])
  }

  useEffect(() => {
    loadPrompts().catch(() => {})
  }, [])

  useEffect(() => {
    apiGet('/api/calendar/token')
      .then((d) => setCalToken(d.token ?? ''))
      .catch(() => {})
  }, [])

  const calUrl = calToken ? `${location.origin}/api/calendar.ics?token=${calToken}` : ''
  const customCount = prompts.filter((p) => p.is_custom).length

  async function copyText(t: string): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(t)
      return true
    } catch {
      /* fallthrough */
    }
    try {
      const ta = document.createElement('textarea')
      ta.value = t
      ta.style.position = 'fixed'
      ta.style.opacity = '0'
      document.body.appendChild(ta)
      ta.focus()
      ta.select()
      const ok = document.execCommand('copy')
      ta.remove()
      return ok
    } catch {
      return false
    }
  }

  async function copyCal() {
    const ok = await copyText(calUrl)
    setCalMsg(ok ? '已复制订阅链接' : '复制失败，请手动选中链接复制')
  }

  async function regenerateCal() {
    setErr('')
    try {
      const d = await apiPost('/api/calendar/token', {})
      setCalToken(d.token)
      setCalMsg('已重新生成，旧订阅链接已失效')
    } catch (e: any) {
      setErr(e.message)
    }
  }

  useEffect(() => {
    apiGet('/api/settings/llm-config')
      .then((d) => {
        const r = d.resolved
        if (r) setResolvedText(`${r.provider} · ${r.model}`)
        else if (d.resolve_error) setResolvedText(`配置存在问题：${d.resolve_error}`)
        else setResolvedText('未配置')
      })
      .catch(() => setResolvedText('未配置'))
  }, [])

  async function changePw() {
    setPwMsg('')
    setErr('')
    try {
      await apiPost('/api/settings/password', { old_password: oldPw, new_password: newPw })
      setPwMsg('密码已修改，请重新登录')
      setOldPw('')
      setNewPw('')
    } catch (e: any) {
      setErr(e.message)
    }
  }

  return (
    <div className="w-full">
      <PageHeader
        title="设置"
        meta={
          <span className="inline-flex items-center gap-1">
            <Gear className="size-3.5" aria-hidden /> 账号 · AI · 数据
          </span>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      <div className="max-w-[640px]">
      <SettingsGroup>
        <SettingsRow
          title="LLM 配置"
          description="多 Provider、结构化输出与思考强度"
          value={<span className="max-w-[12rem] truncate">{resolvedText}</span>}
          onClick={() => navigate('/settings/llm')}
        />
        <SettingsRow
          title="提示词"
          description={customCount > 0 ? `${customCount} 份已自定义` : '当前全部使用内置默认'}
          value={
            <span className="inline-flex items-center gap-1.5">
              <span className="font-mono tabular-nums">{prompts.length}</span>
              {customCount > 0 && <SemBadge sem="info">自定义 {customCount}</SemBadge>}
            </span>
          }
          href="/settings/prompts"
        />
      </SettingsGroup>

      <SettingsGroup title="账号">
        <div>
          <button
            type="button"
            aria-expanded={pwOpen}
            onClick={() => setPwOpen((v) => !v)}
            className="flex w-full items-center gap-3 px-4 py-3.5 text-left transition-colors hover:bg-muted/60"
          >
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium">修改密码</div>
              <p className="mt-0.5 text-xs text-muted-foreground">登录凭据仅本机账号使用</p>
            </div>
            <CaretDown className={`size-4 text-muted-foreground transition-transform ${pwOpen ? 'rotate-180' : ''}`} aria-hidden />
          </button>
          {pwOpen && (
            <div className="space-y-3 border-t border-border px-4 py-3">
              <FormField label="旧密码" htmlFor="pw-old">
                <Input
                  id="pw-old"
                  type="password"
                  value={oldPw}
                  onChange={(e) => setOldPw(e.target.value)}
                  autoComplete="current-password"
                />
              </FormField>
              <FormField label="新密码" htmlFor="pw-new" hint="至少 6 位">
                <Input
                  id="pw-new"
                  type="password"
                  value={newPw}
                  onChange={(e) => setNewPw(e.target.value)}
                  autoComplete="new-password"
                />
              </FormField>
              <div className="flex items-center gap-3">
                <Button variant="secondary" onClick={changePw}>
                  修改密码
                </Button>
                {pwMsg && <span className="text-sm font-medium text-success">{pwMsg}</span>}
              </div>
            </div>
          )}
        </div>
        <div className="px-4 py-3.5">
          <div className="text-sm font-medium">日历订阅</div>
          <p className="mt-0.5 text-xs text-muted-foreground">面试轮次与复习到期同步到日历 App，不含题目正文</p>
          <div className="mt-2.5 flex flex-wrap items-center gap-2">
            <Input
              id="cal-url"
              readOnly
              value={calUrl}
              placeholder="加载中…"
              onFocus={(e) => e.currentTarget.select()}
              className="min-w-0 flex-1 font-mono text-xs"
            />
            <Button variant="secondary" size="sm" onClick={copyCal} disabled={!calUrl}>
              复制
            </Button>
            <Button variant="ghost" size="sm" onClick={regenerateCal} disabled={!calUrl} title="泄露或弃用时吊销旧链接">
              重新生成
            </Button>
          </div>
          {calMsg && <p className="mt-1.5 text-sm font-medium text-success">{calMsg}</p>}
        </div>
      </SettingsGroup>

      <SettingsGroup title="数据">
        <div className="flex items-center gap-3 px-4 py-3.5">
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium">全量备份</div>
            <p className="mt-0.5 text-xs text-muted-foreground">导出全量 JSON 备份</p>
          </div>
          <Button variant="secondary" size="sm" asChild>
            <a href="/api/export" download>
              下载
            </a>
          </Button>
        </div>
      </SettingsGroup>

      <UserAdmin />
      </div>
    </div>
  )
}

interface AdminUser {
  id: number
  username: string
  role: string
  row_status: string
  created_at: string
}

function UserAdmin() {
  const [users, setUsers] = useState<AdminUser[] | null>(null)
  const [isAdmin, setIsAdmin] = useState(false)
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [msg, setMsg] = useState('')
  const [err, setErr] = useState('')
  const [resetTarget, setResetTarget] = useState<{ id: number; username: string } | null>(null)
  const [newPassword, setNewPassword] = useState('')
  const [resetErr, setResetErr] = useState('')
  async function load() {
    try {
      const me = await apiGet('/api/me')
      setIsAdmin(me.role === 'admin')
      if (me.role === 'admin') {
        setUsers(await apiGet('/api/admin/users'))
      }
    } catch {
      /* ignore */
    }
  }
  useEffect(() => {
    load()
  }, [])

  if (!isAdmin) return null

  async function createUser() {
    setErr('')
    setMsg('')
    try {
      await apiPost('/api/admin/users', { username, password })
      setUsername('')
      setPassword('')
      setMsg(`已创建用户 ${username}`)
      await load()
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function patchUser(id: number, body: Record<string, unknown>, okMsg: string) {
    setErr('')
    setMsg('')
    try {
      await apiPatch(`/api/admin/users/${id}`, body)
      setMsg(okMsg)
      await load()
    } catch (e: any) {
      setErr(e.message)
    }
  }

  return (
    <SettingsGroup title="用户管理">
      <div className="px-4 py-3">
        <p className="mb-3 text-xs text-muted-foreground">开账号、停用（拒登录、数据保留）与重置密码；不开放自助注册。</p>
        <div className="flex flex-wrap items-center gap-2">
          <Input className="w-full sm:w-36" value={username} onChange={(e) => setUsername(e.target.value)} placeholder="用户名" aria-label="新用户名" />
          <Input
            className="w-full sm:w-40"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="密码（至少 6 位）"
            type="password"
            aria-label="初始密码"
          />
          <Button size="sm" onClick={createUser} disabled={!username.trim() || password.length < 6}>
            创建
          </Button>
        </div>
        {(msg || err) && <p className={`mt-2 text-sm font-medium ${err ? 'text-destructive' : 'text-success'}`}>{err || msg}</p>}
      </div>
      <ul>
        {(users ?? []).map((u) => (
          <li key={u.id} className="flex min-h-[48px] flex-wrap items-center gap-2 border-t border-border px-4 py-2.5">
            <span className="inline-flex min-w-0 flex-1 items-center gap-1.5 text-sm font-medium">
              <span className="truncate">{u.username}</span>
              <SemBadge sem={u.role === 'admin' ? 'info' : 'neutral'}>{u.role}</SemBadge>
              {u.row_status === 'disabled' && <SemBadge sem="danger">已停用</SemBadge>}
            </span>
            <span className="flex items-center gap-1">
              <Button
                size="sm"
                variant="ghost"
                onClick={() =>
                  patchUser(
                    u.id,
                    { row_status: u.row_status === 'active' ? 'disabled' : 'active' },
                    u.row_status === 'active' ? `已停用 ${u.username}` : `已恢复 ${u.username}`,
                  )
                }
              >
                {u.row_status === 'active' ? '停用' : '恢复'}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  setResetTarget({ id: u.id, username: u.username })
                  setNewPassword('')
                  setResetErr('')
                }}
              >
                重置密码
              </Button>
            </span>
          </li>
        ))}
      </ul>

      {resetTarget && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-sm rounded-lg border border-border bg-card p-5 shadow-xl">
            <h3 className="text-sm font-bold text-foreground">重置用户密码</h3>
            <p className="mt-1 text-xs text-muted-foreground">
              为 <b>{resetTarget.username}</b> 设置新登录密码
            </p>
            <div className="mt-4">
              <FormField label="新密码" required htmlFor="reset-user-pw" error={resetErr} hint="至少 6 位字符">
                <Input
                  id="reset-user-pw"
                  type="password"
                  autoFocus
                  value={newPassword}
                  onChange={(e) => {
                    setResetErr('')
                    setNewPassword(e.target.value)
                  }}
                  placeholder="输入新密码…"
                />
              </FormField>
            </div>
            <div className="mt-5 flex justify-end gap-2">
              <Button size="sm" variant="ghost" onClick={() => setResetTarget(null)}>
                取消
              </Button>
              <Button
                size="sm"
                onClick={() => {
                  if (newPassword.length < 6) {
                    setResetErr('密码长度至少 6 位')
                    return
                  }
                  patchUser(resetTarget.id, { password: newPassword }, `已重置 ${resetTarget.username} 的密码`)
                  setResetTarget(null)
                }}
              >
                确认重置
              </Button>
            </div>
          </div>
        </div>
      )}
    </SettingsGroup>
  )
}
