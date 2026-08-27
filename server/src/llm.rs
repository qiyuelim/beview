//! LLM 引擎（ADR-0016 / ADR-0017 D2）：纯粹的 OpenAI Responses API 底层客户端。
//!
//! 职责边界（Ticket 02 契约插件化后收口）：仅保留请求体组装、HTTP 连接池、
//! 非流式/流式分发、OTel 观测（span + metrics）与友好错误转换。
//! 出口的 prompt/schema/输入组装/强类型解析/文本降级全部上移至 `crate::contracts`，
//! 本模块不再包含任何业务语义。
//!
//! - 全部出口走 `POST {base_url}/responses`（create_byot / create_stream_byot，Value 通路：
//!   支撑 extra_body 任意 KV 注入、七档 reasoning effort 含 SDK 枚举外的 max、动态 json_schema）。
//! - 观测纪律（AGENTS 基准 3）：respond/stream 单一咽喉点记 span + llm_calls/llm_duration/
//!   llm_tokens/llm_errors；usage 从 Response.usage(input_tokens/output_tokens) 归一化为
//!   prompt_tokens/completion_tokens 写回 raw，调用方读法不变。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use async_openai::config::OpenAIConfig;
use async_openai::Client;
use serde_json::{json, Value};
use tracing::Instrument;

use crate::error::AppError;
use crate::metrics;
use crate::settings::LlmConfig;

/// 纯文本评审模式的 prompt 包装（ADR-0016 D3）：正向无条件指令——不依赖原文是否含有
/// 「输出 JSON」字样（用户自定义 prompt 可能已删掉该句），一律覆盖为 Markdown 文本要求。
#[allow(dead_code)]
pub(super) fn text_review_wrap(system: &str, hint: &str) -> String {
    format!(
        "{system}\n\n【输出模式】本次调用未启用结构化输出：无论上文如何约定输出格式，本次请直接输出中文 Markdown 评审文本，\
         不要输出 JSON，也不要用代码块包裹整个回答。{hint}"
    )
}

// ---------- 引擎核心 ----------

// ---------- HTTP 连接池 ----------

/// reqwest 客户端缓存（key = 超时秒数）：reqwest::Client 内部自带连接池，跨请求复用可省去
/// 每次调用的 TLS 握手与池重建开销（ADR-0017 D2「连接池」职责）。不同用户配置的超时档位
/// 各自持有一个实例，数量天然有界（5-600s 区间内的离散值）。
static HTTP_CLIENTS: OnceLock<Mutex<HashMap<u64, reqwest::Client>>> = OnceLock::new();

fn http_client(timeout_secs: u64) -> reqwest::Client {
    let map = HTTP_CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(c) = map.lock().unwrap().get(&timeout_secs) {
        return c.clone();
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(5)))
        // 关闭自动重定向：301/302 会被降级为 GET，POST /responses 会变成 GET 而报
        // 「Method Not Allowed」，把可诊断的配置错误（http→https）变成假象；改为让 3xx
        // 直接返回，由 friendly_llm_err 给出修正提示。
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default(); // builder 仅在 TLS 后端初始化失败时才报错；退回默认客户端
    map.lock().unwrap().insert(timeout_secs, client.clone());
    client
}

fn client(config: &LlmConfig) -> Result<Client<OpenAIConfig>, AppError> {
    let cfg = OpenAIConfig::new()
        .with_api_base(&config.base_url)
        .with_api_key(&config.api_key);
    Ok(Client::with_config(cfg).with_http_client(http_client(config.timeout)))
}

