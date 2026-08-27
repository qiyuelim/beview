//! 简历契约族：
//! - `resume_parse`：原文 → 结构化 parsed（**结构必需出口**，ADR-0016 D3）。
//! - `resume_changeset`：parsed + 优化意图 → 变更集提案（ADR-0021，结构必需出口）。
//!
//! 变更集寻址 = **模块名白名单**（parsed 顶层键），不绑定强类型——parsed 是模型产物原样
//! Value 存储、字段集随 prompt 契约演进；白名单外的模块一律在应用层拒绝。
//! 旧值断言（old_value）让"提案生成后简历又变过"的场景天然失效：应用时断言不匹配即拒用，
//! 无需额外版本戳。
//!
//! 输出刻意保持 `serde_json::Value` 透传而非强类型结构：`resumes.parsed` 列存储的是
//! 模型产物的原样 JSON（字段集随 prompt 契约演进、前端按存在性渲染），
//! 经强类型往返会改变缺省字段的形状（补 null/丢未知键），违背「模型给什么存什么」的
//! 存储保真语义。契约层在此的职责 = schema 强约束 + 能力位闸门 + 对象校验。

use serde_json::{json, Value};

use super::AiContract;
use crate::error::AppError;
use crate::prompts;

#[derive(Clone, Debug)]
pub struct ResumeParse {
    pub raw: String,
}

impl ResumeParse {
    pub fn new(raw: &str) -> Self {
        Self { raw: raw.to_string() }
    }
}

impl AiContract for ResumeParse {
    /// 存储保真：透传 Value（见模块注释）
    type Output = Value;

