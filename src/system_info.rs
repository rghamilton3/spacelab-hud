use sysinfo::{Components, Disks, Networks, System};

use crate::format::{fmt_bytes, fmt_load_avg, fmt_uptime};

pub struct SystemMetrics {
    pub hostname:  String,
    pub ip_addr:   String,
    pub uptime:    String,
    pub load_avg:  String,
    pub cpu_usage: String,
    pub cpu_pct:   f32,
    pub cpu_temp:  String,
    pub temp_pct:  f32,
    pub ram:       String,
    pub ram_pct:   f32,
    pub disk:      String,
    pub disk_pct:  f32,
    pub net_rx:    String,
    pub net_tx:    String,
}

pub struct MetricsCollector {
    sys:        System,
    networks:   Networks,
    disks:      Disks,
    components: Components,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        // Prime CPU delta so first collect() returns a real value
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let mut networks = Networks::new_with_refreshed_list();
        // Second refresh resets the delta counter so first collect() shows ~0 rates
        networks.refresh(true);
        Self {
            sys,
            networks,
            disks:      Disks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
        }
    }

    pub fn collect(&mut self) -> SystemMetrics {
        self.sys.refresh_memory();
        self.sys.refresh_cpu_all();
        self.networks.refresh(true);
        self.disks.refresh(false);
        self.components.refresh(false);

        let cpu_pct = self.sys.global_cpu_usage() / 100.0;
        let ram_pct = ram_pct(&self.sys);
        let disk_pct = disk_pct(&self.disks);
        let (cpu_temp, temp_pct) = cpu_temp_with_pct(&self.components);
        let (net_rx, net_tx) = net_rates(&self.networks);

        SystemMetrics {
            hostname:  hostname(),
            ip_addr:   ip_addr(&self.networks),
            uptime:    uptime(),
            load_avg:  load_avg(),
            cpu_usage: format!("{:.1}%", self.sys.global_cpu_usage()),
            cpu_pct,
            cpu_temp,
            temp_pct,
            ram:       ram(&self.sys),
            ram_pct,
            disk:      disk(&self.disks),
            disk_pct,
            net_rx,
            net_tx,
        }
    }
}

fn hostname() -> String {
    System::host_name().unwrap_or_else(|| "—".to_string())
}

fn ip_addr(networks: &Networks) -> String {
    for (name, data) in networks {
        if name == "lo" {
            continue;
        }
        for ip_net in data.ip_networks() {
            if ip_net.addr.is_ipv4() {
                return ip_net.addr.to_string();
            }
        }
    }
    "—".to_string()
}

fn uptime() -> String {
    fmt_uptime(System::uptime())
}

fn load_avg() -> String {
    let la = System::load_average();
    fmt_load_avg(la.one, la.five, la.fifteen)
}

fn cpu_temp_with_pct(components: &Components) -> (String, f32) {
    for comp in components {
        let label = comp.label().to_lowercase();
        if label.contains("cpu") || label.contains("core") || label.contains("package") {
            if let Some(t) = comp.temperature() {
                return (format!("{:.1} °C", t), temp_to_pct(t));
            }
        }
    }
    // Fallback for Pi 3 (thermal zone not exposed via hwmon)
    if let Ok(raw) = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
        if let Ok(millideg) = raw.trim().parse::<u32>() {
            let t = millideg as f32 / 1000.0;
            return (format!("{:.1} °C", t), temp_to_pct(t));
        }
    }
    ("—".to_string(), 0.0)
}

// Pi 3 throttles at 80°C; normalize temperature to that range.
fn temp_to_pct(temp_c: f32) -> f32 {
    (temp_c / 80.0).clamp(0.0, 1.0)
}

fn ram(sys: &System) -> String {
    format!("{} / {}", fmt_bytes(sys.used_memory()), fmt_bytes(sys.total_memory()))
}

fn ram_pct(sys: &System) -> f32 {
    ram_pct_from(sys.used_memory(), sys.total_memory())
}

fn ram_pct_from(used: u64, total: u64) -> f32 {
    if total == 0 { return 0.0; }
    used as f32 / total as f32
}

