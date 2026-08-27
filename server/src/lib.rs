//! Beview 后端库（main 与集成测试共用）。
//! 可测试性 seam：`build_api` 把「真实 axum 路由 + 认证中间件」组装成 Router，
//! 集成测试通过 HTTP 打真实接口（见 tests/），而非测内部实现。

pub mod auth;
pub mod config;
pub mod contracts;
pub mod crypto;
pub mod db;
pub mod error;
pub mod events;
pub mod llm;
pub mod metrics;
pub mod models;
pub mod observe;
pub mod points;
pub mod prompts;
pub mod routes;
pub mod services;
pub mod settings;
pub mod state;

use axum::middleware;
use axum::Router;

use crate::state::AppState;

/// 组装完整应用路由（/api 子路由含认证中间件 + 静态回退）。main 与测试共用同一组装：
/// 测试打到与生产一致的完整路径（如 /api/review/queue）。
pub fn build_api(app_state: AppState) -> Router {
    let api = routes::api_router()
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            routes::require_auth,
        ))
        .with_state(app_state);
    Router::new()
        .nest("/api", api)
        .layer(middleware::from_fn(observe::http_trace_mw))
}
