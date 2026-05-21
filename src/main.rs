#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::time::Duration;

use chrono::Local;

mod system_info;
mod remote_metrics;

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

    // ── Clock + heartbeat (1 s) ──────────────────────────────────────
    let ui_handle2 = ui.as_weak();
    let clock_timer = slint::Timer::default();
    clock_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            if let Some(ui) = ui_handle2.upgrade() {
                ui.set_clock_str(current_clock());
                ui.set_heartbeat(!ui.get_heartbeat());
            }
        },
    );

    // ── Remote metrics (VPS + Alertmanager) — background thread ─────
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || loop {
        let vps    = remote_metrics::VpsSnapshot::fetch();
        let alerts = remote_metrics::RemoteAlert::fetch_all();

        let ui = ui_weak.clone();
        slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else { return };

            ui.set_vps_metrics(VpsMetrics {
                reachable:   vps.reachable,
                cadvisor_up: vps.cadvisor_up,
                uptime:      vps.uptime.into(),
                load_avg:    vps.load_avg.into(),
                cpu_usage:   vps.cpu_usage.into(),
                cpu_pct:     vps.cpu_pct,
                ram:         vps.ram.into(),
                ram_pct:     vps.ram_pct,
                disk:        vps.disk.into(),
                disk_pct:    vps.disk_pct,
            });

            let items: Vec<HudAlert> = alerts.iter().map(|a| HudAlert {
                name:     a.name.clone().into(),
                severity: a.severity.clone().into(),
                age:      a.age.clone().into(),
                summary:  a.summary.clone().into(),
            }).collect();
            let count = items.len() as i32;
            ui.set_alerts(slint::ModelRc::new(slint::VecModel::from(items)));
            ui.set_alert_count(count);
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