fn disk(disks: &Disks) -> String {
    let target = std::path::Path::new("/");
    let found = disks.iter().find(|d| d.mount_point() == target)
        .or_else(|| disks.iter().next());
    match found {
        Some(d) => {
            let used = d.total_space().saturating_sub(d.available_space());
            format!("{} / {}", fmt_bytes(used), fmt_bytes(d.total_space()))
        }
        None => "—".to_string(),
    }
}

fn disk_pct(disks: &Disks) -> f32 {
    let target = std::path::Path::new("/");
    let found = disks.iter().find(|d| d.mount_point() == target)
        .or_else(|| disks.iter().next());
    match found {
        Some(d) => disk_used_pct(
            d.total_space().saturating_sub(d.available_space()),
            d.total_space(),
        ),
        None => 0.0,
    }
}

fn disk_used_pct(used: u64, total: u64) -> f32 {
    if total == 0 { return 0.0; }
    used as f32 / total as f32
}

fn net_rates(networks: &Networks) -> (String, String) {
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    for (name, data) in networks {
        if name == "lo" { continue; }
        rx_total += data.received();
        tx_total += data.transmitted();
    }
    // sysinfo returns bytes since last refresh (5s interval)
    (format_rate(rate_per_sec(rx_total, 5)), format_rate(rate_per_sec(tx_total, 5)))
}

fn rate_per_sec(bytes: u64, interval_secs: u64) -> u64 {
    if interval_secs == 0 { return 0; }
    bytes / interval_secs
}

fn format_rate(bps: u64) -> String {
    if bps >= 1_048_576 {
        format!("{:.1} MB/s", bps as f64 / 1_048_576.0)
    } else if bps >= 1024 {
        format!("{:.0} KB/s", bps as f64 / 1024.0)
    } else {
        format!("{} B/s", bps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_rate_uses_bytes_below_1024() {
        assert_eq!(format_rate(0), "0 B/s");
        assert_eq!(format_rate(999), "999 B/s");
        assert_eq!(format_rate(1023), "1023 B/s");
    }

    #[test]
    fn format_rate_crosses_to_kb_at_1024() {
        assert_eq!(format_rate(1024), "1 KB/s");
        assert_eq!(format_rate(1_048_575), "1024 KB/s");
    }

    #[test]
    fn format_rate_crosses_to_mb_at_1_mib() {
        assert_eq!(format_rate(1_048_576), "1.0 MB/s");
        assert_eq!(format_rate(10_485_760), "10.0 MB/s");
    }

    #[test]
    fn ram_pct_from_handles_zero_total() {
        assert_eq!(ram_pct_from(0, 0), 0.0);
        assert_eq!(ram_pct_from(100, 0), 0.0);
    }

    #[test]
    fn ram_pct_from_returns_used_over_total() {
        assert!((ram_pct_from(50, 100) - 0.5).abs() < f32::EPSILON);
        assert!((ram_pct_from(100, 100) - 1.0).abs() < f32::EPSILON);
        assert!((ram_pct_from(0, 100) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn disk_used_pct_handles_zero_total() {
        assert_eq!(disk_used_pct(0, 0), 0.0);
        assert_eq!(disk_used_pct(50, 0), 0.0);
    }

    #[test]
    fn disk_used_pct_returns_used_over_total() {
        assert!((disk_used_pct(250, 1000) - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn temp_to_pct_normalizes_against_80c() {
        assert_eq!(temp_to_pct(0.0), 0.0);
        assert!((temp_to_pct(40.0) - 0.5).abs() < f32::EPSILON);
        assert_eq!(temp_to_pct(80.0), 1.0);
    }

    #[test]
    fn temp_to_pct_clamps_out_of_range() {
        assert_eq!(temp_to_pct(100.0), 1.0);
        assert_eq!(temp_to_pct(-5.0), 0.0);
    }

    #[test]
    fn rate_per_sec_divides_evenly() {
        assert_eq!(rate_per_sec(0, 5), 0);
        assert_eq!(rate_per_sec(5_120, 5), 1024);
        assert_eq!(rate_per_sec(10_485_760, 5), 2_097_152);
    }

    #[test]
    fn rate_per_sec_returns_zero_for_zero_interval() {
        assert_eq!(rate_per_sec(1000, 0), 0);
    }
}
