//! M1 模拟面试（一引擎三场景之 interview）——TDD 红测试。
//! 场景：用户能建一场模拟面试、发"开始"拿到流式第一题（并沉淀进题库）、
//! 答完拿即时判分（低分进错题本）、题数达标后 AI 收尾总结并结束场次。

mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;
use common::llm_mock::LlmMock;
use common::TestApp;

/// 用户能创建一场模拟面试并看到它出现在列表里
#[tokio::test]
async fn user_can_create_and_list_a_drill() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (sc, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "title": "后端一面", "position": "后端工程师", "direction": "Java", "stages": ["project", "basics"], "target_questions": 2 })),
        )
        .await;
    assert!(sc.is_success(), "创建模拟面试应成功, got {sc}");
    let did = d["id"].as_i64().expect("drill id");

    let (_, list) = app.req(Method::GET, "/api/drills", None).await;
    let ids: Vec<i64> = list
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["id"].as_i64()).collect())
        .unwrap_or_default();
    assert!(ids.contains(&did), "新建的面试应出现在列表");

    let (_, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(detail["status"], "ongoing", "新建场次应为进行中");
    assert_eq!(detail["position"], "后端工程师");
}

/// 发"开始"→ AI 流式出第一题（SSE delta）→ 消息落库(kind=question) → 题目沉淀进题库(source=ai_drill)
#[tokio::test]
async fn interview_streams_first_question_and_persists_it() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 2 })))
        .await;
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["请先".to_string(), "介绍".to_string(), "一下 HashMap 的实现原理。".to_string()]);

    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" })))
        .await;
    assert!(sc.is_success(), "start 消息应成功");
    assert!(body.contains("event: delta"), "应返回 SSE 流式事件，实际：{body}");
    assert!(body.contains("HashMap 的实现原理"), "第一题内容应出现在流里");
    assert!(body.contains("event: done"), "应有 done 收尾");

    // 消息落库：一条 AI question
    let (_, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let msgs = detail["messages"].as_array().cloned().unwrap_or_default();
    let qs: Vec<&serde_json::Value> = msgs.iter().filter(|m| m["kind"] == "question" && m["role"] == "ai").collect();
    assert_eq!(qs.len(), 1, "应有一条 AI 出的题");
    assert!(qs[0]["content"].as_str().unwrap_or("").contains("HashMap"));

    // 题目沉淀进题库：source=ai_drill 且带 drill_id
    let (_, questions) = app.req(Method::GET, "/api/questions", None).await;
    let qarr = questions.as_array().cloned().unwrap_or_default();
    let ai: Vec<&serde_json::Value> = qarr.iter().filter(|q| q["source"].as_str() == Some("ai_drill")).collect();
    assert_eq!(ai.len(), 1, "AI 出的题应沉淀进题库");
    assert!(ai[0]["content"].as_str().unwrap_or("").contains("HashMap"));
}

