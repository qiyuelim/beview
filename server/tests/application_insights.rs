//! v5.5-M1（票07）：投递全局智能洞察。
//! 覆盖：空数据引导（POST 400 + GET null）、四段结构往返落库、per-user 隔离。

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

mod common;

use common::{llm_mock::LlmMock, TestApp};

fn valid_insight_json() -> String {
    json!({
        "summary": "当前求职节奏稳健，面试转化为主要瓶颈。",
        "observations": ["投递集中在内推渠道", "面试→offer 转化为零"],
        "recommendations": ["补充中小厂投递面", "针对系统设计专项补强"],
        "priority": [
            {"action": "本周内完成 2 场系统设计专项模考", "reason": "两次面试均在该环节失利"}
        ]
    })
    .to_string()
}

#[tokio::test]
async fn empty_state_gives_guidance_not_empty_report() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // GET：无洞察 → insight=null（前端据此渲染引导态）
    let (s, body) = app.req(Method::GET, "/api/applications/insights", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["insight"].is_null());
    assert_eq!(body["ai_jobs"].as_array().unwrap().len(), 0);

    // POST：无任何投递 → 400 引导文案，绝不浪费一次 LLM 调用
    let (s, body) = app
        .req(Method::POST, "/api/applications/insights", Some(json!({})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("暂无投递数据"), "{body}");
    // mock 未被消费
    assert!(mock.request_bodies().is_empty(), "空数据不应发起 LLM 调用");
}

#[tokio::test]
async fn four_section_report_roundtrip_and_persist() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let aid = common::create_application(&app, "洞察公司", "后端工程师").await;
    common::create_round(&app, aid, "一面").await;
    // 状态流水：直接落一条事件（模拟状态推进历史）
    sqlx::query(
        "INSERT INTO application_events(user_id, application_id, from_status, to_status) \
         VALUES ((SELECT user_id FROM applications WHERE id=$1), $1, 'applied', 'interviewing')",
    )
    .bind(aid)
    .execute(&app.pool)
    .await
    .expect("插入状态流水失败");

    mock.queue_nonstream(&valid_insight_json());

    // 受理 → job_id
    let (s, resp) = app
        .req(Method::POST, "/api/applications/insights", Some(json!({})))
        .await;
    assert!(s.is_success() || s == StatusCode::ACCEPTED, "{resp}");
    let job_id = resp["job_id"].as_u64().expect("应返回 job_id");

    // 等终态并校验结果落库、GET 可回看
    let done = common::wait_ai_job(&app, job_id, 15_000).await;
    assert_eq!(done["status"], "done", "{done}");

    let (s, body) = app.req(Method::GET, "/api/applications/insights", None).await;
    assert_eq!(s, StatusCode::OK);
    let insight = &body["insight"];
    assert!(!insight.is_null(), "应有最新洞察");
    assert_eq!(insight["summary"], "当前求职节奏稳健，面试转化为主要瓶颈。");
    assert_eq!(insight["observations"].as_array().unwrap().len(), 2);
    assert_eq!(insight["recommendations"].as_array().unwrap().len(), 2);
    let priority = insight["priority"].as_array().unwrap();
    assert_eq!(priority.len(), 1);
    assert_eq!(priority[0]["action"], "本周内完成 2 场系统设计专项模考");
    assert!(insight["created_at"].as_str().is_some());

    // 幂等受理：再次 POST 不冲突（新任务可发起，旧任务已结束）
    mock.queue_nonstream(&valid_insight_json());
    let (s, resp2) = app
        .req(Method::POST, "/api/applications/insights", Some(json!({})))
        .await;
    assert!(s.is_success());
    let _ = common::wait_ai_job(&app, resp2["job_id"].as_u64().unwrap(), 15_000).await;
    let (_, again) = app.req(Method::GET, "/api/applications/insights", None).await;
    assert_eq!(again["insight"]["summary"], "当前求职节奏稳健，面试转化为主要瓶颈。");
}

#[tokio::test]
async fn insights_are_row_level_isolated() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = common::create_application(&app, "隔离洞察公司", "测试").await;
    let _ = aid;

    let (s, _) = app
        .req(Method::POST, "/api/admin/users", Some(json!({ "username": "insbob", "password": "bobpass123" })))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, cookie) = app.login_as("insbob", "bobpass123").await;
    let cookie = cookie.unwrap();

    // B 无投递 → GET 为空；POST 被引导拒绝（看不到 A 的任何数据）
    let (s, body) = app
        .req_as(&cookie, Method::GET, "/api/applications/insights", None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["insight"].is_null());
    let (s, body) = app
        .req_as(&cookie, Method::POST, "/api/applications/insights", Some(json!({})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("暂无投递数据"));
}
