//! 集成测试基座（TDD 教义：在 public seam `build_api` 上测真实 HTTP + 真实 Postgres 测试库；
//! 仅把外部 LLM API 在系统边界用本地 mock 替换）。
//! 共享助手被不同测试二进制（v2/drills/paper_resume/export）各自引用，单二进制视角会报
//! 未用死代码（如 v2 不用 mock），故整模块允许 dead_code。

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tower::ServiceExt;

use server::auth::SessionStore;
use server::build_api;
use server::config::Config;
use server::db;
use server::state::AppState;

/// 测试库全局初始化（建库与迁移每进程只做一次）
static DB_INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
/// 测试库全局互斥：多测试共享同一测试库，串行化避免互相清空（tokio 并发测试默认并行）
static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub mod llm_mock;

/// 从 config.toml 的 DSN 派生测试库 URL（库名 + `_test`）
fn test_dsn(dsn: &str) -> String {
    let (head, db) = dsn
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("DSN 缺少库名: {dsn}"));
    format!("{head}/{}_test", db.split('?').next().unwrap())
}

/// 确保测试库存在（连维护库 postgres 建库，已存在则忽略）
async fn ensure_database(dsn: &str) {
    let (url, dbname) = dsn
        .rsplit_once('/')
        .map(|(head, db)| (head.to_string(), db.to_string()))
        .unwrap();
    let dbname = dbname.split('?').next().unwrap().to_string();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("连接维护库失败");
    let res = sqlx::query(&format!("CREATE DATABASE {}_test", dbname))
        .execute(&pool)
        .await;
    match res {
        Ok(_) => tracing::info!("已创建测试库 {}", dbname),
        Err(e) => {
            let s = e.to_string();
            if !s.contains("already exists") {
                panic!("创建测试库失败: {e}");
            }
        }
    }
    pool.close().await;
}

async fn truncate_all(pool: &PgPool) {
    sqlx::query(
        r#"
        TRUNCATE TABLE
          users, companies, positions, sessions, rounds, questions, tags, question_tags,
          analyses, comments, settings, drills, drill_messages, review_records, resumes,
          mall_items, points_ledger, applications, question_answers, question_rounds,
          skills, question_skills
        RESTART IDENTITY CASCADE
        -- interviewer_personas 为内置种子参考数据，不参与清库（M5a）
        "#,
    )
    .execute(pool)
    .await
    .expect("TRUNCATE 失败");
}

