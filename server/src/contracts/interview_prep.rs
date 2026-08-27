//! 面试官笔记契约（V6-M3，ADR-0023 D3）：关联投递 JD + 简历 parsed（可选关联真实轮次真题与回答）
//! → 四段结构化预读笔记（job_requirements / candidate_facts / risk_signals / next_followups）。
//!
//! **结构必需出口**：笔记唯一用途是落库为 `drills.interview_state` 结构化 JSON 并注入会话上下文，
//! 文本降级没有意义——解析失败显式报错让用户重试。
//!
//! 「解析不全」兜底（D3）：LLM 输出缺段时由 post_process 以规则提取补齐并标记
//! `rule_backfilled = true`；原始输入引用由服务层随 `sources` 一并落库（保留原始引用）。
//!
//! 与「考官题本」（dossier）互补不互替：**题本管问什么，笔记管问谁**。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::AiContract;
use crate::prompts;

/// 规则兜底原料（服务层从 DB 装配；post_process 缺段时使用）
#[derive(Clone, Debug, Default)]
pub struct RuleFacts {
    pub position: String,
    pub company: String,
    /// 场次方向等可廉价获得的关键词（如「系统设计」）；无则空
    pub keywords: Vec<String>,
    /// 关联真实轮次真题所挂技能点名称（规则提取的技能关键字）
    pub skill_keywords: Vec<String>,
    pub resume_excerpt: Option<String>,
    /// 关联真实轮次的真题主题（已截断）
    pub round_topics: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct InterviewPrep {
    pub user_content: String,
    pub facts: RuleFacts,
}

impl InterviewPrep {
    pub fn new(user_content: impl Into<String>, facts: RuleFacts) -> Self {
        Self { user_content: user_content.into(), facts }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct InterviewNotes {
    #[serde(default)]
    pub job_requirements: Vec<String>,
    #[serde(default)]
    pub candidate_facts: Vec<String>,
    #[serde(default)]
    pub risk_signals: Vec<String>,
    #[serde(default)]
    pub next_followups: Vec<String>,
    /// 任一段由规则提取补齐 = true（前端徽章「部分规则兜底」；全 LLM 时序列化中省略）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rule_backfilled: bool,
}

impl AiContract for InterviewPrep {
    type Output = InterviewNotes;

    fn prompt_key(&self) -> &'static str {
        prompts::INTERVIEW_PREP
    }
    fn kind(&self) -> &'static str {
        "interview_prep"
    }
    fn schema_name(&self) -> &'static str {
        "interview_prep"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "job_requirements": { "type": "array", "items": { "type": "string" } },
                "candidate_facts": { "type": "array", "items": { "type": "string" } },
                "risk_signals": { "type": "array", "items": { "type": "string" } },
                "next_followups": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["job_requirements", "candidate_facts", "risk_signals", "next_followups"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        self.user_content.clone()
    }
    fn structured_required_action(&self) -> Option<&'static str> {
        Some("生成面试官笔记")
    }
    fn text_hint(&self) -> &str {
        "请输出中文 Markdown 备课笔记，包含「岗位要求」「候选人事实」「风险信号」「建议追问」四个小节。"
    }
    fn post_process(&self, mut out: Self::Output) -> Result<Self::Output, crate::error::AppError> {
        // 钳制：各段最多 10 条，防失控刷屏
        out.job_requirements.truncate(10);
        out.candidate_facts.truncate(10);
        out.risk_signals.truncate(10);
        out.next_followups.truncate(10);
        fill_missing_sections(&mut out, &self.facts);
        Ok(out)
    }
}

