//! v5 M3: 考官题本（Interviewer Dossier）与动态追问模拟面试集成测试

mod common;

use axum::http::Method;
use common::llm_mock::LlmMock;
use common::TestApp;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn test_drill_creation_with_dossier() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 创建公司与真实题目
    let (s, c_res) = app.req(Method::POST, "/api/companies", Some(json!({ "name": "星舰科技" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let cid = c_res["id"].as_i64().unwrap();

    let (s, p_res) = app.req(
        Method::POST,
        &format!("/api/companies/{cid}/positions"),
        Some(json!({ "title": "后端架构师" })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let _pid = p_res["id"].as_i64().unwrap();

    let (s, a_res) = app.req(
        Method::POST,
        "/api/applications",
        Some(json!({ "company_id": cid, "position": "后端架构师" })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let aid = a_res["id"].as_i64().unwrap();

    let (s, r_res) = app.req(
        Method::POST,
        &format!("/api/applications/{aid}/rounds"),
        Some(json!({ "name": "一面技术深挖" })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let rid = r_res["id"].as_i64().unwrap();

    let (s, q_res) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "Redis 分布式锁在 Redlock 算法下如何应对时钟跳跃与 GC 停顿？",
            "my_answer": "通过延长锁租期与多数派选举校验"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let qid = q_res["id"].as_i64().unwrap();

    // 写入该题的参考答案
    sqlx::query(
        "INSERT INTO analyses(question_id, provider, model, tags, difficulty, ref_answer, score, feedback, raw, answer_snapshot)
         VALUES($1, 'openai', 'gpt-4o', '[\"分布式锁\",\"Redis\"]', 4, 'Redlock 依赖单调时钟与 NPC 规避策略，或使用 Fencing Token 递增令牌', 85, '回答切中要点', '{}', 'test')"
    )
    .bind(qid)
    .execute(&app.pool)
    .await
    .unwrap();

    // 2. 发起携带考官题本 (dossier) 的模拟面试
    let (s, d_res) = app.req(
        Method::POST,
        "/api/drills",
        Some(json!({
            "kind": "interview",
            "title": "星舰科技·架构师专属题本模拟面试",
            "position": "后端架构师",
            "target_questions": 3,
            "dossier": {
                "summary": "重点深挖分布式锁安全性与时钟漂移边界",
                "question_ids": [qid]
            }
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    let drill_id = d_res["id"].as_i64().unwrap();

    // 3. 查询详情，验证题本已自动补充 content 与 ref_answer
    let (s, d_detail) = app.req(Method::GET, &format!("/api/drills/{drill_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let dossier = &d_detail["dossier"];
    assert_eq!(dossier["summary"].as_str().unwrap(), "重点深挖分布式锁安全性与时钟漂移边界");
    let qs = dossier["questions"].as_array().unwrap();
    assert_eq!(qs.len(), 1);
    assert!(qs[0]["content"].as_str().unwrap().contains("Redlock"));
    assert!(qs[0]["ref_answer"].as_str().unwrap().contains("Fencing Token"));
}

#[tokio::test]
async fn test_drill_probe_rhythm_and_intent() {
    let mock = LlmMock::start();
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 配置 LLM
    let (s, _) = app.req(
        Method::PUT,
        "/api/settings/llm-config",
        Some(json!({
            "providers": [{ "id": "p1", "name": "Mock", "base_url": mock.base_url(), "api_key": "" }],
            "models": [{
                "id": "m1", "provider_id": "p1", "name": "mock",
                "context_length": 128000,
                "caps": { "structured_output": true, "web_search": false },
                "advanced": { "store": false, "reasoning_effort": "none" }
            }],
            "active_model_id": "m1"
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // 1. 创建 2 题的模拟面试
    let (s, d_res) = app.req(
        Method::POST,
        "/api/drills",
        Some(json!({
            "kind": "interview",
            "title": "动态追问节奏测试",
            "position": "资深后端工程师",
            "target_questions": 2,
            "dossier": {
                "summary": "深入考核数据库事务与 MVCC"
            }
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    let drill_id = d_res["id"].as_i64().unwrap();

    // 2. 第一轮（开始）：Mock 返回第 1 道主问题
    mock.queue_stream(vec!["请解释 MySQL InnoDB 中 Undo Log 与 Read View 是如何协同实现 RC / RR 隔离级别的？".to_string()]);

    let (s, sse_out) = app.req_raw(
        Method::POST,
        &format!("/api/drills/{drill_id}/messages"),
        Some(json!({ "content": "准备好了，开始面试" })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    assert!(sse_out.contains("Undo Log"));

    // 检查第 1 题的 intent 为 main_question
    let (s, d_detail) = app.req(Method::GET, &format!("/api/drills/{drill_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let msgs = d_detail["messages"].as_array().unwrap();
    let q1 = msgs.iter().find(|m| m["kind"] == "question").unwrap();
    assert_eq!(q1["intent"].as_str().unwrap(), "main_question");

    // 3. 用户回答第 1 题：M4 两段式——模型自主决定深挖（PROBE 元数据 + 追问题干）
    // 哨兵行以换行结束（协议要求元数据独立成行），随后正文照常流式透传
    mock.queue_stream(vec![
        r#"<<<PROBE>>>{"anchor_keyword":"Read View","reason":"depth_probe"}
"#.to_string(),
        "针对你提到的 Read View，在长事务未提交的情况下，Undo Log 链条过长会引发什么线上问题？如何观测？".to_string(),
    ]);

    let (s, sse_out2) = app.req_raw(
        Method::POST,
        &format!("/api/drills/{drill_id}/messages"),
        Some(json!({ "content": "RC 在每次 SELECT 时生成 Read View，RR 在第一次 SELECT 时生成" })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    assert!(sse_out2.contains("Undo Log 链条过长"));

    // 检查追问题的 kind 为 probe，intent 为 followup_probe
    let (s, d_detail2) = app.req(Method::GET, &format!("/api/drills/{drill_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let msgs2 = d_detail2["messages"].as_array().unwrap();
    let q2 = msgs2.iter().find(|m| m["kind"] == "probe" || m["intent"] == "followup_probe").unwrap();
    assert_eq!(q2["kind"].as_str().unwrap(), "probe");
    assert_eq!(q2["intent"].as_str().unwrap(), "followup_probe");
}
