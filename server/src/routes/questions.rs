use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::CurrentUser;
use crate::contracts;
use crate::error::AppError;
use crate::models::{
    AnalysisRow, BulkDeleteReq, CommentRow, CreateFollowupReq, CreateQuestionReq, QuestionDetail, QuestionFilters,
    QuestionRow, UpdateQuestionReq,
};
use crate::settings;
use crate::state::{AiJob, AiStart, AppState};

use tracing::Instrument;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tags", get(list_tags))
        .route("/questions", get(list_questions).post(create_question).delete(bulk_delete_questions))
        .route("/questions/self", post(create_self_question))
        .route(
            "/questions/{id}",
            get(get_question).patch(update_question).delete(delete_question),
        )
        .route("/questions/{id}/followups", post(create_question_followup))
        .route("/questions/{id}/analyses", get(list_analyses))
        .route("/questions/{id}/analyze", post(analyze_question))
        .route("/questions/{id}/ref", post(ref_question).put(update_ref))
        .route("/questions/{id}/answers", post(record_answer_route))
        .route("/questions/{id}/related", get(related_questions))
        .route("/questions/{id}/round-links", post(add_round_link))
        .route("/questions/{id}/round-links/{rid}", axum::routing::delete(remove_round_link))
}

const QUESTION_SELECT: &str = r#"
    SELECT q.id, q.round_id, q.parent_id, q.content, q.my_answer, q.starred, q.asked_at, q.created_at, q.source,
           COALESCE(array_agg(t.name) FILTER (WHERE t.name IS NOT NULL), '{}') AS tags,
           EXISTS(SELECT 1 FROM analyses a WHERE a.question_id=q.id) AS analyzed,
           -- 稳定语义：score 只看评分（回答级）、difficulty 只看固有属性（题目级），互不干扰、都不闪
           (SELECT a.score FROM analyses a WHERE a.question_id=q.id AND a.score IS NOT NULL ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS last_score,
           (SELECT a.difficulty FROM analyses a WHERE a.question_id=q.id AND a.difficulty IS NOT NULL ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS last_difficulty,
           (SELECT a.feedback FROM analyses a WHERE a.question_id=q.id AND a.feedback IS NOT NULL AND trim(a.feedback)<>'' ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS last_feedback,
           (SELECT count(*) FROM questions f WHERE f.parent_id=q.id) AS followup_count,
           q.skill_id,
           (SELECT s.name FROM skills s WHERE s.id=q.skill_id) AS skill_name,
           (SELECT s.path FROM skills s WHERE s.id=q.skill_id) AS skill_path,
           q.question_type,
           q.difficulty,
           -- 归属双路径（反馈七#4）：真实投递走 application→position→company；
           -- 陪练沉淀等系统容器走 session→company（application_id 为 NULL）
           (SELECT c.name FROM companies c JOIN positions p ON p.company_id=c.id JOIN applications a ON a.position_id=p.id JOIN rounds r ON r.application_id=a.id WHERE r.id=q.round_id) AS company,
           (SELECT c.id FROM companies c JOIN positions p ON p.company_id=c.id JOIN applications a ON a.position_id=p.id JOIN rounds r ON r.application_id=a.id WHERE r.id=q.round_id) AS company_id,
           (SELECT p.department FROM positions p JOIN applications a ON a.position_id=p.id JOIN rounds r ON r.application_id=a.id WHERE r.id=q.round_id) AS department,
           (SELECT p.title FROM positions p JOIN applications a ON a.position_id=p.id JOIN rounds r ON r.application_id=a.id WHERE r.id=q.round_id) AS position,
           (SELECT s.department FROM sessions s JOIN rounds r ON r.session_id=s.id WHERE r.id=q.round_id AND r.application_id IS NULL) AS container_dept,
           (SELECT s.position FROM sessions s JOIN rounds r ON r.session_id=s.id WHERE r.id=q.round_id AND r.application_id IS NULL) AS container_pos
    FROM questions q
    LEFT JOIN question_tags qt ON qt.question_id=q.id
    LEFT JOIN tags t ON t.id=qt.tag_id
"#;

#[tracing::instrument(skip_all)]
async fn list_questions(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Query(f): Query<QuestionFilters>,
) -> Result<Json<Vec<QuestionRow>>, AppError> {
    let subtree_tag_clause = crate::services::skill_query::subtree_condition_sql("q", "$9", "NULL", "$6");
    let subtree_skill_clause = crate::services::skill_query::subtree_condition_sql("q", "$9", "$10", "NULL");
    let sql = format!(
        r#"{QUESTION_SELECT}
        WHERE q.user_id = $9
          AND q.parent_id IS NULL
          AND ($1::text IS NULL OR q.content ILIKE '%'||$1||'%' OR q.my_answer ILIKE '%'||$1||'%')
          AND ($2::bool IS NULL OR q.starred = $2)
          AND ($3::bigint IS NULL OR q.round_id = $3 OR EXISTS(SELECT 1 FROM question_rounds qr WHERE qr.question_id=q.id AND qr.round_id=$3))
          AND ($4::bigint IS NULL OR EXISTS(SELECT 1 FROM rounds r WHERE r.id=q.round_id AND r.session_id=$4))
          AND ($5::bigint IS NULL OR EXISTS(
                 SELECT 1 FROM rounds r2
                 LEFT JOIN applications a2 ON a2.id=r2.application_id
                 LEFT JOIN positions p2 ON p2.id=a2.position_id
                 LEFT JOIN companies c2a ON c2a.id=p2.company_id
                 LEFT JOIN sessions s2 ON s2.id=r2.session_id AND r2.application_id IS NULL
                 LEFT JOIN companies c2b ON c2b.id=s2.company_id
                 WHERE r2.id=q.round_id AND (c2a.id=$5 OR c2b.id=$5)))
          AND ($6::text IS NULL OR {subtree_tag_clause})
          AND ($7::bool IS NULL OR EXISTS(SELECT 1 FROM analyses a WHERE a.question_id=q.id) = $7)
          AND ($8::text IS NULL OR q.source = $8)
          AND ($10::bigint IS NULL OR {subtree_skill_clause})
          AND ($11::text IS NULL OR q.question_type = $11)
          AND ($12::bigint IS NULL OR q.predicted_position_id = $12)
        GROUP BY q.id
        ORDER BY q.created_at DESC, q.id DESC
        "#
    );
    let rows = sqlx::query_as::<_, QuestionRow>(&sql)
        .bind(f.q)
        .bind(f.starred)
        .bind(f.round)
        .bind(f.session)
        .bind(f.company)
        .bind(f.tag)
        .bind(f.analyzed)
        .bind(f.source)
        .bind(user.0)
        .bind(f.skill_id)
        .bind(f.question_type)
        .bind(f.position_id)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(rows))
}

#[tracing::instrument(skip_all)]
async fn list_tags(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<String>>, AppError> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT t.name FROM tags t JOIN question_tags qt ON qt.tag_id=t.id JOIN questions q ON q.id=qt.question_id WHERE q.user_id=$1 ORDER BY t.name"
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// 疑似重复题命中（票02）：归一化键相等即命中；双向可见由读侧对称计算保证。
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct DuplicateHit {
    pub id: i64,
    pub content: String,
}

/// 归一化键相等的同用户题目（排除自身，最多 3 条，id 升序稳定输出）。
async fn find_duplicates(
    pool: &sqlx::PgPool,
    uid: i64,
    content: &str,
    exclude_id: Option<i64>,
) -> Result<Vec<DuplicateHit>, AppError> {
    let hits: Vec<DuplicateHit> = sqlx::query_as(
        r#"
        SELECT id, content FROM questions
        WHERE user_id=$1 AND parent_id IS NULL AND content_normalized = normalize_question_content($2)
          AND ($3::bigint IS NULL OR id <> $3)
        ORDER BY id LIMIT 3
        "#,
    )
    .bind(uid)
    .bind(content)
    .bind(exclude_id)
    .fetch_all(pool)
    .await?;
    Ok(hits)
}

#[tracing::instrument(skip_all)]
async fn create_question(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateQuestionReq>,
) -> Result<impl IntoResponse, AppError> {
    // 票02：录入提示不阻塞创建——命中仅作为响应附带信息，由前端决定展示
    let duplicates = find_duplicates(&state.pool, user.0, req.content.trim(), None).await?;
    let id = create_question_row(&state.pool, &state.event_bus, user.0, &req).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "duplicates": duplicates })),
    ))
}

