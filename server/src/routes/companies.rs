//! 公司与岗位（ADR-0012）：公司 → 岗位 → 投递 三层实体。
//! - GET  /api/companies            公司列表（岗位数/投递数/最近活动）
//! - POST /api/companies            新建公司
//! - GET  /api/companies/:id        公司详情 + 岗位卡片数据
//! - PATCH /api/companies/:id       改描述
//! - POST /api/companies/:id/positions  新建岗位
//! - GET    /api/positions/:id      岗位详情（含公司、predict_result、ai_jobs）
//! - PATCH  /api/positions/:id      改标题/地点/JD
//! - DELETE /api/positions/:id      删岗位（有投递时拒绝）
//! - GET    /api/positions/:id/applications  该岗所有投递

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::CurrentUser;
use crate::contracts::{self, jd::PositionPredict, ContractOut};
use crate::error::AppError;
use crate::events::DomainEvent;
use crate::models::CreateCompanyReq;
use crate::settings;
use crate::state::{AiJob, AiStart, AppState};
use tracing::Instrument;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/companies", get(list_companies).post(create_company))
        .route(
            "/companies/{id}",
            get(get_company).patch(patch_company),
        )
        .route("/companies/{id}/topic-profile", get(company_topic_profile))
        .route("/companies/{id}/positions", get(list_positions).post(create_position))
        .route(
            "/positions/{id}",
            get(get_position).patch(patch_position).delete(delete_position),
        )
        .route("/positions/{id}/applications", get(position_applications))
        .route("/positions/{id}/predict", post(predict_position_questions))
        .route("/positions/{id}/predict/ingest", post(ingest_predicted_questions))
        .route("/positions/{id}/predict/drill", post(create_predicted_drill))
        .route("/positions/{id}/interpret", post(interpret_position_jd))
}

// ---------- 公司列表 ----------

#[derive(FromRow, Serialize)]
pub struct CompanySummary {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub position_count: i64,
    pub application_count: i64,
    pub last_activity: Option<DateTime<Utc>>,
}

/// 公司列表。默认排除系统公司（回收站/自录题库/模拟面试）；
/// 题库筛选传 ?include_system=true 取全量（ADR-0014 D6）。
#[derive(Deserialize)]
struct CompanyListQuery {
    pub include_system: Option<bool>,
}

#[derive(FromRow, Serialize)]
struct CompanyAdminRow {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_system: bool,
    pub position_count: i64,
    pub application_count: i64,
    pub last_activity: Option<DateTime<Utc>>,
}

#[tracing::instrument(skip_all)]
async fn list_companies(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<CompanyListQuery>,
) -> Result<Json<Vec<serde_json::Value>>, AppError> {
    let exclude = if q.include_system == Some(true) { "false" } else { "c.is_system" };
    let rows = sqlx::query_as::<_, CompanyAdminRow>(
        &format!(
        r#"
        SELECT c.id, c.name, c.description, c.is_system,
          (SELECT count(*) FROM positions p WHERE p.company_id=c.id)::bigint AS position_count,
          (SELECT count(*) FROM applications a JOIN positions p ON p.id=a.position_id
             WHERE p.company_id=c.id)::bigint AS application_count,
          GREATEST(
            c.created_at,
            COALESCE((SELECT max(a.applied_at) FROM applications a JOIN positions p ON p.id=a.position_id
               WHERE p.company_id=c.id), c.created_at)
          ) AS last_activity
        FROM companies c
        WHERE c.user_id = $1 AND NOT {exclude}
        ORDER BY last_activity DESC, c.id DESC
        "#
        )
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    // 非系统公司保持原字段形状；include_system 时附加 is_system 标记
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id, "name": r.name, "description": r.description,
                "position_count": r.position_count, "application_count": r.application_count,
                "last_activity": r.last_activity, "is_system": r.is_system,
            })
        })
        .collect();
    Ok(Json(out))
}

#[tracing::instrument(skip_all)]
async fn create_company(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateCompanyReq>,
) -> Result<impl IntoResponse, AppError> {
    let uid = user.0;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("公司名不能为空".to_string()));
    }
    // ADR-0014 §17 写保护：系统公司名保留，只能经 ensure_*() 创建
    if crate::services::system_containers::TOMBSTONE_COMPANY == name
        || crate::services::system_containers::SELF_COMPANY == name
        || name == "模拟面试"
    {
        return Err(AppError::BadRequest("该公司名为系统保留名".to_string()));
    }
    let id: i64 = match sqlx::query_scalar("INSERT INTO companies(user_id, name) VALUES($1, $2) RETURNING id")
        .bind(uid)
        .bind(name)
        .fetch_one(&state.pool)
        .await
    {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.constraint() == Some("companies_user_name_key") => {
            return Err(AppError::Conflict("公司已存在".to_string()))
        }
        Err(e) => return Err(e.into()),
    };
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

