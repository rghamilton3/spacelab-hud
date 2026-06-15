# Plan: Dynamic Beszel-driven screens

## Goal

Stop hardcoding the VPS / NAS / HA health screens. Instead, enumerate the
systems the Beszel hub reports and build **one health screen per system** at
runtime. GitHub, Alerts, the Overview, and the local Pi "System Info" screen
stay as fixed screens.

Per the agreed decisions:

- **Scope:** show **all** systems the hub reports, with config to **reorder**
  and **hide** specific ones.
- **Local screen:** keep the local Pi "System Info" as its own fixed,
  non-Beszel screen.
- **Overview:** keep the existing fixed overview layout, but feed its three
  remote card slots from the **first N = 3** dynamic systems instead of named
  VPS/NAS/HA.

## Carousel layout (before → after)

Today (`ui/app-window.slint`), `page-count: 7`, fixed indices:

```
0 Overview  1 GitHub  2 VPS  3 NAS  4 HA  5 SystemInfo(local)  6 Alerts
```

After — fixed screens bracket a dynamic middle block:

```
0 Overview  1 GitHub  [2 .. 2+N) dynamic systems  (2+N) SystemInfo(local)  (3+N) Alerts
```

where `N = systems.length`. `page-count` becomes `4 + N`.

Rationale for placement: GitHub stays at a fixed low index (overview taps to it
directly); the dynamic block is contiguous starting at page 2 so a repeater can
position each screen at `(2 + idx) * width`; the two trailing fixed screens
(local, alerts) compute their x from the live system count.

---

## 1. Slint UI

### 1a. New `ui/health-screen.slint` (generalize the three panels)

`vps-health.slint`, `nas-health.slint`, `ha-health.slint` are near-identical —
they differ only in the header title (`"SPACELAB-1 // VPS"`) and which metrics
property they bind. Replace all three with one component:

```slint
export component HealthScreen {
    in property <ServiceMetrics> metrics;
    in property <string>         title;        // e.g. "VPS" / system name
    in property <bool>           show-back:    false;
    in property <bool>           show-forward: false;
    // ... identical body, header text: "SPACELAB-1 // " + root.title
}
```

`title` comes from the system name. `ServiceMetrics.hostname` already carries
the Beszel system name (see `remote_metrics::offline_metrics`), so the repeater
can pass `title: systems[idx].hostname`. (Uppercasing, if wanted, is done in
Rust when building the model — Slint has no string-upper.)

Delete `vps-health.slint`, `nas-health.slint`, `ha-health.slint`.

### 1b. `ui/app-window.slint`

