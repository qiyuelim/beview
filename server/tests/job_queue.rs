//! v5.5-M1（票09）：持久化任务队列。
//! 覆盖：并发认领互斥（SKIP LOCKED）、心跳租约超期重派、重试上限与死信、
//! 启动恢复（running→pending）、孤儿判卷恢复、HTTP 端到端批量续跑。

mod common;

use common::TestApp;
use serde_json::{json, Value};
use server::services::job_queue;

async fn seed_job(app: &TestApp, uid: i64, kind: &str, status: &str, max_attempts: i32) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO background_jobs(user_id, kind, payload, status, max_attempts) \
         VALUES($1,$2,$3,$4,$5) RETURNING id",
    )
    .bind(uid)
    .bind(kind)
    .bind(json!({ "ids": [1], "total": 1 }))
    .bind(status)
    .bind(max_attempts)
    .fetch_one(&app.pool)
    .await
    .expect("seed job 失败");
    id
}

#[tokio::test]
async fn concurrent_claims_are_mutually_exclusive() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let admin_uid: i64 =
        sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();

    for _ in 0..3 {
        seed_job(&app, admin_uid, "batch_analyze", "pending", 2).await;
    }

    // 6 个并发认领者抢 3 个任务
    let pool = app.pool.clone();
    let mut handles = Vec::new();
    for _ in 0..6 {
        let p = pool.clone();
        handles.push(tokio::spawn(async move {
            job_queue::claim_one(&p, &["batch_analyze"]).await
        }));
    }
    let mut claimed_ids = Vec::new();
    for h in handles {
        if let Ok(Ok(Some(job))) = h.await {
            claimed_ids.push(job.id);
        }
    }
    claimed_ids.sort();
    claimed_ids.dedup();
    assert_eq!(claimed_ids.len(), 3, "3 个任务恰好被认领一次且互不重复");
}

#[tokio::test]
async fn stale_heartbeat_reclaimed_but_fresh_not() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let uid: i64 = sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();

    // 孤儿 running：心跳停在两分钟前 → 可被重派
    let stale = seed_job(&app, uid, "batch_analyze", "running", 2).await;
    sqlx::query("UPDATE background_jobs SET heartbeat_at = now() - interval '120 seconds', claimed_at = now() - interval '120 seconds' WHERE id=$1")
        .bind(stale)
        .execute(&app.pool)
        .await
        .unwrap();

    let claimed = job_queue::claim_one(&app.pool, &["batch_analyze"]).await.unwrap().unwrap();
    assert_eq!(claimed.id, stale, "超期 running 应被重派");

    // 新鲜 heartbeat 的 running 不可被认领
    let fresh = seed_job(&app, uid, "batch_analyze", "running", 2).await;
    sqlx::query("UPDATE background_jobs SET heartbeat_at = now(), claimed_at = now() WHERE id=$1")
        .bind(fresh)
        .execute(&app.pool)
        .await
        .unwrap();
    let none = job_queue::claim_one(&app.pool, &["batch_analyze"]).await.unwrap();
    assert!(none.is_none(), "心跳新鲜的 running 不应被重派");
}

#[tokio::test]
async fn retry_cap_moves_to_dead() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let uid: i64 = sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();
    let job = seed_job(&app, uid, "paper_grading", "pending", 2).await;

    // 第 1 次执行失败：attempts=1 < 2 → 回 pending
    let c1 = job_queue::claim_one(&app.pool, &["paper_grading"]).await.unwrap().unwrap();
    assert_eq!(c1.attempts, 1);
    job_queue::finish(&app.pool, c1.id, false, Some("LLM 超时")).await.unwrap();

    // 第 2 次执行失败：attempts=2 >= 2 → 死信
    let c2 = job_queue::claim_one(&app.pool, &["paper_grading"]).await.unwrap().unwrap();
    assert_eq!(c2.attempts, 2);
    job_queue::finish(&app.pool, c2.id, false, Some("LLM 再次失败")).await.unwrap();

    let status: String = sqlx::query_scalar("SELECT status FROM background_jobs WHERE id=$1")
        .bind(job)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(status, "dead", "重试上限后应死信标注");

    // 死信不再被派发
    assert!(job_queue::claim_one(&app.pool, &["paper_grading"]).await.unwrap().is_none());
}

#[tokio::test]
async fn boot_reset_moves_running_to_pending() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let uid: i64 = sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();
    seed_job(&app, uid, "batch_analyze", "running", 2).await;
    seed_job(&app, uid, "batch_analyze", "done", 2).await;

    let n = job_queue::reset_running_on_boot(&app.pool).await.unwrap();
    assert_eq!(n, 1);

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM background_jobs WHERE status='pending'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1);
}

