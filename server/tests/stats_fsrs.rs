//! v5.5-M1（ADR-0022 D1）：FSRS 记忆大盘集成测试。
//! 覆盖：空数据回退（fitted=false）、种子历史拟合（fitted=true）、
//! 同输入幂等（两次响应完全一致）、行级隔离。

use axum::http::Method;
use serde_json::{json, Value};

mod common;

use common::TestApp;

async fn mk_question(app: &TestApp, rid: i64, content: &str) -> i64 {
    let (s, q) = app
        .req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": content })))
        .await;
    assert!(s.is_success(), "创建题目失败: {s}");
    q["id"].as_i64().unwrap()
}

#[tokio::test]
async fn fsrs_memory_empty_returns_structured_defaults_unfitted() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (s, body) = app.req(Method::GET, "/api/stats/fsrs-memory", None).await;
    assert_eq!(s, axum::http::StatusCode::OK);
    assert_eq!(body["total_cards"], 0);
    assert_eq!(body["avg_retention"].as_f64(), Some(100.0));
    assert_eq!(body["fitted"], false);
    for key in ["solid", "good", "fading", "risk"] {
        assert_eq!(body["distribution"][key], 0);
    }
    assert_eq!(body["due_next_7_days"].as_array().unwrap().len(), 7);
}

#[tokio::test]
async fn fsrs_memory_fits_from_seeded_history_and_is_deterministic() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_aid, rid) = common::setup_application_round(&app).await;
    let mut qids = Vec::new();
    for i in 0..24 {
        qids.push(mk_question(&app, rid, &format!("FSRS 拟合测试题 {i}：请解释该概念")).await);
    }

    // 第一轮自评：混合评分，经 API 写入 review_logs（评分映射 forgot→1/fuzzy→2/remembered→3）
    let results = ["remembered", "fuzzy", "forgot"];
    for (i, qid) in qids.iter().enumerate() {
        let (s, _) = app
            .req(
                Method::POST,
                &format!("/api/review/{qid}/grade"),
                Some(json!({ "result": results[i % 3] })),
            )
            .await;
        assert!(s.is_success(), "评分失败 qid={qid}");
    }

    // 第一轮日志回溯到 25~44 天前，制造真实的间隔分布
    sqlx::query(
        "UPDATE review_logs SET reviewed_at = now() - (interval '1 day' * ((id % 20) + 25))",
    )
    .execute(&app.pool)
    .await
    .expect("回填日志时间失败");

    // 第二轮自评：14 张卡今日再评一次 → 这些卡成为多复习卡（训练集）
    for qid in qids.iter().take(14) {
        let (s, _) = app
            .req(
                Method::POST,
                &format!("/api/review/{qid}/grade"),
                Some(json!({ "result": results[*qid as usize % 3] })),
            )
            .await;
        assert!(s.is_success());
    }

    let (s1, body1) = app.req(Method::GET, "/api/stats/fsrs-memory", None).await;
    assert_eq!(s1, axum::http::StatusCode::OK);
    assert_eq!(
        body1["fitted"],
        true,
        "38 条多日距日志应触发个性化拟合: {body1}"
    );
    assert_eq!(body1["total_cards"], 24);

    // 分桶守恒：四桶之和 == 卡片总数
    let dist = &body1["distribution"];
    let sum: i64 = ["solid", "good", "fading", "risk"]
        .iter()
        .map(|k| dist[k].as_i64().unwrap())
        .sum();
    assert_eq!(sum, 24);

    // 幂等性：同数据两次请求产出完全一致的预测
    let (_, body2) = app.req(Method::GET, "/api/stats/fsrs-memory", None).await;
    assert_eq!(value_canon(&body1), value_canon(&body2), "两次计算必须一致");
}

#[tokio::test]
async fn fsrs_memory_is_row_level_isolated() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_aid, rid) = common::setup_application_round(&app).await;
    let qid = mk_question(&app, rid, "隔离检查题").await;
    let (s, _) = app
        .req(Method::POST, &format!("/api/review/{qid}/grade"), Some(json!({ "result": "remembered" })))
        .await;
    assert!(s.is_success());

    // 第二个用户看不到别人的复习数据（管理员建号，无自助注册——基准6）
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "fsrsbob", "password": "bobpass123" })),
        )
        .await;
    assert_eq!(s, 201, "管理员建号应成功");
    let (status, cookie) = app.login_as("fsrsbob", "bobpass123").await;
    assert!(status.is_success(), "新用户应能登录");
    let cookie = cookie.expect("应有会话 Cookie");
    let (s, body) = app
        .req_as(&cookie, Method::GET, "/api/stats/fsrs-memory", None)
        .await;
    assert_eq!(s, axum::http::StatusCode::OK);
    assert_eq!(body["total_cards"], 0);
    assert_eq!(body["fitted"], false);
}

#[tokio::test]
async fn test_fsrs_review_scheduling_with_fitted_and_unfitted_fallback() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_aid, rid) = common::setup_application_round(&app).await;
    let qid = mk_question(&app, rid, "排程化单题测试").await;

    // 1. 无拟合/首评（fitted=false 回退）：remembered 初次排程
    let (s, res1) = app
        .req(Method::POST, &format!("/api/review/{qid}/grade"), Some(json!({ "result": "remembered" })))
        .await;
    assert!(s.is_success());
    assert!(res1["interval_days"].as_i64().unwrap() >= 1);

    // 2. forgot 必定重置为 1 天
    let (s_forgot, res_forgot) = app
        .req(Method::POST, &format!("/api/review/{qid}/grade"), Some(json!({ "result": "forgot" })))
        .await;
    assert!(s_forgot.is_success());
    assert_eq!(res_forgot["interval_days"], 1, "forgot 必须排程为 1 天");

    // 3. 构建充分拟合的数据集（24 卡 38+ 日志）
    let mut qids = Vec::new();
    for i in 0..24 {
        qids.push(mk_question(&app, rid, &format!("排程大批量题 {i}")).await);
    }
    let results = ["remembered", "fuzzy", "forgot"];
    for (i, q) in qids.iter().enumerate() {
        let (s, _) = app
            .req(Method::POST, &format!("/api/review/{q}/grade"), Some(json!({ "result": results[i % 3] })))
            .await;
        assert!(s.is_success());
    }
    sqlx::query("UPDATE review_logs SET reviewed_at = now() - (interval '1 day' * ((id % 20) + 25))")
        .execute(&app.pool)
        .await
        .unwrap();
    for q in qids.iter().take(14) {
        let (s, _) = app
            .req(Method::POST, &format!("/api/review/{q}/grade"), Some(json!({ "result": "remembered" })))
            .await;
        assert!(s.is_success());
    }

    // 确认此时处于 fitted=true
    let (_, stats) = app.req(Method::GET, "/api/stats/fsrs-memory", None).await;
    assert_eq!(stats["fitted"], true);

    // 再次对稳定记忆卡自评 remembered：使用拟合权重计算稳定性排程
    let (s_fitted, res_fitted) = app
        .req(Method::POST, &format!("/api/review/{}/grade", qids[0]), Some(json!({ "result": "remembered" })))
        .await;
    assert!(s_fitted.is_success());
    assert!(res_fitted["interval_days"].as_i64().unwrap() >= 1);
}

/// 稳定序列化（BTreeMap 键序），用于幂等比较。
fn value_canon(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                out.push('{');
                for k in keys {
                    out.push_str(&format!("{k}:"));
                    walk(&m[k], out);
                    out.push(',');
                }
                out.push('}');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut s = String::new();
    walk(v, &mut s);
    s
}
