//! 可观测性装配（ADR-0003 + 反馈八·分类日志）：
//! - 结构化 JSON 日志（带 span 列表，全链 span 终端可见）
//! - 分类分文件：logs/{interface,remote,db,error,app}/ 按日滚动
//!   · interface — HTTP 访问（target=app::interface，traceparent_mw 打点）
//!   · remote    — 出站远程调用（LLM 等，target=server::llm*）
//!   · db        — SQL（target=sqlx*）
//!   · error     — 全部 ERROR 级事件（任何类别，单独抽取便于告警巡检）
//!   · app       — 兜底全量主日志（INFO+，替代原单文件 server.log）
//! - OpenTelemetry：配置了 OTLP endpoint 才导出 span（graceful degradation），否则 noop
//! - traceparent 透传：浏览器 -> HTTP -> DB -> LLM 同一 trace

use std::path::{Path, PathBuf};

use opentelemetry::trace::TracerProvider as _;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::filter::FilterFn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

/// 非 blocking writer 的 guard 必须保活到进程结束——main 持有本结构即可。
pub struct LogGuards {
    _guards: Vec<WorkerGuard>,
}

/// 默认日志根目录：<workspace>/logs
pub fn default_log_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../logs")
}

pub fn init(otlp_endpoint: Option<&str>, log_dir: &Path) -> LogGuards {
    let _ = std::fs::create_dir_all(log_dir);
    let mut guards: Vec<WorkerGuard> = Vec::new();



    // ---- 路由谓词（基于 callsite 元数据，不做字符串格式化） ----
    fn is_interface(md: &tracing::Metadata<'_>) -> bool {
        md.target() == "interface" || md.target() == "app::interface"
    }
    fn is_remote(md: &tracing::Metadata<'_>) -> bool {
        md.target() == "remote" || md.target().starts_with("server::llm")
    }
    fn is_db(md: &tracing::Metadata<'_>) -> bool {
        md.target() == "database" || md.target().starts_with("sqlx")
    }
    fn is_audit(md: &tracing::Metadata<'_>) -> bool {
        md.target() == "audit" || md.target() == "security"
    }
    fn is_error(md: &tracing::Metadata<'_>) -> bool {
        md.level() == &tracing::Level::ERROR
    }
    fn not_sql(md: &tracing::Metadata<'_>) -> bool {
        !md.target().starts_with("sqlx") && md.target() != "database"
    }

    type BoxLayer = Box<dyn Layer<Registry> + Send + Sync>;

    // 分类抽取层：JSON 格式（结构化查询友好）
    let json_layer = |class: &'static str,
                      pred: for<'a, 'b> fn(&'a tracing::Metadata<'b>) -> bool,
                      guards: &mut Vec<WorkerGuard>| {
        let (w, g) = tracing_appender::non_blocking(rolling::daily(log_dir.join(class), "log"));
        guards.push(g);
        tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .with_writer(w)
            .with_filter(FilterFn::new(pred))
            .boxed()
    };

    let interface_layer: BoxLayer = json_layer("interface", is_interface, &mut guards);
    let remote_layer: BoxLayer = json_layer("remote", is_remote, &mut guards);
    let db_layer: BoxLayer = json_layer("db", is_db, &mut guards);
    let audit_layer: BoxLayer = json_layer("audit", is_audit, &mut guards);
    let error_layer: BoxLayer = json_layer("error", is_error, &mut guards);

    // 运行主日志（app）：易读文本格式——追踪项目运行动作（非 JSON）；SQL 单独进 db 目录
    let (app_w, app_g) = tracing_appender::non_blocking(rolling::daily(log_dir.join("app"), "log"));
    guards.push(app_g);
    let app_layer: BoxLayer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(app_w)
        .with_filter(FilterFn::new(not_sql))
        .boxed();

    // stdout：开发终端输出（默认紧凑格式并附带 target；若开启 debug/trace/sqlx 过滤则展示全部日志含 SQL）
    let show_all_stdout = std::env::var("APP_LOG_ALL_STDOUT")
        .or_else(|_| std::env::var("RUST_LOG"))
        .map(|v| {
            let v = v.to_lowercase();
            v == "1" || v == "true" || v == "all" || v == "trace" || v == "debug" || v.contains("sqlx") || v.contains("database")
        })
        .unwrap_or(false);

    let stdout_layer: BoxLayer = if show_all_stdout {
        tracing_subscriber::fmt::layer()
            .with_target(true)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .compact()
            .with_target(true)
            .with_filter(FilterFn::new(not_sql))
            .boxed()
    };

    // OTel span 导出（配置 OTLP 才外发）
    let otel_layer: BoxLayer = match otlp_endpoint {
        Some(endpoint) => {
            use opentelemetry_otlp::WithExportConfig;
            let exporter = opentelemetry_otlp::new_exporter()
                .http()
                .with_endpoint(endpoint)
                .build_span_exporter()
                .expect("构建 OTLP span exporter 失败");
            let provider = opentelemetry_sdk::trace::TracerProvider::builder()
                .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
                .build();
            let tracer = provider.tracer("beview");
            opentelemetry::global::set_tracer_provider(provider);
            tracing_opentelemetry::layer().with_tracer(tracer).boxed()
        }
        None => {
            let provider = opentelemetry_sdk::trace::TracerProvider::builder().build();
            let tracer = provider.tracer("beview");
            opentelemetry::global::set_tracer_provider(provider);
            tracing_opentelemetry::layer().with_tracer(tracer).boxed()
        }
    };

    // 统一面向 Registry 构建，Vec 组合（顺序即语义：全局 filter 在最前）
    let filter: BoxLayer = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=debug,server=debug,sqlx=info"))
        .boxed();

    let layers: Vec<BoxLayer> = vec![
        filter,
        otel_layer,
        app_layer,
        interface_layer,
        remote_layer,
        db_layer,
        audit_layer,
        error_layer,
        stdout_layer,
    ];

    tracing_subscriber::registry().with(layers).init();

    LogGuards { _guards: guards }
}

