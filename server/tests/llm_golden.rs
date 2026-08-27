//! v5.5-M1（票08）：LLM 黄金集回归设施。
//!
//! 三层黄金网（纯本地，零真实 LLM 调用、零 DB 依赖）：
//! 1. schema 快照——全部已注册结构化出口的 strict schema 全文；
//! 2. prompt 快照——全部注册提示词的内置默认模板全文；
//! 3. 往返快照——schema 驱动生成最小合法样本 → parse_json_loose → 强类型反序列化 →
//!    post_process 钳制 → 序列化结果。锁死「schema ↔ Rust Output 类型」兼容性。
//!
//! 结构漂移（schema/类型不匹配、解析失败）= 测试红；内容漂移 = diff 明细打印，
//! 人工裁决后按下方刷新流程显式更新。
//!
//! 刷新黄金集的显式流程：确认漂移是**有意变更**后运行
//!   `GOLDEN_UPDATE=1 cargo test --test llm_golden`
//! （进程重写全部 golden 文件并在输出列出更新清单），随功能改动同 commit 提交。

use serde_json::{json, Value};
use server::contracts::{self, AiContract};

const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

fn update_mode() -> bool {
    std::env::var("GOLDEN_UPDATE").is_ok()
}

/// 与 golden 文件比对；UPDATE 模式下写入并放行（输出更新清单供 commit 审查）
fn check_golden(rel_path: &str, actual: &str, label: &str) -> Result<(), String> {
    let path = format!("{GOLDEN_DIR}/{rel_path}");
    if update_mode() {
        if let Some(dir) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, actual).map_err(|e| e.to_string())?;
        println!("[golden] 已更新 {rel_path}");
        return Ok(());
    }
    match std::fs::read_to_string(&path) {
        Ok(expected) => {
            if expected == actual {
                Ok(())
            } else {
                Err(format!(
                    "{label} 与黄金基线不一致（{rel_path}）。\n\
                     —— 请人工裁决：若属有意变更，先审阅 diff 再以 GOLDEN_UPDATE=1 重跑刷新。\n\
                     === 期望（golden）===\n{expected}\n=== 实际 ===\n{actual}"
                ))
            }
        }
        Err(_) => Err(format!(
            "{label} 缺少黄金基线文件 {rel_path}。\n\
             —— 确认新增属于有意变更后，以 GOLDEN_UPDATE=1 cargo test --test llm_golden 生成。"
        )),
    }
}

/// 从 strict schema 生成最小合法样本：union 含 null 取 null；enum 取首项；
/// string→占位、number/integer→0、boolean→false、array→单元素、object→递归。
fn gen_from_schema(schema: &Value) -> Value {
    use Value::*;
    if let Some(enum_vals) = schema.get("enum").and_then(|v| v.as_array()) {
        return enum_vals.first().cloned().unwrap_or(Null);
    }
    let t = match schema.get("type") {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Array(opts)) => opts
            .iter()
            .find(|o| o.as_str() == Some("null"))
            .cloned()
            .or_else(|| opts.first().cloned())
            .and_then(|o| o.as_str().map(std::string::ToString::to_string)),
        _ => None,
    };
    match t.as_deref() {
        Some("null") => Null,
        Some("string") => String("占位".into()),
        Some("number") | Some("integer") => Number(serde_json::Number::from(0)),
        Some("boolean") => Bool(false),
        Some("array") => {
            let item = schema.get("items").map(gen_from_schema).unwrap_or(Null);
            Array(vec![item])
        }
        Some("object") => {
            let mut m = serde_json::Map::new();
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                for (k, sub) in props {
                    m.insert(k.clone(), gen_from_schema(sub));
                }
            }
            Object(m)
        }
        // 无 type 约束的自由值（如变更集 old_value/new_value）
        _ => Null,
    }
}

/// 泛型往返管线：松散解析 → 反序列化 → post_process 钳制 → 序列化
fn rt_pipeline<C>(c: C, sample: &Value) -> Result<Value, String>
where
    C: AiContract,
    C::Output: serde::Serialize,
{
    let raw = serde_json::to_string_pretty(sample).expect("样本序列化失败");
    let v = server::llm::parse_json_loose(&raw)?;
    let out: C::Output = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let out = c.post_process(out).map_err(|e| e.to_string())?;
    serde_json::to_value(out).map_err(|e| e.to_string())
}

