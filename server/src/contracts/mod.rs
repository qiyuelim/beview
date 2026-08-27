//! AI 契约层（ADR-0017 D2 Contract-First SPI，Ticket 02）。
//!
//! 每个 AI 出口 = 一个高内聚微模块：System Prompt 注册 key + Strict JSON Schema +
//! 强类型 Rust 输出 + 纯文本评审模式降级策略，全部封装在同一个 `AiContract` 实现里；
//! 业务 route 只构造契约实例并调用 [`execute`]，不再接触 schema/prompt/解析细节。
//! `llm` 由此瘦身为纯粹的 Responses API 引擎（请求组装/连接池/流式分发/观测/错误转换）。
//!
//! 纪律（延续 ADR-0016）：
//! - prompt 是语义层、schema 是格式层，量纲唯一（综合分 0-100 / 难度 1-5，AGENTS 基准 4）；
//! - 结构必需出口（试卷生成/批量判卷/简历解析）无结构化能力直接拒绝（D3）；
//! - 新增出口必须在 [`registry_specs`] 登记，纳入注册表完整性测试。

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::error::AppError;
use crate::llm;
use crate::settings::LlmConfig;
use crate::prompts;

pub mod insights;
pub mod interview_prep;
pub mod jd;
pub mod question;
pub mod retro;
pub mod resume;
pub mod skills;

/// 契约执行结果（ADR-0016 D3 双轨）：
/// - Structured：strict schema 强约束下的强类型输出；
/// - Text：纯文本评审模式的 Markdown 全文（调用方按 ir_mode=text 落库整体渲染）。
#[derive(Debug)]
pub enum ContractOut<T> {
    Structured(T),
    Text(String),
}

impl<T> ContractOut<T> {
    /// 取结构化结果；结构必需出口理论上不会走到 Text 分支（受理前已被闸门拒绝），
    /// 防御性转换为显式错误而非 panic。
    pub fn structured(self) -> Result<T, AppError> {
        match self {
            ContractOut::Structured(v) => Ok(v),
            ContractOut::Text(_) => Err(AppError::BadRequest(
                "该出口不支持纯文本评审模式".to_string(),
            )),
        }
    }
}

/// 统一 AI 出口契约。实现方 = 该出口的唯一事实源：
/// prompt 引用、schema、输入组装、输出类型、降级策略与校验钳制都在这里。
pub trait AiContract {
    /// strict schema 强约束下的强类型输出
    type Output: DeserializeOwned + Send;

    /// prompts.rs 注册表 key（system prompt 来源，设置页「处处可编辑」）
    fn prompt_key(&self) -> &'static str;

    /// 出口标识：引擎 span 名 / meta.ir_kind / 日志字段
    fn kind(&self) -> &'static str;

    /// text.format.name（strict schema 名；历史命名见各出口实现，与 kind 不必相同）
    fn schema_name(&self) -> &'static str;

    /// strict json_schema（additionalProperties:false + 全字段 required，可空用 null 联合）
    fn schema(&self) -> Value;

    /// 组装发给模型的 user 消息（输入语义归契约所有）
    fn user_content(&self) -> String;

    /// 纯文本评审模式下对 system 的覆盖提示（描述文本应覆盖的要点）
    fn text_hint(&self) -> &str {
        ""
    }

    /// 长文任务档（max_tokens_long），默认短档（max_tokens）
    fn long_output(&self) -> bool {
        false
    }

    /// 结构必需出口（ADR-0016 D3）：Some(动作名) = 无结构化能力直接拒绝
    /// （如 Some("批量判卷") → 「…无法批量判卷…」）；None = 可降级纯文本评审
    fn structured_required_action(&self) -> Option<&'static str> {
        None
    }

    /// 解析后的校验/钳制（缺省原样通过）
    fn post_process(&self, out: Self::Output) -> Result<Self::Output, AppError> {
        Ok(out)
    }
}

/// 能力位闸门（纯函数，便于无网络单测）：结构必需出口在未开能力位时同步拒绝。
pub fn ensure_capability<C: AiContract>(config: &LlmConfig, contract: &C) -> Result<(), AppError> {
    if config.structured_output {
        return Ok(());
    }
    match contract.structured_required_action() {
        Some(action) => Err(AppError::BadRequest(format!(
            "当前模型未启用「结构化输出」能力，无法{action}；请在设置中开启该能力位或更换模型"
        ))),
        None => Ok(()),
    }
}

/// 纯文本评审模式的 system 包装（ADR-0016 D3）：正向无条件覆盖——不依赖原文是否含
/// 「输出 JSON」字样（用户自定义 prompt 可能已删掉该句），一律改写为 Markdown 文本要求。
pub fn wrap_text_system(system: &str, hint: &str) -> String {
    format!(
        "{system}\n\n【输出模式】本次调用未启用结构化输出：无论上文如何约定输出格式，本次请直接输出中文 Markdown 评审文本，\
         不要输出 JSON，也不要用代码块包裹整个回答。{hint}"
    )
}

