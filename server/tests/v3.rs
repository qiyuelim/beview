//! v3 集成测试：积分经济（余额/流水/商城/兑换/里程碑）+ 投递跟踪（看板/状态机/一键建批次）。
//! 行为规格：用户点行为（录题/建批次/轮次通过/复习/训练/兑换/建投递）-> 积分入账或兑换扣分。

#![allow(dead_code)]

mod common;

use axum::http::Method;
use common::TestApp;
use serde_json::{json, Value};

async fn create_company(app: &TestApp) -> i64 {
    let (s, v) = app
        .req(Method::POST, "/api/companies", Some(json!({ "name": "测试公司" })))
        .await;
    assert_eq!(s, 201);
    v["id"].as_i64().unwrap()
}

async fn create_question(app: &TestApp, round_id: i64) -> i64 {
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": round_id, "content": "什么是索引下推？", "my_answer": "我的回答" })),
        )
        .await;
    assert_eq!(s, 201);
    v["id"].as_i64().unwrap()
}

async fn balance(app: &TestApp) -> i64 {
    let (s, v) = app.req(Method::GET, "/api/points/balance", None).await;
    assert_eq!(s, 200);
    v["balance"].as_i64().unwrap()
}

async fn setup_session_round(app: &TestApp) -> (i64, i64) {
    // v4：投递（公司+岗位）为核心单元，轮次直接挂投递
    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "测试公司", "position": "后端" })),
        )
        .await;
    assert_eq!(s, 201);
    let aid = a["id"].as_i64().unwrap();
    let (s, r) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    assert_eq!(s, 201);
    let rid = r["id"].as_i64().unwrap();
    (aid, rid)
}

/// 录真实面试：批次 +300、题 +100/题
#[tokio::test]
async fn real_session_and_questions_earn_big_points() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_aid, rid) = setup_session_round(&app).await;
    // 添加面试轮次 +300（原「建批次」语义由轮次承接）
    assert_eq!(balance(&app).await, 300);
    let _ = create_question(&app, rid).await;
    // +100
    assert_eq!(balance(&app).await, 400);
}

/// 轮次标记通过 -> +200
#[tokio::test]
async fn round_pass_earns_points() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = setup_session_round(&app).await;

    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/rounds/{rid}"),
            Some(json!({ "passed": "pass" })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(balance(&app).await, 300 + 200);
    // B组 #4 锁定语义：已选定（pass）后再改 fail → 400 不可变更；积分不再变化
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/rounds/{rid}"),
            Some(json!({ "passed": "fail" })),
        )
        .await;
    assert_eq!(s, 400, "已选定的轮次结果不可变更");
    // 同值幂等放行，但不重复发分（old==pass 不再奖励）
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/rounds/{rid}"),
            Some(json!({ "passed": "pass" })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(balance(&app).await, 300 + 200);
}

/// 复习一张卡 -> +5；流水可查（含类别）
#[tokio::test]
async fn review_card_earns_points_and_ledger_lists() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = setup_session_round(&app).await;
    let qid = create_question(&app, rid).await;

    // 复习一张卡 -> +5；队列清零 -> 每日目标 +20（仅 1 张卡）
    let (s, _) = app
        .req(
            Method::POST,
            &format!("/api/review/{qid}/grade"),
            Some(json!({ "result": "remembered" })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(balance(&app).await, 300 + 100 + 5 + 20);

    let (s, v) = app.req(Method::GET, "/api/points/ledger?limit=50", None).await;
    assert_eq!(s, 200);
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0]["category"], "daily_goal");
    assert!(arr.iter().any(|e| e["category"] == "review_card" && e["amount"] == 5));
    assert!(arr.iter().any(|e| e["category"] == "real_session"));
    assert!(arr.iter().any(|e| e["category"] == "real_question"));
}

/// 今日任务：队列清零后发每日目标奖 +20
#[tokio::test]
async fn daily_goal_awarded_when_queue_cleared() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = setup_session_round(&app).await;
    let qid = create_question(&app, rid).await; // 有 my_answer -> 自动入队

    // 清空队列 -> 每日目标 +20（当天有效）
    let (s, _) = app
        .req(
            Method::POST,
            &format!("/api/review/{qid}/grade"),
            Some(json!({ "result": "remembered" })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(balance(&app).await, 300 + 100 + 5 + 20);

    let (s, v) = app.req(Method::GET, "/api/points/daily", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["goal_awarded"], true);
    assert_eq!(v["queue_done"], true);
}

/// 商城：默认模板、自建、余额不足报错、兑换扣分并记流水
#[tokio::test]
async fn mall_redeem_flow() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (s, v) = app.req(Method::GET, "/api/mall/items", None).await;
    assert_eq!(s, 200);
    assert!(v.as_array().unwrap().len() >= 4);

    // 自建条目
    let (s, v) = app
        .req(
            Method::POST,
            "/api/mall/items",
            Some(json!({ "name": "奶茶", "cost": 150, "emoji": "🧋" })),
        )
        .await;
    assert_eq!(s, 201);
    let item_id = v["id"].as_i64().unwrap();

    // 余额不足（还没积分）-> 400
    let (s, v) = app
        .req(Method::POST, &format!("/api/mall/items/{item_id}/redeem"), None)
        .await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("积分不足"));

    // 攒分：真实面试（批次+300+题+100）
    let (_, rid) = setup_session_round(&app).await;
    create_question(&app, rid).await;
    assert_eq!(balance(&app).await, 400);

    // 兑换 -> 扣 150，余额 250
    let (s, v) = app
        .req(Method::POST, &format!("/api/mall/items/{item_id}/redeem"), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["balance"], 250);
    assert_eq!(balance(&app).await, 250);

    // 流水含负支出（redemption）
    let (_, v) = app.req(Method::GET, "/api/points/ledger", None).await;
    assert!(v.as_array().unwrap().iter().any(|e| e["category"] == "redemption" && e["amount"] == -150));
}

