//! v5 领域事件总线（ADR-0017 D3）：解耦业务主流程与积分、复习队列、状态流水等副作用。
//! 预留生产级适配器接口，便于未来无缝桥接 Redis Pub/Sub 或外部 MQ。

use sqlx::PgPool;
use tokio::sync::broadcast;

/// 核心领域事件枚举
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum DomainEvent {
    /// 陪练场次结束
    DrillFinished {
        user_id: i64,
        drill_id: i64,
        kind: String,
        score: Option<i32>,
    },
    /// AI 沉淀题判分完成
    AiSinkQuestionGraded {
        user_id: i64,
        drill_id: i64,
        question_id: i64,
    },
    /// 单题手动分析完成
    ManualAnalysisDone {
        user_id: i64,
        question_id: i64,
    },
    /// 批量分析单个题目完成
    BatchAnalysisItemDone {
        user_id: i64,
        question_id: i64,
    },
    /// 复习卡自评完成（remembered/fuzzy/forgot）
    ReviewCardGraded {
        user_id: i64,
        question_id: i64,
        result: String,
        answer: Option<String>,
    },
    /// 真实面试轮次创建
    RealRoundCreated {
        user_id: i64,
        application_id: i64,
        round_id: i64,
        round_name: String,
    },
    /// 真实面试轮次标记通过
    RealRoundPassed {
        user_id: i64,
        application_id: i64,
        round_id: i64,
        round_name: String,
    },
    /// 真实面试题创建
    RealQuestionCreated {
        user_id: i64,
        question_id: i64,
        round_id: i64,
    },
}

/// 进程内领域事件总线
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<DomainEvent>,
    pool: PgPool,
}

impl EventBus {
    pub fn new(pool: PgPool) -> Self {
        let (sender, _) = broadcast::channel(512);
        Self { sender, pool }
    }

    /// 发布领域事件并执行已注册的内置副作用
    #[tracing::instrument(skip(self), fields(event = ?event))]
    pub async fn dispatch(&self, event: DomainEvent) -> Result<(), crate::error::AppError> {
        // 1. 广播给外部订阅者（如 SSE、可观测性、未来 MQ 桥接等）
        let _ = self.sender.send(event.clone());

        // 2. 执行核心内置副作用（积分、复习队列、流水等）
        handle_domain_event(&self.pool, &event).await?;
        Ok(())
    }

    /// 订阅事件广播流（供外部监听器/SSE 消费）
    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.sender.subscribe()
    }
}

