//! v4 M2 IA 重组（ADR-0011）：求职台周投递目标（GET/PUT /api/stats/goal）。
//! 目标存 per-user settings（key=weekly_application_goal）；进度 = 本周一以来的投递数。

mod common;

use axum::http::{Method, StatusCode};
use common::TestApp;
use serde_json::{json, Value};

#[tokio::test]
async fn weekly_goal_roundtrip_and_progress() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 默认目标为 0（未设置）
    let (s, v) = app.req(Method::GET, "/api/stats/goal", None).await;
    assert_eq!(s, 200);
    assert_eq!(v["weekly_target"].as_i64(), Some(0));
    assert_eq!(v["applied_this_week"].as_i64(), Some(0));

    // 投递两份（本周）
    let (_, c) = app
        .req(Method::POST, "/api/companies", Some(json!({ "name": "目标公司" })))
        .await;
    let cid = c["id"].as_i64().unwrap();
    for pos in ["后端A", "后端B"] {
        let (s, _) = app
            .req(
                Method::POST,
                "/api/applications",
                Some(json!({ "company_id": cid, "position": pos })),
            )
            .await;
        assert!(s.is_success());
    }

    // 未设目标也有进度
    let (_, v) = app.req(Method::GET, "/api/stats/goal", None).await;
    assert_eq!(v["applied_this_week"].as_i64(), Some(2));

    // 设定目标 -> 回读
    let (s, _) = app
        .req(Method::PUT, "/api/stats/goal", Some(json!({ "weekly_target": 10 })))
        .await;
    assert_eq!(s, 200);
    let (_, v) = app.req(Method::GET, "/api/stats/goal", None).await;
    assert_eq!(v["weekly_target"].as_i64(), Some(10));
    assert_eq!(v["applied_this_week"].as_i64(), Some(2));

    // 非法目标拒绝
    let (s, _) = app
        .req(Method::PUT, "/api/stats/goal", Some(json!({ "weekly_target": -1 })))
        .await;
    assert_eq!(s, 400);

    // per-user：B 的目标是自己的（0），进度也是自己的（0）
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "carol", "password": "carolpass1" })),
        )
        .await;
    assert_eq!(s, 201);
    let (s, carol) = app.login_as("carol", "carolpass1").await;
    assert_eq!(s, 200);
    let carol = carol.unwrap();
    let (_, v) = app.req_as(&carol, Method::GET, "/api/stats/goal", None).await;
    assert_eq!(v["weekly_target"].as_i64(), Some(0), "B 的目标独立");
    assert_eq!(v["applied_this_week"].as_i64(), Some(0), "B 的进度独立");
}

#[tokio::test]
async fn applications_list_supports_note_todo() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 创建投递带备注（待跟进事项与备注已合并为一个字段，Q4）
    let (_, c) = app
        .req(Method::POST, "/api/companies", Some(json!({ "name": "待办公司" })))
        .await;
    let cid = c["id"].as_i64().unwrap();
    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_id": cid, "position": "后端", "note": "周三跟进 HR" })),
        )
        .await;
    assert!(s.is_success());
    let aid = a["id"].as_i64().unwrap();

    // 列表应带出 note（求职台今日待办数据源）
    let (_, list) = app.req(Method::GET, "/api/applications", None).await;
    let found = list.as_array().unwrap().iter().find(|x| x["id"].as_i64() == Some(aid));
    assert!(found.is_some());
    assert_eq!(found.unwrap()["note"], "周三跟进 HR");
}

// ---------- M3 投递枢纽：事件流水 / JD 字段 / 详情聚合 / 轮次回写提示 ----------

/// 建一份投递并返回 id
async fn mk_application(app: &TestApp, name: &str) -> i64 {
    let (_, c) = app
        .req(Method::POST, "/api/companies", Some(json!({ "name": name })))
        .await;
    let cid = c["id"].as_i64().unwrap();
    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_id": cid, "position": "后端", "next_action": "准备笔试" })),
        )
        .await;
    assert!(s.is_success());
    a["id"].as_i64().unwrap()
}

