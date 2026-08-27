import { useCallback, useEffect, useRef, useState } from 'react'

export function getStoredStreamSpeedRate(): number {
  if (typeof window === 'undefined') return 20
  const val = localStorage.getItem('beview_ai_stream_speed_rate')
  if (val) {
    const num = Number(val)
    if (!isNaN(num) && num >= 10 && num <= 210) {
      return num
    }
  }
  // 兼容旧枚举配置
  const oldVal = localStorage.getItem('beview_ai_stream_speed')
  if (oldVal === 'slow') return 10
  if (oldVal === 'normal') return 20
  if (oldVal === 'fast') return 40
  if (oldVal === 'instant') return 210
  return 20
}

export function setStoredStreamSpeedRate(rate: number) {
  if (typeof window !== 'undefined') {
    localStorage.setItem('beview_ai_stream_speed_rate', String(rate))
  }
}

const segmenter =
  typeof Intl !== 'undefined' && (Intl as any).Segmenter
    ? new (Intl as any).Segmenter(undefined, { granularity: 'grapheme' })
    : null

function splitGraphemes(text: string): string[] {
  if (!segmenter) return Array.from(text)
  return Array.from(segmenter.segment(text), (s: any) => s.segment)
}

const PUNCTUATION_REGEX = /[，。！？；：\n,.!?;:]/

/**
 * 高精度自适应平滑流式输出 Hook：
 * - 接收上游 delta chunk，平滑匀速逐字渲染
 * - 采用 Unicode Grapheme 分词 + 标点自然节奏停顿
 * - 采用浮点字符累加器，精确实现最低 10 到 200 字/秒，超过 200 为无限制立即输出
 */
export function useSmoothStream(options?: { rateCharsPerSec?: number }) {
  const [displayedText, setDisplayedText] = useState('')
  const [isDrained, setIsDrained] = useState(true)
  const targetTextRef = useRef('')
  const targetGraphemesRef = useRef<string[]>([])
  const currentGraphemeIndexRef = useRef(0)
  const currentTextRef = useRef('')
  const pauseRemainingMs = useRef(0)
  const fracAccumulatorRef = useRef(0)
  const timerRef = useRef<number | null>(null)
  const isFinishedRef = useRef(false)
  const drainResolverRef = useRef<(() => void) | null>(null)
  const configuredRate = options?.rateCharsPerSec ?? getStoredStreamSpeedRate()

  const lastTimeRef = useRef(0)
  const stepRef = useRef<((time: number) => void) | null>(null)

  const clear = useCallback(() => {
    targetTextRef.current = ''
    targetGraphemesRef.current = []
    currentGraphemeIndexRef.current = 0
    currentTextRef.current = ''
    pauseRemainingMs.current = 0
    fracAccumulatorRef.current = 0
    isFinishedRef.current = false
    setIsDrained(true)
    if (drainResolverRef.current) {
      drainResolverRef.current()
      drainResolverRef.current = null
    }
    if (timerRef.current != null) {
      cancelAnimationFrame(timerRef.current)
      timerRef.current = null
    }
    setDisplayedText('')
  }, [])

  const appendChunk = useCallback((chunk: string) => {
    targetTextRef.current += chunk
    targetGraphemesRef.current = splitGraphemes(targetTextRef.current)
    setIsDrained(false)
    if (timerRef.current == null && stepRef.current != null) {
      lastTimeRef.current = performance.now()
      timerRef.current = requestAnimationFrame(stepRef.current)
    }
  }, [])

  const finishStream = useCallback(() => {
    isFinishedRef.current = true
    if (currentGraphemeIndexRef.current >= targetGraphemesRef.current.length) {
      setIsDrained(true)
      if (drainResolverRef.current) {
        drainResolverRef.current()
        drainResolverRef.current = null
      }
      if (timerRef.current != null) {
        cancelAnimationFrame(timerRef.current)
        timerRef.current = null
      }
    }
  }, [])

  const waitUntilDrained = useCallback((): Promise<void> => {
    if (isFinishedRef.current && currentGraphemeIndexRef.current >= targetGraphemesRef.current.length) {
      return Promise.resolve()
    }
    return new Promise((resolve) => {
      drainResolverRef.current = resolve
    })
  }, [])

  useEffect(() => {
    lastTimeRef.current = performance.now()

    const step = (time: number) => {
      const delta = Math.min(100, Math.max(0, time - lastTimeRef.current)) // 限制单帧最大时间防休眠跳帧
      lastTimeRef.current = time

      const target = targetTextRef.current
      const targetGraphemes = targetGraphemesRef.current
      const currentIndex = currentGraphemeIndexRef.current
      const remainingGraphemes = targetGraphemes.length - currentIndex

      if (remainingGraphemes > 0) {
        if (configuredRate >= 210) {
          currentGraphemeIndexRef.current = targetGraphemes.length
          currentTextRef.current = target
          setDisplayedText(target)
        } else {
          // 标点符号轻微节奏停顿（模拟真实人类说话思维呼吸节奏）
          if (pauseRemainingMs.current > 0) {
            pauseRemainingMs.current -= delta
          } else {
            let rate = configuredRate / 1000 // 字符/毫秒

            // 适度追赶大缓冲区（避免严重堆积）
            if (remainingGraphemes > 150) {
              rate *= 2.2
            } else if (remainingGraphemes > 60) {
              rate *= 1.5
            }

            // 当流已结束且仅剩几个字时自然收尾
            if (isFinishedRef.current && remainingGraphemes < 5) {
              rate *= 1.5
            }

            fracAccumulatorRef.current += delta * rate
            if (fracAccumulatorRef.current >= 1) {
              const toAdvance = Math.min(remainingGraphemes, Math.floor(fracAccumulatorRef.current))
              fracAccumulatorRef.current -= toAdvance

              const nextIndex = currentIndex + toAdvance
              currentGraphemeIndexRef.current = nextIndex
              const nextText = targetGraphemes.slice(0, nextIndex).join('')

              // 检测本次输出的末尾是否为标点，若是且语速不超高时加入 25~50ms 拟真停顿
              const lastChar = targetGraphemes[nextIndex - 1] || ''
              if (PUNCTUATION_REGEX.test(lastChar) && configuredRate <= 60 && remainingGraphemes > 5) {
                pauseRemainingMs.current = lastChar === '\n' || lastChar === '。' || lastChar === '！' ? 50 : 25
              }

              currentTextRef.current = nextText
              setDisplayedText(nextText)
            }
          }
        }
      }

      if (currentGraphemeIndexRef.current >= targetGraphemesRef.current.length && targetGraphemesRef.current.length > 0) {
        if (isFinishedRef.current) {
          setIsDrained(true)
          if (drainResolverRef.current) {
            drainResolverRef.current()
            drainResolverRef.current = null
          }
          timerRef.current = null
          return // 完成且输出完毕，停止继续调度 rAF
        }
      }

      timerRef.current = requestAnimationFrame(step)
    }

    stepRef.current = step
    if (timerRef.current == null && (targetTextRef.current.length > currentTextRef.current.length || !isFinishedRef.current)) {
      timerRef.current = requestAnimationFrame(step)
    }

    return () => {
      if (timerRef.current != null) {
        cancelAnimationFrame(timerRef.current)
        timerRef.current = null
      }
      stepRef.current = null
    }
  }, [configuredRate])

  return {
    displayedText,
    isDrained,
    appendChunk,
    finishStream,
    waitUntilDrained,
    clear,
  }
}
