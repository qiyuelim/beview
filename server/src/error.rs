use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    Unauthorized,
    Forbidden,
    NotFound,
    BadRequest(String),
    Conflict(String),
    Internal(String),
    /// 请求过于频繁（登录限流 SEC1）
    TooManyRequests(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "未登录或会话过期".to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "无权限".to_string()),
            AppError::NotFound => (StatusCode::NOT_FOUND, "资源不存在".to_string()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m),
            AppError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            AppError::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => AppError::NotFound,
            other => {
                tracing::error!(err = %other, "db error");
                AppError::Internal("数据库错误".to_string())
            }
        }
    }
}

impl From<argon2::password_hash::Error> for AppError {
    fn from(e: argon2::password_hash::Error) -> Self {
        AppError::Internal(format!("密码处理失败: {e}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::BadRequest(format!("JSON 解析失败: {e}"))
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Internal(format!("上游请求失败: {e}"))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            AppError::Unauthorized => "未登录或会话过期",
            AppError::Forbidden => "无权限",
            AppError::NotFound => "资源不存在",
            AppError::BadRequest(m) => return write!(f, "{m}"),
            AppError::Conflict(m) => return write!(f, "{m}"),
            AppError::Internal(m) => return write!(f, "{m}"),
            AppError::TooManyRequests(m) => return write!(f, "{m}"),
        };
        write!(f, "{msg}")
    }
}
