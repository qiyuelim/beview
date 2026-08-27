//! v2 集成测试（TDD：在 public seam `build_api` 上测真实 HTTP + 真实测试库）。
//! 每片：先写「用户可…」的红测试，再实现到绿。

mod common;

use axum::http::Method;
use serde_json::json;
use common::TestApp;

/// 认证边界：未登录访问受保护接口返回 401（回归，ADR-0005）
#[tokio::test]
async fn unauthenticated_access_is_rejected() {
    let app = TestApp::setup().await;
    let (status, _) = app.req(Method::GET, "/api/questions", None).await;
    assert_eq!(status, 401, "未登录访问 /api/questions 应 401");
}

/// 首启建管理员：users 空时 /api/setup 可创建，重复创建被拒
#[tokio::test]
async fn first_run_setup_creates_admin_and_rejects_duplicate() {
    let app = TestApp::setup().await;
    let (s1, _) = app.setup_admin().await;
    assert!(s1.is_success(), "首次建管理员应成功");
    let (s2, _) = app.setup_admin().await;
    assert_eq!(s2, 409, "重复建管理员应 409");
}

/// 录入一道「写了我的回答」的题 → 自动进复习队 → 自评「记得」→ 下次复习推到未来
/// （M0 复习闭环的端到端规格：用户能把一道题加入复习并完成一次自评）
#[tokio::test]
async fn user_can_review_a_question_and_grade_it() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 建 投递->轮次->题目(带我的回答 => 可复习)
    let (_aid, rid) = common::setup_application_round(&app).await;
    let (sc, q) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "讲一下 HashMap", "my_answer": "数组+链表" })),
        )
        .await;
    assert!(sc.is_success(), "创建题目应成功");
    let qid = q["id"].as_i64().unwrap();

    // 可复习判定：写了我的回答 -> 自动入队
    let (_, queue) = app.req(Method::GET, "/api/review/queue", None).await;
    let ids: Vec<i64> = queue
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["question_id"].as_i64()).collect())
        .unwrap_or_default();
    assert!(ids.contains(&qid), "带我的回答的题应自动进入复习队列");

    // 自评「记得」：间隔放大、下次复习在将来
    let (sg, _) = app
        .req(Method::POST, &format!("/api/review/{qid}/grade"), Some(json!({ "result": "remembered" })))
        .await;
    assert!(sg.is_success(), "自评应成功");
    let queue2 = app.req(Method::GET, "/api/review/queue", None).await.1;
    let ids2: Vec<i64> = queue2
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["question_id"].as_i64()).collect())
        .unwrap_or_default();
    assert!(!ids2.contains(&qid), "自评「记得」后该题不应仍在今日队列");
}

/// 自评「忘了」→ 该题进入错题本（复习回流的钩子）
#[tokio::test]
async fn forgot_question_appears_in_wrong_book() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_aid, rid) = common::setup_application_round(&app).await;
    let (_, q) = app
        .req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "题目X", "my_answer": "答" })))
        .await;
    let qid = q["id"].as_i64().unwrap();

    app.req(Method::POST, &format!("/api/review/{qid}/grade"), Some(json!({ "result": "forgot" }))).await;

    let (_, wrong) = app.req(Method::GET, "/api/review/wrong", None).await;
    let ids: Vec<i64> = wrong
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["question_id"].as_i64()).collect())
        .unwrap_or_default();
    assert!(ids.contains(&qid), "自评「忘了」后应出现在错题本");
}

/// 造一轮次与题目，返回 (question_id, round_id)
async fn seed_question(app: &TestApp, content: &str, my_answer: Option<&str>) -> i64 {
    static N: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let aid = common::create_application(app, &format!("公司{n}"), "后端").await;
    let rid = common::create_round(app, aid, "一面").await;
    let (_, q) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({ "round_id": rid, "content": content, "my_answer": my_answer })),
    ).await;
    q["id"].as_i64().unwrap()
}

/// 用户能批量删除多道题目（级联删除分析）
#[tokio::test]
async fn user_can_bulk_delete_questions() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let q1 = seed_question(&app, "批量删除题A", Some("答")).await;
    let q2 = seed_question(&app, "批量删除题B", Some("答")).await;
    let q3 = seed_question(&app, "保留题C", Some("答")).await;

    let (sc, body) = app
        .req(Method::DELETE, "/api/questions", Some(json!({ "ids": [q1, q2] })))
        .await;
    assert!(sc.is_success(), "批量删除应成功, got {sc}");
    assert_eq!(body["deleted"].as_i64(), Some(2), "应删除 2 道题");

    let (_, list) = app.req(Method::GET, "/api/questions", None).await;
    let ids: Vec<i64> = list
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["id"].as_i64()).collect())
        .unwrap_or_default();
    assert!(!ids.contains(&q1) && !ids.contains(&q2), "被删题目不应再出现");
    assert!(ids.contains(&q3), "未选中的题应保留");
}
