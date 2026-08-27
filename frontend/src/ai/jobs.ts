/**
 * 全局 AI 任务中心（ADR-0013）：跨页面/刷新跟踪进行中的 AI 任务。
 * - 触发：startAiJob() 受理后登记任务（同目标重复触发由后端幂等去重）；
 * - 回显：EventSource 订阅 /api/events（实时，含批量分析与陪练终态）；
 *   评审整改：删除了从未接线的「3s 轮询兑底」死代码——SSE 断线由 EventSource 自动重连兜底，
 *   刷新恢复走各域 GET 的 ai_jobs[] 字段（trackRunning）。
 * - 刷新恢复：各域 GET 响应带 ai_jobs[]，页面 load 后 trackRunning() 即恢复「进行中」展示；
 * - 完成：移除任务、通知订阅组件、执行页面注册的 onDone 回调（reload 数据）。
 */

export type AiKind =
  | 'ref'
  | 'analyze'
  | 'jd_interpret'
  | 'jd_match'
  | 'resume_parse'
  | 'retrospective'
  | 'overall'
  | 'app_insights'
  | 'interview_prep'
  | 'position_predict'

export interface TrackedJob {
  jobId: number
  kind: AiKind
  targetId: number
}

/** SSE 帧结构（后端 AiEvent 的 JSON 形态） */
export interface AiEventFrame {
  job_id: number
  kind: string
  target_id: number
  status: string
}

const jobs = new Map<string, TrackedJob>() // key: kind:targetId
const listeners = new Set<() => void>()
const doneCbs = new Map<string, ((ok: boolean) => void)[]>() // key -> callbacks
const eventListeners = new Set<(ev: AiEventFrame) => void>() // 原始帧监听（如 interview_prep 终态）
let snapshot: TrackedJob[] = []
let es: EventSource | null = null

function emitChange() {
  snapshot = Array.from(jobs.values())
  listeners.forEach((l) => l())
}

function keyOf(kind: string, targetId: number) {
  return `${kind}:${targetId}`
}

function finish(jobId: number, ok: boolean, known?: { kind: string; target_id: number }) {
  let j: TrackedJob | undefined
  for (const t of jobs.values()) {
    if (t.jobId === jobId) {
      j = t
      break
    }
  }
  if (!j && known) {
    // SSE 事件可能先于跟踪注册（他页触发的任务），按事件信息直接完成回调分发
    j = { jobId, kind: known.kind as AiKind, targetId: known.target_id }
  }
  if (!j) return
  const k = keyOf(j.kind, j.targetId)
  if (jobs.delete(k)) emitChange()
  ;(doneCbs.get(k) ?? []).forEach((cb) => {
    try {
      cb(ok)
    } catch {
      /* 页面回调异常不阻断其他回调 */
    }
  })
  doneCbs.delete(k)
}

export function track(kind: AiKind, targetId: number, jobId: number) {
  const k = keyOf(kind, targetId)
  if (jobs.has(k)) return
  jobs.set(k, { jobId, kind, targetId })
  emitChange()
}

/** 域 GET 的 ai_jobs 字段恢复跟踪（刷新/进页时） */
export function trackRunning(list?: { id: number; kind: string; target_id: number }[] | null) {
  for (const j of list ?? []) {
    track(j.kind as AiKind, j.target_id, j.id)
  }
}

/** 触发 AI 任务：POST 受理后登记跟踪；同步校验错误原样抛给调用方展示 */
export async function startAiJob(kind: AiKind, targetId: number, path: string): Promise<any> {
  const { apiPost } = await import('../api/client')
  const resp = await apiPost(path)
  if (resp && typeof resp.job_id === 'number') {
    track(kind, targetId, resp.job_id)
  }
  return resp
}

/** 页面注册任务完成回调（自动去重；返回反注册函数，供 useEffect 清理） */
export function onJobDone(kind: AiKind, targetId: number, cb: (ok: boolean) => void): () => void {
  const k = keyOf(kind, targetId)
  const list = doneCbs.get(k) ?? []
  list.push(cb)
  doneCbs.set(k, list)
  return () => {
    const cur = doneCbs.get(k) ?? []
    doneCbs.set(
      k,
      cur.filter((f) => f !== cb),
    )
  }
}