    fn prompt_key(&self) -> &'static str {
        prompts::RESUME_PARSE
    }
    fn kind(&self) -> &'static str {
        "resume_parse"
    }
    fn schema_name(&self) -> &'static str {
        "resume_parse"
    }
    fn schema(&self) -> Value {
        // 中国简历标准字段（与 prompts::DEFAULT_RESUME_PARSE 字段语义一一对应；
        // 字段名是解析契约，不得增删改名）
        json!({
            "type": "object",
            "properties": {
                "name": { "type": ["string", "null"] },
                "summary": { "type": ["string", "null"] },
                "gender": { "type": ["string", "null"] },
                "age": { "type": ["string", "null"] },
                "phone": { "type": ["string", "null"] },
                "email": { "type": ["string", "null"] },
                "city": { "type": ["string", "null"] },
                "years": { "type": ["string", "null"] },
                "political": { "type": ["string", "null"] },
                "intent_position": { "type": ["string", "null"] },
                "intent_city": { "type": ["string", "null"] },
                "intent_salary": { "type": ["string", "null"] },
                "education": { "type": "array", "items": { "type": "object", "properties": {
                    "school": { "type": "string" }, "degree": { "type": "string" },
                    "courses": { "type": "array", "items": { "type": "string" } } },
                    "required": ["school", "degree", "courses"], "additionalProperties": false } },
                "experience": { "type": "array", "items": { "type": "object", "properties": {
                    "company": { "type": "string" }, "title": { "type": "string" }, "period": { "type": "string" },
                    "responsibilities": { "type": "array", "items": { "type": "string" } },
                    "achievements": { "type": "array", "items": { "type": "string" } } },
                    "required": ["company", "title", "period", "responsibilities", "achievements"], "additionalProperties": false } },
                "projects": { "type": "array", "items": { "type": "object", "properties": {
                    "name": { "type": "string" }, "role": { "type": "string" },
                    "tech_stack": { "type": "string" }, "start_date": { "type": "string" },
                    "end_date": { "type": "string" }, "detail": { "type": "string" } },
                    "required": ["name", "role", "tech_stack", "start_date", "end_date", "detail"], "additionalProperties": false } },
                "skills": { "type": "array", "items": { "type": "string" } },
                "certificates": { "type": "array", "items": { "type": "object", "properties": {
                    "name": { "type": "string" }, "date": { "type": "string" } },
                    "required": ["name", "date"], "additionalProperties": false } },
                "self_evaluation": { "type": ["string", "null"] },
                "links": { "type": "array", "items": { "type": "object", "properties": {
                    "label": { "type": "string" }, "url": { "type": "string" } },
                    "required": ["label", "url"], "additionalProperties": false } }
            },
            "required": ["name", "summary", "gender", "age", "phone", "email", "city", "years", "political",
                         "intent_position", "intent_city", "intent_salary", "education", "experience",
                         "projects", "skills", "certificates", "self_evaluation", "links"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        self.raw.clone()
    }
    fn long_output(&self) -> bool {
        true
    }
    fn structured_required_action(&self) -> Option<&'static str> {
        Some("解析简历")
    }
    fn post_process(&self, out: Value) -> Result<Value, AppError> {
        if !out.is_object() {
            return Err(AppError::BadRequest("简历解析结果不是对象".to_string()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_must_be_object_and_passthrough_verbatim() {
        let c = ResumeParse::new("张三 后端工程师");
        assert_eq!(c.user_content(), "张三 后端工程师");
        let v = json!({"name":"张三","custom_future_field":1});
        let out = c.post_process(v.clone()).unwrap();
        assert_eq!(out, v, "产物应原样透传（含未来新增字段），不改写不裁剪");
        assert!(c.post_process(json!(["not","object"])).is_err());
        assert_eq!(c.structured_required_action(), Some("解析简历"));
        assert!(c.long_output());
    }
}

// ==================== resume_changeset（ADR-0021 票05） ====================

/// parsed 白名单模块（与 ResumeParse schema 字段一一对应）
pub const KNOWN_MODULES: &[&str] = &[
    "name", "summary", "gender", "age", "phone", "email", "city", "years", "political",
    "intent_position", "intent_city", "intent_salary",
    "education", "experience", "projects", "skills", "certificates", "self_evaluation", "links",
];

fn is_scalar_module(m: &str) -> bool {
    !matches!(m, "education" | "experience" | "projects" | "skills" | "certificates" | "links")
}

/// 单条变更操作（模型输出契约）
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ResumeChange {
    /// update | add | remove
    pub action: String,
    /// 目标模块名（白名单内）
    pub module: String,
    /// 旧值断言：update 要求等于当前值；remove 作为数组元素定位锚（对象=子集匹配 / 字符串=全等）
    #[serde(default)]
    pub old_value: Value,
    /// update/add 的新值
    #[serde(default)]
    pub new_value: Value,
    #[serde(default)]
    pub reason: String,
}

/// 契约输出：变更集提案
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ChangesetProposal {
    pub summary: String,
    pub changes: Vec<ResumeChange>,
}

#[derive(Clone, Debug)]
pub struct ResumeChangeset {
    pub parsed: Value,
    pub intent: String,
}

impl ResumeChangeset {
    pub fn new(parsed: &Value, intent: &str) -> Self {
        Self {
            parsed: parsed.clone(),
            intent: intent.trim().to_string(),
        }
    }
}

impl AiContract for ResumeChangeset {
    type Output = ChangesetProposal;

    fn prompt_key(&self) -> &'static str {
        prompts::RESUME_OPTIMIZE
    }
    fn kind(&self) -> &'static str {
        "resume_optimize"
    }
    fn schema_name(&self) -> &'static str {
        "resume_changeset"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" },
                "changes": { "type": "array", "items": { "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["update", "add", "remove"] },
                    "module": { "type": "string" },
                    "old_value": {},
                    "new_value": {},
                    "reason": { "type": "string" }
                }, "required": ["action", "module", "old_value", "new_value", "reason"],
                   "additionalProperties": false } }
            },
            "required": ["summary", "changes"],
            "additionalProperties": false
        })
    }
    fn user_content(&self) -> String {
        format!(
            "【当前简历结构化数据】\n{}\n\n【优化意图】\n{}",
            serde_json::to_string_pretty(&self.parsed).unwrap_or_default(),
            if self.intent.is_empty() { "全面提升表达质量与竞争力".to_string() } else { self.intent.clone() }
        )
    }
    fn structured_required_action(&self) -> Option<&'static str> {
        Some("生成简历变更集")
    }
    fn post_process(&self, out: ChangesetProposal) -> Result<ChangesetProposal, AppError> {
        // 钳制：单次提案最多 20 条，防止模型失控刷屏
        let mut out = out;
        out.changes.truncate(20);
        Ok(out)
    }
}

/// 应用结果：逐条裁决明细（ADR-0021 D2：拒绝的条目不落库）
#[derive(Debug, serde::Serialize)]
pub struct ApplyOutcome {
    pub applied: usize,
    pub rejected: Vec<RejectedOp>,
    pub parsed: Value,
}

#[derive(Debug, serde::Serialize)]
pub struct RejectedOp {
    pub index: usize,
    pub action: String,
    pub module: String,
    pub reason: String,
}

