// 全局 trace id：页面加载时生成一次，全链路 trace 的根
const TRACE_ID = Array.from({ length: 16 }, () =>
  Math.floor(Math.random() * 256).toString(16).padStart(2, '0'),
).join('')

function spanId(): string {
  return Array.from({ length: 8 }, () =>
    Math.floor(Math.random() * 256).toString(16).padStart(2, '0'),
  ).join('')
}

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

async function request(path: string, init: RequestInit = {}): Promise<any> {
  const headers = new Headers(init.headers)
  headers.set('traceparent', `00-${TRACE_ID}-${spanId()}-01`)
  if (init.body) headers.set('Content-Type', 'application/json')
  const res = await fetch(path, {
    ...init,
    credentials: 'include',
    headers,
  })
  const text = await res.text()
  let data: any = null
  try {
    data = text ? JSON.parse(text) : null
  } catch {
    data = null
  }
  if (!res.ok) {
    const msg = data?.error || data?.message || res.statusText || '请求失败'
    // 未授权(401)：除认证探测外，记录目标路径并派发应用内事件，由 App 切到登录态
    // （不用 window.location.href 硬跳转，避免浏览器级重定向报"被重定向"/整页刷新）
    if (res.status === 401) {
      const probe = path === '/api/me' || path === '/api/setup/status' || path === '/api/login'
      if (!probe) {
        sessionStorage.setItem('beview_redirect', window.location.pathname + window.location.search)
        window.dispatchEvent(new Event('beview:unauthorized'))
      }
    }
    throw new ApiError(res.status, msg)
  }
  return data
}

export const apiGet = (p: string) => request(p)
export const apiPost = (p: string, body?: unknown) =>
  request(p, { method: 'POST', body: JSON.stringify(body ?? {}) })
export const apiPut = (p: string, body?: unknown) =>
  request(p, { method: 'PUT', body: JSON.stringify(body ?? {}) })
export const apiPatch = (p: string, body: unknown) =>
  request(p, { method: 'PATCH', body: JSON.stringify(body) })
export const apiDelete = (p: string, body?: unknown) =>
  request(p, { method: 'DELETE', body: body === undefined ? undefined : JSON.stringify(body) })

/**
 * SSE 流式请求（v2：AI 讲解 / 模拟面试）。
 * onEvent(event, dataJson)：event 为后端事件名，dataJson 为原始 data 字符串（前端自行 JSON.parse）。
 * 401 与非 2xx 与普通请求一致处理（派发 beview:unauthorized / 抛 ApiError）。
 *
 * v3 M0：断线自动重连——连接在收到任何事件之前中断（fetch 网络错误 / 空流），
 * 自动重试最多 opts.retries 次（指数退避），每次重连回调 onReconnect(attempt)。
 * 若已收到事件后中断，不自动重试（避免重复内容），交由调用方提示手动重试。
 */
export async function apiStream(
  path: string,
  body: unknown,
  onEvent: (event: string, dataJson: string) => void,
  opts: { retries?: number; onReconnect?: (attempt: number) => void } = {},
): Promise<void> {
  const maxRetries = opts.retries ?? 3
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
  for (let attempt = 0; ; attempt++) {
    const out = await streamOnce(path, body, onEvent)
    if (out.terminal || out.error === null) {
      if (out.error) throw out.error
      return
    }
    // 收到过事件 -> 不自动重试（防重复），把错误抛给调用方
    if (out.receivedAny || attempt >= maxRetries) throw out.error
    opts.onReconnect?.(attempt + 1)
    await sleep(600 * 2 ** attempt)
  }
}

async function streamOnce(
  path: string,
  body: unknown,
  onEvent: (event: string, dataJson: string) => void,
): Promise<{ terminal: boolean; receivedAny: boolean; error: Error | null }> {
  const res = await fetch(path, {
    method: 'POST',
    credentials: 'include',
    headers: {
      'Content-Type': 'application/json',
      traceparent: `00-${TRACE_ID}-${spanId()}-01`,
    },
    body: JSON.stringify(body ?? {}),
  })
  if (!res.ok) {
    let msg = res.statusText || '请求失败'
    try {
      const d = await res.json()
      msg = d?.error || msg
    } catch {
      /* ignore */
    }
    if (res.status === 401) {
      sessionStorage.setItem('beview_redirect', window.location.pathname + window.location.search)
      window.dispatchEvent(new Event('beview:unauthorized'))
    }
    throw new ApiError(res.status, msg)
  }
  if (!res.body) throw new ApiError(0, '浏览器不支持流式读取')
  const reader = res.body.getReader()
  const dec = new TextDecoder()
  let buf = ''
  let receivedAny = false
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buf += dec.decode(value, { stream: true })
      let sep: number
      while ((sep = buf.indexOf('\n\n')) !== -1) {
        const raw = buf.slice(0, sep)
        buf = buf.slice(sep + 2)
        let event = 'message'
        let data = ''
        for (const line of raw.split('\n')) {
          if (line.startsWith('event:')) event = line.slice(6).trim()
          else if (line.startsWith('data:')) data += line.slice(5).trim()
        }
        if (event !== 'message' || data) {
          receivedAny = true
          onEvent(event, data)
        }
      }
    }
  } catch (e) {
    return { terminal: false, receivedAny, error: e instanceof Error ? e : new Error('流读取中断') }
  }
  // 正常 EOF（后端已发完）；是否收到终止事件由调用方的事件语义决定，这里视为终端
  return { terminal: true, receivedAny, error: null }
}
