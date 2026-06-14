# SpaceLab HUD

[![CI](https://github.com/rghamilton3/spacelab-hud/actions/workflows/ci.yml/badge.svg)](https://github.com/rghamilton3/spacelab-hud/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/rghamilton3/spacelab-hud?sort=semver&logo=github)](https://github.com/rghamilton3/spacelab-hud/releases/latest)
[![License: MIT](https://img.shields.io/github/license/rghamilton3/spacelab-hud)](LICENSE)
[![Last commit](https://img.shields.io/github/last-commit/rghamilton3/spacelab-hud)](https://github.com/rghamilton3/spacelab-hud/commits/main)

[![Rust](https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/Slint-1.16-2379f4?logo=slint&logoColor=white)](https://slint.rs/)
[![Platform](https://img.shields.io/badge/platform-Raspberry%20Pi%204%20%C2%B7%20aarch64-c51a4a?logo=raspberrypi&logoColor=white)](https://www.raspberrypi.com/)
[![Display](https://img.shields.io/badge/display-720%C3%97720%20touch-444444)](https://www.waveshare.com/4inch-hdmi-lcd-c.htm)

A homelab server rack monitor UI running on a Raspberry Pi 4, built with [Slint](https://slint.rs/) and Rust. Displays real-time system metrics on a [Waveshare 4" Square HDMI Capacitive Touch LCD](https://www.waveshare.com/4inch-hdmi-lcd-c.htm) (720×720).

## What it shows

A multi-screen, swipe-and-tap touch UI. Each screen surfaces a different slice of the homelab:

- **Local system** — the Pi's own hostname, IP, uptime, load, CPU usage/temp, RAM, disk, and network throughput.
- **Remote hosts** — a VPS (via [Beszel](https://beszel.dev/)), plus NAS and Home Assistant tiles, each with reachability and metrics.
- **GitHub** — CI run status, open PR and issue counts, and a recent-activity feed for a configured set of watched repos.
- **Alerts** — rack temperature / fan-RPM anomalies pushed from a fan controller over USB serial.

## Configuration

Configuration lives in three places, depending on how often a value changes.

### 1. Web config UI (runtime, no rebuild)

The app serves a small config page over HTTP. On the Pi, browse to `http://<pi-host>/` (port **80** by default) to set:

| Field | Description |
| --- | --- |
| **GitHub PAT** | Fine-grained (recommended) or classic token. The page lists the exact read-only permissions to grant. |
| **GitHub username** | Distinguishes your activity from others in the feed. |
| **Repos to watch** | Picked from a live list fetched with the PAT, or added manually as `owner/repo`. |
| **Poll interval** | Seconds between GitHub polls (min 30). |

Settings are persisted to `~/.config/spacelab-hud/config.json`. The web config port itself is **not** editable from the UI — change `web_config_port` in that JSON file (then restart) if port 80 is unavailable.

### 2. Environment variables

Set before launching the app (e.g. in the systemd unit or launch script):

| Variable | Purpose |
| --- | --- |
| `BESZEL_ADMIN_EMAIL` | Beszel admin login for VPS metrics. |
| `BESZEL_ADMIN_PASSWORD` | Beszel admin password. Without these two, the VPS tile shows reachable-but-no-metrics. |

### 3. Compile-time constants (require rebuild)

Host-specific addresses are hardcoded as `const`s — edit the source and recompile to change them:

| Location | Constants |
| --- | --- |
| `src/main.rs` | `NETWORK_PROBE_HOST` / `_PORT` (reachability probe), and the NAS / Home Assistant hostnames + IPs passed to `offline_metrics`. |
| `src/remote_metrics.rs` | `BESZEL_HOST`, `VPS_SYSTEM_NAME`, `VPS_HOSTNAME`, `VPS_IP_ADDR`. |
| `src/fan_telem.rs` | `SERIAL_PORT` (default `/dev/ttyACM0`), `TEMP_WARN_C` (35 °C), `TEMP_CRIT_C` (40 °C). |

## Building

```bash
# Native (dev/preview)
cargo run

# Cross-compile for Pi 3 (glibc ≥ 2.31)
scripts/build-pi.sh

# Cross-compile for Pi 4 / Bookworm (glibc ≥ 2.35)
scripts/build-pi4.sh
```

### Cross-compilation setup (one-time)

```bash
cargo install cargo-zigbuild
mise install zig && mise use zig        # or any method that puts zig in PATH
rustup target add aarch64-unknown-linux-gnu
scripts/setup-pi-sysroot.sh             # pulls Slint's Docker image and extracts sysroot
```

## Releasing

Push a version tag — CI builds the Pi 4 binary and publishes a GitHub Release automatically:

```bash
git tag v0.x.y && git push --tags
```

## License

MIT
