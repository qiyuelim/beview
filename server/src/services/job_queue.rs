//! 后台任务持久化队列（票09）：批量分析 / 试卷判卷等后台任务从进程内注册表迁入 Postgres。
//!
//! 语义（单实例部署，HANDOFF：单人自用局域网）：
//! - 原子认领：`claim_one` 以 `FOR UPDATE SKIP LOCKED` 认领 pending 或心跳超期的 running 任务，
//!   并发调用互斥；
//! - 心跳与超时重派：执行侧周期性刷新 heartbeat_at；超过 LEASE_SECS 无心跳的 running 视为
//!   孤儿（进程崩溃）可被重新认领，attempts 已随之累加；
//! - 启动恢复：`reset_running_on_boot` 把全部 running 重置为 pending（单实例下进程重启即
//!   意味着旧执行者已死），配合重派实现「重启后未完成任务自动续跑」——重启即清成为历史；
//! - 失败重试上限：失败且 attempts >= max_attempts 时置 dead（死信标注），不再自动重派。
//!
//! 纪律：队列只管「何时何地跑」，任务体本身仍复用各域既有管线（run_analysis /
//! grade_paper_batch_inner），AGENTS 基准 3 的手动触发纪律不变。

use serde_json::json;

/// running 任务的心跳租约秒数：超过此时长无心跳即可被其他认领者接管
pub const LEASE_SECS: i64 = 60;

#[derive(Debug, sqlx::FromRow)]
pub struct QueuedJob {
    pub id: i64,
    pub user_id: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

/// 入队：status=pending，返回行 id（即对外 job_id）
pub async fn enqueue(
    pool: &sqlx::PgPool,
    uid: i64,
    kind: &str,
    payload: &serde_json::Value,
    max_attempts: i32,
) -> Result<i64, crate::error::AppError> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO background_jobs(user_id, kind, payload, max_attempts) VALUES($1,$2,$3,$4) RETURNING id",
    )
    .bind(uid)
    .bind(kind)
    .bind(payload)
    .bind(max_attempts)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// 原子认领一个可执行任务：pending 优先；running 且心跳超期（孤儿）同样可认领。
/// UPDATE ... RETURNING 保证多并发认领者之间互斥（SKIP LOCKED 避免互相等锁）。
pub async fn claim_one(
    pool: &sqlx::PgPool,
    kinds: &[&str],
) -> Result<Option<QueuedJob>, crate::error::AppError> {
    let row: Option<QueuedJob> = sqlx::query_as(
        r#"
        -- 认领即计一次尝试（attempts 在收尾时与 max_attempts 比较，决定重派或死信）
        UPDATE background_jobs SET status='running', claimed_at=now(), heartbeat_at=now(), attempts=attempts+1
        WHERE id = (
          SELECT id FROM background_jobs
          WHERE status='pending' AND kind = ANY($1)
             OR (status='running' AND heartbeat_at < now() - make_interval(secs => $2::double precision) AND kind = ANY($1))
          ORDER BY created_at ASC LIMIT 1
          FOR UPDATE SKIP LOCKED
        )
        RETURNING id, user_id, kind, payload, attempts, max_attempts
        "#,
    )
    .bind(&kinds)
    .bind(LEASE_SECS as f64)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 执行期间心跳续租（由执行侧周期性调用）
pub async fn heartbeat(pool: &sqlx::PgPool, id: i64) -> Result<(), crate::error::AppError> {
    sqlx::query("UPDATE background_jobs SET heartbeat_at=now() WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 收尾：成功 → done；失败 → attempts 未超上限回 pending 等待重派，超上限置 dead（死信）。
/// 若任务已被取消（cancelled），保持 cancelled 状态不覆写。
pub async fn finish(
    pool: &sqlx::PgPool,
    id: i64,
    ok: bool,
    error: Option<&str>,
) -> Result<(), crate::error::AppError> {
    if ok {
        sqlx::query("UPDATE background_jobs SET status='done', finished_at=now(), error=NULL WHERE id=$1 AND status != 'cancelled'")
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE background_jobs
        SET status = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'pending' END,
            finished_at = now(),
            error = $2
        WHERE id=$1 AND status != 'cancelled'
        "#,
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// 进度更新（批量任务逐题回写 done/ok/failed 与已完成题目集合，供重启后续跑去重）
pub async fn update_progress(
    pool: &sqlx::PgPool,
    id: i64,
    progress: &serde_json::Value,
) -> Result<(), crate::error::AppError> {
    sqlx::query("UPDATE background_jobs SET progress=$2 WHERE id=$1")
        .bind(id)
        .bind(progress)
        .execute(pool)
        .await?;
    Ok(())
}

/// 取消：仅 pending/running 可取消（幂等：已终态返回 false 由调用方决定语义）
pub async fn cancel(pool: &sqlx::PgPool, id: i64, uid: i64) -> Result<bool, crate::error::AppError> {
    let n = sqlx::query(
        "UPDATE background_jobs SET status='cancelled', finished_at=now() \
         WHERE id=$1 AND user_id=$2 AND status IN ('pending','running')",
    )
    .bind(id)
    .bind(uid)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// 是否存在同 kind 且 payload 中指定字段匹配的未完结任务（受理幂等用）
pub async fn has_active_with_payload_id(
    pool: &sqlx::PgPool,
    uid: i64,
    kind: &str,
    payload_key: &str,
    value: i64,
) -> Result<bool, crate::error::AppError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM background_jobs \
         WHERE user_id=$1 AND kind=$2 AND status IN ('pending','running') AND (payload->>$3)::bigint=$4)",
    )
    .bind(uid)
    .bind(kind)
    .bind(payload_key)
    .bind(value)
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

/// 启动恢复：把上一进程遗留的 running 全部置回 pending（单实例语义，见模块注释）。
/// 返回重置数量供启动日志。
pub async fn reset_running_on_boot(pool: &sqlx::PgPool) -> Result<u64, crate::error::AppError> {
    let n = sqlx::query("UPDATE background_jobs SET status='pending', heartbeat_at=NULL WHERE status='running'")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(n)
}

#[derive(Debug, serde::Serialize)]
pub struct JobView {
    pub id: i64,
    pub status: String,
    pub total: usize,
    pub done: usize,
    pub ok: usize,
    pub failed: usize,
}

/// 读取任务视图（行级隔离由调用方过滤 uid）
pub async fn get_view(pool: &sqlx::PgPool, uid: i64, id: i64) -> Result<Option<JobView>, crate::error::AppError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        status: String,
        total: i32,
        progress: Option<serde_json::Value>,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT status, COALESCE((payload->>'total')::int, 0) AS total, progress FROM background_jobs WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(uid)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        let p = r.progress.clone().unwrap_or(json!({}));
        JobView {
            id,
            // 对外状态映射：pending 视同 running（前端只有 running/done/cancelled/error 四态）
            status: match r.status.as_str() {
                "done" => "done".into(),
                "cancelled" => "cancelled".into(),
                "failed" | "dead" => "error".into(),
                _ => "running".into(),
            },
            total: r.total.max(0) as usize,
            done: p.get("done").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            ok: p.get("ok").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            failed: p.get("failed").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        }
    }))
}

