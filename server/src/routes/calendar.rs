//! v4 M5：ICS 日历订阅源（ADR-0011 R5：日历提醒 = ICS 订阅 + 求职台待办，不做邮件）。
//!
//! - `GET /api/calendar/token`：登录态取 per-user 订阅 token（settings key=`calendar_token`，
//!   懒生成、幂等）；`POST` 重新生成 = 吊销旧链接。
//! - `GET /api/calendar.ics?token=…`：日历 App 无法走 session cookie，故免 session、
//!   由本 handler 自行校验 token（require_auth 白名单）。token 只授予只读日历范围，泄露可重置吊销。
//! - 内容 = 面试轮次（`rounds.date`：未来全部 + 过去 30 天）+ 复习到期
//!   （14 天视野按天聚合，逾期并入今日）。无日期的待办（复盘改进项/未分析题）留在求职台今日待办。
//! - 形态：全天事件（`VALUE=DATE`，面试只精确到日期、复习按天聚合，避开时区泥潭）+
//!   稳定 UID（客户端去重/更新）+ RFC 5545 文本转义 + CRLF 行尾；无 RRULE/VTIMEZONE。

use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::state::AppState;

const SETTINGS_KEY: &str = "calendar_token";
const UID_DOMAIN: &str = "beview";
/// 面试轮次回看窗口（天）：未来全部 + 近 N 天历史
const ROUND_PAST_DAYS: i64 = 30;
/// 复习到期前瞻窗口（天）
const REVIEW_HORIZON_DAYS: i64 = 14;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/calendar/token", get(get_token).post(regenerate_token))
        .route("/calendar/events", get(calendar_events))
        .route("/calendar.ics", get(ics))
}

// ---------- 总览日历数据源（v4.2 M2，ADR-0015 D6） ----------

#[derive(Deserialize)]
struct CalendarEventsQuery {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// 面试轮次日历事件：窗口 = 未来全部 + 近 30 天（与 ICS 同口径），按日期升序。
/// 供总览页日历/时间线聚合；session 鉴权（与 /calendar/token 同层）。
#[tracing::instrument(skip_all)]
async fn calendar_events(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<CalendarEventsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows: Vec<(i64, String, NaiveDate, String, Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT r.id, r.name, r.date, r.passed, c.name, p.title
        FROM rounds r
        JOIN applications a ON a.id = r.application_id
        JOIN positions p ON p.id = a.position_id
        LEFT JOIN companies c ON c.id = p.company_id
        WHERE a.user_id = $1
          AND r.date IS NOT NULL
          AND r.date >= CURRENT_DATE - $2::int
          AND ($3::date IS NULL OR r.date >= $3)
          AND ($4::date IS NULL OR r.date <= $4)
        ORDER BY r.date ASC, r.id ASC
        "#,
    )
    .bind(user.0)
    .bind(ROUND_PAST_DAYS as i32)
    .bind(q.from)
    .bind(q.to)
    .fetch_all(&state.pool)
    .await?;

    let events: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, name, date, passed, company, position)| {
            json!({
                "kind": "round",
                "id": id,
                "name": name,
                "date": date.to_string(),
                "passed": passed,
                "company": company,
                "position": position,
            })
        })
        .collect();
    Ok(Json(json!({ "events": events })))
}

// ---------- token 管理 ----------

async fn load_token(pool: &sqlx::PgPool, uid: i64) -> Result<Option<String>, sqlx::Error> {
    Ok(crate::settings::get(pool, uid, SETTINGS_KEY)
        .await?
        .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(String::from)))
}

async fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[tracing::instrument(skip_all)]
async fn get_token(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = match load_token(&state.pool, user.0).await? {
        Some(t) => t,
        None => {
            let t = new_token().await;
            crate::settings::set(&state.pool, user.0, SETTINGS_KEY, json!({ "token": t })).await?;
            t
        }
    };
    Ok(Json(json!({ "token": token })))
}

/// 重新生成 = 吊销旧订阅链接（泄露/弃用场景）
#[tracing::instrument(skip_all)]
async fn regenerate_token(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = new_token().await;
    crate::settings::set(&state.pool, user.0, SETTINGS_KEY, json!({ "token": token })).await?;
    Ok(Json(json!({ "token": token })))
}

// ---------- ICS 订阅源 ----------