/// 结构化输出解析失败统一提示：部分网关会静默忽略 text.format=json_schema，
/// 模型自由发挥出 YAML 等格式——把该可能直接告诉用户，别只留一句解析失败。
fn parse_err(e: impl std::fmt::Display) -> AppError {
    AppError::BadRequest(format!(
        "{e}\n（提示：该端点可能未执行结构化输出规范；若持续失败请确认模型支持 json_schema 输出）"
    ))
}

/// 契约执行器（唯一咽喉点）：prompt 解析 → 能力位闸门 → 引擎调用（structured/text 双轨）→
/// 宽容解析（剥围栏）→ 反序列化为强类型 → post_process 校验钳制。
/// 返回 (结果, 观测元数据 meta)：meta 含归一化 usage 与 ir_mode，调用方落各出口 raw 列。
pub async fn execute<C: AiContract>(
    config: &LlmConfig,
    pool: &PgPool,
    uid: i64,
    contract: &C,
) -> Result<(ContractOut<C::Output>, Value), AppError> {
    ensure_capability(config, contract)?;
    let system = prompts::effective(pool, uid, contract.prompt_key()).await?;
    let user_message = json!({ "role": "user", "content": contract.user_content() });
    let max_tokens = if contract.long_output() {
        config.max_tokens_long
    } else {
        config.max_tokens
    };

    // 纯文本评审模式：正向无条件覆盖 system 的格式约定，不解析直接回全文
    if !config.structured_output {
        let wrapped = json!({ "role": "system", "content": wrap_text_system(&system, contract.text_hint()) });
        let body = llm::build_body(config, &[wrapped, user_message], None, Some(max_tokens), false, None);
        let (text, meta) = llm::respond(config, contract.kind(), body).await?;
        return Ok((ContractOut::Text(text), with_text_mode(meta)));
    }

    let system_message = json!({ "role": "system", "content": system });
    let body = llm::build_body(
        config,
        &[system_message.clone(), user_message.clone()],
        Some((contract.schema_name(), &contract.schema())),
        Some(max_tokens),
        false,
        None,
    );
    let (text, meta) = llm::respond(config, contract.kind(), body).await?;
    let value = match llm::parse_json_loose(&text) {
        Ok(v) => Ok((v, meta.clone())),
        Err(e) => {
            tracing::warn!(err = %e, "契约解析初次失败，发起就地纠偏重试");
            let retry_user_msg = json!({
                "role": "user",
                "content": format!("你刚才的输出并非合法的 JSON 格式（解析错误：{e}）。请修复全部语法问题，严格输出合法的纯 JSON 数据，禁止输出任何未转义的引号或非法控制字符。")
            });
            let retry_body = llm::build_body(
                config,
                &[
                    system_message,
                    user_message,
                    json!({ "role": "assistant", "content": text }),
                    retry_user_msg,
                ],
                Some((contract.schema_name(), &contract.schema())),
                Some(max_tokens),
                false,
                None,
            );
            let (retry_text, retry_meta) = llm::respond(config, contract.kind(), retry_body).await?;
            match llm::parse_json_loose(&retry_text) {
                Ok(v) => Ok((v, retry_meta)),
                Err(retry_err) => {
                    tracing::warn!(err = %retry_err, "契约解析二次纠偏依然失败");
                    Err((retry_text, retry_err, retry_meta))
                }
            }
        }
    };

    match value {
        Ok((val, effective_meta)) => {
            match serde_json::from_value::<C::Output>(val) {
                Ok(typed) => {
                    let out = contract.post_process(typed)?;
                    Ok((ContractOut::Structured(out), effective_meta))
                }
                Err(e) => {
                    if let Some(action) = contract.structured_required_action() {
                        Err(parse_err(format!("输出不符合契约字段: {e}（{action}必需结构化数据）")))
                    } else {
                        tracing::warn!(err = %e, "契约反序列化强类型失败，评价式出口降级为纯文本 Markdown");
                        Ok((ContractOut::Text(text), with_text_mode(effective_meta)))
                    }
                }
            }
        }
        Err((retry_text, retry_err, retry_meta)) => {
            if let Some(action) = contract.structured_required_action() {
                Err(parse_err(format!("{retry_err}（{action}必需结构化数据）")))
            } else {
                tracing::warn!("二次纠偏失败，评价式出口降级为纯文本 Markdown");
                let fallback_text = if retry_text.trim().is_empty() { text } else { retry_text };
                Ok((ContractOut::Text(fallback_text), with_text_mode(retry_meta)))
            }
        }
    }
}

fn with_text_mode(mut meta: Value) -> Value {
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("ir_mode".to_string(), json!("text"));
    }
    meta
}

// ---------- 量纲钳制反序列化（解析即校验；宽容 i64 溢出/越界，绝不因超范围值整体失败） ----------

/// 数值钳制反序列化助手：模型输出的整数先按 i64 接住再钳到量纲内，
/// 避免 99999999999 这类越界值让整个契约反序列化失败（旧行为是钳制不报错）。
pub(crate) mod clamp {
    use serde::{Deserialize, Deserializer};