/// D3 兜底：LLM 缺段 → 规则提取补齐 + 标记 rule_backfilled（不覆盖 LLM 已产出的内容）
fn fill_missing_sections(notes: &mut InterviewNotes, facts: &RuleFacts) {
    let mut backfilled = false;

    if notes.job_requirements.is_empty() {
        let mut items = vec![format!(
            "岗位：{}（{}）（规则提取自本场设置）",
            if facts.position.is_empty() { "通用" } else { &facts.position },
            if facts.company.is_empty() { "未关联公司" } else { &facts.company }
        )];
        if !facts.keywords.is_empty() {
            items.push(format!("考察侧重关键词：{}", facts.keywords.join(" / ")));
        }
        if !facts.skill_keywords.is_empty() {
            items.push(format!("关联技能点：{}", facts.skill_keywords.join(" / ")));
        }
        notes.job_requirements = items;
        backfilled = true;
    }

    if notes.candidate_facts.is_empty() {
        match facts.resume_excerpt.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(resume) => {
                // 保留原始引用：取简历摘要前几条非空行作为客观事实线索
                let lines: Vec<String> = resume
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .take(3)
                    .map(|l| format!("简历摘录：{}", truncate_line(l)))
                    .collect();
                notes.candidate_facts = lines;
            }
            None => {
                notes.candidate_facts = vec![
                    "简历缺失（未关联投递或未完成解析）——请在「简历」页保存并解析后重新生成笔记".to_string(),
                ];
            }
        }
        backfilled = true;
    }

    if notes.risk_signals.is_empty() {
        notes.risk_signals = vec![
            "规则兜底：现有输入不足以推断风险项，建议补充 JD/简历后重新生成".to_string(),
        ];
        backfilled = true;
    }

    if notes.next_followups.is_empty() {
        if facts.round_topics.is_empty() {
            notes.next_followups = vec!["请面试官基于候选人前述回答自行选择深挖方向".to_string()];
        } else {
            notes.next_followups = facts
                .round_topics
                .iter()
                .take(3)
                .map(|t| format!("围绕真实轮次真题「{t}」追问实现细节与权衡"))
                .collect();
        }
        backfilled = true;
    }

    notes.rule_backfilled = backfilled;
}

/// 把 parsed 简历收成短摘要，禁止把整份 JSON / 原文灌进备课上下文。
pub fn compact_parsed_resume(parsed: &Value) -> String {
    let mut lines: Vec<String> = Vec::new();
    let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    let years = parsed.get("years").and_then(|v| v.as_str()).unwrap_or("").trim();
    let intent = parsed.get("intent_position").and_then(|v| v.as_str()).unwrap_or("").trim();
    let head = [name, years, intent].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    if !head.is_empty() {
        lines.push(head);
    }
    if let Some(arr) = parsed.get("experience").and_then(|v| v.as_array()) {
        for e in arr.iter().take(3) {
            let company = e.get("company").and_then(|v| v.as_str()).unwrap_or("").trim();
            let title = e.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
            let mut line = [company, title].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" / ");
            if let Some(resp) = e.get("responsibilities").and_then(|v| v.as_array()) {
                let bits: Vec<&str> = resp.iter().filter_map(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()).take(2).collect();
                if !bits.is_empty() {
                    if !line.is_empty() {
                        line.push_str("：");
                    }
                    line.push_str(&bits.join("；"));
                }
            }
            if !line.is_empty() {
                lines.push(format!("经历 {line}"));
            }
        }
    }
    if let Some(arr) = parsed.get("projects").and_then(|v| v.as_array()) {
        for p in arr.iter().take(2) {
            let pname = p.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let detail = p.get("detail").and_then(|v| v.as_str()).unwrap_or("").trim();
            if pname.is_empty() && detail.is_empty() {
                continue;
            }
            let d = if detail.chars().count() > 60 {
                format!("{}…", detail.chars().take(60).collect::<String>())
            } else {
                detail.to_string()
            };
            lines.push(format!("项目 {} {}", pname, d).trim().to_string());
        }
    }
    if let Some(skills) = parsed.get("skills").and_then(|v| v.as_array()) {
        let s: Vec<&str> = skills.iter().filter_map(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()).take(8).collect();
        if !s.is_empty() {
            lines.push(format!("技能 {}", s.join("、")));
        }
    }
    lines.join("\n")
}

/// 复用岗位 JD 解读要点，不把 JD 原文灌进备课。
pub fn compact_jd_interpret(v: &Value) -> Option<String> {
    let overall = v.get("overall").and_then(|x| x.as_str()).unwrap_or("").trim();
    let cautions: Vec<&str> = v
        .get("cautions")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|i| i.as_str()).map(str::trim).filter(|s| !s.is_empty()).take(5).collect())
        .unwrap_or_default();
    if overall.is_empty() && cautions.is_empty() {
        return None;
    }
    let mut s = String::new();
    if !overall.is_empty() {
        s.push_str(overall);
    }
    for c in cautions {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str("- ");
        s.push_str(c);
    }
    Some(s)
}

