// 临时维护工具：重置管理员密码（复用服务端 argon2 哈希，与登录同源）。
// 用法: cargo run --example reset_password -- '新密码'
use server::auth;
use server::config::Config;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let pw = std::env::args().nth(1).expect("用法: reset_password '新密码'");
    if pw.len() < 6 {
        panic!("密码至少 6 位");
    }
    let cfg = Config::load();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("连接数据库失败");
    let hash = auth::hash_password(&pw).expect("argon2 哈希失败");
    let affected = sqlx::query("UPDATE users SET password_hash=$1 WHERE role='admin'")
        .bind(&hash)
        .execute(&pool)
        .await
        .expect("更新失败")
        .rows_affected();
    println!("已重置管理员密码，影响 {affected} 个用户");
}
