#!/usr/bin/env bash
# Cross-compiles spacelab-hub for Raspberry Pi 4 (aarch64, glibc >= 2.35 / Bookworm).
# Requires: cargo-zigbuild, zig, and the aarch64 sysroot (run setup-pi-sysroot.sh first).
set -euo pipefail

SYSROOT="$HOME/.local/share/pi-sysroot-aarch64"

if [ ! -d "$SYSROOT/usr/lib/aarch64-linux-gnu" ]; then
  echo "Sysroot not found. Run scripts/setup-pi-sysroot.sh first."
  exit 1
fi

PKG_CONFIG_SYSROOT_DIR="$SYSROOT" \
PKG_CONFIG_LIBDIR="$SYSROOT/usr/lib/aarch64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig" \
PKG_CONFIG_ALLOW_CROSS=1 \
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-L $SYSROOT/usr/lib/aarch64-linux-gnu" \
  cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.35

echo ""
echo "Binary: target/aarch64-unknown-linux-gnu/release/spacelab-hub"
echo "Deploy: scp target/aarch64-unknown-linux-gnu/release/spacelab-hub pi@<host>:~/"
