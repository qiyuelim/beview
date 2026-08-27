//! v5.5-M1（票05，ADR-0021）：简历 AI 优化变更集。
//! propose = 同步契约执行（mock LLM）；apply = 服务端逐条校验 + 自动快照兜底 + 落库。

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

mod common;

use common::{llm_mock::LlmMock, TestApp};

/// 建工作副本并写入 parsed 结构化数据
async fn seed_working_copy(app: &TestApp, parsed: Value) -> Value {
    let (s, body) = app
        .req(
            Method::PUT,
            "/api/resume",
            Some(json!({
                "raw_text": "张三 后端工程师 三年经验…",
                "parsed": parsed,
            })),
        )
        .await;
    assert!(s.is_success(), "保存工作副本失败: {body}");
    body
}

fn sample_parsed() -> Value {
    json!({
        "name": "张三",
        "summary": "三年后端开发",
        "skills": ["Rust", "SQL"],
        "experience": [
            {"company": "甲公司", "title": "后端", "period": "2021-2024"}
        ]
    })
}

#[tokio::test]
async fn propose_returns_changeset_via_mock() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    seed_working_copy(&app, sample_parsed()).await;

    // mock 返回合法变更集 JSON（strict schema 解析路径）
    mock.queue_nonstream(
        json!({
            "summary": "强化成果导向表达",
            "changes": [
                {"action": "update", "module": "summary",
                 "old_value": "三年后端开发", "new_value": "三年后端开发，主导过日均千万级请求服务",
                 "reason": "突出规模与成果"},
                {"action": "add", "module": "skills", "old_value": null, "new_value": "Kubernetes",
                 "reason": "补齐云原生技能"}
            ]
        })
        .to_string()
        .as_str(),
    );

    let (s, resp) = app
        .req(
            Method::POST,
            "/api/resume/optimize/propose",
            Some(json!({ "intent": "突出项目成果" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "propose 失败: {resp}");
    assert_eq!(resp["summary"], "强化成果导向表达");
    assert_eq!(resp["changes"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn propose_schema_broken_output_triggers_retry() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    seed_working_copy(&app, sample_parsed()).await;

    // 第一次输出非法 JSON（触发就地纠偏重试），第二次合法
    mock.queue_nonstream("这不是 JSON {{{");
    mock.queue_nonstream(
        json!({"summary": "s", "changes": []}).to_string().as_str(),
    );

    let (s, resp) = app
        .req(Method::POST, "/api/resume/optimize/propose", Some(json!({})))
        .await;
    assert_eq!(s, StatusCode::OK, "纠偏重试后应成功: {resp}");
    assert_eq!(resp["changes"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn propose_requires_parsed_working_copy() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    // 无任何简历行
    let (s, _) = app
        .req(Method::POST, "/api/resume/optimize/propose", Some(json!({})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // 有原文但无 parsed → 同样拒绝
    let (s, _) = app
        .req(Method::PUT, "/api/resume", Some(json!({ "raw_text": "只有原文" })))
        .await;
    assert!(s.is_success());
    let (s, body) = app
        .req(Method::POST, "/api/resume/optimize/propose", Some(json!({})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn apply_validates_assertions_snapshots_and_persists() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    seed_working_copy(&app, sample_parsed()).await;

    // 混合批次：1 条有效 update + 1 条陈旧断言 + 1 条未知模块
    let changes = json!([
        {"action": "update", "module": "summary", "old_value": "三年后端开发",
         "new_value": "三年后端开发，专注高并发", "reason": "r1"},
        {"action": "update", "module": "name", "old_value": "错误断言", "new_value": "李四", "reason": "r2"},
        {"action": "update", "module": "hacker_field", "old_value": null, "new_value": "y", "reason": "r3"}
    ]);
    let (s, resp) = app
        .req(Method::POST, "/api/resume/optimize/apply", Some(json!({ "changes": changes })))
        .await;
    assert_eq!(s, StatusCode::OK, "{resp}");
    assert_eq!(resp["applied"], 1);
    let rejected = resp["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), 2);
    assert_eq!(rejected[0]["index"], 1);
    assert!(rejected[0]["reason"].as_str().unwrap_or("").contains("旧值断言"));
    assert_eq!(rejected[1]["index"], 2);

    // 工作副本已更新（只有成功的那条生效）
    let (_, resume) = app.req(Method::GET, "/api/resume", None).await;
    assert_eq!(resume["parsed"]["summary"], "三年后端开发，专注高并发");
    assert_eq!(resume["parsed"]["name"], "张三", "被拒条目不得落库");

    // 自动快照兜底：留档列表出现「变更前快照」（列表不含 parsed，经详情端点断言内容）
    let (_, list) = app.req(Method::GET, "/api/resumes", None).await;
    let archives = list.as_array().unwrap();
    let snap = archives
        .iter()
        .find(|r| r["version_name"].as_str().unwrap_or("").contains("变更前快照"))
        .expect("应有自动快照");
    assert_eq!(snap["is_archived"], true);
    assert_eq!(snap["has_parsed"], true);
    let snap_id = snap["id"].as_i64().unwrap();
    let (s, snap_detail) = app.req(Method::GET, &format!("/api/resumes/{snap_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(snap_detail["parsed"]["summary"], "三年后端开发", "快照应是应用前状态");
}

#[tokio::test]
async fn apply_without_changes_returns_zero_applied() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    seed_working_copy(&app, sample_parsed()).await;

    let (s, resp) = app
        .req(Method::POST, "/api/resume/optimize/apply", Some(json!({ "changes": [] })))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(resp["applied"], 0);
}
