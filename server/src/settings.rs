use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

/// settings 表 KV 读写（value 为 JSONB，按用户隔离；v4 M1 起 llm_*/prompt_* 全部 per-user）
pub async fn get(pool: &PgPool, uid: i64, key: &str) -> Result<Option<Value>, sqlx::Error> {
    sqlx::query_scalar("SELECT value FROM settings WHERE user_id=$1 AND key=$2")
        .bind(uid)
        .bind(key)
        .fetch_optional(pool)
        .await
}

pub async fn set(pool: &PgPool, uid: i64, key: &str, value: Value) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO settings(user_id, key, value) VALUES($1,$2,$3) \
         ON CONFLICT (user_id, key) DO UPDATE SET value=EXCLUDED.value",
    )
    .bind(uid)
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &PgPool, uid: i64, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM settings WHERE user_id=$1 AND key=$2")
        .bind(uid)
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------- LLM 配置文档（ADR-0016：settings 单键 llm_config，多 Provider × 多 Model） ----------

pub const LLM_CONFIG_KEY: &str = "llm_config";

/// 思考强度七档递增（ADR-0016 D2）：默认 xhigh；"none" = 不下发 reasoning 字段
/// （跨端最稳的「关」；需要显式 effort:"none" 的端点经 extra_body 自行指定）
pub const REASONING_EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
pub const DEFAULT_REASONING_EFFORT: &str = "xhigh";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ProviderEntry {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// AES-256-GCM 密文（enc:v1:...），绝不明文落库（ADR-0011 R5 延续）
    #[serde(default)]
    pub api_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ModelCaps {
    /// 结构化输出：true=json_schema strict；false=评审型出口走「纯文本评审」模式
    #[serde(default = "default_true")]
    pub structured_output: bool,
    /// 联网搜索：true 时请求携带 tools:[{"type":"web_search"}]
    #[serde(default)]
    pub web_search: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ModelAdvanced {
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    /// 七档思考强度；None=不下发 reasoning 字段
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// 默认 false（面试数据不留存第三方）；可手动开 true
    #[serde(default)]
    pub store: Option<bool>,
    /// 自定义 KV 以字面 extra_body 字段嵌套下发（不与内置顶层字段合并），如 {"enable_thinking": true}
    #[serde(default)]
    pub extra_body: Value,
}

impl ModelAdvanced {
    pub fn effort_or_default(&self) -> Option<String> {
        self.reasoning_effort
            .clone()
            .filter(|e| !e.is_empty())
            .or_else(|| Some(DEFAULT_REASONING_EFFORT.to_string()))
    }
    pub fn store_or_default(&self) -> bool {
        self.store.unwrap_or(false)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModelEntry {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    /// 模型属性，添加时固定；元数据与输入护栏用，不进请求体（Responses API 无此参数）
    #[serde(default)]
    pub context_length: Option<u64>,
    #[serde(default)]
    pub caps: ModelCaps,
    #[serde(default)]
    pub advanced: ModelAdvanced,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LlmGlobal {
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default = "default_short")]
    pub max_output_tokens_short: u32,
    #[serde(default = "default_long")]
    pub max_output_tokens_long: u32,
}

impl Default for LlmGlobal {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            max_output_tokens_short: default_short(),
            max_output_tokens_long: default_long(),
        }
    }
}

fn default_timeout() -> u64 {
    120
}
fn default_short() -> u32 {
    4096
}
fn default_long() -> u32 {
    8192
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LlmConfigDoc {
    #[serde(default)]
    pub providers: Vec<ProviderEntry>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub active_model_id: Option<String>,
    #[serde(default)]
    pub global: LlmGlobal,
}

impl LlmConfigDoc {
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.models.is_empty()
    }

    /// 解析激活模型 + 其 Provider（active 缺失时回落第一个 model）；
    /// api_key 解密为运行时明文。返回 Err(描述) 表示配置存在但解析失败——
    /// 调用方应把该原因透出给用户（见 [`load_llm_state`] / 设置页 resolve_error），不得吞掉。
    pub fn resolve(&self) -> Result<LlmConfig, String> {
        let model = self
            .models
            .iter()
            .find(|m| Some(&m.id) == self.active_model_id.as_ref())
            .or_else(|| self.models.first())
            .ok_or_else(|| "未配置模型".to_string())?;
        let provider = self
            .providers
            .iter()
            .find(|p| p.id == model.provider_id)
            .ok_or_else(|| format!("模型 {} 引用的 provider 不存在", model.name))?;
        if provider.base_url.trim().is_empty() || model.name.trim().is_empty() {
            return Err("base_url 或 model 为空".to_string());
        }
        // 密文解密（解密失败保底空串，不阻塞调用——错误会在请求阶段暴露）
        let api_key = if crate::crypto::is_encrypted(&provider.api_key) {
            crate::crypto::decrypt(&provider.api_key).unwrap_or_default()
        } else {
            provider.api_key.clone()
        };
        Ok(LlmConfig {
            provider: if provider.name.trim().is_empty() {
                provider_of(&provider.base_url)
            } else {
                provider.name.clone()
            },
            base_url: provider.base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.name.clone(),
            structured_output: model.caps.structured_output,
            web_search: model.caps.web_search,
            context_length: model.context_length,
            temperature: model.advanced.temperature.filter(|t| (0.0..=2.0).contains(t)),
            top_p: model.advanced.top_p.filter(|p| (0.0..=1.0).contains(p)),
            reasoning_effort: model.advanced.effort_or_default(),
            store: model.advanced.store_or_default(),
            extra_body: match model.advanced.extra_body {
                Value::Object(_) => model.advanced.extra_body.clone(),
                _ => serde_json::json!({}),
            },
            timeout: self.global.timeout.clamp(5, 600),
            max_tokens: self.global.max_output_tokens_short.max(512),
            max_tokens_long: self.global.max_output_tokens_long.max(self.global.max_output_tokens_short.max(512)),
        })
    }
}

/// 运行时解析后的 LLM 配置（llm.rs 引擎消费）。字段名 max_tokens 保持短任务档语义。
#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub structured_output: bool,
    pub web_search: bool,
    pub context_length: Option<u64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub store: bool,
    pub extra_body: Value,
    pub timeout: u64,
    /// 短任务档（判卷/评分/标签）
    pub max_tokens: u32,
    /// 长文任务档（复盘全文/参考答案/试卷生成），≥ 短任务档
    pub max_tokens_long: u32,
}

