use std::time::Duration;

use dashmap::DashMap;
use rand::RngCore;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

pub const COOKIE_NAME: &str = "beview_session";

/// 登录失败限流（SEC1，v4 评审整改）：per-username 滑动窗口，
/// `LOGIN_WINDOW` 内失败达 `LOGIN_MAX_FAILS` 次即拒绝；成功登录清零。
/// 进程内存态（与 SessionStore 同级）：重启即清，局域网自用足够，不引入外部依赖。
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const LOGIN_MAX_FAILS: usize = 5;
static LOGIN_FAILS: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 该用户名当前是否被限流（达到窗口内失败上限）
pub fn login_throttled(username: &str) -> bool {
    let mut map = LOGIN_FAILS.lock().expect("login fails mutex poisoned");
    let now = Instant::now();
    if let Some(stamps) = map.get_mut(username) {
        stamps.retain(|t| now.duration_since(*t) < LOGIN_WINDOW);
        stamps.len() >= LOGIN_MAX_FAILS
    } else {
        false
    }
}

/// 记录一次登录失败
pub fn record_login_fail(username: &str) {
    let mut map = LOGIN_FAILS.lock().expect("login fails mutex poisoned");
    let now = Instant::now();
    let stamps = map.entry(username.to_string()).or_default();
    stamps.retain(|t| now.duration_since(*t) < LOGIN_WINDOW);
    stamps.push(now);
}

/// 登录成功后清零该用户名的失败计数
pub fn clear_login_fails(username: &str) {
    LOGIN_FAILS
        .lock()
        .expect("login fails mutex poisoned")
        .remove(username);
}

/// 服务端会话表（内存）。单用户工具足够；重启后需重新登录。
#[derive(Clone)]
pub struct SessionStore(Arc<DashMap<String, Session>>);

#[derive(Clone)]
pub struct Session {
    pub user_id: i64,
    pub expires: std::time::SystemTime,
}

impl SessionStore {
    pub fn new() -> Self {
        Self(Arc::new(DashMap::new()))
    }

    pub fn create(&self, user_id: i64, ttl: Duration) -> String {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        self.0.insert(
            token.clone(),
            Session {
                user_id,
                expires: std::time::SystemTime::now() + ttl,
            },
        );
        token
    }

    pub fn get(&self, token: &str) -> Option<i64> {
        let s = self.0.get(token)?;
        if s.expires > std::time::SystemTime::now() {
            Some(s.user_id)
        } else {
            None
        }
    }

    pub fn remove(&self, token: &str) {
        self.0.remove(token);
    }

    /// 使某用户所有会话失效（改密码后调用）
    pub fn invalidate_user(&self, user_id: i64) {
        self.0.retain(|_, s| s.user_id != user_id);
    }
}

/// 当前登录用户（中间件写入 request extensions）
#[derive(Clone, Copy)]
pub struct CurrentUser(pub i64);

/// argon2 密码哈希 / 校验（个人工具，argon2 为事实标准）
pub fn hash_password(pw: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    Ok(argon2::Argon2::default()
        .hash_password(pw.as_bytes(), &salt)?
        .to_string())
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    use argon2::password_hash::PasswordVerifier;
    argon2::PasswordHash::new(hash)
        .map(|h| argon2::Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

pub fn cookie_header(token: &str, ttl_hours: u64) -> axum::http::HeaderValue {
    let max_age = ttl_hours * 3600;
    axum::http::HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}"
    ))
    .expect("cookie header")
}

pub fn clear_cookie_header() -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_str(&format!("{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"))
        .expect("cookie header")
}

pub fn read_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|kv| {
        let mut it = kv.trim().splitn(2, '=');
        let k = it.next()?;
        if k == COOKIE_NAME {
            it.next().map(|v| v.trim().to_string())
        } else {
            None
        }
    })
}
