//! 面试官人格域（V6-M5a，ADR-0023 D1）：内置种子（不可删改）+ 用户自定义 CRUD。
//! 选人即选侧重（focus_tags 并入 persona，stages 写路径已退役）。
//! 删除语义：软删除——历史场次仍显示「已删除的面试官」（M5a 验收标准）。

use axum::extract::{Extension, Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::state::AppState;

/// 启动时幂等补种内置人格（迁移已建表但种子被清/缺的场景；ON CONFLICT 跳过）
pub async fn ensure_builtins(pool: &sqlx::PgPool) {
    // 内置种子按名称唯一（幂等重播种防重；对存量库以启动期 DDL 补建索引）
    let _ = sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS uq_personas_builtin_name ON interviewer_personas(name) WHERE builtin",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        r#"
        INSERT INTO interviewer_personas(owner_user_id, name, title, persona_prompt, difficulty_hint, temperature_hint, focus_tags, builtin)
        VALUES
        (NULL, '沉稳技术官', '资深后端架构师',
         E'你是一位沉稳内敛的资深后端架构师，语速平缓但问题扎实。你偏好从底层机制出发逐层深入：先确认候选人对基础数据结构与协议语义的理解，再推进到高并发与一致性权衡。你不接受模糊表述，会礼貌地要求候选人给出具体依据。',
         '注重原理深度与工程权衡', 0.35, ARRAY['系统设计','数据库','缓存'], true),
        (NULL, '犀利交叉官', '跨部门压力面试官',
         E'你是一位言辞犀利的交叉面考官，习惯连环追问并快速切换考点，专门检验候选人在压力下的思路稳定性。你会抓住回答中的矛盾点当场对质（contradiction），也常把问题抛到没准备过的边界场景。',
         '高压节奏 · 矛盾对质 · 快速切题', 0.60, ARRAY['场景设计','线上排障','项目深挖'], true),
        (NULL, '亲和HRBP', 'HR 业务伙伴',
         E'你是一位温和亲切的 HRBP，关注候选人的协作方式、成长动机与职业规划。你的提问开放而有层次，善于用追问帮助候选人展开行为面试的 STAR 结构，营造接近真实 HR 面的氛围。',
         '行为面试 · 协作与成长动机', 0.85, ARRAY['行为面','职业规划'], true),
        (NULL, '经典面试官', '通用技术面试',
         E'你是一位经验丰富的通用技术面试官，风格均衡而专业。你会根据候选人的岗位与方向灵活调整提问深度，既考察基础原理，也关注工程实践与系统思维。你的提问循序渐进，善于用追问帮助候选人展现真实水平。',
         '均衡全面 · 循序渐进', 0.5, ARRAY[]::text[], true)
        ON CONFLICT DO NOTHING
        "#,
    )
    .execute(pool)
    .await;
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/personas", get(list_personas).post(create_persona))
        .route("/personas/{id}", axum::routing::put(update_persona).delete(delete_persona))
}

#[derive(FromRow, Serialize)]
pub struct PersonaRow {
    pub id: i64,
    pub name: String,
    pub title: Option<String>,
    pub persona_prompt: String,
    pub difficulty_hint: Option<String>,
    pub temperature_hint: Option<f64>,
    pub focus_tags: Vec<String>,
    pub builtin: bool,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Deserialize)]
struct UpsertPersonaReq {
    name: String,
    #[serde(default)]
    title: Option<String>,
    persona_prompt: String,
    #[serde(default)]
    difficulty_hint: Option<String>,
    /// 0.3–0.9；越界由 DB CHECK 与 API 双重拒绝
    #[serde(default)]
    temperature_hint: Option<f64>,
    #[serde(default)]
    focus_tags: Vec<String>,
}