/// 用户回答 → AI 即时判分(score 消息) → 判低分(<60) 自动进错题本
#[tokio::test]
async fn low_scored_answer_lands_in_wrong_book() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 2 })))
        .await;
    let did = d["id"].as_i64().unwrap();

    // 第一题
    mock.queue_stream(vec!["请讲一下 HashMap 的实现原理。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;

    // 用户回答第一题 -> 进入追问（不触发独立判分）
    mock.queue_stream(vec!["追问：HashMap 在并发下扩容会有什么问题？".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "用数组和链表实现" }))).await;

    // 用户回答追问 -> 收尾轮（M4 两段式）：单次流内先出复盘、后附 REPORT(score=30)
    mock.queue_stream(vec![
        "# 🎯 全场复盘：HashMap 集群表现不足。\n\n## 🚀 四、靶向强化建议与行动指南\n补并发扩容。".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"哈希\"],\"difficulty\":3,\"ref_answer\":\"数组+链表\",\"score\":30,\"feedback\":\"回答不完整，缺少扩容细节\"}".to_string(),
    ]);
    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "死循环" })))
        .await;
    assert!(sc.is_success(), "回答应成功");
    assert!(body.contains("event: done"), "应流式完成");

    // score 消息落库，score=30
    let (_, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let msgs = detail["messages"].as_array().cloned().unwrap_or_default();
    let scores: Vec<&serde_json::Value> = msgs.iter().filter(|m| m["kind"] == "score").collect();
    assert_eq!(scores.len(), 1, "应有一条判分消息");
    assert_eq!(scores[0]["score"].as_i64(), Some(30));

    // 判低分 -> 错题本
    let (_, wrong) = app.req(Method::GET, "/api/review/wrong", None).await;
    let in_wrong = wrong
        .as_array()
        .map(|a| a.iter().any(|x| x["content"].as_str().unwrap_or("").contains("HashMap")))
        .unwrap_or(false);
    assert!(in_wrong, "判分<60 的题应进错题本");

    // 沉淀题带分析(score=30)
    let (_, q) = app.req(Method::GET, "/api/questions", None).await;
    let qarr = q.as_array().cloned().unwrap_or_default();
    let ai = qarr
        .iter()
        .find(|x| x["source"].as_str() == Some("ai_drill") && x["content"].as_str().unwrap_or("").contains("HashMap"))
        .cloned()
        .unwrap();
    assert_eq!(ai["last_score"].as_i64(), Some(30), "沉淀题应带判分");
}

/// 题数达标后：AI 给整场总结(summary) 且场次结束(finished)
#[tokio::test]
async fn interview_finishes_with_summary_after_target_questions() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 1 })))
        .await;
    let did = d["id"].as_i64().unwrap();

    // Q1
    mock.queue_stream(vec!["第一题：HashMap 原理。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;

    // 回答 Q1 -> 收尾轮（M4 两段式）：复盘正文先流出，REPORT(85) 后置
    mock.queue_stream(vec![
        "整场总结：你对哈希掌握不错，建议补一下并发扩容。".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"哈希\"],\"difficulty\":2,\"ref_answer\":\"数组+链表\",\"score\":85,\"feedback\":\"不错\"}".to_string(),
    ]);
    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "数组加链表解决冲突" })))
        .await;
    assert!(sc.is_success(), "回答应成功");
    assert!(body.contains("整场总结"), "应流式给出整场总结");

    let (_, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(detail["status"], "finished", "达标后场次应结束");
    let msgs = detail["messages"].as_array().cloned().unwrap_or_default();
    assert!(msgs.iter().any(|m| m["kind"] == "summary"), "应有 summary 消息");
    assert!(msgs.iter().any(|m| m["role"] == "user" && m["kind"] == "answer"), "应保留用户回答");
}

/// 场次标题自动可区分（含岗位+时间），参考内容进入出题 prompt（系统边界断言）
#[tokio::test]
async fn drill_title_is_distinct_and_references_feed_prompt() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (sc, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({
                "kind": "interview",
                "position": "后端工程师",
                "references": "岗位要求：熟悉 Redis、分布式锁、缓存一致性；常见面试题：缓存穿透怎么解决",
                "target_questions": 2,
            })),
        )
        .await;
    assert!(sc.is_success(), "建场应成功");
    let title = d["title"].as_str().unwrap_or("");
    assert!(title.contains("后端工程师"), "标题应含岗位: {title}");
    assert!(title.contains("模拟面试"), "标题应含场景: {title}");
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["请讲一下缓存穿透怎么解决。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;

    let bodies = mock.request_bodies();
    // ADR-0016：请求体为 Responses 形态——system 提升 instructions，对话在 input 数组
    let has_ref = bodies.iter().any(|b| {
        b["instructions"].as_str().map_or(false, |s| s.contains("缓存穿透"))
            || b["input"].as_array().map_or(false, |arr| {
                arr.iter().any(|m| m["content"].as_str().unwrap_or("").contains("缓存穿透"))
            })
    });
    assert!(has_ref, "出题 prompt 应包含参考内容");
}