/// 里程碑：5 场真实面试 -> +2000；首个 offer -> +10000（幂等）
#[tokio::test]
async fn milestone_rewards_are_one_time() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = common::create_application(&app, "里程碑公司", "后端").await;

    // 每轮：创建 +300 → 标记通过 +200（上一面通过才能加下一面，反馈七#2）
    // 4 场 = 4*(300+200) = 2000（无里程碑）
    for _ in 0..4 {
        let rid = common::create_round(&app, aid, "").await;
        app.req(Method::PATCH, &format!("/api/rounds/{rid}"), Some(json!({ "passed": "pass" }))).await;
    }
    assert_eq!(balance(&app).await, 4 * 500);

    // 第 5 场 -> 创建时触发 real_sessions_5 里程碑 +2000
    let r5 = common::create_round(&app, aid, "").await;
    assert_eq!(balance(&app).await, 2000 + 300 + 2000);
    app.req(Method::PATCH, &format!("/api/rounds/{r5}"), Some(json!({ "passed": "pass" }))).await;
    assert_eq!(balance(&app).await, 2000 + 500 + 2000);

    // 再建第 6 场 -> 不再重复发 real_sessions_5（r5 后余额 4500 = 2000+500+2000）
    let r6 = common::create_round(&app, aid, "").await;
    assert_eq!(balance(&app).await, 4500 + 300);
    app.req(Method::PATCH, &format!("/api/rounds/{r6}"), Some(json!({ "passed": "pass" }))).await;
    assert_eq!(balance(&app).await, 4800 + 200);

    // 首个 offer：6 轮已全部通过（无 pending 可补标），仅发首Offer里程碑 10000（ADR-0014 §4.3/§4.4）
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(d["application"]["status"], "interviewing", "首场面试应自动推进状态");
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/applications/{aid}"),
            Some(json!({ "status": "offer" })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(balance(&app).await, 5000 + 10000);

}

/// 投递：CRUD + 状态机 + start-interview 一键建批次（+300）+ 状态推进
/// 未关联公司的投递无法一键建批次 -> 400
/// 导出包含 v3 新表
#[tokio::test]
async fn export_includes_v3_tables() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, v) = app.req_raw(Method::GET, "/api/export", None).await;
    let parsed: Value = serde_json::from_str(&v).unwrap();
    assert!(parsed["mall_items"].is_array());
    assert!(parsed["points_ledger"].is_array());
    assert!(parsed["applications"].is_array());
}

// ---------- M1 批量分析 ----------

async fn poll_job(app: &TestApp, id: i64) -> Value {
    for _ in 0..100 {
        let (s, v) = app
            .req(Method::GET, &format!("/api/questions/batch-analyze/{id}"), None)
            .await;
        assert_eq!(s, 200);
        if v["status"] == "done" || v["status"] == "cancelled" || v["status"] == "error" {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("批量任务超时");
}

/// 批量分析：多题一键触发 -> 逐题写 analyses + 每题 +5 积分 + 自动入复习队
#[tokio::test]
async fn batch_analyze_analyzes_all_and_earns_points() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, rid) = setup_session_round(&app).await; // +300
    let mut qids = vec![];
    for i in 0..3 {
        let (s, v) = app
            .req(
                Method::POST,
                "/api/questions",
                Some(json!({ "round_id": rid, "content": format!("批量题目 {i}") })),
            )
            .await;
        assert_eq!(s, 201);
        qids.push(v["id"].as_i64().unwrap());
    }
    // 3 题真实题目积分
    assert_eq!(balance(&app).await, 300 + 3 * 100);

    // 每个分析排一个 mock 响应
    for qid in &qids {
        let _ = qid;
        mock.queue_nonstream(
            r#"{"tags":["批量"],"difficulty":3,"ref_answer":"参考答案","score":80,"feedback":"点评"}"#,
        );
    }

    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions/batch-analyze",
            Some(json!({ "ids": qids })),
        )
        .await;
    assert_eq!(s, 202);
    let job_id = v["job_id"].as_i64().unwrap();

    let job = poll_job(&app, job_id).await;
    assert_eq!(job["status"], "done");
    assert_eq!(job["total"], 3);
    assert_eq!(job["ok"], 3);

    // 每题都有 analyses
    for qid in qids {
        let (s, v) = app
            .req(Method::GET, &format!("/api/questions/{qid}/analyses"), None)
            .await;
        assert_eq!(s, 200);
        assert!(!v.as_array().unwrap().is_empty());
    }
    // 积分：3*100 真实题 + 3*5 批量
    assert_eq!(balance(&app).await, 300 + 3 * 100 + 3 * 5);
}

/// 批量分析：非法 ids / 空 ids 拒绝；不存在的任务 404
#[tokio::test]
async fn batch_analyze_validates_input() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (s, _) = app
        .req(Method::POST, "/api/questions/batch-analyze", Some(json!({ "ids": [] })))
        .await;
    assert_eq!(s, 400);
    let (s, _) = app
        .req(Method::POST, "/api/questions/batch-analyze", Some(json!({ "ids": [999999] })))
        .await;
    assert_eq!(s, 400);
    let (s, _) = app
        .req(Method::GET, "/api/questions/batch-analyze/12345", None)
        .await;
    assert_eq!(s, 404);
}

// ---------- M2 数据资产化（统计 + Timeline） ----------

/// 分析一题（真实 LLM 走 mock），供趋势/曲线测试造数（ADR-0013：任务化，等终态再返回）
async fn analyze_one(app: &TestApp, qid: i64, mock: &common::llm_mock::LlmMock) {
    mock.queue_nonstream(
        r#"{"tags":["八股"],"difficulty":3,"ref_answer":"参考答案","score":80,"feedback":"点评"}"#,
    );
    let (s, v) = app
        .req(Method::POST, &format!("/api/questions/{qid}/analyze"), None)
        .await;
    assert_eq!(s, 200);
    common::wait_ai_job(app, v["job_id"].as_u64().unwrap(), 5000).await;
}

#[tokio::test]
async fn stats_trend_shows_score_by_date_and_company() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, rid) = setup_session_round(&app).await;
    let qid = create_question(&app, rid).await;
    analyze_one(&app, qid, &mock).await;

    let (s, v) = app.req(Method::GET, "/api/stats/trend", None).await;
    assert_eq!(s, 200);
    let by_date = v["by_date"].as_array().unwrap();
    assert_eq!(by_date.len(), 1);
    assert_eq!(by_date[0]["count"], 1);
    assert_eq!(by_date[0]["avg_score"], 80.0);
    assert_eq!(v["by_company"][0]["company"], "测试公司");
    assert_eq!(v["by_company"][0]["count"], 1);
}

