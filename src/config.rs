use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// A single GitHub credential and the repos it watches.
///
/// One source maps to one resource owner: a fine-grained PAT is scoped to a
/// single org (or your personal account), so watching repos across multiple
/// orgs requires one source per org. Each source carries its own `username`
/// (the login the PAT authenticates as) so per-repo "is this my activity?"
/// filtering stays correct even when sources belong to different accounts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GithubSource {
    pub pat:      String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub repos:    Vec<String>,
}

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
    /// One credential per resource owner (org / account). See [`GithubSource`].
    pub github_sources:   Vec<GithubSource>,
    pub github_poll_secs: u64,

    // ── Legacy single-credential GitHub fields ────────────────────────
    // Retained only so pre-multi-source `config.json` files deserialize and
    // fold into `github_sources` on load (see [`AppConfig::migrate`]). They are
    // never written back out, so a saved config drops them permanently.
    #[serde(default, rename = "github_pat", skip_serializing)]
    legacy_github_pat:      String,
    #[serde(default, rename = "github_username", skip_serializing)]
    legacy_github_username: String,
    #[serde(default, rename = "github_repos", skip_serializing)]
    legacy_github_repos:    Vec<String>,

    // ── Web config server ─────────────────────────────────────────────
    pub web_config_port:  u16,

    // ── VPS (Beszel monitoring) ───────────────────────────────────────
    /// Base URL of the Beszel instance, e.g. `http://host:8090`.
    pub beszel_url:    String,
    /// Beszel superuser email used to obtain an API auth token. Shared by all
    /// monitored systems (they live in one Beszel instance).
    pub beszel_email:    String,
    /// Beszel superuser password. Stored alongside the other LAN-trusted
    /// secrets in `config.json` (same model as the GitHub PATs).
    pub beszel_password: String,
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
    /// System name as registered in Beszel (used to look up the record).
    pub nas_name:      String,
    pub nas_hostname:  String,
    pub nas_ip:        String,

    // ── Home Assistant panel ──────────────────────────────────────────
    /// System name as registered in Beszel (used to look up the record).
    pub ha_name:       String,
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
            github_sources:   vec![],
            github_poll_secs: 60,

            legacy_github_pat:      String::new(),
            legacy_github_username: String::new(),
            legacy_github_repos:    vec![],

            web_config_port:  80,

            // Infrastructure endpoints are intentionally blank by default —
            // populate them via the web config UI. Keeping them empty avoids
            // baking one deployment's hostnames/IPs into the shared source.
            beszel_url:      String::new(),
            beszel_email:    String::new(),
            beszel_password: String::new(),
            vps_name:      String::new(),
            vps_hostname:  String::new(),
            vps_ip:        String::new(),

            probe_host:    String::new(),
            probe_port:    22,

            nas_name:      String::new(),
            nas_hostname:  String::new(),
            nas_ip:        String::new(),

            ha_name:       String::new(),
            ha_hostname:   String::new(),
            ha_ip:         String::new(),

            fan_serial_port: "/dev/ttyACM0".to_string(),
            fan_temp_warn_c: 35.0,
            fan_temp_crit_c: 40.0,
        }
    }
}

impl AppConfig {
    /// Path to `config.json`. A pure computation — no I/O — so callers (and
    /// tests) can rely on it not creating directories as a side effect. The
    /// containing dir is created in [`save`](Self::save), the only writer.
    pub fn config_path() -> PathBuf {
        config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        let mut cfg: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        cfg.migrate();
        cfg.decrypt_secrets();
        cfg
    }

    /// Secret fields, encrypted at rest. The in-memory `AppConfig` always holds
    /// them in plaintext; encryption happens only at the [`save`](Self::save)
    /// boundary and decryption only here on [`load`](Self::load).
    ///
    /// On load, an `enc:` token is decrypted; a plaintext value (a hand-edited
    /// config, or one written before encryption existed) is left as-is and gets
    /// sealed on the next save. A token that fails to decrypt (wrong/missing
    /// key) is cleared rather than left as garbage the app would try to use.
    fn decrypt_secrets(&mut self) {
        decrypt_field(&mut self.beszel_password);
        for src in &mut self.github_sources {
            decrypt_field(&mut src.pat);
        }
    }

    /// Inverse of [`decrypt_secrets`](Self::decrypt_secrets); see its docs for
    /// the field list and the plaintext-migration behavior.
    fn encrypt_secrets(&mut self) -> anyhow::Result<()> {
        encrypt_field(&mut self.beszel_password)?;
        for src in &mut self.github_sources {
            encrypt_field(&mut src.pat)?;
        }
        Ok(())
    }

    /// Folds a legacy single-credential config (pre-multi-source) into the
    /// first `github_sources` entry. A no-op once `github_sources` is populated
    /// or no legacy PAT is present, so it is safe to call on every load.
    fn migrate(&mut self) {
        if self.github_sources.is_empty() && !self.legacy_github_pat.is_empty() {
            self.github_sources.push(GithubSource {
                pat:      std::mem::take(&mut self.legacy_github_pat),
                username: std::mem::take(&mut self.legacy_github_username),
                repos:    std::mem::take(&mut self.legacy_github_repos),
            });
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        // Encrypt secrets in a clone so the live config stays plaintext for the
        // running threads (and the web UI, which re-renders the saved values).
        let mut to_store = self.clone();
        to_store.encrypt_secrets()?;
        let json = serde_json::to_string_pretty(&to_store)?;
        let path = Self::config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, json)?;
        restrict_config_perms(&path);
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        self.github_sources
            .iter()
            .any(|s| !s.pat.is_empty() && !s.repos.is_empty())
    }
}

