use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Cached Beszel auth token, keyed on the URL it was issued for. Keying on the
/// URL means that when an operator points the config at a different Beszel
/// instance, the stale token is discarded up front rather than tried, 401'd,
/// and refreshed — which would otherwise blank all three panels for one cycle.
static AUTH_TOKEN: Mutex<Option<(String, String)>> = Mutex::new(None);

// ── Public types ──────────────────────────────────────────────────────

/// Runtime-configurable Beszel connection settings for a single monitored
/// system (VPS, NAS, or Home Assistant), snapshotted from
/// [`crate::config::AppConfig`] each polling cycle. All targets share one
/// Beszel instance (`beszel_url`) and differ only by `system_name`.
#[derive(Clone)]
pub struct BeszelTarget {
    /// Base URL of the Beszel instance, e.g. `http://host:8090`.
    pub beszel_url:  String,
    /// Beszel user email used to obtain an API auth token. A read-only user
    /// with the monitored systems shared to it is sufficient.
    pub email:       String,
    /// Beszel user password.
    pub password:    String,
    /// System name as registered in Beszel. Also shown on the panel as its
    /// identity — Beszel already knows the host, so there's no separately
    /// configured hostname/IP to display.
    pub system_name: String,
}

impl BeszelTarget {
    pub fn vps(cfg: &crate::config::AppConfig) -> Self {
        Self {
            beszel_url:  cfg.beszel_url.clone(),
            email:       cfg.beszel_email.clone(),
            password:    cfg.beszel_password.clone(),
            system_name: cfg.vps_name.clone(),
        }
    }

    pub fn nas(cfg: &crate::config::AppConfig) -> Self {
        Self {
            beszel_url:  cfg.beszel_url.clone(),
            email:       cfg.beszel_email.clone(),
            password:    cfg.beszel_password.clone(),
            system_name: cfg.nas_name.clone(),
        }
    }

    pub fn ha(cfg: &crate::config::AppConfig) -> Self {
        Self {
            beszel_url:  cfg.beszel_url.clone(),
            email:       cfg.beszel_email.clone(),
            password:    cfg.beszel_password.clone(),
            system_name: cfg.ha_name.clone(),
        }
    }
}

pub struct RemoteAlert {
    pub name:     String,
    pub severity: String,
    pub age:      String,
    pub summary:  String,
}

// ── ServiceMetrics constructors ───────────────────────────────────────

pub fn offline_metrics(system: &str) -> crate::ServiceMetrics {
    crate::ServiceMetrics {
        hostname:  system.into(),
        reachable: false,
        uptime:    "—".into(),
        load_avg:  "—  —  —".into(),
        cpu_usage: "—".into(),
        cpu_pct:   0.0,
        cpu_temp:  "—".into(),
        temp_pct:  0.0,
        ram:       "—".into(),
        ram_pct:   0.0,
        disk:      "—".into(),
        disk_pct:  0.0,
    }
}

fn online_no_metrics(system: &str) -> crate::ServiceMetrics {
    crate::ServiceMetrics {
        reachable: true,
        hostname:  system.into(),
        uptime:    "—".into(),
        load_avg:  "—  —  —".into(),
        cpu_usage: "—".into(),
        cpu_pct:   0.0,
        cpu_temp:  "—".into(),
        temp_pct:  0.0,
        ram:       "—".into(),
        ram_pct:   0.0,
        disk:      "—".into(),
        disk_pct:  0.0,
    }
}

// ── Auth token cache ──────────────────────────────────────────────────

fn invalidate_token() {
    if let Ok(mut guard) = AUTH_TOKEN.lock() {
        *guard = None;
    }
}