/// 多轮链式上下文（ADR-0016 D7 补充 + 用户指令）：模型开启 store 时，
/// 第二轮请求应带 previous_response_id = 上一响应顶层 id（UUID 形态，非 msg_* id），
/// 且不再重放【对话历史】；首轮无 previous_response_id。
#[tokio::test]
async fn drill_multi_turn_chains_via_previous_response_id_when_store_enabled() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();

    // 开启 store 的模型配置（previous_response_id 链式要求上游留存响应）
    let (sc, v) = app
        .req(
            Method::PUT,
            "/api/settings/llm-config",
            Some(json!({
                "providers": [{ "id": "p1", "name": "Mock", "base_url": mock.base_url(), "api_key": "" }],
                "models": [{
                    "id": "m1", "provider_id": "p1", "name": "mock",
                    "context_length": 128000,
                    "caps": { "structured_output": true, "web_search": false },
                    "advanced": { "store": true, "reasoning_effort": "none" }
                }],
                "active_model_id": "m1"
            })),
        )
        .await;
    assert_eq!(sc, 200, "put llm-config 失败: {v}");

    let (sc, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "position": "后端工程师", "target_questions": 2 })),
        )
        .await;
    assert!(sc.is_success(), "建场应成功");
    let did = d["id"].as_i64().unwrap();

    // 第一轮：出题（全量模式，无 previous_response_id）
    mock.queue_stream(vec!["第一题：讲讲 HashMap".to_string()]);
    let (sc, _) = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    assert!(sc.is_success(), "首轮应成功");

    // 第二轮：回答后请求下一题（应链式：带 previous_response_id，不重放历史）
    mock.queue_stream(vec!["第二题：讲讲 ConcurrentHashMap".to_string()]);
    let (sc, body) = app
        .req_raw(
            Method::POST,
            &format!("/api/drills/{did}/messages"),
            Some(json!({ "content": "数组加链表解决冲突" })),
        )
        .await;
    assert!(sc.is_success(), "第二轮应成功: {body}");

    let bodies = mock.request_bodies();
    assert!(bodies.len() >= 2, "应有两次 LLM 请求");
    // 首轮：不带 previous_response_id
    let first = &bodies[0];
    assert!(first.get("previous_response_id").is_none(), "首轮不应有 previous_response_id: {first}");
    // 第二轮：带上一响应顶层 id（UUID 形态），绝不重放历史
    let second = bodies
        .iter()
        .find(|b| b["input"].as_array().map_or(false, |arr| {
            arr.iter().any(|m| m["content"].as_str().unwrap_or("").contains("候选人最新回答"))
        }))
        .expect("第二轮应为链式增量消息（含候选人最新回答）");
    let prev = second["previous_response_id"].as_str().expect("第二轮必须携带 previous_response_id");
    assert!(
        prev.starts_with("f0dbb153-117f-9bbf-8176-5284b47f"),
        "previous_response_id 应为 mock 发出的响应顶层 id（UUID 形态）: {prev}"
    );
    assert!(!prev.starts_with("msg_"), "绝不能使用 output 里消息的 msg_* id: {prev}");
    let second_input = second["input"].as_array().unwrap();
    let input_text = second_input.iter().filter_map(|m| m["content"].as_str()).collect::<Vec<_>>().join("\n");
    assert!(
        !input_text.contains("第一题：讲讲 HashMap") && !input_text.contains("开始"),
        "链式模式不重放历史消息（首题文本/用户首轮消息都不应出现）: {input_text}"
    );
}

/// store=false（隐私默认）时不启用链式：每轮都走全量重放，不携带 previous_response_id。
#[tokio::test]
async fn drill_stays_full_replay_when_store_disabled() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await; // 迁移默认 store=false

    let (sc, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 2 })))
        .await;
    assert!(sc.is_success());
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["第一题".to_string()]);
    let _ = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    mock.queue_stream(vec!["第二题".to_string()]);
    let _ = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "回答一" }))).await;

    for b in mock.request_bodies() {
        assert!(
            b.get("previous_response_id").is_none(),
            "store=false 不应启用链式: {b}"
        );
    }
    // 全量重放模式仍工作：历史文本出现在 user 消息里（合并为单条）
    let has_history_replay = mock.request_bodies().iter().any(|b| {
        b["input"].as_array().map_or(false, |arr| {
            arr.iter().any(|m| m["content"].as_str().unwrap_or("").contains("【对话历史】"))
        })
    });
    assert!(has_history_replay, "全量重放模式应包含对话历史块");
}

