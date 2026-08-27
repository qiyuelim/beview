//! V6-M4（ADR-0023 D2）：对话控制流/报告流分离。
//!
//! 验收：
//! - 提交回答后首个流式 token 为续接（题目正文先流出，哨兵行绝不泄漏到直播画面）；
//! - 评分点评降格为后置报告流：feedback 事件在 delta 之后到达并原位填充；
//! - 追问携带 anchor + 封闭理由枚举元数据落库；枚举外输出被 schema 拒绝（剥除徽章、保留追问）；
//! - 评分落库时序语义不变：analyses/score 消息/错题本口径与旧链路一致。

use axum::http::Method;
use serde_json::json;

mod common;
use common::TestApp;

async fn setup_drill_with_first_question(app: &TestApp, mock: &common::llm_mock::LlmMock) -> i64 {
    let (_, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 2 })))
        .await;
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["第一题：请讲讲 HashMap 的实现原理。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    did
}

/// 收尾轮两段式：复盘正文 delta 先流出，feedback 事件后置；评分落库与错题本口径不变。
#[tokio::test]
async fn answer_continuation_streams_before_feedback_event() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let did = setup_drill_with_first_question(&app, &mock).await;

    // 收尾轮（target=2，第二答达标）：单次流 = 复盘正文 + REPORT(30)
    mock.queue_stream(vec![
        "# 🎯 全场复盘报告\n\n## 🚀 四、靶向强化建议\n补并发扩容。".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"哈希\"],\"difficulty\":3,\"ref_answer\":\"数组+链表\",\"score\":30,\"feedback\":\"回答不完整\"}".to_string(),
    ]);
    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "用数组和链表实现" })))
        .await;
    assert!(sc.is_success(), "{sc}");

    // ① 哨兵绝不泄漏进直播画面
    assert!(!body.contains("<<<"), "哨兵不得出现在 SSE 流: {body}");
    // ② 续接先流出、评分后置（位置断言）
    let last_delta = body.rfind("event: delta").expect("应有 delta");
    let feedback = body.find("event: feedback").expect("应有 feedback 事件");
    assert!(last_delta < feedback, "续接 delta 必须先于 feedback 事件");
    // ③ 点评原位填充的数据在事件里
    assert!(body.contains("回答不完整"), "feedback 应携带点评");

    // ④ 落库时序不变：score 消息 + analyses + 错题本口径一致
    let (_, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let msgs = detail["messages"].as_array().cloned().unwrap_or_default();
    let scores: Vec<&serde_json::Value> = msgs.iter().filter(|m| m["kind"] == "score").collect();
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0]["score"], 30);

    let (_, wrong) = app.req(Method::GET, "/api/review/wrong", None).await;
    let in_wrong = wrong
        .as_array()
        .map(|a| a.iter().any(|x| x["content"].as_str().unwrap_or("").contains("HashMap")))
        .unwrap_or(false);
    assert!(in_wrong, "低分题应进错题本");
}

/// 追问轮：PROBE 元数据（锚点+封闭理由枚举）落库供徽章渲染；枚举外输出被拒绝但追问保留。
#[tokio::test]
async fn probe_meta_persists_and_invalid_enum_rejected() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let did = setup_drill_with_first_question(&app, &mock).await;

    // 回答 Q1 -> 模型自主决定深挖：合法枚举 meta + 题干
    mock.queue_stream(vec![
        r#"<<<PROBE>>>{"anchor_keyword":"并发扩容","reason":"contradiction"}"#.to_string(),
        "追问：你说数组加链表，那并发扩容时迭代器会怎样？".to_string(),
    ]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "数组加链表" }))).await;

    let (_, d1) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let msgs = d1["messages"].as_array().cloned().unwrap_or_default();
    let probes: Vec<&serde_json::Value> = msgs.iter().filter(|m| m["kind"] == "probe").collect();
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0]["meta"]["anchor_keyword"], "并发扩容", "锚点应落库: {}", probes[0]);
    assert_eq!(probes[0]["meta"]["reason"], "contradiction", "封闭理由应落库");

    // 回答追问 -> 枚举外 reason：schema 拒绝（无 meta），追问本身保留
    mock.queue_stream(vec![
        r#"<<<PROBE>>>{"anchor_keyword":"扩容","reason":"自由发挥的非法值"}"#.to_string(),
        "再追问：那 rehash 过程呢？".to_string(),
    ]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "rehash 会迁移桶" }))).await;

    let (_, d2) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let msgs2 = d2["messages"].as_array().cloned().unwrap_or_default();
    let probes2: Vec<&serde_json::Value> = msgs2.iter().filter(|m| m["kind"] == "probe").collect();
    assert_eq!(probes2.len(), 2, "追问消息应保留");
    assert!(probes2[1].get("meta").map(|m| m.is_null() || m.as_object().map(|o| o.is_empty()).unwrap_or(false)).unwrap_or(true),
        "枚举外元数据应被 schema 拒绝（不落库）: {}", probes2[1]);
}
