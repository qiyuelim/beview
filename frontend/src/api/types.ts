// ---------- 考察维度（七类通用维度，票 03）----------
// 全仓库唯一来源：后端 schema enum / Rust fallback / 前端标签 / 筛选 chip 共用此定义。

export type AssessmentDimension =
  | 'motivation_culture_fit'
  | 'experience_track_record'
  | 'professional_knowledge'
  | 'scenario_case'
  | 'practice_execution'
  | 'problem_solving_resilience'
  | 'collaboration'

export const ASSESSMENT_DIMENSION_LABELS: Record<AssessmentDimension, string> = {
  motivation_culture_fit: '意愿与适配度',
  experience_track_record: '履历与项目深挖',
  professional_knowledge: '专业理论基础',
  scenario_case: '业务场景推演',
  practice_execution: '现场实操交付',
  problem_solving_resilience: '异常与危机处理',
  collaboration: '沟通与团队协同',
}

// 三模块展示分组（前端 MODULE_GROUPS，不进 DB）
export const ASSESSMENT_DIMENSION_GROUPS: { name: string; dims: AssessmentDimension[] }[] = [
  { name: '动机与过去', dims: ['motivation_culture_fit', 'experience_track_record'] },
  { name: '核心业务能力', dims: ['professional_knowledge', 'scenario_case', 'practice_execution', 'problem_solving_resilience'] },
  { name: '软性协同', dims: ['collaboration'] },
]

/** 七类考察维度 → 陪练 stage slug（最薄弱项直达陪练） */
export const ASSESSMENT_DIMENSION_STAGE: Record<AssessmentDimension, string> = {
  motivation_culture_fit: 'basics',
  experience_track_record: 'project',
  professional_knowledge: 'basics',
  scenario_case: 'scenario',
  practice_execution: 'comprehensive',
  problem_solving_resilience: 'comprehensive',
  collaboration: 'basics',
}

export interface SkillRow {
  id: number
  user_id: number
  parent_id: number | null
  name: string
  path: string
  icon: string | null
  visibility: string
  created_at: string
  updated_at: string
}

export interface SkillTreeNode {
  id: number
  parent_id: number | null
  name: string
  path: string
  icon: string | null
  question_count: number
  proficiency: number
  weakness_index: number
  avg_score: number | null
  children: SkillTreeNode[]
}

export interface RadarDimension {
  key: string
  name: string
  score: number
  question_count: number
}

export interface SkillGraphData {
  tree: SkillTreeNode[]
  radar: RadarDimension[]
  total_skills: number
  total_tagged_questions: number
  overall_proficiency: number
}

export function flattenSkillTree(nodes: any[]): { id: number; name: string; path: string }[] {
  const result: { id: number; name: string; path: string }[] = []
  function walk(list: any[]) {
    for (const n of list || []) {
      if (n && n.id && n.name) {
        result.push({ id: n.id, name: n.name, path: n.path || '' })
      }
      if (n && n.children && n.children.length > 0) {
        walk(n.children)
      }
    }
  }
  walk(nodes)
  return result
}

export interface MatrixCell {
  domain: string
  question_type: string
  count: number
  avg_score: number
  proficiency: number
  irt_theta: number
}

export interface SkillMatrixData {
  domains: string[]
  types: string[]
  cells: MatrixCell[]
  weakest_cell: MatrixCell | null
}

export const QUESTION_TYPES = (
  Object.entries(ASSESSMENT_DIMENSION_LABELS) as [AssessmentDimension, string][]
).map(([value, label]) => ({ value, label }))

export const QUESTION_TYPE_LABELS: Record<string, string> = { ...ASSESSMENT_DIMENSION_LABELS }

export interface PredictedQuestionItem {
  content: string
  category: string
  focus_points: string[]
  sample_direction: string
  probability: number | null
}

export interface PositionPredictResponse {
  summary: string
  questions: PredictedQuestionItem[]
  text_fallback?: string | null
}

export interface CompanySummary {
  id: number
  name: string
  session_count: number
  question_count: number
  avg_score: number | null
  last_interview: string | null
  sessions: CompanySessionBrief[]
}

export interface CompanySessionBrief {
  id: number
  department: string | null
  position: string | null
  status: string
  started_at: string | null
  question_count: number
  avg_score: number | null
}

export interface SessionView {
  id: number
  company_id: number
  department: string | null
  position: string | null
  started_at: string | null
  status: string
  created_at: string
  round_count: number
  question_count: number
  avg_score: number | null
}

export interface RoundView {
  id: number
  session_id: number
  name: string
  sort_order: number
  date: string | null
  passed: string
  created_at: string
  question_count: number
  avg_score: number | null
}