**Properties** — replace the three scalar metrics with one model, and take a
Rust-computed worst-level scalar (Slint can't fold a model to a max):

```slint
in property <[ServiceMetrics]> systems;
in property <int>              systems-worst-level;   // computed in Rust
```

Remove `vps-metrics` / `nas-metrics` / `ha-metrics` and the `vps-/nas-/ha-worst`
+ `vps-/nas-/ha-level` derived blocks. `overall-level` becomes:

```slint
property <int> overall-level:
    Math.max(Math.max(pi-level, systems-worst-level),
             Math.max(alerts-rollup-level, github-level));
```

**Page count & swipe bounds:**

```slint
property <int> page-count: 4 + root.systems.length;
```

**Screens container** — fixed screens plus a repeater:

```slint
OverviewScreen { x: 0 * root.width; /* … systems, systems-worst-level … */ }
GithubScreen   { x: 1 * root.width; /* … */ }

for system[idx] in root.systems : HealthScreen {
    x: (2 + idx) * root.width;
    width: root.width; height: parent.height;
    metrics: system;
    title:   system.hostname;
    show-back: true; show-forward: true;
}

SystemInfoScreen { x: (2 + root.systems.length) * root.width; /* … */ }
AlertsScreen     { x: (3 + root.systems.length) * root.width; /* … */ }
```

The `states`/animation block keys off `current-page * width` and is unchanged.

**Overview tap navigation** (the `abs-delta < 20px` branch for page 0): replace
hardcoded page numbers with counts derived from `root.systems.length`:

| Tap region            | Target page              |
|-----------------------|--------------------------|
| Header right edge     | `1` (GitHub)             |
| Row 1 left (LOCAL)    | `2 + systems.length`     |
| Row 1 right (slot 0)  | `2`                      |
| Row 2 left (slot 1)   | `3`                      |
| Row 2 mid (slot 2)    | `4`                      |
| Row 2 right (GITHUB)  | `1`                      |
| Alerts row            | `3 + systems.length`     |

Guard the slot taps so they only fire when that system exists
(`idx < systems.length`).

### 1c. `ui/overview.slint`

Keep the layout (Row 1: LOCAL | slot0; Row 2: slot1 | slot2 | GITHUB; Alerts
row). Replace the `vps-metrics`/`nas-metrics`/`ha-metrics` inputs with:

```slint
in property <[ServiceMetrics]> systems;
```

The three remote `StoplightCard`s bind to `systems[0]`, `systems[1]`,
`systems[2]`, with `name: systems[i].hostname`. The per-card `level`/`detail`
inline the same worst→level expression currently used, reading `systems[i].*`.

Empty-slot handling: when there are fewer than 3 systems, `systems[i]` for an
out-of-range `i` yields a default `ServiceMetrics` (`reachable=false`). Set each
remote card's `visible: i < systems.length` so unused slots collapse rather than
render a misleading "unreachable" tile. (LOCAL, GITHUB, ALERTS are always
visible.)

---

## 2. Rust — Beszel enumeration (`src/remote_metrics.rs`)

**Replace** the per-target `BeszelTarget` + `vps()`/`nas()`/`ha()` builders with
a hub-level fetch.

New public surface:

```rust
/// One system as discovered on the hub (pre-stats).
struct SystemBrief { id: String, name: String, uptime_secs: u64, load: Vec<f64> }

/// List every system the hub knows about (no name filter).
fn fetch_all_systems(url, email, password) -> Option<Vec<SystemBrief>>
//   GET /api/collections/systems/records?perPage=200&sort=name
//   reuses get_or_refresh_token + 401 invalidation already present

/// Build the ordered, filtered Vec<ServiceMetrics> for the UI.
pub fn fetch_systems(
    url: &str, email: &str, password: &str,
    order: &[String], hidden: &[String], hub_alive: bool,
) -> Vec<crate::ServiceMetrics>
```

`fetch_systems`:

1. If hub creds blank or `!hub_alive` → return `vec![]` (carousel shows just the
   fixed screens).
2. `fetch_all_systems(...)`; on `None` → `vec![]`.
3. Drop any system whose name is in `hidden`.
4. Sort: names listed in `order` come first in that order; the rest follow in
   hub order (stable). New systems on the hub therefore appear automatically,
   after the ordered ones.
5. For each remaining system, fetch latest 1-minute stats by id (the existing
   step-2 query, factored out to take a `system_id`) and assemble
   `ServiceMetrics` (reuse the existing field-mapping; `online_no_metrics` on
   stats miss).

The ordering/filter step is a pure function → unit-testable without network.

Keep `beszel_alive`, `percent_encode`, `fmt_uptime`, `offline_metrics`,
`online_no_metrics`, the auth-token cache, and the `RemoteAlert` stub as-is.

---

## 3. Rust — config (`src/config.rs`)

Add:

```rust
/// Ordering prefix: systems named here sort first, in this order. Unlisted
/// hub systems follow in hub order.
#[serde(default)] pub system_order: Vec<String>,
/// Systems to hide entirely (by Beszel name).
#[serde(default)] pub hidden_systems: Vec<String>,
```

Demote the three name fields to legacy migration-only (same pattern as the
GitHub legacy fields):

```rust
#[serde(default, rename = "vps_name", skip_serializing)] legacy_vps_name: String,
#[serde(default, rename = "nas_name", skip_serializing)] legacy_nas_name: String,
#[serde(default, rename = "ha_name",  skip_serializing)] legacy_ha_name:  String,
```

Extend `migrate()`: if `system_order` is empty, seed it from the non-empty
legacy names in `[vps, nas, ha]` order, then take them. This preserves today's
VPS→NAS→HA screen ordering across the upgrade, and the legacy keys drop on the
next save (verified by an analogue of `saved_config_drops_legacy_fields`).

`beszel_url` / `beszel_email` / `beszel_password` are unchanged. No secret-field
changes (`hidden_systems`/`system_order` are not secrets).

---

## 4. Rust — wiring (`src/main.rs`)

**Startup:** replace the NAS/HA offline seeding with an empty systems model:

```rust
ui.set_systems(ModelRc::new(VecModel::from(vec![])));
ui.set_systems_worst_level(0);
```

**Remote loop:** snapshot hub creds + `system_order` + `hidden_systems` under
one read lock; then off-lock:

```rust
let hub_alive = remote_metrics::beszel_alive(&url);
let systems   = remote_metrics::fetch_systems(&url, &email, &pw, &order, &hidden, hub_alive);
let worst     = systems.iter().map(level_of).max().unwrap_or(0);
```

`level_of` applies the same worst→level rule the Slint side used
(`!reachable → 2`, `>0.85 → 2`, `>0.70 → 1`, else `0`). Push to the UI via
`invoke_from_event_loop`: `set_systems(VecModel)` + `set_systems_worst_level`.
Cadence stays 15 s; the alert stub path is untouched.

---

## 5. Rust — web config (`src/web_config.rs`)

Replace the three fixed "BESZEL SYSTEM NAME" inputs (VPS/NAS/HA sections) with a
**discovered-systems manager**:

- On render, call `fetch_all_systems(...)` against the configured hub. For each
  discovered system show: name, a **Hide** checkbox (checked if in
  `hidden_systems`), and an **order index** input (position in `system_order`).
- Persist back into `system_order` (visible systems in chosen order) and
  `hidden_systems`.
- **Fallback** when the hub is unreachable / unconfigured: render the saved
  `system_order` and `hidden_systems` as plain editable text lists so config is
  never lost or uneditable.

Drag-reorder is out of scope for v1 — numeric order index + hide checkbox is
enough and keeps the form server-rendered with no new JS. Update the form
struct (`vps_name`/`nas_name`/`ha_name` fields removed) and the save handler
accordingly.

---

## 6. Tests

- `config.rs`: legacy `vps_name`/`nas_name`/`ha_name` → `system_order`
  migration; saved config drops the legacy keys.
- `remote_metrics.rs`: ordering/hide pure function — listed-first ordering,
  unlisted appended in hub order, hidden removed. Keep the existing
  `percent_encode` test.

## 7. Housekeeping

- Bump version in `Cargo.toml`; commit the regenerated `Cargo.lock` in the same
  change (repo convention).
- `cargo check` + `cargo clippy` clean; native `cargo build`.
- Feature branch + PR against `main`.

---

## Risks / call-outs

- **Slint model indexing in Overview** (`systems[i]`) with visibility guards is
  the least-certain bit; if Slint balks at out-of-range indexing in bindings,
  fall back to a small fixed `overview-cards` model (length ≤ 3) built in Rust.
- **Request volume:** `fetch_systems` now does `auth + list + N×stats` per 15 s
  cycle (was a fixed 3). Fine for a homelab handful; if a hub grows large,
  hidden systems still incur the stats fetch unless we filter before step 5
  (we do — hidden are dropped in step 3).
- **N = 3 overview cap** is a deliberate UI constraint; systems beyond the first
  three are reachable only by swiping to their screens, not from the overview.
