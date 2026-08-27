//! ADR-0016 行为规格：llm_config 多 Provider/Model + 能力位降级语义。
//!
//! 覆盖：
//! 1. caps.structured_output=false → 评审型出口走「纯文本评审」：全文落库、score/tags/difficulty 空、
//!    raw.ir_mode=text（无需解析直接展示）；
//! 2. 结构必需出口（试卷生成/简历解析）无结构化能力即 400 拒绝；
//! 3. 能力位/高级参数到达请求边界：web_search→tools、store 显式下发、reasoning_effort 原样、
//!    extra_body 并入顶层、strict json_schema。

use axum::http::Method;
use serde_json::{json, Value};
use sqlx::Row;

use common::TestApp;
use common::llm_mock::LlmMock;

mod common;

/// 行为桩：对任意请求回固定状态（模拟严格网关 405 / http→https 重定向）
async fn start_stub(status_line: &'static str, location: Option<&'static str>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    let body = "{\"error\":{\"message\":\"Method Not Allowed\"}}".to_string();
    let head = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
        location.map(|l| format!("Location: {l}\r\n")).unwrap_or_default(),
        body.len()
    );
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { continue };
            let head = head.clone();
            let body = body.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let mut buf = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
            });
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

async fn put_llm_config(app: &TestApp, base_url: &str, structured: bool, web_search: bool, advanced: Value) {
    let (s, v) = app
        .req(
            Method::PUT,
            "/api/settings/llm-config",
            Some(json!({
                "providers": [{ "id": "p1", "name": "Mock", "base_url": base_url, "api_key": "" }],
                "models": [{
                    "id": "m1", "provider_id": "p1", "name": "mock",
                    "context_length": 128000,
                    "caps": { "structured_output": structured, "web_search": web_search },
                    "advanced": advanced
                }],
                "active_model_id": "m1"
            })),
        )
        .await;
    assert_eq!(s, 200, "put llm-config 失败: {v}");
}

/// 建 投递→轮次→题目（v4 核心单元链）
async fn setup_question(app: &TestApp) -> i64 {
    let (s, a) = app
        .req(Method::POST, "/api/applications", Some(json!({ "company_name": "测试公司", "position": "后端" })))
        .await;
    assert_eq!(s, 201);
    let aid = a["id"].as_i64().unwrap();
    let (s, r) = app.req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面" }))).await;
    assert_eq!(s, 201);
    let rid = r["id"].as_i64().unwrap();
    let (s, q) = app
        .req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "什么是索引下推？" })))
        .await;
    assert_eq!(s, 201);
    q["id"].as_i64().unwrap()
}

/// 纯文本评审模式（ADR-0016 D3）：ref 出口全文落 ref_answer，tags/difficulty/score 空，ir_mode=text
#[tokio::test]
async fn text_review_mode_stores_fulltext_without_parsing() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    put_llm_config(&app, &mock.base_url(), false, false, json!({})).await;

    let qid = setup_question(&app).await;
    mock.queue_nonstream("## 参考答案\n索引下推把过滤条件推到存储层，减少回表。TEXT_REVIEW_MARKER");
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200, "{v}");
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;

    let row = sqlx::query(
        "SELECT ref_answer, score, difficulty, tags, raw FROM analyses WHERE question_id=$1 ORDER BY id DESC LIMIT 1",
    )
    .bind(qid)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let ref_answer: String = row.get("ref_answer");
    assert!(ref_answer.contains("TEXT_REVIEW_MARKER"), "全文应原样落库");
    assert_eq!(ref_answer, "## 参考答案\n索引下推把过滤条件推到存储层，减少回表。TEXT_REVIEW_MARKER", "不解析不改写");
    let score: Option<i32> = row.get("score");
    let difficulty: Option<i32> = row.get("difficulty");
    assert_eq!(score, None, "text 模式无评分");
    assert_eq!(difficulty, None, "text 模式无难度");
    let tags: Value = row.get("tags");
    assert_eq!(tags, json!([]), "text 模式无标签");
    let raw: Value = row.get("raw");
    assert_eq!(raw["ir_mode"], "text", "raw 应带 text 判别标记");
}

/// 结构必需出口（ADR-0016 D3）：简历解析在无结构化能力时 400 拒绝
/// （票 01：paper_generate 已随试卷退役删除；interview_prep 的能力检查在任务执行侧，受理前同步拒绝的唯一出口是简历解析）
#[tokio::test]
async fn structured_required_exits_reject_when_capability_off() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    put_llm_config(&app, &mock.base_url(), false, false, json!({})).await;

    // 简历解析拒绝（受理前同步 400）
    let (s, _) = app.req(Method::PUT, "/api/resume", Some(json!({ "raw_text": "张三 后端工程师" }))).await;
    assert!(s.is_success());
    let (s, v) = app.req(Method::POST, "/api/resume/parse", Some(json!({}))).await;
    assert_eq!(s, 400);
    assert!(v["error"].as_str().unwrap_or("").contains("结构化输出"), "应给能力位提示: {v}");
}