/// 组装 /responses 请求体。messages 中前导 system 提升为 instructions，其余转 input 数组；
/// format=Some 时带 text.format=json_schema strict；extra_body 以字面字段嵌套下发（不与内置顶层字段合并，
/// 修订自 ADR-0016 D2 原顶层合并语义，见 ADR 修订记录）。
///
/// 请求形态参数显式入参（评审 P0：禁止调用方旁路改 body）：
/// - `stream`：流式调用必须在此显式带 `"stream": true`——byot 流式分支不会自动补；
/// - `previous_response_id`：多轮链式上下文（Responses API 自动关联历史，无需重放消息）。
///   必须传响应顶层 `id`（UUID 形态），**不是** output 数组里消息的 `msg_*` id；
///   上游保留期默认 7 天，过期由调用方负责兜底（全量重放）。
pub(crate) fn build_body(
    config: &LlmConfig,
    messages: &[Value],
    format: Option<(&str, &Value)>,
    max_output_tokens: Option<u32>,
    stream: bool,
    previous_response_id: Option<&str>,
) -> Value {
    let mut instructions = String::new();
    let mut input: Vec<Value> = Vec::new();
    for m in messages {
        let role = m["role"].as_str().unwrap_or("user");
        let content = m["content"].as_str().unwrap_or("");
        if role == "system" && input.is_empty() {
            if !instructions.is_empty() {
                instructions.push_str("\n\n");
            }
            instructions.push_str(content);
        } else {
            input.push(json!({ "role": role, "content": content }));
        }
    }
    let mut body = json!({
        "model": config.model,
        "input": input,
        "store": config.store,
    });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions);
    }
    if let Some(id) = previous_response_id {
        body["previous_response_id"] = json!(id);
    }
    if stream {
        body["stream"] = json!(true);
    }
    if let Some(n) = max_output_tokens {
        body["max_output_tokens"] = json!(n);
    }
    if let Some(t) = config.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(p) = config.top_p {
        body["top_p"] = json!(p);
    }
    // 思考强度：none = 完全不下发 reasoning（跨端最稳的「关」——部分端点会把思考参数
    // 映射为自家字段如 thinking_budget 并做严格校验）。
    // 注意：extra_body 现以字面字段嵌套下发（见下），不再与内置顶层字段合并，
    // 因此无法经 extra_body 覆写 reasoning 等内置参数；关闭思考用档位 none。
    if let Some(effort) = &config.reasoning_effort {
        if effort != "none" {
            body["reasoning"] = json!({ "effort": effort });
        }
    }
    if config.web_search {
        body["tools"] = json!([{ "type": "web_search" }]);
    }
    if let Some((name, schema)) = format {
        body["text"] = json!({
            "format": { "type": "json_schema", "name": name, "strict": true, "schema": schema }
        });
    }
    // extra_body：整体作为请求体的字面 extra_body 字段下发（网关按该字段解包/透传自定义
    // 参数），绝不平铺进顶层——顶层拼接会被严格端点判为未知参数 400 或被静默丢弃。
    // 修订自 ADR-0016 D2 原语义（原为并入顶层模拟 SDK 客户端合并行为）。
    if let Some(obj) = config.extra_body.as_object() {
        if !obj.is_empty() {
            body["extra_body"] = Value::Object(obj.clone());
        }
    }
    body
}

/// 从 Response.output 提取 message/output_text 全文
fn extract_output_text(resp: &Value) -> String {
    let mut out = String::new();
    if let Some(items) = resp["output"].as_array() {
        for item in items {
            if item["type"].as_str() != Some("message") {
                continue;
            }
            if let Some(parts) = item["content"].as_array() {
                for p in parts {
                    if p["type"].as_str() == Some("output_text") {
                        if let Some(t) = p["text"].as_str() {
                            out.push_str(t);
                        }
                    }
                }
            }
        }
    }
    out
}

/// usage 归一化：Responses 的 input_tokens/output_tokens → 旧读法 usage.prompt_tokens/completion_tokens
fn normalize_meta(config: &LlmConfig, kind: &str, resp: &Value) -> Value {
    let u = &resp["usage"];
    let pt = u["input_tokens"].as_u64().or(u["prompt_tokens"].as_u64()).unwrap_or(0);
    let ct = u["output_tokens"].as_u64().or(u["completion_tokens"].as_u64()).unwrap_or(0);
    json!({
        "ir_mode": "structured",
        "ir_kind": kind,
        "id": resp["id"].clone(),
        "model": config.model,
        "provider": config.provider,
        "status": resp["status"].clone(),
        "usage": { "prompt_tokens": pt, "completion_tokens": ct, "total_tokens": pt + ct },
    })
}

