//! 训练引擎（ADR-0008 一引擎三场景）。M1 交付 interview 场景：
//! 建场 → SSE 流式多轮对话（AI 出题/即时判分/收尾总结）→ 题目沉淀进题库(source=ai_drill)
//! → 判分回流错题本/复习队列。

use std::convert::Infallible;

use axum::extract::{Extension, Path, Query, State};
use axum::http::header;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::contracts;
use crate::error::AppError;
use crate::llm;
use crate::state::AiStart;
use crate::services::answer_flow;
use crate::models::{
    CreateDrillReq, DrillDetail, DrillMessage, DrillView, SendMessageReq,
};
use crate::auth::CurrentUser;
use crate::settings;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/drills", get(list_drills).post(create_drill))
        .route("/drills/dossier/match", post(match_dossier_questions))
        .route("/drills/{id}", get(get_drill).delete(delete_drill))
        .route("/drills/{id}/messages", post(send_message))
        .route("/drills/{id}/finish", post(finish_drill))
        .route("/drills/{id}/transcript", get(transcript))
        .route("/drills/{id}/interview_prep", post(start_interview_prep))
}

#[derive(Deserialize, Default)]
struct DrillFilter {
    kind: Option<String>,
}

const DRILL_VIEW_SELECT: &str = r#"
    SELECT d.id, d.kind, d.title, d.position, d.direction, d.stages, d.status, d.grading, d.score,
           d.started_at, d.finished_at,
           (SELECT count(*) FROM drill_messages m WHERE m.drill_id=d.id) AS message_count,
           (SELECT count(*) FROM questions q WHERE q.drill_id=d.id AND q.source='ai_drill') AS question_count,
           d.dossier, d.interview_state,
           -- 票 08：迁移 14 回填后不再有 persona_id IS NULL 的行；「经典模式」为退役词条
           CASE WHEN ip.deleted_at IS NOT NULL THEN '已删除的面试官'
                ELSE ip.name END AS persona_label
    FROM drills d
    LEFT JOIN interviewer_personas ip ON ip.id = d.persona_id
"#;

#[tracing::instrument(skip_all)]
async fn list_drills(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Query(f): Query<DrillFilter>,
) -> Result<Json<Vec<DrillView>>, AppError> {
    let sql = format!(
        r#"{DRILL_VIEW_SELECT}
        WHERE d.user_id = $2 AND ($1::text IS NULL OR d.kind = $1)
        ORDER BY d.started_at DESC, d.id DESC
        "#
    );
    let rows = sqlx::query_as::<_, DrillView>(&sql)
        .bind(f.kind)
        .bind(user.0)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(rows))
}

#[tracing::instrument(skip_all)]
async fn create_drill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateDrillReq>,
) -> Result<Json<Value>, AppError> {
    if req.kind.as_str() != "interview" {
        return Err(AppError::BadRequest("kind 必须是 interview（paper/试卷与 resume_grill 已按 ADR-0023 全链路退役）".to_string()));
    }
    // 标题：未提供时自动生成可区分的默认（岗位 + 日期时间），避免多场次同叫"模拟面试"
    let title = match req.title.as_deref().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            let now = chrono::Local::now().format("%m-%d %H:%M").to_string();
            let pos = req.position.as_deref().unwrap_or("");
            format!("模拟面试 · {pos} · {now}")
        }
    };
    // 关联投递校验（JD 驱动陪练，ADR-0011 R4.a）
    if let Some(app_id) = req.application_id {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM applications WHERE id=$1 AND user_id=$2)",
        )
        .bind(app_id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
        if !owned {
            return Err(AppError::BadRequest("关联投递不存在".to_string()));
        }
    }

    // 考官题本（Interviewer Dossier）：如果传入了题目 ID 列表，或传入了 skill_id/tags 靶向圈题，补全题干与参考答案
    let mut dossier = req.dossier;
    if let Some(ref mut d) = dossier {
        let mut ids: Vec<i64> = d.get("question_ids").and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();

        // 靶向攻坚真圈题：若未显式指定 question_ids 但指定了 skill_id / skill_ids 或 tags，自动圈定关联真题
        if ids.is_empty() {
            let skill_id = d.get("skill_id").and_then(|v| v.as_i64());
            let skill_ids: Vec<i64> = d.get("skill_ids").and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
                .unwrap_or_default();
            let tags: Vec<String> = d.get("tags").and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            if skill_id.is_some() || !skill_ids.is_empty() || !tags.is_empty() {
                let subtree_clause = crate::services::skill_query::subtree_multi_condition_sql("q", "$1", "$2", "$4", "$3");
                let sql = format!(
                    r#"
                    SELECT DISTINCT q.id FROM questions q
                    WHERE q.user_id = $1 AND q.parent_id IS NULL
                      AND (
                        ($2::bigint IS NULL AND $4::bigint[] IS NULL AND $3::text[] IS NULL)
                        OR {subtree_clause}
                      )
                    ORDER BY q.id DESC LIMIT 10
                    "#
                );
                let auto_ids: Vec<i64> = sqlx::query_scalar(&sql)
                    .bind(user.0)
                    .bind(skill_id)
                    .bind(if tags.is_empty() { None } else { Some(&tags[..]) })
                    .bind(if skill_ids.is_empty() { None } else { Some(&skill_ids[..]) })
                    .fetch_all(&state.pool)
                    .await
                .map_err(|e| {
                    tracing::warn!(error = %e, skill_id = ?skill_id, skill_ids = ?skill_ids, tags = ?tags, "自动靶向圈题查询失败");
                    e
                })
                .unwrap_or_default();
                ids = auto_ids;
            }
        }

        if !ids.is_empty() {
            #[derive(sqlx::FromRow)]
            struct QInfo {
                id: i64,
                content: String,
                ref_answer: Option<String>,
            }
            let q_rows: Vec<QInfo> = sqlx::query_as(
                r#"
                SELECT q.id, q.content,
                       (SELECT a.ref_answer FROM analyses a WHERE a.question_id=q.id ORDER BY a.created_at DESC LIMIT 1) AS ref_answer
                FROM questions q
                WHERE q.id = ANY($1) AND q.user_id = $2
                "#
            )
            .bind(&ids)
            .bind(user.0)
            .fetch_all(&state.pool)
            .await?;

            let detailed_qs: Vec<Value> = q_rows.into_iter().map(|r| {
                json!({
                    "id": r.id,
                    "question_id": r.id,
                    "content": r.content,
                    "ref_answer": r.ref_answer
                })
            }).collect();

            if let Some(obj) = d.as_object_mut() {
                obj.insert("question_ids".to_string(), json!(ids));
                obj.insert("questions".to_string(), Value::Array(detailed_qs));
            }
        }
    }

    // 场次级人格校验：内置或本人自定义、未删除（ADR-0023 D1）
    // 未传 persona_id 时解析「经典面试官」内置种子（票 08：每行 drills 都有归属）
    let effective_persona_id = match req.persona_id {
        Some(pid) => {
            let ok: Option<(bool, Option<i64>)> = sqlx::query_as(
                "SELECT builtin, owner_user_id FROM interviewer_personas WHERE id=$1 AND deleted_at IS NULL",
            )
            .bind(pid)
            .fetch_optional(&state.pool)
            .await?;
            match ok {
                Some((_, Some(owner))) if owner == user.0 => pid,
                Some((true, _)) => pid,
                _ => return Err(AppError::BadRequest("persona 不存在或不可用".into())),
            }
        }
        None => {
            let classic_id: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM interviewer_personas WHERE name='经典面试官' AND builtin AND deleted_at IS NULL",
            )
            .fetch_optional(&state.pool)
            .await?;
            classic_id.ok_or_else(|| AppError::Internal("经典面试官种子未找到".into()))?
        }
    };

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO drills(user_id, kind, title, position, direction, target_questions, status, ref_content, application_id, dossier, persona_id)
         VALUES($1,$2,$3,$4,$5,COALESCE($6,5),'ongoing',$7,$8,$9,$10) RETURNING id",
    )
    .bind(user.0)
    .bind(&req.kind)
    .bind(&title)
    .bind(req.position.as_deref())
    .bind(req.direction.as_deref())
    .bind(req.target_questions)
    .bind(req.references.as_deref())
    .bind(req.application_id)
    .bind(dossier)
    .bind(effective_persona_id)
    .fetch_one(&state.pool)
    .await?;
    tracing::info!(
        event = "drill.created",
        user_id = user.0,
        drill_id = id,
        kind = %req.kind,
        title = %title,
        "drill session created successfully"
    );
    Ok(Json(json!({ "id": id, "title": title })))
}