#[tokio::test]
async fn application_status_changes_record_events() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = mk_application(&app, "流水公司").await;

    // ADR-0014 §3.2：进行中由添加首场面试自动推进；手工 PATCH 制造 interviewing 被拒
    let (s, e) = app
        .req(
            Method::PATCH,
            &format!("/api/applications/{aid}"),
            Some(json!({ "status": "interviewing" })),
        )
        .await;
    assert_eq!(s, 400, "手工制造进行中应被拒");
    assert!(e["error"].as_str().unwrap().contains("自动推进"));

    // 唯一合法触发：添加首场面试 → 自动流转（source=auto）
    let (s, _) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    assert_eq!(s, 201);

    // 非法流转：interviewing 回退 applied -> 400（forward-only）
    let (s, e) = app
        .req(
            Method::PATCH,
            &format!("/api/applications/{aid}"),
            Some(json!({ "status": "applied" })),
        )
        .await;
    assert_eq!(s, 400, "状态只进不退");
    assert!(e["error"].as_str().unwrap().contains("只进不退"));

    // 详情聚合：创建 1 条 + 自动流转 1 条
    let (s, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(s, 200);
    assert_eq!(d["application"]["id"].as_i64(), Some(aid));
    let events = d["events"].as_array().unwrap();
    // 倒序最新在上：添加面试 → 自动推进（因果：先推进再记轮次）→ 创建投递
    assert_eq!(events.len(), 3, "应有 创建+自动流转+添加面试 共 3 条事件");
    assert_eq!(events[0]["kind"], "round");
    assert_eq!(events[0]["note"].as_str().unwrap().contains("添加面试"), true);
    assert_eq!(events[1]["source"], "auto");
    assert_eq!(events[1]["from_status"], "applied");
    assert_eq!(events[1]["to_status"], "interviewing");
    assert_eq!(events[2]["to_status"], "applied");
}

#[tokio::test]
async fn application_detail_aggregates_rounds_and_status_events() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = mk_application(&app, "关联公司").await;

    // 添加面试：首场自动推进 applied→interviewing（ADR-0012 D3，source=auto）
    let (s, r1) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" }))).await;
    assert_eq!(s, 201);
    let _ = r1;

    // 详情聚合：rounds + events（含自动推进事件）
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    let rounds = d["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0]["name"], "一面");
    assert_eq!(d["application"]["status"], "interviewing", "首场面试应自动推进状态");
    let events = d["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["to_status"] == "interviewing" && e["source"] == "auto"));
}

#[tokio::test]
async fn jd_text_roundtrip() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    // ADR-0012：JD 属于岗位；创建投递时携带 JD 写入岗位
    let (_, c) = app.req(Method::POST, "/api/companies", Some(json!({ "name": "JD 公司" }))).await;
    let cid = c["id"].as_i64().unwrap();
    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_id": cid, "position": "后端", "jd_text": "负责高并发网关设计，要求熟悉 Rust/K8s…" })),
        )
        .await;
    assert_eq!(s, 201);
    let aid = a["id"].as_i64().unwrap();

    // 投递详情带出 position_id；岗位详情可读回 JD
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    let pid = d["application"]["position_id"].as_i64().expect("应返回 position_id");
    let (st, p) = app.req(Method::GET, &format!("/api/positions/{pid}"), None).await;
    assert_eq!(st, 200);
    assert!(p["jd_text"].as_str().unwrap_or("").contains("Rust"), "JD 应存在岗位并可读回");

    // 岗位 PATCH 更新 JD + 地点
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/positions/{pid}"),
            Some(json!({ "jd_text": "更新后的 JD：重 Rust 异步", "location": "杭州" })),
        )
        .await;
    assert_eq!(s, 200);
    let (_, p) = app.req(Method::GET, &format!("/api/positions/{pid}"), None).await;
    assert!(p["jd_text"].as_str().unwrap().contains("异步"));
    assert_eq!(p["location"], "杭州");
}

#[tokio::test]
async fn round_pass_suggests_application_advance() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = mk_application(&app, "提示公司").await;

    // ADR-0012 D3 修正（反馈 #5）：单轮通过 ≠ offer。轮次通过不再给任何提示；
    // 前端内联提供「进入下一面 / 这是最终面·标记 Offer」二选一。
    let (_, r) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" }))).await;
    let rid = r["id"].as_i64().unwrap();
    let (s, resp) = app
        .req(Method::PATCH, &format!("/api/rounds/{rid}"), Some(json!({ "passed": "pass" })))
        .await;
    assert_eq!(s, 200);
    assert!(resp.get("application_hint").is_none(), "通过不应再建议 offer");

    // 未通过 -> 仍建议标记 rejected（确认流保留）
    let (_, r2) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "二面" }))).await;
    let rid2 = r2["id"].as_i64().unwrap();
    let (s, resp2) = app
        .req(Method::PATCH, &format!("/api/rounds/{rid2}"), Some(json!({ "passed": "fail" })))
        .await;
    assert_eq!(s, 200);
    let hint = &resp2["application_hint"];
    assert_eq!(hint["application_id"].as_i64(), Some(aid));
    assert_eq!(hint["suggested_status"], "rejected", "未过应建议未通过");

    // 模拟用户接受：标记未通过（终态）
    app.req(Method::PATCH, &format!("/api/applications/{aid}"), Some(json!({ "status": "rejected" }))).await;
}

