use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde::Deserialize;

const PROMETHEUS:   &str = "http://localhost:9090/api/v1/query";
const ALERTMANAGER: &str = "http://localhost:9093/api/v2/alerts";
const TIMEOUT:      Duration = Duration::from_secs(5);

const VPS_HOSTNAME: &str = "spacevps";
const VPS_IP_ADDR:  &str = "spacevps.tail718406.ts.net";

// ── Public types ──────────────────────────────────────────────────────

pub struct RemoteAlert {
    pub name:     String,
    pub severity: String,
    pub age:      String,
    pub summary:  String,
}

// ── Prometheus helpers ────────────────────────────────────────────────

#[derive(Deserialize)]
struct PromResponse {
    status: String,
    data:   PromData,
}

#[derive(Deserialize)]
struct PromData {
    result: Vec<PromResult>,
}

#[derive(Deserialize)]
struct PromResult {
    value: (f64, String),
}

fn query(q: &str) -> Option<f64> {
    let url = format!("{}?query={}", PROMETHEUS, pct_encode(q));
    let resp = match ureq::get(&url).timeout(TIMEOUT).call() {
        Ok(r)  => r,
        Err(e) => { eprintln!("prometheus HTTP error for {q:?}: {e}"); return None; }
    };
    let parsed: PromResponse = match resp.into_json() {
        Ok(v)  => v,
        Err(e) => { eprintln!("prometheus JSON parse error for {q:?}: {e}"); return None; }
    };
    if parsed.status != "success" {
        eprintln!("prometheus non-success status {:?} for {q:?}", parsed.status);
        return None;
    }
    match parsed.data.result.first()?.value.1.parse() {
        Ok(v)  => Some(v),
        Err(e) => { eprintln!("prometheus value parse error for {q:?}: {e}"); None }
    }
}

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b => { let _ = std::fmt::write(&mut out, format_args!("%{:02X}", b)); }
        }
    }
    out
}

// ── Alertmanager helpers ──────────────────────────────────────────────

#[derive(Deserialize)]
struct AmAlert {
    labels:      AmLabels,
    annotations: AmAnnotations,
    #[serde(rename = "startsAt")]
    starts_at:   String,
    state:       String,
}

#[derive(Deserialize)]
struct AmLabels {
    alertname: String,
    severity:  Option<String>,
}

#[derive(Deserialize)]
struct AmAnnotations {
    summary: Option<String>,
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

// ── VPS fetch ─────────────────────────────────────────────────────────

pub fn fetch_vps() -> crate::ServiceMetrics {
    let reachable = query("up{job=\"vps-node\"}").map(|v| v > 0.5).unwrap_or(false);

    if !reachable {
        return offline_metrics(VPS_HOSTNAME, VPS_IP_ADDR);
    }

    let cpu_val = match query(r#"100-(avg(rate(node_cpu_seconds_total{mode="idle",job="vps-node"}[5m]))*100)"#) {
        Some(v) => v,
        None => {
            eprintln!("fetch_vps: cpu query failed despite probe passing — treating as offline");
            return offline_metrics(VPS_HOSTNAME, VPS_IP_ADDR);
        }
    };

    let ram_avail  = query(r#"node_memory_MemAvailable_bytes{job="vps-node"}"#).unwrap_or(0.0);
    let ram_total  = query(r#"node_memory_MemTotal_bytes{job="vps-node"}"#).unwrap_or(1.0).max(1.0);
    let disk_avail = query(r#"node_filesystem_avail_bytes{job="vps-node",mountpoint="/"}"#).unwrap_or(0.0);
    let disk_total = query(r#"node_filesystem_size_bytes{job="vps-node",mountpoint="/"}"#).unwrap_or(1.0).max(1.0);
    let boot_ts    = query(r#"node_boot_time_seconds{job="vps-node"}"#).unwrap_or(0.0);
    let load1      = query(r#"node_load1{job="vps-node"}"#).unwrap_or(0.0);
    let load5      = query(r#"node_load5{job="vps-node"}"#).unwrap_or(0.0);
    let load15     = query(r#"node_load15{job="vps-node"}"#).unwrap_or(0.0);

    // CPU temp via hwmon: not always exposed on virtualized VPS.
    // Returns avg of any reported temp sensors, or 0.0 if none.
    let temp_c = query(r#"avg(node_hwmon_temp_celsius{job="vps-node"})"#).unwrap_or(0.0);

    let ram_pct  = (1.0 - ram_avail  / ram_total) .clamp(0.0, 1.0) as f32;
    let disk_pct = (1.0 - disk_avail / disk_total).clamp(0.0, 1.0) as f32;
    let cpu_pct  = (cpu_val / 100.0)              .clamp(0.0, 1.0) as f32;
    let temp_pct = (temp_c  / 80.0)               .clamp(0.0, 1.0) as f32;

    let now_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
    let uptime = if boot_ts > 0.0 {
        fmt_uptime((now_ts - boot_ts).max(0.0) as u64)
    } else {
        "UNKNOWN".to_owned()
    };

    crate::ServiceMetrics {
        hostname:  VPS_HOSTNAME.into(),
        ip_addr:   VPS_IP_ADDR.into(),
        reachable: true,
        uptime:    uptime.into(),
        load_avg:  format!("{:.2}  {:.2}  {:.2}", load1, load5, load15).into(),
        cpu_usage: format!("{:.1}%", cpu_val).into(),
        cpu_pct,
        cpu_temp:  if temp_c > 0.0 { format!("{:.1} °C", temp_c).into() } else { "—".into() },
        temp_pct,
        ram:       format!("{} / {}", fmt_bytes((ram_total  - ram_avail)  as u64), fmt_bytes(ram_total  as u64)).into(),
        ram_pct,
        disk:      format!("{} / {}", fmt_bytes((disk_total - disk_avail) as u64), fmt_bytes(disk_total as u64)).into(),
        disk_pct,
    }
}

// ── RemoteAlert::fetch_all ────────────────────────────────────────────

impl RemoteAlert {
    /// Returns `None` when Alertmanager is unreachable or returns unparseable data.
    /// Returns `Some(vec![])` when reachable but no active alerts.
    pub fn fetch_all() -> Option<Vec<Self>> {
        let raw: Vec<AmAlert> = match ureq::get(ALERTMANAGER).timeout(TIMEOUT).call() {
            Ok(r) => match r.into_json() {
                Ok(v)  => v,
                Err(e) => { eprintln!("alertmanager JSON parse error: {e}"); return None; }
            },
            Err(e) => { eprintln!("alertmanager HTTP error: {e}"); return None; }
        };

        let now_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();

        Some(raw.into_iter()
            .filter(|a| a.state == "active" || a.state == "firing")
            .map(|a| {
                let starts = DateTime::parse_from_rfc3339(&a.starts_at)
                    .map(|dt| dt.timestamp() as f64)
                    .unwrap_or(now_ts);
                let age_secs = (now_ts - starts).max(0.0) as u64;
                RemoteAlert {
                    name:     a.labels.alertname,
                    severity: a.labels.severity.unwrap_or_else(|| "info".into()),
                    age:      fmt_age(age_secs),
                    summary:  a.annotations.summary.unwrap_or_default(),
                }
            })
            .collect())
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

fn fmt_age(secs: u64) -> String {
    if      secs < 60    { format!("{}s", secs) }
    else if secs < 3600  { format!("{}m", secs / 60) }
    else if secs < 86400 { format!("{}h", secs / 3600) }
    else                 { format!("{}d", secs / 86400) }
}

fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    }
}
