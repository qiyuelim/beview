use prometheus::{HistogramOpts, HistogramVec, IntCounter, IntCounterVec, Opts, Registry, TextEncoder};
use std::sync::OnceLock;

pub struct Metrics {
    pub http_requests: IntCounterVec,
    pub http_duration: HistogramVec,
    pub llm_calls: IntCounterVec,
    pub llm_duration: HistogramVec,
    pub llm_tokens: IntCounter,
    pub llm_errors: IntCounter,
    registry: Registry,
}

static METRICS: OnceLock<Metrics> = OnceLock::new();

pub fn m() -> &'static Metrics {
    METRICS.get_or_init(|| {
        let registry = Registry::new();
        let http_requests = IntCounterVec::new(
            Opts::new("http_requests_total", "HTTP 请求总数"),
            &["method", "status"],
        )
        .unwrap();
        let http_duration = HistogramVec::new(
            HistogramOpts::new("http_request_duration_seconds", "HTTP 请求耗时"),
            &["method", "path"],
        )
        .unwrap();
        let llm_calls = IntCounterVec::new(
            Opts::new("llm_calls_total", "LLM 调用数"),
            &["provider", "ok"],
        )
        .unwrap();
        let llm_duration = HistogramVec::new(
            HistogramOpts::new("llm_duration_seconds", "LLM 调用耗时"),
            &["provider"],
        )
        .unwrap();
        let llm_tokens = IntCounter::new("llm_tokens_total", "LLM token 总数").unwrap();
        let llm_errors = IntCounter::new("llm_errors_total", "LLM 错误数").unwrap();
        registry
            .register(Box::new(http_requests.clone()) as Box<dyn prometheus::core::Collector>)
            .unwrap();
        registry
            .register(Box::new(http_duration.clone()) as Box<dyn prometheus::core::Collector>)
            .unwrap();
        registry
            .register(Box::new(llm_calls.clone()) as Box<dyn prometheus::core::Collector>)
            .unwrap();
        registry
            .register(Box::new(llm_duration.clone()) as Box<dyn prometheus::core::Collector>)
            .unwrap();
        registry
            .register(Box::new(llm_tokens.clone()) as Box<dyn prometheus::core::Collector>)
            .unwrap();
        registry
            .register(Box::new(llm_errors.clone()) as Box<dyn prometheus::core::Collector>)
            .unwrap();
        Metrics {
            http_requests,
            http_duration,
            llm_calls,
            llm_duration,
            llm_tokens,
            llm_errors,
            registry,
        }
    })
}

pub fn render() -> String {
    TextEncoder::new()
        .encode_to_string(&m().registry.gather())
        .unwrap_or_default()
}
