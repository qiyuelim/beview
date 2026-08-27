import { useEffect, useState } from 'react'
import { apiDelete, apiGet, apiPost } from '../api/client'
import type { DailyProgress, LedgerEntry, MallItem } from '../api/types'
import { Coin, Plus, Trash } from '@phosphor-icons/react'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'
import { ConfirmDialog } from '../components/ConfirmDialog'
import { FormField } from '../components/FormField'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { toast } from 'sonner'

const LEDGER_PREVIEW = 5
const LEDGER_PAGE = 20

/** 积分页（v4 评审 P2：自数据页拆出独立导航，ADR-0011 R2 修订） */
export default function PointsPage() {
  return (
    <div className="mx-auto w-full">
      <PageHeader title="积分" meta={<span>真实面试收益高于日常陪练</span>} />
      <PointsSection />
    </div>
  )
}

export function PointsSection() {
  const [balance, setBalance] = useState<number | null>(null)
  const [items, setItems] = useState<MallItem[]>([])
  const [daily, setDaily] = useState<DailyProgress | null>(null)
  // 积分明细分页：offset 翻页 + 按类筛选 + 日期范围
  const [ledger, setLedger] = useState<LedgerEntry[]>([])
  const [cat, setCat] = useState('')
  const [from, setFrom] = useState('')
  const [to, setTo] = useState('')
  const [offset, setOffset] = useState(0)
  const [hasMore, setHasMore] = useState(false)
  const [ledgerOpen, setLedgerOpen] = useState(false)
  const [err, setErr] = useState('')
  const [newItem, setNewItem] = useState({ name: '', cost: '', emoji: '🎁' })
  // 确认弹窗：兑换 / 删除奖励
  const [redeemTarget, setRedeemTarget] = useState<MallItem | null>(null)
  const [removeTarget, setRemoveTarget] = useState<MallItem | null>(null)

  function ledgerQuery(off: number, c = cat, f = from, t = to, limit = LEDGER_PAGE) {
    const p = new URLSearchParams({ limit: String(limit), offset: String(off) })
    if (c) p.set('category', c)
    if (f) p.set('from', f)
    if (t) p.set('to', t)
    return `/api/points/ledger?${p}`
  }

  async function loadLedger(reset = false) {
    const off = reset ? 0 : offset
    const l: LedgerEntry[] = await apiGet(ledgerQuery(off))
    setLedger(reset ? l : [...ledger, ...l])
    setOffset(off + l.length)
    setHasMore(l.length === LEDGER_PAGE)
  }

  // 筛选条件变化后重置翻页重新拉取（显式传参避免 setState 异步旧值）
  async function refetchHead(c: string, f: string, t: string) {
    try {
      const l: LedgerEntry[] = await apiGet(ledgerQuery(0, c, f, t))
      setLedger(l)
      setOffset(l.length)
      setHasMore(l.length === LEDGER_PAGE)
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function load() {
    const [b, it, d] = await Promise.all([
      apiGet('/api/points/balance'),
      apiGet('/api/mall/items'),
      apiGet('/api/points/daily'),
    ])
    setBalance(b.balance)
    setItems(it)
    setDaily(d)
    const l: LedgerEntry[] = await apiGet(ledgerQuery(0, '', '', '', LEDGER_PREVIEW + 1))
    setHasMore(l.length > LEDGER_PREVIEW)
    setLedger(l.slice(0, LEDGER_PREVIEW))
    setOffset(Math.min(l.length, LEDGER_PREVIEW))
    setLedgerOpen(false)
  }
  useEffect(() => {
    load().catch((e) => setErr(e.message))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  function changeCat(c: string) {
    setCat(c)
    refetchHead(c, from, to)
  }

  function changeRange(f: string, t: string) {
    setFrom(f)
    setTo(t)
    refetchHead(cat, f, t)
  }

  async function redeem(item: MallItem) {
    setErr('')
    try {
      const r = await apiPost(`/api/mall/items/${item.id}/redeem`)
      toast.success(`已兑换「${item.name}」 −${r.cost} 分，余额 ${r.balance}`)
      setRedeemTarget(null)
      await load()
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function addItem() {
    const cost = parseInt(newItem.cost, 10)
    if (!newItem.name.trim() || !cost) return
    setErr('')
    try {
      await apiPost('/api/mall/items', { name: newItem.name.trim(), cost, emoji: newItem.emoji || '🎁' })
      setNewItem({ name: '', cost: '', emoji: '🎁' })
      await load()
    } catch (e: any) {
      setErr(e.message)
    }
  }

  async function removeItem() {
    if (!removeTarget) return
    await apiDelete(`/api/mall/items/${removeTarget.id}`)
    setRemoveTarget(null)
    await load()
  }

  const CAT_LABEL: Record<string, string> = {
    review_card: '复习打卡',
    daily_goal: '今日队列清空',
    streak7: '连续 7 天',
    drill: '完成陪练',
    real_question: '新增真实面试题',
    real_session: '新建真实面试批次',
    round_pass: '轮次通过',
    ai_sink: '题目 AI 判分沉淀',
    manual_analysis: '深度分析',
    batch_analysis: '题目 AI 判分沉淀',
    redemption: '商城兑换',
    milestone: '里程碑',
  }

  return (
    <>
      {err && (
        <p role="alert" className="mb-3 rounded-md bg-destructive/10 px-3 py-2 text-sm font-medium text-destructive">
          {err}
        </p>
      )}

      <Section
        title="今日任务 · 可挣积分"
        action={
          <span className="inline-flex items-center gap-1.5 rounded-full bg-primary px-2.5 py-1 text-sm font-semibold text-primary-foreground" title="当前积分余额">
            <Coin weight="fill" className="size-4" aria-hidden />
            <b className="font-mono tabular-nums">{balance ?? '…'}</b>
          </span>
        }
      >
        {daily && (
          <div className="space-y-1">
            <div className="flex items-baseline justify-between text-sm">
              <span>复习打卡（+5）</span>
              <b className="font-mono tabular-nums">{daily.cards_today} 张</b>
            </div>
            <div className="flex items-baseline justify-between text-sm">
              <span>今日队列 100%（+20）</span>
              {daily.queue_done ? (
                <b className="font-mono tabular-nums text-success">✓ 已达标</b>
              ) : (
                <b className="font-mono tabular-nums">
                  {daily.done_today} / {Math.max(daily.due_today, 1)}
                </b>
              )}
            </div>
            <div className="flex items-baseline justify-between text-sm">
              <span>完成一场陪练（+30）</span>
              <b className="font-mono tabular-nums">{daily.drills_today} 场</b>
            </div>
            <div className="flex items-baseline justify-between text-sm">
              <span>新增真实面试题（+100/题）</span>
              <b className="font-mono tabular-nums">实时入账</b>
            </div>
            <div className="flex items-baseline justify-between text-sm">
              <span>里程碑</span>
              <b className="font-mono tabular-nums">累计 5/10/20 场真面试 +2k/5k/10k</b>
            </div>
          </div>
        )}
      </Section>

      <Section title="商城 · 目录" className="mt-4">
        <p className="mb-2.5 text-xs text-muted-foreground">
          攒够才有资格解锁加餐与购物——兑换即自授（honor system），流水可回溯。
        </p>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {items.map((it) => (
            <div key={it.id} className="relative rounded-lg border border-border p-3 text-center">
              <Button
                size="icon"
                variant="ghost"
                className="absolute right-1 top-1 size-6 text-muted-foreground hover:text-destructive"
                onClick={() => setRemoveTarget(it)}
                aria-label="删除"
                title="删除"
              >
                <Trash className="size-3.5" aria-hidden />
              </Button>
              <div className="text-3xl" aria-hidden>
                {it.emoji}
              </div>
              <div className="mt-1 truncate text-sm font-medium">{it.name}</div>
              <div className="mt-0.5 inline-flex items-center gap-1 text-sm text-muted-foreground">
                <Coin className="size-3.5" aria-hidden /> {it.cost}
              </div>
              <Button
                size="sm"
                className="mt-2 w-full"
                disabled={(balance ?? 0) < it.cost}
                onClick={() => setRedeemTarget(it)}
              >
                {it.cost <= (balance ?? 0) ? '兑换' : `还差 ${it.cost - (balance ?? 0)}`}
              </Button>
            </div>
          ))}
        </div>
      </Section>

      <Section title="新增奖励" className="mt-4">
        <form
          className="grid grid-cols-1 gap-3 sm:grid-cols-3"
          onSubmit={(e) => {
            e.preventDefault()
            addItem()
          }}
        >
          <FormField label="奖励名称" htmlFor="mi-name">
            <Input id="mi-name" placeholder="例如：一顿火锅" value={newItem.name} onChange={(e) => setNewItem((s) => ({ ...s, name: e.target.value }))} />
          </FormField>
          <FormField label="积分成本" htmlFor="mi-cost">
            <Input id="mi-cost" placeholder="例如：2000" type="number" min={1} value={newItem.cost} onChange={(e) => setNewItem((s) => ({ ...s, cost: e.target.value }))} />
          </FormField>
          <FormField label="图标（emoji）" htmlFor="mi-emoji">
            <Input id="mi-emoji" placeholder="🎁" value={newItem.emoji} onChange={(e) => setNewItem((s) => ({ ...s, emoji: e.target.value }))} />
          </FormField>
          <div className="sm:col-span-3">
            <Button type="submit" disabled={!newItem.name.trim() || !newItem.cost}>
              <Plus weight="bold" className="size-4" aria-hidden /> 添加奖励
            </Button>
          </div>
        </form>
      </Section>

      <Section
        title="积分流水"
        className="mt-4"
        action={
          ledgerOpen ? (
          <div className="flex flex-wrap items-center gap-1.5">
            <Input type="date" value={from} onChange={(e) => changeRange(e.target.value, to)} aria-label="开始日期" className="h-8 w-36" />
            <span className="text-xs text-muted-foreground">至</span>
            <Input type="date" value={to} onChange={(e) => changeRange(from, e.target.value)} aria-label="结束日期" className="h-8 w-36" />
            {(from || to) && (
              <Button size="sm" variant="ghost" onClick={() => changeRange('', '')}>
                清日期
              </Button>
            )}
            <select value={cat} onChange={(e) => changeCat(e.target.value)} aria-label="按类型筛选" className="h-8 rounded-md border border-input bg-card px-2 text-sm">
              <option value="">全部类型</option>
              {Object.entries(CAT_LABEL).map(([k, label]) => (
                <option key={k} value={k}>
                  {label}
                </option>
              ))}
            </select>
          </div>
          ) : undefined
        }
      >
        {ledger.length === 0 ? (
          <p className="text-sm text-muted-foreground">还没有积分流水</p>
        ) : (
          <>
            <ul className="divide-y divide-border">
              {ledger.map((e) => {
                const label = CAT_LABEL[e.category] || e.category
                const showNote = e.note && e.category !== 'redemption' && e.note !== label
                return (
                <li key={e.id} className="flex items-start justify-between gap-3 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium text-foreground">{label}</div>
                    {showNote && (
                      <div className="mt-0.5 text-xs leading-relaxed text-muted-foreground break-words">{e.note}</div>
                    )}
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-0.5">
                    <span
                      className={`font-mono text-sm font-semibold tabular-nums ${
                        e.amount > 0 ? 'text-success' : 'text-destructive'
                      }`}
                    >
                      {e.amount > 0 ? `+${e.amount}` : e.amount}
                    </span>
                    <span className="font-mono text-[10px] tabular-nums text-muted-foreground">
                      {e.created_at.slice(0, 16).replace('T', ' ')}
                    </span>
                  </div>
                </li>
                )
              })}
            </ul>
            {!ledgerOpen && hasMore && (
              <Button
                size="sm"
                variant="ghost"
                className="mt-2"
                onClick={() => {
                  setLedgerOpen(true)
                  refetchHead(cat, from, to).catch((e) => setErr(e.message))
                }}
              >
                查看全部流水
              </Button>
            )}
            {ledgerOpen && hasMore && (
              <Button size="sm" variant="ghost" className="mt-2" onClick={() => loadLedger().catch((e) => setErr(e.message))}>
                加载更多
              </Button>
            )}
          </>
        )}
      </Section>

      {/* 兑换确认（honor system） */}
      <ConfirmDialog
        open={redeemTarget !== null}
        onOpenChange={(v) => !v && setRedeemTarget(null)}
        title={`用 ${redeemTarget?.cost ?? 0} 分兑换「${redeemTarget?.emoji ?? ''} ${redeemTarget?.name ?? ''}」？`}
        description="honor system：兑换后自行享受，流水可回溯。"
        confirmLabel="兑换"
        onConfirm={() => redeemTarget && redeem(redeemTarget)}
      />
      {/* 删除奖励确认 */}
      <ConfirmDialog
        open={removeTarget !== null}
        onOpenChange={(v) => !v && setRemoveTarget(null)}
        destructive
        title="删除该奖励条目？"
        confirmLabel="删除"
        onConfirm={removeItem}
      />
    </>
  )
}
