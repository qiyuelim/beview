//! v4 评审整改 SEC1：登录接口限流（per-username 滑动窗口，60s 内 ≥5 次失败即 429，
//! 成功登录清零）。进程内存态，重启即清（与 SessionStore 同级，局域网自用足够）。
//!
//! 注意：限流计数器是进程级 static，跨测试二进制内共享——本文件用独立用户名避免污染其他用例。

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::TestApp;
use serde_json::json;
use tower::ServiceExt;

async fn try_login(app: &TestApp, user: &str, pw: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("{}/api/login", app.base_url()))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "username": user, "password": pw }).to_string()))
        .unwrap();
    app.app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn login_rate_limited_after_five_failures() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 建一个专用用户（独立用户名，避免污染共享限流表里其他用例的计数）
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "carol_throttle", "password": "carolpass123", "role": "user" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);

    // 前 5 次错误密码 -> 401
    for i in 0..5 {
        let st = try_login(&app, "carol_throttle", "wrong-password").await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "第 {} 次失败应 401", i + 1);
    }

    // 第 6 次：即使密码正确也 429（限流生效）
    let st = try_login(&app, "carol_throttle", "carolpass123").await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "触发限流应 429");

    // 其他用户不受影响
    let st = try_login(&app, "admin", "admin123").await;
    assert_eq!(st, StatusCode::OK);
}
