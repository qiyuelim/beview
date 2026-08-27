//! 可观测性集成测试 (ADR-0003 + v5.4-M2 Ticket 04)

use axum::http::{Method, StatusCode};
use serde_json::json;

mod common;
use common::*;

#[tokio::test]
async fn test_x_trace_id_generated_and_returned_in_response_header() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 发起常规请求，不带 trace 请求头
    let builder = axum::http::Request::builder()
        .method(Method::GET)
        .uri("http://test/api/me")
        .header("cookie", app.cookie.as_ref().unwrap());
    let req = builder.body(axum::body::Body::empty()).unwrap();
    let resp = tower::ServiceExt::oneshot(&mut app.app.clone(), req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let trace_id_header = resp.headers().get("x-trace-id");
    assert!(trace_id_header.is_some(), "响应头中必须包含 x-trace-id");
    let trace_id = trace_id_header.unwrap().to_str().unwrap();
    assert!(!trace_id.is_empty(), "生成的 trace_id 不能为空");
    assert_eq!(trace_id.len(), 32, "生成的 trace_id 应为 32 位 hex");
}

#[tokio::test]
async fn test_x_trace_id_preserves_upstream_traceparent_and_custom_trace_id() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 上游传入 traceparent (W3C 规范)
    let upstream_trace = "4bf92f3577b34da6a3ce929d0e0e4736";
    let traceparent = format!("00-{upstream_trace}-00f067aa0ba902b7-01");
    let req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("http://test/api/me")
        .header("cookie", app.cookie.as_ref().unwrap())
        .header("traceparent", traceparent)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(&mut app.app.clone(), req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let resp_trace = resp.headers().get("x-trace-id").unwrap().to_str().unwrap();
    assert_eq!(resp_trace, upstream_trace, "应从 traceparent 中精准继承 32 位 trace_id");

    // 2. 上游传入自定义 x-trace-id
    let custom_trace = "custom-trace-id-123456";
    let req2 = axum::http::Request::builder()
        .method(Method::GET)
        .uri("http://test/api/me")
        .header("cookie", app.cookie.as_ref().unwrap())
        .header("x-trace-id", custom_trace)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp2 = tower::ServiceExt::oneshot(&mut app.app.clone(), req2).await.unwrap();

    assert_eq!(resp2.status(), StatusCode::OK);
    let resp_trace2 = resp2.headers().get("x-trace-id").unwrap().to_str().unwrap();
    assert_eq!(resp_trace2, custom_trace, "应直接继承传入的 x-trace-id");
}

#[test]
fn test_sensitive_data_masking_and_length_truncation() {
    use server::observe::format_and_mask_body;

    // 1. 敏感字段脱敏测试
    let payload = json!({
        "username": "alice",
        "password": "super_secret_password_123",
        "api_key": "sk-1234567890abcdef",
        "nested": {
            "token": "bearer_secret_token",
            "normal_field": "visible_value"
        }
    });
    let raw_bytes = serde_json::to_vec(&payload).unwrap();
    let masked = format_and_mask_body(&raw_bytes);

    assert!(!masked.contains("super_secret_password_123"), "密码必须被脱敏");
    assert!(!masked.contains("sk-1234567890abcdef"), "API Key 必须被脱敏");
    assert!(!masked.contains("bearer_secret_token"), "Token 必须被脱敏");
    assert!(masked.contains("*** [REDACTED] ***"), "敏感字段值应替换为 REDACTED 标记");
    assert!(masked.contains("visible_value"), "非敏感字段应正常保留");

    // 2. 超长数据截断测试 (> 2048 字节)
    let long_str = "a".repeat(3000);
    let long_payload = json!({ "content": long_str });
    let long_bytes = serde_json::to_vec(&long_payload).unwrap();
    let masked_long = format_and_mask_body(&long_bytes);

    assert!(masked_long.contains("... [truncated]"), "超长报文必须自动截断并附带 truncated 标记");
    assert!(masked_long.len() <= 2100, "截断后长度应在合理限制范围内");

    // 3. UTF-8 多字节（中文）边界截断安全测试（防止 panic）
    // " Rust所有权与生命周期 " 每个汉字 3 字节，拼接使第 2048 字节恰好落在一个汉字中间
    let chinese_unit = "Rust系统架构高并发性能调优与分布式实战演练"; // 64 bytes
    let chinese_long = chinese_unit.repeat(100); // 6400 bytes
    let chinese_bytes = chinese_long.as_bytes();
    let truncated_chinese = format_and_mask_body(chinese_bytes);
    assert!(truncated_chinese.contains("... [truncated]"), "长中文报文应安全截断且不 panic");

    // 4. 遥测 tokens_used 字段不误伤测试 (S4)
    let telemetry_payload = json!({
        "tokens_used": 1500,
        "total_tokens": 3200,
        "auth_token": "secret_session_token_xyz"
    });
    let tele_masked = format_and_mask_body(&serde_json::to_vec(&telemetry_payload).unwrap());
    assert!(tele_masked.contains("1500"), "tokens_used 应正常保留");
    assert!(tele_masked.contains("3200"), "total_tokens 应正常保留");
    assert!(!tele_masked.contains("secret_session_token_xyz"), "auth_token 必须被脱敏");
}