use crate::services::skill_service::sync_question_skills;

/// 题目创建核心（归属校验/落库/标签/技能关联/复习入队/回答版本/关联轮次/积分）——两个入口共用
async fn create_question_row(
    pool: &sqlx::PgPool,
    event_bus: &crate::events::EventBus,
    uid: i64,
    req: &CreateQuestionReq,
) -> Result<i64, AppError> {
    let content = req.content.trim();
    if content.is_empty() {
        return Err(AppError::BadRequest("题目内容不能为空".to_string()));
    }
    let round_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rounds r JOIN applications a ON a.id=r.application_id WHERE r.id=$1 AND a.user_id=$2)",
    )
    .bind(req.round_id)
    .bind(uid)
    .fetch_one(pool)
    .await?;
    if !round_exists {
        return Err(AppError::BadRequest("轮次不存在".to_string()));
    }
    let q_type = req.question_type.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("professional_knowledge");
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO questions(user_id, round_id, parent_id, content, content_normalized, my_answer, asked_at, skill_id, question_type) VALUES($1,$2,$3,$4, normalize_question_content($4), $5,$6,$7,$8) RETURNING id",
    )
    .bind(uid)
    .bind(req.round_id)
    .bind(req.parent_id)
    .bind(content)
    .bind(&req.my_answer)
    .bind(req.asked_at)
    .bind(req.skill_id)
    .bind(q_type)
    .fetch_one(pool)
    .await?;
    if let Some(tags) = &req.tags {
        attach_tags(pool, uid, id, &tags).await?;
    }
    sync_question_skills(pool, id, req.skill_id, req.skill_ids.as_deref()).await?;

    if let Some(a) = req.my_answer.as_deref() {
        if !a.trim().is_empty() {
            enqueue_review(pool, id).await?;
            record_answer(pool, id, "manual", a).await?;
        }
    }
    // 题↔轮次：主归属 + 关联表（多面试关联）
    link_round(pool, id, req.round_id).await?;

    // 录入时一并提交的一级子追问处理
    if let Some(followups) = &req.followups {
        for f in followups {
            let f_content = f.content.trim();
            if f_content.is_empty() {
                continue;
            }
            let f_type = f.question_type.as_deref().or(req.question_type.as_deref()).filter(|s| !s.trim().is_empty()).unwrap_or("professional_knowledge");
            let fid: i64 = sqlx::query_scalar(
                "INSERT INTO questions(user_id, round_id, parent_id, content, content_normalized, my_answer, asked_at, skill_id, question_type) VALUES($1,$2,$3,$4, normalize_question_content($4), $5,$6,$7,$8) RETURNING id",
            )
            .bind(uid)
            .bind(req.round_id)
            .bind(id)
            .bind(f_content)
            .bind(&f.my_answer)
            .bind(req.asked_at)
            .bind(f.skill_id.or(req.skill_id))
            .bind(f_type)
            .fetch_one(pool)
            .await?;
            if let Some(ftags) = &f.tags {
                attach_tags(pool, uid, fid, ftags).await?;
            }
            sync_question_skills(
                pool,
                fid,
                f.skill_id.or(req.skill_id),
                f.skill_ids.as_deref().or(req.skill_ids.as_deref()),
            ).await?;

            if let Some(fa) = f.my_answer.as_deref() {
                if !fa.trim().is_empty() {
                    enqueue_review(pool, fid).await?;
                    record_answer(pool, fid, "manual", fa).await?;
                }
            }
            // 恢复追问关联轮次行（N3 修复）
            link_round(pool, fid, req.round_id).await?;
        }
    }

    // 仅主题目派发创建事件（追问不增加主题目计数与重复发分）；副作用失败不回滚建题
    if req.parent_id.is_none() {
        if let Err(e) = event_bus.dispatch(crate::events::DomainEvent::RealQuestionCreated {
            user_id: uid,
            question_id: id,
            round_id: req.round_id,
        }).await {
            tracing::error!(error = %e, question_id = id, "真实题积分发放失败（题目已创建）");
        }
    }
    tracing::info!(
        event = "question.created",
        user_id = uid,
        question_id = id,
        round_id = req.round_id,
        "question created successfully"
    );
    Ok(id)
}