#[tracing::instrument(skip_all)]
async fn get_drill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<DrillDetail>, AppError> {
    let sql = format!(r#"{DRILL_VIEW_SELECT} WHERE d.id=$1 AND d.user_id=$2"#);
    let view = sqlx::query_as::<_, DrillView>(&sql)
        .bind(id)
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    // 评审修复：判卷任务的 panic 守卫与收尾已收编进任务自身（catch_unwind + 成败都发
    // paper_grading 事件），GET 不再承担「检测异常 → 顺手写库」的隐藏副作用，回归纯读。
    let messages = sqlx::query_as::<_, DrillMessage>(
        "SELECT id, drill_id, role, kind, content, score, difficulty, feedback, intent, meta, created_at
         FROM drill_messages WHERE drill_id=$1 ORDER BY id ASC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(DrillDetail {
        view,
        messages,
        ai_jobs: state.ai_jobs.running_for(user.0, "interview_prep", id).into_iter().collect(),
    }))
}

#[tracing::instrument(skip_all)]
async fn delete_drill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // 评审 P0 修复：此前多绑了一个 id（3 值对 2 占位符），Postgres 协议层直接报错，
    // 删除场次接口必然失败。
    let deleted = sqlx::query("DELETE FROM drills WHERE id=$1 AND user_id=$2")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

#[tracing::instrument(skip_all)]
async fn finish_drill(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let updated = sqlx::query("UPDATE drills SET status='finished', finished_at=now() WHERE id=$1 AND user_id=$2 AND status='ongoing'")
        .bind(id)
        .bind(user.0)
        .execute(&state.pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::BadRequest("场次不存在或已结束".to_string()));
    }
    // v5 事件总线：派发训练完成事件（积分发放等副作用由监听器统一处理）；失败不影响场次结束
    if let Err(e) = state.event_bus.dispatch(crate::events::DomainEvent::DrillFinished {
        user_id: user.0,
        drill_id: id,
        kind: "manual".into(),
        score: None,
    }).await {
        tracing::error!(error = %e, drill_id = id, "训练积分发放失败（场次已结束）");
    }
    Ok(Json(json!({ "ok": true })))
}

