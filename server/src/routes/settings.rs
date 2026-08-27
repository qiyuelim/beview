use axum::extract::{Extension, State};
use axum::extract::Path;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, CurrentUser};
use crate::error::AppError;
use crate::llm;
use crate::settings;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/llm-config", get(get_llm_config).put(put_llm_config))
        .route("/settings/llm-config/providers", post(create_llm_provider))
        .route(
            "/settings/llm-config/providers/{id}",
            patch(patch_llm_provider).delete(delete_llm_provider),
        )
        .route("/settings/llm-config/models", post(create_llm_model))
        .route(
            "/settings/llm-config/models/{id}",
            patch(patch_llm_model).delete(delete_llm_model),
        )
        .route("/settings/llm-config/global", patch(patch_llm_global))
        .route("/settings/llm-config/test", post(test_llm_config))
        .route("/settings/prompts", get(get_prompts).put(put_prompts))
        .route("/settings/password", post(change_password))
        .route("/settings/resume-display", get(get_resume_display).put(put_resume_display))
}

// ---------- 简历显示偏好（反馈 #6：主题/密度/模块顺序与显隐，per-user） ----------

pub const RESUME_MODULES: [&str; 8] = [
    "basic",
    "education",
    "experience",
    "projects",
    "skills",
    "certificates",
    "self_evaluation",
    "links",
];

fn default_resume_display() -> Value {
    json!({
        "theme": "classic",
        "density": "normal",
        "hidden": [],
        "order": RESUME_MODULES,
    })
}

#[tracing::instrument(skip_all)]
async fn get_resume_display(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let stored = settings::get(&state.pool, user.0, "resume_display").await?;
    Ok(Json(stored.unwrap_or_else(default_resume_display)))
}

#[derive(Deserialize)]
struct ResumeDisplayReq {
    pub theme: Option<String>,
    pub density: Option<String>,
    pub hidden: Option<Vec<String>>,
    pub order: Option<Vec<String>>,
}

#[tracing::instrument(skip_all)]
async fn put_resume_display(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<ResumeDisplayReq>,
) -> Result<Json<Value>, AppError> {
    let mut value = default_resume_display();
    if let Some(t) = req.theme {
        if t != "classic" && t != "compact" {
            return Err(AppError::BadRequest(format!("非法 theme: {t}（可选 classic/compact）")));
        }
        value["theme"] = json!(t);
    }
    if let Some(d) = req.density {
        if d != "normal" && d != "tight" {
            return Err(AppError::BadRequest(format!("非法 density: {d}（可选 normal/tight）")));
        }
        value["density"] = json!(d);
    }
    if let Some(hidden) = req.hidden {
        if hidden.iter().any(|h| !RESUME_MODULES.contains(&h.as_str())) {
            return Err(AppError::BadRequest("hidden 含未知模块".to_string()));
        }
        value["hidden"] = json!(hidden);
    }
    if let Some(order) = req.order {
        let mut sorted = order.clone();
        sorted.sort();
        let mut expected: Vec<&str> = RESUME_MODULES.to_vec();
        expected.sort();
        if sorted != expected {
            return Err(AppError::BadRequest("order 必须是全部模块的排列".to_string()));
        }
        value["order"] = json!(order);
    }
    settings::set(&state.pool, user.0, "resume_display", value.clone()).await?;
    Ok(Json(value))
}

// ---------- LLM 配置（ADR-0016：llm_config 文档，多 Provider × 多 Model + 能力位 + 高级参数） ----------

fn mask_key(k: &str) -> String {
    if k.len() <= 8 {
        "****".to_string()
    } else {
        format!("****...{}", &k[k.len() - 4..])
    }
}

