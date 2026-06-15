use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Cached Beszel auth token, keyed on the URL it was issued for. Keying on the
/// URL means that when an operator points the config at a different Beszel
/// instance, the stale token is discarded up front rather than tried, 401'd,
/// and refreshed — which would otherwise blank all three panels for one cycle.
static AUTH_TOKEN: Mutex<Option<(String, String, String)>> = Mutex::new(None);

// ── Public types ──────────────────────────────────────────────────────

/// Hub-level Beszel connection settings, snapshotted from
/// [`crate::config::AppConfig`] each polling cycle. The hub is enumerated for
/// all its systems, which are then ordered/filtered per config and turned into
/// one screen each.
#[derive(Clone)]
pub struct BeszelHub {
    /// Base URL of the Beszel instance, e.g. `http://host:8090`.
    pub beszel_url:     String,
    /// Beszel user email used to obtain an API auth token. A read-only user
    /// with the monitored systems shared to it is sufficient.
    pub email:          String,
    /// Beszel user password.
    pub password:       String,
    /// Ordering prefix: listed system names sort first, in this order.
    pub system_order:   Vec<String>,
    /// System names to hide entirely.
    pub hidden_systems: Vec<String>,
}

impl BeszelHub {
    pub fn from_config(cfg: &crate::config::AppConfig) -> Self {
        Self {
            beszel_url:     cfg.beszel_url.clone(),
            email:          cfg.beszel_email.clone(),
            password:       cfg.beszel_password.clone(),
            system_order:   cfg.system_order.clone(),
            hidden_systems: cfg.hidden_systems.clone(),
        }
    }
}

/// One system as discovered on the hub, before its detailed stats are fetched.
struct SystemBrief {
    id:          String,
    name:        String,
    uptime_secs: u64,
    load:        Vec<f64>,
}

pub struct RemoteAlert {
    pub name:     String,
    pub severity: String,
    pub age:      String,
    pub summary:  String,
}

// ── ServiceMetrics constructors ───────────────────────────────────────

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
        if let Some((url, cached_email, token)) = guard.as_ref() {
            // Key on (url, email) so reconfiguring to different credentials on
            // the same hub — e.g. swapping a superuser login for a read-only
            // user — forces a fresh auth instead of reusing the stale token.
            if url == beszel_url && cached_email == email {
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
    *guard = Some((beszel_url.to_string(), email.to_string(), body.token.clone()));
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

/// Latest 1-minute stats for a system.
struct Stats {
    cpu_pct:       f64,
    ram_used_gb:   f64,
    ram_total_gb:  f64,
    ram_pct:       f64,
    disk_used_gb:  f64,
    disk_total_gb: f64,
    disk_pct:      f64,
}

/// Enumerate every system the hub knows about (no name filter), in hub order.
/// Ordering/hiding is applied separately ([`order_systems`]) so it stays a
/// pure, testable transform.
fn fetch_all_systems(beszel_url: &str, email: &str, password: &str) -> Option<Vec<SystemBrief>> {
    let token = get_or_refresh_token(beszel_url, email, password)?;

    #[derive(Deserialize)]
    struct SystemRecord {
        id:   String,
        name: String,
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
        "{}/api/collections/systems/records?perPage=200&sort=name",
        beszel_url
    ))
    .set("Authorization", &token)
    .timeout(TIMEOUT)
    .call();

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => { invalidate_token(); return None; }
        Err(e) => { eprintln!("beszel systems list error: {e}"); return None; }
    };

    let list: SystemsList = resp.into_json().ok()?;
    Some(list.items.into_iter().map(|s| SystemBrief {
        id:          s.id,
        name:        s.name,
        uptime_secs: s.info.u.unwrap_or(0.0) as u64,
        load:        s.info.la.unwrap_or_default(),
    }).collect())
}

/// Fetch the latest 1-minute stats for a system by its record id.
fn fetch_stats(beszel_url: &str, system_id: &str, email: &str, password: &str) -> Option<Stats> {
    let token = get_or_refresh_token(beszel_url, email, password)?;

    #[derive(Deserialize)]
    struct StatsRecord { stats: RawStats }
    #[derive(Deserialize)]
    struct RawStats {
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

    let resp = ureq::get(&format!(
        "{}/api/collections/system_stats/records?sort=-created&perPage=1&filter=system%3D%22{}%22%26%26type%3D%221m%22",
        beszel_url,
        percent_encode(system_id)
    ))
    .set("Authorization", &token)
    .timeout(TIMEOUT)
    .call();

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(401, _)) => { invalidate_token(); return None; }
        Err(e) => { eprintln!("beszel stats fetch error: {e}"); return None; }
    };

    let list: StatsList = resp.into_json().ok()?;
    let s = list.items.into_iter().next()?.stats;
    Some(Stats {
        cpu_pct:       s.cpu,
        ram_used_gb:   s.mu,
        ram_total_gb:  s.m,
        ram_pct:       s.mp,
        disk_used_gb:  s.du,
        disk_total_gb: s.d,
        disk_pct:      s.dp,
    })
}

