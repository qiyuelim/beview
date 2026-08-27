use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde_json::json;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::models::{CommentRow, CreateCommentReq};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/questions/{id}/comments", get(list_question_comments).post(add_question_comment))
        .route("/sessions/{id}/comments", get(list_session_comments).post(add_session_comment))
        .route("/comments/{id}", delete(delete_comment))
}

#[tracing::instrument(skip_all)]
async fn list_question_comments(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<CommentRow>>, AppError> {
    let rows = sqlx::query_as::<_, CommentRow>(
        "SELECT id, body, created_at FROM comments WHERE question_id=$1 AND user_id=$2 ORDER BY created_at DESC, id DESC",
    )
    .bind(id)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[tracing::instrument(skip_all)]
async fn add_question_comment(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<CreateCommentReq>,
) -> Result<impl IntoResponse, AppError> {
    let body = req.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("评论不能为空".to_string()));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM questions WHERE id=$1 AND user_id=$2)")
        .bind(id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    if !exists {
        return Err(AppError::NotFound);
    }
    let cid: i64 = sqlx::query_scalar("INSERT INTO comments(user_id, question_id, body) VALUES($1,$2,$3) RETURNING id")
        .bind(user.0)
        .bind(id)
        .bind(body)
        .fetch_one(&state.pool)
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": cid }))))
}

#[tracing::instrument(skip_all)]
async fn list_session_comments(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<CommentRow>>, AppError> {
    let rows = sqlx::query_as::<_, CommentRow>(
        "SELECT id, body, created_at FROM comments WHERE session_id=$1 AND user_id=$2 ORDER BY created_at DESC, id DESC",
    )
    .bind(id)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[tracing::instrument(skip_all)]
async fn add_session_comment(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<CreateCommentReq>,
) -> Result<impl IntoResponse, AppError> {
    let body = req.body.trim();
    if body.is_empty() {
        return Err(AppError::BadRequest("评论不能为空".to_string()));
    }
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=$1 AND user_id=$2)")
        .bind(id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    if !exists {
        return Err(AppError::NotFound);
    }
    let cid: i64 = sqlx::query_scalar("INSERT INTO comments(user_id, session_id, body) VALUES($1,$2,$3) RETURNING id")
        .bind(user.0)
        .bind(id)
        .bind(body)
        .fetch_one(&state.pool)
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": cid }))))
}

#[tracing::instrument(skip_all)]
async fn delete_comment(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let deleted = sqlx::query("DELETE FROM comments WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}
