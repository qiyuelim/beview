//! 题目域三出口契约：question_full（全量分析）/ question_ref（参考答案）/ answer_evaluate（回答评价）。
//! 量纲唯一：综合分 0-100 / 难度 1-5（AGENTS 基准 4）；解析即钳制。

use serde::Deserialize;
use serde_json::{json, Value};

use super::{clamp, AiContract};
use crate::error::AppError;
use crate::prompts;

// ---------- question_full：题目全量分析（训练即时判分/批量分析/重新分析共用管线） ----------

/// 输入：题面 + 现场回答（可无）+ 已有参考答案（重分析时对照，保持基本不变）
#[derive(Clone, Debug)]
pub struct QuestionFull {
    pub content: String,
    pub my_answer: Option<String>,
    pub existing_ref: Option<String>,
}

impl QuestionFull {
    pub fn new(content: &str, my_answer: Option<&str>, existing_ref: Option<&str>) -> Self {
        Self {
            content: content.to_string(),
            my_answer: my_answer.map(str::to_string),
            existing_ref: existing_ref.map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct NewSkillItem {
    pub l1: String,
    pub l2: String,
    pub l3: String,
}

/// 强类型输出（schema interview_analysis）
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct QuestionFullOut {
    #[serde(default)]
    pub skill_path: Option<String>,
    #[serde(default)]
    pub new_skill: Option<NewSkillItem>,
    #[serde(default)]
    pub question_type: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "clamp::difficulty_1_5")]
    pub difficulty: Option<i32>,
    #[serde(default)]
    pub ref_answer: String,
    #[serde(default, deserialize_with = "clamp::score_0_100")]
    pub score: Option<i32>,
    #[serde(default)]
    pub feedback: String,
}

impl AiContract for QuestionFull {
    type Output = QuestionFullOut;

    fn prompt_key(&self) -> &'static str {
        prompts::QUESTION_FULL
    }
    fn kind(&self) -> &'static str {
        "analyze"
    }
    fn schema_name(&self) -> &'static str {
        "interview_analysis"
    }
    // 语义层/格式层分工（ADR-0016 D4）：字段名是解析契约，prompt 不内嵌骨架
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill_path": { "type": ["string", "null"] },
                "new_skill": {
                    "type": ["object", "null"],
                    "properties": {
                        "l1": { "type": "string" },
                        "l2": { "type": "string" },
                        "l3": { "type": "string" }
                    },
                    "required": ["l1", "l2", "l3"],
                    "additionalProperties": false
                },
                "question_type": {
                    "type": "string",
                    "enum": ["motivation_culture_fit", "experience_track_record", "professional_knowledge", "scenario_case", "practice_execution", "problem_solving_resilience", "collaboration"]
                },
                "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 3 },
                "difficulty": { "type": ["integer", "null"] },
                "ref_answer": { "type": "string" },
                "score": { "type": ["integer", "null"] },
                "feedback": { "type": "string" }
            },
            "required": ["skill_path", "new_skill", "question_type", "tags", "difficulty", "ref_answer", "score", "feedback"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        let mut msg = match self.my_answer.as_deref() {
            Some(a) if !a.trim().is_empty() => {
                format!("面试题：\n{}\n\n候选人现场回答：\n{a}", self.content)
            }
            _ => format!("面试题：\n{}\n\n候选人现场回答：（未记录现场回答）", self.content),
        };
        if let Some(r) = self.existing_ref.as_deref().filter(|r| !r.trim().is_empty()) {
            msg += &format!("\n\n已有参考答案（保持基本不变，仅用于对照评分）：\n{r}");
        }
        msg
    }
    fn post_process(&self, mut out: Self::Output) -> Result<Self::Output, AppError> {
        if out.ref_answer.trim().is_empty() && out.feedback.trim().is_empty() {
            return Err(AppError::BadRequest("缺少 ref_answer/feedback 关键字段".to_string()));
        }
        // 约束：tags 最多保留 3 个
        if out.tags.len() > 3 {
            out.tags.truncate(3);
        }
        // 约束：考察维度合法性兜底（七类通用维度；未知值/None 落到 professional_knowledge）
        if let Some(qt) = &out.question_type {
            let valid = matches!(qt.as_str(),
                "motivation_culture_fit" | "experience_track_record" | "professional_knowledge" |
                "scenario_case" | "practice_execution" | "problem_solving_resilience" | "collaboration");
            if !valid {
                out.question_type = Some("professional_knowledge".to_string());
            }
        } else {
            out.question_type = Some("professional_knowledge".to_string());
        }
        Ok(out)
    }
}

