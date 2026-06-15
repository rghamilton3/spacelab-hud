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

Everything is configured at runtime through the web config UI — no rebuild and no environment variables required. The app serves a small config page over HTTP; on the Pi, browse to `http://<pi-host>/` (port **80** by default).

| Section | Fields |
| --- | --- |
| **GitHub** | One source per org/account: a PAT (fine-grained recommended — the page lists the exact read-only permissions), an optional username (distinguishes your activity in the feed), and the repos to watch (picked from a live list or added manually as `owner/repo`). Plus a global poll interval (seconds, min 30). |
| **VPS / Beszel** | Base URL of your [Beszel](https://beszel.dev/) instance, the admin email + password used to obtain an API token, and the VPS system name / hostname / IP. |
| **Network probe** | Host and TCP port that the local-network reachability indicator connects to. |
| **NAS** and **Home Assistant** | The Beszel system name for each (they share the one Beszel instance above), plus hostname / IP. Leave a system name blank to keep that panel offline. |
| **Fan controller** | USB serial port and the rack-temperature warn / critical thresholds (°C). |

All monitored systems (VPS, NAS, Home Assistant) read from the **same** Beszel instance, differing only by system name — so the Beszel URL and admin credentials are entered once.

Settings are persisted to `~/.config/spacelab-hud/config.json` (mode `0600`). The web config port itself is **not** editable from the UI — change `web_config_port` in that JSON file (then restart) if port 80 is unavailable.

### Secret storage

The secret fields — the Beszel admin password and the GitHub PATs — are **encrypted at rest** (ChaCha20-Poly1305) before being written to `config.json`. The symmetric key is generated on first run and kept in a separate `0600` file at `~/.local/state/spacelab-hud/secret.key`, so a leaked config backup or stray commit doesn't also expose the key. Everything else in the config is stored as readable JSON.

This is proportionate protection for an unattended LAN device: because the HUD auto-boots and must decrypt with no human present, the key necessarily lives on the same machine — so this defends against *casual* exposure (backups, screen-shares, accidental commits), not against an attacker who already has read access to the Pi's filesystem. (The key layer is isolated in `src/secrets.rs` so it can later be upgraded to a TPM-sealed key.) If `secret.key` is lost, re-enter the secrets in the web config UI.

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
