//! v3 积分经济系统（ADR-0009）：积分 = 唯一游戏货币，honor system 自授。
//! 所有收益写 `points_ledger` 一条账；`award` 是各里程碑（复习/训练/录入/分析/兑换）的
//! 统一入口（基建 seam，M0 落地，各 M 按事件钩子调用）。
//!
//! 评审整改：
//! - `award`/`balance`/`awarded` 改为接受任意 executor（连接池或事务），供事务内复用；
//! - 幂等类别（daily_goal/streak7/milestone）由部分唯一索引硬约束兜底（迁移 v22），
//!   不再纯靠「先查后插」的应用层判断（并发下可双发）；
//! - `redeem` 在事务内先取 per-user 顾问锁再做余额校验与扣减，消除 TOCTOU 透支。

use sqlx::PgPool;
use crate::error::AppError;

// ---------- 积分类别（context.md 之外的 v3 词表，前端/测试需对齐） ----------

/// 复习一张卡（自评）——日常保底
pub const CAT_REVIEW_CARD: &str = "review_card";
/// 今日队列 100% 完成——每日一次
pub const CAT_DAILY_GOAL: &str = "daily_goal";
/// 连续 7 天加成
pub const CAT_STREAK: &str = "streak7";
/// 完成一场陪练（模拟面试）
pub const CAT_DRILL: &str = "drill";
/// 新增一道真实面试题（source=manual）——主收益
pub const CAT_REAL_QUESTION: &str = "real_question";
/// 新建一场真实面试批次——主收益
pub const CAT_REAL_SESSION: &str = "real_session";
/// 轮次标记通过——主收益
pub const CAT_ROUND_PASS: &str = "round_pass";
/// AI 沉淀题判分完成（进复习队）
pub const CAT_AI_SINK: &str = "ai_sink";
/// 单题手动分析
pub const CAT_MANUAL_ANALYSIS: &str = "manual_analysis";
/// 批量分析每完成一题
pub const CAT_BATCH_ANALYSIS: &str = "batch_analysis";
/// 商城兑换（负）
pub const CAT_REDEMPTION: &str = "redemption";
/// 里程碑一次性奖励
pub const CAT_MILESTONE: &str = "milestone";

/// 幂等类别：同一 (user, category, note) 只允许一条流水（部分唯一索引硬约束）。
/// 这些类别的 note 是天然幂等键（日期/streakN/里程碑名）；其余类别同 note 可合法重复。
const IDEMPOTENT_CATEGORIES: &[&str] = &[CAT_DAILY_GOAL, CAT_STREAK, CAT_MILESTONE];

// ---------- 积分数值（ADR-0009 §4，真实面试巨大领先） ----------

pub const P_REVIEW_CARD: i32 = 5;
pub const P_DAILY_GOAL: i32 = 20;
pub const P_STREAK7: i32 = 50;
pub const P_DRILL: i32 = 30;
pub const P_REAL_QUESTION: i32 = 100;
pub const P_REAL_SESSION: i32 = 300;
pub const P_ROUND_PASS: i32 = 200;
pub const P_AI_SINK: i32 = 10;
pub const P_MANUAL_ANALYSIS: i32 = 15;
pub const P_BATCH_ANALYSIS: i32 = 5;

/// 记一条积分流水（正=收入，负=支出）。幂等类别撞唯一索引时静默跳过（评审 P3 整改：
/// 硬约束兜底，不再依赖应用层 check-then-insert）。
#[tracing::instrument(skip_all)]
pub async fn award(
    db: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    uid: i64,
    amount: i32,
    category: &str,
    ref_type: Option<&str>,
    ref_id: Option<i64>,
    note: &str,
) -> Result<(), AppError> {
    let sql = if IDEMPOTENT_CATEGORIES.contains(&category) {
        "INSERT INTO points_ledger(user_id, amount, category, ref_type, ref_id, note) VALUES($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (user_id, category, note) WHERE category IN ('daily_goal','streak7','milestone') DO NOTHING"
    } else {
        "INSERT INTO points_ledger(user_id, amount, category, ref_type, ref_id, note) VALUES($1,$2,$3,$4,$5,$6)"
    };
    sqlx::query(sql)
        .bind(uid)
        .bind(amount)
        .bind(category)
        .bind(ref_type)
        .bind(ref_id)
        .bind(note)
        .execute(db)
        .await?;
    tracing::info!(uid, amount, category, note, "积分发放");
    Ok(())
}

/// 当前余额 = 全部流水之和
pub async fn balance(db: impl sqlx::Executor<'_, Database = sqlx::Postgres>, uid: i64) -> Result<i64, AppError> {
    let b: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount),0) FROM points_ledger WHERE user_id=$1")
        .bind(uid)
        .fetch_one(db)
        .await?;
    Ok(b)
}

/// 是否已有该类别 + note 的流水（幂等防重复发放，用于 daily/streak/milestone）
pub async fn awarded(
    db: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    uid: i64,
    category: &str,
    note: &str,
) -> Result<bool, AppError> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM points_ledger WHERE user_id=$1 AND category=$2 AND COALESCE(note,'')=$3",
    )
    .bind(uid)
    .bind(category)
    .bind(note)
    .fetch_one(db)
    .await?;
    Ok(n > 0)
}

/// 今日任务进度：复习卡数 / 今日队列完成度 / 今日训练数 / 今日是否已达标
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DailyProgress {
    pub due_today: i64,
    pub done_today: i64,
    pub queue_done: bool,   // 今日队列 100%（已无到期卡）
    pub cards_today: i64,   // 今日已复习卡数
    pub drills_today: i64,  // 今日完成训练数
    pub goal_awarded: bool, // 今日是否已发 daily_goal
}

