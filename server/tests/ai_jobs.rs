//! ADR-0013：AI 任务幂等化集成测试。
//! 覆盖：POST /ref 受理形状、任务终态后结果落库、同题 running 幂等去重、
//! 域 GET 暴露 running 任务（刷新恢复通道）、SSE /api/events 认证与事件推送。

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use futures_util::StreamExt;
use serde_json::json;
use tower::ServiceExt;

mod common;
use common::TestApp;

/// 建一题并返回 id（带回答，供 /analyze 用）
async fn create_question_with_answer(app: &TestApp, content: &str) -> i64 {
    let aid = common::create_application(app, "幂等公司", "后端").await;
    let rid = common::create_round(app, aid, "一面").await;
    let (s, v) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": content, "my_answer": "我的现场回答" })),
        )
        .await;
    assert_eq!(s, 201);
    v["id"].as_i64().unwrap()
}

/// POST /ref 受理即返回 {job_id, status}；终态后参考答案落库可见，且域 GET 的 ai_jobs 清空
#[tokio::test]
async fn ref_post_accepts_job_and_persists_after_done() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let qid = create_question_with_answer(&app, "什么是零拷贝？").await;

    mock.queue_nonstream(r#"{"tags":["OS"],"difficulty":3,"ref_answer":"mmap + sendfile"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200, "POST /ref 应受理: {v}");
    assert!(v["job_id"].as_u64().unwrap() > 0, "应返回 job_id: {v}");
    assert_eq!(v["status"], "running");

    // 终态：done；结果落库
    let done = common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;
    assert_eq!(done["status"], "done");
    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let row = d["analyses"].as_array().unwrap().iter().find(|a| a["ref_answer"] == "mmap + sendfile");
    assert!(row.is_some(), "任务完成后参考答案应落库");
    // 域 GET 不再暴露 running 任务（空时字段被 skip）
    assert_eq!(d["ai_jobs"].as_array().map(|a| a.len()).unwrap_or(0), 0, "完成后 ai_jobs 应清空");
}

/// 同题 ref 在跑时再次 POST：幂等去重——返回同一 job_id，不重复起 LLM（请求体只此一份）
#[tokio::test]
async fn duplicate_running_job_is_deduplicated() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let qid = create_question_with_answer(&app, "什么是写时复制？").await;

    mock.set_delay_ms(1500); // 模拟慢 LLM，制造可观察的 running 窗口
    mock.queue_nonstream(r#"{"tags":["OS"],"difficulty":4,"ref_answer":"COW 说明"}"#);
    let (s, v1) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200);
    let job1 = v1["job_id"].as_u64().unwrap();

    // running 态在域 GET 可见（刷新恢复通道）
    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let jobs = d["ai_jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1, "应暴露一个 running 任务");
    assert_eq!(jobs[0]["id"].as_u64(), Some(job1));
    assert_eq!(jobs[0]["kind"], "ref");
    assert_eq!(jobs[0]["status"], "running");

    // 二次点击 → 幂等：同一 job，不新增 LLM 请求
    let (s2, v2) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s2, 200);
    assert_eq!(v2["job_id"].as_u64(), Some(job1), "running 中重复触发应去重");

    // 等待完成；LLM 只被调用一次
    let done = common::wait_ai_job(&app, job1, 8000).await;
    assert_eq!(done["status"], "done");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let llm_calls = mock.request_bodies().iter().filter(|b| b["stream"] != json!(true)).count();
    assert_eq!(llm_calls, 1, "同题 running 去重后 LLM 只应被调用一次");
}

/// 完成后的任务允许重跑（重新分析语义）：新 job_id、新分析行
#[tokio::test]
async fn rerun_after_done_creates_new_job() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let qid = create_question_with_answer(&app, "什么是内存屏障？").await;

    mock.queue_nonstream(r#"{"tags":["并发"],"difficulty":3,"ref_answer":"第一版"}"#);
    let (s, v1) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200);
    common::wait_ai_job(&app, v1["job_id"].as_u64().unwrap(), 5000).await;

    mock.queue_nonstream(r#"{"tags":["并发"],"difficulty":3,"ref_answer":"第二版"}"#);
    let (s, v2) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200);
    assert_ne!(v2["job_id"].as_u64(), v1["job_id"].as_u64(), "完成后重跑应是新任务");
    common::wait_ai_job(&app, v2["job_id"].as_u64().unwrap(), 5000).await;

    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    assert!(d["analyses"].as_array().unwrap().iter().any(|a| a["ref_answer"] == "第二版"));
}