/// 能力位与高级参数到达请求边界：tools/store/reasoning.effort/extra_body/strict schema/LONG 档
#[tokio::test]
async fn responses_request_carries_caps_store_and_extra_body() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    put_llm_config(
        &app,
        &mock.base_url(),
        true,
        true,
        json!({
            "reasoning_effort": "max",
            "store": true,
            "temperature": 0.4,
            // SDK 风格包裹形式：存储归一为内层裸 KV，下发时以字面 extra_body 字段嵌套（ADR-0016 修订）
            "extra_body": { "extra_body": { "enable_thinking": true } }
        }),
    )
    .await;

    let qid = setup_question(&app).await;
    mock.queue_nonstream(r#"{"tags":["索引"],"difficulty":3,"ref_answer":"R"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200, "{v}");
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;

    let bodies = mock.request_bodies();
    let b = bodies.last().expect("应捕获到请求体");
    assert_eq!(b["model"], "mock");
    assert_eq!(b["store"], true, "store 应显式下发（本例用户开启）");
    assert_eq!(b["reasoning"]["effort"], "max", "七档 effort 应原样下发");
    assert_eq!(b["extra_body"]["enable_thinking"], true, "extra_body 应以字面字段嵌套下发");
    assert!(b.get("enable_thinking").is_none(), "extra_body 不得平铺进请求体顶层");
    assert_eq!(b["temperature"], 0.4);
    assert_eq!(b["tools"][0]["type"], "web_search", "联网搜索能力位 → 内置工具");
    assert_eq!(b["text"]["format"]["type"], "json_schema");
    assert_eq!(b["text"]["format"]["strict"], true);
    assert_eq!(b["text"]["format"]["name"], "question_ref");
    assert_eq!(
        b["text"]["format"]["schema"]["additionalProperties"],
        false,
        "strict schema 纪律"
    );
    // ref 是长文任务 → LONG 档（默认 8192）
    assert_eq!(b["max_output_tokens"], 8192);

    // 结果正常解析落库（structured 模式）
    let row = sqlx::query("SELECT difficulty, raw FROM analyses WHERE question_id=$1 ORDER BY id DESC LIMIT 1")
        .bind(qid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let difficulty: Option<i32> = row.get("difficulty");
    assert_eq!(difficulty, Some(3));
    let raw: Value = row.get("raw");
    assert_eq!(raw["ir_mode"], "structured");
}

/// 局部保存：PATCH 单个模型/provider/global 只替换目标条目，其余原样保留
#[tokio::test]
async fn scoped_patch_updates_only_target() {
    use axum::http::Method as M2;

    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    put_llm_config(&app, &mock.base_url(), true, false, json!({"reasoning_effort": "xhigh"})).await;

    // PATCH 单个模型：关闭结构化输出、降思考强度——其余字段不动
    let (s, v) = app
        .req(
            M2::PATCH,
            "/api/settings/llm-config/models/m1",
            Some(json!({
                "id": "m1", "provider_id": "p1", "name": "mock", "context_length": 128000,
                "caps": { "structured_output": false, "web_search": false },
                "advanced": { "reasoning_effort": "none", "store": false,
                              "extra_body": { "extra_body": { "enable_thinking": false } } }
            })),
        )
        .await;
    assert_eq!(s, 200, "{v}");

    let (_, d) = app.req(M2::GET, "/api/settings/llm-config", None).await;
    let m = &d["config"]["models"][0];
    assert_eq!(m["caps"]["structured_output"], false, "仅该模型被更新");
    assert_eq!(m["advanced"]["reasoning_effort"], "none");
    assert_eq!(m["advanced"]["extra_body"]["enable_thinking"], false, "包裹形式归一为裸 KV");
    assert_eq!(d["config"]["active_model_id"], "m1", "激活模型不受影响");
    assert_eq!(d["resolved"]["reasoning_effort"], "none");

    // PATCH 全局参数
    let (s, _) = app
        .req(M2::PATCH, "/api/settings/llm-config/global", Some(json!({ "timeout": 300, "max_output_tokens_short": 4096, "max_output_tokens_long": 16384 })))
        .await;
    assert_eq!(s, 200);
    let (_, d) = app.req(M2::GET, "/api/settings/llm-config", None).await;
    assert_eq!(d["config"]["global"]["timeout"], 300);
    assert_eq!(d["config"]["models"][0]["name"], "mock", "global PATCH 不影响 models");

    // 不存在的 id → 404
    let (s, _) = app.req(M2::PATCH, "/api/settings/llm-config/models/nope", Some(json!({}))).await;
    assert_eq!(s, 404);
}

/// URL 拼接锁定：请求必须命中 {base_url}/responses（mock 对其他路径回 405）
#[tokio::test]
async fn requests_hit_base_url_responses_path() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    put_llm_config(&app, &mock.base_url(), true, false, json!({})).await;

    let qid = setup_question(&app).await;
    mock.queue_nonstream(r#"{"tags":[],"difficulty":1,"ref_answer":"R"}"#);
    let (s, v) = app.req(Method::POST, &format!("/api/questions/{qid}/ref"), None).await;
    assert_eq!(s, 200, "{v}");
    common::wait_ai_job(&app, v["job_id"].as_u64().unwrap(), 5000).await;

    let paths = mock.request_paths();
    assert!(!paths.is_empty());
    assert!(
        paths.iter().all(|p| p.ends_with("/responses")),
        "全部请求应命中 {{base}}/responses，实际：{paths:?}"
    );
}

/// 严格网关（无 /responses 路由，回 405）：测试连接应给出可行动的错误提示
#[tokio::test]
async fn strict_gateway_405_surfaces_actionable_error() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let base = start_stub("405 Method Not Allowed", None).await;

    let (s, v) = app
        .req(
            Method::POST,
            "/api/settings/llm-config/test",
            Some(json!({ "base_url": base, "model": "any-model" })),
        )
        .await;
    assert_eq!(s, 400);
    let msg = v["error"].as_str().unwrap_or("");
    assert!(msg.contains("不支持 OpenAI Responses API"), "应提示协议不支持：{msg}");
    assert!(msg.contains("chat/completions 兼容网关"), "应点名兼容网关边界：{msg}");
}

