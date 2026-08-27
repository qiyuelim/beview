//! 技能域契约：tag_cleanup（自由标签聚合清洗，用户裁决 3）。
//!
//! 流程：后端收集未建树自由标签 → LLM 按技术域归组（canonical + aliases）→
//! **人工核实**后调 apply 接口执行合并。本契约只负责「给建议」，绝不直接改库。

use serde::Deserialize;
use serde_json::{json, Value};

use super::AiContract;
use crate::error::AppError;
use crate::prompts;

/// 输入：未建树自由标签及关联题数
#[derive(Clone, Debug)]
pub struct TagCleanup {
    pub tags: Vec<(String, i64)>,
}

impl TagCleanup {
    pub fn new(tags: Vec<(String, i64)>) -> Self {
        Self { tags }
    }
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct TagGroup {
    /// 规范名（组内真实存在的写法或标准技术名词）
    #[serde(default)]
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct TagCleanupOut {
    #[serde(default)]
    pub groups: Vec<TagGroup>,
}

impl AiContract for TagCleanup {
    type Output = TagCleanupOut;

    fn prompt_key(&self) -> &'static str {
        prompts::TAG_CLEANUP
    }
    fn kind(&self) -> &'static str {
        "tag_cleanup"
    }
    fn schema_name(&self) -> &'static str {
        "tag_cleanup"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "groups": { "type": "array", "items": { "type": "object", "properties": {
                    "canonical": { "type": "string" },
                    "aliases": { "type": "array", "items": { "type": "string" } },
                    "note": { "type": "string" } },
                    "required": ["canonical", "aliases", "note"], "additionalProperties": false } }
            },
            "required": ["groups"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        let mut s = String::from("自由标签清单（JSON 数组）：\n[");
        for (i, (tag, count)) in self.tags.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(&format!(r#"{{"tag":"{tag}","count":{count}}}"#));
        }
        s.push(']');
        s
    }
    fn long_output(&self) -> bool {
        false
    }
    /// 合并产物是数据结构：无结构化能力直接拒绝（ADR-0016 D3）
    fn structured_required_action(&self) -> Option<&'static str> {
        Some("标签聚合清洗")
    }
    fn post_process(&self, mut out: Self::Output) -> Result<Self::Output, AppError> {
        // 防御性清洗：空 canonical 组丢弃；别名中的规范名自身剔除；全空白别名剔除
        out.groups.retain(|g| !g.canonical.trim().is_empty());
        for g in &mut out.groups {
            g.canonical = g.canonical.trim().to_string();
            g.aliases = g
                .aliases
                .iter()
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty() && a != &g.canonical)
                .collect();
        }
        out.groups.retain(|g| !g.aliases.is_empty());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 输入组装：逐个带题数的 JSON 数组
    #[test]
    fn user_content_lists_tags_with_counts() {
        let c = TagCleanup::new(vec![("JVM".into(), 3), ("Redis".into(), 1)]);
        assert_eq!(
            c.user_content(),
            "自由标签清单（JSON 数组）：\n[{\"tag\":\"JVM\",\"count\":3}, {\"tag\":\"Redis\",\"count\":1}]"
        );
    }

    /// post_process：空组/空别名组剔除；别名里的规范名自身剔除；首尾空白清理
    #[test]
    fn post_process_sanitizes_groups() {
        let c = TagCleanup::new(vec![]);
        let out = TagCleanupOut {
            groups: vec![
                TagGroup { canonical: " JVM ".into(), aliases: vec!["JVM".into(), " Java内存模型 ".into(), "".into()], note: "n".into() },
                TagGroup { canonical: "孤儿".into(), aliases: vec![].into(), note: String::new() },
            ],
        };
        let out = c.post_process(out).unwrap();
        assert_eq!(out.groups.len(), 1);
        assert_eq!(out.groups[0].canonical, "JVM");
        assert_eq!(out.groups[0].aliases, vec!["Java内存模型"]);
    }

    /// 结构必需语义
    #[test]
    fn is_structured_required() {
        assert_eq!(TagCleanup::new(vec![]).structured_required_action(), Some("标签聚合清洗"));
    }
}
