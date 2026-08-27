pub mod companies;
pub mod comments;
pub mod dashboard;
pub mod export;
pub mod drills;
pub mod questions;
pub mod resume;
pub mod review;
pub mod sessions;
pub mod settings;
pub mod points;
pub mod personas;
pub mod mall;
pub mod applications;
pub mod batch;
pub mod stats;
pub mod calendar;
pub mod ai_jobs;
pub mod skills;

use std::time::Instant;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use axum::extract::{Path, Request};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::{self, CurrentUser};
use crate::error::AppError;
use crate::metrics;
use crate::models::{LoginReq, SetupReq, UserRow};
use crate::state::AppState;

/// 组装全部 /api 路由（认证中间件在 main 中挂载，因需 state）
pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/setup/status", get(setup_status))
        .route("/setup", post(setup))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .merge(companies::routes())
        .merge(sessions::routes())
        .merge(questions::routes())
        .merge(comments::routes())
        .merge(settings::routes())
        .merge(dashboard::routes())
        .merge(export::routes())
        .merge(drills::routes())
        .merge(personas::routes())
        .merge(review::routes())
        .merge(resume::routes())
        .merge(points::routes())
        .merge(mall::routes())
        .merge(applications::routes())
        .merge(batch::routes())
        .merge(stats::routes())
        .merge(calendar::routes())
        .merge(ai_jobs::routes())
        .merge(skills::routes())
        .route("/metrics", get(metrics))
        .route("/admin/users", get(admin_list_users).post(admin_create_user))
        .route(
            "/admin/users/{id}",
            axum::routing::patch(admin_update_user),
        )
}

// ---------- 管理员：用户管理（ADR-0011 R5，memos 式） ----------

#[derive(serde::Serialize, sqlx::FromRow)]
struct AdminUserRow {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub row_status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[tracing::instrument(skip_all)]
async fn admin_list_users(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
) -> Result<Json<Vec<AdminUserRow>>, AppError> {
    ensure_admin(&state, &user).await?;
    let rows = sqlx::query_as::<_, AdminUserRow>(
        "SELECT id, username, role, row_status, created_at FROM users ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct CreateUserReq {
    pub username: String,
    pub password: String,
    pub role: Option<String>, // 默认 user
}

#[tracing::instrument(skip_all)]
async fn admin_create_user(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Json(req): Json<CreateUserReq>,
) -> Result<impl IntoResponse, AppError> {
    ensure_admin(&state, &user).await?;
    let username = req.username.trim();
    if username.is_empty() {
        return Err(AppError::BadRequest("用户名不能为空".to_string()));
    }
    if req.password.len() < 6 {
        return Err(AppError::BadRequest("密码至少 6 位".to_string()));
    }
    let role = match req.role.as_deref() {
        None | Some("user") => "user",
        Some("admin") => "admin",
        Some(other) => return Err(AppError::BadRequest(format!("非法角色: {other}"))),
    };
    let hash = auth::hash_password(&req.password)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users(username, password_hash, role) VALUES($1,$2,$3) RETURNING id",
    )
    .bind(username)
    .bind(hash)
    .bind(role)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref d) if d.constraint() == Some("users_username_key") => {
            AppError::Conflict("用户名已存在".to_string())
        }
        other => other.into(),
    })?;
    tracing::info!(
        target: "audit",
        event = "audit.user.created",
        admin_id = user.0,
        new_user_id = id,
        new_username = %username,
        role = %role,
        "admin created user"
    );
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

#[derive(Deserialize)]
struct UpdateUserReq {
    pub row_status: Option<String>,   // active / disabled
    pub password: Option<String>,     // 重置密码
    pub role: Option<String>,
}

