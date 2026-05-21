# SpaceLab HUD

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
