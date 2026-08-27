//! v4 M5：ICS 日历订阅源（ADR-0011 R5：日历提醒 = ICS 订阅，不做邮件）。
//! - token 管理：GET /api/calendar/token（登录态懒生成、幂等）+ POST（重新生成吊销旧值）
//! - 订阅源：GET /api/calendar.ics?token=…（免 session，token 鉴权；per-user 行级隔离）
//!   内容 = 面试轮次（rounds.date：未来全部 + 过去30天）+ 复习到期（14天视野按天聚合，逾期并入今日）
//!   全天事件（VALUE=DATE）+ 稳定 UID + RFC 5545 文本转义。

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::{create_application, TestApp};
use serde_json::json;
use tower::ServiceExt;

async fn get_token(app: &TestApp) -> String {
    let (s, v) = app.req(Method::GET, "/api/calendar/token", None).await;
    assert_eq!(s, StatusCode::OK, "取日历 token 应成功");
    v["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn calendar_token_lazy_generate_and_regenerate() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 未登录 -> 401
    let saved = app.cookie.take();
    let (s, _) = app.req(Method::GET, "/api/calendar/token", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    app.cookie = saved;

    // 懒生成：32 字节 hex = 64 字符；再取幂等
    let t1 = get_token(&app).await;
    assert_eq!(t1.len(), 64);
    let t2 = get_token(&app).await;
    assert_eq!(t1, t2);

    // 重新生成 -> 换新值（旧订阅链接失效）
    let (s, v) = app.req(Method::POST, "/api/calendar/token", None).await;
    assert_eq!(s, StatusCode::OK);
    let t3 = v["token"].as_str().unwrap().to_string();
    assert_ne!(t1, t3);
    assert_eq!(t3, get_token(&app).await);
}

#[tokio::test]
async fn ics_requires_valid_token_and_serves_text_calendar() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 无 token / 错 token -> 401
    let (s, _) = app.req_raw(Method::GET, "/api/calendar.ics", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = app
        .req_raw(Method::GET, "/api/calendar.ics?token=deadbeef", None)
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // 有效 token -> 200 + text/calendar + 日历骨架
    let t = get_token(&app).await;
    let resp = app
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/calendar.ics?token={t}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/calendar"), "content-type 应为 text/calendar，实际 {ct}");
    let body = String::from_utf8_lossy(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap())
        .to_string();
    assert!(body.contains("BEGIN:VCALENDAR"));
    assert!(body.contains("VERSION:2.0"));
    assert!(body.contains("METHOD:PUBLISH"));
    assert!(body.contains("X-WR-CALNAME:求职工作台"));
    assert!(body.contains("END:VCALENDAR"));
    // RFC 5545 §3.1：每个物理行 ≤75 字节（超长折叠为续行）
    for line in body.split("\r\n") {
        assert!(line.len() <= 75, "ICS 行超长（{} bytes）：{}", line.len(), line);
    }
}

#[tokio::test]
async fn ics_contains_round_and_review_events() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 投递（公司名带逗号顺带验证转义）+ 未来一面（视频）+ 过去已通过二面
    let aid = create_application(&app, "Acme,Inc", "后端").await;
    let today = chrono::Local::now().date_naive();
    let future = today + chrono::Duration::days(7);
    let past = today - chrono::Duration::days(3);
    let (s, r1) = app
        .req(
            Method::POST,
            &format!("/api/applications/{aid}/rounds"),
            Some(json!({ "name": "一面", "date": future.to_string(), "form": "视频" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let rid1 = r1["id"].as_i64().unwrap();
    // 反馈七#2 校验：上一面需标记通过才能添加下一面
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/rounds/{rid1}"), Some(json!({ "passed": "pass" })))
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, r2) = app
        .req(
            Method::POST,
            &format!("/api/applications/{aid}/rounds"),
            Some(json!({ "name": "二面", "date": past.to_string() })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let rid2 = r2["id"].as_i64().unwrap();
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/rounds/{rid2}"), Some(json!({ "passed": "pass" })))
        .await;
    assert_eq!(s, StatusCode::OK);

    // 复习到期：一道明天到期、一道已逾期（应并入今日），各聚一条全天事件
    let mut qids = Vec::new();
    for content in ["什么是红黑树", "TCP 拥塞控制"] {
        let (s, q) = app
            .req(
                Method::POST,
                "/api/questions",
                Some(json!({ "round_id": rid1, "content": content })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED);
        qids.push(q["id"].as_i64().unwrap());
    }
    sqlx::query("INSERT INTO review_records(question_id, next_review_at) VALUES ($1, now() + interval '1 day')")
        .bind(qids[0])
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO review_records(question_id, next_review_at) VALUES ($1, now() - interval '2 days')")
        .bind(qids[1])
        .execute(&app.pool)
        .await
        .unwrap();

    let t = get_token(&app).await;
    let (s, body) = app
        .req_raw(Method::GET, &format!("/api/calendar.ics?token={t}"), None)
        .await;
    assert_eq!(s, StatusCode::OK);

    // 轮次事件：SUMMARY 转义逗号 + 通过状态后缀；全天 DATE；稳定 UID
    assert!(body.contains(r"SUMMARY:Acme\,Inc·后端·一面"), "未来一面 SUMMARY 缺失：{body}");
    assert!(body.contains(r"SUMMARY:Acme\,Inc·后端·二面 · 已通过"), "二面通过状态缺失");
    assert!(
        body.contains(&format!("DTSTART;VALUE=DATE:{}", future.format("%Y%m%d"))),
        "一面应为全天事件"
    );
    assert!(body.contains(&format!("UID:round-{rid1}@beview")));
    assert!(body.contains(&format!("UID:round-{rid2}@beview")));
    assert!(body.contains("DESCRIPTION:形式：视频"), "轮次形式应进 DESCRIPTION");

    // 复习事件：今日（逾期并入）与明日各一条
    assert!(
        body.contains(&format!("UID:review-{}@beview", today.format("%Y%m%d"))),
        "逾期复习应并入今日"
    );
    assert!(
        body.contains(&format!(
            "UID:review-{}@beview",
            (today + chrono::Duration::days(1)).format("%Y%m%d")
        )),
        "明日到期复习缺失"
    );
    assert_eq!(body.matches("SUMMARY:复习 1 张卡到期").count(), 2, "两天各聚合 1 张卡");

    // RFC 5545 行尾 CRLF
    assert!(body.contains("BEGIN:VEVENT\r\n"), "应以 CRLF 分行");
}

#[tokio::test]
async fn ics_feed_is_per_user() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 管理员的投递与轮次
    let aid = create_application(&app, "管理员公司", "后端").await;
    let (s, _) = app
        .req(
            Method::POST,
            &format!("/api/applications/{aid}/rounds"),
            Some(json!({ "name": "一面", "date": "2099-01-01" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);

    // 第二个用户 bob：自己的 token，feed 不含管理员数据
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "bob", "password": "bobpass123", "role": "user" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, bob_cookie) = app.login_as("bob", "bobpass123").await;
    assert_eq!(s, StatusCode::OK);
    let bob = bob_cookie.unwrap();

    let (s, v) = app.req_as(&bob, Method::GET, "/api/calendar/token", None).await;
    assert_eq!(s, StatusCode::OK);
    let bob_token = v["token"].as_str().unwrap().to_string();

    let (s, body) = app
        .req_raw(Method::GET, &format!("/api/calendar.ics?token={bob_token}"), None)
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(!body.contains("管理员公司"), "bob 的 feed 不应含管理员投递");
}

// ---------- GET /api/calendar/events（v4.2 M2：总览日历数据源，ADR-0015 D6） ----------
// 口径与 ICS 同源同窗：面试轮次（未来全部 + 近 30 天），session 鉴权；复习到期不进日历。

#[tokio::test]
async fn calendar_events_requires_login_and_serves_round_window() {
    let mut app = TestApp::setup().await;
    // 未登录 -> 401
    let (s, _) = app.req(Method::GET, "/api/calendar/events", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    app.setup_admin_and_login().await;
    // 空态：{"events":[]}
    let (s, v) = app.req(Method::GET, "/api/calendar/events", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["events"].as_array().unwrap().len(), 0);

    // 建带日期轮次：一面(近3天)→通过→二面(未来)；40天前的笔试放到另一份投递（窗外）
    // （反馈七#2 校验：上一面通过才能加下一面）
    let aid = create_application(&app, "星尘科技", "资深后端").await;
    let today = chrono::Local::now().date_naive();
    let future = (today + chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
    let recent = (today - chrono::Duration::days(3)).format("%Y-%m-%d").to_string();
    let stale = (today - chrono::Duration::days(40)).format("%Y-%m-%d").to_string();

    let (s, r1) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "一面", "date": recent })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "建一面应成功");
    app.req(Method::PATCH, &format!("/api/rounds/{}", r1["id"].as_i64().unwrap()), Some(json!({ "passed": "pass" }))).await;
    let (s, _) = app
        .req(Method::POST, &format!("/api/applications/{aid}/rounds"), Some(json!({ "name": "二面", "date": future })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "建二面应成功");
    let stale_aid = create_application(&app, "星尘科技", "资深后端").await;
    let (s, _) = app
        .req(Method::POST, &format!("/api/applications/{stale_aid}/rounds"), Some(json!({ "name": "笔试", "date": stale })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "建笔试(窗外)应成功");

    let (s, v) = app.req(Method::GET, "/api/calendar/events", None).await;
    assert_eq!(s, StatusCode::OK);
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "窗口=未来全部+近30天；40天前不入列");

    // 按日期升序：近3天的一面在前；字段齐全（含轮次 id 供跳详情）
    assert_eq!(events[0]["kind"], "round");
    assert!(events[0]["id"].is_i64(), "应携带轮次 id（日历卡跳转详情依赖）");
    assert_eq!(events[0]["name"], "一面");
    assert_eq!(events[0]["date"], json!(recent));
    assert_eq!(events[0]["passed"], "pass"); // 一面已通过（新增校验需要先通过才能建二面）
    assert_eq!(events[0]["company"], "星尘科技");
    assert_eq!(events[0]["position"], "资深后端");
    assert_eq!(events[1]["name"], "二面");
    assert_eq!(events[1]["date"], json!(future));

    // from/to 过滤：只取未来一周
    let to = (today + chrono::Duration::days(8)).format("%Y-%m-%d").to_string();
    let (s, v) = app
        .req(
            Method::GET,
            &format!("/api/calendar/events?from={future}&to={to}"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "from/to 过滤后只剩未来轮次");
    assert_eq!(events[0]["name"], "二面");
}