/// Order and filter discovered systems for display. Systems whose name is in
/// `order` come first, in that order; the rest follow in their original (hub)
/// order. Names in `hidden` are dropped. Pure — no network — so it's unit
/// tested directly.
fn order_systems(
    systems: Vec<SystemBrief>,
    order:   &[String],
    hidden:  &[String],
) -> Vec<SystemBrief> {
    let mut visible: Vec<SystemBrief> = systems
        .into_iter()
        .filter(|s| !hidden.iter().any(|h| h == &s.name))
        .collect();
    // Stable sort keeps unlisted systems in hub order behind the listed ones.
    visible.sort_by_key(|s| order.iter().position(|o| o == &s.name).unwrap_or(usize::MAX));
    visible
}

// ── Beszel system fetch ───────────────────────────────────────────────

/// Build the ordered, filtered list of per-system metrics for the UI.
///
/// Blank hub credentials or a dead hub short-circuit to an empty list, so the
/// carousel shows just its fixed screens rather than spamming failed requests.
/// The caller probes [`beszel_alive`] once per cycle and passes the result in.
pub fn fetch_systems(hub: &BeszelHub, hub_alive: bool) -> Vec<crate::ServiceMetrics> {
    if hub.beszel_url.is_empty()
        || hub.email.is_empty()
        || hub.password.is_empty()
        || !hub_alive
    {
        return vec![];
    }

    let systems = match fetch_all_systems(&hub.beszel_url, &hub.email, &hub.password) {
        Some(s) => s,
        None    => return vec![],
    };

    order_systems(systems, &hub.system_order, &hub.hidden_systems)
        .into_iter()
        .map(|brief| {
            match fetch_stats(&hub.beszel_url, &brief.id, &hub.email, &hub.password) {
                Some(stats) => service_metrics(&brief, &stats),
                None        => online_no_metrics(&brief.name),
            }
        })
        .collect()
}

/// Discover the names of every system on the hub, in hub order, for the config
/// UI's system manager. Returns `None` if the hub can't be reached or auth
/// fails, so the caller can fall back to the saved configuration.
pub fn discover_system_names(beszel_url: &str, email: &str, password: &str) -> Option<Vec<String>> {
    fetch_all_systems(beszel_url, email, password)
        .map(|systems| systems.into_iter().map(|s| s.name).collect())
}

/// Worst stoplight level across a set of systems: 0 nominal, 1 warn, 2 crit.
/// Mirrors the Slint-side rule so the overview header and per-screen colors
/// agree. An empty set is nominal.
pub fn worst_level(systems: &[crate::ServiceMetrics]) -> i32 {
    systems.iter().map(level_of).max().unwrap_or(0)
}

fn level_of(m: &crate::ServiceMetrics) -> i32 {
    if !m.reachable {
        return 2;
    }
    let worst = m.cpu_pct.max(m.temp_pct).max(m.ram_pct).max(m.disk_pct);
    if worst > 0.85 { 2 } else if worst > 0.70 { 1 } else { 0 }
}

fn service_metrics(brief: &SystemBrief, s: &Stats) -> crate::ServiceMetrics {
    let load1  = brief.load.first().copied().unwrap_or(0.0);
    let load5  = brief.load.get(1).copied().unwrap_or(0.0);
    let load15 = brief.load.get(2).copied().unwrap_or(0.0);

    crate::ServiceMetrics {
        hostname:  brief.name.as_str().into(),
        reachable: true,
        uptime:    fmt_uptime(brief.uptime_secs).into(),
        load_avg:  format!("{:.2}  {:.2}  {:.2}", load1, load5, load15).into(),
        cpu_usage: format!("{:.1}%", s.cpu_pct).into(),
        cpu_pct:   (s.cpu_pct / 100.0).clamp(0.0, 1.0) as f32,
        cpu_temp:  "—".into(),
        temp_pct:  0.0,
        ram:       format!("{:.1} / {:.1} GB", s.ram_used_gb, s.ram_total_gb).into(),
        ram_pct:   (s.ram_pct / 100.0).clamp(0.0, 1.0) as f32,
        disk:      format!("{:.1} / {:.1} GB", s.disk_used_gb, s.disk_total_gb).into(),
        disk_pct:  (s.disk_pct / 100.0).clamp(0.0, 1.0) as f32,
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

    fn brief(name: &str) -> SystemBrief {
        SystemBrief { id: name.into(), name: name.into(), uptime_secs: 0, load: vec![] }
    }

    fn names(systems: Vec<SystemBrief>) -> Vec<String> {
        systems.into_iter().map(|s| s.name).collect()
    }

    #[test]
    fn order_systems_lists_ordered_first_then_hub_order() {
        let systems = vec![brief("c"), brief("a"), brief("b"), brief("d")];
        let order   = vec!["b".to_string(), "a".to_string()];
        // b, a come first (config order); c, d follow in hub order.
        assert_eq!(names(order_systems(systems, &order, &[])), ["b", "a", "c", "d"]);
    }

    #[test]
    fn order_systems_drops_hidden() {
        let systems = vec![brief("a"), brief("b"), brief("c")];
        let hidden  = vec!["b".to_string()];
        assert_eq!(names(order_systems(systems, &[], &hidden)), ["a", "c"]);
    }

    #[test]
    fn order_systems_default_is_hub_order() {
        let systems = vec![brief("z"), brief("a"), brief("m")];
        assert_eq!(names(order_systems(systems, &[], &[])), ["z", "a", "m"]);
    }
}
