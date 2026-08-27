//! v3 积分经济路由（ADR-0009）：余额 / 流水 / 今日任务。

use axum::extract::{Extension, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sqlx::FromRow;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::points;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/points/balance", get(balance))
        .route("/points/ledger", get(ledger))
        .route("/points/daily", get(daily))
}

#[derive(Deserialize, Default)]
struct LedgerQuery {
    pub limit: Option<i64>,
    /// 分页偏移（评审 P2：明细不再一次性平铺）
    pub offset: Option<i64>,
    pub category: Option<String>,
    /// 日期范围（含当天，created_at::date 比较）
    pub from: Option<chrono::NaiveDate>,
    pub to: Option<chrono::NaiveDate>,
}

use serde::Deserialize;

#[derive(FromRow, Serialize)]
pub struct LedgerEntry {
    pub id: i64,
    pub amount: i32,
    pub category: String,
    pub ref_type: Option<String>,
    pub ref_id: Option<i64>,
    pub note: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
use serde::Serialize;

#[tracing::instrument(skip_all)]
async fn balance(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<serde_json::Value>, AppError> {
    let bal = points::balance(&state.pool, user.0).await?;
    Ok(Json(json!({ "balance": bal })))
}

#[tracing::instrument(skip_all)]
async fn ledger(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<LedgerQuery>,
) -> Result<Json<Vec<LedgerEntry>>, AppError> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).clamp(0, i64::MAX);
    let rows = sqlx::query_as::<_, LedgerEntry>(
        r#"
        SELECT id, amount, category, ref_type, ref_id, note, created_at
        FROM points_ledger
        WHERE user_id=$1 AND ($2::text IS NULL OR category=$2)
          AND ($4::date IS NULL OR created_at::date >= $4)
          AND ($5::date IS NULL OR created_at::date <= $5)
        ORDER BY created_at DESC, id DESC
        LIMIT $3 OFFSET $6
        "#,
    )
    .bind(user.0)
    .bind(q.category)
    .bind(limit)
    .bind(q.from)
    .bind(q.to)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[tracing::instrument(skip_all)]
async fn daily(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<points::DailyProgress>, AppError> {
    let d = points::daily(&state.pool, user.0).await?;
    Ok(Json(d))
}