/// 参与往返层的出口（V6 M2：扩展至全部已注册契约出口，paper 已退役）。
fn roundtrip_targets() -> Vec<(&'static str, Box<dyn Fn(&Value) -> Result<Value, String>>)> {
    use contracts::insights::ApplicationInsights;
    use contracts::jd::{JdInterpret, JdMatch, PositionPredict};
    use contracts::question::{AnswerEvaluate, QuestionFull, QuestionRef};
    use contracts::resume::{ResumeChangeset, ResumeParse};
    use contracts::retro::{ApplicationOverall, Retrospective};
    vec![
        (
            "analyze",
            Box::new(|s| rt_pipeline(QuestionFull::new("", None, None), s)),
        ),
        (
            "ref",
            Box::new(|s| rt_pipeline(QuestionRef::new(""), s)),
        ),
        (
            "answer",
            Box::new(|s| rt_pipeline(AnswerEvaluate::new("", "", None), s)),
        ),
        (
            "resume_parse",
            Box::new(|s| rt_pipeline(ResumeParse::new(""), s)),
        ),
        (
            "resume_optimize",
            Box::new(move |s| {
                rt_pipeline(
                    ResumeChangeset::new(&json!({}), ""),
                    s,
                )
            }),
        ),
        (
            "jd_interpret",
            Box::new(|s| rt_pipeline(JdInterpret::new(""), s)),
        ),
        (
            "jd_match",
            Box::new(|s| rt_pipeline(JdMatch::new(String::new()), s)),
        ),
        (
            "retrospective",
            Box::new(|s| rt_pipeline(Retrospective::new(""), s)),
        ),
        (
            "application_overall",
            Box::new(|s| rt_pipeline(ApplicationOverall::new(""), s)),
        ),
        (
            "position_predict",
            Box::new(|s| rt_pipeline(PositionPredict::new(String::new()), s)),
        ),
        (
            "app_insights",
            Box::new(|s| rt_pipeline(ApplicationInsights::new(""), s)),
        ),
        (
            "interview_prep",
            Box::new(|s| {
                rt_pipeline(contracts::interview_prep::InterviewPrep::new(
                    "",
                    contracts::interview_prep::RuleFacts::default(),
                ), s)
            }),
        ),
    ]
}

#[test]
fn golden_schemas_cover_all_registered_exits() {
    for (kind, schema_name, _prompt_key, schema) in contracts::registry_specs() {
        let pretty = serde_json::to_string_pretty(&schema).unwrap();
        check_golden(
            &format!("schemas/{schema_name}.json"),
            &pretty,
            &format!("出口 {kind} 的 schema"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
}

#[test]
fn golden_prompts_cover_all_registered_defaults() {
    let mut seen = std::collections::HashSet::new();
    for def in server::prompts::DEFS {
        let text = server::prompts::default_of(def.key);
        assert!(text.len() > 10, "prompt {} 默认值过短", def.key);
        check_golden(&format!("prompts/{}.md", def.key), &text, &format!("prompt {}", def.key))
            .unwrap_or_else(|e| panic!("{e}"));
        seen.insert(def.key);
    }
    assert_eq!(seen.len(), 14, "应登记 14 个提示词（V6.1 票 01 退役 paper_generate/paper_grade 后）");
}

#[test]
fn golden_roundtrip_pins_schema_to_typed_output() {
    let kinds_registered = contracts::registry_specs();
    let targets = roundtrip_targets();

    // 往返目标必须都登记于 registry（防手滑写错 kind 名）
    for (kind, _) in &targets {
        assert!(
            kinds_registered.iter().any(|(k, _, _, _)| *k == *kind),
            "roundtrip 目标 {kind} 不在 registry_specs 中"
        );
    }

    for (kind, pipeline) in targets {
        let spec = kinds_registered
            .iter()
            .find(|(k, _, _, _)| **k == *kind)
            .unwrap_or_else(|| panic!("{kind} 未登记"));
        let sample = gen_from_schema(&spec.3);
        let actual = match pipeline(&sample) {
            Ok(v) => serde_json::to_string_pretty(&v).unwrap(),
            Err(e) => panic!(
                "{kind}: 最小合法样本往返失败——schema 与 Rust Output 类型可能失配：{e}\n样本：{sample}"
            ),
        };
        check_golden(&format!("roundtrip/{kind}.json"), &actual, &format!("出口 {kind} 往返"))
            .unwrap_or_else(|e| panic!("{e}"));
    }
}
