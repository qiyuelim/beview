//! v3 批量分析（ADR-0009 §2）→ 票09 持久化队列化：
//! 受理即入 background_jobs（DB 为唯一真相源），dispatcher 认领执行；
//! 进度经 GET 轮询（读 DB），DELETE 取消（写 status=cancelled，执行侧逐题检查）。
//! 进程重启后 running 自动回 pending 续跑——「重启即清」成为历史。
//! 纪律（AGENTS 基准 3）：仅用户点击触发，每题复用 `run_analysis`（含 span + /metrics）。

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::routes::questions::run_analysis;
use crate::services::job_queue::{self};
use crate::settings;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/questions/batch-analyze", post(start_batch))
        .route(
            "/questions/batch-analyze/{id}",
            get(get_batch).delete(cancel_batch),
        )
}

#[derive(serde::Deserialize)]
struct StartBatchReq {
    pub ids: Vec<i64>,
    /// unanalyzed（默认，跳过已分析）| all（覆盖式重评全部选中）
    pub mode: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn start_batch(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<StartBatchReq>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    if req.ids.is_empty() {
        return Err(AppError::BadRequest("未选择任何题目".to_string()));
    }
    let uid = user.0;
    let mode = req.mode.as_deref().unwrap_or("unanalyzed");
    if !matches!(mode, "unanalyzed" | "all") {
        return Err(AppError::BadRequest(format!("非法 mode: {mode}（可选 unanalyzed/all）")));
    }
    let filter_unanalyzed = mode == "unanalyzed";
    let sql = if filter_unanalyzed {
        "SELECT q.id FROM questions q WHERE q.user_id = $2 AND q.id = ANY($1) \
         AND NOT EXISTS(SELECT 1 FROM analyses a WHERE a.question_id = q.id) ORDER BY q.id"
    } else {
        "SELECT q.id FROM questions q WHERE q.user_id = $2 AND q.id = ANY($1) ORDER BY q.id"
    };
    let ids: Vec<i64> = sqlx::query_scalar(sql)
        .bind(&req.ids)
        .bind(uid)
        .fetch_all(&state.pool)
        .await?;
    if ids.is_empty() {
        return Err(AppError::BadRequest("所选题目均已分析，无需批量分析".to_string()));
    }
    // 快速失败：模型未配置时受理前即报错
    settings::require_llm(&state.pool, uid).await?;

    // 票09：入持久化队列；job_id 即队列表行 id，GET/DELETE 均以 DB 为准
    let job_id = job_queue::enqueue(
        &state.pool,
        uid,
        "batch_analyze",
        &json!({ "ids": ids, "mode": mode, "total": ids.len() }),
        2,
    )
    .await?;

    // 广播批量任务开始事件（形状与旧进程内版本一致，前端零改动）
    state.ai_jobs.publish_event(crate::state::AiEvent {
        uid,
        job_id: job_id as u64,
        kind: "batch_analyze".into(),
        target_id: ids.len() as i64,
        status: "running".into(),
    });

    Ok((StatusCode::ACCEPTED, Json(json!({ "job_id": job_id }))))
}

/// dispatcher 入口（票09）：认领后的批量任务执行体。
/// 逐题并发 4；每题完成即回写 DB 进度并广播单题事件；取消态逐题检查（读 DB）。
pub(crate) async fn execute_batch_job(
    state: &AppState,
    job: &crate::services::job_queue::QueuedJob,
) -> Result<(), AppError> {
    use futures_util::stream::{self, StreamExt};

    let uid = job.user_id;
    let ids: Vec<i64> = job
        .payload
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();
    let config = settings::require_llm(&state.pool, uid).await?;

    stream::iter(ids.iter().copied())
        .for_each_concurrent(4, |qid| {
            let pool = state.pool.clone();
            let event_bus = state.event_bus.clone();
            let ai_jobs = state.ai_jobs.clone();
            let config = config.clone();
            let job_id = job.id;
            let uid = uid;

            async move {
                // 取消检查：读 DB 权威状态
                let cancelled: bool = sqlx::query_scalar(
                    "SELECT (status='cancelled') FROM background_jobs WHERE id=$1",
                )
                .bind(job_id)
                .fetch_one(&pool)
                .await
                .unwrap_or(false);
                if cancelled {
                    return;
                }

                let result = run_batch_one(&pool, &event_bus, uid, &config, qid).await;
                let is_ok = result.is_ok();

                // 进度回写 DB（原子累加）
                sqlx::query(
                    r#"
                    UPDATE background_jobs SET progress = jsonb_set(jsonb_set(jsonb_set(
                        COALESCE(progress,'{}'::jsonb),
                        '{done}', (COALESCE((progress->>'done')::int,0)+1)::text::jsonb),
                        '{ok}',   (COALESCE((progress->>'ok')::int,0)  + CASE WHEN $2 THEN 1 ELSE 0 END)::text::jsonb),
                        '{failed}',(COALESCE((progress->>'failed')::int,0)+ CASE WHEN $2 THEN 0 ELSE 1 END)::text::jsonb)
                    WHERE id=$1
                    "#,
                )
                .bind(job_id)
                .bind(is_ok)
                .execute(&pool)
                .await
                .ok();

                ai_jobs.publish_event(crate::state::AiEvent {
                    uid,
                    job_id: job_id as u64,
                    kind: "batch_item_done".into(),
                    target_id: qid,
                    status: if is_ok { "done".into() } else { "failed".into() },
                });
            }
        })
        .await;

    // 检查最终是否被取消（读 DB 权威状态）
    let is_cancelled: bool = sqlx::query_scalar(
        "SELECT (status='cancelled') FROM background_jobs WHERE id=$1",
    )
    .bind(job.id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);

    // 广播批量分析终态事件（通知前端收尾，清空 analyzingIds，恢复按钮）
    state.ai_jobs.publish_event(crate::state::AiEvent {
        uid,
        job_id: job.id as u64,
        kind: "batch_analyze".into(),
        target_id: 0,
        status: if is_cancelled { "cancelled".into() } else { "done".into() },
    });

    Ok(())
}

/// 批量逐题分析：复用共享分析管线（LLM + span/指标 + analyses + 入复习队），另发批量积分。
async fn run_batch_one(
    pool: &sqlx::PgPool,
    event_bus: &crate::events::EventBus,
    uid: i64,
    config: &settings::LlmConfig,
    qid: i64,
) -> Result<(), AppError> {
    let q: (String, Option<String>) =
        sqlx::query_as("SELECT content, my_answer FROM questions WHERE id=$1 AND user_id=$2")
            .bind(qid)
            .bind(uid)
            .fetch_one(pool)
            .await?;
    run_analysis(pool, uid, qid, &q.0, q.1.as_deref(), config).await?;
    if let Some(a) = q.1.as_deref() {
        crate::routes::questions::record_answer(pool, qid, "manual", a).await?;
    }
    let _ = event_bus.dispatch(crate::events::DomainEvent::BatchAnalysisItemDone {
        user_id: uid,
        question_id: qid,
    }).await;
    Ok(())
}

#[tracing::instrument(skip_all)]
async fn get_batch(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let view = job_queue::get_view(&state.pool, user.0, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::to_value(view)?))
}

#[tracing::instrument(skip_all)]
async fn cancel_batch(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 不存在与不属于他人同样返回 404，不泄露任务存在性
    let cancelled = job_queue::cancel(&state.pool, id, user.0).await?;
    if !cancelled {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true, "status": "cancelling" })))
}