/// B组 #4：轮次结果选定后不可变更——非 pending 再改其它值 400，同值幂等放行。
#[tokio::test]
async fn round_result_locks_after_selection() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = mk_application(&app, "锁定公司").await;
    let (_, r) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" }))).await;
    let rid = r["id"].as_i64().unwrap();

    // 待定 -> 通过：允许
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/rounds/{rid}"), Some(json!({ "passed": "pass" })))
        .await;
    assert_eq!(s, 200);

    // 已选定后再改 fail：400 锁定（前端快捷按钮 + 后端硬锁双层）
    let (s, e) = app
        .req(Method::PATCH, &format!("/api/rounds/{rid}"), Some(json!({ "passed": "fail" })))
        .await;
    assert_eq!(s, 400, "已选定的轮次结果不可变更");
    assert!(e["error"].as_str().unwrap_or("").contains("不可变更"), "错误文案应说明锁定: {e}");

    // 同值重复 PATCH 幂等放行（网络重试不炸）
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/rounds/{rid}"), Some(json!({ "passed": "pass" })))
        .await;
    assert_eq!(s, 200, "同值幂等应放行");
}

/// 反馈 #6：终态投递不能再添加面试（后端守卫 + 前端隐藏）
#[tokio::test]
async fn cannot_add_round_to_terminal_application() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = mk_application(&app, "终态公司").await;

    // 先进入面试中（首场自动推进），再标记 offer
    let (s, _) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" })))
        .await;
    assert_eq!(s, 201);
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{aid}"), Some(json!({ "status": "offer" })))
        .await;
    assert_eq!(s, 200);

    let (s, e) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "加面" })))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "终态不应能添加面试");
    assert!(e["error"].as_str().unwrap().contains("终态"));
}

// ---------- M4：JD 驱动 AI + 复盘报告 ----------

use common::llm_mock::LlmMock;

/// 建第二个用户（走管理员 API）并登录，返回会话 cookie
async fn create_and_login_second_user(app: &TestApp) -> (StatusCode, Option<String>) {
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "bob", "password": "bobpass123" })),
        )
        .await;
    assert_eq!(s, 201);
    app.login_as("bob", "bobpass123").await
}

#[tokio::test]
async fn jd_interpret_structures_and_persists() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let aid = mk_application(&app, "解读公司").await;

    // 创建投递后给岗位贴 JD（ADR-0012：JD 属于岗位）
    let pid = {
        let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
        d["application"]["position_id"].as_i64().unwrap()
    };
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/positions/{pid}"),
            Some(json!({ "jd_text": "负责网关设计，要求熟悉 Rust 与 K8s，有高并发经验加分" })),
        )
        .await;
    assert_eq!(s, 200);

    mock.queue_nonstream(
        r#"{"overall":"平台大、要求扎实，值得投","cautions":["职责偏运维，与标题不符"]}"#,
    );
    let (s, v) = app
        .req(Method::POST, &format!("/api/applications/{aid}/interpret"), None)
        .await;
    assert_eq!(s, 200, "解读应受理: {v}");
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await; // ADR-0013 任务化：等终态后读落库

    // 持久化：详情聚合带出 jd_interpret
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(d["application"]["jd_interpret"]["cautions"][0], "职责偏运维，与标题不符");

    // 无 JD 时拒绝
    let aid2 = mk_application(&app, "无JD公司").await;
    let (s, e) = app.req(Method::POST, &format!("/api/applications/{aid2}/interpret"), None).await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("JD"));
}

