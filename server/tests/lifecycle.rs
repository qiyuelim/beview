//! ADR-0014 投递生命周期与题目归属（TDD 矩阵 §24）：
//! - Forward-Only 状态机全套 + 手工制造 interviewing 拒绝
//! - Offer：唯一入口守卫（0 轮拒）/ pending 补标发分 / pass·fail 不动 / 重复 Offer 幂等
//! - 批量：batch-status 局部成功；batch-delete 全量预校验
//! - 墓碑：删除投递后题目存活且挂墓碑轮次，application 消失
//! - 系统公司排除：看板/companies 列表默认不含；题库可筛回收站/自录题库

use axum::http::Method;
use serde_json::json;

mod common;
use common::{create_application, create_round, TestApp};
use server::services::system_containers;

async fn balance(app: &TestApp) -> i64 {
    let (_, v) = app.req(Method::GET, "/api/points/balance", None).await;
    v["balance"].as_i64().unwrap()
}

async fn admin_uid(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
        .fetch_one(&app.pool)
        .await
        .unwrap()
}

/// ---------- 状态机 ----------

#[tokio::test]
async fn state_machine_full_matrix() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // applied → interviewing 仅 auto：手工拒绝
    let a1 = create_application(&app, "状态机公司", "后端").await;
    let (s, e) = app
        .req(Method::PATCH, &format!("/api/applications/{a1}"), Some(json!({ "status": "interviewing" })))
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("自动推进"));

    // applied → interviewing 经添加首场面试 ✓（自动）
    let (s, _) = app.req(Method::POST, &format!("/api/applications/{a1}/rounds"), Some(json!({}))).await;
    assert_eq!(s, 201);
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{a1}"), None).await;
    assert_eq!(d["application"]["status"], "interviewing");

    // applied → rejected / withdrawn 合法
    let a2 = create_application(&app, "拒公司", "后端").await;
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{a2}"), Some(json!({ "status": "rejected" })))
        .await;
    assert_eq!(s, 200, "未面即拒应合法（round_count=0）");
    let a3 = create_application(&app, "弃公司", "后端").await;
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{a3}"), Some(json!({ "status": "withdrawn" })))
        .await;
    assert_eq!(s, 200);

    // interviewing → offer 合法（≥1 轮）
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{a1}"), Some(json!({ "status": "offer" })))
        .await;
    assert_eq!(s, 200);

    // 终态禁一切流转
    for to in ["applied", "interviewing", "rejected", "withdrawn"] {
        let (s, e) = app
            .req(Method::PATCH, &format!("/api/applications/{a1}"), Some(json!({ "status": to })))
            .await;
        assert_eq!(s, 400, "offer → {to} 应被拒");
        assert!(e["error"].as_str().unwrap().contains("非法流转"));
    }

    // withdrawn 终态同样禁流转（§6：不存在终态 → withdrawn）
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{a3}"), Some(json!({ "status": "applied" })))
        .await;
    assert_eq!(s, 400, "withdrawn 终态禁流转");
}

/// ---------- Offer ----------

