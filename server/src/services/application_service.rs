//! 投递状态机（ADR-0014 D2/D3）：所有 status 变更的唯一入口。
//!
//! 合法流转（Forward-Only）：
//! ```text
//! ∅ → applied
//! applied → interviewing | rejected | withdrawn
//! interviewing → offer | rejected | withdrawn
//! ```
//! 终态（offer/rejected/withdrawn）禁一切流转；同态请求幂等 no-op。
//!
//! 伴随动作：
//! - applied→interviewing：仅 source=Auto（添加首场面试触发），手工 PATCH 制造进行中被拒绝；
//! - →offer：守卫 round_count>=1，补标全部 pending 轮次为 pass（每轮真实 pending→pass 才 +200，
//!   幂等由「只处理 pending」保证——重复 Offer 无 pending 可补，绝不重复发分）。

use sqlx::PgPool;

use crate::error::AppError;
use crate::points;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionSource {
    /// 系统自动推进（如添加首场面试）
    Auto,
    /// 用户显式操作（详情页按钮/轮次确认流/批量管理/PATCH API）
    Manual,
}

impl TransitionSource {
    fn as_str(self) -> &'static str {
        match self {
            TransitionSource::Auto => "auto",
            TransitionSource::Manual => "manual",
        }
    }
}

#[derive(Debug)]
pub struct TransitionOutcome {
    pub from: String,
    pub to: String,
    /// 本次因 Offer 补标而 pending→pass 的轮次数（积分审计用）
    pub rounds_promoted: i64,
}

/// forward-only 守卫表（与 ADR-0014 §2.2 一致）
fn can_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("applied", "interviewing")
            | ("applied", "rejected")
            | ("applied", "withdrawn")
            | ("interviewing", "offer")
            | ("interviewing", "rejected")
            | ("interviewing", "withdrawn")
    )
}

/// 统一状态机入口。同态请求幂等返回（不记流水、不发分、不改轮次）。
#[tracing::instrument(skip_all, fields(uid, application_id, to))]
pub async fn transition(
    pool: &PgPool,
    uid: i64,
    application_id: i64,
    to: &str,
    source: TransitionSource,
) -> Result<TransitionOutcome, AppError> {
    let old: Option<String> =
        sqlx::query_scalar("SELECT status FROM applications WHERE id=$1 AND user_id=$2")
            .bind(application_id)
            .bind(uid)
            .fetch_optional(pool)
            .await?;
    let Some(old) = old else {
        return Err(AppError::NotFound);
    };

    // 同态幂等：offer→offer 等重复请求直接成功且无副作用（§4.5）
    if old == to {
        return Ok(TransitionOutcome { from: old, to: to.to_string(), rounds_promoted: 0 });
    }

    if !can_transition(&old, to) {
        let hint = if can_transition(to, &old) { "（状态只进不退）" } else { "" };
        return Err(AppError::BadRequest(format!(
            "非法流转：{} → {}{hint}",
            old, to
        )));
    }

    // §3.2：进行中只能由添加首场面试自动推进，禁止手工 PATCH 制造
    if old == "applied" && to == "interviewing" && source == TransitionSource::Manual {
        return Err(AppError::BadRequest(
            "「进行中」由添加首场面试自动推进，不能手动设置".to_string(),
        ));
    }

    // §4.2：Offer 前置守卫——至少要有一场面试
    if to == "offer" {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM rounds WHERE application_id=$1")
            .bind(application_id)
            .fetch_one(pool)
            .await?;
        if n == 0 {
            return Err(AppError::BadRequest(
                "还没有面试轮次，无法标记 Offer".to_string(),
            ));
        }
    }

    // 评审 P1 整改：主流程事务化——状态更新、流水、补标、积分要么全成、要么全不落，
    // 不再出现「状态改了但轮次没补标/积分发一半」的半套状态。
    let mut tx = pool.begin().await?;

    sqlx::query("UPDATE applications SET status=$2, updated_at=now() WHERE id=$1 AND user_id=$3")
        .bind(application_id)
        .bind(to)
        .bind(uid)
        .execute(&mut *tx)
        .await?;

    crate::routes::applications::record_event(&mut *tx, uid, application_id, Some(&old), to, source.as_str(), None).await?;

    tracing::info!(application_id, from = %old, to, source = source.as_str(), "投递状态流转");

    // §4.3 补标规则：仅 pending → pass；pass/fail 不动。每轮真实流转才 +200（§4.4）。
    let mut rounds_promoted: i64 = 0;
    if to == "offer" {
        let pending: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM rounds WHERE application_id=$1 AND passed='pending' ORDER BY sort_order, id",
        )
        .bind(application_id)
        .fetch_all(&mut *tx)
        .await?;
        if !pending.is_empty() {
            // 一条语句批量补标（评审 P3：不再逐轮 UPDATE）
            sqlx::query("UPDATE rounds SET passed='pass' WHERE id = ANY($1)")
                .bind(&pending)
                .execute(&mut *tx)
                .await?;
        }
        for rid in &pending {
            points::award(
                &mut *tx,
                uid,
                points::P_ROUND_PASS,
                points::CAT_ROUND_PASS,
                Some("rounds"),
                Some(*rid),
                "轮次通过（Offer 补标）",
            )
            .await?;
            rounds_promoted += 1;
        }
    }

    tx.commit().await?;

    // 状态变化（尤其 -> offer）触发里程碑检查（首个 offer 等）；幂等且非关键路径，提交后执行
    points::check_milestones(pool, uid).await?;

    Ok(TransitionOutcome { from: old, to: to.to_string(), rounds_promoted })
}
