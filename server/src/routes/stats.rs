//! v3 数据资产化（ADR-0009 §3，M2）：统计图表（综合分趋势/记忆率曲线）+ Timeline 面试旅程。

use axum::extract::{Extension, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::FromRow;

use crate::auth::CurrentUser;
use crate::error::AppError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/stats/trend", get(trend))
        .route("/stats/review-curve", get(review_curve))
        .route("/stats/timeline", get(timeline))
        .route("/stats/funnel", get(funnel))
        .route("/stats/fsrs-memory", get(fsrs_memory))
        .route("/stats/prediction-hit-rate", get(prediction_hit_rate))
        .route("/dashboard/activity", get(activity))
        .route("/stats/goal", get(goal).put(set_goal))
}

/// 求职台周投递目标（ADR-0011 R4.e）：目标存 per-user settings，进度 = 本周一以来投递数。
#[derive(serde::Serialize, sqlx::FromRow)]
struct GoalRow {
    pub weekly_target: i64,
    pub applied_this_week: i64,
}

#[tracing::instrument(skip_all)]
async fn goal(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let target: i64 = crate::settings::get(&state.pool, user.0, "weekly_application_goal")
        .await?
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let row = sqlx::query_as::<_, GoalRow>(
        r#"
        SELECT $2::bigint AS weekly_target,
               count(*) FILTER (WHERE a.applied_at >= date_trunc('week', now()))::bigint AS applied_this_week
        FROM applications a
        JOIN positions p ON p.id=a.position_id JOIN companies c ON c.id=p.company_id
        WHERE a.user_id=$1 AND NOT c.is_system
        "#,
    )
    .bind(user.0)
    .bind(target)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(json!({
        "weekly_target": row.weekly_target,
        "applied_this_week": row.applied_this_week,
    })))
}

#[derive(serde::Deserialize)]
struct GoalReq {
    weekly_target: i64,
}

#[tracing::instrument(skip_all)]
async fn set_goal(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<GoalReq>,
) -> Result<Json<Value>, AppError> {
    if !(0..=1000).contains(&req.weekly_target) {
        return Err(AppError::BadRequest("周目标需在 0-1000 之间".to_string()));
    }
    crate::settings::set(&state.pool, user.0, "weekly_application_goal", json!(req.weekly_target)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(FromRow, Serialize)]
struct ScorePoint {
    pub date: chrono::NaiveDate,
    pub avg_score: f64,
    pub count: i64,
}
use serde::Serialize;

#[derive(FromRow, Serialize)]
struct CompanyScore {
    pub company: String,
    pub avg_score: Option<f64>,
    pub count: i64,
}

/// 综合分趋势：按分析日期（最近 90 天）+ 按公司。数据源：analyses（真实 + AI 沉淀题统一口径）。
#[tracing::instrument(skip_all)]
async fn trend(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let by_date = sqlx::query_as::<_, ScorePoint>(
        r#"
        SELECT a.created_at::date AS date, avg(a.score)::float8 AS avg_score, count(*)::bigint AS count
        FROM analyses a
        JOIN questions q ON q.id = a.question_id
        JOIN rounds r ON r.id = q.round_id
        JOIN applications a2 ON a2.id = r.application_id
        JOIN positions p ON p.id = a2.position_id
        JOIN companies c ON c.id = p.company_id
        WHERE q.user_id = $1 AND a.score IS NOT NULL AND a.created_at >= now() - interval '90 days'
          AND NOT c.is_system
        GROUP BY a.created_at::date
        ORDER BY a.created_at::date
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    let by_company = sqlx::query_as::<_, CompanyScore>(
        r#"
        SELECT c.name AS company, avg(a.score)::float8 AS avg_score, count(*)::bigint AS count
        FROM analyses a
        JOIN questions q ON q.id = a.question_id
        JOIN rounds r ON r.id = q.round_id
        JOIN applications a2 ON a2.id = r.application_id
        JOIN positions p ON p.id = a2.position_id
        JOIN companies c ON c.id = p.company_id
        WHERE q.user_id = $1 AND a.score IS NOT NULL AND NOT c.is_system
        GROUP BY c.name
        ORDER BY count DESC
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({ "by_date": by_date, "by_company": by_company })))
}

#[derive(FromRow, Serialize)]
struct CurveDay {
    pub date: chrono::NaiveDate,
    pub remembered: i64,
    pub fuzzy: i64,
    pub forgot: i64,
}

/// 复习记忆率曲线：每日自评分布（最近 90 天）+ 累计分布 + 连续天数。
#[tracing::instrument(skip_all)]
async fn review_curve(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let daily = sqlx::query_as::<_, CurveDay>(
        r#"
        SELECT rr.last_reviewed_at::date AS date,
          count(*) FILTER (WHERE rr.last_result='remembered')::bigint AS remembered,
          count(*) FILTER (WHERE rr.last_result='fuzzy')::bigint AS fuzzy,
          count(*) FILTER (WHERE rr.last_result='forgot')::bigint AS forgot
        FROM review_records rr JOIN questions q ON q.id=rr.question_id
        WHERE q.user_id=$1 AND rr.last_reviewed_at IS NOT NULL AND rr.last_reviewed_at >= now() - interval '90 days'
        GROUP BY rr.last_reviewed_at::date
        ORDER BY rr.last_reviewed_at::date
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    let (remembered, fuzzy, forgot): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE rr.last_result='remembered'),
                count(*) FILTER (WHERE rr.last_result='fuzzy'),
                count(*) FILTER (WHERE rr.last_result='forgot')
         FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1",
    )
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    let dates: Vec<chrono::NaiveDate> = sqlx::query_scalar(
        "SELECT DISTINCT rr.last_reviewed_at::date FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_reviewed_at IS NOT NULL ORDER BY 1 DESC",
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "daily": daily,
        "totals": { "remembered": remembered, "fuzzy": fuzzy, "forgot": forgot },
        "streak_days": crate::routes::review::compute_streak(&dates),
    })))
}

