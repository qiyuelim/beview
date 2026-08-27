//! 投递智能洞察契约（票07）：applications + 状态流水 + 复盘摘要 → 四段结构化报告。
//!
//! 输入由服务端装配为紧凑文本（每条投递一行流水），输出强类型四段；
//! **文本降级可用**（洞察是评价式内容而非数据必需，text 模式下前端按 Markdown 渲染）。

use serde::Deserialize;
use serde_json::{json, Value};

use super::AiContract;
use crate::prompts;

#[derive(Clone, Debug)]
pub struct ApplicationInsights {
    pub user_content: String,
}

impl ApplicationInsights {
    pub fn new(user_content: impl Into<String>) -> Self {
        Self { user_content: user_content.into() }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct PriorityAction {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct InsightReport {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub observations: Vec<String>,
    #[serde(default)]
    pub recommendations: Vec<String>,
    #[serde(default)]
    pub priority: Vec<PriorityAction>,
}

impl AiContract for ApplicationInsights {
    type Output = InsightReport;

    fn prompt_key(&self) -> &'static str {
        prompts::APPLICATION_INSIGHTS
    }
    fn kind(&self) -> &'static str {
        "app_insights"
    }
    fn schema_name(&self) -> &'static str {
        "application_insights"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "observations": { "type": "array", "items": { "type": "string" } },
                "recommendations": { "type": "array", "items": { "type": "string" } },
                "priority": { "type": "array", "items": { "type": "object", "properties": {
                    "action": { "type": "string" }, "reason": { "type": "string" } },
                    "required": ["action", "reason"], "additionalProperties": false } }
            },
            "required": ["summary", "observations", "recommendations", "priority"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        self.user_content.clone()
    }
    fn text_hint(&self) -> &str {
        "请输出中文 Markdown 洞察报告，包含「总体评价」「观察」「建议」「优先行动」四个小节。"
    }
    fn post_process(&self, mut out: Self::Output) -> Result<Self::Output, crate::error::AppError> {
        // 钳制：各列表最多 8 条，防失控刷屏
        out.observations.truncate(8);
        out.recommendations.truncate(8);
        out.priority.truncate(5);
        Ok(out)
    }
}