/// 提示 3 级阶梯式生成 + 用户带 hint_level 提交回答 + 判分注入提示扣分规则
#[tokio::test]
async fn progressive_hint_and_hint_level_scoring() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 1 })))
        .await;
    let did = d["id"].as_i64().unwrap();

    // 1. 开始出题
    mock.queue_stream(vec!["请解释 Redis 的跳表实现细节。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;

    // 2. 请求提示 (hint)
    mock.queue_stream(vec![
        "### Level 1: 思考方向\n从双向链表与多层索引跳跃切入。\n\n### Level 2: 核心原理\n通过随机层高实现 O(logN) 查询。\n\n### Level 3: 关键解法\nzskiplistNode 包含 forward 指针和 span 跨度。".to_string()
    ]);
    let (sc, body) = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "action": "hint", "content": "" }))).await;
    assert!(sc.is_success());
    assert!(body.contains("Level 1: 思考方向") && body.contains("Level 2: 核心原理") && body.contains("Level 3: 关键解法"));

    // 3. 用户解锁至 Level 2 提示并提交作答，hint_level = 2
    mock.queue_stream(vec![
        "本场面试复盘报告：表现良好。".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"Redis\",\"跳表\"],\"difficulty\":4,\"ref_answer\":\"多层随机索引链表\",\"score\":65,\"feedback\":\"结合了 Level 2 原理提示作答，独立思考有所欠缺，扣除相应分数\"}".to_string(),
    ]);
    let (sc, body2) = app.req_raw(
        Method::POST,
        &format!("/api/drills/{did}/messages"),
        Some(json!({
            "action": "answer",
            "content": "跳表通过随机生成节点的层高，配合多层索引跳跃实现对数时间复杂度查找",
            "hint_level": 2
        }))
    ).await;
    assert!(sc.is_success());
    assert!(body2.contains("event: feedback"));

    // 验证判分请求携带了 hint_level 扣分与考量说明
    let bodies = mock.request_bodies();
    let grade_req = bodies.iter().find(|b| {
        b["input"].as_array().map_or(false, |arr| {
            arr.iter().any(|m| m["content"].as_str().unwrap_or("").contains("Level 2 核心原理"))
        })
    });
    assert!(grade_req.is_some(), "判分请求中应携带 Level 2 提示使用情况说明");
}