#[tracing::instrument(skip_all)]
async fn admin_update_user(
    State(state): State<AppState>,
    Extension(user): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateUserReq>,
) -> Result<Json<Value>, AppError> {
    ensure_admin(&state, &user).await?;
    if let Some(rs) = req.row_status.as_deref() {
        if rs != "active" && rs != "disabled" {
            return Err(AppError::BadRequest("row_status 必须是 active/disabled".to_string()));
        }
        let updated = sqlx::query("UPDATE users SET row_status=$2 WHERE id=$1")
            .bind(id)
            .bind(rs)
            .execute(&state.pool)
            .await?;
        if updated.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }
        // 停用即踢下线；恢复登录态由用户重新登录
        if rs == "disabled" {
            state.sessions.invalidate_user(id);
        }
        tracing::info!(
            target: "audit",
            event = "audit.user.status_changed",
            admin_id = user.0,
            target_user_id = id,
            new_status = %rs,
            "admin changed user status"
        );
    }
    if let Some(pw) = req.password.as_deref() {
        if pw.len() < 6 {
            return Err(AppError::BadRequest("密码至少 6 位".to_string()));
        }
        let hash = auth::hash_password(pw)?;
        sqlx::query("UPDATE users SET password_hash=$2 WHERE id=$1")
            .bind(id)
            .bind(hash)
            .execute(&state.pool)
            .await?;
        state.sessions.invalidate_user(id);
        tracing::info!(
            target: "audit",
            event = "audit.user.password_reset",
            admin_id = user.0,
            target_user_id = id,
            "admin reset user password"
        );
    }
    if let Some(role) = req.role.as_deref() {
        if role != "admin" && role != "user" {
            return Err(AppError::BadRequest("role 必须是 admin/user".to_string()));
        }
        if id == user.0 && role != "admin" {
            return Err(AppError::BadRequest("不能降级自己的 admin 角色".to_string()));
        }
        sqlx::query("UPDATE users SET role=$2 WHERE id=$1")
            .bind(id)
            .bind(role)
            .execute(&state.pool)
            .await?;
        tracing::info!(
            target: "audit",
            event = "audit.user.role_changed",
            admin_id = user.0,
            target_user_id = id,
            new_role = %role,
            "admin changed user role"
        );
    }
    Ok(Json(json!({ "ok": true })))
}

/// 管理员校验：查库验证角色（不信任会话缓存）
async fn ensure_admin(state: &AppState, user: &CurrentUser) -> Result<(), AppError> {
    let role: String = sqlx::query_scalar("SELECT role FROM users WHERE id=$1")
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    if role != "admin" {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[tracing::instrument(skip_all)]
async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "beview" }))
}

// ---------- 认证中间件 ----------

#[tracing::instrument(skip_all)]
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    // 白名单（nest 于 /api 下，路径已剥前缀）：存活探针 / 首启建管理员 / 登录 /
    // ICS 订阅源（日历 App 无法走 session cookie，handler 内自行校验 per-user token）
    let whitelisted = path == "/health" || path == "/login" || path == "/setup"
        || path == "/setup/status" || path == "/calendar.ics";
    let resp = if whitelisted {
        next.run(req).await
    } else {
        match auth::read_cookie(req.headers()).and_then(|t| state.sessions.get(&t)) {
            Some(user_id) => {
                let mut req = req;
                req.extensions_mut().insert(CurrentUser(user_id));
                next.run(req).await
            }
            None => {
                tracing::warn!(
                    target: "security",
                    event = "security.auth.unauthorized",
                    http.method = %method,
                    http.route = %path,
                    "unauthorized request"
                );
                AppError::Unauthorized.into_response()
            }
        }
    };
    metrics::m()
        .http_requests
        .with_label_values(&[&method, &resp.status().as_u16().to_string()])
        .inc();
    metrics::m()
        .http_duration
        .with_label_values(&[&method, &path])
        .observe(start.elapsed().as_secs_f64());
    resp
}

#[tracing::instrument(skip_all)]
async fn metrics() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        crate::metrics::render(),
    )
        .into_response()
}

// ---------- 认证 handlers ----------

#[tracing::instrument(skip_all)]
async fn setup_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(json!({ "setup_done": count > 0 })))
}

