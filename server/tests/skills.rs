//! v5 M1 技能图谱与能力雷达集成测试

use axum::http::{Method, StatusCode};
use serde_json::json;

mod common;
use common::*;

fn find_node<'a>(nodes: &'a [serde_json::Value], name: &str) -> Option<&'a serde_json::Value> {
    for n in nodes {
        if n["name"].as_str() == Some(name) {
            return Some(n);
        }
        if let Some(children) = n["children"].as_array() {
            if let Some(found) = find_node(children, name) {
                return Some(found);
            }
        }
    }
    None
}

#[tokio::test]
async fn test_skills_seed_and_tree_hierarchy() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 首次获取技能树自动种子初始化
    let (status, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    assert_eq!(status, StatusCode::OK);
    
    let tree = body["tree"].as_array().expect("tree should be array");
    assert_eq!(tree.len(), 6, "应该严格有 6 个系统顶级知识领域");
    
    let radar = body["radar"].as_array().expect("radar should be array");
    assert_eq!(radar.len(), 6, "全景能力雷达维度应为严格 6 维");

    // 验证顶级节点名称
    let root_names: Vec<&str> = tree.iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert!(root_names.contains(&"专业技术与硬技能"));
    assert!(root_names.contains(&"系统设计与架构思考"));
    assert!(root_names.contains(&"业务理解与行业实操"));
    assert!(root_names.contains(&"工程落地与质量调优"));
    assert!(root_names.contains(&"项目协作与团队管理"));
    assert!(root_names.contains(&"问题分析与通用素养"));

    // 验证底层子考点
    let rust_node = find_node(tree, "Rust 核心与并发");
    assert!(rust_node.is_some(), "底层考点「Rust 核心与并发」应存在于技能树中");
}