/// 追问回答与主回答分别入库 + 分析结果 answer_snapshot 与主回答一致 + 题目详情直接命中分析
#[tokio::test]
async fn drill_followup_and_analysis_sync_to_questions_detail() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 2 })))
        .await;
    let did = d["id"].as_i64().unwrap();

    // 1. 开始第一题
    mock.queue_stream(vec!["请解释 MySQL 的 InnoDB 锁机制。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;

    // 2. 回答第一题 -> M4 两段式追问：模型自主决定深挖，输出 PROBE 元数据行 + 题干
    mock.queue_stream(vec![
        r#"<<<PROBE>>>{"anchor_keyword":"隔离级别","reason":"depth_probe"}"#.to_string(),
        "追问：Next-Key Lock 在什么隔离级别下生效？它是如何避免幻读的？".to_string(),
    ]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "主要有行锁、表锁、间隙锁和 Next-Key Lock" }))).await;

    // 3. 回答追问，题完结并产出结构化复盘
    mock.queue_stream(vec![
        "# 🎯 模拟面试全场综合复盘报告\n\n## 📊 一、综合表现评级与总评\n- **总评等级**：【A 良好】".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"MySQL\",\"锁机制\"],\"difficulty\":4,\"ref_answer\":\"Next-Key Lock 在可重复读 (RR) 隔离级别下生效，结合记录锁与间隙锁避免幻读\",\"score\":88,\"feedback\":\"回答准确，对 Next-Key Lock 与 RR 级别机制阐述清晰\"}".to_string(),
    ]);
    let (sc, body) = app.req_raw(
        Method::POST,
        &format!("/api/drills/{did}/messages"),
        Some(json!({
            "action": "answer",
            "content": "Next-Key Lock 在 RR 隔离级别下生效，锁住记录本身及其前面的间隙，防止其他事务插入新行导致幻读"
        }))
    ).await;
    assert!(sc.is_success());
    assert!(body.contains("event: feedback"));

    // 验证题库中的主考题与追问题目
    let (_, questions) = app.req(Method::GET, "/api/questions", None).await;
    let qarr = questions.as_array().cloned().unwrap_or_default();
    let main_q = qarr.iter().find(|q| q["content"].as_str().unwrap_or("").contains("InnoDB 锁机制")).expect("主考题应沉淀入库");
    let main_qid = main_q["id"].as_i64().unwrap();

    // 验证主考题的 my_answer 是主考题回答，而不是追问回答
    let (_, qdetail) = app.req(Method::GET, &format!("/api/questions/{main_qid}"), None).await;
    assert_eq!(qdetail["my_answer"], "主要有行锁、表锁、间隙锁和 Next-Key Lock", "主考题 my_answer 应为主考题回答");

    // 验证 analyses 中的 answer_snapshot 与主考题 my_answer 严格一致
    let analyses = qdetail["analyses"].as_array().cloned().unwrap_or_default();
    assert_eq!(analyses.len(), 1, "应有一条分析记录");
    assert_eq!(analyses[0]["score"], 88, "评分应为 88");
    assert_eq!(analyses[0]["answer_snapshot"], "主要有行锁、表锁、间隙锁和 Next-Key Lock");

    // 验证追问题目也已记录并在 followups 中
    let followups = qdetail["followups"].as_array().cloned().unwrap_or_default();
    assert_eq!(followups.len(), 1, "应有一条追问题目");
    assert!(followups[0]["content"].as_str().unwrap_or("").contains("Next-Key Lock"));
    assert!(followups[0]["my_answer"].as_str().unwrap_or("").contains("RR 隔离级别下生效"), "追问题目的 my_answer 应正确入库");

    // 验证复盘报告格式
    let req_bodies = mock.request_bodies();
    let summary_req = req_bodies.iter().find(|b| {
        b["input"].as_array().map_or(false, |arr| {
            arr.iter().any(|m| m["content"].as_str().unwrap_or("").contains("🎯 模拟面试全场综合复盘报告"))
        })
    });
    assert!(summary_req.is_some(), "收尾复盘应携带结构化 Markdown 复盘报告 Prompt 模板");
}