/// 时间线 / 活动流（ADR-0010 R13 用户修订）：复习自评 / 训练 / 投递 / 真实批次轮次 / 积分购物，按时间倒序。
/// 是面向用户的「最近都做了什么」，不是审计日志（审查日志/系统安全不入时间线）。
/// 总览时间线卡与 /stats/timeline 共用同一逻辑。
#[tracing::instrument(skip_all)]
async fn timeline(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let items = activity_items(&state.pool, user.0).await?;
    Ok(Json(json!({ "items": items })))
}

/// 总览专用：时间线活动流（与 /stats/timeline 同源契约）
#[tracing::instrument(skip_all)]
async fn activity(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let items = activity_items(&state.pool, user.0).await?;
    Ok(Json(json!({ "items": items })))
}

/// 活动流数据源：今日复习完成 + 训练（finished）+ 投递 + 真实批次/轮次 + 积分购物（redemption），每条带 type + ts。
async fn activity_items(pool: &sqlx::PgPool, uid: i64) -> Result<Vec<Value>, AppError> {
    use sqlx::Row;
    let mut items: Vec<Value> = Vec::new();

    // 今日复习完成（points_ledger daily_goal，幂等每天一条）：当天到期队列清空时授予。
    // 不复述每张卡的复习（逐条会污染时间线），只留「今日复习完成」这一个里程碑事件。
    let daily = sqlx::query(
        r#"
        SELECT l.created_at AS ts, l.note,
               (SELECT count(*) FROM review_records r WHERE r.last_reviewed_at::date = l.note::date) AS cards
        FROM points_ledger l
        WHERE l.category='daily_goal' AND l.note IS NOT NULL
        ORDER BY l.created_at
        "#,
    )
    .fetch_all(pool)
    .await?;
    for row in daily {
        let ts: chrono::DateTime<chrono::Utc> = row.try_get("ts")?;
        let note: String = row.try_get("note")?;
        let cards: i64 = row.try_get("cards")?;
        items.push(json!({
            "type": "review_done", "ts": ts.to_rfc3339(), "date": ts.date_naive().to_string(),
            "title": "今日复习完成", "detail": format!("· {cards} 张卡"), "note": note,
        }));
    }

    // 训练（完成）
    let drills = sqlx::query(
        r#"
        SELECT d.finished_at AS ts, d.kind, d.title, d.score
        FROM drills d WHERE d.status='finished' AND d.finished_at IS NOT NULL
        ORDER BY d.finished_at
        "#,
    )
    .fetch_all(pool)
    .await?;
    for row in drills {
        let ts: chrono::DateTime<chrono::Utc> = row.try_get("ts")?;
        let kind: String = row.try_get("kind")?;
        let title: String = row.try_get("title")?;
        let score: Option<i32> = row.try_get("score")?;
        items.push(json!({
            "type": "drill", "ts": ts.to_rfc3339(), "date": ts.date_naive().to_string(),
            "kind": kind, "title": format!("陪练 · {title}"), "score": score,
        }));
    }

    // 积分购物 / 兑换（points_ledger 的 redemption 支出，用户可见的「购物」时刻）
    let redemptions = sqlx::query(
        r#"
        SELECT l.created_at AS ts, l.note AS item, l.amount
        FROM points_ledger l
        WHERE l.category='redemption'
        ORDER BY l.created_at
        "#,
    )
    .fetch_all(pool)
    .await?;
    for row in redemptions {
        let ts: chrono::DateTime<chrono::Utc> = row.try_get("ts")?;
        let item: Option<String> = row.try_get("item")?;
        let amount: i32 = row.try_get("amount")?;
        items.push(json!({
            "type": "point", "ts": ts.to_rfc3339(), "date": ts.date_naive().to_string(),
            "title": format!("积分购物 · {}", item.unwrap_or_else(|| "兑换".into())),
            "amount": amount, "detail": format!("−{} 分", amount.abs()),
        }));
    }

    // 投递
    let apps = sqlx::query(
        r#"
        SELECT a.applied_at AS ts, a.status, p.title AS position, a.channel, c.name AS company
        FROM applications a
        JOIN positions p ON p.id = a.position_id
        LEFT JOIN companies c ON c.id = p.company_id
        WHERE a.user_id = $1 AND NOT COALESCE(c.is_system, false)
        ORDER BY a.applied_at
        "#,
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;
    for row in apps {
        let ts: chrono::DateTime<chrono::Utc> = row.try_get("ts")?;
        let status: String = row.try_get("status")?;
        let position: Option<String> = row.try_get("position")?;
        let company: Option<String> = row.try_get("company")?;
        items.push(json!({
            "type": "application", "ts": ts.to_rfc3339(), "date": ts.date_naive().to_string(),
            "status": status,
            "title": format!("投递 · {}", position.unwrap_or_else(|| "未填岗位".into())),
            "company": company,
        }));
    }

    // 面试轮次（挂投递）
    let rounds = sqlx::query(
        r#"
        SELECT COALESCE(r.date::timestamptz, r.created_at) AS ts, r.name, r.passed, r.form,
               c.name AS company, p.title AS position
        FROM rounds r JOIN applications a ON a.id=r.application_id
        JOIN positions p ON p.id = a.position_id
        LEFT JOIN companies c ON c.id = p.company_id
        WHERE a.user_id = $1 AND NOT COALESCE(c.is_system, false)
        ORDER BY ts
        "#,
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;
    for row in rounds {
        let ts: chrono::DateTime<chrono::Utc> = row.try_get("ts")?;
        let name: String = row.try_get("name")?;
        let passed: String = row.try_get("passed")?;
        let form: Option<String> = row.try_get("form")?;
        let company: Option<String> = row.try_get("company")?;
        let position: Option<String> = row.try_get("position")?;
        items.push(json!({
            "type": "round", "ts": ts.to_rfc3339(), "date": ts.date_naive().to_string(),
            "passed": passed,
            "title": format!("面试 · {}", position.unwrap_or_else(|| "未填岗位".into())),
            "company": company, "detail": format!("{name}{}", form.map(|f| format!(" · {f}")).unwrap_or_default()),
        }));
    }

    items.sort_by(|a, b| b["ts"].as_str().cmp(&a["ts"].as_str()));
    Ok(items)
}

/// 求职漏斗 + 渠道效果（ADR-0009 §5 / plan M4）：
/// 漏斗按「达到过某阶段」近似（当前状态机器中的上限态）：
///   applied = 全部投递；interviewing = status∈(interviewing,offer)；
///   interviewing = status∈(interviewing,offer)；offer = status=offer。
/// 渠道效果按 channel 分组（interview_rate / offer_rate）。
#[tracing::instrument(skip_all)]
async fn funnel(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    // ADR-0014 §16：漏斗/渠道排除系统容器投递（回收站/自录题库）
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM applications a
         JOIN positions p ON p.id=a.position_id JOIN companies c ON c.id=p.company_id
         WHERE a.user_id=$1 AND NOT c.is_system",
    )
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    let interviewing_cnt: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM applications a
         JOIN positions p ON p.id=a.position_id JOIN companies c ON c.id=p.company_id
         WHERE a.user_id=$1 AND NOT c.is_system AND a.status IN ('interviewing','offer')",
    )
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    let offer: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM applications a
         JOIN positions p ON p.id=a.position_id JOIN companies c ON c.id=p.company_id
         WHERE a.user_id=$1 AND NOT c.is_system AND a.status='offer'",
    )
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    let stages = [("applied", total), ("interviewing", interviewing_cnt), ("offer", offer)];
    let mut conversion = Vec::new();
    for w in stages.windows(2) {
        let (from, from_c) = w[0];
        let (to, to_c) = w[1];
        let rate = if from_c > 0 {
            (to_c as f64 / from_c as f64 * 100.0).round()
        } else {
            0.0
        };
        conversion.push(json!({ "from": from, "to": to, "rate": rate }));
    }

    #[derive(FromRow, Serialize)]
    struct ChannelRow {
        pub channel: String,
        pub count: i64,
        pub interviewed: i64,
        pub offers: i64,
    }
    let channels = sqlx::query_as::<_, ChannelRow>(
        r#"
        SELECT COALESCE(NULLIF(trim(channel), ''), '未填渠道') AS channel,
               count(*)::bigint AS count,
               count(*) FILTER (WHERE status IN ('interviewing','offer'))::bigint AS interviewed,
               count(*) FILTER (WHERE status='offer')::bigint AS offers
        FROM applications a
        JOIN positions p ON p.id=a.position_id JOIN companies c ON c.id=p.company_id
        WHERE a.user_id=$1 AND NOT c.is_system
        GROUP BY 1
        ORDER BY count DESC, channel
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    let channels_json: Vec<Value> = channels
        .iter()
        .map(|c| {
            json!({
                "channel": c.channel,
                "count": c.count,
                "interviewed": c.interviewed,
                "offers": c.offers,
                "interview_rate": if c.count > 0 { (c.interviewed as f64 / c.count as f64 * 100.0).round() } else { 0.0 },
                "offer_rate": if c.count > 0 { (c.offers as f64 / c.count as f64 * 100.0).round() } else { 0.0 },
            })
        })
        .collect();
    Ok(Json(json!({
        "funnel": stages.iter().map(|(s, c)| json!({ "stage": s, "count": c })).collect::<Vec<_>>(),
        "conversion": conversion,
        "channels": channels_json,
    })))
}