#[derive(Deserialize)]
struct CreateFollowupInlineReq {
    pub content: String,
    pub my_answer: Option<String>,
    pub tags: Option<Vec<String>>,
    pub skill_id: Option<i64>,
    pub skill_ids: Option<Vec<i64>>,
    pub question_type: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn create_question_followup(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<CreateFollowupInlineReq>,
) -> Result<impl IntoResponse, AppError> {
    let parent: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT round_id, parent_id FROM questions WHERE id=$1 AND user_id=$2"
    )
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;

    let Some(parent) = parent else {
        return Err(AppError::NotFound);
    };

    let target_parent_id = parent.1.unwrap_or(id);
    let inner = CreateQuestionReq {
        round_id: parent.0,
        content: req.content,
        my_answer: req.my_answer,
        asked_at: None,
        tags: req.tags,
        parent_id: Some(target_parent_id),
        skill_id: req.skill_id,
        skill_ids: req.skill_ids,
        question_type: req.question_type,
        followups: None,
    };
    let fid = create_question_row(&state.pool, &state.event_bus, user.0, &inner).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": fid, "parent_id": target_parent_id }))))
}

#[derive(serde::Deserialize)]
struct CreateSelfQuestionReq {
    pub content: String,
    pub my_answer: Option<String>,
    pub asked_at: Option<chrono::NaiveDate>,
    pub tags: Option<Vec<String>>,
    pub skill_id: Option<i64>,
    pub skill_ids: Option<Vec<i64>>,
    pub question_type: Option<String>,
    pub followups: Option<Vec<CreateFollowupReq>>,
}

#[tracing::instrument(skip_all)]
async fn create_self_question(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateSelfQuestionReq>,
) -> Result<impl IntoResponse, AppError> {
    let round_id = crate::services::system_containers::ensure_self_round(&state.pool, user.0).await?;
    let inner = CreateQuestionReq {
        round_id,
        content: req.content,
        my_answer: req.my_answer,
        asked_at: req.asked_at,
        tags: req.tags,
        parent_id: None,
        skill_id: req.skill_id,
        skill_ids: req.skill_ids,
        question_type: req.question_type,
        followups: req.followups,
    };
    let duplicates = find_duplicates(&state.pool, user.0, inner.content.trim(), None).await?;
    let id = create_question_row(&state.pool, &state.event_bus, user.0, &inner).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "duplicates": duplicates })),
    ))
}

#[tracing::instrument(skip_all)]
async fn bulk_delete_questions(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<BulkDeleteReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.ids.is_empty() {
        return Err(AppError::BadRequest("未选择任何题目".to_string()));
    }
    let result = sqlx::query("DELETE FROM questions WHERE user_id=$2 AND id = ANY($1)")
        .bind(&req.ids)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    tracing::info!(user_id = user.0, count = req.ids.len(), deleted = result.rows_affected(), "批量删除题目成功");
    Ok(Json(json!({ "deleted": result.rows_affected() })))
}