#[tokio::test]
async fn test_skill_crud() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_skills = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let hard_skills_id = hard_skills["id"].as_i64().unwrap();

    // 1. 在顶级领域下创建子知识专区
    let (s, res) = app.req(
        Method::POST,
        "/api/skills",
        Some(json!({
            "name": "AI 与大模型应用",
            "icon": "Brain",
            "parent_id": hard_skills_id
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let zone_id = res["id"].as_i64().unwrap();

    // 2. 创建具体考点
    let (s, c_res) = app.req(
        Method::POST,
        "/api/skills",
        Some(json!({
            "name": "Prompt 工程与 RAG",
            "icon": "ChatDots",
            "parent_id": zone_id
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let child_id = c_res["id"].as_i64().unwrap();

    // 3. 创建同义重复考点用于测试合并
    let (s, dup_res) = app.req(
        Method::POST,
        "/api/skills",
        Some(json!({
            "name": "Prompt 提示词设计",
            "icon": "ChatDots",
            "parent_id": zone_id
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let dup_id = dup_res["id"].as_i64().unwrap();

    // 4. 将同义考点合并至标准考点
    let (s, merge_res) = app.req(
        Method::POST,
        &format!("/api/skills/{dup_id}/merge"),
        Some(json!({
            "target_id": child_id
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(merge_res["source_id"].as_i64().unwrap(), dup_id);
    assert_eq!(merge_res["target_id"].as_i64().unwrap(), child_id);

    // 5. 修改考点名称
    let (s, _) = app.req(
        Method::PATCH,
        &format!("/api/skills/{child_id}"),
        Some(json!({
            "name": "Prompt 工程与 Agent 开发"
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // 6. 删除考点
    let (s, _) = app.req(Method::DELETE, &format!("/api/skills/{child_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn test_question_skills_binding_and_proficiency_calculation() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 获取技能树拿到 Rust 技能 ID
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let rust_node = find_node(tree, "Rust 核心与并发").expect("Rust 技能节点应存在");
    let rust_id = rust_node["id"].as_i64().unwrap();

    // 2. 创建真实面试并录入题目
    let aid = create_application(&app, "Rust 科技", "后端专家").await;
    let rid = create_round(&app, aid, "技术一面").await;

    let (s, q_res) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "请详细解释 Rust 的所有权、借用检查与生命周期机制",
            "my_answer": "Rust 依靠所有权和 RAII 实现无 GC 内存安全..."
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let qid = q_res["id"].as_i64().unwrap();

    // 3. 绑定题目与技能
    let (s, _) = app.req(
        Method::POST,
        &format!("/api/questions/{qid}/skills"),
        Some(json!({ "skill_ids": [rust_id] })),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // 验证获取绑定技能
    let (s, list) = app.req(Method::GET, &format!("/api/questions/{qid}/skills"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list.as_array().unwrap()[0]["id"].as_i64().unwrap(), rust_id);

    // 4. 自评该卡片（remembered）
    let (s, _) = app.req(
        Method::POST,
        &format!("/api/review/{qid}/grade"),
        Some(json!({ "result": "remembered" })),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // 5. 再次查询技能图谱与能力矩阵：题目数与掌握度应发生联动，主列 questions.skill_id 必须同步（N2 验证）
    let (_, detail) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert_eq!(detail["skill_name"].as_str().unwrap(), "Rust 核心与并发", "改绑后 questions.skill_id 应同步更新");

    let (_, matrix) = app.req(Method::GET, "/api/skills/matrix", None).await;
    let cells = matrix["cells"].as_array().expect("cells should be array");
    let hard_cell = cells.iter().find(|c| c["domain"] == "专业技术与硬技能" && c["question_type"] == "professional_knowledge").unwrap();
    assert_eq!(hard_cell["count"].as_i64().unwrap(), 1, "二维矩阵应感知多对多技能归属");

    let (_, graph) = app.req(Method::GET, "/api/skills/tree", None).await;
    assert_eq!(graph["total_tagged_questions"].as_i64().unwrap(), 1);

    let tree2 = graph["tree"].as_array().unwrap();
    let rust_node2 = find_node(tree2, "Rust 核心与并发").unwrap();
    let hard_root = tree2.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();

    assert_eq!(rust_node2["question_count"].as_i64().unwrap(), 1);
    assert!(rust_node2["proficiency"].as_i64().unwrap() > 0, "掌握度应大于 0");
    assert!(hard_root["question_count"].as_i64().unwrap() >= 1, "顶级知识域题目计数应被向上汇总");
}

#[tokio::test]
async fn test_unmapped_tags_and_ingest_flow() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 录入带新标签的题目
    let (_, q_res) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({
            "content": "Raft 协议的选主逻辑与安全性",
            "tags": ["分布式共识", "Raft"]
        })),
    ).await;
    let _ = q_res["id"].as_i64().unwrap();

    // 2. 查看未建树标签池
    let (s, unmapped) = app.req(Method::GET, "/api/skills/unmapped-tags", None).await;
    assert_eq!(s, StatusCode::OK);
    let list = unmapped.as_array().expect("unmapped should be array");
    let tag_names: Vec<&str> = list.iter().map(|item| item["tag"].as_str().unwrap()).collect();
    assert!(tag_names.contains(&"Raft") || tag_names.contains(&"分布式共识"));

    // 3. 一键沉淀为技能树节点
    let (s, ingest_res) = app.req(
        Method::POST,
        "/api/skills/ingest-tag",
        Some(json!({
            "tag": "Raft",
            "parent_id": null
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    assert!(ingest_res["id"].as_i64().is_some());

    // 4. 再次查询技能树，Raft 已成为技能节点且自动关联了题目
    let (_, graph) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = graph["tree"].as_array().unwrap();
    let raft_node = find_node(tree, "Raft");
    assert!(raft_node.is_some(), "Raft 应该已成为技能节点");
    assert_eq!(raft_node.unwrap()["question_count"].as_i64().unwrap(), 1);
}

#[tokio::test]
async fn test_question_followups_flow() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 录入包含一级子追问的题目
    let (s, q_res) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({
            "content": "谈谈你对 HashMap 的理解",
            "my_answer": "数组+链表+红黑树",
            "followups": [
                {
                    "content": "HashMap 扩容死循环是怎么产生的？",
                    "my_answer": "JDK 1.7 头插法逆序"
                }
            ]
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let qid = q_res["id"].as_i64().unwrap();

    // 2. 题库列表应该只返回 1 道主题目（追问不增加独立题量）
    let (_, list_res) = app.req(Method::GET, "/api/questions", None).await;
    let rows = list_res.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"].as_i64().unwrap(), qid);
    assert_eq!(rows[0]["followup_count"].as_i64().unwrap(), 1);

    // 3. 进入题目详情，应该包含所属的追问
    let (_, detail) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let followups = detail["followups"].as_array().expect("followups should be array");
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0]["content"].as_str().unwrap(), "HashMap 扩容死循环是怎么产生的？");

    // 4. 追加一条追问
    let (s, f_res) = app.req(
        Method::POST,
        &format!("/api/questions/{qid}/followups"),
        Some(json!({
            "content": "ConcurrentHashMap 在 JDK 1.8 做了什么优化？",
            "my_answer": "CAS + synchronized 锁桶头"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(f_res["parent_id"].as_i64().unwrap(), qid);

    // 5. 再次查询详情，追问增至 2 轮
    let (_, detail2) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let followups2 = detail2["followups"].as_array().unwrap();
    assert_eq!(followups2.len(), 2);
    let f1_id = followups2[0]["id"].as_i64().unwrap();
    let f2_id = followups2[1]["id"].as_i64().unwrap();

    // 验证追问在 question_rounds 中均有对应关联记录（N3 验证）
    let (s, r1) = app.req(Method::GET, &format!("/api/questions/{f1_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!r1["round_links"].as_array().unwrap().is_empty(), "批量创建的追问应有关联轮次记录");

    let (s, r2) = app.req(Method::GET, &format!("/api/questions/{f2_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!r2["round_links"].as_array().unwrap().is_empty(), "追加创建的追问应有关联轮次记录");
}

/// 追问评价捆绑（用户裁决 2a）：主题目回答评价首次执行时，一并给「有现场回答且尚无评分」
/// 的追问补评价；后续重新评价主题目不再重评追问（评价只跟第一手现场记录走）。
#[tokio::test]
async fn followup_answers_evaluated_with_consolidated_parent_analysis() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (s, q_res) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({
            "content": "谈谈 MySQL 索引",
            "my_answer": "B+ 树",
            "followups": [{"content": "为什么用 B+ 树不用跳表？", "my_answer": "磁盘 IO 友好"}]
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let qid = q_res["id"].as_i64().unwrap();

    // 统一合并评价（v5.3 规格）：主问与追问拼成整体上下文只请求一次 LLM
    mock.queue_nonstream(r#"{"score":85,"feedback":"整体回答扎实，对比了 B+ 树与跳表的磁盘 IO 特性"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/analyze"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 8000).await;

    let (_, detail) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert_eq!(detail["last_score"].as_i64().unwrap(), 85, "主题目获得合并评分");
    assert!(detail["last_feedback"].as_str().unwrap().contains("整体回答扎实"));

    // 验证 LLM 收到的请求包含了主问与追问完整对话
    let req_bodies = mock.request_bodies();
    let last_req = req_bodies.last().unwrap();
    let body_str = serde_json::to_string(last_req).unwrap();
    assert!(body_str.contains("磁盘 IO 友好"));

    // 验证追问本身不生成分裂的孤立 analysis 记录
    let child_id = detail["followups"][0]["id"].as_i64().unwrap();
    let (child_n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM analyses WHERE question_id=$1",
    )
    .bind(child_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(child_n, 0, "追问不单独落库分析记录，统一归并于主题目合并分析");
}

/// 标签聚合清洗（用户裁决 3）：LLM 给出合并建议 → 人工确认应用 → 别名标签全局并入规范名。
#[tokio::test]
async fn tag_cleanup_propose_and_apply_merges_aliases() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    for (c, t) in [("Q1", "JVM"), ("Q2", "Java内存模型"), ("Q3", "Redis")] {
        let (s, _) = app.req(
            Method::POST,
            "/api/questions/self",
            Some(json!({ "content": c, "tags": [t] })),
        ).await;
        assert_eq!(s, StatusCode::CREATED);
    }

    // 未建树标签池应包含三个自由标签
    let (_, pool) = app.req(Method::GET, "/api/skills/unmapped-tags", None).await;
    let names: Vec<&str> = pool.as_array().unwrap().iter().map(|t| t["tag"].as_str().unwrap()).collect();
    assert!(names.contains(&"JVM") && names.contains(&"Java内存模型") && names.contains(&"Redis"));

    // LLM 清洗建议（strict 契约）
    mock.queue_nonstream(
        r#"{"groups":[{"canonical":"JVM","aliases":["Java内存模型"],"note":"同属 JVM 内存域"}]}"#,
    );
    let (s, propose) = app.req(Method::POST, "/api/skills/tags/cleanup/propose", None).await;
    assert_eq!(s, StatusCode::OK, "{propose}");
    assert_eq!(propose["groups"][0]["canonical"], "JVM");

    // 人工确认后应用：别名并入规范名
    let (s, applied) = app.req(
        Method::POST,
        "/api/skills/tags/cleanup/apply",
        Some(json!({ "groups": [{ "canonical": "JVM", "aliases": ["Java内存模型"] }] })),
    ).await;
    assert_eq!(s, StatusCode::OK, "{applied}");

    // Q2 的标签已并入 JVM；别名标签消失
    let (_, list) = app.req(Method::GET, "/api/questions", None).await;
    let rows = list.as_array().unwrap();
    let q2 = rows.iter().find(|r| r["content"] == "Q2").unwrap();
    assert_eq!(q2["tags"], json!(["JVM"]), "别名应并入规范名");
    let (_, pool2) = app.req(Method::GET, "/api/skills/unmapped-tags", None).await;
    let names2: Vec<&str> = pool2.as_array().unwrap().iter().map(|t| t["tag"].as_str().unwrap()).collect();
    assert!(!names2.contains(&"Java内存模型"), "别名标签应被删除");
    assert!(names2.contains(&"Redis"), "无关标签不受影响");
}

/// v5.4 Ticket 02: merge-to-skill 一次性迁移流
/// 验证：标签归组建议支持映射至技能节点，人工确认应用后，题目关联批量挂靠至技能树 (question_skills + skill_id)，且原规范标签降级保留为卡片展示标签。
#[tokio::test]
async fn test_merge_to_skill_migration_workflow_rebinds_questions_and_preserves_display_tags() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 1. 初始化技能树并自建一个专业技能节点
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_skills = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let hard_skills_id = hard_skills["id"].as_i64().unwrap();

    let (s, created_skill) = app.req(
        Method::POST,
        "/api/skills",
        Some(json!({ "name": "Rust异步与并发", "parent_id": hard_skills_id })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let target_skill_id = created_skill["id"].as_i64().unwrap();

    // 2. 创建 2 道带有分散自由标签（无 skill_id 绑定）的历史题目
    let (s1, q1) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({ "content": "Tokio Runtime 工作窃取原理", "tags": ["tokio-rs"] })),
    ).await;
    assert_eq!(s1, StatusCode::CREATED);
    let q1_id = q1["id"].as_i64().unwrap();

    let (s2, q2) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({ "content": "Future 与 Pin 机制解析", "tags": ["rust-future"] })),
    ).await;
    assert_eq!(s2, StatusCode::CREATED);
    let q2_id = q2["id"].as_i64().unwrap();

    // 3. 触发 propose 建议
    mock.queue_nonstream(
        r#"{"groups":[{"canonical":"Rust异步","aliases":["tokio-rs","rust-future"],"note":"归纳为 Rust 异步技术栈"}]}"#,
    );
    let (s, propose) = app.req(Method::POST, "/api/skills/tags/cleanup/propose", None).await;
    assert_eq!(s, StatusCode::OK);
    let prop_groups = propose["groups"].as_array().unwrap();
    assert_eq!(prop_groups[0]["canonical"], "Rust异步");
    // 验证后端自动匹配到了 "Rust异步与并发" 技能节点
    assert_eq!(prop_groups[0]["target_skill_id"], target_skill_id);
    assert_eq!(prop_groups[0]["target_skill_name"], "Rust异步与并发");

    // 4. 用户人工确认并应用 merge-to-skill 迁移
    let (s, applied) = app.req(
        Method::POST,
        "/api/skills/tags/cleanup/apply",
        Some(json!({
            "groups": [{
                "canonical": "Rust异步",
                "aliases": ["tokio-rs", "rust-future"],
                "target_skill_id": target_skill_id
            }]
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(applied["ok"], true);

    // 5. 验证题目列表与展示标签：
    // - 标签展示保留为规范名 "Rust异步"（历史数据零丢失）
    // - 题目的 skill_id 均已正确回填为 target_skill_id
    let (_, list) = app.req(Method::GET, "/api/questions", None).await;
    let rows = list.as_array().unwrap();
    let q1_row = rows.iter().find(|r| r["id"] == q1_id).unwrap();
    let q2_row = rows.iter().find(|r| r["id"] == q2_id).unwrap();

    assert_eq!(q1_row["tags"], json!(["Rust异步"]));
    assert_eq!(q2_row["tags"], json!(["Rust异步"]));
    assert_eq!(q1_row["skill_id"], target_skill_id);
    assert_eq!(q2_row["skill_id"], target_skill_id);

    // 6. 验证按 skill_id 过滤可以直接检索出迁移后的题目
    let (_, skill_filtered) = app.req(Method::GET, &format!("/api/questions?skill_id={target_skill_id}"), None).await;
    let skill_rows = skill_filtered.as_array().unwrap();
    assert_eq!(skill_rows.len(), 2, "按技能 ID 应精确命中 2 道迁移后的题目");

    // 7. 验证按顶层领域子树过滤也能级联命中
    let (_, domain_filtered) = app.req(Method::GET, "/api/questions?tag=专业技术与硬技能", None).await;
    let domain_rows = domain_filtered.as_array().unwrap();
    assert!(domain_rows.iter().any(|r| r["id"] == q1_id));
    assert!(domain_rows.iter().any(|r| r["id"] == q2_id));
}

#[tokio::test]
async fn test_seed_tree_preserves_custom_skills_and_bindings() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始化技能树
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_skills = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let hard_skills_id = hard_skills["id"].as_i64().unwrap();

    // 2. 用户自建一个技能节点并绑定一道题目
    let (s, created) = app.req(
        Method::POST,
        "/api/skills",
        Some(json!({ "name": "自定义领域专家技能", "parent_id": hard_skills_id })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let custom_skill_id = created["id"].as_i64().unwrap();

    let (s, q) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({
            "content": "自建考点绑定的核心题目",
            "skill_id": custom_skill_id
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let qid = q["id"].as_i64().unwrap();

    let (s, _) = app.req(
        Method::POST,
        &format!("/api/questions/{qid}/skills"),
        Some(json!({ "skill_ids": [custom_skill_id] })),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // 3. 再次触发 seed_tree (重置/补齐预置技能树)
    let (s, res) = app.req(Method::POST, "/api/skills/seed", None).await;
    assert_eq!(s, StatusCode::CREATED);
    let new_tree = res["tree"].as_array().unwrap();

    // 4. 断言：自定义节点依然完好保留，题目依然关联在该节点上
    let custom_node = find_node(new_tree, "自定义领域专家技能");
    assert!(custom_node.is_some(), "自定义技能节点必须完整保留，绝不被清空删除");
    assert_eq!(custom_node.unwrap()["question_count"], 1, "自建节点的题目计数必须保持一致");

    let (s, skills_list) = app.req(Method::GET, &format!("/api/questions/{qid}/skills"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(skills_list.as_array().unwrap().len(), 1);
    assert_eq!(skills_list.as_array().unwrap()[0]["id"].as_i64().unwrap(), custom_skill_id, "题目的技能挂靠绝不丢失");
}

#[tokio::test]
async fn test_resolve_or_create_skill_prevents_rogue_top_level_domain() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 模拟 LLM 返回了一个非标准的 L1 顶级领域（如 "餐饮外卖履约架构"）
    let new_skill = server::contracts::question::NewSkillItem {
        l1: "餐饮外卖履约架构".to_string(),
        l2: "超时预警调度".to_string(),
        l3: "分层时间轮机制".to_string(),
    };

    let sid = server::services::skill_service::resolve_or_create_skill(
        &app.pool,
        1, // admin user_id
        None,
        Some(&new_skill),
    )
    .await
    .expect("应成功解析并创建技能")
    .expect("必须返回有效技能 ID");

    // 2. 查询根节点列表：根节点总数必须严格等于 6，绝不能产生第七个根节点
    let roots: Vec<String> = sqlx::query_scalar("SELECT name FROM skills WHERE user_id=$1 AND parent_id IS NULL ORDER BY id ASC")
        .bind(1i64)
        .fetch_all(&app.pool)
        .await
        .unwrap();
    assert_eq!(roots.len(), 6, "顶级域必须严格保持为 6 大系统域，绝不被污染新增根节点: {:?}", roots);

    // 3. 验证新创建的技能节点挂靠在推导出的系统顶级域（"系统设计与架构思考"）之下
    let skill_path: String = sqlx::query_scalar("SELECT path FROM skills WHERE id=$1")
        .bind(sid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(skill_path.starts_with("/architecture"), "异名顶级域应正确收敛归入 /architecture 下: {skill_path}");
}

#[tokio::test]
async fn test_matrix_cell_count_matches_filtered_question_list_identity() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始化技能树
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_skills = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let hard_skills_children = hard_skills["children"].as_array().unwrap();
    let rust_cat = hard_skills_children.first().unwrap();
    let rust_skills = rust_cat["children"].as_array().unwrap();
    assert!(rust_skills.len() >= 2, "应至少有两个 Rust 子技能");
    let s1_id = rust_skills[0]["id"].as_i64().unwrap();
    let s2_id = rust_skills[1]["id"].as_i64().unwrap();

    // 2. 创建一个投递与轮次
    let aid = create_application(&app, "矩阵恒等测试公司", "Rust架构师").await;
    let rid = create_round(&app, aid, "技术一面").await;

    // 3. 创建多道不同题型题目
    let (s, q_res) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "请详细解释 Rust 所有权与生命周期协变机制？",
            "my_answer": "所有权通过 RAII 管理资源，生命周期协变保证引用有效性。",
            "skill_ids": [s1_id, s2_id],
            "question_type": "professional_knowledge"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let _qid = q_res["id"].as_i64().unwrap();

    let (s2, q_res2) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "在高并发场景下如何排查并消除 Tokio 任务死锁？",
            "my_answer": "通过 tracing 收集 span，结合 console 排查阻塞 future。",
            "skill_ids": [s1_id],
            "question_type": "problem_solving_resilience"
        })),
    ).await;
    assert_eq!(s2, StatusCode::CREATED);
    let _qid2 = q_res2["id"].as_i64().unwrap();

    // 4. 查询能力矩阵
    let (s, matrix) = app.req(Method::GET, "/api/skills/matrix", None).await;
    assert_eq!(s, StatusCode::OK);
    let cells = matrix["cells"].as_array().unwrap();

    // 5. 遍历所有非空单元格，断言其 count 恒等于 /api/questions?tag=...&question_type=...
    for cell in cells {
        let domain = cell["domain"].as_str().unwrap();
        let q_type = cell["question_type"].as_str().unwrap();
        let count = cell["count"].as_i64().unwrap();
        if count > 0 {
            let (sc, list) = app.req(
                Method::GET,
                &format!("/api/questions?tag={domain}&question_type={q_type}"),
                None,
            ).await;
            assert_eq!(sc, StatusCode::OK);
            let list_rows = list.as_array().unwrap();
            assert_eq!(count, list_rows.len() as i64, "单元格 ({domain}, {q_type}) 计数 {count} 必须恒等于列表返回数 {}", list_rows.len());
        }
    }

    // 6. 具体单元格精准断言
    let hard_principle_cell = cells.iter().find(|c| {
        c["domain"] == "专业技术与硬技能" && c["question_type"] == "professional_knowledge"
    }).unwrap();
    assert_eq!(hard_principle_cell["count"], 1);

    let hard_trouble_cell = cells.iter().find(|c| {
        c["domain"] == "专业技术与硬技能" && c["question_type"] == "problem_solving_resilience"
    }).unwrap();
    assert_eq!(hard_trouble_cell["count"], 1);
}

#[tokio::test]
async fn test_graph_strict_depth_invariant_and_healing() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始化树
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_domain = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let hard_id = hard_domain["id"].as_i64().unwrap();

    // 2. 通过 API 创建 L2 与 L3
    let (_, l2_res) = app.req(Method::POST, "/api/skills", Some(json!({
        "name": "深度测试L2专区",
        "icon": "Folder",
        "parent_id": hard_id
    }))).await;
    let l2_id = l2_res["id"].as_i64().unwrap();

    let (_, l3_res) = app.req(Method::POST, "/api/skills", Some(json!({
        "name": "深度测试L3考点",
        "icon": "FileCode",
        "parent_id": l2_id
    }))).await;
    let l3_id = l3_res["id"].as_i64().unwrap();

    // 3. 尝试在 L3 下再次创建子节点（深度守卫：应拒绝创建 L4，就地收敛至 l3_id）
    let (_, l4_attempt) = app.req(Method::POST, "/api/skills", Some(json!({
        "name": "非法L4超深考点",
        "icon": "TreeStructure",
        "parent_id": l3_id
    }))).await;
    // 由于深度守卫，返回的 ID 必须是 l3_id 或受到约束
    assert_eq!(l4_attempt["id"].as_i64().unwrap(), l3_id, "深度守卫应防止创建 L4 并收敛至 L3");

    // 4. 模拟数据库中存在的存量脏数据：手动插入相邻同名节点和 L4 节点
    let dirty_dup_id: i64 = sqlx::query_scalar(
        "INSERT INTO skills (user_id, parent_id, name, path, icon) VALUES ($1, $2, $3, $4, $5) RETURNING id"
    )
    .bind(1i64)
    .bind(l3_id)
    .bind("深度测试L3考点") // 与 parent 同名
    .bind("/hard/deep/dup")
    .bind("TreeStructure")
    .fetch_one(&app.pool)
    .await
    .unwrap();

    // 5. 触发 healing
    server::services::skill_service::heal_tree_depth_and_duplicates(&app.pool, 1).await.unwrap();

    // 6. 验证同名脏节点已被自愈清除
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM skills WHERE id=$1)")
        .bind(dirty_dup_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(!exists, "相邻同名脏节点必须在 healing 后被彻底清理并合并");
}

#[tokio::test]
async fn test_skill_graph_node_count_matches_filtered_question_list_identity() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始化技能树
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_skills = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let hard_id = hard_skills["id"].as_i64().unwrap();
    let hard_children = hard_skills["children"].as_array().unwrap();
    let rust_cat = hard_children.first().unwrap();
    let rust_cat_id = rust_cat["id"].as_i64().unwrap();
    let rust_skills = rust_cat["children"].as_array().unwrap();
    let s1_id = rust_skills[0]["id"].as_i64().unwrap();
    let s2_id = rust_skills[1]["id"].as_i64().unwrap();

    // 2. 创建投递与轮次
    let aid = create_application(&app, "恒等测试第二条公司", "Rust技术专家").await;
    let rid = create_round(&app, aid, "终面").await;

    // 3. 创建多道题目：
    // Q1: 关联 s1
    // Q2: 关联 s1 和 s2
    // Q3: 关联 s2
    // Q4: 无任何技能关联（NULL 技能题）
    let (_s, _q1) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "Q1: Rust 所有权生命周期",
            "skill_ids": [s1_id]
        })),
    ).await;
    assert_eq!(_s, StatusCode::CREATED);

    let (_s, _q2) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "Q2: Rust 并发与 channel 源码",
            "skill_ids": [s1_id, s2_id]
        })),
    ).await;
    assert_eq!(_s, StatusCode::CREATED);

    let (_s, _q3) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "Q3: Rust 异步运行时调度",
            "skill_ids": [s2_id]
        })),
    ).await;
    assert_eq!(_s, StatusCode::CREATED);

    let (_s, _q4) = app.req(
        Method::POST,
        "/api/questions",
        Some(json!({
            "round_id": rid,
            "content": "Q4: 通用性格测试（无技能绑定）"
        })),
    ).await;
    assert_eq!(_s, StatusCode::CREATED);

    // 4. 获取图谱数据
    let (_, tree_res) = app.req(Method::GET, "/api/skills/tree", None).await;
    let new_tree = tree_res["tree"].as_array().unwrap();

    // 5. 递归收集树中全部节点
    fn collect_all_nodes<'a>(nodes: &'a [serde_json::Value], out: &mut Vec<&'a serde_json::Value>) {
        for n in nodes {
            out.push(n);
            if let Some(ch) = n["children"].as_array() {
                collect_all_nodes(ch, out);
            }
        }
    }
    let mut all_nodes = Vec::new();
    collect_all_nodes(new_tree, &mut all_nodes);

    // 6. 恒等测试 #2：遍历每个节点，断言 node.question_count == GET /api/questions?skill_id={id} 长度
    for node in all_nodes {
        let nid = node["id"].as_i64().unwrap();
        let nname = node["name"].as_str().unwrap();
        let node_q_cnt = node["question_count"].as_i64().unwrap();

        let (status, list_body) = app.req(Method::GET, &format!("/api/questions?skill_id={nid}"), None).await;
        assert_eq!(status, StatusCode::OK);
        let list = list_body.as_array().unwrap();

        assert_eq!(
            node_q_cnt,
            list.len() as i64,
            "节点「{}」(id={}) 的 question_count ({}) 必须恒等于列表过滤行数 ({})",
            nname, nid, node_q_cnt, list.len()
        );
    }

    // 特别断言关键节点计数：
    // s1 命中 Q1, Q2 => 2
    // s2 命中 Q2, Q3 => 2
    // rust_cat (L2) 命中 Q1, Q2, Q3 => 3
    // hard_skills (L1) 命中 Q1, Q2, Q3 => 3
    let (_s, s1_list) = app.req(Method::GET, &format!("/api/questions?skill_id={s1_id}"), None).await;
    assert_eq!(s1_list.as_array().unwrap().len(), 2);
    let (_s, s2_list) = app.req(Method::GET, &format!("/api/questions?skill_id={s2_id}"), None).await;
    assert_eq!(s2_list.as_array().unwrap().len(), 2);
    let (_s, cat_list) = app.req(Method::GET, &format!("/api/questions?skill_id={rust_cat_id}"), None).await;
    assert_eq!(cat_list.as_array().unwrap().len(), 3);
    let (_s, hard_list) = app.req(Method::GET, &format!("/api/questions?skill_id={hard_id}"), None).await;
    assert_eq!(hard_list.as_array().unwrap().len(), 3);
}

/// 恒等测试 #3 (ADR-0018 D4 / Ticket 03): 靶向圈出的题集 ⊆ 对应节点子树过滤结果
/// 断言：
/// 1. 任何节点作为靶向圈题入参时，dossier 圈出的题目 ID 集合严格是该节点子树过滤结果的子集 (subset)；
/// 2. 当该节点子树存在题目时，dossier 圈出的题目非空且所见即所得；
/// 3. 空守卫场景（节点子树下无题）不报错，dossier.questions 为空数组且流程平稳进行。
#[tokio::test]
async fn test_targeted_drill_dossier_questions_subset_of_skill_subtree_identity() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始化技能树
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();

    let hard_skills = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let hard_id = hard_skills["id"].as_i64().unwrap();
    let hard_children = hard_skills["children"].as_array().unwrap();
    let rust_cat = hard_children.first().unwrap();
    let rust_cat_id = rust_cat["id"].as_i64().unwrap();
    let rust_skills = rust_cat["children"].as_array().unwrap();
    let s1_id = rust_skills[0]["id"].as_i64().unwrap();
    let s2_id = rust_skills[1]["id"].as_i64().unwrap();

    let soft_skills = tree.iter().find(|n| n["name"] == "项目协作与团队管理").unwrap();
    let soft_id = soft_skills["id"].as_i64().unwrap();

    // 2. 录入多道题目并绑定技能
    let (s, q1) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({ "content": "恒等#3 Q1: Tokio 异步", "skill_id": s1_id })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let q1_id = q1["id"].as_i64().unwrap();

    let (s, q2) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({ "content": "恒等#3 Q2: Tokio 内存", "skill_id": s2_id })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let q2_id = q2["id"].as_i64().unwrap();

    let (s, q_soft) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({ "content": "恒等#3 Q3: 跨团队沟通冲突", "skill_id": soft_id })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let q_soft_id = q_soft["id"].as_i64().unwrap();

    // 3. 测试 4 个场景节点（叶子节点、分类目录、根域、空节点）
    let test_cases = vec![
        ("叶子技能 s1", s1_id, vec![q1_id]),
        ("分类目录 rust_cat", rust_cat_id, vec![q1_id, q2_id]),
        ("根域 hard_skills", hard_id, vec![q1_id, q2_id]),
        ("软技能域 soft_skills", soft_id, vec![q_soft_id]),
    ];

    for (desc, test_node_id, expected_members) in test_cases {
        // a. 查题库列表过滤结果（基准集合）
        let (s, list_res) = app.req(Method::GET, &format!("/api/questions?skill_id={test_node_id}"), None).await;
        assert_eq!(s, StatusCode::OK);
        let list_qids: Vec<i64> = list_res.as_array().unwrap().iter().map(|q| q["id"].as_i64().unwrap()).collect();

        for expected_id in &expected_members {
            assert!(list_qids.contains(expected_id), "{} 的列表过滤结果必须包含题目 {}", desc, expected_id);
        }

        // b. 发起靶向模考圈题
        let (s, d_res) = app.req(
            Method::POST,
            "/api/drills",
            Some(json!({
                "kind": "interview",
                "title": format!("靶向模考 · {}", desc),
                "dossier": {
                    "skill_id": test_node_id,
                    "summary": desc
                }
            })),
        ).await;
        assert_eq!(s, StatusCode::OK);
        let did = d_res["id"].as_i64().unwrap();

        // c. 查询详情中的 dossier.questions
        let (s, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
        assert_eq!(s, StatusCode::OK);
        let dossier_qs = detail["dossier"].get("questions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let dossier_qids: Vec<i64> = dossier_qs.iter().map(|q| q["id"].as_i64().unwrap()).collect();

        // 恒等断言 1: 靶向圈出的题集 ⊆ 列表过滤结果
        for qid in &dossier_qids {
            assert!(
                list_qids.contains(qid),
                "场景 [{}] 圈出的题目 ID {} 必须在列表子树过滤结果 ({:?}) 中",
                desc, qid, list_qids
            );
        }

        // 恒等断言 2: 圈出集合非空且数量一致（在 limit 10 内）
        assert_eq!(dossier_qids.len(), expected_members.len(), "场景 [{}] 圈出的题目数量应等于预期子树题目数", desc);
    }

    // 4. 空守卫场景测试：圈一个完全没有题目的全新自建技能节点
    let (s, empty_skill) = app.req(
        Method::POST,
        "/api/skills",
        Some(json!({ "name": "无题目的全新考点", "parent_id": hard_id })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let empty_skill_id = empty_skill["id"].as_i64().unwrap();

    let (s, d_empty) = app.req(
        Method::POST,
        "/api/drills",
        Some(json!({
            "kind": "interview",
            "title": "靶向模考 · 空题集场景",
            "dossier": {
                "skill_id": empty_skill_id
            }
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    let did_empty = d_empty["id"].as_i64().unwrap();

    let (s, detail_empty) = app.req(Method::GET, &format!("/api/drills/{did_empty}"), None).await;
    assert_eq!(s, StatusCode::OK);
    // 空守卫场景行为不变：dossier 正常保留，questions 为空或不存在，建场成功
    let d = &detail_empty["dossier"];
    if let Some(qs) = d.get("questions").and_then(|v| v.as_array()) {
        assert!(qs.is_empty(), "空考点圈题结果应为空数组");
    }
}