#[tracing::instrument(skip_all)]
async fn setup(State(state): State<AppState>, Json(req): Json<SetupReq>) -> Result<impl IntoResponse, AppError> {
    let username = req.username.trim();
    if username.is_empty() || req.password.len() < 6 {
        return Err(AppError::BadRequest("用户名不能为空，密码至少 6 位".to_string()));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&state.pool)
        .await?;
    if count > 0 {
        return Err(AppError::Conflict("系统已初始化，不能重复创建管理员".to_string()));
    }
    let hash = auth::hash_password(&req.password)?;
    // 评审 P3 整改：单语句原子判断 + 插入，并发首启不再可能产生第二个管理员
    // （此前 count 检查与 INSERT 非原子）。
    let inserted = sqlx::query(
        "INSERT INTO users(username, password_hash, role) \
         SELECT $1, $2, 'admin' WHERE NOT EXISTS(SELECT 1 FROM users)",
    )
    .bind(username)
    .bind(hash)
    .execute(&state.pool)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(AppError::Conflict("系统已初始化，不能重复创建管理员".to_string()));
    }
    tracing::info!(
        target: "audit",
        event = "audit.system.initialized",
        username = %username,
        "system initialized with admin user"
    );
    Ok((StatusCode::CREATED, Json(json!({ "ok": true }))))
}

#[tracing::instrument(skip_all)]
async fn login(State(state): State<AppState>, Json(req): Json<LoginReq>) -> Result<impl IntoResponse, AppError> {
    let username = req.username.trim().to_string();
    // SEC1 限流：窗口内失败达上限直接拒绝（在查库前拦截，防暴力爆破）
    if auth::login_throttled(&username) {
        tracing::warn!(
            target: "security",
            event = "security.rate_limit.exceeded",
            username = %username,
            "login rate limit exceeded"
        );
        return Err(AppError::TooManyRequests("尝试过于频繁，请稍后再试".to_string()));
    }
    let user: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, password_hash, row_status FROM users WHERE username=$1")
            .bind(&username)
            .fetch_optional(&state.pool)
            .await?;
    let Some((id, hash, row_status)) = user else {
        auth::record_login_fail(&username);
        tracing::warn!(
            target: "security",
            event = "security.auth.failed",
            username = %username,
            reason = "user_not_found",
            "login failed"
        );
        return Err(AppError::Unauthorized);
    };
    // 停用账号拒绝登录（数据保留）
    if row_status != "active" {
        tracing::warn!(
            target: "security",
            event = "security.auth.disabled_user",
            username = %username,
            user_id = id,
            "disabled user login attempt rejected"
        );
        return Err(AppError::Forbidden);
    }
    if !auth::verify_password(&req.password, &hash) {
        auth::record_login_fail(&username);
        tracing::warn!(
            target: "security",
            event = "security.auth.failed",
            username = %username,
            user_id = id,
            reason = "invalid_password",
            "login failed"
        );
        return Err(AppError::Unauthorized);
    }
    auth::clear_login_fails(&username);
    tracing::info!(
        target: "audit",
        event = "audit.user.login",
        username = %username,
        user_id = id,
        "user login succeeded"
    );
    let token = state.sessions.create(id, state.session_ttl());
    let mut resp = (StatusCode::OK, Json(json!({ "ok": true }))).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        auth::cookie_header(&token, state.session_ttl_hours),
    );
    Ok(resp)
}

#[tracing::instrument(skip_all)]
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(token) = auth::read_cookie(&headers) {
        state.sessions.remove(&token);
    }
    tracing::info!(
        target: "audit",
        event = "audit.user.logout",
        "user logged out"
    );
    let mut resp = (StatusCode::OK, Json(json!({ "ok": true }))).into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, auth::clear_cookie_header());
    resp
}

#[tracing::instrument(skip_all)]
async fn me(State(state): State<AppState>, Extension(user): Extension<CurrentUser>) -> Result<Json<UserRow>, AppError> {
    let u = sqlx::query_as::<_, UserRow>("SELECT id, username, role, created_at FROM users WHERE id=$1")
        .bind(user.0)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(u))
}