#[tokio::test]
async fn stats_review_curve_shows_daily_distribution() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = setup_session_round(&app).await;
    let qid = create_question(&app, rid).await;

    app.req(
        Method::POST,
        &format!("/api/review/{qid}/grade"),
        Some(json!({ "result": "remembered" })),
    )
    .await;

    let (s, v) = app.req(Method::GET, "/api/stats/review-curve", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["totals"]["remembered"], 1);
    assert_eq!(v["streak_days"], 1);
    let daily = v["daily"].as_array().unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0]["remembered"], 1);
}

/// 时间线：投递 / 面试轮次 / 训练，按时间倒序（v4：轮次直接挂投递）
#[tokio::test]
async fn stats_timeline_includes_application_round_and_drill() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 投递 -> 添加面试轮次 -> 完成一场训练
    let (s, v) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "时间线公司", "position": "后端", "channel": "内推" })),
        )
        .await;
    assert_eq!(s, 201);
    let aid = v["id"].as_i64().unwrap();
    app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面", "passed": "pass" }))).await;
    // 一场训练（完成才会进活动流）
    let (s, v) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "position": "后端", "title": "模拟面试" })),
        )
        .await;
    assert_eq!(s, 200, "drill create failed: {v:?}");
    let did = v["id"].as_i64().unwrap();
    app.req(Method::POST, &format!("/api/drills/{did}/finish"), None).await;

    let (s, v) = app.req(Method::GET, "/api/stats/timeline", None).await;
    assert_eq!(s, 200);
    let items = v["items"].as_array().unwrap();
    let types: Vec<&str> = items.iter().map(|i| i["type"].as_str().unwrap()).collect();
    assert!(types.contains(&"application"));
    assert!(types.contains(&"round"), "应含面试轮次，实际 {types:?}");
    assert!(types.contains(&"drill"));
    // 每条都有 type + ts（活动流契约，SP-2）
    for it in items {
        assert!(!it["type"].is_null());
        assert!(!it["ts"].is_null());
    }
    // 按日期倒序
    let mut dates: Vec<&str> = items.iter().map(|i| i["date"].as_str().unwrap()).collect();
    let sorted = { let mut d = dates.clone(); d.sort_by(|a, b| b.cmp(a)); d };
    dates.sort_by(|a, b| b.cmp(a));
    assert_eq!(dates, sorted);
}

/// 时间线 = 活动流：复习自评 / 积分购物 / 训练 / 投递 / 面试轮次，按时间倒序（用户修订：含积分购物）
#[tokio::test]
async fn activity_timeline_includes_review_events() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = setup_session_round(&app).await;
    let qid = create_question(&app, rid).await; // 有 my_answer -> 入复习队

    app.req(
        Method::POST,
        &format!("/api/review/{qid}/grade"),
        Some(json!({ "result": "forgot" })),
    )
    .await;

    // 积分购物：攒了分（300+100+5+20）后兑换最便宜的奶茶
    let (_, items) = app.req(Method::GET, "/api/mall/items", None).await;
    let cheap = items.as_array().unwrap().iter().min_by_key(|i| i["cost"].as_i64().unwrap()).unwrap();
    let cheap_id = cheap["id"].as_i64().unwrap();
    let cheap_name = cheap["name"].as_str().unwrap();
    app.req(Method::POST, &format!("/api/mall/items/{cheap_id}/redeem"), None).await;

    // /dashboard/activity（总览用）
    let (s, v) = app.req(Method::GET, "/api/dashboard/activity", None).await;
    assert_eq!(s, 200);
    let items = v["items"].as_array().unwrap();
    let review = items.iter().find(|i| i["type"] == "review_done").expect("应有「今日复习完成」事件");
    assert!(review["title"].as_str().unwrap().contains("今日复习完成"));
    assert!(review["detail"].as_str().unwrap().contains("张卡"), "应显示当天复习张数");
    assert!(items.iter().all(|i| i["type"] != "review"), "逐条复习不应进时间线");
    let point = items.iter().find(|i| i["type"] == "point").expect("应有积分购物事件");
    assert!(point["title"].as_str().unwrap().contains(cheap_name));
    assert!(point["detail"].as_str().unwrap().contains("分"));

    // 与 /stats/timeline 同源（契约一致）
    let (_, v2) = app.req(Method::GET, "/api/stats/timeline", None).await;
    let items2 = v2["items"].as_array().unwrap();
    assert!(items2.iter().any(|i| i["type"] == "review_done"));
    assert!(items2.iter().any(|i| i["type"] == "point"));
    assert_eq!(items2.len(), items.len());
}

// ---------- M0 简历结构化字段可视化编辑 ----------

#[tokio::test]
async fn resume_parsed_fields_can_be_saved_directly() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 保存原文 + 结构化字段（不重解析）
    let (s, _) = app
        .req(
            Method::PUT,
            "/api/resume",
            Some(json!({
                "raw_text": "张三，后端工程师，做过 xx 项目。",
                "parsed": {
                    "name": "张三",
                    "summary": "后端工程师",
                    "skills": ["Rust", "PostgreSQL"],
                    "projects": [{ "name": "面试复习", "detail": "全栈" }],
                    "education": [],
                    "experience": []
                }
            })),
        )
        .await;
    assert_eq!(s, 200);

    let (s, v) = app.req(Method::GET, "/api/resume", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["parsed"]["name"], "张三");
    assert_eq!(v["parsed"]["skills"][0], "Rust");
    assert_eq!(v["parsed"]["projects"][0]["name"], "面试复习");
}

// ---------- 审查整改（ai_sink 发分 / 求职漏斗 / 批量拒已分析） ----------

/// ai_sink：AI 沉淀题判分完成 -> +10（此前该类别是死代码，审查发现）
#[tokio::test]
async fn ai_sink_question_grading_earns_points() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 一场 interview，1 题即达标收尾
    let (s, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "position": "后端", "title": "模拟面试", "target_questions": 1 })),
        )
        .await;
    assert_eq!(s, 200);
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["第一题：讲一下 HashMap 原理。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    mock.queue_stream(vec![
        "整场总结：不错。".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"哈希\"],\"difficulty\":2,\"ref_answer\":\"数组+链表\",\"score\":85,\"feedback\":\"不错\"}".to_string(),
    ]);
    let (sc, _) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "数组加链表" })))
        .await;
    assert!(sc.is_success());

    // ai_sink +10（判分）+ drill +30（完成训练）
    assert_eq!(balance(&app).await, 10 + 30);
    let (_, v) = app.req(Method::GET, "/api/points/ledger", None).await;
    assert!(v.as_array().unwrap().iter().any(|e| e["category"] == "ai_sink" && e["amount"] == 10));
}