/// 获取题目详情
#[tracing::instrument(skip_all)]
async fn get_question(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<QuestionDetail>, AppError> {
    let row = fetch_question_row(&state.pool, user.0, id).await?;
    let followups = fetch_followups(&state.pool, user.0, id).await?;
    let analyses = fetch_analyses(&state.pool, user.0, id).await?;
    let comments: Vec<CommentRow> = sqlx::query_as(
        "SELECT id, body, created_at FROM comments WHERE question_id=$1 AND user_id=$2 ORDER BY created_at DESC, id DESC",
    )
    .bind(id)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    let answers = fetch_answers(&state.pool, user.0, id).await?;
    let round_links = fetch_round_links(&state.pool, user.0, id).await?;
    // 票02：双向徽章——读侧对称计算，任一侧进入详情都能看到对方
    let duplicates = find_duplicates(&state.pool, user.0, &row.content, Some(id)).await?;
    let ai_jobs: Vec<crate::state::AiJob> = ["ref", "analyze"]
        .iter()
        .filter_map(|k| state.ai_jobs.running_for(user.0, k, id))
        .collect();
    Ok(Json(QuestionDetail {
        row,
        followups,
        analyses,
        comments,
        answers,
        round_links,
        ai_jobs,
        duplicates,
    }))
}

/// 更新题目信息
/// 技能归属更新契约（N4 规范）：
/// `skill_ids` 与 `skill_id` 均采用声明式全量替换语义；传入空数组表示清空解绑，首个元素自动同步为主技能 `questions.skill_id`。
#[tracing::instrument(skip_all)]
async fn update_question(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateQuestionReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(round_id) = req.round_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rounds r JOIN applications a ON a.id=r.application_id WHERE r.id=$1 AND a.user_id=$2)",
        )
        .bind(round_id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
        if !exists {
            return Err(AppError::BadRequest("轮次不存在".to_string()));
        }
    }
    // 票02：内容变更时同步刷新归一化键（COALESCE 语义与 content 列一致：未传则保留原值）
    let trimmed_content = req.content.as_deref().map(|s| s.trim().to_string());
    let updated = sqlx::query(
        "UPDATE questions SET round_id=COALESCE($2, round_id), content=COALESCE($3, content),
         content_normalized=COALESCE(normalize_question_content($3), content_normalized),
         my_answer=COALESCE($4, my_answer), starred=COALESCE($5, starred), asked_at=COALESCE($6, asked_at),
         question_type=COALESCE($8, question_type)
         WHERE id=$1 AND user_id=$7",
    )
    .bind(id)
    .bind(req.round_id)
    .bind(trimmed_content.clone())
    .bind(&req.my_answer)
    .bind(req.starred)
    .bind(req.asked_at)
    .bind(user.0)
    .bind(req.question_type)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    if let Some(rid) = req.round_id {
        link_round(&state.pool, id, rid).await?;
    }
    if let Some(tags) = req.tags {
        sqlx::query("DELETE FROM question_tags WHERE question_id=$1")
            .bind(id)
            .execute(&state.pool)
            .await?;
        attach_tags(&state.pool, user.0, id, &tags).await?;
    }
    if req.skill_ids.is_some() || req.skill_id.is_some() {
        sync_question_skills(&state.pool, id, req.skill_id, req.skill_ids.as_deref()).await?;
    }
    if let Some(a) = req.my_answer.as_deref() {
        if !a.trim().is_empty() {
            enqueue_review(&state.pool, id).await?;
            record_answer(&state.pool, id, "manual", a).await?;
        }
    }
    // 票02：编辑后若与其它题归一化等价，响应附带提示（前端详情页据此刷新徽章）
    let duplicates = match trimmed_content.as_deref() {
        Some(c) => find_duplicates(&state.pool, user.0, c.trim(), Some(id)).await?,
        None => Vec::new(),
    };
    tracing::info!(user_id = user.0, question_id = id, "题目更新成功");
    Ok(Json(json!({ "ok": true, "duplicates": duplicates })))
}

#[tracing::instrument(skip_all)]
async fn delete_question(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let deleted = sqlx::query("DELETE FROM questions WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    tracing::info!(user_id = user.0, question_id = id, "题目删除成功");
    Ok(Json(json!({ "ok": true })))
}

#[tracing::instrument(skip_all)]
async fn list_analyses(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<AnalysisRow>>, AppError> {
    let rows = fetch_analyses(&state.pool, user.0, id).await?;
    Ok(Json(rows))
}

/// 任务受理响应（ADR-0013 D2）：已受理/已在跑统一回 {job_id, status}，结果经事件流/轮询回显
fn job_accepted(j: &AiJob) -> Value {
    json!({ "job_id": j.id, "status": j.status })
}

/// 评价回答（回答级，ADR-0013 D2 任务化）：同步校验保留（无回答/未配置 LLM 立即 400），
/// LLM 部分入后台任务——同题幂等去重，完成广播事件并落库 analyses。
#[tracing::instrument(skip_all)]
async fn analyze_question(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let q: (String, Option<String>) =
        sqlx::query_as("SELECT content, my_answer FROM questions WHERE id=$1 AND user_id=$2")
            .bind(id)
            .bind(user.0)
            .fetch_one(&state.pool)
            .await?;
    let my_answer = q.1.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("").to_string();
    if my_answer.is_empty() {
        return Err(AppError::BadRequest("请先填写「我的回答」再评价".to_string()));
    }
    let config = settings::require_llm(&state.pool, user.0).await?;
    let job = match state.ai_jobs.start(user.0, "analyze", id) {
        AiStart::AlreadyRunning(j) => return Ok(Json(job_accepted(&j))),
        AiStart::Started(j) => j,
    };
    tracing::info!(user_id = user.0, question_id = id, job_id = job.id, "发起题目 LLM 评价任务");
    let st = state.clone();
    let uid = user.0;
    // panic 守卫统一收尾（评审 P0）：正常失败/panic 均释放 running 条目，不阻塞同键重试
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        // 主问与追问整体合并评价（v5.3 统一口径）：若有追问与其现场作答，拼装完整对话进行整体评分与点评
        let children: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT q.content, q.my_answer FROM questions q \
             WHERE q.parent_id=$1 AND q.user_id=$2 \
             ORDER BY q.id ASC",
        )
        .bind(id)
        .bind(uid)
        .fetch_all(&st.pool)
        .await?;

        let eval_answer = if children.iter().any(|(_, a)| a.as_deref().map_or(false, |s| !s.trim().is_empty())) {
            let mut buf = format!("【主问题回答】\n{}\n", my_answer);
            for (i, (c, a)) in children.iter().enumerate() {
                if let Some(ans) = a.as_deref().filter(|s| !s.trim().is_empty()) {
                    buf.push_str(&format!("\n【追问 {}】：{}\n【追问回答】：{}\n", i + 1, c, ans));
                }
            }
            buf
        } else {
            my_answer.clone()
        };

        let row = run_analysis_ext(&st.pool, uid, id, &q.0, Some(&my_answer), Some(&eval_answer), &config).await?;
        // 每次评价自动把当时的 my_answer 落成版本（回答切换列表完整、批注有主可挂）
        record_answer(&st.pool, id, "manual", &my_answer).await?;
        // v5 事件总线：派发单题手动分析完成事件；积分失败不影响已完成的分析结果
        if let Err(e) = st.event_bus.dispatch(crate::events::DomainEvent::ManualAnalysisDone {
            user_id: uid,
            question_id: id,
        }).await {
            tracing::error!(error = %e, question_id = id, "手动分析积分发放失败（分析结果已落库）");
        }
        drop(row);
        Ok::<_, AppError>(())
    });
    Ok(Json(job_accepted(&job)))
}

