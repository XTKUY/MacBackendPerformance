use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 应用配置（TUI 设置界面可修改并持久化）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub interval_ms: u64,
    pub process_interval_ms: u64,
    pub top_n: usize,
    pub gpu: bool,
    pub theme: String,
    pub db_path: PathBuf,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            interval_ms: 2000,
            process_interval_ms: 2000,
            top_n: 15,
            gpu: true,
            theme: "mocha".to_string(),
            db_path: PathBuf::from("neko-perf.db"),
        }
    }
}

pub fn config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/neko-perf/config.toml")
}

pub fn load() -> AppConfig {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => toml::from_str(&s).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

pub fn save(cfg: &AppConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let s = toml::to_string(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, s).map_err(|e| e.to_string())
}
