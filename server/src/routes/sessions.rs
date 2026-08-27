//! v4 M3 重构：轮次直接挂投递（ADR-0011 UX 整改）——一个公司的一个岗位（投递）＝核心单元。
//! 「批次(session)」真实面试域退役：session 表仅作陪练沉淀容器内部保留。
//! 本文件 = 轮次 CRUD + 轮次复盘 + 投递整体分析。

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::CurrentUser;
use crate::contracts;
use crate::error::AppError;
use crate::state::{AiStart, AppState};
use tracing::Instrument;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/applications/{aid}/rounds", post(create_round))
        .route(
            "/rounds/{id}",
            get(get_round).patch(update_round).delete(delete_round),
        )
        .route("/rounds/{id}/detail", get(round_detail))
        .route(
            "/rounds/{id}/retrospective",
            get(get_retrospective).put(save_retrospective),
        )
        .route("/rounds/{id}/retrospective/ai", post(ai_retrospective))
        .route("/rounds/{id}/retrospective/to-review", post(retrospective_to_review))
        .route(
            "/applications/{aid}/overall-analysis",
            get(get_overall).put(save_overall),
        )
        .route("/applications/{aid}/overall-analysis/ai", post(ai_overall_analysis))
        .route("/rounds/all", get(list_all_rounds))
}

// ---------- 归属工具 ----------

/// 轮次归属校验（经 application.user_id），返回 (round_id, application_id)
async fn owned_round(pool: &sqlx::PgPool, uid: i64, round_id: i64) -> Result<(i64, i64), AppError> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT r.id, r.application_id FROM rounds r JOIN applications a ON a.id=r.application_id
         WHERE r.id=$1 AND a.user_id=$2 AND r.application_id IS NOT NULL",
    )
    .bind(round_id)
    .bind(uid)
    .fetch_optional(pool)
    .await?;
    row.ok_or(AppError::NotFound)
}

// ---------- 轮次 CRUD ----------

#[derive(serde::Deserialize)]
struct CreateRoundReq {
    pub name: Option<String>,
    pub date: Option<chrono::NaiveDate>,
    /// 形式：现场 / 视频 / 电话 / 其他
    pub form: Option<String>,
}

/// 添加面试（创建轮次）：必须关联投递（路径即投递）；名称缺省自动「一面/二面/…」递增。
/// 真实面试发生 = 主收益（+300，承接原 real_session 语义）。
#[tracing::instrument(skip_all)]
async fn create_round(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(aid): Path<i64>,
    Json(req): Json<CreateRoundReq>,
) -> Result<impl IntoResponse, AppError> {
    // 投递归属校验；终态（offer/rejected/withdrawn）不能再添加面试（反馈 #6）
    let row: Option<(bool, String)> = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM applications WHERE id=$1 AND user_id=$2), status \
         FROM applications WHERE id=$1 AND user_id=$2",
    )
    .bind(aid)
    .bind(user.0)
    .fetch_optional(&state.pool)
    .await?;
    let Some((owned, app_status)) = row else {
        return Err(AppError::NotFound);
    };
    if !owned {
        return Err(AppError::NotFound);
    }
    if matches!(app_status.as_str(), "offer" | "rejected" | "withdrawn") {
        return Err(AppError::BadRequest(
            "投递已终态，不能再添加面试".to_string(),
        ));
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM rounds WHERE application_id=$1")
        .bind(aid)
        .fetch_one(&state.pool)
        .await?;
    // 反馈七#2：上一面未出结果/未通过，不许添加下一面
    if n > 0 {
        let latest: Option<(String, String)> = sqlx::query_as(
            "SELECT COALESCE(passed,'pending'), COALESCE(name,'上一面') FROM rounds
             WHERE application_id=$1 ORDER BY sort_order DESC, id DESC LIMIT 1",
        )
        .bind(aid)
        .fetch_optional(&state.pool)
        .await?;
        if let Some((passed, name)) = latest {
            if passed == "pending" {
                return Err(AppError::BadRequest(format!(
                    "「{name}」还未标记结果，不能添加下一面"
                )));
            }
            if passed == "fail" {
                return Err(AppError::BadRequest(format!(
                    "「{name}」未通过，无法添加下一面；如需继续请先在轮次详情复核结果"
                )));
            }
        }
    }
    let default_name = format!("{}面", ['一', '二', '三', '四', '五']
        .get(n as usize)
        .copied()
        .unwrap_or('N'));
    let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&default_name);
    let form = req.form.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO rounds(application_id, session_id, name, sort_order, date, passed, form)
         VALUES($1, NULL, $2, $3, $4, 'pending', $5) RETURNING id",
    )
    .bind(aid)
    .bind(name)
    .bind((n + 1) as i32)
    .bind(req.date)
    .bind(form)
    .fetch_one(&state.pool)
    .await?;
    // 因果顺序：先推进投递状态并记流水，再派发轮次创建（阅读 DESC 时「添加面试」在上，自动推进为其因）
    if app_status == "applied" {
        crate::services::application_service::transition(
            &state.pool,
            user.0,
            aid,
            "interviewing",
            crate::services::application_service::TransitionSource::Auto,
        )
        .await?;
    }
    if let Err(e) = state.event_bus.dispatch(crate::events::DomainEvent::RealRoundCreated {
        user_id: user.0,
        application_id: aid,
        round_id: id,
        round_name: name.to_string(),
    }).await {
        tracing::error!(error = %e, application_id = aid, round_id = id, "真实面试积分发放失败（轮次已创建）");
    }
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "name": name }))))
}

