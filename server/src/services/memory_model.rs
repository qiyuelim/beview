//! FSRS 记忆模型胶水层（ADR-0022 D1）：个人化权重拟合 + 卡片记忆状态与可提取性预测。
//!
//! 本模块是**纯函数接缝**：输入输出均为普通数据结构，不含任何 DB / HTTP 访问，
//! 单元测试在此层完成；`routes/stats.rs` 负责取数与装配。
//!
//! 评分映射（词表口径）：forgot→1(Again)、fuzzy→2(Hard)、remembered→3(Good)。
//! 拟合门槛：日志总数低于 [`MIN_FIT_LOGS`] 时回退默认权重并标注 `fitted=false`
//! （诚实优先——样本不足时的"个性化"只是噪声，ADR-0022 D1）。

use fsrs::{
    compute_parameters, current_retrievability, ComputeParametersInput, FSRS, FSRS6_DEFAULT_DECAY,
    FSRSItem, FSRSReview,
};
pub use fsrs::DEFAULT_PARAMETERS;

use std::sync::Once;

/// 个人规模复习日志无法满足 fsrs-rs 离群分析的稠密分桶要求（Anki 万条级设计），
/// 会被整体判为离点导致 NotEnoughData；该环境变量是上游提供的官方旁路。
/// 进程内一次性设置：fsrs crate 仅在训练入口读它，本模块是进程内唯一使用方。
static NO_OUTLIER_INIT: Once = Once::new();
fn ensure_fsrs_no_outlier() {
    NO_OUTLIER_INIT.call_once(|| {
        // SAFETY: 进程启动后单次写入，此后不再变更；crate 内仅读取。
        // 当前调用点可能发生在测试多线程环境下，但无其他代码并发读写此变量。
        unsafe { std::env::set_var("FSRS_NO_OUTLIER", "1") };
    });
}

/// 启用权重拟合的最低复习日志总条数（含回填的存量合成日志）。
pub const MIN_FIT_LOGS: usize = 20;

/// 无复习记录的新卡兜底留存率（沿用 v5.2 口径）。
pub const NEW_CARD_RETENTION: f64 = 0.4;

/// 一张卡的复习日志序列，`days_elapsed` 为距现在的天数。
/// 调用方保证按时间**升序**排列（即 `days_elapsed` 递减）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReviewLog {
    pub rating: u32,
    pub days_elapsed: f64,
}

/// 拟合结果：权重 + 是否真的完成了个性化拟合。
#[derive(Debug, Clone, PartialEq)]
pub struct FitResult {
    pub weights: Vec<f32>,
    pub fitted: bool,
}

/// 复习自评等级 → FSRS rating。未知等级返回 None（调用方跳过该条）。
pub fn fsrs_rating(result: &str) -> Option<u32> {
    match result {
        "forgot" => Some(1),
        "fuzzy" => Some(2),
        "remembered" => Some(3),
        _ => None,
    }
}

/// 把单卡升序日志序列转为 `FSRSItem`：
/// - 首条 `delta_t = 0`（crate 硬约束）；
/// - 相邻间隔天数四舍五入；
/// - 同日多条评分折叠为当日最后一条（FSRS 日粒度模型不消费日内多次评分）。
/// 空序列返回 None。
pub fn build_item(logs: &[ReviewLog]) -> Option<FSRSItem> {
    let mut collapsed: Vec<ReviewLog> = Vec::new();
    for log in logs {
        match collapsed.last_mut() {
            // 同日（间隔 < 0.5 天视为同日）：保留更晚的一条
            Some(prev) if (prev.days_elapsed - log.days_elapsed) < 0.5 => *prev = *log,
            _ => collapsed.push(*log),
        }
    }
    if collapsed.is_empty() {
        return None;
    }
    let mut reviews = Vec::with_capacity(collapsed.len());
    let mut prev_elapsed = collapsed[0].days_elapsed;
    for log in &collapsed {
        reviews.push(FSRSReview {
            rating: log.rating.clamp(1, 4),
            delta_t: (prev_elapsed - log.days_elapsed).round().max(0.0) as u32,
        });
        prev_elapsed = log.days_elapsed;
    }
    reviews[0].delta_t = 0; // crate 要求首评 delta_t 必须为 0
    Some(FSRSItem { reviews })
}