#[derive(FromRow)]
#[allow(dead_code)]
struct ReviewItemRow {
    pub question_id: i64,
    pub review_count: i32,
    pub interval_days: i32,
    pub ease: f64,
    pub last_reviewed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub next_review_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_result: Option<String>,
}

/// FSRS 记忆保持率与未来复习压力预测（v5.5：真 FSRS 口径，ADR-0022 D1）。
/// 个人化权重拟合 + 遗忘曲线可提取性；响应新增 fitted 字段，其余键与 v5.2 兼容。
#[tracing::instrument(skip_all)]
async fn fsrs_memory(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    use crate::services::memory_model::{
        build_item, fit_weights, retrievability, FitResult, ReviewLog,
    };
    use std::collections::BTreeMap;

    let rows = sqlx::query_as::<_, ReviewItemRow>(
        r#"
        SELECT rr.question_id, rr.review_count, rr.interval_days, rr.ease,
               rr.last_reviewed_at, rr.next_review_at, rr.last_result
        FROM review_records rr
        JOIN questions q ON q.id=rr.question_id
        WHERE q.user_id=$1 AND q.parent_id IS NULL
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    // 逐次复习日志（时间升序），按卡分组
    let log_rows: Vec<(i64, i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        r#"
        SELECT rl.question_id, rl.rating, rl.reviewed_at
        FROM review_logs rl
        JOIN questions q ON q.id=rl.question_id
        WHERE q.user_id=$1 AND q.parent_id IS NULL
        ORDER BY rl.question_id, rl.reviewed_at ASC
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    let now = chrono::Utc::now();
    let mut logs_by_card: BTreeMap<i64, Vec<ReviewLog>> = BTreeMap::new();
    for (question_id, rating, reviewed_at) in &log_rows {
        if (1..=4).contains(rating) {
            let days_elapsed = (now - *reviewed_at).num_seconds() as f64 / 86_400.0;
            logs_by_card.entry(*question_id).or_default().push(ReviewLog {
                rating: *rating as u32,
                days_elapsed: days_elapsed.max(0.0),
            });
        }
    }

    // 拟合个人化权重（样本不足自动回退默认参数）
    let items: Vec<_> = logs_by_card.values().filter_map(|l| build_item(l)).collect();
    let FitResult { weights, fitted } = fit_weights(&items);
    tracing::debug!(
        fitted,
        cards = rows.len(),
        logs = log_rows.len(),
        "FSRS 记忆大盘计算完成"
    );

    let total_cards = rows.len();

    let mut solid = 0; // >= 90%
    let mut good = 0; // 70% - 90%
    let mut fading = 0; // 50% - 70%
    let mut risk = 0; // < 50%
    let mut retention_sum = 0.0;

    // 未来 7 天每日到期压力预测
    let mut due_next_7_days = vec![0; 7];

    for r in &rows {
        // 真 FSRS 遗忘曲线；无日志的存量卡（防御路径，正常已被迁移回填）退回旧幂律近似
        let r_val = match logs_by_card.get(&r.question_id).and_then(|l| build_item(l)) {
            Some(item) => {
                let elapsed = logs_by_card[&r.question_id]
                    .last()
                    .map(|l| l.days_elapsed)
                    .unwrap_or(0.0);
                retrievability(&weights, &item, elapsed)
                    .unwrap_or_else(|| legacy_power_law(r, &now))
            }
            None => legacy_power_law(r, &now),
        };

        retention_sum += r_val;

        if r_val >= 0.90 {
            solid += 1;
        } else if r_val >= 0.70 {
            good += 1;
        } else if r_val >= 0.50 {
            fading += 1;
        } else {
            risk += 1;
        }

        if let Some(next) = r.next_review_at {
            let diff_days = (next.date_naive() - now.date_naive()).num_days();
            if (0..7).contains(&diff_days) {
                due_next_7_days[diff_days as usize] += 1;
            }
        }
    }

    let avg_retention = if total_cards > 0 {
        ((retention_sum / total_cards as f64) * 100.0).round()
    } else {
        100.0
    };

    Ok(Json(json!({
        "total_cards": total_cards,
        "avg_retention": avg_retention,
        "distribution": {
            "solid": solid,
            "good": good,
            "fading": fading,
            "risk": risk,
        },
        "due_next_7_days": due_next_7_days,
        "fitted": fitted,
    })))
}

/// 旧幂律近似（v5.2 口径）：仅作为"有复习状态但无日志行"的防御性回退，
/// 正常数据经迁移回填后不会走到这里。
fn legacy_power_law(
    r: &ReviewItemRow,
    now: &chrono::DateTime<chrono::Utc>,
) -> f64 {
    if r.review_count == 0 {
        return 0.4;
    }
    let stability = (r.interval_days as f64).max(1.0) * (r.ease / 2.5);
    let elapsed_days = match r.last_reviewed_at {
        Some(last) => (*now - last).num_seconds() as f64 / 86_400.0,
        None => 0.0,
    };
    (1.0 + (elapsed_days / (9.0 * stability))).powf(-1.0)
}

/// 押题命中闭环度量（票03）：source='predicted' 的题目按 predicted_position_id 聚合，
/// hit = 该题最近一次自评为 remembered 的比例。空数据返回结构化零值与样本量，
/// 旧 ingest 的 source='manual' 抹除导致无法回溯——仅新产生的押题可统计（票面已记录）。
#[tracing::instrument(skip_all)]
async fn prediction_hit_rate(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    // 总量 + 分岗位（LEFT JOIN positions 保留岗位但 0 命中时样本量为 0）
    let total: (i64, i64) = sqlx::query_as(
        r#"
        WITH pred AS (
          SELECT q.id, q.predicted_position_id
          FROM questions q
          WHERE q.user_id=$1 AND q.source='predicted' AND q.parent_id IS NULL
        )
        SELECT
          COUNT(*) FILTER (WHERE rr.last_result='remembered') AS hits,
          COUNT(*) AS reviewed
        FROM pred p
        JOIN review_records rr ON rr.question_id=p.id AND rr.review_count > 0
        "#,
    )
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;

    let by_position: Vec<(Option<i64>, Option<String>, i64, i64, i64)> = sqlx::query_as(
        r#"
        WITH pred AS (
          SELECT q.id, q.predicted_position_id
          FROM questions q
          WHERE q.user_id=$1 AND q.source='predicted' AND q.parent_id IS NULL
        )
        SELECT
          p.predicted_position_id,
          pos.title,
          -- 注意用 COUNT(rr.*) 而非 COUNT(*)：LEFT JOIN 下未复习题不应计入样本
          COUNT(rr.question_id) FILTER (WHERE rr.last_result='remembered') AS hits,
          COUNT(rr.question_id) AS reviewed,
          (SELECT COUNT(*) FROM pred WHERE pred.predicted_position_id = p.predicted_position_id) AS total_predicted
        FROM pred p
        LEFT JOIN positions pos ON pos.id=p.predicted_position_id
        LEFT JOIN review_records rr ON rr.question_id=p.id AND rr.review_count > 0
        GROUP BY p.predicted_position_id, pos.title
        ORDER BY reviewed DESC, total_predicted DESC, pos.title NULLS LAST
        "#,
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    let to_rate = |hits: i64, reviewed: i64| -> f64 {
        if reviewed == 0 {
            0.0
        } else {
            (hits as f64 * 1000.0 / reviewed as f64).round() / 10.0 // 一位小数百分比
        }
    };
    let total_rate = to_rate(total.0, total.1);

    let by_position_json: Vec<Value> = by_position
        .into_iter()
        .map(|(pos_id, title, hits, reviewed, total_predicted)| {
            json!({
                "position_id": pos_id,
                "position_title": title,
                "predicted_count": total_predicted,
                "reviewed_count": reviewed,
                "hit_rate_percent": to_rate(hits, reviewed),
            })
        })
        .collect();

    Ok(Json(json!({
        "total": {
            "predicted_count": (by_position_json.iter().map(|b| b["predicted_count"].as_i64().unwrap_or(0)).sum::<i64>()),
            "reviewed_count": total.1,
            "hit_rate_percent": total_rate,
        },
        "by_position": by_position_json,
    })))
}
