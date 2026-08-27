//! v4 M1 多用户底座（ADR-0011 R5）：
//! 行级隔离（用户间数据不可见）、停用拒登录、管理员用户管理、LLM 配置 per-user、api_key 加密落库。

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{json, Value};

/// 建第二个用户（走管理员 API）并返回其会话 cookie
async fn create_and_login_second_user(app: &TestApp) -> (StatusCode, Option<String>) {
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "bob", "password": "bobpass123" })),
        )
        .await;
    assert_eq!(s, 201, "管理员建号应成功");
    app.login_as("bob", "bobpass123").await
}

// ---------- 行级隔离 ----------

#[tokio::test]
async fn users_cannot_see_each_others_data() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 管理员（user A）建投递 + 轮次
    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "A 的公司", "position": "后端" })),
        )
        .await;
    assert_eq!(s, 201);
    let aid = a["id"].as_i64().unwrap();
    let (s, r) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({})))
        .await;
    assert_eq!(s, 201);
    let rid = r["id"].as_i64().unwrap();

    // 用户 B 登录
    let (s, bob_cookie) = create_and_login_second_user(&app).await;
    assert_eq!(s, 200, "新用户应能登录");
    let bob_cookie = bob_cookie.unwrap();

    // B 的投递列表为空（看不到 A 的）
    let (s, list) = app.req_as(&bob_cookie, Method::GET, "/api/applications", None).await;
    assert_eq!(s, 200);
    assert_eq!(list.as_array().unwrap().len(), 0, "B 不应看到 A 的投递");

    // B 直接访问 A 的轮次 -> 404
    let (s, _) = app
        .req_as(&bob_cookie, Method::GET, &format!("/api/rounds/{rid}"), None)
        .await;
    assert_eq!(s, 404, "跨用户访问应 404");

    // B 可建同名公司的投递（唯一约束按用户隔离）
    let (s2, _) = app
        .req_as(
            &bob_cookie,
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "A 的公司", "position": "前端" })),
        )
        .await;
    assert_eq!(s2, 201, "不同用户可对同一公司各建投递");

    // B 的题目列表为空
    let (s, qs) = app.req_as(&bob_cookie, Method::GET, "/api/questions", None).await;
    assert_eq!(s, 200);
    assert_eq!(qs.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn cross_user_writes_are_rejected() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // A 建投递 + 轮次
    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "甲公司", "position": "后端" })),
        )
        .await;
    assert_eq!(s, 201);
    let aid = a["id"].as_i64().unwrap();
    let (_, r) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    let rid = r["id"].as_i64().unwrap();

    // B 登录后试图往 A 的轮次里录题 -> 轮次不存在（404/400）
    let (s, bob) = create_and_login_second_user(&app).await;
    let bob = bob.unwrap();
    let (s, _) = app
        .req_as(
            &bob,
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "B 的题" })),
        )
        .await;
    assert!(
        s == StatusCode::NOT_FOUND || s == StatusCode::BAD_REQUEST,
        "跨用户录题应被拒，实际 {s}"
    );
}

// ---------- 停用与登录 ----------

#[tokio::test]
async fn disabled_user_cannot_login_and_is_kicked() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (s, bob) = create_and_login_second_user(&app).await;
    assert_eq!(s, 200);
    let bob = bob.unwrap();

    // B 正常可用
    let (s, _) = app.req_as(&bob, Method::GET, "/api/me", None).await;
    assert_eq!(s, 200);

    // 管理员停用 bob（id=2，RESTART IDENTITY 后顺序确定；稳妥起见从列表找）
    let (_, users) = app.req(Method::GET, "/api/admin/users", None).await;
    let bob_id = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "bob")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/admin/users/{bob_id}"),
            Some(json!({ "row_status": "disabled" })),
        )
        .await;
    assert_eq!(s, 200);

    // 已登录的会话被踢
    let (s, _) = app.req_as(&bob, Method::GET, "/api/me", None).await;
    assert_eq!(s, 401, "停用后已有会话应失效");

    // 再登录被拒（403 Forbidden 语义：账号存在但停用）
    let (s, _) = app.login_as("bob", "bobpass123").await;
    assert_eq!(s, 403, "停用账号登录应 403");

    // 恢复后可再登录
    app.login("admin", "admin123").await;
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/admin/users/{bob_id}"),
            Some(json!({ "row_status": "active" })),
        )
        .await;
    assert_eq!(s, 200);
    let (s, _) = app.login_as("bob", "bobpass123").await;
    assert_eq!(s, 200, "恢复后应可登录");
}

