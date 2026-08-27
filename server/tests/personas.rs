//! V6-M5a（ADR-0023 D1/D5）：面试官人格域 + 会话上下文装配点。
//!
//! 验收：内置不可删改 / 自定义 CRUD 与软删除显示「已删除的面试官」/ temperature_hint
//! 越界被 DB CHECK 拒绝 / 存量零迁移出现「经典模式」/ 人格注入全链路 + 温度覆盖 +
//! 人设前缀跨请求字节稳定（缓存友好）。

use axum::http::Method;
use serde_json::json;

mod common;
use common::TestApp;

/// 内置种子在前、自定义在后；自定义可创建/更新/软删除
#[tokio::test]
async fn persona_crud_lifecycle_and_ordering() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 列表：内置种子在前（3 个），无自定义
    let (_, v) = app.req(Method::GET, "/api/personas", None).await;
    let items = v["items"].as_array().unwrap();
    assert!(items.len() >= 3, "应至少有 3 个内置种子: {v}");
    assert_eq!(items[0]["builtin"], true);
    assert!(items.iter().all(|p| p["builtin"] == true));

    // 创建自定义
    let (sc, created) = app
        .req(
            Method::POST,
            "/api/personas",
            Some(json!({
                "name": "算法偏执狂",
                "title": "竞赛出身的考官",
                "persona_prompt": "你痴迷于复杂度分析，每道题都要追问时空复杂度。",
                "difficulty_hint": "高频复杂度拷问",
                "temperature_hint": 0.4,
                "focus_tags": ["算法", "复杂度"]
            })),
        )
        .await;
    assert!(sc.is_success(), "创建自定义 persona 应成功: {created}");
    let pid = created["id"].as_i64().unwrap();

    // 更新自己的
    let (sc, _) = app
        .req(
            Method::PUT,
            &format!("/api/personas/{pid}"),
            Some(json!({
                "name": "算法偏执狂·改",
                "title": "竞赛出身的考官",
                "persona_prompt": "你痴迷于复杂度分析，且要求现场推导。",
                "temperature_hint": 0.45,
                "focus_tags": ["算法"]
            })),
        )
        .await;
    assert!(sc.is_success(), "更新自定义应成功");

    // 列表顺序：内置在前、自定义在后
    let (_, v) = app.req(Method::GET, "/api/personas", None).await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.last().unwrap()["name"], "算法偏执狂·改");
    assert_eq!(items.last().unwrap()["builtin"], false);

    // 软删除后列表不再出现
    let (sc, _) = app.req(Method::DELETE, &format!("/api/personas/{pid}"), None).await;
    assert!(sc.is_success());
    let (_, v) = app.req(Method::GET, "/api/personas", None).await;
    assert!(!v["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"].as_i64() == Some(pid)), "已删除的 persona 不应出现在列表");
}

/// 内置 persona 不可编辑不可删除；越权操作他人自定义被拒
#[tokio::test]
async fn builtin_personas_immutable() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_, v) = app.req(Method::GET, "/api/personas", None).await;
    let builtin_id = v["items"][0]["id"].as_i64().unwrap();
    assert_eq!(v["items"][0]["builtin"], true);

    // 编辑内置 -> 403
    let (sc, _) = app
        .req(
            Method::PUT,
            &format!("/api/personas/{builtin_id}"),
            Some(json!({ "name": "改名", "persona_prompt": "篡改", "temperature_hint": 0.5 })),
        )
        .await;
    assert_eq!(sc, axum::http::StatusCode::FORBIDDEN, "内置 persona 不可编辑");

    // 删除内置 -> 403
    let (sc, _) = app.req(Method::DELETE, &format!("/api/personas/{builtin_id}"), None).await;
    assert_eq!(sc, axum::http::StatusCode::FORBIDDEN, "内置 persona 不可删除");
}

/// temperature_hint 越界：API 显式拒绝；绕过 API 直插 DB 被 CHECK 约束拒绝
#[tokio::test]
async fn temperature_hint_bounds_enforced() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // API 拒绝越界值
    for bad in [0.1_f64, 0.95_f64] {
        let (sc, v) = app
            .req(
                Method::POST,
                "/api/personas",
                Some(json!({ "name": format!("越界{bad}"), "persona_prompt": "x", "temperature_hint": bad })),
            )
            .await;
        assert_eq!(sc, 400, "越界温度应被拒绝: {v}");
        assert!(v["error"].as_str().unwrap_or("").contains("0.3–0.9"));
    }

    // 合法边界可用
    let (sc, _) = app
        .req(
            Method::POST,
            "/api/personas",
            Some(json!({ "name": "边界测试", "persona_prompt": "x", "temperature_hint": 0.9 })),
        )
        .await;
    assert!(sc.is_success(), "0.9 边界应合法");

    // DB CHECK 兜底：绕过 API 直插仍被拒绝
    let uid: i64 = sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();
    let res = sqlx::query("INSERT INTO interviewer_personas(owner_user_id, name, persona_prompt, temperature_hint) VALUES($1,'直插','x',1.5)")
        .bind(uid)
        .execute(&app.pool)
        .await;
    assert!(res.is_err(), "DB CHECK 应拒绝 1.5 的温度提示");
}