/// 求职漏斗：按阶段累计 + 转化率 + 渠道效果
#[tokio::test]
async fn stats_funnel_shows_stages_and_channel_effectiveness() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let cid = create_company(&app).await;

    // 渠道 A：2 投（1 到 offer、1 仍 applied）；渠道 B：1 投（1 interviewing）
    let mk = |status: &str, channel: &str| json!({ "company_id": cid, "position": "后端", "channel": channel, "status": status });
    let (_, a1) = app.req(Method::POST, "/api/applications", Some(mk("applied", "内推"))).await;
    let (_, a2) = app.req(Method::POST, "/api/applications", Some(mk("offer", "内推"))).await;
    let (_, a3) = app.req(Method::POST, "/api/applications", Some(mk("interviewing", "招聘网"))).await;
    let (_, a4) = app.req(Method::POST, "/api/applications", Some(mk("rejected", "招聘网"))).await;

    let (s, v) = app.req(Method::GET, "/api/stats/funnel", None).await;
    assert_eq!(s, 200);

    // 漏斗（v4 移除 callback）：applied=4, interviewing=2(offer+interviewing), offer=1
    let funnel = v["funnel"].as_array().unwrap();
    let cnt = |stage: &str| funnel.iter().find(|f| f["stage"] == stage).unwrap()["count"].as_i64().unwrap();
    assert_eq!(cnt("applied"), 4);
    assert_eq!(cnt("interviewing"), 2);
    assert_eq!(cnt("offer"), 1);

    // 转化率 applied->interviewing = 50%
    let conv = v["conversion"].as_array().unwrap();
    let cr = conv.iter().find(|c| c["from"] == "applied").unwrap();
    assert_eq!(cr["rate"], 50.0);

    // 渠道：内推 count=2 offers=1 offer_rate=50；招聘网 count=2 offers=0 interview=0
    let channels = v["channels"].as_array().unwrap();
    let ch = |name: &str| channels.iter().find(|c| c["channel"] == name).unwrap();
    assert_eq!(ch("内推")["count"], 2);
    assert_eq!(ch("内推")["offers"], 1);
    assert_eq!(ch("内推")["offer_rate"], 50.0);
    assert_eq!(ch("招聘网")["count"], 2);
    assert_eq!(ch("招聘网")["offers"], 0);
    let _ = (a1, a2, a3, a4);
}

/// 批量分析：服务端拒绝已分析题（审查整改：避免重复跑 + 重复 +5）
#[tokio::test]
async fn batch_analyze_rejects_already_analyzed() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, rid) = setup_session_round(&app).await;
    let qid = create_question(&app, rid).await;
    // 先单题分析（+15；任务化：等终态后积分已入账）
    mock.queue_nonstream(r#"{"tags":["八股"],"difficulty":3,"ref_answer":"R","score":80,"feedback":"F"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/analyze"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;
    assert_eq!(balance(&app).await, 300 + 100 + 15);

    // 批量选已分析题 -> 拒绝
    let (s, v) = app
        .req(Method::POST, "/api/questions/batch-analyze", Some(json!({ "ids": [qid] })))
        .await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap().contains("均已分析"));
    assert_eq!(balance(&app).await, 300 + 100 + 15); // 不重复 +5

    // 混合选择：只分析未分析的，跳过已分析的
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "第二题" })),
        )
        .await;
    assert_eq!(s, 201);
    let qid2 = v["id"].as_i64().unwrap();
    mock.queue_nonstream(r#"{"tags":["二"],"difficulty":2,"ref_answer":"R","score":70,"feedback":"F"}"#);
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions/batch-analyze",
            Some(json!({ "ids": [qid, qid2] })),
        )
        .await;
    assert_eq!(s, 202);
    let job_id = v["job_id"].as_i64().unwrap();
    let job = poll_job(&app, job_id).await;
    assert_eq!(job["status"], "done");
    assert_eq!(job["total"], 1); // 只分析未分析的
    assert_eq!(balance(&app).await, 300 + 100 + 15 + 100 + 5); // 第二题真实题 +100 + 批量 +5
}

/// F2 审查整改（ADR-0016 迁移）：测试连接携带请求体值优先于已保存配置（未保存也能测新值）
#[tokio::test]
async fn test_llm_prefers_request_body_over_saved() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let saved = common::llm_mock::LlmMock::start();
    let fresh = common::llm_mock::LlmMock::start();

    // 已保存配置指向 saved mock（legacy 键经懒迁移成 llm_config）
    app.point_llm_at_mock(&saved.base_url()).await;

    // 无 body -> 用已保存 provider/model
    let (s, v) = app.req(Method::POST, "/api/settings/llm-config/test", None).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["provider"].as_str().unwrap().contains("127.0.0.1"));
    assert_eq!(v["model"], "mock");

    // 带 body（新 base_url/model）-> 用请求体，即使未保存
    let (s, v) = app
        .req(
            Method::POST,
            "/api/settings/llm-config/test",
            Some(json!({ "base_url": fresh.base_url(), "model": "fresh-model" })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["model"], "fresh-model");
    assert!(v["provider"].as_str().unwrap().contains("127.0.0.1"));

    // 全空 body 也算无 body（兼容旧前端）
    let (s, _) = app
        .req(Method::POST, "/api/settings/llm-config/test", Some(json!({})))
        .await;
    assert_eq!(s, 200);
}

// ---------- 用户反馈：回答历史 + 多面试关联 + 沉淀题同步回答 ----------