/// 发一条用户消息 → SSE 流式回 AI（首题 / 判分+下一题 / 总结）。
#[tracing::instrument(skip_all)]
async fn send_message(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(did): Path<i64>,
    Json(req): Json<SendMessageReq>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let drill: (
        String, Option<String>, Option<String>, Option<Value>, Option<i32>, String, Option<String>,
        Option<String>, Option<String>, Option<Value>, Option<i64>, Option<Value>,
        Option<f64>, Option<String>, Option<String>,
    ) = sqlx::query_as(
        r#"
        SELECT d.kind, d.position, d.direction, d.stages, d.target_questions, d.status, d.ref_content,
               p.jd_text, d.llm_response_id, d.dossier, d.application_id, d.interview_state,
               ip.temperature_hint::float8 AS temperature_hint, ip.persona_prompt AS persona_prompt,
               array_to_string(ip.focus_tags, '、') AS focus_tags_str
        FROM drills d
        LEFT JOIN applications a ON a.id = d.application_id
        LEFT JOIN positions p ON p.id = a.position_id
        LEFT JOIN interviewer_personas ip ON ip.id = d.persona_id
        WHERE d.id=$1 AND d.user_id=$2
        "#,
    )
    .bind(did)
    .bind(user.0)
    .fetch_one(&state.pool)
    .await?;
    let (_kind, position, direction, _stages, target_questions, status, references, jd_text, chain_base, dossier, _application_id, interview_state, persona_temperature, persona_prompt, persona_focus) = drill;
    let uid = user.0;
    if status != "ongoing" {
        return Err(AppError::BadRequest("该场次已结束，无法继续作答".to_string()));
    }
    let config = settings::require_llm(&state.pool, user.0).await?;
    let target = target_questions.unwrap_or(5).max(1);

    let msgs = sqlx::query_as::<_, DrillMessage>(
        "SELECT id, drill_id, role, kind, content, score, difficulty, feedback, intent, meta, created_at
         FROM drill_messages WHERE drill_id=$1 ORDER BY id ASC",
    )
    .bind(did)
    .fetch_all(&state.pool)
    .await?;
    let last_question = msgs.iter().rev().find(|m| m.role == "ai" && (m.kind == "question" || m.kind == "probe")).cloned();

    // 可编辑 system prompt（高级设置；保持前缀稳定以命中 prompt 缓存，Codex 同策略）
    let system = load_system_prompt(&state.pool, user.0).await?;
    // 题库优先抽取（ADR-0008 §3）：若有考官专属题本（dossier）则跳过题库注入，确保题本最高优先级；否则取自有题库相似题供 AI 优先复用/改编
    let bank = if dossier.is_some() {
        Vec::new()
    } else {
        crate::services::context_manager::load_bank_samples(&state.pool, user.0, &position, &direction).await
    };
    // 本场稳定上下文：唯一装配点（ADR-0023 D5）——人设+笔记+题本/题库/参考/JD 分层装配与裁剪
    let persona_prefix: Option<String> = persona_prompt.as_deref().map(|prompt| {
        let mut s = prompt.trim().to_string();
        if let Some(focus) = persona_focus.as_deref().filter(|f| !f.is_empty()) {
            s.push_str(&format!("\n考察侧重：{focus}"));
        }
        s
    });
    let history_turns: Vec<crate::services::context_manager::Turn> = msgs
        .iter()
        .filter_map(|m| {
            if m.role == "ai" && (m.kind == "question" || m.kind == "probe") {
                Some(crate::services::context_manager::Turn {
                    role: "ai".into(),
                    content: format!("【面试官提问】{}", m.content),
                })
            } else if m.role == "user" && m.kind == "answer" {
                Some(crate::services::context_manager::Turn {
                    role: "user".into(),
                    content: format!("【候选人回答】{}", m.content),
                })
            } else {
                None
            }
        })
        .collect();
    let session = crate::services::context_manager::assemble_session(
        &crate::services::context_manager::SessionInput {
            persona_prefix: persona_prefix.as_deref(),
            notes: interview_state.as_ref(),
            dossier: dossier.as_ref(),
            references: references.as_deref(),
            bank_lines: &bank,
            jd_text: jd_text.as_deref(),
            turns: &history_turns,
        },
    );
    let context = session.system_prefix;

    // M5a：温度即人格——persona temperature_hint 覆盖用户默认采样参数（仅本场会话）
    let mut config = config;
    if let Some(t) = persona_temperature {
        config.temperature = Some(t);
    }

    let is_first_turn = last_question.is_none();

    // 显式 action 驱动：首轮缺省启动，答题轮缺省作答，彻底消灭字符串模糊匹配
    let action = req.action.as_deref().unwrap_or_else(|| {
        if is_first_turn {
            "start"
        } else {
            "answer"
        }
    });

    let is_hint_req = action == "hint";
    let is_skip_req = action == "skip";
    let is_finish_req = action == "finish";

    // 状态安全校验：尚未开始出题时，禁止触发 hint / skip / finish
    if is_first_turn && (is_hint_req || is_skip_req || is_finish_req) {
        return Err(AppError::BadRequest("面试尚未开始，请先启动面试（action='start'）".into()));
    }

    let last_answer: Option<String> = sqlx::query_scalar(
        "SELECT content FROM drill_messages WHERE drill_id=$1 AND role='user' AND kind='answer' ORDER BY id DESC LIMIT 1",
    )
    .bind(did)
    .fetch_optional(&state.pool)
    .await?;

    let is_retry = last_answer.as_deref() == Some(req.content.as_str()) && !is_hint_req && !is_skip_req && !is_finish_req;

    let target_question = if is_retry {
        let ans_idx = msgs.iter().rposition(|m| m.role == "user" && m.kind == "answer").unwrap_or(0);
        let prev_q = msgs[..ans_idx].iter().rev().find(|m| m.role == "ai" && (m.kind == "question" || m.kind == "probe")).cloned();
        prev_q.or_else(|| last_question.clone())
    } else {
        last_question.clone()
    };

    // 决定下一轮节奏与意图：有效答题数（第一道考题之后的回答）达标/用户交卷 -> 总结
    let first_q_idx = msgs.iter().position(|m| m.role == "ai" && (m.kind == "question" || m.kind == "probe"));
    let valid_answers = if let Some(idx) = first_q_idx {
        msgs[idx..].iter().filter(|m| m.role == "user" && m.kind == "answer").count()
    } else {
        0
    };
    let current_valid_answer = if !is_hint_req && target_question.is_some() && !is_retry { 1 } else { 0 };
    let total_answers = valid_answers + current_valid_answer;

    let mut to_summary = is_finish_req || (target_question.is_some() && !is_hint_req && {
        let reached = (total_answers as i32) >= target;
        reached
    });

    let asked = msgs.iter().filter(|m| m.kind == "question" || m.kind == "probe").count();
    let last_was_probe = msgs.iter().rev().find(|m| m.kind == "probe" || (m.kind == "question" && m.intent.as_deref() == Some("followup_probe"))).is_some();
    let mut intent_out = if to_summary {
        "summary"
    } else if is_hint_req {
        "hint"
    } else if is_skip_req {
        "skip"
    } else if last_question.is_none() {
        "main_question"
    } else if !last_was_probe && asked < target as usize {
        "followup_probe"
    } else {
        "main_question"
    };

    // 判分（回答轮）：
    // - 快捷辅助指令(hint)：仅记录交互，不扣分不判分
    // - 首轮(start)：仅记录开始
    // - 追问阶段(followup_probe)：不单独判分，仅记录当前回答，留待追问完成后整题合并判分
    // - 题完结阶段(main_question/summary)：结合本道题主考题+所有追问轮次进行统一判分
    // M4（ADR-0023 D2）：answer 轮统一走「单次流式两段式」——续接先流出、评分后置；
    // hint/skip/start 维持原路径。merged 轮的评分在流内 REPORT 段解析后落库（时序不变，错题本/积分零影响）。
    let merged_answer_turn = target_question.is_some() && !is_hint_req && !is_skip_req;

    #[derive(Clone)]
    struct InlineReportCtx {
        main_q_content: String,
        main_answer: String,
        probe_answer: Option<String>,
    }
    let mut inline_report_ctx: Option<InlineReportCtx> = None;

    let (feedback, grade_score, reuse): (Option<String>, Option<i32>, bool) = if let Some(q) = &target_question {
        if is_hint_req {
            // 辅助指令：仅当用户传了具体文本时才记录，避免空交互行
            if !req.content.trim().is_empty() {
                append_drill_message(&state.pool, uid, did, "user", "control", req.content.trim(), None, None, None, None).await?;
            }
            (None, None, false)
        } else if is_skip_req {
            // 跳过本题：记录 0 分与跳过标记
            let main_q = msgs.iter().rfind(|m| m.role == "ai" && m.kind == "question").cloned().unwrap_or_else(|| q.clone());
            let analysis = grade_and_record(&state.pool, &state.event_bus, uid, &config, did, &main_q.content, "（候选人选择跳过本题）", None, "（候选人选择跳过本题）", "（候选人选择跳过本题）", true).await?;
            (analysis.feedback, Some(0), false)
        } else {
            // M4 合并轮（ADR-0023 D2）：续接先行的前置工作——仅落用户回答消息；
            // 题库同步与合并判分延后至流内 REPORT 段解析（record_inline_analysis）。
            // 幂等恢复：retry 且该题已判分 -> 复用既有分析（不重复 LLM 调用、不重复记分）
            let main_q_idx = msgs.iter().rposition(|m| m.role == "ai" && m.kind == "question").unwrap_or(0);
            let main_q_content = msgs
                .iter()
                .rfind(|m| m.role == "ai" && m.kind == "question")
                .map(|m| m.content.clone())
                .unwrap_or_else(|| q.content.clone());
            let has_probe = msgs[main_q_idx + 1..]
                .iter()
                .any(|m| m.role == "ai" && (m.kind == "probe" || m.intent.as_deref() == Some("followup_probe")));
            let main_answer = msgs[main_q_idx + 1..]
                .iter()
                .find(|m| m.role == "user" && m.kind == "answer")
                .map(|m| m.content.clone())
                .unwrap_or_else(|| req.content.clone());

            let already_scored: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM questions q JOIN analyses a ON a.question_id=q.id WHERE q.drill_id=$1 AND q.content=$2)",
            )
            .bind(did)
            .bind(&main_q_content)
            .fetch_one(&state.pool)
            .await?;
            let reuse = is_retry && already_scored;

            let analysis = if reuse {
                let qid: i64 = sqlx::query_scalar(
                    "SELECT id FROM questions WHERE drill_id=$1 AND source='ai_drill' AND content=$2 ORDER BY id DESC LIMIT 1",
                )
                .bind(did)
                .bind(&main_q_content)
                .fetch_one(&state.pool)
                .await?;
                Some(fetch_q_analysis(&state.pool, qid).await?)
            } else {
                None
            };

            if !is_retry {
                append_drill_message(&state.pool, uid, did, "user", "answer", &req.content, None, None, None, None).await?;
            }

            inline_report_ctx = Some(InlineReportCtx {
                main_q_content,
                main_answer,
                probe_answer: if has_probe { Some(req.content.clone()) } else { None },
            });
            (
                analysis.as_ref().and_then(|a| a.feedback.clone()),
                analysis.as_ref().and_then(|a| a.score),
                reuse,
            )
        }
    } else {
        // 首轮启动：仅落一条 start 消息（且幂等），绝不插入空 answer 行
        let already_has_start = msgs.iter().any(|m| m.role == "user" && (m.kind == "start" || m.kind == "answer"));
        if !already_has_start {
            append_drill_message(&state.pool, uid, did, "user", "start", "开始", None, None, None, None).await?;
        }
        (None, None, false)
    };

    // 极端糟糕回答快速提前熔断
    if !to_summary && total_answers >= 2 && grade_score.map_or(false, |s| s <= 25 && !is_skip_req) {
        to_summary = true;
        intent_out = "summary";
    }

    let pool = state.pool.clone();
    let event_bus = state.event_bus.clone();
    let latest_answer = req.content.clone();
    let first_turn = last_question.is_none();
    let history: Vec<String> = session
        .turns
        .iter()
        .map(|t| {
            if t.role == "ai" {
                t.content.replacen("【面试官提问】", "【面试官提问】", 1)
            } else {
                t.content.clone()
            }
        })
        .collect();

    // M4 两段式协议 task（ADR-0023 D2）：merged 轮的「续接」与「报告」在同一次流式调用中产出。
    let task = if first_turn {
        "请直接给出本场第一道核心面试题。严禁任何寒暄客套，严禁任何前缀解释，100%只输出题目正文本身。".to_string()
    } else if merged_answer_turn {
        const PROTOCOL_TAIL: &str =
            "第二段：仅当本次输出为新主问题或复盘报告时（追问轮次禁止），在正文之后另起一行输出:\n<<<REPORT>>>\n{\"tags\":[\"考点\"],\"difficulty\":3,\"ref_answer\":\"参考答案要点\",\"score\":0,\"feedback\":\"中文点评（综合分 0-100：正确性50%+完整性30%+表达清晰度20%）\"}\n其中 JSON 为对上一道主考题合并判分的结果（结合其全部追问轮次）。";
        let hint_note = match req.hint_level.unwrap_or(0) {
            1 => "\n【提示使用情况】候选人使用了 Level 1 思考方向提示：评分扣除 5~10 分独立思考分，并在点评中指出其对提示的依赖。",
            2 => "\n【提示使用情况】候选人使用了 Level 2 核心原理提示：重点扣分（15~25）并指出依赖提示才能作答的短板。",
            3 => "\n【提示使用情况】候选人完整使用了 Level 3 关键解法提示：本题综合得分上限不超过 60 分，并严肃指出严重依赖答案提示的问题。",
            _ => "",
        };
        let summary_part = if to_summary {
            "本场考核已达标收尾。第一段改为输出全场综合复盘报告 Markdown，结构如下：\n# 🎯 模拟面试全场综合复盘报告\n## 📊 一、综合表现评级与总评（总评等级 S/A/B/C 四选一 + 核心能力画像）\n## 🌟 二、答题亮点剖析\n## ⚠️ 三、主要短板与破绽漏洞\n## 🚀 四、靶向强化建议与行动指南".to_string()
        } else {
            "第一段：通读候选人本题全部作答后自主判断——若存在值得深挖的破绽/含糊处，输出一道深入底层原理、高并发边界或破绽细节的追问题干，并在其前单独一行输出 <<<PROBE>>>{\"anchor_keyword\":\"锚点关键词\",\"reason\":\"枚举值\"}（reason 仅限 depth_probe/clarification/edge_case/contradiction/breadth_pivot 五选一）；否则直接输出一道全新核心面试题正文（不与对话历史重复）。此段严禁任何点评与寒暄。".to_string()
        };
        format!("【回答处理·两段式】候选人已提交回答。请严格按以下两段输出：\n{summary_part}\n{PROTOCOL_TAIL}{hint_note}")
    } else if is_hint_req {
        "【三级阶梯式提示】候选人思路卡壳请求提示。请严格按照以下三层结构给出提示（每层必须严格以指定 markdown 标题开头，供系统分级解析展示）：\n### Level 1: 思考方向\n[给出 1~2 句话的思考维度与切入点引导，启发思考，严禁透露具体实现]\n\n### Level 2: 核心原理\n[给出 1~2 句话的关键机制、底层数据结构或技术选型要点]\n\n### Level 3: 关键解法\n[给出核心算法逻辑、关键伪代码骨架或架构设计完整闭环要点]".to_string()
    } else if is_skip_req {
        "【跳过公布答案】候选人跳过本题。请用 2-3 句话简要给出本题的核心标准答案要点，然后立刻给出下一道核心面试题。".to_string()
    } else {
        // 防御性兜底：正常不可达（answer 轮已被 merged_answer_turn 覆盖），绝不 panic
        "【切换新考点】请直接提出下一道全新核心面试题（不要与对话历史中已出的题重复）。".to_string()
    };
    let ctx = InterviewCtx {
        system,
        context,
        history,
        task,
        asked,
        to_summary,
        feedback: feedback.clone(),
        grade_score,
        reuse,
        intent: intent_out.to_string(),
        merged: merged_answer_turn && !reuse,
        chain_base: if config.store { chain_base } else { None },
    };

    let s = async_stream::stream! {
        use futures_util::StreamExt as _;
        let mut kind_out: &'static str = if ctx.to_summary {
            "summary"
        } else if ctx.intent == "followup_probe" {
            "probe"
        } else if ctx.intent == "hint" {
            "hint"
        } else {
            "question"
        };

        // 幂等恢复（retry 且已判分）：状态已就绪，不重复生成、不推进，直接结束
        if !ctx.reuse {
            // 1) 旧路径的判分点评事件（非 merged 轮：评分先行，见 M4 前的行为）
            if !ctx.merged {
                if let Some(fb) = &ctx.feedback {
                    let _ = sqlx::query(
                        "UPDATE drill_messages SET feedback=$1, score=$2 WHERE drill_id=$3 AND role='user' AND kind='answer' AND id=(SELECT id FROM drill_messages WHERE drill_id=$3 AND role='user' AND kind='answer' ORDER BY id DESC LIMIT 1)"
                    )
                    .bind(fb)
                    .bind(ctx.grade_score)
                    .bind(did)
                    .execute(&pool)
                    .await;

                    yield Ok::<Event, Infallible>(
                        Event::default().event("feedback").data(json!({ "score": ctx.grade_score, "feedback": fb }).to_string())
                    );
                }
            }

            // 2) 流式生成：merged 轮经 SentinelGate 过滤哨兵行后再下发（续接先流出）
            let mut text = String::new();
            let mut gate = answer_flow::SentinelGate::new();
            let mut chain: Option<String> = ctx.chain_base.clone();
            let mut resp_id: Option<String> = None;
            loop {
                let messages = if chain.is_some() {
                    chain_turn_messages(&ctx.system, &latest_answer, &ctx.task, Some(&ctx.context))
                } else {
                    turn_messages(&ctx.system, &ctx.context, &ctx.history, &ctx.task)
                };
                let mut degraded = false;
                let mut llm_stream = Box::pin(llm::stream_chat(config.clone(), messages, chain.clone()));
                while let Some(r) = llm_stream.next().await {
                    match r {
                        Ok(llm::StreamItem::Content(d)) => {
                            text.push_str(&d);
                            if ctx.merged {
                                let live = gate.push(&d);
                                if !live.is_empty() {
                                    yield Ok::<Event, Infallible>(delta_event(&json!({ "text": live })));
                                }
                            } else {
                                yield Ok::<Event, Infallible>(delta_event(&json!({ "text": d })));
                            }
                        }
                        Ok(llm::StreamItem::Thinking(t)) => {
                            yield Ok::<Event, Infallible>(
                                Event::default().event("thinking").data(json!({ "text": t }).to_string()),
                            );
                        }
                        Ok(llm::StreamItem::Completed(id)) => {
                            resp_id = Some(id);
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            if chain.is_some() && text.is_empty() && looks_like_chain_error(&msg) {
                                degraded = true;
                                break;
                            }
                            yield Ok::<Event, Infallible>(
                                Event::default().event("error").data(json!({ "message": msg }).to_string()),
                            );
                        }
                    }
                }
                if !degraded {
                    break;
                }
                tracing::warn!(drill_id = did, "previous_response_id 链式请求被上游拒绝，降级为全量重放");
                chain = None;
            }

            // 记录本轮响应顶层 id
            if let Some(id) = resp_id.as_deref() {
                let _ = sqlx::query("UPDATE drills SET llm_response_id=$2 WHERE id=$1")
                    .bind(did)
                    .bind(id)
                    .execute(&pool)
                    .await;
            }

            // 3) M4：merged 轮协议解析——续接/报告分离；追问元数据入 meta 列
            let mut inline_report: Option<Value> = None;
            let mut probe_meta_json: Option<Value> = None;
            let text_trimmed = if ctx.merged {
                // 流结束：把被门 hold 住的「续接」尾巴补发为最后一个 delta（REPORT 段不下发）
                let cont_tail = gate.finish();
                if !cont_tail.is_empty() {
                    yield Ok::<Event, Infallible>(delta_event(&json!({ "text": cont_tail })));
                }
                let out = answer_flow::split_answer_output(&text, ctx.to_summary);
                kind_out = match &out.continuation {
                    answer_flow::Continuation::Probe { .. } => "probe",
                    answer_flow::Continuation::Summary => "summary",
                    answer_flow::Continuation::Question => "question",
                };
                if let answer_flow::Continuation::Probe { meta } = &out.continuation {
                    probe_meta_json = serde_json::to_value(meta).ok();
                }
                inline_report = out.report;
                out.continuation_text
            } else {
                text.trim().to_string()
            };
            let text_trimmed = text_trimmed.trim();

            if !text_trimmed.is_empty() {
                if kind_out == "summary" {
                    let _ = sqlx::query(
                        "INSERT INTO drill_messages(user_id, drill_id, role, kind, content, intent) VALUES($1,$2,'ai','summary',$3,$4)",
                    )
                    .bind(uid)
                    .bind(did)
                    .bind(text_trimmed)
                    .bind(&ctx.intent)
                    .execute(&pool)
                    .await;
                    let _ = sqlx::query("UPDATE drills SET status='finished', finished_at=now() WHERE id=$1")
                        .bind(did)
                        .execute(&pool)
                        .await;
                    let _ = event_bus.dispatch(crate::events::DomainEvent::DrillFinished {
                        user_id: uid,
                        drill_id: did,
                        kind: "interview".into(),
                        score: None,
                    }).await;
                } else if kind_out == "question" {
                    // 主考题：沉淀入题库独立记录
                    let _qid = sink_ai_question(&pool, uid, did, text_trimmed).await;
                    let _ = sqlx::query(
                        "INSERT INTO drill_messages(user_id, drill_id, role, kind, content, intent) VALUES($1,$2,'ai','question',$3,'main_question')",
                    )
                    .bind(uid)
                    .bind(did)
                    .bind(text_trimmed)
                    .execute(&pool)
                    .await;
                } else if kind_out == "probe" {
                    // 追问：关联入当前题目的 parent_id 追问链，不生成独立题目
                    let parent_qid: Option<i64> = sqlx::query_scalar(
                        "SELECT id FROM questions WHERE drill_id=$1 AND user_id=$2 AND source='ai_drill' AND parent_id IS NULL ORDER BY id DESC LIMIT 1",
                    )
                    .bind(did)
                    .bind(uid)
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten();

                    if let Some(pid) = parent_qid {
                        let round_id = ensure_ai_round(&pool, uid).await.unwrap_or(0);
                        let _ = sqlx::query(
                            "INSERT INTO questions(user_id, round_id, content, content_normalized, parent_id, source, drill_id) VALUES($1,$2,$3, normalize_question_content($3), $4,'ai_drill',$5)",
                        )
                        .bind(uid)
                        .bind(round_id)
                        .bind(text_trimmed)
                        .bind(pid)
                        .bind(did)
                        .execute(&pool)
                        .await;
                    }
                    if let Err(e) = append_drill_message_meta(
                        &pool,
                        uid,
                        did,
                        "ai",
                        "probe",
                        text_trimmed,
                        None,
                        None,
                        None,
                        Some("followup_probe"),
                        probe_meta_json.as_ref(),
                    )
                    .await
                    {
                        yield Ok::<Event, Infallible>(
                            Event::default().event("error").data(json!({ "message": e.to_string() }).to_string()),
                        );
                    }
                } else {
                    // 提示(hint) 或 辅助指令：仅记录当场会话，不入题库
                    let _ = sqlx::query(
                        "INSERT INTO drill_messages(user_id, drill_id, role, kind, content, intent) VALUES($1,$2,'ai',$3,$4,$5)",
                    )
                    .bind(uid)
                    .bind(did)
                    .bind(kind_out)
                    .bind(text_trimmed)
                    .bind(&ctx.intent)
                    .execute(&pool)
                    .await;
                }

                // 4) M4：REPORT 段落库——评分后置但持久化语义与 grade_and_record 完全一致
                if let Some(report) = &inline_report {
                    match (inline_report_ctx.as_ref(), Some(report)) {
                        (Some(ictx), Some(report)) => {
                            match record_inline_analysis(
                                &pool,
                                &event_bus,
                                uid,
                                &config,
                                did,
                                &ictx.main_q_content,
                                &ictx.main_answer,
                                ictx.probe_answer.as_deref(),
                                &report,
                            )
                            .await
                            {
                                Ok((score, feedback)) => {
                                    if feedback.is_some() || score.is_some() {
                                        let _ = sqlx::query(
                                            "UPDATE drill_messages SET feedback=$1, score=$2 WHERE drill_id=$3 AND role='user' AND kind='answer' AND id=(SELECT id FROM drill_messages WHERE drill_id=$3 AND role='user' AND kind='answer' ORDER BY id DESC LIMIT 1)"
                                        )
                                        .bind(&feedback)
                                        .bind(score)
                                        .bind(did)
                                        .execute(&pool)
                                        .await;

                                        yield Ok::<Event, Infallible>(
                                            Event::default().event("feedback").data(json!({ "score": score, "feedback": feedback }).to_string())
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(drill_id = did, err = %e, "内联判分落库失败");
                                    yield Ok::<Event, Infallible>(
                                        Event::default().event("error").data(json!({ "message": e.to_string() }).to_string()),
                                    );
                                }
                            }
                        }
                        _ => {
                            tracing::warn!(drill_id = did, "REPORT 段存在但缺少内联上下文，跳过落库");
                        }
                    }
                }
            }
        }
        yield Ok(meta_event(&json!({
            "score": ctx.grade_score,
            "kind": kind_out,
            "intent": ctx.intent,
            "question_count": ctx.asked,
            "finished": ctx.to_summary,
        })));
        yield Ok(Event::default().event("done").data("{}"));
    };
    Ok(Sse::new(s).keep_alive(axum::response::sse::KeepAlive::new()))
}

