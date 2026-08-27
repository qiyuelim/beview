//! v5 M2 岗位精准押题与资产流转集成测试

use axum::http::{Method, StatusCode};
use serde_json::json;

mod common;
use common::*;

#[tokio::test]
async fn test_position_predict_and_ingest_flow() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 创建公司与带 JD 的岗位
    let (s, c_res) = app.req(
        Method::POST,
        "/api/companies",
        Some(json!({ "name": "未来智行科技" })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let cid = c_res["id"].as_i64().unwrap();

    let (s, p_res) = app.req(
        Method::POST,
        &format!("/api/companies/{cid}/positions"),
        Some(json!({
            "title": "高并发分布式系统专家",
            "jd_text": "负责核心支付与交易结算系统，要求熟练掌握 Rust/Go 高并发架构、分布式事务与一致性协议。"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let pid = p_res["id"].as_i64().unwrap();

    // 2. 模拟题目流转沉淀入题库 (/predict/ingest)
    let sample_questions = vec![
        json!({
            "content": "请详细分析在跨行转账场景下，Saga 模式与两阶段提交（2PC）的权衡与故障恢复机制",
            "category": "分布式事务与一致性",
            "focus_points": ["Saga 补偿事务", "2PC 同步阻塞与单点故障", "幂等与去重表"],
            "sample_direction": "先对比强一致与最终一致性适用场景，再给出转账冲正设计",
            "probability": 95
        }),
        json!({
            "content": "Rust 中如何基于 Tokio 构建百万长连接并发网关？如何避免内存碎片与 Epoll 惊群？",
            "category": "Rust 并发与网络",
            "focus_points": ["Tokio 运行时架构", "异步 IO 与缓冲区复用", "反压机制"],
            "sample_direction": "阐述 Work-stealing 调度器及 Zero-copy 切片设计",
            "probability": 88
        }),
    ];

    let (s, ingest_res) = app.req(
        Method::POST,
        &format!("/api/positions/{pid}/predict/ingest"),
        Some(json!({ "questions": sample_questions })),
    ).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ingest_res["created_count"].as_i64().unwrap(), 2);
    let qids = ingest_res["question_ids"].as_array().unwrap();
    assert_eq!(qids.len(), 2);

    let first_qid = qids[0].as_i64().unwrap();

    // 3. 验证题库详情中的内容与标签
    let (s, q_detail) = app.req(Method::GET, &format!("/api/questions/{first_qid}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(q_detail["content"].as_str().unwrap().contains("Saga 模式"));
    let tags = q_detail["tags"].as_array().unwrap();
    let tag_strs: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tag_strs.contains(&"岗位押题"));
    assert!(tag_strs.contains(&"分布式事务与一致性"));
    assert!(tag_strs.contains(&"高并发分布式系统专家"));

    // 4. 验证已自动入待复习队列
    let (s, rev_res) = app.req(Method::GET, "/api/review/stats", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(rev_res["due"].as_i64().unwrap() >= 2);
}

#[tokio::test]
async fn test_position_predict_drill_flow() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 创建公司与岗位
    let (s, c_res) = app.req(Method::POST, "/api/companies", Some(json!({ "name": "星际网络" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let cid = c_res["id"].as_i64().unwrap();

    let (s, p_res) = app.req(
        Method::POST,
        &format!("/api/companies/{cid}/positions"),
        Some(json!({
            "title": "基础设施架构师",
            "jd_text": "负责大规模 Kubernetes 集群与服务网格治理"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let pid = p_res["id"].as_i64().unwrap();

    // 2. 以押题发起模拟试卷练习 (/predict/drill)
    let (s, drill_res) = app.req(
        Method::POST,
        &format!("/api/positions/{pid}/predict/drill"),
        Some(json!({
            "title": "星际网络·架构师专项考前冲刺",
            "questions": [
                {
                    "content": "K8s CNI 插件中 Cilium 基于 eBPF 的网络路由与 iptables 模式相比有何本质差异？",
                    "category": "容器网络",
                    "focus_points": ["eBPF XDP hook", "绕过 conntrack 瓶颈"],
                    "sample_direction": "从内核空间数据包转发路径与 CPU 消耗展开",
                    "probability": 90
                }
            ]
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let drill_id = drill_res["drill_id"].as_i64().unwrap();

    // 3. 验证 drills 详情（v5.1 精简收拢为携带专属题本的 interview 模拟面试）
    let (s, d_detail) = app.req(Method::GET, &format!("/api/drills/{drill_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(d_detail["title"].as_str().unwrap(), "星际网络·架构师专项考前冲刺");
    assert_eq!(d_detail["kind"].as_str().unwrap(), "interview");
    assert!(d_detail["dossier"].is_object(), "应保存考官专属题本");
}

#[tokio::test]
async fn test_position_predict_with_resume_llm_flow() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 1. 创建用户简历
    let (s, _) = app.req(
        Method::PUT,
        "/api/resume",
        Some(json!({
            "raw_text": "5年 Rust 后端研发，精通高并发与分布式架构",
            "parsed": { "skills": ["Rust", "PostgreSQL", "Raft"] }
        })),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // 2. 创建公司与岗位
    let (s, c_res) = app.req(Method::POST, "/api/companies", Some(json!({ "name": "极速算力" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let cid = c_res["id"].as_i64().unwrap();

    let (s, p_res) = app.req(
        Method::POST,
        &format!("/api/companies/{cid}/positions"),
        Some(json!({
            "title": "分布式存储开发专家",
            "jd_text": "负责自研分布式 KV 存储引擎研发，深入理解 LSM-Tree、WAL 与 MVCC 并并发控制。"
        })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);
    let pid = p_res["id"].as_i64().unwrap();

    // 3. Mock LLM 押题返回
    let predict_out = json!({
        "questions": [
            {
                "content": "LSM-Tree 在高写入压力下 Compaction 产生写放大该如何优化？",
                "category": "存储引擎",
                "focus_points": ["Tiered Compaction", "Leveled Compaction", "Rate Limiter"],
                "sample_direction": "对比大小紧凑策略与分层策略的写放大与读放大权衡",
                "probability": 92
            }
        ]
    }).to_string();
    mock.queue_nonstream(&predict_out);

    // 4. 发起押题：受理即返回 job，结果落 positions.predict_result（ADR-0013）
    let (s, res) = app.req(Method::POST, &format!("/api/positions/{pid}/predict"), None).await;
    assert_eq!(s, StatusCode::OK, "押题接口携带简历时应正常返回 200: {res}");
    assert_eq!(res["status"], "running", "应受理为后台任务: {res}");
    let job_id = res["job_id"].as_u64().expect("应返回 job_id");

    let done = common::wait_ai_job(&app, job_id, 5000).await;
    assert_eq!(done["status"], "done", "押题任务应完成: {done}");

    let (s, pos) = app.req(Method::GET, &format!("/api/positions/{pid}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let qs = pos["predict_result"]["questions"].as_array().expect("应落库押题结果");
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0]["probability"], 92);
    assert_eq!(pos["ai_jobs"].as_array().map(|a| a.len()).unwrap_or(0), 0);
}

/// 押题 running 时域 GET 暴露 ai_jobs，重复 POST 幂等去重
#[tokio::test]
async fn test_position_predict_job_refresh_restore_and_dedup() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (s, c_res) = app.req(Method::POST, "/api/companies", Some(json!({ "name": "刷新恢复司" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let cid = c_res["id"].as_i64().unwrap();
    let (s, p_res) = app
        .req(
            Method::POST,
            &format!("/api/companies/{cid}/positions"),
            Some(json!({ "title": "后端", "jd_text": "负责支付核心链路与一致性。" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let pid = p_res["id"].as_i64().unwrap();

    mock.set_delay_ms(1500);
    mock.queue_nonstream(
        r#"{"summary":"高频","questions":[{"content":"讲讲幂等","category":"基础","focus_points":["去重"],"sample_direction":"先业务后实现","probability":80}]}"#,
    );

    let (s, v1) = app.req(Method::POST, &format!("/api/positions/{pid}/predict"), None).await;
    assert_eq!(s, StatusCode::OK);
    let job1 = v1["job_id"].as_u64().unwrap();

    let (_, d) = app.req(Method::GET, &format!("/api/positions/{pid}"), None).await;
    let jobs = d["ai_jobs"].as_array().expect("running 时应暴露 ai_jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["id"].as_u64(), Some(job1));
    assert_eq!(jobs[0]["kind"], "position_predict");
    assert_eq!(jobs[0]["status"], "running");

    let (s2, v2) = app.req(Method::POST, &format!("/api/positions/{pid}/predict"), None).await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(v2["job_id"].as_u64(), Some(job1), "running 中重复触发应去重");

    let done = common::wait_ai_job(&app, job1, 8000).await;
    assert_eq!(done["status"], "done");
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let llm_calls = mock.request_bodies().iter().filter(|b| b["stream"] != json!(true)).count();
    assert_eq!(llm_calls, 1, "同岗 running 去重后 LLM 只应被调用一次");
}