/// GET：完整文档，api_key 掩码展示；附 resolved 生效模型摘要
#[tracing::instrument(skip_all)]
async fn get_llm_config(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let uid = user.0;
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    let mut value = serde_json::to_value(&doc)?;
    if let Some(providers) = value["providers"].as_array_mut() {
        for p in providers {
            let stored = p["api_key"].as_str().unwrap_or("").to_string();
            let plain = (!stored.is_empty())
                .then(|| crate::crypto::decrypt(&stored).unwrap_or_default())
                .unwrap_or_default();
            p["api_key"] = json!(if stored.is_empty() { String::new() } else { mask_key(&plain) });
            p["has_key"] = json!(!stored.is_empty());
        }
    }
    // 评审 P1：配置存在但解析失败时，把具体原因透给设置页（resolve_error），
    // 不再静默表现为「未配置」
    let (resolved, resolve_error) = match doc.resolve() {
        Ok(c) => (
            Some(json!({
                "provider": c.provider,
                "model": c.model,
                "structured_output": c.structured_output,
                "web_search": c.web_search,
                "reasoning_effort": c.reasoning_effort,
            })),
            Value::Null,
        ),
        Err(reason) => (None, json!(reason)),
    };
    Ok(Json(json!({ "config": value, "resolved": resolved, "resolve_error": resolve_error })))
}

#[derive(Deserialize)]
struct PutLlmConfigReq {
    providers: Vec<Value>,
    models: Vec<Value>,
    active_model_id: Option<String>,
    global: Option<settings::LlmGlobal>,
}

/// PUT：整文档替换。校验严格（id 唯一/引用存在/量纲范围）；
/// api_key 以 * 开头视为未修改保留密文；非空明文加密落库；空串清除。
#[tracing::instrument(skip_all)]
async fn put_llm_config(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<PutLlmConfigReq>,
) -> Result<Json<Value>, AppError> {
    let uid = user.0;
    let old_doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();

    // providers / models（校验逻辑见 validate_*_entry，与 scoped PATCH 共用）
    let mut providers = Vec::new();
    let mut seen_provider = std::collections::HashSet::new();
    for (i, pv) in req.providers.iter().enumerate() {
        let p = validate_provider_entry(pv, &old_doc, i)?;
        if !seen_provider.insert(p.id.clone()) {
            return Err(AppError::BadRequest(format!("provider[{i}] id 重复")));
        }
        providers.push(p);
    }

    let mut models = Vec::new();
    let mut seen_model = std::collections::HashSet::new();
    for (i, mv) in req.models.iter().enumerate() {
        let m = validate_model_entry(mv, &providers, i)?;
        if !seen_model.insert(m.id.clone()) {
            return Err(AppError::BadRequest(format!("model[{i}] id 重复")));
        }
        models.push(m);
    }

    if let Some(active) = &req.active_model_id {
        if !models.iter().any(|m| &m.id == active) {
            return Err(AppError::BadRequest("active_model_id 不存在".to_string()));
        }
    }
    let global = req.global.unwrap_or_default();
    if !(5..=600).contains(&global.timeout) {
        return Err(AppError::BadRequest("timeout 需在 5-600 秒".to_string()));
    }
    if global.max_output_tokens_short < 512 {
        return Err(AppError::BadRequest("max_output_tokens_short 至少 512".to_string()));
    }
    if global.max_output_tokens_long < global.max_output_tokens_short {
        return Err(AppError::BadRequest("max_output_tokens_long 不能小于短任务档".to_string()));
    }

    let doc = settings::LlmConfigDoc {
        active_model_id: match req.active_model_id {
            Some(a) => Some(a),
            None => req.models.first().and_then(|m| m["id"].as_str().map(String::from)), // 缺省激活第一个模型
        },
        providers,
        models,
        global,
    };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&doc)?).await?;
    Ok(Json(json!({ "ok": true })))
}

/// 校验并构建 Provider 条目（PUT 全量与 PATCH 单个共用；api_key 掩码语义一致）
fn validate_provider_entry(
    pv: &Value,
    old_doc: &settings::LlmConfigDoc,
    _i: usize,
) -> Result<settings::ProviderEntry, AppError> {
    let id = pv["id"].as_str().unwrap_or("").trim().to_string();
    let id = if id.is_empty() {
        format!("prov-{}", hex::encode(rand::random::<[u8; 6]>()))
    } else {
        id
    };
    let name = pv["name"].as_str().unwrap_or("").trim().to_string();
    let base_url = pv["base_url"].as_str().unwrap_or("").trim().trim_end_matches('/').to_string();
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(AppError::BadRequest(format!(
            "provider「{name}」base_url 需以 http(s):// 开头"
        )));
    }
    let raw_key = pv["api_key"].as_str().unwrap_or("").trim().to_string();
    let old_entry = old_doc.providers.iter().find(|p| p.id == id);
    let api_key = if raw_key.is_empty() {
        String::new() // 显式清除
    } else if raw_key.starts_with('*') {
        old_entry.map(|e| e.api_key.clone()).unwrap_or_default() // 掩码=未修改
    } else {
        crate::crypto::encrypt(&raw_key)? // 新明文加密落库（ADR-0011 R5 延续）
    };
    Ok(settings::ProviderEntry { id, name, base_url, api_key })
}