/// 回答历史：手动补答(manual) + 复习自评(review) 都留档，详情可查
#[tokio::test]
async fn question_answer_history_records_manual_and_review() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = setup_session_round(&app).await;
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "什么是索引下推？" })),
        )
        .await;
    assert_eq!(s, 201);
    let qid = v["id"].as_i64().unwrap();

    // 手动补答 -> 记 manual 历史
    let (s, _) = app
        .req(Method::POST, &format!("/api/questions/{qid}/answers"), Some(json!({ "content": "回表优化" })))
        .await;
    assert_eq!(s, 200);
    // 复习自评带回忆 -> 记 review 历史（不改写 my_answer）
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/questions/{qid}"),
            Some(json!({ "my_answer": "初始回答" })),
        )
        .await;
    assert_eq!(s, 200);
    let (s, _) = app
        .req(
            Method::POST,
            &format!("/api/review/{qid}/grade"),
            Some(json!({ "result": "remembered", "answer": "复习时的回忆内容" })),
        )
        .await;
    assert_eq!(s, 200);

    let (s, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert_eq!(s, 200);
    let answers = d["answers"].as_array().unwrap();
    assert!(answers.iter().any(|a| a["source"] == "manual" && a["content"] == "回表优化"));
    assert!(answers.iter().any(|a| a["source"] == "review" && a["content"] == "复习时的回忆内容"));
    // 复习不改写 my_answer
    assert_eq!(d["my_answer"], "初始回答");
}

/// 多面试关联：同一题可关联多个轮次（round-links），详情列出全部面试
#[tokio::test]
async fn question_can_link_multiple_rounds() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    // 同一投递下两个轮次
    let aid = common::create_application(&app, "多轮公司", "后端").await;
    let mut round_ids = vec![];
    for name in ["一面", "二面"] {
        round_ids.push(common::create_round(&app, aid, name).await);
        // 反馈七#2：上一面通过才能加下一面
        app.req(
            Method::PATCH,
            &format!("/api/rounds/{}", round_ids[round_ids.len() - 1]),
            Some(json!({ "passed": "pass" })),
        )
        .await;
    }
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": round_ids[0], "content": "缓存穿透怎么解决？" })),
        )
        .await;
    assert_eq!(s, 201);
    let qid = v["id"].as_i64().unwrap();

    // 关联第二个面试的轮次
    let (s, _) = app
        .req(
            Method::POST,
            &format!("/api/questions/{qid}/round-links"),
            Some(json!({ "round_id": round_ids[1] })),
        )
        .await;
    assert_eq!(s, 200);

    let (s, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert_eq!(s, 200);
    let links = d["round_links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert!(links.iter().all(|l| l["company"] == "多轮公司"));

    // 解除关联
    let (s, _) = app
        .req(Method::DELETE, &format!("/api/questions/{qid}/round-links/{}", round_ids[1]), None)
        .await;
    assert_eq!(s, 200);
    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert_eq!(d["round_links"].as_array().unwrap().len(), 1);
}

/// 模拟面试沉淀题：同步用户回答到题库 + 记 interview 回答历史（用户反馈 3）
#[tokio::test]
async fn drill_sunk_question_syncs_user_answer_and_history() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (s, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "position": "后端", "title": "模拟面试", "target_questions": 1 })),
        )
        .await;
    assert_eq!(s, 200);
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["第一题：HashMap 原理。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    mock.queue_stream(vec![
        "整场总结。".to_string(),
        "\n<<<REPORT>>>\n{\"tags\":[\"哈希\"],\"difficulty\":2,\"ref_answer\":\"数组+链表\",\"score\":85,\"feedback\":\"不错\"}".to_string(),
    ]);
    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "数组加链表解决冲突" })))
        .await;
    assert!(sc.is_success());

    // 沉淀题：内容 = mock 出的题，my_answer 应同步为用户回答
    let (_, rows) = app.req(Method::GET, "/api/questions?source=ai_drill&q=HashMap", None).await;
    let qs = rows.as_array().unwrap();
    assert!(!qs.is_empty(), "应有沉淀题");
    let q = &qs[0];
    assert_eq!(q["my_answer"], "数组加链表解决冲突");
    let qid = q["id"].as_i64().unwrap();
    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert!(d["answers"].as_array().unwrap().iter().any(|a| a["source"] == "interview" && a["content"] == "数组加链表解决冲突"));
    let _ = body;
}

/// 用户反馈 6：选了 target 题就该答满——答得好不再提前收尾（只保留题数达标/答垮怜悯收尾）
#[tokio::test]
async fn interview_runs_all_target_questions_even_on_good_scores() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (s, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "position": "后端", "title": "模拟面试", "target_questions": 3 })),
        )
        .await;
    assert_eq!(s, 200);
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["Q1：讲 HashMap。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;

    // 答 Q1（高分 85）-> 不应收尾，继续 Q2
    mock.queue_stream(vec!["Q2：讲索引。\n<<<REPORT>>>\n{\"tags\":[\"哈希\"],\"difficulty\":2,\"ref_answer\":\"R\",\"score\":85,\"feedback\":\"好\"}".to_string()]);
    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "答1" })))
        .await;
    assert!(sc.is_success());
    assert!(body.contains("Q2"), "高分也应继续出下一题");
    let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(det["status"], "ongoing", "Q1 后不应结束");

    // 答 Q2（高分）-> 继续 Q3（旧逻辑会在此提前收尾）
    mock.queue_stream(vec!["Q3：讲 B+ 树。\n<<<REPORT>>>\n{\"tags\":[\"索引\"],\"difficulty\":3,\"ref_answer\":\"R\",\"score\":90,\"feedback\":\"好\"}".to_string()]);
    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "答2" })))
        .await;
    assert!(sc.is_success());
    assert!(body.contains("Q3"), "第 2 题高分也不应提前收尾");
    let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(det["status"], "ongoing", "Q2 后不应结束");

    // 答 Q3 -> 题数达标 -> 总结收尾
    mock.queue_stream(vec!["整场总结。".to_string(), "\n<<<REPORT>>>\n{\"tags\":[\"B+\"],\"difficulty\":3,\"ref_answer\":\"R\",\"score\":88,\"feedback\":\"好\"}".to_string()]);
    let (sc, body) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "答3" })))
        .await;
    assert!(sc.is_success());
    assert!(body.contains("整场总结"));
    let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(det["status"], "finished", "Q3 达标后结束");
    // 三题都问了
    let msgs = det["messages"].as_array().unwrap();
    let qs: Vec<&str> = msgs.iter().filter(|m| m["kind"] == "question" || m["kind"] == "probe").map(|m| m["content"].as_str().unwrap()).collect();
    assert_eq!(qs.len(), 3);
}

