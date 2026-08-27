// 临时探测工具：列出 PostgreSQL 的数据库，确认认证与目标库是否存在。
// 用法: DATABASE_URL=postgres://USER:PASSWORD@127.0.0.1:5432/postgres cargo run --example probe
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[tokio::main]
async fn main() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let opts: PgConnectOptions = url.parse().expect("parse url");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("connect");
    let rows: Vec<String> = sqlx::query_scalar("SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname")
        .fetch_all(&pool)
        .await
        .expect("list dbs");
    println!("数据库列表: {:?}", rows);
    let ver: String = sqlx::query_scalar("SELECT version()").fetch_one(&pool).await.unwrap();
    println!("版本: {ver}");

    // 确保 beview 库存在（幂等）
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = 'beview')")
        .fetch_one(&pool).await.unwrap();
    if !exists {
        sqlx::query("CREATE DATABASE beview").execute(&pool).await.expect("create db");
        println!("已创建数据库 beview");
    } else {
        println!("数据库 beview 已存在");
    }
}
