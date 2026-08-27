//! JD 驱动两出口契约：jd_interpret（评价式解读）/ jd_match（简历-JD 匹配度）。

use serde::Deserialize;
use serde_json::{json, Value};

use super::{clamp, AiContract};
use crate::prompts;

// ---------- jd_interpret：JD 解读（评价式） ----------

#[derive(Clone, Debug)]
pub struct JdInterpret {
    pub jd: String,
}

impl JdInterpret {
    pub fn new(jd: &str) -> Self {
        Self { jd: jd.to_string() }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct JdInterpretOut {
    #[serde(default)]
    pub overall: String,
    #[serde(default)]
    pub cautions: Vec<String>,
}

impl AiContract for JdInterpret {
    type Output = JdInterpretOut;

    fn prompt_key(&self) -> &'static str {
        prompts::JD_INTERPRET
    }
    fn kind(&self) -> &'static str {
        "jd_interpret"
    }
    fn schema_name(&self) -> &'static str {
        "jd_interpret"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "overall": { "type": "string" },
                "cautions": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["overall", "cautions"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        self.jd.clone()
    }
    fn long_output(&self) -> bool {
        true
    }
    fn text_hint(&self) -> &str {
        "内容需覆盖：岗位定位与水平判断、注意点/风险信号。"
    }
}

// ---------- jd_match：简历-JD 匹配度（score 量纲 0-100） ----------

/// 输入：调用方组装的完整 user 消息（【目标岗位 JD】+【候选人简历（结构化）】）
#[derive(Clone, Debug)]
pub struct JdMatch {
    pub user_content: String,
}

impl JdMatch {
    pub fn new(user_content: String) -> Self {
        Self { user_content }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct JdMatchOut {
    #[serde(default, deserialize_with = "clamp::score_0_100")]
    pub score: Option<i32>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub strengths: Vec<String>,
    #[serde(default)]
    pub gaps: Vec<String>,
    #[serde(default)]
    pub resume_advice: Vec<String>,
}

impl AiContract for JdMatch {
    type Output = JdMatchOut;

    fn prompt_key(&self) -> &'static str {
        prompts::JD_MATCH
    }
    fn kind(&self) -> &'static str {
        "jd_match"
    }
    fn schema_name(&self) -> &'static str {
        "jd_match"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "score": { "type": ["integer", "null"] },
                "summary": { "type": "string" },
                "strengths": { "type": "array", "items": { "type": "string" } },
                "gaps": { "type": "array", "items": { "type": "string" } },
                "resume_advice": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["score", "summary", "strengths", "gaps", "resume_advice"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        self.user_content.clone()
    }
    fn long_output(&self) -> bool {
        true
    }
    fn text_hint(&self) -> &str {
        "内容需覆盖：匹配度总评、优势、差距清单、简历修改建议。"
    }
}

// ---------- position_predict：岗位精准押题 (高频考题预测) ----------

#[derive(Clone, Debug)]
pub struct PositionPredict {
    pub user_content: String,
}

impl PositionPredict {
    pub fn new(user_content: String) -> Self {
        Self { user_content }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct PredictedQuestionItem {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub focus_points: Vec<String>,
    #[serde(default)]
    pub sample_direction: String,
    #[serde(default, deserialize_with = "clamp::score_0_100")]
    pub probability: Option<i32>,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct PositionPredictOut {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub questions: Vec<PredictedQuestionItem>,
}

impl AiContract for PositionPredict {
    type Output = PositionPredictOut;

    fn prompt_key(&self) -> &'static str {
        prompts::POSITION_PREDICT
    }
    fn kind(&self) -> &'static str {
        "position_predict"
    }
    fn schema_name(&self) -> &'static str {
        "position_predict"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "category": { "type": "string" },
                            "focus_points": { "type": "array", "items": { "type": "string" } },
                            "sample_direction": { "type": "string" },
                            "probability": { "type": ["integer", "null"] }
                        },
                        "required": ["content", "category", "focus_points", "sample_direction", "probability"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["summary", "questions"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        self.user_content.clone()
    }
    fn long_output(&self) -> bool {
        true
    }
    fn text_hint(&self) -> &str {
        "内容需覆盖：岗位考点核心总结、预测高频考题清单、每题考察要点与建议回答方向。"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 匹配度解析：score 钳制 0-100（量纲不变），字段缺省回落
    #[test]
    fn match_out_clamps_score_and_defaults_fields() {
        let v = json!({"score":150,"summary":"高匹配","strengths":["Rust"],"gaps":[],"resume_advice":["补压测数据"]});
        let out: JdMatchOut = serde_json::from_value(v).unwrap();
        assert_eq!(out.score, Some(100), "越界分数钳到 100");
        assert_eq!(out.summary, "高匹配");
        assert_eq!(out.resume_advice, vec!["补压测数据"]);

        let v2 = json!({"score":null,"summary":"s"});
        let out2: JdMatchOut = serde_json::from_value(v2).unwrap();
        assert_eq!(out2.score, None);
        assert!(out2.gaps.is_empty());

        // 文本降级提示与旧实现逐字一致
        assert_eq!(
            JdInterpret::new("").text_hint(),
            "内容需覆盖：岗位定位与水平判断、注意点/风险信号。"
        );
        assert_eq!(
            JdMatch::new(String::new()).text_hint(),
            "内容需覆盖：匹配度总评、优势、差距清单、简历修改建议。"
        );
    }
}
