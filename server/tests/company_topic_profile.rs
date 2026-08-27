//! v5.5-M1（票04）：公司高频考点画像（服务端聚合）。
//! 覆盖：三条归属链聚合、排序稳定性、跨用户隔离、空画像结构化返回。

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

mod common;

use common::TestApp;

async fn mk_question_with_tags(app: &TestApp, rid: i64, content: &str, tags: &[&str]) -> i64 {
    let (s, q) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({
                "round_id": rid,
                "content": content,
                "tags": tags,
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    q["id"].as_i64().unwrap()
}

/// 标准 公司→岗位→投递→轮次 链（复用 applications 的按名 upsert 建链语义）
async fn setup_company_chain(app: &TestApp, company: &str, position: &str) -> (i64, i64) {
    // (company_id, round_id)
    let (s, c) = app.req(Method::POST, "/api/companies", Some(json!({ "name": company }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let cid = c["id"].as_i64().unwrap();

    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": company, "position": position })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "建投递失败: {a}");
    let aid = a["id"].as_i64().unwrap();
    let (s, r) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    assert!(s.is_success(), "建轮次失败: {r}");
    (cid, r["id"].as_i64().unwrap())
}

#[tokio::test]
async fn profile_aggregates_main_chain_and_predicted_chain() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (cid, rid_b) = setup_company_chain(&app, "聚合公司 B", "平台开发").await;

    // 干扰项：另一家公司 C 的题目，不得混入 B 的画像
    let (_other_cid, rid_c) = setup_company_chain(&app, "无关公司 C", "测试岗").await;
    mk_question_with_tags(&app, rid_c, "C-公司专属题", &["C标签"]).await;

    // B 公司题目：tag 分布 存储×3、网络×1
    mk_question_with_tags(&app, rid_b, "B-题1 存储引擎", &["存储", "高频"]).await;
    mk_question_with_tags(&app, rid_b, "B-题2 缓存策略", &["存储", "中频"]).await;
    mk_question_with_tags(&app, rid_b, "B-题3 LSM 树", &["存储"]).await;
    mk_question_with_tags(&app, rid_b, "B-题4 TCP 重传", &["网络"]).await;

    // 押题链：给 B 公司岗位押题入库（挂 self-round，但 predicted_position_id 指向 B 岗位）
    // 找到 B 公司岗位 id：positions 列表接口
    let (s, pos_list) = app.req(Method::GET, &format!("/api/companies/{cid}/positions"), None).await;
    assert_eq!(s, StatusCode::OK);
    let pid = pos_list.as_array().unwrap()[0]["id"].as_i64().unwrap();

    let (s, ing) = app
        .req(
            Method::POST,
            &format!("/api/positions/{pid}/predict/ingest"),
            Some(json!({ "questions": [
                { "content": "押题-Kafka 顺序性", "category": "消息队列" }
            ] })),
        )
        .await;
    assert!(s.is_success(), "押题入题库失败: {ing}");

    let (s, profile) = app.req(Method::GET, &format!("/api/companies/{cid}/topic-profile"), None).await;
    assert_eq!(s, StatusCode::OK);

    // 总数：主链 4 题 + 押题 1 题 = 5；C 公司题目不得混入
    assert_eq!(profile["total_questions"], 5, "主归属链与押题链都应计入: {profile}");

    // tag 排序：存储(3) > 其余同计数按名称升序稳定输出
    let tags: Vec<(String, i64)> = profile["top_tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(tags[0], ("存储".into(), 3), "计数最高应居首: {tags:?}");
    let rest_names: Vec<&str> = tags[1..].iter().map(|(n, _)| n.as_str()).collect();
    let mut sorted = rest_names.clone();
    sorted.sort();
    assert_eq!(rest_names, sorted, "同计数按名称升序稳定输出");

    // 类型分布计数和 = 总题数
    let type_sum: i64 = profile["type_distribution"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["count"].as_i64().unwrap())
        .sum();
    assert_eq!(type_sum, 5);
}

#[tokio::test]
async fn profile_empty_returns_structured_zero() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let cid = create_company_direct(&app, "空画像公司").await;
    let (s, prof) = app.req(Method::GET, &format!("/api/companies/{cid}/topic-profile"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(prof["total_questions"], 0);
    assert_eq!(prof["top_tags"].as_array().unwrap().len(), 0);
    assert_eq!(prof["top_skills"].as_array().unwrap().len(), 0);
    assert_eq!(prof["type_distribution"].as_array().unwrap().len(), 0);
}

async fn create_company_direct(app: &TestApp, name: &str) -> i64 {
    let (s, c) = app.req(Method::POST, "/api/companies", Some(json!({ "name": name }))).await;
    assert_eq!(s, StatusCode::CREATED);
    c["id"].as_i64().unwrap()
}

#[tokio::test]
async fn profile_is_row_level_isolated() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let cid = create_company_direct(&app, "隔离画像公司").await;

    // 第二用户访问 A 的公司画像 → 404（不泄露存在性）
    let (s, _) = app
        .req(Method::POST, "/api/admin/users", Some(json!({ "username": "profob", "password": "bobpass123" })))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, cookie) = app.login_as("profob", "bobpass123").await;
    let cookie = cookie.unwrap();
    let (s, _) = app
        .req_as(&cookie, Method::GET, &format!("/api/companies/{cid}/topic-profile"), None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "跨用户访问他人公司画像应 404");
}
