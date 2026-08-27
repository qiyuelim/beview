//! v5.5-M1（票03）：押题命中闭环度量。
//! 数据局限：旧 ingest 用了 source='manual'，无法回溯——仅新押题（source='predicted'）参与统计。

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

mod common;

use common::TestApp;

async fn setup_pos(app: &TestApp, name: &str, pos: &str) -> (i64, i64) {
    // (company_id, position_id)
    let (s, c) = app
        .req(
            Method::POST,
            "/api/companies",
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let cid = c["id"].as_i64().unwrap();
    let (s, p) = app
        .req(
            Method::POST,
            &format!("/api/companies/{cid}/positions"),
            Some(json!({ "title": pos })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    (cid, p["id"].as_i64().unwrap())
}

/// 押题入题库：直接复用真实 ingest 端点确保链路。
async fn ingest_predictions(app: &TestApp, pos_id: i64, items: &[(&str, &str)]) -> Vec<i64> {
    let qs: Vec<Value> = items
        .iter()
        .map(|(c, cat)| json!({ "content": c, "category": cat }))
        .collect();
    let (s, r) = app
        .req(
            Method::POST,
            &format!("/api/positions/{pos_id}/predict/ingest"),
            Some(json!({ "questions": qs })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "押题入题库失败: {r}");
    r["question_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect()
}

async fn grade(app: &TestApp, qid: i64, result: &str) {
    let (s, _) = app
        .req(
            Method::POST,
            &format!("/api/review/{qid}/grade"),
            Some(json!({ "result": result })),
        )
        .await;
    assert!(s.is_success());
}

#[tokio::test]
async fn empty_returns_structured_zeros() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (s, body) = app.req(Method::GET, "/api/stats/prediction-hit-rate", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["total"]["predicted_count"], 0);
    assert_eq!(body["total"]["reviewed_count"], 0);
    assert_eq!(body["total"]["hit_rate_percent"], 0.0);
    assert_eq!(body["by_position"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn per_position_breakdown_and_isolation() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_cid1, p1) = setup_pos(&app, "押题命中公司 A", "后端").await;
    let (_cid2, p2) = setup_pos(&app, "押题命中公司 B", "前端").await;

    // p1：4 题（3 记得 / 1 模糊）
    let p1_ids = ingest_predictions(
        &app,
        p1,
        &[
            ("p1-q1 Redis 持久化", "存储"),
            ("p1-q2 进程与线程", "操作系统"),
            ("p1-q3 TCP 握手", "网络"),
            ("p1-q4 B+ 树索引", "存储"),
        ],
    )
    .await;
    grade(&app, p1_ids[0], "remembered").await;
    grade(&app, p1_ids[1], "remembered").await;
    grade(&app, p1_ids[2], "remembered").await;
    grade(&app, p1_ids[3], "fuzzy").await;

    // p2：2 题（0 复习）—— 出现在 by_position 但样本量 0，命中率 0
    let p2_ids = ingest_predictions(
        &app,
        p2,
        &[("p2-q1 防抖节流", "前端"), ("p2-q2 VDOM 原理", "前端")],
    )
    .await;
    let _ = p2_ids;

    let (s, body) = app.req(Method::GET, "/api/stats/prediction-hit-rate", None).await;
    assert_eq!(s, StatusCode::OK);
    let total = &body["total"];
    assert_eq!(total["predicted_count"], 6);
    assert_eq!(total["reviewed_count"], 4);
    assert!((total["hit_rate_percent"].as_f64().unwrap() - 75.0).abs() < 0.5,
        "期望 ~75.0% 实际 {}%", total["hit_rate_percent"]);

    let by_pos = body["by_position"].as_array().unwrap();
    // 排序：reviewed DESC,  p1(4) 在前，p2(0) 在后
    assert_eq!(by_pos.len(), 2);
    assert_eq!(by_pos[0]["position_id"], p1);
    assert_eq!(by_pos[0]["reviewed_count"], 4);
    assert_eq!(by_pos[0]["predicted_count"], 4);
    assert!((by_pos[0]["hit_rate_percent"].as_f64().unwrap() - 75.0).abs() < 0.5);
    assert_eq!(by_pos[1]["position_id"], p2);
    assert_eq!(by_pos[1]["reviewed_count"], 0);
    assert_eq!(by_pos[1]["predicted_count"], 2);
    assert_eq!(by_pos[1]["hit_rate_percent"], 0.0);
}

#[tokio::test]
async fn predicted_questions_filter_in_list() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_cid, pos) = setup_pos(&app, "列表筛选公司", "后端").await;
    let ids = ingest_predictions(&app, pos, &[("list-q1 题目一", "存储")]).await;
    // 再创建一道普通手动题，应该不被 predicted 筛选命中
    let (_, rid) = common::setup_application_round(&app).await;
    let (s, q) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "普通手动题" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let manual_id = q["id"].as_i64().unwrap();

    let (s, list) = app
        .req(
            Method::GET,
            &format!("/api/questions?source=predicted&position_id={pos}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1, "应只命中押题题");
    assert_eq!(arr[0]["id"], ids[0]);
    let _ = manual_id;
}

#[tokio::test]
async fn hit_rate_is_row_level_isolated() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_cid, pos) = setup_pos(&app, "隔离公司 A", "后端").await;
    let a_ids = ingest_predictions(&app, pos, &[("A-题 1", "存储"), ("A-题 2", "网络")]).await;
    grade(&app, a_ids[0], "remembered").await;
    grade(&app, a_ids[1], "forgot").await;

    // 新用户 B 应看不到 A 的统计
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "hitbob", "password": "bobpass123" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, cookie) = app.login_as("hitbob", "bobpass123").await;
    let cookie = cookie.unwrap();
    let (s, body) = app
        .req_as(&cookie, Method::GET, "/api/stats/prediction-hit-rate", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["total"]["predicted_count"], 0);
    assert_eq!(body["by_position"].as_array().unwrap().len(), 0);
}