/// 把变更集应用到 parsed 上（纯函数接缝）：顺序语义——后一条在前一条的结果上校验执行；
/// 单条失败仅记录拒绝原因并跳过，不影响其余条目。
pub fn apply_changeset(parsed: &Value, changes: &[ResumeChange]) -> Result<ApplyOutcome, AppError> {
    let mut root = parsed.clone();
    let mut applied = 0usize;
    let mut rejected = Vec::new();
    for (i, ch) in changes.iter().enumerate() {
        let reject = |reason: String| RejectedOp {
            index: i,
            action: ch.action.clone(),
            module: ch.module.clone(),
            reason,
        };
        if !KNOWN_MODULES.contains(&ch.module.as_str()) {
            rejected.push(reject(format!("未知模块 {}（白名单外）", ch.module)));
            continue;
        }
        let module_val = root.get(&ch.module).cloned().unwrap_or(Value::Null);
        let result: Result<(), RejectedOp> = match ch.action.as_str() {
            "update" => {
                if is_scalar_module(&ch.module) {
                    if module_val != ch.old_value && !(module_val.is_null() && ch.old_value.is_null()) {
                        Err(reject("旧值断言不匹配：该模块当前值已变化，请基于最新简历重新生成提案".into()))
                    } else if !ch.new_value.is_string() {
                        Err(reject("update 的新值必须是字符串".into()))
                    } else {
                        root[&ch.module] = ch.new_value.clone();
                        Ok(())
                    }
                } else {
                    Err(reject("数组模块不支持整体 update，请用 add/remove 操作其条目".into()))
                }
            }
            "add" => {
                if is_scalar_module(&ch.module) {
                    Err(reject("标量模块不支持 add，请用 update".into()))
                } else if module_val.is_null() {
                    root[&ch.module] = json!([ch.new_value.clone()]);
                    Ok(())
                } else if let Value::Array(arr) = &mut root[&ch.module] {
                    arr.push(ch.new_value.clone());
                    Ok(())
                } else {
                    Err(reject(format!("模块 {} 的当前形状不是数组", ch.module)))
                }
            }
            "remove" => {
                if is_scalar_module(&ch.module) {
                    Err(reject("标量模块不支持 remove，请置空请用 update".into()))
                } else if module_val.is_null() {
                    Err(reject(format!("模块 {} 不存在或为空，无可移除条目", ch.module)))
                } else if !module_val.is_array() {
                    Err(reject(format!("模块 {} 的当前形状不是数组", ch.module)))
                } else {
                    let arr = module_val.as_array().unwrap();
                    let matches: Vec<usize> = arr
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| item_matches_anchor(item, &ch.old_value))
                        .map(|(i, _)| i)
                        .collect();
                    match matches.len() {
                        0 => Err(reject("定位失败：没有条目匹配 old_value 锚点".into())),
                        1 => {
                            let idx = matches[0];
                            if let Value::Array(cur) = &mut root[&ch.module] {
                                cur.remove(idx);
                            }
                            Ok(())
                        }
                        n => Err(reject(format!("定位歧义：{n} 个条目匹配 old_value 锚点，请提供更多特征字段"))),
                    }
                }
            }
            other => Err(reject(format!("未知操作类型 {other}"))),
        };
        match result {
            Ok(()) => applied += 1,
            Err(r) => rejected.push(r),
        }
    }
    Ok(ApplyOutcome { applied, rejected, parsed: root })
}

/// 数组元素 ↔ 锚点匹配：字符串数组要求全等；对象数组要求锚点提供的每个键都相等（子集匹配）。
fn item_matches_anchor(item: &Value, anchor: &Value) -> bool {
    match (item, anchor) {
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Object(item), Value::Object(anchor)) => !anchor.is_empty()
            && anchor.iter().all(|(k, av)| item.get(k).is_some_and(|iv| iv == av)),
        _ => false,
    }
}

#[cfg(test)]
mod changeset_tests {
    use super::*;
    use serde_json::json;

    fn change(action: &str, module: &str, old: Value, new: Value) -> ResumeChange {
        ResumeChange {
            action: action.into(),
            module: module.into(),
            old_value: old,
            new_value: new,
            reason: "测试".into(),
        }
    }

    fn sample_parsed() -> Value {
        json!({
            "name": "张三",
            "summary": "三年后端开发",
            "skills": ["Rust", "SQL"],
            "experience": [
                {"company": "甲公司", "title": "后端", "period": "2021-2024"},
                {"company": "乙公司", "title": "实习", "period": "2020-2021"}
            ],
            "projects": []
        })
    }