// ---------- question_ref：参考答案动作（题目固有属性，不评分） ----------

#[derive(Clone, Debug)]
pub struct QuestionRef {
    pub content: String,
}

impl QuestionRef {
    pub fn new(content: &str) -> Self {
        Self { content: content.to_string() }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct QuestionRefOut {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "clamp::difficulty_1_5")]
    pub difficulty: Option<i32>,
    #[serde(default)]
    pub ref_answer: String,
}

impl AiContract for QuestionRef {
    type Output = QuestionRefOut;

    fn prompt_key(&self) -> &'static str {
        prompts::QUESTION_REF
    }
    fn kind(&self) -> &'static str {
        "ref"
    }
    fn schema_name(&self) -> &'static str {
        "question_ref"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array", "items": { "type": "string" } },
                "difficulty": { "type": ["integer", "null"] },
                "ref_answer": { "type": "string" }
            },
            "required": ["tags", "difficulty", "ref_answer"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        format!("面试题：\n{}", self.content)
    }
    fn long_output(&self) -> bool {
        true // 参考答案要求详尽 → 长文档位
    }
    fn post_process(&self, out: Self::Output) -> Result<Self::Output, AppError> {
        if out.ref_answer.trim().is_empty() {
            return Err(AppError::BadRequest("缺少 ref_answer".to_string()));
        }
        Ok(out)
    }
}

// ---------- answer_evaluate：回答评价（回答级，只产评分/点评） ----------

#[derive(Clone, Debug)]
pub struct AnswerEvaluate {
    pub content: String,
    pub my_answer: String,
    pub existing_ref: Option<String>,
}