/// http→https 重定向：POST 被降级为 GET 会报 405；应提示修正 base_url 而非假象
#[tokio::test]
async fn redirect_surfaces_https_hint() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let base = start_stub("301 Moved Permanently", Some("https://api.example.com/v1/responses")).await;

    let (s, v) = app
        .req(
            Method::POST,
            "/api/settings/llm-config/test",
            Some(json!({ "base_url": base, "model": "any-model" })),
        )
        .await;
    assert_eq!(s, 400);
    let msg = v["error"].as_str().unwrap_or("");
    assert!(msg.contains("重定向"), "应提示重定向原因：{msg}");
    assert!(msg.contains("https"), "应建议改用最终 https 地址：{msg}");
}

/// 连通性测试携带全部高级参数（reasoning_effort、temperature、extra_body）
#[tokio::test]
async fn test_connection_carries_full_parameters() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();

    mock.queue_nonstream(r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"pong"}]}]}"#);

    let (s, _) = app
        .req(
            Method::POST,
            "/api/settings/llm-config/test",
            Some(json!({
                "base_url": mock.base_url(),
                "model": "deepseek-r1",
                "reasoning_effort": "high",
                "temperature": 0.7,
                "extra_body": { "thinking_budget": 2048 }
            })),
        )
        .await;
    assert_eq!(s, 200);

    let bodies = mock.request_bodies();
    assert_eq!(bodies.len(), 1);
    let req = &bodies[0];
    assert_eq!(req["model"], "deepseek-r1");
    assert_eq!(req["reasoning"]["effort"], "high");
    assert_eq!(req["temperature"], 0.7);
    assert_eq!(req["extra_body"]["thinking_budget"], 2048, "额外参数应以字面 extra_body 字段嵌套下发");
}

/// Provider 与 Model 的微端点 CRUD 与级联删除自愈
#[tokio::test]
async fn test_provider_and_model_crud_and_cascade() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 1. POST 新建 Provider
    let (s, p_res) = app
        .req(
            Method::POST,
            "/api/settings/llm-config/providers",
            Some(json!({
                "name": "Custom Provider",
                "base_url": "https://custom.api.com/v1",
                "api_key": "sk-custom"
            })),
        )
        .await;
    assert_eq!(s, 200);
    let pid = p_res["id"].as_str().unwrap().to_string();

    // 2. POST 新建 Model
    let (s, m_res) = app
        .req(
            Method::POST,
            "/api/settings/llm-config/models",
            Some(json!({
                "provider_id": pid,
                "name": "custom-gpt",
                "advanced": { "reasoning_effort": "medium" }
            })),
        )
        .await;
    assert_eq!(s, 200);
    let mid = m_res["id"].as_str().unwrap().to_string();

    // 验证 doc 包含了新建的 provider 和 model，且 active_model_id 指向该 model
    let (_, doc_res) = app.req(Method::GET, "/api/settings/llm-config", None).await;
    let conf = &doc_res["config"];
    assert_eq!(conf["active_model_id"], mid);

    // 3. DELETE 删除 Provider，应级联删除其下的 model
    let (s, _) = app.req(Method::DELETE, &format!("/api/settings/llm-config/providers/{pid}"), None).await;
    assert_eq!(s, 200);

    let (_, doc_res2) = app.req(Method::GET, "/api/settings/llm-config", None).await;
    let conf2 = &doc_res2["config"];
    assert!(!conf2["providers"].as_array().unwrap().iter().any(|p| p["id"] == pid));
    assert!(!conf2["models"].as_array().unwrap().iter().any(|m| m["id"] == mid));
}
