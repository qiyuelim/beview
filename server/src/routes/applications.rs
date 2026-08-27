//! v3 投递跟踪（ADR-0009 §5）+ ADR-0012 岗位实体：投递 = 引用一个岗位进入管道。
//! 状态机：applied(已投) → interviewing(进行中) → offer/rejected/withdrawn。
//! 只进不退（forward-only）；终态不可再流转；applied→interviewing 由添加首场面试自动推进（sessions.rs）。

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::CurrentUser;
use crate::contracts;
use crate::error::AppError;
use crate::observe::truncate_chars;
use crate::settings;
use crate::services::application_service::{self, TransitionSource};
use crate::services::system_containers;
use crate::state::{AiJob, AiStart, AppState};
use tracing::Instrument;

/// 任务受理响应（ADR-0013 D2）
fn job_accepted(j: &AiJob) -> Value {
    json!({ "job_id": j.id, "status": j.status })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/applications", get(list_applications).post(create_application))
        .route(
            "/applications/{id}",
            axum::routing::patch(update_application)
                .delete(delete_application)
                .get(get_application_detail),
        )
        .route("/applications/batch-status", post(batch_status))
        .route("/applications/batch-delete", post(batch_delete))
        .route("/applications/{id}/interpret", post(interpret_jd))
        .route("/applications/{id}/match", post(match_jd))
        // 票07：投递全局智能洞察（异步 AiJob + 结果落库可回看）
        .route(
            "/applications/insights",
            get(get_latest_insight).post(generate_insights),
        )
}

pub const STATUSES: [&str; 5] = ["applied", "interviewing", "offer", "rejected", "withdrawn"];

/// 合法流转表（forward-only，ADR-0011 M3 UX 整改）：终态（offer/rejected/withdrawn）不可再流转。
fn validate_status(s: Option<&str>) -> Result<&'static str, AppError> {
    match s {
        None => Ok("applied"),
        Some("applied") => Ok("applied"),
        Some("interviewing") => Ok("interviewing"),
        Some("offer") => Ok("offer"),
        Some("rejected") => Ok("rejected"),
        Some("withdrawn") => Ok("withdrawn"),
        Some(other) => Err(AppError::BadRequest(format!("非法投递状态: {other}"))),
    }
}

