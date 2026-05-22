use std::sync::{Arc, Mutex};

/// Plain Rust mirror of HudAlert (created on any thread; converted to Slint type on the UI thread).
#[derive(Clone, Default)]
pub struct AlertData {
    pub name:     String,
    pub severity: String,
    pub age:      String,
    pub summary:  String,
}

/// Holds alert sets from each source. Either source can update its half independently.
#[derive(Default)]
pub struct SharedAlerts {
    pub system: Vec<AlertData>,
    pub github: Vec<AlertData>,
}

impl SharedAlerts {
    pub fn merged(&self) -> Vec<AlertData> {
        let mut all = self.github.clone();
        all.extend(self.system.iter().cloned());
        all
    }
}

pub type SharedAlertsRef = Arc<Mutex<SharedAlerts>>;

pub fn new_shared_alerts() -> SharedAlertsRef {
    Arc::new(Mutex::new(SharedAlerts::default()))
}

/// Push the merged alert list to the UI. Must be called from any thread (uses invoke_from_event_loop).
pub fn push_alerts_to_ui(
    ui_weak:  &slint::Weak<crate::AppWindow>,
    alerts:   &SharedAlertsRef,
    reachable: bool,
) {
    let merged = alerts.lock().unwrap().merged();
    let ui = ui_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = ui.upgrade() else { return };

        let items: Vec<crate::HudAlert> = merged.iter().map(|a| crate::HudAlert {
            name:     a.name.clone().into(),
            severity: a.severity.clone().into(),
            age:      a.age.clone().into(),
            summary:  a.summary.clone().into(),
        }).collect();

        let count   = items.len() as i32;
        let worst   = merged.iter().map(|a| match a.severity.as_str() {
            "critical" => 2,
            "warning"  => 1,
            _          => 0,
        }).max().unwrap_or(0);

        ui.set_alerts_reachable(reachable);
        ui.set_alerts(slint::ModelRc::new(slint::VecModel::from(items)));
        ui.set_alert_count(count);
        ui.set_alert_worst_severity(worst);
    });
}