/// SSE /api/events：未登录 401；登录后能收到任务的 running/done 事件（per-user 回显通道）
#[tokio::test]
async fn events_stream_requires_auth_and_pushes_events() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let cookie = app.cookie.clone().expect("已登录应有 cookie");
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let qid = create_question_with_answer(&app, "什么是虚拟内存？").await;

    // 未登录 → 401
    let req = Request::builder()
        .method(Method::GET)
        .uri("http://test/api/events")
        .body(Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 登录态：打开 SSE（先订阅再触发，保证事件不丢）
    let req = Request::builder()
        .method(Method::GET)
        .uri("http://test/api/events")
        .header(header::COOKIE, cookie.clone())
        .body(Body::empty())
        .unwrap();
    let resp = app.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/event-stream"),
        "应为 text/event-stream"
    );
    let mut stream = resp.into_body().into_data_stream();

    // 触发一个慢任务，随后从流里读到 running 与 done 两帧。
    // 只克隆 Router（不能 move TestApp——它持有 DB 锁守卫，提前释放会放行并行测试清库）。
    mock.set_delay_ms(600);
    mock.queue_nonstream(r#"{"tags":["OS"],"difficulty":3,"ref_answer":"页表映射"}"#);
    let router = app.app.clone();
    let cookie2 = cookie.clone();
    let handle = tokio::spawn(async move {
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("http://test/api/questions/{qid}/ref"))
            .header(header::COOKIE, cookie2)
            .body(Body::empty())
            .unwrap();
        router.oneshot(req).await.unwrap().status()
    });

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    let (mut buf, mut saw_done) = (String::new(), false);
    while tokio::time::Instant::now() < deadline {
        let chunk = tokio::time::timeout(std::time::Duration::from_millis(500), stream.next()).await;
        match chunk {
            Ok(Some(Ok(bytes))) => buf.push_str(&String::from_utf8_lossy(&bytes)),
            _ => continue,
        }
        if buf.contains("\"status\":\"done\"") {
            saw_done = true;
            break;
        }
    }
    assert_eq!(handle.await.unwrap(), StatusCode::OK);
    assert!(buf.contains("\"kind\":\"ref\""), "流中应有 ref 事件: {buf}");
    assert!(buf.contains("\"target_id\"") && buf.contains(&qid.to_string().as_str()), "流中应带目标 id: {buf}");
    assert!(saw_done, "流中应收到 done 帧: {buf}");
}

/// 评价式出口（如题目分析）：二次纠偏依然失败时降级为 Markdown 直出，ir_mode 如实标注为 text
#[tokio::test]
async fn evaluative_exit_degrades_to_markdown_on_double_json_failure() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;
    let qid = create_question_with_answer(&app, "什么是 CAS？").await;

    // 第一次返回非 JSON 文本，就地纠偏重试也返回非 JSON 纯 Markdown 评价
    mock.queue_nonstream("这是非 JSON 的普通回复，缺少括号");
    mock.queue_nonstream("## CAS 点评\n回答得非常清晰，说明了底层 cmpxchg 指令。");

    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/analyze"), None).await;
    assert_eq!(s, 200, "POST /analyze 应受理: {v}");
    let done = common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;
    assert_eq!(done["status"], "done");

    let (_, d) = app.req(Method::GET, &format!("/api/questions/{qid}"), None).await;
    let analyses = d["analyses"].as_array().unwrap();
    assert!(!analyses.is_empty(), "分析记录应落库");
    let a = &analyses[analyses.len() - 1];
    assert!(
        a["feedback"].as_str().unwrap().contains("CAS 点评"),
        "应降级为纯文本 Markdown 评价: {a:?}"
    );
}


