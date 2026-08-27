import { useNavigate, useSearchParams } from 'react-router-dom'
import QuestionEntry from '../components/QuestionEntry'
import { PageHeader } from '../components/PageHeader'
import { Section } from '../components/Section'

export default function NewQuestion() {
  const [sp] = useSearchParams()
  const nav = useNavigate()
  const roundId = sp.get('round_id') ? Number(sp.get('round_id')) : undefined
  return (
    <div>
      <PageHeader title="录入题目" meta={<span>写入自录或当前轮次</span>} />
      <Section>
        {/* 从轮次进入（?round_id=）：保存成功后回到该轮次详情页，形成录题闭环（反馈 #8） */}
        <QuestionEntry
          initialRoundId={roundId}
          onDone={roundId ? () => nav(`/rounds/${roundId}`) : undefined}
        />
      </Section>
    </div>
  )
}