/// 校验并构建 Model 条目（PUT 全量与 PATCH 单个共用）
fn validate_model_entry(
    mv: &Value,
    providers: &[settings::ProviderEntry],
    i: usize,
) -> Result<settings::ModelEntry, AppError> {
    let id = mv["id"].as_str().unwrap_or("").trim().to_string();
    let id = if id.is_empty() {
        format!("model-{}", hex::encode(rand::random::<[u8; 6]>()))
    } else {
        id
    };
    let provider_id = mv["provider_id"].as_str().unwrap_or("").trim().to_string();
    if !providers.iter().any(|p| p.id == provider_id) {
        return Err(AppError::BadRequest(format!("model[{i}] 引用的 provider 不存在")));
    }
    let name = mv["name"].as_str().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest(format!("model[{i}] 名称不能为空")));
    }
    let caps: settings::ModelCaps = serde_json::from_value(mv.get("caps").cloned().unwrap_or(json!({})))
        .map_err(|e| AppError::BadRequest(format!("model[{i}] caps 非法: {e}")))?;
    let mut advanced: settings::ModelAdvanced =
        serde_json::from_value(mv.get("advanced").cloned().unwrap_or(json!({})))
            .map_err(|e| AppError::BadRequest(format!("model[{i}] advanced 非法: {e}")))?;
    if !advanced.extra_body.is_object() {
        if advanced.extra_body.is_null() {
            advanced.extra_body = json!({}); // 缺省归一为空对象
        } else {
            return Err(AppError::BadRequest(format!("model[{i}] extra_body 必须是对象")));
        }
    }
    // SDK 风格输入归一（ADR-0016 D2）：{"extra_body": {...}} → 取内层 KV 集
    if let Some(inner) = advanced.extra_body.get("extra_body") {
        if inner.is_object() {
            advanced.extra_body = inner.clone();
        } else if !inner.is_null() {
            return Err(AppError::BadRequest(format!(
                "model[{i}] extra_body 内层必须是对象，如 {{\"extra_body\": {{\"enable_thinking\": true}}}}"
            )));
        }
    }
    if let Some(effort) = &advanced.reasoning_effort {
        let effort = effort.trim();
        if effort.is_empty() {
            advanced.reasoning_effort = None;
        } else if !settings::REASONING_EFFORTS.contains(&effort) {
            return Err(AppError::BadRequest(format!(
                "model[{i}] 思考强度非法：可选 {}",
                settings::REASONING_EFFORTS.join("/")
            )));
        } else {
            advanced.reasoning_effort = Some(effort.to_string());
        }
    }
    if let Some(t) = advanced.temperature {
        if !(0.0..=2.0).contains(&t) {
            return Err(AppError::BadRequest(format!("model[{i}] temperature 需在 0-2")));
        }
    }
    if let Some(p) = advanced.top_p {
        if !(0.0..=1.0).contains(&p) {
            return Err(AppError::BadRequest(format!("model[{i}] top_p 需在 0-1")));
        }
    }
    let context_length = mv["context_length"].as_u64();
    if let Some(cl) = context_length {
        if !(1024..=10_000_000).contains(&cl) {
            return Err(AppError::BadRequest(format!(
                "model[{i}] 上下文长度需在 1024-10000000 之间（元数据仅作展示与输入护栏）"
            )));
        }
    }
    Ok(settings::ModelEntry { id, provider_id, name, context_length, caps, advanced })
}

