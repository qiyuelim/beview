//! 简历复数化 (Multiplexing) 与投递软引用 (Soft Reference) 集成测试 (ADR-0019 + v5.4-M2 Ticket 10)

mod common;

use axum::http::{Method, StatusCode};
use common::llm_mock::LlmMock;
use common::TestApp;
use serde_json::json;

#[tokio::test]
async fn test_resume_multiplexing_and_snapshot_lifecycle() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始保存工作副本
    let (s, _) = app
        .req(
            Method::PUT,
            "/api/resume",
            Some(json!({
                "raw_text": "张三，资深 Rust / Go 研发专家，具备 8 年高并发与分布式架构经验。",
                "name": "张三的主简历"
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // 2. 查询当前工作副本
    let (s, working) = app.req(Method::GET, "/api/resume", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(working["name"], "张三的主简历");
    assert_eq!(working["version_name"], "工作副本");
    assert_eq!(working["is_archived"], false);
    let working_id = working["id"].as_i64().unwrap();

    // 3. 列表查询（此时仅有 1 份工作副本）
    let (s, list) = app.req(Method::GET, "/api/resumes", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);

    // 4. 创建静态快照（留档归档）
    let (s, snapshot) = app
        .req(
            Method::POST,
            "/api/resumes/snapshot",
            Some(json!({ "version_name": "2026淘天架构投递专版" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let snapshot_id = snapshot["id"].as_i64().unwrap();
    assert_ne!(snapshot_id, working_id, "快照必须是独立的新行");
    assert_eq!(snapshot["version_name"], "2026淘天架构投递专版");

    // 5. 再次查询列表（应有 2 份：1 份工作副本 + 1 份留档快照）
    let (s, list2) = app.req(Method::GET, "/api/resumes", None).await;
    assert_eq!(s, StatusCode::OK);
    let arr = list2.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let snap_item = arr.iter().find(|i| i["id"] == snapshot_id).unwrap();
    assert_eq!(snap_item["is_archived"], true);
    assert_eq!(snap_item["version_name"], "2026淘天架构投递专版");

    // 6. 按 ID 详情查询快照
    let (s, snap_detail) = app.req(Method::GET, &format!("/api/resumes/{snapshot_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(snap_detail["id"], snapshot_id);
    assert_eq!(snap_detail["is_archived"], true);
    assert!(snap_detail["raw_text"].as_str().unwrap().contains("8 年高并发"));
}

#[tokio::test]
async fn test_archive_action_and_auto_clone_working_copy() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始化简历
    app.req(
        Method::PUT,
        "/api/resume",
        Some(json!({
            "raw_text": "李四，云原生平台研发工程师，熟悉 Kubernetes 与 Istio。",
            "name": "李四简历"
        })),
    ).await;

    let (_, working) = app.req(Method::GET, "/api/resume", None).await;
    let old_working_id = working["id"].as_i64().unwrap();

    // 2. 归档当前工作副本
    let (s, _) = app.req(Method::POST, &format!("/api/resumes/{old_working_id}/archive"), None).await;
    assert_eq!(s, StatusCode::OK);

    // 3. 验证旧行已被归档
    let (_, old_item) = app.req(Method::GET, &format!("/api/resumes/{old_working_id}"), None).await;
    assert_eq!(old_item["is_archived"], true);

    // 4. 验证系统自动克隆了新的未归档工作副本
    let (_, new_working) = app.req(Method::GET, "/api/resume", None).await;
    assert_ne!(new_working["id"].as_i64().unwrap(), old_working_id, "必须生成新的工作副本 ID");
    assert_eq!(new_working["is_archived"], false);
    assert_eq!(new_working["version_name"], "工作副本");
    assert!(new_working["raw_text"].as_str().unwrap().contains("Kubernetes"));
}

#[tokio::test]
async fn test_auto_snapshot_backup_before_ai_reparse() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 1. 保存简历并带有已解析内容
    let (s, _) = app
        .req(
            Method::PUT,
            "/api/resume",
            Some(json!({
                "raw_text": "王五，数据中台研发负责人，主导 ClickHouse 大数据 OLAP 引擎。",
                "name": "王五简历",
                "parsed": {
                    "name": "王五",
                    "skills": ["ClickHouse", "Flink"]
                }
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // 2. 触发 AI 重新解析
    mock.queue_nonstream(r#"{"name":"王五","summary":"数据中台负责人","skills":["ClickHouse","Flink","Spark"]}"#);
    let (s, job_resp) = app.req(Method::POST, "/api/resume/parse", Some(json!({}))).await;
    assert_eq!(s, StatusCode::OK);
    common::wait_ai_job(&app, job_resp["job_id"].as_u64().unwrap(), 5000).await;

    // 3. 验证当前工作副本更新为最新解析
    let (_, working) = app.req(Method::GET, "/api/resume", None).await;
    let skills = working["parsed"]["skills"].as_array().unwrap();
    assert!(skills.iter().any(|s| s == "Spark"), "最新工作副本应包含新解析技能 Spark");

    // 4. 验证系统在重新解析前自动留存了归档快照
    let (_, list) = app.req(Method::GET, "/api/resumes", None).await;
    let arr = list.as_array().unwrap();
    let auto_backup = arr.iter().find(|i| i["version_name"].as_str().unwrap().contains("解析前快照"));
    assert!(auto_backup.is_some(), "必须在解析前自动生成历史快照留档");
    assert_eq!(auto_backup.unwrap()["is_archived"], true);
}

#[tokio::test]
async fn test_application_soft_reference_to_resume_snapshot() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 保存简历并创建 2 个快照
    app.req(
        Method::PUT,
        "/api/resume",
        Some(json!({
            "raw_text": "赵六，基础架构资深工程师。",
            "name": "赵六简历"
        })),
    ).await;

    let (_, snap1) = app.req(Method::POST, "/api/resumes/snapshot", Some(json!({ "version_name": "快照A·微服务版" }))).await;
    let snap1_id = snap1["id"].as_i64().unwrap();

    let (_, snap2) = app.req(Method::POST, "/api/resumes/snapshot", Some(json!({ "version_name": "快照B·存储底座版" }))).await;
    let snap2_id = snap2["id"].as_i64().unwrap();

    // 2. 创建投递并绑定快照 A
    let (s, app_resp) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({
                "company_name": "蚂蚁集团",
                "position": "基础架构工程师",
                "resume_id": snap1_id
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let aid = app_resp["id"].as_i64().unwrap();

    // 3. 查询投递详情，断言软引用及其版本名已正确回查
    let (s, detail) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(detail["application"]["resume_id"], snap1_id);
    assert_eq!(detail["application"]["resume_version_name"], "快照A·微服务版");

    // 4. 更新投递切换软引用至快照 B
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/applications/{aid}"),
            Some(json!({ "resume_id": snap2_id })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    let (_, detail2) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(detail2["application"]["resume_id"], snap2_id);
    assert_eq!(detail2["application"]["resume_version_name"], "快照B·存储底座版");

    // 5. 删除快照 B，验证 ON DELETE SET NULL 软引用清空且投递完好
    let (s, _) = app.req(Method::DELETE, &format!("/api/resumes/{snap2_id}"), None).await;
    assert_eq!(s, StatusCode::OK);

    let (_, detail3) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert!(detail3["application"]["resume_id"].is_null(), "删除关联简历后投递的 resume_id 应置 NULL");
    assert_eq!(detail3["application"]["company"], "蚂蚁集团", "投递本身不受影响");
}

#[tokio::test]
async fn test_resume_markdown_export_with_version_selection() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 创建带有结构化信息的工作副本
    app.req(
        Method::PUT,
        "/api/resume",
        Some(json!({
            "raw_text": "工作副本原文",
            "name": "工作副本",
            "parsed": {
                "name": "工作副本张三",
                "summary": "工作副本摘要",
                "skills": ["Rust"]
            }
        })),
    ).await;

    // 2. 创建静态快照（不同内容）
    let (_, snap) = app.req(Method::POST, "/api/resumes/snapshot", Some(json!({ "version_name": "快照历史版" }))).await;
    let snap_id = snap["id"].as_i64().unwrap();

    // 3. 修改工作副本
    app.req(
        Method::PUT,
        "/api/resume",
        Some(json!({
            "raw_text": "修改后的工作副本原文",
            "name": "修改后的工作副本",
            "parsed": {
                "name": "新版张三",
                "summary": "新版摘要",
                "skills": ["Go", "Kubernetes"]
            }
        })),
    ).await;

    // 4. 导出默认工作副本
    let req1 = axum::http::Request::builder()
        .method(Method::GET)
        .uri("http://test/api/resume/export/markdown")
        .header("cookie", app.cookie.as_ref().unwrap())
        .body(axum::body::Body::empty())
        .unwrap();
    let resp1 = tower::ServiceExt::oneshot(&mut app.app.clone(), req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);
    let bytes1 = axum::body::to_bytes(resp1.into_body(), 64 * 1024).await.unwrap();
    let md1 = String::from_utf8(bytes1.to_vec()).unwrap();
    assert!(md1.contains("新版张三"), "默认导出应为最新工作副本");
    assert!(md1.contains("Kubernetes"));

    // 5. 指定 resume_id 导出快照版本
    let req2 = axum::http::Request::builder()
        .method(Method::GET)
        .uri(format!("http://test/api/resume/export/markdown?resume_id={snap_id}"))
        .header("cookie", app.cookie.as_ref().unwrap())
        .body(axum::body::Body::empty())
        .unwrap();
    let resp2 = tower::ServiceExt::oneshot(&mut app.app.clone(), req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let bytes2 = axum::body::to_bytes(resp2.into_body(), 64 * 1024).await.unwrap();
    let md2 = String::from_utf8(bytes2.to_vec()).unwrap();
    assert!(md2.contains("工作副本张三"), "指定 id 导出必须输出对应的快照内容");
    assert!(md2.contains("Rust"));
}