#[tokio::test]
async fn jd_match_scores_against_resume() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 简历（结构化）
    let (s, _) = app
        .req(
            Method::PUT,
            "/api/resume",
            Some(json!({
                "raw_text": "张三的简历",
                "parsed": { "name": "张三", "skills": ["Java"], "projects": [{"name": "订单系统", "detail": "高并发"}] }
            })),
        )
        .await;
    assert!(s.is_success());

    let aid = mk_application(&app, "匹配公司").await;
    let pid = {
        let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
        d["application"]["position_id"].as_i64().unwrap()
    };
    app.req(
        Method::PATCH,
        &format!("/api/positions/{pid}"),
        Some(json!({ "jd_text": "要求 Rust + K8s 经验" })),
    )
    .await;

    mock.queue_nonstream(
        r#"{"score": 120, "summary": "总体匹配中等", "strengths": ["高并发项目经验"], "gaps": ["缺少 Kubernetes 生产实践经验"], "resume_advice": ["项目经历补充压测数据与量化指标"]}"#,
    );
    let (s, v) = app.req(Method::POST, &format!("/api/applications/{aid}/match"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await; // ADR-0013 任务化
    let (_, m) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    let v = &m["application"]["jd_match"];
    assert_eq!(v["score"].as_i64(), Some(100), "分数应钳制到 0-100");
    assert_eq!(v["gaps"][0], "缺少 Kubernetes 生产实践经验");
    assert_eq!(v["resume_advice"][0], "项目经历补充压测数据与量化指标", "应输出简历修改建议");

    // 无简历时拒绝
    let (_, bob) = create_and_login_second_user(&app).await;
    let bob = bob.unwrap();
    let aid2 = {
        // B 建自己的投递（带 JD，让校验走到「无简历」分支）
        let (s, c) = app.req_as(&bob, Method::POST, "/api/companies", Some(json!({"name":"B公司"}))).await;
        assert_eq!(s, 201);
        let cid = c["id"].as_i64().unwrap();
        let (s, a) = app.req_as(&bob, Method::POST, "/api/applications", Some(json!({"company_id": cid, "position": "后端", "jd_text": "要求 Rust"}))).await;
        assert!(s.is_success());
        a["id"].as_i64().unwrap()
    };
    let (s2, e2) = app
        .req_as(&bob, Method::POST, &format!("/api/applications/{}/match", aid2), None)
        .await;
    assert_eq!(s2, 400, "B 无简历应拒绝");
    assert!(e2["error"].as_str().unwrap().contains("简历"));
}

#[tokio::test]
async fn drill_with_application_uses_jd_in_prompt() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let aid = mk_application(&app, "JD陪练公司").await;
    let pid = {
        let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
        d["application"]["position_id"].as_i64().unwrap()
    };
    app.req(
        Method::PATCH,
        &format!("/api/positions/{pid}"),
        Some(json!({ "jd_text": "MARKER_JD_需要深入掌握 Rust 异步运行时" })),
    )
    .await;

    // 建陪练场次并关联投递
    let (_, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "position": "后端", "application_id": aid })),
        )
        .await;
    let did = d["id"].as_i64().unwrap();

    mock.queue_stream(vec!["什么是异步运行时？".to_string()]);
    let (s, _) = app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    assert!(s.is_success());

    // LLM 请求边界应包含 JD 标记（Responses 形态：system 提升 instructions，对话在 input）
    let bodies = mock.request_bodies();
    let body_has = |b: &serde_json::Value, needle: &str| {
        b["instructions"].as_str().map_or(false, |s| s.contains(needle))
            || b["input"].as_array().map_or(false, |ms| {
                ms.iter().any(|m| m["content"].as_str().unwrap_or("").contains(needle))
            })
    };
    assert!(
        bodies.iter().any(|b| body_has(b, "MARKER_JD_需要深入掌握 Rust 异步运行时")),
        "陪练 prompt 应注入关联投递的 JD"
    );

    // 未关联投递的场次不注入
    let (_, d2) = app.req(Method::POST, "/api/drills", Some(json!({ "kind": "interview" }))).await;
    let did2 = d2["id"].as_i64().unwrap();
    mock.queue_stream(vec!["下一题".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did2}/messages"), Some(json!({ "content": "开始" }))).await;
    let bodies = mock.request_bodies();
    let last = bodies.last().unwrap();
    let last_has = |needle: &str| {
        last["instructions"].as_str().map_or(false, |s| s.contains(needle))
            || last["input"].as_array().map_or(false, |ms| {
                ms.iter().any(|m| m["content"].as_str().unwrap_or("").contains(needle))
            })
    };
    assert!(!last_has("MARKER_JD"), "未关联投递不应注入 JD");
}

