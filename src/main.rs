#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::time::Duration;

mod system_info;
use system_info::MetricsCollector;

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    let ui = AppWindow::new()?;

    let mut collector = MetricsCollector::new();
    push_metrics(&ui, collector.collect());

    let ui_handle = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(5),
        move || {
            if let Some(ui) = ui_handle.upgrade() {
                push_metrics(&ui, collector.collect());
            }
        },
    );

    ui.run()?;
    Ok(())
}

fn push_metrics(ui: &AppWindow, m: system_info::SystemMetrics) {
    ui.set_metrics(SysMetrics {
        hostname:  m.hostname.into(),
        ip_addr:   m.ip_addr.into(),
        uptime:    m.uptime.into(),
        load_avg:  m.load_avg.into(),
        cpu_usage: m.cpu_usage.into(),
        cpu_temp:  m.cpu_temp.into(),
        ram:       m.ram.into(),
        disk:      m.disk.into(),
    });
}