/// 未启动前禁止 hint/skip/finish，回答中包含“跳过/提示”关键词绝不被误判为快捷动作
#[tokio::test]
async fn pre_start_actions_rejected_and_no_fuzzy_matching_hijack() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 1 })))
        .await;
    let did = d["id"].as_i64().unwrap();

    // 1. 未开始前尝试 hint / skip / finish，应被拦截为 400 Bad Request
    let (sc1, _) = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "action": "hint", "content": "" }))).await;
    assert_eq!(sc1, StatusCode::BAD_REQUEST);

    let (sc2, _) = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "action": "skip", "content": "" }))).await;
    assert_eq!(sc2, StatusCode::BAD_REQUEST);

    let (sc3, _) = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "action": "finish", "content": "" }))).await;
    assert_eq!(sc3, StatusCode::BAD_REQUEST);

    // 检查消息表为空，没有任何空消息入库
    let (_, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(detail["messages"].as_array().unwrap().len(), 0, "未开考前拦截的请求绝不落库任何空消息");

    // 2. 正常启动面试 (action: 'start')
    mock.queue_stream(vec!["请讲讲如何对包含大量 NULL 值的字段进行跳过扫描优化？".to_string()]);
    let (sc_start, _) = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "action": "start", "content": "" }))).await;
    assert_eq!(sc_start, StatusCode::OK);

    // 3. 用户回答包含“提示”和“跳过”字样（例如：“在优化器中可以加入索引提示，提示优化器跳过空值”，action 为 answer）
    mock.queue_stream(vec![
        "# 🎯 模拟面试全场综合复盘报告\n\n## 📊 一、综合表现评级与总评\n- **总评等级**：【A 良好】".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"MySQL\",\"索引\"],\"difficulty\":3,\"ref_answer\":\"使用 index hint 强制走局部索引或覆盖索引\",\"score\":85,\"feedback\":\"准确\"}".to_string(),
    ]);

    let (sc_ans, body_ans) = app.req_raw(
        Method::POST,
        &format!("/api/drills/{did}/messages"),
        Some(json!({ "content": "在优化器中可以加入索引提示（Index Hint），提示优化器跳过不需要的回表扫描", "action": "answer" }))
    ).await;
    assert_eq!(sc_ans, StatusCode::OK);
    assert!(body_ans.contains("event: feedback"), "正常判分，绝不被误判为 skip/hint");

    // 验证回答落库且评分正常（非跳过的 0 分）
    let (_, detail2) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let msgs = detail2["messages"].as_array().unwrap();
    let ans_msg = msgs.iter().find(|m| m["role"] == "user" && m["kind"] == "answer").expect("应记录考生回答");
    assert_eq!(ans_msg["score"], 85, "评分应为 85，证明未被误判为跳过(0分)");
}

/// 靶向攻坚真圈题：传入顶级知识域 ID 或叶子技能 ID 列表时，自动圈定子树真题（N1 验证）
#[tokio::test]
async fn targeted_drill_auto_circles_descendant_skills() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. 初始化技能树
    let (_, body) = app.req(Method::GET, "/api/skills/tree", None).await;
    let tree = body["tree"].as_array().unwrap();
    let hard_skills = tree.iter().find(|n| n["name"] == "专业技术与硬技能").unwrap();
    let root_id = hard_skills["id"].as_i64().unwrap();

    let rust_node = tree.iter()
        .flat_map(|n| n["children"].as_array().cloned().unwrap_or_default())
        .flat_map(|n| n["children"].as_array().cloned().unwrap_or_default())
        .find(|n| n["name"] == "Rust 核心与并发")
        .expect("Rust 技能节点应存在");
    let rust_id = rust_node["id"].as_i64().unwrap();

    // 2. 录入一道挂靠在叶子技能 Rust 上的真题
    let (_, q_res) = app.req(
        Method::POST,
        "/api/questions/self",
        Some(json!({
            "content": "请分析 Tokio 异步运行时的调度器工作窃取算法",
            "my_answer": "多工作线程 + 双端队列 work stealing",
            "skill_id": rust_id
        })),
    ).await;
    let _qid = q_res["id"].as_i64().unwrap();

    // 3. 以顶级根域 ID + 域标签发起靶向攻坚模考（完全模拟 Skills.tsx → DrillNew.tsx 兜底链路）
    let (sc, d) = app.req(
        Method::POST,
        "/api/drills",
        Some(json!({
            "kind": "interview",
            "title": "靶向攻坚 · 专业技术与硬技能",
            "dossier": {
                "skill_id": root_id,
                "tags": ["专业技术与硬技能"],
                "summary": "专业技术与硬技能"
            }
        })),
    ).await;
    assert_eq!(sc, StatusCode::OK);
    let did = d["id"].as_i64().unwrap();

    // 验证考官题本成功自动圈定挂靠在该顶级域子树下的真题（不受 tags 冲突影响）
    let (_, detail) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let dossier = &detail["dossier"];
    let qs = dossier["questions"].as_array().expect("dossier.questions 应存在");
    assert!(!qs.is_empty(), "顶级域 skill_id + tags 组合应能递归圈定其子树下的叶子真题");
    assert!(qs[0]["content"].as_str().unwrap().contains("Tokio 异步运行时"));
}