/// 从 base_url 取 provider 名（host），用于指标兜底标签
pub fn provider_of(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .split("://")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_string()
}

/// 读取配置文档（含旧六键一次性迁移）。None=未配置；Err=读取/写库失败。
pub async fn load_doc(pool: &PgPool, uid: i64) -> Result<Option<LlmConfigDoc>, sqlx::Error> {
    if let Some(v) = get(pool, uid, LLM_CONFIG_KEY).await? {
        match serde_json::from_value::<LlmConfigDoc>(v) {
            Ok(doc) => return Ok(Some(doc)),
            Err(_) => return Ok(None), // 文档损坏视为未配置，不阻塞业务（运行时路径由 load_llm_state 透出原因）
        }
    }
    migrate_legacy(pool, uid).await
}

/// ADR-0016 D2：旧 llm_* 六键 → 合成首个 provider/model 后删除旧键。幂等：无旧键即返回 None。
async fn migrate_legacy(pool: &PgPool, uid: i64) -> Result<Option<LlmConfigDoc>, sqlx::Error> {
    let base_url = get(pool, uid, "llm_base_url").await?.and_then(|v| v.as_str().map(String::from));
    let model = get(pool, uid, "llm_model").await?.and_then(|v| v.as_str().map(String::from));
    let (Some(base_url), Some(model)) = (base_url, model) else {
        return Ok(None);
    };
    let raw_key = get(pool, uid, "llm_api_key")
        .await?
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    // 明文 → 加密后入新文档（历史懒迁移逻辑收编于此）
    let enc_key = if raw_key.is_empty() {
        String::new()
    } else if crate::crypto::is_encrypted(&raw_key) {
        raw_key
    } else {
        crate::crypto::encrypt(&raw_key).unwrap_or(raw_key)
    };
    let thinking = get(pool, uid, "llm_thinking").await?.and_then(|v| v.as_bool()).unwrap_or(false);
    let timeout = get(pool, uid, "llm_timeout").await?.and_then(|v| v.as_u64()).unwrap_or(120);
    let max_tokens = get(pool, uid, "llm_max_tokens").await?.and_then(|v| v.as_u64()).unwrap_or(4096) as u32;

    let doc = LlmConfigDoc {
        providers: vec![ProviderEntry {
            id: "p_legacy".to_string(),
            name: provider_of(&base_url),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: enc_key,
        }],
        models: vec![ModelEntry {
            id: "m_legacy".to_string(),
            provider_id: "p_legacy".to_string(),
            name: model,
            context_length: None,
            caps: ModelCaps { structured_output: true, web_search: false },
            advanced: ModelAdvanced {
                temperature: None,
                top_p: None,
                // 旧 thinking=true → medium；否则按新默认 xhigh（ADR-0016 D2 迁移规则）
                reasoning_effort: Some(if thinking { "medium" } else { DEFAULT_REASONING_EFFORT }.to_string()),
                store: Some(false),
                extra_body: serde_json::json!({}),
            },
        }],
        active_model_id: Some("m_legacy".to_string()),
        global: LlmGlobal {
            timeout: timeout.clamp(5, 600),
            max_output_tokens_short: max_tokens.max(512),
            max_output_tokens_long: default_long().max(max_tokens.max(512)),
        },
    };
    set(pool, uid, LLM_CONFIG_KEY, serde_json::to_value(&doc).unwrap_or_default()).await?;
    for k in ["llm_base_url", "llm_api_key", "llm_model", "llm_timeout", "llm_thinking", "llm_max_tokens"] {
        let _ = delete(pool, uid, k).await;
    }
    tracing::info!(uid, "llm 配置已从旧键迁移到 llm_config 文档");
    Ok(Some(doc))
}

