# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Purpose

`spacelab-hud` is a homelab server rack monitor UI running on a Raspberry Pi 3 connected to a [Waveshare 4" Square HDMI Capacitive Touch LCD](https://www.waveshare.com/4inch-hdmi-lcd-c.htm) (720×720 resolution). The UI is built with Slint 1.14.1 in Rust.

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

# Cross-compile for Raspberry Pi 3 (aarch64)
cargo build --release --target aarch64-unknown-linux-gnu
```

## Cross-Compilation Setup

Cross-compilation targets the Pi 3 (ARM Cortex-A53, 64-bit). Use Slint's official cross-compilation config at:
https://github.com/slint-ui/slint/blob/master/Cross.toml

Steps to set up cross-compilation:
1. Install `cross`: `cargo install cross`
2. Add target: `rustup target add aarch64-unknown-linux-gnu`
3. Place or reference Slint's `Cross.toml` at the repo root
4. Build: `cross build --release --target aarch64-unknown-linux-gnu`

The `.cargo/config.toml` currently only defines Windows stack-size flags; Pi-specific linker config goes there under `[target.aarch64-unknown-linux-gnu]`.

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

## Slint IDE Integration

Install the [Slint VS Code extension](https://marketplace.visualstudio.com/items?itemName=Slint.slint) for `.slint` file previewing, syntax highlighting, and the LSP. The extension provides live preview of `.slint` components without building.
