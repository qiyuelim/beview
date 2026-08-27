// axe 无障碍扫描辅助：创建/清除临时扫描账号（不触碰真实管理员凭据）。
// 用法:
//   cargo run --example axe_scan_user -- up '临时密码'   # 创建/重置 axe_scan 管理员
//   cargo run --example axe_scan_user -- down            # 删除 axe_scan
use server::auth;
use server::config::Config;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let action = std::env::args().nth(1).unwrap_or_default();
    let cfg = Config::load();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("连接数据库失败");
    match action.as_str() {
        "up" => {
            let pw = std::env::args().nth(2).expect("用法: axe_scan_user up '临时密码'");
            assert!(pw.len() >= 6, "密码至少 6 位");
            let hash = auth::hash_password(&pw).expect("argon2 哈希失败");
            sqlx::query(
                "INSERT INTO users(username, password_hash, role, row_status) \
                 VALUES('axe_scan', $1, 'admin', 'active') \
                 ON CONFLICT (username) DO UPDATE SET password_hash=EXCLUDED.password_hash, row_status='active'",
            )
            .bind(&hash)
            .execute(&pool)
            .await
            .expect("创建扫描账号失败");
            println!("axe_scan 就绪（临时账号，扫描后请执行 down 删除）");
        }
        "down" => {
            // 先清登录/懒生成产生的从属行，再删用户
            let uid: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username='axe_scan'")
                .fetch_optional(&pool)
                .await
                .expect("查询失败");
            if let Some(uid) = uid {
                // 临时账号专用：按外键安全顺序清空其全部业务数据（正常用户数据不受影响）
                for sql in [
                    "DELETE FROM application_events WHERE application_id IN (SELECT id FROM applications WHERE user_id=$1)",
                    "DELETE FROM rounds WHERE application_id IN (SELECT id FROM applications WHERE user_id=$1)",
                    "DELETE FROM applications WHERE user_id=$1",
                    "DELETE FROM drill_messages WHERE drill_id IN (SELECT id FROM drills WHERE user_id=$1)",
                    "UPDATE drills SET application_id=NULL WHERE user_id=$1",
                    "DELETE FROM drills WHERE user_id=$1",
                    "DELETE FROM review_records WHERE question_id IN (SELECT id FROM questions WHERE user_id=$1)",
                    "DELETE FROM question_answers WHERE question_id IN (SELECT id FROM questions WHERE user_id=$1)",
                    "DELETE FROM question_rounds WHERE question_id IN (SELECT id FROM questions WHERE user_id=$1)",
                    "DELETE FROM analyses WHERE question_id IN (SELECT id FROM questions WHERE user_id=$1)",
                    "DELETE FROM comments WHERE question_id IN (SELECT id FROM questions WHERE user_id=$1)",
                    "DELETE FROM question_tags WHERE question_id IN (SELECT id FROM questions WHERE user_id=$1)",
                    "DELETE FROM questions WHERE user_id=$1",
                    "DELETE FROM tags WHERE user_id=$1",
                    "DELETE FROM positions WHERE company_id IN (SELECT id FROM companies WHERE user_id=$1)",
                    "DELETE FROM companies WHERE user_id=$1",
                    "DELETE FROM points_ledger WHERE user_id=$1",
                    "DELETE FROM settings WHERE user_id=$1",
                ] {
                    sqlx::query(sql).bind(uid).execute(&pool).await.expect("清理失败");
                }
            }
            let affected = sqlx::query("DELETE FROM users WHERE username='axe_scan'")
                .execute(&pool)
                .await
                .expect("删除失败")
                .rows_affected();
            println!("axe_scan 已删除（{affected} 行）");
        }
        _ => panic!("用法: axe_scan_user up '密码' | down"),
    }
}
