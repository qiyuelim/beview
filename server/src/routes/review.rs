use std::convert::Infallible;

use axum::extract::{Extension, Path, State};
use axum::response::sse::{Event, Sse};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use futures_util::Stream;
use serde_json::{json, Value};

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::llm;
use crate::models::{ExplainReq, GradeReq, GradeResult, ReviewQueueItem, ReviewStats, WrongItem};
use crate::settings;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/review/queue", get(queue))
        .route("/review/wrong", get(wrong))
        .route("/review/stats", get(stats))
        .route("/review/reset", post(reset_all))
        .route("/review/{id}/grade", post(grade))
        .route("/review/{id}/relearn", post(relearn))
        .route("/review/{id}/explain", post(explain))
}

/// 今日队列：next_review_at <= 现在，按逾期升序（最久没复习的优先）
#[tracing::instrument(skip_all)]
async fn queue(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<ReviewQueueItem>>, AppError> {
    let rows = sqlx::query_as::<_, ReviewQueueItem>(
        r#"
        SELECT q.id AS question_id, q.content, q.my_answer, q.source,
               (SELECT a.difficulty FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS difficulty,
               (SELECT a.score FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS score,
               (SELECT a.ref_answer FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS ref_answer,
               (SELECT a.feedback FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS feedback,
               COALESCE(array_agg(t.name) FILTER (WHERE t.name IS NOT NULL), '{}') AS tags,
               (SELECT c.name FROM companies c JOIN positions p ON p.company_id=c.id JOIN applications a ON a.position_id=p.id JOIN rounds r ON r.application_id=a.id WHERE r.id=q.round_id) AS company,
               r.last_result, r.interval_days, r.next_review_at
        FROM review_records r
        JOIN questions q ON q.id = r.question_id
        LEFT JOIN question_tags qt ON qt.question_id = q.id
        LEFT JOIN tags t ON t.id = qt.tag_id
        WHERE q.user_id = $1 AND r.next_review_at <= now()
        GROUP BY q.id, r.last_result, r.interval_days, r.next_review_at
        ORDER BY r.next_review_at ASC
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// 错题本：自评忘了 ∪ 最新判分 < 60（动态聚合，不加字段）
#[tracing::instrument(skip_all)]
async fn wrong(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<WrongItem>>, AppError> {
    let rows = sqlx::query_as::<_, WrongItem>(
        r#"
        SELECT q.id AS question_id, q.content, q.my_answer, q.source,
               r.last_result, r.review_count,
               (SELECT a.score FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS score,
               (SELECT a.ref_answer FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1) AS ref_answer,
               COALESCE(array_agg(t.name) FILTER (WHERE t.name IS NOT NULL), '{}') AS tags,
               (SELECT c.name FROM companies c JOIN positions p ON p.company_id=c.id JOIN applications a ON a.position_id=p.id JOIN rounds r ON r.application_id=a.id WHERE r.id=q.round_id) AS company
        FROM questions q
        JOIN review_records r ON r.question_id = q.id
        LEFT JOIN question_tags qt ON qt.question_id = q.id
        LEFT JOIN tags t ON t.id = qt.tag_id
        WHERE q.user_id = $1
          AND (r.last_result = 'forgot'
           OR (SELECT a.score FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1) < 60)
        GROUP BY q.id, r.last_result, r.review_count
        ORDER BY (r.last_result='forgot') DESC, score ASC NULLS LAST
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// 复习统计：今日待复习/已完成、记忆分布、连续天数
#[tracing::instrument(skip_all)]
async fn stats(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<ReviewStats>, AppError> {
    let (due, done_today, remembered, fuzzy, forgot): (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.next_review_at <= now()),
          (SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_reviewed_at::date = CURRENT_DATE),
          (SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_result='remembered'),
          (SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_result='fuzzy'),
          (SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_result='forgot')
        "#,
    )
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT DISTINCT rr.last_reviewed_at::date FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_reviewed_at IS NOT NULL ORDER BY 1 DESC",
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(ReviewStats {
        due,
        done_today,
        remembered,
        fuzzy,
        forgot,
        streak_days: compute_streak(&dates),
    }))
}

pub fn compute_streak(dates: &[NaiveDate]) -> i64 {
    let today = Utc::now().date_naive();
    let mut expected = if dates.first().map(|d| *d == today).unwrap_or(false) {
        today
    } else {
        today - Duration::days(1)
    };
    let mut streak = 0i64;
    for d in dates {
        if *d == expected {
            streak += 1;
            expected -= Duration::days(1);
        } else if *d < expected {
            break;
        }
    }
    streak
}

/// 全部重置（可选维护操作）：所有复习卡间隔回到 1 天、立即到期
#[tracing::instrument(skip_all)]
async fn reset_all(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    sqlx::query(
        "UPDATE review_records rr SET interval_days=1, ease=2.5, next_review_at=now(), last_result=NULL
         FROM questions q WHERE q.id=rr.question_id AND q.user_id=$1",
    )
    .bind(user.0)
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// 自评三态 -> SRS 更新（ADR-0007）
#[tracing::instrument(skip_all)]
async fn grade(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<GradeReq>,
) -> Result<Json<GradeResult>, AppError> {
    if !matches!(req.result.as_str(), "remembered" | "fuzzy" | "forgot") {
        return Err(AppError::BadRequest("自评结果必须是 remembered/fuzzy/forgot".to_string()));
    }
    // 归属校验（题目必须属于当前用户）
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM questions WHERE id=$1 AND user_id=$2)")
        .bind(id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    if !owned {
        return Err(AppError::NotFound);
    }
    // 评审 P1 整改：确保行存在 + 调度更新在同一事务内（并发自评同题不再互相覆盖中间态）
    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO review_records(question_id) VALUES($1) ON CONFLICT (question_id) DO NOTHING")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let (ease, interval): (f64, i32) = sqlx::query_as(
        "SELECT ease, interval_days FROM review_records WHERE question_id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    let rating = crate::services::memory_model::fsrs_rating(&req.result)
        .ok_or_else(|| AppError::BadRequest("自评结果必须是 remembered/fuzzy/forgot".to_string()))?;
    sqlx::query("INSERT INTO review_logs(question_id, rating) VALUES($1, $2)")
        .bind(id)
        .bind(rating as i32)
        .execute(&mut *tx)
        .await?;

    let log_rows: Vec<(i32, DateTime<Utc>)> = sqlx::query_as(
        "SELECT rating, reviewed_at FROM review_logs WHERE question_id=$1 ORDER BY reviewed_at ASC",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;

    let now = Utc::now();
    let mut card_logs = Vec::new();
    for (r, t) in &log_rows {
        let elapsed = now.signed_duration_since(*t).num_seconds() as f64 / 86_400.0;
        card_logs.push(crate::services::memory_model::ReviewLog {
            rating: *r as u32,
            days_elapsed: elapsed.max(0.0),
        });
    }

    // 用户全量复习日志拟合权重（V6 M2 排程化：同源权重，不足门槛自动回退默认参数并 fitted=false）
    let all_user_logs: Vec<(i64, i32, DateTime<Utc>)> = sqlx::query_as(
        "SELECT rl.question_id, rl.rating, rl.reviewed_at \
         FROM review_logs rl \
         JOIN questions q ON q.id=rl.question_id \
         WHERE q.user_id=$1 \
         ORDER BY rl.question_id, rl.reviewed_at ASC",
    )
    .bind(user.0)
    .fetch_all(&mut *tx)
    .await?;

    let mut user_items_map: std::collections::BTreeMap<i64, Vec<crate::services::memory_model::ReviewLog>> =
        std::collections::BTreeMap::new();
    for (qid, r, t) in &all_user_logs {
        let elapsed = now.signed_duration_since(*t).num_seconds() as f64 / 86_400.0;
        user_items_map.entry(*qid).or_default().push(crate::services::memory_model::ReviewLog {
            rating: *r as u32,
            days_elapsed: elapsed.max(0.0),
        });
    }
    let items: Vec<_> = user_items_map
        .values()
        .filter_map(|l| crate::services::memory_model::build_item(l))
        .collect();
    let fit = crate::services::memory_model::fit_weights(&items);

    let days = crate::services::memory_model::schedule_next_interval(
        &fit.weights,
        &card_logs,
        interval,
        &req.result,
    );

    let ease2 = match req.result.as_str() {
        "remembered" => (ease + 0.15).min(4.0),
        "fuzzy" => ease,
        _ => (ease - 0.2).max(1.3),
    };
    let next = Utc::now() + Duration::days(days as i64);

    let (interval2, review_count): (i32, i32) = sqlx::query_as(
        r#"
        UPDATE review_records
        SET ease=$2, interval_days=$3, next_review_at=$4, last_result=$5,
            review_count=review_count+1, last_reviewed_at=now()
        WHERE question_id=$1
        RETURNING interval_days, review_count
        "#,
    )
    .bind(id)
    .bind(ease2)
    .bind(days)
    .bind(next)
    .bind(&req.result)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    // v5 事件总线：派发自评完成事件。副作用失败只记日志、不污染主流程——
    // 调度已提交，若此处报错用户重试会导致 interval 再次倍增（评审 P1：非幂等重试放大）。
    if let Err(e) = state.event_bus.dispatch(crate::events::DomainEvent::ReviewCardGraded {
        user_id: user.0,
        question_id: id,
        result: req.result.clone(),
        answer: req.answer.clone(),
    }).await {
        tracing::error!(error = %e, question_id = id, "复习积分/奖励结算失败（调度已生效，不影响本次复习）");
    }
    Ok(Json(GradeResult {
        last_result: req.result,
        ease: ease2,
        interval_days: interval2,
        next_review_at: next,
        review_count,
    }))
}

/// 重练：把该题重置为 1 天后再复习（错题本操作）
#[tracing::instrument(skip_all)]
async fn relearn(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let updated = sqlx::query(
        "UPDATE review_records rr SET interval_days=1, ease=2.5, next_review_at=now(), last_result=NULL
         FROM questions q WHERE q.id=rr.question_id AND rr.question_id=$1 AND q.user_id=$2",
    )
    .bind(id)
    .bind(user.0)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

/// AI 讲解（SSE 流式）：思路 + 口诀 + 变式题
#[tracing::instrument(skip_all)]
async fn explain(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<ExplainReq>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let (content, my_answer, ref_answer): (String, Option<String>, Option<String>) = sqlx::query_as(
        r#"
        SELECT q.content, q.my_answer,
               (SELECT a.ref_answer FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC, a.id DESC LIMIT 1)
        FROM questions q WHERE q.id=$1 AND q.user_id=$2
        "#,
    )
    .bind(id)
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    let config = settings::require_llm(&state.pool, user.0).await?;
    let messages = explain_messages(&content, my_answer.as_deref(), ref_answer.as_deref().unwrap_or(""), req.focus.as_deref());
    let stream = llm::stream_chat(config, messages, None); // AI 讲解为单轮出口，不链式

    let s = async_stream::stream! {
        use futures_util::StreamExt as _;
        let mut deltas = Box::pin(stream);
        while let Some(r) = deltas.next().await {
            let ev = match r {
                // 思考过程增量走独立 thinking 事件（前端可折叠展示，不混入讲解正文）
                Ok(llm::StreamItem::Content(d)) => {
                    Event::default().event("delta").data(json!({ "text": d }).to_string())
                }
                Ok(llm::StreamItem::Thinking(t)) => Event::default()
                    .event("thinking")
                    .data(json!({ "text": t }).to_string()),
                // 单轮出口不链式：忽略响应顶层 id（多轮链式见 drills::send_message）
                Ok(llm::StreamItem::Completed(_)) => continue,
                Err(e) => Event::default().event("error").data(json!({ "message": e.to_string() }).to_string()),
            };
            yield Ok::<Event, Infallible>(ev);
        }
        yield Ok(Event::default().event("done").data("{}"));
    };
    Ok(Sse::new(s).keep_alive(axum::response::sse::KeepAlive::new()))
}

fn explain_messages(content: &str, my_answer: Option<&str>, ref_answer: &str, focus: Option<&str>) -> Vec<Value> {
    let system = r#"你是资深面试讲解教练。针对一道面试题用中文给出：
1. 解题思路（拆解要点，分点）
2. 一句话记忆口诀
3. 一道相关变式题（帮加深巩固）
简洁、口语化、可直接执行。不要输出 JSON，直接 Markdown。"#;
    let mut user = format!("面试题：\n{content}\n");
    if let Some(a) = my_answer {
        if !a.trim().is_empty() {
            user += &format!("\n我的回答（参考，指出可改进处）：\n{a}\n");
        }
    }
    if !ref_answer.trim().is_empty() {
        user += &format!("\n参考答案：\n{ref_answer}\n");
    }
    if let Some(f) = focus {
        if !f.trim().is_empty() {
            user += &format!("\n重点讲解：{f}\n");
        }
    }
    vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ]
}
