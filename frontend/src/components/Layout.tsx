import { cloneElement, lazy, Suspense, useEffect, useState } from 'react'
import { NavLink, Route, Routes } from 'react-router-dom'
import {
  Buildings,
  ChartBar,
  ChatsCircle,
  Coin,
  FileText,
  Gauge,
  Gear,
  ListChecks,
  Moon,
  PaperPlaneTilt,
  SignOut,
  Sparkle,
  Sun,
  TreeStructure,
} from '@phosphor-icons/react'
import type { IconProps } from '@phosphor-icons/react'
import type { User } from '../api/types'
import { apiGet } from '../api/client'
import { connectAiEvents, useGlobalBatchAnalysis } from '../ai/jobs'
import { getTheme, toggleTheme, type Theme } from '../theme'
import { cn } from '@/lib/utils'
import { Toaster } from '@/components/ui/sonner'
import { BrandMark } from './BrandMark'

// v4.2 M8：页面按路由懒加载分包（每页独立 chunk，首屏只拉当前页）
import { Skeleton } from '@/components/ui/skeleton'

const Dashboard = lazy(() => import('../pages/Dashboard'))
const Review = lazy(() => import('../pages/Review'))
const ReviewWrong = lazy(() => import('../pages/ReviewWrong'))
const Drills = lazy(() => import('../pages/Drills'))
const DrillNew = lazy(() => import('../pages/DrillNew'))
const DrillSession = lazy(() => import('../pages/DrillSession'))
const ResumePage = lazy(() => import('../pages/Resume'))
const Applications = lazy(() => import('../pages/Applications'))
const Companies = lazy(() => import('../pages/Companies'))
const CompanyDetail = lazy(() => import('../pages/CompanyDetail'))
const PositionDetail = lazy(() => import('../pages/PositionDetail'))
const RoundDetail = lazy(() => import('../pages/RoundDetail'))
const Questions = lazy(() => import('../pages/Questions'))
const QuestionDetail = lazy(() => import('../pages/QuestionDetail'))
const NewQuestion = lazy(() => import('../pages/NewQuestion'))
const Settings = lazy(() => import('../pages/Settings'))
const LlmSettings = lazy(() => import('../pages/LlmSettings'))
const PromptSettings = lazy(() => import('../pages/PromptSettings'))
const DataPage = lazy(() => import('../pages/DataPage'))
const PointsPage = lazy(() => import('../pages/Points'))
const ApplicationDetail = lazy(() => import('../pages/ApplicationDetail'))
const Skills = lazy(() => import('../pages/Skills'))

type NavIcon = React.ReactElement<IconProps>

const NAV: { to: string; label: string; icon: NavIcon; end?: boolean }[] = [
  { to: '/', label: '求职台', icon: <Gauge />, end: true },
  { to: '/applications', label: '投递', icon: <PaperPlaneTilt /> },
  { to: '/drills', label: '陪练', icon: <ChatsCircle /> },
  { to: '/resume', label: '简历', icon: <FileText /> },
  { to: '/companies', label: '企业', icon: <Buildings /> },
  { to: '/questions', label: '题库', icon: <ListChecks /> },
  { to: '/skills', label: '图谱', icon: <TreeStructure /> },
  { to: '/data', label: '数据', icon: <ChartBar /> },
  { to: '/points', label: '积分', icon: <Coin /> },
  { to: '/settings', label: '设置', icon: <Gear /> },
]