/// 启动恢复（票09 判卷侧）：试卷卡在 grading='grading' 但已无活跃队列任务的
/// （上一进程崩溃遗留），自动补一条 paper_grading 队列任务，由 dispatcher 续判。
pub async fn recover_orphaned_paper_gradings(
    pool: &sqlx::PgPool,
) -> Result<u64, crate::error::AppError> {
    #[derive(sqlx::FromRow)]
    struct Orphan {
        id: i64,
        user_id: i64,
    }
    let orphans: Vec<Orphan> = sqlx::query_as(
        r#"
        SELECT d.id, d.user_id FROM drills d
        WHERE d.grading = 'grading'
          AND NOT EXISTS (
            SELECT 1 FROM background_jobs bj
            WHERE bj.kind = 'paper_grading'
              AND bj.status IN ('pending','running')
              AND (bj.payload->>'drill_id')::bigint = d.id
          )
        "#,
    )
    .fetch_all(pool)
    .await?;
    let n = orphans.len() as u64;
    for o in &orphans {
        enqueue(pool, o.user_id, "paper_grading", &json!({ "drill_id": o.id }), 2).await?;
    }
    Ok(n)
}

/// 分层 TTL 物理清理（V6 M2 底座清偿）：
/// - done / cancelled: finished_at < NOW() - 7 days (7天物理清除)
/// - dead: finished_at < NOW() - 90 days (90天留档审计后物理清除)
/// - pending / running: 绝不进行 TTL 清理（由心跳租约与启动重置机制管理）
pub async fn cleanup_expired_jobs(pool: &sqlx::PgPool) -> Result<u64, crate::error::AppError> {
    let n = sqlx::query(
        r#"
        DELETE FROM background_jobs
        WHERE (status IN ('done', 'cancelled') AND finished_at < now() - interval '7 days')
           OR (status = 'dead' AND finished_at < now() - interval '90 days')
        "#,
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}

/// 常驻派发循环（票09）：进程级单例，认领 pending/超期任务并按 kind 分发执行。
/// 每个执行任务附带心跳 sidecar（15s 续租），失败按 attempts/max_attempts 决定重派或死信。
pub fn spawn_dispatcher(st: crate::state::AppState) {
    tokio::spawn(async move {
        loop {
            match claim_one(&st.pool, &["batch_analyze", "paper_grading"]).await {
                Ok(Some(job)) => {
                    let st2 = st.clone();
                    tokio::spawn(async move {
                        // 心跳 sidecar：每 15s 续租，任务结束即中止
                        let hb_pool = st2.pool.clone();
                        let hb_job_id = job.id;
                        let heartbeat_task = tokio::spawn(async move {
                            loop {
                                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                                if heartbeat(&hb_pool, hb_job_id).await.is_err() {
                                    break;
                                }
                            }
                        });

                        let result = match job.kind.as_str() {
                            "batch_analyze" => crate::routes::batch::execute_batch_job(&st2, &job).await,
                            other => Err(crate::error::AppError::BadRequest(format!(
                                "未知后台任务类型 {other}"
                            ))),
                        };

                        heartbeat_task.abort();
                        match result {
                            Ok(()) => {
                                let _ = finish(&st2.pool, job.id, true, None).await;
                            }
                            Err(e) => {
                                tracing::error!(
                                    job_id = job.id, kind = %job.kind, error = %e,
                                    "后台队列任务执行失败（将按 attempts 重试或死信）"
                                );
                                let _ =
                                    finish(&st2.pool, job.id, false, Some(&e.to_string())).await;
                            }
                        }
                    });
                }
                Ok(None) => {
                    #[cfg(test)]
                    let delay = std::time::Duration::from_millis(50);
                    #[cfg(not(test))]
                    let delay = std::time::Duration::from_millis(1000);
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "后台任务认领失败");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}