fn record_metrics(config: &LlmConfig, duration_ms: u64, ok: bool, meta: Option<&Value>) {
    metrics::m()
        .llm_calls
        .with_label_values(&[&config.provider, if ok { "ok" } else { "err" }])
        .inc();
    metrics::m()
        .llm_duration
        .with_label_values(&[&config.provider])
        .observe(duration_ms as f64 / 1000.0);
    if !ok {
        metrics::m().llm_errors.inc();
    }
    if let Some(m) = meta {
        let pt = m["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        let ct = m["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        metrics::m().llm_tokens.inc_by(pt + ct);
    }
}

/// 非 4xx 明细的错误信息整理：404/405 给「不支持 Responses API」明确提示（ADR-0016 D1）
pub(super) fn friendly_llm_err(e: &async_openai::error::OpenAIError) -> AppError {
    use async_openai::error::OpenAIError as E;
    let unsupported_responses = |code: u16| matches!(code, 404 | 405);
    match e {
        E::ApiError(resp) => {
            let code = resp.status_code.as_u16();
            let raw_msg = &resp.api_error.message;
            let msg = format!("LLM 返回 {code}: {}", truncate(raw_msg, 300));
            let low = raw_msg.to_lowercase();
            // 思考参数签名：部分端点把 reasoning.effort 映射为自有思考预算字段或不支持扩展档（xhigh/max）
            if low.contains("thinking_budget")
                || low.contains("thinking budget")
                || low.contains("reasoning")
                || low.contains("reasoning_effort")
                || low.contains("effort")
                || low.contains("unknown parameter: 'reasoning'")
            {
                return AppError::BadRequest(format!(
                    "{msg}。该端点不支持当前思考参数或档位：请在模型高级参数中把思考强度调低（如 low/medium/high）或选 none 关闭"
                ));
            }
            if unsupported_responses(code) {
                AppError::BadRequest(format!(
                    "{msg}。该端点疑似不支持 OpenAI Responses API（应 POST {{base_url}}/responses）；\
                     请确认 base_url 指向支持该协议的服务（如 https://api.openai.com/v1），\
                     chat/completions 兼容网关（DashScope/Ollama 等）不可用"
                ))
            } else if (300..400).contains(&code) {
                AppError::BadRequest(format!(
                    "{msg}。端点发生了重定向（常见原因：base_url 用了 http:// 被重定向到 https，\
                     重定向后 POST 会降级为 GET 而失败）；请直接填写最终地址（通常改为 https://）"
                ))
            } else {
                AppError::BadRequest(msg)
            }
        }
        other => AppError::BadRequest(format!("LLM 请求失败: {other}")),
    }
}

/// 非 4xx 明细的错误信息整理（内部）：截断上游消息正文
pub(super) fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out = String::new();
        for c in s.chars() {
            if out.chars().count() >= n {
                break;
            }
            out.push(c);
        }
        format!("{out}...")
    }
}