    #[test]
    fn update_scalar_with_matching_assertion_applies() {
        let parsed = sample_parsed();
        let out = apply_changeset(&parsed, &[change("update", "summary", json!("三年后端开发"), json!("三年后端开发，专注高并发"))]).unwrap();
        assert_eq!(out.applied, 1);
        assert_eq!(out.parsed["summary"], "三年后端开发，专注高并发");
        assert!(out.rejected.is_empty());
    }

    #[test]
    fn update_with_stale_assertion_rejected() {
        let parsed = sample_parsed();
        let out = apply_changeset(&parsed, &[change("update", "summary", json!("旧摘要已过期"), json!("新摘要"))]).unwrap();
        assert_eq!(out.applied, 0);
        assert_eq!(out.parsed["summary"], "三年后端开发", "拒绝后原值不变");
        assert!(out.rejected[0].reason.contains("旧值断言不匹配"));
    }

    #[test]
    fn add_appends_to_array_and_creates_missing_module() {
        let parsed = sample_parsed();
        let item = json!({"company": "丙公司", "title": "架构师", "period": "2024-至今"});
        let out = apply_changeset(&parsed, &[change("add", "experience", Value::Null, item)]).unwrap();
        assert_eq!(out.applied, 1);
        assert_eq!(out.parsed["experience"].as_array().unwrap().len(), 3);

        // 缺失模块从 null 建数组（links 在样例中不存在）
        let out2 = apply_changeset(&parsed, &[change("add", "links", Value::Null, json!({"label":"GitHub","url":"https://github.com/x"}))]).unwrap();
        assert_eq!(out2.applied, 1);
        assert_eq!(out2.parsed["links"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn remove_by_object_anchor_requires_unique_match() {
        let parsed = sample_parsed();
        // 子集锚点命中一条
        let ok = apply_changeset(&parsed, &[change("remove", "experience", json!({"company": "乙公司"}), Value::Null)]).unwrap();
        assert_eq!(ok.applied, 1);
        let arr = ok.parsed["experience"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["company"], "甲公司");

        // 零匹配拒绝
        let miss = apply_changeset(&parsed, &[change("remove", "experience", json!({"company": "不存在的"}), Value::Null)]).unwrap();
        assert!(miss.rejected[0].reason.contains("没有条目匹配"));

        // 歧义拒绝：两段经历 period 相同时按 {"period":"2020-2021"} 会命中多条——构造歧义
        let dup_parsed = json!({
            "certificates": [
                {"name": "奖A", "date": "2023"},
                {"name": "奖B", "date": "2023"}
            ]
        });
        let amb = apply_changeset(&dup_parsed, &[change("remove", "certificates", json!({"date": "2023"}), Value::Null)]).unwrap();
        assert!(amb.rejected[0].reason.contains("定位歧义"));
    }

    #[test]
    fn string_array_remove_uses_exact_equality() {
        let parsed = sample_parsed();
        let out = apply_changeset(&parsed, &[change("remove", "skills", json!("SQL"), Value::Null)]).unwrap();
        assert_eq!(out.applied, 1);
        assert_eq!(out.parsed["skills"], json!(["Rust"]));
    }

    #[test]
    fn unknown_module_and_wrong_shape_rejected() {
        let parsed = sample_parsed();
        // 白名单外模块
        let unknown = apply_changeset(&parsed, &[change("update", "hacker_field", json!("x"), json!("y"))]).unwrap();
        assert!(unknown.rejected[0].reason.contains("未知模块"));
        // 数组模块整体 update 拒绝
        let wrong = apply_changeset(&parsed, &[change("update", "skills", json!(["Rust","SQL"]), json!([]))]).unwrap();
        assert!(wrong.rejected[0].reason.contains("不支持整体 update"));
        // 标量模块 add 拒绝
        let scalar_add = apply_changeset(&parsed, &[change("add", "name", Value::Null, json!("李四"))]).unwrap();
        assert!(scalar_add.rejected[0].reason.contains("标量模块不支持 add"));
    }

    #[test]
    fn sequential_semantics_and_partial_failure() {
        let parsed = sample_parsed();
        let ops = vec![
            change("update", "name", json!("张三"), json!("张三丰")),           // 成功
            change("update", "summary", json!("错误断言"), json!("x")),         // 失败
            change("add", "skills", Value::Null, json!("K8s")),                 // 成功
        ];
        let out = apply_changeset(&parsed, &ops).unwrap();
        assert_eq!(out.applied, 2);
        assert_eq!(out.rejected.len(), 1);
        assert_eq!(out.rejected[0].index, 1);
        assert_eq!(out.parsed["name"], "张三丰");
        assert_eq!(out.parsed["skills"], json!(["Rust", "SQL", "K8s"]));
    }
}