/// 从单个 `traceparent` 请求头提取 OTel 上下文（W3C TraceContext）
pub struct TraceparentExtractor<'a>(pub &'a str);

impl opentelemetry::propagation::Extractor for TraceparentExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        if key.eq_ignore_ascii_case("traceparent") {
            Some(self.0)
        } else {
            None
        }
    }
    fn keys(&self) -> Vec<&str> {
        vec!["traceparent"]
    }
}

/// 判定 JSON 键是否为敏感凭证字段（精确/后缀/特定前缀匹配，避免误伤 tokens_used 等合法遥测字段）
fn is_sensitive_key(k: &str) -> bool {
    let k = k.to_lowercase();
    k.contains("password")
        || k.contains("api_key")
        || k.contains("secret")
        || k == "authorization"
        || k == "token"
        || k.ends_with("_token")
        || k.starts_with("token_")
        || k.contains("access_token")
        || k.contains("refresh_token")
        || k.contains("session_token")
        || k.contains("auth_token")
}

/// 递归脱敏 JSON 中的敏感字段
pub fn mask_sensitive_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_sensitive_key(k) {
                    *v = serde_json::json!("*** [REDACTED] ***");
                } else {
                    mask_sensitive_json(v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                mask_sensitive_json(item);
            }
        }
        _ => {}
    }
}

/// 安全截断 UTF-8 字符串至指定字节数上限，绝不在多字节字符中间断开
pub fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        let mut boundary = max_bytes;
        while boundary > 0 && !s.is_char_boundary(boundary) {
            boundary -= 1;
        }
        &s[..boundary]
    }
}

/// 字符级安全截断，超出上限时追加省略号（全仓共享助手）
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

/// 格式化并脱敏 body 字符串，长度限制 ~2KB (2048 字节)，UTF-8 安全截断
pub fn format_and_mask_body(raw_bytes: &[u8]) -> String {
    if raw_bytes.is_empty() {
        return String::new();
    }
    if let Ok(text) = std::str::from_utf8(raw_bytes) {
        if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(text) {
            mask_sensitive_json(&mut json_val);
            let s = json_val.to_string();
            if s.len() > 2048 {
                let valid = truncate_utf8(&s, 2048);
                format!("{valid}... [truncated]")
            } else {
                s
            }
        } else if text.len() > 2048 {
            let valid = truncate_utf8(text, 2048);
            format!("{valid}... [truncated]")
        } else {
            text.to_string()
        }
    } else {
        format!("<binary data {} bytes>", raw_bytes.len())
    }
}

/// 检查环境变量是否启用了 debug 级请求/响应体日志
pub fn is_debug_body_log_enabled() -> bool {
    std::env::var("APP_DEBUG_HTTP_BODY")
        .or_else(|_| std::env::var("DEBUG_BODY_LOG"))
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use axum::body::Bytes;
use tracing::Instrument;

/// 生成 32 位 Hex 随机 trace_id
pub fn generate_trace_id() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    hex::encode(bytes)
}