const batchItemCbs = new Set<(qid: number, ok: boolean) => void>()

export function onBatchItemDone(cb: (qid: number, ok: boolean) => void) {
  batchItemCbs.add(cb)
  return () => batchItemCbs.delete(cb)
}

/** 原始 SSE 帧监听（评审新增）：供非 AiJob 注册表的事件消费（如陪练备课终态） */
export function onAiEvent(cb: (ev: AiEventFrame) => void): () => void {
  eventListeners.add(cb)
  return () => eventListeners.delete(cb)
}

/** 登录后调用一次：建立 SSE 长连接（EventSource 自动重连） */
export function connectAiEvents() {
  if (es || typeof EventSource === 'undefined') return
  es = new EventSource('/api/events', { withCredentials: true })
  es.onmessage = (ev) => {
    try {
      const d = JSON.parse(ev.data) as AiEventFrame
      eventListeners.forEach((l) => l(d))
      
      // 批量分析整批状态变化
      if (d.kind === 'batch_analyze') {
        if (d.status === 'running') {
          batchState = {
            jobId: d.job_id,
            total: d.target_id || (batchState ? batchState.total : 0),
            done: 0,
            ok: 0,
            failed: 0,
            status: 'running',
          }
        } else if (batchState) {
          batchState = {
            ...batchState,
            jobId: d.job_id,
            status: d.status as any,
          }
        }
        batchListeners.forEach((l) => l())
        return
      }

      // 批量分析单题完成事件
      if (d.kind === 'batch_item_done') {
        const qid = d.target_id
        const ok = d.status === 'done'
        if (batchState) {
          batchState = {
            ...batchState,
            jobId: d.job_id,
            done: batchState.done + 1,
            ok: batchState.ok + (ok ? 1 : 0),
            failed: batchState.failed + (ok ? 0 : 1),
          }
          batchListeners.forEach((l) => l())
        }
        batchItemCbs.forEach((cb) => cb(qid, ok))
        return
      }

      // 单题/常规 AI 任务
      if (d.status === 'running') {
        const k = keyOf(d.kind, d.target_id)
        if (!jobs.has(k)) track(d.kind as AiKind, d.target_id, d.job_id)
      } else {
        finish(d.job_id, d.status === 'done', { kind: d.kind, target_id: d.target_id })
      }
    } catch {
      /* 忽略无法解析的帧 */
    }
  }
}

export function disconnectAiEvents() {
  es?.close()
  es = null
}

function subscribe(cb: () => void) {
  listeners.add(cb)
  return () => listeners.delete(cb)
}

function getSnapshot() {
  return snapshot
}

import { useSyncExternalStore } from 'react'

/** React 绑定：当前进行中的 AI 任务列表（不可变快照） */
export function useAiJobs(): TrackedJob[] {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

export function isRunning(list: TrackedJob[], kind: AiKind, targetId?: number): boolean {
  return list.some((j) => j.kind === kind && (targetId === undefined || j.targetId === targetId))
}

export interface BatchAnalysisState {
  jobId: number
  total: number
  done: number
  ok: number
  failed: number
  status: 'running' | 'done' | 'cancelled' | 'error'
}

let batchState: BatchAnalysisState | null = null
const batchListeners = new Set<() => void>()

export function setGlobalBatchAnalysis(st: BatchAnalysisState | null) {
  batchState = st
  batchListeners.forEach((l) => l())
}

function getBatchSnapshot() {
  return batchState
}

function subscribeBatch(cb: () => void) {
  batchListeners.add(cb)
  return () => batchListeners.delete(cb)
}

export function useGlobalBatchAnalysis(): BatchAnalysisState | null {
  return useSyncExternalStore(subscribeBatch, getBatchSnapshot, getBatchSnapshot)
}