/// 以全部卡片的复习历史拟合个人化 FSRS 权重。
/// 样本不足或训练失败时回退默认参数（`fitted=false`）。同一输入重复拟合约定产出一致权重。
pub fn fit_weights(items: &[FSRSItem]) -> FitResult {
    ensure_fsrs_no_outlier();
    let total_logs: usize = items.iter().map(|i| i.reviews.len()).sum();
    if items.is_empty() || total_logs < MIN_FIT_LOGS {
        return FitResult {
            weights: DEFAULT_PARAMETERS.to_vec(),
            fitted: false,
        };
    }
    let input = ComputeParametersInput {
        train_set: items.to_vec(),
        enable_short_term: false, // 复习流为日粒度，无日内短期记忆步
        ..Default::default()
    };
    match compute_parameters(input) {
        // 诚实不变量：fitted=true 当且仅当权重确实因用户数据而异于默认参数
        // （crate 对极小/低多样性数据集会静默原样返回默认参数）。
        Ok(w) if !w.is_empty() && w != DEFAULT_PARAMETERS.to_vec() => {
            FitResult { weights: w, fitted: true }
        }
        _ => FitResult {
            weights: DEFAULT_PARAMETERS.to_vec(),
            fitted: false,
        },
    }
}

pub fn retrievability(weights: &[f32], item: &FSRSItem, now_elapsed_days: f64) -> Option<f64> {
    let model = FSRS::new(weights).ok()?;
    let state = model.memory_state(item.clone(), None).ok()?;
    let r = current_retrievability(state, now_elapsed_days.max(0.0) as f32, FSRS6_DEFAULT_DECAY);
    Some(r.clamp(0.0, 1.0) as f64)
}

/// 基于 FSRS 拟合稳定性计算下一次复习间隔天数（V6 M2 排程化）。
/// `desired_retention` 目标留存率（默认 0.9）。
/// 间隔基于稳定性参数推导：`I = next_interval(stability, desired_retention, decay)`
pub fn next_interval_days(weights: &[f32], item: &FSRSItem, desired_retention: f32) -> Option<i32> {
    let model = FSRS::new(weights).ok()?;
    let state = model.memory_state(item.clone(), None).ok()?;
    let interval = model.next_interval(Some(state.stability), desired_retention, 3650);
    Some(interval.ceil().max(1.0) as i32)
}