export interface QuestionRow {
  id: number
  round_id: number
  parent_id?: number | null
  content: string
  my_answer: string | null
  starred: boolean
  asked_at: string | null
  created_at: string
  source: 'manual' | 'ai_drill'
  tags: string[]
  analyzed: boolean
  last_score: number | null
  last_difficulty: number | null
  last_feedback?: string | null
  company: string | null
  followup_count?: number
  skill_id?: number | null
  skill_name?: string | null
  skill_path?: string | null
  question_type?: string | null
  department?: string | null
  position?: string | null
}

export interface AnalysisRow {
  id: number
  provider: string | null
  model: string | null
  tags: string[] | null
  difficulty: number | null
  ref_answer: string | null
  score: number | null
  feedback: string | null
  answer_snapshot: string | null
  created_at: string
}

export interface CommentRow {
  id: number
  body: string
  created_at: string
}

export interface CompanyDetail {
  id: number
  name: string
  created_at: string
  sessions: SessionView[]
}

export interface SessionDetail extends SessionView {
  rounds: RoundView[]
  comments: CommentRow[]
}

export interface QuestionDetail extends QuestionRow {
  followups?: QuestionRow[]
  analyses: AnalysisRow[]
  comments: CommentRow[]
  answers: AnswerRow[]
  round_links: RoundLinkRow[]
  /** 疑似重复题（票02，归一化键相等；双向对称） */
  duplicates?: { id: number; content: string }[]
}

export interface UnmappedTag {
  tag: string
  question_count: number
}

export interface AnswerRow {
  id: number
  question_id: number
  source: 'manual' | 'review' | 'interview'
  content: string
  created_at: string
}

export interface RoundLinkRow {
  round_id: number
  round_name: string
  session_id: number
  department: string | null
  position: string | null
  company: string | null
  date: string | null
  passed: string
}

export const ANSWER_SOURCE: Record<string, string> = {
  manual: '手动补答',
  review: '复习自评',
  interview: '面试作答',
}

export interface User {
  id: number
  username: string
  role: string
  created_at: string
}

export interface LlmSettings {
  llm_base_url: string | null
  llm_api_key: string | null
  llm_model: string | null
  llm_timeout: number | null
  llm_thinking: boolean | null
  has_key: boolean
}

// ---------- LLM 配置（ADR-0016：多 Provider × 多 Model + 能力位 + 高级参数） ----------

export interface LlmProvider {
  id: string
  name: string
  base_url: string
  /** GET 时为掩码；PUT 时 * 开头=未修改，空串=清除，其余=新明文 */
  api_key: string
  has_key?: boolean
}

export interface LlmModelCaps {
  structured_output: boolean
  web_search: boolean
}

export interface LlmModelAdvanced {
  temperature?: number | null
  top_p?: number | null
  /** none|minimal|low|medium|high|xhigh|max；null=不下发 */
  reasoning_effort?: string | null
  store?: boolean | null
  extra_body?: Record<string, unknown>
}

export interface LlmModel {
  id: string
  provider_id: string
  name: string
  context_length?: number | null
  caps: LlmModelCaps
  advanced: LlmModelAdvanced
}

export interface LlmGlobalCfg {
  timeout: number
  max_output_tokens_short: number
  max_output_tokens_long: number
}

export interface LlmConfigDoc {
  providers: LlmProvider[]
  models: LlmModel[]
  active_model_id?: string | null
  global: LlmGlobalCfg
}

export interface LlmResolved {
  provider: string
  model: string
  structured_output: boolean
  web_search: boolean
  reasoning_effort: string | null
}

export const SESSION_STATUS: Record<string, string> = {
  ongoing: '进行中',
  offer: 'Offer',
  rejected: '未通过',
  withdrawn: '放弃',
}

export const ROUND_PASSED: Record<string, string> = {
  pending: '待定',
  pass: '通过',
  fail: '未通过',
}

export const PASSED_ICON: Record<string, string> = {
  pending: '⏳',
  pass: '✅',
  fail: '❌',
}

export const STATUS_COLOR: Record<string, string> = {
  // 大公司（Google/Material）风格高对比色板，一律配白字
  ongoing: '#1a73e8',   // 蓝：进行中
  offer: '#188038',     // 绿：Offer
  rejected: '#d93025',  // 红：未通过
  withdrawn: '#5f6368', // 灰：放弃
  pending: '#e8710a',   // 橙：轮次待定
  pass: '#188038',      // 绿：通过
  fail: '#d93025',      // 红：未通过
}