/// 生成/刷新参考答案（题目级，ADR-0013 D2 任务化）：tags/难度/参考答案，不评分
#[tracing::instrument(skip_all)]
async fn ref_question(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let content: String = sqlx::query_scalar("SELECT content FROM questions WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    let config = settings::require_llm(&state.pool, user.0).await?;
    let job = match state.ai_jobs.start(user.0, "ref", id) {
        AiStart::AlreadyRunning(j) => return Ok(Json(job_accepted(&j))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        run_ref_analysis(&st.pool, uid, id, &content, &config).await.map(|_| ())
    });
    Ok(Json(job_accepted(&job)))
}

#[derive(Deserialize)]
struct UpdateRefReq {
    ref_answer: String,
}

/// 手动修改参考答案（题目固有属性）：LLM 生成的参考不佳/不详细时兜底，就地改最近一条固有属性行（难度/标签不动）
#[tracing::instrument(skip_all)]
async fn update_ref(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRefReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ref_answer = req.ref_answer.trim().to_string();
    if ref_answer.is_empty() {
        return Err(AppError::BadRequest("参考答案不能为空".to_string()));
    }
    let updated = sqlx::query(
        "UPDATE analyses SET ref_answer=$2
         WHERE id = (SELECT a.id FROM analyses a JOIN questions q ON q.id=a.question_id
                     WHERE a.question_id=$1 AND q.user_id=$3 AND a.ref_answer IS NOT NULL AND trim(a.ref_answer)<>''
                     ORDER BY a.created_at DESC, a.id DESC LIMIT 1)",
    )
    .bind(id)
    .bind(&ref_answer)
    .bind(user.0)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::BadRequest("还没有参考答案可修改，请先「分析题目」".to_string()));
    }
    Ok(Json(json!({ "ok": true })))
}

/// 推荐关联题目（离线，无 LLM）：与本题共享标签、评分最低的 5 条，点击可跳转。
/// 排序：有评分在前、评分升序（越弱越值得复习），再按共享标签数降序。
#[tracing::instrument(skip_all)]
async fn related_questions(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Value>>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT q.id, q.content,
               (SELECT a.score FROM analyses a WHERE a.question_id=q.id AND a.score IS NOT NULL
                ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS last_score,
               (SELECT count(DISTINCT t.name) FROM question_tags qt JOIN tags t ON t.id=qt.tag_id
                WHERE qt.question_id=q.id AND t.name IN (
                    SELECT t2.name FROM question_tags qt2 JOIN tags t2 ON t2.id=qt2.tag_id WHERE qt2.question_id=$1
                )) AS shared
        FROM questions q
        WHERE q.user_id = $2
          AND q.id <> $1
          AND EXISTS(SELECT 1 FROM question_tags qt3 JOIN tags t3 ON t3.id=qt3.tag_id
                     WHERE qt3.question_id=q.id AND t3.name IN (
                         SELECT t4.name FROM question_tags qt4 JOIN tags t4 ON t4.id=qt4.tag_id WHERE qt4.question_id=$1
                     ))
        ORDER BY last_score ASC NULLS LAST, shared DESC
        LIMIT 5
        "#,
    )
    .bind(id)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    use sqlx::Row;
    let out = rows
        .iter()
        .map(|r| {
            let qid: i64 = r.get("id");
            let content: String = r.get("content");
            let last_score: Option<i32> = r.get("last_score");
            let shared: i64 = r.get("shared");
            json!({"id": qid, "content": content, "last_score": last_score, "shared": shared})
        })
        .collect::<Vec<_>>();
    Ok(Json(out))
}

