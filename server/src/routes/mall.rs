//! v3 积分商城（ADR-0009）：自定义奖励目录 + 兑换（honor system 自授）。

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use sqlx::FromRow;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::points;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mall/items", get(list_items).post(create_item))
        .route("/mall/items/{id}", delete(delete_item))
        .route("/mall/items/{id}/redeem", post(redeem_item))
}

#[derive(FromRow, Serialize)]
pub struct MallItem {
    pub id: i64,
    pub name: String,
    pub cost: i32,
    pub emoji: String,
    pub sort_order: i32,
}
use serde::Serialize;

#[derive(Deserialize)]
struct CreateItemReq {
    pub name: String,
    pub cost: i32,
    pub emoji: Option<String>,
    pub sort_order: Option<i32>,
}
use serde::Deserialize;

#[tracing::instrument(skip_all)]
async fn list_items(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<MallItem>>, AppError> {
    let rows = sqlx::query_as::<_, MallItem>(
        "SELECT id, name, cost, emoji, sort_order FROM mall_items WHERE user_id=$1 ORDER BY sort_order, id",
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[tracing::instrument(skip_all)]
async fn create_item(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateItemReq>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("奖励名称不能为空".to_string()));
    }
    if req.cost <= 0 {
        return Err(AppError::BadRequest("积分成本必须为正".to_string()));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO mall_items(user_id, name, cost, emoji, sort_order) VALUES($1,$2,$3,$4,COALESCE($5,0)) RETURNING id",
    )
    .bind(user.0)
    .bind(name)
    .bind(req.cost)
    .bind(req.emoji.as_deref().unwrap_or("🎁"))
    .bind(req.sort_order)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[tracing::instrument(skip_all)]
async fn delete_item(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let deleted = sqlx::query("DELETE FROM mall_items WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

/// 兑换：余额够 -> 扣分 + 记流水
#[tracing::instrument(skip_all)]
async fn redeem_item(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (cost, remaining) = points::redeem(&state.pool, user.0, id).await?;
    Ok(Json(json!({ "cost": cost, "balance": remaining })))
}