fn truncate_line(s: &str) -> String {
    if s.chars().count() <= 80 {
        s.to_string()
    } else {
        let cut: String = s.chars().take(77).collect();
        format!("{cut}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_resume_keeps_highlights_not_blob() {
        let v = json!({
            "name": "张三",
            "years": "5 年",
            "intent_position": "后端",
            "experience": [{ "company": "甲", "title": "工程师", "responsibilities": ["做网关", "带人"] }],
            "projects": [{ "name": "支付", "detail": "高并发清算" }],
            "skills": ["Rust", "Tokio"]
        });
        let s = compact_parsed_resume(&v);
        assert!(s.contains("张三"));
        assert!(s.contains("甲"));
        assert!(s.contains("Rust"));
        assert!(!s.contains("raw"));
    }

    #[test]
    fn compact_jd_uses_interpret_not_full_text() {
        let v = json!({ "overall": "平台岗", "cautions": ["偏运维"] });
        let s = compact_jd_interpret(&v).unwrap();
        assert!(s.contains("平台岗"));
        assert!(s.contains("偏运维"));
    }

    fn facts() -> RuleFacts {
        RuleFacts {
            position: "Rust 后端".into(),
            company: "示例科技".into(),
            keywords: vec!["系统设计".into()],
            skill_keywords: vec!["Tokio".into()],
            resume_excerpt: Some("三年 Rust 经验\n主导支付网关\n\n熟悉 Tokio".into()),
            round_topics: vec!["零拷贝与 mmap".into()],
        }
    }

    #[test]
    fn partial_llm_output_gets_rule_fallback_and_marker() {
        let mut notes = InterviewNotes {
            job_requirements: vec!["LLM 提炼的岗位要求".into()],
            ..Default::default()
        };
        fill_missing_sections(&mut notes, &facts());
        assert_eq!(notes.job_requirements, vec!["LLM 提炼的岗位要求".to_string()], "已有段不得被覆盖");
        assert!(notes.candidate_facts[0].starts_with("简历摘录："), "应从简历原文引用");
        assert!(notes.candidate_facts.iter().any(|f| f.contains("主导支付网关")));
        assert!(notes.risk_signals[0].contains("规则兜底"));
        assert!(notes.next_followups[0].contains("零拷贝与 mmap"), "追问锚定真实真题");
        assert!(notes.rule_backfilled);
    }

    #[test]
    fn full_llm_output_untouched_no_marker() {
        let mut notes = InterviewNotes {
            job_requirements: vec!["a".into()],
            candidate_facts: vec!["b".into()],
            risk_signals: vec!["c".into()],
            next_followups: vec!["d".into()],
            rule_backfilled: false,
        };
        fill_missing_sections(&mut notes, &facts());
        assert!(!notes.rule_backfilled);
        // 全 LLM 时 marker 字段不出现在序列化结果里（skip_serializing_if）
        let v = serde_json::to_value(&notes).unwrap();
        assert!(v.get("rule_backfilled").is_none(), "false 时应省略字段");
    }

    #[test]
    fn missing_resume_yields_explicit_gap_hint() {
        let mut notes = InterviewNotes::default();
        fill_missing_sections(&mut notes, &RuleFacts { position: "后端".into(), ..Default::default() });
        assert!(notes.candidate_facts[0].contains("简历缺失"));
        assert!(notes.job_requirements[0].contains("未关联公司"));
        assert!(notes.next_followups[0].contains("自行选择深挖方向"));
    }

    #[test]
    fn sections_clamped_to_ten_via_post_process() {
        let notes = InterviewNotes {
            job_requirements: (0..25).map(|i| format!("r{i}")).collect(),
            candidate_facts: (0..30).map(|i| format!("f{i}")).collect(),
            risk_signals: (0..12).map(|i| format!("k{i}")).collect(),
            next_followups: vec!["x".into()],
            rule_backfilled: false,
        };
        let contract = InterviewPrep::new("", facts());
        let out = contract.post_process(notes).unwrap();
        assert_eq!(out.job_requirements.len(), 10);
        assert_eq!(out.candidate_facts.len(), 10);
        assert_eq!(out.risk_signals.len(), 10);
        // 非空段不触发规则兑底
        assert!(!out.rule_backfilled);
    }
}