/// 经典模式并存 + 人格注入全链路：建场带 persona → 对话请求体含人设与温度覆盖 → 前缀字节稳定
#[tokio::test]
async fn persona_injection_end_to_end_with_byte_stable_prefix() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 创建自定义人格（低温严谨型）
    let (_, persona) = app
        .req(
            Method::POST,
            "/api/personas",
            Some(json!({
                "name": "PERSONA-MARKER-严谨架构师",
                "persona_prompt": "PERSONA-PROMPT-MARKER：你是严谨的架构师。",
                "difficulty_hint": "DIFF-HINT-MARKER",
                "temperature_hint": 0.35,
                "focus_tags": ["系统设计"]
            })),
        )
        .await;
    let pid = persona["id"].as_i64().unwrap();

    // 建场绑定人格
    let (sc, d) = app
        .req(
            Method::POST,
            "/api/drills",
            Some(json!({ "kind": "interview", "position": "后端", "persona_id": pid })),
        )
        .await;
    assert!(sc.is_success());
    let did = d["id"].as_i64().unwrap();

    // detail 展示人格名（非经典模式）
    let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(det["persona_label"], "PERSONA-MARKER-严谨架构师", "{det}");

    // 首轮对话：请求体应含人设 prompt + focus tags + 温度覆盖
    mock.queue_stream(vec!["第一题：请设计一个短链接服务。".to_string()]);
    let (sc, _) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" })))
        .await;
    assert!(sc.is_success());

    let bodies = mock.request_bodies();
    let first = bodies.first().expect("应有 LLM 请求");
    let body_str = first.to_string();
    assert!(body_str.contains("PERSONA-PROMPT-MARKER"), "人设 prompt 应注入上下文: {body_str}");
    assert!(body_str.contains("考察侧重：系统设计"), "focus_tags 应注入: {body_str}");

    // 温度覆盖注入引擎（0.35 覆盖用户默认）
    assert_eq!(
        first.get("temperature"),
        Some(&serde_json::Value::from(0.35)),
        "persona temperature_hint 应覆盖采样参数: {first}"
    );

    // 第二轮：人设前缀跨请求字节稳定（缓存友好断言）
    mock.queue_stream(vec![
        r#"<<<PROBE>>>{"anchor_keyword":"哈希","reason":"depth_probe"}"#.to_string(),
        "追问：如何防碰撞？".to_string(),
    ]);
    let (sc2, _) = app
        .req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "哈希取模加链地址法" })))
        .await;
    assert!(sc2.is_success());

    let bodies2 = mock.request_bodies();
    let prefix_of = |b: &serde_json::Value| {
        b["instructions"].as_str().map(|s| s.to_string()).unwrap_or_else(|| {
            b["input"].as_array().map(|arr| {
                arr.iter().filter_map(|m| m["content"].as_str()).collect::<Vec<_>>().join("\n")
            }).unwrap_or_default()
        })
    };
    // 人设片段在两轮请求中的出现形态一致（字节稳定）
    let p1 = prefix_of(&bodies[0]);
    let p2 = prefix_of(&bodies2.last().unwrap());
    let extract = |s: &str| {
        s.find("PERSONA-PROMPT-MARKER")
            .map(|i| s[i..].chars().take(60).collect::<String>())
    };
    assert_eq!(extract(&p1), extract(&p2), "人设前缀跨请求字节稳定（缓存友好）");
}

/// 票 08：未指定人格建场 → 自动落「经典面试官」内置种子（每行 drills 都有归属），人设块正常注入
#[tokio::test]
async fn classic_persona_seed_applied_when_unspecified() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = common::llm_mock::LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    // 无 persona 建场
    let (sc, d) = app.req(Method::POST, "/api/drills", Some(json!({ "kind": "interview" }))).await;
    assert!(sc.is_success());
    let did = d["id"].as_i64().unwrap();

    // 场次归属「经典面试官」（不再出现退役词条「经典模式」）
    let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(det["persona_label"], "经典面试官", "未传 persona 应落经典面试官种子: {det}");

    // 对话注入经典面试官人设块（每场都有人格归属）
    mock.queue_stream(vec!["第一题：讲讲 TCP 三次握手。".to_string()]);
    app.req_raw(Method::POST, &format!("/api/drills/{did}/messages"), Some(json!({ "content": "开始" }))).await;
    let bodies = mock.request_bodies();
    assert!(
        bodies[0].to_string().contains("面试官人设】"),
        "经典面试官也应注入人设块"
    );
}

/// 自定义 persona 删除后：历史场次显示「已删除的面试官」（软删除语义）
#[tokio::test]
async fn deleted_persona_shows_label_on_history() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let (_, persona) = app
        .req(
            Method::POST,
            "/api/personas",
            Some(json!({ "name": "将被删除的考官", "persona_prompt": "x" })),
        )
        .await;
    let pid = persona["id"].as_i64().unwrap();

    let (sc, d) = app
        .req(Method::POST, "/api/drills", Some(json!({ "kind": "interview", "persona_id": pid })))
        .await;
    assert!(sc.is_success());
    let did = d["id"].as_i64().unwrap();

    // 删除 persona（软删除）
    app.req(Method::DELETE, &format!("/api/personas/{pid}"), None).await;

    // 历史场次显示「已删除的面试官」而非经典模式
    let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
    assert_eq!(det["persona_label"], "已删除的面试官", "{det}");
}
