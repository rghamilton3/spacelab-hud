#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use chrono::Local;

mod config;
mod fan_telem;
mod github_watcher;
mod remote_metrics;
mod shared_state;
mod system_info;
mod web_config;

const NETWORK_PROBE_TIMEOUT:  Duration  = Duration::from_millis(500);
const NETWORK_PROBE_INTERVAL: Duration  = Duration::from_secs(5);

fn probe_network(host: &str, port: u16) -> bool {
    let addrs = match (host, port).to_socket_addrs() {
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
    ui.window().set_fullscreen(true);

    // ── Configuration (live, shared with the web config server) ──────
    let cfg     = config::AppConfig::load();
    let port    = cfg.web_config_port;
    let cfg_ref = config::new_config_ref(cfg);

    // ── Local metrics (Pi) ───────────────────────────────────────────
    let mut collector = MetricsCollector::new();
    push_local_metrics(&ui, collector.collect());
    ui.set_clock_str(current_clock());

    // ── NAS + Home Assistant — start offline ─────────────────────────
    {
        let cfg = cfg_ref.read().unwrap();
        ui.set_nas_metrics(remote_metrics::offline_metrics(&cfg.nas_hostname, &cfg.nas_ip));
        ui.set_ha_metrics(remote_metrics::offline_metrics(&cfg.ha_hostname, &cfg.ha_ip));
    }

    // ── GitHub initial state — "not configured" until watcher runs ───
    ui.set_github_configured(false);
    ui.set_github_reachable(false);
    ui.set_github_level(0);
    ui.set_github_status("SETUP".into());
    ui.set_github_pr_count(0);
    ui.set_github_issue_count(0);
    ui.set_github_repos(slint::ModelRc::new(slint::VecModel::from(vec![])));
    ui.set_github_events(slint::ModelRc::new(slint::VecModel::from(vec![])));

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

    // ── Shared alert state ───────────────────────────────────────────
    let shared_alerts = shared_state::new_shared_alerts();

    // ── Network reachability probe ────────────────────────────────────
    let ui_weak_net    = ui.as_weak();
    let cfg_ref_net    = cfg_ref.clone();
    std::thread::spawn(move || loop {
        let (host, probe_port) = {
            let cfg = cfg_ref_net.read().unwrap();
            (cfg.probe_host.clone(), cfg.probe_port)
        };
        let result = std::panic::catch_unwind(|| probe_network(&host, probe_port));
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

    // ── Remote metrics (VPS) + system alerts ─────────────────────────
    let ui_weak_remote   = ui.as_weak();
    let shared_alerts_2  = shared_alerts.clone();
    let cfg_ref_remote   = cfg_ref.clone();
    std::thread::spawn(move || loop {
        let vps_settings = remote_metrics::VpsSettings::from_config(&cfg_ref_remote.read().unwrap());
        let (vps, sys_alerts) = match std::panic::catch_unwind(|| {
            (remote_metrics::fetch_vps(&vps_settings), remote_metrics::RemoteAlert::fetch_all())
        }) {
            Ok(pair) => pair,
            Err(_) => {
                eprintln!("remote_metrics: fetch thread panicked, skipping tick");
                std::thread::sleep(Duration::from_secs(15));
                continue;
            }
        };

        // Update system alerts in shared state
        let reachable = sys_alerts.is_some();
        {
            let mut guard = shared_alerts_2.lock().unwrap();
            guard.system = sys_alerts.unwrap_or_default().into_iter().map(|a| {
                shared_state::AlertData {
                    name:     a.name,
                    severity: a.severity,
                    age:      a.age,
                    summary:  a.summary,
                }
            }).collect();
        }

        let ui = ui_weak_remote.clone();
        if let Err(e) = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else { return };
            ui.set_vps_metrics(vps);
        }) {
            eprintln!("remote_metrics: event loop gone ({e:?}), exiting thread");
            return;
        }

        shared_state::push_alerts_to_ui(&ui_weak_remote, &shared_alerts_2, reachable);

        std::thread::sleep(Duration::from_secs(15));
    });

    // ── Fan controller telemetry (USB serial) ───────────────────────
    fan_telem::spawn(shared_alerts.clone(), ui.as_weak(), cfg_ref.clone());

    // ── Tokio runtime for async tasks (GitHub watcher + web config) ───
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let ui_weak_gh     = ui.as_weak();
    let cfg_ref_gh     = cfg_ref.clone();
    let shared_alerts_gh = shared_alerts.clone();
    rt.spawn(async move {
        github_watcher::run(ui_weak_gh, cfg_ref_gh, shared_alerts_gh).await;
    });

    rt.spawn(async move {
        web_config::serve(cfg_ref, port).await;
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