/// 非流式请求咽喉点：span + metrics 记录在此一次性完成，调用方不再重复记录。
/// 返回 (输出全文, 归一化元数据)。
pub(crate) async fn respond(
    config: &LlmConfig,
    kind: &'static str,
    body: Value,
) -> Result<(String, Value), AppError> {
    let span = tracing::info_span!(
        target: "remote",
        "remote.llm",
        remote.service = %config.provider,
        remote.operation = kind,
        remote.model = %config.model,
    );
    async {
        let start = std::time::Instant::now();
        let c = client(config)?;
        // 失败时要把完整出站请求体带进日志（诊断 extra_body/思考强度等参数类 400）
        let body_for_log = serde_json::to_string(&body).unwrap_or_default();
        let result: Result<Value, _> = c.responses().create_byot::<_, Value>(body).await;
        match result {
            Ok(resp) => {
                if let Some(err) = resp.get("error").filter(|v| !v.is_null()) {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    record_metrics(config, duration_ms, false, None);
                    let err_str = truncate(&err.to_string(), 300);
                    tracing::warn!(
                        target: "remote",
                        event = "remote.llm.error_response",
                        remote.service = %config.provider,
                        remote.operation = kind,
                        remote.model = %config.model,
                        duration_ms,
                        error = %err_str,
                        request_body = %body_for_log,
                        "upstream returned error field"
                    );
                    return Err(AppError::BadRequest(format!("LLM 返回错误: {err_str}")));
                }
                let status = resp["status"].as_str().unwrap_or("");
                if status == "failed" {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    record_metrics(config, duration_ms, false, None);
                    let msg = resp["error"]["message"].as_str().unwrap_or("unknown");
                    tracing::error!(
                        target: "remote",
                        event = "remote.llm.failed",
                        remote.service = %config.provider,
                        remote.operation = kind,
                        remote.model = %config.model,
                        duration_ms,
                        error = %msg,
                        request_body = %body_for_log,
                        "LLM processing failed"
                    );
                    return Err(AppError::BadRequest(format!("LLM 处理失败: {}", truncate(msg, 300))));
                }
                if status == "incomplete" {
                    tracing::warn!(
                        target: "remote",
                        event = "remote.llm.incomplete",
                        remote.service = %config.provider,
                        remote.operation = kind,
                        remote.model = %config.model,
                        reason = %resp["incomplete_details"],
                        "response incomplete"
                    );
                }
                let text = extract_output_text(&resp);
                if text.is_empty() {
                    let duration_ms = start.elapsed().as_millis() as u64;
                    record_metrics(config, duration_ms, false, None);
                    return Err(AppError::BadRequest("LLM 响应缺少文本输出".to_string()));
                }
                let meta = normalize_meta(config, kind, &resp);
                let duration_ms = start.elapsed().as_millis() as u64;
                let pt = meta["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
                let ct = meta["usage"]["completion_tokens"].as_u64().unwrap_or(0);
                record_metrics(config, duration_ms, true, Some(&meta));
                tracing::info!(
                    target: "remote",
                    event = "remote.llm.completed",
                    remote.service = %config.provider,
                    remote.operation = kind,
                    remote.model = %config.model,
                    duration_ms,
                    tokens_used = pt + ct,
                    content_chars = text.chars().count(),
                    status = 200,
                    "remote llm request completed"
                );
                Ok((text, meta))
            }
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                record_metrics(config, duration_ms, false, None);
                tracing::error!(
                    target: "remote",
                    event = "remote.llm.failed",
                    remote.service = %config.provider,
                    remote.operation = kind,
                    remote.model = %config.model,
                    duration_ms,
                    error = %e,
                    request_body = %body_for_log,
                    "remote llm request failed"
                );
                Err(friendly_llm_err(&e))
            }
        }
    }
    .instrument(span)
    .await
}

// ---------- 供契约层调用的底层原语（crate 内可见；业务 route 不再直接使用） ----------

/// 启发式修复常见 LLM JSON 缺陷（围栏、尾随逗号、未转义引号、未转义控制字符、冗余包裹等）
fn repair_json_string(s: &str) -> Option<String> {
    let trimmed = s.trim();
    // 1. 尝试直接提取 {...} 或 [...] 边界
    let mut candidate = if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            trimmed[start..=end].to_string()
        } else {
            trimmed.to_string()
        }
    } else if let (Some(start), Some(end)) = (trimmed.find('['), trimmed.rfind(']')) {
        if start < end {
            trimmed[start..=end].to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        trimmed.to_string()
    };

    // 2. 检查多层冗余括号包裹，例如 `{{ ... }}` -> `{ ... }`
    while candidate.starts_with("{{") && candidate.ends_with("}}") && candidate.len() >= 4 {
        let inner = candidate[1..candidate.len() - 1].trim();
        if inner.starts_with('{') && inner.ends_with('}') {
            candidate = inner.to_string();
        } else {
            break;
        }
    }

    // 3. 移除尾随逗号与简单注释
    candidate = remove_trailing_commas_and_comments(&candidate);

    // 4. 修复未转义的控制字符与未转义的双引号
    candidate = repair_string_literals(&candidate);

    Some(candidate)
}