/// 列表：内置在前、自定义在后；附带累计带教场次（陪练页网格展示）
async fn list_personas(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query_as::<_, PersonaRow>(
        r#"
        SELECT id, owner_user_id, name, title, persona_prompt, difficulty_hint,
               temperature_hint::float8 AS temperature_hint, focus_tags, builtin, deleted_at
        FROM interviewer_personas
        WHERE deleted_at IS NULL AND (builtin OR owner_user_id=$1)
        ORDER BY builtin DESC, id ASC
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    let items: Vec<Value> = rows
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "title": p.title,
                "persona_prompt": p.persona_prompt,
                "difficulty_hint": p.difficulty_hint,
                "temperature_hint": p.temperature_hint,
                "focus_tags": p.focus_tags,
                "builtin": p.builtin,
            })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

/// 校验温度人格参数（0.3–0.9），越界显式报错（DB CHECK 之外的入口防线）
fn validate_temperature(t: Option<f64>) -> Result<(), AppError> {
    if let Some(v) = t {
        if !(0.3..=0.9).contains(&v) {
            return Err(AppError::BadRequest("temperature_hint 必须在 0.3–0.9 之间".into()));
        }
    }
    Ok(())
}

async fn create_persona(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<UpsertPersonaReq>,
) -> Result<Json<Value>, AppError> {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(AppError::BadRequest("persona 名称必填且不超过 100 字".into()));
    }
    if req.persona_prompt.trim().is_empty() {
        return Err(AppError::BadRequest("persona_prompt 不能为空".into()));
    }
    validate_temperature(req.temperature_hint)?;

    let id: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO interviewer_personas(owner_user_id, name, title, persona_prompt, difficulty_hint, temperature_hint, focus_tags)
         VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id",
    )
    .bind(user.0)
    .bind(name)
    .bind(req.title.as_deref())
    .bind(req.persona_prompt.trim())
    .bind(req.difficulty_hint.as_deref())
    .bind(req.temperature_hint)
    .bind(&req.focus_tags)
    .fetch_one(&state.pool)
    .await;

    match id {
        Ok(id) => Ok(Json(json!({ "id": id }))),
        Err(e) if e.to_string().contains("uq_personas_owner_name") => {
            Err(AppError::Conflict("同名自定义面试官已存在".into()))
        }
        Err(e) => Err(e.into()),
    }
}

/// 内置种子不可删改（ADR-0023 D1）；仅本人自定义可改
async fn load_owned_custom(pool: &sqlx::PgPool, uid: i64, id: i64) -> Result<(), AppError> {
    let row: Option<(bool, Option<i64>)> =
        sqlx::query_as("SELECT builtin, owner_user_id FROM interviewer_personas WHERE id=$1 AND deleted_at IS NULL")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    match row {
        None => Err(AppError::NotFound),
        Some((builtin, owner)) if builtin || owner != Some(uid) => Err(AppError::Forbidden),
        Some(_) => Ok(()),
    }
}

async fn update_persona(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpsertPersonaReq>,
) -> Result<Json<Value>, AppError> {
    load_owned_custom(&state.pool, user.0, id).await?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(AppError::BadRequest("persona 名称必填且不超过 100 字".into()));
    }
    validate_temperature(req.temperature_hint)?;
    sqlx::query(
        "UPDATE interviewer_personas SET name=$1, title=$2, persona_prompt=$3, difficulty_hint=$4, temperature_hint=$5, focus_tags=$6 WHERE id=$7",
    )
    .bind(name)
    .bind(req.title.as_deref())
    .bind(req.persona_prompt.trim())
    .bind(req.difficulty_hint.as_deref())
    .bind(req.temperature_hint)
    .bind(&req.focus_tags)
    .bind(id)
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// 软删除：drills.persona_id 保持指向，历史场次显示「已删除的面试官」（ADR-0023 D1 M5a 验收）
async fn delete_persona(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    load_owned_custom(&state.pool, user.0, id).await?;
    sqlx::query("UPDATE interviewer_personas SET deleted_at=now() WHERE id=$1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}