/// PATCH 单个 Provider：仅替换该条目，其余配置原样保留（底层为单键文档覆盖写，无版本历史）
#[tracing::instrument(skip_all)]
async fn patch_llm_provider(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(pv): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let uid = user.0;
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    if !doc.providers.iter().any(|p| p.id == id) {
        return Err(AppError::NotFound);
    }
    let pos = doc.providers.iter().position(|x| x.id == id).ok_or(AppError::NotFound)?;
    let mut p = validate_provider_entry(&pv, &doc, 0)?;
    p.id = id; // path 为权威
    let mut providers = doc.providers.clone();
    providers[pos] = p;
    let new_doc = settings::LlmConfigDoc { providers, ..doc };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&new_doc)?).await?;
    Ok(Json(json!({ "ok": true })))
}

/// PATCH 单个 Model：能力位/高级参数局部保存
#[tracing::instrument(skip_all)]
async fn patch_llm_model(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(mv): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let uid = user.0;
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    let pos = doc.models.iter().position(|m| m.id == id).ok_or(AppError::NotFound)?;
    let mut m = validate_model_entry(&mv, &doc.providers, 0)?;
    m.id = id;
    let mut models = doc.models.clone();
    models[pos] = m;
    let new_doc = settings::LlmConfigDoc { models, ..doc };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&new_doc)?).await?;
    Ok(Json(json!({ "ok": true })))
}

/// POST 新建 Provider 并持久化
#[tracing::instrument(skip_all)]
async fn create_llm_provider(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(pv): Json<Value>,
) -> Result<Json<settings::ProviderEntry>, AppError> {
    let uid = user.0;
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    let mut p = validate_provider_entry(&pv, &doc, doc.providers.len())?;
    if p.id.trim().is_empty() {
        p.id = format!("prov-{}", hex::encode(rand::random::<[u8; 6]>()));
    }
    let mut providers = doc.providers.clone();
    if let Some(pos) = providers.iter().position(|x| x.id == p.id) {
        providers[pos] = p.clone();
    } else {
        providers.push(p.clone());
    }
    let new_doc = settings::LlmConfigDoc { providers, ..doc };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&new_doc)?).await?;
    Ok(Json(p))
}

/// DELETE 删除指定 Provider（级联删除其所属模型并自愈 active_model_id）
#[tracing::instrument(skip_all)]
async fn delete_llm_provider(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let uid = user.0;
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    let mut providers = doc.providers.clone();
    let prev_len = providers.len();
    providers.retain(|p| p.id != id);
    if providers.len() == prev_len {
        return Err(AppError::NotFound);
    }
    let mut models = doc.models.clone();
    models.retain(|m| m.provider_id != id);
    let mut active_model_id = doc.active_model_id.clone();
    if !models.iter().any(|m| Some(&m.id) == active_model_id.as_ref()) {
        active_model_id = models.first().map(|m| m.id.clone());
    }
    let new_doc = settings::LlmConfigDoc {
        providers,
        models,
        active_model_id,
        ..doc
    };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&new_doc)?).await?;
    Ok(Json(json!({ "ok": true })))
}

/// POST 新建 Model 并持久化
#[tracing::instrument(skip_all)]
async fn create_llm_model(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(mv): Json<Value>,
) -> Result<Json<settings::ModelEntry>, AppError> {
    let uid = user.0;
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    let mut m = validate_model_entry(&mv, &doc.providers, doc.models.len())?;
    if m.id.trim().is_empty() {
        m.id = format!("model-{}", hex::encode(rand::random::<[u8; 6]>()));
    }
    let mut models = doc.models.clone();
    if let Some(pos) = models.iter().position(|x| x.id == m.id) {
        models[pos] = m.clone();
    } else {
        models.push(m.clone());
    }
    let mut active_model_id = doc.active_model_id.clone();
    if active_model_id.as_deref().map_or(true, str::is_empty) || !models.iter().any(|x| Some(&x.id) == active_model_id.as_ref()) {
        active_model_id = Some(m.id.clone());
    }
    let new_doc = settings::LlmConfigDoc {
        models,
        active_model_id,
        ..doc
    };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&new_doc)?).await?;
    Ok(Json(m))
}