#[tokio::test]
async fn offer_guards_promotes_pending_and_is_idempotent() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    // 0 轮 → offer 拒绝（interviewing 直建，避开 applied→offer 的 forward-only 拒绝）
    let (_, a0v) = app
        .req(
            Method::POST,
            "/api/applications",
            Some(json!({ "company_name": "零轮公司", "position": "后端", "status": "interviewing" })),
        )
        .await;
    let a0 = a0v["id"].as_i64().unwrap();
    let (s, e) = app
        .req(Method::PATCH, &format!("/api/applications/{a0}"), Some(json!({ "status": "offer" })))
        .await;
    assert_eq!(s, 400);
    assert!(e["error"].as_str().unwrap().contains("面试轮次"), "实际错误: {}", e);

    // 真实序列（反馈七#2 校验生效）：一面通过 → 才能加二面（pending）→ 直接触发 Offer
    let aid = create_application(&app, "补标公司", "后端").await;
    let rid1 = create_round(&app, aid, "一面").await;
    app.req(Method::PATCH, &format!("/api/rounds/{rid1}"), Some(json!({ "passed": "pass" }))).await;
    let rid2 = create_round(&app, aid, "二面").await; // 此时二面为 pending（最新轮）
    let base = balance(&app).await;

    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{aid}"), Some(json!({ "status": "offer" })))
        .await;
    assert_eq!(s, 200);
    // 仅 pending 的二面被补标 → +200（一面的 +200 在确认流已发）
    assert_eq!(balance(&app).await, base + 200 + 10000, "仅 pending→pass 发分 + 首Offer里程碑");

    // 补标落库：两面全 pass
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    let rounds = d["rounds"].as_array().unwrap();
    assert_eq!(rounds.iter().filter(|r| r["passed"] == "pass").count(), 2);

    // 重复 Offer 幂等：不再加分、不再补流水
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{aid}"), Some(json!({ "status": "offer" })))
        .await;
    assert_eq!(s, 200, "重复 Offer 同态幂等应成功");
    assert_eq!(balance(&app).await, base + 200 + 10000, "重复 Offer 不重复发分/发里程碑");
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    let offer_events = d["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["to_status"] == "offer")
        .count();
    assert_eq!(offer_events, 1, "重复 Offer 不重复补流水");

    // fail 轮次不被 Offer 修改：一面未过 → 面试中直接给 offer（业务上少见但合法），fail 保持
    let aid2 = create_application(&app, "失败保留公司", "后端").await;
    let r = create_round(&app, aid2, "一面").await;
    app.req(Method::PATCH, &format!("/api/rounds/{r}"), Some(json!({ "passed": "fail" }))).await;
    let b2 = balance(&app).await;
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{aid2}"), Some(json!({ "status": "offer" })))
        .await;
    assert_eq!(s, 200);
    let (_, d) = app.req(Method::GET, &format!("/api/applications/{aid2}"), None).await;
    assert_eq!(d["rounds"][0]["passed"], "fail", "fail 不被补标");
    assert_eq!(balance(&app).await - b2, 0, "无 pending 可补不发分；首Offer里程碑全局一次已在前一发过");
}

/// ---------- 批量 ----------

#[tokio::test]
async fn batch_status_partial_success_and_delete_prevalidation() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let ok1 = create_application(&app, "批量A", "后端").await;
    let ok2 = create_application(&app, "批量B", "后端").await;
    let terminal = create_application(&app, "批量C", "后端").await;
    app.req(Method::PATCH, &format!("/api/applications/{terminal}"), Some(json!({ "status": "withdrawn" }))).await;

    // 局部成功：合法执行、终态跳过
    let (s, v) = app
        .req(
            Method::POST,
            "/api/applications/batch-status",
            Some(json!({ "ids": [ok1, terminal, ok2], "status": "rejected" })),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["succeeded"].as_array().unwrap().len(), 2);
    assert_eq!(v["failed"].as_array().unwrap().len(), 1);
    assert_eq!(v["failed"][0]["id"].as_i64(), Some(terminal));

    // offer 不能批量直达（唯一入口在详情页）
    let (s, e) = app
        .req(
            Method::POST,
            "/api/applications/batch-status",
            Some(json!({ "ids": [ok1], "status": "offer" })),
        )
        .await;
    assert_eq!(s, 400, "Offer 不能批量设置");
    assert!(e["error"].as_str().unwrap().contains("Offer"));

    // batch-delete 全量预校验：含不存在 ID → 整体拒绝
    let (s, _) = app
        .req(
            Method::POST,
            "/api/applications/batch-delete",
            Some(json!({ "ids": [ok2, 999999] })),
        )
        .await;
    assert_eq!(s, 400, "预校验失败应整体拒绝");
    let (_, d) = app.req(Method::GET, "/api/applications", None).await;
    assert!(
        d.as_array().unwrap().iter().any(|a| a["id"].as_i64() == Some(ok2)),
        "预校验失败不应删除任何投递"
    );

    // 全部合法 → 删除成功
    let (s, v) = app
        .req(Method::POST, "/api/applications/batch-delete", Some(json!({ "ids": [ok1, ok2] })))
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["deleted"], 2);
    let (_, d) = app.req(Method::GET, "/api/applications", None).await;
    assert!(!d.as_array().unwrap().iter().any(|a| a["id"].as_i64() == Some(ok1)));
}

/// ---------- 墓碑 ----------