#[tokio::test]
async fn retrospective_roundtrip_and_to_review() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 投递 + 轮次 + 一题
    let aid = mk_application(&app, "复盘公司").await;
    let rid = common::create_round(&app, aid, "一面").await;
    let (s, _) = app.req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "讲讲索引", "my_answer": "B+树…" }))).await;
    assert!(s.is_success());

    // PUT 保存轮次复盘
    let (s, _) = app
        .req(
            Method::PUT,
            &format!("/api/rounds/{rid}/retrospective"),
            Some(json!({
                "overall": "基础扎实，深度不足",
                "problems": ["索引下推说不清"],
                "improvements": ["补齐索引下推原理", "练习 STAR 表达"]
            })),
        )
        .await;
    assert_eq!(s, 200);
    let (_, v) = app.req(Method::GET, &format!("/api/rounds/{rid}/retrospective"), None).await;
    assert_eq!(v["retrospective"]["overall"], "基础扎实，深度不足");
    assert_eq!(v["retrospective"]["generated_by_ai"], false);

    // 改进项一键入复习队列
    let (s, v) = app
        .req(
            Method::POST,
            &format!("/api/rounds/{rid}/retrospective/to-review"),
            Some(json!({ "items": ["补齐索引下推原理", "练习 STAR 表达"] })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["created"].as_i64(), Some(2));

    let n_q: i64 = sqlx::query_scalar("SELECT count(*) FROM questions WHERE round_id=$1 AND content LIKE '补齐%'")
        .bind(rid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(n_q, 1);
    let n_r: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.round_id=$1",
    )
    .bind(rid)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(n_r >= 2, "改进项应直接入复习队列，实际 {n_r}");
}

#[tokio::test]
async fn retrospective_ai_draft_persists() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let aid = mk_application(&app, "AI复盘公司").await;
    let rid = common::create_round(&app, aid, "一面").await;
    app.req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "什么是事务隔离级别？", "my_answer": "Read Committed…" }))).await;

    mock.queue_nonstream(
        r#"{"performance":"良好","match":"中高","confidence":"高","overall":"基础概念清楚，深度欠缺",
        "strengths":[{"point":"概念表述清晰","evidence":"事务隔离级别题回答完整","why_plus":"展示知识体系"}],
        "weaknesses":[{"question":"什么是事务隔离级别？","problem":"未讲清 MVCC 实现","impact":"面试官可能疑虑深度","better":"补 MVCC 原理后再答"}],
        "abilities":[{"ability":"数据库基础","tested":true,"evidence_strength":"中","risk":"深度待证明"}],
        "interviewer_view":{"positive":["基础扎实"],"doubts":["深度不足"],"unverified":["工程实践"]},
        "problems":["未讲清可重复读的实现"],"improvements":["学习 MVCC 原理"],"advice":"下一场前补齐 MVCC 与索引原理"}"#,
    );
    let (s, v) = app.req(Method::POST, &format!("/api/rounds/{rid}/retrospective/ai"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await; // ADR-0013 任务化
    let (_, v) = app.req(Method::GET, &format!("/api/rounds/{rid}/retrospective"), None).await;
    assert_eq!(v["retrospective"]["generated_by_ai"], true);
    // 新结构化字段（反馈 #2：参考全场复盘教练 prompt 适配）
    assert_eq!(v["retrospective"]["performance"], "良好");
    assert_eq!(v["retrospective"]["strengths"][0]["point"], "概念表述清晰");
    assert_eq!(v["retrospective"]["weaknesses"][0]["better"], "补 MVCC 原理后再答");
    assert_eq!(v["retrospective"]["interviewer_view"]["doubts"][0], "深度不足");
    // 兼容字段：improvements 仍可一键入复习队
    assert_eq!(v["retrospective"]["improvements"][0], "学习 MVCC 原理");

    let (_, v) = app.req(Method::GET, &format!("/api/rounds/{rid}/retrospective"), None).await;
    assert_eq!(v["retrospective"]["generated_by_ai"], true);

    // 无题轮次拒绝（上一面通过才能加下一面，反馈七#2）
    app.req(Method::PATCH, &format!("/api/rounds/{rid}"), Some(json!({ "passed": "pass" }))).await;
    let rid2 = common::create_round(&app, aid, "二面").await;
    let (s, e) = app.req(Method::POST, &format!("/api/rounds/{rid2}/retrospective/ai"), None).await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("题目"));
}