struct InterviewCtx {
    system: String,
    context: String,
    history: Vec<String>,
    task: String,
    asked: usize,
    to_summary: bool,
    feedback: Option<String>,
    grade_score: Option<i32>,
    reuse: bool,
    intent: String,
    /// M4：本轮走「两段式合并流」（续接先流出、REPORT 后置解析）
    merged: bool,
    /// 链式上下文基底：上一成功响应的顶层 id（UUID 形态）；None = 全量重放模式
    chain_base: Option<String>,
}

/// 默认 system prompt（高级设置可编辑）。保持常量 => 命中 prompt 缓存（OpenAI Codex 同策略：
/// 稳定前缀在前，动态内容由后端放到末尾）。
pub const DEFAULT_SYSTEM_PROMPT: &str = r#"你是资深技术面试官与面试复盘教练，正在主持模拟面试。
你会收到三块信息：【本场信息】（岗位/方向/阶段/考官题本/参考内容/简历/面试官笔记）、【对话历史】、【当前任务】。

出题与追问要求：
0. 【最高优先级·专属题本】：若本场信息中包含【考官专属参考题本 (Interviewer Dossier)】，你必须严格以题本中的考核侧重、题目范围与考察重点为主线进行考核与出题，严禁脱离题本范围泛化出题；
1. 题目具体、可深挖，紧扣本场阶段、岗位技术栈与考核侧重；
2. 场次内尽量覆盖 ≥2 个不同考察维度（意愿与适配度/履历与项目深挖/专业理论基础/业务场景推演/现场实操交付/异常与危机处理/沟通与团队协同），若面试官人格 focus_tags 指定了侧重维度，优先围绕侧重维度出题；
3. 有【自有题库候选】时，优先复用或改编其中的题（结合本场岗位/方向）；
3. 有【参考内容】时，优先从中提炼真实岗位常考题；
4. 有【简历】时针对简历中的项目/经历逐条深挖细节；
5. 不要与对话历史中已出的题重复；一次只出一道题；
6. 【绝对禁令】：当任务为出题或追问时，严禁对候选人上一轮回答进行任何点评、评价或客套（如“回答得不错”、“针对你的回答”等），系统已有独立判分管线向候选人反馈评语，你必须 100% 仅输出题目正文本身！
7. 有【面试官笔记 (Interviewer Notes)】时，围绕其中的「风险信号」与「建议追问」组织本题考察点与追问链（题本管问什么，笔记管问谁）；
判分量纲（判分时使用）：综合分 0-100（正确性 50% + 完整性 30% + 表达清晰度 20%）、难度 1-5、中文点评。"#;