/// HTTP 跟踪中间件：
/// 1. 提取上游 traceparent / x-trace-id 或生成新 trace_id
/// 2. 注入根 span 与 OTel 上下文
/// 3. 回写响应头 x-trace-id
/// 4. 记录 interface 结构化日志
/// 5. 根据环境变量在 DEBUG 级别打印限长脱敏的请求与响应体（SSE 流式响应旁路直通）
pub async fn http_trace_mw(req: Request, next: Next) -> Response {
    let start = std::time::Instant::now();
    let path = req.uri().path().to_string();
    let method = req.method().to_string();

    let tp = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let req_trace_id = req
        .headers()
        .get("x-trace-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let trace_id = if let Some(id) = req_trace_id {
        id
    } else if !tp.is_empty() {
        let parts: Vec<&str> = tp.split('-').collect();
        if parts.len() >= 2 && parts[1].len() == 32 {
            parts[1].to_string()
        } else {
            generate_trace_id()
        }
    } else {
        generate_trace_id()
    };

    let span = tracing::info_span!(
        target: "interface",
        "http.request",
        http.method = %method,
        http.route = %path,
        trace_id = %trace_id,
        traceparent = %tp
    );

    if !tp.is_empty() {
        let cx = opentelemetry::global::get_text_map_propagator(|p| {
            p.extract(&TraceparentExtractor(&tp))
        });
        tracing_opentelemetry::OpenTelemetrySpanExt::set_parent(&span, cx);
    }

    let debug_body = is_debug_body_log_enabled();

    let (req, req_bytes) = if debug_body {
        let (parts, body) = req.into_parts();
        let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
            Ok(b) => b,
            Err(_) => Bytes::new(),
        };
        let new_req = Request::from_parts(parts, axum::body::Body::from(bytes.clone()));
        (new_req, Some(bytes))
    } else {
        (req, None)
    };

    if let Some(bytes) = &req_bytes {
        if !bytes.is_empty() {
            let masked = format_and_mask_body(bytes);
            tracing::debug!(
                target: "interface",
                parent: &span,
                event = "http.request.body",
                trace_id = %trace_id,
                http.method = %method,
                http.route = %path,
                body = %masked,
                "HTTP request body"
            );
        }
    }

    let mut resp = next.run(req).instrument(span.clone()).await;

    if let Ok(val) = axum::http::HeaderValue::from_str(&trace_id) {
        resp.headers_mut().insert("x-trace-id", val);
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    let status_code = resp.status().as_u16();

    if status_code >= 400 {
        tracing::warn!(
            target: "interface",
            parent: &span,
            event = "http.request.failed",
            trace_id = %trace_id,
            http.method = %method,
            http.route = %path,
            http.status_code = status_code,
            duration_ms = duration_ms,
            "http request failed"
        );
    } else {
        tracing::info!(
            target: "interface",
            parent: &span,
            event = "http.request.completed",
            trace_id = %trace_id,
            http.method = %method,
            http.route = %path,
            http.status_code = status_code,
            duration_ms = duration_ms,
            "http request completed"
        );
    }

    if debug_body {
        // SSE 流式响应（text/event-stream）绝不执行 to_bytes 缓冲，避免挂起打字机流式连接
        let is_sse = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map_or(false, |ct| ct.contains("text/event-stream"));

        if is_sse {
            tracing::debug!(
                target: "interface",
                parent: &span,
                event = "http.response.body",
                trace_id = %trace_id,
                http.status_code = status_code,
                body = "<streaming event-stream response bypass>",
                "HTTP response body"
            );
            resp
        } else {
            let (parts, body) = resp.into_parts();
            if let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024).await {
                let masked = format_and_mask_body(&bytes);
                tracing::debug!(
                    target: "interface",
                    parent: &span,
                    event = "http.response.body",
                    trace_id = %trace_id,
                    http.status_code = status_code,
                    body = %masked,
                    "HTTP response body"
                );
                Response::from_parts(parts, axum::body::Body::from(bytes))
            } else {
                tracing::debug!(
                    target: "interface",
                    parent: &span,
                    event = "http.response.body",
                    trace_id = %trace_id,
                    http.status_code = status_code,
                    body = "<response body exceeded 64KB or streaming>",
                    "HTTP response body"
                );
                Response::from_parts(parts, axum::body::Body::empty())
            }
        }
    } else {
        resp
    }
}