// ADR-0015 M1：Layout 壳迁移至设计语言 v2（Tailwind 语义 token + Phosphor）。
// 路由契约不变；内容容器契约对齐 legacy .content > *（见 index.css .app-main-inner）。
export default function Layout({ user, onLogout }: { user: User; onLogout: () => void }) {
  const [balance, setBalance] = useState<number | null>(null)
  const [theme, setThemeState] = useState<Theme>(() => getTheme())
  useEffect(() => {
    apiGet('/api/points/balance')
      .then((b) => setBalance(b.balance))
      .catch(() => {})
  }, [])
  useEffect(() => {
    connectAiEvents()
  }, [])

  const globalBatch = useGlobalBatchAnalysis()

  const link = (to: string, label: string, icon: NavIcon, end = false) => (
    <NavLink
      to={to}
      end={end}
      className={({ isActive }) =>
        cn(
          'flex h-9 min-h-[36px] cursor-pointer items-center gap-2.5 whitespace-nowrap rounded-md px-3 text-sm font-medium transition-colors duration-150 md:h-8',
          isActive
            ? 'nav-item-active'
            : 'text-foreground/80 hover:bg-muted hover:text-heading',
        )
      }
    >
      {({ isActive }: { isActive: boolean }) => (
        <>
          {cloneElement(icon, {
            weight: isActive ? 'bold' : 'regular',
            className: cn('size-[18px] shrink-0', isActive ? 'text-accent' : undefined),
            'aria-hidden': true,
          })}
          <span>{label}</span>
        </>
      )}
    </NavLink>
  )

  return (
    <div className="flex min-h-screen flex-col bg-background text-foreground md:flex-row">
      <aside className="flex shrink-0 flex-col border-b border-border bg-card md:sticky md:top-0 md:h-screen md:w-56 md:border-b-0 md:border-r">
        <div className="flex items-center gap-2.5 px-3.5 py-3 md:px-3 md:py-4">
          <BrandMark className="size-8 shrink-0 rounded-md" />
          <div className="min-w-0">
            <div className="text-[15px] font-semibold tracking-tight text-heading">Beview</div>
            <div className="hidden truncate text-[10px] leading-tight tracking-wide text-heading/70 md:block">
              Be Ready, Review Better.
            </div>
          </div>
        </div>

        <nav
          aria-label="主导航"
          className="flex gap-1 overflow-x-auto px-3 pb-2 md:flex-1 md:flex-col md:gap-0.5 md:overflow-y-auto md:px-2 md:pb-3"
        >
          {NAV.map((item) => link(item.to, item.label, item.icon, item.end))}
        </nav>

        {globalBatch && globalBatch.status === 'running' && (
          <div className="px-3 pb-2">
            <NavLink
              to="/questions"
              className="flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-all hover:bg-accent/20 chip-accent-selected"
              title="点击返回题库查看批量分析进度"
            >
              <Sparkle weight="fill" className="size-3.5 animate-spin text-accent shrink-0" />
              <span className="truncate">
                批量分析中 ({globalBatch.done}/{globalBatch.total})
              </span>
            </NavLink>
          </div>
        )}

        <div className="flex items-center gap-2 border-t border-border px-3 py-2 md:flex-col md:items-stretch md:gap-1 md:py-3">
          <NavLink
            to="/points"
            title="积分余额（见积分页）"
            className="flex h-9 min-w-0 cursor-pointer items-center gap-2 rounded-md px-2 text-sm transition-colors duration-150 hover:bg-muted hover:text-heading"
          >
            <Coin className="size-4 shrink-0" aria-hidden />
            <span className="truncate">
              积分{' '}
              <b className="font-mono tabular-nums text-foreground">{balance ?? '…'}</b>
            </span>
          </NavLink>
          <div className="ml-auto flex items-center gap-1 md:ml-0">
            <button
              onClick={() => setThemeState(toggleTheme())}
              aria-label={theme === 'dark' ? '切换到亮色主题' : '切换到暗色主题'}
              title={theme === 'dark' ? '切换到亮色主题' : '切换到暗色主题'}
              className="grid size-9 cursor-pointer place-items-center rounded-md text-foreground transition-colors duration-150 hover:bg-muted hover:text-heading"
            >
              {theme === 'dark' ? (
                <Sun className="size-[18px]" aria-hidden />
              ) : (
                <Moon className="size-[18px]" aria-hidden />
              )}
            </button>
            <div
              className="flex min-w-0 items-center gap-2 rounded-md px-1.5 py-1"
              title={user.username}
            >
              <span className="grid size-7 shrink-0 place-items-center rounded-full bg-secondary text-xs font-bold text-secondary-foreground">
                {user.username.slice(0, 1).toUpperCase()}
              </span>
              <span className="hidden truncate text-sm font-medium lg:inline">
                {user.username}
              </span>
            </div>
            <button
              onClick={onLogout}
              aria-label="退出登录"
              title="退出登录"
              className="grid size-9 shrink-0 cursor-pointer place-items-center rounded-md text-foreground transition-colors duration-150 hover:bg-muted hover:text-destructive"
            >
              <SignOut className="size-[18px]" aria-hidden />
            </button>
          </div>
        </div>
      </aside>

      <main className="min-w-0 flex-1">
        <div className="app-main-inner mx-auto w-full px-4 pb-16 pt-5 md:px-8 md:pb-20 md:pt-6">
          <Suspense
            fallback={
              <div className="space-y-2 py-8">
                <Skeleton className="h-8 w-48" />
                <Skeleton className="h-64 w-full" />
              </div>
            }
          >
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/review" element={<Review />} />
            <Route path="/review/wrong" element={<ReviewWrong />} />
            <Route path="/drills" element={<Drills />} />
            <Route path="/drills/new" element={<DrillNew />} />
            <Route path="/drills/:id" element={<DrillSession />} />
            <Route path="/resume" element={<ResumePage />} />
            <Route path="/points" element={<PointsPage />} />
            <Route path="/data" element={<DataPage />} />
            <Route path="/applications" element={<Applications />} />
            <Route path="/applications/:id" element={<ApplicationDetail />} />
            <Route path="/companies" element={<Companies />} />
            <Route path="/companies/:id" element={<CompanyDetail />} />
            <Route path="/positions/:id" element={<PositionDetail />} />
            <Route path="/rounds/:id" element={<RoundDetail />} />
            <Route path="/questions" element={<Questions />} />
            <Route path="/questions/:id" element={<QuestionDetail />} />
            <Route path="/skills" element={<Skills />} />
            <Route path="/new" element={<NewQuestion />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/settings/llm" element={<LlmSettings />} />
            <Route path="/settings/prompts" element={<PromptSettings />} />
            <Route
              path="*"
              element={
                <div className="py-24 text-center text-muted-foreground">404 · 页面不存在</div>
              }
            />
          </Routes>
          </Suspense>
        </div>
      </main>

      <Toaster position="top-center" />
    </div>
  )
}