#[tokio::test]
async fn rounds_can_be_added_repeatedly_but_names_default_increment() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = mk_application(&app, "多轮公司").await;

    // 多轮面试是有意设计：默认名称自动递增（上一面通过才能加下一面，反馈七#2）
    let (_, r1) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({}))).await;
    app.req(Method::PATCH, &format!("/api/rounds/{}", r1["id"].as_i64().unwrap()), Some(json!({ "passed": "pass" }))).await;
    let (_, r2) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({}))).await;
    assert_eq!(r1["name"], "一面");
    assert_eq!(r2["name"], "二面");
}

#[tokio::test]
async fn create_application_with_new_company_name_creates_company() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 直接输新公司名建投递（致命 bug 回归：看板必须能新增公司）
    let (s, a) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "全新公司X", "position": "后端", "jd_text": "JD 原文" })),
        )
        .await;
    assert_eq!(s, 201, "输新公司名应能直接建投递");
    let aid = a["id"].as_i64().unwrap();

    // 公司被自动创建；JD 存到岗位（ADR-0012）
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(d["application"]["company"], "全新公司X");
    let pid = d["application"]["position_id"].as_i64().unwrap();
    let (_, p) = app.req(Method::GET, &format!("/api/positions/{pid}"), None).await;
    assert_eq!(p["jd_text"], "JD 原文", "创建时的 JD 应写入岗位");

    // 再建一份同名公司的投递 -> 复用同一家公司
    let (s, a2) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "全新公司X", "position": "前端" })),
        )
        .await;
    assert_eq!(s, 201);
    let (_, d2) = app.req(Method::GET, &format!("/api/applications/{}", a2["id"].as_i64().unwrap()), None).await;
    assert_eq!(d2["application"]["company_id"], d["application"]["company_id"], "同名公司应复用");
}

// ---------- v4.1 反馈 #3/#4：轮次子页聚合 + 复盘基于首次真实回答 ----------

/// 子页聚合：题目带第一手真实回答（优先 interview 来源最早一条）
#[tokio::test]
async fn round_detail_aggregates_questions_with_first_answer() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = mk_application(&app, "子页公司").await;
    let rid = common::create_round(&app, aid, "一面").await;
    let (s, q) = app
        .req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "讲讲事务隔离", "my_answer": "当前版本答案" })))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let qid = q["id"].as_i64().unwrap();

    // 作答历史：先手动补答、后面试现场原话（interview 来源应胜出）
    for (src, text) in [("manual", "手动补答版"), ("interview", "现场第一手原话")] {
        sqlx::query("INSERT INTO question_answers(question_id, source, content) VALUES($1,$2,$3)")
            .bind(qid)
            .bind(src)
            .bind(text)
            .execute(&app.pool)
            .await
            .unwrap();
    }

    let (s, d) = app.req(Method::GET, &format!("/api/rounds/{rid}/detail"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(d["round"]["id"].as_i64(), Some(rid));
    assert_eq!(d["application"]["id"].as_i64(), Some(aid));
    assert_eq!(d["application"]["company"], "子页公司");
    let qs = d["questions"].as_array().unwrap();
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0]["first_answer"]["content"], "现场第一手原话", "应取 interview 来源的第一手回答");
    assert_eq!(qs[0]["first_answer"]["source"], "interview");

    // 无作答题：first_answer 为 null 且不报错
    let (s, q2) = app.req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "没答过的题" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, d2) = app.req(Method::GET, &format!("/api/rounds/{rid}/detail"), None).await;
    let qs2 = d2["questions"].as_array().unwrap();
    assert_eq!(qs2.len(), 2);
    assert!(qs2.iter().find(|x| x["id"].as_i64() == q2["id"].as_i64()).unwrap()["first_answer"].is_null());
}