#[tokio::test]
async fn delete_application_moves_questions_to_tombstone() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;

    let aid = create_application(&app, "墓碑公司", "后端").await;
    let rid = create_round(&app, aid, "一面").await;
    let (_, q) = app
        .req(
            Method::POST,
            "/api/questions",
            Some(json!({ "round_id": rid, "content": "讲一下 Redis 持久化" })),
        )
        .await;
    let qid = q["id"].as_i64().unwrap();

    let (s, _) = app.req(Method::DELETE, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(s, 200);

    // application 消失
    let (s, _) = app.req(Method::GET, &format!("/api/applications/{aid}"), None).await;
    assert_eq!(s, 404);

    // 题目存活且公司显示回收站（挂墓碑轮次）
    let (_, all) = app.req(Method::GET, "/api/questions", None).await;
    let row = all
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["id"].as_i64() == Some(qid))
        .expect("题目应在删除投递后仍然存在");
    assert_eq!(row["content"], "讲一下 Redis 持久化");
    assert_eq!(row["company"], "回收站", "题目应挂靠回收站墓碑轮次");

    // 题目 round_id 已指向墓碑轮次
    let tombstone: i64 =
        sqlx::query_scalar("SELECT q.round_id FROM questions q WHERE q.id=$1").bind(qid).fetch_one(&app.pool).await.unwrap();
    let round_name: String =
        sqlx::query_scalar("SELECT name FROM rounds WHERE id=$1").bind(tombstone).fetch_one(&app.pool).await.unwrap();
    assert_eq!(round_name, "已删除投递");

    // 复习统计链路（经 questions join）在投递删除后仍正常工作
    let (s, _) = app.req(Method::GET, "/api/review/stats", None).await;
    assert_eq!(s, 200, "复习统计不应因投递删除而失败");

}

/// ---------- 系统容器与排除 ----------

#[tokio::test]
async fn system_containers_excluded_from_board_but_visible_in_question_bank() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let uid = admin_uid(&app).await;

    // 真实投递 + 真题
    let aid = create_application(&app, "自录公司", "后端").await;
    let rid = create_round(&app, aid, "一面").await;
    app.req(Method::POST, "/api/questions", Some(json!({ "round_id": rid, "content": "真实面试题" }))).await;

    // 系统容器：ensure 回收站 + 自录题库（懒创建）
    system_containers::ensure_tombstone_round(&app.pool, uid).await.unwrap();
    let self_round = system_containers::ensure_self_round(&app.pool, uid).await.unwrap();
    app.req(
        Method::POST,
        "/api/questions",
        Some(json!({ "round_id": self_round, "content": "自己搜罗的模拟题", "tags": ["自录"] })),
    )
    .await;

    // companies 默认列表不含系统公司；include_system=true 含
    let (_, cs) = app.req(Method::GET, "/api/companies", None).await;
    let names: Vec<&str> = cs.as_array().unwrap().iter().filter_map(|c| c["name"].as_str()).collect();
    assert!(!names.contains(&"回收站"), "公司列表默认不应出现回收站");
    assert!(!names.contains(&"自录题库"));
    assert!(!names.contains(&"模拟面试"));
    let (_, cs_all) = app.req(Method::GET, "/api/companies?include_system=true", None).await;
    let names_all: Vec<&str> = cs_all.as_array().unwrap().iter().filter_map(|c| c["name"].as_str()).collect();
    assert!(names_all.contains(&"回收站") && names_all.contains(&"自录题库"));

    // 看板不含系统投递（只有真实投递）
    let (_, apps) = app.req(Method::GET, "/api/applications", None).await;
    assert_eq!(apps.as_array().unwrap().len(), 1, "系统容器投递不进看板: {}", serde_json::to_string(&apps).unwrap());

    // 题库两题都在：真投递的 + 自录的（可展示可筛）
    let (_, qs) = app.req(Method::GET, "/api/questions", None).await;
    assert_eq!(qs.as_array().unwrap().len(), 2);
    let self_row = qs
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["content"] == "自己搜罗的模拟题")
        .expect("自录题应可见");
    assert_eq!(self_row["company"], "自录题库");

    // 系统容器投递不可经业务 API 触达（写保护）
    let tomb_app: i64 = sqlx::query_scalar(
        "SELECT a.id FROM applications a JOIN positions p ON p.id=a.position_id
         JOIN companies c ON c.id=p.company_id WHERE c.is_system ORDER BY a.id LIMIT 1",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let (s, _) = app
        .req(Method::PATCH, &format!("/api/applications/{tomb_app}"), Some(json!({ "note": "hack" })))
        .await;
    assert_eq!(s, 400, "系统容器投递应写保护");
    let (s, _) = app.req(Method::DELETE, &format!("/api/applications/{tomb_app}"), None).await;
    assert_eq!(s, 400, "系统容器投递不可删除");
}
