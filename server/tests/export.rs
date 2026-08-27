//! M3 全量数据导出 —— TDD 红测试。
//! 场景：用户能把全部数据（公司/岗位/投递/轮次/题目/分析/评论/复习/训练/简历等）导出为一份 JSON。

mod common;

use axum::http::Method;
use serde_json::json;
use common::TestApp;

/// 完整性守卫（评审 P1）：迁移里的业务表必须全部出现在导出清单，
/// 防止新增表后静默漏导（此前 positions/sessions 等就漏了）。
#[test]
fn export_table_list_covers_all_migrations() {
    let migrations_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/migrations");
    let mut business_tables: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(migrations_dir).expect("应能读取迁移目录");
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        let sql = std::fs::read_to_string(&path).unwrap();
        for cap in sql.split("CREATE TABLE ").skip(1) {
            let name: String = cap
                .trim_start_matches("IF NOT EXISTS ")
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                business_tables.push(name);
            }
        }
    }
    let exported: Vec<&str> = server::routes::export::EXPORT_TABLES.to_vec();
    for t in business_tables {
        if ["users", "settings", "_sqlx_migrations"].contains(&t.as_str()) {
            continue; // 有意不导出（凭据/密钥材料）
        }
        assert!(exported.contains(&t.as_str()), "业务表 {t} 未纳入导出清单 EXPORT_TABLES");
    }
}

/// 用户能导出包含所有主要数据的 JSON 备份
#[tokio::test]
async fn user_can_export_all_data() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 造一点数据：投递(公司+岗位)->轮次->题目(带我的回答)
    let (_, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "导出公司", "position": "后端", "jd_text": "JD" })),
        )
        .await;
    let aid = a["id"].as_i64().unwrap();
    let (_, r) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" }))).await;
    let rid = r["id"].as_i64().unwrap();
    app.req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "导出题", "my_answer": "答" }))).await;

    // 全量导出
    let (sc, body) = app.req_raw(Method::GET, "/api/export", None).await;
    assert!(sc.is_success(), "导出应成功, got {sc}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("导出应为合法 JSON");

    assert_eq!(v["applications"].as_array().map(|x| x.len()), Some(1), "应导出 1 份投递");
    assert_eq!(v["rounds"].as_array().map(|x| x.len()), Some(1), "应导出 1 个轮次");
    assert_eq!(v["questions"].as_array().map(|a| a.len()), Some(1), "应导出 1 道题");
    assert!(v["questions"][0]["content"].as_str().unwrap_or("").contains("导出题"), "题目内容应完整");
    assert_eq!(v["review_records"].as_array().map(|a| a.len()), Some(1), "应导出复习记录");
    assert!(v["exported_at"].as_str().is_some(), "应含导出时间");
    // 关键集合键都应存在（即使为空数组）——含 v4/v5 新增表（评审整改前静默漏导的那批）
    for key in [
        "analyses", "comments", "tags", "drills", "drill_messages", "resumes",
        "positions", "sessions", "application_events", "question_answers",
        "question_rounds", "question_skills", "skills", "points_ledger", "mall_items",
    ] {
        assert!(v[key].is_array(), "导出应含数组键 {key}");
    }
}