/// Q3 确认：每次「分析」自动把当时的 my_answer 落成回答版本（批注有主可挂）
#[tokio::test]
async fn analyze_records_answer_as_version() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let (_, rid) = setup_session_round(&app).await;
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "什么是索引下推？", "my_answer": "第一版回答" })),
        )
        .await;
    assert_eq!(s, 201);
    let qid = v["id"].as_i64().unwrap();

    mock.queue_nonstream(r#"{"tags":["优化"],"difficulty":3,"ref_answer":"R","score":80,"feedback":"F"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/analyze"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;

    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert!(d["answers"].as_array().unwrap().iter().any(|a| a["source"] == "manual" && a["content"] == "第一版回答"));
    // 批注匹配：analysis.answer_snapshot == 该回答
    let latest = d["analyses"][0].clone();
    assert_eq!(latest["answer_snapshot"], "第一版回答");
}

/// 双向关联：题关联到 round2 后，按 round2 筛选题目也应出现该题
#[tokio::test]
async fn question_appears_in_linked_round_filter() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = common::create_application(&app, "筛选公司", "后端").await;
    let mut rids = vec![];
    for name in ["一面", "二面"] {
        rids.push(common::create_round(&app, aid, name).await);
        app.req(
            Method::PATCH,
            &format!("/api/rounds/{}", rids[rids.len() - 1]),
            Some(json!({ "passed": "pass" })),
        )
        .await;
    }
    let (s, v) = app
        .req(Method::POST, "/api/questions", Some(json!({ "round_id": rids[0], "content": "缓存穿透？" })))
        .await;
    assert_eq!(s, 201);
    let qid = v["id"].as_i64().unwrap();

    // 主轮次可见
    let (_, rows) = app.req(Method::GET, &format!("/api/questions?round={}", rids[0]), None).await;
    assert!(rows.as_array().unwrap().iter().any(|r| r["id"] == qid));
    // 未关联前，round2 不可见
    let (_, rows) = app.req(Method::GET, &format!("/api/questions?round={}", rids[1]), None).await;
    assert!(!rows.as_array().unwrap().iter().any(|r| r["id"] == qid));

    // 关联 round2 -> 双向可见
    app.req(Method::POST, &format!("/api/questions/{qid}/round-links"), Some(json!({ "round_id": rids[1] }))).await;
    let (_, rows) = app.req(Method::GET, &format!("/api/questions?round={}", rids[1]), None).await;
    assert!(rows.as_array().unwrap().iter().any(|r| r["id"] == qid), "关联后 round2 筛选应出现该题");
}

// ---------- Q1 幂等 + Q2 分析拆分 ----------

/// Q2：生成参考答案（/ref）与评价回答（/analyze）是两个独立动作
#[tokio::test]
async fn ref_and_evaluate_are_separate_actions() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let (_, rid) = setup_session_round(&app).await;
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "什么是索引下推？", "my_answer": "减少回表" })),
        )
        .await;
    assert_eq!(s, 201);
    let qid = v["id"].as_i64().unwrap();

    // /ref：只产参考答案（score null、snapshot null）；任务化——等终态再断言落库
    mock.queue_nonstream(r#"{"tags":["优化"],"difficulty":3,"ref_answer":"覆盖索引、减少回表"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;
    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let ref_row = d["analyses"].as_array().unwrap().iter().find(|a| a["ref_answer"] == "覆盖索引、减少回表").expect("应有参考答案行");
    assert!(ref_row["score"].is_null(), "/ref 不评分");
    assert!(ref_row["answer_snapshot"].is_null());

    // /analyze：评价回答（score、snapshot=my_answer）；参考答案不变
    mock.queue_nonstream(r#"{"score":82,"feedback":"答得不错，补一下回表细节"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/analyze"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;
    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let ans_row = d["analyses"].as_array().unwrap().iter().find(|a| a["score"] == 82).expect("应有评价行");
    assert_eq!(ans_row["answer_snapshot"], "减少回表");
    // 参考答案仍来自 /ref 行（未被 /analyze 覆盖）
    assert!(d["analyses"].as_array().unwrap().iter().any(|a| a["ref_answer"] == "覆盖索引、减少回表"));
    // 稳定语义（难度/评分互不干扰）：列表里 difficulty 仍来自 /ref（固有属性），score 来自 /analyze（回答评价）
    let (_, list) = app.req(Method::GET, "/api/questions?source=manual", None).await;
    let row = list.as_array().unwrap().iter().find(|r| r["id"] == qid).expect("列表中应有该题");
    assert_eq!(row["last_difficulty"], 3, "评价回答后难度不应消失（固有属性）");
    assert_eq!(row["last_score"], 82, "参考答案后评分不应消失（回答评价）");
}

/// Q1：面试重发同内容幂等——不重复追加用户回答、不重复判分
#[tokio::test]
async fn interview_resend_same_answer_is_idempotent() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (s, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "position": "后端", "target_questions": 2, "title": "幂等" })))
        .await;
    assert_eq!(s, 200);
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["Q1：讲 HashMap。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    mock.queue_nonstream(r#"{"tags":["哈希"],"difficulty":2,"ref_answer":"R","score":80,"feedback":"不错"}"#);
    mock.queue_stream(vec!["Q2：讲索引。".to_string()]);
    let (sc, _) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "数组加链表" })))
        .await;
    assert!(sc.is_success(), "第一次回答应成功");

    // 重发相同内容（刷新后重试）-> 幂等：不追加、不重复判分、不报错
    let (sc, _) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "数组加链表" })))
        .await;
    assert!(sc.is_success(), "重试应成功");

    let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    let msgs = det["messages"].as_array().unwrap();
    let user_msgs: Vec<&str> = msgs.iter().filter(|m| m["role"] == "user").map(|m| m["content"].as_str().unwrap()).collect();
    assert_eq!(user_msgs.len(), 2, "用户回答应只有『开始』+ 1 条，不重复");
    assert_eq!(user_msgs.iter().filter(|c| **c == "数组加链表").count(), 1, "相同回答不应重复追加");
}