// ---------- 公司详情 / 描述 ----------

#[derive(Deserialize)]
struct PatchCompanyReq {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn get_company(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let sys: Option<bool> = sqlx::query_scalar("SELECT is_system FROM companies WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?;
    if sys == Some(true) {
        return Err(AppError::BadRequest("系统容器公司不可打开".to_string()));
    }
    let company = sqlx::query_as::<_, CompanySummary>(
        r#"
        SELECT c.id, c.name, c.description,
          (SELECT count(*) FROM positions p WHERE p.company_id=c.id)::bigint AS position_count,
          (SELECT count(*) FROM applications a JOIN positions p ON p.id=a.position_id
             WHERE p.company_id=c.id)::bigint AS application_count,
          GREATEST(c.created_at,
            COALESCE((SELECT max(a.applied_at) FROM applications a JOIN positions p ON p.id=a.position_id
               WHERE p.company_id=c.id), c.created_at)) AS last_activity
        FROM companies c
        WHERE c.id=$1 AND c.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let positions = positions_with_stats(&state.pool, user.0, Some(id)).await?;
    Ok(Json(json!({ "company": company, "positions": positions })))
}

#[tracing::instrument(skip_all)]
async fn patch_company(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<PatchCompanyReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let comp: Option<(bool, String)> = sqlx::query_as("SELECT is_system, name FROM companies WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?;
    let (is_system, curr_name) = comp.ok_or(AppError::NotFound)?;
    if is_system && req.name.is_some() && req.name.as_deref() != Some(&curr_name) {
        return Err(AppError::BadRequest("系统内置公司名称不可修改".to_string()));
    }

    let new_name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let new_desc = req.description.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let result = sqlx::query(
        "UPDATE companies SET name=COALESCE($3, name), description=COALESCE($4, description) WHERE id=$1 AND user_id=$2"
    )
    .bind(id)
    .bind(user.0)
    .bind(new_name)
    .bind(new_desc)
    .execute(&state.pool)
    .await;

    match result {
        Ok(updated) => {
            if updated.rows_affected() == 0 {
                return Err(AppError::NotFound);
            }
            Ok(Json(json!({ "ok": true })))
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("unique") || msg.contains("duplicate key") {
                Err(AppError::Conflict("该公司名称已存在，请换一个名称".to_string()))
            } else {
                Err(AppError::from(e))
            }
        }
    }
}

// ---------- 岗位 ----------

/// 岗位卡片行：标题/地点/投递数/在招状态分布
#[derive(FromRow, Serialize)]
pub struct PositionRow {
    pub id: i64,
    pub title: String,
    /// 部门（岗位属性，ADR-0014 D1）
    pub department: Option<String>,
    pub location: Option<String>,
    pub jd_text: Option<String>,
    pub application_count: i64,
    /// 最新一份投递的状态（卡片角标）；无投递为 NULL
    pub latest_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

async fn positions_with_stats(
    pool: &sqlx::PgPool,
    uid: i64,
    company_id: Option<i64>,
) -> Result<Vec<PositionRow>, AppError> {
    let rows = sqlx::query_as::<_, PositionRow>(
        r#"
        SELECT p.id, p.title, p.department, p.location, p.jd_text, p.created_at,
          (SELECT count(*) FROM applications a WHERE a.position_id=p.id)::bigint AS application_count,
          (SELECT a.status FROM applications a WHERE a.position_id=p.id
             ORDER BY CASE a.status WHEN 'interviewing' THEN 0 WHEN 'applied' THEN 1 ELSE 2 END,
                      a.applied_at DESC LIMIT 1) AS latest_status
        FROM positions p
        WHERE p.user_id=$1 AND ($2::bigint IS NULL OR p.company_id=$2)
        ORDER BY p.created_at DESC, p.id DESC
        "#,
    )
    .bind(uid)
    .bind(company_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[derive(Deserialize)]
struct CreatePositionReq {
    pub title: Option<String>,
    /// 部门（岗位属性，ADR-0014 D1）
    pub department: Option<String>,
    pub location: Option<String>,
    pub jd_text: Option<String>,
}

/// 系统容器公司写保护（ADR-0014 §17）：业务 API 不得触达 is_system 公司
async fn ensure_business_company(pool: &sqlx::PgPool, uid: i64, company_id: i64) -> Result<(), AppError> {
    let sys: Option<bool> =
        sqlx::query_scalar("SELECT is_system FROM companies WHERE id=$1 AND user_id=$2")
            .bind(company_id)
            .bind(uid)
            .fetch_optional(pool)
            .await?;
    match sys {
        None => Err(AppError::NotFound),
        Some(true) => Err(AppError::BadRequest("系统容器公司不可修改".to_string())),
        Some(false) => Ok(()),
    }
}

fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

#[tracing::instrument(skip_all)]
async fn create_position(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(company_id): Path<i64>,
    Json(req): Json<CreatePositionReq>,
) -> Result<impl IntoResponse, AppError> {
    // 系统容器写保护：不能往回收站/自录题库加岗位（ADR-0014 §17）
    ensure_business_company(&state.pool, user.0, company_id).await?;
    let title = clean(req.title.as_deref()).ok_or_else(|| AppError::BadRequest("岗位名称不能为空".to_string()))?;
    let id: i64 = match sqlx::query_scalar(
        "INSERT INTO positions(user_id, company_id, title, department, location, jd_text) VALUES($1,$2,$3,$4,$5,$6) RETURNING id",
    )
    .bind(user.0)
    .bind(company_id)
    .bind(&title)
    .bind(clean(req.department.as_deref()))
    .bind(clean(req.location.as_deref()))
    .bind(clean(req.jd_text.as_deref()))
    .fetch_one(&state.pool)
    .await
    {
        Ok(id) => id,
        Err(sqlx::Error::Database(e))
            if e.constraint().is_some_and(|c| c.contains("positions_user_company_title_key")) =>
        {
            return Err(AppError::Conflict("该公司已有同名岗位".to_string()))
        }
        Err(e) => return Err(e.into()),
    };
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[tracing::instrument(skip_all)]
async fn list_positions(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(company_id): Path<i64>,
) -> Result<Json<Vec<PositionRow>>, AppError> {
    let owned: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM companies WHERE id=$1 AND user_id=$2)")
            .bind(company_id)
            .bind(user.0)
            .fetch_one(&state.pool)
            .await?;
    if !owned {
        return Err(AppError::NotFound);
    }
    Ok(Json(positions_with_stats(&state.pool, user.0, Some(company_id)).await?))
}

#[derive(FromRow, Serialize)]
struct PositionDetail {
    pub id: i64,
    pub company_id: i64,
    pub company: String,
    pub title: String,
    /// 部门（岗位属性，ADR-0014 D1）
    pub department: Option<String>,
    pub location: Option<String>,
    pub jd_text: Option<String>,
    pub jd_interpret: Option<Value>,
    pub predict_result: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct PositionDetailView {
    #[serde(flatten)]
    pos: PositionDetail,
    /// 刷新恢复通道（ADR-0013 D3）：jd_interpret + position_predict
    pub ai_jobs: Vec<AiJob>,
}

#[tracing::instrument(skip_all)]
async fn get_position(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<PositionDetailView>, AppError> {
    let pos = sqlx::query_as::<_, PositionDetail>(
        r#"
        SELECT p.id, p.company_id, c.name AS company, p.title, p.department, p.location,
               p.jd_text, p.jd_interpret, p.predict_result, p.created_at
        FROM positions p JOIN companies c ON c.id=p.company_id
        WHERE p.id=$1 AND p.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let mut ai_jobs = Vec::new();
    if let Some(j) = state.ai_jobs.running_for(user.0, "jd_interpret", id) {
        ai_jobs.push(j);
    }
    if let Some(j) = state.ai_jobs.running_for(user.0, "position_predict", id) {
        ai_jobs.push(j);
    }
    Ok(Json(PositionDetailView { pos, ai_jobs }))
}

/// JD 解读归属岗位：同一岗位下所有投递共享。仅用户点击触发（AGENTS 基准 3）。
#[tracing::instrument(skip_all)]
async fn interpret_position_jd(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    start_position_jd_interpret(&state, user.0, id).await
}

pub async fn start_position_jd_interpret(
    state: &AppState,
    uid: i64,
    position_id: i64,
) -> Result<Json<Value>, AppError> {
    let jd: Option<Option<String>> = sqlx::query_scalar(
        "SELECT jd_text FROM positions WHERE id=$1 AND user_id=$2",
    )
    .bind(position_id)
    .bind(uid)
    .fetch_optional(&state.pool)
    .await?;
    let Some(jd_opt) = jd else {
        return Err(AppError::NotFound);
    };
    let jd = jd_opt.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        AppError::BadRequest("请先到岗位详情填写 JD 原文".to_string())
    })?;
    let config = settings::require_llm(&state.pool, uid).await?;
    let job = match state.ai_jobs.start(uid, "jd_interpret", position_id) {
        AiStart::AlreadyRunning(j) => return Ok(Json(json!({ "job_id": j.id, "status": j.status }))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let span = tracing::info_span!("llm.jd_interpret", model = %config.model, provider = %config.provider, position_id);
        let contract = crate::contracts::jd::JdInterpret::new(&jd);
        let (result, _meta) = contracts::execute(&config, &st.pool, uid, &contract)
            .instrument(span)
            .await?;
        let parsed = match result {
            contracts::ContractOut::Structured(v) => serde_json::to_value(v)?,
            contracts::ContractOut::Text(t) => json!({ "ir_mode": "text", "content": t }),
        };
        sqlx::query("UPDATE positions SET jd_interpret=$2 WHERE id=$1 AND user_id=$3")
            .bind(position_id)
            .bind(json!(parsed))
            .bind(uid)
            .execute(&st.pool)
            .await?;
        Ok::<_, AppError>(())
    });
    Ok(Json(json!({ "job_id": job.id, "status": job.status })))
}

#[derive(Deserialize)]
struct PatchPositionReq {
    pub title: Option<String>,
    /// 部门（岗位属性，ADR-0014 D1）
    pub department: Option<String>,
    pub location: Option<String>,
    pub jd_text: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn patch_position(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<PatchPositionReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let company_id: i64 = sqlx::query_scalar(
        "SELECT p.company_id FROM positions p WHERE p.id=$1 AND p.user_id=$2",
    )
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    ensure_business_company(&state.pool, user.0, company_id).await?;
    // 注意：location/jd_text 传空串即清空、缺省（None）不变；title 仅非空更新
    let updated = sqlx::query(
        r#"
        UPDATE positions SET
          title = COALESCE($3, title),
          department = COALESCE($4, department),
          location = COALESCE($5, location),
          jd_text = COALESCE($6, jd_text)
        WHERE id=$1 AND user_id=$2
        "#,
    )
    .bind(id)
    .bind(user.0)
    .bind(clean(req.title.as_deref()))
    .bind(clean(req.department.as_deref()))
    .bind(match req.location {
        Some(v) => Some(clean(Some(&v)).unwrap_or_default()),
        None => None,
    })
    .bind(match req.jd_text {
        Some(v) => Some(clean(Some(&v)).unwrap_or_default()),
        None => None,
    })
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

#[tracing::instrument(skip_all)]
async fn delete_position(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let company_id: i64 = sqlx::query_scalar(
        "SELECT p.company_id FROM positions p WHERE p.id=$1 AND p.user_id=$2",
    )
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    ensure_business_company(&state.pool, user.0, company_id).await?;
    let in_use: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM applications WHERE position_id=$1)",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if in_use {
        return Err(AppError::BadRequest(
            "该岗位下仍有投递记录，请先删除对应投递".to_string(),
        ));
    }
    let deleted = sqlx::query("DELETE FROM positions WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------- 岗位下的投递 ----------

#[derive(FromRow, Serialize)]
struct PositionApplicationRow {
    pub id: i64,
    pub status: String,
    pub channel: Option<String>,
    pub salary: Option<String>,
    pub note: Option<String>,
    pub applied_at: DateTime<Utc>,
    pub round_count: i64,
    pub latest_round_passed: Option<String>,
    /// 面试轮次进度（反馈 #10 节点时间线）：[{name, passed}]
    pub interview_stages: Option<Value>,
}

#[tracing::instrument(skip_all)]
async fn position_applications(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<PositionApplicationRow>>, AppError> {
    let owned: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM positions WHERE id=$1 AND user_id=$2)")
            .bind(id)
            .bind(user.0)
            .fetch_one(&state.pool)
            .await?;
    if !owned {
        return Err(AppError::NotFound);
    }
    let rows = sqlx::query_as::<_, PositionApplicationRow>(
        r#"
        SELECT a.id, a.status, a.channel, a.salary, a.note, a.applied_at,
          (SELECT count(*) FROM rounds r WHERE r.application_id=a.id)::bigint AS round_count,
          (SELECT r.passed FROM rounds r WHERE r.application_id=a.id
             ORDER BY r.sort_order DESC, r.id DESC LIMIT 1) AS latest_round_passed,
          (SELECT COALESCE(json_agg(json_build_object('name', r.name, 'passed', r.passed) ORDER BY r.sort_order, r.id), '[]'::json)
             FROM rounds r WHERE r.application_id=a.id) AS interview_stages
        FROM applications a
        WHERE a.position_id=$1 AND a.user_id=$2
        ORDER BY a.applied_at DESC, a.id DESC
        "#,
    )
    .bind(id)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

// ---------- v5 岗位精准押题与资产流转 (ADR-0017 §3.2) ----------

#[derive(Serialize)]
pub struct PositionPredictResponse {
    pub summary: String,
    pub questions: Vec<crate::contracts::jd::PredictedQuestionItem>,
    pub text_fallback: Option<String>,
}

fn clip_chars(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// POST /api/positions/:id/predict：结合 JD 与候选人背景预测高频考题。
/// 异步 AiJob（ADR-0013）：受理即返回，结果落 `positions.predict_result`，刷新可恢复。
#[tracing::instrument(skip_all)]
async fn predict_position_questions(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    #[derive(FromRow)]
    struct PosDetail {
        title: String,
        company_name: String,
        jd_text: Option<String>,
        jd_interpret: Option<Value>,
    }

    let pos: Option<PosDetail> = sqlx::query_as(
        r#"
        SELECT p.title, c.name AS company_name, p.jd_text, p.jd_interpret
        FROM positions p
        JOIN companies c ON c.id = p.company_id
        WHERE p.id = $1 AND p.user_id = $2
        "#,
    )
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;

    let Some(pos) = pos else {
        return Err(AppError::NotFound);
    };

    let jd_text = pos.jd_text.as_deref().unwrap_or("").trim();
    if jd_text.is_empty() {
        return Err(AppError::BadRequest("该岗位暂无 JD 描述，请先补充岗位职责与要求".to_string()));
    }

    let parsed: Option<Option<Value>> = sqlx::query_scalar(
        "SELECT parsed FROM resumes WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;
    let parsed = parsed.flatten().filter(|p| !p.is_null());

    let resume_bg = match parsed.as_ref() {
        Some(p) => format!(
            "【候选人背景】\n{}",
            crate::contracts::interview_prep::compact_parsed_resume(p)
        ),
        None => "【候选人背景】未解析简历，仅按岗位 JD 预测".to_string(),
    };

    let mut user_content = format!(
        "【目标岗位】\n公司：{}\n职位：{}\n\n【岗位 JD 描述】\n{}\n\n{}",
        pos.company_name,
        pos.title,
        clip_chars(jd_text, 4000),
        resume_bg
    );
    if let Some(ex) = pos
        .jd_interpret
        .as_ref()
        .and_then(crate::contracts::interview_prep::compact_jd_interpret)
    {
        user_content.push_str("\n\n【已有 JD 解读要点】\n");
        user_content.push_str(&ex);
    }

    let config = settings::require_llm(&state.pool, user.0).await?;
    let job = match state.ai_jobs.start(user.0, "position_predict", id) {
        AiStart::AlreadyRunning(j) => return Ok(Json(json!({ "job_id": j.id, "status": j.status }))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let span = tracing::info_span!(
            "llm.position_predict",
            model = %config.model,
            provider = %config.provider,
            position_id = id
        );
        let contract = PositionPredict::new(user_content);
        let (out, _meta) = contracts::execute(&config, &st.pool, uid, &contract)
            .instrument(span)
            .await?;
        let stored = match out {
            ContractOut::Structured(typed) => serde_json::to_value(PositionPredictResponse {
                summary: typed.summary,
                questions: typed.questions,
                text_fallback: None,
            })?,
            ContractOut::Text(text) => serde_json::to_value(PositionPredictResponse {
                summary: "（纯文本评审模式生成）".to_string(),
                questions: Vec::new(),
                text_fallback: Some(text),
            })?,
        };
        sqlx::query("UPDATE positions SET predict_result=$2 WHERE id=$1 AND user_id=$3")
            .bind(id)
            .bind(stored)
            .bind(uid)
            .execute(&st.pool)
            .await?;
        Ok::<_, AppError>(())
    });
    Ok(Json(json!({ "job_id": job.id, "status": job.status })))
}

#[derive(Deserialize)]
struct IngestPredictedReq {
    questions: Vec<crate::contracts::jd::PredictedQuestionItem>,
}

/// POST /api/positions/:id/predict/ingest：一键将预测考题沉淀流转入自录题库与复习队列
#[tracing::instrument(skip_all)]
async fn ingest_predicted_questions(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(pos_id): Path<i64>,
    Json(req): Json<IngestPredictedReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.questions.is_empty() {
        return Err(AppError::BadRequest("待入库题目列表不能为空".to_string()));
    }

    // 查岗位名称
    let pos_title: String = sqlx::query_scalar("SELECT title FROM positions WHERE id=$1 AND user_id=$2")
        .bind(pos_id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    // 获取自录题库固定轮次
    let round_id = crate::services::system_containers::ensure_self_round(&state.pool, user.0).await?;

    let mut created_ids = Vec::new();
    let mut created_tags = Vec::new();
    let mut tx = state.pool.begin().await?;

    for item in req.questions {
        let content = item.content.trim();
        if content.is_empty() {
            continue;
        }

        // 票03：押题入题库必须打 source='predicted' 并锁定 predicted_position_id，
        // 命中率端点据此聚合；同时计算 content_normalized 接入题目去重 SSOT。
        let qid: i64 = sqlx::query_scalar(
            "INSERT INTO questions (user_id, round_id, content, content_normalized, my_answer, source, predicted_position_id)
             VALUES ($1, $2, $3, normalize_question_content($3), NULL, 'predicted', $4)
             RETURNING id"
        )
        .bind(user.0)
        .bind(round_id)
        .bind(content)
        .bind(pos_id)
        .fetch_one(&mut *tx)
        .await?;

        // 自动加入待复习队列
        sqlx::query("INSERT INTO review_records(question_id) VALUES($1) ON CONFLICT (question_id) DO NOTHING")
            .bind(qid)
            .execute(&mut *tx)
            .await?;

        created_tags.push((qid, vec!["岗位押题".to_string(), item.category.clone(), pos_title.clone()]));
        created_ids.push(qid);
    }

    tx.commit().await?;

    // 提交后再插入标签并自动挂载技能树
    for (qid, tags) in created_tags {
        crate::routes::questions::attach_tags(&state.pool, user.0, qid, &tags).await?;
    }

    // 触发领域事件
    for qid in &created_ids {
        let _ = state.event_bus.dispatch(DomainEvent::RealQuestionCreated {
            user_id: user.0,
            question_id: *qid,
            round_id,
        }).await;
    }

    Ok(Json(json!({
        "ok": true,
        "created_count": created_ids.len(),
        "question_ids": created_ids
    })))
}

#[derive(Deserialize)]
struct DrillPredictedReq {
    title: Option<String>,
    questions: Vec<crate::contracts::jd::PredictedQuestionItem>,
}

/// POST /api/positions/:id/predict/drill：一键以预测考题创建针对性模拟练习（试卷模式）
#[tracing::instrument(skip_all)]
async fn create_predicted_drill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(pos_id): Path<i64>,
    Json(req): Json<DrillPredictedReq>,
) -> Result<impl IntoResponse, AppError> {
    if req.questions.is_empty() {
        return Err(AppError::BadRequest("练习题目不能为空".to_string()));
    }

    let pos: (String, String) = sqlx::query_as(
        "SELECT p.title, c.name FROM positions p JOIN companies c ON c.id=p.company_id WHERE p.id=$1 AND p.user_id=$2"
    )
    .bind(pos_id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let title = req.title.as_deref().unwrap_or("").trim();
    let title = if title.is_empty() {
        format!("{} · {} 岗位针对押题专项模考", pos.1, pos.0)
    } else {
        title.to_string()
    };

    let count = req.questions.len() as i32;
    let dossier_val = serde_json::json!({
        "summary": format!("针对 {} · {} 岗位的精准押题考点", pos.1, pos.0),
        "predicted_questions": req.questions
    });

    // 票 08：未指定人格的场次同样落「经典面试官」内置种子（每行 drills 都有归属）
    let classic_persona_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM interviewer_personas WHERE name='经典面试官' AND builtin AND deleted_at IS NULL",
    )
    .fetch_optional(&state.pool)
    .await?;

    let drill_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO drills (user_id, kind, title, position, target_questions, status, dossier, persona_id)
        VALUES ($1, 'interview', $2, $3, $4, 'ongoing', $5, $6)
        RETURNING id
        "#
    )
    .bind(user.0)
    .bind(&title)
    .bind(&pos.0)
    .bind(count)
    .bind(&dossier_val)
    .bind(classic_persona_id)
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "drill_id": drill_id }))))
}


// ---------- 公司高频考点画像（票04，服务端聚合） ----------

#[derive(Debug, serde::Serialize)]
pub struct TopicProfile {
    pub total_questions: i64,
    pub top_tags: Vec<TopicNameCount>,
    pub top_skills: Vec<TopicNameCount>,
    pub type_distribution: Vec<TopicTypeCount>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct TopicNameCount {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct TopicTypeCount {
    pub question_type: Option<String>,
    pub count: i64,
}

/// 公司题集合（三条归属链的并集）：
/// 主归属 round→application→position→company、多轮关联 question_rounds 同链、押题 predicted_position_id。
/// 每条聚合查询各自内嵌该 CTE（IMMUTABLE 语义下等价且免跨语句传递）。
macro_rules! company_qs_cte {
    () => {
        r#"
        WITH company_qs AS (
          SELECT DISTINCT q.id, q.question_type
          FROM questions q
          LEFT JOIN rounds r ON r.id=q.round_id
          LEFT JOIN applications a ON a.id=r.application_id
          LEFT JOIN positions p2 ON p2.id=a.position_id
          WHERE q.user_id=$1 AND q.parent_id IS NULL AND (
            p2.company_id = $2
            OR EXISTS(SELECT 1 FROM question_rounds qr
                      JOIN rounds r3 ON r3.id=qr.round_id
                      LEFT JOIN applications a3 ON a3.id=r3.application_id
                      LEFT JOIN positions p3 ON p3.id=a3.position_id
                      WHERE qr.question_id=q.id AND p3.company_id=$2)
            OR q.predicted_position_id IN (SELECT pp.id FROM positions pp WHERE pp.company_id=$2)
          )
        )
        "#
    };
}

#[tracing::instrument(skip_all)]
async fn company_topic_profile(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<TopicProfile>, AppError> {
    // 归属校验：公司必须属于当前用户（含 is_system 的 per-user 行）
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM companies WHERE id=$2 AND user_id=$1)",
    )
    .bind(user.0)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;
    if !owned {
        return Err(AppError::NotFound);
    }

    let cte = company_qs_cte!();

    let total: (i64,) = sqlx::query_as(&format!(
        "{cte} SELECT COUNT(*) FROM company_qs"
    ))
    .bind(user.0)
    .bind(id)
    .fetch_one(&state.pool)
    .await?;

    let top_tags: Vec<TopicNameCount> = sqlx::query_as(&format!(
        "{cte} SELECT t.name AS name, COUNT(DISTINCT cq.id) AS count \
         FROM company_qs cq JOIN question_tags qt ON qt.question_id=cq.id JOIN tags t ON t.id=qt.tag_id \
         GROUP BY t.name ORDER BY count DESC, t.name ASC LIMIT 8"
    ))
    .bind(user.0)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let top_skills: Vec<TopicNameCount> = sqlx::query_as(&format!(
        "{cte} SELECT s.name AS name, COUNT(DISTINCT cq.id) AS count \
         FROM company_qs cq JOIN question_skills qs ON qs.question_id=cq.id JOIN skills s ON s.id=qs.skill_id \
         GROUP BY s.name ORDER BY count DESC, s.name ASC LIMIT 8"
    ))
    .bind(user.0)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let type_dist: Vec<TopicTypeCount> = sqlx::query_as(&format!(
        "{cte} SELECT cq.question_type, COUNT(*) AS count \
         FROM company_qs cq GROUP BY cq.question_type ORDER BY count DESC, cq.question_type ASC NULLS LAST"
    ))
    .bind(user.0)
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(TopicProfile {
        total_questions: total.0,
        top_tags,
        top_skills,
        type_distribution: type_dist,
    }))
}