// ---------- 管理员权限 ----------

#[tokio::test]
async fn non_admin_cannot_manage_users() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (s, bob) = create_and_login_second_user(&app).await;
    assert_eq!(s, 200);
    let bob = bob.unwrap();

    // B 访问用户管理 -> 403
    let (s, _) = app.req_as(&bob, Method::GET, "/api/admin/users", None).await;
    assert_eq!(s, 403);

    // B 试图把自己升 admin -> 403
    let (_, users) = app.req(Method::GET, "/api/admin/users", None).await;
    let bob_id = users.as_array().unwrap().iter().find(|u| u["username"] == "bob").unwrap()["id"]
        .as_i64()
        .unwrap();
    let (s, _) = app
        .req_as(
            &bob,
            Method::PATCH,
            &format!("/api/admin/users/{bob_id}"),
            Some(json!({ "role": "admin" })),
        )
        .await;
    assert_eq!(s, 403);
}

// ---------- LLM 配置 per-user + api_key 加密落库（ADR-0016：llm_config 文档） ----------

#[tokio::test]
async fn llm_settings_are_per_user_and_key_encrypted_at_rest() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // A 配置 LLM（provider 带明文 key）
    let (s, v) = app
        .req(
            Method::PUT,
            "/api/settings/llm-config",
            Some(json!({
                "providers": [{ "id": "p1", "name": "Example", "base_url": "https://api.example.com/v1", "api_key": "sk-secret-abc123" }],
                "models": [{ "id": "m1", "provider_id": "p1", "name": "test-model",
                             "context_length": 128000,
                             "caps": { "structured_output": true, "web_search": false },
                             "advanced": { "reasoning_effort": "xhigh", "store": false } }],
                "active_model_id": "m1"
            })),
        )
        .await;
    assert_eq!(s, 200, "put 失败: {v}");


    // 库中必须是密文：enc:v1: 前缀且不含明文片段
    let stored: String = sqlx::query_scalar(
        "SELECT value #>> '{providers,0,api_key}' FROM settings WHERE key='llm_config' LIMIT 1",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(stored.starts_with("enc:v1:"), "api_key 应加密存储，实际：{stored}");
    assert!(!stored.contains("sk-secret-abc123"), "库中不得出现明文 key");

    // A 读回：掩码显示 + has_key=true；resolved 摘要正确
    let (s, v) = app.req(Method::GET, "/api/settings/llm-config", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["config"]["providers"][0]["has_key"], true);
    let masked = v["config"]["providers"][0]["api_key"].as_str().unwrap();
    assert!(masked.starts_with('*'), "key 应掩码显示");
    assert!(!masked.contains("sk-secret-abc123"));
    assert_eq!(v["resolved"]["model"], "test-model");
    assert_eq!(v["resolved"]["reasoning_effort"], "xhigh");

    // B 的 LLM 配置为空（per-user 隔离）
    let (s, bob) = create_and_login_second_user(&app).await;
    assert_eq!(s, 200);
    let bob = bob.unwrap();
    let (s, v) = app.req_as(&bob, Method::GET, "/api/settings/llm-config", None).await;
    assert_eq!(s, 200);
    assert!(v["config"]["providers"].as_array().unwrap().is_empty(), "B 不应有 A 的 provider");
    assert!(v["resolved"].is_null(), "B 不应有生效模型");

    // 提示词也是 per-user：B 未自定义
    let (s, v) = app.req_as(&bob, Method::GET, "/api/settings/prompts", None).await;
    assert_eq!(s, 200);
    assert!(v["prompts"].as_array().unwrap().iter().all(|p| p["is_custom"] == false));
}