async fn load_system_prompt(pool: &sqlx::PgPool, uid: i64) -> Result<String, AppError> {
    // 提示词注册表统一管理（自定义 ?? 旧 key ?? 内置默认）
    crate::prompts::effective(pool, uid, crate::prompts::DRILL_INTERVIEW).await
}

/// Codex 式消息结构（全量重放模式）：system(常量前缀,可缓存) + 单条 user 消息——
/// 本场稳定 context 在前、动态 history/task 收尾，合并为一条 Content。
/// 评审 P0：连续 role:user 消息不再下发（部分端点会拒绝或误解多段 user 输入）。
fn turn_messages(system: &str, context: &str, history: &[String], task: &str) -> Vec<Value> {
    let history_str = if history.is_empty() {
        "（暂无）".to_string()
    } else {
        history.join("\n")
    };
    let mut user = String::new();
    if !context.trim().is_empty() {
        user += &format!("{context}\n\n");
    }
    user += &format!("【对话历史】\n{history_str}\n\n【当前任务】\n{task}");
    vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ]
}

/// 链式模式消息（previous_response_id 已关联全部历史）：携带本场关键上下文约束与最新作答——
/// 确保即便上游上下文发生截断或衰减，核心考核侧重与考官题本约束依然稳固。
fn chain_turn_messages(system: &str, latest_answer: &str, task: &str, context: Option<&str>) -> Vec<Value> {
    let mut user = String::new();
    if let Some(ctx) = context {
        if !ctx.trim().is_empty() {
            user += &format!("{ctx}\n\n");
        }
    }
    user += &format!("【候选人最新回答】\n{latest_answer}\n\n【当前任务】\n{task}");
    vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ]
}

/// 链式相关错误签名：previous_response_id 未被留存/已超 7 天有效期/端点不支持链式。
/// 命中后在未吐正文的前提下自动降级全量重放重试一次（对用户透明）。
fn looks_like_chain_error(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("previous_response")
        || (m.contains("response")
            && (m.contains("not found")
                || m.contains("not_found")
                || m.contains("expired")
                || m.contains("不存在")
                || m.contains("过期")))
}


/// 调用 LLM 流式生成并拼成完整文本
/// AI 出的题沉淀进题库（去重：同 content 的 ai_drill 题复用），自动入复习队
async fn sink_ai_question(pool: &sqlx::PgPool, uid: i64, drill_id: i64, content: &str) -> Result<i64, AppError> {
    let content = content.trim();
    if content.is_empty() {
        return Err(AppError::BadRequest("AI 未产出有效题目".to_string()));
    }
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM questions WHERE user_id=$1 AND content=$2 AND source='ai_drill' ORDER BY id DESC LIMIT 1",
    )
    .bind(uid)
    .bind(content)
    .fetch_optional(pool)
    .await?;
    // 沉淀时点：AI 出的题先入题库（不直接进复习队）；
    // 判分（run_analysis）完成才 enqueue_review，避免未分析的卡污染复习队列（ADR-0006 §6）
    if let Some(id) = existing {
        return Ok(id);
    }
    let round_id = ensure_ai_round(pool, uid).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO questions(user_id, round_id, content, content_normalized, source, drill_id) VALUES($1,$2,$3, normalize_question_content($3), 'ai_drill',$4) RETURNING id",
    )
    .bind(uid)
    .bind(round_id)
    .bind(content)
    .bind(drill_id)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// 导出整场对话为纯文本（plan.md v2 API）
#[tracing::instrument(skip_all)]
async fn transcript(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let rows: Vec<(String, String, String, Option<i32>)> = sqlx::query_as(
        "SELECT m.role, m.kind, m.content, m.score FROM drill_messages m
         JOIN drills d ON d.id=m.drill_id WHERE m.drill_id=$1 AND d.user_id=$2 ORDER BY m.id ASC",
    )
    .bind(id)
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;
    let mut out = String::new();
    for (role, kind, content, score) in rows {
        let tag = match (role.as_str(), kind.as_str()) {
            ("ai", "question") => "面试官",
            ("ai", "score") => {
                out.push_str(&format!("【判分 {} 分】\n", score.unwrap_or(0)));
                "面试官"
            }
            ("ai", "summary") => "总结",
            ("user", _) => "我",
            _ => "面试官",
        };
        out.push_str(&format!("{tag}：{content}\n\n"));
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"drill_{id}.txt\""),
            ),
        ],
        out,
    ))
}

/// 统一插入一条训练消息（消除 send_message/generate_paper/submit_paper 的重复 INSERT）。
/// 接受任意 executor（连接池或事务），供事务内使用（评审 P1 整改）。
async fn append_drill_message(
    db: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    uid: i64,
    drill_id: i64,
    role: &str,
    kind: &str,
    content: &str,
    score: Option<i32>,
    difficulty: Option<i32>,
    feedback: Option<&str>,
    intent: Option<&str>,
) -> Result<(), AppError> {
    append_drill_message_meta(db, uid, drill_id, role, kind, content, score, difficulty, feedback, intent, None).await
}

async fn append_drill_message_meta(
    db: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    uid: i64,
    drill_id: i64,
    role: &str,
    kind: &str,
    content: &str,
    score: Option<i32>,
    difficulty: Option<i32>,
    feedback: Option<&str>,
    intent: Option<&str>,
    meta: Option<&Value>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO drill_messages(user_id, drill_id, role, kind, content, score, difficulty, feedback, intent, meta) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(uid)
    .bind(drill_id)
    .bind(role)
    .bind(kind)
    .bind(content)
    .bind(score)
    .bind(difficulty)
    .bind(feedback)
    .bind(intent)
    .bind(meta)
    .execute(db)
    .await?;
    Ok(())
}