/// 生成/刷新参考答案（题目级）：契约 question_ref → 写 analyses（score 为 null）+ 标签
#[tracing::instrument(skip_all)]
pub async fn run_ref_analysis(
    pool: &sqlx::PgPool,
    uid: i64,
    question_id: i64,
    content: &str,
    config: &settings::LlmConfig,
) -> Result<AnalysisRow, AppError> {
    let provider = settings::provider_of(&config.base_url);
    let span = tracing::info_span!("llm.ref", model = %config.model, provider = %provider, question_id = question_id);
    let start = std::time::Instant::now();
    // 契约层（ADR-0017 D2）：prompt/schema/解析/文本降级全部内聚在 QuestionRef
    let (out, meta) = contracts::execute(config, pool, uid, &crate::contracts::question::QuestionRef::new(content))
        .instrument(span)
        .await?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let (a_tags, a_difficulty, a_ref_answer) = match out {
        contracts::ContractOut::Structured(o) => (o.tags, o.difficulty, o.ref_answer),
        contracts::ContractOut::Text(t) => (vec![], None, t), // 文本评审全文落 ref_answer（ir_mode 在 meta）
    };
    let tags_json = serde_json::to_value(&a_tags)?;
    let aid: i64 = sqlx::query_scalar(
        "INSERT INTO analyses(question_id, provider, model, tags, difficulty, ref_answer, score, feedback, raw, answer_snapshot)
         VALUES($1,$2,$3,$4,$5,$6,NULL,NULL,$7,NULL) RETURNING id",
    )
    .bind(question_id)
    .bind(&provider)
    .bind(&config.model)
    .bind(&tags_json)
    .bind(a_difficulty)
    .bind(&a_ref_answer)
    .bind(&meta)
    .fetch_one(pool)
    .await?;
    attach_tags(pool, uid, question_id, &a_tags).await?;
    tracing::info!(question_id, duration_ms, "ref analysis done");
    fetch_analysis_row(pool, aid).await
}

