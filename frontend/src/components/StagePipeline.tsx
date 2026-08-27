/**
 * 面试进度 Pipeline（反馈六#2 设计规范 §3）：节点型进度锚点。
 * - 每轮一个节点，按创建顺序排列；当前轮（最新创建且未出结果）为环形强调节点
 * - 历史节点小实心点（pass 绿 / fail 红 / pending 中性灰）
 * - 只表达「走到哪了」，不重复展示状态文字；横向可延伸（overflow-x-auto）
 * 设计语言 v2（ADR-0015）：语义 token，双主题自适应。
 */
export default function StagePipeline({
  stages,
  currentIndex,
}: {
  stages: { name: string; passed: string }[]
  /** 当前轮索引；-1 表示无当前轮（终态或全部已有结论） */
  currentIndex: number
}) {
  if (stages.length === 0) return null
  return (
    <div
      className="flex items-center gap-3 overflow-x-auto py-2.5"
      role="img"
      aria-label="面试进度"
    >
      {stages.map((s, i) => {
        const isCurrent = i === currentIndex
        const dotCls =
          s.passed === 'pass' ? 'bg-success' : s.passed === 'fail' ? 'bg-destructive' : 'bg-muted-foreground'
        return (
          <div className="flex shrink-0 items-center gap-3" key={i}>
            {i > 0 && (
              <span
                className={`h-px w-6 ${i <= currentIndex ? 'bg-primary' : 'bg-border-strong'}`}
                aria-hidden
              />
            )}
            <span className="flex items-center gap-1.5">
              <span
                className={`size-2.5 rounded-full ${isCurrent ? 'bg-primary shadow-[0_0_0_4px] shadow-primary/20' : dotCls}`}
                aria-hidden
              />
              <span className={`whitespace-nowrap text-xs ${isCurrent ? 'font-semibold text-primary' : 'text-muted-foreground'}`}>
                {s.name || `第${i + 1}轮`}
              </span>
            </span>
          </div>
        )
      })}
    </div>
  )
}