/// DELETE 删除指定 Model 并自愈 active_model_id
#[tracing::instrument(skip_all)]
async fn delete_llm_model(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let uid = user.0;
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    let mut models = doc.models.clone();
    let prev_len = models.len();
    models.retain(|m| m.id != id);
    if models.len() == prev_len {
        return Err(AppError::NotFound);
    }
    let mut active_model_id = doc.active_model_id.clone();
    if active_model_id.as_deref() == Some(&id) || !models.iter().any(|m| Some(&m.id) == active_model_id.as_ref()) {
        active_model_id = models.first().map(|m| m.id.clone());
    }
    let new_doc = settings::LlmConfigDoc {
        models,
        active_model_id,
        ..doc
    };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&new_doc)?).await?;
    Ok(Json(json!({ "ok": true })))
}

/// PATCH 全局参数
#[tracing::instrument(skip_all)]
async fn patch_llm_global(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(global): Json<settings::LlmGlobal>,
) -> Result<Json<Value>, AppError> {
    let uid = user.0;
    if !(5..=600).contains(&global.timeout) {
        return Err(AppError::BadRequest("timeout 需在 5-600 秒".to_string()));
    }
    if global.max_output_tokens_short < 512 {
        return Err(AppError::BadRequest("max_output_tokens_short 至少 512".to_string()));
    }
    if global.max_output_tokens_long < global.max_output_tokens_short {
        return Err(AppError::BadRequest("max_output_tokens_long 不能小于短任务档".to_string()));
    }
    let doc = settings::load_doc(&state.pool, uid).await?.unwrap_or_default();
    let new_doc = settings::LlmConfigDoc { global, ..doc };
    settings::set(&state.pool, uid, settings::LLM_CONFIG_KEY, serde_json::to_value(&new_doc)?).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
struct TestLlmConfigReq {
    /// 已保存模型的 id（优先）；否则用内联字段（新建未保存也能测）
    pub model_id: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub web_search: Option<bool>,
    pub structured_output: Option<bool>,
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub store: Option<bool>,
    pub extra_body: Option<Value>,
    pub timeout: Option<u64>,
}

/// 测试连接：POST /responses ping 一次；404/405 给「不支持 Responses API」明确提示（F2 先例：表单值优先）。
#[tracing::instrument(skip_all)]
async fn test_llm_config(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    payload: Option<Json<TestLlmConfigReq>>,
) -> Result<Json<Value>, AppError> {
    let req = payload.map(|p| p.0).unwrap_or_default();
    let doc = settings::load_doc(&state.pool, user.0).await?.unwrap_or_default();

    // 解析目标：saved model（掩码 key 回源密文）或内联字段
    let saved_model = req.model_id.as_deref().filter(|s| !s.is_empty()).and_then(|mid| {
        doc.models.iter().find(|m| m.id == mid).and_then(|m| {
            doc.providers
                .iter()
                .find(|p| p.id == m.provider_id)
                .map(|p| (p.name.clone(), p.base_url.clone(), p.api_key.clone(), m))
        })
    });

    let (provider_name, base_url, api_key, model_name, web_search, structured, effort, temperature, top_p, store, extra_body) =
        if let Some((pname, purl, pkey, m)) = saved_model {
            // 内联 api_key 覆盖（非掩码非空才生效）
            let key_override = req.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty() && !k.starts_with('*'));
            (
                pname,
                req.base_url.clone().filter(|s| !s.trim().is_empty()).unwrap_or(purl),
                key_override.map(|k| k.to_string()).unwrap_or(pkey),
                req.model.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| m.name.clone()),
                req.web_search.unwrap_or(m.caps.web_search),
                req.structured_output.unwrap_or(m.caps.structured_output),
                req.reasoning_effort.clone().or_else(|| m.advanced.effort_or_default()),
                req.temperature.or(m.advanced.temperature),
                req.top_p.or(m.advanced.top_p),
                Some(req.store.unwrap_or(m.advanced.store_or_default())),
                req.extra_body.clone().filter(|v| v.is_object()).or(Some(m.advanced.extra_body.clone())),
            )
        } else {
            let base_url = req.base_url.clone().filter(|s| !s.trim().is_empty())
                .or_else(|| doc.providers.first().map(|p| p.base_url.clone()))
                .ok_or_else(|| AppError::BadRequest("请填写 base_url（或先保存配置）".to_string()))?;
            let model_name = req.model.clone().filter(|s| !s.trim().is_empty())
                .or_else(|| doc.models.first().map(|m| m.name.clone()))
                .ok_or_else(|| AppError::BadRequest("请填写 model".to_string()))?;
            let api_key = req.api_key.clone().map(|k| k.trim().to_string()).filter(|k| !k.is_empty() && !k.starts_with('*'))
                .or_else(|| doc.providers.first().map(|p| p.api_key.clone()))
                .unwrap_or_default();
            (
                settings::provider_of(&base_url),
                base_url,
                api_key,
                model_name,
                req.web_search.unwrap_or(false),
                req.structured_output.unwrap_or(true),
                Some(req.reasoning_effort.clone().unwrap_or_else(|| settings::DEFAULT_REASONING_EFFORT.to_string())),
                req.temperature,
                req.top_p,
                Some(req.store.unwrap_or(false)),
                req.extra_body.clone().filter(|v| v.is_object()).or(Some(json!({}))),
            )
        };

    // 密文解密为运行时明文
    let api_key = if crate::crypto::is_encrypted(&api_key) {
        crate::crypto::decrypt(&api_key).unwrap_or_default()
    } else {
        api_key
    };
    let config = settings::LlmConfig {
        provider: provider_name,
        base_url: base_url.trim_end_matches('/').to_string(),
        api_key,
        model: model_name.clone(),
        structured_output: structured,
        web_search,
        context_length: None,
        temperature: temperature.filter(|t| (0.0..=2.0).contains(t)),
        top_p: top_p.filter(|p| (0.0..=1.0).contains(p)),
        reasoning_effort: effort,
        store: store.unwrap_or(false),
        extra_body: extra_body.unwrap_or_else(|| json!({})),
        timeout: req.timeout.filter(|t| (5..=600).contains(t)).unwrap_or(doc.global.timeout),
        max_tokens: doc.global.max_output_tokens_short,
        max_tokens_long: doc.global.max_output_tokens_long,
    };
    llm::test_connection(&config).await?;
    Ok(Json(json!({ "ok": true, "provider": config.provider, "model": config.model })))
}

