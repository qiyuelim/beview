use std::net::SocketAddr;
use std::path::PathBuf;

use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use server::auth;
use server::build_api;
use server::config::Config;
use server::db;
use server::observe;
use server::state::AppState;

/// HTML 不缓存（反馈 #7 配套永久修复）：index.html 引用哈希资产，发新版后浏览器立即拉新
async fn html_no_cache(req: Request, next: Next) -> Response {
    let is_api = req.uri().path().starts_with("/api");
    let mut resp = next.run(req).await;
    if !is_api {
        let is_html = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("text/html"))
            .unwrap_or(false);
        if is_html {
            resp.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::header::HeaderValue::from_static("no-cache"),
            );
        }
    }
    resp
}

fn static_dir() -> PathBuf {
    // 生产期托管前端构建产物（server/static），dev 期由 Vite 负责
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}

/// 连接串脱敏（仅用于日志展示）：postgres://user:***@host:port/dbname
fn redacted_dsn(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("postgres://").or_else(|| url.strip_prefix("postgresql://")) {
        if let Some(at) = rest.find('@') {
            let host_part = &rest[at + 1..];
            let user = rest[..at].split(':').next().unwrap_or("");
            return format!("postgres://{user}:***@{host_part}");
        }
    }
    "<非标准连接串>".to_string()
}

#[tokio::main]
async fn main() {
    let cfg = Config::load();
    // 分类日志（logs/{interface,remote,db,error,app}），guard 必须保活到进程结束
    let log_dir = observe::default_log_dir();
    let _log_guards = observe::init(cfg.otlp_endpoint.as_deref(), &log_dir);

    // ---- 运行信息（启动摘要，进 app 主日志）----
    tracing::info!(
        target: "runtime",
        event = "runtime.started",
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        log_dir = %log_dir.display(),
        otlp = ?cfg.otlp_endpoint.as_deref().map(|_| "configured").unwrap_or("off"),
        "service started"
    );
    tracing::info!(
        target: "runtime",
        event = "runtime.config_loaded",
        bind = %cfg.bind_addr,
        session_ttl_hours = cfg.session_ttl_hours,
        dsn = %redacted_dsn(&cfg.database_url),
        "runtime configuration loaded"
    );
    tracing::info!(
        target: "runtime",
        event = "runtime.static_assets",
        static_dir = %static_dir().display(),
        exists = static_dir().exists(),
        "static directory verified"
    );

    // 连接 PostgreSQL 并执行迁移
    let pool = db::connect(&cfg.database_url).await;
    let app_state = AppState {
        pool: pool.clone(),
        sessions: auth::SessionStore::new(),
        session_ttl_hours: cfg.session_ttl_hours,
        batch_jobs: server::state::BatchJobs::new(),
        ai_jobs: server::state::AiJobs::new(),
        event_bus: server::events::EventBus::new(pool.clone()),
    };

    // 票09：后台任务队列启动恢复——上一进程遗留的 running 全部置回 pending，
    // 由 dispatcher 续跑（单实例语义，见 services::job_queue 模块注释）
    match server::services::job_queue::reset_running_on_boot(&app_state.pool).await {
        Ok(n) if n > 0 => tracing::warn!(
            target: "runtime",
            event = "jobqueue.recovered",
            recovered = n,
            "检测到上次进程遗留的后台任务，已重置为待派发"
        ),
        Ok(_) => {}
        Err(e) => tracing::error!(
            target: "runtime",
            event = "jobqueue.recover_failed",
            error = %e,
            "后台任务恢复失败"
        ),
    }
    // 遗留卡滞判卷恢复：drills 停在 grading 且无队列任务的自动补发入队
    match server::services::job_queue::recover_orphaned_paper_gradings(&app_state.pool).await {
        Ok(n) if n > 0 => tracing::warn!(
            target: "runtime",
            event = "jobqueue.orphans_recovered",
            recovered = n,
            "检测到遗留的卡滞判卷任务，已补全入队"
        ),
        Ok(_) => {}
        Err(e) => tracing::error!(
            target: "runtime",
            event = "jobqueue.orphan_recover_failed",
            error = %e,
            "遗留判卷任务恢复失败"
        ),
    }
    server::services::job_queue::spawn_dispatcher(app_state.clone());

    match sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _sqlx_migrations")
        .fetch_one(&app_state.pool)
        .await
    {
        Ok(v) => tracing::info!(
            target: "database",
            event = "database.ready",
            schema_version = v,
            "database ready"
        ),
        Err(e) => tracing::warn!(
            target: "database",
            event = "database.schema_version_failed",
            error = %e,
            "unable to read schema version"
        ),
    }

    let dist = static_dir();
    let spa = ServeDir::new(&dist).not_found_service(ServeFile::new(dist.join("index.html")));
    // build_api 已含 /api nest + 认证中间件 + state；此处再挂 traceparent（最外层）、
    // HTML 不缓存（发新版后浏览器立即拉新 index，哈希资产照常缓存）与静态回退
    let app = Router::new()
        .merge(build_api(app_state))
        .fallback_service(spa)
        .layer(middleware::from_fn(html_no_cache));

    let bind_addr: SocketAddr = cfg.bind_addr.parse().expect("bind_addr 非法");
    let listener = tokio::net::TcpListener::bind(bind_addr).await.unwrap();
    tracing::info!(
        target: "runtime",
        event = "runtime.listening",
        %bind_addr,
        "server listening"
    );
    axum::serve(listener, app).await.unwrap();
}
