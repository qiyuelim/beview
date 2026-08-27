import { useCallback, useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import {
  CaretDown,
  CaretLeft,
  CaretRight,
  DotsThree,
  FolderSimplePlus,
  PencilSimple,
  Trash,
  TreeStructure,
  WarningCircle,
  ArrowsClockwise,
  CheckCircle,
  Lightning,
  GitMerge,
} from '@phosphor-icons/react'
import { apiDelete, apiGet, apiPatch, apiPost } from '../api/client'
import type { SkillGraphData, SkillMatrixData, SkillTreeNode } from '../api/types'
import { SkillRadarPanel } from '../components/SkillRadarPanel'
import { PageHeader } from '../components/PageHeader'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { Button } from '@/components/ui/button'
import { toast } from 'sonner'
import { cn } from '@/lib/utils'

import { ASSESSMENT_DIMENSION_LABELS, ASSESSMENT_DIMENSION_STAGE, type AssessmentDimension } from '../api/types'

export default function Skills() {
  const [data, setData] = useState<SkillGraphData | null>(null)
  const [matrix, setMatrix] = useState<SkillMatrixData | null>(null)
  const [loading, setLoading] = useState(true)
  const [collapsed, setCollapsed] = useState<Record<number, boolean>>({})
  const [matrixDomain, setMatrixDomain] = useState<string | null>(null)
  const [treePath, setTreePath] = useState<SkillTreeNode[]>([])
  const [treeMoreId, setTreeMoreId] = useState<number | null>(null)

  // 弹窗状态
  const [createModal, setCreateModal] = useState<{ open: boolean; parentId: number | null; parentName: string }>({
    open: false,
    parentId: null,
    parentName: '',
  })
  const [editModal, setEditModal] = useState<{ open: boolean; id: number; name: string }>({
    open: false,
    id: 0,
    name: '',
  })
  const [mergeModal, setMergeModal] = useState<{ open: boolean; sourceId: number; sourceName: string }>({
    open: false,
    sourceId: 0,
    sourceName: '',
  })
  const [mergeTargetId, setMergeTargetId] = useState<number | ''>('')
  const [merging, setMerging] = useState(false)
  const [skillNameInput, setSkillNameInput] = useState('')
  const [seedConfirmOpen, setSeedConfirmOpen] = useState(false)
  const [delTarget, setDelTarget] = useState<{ id: number; name: string } | null>(null)
  const [busy, setBusy] = useState(false)

  const loadData = useCallback(async () => {
    try {
      setLoading(true)
      const [resTree, resMatrix] = await Promise.all([
        apiGet('/api/skills/tree'),
        apiGet('/api/skills/matrix').catch(() => null),
      ])
      setData(resTree)
      if (resMatrix) setMatrix(resMatrix)
    } catch (e: any) {
      toast.error(e.message || '加载技能图谱失败')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    loadData()
  }, [loadData])

  const toggleCollapse = (id: number) => {
    setCollapsed((prev) => ({ ...prev, [id]: !prev[id] }))
  }

  const handleSeed = () => {
    setSeedConfirmOpen(true)
  }

  const doSeed = async () => {
    setBusy(true)
    try {
      const res = await apiPost('/api/skills/seed')
      setData(res)
      toast.success('已初始化默认技能树')
      loadData()
    } catch (e: any) {
      toast.error(e.message || '初始化技能树失败')
    } finally {
      setBusy(false)
    }
  }

  const handleCreate = async () => {
    if (!skillNameInput.trim()) {
      toast.error('请输入技能名称')
      return
    }
    if (!createModal.parentId) {
      toast.error('请选择所属顶级领域或父级技能')
      return
    }
    try {
      await apiPost('/api/skills', {
        name: skillNameInput.trim(),
        parent_id: createModal.parentId,
      })
      toast.success('技能节点已创建')
      setCreateModal({ open: false, parentId: null, parentName: '' })
      setSkillNameInput('')
      loadData()
    } catch (e: any) {
      toast.error(e.message || '创建失败')
    }
  }

  const handleUpdate = async () => {
    if (!skillNameInput.trim()) {
      toast.error('技能名称不能为空')
      return
    }
    try {
      await apiPatch(`/api/skills/${editModal.id}`, { name: skillNameInput.trim() })
      toast.success('技能已更新')
      setEditModal({ open: false, id: 0, name: '' })
      setSkillNameInput('')
      loadData()
    } catch (e: any) {
      toast.error(e.message || '更新失败')
    }
  }

  const handleDelete = (id: number, name: string) => {
    setDelTarget({ id, name })
  }

  const doDelete = async () => {
    if (!delTarget) return
    setBusy(true)
    try {
      await apiDelete(`/api/skills/${delTarget.id}`)
      toast.success('技能已删除')
      loadData()
    } catch (e: any) {
      toast.error(e.message || '删除失败')
    } finally {
      setBusy(false)
      setDelTarget(null)
    }
  }

  const handleMerge = async () => {
    if (!mergeTargetId) {
      toast.error('请选择要合并到的目标节点')
      return
    }
    setMerging(true)
    try {
      const res = await apiPost(`/api/skills/${mergeModal.sourceId}/merge`, {
        target_id: mergeTargetId,
      })
      toast.success(`节点已合并！迁移题目 ${res.remapped_questions} 道，子技能 ${res.remapped_children} 个`)
      setMergeModal({ open: false, sourceId: 0, sourceName: '' })
      setMergeTargetId('')
      loadData()
    } catch (e: any) {
      toast.error(e.message || '合并节点失败')
    } finally {
      setMerging(false)
    }
  }

  // 递归提取所有薄弱节点（只取有题且掌握度 < 60 的真实薄弱点）
  const extractWeakNodes = (nodes: SkillTreeNode[]): SkillTreeNode[] => {
    let list: SkillTreeNode[] = []
    for (const n of nodes) {
      if (n.question_count > 0 && n.proficiency < 60) {
        list.push(n)
      }
      if (n.children.length > 0) {
        list = list.concat(extractWeakNodes(n.children))
      }
    }
    return list
  }

  // 递归扁平化候选合并目标节点（排除源节点自身）
  const getAllNodesExcept = (nodes: SkillTreeNode[], excludeId: number): { id: number; name: string; path: string }[] => {
    let list: { id: number; name: string; path: string }[] = []
    for (const n of nodes) {
      if (n.id !== excludeId) {
        list.push({ id: n.id, name: n.name, path: n.path })
        if (n.children && n.children.length > 0) {
          list = list.concat(getAllNodesExcept(n.children, excludeId))
        }
      }
    }
    return list
  }

  const weakNodes = data ? extractWeakNodes(data.tree).sort((a, b) => a.proficiency - b.proficiency).slice(0, 5) : []
  const mergeCandidates = data && mergeModal.sourceId ? getAllNodesExcept(data.tree, mergeModal.sourceId) : []

  return (
    <div className="space-y-6">
      <PageHeader
        title="技能图谱"
        meta={<span>知识树 · 七类考察维度</span>}
        actions={
          <div className="flex items-center gap-2">
            <button
              onClick={handleSeed}
              className="inline-flex h-9 items-center gap-1.5 rounded-md border border-border bg-card px-3 text-xs font-medium text-foreground transition-colors hover:bg-muted"
              title="重新加载通用技术架构默认技能树"
            >
              <ArrowsClockwise className="size-3.5" />
              <span>重置预置技能树</span>
            </button>
          </div>
        }
      />

      {/* 顶部指标卡片 */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <div className="rounded-lg border border-border bg-card p-4">
          <div className="text-xs text-muted-foreground">总技能节点</div>
          <div className="mt-1 font-mono text-2xl font-bold">{data?.total_skills ?? 0}</div>
        </div>
        <div className="rounded-lg border border-border bg-card p-4">
          <div className="text-xs text-muted-foreground">已挂靠题目</div>
          <div className="mt-1 font-mono text-2xl font-bold">{data?.total_tagged_questions ?? 0}</div>
        </div>
        <div className="rounded-lg border border-border bg-card p-4">
          <div className="text-xs text-muted-foreground">全景综合掌握度</div>
          <div className="mt-1 flex items-baseline gap-1 font-mono text-2xl font-bold text-primary">
            {data?.overall_proficiency ?? 0}
            <span className="text-xs font-normal text-muted-foreground">/ 100</span>
          </div>
        </div>
        <div className="rounded-lg border border-border bg-card p-4">
          <div className="text-xs text-muted-foreground">薄弱预警知识点</div>
          <div className="mt-1 flex items-baseline gap-1 font-mono text-2xl font-bold text-destructive">
            {weakNodes.length}
            <span className="text-xs font-normal text-muted-foreground">项待强化</span>
          </div>
        </div>
      </div>

      {matrix && matrix.domains.length > 0 && (
        <section className="rounded-xl border border-border bg-card p-4" aria-label="二维能力诊断矩阵">
          <div className="flex items-center justify-between border-b border-border pb-3">
            <div>
              <h2 className="text-[13px] font-semibold tracking-wide text-heading">二维能力矩阵（技术大纲 × 考察维度）</h2>
              <p className="text-xs text-muted-foreground mt-0.5">
                结合题目难度加权计算能力指数（ADR-0022），色块代表掌握熟练度（绿：熟练 / 黄：巩固 / 红：薄弱）
              </p>
            </div>
            {matrix.weakest_cell && matrix.weakest_cell.count > 0 && (() => {
              const targetStage =
                ASSESSMENT_DIMENSION_STAGE[matrix.weakest_cell.question_type as AssessmentDimension] || 'basics'
              const targetDossier = `${matrix.weakest_cell.domain} (${ASSESSMENT_DIMENSION_LABELS[matrix.weakest_cell.question_type as AssessmentDimension] ?? matrix.weakest_cell.question_type})`

              // 查找匹配的技能节点并收集整棵子树全部 skill_id（N1 修复：使叶子挂靠题能被正确圈出）
              let rootSkillId: number | undefined
              const matchedSkillIds: number[] = []
              if (data?.tree) {
                const findNode = (nodes: SkillTreeNode[]): SkillTreeNode | undefined => {
                  for (const n of nodes) {
                    if (n.name === matrix.weakest_cell?.domain || n.path.includes(matrix.weakest_cell?.domain || '')) return n
                    if (n.children && n.children.length > 0) {
                      const found = findNode(n.children)
                      if (found) return found
                    }
                  }
                  return undefined
                }
                const found = findNode(data.tree)
                if (found) {
                  rootSkillId = found.id
                  const collect = (node: SkillTreeNode) => {
                    matchedSkillIds.push(node.id)
                    for (const c of node.children || []) {
                      collect(c)
                    }
                  }
                  collect(found)
                }
              }

              const queryObj: Record<string, string> = {
                kind: 'interview',
                direction: matrix.weakest_cell.domain,
                stage: targetStage,
                dossier: targetDossier,
                title: `靶向攻坚 · ${matrix.weakest_cell.domain}`,
                tag: matrix.weakest_cell.domain,
                skill_name: matrix.weakest_cell.domain,
              }
              if (rootSkillId) {
                queryObj.skill_id = String(rootSkillId)
              }
              if (matchedSkillIds.length > 0) {
                queryObj.skill_ids = matchedSkillIds.join(',')
              }
              const query = new URLSearchParams(queryObj).toString()

              return (
                <Link
                  to={`/drills/new?${query}`}
                  className="inline-flex items-center gap-1.5 rounded-md bg-destructive/10 border border-destructive/30 px-2.5 py-1 text-xs font-semibold text-destructive hover:bg-destructive/20 transition-colors"
                  title="针对当前最短板一键发起靶向模考"
                >
                  <Lightning className="size-3.5" weight="fill" />
                  <span>靶向攻坚：{matrix.weakest_cell.domain} · {ASSESSMENT_DIMENSION_LABELS[matrix.weakest_cell.question_type as AssessmentDimension] ?? matrix.weakest_cell.question_type}</span>
                </Link>
              )
            })()}
          </div>

          <div className="mt-4 space-y-2 md:hidden">
            {matrix.domains.map((dom) => {
              const open = (matrixDomain ?? matrix.domains[0]) === dom
              return (
                <div key={dom} className="rounded-lg border border-border">
                  <button
                    type="button"
                    className="flex min-h-[44px] w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm font-medium"
                    onClick={() => setMatrixDomain(open ? '' : dom)}
                    aria-expanded={open}
                  >
                    <span className="min-w-0">{dom}</span>
                    {open ? <CaretDown className="size-4 shrink-0" /> : <CaretRight className="size-4 shrink-0" />}
                  </button>
                  {open && (
                    <ul className="border-t border-border px-3 py-2">
                      {matrix.types.map((t) => {
                        const cell = matrix.cells.find((c) => c.domain === dom && c.question_type === t)
                        const label = ASSESSMENT_DIMENSION_LABELS[t as AssessmentDimension] ?? t
                        if (!cell || cell.count === 0) {
                          return (
                            <li key={t} className="flex items-center justify-between py-2 text-sm">
                              <span>{label}</span>
                              <span className="font-mono text-muted-foreground">—</span>
                            </li>
                          )
                        }
                        return (
                          <li key={t}>
                            <Link
                              to={`/questions?tag=${encodeURIComponent(dom)}&question_type=${encodeURIComponent(t)}`}
                              className="flex min-h-[44px] items-center justify-between py-2 text-sm"
                            >
                              <span>{label}</span>
                              <span className="font-mono tabular-nums font-semibold">{cell.proficiency}%</span>
                            </Link>
                          </li>
                        )
                      })}
                    </ul>
                  )}
                </div>
              )
            })}
          </div>

          <div className="mt-4 hidden overflow-x-auto md:block">
            <table className="w-full text-left text-xs border-collapse">
              <thead>
                <tr className="border-b border-border/60 text-muted-foreground">
                  <th className="py-2.5 px-3 font-semibold">技术知识域</th>
                  {matrix.types.map((t) => (
                    <th key={t} className="py-2.5 px-3 font-semibold text-center">
                      {ASSESSMENT_DIMENSION_LABELS[t as AssessmentDimension] ?? t}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody className="divide-y divide-border/40">
                {matrix.domains.map((dom) => (
                  <tr key={dom} className="hover:bg-muted/30 transition-colors">
                    <td className="py-2.5 px-3 font-medium text-foreground">
                      <Link
                        to={`/questions?tag=${encodeURIComponent(dom)}`}
                        className="hover:text-primary hover:underline transition-colors"
                        title={`查看「${dom}」全部题目`}
                      >
                        {dom}
                      </Link>
                    </td>
                    {matrix.types.map((t) => {
                      const cell = matrix.cells.find((c) => c.domain === dom && c.question_type === t)
                      if (!cell || cell.count === 0) {
                        return (
                          <td key={t} className="py-2.5 px-3 text-center">
                            <span className="text-muted-foreground/40 font-mono text-[11px]">—</span>
                          </td>
                        )
                      }
                      const colorBg =
                        cell.proficiency >= 80
                          ? 'bg-success/15 border-success/40 text-success'
                          : cell.proficiency >= 60
                          ? 'bg-warning/15 border-warning/40 text-warning'
                          : 'bg-destructive/15 border-destructive/40 text-destructive'
                      return (
                        <td key={t} className="py-2.5 px-3 text-center">
                          <Link
                            to={`/questions?tag=${encodeURIComponent(dom)}&question_type=${encodeURIComponent(t)}`}
                            className={cn(
                              'inline-flex flex-col items-center justify-center rounded-lg border px-2 py-1 transition-colors duration-150 hover:bg-muted',
                              colorBg
                            )}
                            title={`题量: ${cell.count} | 掌握度: ${cell.proficiency}% | 能力指数: ${cell.irt_theta}`}
                          >
                            <span className="font-mono font-bold text-xs">{cell.proficiency}%</span>
                            <span className="font-mono text-[10px] opacity-75">{cell.count}题</span>
                          </Link>
                        </td>
                      )
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}

      {data && <SkillRadarPanel dimensions={data.radar} />}

      <div className="grid grid-cols-1 gap-6 lg:grid-cols-3">
        <div className="space-y-6">

          <div className="rounded-lg border border-border bg-card">
            <div className="flex items-center justify-between border-b border-border px-3 py-2.5">
              <h2 className="text-sm font-semibold text-foreground">高频薄弱点排行榜</h2>
              <WarningCircle weight="fill" className="size-4 text-destructive" />
            </div>
            <div className="divide-y divide-border p-3">
              {weakNodes.length === 0 ? (
                <div className="py-6 text-center text-sm text-muted-foreground">
                  <CheckCircle className="mx-auto mb-1 size-5 text-success" />
                  当前没有严重薄弱技能，保持练习！
                </div>
              ) : (
                weakNodes.map((n) => (
                  <div key={n.id} className="flex items-center justify-between py-2.5 text-sm">
                    <div className="min-w-0 pr-2">
                      <Link
                        to={`/questions?tag=${encodeURIComponent(n.name)}`}
                        className="truncate font-medium text-foreground hover:text-primary hover:underline"
                      >
                        {n.name}
                      </Link>
                      <div className="font-mono text-xs text-muted-foreground">{n.path}</div>
                    </div>
                    <span className="rounded bg-destructive/10 px-1.5 py-0.5 font-mono text-xs font-bold text-destructive">
                      掌握度 {n.proficiency}%
                    </span>
                  </div>
                ))
              )}
            </div>
          </div>
        </div>

        {/* 右侧：层级技能树列表 */}
        <div className="rounded-lg border border-border bg-card p-5 lg:col-span-2">
          <div className="flex items-center justify-between border-b border-border pb-3">
            <div>
              <h2 className="text-sm font-semibold text-foreground">知识图谱目录树</h2>
              <p className="text-xs text-muted-foreground mt-0.5">
                6 大顶级领域固定承托 · 支持添加专区与考点 · 支持同义节点一键合并
              </p>
            </div>
            <div className="text-xs text-muted-foreground font-mono">共 {data?.tree.length ?? 0} 个顶级领域</div>
          </div>

          {loading ? (
            <div className="py-12 text-center text-xs text-muted-foreground">正在加载知识图谱…</div>
          ) : !data || data.tree.length === 0 ? (
            <div className="py-12 text-center text-xs text-muted-foreground">
              暂无技能节点，可点击上方「重置预置技能树」快速初始化
            </div>
          ) : (
            <>
            <div className="mt-4 md:hidden">
              {treePath.length > 0 && (
                <button
                  type="button"
                  className="mb-3 flex min-h-[44px] items-center gap-1 text-sm font-medium"
                  onClick={() => {
                    setTreePath((p) => p.slice(0, -1))
                    setTreeMoreId(null)
                  }}
                >
                  <CaretLeft className="size-4" />
                  {treePath.length === 1 ? '顶级领域' : treePath[treePath.length - 2].name}
                </button>
              )}
              {treePath.length > 0 && (
                <div className="mb-2 text-sm font-semibold text-heading">{treePath[treePath.length - 1].name}</div>
              )}
              <ul className="divide-y divide-border rounded-lg border border-border">
                {(treePath.length === 0 ? data.tree : treePath[treePath.length - 1].children).map((node) => (
                  <li key={node.id} className="px-3 py-2">
                    <div className="flex min-h-[44px] items-center gap-2">
                      {node.children.length > 0 ? (
                        <button
                          type="button"
                          className="min-w-0 flex-1 truncate text-left text-sm font-medium"
                          onClick={() => {
                            setTreePath((p) => [...p, node])
                            setTreeMoreId(null)
                          }}
                        >
                          {node.name}
                        </button>
                      ) : (
                        <Link
                          to={`/questions?tag=${encodeURIComponent(node.name)}`}
                          className="min-w-0 flex-1 truncate text-sm font-medium"
                        >
                          {node.name}
                        </Link>
                      )}
                      <span className="shrink-0 font-mono text-xs tabular-nums">
                        {node.question_count > 0 ? `${node.proficiency}%` : '—'}
                      </span>
                      <button
                        type="button"
                        className="grid size-9 place-items-center rounded-md"
                        aria-label="更多"
                        onClick={() => setTreeMoreId((id) => (id === node.id ? null : node.id))}
                      >
                        <DotsThree className="size-5" weight="bold" />
                      </button>
                    </div>
                    {treeMoreId === node.id && (
                      <div className="mt-1 flex flex-wrap gap-2 pb-1">
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            setCreateModal({ open: true, parentId: node.id, parentName: node.name })
                            setSkillNameInput('')
                            setTreeMoreId(null)
                          }}
                        >
                          添加子节点
                        </Button>
                        {treePath.length > 0 && (
                          <>
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={() => {
                                setEditModal({ open: true, id: node.id, name: node.name })
                                setSkillNameInput(node.name)
                                setTreeMoreId(null)
                              }}
                            >
                              重命名
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={() => {
                                setMergeModal({ open: true, sourceId: node.id, sourceName: node.name })
                                setMergeTargetId('')
                                setTreeMoreId(null)
                              }}
                            >
                              合并
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="text-destructive"
                              onClick={() => {
                                handleDelete(node.id, node.name)
                                setTreeMoreId(null)
                              }}
                            >
                              删除
                            </Button>
                          </>
                        )}
                      </div>
                    )}
                  </li>
                ))}
              </ul>
            </div>
            <div className="mt-4 hidden space-y-3 md:block">
              {data.tree.map((root) => (
                <TreeNodeItem
                  key={root.id}
                  node={root}
                  level={0}
                  collapsed={collapsed}
                  onToggleCollapse={toggleCollapse}
                  onAddChild={(pid, pname) => {
                    setCreateModal({ open: true, parentId: pid, parentName: pname })
                    setSkillNameInput('')
                  }}
                  onEdit={(id, name) => {
                    setEditModal({ open: true, id, name })
                    setSkillNameInput(name)
                  }}
                  onDelete={handleDelete}
                  onMerge={(id, name) => {
                    setMergeModal({ open: true, sourceId: id, sourceName: name })
                    setMergeTargetId('')
                  }}
                />
              ))}
            </div>
            </>
          )}
        </div>
      </div>

      {/* 新增技能弹窗 */}
      {createModal.open && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm">
          <div className="w-full max-w-md rounded-lg border border-border bg-card p-6 shadow-xl">
            <h3 className="text-base font-bold text-foreground">
              新增技能节点 {createModal.parentId ? `（归属: ${createModal.parentName}）` : ''}
            </h3>
            <div className="mt-4 space-y-3">
              <div>
                <label className="text-xs font-medium text-muted-foreground">技能名称</label>
                <input
                  type="text"
                  value={skillNameInput}
                  onChange={(e) => setSkillNameInput(e.target.value)}
                  placeholder="如：Redis 缓存穿透与击穿"
                  className="mt-1 w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-primary"
                  autoFocus
                />
              </div>
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <button
                onClick={() => setCreateModal({ open: false, parentId: null, parentName: '' })}
                className="rounded-md border border-border px-3 py-1.5 text-xs font-medium text-muted-foreground hover:bg-muted"
              >
                取消
              </button>
              <button
                onClick={handleCreate}
                className="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90"
              >
                创建
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 编辑技能弹窗 */}
      {editModal.open && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm">
          <div className="w-full max-w-md rounded-lg border border-border bg-card p-6 shadow-xl">
            <h3 className="text-base font-bold text-foreground">编辑技能名称</h3>
            <div className="mt-4 space-y-3">
              <div>
                <label className="text-xs font-medium text-muted-foreground">技能名称</label>
                <input
                  type="text"
                  value={skillNameInput}
                  onChange={(e) => setSkillNameInput(e.target.value)}
                  className="mt-1 w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-primary"
                  autoFocus
                />
              </div>
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <button
                onClick={() => setEditModal({ open: false, id: 0, name: '' })}
                className="rounded-md border border-border px-3 py-1.5 text-xs font-medium text-muted-foreground hover:bg-muted"
              >
                取消
              </button>
              <button
                onClick={handleUpdate}
                className="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90"
              >
                保存
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 合并同义技能节点弹窗 */}
      {mergeModal.open && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm">
          <div className="w-full max-w-md rounded-lg border border-border bg-card p-6 shadow-xl">
            <h3 className="text-base font-bold text-foreground flex items-center gap-2">
              <GitMerge className="size-5 text-primary" />
              <span>合并技能节点</span>
            </h3>
            <p className="mt-2 text-xs text-muted-foreground leading-relaxed">
              将节点 <b className="text-foreground font-semibold">「{mergeModal.sourceName}」</b> 关联的所有题目及子技能一并迁移至目标节点，并删除原节点。
            </p>
            <div className="mt-4 space-y-3">
              <div>
                <label className="text-xs font-medium text-muted-foreground">选择合并目标节点</label>
                <select
                  value={mergeTargetId}
                  onChange={(e) => setMergeTargetId(e.target.value ? Number(e.target.value) : '')}
                  className="mt-1.5 w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-primary"
                >
                  <option value="">-- 请选择目标技能节点 --</option>
                  {mergeCandidates.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name} ({c.path})
                    </option>
                  ))}
                </select>
              </div>
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <button
                disabled={merging}
                onClick={() => setMergeModal({ open: false, sourceId: 0, sourceName: '' })}
                className="rounded-md border border-border px-3 py-1.5 text-xs font-medium text-muted-foreground hover:bg-muted"
              >
                取消
              </button>
              <button
                disabled={merging || !mergeTargetId}
                onClick={handleMerge}
                className="rounded-md bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
              >
                {merging ? '合并中…' : '确认合并'}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 初始化默认技能树确认 */}
      <ConfirmDialog
        open={seedConfirmOpen}
        onOpenChange={setSeedConfirmOpen}
        busy={busy}
        title="重置并初始化默认技能树？"
        description="系统预设大纲节点将被同步修复，已添加的自定义节点仍将保留。"
        confirmLabel="确认初始化"
        onConfirm={doSeed}
      />

      {/* 删除技能节点确认 */}
      <ConfirmDialog
        open={delTarget !== null}
        onOpenChange={(open) => !open && setDelTarget(null)}
        destructive
        busy={busy}
        title={`删除技能节点「${delTarget?.name ?? ''}」？`}
        description="其子技能将一并删除，关联题目将自动移出本节点。"
        confirmLabel="确认删除"
        onConfirm={doDelete}
      />
    </div>
  )
}

interface TreeNodeItemProps {
  node: SkillTreeNode
  level: number
  collapsed: Record<number, boolean>
  onToggleCollapse: (id: number) => void
  onAddChild: (id: number, name: string) => void
  onEdit: (id: number, name: string) => void
  onDelete: (id: number, name: string) => void
  onMerge: (id: number, name: string) => void
}

function TreeNodeItem({
  node,
  level,
  collapsed,
  onToggleCollapse,
  onAddChild,
  onEdit,
  onDelete,
  onMerge,
}: TreeNodeItemProps) {
  const isCollapsed = !!collapsed[node.id]
  const hasChildren = node.children.length > 0
  const isRoot = level === 0

  // 掌握度进度条颜色判定
  const profColor =
    node.proficiency >= 80
      ? 'bg-success'
      : node.proficiency >= 60
      ? 'bg-primary'
      : node.question_count > 0
      ? 'bg-destructive'
      : 'bg-muted'

  return (
    <div className={cn('space-y-1.5', level > 0 && 'border-l border-border/60 pl-3')}>
      <div className="group flex items-center justify-between rounded-lg p-2 transition-colors hover:bg-muted/60">
        <div className="flex min-w-0 items-center gap-2">
          {hasChildren ? (
            <button
              onClick={() => onToggleCollapse(node.id)}
              aria-label={isCollapsed ? `展开 ${node.name}` : `折叠 ${node.name}`}
              className="grid size-6 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground active:scale-95"
            >
              {isCollapsed ? <CaretRight className="size-4" /> : <CaretDown className="size-4" />}
            </button>
          ) : (
            <div className="size-6" />
          )}

          <TreeStructure className="size-4 shrink-0 text-muted-foreground" />
          <Link
            to={`/questions?tag=${encodeURIComponent(node.name)}`}
            className="truncate text-sm font-medium text-foreground transition-colors hover:text-primary hover:underline"
            title={`点击查看「${node.name}」关联题目清单`}
          >
            {node.name}
          </Link>
          {isRoot && (
            <span className="rounded-md bg-secondary border border-border-strong px-1.5 py-0.5 text-[10px] font-semibold text-heading">
              顶级领域
            </span>
          )}
          <span className="rounded-md bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
            {node.question_count} 题
          </span>
        </div>

        <div className="flex items-center gap-2 sm:gap-3">
          {/* 掌握度进度条 */}
          <div className="hidden items-center gap-2 sm:flex">
            <div className="h-1.5 w-20 overflow-hidden rounded-full bg-muted">
              <div
                className={cn('h-full transition-all duration-300', profColor)}
                style={{ width: `${node.proficiency}%` }}
              />
            </div>
            <span className="w-8 font-mono text-xs font-bold text-foreground">
              {node.question_count > 0 ? `${node.proficiency}%` : '—'}
            </span>
          </div>

          {/* 操作按钮组：移动端常驻可见，桌面端 hover 显示 */}
          <div className="flex items-center gap-0.5 opacity-100 sm:opacity-0 sm:group-hover:opacity-100 transition-opacity">
            <button
              onClick={() => onAddChild(node.id, node.name)}
              title="添加子技能"
              className="grid size-8 sm:size-7 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground active:scale-95"
            >
              <FolderSimplePlus className="size-4 sm:size-3.5" />
            </button>
            {!isRoot && (
              <>
                <button
                  onClick={() => onMerge(node.id, node.name)}
                  title="合并到其他节点…"
                  className="grid size-8 sm:size-7 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground active:scale-95"
                >
                  <GitMerge className="size-4 sm:size-3.5" />
                </button>
                <button
                  onClick={() => onEdit(node.id, node.name)}
                  title="重命名"
                  className="grid size-8 sm:size-7 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground active:scale-95"
                >
                  <PencilSimple className="size-4 sm:size-3.5" />
                </button>
                <button
                  onClick={() => onDelete(node.id, node.name)}
                  title="删除"
                  className="grid size-8 sm:size-7 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-destructive active:scale-95"
                >
                  <Trash className="size-4 sm:size-3.5" />
                </button>
              </>
            )}
          </div>
        </div>
      </div>

      {/* 子节点递归展示 */}
      {hasChildren && !isCollapsed && (
        <div className="space-y-1 pl-2">
          {node.children.map((child) => (
            <TreeNodeItem
              key={child.id}
              node={child}
              level={level + 1}
              collapsed={collapsed}
              onToggleCollapse={onToggleCollapse}
              onAddChild={onAddChild}
              onEdit={onEdit}
              onDelete={onDelete}
              onMerge={onMerge}
            />
          ))}
        </div>
      )}
    </div>
  )
}
