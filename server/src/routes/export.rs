//! 全量数据导出（M3）：把当前用户的全部业务数据导出为一份 JSON 备份（含 Content-Disposition 附件头）。
//!
//! 评审整改：
//! - 表清单集中一处（EXPORT_TABLES），并由 tests/export.rs 的完整性测试保证
//!   「迁移里的业务表 ⊆ 导出清单」——此前 v4/v5 新增表（positions/sessions 等）静默漏导。
//! - 查询一律参数绑定（$1=uid），不再 format! 拼 SQL。
//! - settings（含加密 api_key）与 users（密码哈希）有意不导出；恢复流程需配合 `.master_key`
//!   备份（见 scripts/backup.sh）。

use axum::extract::{Extension, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::{Column, Row, TypeInfo};

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::AppState;

/// 导出键名 = 表名。新增业务表时在此登记（完整性测试会强制对齐迁移）。
/// 有意不导出：users（凭据）、settings（加密密钥材料，且 per-user 可重建）、_sqlx_migrations。
pub const EXPORT_TABLES: &[&str] = &[
    "companies",
    "interviewer_personas",
    "positions",
    "applications",
    "application_events",
    "sessions",
    "rounds",
    "questions",
    "question_tags",
    "question_answers",
    "question_rounds",
    "question_skills",
    "skills",
    "analyses",
    "comments",
    "tags",
    "review_records",
    "review_logs",
    "application_insights",
    "background_jobs",
    "drills",
    "drill_messages",
    "resumes",
    "mall_items",
    "points_ledger",
];

pub fn routes() -> Router<AppState> {
    Router::new().route("/export", get(export_all))
}

#[tracing::instrument(skip_all)]
async fn export_all(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<impl IntoResponse, AppError> {
    let uid = user.0;
    // 只导出当前用户的数据；每条查询 $1 = uid（参数绑定，绝不拼接）
    let mut data = serde_json::Map::new();
    data.insert("exported_at".into(), json!(Utc::now().to_rfc3339()));
    data.insert("version".into(), json!(4));
    for table in EXPORT_TABLES {
        let sql = match *table {
            // interviewer_personas：内置种子（builtin）+ 本人自定义；软删除行一并导出以保留历史标注
            "interviewer_personas" => {
                "SELECT * FROM interviewer_personas WHERE builtin OR owner_user_id=$1 ORDER BY id"
            }
            "positions" => "SELECT p.* FROM positions p WHERE p.user_id=$1 ORDER BY p.id",
            "companies" => "SELECT * FROM companies WHERE user_id=$1 ORDER BY id",
            "applications" => "SELECT * FROM applications WHERE user_id=$1 ORDER BY id",
            "application_events" => "SELECT * FROM application_events WHERE user_id=$1 ORDER BY id",
            "sessions" => "SELECT * FROM sessions WHERE user_id=$1 ORDER BY id",
            "rounds" => {
                "SELECT r.* FROM rounds r JOIN applications a ON a.id=r.application_id WHERE a.user_id=$1 ORDER BY r.id"
            }
            "questions" => "SELECT * FROM questions WHERE user_id=$1 ORDER BY id",
            "analyses" => {
                "SELECT a.* FROM analyses a JOIN questions q ON q.id=a.question_id WHERE q.user_id=$1 ORDER BY a.id"
            }
            "comments" => "SELECT * FROM comments WHERE user_id=$1 ORDER BY id",
            "tags" => "SELECT * FROM tags WHERE user_id=$1 ORDER BY id",
            "question_tags" => {
                "SELECT qt.* FROM question_tags qt JOIN questions q ON q.id=qt.question_id WHERE q.user_id=$1 ORDER BY qt.question_id, qt.tag_id"
            }
            "question_answers" => {
                "SELECT qa.* FROM question_answers qa JOIN questions q ON q.id=qa.question_id WHERE q.user_id=$1 ORDER BY qa.id"
            }
            "question_rounds" => {
                "SELECT qr.* FROM question_rounds qr JOIN questions q ON q.id=qr.question_id WHERE q.user_id=$1 ORDER BY qr.question_id, qr.round_id"
            }
            "question_skills" => {
                "SELECT qs.* FROM question_skills qs JOIN questions q ON q.id=qs.question_id WHERE q.user_id=$1 ORDER BY qs.question_id, qs.skill_id"
            }
            "skills" => "SELECT * FROM skills WHERE user_id=$1 ORDER BY id",
            "review_records" => {
                "SELECT rr.* FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 ORDER BY rr.id"
            }
            "review_logs" => {
                "SELECT rl.* FROM review_logs rl JOIN questions q ON q.id=rl.question_id WHERE q.user_id=$1 ORDER BY rl.id"
            }
            "application_insights" => {
                "SELECT * FROM application_insights WHERE user_id=$1 ORDER BY id"
            }
            // background_jobs：进行中的任务不导出（瞬态数据），仅导出终态供审计回看
            "background_jobs" => {
                "SELECT * FROM background_jobs WHERE user_id=$1 AND status IN ('done','dead') ORDER BY id"
            }
            "drills" => "SELECT * FROM drills WHERE user_id=$1 ORDER BY id",
            "drill_messages" => "SELECT * FROM drill_messages WHERE user_id=$1 ORDER BY id",
            "resumes" => "SELECT * FROM resumes WHERE user_id=$1 ORDER BY id",
            "mall_items" => "SELECT * FROM mall_items WHERE user_id=$1 ORDER BY id",
            "points_ledger" => "SELECT * FROM points_ledger WHERE user_id=$1 ORDER BY id",
            other => return Err(AppError::BadRequest(format!("未登记导出 SQL 的表: {other}"))),
        };
        data.insert(
            (*table).to_string(),
            Value::Array(dump_table(&state.pool, sql, uid).await?),
        );
    }

    let payload = serde_json::to_vec_pretty(&Value::Object(data))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"beview_backup.json\"".to_string(),
            ),
        ],
        payload,
    ))
}