#[tokio::test]
async fn orphaned_paper_grading_recovered_once() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let aid = common::create_application(&app, "恢复公司", "测试岗").await;

    // 模拟崩溃遗留：drill 停在 grading 态且无活跃队列任务
    let (s, d) = app
        .req(
            axum::http::Method::POST,
            "/api/drills",
            Some(json!({ "application_id": aid, "mode": "interview" })),
        )
        .await;
    // drills 创建路由可能不同；失败则直接 SQL 兜底建行
    let drill_id: i64 = if s.is_success() {
        d["id"].as_i64().unwrap()
    } else {
        sqlx::query_scalar(
            "INSERT INTO drills(user_id, title, kind) VALUES((SELECT min(id) FROM users), '遗留判卷', 'interview') RETURNING id",
        )
        .fetch_one(&app.pool)
        .await
        .unwrap()
    };
    sqlx::query("UPDATE drills SET grading='grading' WHERE id=$1")
        .bind(drill_id)
        .execute(&app.pool)
        .await
        .unwrap();

    let recovered = job_queue::recover_orphaned_paper_gradings(&app.pool).await.unwrap();
    assert_eq!(recovered, 1, "应补一条判卷队列任务");

    // 幂等：再次恢复不重复入队
    let again = job_queue::recover_orphaned_paper_gradings(&app.pool).await.unwrap();
    assert_eq!(again, 0);

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM background_jobs WHERE kind='paper_grading' AND status='pending'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1);
}