pub async fn daily(pool: &PgPool, uid: i64) -> Result<DailyProgress, AppError> {
    let (due, done_today, drills_today): (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.next_review_at <= now()),
          (SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_reviewed_at::date = CURRENT_DATE),
          (SELECT count(*) FROM drills WHERE user_id=$1 AND status='finished' AND finished_at::date = CURRENT_DATE)
        "#,
    )
    .bind(uid)
    .fetch_one(pool)
    .await?;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    Ok(DailyProgress {
        due_today: due,
        done_today,
        queue_done: due == 0,
        cards_today: done_today, // 与 done_today 同源（今日已复习卡数）
        drills_today,
        goal_awarded: awarded(pool, uid, CAT_DAILY_GOAL, &today).await?,
    })
}

/// 复习后检查：今日队列清零 -> 发每日目标奖；连续 7 天 -> 发加成。
/// 幂等（awarded 判断 + 唯一索引双保险）。在 grade() 成功后调用。
pub async fn check_review_rewards(pool: &PgPool, uid: i64) -> Result<(), AppError> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let due: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.next_review_at <= now()",
    )
    .bind(uid)
    .fetch_one(pool)
    .await?;
    if due == 0 && !awarded(pool, uid, CAT_DAILY_GOAL, &today).await? {
        award(pool, uid, P_DAILY_GOAL, CAT_DAILY_GOAL, None, None, &today).await?;
    }
    // 连续 7 天：复用 review::compute_streak 的统计口径
    let dates: Vec<chrono::NaiveDate> = sqlx::query_scalar(
        "SELECT DISTINCT rr.last_reviewed_at::date FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1 AND rr.last_reviewed_at IS NOT NULL ORDER BY 1 DESC",
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;
    let streak = crate::routes::review::compute_streak(&dates);
    if streak >= 7 && streak % 7 == 0 && !awarded(pool, uid, CAT_STREAK, &format!("streak{streak}")).await? {
        award(pool, uid, P_STREAK7, CAT_STREAK, None, None, &format!("streak{streak}")).await?;
    }
    Ok(())
}

/// 里程碑一次性奖励（幂等）。在 建真实批次/复习/设置 offer 等事件后调用。
pub async fn check_milestones(pool: &PgPool, uid: i64) -> Result<(), AppError> {
    // 真实面试场数（排除系统「模拟面试」公司）：5/10/20
    // ADR-0014 §16：排除系统公司（模拟面试沉淀/回收站/自录题库）的轮次
    let real: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rounds r
         JOIN applications a ON a.id=r.application_id
         JOIN positions p ON p.id=a.position_id JOIN companies c ON c.id=p.company_id
         WHERE a.user_id=$1 AND NOT c.is_system",
    )
    .bind(uid)
    .fetch_one(pool)
    .await?;
    for (n, amt) in [(5i64, 2000i64), (10, 5000), (20, 10000)] {
        let note = format!("real_sessions_{n}");
        if real >= n && !awarded(pool, uid, CAT_MILESTONE, &note).await? {
            award(pool, uid, amt as i32, CAT_MILESTONE, Some("sessions"), None, &note).await?;
        }
    }
    // 累计复习：100/500/1000
    let total_reviewed: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(rr.review_count),0) FROM review_records rr JOIN questions q ON q.id=rr.question_id WHERE q.user_id=$1",
    )
    .bind(uid)
    .fetch_one(pool)
    .await?;
    for (n, amt) in [(100i64, 300i64), (500, 1000), (1000, 2000)] {
        let note = format!("review_total_{n}");
        if total_reviewed >= n && !awarded(pool, uid, CAT_MILESTONE, &note).await? {
            award(pool, uid, amt as i32, CAT_MILESTONE, Some("review"), None, &note).await?;
        }
    }
    // 首个 offer
    let offer: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM applications WHERE status='offer' AND user_id=$1)")
        .bind(uid)
        .fetch_one(pool)
        .await?;
    if offer && !awarded(pool, uid, CAT_MILESTONE, "first_offer").await? {
        award(pool, uid, 10000, CAT_MILESTONE, Some("session"), None, "first_offer").await?;
    }
    Ok(())
}

/// 兑换校验：余额足够 -> 写一条负流水（category=redemption）。返回 (cost, 剩余余额)。
/// 评审 P3 整改：per-user 顾问锁 + 事务，消除并发兑换的 check-then-insert 透支窗口。
pub async fn redeem(pool: &PgPool, uid: i64, item_id: i64) -> Result<(i32, i64), AppError> {
    let mut tx = pool.begin().await?;
    // 同一用户的积分操作串行化（会话级作用域，事务结束自动释放）
    sqlx::query_scalar::<_, ()>("SELECT pg_advisory_xact_lock($1)")
        .bind(uid as i64)
        .fetch_one(&mut *tx)
        .await?;

    let item: Option<(i64, String, i32)> =
        sqlx::query_as("SELECT id, name, cost FROM mall_items WHERE id=$1 AND user_id=$2")
            .bind(item_id)
            .bind(uid)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((id, name, cost)) = item else {
        return Err(AppError::NotFound);
    };
    let bal = balance(&mut *tx, uid).await?;
    if bal < cost as i64 {
        return Err(AppError::BadRequest(format!(
            "积分不足：需要 {cost}，当前余额 {bal}"
        )));
    }
    award(&mut *tx, uid, -cost, CAT_REDEMPTION, Some("mall_items"), Some(id), &name).await?;
    let remaining = balance(&mut *tx, uid).await?;
    tx.commit().await?;
    Ok((cost, remaining))
}
