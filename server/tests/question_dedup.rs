//! v5.5-M1（票02）：疑似重复题检测。
//! 归一化键（全角折叠/标点空白剥离/小写）相等即命中；录入响应附带提示、
//! 详情页双向徽章、跨用户隔离、编辑后重检。

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

mod common;

use common::TestApp;

async fn mk_question(app: &TestApp, rid: i64, content: &str) -> Value {
    let (s, body) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": content })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "创建失败: {body}");
    body
}

#[tokio::test]
async fn exact_duplicate_create_returns_hint() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = common::setup_application_round(&app).await;

    let first = mk_question(&app, rid, "请解释数据库 ACID 特性").await;
    assert!(first["duplicates"].as_array().unwrap().is_empty(), "首录不应有提示");

    let second = mk_question(&app, rid, "请解释数据库 ACID 特性").await;
    let dupes = second["duplicates"].as_array().unwrap();
    assert_eq!(dupes.len(), 1, "完全相同内容应命中: {second}");
    assert_eq!(dupes[0]["id"], first["id"]);
}

#[tokio::test]
async fn normalized_equivalent_triggers_hint() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = common::setup_application_round(&app).await;

    // 全角/半角、大小写、空白、标点差异——归一化后等价
    let first = mk_question(&app, rid, "Redis 持久化机制有哪些？").await;
    let second = mk_question(&app, rid, "ｒｅｄｉｓ　持久化机制有哪些!").await;
    let dupes = second["duplicates"].as_array().unwrap();
    assert_eq!(
        dupes.len(),
        1,
        "归一化等价内容应命中: first={first} second={second}"
    );
    assert_eq!(dupes[0]["id"], first["id"]);
}

#[tokio::test]
async fn distinct_content_no_false_positive() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = common::setup_application_round(&app).await;

    mk_question(&app, rid, "解释 TCP 三次握手过程").await;
    let other = mk_question(&app, rid, "解释 TCP 四次挥手过程").await;
    assert!(
        other["duplicates"].as_array().unwrap().is_empty(),
        "不同语义内容不得误报: {other}"
    );
}

#[tokio::test]
async fn detail_badge_is_bidirectional() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = common::setup_application_round(&app).await;

    let first = mk_question(&app, rid, "什么是 CAP 定理").await;
    let second = mk_question(&app, rid, "什么是 cap 定理").await;
    let id1 = first["id"].as_i64().unwrap();
    let id2 = second["id"].as_i64().unwrap();

    for (viewer, expected) in [(id1, id2), (id2, id1)] {
        let (s, detail) = app
            .req(Method::GET, &format!("/api/questions/{viewer}"), None)
            .await;
        assert_eq!(s, StatusCode::OK);
        let dupes = detail["duplicates"].as_array().unwrap();
        assert_eq!(dupes.len(), 1, "{viewer} 的详情应看到对方");
        assert_eq!(dupes[0]["id"], expected);
        assert!(!detail["duplicates"][0]["content"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn duplicates_are_row_level_isolated() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = common::setup_application_round(&app).await;
    mk_question(&app, rid, "用户 A 的独有题目：谈谈索引").await;

    // 用户 B 录入同样内容：不应看到 A 的题
    let (s, _) = app
        .req(
            Method::POST,
            "/api/admin/users",
            Some(json!({ "username": "dedupbob", "password": "bobpass123" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, bob_cookie) = app.login_as("dedupbob", "bobpass123").await;
    assert!(s.is_success());
    let cookie = bob_cookie.unwrap();

    let (s, r) = app
        .req_as(
            &cookie,
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "B 公司", "position": "后端" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let aid = r["id"].as_i64().unwrap();
    let (s, r) = app
        .req_as(
            &cookie,
            Method::POST,
            &format!("/api/applications/{aid}/rounds"),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let rid_b = r["id"].as_i64().unwrap();

    let created = {
        let (s, b) = app
            .req_as(
                &cookie,
                Method::POST,
                "/api/questions",
                Some(json!({ "round_id": rid_b, "content": "用户 A 的独有题目：谈谈索引" })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED);
        b
    };
    assert!(
        created["duplicates"].as_array().unwrap().is_empty(),
        "跨用户不得互相命中: {created}"
    );
}

#[tokio::test]
async fn patch_recheck_attaches_duplicates_on_content_change() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let (_, rid) = common::setup_application_round(&app).await;

    let a = mk_question(&app, rid, "讲讲操作系统进程与线程的区别").await;
    let b = mk_question(&app, rid, "随便什么别的内容").await;
    assert!(b["duplicates"].as_array().unwrap().is_empty());

    // 把 b 编辑成与 a 等价 → PATCH 响应应携带命中
    let id_b = b["id"].as_i64().unwrap();
    let (s, resp) = app
        .req(
            Method::PATCH,
            &format!("/api/questions/{id_b}"),
            Some(json!({ "content": "讲讲操作系统进程与线程的区别。" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let dupes = resp["duplicates"].as_array().unwrap();
    assert_eq!(dupes.len(), 1, "编辑为等价内容应在响应中提示: {resp}");
    assert_eq!(dupes[0]["id"], a["id"]);

    // 且详情徽章已生效（归一化列同步更新）
    let (_, detail) = app
        .req(Method::GET, &format!("/api/questions/{id_b}"), None)
        .await;
    assert_eq!(detail["duplicates"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn all_write_paths_populate_normalized_content() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = common::create_application(&app, "写入路径全覆盖公司", "测试架构师").await;
    let rid = common::create_round(&app, aid, "技术一面").await;

    // 1. rounds / retrospective / to-review 写入路径
    let (s, resp) = app
        .req(
            Method::POST,
            &format!("/api/rounds/{rid}/retrospective/to-review"),
            Some(json!({ "items": ["什么是垃圾回收机制？"] })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{resp}");

    // 2. positions / predict / ingest 押题流转写入路径
    let pos_id: i64 = sqlx::query_scalar("SELECT position_id FROM applications WHERE id=$1")
        .bind(aid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let (s, ingest_resp) = app
        .req(
            Method::POST,
            &format!("/api/positions/{pos_id}/predict/ingest"),
            Some(json!({
                "questions": [{
                    "content": "分布式事务两阶段提交与三阶段提交的区别？",
                    "category": "分布式系统",
                    "focus_points": ["2PC", "3PC"],
                    "sample_direction": "对比阻塞问题与单点故障",
                    "probability": 90
                }]
            })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{ingest_resp}");

    // 3. 验证 DB 中所有插入的题目均具有非空的 content_normalized
    let unnormalized_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM questions WHERE content_normalized IS NULL",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(unnormalized_count, 0, "所有题目写入路径均必须回填 content_normalized");

    // 4. 再次通过主入口录入等价题目，应命中之前通过 to-review 与 predict/ingest 入库的题目
    let second_retro = mk_question(&app, rid, "什么是垃圾回收机制").await;
    let dupes_retro = second_retro["duplicates"].as_array().unwrap();
    assert_eq!(dupes_retro.len(), 1, "应命中跨 to-review 路径入库的题目");

    let second_predict = mk_question(&app, rid, "分布式事务两阶段提交与三阶段提交的区别!").await;
    let dupes_predict = second_predict["duplicates"].as_array().unwrap();
    assert_eq!(dupes_predict.len(), 1, "应命中跨 predict/ingest 路径入库的题目");
}
