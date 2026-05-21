use sysinfo::{Components, Disks, Networks, System};

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
    let secs = System::uptime();
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {:02}m", days, hours, mins)
    } else {
        format!("{}h {:02}m", hours, mins)
    }
}

fn load_avg() -> String {
    let la = System::load_average();
    format!("{:.2}  {:.2}  {:.2}", la.one, la.five, la.fifteen)
}

fn cpu_temp_with_pct(components: &Components) -> (String, f32) {
    for comp in components {
        let label = comp.label().to_lowercase();
        if label.contains("cpu") || label.contains("core") || label.contains("package") {
            if let Some(t) = comp.temperature() {
                // Pi 3 throttles at 80°C; normalize to that range
                return (format!("{:.1} °C", t), (t / 80.0).clamp(0.0, 1.0));
            }
        }
    }
    // Fallback for Pi 3 (thermal zone not exposed via hwmon)
    if let Ok(raw) = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
        if let Ok(millideg) = raw.trim().parse::<u32>() {
            let t = millideg as f32 / 1000.0;
            return (format!("{:.1} °C", t), (t / 80.0).clamp(0.0, 1.0));
        }
    }
    ("—".to_string(), 0.0)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    }
}

fn ram(sys: &System) -> String {
    format!("{} / {}", format_bytes(sys.used_memory()), format_bytes(sys.total_memory()))
}

fn ram_pct(sys: &System) -> f32 {
    let total = sys.total_memory();
    if total == 0 { return 0.0; }
    sys.used_memory() as f32 / total as f32
}

fn disk(disks: &Disks) -> String {
    let target = std::path::Path::new("/");
    let found = disks.iter().find(|d| d.mount_point() == target)
        .or_else(|| disks.iter().next());
    match found {
        Some(d) => {
            let used = d.total_space().saturating_sub(d.available_space());
            format!("{} / {}", format_bytes(used), format_bytes(d.total_space()))
        }
        None => "—".to_string(),
    }
}

fn disk_pct(disks: &Disks) -> f32 {
    let target = std::path::Path::new("/");
    let found = disks.iter().find(|d| d.mount_point() == target)
        .or_else(|| disks.iter().next());
    match found {
        Some(d) if d.total_space() > 0 => {
            let used = d.total_space().saturating_sub(d.available_space());
            used as f32 / d.total_space() as f32
        }
        _ => 0.0,
    }
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
    (format_rate(rx_total / 5), format_rate(tx_total / 5))
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
