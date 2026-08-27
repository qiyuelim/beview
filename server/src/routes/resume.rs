//! 简历（ADR-0006 + ADR-0019）：保存原文 + AI 解析成结构化字段 + 复数版本管理与快照留档。
//! 简历拷打场景以 parsed 为背景板（见 drills.rs）。

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::CurrentUser;
use crate::contracts;
use crate::error::AppError;
use crate::models::{CreateSnapshotReq, ResumeListItem, ResumeView, SaveResumeReq};
use crate::settings;
use crate::state::{AiStart, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/resume", get(get_resume).put(save_resume))
        .route("/resumes", get(list_resumes))
        .route("/resumes/{id}", get(get_resume_by_id).delete(delete_resume))
        .route("/resumes/snapshot", post(create_snapshot))
        .route("/resumes/{id}/archive", post(archive_resume))
        .route("/resume/parse", post(parse_resume))
        .route("/resume/optimize/propose", post(optimize_propose))
        .route("/resume/optimize/apply", post(optimize_apply))
        .route("/resume/export/markdown", get(export_markdown))
}

const RESUME_VIEW_SELECT: &str = r#"
    SELECT id, name, version_name, is_archived, raw_text, parsed, is_active, updated_at
    FROM resumes
"#;

/// 获取当前活跃的工作副本（未留档，可编辑）
#[tracing::instrument(skip_all)]
async fn get_resume(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ResumeView>, AppError> {
    let mut row = sqlx::query_as::<_, ResumeView>(&format!(
        "{RESUME_VIEW_SELECT} WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1"
    ))
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(ResumeView {
        id: 0,
        name: "我的简历".to_string(),
        version_name: "工作副本".to_string(),
        is_archived: false,
        raw_text: String::new(),
        parsed: None,
        is_active: true,
        updated_at: chrono::Utc::now(),
        ai_jobs: Vec::new(),
    });
    // ADR-0013 D3：暴露 running 的简历解析任务（刷新恢复通道）
    if let Some(j) = state.ai_jobs.running_for(user.0, "resume_parse", row.id) {
        row.ai_jobs.push(j);
    }
    Ok(Json(row))
}

/// 获取当前用户全部简历版本列表（包含工作副本与留档快照）
#[tracing::instrument(skip_all)]
async fn list_resumes(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<ResumeListItem>>, AppError> {
    let rows = sqlx::query_as::<_, ResumeListItem>(
        r#"
        SELECT id, name, version_name, is_archived, is_active,
               length(raw_text)::bigint AS char_count,
               (parsed IS NOT NULL AND parsed != 'null'::jsonb) AS has_parsed,
               created_at, updated_at
        FROM resumes
        WHERE user_id = $1
        ORDER BY is_archived ASC, updated_at DESC
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// 按 ID 获取指定简历版本的完整内容（只读或详情展示）
#[tracing::instrument(skip_all)]
async fn get_resume_by_id(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<ResumeView>, AppError> {
    let mut row = sqlx::query_as::<_, ResumeView>(&format!(
        "{RESUME_VIEW_SELECT} WHERE id=$1 AND user_id=$2"
    ))
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if let Some(j) = state.ai_jobs.running_for(user.0, "resume_parse", row.id) {
        row.ai_jobs.push(j);
    }
    Ok(Json(row))
}

/// 保存简历到当前工作副本（仅当未归档时允许修改）
#[tracing::instrument(skip_all)]
async fn save_resume(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<SaveResumeReq>,
) -> Result<Json<Value>, AppError> {
    let raw = req.raw_text.trim().to_string();
    if raw.is_empty() {
        return Err(AppError::BadRequest("简历内容不能为空".to_string()));
    }
    let name = req.name.unwrap_or_else(|| "我的简历".to_string());
    let version_name = req.version_name.unwrap_or_else(|| "工作副本".to_string());
    let parsed = req.parsed;

    let existing: Option<(i64, bool)> = sqlx::query_as(
        "SELECT id, is_archived FROM resumes WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1"
    )
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;

    match existing {
        Some((id, is_archived)) => {
            if is_archived {
                return Err(AppError::BadRequest("已留档的简历版本只读，不可修改".to_string()));
            }
            if let Some(p) = parsed {
                sqlx::query(
                    "UPDATE resumes SET raw_text=$2, name=$3, version_name=$4, parsed=$5, updated_at=now() WHERE id=$1 AND user_id=$6"
                )
                .bind(id)
                .bind(&raw)
                .bind(&name)
                .bind(&version_name)
                .bind(json!(p))
                .bind(user.0)
                .execute(&state.pool)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE resumes SET raw_text=$2, name=$3, version_name=$4, updated_at=now() WHERE id=$1 AND user_id=$5"
                )
                .bind(id)
                .bind(&raw)
                .bind(&name)
                .bind(&version_name)
                .bind(user.0)
                .execute(&state.pool)
                .await?;
            }
        }
        None => {
            sqlx::query(
                "INSERT INTO resumes(user_id, name, version_name, raw_text, parsed, is_active, is_archived)
                 VALUES($1,$2,$3,$4,$5,true,false)"
            )
            .bind(user.0)
            .bind(&name)
            .bind(&version_name)
            .bind(&raw)
            .bind(parsed.map(|p| json!(p)))
            .execute(&state.pool)
            .await?;
        }
    }
    tracing::info!(user_id = user.0, "保存工作副本简历成功");
    Ok(Json(json!({ "ok": true })))
}

/// 创建当前工作副本的静态快照（留档归档）
#[tracing::instrument(skip_all)]
async fn create_snapshot(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateSnapshotReq>,
) -> Result<impl IntoResponse, AppError> {
    let current = sqlx::query_as::<_, ResumeView>(&format!(
        "{RESUME_VIEW_SELECT} WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1"
    ))
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::BadRequest("未找到可快照的工作副本，请先保存简历".to_string()))?;

    if current.raw_text.trim().is_empty() {
        return Err(AppError::BadRequest("当前工作副本内容为空，无法创建快照".to_string()));
    }

    let default_ver = format!("快照 · {}", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let version_name = req.version_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&default_ver);

    let snapshot_id: i64 = sqlx::query_scalar(
        "INSERT INTO resumes(user_id, name, version_name, raw_text, parsed, is_active, is_archived)
         VALUES($1, $2, $3, $4, $5, false, true) RETURNING id",
    )
    .bind(user.0)
    .bind(&current.name)
    .bind(version_name)
    .bind(&current.raw_text)
    .bind(current.parsed)
    .fetch_one(&state.pool)
    .await?;

    tracing::info!(user_id = user.0, snapshot_id = snapshot_id, version_name = %version_name, "创建简历快照成功");
    Ok((StatusCode::CREATED, Json(json!({ "id": snapshot_id, "version_name": version_name }))))
}

/// 归档当前版本并克隆一份新的工作副本
#[tracing::instrument(skip_all)]
async fn archive_resume(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let target = sqlx::query_as::<_, ResumeView>(&format!(
        "{RESUME_VIEW_SELECT} WHERE id=$1 AND user_id=$2"
    ))
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if target.is_archived {
        return Ok(Json(json!({ "ok": true, "message": "该版本已处于归档状态" })));
    }

    // 1. 标记当前为归档
    sqlx::query("UPDATE resumes SET is_archived=true, is_active=false, updated_at=now() WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;

    // 2. 检查是否仍有未归档的工作副本，若无则克隆一份新的工作副本
    let has_working: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM resumes WHERE user_id=$1 AND NOT is_archived)"
    )
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;

    if !has_working {
        sqlx::query(
            "INSERT INTO resumes(user_id, name, version_name, raw_text, parsed, is_active, is_archived)
             VALUES($1, $2, '工作副本', $3, $4, true, false)",
        )
        .bind(user.0)
        .bind(&target.name)
        .bind(&target.raw_text)
        .bind(target.parsed)
        .execute(&state.pool)
        .await?;
    }

    tracing::info!(user_id = user.0, resume_id = id, "归档简历版本成功");
    Ok(Json(json!({ "ok": true })))
}

/// 删除已归档的简历快照（禁止删除唯一的工作副本）
#[tracing::instrument(skip_all)]
async fn delete_resume(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let target = sqlx::query_as::<_, ResumeView>(&format!(
        "{RESUME_VIEW_SELECT} WHERE id=$1 AND user_id=$2"
    ))
    .bind(id)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if !target.is_archived {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM resumes WHERE user_id=$1")
            .bind(user.0)
            .fetch_one(&state.pool)
            .await?;
        if count <= 1 {
            return Err(AppError::BadRequest("不可删除当前唯一的简历副本，可清空内容或重新编辑".to_string()));
        }
    }

    sqlx::query("DELETE FROM resumes WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;

    tracing::info!(user_id = user.0, resume_id = id, "删除简历版本成功");
    Ok(Json(json!({ "ok": true })))
}

/// AI 把当前简历原文解析成结构化字段（personal/education/projects/skills/experience）
/// 解析前若工作副本已有解析产物，自动进行快照备份，确保历史数据不丢失
#[tracing::instrument(skip_all)]
async fn parse_resume(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    _body: Option<Json<Value>>,
) -> Result<Json<Value>, AppError> {
    let current = sqlx::query_as::<_, ResumeView>(&format!(
        "{RESUME_VIEW_SELECT} WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1"
    ))
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::BadRequest("请先保存简历原文再解析".to_string()))?;

    let raw = current.raw_text.trim().to_string();
    if raw.is_empty() {
        return Err(AppError::BadRequest("请先保存简历原文再解析".to_string()));
    }

    let config = settings::require_llm(&state.pool, user.0).await?;
    // 结构必需出口（ADR-0016 D3）：解析产物是结构化简历，无结构化能力即拒绝（受理前同步返回）
    if !config.structured_output {
        return Err(AppError::BadRequest(
            "当前模型未启用「结构化输出」能力，无法解析简历；请在设置中开启该能力位或更换模型".to_string(),
        ));
    }

    // 若当前工作副本已有非空解析产物，自动为用户备份一份快照
    if let Some(old_parsed) = &current.parsed {
        if !old_parsed.is_null() {
            let auto_ver = format!("解析前快照 · {}", chrono::Local::now().format("%m-%d %H:%M"));
            let _ = sqlx::query(
                "INSERT INTO resumes(user_id, name, version_name, raw_text, parsed, is_active, is_archived)
                 VALUES($1, $2, $3, $4, $5, false, true)",
            )
            .bind(user.0)
            .bind(&current.name)
            .bind(&auto_ver)
            .bind(&current.raw_text)
            .bind(old_parsed)
            .execute(&state.pool)
            .await;
        }
    }

    let rid = current.id;
    // ADR-0013 D2 任务化：同目标（该简历行）幂等去重；完成事件回显
    let job = match state.ai_jobs.start(user.0, "resume_parse", rid) {
        AiStart::AlreadyRunning(j) => return Ok(Json(json!({ "job_id": j.id, "status": j.status }))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    // panic 守卫统一收尾（评审 P0）；契约层：prompt/schema/能力位闸门/解析内聚在 ResumeParse
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let contract = crate::contracts::resume::ResumeParse::new(&raw);
        let (result, _meta) = contracts::execute(&config, &st.pool, uid, &contract).await?;
        let parsed = result.structured()?;
        sqlx::query(
            "UPDATE resumes SET parsed=$1, updated_at=now() WHERE id=$2 AND user_id=$3",
        )
        .bind(json!(parsed))
        .bind(rid)
        .bind(uid)
        .execute(&st.pool)
        .await?;
        drop(parsed);
        Ok::<_, AppError>(())
    });
    tracing::info!(user_id = user.0, resume_id = rid, job_id = job.id, "发起简历 AI 解析任务");
    Ok(Json(json!({ "job_id": job.id, "status": "running" })))
}

// ==================== AI 优化变更集（票05，ADR-0021） ====================

#[derive(Deserialize)]
struct OptimizeProposeReq {
    /// 用户优化意图（可选）：如"突出项目成果""针对后端岗位精简"
    pub intent: Option<String>,
}

#[derive(Deserialize)]
struct OptimizeApplyReq {
    /// 前端逐条采纳后的操作子集（每条与提案 verbatim 一致）
    pub changes: Vec<crate::contracts::resume::ResumeChange>,
}

/// 加载当前工作副本（必须存在且已有非空解析产物）
async fn require_parsed_working_copy(
    pool: &sqlx::PgPool,
    uid: i64,
) -> Result<ResumeView, AppError> {
    let current = sqlx::query_as::<_, ResumeView>(&format!(
        "{RESUME_VIEW_SELECT} WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1"
    ))
    .bind(uid)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::BadRequest("未找到工作副本简历".to_string()))?;
    match &current.parsed {
        Some(p) if !p.is_null() => Ok(current),
        _ => Err(AppError::BadRequest(
            "当前工作副本还没有结构化数据，请先保存原文并完成 AI 解析".to_string(),
        )),
    }
}

/// 变更集提案：同步执行（单次 LLM 往返、输出体量小），手动触发纪律不变。
#[tracing::instrument(skip_all)]
async fn optimize_propose(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<OptimizeProposeReq>,
) -> Result<Json<Value>, AppError> {
    let current = require_parsed_working_copy(&state.pool, user.0).await?;
    let config = settings::require_llm(&state.pool, user.0).await?;
    let parsed = current.parsed.clone().unwrap();
    let contract =
        crate::contracts::resume::ResumeChangeset::new(&parsed, req.intent.as_deref().unwrap_or(""));
    let (result, _meta) = contracts::execute(&config, &state.pool, user.0, &contract).await?;
    let proposal = result.structured()?;
    tracing::info!(
        event = "resume.changeset.proposed",
        user_id = user.0,
        resume_id = current.id,
        change_count = proposal.changes.len(),
        "生成简历优化变更集"
    );
    Ok(Json(serde_json::to_value(proposal)?))
}

/// 应用采纳子集：服务端逐条校验（旧值断言/白名单守卫）→ 自动快照兜底 → 落库。
#[tracing::instrument(skip_all)]
async fn optimize_apply(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<OptimizeApplyReq>,
) -> Result<Json<Value>, AppError> {
    let current = require_parsed_working_copy(&state.pool, user.0).await?;
    let parsed = current.parsed.clone().unwrap();

    let outcome = crate::contracts::resume::apply_changeset(&parsed, &req.changes)?;

    // ADR-0021 D2：应用前自动快照（兜底撤销），复用重解析前自动快照机制
    let auto_ver = format!("变更前快照 · {}", chrono::Local::now().format("%m-%d %H:%M"));
    sqlx::query(
        "INSERT INTO resumes(user_id, name, version_name, raw_text, parsed, is_active, is_archived)
         VALUES($1, $2, $3, $4, $5, false, true)",
    )
    .bind(user.0)
    .bind(&current.name)
    .bind(&auto_ver)
    .bind(&current.raw_text)
    .bind(&parsed)
    .execute(&state.pool)
    .await?;

    sqlx::query("UPDATE resumes SET parsed=$1, updated_at=now() WHERE id=$2 AND user_id=$3")
        .bind(json!(outcome.parsed))
        .bind(current.id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;

    tracing::info!(
        event = "resume.changeset.applied",
        user_id = user.0,
        resume_id = current.id,
        applied = outcome.applied,
        rejected = outcome.rejected.len(),
        "应用简历优化变更集"
    );
    Ok(Json(serde_json::to_value(outcome)?))
}

#[derive(Deserialize)]
struct ExportQuery {
    pub resume_id: Option<i64>,
}

/// 简历 Markdown 导出（中国简历标准分区；空分区跳过；支持按 resume_id 指定导出特定版本）。
#[tracing::instrument(skip_all)]
async fn export_markdown(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<ExportQuery>,
) -> Result<impl IntoResponse, AppError> {
    let row = if let Some(rid) = q.resume_id {
        sqlx::query_as::<_, ResumeView>(&format!(
            "{RESUME_VIEW_SELECT} WHERE id=$1 AND user_id=$2"
        ))
        .bind(rid)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?
    } else {
        sqlx::query_as::<_, ResumeView>(&format!(
            "{RESUME_VIEW_SELECT} WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1"
        ))
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?
    }
    .unwrap_or(ResumeView {
        id: 0,
        name: "我的简历".to_string(),
        version_name: "工作副本".to_string(),
        is_archived: false,
        raw_text: String::new(),
        parsed: None,
        is_active: true,
        updated_at: chrono::Utc::now(),
        ai_jobs: Vec::new(),
    });

    let md = render_markdown(row.parsed.as_ref());
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"resume.md\""),
        ],
        md,
    ))
}