#[derive(serde::Serialize, sqlx::FromRow)]
struct RoundFull {
    pub id: i64,
    pub application_id: i64,
    pub name: String,
    pub sort_order: i32,
    pub date: Option<chrono::NaiveDate>,
    pub passed: String,
    pub form: Option<String>,
    pub created_at: DateTime<Utc>,
    pub question_count: i64,
}

#[tracing::instrument(skip_all)]
async fn get_round(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<RoundFull>, AppError> {
    let (_, app_id) = owned_round(&state.pool, user.0, id).await?;
    let row = sqlx::query_as::<_, RoundFull>(
        r#"
        SELECT r.id, r.application_id, r.name, r.sort_order, r.date, r.passed, r.form, r.created_at,
          (SELECT count(*) FROM questions q WHERE q.round_id=r.id)::bigint AS question_count
        FROM rounds r WHERE r.id=$1
        "#,
    )
    .bind(id)
    .bind(app_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(row))
}

#[derive(serde::Deserialize)]
struct UpdateRoundReq {
    pub name: Option<String>,
    pub date: Option<chrono::NaiveDate>,
    pub form: Option<String>,
    pub passed: Option<String>, // pending / pass / fail
}

pub fn validate_passed(s: Option<&str>) -> Result<&'static str, AppError> {
    match s {
        None | Some("pending") => Ok("pending"),
        Some("pass") => Ok("pass"),
        Some("fail") => Ok("fail"),
        Some(other) => Err(AppError::BadRequest(format!("非法轮次结果: {other}"))),
    }
}

