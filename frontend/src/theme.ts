// ADR-0015 D3：双主题（亮默认）。主题状态 = <html> 上的 .dark 类 + localStorage 持久化。
const KEY = 'beview-theme'

export type Theme = 'light' | 'dark'

export function getTheme(): Theme {
  return typeof document !== 'undefined' && document.documentElement.classList.contains('dark')
    ? 'dark'
    : 'light'
}

let mediaQueryListenerAttached = false

/** 首帧前调用：localStorage 优先，否则跟随系统。避免闪白/闪黑。同时监听系统偏好变更。 */
export function initTheme() {
  const saved = localStorage.getItem(KEY) as Theme | null
  const mql = window.matchMedia('(prefers-color-scheme: dark)')
  const dark = saved ? saved === 'dark' : mql.matches
  document.documentElement.classList.toggle('dark', dark)

  if (!mediaQueryListenerAttached && typeof window !== 'undefined') {
    mediaQueryListenerAttached = true
    mql.addEventListener('change', (e) => {
      // 仅在用户未显式设置偏好时跟随系统动态切换
      if (!localStorage.getItem(KEY)) {
        document.documentElement.classList.toggle('dark', e.matches)
      }
    })
  }
}

export function setTheme(theme: Theme) {
  document.documentElement.classList.toggle('dark', theme === 'dark')
  localStorage.setItem(KEY, theme)
}

export function toggleTheme(): Theme {
  const next: Theme = getTheme() === 'dark' ? 'light' : 'dark'
  setTheme(next)
  return next
}
