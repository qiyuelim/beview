//! 复盘域两出口契约：retrospective（单轮复盘）/ application_overall（投递整体复盘）。
//! 输出字段与 prompts.rs 对应默认 prompt 的「字段语义」一一对应，字段名是解析契约。

use serde::Deserialize;
use serde_json::{json, Value};

use super::AiContract;
use crate::prompts;

// ---------- retrospective：单场面试复盘（轮次级） ----------

/// 输入：调用方组装的逐题记录上下文
#[derive(Clone, Debug)]
pub struct Retrospective {
    pub ctx: String,
}

impl Retrospective {
    pub fn new(ctx: impl Into<String>) -> Self {
        Self { ctx: ctx.into() }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct Strength {
    #[serde(default)]
    pub point: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub why_plus: String,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct Weakness {
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub problem: String,
    #[serde(default)]
    pub impact: String,
    #[serde(default)]
    pub better: String,
}

/// 薄弱点条目：**新旧双形兼容**（前端 RetroPanel 同样双形渲染）——
/// - Legacy：纯字符串 = 问题描述（历史数据/非规范网关产物）；序列化时原样回传字符串，
///   保证落库形状与模型输出一致；
/// - Detailed：现行结构化形态（schema 强约束的对象）。
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum WeaknessEntry {
    Legacy(String),
    Detailed(Weakness),
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct AbilityEvidence {
    #[serde(default)]
    pub ability: String,
    #[serde(default)]
    pub tested: Option<bool>,
    /// 高|中|低|无证据（schema enum 约束；宽容网关下透传原值）
    #[serde(default)]
    pub evidence_strength: Option<String>,
    #[serde(default)]
    pub risk: String,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct InterviewerView {
    #[serde(default)]
    pub positive: Vec<String>,
    #[serde(default)]
    pub doubts: Vec<String>,
    #[serde(default)]
    pub unverified: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct RetrospectiveOut {
    /// 优秀|良好|一般|偏弱（schema enum）
    #[serde(default)]
    pub performance: String,
    /// 高|中高|中|中低|低（schema enum）
    #[serde(default)]
    pub r#match: String,
    /// 高|中|低（schema enum）
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub overall: String,
    #[serde(default)]
    pub strengths: Vec<Strength>,
    #[serde(default)]
    pub weaknesses: Vec<WeaknessEntry>,
    #[serde(default)]
    pub abilities: Vec<AbilityEvidence>,
    #[serde(default)]
    pub interviewer_view: InterviewerView,
    #[serde(default)]
    pub problems: Vec<String>,
    #[serde(default)]
    pub improvements: Vec<String>,
    #[serde(default)]
    pub advice: String,
}

impl AiContract for Retrospective {
    type Output = RetrospectiveOut;

    fn prompt_key(&self) -> &'static str {
        prompts::RETROSPECTIVE
    }
    fn kind(&self) -> &'static str {
        "retrospective"
    }
    fn schema_name(&self) -> &'static str {
        "round_retrospective"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "performance": { "type": "string", "enum": ["优秀", "良好", "一般", "偏弱"] },
                "match": { "type": "string", "enum": ["高", "中高", "中", "中低", "低"] },
                "confidence": { "type": "string", "enum": ["高", "中", "低"] },
                "overall": { "type": "string" },
                "strengths": { "type": "array", "items": { "type": "object", "properties": {
                    "point": { "type": "string" }, "evidence": { "type": "string" }, "why_plus": { "type": "string" } },
                    "required": ["point", "evidence", "why_plus"], "additionalProperties": false } },
                "weaknesses": { "type": "array", "items": { "type": "object", "properties": {
                    "question": { "type": "string" }, "problem": { "type": "string" }, "impact": { "type": "string" }, "better": { "type": "string" } },
                    "required": ["question", "problem", "impact", "better"], "additionalProperties": false } },
                "abilities": { "type": "array", "items": { "type": "object", "properties": {
                    "ability": { "type": "string" }, "tested": { "type": ["boolean", "null"] }, "evidence_strength": { "type": ["string", "null"], "enum": ["高", "中", "低", "无证据", null] }, "risk": { "type": "string" } },
                    "required": ["ability", "tested", "evidence_strength", "risk"], "additionalProperties": false } },
                "interviewer_view": { "type": "object", "properties": {
                    "positive": { "type": "array", "items": { "type": "string" } },
                    "doubts": { "type": "array", "items": { "type": "string" } },
                    "unverified": { "type": "array", "items": { "type": "string" } } },
                    "required": ["positive", "doubts", "unverified"], "additionalProperties": false },
                "problems": { "type": "array", "items": { "type": "string" } },
                "improvements": { "type": "array", "items": { "type": "string" } },
                "advice": { "type": "string" }
            },
            "required": ["performance", "match", "confidence", "overall", "strengths", "weaknesses", "abilities", "interviewer_view", "problems", "improvements", "advice"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        format!("以下是本轮面试的逐题记录：\n\n{}", self.ctx)
    }
    fn long_output(&self) -> bool {
        true
    }
    fn text_hint(&self) -> &str {
        "内容需覆盖：整体表现结论、薄弱点与证据、改进建议、给下一场的建议。"
    }
}

// ---------- application_overall：投递整体复盘（终态后跨轮次归因，十节报告双轨） ----------

#[derive(Clone, Debug)]
pub struct ApplicationOverall {
    pub user_content: String,
}

impl ApplicationOverall {
    pub fn new(user_content: impl Into<String>) -> Self {
        Self { user_content: user_content.into() }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct AbilityMatrixItem {
    #[serde(default)]
    pub ability: String,
    /// 高|中|低（schema enum）
    #[serde(default)]
    pub importance: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub risk: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct ImprovementAction {
    #[serde(default)]
    pub priority: Option<i64>,
    #[serde(default)]
    pub problem: String,
    #[serde(default)]
    pub action: String,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct ApplicationOverallOut {
    #[serde(default)]
    pub performance: String,
    #[serde(default)]
    pub r#match: String,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub strengths: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub loss_points: Vec<String>,
    #[serde(default)]
    pub keep_answers: Vec<String>,
    #[serde(default)]
    pub retrain_answers: Vec<String>,
    #[serde(default)]
    pub ability_matrix: Vec<AbilityMatrixItem>,
    #[serde(default)]
    pub improvements: Vec<ImprovementAction>,
    /// 完整 markdown 全文报告（十节结构）
    #[serde(default)]
    pub report: String,
}

impl AiContract for ApplicationOverall {
    type Output = ApplicationOverallOut;

    fn prompt_key(&self) -> &'static str {
        prompts::APPLICATION_OVERALL
    }
    fn kind(&self) -> &'static str {
        "application_overall"
    }
    fn schema_name(&self) -> &'static str {
        "application_overall"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "performance": { "type": "string", "enum": ["优秀", "良好", "一般", "偏弱"] },
                "match": { "type": "string", "enum": ["高", "中高", "中", "中低", "低"] },
                "confidence": { "type": "string", "enum": ["高", "中", "低"] },
                "summary": { "type": "string" },
                "strengths": { "type": "array", "items": { "type": "string" } },
                "risks": { "type": "array", "items": { "type": "string" } },
                "loss_points": { "type": "array", "items": { "type": "string" } },
                "keep_answers": { "type": "array", "items": { "type": "string" } },
                "retrain_answers": { "type": "array", "items": { "type": "string" } },
                "ability_matrix": { "type": "array", "items": { "type": "object", "properties": {
                    "ability": { "type": "string" }, "importance": { "type": "string", "enum": ["高", "中", "低"] }, "evidence": { "type": "string" }, "risk": { "type": ["string", "null"] } },
                    "required": ["ability", "importance", "evidence", "risk"], "additionalProperties": false } },
                "improvements": { "type": "array", "items": { "type": "object", "properties": {
                    "priority": { "type": ["integer", "null"] }, "problem": { "type": "string" }, "action": { "type": "string" } },
                    "required": ["priority", "problem", "action"], "additionalProperties": false } },
                "report": { "type": "string" }
            },
            "required": ["performance", "match", "confidence", "summary", "strengths", "risks", "loss_points", "keep_answers", "retrain_answers", "ability_matrix", "improvements", "report"],
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
        "按十节结构输出完整复盘报告（整场结论/能力匹配/逐轮对比/回答质量/证据链/一致性/面试官视角/归因/改进项/行动方案）。"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 宽容网关回归：v4 集成测试同款「缺省多字段」payload 必须可解析（缺省回落默认）
    #[test]
    fn retro_tolerates_partial_payload_like_legacy_passthrough() {
        // 旧实现是 Value 直存；契约化后缺省字段回落默认值，断言过的键必须保真
        let v = serde_json::json!({
            "overall":"概念能说清，深度不足",
            "weaknesses":[{"question":"MVCC","problem":"空白","impact":"疑虑","better":"补原理"}],
            "problems":["未讲清 ReadView"],
            "improvements":["补 MVCC 原理"],
            "advice":"下一场主动引导到自己熟悉的项目"
        });
        let out: RetrospectiveOut = serde_json::from_value(v).unwrap();
        match &out.weaknesses[0] { WeaknessEntry::Detailed(w) => assert_eq!(w.question, "MVCC"), _ => panic!("应为结构化形态") }
        assert_eq!(out.advice, "下一场主动引导到自己熟悉的项目");
        assert_eq!(out.performance, "", "未给字段回落默认");
        assert!(out.abilities.is_empty());

        // 完整 payload：枚举字段 + 嵌套结构保真（weaknesses 新形态对象）
        let full = serde_json::json!({
            "performance":"良好","match":"中高","confidence":"高",
            "strengths":[{"point":"概念清晰","evidence":"隔离级别题","why_plus":"体系完整"}],
            "abilities":[{"ability":"数据库","tested":true,"evidence_strength":"中","risk":"深度待证明"}],
            "interviewer_view":{"positive":["基础扎实"],"doubts":["深度不足"],"unverified":["工程实践"]},
            "weaknesses":[{"question":"Q","problem":"P","impact":"I","better":"B"}]
        });
        let out: RetrospectiveOut = serde_json::from_value(full).unwrap();
        assert_eq!(out.r#match, "中高");
        assert_eq!(out.strengths[0].point, "概念清晰");
        assert_eq!(out.abilities[0].evidence_strength.as_deref(), Some("中"));
        assert_eq!(out.interviewer_view.doubts.len(), 1);
        // 新形态：Detailed 对象可访问且序列化回对象
        match &out.weaknesses[0] {
            WeaknessEntry::Detailed(w) => assert_eq!(w.problem, "P"),
            _ => panic!("应为结构化形态"),
        }
        let round = serde_json::to_value(&out).unwrap();
        assert!(round["weaknesses"][0].is_object());
    }

    /// 旧形态（字符串数组）透传保真：反序列化→再序列化后仍是字符串（落库不改写）
    #[test]
    fn weakness_legacy_string_shape_round_trips_verbatim() {
        let v = serde_json::json!({"weaknesses":["MVCC 实现细节空白"]});
        let out: RetrospectiveOut = serde_json::from_value(v).unwrap();
        match &out.weaknesses[0] {
            WeaknessEntry::Legacy(s) => assert_eq!(s, "MVCC 实现细节空白"),
            _ => panic!("应为旧字符串形态"),
        }
        let round = serde_json::to_value(&out).unwrap();
        assert_eq!(round["weaknesses"][0], "MVCC 实现细节空白", "旧形态必须原样回传");
    }

    /// 整体复盘解析：十节结构 + report 全文 + priority 可空
    #[test]
    fn overall_out_parses_ten_section_report() {
        let v = serde_json::json!({
            "performance":"良好","match":"中高","confidence":"高",
            "summary":"整场表现与简历预期基本一致",
            "strengths":["异步基础扎实"],"risks":["深度追问易暴露"],"loss_points":["MVCC 未讲清"],
            "keep_answers":["异步运行时题回答"],"retrain_answers":["隔离级别题回答"],
            "ability_matrix":[{"ability":"Rust 异步","importance":"高","evidence":"MARKER 回答","risk":"深度待补"}],
            "improvements":[{"priority":1,"problem":"数据库深度不足","action":"系统学习 MVCC"}],
            "report":"# 一、整场面试结论\n…（全文）"
        });
        let out: ApplicationOverallOut = serde_json::from_value(v).unwrap();
        assert_eq!(out.ability_matrix[0].importance, "高");
        assert_eq!(out.improvements[0].priority, Some(1));
        assert!(out.report.starts_with("# 一、"));
    }

    /// 文本降级提示与旧实现逐字一致（防漂移）
    #[test]
    fn text_hints_match_legacy_wording() {
        assert_eq!(
            Retrospective::new("").text_hint(),
            "内容需覆盖：整体表现结论、薄弱点与证据、改进建议、给下一场的建议。"
        );
        assert_eq!(
            ApplicationOverall::new("").text_hint(),
            "按十节结构输出完整复盘报告（整场结论/能力匹配/逐轮对比/回答质量/证据链/一致性/面试官视角/归因/改进项/行动方案）。"
        );
    }
}
