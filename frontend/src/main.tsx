import React from 'react'
import ReactDOM from 'react-dom/client'
import { createBrowserRouter, RouterProvider } from 'react-router-dom'
import App from './App'
import { initTheme } from './theme'
import { toast } from 'sonner'
// 设计语言 v2 字体（ADR-0015 D2：@fontsource 自托管，不依赖 Google CDN）
import '@fontsource-variable/inter'
import '@fontsource/fira-code/400.css'
import '@fontsource/fira-code/500.css'
import '@fontsource/fira-code/600.css'
// 设计语言 v2 全局样式（Tailwind 分层 + 语义 token + preflight）
import './index.css'


const router = createBrowserRouter([
  {
    path: '*',
    element: <App />,
  },
])

initTheme()

// 注册 PWA ServiceWorker（离线缓存与后台同步，网络优先导航，ADR-0017 §3.4）
if ('serviceWorker' in navigator && import.meta.env.PROD) {
  window.addEventListener('load', () => {
    navigator.serviceWorker
      .register('/sw.js')
      .then((reg) => {
        // 页面重新可见时主动检查更新
        document.addEventListener('visibilitychange', () => {
          if (document.visibilityState === 'visible') {
            reg.update().catch(() => {})
          }
        })

        // 监听新 ServiceWorker 安装事件
        reg.addEventListener('updatefound', () => {
          const newWorker = reg.installing
          if (!newWorker) return
          newWorker.addEventListener('statechange', () => {
            if (newWorker.state === 'installed' && navigator.serviceWorker.controller) {
              toast('新版本已就绪', {
                description: '检测到最新版本构建，点击刷新以载入最新功能与界面修复',
                action: {
                  label: '立即刷新',
                  onClick: () => {
                    newWorker.postMessage({ type: 'SKIP_WAITING' })
                    window.location.reload()
                  },
                },
                duration: 10000,
              })
            }
          })
        })
      })
      .catch((err) => {
        console.warn('ServiceWorker registration failed: ', err)
      })

    // 控制权转移时自动重载页面
    let refreshing = false
    navigator.serviceWorker.addEventListener('controllerchange', () => {
      if (!refreshing) {
        refreshing = true
        window.location.reload()
      }
    })
  })
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>,
)