#[derive(FromRow, Serialize)]
pub struct ApplicationRow {
    pub id: i64,
    pub position_id: i64,
    /// 岗位标题（join positions，字段名保持 position 兼容既有前端消费方）
    pub position: Option<String>,
    /// 工作地点（岗位属性，ADR-0012）
    pub location: Option<String>,
    pub company_id: Option<i64>,
    pub company: Option<String>,
    /// offer 薪资（TEXT 自由格式，如「25k·16薪」）
    pub salary: Option<String>,
    pub channel: Option<String>,
    pub applied_at: DateTime<Utc>,
    pub status: String,
    pub note: Option<String>,
    /// 部门（岗位属性，ADR-0014 D1：join positions 输出同名字段保持前端兼容）
    pub department: Option<String>,
    pub jd_interpret: Option<Value>,
    pub jd_match: Option<Value>,
    pub resume_id: Option<i64>,
    pub resume_version_name: Option<String>,
    /// 面试轮次进度（看板卡展示面试阶段）：[{name, passed}]
    pub interview_stages: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
use serde::Serialize;

const APP_SELECT: &str = r#"
    SELECT a.id, p.id AS position_id, p.title AS position, p.location,
           c.id AS company_id, c.name AS company, a.salary, p.department AS department, a.channel,
           a.applied_at, a.status, a.note, p.jd_interpret, a.jd_match,
           a.resume_id, res.version_name AS resume_version_name,
           (SELECT COALESCE(json_agg(json_build_object('name', r.name, 'passed', r.passed, 'date', r.date) ORDER BY r.sort_order, r.id), '[]'::json)
              FROM rounds r WHERE r.application_id=a.id) AS interview_stages,
           a.created_at, a.updated_at
    FROM applications a
    JOIN positions p ON p.id = a.position_id
    JOIN companies c ON c.id = p.company_id
    LEFT JOIN resumes res ON res.id = a.resume_id
"#;

#[tracing::instrument(skip_all)]
async fn list_applications(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<ApplicationRow>>, AppError> {
    // ADR-0014 §16：看板必须排除系统公司投递（否则回收站墓碑重回看板）
    let rows = sqlx::query_as::<_, ApplicationRow>(&format!(
        "{APP_SELECT} WHERE a.user_id = $1 AND NOT c.is_system ORDER BY CASE a.status WHEN 'applied' THEN 1 WHEN 'interviewing' THEN 2 WHEN 'offer' THEN 3 ELSE 4 END, a.applied_at DESC"
    ))
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct CreateApplicationReq {
    pub company_id: Option<i64>,
    /// 直接输新公司名（与 company_id 二选一，优先 company_id）；公司必填（岗位须属公司）
    pub company_name: Option<String>,
    /// 岗位标题（find-or-create positions）
    pub position: Option<String>,
    /// 岗位所属部门
    pub department: Option<String>,
    /// 岗位地点（写入岗位）
    pub location: Option<String>,
    /// JD 原文（写入岗位）
    pub jd_text: Option<String>,
    pub channel: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    pub status: Option<String>,
    pub note: Option<String>,
    pub resume_id: Option<i64>,
}
use serde::Deserialize;

#[tracing::instrument(skip_all)]
async fn create_application(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateApplicationReq>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let status = validate_status(req.status.as_deref())?;
    // 公司解析（必填，岗位须属公司）：company_id 优先；否则 company_name find-or-create
    let company_id: i64 = if let Some(cid) = req.company_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM companies WHERE id=$1 AND user_id=$2)",
        )
        .bind(cid)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
        if !exists {
            return Err(AppError::BadRequest("关联公司不存在".to_string()));
        }
        cid
    } else {
        let name = req
            .company_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| AppError::BadRequest("请选择或输入公司".to_string()))?;
        sqlx::query_scalar(
            "INSERT INTO companies(user_id, name) VALUES($1,$2)
             ON CONFLICT (user_id, name) DO UPDATE SET name=EXCLUDED.name RETURNING id",
        )
        .bind(user.0)
        .bind(name)
        .fetch_one(&state.pool)
        .await?
    };
    // 岗位 find-or-create（ADR-0012）：同公司同标题复用；提供部门/地点/JD 时写入（最新为准）
    let title = req
        .position
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("请填写岗位名称".to_string()))?;
    let position_id: i64 = sqlx::query_scalar(
        "INSERT INTO positions(user_id, company_id, title, department, location, jd_text)
         VALUES($1,$2,$3,$4,$5,$6)
         ON CONFLICT (user_id, company_id, title) DO UPDATE
           SET department = COALESCE(EXCLUDED.department, positions.department),
               location   = COALESCE(EXCLUDED.location, positions.location),
               jd_text    = COALESCE(EXCLUDED.jd_text, positions.jd_text)
         RETURNING id",
    )
    .bind(user.0)
    .bind(company_id)
    .bind(title)
    .bind(req.department.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.location.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.jd_text.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .fetch_one(&state.pool)
    .await?;
    if let Some(rid) = req.resume_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resumes WHERE id=$1 AND user_id=$2)")
            .bind(rid)
            .bind(user.0)
            .fetch_one(&state.pool)
            .await?;
        if !exists {
            return Err(AppError::BadRequest("关联简历不存在".to_string()));
        }
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO applications(user_id, position_id, channel, applied_at, status, note, resume_id)
         VALUES($1,$2,$3,COALESCE($4, now()),$5,$6,$7) RETURNING id",
    )
    .bind(user.0)
    .bind(position_id)
    .bind(req.channel)
    .bind(req.applied_at)
    .bind(status)
    .bind(req.note)
    .bind(req.resume_id)
    .fetch_one(&state.pool)
    .await?;
    // 初始事件（from=NULL 表示起点）
    record_event(&state.pool, user.0, id, None, status, "create", None).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[tracing::instrument(skip_all)]
