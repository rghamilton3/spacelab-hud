# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Purpose

`spacelab-hud` is a homelab server rack monitor UI running on a Raspberry Pi 3 connected to a [Waveshare 4" Square HDMI Capacitive Touch LCD](https://www.waveshare.com/4inch-hdmi-lcd-c.htm) (720×720 resolution). The UI is built with Slint 1.16.1 in Rust.

## Build Commands

```bash
# Build (native/dev)
cargo build

# Run locally
cargo run

# Check without building
cargo check

# Release build for native
cargo build --release

# Cross-compile for Raspberry Pi 3 (aarch64, glibc 2.31)
scripts/build-pi.sh

# Cross-compile for Raspberry Pi 4 (aarch64, glibc 2.35 / Bookworm)
scripts/build-pi4.sh
```

## Cross-Compilation Setup

Cross-compilation uses `cargo-zigbuild` (not `cross`) because Slint's Docker image is based on Ubuntu 20.04 (glibc 2.31) but the host Rust toolchain generates build-script binaries that require newer glibc — causing a mismatch when `cross` runs them inside the container. `cargo-zigbuild` runs build scripts on the host and uses Zig as the cross-linker, avoiding the issue entirely.

### One-time setup

1. Install tools:
   ```bash
   cargo install cargo-zigbuild
   mise install zig && mise use zig   # or any method that puts zig in PATH
   rustup target add aarch64-unknown-linux-gnu
   ```

2. Extract the aarch64 sysroot (needs Docker — pulls Slint's cross image):
   ```bash
   scripts/setup-pi-sysroot.sh
   ```
   Sysroot lands in `~/.local/share/pi-sysroot-aarch64`. Re-run after major Slint upgrades.

### Building

```bash
scripts/build-pi.sh
```

Output: `target/aarch64-unknown-linux-gnu/release/spacelab-hub`.
- Pi 3 build (`build-pi.sh`): requires glibc ≥ 2.31 — compatible with both Pi 3 and Pi 4.
- Pi 4 build (`build-pi4.sh`): requires glibc ≥ 2.35 — Pi 4 Bookworm only; also produced by CI.

The binary dynamically links only `libfontconfig.so.1` and standard libc; Wayland/EGL/GLES are loaded via `dlopen` at runtime, so the Pi's native libs are used without bundling them.

## Architecture

The project follows Slint's compile-time UI model:

- **`ui/app-window.slint`** — All UI markup lives here. `build.rs` compiles it at build time via `slint_build::compile`. Slint generates Rust types from this file.
- **`build.rs`** — Invokes `slint_build::compile("ui/app-window.slint")`. Add additional `.slint` files here if the UI is split.
- **`src/main.rs`** — Rust entry point. Calls `slint::include_modules!()` to bring generated types into scope, instantiates `AppWindow`, wires callbacks, and calls `ui.run()`.

The Slint data-binding model means UI state flows via properties (`get_*` / `set_*`) and events flow via callbacks (`on_*`). Logic that needs to run on the Rust side is registered as a callback handler before `ui.run()`.

## Display Constraints

Target display is 720×720 pixels, touch-enabled. Design UI components with:
- Square aspect ratio
- Touch-friendly tap targets (minimum ~44px)
- No mouse hover states (touch-only device)
- The Pi may render without a window manager; Slint's `linuxkms` or `wayland` backend may be needed depending on the OS setup

## CI

GitHub Actions (`.github/workflows/ci.yml`) triggers on every push to `main`, on pull requests, and on version tags (`v*`):

- **Check & Clippy** — `cargo check` + `cargo clippy` on the native host. Runs on `main` pushes, PRs, and tags.
- **Build Pi 4** — cross-compiles the release binary for `aarch64-unknown-linux-gnu.2.35` (Pi 4 Bookworm). Runs **only on version tags** (`refs/tags/v*`): uploads the binary as a workflow artifact and attaches it to the tag's GitHub Release.

To release: `git tag v0.x.y && git push --tags`. The release binary will appear at the tag's GitHub Release page.

To force a sysroot cache refresh (e.g. after a major Slint version bump), increment `SYSROOT_CACHE_VERSION` in the workflow file.

## Slint IDE Integration

Install the [Slint VS Code extension](https://marketplace.visualstudio.com/items?itemName=Slint.slint) for `.slint` file previewing, syntax highlighting, and the LSP. The extension provides live preview of `.slint` components without building.