/// 判分并落 用户回答 + score 消息（面试与判卷共用），返回分析结果
async fn grade_and_record(
    pool: &sqlx::PgPool,
    event_bus: &crate::events::EventBus,
    uid: i64,
    config: &settings::LlmConfig,
    drill_id: i64,
    content: &str,
    main_answer: &str,
    probe_answer: Option<&str>,
    display_user_msg: &str,
    eval_answer: &str,
    append_answer: bool,
) -> Result<crate::models::AnalysisRow, AppError> {
    let qid: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM questions WHERE user_id=$1 AND drill_id=$2 AND source='ai_drill' AND content=$3 ORDER BY id DESC LIMIT 1",
    )
    .bind(uid)
    .bind(drill_id)
    .bind(content)
    .fetch_optional(pool)
    .await?;
    let qid = match qid {
        Some(id) => id,
        None => sink_ai_question(pool, uid, drill_id, content).await?,
    };
    // 幂等：首次才追加用户回答 + 同步题库 + 记历史 + 关联训练轮次（重试复用不重复）
    if append_answer {
        append_drill_message(pool, uid, drill_id, "user", "answer", display_user_msg, None, None, None, None).await?;
        let _ = sqlx::query("UPDATE questions SET my_answer=$2 WHERE id=$1")
            .bind(qid)
            .bind(main_answer)
            .execute(pool)
            .await;
        crate::routes::questions::record_answer(pool, qid, "interview", main_answer).await?;
        if let Ok(sink_round) = ensure_ai_round(pool, uid).await {
            let _ = crate::routes::questions::link_round(pool, qid, sink_round).await;
        }

        // 如果存在追问且传入了追问回答，同步更新关联的追问题目记录的 my_answer 与历史记录
        if let Some(p_ans) = probe_answer {
            let followup_qid: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM questions WHERE drill_id=$1 AND user_id=$2 AND parent_id=$3 ORDER BY id DESC LIMIT 1",
            )
            .bind(drill_id)
            .bind(uid)
            .bind(qid)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

            if let Some(f_qid) = followup_qid {
                let _ = sqlx::query("UPDATE questions SET my_answer=$2 WHERE id=$1")
                    .bind(f_qid)
                    .bind(p_ans)
                    .execute(pool)
                    .await;
                let _ = crate::routes::questions::record_answer(pool, f_qid, "interview", p_ans).await;
            }
        }
    }
    // 判分：将主考题回答作为 answer_snapshot，完整上下文作为 eval_answer，确保题库详情精准匹配即时点评
    grade_question(pool, event_bus, uid, config, drill_id, qid, content, main_answer, eval_answer).await
}

/// 对一道已落库的题判分：run_analysis_ext + 写 score 消息；失败降级不中断（answers 已先落库）
async fn grade_question(
    pool: &sqlx::PgPool,
    event_bus: &crate::events::EventBus,
    uid: i64,
    config: &settings::LlmConfig,
    drill_id: i64,
    qid: i64,
    content: &str,
    main_answer: &str,
    eval_answer: &str,
) -> Result<crate::models::AnalysisRow, AppError> {
    let analysis = match crate::routes::questions::run_analysis_ext(pool, uid, qid, content, Some(main_answer), Some(eval_answer), config).await {
        Ok(a) => {
            // v5 事件总线：派发 AI 沉淀题判分完成事件（积分发放由监听器处理）
            let _ = event_bus.dispatch(crate::events::DomainEvent::AiSinkQuestionGraded {
                user_id: uid,
                drill_id,
                question_id: qid,
            }).await;
            a
        }
        Err(e) => {
            tracing::warn!(question_id = qid, err = %e, "判分失败，降级继续");
            crate::models::AnalysisRow {
                id: 0,
                provider: None,
                model: None,
                tags: None,
                difficulty: None,
                ref_answer: None,
                score: None,
                feedback: Some(format!("（本回答判分失败：{e}，已记录回答，可重新判卷或到题目详情补分析）")),
                answer_snapshot: None,
                created_at: chrono::Utc::now(),
            }
        }
    };
    append_drill_message(
        pool,
        uid,
        drill_id,
        "ai",
        "score",
        analysis.feedback.as_deref().unwrap_or(""),
        analysis.score,
        analysis.difficulty,
        analysis.feedback.as_deref(),
        None,
    )
    .await?;
    Ok(analysis)
}

/// M4（ADR-0023 D2）：单次流式输出中的内联 REPORT 落库。
/// 持久化语义与 grade_and_record 完全一致（题库行、my_answer 同步、analyses、标签、复习入队、积分事件、
/// score 消息），仅省去独立 LLM 判分调用——数据由两段式协议的 REPORT 段提供。
async fn record_inline_analysis(
    pool: &sqlx::PgPool,
    event_bus: &crate::events::EventBus,
    uid: i64,
    config: &settings::LlmConfig,
    drill_id: i64,
    content: &str,
    main_answer: &str,
    probe_answer: Option<&str>,
    report: &Value,
) -> Result<(Option<i32>, Option<String>), AppError> {
    // 题目行：续接落库时 sink_ai_question 已创建，此处按同键解析兜底
    let qid: i64 = match sqlx::query_scalar(
        "SELECT id FROM questions WHERE user_id=$1 AND drill_id=$2 AND source='ai_drill' AND content=$3 ORDER BY id DESC LIMIT 1",
    )
    .bind(uid)
    .bind(drill_id)
    .bind(content)
    .fetch_optional(pool)
    .await?
    {
        Some(id) => id,
        None => sink_ai_question(pool, uid, drill_id, content).await?,
    };

    // my_answer 同步 + 回答历史 + 训练轮次关联（与 grade_and_record append_answer 分支一致）
    let _ = sqlx::query("UPDATE questions SET my_answer=$2 WHERE id=$1")
        .bind(qid)
        .bind(main_answer)
        .execute(pool)
        .await;
    crate::routes::questions::record_answer(pool, qid, "interview", main_answer).await?;
    if let Ok(sink_round) = ensure_ai_round(pool, uid).await {
        let _ = crate::routes::questions::link_round(pool, qid, sink_round).await;
    }
    if let Some(p_ans) = probe_answer {
        let followup_qid: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM questions WHERE drill_id=$1 AND user_id=$2 AND parent_id=$3 ORDER BY id DESC LIMIT 1",
        )
        .bind(drill_id)
        .bind(uid)
        .bind(qid)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if let Some(f_qid) = followup_qid {
            let _ = sqlx::query("UPDATE questions SET my_answer=$2 WHERE id=$1")
                .bind(f_qid)
                .bind(p_ans)
                .execute(pool)
                .await;
            let _ = crate::routes::questions::record_answer(pool, f_qid, "interview", p_ans).await;
        }
    }

    // analyses 行
    let score = report["score"].as_i64().map(|v| v as i32);
    let feedback = report["feedback"].as_str().unwrap_or("").to_string();
    let difficulty = report["difficulty"].as_i64().map(|v| v as i32);
    let ref_answer = report["ref_answer"].as_str().unwrap_or("");
    let tags: Vec<String> =
        serde_json::from_value(report["tags"].clone()).unwrap_or_default();
    let provider = settings::provider_of(&config.base_url);
    let tags_json = serde_json::to_value(&tags)?;
    sqlx::query(
        "INSERT INTO analyses(question_id, provider, model, tags, difficulty, ref_answer, score, feedback, raw, answer_snapshot)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(qid)
    .bind(&provider)
    .bind(&config.model)
    .bind(&tags_json)
    .bind(difficulty)
    .bind(ref_answer)
    .bind(score)
    .bind(&feedback)
    .bind(json!({ "inline": true, "protocol": "m4_answer_flow" }))
    .bind(Some(main_answer).filter(|s| !s.trim().is_empty()))
    .execute(pool)
    .await?;

    // 难度/技能归属更新（内联报告不含 skill_path/question_type，COALESCE 保留既有值）
    let _ = sqlx::query(
        "UPDATE questions SET difficulty=COALESCE($2, difficulty) WHERE id=$1",
    )
    .bind(qid)
    .bind(difficulty)
    .execute(pool)
    .await;

    crate::routes::questions::attach_tags(pool, uid, qid, &tags).await?;
    crate::routes::questions::enqueue_review(pool, qid).await?;

    // 积分事件（监听器发放训练积分；时序与 grade_question 相同）
    let _ = event_bus.dispatch(crate::events::DomainEvent::AiSinkQuestionGraded {
        user_id: uid,
        drill_id,
        question_id: qid,
    }).await;

    // score 消息（错题本/统计口径来源）
    append_drill_message(
        pool,
        uid,
        drill_id,
        "ai",
        "score",
        &feedback,
        score,
        difficulty,
        Some(&feedback),
        None,
    )
    .await?;

    Ok((score, Some(feedback)))
}

/// 系统公司「模拟面试」/ 批次「AI 训练」/ 轮次「AI 生成」（首次沉淀时自动建）
async fn ensure_ai_round(pool: &sqlx::PgPool, uid: i64) -> Result<i64, AppError> {
    // 系统公司「模拟面试」按用户隔离（companies(user_id,name) 唯一）；
    // is_system 显式标记 + 冲突兜底（ADR-0014 §16/§17：系统公司必须被统计/列表排除）
    let company_id: i64 = sqlx::query_scalar(
        "INSERT INTO companies(user_id, name, is_system) VALUES($1,'模拟面试',true)
         ON CONFLICT (user_id, name) DO UPDATE SET name=EXCLUDED.name, is_system=true RETURNING id",
    )
    .bind(uid)
    .fetch_one(pool)
    .await?;
    let session_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE user_id=$1 AND company_id=$2 AND department='AI 训练' LIMIT 1",
    )
    .bind(uid)
    .bind(company_id)
    .fetch_optional(pool)
    .await?;
    let session_id = match session_id {
        Some(id) => id,
        None => sqlx::query_scalar(
            "INSERT INTO sessions(user_id, company_id, department, position, status) VALUES($1,$2,'AI 训练','AI 模拟','ongoing') RETURNING id",
        )
        .bind(uid)
        .bind(company_id)
        .fetch_one(pool)
        .await?,
    };
    let round_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM rounds WHERE session_id=$1 AND name='AI 生成' LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    let round_id = match round_id {
        Some(id) => id,
        None => sqlx::query_scalar(
            "INSERT INTO rounds(session_id, name, sort_order, passed) VALUES($1,'AI 生成',0,'pending') RETURNING id",
        )
        .bind(session_id)
        .fetch_one(pool)
        .await?,
    };
    Ok(round_id)
}