fn get_or_refresh_token(beszel_url: &str, email: &str, password: &str) -> Option<String> {
    {
        let guard = AUTH_TOKEN.lock().ok()?;
        if let Some((url, token)) = guard.as_ref() {
            if url == beszel_url {
                return Some(token.clone());
            }
        }
    }

    if email.is_empty() || password.is_empty() {
        return None;
    }

    #[derive(Deserialize)]
    struct AuthResp { token: String }

    // Authenticate as a regular Beszel user (the `users` collection), not a
    // superuser — read-only status polling only needs a user the systems have
    // been shared with, so there's no reason to hand the HUD admin rights.
    let resp = ureq::post(&format!(
        "{}/api/collections/users/auth-with-password",
        beszel_url
    ))
    .timeout(TIMEOUT)
    .send_json(serde_json::json!({ "identity": email, "password": password }))
    .ok()?;

    let body: AuthResp = resp.into_json().ok()?;

    let mut guard = AUTH_TOKEN.lock().ok()?;
    *guard = Some((beszel_url.to_string(), body.token.clone()));
    Some(body.token)
}

// ── Beszel API ────────────────────────────────────────────────────────

/// Percent-encodes a value for safe interpolation into a query string,
/// escaping everything outside the RFC 3986 unreserved set. Keeps a Beszel
/// system name containing `&`, `%`, or `"` from corrupting the `filter=`
/// expression. Kept inline to avoid pulling in a urlencoding crate.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn beszel_alive(beszel_url: &str) -> bool {
    matches!(
        ureq::get(&format!("{}/api/health", beszel_url))
            .timeout(TIMEOUT)
            .call(),
        Ok(r) if r.status() == 200
    )
}

struct BeszelMetrics {
    uptime_secs:   u64,
    load:          Vec<f64>,
    cpu_pct:       f64,
    ram_used_gb:   f64,
    ram_total_gb:  f64,
    ram_pct:       f64,
    disk_used_gb:  f64,
    disk_total_gb: f64,
    disk_pct:      f64,
}

fn fetch_beszel_metrics(
    beszel_url:  &str,
    system_name: &str,
    email:       &str,
    password:    &str,
) -> Option<BeszelMetrics> {
    let token = get_or_refresh_token(beszel_url, email, password)?;

    // ── Step 1: resolve system ID and uptime ──────────────────────────
    #[derive(Deserialize)]
    struct SystemRecord {
        id:   String,
        info: SystemInfo,
    }
    #[derive(Deserialize)]
    struct SystemInfo {
        u:  Option<f64>,
        la: Option<Vec<f64>>,
    }
    #[derive(Deserialize)]
    struct SystemsList { items: Vec<SystemRecord> }

    let resp = ureq::get(&format!(
        "{}/api/collections/systems/records?filter=name%3D%22{}%22&perPage=1",
        beszel_url,
        percent_encode(system_name)
    ))
    .set("Authorization", &token)
    .timeout(TIMEOUT)
    .call();

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => { invalidate_token(); return None; }
        Err(e) => { eprintln!("beszel systems fetch error: {e}"); return None; }
    };

    let systems: SystemsList = resp.into_json().ok()?;
    let system = systems.items.into_iter().next()?;

    let system_id   = system.id;
    let uptime_secs = system.info.u.unwrap_or(0.0) as u64;
    let load        = system.info.la.unwrap_or_default();

    // ── Step 2: fetch latest 1-minute stats ───────────────────────────
    #[derive(Deserialize)]
    struct StatsRecord { stats: Stats }
    #[derive(Deserialize)]
    struct Stats {
        cpu: f64,
        m:   f64,
        mu:  f64,
        mp:  f64,
        d:   f64,
        du:  f64,
        dp:  f64,
    }
    #[derive(Deserialize)]
    struct StatsList { items: Vec<StatsRecord> }

    let resp2 = ureq::get(&format!(
        "{}/api/collections/system_stats/records?sort=-created&perPage=1&filter=system%3D%22{}%22%26%26type%3D%221m%22",
        beszel_url,
        percent_encode(&system_id)
    ))
    .set("Authorization", &token)
    .timeout(TIMEOUT)
    .call();

    let resp2 = match resp2 {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => { invalidate_token(); return None; }
        Err(e) => { eprintln!("beszel stats fetch error: {e}"); return None; }
    };

    let stats_list: StatsList = resp2.into_json().ok()?;
    let s = stats_list.items.into_iter().next()?.stats;

    Some(BeszelMetrics {
        uptime_secs,
        load,
        cpu_pct:       s.cpu,
        ram_used_gb:   s.mu,
        ram_total_gb:  s.m,
        ram_pct:       s.mp,
        disk_used_gb:  s.du,
        disk_total_gb: s.d,
        disk_pct:      s.dp,
    })
}

