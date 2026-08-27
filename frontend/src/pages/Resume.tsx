import { useEffect, useState } from 'react'
import {
  Archive,
  ArrowLeft,
  CircleNotch,
  ClockCounterClockwise,
  Copy,
  DownloadSimple,
  Eye,
  FileArrowDown,
  MagicWand,
  PencilSimple,
  Plus,
  Sparkle,
  Trash,
  X,
} from '@phosphor-icons/react'
import { apiDelete, apiGet, apiPost, apiPut } from '../api/client'
import { isRunning, onJobDone, startAiJob, trackRunning, useAiJobs } from '../ai/jobs'
import { PageHeader } from '../components/PageHeader'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { FormField } from '../components/FormField'
import ResumeOptimizeDialog from '../components/ResumeOptimizeDialog'
import { toast } from 'sonner'
import '../resume-paper.css'

export interface ResumeListItem {
  id: number
  name: string
  version_name: string
  is_archived: boolean
  is_active: boolean
  updated_at: string
  raw_text_preview?: string
  parsed_name?: string
  parsed_intent?: string
}

export interface StructuredResume {
  name: string
  summary: string
  gender?: string
  age?: string
  phone?: string
  email?: string
  city?: string
  years?: string
  intent_position?: string
  intent_city?: string
  intent_salary?: string
  skills?: string[]
  experience?: {
    company: string
    title: string
    department?: string
    start_date?: string
    end_date?: string
    period?: string
    responsibilities?: string[]
    achievements?: string[]
    detail?: string
  }[]
  projects?: {
    name: string
    role?: string
    start_date?: string
    end_date?: string
    tech_stack?: string
    detail?: string
    link?: string
  }[]
  education?: {
    school: string
    major?: string
    degree?: string
    start_date?: string
    end_date?: string
    courses?: string[]
  }[]
  certificates?: {
    name: string
    date?: string
  }[]
  self_evaluation?: string
  links?: {
    title: string
    url: string
  }[]
}

function blankStructured(): StructuredResume {
  return {
    name: '',
    summary: '',
    phone: '',
    email: '',
    city: '',
    years: '',
    intent_position: '',
    intent_city: '',
    intent_salary: '',
    skills: [],
    experience: [],
    projects: [],
    education: [],
    certificates: [],
    self_evaluation: '',
    links: [],
  }
}