/** 状态徽标样式：彩色底 + 白字（避免黑字蓝底看不清） */
export const STATUS_STYLE = (status: string): { background: string; color: string } => ({
  background: STATUS_COLOR[status] || '#8a8f93',
  color: '#ffffff',
})

// ---------- 仪表盘 ----------

export interface DashboardSummary {
  companies: number
  sessions: number
  questions: number
  analyzed: number
  unanalyzed: number
  unanswered: number
  starred: number
  pending_rounds: number
  avg_score: number | null
  avg_difficulty: number | null
}

export interface LightQuestion {
  id: number
  content: string
  created_at: string
  company: string | null
  session: string | null
  round: string | null
}

export interface PendingRound {
  id: number
  name: string
  company: string | null
  session: string | null
}

export interface RecentAnalysis {
  id: number
  question_id: number
  content: string
  score: number | null
  difficulty: number | null
  created_at: string
  company: string | null
}

export interface TagCount {
  name: string
  cnt: number
}

export interface RecentSession {
  id: number
  company: string | null
  department: string | null
  position: string | null
  status: string
  started_at: string | null
}

export interface Dashboard {
  summary: DashboardSummary
  unanswered: LightQuestion[]
  unanalyzed: LightQuestion[]
  pending_rounds: PendingRound[]
  recent_analyses: RecentAnalysis[]
  top_tags: TagCount[]
  recent_sessions: RecentSession[]
}

// ---------- 复习（v2） ----------

export interface ReviewQueueItem {
  question_id: number
  content: string
  my_answer: string | null
  source: string
  difficulty: number | null
  score: number | null
  ref_answer: string | null
  feedback: string | null
  tags: string[]
  company: string | null
  last_result: string | null
  interval_days: number
  next_review_at: string
}

export interface WrongItem {
  question_id: number
  content: string
  my_answer: string | null
  source: string
  last_result: string | null
  review_count: number
  score: number | null
  ref_answer: string | null
  tags: string[]
  company: string | null
}

export interface ReviewStats {
  due: number
  done_today: number
  remembered: number
  fuzzy: number
  forgot: number
  streak_days: number
}

// ---------- 陪练（v2 M1/M2） ----------

export type DrillKind = 'interview' // paper/试卷已全链路退役

export interface InterviewerDossier {
  summary?: string
  question_ids?: number[]
  questions?: Array<{
    question_id?: number
    content: string
    ref_answer?: string | null
  }>
  skill_ids?: number[]
  skills?: Array<{
    skill_id?: number
    name: string
  }>
}

export interface DrillMessage {
  id: number
  drill_id: number
  role: 'ai' | 'user'
  kind: 'question' | 'probe' | 'answer' | 'score' | 'summary' | 'hint' | 'control' | 'start' | 'message'
  content: string
  score: number | null
  difficulty: number | null
  feedback: string | null
  meta?: { anchor_keyword: string; reason: string } | null
  intent?: 'main_question' | 'followup_probe' | 'turn_wrapup' | 'summary' | string | null
  created_at: string
}

export interface DrillView {
  id: number
  kind: DrillKind
  title: string
  position: string | null
  direction: string | null
  stages: string[] | null
  status: 'ongoing' | 'finished' | 'aborted'
  grading: string | null
  score: number | null
  started_at: string
  finished_at: string | null
  message_count: number
  question_count: number
  dossier?: InterviewerDossier | null
}

export interface InterviewerPersona {
  id: number
  name: string
  title?: string | null
  persona_prompt: string
  difficulty_hint?: string | null
  temperature_hint?: number | null
  focus_tags: string[]
  builtin: boolean
}

export interface DrillDetail {
  id: number
  kind: DrillKind
  title: string
  position: string | null
  direction: string | null
  stages: string[] | null
  status: 'ongoing' | 'finished' | 'aborted'
  grading: string | null
  score: number | null
  started_at: string
  finished_at: string | null
  message_count: number
  question_count: number
  dossier?: InterviewerDossier | null
  /** 人格展示名：经典模式 / 已删除的面试官 / 人格名（M5a） */
  persona_label?: string | null
  /** 面试官笔记（V6-M3 ADR-0023 D3）：手动触发生成后落库 */
  interview_state?: InterviewerNotes | null
  /** 进行中的面试官笔记任务（刷新恢复通道；空时省略） */
  ai_jobs?: { id: number; kind: 'interview_prep'; status: string; target_id: number; started_at: string }[]
  messages: DrillMessage[]
}