/// Test-only override for the config dir, so the disk round-trip test can use
/// a temp dir without mutating process-wide `HOME` (unsound across the parallel
/// test harness). Paired with [`crate::secrets::TEST_STATE_DIR`].
#[cfg(test)]
pub(crate) static TEST_CONFIG_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Directory holding `config.json`. Pure (no I/O); see [`AppConfig::config_path`].
fn config_dir() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(dir) = TEST_CONFIG_DIR.get() {
            return dir.clone();
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/spacelab-hud")
}

/// Decrypts a secret field in place, leaving plaintext untouched (migration)
/// and clearing a value that can't be decrypted.
fn decrypt_field(field: &mut String) {
    if crate::secrets::is_encrypted(field) {
        match crate::secrets::decrypt(field) {
            Ok(plain) => *field = plain,
            Err(e) => {
                eprintln!("config: failed to decrypt a secret field, clearing it: {e}");
                field.clear();
            }
        }
    }
}

/// Encrypts a non-empty plaintext field in place; no-op on empty or
/// already-encrypted values.
fn encrypt_field(field: &mut String) -> anyhow::Result<()> {
    if !field.is_empty() && !crate::secrets::is_encrypted(field) {
        *field = crate::secrets::encrypt(field)?;
    }
    Ok(())
}

/// Tightens `config.json` to owner-only (`0600`). It holds ciphertext, but the
/// key lives elsewhere, so this is defense in depth against casual reads.
#[cfg(unix)]
fn restrict_config_perms(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("config: failed to chmod 600 {}: {e}", path.display());
    }
}

#[cfg(not(unix))]
fn restrict_config_perms(_path: &std::path::Path) {}

pub type ConfigRef = Arc<RwLock<AppConfig>>;

pub fn new_config_ref(cfg: AppConfig) -> ConfigRef {
    Arc::new(RwLock::new(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_str(json: &str) -> AppConfig {
        let mut cfg: AppConfig = serde_json::from_str(json).unwrap();
        cfg.migrate();
        cfg
    }

    #[test]
    fn legacy_config_migrates_into_one_source() {
        let cfg = load_str(
            r#"{ "github_pat": "ghp_x", "github_username": "alice",
                 "github_repos": ["alice/a", "alice/b"], "github_poll_secs": 90 }"#,
        );
        assert_eq!(cfg.github_sources.len(), 1);
        let src = &cfg.github_sources[0];
        assert_eq!(src.pat, "ghp_x");
        assert_eq!(src.username, "alice");
        assert_eq!(src.repos, vec!["alice/a", "alice/b"]);
        assert_eq!(cfg.github_poll_secs, 90);
        assert!(cfg.is_configured());
    }

    #[test]
    fn multi_source_config_skips_migration() {
        let cfg = load_str(
            r#"{ "github_sources": [
                   { "pat": "ghp_1", "username": "alice", "repos": ["org1/a"] },
                   { "pat": "ghp_2", "username": "alice", "repos": ["org2/b"] }
                 ] }"#,
        );
        assert_eq!(cfg.github_sources.len(), 2);
        assert_eq!(cfg.github_sources[1].repos, vec!["org2/b"]);
    }

    #[test]
    fn saved_config_drops_legacy_fields() {
        // A migrated config, once serialized, must not re-emit the legacy keys —
        // otherwise the next load would resurrect them.
        let cfg = load_str(r#"{ "github_pat": "ghp_x", "github_repos": ["alice/a"] }"#);
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("github_pat"));
        assert!(!json.contains("github_repos\""));
        assert!(json.contains("github_sources"));
    }

    #[test]
    fn empty_legacy_pat_yields_no_source() {
        let cfg = load_str(r#"{ "github_repos": ["alice/a"] }"#);
        assert!(cfg.github_sources.is_empty());
        assert!(!cfg.is_configured());
    }

    /// Full boundary round-trip: secrets must land on disk as ciphertext yet
    /// come back as plaintext through `load`. Points the config dir and key
    /// file at a unique temp dir via the test-only `OnceLock` overrides, so the
    /// real `config_path` / key file are exercised without mutating
    /// process-wide env vars. This is the only test that initializes the global
    /// cipher, so the temp-dir key it installs can't bleed into other tests.
    #[test]
    fn secrets_round_trip_through_disk() {
        let tmp = std::env::temp_dir().join(format!("slhud-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        TEST_CONFIG_DIR.set(tmp.join("config")).expect("config dir set once");
        crate::secrets::TEST_STATE_DIR
            .set(tmp.join("state"))
            .expect("state dir set once");

        let cfg = AppConfig {
            beszel_password: "hunter2".to_string(),
            github_sources: vec![GithubSource {
                pat: "ghp_topsecret".to_string(),
                username: "alice".to_string(),
                repos: vec!["alice/a".to_string()],
            }],
            ..Default::default()
        };
        cfg.save().unwrap();

        // On disk: ciphertext markers present, no plaintext secrets leaked.
        let raw = std::fs::read_to_string(AppConfig::config_path()).unwrap();
        assert!(raw.contains("enc:v1:"));
        assert!(!raw.contains("hunter2"));
        assert!(!raw.contains("ghp_topsecret"));
        // Non-secret fields stay readable.
        assert!(raw.contains("alice/a"));

        // Reload: secrets decrypt back to plaintext.
        let loaded = AppConfig::load();
        assert_eq!(loaded.beszel_password, "hunter2");
        assert_eq!(loaded.github_sources[0].pat, "ghp_topsecret");
        assert_eq!(loaded.github_sources[0].username, "alice");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