/// 详情聚合（ADR-0011 R3）：投递本体 + 状态流水 + 关联批次（含轮次数/均分）
#[tracing::instrument(skip_all)]
async fn get_application_detail(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let application = sqlx::query_as::<_, ApplicationRow>(&format!("{APP_SELECT} WHERE a.id=$1 AND a.user_id=$2"))
        .bind(id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let events = sqlx::query_as::<_, ApplicationEvent>(
        "SELECT id, kind, from_status, to_status, source, note, created_at FROM application_events
         WHERE application_id=$1 AND user_id=$2 ORDER BY id DESC",
    )
    .bind(id)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    // 解读任务挂岗位 id；匹配度仍挂投递 id
    let mut ai_jobs: Vec<AiJob> = Vec::new();
    if let Some(j) = state.ai_jobs.running_for(user.0, "jd_interpret", application.position_id) {
        ai_jobs.push(j);
    }
    if let Some(j) = state.ai_jobs.running_for(user.0, "jd_match", id) {
        ai_jobs.push(j);
    }

    let rounds = sqlx::query_as::<_, RoundBrief>(
        r#"
        SELECT r.id, r.name, r.sort_order, r.date, r.form, r.passed,
               to_char(r.created_at, 'YYYY-MM-DD') AS created,
               (SELECT count(*) FROM questions q WHERE q.round_id=r.id)::bigint AS question_count
        FROM rounds r WHERE r.application_id=$1
        ORDER BY r.sort_order, r.id
        "#,
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({
        "application": application,
        "events": events,
        "rounds": rounds,
        "ai_jobs": ai_jobs,
    })))
}

#[derive(serde::Serialize, sqlx::FromRow)]
struct RoundBrief {
    pub id: i64,
    pub name: String,
    pub sort_order: i32,
    pub date: Option<chrono::NaiveDate>,
    pub form: Option<String>,
    pub passed: String,
    pub created: Option<String>,
    pub question_count: i64,
}

#[derive(serde::Serialize, sqlx::FromRow)]
struct ApplicationEvent {
    pub id: i64,
    /// status（投递状态流转）| round（面试轮次跟踪）
    pub kind: String,
    pub from_status: Option<String>,
    pub to_status: String,
    pub source: String,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 追加一条状态流水（append-only，不改写）。pub(crate)：sessions.rs 自动推进时复用。
/// 接受任意 executor（连接池或事务），供状态机事务内使用（评审 P1 整改）。
pub(crate) async fn record_event(
    db: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    uid: i64,
    application_id: i64,
    from_status: Option<&str>,
    to_status: &str,
    source: &str,
    note: Option<&str>,
) -> Result<(), AppError> {
    record_event_kind(db, uid, application_id, "status", Some(from_status.unwrap_or("")), to_status, source, note).await
}

/// 事件记录（ADR-0014 扩展）：kind=status（投递状态流转）| round（面试轮次跟踪，from/to 留空）
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_event_kind(
    db: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    uid: i64,
    application_id: i64,
    kind: &str,
    from_status: Option<&str>,
    to_status: &str,
    source: &str,
    note: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO application_events(user_id, application_id, kind, from_status, to_status, source, note)
         VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(uid)
    .bind(application_id)
    .bind(kind)
    .bind(from_status)
    .bind(to_status)
    .bind(source)
    .bind(note)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Deserialize)]
struct UpdateApplicationReq {
    pub channel: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    pub status: Option<String>,
    pub note: Option<String>,
    /// offer 薪资（TEXT 自由格式）
    pub salary: Option<String>,
    pub department: Option<String>,
    pub resume_id: Option<i64>,
}