fn remove_trailing_commas_and_comments(input: &str) -> String {
    // 过滤 // 注释行
    let mut cleaned_lines = Vec::new();
    for line in input.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.starts_with("//") {
            continue;
        }
        cleaned_lines.push(line);
    }
    let joined = cleaned_lines.join("\n");

    // 移除尾随逗号（如 `,\n}` / `, ]` 等）
    let chars: Vec<char> = joined.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ',' {
            // 查看下一个非空白字符是否是 '}' 或 ']'
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                // 跳过此逗号
                i += 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn repair_string_literals(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(chars.len() + 16);
    let mut in_string = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if !in_string {
            if c == '"' {
                in_string = true;
                out.push(c);
            } else {
                out.push(c);
            }
            i += 1;
        } else {
            // inside string literal
            if c == '\\' {
                out.push(c);
                if i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            } else if c == '\n' {
                out.push_str("\\n");
                i += 1;
            } else if c == '\r' {
                out.push_str("\\r");
                i += 1;
            } else if c == '\t' {
                out.push_str("\\t");
                i += 1;
            } else if c == '"' {
                // Lookahead: 是否是字符串闭合引号
                // 闭合引号后跟可选空白以及 `,`、`}`、`]`、`:` 或 EOF
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                let is_closing = if j >= chars.len() {
                    true
                } else {
                    let next = chars[j];
                    next == ',' || next == '}' || next == ']' || next == ':'
                };

                if is_closing {
                    in_string = false;
                    out.push('"');
                } else {
                    // 内部未转义双引号 -> 转义为 \"
                    out.push_str("\\\"");
                }
                i += 1;
            } else {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// 宽容 JSON 解析（剥围栏 + 启发式修复）；strict schema 下通常已是纯 JSON。
pub fn parse_json_loose(s: &str) -> Result<Value, String> {
    let cleaned = strip_fences(s);
    match serde_json::from_str(cleaned) {
        Ok(v) => Ok(v),
        Err(e) => {
            tracing::warn!(raw_output = %cleaned, err = %e, "初次 JSON 解析失败，尝试启发式修复");
            if let Some(repaired) = repair_json_string(cleaned) {
                if let Ok(v) = serde_json::from_str(&repaired) {
                    tracing::info!("启发式 JSON 语法修复成功");
                    return Ok(v);
                }
            }
            Err(format!("JSON 解析失败: {e}，原文: {}", truncate(cleaned, 300)))
        }
    }
}

fn strip_fences(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix("```json").or_else(|| s.strip_prefix("```")).unwrap_or(s)
        .strip_suffix("```").unwrap_or(s)
        .trim()
}

/// 验证连通性（设置页「测试连接」）：下发包含当前思考档位、温度及 extra_body 的完整真实请求。
/// 404/405 → 明确提示端点不支持 Responses API。
pub async fn test_connection(config: &LlmConfig) -> Result<(), AppError> {
    let span = tracing::info_span!(
        target: "remote",
        "remote.llm.test",
        remote.service = %config.provider,
        remote.model = %config.model,
    );
    async {
        let body = build_body(
            config,
            &[
                json!({ "role": "system", "content": "连通性测试，回复 pong 即可。" }),
                json!({ "role": "user", "content": "ping" }),
            ],
            None,
            Some(16),
            false,
            None,
        );
        let body_for_log = serde_json::to_string(&body).unwrap_or_default();
        tracing::debug!(
            target: "remote",
            event = "remote.llm.test_started",
            remote.service = %config.provider,
            remote.model = %config.model,
            request_body = %body_for_log,
            "starting connectivity test"
        );
        let c = client(config)?;
        match c.responses().create_byot::<_, Value>(body).await {
            Ok(resp) => {
                tracing::info!(
                    target: "remote",
                    event = "remote.llm.test_completed",
                    remote.service = %config.provider,
                    remote.model = %config.model,
                    response = %resp.to_string(),
                    "connectivity test succeeded"
                );
                Ok(())
            }
            Err(e) => {
                tracing::error!(
                    target: "remote",
                    event = "remote.llm.test_failed",
                    remote.service = %config.provider,
                    remote.model = %config.model,
                    error = %e,
                    request_body = %body_for_log,
                    "connectivity test failed"
                );
                Err(friendly_llm_err(&e))
            }
        }
    }
    .instrument(span)
    .await
}

// ---------- 流式（陪练对话 / AI 讲解，SSE 契约不变：Content 正文增量 / Thinking 思考增量） ----------

pub enum StreamItem {
    Content(String),
    Thinking(String),
    /// 流结束元数据：响应顶层 `id`（UUID 形态）。多轮对话可在下一次请求将其作为
    /// `previous_response_id` 链式关联上下文（上游默认保留 7 天；不可用 output 数组里的 `msg_*` id）。
    Completed(String),
}

/// 流式对话。`previous_response_id`：
/// - `Some(id)`：链式续接该响应的上下文，`messages` 只需携带新增内容（无需重放全部历史）；
/// - `None`：全量模式，`messages` 自带完整上下文。
///
/// 注意：链式要求上游留存响应（`store=true`）；对不留存的端点传了也会被拒，
/// 由调用方按错误签名兜底回退全量模式（见 drills::send_message）。
pub fn stream_chat(
    config: LlmConfig,
    messages: Vec<Value>,
    previous_response_id: Option<String>,
) -> impl futures_util::Stream<Item = Result<StreamItem, AppError>> {
    use futures_util::StreamExt as _;
    async_stream::stream! {
        let span = tracing::info_span!(
            target: "remote",
            "remote.llm.stream",
            remote.service = %config.provider,
            remote.model = %config.model,
            chained = previous_response_id.is_some()
        );
        let _g = span.enter();
        let start = std::time::Instant::now();
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut chars: u64 = 0;
        let mut failed: Option<String> = None;
        // 流式不限输出长度（旧行为：不带 max_output_tokens），其余高级参数/工具/extra_body 与非流式一致；
        // stream:true 在 build_body 内显式带上（byot 流式分支不会自动补，评审 P0 收口为唯一组装点）。
        let body = build_body(
            &config,
            &messages,
            None,
            None,
            true,
            previous_response_id.as_deref(),
        );

        match client(&config) {
            Ok(c) => {
                match c.responses().create_stream_byot::<_, Value>(body).await {
                    Ok(mut event_stream) => {
                        loop {
                            // 流空闲超时：长时间无数据则中断（防止上游挂死）
                            let ev = match tokio::time::timeout(Duration::from_secs(120), event_stream.next()).await {
                                Ok(item) => item,
                                Err(_) => {
                                    failed = Some("LLM 流空闲超时".to_string());
                                    break;
                                }
                            };
                            let Some(ev) = ev else { break };
                            match ev {
                                Ok(ev) => {
                                    let ty = ev["type"].as_str().unwrap_or("");
                                    match ty {
                                        "response.output_text.delta" => {
                                            if let Some(d) = ev["delta"].as_str() {
                                                if !d.is_empty() {
                                                    chars += d.chars().count() as u64;
                                                    yield Ok(StreamItem::Content(d.to_string()));
                                                }
                                            }
                                        }
                                        // 思考过程增量（summary 或原生 reasoning 文本）：独立事件转发，不入正文
                                        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                                            if let Some(t) = ev["delta"].as_str() {
                                                if !t.is_empty() {
                                                    yield Ok(StreamItem::Thinking(t.to_string()));
                                                }
                                            }
                                        }
                                        "response.completed" | "response.incomplete" => {
                                            let u = &ev["response"]["usage"];
                                            prompt_tokens = u["input_tokens"].as_u64().unwrap_or(0);
                                            completion_tokens = u["output_tokens"].as_u64().unwrap_or(0);
                                            // 响应顶层 id（UUID 形态）：供多轮链式 previous_response_id 使用
                                            if let Some(id) = ev["response"]["id"].as_str() {
                                                yield Ok(StreamItem::Completed(id.to_string()));
                                            }
                                            if ty == "response.incomplete" {
                                                tracing::warn!(reason = %ev["response"]["incomplete_details"], "流式响应不完整");
                                            }
                                            break;
                                        }
                                        "response.failed" => {
                                            let msg = ev["response"]["error"]["message"].as_str().unwrap_or("unknown");
                                            failed = Some(format!("LLM 处理失败: {}", truncate(msg, 300)));
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                                Err(e) => {
                                    failed = Some(format!("读取流失败: {e}"));
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        failed = Some(friendly_llm_err(&e).to_string());
                    }
                }
            }
            Err(e) => failed = Some(e.to_string()),
        }

        // 观测：时长 + token（真实用量或字符估算）+ 调用结果
        let duration_ms = start.elapsed().as_millis() as u64;
        let total = if completion_tokens > 0 {
            prompt_tokens + completion_tokens
        } else {
            chars / 3 + 8
        };
        match &failed {
            Some(msg) => {
                metrics::m().llm_errors.inc();
                metrics::m()
                    .llm_calls
                    .with_label_values(&[&config.provider, "err"])
                    .inc();
                tracing::error!(
                    target: "remote",
                    event = "remote.llm.stream_failed",
                    remote.service = %config.provider,
                    remote.model = %config.model,
                    duration_ms,
                    error = %msg,
                    "llm stream failed"
                );
                yield Err(AppError::BadRequest(msg.clone()));
            }
            None => {
                metrics::m()
                    .llm_calls
                    .with_label_values(&[&config.provider, "ok"])
                    .inc();
                metrics::m()
                    .llm_duration
                    .with_label_values(&[&config.provider])
                    .observe(duration_ms as f64 / 1000.0);
                metrics::m().llm_tokens.inc_by(total);
                tracing::info!(
                    target: "remote",
                    event = "remote.llm.stream_completed",
                    remote.service = %config.provider,
                    remote.model = %config.model,
                    duration_ms,
                    prompt_tokens,
                    completion_tokens,
                    tokens_used = total,
                    "llm stream completed"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LlmConfig {
        LlmConfig {
            provider: "OpenAI".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk".into(),
            model: "gpt-5.2".into(),
            structured_output: true,
            web_search: true,
            context_length: Some(400000),
            temperature: Some(0.5),
            top_p: None,
            reasoning_effort: Some("xhigh".into()),
            store: false,
            extra_body: json!({"enable_thinking": true}),
            timeout: 120,
            max_tokens: 4096,
            max_tokens_long: 8192,
        }
    }

    #[test]
    fn build_body_hoists_system_and_nests_extra_body() {
        let c = cfg();
        let msgs = vec![
            json!({"role":"system","content":"sys"}),
            json!({"role":"user","content":"u1"}),
            json!({"role":"assistant","content":"a1"}),
            json!({"role":"user","content":"u2"}),
        ];
        // 引擎不携带任何业务 schema（已上移契约层）；用最小示意 schema 验证组装行为
        let schema = json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false});
        let b = build_body(&c, &msgs, Some(("interview_analysis", &schema)), Some(4096), false, None);
        assert_eq!(b["model"], "gpt-5.2");
        assert_eq!(b["store"], false); // Q6：默认 false 显式下发
        assert_eq!(b["instructions"], "sys");
        assert_eq!(b["input"].as_array().unwrap().len(), 3);
        assert_eq!(b["max_output_tokens"], 4096);
        assert_eq!(b["temperature"], 0.5);
        assert_eq!(b["reasoning"]["effort"], "xhigh"); // 七档 effort 原样下发
        assert_eq!(b["tools"][0]["type"], "web_search"); // 能力位 → 内置工具
        assert_eq!(b["text"]["format"]["type"], "json_schema");
        assert_eq!(b["text"]["format"]["strict"], true);
        assert_eq!(b["text"]["format"]["name"], "interview_analysis");
        assert_eq!(b["extra_body"]["enable_thinking"], true); // extra_body 以字面字段嵌套下发
        assert!(b.get("enable_thinking").is_none(), "extra_body 不得平铺进请求体顶层");
        assert!(b.get("stream").is_none()); // 非流式不带 stream 字段
        assert!(b.get("previous_response_id").is_none());
        // strict schema 纪律：全 required + additionalProperties false
        let sch = &b["text"]["format"]["schema"];
        assert_eq!(sch["additionalProperties"], false);
        let req = sch["required"].as_array().unwrap();
        let props = sch["properties"].as_object().unwrap();
        assert_eq!(req.len(), props.len());
    }

    #[test]
    fn build_body_carries_stream_and_chain_params() {
        // P0 收口：stream 只能经 build_body 入参下发，不允许旁路改 body；
        // previous_response_id 用响应顶层 id（UUID 形态），绝不取 output 里的 msg_* id。
        let c = cfg();
        let b = build_body(
            &c,
            &[json!({"role":"user","content":"下一题"})],
            None,
            None,
            true,
            Some("f0dbb153-117f-9bbf-8176-5284b47f3001"),
        );
        assert_eq!(b["stream"], true);
        assert_eq!(b["previous_response_id"], "f0dbb153-117f-9bbf-8176-5284b47f3001");
    }

    #[test]
    fn build_body_omits_absent_params() {
        let c = LlmConfig {
            store: true,
            structured_output: false,
            web_search: false,
            context_length: None,
            temperature: None,
            top_p: None,
            reasoning_effort: None,
            ..cfg()
        };
        let b = build_body(&c, &[json!({"role":"user","content":"hi"})], None, None, false, None);
        assert!(b.get("instructions").is_none()); // 无 system 不空发 instructions
        assert!(b.get("max_output_tokens").is_none());
        assert!(b.get("temperature").is_none());
        assert!(b.get("reasoning").is_none()); // effort 未配置不下发
        assert!(b.get("tools").is_none()); // 无能力位不带工具
        assert!(b.get("text").is_none()); // 无 schema 不带格式
        assert!(b.get("stream").is_none()); // 非流式不带 stream 字段
        assert!(b.get("previous_response_id").is_none());
        assert_eq!(b["store"], true); // 用户显式开启（Q6 可改 true）
    }

    #[test]
    fn build_body_omits_reasoning_on_none_effort() {
        // none 档 = 不下发 reasoning（部分端点将思考参数映射为 thinking_budget 等自有字段并严格校验）
        let c = LlmConfig { reasoning_effort: Some("none".into()), ..cfg() };
        let b = build_body(&c, &[json!({"role":"user","content":"hi"})], None, None, false, None);
        assert!(b.get("reasoning").is_none(), "none 档不应下发 reasoning");
        // 修订后：需要显式 effort:"none" 的端点经 extra_body 字面嵌套自行下发，
        // 不再覆写内置顶层字段（extra_body 不与内置参数合并）
        let c2 = LlmConfig { extra_body: json!({"reasoning": {"effort": "none"}}), ..c };
        let b2 = build_body(&c2, &[json!({"role":"user","content":"hi"})], None, None, false, None);
        assert!(b2.get("reasoning").is_none(), "extra_body 不再覆写内置顶层字段");
        assert_eq!(b2["extra_body"]["reasoning"]["effort"], "none");
    }

    #[test]
    fn friendly_err_hints_thinking_budget_downgrade() {
        use async_openai::error::{ApiError, ApiErrorResponse, OpenAIError as E};
        let err = E::ApiError(ApiErrorResponse {
            status_code: reqwest::StatusCode::BAD_REQUEST,
            api_error: ApiError {
                message: "InternalError.Algo.InvalidParameter: The thinking_budget parameter must be a positive integer and not greater than 131072".into(),
                r#type: None,
                param: None,
                code: None,
            },
        });
        let msg = friendly_llm_err(&err).to_string();
        assert!(msg.contains("思考强度调低"), "应给降档指引：{msg}");
    }

    #[test]
    fn extract_output_text_walks_items() {
        let resp = json!({
            "output": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "content": [
                    {"type": "output_text", "text": "Hello"},
                    {"type": "output_text", "text": " 世界"}
                ]}
            ]
        });
        assert_eq!(extract_output_text(&resp), "Hello 世界");
    }

    #[test]
    fn parse_json_loose_strips_fences() {
        let v = parse_json_loose("```json\n{\"a\":1}\n```").unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn parse_json_loose_handles_unescaped_quotes() {
        let raw = r#"{"summary": "面试官问了 "Rust 为什么快" 这个关键问题", "score": 85}"#;
        let v = parse_json_loose(raw).unwrap();
        assert_eq!(v["score"], 85);
        assert!(v["summary"].as_str().unwrap().contains("Rust 为什么快"));
    }

    #[test]
    fn parse_json_loose_handles_trailing_commas_and_comments() {
        let raw = r#"{
            // 这里是分析结果
            "items": [
                "item1",
                "item2",
            ],
            "valid": true,
        }"#;
        let v = parse_json_loose(raw).unwrap();
        assert_eq!(v["valid"], true);
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parse_json_loose_handles_redundant_braces_and_commentary() {
        let raw = "Here is the result:\n```json\n{{\n  \"score\": 95,\n  \"feedback\": \"excellent\"\n}}\n```\nHope it helps!";
        let v = parse_json_loose(raw).unwrap();
        assert_eq!(v["score"], 95);
        assert_eq!(v["feedback"], "excellent");
    }

    #[test]
    fn parse_json_loose_handles_unescaped_newlines_in_strings() {
        let raw = "{\n  \"multiline\": \"line 1\nline 2\nline 3\",\n  \"count\": 3\n}";
        let v = parse_json_loose(raw).unwrap();
        assert_eq!(v["count"], 3);
        assert!(v["multiline"].as_str().unwrap().contains("line 1"));
    }
}