async fn question_excerpt(pool: &PgPool, qid: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT left(content, 48) FROM questions WHERE id=$1")
        .bind(qid)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// 处理核心领域事件对应的副作用
#[tracing::instrument(skip(pool))]
async fn handle_domain_event(pool: &PgPool, event: &DomainEvent) -> Result<(), crate::error::AppError> {
    match event {
        DomainEvent::DrillFinished { user_id, drill_id, kind: _, score: _ } => {
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_DRILL,
                crate::points::CAT_DRILL,
                Some("drills"),
                Some(*drill_id),
                "",
            )
            .await?;
        }
        DomainEvent::AiSinkQuestionGraded { user_id, drill_id: _, question_id } => {
            let note = question_excerpt(pool, *question_id).await;
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_AI_SINK,
                crate::points::CAT_AI_SINK,
                Some("questions"),
                Some(*question_id),
                &note,
            )
            .await?;
        }
        DomainEvent::ManualAnalysisDone { user_id, question_id } => {
            let note = question_excerpt(pool, *question_id).await;
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_MANUAL_ANALYSIS,
                crate::points::CAT_MANUAL_ANALYSIS,
                Some("questions"),
                Some(*question_id),
                &note,
            )
            .await?;
        }
        DomainEvent::BatchAnalysisItemDone { user_id, question_id } => {
            let note = question_excerpt(pool, *question_id).await;
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_BATCH_ANALYSIS,
                crate::points::CAT_BATCH_ANALYSIS,
                Some("questions"),
                Some(*question_id),
                &note,
            )
            .await?;
        }
        DomainEvent::ReviewCardGraded { user_id, question_id, result: _, answer } => {
            let note = question_excerpt(pool, *question_id).await;
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_REVIEW_CARD,
                crate::points::CAT_REVIEW_CARD,
                Some("questions"),
                Some(*question_id),
                &note,
            )
            .await?;
            // 2. 每日目标与连续天数奖励检查
            crate::points::check_review_rewards(pool, *user_id).await?;
            crate::points::check_milestones(pool, *user_id).await?;
            // 3. 复习回答历史记录
            if let Some(a) = answer.as_deref() {
                if !a.trim().is_empty() {
                    crate::routes::questions::record_answer(pool, *question_id, "review", a).await?;
                }
            }
        }
        DomainEvent::RealRoundCreated { user_id, application_id, round_id, round_name } => {
            // 1. 状态流水跟踪
            crate::routes::applications::record_event_kind(
                pool,
                *user_id,
                *application_id,
                "round",
                None,
                "",
                "manual",
                Some(&format!("添加面试：{round_name}")),
            )
            .await?;
            // 2. 真实面试主收益积分 (+300)
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_REAL_SESSION,
                crate::points::CAT_REAL_SESSION,
                Some("rounds"),
                Some(*round_id),
                "真实面试",
            )
            .await?;
            // 3. 里程碑检查
            crate::points::check_milestones(pool, *user_id).await?;
        }
        DomainEvent::RealRoundPassed { user_id, application_id, round_id, round_name } => {
            // 1. 状态流水记录
            crate::routes::applications::record_event_kind(
                pool,
                *user_id,
                *application_id,
                "round",
                None,
                "",
                "manual",
                Some(&format!("{round_name} · 标记通过")),
            )
            .await?;
            // 2. 轮次通过主收益积分 (+200)
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_ROUND_PASS,
                crate::points::CAT_ROUND_PASS,
                Some("rounds"),
                Some(*round_id),
                "轮次通过",
            )
            .await?;
        }
        DomainEvent::RealQuestionCreated { user_id, question_id, round_id: _ } => {
            // 新增真实面试题主收益积分 (+100)
            crate::points::award(
                pool,
                *user_id,
                crate::points::P_REAL_QUESTION,
                crate::points::CAT_REAL_QUESTION,
                Some("questions"),
                Some(*question_id),
                &question_excerpt(pool, *question_id).await,
            )
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serialization_roundtrip() {
        let ev = DomainEvent::DrillFinished {
            user_id: 1,
            drill_id: 42,
            kind: "interview".into(),
            score: Some(85),
        };
        let json = serde_json::to_string(&ev).expect("serialize failed");
        let decoded: DomainEvent = serde_json::from_str(&json).expect("deserialize failed");
        match decoded {
            DomainEvent::DrillFinished { user_id, drill_id, score, .. } => {
                assert_eq!(user_id, 1);
                assert_eq!(drill_id, 42);
                assert_eq!(score, Some(85));
            }
            _ => panic!("type mismatch"),
        }

        let ev2 = DomainEvent::ReviewCardGraded {
            user_id: 2,
            question_id: 100,
            result: "remembered".into(),
            answer: Some("我的回答".into()),
        };
        let json2 = serde_json::to_string(&ev2).expect("serialize failed");
        let decoded2: DomainEvent = serde_json::from_str(&json2).expect("deserialize failed");
        match decoded2 {
            DomainEvent::ReviewCardGraded { user_id, question_id, result, answer } => {
                assert_eq!(user_id, 2);
                assert_eq!(question_id, 100);
                assert_eq!(result, "remembered");
                assert_eq!(answer, Some("我的回答".into()));
            }
            _ => panic!("type mismatch"),
        }
    }
}
