//! V6-M3（ADR-0023 D3/D4）：面试官笔记 + 简历拷打退役。
//!
//! 覆盖 plan.md M3 验收：
//! - 一键预读生成四段笔记并持久化（含 sources 原始引用、重跑覆盖语义）；
//! - 残缺输入走规则兜底且保留原始引用（rule_backfilled 标记）；
//! - 笔记到达后续每轮 LLM 请求边界（send_message 的 user 消息含笔记内容）；
//! - resume_grill 场次清零、沉淀题无损（迁移同源 SQL 行为钉死）+ 创建路径拒绝。

use axum::http::Method;
use serde_json::json;

mod common;
use common::TestApp;

/// 建「投递 + 真实轮次 + 带回答真题」并返回 (aid, rid)
async fn seed_application_with_real_round(app: &TestApp) -> (i64, i64) {
    let aid = common::create_application(app, "备课公司", "Rust 架构师").await;
    // JD 文本挂在 position 上（create_application 内部已建 position）
    sqlx::query("UPDATE positions SET jd_text=$1 WHERE id=(SELECT position_id FROM applications WHERE id=$2)")
        .bind("负责高并发网关系统设计，要求熟悉 Tokio 异步生态与分布式一致性。")
        .bind(aid)
        .execute(&app.pool)
        .await
        .expect("写入 JD 失败");
    let rid = common::create_round(app, aid, "真实技术一面").await;
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({
                "round_id": rid,
                "content": "真实一面原题：mmap 零拷贝在网关里怎么用？",
                "my_answer": "当时只答出了 sendfile，没讲清 COW。"
            })),
        )
        .await;
    assert_eq!(s, 201, "建真实轮次真题失败: {v}");
    (aid, rid)
}

async fn create_interview_drill(app: &TestApp, body: serde_json::Value) -> i64 {
    let (s, v) = app.req(Method::POST, "/api/drills", Some(body)).await;
    assert_eq!(s, 200, "建场失败: {v}");
    v["id"].as_i64().unwrap()
}

/// M3-验收①：一键预读生成四段笔记落库；sources 保留真实轮次原始引用；完成后可重跑覆盖。
#[tokio::test]
async fn interview_prep_generates_and_persists_notes() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (aid, _rid) = seed_application_with_real_round(&app).await;
    let did = create_interview_drill(
        &app,
        json!({ "kind": "interview", "position": "Rust 架构师", "application_id": aid }),
    )
    .await;

    mock.queue_nonstream(
        r#"{"job_requirements":["LLM-MARKER-JD-要求"],"candidate_facts":["LLM-MARKER-简历事实"],
            "risk_signals":["LLM-MARKER-风险"],"next_followups":["LLM-MARKER-追问"]}"#,
    );
    let (sc, v) = app.req(Method::POST, &format!("/api/drills/{did}/interview_prep"), None).await;
    assert_eq!(sc, 200, "受理应成功: {v}");
    let job1 = v["job_id"].as_u64().unwrap();
    let done = common::wait_ai_job(&app, job1, 10_000).await;
    assert_eq!(done["status"], "done", "任务应成功: {done}");

    let (_, d) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let st = &d["interview_state"];
    assert!(st.is_object(), "interview_state 应落库: {d}");
    assert_eq!(st["job_requirements"][0], "LLM-MARKER-JD-要求");
    assert_eq!(st["risk_signals"][0], "LLM-MARKER-风险");
    // sources 原始引用：真实轮次真题进入 sources.round_qas
    assert!(
        st["sources"]["round_qas"][0]["question"].as_str().unwrap_or("").contains("mmap 零拷贝"),
        "应保留真实真题原始引用: {st}"
    );
    assert!(st["generated_at"].is_string());
    // 全 LLM 输出 → 无规则兜底标记（false 序列化时省略）
    assert!(st.get("rule_backfilled").is_none(), "全 LLM 时不应出现兜底标记: {st}");

    // 重跑覆盖：新任务、新结果
    mock.queue_nonstream(
        r#"{"job_requirements":["第二版要求"],"candidate_facts":["f"],"risk_signals":["r"],"next_followups":["u"]}"#,
    );
    let (sc2, v2) = app.req(Method::POST, &format!("/api/drills/{did}/interview_prep"), None).await;
    assert_eq!(sc2, 200);
    assert_ne!(v2["job_id"].as_u64(), Some(job1), "终态后重跑应创建新任务");
    common::wait_ai_job(&app, v2["job_id"].as_u64().unwrap(), 10_000).await;
    let (_, d2) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(d2["interview_state"]["job_requirements"][0], "第二版要求", "应覆盖旧笔记");
}