/// 复盘升级：输入含第一手真实回答；输出扩展 weaknesses/advice；AI 重生成不覆盖人类心得
#[tokio::test]
async fn retro_ai_uses_first_answer_and_extended_schema() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let aid = mk_application(&app, "复盘升级公司").await;
    let rid = common::create_round(&app, aid, "一面").await;
    let (s, q) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "什么是 MVCC？", "my_answer": "MARK_CURRENT_被复习覆盖的答案" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    sqlx::query("INSERT INTO question_answers(question_id, source, content) VALUES($1,'interview','MARK_FIRST_现场讲不清隔离级别实现')")
        .bind(q["id"].as_i64().unwrap())
        .execute(&app.pool)
        .await
        .unwrap();

    // 先写入人类心得
    let (s, _) = app
        .req(
            Method::PUT,
            &format!("/api/rounds/{rid}/retrospective"),
            Some(json!({ "overall": "占位", "problems": [], "improvements": [], "notes": "我的心得：下次带纸笔" })),
        )
        .await;
    assert_eq!(s, 200);

    mock.queue_nonstream(
        r#"{"overall":"概念能说清，深度不足","weaknesses":["MVCC 实现细节空白"],"problems":["未讲清 ReadView"],"improvements":["补 MVCC 原理"],"advice":"下一场主动引导到自己熟悉的项目"}"#,
    );
    let (s, v) = app.req(Method::POST, &format!("/api/rounds/{rid}/retrospective/ai"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await; // ADR-0013 任务化
    let (_, rv) = app.req(Method::GET, &format!("/api/rounds/{rid}/retrospective"), None).await;
    let v = &rv["retrospective"];
    assert_eq!(v["weaknesses"][0], "MVCC 实现细节空白", "输出应含薄弱点");
    assert_eq!(v["advice"], "下一场主动引导到自己熟悉的项目", "输出应含综合建议");
    // 心得不被 AI 覆盖
    assert_eq!(v["notes"], "我的心得：下次带纸笔");

    // LLM 输入应包含第一手真实回答、不包含被覆盖的当前答案（Responses 形态）
    let bodies = mock.request_bodies();
    let body_has = |b: &serde_json::Value, needle: &str| {
        b["instructions"].as_str().map_or(false, |s| s.contains(needle))
            || b["input"].as_array().map_or(false, |ms| {
                ms.iter().any(|m| m["content"].as_str().unwrap_or("").contains(needle))
            })
    };
    assert!(
        bodies.iter().any(|b| body_has(b, "MARK_FIRST_现场讲不清隔离级别实现")),
        "prompt 应含第一手真实回答"
    );
    // ADR-0016 D4：封闭枚举落位到 strict json_schema（enum 强制），而非仅靠 prompt 文字约定
    assert!(
        bodies.iter().any(|b| {
            let enums = &b["text"]["format"]["schema"]["properties"]["performance"]["enum"];
            enums.is_array() && enums.as_array().unwrap().len() == 4
        }),
        "retrospective schema 应含 performance 枚举（优秀/良好/一般/偏弱）"
    );
    assert!(
        !bodies.iter().any(|b| body_has(b, "MARK_CURRENT_被复习覆盖的答案")),
        "prompt 不应用被复习覆盖后的当前答案"
    );

    // 持久化：weaknesses/advice 落库，且人类心得未被 AI 覆盖
    let (_, d) = app.req(Method::GET, &format!("/api/rounds/{rid}/detail"), None).await;
    let retro = &d["retrospective"];
    assert_eq!(retro["weaknesses"][0], "MVCC 实现细节空白");
    assert_eq!(retro["advice"], "下一场主动引导到自己熟悉的项目");
    assert_eq!(retro["notes"], "我的心得：下次带纸笔", "AI 重生成不应覆盖人类心得");
    assert_eq!(retro["generated_by_ai"], true);
}

/// 反馈 #1：投递整体复盘 AI（终态解锁；结构化落 overall_analysis；LLM 边界含 JD/简历/逐题记录）
#[tokio::test]
async fn overall_analysis_ai_structured() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 投递 + 岗位 JD + 一轮一题（带第一手回答）+ 简历
    let aid = mk_application(&app, "整体复盘公司").await;
    let pid = {
        let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
        d["application"]["position_id"].as_i64().unwrap()
    };
    let (s, _) = app
        .req(
            Method::PATCH,
            &format!("/api/positions/{pid}"),
            Some(json!({ "jd_text": "MARKER_JD_要求深入掌握 Rust 异步与高并发" })),
        )
        .await;
    assert_eq!(s, 200);
    let rid = common::create_round(&app, aid, "一面").await;
    app.req(
        Method::POST,
        "/api/questions",
        Some(json!({ "round_id": rid, "content": "MARKER_Q_讲讲异步运行时", "my_answer": "MARKER_A_我从 Tokio 源码角度…" })),
    )
    .await;
    let (s, _) = app
        .req(
            Method::PUT,
            "/api/resume",
            Some(json!({
                "raw_text": "张三的简历",
                "parsed": { "name": "张三", "skills": ["Rust", "Tokio"], "projects": [{"name": "网关", "detail": "高并发"}] }
            })),
        )
        .await;
    assert!(s.is_success());

    // 推进到终态（添加首场已自动 interviewing；直接标 offer）
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{aid}"), Some(json!({ "status": "offer" })))
        .await;
    assert_eq!(s, 200);

    mock.queue_nonstream(
        r##"{"performance":"良好","match":"中高","confidence":"高",
        "summary":"整场表现与简历预期基本一致，深度证明不足",
        "strengths":["异步基础扎实","项目表述具体"],"risks":["深度追问易暴露边界"],"loss_points":["MVCC 未讲清"],
        "keep_answers":["异步运行时题回答"],"retrain_answers":["隔离级别题回答"],
        "ability_matrix":[{"ability":"Rust 异步","importance":"高","evidence":"MARKER_Q_讲讲异步运行时的回答","risk":"深度待补"}],
        "improvements":[{"priority":1,"problem":"数据库深度不足","action":"系统学习 MVCC 并重讲一遍"}],
        "report":"# 一、整场面试结论\n…（全文）"}"##,
    );
    let (s, v) = app
        .req(Method::POST, &format!("/api/applications/{aid}/overall-analysis/ai"), None)
        .await;
    assert_eq!(s, 200, "终态后应可触发整体复盘 AI");
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;

    // 结构化落库可提取
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}/overall-analysis"), None).await;
    assert_eq!(d["analysis"]["generated_by_ai"], true);
    assert_eq!(d["analysis"]["performance"], "良好");
    assert_eq!(d["analysis"]["match"], "中高");
    assert_eq!(d["analysis"]["loss_points"][0], "MVCC 未讲清");
    assert_eq!(d["analysis"]["improvements"][0]["action"], "系统学习 MVCC 并重讲一遍");
    assert!(d["analysis"]["report"].as_str().unwrap().contains("整场面试结论"));

    // LLM 请求边界：应包含 JD 标记、题目、第一手回答与简历
    let bodies = mock.request_bodies();
    let joined = format!("{bodies:?}");
    assert!(joined.contains("MARKER_JD_要求深入掌握 Rust 异步与高并发"), "应注入 JD");
    assert!(joined.contains("MARKER_Q_讲讲异步运行时"), "应注入逐题记录");
    assert!(joined.contains("MARKER_A_我从 Tokio 源码角度"), "应注入第一手真实回答");
    assert!(joined.contains("张三"), "应注入简历");
    // 长文任务 max_output_tokens 应提升到 LONG 档（修复：2048 硬编码截断整体复盘输出）
    assert!(
        bodies.iter().any(|b| b["max_output_tokens"].as_u64() == Some(8192)),
        "整体复盘应使用长文档 max_output_tokens=8192: {bodies:?}"
    );

    // 非终态拒绝：新建一份未终态投递
    let aid2 = mk_application(&app, "未终态公司").await;
    let (s, e) = app.req(Method::POST, &format!("/api/applications/{aid2}/overall-analysis/ai"), None).await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("终态"));
}