// ── Beszel system fetch ───────────────────────────────────────────────

impl BeszelTarget {
    /// Fetch live metrics for this Beszel system, given the hub's liveness.
    ///
    /// An unconfigured target (blank Beszel URL, system name, or admin
    /// credentials) short-circuits to an offline panel without touching the
    /// network, so panels the user hasn't set up stay clean rather than
    /// spamming failed requests. All three targets share one Beszel instance,
    /// so the caller probes [`beszel_alive`] once per cycle and passes the
    /// result to all three, avoiding two redundant round-trips.
    pub fn fetch_assuming_alive(&self, hub_alive: bool) -> crate::ServiceMetrics {
        if self.beszel_url.is_empty()
            || self.system_name.is_empty()
            || self.email.is_empty()
            || self.password.is_empty()
        {
            return offline_metrics(&self.system_name);
        }

        if !hub_alive {
            return offline_metrics(&self.system_name);
        }

        let m = match fetch_beszel_metrics(
            &self.beszel_url,
            &self.system_name,
            &self.email,
            &self.password,
        ) {
            Some(m) => m,
            None    => return online_no_metrics(&self.system_name),
        };

        let load1  = m.load.first()  .copied().unwrap_or(0.0);
        let load5  = m.load.get(1)   .copied().unwrap_or(0.0);
        let load15 = m.load.get(2)   .copied().unwrap_or(0.0);

        crate::ServiceMetrics {
            hostname:  self.system_name.as_str().into(),
            reachable: true,
            uptime:    fmt_uptime(m.uptime_secs).into(),
            load_avg:  format!("{:.2}  {:.2}  {:.2}", load1, load5, load15).into(),
            cpu_usage: format!("{:.1}%", m.cpu_pct).into(),
            cpu_pct:   (m.cpu_pct / 100.0).clamp(0.0, 1.0) as f32,
            cpu_temp:  "—".into(),
            temp_pct:  0.0,
            ram:       format!("{:.1} / {:.1} GB", m.ram_used_gb, m.ram_total_gb).into(),
            ram_pct:   (m.ram_pct / 100.0).clamp(0.0, 1.0) as f32,
            disk:      format!("{:.1} / {:.1} GB", m.disk_used_gb, m.disk_total_gb).into(),
            disk_pct:  (m.disk_pct / 100.0).clamp(0.0, 1.0) as f32,
        }
    }
}

// ── RemoteAlert (Alertmanager removed — stub returns no alerts) ───────

impl RemoteAlert {
    pub fn fetch_all() -> Option<Vec<Self>> {
        Some(vec![])
    }
}

// ── Formatting helpers ────────────────────────────────────────────────

fn fmt_uptime(secs: u64) -> String {
    let days  = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins  = (secs % 3600) / 60;
    if days > 0 { format!("{}d {}h {:02}m", days, hours, mins) }
    else        { format!("{}h {:02}m", hours, mins) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_escapes_query_breaking_chars() {
        // Unreserved chars pass through untouched.
        assert_eq!(percent_encode("nas-01.home_lab~v2"), "nas-01.home_lab~v2");
        // Characters that would corrupt the `filter=` expression are escaped.
        assert_eq!(percent_encode(r#"a&b="c"%"#), "a%26b%3D%22c%22%25");
        assert_eq!(percent_encode("a b"), "a%20b");
    }
}