/// 改轮次信息/结果。结果标记通过时提示推进投递状态（application_hint，提示而非自动写）。
#[tracing::instrument(skip_all)]
async fn update_round(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRoundReq>,
) -> Result<Json<Value>, AppError> {
    let (round_id, _app_id) = owned_round(&state.pool, user.0, id).await?;
    let old_passed: Option<String> =
        sqlx::query_scalar("SELECT passed FROM rounds WHERE id=$1").bind(round_id).fetch_optional(&state.pool).await?;
    let passed = match req.passed.as_deref() {
        Some(s) => Some(validate_passed(Some(s))?.to_string()),
        None => None,
    };
    // B组 #4：结果选定后不可变更（非 pending 改其它值 400；同值幂等放行）。
    // 误触风险由前端内联确认流拦截：确认才落库，落库即锁定。
    if let (Some(old), Some(new)) = (old_passed.as_deref(), passed.as_deref()) {
        if old != "pending" && old != new {
            return Err(AppError::BadRequest(format!(
                "该轮结果已选定（{old}），不可变更"
            )));
        }
    }
    let updated = sqlx::query(
        "UPDATE rounds SET name=COALESCE($2, name), date=COALESCE($3, date),
         form=COALESCE($4, form), passed=COALESCE($5, passed) WHERE id=$1",
    )
    .bind(round_id)
    .bind(req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.date)
    .bind(req.form.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(passed.clone())
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    // 反馈六：轮次结果标记进状态流水（kind=round；锁定语义下仅 pending→结论 各一次）
    if let Some(new_passed) = passed.as_deref() {
        let label = match new_passed {
            "pass" => "通过",
            "fail" => "未通过",
            _ => new_passed,
        };
        let round_name: String =
            sqlx::query_scalar("SELECT COALESCE(name, '面试') FROM rounds WHERE id=$1")
                .bind(round_id)
                .fetch_one(&state.pool)
                .await?;
        if old_passed.as_deref() != Some("pass") && new_passed == "pass" {
            // v5 事件总线：派发轮次标记通过事件（流水记录 + 积分发放）；失败不影响标记结果
            if let Err(e) = state.event_bus.dispatch(crate::events::DomainEvent::RealRoundPassed {
                user_id: user.0,
                application_id: _app_id,
                round_id,
                round_name,
            }).await {
                tracing::error!(error = %e, round_id, "轮次通过积分发放失败（已标记通过）");
            }
        } else {
            crate::routes::applications::record_event_kind(
                &state.pool,
                user.0,
                _app_id,
                "round",
                None,
                "",
                "manual",
                Some(&format!("{round_name} · 标记{label}")),
            )
            .await?;
        }
    }
    // 轮次结果 -> 确认流提示（反馈 #5 修正：单轮通过 ≠ offer）。
    // 通过不给提示（前端内联提供「进入下一面 / 标记 Offer」二选一）；
    // 未通过且投递进行中 → 建议标记 rejected。
    let mut resp = json!({ "ok": true });
    if req.passed.as_deref() == Some("fail") {
        let row: Option<(i64, String)> = sqlx::query_as(
            "SELECT a.id, a.status FROM applications a
             JOIN rounds r ON r.application_id=a.id WHERE r.id=$1 AND a.user_id=$2",
        )
        .bind(round_id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?;
        if let Some((app_id, app_status)) = row {
            if app_status == "interviewing" {
                resp["application_hint"] = json!({
                    "application_id": app_id,
                    "current_status": app_status,
                    "suggested_status": "rejected",
                    "message": "将此投递标记为未通过？",
                });
            }
        }
    }
    Ok(Json(resp))
}

#[tracing::instrument(skip_all)]
async fn delete_round(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let (round_id, app_id) = owned_round(&state.pool, user.0, id).await?;
    // 反馈七#2：误创建的轮次才可删——已选定结果的轮次受锁定保护不可删
    let (passed, qcount): (String, i64) = sqlx::query_as(
        "SELECT COALESCE(passed,'pending'),
                (SELECT count(*) FROM questions WHERE round_id=r.id) FROM rounds r WHERE id=$1",
    )
    .bind(round_id)
    .fetch_one(&state.pool)
    .await?;
    if passed != "pending" {
        return Err(AppError::BadRequest(
            "该轮次结果已选定，不可删除".to_string(),
        ));
    }
    if qcount > 0 {
        return Err(AppError::BadRequest(format!(
            "该轮次下还有 {qcount} 道题目，请先处理后再删除"
        )));
    }
    sqlx::query("DELETE FROM rounds WHERE id=$1")
        .bind(round_id)
        .execute(&state.pool)
        .await?;
    crate::routes::applications::record_event_kind(
        &state.pool,
        user.0,
        app_id,
        "round",
        None,
        "",
        "manual",
        Some("删除了误创建的面试轮次"),
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// 全部轮次（题目关联下拉等），带投递上下文；支持 ?company= 按公司过滤（筛选修复）
#[derive(Deserialize)]
struct AllRoundsQuery {
    pub company: Option<i64>,
}

#[derive(serde::Serialize, sqlx::FromRow)]
struct AllRoundRow {
    pub round_id: i64,
    pub round_name: String,
    pub application_id: i64,
    pub department: Option<String>,
    pub position: Option<String>,
    pub company: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn list_all_rounds(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Query(q): Query<AllRoundsQuery>,
) -> Result<Json<Vec<AllRoundRow>>, AppError> {
    let rows = sqlx::query_as::<_, AllRoundRow>(
        r#"
        SELECT r.id AS round_id, r.name AS round_name, a.id AS application_id,
               p.department, p.title AS position, c.name AS company
        FROM rounds r
        JOIN applications a ON a.id = r.application_id
        JOIN positions p ON p.id = a.position_id
        LEFT JOIN companies c ON c.id = p.company_id
        WHERE a.user_id = $1 AND ($2::bigint IS NULL OR p.company_id = $2)
        ORDER BY COALESCE(r.date, r.created_at::date) DESC, r.id
        "#,
    )
    .bind(user.0)
    .bind(q.company)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

// ---------- 轮次复盘（每轮一份） ----------

/// 轮次子页聚合（反馈 #4）：轮次本体 + 所属投递上下文 + 逐题（含第一手真实回答）+ 复盘
#[derive(serde::Serialize, sqlx::FromRow)]
struct DetailQuestion {
    pub id: i64,
    pub content: String,
    pub my_answer: Option<String>,
    pub first_answer: Option<Value>,
    pub score: Option<i32>,
    pub feedback: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn round_detail(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let (round_id, app_id) = owned_round(&state.pool, user.0, id).await?;
    let round = sqlx::query_as::<_, RoundFull>(
        r#"
        SELECT r.id, r.application_id, r.name, r.sort_order, r.date, r.passed, r.form, r.created_at,
          (SELECT count(*) FROM questions q WHERE q.round_id=r.id)::bigint AS question_count
        FROM rounds r WHERE r.id=$1
        "#,
    )
    .bind(round_id)
    .fetch_one(&state.pool)
    .await?;

    let application: Option<(i64, Option<String>, Option<String>, i64, String)> = sqlx::query_as(
        r#"
        SELECT a.id, c.name AS company, p.title AS position, p.id AS position_id, a.status
        FROM applications a
        JOIN positions p ON p.id = a.position_id
        LEFT JOIN companies c ON c.id = p.company_id
        WHERE a.id=$1
        "#,
    )
    .bind(app_id)
    .fetch_optional(&state.pool)
    .await?;
    let application = application.map(
        |(id, company, position, position_id, status)| {
            json!({ "id": id, "company": company, "position": position, "position_id": position_id, "status": status })
        },
    );

    // 第一手真实回答：优先 interview 来源中最早一条，否则全部来源中最早一条
    let questions = sqlx::query_as::<_, DetailQuestion>(
        r#"
        SELECT q.id, q.content, q.my_answer,
          (SELECT json_build_object('content', qa.content, 'source', qa.source, 'created_at', qa.created_at)
             FROM question_answers qa WHERE qa.question_id=q.id
             ORDER BY (qa.source='interview') DESC, qa.created_at ASC LIMIT 1) AS first_answer,
          (SELECT a.score FROM analyses a WHERE a.question_id=q.id AND a.score IS NOT NULL ORDER BY a.created_at DESC LIMIT 1) AS score,
          (SELECT a.feedback FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC LIMIT 1) AS feedback
        FROM questions q WHERE q.round_id=$1 ORDER BY q.id
        "#,
    )
    .bind(round_id)
    .fetch_all(&state.pool)
    .await?;

    let retrospective: Option<Value> =
        sqlx::query_scalar("SELECT retrospective FROM rounds WHERE id=$1").bind(round_id).fetch_one(&state.pool).await?;

    // B组 #3：同投递全部轮次进展（统一时间线用）
    let stages: Value = sqlx::query_scalar(
        r#"
        SELECT COALESCE(json_agg(json_build_object('name', r.name, 'passed', r.passed) ORDER BY r.sort_order, r.id), '[]'::json)
        FROM rounds r WHERE r.application_id=$1
        "#,
    )
    .bind(app_id)
    .fetch_one(&state.pool)
    .await?;

    // ADR-0013 D3：该轮 running 的复盘任务（刷新恢复通道）
    let ai_jobs: Vec<crate::state::AiJob> = state
        .ai_jobs
        .running_for(user.0, "retrospective", round_id)
        .into_iter()
        .collect();

    Ok(Json(json!({
        "round": round,
        "application": application,
        "stages": stages,
        "questions": questions,
        "retrospective": retrospective,
        "ai_jobs": ai_jobs,
    })))
}

#[tracing::instrument(skip_all)]
async fn get_retrospective(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let (round_id, _) = owned_round(&state.pool, user.0, id).await?;
    let r: Option<Value> =
        sqlx::query_scalar("SELECT retrospective FROM rounds WHERE id=$1").bind(round_id).fetch_one(&state.pool).await?;
    Ok(Json(json!({ "retrospective": r })))
}

#[derive(serde::Deserialize)]
struct RetrospectiveReq {
    pub overall: String,
    #[serde(default)]
    pub problems: Vec<String>,
    #[serde(default)]
    pub improvements: Vec<String>,
    /// 人类心得备注（反馈 #3：AI 永不覆盖）
    #[serde(default)]
    pub notes: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn save_retrospective(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<RetrospectiveReq>,
) -> Result<Json<Value>, AppError> {
    let (round_id, _) = owned_round(&state.pool, user.0, id).await?;
    if req.overall.trim().is_empty() {
        return Err(AppError::BadRequest("整体表现不能为空".to_string()));
    }
    // 手动保存：保留既有 weaknesses/advice（AI 产物，编辑器不展示不覆盖）与 generated_by_ai 标记
    let existing: Option<Value> =
        sqlx::query_scalar("SELECT retrospective FROM rounds WHERE id=$1").bind(round_id).fetch_one(&state.pool).await?;
    let mut body = json!({
        "overall": req.overall.trim(),
        "problems": req.problems,
        "improvements": req.improvements,
        "notes": req.notes.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        "generated_by_ai": false,
        "updated_at": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(ext) = existing {
        for k in ["weaknesses", "advice"] {
            if let Some(v) = ext.get(k) {
                body[k] = v.clone();
            }
        }
    }
    sqlx::query("UPDATE rounds SET retrospective=$2 WHERE id=$1")
        .bind(round_id)
        .bind(body)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// AI 生成该轮复盘草稿：上下文 = 本轮题目 + 回答 + 判分（仅用户点击触发，基准 3）
#[tracing::instrument(skip_all)]
async fn ai_retrospective(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let (round_id, _) = owned_round(&state.pool, user.0, id).await?;
    let config = crate::settings::require_llm(&state.pool, user.0).await?;
    // 逐题记录：第一手真实回答（优先 interview 来源）+ 判分/点评（反馈 #3 输入语义）
    let rows: Vec<(String, Option<String>, Option<String>, Option<i32>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT q.content, q.my_answer,
          (SELECT qa.content FROM question_answers qa WHERE qa.question_id=q.id
             ORDER BY (qa.source='interview') DESC, qa.created_at ASC LIMIT 1) AS first_answer,
          (SELECT a.score FROM analyses a WHERE a.question_id=q.id AND a.score IS NOT NULL ORDER BY a.created_at DESC LIMIT 1),
          (SELECT a.feedback FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC LIMIT 1)
        FROM questions q WHERE q.round_id=$1 ORDER BY q.id
        "#,
    )
    .bind(round_id)
    .fetch_all(&state.pool)
    .await?;
    if rows.is_empty() {
        return Err(AppError::BadRequest("该轮还没有题目，无法生成复盘".to_string()));
    }
    let mut ctx = String::new();
    for (content, _my_answer, first_answer, score, feedback) in &rows {
        ctx += &format!(
            "题目：{}\n候选人当时的第一手真实回答：{}\n判分：{} 分\n点评：{}\n\n",
            content,
            first_answer.as_deref().unwrap_or("（未留作答记录）"),
            score.map(|s| s.to_string()).unwrap_or_else(|| "未判".into()),
            feedback.as_deref().unwrap_or("—")
        );
    }

    // ADR-0013 D2 任务化：同轮幂等去重；完成事件回显（结果照旧落 rounds.retrospective）
    let job = match state.ai_jobs.start(user.0, "retrospective", round_id) {
        AiStart::AlreadyRunning(j) => return Ok(Json(json!({ "job_id": j.id, "status": j.status }))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    // panic 守卫统一收尾（评审 P0）；契约层：prompt/schema/解析内聚在 Retrospective
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let span = tracing::info_span!("llm.retrospective", model = %config.model, provider = %config.provider, round_id);
        let contract = crate::contracts::retro::Retrospective::new(ctx);
        let (result, _meta) = contracts::execute(&config, &st.pool, uid, &contract)
            .instrument(span)
            .await?;
        // ADR-0016：strict json_schema；纯文本评审模式 → ir_mode=text 全文落库（improvements 缺失，to-review 不可用）
        let mut body = match result {
            contracts::ContractOut::Structured(v) => serde_json::to_value(v)?,
            contracts::ContractOut::Text(t) => serde_json::json!({
                "ir_mode": "text",
                "content": t,
            }),
        };
        body["generated_by_ai"] = json!(true);
        body["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
        // AI 重生成永不覆盖人类心得（反馈 #3：notes 属于人类）
        if let Ok(existing) =
            sqlx::query_scalar::<_, Value>("SELECT retrospective FROM rounds WHERE id=$1")
                .bind(round_id)
                .fetch_one(&st.pool)
                .await
        {
            if let Some(notes) = existing.get("notes") {
                body["notes"] = notes.clone();
            }
        }
        sqlx::query("UPDATE rounds SET retrospective=$2 WHERE id=$1")
            .bind(round_id)
            .bind(body.clone())
            .execute(&st.pool)
            .await?;
        drop(body);
        Ok::<_, AppError>(())
    });
    Ok(Json(json!({ "job_id": job.id, "status": "running" })))
}

/// 改进项一键入复习队列：在本轮下建题（source=manual）并直接入队——
/// 用户显式要求复习（对 ADR-0006 §6 的有意豁免，代码即文档）。
#[derive(serde::Deserialize)]
struct ToReviewReq {
    pub items: Vec<String>,
}

#[tracing::instrument(skip_all)]
async fn retrospective_to_review(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<ToReviewReq>,
) -> Result<Json<Value>, AppError> {
    let (round_id, _) = owned_round(&state.pool, user.0, id).await?;
    let items: Vec<String> = req.items.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if items.is_empty() {
        return Err(AppError::BadRequest("未选择任何改进项".to_string()));
    }
    let mut created = 0i64;
    for item in &items {
        let qid: i64 = sqlx::query_scalar(
            "INSERT INTO questions(user_id, round_id, content, content_normalized) VALUES($1,$2,$3, normalize_question_content($3)) RETURNING id",
        )
        .bind(user.0)
        .bind(round_id)
        .bind(item)
        .fetch_one(&state.pool)
        .await?;
        sqlx::query("INSERT INTO review_records(question_id) VALUES($1) ON CONFLICT (question_id) DO NOTHING")
            .bind(qid)
            .execute(&state.pool)
            .await?;
        created += 1;
    }
    Ok(Json(json!({ "created": created })))
}

// ---------- 投递整体分析（走向终态后解锁，Q8） ----------

fn require_terminal(status: &str) -> Result<(), AppError> {
    if !matches!(status, "offer" | "rejected" | "withdrawn") {
        return Err(AppError::BadRequest(
            "整体分析在投递走向终态（Offer/拒/弃）后解锁".to_string(),
        ));
    }
    Ok(())
}

#[tracing::instrument(skip_all)]
async fn get_overall(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(aid): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let (status, analysis): (String, Option<Value>) = sqlx::query_as(
        "SELECT status, overall_analysis FROM applications WHERE id=$1 AND user_id=$2",
    )
    .bind(aid)
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    // ADR-0013 D3：running 的整体复盘任务（刷新恢复通道）
    let ai_jobs: Vec<crate::state::AiJob> = state
        .ai_jobs
        .running_for(user.0, "overall", aid)
        .into_iter()
        .collect();
    Ok(Json(json!({
        "unlocked": matches!(status.as_str(), "offer" | "rejected" | "withdrawn"),
        "analysis": analysis,
        "ai_jobs": ai_jobs,
    })))
}

#[derive(serde::Deserialize)]
struct OverallReq {
    pub content: String,
}

#[tracing::instrument(skip_all)]
async fn save_overall(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(aid): Path<i64>,
    Json(req): Json<OverallReq>,
) -> Result<Json<Value>, AppError> {
    let status: String = sqlx::query_scalar("SELECT status FROM applications WHERE id=$1 AND user_id=$2")
        .bind(aid)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    require_terminal(&status)?;
    if req.content.trim().is_empty() {
        return Err(AppError::BadRequest("内容不能为空".to_string()));
    }
    let body = json!({ "content": req.content.trim(), "updated_at": chrono::Utc::now().to_rfc3339() });
    sqlx::query("UPDATE applications SET overall_analysis=$2, updated_at=now() WHERE id=$1 AND user_id=$3")
        .bind(aid)
        .bind(body.clone())
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    Ok(Json(body))
}

// ---------- 投递整体复盘 AI（终态解锁；ADR-0013 任务化；跨轮次归因，反馈 #1） ----------

/// 组装整体复盘上下文：JD + 简历 + 各轮（含逐题第一手回答/判分/点评 + 每轮复盘结论）
async fn build_overall_context(
    pool: &sqlx::PgPool,
    uid: i64,
    aid: i64,
) -> Result<(String, Option<String>), AppError> {
    // JD（岗位属性，ADR-0012）+ 投递基本信息
    let (jd,): (Option<String>,) = sqlx::query_as(
        "SELECT p.jd_text FROM applications a JOIN positions p ON p.id=a.position_id WHERE a.id=$1 AND a.user_id=$2",
    )
    .bind(aid)
    .bind(uid)
    .fetch_one(pool)
    .await?;

    // 各轮 + 逐题
    let rounds: Vec<(i64, String, Option<chrono::NaiveDate>, Option<String>, String)> = sqlx::query_as(
        "SELECT r.id, r.name, r.date, r.form, r.passed FROM rounds r
         WHERE r.application_id=$1 ORDER BY r.sort_order, r.id",
    )
    .bind(aid)
    .fetch_all(pool)
    .await?;

    let mut ctx = String::new();
    for (rid, rname, rdate, rform, rpassed) in &rounds {
        ctx += &format!(
            "\n### 轮次：{}（{}）结果：{}\n",
            rname,
            rform.as_deref().unwrap_or("形式未定"),
            match rpassed.as_str() {
                "pass" => "通过",
                "fail" => "未通过",
                _ => "待定",
            }
        );
        if let Some(d) = rdate {
            ctx += &format!("面试时间：{d}\n");
        }
        // 每轮复盘结论（若有）
        let retro: Option<Value> = sqlx::query_scalar("SELECT retrospective FROM rounds WHERE id=$1")
            .bind(rid)
            .fetch_optional(pool)
            .await?
            .flatten();
        if let Some(overall) = retro.as_ref().and_then(|r| r.get("overall")).and_then(|v| v.as_str()) {
            ctx += &format!("本轮复盘结论：{overall}\n");
        }
        let rows: Vec<(String, Option<String>, Option<i32>, Option<String>)> = sqlx::query_as(
            r#"
            SELECT q.content,
              (SELECT qa.content FROM question_answers qa WHERE qa.question_id=q.id
                 ORDER BY (qa.source='interview') DESC, qa.created_at ASC LIMIT 1) AS first_answer,
              (SELECT a.score FROM analyses a WHERE a.question_id=q.id AND a.score IS NOT NULL ORDER BY a.created_at DESC LIMIT 1),
              (SELECT a.feedback FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC LIMIT 1)
            FROM questions q WHERE q.round_id=$1 ORDER BY q.id
            "#,
        )
        .bind(rid)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            ctx += "（本轮无题目记录）\n";
        }
        for (content, first_answer, score, feedback) in &rows {
            ctx += &format!(
                "题目：{}\n候选人第一手真实回答：{}\n判分：{} 分\n点评：{}\n\n",
                content,
                first_answer.as_deref().unwrap_or("（未留作答记录）"),
                score.map(|s| s.to_string()).unwrap_or_else(|| "未判".into()),
                feedback.as_deref().unwrap_or("—")
            );
        }
    }

    // 简历（结构化）
    let resume: Option<Value> = sqlx::query_scalar(
        "SELECT parsed FROM resumes WHERE user_id=$1 AND is_active ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(uid)
    .fetch_optional(pool)
    .await?
    .flatten();

    let mut user_msg = String::from("【岗位信息】\n");
    if let Some(jd) = jd.as_deref().filter(|s| !s.trim().is_empty()) {
        user_msg += &format!("JD：\n{jd}\n\n");
    } else {
        user_msg += "JD：未填写\n\n";
    }
    user_msg += "【候选人简历】\n";
    match resume {
        Some(v) if v.is_object() => user_msg += &format!("{}\n\n", serde_json::to_string_pretty(&v)?),
        _ => user_msg += "未提供\n\n",
    }
    user_msg += &format!("【面试记录（按时间顺序）】\n{ctx}");
    Ok((user_msg, jd))
}

/// POST /applications/:aid/overall-analysis/ai：终态解锁；任务化幂等；结构化落 overall_analysis。
/// 人类手写内容（content）永不覆盖——迁移为 manual_content 保留。
#[tracing::instrument(skip_all)]
async fn ai_overall_analysis(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(aid): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let status: String = sqlx::query_scalar("SELECT status FROM applications WHERE id=$1 AND user_id=$2")
        .bind(aid)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    require_terminal(&status)?;
    let config = crate::settings::require_llm(&state.pool, user.0).await?;

    let job = match state.ai_jobs.start(user.0, "overall", aid) {
        AiStart::AlreadyRunning(j) => return Ok(Json(json!({ "job_id": j.id, "status": j.status }))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    // panic 守卫统一收尾（评审 P0）；契约层：prompt/schema/解析内聚在 ApplicationOverall
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let (user_msg, _jd) = build_overall_context(&st.pool, uid, aid).await?;
        let span = tracing::info_span!("llm.application_overall", model = %config.model, provider = %config.provider, application_id = aid);
        // ADR-0016：十节报告 strict schema（结构化字段 + report 全文双轨）；text 模式全文落 content
        let contract = crate::contracts::retro::ApplicationOverall::new(user_msg);
        let (result, _meta) = contracts::execute(&config, &st.pool, uid, &contract)
            .instrument(span)
            .await?;
        let mut body = match result {
            contracts::ContractOut::Structured(v) => serde_json::to_value(v)?,
            contracts::ContractOut::Text(t) => serde_json::json!({
                "ir_mode": "text",
                "content": t,
            }),
        };
        body["generated_by_ai"] = json!(true);
        body["updated_at"] = json!(chrono::Utc::now().to_rfc3339());
        // 人类手写内容永不覆盖（先例：retrospective.notes 属于人类）
        if let Ok(existing) =
            sqlx::query_scalar::<_, Value>("SELECT overall_analysis FROM applications WHERE id=$1")
                .bind(aid)
                .fetch_one(&st.pool)
                .await
        {
            if let Some(content) = existing.get("content").and_then(|v| v.as_str()) {
                if !content.trim().is_empty() {
                    body["manual_content"] = json!(content);
                }
            }
        }
        sqlx::query("UPDATE applications SET overall_analysis=$2, updated_at=now() WHERE id=$1 AND user_id=$3")
            .bind(aid)
            .bind(body.clone())
            .bind(uid)
            .execute(&st.pool)
            .await?;
        drop(body);
        Ok::<_, AppError>(())
    });
    Ok(Json(json!({ "job_id": job.id, "status": "running" })))
}


/// 日志/错误信息截断：LLM 输出解析失败时只展示头部，避免整段长文刷屏
pub fn truncate_err(e: impl std::fmt::Display) -> String {
    let e = e.to_string();
    const KEEP: usize = 300;
    if e.chars().count() <= KEEP {
        e.to_string()
    } else {
        let head: String = e.chars().take(KEEP).collect();
        format!("{head}…（共 {} 字符，已截断）", e.chars().count())
    }
}
