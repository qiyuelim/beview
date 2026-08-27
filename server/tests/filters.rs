//! v4.1 评审整改：题库筛选修复（轮次级联失效）+ 积分流水日期筛选。
//!
//! - #4 根因：前端轮次下拉数据源走已退役的 /api/sessions/{id}/rounds，session 筛选器已删，
//!   导致轮次下拉恒空。修复：/api/rounds/all 支持 ?company= 过滤，前端按公司级联加载轮次。
//! - #5：/api/points/ledger 支持 from/to 日期过滤（created_at::date）。

mod common;

use axum::http::{Method, StatusCode};
use common::{create_application, create_round, TestApp};
use serde_json::json;

async fn create_question(app: &TestApp, round_id: i64, content: &str) -> i64 {
    let (s, q) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": round_id, "content": content })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    q["id"].as_i64().unwrap()
}

/// #4：/api/rounds/all?company= 只返回该公司的轮次；题库按公司/轮次筛选正确级联
#[tokio::test]
async fn rounds_all_filters_by_company_and_question_filters_cascade() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 公司 A：投递+轮次+题；公司 B：投递+轮次+题
    let aid_a = create_application(&app, "甲公司", "后端").await;
    let (s, r) = app
        .req(Method::POST, &format!("/api/applications/{aid_a}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let rid_a = r["id"].as_i64().unwrap();
    let qid_a = create_question(&app, rid_a, "甲公司题目").await;

    let aid_b = create_application(&app, "乙公司", "前端").await;
    let (s, r) = app
        .req(Method::POST, &format!("/api/applications/{aid_b}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let rid_b = r["id"].as_i64().unwrap();
    let _qid_b = create_question(&app, rid_b, "乙公司题目").await;

    // 公司 id（经投递列表反查）
    let (s, apps) = app.req(Method::GET, "/api/applications", None).await;
    assert_eq!(s, StatusCode::OK);
    let cid_a = apps
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["company"] == "甲公司")
        .and_then(|a| a["company_id"].as_i64())
        .expect("甲公司 id");

    // /rounds/all?company=甲 → 只含甲的轮次
    let (s, rounds) = app
        .req(Method::GET, &format!("/api/rounds/all?company={cid_a}"), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    let arr = rounds.as_array().unwrap();
    assert_eq!(arr.len(), 1, "应只返回甲公司的轮次：{rounds}");
    assert_eq!(arr[0]["round_id"].as_i64(), Some(rid_a));

    // 题库 company 筛选：只含甲的题
    let (s, qs) = app.req(Method::GET, &format!("/api/questions?company={cid_a}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let arr = qs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"].as_i64(), Some(qid_a));

    // 题库 round 筛选（级联后选轮次）：只含该轮的题
    let (s, qs) = app.req(Method::GET, &format!("/api/questions?round={rid_a}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(qs.as_array().unwrap().len(), 1);
}

/// #5：流水 from/to 日期过滤
#[tokio::test]
async fn ledger_filters_by_date_range() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 今天产生流水（建轮次 +300）
    let (aid, _) = {
        let aid = create_application(&app, "日期公司", "后端").await;
        let (s, r) = app
            .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
            .await;
        assert_eq!(s, StatusCode::CREATED);
        (aid, r["id"].as_i64().unwrap())
    };
    let _ = aid;

    // 与 DB 会话时区(UTC)一致：created_at::date 按 UTC 比较，避免跨午夜 Local 日期超前
    let today = chrono::Utc::now().date_naive();
    let yesterday = (today - chrono::Duration::days(1)).to_string();
    let tomorrow = (today + chrono::Duration::days(1)).to_string();

    // from=明天 → 空
    let (_, v) = app.req(Method::GET, &format!("/api/points/ledger?from={tomorrow}"), None).await;
    assert_eq!(v.as_array().unwrap().len(), 0, "未来日期不应有流水");
    // to=昨天 → 空
    let (_, v) = app.req(Method::GET, &format!("/api/points/ledger?to={yesterday}"), None).await;
    assert_eq!(
        v.as_array().unwrap().len(), 0,
        "过去截止不应包含今日流水; got: {}",
        serde_json::to_string(&v).unwrap()
    );
    // from=今天 → 有
    let (_, v) = app.req(Method::GET, &format!("/api/points/ledger?from={today}"), None).await;
    assert!(v.as_array().unwrap().len() >= 1);
    // from=昨天&to=今天 → 有
    let (_, v) = app
        .req(Method::GET, &format!("/api/points/ledger?from={yesterday}&to={today}"), None)
        .await;
    assert!(v.as_array().unwrap().len() >= 1);
}

/// 反馈 #7 复核：选「没有题的公司」应返回空数组（而非旧列表/报错）
#[tokio::test]
async fn questions_filter_with_question_less_company_returns_empty() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let aid = create_application(&app, "有题公司", "后端").await;
    let (s, r) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let _ = create_question(&app, r["id"].as_i64().unwrap(), "有题公司的题目").await;

    let (s, _) = app.req(Method::POST, "/api/companies", Some(json!({ "name": "无题公司" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, companies) = app.req(Method::GET, "/api/companies", None).await;
    let cid_b = companies
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "无题公司")
        .and_then(|c| c["id"].as_i64())
        .expect("无题公司 id");

    let (s, v) = app.req(Method::GET, &format!("/api/questions?company={cid_b}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 0, "无题公司应返回空数组，实际 {v}");

    // 其他条件同理：不存在的标签/未分析过滤在空集上也为空数组
    let (s, v) = app.req(Method::GET, "/api/questions?tag=不存在的标签", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v.as_array().unwrap().len(), 0);
}

/// 时间线排除系统容器（用户反馈 6）：自录题库/搜罗题容器投递不得渲染为「投递 · …」。
#[tokio::test]
async fn timeline_excludes_system_container_applications() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 自录题 → 触发「搜罗题」系统容器公司/岗位/投递链路
    let (s, _) = app
        .req(Method::POST, "/api/questions/self", Some(serde_json::json!({ "content": "自录冒烟题" })))
        .await;
    assert_eq!(s, 201);

    // 正向对照：真实投递必须出现在时间线
    let (s, _) = app
        .req(
            axum::http::Method::POST,
            "/api/applications",
            Some(serde_json::json!({ "company_name": "真实公司", "position": "后端" })),
        )
        .await;
    assert_eq!(s, 201);

    let (s, v) = app.req(axum::http::Method::GET, "/api/dashboard/activity", None).await;
    assert_eq!(s, 200);
    let text = serde_json::to_string(&v).unwrap();
    assert!(
        text.contains("投递 · 后端"),
        "真实投递应出现在时间线: {text}"
    );
    assert!(
        !text.contains("搜罗题"),
        "时间线不得出现系统容器（搜罗题/自录题库）: {text}"
    );
}

#[tokio::test]
async fn test_fsrs_memory_stats() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let aid = create_application(&app, "测试公司", "Go后端").await;
    let (_, r) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    let rid = r["id"].as_i64().unwrap();
    let qid = create_question(&app, rid, "GMP 调度模型与工作窃取机制").await;

    // 自评一次
    let (s, _) = app
        .req(
            Method::POST,
            &format!("/api/review/{qid}/grade"),
            Some(json!({ "result": "remembered" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // 查询 fsrs 记忆统计
    let (s, fsrs) = app.req(Method::GET, "/api/stats/fsrs-memory", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(fsrs["total_cards"].as_i64().unwrap(), 1);
    assert_eq!(fsrs["avg_retention"].as_f64().unwrap() >= 80.0, true);
    assert_eq!(fsrs["due_next_7_days"].as_array().unwrap().len(), 7);
}

/// 题库 tag/域名 筛选与 skill_id 筛选能递归下钻子树叶子题（#7 验证）
#[tokio::test]
async fn question_filter_by_domain_tag_matches_leaf_skills_subtree() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 获取技能树拿到 Rust 叶子技能 ID 及顶级根域 ID
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_root = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let root_id = hard_root["id"].as_i64().unwrap();

    let rust_node = tree.iter()
        .flat_map(|n| n["children"].as_array().cloned().unwrap_or_default())
        .flat_map(|n| n["children"].as_array().cloned().unwrap_or_default())
        .find(|n| n["name"] == "Rust 核心与并发")
        .expect("Rust 技能节点应存在");
    let rust_id = rust_node["id"].as_i64().unwrap();

    // 2. 录入题目并挂靠在叶子技能 Rust 上
    let aid = create_application(&app, "矩阵科技", "Rust专家").await;
    let (_, r) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" }))).await;
    let rid = r["id"].as_i64().unwrap();

    let (s, q_res) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "Rust Pin 与 Unpin 的本质与自引用结构体的内存安全",
            "my_answer": "保证内存在 move 时不发生地址漂移"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let qid = q_res["id"].as_i64().unwrap();

    // 绑定叶子技能
    let (s, _) = app.req(Method::POST, &format!("/api/questions/{qid}/skills"), Some(json!({ "skill_ids": [rust_id] }))).await;
    assert_eq!(s, StatusCode::OK);

    // 3. 验证通过 tag=专业技术与硬技能（中文根域名）可成功过滤出该题
    let (s, q_list1) = app.req(Method::GET, "/api/questions?tag=专业技术与硬技能", None).await;
    assert_eq!(s, StatusCode::OK);
    let ids1: Vec<i64> = q_list1.as_array().unwrap().iter().filter_map(|q| q["id"].as_i64()).collect();
    assert!(ids1.contains(&qid), "中文根域名应能通过子树递归下钻匹配到叶子题，实际: {ids1:?}");

    // 4. 验证通过 skill_id=<root_id>（根节点 ID）也可成功过滤出该题
    let (s, q_list2) = app.req(Method::GET, &format!("/api/questions?skill_id={root_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let ids2: Vec<i64> = q_list2.as_array().unwrap().iter().filter_map(|q| q["id"].as_i64()).collect();
    assert!(ids2.contains(&qid), "根域 skill_id 应能通过子树递归下钻匹配到叶子题，实际: {ids2:?}");
}

#[tokio::test]
async fn test_company_rename_and_system_protection() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 创建普通公司
    let aid1 = create_application(&app, "拼多多", "后端工程师").await;
    let (s, app_detail) = app.req(Method::GET, &format!("/api/applications/{aid1}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let cid1 = app_detail["application"]["company_id"].as_i64().unwrap();

    // 2. 修改公司名称与描述
    let (s, patch_res) = app.req(
        Method::PATCH,
        &format!("/api/companies/{cid1}"),
        Some(json!({
            "name": "PDD Holdings",
            "description": "电商头部企业"
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(patch_res["ok"], true);

    let (s, comp1) = app.req(Method::GET, &format!("/api/companies/{cid1}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(comp1["company"]["name"], "PDD Holdings");
    assert_eq!(comp1["company"]["description"], "电商头部企业");

    // 3. 创建另一家公司并测试重命名同名冲突
    let aid2 = create_application(&app, "淘宝", "架构师").await;
    let (s, app_detail2) = app.req(Method::GET, &format!("/api/applications/{aid2}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let cid2 = app_detail2["application"]["company_id"].as_i64().unwrap();

    let (s, conflict_res) = app.req(
        Method::PATCH,
        &format!("/api/companies/{cid2}"),
        Some(json!({ "name": "PDD Holdings" })),
    ).await;
    assert_eq!(s, StatusCode::CONFLICT, "同用户下同名公司应被 409 拒绝: {conflict_res:?}");
    assert!(conflict_res["error"].as_str().unwrap().contains("已存在"));

    // 4. 系统内置公司禁改名保护
    server::services::system_containers::ensure_self_round(&app.pool, 1).await.unwrap();
    let sys_cid: i64 = sqlx::query_scalar("SELECT id FROM companies WHERE is_system=true LIMIT 1")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    let (s, sys_res) = app.req(
        Method::PATCH,
        &format!("/api/companies/{sys_cid}"),
        Some(json!({ "name": "篡改系统公司名" })),
    ).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "系统公司应拒绝改名: {sys_res:?}");
    assert!(sys_res["error"].as_str().unwrap().contains("系统内置公司"));
}

#[tokio::test]
async fn test_question_type_crud_and_filter() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let aid = create_application(&app, "题型测试公司", "算法岗").await;
    let rid = create_round(&app, aid, "专业一面").await;

    // 1. 创建 coding 题型
    let (s, q1_res) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "手写 LRU 缓存与双向链表实现",
            "question_type": "coding"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let q1_id = q1_res["id"].as_i64().unwrap();

    // 2. 创建 principle 题型
    let (s, q2_res) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "解释 Raft 协议 Leader 选举机制",
            "question_type": "principle"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let q2_id = q2_res["id"].as_i64().unwrap();

    // 3. 过滤 coding 题型
    let (s, coding_list) = app.req(Method::GET, "/api/questions?question_type=coding", None).await;
    assert_eq!(s, StatusCode::OK);
    let c_ids: Vec<i64> = coding_list.as_array().unwrap().iter().filter_map(|q| q["id"].as_i64()).collect();
    assert!(c_ids.contains(&q1_id));
    assert!(!c_ids.contains(&q2_id));

    // 4. 更新 q1 为 troubleshooting
    let (s, _) = app.req(
        Method::PATCH,
        &format!("/api/questions/{q1_id}"),
        Some(json!({ "question_type": "troubleshooting" })),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // 5. 过滤 troubleshooting 题型
    let (s, trouble_list) = app.req(Method::GET, "/api/questions?question_type=troubleshooting", None).await;
    assert_eq!(s, StatusCode::OK);
    let t_ids: Vec<i64> = trouble_list.as_array().unwrap().iter().filter_map(|q| q["id"].as_i64()).collect();
    assert!(t_ids.contains(&q1_id));
}