/// LLM 配置加载结果（评审 P1）：区分「未配置」与「配置存在但已损坏」，
/// 损坏时携带具体原因（如模型引用的 provider 已丢失），业务侧不再静默当作未配置。
#[derive(Debug, Clone)]
pub enum LlmLoad {
    /// 从未配置过（无 llm_config 且无旧六键）
    NotConfigured,
    /// 配置存在但不可用（文档解析失败 / 模型引用 provider 丢失 / 关键字段为空等）
    Broken(String),
    Ready(LlmConfig),
}

fn classify(doc: LlmConfigDoc) -> LlmLoad {
    match doc.resolve() {
        Ok(c) => LlmLoad::Ready(c),
        Err(reason) => LlmLoad::Broken(format!("{reason}（请到 设置 → 模型服务 检查模型与 Provider 引用）")),
    }
}

/// 加载并分类当前用户的 LLM 配置状态（评审 P1 入口）。
pub async fn load_llm_state(pool: &PgPool, uid: i64) -> Result<LlmLoad, sqlx::Error> {
    if let Some(v) = get(pool, uid, LLM_CONFIG_KEY).await? {
        return Ok(match serde_json::from_value::<LlmConfigDoc>(v) {
            Ok(doc) => classify(doc),
            Err(e) => LlmLoad::Broken(format!(
                "llm_config 文档无法解析为已知结构（{e}）；请到 设置 → 模型服务 重新保存一次"
            )),
        });
    }
    Ok(match migrate_legacy(pool, uid).await? {
        Some(doc) => classify(doc),
        None => LlmLoad::NotConfigured,
    })
}

/// 业务出口统一入口：就绪则返回运行时配置；未配置/损坏均以带具体原因的 400 拒绝，
/// 不再静默当作「未配置」（评审 P1：Model 引用的 Provider ID 丢失等配置损伤要能被看见）。
pub async fn require_llm(pool: &PgPool, uid: i64) -> Result<LlmConfig, crate::error::AppError> {
    match load_llm_state(pool, uid).await? {
        LlmLoad::Ready(c) => Ok(c),
        LlmLoad::Broken(reason) => Err(crate::error::AppError::BadRequest(format!(
            "LLM 配置存在问题：{reason}"
        ))),
        LlmLoad::NotConfigured => Err(crate::error::AppError::BadRequest(
            "请先到设置页配置 LLM (base_url 与 model)".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 评审 P1：Model 引用的 Provider ID 丢失 → 必须分类为 Broken 并携带具体原因
    /// （哪个模型、缺什么），不得静默当作「未配置」。
    #[test]
    fn orphan_provider_reference_classifies_broken_with_reason() {
        let doc: LlmConfigDoc = serde_json::from_value(json!({
            "providers": [],
            "models": [{ "id": "m1", "provider_id": "p_missing", "name": "gpt-x" }],
            "active_model_id": "m1"
        }))
        .unwrap();
        match classify(doc) {
            LlmLoad::Broken(reason) => {
                assert!(reason.contains("provider 不存在"), "应含具体原因: {reason}");
                assert!(reason.contains("gpt-x"), "应指明是哪个模型: {reason}");
                assert!(reason.contains("设置"), "应给修复入口指引: {reason}");
            }
            _ => panic!("孤儿引用必须分类为 Broken"),
        }
    }

    #[test]
    fn valid_doc_classifies_ready() {
        let doc: LlmConfigDoc = serde_json::from_value(json!({
            "providers": [{ "id": "p1", "name": "OpenAI", "base_url": "https://api.openai.com/v1" }],
            "models": [{ "id": "m1", "provider_id": "p1", "name": "gpt-5.2" }],
            "active_model_id": "m1"
        }))
        .unwrap();
        match classify(doc) {
            LlmLoad::Ready(c) => {
                assert_eq!(c.model, "gpt-5.2");
                assert_eq!(c.provider, "OpenAI");
            }
            other => panic!("合法文档应为 Ready，实为 {other:?}"),
        }
    }

    #[test]
    fn empty_base_url_classifies_broken() {
        let doc: LlmConfigDoc = serde_json::from_value(json!({
            "providers": [{ "id": "p1", "name": "", "base_url": "  " }],
            "models": [{ "id": "m1", "provider_id": "p1", "name": "m" }]
        }))
        .unwrap();
        assert!(matches!(classify(doc), LlmLoad::Broken(_)));
    }
}