async fn dump_table(pool: &sqlx::PgPool, sql: &str, uid: i64) -> Result<Vec<Value>, AppError> {
    let rows = sqlx::query(sql).bind(uid).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let mut obj = serde_json::Map::new();
        for (i, col) in r.columns().iter().enumerate() {
            obj.insert(col.name().to_string(), row_value(&r, i));
        }
        out.push(Value::Object(obj));
    }
    Ok(out)
}

/// 把 sqlx::Row 的一列转为 Value。已知类型直取；未知类型按候选类型逐个尝试，
/// 全部失败则 warn 日志（评审整改：不再静默丢成 null）。
fn row_value(row: &sqlx::postgres::PgRow, i: usize) -> Value {
    let col = &row.columns()[i];
    match col.type_info().name() {
        "INT8" | "INT4" | "INT2" => match row.try_get::<i64, _>(i) {
            Ok(v) => json!(v),
            Err(_) => Value::Null,
        },
        "BOOL" => match row.try_get::<bool, _>(i) {
            Ok(v) => json!(v),
            Err(_) => Value::Null,
        },
        "FLOAT8" | "NUMERIC" | "FLOAT4" => match row.try_get::<f64, _>(i) {
            Ok(v) => json!(v),
            Err(_) => Value::Null,
        },
        "JSON" | "JSONB" => match row.try_get::<Value, _>(i) {
            Ok(v) => v,
            Err(_) => Value::Null,
        },
        "TIMESTAMPTZ" | "TIMESTAMP" => match row.try_get::<chrono::DateTime<chrono::Utc>, _>(i) {
            Ok(v) => json!(v.to_rfc3339()),
            Err(_) => Value::Null,
        },
        "DATE" => match row.try_get::<chrono::NaiveDate, _>(i) {
            Ok(v) => json!(v.to_string()),
            Err(_) => Value::Null,
        },
        "TEXT[]" | "VARCHAR[]" => match row.try_get::<Vec<String>, _>(i) {
            Ok(v) => json!(v),
            Err(_) => Value::Null,
        },
        _ => {
            // 未知类型兜底链：TEXT → JSON → 数值；仍失败则显式告警，不无声丢弃
            if let Ok(v) = row.try_get::<String, _>(i) {
                return json!(v);
            }
            if let Ok(v) = row.try_get::<Value, _>(i) {
                return v;
            }
            if let Ok(v) = row.try_get::<i64, _>(i) {
                return json!(v);
            }
            tracing::warn!(column = %col.name(), pg_type = %col.type_info().name(), "导出遇到未识别的列类型，已置 null");
            Value::Null
        }
    }
}