/// parsed -> Markdown（中国简历惯例：标题/简介/联系行/求职意向行/各分区；空内容跳过）
fn render_markdown(parsed: Option<&Value>) -> String {
    let empty = Value::Null;
    let p = parsed.unwrap_or(&empty);
    let s = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let arr = |k: &str| p.get(k).and_then(|v| v.as_array()).cloned().unwrap_or_default();

    let mut out = String::new();

    let name = s("name");
    out.push_str(&format!("# {}\n", if name.is_empty() { "简历" } else { &name }));

    let summary = s("summary");
    if !summary.is_empty() {
        out.push_str(&format!("\n> {summary}\n"));
    }

    // 联系方式行：性别 ｜ 年龄 ｜ 电话 ｜ 邮箱 ｜ 城市 ｜ 工作年限 ｜ 政治面貌
    let mut contacts: Vec<String> = Vec::new();
    for key in ["gender", "age", "phone", "email", "city", "years", "political"] {
        let v = s(key);
        if !v.is_empty() {
            contacts.push(v);
        }
    }
    if !contacts.is_empty() {
        out.push_str(&format!("\n{}\n", contacts.join(" ｜ ")));
    }

    // 求职意向行：期望职位 · 期望城市 · 期望薪资
    let mut intent: Vec<String> = Vec::new();
    for key in ["intent_position", "intent_city", "intent_salary"] {
        let v = s(key);
        if !v.is_empty() {
            intent.push(v);
        }
    }
    if !intent.is_empty() {
        out.push_str(&format!("\n**求职意向**：{}\n", intent.join(" · ")));
    }

    let edu = arr("education");
    if !edu.is_empty() {
        out.push_str("\n## 教育经历\n");
        for e in &edu {
            let school = e.get("school").and_then(|v| v.as_str()).unwrap_or("").trim();
            let degree = e.get("degree").and_then(|v| v.as_str()).unwrap_or("").trim();
            let mut line = String::from("- ");
            if !school.is_empty() {
                line.push_str(school);
            }
            if !degree.is_empty() {
                if !school.is_empty() {
                    line.push_str(" · ");
                }
                line.push_str(degree);
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }

    let exp = arr("experience");
    if !exp.is_empty() {
        out.push_str("\n## 工作经历\n");
        for e in &exp {
            let company = e.get("company").and_then(|v| v.as_str()).unwrap_or("").trim();
            let title = e.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
            let period = e.get("period").and_then(|v| v.as_str()).unwrap_or("").trim();
            let mut head = String::from("### ");
            if !company.is_empty() {
                head.push_str(company);
            }
            if !title.is_empty() {
                if !company.is_empty() {
                    head.push_str(" · ");
                }
                head.push_str(title);
            }
            out.push_str(head.trim_end());
            out.push_str("  \n");
            if !period.is_empty() {
                out.push_str(period);
                out.push('\n');
            }
            let duties: Vec<String> = e.get("responsibilities").and_then(|v| v.as_array()).map(|a| {
                a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
            }).unwrap_or_default();
            if !duties.is_empty() {
                out.push_str("职责：\n");
                for d in duties {
                    out.push_str(&format!("- {d}\n"));
                }
            }
            let ach: Vec<String> = e.get("achievements").and_then(|v| v.as_array()).map(|a| {
                a.iter().filter_map(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
            }).unwrap_or_default();
            if !ach.is_empty() {
                out.push_str("业绩：\n");
                for d in ach {
                    out.push_str(&format!("- {d}\n"));
                }
            }
        }
    }

    let projects = arr("projects");
    if !projects.is_empty() {
        out.push_str("\n## 项目经历\n");
        for pr in &projects {
            let pname = pr.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let detail = pr.get("detail").and_then(|v| v.as_str()).unwrap_or("").trim();
            if !pname.is_empty() {
                out.push_str(&format!("### {pname}\n"));
            }
            if !detail.is_empty() {
                out.push_str(detail);
                out.push('\n');
            }
            out.push('\n');
        }
    }

    let skills: Vec<String> = arr("skills")
        .iter()
        .filter_map(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect();
    if !skills.is_empty() {
        out.push_str(&format!("\n## 技能特长\n{}\n", skills.join("、")));
    }

    let certs = arr("certificates");
    if !certs.is_empty() {
        out.push_str("\n## 证书荣誉\n");
        for c in &certs {
            let cname = c.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let cdate = c.get("date").and_then(|v| v.as_str()).unwrap_or("").trim();
            if cname.is_empty() && cdate.is_empty() {
                continue;
            }
            match (cname.is_empty(), cdate.is_empty()) {
                (false, false) => out.push_str(&format!("- {cname}（{cdate}）\n")),
                (false, true) => out.push_str(&format!("- {cname}\n")),
                (true, false) => out.push_str(&format!("- {cdate}\n")),
                (true, true) => {}
            }
        }
    }

    let self_eval = s("self_evaluation");
    if !self_eval.is_empty() {
        out.push_str(&format!("\n## 自我评价\n{self_eval}\n"));
    }

    let links = arr("links");
    if !links.is_empty() {
        out.push_str("\n## 相关链接\n");
        for l in &links {
            let label = l.get("label").and_then(|v| v.as_str()).unwrap_or("").trim();
            let url = l.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
            if label.is_empty() && url.is_empty() {
                continue;
            }
            if label.is_empty() {
                out.push_str(&format!("- {url}\n"));
            } else if url.is_empty() {
                out.push_str(&format!("- {label}\n"));
            } else {
                out.push_str(&format!("- [{label}]({url})\n"));
            }
        }
    }

    out
}
