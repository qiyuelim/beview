import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { apiDelete, apiGet, apiPatch, apiPost, apiPut } from '../api/client'
import type { LlmConfigDoc, LlmModel, LlmProvider, LlmResolved } from '../api/types'

/** 额外参数的行草稿（key/value 均为输入框原文；写回 doc 时做类型识别） */
interface KvRow {
  k: string
  v: string
}

/** 标量识别：true/false → 布尔，数字字面量 → 数值，合法 JSON → 解析，否则按字符串 */
function parseScalar(raw: string): unknown {
  const t = raw.trim()
  if (t === 'true') return true
  if (t === 'false') return false
  if (t !== '' && !Number.isNaN(Number(t))) return Number(t)
  try {
    return JSON.parse(t)
  } catch {
    return raw
  }
}
import { ArrowLeft, CaretDown, CheckCircle, CircleNotch, Plus, Sparkle, Trash, Warning } from '@phosphor-icons/react'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { FormField } from '../components/FormField'
import { SemBadge } from '../components/SemBadge'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { cn } from '@/lib/utils'
import { toast } from 'sonner'
import { getStoredStreamSpeedRate, setStoredStreamSpeedRate } from '../hooks/useSmoothStream'

const EFFORTS = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const
const EFFORT_LABELS: Record<string, string> = {
  none: 'none · 不下发（关）',
  minimal: 'minimal · 标准档',
  low: 'low · 标准档',
  medium: 'medium · 标准档（默认）',
  high: 'high · 标准档',
  xhigh: 'xhigh · 扩展档',
  max: 'max · 扩展档',
}

function genId(prefix: string): string {
  return `${prefix}_${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`
}

function emptyDoc(): LlmConfigDoc {
  return {
    providers: [],
    models: [],
    active_model_id: null,
    global: { timeout: 120, max_output_tokens_short: 4096, max_output_tokens_long: 8192 },
  }
}