/// 取某训练题最新分析（幂等复用）
async fn fetch_q_analysis(pool: &sqlx::PgPool, qid: i64) -> Result<crate::models::AnalysisRow, AppError> {
    let row = sqlx::query_as::<_, crate::models::AnalysisRow>(
        "SELECT id, provider, model, tags, difficulty, ref_answer, score, feedback, answer_snapshot, created_at
         FROM analyses WHERE question_id=$1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(qid)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// 加载简历上下文：优先 parsed；若仅有原文则截取 1200 字摘要（不静默发起解析，不回写 parsed）。
async fn load_resume_context(pool: &sqlx::PgPool, uid: i64, application_id: Option<i64>) -> Result<Option<String>, AppError> {
    #[derive(sqlx::FromRow)]
    struct ResumeBits {
        parsed: Option<Value>,
    }
    let bits: Option<ResumeBits> = if let Some(aid) = application_id {
        sqlx::query_as(
            "SELECT r.parsed FROM applications a
             JOIN resumes r ON r.id = a.resume_id
             WHERE a.id = $1 AND a.user_id = $2",
        )
        .bind(aid)
        .bind(uid)
        .fetch_optional(pool)
        .await?
    } else {
        None
    };
    let bits = match bits {
        Some(b) => b,
        None => sqlx::query_as(
            "SELECT parsed FROM resumes WHERE user_id=$1 AND NOT is_archived ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(uid)
        .fetch_optional(pool)
        .await?
        .unwrap_or(ResumeBits { parsed: None }),
    };
    if let Some(p) = bits.parsed.filter(|v| v.is_object() && v.as_object().map(|o| !o.is_empty()).unwrap_or(false)) {
        let compact = crate::contracts::interview_prep::compact_parsed_resume(&p);
        if !compact.trim().is_empty() {
            return Ok(Some(compact));
        }
    }
    // 无解析结果时不灌原文，由装配层按岗位名称通用降级
    Ok(None)
}

fn delta_event(v: &Value) -> Event {
    Event::default().event("delta").data(v.to_string())
}
fn meta_event(v: &Value) -> Event {
    Event::default().event("meta").data(v.to_string())
}

#[derive(Deserialize)]
pub struct DossierMatchReq {
    pub keywords: Option<String>,
    pub position: Option<String>,
    pub application_id: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct MatchedQuestion {
    pub id: i64,
    pub content: String,
    pub company: Option<String>,
    pub tags: Vec<String>,
    pub last_score: Option<i32>,
    pub match_reason: String,
}

#[derive(serde::Serialize)]
pub struct DossierMatchResp {
    pub matched_tags: Vec<String>,
    pub questions: Vec<MatchedQuestion>,
}

#[tracing::instrument(skip_all)]
pub async fn match_dossier_questions(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<DossierMatchReq>,
) -> Result<Json<DossierMatchResp>, AppError> {
    let mut terms: Vec<String> = Vec::new();
    if let Some(kw) = req.keywords {
        for part in kw.split(|c: char| c.is_whitespace() || c == ',' || c == '，' || c == '、' || c == '/') {
            let p = part.trim();
            if p.len() >= 2 {
                terms.push(p.to_lowercase());
            }
        }
    }
    if let Some(pos) = req.position {
        for part in pos.split(|c: char| c.is_whitespace() || c == '/' || c == '-') {
            let p = part.trim();
            if p.len() >= 2 {
                terms.push(p.to_lowercase());
            }
        }
    }
    if let Some(app_id) = req.application_id {
        let jd: Option<String> = sqlx::query_scalar(
            "SELECT p.jd_text FROM applications a JOIN positions p ON p.id=a.position_id WHERE a.id=$1 AND a.user_id=$2"
        )
        .bind(app_id)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(jd_text) = jd {
            for word in ["redis", "mysql", "kafka", "jvm", "spring", "并发", "分布式", "微服务", "架构", "设计模式", "算法", "网络", "linux", "docker", "k8s"] {
                if jd_text.to_lowercase().contains(word) && !terms.contains(&word.to_string()) {
                    terms.push(word.to_string());
                }
            }
        }
    }

    #[derive(sqlx::FromRow)]
    struct RawQ {
        id: i64,
        content: String,
        company: Option<String>,
        tags: Vec<String>,
        last_score: Option<i32>,
        analyzed: bool,
    }

    let rows = sqlx::query_as::<_, RawQ>(
        r#"
        SELECT q.id, q.content,
               (SELECT c.name FROM companies c JOIN positions p ON p.company_id=c.id JOIN applications a ON a.position_id=p.id JOIN rounds r ON r.application_id=a.id WHERE r.id=q.round_id) AS company,
               COALESCE(array_agg(t.name) FILTER (WHERE t.name IS NOT NULL), '{}') AS tags,
               (SELECT a.score FROM analyses a WHERE a.question_id=q.id AND a.score IS NOT NULL ORDER BY a.created_at DESC LIMIT 1) AS last_score,
               EXISTS(SELECT 1 FROM analyses a WHERE a.question_id=q.id) AS analyzed
        FROM questions q
        LEFT JOIN question_tags qt ON qt.question_id=q.id
        LEFT JOIN tags t ON t.id=qt.tag_id
        WHERE q.user_id=$1 AND q.parent_id IS NULL
        GROUP BY q.id
        ORDER BY q.created_at DESC
        LIMIT 200
        "#
    )
    .bind(user.0)
    .fetch_all(&state.pool)
    .await?;

    let mut matched_tags_set = std::collections::HashSet::new();
    let mut scored_questions: Vec<(i32, MatchedQuestion)> = Vec::new();

    for q in rows {
        let content_lower = q.content.to_lowercase();
        let mut score = 0;
        let mut reasons = Vec::new();

        for term in &terms {
            if content_lower.contains(term) {
                score += 30;
                reasons.push(format!("题干匹配「{term}」"));
            }
            for tag in &q.tags {
                if tag.to_lowercase().contains(term) {
                    score += 40;
                    matched_tags_set.insert(tag.clone());
                    reasons.push(format!("命中标签「{tag}」"));
                }
            }
        }

        if score > 0 {
            if q.analyzed {
                score += 15;
            }
            if let Some(s) = q.last_score {
                if s < 60 {
                    score += 25;
                    reasons.push(format!("薄弱考点（历史 {s}分）"));
                }
            }
            let reason_str = if reasons.is_empty() {
                "关联度匹配".to_string()
            } else {
                reasons.join(" · ")
            };
            scored_questions.push((
                score,
                MatchedQuestion {
                    id: q.id,
                    content: q.content,
                    company: q.company,
                    tags: q.tags,
                    last_score: q.last_score,
                    match_reason: reason_str,
                }
            ));
        }
    }

    scored_questions.sort_by(|a, b| b.0.cmp(&a.0));
    let questions: Vec<MatchedQuestion> = scored_questions.into_iter().take(8).map(|x| x.1).collect();
    let mut matched_tags: Vec<String> = matched_tags_set.into_iter().collect();
    matched_tags.sort();

    Ok(Json(DossierMatchResp {
        matched_tags,
        questions,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 评审 P0：连续 role:user 消息必须合并为单条 Content（部分端点拒绝/误解多段 user 输入）。
    #[test]
    fn turn_messages_merges_context_history_task_into_single_user_message() {
        let msgs = turn_messages(
            "你是面试官",
            "【本场信息】\n岗位：后端",
            &["讲 HashMap".to_string(), "数组加链表".to_string()],
            "请出下一题",
        );
        assert_eq!(msgs.len(), 2, "system + 单条 user");
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        let content = msgs[1]["content"].as_str().unwrap();
        assert!(content.starts_with("【本场信息】"), "context 在前：{content}");
        assert!(content.contains("【对话历史】\n讲 HashMap\n数组加链表"));
        assert!(content.ends_with("【当前任务】\n请出下一题"));
        // input 数组里不应再出现连续 user（build_body 只会把这条 user 转为 1 个 input 元素）
    }

    #[test]
    fn turn_messages_empty_history_placeholder() {
        let msgs = turn_messages("sys", "", &[], "第一题任务");
        let content = msgs[1]["content"].as_str().unwrap();
        assert!(content.contains("【对话历史】\n（暂无）"), "空历史给占位：{content}");
        assert!(!content.starts_with("\n"), "context 为空不产生前导空行：{content}");
    }

    /// 链式模式：携带 context 与最新回答 + 任务，不重放对话历史。
    #[test]
    fn chain_turn_messages_carries_context_but_not_history() {
        let msgs = chain_turn_messages("sys", "我的回答内容", "请出下一题", Some("【本场信息】\n岗位：后端"));
        assert_eq!(msgs.len(), 2);
        let content = msgs[1]["content"].as_str().unwrap();
        assert!(content.contains("【本场信息】\n岗位：后端"));
        assert!(content.contains("【候选人最新回答】\n我的回答内容"));
        assert!(content.contains("【当前任务】\n请出下一题"));
        assert!(!content.contains("【对话历史】"), "链式模式不重放历史");
    }

    /// 链式错误签名：previous_response 未留存 / 过期 / 不存在应命中；普通错误不误伤。
    #[test]
    fn chain_error_signatures() {
        assert!(looks_like_chain_error("LLM 返回 400: Previous response with id 'f0dbb153' not found."));
        assert!(looks_like_chain_error("LLM 返回 404: previous_response_id 不存在"));
        assert!(looks_like_chain_error("LLM 返回 400: response expired after 7 days"));
        assert!(!looks_like_chain_error("LLM 返回 429: rate limit exceeded"));
        assert!(!looks_like_chain_error("LLM 流空闲超时"));
        // 含 response 但无失效语义的普通 400 不应误判（避免多余重试）
        assert!(!looks_like_chain_error("LLM 返回 400: invalid model name in response request"));
    }
}

// ==================== 面试官笔记（V6-M3，ADR-0023 D3） ====================
// 一键预读：关联投递 JD + 简历 parsed（可选关联真实轮次真题与回答）→ 四段结构化笔记
// 落 drills.interview_state，注入本场全程会话上下文。手动触发 + 受理幂等（ai_jobs 去重）。
// 与考官题本（dossier）互补不互替：题本管问什么，笔记管问谁。

/// 装配备课输入：(紧凑文本, 规则兜底原料, sources 原始引用 JSON)
async fn assemble_interview_prep_context(
    pool: &sqlx::PgPool,
    uid: i64,
    did: i64,
) -> Result<(String, contracts::interview_prep::RuleFacts, Value), AppError> {
    let drill: (Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT position, direction, application_id FROM drills WHERE id=$1 AND user_id=$2",
    )
    .bind(did)
    .bind(uid)
    .fetch_one(pool)
    .await?;
    let (position, direction, application_id) = drill;

    let mut company = String::new();
    let mut jd_excerpt: Option<String> = None;
    if let Some(aid) = application_id {
        let row: (Option<String>, Option<Value>) = sqlx::query_as(
            "SELECT c.name, p.jd_interpret FROM applications a
             LEFT JOIN positions p ON p.id = a.position_id
             LEFT JOIN companies c ON c.id = p.company_id
             WHERE a.id=$1 AND a.user_id=$2",
        )
        .bind(aid)
        .bind(uid)
        .fetch_one(pool)
        .await?;
        company = row.0.unwrap_or_default();
        jd_excerpt = row.1.as_ref().and_then(crate::contracts::interview_prep::compact_jd_interpret);
    }

    // 简历 parsed（复用原简历拷打加载逻辑：投递快照优先，否则未归档工作副本）
    let resume = load_resume_context(pool, uid, application_id).await?;

    // 关联真实轮次真题与用户回答（排除 AI 陪练沉淀题；最多 8 条防 prompt 失控）
    let qas: Vec<(String, Option<String>)> = match application_id {
        Some(aid) => sqlx::query_as(
            "SELECT q.content, q.my_answer FROM questions q
             JOIN rounds r ON r.id = q.round_id
             WHERE r.application_id=$1 AND q.drill_id IS NULL
             ORDER BY q.id DESC LIMIT 8",
        )
        .bind(aid)
        .fetch_all(pool)
        .await?,
        None => Vec::new(),
    };

    let resume_excerpt = resume.filter(|s| !s.trim().is_empty());
    let pos_label = position.clone().unwrap_or_default();
    let resume_block = resume_excerpt.clone().unwrap_or_else(|| {
        if pos_label.is_empty() {
            "（简历未解析：按通用考点备课，不针对履历深挖）".into()
        } else {
            format!("（简历未解析：按岗位「{pos_label}」通用备课，不针对履历深挖）")
        }
    });
    let jd_block = jd_excerpt.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
        if pos_label.is_empty() {
            "（该岗尚未 JD 解读：按通用考点备课）".into()
        } else {
            format!("（该岗尚未 JD 解读：按岗位「{pos_label}」通用备课）")
        }
    });

    let mut c = String::from("【目标岗位 JD 解读】\n");
    c += &jd_block;
    c += "\n\n【候选人简历要点】\n";
    c += &resume_block;
    if !qas.is_empty() {
        c += "\n\n【关联真实轮次真题与用户回答】\n";
        for (i, (q, a)) in qas.iter().enumerate() {
            c += &format!(
                "{}. 真题：{}\n   用户当时回答：{}\n",
                i + 1,
                crate::observe::truncate_chars(q, 160),
                a.as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|a| crate::observe::truncate_chars(a, 200))
                    .unwrap_or_else(|| "（无回答记录）".to_string()),
            );
        }
    }

    let facts = contracts::interview_prep::RuleFacts {
        position: position.unwrap_or_default(),
        company,
        keywords: direction.into_iter().collect(),
        resume_excerpt: resume_excerpt.clone(),
        round_topics: qas.iter().map(|(q, _)| crate::observe::truncate_chars(q, 60)).collect(),
        skill_keywords: harvest_skill_keywords(pool, application_id).await?,
    };

    // sources 原始引用随笔记落库（D3：解析不全时保留原始输入引用）
    let sources = json!({
        "jd_excerpt": jd_excerpt,
        "resume_excerpt": resume_excerpt,
        "round_qas": qas.iter().map(|(q, a)| json!({
            "question": crate::observe::truncate_chars(q, 160),
            "answer_excerpt": a.as_deref().map(str::trim).filter(|s| !s.is_empty())
                .map(|a| crate::observe::truncate_chars(a, 200)),
        })).collect::<Vec<_>>(),
    });

    Ok((c, facts, sources))
}

/// 规则提取技能/项目关键字（ADR-0023 D3 兑底原料）：关联真实轮次真题所挂的技能点名称
async fn harvest_skill_keywords(
    pool: &sqlx::PgPool,
    application_id: Option<i64>,
) -> Result<Vec<String>, AppError> {
    let Some(aid) = application_id else {
        return Ok(Vec::new());
    };
    // 真实轮次真题（排除 AI 陪练沉淀题）直接挂的 skill_id + 经 question_skills 多对多挂的技能点
    #[derive(sqlx::FromRow)]
    struct Row {
        name: String,
    }
    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT DISTINCT s.name FROM skills s
        WHERE s.id IN (
            SELECT q.skill_id FROM questions q
            JOIN rounds r ON r.id = q.round_id
            WHERE r.application_id=$1 AND q.drill_id IS NULL AND q.skill_id IS NOT NULL
            UNION
            SELECT qs.skill_id FROM question_skills qs
            JOIN questions q ON q.id = qs.question_id
            JOIN rounds r ON r.id = q.round_id
            WHERE r.application_id=$1 AND q.drill_id IS NULL
        )
        LIMIT 8
        "#,
    )
    .bind(aid)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

/// 发起面试官笔记生成（受理幂等：同场次同出口 running 去重；完成后覆盖写 interview_state）
#[tracing::instrument(skip_all)]
async fn start_interview_prep(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(did): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let kind: String = sqlx::query_scalar("SELECT kind FROM drills WHERE id=$1 AND user_id=$2")
        .bind(did)
        .bind(user.0)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    if kind != "interview" {
        return Err(AppError::BadRequest("面试官笔记仅适用于模拟面试场次".to_string()));
    }
    let config = settings::require_llm(&state.pool, user.0).await?;

    let job = match state.ai_jobs.start(user.0, "interview_prep", did) {
        AiStart::AlreadyRunning(j) => return Ok(Json(json!({ "job_id": j.id, "status": j.status }))),
        AiStart::Started(j) => j,
    };
    let st = state.clone();
    let uid = user.0;
    state.ai_jobs.spawn_guarded(job.clone(), async move {
        let (user_content, facts, sources) = assemble_interview_prep_context(&st.pool, uid, did).await?;
        let contract = contracts::interview_prep::InterviewPrep::new(user_content, facts);
        let (result, _meta) = contracts::execute(&config, &st.pool, uid, &contract).await?;
        let notes = match result {
            contracts::ContractOut::Structured(notes) => serde_json::to_value(&notes)?,
            // 结构必需出口：能力位闸门/解析层已显式报错，不会走到 Text 分支（防御性兜底）
            contracts::ContractOut::Text(_) => {
                return Err(AppError::BadRequest("该出口不支持纯文本评审模式".to_string()))
            }
        };
        // 合并 sources 原始引用 + 元数据后落库
        let mut payload = notes;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("sources".into(), sources);
            obj.insert("generated_at".into(), json!(chrono::Utc::now().to_rfc3339()));
        }
        sqlx::query("UPDATE drills SET interview_state=$1 WHERE id=$2 AND user_id=$3")
            .bind(&payload)
            .bind(did)
            .bind(uid)
            .execute(&st.pool)
            .await?;
        tracing::info!(event = "interview_prep.done", user_id = uid, drill_id = did, "面试官笔记已落库");
        Ok::<(), AppError>(())
    });
    tracing::info!(user_id = user.0, job_id = job.id, drill_id = did, "发起面试官笔记任务");
    Ok(Json(json!({ "job_id": job.id, "status": job.status })))
}