pub struct TestApp {
    pub app: Router,
    pub pool: PgPool,
    pub state: AppState,
    pub cookie: Option<String>,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

/// 轮询 GET /api/drills/{did} 直到条件满足（后台批量判卷任务等测试用），超时 panic
pub async fn wait_drill_until(
    app: &TestApp,
    did: i64,
    mut cond: impl FnMut(&Value) -> bool,
    timeout_ms: u64,
) -> Value {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
    loop {
        let (_, det) = app.req(Method::GET, &format!("/api/drills/{did}"), None).await;
        if cond(&det) {
            return det;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("等待 /api/drills/{did} 条件满足超时");
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// 等待 AI 任务终态（ADR-0013）：轮询 GET /api/ai-jobs/{id} 直到 done/failed 或超时。
/// 任务由 mock LLM 快速完成，正常几十毫秒内终态。
pub async fn wait_ai_job(app: &TestApp, job_id: u64, timeout_ms: u64) -> Value {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
    loop {
        let (s, v) = app.req(Method::GET, &format!("/api/ai-jobs/{job_id}"), None).await;
        if s.as_u16() == 200 && (v["status"] == "done" || v["status"] == "failed") {
            return v;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("等待 ai-job {job_id} 终态超时");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// 启动测试应用：真实 axum 路由 + 真实测试库（每次建库/清空/迁移）。
/// 返回前不会自动登录；先 `setup_admin` 建管理员再 `login`。
impl TestApp {
    pub async fn setup() -> TestApp {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let guard = DB_LOCK.lock().await; // 持有到 TestApp 释放
        let cfg = Config::load();
        let tdsn = test_dsn(&cfg.database_url);
        let cfg_db_url = cfg.database_url.clone();
        let tdsn_clone = tdsn.clone();
        DB_INIT
            .get_or_init(|| async move {
                ensure_database(&cfg_db_url).await;
                let init_pool = PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&tdsn_clone)
                    .await
                    .expect("连接测试库失败");
                db::migrate(&init_pool).await;
                init_pool.close().await;
            })
            .await;

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&tdsn)
            .await
            .expect("连接测试库失败");
        truncate_all(&pool).await;
        // M5a：内置人格种子属参考数据，但 TRUNCATE users 的 CASCADE 会连带清掉它——
        // 清库后幂等补种（与生产 db::migrate 同源逻辑）
        server::routes::personas::ensure_builtins(&pool).await;
        let app_state = AppState {
            pool: pool.clone(),
            sessions: SessionStore::new(),
            session_ttl_hours: cfg.session_ttl_hours,
            batch_jobs: server::state::BatchJobs::new(),
            ai_jobs: server::state::AiJobs::new(),
            event_bus: server::events::EventBus::new(pool.clone()),
        };
        // 票09：测试环境同样启动队列 dispatcher（与生产 main.rs 一致），
        // 否则入队任务永远无人认领。启动恢复在 truncate 后无意义，跳过。
        let disp_state = app_state.clone();
        server::services::job_queue::spawn_dispatcher(disp_state);

        let app = build_api(app_state.clone());
        TestApp {
            app,
            pool,
            state: app_state,
            cookie: None,
            _guard: guard,
        }
    }

    pub fn base_url(&self) -> String {
        "http://test".to_string()
    }

    /// 发真实 HTTP 请求（打 build_api 的 /api 路由），返回 (status, body)
    pub async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("{}{}", self.base_url(), path));
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        if let Some(c) = &self.cookie {
            builder = builder.header("cookie", c.clone());
        }
        let req = builder
            .body(Body::from(
                body.map(|b| b.to_string()).unwrap_or_default(),
            ))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    /// 用指定会话 cookie 发请求（多用户测试：第二个用户的会话）
    pub async fn req_as(&self, cookie: &str, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("{}{}", self.base_url(), path))
            .header("cookie", cookie.to_string());
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let req = builder
            .body(Body::from(
                body.map(|b| b.to_string()).unwrap_or_default(),
            ))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, v)
    }

    /// 以指定账号登录并返回会话 cookie（不改变 self.cookie）
    pub async fn login_as(&self, user: &str, pw: &str) -> (StatusCode, Option<String>) {
        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("{}/api/login", self.base_url()))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "username": user, "password": pw }).to_string()))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let cookie = resp
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(String::from);
        (status, cookie)
    }

    /// 发真实 HTTP 请求并返回原始文本 body（SSE 流等非 JSON）
    pub async fn req_raw(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, String) {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("{}{}", self.base_url(), path));
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        if let Some(c) = &self.cookie {
            builder = builder.header("cookie", c.clone());
        }
        let req = builder
            .body(Body::from(
                body.map(|b| b.to_string()).unwrap_or_default(),
            ))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// 建管理员并登录，记住 cookie
    pub async fn setup_admin_and_login(&mut self) {
        self.setup_admin().await;
        self.login("admin", "admin123").await;
    }

    pub async fn setup_admin(&self) -> (StatusCode, Value) {
        let r = self
            .req(
                Method::POST,
                "/api/setup",
                Some(json!({ "username": "admin", "password": "admin123" })),
            )
            .await;
        // 商城默认模板（迁移中的种子会被 TRUNCATE 清掉）：归首管理员
        sqlx::query(
            "INSERT INTO mall_items (user_id, name, cost, emoji, sort_order)
             SELECT (SELECT min(id) FROM users WHERE role='admin'), x.name, x.cost, x.emoji, x.sort_order
             FROM (VALUES ('奶茶',150,'🧋',1), ('加餐',2000,'🍱',2), ('购物',5000,'🛍️',3), ('游戏时间',800,'🎮',4))
                  AS x(name, cost, emoji, sort_order)",
        )
        .execute(&self.pool)
        .await
        .expect("商城种子失败");
        r
    }

    pub async fn login(&mut self, user: &str, pw: &str) -> (StatusCode, Value) {
        let builder = Request::builder()
            .method(Method::POST)
            .uri(format!("{}/api/login", self.base_url()))
            .header("content-type", "application/json");
        let req = builder
            .body(Body::from(json!({ "username": user, "password": pw }).to_string()))
            .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        if let Some(set) = resp.headers().get("set-cookie") {
            let val = set.to_str().unwrap_or("");
            if let Some(sess) = val.split(';').next() {
                self.cookie = Some(sess.to_string());
            }
        }
        (status, Value::Null)
    }

    /// 直接向测试库注入 LLM 设置指向本地 mock（系统边界 mock；settings 已 per-user，归首管理员）
    pub async fn point_llm_at_mock(&self, base_url: &str) {
        sqlx::query(
            r#"
            INSERT INTO settings(user_id, key, value)
            SELECT (SELECT min(id) FROM users WHERE role='admin'), k, to_jsonb(v)
            FROM (VALUES
              ('llm_base_url', $1::text),
              ('llm_model', 'mock'),
              ('llm_api_key', ''),
              ('llm_timeout', '30')
            ) AS seed(k, v)
            ON CONFLICT (user_id, key) DO UPDATE SET value=EXCLUDED.value
            "#,
        )
        .bind(base_url)
            .execute(&self.pool)
            .await
            .expect("注入 LLM 设置失败");
    }
}
// ---------- v4 投递核心单元助手（公司+岗位=投递，轮次挂投递） ----------

/// 建一份投递（公司不存在则自动建）
pub async fn create_application(app: &TestApp, company: &str, position: &str) -> i64 {
    let (s, a) = app
        .req(
            axum::http::Method::POST,
            "/api/applications",
            Some(serde_json::json!({ "company_name": company, "position": position })),
        )
        .await;
    assert_eq!(s, 201, "建投递应成功");
    a["id"].as_i64().unwrap()
}

/// 在投递下建轮次
pub async fn create_round(app: &TestApp, aid: i64, name: &str) -> i64 {
    let (s, r) = app
        .req(
            axum::http::Method::POST,
            &format!("/api/applications/{aid}/rounds"),
            Some(serde_json::json!({ "name": name })),
        )
        .await;
    assert_eq!(s, 201, "建轮次应成功");
    r["id"].as_i64().unwrap()
}

/// 一站式：投递 + 轮次（替代旧 公司→批次→轮次 链）
pub async fn setup_application_round(app: &TestApp) -> (i64, i64) {
    let aid = create_application(app, "测试公司", "后端").await;
    let rid = create_round(app, aid, "一面").await;
    (aid, rid)
}