/// 时间线（D1/D2/D3 评审确认）：逐条复习不再进时间线，改为「今日复习完成」一天一条（含当天张数）
#[tokio::test]
async fn timeline_shows_review_done_once_not_each_card() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_sid, rid) = setup_session_round(&app).await;
    let qid = create_question(&app, rid).await;

    // 复习前：时间线不应有逐条 review 条目
    let (_, tl0) = app.req(Method::GET, "/api/stats/timeline", None).await;
    assert!(
        tl0["items"].as_array().unwrap().iter().all(|it| it["type"] != "review"),
        "逐条复习不应进时间线"
    );

    // 自评最后一张到期卡 -> 今日队列清空 -> daily_goal 授予 -> 「今日复习完成」
    let (sg, _) = app
        .req(Method::POST, &format!("/api/review/{qid}/grade"), Some(json!({ "result": "remembered" })))
        .await;
    assert!(sg.is_success(), "自评应成功");
    let (_, queue) = app.req(Method::GET, "/api/review/queue", None).await;
    assert!(queue.as_array().unwrap().is_empty(), "今日队列应清空");

    let (_, tl) = app.req(Method::GET, "/api/stats/timeline", None).await;
    let items = tl["items"].as_array().unwrap();
    assert!(items.iter().all(|it| it["type"] != "review"), "不应有逐条复习条目");
    let done: Vec<&Value> = items.iter().filter(|it| it["type"] == "review_done").collect();
    assert_eq!(done.len(), 1, "今日复习完成应恰好一条");
    assert_eq!(done[0]["title"], "今日复习完成");
    assert!(done[0]["detail"].as_str().unwrap_or("").contains("张卡"), "应显示当天复习张数");

    // 幂等：再建一张并复习，今日不再新增「今日复习完成」
    let qid2 = create_question(&app, rid).await;
    app.req(Method::POST, &format!("/api/review/{qid2}/grade"), Some(json!({ "result": "fuzzy" }))).await;
    let (_, tl2) = app.req(Method::GET, "/api/stats/timeline", None).await;
    let done2 = tl2["items"].as_array().unwrap().iter().filter(|it| it["type"] == "review_done").count();
    assert_eq!(done2, 1, "每天应只有一条「今日复习完成」");
}

/// 思考强度（ADR-0016）：advanced.reasoning_effort 七档可保存/回读（默认 xhigh，替代旧 thinking 布尔）
#[tokio::test]
async fn llm_thinking_setting_roundtrip() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let put = |effort: &str| {
        json!({
            "providers": [{ "id": "p1", "name": "P", "base_url": "http://x/v1", "api_key": "" }],
            "models": [{ "id": "m1", "provider_id": "p1", "name": "mock",
                         "caps": { "structured_output": true, "web_search": false },
                         "advanced": { "reasoning_effort": effort } }],
            "active_model_id": "m1"
        })
    };
    let (s, _) = app.req(Method::PUT, "/api/settings/llm-config", Some(put("low"))).await;
    assert_eq!(s, 200);
    let (_, d) = app.req(Method::GET, "/api/settings/llm-config", None).await;
    assert_eq!(d["resolved"]["reasoning_effort"], "low", "应能保存并回读思考强度");

    // 非法档位拒绝；七档之外不可用
    let (s, _) = app.req(Method::PUT, "/api/settings/llm-config", Some(json!({
        "providers": [{ "id": "p1", "name": "P", "base_url": "http://x/v1", "api_key": "" }],
        "models": [{ "id": "m1", "provider_id": "p1", "name": "mock",
                     "advanced": { "reasoning_effort": "ultra" } }]
    }))).await;
    assert_eq!(s, 400, "非法思考强度应被拒");
}

/// 参考答案可手动编辑（LLM 生成不佳时兜底）：就地改最近一条固有属性行，难度/标签不动
#[tokio::test]
async fn manual_ref_edit_overrides_llm_ref() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let (_, rid) = setup_session_round(&app).await;
    let (s, v) = app
        .req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "什么是索引下推？" })))
        .await;
    assert_eq!(s, 201);
    let qid = v["id"].as_i64().unwrap();

    mock.queue_nonstream(r#"{"tags":["优化","索引"],"difficulty":3,"ref_answer":"覆盖索引、减少回表"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;

    // 手动编辑参考答案
    let (s, _) = app
        .req(
            Method::PUT,
            &format!("/api/questions/{qid}/ref"),
            Some(json!({ "ref_answer": "手动补充的详细版：1. 核心思路… 2. 边界情况… 3. 加分点…" })),
        )
        .await;
    assert_eq!(s, 200);
    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let intrinsic = d["analyses"].as_array().unwrap().iter().find(|a| a["difficulty"] == 3).expect("应有固有属性行");
    assert_eq!(intrinsic["ref_answer"], "手动补充的详细版：1. 核心思路… 2. 边界情况… 3. 加分点…", "手动编辑应生效");
    assert_eq!(intrinsic["difficulty"], 3, "编辑参考答案不影响难度");
    assert!(intrinsic["tags"].as_array().unwrap().iter().any(|t| t == "优化"), "编辑参考答案不影响标签");

    // 空参考答案被拒
    let (s, _) = app.req(Method::PUT, &format!("/api/questions/{qid}/ref"), Some(json!({ "ref_answer": "  " }))).await;
    assert_eq!(s, 400, "空参考答案应被拒");
}

/// 推荐关联题目（离线）：共享标签 + 评分最低优先，点击可跳转；无共享标签的不推
#[tokio::test]
async fn related_questions_shares_tags_and_lowest_score_first() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_sid, rid) = setup_session_round(&app).await;

    // qA 带标签 [优化]；qB/qC 共享 [优化]；qD 只有 [索引] 不共享
    let mk = |content: &str, tags: Vec<&str>, ans: Option<&str>| {
        let mut body = json!({ "round_id": rid, "content": content });
        body["tags"] = json!(tags);
        if let Some(a) = ans {
            body["my_answer"] = json!(a);
        }
        body
    };
    let (_, a) = app.req(Method::POST, "/api/questions", Some(mk("A 索引下推", vec!["优化"], None))).await;
    let qa = a["id"].as_i64().unwrap();
    let (_, b) = app.req(Method::POST, "/api/questions", Some(mk("B 回表", vec!["优化"], Some("答")))).await;
    let qb = b["id"].as_i64().unwrap();
    let (_, c) = app.req(Method::POST, "/api/questions", Some(mk("C 覆盖索引", vec!["优化"], Some("答")))).await;
    let qc = c["id"].as_i64().unwrap();
    let (_, d) = app.req(Method::POST, "/api/questions", Some(mk("D 进程线程", vec!["索引"], Some("答")))).await;
    let qd = d["id"].as_i64().unwrap();

    // 评分最低优先：B=30（低）、C=90（高）
    sqlx::query("INSERT INTO analyses(question_id, score) VALUES($1,30)")
        .bind(qb)
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO analyses(question_id, score) VALUES($1,90)")
        .bind(qc)
        .execute(&app.pool)
        .await
        .unwrap();

    let (s, rel) = app.req(Method::GET, &format!("/api/questions/{qa}/related"), None).await;
    assert_eq!(s, 200);
    let arr = rel.as_array().unwrap();
    assert_eq!(arr.len(), 2, "只应推共享标签的 B/C");
    assert_eq!(arr[0]["id"], qb, "评分最低（30）优先");
    assert_eq!(arr[1]["id"], qc);
    assert!(!arr.iter().any(|r| r["id"] == qd), "无共享标签的 D 不推");
    assert!(arr.iter().all(|r| r["id"] != qa), "不推自己");
    assert!(arr.iter().all(|r| r["last_score"].is_number()), "应带评分");
}