impl AnswerEvaluate {
    pub fn new(content: &str, my_answer: &str, existing_ref: Option<&str>) -> Self {
        Self {
            content: content.to_string(),
            my_answer: my_answer.to_string(),
            existing_ref: existing_ref.map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct AnswerEvaluateOut {
    #[serde(default, deserialize_with = "clamp::score_0_100")]
    pub score: Option<i32>,
    #[serde(default)]
    pub feedback: String,
}

impl AiContract for AnswerEvaluate {
    type Output = AnswerEvaluateOut;

    fn prompt_key(&self) -> &'static str {
        prompts::ANSWER_EVALUATE
    }
    fn kind(&self) -> &'static str {
        "answer"
    }
    fn schema_name(&self) -> &'static str {
        "answer_evaluate"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "score": { "type": ["integer", "null"] },
                "feedback": { "type": "string" }
            },
            "required": ["score", "feedback"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        let mut user = format!("面试题：\n{}\n\n候选人现场回答：\n{}", self.content, self.my_answer);
        if let Some(r) = self.existing_ref.as_deref().filter(|r| !r.trim().is_empty()) {
            user += &format!("\n\n已有参考答案（对照评分用）：\n{r}");
        }
        user
    }
    fn long_output(&self) -> bool {
        true
    }
    fn post_process(&self, out: Self::Output) -> Result<Self::Output, AppError> {
        if out.feedback.trim().is_empty() && out.score.is_none() {
            return Err(AppError::BadRequest("缺少 score/feedback".to_string()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全量分析解析：字段提取 + 量纲钳制 + 围栏剥离容错
    #[test]
    fn full_out_parses_clamps_and_tolerates_fences() {
        let v: Value = serde_json::from_str(
            r#"{"tags":["算法","哈希"],"difficulty":3,"ref_answer":"答","score":75,"feedback":"点评"}"#,
        )
        .unwrap();
        let out: QuestionFullOut = serde_json::from_value(v).unwrap();
        assert_eq!(out.tags, vec!["算法", "哈希"]);
        assert_eq!(out.difficulty, Some(3));
        assert_eq!(out.score, Some(75));
        assert_eq!(out.ref_answer, "答");

        // 围栏 + 越界值钳制（难度 9→5、分数 150→100）
        let fenced = "```json\n{\"tags\":[],\"difficulty\":9,\"ref_answer\":\"x\",\"score\":150,\"feedback\":\"y\"}\n```";
        let cleaned = crate::llm::parse_json_loose(fenced).unwrap();
        let out: QuestionFullOut = serde_json::from_value(cleaned).unwrap();
        assert_eq!(out.difficulty, Some(5));
        assert_eq!(out.score, Some(100));

        // null 透传为 None；缺失字段回落默认（宽容非规范网关）
        let loose = serde_json::json!({"ref_answer":"x","score":null,"feedback":"y"});
        let out: QuestionFullOut = serde_json::from_value(loose).unwrap();
        assert_eq!(out.score, None);
        assert_eq!(out.difficulty, None);
        assert!(out.tags.is_empty());
    }

    /// 超范围整数不炸整体反序列化（i64 接住后钳制）
    #[test]
    fn oversized_integers_clamp_instead_of_failing() {
        let v = serde_json::json!({"difficulty": 99999999999i64, "score": -42, "ref_answer": "a", "feedback": "b"});
        let out: QuestionFullOut = serde_json::from_value(v).unwrap();
        assert_eq!(out.difficulty, Some(5));
        assert_eq!(out.score, Some(0));
    }

    /// 关键字段校验沿用旧行为：ref_answer 与 feedback 双空才报错
    #[test]
    fn full_post_process_rejects_both_key_fields_empty() {
        let c = QuestionFull::new("t", None, None);
        let bad = QuestionFullOut { tags: vec![], difficulty: None, ref_answer: "".into(), score: None, feedback: "".into(), ..Default::default() };
        assert!(c.post_process(bad).is_err());
        let ok_feedback_only = QuestionFullOut { tags: vec![], difficulty: None, ref_answer: "".into(), score: None, feedback: "有点评".into(), ..Default::default() };
        assert!(c.post_process(ok_feedback_only).is_ok());
    }

    /// 参考答案出口：缺 ref_answer 报错；user_content 组装语义不变
    #[test]
    fn ref_contract_validation_and_input_shape() {
        let c = QuestionRef::new("讲索引");
        assert_eq!(c.user_content(), "面试题：\n讲索引");
        let bad: QuestionRefOut = serde_json::from_value(json!({"tags":[],"difficulty":2,"ref_answer":" "})).unwrap();
        assert!(c.post_process(bad).is_err());

        // 全量出口输入组装：有回答 / 无回答 / 带已有参考答案三种形态与旧管线一致
        let f = QuestionFull::new("Q", Some("A"), None);
        assert!(f.user_content().contains("候选人现场回答：\nA"));
        let f2 = QuestionFull::new("Q", Some("  "), None);
        assert!(f2.user_content().contains("（未记录现场回答）"), "空白回答视同未记录");
        let f3 = QuestionFull::new("Q", Some("A"), Some("REF"));
        assert!(f3.user_content().contains("已有参考答案（保持基本不变，仅用于对照评分）：\nREF"));
    }

    /// 回答评价出口：score/feedback 双空报错；对照参考答案进 user 消息
    #[test]
    fn answer_contract_validation_and_existing_ref() {
        let c = AnswerEvaluate::new("Q", "A", Some("REF"));
        assert!(c.user_content().contains("已有参考答案（对照评分用）：\nREF"));
        let bad: AnswerEvaluateOut = serde_json::from_value(json!({"score":null,"feedback":""})).unwrap();
        assert!(c.post_process(bad).is_err());
        let ok: AnswerEvaluateOut = serde_json::from_value(json!({"score":88,"feedback":"好"})).unwrap();
        assert_eq!(ok.score, Some(88));
        assert!(c.post_process(ok).is_ok());
    }
}
