import { useCallback, useEffect, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { apiGet, apiPost } from './api/client'
import type { User } from './api/types'
import Setup from './pages/Setup'
import Login from './pages/Login'
import Layout from './components/Layout'

type AuthState =
  | { status: 'loading' }
  | { status: 'setup' }
  | { status: 'login' }
  | { status: 'authed'; user: User }

export default function App() {
  const [auth, setAuth] = useState<AuthState>({ status: 'loading' })
  const navigate = useNavigate()
  const location = useLocation()

  const refresh = useCallback(async () => {
    try {
      const st = await apiGet('/api/setup/status')
      if (!st.setup_done) {
        setAuth({ status: 'setup' })
        return
      }
      try {
        const user = await apiGet('/api/me')
        setAuth({ status: 'authed', user })
      } catch {
        setAuth({ status: 'login' })
      }
    } catch {
      setAuth({ status: 'login' })
    }
  }, [])

  useEffect(() => {
    refresh()
  }, [refresh])

  // 会话过期/失效：client 派发 beview:unauthorized 事件 -> 切登录态（应用内，无浏览器级重定向）
  useEffect(() => {
    const onUnauthorized = () => setAuth({ status: 'login' })
    window.addEventListener('beview:unauthorized', onUnauthorized)
    return () => window.removeEventListener('beview:unauthorized', onUnauthorized)
  }, [])

  // 认证状态与 URL 同步：未登录/未初始化 -> /login / /setup；已登录 -> 回到目标页
  useEffect(() => {
    if (auth.status === 'setup' && location.pathname !== '/setup') {
      navigate('/setup', { replace: true })
    } else if (auth.status === 'login' && location.pathname !== '/login') {
      navigate('/login', { replace: true })
    } else if (
      auth.status === 'authed' &&
      (location.pathname === '/login' || location.pathname === '/setup')
    ) {
      const dest = sessionStorage.getItem('beview_redirect')
      sessionStorage.removeItem('beview_redirect')
      navigate(dest || '/', { replace: true })
    }
  }, [auth.status, location.pathname, navigate])

  if (auth.status === 'loading') {
    return <div className="py-24 text-center text-muted-foreground">加载中…</div>
  }
  if (auth.status === 'setup') {
    return <Setup onDone={() => setAuth({ status: 'login' })} />
  }
  if (auth.status === 'login') {
    return <Login onLogin={refresh} />
  }
  return (
    <Layout
      user={auth.user}
      onLogout={async () => {
        await apiPost('/api/logout')
        setAuth({ status: 'login' })
      }}
    />
  )
}