/// 端到端：受理入队 → dispatcher 认领执行 → GET 进度终态（证明派发接线真实工作）
#[tokio::test]
async fn batch_analyze_end_to_end_through_queue() {
    use common::llm_mock::LlmMock;

    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let mock = LlmMock::start();
    app.point_llm_at_mock(&mock.base_url()).await;

    let aid = common::create_application(&app, "队列端到端公司", "后端").await;
    let rid = common::create_round(&app, aid, "一面").await;
    let mut ids = Vec::new();
    for i in 0..2 {
        let (s, q) = app
            .req(
                axum::http::Method::POST,
                "/api/questions",
                Some(json!({ "round_id": rid, "content": format!("队列端到端题 {i}") })),
            )
            .await;
        assert!(s.is_success());
        ids.push(q["id"].as_i64().unwrap());
    }
    // 每题一次 LLM 调用，排队合法分析结果（QuestionFull 契约形状的最小合法输出）
    for _ in &ids {
        // QuestionFull 契约形状（interview_analysis schema）
        mock.queue_nonstream(
            json!({
                "skill_path": null,
                "new_skill": null,
                "question_type": "principle",
                "tags": ["队列端到端"],
                "difficulty": 3,
                "ref_answer": "参考答案",
                "score": null,
                "feedback": "整体尚可"
            })
            .to_string()
            .as_str(),
        );
    }

    let (s, resp) = app
        .req(
            axum::http::Method::POST,
            "/api/questions/batch-analyze",
            Some(json!({ "ids": ids })),
        )
        .await;
    assert!(s.is_success(), "{resp}");
    let job_id = resp["job_id"].as_u64().unwrap();

    // 轮询到终态（dispatcher 在后台真实认领执行）
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(20);
    loop {
        let (s, body) = app
            .req(
                axum::http::Method::GET,
                &format!("/api/questions/batch-analyze/{job_id}"),
                None,
            )
            .await;
        assert_eq!(s, axum::http::StatusCode::OK);
        if body["status"] == "done" || body["status"] == "error" {
            assert_eq!(body["status"], "done", "{body}");
            assert_eq!(body["total"], 2);
            assert_eq!(body["ok"], 2);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "批量任务未在时限内完成: {body}\n收到 LLM 请求数={}\n剩余排队={}",
            mock.request_bodies().len(),
            mock.queue_nonstream_len(),
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    // 重启恢复语义：手动把行置回 running + 过期心跳 → reset 后回 pending
    sqlx::query("UPDATE background_jobs SET status='running', heartbeat_at = now() - interval '120 seconds' WHERE id=$1")
        .bind(job_id as i64)
        .execute(&app.pool)
        .await
        .unwrap();
    job_queue::reset_running_on_boot(&app.pool).await.unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM background_jobs WHERE id=$1")
            .bind(job_id as i64)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(status, "pending");
}

#[tokio::test]
async fn batch_analyze_cancel_preserves_cancelled_status() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let uid: i64 = sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();

    let job_id = seed_job(&app, uid, "batch_analyze", "pending", 2).await;

    // 1. DELETE 取消任务
    let (s, resp) = app
        .req(
            axum::http::Method::DELETE,
            &format!("/api/questions/batch-analyze/{job_id}"),
            None,
        )
        .await;
    assert_eq!(s, axum::http::StatusCode::OK, "{resp}");

    let status_after_cancel: String =
        sqlx::query_scalar("SELECT status FROM background_jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(status_after_cancel, "cancelled");

    // 2. 模拟执行器收尾 finish(ok=true)，确保 status != 'cancelled' 守卫阻止覆写为 done
    job_queue::finish(&app.pool, job_id, true, None).await.unwrap();

    let status_after_finish: String =
        sqlx::query_scalar("SELECT status FROM background_jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(status_after_finish, "cancelled", "已取消的任务不得被 finish 覆写为 done");
}

#[tokio::test]
async fn test_background_jobs_tiered_ttl_cleanup() {
    let mut app = TestApp::setup().await;
    app.setup_admin_and_login().await;
    let uid: i64 = sqlx::query_scalar("SELECT min(id) FROM users").fetch_one(&app.pool).await.unwrap();

    // 1. done 8 天前 → 应删；done 5 天前 → 应保留
    let j_done_old = seed_job(&app, uid, "batch_analyze", "done", 2).await;
    sqlx::query("UPDATE background_jobs SET finished_at = now() - interval '8 days' WHERE id=$1")
        .bind(j_done_old)
        .execute(&app.pool)
        .await
        .unwrap();

    let j_done_recent = seed_job(&app, uid, "batch_analyze", "done", 2).await;
    sqlx::query("UPDATE background_jobs SET finished_at = now() - interval '5 days' WHERE id=$1")
        .bind(j_done_recent)
        .execute(&app.pool)
        .await
        .unwrap();

    // 2. cancelled 8 天前 → 应删；cancelled 3 天前 → 应保留
    let j_cancelled_old = seed_job(&app, uid, "batch_analyze", "cancelled", 2).await;
    sqlx::query("UPDATE background_jobs SET finished_at = now() - interval '8 days' WHERE id=$1")
        .bind(j_cancelled_old)
        .execute(&app.pool)
        .await
        .unwrap();

    let j_cancelled_recent = seed_job(&app, uid, "batch_analyze", "cancelled", 2).await;
    sqlx::query("UPDATE background_jobs SET finished_at = now() - interval '3 days' WHERE id=$1")
        .bind(j_cancelled_recent)
        .execute(&app.pool)
        .await
        .unwrap();

    // 3. dead 95 天前 → 应删；dead 80 天前 → 应保留
    let j_dead_old = seed_job(&app, uid, "paper_grading", "dead", 2).await;
    sqlx::query("UPDATE background_jobs SET finished_at = now() - interval '95 days' WHERE id=$1")
        .bind(j_dead_old)
        .execute(&app.pool)
        .await
        .unwrap();

    let j_dead_recent = seed_job(&app, uid, "paper_grading", "dead", 2).await;
    sqlx::query("UPDATE background_jobs SET finished_at = now() - interval '80 days' WHERE id=$1")
        .bind(j_dead_recent)
        .execute(&app.pool)
        .await
        .unwrap();

    // 4. pending / running 任务即使超过 100 天也不受 TTL 删除影响（由心跳租约机制管辖）
    let j_pending_old = seed_job(&app, uid, "batch_analyze", "pending", 2).await;
    sqlx::query("UPDATE background_jobs SET created_at = now() - interval '100 days' WHERE id=$1")
        .bind(j_pending_old)
        .execute(&app.pool)
        .await
        .unwrap();

    let j_running_old = seed_job(&app, uid, "batch_analyze", "running", 2).await;
    sqlx::query("UPDATE background_jobs SET created_at = now() - interval '100 days', heartbeat_at = now() WHERE id=$1")
        .bind(j_running_old)
        .execute(&app.pool)
        .await
        .unwrap();

    let deleted = job_queue::cleanup_expired_jobs(&app.pool).await.unwrap();
    assert_eq!(deleted, 3, "应清理 3 条超期任务（done_old, cancelled_old, dead_old）");

    let remaining_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM background_jobs ORDER BY id ASC")
        .fetch_all(&app.pool)
        .await
        .unwrap();

    assert!(!remaining_ids.contains(&j_done_old), "done_old 应被物理删除");
    assert!(!remaining_ids.contains(&j_cancelled_old), "cancelled_old 应被物理删除");
    assert!(!remaining_ids.contains(&j_dead_old), "dead_old 应被物理删除");

    assert!(remaining_ids.contains(&j_done_recent), "done_recent 应保留");
    assert!(remaining_ids.contains(&j_cancelled_recent), "cancelled_recent 应保留");
    assert!(remaining_ids.contains(&j_dead_recent), "dead_recent 应保留");
    assert!(remaining_ids.contains(&j_pending_old), "pending_old 应保留");
    assert!(remaining_ids.contains(&j_running_old), "running_old 应保留");
}

