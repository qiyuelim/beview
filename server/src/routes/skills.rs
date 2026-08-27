//! v5 技能图谱与能力雷达路由层 (ADR-0017 §3.1)

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::services::skill_service::{self, SkillGraphData, SkillRow};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/skills/tree", get(get_tree))
        .route("/skills/matrix", get(get_matrix))
        .route("/skills/seed", post(seed_tree))
        .route("/skills/unmapped-tags", get(list_unmapped_tags))
        .route("/skills/ingest-tag", post(ingest_tag))
        .route("/skills/tags/cleanup/propose", post(cleanup_propose))
        .route("/skills/tags/cleanup/apply", post(cleanup_apply))
        .route("/skills", post(create_skill))
        .route("/skills/{id}", patch(update_skill).delete(delete_skill))
        .route("/skills/{id}/merge", post(merge_skill))
        .route("/questions/{id}/skills", get(get_question_skills).post(bind_question_skills))
}

#[tracing::instrument(skip_all)]
async fn get_matrix(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<skill_service::SkillMatrixData>, AppError> {
    let data = skill_service::get_capability_matrix(&state.pool, user.0).await?;
    Ok(Json(data))
}

#[tracing::instrument(skip_all)]
async fn list_unmapped_tags(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<skill_service::UnmappedTag>>, AppError> {
    let list = skill_service::get_unmapped_tags(&state.pool, user.0).await?;
    Ok(Json(list))
}

/// 标签聚合清洗·建议（用户裁决 3）：收集未建树自由标签 → LLM 归组建议（不直接改库）。
/// 结构必需出口：无结构化能力同步拒绝；无自由标签时 400 提示。
#[tracing::instrument(skip_all)]
async fn cleanup_propose(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tags = skill_service::get_unmapped_tags(&state.pool, user.0).await?;
    if tags.is_empty() {
        return Err(AppError::BadRequest("没有待清洗的自由标签".to_string()));
    }
    let config = crate::settings::require_llm(&state.pool, user.0).await?;
    let contract = crate::contracts::skills::TagCleanup::new(
        tags.into_iter().map(|t| (t.tag, t.question_count)).collect(),
    );
    let (result, _meta) = crate::contracts::execute(&config, &state.pool, user.0, &contract).await?;
    let groups = result.structured()?.groups;

    // 尝试将建议的 canonical 映射至既有技能树节点 (merge-to-skill 候选)
    let user_skills: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, name, path FROM skills WHERE user_id = $1"
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    let enhanced_groups: Vec<serde_json::Value> = groups
        .into_iter()
        .map(|g| {
            let matched_skill = user_skills.iter().find(|(_, name, path)| {
                name.eq_ignore_ascii_case(&g.canonical)
                    || path.ends_with(&format!("/{}", g.canonical.to_lowercase()))
                    || name.to_lowercase().contains(&g.canonical.to_lowercase())
                    || g.canonical.to_lowercase().contains(&name.to_lowercase())
            });
            json!({
                "canonical": g.canonical,
                "aliases": g.aliases,
                "note": g.note,
                "target_skill_id": matched_skill.map(|(id, _, _)| *id),
                "target_skill_name": matched_skill.map(|(_, name, _)| name.clone()),
            })
        })
        .collect();

    Ok(Json(json!({ "groups": enhanced_groups })))
}

#[derive(serde::Deserialize)]
struct CleanupApplyReq {
    /// 每组：canonical 规范名 + aliases 待并入别名 + target_skill_id 目标技能节点（可选）
    groups: Vec<CleanupGroup>,
}

#[derive(serde::Deserialize)]
struct CleanupGroup {
    canonical: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    target_skill_id: Option<i64>,
}

/// 标签聚合清洗·应用（人工确认后）：别名题目关联并入规范名，重挂至技能节点（merge-to-skill）
#[tracing::instrument(skip_all)]
async fn cleanup_apply(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CleanupApplyReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.groups.is_empty() {
        return Err(AppError::BadRequest("未选择任何清洗组".to_string()));
    }
    let parsed: Vec<skill_service::TagMergeGroupInput> = req
        .groups
        .into_iter()
        .map(|g| skill_service::TagMergeGroupInput {
            canonical: g.canonical,
            aliases: g.aliases,
            target_skill_id: g.target_skill_id,
        })
        .collect();
    let r = skill_service::apply_tag_merges(&state.pool, user.0, &parsed).await?;
    Ok(Json(json!({ "ok": true, "remapped": r.remapped, "removed_tags": r.removed_tags })))
}

#[derive(serde::Deserialize)]
struct IngestTagReq {
    tag: String,
    parent_id: Option<i64>,
}

#[tracing::instrument(skip_all)]
async fn ingest_tag(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<IngestTagReq>,
) -> Result<impl IntoResponse, AppError> {
    let sid = skill_service::ingest_unmapped_tag(&state.pool, user.0, &req.tag, req.parent_id).await?;
    let data = skill_service::get_skill_graph(&state.pool, user.0).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": sid, "graph": data }))))
}

/// 获取技能树与雷达图数据（若首次进入为空则自动预置默认技能树）
#[tracing::instrument(skip_all)]
async fn get_tree(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<SkillGraphData>, AppError> {
    let data = skill_service::get_skill_graph(&state.pool, user.0).await?;
    Ok(Json(data))
}

/// 一键重置/补齐默认预置技能树（补齐缺失的标准大纲节点，保留自定义节点与题目挂靠）
#[tracing::instrument(skip_all)]
async fn seed_tree(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<impl IntoResponse, AppError> {
    skill_service::seed_default_skills(&state.pool, user.0).await?;
    let data = skill_service::get_skill_graph(&state.pool, user.0).await?;
    Ok((StatusCode::CREATED, Json(data)))
}

#[derive(serde::Deserialize)]
struct CreateSkillReq {
    name: String,
    parent_id: Option<i64>,
    icon: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn create_skill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateSkillReq>,
) -> Result<impl IntoResponse, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("技能名称不能为空".to_string()));
    }

    let Some(pid) = req.parent_id else {
        return Err(AppError::BadRequest("新建技能必须选择所属顶级领域或父级技能节点".to_string()));
    };

    let parent: Option<SkillRow> = sqlx::query_as(
        "SELECT id, user_id, parent_id, name, path, icon, visibility, created_at, updated_at FROM skills WHERE id=$1 AND user_id=$2"
    )
    .bind(pid)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;

    let Some(p) = parent else {
        return Err(AppError::BadRequest("父技能节点不存在".to_string()));
    };
    let path = format!("{}/{}", p.path.trim_end_matches('/'), name.to_lowercase().replace(' ', "-"));

    let id = skill_service::find_or_create_child(
        &state.pool,
        user.0,
        pid,
        name,
        &path,
        req.icon.as_deref().unwrap_or("TreeStructure"),
    )
    .await?;

    Ok((StatusCode::CREATED, Json(json!({ "id": id, "name": name, "path": path }))))
}

#[derive(serde::Deserialize)]
struct UpdateSkillReq {
    name: Option<String>,
    icon: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn update_skill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateSkillReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    let skill: Option<SkillRow> = sqlx::query_as("SELECT id, user_id, parent_id, name, path, icon, visibility, created_at, updated_at FROM skills WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?;

    let Some(s) = skill else {
        return Err(AppError::NotFound);
    };

    if s.parent_id.is_none() && skill_service::TOP_LEVEL_DOMAINS.iter().any(|(name, _, _)| *name == s.name) {
        return Err(AppError::BadRequest("系统顶级知识域不可直接重命名".to_string()));
    }

    let updated = sqlx::query(
        "UPDATE skills
         SET name = COALESCE($1, name),
             icon = COALESCE($2, icon),
             updated_at = now()
         WHERE id = $3 AND user_id = $4"
    )
    .bind(req.name.as_deref().map(|s| s.trim()))
    .bind(req.icon.as_deref())
    .bind(id)
    .bind(user.0)
    .execute(&state.pool)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(Json(json!({ "ok": true })))
}

#[tracing::instrument(skip_all)]
async fn delete_skill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let skill: Option<SkillRow> = sqlx::query_as("SELECT id, user_id, parent_id, name, path, icon, visibility, created_at, updated_at FROM skills WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?;

    let Some(s) = skill else {
        return Err(AppError::NotFound);
    };

    if s.parent_id.is_none() && skill_service::TOP_LEVEL_DOMAINS.iter().any(|(name, _, _)| *name == s.name) {
        return Err(AppError::BadRequest("系统顶级知识域不可删除".to_string()));
    }

    let deleted = sqlx::query("DELETE FROM skills WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;

    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(Json(json!({ "ok": true })))
}

#[derive(serde::Deserialize)]
struct MergeSkillReq {
    target_id: i64,
}

#[tracing::instrument(skip_all)]
async fn merge_skill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<MergeSkillReq>,
) -> Result<Json<skill_service::MergeSkillResult>, AppError> {
    let res = skill_service::merge_skill_node(&state.pool, user.0, id, req.target_id).await?;
    Ok(Json(res))
}

#[derive(serde::Deserialize)]
struct BindSkillsReq {
    skill_ids: Vec<i64>,
}

/// 绑定题目与技能关联（全量替换并同步主列，遵循 SSOT 契约）
#[tracing::instrument(skip_all)]
async fn bind_question_skills(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(qid): Path<i64>,
    Json(req): Json<BindSkillsReq>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 校验题目归属
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM questions WHERE id=$1 AND user_id=$2)")
        .bind(qid)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;

    if !exists {
        return Err(AppError::NotFound);
    }

    // 过滤出属于当前用户的合法 skill_ids
    let mut valid_sids: Vec<i64> = Vec::new();
    for sid in req.skill_ids {
        let skill_valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM skills WHERE id=$1 AND user_id=$2)")
            .bind(sid)
            .bind(user.0)
            .fetch_one(&state.pool)
            .await?;
        if skill_valid {
            valid_sids.push(sid);
        }
    }

    skill_service::sync_question_skills(&state.pool, qid, None, Some(&valid_sids)).await?;

    Ok(Json(json!({ "ok": true })))
}

/// 获取题目关联的所有技能列表
#[tracing::instrument(skip_all)]
async fn get_question_skills(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(qid): Path<i64>,
) -> Result<Json<Vec<SkillRow>>, AppError> {
    let rows: Vec<SkillRow> = sqlx::query_as(
        "SELECT s.id, s.user_id, s.parent_id, s.name, s.path, s.icon, s.visibility, s.created_at, s.updated_at
         FROM skills s
         JOIN question_skills qs ON qs.skill_id = s.id
         WHERE qs.question_id = $1 AND s.user_id = $2
         ORDER BY s.path ASC"
    )
    .bind(qid)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(rows))
}