export default function LlmSettings() {
  const navigate = useNavigate()
  const [doc, setDoc] = useState<LlmConfigDoc>(emptyDoc)
  const [resolved, setResolved] = useState<LlmResolved | null>(null)
  const [resolveError, setResolveError] = useState('')
  const [expanded, setExpanded] = useState<string>('')
  const [kvDrafts, setKvDrafts] = useState<Record<string, KvRow[]>>({})
  const [delTarget, setDelTarget] = useState<{ kind: 'provider' | 'model'; id: string; name: string } | null>(null)
  const [saving, setSaving] = useState(false)
  const [testingId, setTestingId] = useState<string>('')
  const [testResult, setTestResult] = useState<{ id: string; ok: boolean; msg: string } | null>(null)
  const [err, setErr] = useState('')
  const [speedRate, setSpeedRate] = useState<number>(() => getStoredStreamSpeedRate())

  useEffect(() => {
    apiGet('/api/settings/llm-config')
      .then((d) => {
        setDoc({ ...emptyDoc(), ...d.config })
        setResolved(d.resolved ?? null)
        setResolveError(d.resolve_error ?? '')
        setKvDrafts({})
      })
      .catch((e) => setErr(e.message))
  }, [])

  function patch(fn: (d: LlmConfigDoc) => void) {
    setDoc((prev) => {
      const next = structuredClone(prev)
      fn(next)
      return next
    })
    setTestResult(null)
  }

  function addProvider() {
    patch((d) => {
      d.providers.push({ id: genId('p'), name: '', base_url: '', api_key: '' })
    })
  }

  function addModel(providerId: string) {
    patch((d) => {
      const id = genId('m')
      d.models.push({
        id,
        provider_id: providerId,
        name: '',
        context_length: null,
        caps: { structured_output: true, web_search: false },
        advanced: { reasoning_effort: 'medium', store: false, extra_body: {} },
      })
      setExpanded(id) // 新建即展开
      if (!d.active_model_id) d.active_model_id = id
    })
  }

  async function removeProvider(id: string) {
    setErr('')
    try {
      await apiDelete(`/api/settings/llm-config/providers/${id}`)
      patch((d) => {
        d.providers = d.providers.filter((p) => p.id !== id)
        const gone = new Set(d.models.filter((m) => m.provider_id === id).map((m) => m.id))
        d.models = d.models.filter((m) => m.provider_id !== id)
        if (gone.has(d.active_model_id ?? '')) d.active_model_id = d.models[0]?.id ?? null
      })
      toast.success('已删除 Provider')
    } catch (e: any) {
      setErr(e.message)
      toast.error(`删除失败: ${e.message}`)
    } finally {
      setDelTarget(null)
    }
  }

  async function removeModel(id: string) {
    setErr('')
    try {
      await apiDelete(`/api/settings/llm-config/models/${id}`)
      patch((d) => {
        d.models = d.models.filter((m) => m.id !== id)
        if (d.active_model_id === id) d.active_model_id = d.models[0]?.id ?? null
      })
      toast.success('已删除模型')
    } catch (e: any) {
      setErr(e.message)
      toast.error(`删除失败: ${e.message}`)
    } finally {
      setDelTarget(null)
    }
  }

  async function save(): Promise<boolean> {
    setErr('')
    setSaving(true)
    try {
      await apiPut('/api/settings/llm-config', doc)
      const d = await apiGet('/api/settings/llm-config')
      setDoc({ ...emptyDoc(), ...d.config })
      setResolved(d.resolved ?? null)
      setKvDrafts({})
      toast.success('已保存全部 LLM 配置')
      return true
    } catch (e: any) {
      setErr(e.message)
      return false
    } finally {
      setSaving(false)
    }
  }

  function commitKv(m: LlmModel): Record<string, unknown> {
    const obj: Record<string, unknown> = {}
    for (const r of kvRowsOf(m)) {
      const k = r.k.trim()
      if (!k) continue
      obj[k] = parseScalar(r.v)
    }
    return obj
  }

  async function saveProvider(p: LlmProvider) {
    setErr('')
    try {
      try {
        await apiPatch(`/api/settings/llm-config/providers/${p.id}`, p)
      } catch (e: any) {
        if (e.status === 404 || e.message?.includes('not found') || e.message?.includes('未找到')) {
          const created = await apiPost('/api/settings/llm-config/providers', p)
          if (created?.id && created.id !== p.id) {
            patch((d) => {
              const item = d.providers.find((x) => x.id === p.id)
              if (item) item.id = created.id
            })
          }
        } else {
          throw e
        }
      }
      toast.success(`已保存 Provider「${p.name || '未命名'}」`)
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function saveModel(mi: number): Promise<LlmModel | null> {
    const m = doc.models[mi]
    setErr('')
    try {
      const payload: LlmModel = {
        ...m,
        advanced: { ...m.advanced, extra_body: commitKv(m) },
      }
      try {
        await apiPatch(`/api/settings/llm-config/models/${m.id}`, payload)
      } catch (e: any) {
        if (e.status === 404 || e.message?.includes('not found') || e.message?.includes('未找到')) {
          const created = await apiPost('/api/settings/llm-config/models', payload)
          if (created?.id && created.id !== m.id) {
            patch((d) => {
              const item = d.models.find((x) => x.id === m.id)
              if (item) item.id = created.id
            })
          }
        } else {
          throw e
        }
      }
      setKvDrafts((d) => {
        const next = { ...d }
        delete next[m.id]
        return next
      })
      toast.success(`已保存模型「${m.name || '未命名'}」`)
      return payload
    } catch (e: any) {
      setErr(e.message)
      return null
    }
  }

  async function saveGlobal() {
    setErr('')
    try {
      await apiPatch('/api/settings/llm-config/global', doc.global)
      toast.success('已保存全局参数')
    } catch (e: any) {
      setErr(e.message)
    }
  }

  function kvRowsOf(m: LlmModel): KvRow[] {
    return (
      kvDrafts[m.id] ??
      Object.entries(m.advanced.extra_body ?? {}).map(([k, v]) => ({
        k,
        v: typeof v === 'string' ? v : JSON.stringify(v),
      }))
    )
  }

  function setKvRows(mi: number, m: LlmModel, rows: KvRow[]) {
    setKvDrafts((d) => ({ ...d, [m.id]: rows }))
    const obj: Record<string, unknown> = {}
    for (const r of rows) {
      const k = r.k.trim()
      if (!k) continue
      obj[k] = parseScalar(r.v)
    }
    patch((d) => void (d.models[mi].advanced.extra_body = obj))
  }

  async function testModel(m: LlmModel) {
    // 先局部保存该模型（保证测的是界面上的当前值），失败则中止
    const mi = doc.models.indexOf(m)
    if (!(await saveModel(mi))) return
    setTestingId(m.id)
    setTestResult(null)
    try {
      const r = await apiPost('/api/settings/llm-config/test', { model_id: m.id })
      setTestResult({ id: m.id, ok: true, msg: `连接成功：${r.provider} / ${r.model}` })
    } catch (e: any) {
      setTestResult({ id: m.id, ok: false, msg: `连接失败：${e.message}` })
    } finally {
      setTestingId('')
    }
  }

  return (
    <div className="mx-auto w-full max-w-[860px]">
      <PageHeader
        title={
          <span className="inline-flex items-center gap-2">
            <button
              onClick={() => navigate('/settings')}
              className="rounded-md p-1 hover:bg-muted"
              aria-label="返回设置"
            >
              <ArrowLeft className="size-4" aria-hidden />
            </button>
            LLM 配置
          </span>
        }
        meta={
          <span className="inline-flex items-center gap-1">
            <Sparkle className="size-3.5" weight="fill" aria-hidden /> Responses API · 多 Provider · 多 Model
          </span>
        }
      />
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      {/* 生效模型摘要（提示真实：未配置时中性提示，不误导；损坏时警示具体原因） */}
      <Section title="生效模型">
        {resolved ? (
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <CheckCircle weight="fill" className="size-4 text-success" aria-hidden />
            <span className="font-semibold">{resolved.provider}</span>
            <span aria-hidden>/</span>
            <span className="font-mono">{resolved.model}</span>
            {resolved.structured_output ? (
              <SemBadge sem="pass">结构化输出</SemBadge>
            ) : (
              <SemBadge sem="warn">纯文本评审</SemBadge>
            )}
            {resolved.web_search && <SemBadge sem="info">联网搜索</SemBadge>}
            {resolved.reasoning_effort && (
              <span className="text-xs text-muted-foreground">思考强度 {resolved.reasoning_effort}</span>
            )}
          </div>
        ) : resolveError ? (
          <div className="flex flex-wrap items-center gap-2 text-sm" role="alert">
            <Warning weight="fill" className="size-4 text-destructive" aria-hidden />
            <span className="font-medium text-destructive">LLM 配置存在问题：</span>
            <span className="text-muted-foreground">{resolveError}</span>
          </div>
        ) : (
          <p className="text-sm text-foreground">添加 Provider 与 Model 后保存，即可启用。</p>
        )}
      </Section>

      {/* Provider 卡片 × Model 行 */}
      {doc.providers.map((p, pi) => (
        <Section
          key={p.id}
          className="mt-4"
          title={
            <input
              value={p.name}
              onChange={(e) => patch((d) => void (d.providers[pi].name = e.target.value))}
              placeholder="Provider 名称，如 OpenAI"
              aria-label={`Provider ${pi + 1} 名称`}
              className="w-52 rounded-md border border-transparent bg-transparent px-1 py-0.5 text-sm font-semibold hover:border-border focus:border-ring focus:outline-none"
            />
          }
          action={
            <>
              <Button variant="ghost" size="sm" onClick={() => saveProvider(p)}>
                保存此 Provider
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setDelTarget({ kind: 'provider', id: p.id, name: p.name || '未命名' })}>
                <Trash className="size-3.5" aria-hidden /> 删除
              </Button>
            </>
          }
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <FormField label="Base URL" htmlFor={`p-url-${p.id}`} required className="sm:col-span-2">
              <Input
                id={`p-url-${p.id}`}
                value={p.base_url}
                onChange={(e) => patch((d) => void (d.providers[pi].base_url = e.target.value))}
                placeholder="https://api.openai.com/v1"
              />
            </FormField>
            <FormField
              label="API Key"
              htmlFor={`p-key-${p.id}`}
              hint={p.has_key ? `已配置 ${p.api_key}` : undefined}
            >
              <Input
                id={`p-key-${p.id}`}
                type="password"
                autoComplete="new-password"
                value={p.api_key}
                onChange={(e) => patch((d) => void (d.providers[pi].api_key = e.target.value))}
                placeholder={p.has_key ? `${p.api_key}（输入可覆盖）` : 'sk-…'}
              />
            </FormField>
          </div>

          {/* Model 行 */}
          <div className="mt-4 flex flex-col gap-2" role="group" aria-label={`${p.name || 'Provider'} 的模型`}>
            {doc.models
              .filter((m) => m.provider_id === p.id)
              .map((m) => {
                const mi = doc.models.indexOf(m)
                const isActive = doc.active_model_id === m.id
                const open = expanded === m.id
                return (
                  <div key={m.id} className={cn('rounded-lg border', isActive ? 'border-primary/50' : 'border-border')}>
                    {/* 行头：激活 / 名称 / 能力徽章 / 操作 */}
                    <div className="flex flex-wrap items-center gap-2 px-3 py-2.5">
                      <button
                        onClick={() => patch((d) => void (d.active_model_id = m.id))}
                        className={cn(
                          'flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium transition-colors',
                          isActive ? 'chip-accent-selected' : 'border border-border text-muted-foreground hover:text-foreground hover:bg-muted/50',
                        )}
                        aria-pressed={isActive}
                      >
                        {isActive ? '使用中' : '设为使用'}
                      </button>
                      <Input
                        value={m.name}
                        onChange={(e) => patch((d) => void (d.models[mi].name = e.target.value))}
                        placeholder="模型名，如 gpt-5.2"
                        aria-label="模型名"
                        className="h-8 w-44 font-mono text-[13px]"
                      />
                      {m.caps.structured_output ? (
                        <SemBadge sem="pass">结构化输出</SemBadge>
                      ) : (
                        <SemBadge sem="warn">纯文本评审</SemBadge>
                      )}
                      {m.caps.web_search && <SemBadge sem="info">联网搜索</SemBadge>}
                      <div className="ml-auto flex items-center gap-1.5">
                        <Button variant="secondary" size="sm" onClick={() => saveModel(mi)}>
                          保存此模型
                        </Button>
                        <Button variant="secondary" size="sm" onClick={() => testModel(m)} disabled={testingId === m.id}>
                          {testingId === m.id ? (
                            <>
                              <CircleNotch className="size-3.5 animate-spin" aria-hidden /> 测试中
                            </>
                          ) : (
                            '测试连接'
                          )}
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => setExpanded(open ? '' : m.id)}
                          aria-expanded={open}
                        >
                          <CaretDown className={cn('size-3.5 transition-transform', open && 'rotate-180')} aria-hidden />
                          高级参数
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          aria-label={`删除模型 ${m.name || '未命名'}`}
                          onClick={() => setDelTarget({ kind: 'model', id: m.id, name: m.name || '未命名' })}
                        >
                          <Trash className="size-3.5" aria-hidden />
                        </Button>
                      </div>
                    </div>

                    {/* 测试结果（就近展示） */}
                    {testResult?.id === m.id && (
                      <p
                        role="status"
                        className={cn(
                          'mx-3 mb-2 rounded-md px-3 py-2 text-sm font-medium',
                          testResult.ok ? 'bg-success/10 text-success' : 'bg-destructive/10 text-destructive',
                        )}
                      >
                        {testResult.msg}
                      </p>
                    )}

                    {/* 展开区：模型属性 + 能力位 + 高级参数 */}
                    {open && (
                      <div className="flex flex-col gap-3 border-t border-border px-3 py-3">
                        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                          <FormField label="上下文长度" htmlFor={`m-ctx-${m.id}`} hint="模型属性，仅记录与护栏用，不随请求发送；常见值：qwen3 1000000、gpt-5.2 400000">
                            <Input
                              id={`m-ctx-${m.id}`}
                              inputMode="numeric"
                              value={m.context_length ?? ''}
                              onChange={(e) =>
                                patch((d) => void (d.models[mi].context_length = e.target.value ? Number(e.target.value) : null))
                              }
                              placeholder="如 400000"
                            />
                          </FormField>
                          <div className="flex items-end gap-4 pb-1">
                            <label className="flex items-center gap-2 text-sm">
                              <Switch
                                checked={m.caps.structured_output}
                                onCheckedChange={(v) => patch((d) => void (d.models[mi].caps.structured_output = v))}
                              />
                              结构化输出
                            </label>
                            <label className="flex items-center gap-2 text-sm">
                              <Switch
                                checked={m.caps.web_search}
                                onCheckedChange={(v) => patch((d) => void (d.models[mi].caps.web_search = v))}
                              />
                              联网搜索
                              <span className="text-xs text-muted-foreground">（OpenAI 托管工具，非 OpenAI 上游请关闭）</span>
                            </label>
                          </div>
                        </div>
                        {!m.caps.structured_output && (
                          <p className="rounded-md bg-warning/10 px-3 py-2 text-xs font-medium text-warning">
                            已关闭结构化输出：题目分析/JD 解读/复盘等评审型出口将输出 Markdown 全文（不评分、不打标签）；
                            简历解析等结构必需任务将不可用。
                          </p>
                        )}
                        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
                          <FormField label="温度 temperature" htmlFor={`m-temp-${m.id}`} hint="0–2，留空用端点默认；推理模型常不支持，留空最稳">
                            <Input
                              id={`m-temp-${m.id}`}
                              inputMode="decimal"
                              value={m.advanced.temperature ?? ''}
                              onChange={(e) =>
                                patch(
                                  (d) =>
                                    void (d.models[mi].advanced.temperature =
                                      e.target.value === '' ? null : Number(e.target.value)),
                                )
                              }
                            />
                          </FormField>
                          <FormField label="top_p" htmlFor={`m-topp-${m.id}`} hint="0–1，留空用端点默认；与温度同理，推理模型建议留空">
                            <Input
                              id={`m-topp-${m.id}`}
                              inputMode="decimal"
                              value={m.advanced.top_p ?? ''}
                              onChange={(e) =>
                                patch((d) => void (d.models[mi].advanced.top_p = e.target.value === '' ? null : Number(e.target.value)))
                              }
                            />
                          </FormField>
                          <FormField
                            label="思考强度"
                            htmlFor={`m-effort-${m.id}`}
                            hint="经 reasoning.effort 下发；标准档各端通用，扩展档（xhigh/max）仅部分端点支持——报思考预算类错误请降到标准档或选 none"
                          >
                            <Select
                              value={m.advanced.reasoning_effort ?? ''}
                              onValueChange={(v) => patch((d) => void (d.models[mi].advanced.reasoning_effort = v as string))}
                            >
                              <SelectTrigger id={`m-effort-${m.id}`}>
                                <SelectValue placeholder="选择档位" />
                              </SelectTrigger>
                              <SelectContent>
                                {EFFORTS.map((e) => (
                                  <SelectItem key={e} value={e}>
                                    {EFFORT_LABELS[e]}
                                  </SelectItem>
                                ))}
                              </SelectContent>
                            </Select>
                          </FormField>
                        </div>
                        <label className="flex items-center gap-2 text-sm">
                          <Switch
                            checked={!!m.advanced.store}
                            onCheckedChange={(v) => patch((d) => void (d.models[mi].advanced.store = v))}
                          />
                          在服务端留存对话记录（store）
                          <span className="text-xs text-muted-foreground">默认关闭，面试数据不留存第三方</span>
                        </label>
                        <FormField
                          label="额外参数（extra_body）"
                          htmlFor={`m-extra-add-${m.id}`}
                          hint='键值以请求体字面 extra_body 字段嵌套下发（不与内置顶层参数合并）；值自动识别布尔与数字，如 enable_thinking = true'
                        >
                          <div className="flex flex-col gap-2">
                            {kvRowsOf(m).map((row, ri) => (
                              <div key={`${m.id}-kv-${ri}`} className="flex items-center gap-2">
                                <Input
                                  value={row.k}
                                  onChange={(e) => {
                                    const rows = [...kvRowsOf(m)]
                                    rows[ri] = { ...rows[ri], k: e.target.value }
                                    setKvRows(mi, m, rows)
                                  }}
                                  placeholder="参数名，如 enable_thinking"
                                  aria-label={`额外参数 ${ri + 1} 名称`}
                                  className="h-8 flex-1 font-mono text-xs"
                                />
                                <Input
                                  value={row.v}
                                  onChange={(e) => {
                                    const rows = [...kvRowsOf(m)]
                                    rows[ri] = { ...rows[ri], v: e.target.value }
                                    setKvRows(mi, m, rows)
                                  }}
                                  placeholder="值，如 true 或 8192"
                                  aria-label={`额外参数 ${ri + 1} 值`}
                                  className="h-8 flex-1 font-mono text-xs"
                                />
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  aria-label={`删除额外参数 ${row.k || ri + 1}`}
                                  onClick={() => setKvRows(mi, m, kvRowsOf(m).filter((_, i) => i !== ri))}
                                >
                                  <Trash className="size-3.5" aria-hidden />
                                </Button>
                              </div>
                            ))}
                            <div>
                              <Button
                                id={`m-extra-add-${m.id}`}
                                variant="outline"
                                size="sm"
                                onClick={() => setKvRows(mi, m, [...kvRowsOf(m), { k: '', v: '' }])}
                              >
                                <Plus className="size-3.5" aria-hidden /> 添加参数
                              </Button>
                            </div>
                          </div>
                        </FormField>
                      </div>
                    )}
                  </div>
                )
              })}
            <div>
              <Button variant="outline" size="sm" onClick={() => addModel(p.id)}>
                <Plus className="size-3.5" aria-hidden /> 添加模型
              </Button>
            </div>
          </div>
        </Section>
      ))}

      <div className="mt-4 flex flex-wrap gap-2">
        <Button variant="outline" onClick={addProvider}>
          <Plus className="size-4" aria-hidden /> 添加 Provider
        </Button>
        <Button onClick={() => save()} disabled={saving}>
          {saving ? '保存中…' : '全部保存'}
        </Button>
      </div>
      {doc.providers.length > 0 && doc.models.length === 0 && (
        <p className="mt-2 text-sm text-muted-foreground">还没有任何模型；在 Provider 卡片内「添加模型」后保存。</p>
      )}

      {/* 全局参数 */}
      <Section
        title="全局参数"
        className="mt-4"
        sub="对所有模型生效"
        action={
          <Button variant="secondary" size="sm" onClick={saveGlobal}>
            保存全局参数
          </Button>
        }
      >
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <FormField label="请求超时（秒）" htmlFor="g-timeout" hint="5–600">
            <Input
              id="g-timeout"
              inputMode="numeric"
              value={doc.global.timeout}
              onChange={(e) => patch((d) => void (d.global.timeout = Number(e.target.value) || 0))}
            />
          </FormField>
          <FormField label="短任务输出上限" htmlFor="g-short" hint="判卷/评分/标签等" >
            <Input
              id="g-short"
              inputMode="numeric"
              value={doc.global.max_output_tokens_short}
              onChange={(e) => patch((d) => void (d.global.max_output_tokens_short = Number(e.target.value) || 0))}
            />
          </FormField>
          <FormField label="长文任务输出上限" htmlFor="g-long" hint="复盘全文/参考答案/面试官备课">
            <Input
              id="g-long"
              inputMode="numeric"
              value={doc.global.max_output_tokens_long}
              onChange={(e) => patch((d) => void (d.global.max_output_tokens_long = Number(e.target.value) || 0))}
            />
          </FormField>
        </div>
      </Section>

      {/* 流式语速与交互控制 */}
      <Section
        title="AI 对话交互与流式语速"
        className="mt-4"
        sub="控制模拟面试中的吐字流畅度与打字机节奏（全局统一生效）"
      >
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <FormField
            label="AI 吐字速度"
            hint={`当前：${speedRate >= 210 ? '∞ 无限制（瞬间输出）' : `${speedRate} 字/秒`}`}
          >
            <div className="flex items-center gap-3 pt-1">
              <input
                type="range"
                min="10"
                max="210"
                step="10"
                value={speedRate}
                onChange={(e) => {
                  const v = Number(e.target.value)
                  setSpeedRate(v)
                  setStoredStreamSpeedRate(v)
                  toast.success('AI 对话语速已更新')
                }}
                className="h-2 flex-1 cursor-pointer accent-primary"
                title="调整 AI 吐字语速（10~200 字/秒，最右端为无限制立即输出）"
              />
              <span className="font-mono text-xs w-20 text-right text-foreground font-semibold">
                {speedRate >= 210 ? '∞ 无限制' : `${speedRate} 字/s`}
              </span>
            </div>
          </FormField>
          <div className="flex flex-col justify-center rounded-lg border border-border/60 bg-muted/20 p-3 text-xs text-muted-foreground">
            <span className="font-semibold text-foreground">💡 自适应平滑打字机说明</span>
            <span className="mt-1">采用高精度毫秒级浮点字符累加器，杜绝网络抖动造成的顿挫；空闲时自动休眠节能，滑至最右端 210 即刻进入瞬间全量渲染模式。</span>
          </div>
        </div>
      </Section>

      <ConfirmDialog
        open={!!delTarget}
        onOpenChange={(o) => !o && setDelTarget(null)}
        title={delTarget?.kind === 'provider' ? `删除 Provider「${delTarget.name}」` : `删除模型「${delTarget?.name}」`}
        description={
          delTarget?.kind === 'provider'
            ? '其下所有模型一并删除。点「保存配置」后生效。'
            : '该模型配置删除后不可恢复。点「保存配置」后生效。'
        }
        confirmLabel="删除"
        destructive
        onConfirm={() => (delTarget?.kind === 'provider' ? removeProvider(delTarget.id) : delTarget && removeModel(delTarget.id))}
      />
    </div>
  )
}
