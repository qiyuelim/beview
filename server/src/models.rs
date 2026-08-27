use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;

// ---------- 请求体 ----------

#[derive(Deserialize)]
pub struct CreateCompanyReq {
    pub name: String,
}

#[derive(Deserialize)]
pub struct UpdateCompanyReq {
    pub name: String,
}

#[derive(Deserialize)]
pub struct CreateSessionReq {
    pub department: Option<String>,
    pub position: Option<String>,
    pub started_at: Option<NaiveDate>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateSessionReq {
    pub department: Option<String>,
    pub position: Option<String>,
    pub started_at: Option<NaiveDate>,
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateRoundReq {
    pub name: String,
    pub sort_order: Option<i32>,
    pub date: Option<NaiveDate>,
    pub passed: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateRoundReq {
    pub name: Option<String>,
    pub sort_order: Option<i32>,
    pub date: Option<NaiveDate>,
    pub passed: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct CreateQuestionReq {
    pub round_id: i64,
    pub content: String,
    pub my_answer: Option<String>,
    pub asked_at: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
    pub parent_id: Option<i64>,
    pub skill_id: Option<i64>,
    pub skill_ids: Option<Vec<i64>>,
    pub question_type: Option<String>,
    pub followups: Option<Vec<CreateFollowupReq>>,
}

#[derive(Deserialize, Clone)]
pub struct CreateFollowupReq {
    pub content: String,
    pub my_answer: Option<String>,
    pub tags: Option<Vec<String>>,
    pub skill_id: Option<i64>,
    pub skill_ids: Option<Vec<i64>>,
    pub question_type: Option<String>,
}

#[derive(Deserialize)]
pub struct BulkDeleteReq {
    pub ids: Vec<i64>,
}

#[derive(Deserialize)]
pub struct UpdateQuestionReq {
    pub round_id: Option<i64>,
    pub content: Option<String>,
    pub my_answer: Option<String>,
    pub starred: Option<bool>,
    pub asked_at: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
    pub skill_id: Option<i64>,
    pub skill_ids: Option<Vec<i64>>,
    pub question_type: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateCommentReq {
    pub body: String,
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct SetupReq {
    pub username: String,
    pub password: String,
}

// ---------- 查询参数 ----------

#[derive(Deserialize, Default)]
pub struct QuestionFilters {
    pub company: Option<i64>,
    pub session: Option<i64>,
    pub round: Option<i64>,
    pub tag: Option<String>,
    pub skill_id: Option<i64>,
    pub analyzed: Option<bool>,
    pub starred: Option<bool>,
    pub q: Option<String>,
    pub source: Option<String>, // manual | ai_drill | all（缺省 all）
    pub question_type: Option<String>,
    /// 票03：按 predicted_position_id 筛选押题题（与 source=predicted 组合使用）
    pub position_id: Option<i64>,
}

// ---------- 响应体 ----------

#[derive(FromRow, Serialize)]
pub struct CompanySummary {
    pub id: i64,
    pub name: String,
    pub session_count: i64,
    pub question_count: i64,
    pub avg_score: Option<f64>,
    pub last_interview: Option<NaiveDate>,
    /// 各批次（部门）进度明细：id/department/position/status/question_count/avg_score
    pub sessions: serde_json::Value,
}

#[derive(FromRow, Serialize)]
pub struct RoundView {
    pub id: i64,
    pub session_id: i64,
    pub name: String,
    pub sort_order: i32,
    pub date: Option<NaiveDate>,
    pub passed: String,
    pub created_at: DateTime<Utc>,
    pub question_count: i64,
    pub avg_score: Option<f64>,
}

#[derive(FromRow, Serialize, Clone)]
pub struct QuestionRow {
    pub id: i64,
    pub round_id: i64,
    pub parent_id: Option<i64>,
    pub content: String,
    pub my_answer: Option<String>,
    pub starred: bool,
    pub asked_at: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
    pub source: String,
    pub tags: Vec<String>,
    pub analyzed: bool,
    pub last_score: Option<i32>,
    pub last_difficulty: Option<i32>,
    /// 最近一条非空点评（回答级；追问气泡展示用，用户裁决 2a）
    #[serde(default)]
    pub last_feedback: Option<String>,
    pub company: Option<String>,
    #[serde(default)]
    pub followup_count: i64,
    pub skill_id: Option<i64>,
    pub skill_name: Option<String>,
    pub skill_path: Option<String>,
    pub question_type: Option<String>,
    pub difficulty: Option<i32>,
    /// 归属上下文（反馈七#5）：真实投递 = 公司/部门/岗位；系统容器题（陪练沉淀）company 走 session
    #[serde(default)]
    pub company_id: Option<i64>,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub position: Option<String>,
    #[serde(default)]
    pub container_dept: Option<String>,
    #[serde(default)]
    pub container_pos: Option<String>,
}

#[derive(Serialize)]
pub struct QuestionDetail {
    #[serde(flatten)]
    pub row: QuestionRow,
    pub followups: Vec<QuestionRow>,
    pub analyses: Vec<AnalysisRow>,
    pub comments: Vec<CommentRow>,
    pub answers: Vec<AnswerRow>,
    pub round_links: Vec<RoundLinkRow>,
    /// 该题当前 running 的 AI 任务（ADR-0013 D3：刷新后恢复「进行中」展示）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ai_jobs: Vec<crate::state::AiJob>,
    /// 疑似重复题（票02，归一化键相等；双向对称）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duplicates: Vec<crate::routes::questions::DuplicateHit>,
}

#[derive(FromRow, Serialize)]
pub struct AnswerRow {
    pub id: i64,
    pub question_id: i64,
    pub source: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(FromRow, Serialize)]
pub struct RoundLinkRow {
    pub round_id: i64,
    pub round_name: String,
    pub application_id: i64,
    pub department: Option<String>,
    pub position: Option<String>,
    pub company: Option<String>,
    pub date: Option<NaiveDate>,
    pub passed: String,
}

#[derive(FromRow, Serialize)]
pub struct AnalysisRow {
    pub id: i64,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tags: Option<Value>,
    pub difficulty: Option<i32>,
    pub ref_answer: Option<String>,
    pub score: Option<i32>,
    pub feedback: Option<String>,
    /// 本次分析所基于的用户回答快照（空=未填回答；用于按回答切换分析历史）
    pub answer_snapshot: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(FromRow, Serialize)]
pub struct CommentRow {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(FromRow, Serialize)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

// ---------- v2 复习（ADR-0007） ----------

#[derive(Deserialize)]
pub struct GradeReq {
    pub result: String, // remembered | fuzzy | forgot
    /// 复习时主动回忆的内容（可选）：记入回答历史（source=review），不改写 my_answer
    pub answer: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ExplainReq {
    pub focus: Option<String>,
}

#[derive(FromRow, Serialize)]
pub struct ReviewQueueItem {
    pub question_id: i64,
    pub content: String,
    pub my_answer: Option<String>,
    pub source: String,
    pub difficulty: Option<i32>,
    pub score: Option<i32>,
    pub ref_answer: Option<String>,
    pub feedback: Option<String>,
    pub tags: Vec<String>,
    pub company: Option<String>,
    pub last_result: Option<String>,
    pub interval_days: i32,
    pub next_review_at: DateTime<Utc>,
}

#[derive(FromRow, Serialize)]
pub struct WrongItem {
    pub question_id: i64,
    pub content: String,
    pub my_answer: Option<String>,
    pub source: String,
    pub last_result: Option<String>,
    pub review_count: i32,
    pub score: Option<i32>,
    pub ref_answer: Option<String>,
    pub tags: Vec<String>,
    pub company: Option<String>,
}

#[derive(Serialize)]
pub struct ReviewStats {
    pub due: i64,
    pub done_today: i64,
    pub remembered: i64,
    pub fuzzy: i64,
    pub forgot: i64,
    pub streak_days: i64,
}

#[derive(Serialize)]
pub struct GradeResult {
    pub last_result: String,
    pub ease: f64,
    pub interval_days: i32,
    pub next_review_at: DateTime<Utc>,
    pub review_count: i32,
}

// ---------- v2 训练引擎（ADR-0008） ----------

#[derive(Deserialize)]
pub struct CreateDrillReq {
    pub kind: String, // interview | paper（resume_grill 已退役：ADR-0023 D4）
    pub title: Option<String>,
    pub position: Option<String>,
    pub direction: Option<String>,
    pub stages: Option<Vec<String>>,
    pub target_questions: Option<i32>,
    pub references: Option<String>, // 参考内容（岗位要求/面经/参考题），AI 出题参考
    pub application_id: Option<i64>, // 关联投递：陪练以该投递的 JD 为纲（v4 M4）
    pub dossier: Option<Value>,     // v5 M3 考官题本（题库题/押题/薄弱技能）
    /// 场次级人格（M5a ADR-0023 D1）：None = 经典模式
    pub persona_id: Option<i64>,
}

#[derive(Deserialize)]
pub struct SendMessageReq {
    pub content: String,
    pub action: Option<String>,
    pub hint_level: Option<i32>,
}

#[derive(FromRow, Serialize)]
pub struct DrillView {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub position: Option<String>,
    pub direction: Option<String>,
    pub stages: Option<Value>,
    pub status: String,
    pub grading: Option<String>,
    pub score: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub message_count: i64,
    pub question_count: i64,
    pub dossier: Option<Value>,
    /// 面试官笔记（V6-M3 ADR-0023 D3）：四段预读 + sources 原始引用；手动触发生成
    pub interview_state: Option<Value>,
    /// 人格展示名：经典模式 / 已删除的面试官 / 人格名（M5a）
    pub persona_label: Option<String>,
}

#[derive(Clone, FromRow, Serialize)]
pub struct DrillMessage {
    pub id: i64,
    pub drill_id: i64,
    pub role: String,
    pub kind: String,
    pub content: String,
    pub score: Option<i32>,
    pub difficulty: Option<i32>,
    pub feedback: Option<String>,
    pub intent: Option<String>,
    /// 追问元数据（V6-M4 ADR-0023 D2）：anchor_keyword + 封闭理由枚举
    pub meta: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct DrillDetail {
    #[serde(flatten)]
    pub view: DrillView,
    pub messages: Vec<DrillMessage>,
    /// 进行中的面试官笔记生成任务（刷新恢复通道，ADR-0013 D3 同款；空时省略）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ai_jobs: Vec<crate::state::AiJob>,
}

// ---------- v2/v4 简历（ADR-0006 + ADR-0019） ----------

#[derive(Deserialize)]
pub struct SaveResumeReq {
    pub raw_text: String,
    pub name: Option<String>,
    pub version_name: Option<String>,
    /// v3 M0：可视化编辑后的结构化字段（可随原文一起保存，不重解析）
    pub parsed: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct CreateSnapshotReq {
    pub version_name: Option<String>,
}

#[derive(FromRow, Serialize, Clone)]
pub struct ResumeView {
    pub id: i64,
    pub name: String,
    pub version_name: String,
    pub is_archived: bool,
    pub raw_text: String,
    pub parsed: Option<Value>,
    pub is_active: bool,
    pub updated_at: DateTime<Utc>,
    /// 该简历当前 running 的解析任务（ADR-0013 D3；空时不序列化，读库时 sqlx skip）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[sqlx(skip)]
    pub ai_jobs: Vec<crate::state::AiJob>,
}

#[derive(FromRow, Serialize, Deserialize, Clone)]
pub struct ResumeListItem {
    pub id: i64,
    pub name: String,
    pub version_name: String,
    pub is_archived: bool,
    pub is_active: bool,
    pub char_count: i64,
    pub has_parsed: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
