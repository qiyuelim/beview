import {
  CheckCircleIcon,
  InfoIcon,
  CircleNotchIcon,
  XCircleIcon,
  WarningCircleIcon,
} from "@phosphor-icons/react"
import { Toaster as Sonner, type ToasterProps } from "sonner"

// ADR-0015：不依赖 next-themes；主题由应用在 <html> 上挂 .dark 类管理，
// 渲染时读取当前类判定，跟随系统交给未来的 ThemeProvider 增强。
const currentTheme = (): NonNullable<ToasterProps["theme"]> =>
  typeof document !== "undefined" &&
  document.documentElement.classList.contains("dark")
    ? "dark"
    : "light"

const Toaster = ({ ...props }: ToasterProps) => {
  return (
    <Sonner
      theme={currentTheme()}
      className="toaster group"
      icons={{
        success: <CheckCircleIcon className="size-4" weight="fill" />,
        info: <InfoIcon className="size-4" weight="fill" />,
        warning: <WarningCircleIcon className="size-4" weight="fill" />,
        error: <XCircleIcon className="size-4" weight="fill" />,
        loading: <CircleNotchIcon className="size-4 animate-spin" />,
      }}
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      {...props}
    />
  )
}

export { Toaster }
