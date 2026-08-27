use std::path::PathBuf;

use serde::Deserialize;

/// 运行时配置。配置文件 `server/config.toml`（gitignored），
/// 可用环境变量 `BEVIEW_CONFIG` 指定路径（运维配置，允许 env）。
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Web 服务监听地址（局域网可访问：0.0.0.0:8765）
    pub bind_addr: String,
    /// PostgreSQL 连接串（局域网）
    pub database_url: String,
    /// 会话有效期（小时），默认 168 = 7 天
    #[serde(default = "default_ttl")]
    pub session_ttl_hours: u64,
    /// OTLP 导出端点（可选；配置了才导出 span，否则 stdout）
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
}

fn default_ttl() -> u64 {
    168
}

impl Config {
    pub fn load() -> Config {
        let path = std::env::var("BEVIEW_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.toml"));
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("无法读取配置文件 {}: {e}", path.display()));
        let cfg: Config = toml::from_str(&raw)
            .unwrap_or_else(|e| panic!("配置文件解析失败 {}: {e}", path.display()));
        tracing::info!(config = %path.display(), bind = %cfg.bind_addr, "config loaded");
        cfg
    }
}
