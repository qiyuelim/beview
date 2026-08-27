use std::path::Path;

use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// 连接 PostgreSQL 并执行迁移。库名/账号来自连接串（config.toml）。
pub async fn connect(database_url: &str) -> PgPool {
    let pool = connect_pool(database_url).await;
    migrate(&pool).await;
    tracing::info!("数据库迁移完成");
    pool
}

/// 仅建连接池（不迁移），测试用
pub async fn connect_pool(database_url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .expect("连接 PostgreSQL 失败，请检查 config.toml 与网络")
}

/// 执行迁移（幂等）
pub async fn migrate(pool: &PgPool) {
    let migrator = Migrator::new(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations")))
        .await
        .expect("加载迁移文件失败");
    migrator.run(pool).await.expect("数据库迁移失败");
    // M5a：内置面试官人格幂等补种（种子属参考数据，不随业务清库消失）
    crate::routes::personas::ensure_builtins(pool).await;
    // 后台批量判卷任务重启即清：残留 'grading' 视为失败（答案已落库，前端给「继续判卷」）
    sqlx::query("UPDATE drills SET grading=NULL WHERE grading='grading'")
        .execute(pool)
        .await
        .ok();
}
