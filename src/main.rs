#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use chrono::Local;

mod system_info;
mod remote_metrics;

const NETWORK_PROBE_HOST: &str = "spacevps.tail718406.ts.net";
const NETWORK_PROBE_PORT: u16  = 22;
const NETWORK_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const NETWORK_PROBE_INTERVAL: Duration = Duration::from_secs(5);

fn probe_network() -> bool {
    let Ok(addrs) = (NETWORK_PROBE_HOST, NETWORK_PROBE_PORT).to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        if TcpStream::connect_timeout(&addr, NETWORK_PROBE_TIMEOUT).is_ok() {
            return true;
        }
    }
    false
}

use system_info::MetricsCollector;

slint::include_modules!();

#[cfg(target_os = "linux")]
fn configure_platform() {
    use i_slint_backend_winit::Backend;
    use winit::platform::wayland::WindowAttributesExtWayland;
    let backend = Backend::builder()
        .with_window_attributes_hook(|attrs| attrs.with_name("spacelab-hub", ""))
        .build()
        .unwrap();
    slint::platform::set_platform(Box::new(backend)).unwrap();
}

fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    configure_platform();

    let ui = AppWindow::new()?;

    // ── Local metrics (Pi) ───────────────────────────────────────────
    let mut collector = MetricsCollector::new();
    push_local_metrics(&ui, collector.collect());
    ui.set_clock_str(current_clock());

    // ── Mock NAS + Home Assistant (static for now) ──────────────────
    ui.set_nas_metrics(ServiceMetrics {
        hostname:  "nas-01".into(),
        ip_addr:   "192.168.1.20".into(),
        reachable: true,
        uptime:    "47d 18h 32m".into(),
        load_avg:  "0.12  0.08  0.05".into(),
        cpu_usage: "8.4%".into(),
        cpu_pct:   0.084,
        cpu_temp:  "42.0 °C".into(),
        temp_pct:  0.525,
        ram:       "4.2 GB / 16.0 GB".into(),
        ram_pct:   0.26,
        disk:      "6.1 TB / 12.0 TB".into(),
        disk_pct:  0.51,
    });
    ui.set_ha_metrics(ServiceMetrics {
        hostname:  "homeassistant.local".into(),
        ip_addr:   "192.168.1.30".into(),
        reachable: true,
        uptime:    "9d 02h 14m".into(),
        load_avg:  "1.20  0.95  0.80".into(),
        cpu_usage: "76.2%".into(),
        cpu_pct:   0.762,
        cpu_temp:  "61.5 °C".into(),
        temp_pct:  0.769,
        ram:       "3.1 GB / 4.0 GB".into(),
        ram_pct:   0.78,
        disk:      "18.0 GB / 32.0 GB".into(),
        disk_pct:  0.56,
    });

    let ui_handle = ui.as_weak();
    let metrics_timer = slint::Timer::default();
    metrics_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(5),
        move || {
            if let Some(ui) = ui_handle.upgrade() {
                push_local_metrics(&ui, collector.collect());
            }
        },
    );

    // ── Clock (1 s) ──────────────────────────────────────────────────
    let ui_handle2 = ui.as_weak();
    let clock_timer = slint::Timer::default();
    clock_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            if let Some(ui) = ui_handle2.upgrade() {
                ui.set_clock_str(current_clock());
            }
        },
    );

    // ── Local network reachability (TCP probe to Tailscale host) ────
    let ui_weak_net = ui.as_weak();
    std::thread::spawn(move || loop {
        let reachable = probe_network();
        let ui = ui_weak_net.clone();
        slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_local_network_reachable(reachable);
            }
        }).ok();
        std::thread::sleep(NETWORK_PROBE_INTERVAL);
    });

    // ── Remote metrics (VPS + Alertmanager) — background thread ─────
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || loop {
        let vps    = remote_metrics::VpsSnapshot::fetch();
        let alerts = remote_metrics::RemoteAlert::fetch_all();

        let ui = ui_weak.clone();
        slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else { return };

            ui.set_vps_metrics(ServiceMetrics {
                hostname:  vps.hostname.into(),
                ip_addr:   vps.ip_addr.into(),
                reachable: vps.reachable,
                uptime:    vps.uptime.into(),
                load_avg:  vps.load_avg.into(),
                cpu_usage: vps.cpu_usage.into(),
                cpu_pct:   vps.cpu_pct,
                cpu_temp:  vps.cpu_temp.into(),
                temp_pct:  vps.temp_pct,
                ram:       vps.ram.into(),
                ram_pct:   vps.ram_pct,
                disk:      vps.disk.into(),
                disk_pct:  vps.disk_pct,
            });

            let items: Vec<HudAlert> = alerts.iter().map(|a| HudAlert {
                name:     a.name.clone().into(),
                severity: a.severity.clone().into(),
                age:      a.age.clone().into(),
                summary:  a.summary.clone().into(),
            }).collect();
            let count = items.len() as i32;
            let worst_sev: i32 = alerts.iter().map(|a| match a.severity.as_str() {
                "critical" => 2,
                "warning"  => 1,
                _          => 0,
            }).max().unwrap_or(0);
            ui.set_alerts(slint::ModelRc::new(slint::VecModel::from(items)));
            ui.set_alert_count(count);
            ui.set_alert_worst_severity(worst_sev);
        }).ok();

        std::thread::sleep(Duration::from_secs(15));
    });

    ui.run()?;
    Ok(())
}

fn current_clock() -> slint::SharedString {
    Local::now().format("%H:%M:%S").to_string().into()
}

fn push_local_metrics(ui: &AppWindow, m: system_info::SystemMetrics) {
    ui.set_metrics(SysMetrics {
        hostname:  m.hostname.into(),
        ip_addr:   m.ip_addr.into(),
        uptime:    m.uptime.into(),
        load_avg:  m.load_avg.into(),
        cpu_usage: m.cpu_usage.into(),
        cpu_pct:   m.cpu_pct,
        cpu_temp:  m.cpu_temp.into(),
        temp_pct:  m.temp_pct,
        ram:       m.ram.into(),
        ram_pct:   m.ram_pct,
        disk:      m.disk.into(),
        disk_pct:  m.disk_pct,
        net_rx:    m.net_rx.into(),
        net_tx:    m.net_tx.into(),
    });
}
