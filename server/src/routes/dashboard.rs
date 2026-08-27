use axum::extract::{Extension, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/dashboard", get(dashboard))
}

#[derive(FromRow, Serialize)]
pub struct Summary {
    pub companies: i64,
    pub sessions: i64,
    pub questions: i64,
    pub analyzed: i64,
    pub unanalyzed: i64,
    pub unanswered: i64,
    pub starred: i64,
    pub pending_rounds: i64,
    pub avg_score: Option<f64>,
    pub avg_difficulty: Option<f64>,
}

#[derive(FromRow, Serialize)]
pub struct LightQuestion {
    pub id: i64,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub company: Option<String>,
    pub session: Option<String>,
    pub round: Option<String>,
}

#[derive(FromRow, Serialize)]
pub struct PendingRound {
    pub id: i64,
    pub name: String,
    pub company: Option<String>,
    pub session: Option<String>,
}

#[derive(FromRow, Serialize)]
pub struct RecentAnalysis {
    pub id: i64,
    pub question_id: i64,
    pub content: String,
    pub score: Option<i32>,
    pub difficulty: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub company: Option<String>,
}

#[derive(FromRow, Serialize)]
pub struct TagCount {
    pub name: String,
    pub cnt: i64,
}

#[derive(FromRow, Serialize)]
pub struct RecentSession {
    pub id: i64,
    pub company: Option<String>,
    pub department: Option<String>,
    pub position: Option<String>,
    pub status: String,
    pub started_at: Option<chrono::NaiveDate>,
}

#[derive(Serialize)]
pub struct Dashboard {
    pub summary: Summary,
    pub unanswered: Vec<LightQuestion>,
    pub unanalyzed: Vec<LightQuestion>,
    pub pending_rounds: Vec<PendingRound>,
    pub recent_analyses: Vec<RecentAnalysis>,
    pub top_tags: Vec<TagCount>,
    pub recent_sessions: Vec<RecentSession>,
}

#[tracing::instrument(skip_all)]
async fn dashboard(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Dashboard>, AppError> {
    let uid = user.0;
    let summary = sqlx::query_as::<_, Summary>(
        r#"
        SELECT
          (SELECT count(*) FROM companies WHERE user_id=$1 AND NOT is_system)::bigint AS companies,
          (SELECT count(*) FROM applications a JOIN positions p ON p.id=a.position_id LEFT JOIN companies c ON c.id=p.company_id WHERE a.user_id=$1 AND NOT c.is_system)::bigint AS sessions,
          (SELECT count(*) FROM questions WHERE user_id=$1)::bigint AS questions,
          (SELECT count(*) FROM questions q WHERE q.user_id=$1 AND EXISTS(SELECT 1 FROM analyses a WHERE a.question_id=q.id))::bigint AS analyzed,
          (SELECT count(*) FROM questions q WHERE q.user_id=$1 AND q.parent_id IS NULL AND NOT EXISTS(SELECT 1 FROM analyses a WHERE a.question_id=q.id))::bigint AS unanalyzed,
          (SELECT count(*) FROM questions WHERE user_id=$1 AND (my_answer IS NULL OR my_answer=''))::bigint AS unanswered,
          (SELECT count(*) FROM questions WHERE user_id=$1 AND starred)::bigint AS starred,
          (SELECT count(*) FROM rounds r JOIN applications a ON a.id=r.application_id WHERE a.user_id=$1 AND r.passed='pending')::bigint AS pending_rounds,
          (SELECT avg(a.score)::float8 FROM analyses a JOIN questions q ON q.id=a.question_id WHERE q.user_id=$1) AS avg_score,
          (SELECT avg(a.difficulty)::float8 FROM analyses a JOIN questions q ON q.id=a.question_id WHERE q.user_id=$1) AS avg_difficulty
        "#,
    )
    .bind(uid)
    .fetch_one(&state.pool)
    .await?;

    let unanswered = sqlx::query_as::<_, LightQuestion>(
        r#"
        SELECT q.id, q.content, q.created_at, c.name AS company, p.department AS session, r.name AS round
        FROM questions q
        JOIN rounds r ON r.id=q.round_id JOIN applications a ON a.id=r.application_id
        JOIN positions p ON p.id=a.position_id LEFT JOIN companies c ON c.id=p.company_id
        WHERE q.user_id=$1 AND (q.my_answer IS NULL OR q.my_answer='')
        ORDER BY q.created_at DESC LIMIT 5
        "#,
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;

    let unanalyzed = sqlx::query_as::<_, LightQuestion>(
        r#"
        SELECT q.id, q.content, q.created_at,
               COALESCE(c.name, c2.name) AS company,
               COALESCE(p.department, s.department) AS session,
               r.name AS round
        FROM questions q
        LEFT JOIN rounds r ON r.id=q.round_id
        LEFT JOIN applications a ON a.id=r.application_id
        LEFT JOIN positions p ON p.id=a.position_id
        LEFT JOIN companies c ON c.id=p.company_id
        LEFT JOIN sessions s ON s.id=r.session_id AND r.application_id IS NULL
        LEFT JOIN companies c2 ON c2.id=s.company_id
        WHERE q.user_id=$1 AND q.parent_id IS NULL
          AND NOT EXISTS(SELECT 1 FROM analyses a WHERE a.question_id=q.id)
        ORDER BY q.created_at DESC LIMIT 5
        "#,
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;

    let pending_rounds = sqlx::query_as::<_, PendingRound>(
        r#"
        SELECT r.id, r.name, c.name AS company, p.department AS session
        FROM rounds r
        JOIN applications a ON a.id=r.application_id
        JOIN positions p ON p.id=a.position_id LEFT JOIN companies c ON c.id=p.company_id
        WHERE a.user_id=$1 AND r.passed='pending'
        ORDER BY r.created_at DESC LIMIT 5
        "#,
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;

    let recent_analyses = sqlx::query_as::<_, RecentAnalysis>(
        r#"
        SELECT * FROM (
          SELECT DISTINCT ON (a.question_id)
                 a.id, a.question_id, q.content, a.score, a.difficulty, a.created_at, c.name AS company
          FROM analyses a
          JOIN questions q ON q.id=a.question_id
          JOIN rounds r ON r.id=q.round_id JOIN applications ap ON ap.id=r.application_id
          JOIN positions pp ON pp.id=ap.position_id
          LEFT JOIN companies c ON c.id=pp.company_id
          WHERE q.user_id=$1
          ORDER BY a.question_id, a.created_at DESC, a.id DESC
        ) t
        ORDER BY t.created_at DESC
        LIMIT 5
        "#,
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;

    let top_tags = sqlx::query_as::<_, TagCount>(
        r#"
        SELECT t.name, count(*)::bigint AS cnt
        FROM tags t JOIN question_tags qt ON qt.tag_id=t.id
        WHERE t.user_id=$1
        GROUP BY t.name ORDER BY cnt DESC LIMIT 12
        "#,
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;

    let recent_sessions = sqlx::query_as::<_, RecentSession>(
        r#"
        SELECT a.id, c.name AS company, p.department, p.title AS position, a.status,
               a.applied_at::date AS started_at
        FROM applications a
        JOIN positions p ON p.id = a.position_id
        LEFT JOIN companies c ON c.id = p.company_id
        WHERE a.user_id=$1
        ORDER BY a.updated_at DESC LIMIT 5
        "#,
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(Dashboard {
        summary,
        unanswered,
        unanalyzed,
        pending_rounds,
        recent_analyses,
        top_tags,
        recent_sessions,
    }))
}
