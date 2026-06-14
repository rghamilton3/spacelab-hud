use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// Application configuration, persisted to `~/.config/spacelab-hud/config.json`
/// and editable live via the web config server.
///
/// `#[serde(default)]` lets older config files (which only carried the GitHub
/// fields) deserialize cleanly — any field absent from disk falls back to the
/// value in [`AppConfig::default`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    // ── GitHub ────────────────────────────────────────────────────────
    pub github_pat:       String,
    pub github_username:  String,
    pub github_repos:     Vec<String>,
    pub github_poll_secs: u64,

    // ── Web config server ─────────────────────────────────────────────
    pub web_config_port:  u16,

    // ── VPS (Beszel monitoring) ───────────────────────────────────────
    /// Base URL of the Beszel instance, e.g. `http://host:8090`.
    pub beszel_url:    String,
    /// System name as registered in Beszel (used to look up the record).
    pub vps_name:      String,
    /// Hostname shown on the VPS panel.
    pub vps_hostname:  String,
    /// IP / address shown on the VPS panel.
    pub vps_ip:        String,

    // ── Network reachability probe ────────────────────────────────────
    /// Host the local-network probe connects to.
    pub probe_host:    String,
    /// TCP port the local-network probe connects to.
    pub probe_port:    u16,

    // ── NAS panel ─────────────────────────────────────────────────────
    pub nas_hostname:  String,
    pub nas_ip:        String,

    // ── Home Assistant panel ──────────────────────────────────────────
    pub ha_hostname:   String,
    pub ha_ip:         String,

    // ── Fan controller (USB serial telemetry) ─────────────────────────
    pub fan_serial_port: String,
    /// Rack temperature (°C) at which a warning alert is raised.
    pub fan_temp_warn_c: f32,
    /// Rack temperature (°C) at which a critical alert is raised.
    pub fan_temp_crit_c: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            github_pat:       String::new(),
            github_username:  String::new(),
            github_repos:     vec![],
            github_poll_secs: 60,

            web_config_port:  80,

            beszel_url:    "http://spacevps.tail718406.ts.net:8090".to_string(),
            vps_name:      "spacevps".to_string(),
            vps_hostname:  "spacevps".to_string(),
            vps_ip:        "spacevps.tail718406.ts.net".to_string(),

            probe_host:    "spacevps.tail718406.ts.net".to_string(),
            probe_port:    22,

            nas_hostname:  "nas-01".to_string(),
            nas_ip:        "192.168.1.20".to_string(),

            ha_hostname:   "homeassistant.local".to_string(),
            ha_ip:         "192.168.1.30".to_string(),

            fan_serial_port: "/dev/ttyACM0".to_string(),
            fan_temp_warn_c: 35.0,
            fan_temp_crit_c: 40.0,
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