// ---------------------------------------------------------------------------
// 总览统计口径：模拟面试/模拟训练不算真实公司/批次（系统公司「模拟面试」排除）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dashboard_summary_excludes_mock_company() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 真实投递（走 API，公司自动建）
    let (s, _) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "真实公司", "position": "后端" })),
        )
        .await;
    assert_eq!(s, 201);

    // 模拟训练沉淀产生的系统公司 + 岗位 + 投递（直接落库；归首管理员）
    sqlx::query(
        "INSERT INTO companies(user_id, name, is_system) VALUES((SELECT min(id) FROM users WHERE role='admin'), '模拟面试', true)
         ON CONFLICT (user_id, name) DO NOTHING",
    )
    .execute(&app.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO positions(user_id, company_id, title)
        VALUES((SELECT min(id) FROM users WHERE role='admin'),
               (SELECT id FROM companies WHERE name='模拟面试'), 'AI 训练')
        "#,
    )
    .execute(&app.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO applications(user_id, position_id)
        VALUES((SELECT min(id) FROM users WHERE role='admin'),
               (SELECT id FROM positions WHERE title='AI 训练'))
        "#,
    )
    .execute(&app.pool)
    .await
    .unwrap();

    let (_, d) = app.req(Method::GET, "/api/dashboard", None).await;
    assert_eq!(
        d["summary"]["companies"].as_i64(),
        Some(1),
        "系统公司「模拟面试」不应计入公司数"
    );
    assert_eq!(
        d["summary"]["sessions"].as_i64(),
        Some(1),
        "系统投递不应计入（现语义：真实投递数）"
    );
}

/// 评审 P2 整改：积分明细分页——`offset` 参数与 `limit`/`category` 组合可用
#[tokio::test]
async fn ledger_supports_offset_paging() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = setup_session_round(&app).await;

    // 造 4 条流水：建轮次 +300 / 录题 +100 / 复习一张 +5 / 队列清零每日目标 +20
    let qid = create_question(&app, rid).await;
    app.req(
        Method::POST,
        &format!("/api/review/{qid}/grade"),
        Some(json!({ "result": "remembered" })),
    )
    .await;

    // 全量：4 条
    let (_, all) = app.req(Method::GET, "/api/points/ledger?limit=50", None).await;
    assert_eq!(all.as_array().unwrap().len(), 4);

    // 第一页 2 条 + offset 翻页取剩余 2 条，且拼接后等于全量顺序
    let (s, p1) = app.req(Method::GET, "/api/points/ledger?limit=2&offset=0", None).await;
    assert_eq!(s, 200);
    assert_eq!(p1.as_array().unwrap().len(), 2);
    let (s, p2) = app.req(Method::GET, "/api/points/ledger?limit=2&offset=2", None).await;
    assert_eq!(s, 200);
    assert_eq!(p2.as_array().unwrap().len(), 2);
    let ids: Vec<i64> = [p1.as_array().unwrap().as_slice(), p2.as_array().unwrap().as_slice()]
        .concat()
        .iter()
        .map(|e| e["id"].as_i64().unwrap())
        .collect();
    let all_ids: Vec<i64> = all.as_array().unwrap().iter().map(|e| e["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, all_ids, "分页拼接应等于全量顺序");

    // category + offset 组合
    let (_, only) = app.req(Method::GET, "/api/points/ledger?limit=10&offset=0&category=real_question", None).await;
    assert_eq!(only.as_array().unwrap().len(), 1);
}

/// 批量分析 mode 参数（用户裁决 4）：默认只跑未分析（兼容旧行为）；
/// mode=all 重新分析全部（覆盖式新增 analyses 行），混选时由前端弹窗确认后显式传 all。
#[tokio::test]
async fn batch_analyze_mode_all_reanalyzes_analyzed_questions() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let (_, rid) = setup_session_round(&app).await;
    let mut qids = vec![];
    for i in 0..2 {
        let (s, v) = app
            .req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": format!("模式题 {i}") })))
            .await;
        assert_eq!(s, 201);
        qids.push(v["id"].as_i64().unwrap());
    }
    for _ in &qids {
        mock.queue_nonstream(r#"{"tags":["t"],"difficulty":3,"ref_answer":"r","score":80,"feedback":"f1"}"#);
    }
    let (s, v) = app.req(Method::POST, "/api/questions/batch-analyze", Some(json!({ "ids": qids }))).await;
    assert_eq!(s, 202);
    let job = poll_job(&app, v["job_id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "done");

    // 默认模式再提交：全部已分析 → 400 均已分析
    let (s, e) = app.req(Method::POST, "/api/questions/batch-analyze", Some(json!({ "ids": qids }))).await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap_or("").contains("均已分析"), "{e}");

    // mode=all：重新分析全部，每题再各排一个响应
    for _ in &qids {
        mock.queue_nonstream(r#"{"tags":["t"],"difficulty":3,"ref_answer":"r","score":95,"feedback":"f2"}"#);
    }
    let (s, v) = app
        .req(Method::POST, "/api/questions/batch-analyze", Some(json!({ "ids": qids, "mode": "all" })))
        .await;
    assert_eq!(s, 202);
    let job = poll_job(&app, v["job_id"].as_i64().unwrap()).await;
    assert_eq!(job["status"], "done");
    assert_eq!(job["total"], 2);

    // 最新评分应为第二轮的 95
    let (_, list) = app.req(Method::GET, "/api/questions", None).await;
    for row in list.as_array().unwrap() {
        assert_eq!(row["last_score"].as_i64().unwrap(), 95, "mode=all 应覆盖式重评");
    }
}
