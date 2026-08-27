//! 系统容器（ADR-0014 D4/D5/D6）：per-user 的回收站与自录题库。
//! 仿「模拟面试」ensure 模式懒创建；普通业务 API 禁止触碰 is_system 资源（§17 写保护），
//! 本模块是唯一合法的创建/读取入口。

use sqlx::PgPool;

use crate::error::AppError;

/// 系统公司名（词表见 docs/context.md；is_system=true）
pub const TOMBSTONE_COMPANY: &str = "回收站";
pub const SELF_COMPANY: &str = "自录题库";

/// ensure 链：公司 → 岗位 → 投递 → 轮次，全部幂等（find-or-create）。
async fn ensure_container(
    pool: &PgPool,
    uid: i64,
    company_name: &str,
    position_title: &str,
    round_name: &str,
) -> Result<i64, AppError> {
    let company_id: i64 = sqlx::query_scalar(
        "INSERT INTO companies(user_id, name, is_system) VALUES($1,$2,true)
         ON CONFLICT (user_id, name) DO UPDATE SET name=EXCLUDED.name RETURNING id",
    )
    .bind(uid)
    .bind(company_name)
    .fetch_one(pool)
    .await?;
    // 存量同名公司（理论上不存在）兜底标记为系统公司
    sqlx::query("UPDATE companies SET is_system=true WHERE id=$1 AND NOT is_system")
        .bind(company_id)
        .execute(pool)
        .await?;

    let position_id: i64 = sqlx::query_scalar(
        "INSERT INTO positions(company_id, user_id, title) VALUES($1,$2,$3)
         ON CONFLICT (user_id, company_id, title) DO UPDATE SET title=EXCLUDED.title RETURNING id",
    )
    .bind(company_id)
    .bind(uid)
    .bind(position_title)
    .fetch_one(pool)
    .await?;

    // 容器投递：状态恒 applied、永不流转、被所有看板/统计按 is_system 排除
    let application_id: Option<i64> = sqlx::query_scalar(
        "SELECT a.id FROM applications a JOIN positions p ON p.id=a.position_id
         WHERE a.user_id=$1 AND p.id=$2 LIMIT 1",
    )
    .bind(uid)
    .bind(position_id)
    .fetch_optional(pool)
    .await?;
    let application_id = match application_id {
        Some(id) => id,
        None => sqlx::query_scalar(
            "INSERT INTO applications(user_id, position_id, status) VALUES($1,$2,'applied') RETURNING id",
        )
        .bind(uid)
        .bind(position_id)
        .fetch_one(pool)
        .await?,
    };

    let round_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM rounds WHERE application_id=$1 AND name=$2 LIMIT 1",
    )
    .bind(application_id)
    .bind(round_name)
    .fetch_optional(pool)
    .await?;
    let round_id = match round_id {
        Some(id) => id,
        None => sqlx::query_scalar(
            "INSERT INTO rounds(application_id, session_id, name, sort_order, passed)
             VALUES($1, NULL, $2, 0, 'pending') RETURNING id",
        )
        .bind(application_id)
        .bind(round_name)
        .fetch_one(pool)
        .await?,
    };
    Ok(round_id)
}

/// 回收站固定轮次：删除投递时题目迁移至此（ADR-0014 §12-15）
pub async fn ensure_tombstone_round(pool: &PgPool, uid: i64) -> Result<i64, AppError> {
    ensure_container(pool, uid, TOMBSTONE_COMPANY, "已删除投递", "已删除投递").await
}

/// 自录题库固定轮次：不关联真实公司的搜罗题挂靠于此（ADR-0014 §18-20）
pub async fn ensure_self_round(pool: &PgPool, uid: i64) -> Result<i64, AppError> {
    ensure_container(pool, uid, SELF_COMPANY, "搜罗题", "收藏题").await
}