/// 提示词清单（处处可编辑）：全部注册过的 LLM prompt + 当前生效值 + 是否自定义
#[tracing::instrument(skip_all)]
async fn get_prompts(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Value>, AppError> {
    let mut arr = Vec::new();
    for d in crate::prompts::DEFS {
        let value = crate::prompts::effective(&state.pool, user.0, d.key).await?;
        let is_custom = crate::prompts::is_custom(&state.pool, user.0, d.key).await?;
        arr.push(json!({
            "key": d.key,
            "name": d.name,
            "description": d.description,
            "value": value,
            "is_custom": is_custom,
        }));
    }
    Ok(Json(json!({ "prompts": arr })))
}

#[derive(Deserialize)]
struct PromptSetReq {
    key: String,
    /// None/空串 = 恢复内置默认
    value: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn put_prompts(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<PromptSetReq>,
) -> Result<Json<Value>, AppError> {
    if !crate::prompts::DEFS.iter().any(|d| d.key == req.key) {
        return Err(AppError::BadRequest(format!("未知提示词 key: {}", req.key)));
    }
    match req.value.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(v) => {
            crate::settings::set(&state.pool, user.0, &req.key, json!(v)).await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE user_id=$1 AND key=$2")
                .bind(user.0)
                .bind(&req.key)
                .execute(&state.pool)
                .await?;
        }
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct PasswordReq {
    old_password: String,
    new_password: String,
}

#[tracing::instrument(skip_all)]
async fn change_password(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<PasswordReq>,
) -> Result<Json<Value>, AppError> {
    if req.new_password.len() < 6 {
        return Err(AppError::BadRequest("新密码至少 6 位".to_string()));
    }
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=$1")
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    if !auth::verify_password(&req.old_password, &hash) {
        return Err(AppError::Forbidden);
    }
    let new_hash = auth::hash_password(&req.new_password)?;
    sqlx::query("UPDATE users SET password_hash=$2 WHERE id=$1")
        .bind(user.0)
        .bind(new_hash)
        .execute(&state.pool)
        .await?;
    state.sessions.invalidate_user(user.0);
    tracing::info!(
        target: "audit",
        event = "audit.user.password_changed",
        user_id = user.0,
        "user password changed successfully"
    );
    Ok(Json(json!({ "ok": true })))
}