    /// 难度 1-5（AGENTS 基准 4）
    pub fn difficulty_1_5<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
        Ok(Option::<i64>::deserialize(d)?.map(|v| v.clamp(1, 5) as i32))
    }

    /// 综合分 0-100（正确性 50% + 完整性 30% + 表达清晰度 20%）
    pub fn score_0_100<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
        Ok(Option::<i64>::deserialize(d)?.map(|v| v.clamp(0, 100) as i32))
    }
}

/// 全部已注册契约的规格清单：(kind, schema_name, prompt_key, schema)。
/// 新增出口必须登记于此，否则注册表完整性测试失败。
pub fn registry_specs() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    use jd::{JdInterpret, JdMatch, PositionPredict};
    use question::{AnswerEvaluate, QuestionFull, QuestionRef};
    use resume::{ResumeChangeset, ResumeParse};
    use insights::ApplicationInsights;
    use interview_prep::InterviewPrep;
    use retro::{ApplicationOverall, Retrospective};
    vec![
        spec_of(QuestionFull::new("", None, None)),
        spec_of(QuestionRef::new("")),
        spec_of(AnswerEvaluate::new("", "", None)),
        spec_of(ResumeParse::new("")),
        spec_of(ResumeChangeset::new(&serde_json::json!({}), "")),
        spec_of(JdInterpret::new("")),
        spec_of(JdMatch::new(String::new())),
        spec_of(Retrospective::new("")),
        spec_of(ApplicationOverall::new("")),
        spec_of(PositionPredict::new(String::new())),
        spec_of(ApplicationInsights::new("")),
        spec_of(InterviewPrep::new("", interview_prep::RuleFacts::default())),
    ]
}

fn spec_of<C: AiContract>(c: C) -> (&'static str, &'static str, &'static str, Value) {
    (c.kind(), c.schema_name(), c.prompt_key(), c.schema())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::LlmConfig;

    fn config(structured: bool) -> LlmConfig {
        LlmConfig {
            provider: "p".into(),
            base_url: "http://x/v1".into(),
            api_key: String::new(),
            model: "m".into(),
            structured_output: structured,
            web_search: false,
            context_length: None,
            temperature: None,
            top_p: None,
            reasoning_effort: None,
            store: false,
            extra_body: json!({}),
            timeout: 30,
            max_tokens: 4096,
            max_tokens_long: 8192,
        }
    }

    /// 注册表完整性：kind/schema_name 全局唯一、prompt_key 必须登记于 prompts::DEFS、
    /// schema 必须 strict（additionalProperties:false + 全字段 required，递归）。
    #[test]
    fn registry_is_complete_unique_and_strict() {
        let specs = registry_specs();
        assert_eq!(specs.len(), 12, "当前应有 12 个结构化契约出口（paper 已退役）");
        let mut kinds = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        let def_keys: std::collections::HashSet<_> = prompts::DEFS.iter().map(|d| d.key).collect();
        for (kind, schema_name, prompt_key, schema) in &specs {
            assert!(kinds.insert(*kind), "kind 重复: {kind}");
            assert!(names.insert(*schema_name), "schema_name 重复: {schema_name}");
            assert!(def_keys.contains(prompt_key), "prompt_key 未登记于 prompts::DEFS: {prompt_key}");
            assert_strict(kind, schema);
        }
    }

    fn assert_strict(path: &str, s: &Value) {
        assert_eq!(s["additionalProperties"], false, "{path} 必须 additionalProperties:false");
        let props = s["properties"].as_object().cloned().unwrap_or_default();
        let required: Vec<&str> = s["required"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        if s["type"] == "object" && !props.is_empty() {
            assert_eq!(required.len(), props.len(), "{path} required 必须覆盖全部字段（可空用 null 联合）");
        }
        for (k, v) in props {
            if v["type"] == "object" {
                assert_strict(&format!("{path}.{k}"), &v);
            }
            if v["items"]["type"] == "object" {
                assert_strict(&format!("{path}.{k}[]"), &v["items"]);
            }
        }
    }

    /// 能力位闸门：结构必需出口在未开结构化时拒绝且消息含动作名与指引；普通出口放行。
    #[test]
    fn capability_gate_rejects_required_exits_without_network() {
        use resume::ResumeParse;
        let cfg = config(false);
        let err = ensure_capability(&cfg, &ResumeParse::new("")).unwrap_err();
        assert!(err.to_string().contains("解析简历"), "{err}");
        assert!(err.to_string().contains("结构化输出"), "{err}");

        // 开启能力位 → 放行
        assert!(ensure_capability(&config(true), &ResumeParse::new("")).is_ok());

        // 非必需出口（如 JD 解读）关闭能力位也放行（走文本降级）
        use jd::JdInterpret;
        assert!(ensure_capability(&cfg, &JdInterpret::new("")).is_ok());
    }

    /// 文本模式包装：无条件覆盖格式要求并携带 hint（兼容用户自定义 prompt 已删格式段）。
    #[test]
    fn wrap_text_system_overrides_format_unconditionally() {
        let wrapped = wrap_text_system("自定义人设（不含任何格式指令）", "覆盖要点 X");
        assert!(wrapped.contains("自定义人设"));
        assert!(wrapped.contains("不要输出 JSON"));
        assert!(wrapped.contains("覆盖要点 X"));
    }
}
