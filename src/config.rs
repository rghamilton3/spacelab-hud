use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub github_pat:      String,
    pub github_username: String,
    pub github_repos:    Vec<String>,
    pub github_poll_secs: u64,
    pub web_config_port:  u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            github_pat:       String::new(),
            github_username:  String::new(),
            github_repos:     vec![],
            github_poll_secs: 60,
            web_config_port:  80,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let dir  = PathBuf::from(home).join(".config/spacelab-hud");
        std::fs::create_dir_all(&dir).ok();
        dir.join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::config_path(), json)?;
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        !self.github_pat.is_empty() && !self.github_repos.is_empty()
    }
}

pub type ConfigRef = Arc<RwLock<AppConfig>>;

pub fn new_config_ref(cfg: AppConfig) -> ConfigRef {
    Arc::new(RwLock::new(cfg))
}
