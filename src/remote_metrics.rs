use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

const BESZEL_HOST:     &str     = "http://spacevps.tail718406.ts.net:8090";
const TIMEOUT:         Duration = Duration::from_secs(5);
const VPS_SYSTEM_NAME: &str     = "spacevps";
const VPS_HOSTNAME:    &str     = "spacevps";
const VPS_IP_ADDR:     &str     = "spacevps.tail718406.ts.net";

static AUTH_TOKEN: Mutex<Option<String>> = Mutex::new(None);

// ── Public types ──────────────────────────────────────────────────────

pub struct RemoteAlert {
    pub name:     String,
    pub severity: String,
    pub age:      String,
    pub summary:  String,
}

// ── ServiceMetrics constructors ───────────────────────────────────────

pub fn offline_metrics(hostname: &str, ip_addr: &str) -> crate::ServiceMetrics {
    crate::ServiceMetrics {
        hostname:  hostname.into(),
        ip_addr:   ip_addr.into(),
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

fn online_no_metrics(hostname: &str, ip_addr: &str) -> crate::ServiceMetrics {
    crate::ServiceMetrics {
        reachable: true,
        hostname:  hostname.into(),
        ip_addr:   ip_addr.into(),
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

fn get_or_refresh_token() -> Option<String> {
    {
        let guard = AUTH_TOKEN.lock().ok()?;
        if let Some(t) = guard.as_ref() {
            return Some(t.clone());
        }
    }

    let email    = std::env::var("BESZEL_ADMIN_EMAIL").ok()?;
    let password = std::env::var("BESZEL_ADMIN_PASSWORD").ok()?;

    #[derive(Deserialize)]
    struct AuthResp { token: String }

    let resp = ureq::post(&format!(
        "{}/api/collections/_superusers/auth-with-password",
        BESZEL_HOST
    ))
    .timeout(TIMEOUT)
    .send_json(serde_json::json!({ "identity": email, "password": password }))
    .ok()?;

    let body: AuthResp = resp.into_json().ok()?;

    let mut guard = AUTH_TOKEN.lock().ok()?;
    *guard = Some(body.token.clone());
    Some(body.token)
}

// ── Beszel API ────────────────────────────────────────────────────────

fn beszel_alive() -> bool {
    matches!(
        ureq::get(&format!("{}/api/health", BESZEL_HOST))
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

fn fetch_beszel_metrics() -> Option<BeszelMetrics> {
    let token = get_or_refresh_token()?;

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
        BESZEL_HOST, VPS_SYSTEM_NAME
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
        BESZEL_HOST, system_id
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

// ── VPS fetch ─────────────────────────────────────────────────────────

pub fn fetch_vps() -> crate::ServiceMetrics {
    if !beszel_alive() {
        return offline_metrics(VPS_HOSTNAME, VPS_IP_ADDR);
    }

    let m = match fetch_beszel_metrics() {
        Some(m) => m,
        None    => return online_no_metrics(VPS_HOSTNAME, VPS_IP_ADDR),
    };

    let load1  = m.load.first()  .copied().unwrap_or(0.0);
    let load5  = m.load.get(1)   .copied().unwrap_or(0.0);
    let load15 = m.load.get(2)   .copied().unwrap_or(0.0);

    crate::ServiceMetrics {
        hostname:  VPS_HOSTNAME.into(),
        ip_addr:   VPS_IP_ADDR.into(),
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