#[derive(Deserialize)]
struct IcsQuery {
    token: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn ics(State(state): State<AppState>, Query(q): Query<IcsQuery>) -> Result<Response, AppError> {
    let Some(token) = q.token.filter(|t| !t.is_empty()) else {
        return Err(AppError::Unauthorized);
    };
    // token -> user（settings 按 key 反查；个人工作台规模可接受）
    let uid: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM settings WHERE key=$1 AND value->>'token'=$2")
            .bind(SETTINGS_KEY)
            .bind(&token)
            .fetch_optional(&state.pool)
            .await?;
    let Some(uid) = uid else {
        return Err(AppError::Unauthorized);
    };

    let body = render_ics(&state.pool, uid).await?;
    Ok((
        [(header::CONTENT_TYPE, "text/calendar; charset=utf-8")],
        body,
    )
        .into_response())
}

type RoundRow = (i64, String, NaiveDate, String, Option<String>, Option<String>, Option<String>);

async fn render_ics(pool: &sqlx::PgPool, uid: i64) -> Result<String, AppError> {
    let today = Local::now().date_naive();

    // 1) 面试轮次：未来全部 + 近 30 天历史
    let rounds: Vec<RoundRow> = sqlx::query_as(
        r#"SELECT r.id, r.name, r.date, r.passed, c.name, p.title, r.form
           FROM rounds r
           JOIN applications a ON a.id = r.application_id
           JOIN positions p ON p.id = a.position_id
           LEFT JOIN companies c ON c.id = p.company_id
           WHERE a.user_id = $1 AND r.date IS NOT NULL AND r.date >= $2
           ORDER BY r.date, r.id"#,
    )
    .bind(uid)
    .bind(today - Duration::days(ROUND_PAST_DAYS))
    .fetch_all(pool)
    .await?;

    // 2) 复习到期：14 天视野内逐卡取调度时间，按本地日期聚合；逾期并入今日
    let due: Vec<(DateTime<Utc>,)> = sqlx::query_as(
        r#"SELECT rr.next_review_at FROM review_records rr
           JOIN questions q ON q.id = rr.question_id
           WHERE q.user_id = $1 AND rr.next_review_at < $2"#,
    )
    .bind(uid)
    .bind(Utc::now() + Duration::days(REVIEW_HORIZON_DAYS))
    .fetch_all(pool)
    .await?;
    let mut per_day: std::collections::BTreeMap<NaiveDate, i64> = Default::default();
    for (ts,) in due {
        let d = ts.with_timezone(&Local).date_naive().max(today);
        *per_day.entry(d).or_default() += 1;
    }

    // 3) 组装 VCALENDAR（CRLF 行尾；行折叠从简不实现——主流日历客户端均接受长行）
    let now = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut out = String::new();
    // RFC 5545 §3.1：内容行不得超过 75 字节，超长需折叠为 CRLF+单空格续行
    // （此前「从简不折叠」在部分严格解析器上会导入失败）。UTF-8 多字节字符不拆断。
    fn fold_ical(line: &str) -> String {
        const MAX: usize = 75; // 首行 75 字节；续行含前导空格后同样不超过 75
        if line.len() <= MAX {
            return line.to_string();
        }
        let b = line.as_bytes();
        let mut out = String::with_capacity(b.len() + b.len() / 64 + 8);
        let mut pos = 0usize;
        let mut budget = MAX;
        loop {
            let remaining = b.len() - pos;
            if remaining <= budget {
                out.push_str(&line[pos..]);
                break;
            }
            let mut cut = pos + budget;
            while cut > pos && (b[cut] & 0xC0) == 0x80 {
                cut -= 1; // 回退到 UTF-8 字符边界
            }
            if cut == pos {
                break; // 理论不可达：单字符超过预算
            }
            out.push_str(&line[pos..cut]);
            out.push_str("\r\n ");
            pos = cut;
            budget = MAX - 1; // 续行的前导空格计入 75
        }
        out
    }
    let line = |out: &mut String, s: String| {
        out.push_str(&fold_ical(&s));
        out.push_str("\r\n");
    };
    line(&mut out, "BEGIN:VCALENDAR".into());
    line(&mut out, "VERSION:2.0".into());
    line(&mut out, "PRODID:-//Beview//CN".into());
    line(&mut out, "CALSCALE:GREGORIAN".into());
    line(&mut out, "METHOD:PUBLISH".into());
    line(&mut out, format!("X-WR-CALNAME:{}", escape("求职工作台")));

    for (id, name, date, passed, company, position, form) in rounds {
        let status = match passed.as_str() {
            "pass" => " · 已通过",
            "fail" => " · 未通过",
            _ => "",
        };
        let mut parts: Vec<String> = vec![company.unwrap_or_else(|| "未命名公司".into())];
        if let Some(p) = position.as_deref().filter(|s| !s.is_empty()) {
            parts.push(p.to_string());
        }
        parts.push(name);
        let summary = format!("{}{status}", parts.join("·"));
        line(&mut out, "BEGIN:VEVENT".into());
        line(&mut out, format!("UID:round-{id}@{UID_DOMAIN}"));
        line(&mut out, format!("DTSTAMP:{now}"));
        line(&mut out, format!("DTSTART;VALUE=DATE:{}", date.format("%Y%m%d")));
        line(
            &mut out,
            format!("DTEND;VALUE=DATE:{}", (date + Duration::days(1)).format("%Y%m%d")),
        );
        line(&mut out, format!("SUMMARY:{}", escape(&summary)));
        if let Some(f) = form.as_deref().filter(|s| !s.is_empty()) {
            line(&mut out, format!("DESCRIPTION:{}", escape(&format!("形式：{f}"))));
        }
        line(&mut out, "END:VEVENT".into());
    }

    for (d, n) in per_day {
        line(&mut out, "BEGIN:VEVENT".into());
        line(&mut out, format!("UID:review-{}@{UID_DOMAIN}", d.format("%Y%m%d")));
        line(&mut out, format!("DTSTAMP:{now}"));
        line(&mut out, format!("DTSTART;VALUE=DATE:{}", d.format("%Y%m%d")));
        line(
            &mut out,
            format!("DTEND;VALUE=DATE:{}", (d + Duration::days(1)).format("%Y%m%d")),
        );
        line(&mut out, format!("SUMMARY:复习 {n} 张卡到期"));
        line(&mut out, "END:VEVENT".into());
    }

    line(&mut out, "END:VCALENDAR".into());
    Ok(out)
}

/// RFC 5545 §3.3.11 TEXT 转义：反斜杠/分号/逗号前加反斜杠，换行转 `\n`
fn escape(s: &str) -> String {
    s.replace('\r', "")
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}
