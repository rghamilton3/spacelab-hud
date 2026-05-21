use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde::Deserialize;

use crate::format::{fmt_bytes, fmt_load_avg, fmt_uptime};

const PROMETHEUS:   &str = "http://localhost:9090/api/v1/query";
const ALERTMANAGER: &str = "http://localhost:9093/api/v2/alerts";
const TIMEOUT:      Duration = Duration::from_secs(5);

// ── Public types ──────────────────────────────────────────────────────

pub struct VpsSnapshot {
    pub reachable:   bool,
    pub cadvisor_up: bool,
    pub uptime:      String,
    pub load_avg:    String,
    pub cpu_usage:   String,
    pub cpu_pct:     f32,
    pub ram:         String,
    pub ram_pct:     f32,
    pub disk:        String,
    pub disk_pct:    f32,
}

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

fn query_at(base: &str, q: &str) -> Option<f64> {
    let url = format!("{}?query={}", base, pct_encode(q));
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
        Self::fetch_from(PROMETHEUS)
    }

    fn fetch_from(prom_base: &str) -> Self {
        let reachable   = query_at(prom_base, "up{job=\"vps-node\"}")     .map(|v| v > 0.5).unwrap_or(false);
        let cadvisor_up = query_at(prom_base, "up{job=\"vps-cadvisor\"}") .map(|v| v > 0.5).unwrap_or(false);

        if !reachable {
            return Self {
                reachable:   false,
                cadvisor_up,
                uptime:    "—".into(),
                load_avg:  "—  —  —".into(),
                cpu_usage: "—".into(),
                cpu_pct:   0.0,
                ram:       "—".into(),
                ram_pct:   0.0,
                disk:      "—".into(),
                disk_pct:  0.0,
            };
        }

        let cpu_val  = query_at(prom_base, r#"100-(avg(rate(node_cpu_seconds_total{mode="idle",job="vps-node"}[5m]))*100)"#)
                           .unwrap_or(0.0);
        let ram_avail  = query_at(prom_base, r#"node_memory_MemAvailable_bytes{job="vps-node"}"#).unwrap_or(0.0);
        let ram_total  = query_at(prom_base, r#"node_memory_MemTotal_bytes{job="vps-node"}"#).unwrap_or(1.0).max(1.0);
        let disk_avail = query_at(prom_base, r#"node_filesystem_avail_bytes{job="vps-node",mountpoint="/"}"#).unwrap_or(0.0);
        let disk_total = query_at(prom_base, r#"node_filesystem_size_bytes{job="vps-node",mountpoint="/"}"#).unwrap_or(1.0).max(1.0);
        let boot_ts    = query_at(prom_base, r#"node_boot_time_seconds{job="vps-node"}"#).unwrap_or(0.0);
        let load1      = query_at(prom_base, r#"node_load1{job="vps-node"}"#).unwrap_or(0.0);
        let load5      = query_at(prom_base, r#"node_load5{job="vps-node"}"#).unwrap_or(0.0);
        let load15     = query_at(prom_base, r#"node_load15{job="vps-node"}"#).unwrap_or(0.0);

        let ram_pct  = (1.0 - ram_avail  / ram_total) .clamp(0.0, 1.0) as f32;
        let disk_pct = (1.0 - disk_avail / disk_total).clamp(0.0, 1.0) as f32;
        let cpu_pct  = (cpu_val / 100.0)              .clamp(0.0, 1.0) as f32;

        let now_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        let uptime_secs = (now_ts - boot_ts).max(0.0) as u64;

        Self {
            reachable:   true,
            cadvisor_up,
            uptime:    fmt_uptime(uptime_secs),
            load_avg:  fmt_load_avg(load1, load5, load15),
            cpu_usage: format!("{:.1}%", cpu_val),
            cpu_pct,
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
        Self::fetch_all_from(ALERTMANAGER)
    }

    fn fetch_all_from(am_url: &str) -> Vec<Self> {
        let raw: Vec<AmAlert> = ureq::get(am_url)
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

fn fmt_age(secs: u64) -> String {
    if      secs < 60    { format!("{}s", secs) }
    else if secs < 3600  { format!("{}m", secs / 60) }
    else if secs < 86400 { format!("{}h", secs / 3600) }
    else                 { format!("{}d", secs / 86400) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    // ── Pure formatters ──────────────────────────────────────────────

    #[test]
    fn pct_encode_passes_unreserved_through_unchanged() {
        assert_eq!(pct_encode(""), "");
        assert_eq!(pct_encode("abcXYZ0189"), "abcXYZ0189");
        assert_eq!(pct_encode("-_.~"), "-_.~");
    }

    #[test]
    fn pct_encode_escapes_reserved_and_special_chars() {
        assert_eq!(pct_encode(" "), "%20");
        assert_eq!(pct_encode("{}"), "%7B%7D");
        assert_eq!(pct_encode("="), "%3D");
        assert_eq!(pct_encode("\"job\""), "%22job%22");
    }

    #[test]
    fn pct_encode_handles_multibyte_utf8() {
        // 'é' is 0xC3 0xA9 in UTF-8
        assert_eq!(pct_encode("é"), "%C3%A9");
    }

    #[test]
    fn fmt_age_seconds_under_one_minute() {
        assert_eq!(fmt_age(0), "0s");
        assert_eq!(fmt_age(30), "30s");
        assert_eq!(fmt_age(59), "59s");
    }

    #[test]
    fn fmt_age_minutes_between_60s_and_one_hour() {
        assert_eq!(fmt_age(60), "1m");
        assert_eq!(fmt_age(65), "1m");
        assert_eq!(fmt_age(3_599), "59m");
    }

    #[test]
    fn fmt_age_hours_between_one_hour_and_one_day() {
        assert_eq!(fmt_age(3_600), "1h");
        assert_eq!(fmt_age(86_399), "23h");
    }

    #[test]
    fn fmt_age_days_at_and_above_one_day() {
        assert_eq!(fmt_age(86_400), "1d");
        assert_eq!(fmt_age(3 * 86_400), "3d");
    }

    // ── HTTP-mocked tests ────────────────────────────────────────────

    fn prom_base(server: &mockito::Server) -> String {
        format!("{}/api/v1/query", server.url())
    }

    fn am_url(server: &mockito::Server) -> String {
        format!("{}/api/v2/alerts", server.url())
    }

    fn prom_ok_body(value: &str) -> String {
        format!(
            r#"{{"status":"success","data":{{"result":[{{"value":[1700000000.0,"{}"]}}]}}}}"#,
            value,
        )
    }

    // Registers a 200/JSON response for a specific PromQL query.
    fn mock_prom_ok(server: &mut mockito::Server, q: &str, value: &str) -> mockito::Mock {
        server.mock("GET", "/api/v1/query")
            .match_query(Matcher::UrlEncoded("query".into(), q.into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(prom_ok_body(value))
            .create()
    }

    // Registers a single catch-all Prometheus response, used by negative tests.
    fn mock_prom_response(server: &mut mockito::Server, status: usize, body: &str) -> mockito::Mock {
        server.mock("GET", "/api/v1/query")
            .match_query(Matcher::Any)
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create()
    }

    fn mock_am_response(server: &mut mockito::Server, status: usize, body: &str) -> mockito::Mock {
        server.mock("GET", "/api/v2/alerts")
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create()
    }

    #[test]
    fn query_at_returns_value_on_success() {
        let mut server = mockito::Server::new();
        let _m = mock_prom_ok(&mut server, "vector(1)", "42.5");
        assert_eq!(query_at(&prom_base(&server), "vector(1)"), Some(42.5));
    }

    #[test]
    fn query_at_returns_none_on_http_error() {
        let mut server = mockito::Server::new();
        let _m = mock_prom_response(&mut server, 500, "boom");
        assert_eq!(query_at(&prom_base(&server), "anything"), None);
    }

    #[test]
    fn query_at_returns_none_when_status_field_is_not_success() {
        let mut server = mockito::Server::new();
        let _m = mock_prom_response(&mut server, 200, r#"{"status":"error","data":{"result":[]}}"#);
        assert_eq!(query_at(&prom_base(&server), "anything"), None);
    }

    #[test]
    fn query_at_returns_none_on_malformed_json() {
        let mut server = mockito::Server::new();
        let _m = mock_prom_response(&mut server, 200, "not json");
        assert_eq!(query_at(&prom_base(&server), "anything"), None);
    }

    #[test]
    fn query_at_returns_none_when_result_vector_is_empty() {
        let mut server = mockito::Server::new();
        let _m = mock_prom_response(&mut server, 200, r#"{"status":"success","data":{"result":[]}}"#);
        assert_eq!(query_at(&prom_base(&server), "anything"), None);
    }

    #[test]
    fn vps_snapshot_returns_unreachable_when_up_is_zero() {
        let mut server = mockito::Server::new();
        let _up = mock_prom_ok(&mut server, r#"up{job="vps-node"}"#, "0");
        let _ca = mock_prom_ok(&mut server, r#"up{job="vps-cadvisor"}"#, "1");

        let snap = VpsSnapshot::fetch_from(&prom_base(&server));
        assert!(!snap.reachable);
        assert!(snap.cadvisor_up);
        assert_eq!(snap.uptime, "—");
        assert_eq!(snap.load_avg, "—  —  —");
        assert_eq!(snap.cpu_pct, 0.0);
    }

    #[test]
    fn vps_snapshot_populates_all_fields_on_happy_path() {
        let mut server = mockito::Server::new();

        let pairs: &[(&str, &str)] = &[
            (r#"up{job="vps-node"}"#, "1"),
            (r#"up{job="vps-cadvisor"}"#, "1"),
            (r#"100-(avg(rate(node_cpu_seconds_total{mode="idle",job="vps-node"}[5m]))*100)"#, "25"),
            (r#"node_memory_MemAvailable_bytes{job="vps-node"}"#, "536870912"),    // 0.5 GiB
            (r#"node_memory_MemTotal_bytes{job="vps-node"}"#, "1073741824"),       // 1.0 GiB
            (r#"node_filesystem_avail_bytes{job="vps-node",mountpoint="/"}"#, "5368709120"),  // 5 GiB
            (r#"node_filesystem_size_bytes{job="vps-node",mountpoint="/"}"#, "21474836480"), // 20 GiB
            (r#"node_boot_time_seconds{job="vps-node"}"#, "0"),
            (r#"node_load1{job="vps-node"}"#, "0.50"),
            (r#"node_load5{job="vps-node"}"#, "0.75"),
            (r#"node_load15{job="vps-node"}"#, "1.00"),
        ];

        let mut mocks = Vec::with_capacity(pairs.len());
        for (q, v) in pairs {
            mocks.push(mock_prom_ok(&mut server, q, v));
        }

        let snap = VpsSnapshot::fetch_from(&prom_base(&server));
        assert!(snap.reachable);
        assert!(snap.cadvisor_up);
        assert!((snap.cpu_pct - 0.25).abs() < 1e-4);
        assert!((snap.ram_pct - 0.5).abs() < 1e-4);
        assert!((snap.disk_pct - 0.75).abs() < 1e-4);
        assert_eq!(snap.cpu_usage, "25.0%");
        assert_eq!(snap.load_avg, "0.50  0.75  1.00");
        assert_eq!(snap.ram, "512 MB / 1.0 GB");
        assert_eq!(snap.disk, "15.0 GB / 20.0 GB");

        drop(mocks);
    }

    #[test]
    fn fetch_all_alerts_parses_and_filters_by_state() {
        let mut server = mockito::Server::new();
        let body = r#"[
            {
                "labels": { "alertname": "ActiveOne", "severity": "warning" },
                "annotations": { "summary": "first" },
                "startsAt": "2026-01-15T10:30:00Z",
                "state": "active"
            },
            {
                "labels": { "alertname": "FiringTwo", "severity": "critical" },
                "annotations": { "summary": "second" },
                "startsAt": "2026-01-15T10:30:00Z",
                "state": "firing"
            },
            {
                "labels": { "alertname": "Suppressed", "severity": "info" },
                "annotations": { "summary": "third" },
                "startsAt": "2026-01-15T10:30:00Z",
                "state": "suppressed"
            }
        ]"#;
        let _m = mock_am_response(&mut server, 200, body);

        let alerts = RemoteAlert::fetch_all_from(&am_url(&server));
        let names: Vec<_> = alerts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["ActiveOne", "FiringTwo"]);
        assert_eq!(alerts[0].severity, "warning");
        assert_eq!(alerts[1].severity, "critical");
        assert_eq!(alerts[0].summary, "first");
    }

    #[test]
    fn fetch_all_alerts_defaults_missing_severity_to_info() {
        let mut server = mockito::Server::new();
        let body = r#"[
            {
                "labels": { "alertname": "NoSeverity" },
                "annotations": {},
                "startsAt": "2026-01-15T10:30:00Z",
                "state": "active"
            }
        ]"#;
        let _m = mock_am_response(&mut server, 200, body);

        let alerts = RemoteAlert::fetch_all_from(&am_url(&server));
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, "info");
        assert_eq!(alerts[0].summary, "");
    }

    #[test]
    fn fetch_all_alerts_returns_empty_on_http_error() {
        let mut server = mockito::Server::new();
        let _m = mock_am_response(&mut server, 500, "boom");
        assert!(RemoteAlert::fetch_all_from(&am_url(&server)).is_empty());
    }
}
