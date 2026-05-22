#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use chrono::Local;

mod system_info;
mod remote_metrics;

const NETWORK_PROBE_HOST:     &str      = "spacevps.tail718406.ts.net";
const NETWORK_PROBE_PORT:     u16       = 22;
const NETWORK_PROBE_TIMEOUT:  Duration  = Duration::from_millis(500);
const NETWORK_PROBE_INTERVAL: Duration  = Duration::from_secs(5);

fn probe_network() -> bool {
    let addrs = match (NETWORK_PROBE_HOST, NETWORK_PROBE_PORT).to_socket_addrs() {
        Ok(a)  => a,
        Err(e) => { eprintln!("probe_network: DNS resolution failed: {e}"); return false; }
    };
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, NETWORK_PROBE_TIMEOUT) {
            Ok(_)  => return true,
            Err(e) => eprintln!("probe_network: connect to {addr} failed: {e}"),
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

    // ── NAS + Home Assistant — start offline until fetch threads added ─
    ui.set_nas_metrics(remote_metrics::offline_metrics("nas-01", "192.168.1.20"));
    ui.set_ha_metrics(remote_metrics::offline_metrics("homeassistant.local", "192.168.1.30"));

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

    // ── Network reachability probe (TCP to Tailscale host) ────────────
    let ui_weak_net = ui.as_weak();
    std::thread::spawn(move || loop {
        let result = std::panic::catch_unwind(probe_network);
        let reachable = result.unwrap_or_else(|_| {
            eprintln!("probe_network: thread panicked");
            false
        });
        let ui = ui_weak_net.clone();
        if let Err(e) = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_local_network_reachable(reachable);
            }
        }) {
            eprintln!("network probe: event loop gone ({e:?}), exiting thread");
            return;
        }
        std::thread::sleep(NETWORK_PROBE_INTERVAL);
    });

    // ── Remote metrics (VPS + Alertmanager) — background thread ──────
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || loop {
        let (vps, alerts) = match std::panic::catch_unwind(|| {
            (remote_metrics::fetch_vps(), remote_metrics::RemoteAlert::fetch_all())
        }) {
            Ok(pair) => pair,
            Err(_) => {
                eprintln!("remote_metrics: fetch thread panicked, skipping tick");
                std::thread::sleep(Duration::from_secs(15));
                continue;
            }
        };

        let ui = ui_weak.clone();
        if let Err(e) = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else { return };

            ui.set_vps_metrics(vps);

            let reachable = alerts.is_some();
            let alert_vec = alerts.unwrap_or_default();

            let items: Vec<HudAlert> = alert_vec.iter().map(|a| HudAlert {
                name:     a.name.clone().into(),
                severity: a.severity.clone().into(),
                age:      a.age.clone().into(),
                summary:  a.summary.clone().into(),
            }).collect();
            let count = items.len() as i32;
            let worst_sev: i32 = alert_vec.iter().map(|a| match a.severity.as_str() {
                "critical" => 2,
                "warning"  => 1,
                _          => 0,
            }).max().unwrap_or(0);

            ui.set_alerts_reachable(reachable);
            ui.set_alerts(slint::ModelRc::new(slint::VecModel::from(items)));
            ui.set_alert_count(count);
            ui.set_alert_worst_severity(worst_sev);
        }) {
            eprintln!("remote_metrics: event loop gone ({e:?}), exiting thread");
            return;
        }

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
