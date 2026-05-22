use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde::Deserialize;

const PROMETHEUS:   &str = "http://localhost:9090/api/v1/query";
const ALERTMANAGER: &str = "http://localhost:9093/api/v2/alerts";
const TIMEOUT:      Duration = Duration::from_secs(5);

// ── Public types ──────────────────────────────────────────────────────

pub struct VpsSnapshot {
    pub hostname:    String,
    pub ip_addr:     String,
    pub reachable:   bool,
    pub uptime:      String,
    pub load_avg:    String,
    pub cpu_usage:   String,
    pub cpu_pct:     f32,
    pub cpu_temp:    String,
    pub temp_pct:    f32,
    pub ram:         String,
    pub ram_pct:     f32,
    pub disk:        String,
    pub disk_pct:    f32,
}

const VPS_HOSTNAME: &str = "spacevps";
const VPS_IP_ADDR:  &str = "spacevps.tail718406.ts.net";

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
    let resp: PromResponse = ureq::get(&url)
        .timeout(TIMEOUT)
        .call()
        .ok()?
        .into_json()
        .ok()?;
    if resp.status != "success" { return None; }
    resp.data.result.first()?.value.1.parse().ok()
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

// ── VpsSnapshot::fetch ────────────────────────────────────────────────

impl VpsSnapshot {
    pub fn fetch() -> Self {
        let reachable = query("up{job=\"vps-node\"}").map(|v| v > 0.5).unwrap_or(false);

        if !reachable {
            return Self {
                hostname:  VPS_HOSTNAME.into(),
                ip_addr:   VPS_IP_ADDR.into(),
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
            };
        }

        let cpu_val  = query(r#"100-(avg(rate(node_cpu_seconds_total{mode="idle",job="vps-node"}[5m]))*100)"#)
                           .unwrap_or(0.0);
        let ram_avail = query(r#"node_memory_MemAvailable_bytes{job="vps-node"}"#).unwrap_or(0.0);
        let ram_total = query(r#"node_memory_MemTotal_bytes{job="vps-node"}"#).unwrap_or(1.0).max(1.0);
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
        let temp_pct = (temp_c / 80.0).clamp(0.0, 1.0) as f32;

        let now_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        let uptime_secs = (now_ts - boot_ts).max(0.0) as u64;

        Self {
            hostname:  VPS_HOSTNAME.into(),
            ip_addr:   VPS_IP_ADDR.into(),
            reachable: true,
            uptime:    fmt_uptime(uptime_secs),
            load_avg:  format!("{:.2}  {:.2}  {:.2}", load1, load5, load15),
            cpu_usage: format!("{:.1}%", cpu_val),
            cpu_pct,
            cpu_temp:  if temp_c > 0.0 { format!("{:.1} °C", temp_c) } else { "—".into() },
            temp_pct,
            ram:       format!("{} / {}", fmt_bytes((ram_total - ram_avail) as u64), fmt_bytes(ram_total as u64)),
            ram_pct,
            disk:      format!("{} / {}", fmt_bytes((disk_total - disk_avail) as u64), fmt_bytes(disk_total as u64)),
            disk_pct,
        }
    }
}

// ── RemoteAlert::fetch_all ────────────────────────────────────────────

impl RemoteAlert {
    pub fn fetch_all() -> Vec<Self> {
        let raw: Vec<AmAlert> = ureq::get(ALERTMANAGER)
            .timeout(TIMEOUT)
            .call()
            .ok()
            .and_then(|r| r.into_json().ok())
            .unwrap_or_default();

        let now_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();

        raw.into_iter()
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
            .collect()
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
