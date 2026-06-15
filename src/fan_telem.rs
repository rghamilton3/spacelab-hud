use std::fs::File;
use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde::Deserialize;

use crate::config::ConfigRef;
use crate::shared_state::{AlertData, SharedAlertsRef};

const RECONNECT_WAIT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct Frame {
    temp_c:  f32,
    temp_ok: bool,
    rpm_ok:  bool,
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn parse_line(line: &str) -> Option<Frame> {
    let rest = line.strip_prefix("SLT1 ")?;
    let star = rest.rfind('*')?;
    let json = &rest[..star];
    let hex  = &rest[star + 1..];
    if hex.len() != 2 { return None; }
    let expected = u8::from_str_radix(hex, 16).ok()?;
    if crc8(json.as_bytes()) != expected {
        eprintln!("fan_telem: CRC mismatch, dropping line");
        return None;
    }
    serde_json::from_str(json).ok()
}

fn alerts_for(frame: &Frame, temp_warn_c: f32, temp_crit_c: f32) -> Vec<AlertData> {
    let mut out = Vec::new();
    if !frame.temp_ok {
        out.push(AlertData {
            name:     "RACK TEMP SENSOR".into(),
            severity: "warning".into(),
            age:      "now".into(),
            summary:  "Fan controller: temperature sensor read failed — value stale".into(),
        });
    } else if frame.temp_c >= temp_crit_c {
        out.push(AlertData {
            name:     "RACK TEMP".into(),
            severity: "critical".into(),
            age:      "now".into(),
            summary:  format!("Rack temperature {:.1}°C — exceeds {:.0}°C critical threshold", frame.temp_c, temp_crit_c),
        });
    } else if frame.temp_c >= temp_warn_c {
        out.push(AlertData {
            name:     "RACK TEMP".into(),
            severity: "warning".into(),
            age:      "now".into(),
            summary:  format!("Rack temperature {:.1}°C — exceeds {:.0}°C warning threshold", frame.temp_c, temp_warn_c),
        });
    }
    if !frame.rpm_ok {
        out.push(AlertData {
            name:     "FAN RPM".into(),
            severity: "warning".into(),
            age:      "now".into(),
            summary:  "Fan controller: RPM reading unavailable".into(),
        });
    }
    out
}

pub fn spawn(
    shared:     SharedAlertsRef,
    ui_weak:    slint::Weak<crate::AppWindow>,
    config_ref: ConfigRef,
) {
    std::thread::spawn(move || loop {
        // Re-read config each reconnect cycle so edits via the web UI apply live.
        let (serial_port, temp_warn_c, temp_crit_c) = {
            let cfg = config_ref.read().unwrap();
            (cfg.fan_serial_port.clone(), cfg.fan_temp_warn_c, cfg.fan_temp_crit_c)
        };

        let file = match File::open(&serial_port) {
            Ok(f) => {
                eprintln!("fan_telem: connected to {serial_port}");
                {
                    let mut g = shared.lock().unwrap();
                    g.fan = vec![];
                }
                crate::shared_state::push_alerts_to_ui(&ui_weak, &shared, true);
                f
            }
            Err(e) => {
                eprintln!("fan_telem: cannot open {serial_port}: {e}");
                {
                    let mut g = shared.lock().unwrap();
                    g.fan = vec![AlertData {
                        name:     "FAN CTRL".into(),
                        severity: "critical".into(),
                        age:      "now".into(),
                        summary:  "Fan controller not connected (USB device absent)".into(),
                    }];
                }
                crate::shared_state::push_alerts_to_ui(&ui_weak, &shared, true);
                std::thread::sleep(RECONNECT_WAIT);
                continue;
            }
        };

        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(l)  => l,
                Err(e) => { eprintln!("fan_telem: read error: {e}"); break; }
            };
            let Some(frame) = parse_line(&line) else { continue };
            {
                let mut g = shared.lock().unwrap();
                g.fan = alerts_for(&frame, temp_warn_c, temp_crit_c);
            }
            crate::shared_state::push_alerts_to_ui(&ui_weak, &shared, true);
        }

        eprintln!("fan_telem: connection lost, reconnecting in {RECONNECT_WAIT:?}");
        std::thread::sleep(RECONNECT_WAIT);
    });
}
