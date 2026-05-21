use sysinfo::{Components, Disks, Networks, System};

pub struct SystemMetrics {
    pub hostname:  String,
    pub ip_addr:   String,
    pub uptime:    String,
    pub load_avg:  String,
    pub cpu_usage: String,
    pub cpu_temp:  String,
    pub ram:       String,
    pub disk:      String,
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
        // Prime the CPU delta so the first collect() returns a real value
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        Self {
            sys,
            networks:   Networks::new_with_refreshed_list(),
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

        SystemMetrics {
            hostname:  hostname(),
            ip_addr:   ip_addr(&self.networks),
            uptime:    uptime(),
            load_avg:  load_avg(),
            cpu_usage: cpu_usage(&self.sys),
            cpu_temp:  cpu_temp(&self.components),
            ram:       ram(&self.sys),
            disk:      disk(&self.disks),
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

fn cpu_usage(sys: &System) -> String {
    format!("{:.1}%", sys.global_cpu_usage())
}

fn cpu_temp(components: &Components) -> String {
    for comp in components {
        let label = comp.label().to_lowercase();
        if label.contains("cpu") || label.contains("core") || label.contains("package") {
            if let Some(t) = comp.temperature() {
                return format!("{:.1} °C", t);
            }
        }
    }
    // Fallback for Pi 3 (thermal zone not exposed via hwmon)
    if let Ok(raw) = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
        if let Ok(millideg) = raw.trim().parse::<u32>() {
            return format!("{:.1} °C", millideg as f32 / 1000.0);
        }
    }
    "—".to_string()
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