/// 评价回答（回答级）：契约 answer_evaluate → 写 analyses（score/feedback，snapshot=当前回答）
#[tracing::instrument(skip_all)]
pub async fn run_answer_analysis(
    pool: &sqlx::PgPool,
    uid: i64,
    question_id: i64,
    content: &str,
    my_answer: &str,
    config: &settings::LlmConfig,
) -> Result<AnalysisRow, AppError> {
    // 参考答案复用：取最近非空参考答案供 prompt 对照（Q2：参考答案不重新生成）
    let existing_ref: Option<String> = sqlx::query_scalar(
        "SELECT ref_answer FROM analyses WHERE question_id=$1 AND ref_answer IS NOT NULL AND trim(ref_answer)<>'' ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(question_id)
    .fetch_optional(pool)
    .await?;
    let provider = settings::provider_of(&config.base_url);
    let span = tracing::info_span!("llm.answer", model = %config.model, provider = %provider, question_id = question_id);
    let start = std::time::Instant::now();
    let contract = crate::contracts::question::AnswerEvaluate::new(content, my_answer, existing_ref.as_deref());
    let (out, meta) = contracts::execute(config, pool, uid, &contract).instrument(span).await?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let (a_score, a_feedback) = match out {
        contracts::ContractOut::Structured(o) => (o.score, o.feedback),
        contracts::ContractOut::Text(t) => (None, t),
    };
    let aid: i64 = sqlx::query_scalar(
        "INSERT INTO analyses(question_id, provider, model, tags, difficulty, ref_answer, score, feedback, raw, answer_snapshot)
         VALUES($1,$2,$3,NULL,NULL,NULL,$4,$5,$6,$7) RETURNING id",
    )
    .bind(question_id)
    .bind(&provider)
    .bind(&config.model)
    .bind(a_score)
    .bind(&a_feedback)
    .bind(&meta)
    .bind(my_answer)
    .fetch_one(pool)
    .await?;
    tracing::info!(question_id, duration_ms, "answer analysis done");
    fetch_analysis_row(pool, aid).await
}

/// 共享分析管线：LLM 调用（带指标）→ 写 analyses + 标签 → 自动入复习队。
/// 训练即时判分（drills）与批量分析（batch）共用（全量：参考答案 + 评分）。
#[tracing::instrument(skip_all)]
pub async fn run_analysis(
    pool: &sqlx::PgPool,
    uid: i64,
    question_id: i64,
    content: &str,
    my_answer: Option<&str>,
    config: &settings::LlmConfig,
) -> Result<AnalysisRow, AppError> {
    run_analysis_ext(pool, uid, question_id, content, my_answer, my_answer, config).await
}

pub async fn run_analysis_ext(
    pool: &sqlx::PgPool,
    uid: i64,
    question_id: i64,
    content: &str,
    answer_snapshot: Option<&str>,
    eval_answer: Option<&str>,
    config: &settings::LlmConfig,
) -> Result<AnalysisRow, AppError> {
    let provider = settings::provider_of(&config.base_url);

    // 参考答案复用：已分析过则取最近的非空参考答案，重新分析时保持其基本不变（只重新评分）
    let existing_ref: Option<String> = sqlx::query_scalar(
        "SELECT ref_answer FROM analyses WHERE question_id=$1 AND ref_answer IS NOT NULL AND trim(ref_answer)<>'' ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(question_id)
    .fetch_optional(pool)
    .await?;

    let system = match "" { _ => String::new() }; // 占位避免无用代码；实际 prompt 由契约层解析
    drop(system);
    let span = tracing::info_span!("llm.analyze", model = %config.model, provider = %provider, question_id = question_id);
    let start = std::time::Instant::now();
    let contract = crate::contracts::question::QuestionFull::new(content, eval_answer, existing_ref.as_deref());
    let (out, meta) = contracts::execute(config, pool, uid, &contract).instrument(span).await?;
    let duration_ms = start.elapsed().as_millis() as u64;
    let (a_tags, a_difficulty, a_ref_answer, a_score, a_feedback, skill_id, q_type) = match out {
        contracts::ContractOut::Structured(o) => {
            let sid = crate::services::skill_service::resolve_or_create_skill(
                pool,
                uid,
                o.skill_path.as_deref(),
                o.new_skill.as_ref(),
            ).await.unwrap_or(None);
            (o.tags, o.difficulty, o.ref_answer, o.score, o.feedback, sid, o.question_type)
        }
        // 文本评审模式：feedback=全文、tags 空、score/difficulty=None（ADR-0016 D3）
        contracts::ContractOut::Text(t) => (vec![], None, String::new(), None, t, None, None),
    };
    let prompt_tokens = meta["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let completion_tokens = meta["usage"]["completion_tokens"].as_u64().unwrap_or(0);

    let tags_json = serde_json::to_value(&a_tags)?;
    let aid: i64 = sqlx::query_scalar(
        "INSERT INTO analyses(question_id, provider, model, tags, difficulty, ref_answer, score, feedback, raw, answer_snapshot)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id",
    )
    .bind(question_id)
    .bind(&provider)
    .bind(&config.model)
    .bind(&tags_json)
    .bind(a_difficulty)
    .bind(&a_ref_answer)
    .bind(a_score)
    .bind(&a_feedback)
    .bind(&meta)
    .bind(answer_snapshot.map(str::trim).filter(|s| !s.is_empty()))
    .fetch_one(pool)
    .await?;

    // 更新题目表的技能归属、题型与难度
    let _ = sqlx::query(
        "UPDATE questions SET skill_id=COALESCE($2, skill_id), question_type=COALESCE($3, question_type), difficulty=COALESCE($4, difficulty) WHERE id=$1"
    )
    .bind(question_id)
    .bind(skill_id)
    .bind(q_type)
    .bind(a_difficulty)
    .execute(pool)
    .await;

    if let Some(sid) = skill_id {
        let _ = sqlx::query("INSERT INTO question_skills (question_id, skill_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(question_id)
            .bind(sid)
            .execute(pool)
            .await;
    }

    attach_tags(pool, uid, question_id, &a_tags).await?;
    enqueue_review(pool, question_id).await?;

    tracing::info!(question_id, duration_ms, prompt_tokens, completion_tokens, "analysis done");
    fetch_analysis_row(pool, aid).await
}

/// 按 id 取一条分析（ref/answer/全量三管线共用）
async fn fetch_analysis_row(pool: &sqlx::PgPool, aid: i64) -> Result<AnalysisRow, AppError> {
    let row =
        sqlx::query_as::<_, AnalysisRow>("SELECT id, provider, model, tags, difficulty, ref_answer, score, feedback, answer_snapshot, created_at FROM analyses WHERE id=$1")
            .bind(aid)
            .fetch_one(pool)
            .await?;
    Ok(row)
}

// ---------- 工具函数（供其他模块复用） ----------

pub async fn fetch_question_row(pool: &sqlx::PgPool, uid: i64, id: i64) -> Result<QuestionRow, AppError> {
    let sql = format!(
        r#"{QUESTION_SELECT}
        WHERE q.id=$1 AND q.user_id=$2 GROUP BY q.id
        "#
    );
    let row = sqlx::query_as::<_, QuestionRow>(&sql)
        .bind(id)
        .bind(uid)
        .fetch_one(pool)
        .await?;
    Ok(row)
}

#[tracing::instrument(skip_all)]
pub async fn fetch_followups(pool: &sqlx::PgPool, uid: i64, parent_id: i64) -> Result<Vec<QuestionRow>, AppError> {
    let sql = format!(
        r#"{QUESTION_SELECT}
        WHERE q.parent_id=$1 AND q.user_id=$2 GROUP BY q.id ORDER BY q.created_at ASC, q.id ASC
        "#
    );
    let rows = sqlx::query_as::<_, QuestionRow>(&sql)
        .bind(parent_id)
        .bind(uid)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

#[tracing::instrument(skip_all)]
pub async fn fetch_analyses(pool: &sqlx::PgPool, uid: i64, question_id: i64) -> Result<Vec<AnalysisRow>, AppError> {
    let rows = sqlx::query_as::<_, AnalysisRow>(
        "SELECT a.id, a.provider, a.model, a.tags, a.difficulty, a.ref_answer, a.score, a.feedback, a.answer_snapshot, a.created_at
         FROM analyses a JOIN questions q ON q.id=a.question_id
         WHERE a.question_id=$1 AND q.user_id=$2 ORDER BY a.created_at DESC, a.id DESC",
    )
    .bind(question_id)
    .bind(uid)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 可复习判定：已分析或已写我的回答 -> 自动入复习队（ADR-0007）
#[tracing::instrument(skip_all)]
pub async fn enqueue_review(pool: &sqlx::PgPool, question_id: i64) -> Result<(), AppError> {
    sqlx::query("INSERT INTO review_records(question_id) VALUES($1) ON CONFLICT (question_id) DO NOTHING")
        .bind(question_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 自由标签：自动去重入 tags 表并关联（LLM 与人工共用），并自动绑定技能图谱（上限硬约束 <= 3）
#[tracing::instrument(skip_all)]
pub async fn attach_tags(pool: &sqlx::PgPool, uid: i64, question_id: i64, tags: &[String]) -> Result<(), AppError> {
    // 硬约束：最多保留 3 个有效标签；去重保序（票03：数组内重名会让 UNNEST upsert
    // 二次命中同一行，PG 拒绝执行——"ON CONFLICT DO UPDATE cannot affect row a second time"）
    let mut seen = std::collections::HashSet::new();
    let valid_tags: Vec<&str> = tags
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert(*s))
        .take(3)
        .collect();

    if !valid_tags.is_empty() {
        // 先清理该题目已有旧标签，避免历史标签越积越多
        let _ = sqlx::query("DELETE FROM question_tags WHERE question_id=$1")
            .bind(question_id)
            .execute(pool)
            .await;

        // 评审 P3 整改：批量 upsert（此前每标签 2 条语句的 N+1 循环）
        let names: Vec<&str> = valid_tags.clone();
        let tag_ids: Vec<i64> = sqlx::query_scalar(
            "INSERT INTO tags(user_id, name) SELECT $1, x FROM UNNEST($2) AS x \
             ON CONFLICT (user_id, name) DO UPDATE SET name=EXCLUDED.name RETURNING id",
        )
        .bind(uid)
        .bind(&names)
        .fetch_all(pool)
        .await?;
        let dedup: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            tag_ids.into_iter().filter(|id| seen.insert(*id)).collect()
        };
        sqlx::query(
            "INSERT INTO question_tags(question_id, tag_id) SELECT $1, x FROM UNNEST($2) AS x ON CONFLICT DO NOTHING",
        )
        .bind(question_id)
        .bind(&dedup)
        .execute(pool)
        .await?;
    }
    // 自动挂载至已有同名/相似技能树节点
    let _ = crate::services::skill_service::auto_bind_skills_by_tags(pool, uid, question_id, tags).await;
    Ok(())
}

// ---------- 回答历史 + 题↔轮次关联（v3 用户反馈） ----------

/// 记录一条回答历史（手动/复习/面试共用；幂等性由调用方保证——每个动作只记一次）。
/// 接受任意 executor（连接池或事务），供事务内使用（评审 P1 整改）。
#[tracing::instrument(skip_all)]
pub async fn record_answer(
    db: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    question_id: i64,
    source: &str,
    content: &str,
) -> Result<(), AppError> {
    let content = content.trim();
    if content.is_empty() {
        return Ok(());
    }
    sqlx::query("INSERT INTO question_answers(question_id, source, content) VALUES($1,$2,$3)")
        .bind(question_id)
        .bind(source)
        .bind(content)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn fetch_answers(pool: &sqlx::PgPool, uid: i64, question_id: i64) -> Result<Vec<crate::models::AnswerRow>, AppError> {
    let rows = sqlx::query_as::<_, crate::models::AnswerRow>(
        "SELECT qa.id, qa.question_id, qa.source, qa.content, qa.created_at FROM question_answers qa
         JOIN questions q ON q.id=qa.question_id
         WHERE qa.question_id=$1 AND q.user_id=$2 ORDER BY qa.created_at DESC, qa.id DESC LIMIT 50",
    )
    .bind(question_id)
    .bind(uid)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 把题关联到某轮次（多面试关联；主轮次也写进去，便于统一按 question_rounds 查询）
#[tracing::instrument(skip_all)]
pub async fn link_round(pool: &sqlx::PgPool, question_id: i64, round_id: i64) -> Result<(), AppError> {
    sqlx::query("INSERT INTO question_rounds(question_id, round_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(question_id)
        .bind(round_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn fetch_round_links(pool: &sqlx::PgPool, uid: i64, question_id: i64) -> Result<Vec<crate::models::RoundLinkRow>, AppError> {
    let rows = sqlx::query_as::<_, crate::models::RoundLinkRow>(
        r#"
        SELECT qr.round_id, r.name AS round_name, a.id AS application_id, p.department, p.title AS position,
               c.name AS company, r.date, r.passed
        FROM question_rounds qr
        JOIN rounds r ON r.id = qr.round_id
        JOIN applications a ON a.id = r.application_id
        JOIN positions p ON p.id = a.position_id
        LEFT JOIN companies c ON c.id = p.company_id
        WHERE qr.question_id=$1 AND a.user_id=$2
        ORDER BY COALESCE(r.date, a.created_at::date) DESC, r.id
        "#,
    )
    .bind(question_id)
    .bind(uid)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[derive(Deserialize)]
struct RecordAnswerReq {
    pub content: String,
    pub source: Option<String>,
}
use serde::Deserialize;

#[tracing::instrument(skip_all)]
async fn record_answer_route(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<RecordAnswerReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 归属校验
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM questions WHERE id=$1 AND user_id=$2)")
        .bind(id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    if !owned {
        return Err(AppError::NotFound);
    }
    let source = match req.source.as_deref() {
        Some("manual") | Some("review") | Some("interview") => req.source.unwrap(),
        _ => "manual".to_string(),
    };
    record_answer(&state.pool, id, &source, &req.content).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AddRoundLinkReq {
    pub round_id: i64,
}

#[tracing::instrument(skip_all)]
async fn add_round_link(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<AddRoundLinkReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let q_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM questions WHERE id=$1 AND user_id=$2)")
        .bind(id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    if !q_exists {
        return Err(AppError::NotFound);
    }
    let r_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rounds r JOIN applications a ON a.id=r.application_id WHERE r.id=$1 AND a.user_id=$2)",
    )
    .bind(req.round_id)
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    if !r_exists {
        return Err(AppError::BadRequest("轮次不存在".to_string()));
    }
    link_round(&state.pool, id, req.round_id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[tracing::instrument(skip_all)]
async fn remove_round_link(
    State(state): State<AppState>,
    Path((id, rid)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 主归属（questions.round_id）不可解除，只能解关联表里的
    let deleted = sqlx::query("DELETE FROM question_rounds WHERE question_id=$1 AND round_id=$2")
        .bind(id)
        .bind(rid)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}
