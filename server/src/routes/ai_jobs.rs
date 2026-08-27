//! AI 任务查询 + SSE 事件流（ADR-0013）：
//! - GET /api/ai-jobs/{id}   轮询兜底通道（SSE 断线/刷新后恢复用）
//! - GET /api/events         per-user SSE：任务状态变化实时推送（前端 AiJobCenter 订阅）
//! 纪律不变：任务仅由用户点击触发的 POST 创建，这里只读。

use std::convert::Infallible;

use axum::extract::{Extension, Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::Stream;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::state::{AiEvent, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ai-jobs/{id}", get(get_job))
        .route("/events", get(events))
}

/// 轮询兜底：查单个任务状态（running 或最近终态；查无/非本人 → 404）
async fn get_job(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<u64>,
) -> Result<Json<crate::state::AiJob>, AppError> {
    let job = state
        .ai_jobs
        .get(id)
        .filter(|j| j.uid == user.0)
        .ok_or(AppError::NotFound)?;
    Ok(Json(job))
}

/// per-user SSE：订阅全局广播，只转发本用户事件；keep-alive 保活防代理断连
async fn events(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let uid = user.0;
    let rx = state.ai_jobs.subscribe();
    let stream = async_stream::stream! {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(AiEvent { uid: ev_uid, job_id, kind, target_id, status }) => {
                    if ev_uid != uid {
                        continue; // 只转发本用户事件
                    }
                    let data = serde_json::json!({
                        "job_id": job_id, "kind": kind, "target_id": target_id, "status": status,
                    });
                    yield Ok(Event::default().data(data.to_string()));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(uid, lagged = n, "ai events lagged");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}
