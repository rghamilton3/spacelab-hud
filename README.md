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