#[tokio::test]
async fn create_application_persists_department() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_, c) = app
        .req(Method::POST, "/api/companies", Some(json!({ "name": "平台技术部公司" })))
        .await;
    let cid = c["id"].as_i64().unwrap();

    let (s, app_res) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({
                "company_id": cid,
                "position": "基础架构工程师",
                "department": "核心中间件团队",
                "location": "北京"
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let aid = app_res["id"].as_i64().unwrap();

    let (s, detail) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(detail["application"]["department"], "核心中间件团队", "新建投递应正确保存并返回所属部门");
}

#[tokio::test]
async fn create_application_with_new_company_and_new_position() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (s, app_res) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({
                "company_name": "全新未收录公司",
                "position": "资深后端架构师",
                "department": "云原生平台部",
                "location": "上海",
                "salary": "35-50k",
                "jd_text": "熟悉 Rust / K8s / 分布式系统"
            })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let aid = app_res["id"].as_i64().unwrap();

    let (s, detail) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(detail["application"]["company"], "全新未收录公司");
    assert_eq!(detail["application"]["position"], "资深后端架构师");
    assert_eq!(detail["application"]["department"], "云原生平台部");
    assert_eq!(detail["application"]["location"], "上海");
    let pid = detail["application"]["position_id"].as_i64().unwrap();
    let (_, p) = app.req(Method::GET, &format!("/api/positions/{pid}"), None).await;
    assert_eq!(p["jd_text"], "熟悉 Rust / K8s / 分布式系统");
}
