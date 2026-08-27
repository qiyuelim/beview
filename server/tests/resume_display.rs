//! v4.1 反馈 #6：简历显示偏好（主题/密度/模块顺序与显隐）——per-user settings 持久化。

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::json;

#[tokio::test]
async fn resume_display_roundtrip_and_validation() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 默认值
    let (s, v) = app.req(Method::GET, "/api/settings/resume-display", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["theme"], "classic");
    assert_eq!(v["density"], "normal");
    assert_eq!(v["hidden"].as_array().unwrap().len(), 0);
    // order 是 8 个模块的全排列
    assert_eq!(v["order"].as_array().unwrap().len(), 8);

    // 合法更新 -> 回读（隐藏 links + 紧凑主题）
    let order = ["basic", "skills", "experience", "projects", "education", "certificates", "self_evaluation", "links"];
    let (s, _) = app
        .req(
            Method::PUT,
            "/api/settings/resume-display",
            Some(json!({ "theme": "compact", "density": "tight", "hidden": ["links"], "order": order })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = app.req(Method::GET, "/api/settings/resume-display", None).await;
    assert_eq!(v["theme"], "compact");
    assert_eq!(v["density"], "tight");
    assert_eq!(v["hidden"][0], "links");
    assert_eq!(v["order"][0], "basic");
    assert_eq!(v["order"][1], "skills");

    // 非法主题拒绝
    let (s, e) = app
        .req(
            Method::PUT,
            "/api/settings/resume-display",
            Some(json!({ "theme": "neon", "density": "normal", "hidden": [], "order": order })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("theme"));

    // order 缺模块拒绝（必须全排列，渲染顺序确定性）
    let bad: Vec<&str> = order[..7].to_vec();
    let (s, _) = app
        .req(
            Method::PUT,
            "/api/settings/resume-display",
            Some(json!({ "theme": "classic", "density": "normal", "hidden": [], "order": bad })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // per-user：B 的偏好独立且为默认
    let (s, bob) = {
        let (s, _) = app
            .req(Method::POST, "/api/admin/users", Some(json!({ "username": "rebe", "password": "rebepass1" })))
            .await;
        assert_eq!(s, 201);
        app.login_as("rebe", "rebepass1").await
    };
    assert_eq!(s, StatusCode::OK);
    let (_, v) = app.req_as(&bob.unwrap(), Method::GET, "/api/settings/resume-display", None).await;
    assert_eq!(v["theme"], "classic", "B 应是默认偏好");
}