/// M3-验收②：残缺输入走规则兜底且保留原始引用（rule_backfilled=true，已有 LLM 段不覆盖）。
#[tokio::test]
async fn incomplete_input_falls_back_to_rules() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 不关联投递：无 JD / 无简历 / 无真实轮次
    let did = create_interview_drill(&app, json!({ "kind": "interview", "position": "后端工程师" })).await;

    // LLM 只回了 job_requirements，其余三段缺失
    mock.queue_nonstream(r#"{"job_requirements":["JD-A"],"candidate_facts":[],"risk_signals":[],"next_followups":[]}"#);
    let (sc, v) = app.req(Method::POST, &format!("/api/drills/{did}/interview_prep"), None).await;
    assert_eq!(sc, 200, "{v}");
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 10_000).await;

    let (_, d) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let st = &d["interview_state"];
    assert_eq!(st["job_requirements"][0], "JD-A", "已有段不被规则覆盖");
    assert!(st["candidate_facts"][0].as_str().unwrap_or("").contains("简历缺失"), "{st}");
    assert!(st["risk_signals"][0].as_str().unwrap_or("").contains("规则兜底"), "{st}");
    assert_eq!(st["rule_backfilled"], true, "应标记规则兜底: {st}");
}

/// M3-验收③：笔记到达后续每轮 LLM 请求边界（send_message 的 user 消息携带笔记内容）。
#[tokio::test]
async fn notes_reach_llm_request_boundary() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let did = create_interview_drill(&app, json!({ "kind": "interview" })).await;
    // 直接种入笔记（生成链路已由前两个测试覆盖）
    sqlx::query(
        "UPDATE drills SET interview_state=$1 WHERE id=$2",
    )
    .bind(json!({
        "job_requirements": ["REQ-MARKER-网关设计"],
        "candidate_facts": ["FACT-MARKER-三年Rust"],
        "risk_signals": ["RISK-MARKER-并发存疑"],
        "next_followups": ["FOLLOW-MARKER-追问锁"]
    }))
    .bind(did)
    .execute(&app.pool)
    .await
    .expect("种入笔记失败");

    mock.queue_stream(vec!["第一题：".to_string(), "请介绍你的项目。".to_string()]);
    let (sc, _body) = app
        .req_raw(
            Method::POST,
            &format!("/api/drills/{did}/messages"),
            Some(json!({ "content": "开始" })),
        )
        .await;
    assert!(sc.is_success(), "对话应成功");

    // 断言任一 LLM 请求体包含笔记内容（边界到达）
    let bodies = mock.request_bodies();
    let hit = bodies.iter().any(|b| {
        b.to_string().contains("REQ-MARKER-网关设计")
            && b.to_string().contains("FOLLOW-MARKER-追问锁")
    });
    assert!(hit, "面试官笔记应出现在 LLM 请求上下文中，实际请求体：{bodies:?}");
}

/// M3-验收④：resume_grill 创建路径拒绝（400）；退役清理 SQL 场次清零、消息级联删、沉淀题无损。
#[tokio::test]
async fn resume_grill_retired_and_legacy_cleanup() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 新建路径：kind=resume_grill 显式拒绝
    let (sc, v) = app.req(Method::POST, "/api/drills", Some(json!({ "kind": "resume_grill" }))).await;
    assert_eq!(sc, 400, "退役 kind 应被拒绝: {v}");
    assert!(v["error"].as_str().unwrap_or("").contains("ADR-0023"), "错误提示应指向决策依据: {v}");

    // 种入 legacy 形态数据（模拟迁移前的存量），再执行与迁移逐字一致的清理语句
    let uid: i64 = sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();
    let aid = common::create_application(&app, "遗留公司", "遗留岗").await;
    let rid = common::create_round(&app, aid, "挂载轮").await;
    let legacy_did: i64 = sqlx::query_scalar(
        "INSERT INTO drills(user_id, kind, title, status) VALUES($1,'resume_grill','legacy-grill','ongoing') RETURNING id",
    )
    .bind(uid)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO drill_messages(user_id, drill_id, role, kind, content) VALUES($1,$2,'ai','question','旧拷打题')")
        .bind(uid)
        .bind(legacy_did)
        .execute(&app.pool)
        .await
        .unwrap();
    // 沉淀题：挂在本场次下
    let qid: i64 = sqlx::query_scalar(
        "INSERT INTO questions(user_id, round_id, drill_id, content, source) VALUES($1,$2,$3,'沉淀的拷打题','manual') RETURNING id",
    )
    .bind(uid)
    .bind(rid)
    .bind(legacy_did)
    .fetch_one(&app.pool)
    .await
    .expect("种入沉淀题失败");

    // 退役拷打场次：删场次、消息级联删、沉淀题 drill_id 置空（FK ON DELETE SET NULL）
    sqlx::query("DELETE FROM drills WHERE kind = 'resume_grill'")
        .execute(&app.pool)
        .await
        .unwrap();

    // 场次清零
    let drills_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM drills WHERE kind='resume_grill'").fetch_one(&app.pool).await.unwrap();
    assert_eq!(drills_left, 0, "resume_grill 场次应清零");
    // 消息随场次级联删除
    let msgs_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM drill_messages WHERE drill_id=$1").bind(legacy_did).fetch_one(&app.pool).await.unwrap();
    assert_eq!(msgs_left, 0, "消息应级联删除");
    // 沉淀题无损、drill_id 置空（FK ON DELETE SET NULL）
    let (q_content, q_drill): (String, Option<i64>) =
        sqlx::query_as("SELECT content, drill_id FROM questions WHERE id=$1").bind(qid).fetch_one(&app.pool).await.unwrap();
    assert_eq!(q_content, "沉淀的拷打题");
    assert!(q_drill.is_none(), "drill_id 应置空保留题目");
}