/// PATCH 是基础 API，不是绕过状态机的后门（ADR-0014 §8）：
/// status 分支一律进 application_service::transition（守卫/流水/补标/积分都在那里）。
#[tracing::instrument(skip_all)]
async fn update_application(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateApplicationReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 归属 + 系统容器写保护（回收站/自录题库投递不可经业务 API 修改）
    ensure_business_application(&state.pool, user.0, id).await?;

    if let Some(rid) = req.resume_id {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resumes WHERE id=$1 AND user_id=$2)")
            .bind(rid)
            .bind(user.0)
            .fetch_one(&state.pool)
            .await?;
        if !exists {
            return Err(AppError::BadRequest("关联简历不存在".to_string()));
        }
    }

    if let Some(dept) = &req.department {
        sqlx::query(
            "UPDATE positions SET department=$1, updated_at=now()
             WHERE id=(SELECT position_id FROM applications WHERE id=$2 AND user_id=$3) AND user_id=$3"
        )
        .bind(if dept.trim().is_empty() { None } else { Some(dept.trim()) })
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    }

    // 非状态字段先行更新
    let updated = sqlx::query(
        r#"
        UPDATE applications SET
          channel=COALESCE($2, channel), applied_at=COALESCE($3, applied_at),
          note=COALESCE($4, note), salary=COALESCE($5, salary),
          resume_id=COALESCE($6, resume_id), updated_at=now()
        WHERE id=$1 AND user_id=$7
        "#,
    )
    .bind(id)
    .bind(req.channel)
    .bind(req.applied_at)
    .bind(req.note)
    .bind(req.salary)
    .bind(req.resume_id)
    .bind(user.0)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    if let Some(s) = req.status.as_deref() {
        let to = validate_status(Some(s))?.to_string();
        application_service::transition(&state.pool, user.0, id, &to, TransitionSource::Manual).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// 归属校验 + 系统容器写保护：is_system 公司的投递禁止业务 API 触达（ADR-0014 §17）
async fn ensure_business_application(pool: &sqlx::PgPool, uid: i64, id: i64) -> Result<(), AppError> {
    let row: Option<bool> = sqlx::query_scalar(
        "SELECT c.is_system FROM applications a
         JOIN positions p ON p.id=a.position_id JOIN companies c ON c.id=p.company_id
         WHERE a.id=$1 AND a.user_id=$2",
    )
    .bind(id)
    .bind(uid)
    .fetch_optional(pool)
    .await?;
    match row {
        None => Err(AppError::NotFound),
        Some(true) => Err(AppError::BadRequest("系统容器投递不可修改".to_string())),
        Some(false) => Ok(()),
    }
}

/// 删除投递（ADR-0014 §14）：事务内先把题目迁移到墓碑轮次，再删投递——题目不随投递消失。
pub(crate) async fn delete_with_tombstone(pool: &sqlx::PgPool, uid: i64, id: i64) -> Result<(), AppError> {
    ensure_business_application(pool, uid, id).await?;
    let tombstone = system_containers::ensure_tombstone_round(pool, uid).await?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE questions q SET round_id=$2 FROM rounds r
         WHERE q.round_id=r.id AND r.application_id=$1",
    )
    .bind(id)
    .bind(tombstone)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM applications WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(uid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tracing::instrument(skip_all)]
async fn delete_application(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    delete_with_tombstone(&state.pool, user.0, id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// 批量状态流转（ADR-0014 §10）：逐条过统一状态机，合法执行、非法跳过（局部成功）。
#[derive(Deserialize)]
struct BatchStatusReq {
    pub ids: Vec<i64>,
    pub status: String,
}

#[tracing::instrument(skip_all)]
async fn batch_status(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<BatchStatusReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let to = validate_status(Some(req.status.as_str()))?.to_string();
    // 终态目标（offer）不允许批量直达：Offer 唯一入口在详情页（§4.1）
    if to == "offer" {
        return Err(AppError::BadRequest(
            "Offer 不能批量设置：请到投递详情使用「整场通过·标记 Offer」".to_string(),
        ));
    }
    let (mut succeeded, mut failed) = (Vec::new(), Vec::new());
    for id in req.ids {
        match application_service::transition(&state.pool, user.0, id, &to, TransitionSource::Manual).await {
            Ok(_) => succeeded.push(id),
            Err(AppError::NotFound) => failed.push(json!({ "id": id, "reason": "not_found" })),
            Err(e) => failed.push(json!({ "id": id, "reason": e.to_string() })),
        }
    }
    Ok(Json(json!({ "succeeded": succeeded, "failed": failed })))
}

/// 批量删除（ADR-0014 §11）：全量预校验后才开事务逐个执行，避免半删除。
#[derive(Deserialize)]
struct BatchDeleteReq {
    pub ids: Vec<i64>,
}

#[tracing::instrument(skip_all)]
async fn batch_delete(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<BatchDeleteReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 预校验：全部必须存在、归属本人、且非系统容器投递
    let mut invalid = Vec::new();
    for id in &req.ids {
        if ensure_business_application(&state.pool, user.0, *id).await.is_err() {
            invalid.push(*id);
        }
    }
    if !invalid.is_empty() {
        return Err(AppError::BadRequest(format!(
            "以下投递不存在或不可删除: {:?}",
            invalid
        )));
    }
    for id in &req.ids {
        delete_with_tombstone(&state.pool, user.0, *id).await?;
    }
    Ok(Json(json!({ "deleted": req.ids.len() })))
}


/// JD 解读：兼容入口，实际归属岗位（同岗共享）。仅用户点击触发。
#[tracing::instrument(skip_all)]
async fn interpret_jd(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let pid: Option<i64> = sqlx::query_scalar(
        "SELECT position_id FROM applications WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;
    let pid = pid.ok_or(AppError::NotFound)?;
    crate::routes::companies::start_position_jd_interpret(&state, user.0, pid).await
}

/// 简历-JD 匹配度（ADR-0011 R4.a）：推理式评估，落 applications.jd_match。JD 读岗位（ADR-0012）。
#[tracing::instrument(skip_all)]
async fn match_jd(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let (jd,): (Option<String>,) = sqlx::query_as(
        "SELECT p.jd_text FROM applications a JOIN positions p ON p.id=a.position_id WHERE a.id=$1 AND a.user_id=$2",
    )
    .bind(id)
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    let jd = jd.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        AppError::BadRequest("请先到岗位详情填写 JD 原文".to_string())
    })?;
    let resume: Option<Value> = sqlx::query_scalar(
        "SELECT parsed FROM resumes WHERE user_id=$1 AND is_active ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;
    let resume = resume.filter(|v| v.is_object()).ok_or_else(|| {
        AppError::BadRequest("请先到简历页填写或解析简历，再评估匹配度".to_string())
    })?;

    let job = match state.ai_jobs.start(user.0, "jd_match", id) {
        AiStart::AlreadyRunning(j) => return Ok(Json(job_accepted(&j))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    let user_msg = format!("【目标岗位 JD】\n{jd}\n\n【候选人简历（结构化）】\n{}", serde_json::to_string_pretty(&resume)?);
    // panic 守卫统一收尾（评审 P0）；契约层：prompt/schema/解析/score 钳制内聚在 JdMatch
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let config = settings::require_llm(&st.pool, uid).await?;
        let span = tracing::info_span!("llm.jd_match", model = %config.model, provider = %config.provider, application_id = id);
        let contract = crate::contracts::jd::JdMatch::new(user_msg);
        let (result, _meta) = contracts::execute(&config, &st.pool, uid, &contract)
            .instrument(span)
            .await?;
        let parsed = match result {
            contracts::ContractOut::Structured(v) => serde_json::to_value(v)?,
            contracts::ContractOut::Text(t) => serde_json::json!({ "ir_mode": "text", "content": t }),
        };
        sqlx::query("UPDATE applications SET jd_match=$2, updated_at=now() WHERE id=$1 AND user_id=$3")
            .bind(id)
            .bind(json!(parsed))
            .bind(uid)
            .execute(&st.pool)
            .await?;
        Ok::<_, AppError>(())
    });
    Ok(Json(job_accepted(&job)))
}

// ==================== 投递全局智能洞察（票07） ====================

/// 装配投递上下文文本：投递+流水+轮次复盘摘要（每用户最多 50 条投递，防 prompt 失控）
async fn assemble_insights_context(pool: &sqlx::PgPool, uid: i64) -> Result<String, AppError> {
    #[derive(FromRow)]
    struct AppRow {
        id: i64,
        company: String,
        title: String,
        status: String,
        channel: Option<String>,
        applied_at: chrono::DateTime<chrono::Utc>,
        note: Option<String>,
    }
    let apps: Vec<AppRow> = sqlx::query_as(
        r#"
        SELECT a.id, c.name AS company, p.title, a.status, a.channel,
               a.applied_at, a.note
        FROM applications a
        JOIN positions p ON p.id=a.position_id
        JOIN companies c ON c.id=p.company_id
        WHERE a.user_id=$1
        ORDER BY a.updated_at DESC LIMIT 50
        "#,
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;
    if apps.is_empty() {
        return Err(AppError::BadRequest("暂无投递数据——先在「求职台」添加投递并跟进状态后，再来生成洞察".to_string()));
    }

    let mut lines: Vec<String> = Vec::new();
    for (i, a) in apps.iter().enumerate() {
        lines.push(format!(
            "【投递{}】{} · {} · 状态:{}{} · 投递于 {}",
            i + 1,
            a.company,
            a.title,
            a.status,
            a.channel.as_deref().map(|c| format!(" · 渠道:{c}")).unwrap_or_default(),
            a.applied_at.format("%Y-%m-%d"),
        ));
        if let Some(n) = a.note.as_deref().filter(|n| !n.trim().is_empty()) {
            lines.push(format!("  备注：{}", truncate_chars(n, 120)));
        }
    }

    // 状态流水（全用户聚合，按时间正序拼接为箭头链）
    #[derive(FromRow)]
    struct EvRow {
        application_id: i64,
        from_status: Option<String>,
        to_status: String,
        created_at: chrono::DateTime<chrono::Utc>,
    }
    let events: Vec<EvRow> = sqlx::query_as(
        r#"
        SELECT ae.application_id, ae.from_status, ae.to_status, ae.created_at
        FROM application_events ae
        JOIN applications a ON a.id=ae.application_id
        WHERE a.user_id=$1 ORDER BY ae.created_at ASC
        "#,
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;
    if !events.is_empty() {
        lines.push(String::new());
        lines.push("【状态流水】".into());
        let mut by_app: std::collections::BTreeMap<i64, Vec<&EvRow>> = std::collections::BTreeMap::new();
        for e in &events {
            by_app.entry(e.application_id).or_default().push(e);
        }
        let company_of: std::collections::HashMap<i64, &str> =
            apps.iter().map(|a| (a.id, a.company.as_str())).collect();
        for (aid, evs) in &by_app {
            // 每段流转带日期（月-日），让模型能推断节奏与停滞时长
            let chain: Vec<String> = evs
                .iter()
                .map(|e| {
                    let d = e.created_at.format("%m-%d");
                    match e.from_status.as_deref() {
                        Some(f) => format!("{f}@{d}→{}", e.to_status),
                        None => format!("{}@{d}", e.to_status),
                    }
                })
                .collect();
            lines.push(format!(
                "  {}（#{}）：{}",
                company_of.get(aid).copied().unwrap_or("未知公司"),
                aid,
                chain.join(" → ")
            ));
        }
    }

    // 轮次复盘摘要（rounds.retrospective JSONB，防御性截断）
    #[derive(FromRow)]
    struct RetroRow {
        application_id: i64,
        name: Option<String>,
        retrospective: Value,
    }
    let retros: Vec<RetroRow> = sqlx::query_as(
        r#"
        SELECT r.application_id, r.name, r.retrospective
        FROM rounds r JOIN applications a ON a.id=r.application_id
        WHERE a.user_id=$1 AND r.retrospective IS NOT NULL
        ORDER BY r.created_at DESC LIMIT 20
        "#,
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;
    if !retros.is_empty() {
        lines.push(String::new());
        lines.push("【轮次复盘摘要】".into());
        for r in retros {
            let company = apps
                .iter()
                .find(|a| a.id == r.application_id)
                .map(|a| a.company.as_str())
                .unwrap_or("未知");
            lines.push(format!(
                "  {} · {}：{}",
                company,
                r.name.as_deref().unwrap_or("面试"),
                truncate_chars(&r.retrospective.to_string(), 500),
            ));
        }
    }

    Ok(lines.join("\n"))
}

/// 发起全局洞察任务（票07）：受理幂等（同用户同出口去重），结果落库可回看。
#[tracing::instrument(skip_all)]
async fn generate_insights(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(_): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let context = assemble_insights_context(&state.pool, user.0).await?;
    let config = settings::require_llm(&state.pool, user.0).await?;

    // 全局单例目标：target_id=0（洞察无单一实体锚点，per-user 幂等键即 (uid, kind, 0)）
    let job = match state.ai_jobs.start(user.0, "app_insights", 0) {
        AiStart::AlreadyRunning(j) => return Ok(Json(job_accepted(&j))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let contract = contracts::insights::ApplicationInsights::new(context);
        let (result, _meta) = contracts::execute(&config, &st.pool, uid, &contract).await?;
        // 文本降级模式：把 Markdown 全文包进 summary 字段落库（前端按整体渲染）
        let payload = match result {
            contracts::ContractOut::Structured(report) => serde_json::to_value(&report)?,
            contracts::ContractOut::Text(text) => json!({ "summary": text }),
        };
        sqlx::query(
            "INSERT INTO application_insights(user_id, payload) VALUES($1,$2)",
        )
        .bind(uid)
        .bind(payload)
        .execute(&st.pool)
        .await?;
        tracing::info!(event = "insight.generated", user_id = uid, job_id = job.id, "投递洞察报告已落库");
        Ok::<_, AppError>(())
    });
    tracing::info!(user_id = user.0, job_id = job.id, "发起投递洞察任务");
    Ok(Json(job_accepted(&job)))
}

#[derive(FromRow)]
#[allow(dead_code)] // id 随 SELECT 取出但暂未消费（回看列表功能预留）
struct InsightRow {
    id: i64,
    payload: Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// 最近一次洞察结果（含 running 态供刷新恢复；无数据时 insight=null 由前端引导）
#[tracing::instrument(skip_all)]
async fn get_latest_insight(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query_as::<_, InsightRow>(
        "SELECT id, payload, created_at FROM application_insights WHERE user_id=$1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;
    // 刷新恢复通道（ADR-0013 D3）：running 态以 ai_jobs[] 形态暴露，前端 trackRunning 恢复跟踪
    let ai_jobs: Vec<AiJob> = match state.ai_jobs.running_for(user.0, "app_insights", 0) {
        Some(j) => vec![j],
        None => Vec::new(),
    };
    Ok(Json(json!({
        "insight": row.map(|r| json!({
            "created_at": r.created_at.to_rfc3339(),
            "summary": r.payload.get("summary"),
            "observations": r.payload.get("observations").cloned().unwrap_or(json!([])),
            "recommendations": r.payload.get("recommendations").cloned().unwrap_or(json!([])),
            "priority": r.payload.get("priority").cloned().unwrap_or(json!([])),
        })),
        "ai_jobs": ai_jobs,
    })))
}