export interface InterviewerNotes {
  job_requirements: string[]
  candidate_facts: string[]
  risk_signals: string[]
  next_followups: string[]
  /** 任一段由规则提取兜底补齐 */
  rule_backfilled?: boolean
  sources?: {
    jd_excerpt?: string | null
    resume_excerpt?: string | null
    round_qas?: { question: string; answer_excerpt?: string | null }[]
  }
  generated_at?: string
}

export interface Resume {
  id: number
  name: string
  raw_text: string
  parsed: any | null
  is_active: boolean
  updated_at: string
}

// ---------- v3 积分经济（M3） ----------

export interface MallItem {
  id: number
  name: string
  cost: number
  emoji: string
  sort_order: number
}

export interface LedgerEntry {
  id: number
  amount: number
  category: string
  ref_type: string | null
  ref_id: number | null
  note: string | null
  created_at: string
}

export interface DailyProgress {
  due_today: number
  done_today: number
  queue_done: boolean
  cards_today: number
  drills_today: number
  goal_awarded: boolean
}

// ---------- v3 投递跟踪（M4） ----------

export type ApplicationStatus = 'applied' | 'interviewing' | 'offer' | 'rejected' | 'withdrawn'

/** 岗位（ADR-0012 一等实体）：公司下的 title/location/JD */
export interface Position {
  id: number
  company_id: number
  company?: string
  title: string
  department?: string | null
  location: string | null
  jd_text: string | null
  jd_interpret?: { overall?: string; cautions?: string[] } | null
  predict_result?: PositionPredictResponse | null
  ai_jobs?: { id: number; kind: string; target_id: number; status?: string }[]
  application_count?: number
  latest_status?: ApplicationStatus | null
  created_at?: string
}

export interface CompanySummary {
  id: number
  name: string
  description: string | null
  position_count: number
  application_count: number
  last_activity: string
}

export interface PositionApplication {
  id: number
  status: ApplicationStatus
  channel: string | null
  salary: string | null
  note: string | null
  applied_at: string
  round_count: number
  latest_round_passed: string | null
  interview_stages: { name: string; passed: string; date?: string | null }[] | null
}

export interface Application {
  id: number
  position_id: number
  company_id: number | null
  company: string | null
  department: string | null
  interview_stages: { name: string; passed: string; date?: string | null }[] | null
  /** 岗位标题（join positions） */
  position: string | null
  location: string | null
  salary: string | null
  channel: string | null
  applied_at: string
  status: ApplicationStatus
  note: string | null
  jd_interpret: { overall?: string; cautions?: string[]; content?: string } | null
  jd_match: any | null
  created_at: string
  updated_at: string
}

export const APP_STATUS: Record<ApplicationStatus, string> = {
  applied: '已投',
  interviewing: '进行中',
  offer: 'Offer',
  rejected: '未通过',
  withdrawn: '放弃',
}

export const APP_STATUS_COLOR: Record<ApplicationStatus, string> = {
  applied: '#5f6368',
  interviewing: '#1a73e8',
  offer: '#188038',
  rejected: '#d93025',
  withdrawn: '#8a8f93',
}

// ---------- v3 数据资产化（M2） ----------

export interface ScorePoint {
  date: string
  avg_score: number
  count: number
}

export interface CompanyScore {
  company: string
  avg_score: number | null
  count: number
}

export interface ScoreTrend {
  by_date: ScorePoint[]
  by_company: CompanyScore[]
}

export interface CurveDay {
  date: string
  remembered: number
  fuzzy: number
  forgot: number
}

export interface ReviewCurve {
  daily: CurveDay[]
  totals: { remembered: number; fuzzy: number; forgot: number }
  streak_days: number
}

export interface TimelineItem {
  date: string
  type: 'application' | 'session' | 'round' | 'drill' | 'review' | 'review_done' | 'point'
  title: string
  status?: string
  passed?: string
  result?: string
  company?: string | null
  channel?: string | null
  detail?: string
  kind?: string
  score?: number | null
  amount?: number | null
}

export interface Timeline {
  items: TimelineItem[]
}

export interface FunnelStage {
  stage: string
  count: number
}

export interface ConversionStep {
  from: string
  to: string
  rate: number
}

export interface ChannelEffect {
  channel: string
  count: number
  interviewed: number
  offers: number
  interview_rate: number
  offer_rate: number
}

export interface Funnel {
  funnel: FunnelStage[]
  conversion: ConversionStep[]
  channels: ChannelEffect[]
}

// ---------- v3 批量分析（M1） ----------

export interface BatchJob {
  id: number
  status: 'running' | 'done' | 'cancelled' | 'error'
  total: number
  done: number
  ok: number
  failed: number
  error: string | null
  started_at: string
}