/// 计算单题在一次自评后的排程间隔（天数，V6 M2 排程化）：
/// - 当自评为 forgot 时，严格重置为 1 天；
/// - 若有复习日志且能解析为 FSRSItem，基于 FSRS stability 推导排程间隔（desired_retention=0.9）；
/// - 若无复习日志或 FSRS 计算不可用，平滑回退到经验间隔（SM-2 变体：remembered 2.5x, fuzzy 1.5x, forgot 1d）。
pub fn schedule_next_interval(
    weights: &[f32],
    logs: &[ReviewLog],
    fallback_interval: i32,
    fallback_result: &str,
) -> i32 {
    if fallback_result == "forgot" {
        return 1;
    }
    if let Some(item) = build_item(logs) {
        if let Some(days) = next_interval_days(weights, &item, 0.9) {
            return days.max(1);
        }
    }
    match fallback_result {
        "remembered" => ((fallback_interval as f64) * 2.5).ceil().max(1.0) as i32,
        "fuzzy" => ((fallback_interval as f64) * 1.5).ceil().max(1.0) as i32,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(rating: u32, days_elapsed: f64) -> ReviewLog {
        ReviewLog { rating, days_elapsed }
    }

    #[test]
    fn rating_mapping_covers_three_grades_and_rejects_unknown() {
        assert_eq!(fsrs_rating("forgot"), Some(1));
        assert_eq!(fsrs_rating("fuzzy"), Some(2));
        assert_eq!(fsrs_rating("remembered"), Some(3));
        assert_eq!(fsrs_rating("bogus"), None);
    }

    #[test]
    fn build_item_sets_first_delta_zero_and_rounds_gaps() {
        // 时间升序 = days_elapsed 递减
        let item = build_item(&[log(3, 10.4), log(3, 7.2), log(1, 2.9), log(3, 0.1)]).unwrap();
        assert_eq!(item.reviews.len(), 4);
        assert_eq!(item.reviews[0].delta_t, 0);
        assert_eq!(item.reviews[1].delta_t, 3); // 10.4-7.2=3.2 → 3
        assert_eq!(item.reviews[2].delta_t, 4); // 7.2-2.9=4.3 → 4
        assert_eq!(item.reviews[3].delta_t, 3); // 2.9-0.1=2.8 → 3
    }

    #[test]
    fn build_item_collapses_same_day_reviews_keeping_latest() {
        let item = build_item(&[log(1, 5.2), log(3, 5.0), log(3, 1.0)]).unwrap();
        assert_eq!(item.reviews.len(), 2);
        assert_eq!(item.reviews[0].rating, 3); // 同日两条折叠保留后一条
        assert_eq!(item.reviews[1].delta_t, 4);
    }

    #[test]
    fn build_item_empty_returns_none() {
        assert!(build_item(&[]).is_none());
    }

    #[test]
    fn fit_below_threshold_falls_back_to_defaults_unfitted() {
        let items: Vec<FSRSItem> = (0..3)
            .map(|_| build_item(&[log(3, 5.0), log(3, 1.0)]).unwrap())
            .collect(); // 6 条日志 < 20
        let fit = fit_weights(&items);
        assert!(!fit.fitted);
        assert_eq!(fit.weights, DEFAULT_PARAMETERS.to_vec());
    }

    /// 合成确定性复习数据：70 张卡（单次/多次混合，280±条日志），评分与间隔均带变化。
    fn synthetic_items() -> Vec<FSRSItem> {
        const PATTERN: [u32; 7] = [3, 3, 2, 3, 1, 3, 2];
        let mut items = Vec::new();
        for card in 0..70usize {
            // 真实用户形态的混合分布：一部分卡只复习过一次，其余 2~4 次
            let steps = match card % 5 {
                0 => 1,
                c => 1 + c, // 2..=4
            };
            let mut logs = Vec::new();
            let mut elapsed = 40.0 + (card % 11) as f64 * 3.0;
            for step in 0..steps {
                logs.push(log(PATTERN[(card + step * 5) % 7], elapsed));
                elapsed -= 1.0 + ((card * 3 + step * 7) % 13) as f64;
            }
            items.push(build_item(&logs).unwrap());
        }
        items
    }

    #[test]
    fn fitting_is_idempotent_for_identical_input() {
        let items = synthetic_items(); // 280 条日志 ≥ 门槛
        let a = fit_weights(&items);
        let b = fit_weights(&items);
        assert!(a.fitted);
        assert_eq!(a.weights, b.weights, "同一输入两次拟合必须产出一致权重");
        assert_ne!(a.weights, DEFAULT_PARAMETERS.to_vec(), "拟合应偏离默认参数");
    }

    #[test]
    fn retrievability_decays_with_time_and_is_clamped() {
        let fit = fit_weights(&synthetic_items());
        let item = synthetic_items().pop().unwrap();
        let fresh = retrievability(&fit.weights, &item, 0.1).unwrap();
        let week = retrievability(&fit.weights, &item, 30.0).unwrap();
        let year = retrievability(&fit.weights, &item, 3650.0).unwrap();
        assert!(fresh > week && week > year, "留存率必须随时间单调衰减");
        assert!(year >= 0.0 && fresh <= 1.0);
    }

    #[test]
    fn retrievability_works_with_default_weights_too() {
        let item = build_item(&[log(3, 4.0)]).unwrap();
        let r = retrievability(&DEFAULT_PARAMETERS.to_vec(), &item, 2.0).unwrap();
        assert!((0.0..=1.0).contains(&r));
    }

    #[test]
    fn schedule_forgot_always_returns_one_day() {
        let fit = fit_weights(&synthetic_items());
        let logs = vec![log(3, 10.0), log(3, 5.0), log(1, 0.1)];
        let days = schedule_next_interval(&fit.weights, &logs, 14, "forgot");
        assert_eq!(days, 1, "forgot 必须强制重置为 1 天");
    }

    #[test]
    fn schedule_uses_fsrs_stability_and_grows_with_reviews() {
        let fit = fit_weights(&synthetic_items());
        let logs1 = vec![log(3, 0.0)];
        let d1 = schedule_next_interval(&fit.weights, &logs1, 1, "remembered");

        let logs2 = vec![log(3, 10.0), log(3, 5.0), log(3, 0.0)];
        let d2 = schedule_next_interval(&fit.weights, &logs2, d1, "remembered");
        assert!(d2 >= d1, "多次记住后 FSRS 稳定性排程应递增: d1={d1}, d2={d2}");
    }

    #[test]
    fn schedule_falls_back_when_unfitted_or_empty_logs() {
        let days = schedule_next_interval(&DEFAULT_PARAMETERS.to_vec(), &[], 4, "remembered");
        assert_eq!(days, 10); // 4 * 2.5 = 10
        let days_fuzzy = schedule_next_interval(&DEFAULT_PARAMETERS.to_vec(), &[], 4, "fuzzy");
        assert_eq!(days_fuzzy, 6); // 4 * 1.5 = 6
    }
}