export default function ResumePage() {
  // 核心状态
  const [workingResume, setWorkingResume] = useState<any>(null)
  const [allResumes, setAllResumes] = useState<ResumeListItem[]>([])
  const [selectedResumeId, setSelectedResumeId] = useState<number | null>(null)
  const [currentViewResume, setCurrentViewResume] = useState<any>(null)

  // 编辑态状态（仅在工作副本下可编辑）
  const [mode, setMode] = useState<'preview' | 'edit'>('preview')
  const [editDraft, setEditDraft] = useState<StructuredResume>(blankStructured())
  const [dirty, setDirty] = useState(false)
  const [saving, setSaving] = useState(false)
  const [loading, setLoading] = useState(true)
  const [nameError, setNameError] = useState('')
  const [newSkillInput, setNewSkillInput] = useState('')
  const [addingSkill, setAddingSkill] = useState(false)

  // 弹窗与抽屉
  const [importOpen, setImportOpen] = useState(false)
  const [rawInput, setRawInput] = useState('')
  const [snapshotOpen, setSnapshotOpen] = useState(false)
  const [snapshotName, setSnapshotName] = useState('')
  const [historyOpen, setHistoryOpen] = useState(false)
  const [exportOpen, setExportOpen] = useState(false)
  const [exportMarkdown, setExportMarkdown] = useState('')
  const [exportLoading, setExportLoading] = useState(false)
  const [delTargetId, setDelTargetId] = useState<number | null>(null)
  const [pendingSwitchId, setPendingSwitchId] = useState<number | null>(null)
  // 票06：AI 优化变更集面板
  const [optimizeOpen, setOptimizeOpen] = useState(false)

  // 全局 AI 任务中心
  const aiJobs = useAiJobs()
  const parsing = isRunning(aiJobs, 'resume_parse')

  // 加载数据
  async function loadInitial() {
    setLoading(true)
    try {
      const [working, list] = await Promise.all([
        apiGet('/api/resume'),
        apiGet('/api/resumes'),
      ])
      setWorkingResume(working)
      setAllResumes(list || [])
      setSelectedResumeId(working.id)
      setCurrentViewResume(working)
      setEditDraft(working.parsed ? { ...blankStructured(), ...working.parsed } : blankStructured())
      setRawInput(working.raw_text || '')
      trackRunning(working.ai_jobs)
      setDirty(false)
    } catch (e: any) {
      toast.error(e.message || '加载简历失败')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadInitial()
  }, [])

  // 监听解析任务完成
  useEffect(() => {
    if (!workingResume?.id) return
    const off = onJobDone('resume_parse', workingResume.id, (ok) => {
      if (!ok) {
        toast.error('AI 解析简历失败，请检查模型配置后重试')
      } else {
        toast.success('简历 AI 解析完成！已更新工作副本并自动留存前序快照')
        setImportOpen(false)
        loadInitial()
      }
    })
    return () => {
      off()
    }
  }, [workingResume?.id])

  async function doSwitchResumeVersion(id: number) {
    setSelectedResumeId(id)
    if (id === workingResume?.id) {
      setCurrentViewResume(workingResume)
      setEditDraft(workingResume.parsed ? { ...blankStructured(), ...workingResume.parsed } : blankStructured())
      setDirty(false)
      setHistoryOpen(false)
      return
    }
    try {
      const snap = await apiGet(`/api/resumes/${id}`)
      setCurrentViewResume(snap)
      setMode('preview') // 历史版本强制为只读预览
      setHistoryOpen(false)
    } catch (e: any) {
      toast.error(e.message || '加载历史快照失败')
    }
  }

  // 切换查看的简历版本
  async function selectResumeVersion(id: number) {
    if (dirty && mode === 'edit') {
      setPendingSwitchId(id)
      return
    }
    await doSwitchResumeVersion(id)
  }

  // 保存工作副本
  async function handleSaveWorkingCopy() {
    if (!workingResume) return
    if (!editDraft.name?.trim()) {
      setNameError('姓名不能为空')
      toast.error('请填写姓名后再保存')
      return
    }
    setNameError('')
    setSaving(true)
    try {
      const payload = {
        name: `${editDraft.name.trim()}的简历`,
        raw_text: rawInput || workingResume.raw_text || '',
        parsed: editDraft,
      }
      const updated = await apiPut('/api/resume', payload)
      setWorkingResume(updated)
      setCurrentViewResume(updated)
      setDirty(false)
      toast.success('工作副本保存成功')
    } catch (e: any) {
      toast.error(e.message || '保存失败')
    } finally {
      setSaving(false)
    }
  }

  // 触发 AI 导入与重新解析
  async function handleTriggerParse() {
    if (!rawInput.trim()) {
      toast.error('请先粘贴或输入简历原文')
      return
    }
    try {
      // 1. 先保存原文（不回传 editDraft，避免空结构体击穿快照守卫）
      await apiPut('/api/resume', {
        raw_text: rawInput,
      })
      // 2. 触发后台任务
      await startAiJob('resume_parse', workingResume.id, '/api/resume/parse')
      setImportOpen(false)
      toast.info('已保存原文并提交解析，可关闭本页等待结果')
    } catch (e: any) {
      toast.error(e.message || '触发解析失败')
    }
  }

  // 创建历史快照
  async function handleCreateSnapshot() {
    if (!snapshotName.trim()) {
      toast.error('请输入快照版本名称')
      return
    }
    try {
      await apiPost('/api/resumes/snapshot', {
        version_name: snapshotName.trim(),
      })
      toast.success(`快照「${snapshotName.trim()}」已成功创建并归档`)
      setSnapshotOpen(false)
      setSnapshotName('')
      const list = await apiGet('/api/resumes')
      setAllResumes(list || [])
    } catch (e: any) {
      toast.error(e.message || '创建快照失败')
    }
  }

  // 删除历史快照
  async function handleDeleteSnapshot() {
    if (!delTargetId) return
    try {
      await apiDelete(`/api/resumes/${delTargetId}`)
      toast.success('历史快照已删除')
      setDelTargetId(null)
      if (selectedResumeId === delTargetId) {
        selectResumeVersion(workingResume.id)
      }
      const list = await apiGet('/api/resumes')
      setAllResumes(list || [])
    } catch (e: any) {
      toast.error(e.message || '删除快照失败')
    }
  }

  // 导出 Markdown
  async function handleOpenExport() {
    setExportLoading(true)
    setExportOpen(true)
    try {
      const url = selectedResumeId && selectedResumeId !== workingResume?.id
        ? `/api/resume/export/markdown?resume_id=${selectedResumeId}`
        : '/api/resume/export/markdown'
      const res = await fetch(url, { credentials: 'same-origin' })
      const text = await res.text()
      setExportMarkdown(text)
    } catch (e: any) {
      toast.error('获取 Markdown 导出内容失败')
    } finally {
      setExportLoading(false)
    }
  }

  function handleCopyMarkdown() {
    navigator.clipboard.writeText(exportMarkdown)
    toast.success('Markdown 已复制到剪贴板')
  }

  function handleDownloadMarkdown() {
    const filename = `${currentViewResume?.parsed?.name || '个人简历'}_${currentViewResume?.version_name || '导出'}.md`
    const blob = new Blob([exportMarkdown], { type: 'text/markdown;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.click()
    URL.revokeObjectURL(url)
    toast.success(`已下载 ${filename}`)
  }

  // Draft Mutators
  function updateDraft<K extends keyof StructuredResume>(key: K, val: StructuredResume[K]) {
    setEditDraft((prev) => ({ ...prev, [key]: val }))
    setDirty(true)
  }

  const isViewingSnapshot = selectedResumeId !== null && selectedResumeId !== workingResume?.id
  const snapshots = allResumes.filter((r) => r.is_archived)
  const displayData: StructuredResume = currentViewResume?.parsed || blankStructured()
  const hasStructuredContent = Boolean(
    displayData.name ||
    displayData.summary ||
    displayData.intent_position ||
    (displayData.skills && displayData.skills.length > 0) ||
    (displayData.experience && displayData.experience.length > 0) ||
    (displayData.projects && displayData.projects.length > 0) ||
    (displayData.education && displayData.education.length > 0)
  )

  if (loading) {
    return (
      <div className="flex h-64 items-center justify-center">
        <CircleNotch className="size-6 animate-spin text-primary" />
      </div>
    )
  }

  return (
    <div className="pb-16">
      {/* 顶部主标题与操作区 */}
      <PageHeader
        title="简历"
        meta={<span>工作副本与快照</span>}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            {isViewingSnapshot ? (
              <>
                <Button variant="outline" size="sm" onClick={() => selectResumeVersion(workingResume.id)}>
                  <ArrowLeft className="size-4" /> 返回工作副本
                </Button>
                <Button variant="outline" size="sm" onClick={handleOpenExport}>
                  <FileArrowDown className="size-4" /> 导出 Markdown
                </Button>
              </>
            ) : (
              <>
                <Button variant="outline" size="sm" onClick={() => setImportOpen(true)} disabled={parsing}>
                  <Sparkle weight="fill" className="size-4 text-primary" />
                  {parsing ? 'AI 解析中…' : '导入原文 / AI解析'}
                </Button>
                {workingResume?.parsed && !isViewingSnapshot && (
                  <Button variant="outline" size="sm" onClick={() => setOptimizeOpen(true)}>
                    <MagicWand weight="bold" className="size-4 text-primary" /> AI 优化
                  </Button>
                )}
                <Button variant="outline" size="sm" onClick={() => setSnapshotOpen(true)}>
                  <Archive className="size-4" /> 存为快照
                </Button>
                <Button variant="outline" size="sm" onClick={handleOpenExport}>
                  <FileArrowDown className="size-4" /> 导出 Markdown
                </Button>
                <Button variant="outline" size="sm" onClick={() => setHistoryOpen(true)}>
                  <ClockCounterClockwise className="size-4" /> 历史留档 ({snapshots.length})
                </Button>

                {mode === 'preview' ? (
                  <Button size="sm" onClick={() => setMode('edit')}>
                    <PencilSimple className="size-4" /> 编辑内容
                  </Button>
                ) : (
                  <>
                    <Button
                      size="sm"
                      onClick={handleSaveWorkingCopy}
                      disabled={!dirty || saving}
                      className={dirty ? 'bg-primary text-primary-foreground font-semibold shadow-sm' : ''}
                    >
                      {saving ? '保存中…' : dirty ? '● 保存修改' : '已保存'}
                    </Button>
                    <Button size="sm" variant="ghost" onClick={() => setMode('preview')}>
                      <Eye className="size-4" /> 完成预览
                    </Button>
                  </>
                )}
              </>
            )}
          </div>
        }
      />

      {/* 历史快照只读提示横幅 */}
      {isViewingSnapshot && (
        <div className="mb-4 flex items-center justify-between rounded-lg border border-warning/30 bg-warning/10 px-4 py-3 text-sm text-warning">
          <div className="flex items-center gap-2">
            <Archive className="size-4 text-warning" />
            <span>
              您正在查看历史只读留档「<b>{currentViewResume?.version_name}</b>」（存档于 {currentViewResume?.updated_at?.slice(0, 10)}）。
              该版本为投递审计快照，不可直接修改。
            </span>
          </div>
          <Button size="sm" variant="outline" onClick={() => selectResumeVersion(workingResume.id)}>
            回到工作副本
          </Button>
        </div>
      )}

      {/* 主工作区 */}
      {mode === 'preview' ? (
        /* 成品展示为主体 (Preview Mode) */
        <div className="mx-auto max-w-4xl">
          {hasStructuredContent ? (
            <div className="resume-paper rounded-xl border border-border bg-card p-6 sm:p-10">
              {/* 头部个人基本信息 */}
              <div className="border-b border-border pb-6 text-center">
                <h1 className="text-2xl font-bold tracking-tight text-foreground sm:text-3xl">
                  {displayData.name || '未填姓名'}
                </h1>
                {displayData.summary && (
                  <p className="mx-auto mt-2 max-w-2xl text-sm text-muted-foreground leading-relaxed">
                    {displayData.summary}
                  </p>
                )}
                <div className="mt-4 flex flex-wrap items-center justify-center gap-x-4 gap-y-1.5 text-xs text-muted-foreground">
                  {displayData.phone && <span>📱 {displayData.phone}</span>}
                  {displayData.email && <span>✉️ {displayData.email}</span>}
                  {displayData.city && <span>📍 {displayData.city}</span>}
                  {displayData.years && <span>💼 {displayData.years} 经验</span>}
                  {displayData.gender && <span>👤 {displayData.gender}</span>}
                  {displayData.age && <span>🎂 {displayData.age}</span>}
                </div>
                {(displayData.intent_position || displayData.intent_city || displayData.intent_salary) && (
                  <div className="mt-3 inline-flex flex-wrap items-center justify-center gap-2 rounded-full bg-muted/60 px-3.5 py-1 text-xs font-medium text-foreground">
                    <span>🎯 求职意向：</span>
                    {displayData.intent_position && <span>{displayData.intent_position}</span>}
                    {displayData.intent_city && <span>· {displayData.intent_city}</span>}
                    {displayData.intent_salary && <span>· {displayData.intent_salary}</span>}
                  </div>
                )}
              </div>

              {/* 技能特长 */}
              {displayData.skills && displayData.skills.length > 0 && (
                <section className="mt-6">
                  <h2 className="border-b border-border pb-1.5 text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    技能特长 / Technical Skills
                  </h2>
                  <div className="mt-3 flex flex-wrap gap-1.5">
                    {displayData.skills.map((s, idx) => (
                      <span
                        key={idx}
                        className="rounded-md border border-border bg-muted/40 px-2.5 py-1 font-mono text-xs text-foreground"
                      >
                        {s}
                      </span>
                    ))}
                  </div>
                </section>
              )}

              {/* 工作经历 */}
              {displayData.experience && displayData.experience.length > 0 && (
                <section className="mt-6">
                  <h2 className="border-b border-border pb-1.5 text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    工作经历 / Experience
                  </h2>
                  <div className="mt-3 space-y-4">
                    {displayData.experience.map((exp, idx) => (
                      <div key={idx} className="space-y-1">
                        <div className="flex flex-wrap items-baseline justify-between gap-2">
                          <span className="font-semibold text-sm text-foreground">
                            {exp.company} {exp.department && `· ${exp.department}`}
                          </span>
                          {(exp.start_date || exp.end_date) && (
                            <span className="font-mono text-xs text-muted-foreground">
                              {exp.start_date} ~ {exp.end_date || '至今'}
                            </span>
                          )}
                        </div>
                        {exp.title && <div className="text-xs font-medium text-primary">{exp.title}</div>}
                        {(exp.responsibilities ?? []).length > 0 && (
                          <ul className="mt-1 list-disc space-y-0.5 pl-4 text-xs leading-relaxed text-foreground">
                            {exp.responsibilities!.map((item, i) => (
                              <li key={i}>{item}</li>
                            ))}
                          </ul>
                        )}
                        {(exp.achievements ?? []).length > 0 && (
                          <ul className="mt-1 list-disc space-y-0.5 pl-4 text-xs leading-relaxed text-foreground">
                            {exp.achievements!.map((item, i) => (
                              <li key={i}>{item}</li>
                            ))}
                          </ul>
                        )}
                      </div>
                    ))}
                  </div>
                </section>
              )}

              {/* 项目经历 */}
              {displayData.projects && displayData.projects.length > 0 && (
                <section className="mt-6">
                  <h2 className="border-b border-border pb-1.5 text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    项目经历 / Projects
                  </h2>
                  <div className="mt-3 space-y-5">
                    {displayData.projects.map((proj, idx) => (
                      <div key={idx} className="space-y-1.5">
                        <div className="flex flex-wrap items-baseline justify-between gap-2">
                          <span className="font-semibold text-sm text-foreground">
                            {proj.name} {proj.role && <span className="font-normal text-xs text-muted-foreground">({proj.role})</span>}
                          </span>
                          {(proj.start_date || proj.end_date) && (
                            <span className="font-mono text-xs text-muted-foreground">
                              {proj.start_date} ~ {proj.end_date || '至今'}
                            </span>
                          )}
                        </div>
                        {proj.tech_stack && (
                          <div className="text-xs text-muted-foreground">
                            <span className="font-semibold text-foreground">技术栈：</span>
                            <span className="font-mono">{proj.tech_stack}</span>
                          </div>
                        )}
                        {proj.detail && (
                          <p className="whitespace-pre-wrap text-xs text-muted-foreground leading-relaxed">
                            {proj.detail}
                          </p>
                        )}
                      </div>
                    ))}
                  </div>
                </section>
              )}

              {/* 教育经历 */}
              {displayData.education && displayData.education.length > 0 && (
                <section className="mt-6">
                  <h2 className="border-b border-border pb-1.5 text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    教育经历 / Education
                  </h2>
                  <div className="mt-3 space-y-3">
                    {displayData.education.map((edu, idx) => (
                      <div key={idx} className="text-xs">
                        <div className="flex flex-wrap items-baseline justify-between gap-2">
                          <span className="font-semibold text-foreground">
                            {edu.school} {edu.major && `· ${edu.major}`} {edu.degree && `(${edu.degree})`}
                          </span>
                          {(edu.start_date || edu.end_date) && (
                            <span className="font-mono text-muted-foreground">
                              {edu.start_date} ~ {edu.end_date || '至今'}
                            </span>
                          )}
                        </div>
                        {edu.courses && edu.courses.length > 0 && (
                          <div className="mt-1.5 flex flex-wrap gap-1">
                            <span className="text-muted-foreground">主修课程：</span>
                            {edu.courses.map((course, cidx) => (
                              <span key={cidx} className="rounded bg-muted/40 px-1.5 py-0.5 text-foreground/80">
                                {course}
                              </span>
                            ))}
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                </section>
              )}

              {/* 证书荣誉 */}
              {displayData.certificates && displayData.certificates.length > 0 && (
                <section className="mt-6">
                  <h2 className="border-b border-border pb-1.5 text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    证书与荣誉 / Certificates & Honors
                  </h2>
                  <div className="mt-3 flex flex-wrap gap-2 text-xs">
                    {displayData.certificates.map((cert, idx) => (
                      <span key={idx} className="rounded border border-border bg-muted/30 px-2.5 py-1 text-foreground">
                        🏆 {cert.name} {cert.date && `(${cert.date})`}
                      </span>
                    ))}
                  </div>
                </section>
              )}

              {/* 自我评价 */}
              {displayData.self_evaluation && (
                <section className="mt-6">
                  <h2 className="border-b border-border pb-1.5 text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    自我评价 / Self Evaluation
                  </h2>
                  <p className="mt-2 whitespace-pre-wrap text-xs text-muted-foreground leading-relaxed">
                    {displayData.self_evaluation}
                  </p>
                </section>
              )}

              {/* 链接作品集 */}
              {displayData.links && displayData.links.length > 0 && (
                <section className="mt-6">
                  <h2 className="border-b border-border pb-1.5 text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    相关链接 / Links
                  </h2>
                  <div className="mt-2 space-y-1 text-xs">
                    {displayData.links.map((link, idx) => (
                      <div key={idx} className="flex items-center gap-2">
                        <span className="font-medium text-foreground">{link.title}:</span>
                        <a
                          href={link.url}
                          target="_blank"
                          rel="noreferrer"
                          className="font-mono text-primary hover:underline"
                        >
                          {link.url}
                        </a>
                      </div>
                    ))}
                  </div>
                </section>
              )}
            </div>
          ) : (
            <div className="rounded-lg border border-dashed border-border bg-card p-12 text-center">
              <div className="mx-auto flex size-12 items-center justify-center rounded-full bg-accent/15 text-accent dark:bg-accent/25">
                <Sparkle weight="fill" className="size-6" />
              </div>
              <h3 className="mt-4 font-semibold text-foreground text-lg">还没有结构化简历</h3>
              <p className="mx-auto mt-2 max-w-md text-sm text-foreground">
                用「导入原文 / AI解析」粘贴文本，或「编辑内容」手工录入。
              </p>
              <div className="mt-6 flex justify-center gap-3">
                <Button onClick={() => setImportOpen(true)}>
                  <Sparkle weight="fill" className="size-4" /> 导入原文 / AI解析
                </Button>
                <Button variant="outline" onClick={() => setMode('edit')}>
                  <PencilSimple className="size-4" /> 手动录入编辑
                </Button>
              </div>
            </div>
          )}
        </div>
      ) : (
        /* 编辑态 (Edit Mode，仅工作副本) */
        <div className="mx-auto max-w-4xl space-y-4">
          {/* 基本信息 & 求职意向 */}
          <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
            <h3 className="font-semibold text-foreground text-sm border-b border-border pb-2">基本信息 & 求职意向</h3>
            <div className="mt-4 grid grid-cols-1 gap-3 sm:grid-cols-3">
              <FormField label="姓名" required htmlFor="res-name" error={nameError}>
                <Input
                  id="res-name"
                  value={editDraft.name}
                  onChange={(e) => {
                    setNameError('')
                    updateDraft('name', e.target.value)
                  }}
                  placeholder="如：张三"
                />
              </FormField>
              <FormField label="电话" htmlFor="res-phone">
                <Input id="res-phone" value={editDraft.phone || ''} onChange={(e) => updateDraft('phone', e.target.value)} placeholder="如：13800000000" />
              </FormField>
              <FormField label="邮箱" htmlFor="res-email">
                <Input id="res-email" value={editDraft.email || ''} onChange={(e) => updateDraft('email', e.target.value)} placeholder="如：zhangsan@example.com" />
              </FormField>
              <FormField label="所在城市" htmlFor="res-city">
                <Input id="res-city" value={editDraft.city || ''} onChange={(e) => updateDraft('city', e.target.value)} placeholder="如：杭州" />
              </FormField>
              <FormField label="工作年限" htmlFor="res-years">
                <Input id="res-years" value={editDraft.years || ''} onChange={(e) => updateDraft('years', e.target.value)} placeholder="如：5年" />
              </FormField>
              <FormField label="期望岗位" htmlFor="res-intent-pos">
                <Input id="res-intent-pos" value={editDraft.intent_position || ''} onChange={(e) => updateDraft('intent_position', e.target.value)} placeholder="如：Rust 后端专家" />
              </FormField>
              <FormField label="期望城市" htmlFor="res-intent-city">
                <Input id="res-intent-city" value={editDraft.intent_city || ''} onChange={(e) => updateDraft('intent_city', e.target.value)} placeholder="如：上海 / 杭州" />
              </FormField>
              <FormField label="期望薪资" htmlFor="res-intent-sal">
                <Input id="res-intent-sal" value={editDraft.intent_salary || ''} onChange={(e) => updateDraft('intent_salary', e.target.value)} placeholder="如：35k-45k" />
              </FormField>
            </div>
            <div className="mt-3">
              <FormField label="一句话核心概述 (Summary)" htmlFor="res-summary">
                <Textarea
                  id="res-summary"
                  rows={2}
                  value={editDraft.summary || ''}
                  onChange={(e) => updateDraft('summary', e.target.value)}
                  placeholder="简述核心竞争力和擅长领域…"
                />
              </FormField>
            </div>
          </div>

          {/* 技能特长 */}
          <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
            <div className="flex items-center justify-between border-b border-border pb-2">
              <h3 className="font-semibold text-foreground text-sm">技能特长 (标签列表)</h3>
              {!addingSkill ? (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    setAddingSkill(true)
                    setNewSkillInput('')
                  }}
                >
                  <Plus className="size-3.5" /> 添加技能
                </Button>
              ) : null}
            </div>

            {addingSkill && (
              <div className="mt-3 flex items-center gap-2 rounded-md border border-border bg-muted/20 p-2.5">
                <Input
                  size={1}
                  className="h-8 text-xs"
                  placeholder="输入技能名称（如 Tokio、PostgreSQL）并按回车…"
                  value={newSkillInput}
                  autoFocus
                  onChange={(e) => setNewSkillInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault()
                      if (newSkillInput.trim()) {
                        updateDraft('skills', [...(editDraft.skills || []), newSkillInput.trim()])
                        setNewSkillInput('')
                        setAddingSkill(false)
                      }
                    } else if (e.key === 'Escape') {
                      setAddingSkill(false)
                    }
                  }}
                />
                <Button
                  size="sm"
                  className="h-8 text-xs"
                  onClick={() => {
                    if (newSkillInput.trim()) {
                      updateDraft('skills', [...(editDraft.skills || []), newSkillInput.trim()])
                      setNewSkillInput('')
                    }
                    setAddingSkill(false)
                  }}
                >
                  添加
                </Button>
                <Button size="sm" variant="ghost" className="h-8 text-xs" onClick={() => setAddingSkill(false)}>
                  取消
                </Button>
              </div>
            )}

            <div className="mt-3 flex flex-wrap gap-2">
              {(editDraft.skills || []).map((skill, idx) => (
                <span key={idx} className="flex items-center gap-1.5 rounded-md border border-border bg-muted/40 px-2.5 py-1 text-xs">
                  <span>{skill}</span>
                  <button
                    onClick={() => {
                      updateDraft('skills', (editDraft.skills || []).filter((_, i) => i !== idx))
                    }}
                    className="text-muted-foreground hover:text-destructive"
                  >
                    <X className="size-3" />
                  </button>
                </span>
              ))}
              {(!editDraft.skills || editDraft.skills.length === 0) && !addingSkill && (
                <span className="text-xs text-muted-foreground">暂无技能特长，请点击上方添加</span>
              )}
            </div>
          </div>

          {/* 工作经历 */}
          <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
            <div className="flex items-center justify-between border-b border-border pb-2">
              <h3 className="font-semibold text-foreground text-sm">工作经历</h3>
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  updateDraft('experience', [
                    ...(editDraft.experience || []),
                    { company: '新公司', title: '职位', start_date: '2023-01', end_date: '至今' },
                  ])
                }}
              >
                <Plus className="size-3.5" /> 添加工作经历
              </Button>
            </div>
            <div className="mt-3 space-y-4">
              {(editDraft.experience || []).map((exp, idx) => (
                <div key={idx} className="rounded-md border border-border/80 p-3.5 space-y-3 bg-muted/10">
                  <div className="flex items-center justify-between">
                    <span className="font-semibold text-xs text-primary">工作经历 #{idx + 1}</span>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-7 text-xs text-destructive hover:bg-destructive/10"
                      onClick={() => {
                        updateDraft('experience', (editDraft.experience || []).filter((_, i) => i !== idx))
                      }}
                    >
                      <Trash className="size-3.5" /> 删除
                    </Button>
                  </div>
                  <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-4">
                    <div>
                      <label className="text-xs text-muted-foreground">公司名称</label>
                      <Input
                        value={exp.company}
                        onChange={(e) => {
                          const arr = [...(editDraft.experience || [])]
                          arr[idx] = { ...arr[idx], company: e.target.value }
                          updateDraft('experience', arr)
                        }}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-muted-foreground">职位/角色</label>
                      <Input
                        value={exp.title}
                        onChange={(e) => {
                          const arr = [...(editDraft.experience || [])]
                          arr[idx] = { ...arr[idx], title: e.target.value }
                          updateDraft('experience', arr)
                        }}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-muted-foreground">起始日期</label>
                      <Input
                        value={exp.start_date || ''}
                        onChange={(e) => {
                          const arr = [...(editDraft.experience || [])]
                          arr[idx] = { ...arr[idx], start_date: e.target.value }
                          updateDraft('experience', arr)
                        }}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-muted-foreground">结束日期</label>
                      <Input
                        value={exp.end_date || ''}
                        onChange={(e) => {
                          const arr = [...(editDraft.experience || [])]
                          arr[idx] = { ...arr[idx], end_date: e.target.value }
                          updateDraft('experience', arr)
                        }}
                      />
                    </div>
                  </div>
                  <div>
                    <label className="text-xs text-muted-foreground">工作职责（一行一条）</label>
                    <Textarea
                      rows={3}
                      value={(exp.responsibilities || []).join('\n')}
                      onChange={(e) => {
                        const arr = [...(editDraft.experience || [])]
                        arr[idx] = { ...arr[idx], responsibilities: e.target.value.split('\n').map((s) => s.trim()).filter(Boolean) }
                        updateDraft('experience', arr)
                      }}
                    />
                  </div>
                  <div>
                    <label className="text-xs text-muted-foreground">主要业绩 / 亮点（一行一条）</label>
                    <Textarea
                      rows={2}
                      value={(exp.achievements || []).join('\n')}
                      onChange={(e) => {
                        const arr = [...(editDraft.experience || [])]
                        arr[idx] = { ...arr[idx], achievements: e.target.value.split('\n').map((s) => s.trim()).filter(Boolean) }
                        updateDraft('experience', arr)
                      }}
                    />
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* 项目经历（非研发岗可折叠） */}
          <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
            <div className="flex items-center justify-between border-b border-border pb-2">
              <h3 className="font-semibold text-foreground text-sm">项目经历（可选）</h3>
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  updateDraft('projects', [
                    ...(editDraft.projects || []),
                    { name: '新项目名称', role: '核心研发', tech_stack: 'Rust / Tokio', detail: '' },
                  ])
                }}
              >
                <Plus className="size-3.5" /> 添加项目
              </Button>
            </div>
            <div className="mt-3 space-y-4">
              {(editDraft.projects || []).map((proj, idx) => (
                <div key={idx} className="rounded-md border border-border/80 p-3.5 space-y-3 bg-muted/10">
                  <div className="flex items-center justify-between">
                    <span className="font-semibold text-xs text-primary">项目 #{idx + 1}</span>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-7 text-xs text-destructive hover:bg-destructive/10"
                      onClick={() => {
                        updateDraft('projects', (editDraft.projects || []).filter((_, i) => i !== idx))
                      }}
                    >
                      <Trash className="size-3.5" /> 删除
                    </Button>
                  </div>
                  <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-3">
                    <div>
                      <label className="text-xs text-muted-foreground">项目名称</label>
                      <Input
                        value={proj.name}
                        onChange={(e) => {
                          const arr = [...(editDraft.projects || [])]
                          arr[idx] = { ...arr[idx], name: e.target.value }
                          updateDraft('projects', arr)
                        }}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-muted-foreground">角色</label>
                      <Input
                        value={proj.role || ''}
                        onChange={(e) => {
                          const arr = [...(editDraft.projects || [])]
                          arr[idx] = { ...arr[idx], role: e.target.value }
                          updateDraft('projects', arr)
                        }}
                      />
                    </div>
                    <div>
                      <label className="text-xs text-muted-foreground">技术栈</label>
                      <Input
                        value={proj.tech_stack || ''}
                        onChange={(e) => {
                          const arr = [...(editDraft.projects || [])]
                          arr[idx] = { ...arr[idx], tech_stack: e.target.value }
                          updateDraft('projects', arr)
                        }}
                      />
                    </div>
                  </div>
                  <div>
                    <label className="text-xs text-muted-foreground">项目描述与成果</label>
                    <Textarea
                      rows={3}
                      value={proj.detail || ''}
                      onChange={(e) => {
                        const arr = [...(editDraft.projects || [])]
                        arr[idx] = { ...arr[idx], detail: e.target.value }
                        updateDraft('projects', arr)
                      }}
                    />
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* 教育经历 */}
          <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
            <div className="flex items-center justify-between border-b border-border pb-2">
              <h3 className="font-semibold text-foreground text-sm">教育经历</h3>
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  updateDraft('education', [
                    ...(editDraft.education || []),
                    { school: '院校名称', major: '专业', degree: '本科', start_date: '2016-09', end_date: '2020-06' },
                  ])
                }}
              >
                <Plus className="size-3.5" /> 添加教育经历
              </Button>
            </div>
            <div className="mt-3 space-y-3">
              {(editDraft.education || []).map((edu, idx) => (
                <div key={idx} className="flex flex-wrap items-center gap-2 rounded-md border border-border/80 p-2.5 bg-muted/10">
                  <Input
                    className="flex-1 min-w-[140px]"
                    placeholder="院校"
                    value={edu.school}
                    onChange={(e) => {
                      const arr = [...(editDraft.education || [])]
                      arr[idx] = { ...arr[idx], school: e.target.value }
                      updateDraft('education', arr)
                    }}
                  />
                  <Input
                    className="w-28"
                    placeholder="专业"
                    value={edu.major || ''}
                    onChange={(e) => {
                      const arr = [...(editDraft.education || [])]
                      arr[idx] = { ...arr[idx], major: e.target.value }
                      updateDraft('education', arr)
                    }}
                  />
                  <Input
                    className="w-24"
                    placeholder="学历"
                    value={edu.degree || ''}
                    onChange={(e) => {
                      const arr = [...(editDraft.education || [])]
                      arr[idx] = { ...arr[idx], degree: e.target.value }
                      updateDraft('education', arr)
                    }}
                  />
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-8 text-destructive"
                    onClick={() => {
                      updateDraft('education', (editDraft.education || []).filter((_, i) => i !== idx))
                    }}
                  >
                    <Trash className="size-3.5" />
                  </Button>
                </div>
              ))}
            </div>
          </div>

          {/* 自我评价 */}
          <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
            <h3 className="font-semibold text-foreground text-sm border-b border-border pb-2">自我评价</h3>
            <div className="mt-3">
              <Textarea
                rows={3}
                value={editDraft.self_evaluation || ''}
                onChange={(e) => updateDraft('self_evaluation', e.target.value)}
                placeholder="自我评价与综合特质…"
              />
            </div>
          </div>
        </div>
      )}

      {/* 票06：AI 优化变更集 */}
      {optimizeOpen && (
        <ResumeOptimizeDialog
          onClose={() => setOptimizeOpen(false)}
          onApplied={loadInitial}
        />
      )}

      {/* 弹窗：导入原文 / AI 解析 */}
      {importOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-2xl max-h-[90vh] flex flex-col rounded-lg border border-border bg-card p-6 shadow-xl">
            <div className="flex shrink-0 items-center justify-between border-b border-border pb-3">
              <div className="flex items-center gap-2">
                <Sparkle weight="fill" className="size-5 text-primary" />
                <h3 className="font-bold text-foreground text-base">导入简历原文与 AI 智能解析</h3>
              </div>
              <button onClick={() => setImportOpen(false)} className="text-muted-foreground hover:text-foreground">
                <X className="size-5" />
              </button>
            </div>
            <div className="mt-4 space-y-3 overflow-y-auto pr-1 flex-1 min-h-0">
              <p className="text-xs text-muted-foreground">
                在此粘贴您的中文/英文 Markdown 或纯文本简历。点击解析后，AI 将自动抽取基本信息、工作经历、项目亮点与技能特长。
              </p>
              <Textarea
                rows={10}
                className="font-mono text-xs max-h-80 resize-y"
                placeholder="粘贴简历文本（支持 Markdown 或自由格式）…"
                value={rawInput}
                onChange={(e) => setRawInput(e.target.value)}
              />
              <div className="rounded bg-muted/40 p-2.5 text-xs text-muted-foreground">
                💡 <b>安全提示</b>：重新解析前，系统会自动将您现存的简历内容备份为一份只读快照（留档），确保历史记录绝不丢失。
              </div>
            </div>
            <div className="mt-6 flex shrink-0 justify-end gap-2.5">
              <Button variant="ghost" size="sm" onClick={() => setImportOpen(false)}>
                取消
              </Button>
              <Button size="sm" onClick={handleTriggerParse} disabled={parsing || !rawInput.trim()}>
                <Sparkle weight="fill" className="size-4" />
                {parsing ? 'AI 正在解析中…' : '保存原文并触发 AI 深度解析'}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 弹窗：创建历史快照 */}
      {snapshotOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-md rounded-lg border border-border bg-card p-6 shadow-xl">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="flex items-center gap-2">
                <Archive className="size-5 text-primary" />
                <h3 className="font-bold text-foreground text-base">新建简历留档快照</h3>
              </div>
              <button onClick={() => setSnapshotOpen(false)} className="text-muted-foreground hover:text-foreground">
                <X className="size-5" />
              </button>
            </div>
            <div className="mt-4 space-y-3">
              <p className="text-xs text-muted-foreground">
                快照将锁定当前工作副本的全部结构化字段与原文作为只读审计版本。投递记录可软引用该版本。
              </p>
              <div>
                <label className="text-xs font-medium text-foreground">版本名称</label>
                <Input
                  className="mt-1"
                  placeholder="如：2026架构投递版、字节跳动专版"
                  value={snapshotName}
                  onChange={(e) => setSnapshotName(e.target.value)}
                />
              </div>
            </div>
            <div className="mt-6 flex justify-end gap-2.5">
              <Button variant="ghost" size="sm" onClick={() => setSnapshotOpen(false)}>
                取消
              </Button>
              <Button size="sm" onClick={handleCreateSnapshot} disabled={!snapshotName.trim()}>
                确认创建快照
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 抽屉/弹窗：历史留档列表 */}
      {historyOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-2xl rounded-lg border border-border bg-card p-6 shadow-xl">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="flex items-center gap-2">
                <ClockCounterClockwise className="size-5 text-primary" />
                <h3 className="font-bold text-foreground text-base">历史留档快照清单 ({snapshots.length})</h3>
              </div>
              <button onClick={() => setHistoryOpen(false)} className="text-muted-foreground hover:text-foreground">
                <X className="size-5" />
              </button>
            </div>
            <div className="mt-4 max-h-96 space-y-3 overflow-y-auto pr-1">
              {/* 当前活跃的工作副本入口 */}
              <div className="flex items-center justify-between rounded-lg border border-primary/40 bg-primary/5 p-3.5">
                <div>
                  <div className="flex items-center gap-2">
                    <span className="font-bold text-sm text-foreground">{workingResume?.name || '工作副本'}</span>
                    <span className="rounded bg-primary/20 px-2 py-0.5 font-semibold text-[10px] text-primary">
                      当前工作副本 (可编辑)
                    </span>
                  </div>
                  <p className="mt-1 text-xs text-muted-foreground">
                    更新时间：{workingResume?.updated_at?.slice(0, 19).replace('T', ' ')}
                  </p>
                </div>
                <Button
                  size="sm"
                  variant={selectedResumeId === workingResume?.id ? 'secondary' : 'default'}
                  onClick={() => selectResumeVersion(workingResume.id)}
                >
                  {selectedResumeId === workingResume?.id ? '正在查看' : '切换至此'}
                </Button>
              </div>

              {/* 历史快照列表 */}
              {snapshots.map((snap) => (
                <div
                  key={snap.id}
                  className="flex items-center justify-between rounded-lg border border-border bg-muted/20 p-3.5 transition-colors hover:bg-muted/40"
                >
                  <div>
                    <div className="flex items-center gap-2">
                      <span className="font-bold text-sm text-foreground">{snap.version_name}</span>
                      <span className="rounded border border-border bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                        只读留档
                      </span>
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      存档时间：{snap.updated_at?.slice(0, 19).replace('T', ' ')}
                    </p>
                  </div>
                  <div className="flex items-center gap-2">
                    <Button
                      size="sm"
                      variant={selectedResumeId === snap.id ? 'secondary' : 'outline'}
                      onClick={() => selectResumeVersion(snap.id)}
                    >
                      {selectedResumeId === snap.id ? '正在查看' : '查看预览'}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-8 w-8 p-0 text-destructive hover:bg-destructive/10"
                      onClick={() => setDelTargetId(snap.id)}
                      title="删除该历史快照"
                    >
                      <Trash className="size-4" />
                    </Button>
                  </div>
                </div>
              ))}

              {snapshots.length === 0 && (
                <div className="py-8 text-center text-xs text-muted-foreground">
                  暂无历史留档快照。点击「存为快照」可固化版本留档。
                </div>
              )}
            </div>
          </div>
        </div>
      )}

      {/* 弹窗：导出 Markdown */}
      {exportOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4">
          <div className="w-full max-w-3xl rounded-lg border border-border bg-card p-6 shadow-xl">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="flex items-center gap-2">
                <FileArrowDown className="size-5 text-primary" />
                <h3 className="font-bold text-foreground text-base">
                  Markdown 导出 ({currentViewResume?.version_name || '工作副本'})
                </h3>
              </div>
              <button onClick={() => setExportOpen(false)} className="text-muted-foreground hover:text-foreground">
                <X className="size-5" />
              </button>
            </div>
            <div className="mt-4 space-y-3">
              {exportLoading ? (
                <div className="flex h-48 items-center justify-center">
                  <CircleNotch className="size-6 animate-spin text-primary" />
                </div>
              ) : (
                <Textarea
                  rows={14}
                  readOnly
                  className="font-mono text-xs bg-muted/20"
                  value={exportMarkdown}
                />
              )}
            </div>
            <div className="mt-6 flex justify-between items-center">
              <span className="text-xs text-muted-foreground">
                支持直接复制排版或下载 .md 文件用于外部 PDF 生成与投递。
              </span>
              <div className="flex gap-2">
                <Button variant="outline" size="sm" onClick={handleCopyMarkdown} disabled={exportLoading}>
                  <Copy className="size-4" /> 复制到剪贴板
                </Button>
                <Button size="sm" onClick={handleDownloadMarkdown} disabled={exportLoading}>
                  <DownloadSimple className="size-4" /> 下载 .md 文件
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* 删除快照确认对话框 */}
      <ConfirmDialog
        open={delTargetId !== null}
        onOpenChange={(open) => !open && setDelTargetId(null)}
        destructive
        title="确认删除该历史快照？"
        description="删除后不可恢复。已绑定该快照的投递记录将自动解绑。"
        confirmLabel="确认删除"
        onConfirm={handleDeleteSnapshot}
      />

      {/* 放弃未保存修改并切换版本确认 */}
      <ConfirmDialog
        open={pendingSwitchId !== null}
        onOpenChange={(open) => !open && setPendingSwitchId(null)}
        destructive
        title="放弃未保存的修改？"
        description="您有未保存的编辑内容，切换版本将丢失当前修改。"
        confirmLabel="放弃并切换"
        onConfirm={async () => {
          if (pendingSwitchId !== null) {
            const targetId = pendingSwitchId
            setPendingSwitchId(null)
            await doSwitchResumeVersion(targetId)
          }
        }}
      />
    </div>
  )
}
